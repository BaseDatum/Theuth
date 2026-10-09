namespace Theuth.Companion.Core;

/// <summary>
/// Design 149 D12 / 96 §4.1: the voice key's gesture machine — hold to talk, double-tap to lock — as a pure reducer over
/// the hotkey's <c>Pressed</c>/<c>Released</c> stream, Esc, the answer card's <c>PanelDone</c> and timer ticks. A port of
/// <c>crates/desktop/src/point/machine.rs</c> with the same constants and the same tests; no clock, no I/O — the driver
/// (<c>PointAndSay/PointController</c>) owns the timers and executes the <see cref="PointEffect"/>s.
/// </summary>
public static class PointMachine
{
    /// <summary>Release before this many ms is a tap, not a hold.</summary>
    public const ulong HoldMs = 200;

    /// <summary>A second press within this many ms of a tap's release locks hands-free.</summary>
    public const ulong DoubleTapMs = 300;

    /// <summary>Notes shorter than this are discarded with <see cref="Hint"/> (89's <c>MIN_NOTE_MS</c>).</summary>
    public const ulong MinNoteMs = 800;

    /// <summary>Pointer sampler rate (96 §4.2).</summary>
    public const ulong SampleHz = 60;

    /// <summary>How long the answer card's <c>done</c> phase lingers before it reports <c>PanelDone</c>.</summary>
    public const ulong DoneMs = 6_000;

    /// <summary>Shown on a missed double-tap, a too-short note, and Esc mid-gesture.</summary>
    public const string Hint = "Hold to talk · double-tap to lock";

    /// <summary>
    /// One transition. <paramref name="refused"/> is the standing refusal reason (a meeting is talking, dictation is busy
    /// elsewhere), answered instead of <see cref="PointEffect.Start"/> on a press from Idle/Replying. Times are ms on the
    /// driver's clock; <c>T0</c> is the press that opened the gesture.
    /// </summary>
    public static (PointState State, IReadOnlyList<PointEffect> Effects) Step(PointState s, PointEvent e, ulong now, string? refused = null)
    {
        var stay = (s, (IReadOnlyList<PointEffect>)Array.Empty<PointEffect>());
        switch (s, e)
        {
            case (PointState.Idle or PointState.Replying, PointEvent.Pressed):
                return refused is { } reason
                    ? (s, One(new PointEffect.Refuse(reason)))
                    : (new PointState.Armed(now), One(new PointEffect.Start()));
            case (PointState.Armed a, PointEvent.Released):
                return Sub(now, a.T0) >= HoldMs ? Send(a.T0, now, "release") : (new PointState.Tap1(a.T0, now), None());
            case (PointState.Armed a, PointEvent.Tick) when Sub(now, a.T0) >= HoldMs:
                return (new PointState.Holding(a.T0), None());
            case (PointState.Holding h, PointEvent.Released):
                return Send(h.T0, now, "release");
            case (PointState.Armed or PointState.Holding or PointState.Locked, PointEvent.Esc):
                return Discard("esc", Hint);
            case (PointState.Tap1 or PointState.Sending, PointEvent.Esc):
                return Discard("esc", null);
            case (PointState.Armed or PointState.Holding or PointState.Tap1 or PointState.Locked, PointEvent.Cancel):
                return Discard("cancel", null);
            case (PointState.Tap1 t, PointEvent.Pressed):
                if (Sub(now, t.Released) <= DoubleTapMs) return (new PointState.Locked(t.T0), One(new PointEffect.ShowLock()));
                // The tick that should have closed the window has not landed yet: close it and treat this press as a
                // fresh gesture rather than swallow it.
                return (new PointState.Armed(now), new PointEffect[] { new PointEffect.Discard("double-tap-missed", Hint), new PointEffect.Start() });
            case (PointState.Tap1 t, PointEvent.Tick) when Sub(now, t.Released) > DoubleTapMs:
                return Discard("double-tap-missed", Hint);
            case (PointState.Locked l, PointEvent.Pressed):
                return Send(l.T0, now, "tap");
            case (PointState.Sending, PointEvent.PanelDone):
                return (new PointState.Replying(), None());
            case (PointState.Sending, PointEvent.Cancel):
                return (new PointState.Idle(), None());
            case (PointState.Replying, PointEvent.Esc or PointEvent.PanelDone or PointEvent.Cancel):
                return (new PointState.Idle(), One(new PointEffect.Conceal()));
            default:
                return stay;
        }
    }

    /// <summary>A note ends: send it, or discard one shorter than <see cref="MinNoteMs"/>.</summary>
    private static (PointState, IReadOnlyList<PointEffect>) Send(ulong t0, ulong now, string reason) =>
        Sub(now, t0) < MinNoteMs ? Discard("short", Hint) : (new PointState.Sending(t0), One(new PointEffect.Stop(reason)));

    private static (PointState, IReadOnlyList<PointEffect>) Discard(string reason, string? hint) =>
        (new PointState.Idle(), One(new PointEffect.Discard(reason, hint)));

    private static ulong Sub(ulong a, ulong b) => a >= b ? a - b : 0; // saturating_sub

    private static IReadOnlyList<PointEffect> One(PointEffect e) => new[] { e };

    private static IReadOnlyList<PointEffect> None() => Array.Empty<PointEffect>();
}

/// <summary>The gesture's state (<c>machine.rs</c> <c>State</c>); <see cref="Json"/> is its tagged snake_case form.</summary>
public abstract record PointState
{
    private PointState() { }

    public sealed record Idle : PointState;

    public sealed record Armed(ulong T0) : PointState;

    public sealed record Holding(ulong T0) : PointState;

    public sealed record Tap1(ulong T0, ulong Released) : PointState;

    public sealed record Locked(ulong T0) : PointState;

    public sealed record Sending(ulong T0) : PointState;

    public sealed record Replying : PointState;

    /// <summary>A gesture is live: overlays up, dictation running (Armed · Holding · Tap1 · Locked).</summary>
    public bool IsGesture => this is Armed or Holding or Tap1 or Locked;

    /// <summary>The <c>serde(tag = "state", rename_all = "snake_case")</c> form the shell's <c>point_state</c> emits.</summary>
    public string Json => this switch
    {
        Idle => """{"state":"idle"}""",
        Armed a => $$"""{"state":"armed","t0":{{a.T0}}}""",
        Holding h => $$"""{"state":"holding","t0":{{h.T0}}}""",
        Tap1 t => $$"""{"state":"tap1","t0":{{t.T0}},"released":{{t.Released}}}""",
        Locked l => $$"""{"state":"locked","t0":{{l.T0}}}""",
        Sending s => $$"""{"state":"sending","t0":{{s.T0}}}""",
        Replying => """{"state":"replying"}""",
        _ => """{"state":"idle"}""",
    };
}

/// <summary>What the driver feeds the machine.</summary>
public abstract record PointEvent
{
    private PointEvent() { }

    public sealed record Pressed : PointEvent;

    public sealed record Released : PointEvent;

    public sealed record Esc : PointEvent;

    /// <summary>A timer the driver armed on entering Armed / Tap1; <see cref="Now"/> is the time it was armed for. The
    /// reducer judges by the clock it is stepped with, so a late tick is still correct.</summary>
    public sealed record Tick(ulong Now) : PointEvent;

    public sealed record PanelDone : PointEvent;

    public sealed record Cancel : PointEvent;
}

/// <summary>What the driver must do after a step.</summary>
public abstract record PointEffect
{
    private PointEffect() { }

    /// <summary>Overlays + voice mode + dictation at 0 ms.</summary>
    public sealed record Start : PointEffect;

    public sealed record ShowLock : PointEffect;

    /// <summary><see cref="Reason"/> = <c>release</c> | <c>tap</c>.</summary>
    public sealed record Stop(string Reason) : PointEffect;

    /// <summary><see cref="Reason"/> = <c>esc</c> | <c>short</c> | <c>double-tap-missed</c> | <c>cancel</c>.</summary>
    public sealed record Discard(string Reason, string? Hint) : PointEffect;

    public sealed record Conceal : PointEffect;

    public sealed record Refuse(string Reason) : PointEffect;
}
