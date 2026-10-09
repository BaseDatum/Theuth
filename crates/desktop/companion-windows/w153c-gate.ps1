#requires -Version 7.0
<#
.SYNOPSIS
  Design 153 W153C gate helper for build-B (run through the build service's `script` action in the interactive session).
  Every output line starts with "G153C: ".

.DESCRIPTION
  Actions (first argument):
    pngs [prefix]          list %TEMP%\<prefix>*.png (default 153-) — the 153 probes' PNGs (name, bytes, time)
    b64 <file>             print a PNG as base64 lines between B64-BEGIN/B64-END; <file> is a name in %TEMP% or a full path with
                           forward slashes (C:/Users/user/tools/pkg/w6/x.png)
    langs show             the user language list (tag + input method tips) and the default input method override
    langs intl             save the original list once (%TEMP%\w153c-langs-orig.json), then make en-US use ONLY the
                           US-International keyboard (0409:00020409) and make it the default input method — proof 22's layout
    langs restore          put the saved language list and default input method back (and delete the save file)
    layout-probe           the foreground thread's keyboard layout and what Ctrl+Alt+D / Ctrl+Alt+C / Ctrl+Alt+Space type on it
                           (ToUnicodeEx, keystate Ctrl+Alt) — the same probe the companion runs
    keys switch            SendInput: hold Ctrl+Alt+Space, after 900 ms press C (Ctrl+Alt+C) — the mode switch while holding —
                           release C after 900 ms (the release of either key ends the hold), then Space and the modifiers
    keys tap-command       SendInput: Ctrl+Alt+C tapped (down/up 80 ms)
#>
param(
    [Parameter(Position = 0)] [string] $Action = 'help',
    [Parameter(ValueFromRemainingArguments = $true)] [string[]] $Rest
)
[Console]::OutputEncoding = [Text.Encoding]::UTF8
$ErrorActionPreference = 'Continue'
if (-not $Rest) { $Rest = @() }
function Say([string] $s) { Write-Output "G153C: $s" }
$save = Join-Path $env:TEMP 'w153c-langs-orig.json'

$native = @'
using System;
using System.Runtime.InteropServices;
using System.Text;
public static class G153Native {
    [StructLayout(LayoutKind.Sequential)] public struct KEYBDINPUT { public ushort wVk; public ushort wScan; public uint dwFlags; public uint time; public IntPtr dwExtraInfo; }
    [StructLayout(LayoutKind.Explicit)] public struct INPUTUNION { [FieldOffset(0)] public KEYBDINPUT ki; [FieldOffset(0)] public long pad1; [FieldOffset(8)] public long pad2; [FieldOffset(16)] public long pad3; }
    [StructLayout(LayoutKind.Sequential)] public struct INPUT { public uint type; public INPUTUNION u; }
    [DllImport("user32.dll", SetLastError = true)] public static extern uint SendInput(uint n, INPUT[] inputs, int size);
    [DllImport("user32.dll")] public static extern IntPtr GetForegroundWindow();
    [DllImport("user32.dll")] public static extern uint GetWindowThreadProcessId(IntPtr hwnd, out uint pid);
    [DllImport("user32.dll")] public static extern IntPtr GetKeyboardLayout(uint thread);
    [DllImport("user32.dll")] public static extern uint MapVirtualKeyW(uint code, uint type);
    [DllImport("user32.dll", CharSet = CharSet.Unicode)] public static extern int ToUnicodeEx(uint vk, uint scan, byte[] state, [Out] char[] buf, int n, uint flags, IntPtr hkl);
    public static uint Key(ushort vk, bool up) {
        var i = new INPUT(); i.type = 1; i.u.ki.wVk = vk; i.u.ki.dwFlags = up ? 2u : 0u;
        return SendInput(1, new[] { i }, Marshal.SizeOf<INPUT>());
    }
    public static IntPtr ForegroundLayout() {
        var fg = GetForegroundWindow(); uint pid;
        uint tid = fg == IntPtr.Zero ? 0 : GetWindowThreadProcessId(fg, out pid);
        return GetKeyboardLayout(tid);
    }
    public static string CtrlAlt(ushort vk, IntPtr hkl) {
        var st = new byte[256]; st[0x11] = 0x80; st[0x12] = 0x80; st[0xA2] = 0x80; st[0xA5] = 0x80;
        var buf = new char[8];
        int n = ToUnicodeEx(vk, MapVirtualKeyW(vk, 0), st, buf, buf.Length, 4, hkl);
        if (n == 0) return "(none)";
        var s = new string(buf, 0, Math.Min(Math.Abs(n), buf.Length));
        var sb = new StringBuilder();
        foreach (var c in s) sb.Append(char.IsControl(c) ? string.Format("U+{0:X4}", (int)c) : c.ToString());
        return (n < 0 ? "dead " : "") + "'" + sb + "'";
    }
}
'@
function Native { if (-not ('G153Native' -as [type])) { Add-Type -TypeDefinition $native -Language CSharp } }

function ShowLangs {
    foreach ($l in Get-WinUserLanguageList) { Say "language $($l.LanguageTag) tips=$($l.InputMethodTips -join ',')" }
    $o = Get-WinDefaultInputMethodOverride
    Say "default input method override: $(if ($o) { $o.InputMethodTip } else { '(none)' })"
}

switch ($Action) {
    'pngs' {
        $prefix = if ($Rest.Count -ge 1) { $Rest[0] } else { '153-' }
        Get-ChildItem -Path $env:TEMP -Filter "$prefix*.png" -ErrorAction SilentlyContinue | Sort-Object LastWriteTime | ForEach-Object {
            Say ("png {0} {1} bytes {2:HH:mm:ss}" -f $_.Name, $_.Length, $_.LastWriteTime)
        }
    }
    'b64' {
        if ($Rest.Count -lt 1) { Say 'usage: b64 <file>'; break }
        $p = $Rest[0]
        if ($p -notmatch '[:/]') { $p = Join-Path $env:TEMP $p }
        $p = $p -replace '/', '\'
        if (-not (Test-Path $p)) { Say "missing $p"; break }
        $b = [Convert]::ToBase64String([IO.File]::ReadAllBytes($p))
        Say "B64-BEGIN $p $($b.Length)"
        for ($i = 0; $i -lt $b.Length; $i += 3000) { Write-Output ("B64:" + $b.Substring($i, [Math]::Min(3000, $b.Length - $i))) }
        Say "B64-END"
    }
    'langs' {
        $sub = if ($Rest.Count -ge 1) { $Rest[0] } else { 'show' }
        switch ($sub) {
            'show' { ShowLangs }
            'intl' {
                if (-not (Test-Path $save)) {
                    $orig = Get-WinUserLanguageList | ForEach-Object { [pscustomobject]@{ Tag = $_.LanguageTag; Tips = @($_.InputMethodTips) } }
                    $ov = Get-WinDefaultInputMethodOverride
                    [pscustomobject]@{ List = $orig; Override = $(if ($ov) { $ov.InputMethodTip } else { $null }) } | ConvertTo-Json -Depth 5 | Set-Content -Path $save -Encoding UTF8
                    Say "saved the original list to $save"
                }
                Say 'before:'; ShowLangs
                $list = Get-WinUserLanguageList
                $us = $list | Where-Object { $_.LanguageTag -eq 'en-US' } | Select-Object -First 1
                if (-not $us) { $list.Add('en-US'); $us = $list | Where-Object { $_.LanguageTag -eq 'en-US' } | Select-Object -First 1 }
                $us.InputMethodTips.Clear()
                $us.InputMethodTips.Add('0409:00020409')
                Set-WinUserLanguageList -LanguageList $list -Force
                Set-WinDefaultInputMethodOverride -InputTip '0409:00020409'
                Start-Sleep -Milliseconds 800
                Say 'after:'; ShowLangs
            }
            'restore' {
                if (-not (Test-Path $save)) { Say "nothing saved ($save)"; ShowLangs; break }
                $s = Get-Content -Raw -Path $save | ConvertFrom-Json
                $list = New-WinUserLanguageList -Language $s.List[0].Tag
                $list[0].InputMethodTips.Clear()
                foreach ($t in $s.List[0].Tips) { $list[0].InputMethodTips.Add($t) }
                for ($i = 1; $i -lt $s.List.Count; $i++) {
                    $list.Add($s.List[$i].Tag)
                    $list[$i].InputMethodTips.Clear()
                    foreach ($t in $s.List[$i].Tips) { $list[$i].InputMethodTips.Add($t) }
                }
                Set-WinUserLanguageList -LanguageList $list -Force
                if ($s.Override) { Set-WinDefaultInputMethodOverride -InputTip $s.Override } else { Set-WinDefaultInputMethodOverride -InputTip '0409:00000409'; Set-WinDefaultInputMethodOverride }
                Remove-Item -Path $save -Force
                Start-Sleep -Milliseconds 800
                Say 'restored:'; ShowLangs
            }
            default { Say "unknown langs action $sub" }
        }
    }
    'layout-probe' {
        Native
        $hkl = [G153Native]::ForegroundLayout()
        Say ("foreground layout 0x{0:X8}" -f [long]$hkl)
        foreach ($k in @(@{n='D';v=0x44}, @{n='C';v=0x43}, @{n='Space';v=0x20})) {
            Say ("Ctrl+Alt+{0} types {1}" -f $k.n, [G153Native]::CtrlAlt([uint16]$k.v, $hkl))
        }
    }
    'keys' {
        Native
        $sub = if ($Rest.Count -ge 1) { $Rest[0] } else { 'switch' }
        switch ($sub) {
            'switch' {
                [void][G153Native]::Key(0x11, $false); [void][G153Native]::Key(0x12, $false); [void][G153Native]::Key(0x20, $false)
                Say 'Ctrl+Alt+Space down (voice key held)'
                Start-Sleep -Milliseconds 900
                [void][G153Native]::Key(0x43, $false)
                Say 'C down (Ctrl+Alt+C while the voice key is held — switch to command)'
                Start-Sleep -Milliseconds 900
                [void][G153Native]::Key(0x43, $true)
                Say 'C up (the release of either key ends the hold)'
                Start-Sleep -Milliseconds 300
                [void][G153Native]::Key(0x20, $true); [void][G153Native]::Key(0x12, $true); [void][G153Native]::Key(0x11, $true)
                Say 'Space, Alt, Ctrl up'
            }
            'tap-command' {
                [void][G153Native]::Key(0x11, $false); [void][G153Native]::Key(0x12, $false); [void][G153Native]::Key(0x43, $false)
                Start-Sleep -Milliseconds 80
                [void][G153Native]::Key(0x43, $true); [void][G153Native]::Key(0x12, $true); [void][G153Native]::Key(0x11, $true)
                Say 'Ctrl+Alt+C tapped'
            }
            default { Say "unknown keys action $sub" }
        }
    }
    default { Say 'actions: pngs [prefix] · b64 <file> · langs show|intl|restore · layout-probe · keys switch|tap-command' }
}
