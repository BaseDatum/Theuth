using System.Text;
using Theuth.Companion.Rpc;

namespace Theuth.Companion.Core;

/// <summary>Why a catch-up runs (D2). The word is also what the log line carries.</summary>
public static class CatchUpTrigger
{
    /// <summary>A <c>TurnFinished</c> for the desk: the transcript, not the stream, says what the reply was.</summary>
    public const string TurnFinished = "turn-finished";

    /// <summary>The engine said it dropped events (<c>Lagged</c>): any delta in the gap is gone for good.</summary>
    public const string Lagged = "lagged";

    /// <summary>The connection was lost and re-opened while the ask collected: everything in between was missed.</summary>
    public const string Reconnect = "reconnect";

    /// <summary>No event for the desk for <see cref="AskWatchdog.Interval"/>: the stream may be silently dead.</summary>
    public const string Watchdog = "watchdog";
}

public enum CatchUpAction
{
    /// <summary>Nothing final yet (a turn is running, or the transcript could not be read for a non-final trigger).</summary>
    KeepCollecting,

    /// <summary>The ask is over; <see cref="CatchUpDecision.Text"/> is the reply and replaces whatever streamed.</summary>
    Finish,

    /// <summary>The ask is over without a reply: <see cref="CatchUpDecision.Error"/> is why (an error row of the transcript), or <see cref="CatchUpDecision.NoReply"/>.</summary>
    Fail,

    /// <summary>The transcript does not show the ask yet and nothing streamed: look once more shortly (the turn may not have started).</summary>
    Settle,
}

/// <summary>What <see cref="AskCatchUp.Pick"/> found in the transcript window.</summary>
/// <param name="Text">The ask's reply (the last assistant entry of its turn), null when none is stored.</param>
/// <param name="Error">An <c>error</c> row after the ask's own user row, null when none.</param>
/// <param name="UserRowFound">The ask's own user row is inside the window.</param>
/// <param name="Turn">The ask's turn id (the seq of its opening user row) once known.</param>
/// <param name="Via">How the entry was matched: <c>turn</c> · <c>text</c> · <c>newest</c> · <c>none</c>.</param>
public sealed record CatchUpPick(string? Text, string? Error, bool UserRowFound, long? Turn, string Via);

/// <summary>What one catch-up decided (D2).</summary>
public sealed record CatchUpDecision(CatchUpAction Action, string? Text, string? Error, string Via, long? Turn, bool Running, bool NoReply = false);

/// <summary>
/// 189w D2: catching an Ask reply up from the transcript. The live stream can lose text — a focus set that excluded the desk, a
/// <c>Lagged</c> gap, a reconnect — and a <c>TurnFinished</c> that arrives with no streamed text says nothing about what the desk
/// answered. <c>load_session</c> is the only RPC that returns a turn's final text (<c>crates/rpc/src/api.rs:552-558</c>), so after any
/// <c>TurnFinished</c> for the desk, a <c>Lagged</c>, a reconnect, and after <see cref="AskWatchdog.Interval"/> of silence, the Store reads
/// the last <see cref="Limit"/> entries and asks this class what they mean. Pure: the Store makes the call, owns the timers, applies the answer.
/// </summary>
public static class AskCatchUp
{
    /// <summary>How many transcript entries a catch-up reads ("about 8": the user row, the tool cards and the reply of a short turn).</summary>
    public const uint Limit = 8;

    /// <summary>How long a catch-up waits for <c>load_session</c> before it counts as unreadable (the ask must never wait on a hung call).</summary>
    public static readonly TimeSpan Deadline = TimeSpan.FromSeconds(10);

    /// <summary>The one re-check of a transcript that does not show the ask yet (<see cref="CatchUpAction.Settle"/>).</summary>
    public static readonly TimeSpan SettleDelay = TimeSpan.FromMilliseconds(1500);

    /// <summary>What the receipt says when the turn finished and the transcript holds no assistant text for it.</summary>
    public const string NoReplyText = "theuth finished without a reply";

    /// <summary>Whitespace runs → one space, trimmed (the engine stores the text as typed; the field may have trailing blanks).</summary>
    public static string Normalize(string? s)
    {
        if (string.IsNullOrEmpty(s)) return "";
        var sb = new StringBuilder(s.Length);
        bool gap = false;
        foreach (var c in s)
        {
            if (char.IsWhiteSpace(c))
            {
                gap = sb.Length > 0;
                continue;
            }
            if (gap) sb.Append(' ');
            gap = false;
            sb.Append(c);
        }
        return sb.ToString();
    }

    /// <summary>
    /// The last assistant entry after the ask's own user row. Match by turn id when it is known (assistant rows carry the seq of the user
    /// row that opened their turn), else by the latest user row with the ask's text (then its seq becomes the turn id), else — nothing
    /// identifies the ask — the newest assistant entry. A blank entry is no text. An <c>error</c> row after the user row is reported
    /// when no assistant text is stored (the transcript replays a failed turn as <c>HistoryEntry::Error</c>).
    /// </summary>
    public static CatchUpPick Pick(IReadOnlyList<TranscriptEntry> history, string askText, long? turn)
    {
        int user = -1;
        if (turn is { } known)
        {
            for (int i = history.Count - 1; i >= 0; i--)
            {
                if (history[i].Role == "user" && history[i].Turn == known) { user = i; break; }
            }
        }
        else
        {
            var want = Normalize(askText);
            if (want.Length > 0)
            {
                for (int i = history.Count - 1; i >= 0; i--)
                {
                    if (history[i].Role == "user" && Normalize(history[i].Text) == want) { user = i; break; }
                }
            }
        }
        long? id = turn ?? (user >= 0 ? history[user].Turn : null);

        // The entries after the user row, up to the next row that opens another turn (a steer row has no seq and belongs to this one).
        int end = history.Count;
        if (user >= 0)
        {
            for (int i = user + 1; i < history.Count; i++)
            {
                if (history[i].Role == "user" && history[i].Turn is { } other && other != id) { end = i; break; }
            }
        }

        string? text = null;
        string via = "none";
        if (id is { } t)
        {
            for (int i = history.Count - 1; i >= 0 && text is null; i--)
            {
                if (history[i].Role == "assistant" && history[i].Turn == t && !string.IsNullOrWhiteSpace(history[i].Text))
                {
                    text = history[i].Text!.Trim();
                    via = turn is not null ? "turn" : "text";
                }
            }
        }
        if (text is null && user >= 0)
        {
            for (int i = end - 1; i > user && text is null; i--)
            {
                if (history[i].Role == "assistant" && !string.IsNullOrWhiteSpace(history[i].Text))
                {
                    text = history[i].Text!.Trim();
                    via = "text";
                }
            }
        }
        if (text is null && user < 0 && turn is null)
        {
            for (int i = history.Count - 1; i >= 0 && text is null; i--)
            {
                if (history[i].Role == "assistant" && !string.IsNullOrWhiteSpace(history[i].Text))
                {
                    text = history[i].Text!.Trim();
                    via = "newest";
                }
            }
        }

        string? error = null;
        if (text is null && user >= 0)
        {
            for (int i = end - 1; i > user && error is null; i--)
            {
                if (history[i].Role == "error" && !string.IsNullOrWhiteSpace(history[i].Text)) error = history[i].Text!.Trim();
            }
        }
        return new CatchUpPick(text, error, user >= 0, id, via);
    }

    /// <summary>
    /// What a catch-up means. <paramref name="tail"/> is null when the transcript could not be read (an error, the deadline, an engine
    /// without <c>load_session</c>); <paramref name="streamed"/> is the text collected from the stream; <paramref name="turn"/> the ask's
    /// turn id when known (<c>TurnStarted.turn_seq</c> or an earlier match); <paramref name="settled"/> marks the one re-check.
    /// A running turn keeps the ask collecting. A finished one never leaves it waiting: its text, an error row, the streamed text, or
    /// <see cref="NoReplyText"/> — except the single <see cref="CatchUpAction.Settle"/> when the transcript simply has not caught up
    /// with the ask (its user row is missing and nothing streamed).
    /// </summary>
    public static CatchUpDecision Decide(string trigger, SessionTail? tail, string askText, long? turn, string streamed, bool settled)
    {
        bool streamedAny = !string.IsNullOrWhiteSpace(streamed);
        if (tail is null)
        {
            if (trigger != CatchUpTrigger.TurnFinished) return new CatchUpDecision(CatchUpAction.KeepCollecting, null, null, "unread", turn, false);
            return streamedAny
                ? new CatchUpDecision(CatchUpAction.Finish, streamed.Trim(), null, "stream", turn, false)
                : new CatchUpDecision(CatchUpAction.Fail, null, null, "unread", turn, false, NoReply: true);
        }
        var pick = Pick(tail.History, askText, turn);
        if (tail.Running) return new CatchUpDecision(CatchUpAction.KeepCollecting, null, null, pick.Via, pick.Turn, true);
        if (pick.Text is { } text) return new CatchUpDecision(CatchUpAction.Finish, text, null, pick.Via, pick.Turn, false);
        if (pick.Error is { } error) return new CatchUpDecision(CatchUpAction.Fail, null, error, "error-row", pick.Turn, false);
        if (!settled && !pick.UserRowFound && !streamedAny && turn is null)
            return new CatchUpDecision(CatchUpAction.Settle, null, null, "none", pick.Turn, false);
        if (streamedAny) return new CatchUpDecision(CatchUpAction.Finish, streamed.Trim(), null, "stream", pick.Turn, false);
        return new CatchUpDecision(CatchUpAction.Fail, null, null, "empty", pick.Turn, false, NoReply: true);
    }

    /// <summary>
    /// How long a finished reply stays in the receipt row: 12 s, plus a second per four words beyond the first twenty (a long markdown
    /// answer is read, not glanced at), at most a minute.
    /// </summary>
    public static double LingerSeconds(string? text)
    {
        int words = 0;
        bool inWord = false;
        foreach (var c in text ?? "")
        {
            if (char.IsWhiteSpace(c)) inWord = false;
            else if (!inWord) { inWord = true; words++; }
        }
        return Math.Clamp(12 + Math.Max(0, words - 20) / 4.0, 12, 60);
    }
}

/// <summary>
/// 189w D2 watchdog: while an ask collects, 15 s without any event for the desk means the stream may be dead (a lost focus claim, a
/// silent drop) — the Store then catches up from the transcript, and again 15 s after each catch-up for as long as the turn runs. Pure
/// with an injected clock: <see cref="Touch"/> on every desk event, <see cref="Decide"/> when the Store's timer fires.
/// </summary>
public sealed class AskWatchdog
{
    public static readonly TimeSpan Interval = TimeSpan.FromSeconds(15);

    /// <summary>What a firing timer should do: catch up now, or wait <see cref="Wait"/> more (events arrived meanwhile); nothing when disarmed.</summary>
    public readonly record struct Verdict(bool CatchUp, TimeSpan Wait, bool Armed);

    private DateTimeOffset last;

    public bool Armed { get; private set; }

    /// <summary>The ask was sent (or the collecting resumed): the silence is counted from <paramref name="now"/>.</summary>
    public void Arm(DateTimeOffset now)
    {
        Armed = true;
        last = now;
    }

    /// <summary>An event for the desk arrived (any type): the silence restarts.</summary>
    public void Touch(DateTimeOffset now)
    {
        if (Armed) last = now;
    }

    /// <summary>A catch-up just ran: the next one is 15 s of silence from here.</summary>
    public void CaughtUp(DateTimeOffset now) => Touch(now);

    public void Disarm() => Armed = false;

    public Verdict Decide(DateTimeOffset now)
    {
        if (!Armed) return new Verdict(false, TimeSpan.Zero, false);
        var silent = now - last;
        return silent >= Interval ? new Verdict(true, Interval, true) : new Verdict(false, Interval - silent, true);
    }
}