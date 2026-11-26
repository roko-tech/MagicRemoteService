//! Tokens and processes: who we run as, and starting programs in the right session and account.

use super::{from_wide, last_error, wide, Handle};
use std::io;
use std::os::windows::process::CommandExt;
use std::path::{Path, PathBuf};
use std::time::Duration;
use windows_sys::Win32::Foundation::*;
use windows_sys::Win32::Security::*;
use windows_sys::Win32::System::Environment::{CreateEnvironmentBlock, DestroyEnvironmentBlock};
use windows_sys::Win32::System::RemoteDesktop::{WTSGetActiveConsoleSessionId, WTSQueryUserToken};
use windows_sys::Win32::System::SystemServices::MAXIMUM_ALLOWED;
use windows_sys::Win32::System::Threading::*;
use windows_sys::Win32::UI::Shell::{GetUserProfileDirectoryW, ShellExecuteW};
use windows_sys::Win32::UI::WindowsAndMessaging::{GetForegroundWindow, GetWindowThreadProcessId, SW_HIDE, SW_SHOWNORMAL};

const CREATE_NO_WINDOW_FLAG: u32 = 0x0800_0000;

fn own_token(access: u32) -> io::Result<Handle> {
    let mut h = std::ptr::null_mut();
    if unsafe { OpenProcessToken(GetCurrentProcess(), access, &mut h) } == 0 {
        return Err(last_error());
    }
    Handle::new(h)
}

/// Running as LocalSystem (the service, or the worker it starts).
pub fn is_system() -> bool {
    let Ok(token) = own_token(TOKEN_QUERY) else { return false };
    // u64s, so the TOKEN_USER read from it is aligned
    let mut buf = [0u64; 64];
    let mut len = 0u32;
    unsafe {
        if GetTokenInformation(token.0, TokenUser, buf.as_mut_ptr() as _, std::mem::size_of_val(&buf) as u32, &mut len) == 0 {
            return false;
        }
        let user = &*(buf.as_ptr() as *const TOKEN_USER);
        IsWellKnownSid(user.User.Sid, WinLocalSystemSid) != 0
    }
}

/// Running with an elevated token (an admin's elevated prompt, or SYSTEM).
pub fn is_elevated() -> bool {
    let Ok(token) = own_token(TOKEN_QUERY) else { return false };
    let mut e = TOKEN_ELEVATION { TokenIsElevated: 0 };
    let mut len = 0u32;
    unsafe {
        GetTokenInformation(token.0, TokenElevation, &mut e as *mut _ as _, std::mem::size_of::<TOKEN_ELEVATION>() as u32, &mut len) != 0
            && e.TokenIsElevated != 0
    }
}

/// The session attached to the physical console, if any.
pub fn console_session() -> Option<u32> {
    let id = unsafe { WTSGetActiveConsoleSessionId() };
    (id != u32::MAX).then_some(id)
}

/// The token of the user signed in at the console. Needs SYSTEM.
pub fn console_user_token() -> io::Result<Handle> {
    let session = console_session().ok_or_else(|| io::Error::other("no console session"))?;
    let mut h = std::ptr::null_mut();
    if unsafe { WTSQueryUserToken(session, &mut h) } == 0 {
        return Err(io::Error::other("nobody is signed in at the PC"));
    }
    Handle::new(h)
}

pub fn profile_dir(token: &Handle) -> Option<PathBuf> {
    let mut buf = [0u16; 512];
    let mut len = buf.len() as u32;
    (unsafe { GetUserProfileDirectoryW(token.0, buf.as_mut_ptr(), &mut len) } != 0).then(|| PathBuf::from(from_wide(&buf)))
}

/// The environment a fresh login of `token`'s user gets.
pub fn environment_of(token: &Handle) -> io::Result<Vec<(String, String)>> {
    let mut block: *mut std::ffi::c_void = std::ptr::null_mut();
    if unsafe { CreateEnvironmentBlock(&mut block, token.0, 0) } == 0 {
        return Err(last_error());
    }
    let mut vars = Vec::new();
    unsafe {
        let mut p = block as *const u16;
        loop {
            let len = (0..).take_while(|&i| *p.add(i) != 0).count();
            if len == 0 {
                break;
            }
            let entry = String::from_utf16_lossy(std::slice::from_raw_parts(p, len));
            // Skip the hidden "=C:=C:\" per-drive entries
            if let Some((k, v)) = entry.split_once('=').filter(|(k, _)| !k.is_empty()) {
                vars.push((k.to_string(), v.to_string()));
            }
            p = p.add(len + 1);
        }
        DestroyEnvironmentBlock(block);
    }
    Ok(vars)
}

fn env_block(vars: &[(String, String)]) -> Vec<u16> {
    let mut b: Vec<u16> = vars.iter().flat_map(|(k, v)| format!("{k}={v}").encode_utf16().chain([0]).collect::<Vec<_>>()).collect();
    b.push(0);
    b
}

/// Quotes a path or argument for a Windows command line.
pub fn quote(s: &str) -> String {
    if !s.is_empty() && !s.contains([' ', '\t', '"']) {
        s.to_string()
    } else {
        format!("\"{}\"", s.replace('"', "\\\""))
    }
}

pub struct RunSpec<'a> {
    pub program: &'a Path,
    /// Command line after the program, already quoted.
    pub args: &'a str,
    pub cwd: Option<&'a Path>,
    /// The complete environment for the child.
    pub env: &'a [(String, String)],
}

/// Waits for a process; kills it after `timeout`. Returns its exit code.
pub fn wait(process: &Handle, timeout: Duration) -> io::Result<u32> {
    let ms = timeout.as_millis().min(u32::MAX as u128 - 1) as u32;
    if unsafe { WaitForSingleObject(process.0, ms) } == WAIT_TIMEOUT {
        unsafe { TerminateProcess(process.0, 1) };
        return Err(io::Error::new(io::ErrorKind::TimedOut, format!("gave up after {} s", timeout.as_secs())));
    }
    let mut code = 0u32;
    unsafe { GetExitCodeProcess(process.0, &mut code) };
    Ok(code)
}

/// CreateProcessAsUser; the child inherits no handles.
fn create_as_user(token: &Handle, spec: &RunSpec, hidden: bool) -> io::Result<(Handle, u32)> {
    let mut cmdline = wide(&format!("{} {}", quote(&spec.program.to_string_lossy()), spec.args));
    let mut desktop = wide("winsta0\\default");
    let mut si: STARTUPINFOW = unsafe { std::mem::zeroed() };
    si.cb = std::mem::size_of::<STARTUPINFOW>() as u32;
    si.lpDesktop = desktop.as_mut_ptr();
    if hidden {
        si.dwFlags |= STARTF_USESHOWWINDOW;
        si.wShowWindow = SW_HIDE as u16;
    }
    let flags = CREATE_UNICODE_ENVIRONMENT | if hidden { CREATE_NO_WINDOW_FLAG } else { 0 };
    let env = env_block(spec.env);
    let cwd = spec.cwd.map(|c| wide(&c.to_string_lossy()));
    let mut pi: PROCESS_INFORMATION = unsafe { std::mem::zeroed() };
    let ok = unsafe {
        CreateProcessAsUserW(
            token.0,
            std::ptr::null(),
            cmdline.as_mut_ptr(),
            std::ptr::null(),
            std::ptr::null(),
            0,
            flags,
            if spec.env.is_empty() { std::ptr::null() } else { env.as_ptr() as _ },
            cwd.as_ref().map_or(std::ptr::null(), |c| c.as_ptr()),
            &si,
            &mut pi,
        )
    };
    if ok == 0 {
        return Err(last_error());
    }
    unsafe { CloseHandle(pi.hThread) };
    Ok((Handle(pi.hProcess), pi.dwProcessId))
}

/// Starts a program without waiting: as the console user when we are SYSTEM (so a command
/// binding or a browser never runs with SYSTEM rights), otherwise as ourselves.
pub fn spawn_as_user(program: &Path, args: &str, visible: bool) -> io::Result<()> {
    if is_system() {
        let token = console_user_token()?;
        let env = environment_of(&token)?;
        let cwd = profile_dir(&token);
        let spec = RunSpec { program, args, cwd: cwd.as_deref(), env: &env };
        create_as_user(&token, &spec, !visible).map(|_| ())
    } else {
        let mut cmd = std::process::Command::new(program);
        cmd.raw_arg(args);
        if !visible {
            cmd.creation_flags(CREATE_NO_WINDOW_FLAG);
        }
        cmd.spawn().map(|_| ())
    }
}

/// Starts a hidden helper (shutdown.exe, net.exe) as ourselves.
pub fn spawn_hidden(program: &str, args: &str) -> io::Result<()> {
    std::process::Command::new(program).raw_arg(args).creation_flags(CREATE_NO_WINDOW_FLAG).spawn().map(|_| ())
}

/// Opens a URL in the console user's default browser.
pub fn open_url(url: &str) -> io::Result<()> {
    if is_system() {
        let rundll = system_dir().join("rundll32.exe");
        spawn_as_user(&rundll, &format!("url.dll,FileProtocolHandler {url}"), true)
    } else {
        let r = unsafe { ShellExecuteW(std::ptr::null_mut(), wide("open").as_ptr(), wide(url).as_ptr(), std::ptr::null(), std::ptr::null(), SW_SHOWNORMAL) };
        if r as usize > 32 {
            Ok(())
        } else {
            Err(io::Error::other("could not open the browser"))
        }
    }
}

pub fn system_dir() -> PathBuf {
    PathBuf::from(std::env::var("SystemRoot").unwrap_or_else(|_| "C:\\Windows".into())).join("System32")
}

/// Starts `program args` in `session` with our own (SYSTEM) token, on the interactive desktop.
pub fn spawn_in_session(session: u32, program: &Path, args: &str) -> io::Result<(Handle, u32)> {
    let own = own_token(TOKEN_DUPLICATE | TOKEN_QUERY)?;
    let mut dup = std::ptr::null_mut();
    if unsafe { DuplicateTokenEx(own.0, MAXIMUM_ALLOWED, std::ptr::null(), SecurityImpersonation, TokenPrimary, &mut dup) } == 0 {
        return Err(last_error());
    }
    let token = Handle::new(dup)?;
    if unsafe { SetTokenInformation(token.0, TokenSessionId, &session as *const u32 as _, 4) } == 0 {
        return Err(last_error());
    }
    let spec = RunSpec { program, args, cwd: program.parent(), env: &[] };
    create_as_user(&token, &spec, false)
}

/// File name without extension of the process owning the foreground window, such as `PotPlayerMini64`.
pub fn foreground_process_name() -> Option<String> {
    unsafe {
        let hwnd = GetForegroundWindow();
        if hwnd.is_null() {
            return None;
        }
        let mut pid = 0u32;
        GetWindowThreadProcessId(hwnd, &mut pid);
        let h = Handle::new(OpenProcess(PROCESS_QUERY_LIMITED_INFORMATION, 0, pid)).ok()?;
        let mut buf = [0u16; 1024];
        let mut len = buf.len() as u32;
        if QueryFullProcessImageNameW(h.0, PROCESS_NAME_WIN32, buf.as_mut_ptr(), &mut len) == 0 {
            return None;
        }
        Path::new(&String::from_utf16_lossy(&buf[..len as usize])).file_stem().map(|s| s.to_string_lossy().into_owned())
    }
}

/// Opens a process to wait on it.
pub fn open_for_wait(pid: u32) -> Option<Handle> {
    Handle::new(unsafe { OpenProcess(PROCESS_SYNCHRONIZE | PROCESS_QUERY_LIMITED_INFORMATION, 0, pid) }).ok()
}

pub fn has_exited(process: &Handle) -> bool {
    let r = unsafe { WaitForSingleObject(process.0, 0) };
    r != WAIT_TIMEOUT
}

pub fn terminate(process: &Handle) {
    unsafe { TerminateProcess(process.0, 1) };
}

/// Waits up to `timeout` for the process to end.
pub fn wait_exit(process: &Handle, timeout: Duration) -> bool {
    unsafe { WaitForSingleObject(process.0, timeout.as_millis().min(u32::MAX as u128 - 1) as u32) == WAIT_OBJECT_0 }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn quotes_only_when_needed() {
        assert_eq!(quote("C:\\node\\node.exe"), "C:\\node\\node.exe");
        assert_eq!(quote("C:\\Program Files\\nodejs\\node.exe"), "\"C:\\Program Files\\nodejs\\node.exe\"");
        assert_eq!(quote(""), "\"\"");
    }

    #[test]
    fn environment_blocks_round_trip() {
        let vars = vec![("PATH".to_string(), "C:\\a;C:\\b".to_string()), ("X".to_string(), "=1".to_string())];
        let b = env_block(&vars);
        assert_eq!(String::from_utf16_lossy(&b), "PATH=C:\\a;C:\\b\0X==1\0\0");
    }
}
