using System.Runtime.InteropServices;

namespace Theuth.Companion.Breakpoints;

/// <summary>
/// The Win32 surface W3 calls, declared here rather than in W2's CsWin32 <c>Interop/NativeMethods.txt</c> so the two
/// waves never edit one file (design 134 §5.2 names CsWin32; the integrator may fold these names into that list).
/// Blittable signatures only; every call is made on the UI thread unless noted.
/// </summary>
internal static class W3Native
{
    // WinEvent hook (BreakpointSource): the foreground window changed, delivered on the hooking thread's message loop.
    public const uint EVENT_SYSTEM_FOREGROUND = 0x0003;
    public const uint WINEVENT_OUTOFCONTEXT = 0x0000;

    public delegate void WinEventProc(nint hWinEventHook, uint @event, nint hwnd, int idObject, int idChild, uint idEventThread, uint dwmsEventTime);

    [DllImport("user32.dll", SetLastError = true)]
    public static extern nint SetWinEventHook(uint eventMin, uint eventMax, nint hmodWinEventProc, WinEventProc lpfnWinEventProc, uint idProcess, uint idThread, uint dwFlags);

    [DllImport("user32.dll")]
    [return: MarshalAs(UnmanagedType.Bool)]
    public static extern bool UnhookWinEvent(nint hWinEventHook);

    [DllImport("user32.dll", SetLastError = true)]
    public static extern uint GetWindowThreadProcessId(nint hWnd, out uint lpdwProcessId);

    // The foreground process's exe path (the Mac's bundle id).
    public const uint PROCESS_QUERY_LIMITED_INFORMATION = 0x1000;

    [DllImport("kernel32.dll", SetLastError = true)]
    public static extern nint OpenProcess(uint dwDesiredAccess, [MarshalAs(UnmanagedType.Bool)] bool bInheritHandle, uint dwProcessId);

    [DllImport("kernel32.dll", SetLastError = true, CharSet = CharSet.Unicode)]
    [return: MarshalAs(UnmanagedType.Bool)]
    public static extern bool QueryFullProcessImageNameW(nint hProcess, uint dwFlags, [Out] char[] lpExeName, ref uint lpdwSize);

    [DllImport("kernel32.dll", SetLastError = true)]
    [return: MarshalAs(UnmanagedType.Bool)]
    public static extern bool CloseHandle(nint hObject);

    // Idle sampling (GetLastInputInfo), 1 Hz.
    [StructLayout(LayoutKind.Sequential)]
    public struct LASTINPUTINFO
    {
        public uint cbSize;
        public uint dwTime;
    }

    [DllImport("user32.dll")]
    [return: MarshalAs(UnmanagedType.Bool)]
    public static extern bool GetLastInputInfo(ref LASTINPUTINFO plii);

    // Low-level keyboard hook (DoubleTapSource): key codes only, while the companion runs (design 134 §4.6, D6).
    public const int WH_KEYBOARD_LL = 13;
    public const uint WM_KEYDOWN = 0x0100, WM_KEYUP = 0x0101, WM_SYSKEYDOWN = 0x0104, WM_SYSKEYUP = 0x0105;

    [StructLayout(LayoutKind.Sequential)]
    public struct KBDLLHOOKSTRUCT
    {
        public uint vkCode;
        public uint scanCode;
        public uint flags;
        public uint time;
        public nuint dwExtraInfo;
    }

    public delegate nint LowLevelKeyboardProc(int nCode, nuint wParam, nint lParam);

    [DllImport("user32.dll", SetLastError = true)]
    public static extern nint SetWindowsHookExW(int idHook, LowLevelKeyboardProc lpfn, nint hMod, uint dwThreadId);

    [DllImport("user32.dll", SetLastError = true)]
    [return: MarshalAs(UnmanagedType.Bool)]
    public static extern bool UnhookWindowsHookEx(nint hhk);

    [DllImport("user32.dll")]
    public static extern nint CallNextHookEx(nint hhk, int nCode, nuint wParam, nint lParam);

    [DllImport("kernel32.dll", CharSet = CharSet.Unicode, SetLastError = true)]
    public static extern nint GetModuleHandleW(string? lpModuleName);

    // The recorder's "held by another app" probe (Settings › Keys): an exclusive RegisterHotKey on this thread.
    public const uint MOD_NOREPEAT = 0x4000;
    public const int ERROR_HOTKEY_ALREADY_REGISTERED = 1409;

    [DllImport("user32.dll", SetLastError = true)]
    [return: MarshalAs(UnmanagedType.Bool)]
    public static extern bool RegisterHotKey(nint hWnd, int id, uint fsModifiers, uint vk);

    [DllImport("user32.dll", SetLastError = true)]
    [return: MarshalAs(UnmanagedType.Bool)]
    public static extern bool UnregisterHotKey(nint hWnd, int id);

    // The recorder's key text: the key's character with no modifiers (W1's KeyPress rule), without touching dead-key state.
    [DllImport("user32.dll", CharSet = CharSet.Unicode)]
    public static extern int ToUnicodeEx(uint wVirtKey, uint wScanCode, byte[] lpKeyState, [Out] char[] pwszBuff, int cchBuff, uint wFlags, nint dwhkl);

    [DllImport("user32.dll")]
    public static extern nint GetKeyboardLayout(uint idThread);

    [DllImport("user32.dll")]
    public static extern uint MapVirtualKeyW(uint uCode, uint uMapType);

    // Settings window placement.
    [DllImport("user32.dll")]
    public static extern uint GetDpiForWindow(nint hwnd);

    // Settings › Rail: the display device names (`\\.\DISPLAY1`) behind the DisplayArea ids.
    [StructLayout(LayoutKind.Sequential, CharSet = CharSet.Unicode)]
    public struct MONITORINFOEXW
    {
        public uint cbSize;
        public RECT rcMonitor;
        public RECT rcWork;
        public uint dwFlags;
        [MarshalAs(UnmanagedType.ByValTStr, SizeConst = 32)]
        public string szDevice;
    }

    [StructLayout(LayoutKind.Sequential)]
    public struct RECT
    {
        public int left, top, right, bottom;
    }

    [DllImport("user32.dll", CharSet = CharSet.Unicode)]
    [return: MarshalAs(UnmanagedType.Bool)]
    public static extern bool GetMonitorInfoW(nint hMonitor, ref MONITORINFOEXW lpmi);

    /// <summary>The foreground process's full exe path (lower-cased), or <c>pid:&lt;n&gt;</c> when it cannot be read (elevated, protected).</summary>
    public static string ProcessIdentity(nint hwnd)
    {
        GetWindowThreadProcessId(hwnd, out uint pid);
        if (pid == 0) return "";
        nint h = OpenProcess(PROCESS_QUERY_LIMITED_INFORMATION, false, pid);
        if (h == 0) return $"pid:{pid}";
        try
        {
            var buf = new char[1024];
            uint n = (uint)buf.Length;
            return QueryFullProcessImageNameW(h, 0, buf, ref n) && n > 0 ? new string(buf, 0, (int)n).ToLowerInvariant() : $"pid:{pid}";
        }
        finally
        {
            CloseHandle(h);
        }
    }

    /// <summary>Seconds since the last keyboard or mouse input in this session (the Mac's <c>HIDIdleTime</c>).</summary>
    public static double IdleSeconds()
    {
        var info = new LASTINPUTINFO { cbSize = (uint)Marshal.SizeOf<LASTINPUTINFO>() };
        if (!GetLastInputInfo(ref info)) return 0;
        uint elapsed = unchecked((uint)Environment.TickCount - info.dwTime); // uint arithmetic survives the 49-day wrap
        return elapsed / 1000.0;
    }
}
