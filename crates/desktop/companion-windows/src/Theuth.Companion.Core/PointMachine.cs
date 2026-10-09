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
    /// 153 §12.6 / D14 (technique A:L170): with <see cref="Gesture.Automatic"/> a release before this many ms toggles hands-free
    /// recording on (press again to stop); a longer press is push-to-talk. There is no double-tap lock in that mode.
    /// </summary>
    public const ulong AutomaticTapMs = 400;

    /// <summary>The hint of the <see cref="Gesture.Automatic"/> gesture (a too-short note, Esc mid-gesture).</summary>
    public const string AutomaticHint = "Tap to toggle · hold to talk";

    /// <summary>When an Armed press becomes a hold: <see cref="HoldMs"/> (hold · double-tap locks) or <see cref="AutomaticTapMs"/>.</summary>
    public static ulong HoldThreshold(Gesture gesture) => gesture == Gesture.Automatic ? AutomaticTapMs : HoldMs;

    /// <summary>The hint for <paramref name="gesture"/>.</summary>
    public static string HintFor(Gesture gesture) => gesture == Gesture.Automatic ? AutomaticHint : Hint;

    /// <summary>
    /// One transition. <paramref name="refused"/> is the standing refusal reason (a meeting is talking, dictation is busy
    /// elsewhere), answered instead of <see cref="PointEffect.Start"/> on a press from Idle/Replying. Times are ms on the
    /// driver's clock; <c>T0</c> is the press that opened the gesture.
    /// <para>153 (wave C): <paramref name="gesture"/> picks the press grammar (<see cref="Gesture.HoldLock"/>, the 149 machine
    /// unchanged, or <see cref="Gesture.Automatic"/>, §12.6); <paramref name="followUp"/> lets a press while the answer is still
    /// being worked on (<see cref="PointState.Sending"/>) open a follow-up gesture (§10.5 / §12.2 "hold to follow up") instead of
    /// being ignored. <see cref="PointEvent.SwitchMode"/> (§10.1, R2): another mode's key while a gesture is live leaves the
    /// state as it is — the recording continues — and answers <see cref="PointEffect.SwitchMode"/> so the driver relabels and
    /// changes the destination at release; outside a live gesture it is ignored (that press is the other key's own gesture).</para>
    /// </summary>
    public static (PointState State, IReadOnlyList<PointEffect> Effects) Step(PointState s, PointEvent e, ulong now, string? refused = null,
        Gesture gesture = Gesture.HoldLock, bool followUp = false)
    {
        var stay = (s, (IReadOnlyList<PointEffect>)Array.Empty<PointEffect>());
        if (e is PointEvent.SwitchMode sw) return s.IsGesture ? (s, One(new PointEffect.SwitchMode(sw.To))) : stay;
        if (gesture == Gesture.Automatic && Automatic(s, e, now) is { } auto) return auto;
        if (followUp && s is PointState.Sending && e is PointEvent.Pressed)
        {
            return refused is { } why ? (s, One(new PointEffect.Refuse(why))) : (new PointState.Armed(now), One(new PointEffect.Start()));
        }
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

    /// <summary>
    /// §12.6's two extra transitions and the Automatic press grammar: Armed → <see cref="PointState.Toggled"/> on a release before
    /// <see cref="AutomaticTapMs"/> (no Tap1, so no double-tap lock), Armed → Holding at <see cref="AutomaticTapMs"/>, Toggled →
    /// Sending on the next press; Esc and short notes carry <see cref="AutomaticHint"/>. Everything else falls through to the
    /// shared table (null).
    /// </summary>
    private static (PointState, IReadOnlyList<PointEffect>)? Automatic(PointState s, PointEvent e, ulong now)
    {
        switch (s, e)
        {
            case (PointState.Armed a, PointEvent.Released):
                return Sub(now, a.T0) < AutomaticTapMs
                    ? (new PointState.Toggled(a.T0), One(new PointEffect.ShowLock()))
                    : Send(a.T0, now, "release", AutomaticHint);
            case (PointState.Armed a, PointEvent.Tick):
                return Sub(now, a.T0) >= AutomaticTapMs ? (new PointState.Holding(a.T0), None()) : (s, None());
            case (PointState.Holding h, PointEvent.Released):
                return Send(h.T0, now, "release", AutomaticHint);
            case (PointState.Toggled t, PointEvent.Pressed):
                return Send(t.T0, now, "tap", AutomaticHint);
            case (PointState.Armed or PointState.Holding or PointState.Toggled, PointEvent.Esc):
                return Discard("esc", AutomaticHint);
            case (PointState.Toggled, PointEvent.Cancel):
                return Discard("cancel", null);
            case (PointState.Toggled, _):
                return (s, None());
            default:
                return null;
        }
    }

    /// <summary>A note ends: send it, or discard one shorter than <see cref="MinNoteMs"/>.</summary>
    private static (PointState, IReadOnlyList<PointEffect>) Send(ulong t0, ulong now, string reason, string hint = Hint) =>
        Sub(now, t0) < MinNoteMs ? Discard("short", hint) : (new PointState.Sending(t0), One(new PointEffect.Stop(reason)));

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

    /// <summary>153 §12.6 <c>recordingToggled</c>: the Automatic gesture's hands-free recording after a tap; the next press sends.</summary>
    public sealed record Toggled(ulong T0) : PointState;

    /// <summary>A gesture is live: overlays up, dictation running (Armed · Holding · Tap1 · Locked · Toggled).</summary>
    public bool IsGesture => this is Armed or Holding or Tap1 or Locked or Toggled;

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
        Toggled t => $$"""{"state":"toggled","t0":{{t.T0}}}""",
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

    /// <summary>153 §10.1 (R2): another mode's key went down while this gesture is live — switch the gesture to <see cref="To"/>.</summary>
    public sealed record SwitchMode(PointMode To) : PointEvent;
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

    /// <summary>153 §10.1: relabel the live gesture (the pill says the new mode) and deliver to <see cref="To"/>'s destination at release.</summary>
    public sealed record SwitchMode(PointMode To) : PointEffect;
}

/// <summary>
/// 153 §10.1 / §14.4: what a gesture is for — <see cref="Ask"/> (the voice key, point and say → theuth's desk),
/// <see cref="Dictate"/> (dictate anywhere → the focused app, §13) or <see cref="Command"/> (the command key → the desk under the
/// command profile, §10). The mode belongs to the driver's gesture, not to <see cref="PointState"/>: a switch keeps the state.
/// </summary>
public enum PointMode
{
    Ask,
    Dictate,
    Command,
}

/// <summary>153 §12.6 / §3.5: the press grammar of one mode's key. <see cref="HoldLock"/> is the default (W-i).</summary>
public enum Gesture
{
    /// <summary>Hold ≥ 200 ms talks, release acts; tap then press within 300 ms locks hands-free (the 96/149 machine).</summary>
    HoldLock,

    /// <summary>Tap &lt; 400 ms toggles (press again to stop); a longer press is push-to-talk; no double-tap lock.</summary>
    Automatic,
}

/// <summary>The words of the modes, as the pill, the logs and the settings store write them.</summary>
public static class PointModeText
{
    /// <summary><c>ask</c> · <c>dictate</c> · <c>command</c> (the §10.1 <c>Mode</c> raw values).</summary>
    public static string RawValue(this PointMode m) => m switch
    {
        PointMode.Dictate => "dictate",
        PointMode.Command => "command",
        _ => "ask",
    };

    /// <summary>The pill's label: "Ask · theuth desk", "Dictate anywhere", "Command · theuth desk" (§3.4).</summary>
    public static string PillLabel(this PointMode m) => m switch
    {
        PointMode.Dictate => "Dictate anywhere",
        PointMode.Command => "Command · theuth desk",
        _ => "theuth's desk",
    };

    /// <summary><c>holdLock</c> · <c>automatic</c> (the §14.3 <c>gesture</c> setting's values).</summary>
    public static string RawValue(this Gesture g) => g == Gesture.Automatic ? "automatic" : "holdLock";

    public static Gesture GestureFromRaw(string? raw) => raw == "automatic" ? Gesture.Automatic : Gesture.HoldLock;

    /// <summary>The Settings picker's words (§15 row "Gesture: Hold · double-tap locks / Automatic").</summary>
    public static string Title(this Gesture g) => g == Gesture.Automatic ? "Automatic — tap toggles, hold talks" : "Hold · double-tap locks";
}

/// <summary>
/// 153 PIN-C: one gesture of a mode key as the hotkey layer reports it (<c>Hotkeys.DictateAnywhere</c> / <c>Hotkeys.CommandKey</c>),
/// after the ONE <see cref="PointMachine"/> the companion runs for the voice key, Dictate anywhere and Command (hold ≥ 200 ms /
/// double-tap lock or Automatic, Esc, the 800 ms short-note rule, the mode switch). <see cref="Id"/> numbers the gesture; every
/// later record of the same gesture carries it.
/// </summary>
public abstract record ModeGesture(int Id, PointMode Mode)
{
    /// <summary>The press opened a gesture in <see cref="ModeGesture.Mode"/> (mic at press, §8.5): the subscriber starts its capture.</summary>
    public sealed record Started(int Id, PointMode Mode, ulong AtMs) : ModeGesture(Id, Mode);

    /// <summary>Hands-free: the double-tap lock or the Automatic toggle; the next press of the key stops the gesture.</summary>
    public sealed record Locked(int Id, PointMode Mode) : ModeGesture(Id, Mode);

    /// <summary>The release of a hold, or the tap that ends a locked/toggled gesture: act now. <see cref="Reason"/> = <c>release</c> | <c>tap</c>.</summary>
    public sealed record Stopped(int Id, PointMode Mode, string Reason, ulong HeldMs) : ModeGesture(Id, Mode);

    /// <summary>Esc, a note under 800 ms, a missed double-tap, a cancel or a chorded modifier-only hold: drop everything (show <see cref="Hint"/> when set).</summary>
    public sealed record Discarded(int Id, PointMode Mode, string Reason, string? Hint) : ModeGesture(Id, Mode);

    /// <summary>
    /// §10.1: the live gesture changed mode. Into <see cref="PointMode.Dictate"/>, <see cref="Words"/> are what the companion heard so far
    /// (prefix them); out of it, the subscriber stops its capture WITHOUT delivering and hands its words back through
    /// <c>Hotkeys.TakeDictateWords</c>.
    /// </summary>
    public sealed record Switched(int Id, PointMode From, PointMode To, string Words) : ModeGesture(Id, To);

    /// <summary>The press was refused (a meeting is talking, not connected, dictation busy): nothing started.</summary>
    public sealed record Refused(int Id, PointMode Mode, string Reason) : ModeGesture(Id, Mode);
}
