//! The PC's displays, so a TV's absolute pointer can map onto the display it shows.

use super::from_wide;
use std::sync::RwLock;
use windows_sys::Win32::Devices::Display::*;
use windows_sys::Win32::Foundation::{LPARAM, RECT};
use windows_sys::Win32::Graphics::Gdi::*;
use windows_sys::Win32::UI::WindowsAndMessaging::MONITORINFOF_PRIMARY;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct Rect {
    pub x: i32,
    pub y: i32,
    pub width: i32,
    pub height: i32,
}

impl Rect {
    fn union(self, o: Rect) -> Rect {
        if self.width <= 0 || self.height <= 0 {
            return o;
        }
        let (l, t) = (self.x.min(o.x), self.y.min(o.y));
        let (r, b) = ((self.x + self.width).max(o.x + o.width), (self.y + self.height).max(o.y + o.height));
        Rect { x: l, y: t, width: r - l, height: b - t }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Screen {
    /// Display target id, the value stored as `Device\<tv>\Display` in the registry.
    pub id: u32,
    /// 1-based, in the order Windows lists the displays.
    pub number: u32,
    pub name: String,
    pub active: bool,
    pub primary: bool,
    /// In physical pixels (the process is per-monitor DPI aware).
    pub bounds: Rect,
}

#[derive(Debug, Clone, Default)]
pub struct Screens {
    pub all: Vec<Screen>,
    pub desktop: Rect,
}

impl Screens {
    /// Normalized `SendInput` coordinates for a point on the TV's 1920x1080 grid shown on
    /// display `id` (the primary display when `id` is absent or inactive).
    pub fn absolute(&self, id: Option<u32>, x: u16, y: u16) -> Option<(i32, i32)> {
        let screen = id.and_then(|id| self.all.iter().find(|s| s.id == id && s.active)).or_else(|| self.all.iter().find(|s| s.primary))?;
        normalize(screen.bounds, self.desktop, x, y)
    }
}

/// Maps a 1920x1080 grid point inside `display` to 0..=65535 over `desktop`.
pub fn normalize(display: Rect, desktop: Rect, x: u16, y: u16) -> Option<(i32, i32)> {
    if desktop.width <= 0 || desktop.height <= 0 {
        return None;
    }
    let px = display.x - desktop.x + (display.width as i64 * x as i64 / 1920) as i32;
    let py = display.y - desktop.y + (display.height as i64 * y as i64 / 1080) as i32;
    // Pixel centres, so the last pixel row and column are reachable but never overshot
    let nx = ((px as i64 * 65536 + 32768) / desktop.width as i64).clamp(0, 65535);
    let ny = ((py as i64 * 65536 + 32768) / desktop.height as i64).clamp(0, 65535);
    Some((nx as i32, ny as i32))
}

static CURRENT: RwLock<Option<Screens>> = RwLock::new(None);

/// `Screens::absolute` on the current displays, without copying them.
pub fn absolute(id: Option<u32>, x: u16, y: u16) -> Option<(i32, i32)> {
    if let Some(s) = CURRENT.read().unwrap_or_else(|e| e.into_inner()).as_ref() {
        return s.absolute(id, x, y);
    }
    refresh().absolute(id, x, y)
}

pub fn current() -> Screens {
    if let Some(s) = CURRENT.read().unwrap_or_else(|e| e.into_inner()).as_ref() {
        return s.clone();
    }
    refresh()
}

/// Re-reads the displays (call on WM_DISPLAYCHANGE). Keeps the previous state on failure.
pub fn refresh() -> Screens {
    let mut cur = CURRENT.write().unwrap_or_else(|e| e.into_inner());
    if let Some(s) = query() {
        *cur = Some(s);
    }
    cur.clone().unwrap_or_default()
}

struct Monitor {
    device: String,
    bounds: Rect,
    primary: bool,
}

unsafe extern "system" fn collect(hmon: HMONITOR, _: HDC, _: *mut RECT, data: LPARAM) -> i32 {
    let list = &mut *(data as *mut Vec<Monitor>);
    let mut mi: MONITORINFOEXW = std::mem::zeroed();
    mi.monitorInfo.cbSize = std::mem::size_of::<MONITORINFOEXW>() as u32;
    if GetMonitorInfoW(hmon, &mut mi as *mut MONITORINFOEXW as *mut MONITORINFO) != 0 {
        let r = mi.monitorInfo.rcMonitor;
        list.push(Monitor {
            device: from_wide(&mi.szDevice),
            bounds: Rect { x: r.left, y: r.top, width: r.right - r.left, height: r.bottom - r.top },
            primary: mi.monitorInfo.dwFlags & MONITORINFOF_PRIMARY != 0,
        });
    }
    1
}

fn query() -> Option<Screens> {
    unsafe {
        let mut monitors: Vec<Monitor> = Vec::new();
        EnumDisplayMonitors(std::ptr::null_mut(), std::ptr::null(), Some(collect), &mut monitors as *mut _ as LPARAM);

        let (mut np, mut nm) = (0u32, 0u32);
        if GetDisplayConfigBufferSizes(QDC_ALL_PATHS, &mut np, &mut nm) != 0 {
            return None;
        }
        let mut paths = vec![std::mem::zeroed::<DISPLAYCONFIG_PATH_INFO>(); np as usize];
        let mut modes = vec![std::mem::zeroed::<DISPLAYCONFIG_MODE_INFO>(); nm as usize];
        if QueryDisplayConfig(QDC_ALL_PATHS, &mut np, paths.as_mut_ptr(), &mut nm, modes.as_mut_ptr(), std::ptr::null_mut()) != 0 {
            return None;
        }
        paths.truncate(np as usize);

        let mut all: Vec<Screen> = Vec::new();
        for p in &paths {
            if p.targetInfo.targetAvailable == 0 {
                continue;
            }
            let id = p.targetInfo.id;
            if !all.iter().any(|s| s.id == id) {
                let mut target: DISPLAYCONFIG_TARGET_DEVICE_NAME = std::mem::zeroed();
                target.header.r#type = DISPLAYCONFIG_DEVICE_INFO_GET_TARGET_NAME;
                target.header.size = std::mem::size_of::<DISPLAYCONFIG_TARGET_DEVICE_NAME>() as u32;
                target.header.adapterId = p.targetInfo.adapterId;
                target.header.id = id;
                let name = if DisplayConfigGetDeviceInfo(&mut target.header) == 0 { from_wide(&target.monitorFriendlyDeviceName) } else { String::new() };
                all.push(Screen { id, number: all.len() as u32 + 1, name, active: false, primary: false, bounds: Rect::default() });
            }
            if p.flags & DISPLAYCONFIG_PATH_ACTIVE == 0 {
                continue;
            }
            let mut source: DISPLAYCONFIG_SOURCE_DEVICE_NAME = std::mem::zeroed();
            source.header.r#type = DISPLAYCONFIG_DEVICE_INFO_GET_SOURCE_NAME;
            source.header.size = std::mem::size_of::<DISPLAYCONFIG_SOURCE_DEVICE_NAME>() as u32;
            source.header.adapterId = p.sourceInfo.adapterId;
            source.header.id = p.sourceInfo.id;
            if DisplayConfigGetDeviceInfo(&mut source.header) != 0 {
                continue;
            }
            let gdi = from_wide(&source.viewGdiDeviceName);
            if let (Some(m), Some(s)) = (monitors.iter().find(|m| m.device == gdi), all.iter_mut().find(|s| s.id == id)) {
                s.active = true;
                s.primary = m.primary;
                s.bounds = m.bounds;
            }
        }
        let desktop = all.iter().filter(|s| s.active).fold(Rect::default(), |acc, s| acc.union(s.bounds));
        Some(Screens { all, desktop })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const FHD: Rect = Rect { x: 0, y: 0, width: 1920, height: 1080 };
    const UHD_RIGHT: Rect = Rect { x: 1920, y: -500, width: 3840, height: 2160 };

    #[test]
    fn maps_the_tv_grid_onto_one_display() {
        assert_eq!(normalize(FHD, FHD, 0, 0), Some((17, 30)));
        assert_eq!(normalize(FHD, FHD, 960, 540), Some((32785, 32798)));
        assert_eq!(normalize(FHD, FHD, 1919, 1079), Some((65518, 65505)));
    }

    #[test]
    fn maps_onto_the_right_part_of_a_multi_display_desktop() {
        let desktop = FHD.union(UHD_RIGHT);
        assert_eq!(desktop, Rect { x: 0, y: -500, width: 5760, height: 2160 });
        let screens = Screens {
            all: vec![
                Screen { id: 1, number: 1, name: "PC".into(), active: true, primary: true, bounds: FHD },
                Screen { id: 2, number: 2, name: "TV".into(), active: true, primary: false, bounds: UHD_RIGHT },
                Screen { id: 3, number: 3, name: "Off".into(), active: false, primary: false, bounds: Rect::default() },
            ],
            desktop,
        };
        // Centre of the TV display: pixel (1920 + 1920, -500 + 1080) -> desktop offset (3840, 1080)
        let (x, y) = screens.absolute(Some(2), 960, 540).unwrap();
        assert_eq!((x * 5760 / 65536, y * 2160 / 65536), (3840, 1080));
        // An inactive or unknown display falls back to the primary
        assert_eq!(screens.absolute(Some(3), 0, 0), screens.absolute(None, 0, 0));
        assert_eq!(screens.absolute(Some(9), 0, 0), screens.absolute(Some(1), 0, 0));
    }

    #[test]
    fn no_desktop_means_no_absolute_moves() {
        assert_eq!(normalize(FHD, Rect::default(), 1, 1), None);
        assert_eq!(Screens::default().absolute(None, 1, 1), None);
    }
}
