//! The notification-area icon and the hidden window that receives power, display and
//! taskbar events. Runs the message loop on the calling thread.

use super::{screens, wide};
use std::sync::atomic::{AtomicBool, AtomicPtr, AtomicU32, Ordering};
use std::sync::OnceLock;
use windows_sys::core::GUID;
use windows_sys::Win32::Foundation::{HWND, LPARAM, LRESULT, POINT, WPARAM};
use windows_sys::Win32::System::LibraryLoader::GetModuleHandleW;
use windows_sys::Win32::System::Power::{RegisterPowerSettingNotification, POWERBROADCAST_SETTING};
use windows_sys::Win32::UI::Shell::*;
use windows_sys::Win32::UI::WindowsAndMessaging::*;

pub struct Config {
    pub on_settings: Box<dyn Fn() + Send + Sync>,
    /// The PC's monitor turned on (true) or off (false).
    pub on_monitor: Box<dyn Fn(bool) + Send + Sync>,
    /// Offer "Exit" (not when the service owns this process).
    pub can_exit: bool,
}

const GUID_MONITOR_POWER_ON: GUID = GUID::from_u128(0x02731015_4510_4526_99e6_e5a17ebd1aea);
const WM_TRAY: u32 = WM_APP + 1;
const WM_SET_TIP: u32 = WM_APP + 2;
const PBT_POWERSETTINGCHANGE: usize = 0x8013;
const ID_SETTINGS: usize = 1;
const ID_EXIT: usize = 2;

static CONFIG: OnceLock<Config> = OnceLock::new();
static HWND_MAIN: AtomicPtr<std::ffi::c_void> = AtomicPtr::new(std::ptr::null_mut());
static TASKBAR_CREATED: AtomicU32 = AtomicU32::new(0);
static TVS: AtomicU32 = AtomicU32::new(0);
static QUIT_REQUESTED: AtomicBool = AtomicBool::new(false);

fn tip_text() -> String {
    match TVS.load(Ordering::Relaxed) {
        0 => "MagicRemoteService: no TV connected".into(),
        1 => "MagicRemoteService: 1 TV connected".into(),
        n => format!("MagicRemoteService: {n} TVs connected"),
    }
}

fn same_guid(a: &GUID, b: &GUID) -> bool {
    (a.data1, a.data2, a.data3, a.data4) == (b.data1, b.data2, b.data3, b.data4)
}

unsafe fn icon_data(hwnd: HWND) -> NOTIFYICONDATAW {
    let mut nid: NOTIFYICONDATAW = std::mem::zeroed();
    nid.cbSize = std::mem::size_of::<NOTIFYICONDATAW>() as u32;
    nid.hWnd = hwnd;
    nid.uID = 1;
    nid
}

unsafe fn add_icon(hwnd: HWND) {
    let mut nid = icon_data(hwnd);
    nid.uFlags = NIF_MESSAGE | NIF_ICON | NIF_TIP;
    nid.uCallbackMessage = WM_TRAY;
    // Icon resource 1 (resources/app.rc)
    nid.hIcon = LoadIconW(GetModuleHandleW(std::ptr::null()), 1 as _);
    let tip = wide(&tip_text());
    nid.szTip[..tip.len().min(127)].copy_from_slice(&tip[..tip.len().min(127)]);
    Shell_NotifyIconW(NIM_ADD, &nid);
}

unsafe fn set_tip(hwnd: HWND) {
    let mut nid = icon_data(hwnd);
    nid.uFlags = NIF_TIP;
    let tip = wide(&tip_text());
    nid.szTip[..tip.len().min(127)].copy_from_slice(&tip[..tip.len().min(127)]);
    Shell_NotifyIconW(NIM_MODIFY, &nid);
}

unsafe fn show_menu(hwnd: HWND, cfg: &Config) {
    let menu = CreatePopupMenu();
    AppendMenuW(menu, MF_STRING, ID_SETTINGS, wide("Open settings").as_ptr());
    if cfg.can_exit {
        AppendMenuW(menu, MF_SEPARATOR, 0, std::ptr::null());
        AppendMenuW(menu, MF_STRING, ID_EXIT, wide("Exit").as_ptr());
    }
    SetMenuDefaultItem(menu, ID_SETTINGS as u32, 0);
    let mut pt = POINT { x: 0, y: 0 };
    GetCursorPos(&mut pt);
    // Needed so the menu closes when the user clicks elsewhere
    SetForegroundWindow(hwnd);
    let cmd = TrackPopupMenu(menu, TPM_RETURNCMD | TPM_RIGHTBUTTON | TPM_NONOTIFY, pt.x, pt.y, 0, hwnd, std::ptr::null());
    PostMessageW(hwnd, WM_NULL, 0, 0);
    DestroyMenu(menu);
    match cmd as usize {
        ID_SETTINGS => (cfg.on_settings)(),
        ID_EXIT => {
            PostMessageW(hwnd, WM_CLOSE, 0, 0);
        }
        _ => {}
    }
}

unsafe extern "system" fn wndproc(hwnd: HWND, msg: u32, wp: WPARAM, lp: LPARAM) -> LRESULT {
    let Some(cfg) = CONFIG.get() else { return DefWindowProcW(hwnd, msg, wp, lp) };
    match msg {
        WM_TRAY => match lp as u32 {
            WM_LBUTTONDBLCLK => (cfg.on_settings)(),
            WM_RBUTTONUP | WM_CONTEXTMENU => show_menu(hwnd, cfg),
            _ => {}
        },
        WM_SET_TIP => set_tip(hwnd),
        WM_POWERBROADCAST if wp == PBT_POWERSETTINGCHANGE => {
            let s = &*(lp as *const POWERBROADCAST_SETTING);
            if same_guid(&s.PowerSetting, &GUID_MONITOR_POWER_ON) {
                // 0 = off, 1 = on (2, dimmed, is ignored as before)
                match s.Data[0] {
                    0 => (cfg.on_monitor)(false),
                    1 => (cfg.on_monitor)(true),
                    _ => {}
                }
            }
            return 1;
        }
        WM_DISPLAYCHANGE => {
            screens::refresh();
        }
        WM_CLOSE => {
            let nid = icon_data(hwnd);
            Shell_NotifyIconW(NIM_DELETE, &nid);
            DestroyWindow(hwnd);
            return 0;
        }
        WM_DESTROY => {
            HWND_MAIN.store(std::ptr::null_mut(), Ordering::Release);
            PostQuitMessage(0);
            return 0;
        }
        m if m != 0 && m == TASKBAR_CREATED.load(Ordering::Relaxed) => {
            // Explorer restarted (or the user just signed in): show the icon again
            add_icon(hwnd);
        }
        _ => {}
    }
    DefWindowProcW(hwnd, msg, wp, lp)
}

/// Shows the icon and runs the message loop until `quit` is called (or Exit is chosen).
pub fn run(cfg: Config) {
    let _ = CONFIG.set(cfg);
    unsafe {
        let hinst = GetModuleHandleW(std::ptr::null());
        let class = wide("MagicRemoteServiceWindow");
        let wc = WNDCLASSW { lpfnWndProc: Some(wndproc), hInstance: hinst, lpszClassName: class.as_ptr(), ..std::mem::zeroed() };
        RegisterClassW(&wc);
        TASKBAR_CREATED.store(RegisterWindowMessageW(wide("TaskbarCreated").as_ptr()), Ordering::Relaxed);
        // A real (never shown) top-level window: message-only windows miss broadcasts
        let hwnd = CreateWindowExW(
            0,
            class.as_ptr(),
            wide("MagicRemoteService").as_ptr(),
            0,
            0,
            0,
            0,
            0,
            std::ptr::null_mut(),
            std::ptr::null_mut(),
            hinst,
            std::ptr::null(),
        );
        if hwnd.is_null() {
            crate::log::error("Cannot create the tray window");
            return;
        }
        HWND_MAIN.store(hwnd, Ordering::Release);
        RegisterPowerSettingNotification(hwnd, &GUID_MONITOR_POWER_ON, 0 /* DEVICE_NOTIFY_WINDOW_HANDLE */);
        add_icon(hwnd);
        if QUIT_REQUESTED.load(Ordering::Acquire) {
            PostMessageW(hwnd, WM_CLOSE, 0, 0);
        }
        let mut msg: MSG = std::mem::zeroed();
        while GetMessageW(&mut msg, std::ptr::null_mut(), 0, 0) > 0 {
            TranslateMessage(&msg);
            DispatchMessageW(&msg);
        }
    }
}

/// Ends `run` from any thread.
pub fn quit() {
    QUIT_REQUESTED.store(true, Ordering::Release);
    let hwnd = HWND_MAIN.load(Ordering::Acquire);
    if !hwnd.is_null() {
        unsafe { PostMessageW(hwnd, WM_CLOSE, 0, 0) };
    }
}

/// Updates the tooltip's TV count, from any thread.
pub fn set_tv_count(n: usize) {
    TVS.store(n as u32, Ordering::Relaxed);
    let hwnd = HWND_MAIN.load(Ordering::Acquire);
    if !hwnd.is_null() {
        unsafe { PostMessageW(hwnd, WM_SET_TIP, 0, 0) };
    }
}
