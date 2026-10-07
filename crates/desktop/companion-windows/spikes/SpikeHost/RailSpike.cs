using System.Runtime.InteropServices;
using Microsoft.UI.Dispatching;
using Microsoft.UI.Windowing;
using Microsoft.UI.Xaml;
using Microsoft.UI.Xaml.Automation;
using Microsoft.UI.Xaml.Controls;
using Microsoft.UI.Xaml.Media;
using Windows.Graphics;

namespace SpikeHost;

/// <summary>
/// S1: a 26x400 dip rail on the right edge of the primary monitor: no frame, no taskbar button, topmost,
/// never activated, excluded from capture, mouse passes through everywhere except over three marks.
/// Variant (second argument): "hittest" (default) = WM_NCHITTEST -> HTTRANSPARENT subclass on the top-level
/// and child HWNDs; "layered" = WS_EX_LAYERED|WS_EX_TRANSPARENT toggled by a cursor poll; "region" = SetWindowRgn
/// clipped to the marks; "all" = hittest + layered.
/// </summary>
internal sealed class RailSpike
{
    private const int RailW = 26, RailH = 400, MarkSize = 18;

    private readonly string _variant;
    private readonly Window _rail;
    private readonly LogWindow _log;
    private readonly Grid _root;
    private readonly List<Border> _marks = new();
    private readonly List<Native.RECT> _markRects = new(); // client coordinates, physical px
    private readonly IntPtr _hwnd;
    private readonly double _scale;
    private readonly Native.SubclassProc _proc;
    private readonly List<IntPtr> _subclassed = new();
    private DispatcherQueueTimer? _timer;
    private bool _transparentNow;
    private IntPtr _lastForeignFg;
    private int _hitLogs;
    private int _toggleLogs;

    public RailSpike(string[] argv)
    {
        SpikeLog.Init("rail");
        _variant = argv.Skip(1).FirstOrDefault(a => !a.StartsWith("--", StringComparison.Ordinal)) ?? "hittest";
        SpikeLog.Append($"S1 variant={_variant}");
        _proc = HitTestProc;

        // The readable log window (normal window, exits the app when closed).
        _log = new LogWindow("SpikeHost S1 log");
        Ui.Place(_log, 900, 620, -80, 0);
        _log.Activate();

        // The rail.
        _rail = new Window { Title = "SpikeHost S1 rail" };
        _root = new Grid { Background = new SolidColorBrush(Windows.UI.Color.FromArgb(255, 28, 30, 44)) };
        var stack = new StackPanel { HorizontalAlignment = HorizontalAlignment.Center, VerticalAlignment = VerticalAlignment.Center, Spacing = 48 };
        var colors = new[] { Windows.UI.Color.FromArgb(255, 230, 70, 70), Windows.UI.Color.FromArgb(255, 70, 200, 90), Windows.UI.Color.FromArgb(255, 80, 140, 255) };
        for (int i = 0; i < 3; i++)
        {
            int idx = i + 1;
            var b = new Border { Width = MarkSize, Height = MarkSize, Background = new SolidColorBrush(colors[i]), CornerRadius = new CornerRadius(4) };
            AutomationProperties.SetAutomationId(b, $"Mark{idx}");
            b.PointerPressed += (s, e) => OnMark(idx);
            _marks.Add(b);
            stack.Children.Add(b);
        }
        _root.Children.Add(stack);
        _root.Loaded += (s, e) => OnRootLoaded();
        _rail.Content = _root;

        _hwnd = Ui.Hwnd(_rail);
        var aw = _rail.AppWindow;
        var presenter = OverlappedPresenter.Create();
        presenter.SetBorderAndTitleBar(false, false);
        presenter.IsResizable = false;
        presenter.IsMaximizable = false;
        presenter.IsMinimizable = false;
        presenter.IsAlwaysOnTop = true;
        aw.SetPresenter(presenter);
        aw.IsShownInSwitchers = false;

        _scale = Ui.Scale(_hwnd);
        var area = DisplayArea.Primary;
        var ob = area.OuterBounds;
        var wa = area.WorkArea;
        int w = (int)Math.Round(RailW * _scale), h = (int)Math.Round(RailH * _scale);
        int x = ob.X + ob.Width - w;
        int y = wa.Y + (wa.Height - h) / 2;
        aw.MoveAndResize(new RectInt32(x, y, w, h));
        SpikeLog.Append($"rail hwnd=0x{_hwnd.ToInt64():X} dpi={Native.GetDpiForWindow(_hwnd)} scale={_scale:F2} monitor outer={ob.X},{ob.Y} {ob.Width}x{ob.Height} work={wa.X},{wa.Y} {wa.Width}x{wa.Height} -> target ({x},{y}) {w}x{h}");

        // Extended styles BEFORE the first show.
        long before = (long)Native.GetWindowLongPtr(_hwnd, Native.GWL_EXSTYLE);
        long want = before | Native.WS_EX_TOOLWINDOW | Native.WS_EX_NOACTIVATE | Native.WS_EX_TOPMOST;
        bool layered = _variant == "layered" || _variant == "all";
        if (layered) want |= Native.WS_EX_LAYERED | Native.WS_EX_TRANSPARENT;
        Native.SetWindowLongPtr(_hwnd, Native.GWL_EXSTYLE, new IntPtr(want));
        int setErr = Marshal.GetLastWin32Error();
        if (layered)
        {
            bool lwa = Native.SetLayeredWindowAttributes(_hwnd, 0, 255, Native.LWA_ALPHA);
            SpikeLog.Append($"SetLayeredWindowAttributes(alpha=255) -> {lwa} err={Marshal.GetLastWin32Error()}");
            _transparentNow = true;
        }
        Native.SetWindowPos(_hwnd, Native.HWND_TOPMOST, 0, 0, 0, 0, Native.SWP_NOMOVE | Native.SWP_NOSIZE | Native.SWP_NOACTIVATE | Native.SWP_FRAMECHANGED);
        long after = (long)Native.GetWindowLongPtr(_hwnd, Native.GWL_EXSTYLE);
        SpikeLog.Append($"exstyle before={Ui.ExStyleBits(before)} wanted={Ui.ExStyleBits(want)} after={Ui.ExStyleBits(after)} (SetWindowLongPtr err={setErr})");

        bool aff = Native.SetWindowDisplayAffinity(_hwnd, Native.WDA_EXCLUDEFROMCAPTURE);
        int affErr = Marshal.GetLastWin32Error();
        Native.GetWindowDisplayAffinity(_hwnd, out var got);
        SpikeLog.Append($"SetWindowDisplayAffinity(WDA_EXCLUDEFROMCAPTURE=0x11) -> {aff} err={affErr}; GetWindowDisplayAffinity -> 0x{got:X}");

        // Show without activating (Window.Activate() would activate it).
        aw.Show(false);
        SpikeLog.Append($"rail shown: rect={Ui.Rect(_hwnd)} visible={Native.IsWindowVisible(_hwnd)} foreground={Ui.Describe(Native.GetForegroundWindow())} exstyle now={Ui.ExStyleBits((long)Native.GetWindowLongPtr(_hwnd, Native.GWL_EXSTYLE))}");
    }

    private void OnRootLoaded()
    {
        // Mark rectangles in client (= window, there is no frame) physical pixels.
        _markRects.Clear();
        for (int i = 0; i < _marks.Count; i++)
        {
            var p = _marks[i].TransformToVisual(_root).TransformPoint(new Windows.Foundation.Point(0, 0));
            var r = new Native.RECT
            {
                Left = (int)Math.Floor(p.X * _scale),
                Top = (int)Math.Floor(p.Y * _scale),
                Right = (int)Math.Ceiling((p.X + MarkSize) * _scale),
                Bottom = (int)Math.Ceiling((p.Y + MarkSize) * _scale),
            };
            _markRects.Add(r);
            SpikeLog.Append($"mark {i + 1} client rect ({r.Left},{r.Top})-({r.Right},{r.Bottom})");
        }

        // Child HWNDs (WinUI 3 hosts its content in a DesktopChildSiteBridge child window).
        var children = new List<IntPtr>();
        Native.EnumChildWindows(_hwnd, (h, l) => { children.Add(h); return true; }, IntPtr.Zero);
        foreach (var c in children) SpikeLog.Append($"child hwnd {Ui.Describe(c)}");

        if (_variant == "hittest" || _variant == "all")
        {
            foreach (var h in new[] { _hwnd }.Concat(children))
            {
                bool ok = Native.SetWindowSubclass(h, _proc, new UIntPtr(0x134), UIntPtr.Zero);
                if (ok) _subclassed.Add(h);
                SpikeLog.Append($"SetWindowSubclass(0x{h.ToInt64():X}) -> {ok}");
            }
        }
        if (_variant == "region")
        {
            IntPtr rgn = Native.CreateRectRgn(0, 0, 0, 0);
            foreach (var r in _markRects)
            {
                IntPtr one = Native.CreateRectRgn(r.Left, r.Top, r.Right, r.Bottom);
                Native.CombineRgn(rgn, rgn, one, Native.RGN_OR);
                Native.DeleteObject(one);
            }
            int res = Native.SetWindowRgn(_hwnd, rgn, true); // the system owns rgn now
            SpikeLog.Append($"SetWindowRgn(3 mark rects) -> {res} err={Marshal.GetLastWin32Error()}");
        }

        _timer = _rail.DispatcherQueue.CreateTimer();
        _timer.Interval = TimeSpan.FromMilliseconds(40);
        _timer.IsRepeating = true;
        _timer.Tick += (s, e) => OnTick();
        _timer.Start();
        SpikeLog.Append("rail ready (poll timer 40 ms running)");
    }

    private bool OverMark(int sx, int sy)
    {
        var p = new Native.POINT { X = sx, Y = sy };
        Native.ScreenToClient(_hwnd, ref p);
        foreach (var r in _markRects)
            if (p.X >= r.Left && p.X < r.Right && p.Y >= r.Top && p.Y < r.Bottom) return true;
        return false;
    }

    private IntPtr HitTestProc(IntPtr hWnd, uint uMsg, IntPtr wParam, IntPtr lParam, UIntPtr uIdSubclass, UIntPtr dwRefData)
    {
        if (uMsg == Native.WM_NCHITTEST)
        {
            int sx = Native.LoWord(lParam), sy = Native.HiWord(lParam);
            bool over = OverMark(sx, sy);
            if (_hitLogs < 12)
            {
                _hitLogs++;
                SpikeLog.Append($"WM_NCHITTEST on 0x{hWnd.ToInt64():X} at screen ({sx},{sy}) overMark={over} -> {(over ? "default" : "HTTRANSPARENT")}");
            }
            if (!over) return new IntPtr(Native.HTTRANSPARENT);
        }
        return Native.DefSubclassProc(hWnd, uMsg, wParam, lParam);
    }

    private void OnTick()
    {
        // Remember the last foreground window that is not the rail (to tell whether a mark click stole it).
        var fg = Native.GetForegroundWindow();
        if (fg != _hwnd && fg != IntPtr.Zero && fg != _lastForeignFg)
        {
            _lastForeignFg = fg;
        }
        if (_variant != "layered" && _variant != "all") return;

        Native.GetCursorPos(out var c);
        Native.GetWindowRect(_hwnd, out var wr);
        bool inside = c.X >= wr.Left && c.X < wr.Right && c.Y >= wr.Top && c.Y < wr.Bottom;
        bool over = inside && OverMark(c.X, c.Y);
        bool wantTransparent = !over;
        if (wantTransparent == _transparentNow) return;
        long ex = (long)Native.GetWindowLongPtr(_hwnd, Native.GWL_EXSTYLE);
        ex = wantTransparent ? (ex | Native.WS_EX_TRANSPARENT) : (ex & ~Native.WS_EX_TRANSPARENT);
        Native.SetWindowLongPtr(_hwnd, Native.GWL_EXSTYLE, new IntPtr(ex));
        _transparentNow = wantTransparent;
        if (_toggleLogs < 20)
        {
            _toggleLogs++;
            SpikeLog.Append($"layered toggle: cursor ({c.X},{c.Y}) overMark={over} -> WS_EX_TRANSPARENT {(wantTransparent ? "ON (click-through)" : "OFF (hit-testable)")} exstyle={Ui.ExStyleBits((long)Native.GetWindowLongPtr(_hwnd, Native.GWL_EXSTYLE))}");
        }
    }

    private void OnMark(int idx)
    {
        var fgNow = Native.GetForegroundWindow();
        bool railIsFg = fgNow == _hwnd;
        bool changed = fgNow != _lastForeignFg;
        SpikeLog.Append($"mark {idx} clicked; foreground before={Ui.Describe(_lastForeignFg)} now={Ui.Describe(fgNow)} railIsForeground={railIsFg} foregroundChanged={changed}");
        _marks[idx - 1].Opacity = _marks[idx - 1].Opacity > 0.6 ? 0.4 : 1.0;
        // Re-check shortly after: activation can be deferred past the pointer event.
        var t = _rail.DispatcherQueue.CreateTimer();
        t.Interval = TimeSpan.FromMilliseconds(250);
        t.IsRepeating = false;
        t.Tick += (s, e) =>
        {
            var fg2 = Native.GetForegroundWindow();
            SpikeLog.Append($"mark {idx} +250ms: foreground={Ui.Describe(fg2)} railIsForeground={fg2 == _hwnd} exstyle={Ui.ExStyleBits((long)Native.GetWindowLongPtr(_hwnd, Native.GWL_EXSTYLE))}");
        };
        t.Start();
    }
}
