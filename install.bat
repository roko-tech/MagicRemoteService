@echo off
setlocal enabledelayedexpansion

echo ========================================
echo   MagicRemoteService Installer v2.1.0
echo ========================================
echo.

:: Check admin
net session >nul 2>&1
if %errorlevel% neq 0 (
    echo This installer requires Administrator privileges.
    echo Right-click and select "Run as administrator".
    pause
    exit /b 1
)

:: ========================================
:: SETUP PATH
:: ========================================
for /d %%u in (C:\Users\*) do (
    if exist "%%u\AppData\Roaming\npm" set "PATH=%%u\AppData\Roaming\npm;!PATH!"
    if exist "%%u\.volta\bin" set "PATH=%%u\.volta\bin;!PATH!"
    if exist "%%u\AppData\Roaming\nvm" (
        for /d %%v in ("%%u\AppData\Roaming\nvm\v*") do set "PATH=%%v;!PATH!"
    )
)
if exist "C:\Program Files\nodejs" set "PATH=C:\Program Files\nodejs;!PATH!"
if exist "C:\Program Files\Volta" set "PATH=C:\Program Files\Volta;!PATH!"
if exist "C:\Program Files (x86)\nodejs" set "PATH=C:\Program Files (x86)\nodejs;!PATH!"
for /d %%u in (C:\Users\*) do (
    if exist "%%u\AppData\Local\Microsoft\WindowsApps\winget.exe" set "PATH=%%u\AppData\Local\Microsoft\WindowsApps;!PATH!"
)

:: ========================================
:: PREREQUISITES
:: ========================================
echo Checking prerequisites...
echo.

set "HAS_NODE=0"
where node >nul 2>&1 && set "HAS_NODE=1"
if "!HAS_NODE!"=="0" (
    echo [MISSING] Node.js
    echo Installing via winget...
    winget install --id OpenJS.NodeJS.LTS -e --accept-source-agreements --accept-package-agreements >nul 2>&1
    if !errorlevel! neq 0 (
        echo Failed. Download from https://nodejs.org/
        pause
        exit /b 1
    )
    echo [INSTALLED] Node.js
    set "PATH=%APPDATA%\npm;C:\Program Files\nodejs;!PATH!"
) else (
    for /f "tokens=*" %%v in ('node --version 2^>nul') do echo [OK] Node.js %%v
)

set "HAS_ARES=0"
where ares-package >nul 2>&1 && set "HAS_ARES=1"
if "!HAS_ARES!"=="0" (
    echo [MISSING] @webos-tools/cli
    echo Installing...
    call npm install -g @webos-tools/cli >nul 2>&1
    echo [INSTALLED] @webos-tools/cli
) else (
    echo [OK] @webos-tools/cli
)

set "MSBUILD="
for %%p in (
    "C:\Program Files\Microsoft Visual Studio\2022\Community\MSBuild\Current\Bin\MSBuild.exe"
    "C:\Program Files\Microsoft Visual Studio\2022\Professional\MSBuild\Current\Bin\MSBuild.exe"
    "C:\Program Files\Microsoft Visual Studio\2022\Enterprise\MSBuild\Current\Bin\MSBuild.exe"
    "C:\Program Files (x86)\Microsoft Visual Studio\2022\BuildTools\MSBuild\Current\Bin\MSBuild.exe"
) do (
    if exist %%p set "MSBUILD=%%~p"
)
if not defined MSBUILD (
    echo [MISSING] Visual Studio Build Tools
    echo Installing via winget ^(may take several minutes^)...
    winget install --id Microsoft.VisualStudio.2022.BuildTools -e --accept-source-agreements --accept-package-agreements --override "--add Microsoft.VisualStudio.Workload.ManagedDesktopBuildTools --includeRecommended --quiet --wait" >nul 2>&1
    for %%p in ("C:\Program Files (x86)\Microsoft Visual Studio\2022\BuildTools\MSBuild\Current\Bin\MSBuild.exe") do (
        if exist %%p set "MSBUILD=%%~p"
    )
    if not defined MSBUILD (
        echo Failed. Download from https://visualstudio.microsoft.com/downloads/
        pause
        exit /b 1
    )
    echo [INSTALLED] VS Build Tools
) else (
    echo [OK] MSBuild
)

if exist "C:\Program Files (x86)\Reference Assemblies\Microsoft\Framework\.NETFramework\v4.7.2" (
    echo [OK] .NET Framework 4.7.2
) else (
    echo [MISSING] .NET Framework 4.7.2
    echo Installing via winget...
    winget install --id Microsoft.DotNet.Framework.DeveloperPack_4 -e --accept-source-agreements --accept-package-agreements >nul 2>&1
    if not exist "C:\Program Files (x86)\Reference Assemblies\Microsoft\Framework\.NETFramework\v4.7.2" (
        echo Failed. Download from https://dotnet.microsoft.com/download/dotnet-framework/net472
        pause
        exit /b 1
    )
    echo [INSTALLED] .NET Framework 4.7.2
)

echo.
echo ========================================
echo   1^) PC Service + TV App ^(full install^)
echo   2^) PC Service only
echo   3^) TV App only
echo ========================================
set /p CHOICE="Enter choice [1-3]: "

if "!CHOICE!"=="3" goto :tv_only
if "!CHOICE!"=="2" goto :pc_service

:: ========================================
:pc_service
echo.
echo ========================================
echo   Building PC Service
echo ========================================

if not exist packages (
    echo Restoring NuGet packages...
    set "HAS_NUGET=0"
    where nuget >nul 2>&1 && set "HAS_NUGET=1"
    if "!HAS_NUGET!"=="1" (
        nuget restore MagicRemoteService.sln -Source https://api.nuget.org/v3/index.json
    ) else (
        powershell -Command "Invoke-WebRequest -Uri 'https://dist.nuget.org/win-x86-commandline/latest/nuget.exe' -OutFile 'nuget.exe'" 2>&1
        if exist nuget.exe (
            nuget.exe restore MagicRemoteService.sln -Source https://api.nuget.org/v3/index.json
            del nuget.exe 2>nul
        )
    )
    if not exist packages (
        "!MSBUILD!" MagicRemoteService.sln /t:Restore /p:RestoreSources=https://api.nuget.org/v3/index.json /v:minimal 2>nul
    )
)

echo Building...
"!MSBUILD!" MagicRemoteService.sln /p:Configuration=Release /t:Rebuild /v:minimal /restore
if !errorlevel! neq 0 (
    echo Build failed!
    pause
    exit /b 1
)

set /p INSTALL_DIR="Install directory [C:\MagicRemoteService]: "
if "!INSTALL_DIR!"=="" set "INSTALL_DIR=C:\MagicRemoteService"

echo Installing to !INSTALL_DIR!...
if not exist "!INSTALL_DIR!" mkdir "!INSTALL_DIR!"

net stop MagicRemoteService >nul 2>&1
timeout /t 3 /nobreak >nul
taskkill /F /IM MagicRemoteService.exe >nul 2>&1
timeout /t 2 /nobreak >nul

copy /y "MagicRemoteService\bin\Release\MagicRemoteService.exe" "!INSTALL_DIR!\" >nul
copy /y "MagicRemoteService\bin\Release\MagicRemoteService.exe.config" "!INSTALL_DIR!\" >nul
copy /y "MagicRemoteService\bin\Release\*.dll" "!INSTALL_DIR!\" >nul
copy /y "settings-ui.html" "!INSTALL_DIR!\" >nul 2>nul
if not exist "!INSTALL_DIR!\Resources" mkdir "!INSTALL_DIR!\Resources"
copy /y "MagicRemoteService\bin\Release\Resources\node-polyfill.js" "!INSTALL_DIR!\Resources\" >nul 2>nul
for %%l in (es fr) do (
    if exist "MagicRemoteService\bin\Release\%%l" (
        if not exist "!INSTALL_DIR!\%%l" mkdir "!INSTALL_DIR!\%%l"
        copy /y "MagicRemoteService\bin\Release\%%l\*" "!INSTALL_DIR!\%%l\" >nul
    )
)

echo Registering service...
sc query MagicRemoteService >nul 2>&1
if !errorlevel! neq 0 (
    C:\Windows\Microsoft.NET\Framework64\v4.0.30319\InstallUtil.exe "!INSTALL_DIR!\MagicRemoteService.exe" >nul 2>&1
    if !errorlevel! neq 0 (
        sc create MagicRemoteService binPath= "!INSTALL_DIR!\MagicRemoteService.exe" start= auto >nul 2>&1
    )
)

netsh advfirewall firewall delete rule name="MagicRemoteService" >nul 2>&1
netsh advfirewall firewall add rule name="MagicRemoteService" dir=in action=allow protocol=TCP localport=41230 >nul 2>&1
netsh advfirewall firewall add rule name="MagicRemoteService Web UI" dir=in action=allow protocol=TCP localport=41231 >nul 2>&1

net start MagicRemoteService >nul 2>&1
timeout /t 3 /nobreak >nul

sc query MagicRemoteService | find "RUNNING" >nul 2>&1
if !errorlevel! equ 0 (
    echo.
    echo [OK] PC Service installed and running!
    echo     Web UI: http://localhost:41231
) else (
    echo.
    echo [WARNING] Service may not have started. Check Services.msc
)

if "!CHOICE!"=="2" goto :done

:: ========================================
:tv_only
echo.
echo ========================================
echo   Installing TV App
echo ========================================
echo.

:: Try auto-detect from running service
set "AUTO_IP="
set "AUTO_MAC="
set "AUTO_MASK="
powershell -Command "try { $r = Invoke-WebRequest -Uri 'http://localhost:41231/api/pcinfo' -UseBasicParsing -TimeoutSec 3; $j = $r.Content | ConvertFrom-Json; $i = $j.interfaces[0]; Write-Host \"IP=$($i.ip)|MAC=$($i.mac)|MASK=$($i.mask)\" } catch {}" 2>nul > "%TEMP%\mrs-pcinfo.txt"
for /f "tokens=1,2,3 delims=|" %%a in ('type "%TEMP%\mrs-pcinfo.txt" 2^>nul') do (
    set "%%a"
    set "%%b"
    set "%%c"
)
del "%TEMP%\mrs-pcinfo.txt" 2>nul
if defined IP set "AUTO_IP=!IP!"
if defined MAC set "AUTO_MAC=!MAC!"
if defined MASK set "AUTO_MASK=!MASK!"

echo Available TV devices:
call ares-setup-device --list 2>nul
echo.

set /p TV_DEVICE="TV device name: "

if defined AUTO_IP (
    echo.
    echo   Auto-detected PC info:
    echo     IP:   !AUTO_IP!
    echo     MAC:  !AUTO_MAC!
    echo     Mask: !AUTO_MASK!
    echo.
    set /p USE_AUTO="Use auto-detected values? [Y/n]: "
    if /i not "!USE_AUTO!"=="n" (
        set "PC_IP=!AUTO_IP!"
        set "PC_MAC=!AUTO_MAC!"
        set "SUBNET=!AUTO_MASK!"
    ) else (
        set /p PC_IP="PC IP address [auto-discover]: "
        set /p PC_MAC="PC MAC address (XX:XX:XX:XX:XX:XX): "
        set /p SUBNET="Subnet mask [255.255.255.0]: "
    )
) else (
    set /p PC_IP="PC IP address [auto-discover]: "
    set /p PC_MAC="PC MAC address (XX:XX:XX:XX:XX:XX): "
    set /p SUBNET="Subnet mask [255.255.255.0]: "
)
if "!SUBNET!"=="" set "SUBNET=255.255.255.0"
set /p PORT="PC listen port [41230]: "
if "!PORT!"=="" set "PORT=41230"

echo.
echo HDMI port ^(auto-detected at runtime if wrong^):
echo   1^) HDMI 1    2^) HDMI 2    3^) HDMI 3    4^) HDMI 4
set /p HDMI_NUM="Enter [1-4]: "

if "!HDMI_NUM!"=="1" (set "HDMI_ID=HDMI_1"&set "HDMI_SHORT=hdmi1"&set "HDMI_NAME=HDMI 1"&set "HDMI_SRC=ext://hdmi:hdmi1")
if "!HDMI_NUM!"=="2" (set "HDMI_ID=HDMI_2"&set "HDMI_SHORT=hdmi2"&set "HDMI_NAME=HDMI 2"&set "HDMI_SRC=ext://hdmi:hdmi2")
if "!HDMI_NUM!"=="3" (set "HDMI_ID=HDMI_3"&set "HDMI_SHORT=hdmi3"&set "HDMI_NAME=HDMI 3"&set "HDMI_SRC=ext://hdmi:hdmi3")
if "!HDMI_NUM!"=="4" (set "HDMI_ID=HDMI_4"&set "HDMI_SHORT=hdmi4"&set "HDMI_NAME=HDMI 4"&set "HDMI_SRC=ext://hdmi:hdmi4")

if not defined HDMI_SHORT (
    echo Invalid HDMI port.
    pause
    exit /b 1
)

set "APP_ID=com.cathwyler.magicremoteservice.!HDMI_SHORT!"
set "SVC_ID=!APP_ID!.service"

echo.
echo   TV:   !TV_DEVICE!
echo   PC:   !PC_IP! ^(empty = auto-discover^)
echo   MAC:  !PC_MAC!
echo   HDMI: !HDMI_NAME! ^(auto-corrected at runtime^)
echo.
set /p CONFIRM="Proceed? [Y/n]: "
if /i "!CONFIRM!"=="n" goto :done

set "BUILD=%TEMP%\mrs-tv-build"
if exist "!BUILD!" rmdir /s /q "!BUILD!"
mkdir "!BUILD!\MagicRemoteService"
mkdir "!BUILD!\Service"
xcopy /s /q "MagicRemoteService\Resources\TV\MagicRemoteService\*" "!BUILD!\MagicRemoteService\" >nul
xcopy /s /q "MagicRemoteService\Resources\TV\Service\*" "!BUILD!\Service\" >nul

:: Generate config.json
echo Generating config.json...
set "CFG_IP=!PC_IP!"
if "!CFG_IP!"=="" set "CFG_IP=127.0.0.1"

powershell -Command "$cfg = [ordered]@{inputId='!HDMI_ID!';inputAppId='com.webos.app.!HDMI_SHORT!';inputName='!HDMI_NAME!';inputSource='!HDMI_SRC!';ip='!CFG_IP!';port=[int]!PORT!;mask='!SUBNET!';mac='!PC_MAC!';appId='!APP_ID!';overlay=$true;inputDirect=$true;longClick=[int]1500;cursorSpeed=[double]1.0;extend=$true}; $json = $cfg | ConvertTo-Json; [System.IO.File]::WriteAllText('!BUILD!\MagicRemoteService\config.json', $json); [System.IO.File]::WriteAllText('!BUILD!\Service\config.json', $json)"

:: Replace IDs in webOS packaging files only
powershell -Command "$c = Get-Content '!BUILD!\MagicRemoteService\appinfo.json' -Raw; $c = $c -replace '\"id\": \"com.cathwyler.magicremoteservice\"', '\"id\": \"!APP_ID!\"'; $c = $c -replace '\"version\": \"1.0.0\"', '\"version\": \"2.1.0\"'; $c = $c -replace '\"appDescription\": \"HDMI\"', '\"appDescription\": \"!HDMI_NAME!\"'; Set-Content '!BUILD!\MagicRemoteService\appinfo.json' $c -NoNewline"
powershell -Command "(Get-Content '!BUILD!\Service\services.json' -Raw) -replace 'com.cathwyler.magicremoteservice.service', '!SVC_ID!' | Set-Content '!BUILD!\Service\services.json' -NoNewline"
powershell -Command "(Get-Content '!BUILD!\Service\package.json' -Raw) -replace 'com.cathwyler.magicremoteservice.service', '!SVC_ID!' | Set-Content '!BUILD!\Service\package.json' -NoNewline"

if not exist "!BUILD!\MagicRemoteService\config.json" (
    echo [ERROR] Failed to generate config.json!
    pause
    exit /b 1
)
echo   config.json generated

echo Packaging...
pushd "!BUILD!"
call ares-package MagicRemoteService Service -o .
if !errorlevel! neq 0 (
    echo Packaging failed!
    popd
    pause
    exit /b 1
)

echo Installing on TV...
for %%f in (*.ipk) do call ares-install "%%f" -d "!TV_DEVICE!"
if !errorlevel! neq 0 (
    echo Installation failed! Check:
    echo   - Developer Mode enabled on TV
    echo   - Key Server turned on
    echo   - Device configured: ares-setup-device --list
    popd
    pause
    exit /b 1
)
popd

echo.
echo [OK] TV App installed!
echo.

set /p LAUNCH="Launch now? [Y/n]: "
if /i not "!LAUNCH!"=="n" (
    call ares-launch "!APP_ID!" -d "!TV_DEVICE!"
    echo Launched!
)

echo.
echo NOTE: Reboot TV if you see "service is busy" errors.
rmdir /s /q "!BUILD!" 2>nul

:: ========================================
:done
echo.
echo ========================================
echo   All Done!
echo ========================================
echo.
echo   Web Settings:  http://localhost:41231
if defined TV_DEVICE (
    echo   Uninstall TV:  ares-install -r !APP_ID! -d !TV_DEVICE!
    echo   Launch TV:     ares-launch !APP_ID! -d !TV_DEVICE!
)
echo   Stop service:  net stop MagicRemoteService
echo   Start service: net start MagicRemoteService
echo.
pause
exit /b 0
