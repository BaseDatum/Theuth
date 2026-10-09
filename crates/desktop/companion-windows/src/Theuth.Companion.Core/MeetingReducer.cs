namespace Theuth.Companion.Core;

// Design 149 §5.1 / D11: the port of UI/store/meeting.ts:156-220 (the current meeting as the island sees it) with the brief
// stack rule :181-190 verbatim. Pure: side effects (meeting_cursor, window show/hide) are the island window's. The inputs
// are Core records (MeetingChange) so the reducer and its tests stand without the Rpc DTOs; Theuth.Companion's
// MeetingWire maps WW1's MeetingEvent / Topic / Brief / Utterance onto them one to one.

/// <summary>A topic button (<c>meeting::Topic</c>).</summary>
public sealed record MeetingTopic(string Id, string Label, long AtMs, string? Brief = null, string? Cached = null, string? Source = null);

/// <summary>A brief in the stack (<c>meeting::Brief</c>); <see cref="Source"/> is the <c>BriefSource</c> word: Auto · Tap · Request · Deeper · Typed · Cache · Oracle.</summary>
public sealed record MeetingBrief(string Id, string? Topic, string Title, IReadOnlyList<string> Bullets, string Source, long AtMs, string? Parent = null, string? Cached = null)
{
    /// <summary>53 §4.6 / 05 §7: an Auto (or Cache) brief queues behind a stepped-back cursor; anything the user asked for jumps.</summary>
    public bool IsAuto => Source is "Auto" or "Cache";
}

/// <summary>One live transcript row (<c>meeting.ts</c> <c>Line</c>); <see cref="Open"/> = rolling partial, replaced by the track's next utterance.</summary>
public sealed record MeetingLine(string Track, long T0Ms, string Text, bool Request, bool Open, string? Speaker)
{
    /// <summary><c>me:</c> / <c>Speaker N:</c> / <c>them:</c> (07 §4).</summary>
    public string Who
    {
        get
        {
            if (Track == "Mic") return "me:";
            if (Speaker is { } s && s.Length > 1 && s[0] == 's' && int.TryParse(s.AsSpan(1), out var n)) return $"Speaker {n}:";
            return "them:";
        }
    }
}

/// <summary>57 §3.4: one oracle verdict; <see cref="Disposition"/> is <c>skip</c> · <c>brief</c> · else escalated.</summary>
public sealed record MeetingOracleRow(string BatchId, string Disposition, bool Tier2, bool? Agreed);

/// <summary>What the reducer consumes: the 13 <c>MeetingEvent</c> variants the island reads plus the meeting session's turn activity.</summary>
public abstract record MeetingChange
{
    private MeetingChange() { }

    public sealed record Started(string Meeting, string Session) : MeetingChange;

    /// <summary><see cref="State"/> is the <c>MeetingState</c> word (<c>Post</c> for the tagged variant, with <see cref="PostStage"/>).</summary>
    public sealed record State(string Meeting, string StateWord, IReadOnlyList<string> Degraded, string? PostStage = null) : MeetingChange;

    public sealed record TopicAdded(string Meeting, MeetingTopic Topic) : MeetingChange;

    public sealed record TopicRemoved(string Meeting, string Topic, string Why) : MeetingChange;

    public sealed record Brief(string Meeting, MeetingBrief Value) : MeetingChange;

    public sealed record Utterance(string Meeting, string Track, long T0Ms, string Text, bool Request, bool Final, string? Speaker) : MeetingChange;

    /// <summary><c>Level</c>: the meters (kept on the snapshot for the strip; <c>meeting.ts</c> drives them from a ref).</summary>
    public sealed record Level(string Meeting, float MicRms, float SystemRms) : MeetingChange;

    public sealed record Listening(string Meeting, bool Held) : MeetingChange;

    public sealed record ApprovalWaiting(string Meeting, bool Waiting) : MeetingChange;

    public sealed record PostProgress(string Meeting, string Stage, int Pct) : MeetingChange;

    public sealed record ReviewReady(string Meeting) : MeetingChange;

    public sealed record Deleted(string Meeting) : MeetingChange;

    public sealed record OracleDeciding(string Meeting, string BatchId) : MeetingChange;

    public sealed record OracleDecision(string Meeting, MeetingOracleRow Decision) : MeetingChange;

    /// <summary><c>TurnStarted</c> → "thinking…", <c>ToolCallStarted</c> → "‹name›…", <c>TurnFinished</c> → null; only for the meeting's session.</summary>
    public sealed record Activity(string Session, string? Text) : MeetingChange;
}

/// <summary>The island's view of the meeting (<c>MeetingSlice</c>). Immutable; <see cref="MeetingReducer"/> returns the next one.</summary>
public sealed record MeetingSnapshot
{
    public static readonly MeetingSnapshot Empty = new();

    public string? Meeting { get; init; }

    public string? Session { get; init; }

    /// <summary>The <c>MeetingState</c> word: Idle · Starting · Recording · Paused · Stopping · Post · Review · Summarizing · Done · Failed.</summary>
    public string State { get; init; } = "Idle";

    public IReadOnlyList<string> Degraded { get; init; } = Array.Empty<string>();

    /// <summary>Recording clock at <see cref="ClockAt"/>; ticking while <see cref="State"/> is Recording.</summary>
    public long ElapsedMs { get; init; }

    public DateTimeOffset ClockAt { get; init; }

    public IReadOnlyList<MeetingTopic> Topics { get; init; } = Array.Empty<MeetingTopic>();

    /// <summary>Tapped, brief not arrived yet: greyed.</summary>
    public IReadOnlyList<string> Tapped { get; init; } = Array.Empty<string>();

    /// <summary>Oldest first; <see cref="Cursor"/> indexes it (−1 = empty).</summary>
    public IReadOnlyList<MeetingBrief> Briefs { get; init; } = Array.Empty<MeetingBrief>();

    public int Cursor { get; init; } = -1;

    /// <summary>Auto briefs that arrived while stepped back (decision 11): the <c>↑N new</c> badge.</summary>
    public int Unseen { get; init; }

    public bool Listening { get; init; }

    public bool ApprovalWaiting { get; init; }

    public string? Activity { get; init; }

    /// <summary>Post pipeline (04 §7): the stage and its 0–100 progress; null outside Post.</summary>
    public (string Stage, int Pct)? Progress { get; init; }

    public IReadOnlyList<MeetingOracleRow> Oracle { get; init; } = Array.Empty<MeetingOracleRow>();

    public string? Deciding { get; init; }

    /// <summary>The last <see cref="MeetingReducer.LinesKeep"/> live rows, oldest first; at most one open row per track.</summary>
    public IReadOnlyList<MeetingLine> Lines { get; init; } = Array.Empty<MeetingLine>();

    /// <summary>The last <c>Level</c> (0–1 RMS) per track.</summary>
    public float MicLevel { get; init; }

    public float SystemLevel { get; init; }

    /// <summary>The brief under the cursor.</summary>
    public MeetingBrief? Current => Cursor >= 0 && Cursor < Briefs.Count ? Briefs[Cursor] : null;

    /// <summary>The recording clock is on while recording (pauses are gaps, 01 §3).</summary>
    public bool IsLive => State is "Recording" or "Paused";

    /// <summary>Recording clock now.</summary>
    public long Elapsed(DateTimeOffset now) => ElapsedMs + (State == "Recording" ? (long)(now - ClockAt).TotalMilliseconds : 0);

    /// <summary>The newest topic or brief time, for D10's 10-minute pending window; null when there is none.</summary>
    public long? LastTopicOrBriefMs
    {
        get
        {
            long? best = null;
            foreach (var t in Topics) best = best is { } b ? Math.Max(b, t.AtMs) : t.AtMs;
            foreach (var b in Briefs) best = best is { } x ? Math.Max(x, b.AtMs) : b.AtMs;
            return best;
        }
    }
}

public static class MeetingReducer
{
    /// <summary>Oracle verdicts kept per meeting (57 §3.4).</summary>
    public const int OracleKeep = 50;

    /// <summary>Live transcript lines kept for the transcript dropdown (03 §2).</summary>
    public const int LinesKeep = 200;

    /// <summary><c>meeting.ts</c> <c>live</c>: Recording or Paused.</summary>
    public static bool Live(string state) => state is "Recording" or "Paused";

    /// <summary>Pure transition (<c>reduce</c>), <paramref name="now"/> standing in for <c>Date.now()</c>.</summary>
    public static MeetingSnapshot Reduce(MeetingSnapshot s, MeetingChange e, DateTimeOffset now)
    {
        switch (e)
        {
            case MeetingChange.Started st:
                return MeetingSnapshot.Empty with { Meeting = st.Meeting, Session = st.Session, State = "Recording", ClockAt = now };
            case MeetingChange.State c:
            {
                if (s.Meeting is not null && s.Meeting != c.Meeting) return s;
                bool live = Live(c.StateWord);
                return s with
                {
                    Meeting = c.Meeting,
                    State = c.StateWord,
                    Degraded = c.Degraded,
                    ElapsedMs = s.Elapsed(now),
                    ClockAt = now,
                    // The cues end with the recording.
                    Listening = live && s.Listening,
                    ApprovalWaiting = live && s.ApprovalWaiting,
                    Activity = live ? s.Activity : null,
                    Progress = c.StateWord == "Post" ? s.Progress : null,
                };
            }
            case MeetingChange.TopicAdded t:
                return s with { Topics = s.Topics.Where(x => x.Id != t.Topic.Id).Append(t.Topic).ToArray() };
            case MeetingChange.TopicRemoved r:
                return s with { Topics = s.Topics.Where(x => x.Id != r.Topic).ToArray(), Tapped = s.Tapped.Where(x => x != r.Topic).ToArray() };
            case MeetingChange.Brief b:
            {
                var briefs = s.Briefs.Append(b.Value).ToArray();
                int top = briefs.Length - 1;
                // 53 §4.6: Auto at the top → show; Auto stepped back → queue and badge; anything the user asked for jumps.
                // 05 §7: a Cache brief (the actor's alias scan) is an Auto for the stack.
                if (b.Value.IsAuto && s.Briefs.Count > 0 && s.Cursor < s.Briefs.Count - 1)
                    return s with { Briefs = briefs, Unseen = s.Unseen + 1 };
                return s with
                {
                    Briefs = briefs,
                    Cursor = top,
                    Unseen = 0,
                    Tapped = b.Value.Topic is { } tp ? s.Tapped.Where(x => x != tp).ToArray() : s.Tapped,
                };
            }
            case MeetingChange.Listening l:
                return s with { Listening = l.Held };
            case MeetingChange.ApprovalWaiting a:
                return s with { ApprovalWaiting = a.Waiting };
            case MeetingChange.PostProgress p:
                if (s.Meeting is not null && s.Meeting != p.Meeting) return s;
                return s with { Meeting = p.Meeting, State = "Post", Progress = (p.Stage, p.Pct) };
            case MeetingChange.ReviewReady rr:
                return s.Meeting == rr.Meeting ? s with { State = "Review", Progress = null } : s;
            case MeetingChange.Deleted d:
                return s.Meeting == d.Meeting ? MeetingSnapshot.Empty : s;
            case MeetingChange.OracleDeciding od:
                return s.Meeting == od.Meeting ? s with { Deciding = od.BatchId } : s;
            case MeetingChange.OracleDecision dec:
            {
                if (s.Meeting != dec.Meeting) return s;
                int i = IndexOf(s.Oracle, x => x.BatchId == dec.Decision.BatchId);
                var oracle = i >= 0
                    ? s.Oracle.Select((x, j) => j == i ? dec.Decision : x).ToArray()
                    : s.Oracle.Append(dec.Decision).TakeLast(OracleKeep).ToArray();
                return s with { Oracle = oracle, Deciding = s.Deciding == dec.Decision.BatchId ? null : s.Deciding };
            }
            case MeetingChange.Utterance u:
            {
                if (s.Meeting != u.Meeting) return s;
                var line = new MeetingLine(u.Track, u.T0Ms, u.Text, u.Request, Open: !u.Final, u.Speaker);
                // A labelled re-publish of a committed row updates it in place (07 §3).
                int i = u.Final && u.Speaker is not null ? IndexOf(s.Lines, l => !l.Open && l.Track == u.Track && l.T0Ms == line.T0Ms) : -1;
                if (i >= 0) return s with { Lines = s.Lines.Select((l, j) => j == i ? line : l).ToArray() };
                var kept = s.Lines.Where(l => !(l.Open && l.Track == u.Track));
                return s with { Lines = kept.Append(line).TakeLast(LinesKeep).ToArray() };
            }
            case MeetingChange.Level lv:
                return s.Meeting == lv.Meeting ? s with { MicLevel = lv.MicRms, SystemLevel = lv.SystemRms } : s;
            case MeetingChange.Activity act:
                return s.Session == act.Session ? s with { Activity = act.Text } : s;
            default:
                return s;
        }
    }

    /// <summary>Step the cursor (+1 newer, −1 older); stepping up eats the badge one by one, down leaves it.</summary>
    public static MeetingSnapshot Move(MeetingSnapshot s, int delta)
    {
        int cursor = Math.Min(s.Briefs.Count - 1, Math.Max(0, s.Cursor + delta));
        if (cursor == s.Cursor) return s;
        return s with { Cursor = cursor, Unseen = Math.Min(s.Unseen, s.Briefs.Count - 1 - cursor) };
    }

    /// <summary>
    /// Island fix §N: show the brief at <paramref name="index"/> (clamped). The ONE path behind a topic pill whose brief exists,
    /// the older/newer buttons and J/K/↑/↓.
    /// </summary>
    public static MeetingSnapshot ShowAt(MeetingSnapshot s, int index)
    {
        if (s.Briefs.Count == 0) return s;
        int cursor = Math.Min(s.Briefs.Count - 1, Math.Max(0, index));
        if (cursor == s.Cursor) return s;
        return s with { Cursor = cursor, Unseen = Math.Min(s.Unseen, s.Briefs.Count - 1 - cursor) };
    }

    /// <summary>J/↓ (+1) and K/↑ (−1): the brief index <paramref name="delta"/> away from the cursor, wrapping at both ends; −1 when there are none.</summary>
    public static int CycleIndex(MeetingSnapshot s, int delta)
    {
        int n = s.Briefs.Count;
        if (n == 0) return -1;
        int from = s.Cursor < 0 ? (delta > 0 ? -1 : 0) : s.Cursor;
        return ((from + delta) % n + n) % n;
    }

    /// <summary>The newest brief of <paramref name="topic"/> (its own <c>brief</c> id first, else the latest brief naming it), or −1: a pill click walks to it.</summary>
    public static int TopicBriefIndex(MeetingSnapshot s, string topic)
    {
        var t = s.Topics.FirstOrDefault(x => x.Id == topic);
        if (t?.Brief is { } id)
        {
            int at = IndexOf(s.Briefs, b => b.Id == id);
            if (at >= 0) return at;
        }
        for (int i = s.Briefs.Count - 1; i >= 0; i--) if (s.Briefs[i].Topic == topic) return i;
        return -1;
    }

    public static MeetingSnapshot MarkTapped(MeetingSnapshot s, string topic) =>
        s.Tapped.Contains(topic) ? s : s with { Tapped = s.Tapped.Append(topic).ToArray() };

    /// <summary><c>meeting_status</c> + <c>meeting_briefs</c> + <c>meeting_live</c> → the snapshot (connect, reconnect).</summary>
    public static MeetingSnapshot Hydrate(
        string? meeting, string? session, string state, IReadOnlyList<string> degraded, long elapsedMs, string? cursor,
        IReadOnlyList<MeetingTopic> topics, bool approvalWaiting, IReadOnlyList<MeetingBrief> briefs, IReadOnlyList<MeetingLine> lines, DateTimeOffset now)
    {
        int at = cursor is null ? -1 : IndexOf(briefs, b => b.Id == cursor);
        return MeetingSnapshot.Empty with
        {
            Meeting = meeting,
            Session = session,
            State = state,
            Degraded = degraded,
            ElapsedMs = elapsedMs,
            ClockAt = now,
            Topics = topics,
            Briefs = briefs,
            Cursor = at >= 0 ? at : briefs.Count - 1,
            ApprovalWaiting = approvalWaiting,
            Lines = lines.TakeLast(LinesKeep).ToArray(),
        };
    }

    /// <summary>57 §3.4 counters as one line: <c>14 skipped · 3 briefs · 2 escalated · 5 tier 2 calls · agreed 92 %</c>; null when nothing ran.</summary>
    public static string? OracleSummary(IReadOnlyList<MeetingOracleRow> oracle)
    {
        if (oracle.Count == 0) return null;
        int skipped = 0, briefs = 0, escalated = 0, tier2 = 0, judged = 0, agreed = 0;
        foreach (var d in oracle)
        {
            if (d.Disposition == "skip") skipped++;
            else if (d.Disposition == "brief") briefs++;
            else escalated++;
            if (d.Tier2) tier2++;
            if (d.Agreed is { } a)
            {
                judged++;
                if (a) agreed++;
            }
        }
        var parts = new List<string>
        {
            $"{skipped} skipped", $"{briefs} brief{(briefs == 1 ? "" : "s")}", $"{escalated} escalated", $"{tier2} tier 2 call{(tier2 == 1 ? "" : "s")}",
        };
        if (judged > 0) parts.Add($"agreed {(int)Math.Round(100.0 * agreed / judged)} %");
        return string.Join(" · ", parts);
    }

    private static int IndexOf<T>(IReadOnlyList<T> xs, Func<T, bool> pred)
    {
        for (int i = 0; i < xs.Count; i++) if (pred(xs[i])) return i;
        return -1;
    }
}
