#requires -Version 7.0
<#
.SYNOPSIS
  Design 149 WW3 gate helper for build-B (run through the build service's `script` action, in the interactive session).
  Every output line starts with "G3: ".

.DESCRIPTION
  Actions (first argument):
    crash [N]            the newest N (default 1) companion crash reports (%LOCALAPPDATA%\theuth\companion\logs\*.crash.json), full text
    log [N]              the last N lines (default 80) of today's companion log
    grep <regex> [N]     the last N (default 60) companion-log lines matching the regex (the regex may not contain `|`: use `point.*|…` → `point`)
    island               the island / overlay / point lines of today's log (last 80)
    hold-voice [ms]      hold Ctrl+Alt+Space for ms (default 1500) with SendInput, then release (the voice key)
    tap-voice            tap Ctrl+Alt+Space twice 120 ms apart (the double-tap lock), then tap once 1.6 s later (the send)
    esc                  tap Esc
    click <x> <y> [ms]   149 island fix: a human-paced left click at (x, y) — move, settle 120 ms, press, hold ms (default 120), release
    key <name>           149 island fix: tap one key with SendInput (j k m e d up down esc enter); prints the time
    ctrl-m               149 island fix: Ctrl down, M down/up, Ctrl up (the island's stop key; a RegisterHotKey-free SendInput chord)
    type <text>          149 island fix: type letters/digits into the focused control with SendInput (A-Z 0-9 only)
    hold-draw [ms]       149 island fix §P: hold Ctrl+Alt+Space and sweep the mouse left → right across the screen and back down
                         over ms (default 6000, 16 ms steps, SendInput moves), then release — the pill must stay put
    move <x> <y>         move the mouse to screen (x, y)
    draw                 move the mouse along a small loop (≈ 1 s) — the trail
    shot <name> [x y w h] screenshot (whole screen or a region) to C:\Users\user\tools\pkg\ww3\<name>.png
    b64 <name>           that PNG as base64 lines (read back through GET /log)
    engine-log [N]       the last N lines (default 40) of the installed engine's log mentioning screen_context / meeting / dictation
    speech-dump          the --speech-dump packets file (%LOCALAPPDATA%\theuth\companion\speech-dump.json) as DUMP lines
    settings-json        print settings.json
#>
param(
    [Parameter(Position = 0)] [string] $Action = 'help',
    [Parameter(ValueFromRemainingArguments = $true)] [string[]] $Rest
)
[Console]::OutputEncoding = [Text.Encoding]::UTF8
$ErrorActionPreference = 'Continue'
if (-not $Rest) { $Rest = @() }
$pkg = 'C:\Users\user\tools\pkg'
$logDir = Join-Path $env:LOCALAPPDATA 'theuth\companion\logs'
function Say([string] $s) { Write-Output "G3: $s" }
function Clip([string] $s, [int] $n = 300) { if ($null -eq $s) { return '' }; return $s.Substring(0, [math]::Min($n, $s.Length)) }
function LogFile { Join-Path $logDir ("companion-{0:yyyy-MM-dd}.log" -f (Get-Date)) }

$native = @'
using System;
using System.Runtime.InteropServices;
public static class G3Native {
    [StructLayout(LayoutKind.Sequential)] public struct INPUT { public uint type; public InputUnion u; }
    [StructLayout(LayoutKind.Explicit)] public struct InputUnion { [FieldOffset(0)] public MOUSEINPUT mi; [FieldOffset(0)] public KEYBDINPUT ki; }
    [StructLayout(LayoutKind.Sequential)] public struct KEYBDINPUT { public ushort wVk; public ushort wScan; public uint dwFlags; public uint time; public IntPtr dwExtraInfo; }
    [StructLayout(LayoutKind.Sequential)] public struct MOUSEINPUT { public int dx; public int dy; public uint mouseData; public uint dwFlags; public uint time; public IntPtr dwExtraInfo; }
    [DllImport("user32.dll", SetLastError = true)] public static extern uint SendInput(uint n, INPUT[] inputs, int size);
    [DllImport("user32.dll")] public static extern bool SetCursorPos(int x, int y);
    public static uint Mouse(uint flags) {
        var i = new INPUT { type = 0 };
        i.u.mi = new MOUSEINPUT { dwFlags = flags };
        return SendInput(1, new[] { i }, Marshal.SizeOf<INPUT>());
    }
    public static uint Key(ushort vk, bool up) {
        var i = new INPUT { type = 1 };
        i.u.ki = new KEYBDINPUT { wVk = vk, dwFlags = up ? 2u : 0u };
        return SendInput(1, new[] { i }, Marshal.SizeOf<INPUT>());
    }
}
'@
if (-not ([System.Management.Automation.PSTypeName]'G3Native').Type) { Add-Type -TypeDefinition $native }

function Chord([bool] $down) {
    # Ctrl (0x11), Alt (0x12), Space (0x20)
    if ($down) { [void][G3Native]::Key(0x11, $false); [void][G3Native]::Key(0x12, $false); [void][G3Native]::Key(0x20, $false) }
    else { [void][G3Native]::Key(0x20, $true); [void][G3Native]::Key(0x12, $true); [void][G3Native]::Key(0x11, $true) }
}

switch ($Action) {
    'crash' {
        $n = if ($Rest.Count -gt 0) { [int]$Rest[0] } else { 1 }
        $files = @(Get-ChildItem -Path $logDir -Filter '*.crash.json' -ErrorAction SilentlyContinue | Sort-Object LastWriteTime -Descending | Select-Object -First $n)
        if ($files.Count -eq 0) { Say "crash: none under $logDir"; break }
        foreach ($f in $files) {
            Say "-- $($f.Name) $($f.LastWriteTime.ToString('HH:mm:ss')) $($f.Length) bytes"
            Get-Content $f.FullName | ForEach-Object { Say (Clip $_ 400) }
        }
    }
    'log' {
        $n = if ($Rest.Count -gt 0) { [int]$Rest[0] } else { 80 }
        $cl = LogFile
        if (Test-Path $cl) { Get-Content $cl -Tail $n | ForEach-Object { Say (Clip $_ 300) } } else { Say "no $cl" }
    }
    'grep' {
        $pat = if ($Rest.Count -gt 0) { $Rest[0] } else { '.' }
        $n = if ($Rest.Count -gt 1) { [int]$Rest[1] } else { 60 }
        $cl = LogFile
        if (Test-Path $cl) { Select-String -Path $cl -Pattern $pat | Select-Object -Last $n | ForEach-Object { Say (Clip $_.Line 300) } } else { Say "no $cl" }
    }
    'island' {
        $cl = LogFile
        if (Test-Path $cl) { Select-String -Path $cl -Pattern 'island|overlay|point|monitors|screen_context|WW3|149' | Select-Object -Last 80 | ForEach-Object { Say (Clip $_.Line 300) } } else { Say "no $cl" }
    }
    'hold-voice' {
        $ms = if ($Rest.Count -gt 0) { [int]$Rest[0] } else { 1500 }
        Say "hold-voice: Ctrl+Alt+Space down at $(Get-Date -Format 'HH:mm:ss.fff') for $ms ms"
        Chord $true
        Start-Sleep -Milliseconds $ms
        Chord $false
        Say "hold-voice: up at $(Get-Date -Format 'HH:mm:ss.fff')"
    }
    'hold-draw' {
        # the voice key held while the pointer loops (≈ 1 s), a screenshot of the region mid-hold, then the release
        $name = if ($Rest.Count -gt 0) { $Rest[0] } else { 'trail' }
        Add-Type -AssemblyName System.Drawing
        Add-Type -AssemblyName System.Windows.Forms
        Say "hold-draw: Ctrl+Alt+Space down at $(Get-Date -Format 'HH:mm:ss.fff')"
        Chord $true
        Start-Sleep -Milliseconds 400
        $cx = 960; $cy = 560
        for ($i = 0; $i -le 70; $i++) {
            $a = $i / 70.0 * 2 * [math]::PI
            [void][G3Native]::SetCursorPos([int]($cx + 160 * [math]::Cos($a)), [int]($cy + 100 * [math]::Sin($a)))
            Start-Sleep -Milliseconds 16
        }
        Start-Sleep -Milliseconds 300
        $dir = Join-Path $pkg 'ww3'
        New-Item -ItemType Directory -Path $dir -Force | Out-Null
        $bmp = New-Object System.Drawing.Bitmap 1920, 760
        $g = [System.Drawing.Graphics]::FromImage($bmp)
        $g.CopyFromScreen(0, 0, 0, 0, $bmp.Size, [System.Drawing.CopyPixelOperation]([System.Drawing.CopyPixelOperation]::SourceCopy -bor [System.Drawing.CopyPixelOperation]::CaptureBlt))
        $file = Join-Path $dir "$name.png"
        $bmp.Save($file, [System.Drawing.Imaging.ImageFormat]::Png)
        $g.Dispose(); $bmp.Dispose()
        Say "hold-draw: shot $file size=$((Get-Item $file).Length) at $(Get-Date -Format 'HH:mm:ss.fff')"
        $after = if ($Rest.Count -gt 1) { [int]$Rest[1] } else { 0 }
        if ($after -gt 0) { Start-Sleep -Milliseconds $after }
        Chord $false
        Say "hold-draw: up at $(Get-Date -Format 'HH:mm:ss.fff')"
    }
    'tap-voice' {
        Say "tap-voice: tap, tap (120 ms), then a tap after 1.6 s at $(Get-Date -Format 'HH:mm:ss.fff')"
        Chord $true; Start-Sleep -Milliseconds 60; Chord $false
        Start-Sleep -Milliseconds 120
        Chord $true; Start-Sleep -Milliseconds 60; Chord $false
        Start-Sleep -Milliseconds 1600
        Chord $true; Start-Sleep -Milliseconds 60; Chord $false
        Say "tap-voice: done at $(Get-Date -Format 'HH:mm:ss.fff')"
    }
    'drag' {
        # a slow mouse drag: left down at (x0, y0), 40 steps over ~700 ms to (x1, y1), left up; then a second line says where the cursor is
        $x0 = [int]$Rest[0]; $y0 = [int]$Rest[1]; $x1 = [int]$Rest[2]; $y1 = [int]$Rest[3]
        [void][G3Native]::SetCursorPos($x0, $y0)
        Start-Sleep -Milliseconds 120
        [void][G3Native]::Mouse(0x0002)
        Start-Sleep -Milliseconds 120
        for ($i = 1; $i -le 40; $i++) {
            [void][G3Native]::SetCursorPos([int]($x0 + ($x1 - $x0) * $i / 40), [int]($y0 + ($y1 - $y0) * $i / 40))
            Start-Sleep -Milliseconds 16
        }
        Start-Sleep -Milliseconds 150
        [void][G3Native]::Mouse(0x0004)
        Say "drag: ($x0,$y0) -> ($x1,$y1) done at $(Get-Date -Format 'HH:mm:ss.fff')"
    }
    'dblclick' {
        $x = [int]$Rest[0]; $y = [int]$Rest[1]
        [void][G3Native]::SetCursorPos($x, $y)
        Start-Sleep -Milliseconds 100
        [void][G3Native]::Mouse(0x0002); Start-Sleep -Milliseconds 40; [void][G3Native]::Mouse(0x0004)
        Start-Sleep -Milliseconds 120
        [void][G3Native]::Mouse(0x0002); Start-Sleep -Milliseconds 40; [void][G3Native]::Mouse(0x0004)
        Say "dblclick: ($x,$y) at $(Get-Date -Format 'HH:mm:ss.fff')"
    }
    'esc' { [void][G3Native]::Key(0x1B, $false); Start-Sleep -Milliseconds 40; [void][G3Native]::Key(0x1B, $true); Say 'esc: sent' }
    'click' {
        $x = [int]$Rest[0]; $y = [int]$Rest[1]
        $hold = if ($Rest.Count -gt 2) { [int]$Rest[2] } else { 120 }
        [void][G3Native]::SetCursorPos($x, $y)
        Start-Sleep -Milliseconds 120
        [void][G3Native]::Mouse(0x0002); Start-Sleep -Milliseconds $hold; [void][G3Native]::Mouse(0x0004)
        Say "click: ($x,$y) held $hold ms at $(Get-Date -Format 'HH:mm:ss.fff')"
    }
    'key' {
        $map = @{ j = 0x4A; k = 0x4B; m = 0x4D; e = 0x45; d = 0x44; up = 0x26; down = 0x28; esc = 0x1B; enter = 0x0D }
        $name = [string]$Rest[0]
        if (-not $map.ContainsKey($name)) { Say "key: unknown $name"; break }
        $vk = [ushort]$map[$name]
        [void][G3Native]::Key($vk, $false); Start-Sleep -Milliseconds 40; [void][G3Native]::Key($vk, $true)
        Say "key: $name at $(Get-Date -Format 'HH:mm:ss.fff')"
    }
    'hold-draw' {
        $ms = if ($Rest.Count -gt 0) { [int]$Rest[0] } else { 6000 }
        [void][G3Native]::SetCursorPos(200, 500)
        Start-Sleep -Milliseconds 100
        Say "hold-draw: chord down at $(Get-Date -Format 'HH:mm:ss.fff')"
        Chord $true
        $steps = [int]($ms / 16)
        for ($i = 0; $i -le $steps; $i++) {
            $t = $i / [double]$steps
            $x = [int](200 + 1500 * [math]::Sin($t * [math]::PI))
            $y = [int](500 + 350 * $t)
            [void][G3Native]::SetCursorPos($x, $y)
            [void][G3Native]::Mouse(0x0001) # MOUSEEVENTF_MOVE (0,0): a real WM_MOUSEMOVE at the new position
            Start-Sleep -Milliseconds 16
        }
        Chord $false
        Say "hold-draw: chord up at $(Get-Date -Format 'HH:mm:ss.fff') after $ms ms"
    }
    'ctrl-m' {
        [void][G3Native]::Key(0x11, $false); Start-Sleep -Milliseconds 30
        [void][G3Native]::Key(0x4D, $false); Start-Sleep -Milliseconds 40; [void][G3Native]::Key(0x4D, $true)
        Start-Sleep -Milliseconds 30; [void][G3Native]::Key(0x11, $true)
        Say "ctrl-m: sent at $(Get-Date -Format 'HH:mm:ss.fff')"
    }
    'type' {
        foreach ($ch in ([string]$Rest[0]).ToUpperInvariant().ToCharArray()) {
            $vk = [ushort][int]$ch
            [void][G3Native]::Key($vk, $false); Start-Sleep -Milliseconds 25; [void][G3Native]::Key($vk, $true); Start-Sleep -Milliseconds 25
        }
        Say "type: $($Rest[0]) at $(Get-Date -Format 'HH:mm:ss.fff')"
    }
    'move' {
        $x = [int]$Rest[0]; $y = [int]$Rest[1]
        [void][G3Native]::SetCursorPos($x, $y)
        Say "move: ($x, $y)"
    }
    'draw' {
        $cx = if ($Rest.Count -gt 0) { [int]$Rest[0] } else { 512 }
        $cy = if ($Rest.Count -gt 1) { [int]$Rest[1] } else { 400 }
        for ($i = 0; $i -le 60; $i++) {
            $a = $i / 60.0 * 2 * [math]::PI
            [void][G3Native]::SetCursorPos([int]($cx + 120 * [math]::Cos($a)), [int]($cy + 80 * [math]::Sin($a)))
            Start-Sleep -Milliseconds 16
        }
        Say "draw: loop around ($cx, $cy) done at $(Get-Date -Format 'HH:mm:ss.fff')"
    }
    'shot' {
        Add-Type -AssemblyName System.Drawing
        Add-Type -AssemblyName System.Windows.Forms
        $name = if ($Rest.Count -gt 0) { $Rest[0] } else { 'shot' }
        $dir = Join-Path $pkg 'ww3'
        New-Item -ItemType Directory -Path $dir -Force | Out-Null
        $b = [System.Windows.Forms.Screen]::PrimaryScreen.Bounds
        $x = 0; $y = 0; $w = $b.Width; $h = $b.Height
        if ($Rest.Count -ge 5) { $x = [int]$Rest[1]; $y = [int]$Rest[2]; $w = [int]$Rest[3]; $h = [int]$Rest[4] }
        $bmp = New-Object System.Drawing.Bitmap $w, $h
        $g = [System.Drawing.Graphics]::FromImage($bmp)
        $g.CopyFromScreen($x, $y, 0, 0, $bmp.Size)
        $file = Join-Path $dir "$name.png"
        $bmp.Save($file, [System.Drawing.Imaging.ImageFormat]::Png)
        $g.Dispose(); $bmp.Dispose()
        Say "shot: $file ${w}x${h} at ($x,$y) $(Get-Date -Format 'HH:mm:ss.fff') size=$((Get-Item $file).Length)"
    }
    'b64' {
        # the PNG as base64, 2000 chars per line, so it can be read back through GET /log (no file endpoint on the service)
        $name = if ($Rest.Count -gt 0) { $Rest[0] } else { 'shot' }
        $file = Join-Path (Join-Path $pkg 'ww3') "$name.png"
        if (-not (Test-Path $file)) { Say "b64: no $file"; break }
        $s = [Convert]::ToBase64String([IO.File]::ReadAllBytes($file))
        Say "b64: $name $($s.Length) chars"
        for ($i = 0; $i -lt $s.Length; $i += 2000) { Say ('B64 ' + $s.Substring($i, [math]::Min(2000, $s.Length - $i))) }
        Say "b64: end"
    }
    'engine-log' {
        $n = if ($Rest.Count -gt 0) { [int]$Rest[0] } else { 40 }
        $dir = Join-Path $env:USERPROFILE '.theuth\logs'
        $f = Get-ChildItem -Path $dir -Filter 'theuth.log.*' -ErrorAction SilentlyContinue | Sort-Object LastWriteTime -Descending | Select-Object -First 1
        if (-not $f) { Say "engine-log: nothing under $dir"; break }
        Say "-- $($f.FullName)"
        Select-String -Path $f.FullName -Pattern 'screen_context|screen context|meeting|dictation|companion' | Select-Object -Last $n | ForEach-Object { Say (Clip $_.Line 300) }
    }
    'speech-dump' {
        $p = Join-Path $env:LOCALAPPDATA 'theuth\companion\speech-dump.json'
        if (Test-Path $p) { $txt = Get-Content -Raw $p; for ($i = 0; $i -lt $txt.Length; $i += 1800) { Say ('DUMP ' + $txt.Substring($i, [Math]::Min(1800, $txt.Length - $i))) } } else { Say "no $p" }
    }
    'settings-json' {
        $p = Join-Path $env:LOCALAPPDATA 'theuth\companion\settings.json'
        if (Test-Path $p) { Get-Content $p | ForEach-Object { Say $_ } } else { Say "no $p" }
    }
    default { Get-Content $PSCommandPath | Select-Object -Skip 1 -First 24 | ForEach-Object { Say $_ } }
}