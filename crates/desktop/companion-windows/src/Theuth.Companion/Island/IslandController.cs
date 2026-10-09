using System.ComponentModel;
using Microsoft.UI.Dispatching;
using Theuth.Companion.Core;
using Theuth.Companion.PointAndSay;
using Theuth.Companion.Rpc;
using Theuth.Companion.Ui;

namespace Theuth.Companion.Island;

/// <summary>
/// The island's controller (149 §3.6, §5.4 C): owns <see cref="IslandModel"/> (placement memory in <c>settings.json</c>,
/// "Always show"), the <see cref="MeetingSnapshot"/> reduced from WW1's raw stream hook through <see cref="MeetingWire"/> and
/// hydrated from the Store's <c>meeting_status</c> + briefs + live on every <c>MeetingRefetched</c>, the D10 visibility timer,
/// the island-scope actions (<c>meeting_tap / dismiss / deeper / request / cursor / pause / resume / stop</c>), the meeting
/// global keys (PTT Alt+` held → <c>meeting_ptt(true/false)</c>, eye Alt+Shift+E; WW1's <c>Hotkeys.Gate</c> registers those two
/// rows only while <c>Store.MeetingLive</c>). Point-and-say borrows the window for its voice mode.
/// </summary>
internal sealed class IslandController : IDisposable
{
    private readonly Store store;
    private readonly DispatcherQueue dispatcher;
    private readonly SettingsStore settings;
    private readonly IslandModel model = new();
    private readonly DispatcherQueueTimer hideTimer;
    private PointSettings point;
    private MeetingSnapshot snapshot = MeetingSnapshot.Empty;
    private DateTimeOffset? lastTopicOrBrief;
    private string? cursorSent;
    private bool disposed;

    public IslandController(Store store, DispatcherQueue dispatcher, SettingsStore settings)
    {
        this.store = store;
        this.dispatcher = dispatcher;
        this.settings = settings;
        model.Load(settings);
        model.FrameRejected += (device, off) => Log.Write($"island: saved frame for {device} ({off.Dx},{off.Dy}) is off its work area — rejected and forgotten");
        point = PointSettings.Load(settings);
        Window = new IslandWindow(dispatcher, model, Act, SaveModel);
        Window.DisplayChanged += () => dispatcher.TryEnqueue(() => { if (Window.IsShown) Window.Relayout(); });
        hideTimer = dispatcher.CreateTimer();
        hideTimer.IsRepeating = false;
        hideTimer.Tick += (_, _) => Evaluate();
        store.EventReceived += OnEvent;
        store.MeetingRefetched += Hydrate;
        store.GlobalKeyPressed += OnGlobalDown;
        store.GlobalKeyReleased += OnGlobalUp;
        store.PropertyChanged += OnStoreChanged;
        if (Environment.GetCommandLineArgs().Contains("--island-always")) model.AlwaysShow = true;
        Hydrate();
        if (Environment.GetCommandLineArgs().Contains("--island-fixture")) Fixture();
        Log.Write($"island: installed (frames={model.Frames.Count} always={(model.AlwaysShow ? "on" : "off")} window={Window.Hwnd})");
    }

    public IslandWindow Window { get; }

    public MeetingSnapshot Snapshot => snapshot;

    public IslandModel Model => model;

    public PointSettings PointSettings => point;

    public void SavePointSettings(PointSettings s)
    {
        point = s;
        try { s.Save(settings); } catch (Exception e) { Log.Write($"island: point settings save failed: {e.Message}"); }
    }

    private void SaveModel()
    {
        try { model.Save(settings); } catch (Exception e) { Log.Write($"island: settings save failed: {e.Message}"); }
    }

    // MARK: data

    private void OnEvent(string type, byte[] json)
    {
        var change = MeetingWire.Decode(type, json);
        if (change is null) return;
        Apply(MeetingReducer.Reduce(snapshot, change, DateTimeOffset.Now), change);
    }

    private void Hydrate()
    {
        if (fixture) return; // the probe's meeting is not the engine's
        var m = store.Meeting;
        if (m is null)
        {
            Apply(MeetingSnapshot.Empty, null);
            return;
        }
        var topics = m.Topics.Select(t => new MeetingTopic(t.Id, t.Label, (long)t.AtMs, t.Brief, t.Cached, t.Source)).ToArray();
        var briefs = store.Briefs.Values.OrderBy(b => b.AtMs).ThenBy(b => b.Id, StringComparer.Ordinal)
            .Select(b => new MeetingBrief(b.Id, b.Topic, b.Title, b.Bullets.ToArray(), b.Source, (long)b.AtMs, b.Parent, b.Cached)).ToArray();
        var lines = store.Live.Select(u => new MeetingLine(u.Track, (long)u.T0Ms, u.Text, u.Request, Open: false, u.Speaker)).ToArray();
        var degraded = m.Degraded.Select(d => d.Detail is null ? d.Kind : $"{d.Kind}:{d.Detail}").ToArray();
        var s = MeetingReducer.Hydrate(m.Meeting, m.Session, m.State.Word, degraded, (long)m.ElapsedMs, m.Cursor, topics, m.ApprovalWaiting, briefs, lines, DateTimeOffset.Now);
        Apply(s, null);
        Log.Write($"island: hydrated — state={m.State} topics={topics.Length} briefs={briefs.Length} lines={lines.Length} cursor={s.Cursor}");
    }

    private void Apply(MeetingSnapshot next, MeetingChange? change)
    {
        var prev = snapshot;
        snapshot = next;
        if (change is MeetingChange.TopicAdded or MeetingChange.Brief) lastTopicOrBrief = DateTimeOffset.Now;
        else if (change is null && next.LastTopicOrBriefMs is { } ms && next.Meeting is not null && lastTopicOrBrief is null)
            lastTopicOrBrief = DateTimeOffset.Now; // a hydrate with pending topics counts as "just arrived" at most once
        if (change is MeetingChange.Deleted || next.Meeting is null) { lastTopicOrBrief = next.Meeting is null && !model.AlwaysShow ? null : lastTopicOrBrief; }
        Window.Update(next);
        var cursor = next.Current?.Id;
        if (!fixture && cursor != cursorSent && (change is MeetingChange.Brief || prev.Cursor != next.Cursor))
        {
            cursorSent = cursor;
            _ = store.MeetingCursorAsync(cursor).ContinueWith(t => { if (t.IsFaulted) Log.Write($"island: meeting_cursor failed: {t.Exception?.GetBaseException().Message}"); }, TaskScheduler.Default);
        }
        Evaluate();
    }

    private void OnStoreChanged(object? sender, PropertyChangedEventArgs e)
    {
        // The meeting state itself arrives through MeetingRefetched (WW1 refetches on connect and on meeting events).
        if (e.PropertyName == nameof(StoreModel.Connection) && store.Connection == RpcClient.State.Open) Log.Write("island: engine connected");
    }

    // MARK: visibility (D10)

    private void Evaluate()
    {
        if (disposed) return;
        var now = DateTimeOffset.Now;
        bool show = Window.InVoiceMode || model.ShouldShow(snapshot.State, lastTopicOrBrief, now);
        if (show)
        {
            if (!Window.IsShown) Window.ShowOn(ChosenMonitor());
        }
        else if (Window.IsShown)
        {
            Window.HideIsland();
        }
        hideTimer.Stop();
        if (IslandModel.HideDue(snapshot.State, lastTopicOrBrief) is { } due && due > now && !model.AlwaysShow)
        {
            hideTimer.Interval = due - now + TimeSpan.FromMilliseconds(50);
            hideTimer.Start();
        }
    }

    /// <summary>The chosen monitor: the one the island was on, else the one with the mouse, else the primary.</summary>
    private ScreenMonitor ChosenMonitor()
    {
        var all = ScreenMonitor.All();
        if (Window.Monitor is { } m && all.FirstOrDefault(x => x.Device == m.Device) is { } same) return same;
        if (Windows.Win32.PInvoke.GetCursorPos(out var p)) return ScreenMonitor.At(all, p.X, p.Y);
        return ScreenMonitor.At(all, 0, 0);
    }

    /// <summary>Point-and-say: show the island (for its voice mode) even without a meeting; hide again when the mode ends.</summary>
    public void SetVoice(VoiceView? v)
    {
        Window.SetVoice(v);
        Evaluate();
    }

    // MARK: actions

    private void Act(IslandAction a)
    {
        if (fixture && a is not (IslandAction.Move or IslandAction.Eye))
        {
            Log.Write($"island: fixture — {a.GetType().Name} not sent");
            if (a is IslandAction.Tap t0) { snapshot = MeetingReducer.MarkTapped(snapshot, t0.Topic); Window.Update(snapshot); }
            if (a is IslandAction.Dismiss d0) Apply(MeetingReducer.Reduce(snapshot, new MeetingChange.TopicRemoved("fixture-meeting", d0.Topic, "Dismissed"), DateTimeOffset.Now), null);
            if (a is IslandAction.Mute) Apply(MeetingReducer.Reduce(snapshot, new MeetingChange.State("fixture-meeting", snapshot.State == "Paused" ? "Recording" : "Paused", Array.Empty<string>()), DateTimeOffset.Now), null);
            return;
        }
        switch (a)
        {
            case IslandAction.Tap t:
                snapshot = MeetingReducer.MarkTapped(snapshot, t.Topic);
                Window.Update(snapshot);
                Fire(store.MeetingTapAsync(t.Topic), "meeting_tap");
                break;
            case IslandAction.Dismiss d: Fire(store.MeetingDismissAsync(d.Topic), "meeting_dismiss"); break;
            case IslandAction.Deeper dp: Fire(store.MeetingDeeperAsync(dp.Brief), "meeting_deeper"); break;
            case IslandAction.Request r: Fire(store.MeetingRequestAsync(r.Text), "meeting_request"); break;
            case IslandAction.Move mv: Apply(MeetingReducer.Move(snapshot, mv.Delta), null); break;
            case IslandAction.Mute:
                if (snapshot.State == "Paused") Fire(store.MeetingResumeAsync(), "meeting_resume");
                else if (snapshot.State == "Recording") Fire(store.MeetingPauseAsync(), "meeting_pause");
                break;
            case IslandAction.Stop: Fire(store.MeetingStopAsync(), "meeting_stop"); break;
            case IslandAction.Eye: ToggleEye(); break;
            case IslandAction.OpenMeetings: _ = Links.Open(new Uri("theuth://meetings")); break;
        }
    }

    private static void Fire(Task t, string what) =>
        t.ContinueWith(x => { if (x.IsFaulted) Log.Write($"island: {what} failed: {x.Exception?.GetBaseException().Message}"); else Log.Write($"island: {what} ok"); }, TaskScheduler.Default);

    /// <summary>The eye (Revision 1 #9): authoritative for the companion's own windows while it is alive; best effort.</summary>
    public bool HiddenFromCapture => Window.HiddenFromCapture;

    public event Action<bool>? EyeChanged;

    private void ToggleEye()
    {
        bool hidden = !Window.HiddenFromCapture;
        Window.SetHiddenFromCapture(hidden);
        EyeChanged?.Invoke(hidden);
        Log.Write($"island: eye → {(hidden ? "hidden from capture" : "visible in capture")}");
    }

    // MARK: the meeting global keys (PTT Alt+` held, eye Alt+Shift+E), registered only while live

    private void OnGlobalDown(KeyAction a)
    {
        if (a == KeyAction.MeetingPtt && snapshot.Meeting is not null) Fire(store.MeetingPttAsync(true), "meeting_ptt(true)");
        else if (a == KeyAction.MeetingEye) ToggleEye();
    }

    private void OnGlobalUp(KeyAction a, TimeSpan held)
    {
        if (a == KeyAction.MeetingPtt && snapshot.Meeting is not null) Fire(store.MeetingPttAsync(false), "meeting_ptt(false)");
    }

    public void Dispose()
    {
        if (disposed) return;
        disposed = true;
        hideTimer.Stop();
        store.EventReceived -= OnEvent;
        store.MeetingRefetched -= Hydrate;
        store.GlobalKeyPressed -= OnGlobalDown;
        store.GlobalKeyReleased -= OnGlobalUp;
        store.PropertyChanged -= OnStoreChanged;
        Window.AllowClose = true;
        try { Window.Close(); } catch (Exception) { }
    }
}
