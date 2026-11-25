//! Swaps the system pointer for the remote's cursor while a TV shows its pointer, and gives the
//! user's cursors, speed and acceleration back when the last TV hides it or disconnects.

use super::wide;
use std::sync::Mutex;
use windows_sys::Win32::UI::WindowsAndMessaging::*;

static CURSOR_FILE: &[u8] = include_bytes!("../../MagicRemoteService/Resources/MagicRemoteService.cur");

const IDS: [SYSTEM_CURSOR_ID; 8] = [OCR_NORMAL, OCR_IBEAM, OCR_WAIT, OCR_CROSS, OCR_UP, OCR_HAND, OCR_NO, OCR_APPSTARTING];
const REMOTE_SPEED: i32 = 10;

struct State {
    /// TVs currently showing their pointer.
    users: u32,
    remote: HCURSOR,
    saved_cursors: Vec<(SYSTEM_CURSOR_ID, HCURSOR)>,
    saved_speed: Option<i32>,
    saved_accel: Option<[i32; 3]>,
}

// Cursor handles are session-wide, not tied to a thread.
unsafe impl Send for State {}

static STATE: Mutex<State> = Mutex::new(State { users: 0, remote: std::ptr::null_mut(), saved_cursors: Vec::new(), saved_speed: None, saved_accel: None });

fn load_remote_cursor() -> HCURSOR {
    let path = std::env::temp_dir().join(format!("MagicRemoteService-{}.cur", std::process::id()));
    if std::fs::write(&path, CURSOR_FILE).is_err() {
        return std::ptr::null_mut();
    }
    let h = unsafe { LoadCursorFromFileW(wide(&path.to_string_lossy()).as_ptr()) };
    let _ = std::fs::remove_file(&path);
    h
}

fn apply(s: &mut State) {
    if s.remote.is_null() {
        s.remote = load_remote_cursor();
        if s.remote.is_null() {
            crate::log::warn(format!("Cannot load the remote's cursor: {}", super::last_error()));
        }
    }
    unsafe {
        if !s.remote.is_null() && s.saved_cursors.is_empty() {
            for id in IDS {
                let current = CopyIcon(LoadCursorW(std::ptr::null_mut(), id as usize as *const u16));
                s.saved_cursors.push((id, current));
                // SetSystemCursor takes ownership of the handle it is given
                SetSystemCursor(CopyIcon(s.remote), id);
            }
        }
        if s.saved_speed.is_none() {
            let mut speed = 0i32;
            if SystemParametersInfoW(SPI_GETMOUSESPEED, 0, &mut speed as *mut i32 as _, 0) != 0 {
                s.saved_speed = Some(speed);
                SystemParametersInfoW(SPI_SETMOUSESPEED, 0, REMOTE_SPEED as usize as _, 0);
            }
        }
        if s.saved_accel.is_none() {
            let mut accel = [0i32; 3];
            if SystemParametersInfoW(SPI_GETMOUSE, 0, accel.as_mut_ptr() as _, 0) != 0 {
                s.saved_accel = Some(accel);
                let mut off = [0i32; 3];
                SystemParametersInfoW(SPI_SETMOUSE, 0, off.as_mut_ptr() as _, 0);
            }
        }
    }
}

fn restore(s: &mut State) {
    unsafe {
        for (id, h) in s.saved_cursors.drain(..) {
            SetSystemCursor(h, id);
        }
        if let Some(speed) = s.saved_speed.take() {
            SystemParametersInfoW(SPI_SETMOUSESPEED, 0, speed as usize as _, 0);
        }
        if let Some(mut accel) = s.saved_accel.take() {
            SystemParametersInfoW(SPI_SETMOUSE, 0, accel.as_mut_ptr() as _, 0);
        }
    }
}

/// One TV started (true) or stopped (false) showing its pointer.
pub fn pointer_shown(shown: bool) {
    let mut s = STATE.lock().unwrap_or_else(|e| e.into_inner());
    if shown {
        s.users += 1;
        if s.users == 1 {
            apply(&mut s);
        }
    } else if s.users > 0 {
        s.users -= 1;
        if s.users == 0 {
            restore(&mut s);
        }
    }
}

/// Puts everything back regardless of who still uses it (the worker is exiting).
pub fn restore_all() {
    let mut s = STATE.lock().unwrap_or_else(|e| e.into_inner());
    s.users = 0;
    restore(&mut s);
}
