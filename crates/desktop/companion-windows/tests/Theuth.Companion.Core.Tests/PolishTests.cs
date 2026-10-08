namespace Theuth.Companion.Core.Tests;

// Design 134 W6 polish: the first-run "pin theuth to the taskbar corner" decision (item 1) and the settings key it persists;
// the failed-desk-turn receipt (item 7).

public sealed class PinTipTests
{
    private const string Exe = @"C:\Users\user\AppData\Local\theuth\companion\Theuth.Companion.exe";
    private static readonly TimeSpan Late = TimeSpan.FromSeconds(61);

    private static NotifyIconEntry Entry(string exe = Exe, int? promoted = null) => new(exe, promoted);

    private static PinTipVerdict Decide(string? shownFor = null, IEnumerable<NotifyIconEntry>? entries = null, TimeSpan? since = null, bool paused = false, bool off = false) =>
        PinTip.Decide(shownFor, Exe, entries ?? Array.Empty<NotifyIconEntry>(), since ?? Late, paused, off);

    [Fact]
    public void NotPromotedShowsOnceTheEntryExists()
    {
        Assert.Equal(PinTipVerdict.Show, Decide(entries: new[] { Entry(promoted: 0) }));
        Assert.Equal(PinTipVerdict.Show, Decide(entries: new[] { Entry(promoted: null) })); // IsPromoted absent = overflow
        // Another install's row (W2's slot path) does not speak for this exe.
        Assert.Equal(PinTipVerdict.NoEntryYet, Decide(entries: new[] { Entry(@"C:\Users\user\Develop\theuth-ci-w2\x\Theuth.Companion.exe", 1) }));
    }

    [Fact]
    public void PromotedResolvesAndNoEntryWaits()
    {
        Assert.Equal(PinTipVerdict.Promoted, Decide(entries: new[] { Entry(promoted: 1) }));
        Assert.Equal(PinTipVerdict.Promoted, Decide(entries: new[] { Entry(Exe.ToUpperInvariant(), 1) })); // Explorer's casing
        Assert.Equal(PinTipVerdict.NoEntryYet, Decide());
        Assert.True(PinTip.Persists(PinTipVerdict.Promoted) && PinTip.Persists(PinTipVerdict.Show));
        Assert.False(PinTip.Persists(PinTipVerdict.NoEntryYet) || PinTip.Persists(PinTipVerdict.Paused) || PinTip.Persists(PinTipVerdict.Off));
    }

    [Fact]
    public void TooEarlyBeforeSixtySeconds()
    {
        Assert.Equal(PinTipVerdict.TooEarly, Decide(entries: new[] { Entry(promoted: 0) }, since: TimeSpan.FromSeconds(59)));
        Assert.Equal(PinTipVerdict.Show, Decide(entries: new[] { Entry(promoted: 0) }, since: TimeSpan.FromSeconds(60)));
        Assert.False(PinTip.IsFinal(PinTipVerdict.TooEarly) || PinTip.IsFinal(PinTipVerdict.NoEntryYet) || PinTip.IsFinal(PinTipVerdict.Paused));
    }

    [Fact]
    public void AlreadyShownIsPerExePath()
    {
        Assert.Equal(PinTipVerdict.AlreadyShown, Decide(shownFor: Exe, entries: new[] { Entry(promoted: 0) }));
        Assert.Equal(PinTipVerdict.AlreadyShown, Decide(shownFor: Exe.ToLowerInvariant() + " ", entries: new[] { Entry(promoted: 0) }));
        // Shown for the Debug build in a slot ≠ shown for the install: once per install.
        Assert.Equal(PinTipVerdict.Show, Decide(shownFor: @"C:\Users\user\Develop\theuth-ci-w6\…\Theuth.Companion.exe", entries: new[] { Entry(promoted: 0) }));
        Assert.True(PinTip.IsFinal(PinTipVerdict.AlreadyShown));
    }

    [Fact]
    public void PausedWaitsAndOffStaysSilent()
    {
        // W3's informational-toast rules: nothing while paused (re-check later), nothing when the deferrable class is Off.
        Assert.Equal(PinTipVerdict.Paused, Decide(entries: new[] { Entry(promoted: 0) }, paused: true));
        Assert.Equal(PinTipVerdict.Promoted, Decide(entries: new[] { Entry(promoted: 1) }, paused: true)); // promotion still resolves
        Assert.Equal(PinTipVerdict.Off, Decide(entries: new[] { Entry(promoted: 0) }, off: true));
        Assert.Equal(PinTipVerdict.AlreadyShown, Decide(shownFor: Exe, entries: new[] { Entry(promoted: 0) }, off: true));
        Assert.True(PinTip.IsFinal(PinTipVerdict.Off));
    }

    [Fact]
    public void TheToastTextNamesTheSettingsPath()
    {
        Assert.Equal("Keep theuth in view", PinTip.Title);
        Assert.Contains("Settings › Personalization › Taskbar › Other system tray icons › theuth", PinTip.Body, StringComparison.Ordinal);
        Assert.Equal("Open taskbar settings", PinTip.ButtonLabel);
        Assert.Equal("ms-settings:taskbar", PinTip.SettingsUri);
        Assert.StartsWith("pin tip: check 2 verdict=Show sinceAdd=61s entry=IsPromoted=0", PinTip.Describe(PinTipVerdict.Show, Entry(promoted: 0), Late, 2), StringComparison.Ordinal);
        Assert.EndsWith("entry=none", PinTip.Describe(PinTipVerdict.NoEntryYet, null, Late, 1), StringComparison.Ordinal);
    }

    [Fact]
    public void TheShownForKeyRoundTripsAndStaysOutOfTheFileWhenEmpty()
    {
        var store = new SettingsStore();
        var s = new CompanionSettings { PinTipShownFor = Exe };
        s.Save(store);
        Assert.Equal(Exe, store.String(PinTip.Key));
        var back = CompanionSettings.Load(store);
        Assert.Equal(Exe, back.PinTipShownFor);
        Assert.Equal(s, back);
        Assert.NotEqual(s, new CompanionSettings()); // the flag is part of equality, so SettingsModel.Update persists it
        new CompanionSettings().Save(store);
        Assert.False(store.Contains(PinTip.Key));
        Assert.Equal("", CompanionSettings.Load(store).PinTipShownFor);
        Assert.Contains(PinTip.Key, CompanionSettings.Key.All);
    }
}

// Design 134 W6 item 7: a failed desk turn ends the Ask bar's receipt (found live on build-B 2026-10-08 04:06 — every turn failed
// with a model resolver error while the receipt said "sent — waiting for the desk" forever).
public sealed class AskReplyFailureTests
{
    private const string Resolver = "model: Resolver error for model 'anthropic::claude-sonnet-4-5 (adapter: Anthropic)'. Cause: ApiKeyEnvNotFound { env_name: \"ANTHROPIC_API_KEY\" }";

    private static AskRouter Sent()
    {
        var r = new AskRouter();
        r.Resolved("desk", hub: true);
        r.Sent("desk");
        return r;
    }

    [Fact]
    public void TheErrorEventEndsTheReceiptWithTheFirstLineClipped()
    {
        var r = Sent();
        Assert.False(r.Observe(new ReplyEvent.Error("other", "noise"))); // another session's error is not ours
        Assert.True(r.Observe(new ReplyEvent.Error("desk", Resolver + "\nsecond line is not shown")));
        var reply = r.Reply!;
        Assert.True(reply.Finished && reply.Failed);
        var text = reply.FirstTwoLines;
        Assert.StartsWith("theuth couldn't reply — model: Resolver error for model 'anthropic::claude-sonnet-4-5", text, StringComparison.Ordinal);
        Assert.EndsWith("…", text, StringComparison.Ordinal);
        Assert.True(text.Length <= AskReply.FailedPrefix.Length + AskReply.FailedClip);
        Assert.DoesNotContain("second line", text, StringComparison.Ordinal);
        // The engine then publishes TurnFinished { stop_reason: "error" }: the finished reply stays as it is.
        Assert.False(r.Observe(new ReplyEvent.TurnFinished("desk", "error")));
        Assert.False(r.Observe(new ReplyEvent.TextDelta("desk", "late")));
        Assert.Equal(text, r.Reply!.FirstTwoLines);
    }

    [Fact]
    public void StopReasonErrorEndsTheReceiptEvenWithNothingStreamed()
    {
        var r = Sent();
        Assert.True(r.Observe(new ReplyEvent.TurnFinished("desk", "error")));
        Assert.True(r.Reply!.Finished && r.Reply.Failed);
        Assert.Equal("theuth couldn't reply — the desk's turn ended with an error", r.Reply.FirstTwoLines);
    }

    [Fact]
    public void CancelledTimedOutAndInterruptedEndTheReceipt()
    {
        var r = Sent();
        Assert.True(r.Observe(new ReplyEvent.TurnFinished("desk", "cancelled")));
        Assert.Equal("theuth couldn't reply — the turn was cancelled", r.Reply!.FirstTwoLines);
        r = Sent();
        Assert.True(r.Observe(new ReplyEvent.TextDelta("desk", "half an answer")));
        Assert.True(r.Observe(new ReplyEvent.TurnFinished("desk", "timed_out")));
        Assert.Equal("theuth couldn't reply — the turn was timed out", r.Reply!.FirstTwoLines); // the failure wins over the partial text
        r = Sent();
        Assert.True(r.Observe(new ReplyEvent.TurnFinished("desk", "interrupted")));
        Assert.True(r.Reply!.Failed);
        Assert.True(AskRouter.IsFailure("error") && AskRouter.IsFailure("cancelled") && AskRouter.IsFailure("timed_out") && AskRouter.IsFailure("interrupted"));
        Assert.False(AskRouter.IsFailure("end_turn") || AskRouter.IsFailure("stop") || AskRouter.IsFailure(null) || AskRouter.IsFailure("max_tokens"));
    }

    [Fact]
    public void ANormalReplyIsUnchanged()
    {
        var r = Sent();
        Assert.False(r.Observe(new ReplyEvent.TurnFinished("desk", "end_turn"))); // the steered-into turn: nothing streamed, not ours
        Assert.True(r.Observe(new ReplyEvent.TextDelta("desk", "On it — w3 owns crates/rpc\n")));
        Assert.True(r.Observe(new ReplyEvent.TurnFinished("desk", "end_turn")));
        var reply = r.Reply!;
        Assert.True(reply.Finished && !reply.Failed);
        Assert.Equal("On it — w3 owns crates/rpc", reply.FirstTwoLines);
        // Older decoders pass no stop reason: still a normal finish.
        r = Sent();
        Assert.True(r.Observe(new ReplyEvent.TextDelta("desk", "yes")));
        Assert.True(r.Observe(new ReplyEvent.TurnFinished("desk")));
        Assert.False(r.Reply!.Failed);
    }

    [Fact]
    public void TheWireShapeCarriesStopReasonAndMessage()
    {
        var finished = Theuth.Companion.Rpc.StreamDelta.Decode("{\"type\":\"TurnFinished\",\"session\":\"desk\",\"turn\":7,\"stop_reason\":\"error\",\"usage\":{},\"context\":12}"u8);
        Assert.Equal(("TurnFinished", "desk", "error"), (finished.Type, finished.Session, finished.StopReason));
        var error = Theuth.Companion.Rpc.StreamDelta.Decode("{\"type\":\"Error\",\"session\":\"desk\",\"turn\":7,\"message\":\"model: boom\"}"u8);
        Assert.Equal(("Error", "desk", "model: boom"), (error.Type, error.Session, error.Message));
        var delta = Theuth.Companion.Rpc.StreamDelta.Decode("{\"type\":\"TextDelta\",\"session\":\"desk\",\"text\":\"hi\"}"u8);
        Assert.True(delta.StopReason is null && delta.Message is null && delta.Text == "hi");
    }
}

// Design 134 W6 item 8: a process LAUNCHED by a toast click reads its own activation (GetActivatedEventArgs) — the mapping
// and the hold-until-refetch rule. Item 9: the Pause notifications ▸ check marks (the Mac menu's rule).
public sealed class ToastActivationTests
{
    private static Dictionary<string, string> Args(params (string K, string V)[] kv) => kv.ToDictionary(p => p.K, p => p.V, StringComparer.Ordinal);

    [Fact]
    public void ParseReadsActionBannerItemAndText()
    {
        var a = ToastActivation.Parse(Args(("action", "opt-2"), ("banner", "hire:1"), ("item", "hire:1"), ("kind", "question")), Args(("text", "  yes ")));
        Assert.Equal(new ToastActivation("opt-2", "hire:1", "hire:1", "yes"), a);
        Assert.Equal(1, a.OptionIndex);
        // A body tap carries no action: open; an empty text box is no text.
        var body = ToastActivation.Parse(Args(("banner", "b")), Args(("text", "   ")));
        Assert.Equal(("open", "b", null, null), (body.Action, body.Banner, body.Item, body.Text));
        Assert.Null(body.OptionIndex);
        Assert.True(ToastActivation.Parse(Args(("action", "later")), null).IsNoop);
    }

    [Fact]
    public void ALaunchedProcessHoldsEverythingButLaterUntilTheFirstRefetch()
    {
        var open = ToastActivation.Parse(Args(("action", "open"), ("banner", "hire:1")), null);
        Assert.True(open.HoldUntilRefetch(launched: true, refetchedOnce: false, itemCount: 0));   // the panel opens on the item once it exists
        Assert.False(open.HoldUntilRefetch(launched: true, refetchedOnce: true, itemCount: 3));
        Assert.False(open.HoldUntilRefetch(launched: false, refetchedOnce: false, itemCount: 0)); // a running instance opens at once (W3)
        var answer = ToastActivation.Parse(Args(("action", "opt-1"), ("item", "x")), null);
        Assert.True(answer.HoldUntilRefetch(launched: false, refetchedOnce: false, itemCount: 0));  // W3's relaunch rule, unchanged
        Assert.False(answer.HoldUntilRefetch(launched: false, refetchedOnce: false, itemCount: 2));
        Assert.True(answer.HoldUntilRefetch(launched: true, refetchedOnce: false, itemCount: 2));
        var later = ToastActivation.Parse(Args(("action", "later")), null);
        Assert.False(later.HoldUntilRefetch(launched: true, refetchedOnce: false, itemCount: 0));
    }
}

public sealed class PauseMenuTests
{
    private static readonly DateTimeOffset Now = new(2026, 10, 8, 4, 23, 0, TimeSpan.FromHours(1));
    private static readonly DateTimeOffset Tomorrow = new(2026, 10, 9, 0, 0, 0, TimeSpan.FromHours(1));

    [Fact]
    public void OffWhenNotPausedOrExpired()
    {
        Assert.Equal(PauseMenuState.Off, PauseMenu.State(null, Now, Tomorrow));
        Assert.Equal(PauseMenuState.Off, PauseMenu.State(Now.AddMinutes(-1), Now, Tomorrow));
        Assert.Equal("Pause notifications", PauseMenu.Title(null, Now));
    }

    [Fact]
    public void HourAndTomorrowFollowTheMacRule()
    {
        Assert.Equal(PauseMenuState.Hour, PauseMenu.State(Now.AddHours(1), Now, Tomorrow));
        Assert.Equal(PauseMenuState.Tomorrow, PauseMenu.State(Tomorrow, Now, Tomorrow));
        // Paused "for 1 hour" at 23:30 ends tomorrow but within the hour: still the hour row.
        var late = new DateTimeOffset(2026, 10, 8, 23, 30, 0, TimeSpan.FromHours(1));
        Assert.Equal(PauseMenuState.Hour, PauseMenu.State(late.AddHours(1), late, Tomorrow));
        Assert.Equal("Pause notifications (until 05:23)", PauseMenu.Title(Now.AddHours(1), Now));
    }
}

// Design 134 W6 item 10: the "theuth engine unreachable" toast waits out NotificationPolicy.EngineGrace — the shell's quit
// handshake stops the engine ~60 ms before WM_CLOSE reaches the companion, and every whole-app quit/update left that toast in
// Notification Center. A loss that lasts still notifies exactly as before.
public sealed class EngineLossGraceTests
{
    private static readonly DateTimeOffset T0 = new(2026, 10, 8, 6, 49, 56, TimeSpan.FromHours(1));
    private static readonly TimeSpan G = NotificationPolicy.EngineGrace;

    private static NotificationPolicy Fresh(NotificationPrefs? prefs = null)
    {
        var p = new NotificationPolicy(prefs ?? new NotificationPrefs());
        _ = p.Update(Array.Empty<AttentionItem>(), T0);
        return p;
    }

    [Fact]
    public void QuitInitiatedLossNeverPosts()
    {
        var p = Fresh();
        p.Quitting(); // the companion sent theuth://quit itself
        Assert.True(p.Engine(false, T0.AddMilliseconds(60)).IsEmpty);
        Assert.False(p.EngineLossPending);
        Assert.True(p.Tick(T0.AddSeconds(30)).IsEmpty);
    }

    [Fact]
    public void ALossIsHeldAndNothingPostsBeforeTheGrace()
    {
        // The packaged shape: engine gone at .562, WM_CLOSE at .596 — the process is gone before any tick inside the grace.
        var p = Fresh();
        Assert.True(p.Engine(false, T0).IsEmpty);
        Assert.True(p.EngineLossPending);
        Assert.True(p.Tick(T0.AddSeconds(1)).IsEmpty);
        Assert.True(p.Tick(T0 + G - TimeSpan.FromMilliseconds(1)).IsEmpty);
        Assert.True(p.Engine(false, T0.AddSeconds(2)).IsEmpty); // repeated drops do not restart the clock
    }

    [Fact]
    public void RecoveryInsideTheGraceIsSilent()
    {
        var p = Fresh();
        _ = p.Engine(false, T0);
        var back = p.Engine(true, T0.AddSeconds(3));
        Assert.True(back.IsEmpty); // nothing was posted, so nothing to withdraw
        Assert.False(p.EngineLossPending);
        Assert.True(p.Tick(T0.AddSeconds(30)).IsEmpty);
        // A later real loss starts a fresh grace.
        _ = p.Engine(false, T0.AddSeconds(40));
        Assert.Single(p.Tick(T0.AddSeconds(40) + G).Post);
    }

    [Fact]
    public void ALastingLossPostsOnceAfterTheGraceAndWithdrawsOnReconnect()
    {
        var p = Fresh();
        _ = p.Engine(false, T0);
        var due = p.Tick(T0 + G);
        var b = Assert.Single(due.Post);
        Assert.Equal((Banner.EngineId, BannerCategory.Error, "theuth engine unreachable", BannerLevel.Active, true), (b.Id, b.Category, b.Title, b.Level, b.Sound));
        Assert.False(p.EngineLossPending);
        Assert.True(p.Tick(T0 + G + TimeSpan.FromSeconds(1)).IsEmpty); // once
        Assert.True(p.Engine(false, T0 + G + TimeSpan.FromSeconds(2)).IsEmpty);
        Assert.Equal(new[] { Banner.EngineId }, p.Engine(true, T0.AddSeconds(60)).Withdraw);
        Assert.Equal(5, (int)G.TotalSeconds);
    }

    [Fact]
    public void PausedOrOffStaySilentAtTheDueTime()
    {
        var paused = Fresh();
        paused.Pause(T0.AddHours(1));
        _ = paused.Engine(false, T0);
        Assert.True(paused.Tick(T0 + G).IsEmpty);
        var off = Fresh(new NotificationPrefs { Error = NotifyMode.Off });
        _ = off.Engine(false, T0);
        Assert.True(off.Tick(T0 + G).IsEmpty);
        // The replay still shows exactly one error banner in its engine segment (C6 reading).
        var steps = NotificationReplay.Run(new[] { PolicyFx.It("decision:1", AttentionKind.Decision, agent: "d1") }, new NotificationPrefs(), PolicyFx.T0);
        var engine = Assert.Single(steps, s => s.Segment == NotificationReplay.SegmentEngine);
        Assert.Single(engine.Output.Post);
    }
}
