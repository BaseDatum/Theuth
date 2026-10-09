using System.Runtime.InteropServices;

namespace Theuth.Companion.AnyApp;

/// <summary>
/// 153 W153D: the Win32 surface any-app dictation needs — SendInput, the clipboard, process identity, the WTS session notifications
/// and a hidden window class — declared here (not in the shared CsWin32 list) so sibling waves never merge over it (the 149 WW2 rule).
/// </summary>
internal static partial class AnyAppNative
{
    // MARK: input

    public const uint InputKeyboard = 1;
    public const uint KeyEventExtendedKey = 0x0001, KeyEventKeyUp = 0x0002, KeyEventUnicode = 0x0004;

    /// <summary>Tags every event we inject, so a low-level hook (ours or a sibling's) can tell them from the user's: "theu".</summary>
    public const nint InjectedTag = 0x74686575;

    public const ushort VkBack = 0x08, VkReturn = 0x0D, VkShift = 0x10, VkControl = 0x11, VkMenu = 0x12, VkLWin = 0x5B, VkRWin = 0x5C,
        VkLShift = 0xA0, VkRShift = 0xA1, VkLControl = 0xA2, VkRControl = 0xA3, VkLMenu = 0xA4, VkRMenu = 0xA5, VkV = 0x56, VkZ = 0x5A,
        /// <summary>An unassigned key (AutoHotkey's menu mask): sent between a held Alt/Win and its release so the release opens no menu.</summary>
        VkMask = 0xE8;

    [StructLayout(LayoutKind.Sequential)]
    public struct KeyboardInput
    {
        public ushort Vk;
        public ushort Scan;
        public uint Flags;
        public uint Time;
        public nint ExtraInfo;
    }

    /// <summary>The INPUT union is sized for MOUSEINPUT (32 bytes on x64); only the keyboard member is used.</summary>
    [StructLayout(LayoutKind.Explicit, Size = 32)]
    public struct InputUnion
    {
        [FieldOffset(0)] public KeyboardInput Ki;
    }

    [StructLayout(LayoutKind.Sequential)]
    public struct Input
    {
        public uint Type;
        public InputUnion U;
    }

    [DllImport("user32.dll", SetLastError = true)]
    public static extern uint SendInput(uint count, [In] Input[] inputs, int size);

    [DllImport("user32.dll")]
    public static extern short GetAsyncKeyState(int vk);

    public static bool IsDown(int vk) => (GetAsyncKeyState(vk) & 0x8000) != 0;

    // MARK: windows and processes

    [DllImport("user32.dll")]
    public static extern nint GetForegroundWindow();

    [DllImport("user32.dll", SetLastError = true)]
    [return: MarshalAs(UnmanagedType.Bool)]
    public static extern bool SetForegroundWindow(nint hwnd);

    [DllImport("user32.dll")]
    public static extern uint GetWindowThreadProcessId(nint hwnd, out uint pid);

    [DllImport("user32.dll")]
    [return: MarshalAs(UnmanagedType.Bool)]
    public static extern bool IsWindow(nint hwnd);

    [DllImport("user32.dll")]
    [return: MarshalAs(UnmanagedType.Bool)]
    public static extern bool GetCursorPos(out Point p);

    [StructLayout(LayoutKind.Sequential)]
    public struct Point
    {
        public int X;
        public int Y;
    }

    [DllImport("user32.dll", CharSet = CharSet.Unicode)]
    public static extern int GetWindowTextW(nint hwnd, [Out] char[] text, int max);

    public static string TitleOf(nint hwnd)
    {
        var b = new char[256];
        int n = hwnd == 0 ? 0 : GetWindowTextW(hwnd, b, b.Length);
        return n > 0 ? new string(b, 0, n) : "";
    }

    public static int ProcessOf(nint hwnd)
    {
        if (hwnd == 0) return 0;
        GetWindowThreadProcessId(hwnd, out uint pid);
        return (int)pid;
    }

    private const uint ProcessQueryLimitedInformation = 0x1000;
    private const uint TokenQuery = 0x0008;
    private const int TokenElevationClass = 20;

    [DllImport("kernel32.dll", SetLastError = true)]
    private static extern nint OpenProcess(uint access, [MarshalAs(UnmanagedType.Bool)] bool inherit, int pid);

    [DllImport("kernel32.dll", SetLastError = true)]
    [return: MarshalAs(UnmanagedType.Bool)]
    private static extern bool CloseHandle(nint h);

    [DllImport("kernel32.dll", SetLastError = true, CharSet = CharSet.Unicode)]
    [return: MarshalAs(UnmanagedType.Bool)]
    private static extern bool QueryFullProcessImageNameW(nint process, uint flags, [Out] char[] path, ref uint size);

    [DllImport("advapi32.dll", SetLastError = true)]
    [return: MarshalAs(UnmanagedType.Bool)]
    private static extern bool OpenProcessToken(nint process, uint access, out nint token);

    [DllImport("advapi32.dll", SetLastError = true)]
    [return: MarshalAs(UnmanagedType.Bool)]
    private static extern bool GetTokenInformation(nint token, int cls, out int info, int size, out int returned);

    [DllImport("kernel32.dll")]
    private static extern nint GetCurrentProcess();

    /// <summary>The full image path of <paramref name="pid"/>, or "" (QueryFullProcessImageNameW with the limited right, as the WE1 reader).</summary>
    public static string ImagePath(int pid)
    {
        if (pid <= 0) return "";
        nint h = OpenProcess(ProcessQueryLimitedInformation, false, pid);
        if (h == 0) return "";
        try
        {
            var b = new char[1024];
            uint n = (uint)b.Length;
            return QueryFullProcessImageNameW(h, 0, b, ref n) ? new string(b, 0, (int)n) : "";
        }
        finally { CloseHandle(h); }
    }

    /// <summary>True when the process token says elevated; a token we may not even open is taken as elevated (UIPI's case).</summary>
    public static bool IsElevated(int pid)
    {
        nint h = pid == Environment.ProcessId ? GetCurrentProcess() : OpenProcess(ProcessQueryLimitedInformation, false, pid);
        if (h == 0) return false;
        try { return TokenElevated(h); }
        finally { if (pid != Environment.ProcessId) CloseHandle(h); }
    }

    private static bool TokenElevated(nint process)
    {
        if (!OpenProcessToken(process, TokenQuery, out nint token)) return Marshal.GetLastWin32Error() == 5; // ERROR_ACCESS_DENIED
        try { return GetTokenInformation(token, TokenElevationClass, out int elevated, sizeof(int), out _) && elevated != 0; }
        finally { CloseHandle(token); }
    }

    // MARK: clipboard

    public const uint CfUnicodeText = 13;
    private const uint GmemMoveable = 0x0002;

    [DllImport("user32.dll", SetLastError = true)]
    [return: MarshalAs(UnmanagedType.Bool)]
    public static extern bool OpenClipboard(nint owner);

    [DllImport("user32.dll", SetLastError = true)]
    [return: MarshalAs(UnmanagedType.Bool)]
    public static extern bool CloseClipboard();

    [DllImport("user32.dll", SetLastError = true)]
    [return: MarshalAs(UnmanagedType.Bool)]
    public static extern bool EmptyClipboard();

    [DllImport("user32.dll", SetLastError = true)]
    public static extern uint EnumClipboardFormats(uint format);

    [DllImport("user32.dll", SetLastError = true)]
    public static extern nint GetClipboardData(uint format);

    [DllImport("user32.dll", SetLastError = true)]
    public static extern nint SetClipboardData(uint format, nint mem);

    [DllImport("user32.dll")]
    [return: MarshalAs(UnmanagedType.Bool)]
    public static extern bool IsClipboardFormatAvailable(uint format);

    [DllImport("user32.dll", SetLastError = true, CharSet = CharSet.Unicode)]
    public static extern uint RegisterClipboardFormatW(string name);

    [DllImport("user32.dll", SetLastError = true, CharSet = CharSet.Unicode)]
    public static extern int GetClipboardFormatNameW(uint format, [Out] char[] name, int max);

    [DllImport("user32.dll")]
    public static extern uint GetClipboardSequenceNumber();

    [DllImport("kernel32.dll", SetLastError = true)]
    public static extern nint GlobalAlloc(uint flags, nuint bytes);

    [DllImport("kernel32.dll", SetLastError = true)]
    public static extern nint GlobalLock(nint mem);

    [DllImport("kernel32.dll", SetLastError = true)]
    [return: MarshalAs(UnmanagedType.Bool)]
    public static extern bool GlobalUnlock(nint mem);

    [DllImport("kernel32.dll", SetLastError = true)]
    public static extern nuint GlobalSize(nint mem);

    [DllImport("kernel32.dll", SetLastError = true)]
    public static extern nint GlobalFree(nint mem);

    /// <summary>A movable HGLOBAL holding <paramref name="bytes"/>; 0 on failure.</summary>
    public static nint Global(ReadOnlySpan<byte> bytes)
    {
        nint mem = GlobalAlloc(GmemMoveable, (nuint)Math.Max(1, bytes.Length));
        if (mem == 0) return 0;
        nint p = GlobalLock(mem);
        if (p == 0) { GlobalFree(mem); return 0; }
        unsafe { bytes.CopyTo(new Span<byte>((void*)p, bytes.Length)); }
        GlobalUnlock(mem);
        return mem;
    }

    /// <summary>The bytes of an HGLOBAL (the clipboard keeps ownership); null when it is not one.</summary>
    public static byte[]? Bytes(nint mem)
    {
        if (mem == 0) return null;
        long size = (long)GlobalSize(mem);
        if (size <= 0) return null;
        nint p = GlobalLock(mem);
        if (p == 0) return null;
        try
        {
            var b = new byte[size];
            Marshal.Copy(p, b, 0, (int)size);
            return b;
        }
        finally { GlobalUnlock(mem); }
    }

    /// <summary>UTF-16 text + NUL.</summary>
    public static byte[] Utf16z(string s)
    {
        var b = new byte[(s.Length + 1) * 2];
        System.Text.Encoding.Unicode.GetBytes(s, 0, s.Length, b, 0);
        return b;
    }

    /// <summary>UTF-16 bytes up to the first NUL.</summary>
    public static string FromUtf16z(byte[] b)
    {
        int n = 0;
        while (n + 1 < b.Length && (b[n] != 0 || b[n + 1] != 0)) n += 2;
        return System.Text.Encoding.Unicode.GetString(b, 0, n);
    }

    // MARK: WTS session notifications (§13.8 lock refusal)

    public const uint WmWtsSessionChange = 0x02B1;
    private const uint NotifyForThisSession = 0;

    [DllImport("wtsapi32.dll", SetLastError = true)]
    [return: MarshalAs(UnmanagedType.Bool)]
    public static extern bool WTSRegisterSessionNotification(nint hwnd, uint flags);

    public static bool RegisterSessionNotification(nint hwnd) => WTSRegisterSessionNotification(hwnd, NotifyForThisSession);

    [DllImport("wtsapi32.dll", SetLastError = true)]
    [return: MarshalAs(UnmanagedType.Bool)]
    public static extern bool WTSUnRegisterSessionNotification(nint hwnd);

    private const int WtsSessionInfoEx = 25;
    private const int WtsCurrentSession = -1;

    [DllImport("wtsapi32.dll", SetLastError = true)]
    [return: MarshalAs(UnmanagedType.Bool)]
    private static extern bool WTSQuerySessionInformationW(nint server, int session, int infoClass, out nint buffer, out int bytes);

    [DllImport("wtsapi32.dll")]
    private static extern void WTSFreeMemory(nint memory);

    /// <summary>
    /// Is this session locked right now (<c>WTSSessionInfoEx</c> → <c>WTSINFOEX_LEVEL1_W.SessionFlags</c>: 0 = WTS_SESSIONSTATE_LOCK,
    /// 1 = UNLOCK, −1 = unknown)? Null when the query failed. Read once at start; the notifications keep it current.
    /// </summary>
    public static bool? SessionLocked()
    {
        if (!WTSQuerySessionInformationW(0, WtsCurrentSession, WtsSessionInfoEx, out nint buf, out int bytes) || buf == 0) return null;
        try
        {
            // WTSINFOEXW { DWORD Level; WTSINFOEX_LEVEL_W Data; } — the union holds LARGE_INTEGERs, so Data starts at 8; Level1 =
            // { ULONG SessionId (8); WTS_CONNECTSTATE_CLASS SessionState (12); LONG SessionFlags (16); … }.
            if (bytes < 20 || Marshal.ReadInt32(buf, 0) != 1) return null;
            int flags = Marshal.ReadInt32(buf, 16);
            return flags switch { 0 => true, 1 => false, _ => null };
        }
        finally { WTSFreeMemory(buf); }
    }

    // MARK: the hidden window (WTS notifications, the clipboard owner, the synthetic lock message)

    public delegate nint WndProc(nint hwnd, uint msg, nint w, nint l);

    [StructLayout(LayoutKind.Sequential, CharSet = CharSet.Unicode)]
    public struct WndClassEx
    {
        public uint Size;
        public uint Style;
        public nint WndProc;
        public int ClsExtra;
        public int WndExtra;
        public nint Instance;
        public nint Icon;
        public nint Cursor;
        public nint Background;
        public string? MenuName;
        public string ClassName;
        public nint IconSm;
    }

    [DllImport("user32.dll", SetLastError = true, CharSet = CharSet.Unicode)]
    public static extern ushort RegisterClassExW(ref WndClassEx cls);

    [DllImport("user32.dll", SetLastError = true, CharSet = CharSet.Unicode)]
    public static extern nint CreateWindowExW(uint exStyle, string className, string title, uint style, int x, int y, int w, int h,
        nint parent, nint menu, nint instance, nint param);

    [DllImport("user32.dll", CharSet = CharSet.Unicode)]
    public static extern nint DefWindowProcW(nint hwnd, uint msg, nint w, nint l);

    [DllImport("user32.dll", SetLastError = true)]
    [return: MarshalAs(UnmanagedType.Bool)]
    public static extern bool DestroyWindow(nint hwnd);

    [DllImport("user32.dll", SetLastError = true)]
    [return: MarshalAs(UnmanagedType.Bool)]
    public static extern bool PostMessageW(nint hwnd, uint msg, nint w, nint l);

    [DllImport("kernel32.dll", CharSet = CharSet.Unicode)]
    public static extern nint GetModuleHandleW(string? name);

    [DllImport("kernel32.dll")]
    public static extern uint ProcessIdToSessionId(uint pid, out uint session);
}
