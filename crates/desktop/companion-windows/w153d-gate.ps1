#requires -Version 7.0
<#
.SYNOPSIS
  153 W153D any-app gate helper for build-B (run through the build service's `script` action, inside the interactive session).
  Every output line starts with "GD: ". Launch the companion itself with w6-gate.ps1 `launch --shareable --probe-anyapp-watch`.

.DESCRIPTION
  Actions (first argument):
    notepad              open %TEMP%\w153d-notepad.txt in Notepad (created empty) and bring it to the front; the pid is remembered
    edge-fields          write %TEMP%\w153d-fields.html (a text input, a textarea with a sentence, a password field, a read-only textarea,
                         a button) and open it in a new Edge window titled "W153D fields"
    focus <title-part>   bring the first top-level window whose title contains <title-part> (underscores = spaces) to the front
    probe <k=v> …        write the any-app trigger file (%LOCALAPPDATA%\theuth\companion\anyapp-probe.txt) — the companion started with
                         --probe-anyapp-watch runs it: text=short|long|multi|mail|<words_with_underscores> lease=1 send=1 lock=1 card=1
                         rewrite=<instruction> history=1 delay=<s>
    clip-set <text>      put <text> (underscores = spaces) on the clipboard as plain text
    clip-get             print the clipboard's text
    history [on|off|show] clipboard history (Win+V): HKCU\Software\Microsoft\Clipboard EnableClipboardHistory
    windows              the visible top-level windows (title, class, pid) — what is in front
    close-mine           stop the Notepad / Edge processes this script started (remembered pids only)
#>
param(
    [Parameter(Position = 0)] [string] $Action = 'help',
    [Parameter(ValueFromRemainingArguments = $true)] [string[]] $Rest
)
[Console]::OutputEncoding = [Text.Encoding]::UTF8
$ErrorActionPreference = 'Continue'
if (-not $Rest) { $Rest = @() }
$pidFile = Join-Path $env:TEMP 'w153d-pids.txt'
$trigger = Join-Path $env:LOCALAPPDATA 'theuth\companion\anyapp-probe.txt'
function Say([string] $s) { Write-Output "GD: $s" }
function Words([string] $s) { return ($s -replace '_', ' ') }
function Remember([int] $p) { Add-Content -Path $pidFile -Value $p }

$native = @'
using System;
using System.Collections.Generic;
using System.Runtime.InteropServices;
using System.Text;
public static class GdNative {
    public delegate bool EnumProc(IntPtr h, IntPtr l);
    [DllImport("user32.dll")] public static extern bool EnumWindows(EnumProc cb, IntPtr l);
    [DllImport("user32.dll")] public static extern bool IsWindowVisible(IntPtr h);
    [DllImport("user32.dll", CharSet = CharSet.Unicode)] public static extern int GetWindowTextW(IntPtr h, StringBuilder b, int n);
    [DllImport("user32.dll", CharSet = CharSet.Unicode)] public static extern int GetClassNameW(IntPtr h, StringBuilder b, int n);
    [DllImport("user32.dll")] public static extern uint GetWindowThreadProcessId(IntPtr h, out uint p);
    [DllImport("user32.dll")] public static extern bool SetForegroundWindow(IntPtr h);
    [DllImport("user32.dll")] public static extern bool ShowWindow(IntPtr h, int cmd);
    [DllImport("user32.dll")] public static extern IntPtr GetForegroundWindow();
    [DllImport("user32.dll")] public static extern void keybd_event(byte vk, byte scan, uint flags, UIntPtr extra);
    public static List<IntPtr> Visible() {
        var l = new List<IntPtr>();
        EnumWindows((h, _) => { if (IsWindowVisible(h)) l.Add(h); return true; }, IntPtr.Zero);
        return l;
    }
    public static string Title(IntPtr h) { var b = new StringBuilder(512); GetWindowTextW(h, b, b.Capacity); return b.ToString(); }
    public static string Cls(IntPtr h) { var b = new StringBuilder(256); GetClassNameW(h, b, b.Capacity); return b.ToString(); }
    public static uint Pid(IntPtr h) { uint p; GetWindowThreadProcessId(h, out p); return p; }
    public static bool Front(IntPtr h) {
        ShowWindow(h, 9); // SW_RESTORE
        // Alt down + an unassigned key + Alt up lifts the foreground lock; the masked Alt opens no menu (a bare Alt tap would).
        keybd_event(0x12, 0, 0, UIntPtr.Zero); keybd_event(0xE8, 0, 0, UIntPtr.Zero); keybd_event(0xE8, 0, 2, UIntPtr.Zero); keybd_event(0x12, 0, 2, UIntPtr.Zero);
        bool ok = SetForegroundWindow(h);
        return ok || GetForegroundWindow() == h;
    }
}
'@
Add-Type -TypeDefinition $native -ErrorAction SilentlyContinue

function FindWindow([string] $part) {
    foreach ($w in [GdNative]::Visible()) {
        $t = [GdNative]::Title($w)
        if ($t -and $t.IndexOf($part, [StringComparison]::OrdinalIgnoreCase) -ge 0) { return $w }
    }
    return [IntPtr]::Zero
}
function Front([string] $part) {
    $w = FindWindow $part
    if ($w -eq [IntPtr]::Zero) { Say "focus: no visible window titled *$part*"; return }
    $ok = [GdNative]::Front($w)
    Start-Sleep -Milliseconds 300
    $fg = [GdNative]::GetForegroundWindow()
    Say "focus: '$([GdNative]::Title($w))' hwnd=0x$($w.ToInt64().ToString('X')) pid=$([GdNative]::Pid($w)) -> $ok (foreground now '$([GdNative]::Title($fg))')"
}

switch ($Action) {
    'help' { Get-Content $PSCommandPath -TotalCount 26 | ForEach-Object { Say $_ } }
    'notepad' {
        $f = Join-Path $env:TEMP 'w153d-notepad.txt'
        if (-not (Test-Path $f)) { Set-Content -Path $f -Value '' -NoNewline -Encoding utf8 }
        $before = @(Get-Process -Name 'Notepad' -ErrorAction SilentlyContinue | ForEach-Object { $_.Id })
        Start-Process -FilePath 'notepad.exe' -ArgumentList "`"$f`""
        Start-Sleep -Seconds 3
        Get-Process -Name 'Notepad' -ErrorAction SilentlyContinue | Where-Object { $before -notcontains $_.Id } | ForEach-Object { Remember $_.Id; Say "notepad: started pid=$($_.Id)" }
        Front 'w153d-notepad'
    }
    'edge-fields' {
        $html = @'
<!doctype html><html><head><meta charset="utf-8"><title>W153D fields</title>
<style>body{font:16px "Segoe UI";margin:20px} label{display:block;margin-top:12px} input,textarea{font:16px "Segoe UI";width:520px}</style></head>
<body><h3>W153D any-app fields</h3>
<label>Message <input id="msg" aria-label="Message"></label>
<label>Notes <textarea id="notes" rows="3" aria-label="Notes">Please send me the report by Friday, if you can find the time, thanks a lot.</textarea></label>
<label>Password <input id="pw" type="password" aria-label="Password"></label>
<label>Read-only <textarea id="ro" rows="2" readonly aria-label="Read-only">This text is read-only.</textarea></label>
<p><button id="btn">A button</button></p>
</body></html>
'@
        $f = Join-Path $env:TEMP 'w153d-fields.html'
        Set-Content -Path $f -Value $html -Encoding utf8
        $edge = @("${env:ProgramFiles(x86)}\Microsoft\Edge\Application\msedge.exe", "$env:ProgramFiles\Microsoft\Edge\Application\msedge.exe") | Where-Object { Test-Path $_ } | Select-Object -First 1
        if (-not $edge) { Say 'edge-fields: msedge.exe not found'; break }
        $before = @(Get-Process -Name 'msedge' -ErrorAction SilentlyContinue | ForEach-Object { $_.Id })
        Start-Process -FilePath $edge -ArgumentList '--new-window', '--no-first-run', ("file:///" + ($f -replace '\\', '/'))
        Start-Sleep -Seconds 4
        Get-Process -Name 'msedge' -ErrorAction SilentlyContinue | Where-Object { $before -notcontains $_.Id } | ForEach-Object { Remember $_.Id }
        Say "edge-fields: $f opened in $edge"
        Front 'W153D fields'
    }
    'focus' { Front (Words ($Rest -join ' ')) }
    'esc' {
        # Escape to the foreground window (leaves a menu mode the old bare-Alt focus trick left behind).
        [GdNative]::keybd_event(0x1B, 0, 0, [UIntPtr]::Zero); [GdNative]::keybd_event(0x1B, 0, 2, [UIntPtr]::Zero)
        Say "esc: sent to '$([GdNative]::Title([GdNative]::GetForegroundWindow()))'"
    }
    'probe' {
        $line = ($Rest -join ' ')
        New-Item -ItemType Directory -Force -Path (Split-Path $trigger) | Out-Null
        Set-Content -Path $trigger -Value $line -NoNewline -Encoding utf8
        Say "probe: wrote '$line' to $trigger at $(Get-Date -Format 'HH:mm:ss.fff')"
    }
    'clip-set' {
        $t = Words ($Rest -join ' ')
        Set-Clipboard -Value $t
        Say "clip-set: '$t'"
    }
    'clip-get' { $t = Get-Clipboard -Raw; Say "clip-get: '$t'" }
    'history' {
        $k = 'HKCU:\Software\Microsoft\Clipboard'
        $mode = if ($Rest.Count -gt 0) { $Rest[0] } else { 'show' }
        if ($mode -eq 'on' -or $mode -eq 'off') {
            New-Item -Path $k -Force | Out-Null
            Set-ItemProperty -Path $k -Name 'EnableClipboardHistory' -Value ([int]($mode -eq 'on')) -Type DWord
        }
        $v = (Get-ItemProperty -Path $k -Name 'EnableClipboardHistory' -ErrorAction SilentlyContinue).EnableClipboardHistory
        Say "history: EnableClipboardHistory=$v"
    }
    'windows' {
        $fg = [GdNative]::GetForegroundWindow()
        foreach ($w in [GdNative]::Visible()) {
            $t = [GdNative]::Title($w)
            if (-not $t) { continue }
            $mark = if ($w -eq $fg) { '*' } else { ' ' }
            Say "$mark 0x$($w.ToInt64().ToString('X')) pid=$([GdNative]::Pid($w)) class=$([GdNative]::Cls($w)) '$t'"
        }
    }
    'close-mine' {
        if (-not (Test-Path $pidFile)) { Say 'close-mine: nothing remembered'; break }
        Get-Content $pidFile | ForEach-Object {
            $p = [int]$_
            $proc = Get-Process -Id $p -ErrorAction SilentlyContinue
            if ($proc) { Say "close-mine: stop $($proc.ProcessName) pid=$p"; Stop-Process -Id $p -Force -ErrorAction SilentlyContinue }
        }
        Remove-Item $pidFile -ErrorAction SilentlyContinue
    }
    default { Say "unknown action '$Action' — try help" }
}
