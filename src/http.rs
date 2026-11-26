//! The settings web UI and its JSON API, on loopback only.

use crate::bindings;
use crate::log;
use crate::settings::{self, PcSettings};
use crate::state::Shared;
use serde_json::{json, Value};
use std::io::Read;
use std::net::{Ipv4Addr, Ipv6Addr};
use std::sync::Arc;
use std::time::UNIX_EPOCH;

pub static SETTINGS_HTML: &str = include_str!("../resources/settings-ui.html");
const MAX_BODY: usize = 1024 * 1024;

/// What the API needs from the running program, so tests can stand in for it.
pub trait Host: Send + Sync {
    fn mode(&self) -> &'static str;
    fn pc_info(&self) -> Value;
    fn reinstall_tv(&self) -> Result<String, String>;
    /// Restart after the response is sent, loading the exe from disk again.
    fn restart(&self);
    fn startup(&self) -> Option<bool>;
    fn set_startup(&self, on: bool) -> Result<(), String>;
    fn save_settings(&self, s: &PcSettings) -> Result<(), String>;
    fn displays(&self) -> Value;
    fn set_display(&self, device: &str, display: u32) -> Result<(), String>;
}

pub struct Request<'a> {
    pub method: &'a str,
    pub url: &'a str,
    pub host: Option<&'a str>,
    pub origin: Option<&'a str>,
    pub requested_with: Option<&'a str>,
    pub body: &'a [u8],
}

#[derive(Debug)]
pub struct Response {
    pub status: u16,
    pub content_type: &'static str,
    pub body: Vec<u8>,
    /// Run once the response has been sent.
    pub after: Option<fn(&dyn Host)>,
}

fn json_response(status: u16, v: Value) -> Response {
    Response { status, content_type: "application/json", body: v.to_string().into_bytes(), after: None }
}

fn ok(v: Value) -> Response {
    json_response(200, v)
}

fn error(status: u16, msg: &str) -> Response {
    json_response(status, json!({ "error": msg }))
}

/// Names the browser may use for this server; anything else is DNS rebinding or a stray request.
fn local_host(host: &str, port: u16) -> bool {
    ["localhost", "127.0.0.1", "[::1]"].iter().any(|h| host.eq_ignore_ascii_case(&format!("{h}:{port}")))
}

fn parse_object(body: &[u8]) -> Result<serde_json::Map<String, Value>, Response> {
    match serde_json::from_slice::<Value>(body) {
        Ok(Value::Object(o)) => Ok(o),
        _ => Err(error(400, "body must be a JSON object")),
    }
}

/// Context the router reads besides the request.
pub struct Ctx<'a> {
    pub shared: &'a Shared,
    pub host: &'a dyn Host,
    /// Ports actually in use (the saved setting may differ until a restart).
    pub ws_port: u16,
    pub http_port: u16,
}

pub fn route(req: &Request, ctx: &Ctx) -> Response {
    if !req.host.is_some_and(|h| local_host(h, ctx.http_port)) {
        return error(403, "forbidden");
    }
    let path = req.url.split(['?', '#']).next().unwrap_or("/");
    if req.method == "POST" {
        // CSRF guard: a cross-origin page can't set a custom header without a CORS preflight,
        // which this server never grants; a foreign Origin is refused outright.
        let foreign_origin = req.origin.is_some_and(|o| !o.strip_prefix("http://").is_some_and(|h| local_host(h, ctx.http_port)));
        if req.requested_with.is_none_or(str::is_empty) || foreign_origin {
            return error(403, "forbidden");
        }
    }
    let shared = ctx.shared;
    match (req.method, path) {
        ("GET", "/api/settings") => {
            let s = shared.settings();
            let clients = shared.clients();
            ok(json!({
                "port": ctx.ws_port,
                "inactivity": s.inactivity,
                "timeoutInactivity": s.timeout_inactivity_ms,
                "videoInput": s.video_input,
                "timeoutVideoInput": s.timeout_video_input_ms,
                "connectedClients": clients.len(),
                "clientIp": clients.last().map(|c| c.ip.to_string()).unwrap_or_default(),
                "scrollExclude": shared.scroll_exclude(),
                "bindings": shared.bindings_doc(),
                "clients": clients.iter().map(|c| json!({
                    "ip": c.ip.to_string(),
                    "since": c.since.duration_since(UNIX_EPOCH).map(|d| d.as_millis() as u64).unwrap_or(0),
                    // Counts and times only, never which keys or text
                    "lastInputMs": c.stats.last_input.map(|t| t.elapsed().as_millis() as u64),
                    "latencyMs": c.stats.latency.map(|d| d.as_millis() as u64),
                    "pointerShown": c.stats.pointer_shown,
                    "liveSettings": c.stats.live_settings,
                    "pointer": c.stats.pointer,
                    "wheel": c.stats.wheel,
                    "keys": c.stats.keys,
                    "text": c.stats.text,
                })).collect::<Vec<_>>(),
                "remotePointer": {
                    "active": crate::win::cursor::status().0,
                    "problem": crate::win::cursor::status().1,
                },
                "version": env!("CARGO_PKG_VERSION"),
                "mode": ctx.host.mode(),
                "hasToken": !shared.token().is_empty(),
            }))
        }
        ("GET", "/api/defaults") => ok(bindings::to_json(&bindings::defaults())),
        ("GET", "/api/pcinfo") => ok(ctx.host.pc_info()),
        ("GET", "/api/config") => ok(settings::config_for_api(settings::read_json(&shared.files.tv_config()))),
        ("GET", "/api/pc") => {
            let s = shared.settings();
            ok(json!({
                "port": s.port,
                "inactivity": s.inactivity,
                "timeoutInactivity": s.timeout_inactivity_ms,
                "videoInput": s.video_input,
                "timeoutVideoInput": s.timeout_video_input_ms,
                "startup": ctx.host.startup(),
                "mode": ctx.host.mode(),
                "runningPort": ctx.ws_port,
            }))
        }
        ("GET", "/api/displays") => ok(ctx.host.displays()),
        ("GET", "/api/logs") => {
            ok(Value::Array(log::recent().into_iter().map(|e| json!({ "time": e.time, "level": e.level.as_str(), "message": e.message })).collect()))
        }
        ("POST", "/api/bindings") => {
            let doc = match parse_object(req.body) {
                Ok(o) => o,
                Err(r) => return r,
            };
            let mut doc = doc;
            if doc.get("bindings").is_some_and(|b| !b.is_object())
                || doc.get("scrollExclude").is_some_and(|b| !b.is_array())
                || doc.get("profiles").is_some_and(|b| !b.is_array())
            {
                return error(400, "bindings must be an object, scrollExclude and profiles arrays");
            }
            // A save that doesn't mention profiles (an older page, a script) keeps the saved ones
            if !doc.contains_key("profiles") {
                if let Some(saved) = shared.bindings_doc().get("profiles").cloned() {
                    doc.insert("profiles".into(), saved);
                }
            }
            if let Err(e) = settings::write_atomic(&shared.files.bindings(), &serde_json::to_vec_pretty(&doc).unwrap_or_default()) {
                return error(500, &format!("cannot save bindings.json: {e}"));
            }
            shared.reload_bindings();
            log::info("Key bindings saved from the web UI");
            ok(json!({ "saved": true, "applied": true }))
        }
        ("POST", "/api/config") => {
            let doc = match parse_object(req.body) {
                Ok(o) => o,
                Err(r) => return r,
            };
            let saved = settings::read_json(&shared.files.tv_config());
            let config = Value::Object(settings::merge_posted_config(doc, saved.as_ref()));
            if let Err(e) = settings::save_tv_config(&shared.files, &config) {
                return error(500, &format!("cannot save tv-config.json: {e}"));
            }
            shared.reload_tv_config();
            log::info("TV config saved from the web UI");
            ok(json!({ "saved": true }))
        }
        ("POST", "/api/token") => {
            // A new device token; TVs need the TV app reinstalled to carry it
            let token = match crate::win::acl::random_token() {
                Ok(t) => t,
                Err(e) => return error(500, &format!("cannot generate a token: {e}")),
            };
            let mut config = match settings::read_json(&shared.files.tv_config()) {
                Some(Value::Object(m)) => m,
                _ => serde_json::Map::new(),
            };
            config.insert("token".into(), Value::String(token));
            if let Err(e) = settings::save_tv_config(&shared.files, &Value::Object(config)) {
                return error(500, &format!("cannot save tv-config.json: {e}"));
            }
            shared.reload_tv_config();
            log::info("New device token generated from the web UI");
            ok(json!({ "saved": true, "hasToken": true }))
        }
        ("POST", "/api/pc") => {
            let doc = match parse_object(req.body) {
                Ok(o) => o,
                Err(r) => return r,
            };
            let old = shared.settings();
            let num = |k: &str, d: u32| doc.get(k).and_then(Value::as_u64).map_or(d, |v| v.min(u32::MAX as u64) as u32);
            let flag = |k: &str, d: bool| doc.get(k).and_then(Value::as_bool).unwrap_or(d);
            let new = PcSettings {
                port: u16::try_from(num("port", old.port as u32)).unwrap_or(0),
                inactivity: flag("inactivity", old.inactivity),
                timeout_inactivity_ms: num("timeoutInactivity", old.timeout_inactivity_ms),
                video_input: flag("videoInput", old.video_input),
                timeout_video_input_ms: num("timeoutVideoInput", old.timeout_video_input_ms),
            };
            if let Err(e) = new.validate() {
                return error(400, &e);
            }
            if let Err(e) = ctx.host.save_settings(&new) {
                return error(500, &e);
            }
            if let Some(on) = doc.get("startup").and_then(Value::as_bool) {
                if let Err(e) = ctx.host.set_startup(on) {
                    return error(500, &e);
                }
            }
            let restart_required = new.port != ctx.ws_port;
            shared.set_settings(new);
            log::info("PC settings saved from the web UI");
            ok(json!({ "saved": true, "restartRequired": restart_required }))
        }
        ("POST", "/api/display") => {
            let doc = match parse_object(req.body) {
                Ok(o) => o,
                Err(r) => return r,
            };
            let device = doc.get("device").and_then(Value::as_str).unwrap_or_default();
            let Some(display) = doc.get("display").and_then(Value::as_u64).and_then(|d| u32::try_from(d).ok()) else {
                return error(400, "display must be a display id");
            };
            match ctx.host.set_display(device, display) {
                Ok(()) => ok(json!({ "saved": true })),
                Err(e) => error(400, &e),
            }
        }
        ("POST", "/api/reinstall") => {
            let (success, message) = match ctx.host.reinstall_tv() {
                Ok(m) => {
                    log::info(format!("TV app reinstalled from the web UI: {m}"));
                    (true, m)
                }
                Err(m) => {
                    log::warn(format!("TV app reinstall from the web UI failed: {m}"));
                    (false, m)
                }
            };
            ok(json!({ "success": success, "message": message }))
        }
        ("POST", "/api/restart") => {
            log::info("Restart requested from the web UI");
            Response { after: Some(|h| h.restart()), ..ok(json!({ "restarting": true })) }
        }
        (_, p) if p.starts_with("/api/") => error(404, "not found"),
        ("GET", _) => Response { status: 200, content_type: "text/html; charset=utf-8", body: SETTINGS_HTML.as_bytes().to_vec(), after: None },
        _ => error(405, "method not allowed"),
    }
}

/// Loopback listeners: IPv4, plus IPv6 when available. Windows tries `localhost` as ::1 first,
/// and a refused connection there costs about two seconds before it falls back to IPv4.
pub fn bind(port: u16) -> std::io::Result<Vec<tiny_http::Server>> {
    let mut servers = vec![tiny_http::Server::http((Ipv4Addr::LOCALHOST, port)).map_err(std::io::Error::other)?];
    if let Ok(v6) = tiny_http::Server::http((Ipv6Addr::LOCALHOST, port)) {
        servers.push(v6);
    }
    Ok(servers)
}

fn header<'a>(req: &'a tiny_http::Request, name: &str) -> Option<&'a str> {
    req.headers().iter().find(|h| h.field.as_str().as_str().eq_ignore_ascii_case(name)).map(|h| h.value.as_str())
}

/// Serves until `server.unblock()`; each request gets its own thread, so a TV reinstall
/// (minutes) doesn't stall the status polling.
pub fn serve(server: Arc<tiny_http::Server>, shared: Arc<Shared>, host: Arc<dyn Host>, ws_port: u16) {
    let http_port = server.server_addr().to_ip().map_or(0, |a| a.port());
    for mut req in server.incoming_requests() {
        let (shared, host) = (shared.clone(), host.clone());
        let spawned = std::thread::Builder::new().name("http".into()).spawn(move || {
            let mut body = Vec::new();
            let too_big = req.body_length().is_some_and(|n| n > MAX_BODY)
                || req.as_reader().take(MAX_BODY as u64 + 1).read_to_end(&mut body).is_err()
                || body.len() > MAX_BODY;
            let method = req.method().as_str().to_string();
            let url = req.url().to_string();
            let resp = if too_big {
                error(413, "body too large")
            } else {
                let r = Request {
                    method: &method,
                    url: &url,
                    host: header(&req, "Host"),
                    origin: header(&req, "Origin"),
                    requested_with: header(&req, "X-Requested-With"),
                    body: &body,
                };
                route(&r, &Ctx { shared: &shared, host: &*host, ws_port, http_port })
            };
            let after = resp.after;
            let mut out = tiny_http::Response::from_data(resp.body).with_status_code(resp.status);
            for (k, v) in
                [("Content-Type", resp.content_type), ("Cache-Control", "no-store"), ("X-Content-Type-Options", "nosniff"), ("X-Frame-Options", "DENY")]
            {
                out.add_header(tiny_http::Header::from_bytes(k, v).expect("static header"));
            }
            let _ = req.respond(out);
            if let Some(f) = after {
                f(&*host);
            }
        });
        if let Err(e) = spawned {
            log::error(format!("Cannot start an HTTP thread: {e}"));
        }
    }
}
