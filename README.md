# MagicRemoteService (Fork)

[![Build](https://github.com/roko-tech/MagicRemoteService/actions/workflows/build.yml/badge.svg)](https://github.com/roko-tech/MagicRemoteService/actions/workflows/build.yml)

Use your LG Magic Remote as a Windows mouse and control your PC with the LG Magic Remote from your LG WebOS TV. MagicRemoteService is a Windows service providing computer remote control from a WebOS app on LG WebOS TV. MagicRemoteService works without rooting your TV. Tested with webOS 6.0 (OLED65C1, OLED48C1), webOS 25 (OLED48C26LA/LG C2), and Windows 10/11.

- [Original Source](https://github.com/Cathwyler/MagicRemoteService)
- [This Fork](https://github.com/roko-tech/MagicRemoteService)

## What's New in This Fork

### New Features
- **Auto-discovery (SSDP)**: The TV automatically finds the PC on your local network — no manual IP configuration needed. Uses standard SSDP multicast protocol.
- **Web-based settings UI**: Browse to `http://localhost:41231` to view service status and edit key bindings from any browser. No need to open the Windows app.
- **JSON key bindings**: Configure remote key mappings in a human-readable `bindings.json` file instead of the Windows registry. Place it next to the exe.
- **Connection status toasts**: Non-blocking notifications on the TV show "Connected", "Connecting...", and "Disconnected" status instead of a blocking dialog.
- **Mac-style smooth scrolling**: Momentum-based smooth scrolling with velocity accumulation, 60fps rendering, and friction decay.
- **Adjustable cursor speed**: `dCursorSpeed` constant for pointer sensitivity (0.5 = slow, 1.0 = normal, 2.0 = fast).
- **Service keepalive heartbeat**: Re-creates the ActivityManager KeepAlive every 60 seconds to prevent webOS 25 from killing the service.
- **One-command installer**: `install.sh` auto-installs all prerequisites and sets up both PC and TV interactively.
- **GitHub Actions CI**: Automated builds on every push. Download pre-built artifacts from the Actions tab, or tag a release for automatic packaging.

### Bug Fixes
- **Screensaver suppression (fixes [#66](https://github.com/Cathwyler/MagicRemoteService/issues/66))**: Fixed inverted `ack` parameter that allowed screensaver to activate during use.
- **webOS 25 sensor compatibility**: Added error code `1003` handling with automatic 3-second retry for gyroscope data.
- **CPU usage reduction**: Reduced user input polling from 10ms to 500ms. Fixed WebSocket receive loop CPU spin (was 400%+ across cores). Reduced sensor callback from 1ms to 16ms (~60Hz).
- **WebSocket security hardening**: Added frame bounds checking, handshake validation, message payload validation, and replaced BinaryFormatter with safe serialization.
- **Resource leak fixes**: Fixed timer disposal, Process handle leaks, RegistryKey leaks, GCHandle leaks, and PowerSettingNotification cleanup.
- **errorCode type coercion**: webOS 25 may return errorCode as a number instead of a string. Added `String()` coercion to prevent silent failures.
- **Assembly binding redirects**: Fixed mismatched versions and added missing redirects for System.Text.Json, System.IO.Pipelines, etc.

## Quick Install

1. Clone this repo to your PC
2. Enable Developer Mode on your LG TV (install "Developer Mode" from LG Content Store)
3. Open Git Bash as Administrator
4. Run: `bash install.sh`

The script will:
- Auto-install all prerequisites (Node.js, ares-cli, VS Build Tools, .NET 4.7.2) via winget
- Build and install the PC Windows service
- Configure and deploy the TV app to your LG TV
- Add firewall rules automatically
- **PC IP is optional** — leave it blank to use auto-discovery

### Download Pre-built Release

Go to the [Actions tab](https://github.com/roko-tech/MagicRemoteService/actions), click the latest successful build, and download the `MagicRemoteService-Release` artifact. Extract it to a folder and run the installer, or register the service manually with `InstallUtil.exe`.

## Configuration

### Web Settings UI

Once the service is running, open **http://localhost:41231** in any browser to:
- View service status (port, inactivity timeout, video input settings)
- Edit key bindings in a live JSON editor with validation
- Save changes (writes `bindings.json` next to the exe)

### Key Bindings (`bindings.json`)

Place a `bindings.json` file next to `MagicRemoteService.exe` to customize remote key mappings. The service loads bindings in this priority order:
1. `bindings.json` (if present)
2. Windows Registry (`HKLM\Software\MagicRemoteService\Remote\Bind`)
3. Built-in defaults

Example binding format:
```json
{
  "bindings": {
    "0x0193": [{ "type": "action", "value": "shutdown" }],
    "0x0194": [{ "type": "keyboard", "virtualKey": 91, "scanCode": 91, "extended": true }],
    "0x0001": [{ "type": "mouse", "value": "left" }],
    "0x01CD": [{ "type": "keyboard", "virtualKey": 27, "scanCode": 1, "extended": false }]
  }
}
```

Binding types:
| Type | Fields | Example |
|------|--------|---------|
| `mouse` | `value`: left, right, middle | `{"type":"mouse","value":"left"}` |
| `keyboard` | `virtualKey`, `scanCode`, `extended` | `{"type":"keyboard","virtualKey":27,"scanCode":1,"extended":false}` |
| `action` | `value`: shutdown, keyboard | `{"type":"action","value":"shutdown"}` |
| `command` | `command`: shell command string | `{"type":"command","command":"notepad.exe"}` |

### Auto-Discovery

The PC service broadcasts itself on the local network using SSDP (multicast `239.255.255.250:1900`). When the TV app starts, it automatically discovers the PC — no manual IP entry needed.

If you configured a specific IP during `install.sh`, auto-discovery is skipped and that IP is used directly. To switch to auto-discovery, reinstall the TV app and leave the PC IP field empty.

### Manual Installation (Original Method)

- Install WebOS Command line interface on your PC. Please refer to [CLI Installation](https://webostv.developer.lge.com/develop/tools/cli-installation#how-to-install).
- Install and activate developer mode app on your LG WebOS TV. Please refer to [Installing Developer Mode app](https://webostv.developer.lge.com/develop/getting-started/developer-mode-app#installing-developer-mode-app) and [Turning Developer Mode on](https://webostv.developer.lge.com/develop/getting-started/developer-mode-app#turning-developer-mode-on).
- Open MagicRemoteService on PC.
- Add your TV. You need to switch on the "Key Server" option on the developer mode app on your LG WebOS TV before confirm. Please refer to [Connecting with CLI](https://webostv.developer.lge.com/develop/getting-started/developer-mode-app#connecting-with-cli).
- Select a TV then configure and install it.
- Configure PC and save.
- (Optional) Configure Remote and save.
- Others
  - (Optional) Setup Wake-on-LAN on your motherboard's PC.
  - (Optional) Setup Windows auto logon. Please refer to [Turn on automatic logon in Windows](https://learn.microsoft.com/en-us/troubleshoot/windows-server/user-profiles-and-logon/turn-on-automatic-logon).

## How It Works

MagicRemoteService is composed of two apps:

| Component | Platform | Technology | Role |
|-----------|----------|------------|------|
| **PC Service** | Windows | C# / .NET Framework 4.7.2 | Receives WebSocket messages, translates to mouse/keyboard input via Win32 SendInput API |
| **TV App** | LG webOS | JavaScript / HTML | Captures Magic Remote sensor data (gyroscope, buttons, scroll), sends as binary WebSocket messages |
| **TV Service** | LG webOS | Node.js | Background service for WoL, auto-launch, SSDP discovery, and logging |

**Data flow:** Magic Remote gyroscope/buttons → TV app (main.js) → WebSocket binary frames → PC service (Service.cs) → Win32 SendInput → Windows mouse/keyboard events

### About Security
There is no encryption between the TV and PC. This is designed for **local network use only**. Don't use it on the internet without a VPN. Don't use it to enter passwords or sensitive information.

## Troubleshooting
- **"Service is busy" or "service is not running"**: Reboot your TV and relaunch the app
- **Pointer not working but scroll works**: Wave the Magic Remote at the TV to activate the gyroscope sensor. It retries every 3 seconds.
- **"Failed to get sensor data [1003]"**: Normal on webOS 25 — the sensor subscription will auto-retry
- **TV can't find PC (auto-discovery)**: Ensure both devices are on the same subnet. Check that UDP multicast is not blocked by your router. Fall back to manual IP if needed.
- **Web settings not loading**: Open `http://localhost:41231` (not the service port 41230). Requires the service to be running.

## Default Remote Key Mappings

| Remote Button | PC Action |
|---------------|-----------|
| Red key | Shutdown / Wake PC |
| Green key | Windows menu |
| Yellow key | Right click |
| Blue key | WebOS keyboard |
| Return | Escape |
| Navigation keys | Arrow keys |
| Short middle click | Left click |
| Long middle click | Right click |
| Wheel scroll | Mouse scroll |
| Numeric keys (0-9) | NumPad keys |
| Volume+ | Ctrl+C |
| Volume- | Ctrl+V |

If you are stuck at startup because Wake-on-LAN didn't work, long press the return button to relaunch the app.

I strongly recommend adding a Windows automatic screen shutdown to prevent pixel burn-in with OLED TVs.

## Updating

After updating, reinstall the TV app for changes to take effect. Stop the service before replacing the exe:

```
net stop MagicRemoteService
# Replace files
net start MagicRemoteService
```

## Building from Source

### Using the installer (recommended)
```bash
bash install.sh
```

### Using build.bat
```
build.bat
```
Supports Visual Studio 2019 and 2022 (Community, Professional, Enterprise, Build Tools).

### Using MSBuild directly
```
nuget restore MagicRemoteService.sln
msbuild MagicRemoteService.sln /p:Configuration=Release /p:Platform="Any CPU"
```

### CI
Every push triggers a GitHub Actions build. Download artifacts from the [Actions tab](https://github.com/roko-tech/MagicRemoteService/actions). Tag a version (`git tag v1.2.5.4 && git push --tags`) to create a GitHub Release automatically.
