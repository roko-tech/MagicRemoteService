<#
.SYNOPSIS
Installs or updates MagicRemoteService on this PC and deploys its app to an LG webOS TV.

.DESCRIPTION
Uses MagicRemoteService.exe from next to this script (a release download) or from
target\release (a source checkout, built with cargo when missing). The exe copies itself to
C:\Program Files\MagicRemoteService and registers the Windows service. The TV is then paired
and the TV app configured and installed through the service's local API, which uses Windows'
own OpenSSH client (no Node.js or webOS CLI needed).

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
    # The firewall only lets the TV in on private and domain networks
    $public = @(Get-NetConnectionProfile -ErrorAction SilentlyContinue | Where-Object NetworkCategory -eq 'Public')
    foreach ($n in $public) {
        Write-Host "Windows treats the network '$($n.Name)' ($($n.InterfaceAlias)) as Public, so the TV can't connect through it." -ForegroundColor Yellow
        Write-Host "If it is your home network, set it to Private: Settings > Network & internet > (the connection) > Network profile type." -ForegroundColor Yellow
    }
}

function Select-Tv {
    $tvs = @((Invoke-Api GET '/api/tvs').tvs)
    if ($tvs.Count -gt 0) {
        Write-Host 'Paired TVs:'
        foreach ($tv in $tvs) { Write-Host ("  {0}  {1}{2}" -f $tv.name, $tv.ip, $(if ($tv.default) { '  (default)' } else { '' })) }
        $name = (Read-Host 'TV to install on (Enter for the default, P to pair another)').Trim()
        if (-not $name) { return }
        if ($name -ne 'P') {
            $null = Invoke-Api POST '/api/tvs/default' @{ name = $name }
            return
        }
    }
    Write-Host @'
To pair a TV: on the TV, install the Developer Mode app from the LG Content Store, sign in,
and turn on Dev Mode Status and Key Server. The app shows the passphrase to enter here.
Turn Key Server off once the TV is paired.
'@
    $ip = (Read-Host 'TV IP address').Trim()
    $passphrase = (Read-Host 'Passphrase').Trim()
    $result = Invoke-Api POST '/api/tvs/pair' @{ ip = $ip; passphrase = $passphrase }
    if (-not $result.success) { throw $result.message }
    Write-Host $result.message -ForegroundColor Green
}

function Install-Tv {
    Write-Step 'Deploying the TV app'
    $null = Wait-Service
    Select-Tv

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
    # The TV must present a token to connect; without one any device on the network could.
    # The service never shows its token (it reports hasToken) and keeps it when none is sent.
    $config.Remove('hasToken')
    if (-not $config.token -and -not $saved.hasToken) { $config.token = [guid]::NewGuid().ToString('N') }

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
