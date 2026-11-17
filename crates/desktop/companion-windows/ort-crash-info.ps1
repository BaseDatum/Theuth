#requires -Version 7.0
# THROWAWAY (chore/ort-1.32-webgpu-0.6): read-only facts about the last webgpu_concurrency crash. Loads nothing.
$ErrorActionPreference = 'Continue'
$since = (Get-Date).AddHours(-3)
Write-Host "== OS: $([Environment]::OSVersion.VersionString), build $((Get-ItemProperty 'HKLM:\SOFTWARE\Microsoft\Windows NT\CurrentVersion').DisplayVersion) UBR $((Get-ItemProperty 'HKLM:\SOFTWARE\Microsoft\Windows NT\CurrentVersion').UBR)"
Write-Host "== System32 D3D12Core.dll: $(Test-Path (Join-Path $env:SystemRoot 'System32\D3D12Core.dll'))"
Write-Host "== Developer Mode: $((Get-ItemProperty 'HKLM:\SOFTWARE\Microsoft\Windows\CurrentVersion\AppModelUnlock' -ErrorAction SilentlyContinue).AllowDevelopmentWithoutDevLicense)"
Write-Host "== GPU driver: $((Get-CimInstance Win32_VideoController | ForEach-Object { "$($_.Name) $($_.DriverVersion)" }) -join '; ')"
Write-Host "== Application Error / WER events since $since"
Get-WinEvent -FilterHashtable @{ LogName = 'Application'; StartTime = $since } -ErrorAction SilentlyContinue |
    Where-Object { $_.ProviderName -in 'Application Error', 'Windows Error Reporting' -and $_.Message -match 'webgpu_concurrency' } |
    Select-Object -First 4 | ForEach-Object { Write-Host "--- $($_.TimeCreated) $($_.ProviderName) $($_.Id)"; Write-Host $_.Message }
Write-Host "== WER report archive"
Get-ChildItem (Join-Path $env:ProgramData 'Microsoft\Windows\WER\ReportArchive'), (Join-Path $env:LOCALAPPDATA 'Microsoft\Windows\WER\ReportArchive') -Directory -ErrorAction SilentlyContinue |
    Where-Object { $_.Name -match 'webgpu_concurrency' -and $_.LastWriteTime -gt $since } | Select-Object -First 2 | ForEach-Object {
        Write-Host "--- $($_.FullName)"
        Get-Content (Join-Path $_.FullName 'Report.wer') -ErrorAction SilentlyContinue | Select-String -Pattern 'Sig\[|DynamicSig|LoadedModule.*(onnx|dx|d3d|webgpu)' | ForEach-Object { Write-Host $_.Line }
    }
