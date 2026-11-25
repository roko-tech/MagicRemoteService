//! The Windows service: a supervisor in session 0 that keeps one worker running in the console
//! session (input can only be injected from there), plus install/uninstall/status.

use super::{proc, wide, Handle};
use crate::log;
use std::ffi::OsString;
use std::path::Path;
use std::sync::mpsc;
use std::time::{Duration, Instant};
use windows_service::service::*;
use windows_service::service_control_handler::{self, ServiceControlHandlerResult};
use windows_service::service_manager::{ServiceManager, ServiceManagerAccess};
use windows_service::{define_windows_service, service_dispatcher};
use windows_sys::Win32::System::Threading::{CreateEventW, ResetEvent, SetEvent};

pub const NAME: &str = "MagicRemoteService";
/// Signalled by the supervisor to ask its worker to stop cleanly.
pub const STOP_EVENT: &str = "Global\\MagicRemoteService.WorkerStop";
const WORKER_GRACE: Duration = Duration::from_secs(8);
const STABLE_RUN: Duration = Duration::from_secs(60);
const MAX_BACKOFF: Duration = Duration::from_secs(30);

define_windows_service!(ffi_service_main, service_main);

/// Hands the process to the service control manager. Fails when not started as a service.
pub fn run() -> windows_service::Result<()> {
    service_dispatcher::start(NAME, ffi_service_main)
}

enum Event {
    Stop,
    ConsoleConnect,
}

fn service_main(_args: Vec<OsString>) {
    super::eventlog::install_sink();
    if let Err(e) = supervise() {
        log::error(format!("Service failed: {e}"));
    }
}

fn status(state: ServiceState, accept: ServiceControlAccept, wait: Duration) -> ServiceStatus {
    ServiceStatus {
        service_type: ServiceType::OWN_PROCESS,
        current_state: state,
        controls_accepted: accept,
        exit_code: ServiceExitCode::Win32(0),
        checkpoint: 0,
        wait_hint: wait,
        process_id: None,
    }
}

struct Worker {
    process: Handle,
    pid: u32,
    session: u32,
    started: Instant,
}

fn supervise() -> windows_service::Result<()> {
    let (tx, rx) = mpsc::channel();
    let handle = service_control_handler::register(NAME, move |control| match control {
        ServiceControl::Stop | ServiceControl::Shutdown => {
            let _ = tx.send(Event::Stop);
            ServiceControlHandlerResult::NoError
        }
        ServiceControl::SessionChange(p) => {
            if p.reason == SessionChangeReason::ConsoleConnect {
                let _ = tx.send(Event::ConsoleConnect);
            }
            ServiceControlHandlerResult::NoError
        }
        ServiceControl::Interrogate => ServiceControlHandlerResult::NoError,
        _ => ServiceControlHandlerResult::NotImplemented,
    })?;
    handle.set_service_status(status(ServiceState::StartPending, ServiceControlAccept::empty(), Duration::from_secs(10)))?;
    let stop_event = Handle::new(unsafe { CreateEventW(std::ptr::null(), 1, 0, wide(STOP_EVENT).as_ptr()) }).map_err(windows_service::Error::Winapi)?;
    handle.set_service_status(status(
        ServiceState::Running,
        ServiceControlAccept::STOP | ServiceControlAccept::SHUTDOWN | ServiceControlAccept::SESSION_CHANGE,
        Duration::ZERO,
    ))?;
    log::info(format!("Service {} started", env!("CARGO_PKG_VERSION")));

    let exe = std::env::current_exe().unwrap_or_else(|_| NAME.into());
    let mut worker: Option<Worker> = None;
    let mut backoff = Duration::from_secs(1);
    let mut next_start = Instant::now();
    let mut last_failure = String::new();
    loop {
        match rx.recv_timeout(Duration::from_millis(500)) {
            Ok(Event::Stop) => break,
            Ok(Event::ConsoleConnect) => {
                // Another session took the console: move the worker there
                if let Some(w) = worker.take() {
                    log::info(format!("Console moved, restarting the worker (was in session {})", w.session));
                    stop_worker(w, &stop_event);
                }
                next_start = Instant::now();
            }
            Err(mpsc::RecvTimeoutError::Timeout) => {}
            Err(mpsc::RecvTimeoutError::Disconnected) => break,
        }
        if let Some(w) = &worker {
            if proc::has_exited(&w.process) {
                let code = proc::wait(&w.process, Duration::ZERO).unwrap_or(u32::MAX);
                let ran = w.started.elapsed();
                backoff = if ran >= STABLE_RUN { Duration::from_secs(1) } else { (backoff * 2).min(MAX_BACKOFF) };
                log::warn(format!("Worker {} exited with code {code} after {} s; restarting in {} s", w.pid, ran.as_secs(), backoff.as_secs()));
                worker = None;
                next_start = Instant::now() + backoff;
            }
        }
        if worker.is_none() && Instant::now() >= next_start {
            match start_worker(&exe, &stop_event) {
                Ok(w) => {
                    worker = Some(w);
                    last_failure.clear();
                }
                Err(e) => {
                    // Once per distinct reason, not every retry
                    let why = e.to_string();
                    if why != last_failure {
                        log::warn(format!("Worker not started, retrying every 2 s: {why}"));
                        last_failure = why;
                    }
                    next_start = Instant::now() + Duration::from_secs(2);
                }
            }
        }
    }
    handle.set_service_status(status(ServiceState::StopPending, ServiceControlAccept::empty(), WORKER_GRACE + Duration::from_secs(2)))?;
    if let Some(w) = worker.take() {
        stop_worker(w, &stop_event);
    }
    log::info("Service stopped");
    handle.set_service_status(status(ServiceState::Stopped, ServiceControlAccept::empty(), Duration::ZERO))?;
    Ok(())
}

fn start_worker(exe: &Path, stop_event: &Handle) -> std::io::Result<Worker> {
    let session = proc::console_session().ok_or_else(|| std::io::Error::other("no console session"))?;
    unsafe { ResetEvent(stop_event.0) };
    let (process, pid) = proc::spawn_in_session(session, exe, &format!("-c --parent {}", std::process::id()))?;
    log::info(format!("Worker {pid} started in session {session}"));
    Ok(Worker { process, pid, session, started: Instant::now() })
}

/// Asks the worker to finish (it restores the cursor and releases keys), then makes sure.
fn stop_worker(w: Worker, stop_event: &Handle) {
    unsafe { SetEvent(stop_event.0) };
    if !proc::wait_exit(&w.process, WORKER_GRACE) {
        log::warn(format!("Worker {} did not stop in time; ending it", w.pid));
        proc::terminate(&w.process);
    }
}

fn manager(access: ServiceManagerAccess) -> Result<ServiceManager, String> {
    ServiceManager::local_computer(None::<&str>, access).map_err(|e| format!("Cannot open the service manager ({e}). Run from an elevated prompt."))
}

/// `Some(true)` running, `Some(false)` installed but not running, `None` not installed.
pub fn state() -> Option<bool> {
    let m = manager(ServiceManagerAccess::CONNECT).ok()?;
    let s = m.open_service(NAME, ServiceAccess::QUERY_STATUS).ok()?;
    Some(s.query_status().ok()?.current_state == ServiceState::Running)
}

fn netsh(line: &str) -> bool {
    use std::os::windows::process::CommandExt;
    std::process::Command::new(proc::system_dir().join("netsh.exe"))
        .raw_arg(line)
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .status()
        .is_ok_and(|s| s.success())
}

/// Inbound rules for the TV's WebSocket port and the SSDP search. The web UI listens on
/// loopback only, so the old "Web UI" rule is removed rather than recreated.
fn firewall(exe: &Path, port: u16, add: bool) -> bool {
    for name in [NAME, "MagicRemoteService Web UI", "MagicRemoteService SSDP"] {
        netsh(&format!("advfirewall firewall delete rule name=\"{name}\""));
    }
    if !add {
        return true;
    }
    let program = proc::quote(&exe.to_string_lossy());
    let tcp = netsh(&format!("advfirewall firewall add rule name=\"{NAME}\" dir=in action=allow program={program} protocol=TCP localport={port}"));
    let udp = netsh(&format!(
        "advfirewall firewall add rule name=\"MagicRemoteService SSDP\" dir=in action=allow program={program} protocol=UDP localport={}",
        crate::ssdp::PORT
    ));
    tcp && udp
}

/// Registers (or re-points) the service at this exe, sets restart-on-failure, the event log
/// source and firewall rules, then starts it.
pub fn install() -> Result<String, String> {
    if !proc::is_elevated() {
        return Err("Run this from an elevated (administrator) prompt.".into());
    }
    let exe = std::env::current_exe().map_err(|e| e.to_string())?;
    let m = manager(ServiceManagerAccess::CONNECT | ServiceManagerAccess::CREATE_SERVICE)?;
    let info = ServiceInfo {
        name: NAME.into(),
        display_name: NAME.into(),
        service_type: ServiceType::OWN_PROCESS,
        start_type: ServiceStartType::AutoStart,
        error_control: ServiceErrorControl::Normal,
        executable_path: exe.clone(),
        launch_arguments: vec![],
        dependencies: vec![],
        account_name: None,
        account_password: None,
    };
    let svc = match m.open_service(NAME, ServiceAccess::all()) {
        Ok(svc) => {
            if svc.query_status().is_ok_and(|s| s.current_state != ServiceState::Stopped) {
                let _ = svc.stop();
                let deadline = Instant::now() + Duration::from_secs(20);
                while svc.query_status().is_ok_and(|s| s.current_state != ServiceState::Stopped) && Instant::now() < deadline {
                    std::thread::sleep(Duration::from_millis(300));
                }
            }
            svc.change_config(&info).map_err(|e| format!("Cannot update the service: {e}"))?;
            svc
        }
        Err(_) => m.create_service(&info, ServiceAccess::all()).map_err(|e| format!("Cannot create the service: {e}"))?,
    };
    let _ = svc.set_description("Lets the LG Magic Remote control this PC through the MagicRemoteService TV app.");
    let restart = ServiceAction { action_type: ServiceActionType::Restart, delay: Duration::from_secs(5) };
    let _ = svc.update_failure_actions(ServiceFailureActions {
        reset_period: ServiceFailureResetPeriod::After(Duration::from_secs(86_400)),
        reboot_msg: None,
        command: None,
        actions: Some(vec![restart.clone(), restart.clone(), restart]),
    });
    let _ = super::eventlog::register_source();
    let port = crate::settings::Scope::Machine.load().port;
    let fw = firewall(&exe, port, true);
    svc.start(&[] as &[&str]).map_err(|e| format!("Installed, but the service did not start: {e}"))?;
    Ok(format!(
        "Service installed from {} and started.{}\nSettings: http://localhost:{}/",
        exe.display(),
        if fw { "" } else { "\nWarning: the firewall rules could not be added." },
        port + 1
    ))
}

pub fn uninstall() -> Result<String, String> {
    if !proc::is_elevated() {
        return Err("Run this from an elevated (administrator) prompt.".into());
    }
    let m = manager(ServiceManagerAccess::CONNECT)?;
    let svc = m
        .open_service(NAME, ServiceAccess::STOP | ServiceAccess::QUERY_STATUS | ServiceAccess::DELETE)
        .map_err(|_| "The service is not installed.".to_string())?;
    let _ = svc.stop();
    let deadline = Instant::now() + Duration::from_secs(20);
    while svc.query_status().is_ok_and(|s| s.current_state != ServiceState::Stopped) && Instant::now() < deadline {
        std::thread::sleep(Duration::from_millis(300));
    }
    svc.delete().map_err(|e| format!("Cannot delete the service: {e}"))?;
    let exe = std::env::current_exe().unwrap_or_default();
    firewall(&exe, 0, false);
    Ok("Service removed. bindings.json and tv-config.json were left in place.".into())
}

/// Whether the service starts with Windows.
pub fn auto_start() -> Option<bool> {
    let m = manager(ServiceManagerAccess::CONNECT).ok()?;
    let s = m.open_service(NAME, ServiceAccess::QUERY_CONFIG).ok()?;
    Some(s.query_config().ok()?.start_type == ServiceStartType::AutoStart)
}

pub fn set_auto_start(on: bool) -> Result<(), String> {
    use windows_sys::Win32::System::Services::*;
    unsafe {
        let scm = OpenSCManagerW(std::ptr::null(), std::ptr::null(), SC_MANAGER_CONNECT);
        if scm.is_null() {
            return Err("Cannot open the service manager.".into());
        }
        let svc = OpenServiceW(scm, wide(NAME).as_ptr(), SERVICE_CHANGE_CONFIG);
        let ok = !svc.is_null()
            && ChangeServiceConfigW(
                svc,
                SERVICE_NO_CHANGE,
                if on { SERVICE_AUTO_START } else { SERVICE_DEMAND_START },
                SERVICE_NO_CHANGE,
                std::ptr::null(),
                std::ptr::null(),
                std::ptr::null_mut(),
                std::ptr::null(),
                std::ptr::null(),
                std::ptr::null(),
                std::ptr::null(),
            ) != 0;
        if !svc.is_null() {
            CloseServiceHandle(svc);
        }
        CloseServiceHandle(scm);
        if ok {
            Ok(())
        } else {
            Err(format!("Cannot change the service start type: {}", super::last_error()))
        }
    }
}
