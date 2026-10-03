#Requires -Version 7
<#
Package theuth for Windows (x86_64): the NSIS desktop installer and the CLI zip, staged for
release-s3.sh. Runs ON the Windows host from the repo root: `just package-windows`, or
`pwsh -NoProfile -File scripts/package-windows.ps1`. Mirrors scripts/package-macos.sh:

  1. cargo build --release -p theuth  (daemon sidecar, x86_64-pc-windows-msvc)
     + packages/apps: bun build --compile -> theuth-app-parser-<hash>.exe (46 S2.1; the same
       hash / --version / stdin-EOF checks as scripts/app-parser-env.sh)
     + packages/documents: bun build --compile -> theuth-documents-<ver>.exe (59 S3.2)
  2. stage all as externalBin: binaries/t-<triple>.exe, binaries/theuth-app-parser-<hash>-<triple>.exe,
     binaries/theuth-documents-<ver>-<triple>.exe (the helpers must sit beside `t.exe` - helper_path -
     so they ride the same mechanism, as package-linux.sh does)
  3. onnxruntime.dll (Microsoft's win-x64 CPU build, pinned in scripts/ort-version.env, sha256-checked,
     zip cached in target/) staged as a Tauri resource that lands beside t.exe in the install dir -
     first in `models::onnx::find_ort_lib`'s search order (<exe dir>\onnxruntime.dll before
     ~/.theuth/models/onnxruntime/). The WebGPU plugin DLLs are not bundled: the app downloads them.
  4. cargo tauri build --config <rendered json>  (target nsis only; crates/desktop/tauri.windows.conf.json:
     currentUser install, WebView2 downloadBootstrapper)
  5. target/release/dist/theuth_<ver>_windows-x86_64-setup.exe
  6. target/release/dist/theuth_<ver>_windows-x86_64.zip = exactly t.exe + both helper exes

Unsigned: no Authenticode on the exes or the installer. The updater signature (.sig) is made by
release-s3.sh on the Mac (cargo tauri signer sign), like the Linux tarballs. Never uploads.

Host tools: rustup (rust-toolchain.toml picks the channel; MSVC target), cargo-tauri, bun.
Env:
  THEUTH_SKIP_BUN_INSTALL=1   skip `bun install` (desktop UI and both packages)
#>
# Progress lines go to the host on purpose: Build-Helper returns a path through the output stream.
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

# `bun run build:helper` in $Pkg, then the compiled exe. Bun appends .exe to a suffix-less
# --outfile on Windows, and both build-helper.ts scripts then copy/self-test the suffix-less name,
# which fails *after* the compile; so the script's exit code is reported, not fatal - the exe
# must exist and pass the same checks release-s3.sh runs on every payload.
function Build-Helper([string]$Pkg, [string]$Stable) {
    $dist = Join-Path $Pkg 'dist'
    if (Test-Path $dist) { Get-ChildItem $dist -File | Remove-Item -Force }
    Push-Location $Pkg
    try {
        $env:OPENUI_TELEMETRY_DISABLED = '1'
        $env:DO_NOT_TRACK = '1'
        if ($env:THEUTH_SKIP_BUN_INSTALL -ne '1') { Exec "bun install ($Pkg)" { bun install --frozen-lockfile } }
        bun run build:helper | Out-Host
        if ($LASTEXITCODE -ne 0) { Write-Host "    note: bun run build:helper exited $LASTEXITCODE (build-helper.ts is not Windows-clean after the compile step); checking the compiled exe" }
    } finally { Pop-Location }
    $exe = Join-Path $dist "$Stable.exe"
    if (-not (Test-Path $exe)) {
        $bare = Join-Path $dist $Stable
        if (Test-Path $bare) { Move-Item $bare $exe -Force } else { Fail "$exe not produced by bun run build:helper" }
    }
    return $exe
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
$Helper = "theuth-app-parser-$HelperHash"
$DocVersion = (Select-String -Path packages/documents/src/protocol.ts -Pattern '^export const PROTOCOL_VERSION = "([0-9.]+)";').Matches[0].Groups[1].Value
if ($DocVersion -notmatch '^[0-9.]+$') { Fail "packages/documents/src/protocol.ts declares no PROTOCOL_VERSION" }
$DocHelper = "theuth-documents-$DocVersion"

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
Exec 'cargo test -p rpc export_bindings' { cargo test -p rpc --features ts --lib --quiet export_bindings }
$uiBindings = Join-Path $Desktop 'ui/src/rpc/bindings'
if (Test-Path $uiBindings) { Remove-Item $uiBindings -Recurse -Force }
New-Item -ItemType Directory -Path $uiBindings | Out-Null
Copy-Item (Join-Path $Root 'bindings/*') $uiBindings -Recurse

Write-Host "==> [1/6] building theuth (release, $Triple)"
Exec 'cargo build --release -p theuth' { cargo build --release -p theuth }
$T = Join-Path $Root 'target/release/t.exe'
if (-not (Test-Path $T)) { Fail "$T not produced" }

Write-Host "==> [1/6] building $Helper.exe (bun build --compile)"
$HelperSrc = Build-Helper 'packages/apps' 'theuth-app-parser'
$said = (& $HelperSrc --version 2>&1 | Out-String).Trim()
if ($LASTEXITCODE -ne 0) { Fail "$HelperSrc --version failed: $said" }
if ($said -notlike "*$HelperHash*") { Fail "$HelperSrc --version printed '$said', expected library hash $HelperHash (stale build? rerun bun run generate && build:helper)" }
Test-StdinEof $HelperSrc

Write-Host "==> [1/6] building $DocHelper.exe (bun build --compile)"
$DocHelperSrc = Build-Helper 'packages/documents' 'theuth-documents'
$said = (& $DocHelperSrc --version 2>&1 | Out-String).Trim()
if ($LASTEXITCODE -ne 0) { Fail "$DocHelperSrc --version failed: $said" }
if ($said -ne "theuth-documents $DocVersion") { Fail "$DocHelperSrc --version printed '$said', expected 'theuth-documents $DocVersion'" }
Test-StdinEof $DocHelperSrc

Write-Host "==> [2/6] staging sidecars (t, $Helper, $DocHelper)"
New-Item -ItemType Directory -Path $Binaries -Force | Out-Null
Get-ChildItem $Binaries -File | Remove-Item -Force
Copy-Item $T (Join-Path $Binaries "t-$Triple.exe")
Copy-Item $HelperSrc (Join-Path $Binaries "$Helper-$Triple.exe")
Copy-Item $DocHelperSrc (Join-Path $Binaries "$DocHelper-$Triple.exe")

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

Write-Host "==> [4/6] tauri build (nsis)"
if ($env:THEUTH_SKIP_BUN_INSTALL -ne '1') {
    Push-Location (Join-Path $Desktop 'ui')
    try { Exec 'bun install (desktop UI)' { bun install } } finally { Pop-Location }
}
# externalBin/resources are not in tauri.conf.json (tauri-build would fail `cargo check` without
# the files); the helper names carry the library hash / protocol version, so the overlay is
# rendered here - to a file, not inline, so no JSON survives Windows argument quoting.
$BundleConf = Join-Path $Root 'target/tauri.windows.bundle.conf.json'
@{ bundle = @{
    externalBin = @('binaries/t', "binaries/$Helper", "binaries/$DocHelper")
    resources   = @{ 'binaries/onnxruntime.dll' = 'onnxruntime.dll' }
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
Copy-Item $DocHelperSrc (Join-Path $stage "$DocHelper.exe")
if (Test-Path $Zip) { Remove-Item $Zip -Force }
Compress-Archive -Path (Join-Path $stage '*') -DestinationPath $Zip -CompressionLevel Optimal
$archive = [System.IO.Compression.ZipFile]::OpenRead($Zip)
try { $members = @($archive.Entries | ForEach-Object FullName | Sort-Object) } finally { $archive.Dispose() }
$expected = @('t.exe', "$Helper.exe", "$DocHelper.exe") | Sort-Object
if (($members -join "`n") -ne ($expected -join "`n")) { Fail "$Zip members: $($members -join ', '); expected exactly: $($expected -join ', ')" }

Write-Host ''
Write-Host 'done:'
foreach ($f in @($Setup, $Zip)) { Write-Host ("  {0}  {1}" -f (Format-Size (Get-Item $f).Length), $f) }
Write-Host "  installer: unsigned NSIS, per-user (%LOCALAPPDATA%\theuth); silent: $($built[0].Name) /S"
Write-Host "  zip: t.exe, $Helper.exe, $DocHelper.exe"
Write-Host "  .sig files: release-s3.sh (cargo tauri signer sign) on the Mac"
