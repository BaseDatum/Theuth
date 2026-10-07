using System.Text;

namespace Theuth.Companion.Core.Tests;

/// <summary>Virtual time for the Store: timers fire on <see cref="Advance"/>, the clock moves with them.</summary>
internal sealed class ManualScheduler : IStoreScheduler
{
    private sealed class Entry(DateTimeOffset due, TimeSpan? every, Action action) : IDisposable
    {
        public DateTimeOffset Due = due;
        public readonly TimeSpan? Every = every;
        public readonly Action Action = action;
        public bool Cancelled;
        public void Dispose() => Cancelled = true;
    }

    private readonly List<Entry> entries = new();
    public DateTimeOffset Now { get; private set; } = DateTimeOffset.FromUnixTimeSeconds(1_791_448_800);

    public IDisposable After(TimeSpan delay, Action action) => Add(new Entry(Now + delay, null, action));

    public IDisposable Every(TimeSpan interval, Action action) => Add(new Entry(Now + interval, interval, action));

    private Entry Add(Entry e)
    {
        entries.Add(e);
        return e;
    }

    public int Pending => entries.Count(e => !e.Cancelled);

    public void Advance(double seconds)
    {
        var until = Now.AddSeconds(seconds);
        while (true)
        {
            var next = entries.Where(e => !e.Cancelled && e.Due <= until).OrderBy(e => e.Due).FirstOrDefault();
            if (next is null) break;
            Now = next.Due;
            if (next.Every is { } every) next.Due = next.Due + every;
            else next.Cancelled = true;
            next.Action();
        }
        Now = until;
        entries.RemoveAll(e => e.Cancelled);
    }
}

/// <summary>Records every call; answers from settable lists; a call can be made to hang on a TaskCompletionSource.</summary>
internal sealed class FakeEngine : IStoreEngine
{
    public readonly List<string> Calls = new();
    public ValueList<AttentionItem> Items = new();
    public ValueList<AgentSummary> AgentsReply = new();
    public ValueList<AgentInfo> ActivityReply = new();
    public Dictionary<string, Packet> PacketsByKey = new();
    public ValueList<ActivityEntry> ActivityEntries = new();
    public bool AnswerResult = true;
    public Exception? Fail;
    public TaskCompletionSource<ValueList<AttentionItem>>? HangAttention;
    public bool NoMesh = true;
    public ActivityFilter? LastFilter;

    private Task<T> Reply<T>(string call, T value)
    {
        Calls.Add(call);
        return Fail is { } f ? Task.FromException<T>(f) : Task.FromResult(value);
    }

    private Task Done(string call) => Reply(call, true);

    public Task<ValueList<AttentionItem>> AttentionListAsync()
    {
        if (HangAttention is { } h)
        {
            Calls.Add("attention_list");
            return h.Task;
        }
        return Reply("attention_list", Items);
    }

    public Task<ValueList<AgentSummary>> AgentsListAsync() => Reply("agents_list", AgentsReply);
    public Task<ValueList<AgentInfo>> ListActivityAsync() => Reply("list_activity", ActivityReply);
    public Task<Packet?> PacketGetAsync(string key) => Reply($"packet_get {key}", PacketsByKey.TryGetValue(key, out var p) ? p : null);
    public Task<bool> AnswerAskAsync(string id, IReadOnlyList<Answer>? answers) => Reply($"answer_ask {id} {TheuthJson.Serialize(answers?.ToList())}", AnswerResult);
    public Task<bool> ResolveApprovalAsync(string id, Decision decision) => Reply($"resolve_approval {id} {decision.RawValue()}", AnswerResult);
    public Task TaskDecideAsync(string decision, string answer) => Done($"task_decide {decision} {answer}");
    public Task TaskMoveAsync(string id, string state, string? note) => Done($"task_move {id} {state} {note ?? "null"}");
    public Task<string> SendInputAsync(string session, string text) => Reply($"send_input {session} {text}", session);
    public Task AttentionDismissAsync(string id) => Done($"attention_dismiss {id}");
    public Task AttentionSnoozeAsync(string id, ulong untilMs) => Done($"attention_snooze {id} {untilMs}");
    public Task<string> SteerAsync(string session, string text) => Reply($"steer {session} {text}", "input-1");

    public Task<MeshStatus> MeshStatusAsync()
    {
        Calls.Add("mesh_status");
        return NoMesh ? Task.FromException<MeshStatus>(new RpcException(-32601, "Method not found")) : Task.FromResult(new MeshStatus { SelfId = "e1", SelfName = "box", LeaseInfo = new MeshStatus.Lease { Holder = "e1" } });
    }

    public Task<string> DeskOpenAsync(string name) => Reply($"desk_open {name}", "desk-1");
    public Task SetEventFocusAsync(IReadOnlyList<string> sessions) => Done($"set_event_focus {string.Join(",", sessions)}");

    public Task<ValueList<ActivityEntry>> ActivityListAsync(ActivityFilter filter)
    {
        LastFilter = filter;
        return Reply("activity_list", ActivityEntries);
    }

    public int Count(string prefix) => Calls.Count(c => c.StartsWith(prefix, StringComparison.Ordinal));
}

/// <summary>The Store's paths (Swift Store.swift + Store+Ask.swift) on virtual time with a recording engine.</summary>
public sealed class StoreModelTests
{
    private readonly ManualScheduler clock = new();
    private readonly FakeEngine engine = new();
    private readonly List<string> logs = new();
    private readonly StoreModel store;

    public StoreModelTests() => store = new StoreModel(engine, clock, () => clock.Now, logs.Add);

    private static byte[] Json(string s) => Encoding.UTF8.GetBytes(s);

    private static readonly byte[] Pending = Json("""{"type":"PendingPrompts","approvals":[],"asks":[],"secrets":[]}""");

    private static AttentionItem Ask(string id, string askId = "ask-1") => new()
    {
        Id = id, Kind = AttentionKind.Ask, Title = id, AtMs = 1_791_448_000_000, Agent = "w2", Session = "s-2",
        Ask = new AskRequested { Id = askId, Session = "s-2", Questions = ValueList.Of(new Question { Id = "q", Text = "Which?", Options = ValueList.Of(new Choice { Label = "A" }, new Choice { Label = "B" }), Recommended = 1 }) },
    };

    private static AttentionItem Review(string task) => new()
    {
        Id = $"review:{task}", Kind = AttentionKind.Review, Title = "review", AtMs = 1_791_448_000_000, Agent = "ops", Task = task, Packet = $"review:{task}",
    };

    [Fact]
    public void EventsDebounceIntoOneRefetchAndTheImmediateOnesDoNot()
    {
        var approval = Json("""{"type":"ApprovalRequested","id":"ap1","session":"s","tool":"shell","args":{},"risk":"mutating"}""");
        store.HandleEvent(approval);
        store.HandleEvent(Json("""{"type":"AskResolved","id":"x","dismissed":false,"timed_out":false}"""));
        clock.Advance(0.1);
        store.HandleEvent(Json("""{"type":"ActivityAppended"}"""));
        clock.Advance(0.2);
        Assert.Equal(0, engine.Count("attention_list")); // 250 ms after the last event
        clock.Advance(0.06);
        Assert.Equal(1, engine.Count("attention_list"));
        Assert.True(store.Approvals.ContainsKey("ap1"));
        store.HandleEvent(Pending); // PendingPrompts: at once, and it replaces the approvals
        Assert.Equal(2, engine.Count("attention_list"));
        Assert.Empty(store.Approvals);
        store.HandleEvent(Json("""{"type":"Lagged","count":3}"""));
        store.HandleEvent(Json("""{"type":"AttentionChanged","count":0,"ids_added":[],"ids_removed":[],"at_ms":1}"""));
        Assert.Equal(3, engine.Count("attention_list")); // Lagged at once; AttentionChanged debounced
        clock.Advance(0.25);
        Assert.Equal(4, engine.Count("attention_list"));
        store.HandleEvent(Json("""{"type":"TextDelta","session":"s","text":"x"}"""));
        clock.Advance(1);
        Assert.Equal(4, engine.Count("attention_list")); // other stream items do not refetch
    }

    [Fact]
    public void ARefetchAskedForDuringOneRunsOnceMoreAfterIt()
    {
        engine.HangAttention = new TaskCompletionSource<ValueList<AttentionItem>>();
        var first = store.Refetch();
        _ = store.Refetch();
        _ = store.Refetch();
        Assert.Equal(1, engine.Count("attention_list"));
        var gate = engine.HangAttention;
        engine.HangAttention = null;
        gate.SetResult(new ValueList<AttentionItem>());
        Assert.True(first.IsCompleted);
        Assert.Equal(2, engine.Count("attention_list")); // coalesced into exactly one more (Swift's defer ordering lost it)
    }

    [Fact]
    public async Task RefetchPublishesListsRailPanelPacketsAndReceipts()
    {
        engine.Items = ValueList.Of(Review("t-9"), Ask("ask:1"));
        engine.PacketsByKey["review:t-9"] = new Packet { Key = "review:t-9", Task = "t-9", Kind = PacketKind.Review, Summary = "Done." };
        engine.ActivityEntries = ValueList.Of(new ActivityEntry { Id = 7, TsMs = (ulong)clock.Now.AddSeconds(-60).ToUnixTimeMilliseconds(), Kind = "ask_answered", Actor = "user@companion", Subject = "ask:9", Summary = "answered" });
        var changed = new List<string?>();
        var refetched = 0;
        store.PropertyChanged += (_, e) => changed.Add(e.PropertyName);
        store.Refetched += () => refetched++;
        await store.Refetch();
        await store.Refetch();
        Assert.Equal(2, store.Items.Count);
        Assert.Equal(2, store.Rail.NeedsYouCount);
        Assert.Equal("2 need you", store.Panel.Headline);
        Assert.Equal("Done.", store.CardFor(store.Items.First(i => i.Kind == AttentionKind.Review)).Why);
        Assert.Equal(1, engine.Count("packet_get")); // once per key
        Assert.Equal(1, store.Panel.ReceiptCount);
        Assert.Equal((uint?)200, engine.LastFilter?.Limit);
        Assert.Equal((ulong?)clock.Now.AddSeconds(-ReceiptModel.Window).ToUnixTimeMilliseconds(), engine.LastFilter?.SinceMs);
        Assert.Equal(2, refetched);
        foreach (var p in new[] { "Items", "Agents", "Activity", "Rail", "Panel", "Packets", "Ask" }) Assert.Contains(p, changed);
    }

    [Fact]
    public async Task AHoldTicksThenReReadsThenSends()
    {
        engine.Items = ValueList.Of(Ask("ask:1"));
        await store.Refetch();
        engine.Calls.Clear();
        store.Perform(Intent.Answer(2));
        var holding = Assert.IsType<StoreModel.ReceiptRow.Holding>(store.Receipt);
        Assert.Equal("B", holding.Held.Label);
        Assert.Equal(3, store.HoldRemaining, 3);
        clock.Advance(2.9);
        Assert.Empty(engine.Calls);
        Assert.Equal(0.1, store.HoldRemaining, 3);
        clock.Advance(0.1);
        Assert.Equal("attention_list", engine.Calls[0]); // E5: re-read first
        Assert.Equal("""answer_ask ask-1 [{"index":1,"kind":"choice"}]""", engine.Calls[1]);
        Assert.Equal(new StoreModel.ReceiptRow.Sent("w2 · B"), store.Receipt);
        clock.Advance(3);
        Assert.Null(store.Receipt); // expired
        Assert.Equal(0, clock.Pending);
    }

    [Fact]
    public async Task EscapeTakesTheHoldBackThenCloses()
    {
        engine.Items = ValueList.Of(Ask("ask:1"));
        await store.Refetch();
        engine.Calls.Clear();
        var closed = 0;
        store.CloseRequested += () => closed++;
        store.Perform(Intent.AnswerRecommended);
        store.Perform(Intent.Escape);
        Assert.Equal(StoreModel.ReceiptRow.Undone.Instance, store.Receipt);
        clock.Advance(10);
        Assert.Empty(engine.Calls); // nothing sent
        Assert.Null(store.Receipt);
        store.Perform(Intent.Escape);
        Assert.Equal(1, closed);
    }

    [Fact]
    public async Task AnsweredElsewhereIsStaleAndNeverRetried()
    {
        engine.Items = ValueList.Of(Ask("ask:1"), Ask("ask:2", "ask-2"));
        await store.Refetch();
        store.Perform(Intent.Answer(1));
        engine.Items = ValueList.Of(Ask("ask:2", "ask-2")); // answered on the phone meanwhile
        engine.Calls.Clear();
        clock.Advance(3);
        Assert.Equal(StoreModel.ReceiptRow.Stale.Instance, store.Receipt);
        Assert.DoesNotContain(engine.Calls, c => c.StartsWith("answer_ask", StringComparison.Ordinal));
        Assert.Single(store.Items); // the re-read list is applied
        // The engine's own stale check: still listed, but answer_ask says false.
        engine.AnswerResult = false;
        store.Perform(Intent.Answer(1));
        clock.Advance(3);
        Assert.Equal(StoreModel.ReceiptRow.Stale.Instance, store.Receipt);
        Assert.Equal(1, engine.Count("answer_ask"));
        clock.Advance(30);
        Assert.Equal(1, engine.Count("answer_ask")); // never retried
    }

    [Fact]
    public async Task AFailedSendSaysWhy()
    {
        engine.Items = ValueList.Of(Ask("ask:1"));
        await store.Refetch();
        store.Perform(Intent.Answer(1));
        engine.Fail = new RpcException(-32000, "boom");
        clock.Advance(3);
        Assert.Equal(new StoreModel.ReceiptRow.Failed("rpc -32000: boom"), store.Receipt);
        Assert.Contains(logs, l => l == "commit failed: rpc -32000: boom");
    }

    [Fact]
    public async Task ASecondHoldCommitsTheDisplacedOneAtOnce()
    {
        engine.Items = ValueList.Of(Ask("ask:1"), Ask("ask:2", "ask-2"));
        await store.Refetch();
        engine.Calls.Clear();
        store.Perform(Intent.Answer(1)); // ask:2 is the focused card? order: oldest first by waiting_since, then id
        var firstHeld = Assert.IsType<StoreModel.ReceiptRow.Holding>(store.Receipt).Held;
        store.Perform(Intent.Snooze);
        Assert.Contains(engine.Calls, c => c.StartsWith($"answer_ask {((CardAction.AnswerAsk)firstHeld.Action).AskId}", StringComparison.Ordinal));
        Assert.IsType<StoreModel.ReceiptRow.Holding>(store.Receipt); // the snooze is now held
    }

    [Fact]
    public async Task SendBackCollectsANoteFirst()
    {
        engine.Items = ValueList.Of(Review("t-9"));
        await store.Refetch();
        engine.Calls.Clear();
        store.Perform(Intent.Answer(2)); // "Send back…"
        Assert.Equal("t-9", store.NoteFor);
        Assert.Null(store.Receipt);
        store.SubmitNote("   ");
        Assert.Equal("t-9", store.NoteFor); // empty refused
        store.SubmitNote("add the restore check");
        Assert.Null(store.NoteFor);
        clock.Advance(3);
        Assert.Contains("task_move t-9 todo add the restore check", engine.Calls);
        store.Perform(Intent.Answer(2));
        store.Perform(Intent.Escape); // Esc first drops the note field
        Assert.Null(store.NoteFor);
    }

    [Fact]
    public async Task SnoozeClearAndApprove()
    {
        engine.Items = ValueList.Of(Review("t-9"));
        await store.Refetch();
        engine.Calls.Clear();
        store.Perform(Intent.Snooze);
        clock.Advance(3);
        var until = (ulong)clock.Now.AddSeconds(KeyGrammar.SnoozeInterval).ToUnixTimeMilliseconds(); // committed at Now
        Assert.Contains($"attention_snooze review:t-9 {until}", engine.Calls);
        store.Perform(Intent.Clear); // a review is not clearable
        Assert.Equal(new StoreModel.ReceiptRow.Sent("ops · snoozed 30 m"), store.Receipt);
        store.Perform(Intent.Answer(1));
        clock.Advance(3);
        Assert.Contains("task_move t-9 done null", engine.Calls);
    }

    [Fact]
    public async Task AToastAnswerSkipsTheHoldAndCanCarryANote()
    {
        engine.Items = ValueList.Of(Ask("ask:1"));
        await store.Refetch();
        engine.Calls.Clear();
        store.AnswerNow(new StoreModel.Held("ask:1", "w2", "A", new CardAction.AnswerAsk("ask-1", ValueList.Of<Answer>(new Answer.Choice(0)))), note: "and hurry", session: "s-2");
        Assert.Equal("attention_list", engine.Calls[0]);
        Assert.StartsWith("answer_ask ask-1", engine.Calls[1]);
        Assert.Contains("send_input s-2 and hurry", engine.Calls);
        Assert.Equal(new StoreModel.ReceiptRow.Sent("w2 · A"), store.Receipt);
    }

    [Fact]
    public async Task AskTheuthRoutesThroughTheDeskAndStreamsTheReply()
    {
        store.SubmitAsk("  ship the docs  ");
        Assert.Equal("ship the docs", Assert.IsType<StoreModel.ReceiptRow.Holding>(store.Receipt).Held.Label);
        clock.Advance(3);
        Assert.Equal(new[] { "mesh_status", "desk_open theuth", "set_event_focus desk-1", "send_input desk-1 ship the docs" },
            engine.Calls.Where(c => !c.StartsWith("attention_list", StringComparison.Ordinal) && !c.StartsWith("agents_list", StringComparison.Ordinal) && !c.StartsWith("list_activity", StringComparison.Ordinal) && !c.StartsWith("activity_list", StringComparison.Ordinal)));
        Assert.Equal(new StoreModel.ReceiptRow.Reply("theuth", "sent — waiting for the desk", "desk-1"), store.Receipt);
        Assert.StartsWith("ask:", store.Ask.Current);
        store.HandleEvent(Json("""{"type":"TextDelta","session":"desk-1","text":"- handed to w3\nthen docs"}"""));
        Assert.Equal(new StoreModel.ReceiptRow.Reply("theuth", "handed to w3 — then docs", "desk-1"), store.Receipt);
        store.HandleEvent(Json("""{"type":"TurnFinished","session":"desk-1","turn":"t","stop_reason":"end_turn","context":1}"""));
        Assert.True(store.Ask.Router.Reply?.Finished);
        clock.Advance(12);
        Assert.Null(store.Receipt);
        engine.Calls.Clear();
        store.HandleEvent(Pending); // the desk focus is re-applied on every PendingPrompts
        Assert.Contains("set_event_focus desk-1", engine.Calls);
    }

    [Fact]
    public async Task ReplyOpensTheRFieldAndEscapeClosesItFirst()
    {
        engine.Items = ValueList.Of(Ask("ask:1"));
        await store.Refetch();
        store.Perform(Intent.Reply);
        Assert.Equal("ask:1", store.Ask.ReplyFor);
        store.SubmitReply("neither");
        Assert.Null(store.Ask.ReplyFor);
        var held = Assert.IsType<StoreModel.ReceiptRow.Holding>(store.Receipt).Held;
        Assert.Equal(new CardAction.AnswerAsk("ask-1", ValueList.Of<Answer>(new Answer.Text("neither"))), held.Action);
        store.Perform(Intent.Escape); // undo the hold
        store.Perform(Intent.Reply);
        store.Perform(Intent.Escape);
        Assert.Null(store.Ask.ReplyFor);
    }

    [Fact]
    public void ConnectionAndHooksAreRaised()
    {
        var states = new List<RpcClient.State>();
        var asked = 0;
        Uri? opened = null;
        store.PropertyChanged += (_, e) => { if (e.PropertyName == nameof(StoreModel.Connection)) states.Add(store.Connection); };
        store.AskBarRequested += () => asked++;
        store.OpenUrlRequested += u => opened = u;
        PendingPrompts? first = null;
        store.PendingPromptsReceived += p => first = p;
        store.HandleState(RpcClient.State.Open);
        store.Perform(Intent.AskBar);
        store.HandleEvent(Pending);
        Assert.Equal(new[] { RpcClient.State.Open }, states);
        Assert.Equal(1, asked);
        Assert.NotNull(first);
        Assert.Null(opened); // nothing focused, nothing to open
    }
}
