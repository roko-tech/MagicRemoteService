# MagicRemoteService (Fork)
Use your LG Magic Remote as a Windows mouse and control your PC with the LG Magic Remote from your LG WebOS TV. MagicRemoteService is a Windows service providing computer remote control from a WebOS app on LG WebOS TV. MagicRemoteService works without rooting your TV. Tested with webOS 6.0 (OLED65C1, OLED48C1), webOS 25 (OLED48C26LA/LG C2), and Windows 10/11.

- [Original Source](https://github.com/Cathwyler/MagicRemoteService)
- [This Fork](https://github.com/rokogan/MagicRemoteService)

## What's New in This Fork

### Bug Fixes
- **Screensaver suppression (fixes [#66](https://github.com/Cathwyler/MagicRemoteService/issues/66))**: The screensaver would activate even while the PC was connected. Fixed inverted `ack` parameter, simplified the activation condition, and enabled it in overlay mode.
- **webOS 25 sensor compatibility**: Added error code `1003` handling alongside `1301` for `getSensorData`, with automatic 3-second retry. The Magic Remote gyroscope now works reliably on webOS 25.
- **errorCode type coercion**: webOS 25 may return errorCode as a number instead of a string. Added `String()` coercion to all switch statements to prevent silent failures.
- **CPU usage reduction**: The user input polling timer was firing every 10ms (100 times/sec), consuming ~20% CPU constantly. Reduced to 500ms (2 times/sec) while maintaining responsiveness.

### New Features
- **Mac-style smooth scrolling**: Replaced the jerky stepped scrolling with momentum-based smooth scrolling. Uses velocity accumulation, 60fps rendering, and friction decay for a natural feel.
- **Adjustable cursor speed**: Added a `dCursorSpeed` constant for configuring pointer sensitivity (0.5 = slow, 1.0 = normal, 2.0 = fast).
- **Service keepalive heartbeat**: Re-creates the ActivityManager KeepAlive every 60 seconds to prevent webOS 25 from killing the service after inactivity.
- **One-command installer**: `install.sh` script that auto-installs all prerequisites and sets up both PC service and TV app interactively.

## Introduction

### How it works
MagicRemoteService is composed of two apps, one for TV sending magic remote inputs and one other for PC reproducing mouse and keyboard inputs. The TV app uses WebOS API and DOM events to catch magic remote inputs, WebSockets (TCP) for main data and Node.js dgram (UDP) for Wake-on-LAN functionality. The PC app uses System.Net.Sockets to receive main data and SendInput Win32 API to reproduce mouse and keyboard inputs.

### About security
There is no encryption data between the TV and the PC. Don't use it if you are unsure of the security of your local network. I strongly recommend to not use it on the internet without a VPN connection. Don't use it to enter password, bank card or any other sensitive information. I clear myself of any responsibility if you got data hacked.

### Possible Improvement
- I already tried Node.js net (TCP) for the main data exchange to get ride of the WebSockets exchange protocol, but using service on TV had really poor performance compared to WebSockets.
- Find a way to detect focus in TextBox control on Windows to automatically pop up the WebOS keyboard.

## Quick Install (This Fork)

1. Clone this repo to your PC
2. Enable Developer Mode on your LG TV (install "Developer Mode" from LG Content Store)
3. Open Git Bash as Administrator
4. Run: `bash install.sh`

The script will:
- Auto-install all prerequisites (Node.js, ares-cli, VS Build Tools, .NET 4.7.2) via winget
- Build and install the PC Windows service
- Configure and deploy the TV app to your LG TV
- Add firewall rules automatically

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
  - (Optional) Setup Windows auto logon. Please refer to [Turn on automatic logon in Windows](https://docs.microsoft.com/en-us/troubleshoot/windows-server/user-profiles-and-logon/turn-on-automatic-logon).

### Troubleshooting
- **"Service is busy" or "service is not running"**: Reboot your TV and relaunch the app
- **Pointer not working but scroll works**: Wave the Magic Remote at the TV to activate the gyroscope sensor. It retries every 3 seconds.
- **"Failed to get sensor data [1003]"**: Normal on webOS 25 — the sensor subscription will auto-retry

## Using MagicRemoteService
MagicRemoteService need to run PC and TV app. TV and PC need properly network and video input wired as you configured in installation step.

Default Magic remote inputs :
- The red key shuts down or starts up your PC.
- The yellow key sends a right click to the PC.
- The green key opens Windows menu.
- The blue key pops up the WebOS keyboard.
- The return key sends an escape key to the PC.
- The navigation keys sends arrows keys to the PC.
- A short middle click sends a left click to the PC.
- A long middle click sends a right click to the PC.
- A wheel scroll is sent to the PC.
- Numeric keys are sent to the PC.
- Volume+ sends Ctrl+C to the PC.
- Volume- sends Ctrl+V to the PC.

If you are stuck at startup because Wake-on-LAN didn't work, you can do a long press on the return button to relaunch the app or starts up PC manually.

Some debugs logs notifications can appear at the bottom of the screen. Short click on it to hide.

I strongly recommend adding a Windows automatic screen shutdown to prevent pixel remaining with OLED TV.

## Updating MagicRemoteService
After almost all MagicRemoteService updates, for changes to take effect and to prevent compatibility bugs, you need to reinstall the TV app.

Be careful while updating MagicRemoteService on the PC if you have "Automatically launch at startup" option checked or older executable file version running. You need to stop MagicRemoteService in your Windows service list or any running instance and replace the executable file. Otherwise there is a chance, due to the unique allowed running instance and even if you launch a new version, to keep an older version running.

If you want to change the location of the executable file and if you have "Automatically launch at startup" option checked, you need to stop MagicRemoteService in your Windows service list and any running instance. Once you have done it, you will be able to move the executable. Finally, you will need to resave the PC tab to reconfigure the Windows service with the new path.
