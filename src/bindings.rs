//! What each remote button does, as stored in `bindings.json` and shown by the web UI.

use serde_json::{json, Map, Value};
use std::collections::{BTreeMap, HashMap};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MouseButton {
    Left,
    Right,
    Middle,
}

/// Actions the TV app performs itself; the value is the byte the TV expects.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Action {
    Shutdown = 0x01,
    Keyboard = 0x02,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct KeyStroke {
    pub vk: u8,
    pub scan: u8,
    pub extended: bool,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Bind {
    Mouse(MouseButton),
    Key(KeyStroke),
    Action(Action),
    Command(String),
}

/// Remote key code -> what it does, in press order.
pub type BindMap = BTreeMap<u16, Vec<Bind>>;

const fn key(vk: u8, scan: u8, extended: bool) -> Bind {
    Bind::Key(KeyStroke { vk, scan, extended })
}

const CTRL: Bind = key(0x11, 0x1D, false);
const ALT: Bind = key(0x12, 0x38, false);
const SHIFT: Bind = key(0x10, 0x2A, false);
const WIN: Bind = key(0x5B, 0x5B, true);

/// The built-in bindings, used for any button `bindings.json` leaves out.
pub fn defaults() -> BindMap {
    let mut m = BindMap::new();
    m.insert(0x0001, vec![Bind::Mouse(MouseButton::Left)]); // OK click
    m.insert(0x0002, vec![Bind::Mouse(MouseButton::Right)]); // OK long press
    m.insert(0x0008, vec![key(0x08, 0x0E, false)]); // Backspace
    m.insert(0x000D, vec![key(0x0D, 0x1C, false)]); // OK -> Enter
    m.insert(0x0021, vec![CTRL, key(0x43, 0x2E, false)]); // CH+ -> Ctrl+C
    m.insert(0x0022, vec![CTRL, key(0x56, 0x2F, false)]); // CH- -> Ctrl+V
    m.insert(0x0025, vec![key(0x25, 0x4B, true)]);
    m.insert(0x0026, vec![key(0x26, 0x48, true)]);
    m.insert(0x0027, vec![key(0x27, 0x4D, true)]);
    m.insert(0x0028, vec![key(0x28, 0x50, true)]);
    // Digits -> numpad keys
    let numpad_scan = [0x52, 0x4F, 0x50, 0x51, 0x4B, 0x4C, 0x4D, 0x47, 0x48, 0x49];
    for (i, scan) in numpad_scan.into_iter().enumerate() {
        m.insert(0x0030 + i as u16, vec![key(0x60 + i as u8, scan, false)]);
    }
    m.insert(0x0193, vec![Bind::Action(Action::Shutdown)]); // Red
    m.insert(0x0194, vec![WIN]); // Green
    m.insert(0x0195, vec![Bind::Mouse(MouseButton::Right)]); // Yellow
    m.insert(0x0196, vec![Bind::Action(Action::Keyboard)]); // Blue
    m.insert(0x01CD, vec![key(0x1B, 0x01, false)]); // Back -> Escape
    m.insert(0x019F, vec![key(0xB3, 0x00, false)]); // Play -> media Play/Pause
    m.insert(0x0013, vec![key(0x13, 0x00, false)]); // Pause -> Pause key
    m.insert(0x01A1, vec![key(0xB0, 0x00, false)]); // Fast-forward -> next track
    m.insert(0x019C, vec![key(0xB1, 0x00, false)]); // Rewind -> previous track
    m.insert(0x019D, vec![key(0xB2, 0x00, false)]); // Stop -> media Stop
    m
}

/// Parses a `"0x01CD"` style key (the `0x` is optional).
pub fn parse_code(name: &str) -> Option<u16> {
    u16::from_str_radix(name.trim_start_matches("0x").trim_start_matches("0X"), 16).ok()
}

pub fn format_code(code: u16) -> String {
    format!("0x{code:04X}")
}

/// Parses one bind entry. The web UI stores a shortcut as one key plus a `mods` object;
/// each modifier becomes its own key so the chord really fires.
fn parse_entry(e: &Value, out: &mut Vec<Bind>) -> Result<(), String> {
    let s = |k: &str| e.get(k).and_then(Value::as_str).ok_or(format!("missing \"{k}\""));
    let n = |k: &str| e.get(k).and_then(Value::as_i64).ok_or(format!("missing \"{k}\"")).map(|v| v as u8);
    match s("type")? {
        "mouse" => out.push(Bind::Mouse(match s("value")? {
            "left" => MouseButton::Left,
            "right" => MouseButton::Right,
            _ => MouseButton::Middle,
        })),
        "keyboard" => {
            let stroke =
                KeyStroke { vk: n("virtualKey")?, scan: n("scanCode")?, extended: e.get("extended").and_then(Value::as_bool).ok_or("missing \"extended\"")? };
            if let Some(mods) = e.get("mods").and_then(Value::as_object) {
                for (name, bind) in [("ctrl", CTRL), ("alt", ALT), ("shift", SHIFT), ("win", WIN)] {
                    if mods.get(name) == Some(&Value::Bool(true)) {
                        out.push(bind);
                    }
                }
            }
            out.push(Bind::Key(stroke));
        }
        "action" => out.push(Bind::Action(if s("value")? == "shutdown" { Action::Shutdown } else { Action::Keyboard })),
        "command" => out.push(Bind::Command(s("command")?.to_string())),
        other => return Err(format!("unknown type \"{other}\"")),
    }
    Ok(())
}

/// The bindings in effect for a `bindings.json` document: defaults, then each button the file lists.
/// Entries that can't be read are skipped and reported, so one bad entry can't kill a button.
pub fn from_file(doc: &Value) -> (BindMap, Vec<String>) {
    let mut map = defaults();
    let mut problems = Vec::new();
    let Some(obj) = doc.get("bindings").and_then(Value::as_object) else {
        return (map, problems);
    };
    for (name, entries) in obj {
        let Some(code) = parse_code(name) else {
            problems.push(format!("{name}: not a key code"));
            continue;
        };
        let mut binds = Vec::new();
        for e in entries.as_array().map(Vec::as_slice).unwrap_or_default() {
            if let Err(why) = parse_entry(e, &mut binds) {
                problems.push(format!("{name}: {why}"));
            }
        }
        map.insert(code, binds);
    }
    (map, problems)
}

/// Buttons that act differently while one of some apps is in front.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Profile {
    pub name: String,
    /// Process names without `.exe`, such as `vlc` or `PotPlayerMini64`.
    pub apps: Vec<String>,
    /// Only the buttons this profile changes; an empty list makes a button do nothing.
    pub map: BindMap,
}

/// The `profiles` of a `bindings.json` document. Entries that can't be read are reported.
pub fn profiles_from_file(doc: &Value) -> (Vec<Profile>, Vec<String>) {
    let mut profiles = Vec::new();
    let mut problems = Vec::new();
    for (i, p) in doc.get("profiles").and_then(Value::as_array).map(Vec::as_slice).unwrap_or_default().iter().enumerate() {
        let name = p.get("name").and_then(Value::as_str).filter(|n| !n.trim().is_empty()).map_or_else(|| format!("Profile {}", i + 1), str::to_string);
        let apps: Vec<String> = p
            .get("apps")
            .and_then(Value::as_array)
            .map(Vec::as_slice)
            .unwrap_or_default()
            .iter()
            .filter_map(Value::as_str)
            .map(str::trim)
            .filter(|a| !a.is_empty())
            .map(str::to_string)
            .collect();
        let mut map = BindMap::new();
        for (key, entries) in p.get("bindings").and_then(Value::as_object).into_iter().flatten() {
            let Some(code) = parse_code(key) else {
                problems.push(format!("profile {name}: {key}: not a key code"));
                continue;
            };
            let mut binds = Vec::new();
            for e in entries.as_array().map(Vec::as_slice).unwrap_or_default() {
                if let Err(why) = parse_entry(e, &mut binds) {
                    problems.push(format!("profile {name}: {key}: {why}"));
                }
            }
            map.insert(code, binds);
        }
        if apps.is_empty() {
            problems.push(format!("profile {name}: lists no app, so it never applies"));
        }
        profiles.push(Profile { name, apps, map });
    }
    (profiles, problems)
}

/// The `/api/defaults` and `bindings.json` form of a bind list.
pub fn to_json(map: &BindMap) -> Value {
    let mut obj = Map::new();
    for (code, binds) in map {
        let list = binds
            .iter()
            .map(|b| match b {
                Bind::Mouse(m) => {
                    json!({"type": "mouse", "value": match m { MouseButton::Left => "left", MouseButton::Right => "right", MouseButton::Middle => "middle" }})
                }
                Bind::Key(k) => json!({"type": "keyboard", "virtualKey": k.vk, "scanCode": k.scan, "extended": k.extended}),
                Bind::Action(a) => json!({"type": "action", "value": match a { Action::Shutdown => "shutdown", Action::Keyboard => "keyboard" }}),
                Bind::Command(c) => json!({"type": "command", "command": c}),
            })
            .collect();
        obj.insert(format_code(*code), Value::Array(list));
    }
    Value::Object(obj)
}

/// An input to inject, independent of the Win32 `INPUT` layout.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum InputEvent {
    Button {
        button: MouseButton,
        down: bool,
    },
    Key {
        vk: u16,
        scan: u16,
        extended: bool,
        down: bool,
    },
    MoveRelative {
        dx: i32,
        dy: i32,
    },
    /// Coordinates normalized to 0..=65535 over the whole virtual desktop.
    MoveAbsolute {
        x: i32,
        y: i32,
    },
    Wheel {
        delta: i32,
    },
    Unicode {
        unit: u16,
        down: bool,
    },
}

/// What a key press does once compiled. Like the C# service, injected input wins over
/// TV actions, which win over commands, when one button mixes them.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Plan {
    /// Press in order on key down; `up` releases in reverse order (Ctrl+C: C up, then Ctrl up).
    Inputs {
        down: Vec<InputEvent>,
        up: Vec<InputEvent>,
    },
    Actions(Vec<Action>),
    Commands(Vec<String>),
}

pub fn plan(binds: &[Bind]) -> Option<Plan> {
    let mut down = Vec::new();
    let mut actions = Vec::new();
    let mut commands = Vec::new();
    for b in binds {
        match b {
            Bind::Mouse(button) => down.push(InputEvent::Button { button: *button, down: true }),
            Bind::Key(k) => down.push(InputEvent::Key { vk: k.vk as u16, scan: k.scan as u16, extended: k.extended, down: true }),
            Bind::Action(a) => actions.push(*a),
            Bind::Command(c) => commands.push(c.clone()),
        }
    }
    if !down.is_empty() {
        let up = down
            .iter()
            .rev()
            .map(|e| match *e {
                InputEvent::Button { button, .. } => InputEvent::Button { button, down: false },
                InputEvent::Key { vk, scan, extended, .. } => InputEvent::Key { vk, scan, extended, down: false },
                other => other,
            })
            .collect();
        Some(Plan::Inputs { down, up })
    } else if !actions.is_empty() {
        Some(Plan::Actions(actions))
    } else if !commands.is_empty() {
        Some(Plan::Commands(commands))
    } else {
        None
    }
}

/// A profile ready for the connection loop.
#[derive(Debug, Default)]
pub struct CompiledProfile {
    /// Lowercase process names.
    pub apps: Vec<String>,
    /// `None`: the profile makes this button do nothing.
    pub plans: HashMap<u16, Option<Plan>>,
}

/// Bindings ready for the connection loop.
#[derive(Debug, Default)]
pub struct Compiled {
    pub plans: HashMap<u16, Plan>,
    pub profiles: Vec<CompiledProfile>,
}

impl Compiled {
    pub fn new(map: &BindMap) -> Self {
        Compiled { plans: map.iter().filter_map(|(code, binds)| Some((*code, plan(binds)?))).collect(), profiles: Vec::new() }
    }

    pub fn with_profiles(map: &BindMap, profiles: &[Profile]) -> Self {
        let profiles = profiles
            .iter()
            .filter(|p| !p.apps.is_empty())
            .map(|p| CompiledProfile {
                apps: p.apps.iter().map(|a| a.to_ascii_lowercase()).collect(),
                plans: p.map.iter().map(|(code, binds)| (*code, plan(binds))).collect(),
            })
            .collect();
        Compiled { profiles, ..Compiled::new(map) }
    }

    pub fn has_profiles(&self) -> bool {
        !self.profiles.is_empty()
    }

    /// What `code` does with `app` in front: the first profile listing the app and changing the
    /// button decides, otherwise the normal bindings.
    pub fn plan_for(&self, code: u16, app: Option<&str>) -> Option<&Plan> {
        if let Some(app) = app {
            let app = app.to_ascii_lowercase();
            if let Some(p) = self.profiles.iter().find(|p| p.apps.contains(&app) && p.plans.contains_key(&code)) {
                return p.plans[&code].as_ref();
            }
        }
        self.plans.get(&code)
    }
}

/// Characters a command binding may not contain: they would chain a second command.
pub fn command_is_safe(command: &str) -> bool {
    !command.contains(['&', '|', ';', '>', '<'])
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn defaults_cover_all_thirty_remote_buttons() {
        let d = defaults();
        assert_eq!(d.len(), 30);
        assert_eq!(d[&0x0033], vec![key(0x63, 0x51, false)]);
        assert_eq!(d[&0x0021], vec![CTRL, key(0x43, 0x2E, false)]);
    }

    #[test]
    fn defaults_round_trip_through_json() {
        let json = to_json(&defaults());
        assert_eq!(json["0x01CD"], json!([{"type": "keyboard", "virtualKey": 27, "scanCode": 1, "extended": false}]));
        assert_eq!(json["0x0193"], json!([{"type": "action", "value": "shutdown"}]));
        let (map, problems) = from_file(&json!({ "bindings": json }));
        assert!(problems.is_empty());
        assert_eq!(map, defaults());
    }

    #[test]
    fn file_overrides_only_the_buttons_it_lists_and_expands_mods() {
        let doc = json!({"bindings": {
            "0x0002": [{"type": "mouse", "value": "middle"}],
            "0x0194": [{"type": "keyboard", "virtualKey": 68, "scanCode": 32, "extended": false, "mods": {"win": true, "ctrl": true, "alt": false}}],
            "0x0196": [{"type": "command", "command": "notepad.exe"}],
            "0x0030": []
        }});
        let (map, problems) = from_file(&doc);
        assert!(problems.is_empty());
        assert_eq!(map[&0x0002], vec![Bind::Mouse(MouseButton::Middle)]);
        assert_eq!(map[&0x0194], vec![CTRL, WIN, key(68, 32, false)]);
        assert_eq!(map[&0x0196], vec![Bind::Command("notepad.exe".into())]);
        assert!(map[&0x0030].is_empty(), "an empty list unbinds the button");
        assert_eq!(map[&0x0001], defaults()[&0x0001]);
    }

    #[test]
    fn bad_entries_are_reported_not_fatal() {
        let doc = json!({"bindings": {
            "zz": [],
            "0x0001": [{"type": "keyboard", "virtualKey": 13}, {"type": "mouse", "value": "right"}],
            "0x0002": [{"type": "teleport"}]
        }});
        let (map, problems) = from_file(&doc);
        assert_eq!(problems.len(), 3);
        assert_eq!(map[&0x0001], vec![Bind::Mouse(MouseButton::Right)]);
        assert!(map[&0x0002].is_empty());
        assert_eq!(from_file(&json!({})).0, defaults());
    }

    #[test]
    fn plans_release_keys_in_reverse_and_prefer_inputs() {
        let p = plan(&[CTRL, key(0x43, 0x2E, false)]).unwrap();
        let Plan::Inputs { down, up } = p else { panic!() };
        assert_eq!(down[0], InputEvent::Key { vk: 0x11, scan: 0x1D, extended: false, down: true });
        assert_eq!(up[0], InputEvent::Key { vk: 0x43, scan: 0x2E, extended: false, down: false });
        assert_eq!(up[1], InputEvent::Key { vk: 0x11, scan: 0x1D, extended: false, down: false });
        assert!(matches!(plan(&[Bind::Action(Action::Keyboard), Bind::Mouse(MouseButton::Left)]), Some(Plan::Inputs { .. })));
        assert_eq!(plan(&[Bind::Command("a".into()), Bind::Action(Action::Shutdown)]), Some(Plan::Actions(vec![Action::Shutdown])));
        assert_eq!(plan(&[]), None);
        assert_eq!(Compiled::new(&defaults()).plans.len(), 30);
    }

    #[test]
    fn profiles_override_only_their_buttons_for_their_apps() {
        let doc = json!({
            "bindings": {"0x0193": [{"type": "command", "command": "calc.exe"}]},
            "profiles": [
                {"name": "Media", "apps": ["VLC", "PotPlayerMini64"], "bindings": {
                    "0x0193": [{"type": "keyboard", "virtualKey": 32, "scanCode": 57, "extended": false}],
                    "0x0194": []
                }},
                {"name": "Nothing", "apps": [], "bindings": {"0x0001": []}},
                {"apps": ["vlc"], "bindings": {"0x0195": [{"type": "mouse", "value": "left"}], "zz": []}}
            ]
        });
        let (map, _) = from_file(&doc);
        let (profiles, problems) = profiles_from_file(&doc);
        assert_eq!(profiles.len(), 3);
        assert_eq!(profiles[2].name, "Profile 3");
        assert_eq!(problems.len(), 2, "{problems:?}"); // the app-less profile and the bad key
        let c = Compiled::with_profiles(&map, &profiles);
        assert!(c.has_profiles());
        let space = Some(&Plan::Inputs {
            down: vec![InputEvent::Key { vk: 32, scan: 57, extended: false, down: true }],
            up: vec![InputEvent::Key { vk: 32, scan: 57, extended: false, down: false }],
        });
        // Red: space in VLC (any case), the normal command elsewhere or with no app known
        assert_eq!(c.plan_for(0x0193, Some("VLC")), space);
        assert_eq!(c.plan_for(0x0193, Some("notepad")), Some(&Plan::Commands(vec!["calc.exe".into()])));
        assert_eq!(c.plan_for(0x0193, None), Some(&Plan::Commands(vec!["calc.exe".into()])));
        // Green does nothing in VLC; buttons a profile leaves alone keep their binding
        assert_eq!(c.plan_for(0x0194, Some("VLC")), None);
        assert!(c.plan_for(0x0001, Some("vlc")).is_some(), "the app-less profile never applies");
        // A later profile for the same app still contributes buttons the first leaves alone
        assert!(matches!(c.plan_for(0x0195, Some("vlc")), Some(Plan::Inputs { .. })));
        assert!(!Compiled::new(&map).has_profiles());
    }

    #[test]
    fn unsafe_commands_are_detected() {
        assert!(command_is_safe("C:\\Tools\\app.exe --flag"));
        for bad in ["a & b", "a | b", "a; b", "a > f", "a < f", "a && b"] {
            assert!(!command_is_safe(bad), "{bad}");
        }
    }
}
