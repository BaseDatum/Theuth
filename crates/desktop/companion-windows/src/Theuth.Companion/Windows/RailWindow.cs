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
/// The pre-149 single-strip rail, kept as design 149 §3.8's escape hatch (Settings › Rail › "Separate pills" off; recheck N4;
/// restored from <c>b84905dd~1</c> with the peek card gone — an agent mark opens the agents panel, as the pills do). Built
/// alongside <see cref="RailPillsWindow"/> and shown only while the toggle is off (<see cref="Suspend"/> / <see cref="Resume"/>).
/// <para>Design 134 §4.1, D4: a <see cref="ToolWindow"/> 26 epx wide at the right (or left) edge of the primary
/// display's work area, vertically centred, sized to its marks — wordmark · count · needs-you marks · hairline · agent marks
/// · <c>+N</c>.</para> <c>WS_EX_NOACTIVATE | WS_EX_LAYERED | WS_EX_TRANSPARENT</c> before the first show, shown with
/// <c>AppWindow.Show(false)</c>, capture-excluded unless <c>--shareable</c>.
/// <para><b>Pass-through</b>: a 40 ms cursor poll toggles <c>WS_EX_TRANSPARENT</c> — off over a mark, on elsewhere — so
/// clicks beside the marks reach the window underneath (spike S1). The same poll records the last foreign foreground window
/// and expands the 4 × 120 epx sliver while the pointer is over it.</para>
/// <para><b>Focus hand-back</b>: the WinUI island activates the window on every press despite <c>WS_EX_NOACTIVATE</c> (S1),
/// so on <c>WM_ACTIVATE</c> a press on a mark that opens the panel (needs-you, finished) keeps the activation — the panel takes
/// the foreground anyway — and every other press (an agent mark → the agents panel, the hairline, the wordmark) hands the
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
    private bool suspended;

    /// <summary>Settings › Rail › Separate pills turned on: hide the strip and stop its poll (the pills take over).</summary>
    public void Suspend()
    {
        if (suspended) return;
        suspended = true;
        poll.Stop();
        quietTimer?.Stop();
        quietTimer = null;
        HideWindow();
        Log.Write("rail strip suspended (pills on)");
    }

    /// <summary>Settings › Rail › Separate pills turned off: the strip comes back where the pills were.</summary>
    public void Resume()
    {
        if (!suspended) return;
        suspended = false;
        Refresh();
        ScheduleCollapse(afterHover: false);
        ShowPassive();
        poll.Start();
        DispatcherQueue.TryEnqueue(DispatcherQueuePriority.Low, Layout);
        Log.Write($"rail strip shown: rect={Win32.Text(ScreenRect)} presence={Presence.RawValue()} side={Side}");
    }

    public bool Suspended => suspended;

    public RailWindow(Store store, RailPresence presence, string side, bool visible)
        : base("theuth rail strip", WINDOW_EX_STYLE.WS_EX_NOACTIVATE | WINDOW_EX_STYLE.WS_EX_LAYERED | WINDOW_EX_STYLE.WS_EX_TRANSPARENT,
               excludeFromCapture: !Flags.Shareable)
    {
        this.store = store;
        suspended = !visible;
        Presence = presence;
        Side = side == "left" ? "left" : "right";
        animate = SystemEffects.AnimationsOn;
        // W6 item 2 (A7): Thin acrylic on the layered rail window was TESTED on build-B (as-built W6 §2) and does not work —
        // the controller paints no material (the fallback colour, black over a white canvas) and attaching it drops the window
        // out of the topmost band (it hid behind a maximized Paint and an activated Notepad). The rail stays W2's opaque
        // surface; `--rail-acrylic` re-enables the controller for a retest on a newer Windows App SDK.
        wantBackdrop = Environment.GetCommandLineArgs().Contains("--rail-acrylic");
        root = new Grid { HorizontalAlignment = HorizontalAlignment.Stretch, VerticalAlignment = VerticalAlignment.Top };
        AutomationProperties.SetAutomationId(root, "rail");
        AutomationProperties.SetName(root, "theuth rail");
        FlatButtons.Apply(root.Resources);
        Content = root;
        ApplyEffects(initial: true);
        // 149 NATIVE: follow Windows light/dark live (the material's theme and the content together).
        Theme.Follow(root, () =>
        {
            backdrop?.SetTheme(root.ActualTheme);
            Refresh();
        });
        if (SystemEffects.Current is { } fx) fx.Changed += () => ApplyEffects(initial: false);
        store.PropertyChanged += OnStoreChanged;
        poll = DispatcherQueue.CreateTimer();
        poll.Interval = TimeSpan.FromMilliseconds(40);
        poll.IsRepeating = true;
        poll.Tick += (_, _) => Tick();
        Render();
        Layout();
        ScheduleCollapse(afterHover: false); // `Always a sliver`: a sliver from the first frame
        if (suspended)
        {
            Log.Write("rail strip built (suspended: Settings › Rail › Separate pills is on)");
            return;
        }
        ShowPassive();
        poll.Start();
        DispatcherQueue.TryEnqueue(DispatcherQueuePriority.Low, Layout); // measured again once the tree is live
        Log.Write($"rail strip shown: rect={Win32.Text(ScreenRect)} exstyle={Win32.ExStyleText(Win32.ExStyle(Hwnd))} affinity=0x{Win32.DisplayAffinity(Hwnd):X} presence={Presence.RawValue()} side={Side} animations={(animate ? "on" : "off")} backdrop={BackdropText}");
    }

    /// <summary>"acrylic-thin" while the material is attached (<c>--rail-acrylic</c> only); "opaque" otherwise (the default, transparency off, unsupported).</summary>
    public string BackdropText => backdrop?.Material ?? "opaque";

    protected override void OnClosingForGood()
    {
        backdrop?.Dispose();
        backdrop = null;
    }

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
            backdrop = Backdrop.Attach(this, BackdropKind.AcrylicThin, Theme.ColorOf(NativeLook.SolidBackgroundBase, root.ActualTheme), root.ActualTheme, "rail");
        }
        else if ((!transparency || !wantBackdrop) && backdrop is not null)
        {
            backdrop.Dispose();
            backdrop = null;
            Log.Write("rail: backdrop detached — solid SolidBackgroundFillColorBase surface (transparency effects off)");
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

    /// <summary>Every poll (40 ms), after the bookkeeping.</summary>
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
        backdrop?.SetTheme(theme);
        root.Children.Clear();
        views.Clear();
        rectsDirty = true;
        if (collapsed)
        {
            // The sliver: a quiet tertiary-text mark (149 NATIVE), the same with or without the acrylic.
            root.Background = Theme.Brush(NativeLook.TextTertiary, theme);
            var sliver = new Border { Height = SliverHeight, Background = Theme.Transparent };
            AutomationProperties.SetAutomationId(sliver, "rail-sliver");
            AutomationProperties.SetName(sliver, "theuth rail, quiet");
            root.Children.Add(sliver);
            return;
        }
        // 149 NATIVE: nothing painted over the Thin acrylic (134 W6's 82 % tint is gone); SolidBackgroundFillColorBase when there is none.
        root.Background = backdrop is null ? Theme.Brush(NativeLook.SolidBackgroundBase, theme) : Theme.Transparent;
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
                Text = $"{rail.NeedsYouCount}", FontSize = 12, FontWeight = Microsoft.UI.Text.FontWeights.SemiBold,
                Foreground = Theme.Brush(NativeLook.AccentText, theme), // R1: the count is text
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
            Background = Theme.Brush(NativeLook.Divider, theme),
        });
        foreach (var m in rail.Agents) stack.Children.Add(MarkButton(m, ++n, theme));
        if (rail.Overflow > 0)
        {
            stack.Children.Add(new TextBlock
            {
                Text = $"+{rail.Overflow}", FontSize = 12,
                Foreground = Theme.Brush(NativeLook.TextSecondary, theme), HorizontalAlignment = HorizontalAlignment.Center,
            });
        }
        if (store.Connection != RpcClient.State.Open)
        {
            var dot = new Microsoft.UI.Xaml.Shapes.Ellipse
            {
                Width = 4, Height = 4, Fill = Theme.Brush(NativeLook.Critical, theme), HorizontalAlignment = HorizontalAlignment.Center,
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
            HorizontalContentAlignment = HorizontalAlignment.Center, Background = Theme.Transparent, MinWidth = 0, MinHeight = 0,
            UseSystemFocusVisuals = false, IsTabStop = false,
        };
        AutomationProperties.SetAutomationId(b, $"mark-{n}");
        AutomationProperties.SetName(b, m.Detail);
        ToolTipService.SetToolTip(b, m.Detail);
        b.Click += (_, _) =>
        {
            CancelDeferredHandBack(); // every mark opens a panel now (the agents panel for agent marks): the panel takes the foreground
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

    /// <summary>Opens the Attention panel when clicked: needs-you and finished marks (they carry an item); other marks open the agents panel.</summary>
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
        if (m is not null)
        {
            Log.Write($"rail strip: press on {m.Label} ({m.State.Kind}) — the panel takes the foreground (no hand-back)");
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