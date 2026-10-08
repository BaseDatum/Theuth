#requires -Version 7.0
<#
.SYNOPSIS
  Design 134 W5 packaging/install gate helper for build-B (run through the build service's `script` action, which runs
  in the interactive session, so `launch` starts the installed app on the visible desktop).

.DESCRIPTION
  Actions (first argument). Every output line starts with "G5: " so the job log can be filtered.
    status          the packaging run (log file, step headers, errors, last exit), dist artifacts, free disk
    install         stop the scratch engine, run the newest dist setup.exe /S, check files, the theuth: scheme, the Run value
    launch          start the installed theuth-desktop.exe (RUST_LOG=info; stderr/stdout to C:\Users\user\tools\pkg\desktop-*.log)
    log [N]         companion/warning lines of desktop-stderr.log, then its last N lines and today's companion log (default 40)
    procs           theuth-desktop / Theuth.Companion / t processes with pid, start time and path
    reg             Run\TheuthCompanion, HKCU\Software\Classes\theuth, <data_dir>\companion.json
    uninstall       run the installed uninstall.exe /S, wait for the files to go, list what is left
    engine-start    start the scratch engine task (theuth-test-engine) again
    restore-clone   put the release clone back on main and pop the pkg-gate stash (refuses while theuth-pkg runs)
    pkg-start SHA   W6: package SHA without an ssh PTY — refuse while theuth-pkg runs; stash main's regenerated files (or
                    discard a detached build's), fetch, detach the release clone at SHA, start the theuth-pkg task;
                    follow it with `status`
    launch-task     W6: start the installed theuth-desktop.exe through a one-shot interactive scheduled task — no output
                    redirection, so nothing inherits this job's handles (`launch` redirects and blocks the service's job)
#>
param(
    [Parameter(Position = 0)] [string] $Action = 'help',
    [Parameter(ValueFromRemainingArguments = $true)] [string[]] $Rest
)
[Console]::OutputEncoding = [Text.Encoding]::UTF8
$ErrorActionPreference = 'Continue'
if (-not $Rest) { $Rest = @() }
$pkg = 'C:\Users\user\tools\pkg'
$clone = 'C:\Users\user\Develop\BaseDatum\theuth'
$app = Join-Path $env:LOCALAPPDATA 'theuth'
$dataDir = if ($env:THEUTH_DATA_DIR) { $env:THEUTH_DATA_DIR } else { Join-Path $env:USERPROFILE '.theuth' }
function Say([string] $s) { Write-Output "G5: $s" }
function Clip([string] $s, [int] $n = 200) { if ($null -eq $s) { return '' }; $s = $s -replace "`e\[[0-9;]*m", ''; return $s.Substring(0, [math]::Min($n, $s.Length)) }
function Procs {
    foreach ($n in 'theuth-desktop', 'Theuth.Companion', 't', 'theuth-documents-*') {
        Get-Process -Name $n -ErrorAction SilentlyContinue | ForEach-Object { Say "proc $($_.ProcessName) pid=$($_.Id) start=$($_.StartTime.ToString('HH:mm:ss')) path=$($_.Path)" }
    }
}
function Reg {
    $run = (Get-ItemProperty 'HKCU:\Software\Microsoft\Windows\CurrentVersion\Run' -ErrorAction SilentlyContinue).TheuthCompanion
    Say "Run\TheuthCompanion = $(if ($null -eq $run) { '<absent>' } else { $run })"
    $cmd = (Get-ItemProperty 'HKCU:\Software\Classes\theuth\shell\open\command' -ErrorAction SilentlyContinue).'(default)'
    Say "Classes\theuth exists=$(Test-Path 'HKCU:\Software\Classes\theuth') command=$cmd"
    $cj = Join-Path $dataDir 'companion.json'
    Say "companion.json ($cj) = $(if (Test-Path $cj) { (Get-Content $cj -Raw).Trim() } else { '<absent>' })"
}
switch ($Action) {
    'status' {
        $l = Get-Content (Join-Path $pkg 'current-log.txt') -ErrorAction SilentlyContinue
        Say "log $l"
        if ($l -and (Test-Path $l)) {
            Select-String -Path $l -Pattern '^==> |^=== |error(\[E\d+\])?:|FAILED|Fail |panicked' | Select-Object -Last 24 | ForEach-Object { Say ('  ' + (Clip $_.Line 180)) }
            Get-Content $l -Tail 3 | ForEach-Object { Say ('  tail: ' + (Clip $_ 180)) }
        }
        Say "last-exit=$(Get-Content (Join-Path $pkg 'last-exit.txt') -ErrorAction SilentlyContinue) task=$((Get-ScheduledTask -TaskName 'theuth-pkg' -ErrorAction SilentlyContinue).State)"
        Get-ChildItem (Join-Path $clone 'target\release\dist') -File -ErrorAction SilentlyContinue | ForEach-Object { Say "dist $($_.Name) $([math]::Round($_.Length/1MB,1)) MB $($_.LastWriteTime.ToString('HH:mm:ss'))" }
        Say "free $([math]::Round((Get-PSDrive C).Free/1GB,1)) GB; clone head $(git -C $clone rev-parse --short HEAD)"
    }
    'install' {
        $setup = Get-ChildItem (Join-Path $clone 'target\release\dist') -Filter 'theuth_*_windows-x86_64-setup.exe' -ErrorAction SilentlyContinue | Sort-Object LastWriteTime | Select-Object -Last 1
        if (-not $setup) { Say 'install: no setup.exe in dist'; exit 1 }
        Say "setup $($setup.Name) $([math]::Round($setup.Length/1MB,1)) MB built $($setup.LastWriteTime.ToString('HH:mm:ss'))"
        Stop-ScheduledTask -TaskName 'theuth-test-engine' -ErrorAction SilentlyContinue
        Get-CimInstance Win32_Process -Filter "Name='t.exe'" | Where-Object { $_.ExecutablePath -like '*theuth-test*' } | ForEach-Object { Stop-Process -Id $_.ProcessId -Force; Say "stopped scratch t.exe pid=$($_.ProcessId)" }
        Start-Sleep 2
        Procs
        $t0 = Get-Date
        $p = Start-Process -FilePath $setup.FullName -ArgumentList '/S' -PassThru -Wait
        Say "install exit=$($p.ExitCode) in $([int]((Get-Date) - $t0).TotalSeconds) s"
        Say "app dir $app exists=$(Test-Path $app)"
        Say "companion exe=$(Test-Path (Join-Path $app 'companion\Theuth.Companion.exe')) files=$(@(Get-ChildItem (Join-Path $app 'companion') -Recurse -File -ErrorAction SilentlyContinue).Count)"
        Say "t.exe=$(Test-Path (Join-Path $app 't.exe')) onnxruntime.dll=$(Test-Path (Join-Path $app 'onnxruntime.dll')) theuth-desktop.exe=$(Test-Path (Join-Path $app 'theuth-desktop.exe')) uninstall.exe=$(Test-Path (Join-Path $app 'uninstall.exe'))"
        Reg
    }
    'launch' {
        $exe = Join-Path $app 'theuth-desktop.exe'
        if (-not (Test-Path $exe)) { Say "launch: $exe missing"; exit 1 }
        $env:RUST_LOG = 'info'
        $p = Start-Process -FilePath $exe -PassThru -WorkingDirectory $app -RedirectStandardError (Join-Path $pkg 'desktop-stderr.log') -RedirectStandardOutput (Join-Path $pkg 'desktop-stdout.log')
        Say "launched theuth-desktop pid=$($p.Id) at $(Get-Date -Format 'HH:mm:ss')"
        Start-Sleep 15
        Procs
    }
    'log' {
        $n = if ($Rest.Count -gt 0) { [int]$Rest[0] } else { 40 }
        $se = Join-Path $pkg 'desktop-stderr.log'
        if (Test-Path $se) {
            Select-String -Path $se -Pattern 'companion|TerminateProcess|single|deep|navigate|WARN|ERROR|panick' | Select-Object -Last 40 | ForEach-Object { Say ('  ' + (Clip $_.Line)) }
            Say "-- last $n lines of desktop-stderr.log"
            Get-Content $se -Tail $n | ForEach-Object { Say ('  ' + (Clip $_)) }
        } else { Say "no $se" }
        $cl = Join-Path $app ('companion\logs\companion-{0:yyyy-MM-dd}.log' -f (Get-Date))
        if (Test-Path $cl) { Say "-- last $n lines of $cl"; Get-Content $cl -Tail $n | ForEach-Object { Say ('  ' + (Clip $_)) } } else { Say "no $cl" }
    }
    'procs' { Procs }
    'reg' { Reg }
    'uninstall' {
        $un = Join-Path $app 'uninstall.exe'
        if (-not (Test-Path $un)) { Say "uninstall: $un missing"; Reg; exit 1 }
        $t0 = Get-Date
        Start-Process -FilePath $un -ArgumentList '/S' -Wait
        $deadline = (Get-Date).AddSeconds(120)
        while ((Test-Path (Join-Path $app 'theuth-desktop.exe')) -and (Get-Date) -lt $deadline) { Start-Sleep 2 }
        Say "uninstall finished after $([int]((Get-Date) - $t0).TotalSeconds) s; theuth-desktop.exe left=$(Test-Path (Join-Path $app 'theuth-desktop.exe')) t.exe left=$(Test-Path (Join-Path $app 't.exe'))"
        Reg
        $cd = Join-Path $app 'companion'
        if (Test-Path $cd) {
            Get-ChildItem $cd -Recurse -File | Select-Object -First 15 | ForEach-Object { Say "left in companion\: $($_.FullName.Substring($cd.Length + 1))" }
            Say "companion\ files left: $(@(Get-ChildItem $cd -Recurse -File).Count)"
        } else { Say 'companion\ removed' }
        Procs
    }
    'engine-start' { Start-ScheduledTask -TaskName 'theuth-test-engine'; Start-Sleep 10; Procs }
    'pkg-start' {
        $sha = if ($Rest.Count -gt 0) { $Rest[0] } else { '' }
        if ($sha -notmatch '^[0-9a-f]{7,40}$') { Say 'usage: pkg-start <sha>'; break }
        if ((Get-ScheduledTask -TaskName 'theuth-pkg').State -eq 'Running') { Say 'theuth-pkg is running; refusing'; break }
        $dirty = @(git -C $clone status --porcelain --untracked-files=no)
        if ($dirty.Count -gt 0) {
            if ((git -C $clone branch --show-current) -eq 'main') {
                git -C $clone stash push -q -m "pkg-gate-134-w6: regenerated files of $(git -C $clone rev-parse --short HEAD)"
                Say "stashed $($dirty.Count) regenerated files of main (restore-clone pops it)"
            } else {
                git -C $clone checkout -q -- .
                Say "discarded $($dirty.Count) regenerated files of the detached build"
            }
        }
        git -C $clone fetch -q origin 2>&1 | ForEach-Object { Say "fetch: $_" }
        git -C $clone checkout -q --detach $sha 2>&1 | ForEach-Object { Say "checkout: $_" }
        $head = git -C $clone rev-parse --short HEAD
        Say "release clone at $head"
        if (-not $sha.StartsWith($head) -and -not $head.StartsWith($sha)) { Say "head $head is not $sha; not starting"; break }
        Remove-Item (Join-Path $pkg 'last-exit.txt') -ErrorAction SilentlyContinue
        Start-ScheduledTask -TaskName 'theuth-pkg'
        Start-Sleep 6
        Say "theuth-pkg $((Get-ScheduledTask -TaskName 'theuth-pkg').State) log=$(Get-Content (Join-Path $pkg 'current-log.txt') -ErrorAction SilentlyContinue)"
    }
    'launch-task' {
        $exe = Join-Path $app 'theuth-desktop.exe'
        if (-not (Test-Path $exe)) { Say "no $exe"; break }
        schtasks /create /tn theuth-app-once /tr $exe /sc once /st 23:59 /it /f 2>&1 | ForEach-Object { Say "schtasks: $_" }
        schtasks /run /tn theuth-app-once 2>&1 | ForEach-Object { Say "schtasks: $_" }
        Start-Sleep 20
        schtasks /delete /tn theuth-app-once /f 2>&1 | Out-Null
        Procs
    }
    'restore-clone' {
        if ((Get-ScheduledTask -TaskName 'theuth-pkg' -ErrorAction SilentlyContinue).State -eq 'Running') { Say 'restore-clone: theuth-pkg is still running; refusing'; exit 1 }
        Push-Location $clone
        git checkout -q -- . 2>&1 | Select-Object -Last 2 | ForEach-Object { Say "checkout --: $_" }
        git checkout -q main 2>&1 | Select-Object -Last 2 | ForEach-Object { Say "checkout main: $_" }
        $st = git stash list | Select-Object -First 1
        if ($st -match 'pkg-gate-134-w5') { git stash pop -q 2>&1 | Select-Object -Last 2 | ForEach-Object { Say "stash pop: $_" } } else { Say "no pkg-gate stash on top: $st" }
        Say "clone head $(git rev-parse --short HEAD) branch=$(git branch --show-current) dirty=$((git status --porcelain --untracked-files=no | Measure-Object).Count) stashes=$(@(git stash list).Count)"
        Pop-Location
    }
    default { Get-Content $PSCommandPath | Select-Object -Skip 1 -First 18 | ForEach-Object { Say $_ } }
}
