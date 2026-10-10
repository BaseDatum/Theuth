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

    /// <summary>Island fix §N: J/↓ (+1) · K/↑ (−1), wrapping.</summary>
    public sealed record Cycle(int Delta, string Via) : IslandAction;

    public sealed record Mute : IslandAction;

    public sealed record Stop : IslandAction;

    public sealed record Eye : IslandAction;

    public sealed record OpenMeetings : IslandAction;

    /// <summary>158 §5.4: the prompt's Start → <c>meeting_start(process: pid, consent: true, source: detected)</c>.</summary>
    public sealed record StartRecording(MeetingDetection Detection) : IslandAction;

    /// <summary>158 §5.3: Not now → <c>meeting_detection_dismiss(app_id, never: false)</c>.</summary>
    public sealed record NotNow(string AppId) : IslandAction;

    /// <summary>158 §5.3: Never for &lt;App&gt; → <c>meeting_detection_dismiss(app_id, never: true)</c>.</summary>
    public sealed record NeverForApp(string AppId) : IslandAction;

    /// <summary>158 §5.2 (d): keep recording — the stop prompt goes (the engine re-asks in 30 s while the app stays off the mic).</summary>
    public sealed record KeepRecording : IslandAction;

    /// <summary>158 §5.2 (c): Start from the upcoming brief → <c>meeting_start(title, consent: true, source: calendar, event, project)</c>.</summary>
    public sealed record StartUpcoming(MeetingUpcoming Upcoming) : IslandAction;

    /// <summary>158 §5.2 (c): Dismiss — the upcoming brief goes (local; the reducer ignores the engine's re-sends of that event).</summary>
    public sealed record DismissUpcoming : IslandAction;
}

/// <summary>The voice mode / answer card the island shows while a point-and-say gesture is live (149 §3.7, D14).</summary>
internal sealed record VoiceView(string Phase, string Live, string Status, string? Answer, string? Hint, bool Locked, bool Working, string? OpenUrl = null,
    string? Title = null, IReadOnlyList<CardStepRow>? Steps = null, CardApprovalBox? Approval = null, bool Failed = false, string? Footer = null);

/// <summary>
/// The native island (149 §3.6, §7.4 #1, D9–D11): a <see cref="ToolWindow"/> with the shell's Windows island styles
/// (<c>WS_EX_TOOLWINDOW | WS_EX_NOACTIVATE | WS_EX_TOPMOST</c>, shown without activation, <c>WDA_EXCLUDEFROMCAPTURE</c>),
/// opaque, rounded, dark-first. Closed: a pill at the top centre of the chosen monitor's work area — rec dot · <c>mm:ss</c> ·
/// the topic count. Open — island fix D1 (Sky 2026-10-09 15:50): OPEN whenever a meeting is live, like the Tauri original; no
/// hover model; the closed pill is the idle wordmark ("Always show") or an explicit collapse (the strip's ˄ or Esc), remembered
/// per meeting by <see cref="IslandModel"/>; a click on the collapsed pill opens it again. The strip
/// (topics with ✕, mute, stop, eye, collapse), the brief pane (title, ≤ 3 bullets, deeper, newer/older, <c>↑N new</c>), the collapsed
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
    private readonly ContentControl focusHost;
    private readonly Border shell;
    private readonly StackPanel body;
    private readonly DispatcherQueueTimer clockTimer;
    private readonly DispatcherQueueTimer dragTimer;
    private readonly DispatcherQueueTimer renderRetry;
    private readonly DispatcherQueueTimer fadeTimer;
    private readonly SUBCLASSPROC subclass149; // referenced for the window's lifetime
    private const nuint Subclass149Id = 0x149;
    private MeetingSnapshot snapshot = MeetingSnapshot.Empty;
    private VoiceView? voice;
    private TextBox? request;
    private TextBlock? clock;
    private TextBlock? transcript;
    private string? renderedKey;
    private string requestDraft = "";
    private bool requestFocused;
    private DateTimeOffset fadeStart;
    private bool hiding;
    private double dragScale = 1;
    private double dragStartScale = 1;
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
        // 149 NATIVE: the island follows Windows light/dark like every other surface (it was forced dark); the shell is the theme's
        // SolidBackgroundFillColorBase with a CardStroke hairline — an un-regioned NOACTIVATE tool window whose shape is the Border's own
        // rounded rectangle over a transparent root, so a system backdrop (which fills the whole HWND rectangle) cannot be used here.
        root = new Grid { Background = Theme.Transparent };
        AutomationProperties.SetAutomationId(root, "island");
        AutomationProperties.SetName(root, "theuth island");
        FlatButtons.Apply(root.Resources);
        body = new StackPanel { Orientation = Orientation.Vertical, Spacing = 6 };
        shell = new Border
        {
            Background = Theme.Brush(NativeLook.SolidBackgroundBase, ElementTheme.Default),
            BorderBrush = Theme.Brush(NativeLook.CardStroke, ElementTheme.Default),
            BorderThickness = new Thickness(1),
            CornerRadius = new CornerRadius(8),
            Padding = new Thickness(10, 4, 10, 4),
            Child = body,
            HorizontalAlignment = HorizontalAlignment.Stretch,
            VerticalAlignment = VerticalAlignment.Top,
        };
        root.Children.Add(shell);
        Theme.Follow(root, () =>
        {
            FlatButtons.Apply(root.Resources, theme: root.ActualTheme);
            shell.Background = Theme.Brush(NativeLook.SolidBackgroundBase, root.ActualTheme);
            shell.BorderBrush = Theme.Brush(NativeLook.CardStroke, root.ActualTheme);
            Render();
        });
        // Island fix §N: a focusable host around the content, so a click on the strip/pane can give the island keyboard focus (a Grid
        // cannot take focus) without focusing the request box; its KeyDown sees every key the island's controls do not eat.
        focusHost = new ContentControl
        {
            Content = root,
            IsTabStop = true,
            UseSystemFocusVisuals = false,
            HorizontalContentAlignment = HorizontalAlignment.Stretch,
            VerticalContentAlignment = VerticalAlignment.Stretch,
        };
        AutomationProperties.SetAutomationId(focusHost, "island-focus");
        Content = focusHost;
        // Island fix D1: no PointerEntered/PointerExited model. The pointer leaving never collapses the island (and the
        // inactive NOACTIVATE window drops pointer events anyway, which is why hover-driven state was unreliable here).
        root.PointerExited += (_, _) => model.PointerLeft();
        root.PointerPressed += OnPressed;
        root.PointerMoved += OnMoved;
        root.PointerReleased += OnReleased;
        focusHost.KeyDown += OnKeyDown;
        // Every press, a control's included: take the foreground so the request box and the island keys get the keyboard. The
        // NOACTIVATE window is not activated by a click on build-B (typed text went to the app behind it).
        root.AddHandler(UIElement.PointerPressedEvent, new PointerEventHandler((_, _) => TakeForeground()), handledEventsToo: true);
        // A re-render that lands while the left button is down over the island would replace the control under the press and
        // the click would never fire (a live meeting re-renders on every partial utterance): it waits for the release.
        renderRetry = Timer(40, () => Render());
        fadeTimer = Timer(16, FadeTick);
        fadeTimer.IsRepeating = true;
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
        if (hiding)
        {
            // A meeting went live again while the close animation ran: stay.
            hiding = false;
            fadeTimer.Stop();
            root.Opacity = 1;
            Log.Write("island: close animation cancelled (live again)");
        }
        Render();
        Relayout();
        if (!IsShown)
        {
            ShowPassive();
            dispatcher.TryEnqueue(DispatcherQueuePriority.Low, () => { root.InvalidateMeasure(); root.UpdateLayout(); Render(); });
            Log.Write($"island shown: rect={Win32.Text(ScreenRect)} on {m.Device} mode={model.Mode} exstyle={Win32.ExStyleText(Win32.ExStyle(Hwnd))} affinity=0x{Win32.DisplayAffinity(Hwnd):X}");
        }
        if (snapshot.State == "Recording" && !clockTimer.IsRunning) clockTimer.Start();
    }

    /// <summary>Whether the close animation is running (the window is still shown).</summary>
    public bool IsHiding => hiding;

    /// <summary>
    /// Island fix D2: the meeting ended (Done · Failed · anything not live) — a <see cref="IslandModel.CloseAnimation"/> fade, then
    /// the window is gone. With Windows' "Animation effects" off it goes at once.
    /// </summary>
    public void HideIsland()
    {
        if (!IsShown || hiding) return;
        clockTimer.Stop();
        EndDrag(snap: false);
        if (!SystemEffects.AnimationsOn)
        {
            FinishHide();
            return;
        }
        hiding = true;
        fadeStart = DateTimeOffset.Now;
        fadeTimer.Start();
        Log.Write($"island: closing ({IslandModel.CloseAnimation.TotalMilliseconds:0} ms fade)");
    }

    private void FadeTick()
    {
        if (!hiding) { fadeTimer.Stop(); return; }
        double t = (DateTimeOffset.Now - fadeStart) / IslandModel.CloseAnimation;
        if (t >= 1)
        {
            FinishHide();
            return;
        }
        root.Opacity = 1 - t * t; // ease-in
    }

    private void FinishHide()
    {
        hiding = false;
        fadeTimer.Stop();
        HideWindow();
        root.Opacity = 1;
        Log.Write("island hidden");
    }

    /// <summary>Re-reads the monitor list (<c>WM_DISPLAYCHANGE</c>) and places again.</summary>
    public void Relayout()
    {
        if (monitor is null) return;
        if (dragging)
        {
            // Mid-drag WM_DISPLAYCHANGE / WM_DPICHANGED (crossing onto a monitor of another scale): never re-place to the old
            // monitor's memory — that is what yanked a drag back off a second monitor. Keep the window under the cursor at the new
            // scale; the release decides the monitor and its memory.
            double s = Scale;
            if (s != dragScale)
            {
                (dragDx, dragDy) = IslandModel.ScaleGrab((dragDx, dragDy), dragScale, s);
                Log.Write($"island: drag crossed into scale {s:0.##} (was {dragScale:0.##}); grab offset now ({dragDx},{dragDy})");
                dragScale = s;
                var sz = CurrentSize();
                if (PInvoke.GetCursorPos(out var c)) Place(new RectInt32(c.X - dragDx, c.Y - dragDy, sz.Width, sz.Height));
            }
            return;
        }
        var all = ScreenMonitor.All();
        monitor = all.FirstOrDefault(x => x.Device == monitor.Device) ?? ScreenMonitor.At(all, 0, 0);
        var size = CurrentSize();
        var (frame, _) = voiceAnchor is { } va && va.Device == monitor.Device
            ? (IslandModel.DefaultFrame(va.AsIslandDisplay(), size), false) // §P: the voice pill is top-centre, never the saved offset
            : model.Place(monitor.AsIslandDisplay(), size);
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
        // Start only when stopped: Start() on a running DispatcherQueueTimer restarts its interval, and a talking meeting updates
        // several times a second — the clock would never tick.
        if (s.State == "Recording" && IsShown) { if (!clockTimer.IsRunning) clockTimer.Start(); } else clockTimer.Stop();
        // A partial utterance changes only the transcript row: update it in place instead of rebuilding every control (a
        // rebuild under a press loses the click, and a rebuild under typing loses the request box's focus).
        if (renderedKey is not null && renderedKey == RenderKey())
        {
            if (transcript is not null && s.Lines.Count > 0) { var l = s.Lines[^1]; transcript.Text = $"{l.Who} {l.Text}"; }
            clock?.SetValue(TextBlock.TextProperty, IslandModel.Clock(s.Elapsed(DateTimeOffset.Now)));
            return;
        }
        Render();
    }

    /// <summary>Whatever changes the island's controls (not the clock, the meters or the transcript text): equal keys = no rebuild.</summary>
    private string RenderKey()
    {
        var s = snapshot;
        var last = s.Oracle.Count > 0 ? s.Oracle[^1] : null;
        bool verdictFresh = last is not null && DateTimeOffset.Now - verdictAt < TimeSpan.FromSeconds(6);
        return string.Join('\u001f',
            model.Mode, hiddenFromCapture, root.ActualTheme, oracleMode, verdictFresh, last?.BatchId, last?.Disposition, s.Deciding,
            s.Meeting, s.State, string.Join(',', s.Topics.Select(t => t.Id + "=" + t.Label)), string.Join(',', s.Tapped),
            s.Cursor, s.Briefs.Count, s.Current?.Id, s.Unseen, string.Join(',', s.Degraded), s.Listening, s.ApprovalWaiting, s.Activity,
            s.Lines.Count > 0, s.Lines.Count > 0 && s.Lines[^1].Open, SpeakerWord(s), s.Prompt, s.Upcoming, s.Upcoming?.Headline(DateTimeOffset.Now));
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
        bool starting = v is not null && voice is null;
        string? phase = voice?.Phase;
        voice = v;
        bool wasAnchored = voiceAnchor is not null;
        if (v is null) voiceAnchor = null;
        if (starting && model.Meeting is null)
        {
            // §P (Sky 16:44): the live-dictation pill and the answer card sit in the island area — top centre of the display under the
            // pointer when the gesture starts — and stay there for the whole gesture; only the trail/ring overlays follow the pointer.
            var all = ScreenMonitor.All();
            voiceAnchor = PInvoke.GetCursorPos(out var c) ? ScreenMonitor.At(all, c.X, c.Y) : ScreenMonitor.At(all, 0, 0);
            monitor = voiceAnchor;
        }
        Render();
        if (v is null && wasAnchored && IsShown && !hiding) Relayout(); // back to the island's own place ("Always show")
        if (voiceAnchor is { } anchor && IsShown && !hiding)
        {
            var size = CurrentSize();
            var f = IslandModel.DefaultFrame(anchor.AsIslandDisplay(), size);
            Place(new RectInt32(f.X, f.Y, f.Width, f.Height));
        }
        voiceLogDue = v is not null && (starting || v.Phase != phase);
    }

    private bool voiceLogDue;

    /// <summary>After the controller showed/placed the window: one line per voice phase with the pill's rect (the §P proof).</summary>
    public void LogVoicePill()
    {
        if (!voiceLogDue || voice is not { } v) return;
        voiceLogDue = false;
        Log.Write($"island: voice pill phase={v.Phase} rect={Win32.Text(ScreenRect)} on {monitor?.Device} {(model.Meeting is null ? "(top centre, fixed for the gesture)" : "(below the meeting strip)")}");
    }

    /// <summary>§P: the display the voice gesture started on (no meeting live): the pill is placed top-centre there, ignoring the saved offset.</summary>
    private ScreenMonitor? voiceAnchor;

    public bool InVoiceMode => voice is not null;

    // MARK: click / collapse (island fix D1)

    /// <summary>The controller presented a new state (open ↔ closed for a live meeting): re-render at the new shape.</summary>
    public void Refresh() => Render();

    /// <summary>The explicit collapse (the strip's ˄, or Esc): the closed pill for this meeting only.</summary>
    private void CollapseIsland(string how)
    {
        if (!model.Collapse()) return;
        Log.Write($"island: collapsed by {how} (meeting {model.Meeting}; remembered for this meeting)");
        Render();
    }

    /// <summary>A click on the collapsed pill of a live meeting: open again (and forget this meeting's collapse).</summary>
    private void ExpandIsland()
    {
        if (voice is not null || !model.Expand()) return;
        Log.Write($"island: expanded by click (meeting {model.Meeting})");
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
            dragScale = dragStartScale = Scale;
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
        if (!wasMoved && voice is null && model.Mode == IslandMode.Closed)
        {
            // A click on the pill (not a drag): a live meeting's collapsed pill opens again; the idle wordmark ("Always show", nothing
            // live) deep-links to theuth://meetings.
            if (model.Meeting is not null) ExpandIsland();
            else { Log.Write("island: click island-pill (idle) → theuth://meetings"); act(new IslandAction.OpenMeetings()); HandBack(); return; }
        }
        // §N: a click on the strip or the pane focuses the island (not the request box), so J/K/↑/↓ work; a drag hands back.
        if (wasMoved) { if (!wantsKey) HandBack(); }
        else if (!wantsKey) FocusIsland();
    }

    private void OnReleased(object sender, PointerRoutedEventArgs e)
    {
        if (dragging) { e.Handled = true; return; } // the drag poll ends it
        if (!wantsKey) FocusIsland();
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
        Log.Write($"island drag ended: {(target == frame ? "kept" : "snapped back")} at {target.X},{target.Y} on {m.Device} mode={model.Mode} detached={model.Detached} frames={model.Frames.Count}");
        if (Scale != dragStartScale) Render(); // landed on a monitor of another scale: re-measure at its scale
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
            // 158 §5.3: Esc on a detection prompt is "Not now" (the island's existing escape path).
            if (inBox) LeaveBox();
            else if (snapshot.Prompt is MeetingPrompt.Detected pd) { Log.Write("island: key Esc → not now"); act(new IslandAction.NotNow(pd.Detection.AppId)); HandBack(); }
            else if (snapshot.Upcoming is not null) { Log.Write("island: key Esc → dismiss upcoming"); act(new IslandAction.DismissUpcoming()); HandBack(); }
            else { CollapseIsland("Esc"); HandBack(); }
            e.Handled = true;
            return;
        }
        if (key.KeyCode == VirtualKeys.Return && !inBox && snapshot.Prompt is { } pr)
        {
            // 158 §5.3: ⏎ = Start recording on (a); ⏎ = Keep recording on (d) — the safe default; Ctrl+M (above) is Stop.
            switch (pr)
            {
                case MeetingPrompt.Detected d: Log.Write("island: key Enter → start recording"); act(new IslandAction.StartRecording(d.Detection)); break;
                case MeetingPrompt.Stop: Log.Write("island: key Enter → keep recording"); act(new IslandAction.KeepRecording()); break;
            }
            e.Handled = true;
            return;
        }
        if (key.KeyCode == VirtualKeys.Return && !inBox && snapshot.Upcoming is { } up)
        {
            // 158 §5.2 (c): ⏎ = Start recording from the upcoming brief (when no detection prompt took it above).
            Log.Write("island: key Enter → start recording (upcoming)");
            act(new IslandAction.StartUpcoming(up));
            e.Handled = true;
            return;
        }
        if (voice is { OpenUrl: { } url } && key.Modifiers.HasFlag(ChordModifiers.Control) && key.KeyCode == 'O')
        {
            _ = Links.Open(new Uri(url)); // Ctrl+O: the desk's session
            e.Handled = true;
            return;
        }
        if (key.Modifiers.HasFlag(ChordModifiers.Control) && !key.Modifiers.HasFlag(ChordModifiers.Alt) && key.KeyCode == 'M')
        {
            // Island fix: Ctrl+M stops the meeting from the request box too — the box is the only place the NOACTIVATE island
            // keeps keyboard focus (every other press hands the foreground back), so the island-scope key was unreachable.
            Log.Write($"island: key Ctrl+M → stop{(inBox ? " (from the request box)" : "")}");
            act(new IslandAction.Stop());
            e.Handled = true;
            return;
        }
        bool plain = key.Modifiers == ChordModifiers.None;
        if (inBox)
        {
            if (key.KeyCode == VirtualKeys.Return && request is not null)
            {
                if (request.Text.Trim().Length > 0) { Log.Write("island: key Enter island-request"); SendRequest(); }
                e.Handled = true;
            }
            // §N: J/K type letters here; ↓/↑ walk the briefs only while the box is empty (else they move the caret).
            else if (plain && (key.KeyCode == VirtualKeys.Down || key.KeyCode == VirtualKeys.Up) && request is { Text.Length: 0 })
            {
                Walk(key.KeyCode == VirtualKeys.Down ? +1 : -1, key.KeyCode == VirtualKeys.Down ? "Down (empty box)" : "Up (empty box)");
                e.Handled = true;
            }
            return;
        }
        if (voice is null && plain && (key.KeyCode == VirtualKeys.Down || key.KeyCode == VirtualKeys.Up))
        {
            Walk(key.KeyCode == VirtualKeys.Down ? +1 : -1, key.KeyCode == VirtualKeys.Down ? "Down" : "Up");
            e.Handled = true;
            return;
        }
        if (key.Modifiers != ChordModifiers.None && key.Modifiers != ChordModifiers.Shift) return;
        switch (key.Chars.ToLowerInvariant())
        {
            case "j" when voice is null: Walk(+1, "J"); e.Handled = true; break;
            case "k" when voice is null: Walk(-1, "K"); e.Handled = true; break;
            case "m": act(new IslandAction.Mute()); e.Handled = true; break;
            case "e":
                if (snapshot.Topics.Count > 0) act(new IslandAction.Dismiss(snapshot.Topics[0].Id));
                e.Handled = true;
                break;
            case "d":
                if (snapshot.Current is { } b) act(new IslandAction.Deeper(b.Id));
                e.Handled = true;
                break;
            case "n" when snapshot.Prompt is MeetingPrompt.Detected nd: // 158 §5.3
                act(new IslandAction.NeverForApp(nd.Detection.AppId));
                e.Handled = true;
                break;
        }
    }

    /// <summary>§N: J/↓ next, K/↑ previous brief, wrapping — the controller's one brief-walk path (the same as a pill click).</summary>
    private void Walk(int delta, string via) => act(new IslandAction.Cycle(delta, via));

    private void LeaveBox()
    {
        wantsKey = false;
        focusHost.Focus(FocusState.Programmatic);
        HandBack();
    }

    // MARK: render

    private void Render()
    {
        renderRetry.Stop();
        if (IsShown && !dragging && PressedOverIsland())
        {
            renderRetry.Start(); // after the release: the control under the press must survive until its Click
            return;
        }
        // The request box survives a rebuild: its text, its focus.
        if (request is not null)
        {
            requestDraft = request.Text;
            requestFocused = request.FocusState != FocusState.Unfocused;
        }
        body.Children.Clear();
        request = null;
        clock = null;
        transcript = null;
        micBar = null;
        sysBar = null;
        if (voice is { } v)
        {
            // §P: during a voice gesture with a live meeting the pill sits directly below the meeting's strip.
            if (model.Meeting is not null) { shell.CornerRadius = new CornerRadius(8); RenderStrip(); }
            RenderVoice(v, padShell: model.Meeting is null);
        }
        else if (model.Mode == IslandMode.Closed) RenderPill();
        else RenderOpen();
        renderedKey = voice is null ? RenderKey() : null;
        if (request is { } box)
        {
            box.Text = requestDraft;
            box.SelectionStart = box.Text.Length;
            if (requestFocused) box.Focus(FocusState.Programmatic);
        }
        if (IsShown) Resize();
    }

    /// <summary>The left button is down and the cursor is over this window.</summary>
    private bool PressedOverIsland()
    {
        if ((PInvoke.GetAsyncKeyState(0x01) & 0x8000) == 0 || !PInvoke.GetCursorPos(out var c)) return false;
        var r = ScreenRect;
        return c.X >= r.left && c.X < r.right && c.Y >= r.top && c.Y < r.bottom;
    }

    private TextBlock Text(string s, double size, string key, FontWeight? weight = null) => new()
    {
        Text = s,
        FontSize = size <= 12.5 ? 12 : size <= 16 ? 14 : 20, // the Fluent ramp: caption 12 · body 14 · subtitle 20
        Foreground = Theme.Brush(key, root.ActualTheme),
        VerticalAlignment = VerticalAlignment.Center,
        TextTrimming = TextTrimming.CharacterEllipsis,
        FontWeight = weight ?? FontWeights.Normal,
    };

    private Button Btn(string label, string tip, Action click, string? id = null)
    {
        var b = new Button
        {
            Content = Text(label, 12.5, NativeLook.TextPrimary),
            Padding = new Thickness(8, 2, 8, 2),
            MinHeight = 24,
            CornerRadius = new CornerRadius(4),
            VerticalAlignment = VerticalAlignment.Center,
        };
        ToolTipService.SetToolTip(b, tip);
        if (id is not null) AutomationProperties.SetAutomationId(b, id);
        AutomationProperties.SetName(b, tip);
        b.Click += (_, _) => Clicked(id, click);
        return b;
    }

    /// <summary>
    /// Every island control: one log line per click (the D3 proof), then the action. Island fix §N: a click on the island keeps the
    /// keyboard focus on the island (not the request box) so J/K/↑/↓ walk the briefs; Esc hands the foreground back.
    /// </summary>
    private void Clicked(string? id, Action click)
    {
        Log.Write($"island: click {id ?? "button"} (state={snapshot.State} mode={model.Mode})");
        click();
        if (!wantsKey) FocusIsland();
    }

    private void FocusIsland()
    {
        if (voice is not null) { HandBack(); return; }
        TakeForeground();
        if (request is null || request.FocusState == FocusState.Unfocused) focusHost.Focus(FocusState.Programmatic);
    }

    /// <summary>The island becomes the foreground window (remembering the one it took it from, for Esc / a drag's hand-back).</summary>
    private void TakeForeground()
    {
        if (IsForeground) return;
        var fg = PInvoke.GetForegroundWindow();
        if (!fg.IsNull && fg != Hwnd && !Win32.IsOurs(fg)) previous = fg;
        bool ok = PInvoke.SetForegroundWindow(Hwnd);
        Log.Write($"island: took the foreground for keys ({(ok || IsForeground ? "ok" : "refused")})");
    }

    /// <summary>149 NATIVE: a Fluent glyph button (eye, dismiss, chevrons), optionally with a word beside the glyph.</summary>
    private Button IconBtn(string glyph, string tip, Action click, string? id = null, string? word = null, bool iconFirst = true, string? tint = null)
    {
        var row = new StackPanel { Orientation = Orientation.Horizontal, Spacing = 4, VerticalAlignment = VerticalAlignment.Center };
        var icon = Theme.Icon(glyph, root.ActualTheme, tint ?? NativeLook.TextPrimary, 12);
        if (iconFirst) row.Children.Add(icon);
        if (word is not null) row.Children.Add(Text(word, 12.5, NativeLook.TextPrimary));
        if (!iconFirst) row.Children.Add(icon);
        var b = new Button { Content = row, Padding = new Thickness(8, 2, 8, 2), MinHeight = 24, CornerRadius = new CornerRadius(4), VerticalAlignment = VerticalAlignment.Center };
        ToolTipService.SetToolTip(b, tip);
        if (id is not null) AutomationProperties.SetAutomationId(b, id);
        AutomationProperties.SetName(b, tip);
        b.Click += (_, _) => Clicked(id, click);
        return b;
    }

    /// <summary>The eye's exposure state (Sky 16:06), shared with Settings › Island: the glyph, its tint resource and its words.</summary>
    internal static string EyeGlyph(bool hidden) => hidden ? NativeLook.Glyph.Hide : NativeLook.Glyph.View;

    internal static string EyeTint(bool hidden) => hidden ? NativeLook.Success : NativeLook.Caution;

    internal static string EyeLabel(bool hidden) => hidden ? "Hidden from screen sharing (best effort)" : "Visible in screen sharing — viewers can see the island";

    /// <summary>"N new" with an Up glyph (not an arrow character), in the accent: unseen briefs ask for attention.</summary>
    private FrameworkElement Unseen(int n)
    {
        var row = Row(2);
        row.Children.Add(Theme.Icon(NativeLook.Glyph.Up, root.ActualTheme, NativeLook.AccentText, 12));
        row.Children.Add(Text($"{n} new", 11.5, NativeLook.AccentText));
        return row;
    }

    private Ellipse Dot(string key, double size = 8) => new()
    {
        Width = size, Height = size, Fill = Theme.Brush(key, root.ActualTheme), VerticalAlignment = VerticalAlignment.Center,
    };

    private StackPanel Row(double spacing = 8) => new() { Orientation = Orientation.Horizontal, Spacing = spacing, VerticalAlignment = VerticalAlignment.Center };

    /// <summary>The meters: mic (faience) and system (sand) bars, 3 epx wide, 3–14 epx tall, bottom-aligned.</summary>
    private StackPanel Meters()
    {
        var p = new StackPanel { Orientation = Orientation.Horizontal, Spacing = 2, VerticalAlignment = VerticalAlignment.Center, Height = 14 };
        AutomationProperties.SetAutomationId(p, "island-meters");
        micBar = new Rectangle { Width = 3, Height = MeterHeight(micLevel), RadiusX = 1.5, RadiusY = 1.5, Fill = Theme.Brush(NativeLook.Accent, root.ActualTheme), VerticalAlignment = VerticalAlignment.Bottom };
        sysBar = new Rectangle { Width = 3, Height = MeterHeight(sysLevel), RadiusX = 1.5, RadiusY = 1.5, Fill = Theme.Brush(NativeLook.TextTertiary, root.ActualTheme), VerticalAlignment = VerticalAlignment.Bottom };
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
            ? last.Disposition == "skip" ? "skip" : last.Disposition == "brief" ? "brief" : "to model" : null;
        var row = Row(4);
        if (effective != "off") row.Children.Add(Text(effective.ToUpperInvariant(), 10, NativeLook.TextTertiary));
        if (verdict is not null) row.Children.Add(Text(verdict, 10.5, s.Deciding is not null ? NativeLook.AccentText : NativeLook.TextSecondary));
        var chip = new Border
        {
            Background = Theme.Brush(NativeLook.ControlFill, root.ActualTheme),
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
        shell.CornerRadius = new CornerRadius(8);
        shell.Padding = new Thickness(12, 0, 12, 0);
        var row = Row();
        row.Height = PillHeight - 2;
        AutomationProperties.SetAutomationId(row, "island-pill");
        var s = snapshot;
        if (s.Meeting is not null) row.Children.Add(Meters());
        if (SpeakerWord(s) is { } who) row.Children.Add(Text(who, 12, NativeLook.TextSecondary));
        if (s.State == "Recording") row.Children.Add(Dot(NativeLook.Critical));
        else if (s.State == "Paused") row.Children.Add(Dot(NativeLook.TextTertiary));
        else row.Children.Add(Dot(NativeLook.Accent, 6));
        clock = Text(s.Meeting is null ? "no meeting" : IslandModel.Clock(s.Elapsed(DateTimeOffset.Now)), 13, NativeLook.TextPrimary, FontWeights.SemiBold);
        AutomationProperties.SetAutomationId(clock, "island-clock");
        row.Children.Add(clock);
        if (s.State is "Paused") row.Children.Add(Text("paused", 12, NativeLook.TextSecondary));
        else if (s.State is "Starting" or "Stopping") row.Children.Add(Text(s.State.ToLowerInvariant() + "…", 12, NativeLook.TextSecondary));
        if (s.Listening) row.Children.Add(Text("listening…", 12, NativeLook.AccentText));
        else if (s.ApprovalWaiting) row.Children.Add(Text("approval waiting", 12, NativeLook.AccentText));
        else if (s.Activity is { } a) row.Children.Add(Text(a, 12, NativeLook.TextSecondary));
        var spacer = new Border { Width = 1 };
        row.Children.Add(spacer);
        int n = s.Topics.Count;
        var count = new Border
        {
            Background = n > 0 ? Theme.Brush(NativeLook.Accent, root.ActualTheme) : Theme.Brush(NativeLook.ControlFillSecondary, root.ActualTheme),
            CornerRadius = new CornerRadius(8),
            Padding = new Thickness(7, 0, 7, 1),
            VerticalAlignment = VerticalAlignment.Center,
            Child = n > 0 ? OnAccentText(n == 1 ? "1 topic" : $"{n} topics", 11.5, FontWeights.SemiBold) : Text($"{n} topics", 11.5, NativeLook.TextSecondary, FontWeights.SemiBold),
        };
        AutomationProperties.SetAutomationId(count, "island-topics");
        row.Children.Add(count);
        if (s.Unseen > 0) row.Children.Add(Unseen(s.Unseen));
        body.Children.Add(row);
    }

    private void RenderOpen()
    {
        shell.CornerRadius = new CornerRadius(8);
        shell.Padding = new Thickness(10, 6, 10, 8);
        var s = snapshot;
        // 158 §5.2: (c) the upcoming brief stands alone or above (a)/(b) — or, when both name the same event, merges into the detection
        // prompt (one pane, W3M parity); (a)/(b) the detection prompt stands alone before a meeting; (d) the stop prompt sits under the
        // live strip and pane.
        if (s.Meeting is not null) RenderStrip();
        bool upcomingShown = s.Upcoming is not null && !s.UpcomingMerged;
        if (upcomingShown) RenderUpcoming(s.Upcoming!, head: s.Meeting is null);
        switch (s.Prompt)
        {
            case MeetingPrompt.Detected: RenderDetected(s.DetectedShown!, head: s.Meeting is null && !upcomingShown); break;
            case MeetingPrompt.Stop st: RenderPane(s); RenderStopPrompt(st); break;
            default: if (!upcomingShown) RenderPane(s); break;
        }
    }

    /// <summary>The prompt's head row: the grip and the eye when there is no strip above (<paramref name="head"/>), the title row between them.</summary>
    private Grid PromptHead(StackPanel title, bool head)
    {
        if (head)
        {
            var grip = new Border { Width = GripWidth, Height = 20, Child = Theme.Icon(NativeLook.Glyph.GripperBarVertical, root.ActualTheme, NativeLook.TextTertiary, 12), VerticalAlignment = VerticalAlignment.Center };
            ToolTipService.SetToolTip(grip, "Drag to move · double-click to snap back");
            title.Children.Insert(0, grip);
        }
        var grid = new Grid();
        grid.ColumnDefinitions.Add(new ColumnDefinition { Width = new GridLength(1, GridUnitType.Star) });
        grid.ColumnDefinitions.Add(new ColumnDefinition { Width = GridLength.Auto });
        grid.Children.Add(title);
        if (head)
        {
            var eye = IconBtn(EyeGlyph(hiddenFromCapture), EyeLabel(hiddenFromCapture) + " — Alt+Shift+E", () => act(new IslandAction.Eye()), "island-eye", tint: EyeTint(hiddenFromCapture));
            Grid.SetColumn(eye, 1);
            grid.Children.Add(eye);
        }
        return grid;
    }

    /// <summary>158 §5.2 (b)/(c): the project chip <c>⟨ project ⟩</c>. Not a button: the project list (§9) has no island surface on Windows.</summary>
    private Border ProjectChip(string project, string id)
    {
        var chip = Badge(project, NativeLook.Accent);
        AutomationProperties.SetAutomationId(chip, id);
        AutomationProperties.SetName(chip, $"Project {project}");
        return chip;
    }

    /// <summary>R3: the lawful sentence, verbatim, wrapped.</summary>
    private TextBlock Lawful(string id)
    {
        var lawful = Text("By starting the recording you acknowledge the recording is lawful.", 11.5, NativeLook.TextTertiary);
        lawful.TextWrapping = TextWrapping.Wrap;
        lawful.TextTrimming = TextTrimming.None;
        AutomationProperties.SetAutomationId(lawful, id);
        return lawful;
    }

    /// <summary>
    /// 158 §5.2 (c): <c>In N min · &lt;Title&gt;</c> + the project chip · up to three brief bullets · the source line · Start recording · Dismiss ·
    /// the lawful line. ⏎ = Start, ⎋ = Dismiss (when no detection prompt is up).
    /// </summary>
    private void RenderUpcoming(MeetingUpcoming u, bool head)
    {
        string headline = u.Headline(DateTimeOffset.Now);
        var group = new StackPanel { Spacing = 6 };
        AutomationProperties.SetAutomationId(group, "island-upcoming");
        AutomationProperties.SetName(group, headline);
        var title = Row(6);
        title.Children.Add(Dot(NativeLook.Accent));
        var line = Text(headline, 14, NativeLook.TextPrimary, FontWeights.SemiBold);
        AutomationProperties.SetAutomationId(line, "island-upcoming-title");
        title.Children.Add(line);
        if (u.Project is { Length: > 0 } project) title.Children.Add(ProjectChip(project, "island-upcoming-project"));
        group.Children.Add(PromptHead(title, head));
        foreach (var bullet in u.Brief.Take(3)) group.Children.Add(Bullet(bullet));
        if (u.SourceLine is { } src)
        {
            var t = Text(src, 11.5, NativeLook.TextTertiary);
            t.TextWrapping = TextWrapping.Wrap;
            t.TextTrimming = TextTrimming.None;
            AutomationProperties.SetAutomationId(t, "island-upcoming-source");
            group.Children.Add(t);
        }
        var buttons = Row(6);
        var start = Btn("Start recording", "Start recording (Enter)", () => act(new IslandAction.StartUpcoming(u)), "island-upcoming-start");
        start.Background = Theme.Brush(NativeLook.Accent, root.ActualTheme);
        start.Content = OnAccentText("Start recording", 12.5, FontWeights.SemiBold);
        buttons.Children.Add(start);
        buttons.Children.Add(Btn("Dismiss", "Dismiss this brief (Esc)", () => act(new IslandAction.DismissUpcoming()), "island-upcoming-dismiss"));
        group.Children.Add(buttons);
        group.Children.Add(Lawful("island-upcoming-lawful"));
        body.Children.Add(group);
    }

    /// <summary>
    /// 158 §5.2 (a)/(b): <c>&lt;App&gt; is using your microphone</c> · (b) <c>&lt;Title&gt; · &lt;hh:mm&gt;–&lt;hh:mm&gt;</c> + the project chip · Start
    /// recording · Not now · Never for &lt;App&gt; · the lawful line verbatim (R3). With no meeting the head row carries the grip and the
    /// eye (there is no strip). "Never for" is a button, not a ⋮ menu: a <c>MenuFlyout</c> hosted in a tool window fail-fasts on build-B
    /// (<c>TrayMenu.cs</c>), and the mock draws three buttons.
    /// </summary>
    private void RenderDetected(MeetingDetection d, bool head)
    {
        string app = d.App.Display;
        var group = new StackPanel { Spacing = 6 };
        AutomationProperties.SetAutomationId(group, "island-prompt");
        AutomationProperties.SetName(group, $"{app} is using your microphone");
        var title = Row(6);
        title.Children.Add(Dot(NativeLook.Critical));
        var line = Text($"{app} is using your microphone", 14, NativeLook.TextPrimary, FontWeights.SemiBold);
        AutomationProperties.SetAutomationId(line, "island-prompt-title");
        title.Children.Add(line);
        if (d.Also.Count > 0) title.Children.Add(Text($"+{d.Also.Count} more", 12, NativeLook.TextTertiary));
        group.Children.Add(PromptHead(title, head));
        if (d.Event is not null || d.Project is { Length: > 0 })
        {
            var ev = Row(6);
            if (d.Event is { } e)
            {
                var evt = Text(e.Line, 12.5, NativeLook.TextSecondary);
                AutomationProperties.SetAutomationId(evt, "island-prompt-event");
                ev.Children.Add(evt);
            }
            if (d.Project is { Length: > 0 } project) ev.Children.Add(ProjectChip(project, "island-prompt-project"));
            group.Children.Add(ev);
        }
        // The merged upcoming brief (or the detection's own cached brief, §9.4): the bullets once, on this pane.
        if (d.Brief is { Count: > 0 } bullets) foreach (var bullet in bullets.Take(3)) group.Children.Add(Bullet(bullet));
        var buttons = Row(6);
        var start = Btn("Start recording", "Start recording (Enter)", () => act(new IslandAction.StartRecording(d)), "island-prompt-start");
        start.Background = Theme.Brush(NativeLook.Accent, root.ActualTheme);
        start.Content = OnAccentText("Start recording", 12.5, FontWeights.SemiBold);
        buttons.Children.Add(start);
        buttons.Children.Add(Btn("Not now", "Not now — ask again in 10 minutes (Esc)", () => act(new IslandAction.NotNow(d.AppId)), "island-prompt-not-now"));
        buttons.Children.Add(Btn($"Never for {app}", $"Never ask for {app} again (N)", () => act(new IslandAction.NeverForApp(d.AppId)), "island-prompt-never"));
        group.Children.Add(buttons);
        group.Children.Add(Lawful("island-prompt-lawful"));
        body.Children.Add(group);
    }

    /// <summary>158 §5.2 (d): <c>&lt;App&gt; stopped using your microphone N s ago</c> · Stop recording? · Stop · Keep recording (⏎ = Keep).</summary>
    private void RenderStopPrompt(MeetingPrompt.Stop st)
    {
        var group = new StackPanel { Spacing = 4 };
        AutomationProperties.SetAutomationId(group, "island-stop-prompt");
        string ago = st.ReleasedForS >= 120 ? $"{st.ReleasedForS / 60} min ago" : $"{st.ReleasedForS} s ago";
        var line = Text($"{st.App.Display} stopped using your microphone {ago}", 12.5, NativeLook.TextSecondary);
        AutomationProperties.SetAutomationId(line, "island-stop-prompt-title");
        AutomationProperties.SetName(group, line.Text);
        group.Children.Add(line);
        var row = Row(6);
        row.Children.Add(Text("Stop recording?", 12.5, NativeLook.TextPrimary, FontWeights.SemiBold));
        row.Children.Add(Btn("Stop", "Stop the meeting (Ctrl+M)", () => act(new IslandAction.Stop()), "island-stop-prompt-stop"));
        row.Children.Add(Btn("Keep recording", "Keep recording (Enter)", () => act(new IslandAction.KeepRecording()), "island-stop-prompt-keep"));
        group.Children.Add(row);
        body.Children.Add(group);
    }

    /// <summary>The meeting strip (and its badge rows): the open island's head, and what the voice pill sits under while a meeting is live (§P).</summary>
    private void RenderStrip()
    {
        var s = snapshot;
        // The strip: grip · rec · clock · topics · badges · mute · stop · eye.
        var strip = Row(6);
        var grip = new Border { Width = GripWidth, Height = 20, Child = Theme.Icon(NativeLook.Glyph.GripperBarVertical, root.ActualTheme, NativeLook.TextTertiary, 12), VerticalAlignment = VerticalAlignment.Center };
        ToolTipService.SetToolTip(grip, "Drag to move · double-click to snap back");
        strip.Children.Add(grip);
        if (s.Meeting is not null) strip.Children.Add(Meters());
        strip.Children.Add(Dot(s.State == "Recording" ? NativeLook.Critical : NativeLook.TextTertiary));
        clock = Text(s.Meeting is null ? "no meeting" : IslandModel.Clock(s.Elapsed(DateTimeOffset.Now)), 13, NativeLook.TextPrimary, FontWeights.SemiBold);
        AutomationProperties.SetAutomationId(clock, "island-clock");
        strip.Children.Add(clock);
        if (OracleChip() is { } oracle) strip.Children.Add(oracle);
        // The degradation badges and the activity line sit on their own row under the strip: in the strip they ran under the
        // mute/stop/eye buttons (build-B, "no echo cancel · model down") and covered them.
        var badges = new StackPanel { Orientation = Orientation.Vertical, Spacing = 4 };
        StackPanel? badgeRow = null;
        void AddBadge(FrameworkElement b)
        {
            // Two per row: every label fits whole at the island's 440 epx (R1: "model do…" was a hard cut in the strip).
            if (badgeRow is null || badgeRow.Children.Count == 2) { badgeRow = Row(4); badges.Children.Add(badgeRow); }
            badgeRow.Children.Add(b);
        }
        foreach (var d in s.Degraded.Take(4)) AddBadge(Badge(Degraded(d), NativeLook.Accent));
        if (s.Listening) strip.Children.Add(Badge("listening…", NativeLook.Accent));
        else if (s.ApprovalWaiting) strip.Children.Add(Badge("approval waiting", NativeLook.Accent));
        else if (s.Activity is { } a) { var act0 = Text(a, 12, NativeLook.TextSecondary); ToolTipService.SetToolTip(act0, a); badges.Children.Add(act0); }
        var right = Row(2);
        right.HorizontalAlignment = HorizontalAlignment.Right;
        if (s.Meeting is not null && MeetingReducer.Live(s.State))
        {
            right.Children.Add(Btn(s.State == "Paused" ? "resume" : "mute", s.State == "Paused" ? "Resume (M)" : "Mute — pause the recording (M)", () => act(new IslandAction.Mute()), "island-mute"));
            right.Children.Add(Btn("stop", "Stop the meeting (Ctrl+Shift+M)", () => act(new IslandAction.Stop()), "island-stop"));
        }
        // Sky 16:06: exposure at a glance — visible to screen sharing = the eye in Caution, hidden = the eye-slash in Success.
        right.Children.Add(IconBtn(EyeGlyph(hiddenFromCapture), EyeLabel(hiddenFromCapture) + " — Alt+Shift+E", () => act(new IslandAction.Eye()), "island-eye", tint: EyeTint(hiddenFromCapture)));
        if (s.Meeting is not null && model.Meeting is not null)
            right.Children.Add(IconBtn(NativeLook.Glyph.ChevronUp, "Collapse the island (Esc) — for this meeting", () => CollapseIsland("click"), "island-collapse"));
        var stripGrid = new Grid();
        stripGrid.ColumnDefinitions.Add(new ColumnDefinition { Width = new GridLength(1, GridUnitType.Star) });
        stripGrid.ColumnDefinitions.Add(new ColumnDefinition { Width = GridLength.Auto });
        Grid.SetColumn(right, 1);
        stripGrid.Children.Add(strip);
        stripGrid.Children.Add(right);
        body.Children.Add(stripGrid);
        if (badges.Children.Count > 0) body.Children.Add(badges);
    }

    private void RenderPane(MeetingSnapshot s)
    {

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
                tap.Background = Theme.Brush(NativeLook.CardBackgroundSecondary, root.ActualTheme);
                var x = IconBtn(NativeLook.Glyph.Cancel, $"Dismiss \"{t.Label}\" (E)", () => act(new IslandAction.Dismiss(t.Id)), $"island-topic-{i}-dismiss");
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
            var title = Text(b.Title, 14, NativeLook.TextPrimary, FontWeights.SemiBold);
            AutomationProperties.SetAutomationId(title, "island-brief-title");
            head.Children.Add(title);
            head.Children.Add(Text($"{s.Cursor + 1}/{s.Briefs.Count}", 11.5, NativeLook.TextTertiary));
            if (s.Unseen > 0) head.Children.Add(Unseen(s.Unseen));
            pane.Children.Add(head);
            foreach (var bullet in b.Bullets.Take(3)) pane.Children.Add(Bullet(bullet));
            var nav = Row(4);
            nav.Children.Add(Btn("deeper", "Go deeper on this brief (D)", () => act(new IslandAction.Deeper(b.Id)), "island-deeper"));
            if (s.Cursor > 0) nav.Children.Add(IconBtn(NativeLook.Glyph.ChevronLeft, "Older brief", () => act(new IslandAction.Move(-1)), "island-older", "older"));
            if (s.Cursor < s.Briefs.Count - 1) nav.Children.Add(IconBtn(NativeLook.Glyph.ChevronRight, "Newer brief", () => act(new IslandAction.Move(+1)), "island-newer", "newer", iconFirst: false));
            nav.Children.Add(Text(b.Source.ToLowerInvariant(), 11, NativeLook.TextTertiary));
            pane.Children.Add(nav);
            body.Children.Add(pane);
        }
        else if (s.Meeting is not null)
        {
            body.Children.Add(Text(s.Topics.Count > 0 ? "Tap a topic for a brief." : "Listening for topics…", 12.5, NativeLook.TextTertiary));
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
            var line = Text($"{l.Who} {l.Text}", 11.5, l.Open ? NativeLook.TextTertiary : NativeLook.TextSecondary);
            AutomationProperties.SetAutomationId(line, "island-transcript");
            body.Children.Add(line);
            transcript = line;
        }

        // The request box.
        if (s.Meeting is not null)
        {
            request = new TextBox
            {
                PlaceholderText = "Ask about the meeting… (Enter sends, Esc leaves)",
                FontSize = 12,
                MinHeight = 28,
                CornerRadius = new CornerRadius(8),
            };
            AutomationProperties.SetAutomationId(request, "island-request");
            request.GotFocus += (_, _) => { wantsKey = true; requestFocused = true; };
            request.LostFocus += (_, _) => { wantsKey = false; requestFocused = false; };
            request.TextChanged += (_, _) => { if (request is { } rb) requestDraft = rb.Text; };
            // D3: the send arrow beside the box (Enter does the same).
            var send = IconBtn(NativeLook.Glyph.Up, "Send to the meeting (Enter)", SendRequest, "island-send");
            send.VerticalAlignment = VerticalAlignment.Stretch;
            var boxRow = new Grid { ColumnSpacing = 4 };
            boxRow.ColumnDefinitions.Add(new ColumnDefinition { Width = new GridLength(1, GridUnitType.Star) });
            boxRow.ColumnDefinitions.Add(new ColumnDefinition { Width = GridLength.Auto });
            Grid.SetColumn(send, 1);
            boxRow.Children.Add(request);
            boxRow.Children.Add(send);
            body.Children.Add(boxRow);
        }
    }

    /// <summary>The request box's text → <c>meeting_request</c> (the send arrow and Enter).</summary>
    private void SendRequest()
    {
        var text = (request?.Text ?? requestDraft).Trim();
        if (text.Length == 0) return;
        act(new IslandAction.Request(text));
        if (request is not null) request.Text = "";
        requestDraft = "";
    }

    private void RenderVoice(VoiceView v, bool padShell = true)
    {
        shell.CornerRadius = new CornerRadius(8);
        if (padShell) shell.Padding = new Thickness(14, 8, 14, 10);
        else shell.Padding = new Thickness(10, 6, 10, 10);
        var pane = new StackPanel { Spacing = 4, Background = Theme.Transparent };
        AutomationProperties.SetAutomationId(pane, "island-voice");
        // 153 §12.2/§12.3: a click (or any scroll) on the collapsed card expands it into the answer card window.
        pane.Tapped += (_, e) => { if (e.OriginalSource is not Button) VoiceTapped?.Invoke(); };
        pane.PointerWheelChanged += (_, _) => VoiceTapped?.Invoke();
        var head = Row(8);
        head.Children.Add(Dot(v.Failed ? NativeLook.Critical : v.Phase == "listening" ? NativeLook.Critical : v.Working ? NativeLook.Accent : NativeLook.Success));
        if (v.Title is { Length: > 0 } title)
        {
            var t = Text(title, 12.5, NativeLook.TextPrimary, FontWeights.SemiBold);
            AutomationProperties.SetAutomationId(t, "island-voice-title");
            head.Children.Add(t);
        }
        var status = Text(v.Status, 12.5, v.Failed ? NativeLook.Critical : NativeLook.TextSecondary);
        AutomationProperties.SetAutomationId(status, "island-voice-status");
        head.Children.Add(status);
        if (v.Locked) head.Children.Add(Badge("locked", NativeLook.Accent));
        pane.Children.Add(head);
        if (v.Live.Length > 0)
        {
            var live = Text(v.Live, 15, NativeLook.TextPrimary);
            live.TextWrapping = TextWrapping.Wrap;
            live.TextTrimming = TextTrimming.None;
            live.MaxLines = 4;
            AutomationProperties.SetAutomationId(live, "island-voice-live");
            pane.Children.Add(live);
        }
        if (v.Steps is { Count: > 0 } steps)
        {
            // 153 §10.3 / §10.7: the command card's step rows in the island's voice view (the collapsed form); the last five.
            var list = new StackPanel { Spacing = 1 };
            AutomationProperties.SetAutomationId(list, "island-voice-steps");
            int n = 0;
            foreach (var s in steps.Skip(Math.Max(0, steps.Count - 5)))
            {
                var row = Row(8);
                string key = s.Status switch
                {
                    StepStatus.Ok => NativeLook.Success,
                    StepStatus.Failed or StepStatus.Denied => NativeLook.Critical,
                    StepStatus.Running => NativeLook.AccentText,
                    _ => NativeLook.TextTertiary,
                };
                var verb = Text(s.Verb, 11.5, key, FontWeights.SemiBold);
                verb.Width = 64;
                row.Children.Add(verb);
                var text = Text(s.Note is { Length: > 0 } note ? $"{s.Text} — {note}" : s.Text, 12, NativeLook.TextPrimary);
                text.MaxWidth = OpenWidth - 120;
                row.Children.Add(text);
                if (s.Status == StepStatus.Running) row.Children.Add(new ProgressRing { IsActive = true, Width = 10, Height = 10, MinWidth = 10, MinHeight = 10 });
                AutomationProperties.SetAutomationId(row, $"island-voice-step-{++n}");
                list.Children.Add(row);
            }
            pane.Children.Add(list);
        }
        if (v.Approval is { } box)
        {
            // 153 §10.6: the amber approval box; Yes/No are real buttons (the island is not key), Esc denies, the voice answers too.
            var inner = new StackPanel { Spacing = 3 };
            var q = Text(box.Question, 12.5, NativeLook.TextPrimary, FontWeights.SemiBold);
            q.TextWrapping = TextWrapping.Wrap;
            q.TextTrimming = TextTrimming.None;
            q.FontFamily = new FontFamily("Cascadia Mono, Consolas");
            inner.Children.Add(q);
            inner.Children.Add(Text(box.RiskLine, 11.5, NativeLook.TextSecondary));
            if (box.Context is { Length: > 0 } ctx) inner.Children.Add(Text(ctx, 11.5, NativeLook.TextSecondary));
            var buttons = Row(8);
            if (!box.Answered)
            {
                var yes = Btn("Yes Enter", "Allow this tool call", () => VoiceAction?.Invoke("approve"), "island-voice-approve");
                var no = Btn("No Esc", "Deny this tool call", () => VoiceAction?.Invoke("deny"), "island-voice-deny");
                foreach (var b in new[] { yes, no }) { b.Padding = new Thickness(8, 1, 8, 2); b.MinHeight = 22; }
                buttons.Children.Add(yes);
                buttons.Children.Add(no);
                buttons.Children.Add(Text("or say “yes” / “no”", 11, NativeLook.TextTertiary));
            }
            else buttons.Children.Add(Text("answered — waiting for the desk…", 11, NativeLook.TextTertiary));
            inner.Children.Add(buttons);
            if (box.Line is { Length: > 0 } line) inner.Children.Add(Text(line, 11.5, NativeLook.AccentText));
            var border = new Border
            {
                Child = inner,
                Padding = new Thickness(8, 5, 8, 6),
                CornerRadius = new CornerRadius(8),
                BorderThickness = new Thickness(1),
                BorderBrush = Theme.Brush(NativeLook.Caution, root.ActualTheme),
                Background = Theme.Brush(NativeLook.ControlFill, root.ActualTheme),
            };
            AutomationProperties.SetAutomationId(border, "island-voice-approval");
            pane.Children.Add(border);
        }
        if (v.Answer is { } ans)
        {
            var card = new Border
            {
                Background = Theme.Brush(NativeLook.ControlFill, root.ActualTheme),
                CornerRadius = new CornerRadius(8),
                Padding = new Thickness(10, 6, 10, 6),
            };
            var inner = new StackPanel { Spacing = 2 };
            var a = Text(ans.Length == 0 ? "…" : ans, 13, v.Failed ? NativeLook.Critical : NativeLook.TextPrimary);
            a.TextWrapping = TextWrapping.Wrap;
            a.TextTrimming = TextTrimming.None;
            a.MaxLines = v.Steps is { Count: > 0 } ? 3 : 6;
            AutomationProperties.SetAutomationId(a, "island-voice-answer");
            inner.Children.Add(a);
            var foot = Row(6);
            foot.Children.Add(Text(v.Working ? "working…" : v.Failed ? "failed" : "done", 11, NativeLook.TextTertiary));
            if (v.OpenUrl is { } url)
            {
                var open = Btn("Open Ctrl+O", "Open the desk's session in theuth", () => _ = Links.Open(new Uri(url)), "island-voice-open");
                open.Padding = new Thickness(6, 0, 6, 0);
                open.MinHeight = 20;
                foot.Children.Add(open);
            }
            foot.Children.Add(Text(v.Footer ?? "Esc hides", 11, NativeLook.TextTertiary));
            inner.Children.Add(foot);
            card.Child = inner;
            pane.Children.Add(card);
        }
        if (v.Hint is { } h) pane.Children.Add(Text(h, 12, NativeLook.AccentText));
        body.Children.Add(pane);
    }

    /// <summary>153: a click or a scroll on the voice view (the collapsed card) — the controller expands the card.</summary>
    public event Action? VoiceTapped;

    /// <summary>153 §10.6: the approval box's Yes / No buttons in the voice view (<c>approve</c> · <c>deny</c>).</summary>
    public event Action<string>? VoiceAction;

    /// <summary>
    /// 153 probes: the island's XAML rendered to a PNG (RenderTargetBitmap — the tree itself, so WDA_EXCLUDEFROMCAPTURE does not blank
    /// it); false when the island is hidden or the render failed.
    /// </summary>
    public async Task<bool> CapturePng(string path)
    {
        if (!IsShown) return false;
        try
        {
            await Theuth.Companion.Ui.XamlPng.Write(root, path);
            return true;
        }
        catch (Exception e)
        {
            Log.Write($"island: capture {path} failed: {e.GetType().Name}: {e.Message}");
            return false;
        }
    }

    /// <summary>158 W1W probe: every TextBlock in the rendered tree, in order, joined by " · " — the copy the island actually shows.</summary>
    public string RenderedText()
    {
        var parts = new List<string>();
        void Walk(DependencyObject d)
        {
            if (d is TextBlock t && t.Text.Length > 0) parts.Add(t.Text);
            int n = VisualTreeHelper.GetChildrenCount(d);
            for (int i = 0; i < n; i++) Walk(VisualTreeHelper.GetChild(d, i));
        }
        Walk(body);
        return string.Join(" · ", parts);
    }

    /// <summary>One brief bullet (the pane and 158 (c)): a drawn 4 epx dot before wrapped text — 149 NATIVE, not a "•" character.</summary>
    private Grid Bullet(string text)
    {
        var t = Text(text, 12.5, NativeLook.TextSecondary);
        t.TextWrapping = TextWrapping.Wrap;
        t.TextTrimming = TextTrimming.None;
        var li = new Grid { ColumnSpacing = 6 };
        li.ColumnDefinitions.Add(new ColumnDefinition { Width = GridLength.Auto });
        li.ColumnDefinitions.Add(new ColumnDefinition { Width = new GridLength(1, GridUnitType.Star) });
        var dotEl = Dot(NativeLook.TextSecondary, 4);
        dotEl.VerticalAlignment = VerticalAlignment.Top;
        dotEl.Margin = new Thickness(0, 7, 0, 0);
        li.Children.Add(dotEl);
        Grid.SetColumn(t, 1);
        li.Children.Add(t);
        return li;
    }

    private Border Badge(string text, string key)
    {
        // R1: accent TEXT resource inside an accent-outlined badge. Island fix R1: the whole label always shows (badges have their
        // own rows); a label longer than the cap ends in an ellipsis with the full text as the tooltip, never a hard cut.
        var label = Text(text, 11, key == NativeLook.Accent ? NativeLook.AccentText : key);
        label.MaxWidth = 196;
        var b = new Border
        {
            Background = Theme.Brush(NativeLook.ControlFill, root.ActualTheme),
            BorderBrush = Theme.Brush(key, root.ActualTheme), BorderThickness = new Thickness(1),
            CornerRadius = new CornerRadius(4),
            Padding = new Thickness(6, 0, 6, 1),
            VerticalAlignment = VerticalAlignment.Center,
            Child = label,
        };
        ToolTipService.SetToolTip(b, text);
        AutomationProperties.SetName(b, text);
        return b;
    }

    /// <summary>
    /// Text on an accent FILL: <c>TextOnAccentFillColorPrimary</c> (the native-look table), unless the system accent is so dark (or
    /// light) that Windows' derived on-accent colour is unreadable on it — then the other theme's on-accent colour (R1).
    /// </summary>
    private string? onAccentLogged;

    private TextBlock OnAccentText(string s, double size, FontWeight? weight = null)
    {
        var t = Text(s, size, NativeLook.OnAccent, weight);
        var theme = root.ActualTheme == ElementTheme.Dark ? ElementTheme.Dark : ElementTheme.Light;
        var fill = Theme.ColorOf(NativeLook.Accent, theme);
        var on = Theme.ColorOf(NativeLook.OnAccent, theme);
        bool readable = NativeLook.OnAccentReadable((fill.R, fill.G, fill.B), (on.R, on.G, on.B));
        if (!readable)
        {
            // White or black by contrast. (Asking the other theme's dictionary for TextOnAccent returns the CURRENT theme's colour
            // in a running app — measured: #000000 in dark — so the fallback is computed, not looked up.)
            var best = NativeLook.BestTextOn((fill.R, fill.G, fill.B));
            t.Foreground = new SolidColorBrush(global::Windows.UI.Color.FromArgb(255, best.R, best.G, best.B));
        }
        var fg = t.Foreground is SolidColorBrush sb ? sb.Color : default;
        string probe = $"{theme} fg=#{fg.A:X2}{fg.R:X2}{fg.G:X2}{fg.B:X2} fill=#{fill.R:X2}{fill.G:X2}{fill.B:X2} on=#{on.R:X2}{on.G:X2}{on.B:X2} readable={readable}";
        if (probe != onAccentLogged) { onAccentLogged = probe; Log.Write($"island: text on accent fill — {probe}{(readable ? "" : " → the better of white/black")}"); }
        return t;
    }

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