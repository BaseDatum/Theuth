#requires -Version 7.0
<#
.SYNOPSIS
  Design 134 W3 gate helper (notifications · breakpoints · Settings · Run key), run through the build service's `script`
  action inside the interactive desktop session (a WinUI window and toasts need one). One verb per run:

    probe   -Seconds N [-Stay] [-From publish|debug]   Theuth.Companion.exe --probe-notify N [--stay]; prints probe-notify.log
    launch  [-Args '--settings notifications']         start the exe detached (returns at once), prints the pid
    stop                                                kill Theuth.Companion, print the companion log tail
    log     [-Tail 80] [-Match 'notifications|login']   print today's companion log tail (filtered)
    reg                                                 reg query HKCU\...\Run /v TheuthCompanion + <data_dir>\companion.json
    settings-get                                        print %LOCALAPPDATA%\theuth\companion\settings.json
    settings-set -Key notifyUrgent -Value true          set one key (true/false/number/string) — companion must not be running
    engine-stop                                         Stop-Process t (the scheduled task's loop restarts it in ~5 s)
    processes                                           Theuth.Companion / t processes

.EXAMPLE
  pwsh -File crates\desktop\companion-windows\w3-gate.ps1 probe -Seconds 12
  pwsh -File crates\desktop\companion-windows\w3-gate.ps1 launch -Args '--settings keys'
  pwsh -File crates\desktop\companion-windows\w3-gate.ps1 reg
#>
param(
    [Parameter(Position = 0)] [ValidateSet('probe', 'launch', 'stop', 'log', 'reg', 'settings-get', 'settings-set', 'engine-stop', 'processes', 'crash')]
    [string] $Verb = 'log',
    [double] $Seconds = 12,
    [switch] $Stay,
    [ValidateSet('publish', 'debug', 'release')] [string] $From = 'publish',
    [string] $Args = '',
    [int] $Tail = 80,
    [string] $Match = '',
    [string] $Key = '',
    [string] $Value = ''
)
[Console]::OutputEncoding = [Text.Encoding]::UTF8
$ErrorActionPreference = 'Stop'
$here = $PSScriptRoot
$repo = (Resolve-Path (Join-Path $here '..\..\..')).Path
$exe = switch ($From) {
    'publish' { Join-Path $repo 'target\release\companion-windows\Theuth.Companion.exe' }
    'debug'   { Join-Path $here 'src\Theuth.Companion\bin\x64\Debug\net10.0-windows10.0.22621.0\win-x64\Theuth.Companion.exe' }
    'release' { Join-Path $here 'src\Theuth.Companion\bin\x64\Release\net10.0-windows10.0.22621.0\win-x64\Theuth.Companion.exe' }
}
$logDir = Join-Path $env:LOCALAPPDATA 'theuth\companion\logs'
$daily = Join-Path $logDir ("companion-{0:yyyy-MM-dd}.log" -f (Get-Date))
$probeLog = Join-Path $logDir 'probe-notify.log'
$settingsFile = Join-Path $env:LOCALAPPDATA 'theuth\companion\settings.json'
$dataDir = if ($env:THEUTH_DATA_DIR) { $env:THEUTH_DATA_DIR } else { Join-Path $env:USERPROFILE '.theuth' }

function Show-Log([int] $n, [string] $pattern) {
    if (-not (Test-Path $daily)) { Write-Host "no log at $daily"; return }
    $lines = Get-Content $daily -Tail $n
    if ($pattern) { $lines = $lines | Where-Object { $_ -match $pattern } }
    Write-Host "---- $daily (tail $n$(if ($pattern) { ", match '$pattern'" }))"
    $lines | ForEach-Object { Write-Host $_ }
    Write-Host '----'
}

function Split-Args([string] $s) {
    if (-not $s) { return @() }
    return @($s -split '\s+' | Where-Object { $_ })
}

switch ($Verb) {
    'processes' {
        Get-Process -Name 'Theuth.Companion', 't' -ErrorAction SilentlyContinue |
            ForEach-Object { Write-Host ("{0,-20} pid {1,-7} started {2:HH:mm:ss}" -f $_.Name, $_.Id, $_.StartTime) }
    }
    'probe' {
        if (-not (Test-Path $exe)) { throw "no exe at $exe" }
        Write-Host ("exe: {0} ({1:yyyy-MM-dd HH:mm:ss})" -f $exe, (Get-Item $exe).LastWriteTime)
        Get-Process -Name 'Theuth.Companion' -ErrorAction SilentlyContinue | Stop-Process -Force
        Start-Sleep -Milliseconds 500
        $a = @('--probe-notify', $Seconds.ToString([Globalization.CultureInfo]::InvariantCulture))
        if ($Stay) { $a += '--stay' }
        $a += Split-Args $Args
        Write-Host "run: Theuth.Companion.exe $($a -join ' ')"
        $p = Start-Process -FilePath $exe -ArgumentList $a -PassThru
        $null = $p.Handle
        if ($Stay) {
            Start-Sleep -Seconds ([int]$Seconds + 6)
            Write-Host "left running: pid $($p.Id) exited=$($p.HasExited)"
        } else {
            if (-not $p.WaitForExit([int](($Seconds + 40) * 1000))) { $p.Kill(); Write-Host "PROBE FAILED: no exit within $($Seconds + 40) s" }
            else { Write-Host "exit code: $($p.ExitCode)" }
        }
        if (Test-Path $probeLog) {
            Write-Host "---- $probeLog"
            Get-Content $probeLog | ForEach-Object { Write-Host $_ }
            Write-Host '----'
        } else { Write-Host "no probe log at $probeLog" }
        Show-Log 60 'notifications|breakpoint|login item|settings|Register|terminate|crash|probe-notify start'
    }
    'launch' {
        if (-not (Test-Path $exe)) { throw "no exe at $exe" }
        Get-Process -Name 'Theuth.Companion' -ErrorAction SilentlyContinue | Stop-Process -Force
        Start-Sleep -Milliseconds 500
        $a = Split-Args $Args
        Write-Host "launch: Theuth.Companion.exe $($a -join ' ')"
        $p = if ($a.Count -gt 0) { Start-Process -FilePath $exe -ArgumentList $a -PassThru } else { Start-Process -FilePath $exe -PassThru }
        Start-Sleep -Seconds 4
        Write-Host "pid $($p.Id) exited=$($p.HasExited)"
        Show-Log 40 'launch|notifications|settings|login item|breakpoints|crash'
    }
    'stop' {
        $procs = @(Get-Process -Name 'Theuth.Companion' -ErrorAction SilentlyContinue)
        Write-Host "stopping $($procs.Count) Theuth.Companion process(es)"
        $procs | Stop-Process -Force
        Start-Sleep -Milliseconds 500
        Show-Log $Tail $Match
    }
    'log' { Show-Log $Tail $Match }
    'reg' {
        Write-Host 'reg query HKCU\Software\Microsoft\Windows\CurrentVersion\Run /v TheuthCompanion'
        & reg query 'HKCU\Software\Microsoft\Windows\CurrentVersion\Run' /v TheuthCompanion 2>&1 | ForEach-Object { Write-Host "  $_" }
        Write-Host "reg exit: $LASTEXITCODE (0 = present, 1 = absent)"
        $f = Join-Path $dataDir 'companion.json'
        if (Test-Path $f) { Write-Host "$f = $((Get-Content $f -Raw).Trim())" } else { Write-Host "$f missing" }
    }
    'settings-get' {
        if (Test-Path $settingsFile) { Write-Host "---- $settingsFile"; Get-Content $settingsFile | ForEach-Object { Write-Host $_ } } else { Write-Host "no settings at $settingsFile" }
    }
    'settings-set' {
        if (Get-Process -Name 'Theuth.Companion' -ErrorAction SilentlyContinue) { throw 'stop the companion first (it writes settings.json itself)' }
        $obj = if (Test-Path $settingsFile) { Get-Content $settingsFile -Raw | ConvertFrom-Json -AsHashtable } else { @{} }
        $v = switch -Regex ($Value) {
            '^(true|false)$' { [bool]::Parse($Value) }
            '^-?\d+(\.\d+)?$' { [double]$Value }
            default { $Value }
        }
        $obj[$Key] = $v
        New-Item -ItemType Directory -Force -Path (Split-Path $settingsFile) | Out-Null
        $obj | ConvertTo-Json -Depth 5 | Set-Content -Path $settingsFile -Encoding UTF8
        Write-Host "set $Key = $Value in $settingsFile"
        Get-Content $settingsFile | ForEach-Object { Write-Host $_ }
    }
    'crash' {
        # The last crash records for Theuth.Companion.exe: .NET Runtime (1026: exception + stack), Application Error (1000), WER (1001).
        $events = Get-WinEvent -FilterHashtable @{ LogName = 'Application'; Id = @(1000, 1001, 1026) } -MaxEvents 40 -ErrorAction SilentlyContinue |
            Where-Object { $_.Message -match 'Theuth\.Companion' } | Select-Object -First 4
        if (-not $events) { Write-Host 'no crash records for Theuth.Companion.exe in the Application log' }
        foreach ($e in $events) {
            Write-Host ("==== {0:HH:mm:ss} {1} id={2}" -f $e.TimeCreated, $e.ProviderName, $e.Id)
            ($e.Message -split "`n" | Select-Object -First 40) | ForEach-Object { Write-Host $_ }
        }
        $crashes = Get-ChildItem (Join-Path $env:LOCALAPPDATA 'theuth\companion\logs') -Filter '*.crash.json' -ErrorAction SilentlyContinue | Sort-Object LastWriteTime -Descending | Select-Object -First 2
        foreach ($c in $crashes) { Write-Host "---- $($c.FullName)"; Get-Content $c.FullName -TotalCount 60 | ForEach-Object { Write-Host $_ } }
    }
    'engine-stop' {
        $t = @(Get-Process -Name 't' -ErrorAction SilentlyContinue)
        Write-Host "stopping $($t.Count) t.exe (the theuth-test-engine loop restarts it in ~5 s)"
        $t | Stop-Process -Force
    }
}
exit 0