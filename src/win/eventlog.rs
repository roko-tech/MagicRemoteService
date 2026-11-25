//! Writes to the Application event log under the source the C# build registered.

use super::wide;
use crate::log::Level;
use windows_sys::Win32::System::EventLog::*;

pub const SOURCE: &str = "MagicRemoteService";
const SOURCE_KEY: &str = "SYSTEM\\CurrentControlSet\\Services\\EventLog\\Application\\MagicRemoteService";

/// Sends log entries at info level and above to the event log.
pub fn install_sink() {
    let h = unsafe { RegisterEventSourceW(std::ptr::null(), wide(SOURCE).as_ptr()) };
    if h.is_null() {
        return;
    }
    let h = h as usize;
    crate::log::set_sink(Box::new(move |level, msg| {
        let kind = match level {
            Level::Debug | Level::Info => EVENTLOG_INFORMATION_TYPE,
            Level::Warn => EVENTLOG_WARNING_TYPE,
            Level::Error => EVENTLOG_ERROR_TYPE,
        };
        let text = wide(msg);
        let strings = [text.as_ptr()];
        unsafe { ReportEventW(h as _, kind, 0, 0, std::ptr::null_mut(), 1, 0, strings.as_ptr(), std::ptr::null()) };
    }));
}

/// Registers the source with .NET's message file, whose event 0 is the plain text `%1`, so
/// Event Viewer shows the messages as written. Needs admin.
pub fn register_source() -> std::io::Result<()> {
    let dll = std::path::PathBuf::from(std::env::var("SystemRoot").unwrap_or_else(|_| "C:\\Windows".into()))
        .join("Microsoft.NET\\Framework64\\v4.0.30319\\EventLogMessages.dll");
    let key = crate::win::registry::create_native(crate::win::registry::LOCAL_MACHINE, SOURCE_KEY)?;
    if dll.is_file() {
        key.set_string("EventMessageFile", &dll.to_string_lossy())?;
    }
    key.set_u32("TypesSupported", 7)
}
