#requires -Version 7.0
<#
.SYNOPSIS
  Seed (or clear) real attention items on this machine's engine for companion UI tests (design 134 W1b).
.EXAMPLE
  pwsh -File crates\desktop\companion-windows\seed.ps1 all
  pwsh -File crates\desktop\companion-windows\seed.ps1 decision --count 3
  pwsh -File crates\desktop\companion-windows\seed.ps1 clear
#>
param([Parameter(ValueFromRemainingArguments = $true)] [string[]] $Rest)
$ErrorActionPreference = 'Stop'
$proj = Join-Path $PSScriptRoot 'tools\Theuth.Companion.Seed\Theuth.Companion.Seed.csproj'
if (-not $Rest -or $Rest.Count -eq 0) { $Rest = @('help') }
& dotnet run --project $proj -c Release -- @Rest
exit $LASTEXITCODE