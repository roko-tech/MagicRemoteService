//! File permissions and random secrets.

use super::{last_error, wide, Handle};
use std::io;
use std::path::Path;
use windows_sys::Win32::Foundation::{LocalFree, HLOCAL};
use windows_sys::Win32::Security::Authorization::{
    ConvertSidToStringSidW, ConvertStringSecurityDescriptorToSecurityDescriptorW, SetNamedSecurityInfoW, SDDL_REVISION_1, SE_FILE_OBJECT,
};
use windows_sys::Win32::Security::Cryptography::{BCryptGenRandom, BCRYPT_USE_SYSTEM_PREFERRED_RNG};
use windows_sys::Win32::Security::{
    GetSecurityDescriptorDacl, GetTokenInformation, TokenUser, ACL, DACL_SECURITY_INFORMATION, PROTECTED_DACL_SECURITY_INFORMATION, TOKEN_QUERY, TOKEN_USER,
};
use windows_sys::Win32::System::Threading::{GetCurrentProcess, OpenProcessToken};

/// Lets only SYSTEM and administrators open the file (it holds a secret), replacing the
/// permissions it inherited from its folder.
pub fn restrict_to_admins(path: &Path) -> io::Result<()> {
    restrict(path, "D:P(A;;FA;;;SY)(A;;FA;;;BA)")
}

/// Like [`restrict_to_admins`], and also the account this process runs as. Windows' SSH client
/// refuses a key file anyone else can open.
pub fn restrict_to_admins_and_self(path: &Path) -> io::Result<()> {
    restrict(path, &format!("D:P(A;;FA;;;SY)(A;;FA;;;BA)(A;;FA;;;{})", own_sid()?))
}

/// The SID of the account this process runs as, such as `S-1-5-18`.
fn own_sid() -> io::Result<String> {
    unsafe {
        let mut h = std::ptr::null_mut();
        if OpenProcessToken(GetCurrentProcess(), TOKEN_QUERY, &mut h) == 0 {
            return Err(last_error());
        }
        let token = Handle::new(h)?;
        // u64s, so the TOKEN_USER read from it is aligned
        let mut buf = [0u64; 64];
        let mut len = 0u32;
        if GetTokenInformation(token.0, TokenUser, buf.as_mut_ptr() as _, std::mem::size_of_val(&buf) as u32, &mut len) == 0 {
            return Err(last_error());
        }
        let user = &*(buf.as_ptr() as *const TOKEN_USER);
        let mut text = std::ptr::null_mut();
        if ConvertSidToStringSidW(user.User.Sid, &mut text) == 0 {
            return Err(last_error());
        }
        let len = (0..).take_while(|&i| *text.add(i) != 0).count();
        let sid = String::from_utf16_lossy(std::slice::from_raw_parts(text, len));
        LocalFree(text as HLOCAL);
        Ok(sid)
    }
}

fn restrict(path: &Path, sddl: &str) -> io::Result<()> {
    let sddl = wide(sddl);
    let mut sd = std::ptr::null_mut();
    unsafe {
        if ConvertStringSecurityDescriptorToSecurityDescriptorW(sddl.as_ptr(), SDDL_REVISION_1, &mut sd, std::ptr::null_mut()) == 0 {
            return Err(last_error());
        }
        let (mut present, mut defaulted) = (0, 0);
        let mut dacl: *mut ACL = std::ptr::null_mut();
        let result = if GetSecurityDescriptorDacl(sd, &mut present, &mut dacl, &mut defaulted) == 0 {
            Err(last_error())
        } else {
            let name = wide(&path.to_string_lossy());
            let code = SetNamedSecurityInfoW(
                name.as_ptr(),
                SE_FILE_OBJECT,
                DACL_SECURITY_INFORMATION | PROTECTED_DACL_SECURITY_INFORMATION,
                std::ptr::null_mut(),
                std::ptr::null_mut(),
                dacl,
                std::ptr::null(),
            );
            if code == 0 {
                Ok(())
            } else {
                Err(io::Error::from_raw_os_error(code as i32))
            }
        };
        LocalFree(sd as HLOCAL);
        result
    }
}

/// 128 bits from the system's cryptographic generator, as 32 hex digits.
pub fn random_token() -> io::Result<String> {
    let mut bytes = [0u8; 16];
    let status = unsafe { BCryptGenRandom(std::ptr::null_mut(), bytes.as_mut_ptr(), bytes.len() as u32, BCRYPT_USE_SYSTEM_PREFERRED_RNG) };
    if status != 0 {
        return Err(io::Error::other(format!("BCryptGenRandom failed ({status:#x})")));
    }
    Ok(bytes.iter().map(|b| format!("{b:02x}")).collect())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn tokens_are_random_hex() {
        let (a, b) = (random_token().unwrap(), random_token().unwrap());
        assert_eq!(a.len(), 32);
        assert!(a.bytes().all(|c| c.is_ascii_hexdigit()));
        assert_ne!(a, b);
    }

    #[test]
    fn restricted_files_are_unreadable_to_ordinary_users() {
        // Run as an ordinary user: after restricting, even the file's creator can't read it
        let path = std::env::temp_dir().join(format!("mrs-acl-test-{}.json", std::process::id()));
        std::fs::write(&path, b"{\"token\":\"x\"}").unwrap();
        restrict_to_admins(&path).unwrap();
        let read = std::fs::read(&path);
        if !super::super::proc::is_elevated() {
            assert_eq!(read.map_err(|e| e.kind()), Err(io::ErrorKind::PermissionDenied));
        }
        // The folder (the user's TEMP) still lets its owner delete it
        std::fs::remove_file(&path).unwrap();
    }

    #[test]
    fn files_restricted_to_self_stay_readable_to_their_owner() {
        assert!(own_sid().unwrap().starts_with("S-1-5-"));
        let path = std::env::temp_dir().join(format!("mrs-acl-self-test-{}.key", std::process::id()));
        std::fs::write(&path, b"key").unwrap();
        restrict_to_admins_and_self(&path).unwrap();
        assert_eq!(std::fs::read(&path).unwrap(), b"key");
        std::fs::remove_file(&path).unwrap();
    }
}
