using System.Text;
using System.Text.Json;

namespace Theuth.Companion.Core.Tests;

/// <summary>Transcript fixtures in the shape <c>load_session</c> returns (crates/agent/src/lib.rs:3003 HistoryEntry, tagged by role).</summary>
internal static class Tx
{
    public static TranscriptEntry User(string text, long? turn) => new() { Role = "user", Text = text, Turn = turn };

    public static TranscriptEntry Assistant(string text, long? turn) => new() { Role = "assistant", Text = text, Turn = turn };

    public static TranscriptEntry Reasoning(string text, long? turn) => new() { Role = "reasoning", Text = text, Turn = turn };

    public static TranscriptEntry Tool() => new() { Role = "tool" };

    public static TranscriptEntry Error(string text) => new() { Role = "error", Text = text };

    public static SessionTail Tail(bool running, params TranscriptEntry[] rows) => new() { Running = running, History = new ValueList<TranscriptEntry>(rows) };
}

/// <summary>Runs the continuations of a pending await inline on the thread that completes it (the owner-thread behaviour the Store relies on), not on xUnit's own context.</summary>
internal sealed class NoSyncContext : IDisposable
{
    private readonly SynchronizationContext? previous = SynchronizationContext.Current;

    public NoSyncContext() => SynchronizationContext.SetSynchronizationContext(null);

    public void Dispose() => SynchronizationContext.SetSynchronizationContext(previous);
}

/// <summary>189w D2: which transcript entry is the Ask's reply — against history fixtures.</summary>
public sealed class AskCatchUpPickTests
{
    private const string Ask = "kill the system settings window";

    [Fact]
    public void ThePickIsTheLastAssistantEntryAfterTheAsksOwnUserRow()
    {
        var pick = AskCatchUp.Pick(new[] { Tx.User("earlier", 7), Tx.Assistant("old reply", 7), Tx.User(Ask, 41), Tx.Reasoning("hm", 41), Tx.Tool(), Tx.Assistant("✓ System Settings closed.", 41) }, Ask, null);
        Assert.Equal("✓ System Settings closed.", pick.Text);
        Assert.True(pick.UserRowFound);
        Assert.Equal(41, pick.Turn); // the matched row's seq becomes the ask's turn id
        Assert.Equal("text", pick.Via);
    }

    [Fact]
    public void ATurnIdMatchesBeforeAnyText()
    {
        // The user row says something else (or has scrolled out of the window): the assistant rows of turn 41 are the reply.
        var history = new[] { Tx.User("not the ask's words", 41), Tx.Tool(), Tx.Assistant("done", 41), Tx.User(Ask, 55), Tx.Assistant("a later turn", 55) };
        var byTurn = AskCatchUp.Pick(history, Ask, 41);
        Assert.Equal("done", byTurn.Text);
        Assert.Equal("turn", byTurn.Via);
        Assert.Equal("a later turn", AskCatchUp.Pick(history, Ask, null).Text); // by text it is the row with the ask's words
        var scrolledOut = AskCatchUp.Pick(new[] { Tx.Tool(), Tx.Assistant("done", 41) }, Ask, 41);
        Assert.Equal("done", scrolledOut.Text);
        Assert.False(scrolledOut.UserRowFound);
    }

    [Fact]
    public void TheLatestUserRowWithTheTextWins()
    {
        var history = new[] { Tx.User("ok", 3), Tx.Assistant("first answer", 3), Tx.User("ok", 9), Tx.Assistant("second answer", 9) };
        Assert.Equal("second answer", AskCatchUp.Pick(history, "ok", null).Text);
    }

    [Fact]
    public void ALaterTurnsReplyIsNeverTheAsksReply()
    {
        var history = new[] { Tx.User("ship", 5), Tx.Assistant("B", 5), Tx.User("something typed in the main window", 9), Tx.Assistant("C", 9) };
        Assert.Equal("B", AskCatchUp.Pick(history, "ship", null).Text);
        Assert.Equal("B", AskCatchUp.Pick(history, "ship", 5).Text);
    }

    [Fact]
    public void TheLastRoundOfAMultiRoundTurnIsTheReply()
    {
        var history = new[] { Tx.User(Ask, 10), Tx.Assistant("Let me check.", 10), Tx.Tool(), Tx.Assistant("✓ Closed.", 10) };
        Assert.Equal("✓ Closed.", AskCatchUp.Pick(history, Ask, null).Text);
    }

    [Fact]
    public void ASteerRowWithoutASeqIsMatchedByTextAndReadsToTheNextTurn()
    {
        // The ask steered into a running turn: its own row has no seq; the reply is the turn's last assistant entry, up to the next turn.
        var history = new[] { Tx.User("long task", 3), Tx.Assistant("started", 3), Tx.User(Ask, null), Tx.Tool(), Tx.Assistant("done with both", 3), Tx.User("next", 8), Tx.Assistant("unrelated", 8) };
        var pick = AskCatchUp.Pick(history, Ask, null);
        Assert.Equal("done with both", pick.Text);
        Assert.Null(pick.Turn);
    }

    [Fact]
    public void TheNewestAssistantEntryIsTheLastResortWhenNothingIdentifiesTheAsk()
    {
        var pick = AskCatchUp.Pick(new[] { Tx.Tool(), Tx.Assistant("older", 4), Tx.Tool(), Tx.Assistant("newest", 4) }, Ask, null);
        Assert.Equal("newest", pick.Text);
        Assert.Equal("newest", pick.Via);
        Assert.False(pick.UserRowFound);
        // A turn that IS known never falls back to another turn's text.
        Assert.Null(AskCatchUp.Pick(new[] { Tx.User("x", 9), Tx.Assistant("another turn", 9) }, Ask, 41).Text);
    }

    [Fact]
    public void BlankAssistantTextIsNoText()
    {
        var pick = AskCatchUp.Pick(new[] { Tx.User(Ask, 2), Tx.Assistant("  \n ", 2), Tx.Tool() }, Ask, null);
        Assert.Null(pick.Text);
        Assert.True(pick.UserRowFound);
    }

    [Fact]
    public void AnErrorRowAfterTheUserRowIsReportedWhenThereIsNoText()
    {
        var pick = AskCatchUp.Pick(new[] { Tx.User(Ask, 2), Tx.Error("model: Resolver error"), }, Ask, null);
        Assert.Null(pick.Text);
        Assert.Equal("model: Resolver error", pick.Error);
        var withText = AskCatchUp.Pick(new[] { Tx.User(Ask, 2), Tx.Assistant("partial", 2), Tx.Error("then it failed") }, Ask, null);
        Assert.Equal("partial", withText.Text);
        Assert.Null(withText.Error);
    }

    [Fact]
    public void WhitespaceInTheAskTextDoesNotMatter()
    {
        var pick = AskCatchUp.Pick(new[] { Tx.User("ship  the\ndocs ", 6), Tx.Assistant("ok", 6) }, "ship the docs", null);
        Assert.Equal("ok", pick.Text);
        Assert.Equal("ship the docs", AskCatchUp.Normalize("  ship \t the\r\n docs  "));
        Assert.Equal("", AskCatchUp.Normalize(null));
    }
}

/// <summary>189w D2: what a catch-up means — running, finished, unreadable, not yet attributable.</summary>
public sealed class AskCatchUpDecideTests
{
    private const string Ask = "ship the docs";
    private static readonly SessionTail Replied = Tx.Tail(false, Tx.User(Ask, 5), Tx.Assistant("Shipped.", 5));

    [Fact]
    public void ARunningTurnKeepsCollectingWhateverTheTranscriptHolds()
    {
        var d = AskCatchUp.Decide(CatchUpTrigger.TurnFinished, Tx.Tail(true, Tx.User(Ask, 5), Tx.Assistant("Shipped.", 5)), Ask, null, "", settled: false);
        Assert.Equal(CatchUpAction.KeepCollecting, d.Action);
        Assert.True(d.Running);
        Assert.Equal(5, d.Turn); // the turn id is learned while it runs
    }

    [Fact]
    public void AFinishedTurnsTranscriptTextReplacesWhatStreamed()
    {
        var d = AskCatchUp.Decide(CatchUpTrigger.TurnFinished, Replied, Ask, null, "Shipp", settled: false);
        Assert.Equal((CatchUpAction.Finish, "Shipped."), (d.Action, d.Text));
    }

    [Fact]
    public void AFinishedTurnWithNoAssistantTextKeepsTheStreamedTextOrEndsWithoutAReply()
    {
        var noText = Tx.Tail(false, Tx.User(Ask, 5), Tx.Tool());
        var streamed = AskCatchUp.Decide(CatchUpTrigger.TurnFinished, noText, Ask, null, "what streamed", settled: false);
        Assert.Equal((CatchUpAction.Finish, "what streamed"), (streamed.Action, streamed.Text));
        var none = AskCatchUp.Decide(CatchUpTrigger.TurnFinished, noText, Ask, null, "", settled: false);
        Assert.Equal(CatchUpAction.Fail, none.Action);
        Assert.True(none.NoReply);
        Assert.Equal("theuth finished without a reply", AskCatchUp.NoReplyText);
    }

    [Fact]
    public void AnErrorRowEndsTheAskAsFailed()
    {
        var d = AskCatchUp.Decide(CatchUpTrigger.Watchdog, Tx.Tail(false, Tx.User(Ask, 5), Tx.Error("model: boom")), Ask, null, "", settled: false);
        Assert.Equal((CatchUpAction.Fail, "model: boom", false), (d.Action, d.Error, d.NoReply));
    }

    [Fact]
    public void AnUnreadableTranscriptNeverLeavesAFinishedTurnWaiting()
    {
        Assert.Equal((CatchUpAction.Finish, "partial"), Pair(AskCatchUp.Decide(CatchUpTrigger.TurnFinished, null, Ask, null, " partial ", settled: false)));
        var none = AskCatchUp.Decide(CatchUpTrigger.TurnFinished, null, Ask, null, "", settled: false);
        Assert.Equal(CatchUpAction.Fail, none.Action);
        Assert.True(none.NoReply);
        // The other triggers only retry at the next one.
        Assert.Equal(CatchUpAction.KeepCollecting, AskCatchUp.Decide(CatchUpTrigger.Watchdog, null, Ask, null, "", settled: false).Action);
        Assert.Equal(CatchUpAction.KeepCollecting, AskCatchUp.Decide(CatchUpTrigger.Reconnect, null, Ask, null, "x", settled: false).Action);
    }

    [Fact]
    public void ATranscriptThatHasNotCaughtUpWithTheAskIsLookedAtOnceMore()
    {
        var elsewhere = Tx.Tail(false, Tx.User("something else", 2), Tx.Assistant("not ours", 2));
        // User row missing, nothing streamed, no turn id: the ask may not have started its turn yet — but the newest entry is found ("newest").
        var empty = Tx.Tail(false, Tx.Tool());
        Assert.Equal(CatchUpAction.Settle, AskCatchUp.Decide(CatchUpTrigger.TurnFinished, empty, Ask, null, "", settled: false).Action);
        Assert.Equal(CatchUpAction.Fail, AskCatchUp.Decide(CatchUpTrigger.TurnFinished, empty, Ask, null, "", settled: true).Action); // the re-check is the last word
        Assert.Equal(CatchUpAction.Finish, AskCatchUp.Decide(CatchUpTrigger.TurnFinished, elsewhere, Ask, null, "", settled: false).Action); // "newest" text exists
        // Never when something streamed or the turn id is known: those say the turn started.
        Assert.Equal(CatchUpAction.Finish, AskCatchUp.Decide(CatchUpTrigger.TurnFinished, empty, Ask, null, "text", settled: false).Action);
        Assert.Equal(CatchUpAction.Fail, AskCatchUp.Decide(CatchUpTrigger.TurnFinished, empty, Ask, 5, "", settled: false).Action);
    }

    [Fact]
    public void ALingeringReplyIsTwelveSecondsPlusReadingTime()
    {
        Assert.Equal(12, AskCatchUp.LingerSeconds("done"));
        Assert.Equal(12, AskCatchUp.LingerSeconds(null));
        Assert.Equal(12, AskCatchUp.LingerSeconds(string.Join(" ", Enumerable.Repeat("word", 20))));
        Assert.Equal(12 + 80 / 4.0, AskCatchUp.LingerSeconds(string.Join(" ", Enumerable.Repeat("word", 100))));
        Assert.Equal(60, AskCatchUp.LingerSeconds(string.Join(" ", Enumerable.Repeat("word", 5000))));
    }

    private static (CatchUpAction, string?) Pair(CatchUpDecision d) => (d.Action, d.Text);
}

/// <summary>189w D3: the terminal stop reasons, and what the receipt says for each.</summary>
public sealed class TerminalStopReasonTests
{
    [Fact]
    public void ErrorCancelledSuspendedTimedOutAndInterruptedAreTerminalAndEverythingElseIsANormalFinish()
    {
        foreach (var reason in new[] { "error", "cancelled", "suspended", "timed_out", "interrupted" }) Assert.True(AskRouter.IsFailure(reason), reason);
        foreach (var reason in new[] { "end_turn", "stop", "max_tokens", "tool_use", "length", "", "something_new" }) Assert.False(AskRouter.IsFailure(reason), reason);
        Assert.False(AskRouter.IsFailure(null));
    }

    [Fact]
    public void EachTerminalReasonReadsAsAFailureWithAReadableReason()
    {
        var texts = new Dictionary<string, string>
        {
            ["error"] = "theuth couldn't reply — the desk's turn ended with an error",
            ["cancelled"] = "theuth couldn't reply — the turn was cancelled",
            ["suspended"] = "theuth couldn't reply — the turn was suspended — theuth is restarting",
            ["timed_out"] = "theuth couldn't reply — the turn was timed out",
            ["interrupted"] = "theuth couldn't reply — the turn was interrupted",
        };
        foreach (var (reason, line) in texts)
        {
            var r = new AskRouter();
            r.Resolved("desk", hub: true);
            r.Sent("desk", "q");
            Assert.True(r.Observe(new ReplyEvent.TurnFinished("desk", reason)), reason);
            Assert.True(r.Reply!.Finished && r.Reply.Failed, reason);
            Assert.Equal(line, r.Reply.FirstTwoLines);
        }
    }

    [Fact]
    public void ASuspendedFinishEndsTheStoreReceiptAtOnceWithoutAnyCatchUp()
    {
        var clock = new ManualScheduler();
        var engine = new FakeEngine();
        var logs = new List<string>();
        var store = new StoreModel(engine, clock, () => clock.Now, logs.Add);
        store.SubmitAsk("ship it");
        clock.Advance(3);
        store.HandleEvent(Encoding.UTF8.GetBytes("""{"type":"TurnFinished","session":"desk-1","turn":"t","stop_reason":"suspended","usage":{},"context":1}"""));
        var row = Assert.IsType<StoreModel.ReceiptRow.Reply>(store.Receipt);
        Assert.Equal("theuth couldn't reply — the turn was suspended — theuth is restarting", row.Text);
        Assert.Equal("", row.Markdown);
        Assert.False(row.Streaming);
        Assert.DoesNotContain(engine.Calls, c => c.StartsWith("load_session", StringComparison.Ordinal));
        Assert.Contains(logs, l => l.StartsWith("ask failed on the desk: stop_reason=suspended", StringComparison.Ordinal));
    }
}

/// <summary>189w D2: the watchdog's decision with an injected clock.</summary>
public sealed class AskWatchdogTests
{
    private static readonly DateTimeOffset T0 = new(2026, 11, 11, 14, 0, 0, TimeSpan.Zero);

    [Fact]
    public void ItCatchesUpAfterFifteenSecondsOfSilenceAndWaitsOutTheRestBefore()
    {
        var w = new AskWatchdog();
        Assert.False(w.Decide(T0).Armed); // nothing collecting, nothing to watch
        w.Arm(T0);
        var early = w.Decide(T0.AddSeconds(14));
        Assert.Equal((false, TimeSpan.FromSeconds(1), true), (early.CatchUp, early.Wait, early.Armed));
        Assert.True(w.Decide(T0.AddSeconds(15)).CatchUp);
        Assert.True(w.Decide(T0.AddSeconds(99)).CatchUp);
    }

    [Fact]
    public void AnyEventForTheDeskRestartsTheSilence()
    {
        var w = new AskWatchdog();
        w.Arm(T0);
        w.Touch(T0.AddSeconds(10));
        Assert.False(w.Decide(T0.AddSeconds(24)).CatchUp);
        Assert.Equal(TimeSpan.FromSeconds(1), w.Decide(T0.AddSeconds(24)).Wait);
        Assert.True(w.Decide(T0.AddSeconds(25)).CatchUp);
    }

    [Fact]
    public void ItIsReArmedAfterEachCatchUpForAsLongAsTheTurnRuns()
    {
        var w = new AskWatchdog();
        w.Arm(T0);
        Assert.True(w.Decide(T0.AddSeconds(15)).CatchUp);
        w.CaughtUp(T0.AddSeconds(15));
        Assert.False(w.Decide(T0.AddSeconds(29)).CatchUp);
        Assert.True(w.Decide(T0.AddSeconds(30)).CatchUp);
        w.Disarm();
        Assert.False(w.Decide(T0.AddSeconds(500)).Armed);
        w.Touch(T0.AddSeconds(501)); // a stray event after the ask ended arms nothing
        Assert.False(w.Armed);
    }
}

/// <summary>189w D1: the event-focus coordinator — the union of claims, serialized, latest wins, resent when asked.</summary>
public sealed class EventFocusTests
{
    private readonly List<string> sent = new();
    private readonly List<string> logs = new();

    private EventFocus Focus(Func<IReadOnlyList<string>, Task>? send = null) => new(send ?? (s => { sent.Add(string.Join(",", s)); return Task.CompletedTask; }), logs.Add);

    [Fact]
    public async Task TheSetThatGoesOutIsTheSortedUnionOfEveryClaim()
    {
        var focus = Focus();
        focus.Claim("point", new[] { "desk-2" });
        await focus.ClaimAsync("ask", new[] { "desk-1", "desk-2" }, "ask");
        Assert.Equal(new[] { "desk-1,desk-2" }, sent);
        Assert.Equal(new[] { "desk-1", "desk-2" }, focus.LastSent);
        Assert.Equal(new[] { "desk-1", "desk-2" }, focus.ClaimOf("ask"));
        Assert.Contains(logs, l => l == "set_event_focus([desk-1,desk-2]) ok (ask)");
    }

    [Fact]
    public async Task ASecondClaimantNeverErasesTheFirst()
    {
        // The macOS incident: one writer sent its own set and the other's desk fell out of it.
        var focus = Focus();
        await focus.ClaimAsync("ask", new[] { "desk-1" }, "ask");
        await focus.ClaimAsync("point", new[] { "desk-9" }, "point");
        Assert.Equal(new[] { "desk-1", "desk-1,desk-9" }, sent);
        Assert.True(focus.Release("point"));
        Assert.False(focus.Release("point"));
        Assert.Equal(new[] { "desk-1" }, focus.Union);
    }

    [Fact]
    public async Task AReleaseNeverSendsAnEmptySetBecauseThatDropsEveryDelta()
    {
        var focus = Focus();
        await focus.ClaimAsync("ask", new[] { "desk-1" }, "ask");
        focus.Release("ask");
        await focus.SendAsync("pending-prompts");
        Assert.Single(sent);
        Assert.Contains(logs, l => l.StartsWith("set_event_focus skipped (pending-prompts): no claims", StringComparison.Ordinal));
    }

    [Fact]
    public async Task EverySendIsForcedEvenWhenTheUnionIsUnchanged()
    {
        var focus = Focus();
        await focus.ClaimAsync("ask", new[] { "desk-1" }, "ask");
        await focus.SendAsync("ask");
        await focus.SendAsync("reconnect");
        Assert.Equal(new[] { "desk-1", "desk-1", "desk-1" }, sent);
        Assert.Equal(3, focus.Sends);
    }

    [Fact]
    public async Task SendsAreSerializedAndTheLatestClaimWins()
    {
        var gates = new List<TaskCompletionSource>();
        var inFlight = 0;
        var maxInFlight = 0;
        var focus = Focus(async s =>
        {
            inFlight++;
            maxInFlight = Math.Max(maxInFlight, inFlight);
            sent.Add(string.Join(",", s));
            var gate = new TaskCompletionSource();
            gates.Add(gate);
            await gate.Task;
            inFlight--;
        });
        var first = focus.ClaimAsync("ask", new[] { "desk-1" }, "a");
        // While the first send is out the claims change twice and two more sends are requested: they coalesce into ONE, with the latest union.
        focus.Claim("ask", new[] { "desk-2" });
        var second = focus.SendAsync("b");
        focus.Claim("point", new[] { "desk-3" });
        var third = focus.SendAsync("c");
        Assert.Single(gates);
        Assert.False(first.IsCompleted);
        gates[0].SetResult();
        await first;
        Assert.Equal(2, gates.Count);
        Assert.False(second.IsCompleted);
        gates[1].SetResult();
        await second;
        await third;
        Assert.Equal(new[] { "desk-1", "desk-2,desk-3" }, sent);
        Assert.Equal(1, maxInFlight);
    }

    [Fact]
    public async Task ARefusedSendIsLoggedNeverThrown()
    {
        var focus = Focus(_ => Task.FromException(new RpcException(-32601, "Method not found")));
        await focus.ClaimAsync("ask", new[] { "desk-1" }, "ask");
        Assert.Null(focus.LastSent);
        Assert.Contains(logs, l => l.StartsWith("set_event_focus failed (ask): rpc -32601: Method not found", StringComparison.Ordinal));
    }

    [Fact]
    public async Task EnsureSinceJoinsARequestMadeAfterTheMarkAndOtherwiseMakesOne()
    {
        var focus = Focus();
        focus.Claim("ask", new[] { "desk-1" });
        long mark = focus.Requested;
        _ = focus.SendAsync("resolve"); // the desk's first resolve, during this ask
        await focus.EnsureSinceAsync(mark, "ask");
        Assert.Equal(1, focus.Sends); // one send for the whole ask
        mark = focus.Requested;
        await focus.EnsureSinceAsync(mark, "ask"); // a later ask on a cached desk: nothing was requested, so this one sends
        Assert.Equal(2, focus.Sends);
    }
}

/// <summary>189w: the Store's Ask path end to end on virtual time with a recording engine.</summary>
public sealed class AskStream189Tests
{
    private readonly ManualScheduler clock = new();
    private readonly FakeEngine engine = new();
    private readonly List<string> logs = new();
    private readonly StoreModel store;
    private const string Q = "kill the system settings window";

    public AskStream189Tests() => store = new StoreModel(engine, clock, () => clock.Now, logs.Add);

    private static byte[] J(string s) => Encoding.UTF8.GetBytes(s);

    private void Send(string text = Q)
    {
        store.SubmitAsk(text);
        clock.Advance(3);
    }

    private void Delta(string text) => store.HandleEvent(J($$"""{"type":"TextDelta","session":"desk-1","turn":"t","text":{{JsonSerializer.Serialize(text)}}}"""));

    private void Finished(string stop = "end_turn") => store.HandleEvent(J($$"""{"type":"TurnFinished","session":"desk-1","turn":"t","stop_reason":"{{stop}}","usage":{},"context":1}"""));

    private StoreModel.ReceiptRow.Reply Row => Assert.IsType<StoreModel.ReceiptRow.Reply>(store.Receipt);

    private IEnumerable<string> Calls(string prefix) => engine.Calls.Where(c => c.StartsWith(prefix, StringComparison.Ordinal));

    // MARK: D1 — the focus

    [Fact]
    public void TheFirstAskSendsTheFocusExactlyOnceBeforeItsInput()
    {
        Send();
        var calls = engine.Calls.Where(c => c.StartsWith("set_event_focus", StringComparison.Ordinal) || c.StartsWith("send_input", StringComparison.Ordinal)).ToList();
        Assert.Equal(new[] { "set_event_focus desk-1", $"send_input desk-1 {Q}" }, calls);
    }

    [Fact]
    public void EveryLaterAskSendsTheFocusAgainBeforeItsInput()
    {
        Send();
        Finished();
        engine.Calls.Clear();
        Send("and the finder window");
        var calls = engine.Calls.Where(c => c.StartsWith("set_event_focus", StringComparison.Ordinal) || c.StartsWith("send_input", StringComparison.Ordinal)).ToList();
        Assert.Equal(new[] { "set_event_focus desk-1", "send_input desk-1 and the finder window" }, calls); // not only on the first ResolveDesk
        Assert.DoesNotContain(engine.Calls, c => c == "desk_open theuth");
    }

    [Fact]
    public void TheFocusIsResentOnEveryPendingPromptsAndEveryReconnect()
    {
        Send();
        engine.Calls.Clear();
        store.HandleEvent(J("""{"type":"PendingPrompts","approvals":[],"asks":[],"secrets":[]}"""));
        Assert.Single(Calls("set_event_focus desk-1"));
        engine.Calls.Clear();
        store.HandleState(RpcClient.State.Open); // the first open: not a reconnect
        Assert.Empty(Calls("set_event_focus"));
        store.HandleState(RpcClient.State.Reconnecting);
        store.HandleState(RpcClient.State.Open);
        Assert.Single(Calls("set_event_focus desk-1"));
        Assert.Contains(logs, l => l == "connection: re-opened (#2)");
    }

    [Fact]
    public async Task TheVoiceKeysDeskJoinsTheUnionInsteadOfBeingDropped()
    {
        Send();
        engine.Calls.Clear();
        await store.ClaimDeltasAsync("point", "desk-9", "point-send");
        Assert.Equal(new[] { "set_event_focus desk-1,desk-9" }, Calls("set_event_focus"));
        engine.Calls.Clear();
        Send("another ask");
        Assert.Equal(new[] { "set_event_focus desk-1,desk-9" }, Calls("set_event_focus")); // the ask's resend keeps the voice key's claim
    }

    // MARK: D2 — catching up from the transcript

    [Fact]
    public void AFinishThatArrivesWithNoStreamedTextIsFilledFromTheTranscript()
    {
        // The macOS incident: the desk answered, the companion never saw a TextDelta, a normal TurnFinished was ignored forever.
        engine.LoadSession = (_, _) => Task.FromResult(Tx.Tail(false, Tx.User(Q, 41), Tx.Tool(), Tx.Assistant("✓ System Settings closed — it's no longer running.", 41)));
        Send();
        Assert.False(store.Ask.Router.Reply!.Finished);
        Finished();
        Assert.True(store.Ask.Router.Reply!.Finished);
        Assert.Equal(new[] { "load_session desk-1 8" }, Calls("load_session"));
        Assert.Equal(("theuth", "✓ System Settings closed — it's no longer running.", "desk-1", "✓ System Settings closed — it's no longer running.", false), (Row.Who, Row.Text, Row.Session, Row.Markdown, Row.Streaming));
    }

    [Fact]
    public void TheTranscriptTextReplacesTheStreamedTextOnFinish()
    {
        engine.LoadSession = (_, _) => Task.FromResult(Tx.Tail(false, Tx.User(Q, 41), Tx.Assistant("Closed it.\n\n- System Settings: gone", 41)));
        Send();
        Delta("Clos"); // a stream that lost the rest
        Assert.Equal("Clos", Row.Markdown);
        Assert.True(Row.Streaming);
        Finished();
        Assert.Equal("Closed it.\n\n- System Settings: gone", Row.Markdown);
        Assert.False(Row.Streaming);
        Assert.Equal("Closed it. — System Settings: gone", Row.Text);
        Assert.Equal("Closed it.\n\n- System Settings: gone", store.Ask.Router.Reply!.Text);
    }

    [Fact]
    public void ARunningTranscriptKeepsTheAskCollectingUntilTheNextFinish()
    {
        var running = true;
        engine.LoadSession = (_, _) => Task.FromResult(Tx.Tail(running, Tx.User(Q, 41), Tx.Assistant("Done.", 41)));
        Send();
        Delta("Do");
        Finished(); // a stale finish: the transcript says a turn is in flight
        Assert.False(store.Ask.Router.Reply!.Finished);
        Assert.True(Row.Streaming);
        running = false;
        Finished();
        Assert.True(store.Ask.Router.Reply!.Finished);
        Assert.Equal("Done.", Row.Markdown);
    }

    [Fact]
    public void ATurnThatLeftNoAssistantTextEndsWithoutAReplyAndKeepsOpen()
    {
        engine.LoadSession = (_, _) => Task.FromResult(Tx.Tail(false, Tx.User(Q, 41), Tx.Tool(), Tx.Tool()));
        Send();
        Finished();
        Assert.True(store.Ask.Router.Reply!.Finished);
        Assert.Equal(("theuth finished without a reply", "desk-1", "", false), (Row.Text, Row.Session, Row.Markdown, Row.Streaming)); // Session stays: Open Ctrl+O
        Assert.Contains(logs, l => l.StartsWith("ask finished (turn-finished · empty): 0 chars, 0 streamed, no reply", StringComparison.Ordinal));
        clock.Advance(12);
        Assert.Null(store.Receipt);
    }

    [Fact]
    public void ATranscriptThatCannotBeReadFallsBackToWhatStreamed()
    {
        Send(); // the fake engine has no load_session
        Delta("- on it");
        Finished();
        Assert.True(store.Ask.Router.Reply!.Finished);
        Assert.Equal("- on it", Row.Markdown);
        Assert.Contains(logs, l => l.Contains("transcript unread (load_session)", StringComparison.Ordinal));
    }

    [Fact]
    public void ALoadThatNeverAnswersCountsAsUnreadAfterTenSecondsAndTheAskEnds()
    {
        using var inline = new NoSyncContext(); // a continuation after a REAL pending await must run inline, as on the app's owner thread
        engine.LoadSession = (_, _) => new TaskCompletionSource<SessionTail>().Task;
        Send();
        Finished();
        Assert.False(store.Ask.Router.Reply!.Finished); // waiting on the call, for at most the deadline
        clock.Advance(9.5);
        Assert.False(store.Ask.Router.Reply!.Finished);
        clock.Advance(1);
        Assert.True(store.Ask.Router.Reply!.Finished);
        Assert.Equal("theuth finished without a reply", Row.Text);
        Assert.Contains(logs, l => l.Contains("did not answer within 10 s", StringComparison.Ordinal));
    }

    [Fact]
    public void AnAskWhoseTranscriptHasNoTraceOfItIsLookedAtOnceMoreAfterASecondAndAHalf()
    {
        var calls = 0;
        engine.LoadSession = (_, _) =>
        {
            calls++;
            return Task.FromResult(calls == 1 ? Tx.Tail(false, Tx.Tool()) : Tx.Tail(false, Tx.User(Q, 12), Tx.Assistant("Now it shows.", 12)));
        };
        Send();
        Finished();
        Assert.False(store.Ask.Router.Reply!.Finished); // not attributable yet
        clock.Advance(1.4);
        Assert.False(store.Ask.Router.Reply!.Finished);
        clock.Advance(0.2);
        Assert.True(store.Ask.Router.Reply!.Finished);
        Assert.Equal("Now it shows.", Row.Markdown);
        Assert.Equal(2, calls);
        Assert.Contains(logs, l => l.StartsWith("ask catch-up (turn-finished, re-check)", StringComparison.Ordinal));
    }

    [Fact]
    public void ALaggedEventCatchesUpWhileAnAskCollectsAndNeverOtherwise()
    {
        engine.LoadSession = (_, _) => Task.FromResult(Tx.Tail(false, Tx.User(Q, 41), Tx.Assistant("Reply found after the gap.", 41)));
        store.HandleEvent(J("""{"type":"Lagged","count":12}"""));
        Assert.Empty(Calls("load_session")); // nothing collecting
        Send();
        store.HandleEvent(J("""{"type":"Lagged","count":12}"""));
        Assert.Equal(new[] { "load_session desk-1 8" }, Calls("load_session"));
        Assert.Equal("Reply found after the gap.", Row.Markdown);
        Assert.True(store.Ask.Router.Reply!.Finished);
    }

    [Fact]
    public void AReconnectCatchesUpWhileAnAskCollects()
    {
        engine.LoadSession = (_, _) => Task.FromResult(Tx.Tail(false, Tx.User(Q, 41), Tx.Assistant("Answered while we were offline.", 41)));
        store.HandleState(RpcClient.State.Open);
        Send();
        store.HandleState(RpcClient.State.Reconnecting);
        store.HandleState(RpcClient.State.Open);
        Assert.Equal("Answered while we were offline.", Row.Markdown);
        Assert.Contains(logs, l => l.StartsWith("ask catch-up (reconnect):", StringComparison.Ordinal));
    }

    [Fact]
    public void FifteenSecondsOfSilenceCatchesUpAndTheWatchdogIsReArmedWhileTheTurnRuns()
    {
        var running = true;
        engine.LoadSession = (_, _) => Task.FromResult(Tx.Tail(running, Tx.User(Q, 41), Tx.Assistant("Finally.", 41)));
        Send();
        clock.Advance(14.9);
        Assert.Empty(Calls("load_session"));
        clock.Advance(0.2);
        Assert.Single(Calls("load_session"));
        Assert.False(store.Ask.Router.Reply!.Finished); // running: keep collecting
        clock.Advance(15);
        Assert.Equal(2, Calls("load_session").Count()); // re-armed
        running = false;
        clock.Advance(15);
        Assert.True(store.Ask.Router.Reply!.Finished);
        Assert.Equal("Finally.", Row.Markdown);
        clock.Advance(60);
        Assert.Equal(3, Calls("load_session").Count()); // finished: the watchdog is gone
        Assert.Equal(3, logs.Count(l => l.StartsWith("ask watchdog: no event for the desk in 15 s", StringComparison.Ordinal)));
    }

    [Fact]
    public void AnEventForTheDeskKeepsTheWatchdogQuiet()
    {
        engine.LoadSession = (_, _) => Task.FromResult(Tx.Tail(true));
        Send();
        for (int i = 0; i < 6; i++)
        {
            clock.Advance(10);
            store.HandleEvent(J("""{"type":"ToolCallStarted","session":"desk-1","turn":"t","call_id":"c","name":"applescript","args":{}}"""));
        }
        Assert.Empty(Calls("load_session"));
        store.HandleEvent(J("""{"type":"ToolCallStarted","session":"other","turn":"t","call_id":"c","name":"x","args":{}}""")); // another session's event is not the desk's
        clock.Advance(15);
        Assert.Single(Calls("load_session"));
    }

    [Fact]
    public void ATypedEventThatFailsItsDecodeStillEndsTheAsk()
    {
        // The typed TurnFinished needs turn + context; an engine that omits them used to be dropped before the Ask's reader saw it.
        engine.LoadSession = (_, _) => Task.FromResult(Tx.Tail(false, Tx.User(Q, 41), Tx.Assistant("Answered.", 41)));
        Send();
        store.HandleEvent(J("""{"type":"TurnFinished","session":"desk-1","stop_reason":"end_turn"}"""));
        Assert.True(store.Ask.Router.Reply!.Finished);
        Assert.Equal("Answered.", Row.Markdown);
    }

    [Fact]
    public void TheTurnStartedSeqBecomesTheAsksTurnId()
    {
        Send();
        store.HandleEvent(J("""{"type":"TurnStarted","session":"desk-1","turn":"t","model":"m","turn_seq":41}"""));
        Assert.Equal(41, store.Ask.Router.Reply!.Turn);
        engine.LoadSession = (_, _) => Task.FromResult(Tx.Tail(false, Tx.User("different words", 41), Tx.Assistant("Found by the turn id.", 41)));
        Finished();
        Assert.Equal("Found by the turn id.", Row.Markdown);
        Assert.Contains(logs, l => l.Contains("via=turn", StringComparison.Ordinal));
    }

    // MARK: D6 — the log

    [Fact]
    public void TheLogCarriesFocusFirstDeltaCatchUpWatchdogAndFinish()
    {
        var running = true;
        engine.LoadSession = (_, _) => Task.FromResult(Tx.Tail(running, Tx.User(Q, 41), Tx.Assistant("All done.", 41)));
        Send();
        clock.Advance(1.5);
        Delta("All ");
        Delta("done.");
        clock.Advance(15.5); // the desk goes quiet: the watchdog reads the transcript, the turn is still running
        running = false;
        Finished();
        Assert.Contains("set_event_focus([desk-1]) ok (resolve)", logs); // the first ask: one send, from the desk's resolve
        Assert.Contains(logs, l => l == "ask sent to desk desk-1 (31 chars)");
        Assert.Contains(logs, l => l.StartsWith("ask: first TextDelta (4 chars) 1500 ms after send", StringComparison.Ordinal));
        Assert.Equal(1, logs.Count(l => l.StartsWith("ask: first TextDelta", StringComparison.Ordinal))); // the first one only
        Assert.Contains(logs, l => l.StartsWith("ask watchdog: no event for the desk in 15 s", StringComparison.Ordinal));
        Assert.Contains(logs, l => l == "ask catch-up (watchdog): 2 entries, 0 chars from the transcript (9 streamed), running=true, via=text → KeepCollecting");
        Assert.Contains(logs, l => l.StartsWith("ask: TurnFinished (end_turn) for the desk after 9 streamed chars", StringComparison.Ordinal));
        Assert.Contains(logs, l => l == "ask catch-up (turn-finished): 2 entries, 9 chars from the transcript (9 streamed), running=false, via=turn → Finish"); // the watchdog's catch-up learned the turn id; this one matches by it
        Assert.Contains(logs, l => l == "ask finished (turn-finished · turn): 9 chars, 9 streamed, ok");
        engine.Calls.Clear();
        logs.Clear();
        Send("a second ask");
        Assert.Contains("set_event_focus([desk-1]) ok (ask)", logs); // every ask names its reason
    }

    [Fact]
    public void AFailedFinishLogsItsReasonAndTheFinish()
    {
        Send();
        store.HandleEvent(J("""{"type":"Error","session":"desk-1","turn":"t","message":"model: Resolver error\nsecond line"}"""));
        Assert.Contains(logs, l => l == "ask failed on the desk: model: Resolver error\nsecond line");
        Assert.Contains(logs, l => l == "ask finished (Error event): 0 chars, 0 streamed, failed");
        Assert.StartsWith("theuth couldn't reply — model: Resolver error", Row.Text, StringComparison.Ordinal);
    }

    [Fact]
    public void AReplyThatFinishedLingersLongerWhenItIsLong()
    {
        var text = string.Join(" ", Enumerable.Repeat("word", 120));
        engine.LoadSession = (_, _) => Task.FromResult(Tx.Tail(false, Tx.User(Q, 41), Tx.Assistant(text, 41)));
        Send();
        Finished();
        clock.Advance(12);
        Assert.NotNull(store.Receipt); // 12 + (120 − 20)/4 = 37 s
        clock.Advance(24);
        Assert.NotNull(store.Receipt);
        clock.Advance(1.5);
        Assert.Null(store.Receipt);
    }

    [Fact]
    public void ASpokenReplyIsTheTranscriptsFinalTextNeverAPartial()
    {
        engine.LoadSession = (_, _) => Task.FromResult(Tx.Tail(false, Tx.User(Q, 41), Tx.Assistant("Final words.", 41)));
        var spoken = new List<(string, string)>();
        store.AskReplyFinished += (s, t) => spoken.Add((s, t));
        Send();
        Delta("Fin");
        Assert.Empty(spoken);
        Finished();
        Assert.Equal(new[] { ("desk-1", "Final words.") }, spoken);
    }
}

/// <summary>189w D4: the receipt's reply text and markdown, as the Ask bar reads them.</summary>
public sealed class AskReplyViewTests
{
    [Fact]
    public void TheAskBarRendersMarkdownOnlyForARealReplyWithText()
    {
        Assert.Null(AskBarModel.ReplyMarkdown(null));
        Assert.Null(AskBarModel.ReplyMarkdown(new StoreModel.ReceiptRow.Reply("theuth", "sent — waiting for the desk", "desk-1")));
        Assert.Null(AskBarModel.ReplyMarkdown(new StoreModel.ReceiptRow.Failed("x")));
        var streaming = new StoreModel.ReceiptRow.Reply("theuth", "Plan", "desk-1", "## Plan\n- a", Streaming: true);
        Assert.Equal(("## Plan\n- a", true), AskBarModel.ReplyMarkdown(streaming));
        Assert.Equal("theuth · replying…", AskBarModel.ReplyHeader(streaming));
        Assert.Equal("theuth", AskBarModel.ReplyHeader(streaming with { Streaming = false }));
        Assert.Equal(("theuth · Plan — a", "Open Ctrl+O"), AskBarModel.ReceiptLines(new StoreModel.ReceiptRow.Reply("theuth", "Plan — a", "desk-1", "x"), 0)); // the one-line form is unchanged
    }

    [Fact]
    public void TheReceiptLineStripsBlockMarkersAndInlineMarksButNotABoldStart()
    {
        AskReply Reply(string text, bool finished = true) => new("desk") { Text = text, Finished = finished };
        Assert.Equal("Done — all green", Reply("**Done** — all `green`").FirstTwoLines);
        Assert.Equal("Plan — fix the fixture", Reply("## Plan\n- fix the *fixture*\n- more").FirstTwoLines);
        Assert.Equal("-5 degrees — and falling", Reply("-5 degrees\n> and falling").FirstTwoLines);
        Assert.Equal("first — second", Reply("---\nfirst\n***\nsecond\nthird").FirstTwoLines);
        Assert.Equal("see the docs", Reply("see [the docs](https://example.com/docs)").FirstTwoLines);
        Assert.Equal("snake_case_name stays", Reply("snake_case_name stays").FirstTwoLines);
        Assert.Equal("bold so far", Reply("**bold so far", finished: false).FirstTwoLines); // streaming: no stray asterisks
        Assert.Equal("**bold so far", Reply("**bold so far", finished: true).FirstTwoLines);
    }
}

/// <summary>189w D4: the incremental markdown parser and the diff a view uses to touch only the block being written.</summary>
public sealed class MarkdownStreamingTests
{
    private static IReadOnlyList<MdInline> Open(string s) => AnswerMarkdown.Inline(s, open: true);

    [Fact]
    public void AnUnclosedMarkAtTheTailShowsAsClosedWhileItStreamsAndLiteralOnceFinal()
    {
        Assert.Equal(new MdInline[] { new MdInline.Text("so "), new MdInline.Bold("bo") }, Open("so **bo"));
        Assert.Equal(new MdInline[] { new MdInline.Bold("bold") }, Open("**bold*")); // the first star of the closing pair
        Assert.Equal(new MdInline[] { new MdInline.Text("run "), new MdInline.Code("cargo te") }, Open("run `cargo te"));
        Assert.Equal(new MdInline[] { new MdInline.Text("a "), new MdInline.Italic("slant") }, Open("a *slant"));
        Assert.Equal(new MdInline[] { new MdInline.Text("a "), new MdInline.Italic("slant") }, Open("a _slant"));
        Assert.Equal(new MdInline[] { new MdInline.Text("see "), new MdInline.Text("the doc") }, Open("see [the doc](https://exa")); // the pending url is never shown
        Assert.Equal(new MdInline[] { new MdInline.Text("see "), new MdInline.Text("the doc") }, Open("see [the doc]"));
        Assert.Equal(new MdInline[] { new MdInline.Text("see "), new MdInline.Text("the") }, Open("see [the"));
        Assert.Equal(new MdInline[] { new MdInline.Text("tail") }, Open("tail*")); // a lone mark at the very end is the start of the next one
        Assert.Equal(new MdInline[] { new MdInline.Text("tail") }, Open("tail`"));
        // Closed text parses exactly as before: an unclosed mark is literal.
        Assert.Equal(new MdInline[] { new MdInline.Text("so **bo") }, AnswerMarkdown.Inline("so **bo"));
        Assert.Equal(new MdInline[] { new MdInline.Text("run `cargo te") }, AnswerMarkdown.Inline("run `cargo te"));
        Assert.Equal(new MdInline[] { new MdInline.Text("2 * 3 = 6") }, Open("2 * 3 = 6"));
    }

    [Fact]
    public void AnUnderscoreInsideAWordIsNotEmphasis()
    {
        Assert.Equal(new MdInline[] { new MdInline.Text("call snake_case_name now") }, AnswerMarkdown.Inline("call snake_case_name now"));
        Assert.Equal(new MdInline[] { new MdInline.Text("a "), new MdInline.Italic("real one"), new MdInline.Text(" here") }, AnswerMarkdown.Inline("a _real one_ here"));
        Assert.Equal(new MdInline[] { new MdInline.Text("my_var_ and _x") }, AnswerMarkdown.Inline("my_var_ and _x"));
        Assert.Equal("snake_case_name", AnswerMarkdown.Plain("snake_case_name"));
    }

    [Fact]
    public void ALastLineThatIsOnlyTheStartOfABlockMarkerIsHeldBack()
    {
        foreach (var stub in new[] { "#", "##", "###### ", "-", "*", "•", "1", "12.", "3)", "`", "``" })
        {
            var blocks = AnswerMarkdown.Parse("First paragraph.\n\n" + stub, streaming: true);
            Assert.Single(blocks);
            Assert.IsType<MdBlock.Paragraph>(blocks[0]);
        }
        // Once the line has a word it is the block; and a final (non-streaming) parse never holds anything back.
        Assert.IsType<MdBlock.Heading>(AnswerMarkdown.Parse("## Pla", streaming: true)[0]);
        Assert.IsType<MdBlock.Bullet>(AnswerMarkdown.Parse("- a", streaming: true)[0]);
        Assert.Equal(2, AnswerMarkdown.Parse("First.\n\n-", streaming: false).Count);
        // A stub inside a fence is code, not a marker.
        Assert.Equal("code\n#", Assert.IsType<MdBlock.Code>(AnswerMarkdown.Parse("```\ncode\n#", streaming: true)[0]).Text);
        // A closing fence still being typed is not code.
        Assert.Equal("code", Assert.IsType<MdBlock.Code>(AnswerMarkdown.Parse("```\ncode\n``", streaming: true)[0]).Text);
    }

    [Fact]
    public void OnlyTheLastBlockMayHoldAnUnclosedMark()
    {
        var blocks = AnswerMarkdown.Parse("**not closed\n\nsecond **bo", streaming: true);
        Assert.Equal(new MdInline[] { new MdInline.Text("**not closed") }, ((MdBlock.Paragraph)blocks[0]).Inlines);
        Assert.Equal(new MdInline[] { new MdInline.Text("second "), new MdInline.Bold("bo") }, ((MdBlock.Paragraph)blocks[1]).Inlines);
    }

    [Fact]
    public void NestedBulletsTablesAndAnUnclosedFenceParse()
    {
        var blocks = AnswerMarkdown.Parse("- top\n  - nested\n    - deeper\n1. one\n   - under one\n\n| a | b |\n|---|---|\n| 1 | 2 |\n\n```rust\nlet x = 1;", streaming: false);
        Assert.Equal(new[] { 0, 1, 2, 0, 1 }, blocks.OfType<MdBlock.Bullet>().Select(b => b.Indent).ToArray());
        var table = blocks.OfType<MdBlock.Code>().First();
        Assert.Equal(("table", "| a | b |\n|---|---|\n| 1 | 2 |"), (table.Language, table.Text));
        var open = blocks.OfType<MdBlock.Code>().Last();
        Assert.Equal(("rust", "let x = 1;"), (open.Language, open.Text));
    }

    [Fact]
    public void EveryExistingBlockKindStillParses()
    {
        var blocks = AnswerMarkdown.Parse("# Title\nThe failing test is `vad::gate_onset`.\n\n- fix the fixture → **vad.rs:112**\n- or keep *7*\n1. first\n2) second\n```rust\nlet x = 1;\n```\nSee [design](theuth://c/x).");
        Assert.Equal(new[] { 'H', 'P', 'B', 'B', 'B', 'B', 'C', 'P' }, blocks.Select(MdDiff.Kind).ToArray());
    }

    [Fact]
    public void ASignatureDistinguishesWhatItDrawsAndIgnoresNothingItDraws()
    {
        string Sig(string md) => MdDiff.Signature(AnswerMarkdown.Parse(md)[0]);
        Assert.NotEqual(Sig("# a"), Sig("## a"));
        Assert.NotEqual(Sig("a"), Sig("a b"));
        Assert.NotEqual(Sig("- a"), Sig("1. a"));
        Assert.NotEqual(Sig("- a"), Sig("  - a"));
        Assert.NotEqual(Sig("**a**"), Sig("*a*"));
        Assert.NotEqual(Sig("[a](x)"), Sig("[a](y)"));
        Assert.NotEqual(Sig("```a\nb"), Sig("```\na\nb"));
        Assert.Equal(Sig("same **text**"), Sig("same **text**"));
    }

    [Fact]
    public void AStreamedChunkChangesOnlyTheBlockBeingWritten()
    {
        const string Reply = "## Plan\n\nHanded to **w3** — the docs ship tonight.\n\n- rail: done\n- panel: `done`\n  - nested note\n\n```rust\nfn main() {}\n```\n\nSee [the notes](https://example.com/n) and snake_case_name.\n\n| a | b |\n|---|---|\n| 1 | 2 |\n";
        IReadOnlyList<string> Sigs(int length) => AnswerMarkdown.Parse(Reply[..length], streaming: true).Select(MdDiff.Signature).ToList();
        var before = Sigs(0);
        int movedTail = 0;
        for (int i = 1; i <= Reply.Length; i++)
        {
            var after = Sigs(i);
            int common = MdDiff.CommonPrefix(before, after);
            // Everything but the last block that existed before stays exactly as drawn: one block's layout per chunk, never the answer's.
            Assert.True(common >= before.Count - 1, $"char {i} ({Reply[i - 1]:D}) changed {before.Count - common} blocks: {string.Join(" | ", before)} → {string.Join(" | ", after)}");
            if (common < before.Count) movedTail++;
            before = after;
        }
        Assert.True(movedTail > 0);
        Assert.Equal(AnswerMarkdown.Parse(Reply).Select(MdDiff.Signature), Sigs(Reply.Length).Concat(Array.Empty<string>())); // the streamed end state equals the final parse
    }

    [Fact]
    public void TheCommonPrefixCountsLeadingIdenticalSignatures()
    {
        Assert.Equal(2, MdDiff.CommonPrefix(new[] { "a", "b", "c" }, new[] { "a", "b", "x", "y" }));
        Assert.Equal(0, MdDiff.CommonPrefix(Array.Empty<string>(), new[] { "a" }));
        Assert.Equal(1, MdDiff.CommonPrefix(new[] { "a" }, new[] { "a", "b" }));
    }
}