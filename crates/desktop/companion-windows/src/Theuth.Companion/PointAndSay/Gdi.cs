using System.Runtime.InteropServices;

namespace Theuth.Companion.PointAndSay;

/// <summary>
/// 149 WW3: the few GDI / monitor entry points the overlays and the island's display lookup need, hand-declared here (not
/// in W2's CsWin32 list) because CsWin32's SafeHandle overloads for <c>CreateCompatibleDC</c>/<c>UpdateLayeredWindow</c> are
/// awkward to mix with pointer parameters. Everything else goes through <c>PInvoke</c>.
/// </summary>
internal static unsafe class Gdi
{
    public const uint UlwAlpha = 0x02;
    public const byte AcSrcOver = 0x00;
    public const byte AcSrcAlpha = 0x01;
    public const int SwHide = 0;
    public const int SwShowNoActivate = 4;
    public const int MdtEffectiveDpi = 0;
    public const int VkLButton = 0x01;
    public const uint MonitorDefaultToNearest = 2;

    [StructLayout(LayoutKind.Sequential)]
    public struct Point { public int X, Y; }

    [StructLayout(LayoutKind.Sequential)]
    public struct Size { public int Cx, Cy; }

    [StructLayout(LayoutKind.Sequential)]
    public struct Rect { public int Left, Top, Right, Bottom; }

    [StructLayout(LayoutKind.Sequential)]
    public struct BlendFunction { public byte BlendOp, BlendFlags, SourceConstantAlpha, AlphaFormat; }

    [StructLayout(LayoutKind.Sequential)]
    public struct BitmapInfoHeader
    {
        public uint Size;
        public int Width;
        public int Height;
        public ushort Planes;
        public ushort BitCount;
        public uint Compression;
        public uint SizeImage;
        public int XPelsPerMeter;
        public int YPelsPerMeter;
        public uint ClrUsed;
        public uint ClrImportant;
    }

    [StructLayout(LayoutKind.Sequential)]
    public struct BitmapInfo
    {
        public BitmapInfoHeader Header;
        public uint Color0;
    }

    [StructLayout(LayoutKind.Sequential, CharSet = CharSet.Unicode)]
    public struct MonitorInfoEx
    {
        public uint Size;
        public Rect Monitor;
        public Rect Work;
        public uint Flags;
        [MarshalAs(UnmanagedType.ByValTStr, SizeConst = 32)] public string Device;
    }

    [DllImport("user32.dll", SetLastError = true)]
    public static extern bool UpdateLayeredWindow(nint hwnd, nint hdcDst, Point* pptDst, Size* psize, nint hdcSrc, Point* pptSrc, uint crKey, BlendFunction* pblend, uint dwFlags);

    [DllImport("user32.dll")]
    public static extern nint GetDC(nint hwnd);

    [DllImport("user32.dll")]
    public static extern int ReleaseDC(nint hwnd, nint hdc);

    [DllImport("user32.dll")]
    public static extern bool ShowWindow(nint hwnd, int cmd);

    [DllImport("user32.dll", CharSet = CharSet.Unicode, SetLastError = true)]
    public static extern bool GetMonitorInfoW(nint hMonitor, ref MonitorInfoEx info);

    [DllImport("user32.dll")]
    public static extern nint MonitorFromPoint(Point pt, uint flags);

    [DllImport("user32.dll")]
    public static extern nint MonitorFromWindow(nint hwnd, uint flags);

    [DllImport("shcore.dll")]
    public static extern int GetDpiForMonitor(nint hMonitor, int dpiType, out uint dpiX, out uint dpiY);

    [DllImport("gdi32.dll")]
    public static extern nint CreateCompatibleDC(nint hdc);

    [DllImport("gdi32.dll")]
    public static extern bool DeleteDC(nint hdc);

    [DllImport("gdi32.dll")]
    public static extern nint CreateDIBSection(nint hdc, ref BitmapInfo pbmi, uint usage, out nint ppvBits, nint hSection, uint offset);

    [DllImport("gdi32.dll")]
    public static extern nint SelectObject(nint hdc, nint h);

    [DllImport("gdi32.dll")]
    public static extern bool DeleteObject(nint h);

    /// <summary>The monitor's device name (<c>\\.\DISPLAY1</c>) and rects, or null when the handle is stale.</summary>
    public static (string Device, Rect Monitor, Rect Work)? MonitorInfo(nint hMonitor)
    {
        var mi = new MonitorInfoEx { Size = (uint)Marshal.SizeOf<MonitorInfoEx>() };
        if (hMonitor == 0 || !GetMonitorInfoW(hMonitor, ref mi)) return null;
        return (mi.Device ?? "", mi.Monitor, mi.Work);
    }

    /// <summary>Effective scale (DPI / 96) of a monitor; 1.0 when the call fails.</summary>
    public static double ScaleOf(nint hMonitor)
    {
        if (hMonitor == 0 || GetDpiForMonitor(hMonitor, MdtEffectiveDpi, out var dx, out _) != 0 || dx == 0) return 1.0;
        return dx / 96.0;
    }
}
