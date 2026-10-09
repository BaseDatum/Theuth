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
}
