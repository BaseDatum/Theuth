#requires -Version 7.0
<#
.SYNOPSIS
  The W4 gate helper (design 134 §7 W4 — Ask bar, live dictation, receipts) for the build service's `script` action, which
  runs in the interactive desktop session on build-B (WASAPI capture and WinUI windows need it; over ssh WASAPI is silent).

.DESCRIPTION
  env         what the Ask bar depends on: the scratch engine (engine.json, version, mesh/voice config), the dictation model
              and ONNX Runtime the engine loads (<data_dir>\models\nemotron-streaming-en, models\onnxruntime), capture
              endpoints and the microphone privacy switches.
  run         start Theuth.Companion.exe with the W4 flags below, wait for it to exit, print its ask-probe log and exit code.
  log         the tail of today's companion log and of logs\ask-probe.log.
  engine-log  the tail of the engine's newest log, filtered by -Pattern.
  kill        stop Theuth.Companion.
  model-link  give the scratch engine the dictation model: junctions <data_dir>\models\{nemotron-streaming-en,onnxruntime}
              to the September data dir's copies (%USERPROFILE%\.theuth-old-2026-09\models) when they are missing;
              model-unlink removes exactly those junctions again. engine-restart stops t.exe (its loop restarts it in ~5 s)
              so a model that was reported missing is looked for again.

  Flags (run): -AskProbe N → --ask-probe N · -Synthetic → --synthetic-dictation · -Speech "text" → --synthetic-speech
  "text" (underscores become spaces: the build service refuses spaces in args) · -MicProbe N → --mic-probe N · -MixFormat → --mic-mix-format · -Replay → --ask-replay · -Shareable → --shareable ·
  -Ask → --ask (open the bar at launch).
  The exe is the Debug build (`dotnet build -p:Platform=x64`) unless -From publish|release.

.EXAMPLE
  pwsh -File crates\desktop\companion-windows\ask.ps1 env
  pwsh -File crates\desktop\companion-windows\ask.ps1 run -AskProbe 9 -Synthetic
  pwsh -File crates\desktop\companion-windows\ask.ps1 run -MicProbe 6
#>
param(
    [Parameter(Position = 0)] [ValidateSet('env', 'run', 'log', 'engine-log', 'kill', 'model-link', 'model-unlink', 'engine-restart')] [string] $Command = 'env',
    [ValidateSet('publish', 'debug', 'release')] [string] $From = 'debug',
    [double] $AskProbe = -1,
    [switch] $Synthetic,
    [string] $Speech = '',
    [double] $MicProbe = -1,
    [switch] $MixFormat,
    [switch] $Replay,
    [switch] $Shareable,
    [switch] $Ask,
    [string] $Pattern = 'dictation|voice|desk|send_input|turn|provider|model|error|warn',
    [int] $Tail = 60
)
[Console]::OutputEncoding = [Text.Encoding]::UTF8
$ErrorActionPreference = 'Stop'
$here = $PSScriptRoot
$repo = (Resolve-Path (Join-Path $here '..\..\..')).Path
$dataDir = if ($env:THEUTH_DATA_DIR) { $env:THEUTH_DATA_DIR } else { Join-Path $env:USERPROFILE '.theuth' }
$logs = Join-Path $env:LOCALAPPDATA 'theuth\companion\logs'
$inv = [System.Globalization.CultureInfo]::InvariantCulture

function Exe {
    switch ($From) {
        'publish' { Join-Path $repo 'target\release\companion-windows\Theuth.Companion.exe' }
        'debug'   { Join-Path $here 'src\Theuth.Companion\bin\x64\Debug\net10.0-windows10.0.22621.0\win-x64\Theuth.Companion.exe' }
        'release' { Join-Path $here 'src\Theuth.Companion\bin\x64\Release\net10.0-windows10.0.22621.0\win-x64\Theuth.Companion.exe' }
    }
}

function Show-Dir([string] $path) {
    if (-not (Test-Path $path)) { Write-Host "  (missing) $path"; return }
    Write-Host "  $path"
    Get-ChildItem -Force $path | ForEach-Object {
        if ($_.PSIsContainer) {
            $files = @(Get-ChildItem -Recurse -File -Force $_.FullName -ErrorAction SilentlyContinue)
            $mb = ($files | Measure-Object -Property Length -Sum).Sum / 1MB
            $link = if ($_.LinkType) { " -> $($_.Target)" } else { '' }
            Write-Host ("    {0}\  {1} files, {2:N1} MB{3}" -f $_.Name, $files.Count, $mb, $link)
        } else {
            Write-Host ("    {0}  {1:N0} bytes" -f $_.Name, $_.Length)
        }
    }
}

switch ($Command) {
    'env' {
        Write-Host "== engine ($dataDir)"
        $engineJson = Join-Path $dataDir 'engine.json'
        if (Test-Path $engineJson) {
            $e = Get-Content $engineJson -Raw | ConvertFrom-Json
            $alive = if ($e.pid -and (Get-Process -Id $e.pid -ErrorAction SilentlyContinue)) { 'alive' } else { 'not running' }
            Write-Host "  engine.json: listen=$($e.listen) pid=$($e.pid) ($alive) version=$($e.version)"
        } else { Write-Host "  engine.json missing" }
        $cfg = Join-Path $dataDir 'config.yaml'
        if (Test-Path $cfg) {
            $lines = Get-Content $cfg
            $keep = $false
            Write-Host "  config.yaml (uncommented voice:/mesh:/models: blocks):"
            foreach ($l in $lines) {
                if ($l -match '^(voice|mesh|models|signal):') { $keep = $true; Write-Host "    $l"; continue }
                if ($keep -and $l -match '^\s+\S' -and $l -notmatch '^\s*#') { Write-Host "    $l"; continue }
                if ($l -match '^\S') { $keep = $false }
            }
        }
        Write-Host "  ORT_DYLIB_PATH=$($env:ORT_DYLIB_PATH)"
        Write-Host '== dictation model and ONNX Runtime'
        Show-Dir (Join-Path $dataDir 'models')
        foreach ($d in @("$env:USERPROFILE\.theuth-old-2026-09\models", 'C:\Users\user\tools\theuth-test\2.12.0')) { Show-Dir $d }
        Write-Host '== capture endpoints (PnP AudioEndpoint)'
        try {
            Get-PnpDevice -Class AudioEndpoint -ErrorAction Stop | Sort-Object FriendlyName |
                ForEach-Object { Write-Host ("  {0,-8} {1}" -f $_.Status, $_.FriendlyName) }
        } catch { Write-Host "  Get-PnpDevice failed: $($_.Exception.Message)" }
        Write-Host '== microphone privacy (CapabilityAccessManager ConsentStore)'
        foreach ($k in @('HKCU:\Software\Microsoft\Windows\CurrentVersion\CapabilityAccessManager\ConsentStore\microphone',
                         'HKCU:\Software\Microsoft\Windows\CurrentVersion\CapabilityAccessManager\ConsentStore\microphone\NonPackaged',
                         'HKLM:\SOFTWARE\Microsoft\Windows\CurrentVersion\CapabilityAccessManager\ConsentStore\microphone')) {
            $v = (Get-ItemProperty -Path $k -Name Value -ErrorAction SilentlyContinue).Value
            Write-Host ("  {0} = {1}" -f $k, $(if ($null -eq $v) { '(unset)' } else { $v }))
        }
        Write-Host "== session: $([Environment]::UserName) session $((Get-Process -Id $PID).SessionId) $(if ($env:SESSIONNAME) { $env:SESSIONNAME } else { '' })"
        exit 0
    }
    'kill' {
        $p = @(Get-Process -Name 'Theuth.Companion' -ErrorAction SilentlyContinue)
        $p | Stop-Process -Force
        Write-Host "killed $($p.Count) Theuth.Companion process(es)"
        exit 0
    }
    'model-link' {
        $models = Join-Path $dataDir 'models'
        $old = Join-Path $env:USERPROFILE '.theuth-old-2026-09\models'
        New-Item -ItemType Directory -Force -Path $models | Out-Null
        foreach ($name in @('nemotron-streaming-en', 'onnxruntime')) {
            $target = Join-Path $old $name
            $link = Join-Path $models $name
            if (Test-Path $link) { Write-Host "  exists  $link"; continue }
            if (-not (Test-Path $target)) { Write-Host "  no source $target"; continue }
            New-Item -ItemType Junction -Path $link -Target $target | Out-Null
            Write-Host "  linked  $link -> $target"
        }
        Show-Dir $models
        exit 0
    }
    'model-unlink' {
        $models = Join-Path $dataDir 'models'
        foreach ($name in @('nemotron-streaming-en', 'onnxruntime')) {
            $link = Join-Path $models $name
            $item = Get-Item $link -ErrorAction SilentlyContinue
            if ($item -and $item.LinkType -eq 'Junction') { $item.Delete(); Write-Host "  removed junction $link" }
            elseif ($item) { Write-Host "  kept (not a junction) $link" }
            else { Write-Host "  absent $link" }
        }
        exit 0
    }
    'engine-restart' {
        $p = @(Get-Process -Name 't' -ErrorAction SilentlyContinue)
        $p | Stop-Process -Force
        Write-Host "stopped $($p.Count) t.exe process(es); the engine-run loop restarts it in ~5 s"
        Start-Sleep -Seconds 8
        $engineJson = Join-Path $dataDir 'engine.json'
        if (Test-Path $engineJson) {
            $e = Get-Content $engineJson -Raw | ConvertFrom-Json
            $alive = if ($e.pid -and (Get-Process -Id $e.pid -ErrorAction SilentlyContinue)) { 'alive' } else { 'not running' }
            Write-Host "  engine.json: listen=$($e.listen) pid=$($e.pid) ($alive) version=$($e.version)"
        }
        exit 0
    }
    'log' {
        $daily = Join-Path $logs ("companion-{0:yyyy-MM-dd}.log" -f (Get-Date))
        foreach ($f in @($daily, (Join-Path $logs 'ask-probe.log'))) {
            Write-Host "---- $f"
            if (Test-Path $f) { Get-Content $f -Tail $Tail | ForEach-Object { Write-Host $_ } } else { Write-Host '(missing)' }
        }
        exit 0
    }
    'engine-log' {
        $f = Get-ChildItem (Join-Path $dataDir 'logs') -Filter 'theuth.log*' -ErrorAction SilentlyContinue | Sort-Object LastWriteTime -Descending | Select-Object -First 1
        if (-not $f) { Write-Host "no engine log under $dataDir\logs"; exit 0 }
        Write-Host "---- $($f.FullName) (pattern: $Pattern)"
        Get-Content $f.FullName | Select-String -Pattern $Pattern | Select-Object -Last $Tail | ForEach-Object { Write-Host $_.Line }
        exit 0
    }
    'run' {
        $exe = Exe
        if (-not (Test-Path $exe)) { throw "no exe at $exe (dotnet build -p:Platform=x64, or build.ps1 for -From publish)" }
        $item = Get-Item $exe
        Write-Host ("exe: {0} ({1:yyyy-MM-dd HH:mm:ss})" -f $item.FullName, $item.LastWriteTime)
        $a = @()
        $secs = 10
        if ($AskProbe -ge 0) { $a += @('--ask-probe', $AskProbe.ToString($inv)); $secs = $AskProbe }
        if ($Synthetic) { $a += '--synthetic-dictation' }
        if ($Speech) { $a += @('--synthetic-speech', ('"' + ($Speech -replace '_', ' ') + '"')) } # the build service refuses spaces in args: write_them_like_this; quoted for Start-Process
        if ($MicProbe -ge 0) { $a += @('--mic-probe', $MicProbe.ToString($inv)); $secs = $MicProbe }
        if ($MixFormat) { $a += '--mic-mix-format' }
        if ($Replay) { $a += '--ask-replay' }
        if ($Shareable) { $a += '--shareable' }
        if ($Ask) { $a += '--ask' }
        if ($a.Count -eq 0) { throw 'run needs -AskProbe N or -MicProbe N' }
        Get-Process -Name 'Theuth.Companion' -ErrorAction SilentlyContinue | Stop-Process -Force
        $err = New-TemporaryFile
        $out = New-TemporaryFile
        Write-Host "run: Theuth.Companion.exe $($a -join ' ')"
        $p = Start-Process -FilePath $exe -ArgumentList $a -PassThru -RedirectStandardError $err -RedirectStandardOutput $out
        $null = $p.Handle
        if (-not $p.WaitForExit([int](($secs + 30) * 1000))) {
            $p.Kill()
            Write-Host "did not exit within $($secs + 30) s — killed"
        }
        $code = $p.ExitCode
        $log = Join-Path $logs 'ask-probe.log'
        Write-Host "---- $log"
        if (Test-Path $log) { Get-Content $log | ForEach-Object { Write-Host $_ } } else { Get-Content $err | ForEach-Object { Write-Host $_ } }
        Write-Host '----'
        Write-Host "exit code: $code"
        Remove-Item $err, $out -ErrorAction SilentlyContinue
        exit $code
    }
}
