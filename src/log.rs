//! Logging to the Windows event log (when a sink is installed) and to a ring the web UI reads.

use std::collections::VecDeque;
use std::sync::{Mutex, OnceLock};
use std::time::{SystemTime, UNIX_EPOCH};

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum Level {
    Debug,
    Info,
    Warn,
    Error,
}

impl Level {
    pub fn as_str(self) -> &'static str {
        match self {
            Level::Debug => "debug",
            Level::Info => "info",
            Level::Warn => "warn",
            Level::Error => "error",
        }
    }
}

#[derive(Debug, Clone)]
pub struct Entry {
    /// Milliseconds since the Unix epoch.
    pub time: u64,
    pub level: Level,
    pub message: String,
}

const RING_SIZE: usize = 300;

type Sink = Box<dyn Fn(Level, &str) + Send + Sync>;

static RING: Mutex<VecDeque<Entry>> = Mutex::new(VecDeque::new());
static SINK: OnceLock<Sink> = OnceLock::new();

/// Installs where entries go besides the ring (the event log in the real program).
pub fn set_sink(sink: Sink) {
    let _ = SINK.set(sink);
}

pub fn write(level: Level, message: &str) {
    // Debug detail only in debug builds, as the C# service did
    if level == Level::Debug && !cfg!(debug_assertions) {
        return;
    }
    if let Some(sink) = SINK.get() {
        sink(level, message);
    }
    let time = SystemTime::now().duration_since(UNIX_EPOCH).map(|d| d.as_millis() as u64).unwrap_or(0);
    let mut ring = RING.lock().unwrap_or_else(|e| e.into_inner());
    if ring.len() == RING_SIZE {
        ring.pop_front();
    }
    ring.push_back(Entry { time, level, message: message.to_string() });
}

pub fn recent() -> Vec<Entry> {
    RING.lock().unwrap_or_else(|e| e.into_inner()).iter().cloned().collect()
}

pub fn debug(m: impl AsRef<str>) {
    write(Level::Debug, m.as_ref())
}
pub fn info(m: impl AsRef<str>) {
    write(Level::Info, m.as_ref())
}
pub fn warn(m: impl AsRef<str>) {
    write(Level::Warn, m.as_ref())
}
pub fn error(m: impl AsRef<str>) {
    write(Level::Error, m.as_ref())
}
