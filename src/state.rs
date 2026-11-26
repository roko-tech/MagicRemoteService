//! State shared by the listeners, the connections and the web UI. Bindings, the token, the
//! scroll exclusions and the timeouts reload in place, so saving them needs no restart.

use crate::bindings::{self, Compiled};
use crate::log;
use crate::settings::{self, Files, PcSettings};
use serde_json::Value;
use std::net::IpAddr;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Arc, Mutex, RwLock};
use std::time::SystemTime;

#[derive(Debug, Clone)]
pub struct Client {
    pub id: u64,
    pub ip: IpAddr,
    pub since: SystemTime,
    pub stats: crate::session::Stats,
}

type Listener = Box<dyn Fn(usize) + Send + Sync>;

pub struct Shared {
    pub files: Files,
    settings: RwLock<PcSettings>,
    bindings: RwLock<Arc<Compiled>>,
    token: RwLock<String>,
    scroll_exclude: RwLock<Vec<String>>,
    clients: Mutex<Vec<Client>>,
    on_clients: Mutex<Option<Listener>>,
    /// `generation << 1 | on` for the PC's monitor power.
    monitor: AtomicU64,
    settings_gen: AtomicU64,
    next_id: AtomicU64,
    stopping: AtomicBool,
}

impl Shared {
    pub fn new(files: Files, settings: PcSettings) -> Arc<Shared> {
        let s = Arc::new(Shared {
            files,
            settings: RwLock::new(settings),
            bindings: RwLock::new(Arc::new(Compiled::default())),
            token: RwLock::new(String::new()),
            scroll_exclude: RwLock::new(Vec::new()),
            clients: Mutex::new(Vec::new()),
            on_clients: Mutex::new(None),
            monitor: AtomicU64::new(1),
            settings_gen: AtomicU64::new(0),
            next_id: AtomicU64::new(1),
            stopping: AtomicBool::new(false),
        });
        s.reload_bindings();
        s.reload_tv_config();
        s
    }

    pub fn settings(&self) -> PcSettings {
        self.settings.read().unwrap_or_else(|e| e.into_inner()).clone()
    }

    pub fn set_settings(&self, s: PcSettings) {
        *self.settings.write().unwrap_or_else(|e| e.into_inner()) = s;
        self.settings_gen.fetch_add(1, Ordering::Release);
    }

    /// Changes each time the settings are saved, so connected TVs pick up new timeouts.
    pub fn settings_gen(&self) -> u64 {
        self.settings_gen.load(Ordering::Acquire)
    }

    /// Re-reads `bindings.json` (defaults when absent) and the scroll exclusions in it.
    pub fn reload_bindings(&self) {
        let doc = settings::read_json(&self.files.bindings());
        let (map, problems) = match &doc {
            Some(d) => bindings::from_file(d),
            None => (bindings::defaults(), Vec::new()),
        };
        for p in &problems {
            log::warn(format!("bindings.json: {p}"));
        }
        *self.bindings.write().unwrap_or_else(|e| e.into_inner()) = Arc::new(Compiled::new(&map));
        *self.scroll_exclude.write().unwrap_or_else(|e| e.into_inner()) = settings::scroll_exclude(doc.as_ref());
    }

    /// Re-reads the device token from `tv-config.json`.
    pub fn reload_tv_config(&self) {
        let token = settings::token(settings::read_json(&self.files.tv_config()).as_ref());
        *self.token.write().unwrap_or_else(|e| e.into_inner()) = token;
    }

    pub fn bindings(&self) -> Arc<Compiled> {
        self.bindings.read().unwrap_or_else(|e| e.into_inner()).clone()
    }

    pub fn token(&self) -> String {
        self.token.read().unwrap_or_else(|e| e.into_inner()).clone()
    }

    pub fn scroll_exclude(&self) -> Vec<String> {
        self.scroll_exclude.read().unwrap_or_else(|e| e.into_inner()).clone()
    }

    pub fn is_scroll_excluded(&self, process: &str) -> bool {
        self.scroll_exclude.read().unwrap_or_else(|e| e.into_inner()).iter().any(|p| p.eq_ignore_ascii_case(process))
    }

    pub fn next_id(&self) -> u64 {
        self.next_id.fetch_add(1, Ordering::Relaxed)
    }

    pub fn add_client(&self, id: u64, ip: IpAddr) {
        let n = {
            let mut c = self.clients.lock().unwrap_or_else(|e| e.into_inner());
            c.push(Client { id, ip, since: SystemTime::now(), stats: Default::default() });
            c.len()
        };
        self.notify(n);
    }

    pub fn remove_client(&self, id: u64) {
        let n = {
            let mut c = self.clients.lock().unwrap_or_else(|e| e.into_inner());
            c.retain(|c| c.id != id);
            c.len()
        };
        self.notify(n);
    }

    /// The connection's latest counters, for the web UI.
    pub fn set_client_stats(&self, id: u64, stats: crate::session::Stats) {
        if let Some(c) = self.clients.lock().unwrap_or_else(|e| e.into_inner()).iter_mut().find(|c| c.id == id) {
            c.stats = stats;
        }
    }

    pub fn clients(&self) -> Vec<Client> {
        self.clients.lock().unwrap_or_else(|e| e.into_inner()).clone()
    }

    /// Called with the number of connected TVs whenever it changes (the tray tooltip).
    pub fn on_clients_changed(&self, f: Listener) {
        *self.on_clients.lock().unwrap_or_else(|e| e.into_inner()) = Some(f);
    }

    fn notify(&self, n: usize) {
        if let Some(f) = self.on_clients.lock().unwrap_or_else(|e| e.into_inner()).as_ref() {
            f(n);
        }
    }

    pub fn set_monitor(&self, on: bool) {
        let gen = (self.monitor.load(Ordering::Relaxed) >> 1) + 1;
        self.monitor.store(gen << 1 | on as u64, Ordering::Release);
    }

    /// `(generation, on)`; the generation changes on every power event.
    pub fn monitor(&self) -> (u64, bool) {
        let m = self.monitor.load(Ordering::Acquire);
        (m >> 1, m & 1 == 1)
    }

    pub fn stop(&self) {
        self.stopping.store(true, Ordering::Release);
    }

    pub fn stopping(&self) -> bool {
        self.stopping.load(Ordering::Acquire)
    }

    /// The raw `bindings.json` document for the web UI (`{}` when absent or unreadable).
    pub fn bindings_doc(&self) -> Value {
        settings::read_json(&self.files.bindings()).unwrap_or_else(|| Value::Object(Default::default()))
    }
}
