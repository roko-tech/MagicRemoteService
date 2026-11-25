//! The TV app built into the exe, and deploying it with the webOS CLI (ares).

use crate::log;
use crate::win::proc::{self, RunSpec};
use crate::win::Handle;
use serde_json::Value;
use std::net::Ipv4Addr;
use std::path::{Path, PathBuf};
use std::time::Duration;

macro_rules! tv_files {
    ($($path:literal),* $(,)?) => {
        &[$(($path, include_bytes!(concat!("../MagicRemoteService/Resources/TV/", $path)) as &[u8])),*]
    };
}

/// TV sources as `install.bat` packages them (`config.json` comes from `tv-config.json`).
static FILES: &[(&str, &[u8])] = tv_files![
    "MagicRemoteService/appinfo.json",
    "MagicRemoteService/appstring.json",
    "MagicRemoteService/index.html",
    "MagicRemoteService/main.js",
    "MagicRemoteService/icon.png",
    "MagicRemoteService/miniIcon.png",
    "MagicRemoteService/largeIcon.png",
    "MagicRemoteService/cursor.png",
    "MagicRemoteService/MuseoSans-Medium.ttf",
    "MagicRemoteService/webOSTVjs-1.2.12/webOSTV-dev.js",
    "MagicRemoteService/webOSTVjs-1.2.12/webOSTV.js",
    "MagicRemoteService/resources/fr/appinfo.json",
    "MagicRemoteService/resources/fr/appstring.json",
    "MagicRemoteService/resources/es/appinfo.json",
    "MagicRemoteService/resources/es/appstring.json",
    "Service/package.json",
    "Service/service.js",
    "Service/services.json",
];

static NODE_POLYFILL: &[u8] = include_bytes!("../MagicRemoteService/Resources/node-polyfill.js");
const TEMPLATE_APP_ID: &str = "com.cathwyler.magicremoteservice";
const ARES_TIMEOUT: Duration = Duration::from_secs(180);

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
                "MagicRemoteService/appinfo.json" => text()
                    .replace(&format!("\"id\": \"{TEMPLATE_APP_ID}\""), &format!("\"id\": \"{app_id}\""))
                    .replace("\"version\": \"1.0.0\"", &format!("\"version\": \"{version}\""))
                    .replace("\"appDescription\": \"HDMI\"", &format!("\"appDescription\": {}", Value::from(input_name)))
                    .into_bytes(),
                "Service/package.json" | "Service/services.json" => text().replace(&format!("{TEMPLATE_APP_ID}.service"), &service_id).into_bytes(),
                _ => data.to_vec(),
            };
            (path.to_string(), data)
        })
        .collect();
    out.push(("MagicRemoteService/config.json".into(), config.to_vec()));
    out.push(("Service/config.json".into(), config.to_vec()));
    out
}

#[derive(Debug, Clone)]
pub struct Device {
    pub name: String,
    pub default: bool,
    pub ip: Option<Ipv4Addr>,
}

/// Parses `ares-setup-device -F` output.
pub fn parse_devices(json: &str) -> Vec<Device> {
    let start = json.find('[').unwrap_or(0);
    let Ok(Value::Array(list)) = serde_json::from_str::<Value>(&json[start..]) else { return Vec::new() };
    list.iter()
        .filter_map(|d| {
            Some(Device {
                name: d.get("name")?.as_str()?.to_string(),
                default: d.get("default").and_then(Value::as_bool).unwrap_or(false),
                ip: d.get("deviceinfo").and_then(|i| i.get("ip")).and_then(Value::as_str).and_then(|s| s.parse().ok()),
            })
        })
        .collect()
}

/// Runs ares-cli as the person signed in at the PC: their device list, SSH keys and Node
/// install, and never with SYSTEM rights.
pub struct Ares {
    user: Option<Handle>,
    env: Vec<(String, String)>,
    scratch: PathBuf,
    node: Option<PathBuf>,
    polyfill: PathBuf,
}

impl Ares {
    pub fn new() -> Result<Ares, String> {
        let (user, mut env, profile) = if proc::is_system() {
            let token = proc::console_user_token()
                .map_err(|_| "No user is signed in at the PC. ares-cli runs with that user's device list and SSH key.".to_string())?;
            let env = proc::environment_of(&token).map_err(|e| format!("Cannot read the signed-in user's environment: {e}"))?;
            let profile = proc::profile_dir(&token);
            (Some(token), env, profile)
        } else {
            (None, std::env::vars().collect::<Vec<_>>(), std::env::var_os("USERPROFILE").map(PathBuf::from))
        };
        let get = |env: &[(String, String)], k: &str| env.iter().find(|(n, _)| n.eq_ignore_ascii_case(k)).map(|(_, v)| v.clone());
        // Where Node and the global npm packages usually live, ahead of PATH
        let mut dirs: Vec<PathBuf> = Vec::new();
        let program_files = get(&env, "ProgramFiles").unwrap_or_else(|| "C:\\Program Files".into());
        dirs.push(Path::new(&program_files).join("nodejs"));
        dirs.push(Path::new(&program_files).join("Volta"));
        if let Some(p) = &profile {
            dirs.push(p.join("AppData\\Roaming\\npm"));
            dirs.push(p.join(".volta\\bin"));
            if let Ok(rd) = std::fs::read_dir(p.join("AppData\\Roaming\\nvm")) {
                dirs.extend(rd.flatten().map(|e| e.path()).filter(|p| p.file_name().is_some_and(|n| n.to_string_lossy().starts_with('v'))));
            }
        }
        let path = dirs.iter().filter(|d| d.is_dir()).map(|d| d.to_string_lossy().into_owned()).chain(get(&env, "PATH")).collect::<Vec<_>>().join(";");
        env.retain(|(k, _)| !k.eq_ignore_ascii_case("PATH"));
        env.push(("PATH".into(), path.clone()));
        let scratch = get(&env, "TEMP").map(PathBuf::from).unwrap_or_else(std::env::temp_dir);
        let node = path.split(';').map(|d| Path::new(d).join("node.exe")).find(|p| p.is_file());
        let polyfill = scratch.join("MagicRemoteService-node-polyfill.js");
        std::fs::write(&polyfill, NODE_POLYFILL).map_err(|e| format!("Cannot write to {}: {e}", scratch.display()))?;
        Ok(Ares { user, env, scratch, node, polyfill })
    }

    pub fn scratch(&self) -> &Path {
        &self.scratch
    }

    /// Runs `command args`; through `node --require polyfill` when possible (Node 22+ removed
    /// an API ares-cli's SFTP needs), else through its .cmd shim.
    pub fn run(&self, command: &str, args: &str) -> Result<String, String> {
        let path = self.env.iter().find(|(k, _)| k == "PATH").map(|(_, v)| v.as_str()).unwrap_or_default();
        let script = path
            .split(';')
            .filter(|d| Path::new(d).join(format!("{command}.cmd")).is_file())
            .map(|d| Path::new(d).join("node_modules\\@webos-tools\\cli\\bin").join(format!("{command}.js")))
            .find(|p| p.is_file());
        let (program, line) = match (&self.node, script) {
            (Some(node), Some(js)) => {
                (node.clone(), format!("--require {} {} {args}", proc::quote(&self.polyfill.to_string_lossy()), proc::quote(&js.to_string_lossy())))
            }
            _ => (proc::system_dir().join("cmd.exe"), format!("/c {command} {args}")),
        };
        let spec = RunSpec {
            program: &program,
            args: &line,
            cwd: Some(&self.scratch),
            env: &self.env,
            user: self.user.as_ref(),
            timeout: ARES_TIMEOUT,
            scratch: &self.scratch,
        };
        let out = proc::run(&spec).map_err(|e| format!("{command}: {e}"))?;
        if out.code != 0 {
            let detail = if out.stderr.trim().is_empty() { out.stdout.trim() } else { out.stderr.trim() };
            return Err(if detail.is_empty() { format!("{command} exited with code {}", out.code) } else { detail.to_string() });
        }
        Ok(out.stdout)
    }

    pub fn devices(&self) -> Result<Vec<Device>, String> {
        Ok(parse_devices(&self.run("ares-setup-device", "-F")?))
    }

    /// The TV ares lists for this address, if any.
    pub fn device_at(&self, ip: Ipv4Addr) -> Option<Device> {
        self.devices().ok()?.into_iter().find(|d| d.ip == Some(ip))
    }
}

impl Drop for Ares {
    fn drop(&mut self) {
        let _ = std::fs::remove_file(&self.polyfill);
    }
}

/// Writes the TV sources into `build` and runs ares-package there. Returns the .ipk path.
fn package(ares: &Ares, build: &Path, app_id: &str, version: &str, input_name: &str, config: &[u8]) -> Result<PathBuf, String> {
    for (rel, data) in render(app_id, version, input_name, config) {
        let path = build.join(rel.replace('/', "\\"));
        std::fs::create_dir_all(path.parent().unwrap_or(build)).map_err(|e| e.to_string())?;
        std::fs::write(&path, data).map_err(|e| e.to_string())?;
    }
    let q = |p: &Path| proc::quote(&p.to_string_lossy());
    ares.run("ares-package", &format!("{} -n -o {} {}", q(&build.join("MagicRemoteService")), q(build), q(&build.join("Service"))))
        .map_err(|e| format!("ares-package failed: {e}"))?;
    let ipk = build.join(format!("{app_id}_{version}_all.ipk"));
    if ipk.is_file() {
        Ok(ipk)
    } else {
        Err(format!("ares-package did not produce {}", ipk.display()))
    }
}

/// Packages the built-in TV app with `tv-config.json`, installs it on ares' default TV and launches it.
pub fn reinstall(tv_config: &Path, version: &str) -> Result<String, String> {
    let config = std::fs::read(tv_config).map_err(|_| "No tv-config.json found. Save the TV config first.".to_string())?;
    let doc: Value = serde_json::from_slice(&config).map_err(|_| "tv-config.json is not valid JSON.".to_string())?;
    let app_id = doc.get("appId").and_then(Value::as_str).unwrap_or_default().to_string();
    if !valid_app_id(&app_id) {
        return Err("Invalid appId in tv-config.json.".into());
    }
    let input_name = doc.get("inputName").and_then(Value::as_str).unwrap_or("HDMI").to_string();
    let ares = Ares::new()?;
    let build = ares.scratch().join("mrs-webui-deploy");
    let _ = std::fs::remove_dir_all(&build);
    let result = (|| {
        let devices = ares.devices().map_err(|e| format!("ares-setup-device failed: {e}"))?;
        let device =
            devices.iter().find(|d| d.default).or(devices.first()).ok_or("No TV device configured in ares-cli. Add one with ares-setup-device.")?.name.clone();
        let ipk = package(&ares, &build, &app_id, version, &input_name, &config)?;
        let q = |p: &Path| proc::quote(&p.to_string_lossy());
        let dev = proc::quote(&device);
        ares.run("ares-install", &format!("-d {dev} {}", q(&ipk))).map_err(|e| format!("ares-install on {device} failed: {e}"))?;
        ares.run("ares-launch", &format!("-d {dev} {app_id}")).map_err(|e| format!("Installed {app_id} {version} on {device}, but ares-launch failed: {e}"))?;
        Ok(format!("Installed and launched {app_id} {version} on {device}."))
    })();
    // The build folder's config.json holds the device token, so never leave it behind
    if std::fs::remove_dir_all(&build).is_err() && build.exists() {
        log::warn("Could not remove the TV build folder");
    }
    result
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
        let appinfo: Value = serde_json::from_str(&get("MagicRemoteService/appinfo.json")).unwrap();
        assert_eq!(appinfo["id"], "com.cathwyler.magicremoteservice.hdmi4");
        assert_eq!(appinfo["version"], "3.0.0");
        assert_eq!(appinfo["appDescription"], "HDMI \"4\"");
        let services: Value = serde_json::from_str(&get("Service/services.json")).unwrap();
        assert_eq!(services["id"], "com.cathwyler.magicremoteservice.hdmi4.service");
        assert_eq!(services["services"][0]["name"], "com.cathwyler.magicremoteservice.hdmi4.service");
        assert!(get("Service/package.json").contains("\"com.cathwyler.magicremoteservice.hdmi4.service\""));
        assert_eq!(get("Service/config.json"), "{\"token\":\"x\"}");
        assert_eq!(files.len(), FILES.len() + 2);
    }

    #[test]
    #[ignore = "needs Node.js and @webos-tools/cli; never contacts a TV"]
    fn packages_the_tv_app_with_ares() {
        let ares = Ares::new().unwrap();
        let build = ares.scratch().join(format!("mrs-package-test-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&build);
        let ipk = package(&ares, &build, "com.cathwyler.magicremoteservice.hdmi9", "3.0.0", "HDMI 9", b"{}");
        let _ = std::fs::remove_dir_all(&build);
        let ipk = ipk.unwrap();
        assert_eq!(ipk.file_name().unwrap(), "com.cathwyler.magicremoteservice.hdmi9_3.0.0_all.ipk");
    }

    #[test]
    fn parses_ares_device_lists() {
        let out = r#"[{"profile":"tv","name":"emulator","default":false,"deviceinfo":{"ip":"127.0.0.1","port":"6622","user":"developer"}},
                     {"profile":"tv","name":"C2","default":true,"deviceinfo":{"ip":"192.168.8.145","port":"9922","user":"prisoner"}}]"#;
        let d = parse_devices(out);
        assert_eq!(d.len(), 2);
        assert_eq!((d[1].name.as_str(), d[1].default, d[1].ip), ("C2", true, Some(Ipv4Addr::new(192, 168, 8, 145))));
        assert!(parse_devices("not json").is_empty());
    }
}
