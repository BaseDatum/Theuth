#Requires -Version 7
<#
Package theuth for Windows (x86_64): the NSIS desktop installer and the CLI zip, staged for
release-s3.sh. Runs ON the Windows host from the repo root: `just package-windows`, or
`pwsh -NoProfile -File scripts/package-windows.ps1`. Mirrors scripts/package-macos.sh:

  1. cargo build --release -p theuth  (daemon sidecar, x86_64-pc-windows-msvc)
     + crates/app-parser-stub: cargo build -> theuth-app-parser-<hash>.exe compatibility stub (46 S2.1; the
       parser runs in-process, docs/design/apps-inproc-as-built-P1.md; same hash / --version / stdin-EOF checks)
  2. stage both as externalBin: binaries/t-<triple>.exe, binaries/theuth-app-parser-<hash>-<triple>.exe
     (the stub must sit beside `t.exe`, so it rides the same mechanism, as package-linux.sh does;
     the documents helper is compiled into `t`)
  3. onnxruntime.dll (Microsoft's win-x64 CPU build, pinned in scripts/ort-version.env, sha256-checked,
     zip cached in target/) staged as a Tauri resource that lands beside t.exe in the install dir -
     first in `models::onnx::find_ort_lib`'s search order (<exe dir>\onnxruntime.dll before
     ~/.theuth/models/onnxruntime/). The WebGPU plugin DLLs are not bundled: the app downloads them.
  3b. 134 W5: crates/desktop/companion-windows/build.ps1 -Configuration Release (dotnet publish, self-contained
     WinUI 3) -> target/release/companion-windows/Theuth.Companion.exe; the S2 toast smoke
     (`--probe-notify 3`: Register() + one Show, exit 0) runs on that release output; the folder is staged
     as the Tauri resource dir `companion/` so NSIS installs %LOCALAPPDATA%\theuth\companion\ (stable path =
     stable AUMID). Skipped with one line when build.ps1 is absent (the C# app lands from 134 W1).
     crates/desktop/nsis/hooks.nsh (bundle.windows.nsis.installerHooks) kills a running companion before
     install/uninstall and deletes HKCU\...\Run\TheuthCompanion on uninstall.
  4. cargo tauri build --config <rendered json>  (target nsis only; crates/desktop/tauri.windows.conf.json:
     currentUser install, WebView2 downloadBootstrapper)
  5. target/release/dist/theuth_<ver>_windows-x86_64-setup.exe
  6. target/release/dist/theuth_<ver>_windows-x86_64.zip = exactly t.exe + the app-parser stub exe

Unsigned: no Authenticode on the exes or the installer. The updater signature (.sig) is made by
release-s3.sh on the Mac (cargo tauri signer sign), like the Linux tarballs. Never uploads.

Host tools: rustup (rust-toolchain.toml picks the channel; MSVC target), cargo-tauri, bun.
Env:
  THEUTH_SKIP_BUN_INSTALL=1   skip `bun install` (desktop UI)
  THEUTH_SKIP_COMPANION_SMOKE=1  stage the companion without running `--probe-notify` on it
#>
# Progress lines go to the host on purpose.
[Diagnostics.CodeAnalysis.SuppressMessageAttribute('PSAvoidUsingWriteHost', '')]
[CmdletBinding()]
param()

Set-StrictMode -Version Latest
$ErrorActionPreference = 'Stop'
$ProgressPreference = 'SilentlyContinue'

function Fail([string]$Reason) {
    [Console]::Error.WriteLine("error: $Reason")
    exit 1
}

# Run a native command; a non-zero exit is fatal with one line naming the step. Output goes to
# the host, never into a caller's return value.
function Exec([string]$What, [scriptblock]$Command) {
    & $Command | Out-Host
    if ($LASTEXITCODE -ne 0) { Fail "$What failed (exit $LASTEXITCODE)" }
}

function Need([string]$Tool, [string]$Hint) {
    if (-not (Get-Command $Tool -ErrorAction SilentlyContinue)) { Fail "$Tool required ($Hint)" }
}

# The executable exits 0 with nothing on stdin (an idle helper must not crash on EOF).
function Test-StdinEof([string]$Exe) {
    $empty = Join-Path $env:TEMP 'theuth-empty-stdin'
    Set-Content -Path $empty -Value '' -NoNewline
    $p = Start-Process -FilePath $Exe -RedirectStandardInput $empty -RedirectStandardOutput (Join-Path $env:TEMP 'theuth-stdin-eof.out') -NoNewWindow -Wait -PassThru
    if ($p.ExitCode -ne 0) { Fail "$Exe did not exit 0 on stdin EOF (exit $($p.ExitCode))" }
}

function Format-Size([long]$Bytes) { '{0,9:N1} MiB' -f ($Bytes / 1MB) }

if (-not $IsWindows) { Fail "builds Windows bundles on Windows only" }
$Root = (Resolve-Path (Join-Path $PSScriptRoot '..')).Path
Set-Location $Root

Need cargo 'rustup; rust-toolchain.toml picks the channel'
Need bun 'https://bun.sh'
cargo tauri --version *> $null
if ($LASTEXITCODE -ne 0) { Fail 'cargo tauri required (cargo install tauri-cli)' }

$Triple = 'x86_64-pc-windows-msvc'
$HostTriple = (rustc -vV | Select-String '^host: (\S+)').Matches[0].Groups[1].Value
if ($HostTriple -ne $Triple) { Fail "host toolchain is $HostTriple; this script builds $Triple only" }

$Version = ((cargo metadata --no-deps --format-version 1 | ConvertFrom-Json).packages | Where-Object name -eq 'theuth').version
if (-not $Version) { Fail 'cargo metadata: no package named theuth' }
$TauriVersion = (Get-Content crates/desktop/tauri.conf.json -Raw | ConvertFrom-Json).version
if ($TauriVersion -ne $Version) { Fail "version mismatch: crates/theuth/Cargo.toml $Version vs crates/desktop/tauri.conf.json $TauriVersion" }

# scripts/app-parser-env.sh, in PowerShell.
$HelperHash = (Get-Content packages/apps/generated/library-hash.txt -Raw).Trim()
if ($HelperHash -notmatch '^[0-9a-f]{64}$') { Fail "packages/apps/generated/library-hash.txt is not a sha256 hex (got '$HelperHash'; run bun run generate in packages/apps)" }
# Review M1: the committed OpenUI bundle (compiled into t) must embed this library hash. A plain text
# search, not the full input hash of scripts/apps-bundle-hash.sh (TODO: port that to PowerShell; the cargo
# build.rs in crates/app enforces the same text check at build time anyway).
$AppsBundle = Get-Content crates/js-runtime/js/theuth-apps-inproc.js -Raw
if (-not $AppsBundle.Contains($HelperHash)) { Fail "crates/js-runtime/js/theuth-apps-inproc.js does not embed library hash $HelperHash (run just apps-bundle and commit)" }
$Helper = "theuth-app-parser-$HelperHash"

# scripts/ort-version.env: the one ORT pin.
$OrtEnv = Get-Content scripts/ort-version.env -Raw
$OrtVersion = [regex]::Match($OrtEnv, '(?m)^ORT_VERSION="([^"]+)"').Groups[1].Value
$OrtSha = [regex]::Match($OrtEnv, '(?m)^ORT_SHA256_WIN_X64="([0-9a-f]{64})"').Groups[1].Value
if (-not $OrtVersion -or -not $OrtSha) { Fail 'scripts/ort-version.env lacks ORT_VERSION / ORT_SHA256_WIN_X64' }

$Desktop = Join-Path $Root 'crates/desktop'
$Binaries = Join-Path $Desktop 'binaries'
$BundleDir = Join-Path $Root 'target/release/bundle/nsis'
$Dist = Join-Path $Root 'target/release/dist'
$Setup = Join-Path $Dist "theuth_${Version}_windows-x86_64-setup.exe"
$Zip = Join-Path $Dist "theuth_${Version}_windows-x86_64.zip"

Write-Host "==> [1/6] TypeScript bindings"
Exec 'cargo test -p theuth export_bindings' { cargo test -p theuth --bin t --quiet wire_contract_tests::export_bindings -- --exact }
Push-Location (Join-Path $Desktop 'ui')
try { Exec 'bun install' { bun install }; Exec 'bun run wire:generate' { bun run wire:generate } } finally { Pop-Location }
$uiBindings = Join-Path $Desktop 'ui/src/rpc/bindings'
if (Test-Path $uiBindings) { Remove-Item $uiBindings -Recurse -Force }
New-Item -ItemType Directory -Path $uiBindings | Out-Null
Copy-Item (Join-Path $Root 'bindings/typescript/*') $uiBindings -Recurse
Copy-Item (Join-Path $Root 'bindings/surface-catalog*') $uiBindings
Copy-Item (Join-Path $Root 'bindings/methods.json') $uiBindings

Write-Host "==> [1/6] building theuth (release, $Triple)"
Exec 'cargo build --release -p theuth' { cargo build --release -p theuth }
$T = Join-Path $Root 'target/release/t.exe'
if (-not (Test-Path $T)) { Fail "$T not produced" }

Write-Host "==> [1/6] building $Helper.exe (compatibility stub, cargo)"
Exec 'cargo build -p app-parser-stub' { cargo build --release -p app-parser-stub }
$HelperSrc = Join-Path $Root 'target/release/theuth-app-parser-stub.exe'
if (-not (Test-Path $HelperSrc)) { Fail "$HelperSrc not produced" }
$said = (& $HelperSrc --version 2>&1 | Out-String).Trim()
if ($LASTEXITCODE -ne 0) { Fail "$HelperSrc --version failed: $said" }
if ($said -notlike "*$HelperHash*") { Fail "$HelperSrc --version printed '$said', expected library hash $HelperHash (stale build? rerun cargo build --release -p app-parser-stub)" }
Test-StdinEof $HelperSrc

Write-Host "==> [2/6] staging sidecars (t, $Helper)"
New-Item -ItemType Directory -Path $Binaries -Force | Out-Null
Get-ChildItem $Binaries -File | Remove-Item -Force
Copy-Item $T (Join-Path $Binaries "t-$Triple.exe")
Copy-Item $HelperSrc (Join-Path $Binaries "$Helper-$Triple.exe")

Write-Host "==> [3/6] onnxruntime.dll $OrtVersion -> binaries/ (bundled beside t.exe)"
$OrtZip = Join-Path $Root "target/onnxruntime-win-x64-$OrtVersion.zip"
$OrtMember = "onnxruntime-win-x64-$OrtVersion/lib/onnxruntime.dll"
if (-not (Test-Path $OrtZip) -or (Get-FileHash $OrtZip -Algorithm SHA256).Hash.ToLower() -ne $OrtSha) {
    Invoke-WebRequest -Uri "https://github.com/microsoft/onnxruntime/releases/download/v$OrtVersion/onnxruntime-win-x64-$OrtVersion.zip" -OutFile $OrtZip
    if ((Get-FileHash $OrtZip -Algorithm SHA256).Hash.ToLower() -ne $OrtSha) { Fail 'onnxruntime zip sha256 mismatch' }
}
Add-Type -AssemblyName System.IO.Compression.FileSystem
$archive = [System.IO.Compression.ZipFile]::OpenRead($OrtZip)
try {
    $entry = $archive.GetEntry($OrtMember)
    if (-not $entry) { Fail "$OrtZip has no member $OrtMember" }
    [System.IO.Compression.ZipFileExtensions]::ExtractToFile($entry, (Join-Path $Binaries 'onnxruntime.dll'), $true)
} finally { $archive.Dispose() }

function Invoke-CompanionSmoke {
    # Runs Theuth.Companion.exe --probe-notify N and reports whether it exited 0.
    # A WinUI 3 process started from session 0 (ssh, a service, a CI agent) has no interactive window
    # station: Microsoft.UI.Input.dll fail-fasts (0xC0000602) while the app is still starting, before the
    # probe writes its first log line. So when this script runs in session 0 the smoke is executed on the
    # logged-on desktop through a transient scheduled task, and skipped only when nobody is logged on.
    param([Parameter(Mandatory = $true)][string]$Exe, [int]$Seconds = 3, [int]$TimeoutSec = 90)
    $me = (Get-Process -Id $PID).SessionId
    if ($me -ne 0) {
        $p = Start-Process -FilePath $Exe -ArgumentList '--probe-notify', "$Seconds" -PassThru
        if (-not $p.WaitForExit($TimeoutSec * 1000)) { $p.Kill(); return @{ Ok = $false; Why = "did not exit within $TimeoutSec s"; Note = '' } }
        if ($p.ExitCode -ne 0) { return @{ Ok = $false; Why = "exited $($p.ExitCode)"; Note = '' } }
        return @{ Ok = $true; Why = ''; Note = 'companion smoke: --probe-notify ran in this session' }
    }
    $desk = @(Get-Process -Name explorer -ErrorAction SilentlyContinue | Where-Object { $_.SessionId -ne 0 })
    if ($desk.Count -eq 0) {
        return @{ Ok = $true; Why = ''; Note = 'companion smoke: skipped -- session 0 has no interactive desktop to host WinUI; run the package from a desktop shell to exercise it' }
    }
    $sid = [System.Security.Principal.WindowsIdentity]::GetCurrent().User.Value
    $name = "theuth-companion-smoke-$PID"
    $act = New-ScheduledTaskAction -Execute $Exe -Argument "--probe-notify $Seconds"
    $prn = New-ScheduledTaskPrincipal -UserId $sid -LogonType Interactive -RunLevel Limited
    $set = New-ScheduledTaskSettingsSet -ExecutionTimeLimit (New-TimeSpan -Seconds $TimeoutSec)
    $code = $null
    try {
        Register-ScheduledTask -TaskName $name -Action $act -Principal $prn -Settings $set -Force | Out-Null
        $t0 = Get-Date
        Start-ScheduledTask -TaskName $name
        while (((Get-Date) - $t0).TotalSeconds -lt $TimeoutSec) {
            Start-Sleep -Milliseconds 500
            $info = Get-ScheduledTaskInfo -TaskName $name
            if ($info.LastTaskResult -ne 267009 -and $info.LastRunTime -ge $t0.AddSeconds(-2)) { $code = $info.LastTaskResult; break }
        }
    } finally {
        Unregister-ScheduledTask -TaskName $name -Confirm:$false -ErrorAction SilentlyContinue
    }
    if ($null -eq $code) { return @{ Ok = $false; Why = "did not finish in the logged-on session within $TimeoutSec s"; Note = '' } }
    if ($code -ne 0) { return @{ Ok = $false; Why = ("exited 0x{0:X8} in the logged-on session" -f [uint32]$code); Note = '' } }
    return @{ Ok = $true; Why = ''; Note = 'companion smoke: --probe-notify ran in the logged-on session (session 0 cannot host WinUI)' }
}

Write-Host "==> [3b/6] companion (Theuth.Companion.exe, 134 W5)"
$CompanionBuild = Join-Path $Root 'crates/desktop/companion-windows/build.ps1'
$CompanionOut = Join-Path $Root 'target/release/companion-windows'
$CompanionExe = Join-Path $CompanionOut 'Theuth.Companion.exe'
$Companion = $null
if (Test-Path $CompanionBuild) {
    Exec 'companion-windows/build.ps1' { pwsh -NoProfile -File $CompanionBuild -Configuration Release }
    if (-not (Test-Path $CompanionExe)) { Fail "$CompanionExe not produced by build.ps1" }
        if ($env:THEUTH_SKIP_COMPANION_SMOKE -ne '1') {
        # S2 on the release publish output (§5.7): Register() must succeed from the layout NSIS will install.
        $smoke = Invoke-CompanionSmoke -Exe $CompanionExe -Seconds 3
        if (-not $smoke.Ok) { Fail "$CompanionExe --probe-notify 3 $($smoke.Why) (Register() smoke)" }
        Write-Host "    $($smoke.Note)"
    }
    $Companion = $CompanionOut
    Write-Host ("    {0} files, {1}" -f (Get-ChildItem $CompanionOut -Recurse -File).Count, (Format-Size ((Get-ChildItem $CompanionOut -Recurse -File | Measure-Object Length -Sum).Sum)))
} else {
    Write-Host "    skipped: $CompanionBuild not present (134 W1); the installer ships without the companion"
}

Write-Host "==> [4/6] tauri build (nsis)"
if ($env:THEUTH_SKIP_BUN_INSTALL -ne '1') {
    Push-Location (Join-Path $Desktop 'ui')
    try { Exec 'bun install (desktop UI)' { bun install } } finally { Pop-Location }
}
# externalBin/resources are not in tauri.conf.json (tauri-build would fail `cargo check` without
# the files); the app parser's name carries its library hash (the documents helper is compiled into
# t.exe, nothing to ship), so the overlay is rendered here - to a file, not inline, so no JSON
# survives Windows argument quoting.
$BundleConf = Join-Path $Root 'target/tauri.windows.bundle.conf.json'
# A directory key under resources is walked and lands under its value (tauri-utils resources.rs:
# dest.join(path.strip_prefix(dir))), so the ~450-file publish folder becomes $INSTDIR\companion\.
$resources = @{ 'binaries/onnxruntime.dll' = 'onnxruntime.dll' }
if ($Companion) { $resources['../../target/release/companion-windows/'] = 'companion/' }
@{ bundle = @{
    externalBin = @('binaries/t', "binaries/$Helper")
    resources   = $resources
} } | ConvertTo-Json -Depth 4 | Set-Content -Path $BundleConf -Encoding utf8
if (Test-Path $BundleDir) { Remove-Item $BundleDir -Recurse -Force }
Push-Location $Desktop
try {
    $env:TAURI_FRONTEND_PATH = 'ui'
    Exec 'cargo tauri build' { cargo tauri build --config $BundleConf }
} finally { Pop-Location }
$built = @(Get-ChildItem $BundleDir -Filter '*-setup.exe' -File -ErrorAction SilentlyContinue)
if ($built.Count -ne 1) { Fail "expected one *-setup.exe in $BundleDir, got: $($built.Name -join ', ')" }

Write-Host "==> [5/6] -> $Dist"
New-Item -ItemType Directory -Path $Dist -Force | Out-Null
Copy-Item $built[0].FullName $Setup -Force

Write-Host "==> [6/6] CLI zip"
$stage = Join-Path $Root 'target/release/windows-cli-stage'
if (Test-Path $stage) { Remove-Item $stage -Recurse -Force }
New-Item -ItemType Directory -Path $stage | Out-Null
Copy-Item $T (Join-Path $stage 't.exe')
Copy-Item $HelperSrc (Join-Path $stage "$Helper.exe")
if (Test-Path $Zip) { Remove-Item $Zip -Force }
Compress-Archive -Path (Join-Path $stage '*') -DestinationPath $Zip -CompressionLevel Optimal
$archive = [System.IO.Compression.ZipFile]::OpenRead($Zip)
try { $members = @($archive.Entries | ForEach-Object FullName | Sort-Object) } finally { $archive.Dispose() }
$expected = @('t.exe', "$Helper.exe") | Sort-Object
if (($members -join "`n") -ne ($expected -join "`n")) { Fail "$Zip members: $($members -join ', '); expected exactly: $($expected -join ', ')" }

Write-Host ''
Write-Host 'done:'
foreach ($f in @($Setup, $Zip)) { Write-Host ("  {0}  {1}" -f (Format-Size (Get-Item $f).Length), $f) }
Write-Host "  installer: unsigned NSIS, per-user (%LOCALAPPDATA%\theuth); silent: $($built[0].Name) /S"
if ($Companion) { Write-Host "  companion: $CompanionExe -> %LOCALAPPDATA%\theuth\companion\ (Run value removed by nsis/hooks.nsh on uninstall)" } else { Write-Host '  companion: not bundled (build.ps1 absent)' }
Write-Host "  zip: t.exe, $Helper.exe"
Write-Host "  .sig files: release-s3.sh (cargo tauri signer sign) on the Mac"
