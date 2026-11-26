//! Drives the real WebSocket server and HTTP API over loopback sockets, with a fake PC.

use mrs::bindings::{InputEvent, MouseButton};
use mrs::http::{self, Ctx, Host, Request};
use mrs::server::{Platform, Server};
use mrs::session::{Effects, ShutdownKind};
use mrs::settings::{Files, PcSettings};
use mrs::state::Shared;
use serde_json::{json, Value};
use std::io::{Read, Write};
use std::net::{IpAddr, Ipv4Addr, TcpListener, TcpStream};
use std::path::PathBuf;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

#[derive(Debug, Clone, PartialEq)]
enum Call {
    Input(Vec<InputEvent>),
    Pointer(bool),
    Command(String),
    Shutdown(ShutdownKind),
}

#[derive(Default)]
struct Recorder(Mutex<Vec<Call>>);

impl Recorder {
    fn push(&self, c: Call) {
        self.0.lock().unwrap().push(c);
    }
    fn take(&self) -> Vec<Call> {
        std::mem::take(&mut *self.0.lock().unwrap())
    }
    /// Waits until at least `n` calls arrived (the server works on its own thread).
    fn wait_for(&self, n: usize) -> Vec<Call> {
        let deadline = Instant::now() + Duration::from_secs(5);
        while self.0.lock().unwrap().len() < n && Instant::now() < deadline {
            std::thread::sleep(Duration::from_millis(5));
        }
        self.take()
    }
}

struct FakeEffects(Arc<Recorder>);

impl Effects for FakeEffects {
    fn send_input(&self, e: &[InputEvent]) {
        self.0.push(Call::Input(e.to_vec()));
    }
    fn absolute(&self, _: Option<u32>, x: u16, y: u16) -> Option<(i32, i32)> {
        Some((x as i32, y as i32))
    }
    fn scroll_excluded(&self) -> bool {
        false
    }
    fn audio_is_local(&self) -> bool {
        false
    }
    fn pointer_shown(&self, s: bool) {
        self.0.push(Call::Pointer(s));
    }
    fn run_command(&self, c: &str) {
        self.0.push(Call::Command(c.into()));
    }
    fn shutdown(&self, k: ShutdownKind) {
        self.0.push(Call::Shutdown(k));
    }
    fn idle_ms(&self) -> Option<u32> {
        None
    }
}

struct FakePlatform(Arc<Recorder>);

impl Platform for FakePlatform {
    fn effects(&self) -> Box<dyn Effects + Send> {
        Box::new(FakeEffects(self.0.clone()))
    }
    fn display_for(&self, _: IpAddr) -> Option<u32> {
        None
    }
}

fn temp_dir(name: &str) -> PathBuf {
    let d = std::env::temp_dir().join(format!("mrs-it-{name}-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&d);
    std::fs::create_dir_all(&d).unwrap();
    d
}

struct Rig {
    shared: Arc<Shared>,
    server: Option<Server>,
    rec: Arc<Recorder>,
    dir: PathBuf,
}

impl Rig {
    fn new(name: &str, tv_config: Option<Value>) -> Rig {
        let dir = temp_dir(name);
        if let Some(c) = tv_config {
            std::fs::write(dir.join("tv-config.json"), c.to_string()).unwrap();
        }
        let shared = Shared::new(Files { dir: dir.clone() }, PcSettings::default());
        let rec = Arc::new(Recorder::default());
        let listener = TcpListener::bind((Ipv4Addr::LOCALHOST, 0)).unwrap();
        let server = Server::start(listener, shared.clone(), Arc::new(FakePlatform(rec.clone()))).unwrap();
        Rig { shared, server: Some(server), rec, dir }
    }

    fn port(&self) -> u16 {
        self.server.as_ref().unwrap().port()
    }

    fn stop(&mut self) {
        self.shared.stop();
        if let Some(s) = self.server.take() {
            s.join();
        }
    }
}

impl Drop for Rig {
    fn drop(&mut self) {
        self.stop();
        let _ = std::fs::remove_dir_all(&self.dir);
    }
}

/// Opens a WebSocket as main.js does; returns the stream and the HTTP status line.
fn upgrade(port: u16, target: &str) -> (TcpStream, String) {
    let mut s = TcpStream::connect((Ipv4Addr::LOCALHOST, port)).unwrap();
    s.set_read_timeout(Some(Duration::from_secs(5))).unwrap();
    write!(s, "GET {target} HTTP/1.1\r\nHost: pc\r\nUpgrade: websocket\r\nConnection: Upgrade\r\nSec-WebSocket-Key: dGhlIHNhbXBsZSBub25jZQ==\r\nSec-WebSocket-Version: 13\r\n\r\n").unwrap();
    let mut head = Vec::new();
    let mut b = [0u8; 1];
    while !head.ends_with(b"\r\n\r\n") {
        if s.read(&mut b).unwrap_or(0) == 0 {
            break;
        }
        head.push(b[0]);
    }
    let head = String::from_utf8(head).unwrap();
    if head.starts_with("HTTP/1.1 101") {
        assert!(head.contains("Sec-WebSocket-Accept: s3pPLMBiTxaQ9kYGzzhZRbK+xOo=\r\n"), "{head}");
    }
    (s, head.lines().next().unwrap_or_default().to_string())
}

/// Sends a masked client frame.
fn send(s: &mut TcpStream, opcode: u8, payload: &[u8]) {
    let mask = [0x12, 0x34, 0x56, 0x78];
    let mut f = vec![0x80 | opcode, 0x80 | payload.len() as u8];
    f.extend_from_slice(&mask);
    f.extend(payload.iter().enumerate().map(|(i, b)| b ^ mask[i % 4]));
    s.write_all(&f).unwrap();
}

/// Reads the next server frame other than a heartbeat (an empty binary message): (opcode, payload).
fn recv(s: &mut TcpStream) -> (u8, Vec<u8>) {
    loop {
        let mut h = [0u8; 2];
        s.read_exact(&mut h).unwrap();
        assert_eq!(h[1] & 0x80, 0, "server frames are unmasked");
        let mut p = vec![0u8; (h[1] & 0x7F) as usize];
        s.read_exact(&mut p).unwrap();
        if (h[0] & 0x0F, p.len()) != (0x2, 0) {
            return (h[0] & 0x0F, p);
        }
    }
}

#[test]
fn token_gates_the_upgrade() {
    let rig = Rig::new("token", Some(json!({ "token": "s3cret/42" })));
    assert_eq!(upgrade(rig.port(), "/").1, "HTTP/1.1 403 Forbidden");
    assert_eq!(upgrade(rig.port(), "/?t=wrong").1, "HTTP/1.1 403 Forbidden");
    assert_eq!(upgrade(rig.port(), "/?t=s3cret%2F42").1, "HTTP/1.1 101 Switching Protocols");
    // Not a WebSocket at all
    let mut s = TcpStream::connect((Ipv4Addr::LOCALHOST, rig.port())).unwrap();
    s.write_all(b"POST / HTTP/1.1\r\n\r\n").unwrap();
    let mut out = String::new();
    s.read_to_string(&mut out).unwrap();
    assert!(out.starts_with("HTTP/1.1 403"));
}

#[test]
fn a_tv_session_end_to_end() {
    let rig = Rig::new("session", None);
    let (mut tv, status) = upgrade(rig.port(), "/");
    assert_eq!(status, "HTTP/1.1 101 Switching Protocols");
    let deadline = Instant::now() + Duration::from_secs(5);
    while rig.shared.clients().is_empty() && Instant::now() < deadline {
        std::thread::sleep(Duration::from_millis(5));
    }
    assert_eq!(rig.shared.clients().len(), 1);

    // Pointer, then OK held down
    send(&mut tv, 0x2, &[0x03, 0x01]);
    send(&mut tv, 0x2, &[0x00, 0x05, 0x00, 0xFB, 0xFF]);
    send(&mut tv, 0x2, &[0x04, 0x01, 0x00, 0x01]);
    let calls = rig.rec.wait_for(3);
    assert_eq!(
        calls,
        vec![
            Call::Pointer(true),
            Call::Input(vec![InputEvent::MoveRelative { dx: 5, dy: -5 }]),
            Call::Input(vec![InputEvent::Button { button: MouseButton::Left, down: true }]),
        ]
    );

    // Blue asks the TV to toggle its keyboard
    send(&mut tv, 0x2, &[0x04, 0x96, 0x01, 0x01]);
    assert_eq!(recv(&mut tv), (0x2, vec![0x02]));

    // Pings are answered with the same payload
    send(&mut tv, 0x9, b"hey");
    assert_eq!(recv(&mut tv), (0xA, b"hey".to_vec()));

    // Bindings saved while connected apply to the next press
    std::fs::write(rig.dir.join("bindings.json"), json!({"bindings": {"0x0030": [{"type": "command", "command": "calc.exe"}]}}).to_string()).unwrap();
    rig.shared.reload_bindings();
    send(&mut tv, 0x2, &[0x04, 0x30, 0x00, 0x01]);
    assert_eq!(rig.rec.wait_for(1), vec![Call::Command("calc.exe".into())]);

    // The TV vanishes without a close frame: the held button is released and the pointer restored
    drop(tv);
    let calls = rig.rec.wait_for(2);
    assert_eq!(calls, vec![Call::Input(vec![InputEvent::Button { button: MouseButton::Left, down: false }]), Call::Pointer(false)]);
    let deadline = Instant::now() + Duration::from_secs(5);
    while !rig.shared.clients().is_empty() && Instant::now() < deadline {
        std::thread::sleep(Duration::from_millis(5));
    }
    assert!(rig.shared.clients().is_empty());
}

#[test]
fn live_settings_reach_a_tv_app_that_says_hello() {
    let rig = Rig::new("live", Some(json!({ "cursorSpeed": 2.5 })));
    let (mut tv, status) = upgrade(rig.port(), "/");
    assert_eq!(status, "HTTP/1.1 101 Switching Protocols");
    let settings = |tv: &mut TcpStream| {
        let (op, p) = recv(tv);
        assert_eq!((op, p[0]), (0x2, 0x04));
        serde_json::from_slice::<Value>(&p[1..]).unwrap()
    };
    // Right after the hello
    send(&mut tv, 0x2, &[0x08, 0x01]);
    assert_eq!(settings(&mut tv), json!({ "cursorSpeed": 2.5, "longClick": 1500, "scrollSpeed": 1.0 }));
    // And again when they are saved while it is connected
    std::fs::write(rig.dir.join("tv-config.json"), json!({ "cursorSpeed": 2.5, "longClick": 800, "scrollSpeed": 3 }).to_string()).unwrap();
    rig.shared.reload_tv_config();
    assert_eq!(settings(&mut tv), json!({ "cursorSpeed": 2.5, "longClick": 800, "scrollSpeed": 3.0 }));
}

#[test]
fn close_frames_are_echoed_and_stopping_says_goodbye() {
    let mut rig = Rig::new("close", None);
    let (mut a, _) = upgrade(rig.port(), "/");
    send(&mut a, 0x8, &[0x03, 0xE8]);
    assert_eq!(recv(&mut a), (0x8, vec![0x03, 0xE8]));

    let (mut b, _) = upgrade(rig.port(), "/");
    rig.stop();
    assert_eq!(recv(&mut b), (0x8, vec![]));
}

#[test]
fn split_frames_and_pipelined_data_are_reassembled() {
    let rig = Rig::new("split", None);
    let (mut tv, _) = upgrade(rig.port(), "/");
    let mask = [1, 2, 3, 4];
    let payload = [0x05u8, 0x41, 0x00];
    let mut f = vec![0x82, 0x83];
    f.extend_from_slice(&mask);
    f.extend(payload.iter().enumerate().map(|(i, b)| b ^ mask[i % 4]));
    for byte in &f {
        tv.write_all(&[*byte]).unwrap();
        tv.flush().unwrap();
        std::thread::sleep(Duration::from_millis(2));
    }
    assert_eq!(rig.rec.wait_for(1), vec![Call::Input(vec![InputEvent::Unicode { unit: 0x41, down: true }, InputEvent::Unicode { unit: 0x41, down: false }])]);
}

struct FakeHost {
    restarts: Mutex<u32>,
    saved: Mutex<Option<PcSettings>>,
}

impl Host for FakeHost {
    fn mode(&self) -> &'static str {
        "standalone"
    }
    fn pc_info(&self) -> Value {
        json!({ "port": 41230, "interfaces": [] })
    }
    fn reinstall_tv(&self) -> Result<String, String> {
        Err("no TV here".into())
    }
    fn tvs(&self) -> Value {
        json!([])
    }
    fn pair_tv(&self, _: &str, _: std::net::Ipv4Addr, _: &str) -> Result<String, String> {
        Err("no TV here".into())
    }
    fn restart(&self) {
        *self.restarts.lock().unwrap() += 1;
    }
    fn startup(&self) -> Option<bool> {
        Some(true)
    }
    fn set_startup(&self, _: bool) -> Result<(), String> {
        Ok(())
    }
    fn save_settings(&self, s: &PcSettings) -> Result<(), String> {
        *self.saved.lock().unwrap() = Some(s.clone());
        Ok(())
    }
    fn displays(&self) -> Value {
        json!({ "displays": [], "devices": [] })
    }
    fn set_display(&self, _: &str, _: u32) -> Result<(), String> {
        Ok(())
    }
}

fn api(shared: &Shared, host: &FakeHost, method: &str, url: &str, body: &str, xrw: bool) -> (u16, Value) {
    let req = Request { method, url, host: Some("localhost:41231"), origin: None, requested_with: xrw.then_some("MagicRemoteService"), body: body.as_bytes() };
    let r = http::route(&req, &Ctx { shared, host, ws_port: 41230, http_port: 41231 });
    if let Some(after) = r.after {
        after(host);
    }
    (r.status, serde_json::from_slice(&r.body).unwrap_or(Value::Null))
}

#[test]
fn http_api_keeps_the_c_sharp_contract_and_guards_requests() {
    let dir = temp_dir("http");
    let shared = Shared::new(Files { dir: dir.clone() }, PcSettings::default());
    let host = FakeHost { restarts: Mutex::new(0), saved: Mutex::new(None) };

    let (code, s) = api(&shared, &host, "GET", "/api/settings", "", false);
    assert_eq!(code, 200);
    for key in ["port", "inactivity", "timeoutInactivity", "videoInput", "timeoutVideoInput", "connectedClients", "clientIp", "scrollExclude", "bindings"] {
        assert!(s.get(key).is_some(), "missing {key}");
    }
    assert_eq!(s["port"], 41230);
    assert_eq!(s["scrollExclude"].as_array().unwrap().len(), 4);

    let (_, d) = api(&shared, &host, "GET", "/api/defaults", "", false);
    assert_eq!(
        d["0x0021"],
        json!([{"type": "keyboard", "virtualKey": 17, "scanCode": 29, "extended": false}, {"type": "keyboard", "virtualKey": 67, "scanCode": 46, "extended": false}])
    );

    // POSTs need the custom header, and hosts other than localhost are refused (DNS rebinding)
    assert_eq!(api(&shared, &host, "POST", "/api/restart", "", false).0, 403);
    let evil = Request { method: "GET", url: "/api/config", host: Some("evil.example:41231"), origin: None, requested_with: None, body: b"" };
    assert_eq!(http::route(&evil, &Ctx { shared: &shared, host: &host, ws_port: 41230, http_port: 41231 }).status, 403);
    let cross = Request {
        method: "POST",
        url: "/api/config",
        host: Some("localhost:41231"),
        origin: Some("http://evil.example"),
        requested_with: Some("x"),
        body: b"{}",
    };
    assert_eq!(http::route(&cross, &Ctx { shared: &shared, host: &host, ws_port: 41230, http_port: 41231 }).status, 403);

    // Bindings: validated, written, applied
    assert_eq!(api(&shared, &host, "POST", "/api/bindings", "[1,2]", true).0, 400);
    assert_eq!(api(&shared, &host, "POST", "/api/bindings", r#"{"bindings": []}"#, true).0, 400);
    let body = r#"{"bindings":{"0x0030":[{"type":"mouse","value":"right"}]},"scrollExclude":["vlc"]}"#;
    assert_eq!(api(&shared, &host, "POST", "/api/bindings", body, true), (200, json!({"saved": true, "applied": true})));
    assert!(shared.is_scroll_excluded("VLC"));
    assert!(matches!(shared.bindings().plans.get(&0x0030), Some(mrs::bindings::Plan::Inputs { .. })));
    assert_eq!(api(&shared, &host, "GET", "/api/settings", "", false).1["bindings"]["bindings"]["0x0030"][0]["value"], "right");

    // Profiles: saved, applied, and kept by a save that doesn't mention them
    let with_profile = r#"{"bindings":{},"profiles":[{"name":"Media","apps":["vlc"],"bindings":{"0x0193":[]}}]}"#;
    assert_eq!(api(&shared, &host, "POST", "/api/bindings", with_profile, true).0, 200);
    assert!(shared.bindings().has_profiles());
    assert_eq!(api(&shared, &host, "POST", "/api/bindings", r#"{"bindings":{"0x0031":[]}}"#, true).0, 200);
    assert!(shared.bindings().has_profiles(), "an older page's save must not wipe profiles");
    assert_eq!(api(&shared, &host, "GET", "/api/settings", "", false).1["bindings"]["profiles"][0]["name"], "Media");
    assert_eq!(api(&shared, &host, "POST", "/api/bindings", r#"{"profiles":{}}"#, true).0, 400);
    assert_eq!(api(&shared, &host, "POST", "/api/bindings", r#"{"bindings":{},"profiles":[]}"#, true).0, 200);
    assert!(!shared.bindings().has_profiles(), "an explicit empty list removes them");

    // TV config: the token applies at once
    assert_eq!(api(&shared, &host, "POST", "/api/config", r#"{"token":"abc","appId":"com.cathwyler.magicremoteservice.hdmi1"}"#, true).0, 200);
    assert_eq!(shared.token(), "abc");
    // The API never shows the token, and saving what it shows keeps it
    let shown = api(&shared, &host, "GET", "/api/config", "", false).1;
    assert_eq!((shown.get("token"), &shown["hasToken"]), (None, &json!(true)));
    assert_eq!(api(&shared, &host, "POST", "/api/config", &shown.to_string(), true).0, 200);
    assert_eq!(shared.token(), "abc");
    assert_eq!(api(&shared, &host, "GET", "/api/settings", "", false).1["hasToken"], true);
    // A generated token replaces it at once
    assert_eq!(api(&shared, &host, "POST", "/api/token", "", true), (200, json!({"saved": true, "hasToken": true})));
    let token = shared.token();
    assert!(token.len() == 32 && token != "abc");
    assert_eq!(api(&shared, &host, "GET", "/api/config", "", false).1["appId"], "com.cathwyler.magicremoteservice.hdmi1");

    // PC settings are validated; a port change needs a restart, timeouts don't
    assert_eq!(api(&shared, &host, "POST", "/api/pc", r#"{"port": 80}"#, true).0, 400);
    let (code, r) = api(&shared, &host, "POST", "/api/pc", r#"{"port": 41240, "timeoutInactivity": 3600000}"#, true);
    assert_eq!((code, r["restartRequired"].clone()), (200, json!(true)));
    assert_eq!(host.saved.lock().unwrap().as_ref().unwrap().timeout_inactivity_ms, 3_600_000);
    assert_eq!(shared.settings().timeout_inactivity_ms, 3_600_000);

    assert_eq!(api(&shared, &host, "POST", "/api/reinstall", "", true).1, json!({"success": false, "message": "no TV here"}));

    // Paired TVs: pairing needs an address; the list is edited by name and never shows a passphrase
    assert_eq!(api(&shared, &host, "POST", "/api/tvs/pair", r#"{"ip": "tv", "passphrase": "ABC123"}"#, true).0, 400);
    assert_eq!(api(&shared, &host, "POST", "/api/tvs/pair", r#"{"ip": "192.168.1.5", "passphrase": "ABC123"}"#, false).0, 403);
    std::fs::write(shared.files.tvs(), r#"{"tvs": [{"name": "C2", "host": "192.168.1.5", "passphrase": "ABC123", "default": true}]}"#).unwrap();
    assert_eq!(api(&shared, &host, "POST", "/api/tvs/default", r#"{"name": "Kitchen"}"#, true).0, 400);
    assert_eq!(api(&shared, &host, "POST", "/api/tvs/default", r#"{"name": "C2"}"#, true).0, 200);
    assert_eq!(api(&shared, &host, "POST", "/api/tvs/remove", r#"{"name": "C2"}"#, true).0, 200);
    assert_eq!(serde_json::from_slice::<Value>(&std::fs::read(shared.files.tvs()).unwrap()).unwrap(), json!({"tvs": []}));
    assert_eq!(api(&shared, &host, "POST", "/api/restart", "", true).1, json!({"restarting": true}));
    assert_eq!(*host.restarts.lock().unwrap(), 1);
    assert_eq!(api(&shared, &host, "GET", "/api/nope", "", false).0, 404);
    let page = http::route(
        &Request { method: "GET", url: "/", host: Some("127.0.0.1:41231"), origin: None, requested_with: None, body: b"" },
        &Ctx { shared: &shared, host: &host, ws_port: 41230, http_port: 41231 },
    );
    assert_eq!(page.content_type, "text/html; charset=utf-8");
    let _ = std::fs::remove_dir_all(dir);
}

#[test]
fn http_server_answers_over_a_real_socket() {
    let dir = temp_dir("http-socket");
    let shared = Shared::new(Files { dir: dir.clone() }, PcSettings::default());
    let server = Arc::new(http::bind(0).unwrap().remove(0));
    let port = server.server_addr().to_ip().unwrap().port();
    let host: Arc<dyn Host> = Arc::new(FakeHost { restarts: Mutex::new(0), saved: Mutex::new(None) });
    let t = {
        let (server, shared) = (server.clone(), shared.clone());
        std::thread::spawn(move || http::serve(server, shared, host, 41230))
    };
    let body = mrs::client::get(port, "/api/settings").unwrap();
    assert_eq!(serde_json::from_str::<Value>(&body).unwrap()["connectedClients"], 0);
    assert!(mrs::client::get(port, "/").unwrap().contains("<html"));
    server.unblock();
    t.join().unwrap();
    let _ = std::fs::remove_dir_all(dir);
}

#[test]
fn a_trickled_handshake_is_cut_off_after_ten_seconds() {
    let rig = Rig::new("trickle", None);
    let mut s = TcpStream::connect((Ipv4Addr::LOCALHOST, rig.port())).unwrap();
    let start = Instant::now();
    let writer = {
        let mut w = s.try_clone().unwrap();
        std::thread::spawn(move || {
            // One byte a second, never finishing the request head
            for b in b"GET / HTTP/1.1\r\nHost: pc\r\nX-Slow: aaaaaaaaaaaaaaaaaaaa".iter() {
                if w.write_all(&[*b]).is_err() {
                    break;
                }
                std::thread::sleep(Duration::from_secs(1));
            }
        })
    };
    s.set_read_timeout(Some(Duration::from_secs(20))).unwrap();
    let mut out = String::new();
    let _ = s.read_to_string(&mut out);
    let took = start.elapsed();
    assert!(out.starts_with("HTTP/1.1 403"), "{out:?}");
    assert!(took >= Duration::from_millis(9_500) && took < Duration::from_secs(13), "{took:?}");
    drop(s);
    let _ = writer.join();
}

#[test]
fn web_pages_cannot_open_the_websocket() {
    let rig = Rig::new("origin", None);
    let attempt = |origin: &str| {
        let mut s = TcpStream::connect((Ipv4Addr::LOCALHOST, rig.port())).unwrap();
        s.set_read_timeout(Some(Duration::from_secs(5))).unwrap();
        write!(s, "GET / HTTP/1.1\r\nHost: pc\r\nOrigin: {origin}\r\nUpgrade: websocket\r\nConnection: Upgrade\r\nSec-WebSocket-Key: dGhlIHNhbXBsZSBub25jZQ==\r\n\r\n").unwrap();
        let mut head = [0u8; 12];
        s.read_exact(&mut head).unwrap();
        String::from_utf8_lossy(&head).into_owned()
    };
    assert_eq!(attempt("https://evil.example"), "HTTP/1.1 403");
    assert_eq!(attempt("http://192.168.1.50"), "HTTP/1.1 403");
    // What the TV app sends
    assert_eq!(attempt("file://"), "HTTP/1.1 101");
    assert_eq!(attempt("null"), "HTTP/1.1 101");
}
