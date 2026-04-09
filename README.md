# MagicRemoteService

[![Build](https://github.com/roko-tech/MagicRemoteService/actions/workflows/build.yml/badge.svg)](https://github.com/roko-tech/MagicRemoteService/actions/workflows/build.yml)

Control your PC with the LG Magic Remote. Point, click, scroll, and type from your couch.

MagicRemoteService turns your LG Magic Remote into a wireless mouse and keyboard for your Windows PC. Works over your local network with no rooting required.

- Forked from [Cathwyler/MagicRemoteService](https://github.com/Cathwyler/MagicRemoteService)
- Tested on LG C2 (webOS 25), LG C1 (webOS 6.0), Windows 10/11

## Quick Start

### Option 1: Download Release (Recommended)

1. Download the latest release from the [Releases page](https://github.com/roko-tech/MagicRemoteService/releases)
2. Extract to a folder (e.g. `C:\MagicRemoteService`)
3. Run `MagicRemoteService.exe`
4. Follow the setup wizard to add your TV and install the TV app

### Option 2: Build from Source

```
git clone https://github.com/roko-tech/MagicRemoteService.git
cd MagicRemoteService
build.bat
```

Requires Visual Studio 2022 (or Build Tools) and .NET Framework 4.7.2.

## Setup

### Prerequisites

- **PC**: Windows 10/11
- **TV**: LG TV with webOS and Developer Mode enabled ([guide](https://webostv.developer.lge.com/develop/getting-started/developer-mode-app))
- **Connection**: PC connected to TV via HDMI, both on the same local network
- **Tools**: Node.js + webOS CLI (`npm install -g @webos-tools/cli`)

### Installation Steps

1. Enable **Developer Mode** on your TV (install "Developer Mode" app from LG Content Store)
2. Run `MagicRemoteService.exe` on your PC
3. In the **TVs** tab: click "Add TV", enter your TV details
4. Select HDMI input, configure IP/MAC settings
5. Click **Install on TV**
6. In the **PC** tab: check "Automatically launch at startup", click Save

The service runs in the background and starts automatically on boot.

## Features

### Core
- **Pointer control** — Move the Magic Remote to control the PC mouse cursor
- **Click** — Short press OK = left click, long press = right click
- **Scroll** — Smooth momentum-based scrolling with Mac-like feel
- **Keyboard** — Arrow keys, Enter, Escape, number pad, and more
- **Wake-on-LAN** — Turn on your PC from the TV

### New in This Fork
- **Auto-discovery (SSDP)** — TV finds your PC automatically, no manual IP needed
- **Web Settings UI** — Configure everything at http://localhost:41231
- **Visual remote configurator** — Click buttons on a visual remote to remap them
- **Smooth scrolling** — Momentum-based with configurable per-app exclusions
- **Per-app scroll exclusions** — Disable smooth scroll for specific apps (e.g. PotPlayer volume)
- **Connection toasts** — Small non-blocking notifications instead of blocking dialogs
- **JSON key bindings** — Human-readable `bindings.json` config file
- **Service keepalive** — Prevents webOS 25 from killing the TV service
- **GitHub Actions CI** — Automated builds on every push

### Bug Fixes
- **Screensaver suppression** ([#66](https://github.com/Cathwyler/MagicRemoteService/issues/66)) — Fixed inverted parameter that let screensaver activate during use
- **webOS 25 sensor retry** — Added error code 1003 handling with auto-retry
- **CPU usage** — Fixed from 20%+ down to ~0% (timer polling + WebSocket spin fixes)
- **WebSocket security** — Frame validation, handshake checks, safe serialization
- **Resource leaks** — Timer, Process, Registry, GCHandle cleanup
- **ares-cli PATH** — Auto-discovers Node.js/npm across user profiles and version managers

## Web Settings UI

Open **http://localhost:41231** in any browser while the service is running.

- **Visual remote** — Click any button to see and change its binding
- **Action types** — Mouse click, keyboard key, keyboard shortcut, shell command, or special action
- **Scroll exclusions** — Add/remove apps where smooth scroll is disabled
- **Service status** — Port, connected TV, auto-shutdown status
- **Save & Restart** — Apply changes immediately

Changes are saved to `bindings.json` next to the exe and synced with the Windows Settings UI.

## Default Key Mappings

| Remote Button | PC Action |
|---------------|-----------|
| Pointer | Mouse cursor |
| Short click (OK) | Left click |
| Long click | Right click |
| Scroll wheel | Mouse scroll (smooth) |
| Arrow keys | Arrow keys |
| Back | Escape |
| Red | Shutdown / Wake PC |
| Green | Windows menu |
| Yellow | Right click |
| Blue | Toggle TV keyboard |
| Volume + | Ctrl+C (Copy) |
| Volume - | Ctrl+V (Paste) |
| 0-9 | NumPad 0-9 |

All mappings are customizable via the web UI or `bindings.json`.

## Configuration

### bindings.json

Place next to `MagicRemoteService.exe`. Loaded on each TV connection.

```json
{
  "bindings": {
    "0x0193": [{ "type": "action", "value": "shutdown" }],
    "0x0194": [{ "type": "keyboard", "virtualKey": 91, "scanCode": 91, "extended": true }],
    "0x0001": [{ "type": "mouse", "value": "left" }]
  },
  "scrollExclude": ["PotPlayerMini64", "PotPlayer"]
}
```

| Type | Fields | Example |
|------|--------|---------|
| `mouse` | `value`: left, right, middle | `{"type":"mouse","value":"left"}` |
| `keyboard` | `virtualKey`, `scanCode`, `extended` | `{"type":"keyboard","virtualKey":27,"scanCode":1}` |
| `action` | `value`: shutdown, keyboard | `{"type":"action","value":"shutdown"}` |
| `command` | `command`: shell command | `{"type":"command","command":"notepad.exe"}` |

### Auto-Discovery

The PC service broadcasts via SSDP on your local network. The TV app discovers it automatically. To use a fixed IP instead, configure it in the Settings UI TVs tab.

## How It Works

```
LG Magic Remote  -->  TV App (webOS)  --WebSocket-->  PC Service (Windows)  --SendInput-->  Mouse/Keyboard
   gyroscope          main.js                          Service.cs                           Windows desktop
   buttons            sensor data                      port 41230
   scroll wheel       binary frames                    
```

The system has two processes on the PC:
- **Service** (SYSTEM) — Listens for WebSocket connections, manages lifecycle
- **Client** (User session) — Receives input via named pipe, calls SendInput

This split is required because SendInput needs the user's desktop session, while the service needs to run at boot.

## Troubleshooting

| Problem | Solution |
|---------|----------|
| "Service is busy" / "not running" | Reboot TV, relaunch app |
| Pointer doesn't work, scroll does | Wave the remote at the TV to activate gyroscope. Auto-retries every 3 seconds |
| "Failed to get sensor data [1003]" | Normal on webOS 25, auto-retries |
| TV can't find PC | Check same subnet, ensure UDP multicast not blocked. Use manual IP as fallback |
| Web UI not loading | Open http://localhost:41231 (not 41230). Service must be running |
| High CPU usage | Update to latest version (fixes timer + WebSocket spin) |
| Settings UI "ares not found" | Install Node.js and `npm install -g @webos-tools/cli` |

## Security

There is no encryption between the TV and PC. Use on trusted local networks only. Don't enter passwords or sensitive information via the remote.

## License

GPL-3.0 — see [LICENSE](LICENSE)
