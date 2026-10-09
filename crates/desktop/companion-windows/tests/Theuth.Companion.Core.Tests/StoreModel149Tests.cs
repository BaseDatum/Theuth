using System.Text;
using Theuth.Companion.Rpc;

namespace Theuth.Companion.Core.Tests;

/// <summary>A recording engine for the 149 members: the W1 calls answer empty, the 149 ones from settable fields; any can be made to fail.</summary>
internal sealed class Engine149 : IStoreEngine
{
    public readonly List<string> Calls = new();
    public ValueList<SessionSummary> Sessions = new();
    public SessionsStatus? Status = new() { Engine = "eng-a", Here = 1, Elsewhere = 1 };
    public MeshStatus? Mesh = new() { SelfId = "eng-a", SelfName = "mac", Peers = ValueList.Of(new MeshStatus.Peer { Id = "eng-b", Name = "build-b", Reachable = true }) };
    public MeetingStatus? Meeting = new();
    public ValueList<Brief> BriefsReply = new();
    public ValueList<Utterance> LiveReply = new();
    public ValueList<AgentInfo> ActivityReply = new();
    public HashSet<string> Failing = new(StringComparer.Ordinal);

    private Task<T> Reply<T>(string call, T? value) where T : class
    {
        Calls.Add(call);
        if (Failing.Contains(call) || value is null) return Task.FromException<T>(new RpcException(-32601, "Method not found"));
        return Task.FromResult(value);
    }

    private Task Done(string call) { Calls.Add(call); return Task.CompletedTask; }

    public Task<ValueList<AttentionItem>> AttentionListAsync() => Reply("attention_list", new ValueList<AttentionItem>());
    public Task<ValueList<AgentSummary>> AgentsListAsync() => Reply("agents_list", new ValueList<AgentSummary>());
    public Task<ValueList<AgentInfo>> ListActivityAsync() => Reply("list_activity", ActivityReply);
    public Task<Packet?> PacketGetAsync(string key) => Task.FromResult<Packet?>(null);
    public Task<bool> AnswerAskAsync(string id, IReadOnlyList<Answer>? answers) => Task.FromResult(true);
    public Task<bool> ResolveApprovalAsync(string id, Decision decision) => Task.FromResult(true);
    public Task TaskDecideAsync(string decision, string answer) => Done("task_decide");
    public Task TaskMoveAsync(string id, string state, string? note) => Done("task_move");
    public Task<string> SendInputAsync(string session, string text) => Reply("send_input", session);
    public Task AttentionDismissAsync(string id) => Done("attention_dismiss");
    public Task AttentionSnoozeAsync(string id, ulong untilMs) => Done("attention_snooze");
    public Task<string> SteerAsync(string session, string text) => Reply("steer", "in-1");
    public Task<MeshStatus> MeshStatusAsync() => Reply("mesh_status", Mesh);
    public Task<string> DeskOpenAsync(string name) => Reply("desk_open", "desk-1");
    public Task SetEventFocusAsync(IReadOnlyList<string> sessions) => Done("set_event_focus");
    public Task<ValueList<ActivityEntry>> ActivityListAsync(ActivityFilter filter) => Reply("activity_list", new ValueList<ActivityEntry>());

    public Task<ValueList<SessionSummary>> ListSessionsAsync() => Reply("list_sessions", Sessions);
    public Task<SessionsStatus> SessionsStatusAsync() => Reply("sessions_status", Status);
    public Task<MeetingStatus> MeetingStatusAsync() => Reply("meeting_status", Meeting);
    public Task<ValueList<Brief>> MeetingBriefsAsync(string meeting) => Reply($"meeting_briefs {meeting}", BriefsReply);
    public Task<ValueList<Utterance>> MeetingLiveAsync(string meeting) => Reply($"meeting_live {meeting}", LiveReply);
    public Task CancelAsync(string session, CancelScope? scope) => Done($"cancel {session} {scope?.Word() ?? "null"}");
    public Task<string> SendInputAsync(string? session, string text, IReadOnlyList<string> files) => Reply($"send_input {session} {text} [{string.Join(",", files)}]", "s-9");
    public Task MeetingPttAsync(bool held) => Done($"meeting_ptt {held}");
    public Task MeetingCursorAsync(string? brief) => Done($"meeting_cursor {brief ?? "null"}");
    public Task<string> SpeakAsync(string session, string text) => Reply($"speak {session} {text}", "sp-1");
}

/// <summary>Design 149 WW1 (§10): the Store's 149 members on the recording engine — stream routing, the raw hook, refetches, the joins.</summary>
public sealed class StoreModel149Tests
{
    private readonly ManualScheduler clock = new();
    private readonly Engine149 engine = new();
    private readonly List<string> logs = new();
    private readonly StoreModel store;

    public StoreModel149Tests() => store = new StoreModel(engine, clock, () => clock.Now, logs.Add);

    private static byte[] Json(string s) => Encoding.UTF8.GetBytes(s);

    private static readonly byte[] Pending = Json("""{"type":"PendingPrompts","approvals":[],"asks":[],"secrets":[]}""");

    [Fact]
    public async Task PendingPromptsFetchesTheMeetingAndTheRefetchFetchesSessions()
    {
        engine.Sessions = ValueList.Of(
            new SessionSummary { Id = "s1", Title = "fix the build", Running = true, Owner = "eng-a", Here = true, Agent = "omp" },
            new SessionSummary { Id = "s2", Title = "remote", Running = true, Owner = "eng-b", Here = false },
            new SessionSummary { Id = "s3", Title = "orphan", Owner = "eng-zzz", Here = false });
        engine.Meeting = new MeetingStatus { State = MeetingState.Recording, Meeting = "m1", Session = "ms", Topics = ValueList.Of(new Topic { Id = "t1", Label = "L" }), Cursor = "b1" };
        engine.BriefsReply = ValueList.Of(new Brief { Id = "b1", Title = "B" });
        engine.LiveReply = ValueList.Of(new Utterance { Track = "Mic", Text = "hi" });
        int sessionsRefetched = 0, meetingRefetched = 0;
        store.SessionsRefetched += () => sessionsRefetched++;
        store.MeetingRefetched += () => meetingRefetched++;

        store.HandleEvent(Pending);
        clock.Advance(0.3);
        await Task.Yield();
        await WaitAsync(() => sessionsRefetched >= 1 && meetingRefetched >= 1);

        Assert.Equal(3, store.Sessions.Count);
        Assert.Equal("eng-a", store.SessionsStatus!.Engine);
        Assert.Equal("eng-a", store.SelfEngineId);
        Assert.Equal("mac", store.Mesh!.SelfName);
        Assert.Equal("", store.HostNameOf(store.Sessions[0]));
        Assert.Equal("build-b", store.HostNameOf(store.Sessions[1]));
        Assert.Equal("eng-zzz", store.HostNameOf(store.Sessions[2]));
        Assert.True(store.MeetingLive);
        Assert.Equal("m1", store.Meeting!.Meeting);
        Assert.Equal("b1", store.MeetingCursor);
        Assert.Single(store.Topics);
        Assert.Equal("B", store.Briefs["b1"].Title);
        Assert.Equal("me", store.Live[0].Who);
        Assert.Contains("meeting_briefs m1", engine.Calls);
        Assert.Contains("meeting_live m1", engine.Calls);
    }

    [Fact]
    public async Task AnOlderEngineIsToleratedCallByCall()
    {
        engine.Failing.Add("sessions_status");
        engine.Failing.Add("meeting_status");
        engine.Sessions = ValueList.Of(new SessionSummary { Id = "s1", Title = "t" });
        int n = 0;
        store.SessionsRefetched += () => n++;
        await store.Refetch149Async();
        await store.RefetchMeetingAsync();
        Assert.Equal(1, n);
        Assert.Single(store.Sessions);
        Assert.Null(store.SessionsStatus);
        Assert.Equal("eng-a", store.SelfEngineId); // falls back to mesh_status.self_id
        Assert.Null(store.Meeting);
        Assert.False(store.MeetingLive);
        Assert.Contains(logs, l => l.StartsWith("sessions_status failed", StringComparison.Ordinal));
        Assert.Contains(logs, l => l.StartsWith("meeting_status failed", StringComparison.Ordinal));
    }

    [Fact]
    public void MeetingEventsUpdateTheIslandStateAndReachBothHooks()
    {
        var order = new List<string>();
        store.MeetingEventReceived += m => order.Add("typed:" + m.Type);
        store.EventReceived += (t, _) => order.Add("raw:" + t);

        store.HandleEvent(Json("""{"type":"TopicAdded","meeting":"m1","topic":{"id":"t1","label":"ticket","at_ms":1}}"""));
        store.HandleEvent(Json("""{"type":"TopicAdded","meeting":"m1","topic":{"id":"t2","label":"two","at_ms":2}}"""));
        store.HandleEvent(Json("""{"type":"Brief","meeting":"m1","brief":{"id":"b1","topic":"t1","title":"T","bullets":["x"],"source":"Tap","at_ms":3}}"""));
        store.HandleEvent(Json("""{"type":"TopicRemoved","meeting":"m1","topic":"t1","why":"Viewed"}"""));
        store.HandleEvent(Json("""{"type":"Level","meeting":"m1","mic_rms":0.4,"system_rms":0.1}"""));
        store.HandleEvent(Json("""{"type":"Listening","meeting":"m1","held":true}"""));
        store.HandleEvent(Json("""{"type":"Utterance","meeting":"m1","track":"System","t0_ms":1,"t1_ms":2,"text":"partial","request":false,"final":false}"""));
        store.HandleEvent(Json("""{"type":"Utterance","meeting":"m1","track":"System","t0_ms":1,"t1_ms":2,"text":"final line","request":false,"speaker":"Speaker 1"}"""));

        Assert.Equal(new[] { "t2" }, store.Topics.Select(t => t.Id));
        Assert.Equal("T", store.Briefs["b1"].Title);
        Assert.Equal(0.4f, store.MeetingMicLevel);
        Assert.True(store.MeetingListening);
        Assert.Single(store.Live); // the partial is not kept
        Assert.Equal("Speaker 1", store.Live[0].Who);
        // Typed before raw, raw last, for every item.
        Assert.Equal(16, order.Count);
        for (int i = 0; i < order.Count; i += 2)
        {
            Assert.StartsWith("typed:", order[i]);
            Assert.StartsWith("raw:", order[i + 1]);
            Assert.Equal(order[i][6..], order[i + 1][4..]);
        }
    }

    [Fact]
    public void TheRawHookSeesUnknownAndUndecodableItemsButNeverTypeless()
    {
        var raw = new List<(string, string)>();
        store.EventReceived += (t, b) => raw.Add((t, Encoding.UTF8.GetString(b)));
        store.HandleEvent(Json("""{"type":"TurnStarted","session":"s1","turn":"t1"}"""));
        store.HandleEvent(Json("""{"type":"Nope2027","x":1}"""));
        store.HandleEvent(Json("""{"type":"AttentionChanged"}""")); // typed, required fields missing → decode fails, still forwarded
        store.HandleEvent(Json("""{"notype":true}"""));
        store.HandleEvent(Json("""not json"""));
        Assert.Equal(new[] { "TurnStarted", "Nope2027", "AttentionChanged" }, raw.Select(r => r.Item1));
        Assert.Contains("\"turn\":\"t1\"", raw[0].Item2);
    }

    [Fact]
    public void SubagentActivityAndDictationItemsAreTyped()
    {
        SubagentSpawned? spawned = null;
        ActivityAppended? appended = null;
        var dict = new List<string>();
        store.SubagentSpawnedReceived += s => spawned = s;
        store.ActivityAppendedReceived += a => appended = a;
        store.DictationEventReceived += d => dict.Add(d.Type);
        store.HandleEvent(Json("""{"type":"SubagentSpawned","parent":"p","child":"c","name":"ww1"}"""));
        store.HandleEvent(Json("""{"type":"ActivityAppended","entry":{"id":"a1","at_ms":1,"kind":"ask_answered","text":"x","by":"user@companion"}}"""));
        store.HandleEvent(Json("""{"type":"DictationStarted","id":"d1"}"""));
        store.HandleEvent(Json("""{"type":"DictationLevel","id":"d1","levels":[0.2]}"""));
        store.HandleEvent(Json("""{"type":"DictationText","id":"d1","utterance":0,"text":"hel","final":false}"""));
        store.HandleEvent(Json("""{"type":"DictationFinished","id":"d1","reason":{"reason":"stopped"}}"""));
        Assert.Equal("c", spawned!.Child);
        Assert.IsType<JsonValue.Object>(appended!.Entry);
        Assert.Equal(new[] { "DictationStarted", "DictationLevel", "DictationText", "DictationFinished" }, dict);
        Assert.Equal(1, clock.Pending); // SubagentSpawned still schedules the W1 refetch
    }

    [Fact]
    public async Task JoinsAndThinActions()
    {
        engine.ActivityReply = ValueList.Of(
            new AgentInfo { Id = "s1", Name = "root", Last = "done: 3 files", WireActivity = new SessionActivity { Children = 2, Live = true } },
            new AgentInfo { Id = "c1", Name = "child", Parent = "s1", Last = "x" },
            new AgentInfo { Id = "c2", Name = "child2", Parent = "s1", Last = "y" });
        engine.Sessions = ValueList.Of(new SessionSummary { Id = "s1", Title = "fix the build" });
        await store.Refetch();
        await store.Refetch149Async();
        var root = store.Activity[0];
        Assert.Equal(2, store.ChildrenOf(root));
        Assert.Equal(2, store.ChildrenOf(root with { WireActivity = null })); // counted from the rows when the engine does not say
        Assert.Equal("fix the build", store.SessionOf(root)!.Title);
        var ctx = store.ContextFor(new AttentionItem { Id = "ask:1", Kind = AttentionKind.Ask, Title = "T", AtMs = 1, Session = "s1" });
        Assert.Equal(("fix the build", "done: 3 files", false), (ctx.Request, ctx.LastReply, ctx.FromEngine));

        await store.CancelTreeAsync("s1");
        Assert.Equal("s-9", await store.SendInputAsync("s1", "look", new[] { "C:\\a.png" }));
        await store.MeetingPttAsync(true);
        await store.MeetingCursorAsync(null);
        Assert.Equal("sp-1", await store.SpeakAsync("s1", "hi"));
        Assert.Contains("cancel s1 tree", engine.Calls);
        Assert.Contains("send_input s1 look [C:\\a.png]", engine.Calls);
        Assert.Contains("meeting_ptt True", engine.Calls);
        Assert.Contains("meeting_cursor null", engine.Calls);
        // A member the engine lacks (default interface member) fails with NotSupportedException, never a crash.
        await Assert.ThrowsAsync<NotSupportedException>(() => store.MeetingStopAsync());
    }

    [Fact]
    public void GlobalKeysAreForwarded()
    {
        var seen = new List<string>();
        store.GlobalKeyPressed += a => seen.Add("down " + a.RawValue());
        store.GlobalKeyReleased += (a, held) => seen.Add($"up {a.RawValue()} {held.TotalMilliseconds:0}");
        store.GlobalKeyDown(KeyAction.VoiceKey);
        store.GlobalKeyUp(KeyAction.VoiceKey, TimeSpan.FromMilliseconds(250));
        Assert.Equal(new[] { "down voiceKey", "up voiceKey 250" }, seen);
    }

    private static async Task WaitAsync(Func<bool> condition, double seconds = 5)
    {
        var until = DateTime.UtcNow.AddSeconds(seconds);
        while (!condition())
        {
            if (DateTime.UtcNow > until) throw new TimeoutException("condition not met");
            await Task.Delay(10);
        }
    }
}
