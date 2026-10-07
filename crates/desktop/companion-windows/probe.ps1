#requires -Version 7.0
<#
.SYNOPSIS
  Run the companion's W1 probe on this machine (design 134 §7 W1): Theuth.Companion.exe --probe <Seconds>, wait for it,
  print the engine record (never the token), the probe log and the exit code, and check the Swift W1 probe shape.

.DESCRIPTION
  For the build service's `script` action, which runs in the interactive desktop session (a WinUI window needs one).
  The exe is the published one (build.ps1 -> target\release\companion-windows\) unless -From debug|release picks a
  `dotnet build` output. Exit code 0 when the probe connected, logged the counts and exited 0.

.EXAMPLE
  pwsh -File crates\desktop\companion-windows\probe.ps1 -Seconds 15
#>
param(
    [double] $Seconds = 15,
    [ValidateSet('publish', 'debug', 'release')] [string] $From = 'publish'
)
$ErrorActionPreference = 'Stop'
$here = $PSScriptRoot
$repo = (Resolve-Path (Join-Path $here '..\..\..')).Path
$exe = switch ($From) {
    'publish' { Join-Path $repo 'target\release\companion-windows\Theuth.Companion.exe' }
    'debug'   { Join-Path $here 'src\Theuth.Companion\bin\x64\Debug\net10.0-windows10.0.22621.0\win-x64\Theuth.Companion.exe' }
    'release' { Join-Path $here 'src\Theuth.Companion\bin\x64\Release\net10.0-windows10.0.22621.0\win-x64\Theuth.Companion.exe' }
}
if (-not (Test-Path $exe)) { throw "no exe at $exe (run build.ps1, or dotnet build -p:Platform=x64)" }
$item = Get-Item $exe
Write-Host ("exe: {0} ({1:yyyy-MM-dd HH:mm:ss})" -f $item.FullName, $item.LastWriteTime)

$dataDir = if ($env:THEUTH_DATA_DIR) { $env:THEUTH_DATA_DIR } else { Join-Path $env:USERPROFILE '.theuth' }
$engineJson = Join-Path $dataDir 'engine.json'
if (Test-Path $engineJson) {
    $e = Get-Content $engineJson -Raw | ConvertFrom-Json
    $alive = if ($e.pid -and (Get-Process -Id $e.pid -ErrorAction SilentlyContinue)) { 'alive' } else { 'not running' }
    Write-Host "engine.json: listen=$($e.listen) pid=$($e.pid) ($alive) version=$($e.version) started=$($e.started)"
} else {
    Write-Host "engine.json: missing at $engineJson"
}
$token = Join-Path $dataDir 'token'
Write-Host ("token: " + $(if (Test-Path $token) { "{0} chars" -f (Get-Content $token -Raw).Trim().Length } else { 'missing' }))

Get-Process -Name 'Theuth.Companion' -ErrorAction SilentlyContinue | Stop-Process -Force
$err = New-TemporaryFile
$out = New-TemporaryFile
$probeArgs = @('--probe', $Seconds.ToString([System.Globalization.CultureInfo]::InvariantCulture))
Write-Host "run: Theuth.Companion.exe $($probeArgs -join ' ')"
$p = Start-Process -FilePath $exe -ArgumentList $probeArgs -PassThru -RedirectStandardError $err -RedirectStandardOutput $out
$null = $p.Handle
if (-not $p.WaitForExit([int](($Seconds + 30) * 1000))) {
    $p.Kill()
    throw "the probe did not exit within $($Seconds + 30) s"
}
$code = $p.ExitCode
$log = Join-Path $env:LOCALAPPDATA 'theuth\companion\logs\probe.log'
Write-Host "---- $log"
$lines = if (Test-Path $log) { @(Get-Content $log) } else { @(Get-Content $err) }
$lines | ForEach-Object { Write-Host $_ }
Write-Host '----'
Write-Host "exit code: $code"
Remove-Item $err, $out -ErrorAction SilentlyContinue

$want = [ordered]@{
    'engine URL'          = 'engine ws://'
    'connection open'     = 'connection: open'
    'PendingPrompts'      = 'probe PendingPrompts'
    'counts'              = 'probe attention_list='
    'done, exit 0'        = 'exit 0'
}
$failed = @()
foreach ($k in $want.Keys) {
    if (-not ($lines | Where-Object { $_.Contains($want[$k]) })) { $failed += $k }
}
if ($code -ne 0) { $failed += "exit code $code" }
if ($failed.Count -gt 0) {
    Write-Host "PROBE FAILED: $($failed -join ', ')"
    exit 1
}
Write-Host 'PROBE OK'
exit 0
