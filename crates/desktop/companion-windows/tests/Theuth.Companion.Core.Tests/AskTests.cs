namespace Theuth.Companion.Core.Tests;

internal static class AskFx
{
    public static readonly DateTimeOffset T0 = DateTimeOffset.FromUnixTimeSeconds(1_791_448_800);

    public static AgentSummary Summary(string name, string? desk = null) => new()
    {
        File = new AgentFile { Name = name, Title = name, Model = "m" },
        Status = AgentStatus.Idle,
        Desk = desk,
    };

    public static AttentionItem Item(string id, AttentionKind kind, string? session = "s-1", string? task = null,
                                     string? decision = null, string? approval = null) => new()
    {
        Id = id,
        Kind = kind,
        Title = id,
        AtMs = 0,
        Task = task,
        Agent = "w3",
        Session = session,
        Approval = approval,
        Decision = decision,
    };

    public static MeshStatus Mesh(string holder, bool withPeer = true) => new()
    {
        SelfId = "e1",
        SelfName = "mac",
        Peers = withPeer ? ValueList.Of(new MeshStatus.Peer { Id = "e2", Name = "nuc", Reachable = true }) : new(),
        LeaseInfo = new MeshStatus.Lease { Holder = holder },
    };
}

public sealed class AskRouterTests
{
    [Fact]
    public void HubRuleFollowsTheLease()
    {
        var hub = AskFx.Mesh("e1");
        var spoke = AskFx.Mesh("e2");
        var none = new MeshStatus { SelfId = "e1", SelfName = "mac", LeaseInfo = new MeshStatus.Lease { Holder = null } };
        Assert.True(hub.IsHub && hub.HubName == "mac");
        Assert.True(!spoke.IsHub && spoke.HubName == "nuc");
        Assert.True(!none.IsHub && none.HubName is null);
    }

    [Fact]
    public void PlanOpensHereOnTheHubAndRefusesElsewhere()
    {
        var spoke = AskFx.Mesh("e2");
        Assert.Equal(AskPlan.OpenHere.Instance, AskRouter.Plan(null, Array.Empty<AgentSummary>())); // no mesh: a lone engine is its own hub
        Assert.Equal(AskPlan.OpenHere.Instance, AskRouter.Plan(AskFx.Mesh("e1", withPeer: false), Array.Empty<AgentSummary>()));
        // Off the hub a replicated desk id still routes; never another agent's desk.
        Assert.Equal(new AskPlan.Use("desk-9"), AskRouter.Plan(spoke, new[] { AskFx.Summary("theuth", desk: "desk-9") }));
        Assert.Equal(new AskPlan.Refuse("theuth's desk lives on nuc — open theuth there"), AskRouter.Plan(spoke, new[] { AskFx.Summary("w2", desk: "desk-2") }));
    }

    [Fact]
    public void ReplyCollectsOnlyTheDeskUntilItsTurnEnds()
    {
        var r = new AskRouter();
        r.Resolved("desk", hub: true);
        Assert.False(r.Observe(new ReplyEvent.TextDelta("desk", "early"))); // nothing collected before a send
        r.Sent("desk");
        Assert.False(r.Observe(new ReplyEvent.TurnFinished("desk"))); // an earlier turn ending with nothing streamed is not the reply
        Assert.False(r.Observe(new ReplyEvent.TextDelta("other", "noise")));
        Assert.True(r.Observe(new ReplyEvent.TextDelta("desk", "- handed to w3 — it owns crates/rpc\n")));
        Assert.True(r.Observe(new ReplyEvent.TextDelta("desk", "told w2 to wait\nthird line")));
        Assert.Equal("handed to w3 — it owns crates/rpc — told w2 to wait", r.Reply?.FirstTwoLines);
        Assert.True(r.Observe(new ReplyEvent.TurnFinished("desk")));
        Assert.True(r.Reply?.Finished);
        Assert.False(r.Observe(new ReplyEvent.TextDelta("desk", "late"))); // finished replies stay put
        Assert.DoesNotContain("late", r.Reply?.Text);
    }

    [Fact]
    public void ReplyActionsFollowTheKind()
    {
        var ask = new AskRequested { Id = "ask-1", Session = "s-1", Questions = ValueList.Of(new Question { Id = "q", Text = "Which?" }) };
        var askItem = AskFx.Item("ask:1", AttentionKind.Ask) with { Ask = ask };
        Assert.Equal(new CardAction.AnswerAsk("ask-1", ValueList.Of<Answer>(new Answer.Text("neither"))), Card.Make(askItem).ReplyAction("neither"));
        var multi = askItem with
        {
            Ask = new AskRequested { Id = "ask-2", Session = "s-1", Questions = ValueList.Of(new Question { Id = "a", Text = "A" }, new Question { Id = "b", Text = "B" }) },
        };
        Assert.Null(Card.Make(multi).ReplyAction("x")); // multi-question asks are answered in theuth
        Assert.Equal(new CardAction.Decide("decision:7", "ship B"), Card.Make(AskFx.Item("d", AttentionKind.Decision, decision: "decision:7")).ReplyAction("ship B"));
        Assert.Equal(new CardAction.DenyWithNote("ap-1", "s-1", "not that file"), Card.Make(AskFx.Item("a", AttentionKind.Approval, approval: "ap-1")).ReplyAction("not that file"));
        Assert.Equal(new CardAction.ReviewSendBackNote("t-1", "tests?"), Card.Make(AskFx.Item("r", AttentionKind.Review, task: "t-1")).ReplyAction("tests?"));
        Assert.Equal(new CardAction.SendNextStep("s-1", "now docs"), Card.Make(AskFx.Item("f", AttentionKind.Finished)).ReplyAction("now docs"));
        Assert.Null(Card.Make(AskFx.Item("f", AttentionKind.Finished, session: null)).ReplyAction("x"));
        Assert.Null(Card.Make(AskFx.Item("s", AttentionKind.Show)).ReplyAction("x"));
    }

    [Fact]
    public void GrammarGainsReplyAndAskBar()
    {
        Assert.Equal(Intent.Reply, KeyGrammar.IntentFor(new KeyPress("r")));
        Assert.Equal(Intent.Reply, KeyGrammar.IntentFor(new KeyPress("R")));
        Assert.Equal(Intent.AskBar, KeyGrammar.IntentFor(new KeyPress("l", ctrl: true)));
        Assert.Equal(Intent.None, KeyGrammar.IntentFor(new KeyPress("l")));
        Assert.Equal(Intent.None, KeyGrammar.IntentFor(new KeyPress("r", ctrl: true)));
    }

    [Fact]
    public void PanelFocusWalksIntoTheDoneZone()
    {
        var p = new PanelModel(new[] { AskFx.Item("ask:1", AttentionKind.Ask), AskFx.Item("finished:1", AttentionKind.Finished) });
        p.ReceiptCount = 2;
        p.Move(1);
        p.Move(1);
        Assert.True(p.Focused is null && p.ReceiptFocus == 0 && p.Others.Count == 2 && p.Position == "2 of 2");
        p.Move(1);
        p.Move(1);
        Assert.Equal(1, p.ReceiptFocus); // clamped at the last receipt
        p.Move(-2);
        Assert.True(p.Focused?.Id == "finished:1" && p.ReceiptFocus is null);
        p.ReceiptCount = 0;
        p.Move(5);
        Assert.Equal("finished:1", p.Focused?.Id); // no receipts: the old clamp
    }
}

public sealed class AskVoiceTests
{
    private static readonly DateTimeOffset T0 = AskFx.T0;

    private static AskVoiceMachine.Event Down(double s) => new AskVoiceMachine.Event.KeyDown(T0.AddSeconds(s));
    private static AskVoiceMachine.Event Up(double s) => new AskVoiceMachine.Event.KeyUp(T0.AddSeconds(s));
    private static AskVoiceMachine.Event Tick(double s) => new AskVoiceMachine.Event.Tick(T0.AddSeconds(s));

    [Fact]
    public void TapTogglesAndNeverRecords()
    {
        var m = new AskVoiceMachine();
        Assert.Empty(m.Handle(Down(0)));
        Assert.Empty(m.Handle(Tick(0.1)));
        Assert.Equal(new[] { AskVoiceMachine.Effect.Tap }, m.Handle(Up(0.2)));
        Assert.Equal(AskVoiceMachine.State.Idle, m.Current);
    }

    [Fact]
    public void HoldStreamsThenTheDictatedTextLandsInTheField()
    {
        var m = new AskVoiceMachine();
        _ = m.Handle(Down(0));
        Assert.Empty(m.Handle(Tick(0.249)));
        Assert.Equal(new[] { AskVoiceMachine.Effect.StartRecording }, m.Handle(Tick(0.25)));
        Assert.True(m.IsRecording);
        Assert.Empty(m.Handle(Tick(3))); // ticks while recording are quiet until the cap
        Assert.Equal(new[] { AskVoiceMachine.Effect.StopDictation }, m.Handle(Up(4)));
        Assert.Empty(m.Handle(Down(4.1))); // a press while stopping is ignored
        Assert.Equal(new[] { AskVoiceMachine.Effect.SetText("ship it") }, m.Handle(new AskVoiceMachine.Event.Dictated("  ship it \n")));
        Assert.Equal(AskVoiceMachine.State.Idle, m.Current);
    }

    [Fact]
    public void EmptyTranscriptAndErrorsFail()
    {
        var m = new AskVoiceMachine();
        _ = m.Handle(Down(0));
        _ = m.Handle(Tick(1));
        _ = m.Handle(Up(2));
        Assert.Equal(new[] { AskVoiceMachine.Effect.Fail("Nothing recognized") }, m.Handle(new AskVoiceMachine.Event.Dictated("  ")));
        _ = m.Handle(Down(0));
        _ = m.Handle(Tick(1));
        Assert.Equal(new[] { AskVoiceMachine.Effect.Fail("no microphone") }, m.Handle(new AskVoiceMachine.Event.Failed("no microphone")));
        Assert.Equal(AskVoiceMachine.State.Idle, m.Current);
        Assert.Empty(m.Handle(Up(0))); // a key-up with nothing pressed is nothing
    }

    [Fact]
    public void RecordingStopsAtTheCap()
    {
        var m = new AskVoiceMachine();
        _ = m.Handle(Down(0));
        _ = m.Handle(Tick(1));
        Assert.Equal(new[] { AskVoiceMachine.Effect.StopDictation }, m.Handle(Tick(1 + AskVoiceMachine.MaxDuration)));
        Assert.Empty(m.Handle(Up(70))); // the release after an auto-stop does nothing
    }
}
