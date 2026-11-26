//! The worker: the process that listens for TVs, injects their input and serves the web UI.
//! The service starts it in the console session; run by hand it works on its own.

use crate::http::{self, Host};
use crate::log;
use crate::server::{self, Server};
use crate::settings::{Files, PcSettings, Scope};
use crate::state::Shared;
use crate::win::registry::{self, CURRENT_USER};
use crate::win::{cursor, effects::RealPlatform, eventlog, net, proc, screens, service, tray, wide, Handle};
use crate::{bindings, settings, ssdp, tv};
use serde_json::{json, Value};
use std::sync::Arc;
use std::time::Duration;
use windows_sys::Win32::Foundation::{GetLastError, ERROR_ACCESS_DENIED, ERROR_ALREADY_EXISTS};
use windows_sys::Win32::System::Threading::{CreateMutexW, OpenEventW, WaitForMultipleObjects, INFINITE, SYNCHRONIZATION_SYNCHRONIZE};

/// One worker per port, so a trial copy on another port can run beside the service.
fn mutex_name(port: u16) -> String {
    format!("Global\\MagicRemoteService.Worker.{port}")
}
const RUN_KEY: &str = "Software\\Microsoft\\Windows\\CurrentVersion\\Run";

pub struct Options {
    /// Started by the service (pid of the supervisor), rather than by hand.
    pub parent: Option<u32>,
    /// Port override for trying a build beside an installed one (standalone only).
    pub port: Option<u16>,
    pub ssdp: bool,
    /// Wait for this process (the instance being replaced by a restart) before starting.
    pub after: Option<u32>,
    /// Arguments to start again with on restart (standalone).
    pub args: Vec<String>,
}

pub fn settings_url(http_port: u16) -> String {
    format!("http://localhost:{http_port}/")
}

struct WorkerHost {
    shared: Arc<Shared>,
    scope: Scope,
    service: bool,
    ws_port: u16,
    restart_args: String,
}

impl Host for WorkerHost {
    fn mode(&self) -> &'static str {
        if self.service {
            "service"
        } else {
            "standalone"
        }
    }

    fn pc_info(&self) -> Value {
        let list: Vec<Value> =
            net::interfaces().into_iter().map(|i| json!({ "ip": i.ip.to_string(), "mac": i.mac, "mask": i.mask.to_string(), "name": i.name })).collect();
        json!({ "port": self.ws_port, "interfaces": list })
    }

    fn reinstall_tv(&self) -> Result<String, String> {
        tv::reinstall(&self.shared.files, env!("CARGO_PKG_VERSION"))
    }

    fn tvs(&self) -> Value {
        Value::Array(tv::tvs(&self.shared.files).iter().map(tv::Tv::public).collect())
    }

    fn pair_tv(&self, name: &str, ip: std::net::Ipv4Addr, passphrase: &str) -> Result<String, String> {
        tv::pair(&self.shared.files, name, ip, passphrase)
    }

    fn restart(&self) {
        let r = if self.service {
            // A full stop and start, so the service loads the exe from disk again (how updates deploy)
            proc::spawn_hidden(&proc::system_dir().join("cmd.exe").to_string_lossy(), &format!("/c net stop {0} & net start {0}", service::NAME))
        } else {
            std::env::current_exe()
                .and_then(|exe| proc::spawn_hidden(&exe.to_string_lossy(), format!("{} --after {}", self.restart_args, std::process::id()).trim_start()))
                .map(|_| tray::quit())
        };
        if let Err(e) = r {
            log::error(format!("Restart failed: {e}"));
        }
    }

    fn startup(&self) -> Option<bool> {
        if self.service {
            service::auto_start()
        } else {
            Some(registry::open_native(CURRENT_USER, RUN_KEY, false).ok()?.get_string(service::NAME).is_some())
        }
    }

    fn set_startup(&self, on: bool) -> Result<(), String> {
        if self.service {
            return service::set_auto_start(on);
        }
        let key = registry::open_native(CURRENT_USER, RUN_KEY, true).map_err(|e| e.to_string())?;
        if on {
            let exe = std::env::current_exe().map_err(|e| e.to_string())?;
            key.set_string(service::NAME, &format!("{} --standalone", proc::quote(&exe.to_string_lossy()))).map_err(|e| e.to_string())
        } else {
            let _ = key.delete_value(service::NAME);
            Ok(())
        }
    }

    fn save_settings(&self, s: &PcSettings) -> Result<(), String> {
        self.scope.save(s).map_err(|e| format!("Cannot save the settings: {e}"))
    }

    fn displays(&self) -> Value {
        let screens = screens::refresh();
        let displays: Vec<Value> = screens
            .all
            .iter()
            .map(|s| json!({ "id": s.id, "number": s.number, "name": s.name, "active": s.active, "primary": s.primary, "width": s.bounds.width, "height": s.bounds.height }))
            .collect();
        let mappings = self.scope.displays();
        let devices: Vec<Value> = tv::tvs(&self.shared.files)
            .iter()
            .map(|t| json!({ "name": t.name, "ip": t.host.to_string(), "default": t.default, "display": mappings.iter().find(|(n, _)| *n == t.name).map(|(_, v)| *v) }))
            .collect();
        json!({ "displays": displays, "devices": devices })
    }

    fn set_display(&self, device: &str, display: u32) -> Result<(), String> {
        self.scope.set_display(device, display).map_err(|e| format!("Cannot save the display: {e}"))
    }
}

/// Whether "Start with Windows" was set, without the service, by a version that registered the
/// bare exe: then a bare start means "run on its own", as it did. The entry gets `--standalone`.
pub fn legacy_standalone_startup() -> bool {
    if service::state().is_some() {
        return false;
    }
    let Ok(key) = registry::open_native(CURRENT_USER, RUN_KEY, true) else { return false };
    let Some(command) = key.get_string(service::NAME) else { return false };
    if command.contains("--standalone") {
        return false;
    }
    if let Ok(exe) = std::env::current_exe() {
        let _ = key.set_string(service::NAME, &format!("{} --standalone", proc::quote(&exe.to_string_lossy())));
    }
    true
}

/// Creates the single-instance mutex; `None` when another worker holds it.
fn single_instance(port: u16) -> Option<Handle> {
    let h = unsafe { CreateMutexW(std::ptr::null(), 1, wide(&mutex_name(port)).as_ptr()) };
    let err = unsafe { GetLastError() };
    if h.is_null() || err == ERROR_ALREADY_EXISTS || err == ERROR_ACCESS_DENIED {
        if !h.is_null() {
            drop(Handle(h));
        }
        return None;
    }
    Some(Handle(h))
}

/// Old versions' settings window kept bindings in the registry; write them to bindings.json once
/// so the web UI shows what the remote really does.
fn migrate_legacy_bindings(scope: Scope, files: &Files) {
    if files.bindings().exists() {
        return;
    }
    if let Some(map) = scope.legacy_bindings() {
        let doc = json!({ "bindings": bindings::to_json(&map) });
        match settings::write_atomic(&files.bindings(), &serde_json::to_vec_pretty(&doc).unwrap_or_default()) {
            Ok(()) => log::info("Moved the key bindings from the registry to bindings.json"),
            Err(e) => log::warn(format!("Could not write bindings.json: {e}")),
        }
    }
}

/// In service mode, ends the message loop when the service asks or the service process dies.
fn watch_supervisor(parent: u32) {
    let event = unsafe { OpenEventW(SYNCHRONIZATION_SYNCHRONIZE, 0, wide(service::STOP_EVENT).as_ptr()) };
    let parent = proc::open_for_wait(parent);
    let mut handles = Vec::new();
    if !event.is_null() {
        handles.push(event);
    }
    if let Some(p) = &parent {
        handles.push(p.0);
    }
    if handles.is_empty() {
        log::warn("Worker cannot watch the service; it will run until stopped");
        return;
    }
    let handles_raw: Vec<usize> = handles.iter().map(|h| *h as usize).collect();
    std::thread::spawn(move || {
        let hs: Vec<_> = handles_raw.iter().map(|h| *h as _).collect();
        unsafe { WaitForMultipleObjects(hs.len() as u32, hs.as_ptr(), 0, INFINITE) };
        tray::quit();
        drop(parent);
    });
}

pub fn run(opts: Options) -> i32 {
    if let Some(pid) = opts.after {
        if let Some(h) = proc::open_for_wait(pid) {
            proc::wait_exit(&h, Duration::from_secs(15));
        }
    }
    eventlog::install_sink();
    let service_mode = opts.parent.is_some();
    let scope = Scope::current();
    let mut pc = scope.load();
    if let (Some(port), false) = (opts.port, service_mode) {
        pc.port = port;
    }
    let Some(_instance) = single_instance(pc.port) else {
        if service_mode {
            log::warn("Another worker is already running");
            return 2;
        }
        // Started by hand while another worker runs: show that one's settings
        return crate::ui::run(Scope::Machine.load().port + 1);
    };
    let files = Files { dir: crate::win::exe_dir() };
    // The device token is in tv-config.json: as the service, keep ordinary accounts out of it
    if proc::is_system() && files.tv_config().exists() {
        if let Err(e) = crate::win::acl::restrict_to_admins(&files.tv_config()) {
            log::warn(format!("Could not restrict access to tv-config.json: {e}"));
        }
    }
    migrate_legacy_bindings(scope, &files);
    screens::refresh();
    let shared = Shared::new(files, pc.clone());
    log::info(format!(
        "Worker {} started ({}), device token auth {}",
        env!("CARGO_PKG_VERSION"),
        if service_mode { "service" } else { "standalone" },
        if shared.token().is_empty() { "off (open mode)" } else { "on" }
    ));

    let listener = match server::bind(pc.port) {
        Ok(l) => l,
        Err(e) => {
            log::error(format!("Cannot listen on port {}: {e}", pc.port));
            return 3;
        }
    };
    let platform = Arc::new(RealPlatform { shared: shared.clone(), scope });
    let ws = match Server::start(listener, shared.clone(), platform) {
        Ok(s) => s,
        Err(e) => {
            log::error(format!("Cannot start the TV server: {e}"));
            return 3;
        }
    };
    let http_port = pc.port + 1;
    let web: Vec<Arc<tiny_http::Server>> = match http::bind(http_port) {
        Ok(list) => list.into_iter().map(Arc::new).collect(),
        Err(e) => {
            log::error(format!("Cannot start the web UI on port {http_port}: {e}"));
            Vec::new()
        }
    };
    let host: Arc<dyn Host> =
        Arc::new(WorkerHost { shared: shared.clone(), scope, service: service_mode, ws_port: pc.port, restart_args: opts.args.join(" ") });
    let web_threads: Vec<_> = web
        .iter()
        .map(|w| {
            let (w, shared, host) = (w.clone(), shared.clone(), host.clone());
            std::thread::spawn(move || http::serve(w, shared, host, pc.port))
        })
        .collect();
    let ssdp_thread = if opts.ssdp {
        let ips: Vec<_> = net::interfaces().into_iter().map(|i| i.ip).collect();
        match ssdp::bind(&ips) {
            Ok(sock) => {
                let shared = shared.clone();
                Some(std::thread::spawn(move || ssdp::run(sock, shared, pc.port)))
            }
            Err(e) => {
                log::warn(format!("SSDP discovery unavailable: {e}"));
                None
            }
        }
    } else {
        None
    };
    log::info(format!("Listening for TVs on port {}, settings at {}", pc.port, settings_url(http_port)));

    shared.on_clients_changed(Box::new(tray::set_tv_count));
    if let Some(parent) = opts.parent {
        watch_supervisor(parent);
    }
    let monitor_shared = shared.clone();
    tray::run(tray::Config {
        on_settings: Box::new(move || crate::ui::launch(http_port)),
        on_monitor: Box::new(move |on| monitor_shared.set_monitor(on)),
        can_exit: !service_mode,
    });

    // Close every TV connection cleanly (keys released, pointer restored) before exiting
    shared.stop();
    ws.join();
    for w in &web {
        w.unblock();
    }
    for t in web_threads {
        let _ = t.join();
    }
    if let Some(t) = ssdp_thread {
        let _ = t.join();
    }
    cursor::restore_all();
    log::info("Worker stopped");
    0
}
