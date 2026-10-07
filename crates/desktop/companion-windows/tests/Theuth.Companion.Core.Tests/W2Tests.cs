namespace Theuth.Companion.Core.Tests;

/// <summary><see cref="FakeEngine"/> plus the two calls 134 W2 added (hire · decline), recorded in the same call list.</summary>
internal sealed class W2Engine(FakeEngine inner) : IStoreEngine
{
    public FakeEngine Inner { get; } = inner;
    public Task<ValueList<AttentionItem>> AttentionListAsync() => Inner.AttentionListAsync();
    public Task<ValueList<AgentSummary>> AgentsListAsync() => Inner.AgentsListAsync();
    public Task<ValueList<AgentInfo>> ListActivityAsync() => Inner.ListActivityAsync();
    public Task<Packet?> PacketGetAsync(string key) => Inner.PacketGetAsync(key);
    public Task<bool> AnswerAskAsync(string id, IReadOnlyList<Answer>? answers) => Inner.AnswerAskAsync(id, answers);
    public Task<bool> ResolveApprovalAsync(string id, Decision decision) => Inner.ResolveApprovalAsync(id, decision);
    public Task TaskDecideAsync(string decision, string answer) => Inner.TaskDecideAsync(decision, answer);
    public Task TaskMoveAsync(string id, string state, string? note) => Inner.TaskMoveAsync(id, state, note);
    public Task<string> SendInputAsync(string session, string text) => Inner.SendInputAsync(session, text);
    public Task AttentionDismissAsync(string id) => Inner.AttentionDismissAsync(id);
    public Task AttentionSnoozeAsync(string id, ulong untilMs) => Inner.AttentionSnoozeAsync(id, untilMs);
    public Task<string> SteerAsync(string session, string text) => Inner.SteerAsync(session, text);
    public Task<MeshStatus> MeshStatusAsync() => Inner.MeshStatusAsync();
    public Task<string> DeskOpenAsync(string name) => Inner.DeskOpenAsync(name);
    public Task SetEventFocusAsync(IReadOnlyList<string> sessions) => Inner.SetEventFocusAsync(sessions);
    public Task<ValueList<ActivityEntry>> ActivityListAsync(ActivityFilter filter) => Inner.ActivityListAsync(filter);

    public Task AgentApproveAsync(string name)
    {
        Inner.Calls.Add($"agent_approve {name}");
        return Task.CompletedTask;
    }

    public Task AgentRemoveAsync(string name, string? note)
    {
        Inner.Calls.Add($"agent_remove {name} {note ?? "null"}");
        return Task.CompletedTask;
    }
}

/// <summary>
/// 134 W2: the per-kind options 130 §4.2 gives hire (Hire · Decline) and blocked (Unblock) cards, which the Swift port
/// left option-less, through the same hold → E5 re-read → send path as every other keystroke.
/// </summary>
public sealed class W2Tests
{
    private readonly ManualScheduler clock = new();
    private readonly FakeEngine fake = new();
    private readonly StoreModel store;

    public W2Tests() => store = new StoreModel(new W2Engine(fake), clock, () => clock.Now);

    private static AttentionItem Hire(string name) => new()
    {
        Id = $"hire:{name}", Kind = AttentionKind.Hire, Title = $"agent:theuth proposes hiring {name} (Docs)", AtMs = 1_791_448_000_000,
        Agent = name, Task = "t-hire",
    };

    private static AttentionItem Blocked(string task, string? decision = null) => new()
    {
        Id = $"blocked:{task}", Kind = AttentionKind.Blocked, Title = "waiting on the vendor", AtMs = 1_791_448_000_000, Task = task,
        Decision = decision,
    };

    [Fact]
    public void HireCardsOfferHireAndDeclineWithNoPrePick()
    {
        var c = Card.Make(Hire("docs"));
        Assert.Equal(new[] { "Hire", "Decline" }, c.Options.Select(o => o.Label));
        Assert.Equal(new[] { 1, 2 }, c.Options.Select(o => o.Key));
        Assert.Null(c.Recommended); // P13: Enter is inert on a hire
        Assert.Equal(new CardAction.HireAgent("docs"), c.Options[0].Action);
        Assert.Equal(new CardAction.DeclineHire("docs"), c.Options[1].Action);
        Assert.False(c.Clearable);
        Assert.Empty(Card.Make(Hire("docs") with { Agent = null }).Options); // nothing to name: no options
    }

    [Fact]
    public void BlockedCardsOfferUnblockUnlessTheBlockerIsADecision()
    {
        var c = Card.Make(Blocked("t-7"));
        var only = Assert.Single(c.Options);
        Assert.Equal("Unblock", only.Label);
        Assert.Equal(new CardAction.Unblock("t-7"), only.Action);
        Assert.False(only.Recommended);
        Assert.Empty(Card.Make(Blocked("t-7", decision: "d-1")).Options);
        Assert.Empty(Card.Make(Blocked("t-7") with { Task = null }).Options);
    }

    [Fact]
    public async Task OptionOneOnAHireHoldsThenReReadsThenApproves()
    {
        fake.Items = ValueList.Of(Hire("docs"));
        await store.Refetch();
        fake.Calls.Clear();
        store.Perform(Intent.Answer(1));
        var holding = Assert.IsType<StoreModel.ReceiptRow.Holding>(store.Receipt);
        Assert.Equal("Hire", holding.Held.Label);
        Assert.Equal("docs", holding.Held.Who);
        clock.Advance(2.9);
        Assert.Empty(fake.Calls);
        clock.Advance(0.1);
        Assert.Equal("attention_list", fake.Calls[0]); // E5
        Assert.Equal("agent_approve docs", fake.Calls[1]);
        Assert.Equal(new StoreModel.ReceiptRow.Sent("docs · Hire"), store.Receipt);
    }

    [Fact]
    public async Task DeclineRemovesTheProposalAndUnblockMovesTheTaskToTodo()
    {
        fake.Items = ValueList.Of(Hire("docs"), Blocked("t-7"));
        await store.Refetch();
        fake.Calls.Clear();
        store.FocusOn("hire:docs");
        store.Perform(Intent.Answer(2));
        clock.Advance(3);
        Assert.Contains("agent_remove docs null", fake.Calls);
        fake.Calls.Clear();
        store.FocusOn("blocked:t-7");
        store.Perform(Intent.Answer(1));
        clock.Advance(3);
        Assert.Contains("task_move t-7 todo null", fake.Calls);
    }

    [Fact]
    public async Task AHireAnsweredElsewhereIsStaleAndNeverSent()
    {
        fake.Items = ValueList.Of(Hire("docs"));
        await store.Refetch();
        store.Perform(Intent.Answer(1));
        fake.Items = new(); // approved in the main window during the hold
        fake.Calls.Clear();
        clock.Advance(3);
        Assert.Equal(StoreModel.ReceiptRow.Stale.Instance, store.Receipt);
        Assert.DoesNotContain(fake.Calls, c => c.StartsWith("agent_approve", StringComparison.Ordinal));
    }

    [Fact]
    public async Task AnEngineWithoutTheW2CallsFailsTheCommitVisibly()
    {
        var plain = new StoreModel(fake, clock, () => clock.Now); // FakeEngine predates W2: the interface defaults answer
        fake.Items = ValueList.Of(Hire("docs"));
        await plain.Refetch();
        plain.Perform(Intent.Answer(1));
        clock.Advance(3);
        var failed = Assert.IsType<StoreModel.ReceiptRow.Failed>(plain.Receipt);
        Assert.Contains("agent_approve", failed.Error);
    }
}
