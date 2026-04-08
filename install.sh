#!/bin/bash
# MagicRemoteService - Complete Installer (PC Service + TV App)
# Run from Git Bash as Administrator for PC service installation

set -e

# Input validation helpers
validate_ip() {
    local ip="$1"
    if [[ ! "$ip" =~ ^[0-9]{1,3}\.[0-9]{1,3}\.[0-9]{1,3}\.[0-9]{1,3}$ ]]; then
        echo "ERROR: Invalid IP address format: $ip"
        return 1
    fi
}
validate_mac() {
    local mac="$1"
    if [[ ! "$mac" =~ ^([0-9A-Fa-f]{2}:){5}[0-9A-Fa-f]{2}$ ]]; then
        echo "ERROR: Invalid MAC address format: $mac (expected XX:XX:XX:XX:XX:XX)"
        return 1
    fi
}
validate_port() {
    local port="$1"
    if [[ ! "$port" =~ ^[0-9]+$ ]] || [ "$port" -lt 1 ] || [ "$port" -gt 65535 ]; then
        echo "ERROR: Invalid port number: $port (must be 1-65535)"
        return 1
    fi
}
validate_name() {
    local name="$1"
    if [[ "$name" =~ [\;\`\$\|\&\>\<\'\"] ]]; then
        echo "ERROR: Name contains invalid characters: $name"
        return 1
    fi
}
validate_number() {
    local num="$1"
    if [[ ! "$num" =~ ^[0-9]*\.?[0-9]+$ ]]; then
        echo "ERROR: Invalid number: $num"
        return 1
    fi
}

SCRIPT_DIR="$(cd "$(dirname "$0")" && pwd)"
SOURCE_DIR="$SCRIPT_DIR/MagicRemoteService/Resources/TV"
BUILD_DIR="$SCRIPT_DIR/build-output"
VERSION="1.2.5.4"

# ========================================
# PATH SETUP — find Node.js, npm, ares-cli
# ========================================
setup_path() {
    local EXTRA_PATHS=""

    # Volta (version manager)
    if [ -d "/c/Program Files/Volta" ]; then
        EXTRA_PATHS="/c/Program Files/Volta:$EXTRA_PATHS"
    fi
    if [ -d "$USERPROFILE/.volta/bin" ]; then
        EXTRA_PATHS="$USERPROFILE/.volta/bin:$EXTRA_PATHS"
    fi

    # nvm-windows
    if [ -n "$NVM_HOME" ]; then
        EXTRA_PATHS="$NVM_HOME:$EXTRA_PATHS"
        if [ -n "$NVM_SYMLINK" ]; then
            EXTRA_PATHS="$NVM_SYMLINK:$EXTRA_PATHS"
        fi
    fi

    # Standard Node.js install locations
    for dir in \
        "/c/Program Files/nodejs" \
        "/c/Program Files (x86)/nodejs" \
        "$APPDATA/npm" \
        "$LOCALAPPDATA/Volta/bin"; do
        if [ -d "$dir" ]; then
            EXTRA_PATHS="$dir:$EXTRA_PATHS"
        fi
    done

    local NPM_GLOBAL=$(echo "$APPDATA/npm" | sed 's|\\|/|g')
    if [ -d "$NPM_GLOBAL" ]; then
        EXTRA_PATHS="$NPM_GLOBAL:$EXTRA_PATHS"
    fi

    export PATH="$EXTRA_PATHS$PATH"
}

setup_path

echo "========================================"
echo "  MagicRemoteService Complete Installer"
echo "========================================"
echo ""

# ========================================
# PREREQUISITES CHECK & AUTO-INSTALL
# ========================================
install_prerequisites() {
    echo "Checking prerequisites..."
    echo ""
    MISSING=()

    # 1. Git
    if command -v git &> /dev/null; then
        echo "  [OK] Git $(git --version | cut -d' ' -f3)"
    else
        echo "  [MISSING] Git"
        MISSING+=("git")
    fi

    # 2. Node.js (>= 14.15.1 for ares-cli)
    if command -v node &> /dev/null; then
        NODE_VER=$(node --version 2>/dev/null)
        echo "  [OK] Node.js $NODE_VER"
        NODE_MAJOR=$(echo "$NODE_VER" | sed 's/v//' | cut -d. -f1)
        if [ "$NODE_MAJOR" -lt 14 ]; then
            echo "    WARNING: ares-cli requires Node.js >= 14.15.1, you have $NODE_VER"
            MISSING+=("nodejs")
        fi
    else
        echo "  [MISSING] Node.js (>= 14.15.1 required)"
        MISSING+=("nodejs")
    fi

    # 3. npm
    if command -v npm &> /dev/null; then
        echo "  [OK] npm $(npm --version 2>/dev/null)"
    else
        echo "  [MISSING] npm"
        MISSING+=("npm")
    fi

    # 4. ares-cli
    if command -v ares-package &> /dev/null; then
        echo "  [OK] @webos-tools/cli"
    elif [ -f "$APPDATA/npm/node_modules/@webos-tools/cli/bin/ares-package.js" ]; then
        echo "  [OK] @webos-tools/cli (found via npm global, creating wrappers)"
        ARES_BIN="$APPDATA/npm/node_modules/@webos-tools/cli/bin"
        ares-package() { node "$ARES_BIN/ares-package.js" "$@"; }
        ares-install() { node "$ARES_BIN/ares-install.js" "$@"; }
        ares-launch() { node "$ARES_BIN/ares-launch.js" "$@"; }
        ares-setup-device() { node "$ARES_BIN/ares-setup-device.js" "$@"; }
        ares-novacom() { node "$ARES_BIN/ares-novacom.js" "$@"; }
        export -f ares-package ares-install ares-launch ares-setup-device ares-novacom
    else
        echo "  [MISSING] @webos-tools/cli"
        MISSING+=("ares-cli")
    fi

    # 5. MSBuild / Visual Studio
    MSBUILD=""
    for path in \
        "/c/Program Files/Microsoft Visual Studio/2022/Community/MSBuild/Current/Bin/MSBuild.exe" \
        "/c/Program Files/Microsoft Visual Studio/2022/Professional/MSBuild/Current/Bin/MSBuild.exe" \
        "/c/Program Files/Microsoft Visual Studio/2022/Enterprise/MSBuild/Current/Bin/MSBuild.exe" \
        "/c/Program Files (x86)/Microsoft Visual Studio/2022/BuildTools/MSBuild/Current/Bin/MSBuild.exe"; do
        if [ -f "$path" ]; then
            MSBUILD="$path"
            break
        fi
    done
    if [ -n "$MSBUILD" ]; then
        echo "  [OK] MSBuild"
    else
        echo "  [MISSING] Visual Studio 2022 / Build Tools"
        MISSING+=("msbuild")
    fi

    # 6. .NET Framework 4.7.2
    if [ -d "/c/Program Files (x86)/Reference Assemblies/Microsoft/Framework/.NETFramework/v4.7.2" ]; then
        echo "  [OK] .NET Framework 4.7.2 targeting pack"
    else
        echo "  [MISSING] .NET Framework 4.7.2 Developer Pack"
        MISSING+=("dotnet472")
    fi

    echo ""

    if [ ${#MISSING[@]} -eq 0 ]; then
        echo "All prerequisites installed!"
        echo ""
        return 0
    fi

    echo "Missing ${#MISSING[@]} prerequisite(s)."

    # Check winget
    HAS_WINGET=false
    if command -v winget &> /dev/null; then
        HAS_WINGET=true
    elif [ -f "/c/Users/$(whoami)/AppData/Local/Microsoft/WindowsApps/winget.exe" ]; then
        HAS_WINGET=true
        alias winget="/c/Users/$(whoami)/AppData/Local/Microsoft/WindowsApps/winget.exe"
    fi

    read -p "Auto-install missing prerequisites? [Y/n]: " AUTO_INSTALL
    AUTO_INSTALL=${AUTO_INSTALL:-Y}

    if [[ ! "$AUTO_INSTALL" =~ ^[Yy]$ ]]; then
        echo ""
        echo "Please install manually:"
        for dep in "${MISSING[@]}"; do
            case $dep in
                git)       echo "  - Git: https://git-scm.com/download/win" ;;
                nodejs)    echo "  - Node.js: https://nodejs.org/ (LTS recommended)" ;;
                npm)       echo "  - npm: comes with Node.js" ;;
                ares-cli)  echo "  - Run: npm install -g @webos-tools/cli" ;;
                msbuild)   echo "  - VS 2022 Build Tools: https://visualstudio.microsoft.com/downloads/#build-tools-for-visual-studio-2022" ;;
                dotnet472) echo "  - .NET 4.7.2 Dev Pack: https://dotnet.microsoft.com/download/dotnet-framework/net472" ;;
            esac
        done
        echo ""
        echo "Then re-run this script."
        exit 1
    fi

    echo ""
    for dep in "${MISSING[@]}"; do
        case $dep in
            git)
                echo "  Installing Git..."
                if $HAS_WINGET; then
                    winget install --id Git.Git -e --accept-source-agreements --accept-package-agreements 2>/dev/null
                else
                    echo "    Download from: https://git-scm.com/download/win"
                    exit 1
                fi
                ;;
            nodejs|npm)
                echo "  Installing Node.js..."
                if command -v volta &> /dev/null; then
                    volta install node@20
                elif $HAS_WINGET; then
                    winget install --id OpenJS.NodeJS.LTS -e --accept-source-agreements --accept-package-agreements 2>/dev/null
                else
                    echo "    Download from: https://nodejs.org/"
                    exit 1
                fi
                setup_path
                ;;
            ares-cli)
                echo "  Installing @webos-tools/cli..."
                npm install -g @webos-tools/cli 2>&1
                ;;
            msbuild)
                echo "  Installing VS 2022 Build Tools (this may take several minutes)..."
                if $HAS_WINGET; then
                    winget install --id Microsoft.VisualStudio.2022.BuildTools -e --accept-source-agreements --accept-package-agreements \
                        --override "--add Microsoft.VisualStudio.Workload.ManagedDesktopBuildTools --includeRecommended --quiet --wait" 2>/dev/null
                else
                    echo "    Download from: https://visualstudio.microsoft.com/downloads/#build-tools-for-visual-studio-2022"
                    exit 1
                fi
                ;;
            dotnet472)
                echo "  Installing .NET Framework 4.7.2 Developer Pack..."
                if $HAS_WINGET; then
                    winget install --id Microsoft.DotNet.Framework.DeveloperPack_4 -e --accept-source-agreements --accept-package-agreements 2>/dev/null
                else
                    echo "    Download from: https://dotnet.microsoft.com/download/dotnet-framework/net472"
                    exit 1
                fi
                ;;
        esac
    done

    echo ""
    echo "Prerequisites installed! You may need to restart Git Bash for PATH changes."
    read -p "Continue with installation? [Y/n]: " CONTINUE
    CONTINUE=${CONTINUE:-Y}
    if [[ ! "$CONTINUE" =~ ^[Yy]$ ]]; then
        exit 0
    fi
}

install_prerequisites

echo "What do you want to install?"
echo "  1) PC Service + TV App (full install)"
echo "  2) PC Service only"
echo "  3) TV App only"
read -p "Enter choice [1-3]: " INSTALL_MODE
INSTALL_MODE=${INSTALL_MODE:-1}

# ========================================
# PC SERVICE INSTALLATION
# ========================================
install_pc_service() {
    echo ""
    echo "========================================"
    echo "  PC Service Installation"
    echo "========================================"

    # Apply timer fix (surgical — only safe change)
    echo "[PC 1/4] Applying timer fix to Service.cs..."
    sed -i 's/Interval = 10,/Interval = 500,/' "$SCRIPT_DIR/MagicRemoteService/Service.cs"
    sed -i 's/((uint)System.Environment.TickCount - lii.dwTime) < 10)/((uint)System.Environment.TickCount - lii.dwTime) < 500)/' "$SCRIPT_DIR/MagicRemoteService/Service.cs"

    # Build
    echo "[PC 2/4] Building PC service..."
    "$MSBUILD" "$SCRIPT_DIR/MagicRemoteService.sln" -p:Configuration=Release -t:Rebuild -v:minimal 2>&1 | tail -5
    BUILD_RESULT=$?

    # Revert source change (keep repo clean)
    cd "$SCRIPT_DIR" && git checkout -- MagicRemoteService/Service.cs 2>/dev/null || true

    if [ $BUILD_RESULT -ne 0 ]; then
        echo "ERROR: Build failed."
        return 1
    fi

    # Install directory
    read -p "Install directory [C:\\MagicRemoteService]: " INSTALL_DIR
    INSTALL_DIR=${INSTALL_DIR:-"C:\\MagicRemoteService"}
    INSTALL_DIR_UNIX=$(echo "$INSTALL_DIR" | sed 's|\\|/|g' | sed 's|^C:|/c|')

    echo "[PC 3/4] Installing files..."

    # Stop existing service if running
    SERVICE_EXISTS=$(powershell -Command "(Get-Service MagicRemoteService -ErrorAction SilentlyContinue) -ne \$null" 2>/dev/null || echo "False")
    if [ "$SERVICE_EXISTS" = "True" ]; then
        echo "  Stopping existing service..."
        powershell -Command "Start-Process powershell -Verb RunAs -Wait -ArgumentList '-Command', 'Stop-Service MagicRemoteService -Force; Start-Sleep 2'" 2>/dev/null || true
    fi

    # Copy files
    mkdir -p "$INSTALL_DIR_UNIX"
    cp "$SCRIPT_DIR/MagicRemoteService/bin/Release/MagicRemoteService.exe" "$INSTALL_DIR_UNIX/"
    cp "$SCRIPT_DIR/MagicRemoteService/bin/Release/MagicRemoteService.exe.config" "$INSTALL_DIR_UNIX/" 2>/dev/null || true
    cp "$SCRIPT_DIR/MagicRemoteService/bin/Release/"*.dll "$INSTALL_DIR_UNIX/" 2>/dev/null || true
    for lang in es fr; do
        if [ -d "$SCRIPT_DIR/MagicRemoteService/bin/Release/$lang" ]; then
            mkdir -p "$INSTALL_DIR_UNIX/$lang"
            cp "$SCRIPT_DIR/MagicRemoteService/bin/Release/$lang/"* "$INSTALL_DIR_UNIX/$lang/"
        fi
    done

    echo "[PC 4/4] Registering Windows service..."
    if [ "$SERVICE_EXISTS" = "True" ]; then
        echo "  Starting existing service..."
        powershell -Command "Start-Process powershell -Verb RunAs -Wait -ArgumentList '-Command', 'Start-Service MagicRemoteService'" 2>/dev/null
    else
        INSTALL_DIR_WIN=$(echo "$INSTALL_DIR" | sed 's|/|\\|g')
        echo "  Registering new service (requires admin)..."
        powershell -Command "Start-Process powershell -Verb RunAs -Wait -ArgumentList '-Command', 'cd \"${INSTALL_DIR_WIN}\"; C:\Windows\Microsoft.NET\Framework64\v4.0.30319\InstallUtil.exe MagicRemoteService.exe; Start-Service MagicRemoteService'" 2>/dev/null
    fi

    # Firewall rule
    FW_PORT=${PC_PORT:-41230}
    echo "  Adding firewall rule for port $FW_PORT..."
    powershell -Command "Start-Process powershell -Verb RunAs -Wait -ArgumentList '-Command', 'Remove-NetFirewallRule -DisplayName \"MagicRemoteService\" -ErrorAction SilentlyContinue; New-NetFirewallRule -DisplayName \"MagicRemoteService\" -Direction Inbound -Protocol TCP -LocalPort $FW_PORT -Action Allow'" 2>/dev/null || echo "  WARNING: Failed to create firewall rule. You may need to add it manually."

    sleep 2
    SERVICE_STATUS=$(powershell -Command "(Get-Service MagicRemoteService).Status" 2>/dev/null || echo "NotFound")
    if [ "$SERVICE_STATUS" = "Running" ]; then
        echo "  PC Service installed and running!"
    else
        echo "  WARNING: Service status: $SERVICE_STATUS"
        echo "  You may need to run this script as Administrator."
    fi
}

# ========================================
# TV APP INSTALLATION
# ========================================
install_tv_app() {
    echo ""
    echo "========================================"
    echo "  TV App Installation"
    echo "========================================"
    echo ""

    # List devices
    echo "Available TV devices:"
    ares-setup-device --list
    echo ""

    # Setup device if needed
    DEVICE_COUNT=$(ares-setup-device --list 2>/dev/null | grep -c "^  " || echo "0")
    if [ "$DEVICE_COUNT" -le 1 ]; then
        echo "No TV devices configured. Let's add one."
        echo "Make sure Developer Mode is enabled on your TV."
        echo ""
        read -p "TV name (e.g., MyTV, C2): " NEW_TV_NAME
        validate_name "$NEW_TV_NAME" || return 1
        read -p "TV IP address: " NEW_TV_IP
        validate_ip "$NEW_TV_IP" || return 1
        read -p "TV passphrase (from Developer Mode app): " NEW_TV_PASS
        ares-setup-device -a "$NEW_TV_NAME" -i "host=$NEW_TV_IP" -i "port=9922" -i "username=prisoner"
        ares-novacom --device "$NEW_TV_NAME" --getkey --passphrase "$NEW_TV_PASS" || echo "  WARNING: Key exchange may have failed. Check passphrase."
        echo ""
        ares-setup-device --list
        echo ""
    fi

    read -p "TV device name: " TV_DEVICE
    read -p "PC IP address: " PC_IP
    validate_ip "$PC_IP" || return 1
    read -p "PC MAC address (XX:XX:XX:XX:XX:XX): " PC_MAC
    validate_mac "$PC_MAC" || return 1
    read -p "Subnet mask [255.255.255.0]: " SUBNET_MASK
    SUBNET_MASK=${SUBNET_MASK:-255.255.255.0}
    validate_ip "$SUBNET_MASK" || return 1
    read -p "PC listen port [41230]: " PC_PORT
    PC_PORT=${PC_PORT:-41230}
    validate_port "$PC_PORT" || return 1

    echo ""
    echo "HDMI port your PC is connected to:"
    echo "  1) HDMI 1    2) HDMI 2    3) HDMI 3    4) HDMI 4"
    read -p "Enter [1-4]: " HDMI_NUM

    case $HDMI_NUM in
        1) HDMI_ID="HDMI_1"; HDMI_SHORT="hdmi1"; HDMI_NAME="HDMI1"; HDMI_SOURCE="ext://hdmi:hdmi1" ;;
        2) HDMI_ID="HDMI_2"; HDMI_SHORT="hdmi2"; HDMI_NAME="HDMI2"; HDMI_SOURCE="ext://hdmi:hdmi2" ;;
        3) HDMI_ID="HDMI_3"; HDMI_SHORT="hdmi3"; HDMI_NAME="HDMI3"; HDMI_SOURCE="ext://hdmi:hdmi3" ;;
        4) HDMI_ID="HDMI_4"; HDMI_SHORT="hdmi4"; HDMI_NAME="HDMI4"; HDMI_SOURCE="ext://hdmi:hdmi4" ;;
        *) echo "Invalid HDMI port"; return 1 ;;
    esac

    read -p "Cursor speed [1.0] (0.5=slow, 2.0=fast): " CURSOR_SPEED
    CURSOR_SPEED=${CURSOR_SPEED:-1.0}
    validate_number "$CURSOR_SPEED" || return 1

    APP_ID="com.cathwyler.magicremoteservice.${HDMI_SHORT}"
    SERVICE_ID="${APP_ID}.service"

    echo ""
    echo "  TV: $TV_DEVICE | PC: $PC_IP | HDMI: $HDMI_NAME | Speed: $CURSOR_SPEED"
    read -p "Proceed? [Y/n]: " CONFIRM
    CONFIRM=${CONFIRM:-Y}
    [[ ! "$CONFIRM" =~ ^[Yy]$ ]] && return 0

    rm -rf "$BUILD_DIR"
    mkdir -p "$BUILD_DIR/MagicRemoteService" "$BUILD_DIR/Service"

    echo ""
    echo "[TV 1/4] Copying source files..."
    cp -r "$SOURCE_DIR/MagicRemoteService/"* "$BUILD_DIR/MagicRemoteService/"
    cp -r "$SOURCE_DIR/Service/"* "$BUILD_DIR/Service/"

    echo "[TV 2/4] Applying configuration..."
    # main.js
    sed -i "s/const strInputId = \"HDMI\"/const strInputId = \"${HDMI_ID}\"/g" "$BUILD_DIR/MagicRemoteService/main.js"
    sed -i "s/const strInputAppId = \"com.webos.app.hdmi\"/const strInputAppId = \"com.webos.app.${HDMI_SHORT}\"/g" "$BUILD_DIR/MagicRemoteService/main.js"
    sed -i "s/const strInputName = \"HDMI\"/const strInputName = \"${HDMI_NAME}\"/g" "$BUILD_DIR/MagicRemoteService/main.js"
    sed -i "s|const strInputSource = \"ext://hdmi\"|const strInputSource = \"${HDMI_SOURCE}\"|g" "$BUILD_DIR/MagicRemoteService/main.js"
    sed -i "s/const strIP = \"127.0.0.1\"/const strIP = \"${PC_IP}\"/g" "$BUILD_DIR/MagicRemoteService/main.js"
    sed -i "s/const uiPort = 41230/const uiPort = ${PC_PORT}/g" "$BUILD_DIR/MagicRemoteService/main.js"
    sed -i "s/const strMask = \"255.255.255.0\"/const strMask = \"${SUBNET_MASK}\"/g" "$BUILD_DIR/MagicRemoteService/main.js"
    sed -i "s/const strMac = \"AA:AA:AA:AA:AA:AA\"/const strMac = \"${PC_MAC}\"/g" "$BUILD_DIR/MagicRemoteService/main.js"
    sed -i "s/const dCursorSpeed = 1.0;/const dCursorSpeed = ${CURSOR_SPEED};/g" "$BUILD_DIR/MagicRemoteService/main.js"
    sed -i "s/const strAppId = \"com.cathwyler.magicremoteservice\"/const strAppId = \"${APP_ID}\"/g" "$BUILD_DIR/MagicRemoteService/main.js"
    # service.js
    sed -i "s/var strAppId = \"com.cathwyler.magicremoteservice\"/var strAppId = \"${APP_ID}\"/g" "$BUILD_DIR/Service/service.js"
    sed -i "s/var strInputAppId = \"com.webos.app.hdmi\"/var strInputAppId = \"com.webos.app.${HDMI_SHORT}\"/g" "$BUILD_DIR/Service/service.js"
    # appinfo.json
    sed -i "s/\"id\": \"com.cathwyler.magicremoteservice\"/\"id\": \"${APP_ID}\"/g" "$BUILD_DIR/MagicRemoteService/appinfo.json"
    sed -i "s/\"version\": \"1.0.0\"/\"version\": \"${VERSION}\"/g" "$BUILD_DIR/MagicRemoteService/appinfo.json"
    sed -i "s/\"appDescription\": \"HDMI\"/\"appDescription\": \"${HDMI_NAME}\"/g" "$BUILD_DIR/MagicRemoteService/appinfo.json"
    # services.json + package.json
    sed -i "s/com.cathwyler.magicremoteservice.service/${SERVICE_ID}/g" "$BUILD_DIR/Service/services.json"
    sed -i "s/com.cathwyler.magicremoteservice.service/${SERVICE_ID}/g" "$BUILD_DIR/Service/package.json"

    echo "[TV 3/4] Packaging..."
    cd "$BUILD_DIR"
    rm -f *.ipk
    ares-package MagicRemoteService Service -o .
    IPK_FILE=$(ls *.ipk)
    echo "  Created: $IPK_FILE"

    echo "[TV 4/4] Installing on TV..."
    ares-install "$IPK_FILE" -d "$TV_DEVICE"

    echo ""
    echo "  TV App installed!"
    echo "  Launch: ares-launch ${APP_ID} -d ${TV_DEVICE}"
    echo "  Remove: ares-install -r ${APP_ID} -d ${TV_DEVICE}"
    echo "  NOTE: Reboot TV if you see 'service is busy' errors."
    echo ""

    read -p "Launch now? [Y/n]: " LAUNCH
    LAUNCH=${LAUNCH:-Y}
    [[ "$LAUNCH" =~ ^[Yy]$ ]] && ares-launch "$APP_ID" -d "$TV_DEVICE" && echo "  Launched!"

    cd "$SCRIPT_DIR"
    rm -rf "$BUILD_DIR"
}

# ========================================
# MAIN
# ========================================
case $INSTALL_MODE in
    1) install_pc_service; install_tv_app ;;
    2) install_pc_service ;;
    3) install_tv_app ;;
    *) echo "Invalid choice."; exit 1 ;;
esac

echo ""
echo "========================================"
echo "  All Done!"
echo "========================================"
echo ""
echo "For a fresh install on a new machine:"
echo "  1. Copy this folder to the new PC"
echo "  2. Open Git Bash as Administrator"
echo "  3. Run: bash install.sh"
echo "  4. The script auto-installs all prerequisites"
echo ""
