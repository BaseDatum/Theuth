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
    move <x> <y>         move the mouse to screen (x, y)
    draw                 move the mouse along a small loop (≈ 1 s) — the trail
    shot <name> [x y w h] screenshot (whole screen or a region) to C:\Users\user\tools\pkg\ww3\<name>.png
    b64 <name>           that PNG as base64 lines (read back through GET /log)
    engine-log [N]       the last N lines (default 40) of the installed engine's log mentioning screen_context / meeting / dictation
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
    'tap-voice' {
        Say "tap-voice: tap, tap (120 ms), then a tap after 1.6 s at $(Get-Date -Format 'HH:mm:ss.fff')"
        Chord $true; Start-Sleep -Milliseconds 60; Chord $false
        Start-Sleep -Milliseconds 120
        Chord $true; Start-Sleep -Milliseconds 60; Chord $false
        Start-Sleep -Milliseconds 1600
        Chord $true; Start-Sleep -Milliseconds 60; Chord $false
        Say "tap-voice: done at $(Get-Date -Format 'HH:mm:ss.fff')"
    }
    'esc' { [void][G3Native]::Key(0x1B, $false); Start-Sleep -Milliseconds 40; [void][G3Native]::Key(0x1B, $true); Say 'esc: sent' }
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
    'settings-json' {
        $p = Join-Path $env:LOCALAPPDATA 'theuth\companion\settings.json'
        if (Test-Path $p) { Get-Content $p | ForEach-Object { Say $_ } } else { Say "no $p" }
    }
    default { Get-Content $PSCommandPath | Select-Object -Skip 1 -First 24 | ForEach-Object { Say $_ } }
}
