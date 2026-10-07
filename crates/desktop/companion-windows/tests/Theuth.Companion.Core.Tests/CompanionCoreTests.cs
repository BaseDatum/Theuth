namespace Theuth.Companion.Core.Tests;

/// <summary>Shared fixtures of <c>CompanionCoreTests.swift</c>.</summary>
internal static class CoreFx
{
    public static readonly DateTimeOffset T0 = DateTimeOffset.FromUnixTimeSeconds(1_791_448_800);

    public static ulong Ms(double secondsAgo) => (ulong)((T0.ToUnixTimeSeconds() - secondsAgo) * 1000);

    public static AttentionItem Item(string id, AttentionKind kind, string? agent = null, double ago = 60, string? session = null,
                                     string? task = null, double? waitingSinceAgo = null) => new()
    {
        Id = id,
        Kind = kind,
        Title = id,
        AtMs = Ms(ago),
        Task = task,
        Agent = agent,
        Session = session,
        Approval = kind == AttentionKind.Approval ? $"ap-{id}" : null,
        Decision = kind == AttentionKind.Decision ? $"decision:{id}" : null,
        WaitingSinceMs = waitingSinceAgo is { } w ? Ms(w) : null,
    };

    public static AgentSummary Desk(string name, AgentStatus status, string? session = null) => new()
    {
        File = new AgentFile { Name = name, Title = name, Model = "m" },
        Status = status,
        Desk = session,
    };

    public static AgentInfo Root(string id, AgentState state, bool live = true) => new()
    {
        Id = id,
        Name = "",
        Model = "claude",
        State = state,
        Activity = new SessionActivity
        {
            State = state,
            TurnStartedAt = state == AgentState.Running ? T0.AddSeconds(-120) : null,
            Live = live,
        },
    };
}

public sealed class RailModelTests
{
    private static readonly DateTimeOffset T0 = CoreFx.T0;

    [Fact]
    public void TwoZonesAndOrdering()
    {
        var items = new[]
        {
            CoreFx.Item("approval:new", AttentionKind.Approval, agent: "w3", ago: 40),
            CoreFx.Item("review:old", AttentionKind.Review, agent: "ops", ago: 660),
            CoreFx.Item("decision:mid", AttentionKind.Decision, agent: "w2", ago: 240),
            CoreFx.Item("show:x", AttentionKind.Show, ago: 10),
            CoreFx.Item("finished:1", AttentionKind.Finished, agent: "w1", ago: 360),
            // E2: `waiting_since_ms` wins over `at_ms`.
            CoreFx.Item("ask:e2", AttentionKind.Ask, agent: "w5", ago: 5, waitingSinceAgo: 900),
        };
        var agents = new[]
        {
            CoreFx.Desk("w2", AgentStatus.Working, session: "s2"), CoreFx.Desk("ops", AgentStatus.Paused), CoreFx.Desk("dead", AgentStatus.Unreachable),
            CoreFx.Desk("idle-desk", AgentStatus.Idle), CoreFx.Desk("newbie", AgentStatus.Proposed),
        };
        var activity = new[]
        {
            CoreFx.Root("s2", AgentState.Running), CoreFx.Root("s7", AgentState.Running), CoreFx.Root("s8", AgentState.Error),
            CoreFx.Root("cold", AgentState.Running, live: false), CoreFx.Root("s10", AgentState.Approval("shell")),
        };
        var rail = RailModel.Derive(items, agents, activity, T0);

        // Oldest first; show is not needs-you.
        Assert.Equal(new[] { "ask:e2", "review:old", "decision:mid", "approval:new" }, rail.NeedsYou.Select(m => m.Id));
        Assert.Equal(
            new[] { MarkState.NeedsYou(AttentionKind.Ask), MarkState.NeedsYou(AttentionKind.Review), MarkState.NeedsYou(AttentionKind.Decision), MarkState.NeedsYou(AttentionKind.Approval) },
            rail.NeedsYou.Select(m => m.State));
        // Zone 2: working · idle · finished · error · lapsed; the desk's own session (s2) is not doubled; proposed skipped; cold skipped.
        Assert.Equal(
            new[] { "agent:w2", "session:s7", "agent:idle-desk", "session:s10", "finished:1", "agent:dead", "session:s8", "agent:ops" },
            rail.Agents.Select(m => m.Id));
        Assert.Equal(
            new[] { MarkState.Working, MarkState.Working, MarkState.Idle, MarkState.Idle, MarkState.Finished, MarkState.Error, MarkState.Error, MarkState.Lapsed },
            rail.Agents.Select(m => m.State));
        Assert.Equal(4, rail.NeedsYouCount);
        Assert.Equal(2, rail.WorkingCount);
        Assert.False(rail.Quiet);
        Assert.Equal("w5, ask, 15 m", rail.NeedsYou[0].Detail);
        Assert.Equal("claude, working, running 2 m", rail.Agents[1].Detail);
        Assert.Equal(0, rail.Overflow); // exactly 12 marks fit
        Assert.Equal(12, rail.Marks.Count);
        Assert.Equal("agent:ops", rail.Marks[^1].Id);
    }

    [Fact]
    public void QuietAndOverflowKeepsNeedsYou()
    {
        var quiet = RailModel.Derive(new[] { CoreFx.Item("show:1", AttentionKind.Show) }, new[] { CoreFx.Desk("a", AgentStatus.Idle) }, Array.Empty<AgentInfo>(), T0);
        Assert.True(quiet.Quiet);
        Assert.Single(quiet.Marks);
        var many = Enumerable.Range(0, 15).Select(i => CoreFx.Item($"ask:{i}", AttentionKind.Ask, ago: i)).ToArray();
        var rail = RailModel.Derive(many, new[] { CoreFx.Desk("a", AgentStatus.Working) }, Array.Empty<AgentInfo>(), T0);
        Assert.Equal(12, rail.NeedsYou.Count);
        Assert.Empty(rail.Agents);
        Assert.Equal(4, rail.Overflow);
        Assert.Equal(15, rail.NeedsYouCount);
        Assert.Equal(1, rail.WorkingCount); // counted even when its mark is cut
    }

    [Fact]
    public void Ago()
    {
        Assert.Equal("59 s", RailModel.Ago(T0.AddSeconds(-59), T0));
        Assert.Equal("1 m", RailModel.Ago(T0.AddSeconds(-60), T0));
        Assert.Equal("2 h", RailModel.Ago(T0.AddSeconds(-7200), T0));
        Assert.Equal("1 d", RailModel.Ago(T0.AddSeconds(-90000), T0));
        Assert.Equal("0 s", RailModel.Ago(T0.AddSeconds(5), T0));
    }
}

public sealed class PanelModelTests
{
    private readonly AttentionItem[] items =
    {
        CoreFx.Item("finished:a", AttentionKind.Finished, ago: 999), CoreFx.Item("ask:b", AttentionKind.Ask, ago: 30),
        CoreFx.Item("show:c", AttentionKind.Show, ago: 5), CoreFx.Item("review:d", AttentionKind.Review, ago: 600),
        CoreFx.Item("finished:e", AttentionKind.Finished, ago: 100),
    };

    [Fact]
    public void OrderHeadlinePosition()
    {
        var p = new PanelModel(items, workingCount: 5);
        Assert.Equal(new[] { "review:d", "ask:b", "finished:a", "finished:e", "show:c" }, p.Items.Select(i => i.Id));
        Assert.Equal("2 need you · 5 working · 2 done", p.Headline);
        Assert.Equal("1 of 5", p.Position);
        Assert.Equal("review:d", p.Focused?.Id);
        Assert.Equal(new[] { "ask:b", "finished:a", "finished:e", "show:c" }, p.Others.Select(i => i.Id));
        p.Move(1);
        p.Move(1);
        Assert.Equal("3 of 5", p.Position);
        p.Move(-10);
        Assert.Equal(0, p.Focus);
        p.Move(10);
        Assert.Equal(4, p.Focus);
        p.FocusOn("ask:b");
        Assert.Equal(1, p.Focus);
        p.FocusOn("nope");
        Assert.Equal(1, p.Focus);
    }

    [Fact]
    public void HeadlineVariants()
    {
        Assert.Equal("1 needs you", new PanelModel(new[] { CoreFx.Item("ask:1", AttentionKind.Ask) }, workingCount: 0).Headline);
        Assert.Equal("2 working", new PanelModel(Array.Empty<AttentionItem>(), workingCount: 2).Headline);
        Assert.Equal("Nothing needs you · 1 done", new PanelModel(new[] { CoreFx.Item("finished:1", AttentionKind.Finished) }, workingCount: 0).Headline);
        Assert.Equal("Nothing needs you", new PanelModel(Array.Empty<AttentionItem>(), workingCount: 0).Headline);
        Assert.Equal("", new PanelModel(Array.Empty<AttentionItem>(), workingCount: 0).Position);
    }

    [Fact]
    public void UpdateKeepsFocusByIdElseClamps()
    {
        var p = new PanelModel(items);
        p.FocusOn("finished:e");
        p.Update(items.Where(i => i.Id != "review:d"));
        Assert.Equal("finished:e", p.Focused?.Id);
        p.Update(new[] { items[1] });
        Assert.Equal(0, p.Focus);
        Assert.Equal("ask:b", p.Focused?.Id);
        p.Update(Array.Empty<AttentionItem>());
        Assert.Null(p.Focused);
        Assert.Equal("", p.Position);
    }
}

public sealed class UndoQueueTests
{
    private static readonly DateTimeOffset T0 = CoreFx.T0;

    [Fact]
    public void HoldTickCancel()
    {
        var q = new UndoQueue<string>();
        Assert.Equal(3, q.Delay);
        Assert.Null(q.Tick(T0));
        Assert.Null(q.Hold("a", T0));
        Assert.True(Math.Abs(q.Remaining(T0.AddSeconds(1)) - 2) < 0.001);
        Assert.Null(q.Tick(T0.AddSeconds(2.999)));
        Assert.Equal("a", q.Tick(T0.AddSeconds(3)));
        Assert.Null(q.Tick(T0.AddSeconds(4))); // fires once
        Assert.Equal(0, q.Remaining(T0));
        q.Hold("b", T0);
        Assert.True(q.Cancel());
        Assert.False(q.Cancel());
        Assert.Null(q.Tick(T0.AddSeconds(10)));
    }

    [Fact]
    public void DisplacementAndClamp()
    {
        var q = new UndoQueue<string>(delay: 9);
        Assert.Equal(5, q.Delay);
        q.Hold("first", T0);
        Assert.Equal("first", q.Hold("second", T0.AddSeconds(1))); // a second keystroke commits the first
        Assert.Equal(T0.AddSeconds(6), q.FireAt);
        var zero = new UndoQueue<string>(delay: -1);
        Assert.Equal(0, zero.Delay);
        zero.Hold("now", T0);
        Assert.Equal("now", zero.Tick(T0));
    }
}

public sealed class KeyGrammarTests
{
    [Fact]
    public void Table()
    {
        var cases = new (KeyPress Key, Intent Want)[]
        {
            (new KeyPress("1"), Intent.Answer(1)), (new KeyPress("2"), Intent.Answer(2)), (new KeyPress("3"), Intent.Answer(3)),
            (new KeyPress("4"), Intent.None),
            (new KeyPress("\r", VirtualKeys.Return), Intent.AnswerRecommended), (new KeyPress("\r", VirtualKeys.NumpadEnter), Intent.AnswerRecommended),
            (new KeyPress("\u001b", VirtualKeys.Escape), Intent.Escape),
            (new KeyPress("j"), Intent.Next), (new KeyPress("K"), Intent.Previous),
            (new KeyPress("", VirtualKeys.Down), Intent.Next), (new KeyPress("", VirtualKeys.Up), Intent.Previous),
            (new KeyPress("s"), Intent.Snooze), (new KeyPress("e"), Intent.Clear),
            (new KeyPress("d"), Intent.OpenDiff), (new KeyPress("o"), Intent.OpenBoard), (new KeyPress("o", ctrl: true), Intent.OpenSession),
            (new KeyPress("1", ctrl: true), Intent.None), (new KeyPress("r"), Intent.Reply), (new KeyPress("l", ctrl: true), Intent.AskBar),
        };
        foreach (var (key, want) in cases) Assert.True(want == KeyGrammar.IntentFor(key), $"{key} → {KeyGrammar.IntentFor(key)}, want {want}");
        Assert.Equal(1800, KeyGrammar.SnoozeInterval);
    }

    [Fact]
    public void DeepLinksAndStaleGuard()
    {
        Assert.Equal("theuth://attention/decision:task%207:d", DeepLink.Attention("decision:task 7:d").OriginalString);
        Assert.Equal("theuth://c/abc", DeepLink.Session("abc").OriginalString);
        var latest = new[] { CoreFx.Item("ask:1", AttentionKind.Ask) };
        Assert.True(StaleGuard.Allows("ask:1", latest));
        Assert.False(StaleGuard.Allows("ask:2", latest));
    }
}

public sealed class CardModelTests
{
    [Fact]
    public void AskCapsAtThreeAndPrePicks()
    {
        var q = new Question { Id = "q", Text = "Which?", Options = Enumerable.Range(0, 5).Select(i => new Choice { Label = $"o{i}" }).ToValueList(), Recommended = 2 };
        var it = CoreFx.Item("ask:1", AttentionKind.Ask, agent: "w2", session: "s") with
        {
            Ask = new AskRequested { Id = "ask-id", Session = "s", Questions = ValueList.Of(q) },
        };
        var c = Card.Make(it);
        Assert.Equal("w2", c.Who);
        Assert.Equal("Which?", c.Question);
        Assert.Equal(new[] { 1, 2, 3 }, c.Options.Select(o => o.Key));
        Assert.Equal(2, c.MoreOptions);
        Assert.Equal("o2", c.Recommended?.Label);
        Assert.Equal(new CardAction.AnswerAsk("ask-id", ValueList.Of<Answer>(new Answer.Choice(0))), c.Options[0].Action);
        Assert.Equal("theuth://c/s", c.SessionUrl?.OriginalString);
        // Several questions: no keystroke answers, open in theuth instead.
        it = it with { Ask = new AskRequested { Id = "ask-id", Session = "s", Questions = ValueList.Of(q, q) } };
        var multi = Card.Make(it);
        Assert.Empty(multi.Options);
        Assert.Equal("2 questions — answer them in theuth", multi.Why);
    }

    [Fact]
    public void DecisionUsesPacketAndForwardedRecommendation()
    {
        var it = CoreFx.Item("decision:1", AttentionKind.Decision, agent: "w2", task: "t");
        Assert.Empty(Card.Make(it).Options); // no packet yet
        var p = new Packet
        {
            Key = "decision:1", Task = "t", Kind = PacketKind.Decision, Question = "Layout?", Summary = "Blocks checkout.",
            Options = ValueList.Of(new PacketOption { Label = "A" }, new PacketOption { Label = "B" }, new PacketOption { Label = "C" }, new PacketOption { Label = "D" }),
            Recommended = 0, Recommendation = new Recommendation { Text = "B.", Option = 1 },
        };
        var c = Card.Make(it, packet: p);
        Assert.Equal("Layout?", c.Question);
        Assert.Equal("Blocks checkout.", c.Why);
        Assert.Equal("B", c.Recommended?.Label);
        Assert.Equal(new CardAction.Decide("decision:decision:1", "C"), c.Options[2].Action);
        Assert.Equal(1, c.MoreOptions);
    }

    [Fact]
    public void ApprovalForcingFunction()
    {
        var it = CoreFx.Item("approval:1", AttentionKind.Approval, agent: "w3");
        var mutating = new ApprovalRequested
        {
            Id = "ap-approval:1", Session = "s", Tool = "shell",
            Args = new JsonValue.Object(new ValueDictionary { ["command"] = new JsonValue.String("git push") }),
            Risk = Risk.Mutating,
        };
        var c = Card.Make(it, approval: mutating);
        Assert.Equal(new[] { "Allow", "Deny" }, c.Options.Select(o => o.Label));
        Assert.Equal("Allow", c.Recommended?.Label);
        Assert.Equal("shell · mutating · git push", c.Why);
        Assert.Equal(new CardAction.Approve("ap-approval:1", Decision.Deny), c.Options[1].Action);
        var destructive = mutating with { Risk = Risk.Destructive };
        Assert.Null(Card.Make(it, approval: destructive).Recommended); // P13: no pre-pick
        Assert.Equal("Allow", Card.Make(it).Recommended?.Label); // without the request we still offer Allow/Deny
        Assert.Equal(2, Card.Make(it).Options.Count);
    }

    [Fact]
    public void ReviewChipsAndFailingChecks()
    {
        var it = CoreFx.Item("review:1", AttentionKind.Review, agent: "ops", task: "task-9");
        var good = new Packet { Key = "review:task-9", Task = "task-9", Kind = PacketKind.Review, Summary = "Done.", Checks = ValueList.Of(new Check { Text = "tests", Done = true }) };
        var c = Card.Make(it, packet: good);
        Assert.Equal("Approve", c.Recommended?.Label);
        Assert.Equal(new[] { new Chip("tests", true) }, c.Chips);
        Assert.Equal(new CardAction.ReviewApprove("task-9"), c.Options[0].Action);
        Assert.Equal(new CardAction.ReviewSendBack("task-9"), c.Options[1].Action);
        Assert.True(c.Options[1].Action.NeedsNote);
        var bad = good with { Checks = good.Checks.Append(new Check { Text = "restore", Done = false }).ToValueList() };
        Assert.Null(Card.Make(it, packet: bad).Recommended); // P13: failing checks remove the pre-pick
    }

    [Fact]
    public void FinishedAndOtherKinds()
    {
        var fin = CoreFx.Item("finished:1", AttentionKind.Finished, agent: "w1", session: "s4") with
        {
            Summary = "Landed.",
            NextSteps = ValueList.Of("Open a PR", "Fix the CLI", "third is dropped"),
        };
        var c = Card.Make(fin);
        Assert.Equal("Landed.", c.Why);
        Assert.True(c.Clearable);
        Assert.Equal(new[] { "Open a PR", "Fix the CLI" }, c.Options.Select(o => o.Label));
        Assert.Equal(new CardAction.SendNextStep("s4", "Open a PR"), c.Recommended?.Action);
        var hire = Card.Make(CoreFx.Item("hire:x", AttentionKind.Hire, agent: "x"));
        Assert.Empty(hire.Options);
        Assert.False(hire.Clearable);
        Assert.Equal("theuth://attention/hire:x", hire.AttentionUrl.OriginalString);
        Assert.True(Card.Make(CoreFx.Item("show:1", AttentionKind.Show)).Clearable);
        Assert.Equal("0192c5e0", Card.Make(CoreFx.Item("x", AttentionKind.Other("weather"), session: "0192c5e0-7a1b")).Who);
    }
}
