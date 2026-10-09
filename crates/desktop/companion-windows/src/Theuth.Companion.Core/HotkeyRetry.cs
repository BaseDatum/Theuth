namespace Theuth.Companion.Core;

/// <summary>A global row whose saved chord another app held when the table was applied: what to retry and the notice to keep if the retry fails.</summary>
public sealed record HotkeyFailure(KeyAction Action, Chord Chord, string Notice);

/// <summary>
/// 149 recheck D1: the companion restarted while the shell was up loses the voice key — the shell re-binds the chord the
/// moment the companion exits and only lets go again on its next 2 s mutex poll (<c>crates/desktop/src/companion/windows.rs</c>
/// POLL → <c>App.tsx useVoiceKeyBinding</c>), so <c>RegisterHotKey</c> at startup answers 0 and nothing re-applied the table.
/// This is the retry: every <see cref="Interval"/> for up to <see cref="Window"/> after the apply, each failed row is tried again;
/// a row that registers leaves the set, and a row still failing when the window closes keeps its notice (a genuine
/// third-party holder is not fought). Pure and clock-fed so the Mac tests cover it; the app's <c>Hotkeys</c> owns the timer.
/// </summary>
public sealed class HotkeyRetry
{
    public static readonly TimeSpan Interval = TimeSpan.FromSeconds(1);
    public static readonly TimeSpan Window = TimeSpan.FromSeconds(20);

    private readonly List<HotkeyFailure> pending;

    public HotkeyRetry(IEnumerable<HotkeyFailure> failures, DateTimeOffset now)
    {
        pending = failures.ToList();
        StartedAt = now;
    }

    public DateTimeOffset StartedAt { get; }

    /// <summary>Rows still unregistered.</summary>
    public IReadOnlyList<HotkeyFailure> Pending => pending;

    /// <summary>Attempts made so far (the startup apply is not one).</summary>
    public int Attempts { get; private set; }

    /// <summary>The window closed with rows still pending: their notices stand.</summary>
    public bool GaveUp { get; private set; }

    /// <summary>Something left to try and time to try it.</summary>
    public bool Active => pending.Count > 0 && !GaveUp;

    /// <summary>How long until the next attempt; null when there is nothing (left) to do.</summary>
    public TimeSpan? NextDelay(DateTimeOffset now)
    {
        if (!Active) return null;
        var remaining = StartedAt + Window - now;
        if (remaining <= TimeSpan.Zero) return null;
        return remaining < Interval ? remaining : Interval;
    }

    /// <summary>
    /// One attempt at <paramref name="now"/>: every pending row through <paramref name="register"/> (true = it registered now).
    /// Past the window nothing is tried and the retry gives up. Returns the rows that registered this time.
    /// </summary>
    public IReadOnlyList<HotkeyFailure> Attempt(DateTimeOffset now, Func<HotkeyFailure, bool> register)
    {
        if (!Active) return Array.Empty<HotkeyFailure>();
        if (now - StartedAt > Window)
        {
            GaveUp = true;
            return Array.Empty<HotkeyFailure>();
        }
        Attempts++;
        var won = new List<HotkeyFailure>();
        foreach (var f in pending.ToList())
        {
            if (!register(f)) continue;
            pending.Remove(f);
            won.Add(f);
        }
        if (pending.Count > 0 && now - StartedAt >= Window) GaveUp = true;
        return won;
    }

    /// <summary>Settings › Keys or the meeting gate re-applied the table: this retry is over (the new apply starts its own).</summary>
    public void Cancel()
    {
        pending.Clear();
    }

    /// <summary>
    /// The Settings › Keys / legend line: while retrying "Ctrl+Alt+Space is held by another app — retrying for up to 20 s",
    /// after giving up the row's own notice; null once every row registered.
    /// </summary>
    public string? Notice => pending.Count == 0
        ? null
        : string.Join(" · ", pending.Select(f => GaveUp ? f.Notice : $"{f.Chord.Symbol} is held by another app — retrying for up to {(int)Window.TotalSeconds} s"));
}
