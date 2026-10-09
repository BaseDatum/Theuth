namespace Theuth.Companion.Core.Tests;

using static PointMachine;

// 149 WW3 / D12: the 17 tests of crates/desktop/src/point/machine.rs, one for one, over the C# port.
public sealed class PointMachineTests
{
    private static readonly PointState Idle = new PointState.Idle();
    private static readonly PointState Replying = new PointState.Replying();
    private static readonly PointEvent Pressed = new PointEvent.Pressed();
    private static readonly PointEvent Released = new PointEvent.Released();
    private static readonly PointEvent Esc = new PointEvent.Esc();
    private static readonly PointEvent PanelDone = new PointEvent.PanelDone();
    private static readonly PointEvent Cancel = new PointEvent.Cancel();
    private static PointEvent Tick(ulong now) => new PointEvent.Tick(now);
    private static PointState Armed(ulong t0) => new PointState.Armed(t0);
    private static PointState Holding(ulong t0) => new PointState.Holding(t0);
    private static PointState Tap1(ulong t0, ulong released) => new PointState.Tap1(t0, released);
    private static PointState Locked(ulong t0) => new PointState.Locked(t0);
    private static PointState Sending(ulong t0) => new PointState.Sending(t0);
    private static PointEffect Start => new PointEffect.Start();
    private static PointEffect ShowLock => new PointEffect.ShowLock();
    private static PointEffect Conceal => new PointEffect.Conceal();
    private static PointEffect Stop(string reason) => new PointEffect.Stop(reason);
    private static PointEffect Discard(string reason, string? hint) => new PointEffect.Discard(reason, hint);

    private static (PointState, IReadOnlyList<PointEffect>) S(PointState st, PointEvent e, ulong now) => Step(st, e, now, null);

    private static void Expect(PointState st, PointEvent e, ulong now, PointState state, params PointEffect[] fx)
    {
        var (s, effects) = S(st, e, now);
        Assert.Equal(state, s);
        Assert.Equal(fx, effects);
    }

    [Fact]
    public void IdlePressStartsAndArms()
    {
        Expect(Idle, Pressed, 10, Armed(10), Start);
        foreach (var e in new[] { Released, Esc, Tick(5), PanelDone, Cancel }) Expect(Idle, e, 10, Idle);
    }

    [Fact]
    public void RefusedPressAnswersRefuseNotStart()
    {
        foreach (var st in new[] { Idle, Replying })
        {
            var (s, fx) = Step(st, Pressed, 10, "meeting");
            Assert.Equal(st, s);
            Assert.Equal(new PointEffect[] { new PointEffect.Refuse("meeting") }, fx);
        }
        // Only a press is refused; nothing else changes.
        var (s2, fx2) = Step(Holding(0), Released, 1000, "meeting");
        Assert.Equal(Sending(0), s2);
        Assert.Equal(new[] { Stop("release") }, fx2);
    }

    [Fact]
    public void AutoRepeatPressIsIgnored()
    {
        foreach (var st in new[] { Armed(0), Holding(0) }) Expect(st, Pressed, 50, st);
        // Locked's press is the send, not a repeat — covered below; Sending ignores presses too.
        Expect(Sending(0), Pressed, 50, Sending(0));
    }

    [Fact]
    public void ArmedReleaseBeforeHoldIsTap1() => Expect(Armed(0), Released, HoldMs - 1, Tap1(0, HoldMs - 1));

    [Fact]
    public void ArmedReleaseAfterHoldSends()
    {
        Expect(Armed(0), Released, MinNoteMs, Sending(0), Stop("release"));
        // A release past HOLD_MS but under MIN_NOTE_MS (a late hold tick) is the same short note Holding would discard.
        Expect(Armed(0), Released, HoldMs + 10, Idle, Discard("short", Hint));
    }

    [Fact]
    public void ArmedTickMovesToHoldingOnlyAfterHold()
    {
        Expect(Armed(0), Tick(HoldMs), HoldMs, Holding(0));
        Expect(Armed(0), Tick(50), HoldMs - 1, Armed(0));
    }

    [Fact]
    public void HoldingReleaseSendsOrDiscardsShort()
    {
        Expect(Holding(0), Released, MinNoteMs, Sending(0), Stop("release"));
        Expect(Holding(0), Released, MinNoteMs - 1, Idle, Discard("short", Hint));
    }

    [Fact]
    public void SecondPressInsideWindowLocks() => Expect(Tap1(0, 100), Pressed, 100 + DoubleTapMs, Locked(0), ShowLock);

    [Fact]
    public void LateSecondPressClosesTheTapAndStartsAnew() =>
        Expect(Tap1(0, 100), Pressed, 100 + DoubleTapMs + 1, Armed(401), Discard("double-tap-missed", Hint), Start);

    [Fact]
    public void Tap1TickAfterWindowDiscardsWithHint()
    {
        var st = Tap1(0, 100);
        Expect(st, Tick(401), 100 + DoubleTapMs, st);
        Expect(st, Tick(401), 100 + DoubleTapMs + 1, Idle, Discard("double-tap-missed", "Hold to talk · double-tap to lock"));
        Expect(st, Released, 150, st);
    }

    [Fact]
    public void LockedPressSendsOrDiscardsShort()
    {
        Expect(Locked(0), Pressed, MinNoteMs - 1, Idle, Discard("short", Hint));
        Expect(Locked(0), Pressed, MinNoteMs, Sending(0), Stop("tap"));
        Expect(Locked(0), Released, 50, Locked(0));
        Expect(Locked(0), Tick(50), 50, Locked(0));
    }

    [Fact]
    public void EscInEveryActiveState()
    {
        foreach (var st in new[] { Armed(0), Holding(0), Locked(0) }) Expect(st, Esc, 50, Idle, Discard("esc", Hint));
        foreach (var st in new[] { Tap1(0, 10), Sending(0) }) Expect(st, Esc, 50, Idle, Discard("esc", null));
        Expect(Replying, Esc, 50, Idle, Conceal);
    }

    [Fact]
    public void CancelInEveryActiveState()
    {
        foreach (var st in new[] { Armed(0), Holding(0), Tap1(0, 10), Locked(0) }) Expect(st, Cancel, 50, Idle, Discard("cancel", null));
        Expect(Sending(0), Cancel, 50, Idle);
        Expect(Replying, Cancel, 50, Idle, Conceal);
    }

    [Fact]
    public void PanelDoneOnlyMattersInSendingAndReplying()
    {
        Expect(Sending(0), PanelDone, 50, Replying);
        Expect(Replying, PanelDone, 50, Idle, Conceal);
        foreach (var st in new[] { Armed(0), Holding(0), Tap1(0, 10), Locked(0) }) Expect(st, PanelDone, 50, st);
    }

    [Fact]
    public void ReplyingPressStartsANewQuestion()
    {
        Expect(Replying, Pressed, 9000, Armed(9000), Start);
        foreach (var e in new[] { Released, Tick(1) }) Expect(Replying, e, 9000, Replying);
    }

    [Fact]
    public void FullHoldThenDoubleTapSequence()
    {
        // Hold: press, tick → Holding, release at 1.2 s → Stop.
        var (st, fx) = S(Idle, Pressed, 0);
        Assert.Equal(new[] { Start }, fx);
        (st, _) = S(st, Tick(HoldMs), HoldMs);
        Assert.Equal(Holding(0), st);
        (st, fx) = S(st, Released, 1200);
        Assert.Equal(Sending(0), st);
        Assert.Equal(new[] { Stop("release") }, fx);
        (st, _) = S(st, PanelDone, 1300);
        (st, fx) = S(st, PanelDone, 7300);
        Assert.Equal(Idle, st);
        Assert.Equal(new[] { Conceal }, fx);
        // Lock: press, release at 100, press at 250 → Locked, press at 2000 → Stop{tap}.
        (st, _) = S(Idle, Pressed, 0);
        (st, _) = S(st, Released, 100);
        (st, fx) = S(st, Pressed, 250);
        Assert.Equal(Locked(0), st);
        Assert.Equal(new[] { ShowLock }, fx);
        (st, _) = S(st, Released, 300);
        (st, fx) = S(st, Pressed, 2000);
        Assert.Equal(Sending(0), st);
        Assert.Equal(new[] { Stop("tap") }, fx);
    }

    [Fact]
    public void StateSerialisesTaggedSnakeCase()
    {
        Assert.Equal("""{"state":"idle"}""", Idle.Json);
        Assert.Equal("""{"state":"tap1","t0":1,"released":2}""", Tap1(1, 2).Json);
    }

    [Fact]
    public void ConstantsMatchTheShell()
    {
        Assert.Equal(200UL, HoldMs);
        Assert.Equal(300UL, DoubleTapMs);
        Assert.Equal(800UL, MinNoteMs);
        Assert.Equal(60UL, SampleHz);
        Assert.Equal(6000UL, DoneMs);
        Assert.True(Armed(1).IsGesture && Tap1(1, 2).IsGesture && Locked(1).IsGesture && Holding(1).IsGesture);
        Assert.False(Idle.IsGesture || Sending(1).IsGesture || Replying.IsGesture);
    }

    // 153 wave C (proof 16): the mode switch while holding (§10.1, R2) and the Automatic gesture (§12.6), one test each as the Swift twin.

    private static PointState Toggled(ulong t0) => new PointState.Toggled(t0);
    private static PointEvent Switch(PointMode to) => new PointEvent.SwitchMode(to);
    private static (PointState, IReadOnlyList<PointEffect>) A(PointState st, PointEvent e, ulong now) => Step(st, e, now, null, Gesture.Automatic);

    private static void ExpectAuto(PointState st, PointEvent e, ulong now, PointState state, params PointEffect[] fx)
    {
        var (s, effects) = A(st, e, now);
        Assert.Equal(state, s);
        Assert.Equal(fx, effects);
    }

    [Fact]
    public void SwitchModeKeepsRecording()
    {
        // Every live state keeps its state (the recording continues) and answers one SwitchMode effect, in both gestures.
        foreach (var st in new[] { Armed(0), Holding(0), Tap1(0, 100), Locked(0), Toggled(0) })
        {
            foreach (var g in new[] { Gesture.HoldLock, Gesture.Automatic })
            {
                var (s, fx) = Step(st, Switch(PointMode.Command), 500, null, g);
                Assert.Equal(st, s);
                Assert.Equal(new PointEffect[] { new PointEffect.SwitchMode(PointMode.Command) }, fx);
            }
        }
        // Outside a gesture the other key's press is its own gesture: the switch is ignored, no effect.
        foreach (var st in new[] { Idle, Sending(0), Replying }) Expect(st, Switch(PointMode.Dictate), 500, st);
        // A hold that switched mid-way still sends at release (the destination is the driver's, by the new mode).
        var (h, _) = S(Idle, Pressed, 0);
        (h, _) = S(h, Tick(HoldMs), HoldMs);
        var (h2, fx2) = S(h, Switch(PointMode.Command), 600);
        Assert.Equal(Holding(0), h2);
        Assert.Equal(new PointEffect[] { new PointEffect.SwitchMode(PointMode.Command) }, fx2);
        Expect(h2, Released, 1500, Sending(0), Stop("release"));
        // A locked gesture switched twice still sends on the next press.
        var (l, _) = S(Locked(0), Switch(PointMode.Ask), 900);
        (l, _) = S(l, Switch(PointMode.Command), 1000);
        Expect(l, Pressed, 2000, Sending(0), Stop("tap"));
    }

    [Fact]
    public void AutomaticTapTogglesHoldTalks()
    {
        Assert.Equal(400UL, AutomaticTapMs);
        Assert.Equal(AutomaticTapMs, HoldThreshold(Gesture.Automatic));
        Assert.Equal(HoldMs, HoldThreshold(Gesture.HoldLock));
        // Press starts as always.
        ExpectAuto(Idle, Pressed, 0, Armed(0), Start);
        // Tap < 400 ms → toggled on (hands-free); the next press stops and sends; a too-short toggle is the short note.
        ExpectAuto(Armed(0), Released, AutomaticTapMs - 1, Toggled(0), ShowLock);
        ExpectAuto(Toggled(0), Pressed, 2000, Sending(0), Stop("tap"));
        ExpectAuto(Toggled(0), Pressed, MinNoteMs - 1, Idle, Discard("short", AutomaticHint));
        // While toggled, key-ups and ticks change nothing; Esc discards with the Automatic hint; Cancel discards quietly.
        foreach (var e in new[] { Released, Tick(5000), PanelDone }) ExpectAuto(Toggled(0), e, 5000, Toggled(0));
        ExpectAuto(Toggled(0), Esc, 900, Idle, Discard("esc", AutomaticHint));
        ExpectAuto(Toggled(0), Cancel, 900, Idle, Discard("cancel", null));
        // Hold ≥ 400 ms = push-to-talk: the tick moves to Holding only at 400 ms, release sends (or discards a short note).
        ExpectAuto(Armed(0), Tick(HoldMs), HoldMs, Armed(0));
        ExpectAuto(Armed(0), Tick(AutomaticTapMs), AutomaticTapMs, Holding(0));
        ExpectAuto(Holding(0), Released, 1200, Sending(0), Stop("release"));
        ExpectAuto(Holding(0), Released, MinNoteMs - 1, Idle, Discard("short", AutomaticHint));
        // A late tick: the release itself decides (≥ 400 ms is a hold).
        ExpectAuto(Armed(0), Released, 1000, Sending(0), Stop("release"));
        ExpectAuto(Armed(0), Released, AutomaticTapMs + 10, Idle, Discard("short", AutomaticHint));
        Assert.True(Toggled(1).IsGesture);
        Assert.Equal("""{"state":"toggled","t0":7}""", Toggled(7).Json);
        // The rest of the machine is shared: Sending → PanelDone → Replying → Esc conceals.
        ExpectAuto(Sending(0), PanelDone, 3000, Replying);
        ExpectAuto(Replying, Esc, 3100, Idle, Conceal);
    }

    [Fact]
    public void AutomaticNoDoubleTapLock()
    {
        // A tap never enters Tap1 in Automatic, so a quick second press cannot lock: it ends the toggled note.
        var (st, fx) = A(Idle, Pressed, 0);
        (st, fx) = A(st, Released, 100);
        Assert.Equal(Toggled(0), st);
        Assert.Equal(new[] { ShowLock }, fx);
        (st, fx) = A(st, Pressed, 250);
        Assert.Equal(Idle, st);
        Assert.Equal(new[] { Discard("short", AutomaticHint) }, fx);
        Assert.DoesNotContain(fx, f => f is PointEffect.ShowLock);
        // The same taps under the default gesture still lock (the 149 grammar is untouched).
        var (h, _) = S(Idle, Pressed, 0);
        (h, _) = S(h, Released, 100);
        Assert.Equal(Tap1(0, 100), h);
        (h, fx) = S(h, Pressed, 250);
        Assert.Equal(Locked(0), h);
        Assert.Equal(new[] { ShowLock }, fx);
        // Follow-up (§10.5): a press while the answer is being worked on opens a new gesture only when the driver asks for it.
        Expect(Sending(0), Pressed, 50, Sending(0));
        var (f1, ffx) = Step(Sending(0), Pressed, 5000, null, Gesture.HoldLock, followUp: true);
        Assert.Equal(Armed(5000), f1);
        Assert.Equal(new[] { Start }, ffx);
        var (f2, ffx2) = Step(Sending(0), Pressed, 5000, "meeting", Gesture.Automatic, followUp: true);
        Assert.Equal(Sending(0), f2);
        Assert.Equal(new PointEffect[] { new PointEffect.Refuse("meeting") }, ffx2);
    }
}
