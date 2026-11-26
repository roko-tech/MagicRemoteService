<#
.SYNOPSIS
Installs or updates MagicRemoteService on this PC and deploys its app to an LG webOS TV.

.DESCRIPTION
Uses MagicRemoteService.exe from next to this script (a release download) or from
target\release (a source checkout, built with cargo when missing). The exe copies itself to
C:\Program Files\MagicRemoteService and registers the Windows service. The TV app is then
configured and installed through the service's local API, which runs the webOS CLI (ares)
as the signed-in user.

.PARAMETER Part
Both (default), Pc (service only) or Tv (TV app only).

.PARAMETER Hdmi
The TV's HDMI input the PC is connected to (1-4). Asked when not given.

.EXAMPLE
.\install.ps1
.\install.ps1 -Part Tv -Hdmi 4
#>
[CmdletBinding()]
param(
    [ValidateSet('Both', 'Pc', 'Tv')]
    [string]$Part,
    [ValidateRange(1, 4)]
    [int]$Hdmi,
    # Set when the script restarted itself elevated in a new window, so it waits before closing
    [switch]$Relaunched
)

$ErrorActionPreference = 'Stop'
$Releases = 'https://github.com/roko-tech/MagicRemoteService/releases'

function Test-Admin {
    $id = [Security.Principal.WindowsIdentity]::GetCurrent()
    ([Security.Principal.WindowsPrincipal]$id).IsInRole([Security.Principal.WindowsBuiltInRole]::Administrator)
}

function Write-Step([string]$Text) {
    Write-Host ''
    Write-Host "== $Text" -ForegroundColor Cyan
}

function Update-SessionPath {
    $env:Path = [Environment]::GetEnvironmentVariable('Path', 'Machine') + ';' + [Environment]::GetEnvironmentVariable('Path', 'User')
}

function Find-ServiceExe {
    foreach ($p in @((Join-Path $PSScriptRoot 'MagicRemoteService.exe'), (Join-Path $PSScriptRoot 'target\release\MagicRemoteService.exe'))) {
        if (Test-Path -LiteralPath $p) { return $p }
    }
    if ((Test-Path -LiteralPath (Join-Path $PSScriptRoot 'Cargo.toml')) -and (Get-Command cargo -ErrorAction SilentlyContinue)) {
        Write-Host 'Building MagicRemoteService.exe with cargo...'
        Push-Location $PSScriptRoot
        try { & cargo build --release --locked } finally { Pop-Location }
        if ($LASTEXITCODE -ne 0) { throw 'cargo build failed.' }
        return (Join-Path $PSScriptRoot 'target\release\MagicRemoteService.exe')
    }
    throw "MagicRemoteService.exe was not found. Download it from $Releases and put it next to this script, or install Rust (https://rustup.rs) to build it."
}

function Get-ApiBase {
    $port = 41230
    $key = Get-ItemProperty -Path 'HKLM:\SOFTWARE\WOW6432Node\MagicRemoteService' -ErrorAction SilentlyContinue
    if ($key -and $key.Port) { $port = [int]$key.Port }
    "http://127.0.0.1:$($port + 1)"
}

function Invoke-Api([string]$Method, [string]$Path, $Body) {
    $request = @{ Uri = "$script:Api$Path"; Method = $Method; UseBasicParsing = $true; TimeoutSec = 600 }
    if ($Method -eq 'POST') {
        # The service refuses POSTs without this header (it stops cross-site requests)
        $request.Headers = @{ 'X-Requested-With' = 'MagicRemoteService' }
        $request.ContentType = 'application/json'
        $json = if ($null -ne $Body) { $Body | ConvertTo-Json -Depth 5 } else { '{}' }
        $request.Body = [Text.Encoding]::UTF8.GetBytes($json)
    }
    (Invoke-WebRequest @request).Content | ConvertFrom-Json
}

function Wait-Service {
    for ($i = 0; $i -lt 30; $i++) {
        try { return (Invoke-Api GET '/api/settings') } catch { Start-Sleep -Seconds 1 }
    }
    throw "The service is not answering at $script:Api. Check it with: MagicRemoteService.exe status"
}

function Install-Pc {
    Write-Step 'Installing the PC service'
    $exe = Find-ServiceExe
    & $exe install 2>&1 | Out-Host
    if ($LASTEXITCODE -ne 0) { throw 'The service could not be installed (see above).' }
}

function Confirm-Yes([string]$Question) {
    (Read-Host "$Question [Y/n]") -notmatch '^\s*n'
}

function Assert-Ares {
    if (-not (Get-Command node -ErrorAction SilentlyContinue)) {
        if ((Get-Command winget -ErrorAction SilentlyContinue) -and (Confirm-Yes 'Node.js is needed to deploy the TV app. Install Node.js LTS with winget now?')) {
            & winget install --exact --id OpenJS.NodeJS.LTS --accept-source-agreements --accept-package-agreements | Out-Host
            Update-SessionPath
        }
        if (-not (Get-Command node -ErrorAction SilentlyContinue)) {
            throw 'Install Node.js LTS from https://nodejs.org, then run this again.'
        }
    }
    if (-not (Get-Command ares-setup-device -ErrorAction SilentlyContinue)) {
        if (Confirm-Yes 'The webOS TV CLI (@webos-tools/cli) is needed. Install it with npm now?') {
            & npm install --global '@webos-tools/cli' | Out-Host
            Update-SessionPath
        }
        if (-not (Get-Command ares-setup-device -ErrorAction SilentlyContinue)) {
            throw 'Install the webOS TV CLI with: npm install --global @webos-tools/cli'
        }
    }
}

function Select-TvDevice {
    $list = & ares-setup-device --list 2>&1 | Out-String
    Write-Host $list
    # The emulator entries point at 127.0.0.1; a real TV has a LAN address
    if ($list -notmatch '@(?!127\.)\d{1,3}(\.\d{1,3}){3}:\d+') {
        throw @'
No TV is set up in the webOS CLI yet. On the TV, install and open the Developer Mode app, turn on
Dev Mode Status and Key Server, then on this PC run:
    ares-setup-device            (add a device: the TV's IP, port 9922, user prisoner)
    ares-novacom --device <name> --getkey   (enter the passphrase the Developer Mode app shows)
and run this installer again.
'@
    }
    $name = (Read-Host 'TV to install on (Enter for the one marked default)').Trim()
    if ($name) {
        & ares-setup-device --default $name | Out-Host
        if ($LASTEXITCODE -ne 0) { throw "ares-setup-device does not know a TV named '$name'." }
    }
}

function Install-Tv {
    Write-Step 'Deploying the TV app'
    Assert-Ares
    $null = Wait-Service
    Select-TvDevice

    $pc = Invoke-Api GET '/api/pcinfo'
    $saved = Invoke-Api GET '/api/config'
    $interfaces = @($pc.interfaces)
    if ($interfaces.Count -eq 0) { throw 'The service found no network interface.' }
    Write-Host 'Networks on this PC (the one with a gateway is listed first):'
    for ($i = 0; $i -lt $interfaces.Count; $i++) {
        Write-Host ("  {0}) {1}  {2}" -f $i, $interfaces[$i].ip, $interfaces[$i].name)
    }
    $pick = (Read-Host 'Network the TV is on [0]').Trim()
    if ($pick -notmatch '^\d*$' -or [int]("0$pick") -ge $interfaces.Count) { throw "Choose a network number from 0 to $($interfaces.Count - 1)." }
    $net = $interfaces[[int]("0$pick")]

    $hdmiNumber = $Hdmi
    if (-not $hdmiNumber) {
        $current = if ($saved.inputId -match '^HDMI_(\d)$') { $Matches[1] } else { '' }
        $answer = (Read-Host "TV HDMI input the PC is on (1-4) [$current]").Trim()
        if (-not $answer) { $answer = $current }
        if ($answer -notmatch '^[1-4]$') { throw 'The HDMI input must be 1, 2, 3 or 4.' }
        $hdmiNumber = [int]$answer
    }

    # Keep what the web UI saved (cursor speed and so on); set what this PC and input decide
    $config = [ordered]@{ overlay = $true; inputDirect = $true; longClick = 1500; cursorSpeed = 1.0; extend = $true }
    if ($saved) { foreach ($p in $saved.PSObject.Properties) { $config[$p.Name] = $p.Value } }
    $config.inputId = "HDMI_$hdmiNumber"
    $config.inputAppId = "com.webos.app.hdmi$hdmiNumber"
    $config.inputName = "HDMI $hdmiNumber"
    $config.inputSource = "ext://hdmi:hdmi$hdmiNumber"
    $config.appId = "com.cathwyler.magicremoteservice.hdmi$hdmiNumber"
    $config.ip = $net.ip
    $config.mask = $net.mask
    $config.mac = $net.mac
    $config.port = [int]$pc.port
    # The TV must present this token to connect; without one any device on the network could
    if (-not $config.token) { $config.token = [guid]::NewGuid().ToString('N') }

    $null = Invoke-Api POST '/api/config' $config
    Write-Host 'Packaging and installing the TV app (this can take a minute)...'
    $result = Invoke-Api POST '/api/reinstall'
    if (-not $result.success) { throw "TV app install failed: $($result.message)" }
    Write-Host $result.message -ForegroundColor Green
}

try {
    if (-not (Test-Admin)) {
        # Installing a service needs administrator rights: start again elevated, with the same choices
        $arguments = @('-NoProfile', '-ExecutionPolicy', 'Bypass', '-File', "`"$PSCommandPath`"", '-Relaunched')
        if ($Part) { $arguments += @('-Part', $Part) }
        if ($Hdmi) { $arguments += @('-Hdmi', $Hdmi) }
        Start-Process -FilePath powershell.exe -Verb RunAs -ArgumentList $arguments -Wait
        exit
    }
    if (-not $Part) {
        Write-Host 'MagicRemoteService installer'
        Write-Host '  1) PC service and TV app'
        Write-Host '  2) PC service only'
        Write-Host '  3) TV app only'
        $Part = @{ '' = 'Both'; '1' = 'Both'; '2' = 'Pc'; '3' = 'Tv' }[(Read-Host 'Choose [1]').Trim()]
        if (-not $Part) { throw 'Choose 1, 2 or 3.' }
    }
    $script:Api = Get-ApiBase
    if ($Part -ne 'Tv') { Install-Pc; $script:Api = Get-ApiBase }
    if ($Part -ne 'Pc') { Install-Tv }
    Write-Host ''
    Write-Host "Done. Settings: http://localhost:$(([uri]$script:Api).Port)/" -ForegroundColor Green
    $code = 0
} catch {
    Write-Host ''
    Write-Host $_.Exception.Message -ForegroundColor Red
    $code = 1
}
if ($Relaunched) { $null = Read-Host 'Press Enter to close' }
exit $code
