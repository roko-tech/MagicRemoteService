//! The Setup page: install the service, find and pair a TV, install the TV app. What's done is
//! worked out from the service's own API, so a set-up PC opens on the Overview instead.

use super::{background, model, port, toast, with, App, FoundTv, MainWindow, LONG, SHORT};
use crate::win::{service, wide, Handle};
use crate::{client, discover, settings::Scope};
use serde_json::json;
use slint::{ComponentHandle, ModelRc, VecModel};
use std::net::Ipv4Addr;
use std::path::Path;
use std::time::Duration;

const THIS_VERSION: &str = env!("CARGO_PKG_VERSION");

/// What setup knows about the PC, kept with the window's state.
#[derive(Default)]
pub(super) struct Setup {
    pub connected: bool,
    pub version: String,
    pub mode: String,
    /// (name, ip, default)
    pub tvs: Vec<(String, String, bool)>,
    pub found: Vec<discover::FoundTv>,
    pub searched: bool,
    pub searching: bool,
    pub page_chosen: bool,
    pub pair_again: bool,
    pub tv_app_again: bool,
}

/// Whether the TV app has been set up: a saved config that names the PC and the input.
fn tv_app_configured(config: &serde_json::Value) -> bool {
    config["ip"].as_str().is_some_and(|s| !s.is_empty()) && config["appId"].as_str().is_some_and(|s| !s.is_empty())
}

/// Recomputes every step from what's known, and starts a TV search when pairing is next.
pub(super) fn refresh(ui: &MainWindow) {
    let app = ui.global::<App>();
    let (connected, version, mode, tvs, config, pair_again, tv_app_again) = with(|s| {
        let x = &s.setup;
        (x.connected, x.version.clone(), x.mode.clone(), x.tvs.clone(), s.config.clone(), x.pair_again, x.tv_app_again)
    });

    // 1. The service
    let installed = service::state();
    let (install_step, button, text, summary) = if connected && mode == "service" && model::newer(THIS_VERSION, &version) {
        (1, "Update", format!("Version {version} is installed; this one is {THIS_VERSION}. Windows will ask for your OK."), String::new())
    } else if connected {
        let how = if mode == "service" {
            "Installed as a Windows service, so the remote works as soon as Windows starts"
        } else {
            "Running on its own, not as a service"
        };
        (2, "Install", String::new(), format!("{how}. Version {version}."))
    } else {
        match installed {
            None => (
                1,
                "Install",
                "Installs MagicRemoteService as a Windows service, so the remote works as soon as Windows starts. Windows will ask for your OK.".to_string(),
                String::new(),
            ),
            Some(_) => {
                (1, "Repair", "MagicRemoteService is installed but isn't answering. Repairing installs it again and starts it.".to_string(), String::new())
            }
        }
    };
    app.set_step_install(install_step);
    app.set_install_button(button.into());
    app.set_install_text(text.into());
    app.set_install_summary(summary.into());
    if !connected && installed.is_none() {
        app.set_status_text("Not set up yet".into());
        app.set_status_kind(1);
    }

    // 2. The TV
    let default_tv = tvs.iter().find(|t| t.2).or(tvs.first()).cloned();
    let pair_step = if !connected {
        0
    } else if default_tv.is_none() || pair_again {
        1
    } else {
        2
    };
    app.set_step_pair(pair_step);
    app.set_pair_summary(default_tv.as_ref().map(|(name, ip, _)| format!("Paired with {name} ({ip}).")).unwrap_or_default().into());

    // 3. The TV app
    let configured = tv_app_configured(&config);
    let app_step = if pair_step != 2 {
        0
    } else if !configured || tv_app_again {
        1
    } else {
        2
    };
    app.set_step_tv_app(app_step);
    let input = config["inputName"].as_str().unwrap_or("its HDMI input").to_string();
    app.set_tv_app_summary(
        default_tv
            .as_ref()
            .map(|(name, ..)| format!("Set up on {name} for {input}. Open MagicRemoteService on the TV, or switch it to {input}, and point the remote."))
            .unwrap_or_default()
            .into(),
    );
    let network = with(|s| {
        let tv: Option<Ipv4Addr> = default_tv.as_ref().and_then(|t| t.1.parse().ok());
        let i = tv.and_then(|tv| model::network_for(tv, &s.networks)).unwrap_or(0);
        s.network_labels.get(i).cloned()
    });
    app.set_setup_network(network.map(|n| format!("The TV will reach this PC through {n}.")).unwrap_or_default().into());
    if app_step == 1 {
        if let Some(n) = config["inputId"].as_str().and_then(|s| s.strip_prefix("HDMI_")).and_then(|n| n.parse::<i32>().ok()) {
            app.set_setup_hdmi((n - 1).clamp(0, 3));
        }
    }

    // An update counts as unfinished: someone who runs a newer exe wants it in front of them
    let complete = connected && install_step == 2 && pair_step == 2 && app_step == 2;
    app.set_setup_complete(complete);

    // Search for TVs as soon as pairing is the next step
    if pair_step == 1 && !with(|s| s.setup.searched) {
        search(ui);
    }
}

/// Opens the Setup page the first time the window has something to show, unless setup is done.
pub(super) fn choose_first_page(ui: &MainWindow) {
    let first = with(|s| !std::mem::replace(&mut s.setup.page_chosen, true));
    if first && !ui.global::<App>().get_setup_complete() {
        ui.global::<App>().set_page(4);
    }
}

fn search(ui: &MainWindow) {
    if with(|s| std::mem::replace(&mut s.setup.searching, true)) {
        return;
    }
    with(|s| s.setup.searched = true);
    ui.global::<App>().set_searching(true);
    background(
        ui,
        || discover::find_tvs(Duration::from_secs(3)),
        |ui, found| {
            let app = ui.global::<App>();
            app.set_searching(false);
            let rows: Vec<FoundTv> = found.iter().map(|t| FoundTv { name: t.name.clone().into(), ip: t.ip.to_string().into() }).collect();
            app.set_found_index(if rows.is_empty() { -1 } else { 0 });
            app.set_found_tvs(ModelRc::new(VecModel::from(rows)));
            with(|s| {
                s.setup.searching = false;
                s.setup.found = found;
            });
        },
    );
}

/// Runs `exe args` elevated (Windows asks for consent) and waits for it. Returns the exit code.
fn run_elevated(exe: &Path, args: &str) -> Result<u32, String> {
    use windows_sys::Win32::Foundation::{GetLastError, ERROR_CANCELLED};
    use windows_sys::Win32::System::Threading::{GetExitCodeProcess, WaitForSingleObject};
    use windows_sys::Win32::UI::Shell::{ShellExecuteExW, SEE_MASK_NOCLOSEPROCESS, SHELLEXECUTEINFOW};
    use windows_sys::Win32::UI::WindowsAndMessaging::SW_HIDE;
    let (verb, file, params) = (wide("runas"), wide(&exe.to_string_lossy()), wide(args));
    let mut info: SHELLEXECUTEINFOW = unsafe { std::mem::zeroed() };
    info.cbSize = std::mem::size_of::<SHELLEXECUTEINFOW>() as u32;
    info.fMask = SEE_MASK_NOCLOSEPROCESS;
    info.lpVerb = verb.as_ptr();
    info.lpFile = file.as_ptr();
    info.lpParameters = params.as_ptr();
    info.nShow = SW_HIDE;
    if unsafe { ShellExecuteExW(&mut info) } == 0 {
        return Err(if unsafe { GetLastError() } == ERROR_CANCELLED {
            "Windows didn't get your OK, so nothing was installed.".into()
        } else {
            format!("Windows couldn't start the installer: {}", std::io::Error::last_os_error())
        });
    }
    let process = Handle::new(info.hProcess).map_err(|e| e.to_string())?;
    unsafe { WaitForSingleObject(process.0, 5 * 60 * 1000) };
    let mut code = 1u32;
    unsafe { GetExitCodeProcess(process.0, &mut code) };
    Ok(code)
}

/// Installs, repairs or updates the service from this exe, then talks to it.
fn install(ui: &MainWindow) {
    let app = ui.global::<App>();
    app.set_installing(true);
    app.set_install_error("".into());
    background(
        ui,
        || -> Result<String, String> {
            let exe = std::env::current_exe().map_err(|e| e.to_string())?;
            let result = std::env::temp_dir().join(format!("MagicRemoteService-install-{}.txt", std::process::id()));
            let code = run_elevated(&exe, &format!("install --result-file \"{}\"", result.display()))?;
            let message = std::fs::read_to_string(&result).unwrap_or_default().trim().to_string();
            let _ = std::fs::remove_file(&result);
            if code == 0 {
                Ok(message)
            } else {
                Err(if message.is_empty() { format!("The installer stopped with code {code}.") } else { message })
            }
        },
        |ui, r| {
            let app = ui.global::<App>();
            app.set_installing(false);
            match r {
                Ok(_) => {
                    // The service listens where its machine-wide settings say
                    let http = Scope::Machine.load().port.saturating_add(1);
                    with(|s| s.port = http);
                    app.set_api_address(format!("127.0.0.1:{http}").into());
                    app.set_loaded(false);
                    toast(&ui, "MagicRemoteService is installed and running.", false);
                    super::load_all(&ui);
                }
                Err(e) => {
                    app.set_install_error(e.clone().into());
                    toast(&ui, &e, true);
                }
            }
        },
    );
}

/// Saves the TV app settings for the chosen input (and a device token if there's none yet), then
/// installs the app on the default TV.
fn set_up_tv_app(ui: &MainWindow) {
    let app = ui.global::<App>();
    let hdmi = (app.get_setup_hdmi() + 1) as u32;
    let tv_port = u16::try_from(app.get_pc_port()).unwrap_or(41230);
    let has_token = app.get_has_token();
    let plan = with(|s| {
        let tv = s.setup.tvs.iter().find(|t| t.2).or(s.setup.tvs.first())?.clone();
        let tv_ip: Ipv4Addr = tv.1.parse().ok()?;
        let i = model::network_for(tv_ip, &s.networks).unwrap_or(0);
        let (ip, mask, mac) = s.networks.get(i)?.clone();
        let num = |k: &str, d: f64| s.config[k].as_f64().filter(|v| *v > 0.0).unwrap_or(d);
        let form = model::TvApp {
            hdmi,
            ip,
            mask,
            mac,
            port: tv_port,
            cursor_speed: num("cursorSpeed", 1.0),
            scroll_speed: num("scrollSpeed", 1.0),
            long_click: num("longClick", 1500.0) as u32,
        };
        Some((tv.0, model::config_post(&s.config, &form).0))
    });
    let Some((tv_name, body)) = plan else {
        return toast(ui, "Pair a TV first, and check this PC is on a network.", true);
    };
    app.set_setup_installing(true);
    app.set_setup_failed(false);
    app.set_setup_status(format!("Installing on {tv_name}. This takes a few seconds…").into());
    let port = port();
    background(
        ui,
        move || -> Result<(String, serde_json::Value), String> {
            client::call(port, "POST", "/api/config", Some(&body), SHORT)?;
            // The TV must carry a token, or any device on the network could drive the PC
            if !has_token {
                client::call(port, "POST", "/api/token", Some(&json!({})), SHORT)?;
            }
            let r = client::call(port, "POST", "/api/reinstall", None, LONG)?;
            let message = r["message"].as_str().unwrap_or_default().to_string();
            if r["success"] == true {
                Ok((message, body))
            } else {
                Err(message)
            }
        },
        |ui, r| {
            let app = ui.global::<App>();
            app.set_setup_installing(false);
            match r {
                Ok((message, saved)) => {
                    app.set_setup_status("".into());
                    app.set_has_token(true);
                    with(|s| {
                        s.config = saved;
                        s.setup.tv_app_again = false;
                    });
                    refresh(&ui);
                    toast(&ui, &message, false);
                }
                Err(e) => {
                    app.set_setup_failed(true);
                    app.set_setup_status(e.clone().into());
                }
            }
        },
    );
}

pub(super) fn wire(ui: &MainWindow) {
    let app = ui.global::<App>();
    let w = ui.as_weak();
    app.on_install_service(move || {
        if let Some(ui) = w.upgrade() {
            install(&ui);
        }
    });
    let w = ui.as_weak();
    app.on_search_tvs(move || {
        if let Some(ui) = w.upgrade() {
            search(&ui);
        }
    });
    let w = ui.as_weak();
    app.on_pair_again(move || {
        if let Some(ui) = w.upgrade() {
            with(|s| {
                s.setup.pair_again = true;
                s.setup.searched = false;
            });
            ui.global::<App>().set_pair_status("".into());
            refresh(&ui);
        }
    });
    let w = ui.as_weak();
    app.on_setup_tv_app(move || {
        if let Some(ui) = w.upgrade() {
            set_up_tv_app(&ui);
        }
    });
    let w = ui.as_weak();
    app.on_tv_app_again(move || {
        if let Some(ui) = w.upgrade() {
            with(|s| s.setup.tv_app_again = true);
            refresh(&ui);
        }
    });
}
