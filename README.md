# MagicRemoteService

[![Build](https://github.com/roko-tech/MagicRemoteService/actions/workflows/build.yml/badge.svg)](https://github.com/roko-tech/MagicRemoteService/actions/workflows/build.yml)

Control your Windows PC with the LG Magic Remote — point, click, scroll, and type from your couch.

Forked from [Cathwyler/MagicRemoteService](https://github.com/Cathwyler/MagicRemoteService). Tested on LG C2 (webOS 25), LG C1 (webOS 6.0), Windows 10/11.

## Install

### Automated (recommended)

```
git clone https://github.com/roko-tech/MagicRemoteService.git
cd MagicRemoteService
install.bat
```

Run as **Administrator**. The installer handles everything:
- Installs all prerequisites automatically (Node.js, webOS CLI, Build Tools, .NET 4.7.2)
- Builds and registers the PC service
- Auto-detects your PC's IP, MAC, and subnet
- Deploys the TV app to your LG TV
- Adds firewall rules

**Only prerequisite:** [Developer Mode](https://webostv.developer.lge.com/develop/getting-started/developer-mode-app) enabled on your TV.

### Manual

1. Download from [Releases](https://github.com/roko-tech/MagicRemoteService/releases) and extract
2. Install [Node.js](https://nodejs.org/) and run `npm install -g @webos-tools/cli`
3. Register the service (admin command prompt):
   ```
   sc create MagicRemoteService binPath= "C:\MagicRemoteService\MagicRemoteService.exe" start= auto
   net start MagicRemoteService
   netsh advfirewall firewall add rule name="MagicRemoteService" dir=in action=allow protocol=TCP localport=41230
   netsh advfirewall firewall add rule name="MagicRemoteService Web" dir=in action=allow protocol=TCP localport=41231
   ```
4. Run `MagicRemoteService.exe` — configure TV in the Settings UI → click **Install on TV**
5. Open http://localhost:41231 to configure key bindings

## Usage

| Action | How |
|--------|-----|
| Move cursor | Point and wave the Magic Remote |
| Left click | Short press the OK/wheel button |
| Right click | Long press the OK/wheel button |
| Scroll | Roll the scroll wheel (smooth momentum) |
| Arrow keys | Press the D-pad directions |
| Enter | Press OK |
| Escape | Press Back |
| Copy | Volume + |
| Paste | Volume - |
| Shutdown PC | Press Red button |
| Open Windows menu | Press Green button |
| Toggle TV keyboard | Press Blue button |
| Numbers 0-9 | Press number pad |
| Exit TV app | Double-press Back (within 500ms) |

All mappings are customizable via the web UI.

## Web Settings

Open **http://localhost:41231** while the service is running.

### Visual Remote Configurator

Click any button on the visual LG remote to remap it:
- **Mouse click** — left, right, middle
- **Keyboard key** — Enter, Escape, F1-F12, A-Z, arrows, etc.
- **Keyboard shortcut** — Ctrl+C, Alt+Tab, Win+D, any combo
- **Shell command** — launch any program
- **Special action** — shutdown PC, toggle TV keyboard

### TV App Configuration

- **HDMI port** — auto-detected at runtime, or select manually
- **PC IP / MAC / Subnet** — auto-detected from your network interfaces
- **Cursor speed** — adjustable from 0.1 (slow) to 5.0 (fast)
- **Reinstall TV App** — one-click deploy without command line

### Smooth Scroll Exclusions

Some apps (like media players) use scroll for volume control. Add their process names to disable momentum scrolling — sends single steps instead.

### Visual Remote Tooltips

Hover any button on the visual remote to see its current binding (e.g. "Green → Win" or "Red → Shutdown PC"). Green dots indicate custom (non-default) bindings.

### Reset All Bindings

A single button at the bottom of the config panel resets all buttons to their default bindings (with confirmation).

## How It Works

```
LG Magic Remote → TV App (webOS) → WebSocket → PC Service (Windows) → Mouse/Keyboard
  gyroscope        captures input    port 41230   translates to          controls your
  buttons          sends binary                   Win32 SendInput        desktop
  scroll wheel     messages
```

**Two processes on the PC:**
- **Service** runs as SYSTEM — starts at boot, accepts network connections
- **Client** runs in your user session — injects mouse/keyboard input via SendInput

**TV app configuration** is stored in `config.json` — no JavaScript files are modified during installation. The HDMI port is auto-detected at runtime.

## Key Improvements Over Original

- **Config injection** — `config.json` replaces fragile sed/PowerShell string replacement
- **Auto-discovery** — TV finds PC via SSDP, no manual IP entry needed
- **Auto-detect HDMI** — switches to the correct port automatically
- **Web settings UI** — visual remote configurator at localhost:41231
- **Smooth scrolling** — Mac-style momentum with per-app exclusions
- **Non-blocking toasts** — small pill notifications instead of full-screen dialogs
- **CPU fixed** — reduced from 20%+ to near 0% (timer + WebSocket spin fixes)
- **Security hardened** — buffer overflow protection, command injection blocking, XSS prevention, pipe access control, CSRF headers
- **Connection state machine** — prevents race conditions during connect/disconnect
- **Node.js v22+** — polyfill for ares-cli compatibility
- **Service keepalive** — prevents webOS 25 from killing the TV service
- **One-click install** — `install.bat` handles all prerequisites

## Keyboard Shortcuts in Web UI

- **Tab** — navigate between remote buttons
- **Enter / Space** — select focused button
- **Enter** in scroll exclusion input — add to list

## Troubleshooting

| Problem | Solution |
|---------|----------|
| "Service is busy" or "not running" | Reboot TV, then relaunch the app |
| Pointer doesn't work but scroll does | Wave the remote at the TV to activate the gyroscope. It auto-retries every 3 seconds |
| "Failed to get sensor data [1003]" | Normal on webOS 25 — auto-retries in the background |
| TV can't find PC | Ensure same subnet, UDP multicast not blocked. Fall back to manual IP in config |
| Web UI won't load | Open http://localhost:41231 (not 41230). The PC service must be running |
| Settings UI says "ares not found" | Install Node.js and run `npm install -g @webos-tools/cli` |
| High CPU usage | Update to v2.2.0+ — fixes timer polling and WebSocket spin bugs |
| Home launcher pointer blocked | Expected in overlay mode — use D-pad to navigate the TV launcher |
| TV app shows after long delay | Normal on cold boot — webOS takes ~60s to initialize developer apps |
| Can't exit TV app | Double-press the Back button quickly (within 500ms) |
| Forgot what a button does | Hover it in the web UI — shows current binding |

## Building from Source

```
git clone https://github.com/roko-tech/MagicRemoteService.git
cd MagicRemoteService
build.bat
```

Requires Visual Studio 2022 (or Build Tools) and .NET Framework 4.7.2.

## Security

- **Local network only** — no encryption between TV and PC
- **Buffer overflow protection** — WebSocket frame bounds checking
- **Command injection blocking** — shell metacharacters rejected in key bindings
- **XSS prevention** — user input sanitized in web UI
- **Pipe access control** — named pipe restricted to authenticated users
- **CSRF headers** — POST requests require X-Requested-With header

Don't use on untrusted networks. Don't enter passwords via the remote.

## License

GPL-3.0 — see [LICENSE](LICENSE)
