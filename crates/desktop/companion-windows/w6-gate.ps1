#requires -Version 7.0
<#
.SYNOPSIS
  Design 134 W6 polish gate helper for build-B (run through the build service's `script` action, which runs in the
  interactive session, so `launch` starts the Debug companion on the visible desktop). Every output line starts with "G6: ".

.DESCRIPTION
  Actions (first argument):
    shell-log [N]        the shell's companion/alive lines from C:\Users\user\tools\pkg\desktop-stdout.log + desktop-stderr.log
                         (the installed theuth-desktop launched by w5-gate.ps1 `launch`), then the companion log's launch/tray lines
    log [N]              the last N lines (default 60) of today's companion log
    grep <regex> [N]     the last N (default 40) companion-log lines matching the regex
    procs                theuth-desktop / Theuth.Companion / t processes: pid, start time (ms), path
    quit-installed       graceful quit of the INSTALLED companion only: WM_CLOSE to its TheuthCompanionRail host window (the same
                         path as tray › Quit theuth Companion → quit(0)); never touches theuth-desktop or the engine
    start-installed      start the installed companion again (C:\Users\user\AppData\Local\theuth\companion\Theuth.Companion.exe)
    launch [flags…]      start THIS worktree's Debug companion with flags (refuses while another companion runs); prints pid + log
    quit-mine            graceful quit of THIS worktree's companion (WM_CLOSE to its host window)
    kill-mine            stop only companions started from this worktree
    tray-reg             HKCU\Control Panel\NotifyIconSettings entries whose ExecutablePath ends in Theuth.Companion.exe
    promote [exe-substr] IsPromoted=1 for this worktree's exe entry (W2's promote, path-scoped)
    unpromote            IsPromoted=0 for this worktree's exe entry (so the pin tip can be re-tested)
    tip-reset            remove pinTipShownFor from %LOCALAPPDATA%\theuth\companion\settings.json
    settings-json        print settings.json
    fx show              Transparency effects (registry EnableTransparency) and Animation effects (SPI_GETCLIENTAREAANIMATION)
    fx transparency on|off   set EnableTransparency and broadcast WM_SETTINGCHANGE "ImmersiveColorSet" (what Settings does)
    fx animations on|off     SystemParametersInfo(SPI_SETCLIENTAREAANIMATION, …, SPIF_UPDATEINIFILE|SPIF_SENDCHANGE)
    hotkey               send Ctrl+Shift+Space (SendInput) — opens/closes the panel
    shot <name> [x y w h] screenshot (whole screen or a region) to C:\Users\user\tools\pkg\w6\<name>.png and print its size
#>
param(
    [Parameter(Position = 0)] [string] $Action = 'help',
    [Parameter(ValueFromRemainingArguments = $true)] [string[]] $Rest
)
[Console]::OutputEncoding = [Text.Encoding]::UTF8
$ErrorActionPreference = 'Continue'
if (-not $Rest) { $Rest = @() }
$here = $PSScriptRoot
$pkg = 'C:\Users\user\tools\pkg'
$installed = Join-Path $env:LOCALAPPDATA 'theuth\companion\Theuth.Companion.exe'
$mine = Join-Path $here 'src\Theuth.Companion\bin\x64\Debug\net10.0-windows10.0.22621.0\win-x64\Theuth.Companion.exe'
$settingsJson = Join-Path $env:LOCALAPPDATA 'theuth\companion\settings.json'
function Say([string] $s) { Write-Output "G6: $s" }
function Clip([string] $s, [int] $n = 220) { if ($null -eq $s) { return '' }; $s = $s -replace "`e\[[0-9;]*m", ''; return $s.Substring(0, [math]::Min($n, $s.Length)) }
function LogFile { Join-Path $env:LOCALAPPDATA ("theuth\companion\logs\companion-{0:yyyy-MM-dd}.log" -f (Get-Date)) }
function Procs {
    foreach ($n in 'theuth-desktop', 'Theuth.Companion', 't') {
        Get-Process -Name $n -ErrorAction SilentlyContinue | ForEach-Object {
            $st = try { $_.StartTime.ToString('HH:mm:ss.fff') } catch { '?' }
            Say "proc $($_.ProcessName) pid=$($_.Id) start=$st path=$($_.Path)"
        }
    }
}

$native = @'
using System;
using System.Runtime.InteropServices;
using System.Text;
public static class W6Native {
    [DllImport("user32.dll", SetLastError = true)] public static extern IntPtr FindWindowW([MarshalAs(UnmanagedType.LPWStr)] string cls, [MarshalAs(UnmanagedType.LPWStr)] string title);
    [DllImport("user32.dll", SetLastError = true)] public static extern IntPtr FindWindowExW(IntPtr parent, IntPtr after, [MarshalAs(UnmanagedType.LPWStr)] string cls, [MarshalAs(UnmanagedType.LPWStr)] string title);
    [DllImport("user32.dll")] public static extern uint GetWindowThreadProcessId(IntPtr hwnd, out uint pid);
    [DllImport("user32.dll")] public static extern bool PostMessageW(IntPtr hwnd, uint msg, IntPtr w, IntPtr l);
    [DllImport("user32.dll", SetLastError = true)] public static extern IntPtr SendMessageTimeoutW(IntPtr hwnd, uint msg, IntPtr w, [MarshalAs(UnmanagedType.LPWStr)] string l, uint flags, uint timeout, out IntPtr result);
    [DllImport("user32.dll", SetLastError = true)] public static extern bool SystemParametersInfoW(uint action, uint param, ref int pv, uint winini);
    [StructLayout(LayoutKind.Sequential)] public struct KEYBDINPUT { public ushort wVk; public ushort wScan; public uint dwFlags; public uint time; public IntPtr dwExtraInfo; }
    [StructLayout(LayoutKind.Explicit)] public struct INPUTUNION { [FieldOffset(0)] public KEYBDINPUT ki; [FieldOffset(0)] public long pad1; [FieldOffset(8)] public long pad2; [FieldOffset(16)] public long pad3; }
    [StructLayout(LayoutKind.Sequential)] public struct INPUT { public uint type; public INPUTUNION u; }
    [DllImport("user32.dll", SetLastError = true)] public static extern uint SendInput(uint n, INPUT[] inputs, int size);
    public static uint Key(ushort vk, bool up) {
        var i = new INPUT(); i.type = 1; i.u.ki.wVk = vk; i.u.ki.dwFlags = up ? 2u : 0u;
        return SendInput(1, new[] { i }, Marshal.SizeOf<INPUT>());
    }
}
'@
function Native { if (-not ('W6Native' -as [type])) { Add-Type -TypeDefinition $native -Language CSharp } }

function HostWindowOf([string] $exePath) {
    Native
    $h = [IntPtr]::Zero
    while ($true) {
        $h = [W6Native]::FindWindowExW([IntPtr]::Zero, $h, 'TheuthCompanionRail', $null)
        if ($h -eq [IntPtr]::Zero) { return $null }
        $pid = 0
        [void][W6Native]::GetWindowThreadProcessId($h, [ref]$pid)
        $p = Get-Process -Id $pid -ErrorAction SilentlyContinue
        if ($p -and $p.Path -ieq $exePath) { return @{ Hwnd = $h; Pid = $pid } }
    }
}

switch ($Action) {
    'shell-log' {
        $n = if ($Rest.Count -gt 0) { [int]$Rest[0] } else { 60 }
        foreach ($f in 'desktop-stdout.log', 'desktop-stderr.log') {
            $p = Join-Path $pkg $f
            if (-not (Test-Path $p)) { Say "no $p"; continue }
            $fi = Get-Item $p
            Say "-- $f size=$($fi.Length) written=$($fi.LastWriteTime.ToString('HH:mm:ss'))"
            Select-String -Path $p -Pattern 'companion' | Select-Object -Last $n | ForEach-Object { Say ('  ' + (Clip $_.Line 260)) }
        }
        $cl = LogFile
        if (Test-Path $cl) {
            Say "-- companion log launch/tray/host/terminate/single lines"
            Select-String -Path $cl -Pattern 'launch exe=|host window|NIM_ADD|terminate|single instance|surfaces up|another companion' | Select-Object -Last $n | ForEach-Object { Say ('  ' + (Clip $_.Line 200)) }
        } else { Say "no $cl" }
    }
    'log' {
        $n = if ($Rest.Count -gt 0) { [int]$Rest[0] } else { 60 }
        $cl = LogFile
        if (Test-Path $cl) { Get-Content $cl -Tail $n | ForEach-Object { Say (Clip $_ 300) } } else { Say "no $cl" }
    }
    'grep' {
        $pat = if ($Rest.Count -gt 0) { $Rest[0] } else { '.' }
        $n = if ($Rest.Count -gt 1) { [int]$Rest[1] } else { 40 }
        $cl = LogFile
        if (Test-Path $cl) { Select-String -Path $cl -Pattern $pat | Select-Object -Last $n | ForEach-Object { Say (Clip $_.Line 300) } } else { Say "no $cl" }
    }
    'procs' { Procs }
    'quit-installed' {
        $w = HostWindowOf $installed
        if (-not $w) { Say "quit-installed: no TheuthCompanionRail window owned by $installed"; Procs; break }
        Say "quit-installed: WM_CLOSE -> hwnd=0x$($w.Hwnd.ToInt64().ToString('X')) pid=$($w.Pid)"
        [void][W6Native]::PostMessageW($w.Hwnd, 0x10, [IntPtr]::Zero, [IntPtr]::Zero)
        $t0 = Get-Date
        while (((Get-Date) - $t0).TotalSeconds -lt 8 -and (Get-Process -Id $w.Pid -ErrorAction SilentlyContinue)) { Start-Sleep -Milliseconds 200 }
        Say "quit-installed: pid $($w.Pid) gone=$(-not (Get-Process -Id $w.Pid -ErrorAction SilentlyContinue)) after $([int]((Get-Date) - $t0).TotalMilliseconds) ms"
        Procs
    }
    'start-installed' {
        if (-not (Test-Path $installed)) { Say "start-installed: $installed missing"; break }
        $p = Start-Process -FilePath $installed -PassThru
        Say "start-installed: pid=$($p.Id) at $(Get-Date -Format 'HH:mm:ss.fff')"
        Start-Sleep -Seconds 5
        Procs
    }
    'launch' {
        if (-not (Test-Path $mine)) { Say "launch: no exe at $mine"; break }
        $others = @(Get-Process -Name 'Theuth.Companion' -ErrorAction SilentlyContinue)
        if ($others.Count -gt 0) { $others | ForEach-Object { Say "launch: REFUSED — a companion runs: pid=$($_.Id) path=$($_.Path)" }; break }
        $flags = @($Rest)
        $p = if ($flags.Count -gt 0) { Start-Process -FilePath $mine -ArgumentList $flags -PassThru } else { Start-Process -FilePath $mine -PassThru }
        Say "launch: $mine $($flags -join ' ') pid=$($p.Id) at $(Get-Date -Format 'HH:mm:ss.fff')"
        Start-Sleep -Seconds 6
        Say "launch: running=$(-not $p.HasExited)"
        $cl = LogFile
        if (Test-Path $cl) { Get-Content $cl -Tail 30 | ForEach-Object { Say ('  ' + (Clip $_ 220)) } }
    }
    'quit-mine' {
        $w = HostWindowOf $mine
        if (-not $w) { Say "quit-mine: no TheuthCompanionRail window owned by $mine"; Procs; break }
        Say "quit-mine: WM_CLOSE -> hwnd=0x$($w.Hwnd.ToInt64().ToString('X')) pid=$($w.Pid)"
        [void][W6Native]::PostMessageW($w.Hwnd, 0x10, [IntPtr]::Zero, [IntPtr]::Zero)
        $t0 = Get-Date
        while (((Get-Date) - $t0).TotalSeconds -lt 8 -and (Get-Process -Id $w.Pid -ErrorAction SilentlyContinue)) { Start-Sleep -Milliseconds 200 }
        Say "quit-mine: pid $($w.Pid) gone=$(-not (Get-Process -Id $w.Pid -ErrorAction SilentlyContinue)) after $([int]((Get-Date) - $t0).TotalMilliseconds) ms"
    }
    'kill-mine' {
        $ps = @(Get-Process -Name 'Theuth.Companion' -ErrorAction SilentlyContinue | Where-Object { $_.Path -and $_.Path.StartsWith($here, [StringComparison]::OrdinalIgnoreCase) })
        $ps | ForEach-Object { Say "kill-mine: pid=$($_.Id) path=$($_.Path)"; Stop-Process -Id $_.Id -Force }
        Say "kill-mine: stopped $($ps.Count)"
    }
    'tray-reg' {
        $root = 'HKCU:\Control Panel\NotifyIconSettings'
        $n = 0
        Get-ChildItem $root -ErrorAction SilentlyContinue | ForEach-Object {
            $v = Get-ItemProperty $_.PSPath
            if ($v.ExecutablePath -like '*Theuth.Companion.exe') {
                $n++
                $promoted = if ($null -eq $v.IsPromoted) { '<absent>' } else { $v.IsPromoted }
                Say "tray-reg $($_.PSChildName) IsPromoted=$promoted exe=$($v.ExecutablePath) tip='$($v.InitialTooltip)'"
            }
        }
        Say "tray-reg: $n Theuth.Companion.exe entries (root exists=$(Test-Path $root))"
    }
    'promote' {
        $root = 'HKCU:\Control Panel\NotifyIconSettings'
        $needle = if ($Rest.Count -gt 0) { $Rest[0] } else { $mine }
        $hit = 0
        Get-ChildItem $root -ErrorAction SilentlyContinue | ForEach-Object {
            $v = Get-ItemProperty $_.PSPath
            if ($v.ExecutablePath -and $v.ExecutablePath -ieq $needle) { Set-ItemProperty -Path $_.PSPath -Name IsPromoted -Value 1 -Type DWord; $hit++; Say "promoted $($_.PSChildName) $($v.ExecutablePath)" }
        }
        if ($hit -eq 0) { Say "promote: no entry for $needle yet" }
    }
    'unpromote' {
        $root = 'HKCU:\Control Panel\NotifyIconSettings'
        $hit = 0
        Get-ChildItem $root -ErrorAction SilentlyContinue | ForEach-Object {
            $v = Get-ItemProperty $_.PSPath
            if ($v.ExecutablePath -and $v.ExecutablePath -ieq $mine) { Set-ItemProperty -Path $_.PSPath -Name IsPromoted -Value 0 -Type DWord; $hit++; Say "unpromoted $($_.PSChildName) $($v.ExecutablePath)" }
        }
        if ($hit -eq 0) { Say "unpromote: no entry for $mine yet" }
    }
    'tip-reset' {
        if (-not (Test-Path $settingsJson)) { Say "tip-reset: no $settingsJson"; break }
        $o = Get-Content $settingsJson -Raw | ConvertFrom-Json -AsHashtable
        $had = $o.ContainsKey('pinTipShownFor')
        $o.Remove('pinTipShownFor')
        ($o | ConvertTo-Json -Depth 8) | Set-Content $settingsJson -Encoding UTF8
        Say "tip-reset: removed=$had"
    }
    'settings-json' { if (Test-Path $settingsJson) { Get-Content $settingsJson | ForEach-Object { Say (Clip $_ 300) } } else { Say "no $settingsJson" } }
    'fx' {
        Native
        $what = if ($Rest.Count -gt 0) { $Rest[0] } else { 'show' }
        $val = if ($Rest.Count -gt 1) { $Rest[1] } else { '' }
        $key = 'HKCU:\Software\Microsoft\Windows\CurrentVersion\Themes\Personalize'
        if ($what -eq 'transparency' -and $val -in 'on', 'off') {
            Set-ItemProperty -Path $key -Name EnableTransparency -Value $(if ($val -eq 'on') { 1 } else { 0 }) -Type DWord
            $res = [IntPtr]::Zero
            [void][W6Native]::SendMessageTimeoutW([IntPtr]0xFFFF, 0x1A, [IntPtr]::Zero, 'ImmersiveColorSet', 2, 2000, [ref]$res)
            Say "fx: EnableTransparency set $val + WM_SETTINGCHANGE ImmersiveColorSet broadcast"
        }
        if ($what -eq 'animations' -and $val -in 'on', 'off') {
            $v = if ($val -eq 'on') { 1 } else { 0 }
            # SPI_SETCLIENTAREAANIMATION = 0x1043; pvParam is the BOOL by value for SET; SPIF_UPDATEINIFILE|SPIF_SENDCHANGE = 3
            $ok = [W6Native]::SystemParametersInfoW(0x1043, 0, [ref]$v, 3)
            Say "fx: SPI_SETCLIENTAREAANIMATION $val -> $ok"
        }
        $t = (Get-ItemProperty $key -ErrorAction SilentlyContinue).EnableTransparency
        $a = 0
        $ok2 = [W6Native]::SystemParametersInfoW(0x1042, 0, [ref]$a, 0)
        Say "fx: EnableTransparency=$(if ($null -eq $t) { '<absent>' } else { $t }) ClientAreaAnimation=$a (read ok=$ok2) at $(Get-Date -Format 'HH:mm:ss.fff')"
    }
    'hotkey' {
        Native
        [void][W6Native]::Key(0x11, $false); [void][W6Native]::Key(0x10, $false); [void][W6Native]::Key(0x20, $false)
        Start-Sleep -Milliseconds 60
        [void][W6Native]::Key(0x20, $true); [void][W6Native]::Key(0x10, $true); [void][W6Native]::Key(0x11, $true)
        Say "hotkey: Ctrl+Shift+Space sent at $(Get-Date -Format 'HH:mm:ss.fff')"
    }
    'shot' {
        Add-Type -AssemblyName System.Drawing
        $name = if ($Rest.Count -gt 0) { $Rest[0] } else { 'shot' }
        $dir = Join-Path $pkg 'w6'
        New-Item -ItemType Directory -Path $dir -Force | Out-Null
        $b = [System.Windows.Forms.Screen]::PrimaryScreen.Bounds 2>$null
        if (-not $b) { Add-Type -AssemblyName System.Windows.Forms; $b = [System.Windows.Forms.Screen]::PrimaryScreen.Bounds }
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
    default { Get-Content $PSCommandPath | Select-Object -Skip 1 -First 30 | ForEach-Object { Say $_ } }
}
