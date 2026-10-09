namespace Theuth.Companion.Core;

/// <summary>What the controller must do with an engine dictation id the start call just returned.</summary>
public enum LeaseVerdict
{
    /// <summary>The gesture is still open: keep the id, it is this gesture's dictation.</summary>
    Keep,
    /// <summary>The gesture ended (sent, cancelled, Esc, refused, disconnected) or belongs to an older press: stop this id now.</summary>
    Stop,
}

/// <summary>
/// 149 voice-key fix: the one engine dictation a gesture owns, from the <c>dictation_start</c> call to the stop. The invariant
/// (b): every finish/abort path stops the dictation the gesture started — including when the start is still in flight, which is
/// how the Mac got stuck on "dictating elsewhere" (a start that returned after the gesture was over was kept, never stopped).
/// Pure and generation-counted so the Mac tests cover it; the app's <c>PointController</c> owns the async calls.
/// <list type="bullet">
/// <item><see cref="Begin"/> opens a lease (a new generation) when the gesture starts and the start call is issued.</item>
/// <item><see cref="Started"/> is the start call's answer: <see cref="LeaseVerdict.Keep"/> while that generation is still open,
/// <see cref="LeaseVerdict.Stop"/> when it was closed meanwhile or the answer is from an older press.</item>
/// <item><see cref="StartFailed"/>: true when the refusal concerns the open lease (the gesture must cancel), false for a stale one.</item>
/// <item><see cref="Close"/> ends the lease on every path and hands back the id to stop (null when none is live yet — a late
/// <see cref="Started"/> then answers Stop). Idempotent: a second close returns null.</item>
/// </list>
/// </summary>
public sealed class DictationLease
{
    public enum Phase { Idle, Starting, Live, Closed }

    public Phase State { get; private set; } = Phase.Idle;

    /// <summary>The live dictation's id (Phase.Live only).</summary>
    public string? Id { get; private set; }

    /// <summary>The open lease's generation; a start answer carrying another one is stale.</summary>
    public int Generation { get; private set; }

    public bool Starting => State == Phase.Starting;

    public bool Live => State == Phase.Live;

    /// <summary>A gesture starts and its <c>dictation_start</c> is issued: returns the generation to hand to the start call.</summary>
    public int Begin()
    {
        Generation++;
        State = Phase.Starting;
        Id = null;
        return Generation;
    }

    /// <summary>The start call for generation <paramref name="generation"/> returned <paramref name="id"/>.</summary>
    public LeaseVerdict Started(int generation, string id)
    {
        if (generation != Generation || State != Phase.Starting) return LeaseVerdict.Stop;
        State = Phase.Live;
        Id = id;
        return LeaseVerdict.Keep;
    }

    /// <summary>The start call for <paramref name="generation"/> was refused: true when it is the open lease (cancel the gesture).</summary>
    public bool StartFailed(int generation)
    {
        if (generation != Generation || State != Phase.Starting) return false;
        State = Phase.Closed;
        Id = null;
        return true;
    }

    /// <summary>
    /// The gesture ends (send, cancel, Esc, refusal, mic failure, disconnect, dispose): the id to stop now, or null when nothing
    /// is live — a start still in flight will be stopped by its own <see cref="Started"/> answer.
    /// </summary>
    public string? Close()
    {
        var id = State == Phase.Live ? Id : null;
        State = Phase.Closed;
        Id = null;
        return id;
    }
}
