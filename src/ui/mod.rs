//! The native settings window: its own unprivileged process (`MagicRemoteService.exe ui`, started
//! as the signed-in user) that talks to the service through the local API the web UI also uses.

mod model;
mod setup;

slint::include_modules!();

use crate::bindings::format_code;
use crate::client;
use model::{Bindings, Form, Source, TvApp};
use serde_json::{json, Value};
use slint::{ComponentHandle, ModelRc, SharedString, Timer, TimerMode, VecModel};
use std::cell::RefCell;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::Duration;

const SHORT: Duration = Duration::from_secs(10);
/// Pairing and installing reach the TV over SSH and can take a while.
const LONG: Duration = Duration::from_secs(300);
const TITLE: &str = "MagicRemoteService Settings";

/// What the window is editing, kept on the UI thread.
#[derive(Default)]
struct State {
    port: u16,
    bindings: Bindings,
    /// `None`: all apps; else the index of the selected profile.
    profile: Option<usize>,
    /// `GET /api/config`, the base for saving the TV app settings.
    config: Value,
    /// The networks the combo box lists: (ip, mask, mac), and how it names them.
    networks: Vec<(String, String, String)>,
    network_labels: Vec<String>,
    /// Display ids in combo box order (0 = primary).
    displays: Vec<u32>,
    pending: Option<Confirm>,
    toast_serial: u64,
    /// Saves the profile's apps a moment after typing stops.
    apps_timer: Option<Timer>,
    setup: setup::Setup,
}

enum Confirm {
    Unpair(String),
    DeleteProfile,
    NewToken,
}

thread_local! {
    static STATE: RefCell<State> = RefCell::new(State::default());
    /// The timer that styles the window once it exists.
    static STYLING: RefCell<Option<Timer>> = const { RefCell::new(None) };
}

fn with<T>(f: impl FnOnce(&mut State) -> T) -> T {
    STATE.with(|s| f(&mut s.borrow_mut()))
}

fn port() -> u16 {
    with(|s| s.port)
}

/// Runs `work` on its own thread and `done` with its result back on the UI thread.
fn background<T: Send + 'static>(ui: &MainWindow, work: impl FnOnce() -> T + Send + 'static, done: impl FnOnce(MainWindow, T) + Send + 'static) {
    let weak = ui.as_weak();
    std::thread::spawn(move || {
        let result = work();
        let _ = weak.upgrade_in_event_loop(move |ui| done(ui, result));
    });
}

fn strings(list: impl IntoIterator<Item = String>) -> ModelRc<SharedString> {
    ModelRc::new(VecModel::from(list.into_iter().map(SharedString::from).collect::<Vec<_>>()))
}

fn toast(ui: &MainWindow, message: &str, error: bool) {
    let app = ui.global::<App>();
    app.set_toast(message.into());
    app.set_toast_error(error);
    app.set_toast_visible(true);
    let serial = with(|s| {
        s.toast_serial += 1;
        s.toast_serial
    });
    let weak = ui.as_weak();
    Timer::single_shot(Duration::from_millis(if error { 6000 } else { 3500 }), move || {
        if let Some(ui) = weak.upgrade() {
            if with(|s| s.toast_serial) == serial {
                ui.global::<App>().set_toast_visible(false);
            }
        }
    });
}

fn confirm(ui: &MainWindow, what: Confirm, title: &str, text: &str, button: &str) {
    with(|s| s.pending = Some(what));
    let app = ui.global::<App>();
    app.set_confirm_title(title.into());
    app.set_confirm_text(text.into());
    app.set_confirm_button(button.into());
    app.set_confirm_open(true);
}

/// Event times arrive as Unix milliseconds; shown as local `HH:MM:SS`.
fn local_time(ms: u64) -> String {
    use windows_sys::Win32::Foundation::{FILETIME, SYSTEMTIME};
    use windows_sys::Win32::Storage::FileSystem::FileTimeToLocalFileTime;
    use windows_sys::Win32::System::Time::FileTimeToSystemTime;
    let t = ms * 10_000 + 116_444_736_000_000_000;
    let utc = FILETIME { dwLowDateTime: t as u32, dwHighDateTime: (t >> 32) as u32 };
    let mut local = FILETIME { dwLowDateTime: 0, dwHighDateTime: 0 };
    let mut st: SYSTEMTIME = unsafe { std::mem::zeroed() };
    unsafe {
        FileTimeToLocalFileTime(&utc, &mut local);
        FileTimeToSystemTime(&local, &mut st);
    }
    format!("{:02}:{:02}:{:02}", st.wHour, st.wMinute, st.wSecond)
}

/// Everything the window shows, fetched together.
struct Snapshot {
    settings: Value,
    defaults: Value,
    config: Value,
    pcinfo: Value,
    pc: Value,
    tvs: Value,
    displays: Value,
    logs: Value,
}

fn fetch_all(port: u16) -> Result<Snapshot, String> {
    let get = |path: &str| client::call(port, "GET", path, None, SHORT);
    // An older service lacks some of these: show the rest
    let optional = |path: &str| get(path).unwrap_or(Value::Null);
    Ok(Snapshot {
        settings: get("/api/settings")?,
        defaults: get("/api/defaults")?,
        config: get("/api/config")?,
        pcinfo: get("/api/pcinfo")?,
        pc: get("/api/pc")?,
        tvs: optional("/api/tvs"),
        displays: optional("/api/displays"),
        logs: optional("/api/logs"),
    })
}

fn load_all(ui: &MainWindow) {
    let port = port();
    background(
        ui,
        move || fetch_all(port),
        |ui, result| match result {
            Ok(snap) => apply_snapshot(&ui, snap),
            Err(_) => {
                set_connected(&ui, false);
                setup::choose_first_page(&ui);
            }
        },
    );
}

fn set_connected(ui: &MainWindow, connected: bool) {
    let app = ui.global::<App>();
    app.set_connected(connected);
    if !connected {
        app.set_status_text("Service not answering".into());
        app.set_status_kind(2);
    }
    with(|s| s.setup.connected = connected);
    setup::refresh(ui);
}

fn apply_snapshot(ui: &MainWindow, snap: Snapshot) {
    let app = ui.global::<App>();
    apply_status(ui, &snap.settings, &snap.logs);
    app.set_loaded(true);

    // Remote buttons
    with(|s| {
        s.bindings = Bindings::from_api(&snap.settings, &snap.defaults);
        if s.profile.is_some_and(|p| p >= s.bindings.profiles.len()) {
            s.profile = None;
        }
    });
    app.set_key_names(strings(model::keys().into_iter().map(|(_, n)| n)));
    render_bindings(ui);
    show_profile_apps(ui);

    // TVs and the TV app
    render_tvs(ui, &snap.tvs);
    apply_config(ui, snap.config, &snap.pcinfo);

    // PC
    let pc = &snap.pc;
    let minutes = |k: &str| (pc[k].as_u64().unwrap_or(3_600_000) / 60_000).clamp(1, 1440) as i32;
    app.set_pc_port(pc["port"].as_u64().unwrap_or(41230) as i32);
    app.set_inactivity(pc["inactivity"].as_bool().unwrap_or(false));
    app.set_inactivity_min(minutes("timeoutInactivity"));
    app.set_monitor_off(pc["videoInput"].as_bool().unwrap_or(false));
    app.set_monitor_min(minutes("timeoutVideoInput"));
    app.set_startup(pc["startup"].as_bool().unwrap_or(false));
    app.set_startup_available(!pc["startup"].is_null());
    app.set_restart_needed(pc["port"] != pc["runningPort"]);
    app.set_mode_note(
        if pc["mode"] == "service" {
            "Running as the Windows service, so the remote works from sign-in and survives restarts."
        } else {
            "Running on its own, not as the service. Start with Windows adds it to your sign-in programs."
        }
        .into(),
    );
    render_displays(ui, &snap.displays);
    setup::refresh(ui);
    setup::choose_first_page(ui);
}

/// The status bar, the Overview counts and the event log, refreshed every few seconds.
fn apply_status(ui: &MainWindow, settings: &Value, logs: &Value) {
    let app = ui.global::<App>();
    app.set_connected(true);
    with(|s| {
        s.setup.connected = true;
        s.setup.version = settings["version"].as_str().unwrap_or_default().to_string();
        s.setup.mode = settings["mode"].as_str().unwrap_or_default().to_string();
    });
    app.set_version(settings["version"].as_str().unwrap_or_default().into());
    app.set_mode(if settings["mode"] == "service" { "Windows service" } else { "Standalone" }.into());
    app.set_has_token(settings["hasToken"].as_bool().unwrap_or(false));
    let now = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map_or(0, |d| d.as_millis() as u64);
    let clients: Vec<ClientRow> = settings["clients"]
        .as_array()
        .into_iter()
        .flatten()
        .map(|c| {
            let n = |k: &str| c[k].as_u64().unwrap_or(0);
            ClientRow {
                ip: c["ip"].as_str().unwrap_or("?").into(),
                connected: model::ago(now.saturating_sub(n("since"))).into(),
                last_input: c["lastInputMs"].as_u64().map_or_else(|| "None yet".into(), |ms| format!("{} ago", model::ago(ms))).into(),
                latency: c["latencyMs"].as_u64().map_or_else(|| "Measuring…".into(), |ms| format!("{ms} ms")).into(),
                pointer: if c["pointerShown"] == true { "Shown" } else { "Hidden" }.into(),
                counts: format!("{} · {} · {} · {}", n("pointer"), n("wheel"), n("keys"), n("text")).into(),
                live: c["liveSettings"] == true,
            }
        })
        .collect();
    let count = clients.len();
    app.set_client_count(count.to_string().into());
    app.set_status_text(
        match count {
            0 => "Waiting for a TV".to_string(),
            1 => "1 TV connected".to_string(),
            n => format!("{n} TVs connected"),
        }
        .into(),
    );
    app.set_status_kind(if count > 0 { 0 } else { 1 });
    app.set_clients(ModelRc::new(VecModel::from(clients)));
    let pointer = &settings["remotePointer"];
    let problem = pointer["problem"].as_str().unwrap_or_default();
    app.set_pointer_problem(!problem.is_empty());
    app.set_pointer_text(
        if !problem.is_empty() {
            problem.to_string()
        } else if pointer["active"] == true {
            "Shown".into()
        } else {
            "Usual pointer".into()
        }
        .into(),
    );
    let events: Vec<EventRow> = logs
        .as_array()
        .into_iter()
        .flatten()
        .rev()
        .take(60)
        .map(|e| EventRow {
            time: local_time(e["time"].as_u64().unwrap_or(0)).into(),
            message: e["message"].as_str().unwrap_or_default().into(),
            level: e["level"].as_str().unwrap_or("info").into(),
        })
        .collect();
    app.set_events(ModelRc::new(VecModel::from(events)));
}

static POLLING: AtomicBool = AtomicBool::new(false);

fn poll(ui: &MainWindow) {
    if POLLING.swap(true, Ordering::AcqRel) {
        return;
    }
    let port = port();
    background(
        ui,
        move || -> Result<(Value, Value), String> {
            Ok((client::call(port, "GET", "/api/settings", None, SHORT)?, client::call(port, "GET", "/api/logs", None, SHORT)?))
        },
        |ui, result| {
            POLLING.store(false, Ordering::Release);
            match result {
                Ok((settings, logs)) if ui.global::<App>().get_loaded() => apply_status(&ui, &settings, &logs),
                // The service came back (or was started after the window): load everything
                Ok(_) => load_all(&ui),
                Err(_) => set_connected(&ui, false),
            }
        },
    );
}

// ----- Remote buttons -----

fn render_bindings(ui: &MainWindow) {
    let app = ui.global::<App>();
    let (keys, names, index, exclude) = with(|s| {
        let keys: Vec<RemoteKey> = model::REMOTE
            .iter()
            .map(|k| {
                let code = k.code.map(format_code).unwrap_or_default();
                RemoteKey {
                    name: if code.is_empty() { "TV button, can't be changed".into() } else { model::button_name(&code).into() },
                    custom: !code.is_empty() && s.bindings.changed(s.profile, &code),
                    code: code.into(),
                    glyph: k.glyph.into(),
                    icon: k.icon,
                    x: k.x,
                    y: k.y,
                    w: k.w,
                    h: k.h,
                    round: k.round,
                    fill: slint::Color::from_argb_encoded(k.fill),
                    plain: k.plain,
                }
            })
            .collect();
        let names: Vec<String> = std::iter::once("All apps".to_string())
            .chain(s.bindings.profiles.iter().map(|p| {
                if p.apps.is_empty() {
                    format!("{} (no app yet)", p.name)
                } else {
                    format!("{} ({})", p.name, p.apps.join(", "))
                }
            }))
            .collect();
        (keys, names, s.profile.map_or(0, |p| p as i32 + 1), s.bindings.scroll_exclude.clone())
    });
    app.set_remote_keys(ModelRc::new(VecModel::from(keys)));
    app.set_profile_names(strings(names));
    app.set_profile_index(index);
    app.set_scroll_exclude(strings(exclude));
    let selected = app.get_selected_code().to_string();
    if !selected.is_empty() {
        show_key(ui, &selected, false);
    }
}

/// Puts the selected profile's apps in the apps box: when the profile changes, not after each
/// save (which would rewrite the box while someone types in it).
fn show_profile_apps(ui: &MainWindow) {
    let apps = with(|s| s.profile.and_then(|p| s.bindings.profiles.get(p)).map(|p| p.apps.join(", ")).unwrap_or_default());
    ui.global::<App>().set_profile_apps(apps.into());
}

/// Shows `code` in the editor; `reload_form` also puts its binding into the form.
fn show_key(ui: &MainWindow, code: &str, reload_form: bool) {
    let app = ui.global::<App>();
    let (binds, source, changed, in_profile) = with(|s| {
        let (binds, source) = s.bindings.effective(s.profile, code);
        (binds, source, s.bindings.changed(s.profile, code), s.profile.is_some())
    });
    app.set_selected_code(code.into());
    app.set_selected_name(model::button_name(code).into());
    app.set_selected_desc(model::describe(&binds).into());
    app.set_selected_source(
        match (source, in_profile) {
            (Source::Profile, _) => "This profile",
            (_, true) => "Normal binding",
            (Source::Custom, false) => "Custom",
            (Source::Default, false) => "Default",
        }
        .into(),
    );
    app.set_selected_changed(changed);
    if reload_form {
        let f = model::form_of(&binds);
        app.set_form_kind(f.kind);
        app.set_form_mouse(f.mouse);
        app.set_form_key(f.key);
        app.set_mod_ctrl(f.mods[0]);
        app.set_mod_alt(f.mods[1]);
        app.set_mod_shift(f.mods[2]);
        app.set_mod_win(f.mods[3]);
        app.set_form_command(f.command.into());
        app.set_form_action(f.action);
    }
}

fn form(app: &App) -> Form {
    Form {
        kind: app.get_form_kind(),
        mouse: app.get_form_mouse(),
        key: app.get_form_key(),
        mods: [app.get_mod_ctrl(), app.get_mod_alt(), app.get_mod_shift(), app.get_mod_win()],
        command: app.get_form_command().to_string(),
        action: app.get_form_action(),
    }
}

/// Saves `bindings.json`; on success shows `done` and redraws the remote.
fn save_bindings(ui: &MainWindow, done: String) {
    let doc = with(|s| s.bindings.to_doc());
    let port = port();
    ui.global::<App>().set_saving_binding(true);
    background(
        ui,
        move || client::call(port, "POST", "/api/bindings", Some(&doc), SHORT),
        move |ui, result| {
            ui.global::<App>().set_saving_binding(false);
            match result {
                Ok(_) => {
                    render_bindings(&ui);
                    if !done.is_empty() {
                        toast(&ui, &done, false);
                    }
                }
                Err(e) => toast(&ui, &format!("Couldn't save the bindings: {e}"), true),
            }
        },
    );
}

// ----- TVs and the TV app -----

fn render_tvs(ui: &MainWindow, tvs: &Value) {
    let rows: Vec<TvRow> = tvs["tvs"]
        .as_array()
        .or(tvs.as_array())
        .into_iter()
        .flatten()
        .map(|t| TvRow {
            name: t["name"].as_str().unwrap_or_default().into(),
            ip: t["ip"].as_str().unwrap_or_default().into(),
            is_default: t["default"] == true,
        })
        .collect();
    with(|s| s.setup.tvs = rows.iter().map(|r| (r.name.to_string(), r.ip.to_string(), r.is_default)).collect());
    ui.global::<App>().set_tvs(ModelRc::new(VecModel::from(rows)));
    setup::refresh(ui);
}

fn apply_config(ui: &MainWindow, config: Value, pcinfo: &Value) {
    let app = ui.global::<App>();
    let mut networks: Vec<(String, String, String)> = Vec::new();
    let mut labels: Vec<String> = Vec::new();
    let saved_ip = config["ip"].as_str().unwrap_or_default().to_string();
    for i in pcinfo["interfaces"].as_array().into_iter().flatten() {
        let ip = i["ip"].as_str().unwrap_or_default().to_string();
        labels.push(format!("{ip} · {}", i["name"].as_str().unwrap_or("network")));
        networks.push((ip, i["mask"].as_str().unwrap_or_default().into(), i["mac"].as_str().unwrap_or_default().into()));
    }
    if !saved_ip.is_empty() && !networks.iter().any(|n| n.0 == saved_ip) {
        labels.insert(0, format!("{saved_ip} · saved, not found now"));
        networks.insert(0, (saved_ip.clone(), config["mask"].as_str().unwrap_or_default().into(), config["mac"].as_str().unwrap_or_default().into()));
    }
    let network = networks.iter().position(|n| n.0 == saved_ip).unwrap_or(0);
    let labels_kept = labels.clone();
    app.set_networks(strings(labels));
    app.set_network_index(network as i32);
    let hdmi = config["inputId"].as_str().and_then(|s| s.strip_prefix("HDMI_")).and_then(|n| n.parse::<i32>().ok()).unwrap_or(1);
    app.set_hdmi_index((hdmi - 1).clamp(0, 3));
    app.set_tv_port(config["port"].as_u64().or(pcinfo["port"].as_u64()).unwrap_or(41230) as i32);
    let speed = |k: &str, d: f64| config[k].as_f64().filter(|v| *v > 0.0).unwrap_or(d) as f32;
    app.set_cursor_speed(speed("cursorSpeed", 1.0));
    app.set_scroll_speed(speed("scrollSpeed", 1.0));
    app.set_long_click(speed("longClick", 1500.0));
    app.set_has_token(config["hasToken"] == true);
    with(|s| {
        s.config = config;
        s.networks = networks;
        s.network_labels = labels_kept;
    });
}

fn tv_app_form(app: &App) -> Result<TvApp, String> {
    let (ip, mask, mac) = with(|s| s.networks.get(app.get_network_index().max(0) as usize).cloned()).ok_or("Choose the network the TV is on.")?;
    Ok(TvApp {
        hdmi: (app.get_hdmi_index() + 1) as u32,
        ip,
        mask,
        mac,
        port: u16::try_from(app.get_tv_port()).map_err(|_| "The port must be 1024-65534.")?,
        cursor_speed: f64::from(app.get_cursor_speed()),
        scroll_speed: f64::from(app.get_scroll_speed()),
        long_click: ((app.get_long_click() / 100.0).round() * 100.0) as u32,
    })
}

// ----- PC -----

fn render_displays(ui: &MainWindow, d: &Value) {
    let mut names = vec!["Primary display".to_string()];
    let mut ids = vec![0u32];
    for s in d["displays"].as_array().into_iter().flatten() {
        let size = if s["active"] == true {
            format!(" ({}×{}{})", s["width"], s["height"], if s["primary"] == true { ", primary" } else { "" })
        } else {
            " (off)".into()
        };
        names.push(format!("{}: {}{size}", s["number"], s["name"].as_str().unwrap_or("Display")));
        ids.push(s["id"].as_u64().unwrap_or(0) as u32);
    }
    let rows: Vec<DisplayRow> = d["devices"]
        .as_array()
        .into_iter()
        .flatten()
        .map(|t| DisplayRow {
            name: t["name"].as_str().unwrap_or_default().into(),
            ip: t["ip"].as_str().unwrap_or_default().into(),
            choice: t["display"].as_u64().and_then(|id| ids.iter().position(|i| *i as u64 == id)).unwrap_or(0) as i32,
        })
        .collect();
    let app = ui.global::<App>();
    app.set_display_names(strings(names));
    app.set_display_tvs(ModelRc::new(VecModel::from(rows)));
    with(|s| s.displays = ids);
}

fn reload(ui: &MainWindow, path: &'static str, apply: fn(&MainWindow, &Value)) {
    let port = port();
    background(
        ui,
        move || client::call(port, "GET", path, None, SHORT),
        move |ui, r| {
            if let Ok(v) = r {
                apply(&ui, &v);
            }
        },
    );
}

// ----- Wiring -----

fn wire(ui: &MainWindow) {
    setup::wire(ui);
    let app = ui.global::<App>();
    let w = ui.as_weak();

    // Remote buttons
    let weak = w.clone();
    app.on_pick_key(move |code| {
        if let Some(ui) = weak.upgrade() {
            show_key(&ui, &code, true);
        }
    });
    let weak = w.clone();
    app.on_save_binding(move || {
        let Some(ui) = weak.upgrade() else { return };
        let app = ui.global::<App>();
        let code = app.get_selected_code().to_string();
        match model::entry_of(&form(&app)) {
            Ok(entry) => {
                let desc = model::describe(std::slice::from_ref(&entry));
                with(|s| s.bindings.set(s.profile, &code, entry));
                save_bindings(&ui, format!("{} now does: {desc}", model::button_name(&code)));
            }
            Err(e) => toast(&ui, &e, true),
        }
    });
    let weak = w.clone();
    app.on_reset_binding(move || {
        let Some(ui) = weak.upgrade() else { return };
        let code = ui.global::<App>().get_selected_code().to_string();
        with(|s| s.bindings.reset(s.profile, &code));
        save_bindings(&ui, String::new());
        show_key(&ui, &code, true);
    });
    let weak = w.clone();
    app.on_choose_profile(move |index| {
        let Some(ui) = weak.upgrade() else { return };
        with(|s| s.profile = (index > 0).then(|| index as usize - 1));
        ui.global::<App>().set_new_profile_open(false);
        render_bindings(&ui);
        show_profile_apps(&ui);
        let code = ui.global::<App>().get_selected_code().to_string();
        if !code.is_empty() {
            show_key(&ui, &code, true);
        }
    });
    let weak = w.clone();
    app.on_create_profile(move |name, apps| {
        let Some(ui) = weak.upgrade() else { return };
        let name = name.trim().to_string();
        if name.is_empty() {
            return toast(&ui, "Give the profile a name.", true);
        }
        let apps = model::parse_apps(&apps);
        with(|s| {
            s.bindings.profiles.push(model::Profile { name: name.clone(), apps, bindings: Default::default() });
            s.profile = Some(s.bindings.profiles.len() - 1);
        });
        ui.global::<App>().set_new_profile_open(false);
        show_profile_apps(&ui);
        save_bindings(&ui, format!("Profile {name} created. Pick a button to change it there."));
    });
    let weak = w.clone();
    app.on_delete_profile(move || {
        let Some(ui) = weak.upgrade() else { return };
        let name = with(|s| s.profile.and_then(|p| s.bindings.profiles.get(p)).map(|p| p.name.clone()));
        if let Some(name) = name {
            confirm(&ui, Confirm::DeleteProfile, &format!("Delete {name}?"), "Its buttons go back to their normal bindings in those apps.", "Delete");
        }
    });
    let weak = w.clone();
    app.on_set_profile_apps(move |text| {
        let Some(ui) = weak.upgrade() else { return };
        with(|s| {
            if let Some(p) = s.profile.and_then(|p| s.bindings.profiles.get_mut(p)) {
                p.apps = model::parse_apps(&text);
            }
        });
        // Save once typing stops
        let weak = ui.as_weak();
        let timer = Timer::default();
        timer.start(TimerMode::SingleShot, Duration::from_millis(900), move || {
            if let Some(ui) = weak.upgrade() {
                save_bindings(&ui, String::new());
            }
        });
        with(|s| s.apps_timer = Some(timer));
    });
    let weak = w.clone();
    app.on_add_exclusion(move |text| {
        let Some(ui) = weak.upgrade() else { return };
        let Some(name) = model::parse_apps(&text).into_iter().next() else { return };
        let added = with(|s| {
            if s.bindings.scroll_exclude.iter().any(|e| e.eq_ignore_ascii_case(&name)) {
                return false;
            }
            s.bindings.scroll_exclude.push(name.clone());
            true
        });
        if added {
            save_bindings(&ui, format!("{name} now scrolls one step at a time."));
        }
    });
    let weak = w.clone();
    app.on_remove_exclusion(move |index| {
        let Some(ui) = weak.upgrade() else { return };
        with(|s| {
            if (index as usize) < s.bindings.scroll_exclude.len() {
                s.bindings.scroll_exclude.remove(index as usize);
            }
        });
        save_bindings(&ui, String::new());
    });

    // TVs
    let weak = w.clone();
    app.on_pair(move |ip, passphrase, name| {
        let Some(ui) = weak.upgrade() else { return };
        let ip = ip.trim().to_string();
        if ip.parse::<std::net::Ipv4Addr>().is_err() {
            return toast(&ui, "Enter the TV's IP address, such as 192.168.1.20.", true);
        }
        if passphrase.trim().is_empty() {
            return toast(&ui, "Enter the passphrase the Developer Mode app shows.", true);
        }
        let app = ui.global::<App>();
        app.set_pairing(true);
        app.set_pair_status("".into());
        let body = json!({ "ip": ip, "passphrase": passphrase.trim(), "name": name.trim() });
        let port = port();
        background(
            &ui,
            move || client::call(port, "POST", "/api/tvs/pair", Some(&body), LONG),
            |ui, result| {
                let app = ui.global::<App>();
                app.set_pairing(false);
                let (ok, message) = match result {
                    Ok(v) => (v["success"] == true, v["message"].as_str().unwrap_or_default().to_string()),
                    Err(e) => (false, e),
                };
                app.set_pair_status(message.clone().into());
                app.set_pair_failed(!ok);
                if ok {
                    with(|s| s.setup.pair_again = false);
                    reload(&ui, "/api/tvs", render_tvs);
                    reload(&ui, "/api/displays", render_displays);
                    toast(&ui, &message, false);
                }
            },
        );
    });
    let weak = w.clone();
    app.on_make_default(move |name| {
        let Some(ui) = weak.upgrade() else { return };
        let port = port();
        let body = json!({ "name": name.as_str() });
        background(
            &ui,
            move || client::call(port, "POST", "/api/tvs/default", Some(&body), SHORT),
            move |ui, r| match r {
                Ok(v) => {
                    render_tvs(&ui, &v);
                    toast(&ui, &format!("{name} is now the default TV."), false);
                }
                Err(e) => toast(&ui, &e, true),
            },
        );
    });
    let weak = w.clone();
    app.on_unpair(move |name| {
        if let Some(ui) = weak.upgrade() {
            confirm(
                &ui,
                Confirm::Unpair(name.to_string()),
                &format!("Unpair {name}?"),
                "This PC deletes its key for the TV. Pair it again to install on it.",
                "Unpair",
            );
        }
    });
    let weak = w.clone();
    app.on_save_tv_app(move || {
        let Some(ui) = weak.upgrade() else { return };
        let app = ui.global::<App>();
        let form = match tv_app_form(&app) {
            Ok(f) => f,
            Err(e) => return toast(&ui, &e, true),
        };
        let (body, reinstall) = with(|s| model::config_post(&s.config, &form));
        app.set_saving_config(true);
        let port = port();
        background(
            &ui,
            move || client::call(port, "POST", "/api/config", Some(&body), SHORT).map(|_| body),
            move |ui, r| {
                ui.global::<App>().set_saving_config(false);
                match r {
                    Ok(saved) => {
                        with(|s| s.config = saved);
                        toast(
                            &ui,
                            if reinstall {
                                "Saved. Reinstall the TV app to use the new HDMI input, network or port."
                            } else {
                                "Saved. Connected TVs with the latest app use it now."
                            },
                            false,
                        );
                    }
                    Err(e) => toast(&ui, &format!("Couldn't save: {e}"), true),
                }
            },
        );
    });
    let weak = w.clone();
    app.on_reinstall(move || {
        let Some(ui) = weak.upgrade() else { return };
        let app = ui.global::<App>();
        app.set_reinstalling(true);
        app.set_reinstall_failed(false);
        app.set_reinstall_status("Building the TV app and installing it on the default TV…".into());
        let port = port();
        background(
            &ui,
            move || client::call(port, "POST", "/api/reinstall", None, LONG),
            |ui, r| {
                let app = ui.global::<App>();
                app.set_reinstalling(false);
                let (ok, message) = match r {
                    Ok(v) => (v["success"] == true, v["message"].as_str().unwrap_or_default().to_string()),
                    Err(e) => (false, e),
                };
                app.set_reinstall_status(message.clone().into());
                app.set_reinstall_failed(!ok);
                toast(&ui, if ok { "TV app installed and started." } else { "The TV app wasn't installed." }, !ok);
            },
        );
    });
    let weak = w.clone();
    app.on_new_token(move || {
        if let Some(ui) = weak.upgrade() {
            confirm(
                &ui,
                Confirm::NewToken,
                "Generate a new device token?",
                "TVs that carry the old token stop connecting until you reinstall the TV app on them.",
                "Generate",
            );
        }
    });

    // PC
    let weak = w.clone();
    app.on_save_pc(move || {
        let Some(ui) = weak.upgrade() else { return };
        let app = ui.global::<App>();
        let port_setting = app.get_pc_port();
        if !(1024..=65534).contains(&port_setting) {
            return toast(&ui, "The port must be 1024-65534.", true);
        }
        let mut body = json!({
            "port": port_setting,
            "inactivity": app.get_inactivity(),
            "timeoutInactivity": app.get_inactivity_min().clamp(1, 1440) * 60_000,
            "videoInput": app.get_monitor_off(),
            "timeoutVideoInput": app.get_monitor_min().clamp(1, 1440) * 60_000,
        });
        if app.get_startup_available() {
            body["startup"] = json!(app.get_startup());
        }
        app.set_saving_pc(true);
        let port = port();
        background(
            &ui,
            move || client::call(port, "POST", "/api/pc", Some(&body), SHORT),
            |ui, r| {
                let app = ui.global::<App>();
                app.set_saving_pc(false);
                match r {
                    Ok(v) => {
                        let restart = v["restartRequired"] == true;
                        app.set_restart_needed(restart);
                        toast(&ui, if restart { "Saved. Restart to listen on the new port." } else { "Saved and applied." }, false);
                    }
                    Err(e) => toast(&ui, &format!("Couldn't save: {e}"), true),
                }
            },
        );
    });
    let weak = w.clone();
    app.on_restart_service(move || {
        let Some(ui) = weak.upgrade() else { return };
        let new_port = ui.global::<App>().get_pc_port() as u16 + 1;
        let port = port();
        background(
            &ui,
            move || client::call(port, "POST", "/api/restart", Some(&json!({})), SHORT),
            move |ui, r| match r {
                Ok(_) => {
                    toast(&ui, "Restarting…", false);
                    with(|s| s.port = new_port);
                    ui.global::<App>().set_api_address(format!("127.0.0.1:{new_port}").into());
                    ui.global::<App>().set_loaded(false);
                }
                Err(e) => toast(&ui, &e, true),
            },
        );
    });
    let weak = w.clone();
    app.on_set_display(move |name, index| {
        let Some(ui) = weak.upgrade() else { return };
        let Some(id) = with(|s| s.displays.get(index.max(0) as usize).copied()) else { return };
        let body = json!({ "device": name.as_str(), "display": id });
        let port = port();
        background(
            &ui,
            move || client::call(port, "POST", "/api/display", Some(&body), SHORT),
            move |ui, r| match r {
                Ok(_) => toast(&ui, &format!("{name}: saved. It applies when the TV next connects."), false),
                Err(e) => toast(&ui, &e, true),
            },
        );
    });

    // Window
    let weak = w.clone();
    app.on_confirm(move |accepted| {
        let Some(ui) = weak.upgrade() else { return };
        ui.global::<App>().set_confirm_open(false);
        let Some(what) = with(|s| s.pending.take()) else { return };
        if !accepted {
            return;
        }
        let port = port();
        match what {
            Confirm::Unpair(name) => {
                let body = json!({ "name": name });
                background(
                    &ui,
                    move || client::call(port, "POST", "/api/tvs/remove", Some(&body), SHORT),
                    move |ui, r| match r {
                        Ok(v) => {
                            render_tvs(&ui, &v);
                            reload(&ui, "/api/displays", render_displays);
                            toast(&ui, &format!("{name} unpaired."), false);
                        }
                        Err(e) => toast(&ui, &e, true),
                    },
                );
            }
            Confirm::DeleteProfile => {
                with(|s| {
                    if let Some(p) = s.profile.take() {
                        s.bindings.profiles.remove(p);
                    }
                });
                show_profile_apps(&ui);
                save_bindings(&ui, "Profile deleted.".into());
            }
            Confirm::NewToken => {
                background(
                    &ui,
                    move || client::call(port, "POST", "/api/token", Some(&json!({})), SHORT),
                    |ui, r| match r {
                        Ok(_) => {
                            ui.global::<App>().set_has_token(true);
                            toast(&ui, "New token saved. Reinstall the TV app so the TV carries it.", false);
                        }
                        Err(e) => toast(&ui, &e, true),
                    },
                );
            }
        }
    });
    app.on_open_web(move || {
        let _ = crate::win::proc::open_url(&format!("http://localhost:{}/", port()));
    });
    let weak = w;
    app.on_retry(move || {
        if let Some(ui) = weak.upgrade() {
            load_all(&ui);
        }
    });
}

/// Gives the window the app's icon and a title bar that matches the dark theme, and centres it
/// on its monitor, no bigger than the work area (at 250% scaling a 800 px tall window is most of
/// a 4K screen), once it exists.
fn style_window() -> bool {
    use crate::win::wide;
    use windows_sys::Win32::Foundation::RECT;
    use windows_sys::Win32::Graphics::Dwm::DwmSetWindowAttribute;
    use windows_sys::Win32::Graphics::Gdi::{GetMonitorInfoW, MonitorFromWindow, MONITORINFO, MONITOR_DEFAULTTONEAREST};
    use windows_sys::Win32::System::LibraryLoader::GetModuleHandleW;
    use windows_sys::Win32::UI::WindowsAndMessaging::{
        FindWindowW, GetWindowRect, LoadIconW, SendMessageW, SetForegroundWindow, SetWindowPos, ICON_BIG, ICON_SMALL, SWP_NOACTIVATE, SWP_NOZORDER, WM_SETICON,
    };
    unsafe {
        let hwnd = FindWindowW(std::ptr::null(), wide(TITLE).as_ptr());
        if hwnd.is_null() {
            return false;
        }
        let mut info: MONITORINFO = std::mem::zeroed();
        info.cbSize = std::mem::size_of::<MONITORINFO>() as u32;
        let mut r = RECT { left: 0, top: 0, right: 0, bottom: 0 };
        if GetMonitorInfoW(MonitorFromWindow(hwnd, MONITOR_DEFAULTTONEAREST), &mut info) != 0 && GetWindowRect(hwnd, &mut r) != 0 {
            let work = info.rcWork;
            let (ww, wh) = (work.right - work.left, work.bottom - work.top);
            let (w, h) = ((r.right - r.left).min(ww * 94 / 100), (r.bottom - r.top).min(wh * 92 / 100));
            SetWindowPos(hwnd, std::ptr::null_mut(), work.left + (ww - w) / 2, work.top + (wh - h) / 2, w, h, SWP_NOZORDER | SWP_NOACTIVATE);
        }
        let icon = LoadIconW(GetModuleHandleW(std::ptr::null()), 1 as _);
        if !icon.is_null() {
            SendMessageW(hwnd, WM_SETICON, ICON_BIG as usize, icon as isize);
            SendMessageW(hwnd, WM_SETICON, ICON_SMALL as usize, icon as isize);
        }
        // Dark title bar (Windows 10 2004 and later), and on Windows 11 the sidebar's colour
        let dark: i32 = 1;
        DwmSetWindowAttribute(hwnd, 20, &dark as *const i32 as _, 4);
        let caption: u32 = 0x0018_1310;
        DwmSetWindowAttribute(hwnd, 35, &caption as *const u32 as _, 4);
        SetForegroundWindow(hwnd);
    }
    true
}

/// When the window is already open, brings it to the front instead of opening another.
fn already_open() -> bool {
    use crate::win::wide;
    use windows_sys::Win32::Foundation::{GetLastError, ERROR_ALREADY_EXISTS};
    use windows_sys::Win32::System::Threading::CreateMutexW;
    use windows_sys::Win32::UI::WindowsAndMessaging::{FindWindowW, IsIconic, SetForegroundWindow, ShowWindow, SW_RESTORE};
    unsafe {
        // Released when this process exits
        let mutex = CreateMutexW(std::ptr::null(), 0, wide("Local\\MagicRemoteService.Settings").as_ptr());
        if mutex.is_null() || GetLastError() != ERROR_ALREADY_EXISTS {
            return false;
        }
        let hwnd = FindWindowW(std::ptr::null(), wide(TITLE).as_ptr());
        if !hwnd.is_null() {
            if IsIconic(hwnd) != 0 {
                ShowWindow(hwnd, SW_RESTORE);
            }
            SetForegroundWindow(hwnd);
        }
        true
    }
}

/// Opens the settings window in its own process, as the user signed in at the PC (never as
/// SYSTEM); the web page if it can't start.
pub fn launch(http_port: u16) {
    use windows_sys::Win32::UI::WindowsAndMessaging::{AllowSetForegroundWindow, ASFW_ANY};
    // The tray click gave us the right to bring a window to the front: pass it on
    unsafe { AllowSetForegroundWindow(ASFW_ANY) };
    let started = std::env::current_exe().and_then(|exe| crate::win::proc::spawn_as_user(&exe, &format!("ui --http-port {http_port}"), true));
    if let Err(e) = started {
        crate::log::warn(format!("Cannot open the settings window ({e}); opening the web page instead"));
        let _ = crate::win::proc::open_url(&format!("http://localhost:{http_port}/"));
    }
}

/// Opens the settings window for the service whose API listens on `http_port`.
pub fn run(http_port: u16) -> i32 {
    if already_open() {
        return 0;
    }
    let Ok(ui) = MainWindow::new() else { return 1 };
    with(|s| s.port = http_port);
    ui.global::<App>().set_api_address(format!("127.0.0.1:{http_port}").into());
    wire(&ui);
    load_all(&ui);
    let weak = ui.as_weak();
    let refresh = Timer::default();
    refresh.start(TimerMode::Repeated, Duration::from_secs(3), move || {
        if let Some(ui) = weak.upgrade() {
            poll(&ui);
        }
    });
    // The window exists once the event loop runs: try until it's there (about 2 s at most)
    let styling = Timer::default();
    let tries = std::cell::Cell::new(0);
    styling.start(TimerMode::Repeated, Duration::from_millis(25), move || {
        tries.set(tries.get() + 1);
        if style_window() || tries.get() > 80 {
            STYLING.with(|t| t.borrow_mut().take());
        }
    });
    STYLING.with(|t| *t.borrow_mut() = Some(styling));
    match ui.run() {
        Ok(()) => 0,
        Err(_) => 1,
    }
}
