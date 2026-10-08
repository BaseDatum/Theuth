using System.ComponentModel;
using Microsoft.UI.Dispatching;
using Microsoft.UI.Windowing;
using Microsoft.UI.Xaml;
using Microsoft.UI.Xaml.Automation;
using Microsoft.UI.Xaml.Controls;
using Microsoft.UI.Xaml.Media;
using Theuth.Companion.Core;
using Theuth.Companion.Interop;
using Theuth.Companion.Rpc;
using Windows.Graphics;
using Windows.Win32;
using Windows.Win32.Foundation;
using Windows.Win32.UI.WindowsAndMessaging;

namespace Theuth.Companion.Ui;

/// <summary>
/// The rail (design 134 §4.1, D4): a <see cref="ToolWindow"/> 26 epx wide at the right (or left) edge of the primary
/// display's work area, vertically centred, sized to its marks — wordmark · count · needs-you marks · hairline · agent marks
/// · <c>+N</c>. <c>WS_EX_NOACTIVATE | WS_EX_LAYERED | WS_EX_TRANSPARENT</c> before the first show, shown with
/// <c>AppWindow.Show(false)</c>, capture-excluded unless <c>--shareable</c>.
/// <para><b>Pass-through</b>: a 40 ms cursor poll toggles <c>WS_EX_TRANSPARENT</c> — off over a mark, on elsewhere — so
/// clicks beside the marks reach the window underneath (spike S1). The same poll records the last foreign foreground window
/// and expands the 4 × 120 epx sliver while the pointer is over it.</para>
/// <para><b>Focus hand-back</b>: the WinUI island activates the window on every press despite <c>WS_EX_NOACTIVATE</c> (S1),
/// so on <c>WM_ACTIVATE</c> a press on a mark that opens the panel (needs-you, finished) keeps the activation — the panel takes
/// the foreground anyway — and every other press (an agent mark → the peek card, the hairline, the wordmark) hands the
/// foreground straight back to the recorded window.</para>
/// UIA: root <c>rail</c>, marks <c>mark-&lt;n&gt;</c> (1-based, display order) named "&lt;agent&gt;, &lt;state&gt;, &lt;since&gt;",
/// <c>needs-you-count</c>, <c>rail-sliver</c>.
/// </summary>
internal sealed partial class RailWindow : ToolWindow
{
    public const double RailWidth = RailCollapse.Width;
    public const double SliverWidth = RailCollapse.SliverWidth;
    private const double SliverHeight = 120;
    private const double MarkRowHeight = 16;

    private readonly Store store;
    private readonly Grid root;
    private readonly DispatcherQueueTimer poll;
    private readonly bool wantBackdrop;
    private bool animate;
    private Backdrop? backdrop;
    private readonly List<(Mark Mark, FrameworkElement View)> views = new();
    private readonly List<(RECT Rel, Mark Mark)> hits = new();
    private DispatcherQueueTimer? quietTimer;
    private bool collapsed;
    private bool hovering;
    private bool transparentNow = true;
    private bool rectsDirty = true;
    private HWND lastSeen;
    private int toggleLogs;

    public RailWindow(Store store, RailPresence presence, string side)
        : base("theuth rail", WINDOW_EX_STYLE.WS_EX_NOACTIVATE | WINDOW_EX_STYLE.WS_EX_LAYERED | WINDOW_EX_STYLE.WS_EX_TRANSPARENT,
               excludeFromCapture: !Flags.Shareable)
    {
        this.store = store;
        Presence = presence;
        Side = side == "left" ? "left" : "right";
        animate = SystemEffects.AnimationsOn;
        // W6 item 2 (A7): Thin acrylic on the layered rail window; `--no-rail-acrylic` keeps W2's opaque surface (the A/B switch).
        wantBackdrop = !Environment.GetCommandLineArgs().Contains("--no-rail-acrylic");
        root = new Grid { HorizontalAlignment = HorizontalAlignment.Stretch, VerticalAlignment = VerticalAlignment.Top };
        AutomationProperties.SetAutomationId(root, "rail");
        AutomationProperties.SetName(root, "theuth rail");
        FlatButtons.Apply(root.Resources);
        Content = root;
        ApplyEffects(initial: true);
        root.ActualThemeChanged += (_, _) =>
        {
            backdrop?.SetTheme(root.ActualTheme);
            Refresh();
        };
        if (SystemEffects.Current is { } fx) fx.Changed += () => ApplyEffects(initial: false);
        store.PropertyChanged += OnStoreChanged;
        poll = DispatcherQueue.CreateTimer();
        poll.Interval = TimeSpan.FromMilliseconds(40);
        poll.IsRepeating = true;
        poll.Tick += (_, _) => Tick();
        Render();
        Layout();
        ScheduleCollapse(afterHover: false); // `Always a sliver`: a sliver from the first frame
        ShowPassive();
        poll.Start();
        DispatcherQueue.TryEnqueue(DispatcherQueuePriority.Low, Layout); // measured again once the tree is live
        Log.Write($"rail shown: rect={Win32.Text(ScreenRect)} exstyle={Win32.ExStyleText(Win32.ExStyle(Hwnd))} affinity=0x{Win32.DisplayAffinity(Hwnd):X} presence={Presence.RawValue()} side={Side} animations={(animate ? "on" : "off")} backdrop={BackdropText}");
    }

    /// <summary>"acrylic-thin" while the material is attached; "opaque" (transparency off, unsupported, or --no-rail-acrylic).</summary>
    public string BackdropText => backdrop?.Material ?? "opaque";

    /// <summary>
    /// C13 live (W6 item 3): "Transparency effects" decides the backdrop (attach / detach, the surface goes opaque),
    /// "Animation effects" decides the working beat and the window transitions; re-rendered on every change.
    /// </summary>
    private void ApplyEffects(bool initial)
    {
        bool transparency = SystemEffects.TransparencyOn;
        bool animations = SystemEffects.AnimationsOn;
        if (wantBackdrop && transparency && backdrop is null)
        {
            backdrop = Backdrop.Attach(this, BackdropKind.AcrylicThin, Palette.ColorOf(Palette.Surface, root.ActualTheme), root.ActualTheme, "rail");
        }
        else if ((!transparency || !wantBackdrop) && backdrop is not null)
        {
            backdrop.Dispose();
            backdrop = null;
            Log.Write("rail: backdrop detached — opaque surface (transparency effects off)");
        }
        SystemEffects.ApplyTransitions(Hwnd, animations);
        if (initial) { animate = animations; return; }
        animate = animations;
        Log.Write($"rail: effects applied — backdrop={BackdropText} animations={(animations ? "on" : "off")}");
        Refresh();
    }

    public RailPresence Presence { get; private set; }

    public string Side { get; private set; }

    public bool Collapsed => collapsed;

    /// <summary>The last foreground window that is not the companion's (the hand-back target, the panel's "previous").</summary>
    public HWND LastForeign { get; private set; }

    /// <summary><see cref="LastForeign"/> without the taskbar and the shell's popups: where Esc returns after a tray click.</summary>
    public HWND LastUserApp { get; private set; }

    /// <summary>A mark was clicked (pointer or UIA invoke): the mark and its row on screen (physical px).</summary>
    public event Action<Mark, RECT>? MarkClicked;

    /// <summary>Every poll (40 ms), after the bookkeeping: the peek card's outside-click check rides on it.</summary>
    public event Action? Polled;

    public int MarkCount => views.Count;

    /// <summary>Settings › Rail changed (W3): apply live.</summary>
    public void Apply(RailPresence presence, string side)
    {
        Presence = presence;
        Side = side == "left" ? "left" : "right";
        quietTimer?.Stop();
        quietTimer = null;
        if (collapsed && (presence == RailPresence.Expanded || (presence == RailPresence.SliverWhenQuiet && !store.Rail.Quiet))) SetCollapsed(false);
        ScheduleCollapse(afterHover: false);
        Refresh();
    }

    private void OnStoreChanged(object? sender, PropertyChangedEventArgs e)
    {
        if (e.PropertyName is not (nameof(Store.Rail) or nameof(Store.Connection))) return;
        if (!store.Rail.Quiet && collapsed && RailCollapse.ExpandsOnActivity(Presence)) SetCollapsed(false);
        ScheduleCollapse(afterHover: false);
        Refresh();
    }

    public void Refresh()
    {
        Render();
        Layout();
    }

    private void Render()
    {
        var theme = root.ActualTheme;
        root.Children.Clear();
        views.Clear();
        rectsDirty = true;
        if (collapsed)
        {
            // The sliver's sand tint rides on the acrylic when there is one, on the opaque surface otherwise.
            root.Background = backdrop is null ? Palette.Tint(Palette.Sand, theme, 0.55) : Palette.Tint(Palette.Sand, theme, 0.45);
            var sliver = new Border { Height = SliverHeight, Background = Palette.Transparent };
            AutomationProperties.SetAutomationId(sliver, "rail-sliver");
            AutomationProperties.SetName(sliver, "theuth rail, quiet");
            root.Children.Add(sliver);
            return;
        }
        root.Background = backdrop is null ? Palette.Brush(Palette.Surface, theme) : Palette.Transparent;
        var rail = store.Rail;
        var stack = new StackPanel { Spacing = 6, Padding = new Thickness(0, 10, 0, 10), HorizontalAlignment = HorizontalAlignment.Stretch };
        var word = MarkGlyph.Wordmark(theme);
        word.HorizontalAlignment = HorizontalAlignment.Center;
        if (store.Connection != RpcClient.State.Open) word.Opacity = 0.45; // the dim wordmark while the engine is away (§5.5)
        stack.Children.Add(word);
        if (rail.NeedsYouCount > 0)
        {
            var count = new TextBlock
            {
                Text = $"{rail.NeedsYouCount}", FontSize = 9, FontWeight = Microsoft.UI.Text.FontWeights.SemiBold,
                FontFamily = new FontFamily("Consolas"), Foreground = Palette.Brush(Palette.Gold, theme),
                HorizontalAlignment = HorizontalAlignment.Center,
            };
            AutomationProperties.SetAutomationId(count, "needs-you-count");
            AutomationProperties.SetName(count, rail.NeedsYouCount == 1 ? "1 needs you" : $"{rail.NeedsYouCount} need you");
            stack.Children.Add(count);
        }
        int n = 0;
        foreach (var m in rail.NeedsYou) stack.Children.Add(MarkButton(m, ++n, theme));
        stack.Children.Add(new Border
        {
            Width = 12, Height = 1, Margin = new Thickness(0, 2, 0, 2), HorizontalAlignment = HorizontalAlignment.Center,
            Background = Palette.Tint(Palette.Ink, theme, 0.18),
        });
        foreach (var m in rail.Agents) stack.Children.Add(MarkButton(m, ++n, theme));
        if (rail.Overflow > 0)
        {
            stack.Children.Add(new TextBlock
            {
                Text = $"+{rail.Overflow}", FontSize = 9, FontFamily = new FontFamily("Consolas"),
                Foreground = Palette.Brush(Palette.Ink2, theme), HorizontalAlignment = HorizontalAlignment.Center,
            });
        }
        if (store.Connection != RpcClient.State.Open)
        {
            var dot = new Microsoft.UI.Xaml.Shapes.Ellipse
            {
                Width = 4, Height = 4, Fill = Palette.Brush(Palette.Carnelian, theme), HorizontalAlignment = HorizontalAlignment.Center,
            };
            var state = store.Connection.ToString().ToLowerInvariant();
            ToolTipService.SetToolTip(dot, $"engine: {state}");
            AutomationProperties.SetName(dot, $"engine: {state}");
            stack.Children.Add(dot);
        }
        root.Children.Add(stack);
    }

    private Button MarkButton(Mark m, int n, ElementTheme theme)
    {
        var glyph = MarkGlyph.Create(m.State, theme, animate);
        glyph.HorizontalAlignment = HorizontalAlignment.Center;
        glyph.VerticalAlignment = VerticalAlignment.Center;
        var b = new Button
        {
            Content = glyph, Height = MarkRowHeight, Padding = new Thickness(0), BorderThickness = new Thickness(0),
            HorizontalAlignment = HorizontalAlignment.Stretch, VerticalAlignment = VerticalAlignment.Center,
            HorizontalContentAlignment = HorizontalAlignment.Center, Background = Palette.Transparent, MinWidth = 0, MinHeight = 0,
            UseSystemFocusVisuals = false, IsTabStop = false,
        };
        AutomationProperties.SetAutomationId(b, $"mark-{n}");
        AutomationProperties.SetName(b, m.Detail);
        ToolTipService.SetToolTip(b, m.Detail);
        b.Click += (_, _) =>
        {
            if (OpensPanel(m)) CancelDeferredHandBack();
            MarkClicked?.Invoke(m, ScreenRectOf(b));
        };
        views.Add((m, b));
        return b;
    }

    /// <summary>Sizes to the content (≥ 60 epx; the sliver 4 × 120) at the configured edge of the primary work area.</summary>
    public void Layout()
    {
        double scale = Scale;
        double wEpx = collapsed ? SliverWidth : RailWidth;
        double hEpx = SliverHeight;
        if (!collapsed)
        {
            root.Measure(new global::Windows.Foundation.Size(wEpx, double.PositiveInfinity));
            hEpx = Math.Max(60, root.DesiredSize.Height);
        }
        var wa = DisplayArea.Primary.WorkArea;
        int w = Math.Max(1, (int)Math.Round(wEpx * scale));
        int h = Math.Min(wa.Height, Math.Max(1, (int)Math.Round(hEpx * scale)));
        int x = Side == "left" ? wa.X : wa.X + wa.Width - w;
        int y = wa.Y + (wa.Height - h) / 2;
        Place(new RectInt32(x, y, w, h));
        rectsDirty = true;
    }

    private void SetCollapsed(bool c)
    {
        quietTimer?.Stop();
        quietTimer = null;
        if (collapsed == c) return;
        collapsed = c;
        Refresh();
    }

    /// <summary>
    /// <see cref="RailCollapse.CollapseDelay"/> decides; one quiet window per quiet spell (a running timer is left alone unless
    /// the pointer just left), and a rail that stops being quiet cancels it.
    /// </summary>
    private void ScheduleCollapse(bool afterHover)
    {
        if (collapsed) return;
        var delay = RailCollapse.CollapseDelay(Presence, store.Rail.Quiet, hovering, afterHover);
        if (delay is not { } d)
        {
            quietTimer?.Stop();
            quietTimer = null;
            return;
        }
        if (quietTimer is not null && !afterHover) return;
        quietTimer?.Stop();
        quietTimer = null;
        if (d <= 0)
        {
            SetCollapsed(true);
            return;
        }
        var t = DispatcherQueue.CreateTimer();
        t.Interval = TimeSpan.FromSeconds(d);
        t.IsRepeating = false;
        t.Tick += (_, _) =>
        {
            if (quietTimer == t) SetCollapsed(true);
        };
        quietTimer = t;
        t.Start();
    }

    /// <summary>A mark's row on screen (physical px): the full rail width × the row.</summary>
    public RECT ScreenRectOf(FrameworkElement e)
    {
        var wr = ScreenRect;
        if (Rel(e) is not { } rel) return wr;
        return new RECT { left = wr.left + rel.left, top = wr.top + rel.top, right = wr.left + rel.right, bottom = wr.top + rel.bottom };
    }

    private RECT? Rel(FrameworkElement e)
    {
        if (e.ActualHeight <= 0) return null;
        try
        {
            var p = e.TransformToVisual(root).TransformPoint(new global::Windows.Foundation.Point(0, 0));
            double s = Scale;
            var wr = ScreenRect;
            return new RECT
            {
                left = 0, right = wr.right - wr.left,
                top = (int)Math.Floor(p.Y * s), bottom = (int)Math.Ceiling((p.Y + e.ActualHeight) * s),
            };
        }
        catch (Exception)
        {
            return null;
        }
    }

    private void ComputeRects()
    {
        hits.Clear();
        bool all = true;
        foreach (var (m, v) in views)
        {
            if (Rel(v) is { } r) hits.Add((r, m));
            else all = false;
        }
        rectsDirty = !all;
    }

    /// <summary>The mark under the cursor, if any.</summary>
    public Mark? MarkAt(System.Drawing.Point c)
    {
        var wr = ScreenRect;
        foreach (var (rel, m) in hits)
        {
            var r = new RECT { left = wr.left + rel.left, top = wr.top + rel.top, right = wr.left + rel.right, bottom = wr.top + rel.bottom };
            if (Win32.Contains(r, c)) return m;
        }
        return null;
    }

    /// <summary>Opens the panel when clicked: needs-you and finished marks (they carry an item).</summary>
    public static bool OpensPanel(Mark m) => m.ItemId is not null;

    private void Tick()
    {
        var fg = PInvoke.GetForegroundWindow();
        if (fg != lastSeen)
        {
            lastSeen = fg;
            if (!fg.IsNull && !Win32.IsOurs(fg))
            {
                LastForeign = fg;
                if (!Win32.IsShellChrome(fg)) LastUserApp = fg;
            }
        }
        var c = Win32.Cursor();
        var wr = ScreenRect;
        bool inside = Win32.Contains(Win32.Inflate(wr, 2), c);
        if (inside != hovering)
        {
            hovering = inside;
            if (inside)
            {
                quietTimer?.Stop();
                quietTimer = null;
                if (collapsed) SetCollapsed(false);
            }
            else
            {
                ScheduleCollapse(afterHover: true);
            }
        }
        if (rectsDirty) ComputeRects();
        bool over = inside && MarkAt(c) is not null;
        bool wantTransparent = !over;
        if (wantTransparent != transparentNow)
        {
            var ex = Win32.ExStyle(Hwnd);
            ex = wantTransparent ? ex | WINDOW_EX_STYLE.WS_EX_TRANSPARENT : ex & ~WINDOW_EX_STYLE.WS_EX_TRANSPARENT;
            Win32.SetExStyle(Hwnd, ex);
            transparentNow = wantTransparent;
            if (toggleLogs < 6)
            {
                toggleLogs++;
                Log.Write($"rail pass-through: cursor ({c.X},{c.Y}) overMark={(over ? "true" : "false")} -> WS_EX_TRANSPARENT {(wantTransparent ? "on" : "off")}");
            }
        }
        Polled?.Invoke();
    }

    protected override void OnActivate(bool active, HWND other)
    {
        if (!active)
        {
            deferred?.Stop();
            deferred = null;
            return;
        }
        var c = Win32.Cursor();
        if (!Win32.Contains(Win32.Inflate(ScreenRect, 2), c))
        {
            // Not a press inside the rail (UI Automation's SetFocus before an Invoke, the keyboard): the Click that may follow
            // decides — a mark that opens the panel cancels this; anything else gets the foreground back in 150 ms.
            var target = LastForeign;
            deferred?.Stop();
            var t = DispatcherQueue.CreateTimer();
            t.Interval = TimeSpan.FromMilliseconds(150);
            t.IsRepeating = false;
            t.Tick += (_, _) =>
            {
                t.Stop();
                if (deferred != t) return;
                deferred = null;
                if (!IsForeground) return;
                bool done = Win32.Foreground(target);
                Log.Write($"rail: activated without a press (automation/keyboard) — hand-back to {Win32.Describe(target)} -> {(done ? "true" : "false")}");
            };
            deferred = t;
            t.Start();
            return;
        }
        var m = MarkAt(c);
        if (m is not null && OpensPanel(m))
        {
            Log.Write($"rail: press on {m.Label} ({m.State.Kind}) — the panel takes the foreground (no hand-back)");
            return;
        }
        var back = LastForeign;
        bool ok = Win32.Foreground(back);
        Log.Write($"rail: activated by a press on {(m is null ? "nothing" : $"agent mark {m.Label}")} — hand-back to {Win32.Describe(back)} -> {(ok ? "true" : "false")}; foreground now {Win32.Describe(PInvoke.GetForegroundWindow())}");
    }

    private DispatcherQueueTimer? deferred;

    /// <summary>A panel-opening mark was clicked: the pending automation hand-back must not take the foreground away.</summary>
    private void CancelDeferredHandBack()
    {
        deferred?.Stop();
        deferred = null;
    }

    /// <summary>The shell's WM_CLOSE goes to the host window (class <c>TheuthCompanionRail</c>); a close of the rail itself is ignored.</summary>
    protected override void OnCloseRequested() { }

    public void Stop()
    {
        poll.Stop();
        quietTimer?.Stop();
        store.PropertyChanged -= OnStoreChanged;
    }
}

/// <summary>Lightweight styling: buttons without hover/press chrome (the rail's marks, the panel's lines).</summary>
internal static class FlatButtons
{
    private static readonly string[] Keys =
    {
        "ButtonBackground", "ButtonBackgroundPointerOver", "ButtonBackgroundPressed", "ButtonBackgroundDisabled",
        "ButtonBorderBrush", "ButtonBorderBrushPointerOver", "ButtonBorderBrushPressed", "ButtonBorderBrushDisabled",
    };

    public static void Apply(ResourceDictionary resources, Brush? hover = null)
    {
        foreach (var k in Keys)
        {
            resources[k] = k is "ButtonBackgroundPointerOver" or "ButtonBackgroundPressed" && hover is not null ? hover : Palette.Transparent;
        }
    }
}
