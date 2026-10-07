namespace Theuth.Companion.Core;

// 130 §5.5 breakpoint sources and spike SB §1 (the double-tap seed); 134 §5.4 / W3: the Windows sources
// (SetWinEventHook(EVENT_SYSTEM_FOREGROUND) + GetLastInputInfo sampled at 1 Hz; the WH_KEYBOARD_LL hook) only feed them.

/// <summary>App switch or ≥ 20 s typing pause → one breakpoint each. The cap timer is the policy's own <c>Tick</c>.</summary>
public sealed class BreakpointDetector
{
    public abstract record Signal
    {
        private Signal() { }

        /// <summary>The new foreground app's identity (Mac: bundle id; Windows: the foreground process, e.g. its exe path).</summary>
        public sealed record AppActivated(string? Id) : Signal;

        /// <summary>Seconds since the last input (Mac: IOKit HIDIdleTime; Windows: GetLastInputInfo), sampled at 1 Hz.</summary>
        public sealed record Idle(double Seconds) : Signal;
    }

    /// <summary>Seconds.</summary>
    public const double TypingPause = 20;

    private readonly string? own;
    private string? frontmost;
    private bool seeded;
    private bool idleFired;

    /// <summary><paramref name="own"/>: the companion's own identity — its own panel taking the foreground is not a switch.</summary>
    public BreakpointDetector(string? own) => this.own = own;

    /// <summary>True when the signal is a breakpoint.</summary>
    public bool Feed(Signal s)
    {
        switch (s)
        {
            case Signal.AppActivated a:
            {
                if (a.Id == own && own is not null) return false;
                bool result = seeded && a.Id != frontmost;
                frontmost = a.Id;
                seeded = true;
                return result;
            }
            case Signal.Idle i:
                if (i.Seconds >= TypingPause)
                {
                    if (idleFired) return false;
                    idleFired = true;
                    return true;
                }
                idleFired = false;
                return false;
            default:
                return false;
        }
    }
}

/// <summary>
/// Device-independent modifier state; the Windows source maps the keyboard hook's <c>VK_*SHIFT</c>, <c>VK_*CONTROL</c>,
/// <c>VK_*MENU</c> (Alt) and <c>VK_*WIN</c> onto it (the Mac's ⌥ and ⌘ bits; Function stays for parity).
/// </summary>
[Flags]
public enum Modifiers : byte
{
    None = 0,
    Shift = 1 << 0,
    Control = 1 << 1,
    Alt = 1 << 2,
    Win = 1 << 3,
    Function = 1 << 4,
}

/// <summary>
/// Spike SB's detector, Windows form (design 134 §3: double-tap Alt, the analogue of ⌥⌥): Alt pressed ALONE and
/// released twice, the second press within <see cref="Window"/> of the first release, each hold ≤ <see cref="MaxHold"/>,
/// nothing else in between. Identical consecutive states (auto-repeat) are ignored. Feed every modifier change;
/// <see cref="Cancel"/> on any key-down / mouse-down.
/// </summary>
public sealed class DoubleTapDetector
{
    private enum Phase { Idle, FirstDown, FirstUp, SecondDown }

    /// <summary>Seconds.</summary>
    public double Window { get; set; } = 0.300;

    /// <summary>Seconds.</summary>
    public double MaxHold { get; set; } = 0.300;

    private Phase phase = Phase.Idle;
    private double phaseAt;
    private Modifiers last = Modifiers.None;

    /// <summary>True exactly once, on the release that completes the second tap. <paramref name="t"/> in seconds.</summary>
    public bool Feed(Modifiers mods, double t)
    {
        if (mods == last) return false;
        var previous = last;
        last = mods;
        bool altAlone = mods == Modifiers.Alt;
        bool released = mods == Modifiers.None;
        switch (phase)
        {
            case Phase.Idle:
                if (altAlone && previous == Modifiers.None) Enter(Phase.FirstDown, t);
                else phase = Phase.Idle;
                break;
            case Phase.FirstDown:
                if (released && t - phaseAt <= MaxHold) Enter(Phase.FirstUp, t);
                else phase = Phase.Idle;
                break;
            case Phase.FirstUp:
                if (altAlone && previous == Modifiers.None) Enter(t - phaseAt <= Window ? Phase.SecondDown : Phase.FirstDown, t);
                else phase = Phase.Idle;
                break;
            case Phase.SecondDown:
                var down = phaseAt;
                phase = Phase.Idle;
                if (released && t - down <= MaxHold) return true;
                break;
        }
        return false;
    }

    public void Cancel() => phase = Phase.Idle;

    public bool IsIdle => phase == Phase.Idle;

    private void Enter(Phase p, double t)
    {
        phase = p;
        phaseAt = t;
    }
}
