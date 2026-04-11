# MagicRemoteService

[![Build](https://github.com/roko-tech/MagicRemoteService/actions/workflows/build.yml/badge.svg)](https://github.com/roko-tech/MagicRemoteService/actions/workflows/build.yml)

Control your Windows PC with the LG Magic Remote. Point, click, scroll, and type from your couch.

Forked from [Cathwyler/MagicRemoteService](https://github.com/Cathwyler/MagicRemoteService) — tested on LG C2 (webOS 25), LG C1 (webOS 6.0), Windows 10/11.

## Quick Start

### Prerequisites

- LG TV with **Developer Mode** enabled ([guide](https://webostv.developer.lge.com/develop/getting-started/developer-mode-app))
- PC connected to TV via HDMI, both on the same network
- [Node.js](https://nodejs.org/) installed on PC
- webOS CLI: `npm install -g @webos-tools/cli`

### Install

```
git clone https://github.com/roko-tech/MagicRemoteService.git
cd MagicRemoteService
install.bat
```

Run as Administrator. The installer:
- Auto-installs missing prerequisites (Build Tools, .NET 4.7.2) via winget
- Builds the PC service and registers it as a Windows service
- Auto-detects your PC's IP, MAC, and subnet
- Deploys the TV app to your LG TV
- Adds firewall rules and startup shortcut

### Manual Install

Download from [Releases](https://github.com/roko-tech/MagicRemoteService/releases), extract, and follow the [manual setup steps](#manual-setup).

## Web Settings UI

Once running, open **http://localhost:41231** in any browser.

### Visual Remote Configurator
Click any button on the visual remote to change what it does:
- **Mouse click** — left, right, middle
- **Keyboard key** — any key from A-Z, F1-F12, arrows, etc.
- **Keyboard shortcut** — Ctrl+C, Alt+Tab, Win+D, etc.
- **Shell command** — launch any program
- **Special action** — shutdown PC, toggle TV keyboard

### TV App Configuration
Auto-detects your PC's network info:
- **HDMI port** — select which input your PC is on
- **PC IP / MAC / Subnet** — auto-populated, select from dropdown
- **Cursor speed** — adjustable (0.1 slow to 5.0 fast)
- **Save Config** — saves without reinstalling
- **Reinstall TV App** — one-click deploy to TV

### Per-App Scroll Exclusions
Disable smooth scrolling for specific apps (e.g. PotPlayer volume control). Add process names in the web UI — scroll sends single steps instead of momentum.

## Features

| Feature | Description |
|---------|-------------|
| Pointer control | Gyroscope-based cursor movement |
| Click | Short press = left click, long press = right click |
| Smooth scroll | Mac-style momentum with friction decay |
| Keyboard | Arrow keys, Enter, Escape, numbers, and more |
| Wake-on-LAN | Turn on PC from TV |
| Auto-discovery | TV finds PC via SSDP — no manual IP needed |
| Config injection | `config.json` loaded at runtime — no JS file editing |
| Web UI | Full settings at http://localhost:41231 |
| Connection toasts | Small non-blocking notifications on TV |
| Service keepalive | Prevents webOS 25 from killing the TV service |
| Node.js v22+ | Polyfill for ares-cli compatibility |
| Low CPU | Near 0% idle (fixed timer + WebSocket spin bugs) |

## Default Key Mappings

| Remote Button | PC Action |
|---------------|-----------|
| Pointer | Mouse cursor |
| Short click | Left click |
| Long click | Right click |
| Scroll wheel | Smooth scroll |
| Arrow keys | Arrow keys |
| OK | Enter |
| Back | Escape |
| Red | Shutdown / Wake PC |
| Green | Windows menu |
| Yellow | Right click |
| Blue | Toggle TV keyboard |
| Volume + | Ctrl+C (Copy) |
| Volume - | Ctrl+V (Paste) |
| 0-9 | NumPad 0-9 |

All mappings customizable via the [web UI](#visual-remote-configurator).

## How It Works

```
Magic Remote  →  TV App (main.js)  →  WebSocket  →  PC Service (Service.cs)  →  SendInput  →  Windows
  gyroscope       loads config.json     port 41230    two-process architecture    mouse/keyboard
  buttons          from TV storage                     Service (SYSTEM) + Client (User session)
```

The PC runs two processes:
- **Service** (SYSTEM) — accepts WebSocket connections at boot
- **Client** (user session) — injects input via SendInput

Config is stored in `config.json` next to the TV app files. No JavaScript files are modified during installation.

## Manual Setup

1. Download the [latest release](https://github.com/roko-tech/MagicRemoteService/releases)
2. Extract to a folder (e.g. `C:\MagicRemoteService`)
3. Open an **admin** command prompt:
   ```
   C:\Windows\Microsoft.NET\Framework64\v4.0.30319\InstallUtil.exe MagicRemoteService.exe
   net start MagicRemoteService
   netsh advfirewall firewall add rule name="MagicRemoteService" dir=in action=allow protocol=TCP localport=41230
   netsh advfirewall firewall add rule name="MagicRemoteService Web" dir=in action=allow protocol=TCP localport=41231
   ```
4. Run `MagicRemoteService.exe` — Settings UI opens
5. **TVs tab**: Refresh TVs or Add TV, select HDMI input, click **Install on TV**
6. **PC tab**: Check "Automatically launch at startup", click Save
7. Open http://localhost:41231 to configure key bindings

## Building from Source

```
git clone https://github.com/roko-tech/MagicRemoteService.git
cd MagicRemoteService
build.bat
```

Requires Visual Studio 2022 (or Build Tools) and .NET Framework 4.7.2.

## Troubleshooting

| Problem | Solution |
|---------|----------|
| "Service is busy" / "not running" | Reboot TV, relaunch app |
| Pointer doesn't work, scroll does | Wave the remote at TV to activate gyroscope (auto-retries every 3s) |
| Sensor error 1003 | Normal on webOS 25, auto-retries |
| TV can't find PC | Check same subnet, UDP multicast not blocked. Use manual IP |
| Web UI not loading | Open http://localhost:41231 (not 41230). Service must be running |
| High CPU | Update to latest version |
| Settings UI "ares not found" | Install Node.js and `npm install -g @webos-tools/cli` |
| Install fails on second run | No longer an issue — config.json replaces sed/PowerShell injection |

## Security

No encryption between TV and PC. Local network only. Don't enter passwords or sensitive data via the remote.

## License

GPL-3.0 — see [LICENSE](LICENSE)
