using Microsoft.UI.Xaml;
using Microsoft.UI.Xaml.Automation;
using Microsoft.UI.Xaml.Controls;
using Microsoft.UI.Xaml.Media;
using Windows.Graphics;

namespace SpikeHost;

internal static class Ui
{
    /// <summary>The read-only log TextBox every mode shows; UIA id "SpikeLog".</summary>
    public static TextBox LogBox()
    {
        var tb = new TextBox
        {
            IsReadOnly = true,
            AcceptsReturn = true,
            TextWrapping = TextWrapping.Wrap,
            FontFamily = new FontFamily("Consolas"),
            FontSize = 12,
            HorizontalAlignment = HorizontalAlignment.Stretch,
            VerticalAlignment = VerticalAlignment.Stretch,
            IsSpellCheckEnabled = false,
        };
        AutomationProperties.SetAutomationId(tb, "SpikeLog");
        ScrollViewer.SetVerticalScrollBarVisibility(tb, ScrollBarVisibility.Auto);
        SpikeLog.Attach(tb);
        return tb;
    }

    /// <summary>Grid with an optional header row and the log box below.</summary>
    public static Grid LogGrid(FrameworkElement? header)
    {
        var grid = new Grid();
        grid.RowDefinitions.Add(new RowDefinition { Height = GridLength.Auto });
        grid.RowDefinitions.Add(new RowDefinition { Height = new GridLength(1, GridUnitType.Star) });
        if (header is not null)
        {
            Grid.SetRow(header, 0);
            grid.Children.Add(header);
        }
        var log = LogBox();
        Grid.SetRow(log, 1);
        grid.Children.Add(log);
        return grid;
    }

    public static IntPtr Hwnd(Window w) => WinRT.Interop.WindowNative.GetWindowHandle(w);

    public static double Scale(IntPtr hwnd) => Native.GetDpiForWindow(hwnd) / 96.0;

    public static string Describe(IntPtr hwnd)
    {
        if (hwnd == IntPtr.Zero) return "0x0 (none)";
        var sb = new System.Text.StringBuilder(512);
        Native.GetWindowText(hwnd, sb, sb.Capacity);
        var cls = new System.Text.StringBuilder(256);
        Native.GetClassName(hwnd, cls, cls.Capacity);
        Native.GetWindowThreadProcessId(hwnd, out var pid);
        string proc = "?";
        try { proc = System.Diagnostics.Process.GetProcessById((int)pid).ProcessName; } catch { }
        return $"0x{hwnd.ToInt64():X} pid={pid} {proc} class=\"{cls}\" title=\"{sb}\"";
    }

    public static string ExStyleBits(long ex)
    {
        var parts = new List<string>();
        if ((ex & Native.WS_EX_TOPMOST) != 0) parts.Add("TOPMOST");
        if ((ex & Native.WS_EX_TRANSPARENT) != 0) parts.Add("TRANSPARENT");
        if ((ex & Native.WS_EX_TOOLWINDOW) != 0) parts.Add("TOOLWINDOW");
        if ((ex & Native.WS_EX_LAYERED) != 0) parts.Add("LAYERED");
        if ((ex & Native.WS_EX_NOACTIVATE) != 0) parts.Add("NOACTIVATE");
        if ((ex & Native.WS_EX_APPWINDOW) != 0) parts.Add("APPWINDOW");
        if ((ex & Native.WS_EX_NOREDIRECTIONBITMAP) != 0) parts.Add("NOREDIRECTIONBITMAP");
        return $"0x{ex:X8} [{string.Join("|", parts)}]";
    }

    public static string Rect(IntPtr hwnd)
    {
        Native.GetWindowRect(hwnd, out var r);
        return $"({r.Left},{r.Top})-({r.Right},{r.Bottom}) {r.Right - r.Left}x{r.Bottom - r.Top}px";
    }

    /// <summary>Place a window at a physical-pixel size, centred on the primary work area with an offset.</summary>
    public static void Place(Window w, int dipW, int dipH, int dx = 0, int dy = 0)
    {
        var hwnd = Hwnd(w);
        var s = Scale(hwnd);
        var wa = Microsoft.UI.Windowing.DisplayArea.Primary.WorkArea;
        int pw = (int)Math.Round(dipW * s), ph = (int)Math.Round(dipH * s);
        int x = wa.X + (wa.Width - pw) / 2 + (int)Math.Round(dx * s);
        int y = wa.Y + (wa.Height - ph) / 2 + (int)Math.Round(dy * s);
        w.AppWindow.MoveAndResize(new RectInt32(x, y, pw, ph));
    }
}
