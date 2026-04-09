@echo off
setlocal enabledelayedexpansion

echo ========================================
echo   MagicRemoteService Installer v2.0.0
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
:: PREREQUISITES
:: ========================================
echo Checking prerequisites...
echo.

:: Check Node.js
where node >nul 2>&1
if %errorlevel% neq 0 (
    echo [MISSING] Node.js
    echo Installing Node.js via winget...
    winget install --id OpenJS.NodeJS.LTS -e --accept-source-agreements --accept-package-agreements >nul 2>&1
    if %errorlevel% neq 0 (
        echo Failed to install Node.js. Download from https://nodejs.org/
        pause
        exit /b 1
    )
    echo [INSTALLED] Node.js
    :: Refresh PATH
    set "PATH=%APPDATA%\npm;C:\Program Files\nodejs;%PATH%"
) else (
    for /f "tokens=*" %%v in ('node --version 2^>nul') do echo [OK] Node.js %%v
)

:: Check ares-cli
where ares-package >nul 2>&1
if %errorlevel% neq 0 (
    echo [MISSING] @webos-tools/cli
    echo Installing webOS CLI tools...
    call npm install -g @webos-tools/cli >nul 2>&1
    echo [INSTALLED] @webos-tools/cli
) else (
    echo [OK] @webos-tools/cli
)

:: Check MSBuild
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
    echo [MISSING] Visual Studio 2022 / Build Tools
    echo Installing VS Build Tools via winget...
    winget install --id Microsoft.VisualStudio.2022.BuildTools -e --accept-source-agreements --accept-package-agreements --override "--add Microsoft.VisualStudio.Workload.ManagedDesktopBuildTools --includeRecommended --quiet --wait" >nul 2>&1
    for %%p in (
        "C:\Program Files (x86)\Microsoft Visual Studio\2022\BuildTools\MSBuild\Current\Bin\MSBuild.exe"
    ) do (
        if exist %%p set "MSBUILD=%%~p"
    )
    if not defined MSBUILD (
        echo Failed to install Build Tools. Download from https://visualstudio.microsoft.com/downloads/
        pause
        exit /b 1
    )
    echo [INSTALLED] VS Build Tools
) else (
    echo [OK] MSBuild
)

echo.
echo ========================================
echo   What do you want to install?
echo ========================================
echo   1) PC Service + TV App (full install)
echo   2) PC Service only
echo   3) TV App only
echo.
set /p CHOICE="Enter choice [1-3]: "

:: ========================================
:: PC SERVICE
:: ========================================
if "%CHOICE%"=="2" goto :pc_only
if "%CHOICE%"=="3" goto :tv_only

:pc_service
echo.
echo ========================================
echo   Building PC Service
echo ========================================

:: NuGet restore
if not exist packages (
    echo Restoring NuGet packages...
    where nuget >nul 2>&1
    if %errorlevel% equ 0 (
        nuget restore MagicRemoteService.sln >nul 2>&1
    ) else (
        echo Downloading NuGet...
        powershell -Command "Invoke-WebRequest -Uri https://dist.nuget.org/win-x86-commandline/latest/nuget.exe -OutFile nuget.exe" >nul 2>&1
        nuget.exe restore MagicRemoteService.sln >nul 2>&1
        del nuget.exe 2>nul
    )
)

echo Building...
"%MSBUILD%" MagicRemoteService.sln /p:Configuration=Release /t:Rebuild /v:minimal
if %errorlevel% neq 0 (
    echo Build failed!
    pause
    exit /b 1
)

set /p INSTALL_DIR="Install directory [C:\MagicRemoteService]: "
if "!INSTALL_DIR!"=="" set "INSTALL_DIR=C:\MagicRemoteService"

echo Installing to !INSTALL_DIR!...
if not exist "!INSTALL_DIR!" mkdir "!INSTALL_DIR!"

:: Stop existing service
net stop MagicRemoteService >nul 2>&1
timeout /t 2 /nobreak >nul

:: Copy files
copy /y "MagicRemoteService\bin\Release\MagicRemoteService.exe" "!INSTALL_DIR!\" >nul
copy /y "MagicRemoteService\bin\Release\MagicRemoteService.exe.config" "!INSTALL_DIR!\" >nul
copy /y "MagicRemoteService\bin\Release\*.dll" "!INSTALL_DIR!\" >nul
copy /y "settings-ui.html" "!INSTALL_DIR!\" >nul 2>nul
if exist "MagicRemoteService\bin\Release\es" (
    if not exist "!INSTALL_DIR!\es" mkdir "!INSTALL_DIR!\es"
    copy /y "MagicRemoteService\bin\Release\es\*" "!INSTALL_DIR!\es\" >nul
)
if exist "MagicRemoteService\bin\Release\fr" (
    if not exist "!INSTALL_DIR!\fr" mkdir "!INSTALL_DIR!\fr"
    copy /y "MagicRemoteService\bin\Release\fr\*" "!INSTALL_DIR!\fr\" >nul
)

:: Register service
echo Registering Windows service...
sc query MagicRemoteService >nul 2>&1
if %errorlevel% neq 0 (
    C:\Windows\Microsoft.NET\Framework64\v4.0.30319\InstallUtil.exe /i "!INSTALL_DIR!\MagicRemoteService.exe" >nul 2>&1
)

:: Firewall
echo Adding firewall rules...
netsh advfirewall firewall delete rule name="MagicRemoteService" >nul 2>&1
netsh advfirewall firewall add rule name="MagicRemoteService" dir=in action=allow protocol=TCP localport=41230 >nul 2>&1
netsh advfirewall firewall add rule name="MagicRemoteService Web UI" dir=in action=allow protocol=TCP localport=41231 >nul 2>&1

:: Start service
echo Starting service...
net start MagicRemoteService >nul 2>&1
timeout /t 2 /nobreak >nul

sc query MagicRemoteService | find "RUNNING" >nul 2>&1
if %errorlevel% equ 0 (
    echo [OK] PC Service installed and running!
    echo     Web UI: http://localhost:41231
) else (
    echo [WARNING] Service may not have started. Check Services.msc
)

if "%CHOICE%"=="2" goto :done

:: ========================================
:: TV APP
:: ========================================
:tv_only
echo.
echo ========================================
echo   Installing TV App
echo ========================================
echo.

:: List devices
echo Available TV devices:
call ares-setup-device --list 2>nul
echo.

set /p TV_DEVICE="TV device name (from list above, or type new name): "
set /p PC_IP="PC IP address [auto-discover]: "
set /p PC_MAC="PC MAC address (XX:XX:XX:XX:XX:XX): "
set /p SUBNET="Subnet mask [255.255.255.0]: "
if "!SUBNET!"=="" set "SUBNET=255.255.255.0"
set /p PORT="PC listen port [41230]: "
if "!PORT!"=="" set "PORT=41230"

echo.
echo HDMI port your PC is connected to:
echo   1) HDMI 1    2) HDMI 2    3) HDMI 3    4) HDMI 4
set /p HDMI_NUM="Enter [1-4]: "

if "%HDMI_NUM%"=="1" (set HDMI_ID=HDMI_1&set HDMI_SHORT=hdmi1&set HDMI_NAME=HDMI 1&set HDMI_SRC=ext://hdmi:hdmi1)
if "%HDMI_NUM%"=="2" (set HDMI_ID=HDMI_2&set HDMI_SHORT=hdmi2&set HDMI_NAME=HDMI 2&set HDMI_SRC=ext://hdmi:hdmi2)
if "%HDMI_NUM%"=="3" (set HDMI_ID=HDMI_3&set HDMI_SHORT=hdmi3&set HDMI_NAME=HDMI 3&set HDMI_SRC=ext://hdmi:hdmi3)
if "%HDMI_NUM%"=="4" (set HDMI_ID=HDMI_4&set HDMI_SHORT=hdmi4&set HDMI_NAME=HDMI 4&set HDMI_SRC=ext://hdmi:hdmi4)

set "APP_ID=com.cathwyler.magicremoteservice.!HDMI_SHORT!"
set "SVC_ID=!APP_ID!.service"

echo.
echo   TV: !TV_DEVICE! ^| PC: !PC_IP! ^| HDMI: !HDMI_NAME!
set /p CONFIRM="Proceed? [Y/n]: "
if /i "!CONFIRM!"=="n" goto :done

:: Build TV app
echo.
echo Preparing TV app...
set "BUILD=%TEMP%\mrs-tv-build"
if exist "!BUILD!" rmdir /s /q "!BUILD!"
mkdir "!BUILD!\MagicRemoteService"
mkdir "!BUILD!\Service"
xcopy /s /q "MagicRemoteService\Resources\TV\MagicRemoteService\*" "!BUILD!\MagicRemoteService\" >nul
xcopy /s /q "MagicRemoteService\Resources\TV\Service\*" "!BUILD!\Service\" >nul

:: Apply config
echo Applying configuration...
set "MJS=!BUILD!\MagicRemoteService\main.js"
set "SJS=!BUILD!\Service\service.js"
set "AJ=!BUILD!\MagicRemoteService\appinfo.json"
set "SVJ=!BUILD!\Service\services.json"
set "PJ=!BUILD!\Service\package.json"

powershell -Command "(Get-Content '!MJS!') -replace 'const strInputId = \"HDMI\"', 'const strInputId = \"!HDMI_ID!\"' -replace 'const strInputAppId = \"com.webos.app.hdmi\"', 'const strInputAppId = \"com.webos.app.!HDMI_SHORT!\"' -replace 'const strInputName = \"HDMI\"', 'const strInputName = \"!HDMI_NAME!\"' -replace 'const strInputSource = \"ext://hdmi\"', 'const strInputSource = \"!HDMI_SRC!\"' -replace 'const strMac = \"AA:AA:AA:AA:AA:AA\"', 'const strMac = \"!PC_MAC!\"' -replace 'const strAppId = \"com.cathwyler.magicremoteservice\"', 'const strAppId = \"!APP_ID!\"' | Set-Content '!MJS!'"

if not "!PC_IP!"=="" (
    powershell -Command "(Get-Content '!MJS!') -replace 'var strIP = \"127.0.0.1\"', 'var strIP = \"!PC_IP!\"' | Set-Content '!MJS!'"
)

powershell -Command "(Get-Content '!SJS!') -replace 'var strAppId = \"com.cathwyler.magicremoteservice\"', 'var strAppId = \"!APP_ID!\"' -replace 'var strInputAppId = \"com.webos.app.hdmi\"', 'var strInputAppId = \"com.webos.app.!HDMI_SHORT!\"' | Set-Content '!SJS!'"
powershell -Command "(Get-Content '!AJ!') -replace '\"id\": \"com.cathwyler.magicremoteservice\"', '\"id\": \"!APP_ID!\"' -replace '\"version\": \"1.0.0\"', '\"version\": \"2.0.0\"' -replace '\"appDescription\": \"HDMI\"', '\"appDescription\": \"!HDMI_NAME!\"' | Set-Content '!AJ!'"
powershell -Command "(Get-Content '!SVJ!') -replace 'com.cathwyler.magicremoteservice.service', '!SVC_ID!' | Set-Content '!SVJ!'"
powershell -Command "(Get-Content '!PJ!') -replace 'com.cathwyler.magicremoteservice.service', '!SVC_ID!' | Set-Content '!PJ!'"

:: Package
echo Packaging...
cd "!BUILD!"
call ares-package MagicRemoteService Service -o .
if %errorlevel% neq 0 (
    echo Packaging failed!
    cd "%~dp0"
    pause
    exit /b 1
)

:: Install
echo Installing on TV...
for %%f in (*.ipk) do call ares-install "%%f" -d "!TV_DEVICE!"
if %errorlevel% neq 0 (
    echo Installation failed! Make sure Developer Mode is enabled and Key Server is on.
    cd "%~dp0"
    pause
    exit /b 1
)

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

cd "%~dp0"
rmdir /s /q "!BUILD!" 2>nul

:: ========================================
:done
echo.
echo ========================================
echo   All Done!
echo ========================================
echo.
echo   Web Settings: http://localhost:41231
echo   To uninstall TV app: ares-install -r %APP_ID% -d %TV_DEVICE%
echo   To stop PC service:  net stop MagicRemoteService
echo.
pause

:pc_only
goto :pc_service
