#Requires -Version 7
<#
Pruned Cargo target/ cache backed by Wasabi S3 (Windows CI runners).

Mirrors scripts/ci-target-cache.sh for PowerShell 7 on windows-latest (x86_64)
and windows-11-arm (aarch64):
  - Keying:
      RUSTC_KEY = first 12 hex chars of sha256(`rustc --version --verbose`)
      LOCK_HASH = first 16 hex chars of sha256(Cargo.lock)
      Restore probes `${RUSTC_KEY}-${LOCK_HASH}.tar.{zst|gz}` then `${RUSTC_KEY}-latest.tar.{zst|gz}`.
      Save uploads a single `${RUSTC_KEY}-latest.tar.{zst|gz}` object (no Wasabi storage bloat).
  - Pruning:
      Whitelists only `.fingerprint`, `build`, `deps` (and `mlx-metal` if present) under each
      profile dir, excluding *.pdb, *.dSYM, *.dmg, *.AppImage, *.deb, *.rpm, *.msi, *.ipa,
      *.xcarchive. Does NOT exclude *.exe so `build/**/build-script-build*.exe` are preserved!
  - Compression & binary safety:
      Uses Windows native System32\tar.exe (avoids MSYS tar drive-letter issues) piped via
      raw .NET Stream.CopyTo into `zstd -T0 -3` (100% immune to PowerShell pipeline encoding),
      writing a single compressed archive to $env:RUNNER_TEMP and removing it immediately.
      Falls back gracefully to `tar -czf` / `tar -xzf` (.tar.gz) if `zstd` is absent.
  - Non-fatal:
      All restore and save failures emit ::warning:: and exit 0 so cache hiccups never break releases.

Usage:
  pwsh -NoProfile -File scripts/ci-target-cache.ps1 restore <platform> <profile-dir> [<profile-dir> ...]
  pwsh -NoProfile -File scripts/ci-target-cache.ps1 save    <platform> <profile-dir> [<profile-dir> ...]
  pwsh -NoProfile -File scripts/ci-target-cache.ps1 pack    <out-archive> <profile-dir> [<profile-dir> ...]
#>
[Diagnostics.CodeAnalysis.SuppressMessageAttribute('PSAvoidUsingWriteHost', '')]
[CmdletBinding()]
param(
    [Parameter(Mandatory = $true, Position = 0)]
    [ValidateSet('restore', 'save', 'pack')]
    [string]$Action,

    [Parameter(Mandatory = $true, Position = 1)]
    [string]$TargetArg,

    [Parameter(Mandatory = $true, Position = 2, ValueFromRemainingArguments = $true)]
    [string[]]$ProfileDirs
)

Set-StrictMode -Version Latest
$ErrorActionPreference = 'Stop'
$ProgressPreference = 'SilentlyContinue'

$Endpoint = if ($env:WASABI_S3_ENDPOINT) { $env:WASABI_S3_ENDPOINT.TrimEnd('/') } else { 'https://s3.us-central-1.wasabisys.com' }
$Region = if ($env:WASABI_S3_REGION) { $env:WASABI_S3_REGION } else { 'us-central-1' }
$Bucket = if ($env:WASABI_CACHE_BUCKET) { $env:WASABI_CACHE_BUCKET } else { 'theuth-build-cache' }
$Prefix = if ($env:WASABI_TARGET_CACHE_PREFIX) { $env:WASABI_TARGET_CACHE_PREFIX.Trim('/') } else { 'target' }

$AccessKey = if ($env:WASABI_ACCESS_KEY_ID) { $env:WASABI_ACCESS_KEY_ID } else { $env:AWS_ACCESS_KEY_ID }
$SecretKey = if ($env:WASABI_SECRET_ACCESS_KEY) { $env:WASABI_SECRET_ACCESS_KEY } else { $env:AWS_SECRET_ACCESS_KEY }

# Exclusions applied inside whitelisted subdirectories (.fingerprint/, build/, deps/).
# Note: *.exe is intentionally NOT excluded so build/**/build-script-build*.exe are preserved.
$TarExcludes = @(
    '--exclude=*.pdb',
    '--exclude=*.dSYM',
    '--exclude=*.dmg',
    '--exclude=*.AppImage',
    '--exclude=*.deb',
    '--exclude=*.rpm',
    '--exclude=*.msi',
    '--exclude=*.ipa',
    '--exclude=*.xcarchive'
)

function Add-ToolPaths {
    if (-not $IsWindows) { return }
    $candidates = @(
        'C:\Program Files\Amazon\AWSCLIV2',
        'C:\Program Files\zstd',
        'C:\ProgramData\chocolatey\bin'
    )
    foreach ($dir in $candidates) {
        if ((Test-Path $dir) -and -not ($env:PATH.Split(';') -contains $dir)) {
            $env:PATH = "$dir;$env:PATH"
        }
    }
}

function Resolve-TarExe {
    if ($IsWindows -and $env:SystemRoot) {
        # Prefer Windows built-in bsdtar (System32\tar.exe) over MSYS/Git GNU tar,
        # which misinterprets drive letters like D:\... as remote hostnames.
        $sysTar = Join-Path $env:SystemRoot 'System32\tar.exe'
        if (Test-Path $sysTar) { return $sysTar }
    }
    $cmd = Get-Command tar -ErrorAction SilentlyContinue
    if ($cmd) { return $cmd.Source }
    return $null
}

function Resolve-ZstdExe {
    $cmd = Get-Command zstd -ErrorAction SilentlyContinue
    if ($cmd) { return $cmd.Source }
    return $null
}

function Get-Sha256StringHex([string]$Text) {
    $bytes = [System.Text.Encoding]::UTF8.GetBytes($Text)
    $hashBytes = [System.Security.Cryptography.SHA256]::HashData($bytes)
    return ([System.BitConverter]::ToString($hashBytes) -replace '-', '').ToLowerInvariant()
}

function Get-CacheKeys([string]$Platform, [string]$Ext) {
    $rustcLines = & rustc --version --verbose
    if ($LASTEXITCODE -ne 0) { throw "rustc --version --verbose failed (exit $LASTEXITCODE)" }
    $rustcText = (($rustcLines -join "`n") + "`n")
    $rustcKey = (Get-Sha256StringHex $rustcText).Substring(0, 12)

    if (Test-Path 'Cargo.lock') {
        $lockHash = (Get-FileHash -Path 'Cargo.lock' -Algorithm SHA256).Hash.ToLowerInvariant().Substring(0, 16)
    } else {
        $lockHash = 'nolock'
    }

    return @{
        RustcKey    = $rustcKey
        LockHash    = $lockHash
        PrimaryKey  = "$Prefix/$Platform/$rustcKey-$lockHash$Ext"
        FallbackKey = "$Prefix/$Platform/$rustcKey-latest$Ext"
    }
}

function Invoke-WasabiAws {
    param(
        [Parameter(Mandatory = $true)][string[]]$AwsArgs,
        [switch]$Quiet
    )
    $saved = @{
        AWS_ACCESS_KEY_ID                  = $env:AWS_ACCESS_KEY_ID
        AWS_SECRET_ACCESS_KEY              = $env:AWS_SECRET_ACCESS_KEY
        AWS_DEFAULT_REGION                 = $env:AWS_DEFAULT_REGION
        AWS_EC2_METADATA_DISABLED          = $env:AWS_EC2_METADATA_DISABLED
        AWS_REQUEST_CHECKSUM_CALCULATION   = $env:AWS_REQUEST_CHECKSUM_CALCULATION
        AWS_RESPONSE_CHECKSUM_VALIDATION   = $env:AWS_RESPONSE_CHECKSUM_VALIDATION
    }
    try {
        $env:AWS_ACCESS_KEY_ID = $AccessKey
        $env:AWS_SECRET_ACCESS_KEY = $SecretKey
        $env:AWS_DEFAULT_REGION = $Region
        $env:AWS_EC2_METADATA_DISABLED = 'true'
        $env:AWS_REQUEST_CHECKSUM_CALCULATION = 'when_required'
        $env:AWS_RESPONSE_CHECKSUM_VALIDATION = 'when_required'

        $fullArgs = @('--endpoint-url', $Endpoint, '--region', $Region) + $AwsArgs
        if ($Quiet) {
            $out = & aws @fullArgs 2>$null
            return @{ ExitCode = $LASTEXITCODE; Output = ($out | Out-String).Trim() }
        } else {
            & aws @fullArgs | Out-Host
            return @{ ExitCode = $LASTEXITCODE; Output = '' }
        }
    } finally {
        foreach ($kv in $saved.GetEnumerator()) {
            if ($null -eq $kv.Value) {
                Remove-Item "Env:$($kv.Key)" -ErrorAction SilentlyContinue
            } else {
                Set-Item "Env:$($kv.Key)" -Value $kv.Value
            }
        }
    }
}

function Get-HeadObjectSize([string]$Key) {
    $res = Invoke-WasabiAws -Quiet -AwsArgs @(
        's3api', 'head-object',
        '--bucket', $Bucket,
        '--key', $Key,
        '--query', 'ContentLength',
        '--output', 'text'
    )
    if ($res.ExitCode -eq 0 -and $res.Output -match '^\d+$') {
        return [long]$res.Output
    }
    return $null
}

function Invoke-NativePipe {
    <#
    Binary-safe native process pipeline using .NET ProcessStartInfo.ArgumentList
    and raw BaseStream.CopyTo (immune to PowerShell encoding and quoting issues).
    #>
    param(
        [Parameter(Mandatory = $true)][string]$ProducerExe,
        [Parameter(Mandatory = $true)][string[]]$ProducerArgs,
        [Parameter(Mandatory = $true)][string]$ConsumerExe,
        [Parameter(Mandatory = $true)][string[]]$ConsumerArgs
    )
    $cwd = (Get-Location).Path

    $p1Info = [System.Diagnostics.ProcessStartInfo]::new($ProducerExe)
    $p1Info.WorkingDirectory = $cwd
    foreach ($a in $ProducerArgs) { [void]$p1Info.ArgumentList.Add($a) }
    $p1Info.UseShellExecute = $false
    $p1Info.RedirectStandardOutput = $true
    $p1Info.RedirectStandardError = $false

    $p2Info = [System.Diagnostics.ProcessStartInfo]::new($ConsumerExe)
    $p2Info.WorkingDirectory = $cwd
    foreach ($a in $ConsumerArgs) { [void]$p2Info.ArgumentList.Add($a) }
    $p2Info.UseShellExecute = $false
    $p2Info.RedirectStandardInput = $true
    $p2Info.RedirectStandardOutput = $false
    $p2Info.RedirectStandardError = $false

    $p1 = [System.Diagnostics.Process]::Start($p1Info)
    $p2 = [System.Diagnostics.Process]::Start($p2Info)
    try {
        try {
            $p1.StandardOutput.BaseStream.CopyTo($p2.StandardInput.BaseStream)
        } finally {
            $p2.StandardInput.Close()
        }
        $p1.WaitForExit()
        $p2.WaitForExit()
        if ($p1.ExitCode -ne 0) {
            throw "$ProducerExe exited with code $($p1.ExitCode)"
        }
        if ($p2.ExitCode -ne 0) {
            throw "$ConsumerExe exited with code $($p2.ExitCode)"
        }
    } finally {
        if (-not $p1.HasExited) { try { $p1.Kill() } catch {} }
        if (-not $p2.HasExited) { try { $p2.Kill() } catch {} }
        $p1.Dispose()
        $p2.Dispose()
    }
}

function Get-NormalizedProfileDirs([string[]]$Dirs) {
    $out = @()
    foreach ($d in $Dirs) {
        $norm = ($d -replace '\\', '/').TrimEnd('/')
        if ($norm) { $out += $norm }
    }
    return ,$out
}

function Test-HasNonEmptyDeps([string[]]$Dirs) {
    foreach ($pdir in $Dirs) {
        $depsDir = Join-Path $pdir 'deps'
        if (Test-Path $depsDir -PathType Container) {
            $first = Get-ChildItem -Path $depsDir -Force -ErrorAction SilentlyContinue | Select-Object -First 1
            if ($null -ne $first) { return $true }
        }
    }
    return $false
}

function Get-IncludePaths([string[]]$Dirs) {
    $paths = @()
    foreach ($pdir in $Dirs) {
        foreach ($sub in @('.fingerprint', 'build', 'deps', 'mlx-metal')) {
            $candidate = "$pdir/$sub"
            if (Test-Path $candidate -PathType Container) {
                $paths += $candidate
            }
        }
    }
    return ,$paths
}

function Invoke-PackArchive {
    param(
        [Parameter(Mandatory = $true)][string]$OutArchive,
        [Parameter(Mandatory = $true)][string[]]$IncludePaths,
        [Parameter(Mandatory = $true)][string]$TarExe,
        [string]$ZstdExe
    )
    if ($ZstdExe -and $OutArchive.EndsWith('.tar.zst')) {
        $tarArgs = @() + $TarExcludes + @('-cf', '-') + $IncludePaths
        $zstdArgs = @('-T0', '-3', '-q', '-f', '-o', $OutArchive)
        Invoke-NativePipe -ProducerExe $TarExe -ProducerArgs $tarArgs -ConsumerExe $ZstdExe -ConsumerArgs $zstdArgs
    } else {
        $tarArgs = @() + $TarExcludes + @('-czf', $OutArchive) + $IncludePaths
        & $TarExe @tarArgs
        if ($LASTEXITCODE -ne 0) {
            throw "$TarExe -czf failed (exit $LASTEXITCODE)"
        }
    }
}

function Invoke-ExtractArchive {
    param(
        [Parameter(Mandatory = $true)][string]$InArchive,
        [Parameter(Mandatory = $true)][string]$TarExe,
        [string]$ZstdExe
    )
    if ($ZstdExe -and $InArchive.EndsWith('.tar.zst')) {
        Invoke-NativePipe -ProducerExe $ZstdExe -ProducerArgs @('-dc', '-q', $InArchive) -ConsumerExe $TarExe -ConsumerArgs @('-xf', '-')
    } else {
        & $TarExe -xzf $InArchive
        if ($LASTEXITCODE -ne 0) {
            throw "$TarExe -xzf failed (exit $LASTEXITCODE)"
        }
    }
}

function Get-TempRoot {
    if ($env:RUNNER_TEMP -and (Test-Path $env:RUNNER_TEMP)) { return $env:RUNNER_TEMP }
    return [System.IO.Path]::GetTempPath()
}

function Test-S3Prereqs([string]$Act) {
    if ([string]::IsNullOrEmpty($AccessKey) -or [string]::IsNullOrEmpty($SecretKey)) {
        Write-Host "Skipping target/ cache ${Act}: WASABI_ACCESS_KEY_ID / WASABI_SECRET_ACCESS_KEY not set"
        return $false
    }
    foreach ($tool in @('aws', 'rustc')) {
        if (-not (Get-Command $tool -ErrorAction SilentlyContinue)) {
            Write-Host "::warning::Skipping target/ cache ${Act}: required tool '$tool' not found on PATH"
            return $false
        }
    }
    if (-not (Resolve-TarExe)) {
        Write-Host "::warning::Skipping target/ cache ${Act}: required tool 'tar' not found"
        return $false
    }
    return $true
}

function Invoke-Restore([string]$Platform, [string[]]$Dirs) {
    Add-ToolPaths
    if (-not (Test-S3Prereqs 'restore')) { return }

    $tarExe = Resolve-TarExe
    $zstdExe = Resolve-ZstdExe
    $ext = if ($zstdExe) { '.tar.zst' } else { '.tar.gz' }
    if (-not $zstdExe) {
        Write-Host "Note: zstd not found; using gzip (.tar.gz) fallback for target/ cache"
    }

    $keys = Get-CacheKeys -Platform $Platform -Ext $ext
    $selectedKey = $null
    $size = $null
    $kind = ''

    $primarySize = Get-HeadObjectSize $keys.PrimaryKey
    if ($null -ne $primarySize) {
        $selectedKey = $keys.PrimaryKey
        $size = $primarySize
        $kind = 'exact'
    } else {
        $fallbackSize = Get-HeadObjectSize $keys.FallbackKey
        if ($null -ne $fallbackSize) {
            $selectedKey = $keys.FallbackKey
            $size = $fallbackSize
            $kind = 'latest fallback'
        }
    }

    if (-not $selectedKey) {
        Write-Host "Wasabi target/ cache miss for $Platform (rustc=$($keys.RustcKey), lock=$($keys.LockHash), ext=$ext)"
        return
    }

    $uri = "s3://$Bucket/$selectedKey"
    $sizeMiB = [math]::Round($size / 1MB, 1)
    Write-Host "Restoring target/ cache ($kind, ~$sizeMiB MiB) from $uri ..."
    $sw = [System.Diagnostics.Stopwatch]::StartNew()
    $tmpArchive = Join-Path (Get-TempRoot) "target-cache-restore-$PID$ext"
    try {
        $dl = Invoke-WasabiAws -AwsArgs @(
            's3', 'cp', $uri, $tmpArchive,
            '--only-show-errors',
            '--cli-connect-timeout', '30',
            '--cli-read-timeout', '300'
        )
        if ($dl.ExitCode -ne 0) {
            throw "aws s3 cp download failed (exit $($dl.ExitCode))"
        }
        Invoke-ExtractArchive -InArchive $tmpArchive -TarExe $tarExe -ZstdExe $zstdExe
        $sw.Stop()
        Write-Host ("Restored target/ cache from $uri in {0:N1}s" -f $sw.Elapsed.TotalSeconds)
    } catch {
        Write-Host "::warning::Failed to restore target/ cache from ${uri}: $($_.Exception.Message); cleaning partial cache dirs and continuing with cold build"
        foreach ($pdir in $Dirs) {
            foreach ($sub in @('.fingerprint', 'build', 'deps', 'mlx-metal')) {
                $p = Join-Path $pdir $sub
                if (Test-Path $p) { Remove-Item -Path $p -Recurse -Force -ErrorAction SilentlyContinue }
            }
        }
    } finally {
        if (Test-Path $tmpArchive) { Remove-Item -Path $tmpArchive -Force -ErrorAction SilentlyContinue }
    }
}

function Invoke-Save([string]$Platform, [string[]]$Dirs) {
    Add-ToolPaths
    if (-not (Test-S3Prereqs 'save')) { return }

    if (-not (Test-HasNonEmptyDeps $Dirs)) {
        Write-Host "Skipping target/ cache save for ${Platform}: no non-empty deps/ directory under: $($Dirs -join ', ')"
        return
    }

    $includePaths = Get-IncludePaths $Dirs
    if ($includePaths.Count -eq 0) {
        Write-Host "Skipping target/ cache save for ${Platform}: no cacheable subdirectories found"
        return
    }

    $tarExe = Resolve-TarExe
    $zstdExe = Resolve-ZstdExe
    $ext = if ($zstdExe) { '.tar.zst' } else { '.tar.gz' }
    if (-not $zstdExe) {
        Write-Host "Note: zstd not found; using gzip (.tar.gz) fallback for target/ cache"
    }

    $keys = Get-CacheKeys -Platform $Platform -Ext $ext
    $latestUri = "s3://$Bucket/$($keys.FallbackKey)"
    Write-Host "Saving pruned target/ cache ($($includePaths -join ' ')) to $latestUri ..."
    $sw = [System.Diagnostics.Stopwatch]::StartNew()
    $tmpArchive = Join-Path (Get-TempRoot) "target-cache-save-$PID$ext"
    try {
        Invoke-PackArchive -OutArchive $tmpArchive -IncludePaths $includePaths -TarExe $tarExe -ZstdExe $zstdExe
        $archiveBytes = (Get-Item $tmpArchive).Length
        $archiveMiB = [math]::Round($archiveBytes / 1MB, 1)
        $ul = Invoke-WasabiAws -AwsArgs @(
            's3', 'cp', $tmpArchive, $latestUri,
            '--only-show-errors',
            '--cli-connect-timeout', '30',
            '--cli-read-timeout', '300'
        )
        if ($ul.ExitCode -ne 0) {
            throw "aws s3 cp upload failed (exit $($ul.ExitCode))"
        }
        $sw.Stop()
        Write-Host ("Saved target/ cache (~$archiveMiB MiB) to $latestUri in {0:N1}s" -f $sw.Elapsed.TotalSeconds)
    } catch {
        Write-Host "::warning::Failed to save pruned target/ cache to ${latestUri} (non-fatal): $($_.Exception.Message)"
    } finally {
        if (Test-Path $tmpArchive) { Remove-Item -Path $tmpArchive -Force -ErrorAction SilentlyContinue }
    }
}

function Invoke-PackOnly([string]$OutArchive, [string[]]$Dirs) {
    Add-ToolPaths
    if (-not (Test-HasNonEmptyDeps $Dirs)) {
        throw "No non-empty deps/ directory under: $($Dirs -join ', ')"
    }
    $includePaths = Get-IncludePaths $Dirs
    if ($includePaths.Count -eq 0) {
        throw "No cacheable subdirectories found under: $($Dirs -join ', ')"
    }
    $tarExe = Resolve-TarExe
    if (-not $tarExe) { throw 'tar not found' }
    $zstdExe = Resolve-ZstdExe
    Invoke-PackArchive -OutArchive $OutArchive -IncludePaths $includePaths -TarExe $tarExe -ZstdExe $zstdExe
}

$normDirs = Get-NormalizedProfileDirs $ProfileDirs
if ($normDirs.Count -eq 0) {
    if ($Action -eq 'pack') { throw 'At least one profile directory is required' }
    Write-Host "::warning::No profile directories supplied to ci-target-cache.ps1"
    exit 0
}

switch ($Action) {
    'restore' {
        try {
            Invoke-Restore -Platform $TargetArg -Dirs $normDirs
        } catch {
            Write-Host "::warning::target/ cache restore failed unexpectedly (non-fatal): $($_.Exception.Message)"
        }
        exit 0
    }
    'save' {
        try {
            Invoke-Save -Platform $TargetArg -Dirs $normDirs
        } catch {
            Write-Host "::warning::target/ cache save failed unexpectedly (non-fatal): $($_.Exception.Message)"
        }
        exit 0
    }
    'pack' {
        Invoke-PackOnly -OutArchive $TargetArg -Dirs $normDirs
    }
}
