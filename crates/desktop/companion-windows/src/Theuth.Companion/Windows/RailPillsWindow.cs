using System.ComponentModel;
using Microsoft.UI.Dispatching;
using Microsoft.UI.Windowing;
using Microsoft.UI.Xaml;
using Microsoft.UI.Xaml.Automation;
using Microsoft.UI.Xaml.Controls;
using Microsoft.UI.Xaml.Input;
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
/// 149 WW2 — the rail as separate pills (design 149 §3.1, D1; Windows form §7.4 #2): one <see cref="ToolWindow"/> at the
/// right (or left) edge of the primary work area, vertically centred, holding four opaque capsules — <b>inbox</b> (wordmark with
/// the gold count beneath it, inside the capsule) · <b>agents</b> (one 12 epx dot per live agent, ≤ 12 then +N; hover grows a dot 1.4×) · <b>actions</b>
/// (ask · mic with carnelian level bars while a dictation records · point) · <b>more</b> (···) — 52 epx wide with 12 epx gaps.
/// The window is shaped with <c>SetWindowRgn</c> to the union of the capsules, so the gaps are not part of the
/// window: nothing is painted there and clicks pass through. Each pill carries a 1 epx hairline instead of a DWM shadow
/// (a regioned window has none — as-built WW2 §Deviations).
/// <para>W6's mechanics are kept: <c>WS_EX_NOACTIVATE | WS_EX_LAYERED | WS_EX_TRANSPARENT</c>, the 40 ms cursor poll that
/// flips <c>WS_EX_TRANSPARENT</c> off over a pill and on elsewhere, the sliver (4 × 120) under the rail presence rule, and
/// the <b>focus hand-back</b>: a press that opens a panel (inbox, a dot, ask) keeps the activation; any other press hands the
/// foreground straight back to the last foreign window.</para>
/// UIA: root <c>rail</c>, <c>pill-inbox</c> · <c>pill-agents</c> · <c>pill-actions</c> · <c>pill-more</c>, <c>needs-you-count</c>,
/// <c>dot-&lt;n&gt;</c> (1-based), <c>act-ask</c> · <c>act-mic</c> · <c>act-point</c>, <c>rail-sliver</c>.
/// </summary>
internal sealed partial class RailPillsWindow : ToolWindow
{
    public const double SliverWidth = RailCollapse.SliverWidth;
    private const double SliverHeight = 120;
    private const double Margin = RailPillsModel.Margin;

    private readonly Store store;
    private readonly Grid root;
    private readonly DispatcherQueueTimer poll;
    private bool animate;
    private readonly List<(FrameworkElement View, object Tag)> hits = new();
    private readonly List<(RECT Rel, object Tag)> rects = new();
    private readonly List<(FrameworkElement View, string Id)> dotViews = new();
    private DispatcherQueueTimer? quietTimer;
    private bool collapsed;
    private bool hovering;
    private bool transparentNow = true;
    private bool rectsDirty = true;
    private HWND lastSeen;
    private int toggleLogs;
    private string? hoveredDot;
    private RailPills pills = RailPills.Empty;
    private Panel? micSlot;
    private Border? inboxPill;
    private TextBlock? badge; // the gold count beneath the wordmark (N1)
    private bool lastRecording;

    public RailPillsWindow(Store store, RailPresence presence, string side, bool visible = true)
        : base("theuth rail", WINDOW_EX_STYLE.WS_EX_NOACTIVATE | WINDOW_EX_STYLE.WS_EX_LAYERED | WINDOW_EX_STYLE.WS_EX_TRANSPARENT,
               excludeFromCapture: !Flags.Shareable)
    {
        this.store = store;
        suspended = !visible;
        Presence = presence;
        Side = side == "left" ? "left" : "right";
        animate = SystemEffects.AnimationsOn;
        root = new Grid { HorizontalAlignment = HorizontalAlignment.Stretch, VerticalAlignment = VerticalAlignment.Top, Background = Theme.Transparent };
        AutomationProperties.SetAutomationId(root, "rail");
        AutomationProperties.SetName(root, "theuth rail");
        FlatButtons.Apply(root.Resources);
        Content = root;
        SystemEffects.ApplyTransitions(Hwnd, animate);
        // 149 NATIVE: follow Windows light/dark live. The pills are one SetWindowRgn-shaped window (capsules with transparent gaps):
        // a regioned window cannot host Mica/Acrylic, so each pill is painted SolidBackgroundFillColorBase with a CardStroke hairline.
        Theme.Follow(root, Refresh);
        if (SystemEffects.Current is { } fx) fx.Changed += () =>
        {
            animate = SystemEffects.AnimationsOn;
            SystemEffects.ApplyTransitions(Hwnd, animate);
            Refresh();
        };
        store.PropertyChanged += OnStoreChanged;
        store.DictationChanged += OnDictation;
        poll = DispatcherQueue.CreateTimer();
        poll.Interval = TimeSpan.FromMilliseconds(40);
        poll.IsRepeating = true;
        poll.Tick += (_, _) => Tick();
        Render();
        Layout();
        ScheduleCollapse(afterHover: false);
        if (suspended)
        {
            Log.Write("rail pills built (suspended: Settings › Rail › Separate pills is off — the strip is shown)");
            return;
        }
        ShowPassive();
        poll.Start();
        DispatcherQueue.TryEnqueue(DispatcherQueuePriority.Low, Layout);
        Log.Write($"rail pills shown: rect={Win32.Text(ScreenRect)} exstyle={Win32.ExStyleText(Win32.ExStyle(Hwnd))} affinity=0x{Win32.DisplayAffinity(Hwnd):X} presence={Presence.RawValue()} side={Side} animations={(animate ? "on" : "off")} dots={pills.Dots.Count}");
    }

    public RailPresence Presence { get; private set; }

    public string Side { get; private set; }

    private bool suspended;

    /// <summary>149 §3.8 escape hatch (recheck N4): Settings › Rail › Separate pills turned off — hide the pills and stop their poll (the strip takes over).</summary>
    public void Suspend()
    {
        if (suspended) return;
        suspended = true;
        poll.Stop();
        quietTimer?.Stop();
        quietTimer = null;
        HideWindow();
        Log.Write("rail pills suspended (strip on)");
    }

    /// <summary>Separate pills turned back on.</summary>
    public void Resume()
    {
        if (!suspended) return;
        suspended = false;
        Refresh();
        ScheduleCollapse(afterHover: false);
        ShowPassive();
        poll.Start();
        DispatcherQueue.TryEnqueue(DispatcherQueuePriority.Low, Layout);
        Log.Write($"rail pills shown: rect={Win32.Text(ScreenRect)} presence={Presence.RawValue()} side={Side} dots={pills.Dots.Count}");
    }

    public bool Suspended => suspended;

    public bool Collapsed => collapsed;

    public RailPills Pills => pills;

    /// <summary>The last foreground window that is not the companion's (the hand-back target, the panel's "previous").</summary>
    public HWND LastForeign { get; private set; }

    /// <summary><see cref="LastForeign"/> without the taskbar and the shell's popups: where Esc returns after a tray click.</summary>
    public HWND LastUserApp { get; private set; }

    /// <summary>The inbox pill was pressed: open the Attention panel beside it (physical px).</summary>
    public event Action<RECT>? InboxClicked;

    /// <summary>An agent dot was pressed: the agents panel scrolled to its row (D23).</summary>
    public event Action<AgentDot, RECT>? DotClicked;

    /// <summary>The agents pill's empty body was pressed: the agents panel.</summary>
    public event Action<RECT>? AgentsClicked;

    /// <summary>The ask glyph (or the mic) was pressed: the Ask bar.</summary>
    public event Action<RECT>? AskClicked;

    /// <summary>The point/camera glyph was pressed (§3.7 — WW3's point-and-say; WW2 raises, the composition root routes).</summary>
    public event Action? PointClicked;

    /// <summary>The ··· pill was pressed: the status menu as a popup at the pill (screen px).</summary>
    public event Action<System.Drawing.Point>? MoreClicked;

    /// <summary>The pointer entered (session) / left (null) an agent dot: the agents panel highlights that row.</summary>
    public event Action<string?>? DotHovered;

    /// <summary>Every poll (40 ms), after the bookkeeping.</summary>
    public event Action? Polled;

    public int DotCount => dotViews.Count;

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
        if (e.PropertyName is not (nameof(Store.Rail) or nameof(Store.Connection) or nameof(Store.RemoteSessions))) return;
        if (!(store.FixtureRail ?? store.Rail).Quiet && collapsed && RailCollapse.ExpandsOnActivity(Presence)) SetCollapsed(false);
        ScheduleCollapse(afterHover: false);
        Refresh();
    }

    /// <summary>D8: the level bars follow every <c>DictationLevel</c> packet in place (no full re-render at 20 ms).</summary>
    private void OnDictation(AskDictation d, bool take)
    {
        bool recording = d.IsActive;
        if (recording != lastRecording)
        {
            lastRecording = recording;
            Refresh();
            return;
        }
        if (!recording || micSlot is null) return;
        pills = pills with { Levels = d.Levels.ToValueList() };
        micSlot.Children.Clear();
        micSlot.Children.Add(PillGlyphs.LevelBars(pills.Levels, 18, 3, root.ActualTheme));
    }

    public void Refresh()
    {
        Render();
        Layout();
    }

    private RailPills DerivePills() => RailPillsModel.Derive(
        store.FixtureRail ?? store.Rail, store.Connection == RpcClient.State.Open, store.Ask.Dictation.IsActive, store.Ask.Dictation.Levels, hoveredDot, store.RemoteSessions);

    private void Render()
    {
        var theme = root.ActualTheme;
        root.Children.Clear();
        hits.Clear();
        dotViews.Clear();
        micSlot = null;
        inboxPill = null;
        badge = null;
        rectsDirty = true;
        pills = DerivePills();
        lastRecording = pills.Recording;
        if (collapsed)
        {
            root.Margin = new Thickness(0);
            var sliver = new Border
            {
                Width = SliverWidth, Height = SliverHeight, CornerRadius = new CornerRadius(4), Background = Theme.Brush(NativeLook.TextTertiary, theme),
                HorizontalAlignment = Side == "left" ? HorizontalAlignment.Left : HorizontalAlignment.Right,
            };
            AutomationProperties.SetAutomationId(sliver, "rail-sliver");
            AutomationProperties.SetName(sliver, "theuth rail, quiet");
            root.Children.Add(sliver);
            hits.Add((sliver, "sliver"));
            return;
        }
        root.Margin = new Thickness(0);
        var stack = new StackPanel { Spacing = RailPillsModel.Gap, Margin = new Thickness(Margin), HorizontalAlignment = HorizontalAlignment.Center, VerticalAlignment = VerticalAlignment.Top };
        stack.Children.Add(InboxPill(theme));
        stack.Children.Add(AgentsPill(theme));
        stack.Children.Add(ActionsPill(theme));
        stack.Children.Add(MorePill(theme));
        root.Children.Add(stack);
    }

    private Border Pill(ElementTheme theme, double height, string id, string name)
    {
        var b = new Border
        {
            Width = RailPillsModel.PillWidth, Height = height, CornerRadius = new CornerRadius(RailPillsModel.Radius),
            Background = Theme.Brush(NativeLook.SolidBackgroundBase, theme), BorderBrush = Theme.Brush(NativeLook.CardStroke, theme), BorderThickness = new Thickness(1),
        };
        AutomationProperties.SetAutomationId(b, id);
        AutomationProperties.SetName(b, name);
        return b;
    }

    private Button Flat(FrameworkElement content, string id, string name, double w, double h)
    {
        var b = new Button
        {
            Content = content, Width = w, Height = h, Padding = new Thickness(0), BorderThickness = new Thickness(0), Background = Theme.Transparent,
            HorizontalContentAlignment = HorizontalAlignment.Center, VerticalContentAlignment = VerticalAlignment.Center,
            MinWidth = 0, MinHeight = 0, UseSystemFocusVisuals = false, IsTabStop = false, CornerRadius = new CornerRadius(h / 2),
        };
        AutomationProperties.SetAutomationId(b, id);
        AutomationProperties.SetName(b, name);
        ToolTipService.SetToolTip(b, name);
        return b;
    }

    private Border InboxPill(ElementTheme theme)
    {
        var pill = Pill(theme, RailPillsModel.PillWidth, "pill-inbox", RailPillsModel.Describe(RailPillKind.Inbox, pills));
        var grid = new Grid();
        // §3.1: "the wordmark with the gold count beneath" — both inside the capsule (149 recheck N1: a corner badge was cut by the
        // screen edge). With a count the wordmark rises to make room; without, it sits centred.
        var column = new StackPanel { Spacing = 1, HorizontalAlignment = HorizontalAlignment.Center, VerticalAlignment = VerticalAlignment.Center };
        var word = MarkGlyph.Wordmark(theme);
        word.Width = 16;
        word.Height = 16;
        word.HorizontalAlignment = HorizontalAlignment.Center;
        if (!pills.Connected) word.Opacity = 0.45;
        column.Children.Add(word);
        if (pills.InboxGold)
        {
            var n = pills.InboxCount;
            var count = new TextBlock
            {
                Text = n > 99 ? "99+" : $"{n}", FontSize = 12, FontWeight = Microsoft.UI.Text.FontWeights.SemiBold, Foreground = Theme.Brush(NativeLook.AccentText, theme), // R1: the count is text
                HorizontalAlignment = HorizontalAlignment.Center, TextAlignment = TextAlignment.Center,
            };
            AutomationProperties.SetAutomationId(count, "needs-you-count");
            AutomationProperties.SetName(count, n == 1 ? "1 needs you" : $"{n} need you");
            column.Children.Add(count);
            badge = count;
        }
        grid.Children.Add(column);
        if (!pills.Connected)
        {
            var dot = new Microsoft.UI.Xaml.Shapes.Ellipse { Width = 4, Height = 4, Fill = Theme.Brush(NativeLook.Critical, theme), HorizontalAlignment = HorizontalAlignment.Center, VerticalAlignment = VerticalAlignment.Bottom, Margin = new Thickness(0, 0, 0, 5) };
            var state = store.Connection.ToString().ToLowerInvariant();
            AutomationProperties.SetName(dot, $"engine: {state}");
            grid.Children.Add(dot);
        }
        var button = Flat(grid, "pill-inbox-button", RailPillsModel.Describe(RailPillKind.Inbox, pills), RailPillsModel.PillWidth, RailPillsModel.PillWidth);
        button.Click += (_, _) =>
        {
            CancelDeferredHandBack();
            InboxClicked?.Invoke(ScreenRectOf(pill));
        };
        pill.Child = button;
        hits.Add((pill, RailPillKind.Inbox));
        inboxPill = pill;
        return pill;
    }

    private Border AgentsPill(ElementTheme theme)
    {
        int n = pills.Dots.Count;
        var pill = Pill(theme, RailPillsModel.AgentsPillHeight(n, pills.Overflow), "pill-agents", RailPillsModel.Describe(RailPillKind.Agents, pills));
        if (n == 0)
        {
            // Keeps its shape with no agents (§3.1); a press still opens the (empty) agents panel.
            var empty = Flat(new Microsoft.UI.Xaml.Shapes.Ellipse { Width = 6, Height = 6, Stroke = Theme.Brush(NativeLook.TextTertiary, theme), StrokeThickness = 1 },
                "pill-agents-button", "Agents, none running", RailPillsModel.PillWidth, RailPillsModel.PillWidth);
            empty.Click += (_, _) =>
            {
                CancelDeferredHandBack();
                AgentsClicked?.Invoke(ScreenRectOf(pill));
            };
            pill.Child = empty;
            hits.Add((pill, RailPillKind.Agents));
            return pill;
        }
        var col = new StackPanel { Spacing = RailPillsModel.DotGap, Padding = new Thickness(0, RailPillsModel.DotPad, 0, RailPillsModel.DotPad), HorizontalAlignment = HorizontalAlignment.Center, VerticalAlignment = VerticalAlignment.Top };
        int i = 0;
        foreach (var d in pills.Dots)
        {
            i++;
            var glyph = PillGlyphs.AgentDot(d, theme, animate);
            glyph.RenderTransformOrigin = new global::Windows.Foundation.Point(0.5, 0.5);
            var scale = new ScaleTransform();
            glyph.RenderTransform = scale;
            if (hoveredDot == d.Id)
            {
                scale.ScaleX = RailPillsModel.HoverScale;
                scale.ScaleY = RailPillsModel.HoverScale;
            }
            var b = Flat(glyph, $"dot-{i}", d.Detail, RailPillsModel.DotSize, RailPillsModel.DotSize);
            b.Margin = new Thickness(0, -RailPillsModel.DotGap / 2, 0, -RailPillsModel.DotGap / 2); // a taller hit target than the 12 epx dot
            b.Height = RailPillsModel.DotSize + RailPillsModel.DotGap;
            b.Width = RailPillsModel.PillWidth - 8;
            var dot = d;
            b.PointerEntered += (_, _) => HoverDot(dot.Id, dot.Session, scale, true);
            b.PointerExited += (_, _) => HoverDot(dot.Id, dot.Session, scale, false);
            b.Click += (_, _) =>
            {
                CancelDeferredHandBack();
                DotClicked?.Invoke(dot, ScreenRectOf(pill));
            };
            col.Children.Add(b);
            dotViews.Add((b, d.Id));
            hits.Add((b, d));
        }
        if (pills.Overflow > 0)
        {
            col.Children.Add(new TextBlock
            {
                Text = $"+{pills.Overflow}", FontSize = 12, Foreground = Theme.Brush(NativeLook.TextSecondary, theme),
                HorizontalAlignment = HorizontalAlignment.Center, Margin = new Thickness(0, -6, 0, 0),
            });
        }
        pill.Child = col;
        hits.Add((pill, RailPillKind.Agents));
        return pill;
    }

    private void HoverDot(string id, string? session, ScaleTransform scale, bool on)
    {
        double s = RailPillsModel.DotScale(on);
        if (animate)
        {
            var sb = new Microsoft.UI.Xaml.Media.Animation.Storyboard();
            foreach (var prop in new[] { "ScaleX", "ScaleY" })
            {
                var a = new Microsoft.UI.Xaml.Media.Animation.DoubleAnimation { To = s, Duration = new Duration(TimeSpan.FromMilliseconds(RailPillsModel.HoverMs)), EnableDependentAnimation = true };
                Microsoft.UI.Xaml.Media.Animation.Storyboard.SetTarget(a, scale);
                Microsoft.UI.Xaml.Media.Animation.Storyboard.SetTargetProperty(a, prop);
                sb.Children.Add(a);
            }
            sb.Begin();
        }
        else
        {
            scale.ScaleX = s;
            scale.ScaleY = s;
        }
        hoveredDot = on ? id : (hoveredDot == id ? null : hoveredDot);
        DotHovered?.Invoke(on ? session : null);
    }

    private Border ActionsPill(ElementTheme theme)
    {
        var pill = Pill(theme, RailPillsModel.PillWidth, "pill-actions", RailPillsModel.Describe(RailPillKind.Actions, pills));
        // ask · mic · point stacked in a 52 epx capsule: three 16 epx targets, the mic in the middle.
        var col = new StackPanel { Spacing = 0, VerticalAlignment = VerticalAlignment.Center, HorizontalAlignment = HorizontalAlignment.Center };
        var ask = Flat(Theme.Icon("\uE70F", theme, NativeLook.TextSecondary, 12), "act-ask", "Ask theuth", 44, 16); // Edit (E70F): "ask"
        ask.Click += (_, _) =>
        {
            CancelDeferredHandBack();
            AskClicked?.Invoke(ScreenRectOf(pill));
        };
        micSlot = new Grid { Width = 44, Height = 18 };
        if (pills.Recording) micSlot.Children.Add(PillGlyphs.LevelBars(pills.Levels, 18, 3, theme));
        else micSlot.Children.Add(Theme.Icon(NativeLook.Glyph.Microphone, theme, NativeLook.TextSecondary, 12));
        var mic = Flat(micSlot, "act-mic", pills.Recording ? "Listening" : "Talk to theuth", 44, 18);
        mic.Click += (_, _) =>
        {
            CancelDeferredHandBack();
            AskClicked?.Invoke(ScreenRectOf(pill));
        };
        var point = Flat(Theme.Icon(NativeLook.Glyph.Camera, theme, NativeLook.TextSecondary, 12), "act-point", "Point and say", 44, 16);
        point.Click += (_, _) => PointClicked?.Invoke();
        col.Children.Add(ask);
        col.Children.Add(mic);
        col.Children.Add(point);
        pill.Child = col;
        hits.Add((pill, RailPillKind.Actions));
        hits.Add((ask, "ask"));
        hits.Add((mic, "ask"));
        hits.Add((point, "point"));
        return pill;
    }

    private Border MorePill(ElementTheme theme)
    {
        var pill = Pill(theme, RailPillsModel.MoreHeight, "pill-more", "More");
        var dots = Theme.Icon(NativeLook.Glyph.More, theme, NativeLook.TextSecondary, 12); // More (E712), not a "···" character
        var b = Flat(dots, "pill-more-button", "More", RailPillsModel.PillWidth, RailPillsModel.MoreHeight);
        b.Click += (_, _) =>
        {
            var r = ScreenRectOf(pill);
            MoreClicked?.Invoke(new System.Drawing.Point(Side == "left" ? r.right : r.left, r.top));
        };
        pill.Child = b;
        hits.Add((pill, RailPillKind.More));
        return pill;
    }

    /// <summary>Sizes to the stack (+ margins) at the configured edge of the primary work area, then shapes the window to its capsules.</summary>
    public void Layout()
    {
        double scale = Scale;
        double wEpx, hEpx;
        if (collapsed)
        {
            wEpx = SliverWidth;
            hEpx = SliverHeight;
        }
        else
        {
            wEpx = RailPillsModel.WindowWidth;
            hEpx = RailPillsModel.StackHeight(pills.Dots.Count, pills.Overflow) + 2 * Margin;
        }
        var wa = DisplayArea.Primary.WorkArea;
        int w = Math.Max(1, (int)Math.Round(wEpx * scale));
        int h = Math.Min(wa.Height, Math.Max(1, (int)Math.Round(hEpx * scale)));
        int x = Side == "left" ? wa.X : wa.X + wa.Width - w;
        int y = wa.Y + (wa.Height - h) / 2;
        Place(new RectInt32(x, y, w, h));
        ApplyShape(scale);
        rectsDirty = true;
    }

    /// <summary>The window region: every pill capsule; the sliver when collapsed.</summary>
    private void ApplyShape(double scale)
    {
        var blobs = new List<Shape.Blob>();
        int P(double v) => (int)Math.Round(v * scale);
        if (collapsed)
        {
            blobs.Add(new Shape.Blob(0, 0, P(SliverWidth), P(SliverHeight), P(SliverWidth / 2)));
        }
        else
        {
            int left = P(Margin), right = P(Margin + RailPillsModel.PillWidth);
            foreach (var s in RailPillsModel.Layout(pills.Dots.Count, pills.Overflow))
            {
                int top = P(Margin + s.Top), bottom = P(Margin + s.Top + s.Height);
                blobs.Add(new Shape.Blob(left, top, right, bottom, P(RailPillsModel.Radius))); // the count sits inside the inbox capsule (N1)
            }
        }
        if (!Shape.Set(Hwnd, blobs)) Log.Write($"rail pills: SetWindowRgn failed {Win32.LastError()}");
    }

    private void SetCollapsed(bool c)
    {
        quietTimer?.Stop();
        quietTimer = null;
        if (collapsed == c) return;
        collapsed = c;
        Refresh();
    }

    private void ScheduleCollapse(bool afterHover)
    {
        if (collapsed) return;
        var delay = RailCollapse.CollapseDelay(Presence, (store.FixtureRail ?? store.Rail).Quiet, hovering, afterHover);
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

    /// <summary>An element's rectangle on screen (physical px).</summary>
    public RECT ScreenRectOf(FrameworkElement e)
    {
        var wr = ScreenRect;
        if (Rel(e) is not { } rel) return wr;
        return new RECT { left = wr.left + rel.left, top = wr.top + rel.top, right = wr.left + rel.right, bottom = wr.top + rel.bottom };
    }

    private RECT? Rel(FrameworkElement e)
    {
        if (e.ActualHeight <= 0 || e.ActualWidth <= 0) return null;
        try
        {
            var p = e.TransformToVisual(root).TransformPoint(new global::Windows.Foundation.Point(0, 0));
            double s = Scale;
            return new RECT
            {
                left = (int)Math.Floor(p.X * s), right = (int)Math.Ceiling((p.X + e.ActualWidth) * s),
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
        rects.Clear();
        bool all = true;
        foreach (var (v, tag) in hits)
        {
            if (Rel(v) is { } r) rects.Add((r, tag));
            else all = false;
        }
        rectsDirty = !all;
    }

    /// <summary>What is under the cursor: an <see cref="AgentDot"/>, a <see cref="RailPillKind"/>, "ask"/"point"/"sliver", or null.</summary>
    public object? HitAt(System.Drawing.Point c)
    {
        var wr = ScreenRect;
        object? pill = null;
        foreach (var (rel, tag) in rects)
        {
            var r = new RECT { left = wr.left + rel.left, top = wr.top + rel.top, right = wr.left + rel.right, bottom = wr.top + rel.bottom };
            if (!Win32.Contains(r, c)) continue;
            if (tag is RailPillKind) { pill ??= tag; continue; }
            return tag; // a dot or an action glyph wins over its pill
        }
        return pill;
    }

    /// <summary>A press that opens a panel keeps the activation; everything else hands the foreground back (W6).</summary>
    private static bool OpensPanel(object? hit) => hit is AgentDot || hit is RailPillKind.Inbox || hit is RailPillKind.Agents || (hit is string s && s == "ask");

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
                if (hoveredDot is not null)
                {
                    hoveredDot = null;
                    DotHovered?.Invoke(null);
                }
            }
        }
        if (rectsDirty) ComputeRects();
        bool over = inside && HitAt(c) is not null;
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
                Log.Write($"rail pass-through: cursor ({c.X},{c.Y}) overPill={(over ? "true" : "false")} -> WS_EX_TRANSPARENT {(wantTransparent ? "on" : "off")}");
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
        var hit = HitAt(c);
        if (OpensPanel(hit))
        {
            Log.Write($"rail: press on {Describe(hit)} — the panel takes the foreground (no hand-back)");
            return;
        }
        var back = LastForeign;
        bool ok = Win32.Foreground(back);
        Log.Write($"rail: activated by a press on {Describe(hit)} — hand-back to {Win32.Describe(back)} -> {(ok ? "true" : "false")}; foreground now {Win32.Describe(PInvoke.GetForegroundWindow())}");
    }

    private static string Describe(object? hit) => hit switch
    {
        AgentDot d => $"dot {d.Label}",
        RailPillKind k => $"pill {k}",
        string s => s,
        _ => "nothing",
    };

    private DispatcherQueueTimer? deferred;

    private void CancelDeferredHandBack()
    {
        deferred?.Stop();
        deferred = null;
    }

    /// <summary>The shell's WM_CLOSE goes to the host window; a close of the rail itself is ignored.</summary>
    protected override void OnCloseRequested() { }

    public void Stop()
    {
        poll.Stop();
        quietTimer?.Stop();
        store.PropertyChanged -= OnStoreChanged;
        store.DictationChanged -= OnDictation;
    }
}