using System.Runtime.InteropServices;
using Microsoft.UI.Xaml;
using Windows.Win32;
using Windows.Win32.Foundation;
using Windows.Win32.UI.WindowsAndMessaging;

namespace Theuth.Companion.Interop;

/// <summary>
/// Small helpers over the CsWin32 surface (design 134 W2): window identity for the foreground bookkeeping, ex-styles,
/// rectangles, DPI and log text. Everything is called on the UI thread.
/// </summary>
internal static class Win32
{
    /// <summary><c>NIN_KEYSELECT</c> = <c>NIN_SELECT | NINF_KEY</c> (not in the metadata).</summary>
    public const uint NinKeySelect = PInvoke.NIN_SELECT | 0x1;

    /// <summary><c>HWND_TOPMOST</c>.</summary>
    public static HWND Topmost => (HWND)(nint)(-1);

    public static HWND Hwnd(Window w) => (HWND)WinRT.Interop.WindowNative.GetWindowHandle(w);

    public static WINDOW_EX_STYLE ExStyle(HWND h) =>
        (WINDOW_EX_STYLE)unchecked((uint)PInvoke.GetWindowLongPtr(h, WINDOW_LONG_PTR_INDEX.GWL_EXSTYLE));

    public static void SetExStyle(HWND h, WINDOW_EX_STYLE s) =>
        PInvoke.SetWindowLongPtr(h, WINDOW_LONG_PTR_INDEX.GWL_EXSTYLE, unchecked((nint)(uint)s));

    /// <summary>Re-asserts topmost and makes a style change take effect, never activating.</summary>
    public static void FrameChanged(HWND h) =>
        PInvoke.SetWindowPos(h, Topmost, 0, 0, 0, 0,
            SET_WINDOW_POS_FLAGS.SWP_NOMOVE | SET_WINDOW_POS_FLAGS.SWP_NOSIZE | SET_WINDOW_POS_FLAGS.SWP_NOACTIVATE | SET_WINDOW_POS_FLAGS.SWP_FRAMECHANGED);

    public static uint ProcessOf(HWND h)
    {
        if (h.IsNull) return 0;
        PInvoke.GetWindowThreadProcessId(h, out uint pid);
        return pid;
    }

    public static bool IsOurs(HWND h) => !h.IsNull && ProcessOf(h) == (uint)Environment.ProcessId;

    public static bool Alive(HWND h) => !h.IsNull && PInvoke.IsWindow(h);

    public static string ClassOf(HWND h)
    {
        if (h.IsNull) return "";
        Span<char> b = stackalloc char[256];
        int n = PInvoke.GetClassName(h, b);
        return n > 0 ? new string(b[..n]) : "";
    }

    public static string TitleOf(HWND h)
    {
        if (h.IsNull) return "";
        Span<char> b = stackalloc char[256];
        int n = PInvoke.GetWindowText(h, b);
        return n > 0 ? new string(b[..n]) : "";
    }

    /// <summary>"0x1a2b pid=1234 Notepad "Untitled - Notepad"" for the log.</summary>
    public static string Describe(HWND h) => h.IsNull ? "none" : $"{h} pid={ProcessOf(h)} {ClassOf(h)} \"{TitleOf(h)}\"";

    // The taskbar, its overflow flyout and the shell's own popups (Start, search, the notification centre): a window the
    // user clicked to reach the companion, not the app they were working in.
    private static readonly HashSet<string> ShellChrome = new(StringComparer.Ordinal)
    {
        "Shell_TrayWnd", "Shell_SecondaryTrayWnd", "NotifyIconOverflowWindow", "TopLevelWindowForOverflowXamlIsland",
        "XamlExplorerHostIslandWindow", "Windows.UI.Core.CoreWindow",
    };

    public static bool IsShellChrome(HWND h) => ShellChrome.Contains(ClassOf(h));

    public static double Scale(HWND h)
    {
        uint dpi = PInvoke.GetDpiForWindow(h);
        return dpi == 0 ? 1.0 : dpi / 96.0;
    }

    public static RECT Rect(HWND h)
    {
        PInvoke.GetWindowRect(h, out var r);
        return r;
    }

    public static string Text(RECT r) => $"({r.left},{r.top}) {r.right - r.left}x{r.bottom - r.top}";

    public static System.Drawing.Point Cursor()
    {
        PInvoke.GetCursorPos(out var p);
        return p;
    }

    public static bool Contains(RECT r, System.Drawing.Point p) => p.X >= r.left && p.X < r.right && p.Y >= r.top && p.Y < r.bottom;

    public static RECT Inflate(RECT r, int by) => new() { left = r.left - by, top = r.top - by, right = r.right + by, bottom = r.bottom + by };

    /// <summary>The key is down now (the high bit of <c>GetKeyState</c>).</summary>
    public static bool IsDown(int vk) => (PInvoke.GetKeyState(vk) & 0x8000) != 0;

    /// <summary>Hands the foreground to <paramref name="target"/> when it is still a window.</summary>
    public static bool Foreground(HWND target) => Alive(target) && PInvoke.SetForegroundWindow(target);

    public static string ExStyleText(WINDOW_EX_STYLE s)
    {
        var names = new List<string>();
        if (s.HasFlag(WINDOW_EX_STYLE.WS_EX_TOPMOST)) names.Add("TOPMOST");
        if (s.HasFlag(WINDOW_EX_STYLE.WS_EX_TOOLWINDOW)) names.Add("TOOLWINDOW");
        if (s.HasFlag(WINDOW_EX_STYLE.WS_EX_NOACTIVATE)) names.Add("NOACTIVATE");
        if (s.HasFlag(WINDOW_EX_STYLE.WS_EX_LAYERED)) names.Add("LAYERED");
        if (s.HasFlag(WINDOW_EX_STYLE.WS_EX_TRANSPARENT)) names.Add("TRANSPARENT");
        return $"0x{(uint)s:X8} [{string.Join("|", names)}]";
    }

    public static uint DisplayAffinity(HWND h) => PInvoke.GetWindowDisplayAffinity(h, out uint a) ? a : 0;

    /// <summary>Every top-level window this process owns (the probe's window list, Swift's CGWindowList line).</summary>
    public static List<HWND> OurWindows()
    {
        var list = new List<HWND>();
        uint me = (uint)Environment.ProcessId;
        PInvoke.EnumWindows((h, _) =>
        {
            if (ProcessOf(h) == me) list.Add(h);
            return true;
        }, default);
        return list;
    }

    public static int LoWord(nint v) => unchecked((short)(v & 0xFFFF));

    public static int HiWord(nint v) => unchecked((short)((v >> 16) & 0xFFFF));

    public static string LastError() => $"err={Marshal.GetLastWin32Error()}";
}
