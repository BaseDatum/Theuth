using System.Runtime.InteropServices;

namespace Theuth.Companion.AskBar;

/// <summary>
/// The few user32 calls the Ask bar needs before W2's CsWin32 <c>Interop/</c> exists: the foreground hand-back on Esc
/// (design 134 D5/D7: the bar is activated when summoned and returns the foreground to the window it took it from), the
/// physical key state that tells a hold from a tap after <c>RegisterHotKey</c> delivered only the key-down, the cursor
/// position for placement, the DPI for sizing and the capture exclusion (C11). W2 can fold these into
/// <c>NativeMethods.txt</c> at integration.
/// </summary>
internal static partial class Win32
{
    public const uint WdaNone = 0;
    public const uint WdaExcludeFromCapture = 0x11;

    [LibraryImport("user32.dll")]
    public static partial nint GetForegroundWindow();

    [LibraryImport("user32.dll")]
    [return: MarshalAs(UnmanagedType.Bool)]
    public static partial bool SetForegroundWindow(nint hwnd);

    [LibraryImport("user32.dll")]
    public static partial short GetAsyncKeyState(int vKey);

    [LibraryImport("user32.dll")]
    [return: MarshalAs(UnmanagedType.Bool)]
    public static partial bool GetCursorPos(out Point point);

    [LibraryImport("user32.dll")]
    public static partial uint GetDpiForWindow(nint hwnd);

    [LibraryImport("user32.dll")]
    [return: MarshalAs(UnmanagedType.Bool)]
    public static partial bool SetWindowDisplayAffinity(nint hwnd, uint affinity);

    [StructLayout(LayoutKind.Sequential)]
    public struct Point
    {
        public int X;
        public int Y;
    }

    /// <summary>true while the key is physically down (the high bit of <see cref="GetAsyncKeyState"/>).</summary>
    public static bool IsKeyDown(int vKey) => (GetAsyncKeyState(vKey) & 0x8000) != 0;
}
