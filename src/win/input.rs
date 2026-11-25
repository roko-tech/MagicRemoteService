//! Injects mouse and keyboard input with `SendInput`.

use crate::bindings::{InputEvent, MouseButton};
use windows_sys::Win32::System::StationsAndDesktops::{CloseDesktop, OpenInputDesktop, SetThreadDesktop, DESKTOP_CONTROL_FLAGS};
use windows_sys::Win32::UI::Input::KeyboardAndMouse::*;

const GENERIC_ALL: u32 = 0x1000_0000;

fn mouse(flags: u32, dx: i32, dy: i32, data: i32) -> INPUT {
    INPUT { r#type: INPUT_MOUSE, Anonymous: INPUT_0 { mi: MOUSEINPUT { dx, dy, mouseData: data as u32, dwFlags: flags, time: 0, dwExtraInfo: 0 } } }
}

fn keyboard(vk: u16, scan: u16, flags: u32) -> INPUT {
    INPUT { r#type: INPUT_KEYBOARD, Anonymous: INPUT_0 { ki: KEYBDINPUT { wVk: vk, wScan: scan, dwFlags: flags, time: 0, dwExtraInfo: 0 } } }
}

pub fn to_input(e: &InputEvent) -> INPUT {
    match *e {
        InputEvent::Button { button, down } => mouse(
            match (button, down) {
                (MouseButton::Left, true) => MOUSEEVENTF_LEFTDOWN,
                (MouseButton::Left, false) => MOUSEEVENTF_LEFTUP,
                (MouseButton::Right, true) => MOUSEEVENTF_RIGHTDOWN,
                (MouseButton::Right, false) => MOUSEEVENTF_RIGHTUP,
                (MouseButton::Middle, true) => MOUSEEVENTF_MIDDLEDOWN,
                (MouseButton::Middle, false) => MOUSEEVENTF_MIDDLEUP,
            },
            0,
            0,
            0,
        ),
        InputEvent::Key { vk, scan, extended, down } => {
            let mut flags = if down { 0 } else { KEYEVENTF_KEYUP };
            if extended {
                flags |= KEYEVENTF_EXTENDEDKEY;
            }
            keyboard(vk, scan, flags)
        }
        InputEvent::MoveRelative { dx, dy } => mouse(MOUSEEVENTF_MOVE, dx, dy, 0),
        InputEvent::MoveAbsolute { x, y } => mouse(MOUSEEVENTF_MOVE | MOUSEEVENTF_ABSOLUTE | MOUSEEVENTF_VIRTUALDESK, x, y, 0),
        InputEvent::Wheel { delta } => mouse(MOUSEEVENTF_WHEEL, 0, 0, delta),
        InputEvent::Unicode { unit, down } => keyboard(0, unit, KEYEVENTF_UNICODE | if down { 0 } else { KEYEVENTF_KEYUP }),
    }
}

/// Moves this thread to the desktop that has input (the sign-in screen, a UAC prompt).
fn follow_input_desktop() -> bool {
    unsafe {
        let desk = OpenInputDesktop(0 as DESKTOP_CONTROL_FLAGS, 1, GENERIC_ALL);
        if desk.is_null() {
            return false;
        }
        let ok = SetThreadDesktop(desk) != 0;
        CloseDesktop(desk);
        ok
    }
}

/// Sends the events; when the input desktop changed under this thread, follows it and retries.
pub fn send(events: &[InputEvent]) -> u32 {
    if events.is_empty() {
        return 0;
    }
    let inputs: Vec<INPUT> = events.iter().map(to_input).collect();
    let size = std::mem::size_of::<INPUT>() as i32;
    let sent = unsafe { SendInput(inputs.len() as u32, inputs.as_ptr(), size) };
    if sent == 0 && follow_input_desktop() {
        unsafe { SendInput(inputs.len() as u32, inputs.as_ptr(), size) }
    } else {
        sent
    }
}

/// Milliseconds since the last input in this session (injected input counts too).
pub fn idle_ms() -> Option<u32> {
    let mut lii = LASTINPUTINFO { cbSize: std::mem::size_of::<LASTINPUTINFO>() as u32, dwTime: 0 };
    if unsafe { GetLastInputInfo(&mut lii) } == 0 {
        return None;
    }
    let now = unsafe { windows_sys::Win32::System::SystemInformation::GetTickCount() };
    Some(now.wrapping_sub(lii.dwTime))
}
