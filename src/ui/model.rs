//! What the settings window shows and saves, worked out from the API's JSON. Kept apart from the
//! window so it can be tested.

use crate::bindings::{format_code, parse_code, BUTTONS};
use serde_json::{json, Map, Value};

const MOD_KEYS: [(u8, &str); 4] = [(0x11, "ctrl"), (0x12, "alt"), (0x10, "shift"), (0x5B, "win")];

/// Keys a binding can press, as the editor lists them: (virtual-key code, name).
pub fn keys() -> Vec<(u8, String)> {
    let mut k: Vec<(u8, String)> = [
        (13, "Enter"),
        (27, "Escape"),
        (8, "Backspace"),
        (9, "Tab"),
        (32, "Space"),
        (46, "Delete"),
        (38, "Arrow Up"),
        (40, "Arrow Down"),
        (37, "Arrow Left"),
        (39, "Arrow Right"),
        (36, "Home"),
        (35, "End"),
        (33, "Page Up"),
        (34, "Page Down"),
    ]
    .iter()
    .map(|(v, n)| (*v, n.to_string()))
    .collect();
    k.extend((1..=12).map(|i| (111 + i, format!("F{i}"))));
    k.extend((b'A'..=b'Z').map(|c| (c, (c as char).to_string())));
    k.extend((0..=9).map(|i| (48 + i, i.to_string())));
    k.extend((0..=9).map(|i| (96 + i, format!("Num {i}"))));
    k.extend(
        [
            (91, "Windows"),
            (173, "Mute"),
            (174, "Volume Down"),
            (175, "Volume Up"),
            (176, "Next Track"),
            (177, "Previous Track"),
            (178, "Stop Media"),
            (179, "Play/Pause"),
            (250, "Play"),
            (19, "Pause"),
            (44, "Print Screen"),
        ]
        .iter()
        .map(|(v, n)| (*v, n.to_string())),
    );
    k
}

/// A short name for a key in a binding's description, such as `Ctrl+C`.
pub fn key_name(vk: u8) -> String {
    match vk {
        0x10 => "Shift".into(),
        0x11 => "Ctrl".into(),
        0x12 => "Alt".into(),
        0x5B => "Win".into(),
        0x60..=0x69 => format!("Num{}", vk - 0x60),
        _ => keys().into_iter().find(|(v, _)| *v == vk).map_or_else(|| format!("VK{vk}"), |(_, n)| n.replace("Arrow ", "")),
    }
}

pub fn button_name(code: &str) -> String {
    parse_code(code).and_then(|c| BUTTONS.iter().find(|(b, _)| *b == c)).map_or_else(|| code.to_string(), |(_, n)| n.to_string())
}

/// What the editor shows for a binding: one of five kinds and its fields.
#[derive(Debug, Clone, PartialEq, Default)]
pub struct Form {
    /// 0 mouse click, 1 key, 2 shortcut, 3 command, 4 TV action
    pub kind: i32,
    /// 0 left, 1 right, 2 middle
    pub mouse: i32,
    /// Index into [`keys`]
    pub key: i32,
    /// Ctrl, Alt, Shift, Win
    pub mods: [bool; 4],
    pub command: String,
    /// 0 shut down the PC, 1 toggle the TV keyboard
    pub action: i32,
}

fn key_index(vk: u8) -> i32 {
    keys().iter().position(|(v, _)| *v == vk).map_or(0, |i| i as i32)
}

fn vk_of(e: &Value) -> Option<u8> {
    (e.get("type")?.as_str()? == "keyboard").then(|| e.get("virtualKey")?.as_u64()).flatten().map(|v| v as u8)
}

/// The editor form for a bind list. The service stores a shortcut either as one key with a `mods`
/// object (what the editors save) or as one keyboard entry per key (`[Ctrl, C]`, the defaults).
pub fn form_of(binds: &[Value]) -> Form {
    let mut f = Form::default();
    let Some(first) = binds.first() else { return f };
    // [Ctrl, Alt, C]: every entry but the last is a modifier key
    if binds.len() >= 2 {
        if let Some(vk) = binds.last().and_then(vk_of) {
            let mods: Option<Vec<usize>> = binds[..binds.len() - 1].iter().map(|e| vk_of(e).and_then(|v| MOD_KEYS.iter().position(|(m, _)| *m == v))).collect();
            if let Some(mods) = mods {
                f.kind = 2;
                f.key = key_index(vk);
                mods.into_iter().for_each(|m| f.mods[m] = true);
                return f;
            }
        }
    }
    let text = |k: &str| first.get(k).and_then(Value::as_str).unwrap_or_default();
    match text("type") {
        "mouse" => {
            f.kind = 0;
            f.mouse = match text("value") {
                "right" => 1,
                "middle" => 2,
                _ => 0,
            };
        }
        "action" => {
            f.kind = 4;
            f.action = i32::from(text("value") != "shutdown");
        }
        "command" => {
            f.kind = 3;
            f.command = text("command").to_string();
        }
        _ => {
            f.key = key_index(vk_of(first).unwrap_or(13));
            let mods = first.get("mods");
            for (i, (_, name)) in MOD_KEYS.iter().enumerate() {
                f.mods[i] = mods.and_then(|m| m.get(*name)) == Some(&Value::Bool(true));
            }
            f.kind = if f.mods.iter().any(|m| *m) { 2 } else { 1 };
        }
    }
    f
}

/// The bind entry the editor saves for a form (the same shape the web UI saves).
pub fn entry_of(f: &Form) -> Result<Value, String> {
    let vk = || keys().get(f.key.max(0) as usize).map_or(13, |(v, _)| *v);
    Ok(match f.kind {
        0 => {
            let button = ["left", "right", "middle"][f.mouse.clamp(0, 2) as usize];
            json!({ "type": "mouse", "value": button })
        }
        1 => json!({ "type": "keyboard", "virtualKey": vk(), "scanCode": 0, "extended": false }),
        2 => {
            if !f.mods.iter().any(|m| *m) {
                return Err("Pick at least one of Ctrl, Alt, Shift or Win for a shortcut.".into());
            }
            let mods: Map<String, Value> = MOD_KEYS.iter().zip(f.mods).map(|((_, n), on)| (n.to_string(), Value::Bool(on))).collect();
            json!({ "type": "keyboard", "virtualKey": vk(), "scanCode": 0, "extended": false, "mods": mods })
        }
        3 => {
            let command = f.command.trim();
            if command.is_empty() {
                return Err("Enter the command to run.".into());
            }
            if command.contains(['&', '|', ';', '<', '>']) {
                return Err("A command can't contain & | ; < or >.".into());
            }
            json!({ "type": "command", "command": command })
        }
        _ => json!({ "type": "action", "value": if f.action == 0 { "shutdown" } else { "keyboard" } }),
    })
}

/// A bind list in words, such as `Ctrl+C` or `Run: notepad.exe`.
pub fn describe(binds: &[Value]) -> String {
    if binds.is_empty() {
        return "Does nothing".into();
    }
    let f = form_of(binds);
    match f.kind {
        0 => ["Left click", "Right click", "Middle click"][f.mouse as usize].into(),
        3 => format!("Run: {}", f.command),
        4 => ["Shut down the PC", "Toggle the TV keyboard"][f.action as usize].into(),
        _ => {
            let key = keys().get(f.key as usize).map_or(13, |(v, _)| *v);
            let mut parts: Vec<String> = ["Ctrl", "Alt", "Shift", "Win"].iter().zip(f.mods).filter(|(_, on)| *on).map(|(n, _)| n.to_string()).collect();
            parts.push(key_name(key));
            parts.join("+")
        }
    }
}

/// Where a button's binding comes from.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Source {
    Default,
    Custom,
    Profile,
}

#[derive(Debug, Clone, PartialEq, Default)]
pub struct Profile {
    pub name: String,
    pub apps: Vec<String>,
    pub bindings: Map<String, Value>,
}

/// The bindings as `bindings.json` holds them, being edited.
#[derive(Debug, Clone, PartialEq, Default)]
pub struct Bindings {
    pub defaults: Map<String, Value>,
    pub custom: Map<String, Value>,
    pub profiles: Vec<Profile>,
    pub scroll_exclude: Vec<String>,
}

/// Key codes as `0x01CD`, whatever case the file used.
fn normalized(map: Option<&Value>) -> Map<String, Value> {
    map.and_then(Value::as_object).into_iter().flatten().filter_map(|(k, v)| Some((format_code(parse_code(k)?), v.clone()))).collect()
}

impl Bindings {
    /// From `GET /api/settings` and `GET /api/defaults`.
    pub fn from_api(settings: &Value, defaults: &Value) -> Bindings {
        let doc = &settings["bindings"];
        let profiles = doc
            .get("profiles")
            .and_then(Value::as_array)
            .into_iter()
            .flatten()
            .map(|p| Profile {
                name: p.get("name").and_then(Value::as_str).unwrap_or("Profile").to_string(),
                apps: p.get("apps").and_then(Value::as_array).into_iter().flatten().filter_map(Value::as_str).map(str::to_string).collect(),
                bindings: normalized(p.get("bindings")),
            })
            .collect();
        let scroll_exclude =
            settings.get("scrollExclude").and_then(Value::as_array).into_iter().flatten().filter_map(Value::as_str).map(str::to_string).collect();
        Bindings { defaults: normalized(Some(defaults)), custom: normalized(doc.get("bindings")), profiles, scroll_exclude }
    }

    fn list(v: Option<&Value>) -> Vec<Value> {
        v.and_then(Value::as_array).cloned().unwrap_or_default()
    }

    /// What `code` does with `profile` selected (`None`: all apps), and where that comes from.
    pub fn effective(&self, profile: Option<usize>, code: &str) -> (Vec<Value>, Source) {
        if let Some(v) = profile.and_then(|p| self.profiles.get(p)).and_then(|p| p.bindings.get(code)) {
            return (Self::list(Some(v)), Source::Profile);
        }
        match self.custom.get(code) {
            Some(v) => (Self::list(Some(v)), Source::Custom),
            None => (Self::list(self.defaults.get(code)), Source::Default),
        }
    }

    /// The map being edited: the selected profile's, or the normal one.
    fn edited(&mut self, profile: Option<usize>) -> &mut Map<String, Value> {
        match profile.and_then(|p| self.profiles.get_mut(p)) {
            Some(p) => &mut p.bindings,
            None => &mut self.custom,
        }
    }

    /// Whether the selected profile (or, for all apps, the normal bindings) changes `code`.
    pub fn changed(&self, profile: Option<usize>, code: &str) -> bool {
        match profile.and_then(|p| self.profiles.get(p)) {
            Some(p) => p.bindings.contains_key(code),
            None => self.custom.contains_key(code),
        }
    }

    pub fn set(&mut self, profile: Option<usize>, code: &str, entry: Value) {
        self.edited(profile).insert(code.to_string(), json!([entry]));
    }

    /// Back to the default (all apps) or to the normal binding (a profile).
    pub fn reset(&mut self, profile: Option<usize>, code: &str) {
        self.edited(profile).remove(code);
    }

    /// The body of `POST /api/bindings`.
    pub fn to_doc(&self) -> Value {
        json!({
            "bindings": self.custom,
            "scrollExclude": self.scroll_exclude,
            "profiles": self.profiles.iter().map(|p| json!({ "name": p.name, "apps": p.apps, "bindings": p.bindings })).collect::<Vec<_>>(),
        })
    }
}

/// Process names from what someone typed: separated by commas, semicolons or spaces, `.exe` dropped.
pub fn parse_apps(text: &str) -> Vec<String> {
    text.split([',', ';', ' ', '\t'])
        .map(|a| a.trim())
        .map(|a| if a.to_ascii_lowercase().ends_with(".exe") { &a[..a.len() - 4] } else { a })
        .filter(|a| !a.is_empty())
        .map(str::to_string)
        .collect()
}

/// A button drawn on the remote, in logical pixels from the remote's top left corner.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct KeySpot {
    /// `None` for buttons the TV keeps for itself.
    pub code: Option<u16>,
    pub glyph: &'static str,
    /// The glyph is from Windows' icon font.
    pub icon: bool,
    pub x: f32,
    pub y: f32,
    pub w: f32,
    pub h: f32,
    pub round: bool,
    /// Fill colour as 0xAARRGGBB, 0 for the usual key colour.
    pub fill: u32,
    /// A caption, not a button.
    pub plain: bool,
}

/// `at` is `[x, y, width, height]`.
const fn k(code: u16, glyph: &'static str, icon: bool, at: [f32; 4], round: bool) -> KeySpot {
    KeySpot { code: Some(code), glyph, icon, x: at[0], y: at[1], w: at[2], h: at[3], round, fill: 0, plain: false }
}
const fn tv(glyph: &'static str, icon: bool, at: [f32; 4], round: bool) -> KeySpot {
    KeySpot { code: None, glyph, icon, x: at[0], y: at[1], w: at[2], h: at[3], round, fill: 0, plain: false }
}
const fn caption(text: &'static str, x: f32, y: f32, w: f32) -> KeySpot {
    KeySpot { code: None, glyph: text, icon: false, x, y, w, h: 14.0, round: false, fill: 0, plain: true }
}
const fn color(code: u16, fill: u32, x: f32) -> KeySpot {
    KeySpot { code: Some(code), glyph: "", icon: false, x, y: 600.0, w: 40.0, h: 16.0, round: false, fill, plain: false }
}

/// The remote, drawn in a 220 x 690 body with the pointer and keyboard gestures below it, in the
/// 220 x 744 `RemoteView` of `ui/remote.slint`.
pub const REMOTE: &[KeySpot] = &[
    tv("\u{E7E8}", true, [20.0, 22.0, 34.0, 34.0], true),
    k(0x0031, "1", false, [26.0, 76.0, 48.0, 34.0], false),
    k(0x0032, "2", false, [86.0, 76.0, 48.0, 34.0], false),
    k(0x0033, "3", false, [146.0, 76.0, 48.0, 34.0], false),
    k(0x0034, "4", false, [26.0, 118.0, 48.0, 34.0], false),
    k(0x0035, "5", false, [86.0, 118.0, 48.0, 34.0], false),
    k(0x0036, "6", false, [146.0, 118.0, 48.0, 34.0], false),
    k(0x0037, "7", false, [26.0, 160.0, 48.0, 34.0], false),
    k(0x0038, "8", false, [86.0, 160.0, 48.0, 34.0], false),
    k(0x0039, "9", false, [146.0, 160.0, 48.0, 34.0], false),
    k(0x0030, "0", false, [86.0, 202.0, 48.0, 34.0], false),
    caption("VOL", 26.0, 252.0, 48.0),
    tv("+", false, [26.0, 270.0, 48.0, 36.0], false),
    tv("\u{2212}", false, [26.0, 310.0, 48.0, 36.0], false),
    tv("\u{E74F}", true, [88.0, 286.0, 44.0, 44.0], true),
    caption("CH", 146.0, 252.0, 48.0),
    k(0x0021, "\u{E70E}", true, [146.0, 270.0, 48.0, 36.0], false),
    k(0x0022, "\u{E70D}", true, [146.0, 310.0, 48.0, 36.0], false),
    k(0x0026, "\u{E70E}", true, [88.0, 383.0, 44.0, 34.0], false),
    k(0x0028, "\u{E70D}", true, [88.0, 495.0, 44.0, 34.0], false),
    k(0x0025, "\u{E76B}", true, [37.0, 434.0, 34.0, 44.0], false),
    k(0x0027, "\u{E76C}", true, [149.0, 434.0, 34.0, 44.0], false),
    k(0x000D, "OK", false, [80.0, 426.0, 60.0, 60.0], true),
    k(0x01CD, "\u{E72B}", true, [30.0, 548.0, 40.0, 40.0], true),
    tv("\u{E80F}", true, [90.0, 548.0, 40.0, 40.0], true),
    tv("\u{E713}", true, [150.0, 548.0, 40.0, 40.0], true),
    color(0x0193, 0xFFE5484D, 18.0),
    color(0x0194, 0xFF30A46C, 66.0),
    color(0x0195, 0xFFF5C518, 114.0),
    color(0x0196, 0xFF3E63DD, 162.0),
    k(0x019C, "\u{E892}", true, [13.0, 636.0, 34.0, 34.0], true),
    k(0x019F, "\u{E768}", true, [53.0, 636.0, 34.0, 34.0], true),
    k(0x0013, "\u{E769}", true, [93.0, 636.0, 34.0, 34.0], true),
    k(0x019D, "\u{E71A}", true, [133.0, 636.0, 34.0, 34.0], true),
    k(0x01A1, "\u{E893}", true, [173.0, 636.0, 34.0, 34.0], true),
    k(0x0001, "Click", false, [0.0, 710.0, 64.0, 32.0], false),
    k(0x0002, "Long click", false, [72.0, 710.0, 84.0, 32.0], false),
    k(0x0008, "\u{E750}", true, [164.0, 710.0, 56.0, 32.0], false),
];

/// Whether version `a` (such as "3.2.0") is newer than `b`.
pub fn newer(a: &str, b: &str) -> bool {
    let parse = |v: &str| v.split('.').map(|n| n.trim().parse::<u32>().unwrap_or(0)).collect::<Vec<_>>();
    parse(a) > parse(b)
}

/// The network the TV is on: the first whose subnet holds `tv`. `networks` are (ip, mask, mac).
pub fn network_for(tv: std::net::Ipv4Addr, networks: &[(String, String, String)]) -> Option<usize> {
    let tv = u32::from(tv);
    networks.iter().position(|(ip, mask, _)| {
        let (Ok(ip), Ok(mask)) = (ip.parse::<std::net::Ipv4Addr>(), mask.parse::<std::net::Ipv4Addr>()) else { return false };
        let (ip, mask) = (u32::from(ip), u32::from(mask));
        mask != u32::MAX && ip & mask == tv & mask
    })
}

/// "12 s", "4 min", "2 h 5 min".
pub fn ago(ms: u64) -> String {
    let s = ms / 1000;
    match s {
        0..60 => format!("{s} s"),
        60..3600 => format!("{} min", s / 60),
        _ => format!("{} h {} min", s / 3600, (s % 3600) / 60),
    }
}

/// The TV app settings the window edits; the rest of `tv-config.json` is kept as saved.
#[derive(Debug, Clone, PartialEq)]
pub struct TvApp {
    /// 1 to 4
    pub hdmi: u32,
    pub ip: String,
    pub mask: String,
    pub mac: String,
    pub port: u16,
    pub cursor_speed: f64,
    pub scroll_speed: f64,
    pub long_click: u32,
}

/// The `POST /api/config` body for `form` over the saved config, and whether the TV app needs
/// reinstalling (the speeds and long press reach running TV apps by themselves).
pub fn config_post(saved: &Value, form: &TvApp) -> (Value, bool) {
    let mut cfg: Map<String, Value> =
        json!({ "overlay": true, "inputDirect": true, "longClick": 1500, "extend": true }).as_object().cloned().unwrap_or_default();
    if let Some(saved) = saved.as_object() {
        cfg.extend(saved.clone());
    }
    cfg.remove("hasToken");
    let h = form.hdmi.clamp(1, 4);
    let set = json!({
        "inputId": format!("HDMI_{h}"),
        "inputAppId": format!("com.webos.app.hdmi{h}"),
        "inputName": format!("HDMI {h}"),
        "inputSource": format!("ext://hdmi:hdmi{h}"),
        "appId": format!("com.cathwyler.magicremoteservice.hdmi{h}"),
        "ip": form.ip,
        "mask": form.mask,
        "mac": form.mac,
        "port": form.port,
        "cursorSpeed": (form.cursor_speed * 10.0).round() / 10.0,
        "scrollSpeed": (form.scroll_speed * 10.0).round() / 10.0,
        "longClick": form.long_click,
    });
    let reinstall = ["inputId", "ip", "port", "mask", "mac", "appId"].iter().any(|k| saved.get(*k) != set.get(*k));
    cfg.extend(set.as_object().cloned().unwrap_or_default());
    (Value::Object(cfg), reinstall)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn key(vk: u8) -> Value {
        json!({ "type": "keyboard", "virtualKey": vk, "scanCode": 0, "extended": false })
    }

    #[test]
    fn shortcuts_are_read_in_both_stored_forms() {
        // The defaults store Ctrl+C as two keys; the editors save one key with mods
        let split = [key(0x11), key(b'C')];
        // Saved with every modifier listed, as the web UI saves it
        let joined = [json!({ "type": "keyboard", "virtualKey": 67, "scanCode": 0, "extended": false,
            "mods": { "ctrl": true, "alt": false, "shift": false, "win": false } })];
        for binds in [&split[..], &joined[..]] {
            let f = form_of(binds);
            assert_eq!((f.kind, f.mods, keys()[f.key as usize].0), (2, [true, false, false, false], b'C'));
            assert_eq!(describe(binds), "Ctrl+C");
        }
        assert_eq!(entry_of(&form_of(&split)).unwrap(), joined[0]);
    }

    #[test]
    fn forms_round_trip_every_kind() {
        for binds in [
            json!([{ "type": "mouse", "value": "middle" }]),
            json!([{ "type": "keyboard", "virtualKey": 27, "scanCode": 0, "extended": false }]),
            json!([{ "type": "command", "command": "notepad.exe" }]),
            json!([{ "type": "action", "value": "keyboard" }]),
        ] {
            let binds = binds.as_array().unwrap();
            assert_eq!(&entry_of(&form_of(binds)).unwrap(), &binds[0]);
        }
        assert_eq!(describe(&[json!({ "type": "mouse", "value": "right" })]), "Right click");
        assert_eq!(describe(&[key(0x25)]), "Left");
        assert_eq!(describe(&[key(0x60)]), "Num0");
        assert_eq!(describe(&[json!({ "type": "action", "value": "shutdown" })]), "Shut down the PC");
        assert_eq!(describe(&[]), "Does nothing");
    }

    #[test]
    fn bad_forms_are_refused() {
        assert!(entry_of(&Form { kind: 2, ..Form::default() }).is_err());
        assert!(entry_of(&Form { kind: 3, command: "  ".into(), ..Form::default() }).is_err());
        assert!(entry_of(&Form { kind: 3, command: "calc & del x".into(), ..Form::default() }).is_err());
    }

    #[test]
    fn profiles_override_then_fall_back() {
        let settings = json!({
            "bindings": {
                "bindings": { "0x0193": [{ "type": "command", "command": "calc.exe" }] },
                "profiles": [{ "name": "Media", "apps": ["vlc"], "bindings": { "0x0194": [] } }]
            },
            "scrollExclude": ["PotPlayerMini64"]
        });
        let defaults = json!({ "0x0193": [{ "type": "action", "value": "shutdown" }], "0x0194": [key(0x5B)] });
        let mut b = Bindings::from_api(&settings, &defaults);
        assert_eq!(b.effective(None, "0x0193").1, Source::Custom);
        assert_eq!(b.effective(Some(0), "0x0193").1, Source::Custom);
        assert_eq!(b.effective(Some(0), "0x0194"), (vec![], Source::Profile));
        assert_eq!(b.effective(None, "0x0194").1, Source::Default);
        assert!(b.changed(Some(0), "0x0194") && !b.changed(None, "0x0194"));

        b.set(Some(0), "0x0193", key(b'P'));
        b.reset(None, "0x0193");
        let doc = b.to_doc();
        assert_eq!(doc["bindings"], json!({}));
        assert_eq!(doc["profiles"][0]["bindings"]["0x0193"][0]["virtualKey"], 80);
        assert_eq!(doc["scrollExclude"], json!(["PotPlayerMini64"]));
    }

    #[test]
    fn apps_are_parsed_like_the_web_ui() {
        assert_eq!(parse_apps(" vlc.EXE, PotPlayerMini64;mpc-hc64  "), ["vlc", "PotPlayerMini64", "mpc-hc64"]);
        assert!(parse_apps(" , ; ").is_empty());
    }

    #[test]
    fn the_remote_covers_every_button_once() {
        for (code, _) in BUTTONS {
            assert_eq!(REMOTE.iter().filter(|k| k.code == Some(*code)).count(), 1, "{code:#06x}");
        }
        // Inside the 220 x 744 RemoteView
        assert!(REMOTE.iter().all(|k| k.x >= 0.0 && k.x + k.w <= 220.0 && k.y + k.h <= 744.0));
    }

    #[test]
    fn config_posts_keep_the_rest_and_say_when_to_reinstall() {
        let saved = json!({ "ip": "192.168.1.10", "port": 41230, "mask": "255.255.255.0", "mac": "aa", "inputId": "HDMI_1",
            "appId": "com.cathwyler.magicremoteservice.hdmi1", "cursorSpeed": 1.0, "overlay": false, "hasToken": true });
        let form = TvApp {
            hdmi: 1,
            ip: "192.168.1.10".into(),
            mask: "255.255.255.0".into(),
            mac: "aa".into(),
            port: 41230,
            cursor_speed: 2.04,
            scroll_speed: 1.0,
            long_click: 900,
        };
        let (post, reinstall) = config_post(&saved, &form);
        assert!(!reinstall);
        assert_eq!((post["overlay"].clone(), post["cursorSpeed"].clone(), post["longClick"].clone()), (json!(false), json!(2.0), json!(900)));
        assert!(post.get("hasToken").is_none());
        assert!(config_post(&saved, &TvApp { hdmi: 2, ..form.clone() }).1);
        assert!(config_post(&saved, &TvApp { port: 41240, ..form }).1);
    }

    #[test]
    fn versions_compare_by_number() {
        assert!(newer("3.2.0", "3.1.0") && newer("3.10.0", "3.9.9") && newer("4.0", "3.99.1"));
        assert!(!newer("3.1.0", "3.1.0") && !newer("3.0.9", "3.1.0") && !newer("", "3.1.0"));
    }

    #[test]
    fn the_tv_s_network_is_the_one_on_its_subnet() {
        let n = |ip: &str, mask: &str| (ip.to_string(), mask.to_string(), String::new());
        // As on a PC with a VPN and Hyper-V switches listed before the Wi-Fi
        let networks = [n("100.90.180.85", "255.255.255.255"), n("172.20.192.1", "255.255.240.0"), n("192.168.8.150", "255.255.255.0")];
        assert_eq!(network_for("192.168.8.145".parse().unwrap(), &networks), Some(2));
        assert_eq!(network_for("10.1.2.3".parse().unwrap(), &networks), None);
    }

    #[test]
    fn times_read_naturally() {
        assert_eq!((ago(4_000), ago(240_000), ago(7_500_000)), ("4 s".to_string(), "4 min".to_string(), "2 h 5 min".to_string()));
    }
}
