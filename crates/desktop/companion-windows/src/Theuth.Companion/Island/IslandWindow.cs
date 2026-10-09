using Microsoft.UI;
using Microsoft.UI.Dispatching;
using Microsoft.UI.Xaml;
using Microsoft.UI.Xaml.Automation;
using Microsoft.UI.Xaml.Controls;
using Microsoft.UI.Xaml.Input;
using Microsoft.UI.Text;
using Microsoft.UI.Xaml.Controls.Primitives;
using Microsoft.UI.Xaml.Media;
using Microsoft.UI.Xaml.Shapes;
using Theuth.Companion.Core;
using Theuth.Companion.Interop;
using Theuth.Companion.PointAndSay;
using Theuth.Companion.Ui;
using Windows.Graphics;
using Windows.UI.Text;
using Windows.Win32;
using Windows.Win32.Foundation;
using Windows.Win32.UI.Shell;
using Windows.Win32.UI.WindowsAndMessaging;

namespace Theuth.Companion.Island;

/// <summary>What the island's controls ask for (the controller turns them into <c>meeting_*</c> RPCs).</summary>
internal abstract record IslandAction
{
    private IslandAction() { }

    public sealed record Tap(string Topic) : IslandAction;

    public sealed record Dismiss(string Topic) : IslandAction;

    public sealed record Deeper(string Brief) : IslandAction;

    public sealed record Request(string Text) : IslandAction;

    public sealed record Move(int Delta) : IslandAction;

    public sealed record Mute : IslandAction;

    public sealed record Stop : IslandAction;

    public sealed record Eye : IslandAction;

    public sealed record OpenMeetings : IslandAction;
}

/// <summary>The voice mode / answer card the island shows while a point-and-say gesture is live (149 §3.7, D14).</summary>
internal sealed record VoiceView(string Phase, string Live, string Status, string? Answer, string? Hint, bool Locked, bool Working);

/// <summary>
/// The native island (149 §3.6, §7.4 #1, D9–D11): a <see cref="ToolWindow"/> with the shell's Windows island styles
/// (<c>WS_EX_TOOLWINDOW | WS_EX_NOACTIVATE | WS_EX_TOPMOST</c>, shown without activation, <c>WDA_EXCLUDEFROMCAPTURE</c>),
/// opaque, rounded, dark-first. Closed: a pill at the top centre of the chosen monitor's work area — rec dot · <c>mm:ss</c> ·
/// the topic count. Open (hover 0.3 s or click; closes 100 ms after the pointer leaves unless pinned by a click): the strip
/// (topics with ✕, mute, stop, eye), the brief pane (title, ≤ 3 bullets, deeper, newer/older, <c>↑N new</c>), the collapsed
/// transcript row and the request box. Draggable anywhere by its grip (a manual move: pointer capture + <c>AppWindow.Move</c>);
/// on release <see cref="IslandModel.DragEnded"/> snaps back within 40 pt or remembers the offset per monitor device name;
/// double-click the grip snaps back. Keys (island scope, §8): M mute · E dismiss the first topic · D deeper · ⎋ leave the
/// box / close; ⏎ in the request box sends. The voice mode (point-and-say) replaces the content while a gesture is live.
/// UIA: root <c>island</c>, <c>island-pill</c>, <c>island-clock</c>, <c>island-topics</c>, topics <c>island-topic-&lt;n&gt;</c>,
/// <c>island-brief-title</c>, <c>island-request</c>, <c>island-voice</c>.
/// </summary>
internal sealed partial class IslandWindow : ToolWindow
{
    public const double PillWidth = 300;
    public const double PillHeight = 36;
    public const double OpenWidth = 440;
    public const double GripWidth = 24;
    public const double VoiceMinHeight = 64;

    private readonly DispatcherQueue dispatcher;
    private readonly IslandModel model;
    private readonly Action<IslandAction> act;
    private readonly Action save;
    private readonly Grid root;
    private readonly Border shell;
    private readonly StackPanel body;
    private readonly DispatcherQueueTimer hoverTimer;
    private readonly DispatcherQueueTimer leaveTimer;
    private readonly DispatcherQueueTimer clockTimer;
    private readonly DispatcherQueueTimer dragTimer;
    private readonly SUBCLASSPROC subclass149; // referenced for the window's lifetime
    private const nuint Subclass149Id = 0x149;
    private MeetingSnapshot snapshot = MeetingSnapshot.Empty;
    private VoiceView? voice;
    private TextBox? request;
    private TextBlock? clock;
    private ScreenMonitor? monitor;
    private HWND previous;
    private bool dragging;
    private bool moved;
    private int dragDx, dragDy;
    private DateTimeOffset lastGripPress;
    private bool wantsKey;
    private bool hiddenFromCapture;
    private Rectangle? micBar, sysBar;
    private float micLevel, sysLevel;
    private string? oracleMode;
    private DateTimeOffset verdictAt;

    public IslandWindow(DispatcherQueue dispatcher, IslandModel model, Action<IslandAction> act, Action save)
        : base("theuth island", WINDOW_EX_STYLE.WS_EX_NOACTIVATE, excludeFromCapture: !Flags.Shareable)
    {
        this.dispatcher = dispatcher;
        this.model = model;
        this.act = act;
        this.save = save;
        root = new Grid { RequestedTheme = ElementTheme.Dark, Background = Palette.Transparent };
        AutomationProperties.SetAutomationId(root, "island");
        AutomationProperties.SetName(root, "theuth island");
        FlatButtons.Apply(root.Resources, Palette.Tint(Palette.Ink, ElementTheme.Dark, 0.12));
        body = new StackPanel { Orientation = Orientation.Vertical, Spacing = 6 };
        shell = new Border
        {
            Background = new SolidColorBrush(ColorHelper.FromArgb(255, 0x14, 0x12, 0x10)),
            BorderBrush = new SolidColorBrush(ColorHelper.FromArgb(255, 0x3a, 0x32, 0x28)),
            BorderThickness = new Thickness(1),
            CornerRadius = new CornerRadius(18),
            Padding = new Thickness(10, 4, 10, 4),
            Child = body,
            HorizontalAlignment = HorizontalAlignment.Stretch,
            VerticalAlignment = VerticalAlignment.Top,
        };
        root.Children.Add(shell);
        Content = root;
        root.PointerEntered += (_, _) => OnEnter();
        root.PointerExited += (_, _) => OnLeave();
        root.PointerPressed += OnPressed;
        root.PointerMoved += OnMoved;
        root.PointerReleased += OnReleased;
        root.KeyDown += OnKeyDown;
        hoverTimer = Timer(300, () => { if (!model.Pinned) OpenIsland(pin: false); });
        leaveTimer = Timer(100, () => { if (model.Close()) Render(); });
        clockTimer = Timer(1000, () => clock?.SetValue(TextBlock.TextProperty, IslandModel.Clock(snapshot.Elapsed(DateTimeOffset.Now))));
        clockTimer.IsRepeating = true;
        dragTimer = Timer(16, DragTick);
        dragTimer.IsRepeating = true;
        hiddenFromCapture = ExcludedFromCapture;
        subclass149 = Subclass149;
        if (!PInvoke.SetWindowSubclass(Hwnd, subclass149, Subclass149Id, 0)) Log.Write($"island: SetWindowSubclass failed {Win32.LastError()}");
        Render();
    }

    /// <summary><c>WM_HOTKEY</c> on this window (point-and-say registers Esc here between press and send).</summary>
    public event Action<int>? Hotkey;

    /// <summary><c>WM_DISPLAYCHANGE</c> / <c>WM_DPICHANGED</c>: the controller re-places the island.</summary>
    public event Action? DisplayChanged;

    private LRESULT Subclass149(HWND h, uint msg, WPARAM w, LPARAM l, nuint id, nuint data)
    {
        try
        {
            if (msg == PInvoke.WM_HOTKEY) Hotkey?.Invoke((int)w.Value);
            else if (msg == PInvoke.WM_DISPLAYCHANGE || msg == 0x02E0 /* WM_DPICHANGED */) DisplayChanged?.Invoke();
        }
        catch (Exception e)
        {
            Log.Write($"island: message 0x{msg:X4} handler failed: {e.GetType().Name}: {e.Message}");
        }
        return PInvoke.DefSubclassProc(h, msg, w, l);
    }

    /// <summary>The eye (Alt+Shift+E): toggles <c>WDA_EXCLUDEFROMCAPTURE</c> on this window; the controller mirrors it on the overlays.</summary>
    public bool HiddenFromCapture => hiddenFromCapture;

    public void SetHiddenFromCapture(bool hidden)
    {
        hiddenFromCapture = hidden;
        if (!PInvoke.SetWindowDisplayAffinity(Hwnd, hidden ? WINDOW_DISPLAY_AFFINITY.WDA_EXCLUDEFROMCAPTURE : WINDOW_DISPLAY_AFFINITY.WDA_NONE))
        {
            Log.Write($"island: SetWindowDisplayAffinity({(hidden ? "exclude" : "none")}) failed {Win32.LastError()}");
        }
        Render();
    }

    private DispatcherQueueTimer Timer(int ms, Action tick)
    {
        var t = dispatcher.CreateTimer();
        t.Interval = TimeSpan.FromMilliseconds(ms);
        t.IsRepeating = false;
        t.Tick += (_, _) => tick();
        return t;
    }

    /// <summary>The monitor the island sits on (the chosen one: where it was last placed, else the primary).</summary>
    public ScreenMonitor? Monitor => monitor;

    // MARK: show / hide / place

    /// <summary>Places on <paramref name="m"/> (saved offset or top-centre) and shows without activation.</summary>
    public void ShowOn(ScreenMonitor m)
    {
        monitor = m;
        Relayout();
        if (!IsShown)
        {
            ShowPassive();
            dispatcher.TryEnqueue(DispatcherQueuePriority.Low, () => { root.InvalidateMeasure(); root.UpdateLayout(); Render(); });
            Log.Write($"island shown: rect={Win32.Text(ScreenRect)} on {m.Device} mode={model.Mode} exstyle={Win32.ExStyleText(Win32.ExStyle(Hwnd))} affinity=0x{Win32.DisplayAffinity(Hwnd):X}");
        }
        if (snapshot.State == "Recording") clockTimer.Start();
    }

    public void HideIsland()
    {
        if (!IsShown) return;
        clockTimer.Stop();
        hoverTimer.Stop();
        leaveTimer.Stop();
        model.Close(force: true);
        HideWindow();
        Log.Write("island hidden");
    }

    /// <summary>Re-reads the monitor list (<c>WM_DISPLAYCHANGE</c>) and places again.</summary>
    public void Relayout()
    {
        if (monitor is null) return;
        var all = ScreenMonitor.All();
        monitor = all.FirstOrDefault(x => x.Device == monitor.Device) ?? ScreenMonitor.At(all, 0, 0);
        var size = CurrentSize();
        var (frame, saved) = model.Place(monitor.AsIslandDisplay(), size);
        if (!saved && model.Mode == IslandMode.Detached) model.SnapBack(monitor.AsIslandDisplay(), size);
        Place(new RectInt32(frame.X, frame.Y, frame.Width, frame.Height));
    }

    /// <summary>The window size for the current mode, physical px: the pill, or the open panel measured.</summary>
    private (int Width, int Height) CurrentSize()
    {
        double s = Scale;
        if (voice is null && model.Mode == IslandMode.Closed)
        {
            return ((int)Math.Round(PillWidth * s), (int)Math.Round(PillHeight * s));
        }
        root.InvalidateMeasure();
        root.Measure(new global::Windows.Foundation.Size(OpenWidth, double.PositiveInfinity));
        double h = Math.Max(voice is null ? PillHeight : VoiceMinHeight, Math.Min(520, root.DesiredSize.Height + 2));
        return ((int)Math.Round(OpenWidth * s), (int)Math.Round(h * s));
    }

    /// <summary>Resizes in place around the pill's centre-x and top (so opening grows down from the pill).</summary>
    private void Resize()
    {
        if (monitor is null) return;
        var r = ScreenRect;
        var size = CurrentSize();
        int cx = (r.left + r.right) / 2;
        int x = cx - size.Width / 2;
        var wa = monitor.WorkArea;
        x = Math.Max(wa.X, Math.Min(x, wa.Right - size.Width));
        int y = Math.Max(wa.Y, Math.Min(r.top, wa.Bottom - size.Height));
        Place(new RectInt32(x, y, size.Width, size.Height));
    }

    // MARK: data

    public void Update(MeetingSnapshot s)
    {
        if ((s.Oracle.Count > 0 ? s.Oracle[^1] : null) != (snapshot.Oracle.Count > 0 ? snapshot.Oracle[^1] : null)) verdictAt = DateTimeOffset.Now;
        snapshot = s;
        if (s.State == "Recording" && IsShown) clockTimer.Start(); else clockTimer.Stop();
        Render();
    }

    /// <summary>The meters (<c>MeetingEvent::Level</c>, ~6/s): the two bars resize in place, no re-render.</summary>
    public void SetLevels(float mic, float system)
    {
        micLevel = mic;
        sysLevel = system;
        if (micBar is { } m) m.Height = MeterHeight(mic);
        if (sysBar is { } s) s.Height = MeterHeight(system);
    }

    private static double MeterHeight(float rms) => 3 + Math.Round(11 * Math.Clamp(Math.Sqrt(Math.Max(0, rms)) * 1.6, 0, 1));

    /// <summary>The oracle chip's mode word (<c>decision_mode('meeting').effective</c>, every 30 s); null/"off" hides the chip unless a verdict is live.</summary>
    public void SetOracleMode(string? mode)
    {
        if (oracleMode == mode) return;
        oracleMode = mode;
        if (voice is null && IsShown) Render();
    }

    /// <summary>Point-and-say's voice mode; null returns the island to its meeting content.</summary>
    public void SetVoice(VoiceView? v)
    {
        bool was = voice is not null;
        voice = v;
        if (v is null && was) model.Close(force: true);
        Render();
    }

    public bool InVoiceMode => voice is not null;

    // MARK: hover / click

    private void OnEnter()
    {
        leaveTimer.Stop();
        if (voice is null && model.Mode == IslandMode.Closed) hoverTimer.Start();
    }

    private void OnLeave()
    {
        hoverTimer.Stop();
        if (dragging) return;
        if (voice is null && model.Mode == IslandMode.Open && !wantsKey) leaveTimer.Start();
    }

    private void OpenIsland(bool pin)
    {
        if (voice is not null) return;
        model.Open(pin);
        Render();
    }

    private void OnPressed(object sender, PointerRoutedEventArgs e)
    {
        var pt = e.GetCurrentPoint(root);
        if (!pt.Properties.IsLeftButtonPressed) return;
        bool grip = IsGrip(pt.Position, e.OriginalSource as DependencyObject);
        if (!grip) return;
        var now = DateTimeOffset.Now;
        if (now - lastGripPress < TimeSpan.FromMilliseconds(400))
        {
            lastGripPress = DateTimeOffset.MinValue;
            SnapBack();
            e.Handled = true;
            return;
        }
        lastGripPress = now;
        if (PInvoke.GetCursorPos(out var c))
        {
            var r = ScreenRect;
            dragDx = c.X - r.left;
            dragDy = c.Y - r.top;
            dragging = true;
            moved = false;
            // The manual move: a 16 ms poll over GetCursorPos + the button state. XAML pointer capture is not relied on —
            // the island activates on the press (S1) and a non-active WinUI window drops PointerMoved/capture (build-B).
            dragTimer.Start();
        }
        e.Handled = true;
    }

    private void OnMoved(object sender, PointerRoutedEventArgs e)
    {
        if (dragging) e.Handled = true;
    }

    private void DragTick()
    {
        if (!dragging) { dragTimer.Stop(); return; }
        bool down = (PInvoke.GetAsyncKeyState(0x01) & 0x8000) != 0;
        if (!PInvoke.GetCursorPos(out var c)) return;
        if (down)
        {
            var r = ScreenRect;
            int nx = c.X - dragDx, ny = c.Y - dragDy;
            if (!moved && Math.Abs(nx - r.left) < 3 && Math.Abs(ny - r.top) < 3) return;
            moved = true;
            AppWindow.Move(new PointInt32(nx, ny));
            return;
        }
        dragTimer.Stop();
        bool wasMoved = moved;
        EndDrag(snap: true);
        if (!wasMoved && voice is null)
        {
            // A click on the pill (not a drag) opens and pins; the idle pill ("Always show", no meeting) deep-links to theuth://meetings.
            if (model.Mode == IslandMode.Closed && snapshot.Meeting is null) act(new IslandAction.OpenMeetings());
            else if (model.Mode == IslandMode.Closed) OpenIsland(pin: true);
            else model.Pinned = true;
        }
        if (!wantsKey) HandBack();
    }

    private void OnReleased(object sender, PointerRoutedEventArgs e)
    {
        if (dragging) { e.Handled = true; return; } // the drag poll ends it
        if (!wantsKey) HandBack();
    }

    private void EndDrag(bool snap)
    {
        if (!dragging) return;
        dragging = false;
        if (!moved || !snap) return;
        var r = ScreenRect;
        var frame = new PxRect(r.left, r.top, r.right - r.left, r.bottom - r.top);
        var all = ScreenMonitor.All();
        var m = ScreenMonitor.At(all, (int)frame.CenterX, (int)frame.CenterY);
        monitor = m;
        var target = model.DragEnded(m.AsIslandDisplay(), frame);
        if (target != frame) Place(new RectInt32(target.X, target.Y, target.Width, target.Height));
        save();
        Log.Write($"island drag ended: {(target == frame ? "kept" : "snapped back")} at {target.X},{target.Y} on {m.Device} mode={model.Mode}");
    }

    private void SnapBack()
    {
        if (monitor is null) return;
        var f = model.SnapBack(monitor.AsIslandDisplay(), CurrentSize());
        Place(new RectInt32(f.X, f.Y, f.Width, f.Height));
        save();
        Log.Write($"island snapped back (double-click) to {f.X},{f.Y} on {monitor.Device}");
    }

    /// <summary>The grip: the whole pill when closed; when open, the left 24 epx of the strip and anything that is not a control.</summary>
    private bool IsGrip(global::Windows.Foundation.Point p, DependencyObject? source)
    {
        if (voice is not null) return true;
        if (model.Mode == IslandMode.Closed) return true;
        if (p.X <= GripWidth + 10) return true;
        for (var d = source; d is not null; d = VisualTreeHelper.GetParent(d))
        {
            if (d is ButtonBase or TextBox or ScrollViewer) return false;
            if (d == root) break;
        }
        return true;
    }

    protected override void OnActivate(bool active, HWND other)
    {
        if (active && !other.IsNull && !Win32.IsOurs(other)) previous = other;
    }

    /// <summary>The WinUI island activates on every press despite <c>WS_EX_NOACTIVATE</c> (spike S1): hand the foreground back.</summary>
    private void HandBack()
    {
        if (!IsForeground || previous.IsNull) return;
        dispatcher.TryEnqueue(() => { if (!wantsKey) Win32.Foreground(previous); });
    }

    // MARK: keys (island scope)

    private void OnKeyDown(object sender, KeyRoutedEventArgs e)
    {
        var key = KeyInput.From(e);
        bool inBox = request is not null && request.FocusState != FocusState.Unfocused;
        if (key.KeyCode == VirtualKeys.Escape)
        {
            if (inBox) LeaveBox(); else { model.Close(force: true); Render(); HandBack(); }
            e.Handled = true;
            return;
        }
        if (inBox)
        {
            if (key.KeyCode == VirtualKeys.Return && request is not null)
            {
                var text = request.Text.Trim();
                if (text.Length > 0) { act(new IslandAction.Request(text)); request.Text = ""; }
                e.Handled = true;
            }
            return;
        }
        if (key.Modifiers.HasFlag(ChordModifiers.Control) && !key.Modifiers.HasFlag(ChordModifiers.Alt) && key.KeyCode == 'M')
        {
            act(new IslandAction.Stop()); // Ctrl+Shift+M (C stores it as Ctrl+M locally)
            e.Handled = true;
            return;
        }
        if (key.Modifiers != ChordModifiers.None && key.Modifiers != ChordModifiers.Shift) return;
        switch (key.Chars.ToLowerInvariant())
        {
            case "m": act(new IslandAction.Mute()); e.Handled = true; break;
            case "e":
                if (snapshot.Topics.Count > 0) act(new IslandAction.Dismiss(snapshot.Topics[0].Id));
                e.Handled = true;
                break;
            case "d":
                if (snapshot.Current is { } b) act(new IslandAction.Deeper(b.Id));
                e.Handled = true;
                break;
        }
    }

    private void LeaveBox()
    {
        wantsKey = false;
        root.Focus(FocusState.Programmatic);
        HandBack();
    }

    // MARK: render

    private void Render()
    {
        body.Children.Clear();
        request = null;
        clock = null;
        micBar = null;
        sysBar = null;
        if (voice is { } v) RenderVoice(v);
        else if (model.Mode == IslandMode.Closed) RenderPill();
        else RenderOpen();
        if (IsShown) Resize();
    }

    private static TextBlock Text(string s, double size, string key, FontWeight? weight = null) => new()
    {
        Text = s,
        FontSize = size,
        Foreground = Palette.Brush(key, ElementTheme.Dark),
        VerticalAlignment = VerticalAlignment.Center,
        TextTrimming = TextTrimming.CharacterEllipsis,
        FontWeight = weight ?? FontWeights.Normal,
    };

    private Button Btn(string label, string tip, Action click, string? id = null)
    {
        var b = new Button
        {
            Content = Text(label, 12.5, Palette.Ink),
            Padding = new Thickness(8, 2, 8, 2),
            MinHeight = 24,
            CornerRadius = new CornerRadius(10),
            VerticalAlignment = VerticalAlignment.Center,
        };
        ToolTipService.SetToolTip(b, tip);
        if (id is not null) AutomationProperties.SetAutomationId(b, id);
        AutomationProperties.SetName(b, tip);
        b.Click += (_, _) => { click(); if (!wantsKey) HandBack(); };
        return b;
    }

    private Ellipse Dot(string key, double size = 8) => new()
    {
        Width = size, Height = size, Fill = Palette.Brush(key, ElementTheme.Dark), VerticalAlignment = VerticalAlignment.Center,
    };

    private StackPanel Row(double spacing = 8) => new() { Orientation = Orientation.Horizontal, Spacing = spacing, VerticalAlignment = VerticalAlignment.Center };

    /// <summary>The meters: mic (faience) and system (sand) bars, 3 epx wide, 3–14 epx tall, bottom-aligned.</summary>
    private StackPanel Meters()
    {
        var p = new StackPanel { Orientation = Orientation.Horizontal, Spacing = 2, VerticalAlignment = VerticalAlignment.Center, Height = 14 };
        AutomationProperties.SetAutomationId(p, "island-meters");
        micBar = new Rectangle { Width = 3, Height = MeterHeight(micLevel), RadiusX = 1.5, RadiusY = 1.5, Fill = Palette.Brush(Palette.Faience, ElementTheme.Dark), VerticalAlignment = VerticalAlignment.Bottom };
        sysBar = new Rectangle { Width = 3, Height = MeterHeight(sysLevel), RadiusX = 1.5, RadiusY = 1.5, Fill = Palette.Brush(Palette.Sand, ElementTheme.Dark), VerticalAlignment = VerticalAlignment.Bottom };
        p.Children.Add(micBar);
        p.Children.Add(sysBar);
        return p;
    }

    /// <summary>The oracle chip (<c>Island.tsx OracleChip</c>): the effective mode word + <c>deciding</c> · <c>skip</c> · <c>brief</c> · <c>→ model</c>; nothing when off and quiet.</summary>
    private Border? OracleChip()
    {
        var s = snapshot;
        var last = s.Oracle.Count > 0 ? s.Oracle[^1] : null;
        string effective = oracleMode ?? "off";
        if (effective == "off" && s.Deciding is null && last is null) return null;
        string? verdict = s.Deciding is not null ? "deciding" : last is not null && DateTimeOffset.Now - verdictAt < TimeSpan.FromSeconds(6)
            ? last.Disposition == "skip" ? "skip" : last.Disposition == "brief" ? "brief" : "→ model" : null;
        var row = Row(4);
        if (effective != "off") row.Children.Add(Text(effective.ToUpperInvariant(), 10, Palette.Sand));
        if (verdict is not null) row.Children.Add(Text(verdict, 10.5, s.Deciding is not null ? Palette.Gold : Palette.Ink2));
        var chip = new Border
        {
            Background = Palette.Tint(Palette.Ink, ElementTheme.Dark, 0.1),
            CornerRadius = new CornerRadius(8),
            Padding = new Thickness(6, 0, 6, 1),
            VerticalAlignment = VerticalAlignment.Center,
            Child = row,
        };
        ToolTipService.SetToolTip(chip, MeetingReducer.OracleSummary(s.Oracle) ?? $"decision oracle {effective}");
        AutomationProperties.SetAutomationId(chip, "island-oracle");
        return chip;
    }

    /// <summary>The pill's title/speaker word: the newest line's speaker (<c>Speaker 2</c> · <c>me</c>).</summary>
    private static string? SpeakerWord(MeetingSnapshot s) => s.Lines.Count > 0 ? s.Lines[^1].Who.TrimEnd(':') : null;

    private void RenderPill()
    {
        shell.CornerRadius = new CornerRadius(18);
        shell.Padding = new Thickness(12, 0, 12, 0);
        var row = Row();
        row.Height = PillHeight - 2;
        AutomationProperties.SetAutomationId(row, "island-pill");
        var s = snapshot;
        if (s.Meeting is not null) row.Children.Add(Meters());
        if (SpeakerWord(s) is { } who) row.Children.Add(Text(who, 12, Palette.Ink2));
        if (s.State == "Recording") row.Children.Add(Dot(Palette.Carnelian));
        else if (s.State == "Paused") row.Children.Add(Dot(Palette.Sand));
        else row.Children.Add(Dot(Palette.Faience, 6));
        clock = Text(s.Meeting is null ? "no meeting" : IslandModel.Clock(s.Elapsed(DateTimeOffset.Now)), 13, Palette.Ink, FontWeights.SemiBold);
        AutomationProperties.SetAutomationId(clock, "island-clock");
        row.Children.Add(clock);
        if (s.State is "Paused") row.Children.Add(Text("paused", 12, Palette.Ink2));
        else if (s.State is "Starting" or "Stopping") row.Children.Add(Text(s.State.ToLowerInvariant() + "…", 12, Palette.Ink2));
        if (s.Listening) row.Children.Add(Text("listening…", 12, Palette.Faience));
        else if (s.ApprovalWaiting) row.Children.Add(Text("approval waiting", 12, Palette.Gold));
        else if (s.Activity is { } a) row.Children.Add(Text(a, 12, Palette.Ink2));
        var spacer = new Border { Width = 1 };
        row.Children.Add(spacer);
        int n = s.Topics.Count;
        var count = new Border
        {
            Background = n > 0 ? Palette.Brush(Palette.Gold, ElementTheme.Dark) : Palette.Tint(Palette.Ink, ElementTheme.Dark, 0.12),
            CornerRadius = new CornerRadius(9),
            Padding = new Thickness(7, 0, 7, 1),
            VerticalAlignment = VerticalAlignment.Center,
            Child = Text(n == 1 ? "1 topic" : $"{n} topics", 11.5, n > 0 ? Palette.Surface : Palette.Ink2, FontWeights.SemiBold),
        };
        AutomationProperties.SetAutomationId(count, "island-topics");
        row.Children.Add(count);
        if (s.Unseen > 0) row.Children.Add(Text($"↑{s.Unseen} new", 11.5, Palette.Gold));
        body.Children.Add(row);
    }

    private void RenderOpen()
    {
        shell.CornerRadius = new CornerRadius(18);
        shell.Padding = new Thickness(10, 6, 10, 8);
        var s = snapshot;
        // The strip: grip · rec · clock · topics · badges · mute · stop · eye.
        var strip = Row(6);
        var grip = new Border { Width = GripWidth, Height = 20, Child = Text("⋮⋮", 12, Palette.Sand), VerticalAlignment = VerticalAlignment.Center };
        ToolTipService.SetToolTip(grip, "Drag to move · double-click to snap back");
        strip.Children.Add(grip);
        if (s.Meeting is not null) strip.Children.Add(Meters());
        strip.Children.Add(Dot(s.State == "Recording" ? Palette.Carnelian : Palette.Sand));
        clock = Text(s.Meeting is null ? "no meeting" : IslandModel.Clock(s.Elapsed(DateTimeOffset.Now)), 13, Palette.Ink, FontWeights.SemiBold);
        AutomationProperties.SetAutomationId(clock, "island-clock");
        strip.Children.Add(clock);
        if (OracleChip() is { } oracle) strip.Children.Add(oracle);
        foreach (var d in s.Degraded.Take(2)) strip.Children.Add(Badge(Degraded(d), Palette.Gold));
        if (s.Listening) strip.Children.Add(Badge("listening…", Palette.Faience));
        else if (s.ApprovalWaiting) strip.Children.Add(Badge("approval waiting", Palette.Gold));
        else if (s.Activity is { } a) strip.Children.Add(Text(a, 12, Palette.Ink2));
        var right = Row(2);
        right.HorizontalAlignment = HorizontalAlignment.Right;
        if (s.Meeting is not null && MeetingReducer.Live(s.State))
        {
            right.Children.Add(Btn(s.State == "Paused" ? "resume" : "mute", s.State == "Paused" ? "Resume (M)" : "Mute — pause the recording (M)", () => act(new IslandAction.Mute()), "island-mute"));
            right.Children.Add(Btn("stop", "Stop the meeting (Ctrl+Shift+M)", () => act(new IslandAction.Stop()), "island-stop"));
        }
        right.Children.Add(Btn(hiddenFromCapture ? "◉" : "○", hiddenFromCapture ? "Hidden from capture (best effort) — Alt+Shift+E" : "Visible in capture — Alt+Shift+E", () => act(new IslandAction.Eye()), "island-eye"));
        var stripGrid = new Grid();
        stripGrid.ColumnDefinitions.Add(new ColumnDefinition { Width = new GridLength(1, GridUnitType.Star) });
        stripGrid.ColumnDefinitions.Add(new ColumnDefinition { Width = GridLength.Auto });
        Grid.SetColumn(right, 1);
        stripGrid.Children.Add(strip);
        stripGrid.Children.Add(right);
        body.Children.Add(stripGrid);

        // Topic buttons (tapped = greyed until the brief lands).
        if (s.Topics.Count > 0)
        {
            var topics = new StackPanel { Orientation = Orientation.Horizontal, Spacing = 4 };
            AutomationProperties.SetAutomationId(topics, "island-topic-row");
            int i = 1;
            foreach (var t in s.Topics.TakeLast(4))
            {
                bool tapped = s.Tapped.Contains(t.Id);
                var row = Row(0);
                var tap = Btn(t.Label, $"Brief me on \"{t.Label}\"", () => act(new IslandAction.Tap(t.Id)), $"island-topic-{i}");
                tap.Opacity = tapped ? 0.5 : 1;
                tap.Background = Palette.Tint(Palette.Gold, ElementTheme.Dark, 0.18);
                var x = Btn("✕", $"Dismiss \"{t.Label}\" (E)", () => act(new IslandAction.Dismiss(t.Id)), $"island-topic-{i}-dismiss");
                x.Padding = new Thickness(4, 2, 4, 2);
                row.Children.Add(tap);
                row.Children.Add(x);
                topics.Children.Add(row);
                i++;
            }
            body.Children.Add(topics);
        }

        // The brief pane.
        if (s.Current is { } b)
        {
            var pane = new StackPanel { Spacing = 3 };
            var head = Row(6);
            var title = Text(b.Title, 14, Palette.Ink, FontWeights.SemiBold);
            AutomationProperties.SetAutomationId(title, "island-brief-title");
            head.Children.Add(title);
            head.Children.Add(Text($"{s.Cursor + 1}/{s.Briefs.Count}", 11.5, Palette.Sand));
            if (s.Unseen > 0) head.Children.Add(Text($"↑{s.Unseen} new", 11.5, Palette.Gold));
            pane.Children.Add(head);
            foreach (var bullet in b.Bullets.Take(3))
            {
                var t = Text("• " + bullet, 12.5, Palette.Ink2);
                t.TextWrapping = TextWrapping.Wrap;
                t.TextTrimming = TextTrimming.None;
                pane.Children.Add(t);
            }
            var nav = Row(4);
            nav.Children.Add(Btn("deeper", "Go deeper on this brief (D)", () => act(new IslandAction.Deeper(b.Id)), "island-deeper"));
            if (s.Cursor > 0) nav.Children.Add(Btn("‹ older", "Older brief", () => act(new IslandAction.Move(-1)), "island-older"));
            if (s.Cursor < s.Briefs.Count - 1) nav.Children.Add(Btn("newer ›", "Newer brief", () => act(new IslandAction.Move(+1)), "island-newer"));
            nav.Children.Add(Text(b.Source.ToLowerInvariant(), 11, Palette.Sand));
            pane.Children.Add(nav);
            body.Children.Add(pane);
        }
        else if (s.Meeting is not null)
        {
            body.Children.Add(Text(s.Topics.Count > 0 ? "Tap a topic for a brief." : "Listening for topics…", 12.5, Palette.Sand));
        }
        else
        {
            var none = Btn("No meeting — open Meetings", "theuth://meetings", () => act(new IslandAction.OpenMeetings()), "island-open-meetings");
            body.Children.Add(none);
        }

        // The collapsed transcript row: the newest line.
        if (s.Lines.Count > 0)
        {
            var l = s.Lines[^1];
            var line = Text($"{l.Who} {l.Text}", 11.5, l.Open ? Palette.Sand : Palette.Ink2);
            AutomationProperties.SetAutomationId(line, "island-transcript");
            body.Children.Add(line);
        }

        // The request box.
        if (s.Meeting is not null)
        {
            request = new TextBox
            {
                PlaceholderText = "Ask about the meeting… (⏎ sends, ⎋ leaves)",
                FontSize = 12.5,
                MinHeight = 28,
                CornerRadius = new CornerRadius(10),
            };
            AutomationProperties.SetAutomationId(request, "island-request");
            request.GotFocus += (_, _) => { wantsKey = true; leaveTimer.Stop(); };
            request.LostFocus += (_, _) => wantsKey = false;
            body.Children.Add(request);
        }
    }

    private void RenderVoice(VoiceView v)
    {
        shell.CornerRadius = new CornerRadius(18);
        shell.Padding = new Thickness(14, 8, 14, 10);
        var pane = new StackPanel { Spacing = 4 };
        AutomationProperties.SetAutomationId(pane, "island-voice");
        var head = Row(8);
        head.Children.Add(Dot(v.Phase == "listening" ? Palette.Carnelian : v.Working ? Palette.Faience : Palette.Malachite));
        head.Children.Add(Text(v.Status, 12.5, Palette.Ink2));
        if (v.Locked) head.Children.Add(Badge("locked", Palette.Gold));
        pane.Children.Add(head);
        if (v.Live.Length > 0)
        {
            var live = Text(v.Live, 15, Palette.Ink);
            live.TextWrapping = TextWrapping.Wrap;
            live.TextTrimming = TextTrimming.None;
            live.MaxLines = 4;
            AutomationProperties.SetAutomationId(live, "island-voice-live");
            pane.Children.Add(live);
        }
        if (v.Answer is { } ans)
        {
            var card = new Border
            {
                Background = Palette.Tint(Palette.Ink, ElementTheme.Dark, 0.08),
                CornerRadius = new CornerRadius(12),
                Padding = new Thickness(10, 6, 10, 6),
            };
            var inner = new StackPanel { Spacing = 2 };
            var a = Text(ans.Length == 0 ? "…" : ans, 13, Palette.Ink);
            a.TextWrapping = TextWrapping.Wrap;
            a.TextTrimming = TextTrimming.None;
            a.MaxLines = 6;
            AutomationProperties.SetAutomationId(a, "island-voice-answer");
            inner.Children.Add(a);
            inner.Children.Add(Text(v.Working ? "working… · Esc hides" : "Open Ctrl+O · Esc hides", 11, Palette.Sand));
            card.Child = inner;
            pane.Children.Add(card);
        }
        if (v.Hint is { } h) pane.Children.Add(Text(h, 12, Palette.Gold));
        body.Children.Add(pane);
    }

    private Border Badge(string text, string key) => new()
    {
        Background = Palette.Tint(key, ElementTheme.Dark, 0.18),
        CornerRadius = new CornerRadius(8),
        Padding = new Thickness(6, 0, 6, 1),
        VerticalAlignment = VerticalAlignment.Center,
        Child = Text(text, 11, key),
    };

    /// <summary>The degradation badges of <c>Island.tsx:37-47</c>, by word.</summary>
    private static string Degraded(string d) => d switch
    {
        "MicNoAec" => "no echo cancel",
        "ModelDown" => "model down",
        "DiskLow" => "disk low",
        "NearMaxLength" => "near max length",
        "NoNote" => "no note",
        "DiarizationDown" => "diarization down",
        _ when d.StartsWith("SttDown", StringComparison.Ordinal) => "transcription down",
        _ when d.StartsWith("PermissionDenied", StringComparison.Ordinal) => "permission denied",
        _ => d,
    };
}
