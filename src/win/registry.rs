//! Registry access in the 32-bit view, where the C# build (a 32-bit process) kept its settings,
//! so this build reads the same values and rolling back to the C# exe keeps them.

use super::{from_wide, wide};
use std::io;
use windows_sys::Win32::Foundation::ERROR_SUCCESS;
use windows_sys::Win32::System::Registry::*;

pub struct Key(HKEY);

// Registry handles may be used from any thread.
unsafe impl Send for Key {}

fn check(code: u32) -> io::Result<()> {
    if code == ERROR_SUCCESS {
        Ok(())
    } else {
        Err(io::Error::from_raw_os_error(code as i32))
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Root {
    LocalMachine,
    CurrentUser,
}

impl Root {
    fn hkey(self) -> HKEY {
        match self {
            Root::LocalMachine => HKEY_LOCAL_MACHINE,
            Root::CurrentUser => HKEY_CURRENT_USER,
        }
    }
}

pub const LOCAL_MACHINE: Root = Root::LocalMachine;
pub const CURRENT_USER: Root = Root::CurrentUser;

fn open_raw(parent: HKEY, path: &str, access: u32) -> io::Result<Key> {
    let mut h = std::ptr::null_mut();
    check(unsafe { RegOpenKeyExW(parent, wide(path).as_ptr(), 0, access, &mut h) })?;
    Ok(Key(h))
}

fn create_raw(parent: HKEY, path: &str, view: u32) -> io::Result<Key> {
    let mut h = std::ptr::null_mut();
    check(unsafe {
        RegCreateKeyExW(parent, wide(path).as_ptr(), 0, std::ptr::null(), 0, KEY_READ | KEY_WRITE | view, std::ptr::null(), &mut h, std::ptr::null_mut())
    })?;
    Ok(Key(h))
}

fn access(write: bool, view: u32) -> u32 {
    view | if write { KEY_READ | KEY_WRITE } else { KEY_READ }
}

impl Key {
    pub fn open(root: Root, path: &str, write: bool) -> io::Result<Key> {
        open_raw(root.hkey(), path, access(write, KEY_WOW64_32KEY))
    }

    pub fn create(root: Root, path: &str) -> io::Result<Key> {
        create_raw(root.hkey(), path, KEY_WOW64_32KEY)
    }

    fn raw(&self, name: &str) -> Option<(u32, Vec<u8>)> {
        let n = wide(name);
        let (mut kind, mut len) = (0u32, 0u32);
        unsafe {
            if RegQueryValueExW(self.0, n.as_ptr(), std::ptr::null(), &mut kind, std::ptr::null_mut(), &mut len) != ERROR_SUCCESS {
                return None;
            }
            let mut buf = vec![0u8; len as usize];
            if RegQueryValueExW(self.0, n.as_ptr(), std::ptr::null(), &mut kind, buf.as_mut_ptr(), &mut len) != ERROR_SUCCESS {
                return None;
            }
            buf.truncate(len as usize);
            Some((kind, buf))
        }
    }

    pub fn get_u32(&self, name: &str) -> Option<u32> {
        match self.raw(name)? {
            (REG_DWORD, b) if b.len() >= 4 => Some(u32::from_le_bytes([b[0], b[1], b[2], b[3]])),
            _ => None,
        }
    }

    pub fn get_string(&self, name: &str) -> Option<String> {
        match self.raw(name)? {
            (REG_SZ | REG_EXPAND_SZ, b) => {
                let w: Vec<u16> = b.as_chunks::<2>().0.iter().map(|c| u16::from_le_bytes(*c)).collect();
                Some(from_wide(&w))
            }
            _ => None,
        }
    }

    pub fn set_u32(&self, name: &str, value: u32) -> io::Result<()> {
        check(unsafe { RegSetValueExW(self.0, wide(name).as_ptr(), 0, REG_DWORD, value.to_le_bytes().as_ptr(), 4) })
    }

    pub fn set_string(&self, name: &str, value: &str) -> io::Result<()> {
        let w = wide(value);
        check(unsafe { RegSetValueExW(self.0, wide(name).as_ptr(), 0, REG_SZ, w.as_ptr() as *const u8, (w.len() * 2) as u32) })
    }

    pub fn delete_value(&self, name: &str) -> io::Result<()> {
        check(unsafe { RegDeleteValueW(self.0, wide(name).as_ptr()) })
    }

    pub fn subkeys(&self) -> Vec<String> {
        let mut out = Vec::new();
        let mut buf = [0u16; 256];
        for i in 0.. {
            let mut len = buf.len() as u32;
            let r = unsafe {
                RegEnumKeyExW(self.0, i, buf.as_mut_ptr(), &mut len, std::ptr::null(), std::ptr::null_mut(), std::ptr::null_mut(), std::ptr::null_mut())
            };
            if r != ERROR_SUCCESS {
                break;
            }
            out.push(String::from_utf16_lossy(&buf[..len as usize]));
        }
        out
    }

    pub fn open_sub(&self, path: &str, write: bool) -> io::Result<Key> {
        open_raw(self.0, path, access(write, KEY_WOW64_32KEY))
    }
}

impl Drop for Key {
    fn drop(&mut self) {
        unsafe { RegCloseKey(self.0) };
    }
}

/// A key in the normal (64-bit) view, for Windows' own settings such as the Run key.
pub fn open_native(root: Root, path: &str, write: bool) -> io::Result<Key> {
    open_raw(root.hkey(), path, access(write, KEY_WOW64_64KEY))
}

/// Creates a key in the normal (64-bit) view.
pub fn create_native(root: Root, path: &str) -> io::Result<Key> {
    create_raw(root.hkey(), path, KEY_WOW64_64KEY)
}
