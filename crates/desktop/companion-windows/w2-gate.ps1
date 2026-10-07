#requires -Version 7.0
<#
.SYNOPSIS
  Design 134 W2 gate helper for build-B (run through the build service's `script` action, in the interactive session).

.DESCRIPTION
  Actions (first argument):
    notepad                 start Notepad un-elevated (the C10 / hand-back witness) and list its windows
    launch <debug|publish> [flags…]   start Theuth.Companion.exe with flags, detached; prints the pid
    second <debug|publish>  start a second Theuth.Companion.exe, wait for it, print its exit code (single instance)
    kill                    stop every Theuth.Companion
    log [N]                 the last N lines (default 60) of today's companion log
    promote                 show the companion's tray icon in the taskbar corner (Windows 11 NotifyIconSettings IsPromoted=1)
    attention               attention_list on the local engine: kind + id per item
    activity [minutes]      activity_list of the last N minutes (default 30): actor · kind · subject · summary
    rpc <method> [json]     any engine call with positional params (default [])
  Prints UTF-8 (the console default is not).

.EXAMPLE
  pwsh -File crates\desktop\companion-windows\w2-gate.ps1 launch debug --shareable
#>
param(
    [Parameter(Position = 0)] [string] $Action = 'help',
    [Parameter(ValueFromRemainingArguments = $true)] [string[]] $Rest
)
[Console]::OutputEncoding = [Text.Encoding]::UTF8
$ErrorActionPreference = 'Stop'
$here = $PSScriptRoot
$repo = (Resolve-Path (Join-Path $here '..\..\..')).Path
if (-not $Rest) { $Rest = @() }

function Get-Exe([string] $from) {
    switch ($from) {
        'publish' { return Join-Path $repo 'target\release\companion-windows\Theuth.Companion.exe' }
        default { return Join-Path $here 'src\Theuth.Companion\bin\x64\Debug\net10.0-windows10.0.22621.0\win-x64\Theuth.Companion.exe' }
    }
}

function Get-Log([int] $n = 60) {
    $f = Join-Path $env:LOCALAPPDATA ("theuth\companion\logs\companion-{0:yyyy-MM-dd}.log" -f (Get-Date))
    if (Test-Path $f) { Get-Content $f -Tail $n } else { "no log at $f" }
}

function Invoke-Rpc([string] $Method, [string] $ParamsJson = '[]') {
    $dataDir = if ($env:THEUTH_DATA_DIR) { $env:THEUTH_DATA_DIR } else { Join-Path $env:USERPROFILE '.theuth' }
    $token = (Get-Content (Join-Path $dataDir 'token') -Raw).Trim()
    $listen = '127.0.0.1:7777'
    $ej = Join-Path $dataDir 'engine.json'
    if (Test-Path $ej) {
        $l = (Get-Content $ej -Raw | ConvertFrom-Json).listen
        if ($l) { $listen = $l }
    }
    $ws = [System.Net.WebSockets.ClientWebSocket]::new()
    $ws.Options.SetRequestHeader('Authorization', "Bearer $token")
    $ws.Options.SetRequestHeader('X-Theuth-Client', 'w2-gate')
    $ws.Options.Proxy = $null
    $cts = [Threading.CancellationTokenSource]::new(20000)
    try {
        $ws.ConnectAsync([Uri]"ws://$listen/", $cts.Token).GetAwaiter().GetResult()
        $req = '{"id":1,"jsonrpc":"2.0","method":"' + $Method + '","params":' + $ParamsJson + '}'
        $bytes = [Text.Encoding]::UTF8.GetBytes($req)
        $ws.SendAsync([ArraySegment[byte]]::new($bytes), [System.Net.WebSockets.WebSocketMessageType]::Text, $true, $cts.Token).GetAwaiter().GetResult()
        $buf = [byte[]]::new(1MB)
        while ($true) {
            $ms = [IO.MemoryStream]::new()
            do {
                $r = $ws.ReceiveAsync([ArraySegment[byte]]::new($buf), $cts.Token).GetAwaiter().GetResult()
                $ms.Write($buf, 0, $r.Count)
            } while (-not $r.EndOfMessage)
            $obj = [Text.Encoding]::UTF8.GetString($ms.ToArray()) | ConvertFrom-Json -Depth 64
            if ($obj.id -eq 1) { return $obj }
        }
    }
    finally { $ws.Dispose() }
}

switch ($Action) {
    'notepad' {
        Start-Process notepad.exe
        Start-Sleep -Seconds 3
        Get-Process | Where-Object { $_.ProcessName -match 'notepad' } | ForEach-Object { "notepad pid=$($_.Id) title='$($_.MainWindowTitle)' hwnd=0x{0:X}" -f $_.MainWindowHandle.ToInt64() }
    }
    'launch' {
        $from = if ($Rest.Count -gt 0) { $Rest[0] } else { 'debug' }
        $flags = @($Rest | Select-Object -Skip 1)
        $exe = Get-Exe $from
        if (-not (Test-Path $exe)) { throw "no exe at $exe" }
        Get-Process -Name 'Theuth.Companion' -ErrorAction SilentlyContinue | Stop-Process -Force
        Start-Sleep -Milliseconds 500
        $p = if ($flags.Count -gt 0) { Start-Process -FilePath $exe -ArgumentList $flags -PassThru } else { Start-Process -FilePath $exe -PassThru }
        Start-Sleep -Seconds 5
        "launched $exe $($flags -join ' ') pid=$($p.Id) running=$(-not $p.HasExited)"
        Get-Log 25
    }
    'second' {
        $exe = Get-Exe $(if ($Rest.Count -gt 0) { $Rest[0] } else { 'debug' })
        $before = @(Get-Process -Name 'Theuth.Companion' -ErrorAction SilentlyContinue | ForEach-Object Id)
        $sw = [Diagnostics.Stopwatch]::StartNew()
        $p = Start-Process -FilePath $exe -PassThru
        $null = $p.Handle
        $done = $p.WaitForExit(20000)
        "second launch pid=$($p.Id) exited=$done exit=$(if ($done) { $p.ExitCode } else { 'n/a' }) after $([int]$sw.ElapsedMilliseconds) ms"
        Start-Sleep -Seconds 2
        "companions before=[$($before -join ',')] after=[$((Get-Process -Name 'Theuth.Companion' -ErrorAction SilentlyContinue | ForEach-Object Id) -join ',')]"
        Get-Log 12
    }
    'kill' {
        $ps = @(Get-Process -Name 'Theuth.Companion' -ErrorAction SilentlyContinue)
        $ps | Stop-Process -Force
        "killed $($ps.Count) companion(s)"
    }
    'log' { Get-Log $(if ($Rest.Count -gt 0) { [int]$Rest[0] } else { 60 }) }
    'promote' {
        $root = 'HKCU:\Control Panel\NotifyIconSettings'
        $hit = 0
        Get-ChildItem $root -ErrorAction SilentlyContinue | ForEach-Object {
            $v = Get-ItemProperty $_.PSPath
            if ($v.ExecutablePath -like '*Theuth.Companion.exe') {
                Set-ItemProperty -Path $_.PSPath -Name IsPromoted -Value 1 -Type DWord
                $hit++
                "promoted $($_.PSChildName) $($v.ExecutablePath) tip='$($v.InitialTooltip)'"
            }
        }
        if ($hit -eq 0) { 'no NotifyIconSettings entry for Theuth.Companion.exe yet (launch it first)' }
    }
    'attention' {
        $r = Invoke-Rpc 'attention_list'
        if ($r.error) { "error: $($r.error | ConvertTo-Json -Compress)" }
        else { "attention_list=$(@($r.result).Count)"; @($r.result) | ForEach-Object { "  $($_.kind) $($_.id) agent=$($_.agent) task=$($_.task) — $($_.title)" } }
    }
    'activity' {
        $mins = if ($Rest.Count -gt 0) { [int]$Rest[0] } else { 30 }
        $since = [DateTimeOffset]::UtcNow.AddMinutes(-$mins).ToUnixTimeMilliseconds()
        $r = Invoke-Rpc 'activity_list' ('[{"since_ms":' + $since + ',"limit":50}]')
        if ($r.error) { "error: $($r.error | ConvertTo-Json -Compress)" }
        else {
            "activity_list($mins min)=$(@($r.result).Count)"
            @($r.result) | ForEach-Object { "  {0:HH:mm:ss} {1} {2} {3} — {4}" -f [DateTimeOffset]::FromUnixTimeMilliseconds([long]$_.ts_ms).ToLocalTime(), $_.actor, $_.kind, $_.subject, $_.summary }
        }
    }
    'crash' {
        $dir = Join-Path $env:LOCALAPPDATA 'theuth\companion\logs'
        Get-ChildItem $dir -Filter '*.crash.json' -ErrorAction SilentlyContinue | Sort-Object LastWriteTime -Descending | Select-Object -First 2 | ForEach-Object {
            "---- $($_.FullName) $($_.LastWriteTime)"
            Get-Content $_.FullName -Raw
        }
        $mins = if ($Rest.Count -gt 0) { [int]$Rest[0] } else { 20 }
        Get-WinEvent -FilterHashtable @{ LogName = 'Application'; StartTime = (Get-Date).AddMinutes(-$mins) } -ErrorAction SilentlyContinue |
            Where-Object { $_.ProviderName -in @('Application Error', '.NET Runtime', 'Windows Error Reporting') -and $_.Message -match 'Theuth' } |
            Select-Object -First 4 | ForEach-Object { "==== $($_.TimeCreated) $($_.ProviderName) id=$($_.Id)"; ($_.Message -split "`n" | Select-Object -First 40) -join "`n" }
    }
    'rpc' {
        $method = $Rest[0]
        $json = if ($Rest.Count -gt 1) { $Rest[1] } else { '[]' }
        Invoke-Rpc $method $json | ConvertTo-Json -Depth 20 -Compress
    }
    default { Get-Help $MyInvocation.MyCommand.Path -Detailed | Out-String }
}
exit 0
