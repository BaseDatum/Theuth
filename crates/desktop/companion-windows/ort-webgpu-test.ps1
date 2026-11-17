#requires -Version 7.0
<#
.SYNOPSIS
  Run the WebGPU concurrency test (crates/meeting/tests/webgpu_concurrency.rs) against the ONNX Runtime + WebGPU plugin
  pinned in scripts/ort-version.env, not the copies in %USERPROFILE%\.theuth\models\onnxruntime.

.DESCRIPTION
  For the build service's `script` action on a Windows GPU box, when the pins are bumped (docs/design/fix-webgpu-run-lock.md).
  Downloads Microsoft's onnxruntime-win-x64-<ORT_VERSION>.zip and the Microsoft.ML.OnnxRuntime.EP.WebGpu nupkg into
  %TEMP%\theuth-ort-verify\, checks both against the sha256 pins, lays out a scratch data dir (runtime + VERSION, the
  plugin DLLs release-models.sh ships, junctions to the real model dirs, which stay read-only), points THEUTH_DATA_DIR
  at it and runs the test once as its own process. Build it first with a cargo-test job (`--no-run`, same -p set) so
  this run compiles nothing. Exit code = the test's.

  With -NoPlugin the data dir gets no plugins\ folder: a CPU-only control (the runtime loads and the diariser opens on
  the CPU; the test then skips) that never loads the WebGPU plugin.

.EXAMPLE
  pwsh -File crates\desktop\companion-windows\ort-webgpu-test.ps1 [-NoPlugin]
#>
param([switch] $NoPlugin)
$ErrorActionPreference = 'Stop'
$repo = (Resolve-Path (Join-Path $PSScriptRoot '..\..\..')).Path
$pins = Get-Content (Join-Path $repo 'scripts\ort-version.env') -Raw
function Pin([string] $name) {
    $v = [regex]::Match($pins, "(?m)^$name=`"([^`"]+)`"").Groups[1].Value
    if (-not $v) { throw "scripts/ort-version.env lacks $name" }
    $v
}
$ort = Pin 'ORT_VERSION'; $ortSha = Pin 'ORT_SHA256_WIN_X64'
$ep = Pin 'WEBGPU_EP_VERSION'; $epSha = Pin 'WEBGPU_EP_SHA256'
Write-Host "pins: ONNX Runtime $ort, WebGPU plugin $ep"

$work = Join-Path $env:TEMP 'theuth-ort-verify'
New-Item -ItemType Directory -Force $work | Out-Null
function Fetch([string] $url, [string] $file, [string] $sha) {
    $path = Join-Path $work $file
    if (-not ((Test-Path $path) -and (Get-FileHash $path -Algorithm SHA256).Hash -eq $sha)) {
        Write-Host "download $url"
        Invoke-WebRequest -Uri $url -OutFile $path
    }
    $got = (Get-FileHash $path -Algorithm SHA256).Hash.ToLower()
    if ($got -ne $sha) { throw "$file sha256 $got, pinned $sha" }
    Write-Host "ok $file sha256 $got"
    $path
}
$zip = Fetch "https://github.com/microsoft/onnxruntime/releases/download/v$ort/onnxruntime-win-x64-$ort.zip" "onnxruntime-win-x64-$ort.zip" $ortSha
$nupkg = Fetch "https://api.nuget.org/v3-flatcontainer/microsoft.ml.onnxruntime.ep.webgpu/$ep/microsoft.ml.onnxruntime.ep.webgpu.$ep.nupkg" "microsoft.ml.onnxruntime.ep.webgpu.$ep.nupkg" $epSha

$data = Join-Path $work ("data-$ort-$ep" + $(if ($NoPlugin) { '-cpu' } else { '' }))   # reused across runs; never deleted here (it holds junctions to the real models)
$rt = Join-Path $data 'models\onnxruntime'
$plugins = Join-Path $rt 'plugins'
New-Item -ItemType Directory -Force $rt | Out-Null
Add-Type -AssemblyName System.IO.Compression.FileSystem
function Extract([string] $archive, [string] $member, [string] $dest) {
    $z = [System.IO.Compression.ZipFile]::OpenRead($archive)
    try {
        $e = $z.GetEntry($member)
        if (-not $e) { throw "$member not in $archive" }
        [System.IO.Compression.ZipFileExtensions]::ExtractToFile($e, $dest, $true)
    } finally { $z.Dispose() }
}
Extract $zip "onnxruntime-win-x64-$ort/lib/onnxruntime.dll" (Join-Path $rt 'onnxruntime.dll')
Set-Content -NoNewline -Path (Join-Path $rt 'VERSION') -Value "$ort`n"
if (-not $NoPlugin) {
    New-Item -ItemType Directory -Force (Join-Path $plugins 'D3D12') | Out-Null
    # release-models.sh plugin_files_of win-*: D3D12\ beside the plugin, as the package's buildTransitive targets copy it.
    foreach ($f in 'onnxruntime_providers_webgpu.dll', 'dxcompiler.dll', 'D3D12/D3D12Core.dll', 'D3D12/d3d12SDKLayers.dll') {
        Extract $nupkg "runtimes/win-x64/native/$f" (Join-Path $plugins $f)
    }
}
$models = Join-Path $env:USERPROFILE '.theuth\models'
foreach ($m in 'nemotron-streaming-en', 'nemotron-diarization', 'pyannote') {
    $link = Join-Path $data "models\$m"
    if (-not (Test-Path $link)) { New-Item -ItemType Junction -Path $link -Target (Join-Path $models $m) | Out-Null }
}
Get-ChildItem -Recurse -File $rt | ForEach-Object { Write-Host ("  {0} {1} sha256 {2}" -f $_.FullName.Substring($data.Length + 1), $_.Length, (Get-FileHash $_.FullName -Algorithm SHA256).Hash.ToLower()) }

$env:THEUTH_DATA_DIR = $data
Remove-Item Env:ORT_DYLIB_PATH -ErrorAction SilentlyContinue
Push-Location $repo
try {
    $t = [Diagnostics.Stopwatch]::StartNew()
    & cargo test -p meeting --test webgpu_concurrency -- --ignored --nocapture
    $code = $LASTEXITCODE
    Write-Host ("webgpu_concurrency: exit $code after {0:n1} s" -f $t.Elapsed.TotalSeconds)
} finally { Pop-Location }
exit $code
