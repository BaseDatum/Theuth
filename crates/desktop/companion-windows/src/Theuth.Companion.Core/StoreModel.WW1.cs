using Theuth.Companion.Rpc;

namespace Theuth.Companion.Core;

/// <summary>
/// Design 149 WW1 — the Store's new observable data and thin actions, Mac-testable (the app's <c>Store.WW1.cs</c> only adds
/// the hotkey plumbing). WW2 (pills, agents panel, card anatomy, Ask pill) and WW3 (island, point-and-say) read these
/// members and raise nothing themselves; every change is a <c>PropertyChanged</c> on the owner thread, every stream
/// item a typed event. Refetch: <see cref="Refetch149Async"/> runs after every <see cref="Refetch"/> (sessions, their
/// status, the mesh) and <see cref="RefetchMeetingAsync"/> on connect and on every meeting event.
/// </summary>
public partial class StoreModel
{
    private ValueList<SessionSummary> sessions = new();
    private SessionsStatus? sessionsStatus;
    private MeshStatus? mesh;
    private MeetingStatus? meeting;
    private ValueList<Topic> topics = new();
    private readonly Dictionary<string, Brief> briefs = new(StringComparer.Ordinal);
    private readonly List<Utterance> live = new();
    private float meetingMicLevel, meetingSystemLevel;
    private bool meetingListening;
    private bool fetching149, fetchAgain149;
    private bool fetchingMeeting, fetchMeetingAgain;

    /// <summary>The most recent transcript lines kept (<see cref="Live"/>); the island shows the tail.</summary>
    public const int LiveCap = 200;

    // MARK: agents and sessions (WW2 AgentsModel: list_activity ∪ agents_list ∪ list_sessions + mesh_status, 149 §5.1)

    /// <summary><c>list_sessions</c>: every session with <c>Running</c>, <c>Owner</c>, <c>Here</c>, <c>Agent</c> (harness kind), <c>Project</c>, <c>Cwd</c>.</summary>
    public IReadOnlyList<SessionSummary> Sessions => sessions;

    /// <summary><c>sessions_status</c>: this engine's id and the here / elsewhere counts (the host group labels).</summary>
    public SessionsStatus? SessionsStatus => sessionsStatus;

    /// <summary><c>mesh_status</c> as last fetched: <c>SelfId</c> / <c>SelfName</c> / <c>Peers</c> name the hosts ("on ‹node›", 149 E7 fallback).</summary>
    public MeshStatus? Mesh => mesh;

    /// <summary>This engine's id (<c>sessions_status.engine</c>, else <c>mesh_status.self_id</c>, else ""): a session whose <c>Owner</c> differs is remote.</summary>
    public string SelfEngineId => sessionsStatus?.Engine ?? mesh?.SelfId ?? "";

    /// <summary>The display name of the engine that owns <paramref name="session"/>: "" for this one, the peer's name, else the raw id.</summary>
    public string HostNameOf(SessionSummary session)
    {
        if (session.Here || session.Owner.Length == 0 || session.Owner == SelfEngineId) return "";
        if (mesh is { } m)
        {
            foreach (var p in m.Peers)
            {
                if (p.Id == session.Owner) return p.Name;
            }
        }
        return session.Owner;
    }

    /// <summary>The session row for an <c>AgentInfo</c> (by id); null when <c>list_sessions</c> has no row for it.</summary>
    public SessionSummary? SessionOf(AgentInfo info)
    {
        foreach (var s in sessions)
        {
            if (s.Id == info.Id) return s;
        }
        return null;
    }

    /// <summary>Running children of <paramref name="info"/>: the engine's <c>activity.children</c> when it reports it, else the rows whose <c>Parent</c> is it.</summary>
    public int ChildrenOf(AgentInfo info)
    {
        if (info.WireActivity is { } w) return w.Children;
        int n = 0;
        foreach (var a in Activity)
        {
            if (a.Parent == info.Id) n++;
        }
        return n;
    }

    /// <summary>The two card bubbles (E6 or the interim join) for an item.</summary>
    public AttentionContext.Resolved ContextFor(AttentionItem item) => AttentionContext.Resolve(item, sessions, Activity);

    /// <summary>A <c>SubagentSpawned</c> item arrived (the row appears on the next refetch; WW2 may animate).</summary>
    public event Action<SubagentSpawned>? SubagentSpawnedReceived;

    /// <summary>An <c>ActivityAppended</c> item arrived (its row raw, Core's <c>ActivityEntry</c> shape).</summary>
    public event Action<ActivityAppended>? ActivityAppendedReceived;

    /// <summary><see cref="Refetch149Async"/> landed: <see cref="Sessions"/>, <see cref="SessionsStatus"/>, <see cref="Mesh"/> are fresh.</summary>
    public event Action? SessionsRefetched;

    // MARK: meeting (WW3 MeetingReducer / IslandModel, 149 §3.6)

    /// <summary><c>meeting_status</c> as last fetched; null before the first fetch or when the engine has no meeting plugin.</summary>
    public MeetingStatus? Meeting => meeting;

    /// <summary>The topic strip: <c>meeting_status.topics</c> on fetch, then <c>TopicAdded</c> / <c>TopicRemoved</c> keep it current.</summary>
    public IReadOnlyList<Topic> Topics => topics;

    /// <summary>Briefs by id: <c>meeting_briefs</c> on fetch, then every <c>Brief</c> event.</summary>
    public IReadOnlyDictionary<string, Brief> Briefs => briefs;

    /// <summary>The engine's cursor (<c>meeting_status.cursor</c>, raw): the brief it believes is on screen.</summary>
    public string? MeetingCursor => meeting?.Cursor;

    /// <summary>The final transcript lines, oldest first, at most <see cref="LiveCap"/> (<c>meeting_live</c> on fetch, then <c>Utterance</c> events with <c>final</c>).</summary>
    public IReadOnlyList<Utterance> Live => live;

    /// <summary>A meeting is recording or paused (the island's "live" rule, 149 Revision 1 #13 — the 10-minute tail is the island's).</summary>
    public bool MeetingLive => meeting?.IsLive ?? false;

    /// <summary>The last <c>Level</c> event (0…1 RMS per track).</summary>
    public float MeetingMicLevel => meetingMicLevel;

    public float MeetingSystemLevel => meetingSystemLevel;

    /// <summary>The push-to-talk key is held (<c>Listening.held</c>).</summary>
    public bool MeetingListening => meetingListening;

    /// <summary>Every <c>meeting::MeetingEvent</c> item, typed and raw, after the Store applied it (the reducer's input).</summary>
    public event Action<MeetingEvent>? MeetingEventReceived;

    /// <summary><see cref="RefetchMeetingAsync"/> landed.</summary>
    public event Action? MeetingRefetched;

    // MARK: dictation (WW2 Ask pill level bars + live partial; WW3 voice key)

    /// <summary>Every <c>Dictation*</c> item typed (<c>Levels</c> on <c>DictationLevel</c>, <c>Text</c> / <c>IsFinal</c> on <c>DictationText</c>), before the Ask bar's reducer sees it.</summary>
    public event Action<DictationEvent>? DictationEventReceived;

    // MARK: global keys (the app forwards Hotkeys.Pressed / Released; WW3's PointMachine does hold / double-tap on these)

    /// <summary>A global row's chord went down (once per press): <c>VoiceKey</c>, <c>OpenAgents</c>, <c>MeetingPtt</c>, <c>MeetingEye</c>, and the W1–W6 rows too.</summary>
    public event Action<KeyAction>? GlobalKeyPressed;

    /// <summary>The chord's key came up, with how long it was held.</summary>
    public event Action<KeyAction, TimeSpan>? GlobalKeyReleased;

    /// <summary>The app's hotkey plumbing calls these on the owner thread.</summary>
    public void GlobalKeyDown(KeyAction action) => GlobalKeyPressed?.Invoke(action);

    public void GlobalKeyUp(KeyAction action, TimeSpan held) => GlobalKeyReleased?.Invoke(action, held);

    // MARK: the stream

    /// <summary>
    /// The raw stream hook (WW3's request): every <c>subscribe_events</c> item — its <c>type</c> word and the item's UTF-8 JSON —
    /// raised at the END of <see cref="HandleEvent"/>, after the Store applied it and every typed event fired, on the Store's
    /// owner thread (the app's UI thread: Store.Create marshals the client's callbacks there). An item whose typed shape
    /// failed to decode still arrives here (type from the JSON, the Store itself ignores it); an item with no string
    /// <c>type</c> never does.
    /// </summary>
    public event Action<string, byte[]>? EventReceived;

    /// <summary>Called from <see cref="HandleEvent"/> with the decoded item; routes the 149 kinds.</summary>
    private void Observe149(WireEvent e)
    {
        switch (e.Payload)
        {
            case SubagentSpawned s:
                SubagentSpawnedReceived?.Invoke(s);
                break;
            case ActivityAppended a:
                ActivityAppendedReceived?.Invoke(a);
                break;
            case DictationEvent d:
                DictationEventReceived?.Invoke(d);
                break;
            case MeetingEvent m:
                ApplyMeeting(m);
                MeetingEventReceived?.Invoke(m);
                break;
            case PendingPrompts:
                _ = RefetchMeetingAsync();
                break;
        }
    }

    private void ApplyMeeting(MeetingEvent m)
    {
        switch (m.Type)
        {
            case "Started":
                briefs.Clear();
                topics = new ValueList<Topic>();
                live.Clear();
                OnPropertyChanged(nameof(Briefs));
                OnPropertyChanged(nameof(Topics));
                OnPropertyChanged(nameof(Live));
                _ = RefetchMeetingAsync();
                break;
            case "State":
                if (meeting is { } cur && m.State is { } st)
                {
                    meeting = cur with { State = st, Degraded = m.Degraded, Meeting = m.Meeting.Length > 0 ? m.Meeting : cur.Meeting };
                    OnPropertyChanged(nameof(Meeting));
                    OnPropertyChanged(nameof(MeetingLive));
                }
                else
                {
                    _ = RefetchMeetingAsync();
                }
                break;
            case "Level":
                meetingMicLevel = m.MicRms;
                meetingSystemLevel = m.SystemRms;
                OnPropertyChanged(nameof(MeetingMicLevel));
                OnPropertyChanged(nameof(MeetingSystemLevel));
                break;
            case "Utterance":
                if (m.IsFinal ?? true)
                {
                    if (m.AsUtterance is { } u)
                    {
                        live.Add(u);
                        if (live.Count > LiveCap) live.RemoveRange(0, live.Count - LiveCap);
                        OnPropertyChanged(nameof(Live));
                    }
                }
                break;
            case "TopicAdded":
                if (m.TopicAdded is { } t)
                {
                    var next = new ValueList<Topic>(topics.Where(x => x.Id != t.Id)) { t };
                    topics = next;
                    OnPropertyChanged(nameof(Topics));
                }
                break;
            case "TopicRemoved":
                if (m.TopicRemovedId is { } id)
                {
                    topics = new ValueList<Topic>(topics.Where(x => x.Id != id));
                    OnPropertyChanged(nameof(Topics));
                }
                break;
            case "Brief":
                if (m.Brief is { } b)
                {
                    briefs[b.Id] = b;
                    OnPropertyChanged(nameof(Briefs));
                }
                break;
            case "Listening":
                meetingListening = m.Held ?? false;
                OnPropertyChanged(nameof(MeetingListening));
                break;
            case "ApprovalWaiting":
                if (meeting is { } cur2) meeting = cur2 with { ApprovalWaiting = m.Waiting ?? false };
                OnPropertyChanged(nameof(Meeting));
                break;
            case "Deleted":
            case "ReviewReady":
            case "Summarized":
            case "PostProgress":
                _ = RefetchMeetingAsync();
                break;
        }
    }

    // MARK: refetch

    /// <summary><c>list_sessions</c> + <c>sessions_status</c> + <c>mesh_status</c>; each tolerated alone (an older engine answers -32601 to some).</summary>
    public async Task Refetch149Async()
    {
        if (fetching149)
        {
            fetchAgain149 = true;
            return;
        }
        fetching149 = true;
        try
        {
            var s = engine.ListSessionsAsync();
            var st = engine.SessionsStatusAsync();
            var m = engine.MeshStatusAsync();
            try { sessions = await s.ConfigureAwait(true); OnPropertyChanged(nameof(Sessions)); }
            catch (Exception e) { log($"list_sessions failed: {Describe(e)}"); }
            try { sessionsStatus = await st.ConfigureAwait(true); OnPropertyChanged(nameof(SessionsStatus)); }
            catch (Exception e) { log($"sessions_status failed: {Describe(e)}"); }
            try { mesh = await m.ConfigureAwait(true); OnPropertyChanged(nameof(Mesh)); }
            catch (Exception e) { log($"mesh_status failed: {Describe(e)}"); }
            SessionsRefetched?.Invoke();
        }
        finally
        {
            fetching149 = false;
        }
        if (fetchAgain149)
        {
            fetchAgain149 = false;
            await Refetch149Async().ConfigureAwait(true);
        }
    }

    /// <summary><c>meeting_status</c>, then — when a meeting exists — <c>meeting_briefs</c> and <c>meeting_live</c>; topics come with the status.</summary>
    public async Task RefetchMeetingAsync()
    {
        if (fetchingMeeting)
        {
            fetchMeetingAgain = true;
            return;
        }
        fetchingMeeting = true;
        try
        {
            MeetingStatus status;
            try { status = await engine.MeetingStatusAsync().ConfigureAwait(true); }
            catch (Exception e)
            {
                log($"meeting_status failed: {Describe(e)}");
                return;
            }
            meeting = status;
            topics = status.Topics;
            OnPropertyChanged(nameof(Meeting));
            OnPropertyChanged(nameof(MeetingLive));
            OnPropertyChanged(nameof(Topics));
            OnPropertyChanged(nameof(MeetingCursor));
            if (status.Meeting is { } id && id.Length > 0)
            {
                try
                {
                    var bs = await engine.MeetingBriefsAsync(id).ConfigureAwait(true);
                    foreach (var b in bs) briefs[b.Id] = b;
                    OnPropertyChanged(nameof(Briefs));
                }
                catch (Exception e) { log($"meeting_briefs failed: {Describe(e)}"); }
                try
                {
                    var us = await engine.MeetingLiveAsync(id).ConfigureAwait(true);
                    live.Clear();
                    live.AddRange(us.Count > LiveCap ? us.Skip(us.Count - LiveCap) : us);
                    OnPropertyChanged(nameof(Live));
                }
                catch (Exception e) { log($"meeting_live failed: {Describe(e)}"); }
            }
            MeetingRefetched?.Invoke();
        }
        finally
        {
            fetchingMeeting = false;
        }
        if (fetchMeetingAgain)
        {
            fetchMeetingAgain = false;
            await RefetchMeetingAsync().ConfigureAwait(true);
        }
    }

    // MARK: thin actions (each a straight engine call; the caller owns receipts and errors)

    /// <summary><c>cancel(session, "tree")</c>: stop the agent and its running descendants (the agents row ✕ / the card's X after the hold).</summary>
    public Task CancelTreeAsync(string session) => engine.CancelAsync(session, CancelScope.Tree);

    /// <summary><c>send_input(session, text, files, null, null)</c> with engine-side file paths attached (point-and-say's PNGs).</summary>
    public Task<string> SendInputAsync(string session, string text, IReadOnlyList<string> files) => engine.SendInputAsync(session, text, files);

    /// <summary><c>steer(session, text, null, files, null)</c> with files.</summary>
    public Task<string> SteerAsync(string session, string text, IReadOnlyList<string> files) => engine.SteerAsync(session, text, files);

    /// <summary><c>screen_context(req)</c>: hide the overlays first.</summary>
    public Task<ScreenContextReply> ScreenContextAsync(ScreenContextReq req) => engine.ScreenContextAsync(req);

    /// <summary><c>put_attachment(base64, ext)</c> → the engine-side path.</summary>
    public Task<string> PutAttachmentAsync(string base64, string ext) => engine.PutAttachmentAsync(base64, ext);

    public Task MeetingPauseAsync() => engine.MeetingPauseAsync();

    public Task MeetingResumeAsync() => engine.MeetingResumeAsync();

    public Task MeetingStopAsync() => engine.MeetingStopAsync();

    /// <summary>158 §5.4: <c>meeting_start</c> from the prompt (<c>process</c> = the detected pid, <c>consent: true</c>, <c>source: detected</c>).</summary>
    public Task<MeetingStarted> MeetingStartAsync(StartMeeting start) => engine.MeetingStartAsync(start);

    /// <summary>158 §5.3: "Not now" (<c>never: false</c>) · "Never for &lt;App&gt;" (<c>never: true</c>).</summary>
    public Task MeetingDetectionDismissAsync(string appId, bool never) => engine.MeetingDetectionDismissAsync(appId, never);

    public Task MeetingTapAsync(string topic) => engine.MeetingTapAsync(topic);

    public Task MeetingDismissAsync(string topic) => engine.MeetingDismissAsync(topic);

    public Task MeetingDeeperAsync(string brief) => engine.MeetingDeeperAsync(brief);

    public Task MeetingRequestAsync(string text) => engine.MeetingRequestAsync(text);

    public Task MeetingPttAsync(bool held) => engine.MeetingPttAsync(held);

    /// <summary><c>meeting_cursor(brief)</c>: which brief the island shows (null = none).</summary>
    public Task MeetingCursorAsync(string? brief) => engine.MeetingCursorAsync(brief);

    public Task<CachedBrief> BriefGetAsync(string id) => engine.BriefGetAsync(id);

    public Task<ValueList<CachedBrief>> BriefSearchAsync(string query, uint? limit = null) => engine.BriefSearchAsync(query, limit);

    /// <summary><c>speak(session, text)</c> → the speech id (the point-and-say reply when "speak the answer" is on).</summary>
    public Task<string> SpeakAsync(string session, string text) => engine.SpeakAsync(session, text);

    public Task StopSpeakingAsync(string? id = null) => engine.StopSpeakingAsync(id);

    public Task AgentPauseAsync(string name, bool paused) => engine.AgentPauseAsync(name, paused);
}
