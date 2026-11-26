# MagicRemoteService

[![Build](https://github.com/roko-tech/MagicRemoteService/actions/workflows/build.yml/badge.svg)](https://github.com/roko-tech/MagicRemoteService/actions/workflows/build.yml)

Control your Windows PC with the LG Magic Remote — point, click, scroll, and type from your couch.

Forked from [Cathwyler/MagicRemoteService](https://github.com/Cathwyler/MagicRemoteService). Tested on LG C2 (webOS 25), LG C1 (webOS 6.0), Windows 10/11.

## Install

You need:
- [Developer Mode](https://webostv.developer.lge.com/develop/getting-started/developer-mode-app) on the TV, with the TV added to the webOS CLI on the PC (the installer explains how when it finds none);
- [Node.js](https://nodejs.org/) and `@webos-tools/cli` on the PC to deploy the TV app (the installer offers to install both).

### From a release (recommended)

1. Download `MagicRemoteService-<version>.zip` from [Releases](https://github.com/roko-tech/MagicRemoteService/releases) and extract it.
2. Double-click `install.bat` and accept the administrator prompt.

### From source

Install [Rust](https://rustup.rs) (MSVC toolchain), then:

```
git clone https://github.com/roko-tech/MagicRemoteService.git
cd MagicRemoteService
install.bat
```

`install.bat` builds the exe with `cargo` when `target\release\MagicRemoteService.exe` doesn't exist yet.

### What the installer does

1. Copies `MagicRemoteService.exe` to `C:\Program Files\MagicRemoteService` and registers the Windows service there, restarting it automatically if it fails. It also adds the firewall rules and starts it. Settings from an earlier version (`bindings.json`, `tv-config.json` and the registry) are kept.
2. Asks which network the TV is on and which HDMI input the PC is plugged into, generates a device token if there is none, and installs and launches the TV app on the webOS CLI's default TV.

Choose "PC service only" or "TV app only" in its menu, or pass `-Part Pc|Tv` and `-Hdmi 1-4` (for example `install.bat -Part Tv -Hdmi 4`).

To update, run the installer of the new version. To uninstall, run `"C:\Program Files\MagicRemoteService\MagicRemoteService.exe" uninstall` from an administrator prompt. Then remove the TV app with `ares-install --device <tv> --remove com.cathwyler.magicremoteservice.hdmi<N>`.

## Usage

| Action | How |
|--------|-----|
| Move cursor | Point and wave the Magic Remote |
| Left click | Short press the OK/wheel button |
| Right click | Long press the OK/wheel button, or press Yellow |
| Scroll | Roll the scroll wheel (smooth momentum) |
| Arrow keys | Press the D-pad directions |
| Enter | Press OK |
| Escape | Press Back |
| Copy | Press CH + |
| Paste | Press CH - |
| Shutdown PC | Press Red button |
| Open Windows menu | Press Green button |
| Toggle TV keyboard | Press Blue button |
| Numbers 0-9 | Press number pad |
| Media keys | On remotes that have them: Play toggles play/pause, Stop stops, Fast-forward and Rewind skip to the next and previous track, and Pause sends the keyboard Pause key |
| Exit TV app | Hold Back button for 1 second |

All mappings are customizable via the web UI.

The remote's volume buttons stay with the TV while Windows plays through it (HDMI, DisplayPort, or S/PDIF, such as the TV or a soundbar on its eARC). While Windows plays on another device, such as headphones, they change the PC volume instead, and the TV app sets the TV volume back after each step. This works while the TV app shows the PC input.

When the PC is off, opening the TV app (or switching back to the PC's input) sends Wake-on-LAN for 15 seconds. Shutting the PC down never wakes it again by itself.

## Web Settings

Open **http://localhost:41231** on the PC. It is only reachable from the PC itself.

- **Remote configurator** — click any button on the visual remote to map it to a mouse click, a key, a shortcut (Ctrl+C, Alt+Tab, Win+D…), a shell command, or a special action (shut down the PC, toggle the TV keyboard). Bindings apply as soon as they're saved. Hover a button to see what it does; green dots mark custom bindings. "Reset All Bindings" restores the defaults.
- **Per-app profiles** — under "Bindings for", create a profile for some apps (their process names, as in Task Manager's Details tab, such as `vlc`). Buttons you change in it act differently while one of those apps is in front; the others keep their normal binding.
- **Smooth scroll exclusions** — apps (such as media players) that use the wheel for volume get single scroll steps instead of momentum. Add their process names.
- **TV app configuration** — the HDMI input the PC is on, the PC's IP, MAC (for Wake-on-LAN) and subnet, the port and the cursor speed. **Reinstall TV App** packages the TV app built into the service with this configuration, then installs and launches it on the webOS CLI's default TV. The CLI runs as the user signed in at the PC.
- **PC** — the TV port, start with Windows, and the shutdown timeouts:
  - **Inactivity:** with the TV connected and no remote input for that long, Windows shuts down. Using the PC in the last five minutes cancels it; the remote counts.
  - **Monitor off:** the same countdown, starting when the PC's monitor turns off.
- **Pointer display** — with several monitors, pick the one each TV shows so its pointer lands there.
- **Recent events** — what the service logged: TVs connecting and leaving, refused connections, saved settings.

Keyboard: **Tab** moves between remote buttons, **Enter/Space** selects one, **Enter** in the exclusion box adds the app.

## Commands

| Command | What it does |
|---------|--------------|
| `MagicRemoteService.exe install` | Copy this exe to `C:\Program Files\MagicRemoteService` and install or update the service from there (administrator) |
| `MagicRemoteService.exe uninstall` | Stop and remove the service (administrator) |
| `MagicRemoteService.exe status` | Service state, port and connected TVs |
| `MagicRemoteService.exe --port 41330 --no-ssdp` | Run a second copy on its own beside the service, to try a build |

These are window-less programs: in PowerShell, pipe them (`… status | Out-Host`) to see their output.

## How It Works

```
LG Magic Remote → TV App (webOS) → WebSocket → PC Service (Windows) → Mouse/Keyboard
  gyroscope        captures input    port 41230   translates to          controls your
  buttons          sends binary                   Win32 SendInput        desktop
  scroll wheel     messages
```

**On the PC**, one exe runs as two processes:
- the **service**, in session 0 as SYSTEM, starts at boot and keeps the worker running, restarting it after a crash or when another session takes the console;
- the **worker**, in the console session as SYSTEM, does the actual work. It listens for the TV on TCP 41230, answers the TV's network search on UDP 1900, serves the web UI on port 41231 (this PC only), and injects input with `SendInput`.

**Settings** live in `C:\Program Files\MagicRemoteService` (`bindings.json`, `tv-config.json`) and in `HKLM\SOFTWARE\WOW6432Node\MagicRemoteService` (port, timeouts, per-TV display). **Logs** go to Event Viewer (Windows Logs → Application, source MagicRemoteService) and to the web UI's Recent Events.

**On the TV**, the web app (`tv/app`) captures the remote and talks to the PC. A small JS service (`tv/service`) finds the PC on the network, sends Wake-on-LAN, and relaunches the app when the PC input is selected. The app reads its settings from a `config.json` packaged from `tv-config.json`.

## Troubleshooting

| Problem | Solution |
|---------|----------|
| "Service is busy" or "not running" | Reboot TV, then relaunch the app |
| Pointer doesn't work but scroll does | Wave the remote at the TV to activate the gyroscope. It auto-retries every 3 seconds |
| "Failed to get sensor data [1003]" | Normal on webOS 25 — auto-retries in the background |
| TV can't reach the PC after its IP changed | Run the installer again ("TV app only"), or update the IP in the web UI, Save Config and Reinstall TV App. A DHCP reservation in your router stops it changing |
| TV keeps retrying; Recent Events shows "invalid or missing token" | The TV app carries an old token: click Reinstall TV App |
| Web UI won't load | Open http://localhost:41231 on the PC itself. Check `MagicRemoteService.exe status` |
| Reinstall TV App says "No user is signed in" | Sign in at the PC: the webOS CLI runs with that user's TV list and keys |
| Reinstall TV App can't find `ares-…` or Node | Install Node.js and run `npm install -g @webos-tools/cli` as the user signed in at the PC |
| Worried about burn-in on an OLED TV | The TV app keeps the TV's screensaver off while the PC is connected, so a video never gets interrupted. Set Windows to turn off the display after a few minutes (Settings › System › Power): it stays on while a video plays, and an idle desktop then goes dark |
| TV says "Disconnected" a few seconds after the PC sleeps | Expected: the PC sends a heartbeat every 5 seconds, and the TV reconnects when it wakes |
| Home launcher pointer blocked | Expected in overlay mode — use D-pad to navigate the TV launcher |
| TV app shows after long delay | Normal on cold boot — webOS takes ~60s to initialize developer apps |
| Can't exit TV app | Press and hold the Back button for 1 second |
| Forgot what a button does | Hover it in the web UI — shows current binding |

## Building from Source

Rust 1.85 or newer with the MSVC toolchain (`rustup` installs it; the Windows SDK supplies the resource compiler).

```
cargo test
cargo build --release
```

The exe is `target\release\MagicRemoteService.exe`. `cargo test -- --ignored` also runs two tests that need this PC's hardware and tools: one reads the default audio device; the other packages the TV app with the webOS CLI, without contacting a TV.

Layout: `src/` is the PC service (`src/win/` holds the Windows calls), `tv/app` and `tv/service` the webOS app and service, `resources/` the web UI, icon, cursor and manifest built into the exe. Pushes and pull requests run formatting, lint and tests in CI; pushing a `v*` tag publishes a release.

## Security

- **Device token** — the TV presents a shared token in the WebSocket URL, and connections without it are refused, so other devices on your LAN can't drive the PC. The installer generates it; **Generate new token** in the web UI replaces it. Every TV must carry the same token, so reinstall a TV's app after the token changes. The service keeps the token to itself: the API only reports whether one is set, and `tv-config.json` is readable only by SYSTEM and administrators. With no token the service runs in open mode, anything on the LAN can control the PC, and the web UI shows a red "No device token" warning.
  - *What it protects against:* blind LAN devices and malicious web pages, which can neither read it cross-origin nor sniff your network. *What it does **not**:* the token travels in cleartext over `ws://` (no TLS), so a sniffer on your network segment could capture and replay it. It raises the bar within a trusted LAN; it is not a substitute for one.
- **Local network only** — no encryption between TV and PC. The firewall rules let the TV in on networks Windows treats as Private or Domain, never Public ones.
- **Service location** — the service runs as SYSTEM from `C:\Program Files\MagicRemoteService`, where only administrators can change files. Never point it at a folder ordinary accounts can write to.
- **Least privilege** — shell-command bindings, the webOS CLI and the browser run as the user signed in at the PC, never as SYSTEM.
- **Web UI and API** — loopback only. Requests with other `Host` names (DNS rebinding) or foreign origins are refused. Changes (POST) need an `X-Requested-With` header, which cross-site pages can't send; send it if you script the API. Any program running on the PC can still use the API: it can change bindings and settings, but it can't read the token.
- **WebSocket** — upgrades from web pages (an `http` or `https` `Origin`) are refused, so a page open in any browser can't drive the PC even in open mode. Frame sizes are capped, the handshake must finish within 10 seconds, at most 16 connections are open at once, and the token is compared in constant time. Refused connections are logged at most once a minute per address.
- **Command bindings** — `& | ; < >` are refused so a binding can't chain commands, but a binding still runs whatever program it names.

Don't enter passwords via the remote.

## History

- **3.0** — the PC service rebuilt in Rust: one exe with no .NET dependency; a supervisor that restarts its worker; settings that apply without a restart; PC settings, pointer display and an event log in the web UI. It also fixes mouse speed not being restored, Wake-on-LAN waking the PC right after a shutdown, and reconnect timers stacking up. See [Releases](https://github.com/roko-tech/MagicRemoteService/releases).
- **2.x** — the C# version: `config.json` instead of patching JavaScript at install, SSDP discovery, the web UI, smooth scrolling with per-app exclusions, non-blocking toasts, CPU use near zero, a keepalive for the webOS 25 TV service, a Node.js 22 polyfill for the webOS CLI, and device token auth.

## License

GPL-3.0 — see [LICENSE](LICENSE)
