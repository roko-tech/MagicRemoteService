//! The TV's WebSocket endpoint: accepts connections, checks the handshake and token, then runs
//! a `Session` per TV with blocking reads bounded by the session's next timer.

use crate::log;
use crate::session::{warning_delay, Effects, Session, SessionConfig};
use crate::state::Shared;
use crate::ws;
use std::collections::HashMap;
use std::io::{ErrorKind, Read, Write};
use std::net::{IpAddr, Ipv4Addr, SocketAddr, TcpListener, TcpStream};
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};
use std::thread::JoinHandle;
use std::time::{Duration, Instant};

const MAX_HEAD: usize = 8 * 1024;
const MAX_PAYLOAD: usize = 64 * 1024;
const HANDSHAKE_TIMEOUT: Duration = Duration::from_secs(10);
/// Longest a connection sleeps in a read before checking for shutdown.
const MAX_WAIT: Duration = Duration::from_millis(250);
/// More than this many sockets at once is not a TV, so refuse the rest.
const MAX_CONNECTIONS: usize = 16;

/// What the server needs from the PC, so tests can run it without touching input.
pub trait Platform: Send + Sync {
    fn effects(&self) -> Box<dyn Effects + Send>;
    /// The display a TV's absolute pointer maps to, from the per-TV setting.
    fn display_for(&self, ip: IpAddr) -> Option<u32>;
}

impl<T: Effects + ?Sized> Effects for Box<T> {
    fn send_input(&self, e: &[crate::bindings::InputEvent]) {
        (**self).send_input(e)
    }
    fn absolute(&self, d: Option<u32>, x: u16, y: u16) -> Option<(i32, i32)> {
        (**self).absolute(d, x, y)
    }
    fn scroll_excluded(&self) -> bool {
        (**self).scroll_excluded()
    }
    fn audio_is_local(&self) -> bool {
        (**self).audio_is_local()
    }
    fn pointer_shown(&self, s: bool) {
        (**self).pointer_shown(s)
    }
    fn run_command(&self, c: &str) {
        (**self).run_command(c)
    }
    fn shutdown(&self, k: crate::session::ShutdownKind) {
        (**self).shutdown(k)
    }
    fn idle_ms(&self) -> Option<u32> {
        (**self).idle_ms()
    }
}

pub struct Server {
    port: u16,
    accept: Option<JoinHandle<()>>,
}

impl Server {
    pub fn start(listener: TcpListener, shared: Arc<Shared>, platform: Arc<dyn Platform>) -> std::io::Result<Server> {
        let port = listener.local_addr()?.port();
        let accept = std::thread::Builder::new().name("ws-accept".into()).spawn(move || accept_loop(listener, shared, platform))?;
        Ok(Server { port, accept: Some(accept) })
    }

    pub fn port(&self) -> u16 {
        self.port
    }

    /// Call after `Shared::stop`: wakes the accept loop and waits for every connection to end.
    pub fn join(mut self) {
        let _ = TcpStream::connect_timeout(&SocketAddr::from((Ipv4Addr::LOCALHOST, self.port)), Duration::from_secs(1));
        if let Some(t) = self.accept.take() {
            let _ = t.join();
        }
    }
}

pub fn bind(port: u16) -> std::io::Result<TcpListener> {
    TcpListener::bind((Ipv4Addr::UNSPECIFIED, port))
}

fn accept_loop(listener: TcpListener, shared: Arc<Shared>, platform: Arc<dyn Platform>) {
    let active = Arc::new(AtomicUsize::new(0));
    let mut threads: Vec<JoinHandle<()>> = Vec::new();
    for stream in listener.incoming() {
        if shared.stopping() {
            break;
        }
        threads.retain(|t| !t.is_finished());
        let Ok(stream) = stream else { continue };
        if active.load(Ordering::Acquire) >= MAX_CONNECTIONS {
            if let Ok(peer) = stream.peer_addr() {
                log_refusal(peer.ip(), "too many connections open");
            }
            continue;
        }
        active.fetch_add(1, Ordering::AcqRel);
        let (shared, platform, counter) = (shared.clone(), platform.clone(), active.clone());
        let spawned = std::thread::Builder::new().name("ws-conn".into()).spawn(move || {
            connection(stream, &shared, &*platform);
            counter.fetch_sub(1, Ordering::AcqRel);
        });
        match spawned {
            Ok(t) => threads.push(t),
            Err(e) => {
                active.fetch_sub(1, Ordering::AcqRel);
                log::error(format!("Cannot start a connection thread: {e}"));
            }
        }
    }
    for t in threads {
        let _ = t.join();
    }
}

enum Handshake {
    Accepted(Vec<u8>),
    Rejected(String),
    Gone,
}

/// Refusals are logged once a minute per address, so a LAN host can't flood the event log.
const REFUSAL_LOG_EVERY: Duration = Duration::from_secs(60);

/// When to log a refusal from `ip`: `Some(n)` to log it, with `n` refusals held back since the
/// last line; `None` to stay quiet.
fn note_refusal(seen: &mut HashMap<IpAddr, (Instant, u32)>, ip: IpAddr, now: Instant) -> Option<u32> {
    match seen.get_mut(&ip) {
        Some((at, held)) if now.duration_since(*at) < REFUSAL_LOG_EVERY => {
            *held += 1;
            None
        }
        entry => {
            let held = entry.map_or(0, |(_, held)| *held);
            seen.retain(|_, (at, _)| now.duration_since(*at) < REFUSAL_LOG_EVERY);
            seen.insert(ip, (now, 0));
            Some(held)
        }
    }
}

static REFUSALS: Mutex<Option<HashMap<IpAddr, (Instant, u32)>>> = Mutex::new(None);

fn log_refusal(ip: IpAddr, why: &str) {
    let mut seen = REFUSALS.lock().unwrap_or_else(|e| e.into_inner());
    if let Some(held) = note_refusal(seen.get_or_insert_with(HashMap::new), ip, Instant::now()) {
        let more = if held > 0 { format!(" ({held} more refused since the last report)") } else { String::new() };
        log::warn(format!("TV {ip}: refused, {why}{more}"));
    }
}

/// An `Origin` value safe to put in a log line.
fn printable(s: &str) -> String {
    s.chars().filter(|c| !c.is_control()).take(80).collect()
}

/// Reads and answers the upgrade request. Returns bytes received after it.
fn handshake(stream: &mut TcpStream, shared: &Shared) -> Handshake {
    // One deadline for the whole request head, so a client trickling a byte at a time can't
    // hold a connection slot open
    let deadline = Instant::now() + HANDSHAKE_TIMEOUT;
    let mut buf = Vec::with_capacity(1024);
    let mut chunk = [0u8; 2048];
    let end = loop {
        if let Some(end) = ws::head_end(&buf) {
            break end;
        }
        if buf.len() >= MAX_HEAD {
            return Handshake::Rejected("request head too large".into());
        }
        if shared.stopping() {
            return Handshake::Gone;
        }
        let left = deadline.saturating_duration_since(Instant::now());
        if left.is_zero() {
            return Handshake::Rejected("handshake not finished in time".into());
        }
        let _ = stream.set_read_timeout(Some(left.min(MAX_WAIT)));
        match stream.read(&mut chunk) {
            Ok(0) => return Handshake::Gone,
            Ok(n) => buf.extend_from_slice(&chunk[..n]),
            Err(e) if matches!(e.kind(), ErrorKind::WouldBlock | ErrorKind::TimedOut) => {}
            Err(_) => return Handshake::Gone,
        }
    };
    let head = String::from_utf8_lossy(&buf[..end]);
    let request = match ws::parse_handshake(&head) {
        Ok(r) => r,
        Err(ws::HandshakeError::NotGet) => return Handshake::Rejected("not a WebSocket upgrade".into()),
        Err(ws::HandshakeError::MissingKey) => return Handshake::Rejected("missing Sec-WebSocket-Key".into()),
    };
    if !ws::origin_allowed(request.origin.as_deref()) {
        return Handshake::Rejected(format!("web page origin {}", printable(request.origin.as_deref().unwrap_or_default())));
    }
    // When a token is configured (tv-config.json), only a TV presenting it (ws://pc:port/?t=TOKEN) may connect
    let token = shared.token();
    if !token.is_empty() {
        let given = ws::query_param(&request.target, "t").unwrap_or_default();
        if !ws::constant_time_eq(given.as_bytes(), token.as_bytes()) {
            return Handshake::Rejected("invalid or missing token".into());
        }
    }
    if stream.write_all(ws::response_101(&request.key).as_bytes()).is_err() {
        return Handshake::Gone;
    }
    Handshake::Accepted(buf.split_off(end))
}

fn connection(mut stream: TcpStream, shared: &Shared, platform: &dyn Platform) {
    let Ok(peer) = stream.peer_addr() else { return };
    let id = shared.next_id();
    let name = format!("TV {} [{id}]", peer.ip());
    let _ = stream.set_nodelay(true);
    let _ = stream.set_write_timeout(Some(Duration::from_secs(5)));
    let mut buf = match handshake(&mut stream, shared) {
        Handshake::Accepted(rest) => rest,
        Handshake::Rejected(why) => {
            log_refusal(peer.ip(), &why);
            let _ = stream.write_all(ws::RESPONSE_403);
            return;
        }
        Handshake::Gone => return,
    };
    log::info(format!("{name}: connected"));
    shared.add_client(id, peer.ip());

    let settings = shared.settings();
    let cfg = SessionConfig {
        inactivity: settings.inactivity.then(|| warning_delay(settings.timeout_inactivity_ms)),
        video_input: settings.video_input.then(|| warning_delay(settings.timeout_video_input_ms)),
        display: platform.display_for(peer.ip()),
        name: name.clone(),
    };
    let mut session = Session::new(platform.effects(), cfg, Instant::now());
    let mut monitor_seen = shared.monitor().0;
    let mut settings_seen = shared.settings_gen();
    let mut chunk = vec![0u8; 16 * 1024];
    let reason = loop {
        let now = Instant::now();
        loop {
            match ws::decode(&mut buf, MAX_PAYLOAD) {
                Ok(Some(f)) => {
                    session.on_frame(f.fin, f.opcode, &buf[f.payload.clone()], &shared.bindings(), now);
                    buf.drain(..f.len);
                }
                Ok(None) => break,
                Err(ws::FrameError::TooLarge(n)) => {
                    log::warn(format!("{name}: frame of {n} bytes exceeds the limit"));
                    session.stop();
                    break;
                }
            }
        }
        let (gen, on) = shared.monitor();
        if gen != monitor_seen {
            monitor_seen = gen;
            session.on_monitor(on, now);
        }
        if shared.settings_gen() != settings_seen {
            settings_seen = shared.settings_gen();
            let s = shared.settings();
            session.set_timeouts(
                s.inactivity.then(|| warning_delay(s.timeout_inactivity_ms)),
                s.video_input.then(|| warning_delay(s.timeout_video_input_ms)),
                now,
            );
        }
        session.tick(now);
        if shared.stopping() {
            session.stop();
        }
        if !session.out.is_empty() {
            let failed = stream.write_all(&session.out).is_err();
            session.out.clear();
            if failed {
                break "connection lost";
            }
        }
        if let Some(r) = session.closed {
            break r;
        }
        let wait = session.next_deadline().map_or(MAX_WAIT, |d| d.saturating_duration_since(Instant::now())).clamp(Duration::from_millis(1), MAX_WAIT);
        let _ = stream.set_read_timeout(Some(wait));
        match stream.read(&mut chunk) {
            Ok(0) => break "connection lost",
            Ok(n) => buf.extend_from_slice(&chunk[..n]),
            Err(e) if matches!(e.kind(), ErrorKind::WouldBlock | ErrorKind::TimedOut) => {}
            Err(_) => break "connection lost",
        }
    };
    session.finish();
    shared.remove_client(id);
    log::info(format!("{name}: disconnected ({reason})"));
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn refusals_are_logged_once_a_minute_per_address() {
        let (mut seen, t0) = (HashMap::new(), Instant::now());
        let (a, b): (IpAddr, IpAddr) = ("192.168.1.9".parse().unwrap(), "192.168.1.10".parse().unwrap());
        assert_eq!(note_refusal(&mut seen, a, t0), Some(0));
        assert_eq!(note_refusal(&mut seen, a, t0 + Duration::from_secs(1)), None);
        assert_eq!(note_refusal(&mut seen, a, t0 + Duration::from_secs(2)), None);
        assert_eq!(note_refusal(&mut seen, b, t0 + Duration::from_secs(3)), Some(0), "other addresses are not held back");
        assert_eq!(note_refusal(&mut seen, a, t0 + Duration::from_secs(61)), Some(2));
        assert_eq!(note_refusal(&mut seen, a, t0 + Duration::from_secs(62)), None);
    }

    #[test]
    fn origins_in_logs_are_trimmed_to_printable_text() {
        assert_eq!(printable("https://x\r\nfake: line"), "https://xfake: line");
        assert_eq!(printable(&"a".repeat(200)).len(), 80);
    }
}
