using System.Runtime.InteropServices;

namespace Theuth.Companion.AskBar;

/// <summary>
/// The few user32 calls the Ask bar needs before W2's CsWin32 <c>Interop/</c> exists: the foreground hand-back on Esc
/// (design 134 D5/D7: the bar is activated when summoned and returns the foreground to the window it took it from), the
/// physical key state that tells a hold from a tap after <c>RegisterHotKey</c> delivered only the key-down, the cursor
/// position for placement, the DPI for sizing and the capture exclusion (C11). W2 can fold these into
/// <c>NativeMethods.txt</c> at integration. Classic <c>DllImport</c>: <c>LibraryImport</c> needs <c>AllowUnsafeBlocks</c>, a
/// shared csproj property this wave does not own.
/// </summary>
internal static class Win32
{
    public const uint WdaNone = 0;
    public const uint WdaExcludeFromCapture = 0x11;

    [DllImport("user32.dll")]
    public static extern nint GetForegroundWindow();

    [DllImport("user32.dll")]
    [return: MarshalAs(UnmanagedType.Bool)]
    public static extern bool SetForegroundWindow(nint hwnd);

    [DllImport("user32.dll")]
    public static extern short GetAsyncKeyState(int vKey);

    [DllImport("user32.dll")]
    [return: MarshalAs(UnmanagedType.Bool)]
    public static extern bool GetCursorPos(out Point point);

    [DllImport("user32.dll")]
    public static extern uint GetDpiForWindow(nint hwnd);

    [DllImport("user32.dll")]
    [return: MarshalAs(UnmanagedType.Bool)]
    public static extern bool SetWindowDisplayAffinity(nint hwnd, uint affinity);

    [StructLayout(LayoutKind.Sequential)]
    public struct Point
    {
        public int X;
        public int Y;
    }

    /// <summary>true while the key is physically down (the high bit of <see cref="GetAsyncKeyState"/>).</summary>
    public static bool IsKeyDown(int vKey) => (GetAsyncKeyState(vKey) & 0x8000) != 0;
}
