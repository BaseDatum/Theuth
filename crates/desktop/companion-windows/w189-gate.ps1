#requires -Version 7.0
<#
.SYNOPSIS
  Design 189w gate helper for build-B (run through the build service's `script` action, in the interactive session). Every
  output line starts with "G189: " so the job log can be filtered.

.DESCRIPTION
  engine          the scratch engine's scheduled task (state, last result, action), its process, engine.json and the tail of
                  its newest log — why the Ask bar says "theuth engine unreachable"
  engine-start    start the task, wait 12 s, then the same report
  engine-errors   the newest engine log's last ERROR/WARN/panic lines and engine-run.ps1 (why the engine restarts in a loop)
  companion-log N today's companion log lines about the Ask (focus, first TextDelta, catch-up, watchdog, finish), last N (default 60)
#>
param(
    [Parameter(Position = 0)] [string] $Action = 'help',
    [Parameter(ValueFromRemainingArguments = $true)] [string[]] $Rest
)
[Console]::OutputEncoding = [Text.Encoding]::UTF8
$ErrorActionPreference = 'Continue'
if (-not $Rest) { $Rest = @() }
function Say([string] $s) { Write-Output "G189: $s" }
$dataDir = if ($env:THEUTH_DATA_DIR) { $env:THEUTH_DATA_DIR } else { Join-Path $env:USERPROFILE '.theuth' }

function EngineReport {
    $t = Get-ScheduledTask -TaskName 'theuth-test-engine' -ErrorAction SilentlyContinue
    if (-not $t) { Say 'no scheduled task theuth-test-engine' }
    else {
        $i = Get-ScheduledTaskInfo -TaskName 'theuth-test-engine'
        Say "task state=$($t.State) lastRun=$($i.LastRunTime) lastResult=$($i.LastTaskResult)"
        foreach ($a in $t.Actions) { Say "task action: $($a.Execute) $($a.Arguments)" }
    }
    $procs = @(Get-Process -ErrorAction SilentlyContinue | Where-Object { $_.ProcessName -in @('t', 'theuth-desktop', 'Theuth.Companion') })
    if ($procs.Count -eq 0) { Say 'no t / theuth-desktop / Theuth.Companion process' }
    foreach ($p in $procs) { Say "proc $($p.ProcessName) pid=$($p.Id) start=$($p.StartTime) $($p.Path)" }
    $ej = Join-Path $dataDir 'engine.json'
    if (Test-Path $ej) { $e = Get-Content $ej -Raw | ConvertFrom-Json; Say "engine.json listen=$($e.listen) pid=$($e.pid) version=$($e.version)" } else { Say "engine.json missing in $dataDir" }
    $logs = Join-Path $dataDir 'logs'
    $newest = if (Test-Path $logs) { Get-ChildItem $logs -File -ErrorAction SilentlyContinue | Where-Object { $_.Name -like 'theuth.log*' } | Sort-Object LastWriteTime | Select-Object -Last 1 }
    if ($newest) {
        Say "engine log $($newest.FullName) ($($newest.LastWriteTime))"
        Get-Content $newest.FullName -Tail 12 | ForEach-Object { Say "  $((($_ -replace '\x1b\[[0-9;]*m', ''))[0..220] -join '')" }
    } else { Say "no engine log under $logs" }
}

switch ($Action) {
    'engine' { EngineReport }
    'engine-start' {
        Start-ScheduledTask -TaskName 'theuth-test-engine' -ErrorAction Continue
        Start-Sleep 12
        EngineReport
    }
    'engine-errors' {
        $logs = Join-Path $dataDir 'logs'
        $newest = Get-ChildItem $logs -File -ErrorAction SilentlyContinue | Where-Object { $_.Name -like 'theuth.log*' } | Sort-Object LastWriteTime | Select-Object -Last 1
        if (-not $newest) { Say "no engine log under $logs"; break }
        Say "engine log $($newest.FullName) ($($newest.LastWriteTime), $($newest.Length) bytes)"
        Get-Content $newest.FullName -Tail 600 | Where-Object { $_ -match 'ERROR|WARN|panic|fatal|Error|failed|exit' } | Select-Object -Last 14 | ForEach-Object { Say "  $((($_ -replace '\x1b\[[0-9;]*m', ''))[0..300] -join '')" }
        $run = 'C:\Users\user\tools\theuth-test\engine-run.ps1'
        if (Test-Path $run) { Say "engine-run.ps1:"; Get-Content $run | ForEach-Object { Say "  $_" } }
    }
    'companion-log' {
        $n = if ($Rest.Count -ge 1) { [int] $Rest[0] } else { 60 }
        $dir = Join-Path $env:LOCALAPPDATA 'theuth\companion\logs'
        $file = Get-ChildItem $dir -File -ErrorAction SilentlyContinue | Where-Object { $_.Name -like 'companion*.log' } | Sort-Object LastWriteTime | Select-Object -Last 1
        if (-not $file) { Say "no companion log in $dir"; break }
        Say "log $($file.FullName)"
        Get-Content $file.FullName | Where-Object { $_ -match 'ask|set_event_focus|catch-up|watchdog|TextDelta|desk' } | Select-Object -Last $n | ForEach-Object { Say $_ }
    }
    default { Say 'usage: engine | engine-start | engine-errors | companion-log [N]' }
}
