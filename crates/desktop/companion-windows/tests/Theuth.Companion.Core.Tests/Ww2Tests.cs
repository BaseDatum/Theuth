using Theuth.Companion.Rpc;

namespace Theuth.Companion.Core.Tests;

/// <summary>Fixtures for the 149 WW2 models (mirror the Swift <c>RailPillsModelTests</c> / <c>AgentsModelTests</c> / <c>CardAnatomyTests</c>).</summary>
internal static class Ww2Fx
{
    public static readonly DateTimeOffset T0 = CoreFx.T0;
    public const string Self = "engine-self";
    public const string PeerB = "engine-build-b";

    public static AgentInfo Live(string id, AgentState state, string? agent = null, string name = "", string parent = "", string last = "",
        ushort children = 0, string? project = null, double runningFor = 120) => new()
    {
        Id = id,
        Name = name,
        Parent = parent,
        Depth = (byte)(parent.Length == 0 ? 0 : 1),
        Agent = agent,
        Model = "claude",
        State = state,
        Last = last,
        Project = project,
        StartedAt = T0.AddSeconds(-runningFor - 1),
        Activity = new SessionActivity
        {
            State = state,
            TurnStartedAt = state.Kind == AgentStateKind.Running ? T0.AddSeconds(-runningFor) : null,
            Children = children,
            Live = true,
        },
    };

    public static SessionRow Session(string id, string title, string owner = "", bool running = false, string? agent = null, string? cwd = null,
        double updatedAgo = 60, string? parent = null) =>
        new(id, title, owner, running, agent, cwd, null, parent, null, null, T0.AddSeconds(-updatedAgo));

    public static AgentsPanel Panel(IEnumerable<AgentInfo>? activity = null, IEnumerable<AgentSummary>? agents = null,
        IEnumerable<SessionRow>? sessions = null, IEnumerable<AttentionItem>? items = null, IEnumerable<HostPeer>? peers = null) =>
        AgentsModel.Derive(activity?.ToList() ?? new(), agents?.ToList() ?? new(), sessions?.ToList() ?? new(), items?.ToList() ?? new(),
            Self, "sky-mbp", peers?.ToList() ?? new List<HostPeer> { new(PeerB, "build-B") }, T0, "lab");
}

public sealed class RailPillsModelTests
{
    private static Rail RailOf(params AttentionItem[] items) =>
        RailModel.Derive(items, Array.Empty<AgentSummary>(), new[]
        {
            Ww2Fx.Live("s-run", AgentState.Running),
            Ww2Fx.Live("s-idle", AgentState.Idle),
            Ww2Fx.Live("s-err", AgentState.Error),
        }, Ww2Fx.T0);

    [Fact]
    public void InboxCountIsGoldOnlyWhenSomethingNeedsYou()
    {
        var quiet = RailPillsModel.Derive(RailModel.Derive(Array.Empty<AttentionItem>(), Array.Empty<AgentSummary>(), Array.Empty<AgentInfo>(), Ww2Fx.T0), true, false, null);
        Assert.Equal(0, quiet.InboxCount);
        Assert.False(quiet.InboxGold);
        Assert.True(quiet.Quiet);
        Assert.Empty(quiet.Dots);

        var busy = RailPillsModel.Derive(RailOf(CoreFx.Item("ask:1", AttentionKind.Ask, agent: "w1", session: "s-ask")), true, false, null);
        Assert.Equal(1, busy.InboxCount);
        Assert.True(busy.InboxGold);
        Assert.False(busy.Quiet);
    }

    [Fact]
    public void OneDotPerAgentGoldFirstThenWorkingThenTheRest()
    {
        var p = RailPillsModel.Derive(RailOf(CoreFx.Item("ask:1", AttentionKind.Ask, agent: "w1", session: "s-ask")), true, false, null);
        Assert.Equal(new[] { "ask:1", "session:s-run", "session:s-idle", "session:s-err" }, p.Dots.Select(d => d.Id));
        Assert.True(p.Dots[0].Gold);
        Assert.True(p.Dots[1].Spins);
        Assert.Equal(MarkStateKind.Idle, p.Dots[2].State.Kind);
        Assert.Equal(MarkStateKind.Error, p.Dots[3].State.Kind);
        Assert.All(p.Dots, d => Assert.False(d.Hollow));
    }

    [Fact]
    public void ANeedsYouItemAndItsRunningSessionFoldIntoOneGoldDot()
    {
        // The ask came from s-run: the rail has a needs-you mark AND an agent mark for it; the pill shows one gold dot.
        var p = RailPillsModel.Derive(RailOf(CoreFx.Item("ask:1", AttentionKind.Ask, agent: "w1", session: "s-run")), true, false, null);
        Assert.Equal(new[] { "ask:1", "session:s-idle", "session:s-err" }, p.Dots.Select(d => d.Id));
        Assert.True(p.Dots[0].Gold);
    }

    [Fact]
    public void RemoteSessionsAreHollowAndTheCapLeavesAnOverflow()
    {
        var many = Enumerable.Range(0, 15).Select(i => Ww2Fx.Live($"s-{i}", AgentState.Running)).ToList();
        var rail = RailModel.Derive(Array.Empty<AttentionItem>(), Array.Empty<AgentSummary>(), many, Ww2Fx.T0);
        var p = RailPillsModel.Derive(rail, true, false, null, remoteSessions: new HashSet<string> { "s-3" });
        Assert.Equal(RailPillsModel.MaxDots, p.Dots.Count);
        Assert.Equal(3, p.Overflow);
        Assert.True(p.Dots.Single(d => d.Session == "s-3").Hollow);
        Assert.False(p.Dots[0].Hollow);
    }

    [Fact]
    public void RecordingCarriesTheLevelBarsAndTheHoveredDot()
    {
        var p = RailPillsModel.Derive(RailOf(), false, true, new[] { 0.1f, 0.5f, 0.9f }, hovered: "session:s-run");
        Assert.True(p.Recording);
        Assert.Equal(new[] { 0.1f, 0.5f, 0.9f }, p.Levels);
        Assert.False(p.Connected);
        Assert.Equal("session:s-run", p.Hovered);
        Assert.Equal(1.4, RailPillsModel.DotScale(true));
        Assert.Equal(1, RailPillsModel.DotScale(false));
        Assert.Equal("Listening", RailPillsModel.Describe(RailPillKind.Actions, p));
    }

    [Fact]
    public void GeometryIsD1sFiftyTwoPointCapsulesWithTwelvePointGaps()
    {
        var l = RailPillsModel.Layout(0);
        Assert.Equal(new[] { RailPillKind.Inbox, RailPillKind.Agents, RailPillKind.Actions, RailPillKind.More }, l.Select(s => s.Kind));
        Assert.Equal(0, l[0].Top);
        Assert.Equal(52, l[0].Height);
        Assert.Equal(64, l[1].Top);
        Assert.Equal(52, l[1].Height); // the agents pill keeps its shape with no agents
        Assert.Equal(128, l[2].Top);
        Assert.Equal(192, l[3].Top);
        Assert.Equal(44, l[3].Height);
        Assert.Equal(236, RailPillsModel.StackHeight(0));
        // 4 dots: 14 + 4×12 + 3×12 + 14 = 112
        Assert.Equal(112, RailPillsModel.AgentsPillHeight(4));
        Assert.Equal(112 + 12, RailPillsModel.AgentsPillHeight(4, overflow: 2));
        Assert.Equal(64, RailPillsModel.WindowWidth);
    }
}

public sealed class AgentsModelTests
{
    [Fact]
    public void GroupsByHostLocalFirstThenPeersByName()
    {
        var p = Ww2Fx.Panel(
            activity: new[] { Ww2Fx.Live("s-1", AgentState.Running, agent: "omp", name: "w1-149-parity") },
            sessions: new[]
            {
                Ww2Fx.Session("s-1", "Bring the rail to One's pills"),
                Ww2Fx.Session("s-b", "134-W7 WinUI rail", owner: Ww2Fx.PeerB, running: true, agent: "opencode"),
                Ww2Fx.Session("s-z", "old chat", owner: "engine-zed", updatedAgo: 30),
            },
            peers: new[] { new HostPeer(Ww2Fx.PeerB, "build-B"), new HostPeer("engine-zed", "alpha") });
        Assert.Equal(3, p.Groups.Count);
        Assert.Equal(AgentsModel.LocalTitle, p.Groups[0].Title);
        Assert.True(p.Groups[0].Local);
        Assert.Equal("alpha · on 🌐 lab", p.Groups[1].Title);
        Assert.Equal("build-B · on 🌐 lab", p.Groups[2].Title);
        var remote = p.Groups[2].Rows.Single();
        Assert.Equal(AgentRowState.Remote, remote.State);
        Assert.True(remote.Remote);
        Assert.False(remote.Live);
        Assert.False(remote.CanTalk);
        Assert.False(remote.CanStop);
        Assert.Equal("open it on build-B", remote.DisabledReason);
        Assert.Equal("on build-B", remote.Last);
        Assert.Equal("opencode", remote.Harness);
    }

    [Fact]
    public void LocalRowsReadNameHarnessChildrenAndLast()
    {
        var p = Ww2Fx.Panel(
            activity: new[]
            {
                Ww2Fx.Live("s-root", AgentState.Running, name: "", last: "Reading the design doc\nsecond line", children: 2, project: "theuth"),
                Ww2Fx.Live("s-kid", AgentState.Running, agent: "omp", name: "w3-149-island", parent: "s-root", last: ""),
                Ww2Fx.Live("s-idle", AgentState.Idle, agent: "codex", name: "docs-149 writer", last: ""),
            },
            sessions: new[] { Ww2Fx.Session("s-root", "149 orchestration", cwd: "/Users/sky/Develop/theuth") });
        var rows = p.Groups[0].Rows;
        Assert.Equal(new[] { "149 orchestration", "w3-149-island", "docs-149 writer" }, rows.Select(r => r.Name));
        Assert.Equal(new[] { 0, 1, 0 }, rows.Select(r => r.Depth));
        Assert.Equal("theuth", rows[0].Harness);
        Assert.Equal(2, rows[0].Children);
        Assert.Equal("Reading the design doc", rows[0].Last);
        Assert.Equal("running 2 m", rows[1].Last);
        Assert.Equal(AgentRowState.Idle, rows[2].State);
        Assert.True(rows[0].CanTalk);
        Assert.True(rows[0].CanStop);
        Assert.False(rows[2].CanStop);
        Assert.Equal("nothing is running", rows[2].DisabledReason);
        Assert.Equal("Stopping 149 orchestration and 2 children · Esc undo", rows[0].StopPill);
        Assert.Equal("Stopping w3-149-island · Esc undo", rows[1].StopPill);
        Assert.Equal("theuth", rows[0].Project);
        Assert.Equal(0, p.FirstWorking);
    }

    [Fact]
    public void NeedsYouJoinsBysessionAndOrdersFirst()
    {
        var p = Ww2Fx.Panel(
            activity: new[]
            {
                Ww2Fx.Live("s-a", AgentState.Running, name: "a"),
                Ww2Fx.Live("s-b", AgentState.Idle, name: "b"),
                Ww2Fx.Live("s-c", AgentState.Awaiting("user"), name: "c"),
            },
            items: new[] { CoreFx.Item("ask:b", AttentionKind.Ask, session: "s-b") });
        var rows = p.Groups[0].Rows;
        Assert.Equal(new[] { "b", "c", "a" }, rows.Select(r => r.Name));
        Assert.Equal(AgentRowState.NeedsYou, rows[0].State);
        Assert.Equal("ask:b", rows[0].ItemId);
        Assert.Equal(AgentRowState.NeedsYou, rows[1].State); // awaiting = needs you even without an item
        Assert.True(rows[1].Running); // ✕ still stops an awaiting session
        Assert.Equal(2, p.FirstWorking);
        Assert.Equal("1 needs you · 1 working", AgentsModel.Counts(p).Replace("2 need you", "1 needs you"));
    }

    [Fact]
    public void DesksAreRowsAwakeOrAsleepAndProposedOnesAreNot()
    {
        var p = Ww2Fx.Panel(
            activity: new[] { Ww2Fx.Live("desk-1", AgentState.Running, name: "") },
            agents: new[]
            {
                CoreFx.Desk("theuth", AgentStatus.Working, session: "desk-1"),
                CoreFx.Desk("ops", AgentStatus.Idle),
                CoreFx.Desk("night", AgentStatus.Paused, session: "desk-9"),
                CoreFx.Desk("newbie", AgentStatus.Proposed),
            },
            items: new[] { CoreFx.Item("ask:ops", AttentionKind.Ask, session: "desk-9") });
        var rows = p.Groups[0].Rows;
        Assert.Equal(new[] { "theuth · desk", "night · desk", "ops · desk" }, rows.Select(r => r.Name));
        Assert.Equal("theuth", rows[0].DeskName);
        Assert.Equal(AgentRowState.Working, rows[0].State);
        Assert.Equal("Pausing theuth · Esc undo", rows[0].StopPill);
        Assert.Equal(AgentRowState.NeedsYou, rows[1].State); // the paused desk's session has an ask
        Assert.False(rows[1].CanStop); // asleep: nothing to pause
        Assert.Equal(AgentRowState.Idle, rows[2].State);
        Assert.False(rows[2].CanTalk); // no desk session to talk into
    }

    [Fact]
    public void RemoteRowsAreOnlyRecentOrRunningAndNeverLocal()
    {
        var p = Ww2Fx.Panel(sessions: new[]
        {
            Ww2Fx.Session("s-old", "stale", owner: Ww2Fx.PeerB, updatedAgo: 3600),
            Ww2Fx.Session("s-run", "live there", owner: Ww2Fx.PeerB, running: true, updatedAgo: 3600),
            Ww2Fx.Session("s-new", "fresh", owner: Ww2Fx.PeerB, updatedAgo: 30),
            Ww2Fx.Session("s-mine", "mine", owner: Ww2Fx.Self, running: true),
            Ww2Fx.Session("s-lone", "lone", owner: "", running: true),
        });
        Assert.Empty(p.Groups[0].Rows); // local rows come from list_activity, never from a bare session list
        var g = Assert.Single(p.Groups.Skip(1));
        Assert.Equal(new[] { "live there", "fresh" }, g.Rows.Select(r => r.Name));
        Assert.Equal(1, p.IndexOfSession("s-new"));
        Assert.Equal(-1, p.IndexOfSession("s-old"));
    }

    [Fact]
    public void EmptyPanelAndCopy()
    {
        var p = Ww2Fx.Panel();
        Assert.True(p.IsEmpty);
        Assert.Equal(0, p.FirstWorking);
        Assert.Equal("No agents running.", AgentsModel.EmptyTitle);
        Assert.Equal("Microphone is off for desktop apps — Settings › Privacy › Microphone", AgentsModel.MicDenied);
    }
}

public sealed class CardAnatomyTests
{
    private static readonly DateTimeOffset Now = CoreFx.T0;

    private static AttentionItem AskItem(string session = "s-1", double ago = 120) => new()
    {
        Id = "ask:1", Kind = AttentionKind.Ask, Title = "Which spacing between the rail pills?", AtMs = CoreFx.Ms(ago), Agent = "w1-149-parity", Session = session,
        Ask = new AskRequested
        {
            Id = "a-1", Session = session,
            Questions = ValueList.Of(new Question
            {
                Id = "q1", Text = "Which spacing between the rail pills?", Recommended = 0,
                Options = ValueList.Of(new Choice { Label = "12 pt gaps, 52 pt pills" }, new Choice { Label = "8 pt gaps, 44 pt pills" }, new Choice { Label = "One continuous capsule" }),
            }),
        },
    };

    private static AttentionItem Finished(string session = "s-2") => new()
    {
        Id = "finished:s-2", Kind = AttentionKind.Finished, Title = "Island panel is in", AtMs = CoreFx.Ms(10), Agent = "w3-149-island", Session = session,
        Summary = "Island panel is in. Drag, snap-back and per-display position memory; 14 tests green.",
        NextSteps = ValueList.Of("Looks good — merge it", "Show me the island on a screen without a notch"),
    };

    [Fact]
    public void AskCardShowsSnoozeNotClearAndStopWhileTheAgentWaits()
    {
        var agent = Ww2Fx.Live("s-1", AgentState.Awaiting("user"), agent: "omp", project: "theuth");
        var a = CardAnatomyModel.Make(Card.Make(AskItem()), 2, 3, agent, Ww2Fx.Session("s-1", "Bring the rail to One's pill layout"), null, Now);
        Assert.Equal(new[] { FooterVerb.Stop, FooterVerb.Snooze }, a.Footer);
        Assert.True(a.Snoozes);
        Assert.False(a.Clears);
        Assert.Equal("w1-149-parity", a.Header.Name);
        Assert.Equal("w1-149-parity, card 2 of 3", a.Header.Spoken);
        Assert.True(a.Header.CanPrevious);
        Assert.True(a.Header.CanNext);
        Assert.Equal("omp", a.Status.Harness);
        Assert.Equal("Waiting for your answer · 2 m · theuth", a.Status.Text);
        Assert.Equal("Bring the rail to One's pill layout", a.Bubbles.Request); // the interim join: the session title
        Assert.Null(a.Bubbles.LastReply); // AgentInfo.last is empty
        Assert.Equal("12 pt gaps, 52 pt pills (Recommended)", a.Options[0].Text);
        Assert.Equal("8 pt gaps, 44 pt pills", a.Options[1].Text);
        Assert.Equal("Type your answer…", a.Reply.Placeholder);
        Assert.True(a.Reply.Point);
        Assert.True(a.Reply.Talk);
        Assert.Equal(FooterVerb.Snooze, CardAnatomyModel.VerbFor(a, 's'));
        Assert.Null(CardAnatomyModel.VerbFor(a, 'e'));
        Assert.Equal(FooterVerb.Stop, CardAnatomyModel.VerbFor(a, 'X'));
        Assert.Contains(a.Keys, k => k.Cap == "V" && k.Label == "talk");
        Assert.Contains(a.Keys, k => k.Cap == "P");
        Assert.DoesNotContain(a.Keys, k => k.Cap == "E");
    }

    [Fact]
    public void FinishedCardShowsClearAndNextStepsAndNeverStop()
    {
        var agent = Ww2Fx.Live("s-2", AgentState.Idle, agent: "omp", last: "Island panel is in. Drag, snap-back and per-display position memory.");
        var a = CardAnatomyModel.Make(Card.Make(Finished()), 1, 1, agent, null, null, Now);
        Assert.Equal(new[] { FooterVerb.Clear }, a.Footer);
        Assert.False(a.Stops);
        Assert.True(a.NextStepsHeader);
        Assert.Equal("Finished · just now · omp", a.Status.Text);
        Assert.Equal("Island panel is in. Drag, snap-back and per-display position memory.", a.Bubbles.LastReply);
        Assert.Null(a.Bubbles.Request);
        Assert.Equal("Looks good — merge it (Recommended)", a.Options[0].Text);
        Assert.Equal("Tell w3-149-island what next", a.Reply.Placeholder);
        Assert.False(a.Header.CanPrevious);
        Assert.False(a.Header.CanNext);
        Assert.Contains(a.Keys, k => k.Cap == "E" && k.Label == "clear");
        Assert.DoesNotContain(a.Keys, k => k.Cap == "X");
        Assert.DoesNotContain(a.Keys, k => k.Cap == "S");
    }

    [Fact]
    public void StopFollowsTheKindWhenUnknownAndTheActivityWhenKnown()
    {
        Assert.True(CardAnatomyModel.StopsAgent(AskItem(), null));
        Assert.True(CardAnatomyModel.StopsAgent(CoreFx.Item("approval:1", AttentionKind.Approval, session: "s"), null));
        Assert.True(CardAnatomyModel.StopsAgent(CoreFx.Item("decision:1", AttentionKind.Decision, session: "s"), null));
        Assert.False(CardAnatomyModel.StopsAgent(Finished(), null));
        Assert.False(CardAnatomyModel.StopsAgent(CoreFx.Item("hire:x", AttentionKind.Hire, session: "s"), null));
        Assert.False(CardAnatomyModel.StopsAgent(CoreFx.Item("decision:t", AttentionKind.Decision), null)); // task-born: no session
        var gone = Ww2Fx.Live("s-1", AgentState.Idle) with { Activity = new SessionActivity { State = AgentState.Idle, Live = false } };
        Assert.False(CardAnatomyModel.StopsAgent(AskItem(), gone));
        Assert.False(CardAnatomyModel.StopsAgent(AskItem(), Ww2Fx.Live("s-1", AgentState.Cancelled)));
        Assert.True(CardAnatomyModel.StopsAgent(AskItem(), Ww2Fx.Live("s-1", AgentState.Approval("shell"))));
        Assert.False(CardAnatomyModel.StopsAgent(AskItem(), Ww2Fx.Live("s-1", AgentState.Running), sessionGone: true));
    }

    [Fact]
    public void E6ContextWinsOverTheInterimJoinAndIsClippedAt160()
    {
        var longText = new string('x', 170) + "\nsecond line";
        var ctx = new ItemContextText(longText, "Three layouts are drafted. One question before I build.");
        var a = CardAnatomyModel.Make(Card.Make(AskItem()), 1, 1, Ww2Fx.Live("s-1", AgentState.Awaiting("user"), last: "stale"), Ww2Fx.Session("s-1", "title"), ctx, Now);
        Assert.Equal(160, a.Bubbles.Request!.Length);
        Assert.EndsWith("…", a.Bubbles.Request);
        Assert.Equal("Three layouts are drafted. One question before I build.", a.Bubbles.LastReply);
        // A pair is never split: 159 ASCII + an emoji that would straddle the cap is dropped whole.
        var emoji = new string('a', 159) + "😀b";
        Assert.Equal(new string('a', 159) + "…", CardAnatomyModel.Clip(emoji));
        Assert.Null(CardAnatomyModel.Clip("   "));
    }

    [Fact]
    public void TheRequestBubbleNeverRepeatsTheQuestionAndTaskBornCardsHaveNoBubbles()
    {
        var a = CardAnatomyModel.Make(Card.Make(AskItem()), 1, 1, null, Ww2Fx.Session("s-1", "Which spacing between the rail pills?"), null, Now);
        Assert.Null(a.Bubbles.Request);
        Assert.False(a.Bubbles.Any);
        var d = CardAnatomyModel.Make(Card.Make(CoreFx.Item("decision:t", AttentionKind.Decision)), 1, 1, null, null, new ItemContextText("r", "l"), Now);
        Assert.False(d.Bubbles.Any);
        Assert.Equal(new[] { FooterVerb.Snooze }, d.Footer);
    }

    [Fact]
    public void StatusLineWordsAgeAndProjectFallbacks()
    {
        Assert.Equal("just now", CardAnatomyModel.Age(Now.AddSeconds(-44), Now));
        Assert.Equal("45 s", CardAnatomyModel.Age(Now.AddSeconds(-45), Now));
        Assert.Equal("Waiting for approval", CardAnatomyModel.StateWord(AttentionKind.Approval));
        Assert.Equal("Needs a decision", CardAnatomyModel.StateWord(AttentionKind.Decision));
        Assert.Equal("gizmo", CardAnatomyModel.StateWord(AttentionKind.Other("gizmo")));
        var cwd = CardAnatomyModel.Make(Card.Make(AskItem()), 1, 1, null, Ww2Fx.Session("s-1", "t", agent: "codex", cwd: "C:\\Users\\user\\theuth-149w\\"), null, Now);
        Assert.Equal("codex", cwd.Status.Harness);
        Assert.Equal("Waiting for your answer · 2 m · theuth-149w", cwd.Status.Text);
        var bare = CardAnatomyModel.Make(Card.Make(AskItem()), 1, 1, null, null, null, Now);
        Assert.Equal("Waiting for your answer · 2 m", bare.Status.Text); // theuth itself is not a project
        var gone = CardAnatomyModel.Make(Card.Make(AskItem()), 1, 1, null, null, null, Now, sessionGone: true);
        Assert.Equal("session gone", gone.Status.Text);
        Assert.False(gone.Reply.Talk);
        Assert.False(gone.Reply.Point);
        Assert.False(gone.Stops);
    }

    [Fact]
    public void EmptyStateCopyIsRevisionOnes()
    {
        Assert.Equal("Nothing needs you.", CardAnatomyModel.EmptyTitle);
        Assert.Equal("Suspiciously quiet — questions and finished work land here.", CardAnatomyModel.EmptyBody);
        Assert.Equal(("Stop agent", "X"), CardAnatomyModel.FooterText(FooterVerb.Stop));
        Assert.Equal(("Snooze", "S"), CardAnatomyModel.FooterText(FooterVerb.Snooze));
        Assert.Equal("Stopping w1 and 1 child · Esc undo", CardAnatomyModel.StopPill("w1", 1));
    }
}


/// <summary><see cref="FakeEngine"/> plus the 149 WW2 verbs (cancel · agent_pause), recorded in the same call list.</summary>
internal sealed class Ww2Engine(FakeEngine inner) : IStoreEngine
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

    public Task CancelAsync(string session, CancelScope? scope)
    {
        Inner.Calls.Add($"cancel {session} {(scope == CancelScope.Tree ? "tree" : "root")}");
        return Task.CompletedTask;
    }

    public Task AgentPauseAsync(string name, bool paused)
    {
        Inner.Calls.Add($"agent_pause {name} {(paused ? "true" : "false")}");
        return Task.CompletedTask;
    }
}

/// <summary>149 WW2 D3: ✕ and the row mic go through the Store's hold — Esc before the commit takes it back; a commit is final.</summary>
public sealed class RowVerbHoldTests
{
    private readonly ManualScheduler clock = new();
    private readonly FakeEngine fake = new();
    private readonly StoreModel store;

    public RowVerbHoldTests() => store = new StoreModel(new Ww2Engine(fake), clock, () => clock.Now);

    [Fact]
    public void StopTreeHoldsThenCancelsTheTreeWithoutAnItemReRead()
    {
        store.Hold(new StoreModel.Held("", "w3-149-island", "Stopping w3-149-island and 2 children · Esc undo", new CardAction.StopTree("s-3")));
        var holding = Assert.IsType<StoreModel.ReceiptRow.Holding>(store.Receipt);
        Assert.Equal("w3-149-island", holding.Held.Who);
        clock.Advance(2.9);
        Assert.Empty(fake.Calls);
        clock.Advance(0.2);
        Assert.Contains("cancel s-3 tree", fake.Calls);
        Assert.DoesNotContain(fake.Calls, c => c.StartsWith("attention_list", StringComparison.Ordinal) && fake.Calls.IndexOf(c) < fake.Calls.IndexOf("cancel s-3 tree"));
        var sent = Assert.IsType<StoreModel.ReceiptRow.Sent>(store.Receipt);
        Assert.Equal("w3-149-island · stopped", sent.Text); // a cancel is final: no "undo" after the commit
    }

    [Fact]
    public void EscDuringTheHoldTakesTheStopBack()
    {
        store.Hold(new StoreModel.Held("", "w1", "Stopping w1 · Esc undo", new CardAction.StopTree("s-1")));
        clock.Advance(1);
        store.Perform(Intent.Escape);
        Assert.IsType<StoreModel.ReceiptRow.Undone>(store.Receipt);
        clock.Advance(3);
        Assert.DoesNotContain(fake.Calls, c => c.StartsWith("cancel", StringComparison.Ordinal));
    }

    [Fact]
    public void PauseDeskAndRowTalkCommitTheirOwnCalls()
    {
        store.Hold(new StoreModel.Held("", "theuth", "Pausing theuth · Esc undo", new CardAction.PauseDesk("theuth")));
        clock.Advance(3.1);
        Assert.Contains("agent_pause theuth true", fake.Calls);
        store.Hold(new StoreModel.Held("", "w1", "sent · Esc undo", new CardAction.SendInputTo("s-1", "ship it")));
        clock.Advance(3.1);
        Assert.Contains(fake.Calls, c => c.StartsWith("send_input s-1", StringComparison.Ordinal) && c.Contains("ship it", StringComparison.Ordinal));
        var sent = Assert.IsType<StoreModel.ReceiptRow.Sent>(store.Receipt);
        Assert.Equal("w1 · sent", sent.Text);
    }
}
