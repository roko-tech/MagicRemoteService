//! The TV app built into the exe, the TVs this PC deploys it to, and deploying it. The package is
//! built here and installed over the TV's Developer Mode SSH with the SSH client built into the
//! exe (`tvssh`), so nothing else is needed: no Node.js, no webOS CLI, no OpenSSH.

use crate::settings::{self, Files};
use crate::tvssh::{self, Session, SshError};
use crate::win::{acl, proc};
use crate::{ipk, log};
use serde_json::{json, Value};
use std::io::{self, Read, Write};
use std::net::{Ipv4Addr, SocketAddr, TcpStream};
use std::path::{Path, PathBuf};
use std::sync::{Mutex, MutexGuard};
use std::time::{Duration, SystemTime};

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
/// service, just SYSTEM and administrators): the keys and passphrases are secrets.
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

/// The host key remembered for the TV at `host:port`, as `ssh-rsa AAAA…`. The file is the
/// `known_hosts` OpenSSH kept for earlier versions, so the TVs they trusted stay pinned.
fn pinned_host_key(files: &Files, host: Ipv4Addr, port: u16) -> Option<String> {
    let text = std::fs::read_to_string(files.tv_known_hosts()).ok()?;
    let prefix = format!("[{host}]:{port} ");
    text.lines().find_map(|l| l.strip_prefix(&prefix)).map(|key| key.split_whitespace().take(2).collect::<Vec<_>>().join(" "))
}

/// Remembers the TV's host key; later sessions refuse a TV that shows another.
fn remember_host_key(files: &Files, host: Ipv4Addr, port: u16, key: &str) {
    if key.is_empty() {
        return;
    }
    forget_host(files, host, port);
    let path = files.tv_known_hosts();
    let mut text = std::fs::read_to_string(&path).unwrap_or_default();
    text.push_str(&format!("[{host}]:{port} {key}\n"));
    let _ = std::fs::write(&path, text);
}

/// Signs in to `tv` with the key at `key_path`, holding it to its remembered host key (and
/// remembering it the first time).
fn sign_in(files: &Files, tv: &Tv, key_path: &Path) -> Result<Session, String> {
    let key = std::fs::read_to_string(key_path).map_err(|_| format!("{}'s key is missing. Pair the TV again.", tv.name))?;
    let pinned = pinned_host_key(files, tv.host, tv.port);
    let session = tvssh::connect(tv.host, tv.port, &tv.username, &key, &tv.passphrase, pinned.as_deref()).map_err(|e| ssh_error(tv, &e))?;
    if pinned.is_none() {
        remember_host_key(files, tv.host, tv.port, &session.host_key);
    }
    Ok(session)
}

/// Runs a short command on the TV and returns its output; a failure becomes a readable error.
fn exec(session: &Session, tv: &Tv, command: &str, input: &[u8], timeout: Duration) -> Result<String, String> {
    let out = session.exec(command, input, false, timeout, |_| false).map_err(|e| ssh_error(tv, &e))?;
    if out.success() {
        return Ok(out.stdout);
    }
    let why = out.stderr.lines().map(str::trim).rfind(|l| !l.is_empty()).map(str::to_string);
    Err(format!("{}: {}", tv.name, why.unwrap_or_else(|| format!("the command ended with code {}", out.exit.map_or("?".into(), |c| c.to_string())))))
}

/// Calls an appInstallService method and follows its replies until it reports `done` or a
/// failure. Closing stdin then ends `luna-send -i`.
fn follow(session: &Session, tv: &Tv, method: &str, param: Value, done: &str) -> Result<(), String> {
    let mut reply = String::new();
    let mut outcome = None;
    let command = format!("/usr/bin/luna-send-pub -i luna://com.webos.appInstallService/dev/{method} '{param}'");
    let out = session
        .exec(&command, &[], true, Duration::from_secs(180), |line| {
            // A reply may span several lines
            reply.push_str(line);
            let Ok(v) = serde_json::from_str::<Value>(&reply) else { return false };
            reply.clear();
            outcome = luna_progress(&v, done);
            outcome.is_some()
        })
        .map_err(|e| ssh_error(tv, &e))?;
    match outcome {
        Some(Ok(())) => Ok(()),
        Some(Err(why)) => Err(format!("{} said: {why}", tv.name)),
        None if !out.stderr.trim().is_empty() => Err(format!("{}: {}", tv.name, out.stderr.trim())),
        None => Err(format!("{} didn't say how it went.", tv.name)),
    }
}

/// What went wrong, and what to do about it.
fn ssh_error(tv: &Tv, e: &SshError) -> String {
    let name = &tv.name;
    match e {
        SshError::WrongPassphrase => format!("The passphrase saved for {name} doesn't unlock its key. Pair the TV again."),
        SshError::BadKey => format!("{name}'s saved key can't be read. Pair the TV again."),
        SshError::Unreachable | SshError::TimedOut => format!(
            "Can't reach {name} at {}. Check the TV is on and its Developer Mode app shows Dev Mode Status on (it turns off when the session runs out).",
            tv.host
        ),
        SshError::HostKeyChanged => format!("{name} isn't the TV that was paired at {} (was it reset?). Pair it again.", tv.host),
        SshError::KeyRefused => format!("{name} refused this PC's key (reinstalling Developer Mode changes it). Pair the TV again."),
        SshError::Other(m) => format!("{name}: {m}"),
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

/// Downloads the TV's key, checks it unlocks with `passphrase` and opens an SSH session, and
/// saves the TV as the default one.
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
    // A TV paired again (after a reset, say) gets its host key trusted afresh
    let session = match tvssh::connect(host, tv.port, &tv.username, &String::from_utf8_lossy(&key), &tv.passphrase, None) {
        Ok(session) => session,
        Err(SshError::WrongPassphrase) => {
            return Err("Wrong passphrase: enter the one the TV's Developer Mode app shows (it changes when Developer Mode is reinstalled).".into());
        }
        Err(e) => return Err(ssh_error(&tv, &e)),
    };
    if exec(&session, &tv, "echo paired", &[], Duration::from_secs(30))?.trim() != "paired" {
        return Err(format!("{name} didn't answer as expected."));
    }
    remember_host_key(files, host, tv.port, &session.host_key);
    drop(session);
    write_secret(&files.tv_key(&name), &key).map_err(|e| format!("Cannot save the TV's key: {e}"))?;
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
fn install(session: &Session, tv: &Tv, app_id: &str, version: &str, package: &[u8]) -> Result<(), String> {
    let remote = format!("/media/developer/temp/{app_id}_{version}_all.ipk");
    // The byte count it reports back confirms the package arrived whole
    let size = package.len();
    let copied = exec(session, tv, &format!("mkdir -p /media/developer/temp && cat > {remote} && wc -c < {remote}"), package, Duration::from_secs(120))?;
    let result = if copied.trim() == size.to_string() {
        follow(session, tv, "install", json!({ "id": "com.ares.defaultName", "ipkUrl": remote, "subscribe": true }), "installed")
    } else {
        Err(format!("Copying the TV app to {} failed ({} of {size} bytes arrived).", tv.name, copied.trim()))
    };
    // The package holds the device token: remove it whatever happened
    let _ = exec(session, tv, &format!("rm -f {remote}"), &[], Duration::from_secs(30));
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
    let session = sign_in(files, tv, &files.tv_key(&tv.name))?;
    let now = SystemTime::now().duration_since(SystemTime::UNIX_EPOCH).map_or(0, |d| d.as_secs());
    install(&session, tv, &app_id, version, &ipk::build(&app_id, version, &render(&app_id, version, input_name, &config), now))?;
    let name = &tv.name;
    let launch = json!({ "id": app_id, "subscribe": false, "params": {} });
    let out = exec(&session, tv, &format!("/usr/bin/luna-send-pub -n 1 luna://com.webos.applicationManager/launch '{launch}'"), &[], Duration::from_secs(30))?;
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
    #[ignore = "installs a throwaway copy of the TV app on the TV set up in the webOS CLI, then removes it"]
    fn installs_and_removes_on_a_real_tv() {
        const ID: &str = "com.cathwyler.magicremoteservice.hdmi9";
        struct Cleanup(PathBuf);
        impl Drop for Cleanup {
            fn drop(&mut self) {
                let _ = std::fs::remove_dir_all(&self.0);
            }
        }
        // Laid out like the service's files, in a folder with spaces in its name
        let files = Files { dir: std::env::temp_dir().join(format!("mrs tv real {}", std::process::id())) };
        std::fs::create_dir_all(&files.dir).unwrap();
        let _cleanup = Cleanup(files.dir.clone());
        let tvs = tvs(&files);
        let tv = tvs.iter().find(|t| t.default).expect("a TV set up with ares-setup-device and ares-novacom --getkey");
        let session = sign_in(&files, tv, &files.tv_key(&tv.name)).unwrap();
        assert!(pinned_host_key(&files, tv.host, tv.port).is_some(), "the host key is remembered");
        let now = SystemTime::now().duration_since(SystemTime::UNIX_EPOCH).unwrap().as_secs();
        install(&session, tv, ID, "0.0.1", &ipk::build(ID, "0.0.1", &render(ID, "0.0.1", "HDMI 9", b"{}"), now)).unwrap();
        let info = exec(&session, tv, "/usr/bin/luna-send-pub -n 1 luna://com.webos.applicationManager/dev/listApps '{}'", &[], Duration::from_secs(30));
        follow(&session, tv, "remove", json!({ "id": ID, "subscribe": true }), "removed").unwrap();
        let info = info.unwrap();
        let listed: Value = serde_json::from_str(info.trim()).unwrap();
        let app = listed["apps"].as_array().unwrap().iter().find(|a| a["id"] == ID).cloned().unwrap_or_default();
        assert_eq!((app["version"].as_str(), app["title"].as_str()), (Some("0.0.1"), Some("MagicRemoteService")), "{info}");
        assert!(!exec(&session, tv, "ls /media/developer/temp", &[], Duration::from_secs(30)).unwrap().contains(ID), "the package was left on the TV");
    }

    #[test]
    #[ignore = "signs in to the TV set up in the webOS CLI with the built-in SSH client"]
    fn built_in_ssh_reaches_a_real_tv() {
        use crate::tvssh::connect;
        use std::time::Instant;
        struct Cleanup(PathBuf);
        impl Drop for Cleanup {
            fn drop(&mut self) {
                let _ = std::fs::remove_dir_all(&self.0);
            }
        }
        let files = Files { dir: std::env::temp_dir().join(format!("mrs ssh spike {}", std::process::id())) };
        std::fs::create_dir_all(&files.dir).unwrap();
        let _cleanup = Cleanup(files.dir.clone());
        let tvs = tvs(&files);
        let tv = tvs.iter().find(|t| t.default).expect("a TV set up with ares-setup-device and ares-novacom --getkey");
        let key = std::fs::read_to_string(files.tv_key(&tv.name)).unwrap();
        let started = Instant::now();
        let session = connect(tv.host, tv.port, &tv.username, &key, &tv.passphrase, None).unwrap();
        println!("signed in in {:?}; host key {}...", started.elapsed(), &session.host_key[..24]);
        let out = session.exec("echo ok; echo err >&2; exit 3", &[], false, Duration::from_secs(20), |_| false).unwrap();
        assert_eq!((out.stdout.trim(), out.stderr.trim(), out.exit), ("ok", "err", Some(3)));
        // Input on stdin, as the package copy sends it
        let out = session.exec("wc -c", &[7u8; 100_000], false, Duration::from_secs(30), |_| false).unwrap();
        assert_eq!(out.stdout.trim(), "100000");
        let pinned = session.host_key.clone();
        drop(session);
        assert_eq!(connect(tv.host, tv.port, &tv.username, &key, "WRONG1", None).err(), Some(SshError::WrongPassphrase));
        assert!(connect(tv.host, tv.port, &tv.username, &key, &tv.passphrase, Some(&pinned)).is_ok());
        assert_eq!(connect(tv.host, tv.port, &tv.username, &key, &tv.passphrase, Some("ssh-rsa AAAAnotthisone")).err(), Some(SshError::HostKeyChanged));
    }

    #[test]
    #[ignore = "pairs the TV set up in the webOS CLI again, from scratch (Key Server must be on)"]
    fn pairs_with_a_real_tv() {
        let cli = temp_files("pair-cli");
        let tvs = tvs(&cli);
        let known = tvs.iter().find(|t| t.default).expect("a TV set up with ares-setup-device and ares-novacom --getkey").clone();
        let fresh = temp_files("pair-fresh");
        let wrong = pair(&fresh, "", known.host, "WRONG1").unwrap_err();
        assert!(!wrong.contains("key server"), "turn on Key Server in the TV's Developer Mode app: {wrong}");
        assert!(wrong.starts_with("Wrong passphrase"), "{wrong}");
        let paired = pair(&fresh, "", known.host, &known.passphrase);
        let _ = std::fs::remove_dir_all(&cli.dir);
        let saved = load(&fresh);
        let pinned = pinned_host_key(&fresh, known.host, DEV_SSH_PORT);
        let _ = std::fs::remove_dir_all(&fresh.dir);
        println!("{paired:?}");
        assert!(paired.is_ok());
        assert_eq!(saved.unwrap()[0].host, known.host);
        assert!(pinned.is_some_and(|k| k.starts_with("ssh-rsa ")));
    }

    #[test]
    fn ssh_errors_say_what_to_do() {
        let t = tv("C2", 145, true);
        assert!(ssh_error(&t, &SshError::Unreachable).contains("Dev Mode Status"));
        assert!(ssh_error(&t, &SshError::KeyRefused).contains("refused this PC's key"));
        assert!(ssh_error(&t, &SshError::HostKeyChanged).contains("Pair it again"));
        assert_eq!(ssh_error(&t, &SshError::Other("something odd".into())), "C2: something odd");
    }

    #[test]
    fn host_keys_pinned_by_openssh_still_hold() {
        let files = temp_files("pins");
        let tv_ip = Ipv4Addr::new(192, 168, 1, 145);
        // A line as Windows' OpenSSH wrote it for earlier versions
        std::fs::write(files.tv_known_hosts(), "[192.168.1.145]:9922 ssh-rsa AAAAold\n[192.168.1.146]:9922 ssh-rsa AAAAother\n").unwrap();
        assert_eq!(pinned_host_key(&files, tv_ip, 9922).as_deref(), Some("ssh-rsa AAAAold"));
        assert_eq!(pinned_host_key(&files, tv_ip, 22), None);
        remember_host_key(&files, tv_ip, 9922, "ssh-rsa AAAAnew");
        assert_eq!(pinned_host_key(&files, tv_ip, 9922).as_deref(), Some("ssh-rsa AAAAnew"));
        assert_eq!(pinned_host_key(&files, Ipv4Addr::new(192, 168, 1, 146), 9922).as_deref(), Some("ssh-rsa AAAAother"));
        forget_host(&files, tv_ip, 9922);
        assert_eq!(pinned_host_key(&files, tv_ip, 9922), None);
        let _ = std::fs::remove_dir_all(&files.dir);
    }
}
