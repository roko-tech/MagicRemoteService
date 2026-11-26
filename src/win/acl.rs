//! File permissions and random secrets.

use super::{last_error, wide};
use std::io;
use std::path::Path;
use windows_sys::Win32::Foundation::{LocalFree, HLOCAL};
use windows_sys::Win32::Security::Authorization::{
    ConvertStringSecurityDescriptorToSecurityDescriptorW, SetNamedSecurityInfoW, SDDL_REVISION_1, SE_FILE_OBJECT,
};
use windows_sys::Win32::Security::Cryptography::{BCryptGenRandom, BCRYPT_USE_SYSTEM_PREFERRED_RNG};
use windows_sys::Win32::Security::{GetSecurityDescriptorDacl, ACL, DACL_SECURITY_INFORMATION, PROTECTED_DACL_SECURITY_INFORMATION};

/// Lets only SYSTEM and administrators open the file (it holds a secret), replacing the
/// permissions it inherited from its folder.
pub fn restrict_to_admins(path: &Path) -> io::Result<()> {
    let sddl = wide("D:P(A;;FA;;;SY)(A;;FA;;;BA)");
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
}
