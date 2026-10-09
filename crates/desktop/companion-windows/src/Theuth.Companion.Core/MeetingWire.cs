using System.Text.Json;

namespace Theuth.Companion.Core;

/// <summary>
/// The meeting wire → Core records (149 §3.6 "all RPCs exist"): decodes <c>meeting_status</c>, <c>meeting_briefs</c>,
/// <c>meeting_live</c> results and the 13 <c>MeetingEvent</c> stream items (<c>crates/meeting/src/lib.rs</c>, tagged by
/// <c>type</c>) into <see cref="MeetingChange"/>s for <see cref="MeetingReducer"/>. Reads JSON directly so the island needs no
/// typed DTO beyond WW1's raw stream hook; every field optional-tolerant (a missing field is a default, never a throw).
/// </summary>
public static class MeetingWire
{
    public static readonly IReadOnlySet<string> EventTypes = new HashSet<string>(StringComparer.Ordinal)
    {
        "Started", "State", "TopicAdded", "TopicRemoved", "Brief", "Utterance", "Listening", "ApprovalWaiting",
        "PostProgress", "ReviewReady", "Deleted", "OracleDeciding", "OracleDecision", "Level",
    };

    public static readonly IReadOnlySet<string> ActivityTypes = new HashSet<string>(StringComparer.Ordinal)
    {
        "TurnStarted", "ToolCallStarted", "ToolCallFinished", "TurnFinished",
    };

    /// <summary>One stream item → a change, or null when it is not the island's (or malformed).</summary>
    public static MeetingChange? Decode(string type, ReadOnlySpan<byte> json)
    {
        if (!EventTypes.Contains(type) && !ActivityTypes.Contains(type)) return null;
        try
        {
            using var doc = JsonDocument.Parse(json.ToArray());
            return Decode(type, doc.RootElement);
        }
        catch (JsonException)
        {
            return null;
        }
    }

    public static MeetingChange? Decode(string type, JsonElement e)
    {
        string meeting = Str(e, "meeting") ?? "";
        switch (type)
        {
            case "Started": return new MeetingChange.Started(meeting, Str(e, "session") ?? "");
            case "State":
            {
                var (word, stage) = StateWord(e.TryGetProperty("state", out var st) ? st : default);
                return new MeetingChange.State(meeting, word, Words(e, "degraded"), stage);
            }
            case "TopicAdded": return e.TryGetProperty("topic", out var t) && Topic(t) is { } topic ? new MeetingChange.TopicAdded(meeting, topic) : null;
            case "TopicRemoved": return new MeetingChange.TopicRemoved(meeting, Str(e, "topic") ?? "", Str(e, "why") ?? "");
            case "Brief": return e.TryGetProperty("brief", out var b) && Brief(b) is { } brief ? new MeetingChange.Brief(meeting, brief) : null;
            case "Utterance":
                return new MeetingChange.Utterance(meeting, Str(e, "track") ?? "System", Long(e, "t0_ms"), Str(e, "text") ?? "", Bool(e, "request"),
                    e.TryGetProperty("final", out var f) && f.ValueKind != JsonValueKind.Undefined ? f.ValueKind == JsonValueKind.True : true, Str(e, "speaker"));
            case "Level":
                return new MeetingChange.Level(meeting, e.TryGetProperty("mic_rms", out var mr) && mr.ValueKind == JsonValueKind.Number ? (float)mr.GetDouble() : 0,
                    e.TryGetProperty("system_rms", out var sr) && sr.ValueKind == JsonValueKind.Number ? (float)sr.GetDouble() : 0);
            case "Listening": return new MeetingChange.Listening(meeting, Bool(e, "held"));
            case "ApprovalWaiting": return new MeetingChange.ApprovalWaiting(meeting, Bool(e, "waiting"));
            case "PostProgress": return new MeetingChange.PostProgress(meeting, Str(e, "stage") ?? "", (int)Long(e, "pct"));
            case "ReviewReady": return new MeetingChange.ReviewReady(meeting);
            case "Deleted": return new MeetingChange.Deleted(meeting);
            case "OracleDeciding": return new MeetingChange.OracleDeciding(meeting, Str(e, "batch_id") ?? "");
            case "OracleDecision":
            {
                if (!e.TryGetProperty("decision", out var d)) return null;
                string kind = d.TryGetProperty("disposition", out var disp) && disp.ValueKind == JsonValueKind.Object ? Str(disp, "kind") ?? "" : Str(d, "disposition") ?? "";
                bool tier2 = d.TryGetProperty("tier2", out var t2) && t2.ValueKind is not (JsonValueKind.Null or JsonValueKind.Undefined);
                bool? agreed = d.TryGetProperty("outcome", out var o) && o.ValueKind == JsonValueKind.Object ? Bool(o, "agreed") : null;
                return new MeetingChange.OracleDecision(meeting, new MeetingOracleRow(Str(d, "batch_id") ?? "", kind, tier2, agreed));
            }
            case "TurnStarted": return new MeetingChange.Activity(Str(e, "session") ?? "", "thinking…");
            case "ToolCallStarted": return new MeetingChange.Activity(Str(e, "session") ?? "", $"{Str(e, "name") ?? "tool"}…");
            case "ToolCallFinished": return new MeetingChange.Activity(Str(e, "session") ?? "", "thinking…");
            case "TurnFinished": return new MeetingChange.Activity(Str(e, "session") ?? "", null);
            default: return null;
        }
    }

    /// <summary><c>meeting_status</c> + <c>meeting_briefs</c> + <c>meeting_live</c> → the hydrated snapshot.</summary>
    public static MeetingSnapshot Hydrate(JsonElement status, JsonElement? briefs, JsonElement? live, DateTimeOffset now)
    {
        var (word, _) = StateWord(status.TryGetProperty("state", out var st) ? st : default);
        var topics = new List<MeetingTopic>();
        if (status.TryGetProperty("topics", out var ts) && ts.ValueKind == JsonValueKind.Array)
            foreach (var t in ts.EnumerateArray()) if (Topic(t) is { } topic) topics.Add(topic);
        var bs = new List<MeetingBrief>();
        if (briefs is { ValueKind: JsonValueKind.Array } ba)
            foreach (var b in ba.EnumerateArray()) if (Brief(b) is { } brief) bs.Add(brief);
        var lines = new List<MeetingLine>();
        if (live is { ValueKind: JsonValueKind.Array } la)
            foreach (var u in la.EnumerateArray())
                lines.Add(new MeetingLine(Str(u, "track") ?? "System", Long(u, "t0_ms"), Str(u, "text") ?? "", Bool(u, "request"), Open: false, Str(u, "speaker")));
        return MeetingReducer.Hydrate(Str(status, "meeting"), Str(status, "session"), word, Words(status, "degraded"), Long(status, "elapsed_ms"),
            Str(status, "cursor"), topics, Bool(status, "approval_waiting"), bs, lines, now);
    }

    /// <summary><c>MeetingState</c>: a word, or <c>{"Post": "Transcribing"}</c>.</summary>
    public static (string Word, string? Stage) StateWord(JsonElement s)
    {
        if (s.ValueKind == JsonValueKind.String) return (s.GetString() ?? "Idle", null);
        if (s.ValueKind == JsonValueKind.Object)
            foreach (var p in s.EnumerateObject()) return (p.Name, p.Value.ValueKind == JsonValueKind.String ? p.Value.GetString() : null);
        return ("Idle", null);
    }

    /// <summary><c>Degradation</c> items: a word, or <c>{"SttDown": "Mic"}</c> → <c>SttDown:Mic</c>.</summary>
    private static IReadOnlyList<string> Words(JsonElement e, string key)
    {
        var out_ = new List<string>();
        if (!e.TryGetProperty(key, out var a) || a.ValueKind != JsonValueKind.Array) return out_;
        foreach (var d in a.EnumerateArray())
        {
            if (d.ValueKind == JsonValueKind.String) out_.Add(d.GetString() ?? "");
            else if (d.ValueKind == JsonValueKind.Object)
                foreach (var p in d.EnumerateObject()) { out_.Add($"{p.Name}:{(p.Value.ValueKind == JsonValueKind.String ? p.Value.GetString() : p.Value.ToString())}"); break; }
        }
        return out_;
    }

    private static MeetingTopic? Topic(JsonElement t)
    {
        if (t.ValueKind != JsonValueKind.Object || Str(t, "id") is not { } id) return null;
        return new MeetingTopic(id, Str(t, "label") ?? "", Long(t, "at_ms"), Str(t, "brief"), Str(t, "cached"), Str(t, "source"));
    }

    private static MeetingBrief? Brief(JsonElement b)
    {
        if (b.ValueKind != JsonValueKind.Object || Str(b, "id") is not { } id) return null;
        var bullets = new List<string>();
        if (b.TryGetProperty("bullets", out var bl) && bl.ValueKind == JsonValueKind.Array)
            foreach (var x in bl.EnumerateArray()) if (x.ValueKind == JsonValueKind.String) bullets.Add(x.GetString() ?? "");
        return new MeetingBrief(id, Str(b, "topic"), Str(b, "title") ?? "", bullets, Str(b, "source") ?? "Auto", Long(b, "at_ms"), Str(b, "parent"), Str(b, "cached"));
    }

    private static string? Str(JsonElement e, string key) =>
        e.ValueKind == JsonValueKind.Object && e.TryGetProperty(key, out var v) ? v.ValueKind switch
        {
            JsonValueKind.String => v.GetString(),
            JsonValueKind.Number => v.GetRawText(),
            _ => null,
        } : null;

    private static long Long(JsonElement e, string key) =>
        e.ValueKind == JsonValueKind.Object && e.TryGetProperty(key, out var v) && v.ValueKind == JsonValueKind.Number && v.TryGetInt64(out var n) ? n : 0;

    private static bool Bool(JsonElement e, string key) =>
        e.ValueKind == JsonValueKind.Object && e.TryGetProperty(key, out var v) && v.ValueKind == JsonValueKind.True;
}
