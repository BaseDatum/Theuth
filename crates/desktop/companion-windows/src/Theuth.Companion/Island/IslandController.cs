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
    private readonly DispatcherQueueTimer oracleTimer;
    private readonly DispatcherQueueTimer engineLost;
    private DateTimeOffset? lostSince;
    private DispatcherQueueTimer? fixtureTimer;
    private PointSettings point;
    private ModeKeySettings modeKeys;
    private MeetingSnapshot snapshot = MeetingSnapshot.Empty;
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
        modeKeys = ModeKeySettings.Load(settings); // 153 §14.3: gesture per mode, the two new rows' switches, modifier-only keys
        Window = new IslandWindow(dispatcher, model, Act, SaveModel);
        Window.DisplayChanged += () => dispatcher.TryEnqueue(() => { if (Window.IsShown) Window.Relayout(); });
        if (!Flags.Shareable && Window.HiddenFromCapture != model.HiddenFromCapture) Window.SetHiddenFromCapture(model.HiddenFromCapture);
        // D11: the oracle chip queries decision_mode('meeting') every 30 s while a meeting is on the island.
        oracleTimer = dispatcher.CreateTimer();
        oracleTimer.Interval = TimeSpan.FromSeconds(30);
        oracleTimer.IsRepeating = true;
        oracleTimer.Tick += (_, _) => PollOracle();
        engineLost = dispatcher.CreateTimer();
        engineLost.IsRepeating = false;
        engineLost.Tick += (_, _) => EngineLost();
        store.EventReceived += OnEvent;
        store.MeetingRefetched += Hydrate;
        store.GlobalKeyPressed += OnGlobalDown;
        store.GlobalKeyReleased += OnGlobalUp;
        store.PropertyChanged += OnStoreChanged;
        if (Environment.GetCommandLineArgs().Contains("--island-always")) model.AlwaysShow = true;
        Hydrate();
        if (Environment.GetCommandLineArgs().Contains("--island-fixture")) Fixture();
        else if (Environment.GetCommandLineArgs().Contains("--island-prompt")) FixturePrompt();
        Log.Write($"island: installed (frames={model.Frames.Count} always={(model.AlwaysShow ? "on" : "off")} window={Window.Hwnd})");
    }

    public IslandWindow Window { get; }

    public MeetingSnapshot Snapshot => snapshot;

    public IslandModel Model => model;

    public PointSettings PointSettings => point;

    /// <summary>Settings › Island "Always show".</summary>
    public void SetAlwaysShow(bool on)
    {
        model.AlwaysShow = on;
        SaveModel();
        Evaluate();
        Log.Write($"island: always show = {(on ? "on" : "off")}");
    }

    /// <summary>Settings › Island eye default (also the Alt+Shift+E toggle's persisted value).</summary>
    public void SetHiddenFromCapture(bool hidden)
    {
        model.HiddenFromCapture = hidden;
        SaveModel();
        if (Window.HiddenFromCapture != hidden) Window.SetHiddenFromCapture(hidden);
        EyeChanged?.Invoke(hidden);
    }

    /// <summary>Settings › Island "Reset position" for one monitor.</summary>
    public void ResetPosition(ScreenMonitor m)
    {
        var f = model.SnapBack(m.AsIslandDisplay(), (Window.ScreenRect.right - Window.ScreenRect.left, Window.ScreenRect.bottom - Window.ScreenRect.top));
        if (Window.IsShown && Window.Monitor?.Device == m.Device) Window.Place(new Windows.Graphics.RectInt32(f.X, f.Y, f.Width, f.Height));
        SaveModel();
        Log.Write($"island: position reset for {m.Device}");
    }

    public void ResetAllPositions()
    {
        model.ResetAll();
        SaveModel();
        if (Window.IsShown) Window.Relayout();
        Log.Write("island: all positions reset");
    }

    public void SavePointSettings(PointSettings s)
    {
        point = s;
        try { s.Save(settings); } catch (Exception e) { Log.Write($"island: point settings save failed: {e.Message}"); }
    }

    /// <summary>153 §12.6 / §14.3: the mode keys' companion-local settings (<c>gesture.&lt;mode&gt;</c>, <c>dictateAnywhereEnabled</c>, <c>commandKeyEnabled</c>, <c>modifierOnlyKeys</c>).</summary>
    public ModeKeySettings ModeKeys => modeKeys;

    /// <summary>Settings › Keys changed a gesture, a switch or the modifier-only option (the hotkeys re-apply).</summary>
    public event Action? ModeKeysChanged;

    public void SaveModeKeys(ModeKeySettings s)
    {
        modeKeys = s.Clone();
        try { modeKeys.Save(settings); } catch (Exception e) { Log.Write($"island: mode key settings save failed: {e.Message}"); }
        Log.Write($"island: mode keys — gesture ask={modeKeys.GestureFor(PointMode.Ask).RawValue()} dictate={modeKeys.GestureFor(PointMode.Dictate).RawValue()} command={modeKeys.GestureFor(PointMode.Command).RawValue()} dictateAnywhere={(modeKeys.DictateAnywhereEnabled ? "on" : "off")} command={(modeKeys.CommandKeyEnabled ? "on" : "off")} modifierOnly={(modeKeys.ModifierOnlyKeys ? "on" : "off")}");
        ModeKeysChanged?.Invoke();
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
        // 158 §4.5: a companion connecting mid-prompt renders it from `meeting_status.detection`.
        var detection = m.Detection is { } det ? new MeetingDetection(Process(det.App), det.AppId, det.Since, det.Also.Select(Process).ToArray()) : null;
        var s = MeetingReducer.Hydrate(m.Meeting, m.Session, m.State.Word, degraded, (long)m.ElapsedMs, m.Cursor, topics, m.ApprovalWaiting, briefs, lines, DateTimeOffset.Now, detection);
        Apply(s, null);
        Log.Write($"island: hydrated — state={m.State} topics={topics.Length} briefs={briefs.Length} lines={lines.Length} cursor={s.Cursor}{(detection is null ? "" : $" prompt={detection.AppId}")}");
    }

    private static MeetingProcess Process(ProcessInfo p) => new(p.Pid, p.Name, p.BundleId, p.Active);

    private void Apply(MeetingSnapshot next, MeetingChange? change)
    {
        var prev = snapshot;
        snapshot = next;
        if (change is MeetingChange.Level lv)
        {
            Window.SetLevels(lv.MicRms, lv.SystemRms); // ~6/s: the bars resize in place, no re-render
            return;
        }
        // Island fix D1: present the state first (open while live unless collapsed for this meeting), then render at that shape.
        var mode = model.Mode;
        model.Present(next.Meeting, next.State, next.Prompt is not null);
        if (prev.State != next.State || prev.Meeting != next.Meeting || mode != model.Mode || !Equals(prev.Prompt, next.Prompt))
            Log.Write($"island: presented state={next.State} meeting={next.Meeting ?? "-"} mode={model.Mode}{(next.Meeting is not null && model.IsCollapsed(next.Meeting) ? " (collapsed for this meeting)" : "")}{(next.Prompt is null ? "" : $" prompt={next.Prompt.GetType().Name}")}");
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
        if (e.PropertyName != nameof(StoreModel.Connection)) return;
        if (store.Connection == RpcClient.State.Open)
        {
            lostSince = null;
            engineLost.Stop();
            Log.Write("island: engine connected");
        }
        else if (lostSince is null)
        {
            lostSince = DateTimeOffset.Now;
            engineLost.Interval = IslandModel.EngineLostGrace + TimeSpan.FromMilliseconds(50);
            engineLost.Start();
        }
    }

    // MARK: visibility (D10)

    /// <summary>
    /// Island fix D2 — the original's rule: shown iff the meeting is live (Starting · Recording · Paused · Stopping) or "Always
    /// show" (or point-and-say borrows the window). Done · Failed · anything else: the close animation, then gone. No linger timer.
    /// </summary>
    private void Evaluate()
    {
        if (disposed) return;
        bool show = Window.InVoiceMode || model.ShouldShow(snapshot.State);
        if (show)
        {
            if (!Window.IsShown || Window.IsHiding) Window.ShowOn(ChosenMonitor());
        }
        else if (Window.IsShown)
        {
            Window.HideIsland();
        }
        if (snapshot.Meeting is not null && MeetingReducer.Live(snapshot.State) && !fixture) { if (!oracleTimer.IsRunning) { oracleTimer.Start(); PollOracle(); } }
        else oracleTimer.Stop();
    }

    /// <summary>The engine connection stayed closed for <see cref="IslandModel.EngineLostGrace"/>: a live meeting ends on the island (a reconnect hydrates it back).</summary>
    private void EngineLost()
    {
        if (disposed || fixture || lostSince is not { } since) return;
        var state = IslandModel.EffectiveState(snapshot.State, DateTimeOffset.Now - since);
        if (state == snapshot.State) return;
        Log.Write($"island: engine connection closed for {(DateTimeOffset.Now - since).TotalSeconds:0.0} s while {snapshot.State} — the meeting is presented as ended");
        Apply(snapshot with { State = state }, null);
    }

    /// <summary><c>decision_mode('meeting')</c> → the chip's effective mode word (<c>ModeInfo.effective</c>: a word, or an object whose first key is the word).</summary>
    private void PollOracle()
    {
        _ = store.Client.CallAsync<System.Text.Json.JsonElement>("decision_mode", "meeting").ContinueWith(t =>
        {
            if (t.IsFaulted)
            {
                Log.Write($"island: decision_mode failed: {t.Exception?.GetBaseException().Message}");
                return;
            }
            string? word = null;
            var e = t.Result;
            if (e.ValueKind == System.Text.Json.JsonValueKind.Object && e.TryGetProperty("effective", out var eff))
            {
                if (eff.ValueKind == System.Text.Json.JsonValueKind.String) word = eff.GetString();
                else if (eff.ValueKind == System.Text.Json.JsonValueKind.Object) foreach (var p in eff.EnumerateObject()) { word = p.Name; break; }
            }
            dispatcher.TryEnqueue(() => Window.SetOracleMode(word?.ToLowerInvariant()));
        }, TaskScheduler.Default);
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
        Window.LogVoicePill();
    }

    // MARK: actions

    private void Act(IslandAction a)
    {
        // D3 / §N: one brief-walk path for a topic pill with a brief, the older/newer buttons and J/K/↑/↓.
        if (a is IslandAction.Cycle cy)
        {
            ShowBrief(MeetingReducer.CycleIndex(snapshot, cy.Delta), cy.Via);
            return;
        }
        if (a is IslandAction.Move mv0)
        {
            ShowBrief(snapshot.Cursor + mv0.Delta, mv0.Delta < 0 ? "older" : "newer");
            return;
        }
        if (a is IslandAction.Tap tb && MeetingReducer.TopicBriefIndex(snapshot, tb.Topic) is var at && at >= 0)
        {
            ShowBrief(at, $"pill {tb.Topic}");
            return;
        }
        if (fixture && a is not IslandAction.Eye)
        {
            Log.Write($"island: fixture — {a.GetType().Name} not sent");
            if (a is IslandAction.Tap t0) { snapshot = MeetingReducer.MarkTapped(snapshot, t0.Topic); Window.Update(snapshot); }
            if (a is IslandAction.Dismiss d0) Apply(MeetingReducer.Reduce(snapshot, new MeetingChange.TopicRemoved("fixture-meeting", d0.Topic, "Dismissed"), DateTimeOffset.Now), null);
            if (a is IslandAction.Mute) Apply(MeetingReducer.Reduce(snapshot, new MeetingChange.State("fixture-meeting", snapshot.State == "Paused" ? "Recording" : "Paused", Array.Empty<string>()), DateTimeOffset.Now), null);
            if (a is IslandAction.Stop) FixtureStop();
            if (a is IslandAction.StartRecording) { FixtureStart(); return; }
            if (a is IslandAction.NotNow or IslandAction.NeverForApp or IslandAction.KeepRecording) Apply(snapshot with { Prompt = null }, null);
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
            case IslandAction.Mute:
                if (snapshot.State == "Paused") Fire(store.MeetingResumeAsync(), "meeting_resume");
                else if (snapshot.State == "Recording") Fire(store.MeetingPauseAsync(), "meeting_pause");
                break;
            case IslandAction.Stop: Fire(store.MeetingStopAsync(), "meeting_stop"); break;
            case IslandAction.Eye: ToggleEye(); break;
            case IslandAction.OpenMeetings: _ = Links.Open(new Uri("theuth://meetings")); break;
            // 158 §5.4: one frozen RPC; the engine fills the defaults (model, memory, template, project) server-side.
            case IslandAction.StartRecording sr:
                Log.Write($"island: start recording — {sr.Detection.App.Display} (pid {sr.Detection.App.Pid}, {sr.Detection.AppId})");
                Fire(store.MeetingStartAsync(new StartMeeting { Process = sr.Detection.App.Pid, Consent = true, Source = StartSource.Detected(sr.Detection.AppId) }), "meeting_start");
                break;
            // 158 §5.3: the prompt goes at once; the engine's `DetectionCleared` confirms (and a failed call is logged, not re-shown).
            case IslandAction.NotNow nn:
                Apply(snapshot with { Prompt = null }, null);
                Fire(store.MeetingDetectionDismissAsync(nn.AppId, never: false), "meeting_detection_dismiss(not now)");
                break;
            case IslandAction.NeverForApp nv:
                Apply(snapshot with { Prompt = null }, null);
                Fire(store.MeetingDetectionDismissAsync(nv.AppId, never: true), "meeting_detection_dismiss(never)");
                break;
            case IslandAction.KeepRecording:
                Apply(snapshot with { Prompt = null }, null); // the engine re-sends `AppReleased` every 30 s while the app stays off the mic
                break;
        }
    }

    /// <summary>Show brief <paramref name="index"/> (the pill, older/newer and J/K all land here); the cursor goes to the engine in <see cref="Apply"/>.</summary>
    private void ShowBrief(int index, string via)
    {
        if (index < 0 || snapshot.Briefs.Count == 0)
        {
            Log.Write($"island: brief walk via {via} — no briefs");
            return;
        }
        var next = MeetingReducer.ShowAt(snapshot, index);
        Log.Write($"island: brief {next.Cursor + 1}/{next.Briefs.Count} via {via} ({next.Current?.Title})");
        Apply(next, null);
    }

    private static void Fire(Task t, string what) =>
        t.ContinueWith(x => { if (x.IsFaulted) Log.Write($"island: {what} failed: {x.Exception?.GetBaseException().Message}"); else Log.Write($"island: {what} ok"); }, TaskScheduler.Default);

    /// <summary>The eye (Revision 1 #9): authoritative for the companion's own windows while it is alive; best effort.</summary>
    public bool HiddenFromCapture => Window.HiddenFromCapture;

    public event Action<bool>? EyeChanged;

    private void ToggleEye()
    {
        bool hidden = !Window.HiddenFromCapture;
        SetHiddenFromCapture(hidden);
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

    /// <summary>
    /// <c>--island-fixture</c> (149 §10 probes; build-B cannot run a meeting without audio): a recording meeting with two topics,
    /// an Auto brief and two transcript lines fed through the reducer exactly as the stream would, so the island shows (D10),
    /// opens, drags and snaps back for the gate. Nothing is sent to the engine; the actions log "fixture" and no-op.
    /// </summary>
    private void Fixture()
    {
        var now = DateTimeOffset.Now;
        fixture = true;
        var s = MeetingReducer.Reduce(MeetingSnapshot.Empty, new MeetingChange.Started("fixture-meeting", "fixture-session"), now - TimeSpan.FromSeconds(751));
        s = MeetingReducer.Reduce(s, new MeetingChange.TopicAdded("fixture-meeting", new MeetingTopic("t1", "PROJ-12", 600_000)), now);
        s = MeetingReducer.Reduce(s, new MeetingChange.TopicAdded("fixture-meeting", new MeetingTopic("t2", "Standup", 700_000)), now);
        s = MeetingReducer.Reduce(s, new MeetingChange.Brief("fixture-meeting", new MeetingBrief("b1", "t1", "PROJ-12 — the ingest retry",
            new[] { "Opened Tuesday by Sky; retries the ingest job three times before it files an incident", "Blocked on the schema change in PR #418", "Owner: ops desk" }, "Auto", 650_000)), now);
        // Island fix R1: the two degradations a model-less build-B meeting shows, so the badge row is proven on the fixture too.
        s = MeetingReducer.Reduce(s, new MeetingChange.State("fixture-meeting", "Recording", new[] { "MicNoAec", "ModelDown" }), now);
        // Island fix §N: three briefs so the walk (pills, older/newer, J/K/↑/↓) wraps visibly.
        s = MeetingReducer.Reduce(s, new MeetingChange.Brief("fixture-meeting", new MeetingBrief("b2", "t2", "Standup — what moved since Tuesday",
            new[] { "Ingest retry fix is in review", "Pricing page layout still open", "Demo moved to Friday" }, "Tap", 700_500)), now);
        s = MeetingReducer.Reduce(s, new MeetingChange.Brief("fixture-meeting", new MeetingBrief("b3", "t1", "PROJ-12 — deeper: the schema change",
            new[] { "PR #418 adds the nullable column the retry reads", "Lands tomorrow after review" }, "Deeper", 720_000, Parent: "b1")), now);
        s = MeetingReducer.Reduce(s, new MeetingChange.Utterance("fixture-meeting", "System", 740_000, "so the ingest retry is still failing on the schema", false, true, "s2"), now);
        s = MeetingReducer.Reduce(s, new MeetingChange.Utterance("fixture-meeting", "Mic", 745_000, "yes, PR 418 lands tomorrow", false, true, null), now);
        s = MeetingReducer.Reduce(s, new MeetingChange.OracleDecision("fixture-meeting", new MeetingOracleRow("batch-1", "brief", false, true)), now);
        Apply(s, new MeetingChange.TopicAdded("fixture-meeting", new MeetingTopic("t2", "Standup", 700_000)));
        Window.SetOracleMode("shadow");
        // Level pulses at 6/s, as the engine sends them.
        int tick = 0;
        fixtureTimer = dispatcher.CreateTimer();
        fixtureTimer.Interval = TimeSpan.FromMilliseconds(160);
        fixtureTimer.IsRepeating = true;
        fixtureTimer.Tick += (_, _) =>
        {
            tick++;
            float mic = (float)(0.05 + 0.35 * Math.Abs(Math.Sin(tick / 3.0)));
            float sys = (float)(0.03 + 0.25 * Math.Abs(Math.Sin(tick / 5.0 + 1)));
            Apply(MeetingReducer.Reduce(snapshot, new MeetingChange.Level("fixture-meeting", mic, sys), DateTimeOffset.Now), new MeetingChange.Level("fixture-meeting", mic, sys));
        };
        fixtureTimer.Start();
        // A live meeting re-renders the island on every partial utterance (~3/s while someone talks): the fixture talks too, so a
        // click or a drag is proven against the same churn a real meeting makes.
        int word = 0;
        string[] words = "so the ingest retry is still failing on the schema and PR 418 lands tomorrow after the review".Split(' ');
        fixtureTalk = dispatcher.CreateTimer();
        fixtureTalk.Interval = TimeSpan.FromMilliseconds(350);
        fixtureTalk.IsRepeating = true;
        fixtureTalk.Tick += (_, _) =>
        {
            if (!MeetingReducer.Live(snapshot.State)) return;
            word = word % words.Length + 1;
            var u = new MeetingChange.Utterance("fixture-meeting", "System", 760_000, string.Join(' ', words.Take(word)), false, word == words.Length, "s2");
            Apply(MeetingReducer.Reduce(snapshot, u, DateTimeOffset.Now), u);
        };
        fixtureTalk.Start();
        Log.Write($"island: fixture meeting applied (recording 12:31, 2 topics, {snapshot.Briefs.Count} briefs, 2 lines, oracle shadow·brief, level pulses, talking)");
    }

    private bool fixture;
    private DispatcherQueueTimer? fixtureTalk;

    /// <summary>The fixture's stand-in for the engine's <c>meeting_stop</c>: Stopping now, Done 500 ms later (the engine's typical drain).</summary>
    private void FixtureStop()
    {
        if (!MeetingReducer.Live(snapshot.State)) return;
        Apply(MeetingReducer.Reduce(snapshot, new MeetingChange.State("fixture-meeting", "Stopping", Array.Empty<string>()), DateTimeOffset.Now), null);
        Log.Write("island: fixture state → Stopping");
        var t = dispatcher.CreateTimer();
        t.Interval = TimeSpan.FromMilliseconds(500);
        t.IsRepeating = false;
        t.Tick += (_, _) =>
        {
            fixtureTalk?.Stop();
            Apply(MeetingReducer.Reduce(snapshot, new MeetingChange.State("fixture-meeting", "Done", Array.Empty<string>()), DateTimeOffset.Now), null);
            Log.Write("island: fixture state → Done");
        };
        t.Start();
    }

    /// <summary>The fixture's detection prompt (158 §5.2 a): <c>--island-prompt</c> shows "Zoom is using your microphone"; Start runs the fixture meeting.</summary>
    private static readonly MeetingProcess FixtureApp = new(4242, "Zoom.exe", null, true);

    private void FixturePrompt()
    {
        fixture = true;
        Apply(MeetingReducer.Reduce(MeetingSnapshot.Empty, new MeetingChange.Detected(new MeetingDetection(FixtureApp, "zoom.exe", DateTimeOffset.Now.ToString("O"), Array.Empty<MeetingProcess>())), DateTimeOffset.Now), null);
        Log.Write("island: fixture prompt applied (Zoom is using your microphone)");
    }

    /// <summary>The fixture's "Start recording": the fixture meeting, then <c>AppReleased</c> 8 s in so the stop prompt (158 §5.2 d) is proven too.</summary>
    private void FixtureStart()
    {
        Fixture();
        var t = dispatcher.CreateTimer();
        t.Interval = TimeSpan.FromSeconds(8);
        t.IsRepeating = false;
        t.Tick += (_, _) =>
        {
            if (!MeetingReducer.Live(snapshot.State)) return;
            Apply(MeetingReducer.Reduce(snapshot, new MeetingChange.AppReleased("fixture-meeting", FixtureApp, 30), DateTimeOffset.Now), null);
            Log.Write("island: fixture AppReleased (30 s) → stop prompt");
        };
        t.Start();
    }

    public void Dispose()
    {
        if (disposed) return;
        disposed = true;
        oracleTimer.Stop();
        engineLost.Stop();
        fixtureTimer?.Stop();
        fixtureTalk?.Stop();
        store.EventReceived -= OnEvent;
        store.MeetingRefetched -= Hydrate;
        store.GlobalKeyPressed -= OnGlobalDown;
        store.GlobalKeyReleased -= OnGlobalUp;
        store.PropertyChanged -= OnStoreChanged;
        Window.AllowClose = true;
        try { Window.Close(); } catch (Exception) { }
    }
}