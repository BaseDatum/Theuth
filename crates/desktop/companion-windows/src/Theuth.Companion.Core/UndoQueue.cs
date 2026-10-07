namespace Theuth.Companion.Core;

/// <summary>
/// The hold before an answer is sent (130 §5.4): one action held at a time, <see cref="Delay"/> seconds (default 3,
/// settings 0–5), cancelled by Esc, fired exactly once. Pure — the caller supplies <c>now</c>.
/// </summary>
public sealed class UndoQueue<TAction> where TAction : class
{
    public sealed record HeldAction(TAction Action, DateTimeOffset Since);

    public const double DefaultDelay = 3;

    /// <summary>Seconds, clamped to 0–5.</summary>
    public double Delay { get; }

    public HeldAction? Held { get; private set; }

    public UndoQueue(double delay = DefaultDelay)
    {
        Delay = Math.Min(5, Math.Max(0, delay));
    }

    /// <summary>
    /// Hold <paramref name="action"/>. A second keystroke while one is held displaces it: the displaced action is
    /// returned and the caller commits it at once (the receipt row says so).
    /// </summary>
    public TAction? Hold(TAction action, DateTimeOffset now)
    {
        var displaced = Held?.Action;
        Held = new HeldAction(action, now);
        return displaced;
    }

    /// <summary>Esc during the hold. <c>true</c> when something was taken back.</summary>
    public bool Cancel()
    {
        bool had = Held is not null;
        Held = null;
        return had;
    }

    /// <summary>Fires the held action once its delay has elapsed; null otherwise.</summary>
    public TAction? Tick(DateTimeOffset now)
    {
        if (Held is not { } h || now < h.Since.AddSeconds(Delay)) return null;
        Held = null;
        return h.Action;
    }

    public DateTimeOffset? FireAt => Held is { } h ? h.Since.AddSeconds(Delay) : null;

    /// <summary>Seconds left, 0 when nothing is held.</summary>
    public double Remaining(DateTimeOffset now) => FireAt is { } at ? Math.Max(0, (at - now).TotalSeconds) : 0;
}
