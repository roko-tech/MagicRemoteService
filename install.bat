@echo off
rem Installs or updates MagicRemoteService and deploys its TV app. The work is done by
rem install.ps1, which asks for administrator rights itself. Options are passed through,
rem for example: install.bat -Part Tv -Hdmi 4
powershell.exe -NoProfile -ExecutionPolicy Bypass -File "%~dp0install.ps1" %*
if errorlevel 1 pause
