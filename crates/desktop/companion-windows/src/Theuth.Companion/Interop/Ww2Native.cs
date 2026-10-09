using System.Runtime.InteropServices;
using Windows.Win32.Foundation;

namespace Theuth.Companion.Interop;

/// <summary>
/// 149 WW2 (§7.4 #2): the Win32 calls the pill rail and the rounded panels need beyond W2's <c>NativeMethods.txt</c> —
/// declared here (not in the shared CsWin32 list) so sibling waves never merge over them. A window's shape comes from
/// <c>SetWindowRgn</c> (a union of capsules: the gaps between pills are then outside the window, so clicks there pass
/// through and nothing is painted); the panels and the Ask bar take DWM's rounded corners, which keep the system shadow.
/// </summary>
internal static class Shape
{
    private const int RGN_OR = 2;
    private const uint DWMWA_WINDOW_CORNER_PREFERENCE = 33;
    private const int DWMWCP_ROUND = 2;
    private const int DWMWCP_DONOTROUND = 1;

    [DllImport("gdi32.dll", SetLastError = true)]
    private static extern nint CreateRoundRectRgn(int x1, int y1, int x2, int y2, int w, int h);

    [DllImport("gdi32.dll", SetLastError = true)]
    private static extern nint CreateEllipticRgn(int x1, int y1, int x2, int y2);

    [DllImport("gdi32.dll", SetLastError = true)]
    private static extern int CombineRgn(nint dst, nint src1, nint src2, int mode);

    [DllImport("gdi32.dll", SetLastError = true)]
    private static extern bool DeleteObject(nint obj);

    [DllImport("user32.dll", SetLastError = true)]
    private static extern int SetWindowRgn(nint hwnd, nint rgn, bool redraw);

    [DllImport("dwmapi.dll")]
    private static extern int DwmSetWindowAttribute(nint hwnd, uint attr, ref int value, int size);

    /// <summary>A capsule or ellipse in window coordinates (physical px). <paramref name="Radius"/> ≥ half the short side makes a capsule.</summary>
    public readonly record struct Blob(int Left, int Top, int Right, int Bottom, int Radius, bool Ellipse = false);

    /// <summary>Shape the window as the union of <paramref name="blobs"/>; the system owns the region afterwards. Empty → the whole rectangle.</summary>
    public static bool Set(HWND hwnd, IReadOnlyList<Blob> blobs)
    {
        if (blobs.Count == 0) return SetWindowRgn(hwnd.Value, 0, true) != 0;
        nint all = 0;
        foreach (var b in blobs)
        {
            // GDI's round-rect ellipse size is the full width/height of the corner ellipse; the right/bottom edges are exclusive.
            nint r = b.Ellipse
                ? CreateEllipticRgn(b.Left, b.Top, b.Right + 1, b.Bottom + 1)
                : CreateRoundRectRgn(b.Left, b.Top, b.Right + 1, b.Bottom + 1, b.Radius * 2, b.Radius * 2);
            if (r == 0) continue;
            if (all == 0)
            {
                all = r;
                continue;
            }
            CombineRgn(all, all, r, RGN_OR);
            DeleteObject(r);
        }
        if (all == 0) return false;
        return SetWindowRgn(hwnd.Value, all, true) != 0; // SetWindowRgn takes ownership
    }

    /// <summary>Back to a rectangle.</summary>
    public static void Clear(HWND hwnd) => SetWindowRgn(hwnd.Value, 0, true);

    /// <summary>DWM's rounded corners (Windows 11; ~8 px, the system shadow stays). False when DWM refused (Windows 10).</summary>
    public static bool RoundCorners(HWND hwnd, bool round = true)
    {
        int v = round ? DWMWCP_ROUND : DWMWCP_DONOTROUND;
        return DwmSetWindowAttribute(hwnd.Value, DWMWA_WINDOW_CORNER_PREFERENCE, ref v, sizeof(int)) == 0;
    }
}
