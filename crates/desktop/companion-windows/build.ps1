#requires -Version 7.0
<#
.SYNOPSIS
  Publish the Windows companion (design 134 §5.2, §5.7): unpackaged, self-contained WinUI 3 on the trimmed Windows App SDK
  2.5.1 component set, into <repo>\target\release\companion-windows\, then check it and smoke AppNotificationManager.Register().

.DESCRIPTION
  1. Stops a running Theuth.Companion (a running exe locks the output: MSB3021).
  2. dotnet publish src\Theuth.Companion -c <Configuration> -p:Platform=<Platform> -p:RuntimeIdentifier=<rid>
     -p:TheuthMsixArch=<msixArch> -o <Output>  (SelfContained comes from the csproj; ExtractRuntimeMsixPayload copies
     the five runtime-MSIX files for the target architecture into the output).
  3. Checks: the five files present, no ML component (onnxruntime.dll / DirectML.dll), the resolved WASDK packages;
     prints the file count and size (~170 MB expected, A2).
  4. Runs the published exe with --register-smoke: Register() must succeed (it throws 0x8007007E without the five files).
  Exit code 0 when everything passed.

.EXAMPLE
  pwsh -File crates\desktop\companion-windows\build.ps1
  pwsh -File crates\desktop\companion-windows\build.ps1 -Configuration Debug -NoSmoke
#>
param(
    [ValidateSet('Release', 'Debug')] [string] $Configuration = 'Release',
    [ValidateSet('x64', 'ARM64')] [string] $Platform = 'x64',
    [string] $Output = '',
    [switch] $Clean,
    [switch] $NoSmoke
)
$ErrorActionPreference = 'Stop'
$here = $PSScriptRoot
$repo = (Resolve-Path (Join-Path $here '..\..\..')).Path
if (-not $Output) { $Output = Join-Path $repo 'target\release\companion-windows' }
$proj = Join-Path $here 'src\Theuth.Companion\Theuth.Companion.csproj'
$five = @(
    'Microsoft.WindowsAppRuntime.Insights.Resource.dll',
    'WindowsAppRuntime.DeploymentExtensions.OneCore.dll',
    'WindowsAppSdk.AppxDeploymentExtensions.Desktop.dll',
    'WindowsAppSdk.AppxDeploymentExtensions.Desktop-EventLog-Instrumentation.dll',
    'WindowsAppRuntime.png'
)

# Stop only companions started from this worktree or from the output dir (they lock files we are about to replace);
# other worktrees' test instances (parallel waves, build-service slots) must survive this build.
$mine = @(($repo.TrimEnd('\') + '\'), ([IO.Path]::GetFullPath($Output).TrimEnd('\') + '\'))
Get-Process -Name 'Theuth.Companion' -ErrorAction SilentlyContinue |
    Where-Object { $p = $_.Path; $p -and ($mine | Where-Object { $p.StartsWith($_, [StringComparison]::OrdinalIgnoreCase) }) } |
    Stop-Process -Force
if ($Clean -and (Test-Path $Output)) { Remove-Item -Recurse -Force $Output }

$rid = if ($Platform -eq 'ARM64') { 'win-arm64' } else { 'win-x64' }
$msixArch = if ($Platform -eq 'ARM64') { 'arm64' } else { 'x64' }

Write-Host "[1/4] dotnet publish $Configuration ($Platform / $rid) -> $Output"
$sw = [System.Diagnostics.Stopwatch]::StartNew()
& dotnet publish $proj -c $Configuration -p:Platform=$Platform -p:RuntimeIdentifier=$rid -p:TheuthMsixArch=$msixArch -o $Output --nologo
if ($LASTEXITCODE -ne 0) { throw "dotnet publish failed (exit $LASTEXITCODE)" }
$secs = $sw.Elapsed.TotalSeconds

Write-Host "[2/4] checks"
$missing = @($five | Where-Object { -not (Test-Path (Join-Path $Output $_)) })
if ($missing.Count -gt 0) { throw "runtime-MSIX files missing from ${Output}: $($missing -join ', ')" }
foreach ($f in $five) {
    $item = Get-Item (Join-Path $Output $f)
    Write-Host ("  ok  {0}  {1:N0} bytes" -f $item.Name, $item.Length)
}
$ml = @('onnxruntime.dll', 'DirectML.dll') | Where-Object { Test-Path (Join-Path $Output $_) }
if ($ml) { throw "the ML component leaked into the output: $($ml -join ', ')" }
Write-Host '  ok  no onnxruntime.dll / DirectML.dll'
$assets = Join-Path $here 'src\Theuth.Companion\obj\project.assets.json'
if (Test-Path $assets) {
    $libs = (Get-Content $assets -Raw | ConvertFrom-Json).libraries.PSObject.Properties.Name |
        Where-Object { $_ -like 'Microsoft.WindowsAppSDK*' -or $_ -like 'Microsoft.Web.WebView2*' } | Sort-Object
    Write-Host "  packages: $($libs -join ', ')"
    if ($libs | Where-Object { $_ -like 'Microsoft.WindowsAppSDK/*' -or $_ -like 'Microsoft.WindowsAppSDK.ML/*' -or $_ -like 'Microsoft.WindowsAppSDK.AI/*' }) {
        throw 'the Windows App SDK meta package (or its ML/AI components) is in the graph'
    }
}

Write-Host "[3/4] size"
$files = @(Get-ChildItem -Recurse -File $Output)
$mb = ($files | Measure-Object -Property Length -Sum).Sum / 1MB
$exe = Join-Path $Output 'Theuth.Companion.exe'
if (-not (Test-Path $exe)) { throw "no Theuth.Companion.exe in $Output" }
Write-Host ("  publish: {0} files, {1:N1} MB, {2:N1} s -> {3}" -f $files.Count, $mb, $secs, $Output)
Write-Host ("  largest: " + (($files | Sort-Object Length -Descending | Select-Object -First 5 | ForEach-Object { '{0} {1:N1} MB' -f $_.Name, ($_.Length / 1MB) }) -join ', '))

if ($NoSmoke) {
    Write-Host '[4/4] Register() smoke skipped (-NoSmoke)'
    exit 0
}
Write-Host '[4/4] Register() smoke: Theuth.Companion.exe --register-smoke'
$err = New-TemporaryFile
$out = New-TemporaryFile
$p = Start-Process -FilePath $exe -ArgumentList '--register-smoke' -PassThru -RedirectStandardError $err -RedirectStandardOutput $out
$null = $p.Handle # keep the handle so ExitCode is readable after exit
if (-not $p.WaitForExit(60000)) {
    $p.Kill()
    throw 'Register() smoke: no exit within 60 s'
}
Get-Content $err | Where-Object { $_ -match 'register-smoke|launch|runtime|terminate|crash' } | ForEach-Object { Write-Host "  $_" }
Remove-Item $err, $out -ErrorAction SilentlyContinue
if ($p.ExitCode -ne 0) { throw "Register() smoke failed (exit $($p.ExitCode))" }
Write-Host '  ok  Register() succeeded on the published layout'
exit 0
