//! Win32 calls, kept out of the pure core.

pub mod acl;
pub mod audio;
pub mod cursor;
pub mod effects;
pub mod eventlog;
pub mod input;
pub mod net;
pub mod proc;
pub mod registry;
pub mod screens;
pub mod service;
pub mod tray;

use windows_sys::Win32::Foundation::{CloseHandle, GetLastError, HANDLE, INVALID_HANDLE_VALUE};

/// UTF-16, NUL-terminated.
pub fn wide(s: &str) -> Vec<u16> {
    s.encode_utf16().chain(std::iter::once(0)).collect()
}

/// A NUL-terminated (or full) UTF-16 buffer back to a String.
pub fn from_wide(w: &[u16]) -> String {
    let end = w.iter().position(|&c| c == 0).unwrap_or(w.len());
    String::from_utf16_lossy(&w[..end])
}

pub fn last_error() -> std::io::Error {
    std::io::Error::from_raw_os_error(unsafe { GetLastError() } as i32)
}

/// Closes a kernel handle when dropped.
pub struct Handle(pub HANDLE);

impl Handle {
    pub fn new(h: HANDLE) -> std::io::Result<Handle> {
        if h.is_null() || h == INVALID_HANDLE_VALUE {
            Err(last_error())
        } else {
            Ok(Handle(h))
        }
    }
}

impl Drop for Handle {
    fn drop(&mut self) {
        unsafe { CloseHandle(self.0) };
    }
}

// A kernel handle may be used from any thread.
unsafe impl Send for Handle {}
unsafe impl Sync for Handle {}

/// Directory holding the running exe, where `bindings.json` and `tv-config.json` live.
pub fn exe_dir() -> std::path::PathBuf {
    std::env::current_exe().ok().and_then(|p| p.parent().map(|d| d.to_path_buf())).unwrap_or_else(|| ".".into())
}
