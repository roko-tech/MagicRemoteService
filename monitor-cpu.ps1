# MagicRemoteService CPU Monitor
# Uses WMI PercentProcessorTime for accurate SYSTEM process measurement
# Run as Admin: powershell -ExecutionPolicy Bypass -File D:\MyScripts\MagicRemoteService\monitor-cpu.ps1

$logFile = "D:\MyScripts\MagicRemoteService\cpu-log.csv"
$processName = "MagicRemoteService"

if (-not (Test-Path $logFile)) {
    "Timestamp,CPU%,WorkingSetMB,Threads,Status" | Out-File $logFile -Encoding utf8
}

Write-Host "Monitoring $processName every 30 seconds..."
Write-Host "Log file: $logFile"
Write-Host "Press Ctrl+C to stop."

# Get ALL PIDs for MagicRemoteService
$procs = Get-Process -Name $processName -ErrorAction SilentlyContinue
if (-not $procs) { Write-Host "Process not found!"; exit }
if ($procs -isnot [array]) { $procs = @($procs) }
$pids = $procs | ForEach-Object { $_.Id }
Write-Host "Tracking PIDs: $($pids -join ', ')"

# Initial WMI samples for each PID
$prevSamples = @{}
foreach ($p in $pids) {
    $s = Get-CimInstance Win32_PerfRawData_PerfProc_Process -Filter "IDProcess=$p" -ErrorAction SilentlyContinue
    if ($s) { $prevSamples[$p] = $s }
}
$t1 = (Get-CimInstance Win32_PerfRawData_PerfOS_System).Timestamp_Sys100NS

Start-Sleep -Seconds 30

while ($true) {
    $timestamp = Get-Date -Format "yyyy-MM-dd HH:mm:ss"
    $t2 = (Get-CimInstance Win32_PerfRawData_PerfOS_System).Timestamp_Sys100NS
    $timeDelta = $t2 - $t1
    $totalCpu = 0
    $totalMem = 0
    $totalThreads = 0
    $details = @()

    foreach ($p in $pids) {
        $s2 = Get-CimInstance Win32_PerfRawData_PerfProc_Process -Filter "IDProcess=$p" -ErrorAction SilentlyContinue
        if ($s2 -and $prevSamples.ContainsKey($p)) {
            $cpuDelta = $s2.PercentProcessorTime - $prevSamples[$p].PercentProcessorTime
            $cpuPct = [math]::Round(($cpuDelta / $timeDelta) * 100, 1)
            $memMB = [math]::Round($s2.WorkingSetPrivate / 1MB, 1)
            $thr = $s2.ThreadCount
            $totalCpu += $cpuPct
            $totalMem += $memMB
            $totalThreads += $thr
            $details += "PID $p : ${cpuPct}% ${memMB}MB ${thr}thr"
            $prevSamples[$p] = $s2
        }
    }
    $t1 = $t2

    $line = "$timestamp,$totalCpu,$totalMem,$totalThreads,Running"
    $line | Out-File $logFile -Append -Encoding utf8

    $detailStr = $details -join " | "
    if ($totalCpu -gt 10) {
        Write-Host "[$timestamp] WARNING: Total CPU=$totalCpu% MEM=${totalMem}MB [$detailStr]" -ForegroundColor Red
    } elseif ($totalCpu -gt 5) {
        Write-Host "[$timestamp] ELEVATED: Total CPU=$totalCpu% MEM=${totalMem}MB [$detailStr]" -ForegroundColor Yellow
    } else {
        Write-Host "[$timestamp] OK: Total CPU=$totalCpu% MEM=${totalMem}MB [$detailStr]"
    }

    Start-Sleep -Seconds 30
}
