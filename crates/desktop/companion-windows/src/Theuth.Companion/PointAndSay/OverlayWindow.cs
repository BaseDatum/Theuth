using System.Runtime.InteropServices;
using Theuth.Companion.Interop;
using Windows.Win32;
using Windows.Win32.Foundation;
using Windows.Win32.UI.WindowsAndMessaging;

namespace Theuth.Companion.PointAndSay;

/// <summary>
/// One point-and-say overlay (149 §7.4 #4, D13): a layered, click-through, topmost, non-activating tool window covering one
/// monitor in physical pixels (<c>WS_EX_LAYERED | WS_EX_TRANSPARENT | WS_EX_TOPMOST | WS_EX_NOACTIVATE | WS_EX_TOOLWINDOW</c>),
/// painted by <see cref="TrailRaster"/> through <c>UpdateLayeredWindow</c> from a 32-bit DIB (per-pixel alpha, no XAML),
/// <c>WDA_EXCLUDEFROMCAPTURE</c> so the trail never lands in a capture — and still hidden before <c>screen_context</c> runs
/// (belt and braces). Shown with <c>SW_SHOWNOACTIVATE</c>; never takes focus or clicks.
/// </summary>
internal sealed unsafe class OverlayWindow : IDisposable
{
    public const string ClassName = "TheuthCompanionOverlay";
    private static WNDPROC? proc; // one class, one procedure, referenced for the process lifetime
    private static bool classRegistered;

    private readonly nint memDc;
    private readonly nint dib;
    private readonly nint bits;
    private readonly nint oldBitmap;
    private bool shown;
    private bool disposed;

    public OverlayWindow(ScreenMonitor monitor, uint rgb, bool excludeFromCapture)
    {
        Monitor = monitor;
        Raster = new TrailRaster(monitor.Bounds.Width, monitor.Bounds.Height, monitor.Scale, rgb);
        EnsureClass();
        HINSTANCE instance = PInvoke.GetModuleHandle(default(PCWSTR));
        fixed (char* cls = ClassName)
        fixed (char* title = "theuth point-and-say")
        {
            Hwnd = PInvoke.CreateWindowEx(
                WINDOW_EX_STYLE.WS_EX_LAYERED | WINDOW_EX_STYLE.WS_EX_TRANSPARENT | WINDOW_EX_STYLE.WS_EX_TOPMOST
                | WINDOW_EX_STYLE.WS_EX_NOACTIVATE | WINDOW_EX_STYLE.WS_EX_TOOLWINDOW,
                cls, title, WINDOW_STYLE.WS_POPUP,
                monitor.Bounds.X, monitor.Bounds.Y, monitor.Bounds.Width, monitor.Bounds.Height, default, default, instance, null);
        }
        if (Hwnd.IsNull)
        {
            Log.Write($"overlay {monitor.Device}: CreateWindowEx failed {Win32.LastError()}");
            return;
        }
        if (excludeFromCapture && !PInvoke.SetWindowDisplayAffinity(Hwnd, WINDOW_DISPLAY_AFFINITY.WDA_EXCLUDEFROMCAPTURE))
        {
            Log.Write($"overlay {monitor.Device}: SetWindowDisplayAffinity failed {Win32.LastError()}");
        }
        nint screen = Gdi.GetDC(0);
        memDc = Gdi.CreateCompatibleDC(screen);
        var bmi = new Gdi.BitmapInfo
        {
            Header = new Gdi.BitmapInfoHeader
            {
                Size = (uint)Marshal.SizeOf<Gdi.BitmapInfoHeader>(),
                Width = Raster.Width,
                Height = -Raster.Height, // top-down
                Planes = 1,
                BitCount = 32,
                Compression = 0, // BI_RGB
            },
        };
        dib = Gdi.CreateDIBSection(screen, ref bmi, 0 /* DIB_RGB_COLORS */, out bits, 0, 0);
        Gdi.ReleaseDC(0, screen);
        if (dib == 0 || bits == 0) Log.Write($"overlay {monitor.Device}: CreateDIBSection failed {Win32.LastError()}");
        else oldBitmap = Gdi.SelectObject(memDc, dib);
    }

    public ScreenMonitor Monitor { get; }

    public TrailRaster Raster { get; }

    public HWND Hwnd { get; }

    public bool IsShown => shown;

    public void SetHiddenFromCapture(bool hidden)
    {
        if (Hwnd.IsNull) return;
        PInvoke.SetWindowDisplayAffinity(Hwnd, hidden ? WINDOW_DISPLAY_AFFINITY.WDA_EXCLUDEFROMCAPTURE : WINDOW_DISPLAY_AFFINITY.WDA_NONE);
    }

    private static void EnsureClass()
    {
        if (classRegistered) return;
        proc = (h, msg, w, l) =>
        {
            // WM_NCHITTEST never reaches a WS_EX_TRANSPARENT layered window; nothing to handle.
            return PInvoke.DefWindowProc(h, msg, w, l);
        };
        HINSTANCE instance = PInvoke.GetModuleHandle(default(PCWSTR));
        fixed (char* cls = ClassName)
        {
            var wc = new WNDCLASSEXW
            {
                cbSize = (uint)Marshal.SizeOf<WNDCLASSEXW>(),
                lpfnWndProc = proc,
                hInstance = instance,
                lpszClassName = cls,
            };
            if (PInvoke.RegisterClassEx(in wc) == 0) Log.Write($"overlay: RegisterClassEx({ClassName}) failed {Win32.LastError()}");
        }
        classRegistered = true;
    }

    /// <summary>Clears the surface and shows the window above everything, without activation.</summary>
    public void Reveal()
    {
        if (Hwnd.IsNull || disposed) return;
        Raster.Clear();
        Present();
        PInvoke.SetWindowPos(Hwnd, Win32.Topmost, Monitor.Bounds.X, Monitor.Bounds.Y, Monitor.Bounds.Width, Monitor.Bounds.Height,
            SET_WINDOW_POS_FLAGS.SWP_NOACTIVATE | SET_WINDOW_POS_FLAGS.SWP_SHOWWINDOW);
        shown = true;
    }

    public void Hide()
    {
        if (Hwnd.IsNull || disposed) return;
        Gdi.ShowWindow((nint)Hwnd.Value, Gdi.SwHide);
        shown = false;
    }

    /// <summary>Pushes <see cref="TrailRaster.Frame"/> to the screen (composes first when dirty).</summary>
    public void Present()
    {
        if (Hwnd.IsNull || bits == 0 || disposed) return;
        if (Raster.Dirty) Raster.Compose();
        Marshal.Copy(Raster.Frame, 0, bits, Raster.Frame.Length);
        nint screen = Gdi.GetDC(0);
        var dst = new Gdi.Point { X = Monitor.Bounds.X, Y = Monitor.Bounds.Y };
        var src = new Gdi.Point { X = 0, Y = 0 };
        var size = new Gdi.Size { Cx = Raster.Width, Cy = Raster.Height };
        var blend = new Gdi.BlendFunction { BlendOp = Gdi.AcSrcOver, BlendFlags = 0, SourceConstantAlpha = 255, AlphaFormat = Gdi.AcSrcAlpha };
        if (!Gdi.UpdateLayeredWindow((nint)Hwnd.Value, screen, &dst, &size, memDc, &src, 0, &blend, Gdi.UlwAlpha) && !ulwLogged)
        {
            ulwLogged = true;
            Log.Write($"overlay {Monitor.Device}: UpdateLayeredWindow failed {Win32.LastError()}");
        }
        Gdi.ReleaseDC(0, screen);
    }

    private bool ulwLogged;

    public void Dispose()
    {
        if (disposed) return;
        disposed = true;
        if (!Hwnd.IsNull) PInvoke.DestroyWindow(Hwnd);
        if (memDc != 0)
        {
            if (oldBitmap != 0) Gdi.SelectObject(memDc, oldBitmap);
            Gdi.DeleteDC(memDc);
        }
        if (dib != 0) Gdi.DeleteObject(dib);
    }
}

/// <summary>
/// One <see cref="OverlayWindow"/> per monitor (D13): created on reveal from the current monitor list (so a display change
/// between gestures is picked up), each sample routed to the monitor under it in that monitor's local physical pixels, all
/// hidden on stop/cancel — the caller then yields one dispatcher turn before <c>screen_context</c>.
/// </summary>
internal sealed class OverlayWindows : IDisposable
{
    private readonly List<OverlayWindow> windows = new();
    private readonly uint rgb;
    private bool excludeFromCapture;
    private int presentsThisGesture;

    /// <summary>The eye: applies to the live windows and to the ones created later.</summary>
    public void SetHiddenFromCapture(bool hidden)
    {
        excludeFromCapture = hidden;
        foreach (var w in windows) w.SetHiddenFromCapture(hidden);
    }

    public OverlayWindows(uint rgb, bool excludeFromCapture)
    {
        this.rgb = rgb;
        this.excludeFromCapture = excludeFromCapture;
    }

    public IReadOnlyList<OverlayWindow> Windows => windows;

    public IReadOnlyList<ScreenMonitor> Monitors { get; private set; } = Array.Empty<ScreenMonitor>();

    public void Reveal()
    {
        var monitors = ScreenMonitor.All();
        bool same = monitors.Count == windows.Count && monitors.Zip(windows).All(p => p.First == p.Second.Monitor);
        if (!same)
        {
            foreach (var w in windows) w.Dispose();
            windows.Clear();
            foreach (var m in monitors) windows.Add(new OverlayWindow(m, rgb, excludeFromCapture));
            Monitors = monitors;
            Log.Write($"overlays: {windows.Count} window(s) — {string.Join("; ", monitors.Select(m => m.Describe()))}");
        }
        presentsThisGesture = 0;
        foreach (var w in windows) w.Reveal();
    }

    /// <summary>One pointer sample in virtual-screen physical pixels.</summary>
    public void Append(int x, int y, bool pen)
    {
        foreach (var w in windows)
        {
            if (w.Monitor.Contains(x, y)) w.Raster.Append(x - w.Monitor.Bounds.X, y - w.Monitor.Bounds.Y, pen);
        }
    }

    /// <summary>Pushes every dirty surface; once per sampler tick.</summary>
    public void Present()
    {
        foreach (var w in windows)
        {
            if (!w.Raster.Dirty) continue;
            w.Present();
            presentsThisGesture++;
        }
    }

    public int Presents => presentsThisGesture;

    public void Hide()
    {
        foreach (var w in windows) w.Hide();
    }

    public void Dispose()
    {
        foreach (var w in windows) w.Dispose();
        windows.Clear();
    }
}
