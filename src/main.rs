// A GUI-subsystem program: no console window for the service, the worker or the tray.
#![windows_subsystem = "windows"]

use mrs::settings::Scope;
use mrs::win::{proc, service};
use mrs::worker::{self, Options};

const HELP: &str = "MagicRemoteService - control this PC with the LG Magic Remote

Usage:
  MagicRemoteService.exe              Open the settings, which set everything up the first time
                                      (Windows starts it as the service)
  MagicRemoteService.exe install      Install or update the service from this exe and start it (admin)
  MagicRemoteService.exe uninstall    Stop and remove the service (admin)
  MagicRemoteService.exe ui           Open the settings window
  MagicRemoteService.exe status       Show the service state and connected TVs
  MagicRemoteService.exe private-network <id>
                                      Make that network Private, so the firewall lets the TV in (admin)
  MagicRemoteService.exe --version

Running without the service:
  --standalone   Run on its own with a tray icon
  --port <n>     Listen for TVs on <n> and serve the settings on <n+1> (to try a build beside the service)
  --no-ssdp      Don't answer the TV's network discovery";

/// The settings API the window talks to: `--http-port`, else the service's (or, with no service
/// running, this user's standalone) port plus one.
fn ui_port(args: &[String]) -> u16 {
    let given = args.iter().position(|a| a == "--http-port").and_then(|i| args.get(i + 1)?.parse().ok());
    given.unwrap_or_else(|| {
        let scope = if service::state() == Some(true) { Scope::Machine } else { Scope::current() };
        scope.load().port.saturating_add(1)
    })
}

/// Lets CLI commands print into the console they were started from.
fn attach_console() {
    use windows_sys::Win32::System::Console::{AttachConsole, ATTACH_PARENT_PROCESS};
    unsafe { AttachConsole(ATTACH_PARENT_PROCESS) };
}

/// The settings window runs commands elevated and shows what happened from `--result-file`.
fn write_result_file(args: &[String], result: &Result<String, String>) {
    if let Some(path) = args.iter().position(|a| a == "--result-file").and_then(|i| args.get(i + 1)) {
        let _ = std::fs::write(path, result.as_ref().unwrap_or_else(|e| e));
    }
}

fn print_result(r: Result<String, String>) -> i32 {
    match r {
        Ok(msg) => {
            println!("{msg}");
            0
        }
        Err(msg) => {
            eprintln!("{msg}");
            1
        }
    }
}

/// Status from the running service's own API, without printing anything secret.
fn status() -> Result<String, String> {
    let state = match service::state() {
        Some(true) => "running",
        Some(false) => "installed, not running",
        None => "not installed",
    };
    let port = Scope::Machine.load().port;
    let mut out = format!("Service: {state}\nTV port: {port}\nSettings: {}", worker::settings_url(port + 1));
    if let Ok(body) = mrs::client::get(port + 1, "/api/settings") {
        if let Ok(v) = serde_json::from_str::<serde_json::Value>(&body) {
            out += &format!("\nVersion: {}\nConnected TVs: {}", v["version"].as_str().unwrap_or("2.x (C#)"), v["connectedClients"]);
            for c in v["clients"].as_array().into_iter().flatten() {
                out += &format!("\n  {}", c["ip"].as_str().unwrap_or("?"));
            }
        }
    } else {
        out += "\nThe settings server is not answering.";
    }
    Ok(out)
}

fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let code = match args.first().map(String::as_str) {
        Some("install") => {
            attach_console();
            let result = service::install();
            write_result_file(&args, &result);
            print_result(result)
        }
        Some("private-network") => {
            attach_console();
            let result = match args.get(1) {
                Some(id) => mrs::win::netlist::make_private(id),
                None => Err("Usage: MagicRemoteService.exe private-network <network id>".into()),
            };
            write_result_file(&args, &result);
            print_result(result)
        }
        Some("uninstall") => {
            attach_console();
            print_result(service::uninstall())
        }
        Some("status") => {
            attach_console();
            print_result(status())
        }
        Some("ui") => mrs::ui::run(ui_port(&args)),
        Some("--version" | "version") => {
            attach_console();
            print_result(Ok(format!("MagicRemoteService {}", env!("CARGO_PKG_VERSION"))))
        }
        Some("help" | "--help" | "-h" | "/?") => {
            attach_console();
            print_result(Ok(HELP.into()))
        }
        _ => run(args),
    };
    std::process::exit(code);
}

fn run(args: Vec<String>) -> i32 {
    let mut opts = Options { parent: None, port: None, ssdp: true, after: None, args: Vec::new() };
    let mut service_worker = false;
    let mut it = args.iter();
    while let Some(a) = it.next() {
        match a.as_str() {
            "-c" => service_worker = true,
            "--parent" => opts.parent = it.next().and_then(|v| v.parse().ok()),
            "--after" => opts.after = it.next().and_then(|v| v.parse().ok()),
            "--port" => {
                opts.port = it.next().and_then(|v| v.parse().ok());
                opts.args.extend(["--port".to_string(), opts.port.unwrap_or_default().to_string()]);
            }
            "--no-ssdp" => {
                opts.ssdp = false;
                opts.args.push(a.clone());
            }
            _ => {}
        }
    }
    if service_worker {
        // Started by the service: needs to know which process to follow
        opts.parent = opts.parent.or(Some(0));
        return worker::run(opts);
    }
    if args.is_empty() {
        // Started by the service control manager: run the supervisor
        if service::run().is_ok() {
            return 0;
        }
        // Started by hand: show the settings, which also install the service the first time. An
        // earlier version's "Start with Windows" (without the service) ran the bare exe: keep
        // running on its own for that.
        if !proc::is_system() && !worker::legacy_standalone_startup() {
            return mrs::ui::run(ui_port(&[]));
        }
    }
    worker::run(opts)
}
