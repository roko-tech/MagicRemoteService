//! The TV app built into the exe, the TVs this PC deploys it to, and deploying it. The package is
//! built here and installed over the TV's Developer Mode SSH with Windows' own OpenSSH client,
//! so no Node.js or webOS CLI is needed.

use crate::settings::{self, Files};
use crate::win::{acl, proc};
use crate::{ipk, log};
use serde_json::{json, Value};
use std::io::{self, BufRead, BufReader, Read, Write};
use std::net::{Ipv4Addr, SocketAddr, TcpStream};
use std::os::windows::process::CommandExt;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::sync::mpsc::{self, RecvTimeoutError};
use std::sync::{Mutex, MutexGuard};
use std::time::{Duration, Instant, SystemTime};

macro_rules! tv_files {
    ($($path:literal),* $(,)?) => {
        &[$(($path, include_bytes!(concat!("../tv/", $path)) as &[u8])),*]
    };
}

/// TV sources as `install.bat` packages them (`config.json` comes from `tv-config.json`).
static FILES: &[(&str, &[u8])] = tv_files![
    "app/appinfo.json",
    "app/appstring.json",
    "app/index.html",
    "app/main.js",
    "app/icon.png",
    "app/miniIcon.png",
    "app/largeIcon.png",
    "app/cursor.png",
    "app/MuseoSans-Medium.ttf",
    "app/webOSTVjs-1.2.12/webOSTV-dev.js",
    "app/webOSTVjs-1.2.12/webOSTV.js",
    "app/resources/fr/appinfo.json",
    "app/resources/fr/appstring.json",
    "app/resources/es/appinfo.json",
    "app/resources/es/appstring.json",
    "service/package.json",
    "service/service.js",
    "service/services.json",
];

const TEMPLATE_APP_ID: &str = "com.cathwyler.magicremoteservice";
/// Where Developer Mode TVs serve their SSH key, and the SSH port and user they accept.
const KEY_SERVER_PORT: u16 = 9991;
const DEV_SSH_PORT: u16 = 9922;
const DEV_SSH_USER: &str = "prisoner";
/// Carries a TV key's passphrase to this exe when ssh runs it as `SSH_ASKPASS`.
pub const ASKPASS_ENV: &str = "MRS_ASKPASS";

/// One TV pairing or install at a time.
static TV_WORK: Mutex<()> = Mutex::new(());

fn one_at_a_time() -> Result<MutexGuard<'static, ()>, String> {
    match TV_WORK.try_lock() {
        Ok(guard) => Ok(guard),
        // A pairing or install that panicked left nothing that matters behind
        Err(std::sync::TryLockError::Poisoned(p)) => Ok(p.into_inner()),
        Err(std::sync::TryLockError::WouldBlock) => Err("Another TV pairing or install is running.".into()),
    }
}

/// Only the ids `install.bat` and the web UI generate, since the id ends up on a command line.
pub fn valid_app_id(id: &str) -> bool {
    id.strip_prefix("com.cathwyler.magicremoteservice.").is_some_and(|s| !s.is_empty() && s.bytes().all(|b| b.is_ascii_lowercase() || b.is_ascii_digit()))
}

/// The file contents to deploy, with the app and service ids, version and input name filled in.
pub fn render(app_id: &str, version: &str, input_name: &str, config: &[u8]) -> Vec<(String, Vec<u8>)> {
    let service_id = format!("{app_id}.service");
    let mut out: Vec<(String, Vec<u8>)> = FILES
        .iter()
        .map(|(path, data)| {
            let text = || String::from_utf8_lossy(data).into_owned();
            let data = match *path {
                "app/appinfo.json" => text()
                    .replace(&format!("\"id\": \"{TEMPLATE_APP_ID}\""), &format!("\"id\": \"{app_id}\""))
                    .replace("\"version\": \"1.0.0\"", &format!("\"version\": \"{version}\""))
                    .replace("\"appDescription\": \"HDMI\"", &format!("\"appDescription\": {}", Value::from(input_name)))
                    .into_bytes(),
                "service/package.json" | "service/services.json" => text().replace(&format!("{TEMPLATE_APP_ID}.service"), &service_id).into_bytes(),
                _ => data.to_vec(),
            };
            (path.to_string(), data)
        })
        .collect();
    out.push(("app/config.json".into(), config.to_vec()));
    out.push(("service/config.json".into(), config.to_vec()));
    out
}

/// A TV in Developer Mode this PC can install the TV app on.
#[derive(Debug, Clone, PartialEq)]
pub struct Tv {
    pub name: String,
    pub host: Ipv4Addr,
    pub port: u16,
    pub username: String,
    pub default: bool,
    /// Unlocks the TV's SSH key; never leaves `tvs.json`.
    passphrase: String,
}

impl Tv {
    fn from_json(v: &Value) -> Option<Tv> {
        let text = |k: &str| v.get(k).and_then(Value::as_str);
        Some(Tv {
            name: text("name").filter(|n| valid_tv_name(n))?.to_string(),
            host: text("host")?.parse().ok()?,
            port: v.get("port").and_then(Value::as_u64).and_then(|p| u16::try_from(p).ok()).unwrap_or(DEV_SSH_PORT),
            username: text("username").unwrap_or(DEV_SSH_USER).to_string(),
            default: v.get("default").and_then(Value::as_bool).unwrap_or(false),
            passphrase: text("passphrase").unwrap_or_default().to_string(),
        })
    }

    fn to_json(&self) -> Value {
        json!({ "name": self.name, "host": self.host.to_string(), "port": self.port, "username": self.username, "default": self.default, "passphrase": self.passphrase })
    }

    /// What the API shows: never the passphrase.
    pub fn public(&self) -> Value {
        json!({ "name": self.name, "ip": self.host.to_string(), "default": self.default })
    }
}

/// TV names end up in file names: letters, digits, `-` and `_`, up to 32.
pub fn valid_tv_name(name: &str) -> bool {
    (1..=32).contains(&name.len()) && name.bytes().all(|b| b.is_ascii_alphanumeric() || b == b'-' || b == b'_')
}

/// The paired TVs, or `None` before the first pairing or import.
pub fn load(files: &Files) -> Option<Vec<Tv>> {
    let doc = settings::read_json(&files.tvs())?;
    Some(doc.get("tvs")?.as_array()?.iter().filter_map(Tv::from_json).collect())
}

/// The paired TVs. The first time, the TVs already set up in the webOS CLI are imported.
pub fn tvs(files: &Files) -> Vec<Tv> {
    load(files).or_else(|| import_from_cli(files)).unwrap_or_default()
}

fn save(files: &Files, tvs: &[Tv]) -> io::Result<()> {
    let doc = json!({ "tvs": tvs.iter().map(Tv::to_json).collect::<Vec<_>>() });
    write_secret(&files.tvs(), &serde_json::to_vec_pretty(&doc).unwrap_or_default())
}

/// Writes a file only SYSTEM, administrators and this process's account can open (as the
/// service, just SYSTEM and administrators). Windows' SSH client refuses a key file others can read.
fn write_secret(path: &Path, data: &[u8]) -> io::Result<()> {
    let tmp = path.with_extension("tmp");
    std::fs::write(&tmp, data)?;
    acl::restrict_to_admins_and_self(&tmp)?;
    std::fs::rename(&tmp, path)
}

pub fn set_default(files: &Files, name: &str) -> Result<(), String> {
    let _one = one_at_a_time()?;
    let mut tvs = tvs(files);
    if !tvs.iter().any(|t| t.name == name) {
        return Err(format!("No TV named {name} is paired."));
    }
    tvs.iter_mut().for_each(|t| t.default = t.name == name);
    save(files, &tvs).map_err(|e| format!("Cannot save tvs.json: {e}"))
}

pub fn remove(files: &Files, name: &str) -> Result<(), String> {
    let _one = one_at_a_time()?;
    let mut tvs = tvs(files);
    let before = tvs.len();
    tvs.retain(|t| t.name != name);
    if tvs.len() == before {
        return Err(format!("No TV named {name} is paired."));
    }
    if !tvs.iter().any(|t| t.default) {
        if let Some(first) = tvs.first_mut() {
            first.default = true;
        }
    }
    save(files, &tvs).map_err(|e| format!("Cannot save tvs.json: {e}"))?;
    let _ = std::fs::remove_file(files.tv_key(name));
    log::info(format!("TV {name} unpaired"));
    Ok(())
}

/// The folders the webOS CLI keeps its TV list (`%APPDATA%\.webos\tv`) and keys (`~\.ssh`) in,
/// for the user signed in at the PC.
fn cli_dirs() -> Option<(PathBuf, PathBuf)> {
    if proc::is_system() {
        let profile = proc::profile_dir(&proc::console_user_token().ok()?)?;
        Some((profile.join("AppData\\Roaming"), profile))
    } else {
        Some((PathBuf::from(std::env::var_os("APPDATA")?), PathBuf::from(std::env::var_os("USERPROFILE")?)))
    }
}

/// Copies the TVs set up with `ares-setup-device` and `ares-novacom --getkey`, so an upgrade
/// keeps deploying where it did. `None` when there is nothing to import (yet).
fn import_from_cli(files: &Files) -> Option<Vec<Tv>> {
    let (appdata, profile) = cli_dirs()?;
    let list: Vec<Value> = serde_json::from_slice(&std::fs::read(appdata.join(".webos\\tv\\novacom-devices.json")).ok()?).ok()?;
    let mut tvs: Vec<Tv> = Vec::new();
    for d in &list {
        let text = |k: &str| d.get(k).and_then(Value::as_str);
        // The emulator entries point at this PC
        let Some(host) = text("host").and_then(|h| h.parse::<Ipv4Addr>().ok()).filter(|h| !h.is_loopback()) else { continue };
        let Some(key_file) = d.get("privateKey").and_then(|k| k.get("openSsh")).and_then(Value::as_str).filter(|f| valid_tv_name(&f.replace('.', "_"))) else {
            continue;
        };
        let Ok(key) = std::fs::read(profile.join(".ssh").join(key_file)) else { continue };
        let base: String = text("name").unwrap_or("TV").chars().map(|c| if c.is_ascii_alphanumeric() || c == '-' { c } else { '_' }).take(28).collect();
        let name = (1..).map(|i| if i == 1 { base.clone() } else { format!("{base}{i}") }).find(|n| valid_tv_name(n) && !tvs.iter().any(|t| &t.name == n))?;
        let port = d.get("port").and_then(|p| p.as_u64().or_else(|| p.as_str()?.parse().ok())).and_then(|p| u16::try_from(p).ok()).unwrap_or(DEV_SSH_PORT);
        if write_secret(&files.tv_key(&name), &key).is_err() {
            continue;
        }
        tvs.push(Tv {
            name,
            host,
            port,
            username: text("username").unwrap_or(DEV_SSH_USER).to_string(),
            default: d.get("default").and_then(Value::as_bool).unwrap_or(false),
            passphrase: text("passphrase").unwrap_or_default().to_string(),
        });
    }
    let first_default = tvs.iter().position(|t| t.default).unwrap_or(0);
    tvs.iter_mut().enumerate().for_each(|(i, t)| t.default = i == first_default);
    if tvs.is_empty() {
        return None;
    }
    if let Err(e) = save(files, &tvs) {
        log::warn(format!("Cannot save the TVs found in the webOS CLI: {e}"));
        return None;
    }
    log::info(format!("Imported the TVs set up in the webOS CLI: {}", tvs.iter().map(|t| t.name.as_str()).collect::<Vec<_>>().join(", ")));
    Some(tvs)
}

/// Downloads the TV's SSH key from the Developer Mode app's key server.
fn fetch_key(host: Ipv4Addr) -> Result<Vec<u8>, String> {
    let unreachable = || format!("Can't reach the key server on {host}. On the TV, open the Developer Mode app and turn on Key Server.");
    let mut s = TcpStream::connect_timeout(&SocketAddr::from((host, KEY_SERVER_PORT)), Duration::from_secs(5)).map_err(|_| unreachable())?;
    let _ = s.set_read_timeout(Some(Duration::from_secs(10)));
    let _ = s.set_write_timeout(Some(Duration::from_secs(10)));
    write!(s, "GET /webos_rsa HTTP/1.0\r\nHost: {host}:{KEY_SERVER_PORT}\r\n\r\n").map_err(|_| unreachable())?;
    let mut reply = Vec::new();
    s.take(64 * 1024).read_to_end(&mut reply).map_err(|_| unreachable())?;
    let body_at = reply.windows(4).position(|w| w == b"\r\n\r\n").map(|i| i + 4);
    let status_ok = reply.split(|&b| b == b'\n').next().is_some_and(|line| String::from_utf8_lossy(line).split_whitespace().nth(1) == Some("200"));
    match body_at {
        Some(at) if status_ok && reply[at..].starts_with(b"-----BEGIN ") && String::from_utf8_lossy(&reply[at..]).contains("PRIVATE KEY-----") => {
            Ok(reply[at..].to_vec())
        }
        _ => Err(format!("The key server on {host} didn't send a key. Turn Key Server off and on in the Developer Mode app, then try again.")),
    }
}

/// Removes the TV's host key, so a TV that was reset can be paired again.
fn forget_host(files: &Files, host: Ipv4Addr, port: u16) {
    let path = files.tv_known_hosts();
    let Ok(text) = std::fs::read_to_string(&path) else { return };
    let prefix = format!("[{host}]:{port} ");
    let kept: String = text.lines().filter(|l| !l.starts_with(&prefix)).map(|l| format!("{l}\n")).collect();
    let _ = std::fs::write(&path, kept);
}

struct SshOutput {
    stdout: String,
    stderr: String,
    success: bool,
}

/// Runs commands on a TV with Windows' OpenSSH client. The key stays encrypted on disk: ssh asks
/// this exe for its passphrase (`SSH_ASKPASS`).
struct Ssh<'a> {
    tv: &'a Tv,
    key: PathBuf,
    known_hosts: PathBuf,
    /// The program ssh runs for the passphrase: this exe.
    askpass: PathBuf,
}

impl Ssh<'_> {
    fn new<'a>(files: &Files, tv: &'a Tv, key: PathBuf) -> Ssh<'a> {
        Ssh { tv, key, known_hosts: files.tv_known_hosts(), askpass: std::env::current_exe().unwrap_or_default() }
    }

    fn command(&self, remote: &str) -> io::Result<Command> {
        let ssh = proc::system_dir().join("OpenSSH\\ssh.exe");
        if !ssh.is_file() {
            return Err(io::Error::new(io::ErrorKind::NotFound, "ssh.exe"));
        }
        let mut c = Command::new(ssh);
        c.args(["-F", "none", "-T", "-i"]).arg(&self.key);
        c.args(["-p", &self.tv.port.to_string(), "-l", &self.tv.username]);
        for option in [
            "IdentitiesOnly=yes",
            "PreferredAuthentications=publickey",
            "NumberOfPasswordPrompts=1",
            "ConnectTimeout=10",
            "ServerAliveInterval=10",
            // The TV's SSH server only knows the older RSA signatures
            "HostKeyAlgorithms=+ssh-rsa",
            "PubkeyAcceptedAlgorithms=+ssh-rsa",
            // Trusts the TV's host key the first time, then holds it to that
            "StrictHostKeyChecking=accept-new",
        ] {
            c.args(["-o", option]);
        }
        c.arg("-o").arg(format!("UserKnownHostsFile=\"{}\"", self.known_hosts.display()));
        c.arg(self.tv.host.to_string()).arg(remote);
        self.hidden_with_askpass(&mut c);
        Ok(c)
    }

    /// No window, pipes for all three streams, and this exe answering the passphrase prompt.
    fn hidden_with_askpass(&self, c: &mut Command) {
        const CREATE_NO_WINDOW: u32 = 0x0800_0000;
        c.env("SSH_ASKPASS", &self.askpass).env("SSH_ASKPASS_REQUIRE", "force").env(ASKPASS_ENV, &self.tv.passphrase);
        c.stdin(Stdio::piped()).stdout(Stdio::piped()).stderr(Stdio::piped()).creation_flags(CREATE_NO_WINDOW);
    }

    /// Whether the passphrase unlocks the key (`None` when ssh-keygen can't run). ssh itself says
    /// "permission denied" for a wrong passphrase, the same as for a key the TV refuses.
    fn unlocks_key(&self) -> Option<bool> {
        let mut c = Command::new(proc::system_dir().join("OpenSSH\\ssh-keygen.exe"));
        c.arg("-y").arg("-f").arg(&self.key);
        self.hidden_with_askpass(&mut c);
        Some(c.output().ok()?.status.success())
    }

    /// Runs `remote` with `input` on its stdin, handing each output line to `stop`. When `stop`
    /// says so, or the output ends, stdin is closed (which ends `luna-send -i`) and ssh gets a
    /// moment to finish. Fails only when ssh can't start or the TV doesn't answer in time.
    fn run(&self, remote: &str, input: Vec<u8>, timeout: Duration, mut stop: impl FnMut(&str) -> bool) -> Result<SshOutput, String> {
        let mut child = self.command(remote).and_then(|mut c| c.spawn()).map_err(|e| {
            if e.kind() == io::ErrorKind::NotFound {
                "Windows' OpenSSH Client isn't installed. Add it under Settings > System > Optional features, then try again.".into()
            } else {
                format!("Cannot start ssh: {e}")
            }
        })?;
        let (Some(mut stdin), Some(stdout), Some(stderr)) = (child.stdin.take(), child.stdout.take(), child.stderr.take()) else {
            let _ = child.kill();
            return Err("Cannot talk to ssh.".into());
        };
        let (tx, rx) = mpsc::channel();
        std::thread::spawn(move || {
            for line in BufReader::new(stdout).lines().map_while(Result::ok) {
                if tx.send(line).is_err() {
                    break;
                }
            }
        });
        let errors = std::thread::spawn(move || {
            let mut text = String::new();
            let _ = BufReader::new(stderr).read_to_string(&mut text);
            text
        });
        // A long input is written while the output is read, and its end closes stdin
        let mut held = None;
        if input.is_empty() {
            held = Some(stdin);
        } else {
            std::thread::spawn(move || {
                let _ = stdin.write_all(&input);
            });
        }
        let deadline = Instant::now() + timeout;
        let mut stdout = String::new();
        let mut timed_out = false;
        loop {
            match rx.recv_timeout(deadline.saturating_duration_since(Instant::now())) {
                Ok(line) => {
                    stdout.push_str(&line);
                    stdout.push('\n');
                    if stop(&line) {
                        break;
                    }
                }
                Err(RecvTimeoutError::Timeout) => {
                    timed_out = true;
                    break;
                }
                Err(RecvTimeoutError::Disconnected) => break,
            }
        }
        drop(held);
        let grace = Instant::now() + Duration::from_secs(5);
        let status = loop {
            match child.try_wait() {
                Ok(Some(status)) => break Some(status),
                Ok(None) if !timed_out && Instant::now() < grace => std::thread::sleep(Duration::from_millis(50)),
                _ => {
                    let _ = child.kill();
                    break child.wait().ok();
                }
            }
        };
        let stderr = errors.join().unwrap_or_default();
        if timed_out {
            return Err(format!("{} ({}) stopped answering.", self.tv.name, self.tv.host));
        }
        Ok(SshOutput { stdout, stderr, success: status.is_some_and(|s| s.success()) })
    }

    /// Calls an appInstallService method and follows its replies until it reports `done` or a
    /// failure.
    fn follow(&self, method: &str, param: Value, done: &str) -> Result<(), String> {
        let mut reply = String::new();
        let mut outcome = None;
        let out = self.run(
            &format!("/usr/bin/luna-send-pub -i luna://com.webos.appInstallService/dev/{method} '{param}'"),
            Vec::new(),
            Duration::from_secs(180),
            |line| {
                // A reply may span several lines
                reply.push_str(line);
                let Ok(v) = serde_json::from_str::<Value>(&reply) else { return false };
                reply.clear();
                outcome = luna_progress(&v, done);
                outcome.is_some()
            },
        )?;
        match outcome {
            Some(Ok(())) => Ok(()),
            Some(Err(why)) => Err(format!("{} said: {why}", self.tv.name)),
            None if !out.success && !out.stderr.trim().is_empty() => Err(ssh_error(self.tv, &out.stderr)),
            None => Err(format!("{} didn't say how it went.", self.tv.name)),
        }
    }

    /// Runs a short command and returns its output; ssh's own failures become a readable error.
    fn exec(&self, remote: &str, input: Vec<u8>, timeout: Duration) -> Result<String, String> {
        let out = self.run(remote, input, timeout, |_| false)?;
        if out.success {
            Ok(out.stdout)
        } else {
            Err(ssh_error(self.tv, &out.stderr))
        }
    }
}

/// What went wrong, from ssh's error output.
fn ssh_error(tv: &Tv, stderr: &str) -> String {
    let s = stderr.to_ascii_lowercase();
    let name = &tv.name;
    if ["connection refused", "timed out", "no route", "unreachable", "could not resolve"].iter().any(|m| s.contains(m)) {
        format!(
            "Can't reach {name} at {}. Check the TV is on and its Developer Mode app shows Dev Mode Status on (it turns off when the session runs out).",
            tv.host
        )
    } else if s.contains("host key") || s.contains("identification has changed") {
        format!("{name} isn't the TV that was paired at {} (was it reset?). Pair it again.", tv.host)
    } else if s.contains("permission denied") {
        format!("{name} refused this PC's key (reinstalling Developer Mode changes it). Pair the TV again.")
    } else {
        let last = stderr.lines().map(str::trim).rfind(|l| !l.is_empty()).unwrap_or("ssh failed");
        format!("{name}: {last}")
    }
}

/// Reads one reply of the TV's install service: `None` until its state says `done` or failed.
pub fn luna_progress(reply: &Value, done: &str) -> Option<Result<(), String>> {
    if reply.get("returnValue") == Some(&Value::Bool(false)) {
        let why = ["errorText", "errorMessage"].iter().find_map(|k| reply.get(*k).and_then(Value::as_str));
        return Some(Err(why.unwrap_or("the install service refused the package").to_string()));
    }
    let details = reply.get("details")?;
    let state = details.get("state")?.as_str()?;
    let lower = state.to_ascii_lowercase();
    if lower.contains("failed") {
        Some(Err(details.get("reason").and_then(Value::as_str).unwrap_or(state).to_string()))
    } else if lower.contains(done) || lower.starts_with("success") {
        Some(Ok(()))
    } else {
        None
    }
}

/// Downloads the TV's key, checks it opens an SSH session with `passphrase`, and saves the TV as
/// the default one.
pub fn pair(files: &Files, name: &str, host: Ipv4Addr, passphrase: &str) -> Result<String, String> {
    let passphrase = passphrase.trim();
    if passphrase.is_empty() || passphrase.len() > 64 || passphrase.chars().any(char::is_control) {
        return Err("Enter the passphrase the TV's Developer Mode app shows.".into());
    }
    if host.is_loopback() || host.is_unspecified() || host.is_broadcast() || host.is_multicast() {
        return Err("Enter the TV's IP address (its network settings show it).".into());
    }
    let _one = one_at_a_time()?;
    let mut tvs = tvs(files);
    let name = match name.trim() {
        "" => tvs.iter().find(|t| t.host == host).map(|t| t.name.clone()).unwrap_or_else(|| {
            (1..).map(|i| if i == 1 { "TV".to_string() } else { format!("TV{i}") }).find(|n| !tvs.iter().any(|t| &t.name == n)).unwrap_or_default()
        }),
        n if valid_tv_name(n) => n.to_string(),
        _ => return Err("A TV name can use letters, digits, - and _ (up to 32).".into()),
    };
    let key = fetch_key(host)?;
    let tv = Tv { name: name.clone(), host, port: DEV_SSH_PORT, username: DEV_SSH_USER.into(), default: true, passphrase: passphrase.to_string() };
    let staged = files.tv_key(&name).with_extension("new");
    write_secret(&staged, &key).map_err(|e| format!("Cannot save the TV's key: {e}"))?;
    let ssh = Ssh::new(files, &tv, staged.clone());
    if ssh.unlocks_key() == Some(false) {
        let _ = std::fs::remove_file(&staged);
        return Err("Wrong passphrase: enter the one the TV's Developer Mode app shows (it changes when Developer Mode is reinstalled).".into());
    }
    forget_host(files, host, tv.port);
    let check = ssh.run("echo paired", Vec::new(), Duration::from_secs(30), |_| false);
    if !matches!(&check, Ok(o) if o.stdout.trim() == "paired") {
        let _ = std::fs::remove_file(&staged);
        return Err(match check {
            Ok(o) => ssh_error(&tv, &o.stderr),
            Err(e) => e,
        });
    }
    std::fs::rename(&staged, files.tv_key(&name)).map_err(|e| format!("Cannot save the TV's key: {e}"))?;
    for old in tvs.iter().filter(|t| t.host == host && t.name != name) {
        let _ = std::fs::remove_file(files.tv_key(&old.name));
    }
    tvs.retain(|t| t.name != name && t.host != host);
    tvs.iter_mut().for_each(|t| t.default = false);
    tvs.push(tv);
    save(files, &tvs).map_err(|e| format!("Cannot save tvs.json: {e}"))?;
    log::info(format!("Paired TV {name} at {host}"));
    Ok(format!("Paired {name} ({host}). Reinstall TV App now installs on it."))
}

/// Copies a package to the TV and installs it.
fn install(ssh: &Ssh, app_id: &str, version: &str, package: Vec<u8>) -> Result<(), String> {
    let name = &ssh.tv.name;
    let remote = format!("/media/developer/temp/{app_id}_{version}_all.ipk");
    // The byte count it reports back confirms the package arrived whole
    let size = package.len();
    let copied = ssh.exec(&format!("mkdir -p /media/developer/temp && cat > {remote} && wc -c < {remote}"), package, Duration::from_secs(120))?;
    let result = if copied.trim() == size.to_string() {
        ssh.follow("install", json!({ "id": "com.ares.defaultName", "ipkUrl": remote, "subscribe": true }), "installed")
    } else {
        Err(format!("Copying the TV app to {name} failed ({} of {size} bytes arrived).", copied.trim()))
    };
    // The package holds the device token: remove it whatever happened
    let _ = ssh.exec(&format!("rm -f {remote}"), Vec::new(), Duration::from_secs(30));
    result
}

/// Packages the built-in TV app with `tv-config.json`, installs it on the default TV and launches it.
pub fn reinstall(files: &Files, version: &str) -> Result<String, String> {
    let config = std::fs::read(files.tv_config()).map_err(|_| "No tv-config.json found. Save the TV config first.".to_string())?;
    let doc: Value = serde_json::from_slice(&config).map_err(|_| "tv-config.json is not valid JSON.".to_string())?;
    let app_id = doc.get("appId").and_then(Value::as_str).unwrap_or_default().to_string();
    if !valid_app_id(&app_id) {
        return Err("Invalid appId in tv-config.json.".into());
    }
    let input_name = doc.get("inputName").and_then(Value::as_str).unwrap_or("HDMI");
    let _one = one_at_a_time()?;
    let tvs = tvs(files);
    let tv = tvs.iter().find(|t| t.default).or(tvs.first()).ok_or("No TV is paired yet. Pair one under TVs first.")?;
    let ssh = Ssh::new(files, tv, files.tv_key(&tv.name));
    let now = SystemTime::now().duration_since(SystemTime::UNIX_EPOCH).map_or(0, |d| d.as_secs());
    install(&ssh, &app_id, version, ipk::build(&app_id, version, &render(&app_id, version, input_name, &config), now))?;
    let name = &tv.name;
    let launch = json!({ "id": app_id });
    let out = ssh.exec(&format!("/usr/bin/luna-send-pub -n 1 luna://com.webos.applicationManager/launch '{launch}'"), Vec::new(), Duration::from_secs(30))?;
    let replies: Vec<Value> = out.lines().filter_map(|l| serde_json::from_str(l).ok()).collect();
    if !replies.iter().any(|v| v.get("returnValue") == Some(&Value::Bool(true))) {
        let why = replies.iter().find_map(|v| v.get("errorText").and_then(Value::as_str)).unwrap_or("no answer");
        return Err(format!("Installed {app_id} {version} on {name}, but it didn't start: {why}"));
    }
    Ok(format!("Installed and launched {app_id} {version} on {name}."))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn app_ids_are_restricted() {
        assert!(valid_app_id("com.cathwyler.magicremoteservice.hdmi4"));
        for bad in [
            "com.cathwyler.magicremoteservice.",
            "com.cathwyler.magicremoteservice.HDMI",
            "com.cathwyler.magicremoteservice.a b",
            "x\" & calc",
            "com.other.hdmi1",
        ] {
            assert!(!valid_app_id(bad), "{bad}");
        }
    }

    #[test]
    fn renders_ids_version_and_config() {
        let files = render("com.cathwyler.magicremoteservice.hdmi4", "3.0.0", "HDMI \"4\"", b"{\"token\":\"x\"}");
        let get = |p: &str| String::from_utf8(files.iter().find(|(n, _)| n == p).unwrap().1.clone()).unwrap();
        let appinfo: Value = serde_json::from_str(&get("app/appinfo.json")).unwrap();
        assert_eq!(appinfo["id"], "com.cathwyler.magicremoteservice.hdmi4");
        assert_eq!(appinfo["version"], "3.0.0");
        assert_eq!(appinfo["appDescription"], "HDMI \"4\"");
        let services: Value = serde_json::from_str(&get("service/services.json")).unwrap();
        assert_eq!(services["id"], "com.cathwyler.magicremoteservice.hdmi4.service");
        assert_eq!(services["services"][0]["name"], "com.cathwyler.magicremoteservice.hdmi4.service");
        assert!(get("service/package.json").contains("\"com.cathwyler.magicremoteservice.hdmi4.service\""));
        assert_eq!(get("service/config.json"), "{\"token\":\"x\"}");
        assert_eq!(files.len(), FILES.len() + 2);
    }

    fn temp_files(name: &str) -> Files {
        let dir = std::env::temp_dir().join(format!("mrs-tv-{name}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        Files { dir }
    }

    fn tv(name: &str, last: u8, default: bool) -> Tv {
        Tv { name: name.into(), host: Ipv4Addr::new(192, 168, 1, last), port: 9922, username: "prisoner".into(), default, passphrase: "ABC123".into() }
    }

    #[test]
    fn paired_tvs_round_trip_and_hide_the_passphrase() {
        let files = temp_files("store");
        assert_eq!(load(&files), None);
        save(&files, &[tv("Living", 10, true), tv("Bedroom", 11, false)]).unwrap();
        std::fs::write(files.tv_key("Living"), "key").unwrap();
        assert_eq!(load(&files).unwrap(), [tv("Living", 10, true), tv("Bedroom", 11, false)]);
        assert_eq!(tv("Living", 10, true).public(), json!({"name": "Living", "ip": "192.168.1.10", "default": true}));

        set_default(&files, "Bedroom").unwrap();
        assert_eq!(load(&files).unwrap().iter().map(|t| t.default).collect::<Vec<_>>(), [false, true]);
        assert!(set_default(&files, "Kitchen").is_err());
        // Removing the default TV makes the next one the default, and its key goes too
        remove(&files, "Bedroom").unwrap();
        remove(&files, "Living").unwrap();
        assert_eq!(load(&files), Some(vec![]));
        assert!(!files.tv_key("Living").exists());
        let _ = std::fs::remove_dir_all(&files.dir);
    }

    #[test]
    fn tv_names_are_file_safe() {
        assert!(valid_tv_name("C2") && valid_tv_name("Living_room-2"));
        for bad in ["", "a b", "..", "a/b", "x\\y", &"n".repeat(33)] {
            assert!(!valid_tv_name(bad), "{bad}");
        }
    }

    #[test]
    fn install_replies_are_read_like_ares_install() {
        let p = |v: Value| luna_progress(&v, "installed");
        assert_eq!(p(json!({"returnValue": true, "subscribed": true})), None);
        assert_eq!(p(json!({"details": {"state": "installing", "progress": 40}})), None);
        assert_eq!(p(json!({"details": {"state": "installed"}})), Some(Ok(())));
        assert_eq!(p(json!({"details": {"state": "SUCCESS"}})), Some(Ok(())));
        assert_eq!(p(json!({"details": {"state": "install failed", "reason": "no space"}})), Some(Err("no space".into())));
        assert_eq!(p(json!({"returnValue": false, "errorText": "denied"})), Some(Err("denied".into())));
        assert_eq!(luna_progress(&json!({"details": {"state": "removed"}}), "removed"), Some(Ok(())));
    }

    #[test]
    fn pairing_says_to_turn_on_the_key_server() {
        assert!(fetch_key(Ipv4Addr::LOCALHOST).unwrap_err().contains("turn on Key Server"));
    }

    #[test]
    #[ignore = "installs a throwaway copy of the TV app on the TV set up in the webOS CLI, then removes it; needs `cargo build` first"]
    fn installs_and_removes_on_a_real_tv() {
        const ID: &str = "com.cathwyler.magicremoteservice.hdmi9";
        struct Cleanup(PathBuf);
        impl Drop for Cleanup {
            fn drop(&mut self) {
                let _ = std::fs::remove_dir_all(&self.0);
            }
        }
        let files = temp_files("real");
        let _cleanup = Cleanup(files.dir.clone());
        let tvs = tvs(&files);
        let tv = tvs.iter().find(|t| t.default).expect("a TV set up with ares-setup-device and ares-novacom --getkey");
        let mut ssh = Ssh::new(&files, tv, files.tv_key(&tv.name));
        // ssh asks the service exe for the passphrase, not this test binary
        ssh.askpass = std::env::current_exe().unwrap().parent().unwrap().parent().unwrap().join("MagicRemoteService.exe");
        assert!(ssh.askpass.is_file(), "{} missing: run cargo build", ssh.askpass.display());
        let now = SystemTime::now().duration_since(SystemTime::UNIX_EPOCH).unwrap().as_secs();
        install(&ssh, ID, "0.0.1", ipk::build(ID, "0.0.1", &render(ID, "0.0.1", "HDMI 9", b"{}"), now)).unwrap();
        let info = ssh.exec("/usr/bin/luna-send-pub -n 1 luna://com.webos.applicationManager/dev/listApps '{}'", Vec::new(), Duration::from_secs(30));
        ssh.follow("remove", json!({ "id": ID, "subscribe": true }), "removed").unwrap();
        let info = info.unwrap();
        let listed: Value = serde_json::from_str(info.trim()).unwrap();
        let app = listed["apps"].as_array().unwrap().iter().find(|a| a["id"] == ID).cloned().unwrap_or_default();
        assert_eq!((app["version"].as_str(), app["title"].as_str()), (Some("0.0.1"), Some("MagicRemoteService")), "{info}");
        assert!(!ssh.exec("ls /media/developer/temp", Vec::new(), Duration::from_secs(30)).unwrap().contains(ID), "the package was left on the TV");
    }

    #[test]
    fn ssh_errors_say_what_to_do() {
        let t = tv("C2", 145, true);
        assert!(ssh_error(&t, "ssh: connect to host 192.168.1.145 port 9922: Connection timed out").contains("Dev Mode Status"));
        assert!(ssh_error(&t, "prisoner@192.168.1.145: Permission denied (publickey).").contains("refused this PC's key"));
        assert_eq!(ssh_error(&t, "\nsomething odd\n\n"), "C2: something odd");
    }
}
