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
  live-engine-start  the 2.12.0 scratch binary on a private fresh data dir (C:\Users\user\tools\w189-data) as a one-shot interactive task, wait for its token and :7777
  live-run [S] [text_with_underscores]  ask.ps1 run -AskProbe S -CatchUp -Send text on that engine (THEUTH_DATA_DIR set)
  live-engine-stop   end the task, stop that one engine pid, show the engine's desk/turn/focus lines
  tail PATH [N]   the last N lines of any text file (diagnostics)
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

$scratchExe = 'C:\Users\user\tools\theuth-test\2.12.0\t.exe'
$liveDir = 'C:\Users\user\tools\w189-data'

function LiveStop {
    schtasks /end /tn w189-engine-once 2>&1 | Out-Null
    schtasks /delete /tn w189-engine-once /f 2>&1 | Out-Null
    $pidFile = Join-Path $liveDir 'engine.pid'
    if (Test-Path $pidFile) {
        $enginePid = [int] (Get-Content $pidFile -ErrorAction SilentlyContinue | Select-Object -First 1)
        $p = Get-Process -Id $enginePid -ErrorAction SilentlyContinue
        if ($p -and $p.Path -eq $scratchExe) { Stop-Process -Id $enginePid -Force; Say "stopped the private engine pid=$enginePid" }
        else { Say "private engine pid=$enginePid is not running" }
    }
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
    'tail' {
        $p = if ($Rest.Count -ge 1) { $Rest[0] } else { '' }
        $n = if ($Rest.Count -ge 2) { [int] $Rest[1] } else { 30 }
        if (-not $p -or -not (Test-Path $p)) { Say "no such file: $p"; break }
        Say "tail $p ($((Get-Item $p).Length) bytes)"
        Get-Content $p -Tail $n | ForEach-Object { Say "  $((($_ -replace '\x1b\[[0-9;]*m', ''))[0..300] -join '')" }
    }
    'live-engine-start' {
        # The default data dir was migrated by a newer engine (theuth-test-engine crash-loops on "migration 37"), so the
        # 2.12.0 scratch binary runs here on a private, fresh data dir: its own token, its own transcript, no model.
        LiveStop
        if (Test-Path $liveDir) { Remove-Item $liveDir -Recurse -Force -ErrorAction SilentlyContinue }
        New-Item -ItemType Directory -Force $liveDir | Out-Null
        $launcher = Join-Path $liveDir 'engine-once.ps1'
        $tpl = @'
$env:THEUTH_DATA_DIR = '__DD__'
foreach ($v in 'THEUTH_RPC__LISTEN', 'THEUTH_LAUNCH') { Remove-Item "Env:$v" -ErrorAction SilentlyContinue }
$p = Start-Process -FilePath '__EXE__' -ArgumentList '--headless' -PassThru -WindowStyle Hidden -RedirectStandardOutput '__DD__\engine-stdout.log' -RedirectStandardError '__DD__\engine-stderr.log'
$p.Id | Set-Content '__DD__\engine.pid'
$p.WaitForExit()
'@
        $tpl.Replace('__DD__', $liveDir).Replace('__EXE__', $scratchExe) | Set-Content -Encoding utf8 $launcher
        schtasks /create /tn w189-engine-once /tr "conhost.exe --headless pwsh.exe -NoProfile -File $launcher" /sc once /st 23:59 /it /f 2>&1 | ForEach-Object { Say "schtasks: $_" }
        schtasks /run /tn w189-engine-once 2>&1 | ForEach-Object { Say "schtasks: $_" }
        $up = $false
        $i = 0
        while ($i -lt 45 -and -not $up) {
            Start-Sleep 1
            $i++
            if (Test-Path (Join-Path $liveDir 'token')) {
                try { $c = [Net.Sockets.TcpClient]::new(); $c.Connect('127.0.0.1', 7777); $c.Close(); $up = $true } catch { }
            }
        }
        Say "private engine up=$up after $i s, data dir $liveDir"
        foreach ($f in 'engine.json', 'engine.pid') {
            $p = Join-Path $liveDir $f
            if (Test-Path $p) { Say "$f = $((Get-Content $p -Raw).Trim())" } else { Say "$f missing" }
        }
        foreach ($f in 'engine-stdout.log', 'engine-stderr.log') {
            $p = Join-Path $liveDir $f
            if (Test-Path $p) { Say "$f $((Get-Item $p).Length) bytes"; Get-Content $p -Tail 6 | ForEach-Object { Say "  $((($_ -replace '\x1b\[[0-9;]*m', ''))[0..240] -join '')" } }
        }
    }
    'live-run' {
        # ask.ps1 run on the private engine: the Ask bar resolves the desk, load_session (--ask-catchup), a typed send (--ask-send).
        $secs = if ($Rest.Count -ge 1) { [double] $Rest[0] } else { 45 }
        $text = if ($Rest.Count -ge 2) { $Rest[1] } else { 'hello_from_the_189w_probe' }
        $env:THEUTH_DATA_DIR = $liveDir
        & (Join-Path $PSScriptRoot 'ask.ps1') run -AskProbe $secs -CatchUp -Send $text -Shareable *>&1 | ForEach-Object { Write-Output "$_" }
    }
    'live-engine-stop' {
        LiveStop
        foreach ($f in 'engine-stdout.log', 'engine-stderr.log') {
            $p = Join-Path $liveDir $f
            if (Test-Path $p) {
                Say "$f $((Get-Item $p).Length) bytes — lines about desk, turn, focus, session, error:"
                Get-Content $p | Where-Object { $_ -match 'set_event_focus|desk|load_session|send_input|TurnFinished|Error|error|WARN' } | Select-Object -Last 14 | ForEach-Object { Say "  $((($_ -replace '\x1b\[[0-9;]*m', ''))[0..300] -join '')" }
            }
        }
    }
    'companion-log' {
        $n = if ($Rest.Count -ge 1) { [int] $Rest[0] } else { 60 }
        $dir = Join-Path $env:LOCALAPPDATA 'theuth\companion\logs'
        $file = Get-ChildItem $dir -File -ErrorAction SilentlyContinue | Where-Object { $_.Name -like 'companion*.log' } | Sort-Object LastWriteTime | Select-Object -Last 1
        if (-not $file) { Say "no companion log in $dir"; break }
        Say "log $($file.FullName)"
        Get-Content $file.FullName | Where-Object { $_ -match 'ask|set_event_focus|catch-up|watchdog|TextDelta|desk' } | Select-Object -Last $n | ForEach-Object { Say $_ }
    }
    default { Say 'usage: engine | engine-start | engine-errors | live-engine-start | live-run [S] [text] | live-engine-stop | tail PATH [N] | companion-log [N]' }
}
