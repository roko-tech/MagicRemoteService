//! PC settings in the registry (same keys and view as the C# build) and the JSON files beside the exe.

use crate::bindings::{Action, Bind, BindMap, KeyStroke, MouseButton};
use crate::win::registry::{Key, Root, CURRENT_USER, LOCAL_MACHINE};
use serde_json::{Map, Value};
use std::path::{Path, PathBuf};

pub const REG_PATH: &str = "Software\\MagicRemoteService";
pub const DEFAULT_PORT: u16 = 41230;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PcSettings {
    /// WebSocket port for the TV; the web UI listens on the next one.
    pub port: u16,
    pub inactivity: bool,
    pub timeout_inactivity_ms: u32,
    pub video_input: bool,
    pub timeout_video_input_ms: u32,
}

impl Default for PcSettings {
    fn default() -> Self {
        PcSettings { port: DEFAULT_PORT, inactivity: true, timeout_inactivity_ms: 7_200_000, video_input: true, timeout_video_input_ms: 900_000 }
    }
}

impl PcSettings {
    pub fn validate(&self) -> Result<(), String> {
        if self.port < 1024 || self.port == u16::MAX {
            return Err("Port must be between 1024 and 65534.".into());
        }
        for (name, ms) in [("Inactivity", self.timeout_inactivity_ms), ("Video input", self.timeout_video_input_ms)] {
            if !(60_000..=86_400_000).contains(&ms) {
                return Err(format!("{name} timeout must be between 1 minute and 24 hours."));
            }
        }
        Ok(())
    }
}

/// Where settings live: machine-wide when elevated (the service), per user otherwise.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Scope {
    Machine,
    User,
}

impl Scope {
    pub fn current() -> Scope {
        if crate::win::proc::is_system() || crate::win::proc::is_elevated() {
            Scope::Machine
        } else {
            Scope::User
        }
    }

    fn root(self) -> Root {
        match self {
            Scope::Machine => LOCAL_MACHINE,
            Scope::User => CURRENT_USER,
        }
    }

    fn open(self, sub: &str) -> Option<Key> {
        let path = if sub.is_empty() { REG_PATH.to_string() } else { format!("{REG_PATH}\\{sub}") };
        Key::open(self.root(), &path, false).ok()
    }

    pub fn load(self) -> PcSettings {
        let d = PcSettings::default();
        let Some(k) = self.open("") else { return d };
        PcSettings {
            port: k.get_u32("Port").and_then(|p| u16::try_from(p).ok()).unwrap_or(d.port),
            inactivity: k.get_u32("Inactivity").map_or(d.inactivity, |v| v != 0),
            timeout_inactivity_ms: k.get_u32("TimeoutInactivity").unwrap_or(d.timeout_inactivity_ms),
            video_input: k.get_u32("VideoInput").map_or(d.video_input, |v| v != 0),
            timeout_video_input_ms: k.get_u32("TimeoutVideoInput").unwrap_or(d.timeout_video_input_ms),
        }
    }

    pub fn save(self, s: &PcSettings) -> std::io::Result<()> {
        let k = Key::create(self.root(), REG_PATH)?;
        k.set_u32("Port", s.port as u32)?;
        k.set_u32("Inactivity", s.inactivity as u32)?;
        k.set_u32("TimeoutInactivity", s.timeout_inactivity_ms)?;
        k.set_u32("VideoInput", s.video_input as u32)?;
        k.set_u32("TimeoutVideoInput", s.timeout_video_input_ms)
    }

    /// The display (target id) each ares TV device maps its absolute pointer to.
    pub fn displays(self) -> Vec<(String, u32)> {
        let Some(k) = self.open("Device") else { return Vec::new() };
        k.subkeys().into_iter().filter_map(|name| Some((name.clone(), k.open_sub(&name, false).ok()?.get_u32("Display")?))).collect()
    }

    pub fn set_display(self, device: &str, display: u32) -> std::io::Result<()> {
        if device.is_empty() || device.contains(['\\', '/']) {
            return Err(std::io::Error::other("invalid device name"));
        }
        Key::create(self.root(), &format!("{REG_PATH}\\Device\\{device}"))?.set_u32("Display", display)
    }

    /// Bindings an old version's settings window saved in the registry, if any.
    pub fn legacy_bindings(self) -> Option<BindMap> {
        let root = self.open("Remote\\Bind")?;
        let mut map = BindMap::new();
        for name in root.subkeys() {
            let Some(code) = crate::bindings::parse_code(&name) else { continue };
            let Ok(key) = root.open_sub(&name, false) else { continue };
            let mut entries: Vec<(u32, Bind)> = Vec::new();
            for idx in key.subkeys() {
                let Ok(b) = key.open_sub(&idx, false) else { continue };
                let v = |n: &str| b.get_u32(n).unwrap_or(0);
                let bind = match b.get_u32("Kind") {
                    Some(0) => Bind::Mouse(match v("Value") {
                        1 => MouseButton::Left,
                        2 => MouseButton::Right,
                        _ => MouseButton::Middle,
                    }),
                    Some(1) => Bind::Key(KeyStroke { vk: v("VirtualKey") as u8, scan: v("ScanCode") as u8, extended: v("Extended") == 1 }),
                    Some(2) => Bind::Action(if v("Value") == 1 { Action::Shutdown } else { Action::Keyboard }),
                    Some(3) => Bind::Command(b.get_string("Command").unwrap_or_default()),
                    _ => continue,
                };
                entries.push((idx.parse().unwrap_or(u32::MAX), bind));
            }
            entries.sort_by_key(|(i, _)| *i);
            map.insert(code, entries.into_iter().map(|(_, b)| b).collect());
        }
        (!map.is_empty()).then_some(map)
    }
}

/// The files kept beside the exe.
#[derive(Debug, Clone)]
pub struct Files {
    pub dir: PathBuf,
}

impl Files {
    pub fn bindings(&self) -> PathBuf {
        self.dir.join("bindings.json")
    }
    pub fn tv_config(&self) -> PathBuf {
        self.dir.join("tv-config.json")
    }
}

pub fn read_json(path: &Path) -> Option<Value> {
    serde_json::from_slice(&std::fs::read(path).ok()?).ok()
}

/// Writes through a temporary file, so a crash never leaves half a file behind.
pub fn write_atomic(path: &Path, data: &[u8]) -> std::io::Result<()> {
    let tmp = path.with_extension("json.tmp");
    std::fs::write(&tmp, data)?;
    std::fs::rename(&tmp, path)
}

/// Default apps whose scroll wheel steps one notch at a time (they map it to volume).
pub const DEFAULT_SCROLL_EXCLUDE: [&str; 4] = ["PotPlayerMini64", "PotPlayerMini", "PotPlayer", "PotPlayer64"];

pub fn scroll_exclude(bindings_doc: Option<&Value>) -> Vec<String> {
    match bindings_doc.and_then(|d| d.get("scrollExclude")).and_then(Value::as_array) {
        Some(list) => list.iter().filter_map(Value::as_str).filter(|s| !s.is_empty()).map(str::to_string).collect(),
        None => DEFAULT_SCROLL_EXCLUDE.iter().map(|s| s.to_string()).collect(),
    }
}

/// The device token from `tv-config.json`; empty means open mode.
pub fn token(tv_config: Option<&Value>) -> String {
    tv_config.and_then(|c| c.get("token")).and_then(Value::as_str).unwrap_or_default().to_string()
}

/// Settings a TV app applies while running: pointer speed, long-press time and scroll speed. Out
/// of range values are clamped rather than refused (the installer posts back whatever is saved),
/// and never 0, which the TV app would ignore.
pub fn live_tv_settings(tv_config: Option<&Value>) -> Value {
    let num = |k: &str, default: f64, lo: f64, hi: f64| tv_config.and_then(|c| c.get(k)).and_then(Value::as_f64).unwrap_or(default).clamp(lo, hi);
    serde_json::json!({
        "cursorSpeed": num("cursorSpeed", 1.0, 0.1, 5.0),
        "longClick": num("longClick", 1500.0, 300.0, 5000.0).round() as u64,
        "scrollSpeed": num("scrollSpeed", 1.0, 0.2, 5.0),
    })
}

/// The TV config as the API shows it: everything but the token, plus whether one is set.
/// The token stays inside the service: only the TV app it deploys needs it.
pub fn config_for_api(saved: Option<Value>) -> Value {
    let mut map = match saved {
        Some(Value::Object(m)) => m,
        _ => Map::new(),
    };
    let has_token = map.remove("token").and_then(|t| t.as_str().map(|s| !s.is_empty())).unwrap_or(false);
    map.insert("hasToken".into(), Value::Bool(has_token));
    Value::Object(map)
}

/// A config posted through the API, ready to save: keeps the saved token unless the post sets
/// `token` itself (an empty string turns token auth off).
pub fn merge_posted_config(mut posted: Map<String, Value>, saved: Option<&Value>) -> Map<String, Value> {
    posted.remove("hasToken");
    if !posted.contains_key("token") {
        if let Some(t) = saved.and_then(|s| s.get("token")).filter(|t| t.is_string()) {
            posted.insert("token".into(), t.clone());
        }
    }
    posted
}

/// Saves `tv-config.json`. As the service, only SYSTEM and administrators may read it, since it
/// holds the device token.
pub fn save_tv_config(files: &Files, config: &Value) -> std::io::Result<()> {
    let path = files.tv_config();
    let tmp = path.with_extension("json.tmp");
    std::fs::write(&tmp, serde_json::to_vec_pretty(config).unwrap_or_default())?;
    if crate::win::proc::is_system() {
        crate::win::acl::restrict_to_admins(&tmp)?;
    }
    std::fs::rename(&tmp, &path)
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn scroll_exclusions_default_to_potplayer() {
        assert_eq!(scroll_exclude(None).len(), 4);
        assert_eq!(scroll_exclude(Some(&json!({"bindings": {}}))).len(), 4);
        assert_eq!(scroll_exclude(Some(&json!({"scrollExclude": ["vlc", "", 3]}))), vec!["vlc"]);
        assert!(scroll_exclude(Some(&json!({"scrollExclude": []}))).is_empty());
    }

    #[test]
    fn the_api_never_shows_the_token_and_saves_keep_it() {
        let saved = json!({"appId": "x", "token": "s3cret"});
        let shown = config_for_api(Some(saved.clone()));
        assert_eq!(shown, json!({"appId": "x", "hasToken": true}));
        assert_eq!(config_for_api(Some(json!({"token": ""})))["hasToken"], false);
        assert_eq!(config_for_api(None), json!({"hasToken": false}));

        // The web UI posts back what it was shown: the token survives
        let Value::Object(posted) = json!({"appId": "y", "hasToken": true}) else { panic!() };
        assert_eq!(Value::Object(merge_posted_config(posted, Some(&saved))), json!({"appId": "y", "token": "s3cret"}));
        // An explicit token replaces it, and an empty one turns auth off
        let Value::Object(posted) = json!({"token": ""}) else { panic!() };
        assert_eq!(Value::Object(merge_posted_config(posted, Some(&saved))), json!({"token": ""}));
        let Value::Object(posted) = json!({"appId": "z"}) else { panic!() };
        assert_eq!(Value::Object(merge_posted_config(posted, None)), json!({"appId": "z"}));
    }

    #[test]
    fn live_tv_settings_have_defaults_and_are_clamped() {
        assert_eq!(live_tv_settings(None), json!({"cursorSpeed": 1.0, "longClick": 1500, "scrollSpeed": 1.0}));
        let c = json!({"cursorSpeed": 0, "longClick": 99999, "scrollSpeed": 2.5, "token": "t"});
        assert_eq!(live_tv_settings(Some(&c)), json!({"cursorSpeed": 0.1, "longClick": 5000, "scrollSpeed": 2.5}));
        assert_eq!(live_tv_settings(Some(&json!({"cursorSpeed": "fast"})))["cursorSpeed"], 1.0);
    }

    #[test]
    fn token_is_optional() {
        assert_eq!(token(None), "");
        assert_eq!(token(Some(&json!({"token": "abc"}))), "abc");
        assert_eq!(token(Some(&json!({"token": 5}))), "");
    }

    #[test]
    fn settings_validation() {
        assert!(PcSettings::default().validate().is_ok());
        assert!(PcSettings { port: 80, ..Default::default() }.validate().is_err());
        assert!(PcSettings { timeout_video_input_ms: 1000, ..Default::default() }.validate().is_err());
    }

    #[test]
    fn atomic_write_replaces_existing_files() {
        let p = std::env::temp_dir().join(format!("mrs-test-{}.json", std::process::id()));
        write_atomic(&p, b"1").unwrap();
        write_atomic(&p, b"2").unwrap();
        assert_eq!(std::fs::read(&p).unwrap(), b"2");
        std::fs::remove_file(p).unwrap();
    }
}
