using System.Text.Json;
using System.Text.Json.Serialization;

namespace Theuth.Companion.Rpc;

// Design 149 WW1: the wire shapes the One-parity surfaces, the native island and point-and-say read (149 §5.1 Models.swift
// row, §6, §7.4 #6). Hand-written mirrors of the Rust types (the ts-rs bindings under crates/*/bindings are not all
// committed; the Rust source is the truth and each type names it). Rules as Models.cs: only the fields the companion
// reads; unknown keys are ignored; a field serde marks `#[serde(default)]` is optional here too, so an older engine
// still decodes. Enums the engine writes PascalCase (meeting::*) are kept as their wire word in a string-backed struct,
// never a C# enum: the shared JsonStringEnumConverter is snake_case and an unknown word must not throw (149 §10).

// MARK: roster::attention — E6 (149 §6, Revision 1 #5)

/// <summary>
/// <c>ItemContext { request: Option&lt;String&gt;, last_reply: Option&lt;String&gt; }</c> — the conversation an attention item sits
/// in, both ≤ 160 chars, either null when unknown. Carried by <see cref="AttentionItem.Context"/> on engines with W0's E6;
/// null before it (the Core <c>AttentionContext</c> join fills in from <c>SessionSummary.title</c> and <c>AgentInfo.last</c>).
/// </summary>
public sealed record ItemContext
{
    public string? Request { get; init; }
    public string? LastReply { get; init; }
}

// MARK: agent (crates/agent/src/lib.rs:2232 SessionSummary; :340 CancelScope; :156 Delivery)

/// <summary>
/// One row of <c>list_sessions</c> (<c>agent::SessionSummary</c>). <c>Owner</c> is the owning engine's id (empty on engines
/// older than 68) and <c>Here</c> whether it is this engine; a row owned elsewhere is drawn hollow with "on ‹node›" (149 E7
/// fallback). <c>Running</c> is the live flag the agents panel reads; <c>Title</c> is the E6 interim <c>request</c>.
/// </summary>
public sealed record SessionSummary
{
    [JsonRequired] public string Id { get; init; } = "";
    public DateTimeOffset? CreatedAt { get; init; }
    public DateTimeOffset? UpdatedAt { get; init; }
    [JsonRequired] public string Title { get; init; } = "";
    public ValueList<string> Tags { get; init; } = new();
    public string Model { get; init; } = "";
    public string? Parent { get; init; }
    public string? Name { get; init; }
    public string? Effort { get; init; }
    /// <summary>External harness key (<c>agents.yaml</c>) when an ACP child; null = a native theuth session.</summary>
    public string? Agent { get; init; }
    public string? Cwd { get; init; }
    public string? HarnessModel { get; init; }
    public string? Desk { get; init; }
    public bool Pinned { get; init; }
    public bool Archived { get; init; }
    public string? Project { get; init; }
    public bool Running { get; init; }
    /// <summary>Owning engine id; "" on an engine without session sync.</summary>
    public string Owner { get; init; } = "";
    /// <summary>Owned by this engine (serde default true).</summary>
    public bool Here { get; init; } = true;
}

/// <summary><c>sessions_status</c> (<c>sessionsync::SessionsStatus</c>): this engine's id, sessions owned here / elsewhere.</summary>
public sealed record SessionsStatus
{
    [JsonRequired] public string Engine { get; init; } = "";
    public uint Here { get; init; }
    public uint Elsewhere { get; init; }
    public ulong LogSeq { get; init; }
    public string? LastSnapshot { get; init; }
}

/// <summary><c>cancel(session, scope)</c>'s trailing optional (<c>#[serde(rename_all = "lowercase")]</c>): <c>root</c> · <c>tree</c>.</summary>
public enum CancelScope { Root, Tree }

/// <summary><c>send_input</c> / <c>steer</c> delivery (<c>snake_case</c>): <c>steer</c> (after the current tool batch) · <c>next_turn</c>.</summary>
public enum Delivery { Steer, NextTurn }

// MARK: computer (crates/computer/src/lib.rs:133-147, pointer.rs:12 Sample)

/// <summary>One pointer sample of the trail: ms since the press, logical screen coordinates.</summary>
public sealed record PointerSample
{
    public PointerSample() { }

    public PointerSample(uint tMs, float x, float y, bool pen = true)
    {
        TMs = tMs;
        X = x;
        Y = y;
        Pen = pen;
    }

    [JsonPropertyName("t_ms")] public uint TMs { get; init; }
    public float X { get; init; }
    public float Y { get; init; }

    /// <summary>The pointer was drawing (left button held in pen mode); the engine defaults absent to true.</summary>
    public bool Pen { get; init; } = true;
}

/// <summary><c>screen_context(req)</c>: the trail, the overlay colour, how far back the engine looks, and whether to read the accessibility tree.</summary>
public sealed record ScreenContextReq
{
    public ValueList<PointerSample> Samples { get; init; } = new();
    public string Color { get; init; } = "";
    public ulong SinceMs { get; init; }
    public bool Accessibility { get; init; }
}

/// <summary>The engine's answer: the <c>[screen …]</c> lines for the prompt and the PNG paths to attach (<c>send_input</c>'s <c>files</c>).</summary>
public sealed record ScreenContextReply
{
    public ValueList<string> Lines { get; init; } = new();
    public ValueList<string> Files { get; init; } = new();
}

// MARK: meeting (crates/meeting/src/lib.rs) — the island's data

/// <summary>
/// <c>meeting::MeetingState</c>: a PascalCase word (<c>Idle</c> · <c>Starting</c> · <c>Recording</c> · <c>Paused</c> · <c>Stopping</c> ·
/// <c>Review</c> · <c>Summarizing</c> · <c>Done</c> · <c>Failed</c>) or the object <c>{"Post": "Transcribing" | "Diarizing" | "Aligning"}</c>.
/// Unknown words decode as themselves (<see cref="Word"/>), never throw.
/// </summary>
[JsonConverter(typeof(MeetingStateConverter))]
public readonly record struct MeetingState(string Word, string? Post = null)
{
    public static MeetingState Idle => new("Idle");
    public static MeetingState Recording => new("Recording");
    public static MeetingState Paused => new("Paused");

    /// <summary>Recording or paused: the island is "live" (149 Revision 1 #13).</summary>
    public bool IsLive => Word is "Recording" or "Paused" or "Starting";

    public override string ToString() => Post is null ? Word : $"{Word}({Post})";
}

public sealed class MeetingStateConverter : JsonConverter<MeetingState>
{
    public override MeetingState Read(ref Utf8JsonReader reader, Type typeToConvert, JsonSerializerOptions options)
    {
        using var doc = JsonDocument.ParseValue(ref reader);
        var e = doc.RootElement;
        if (e.ValueKind == JsonValueKind.String) return new MeetingState(e.GetString() ?? "");
        if (e.ValueKind == JsonValueKind.Object)
        {
            foreach (var p in e.EnumerateObject())
            {
                return new MeetingState(p.Name, p.Value.ValueKind == JsonValueKind.String ? p.Value.GetString() : p.Value.GetRawText());
            }
        }
        return new MeetingState("");
    }

    public override void Write(Utf8JsonWriter writer, MeetingState value, JsonSerializerOptions options)
    {
        if (value.Post is null)
        {
            writer.WriteStringValue(value.Word);
            return;
        }
        writer.WriteStartObject();
        writer.WriteString(value.Word, value.Post);
        writer.WriteEndObject();
    }
}

/// <summary>
/// <c>meeting::Degradation</c>: a word (<c>MicNoAec</c> · <c>ModelDown</c> · <c>DiskLow</c> · <c>NearMaxLength</c> · <c>NoNote</c> ·
/// <c>DiarizationDown</c>) or an object with one argument (<c>{"SttDown":"Mic"}</c>, <c>{"PermissionDenied":"Microphone"}</c>).
/// </summary>
[JsonConverter(typeof(DegradationConverter))]
public readonly record struct Degradation(string Kind, string? Detail = null)
{
    public override string ToString() => Detail is null ? Kind : $"{Kind}({Detail})";
}

public sealed class DegradationConverter : JsonConverter<Degradation>
{
    public override Degradation Read(ref Utf8JsonReader reader, Type typeToConvert, JsonSerializerOptions options)
    {
        using var doc = JsonDocument.ParseValue(ref reader);
        var e = doc.RootElement;
        if (e.ValueKind == JsonValueKind.String) return new Degradation(e.GetString() ?? "");
        if (e.ValueKind == JsonValueKind.Object)
        {
            foreach (var p in e.EnumerateObject())
            {
                return new Degradation(p.Name, p.Value.ValueKind == JsonValueKind.String ? p.Value.GetString() : p.Value.GetRawText());
            }
        }
        return new Degradation("");
    }

    public override void Write(Utf8JsonWriter writer, Degradation value, JsonSerializerOptions options)
    {
        if (value.Detail is null)
        {
            writer.WriteStringValue(value.Kind);
            return;
        }
        writer.WriteStartObject();
        writer.WriteString(value.Kind, value.Detail);
        writer.WriteEndObject();
    }
}

/// <summary>
/// <c>meeting::Support</c> (<c>#[serde(tag = "kind", content = "reason")]</c>): <c>{"kind":"Full"}</c> · <c>{"kind":"Partial","reason":…}</c> ·
/// <c>{"kind":"None","reason":…}</c>. Also read as a bare word (the shared fixtures write <c>"Full"</c>) and as null/absent (<see cref="Kind"/> "",
/// an engine that did not say — the Swift <c>MeetingSupport</c> tolerates the same shapes).
/// </summary>
[JsonConverter(typeof(MeetingSupportConverter))]
public readonly record struct MeetingSupport(string Kind, string? Reason = null)
{
    public static MeetingSupport Full => new("Full");
    public static MeetingSupport Unknown => new("");

    public bool IsFull => Kind == "Full";

    /// <summary><c>None</c> — the host cannot record meetings at all; <see cref="Reason"/> says why.</summary>
    public bool IsUnsupported => Kind == "None";
}

public sealed class MeetingSupportConverter : JsonConverter<MeetingSupport>
{
    public override MeetingSupport Read(ref Utf8JsonReader reader, Type typeToConvert, JsonSerializerOptions options)
    {
        using var doc = JsonDocument.ParseValue(ref reader);
        var e = doc.RootElement;
        if (e.ValueKind == JsonValueKind.String) return new MeetingSupport(e.GetString() ?? "");
        if (e.ValueKind == JsonValueKind.Object)
        {
            string kind = e.TryGetProperty("kind", out var k) && k.ValueKind == JsonValueKind.String ? k.GetString() ?? "" : "";
            string? reason = e.TryGetProperty("reason", out var r) && r.ValueKind == JsonValueKind.String ? r.GetString() : null;
            return new MeetingSupport(kind, reason);
        }
        return MeetingSupport.Unknown;
    }

    public override void Write(Utf8JsonWriter writer, MeetingSupport value, JsonSerializerOptions options)
    {
        writer.WriteStartObject();
        writer.WriteString("kind", value.Kind);
        if (value.Reason is not null) writer.WriteString("reason", value.Reason);
        writer.WriteEndObject();
    }

    public override bool HandleNull => true;
}

/// <summary>One auto-noticed subject (<c>meeting::Topic</c>). <c>Source</c> and the <c>cached</c> id are the oracle's (<c>Auto</c> · <c>Tap</c> · <c>Request</c> · <c>Deeper</c> · <c>Typed</c> · <c>Cache</c> · <c>Oracle</c>).</summary>
public sealed record Topic
{
    [JsonRequired] public string Id { get; init; } = "";
    [JsonRequired] public string Label { get; init; } = "";
    public ulong AtMs { get; init; }
    public string? Brief { get; init; }
    public string? Cached { get; init; }
    public string? Source { get; init; }
}

/// <summary>A brief shown on the island (<c>meeting::Brief</c>): title + bullets; <c>Parent</c> links a "deeper" brief to the one it extends.</summary>
public sealed record Brief
{
    [JsonRequired] public string Id { get; init; } = "";
    public string? Topic { get; init; }
    [JsonRequired] public string Title { get; init; } = "";
    public ValueList<string> Bullets { get; init; } = new();
    public string Source { get; init; } = "";
    public ulong AtMs { get; init; }
    public string? Parent { get; init; }
    public string? Cached { get; init; }
}

/// <summary>One transcript line (<c>meeting::Utterance</c>): <c>Track</c> is <c>Mic</c> (me) or <c>System</c> (them / the named speaker).</summary>
public sealed record Utterance
{
    public string Track { get; init; } = "";
    [JsonPropertyName("t0_ms")] public ulong T0Ms { get; init; }
    [JsonPropertyName("t1_ms")] public ulong T1Ms { get; init; }
    public string Text { get; init; } = "";
    public bool Request { get; init; }
    public string? Speaker { get; init; }

    /// <summary>"me" · the speaker · "them" (the Rust <c>who()</c>).</summary>
    public string Who => Track == "Mic" ? "me" : Speaker ?? "them";
}

/// <summary><c>meeting_status</c> (<c>meeting::Status</c>, ts name <c>MeetingStatus</c>): the whole island state in one read.</summary>
public sealed record MeetingStatus
{
    public MeetingState State { get; init; } = MeetingState.Idle;
    public string? Meeting { get; init; }
    public string? Session { get; init; }
    public ulong ElapsedMs { get; init; }
    public ValueList<Degradation> Degraded { get; init; } = new();
    public string? Cursor { get; init; }
    public ValueList<Topic> Topics { get; init; } = new();
    public bool ApprovalWaiting { get; init; }
    public MeetingSupport Supported { get; init; } = MeetingSupport.Unknown;
    public string Accelerator { get; init; } = "";
    /// <summary>158 §4.5: the current detection prompt, for a companion connecting mid-prompt.</summary>
    public Detection? Detection { get; init; }

    public bool IsLive => State.IsLive;
}

/// <summary><c>meeting::ProcessInfo</c> (the mic holder / tapped app).</summary>
public sealed record ProcessInfo
{
    public uint Pid { get; init; }
    public string Name { get; init; } = "";
    public string? BundleId { get; init; }
    public bool Active { get; init; }
}

/// <summary><c>detect::Detection</c> (158 §15.1); <c>event</c> (W2), <c>project</c> and <c>brief</c> (W3) are <c>#[serde(default)]</c>.</summary>
public sealed record Detection
{
    [JsonRequired] public ProcessInfo App { get; init; } = new();
    [JsonRequired] public string AppId { get; init; } = "";
    public string Since { get; init; } = "";
    public ValueList<ProcessInfo> Also { get; init; } = new();
    public EventMatch? Event { get; init; }
    public string? Project { get; init; }
    public ValueList<string>? Brief { get; init; }
}

/// <summary><c>calendar::Attendee</c> (158 §15.1).</summary>
public sealed record Attendee
{
    [JsonRequired] public string Name { get; init; } = "";
    public string? Email { get; init; }
    public string? Status { get; init; }
    public bool IsMe { get; init; }
}

/// <summary><c>calendar::EventRef</c> (158 §15.1); <c>id</c> is the transparent <c>EventId</c> string <c>source:uid:recurrence_id</c>.</summary>
public sealed record EventRef
{
    [JsonRequired] public string Id { get; init; } = "";
    public string Uid { get; init; } = "";
    [JsonRequired] public string Title { get; init; } = "";
    public string Start { get; init; } = "";
    public string End { get; init; } = "";
    public string? JoinUrl { get; init; }
    public string? JoinHost { get; init; }
    public Attendee? Organizer { get; init; }
    public ValueList<Attendee> Attendees { get; init; } = new();
    public string? Description { get; init; }
}

/// <summary><c>calendar::EventMatch</c> (158 §8.1): the shown event, its score, the rest for the <c>[▾]</c> list.</summary>
public sealed record EventMatch
{
    [JsonRequired] public EventRef Event { get; init; } = new();
    public float Score { get; init; }
    public ValueList<EventRef> Alternatives { get; init; } = new();
}

/// <summary><c>detect::Source</c> (158 §15.1; TS <c>StartSource</c>): <c>{"kind":"manual"|"detected"|"calendar", app_id?, event?}</c>.</summary>
public sealed record StartSource(string Kind, string? AppId = null, string? Event = null)
{
    public static StartSource Manual => new("manual");

    public static StartSource Detected(string appId) => new("detected", AppId: appId);

    public static StartSource Calendar(string eventId) => new("calendar", Event: eventId);
}

/// <summary><c>meeting::StartMeeting</c> (<c>meeting_start</c>): the frozen six fields plus 158's optional four; nulls are omitted on the wire.</summary>
public sealed record StartMeeting
{
    public uint? Process { get; init; }
    public string? Title { get; init; }
    public string? Context { get; init; }
    /// <summary>R3: the lawful line was shown; the prompt's Start always sends true.</summary>
    public bool Consent { get; init; }
    public string? SessionModel { get; init; }
    public string? Memory { get; init; }
    public StartSource? Source { get; init; }
    public string? Event { get; init; }
    public string? Project { get; init; }
    public string? Template { get; init; }
}

/// <summary><c>meeting::Started</c>: the reply to <c>meeting_start</c>.</summary>
public sealed record MeetingStarted
{
    [JsonRequired] public string Meeting { get; init; } = "";
    [JsonRequired] public string Session { get; init; } = "";
}

/// <summary><c>meeting_permissions</c> (<c>meeting::Permissions</c>): <c>Granted</c> · <c>Denied</c> · <c>NotDetermined</c> per capability.</summary>
public sealed record MeetingPermissions
{
    public string Microphone { get; init; } = "";
    public string AudioCapture { get; init; } = "";
}

/// <summary>A cached brief (<c>meeting::cache::CachedBrief</c>): <c>brief_get</c> / <c>brief_search</c>.</summary>
public sealed record CachedBrief
{
    [JsonRequired] public string Id { get; init; } = "";
    [JsonRequired] public string Title { get; init; } = "";
    public ValueList<string> Bullets { get; init; } = new();
    public ValueList<string> Aliases { get; init; } = new();
    public ValueList<string> Tags { get; init; } = new();
    public string Source { get; init; } = "";
    public string CreatedAt { get; init; } = "";
    public string UpdatedAt { get; init; } = "";
    public uint Uses { get; init; }
    public string? LastUsedAt { get; init; }
}

/// <summary><c>meeting_oracle_mode</c> / <c>decision_mode</c> (<c>decision::ModeInfo</c>); <c>Mode</c> words kept verbatim.</summary>
public sealed record ModeInfo
{
    public JsonValue Mode { get; init; } = JsonValue.Null.Instance;
    public JsonValue Effective { get; init; } = JsonValue.Null.Instance;
    public string? Reason { get; init; }
    public bool Enabled { get; init; }
    public bool Installed { get; init; }
    public ulong BudgetMs { get; init; }
    public ulong EscalateBudgetMs { get; init; }
}

// MARK: the stream — meeting events (meeting::MeetingEvent, `#[serde(tag = "type")]`, lib.rs:268), SubagentSpawned, ActivityAppended

/// <summary>
/// Every <c>meeting::MeetingEvent</c> variant in one tolerant record, keyed by <see cref="Type"/> (<c>Started</c> · <c>State</c> ·
/// <c>Level</c> · <c>Utterance</c> · <c>TopicAdded</c> · <c>TopicRemoved</c> · <c>Brief</c> · <c>Listening</c> · <c>ApprovalWaiting</c> ·
/// <c>PostProgress</c> · <c>ReviewReady</c> · <c>Summarized</c> · <c>Deleted</c> · <c>OracleDeciding</c> · <c>OracleDecision</c>). Only the
/// fields of that variant are set; nothing here is required, so a newer engine's extra fields or a new variant never
/// throw — the Core <c>MeetingReducer</c> (WW3) switches on <see cref="Type"/>. <c>TopicAdded.topic</c> is an object and
/// <c>TopicRemoved.topic</c> a string: see <see cref="TopicAdded"/> and <see cref="TopicRemovedId"/>.
/// </summary>
public sealed record MeetingEvent
{
    public string Type { get; init; } = "";
    public string Meeting { get; init; } = "";

    // Started
    public string? Session { get; init; }
    public string? At { get; init; }

    // State
    public MeetingState? State { get; init; }
    public ValueList<Degradation> Degraded { get; init; } = new();

    // Level
    public float MicRms { get; init; }
    public float SystemRms { get; init; }

    // Utterance (same fields as Rpc.Utterance plus `final`, default true)
    public string? Track { get; init; }
    [JsonPropertyName("t0_ms")] public ulong T0Ms { get; init; }
    [JsonPropertyName("t1_ms")] public ulong T1Ms { get; init; }
    public string? Text { get; init; }
    public bool Request { get; init; }
    [JsonPropertyName("final")] public bool? IsFinal { get; init; }
    public string? Speaker { get; init; }

    // TopicAdded (object) / TopicRemoved (id string) share the key `topic`.
    [JsonPropertyName("topic")] public JsonValue? TopicValue { get; init; }
    /// <summary><c>TopicRemoved.why</c>: <c>Viewed</c> · <c>Dismissed</c> · <c>Expired</c>.</summary>
    public string? Why { get; init; }

    // Brief
    public Brief? Brief { get; init; }

    // Listening / ApprovalWaiting
    public bool? Held { get; init; }
    public bool? Waiting { get; init; }

    // PostProgress
    public string? Stage { get; init; }
    public byte? Pct { get; init; }

    // Summarized
    public string? Title { get; init; }

    // OracleDeciding / OracleDecision (the decision kept raw: the island shows nothing of it yet)
    public string? BatchId { get; init; }
    public JsonValue? Decision { get; init; }

    /// <summary>The 15 words of <c>meeting::MeetingEvent</c> — what <see cref="WireEvent"/> routes here.</summary>
    public static readonly IReadOnlySet<string> Types = new HashSet<string>(StringComparer.Ordinal)
    {
        "Started", "State", "Level", "Utterance", "TopicAdded", "TopicRemoved", "Brief", "Listening", "ApprovalWaiting",
        "PostProgress", "ReviewReady", "Summarized", "Deleted", "OracleDeciding", "OracleDecision",
    };

    /// <summary><c>TopicAdded</c>'s topic, decoded; null for every other type or a malformed object.</summary>
    public Topic? TopicAdded
    {
        get
        {
            if (Type != "TopicAdded" || TopicValue is not JsonValue.Object) return null;
            try { return TheuthJson.Deserialize<Topic>(TheuthJson.Serialize(TopicValue)); }
            catch (JsonException) { return null; }
        }
    }

    /// <summary><c>TopicRemoved</c>'s topic id; null otherwise.</summary>
    public string? TopicRemovedId => Type == "TopicRemoved" && TopicValue is JsonValue.String s ? s.Value : null;

    /// <summary>The utterance of an <c>Utterance</c> event as the <c>meeting_live</c> row type; null otherwise.</summary>
    public Utterance? AsUtterance => Type == "Utterance"
        ? new Utterance { Track = Track ?? "", T0Ms = T0Ms, T1Ms = T1Ms, Text = Text ?? "", Request = Request, Speaker = Speaker }
        : null;

    public static MeetingEvent Decode(ReadOnlySpan<byte> utf8) => TheuthJson.Deserialize<MeetingEvent>(utf8);
}

/// <summary><c>AgentEvent::SubagentSpawned { parent, child, name, cap? }</c> (crates/agent/src/lib.rs:2706).</summary>
public sealed record SubagentSpawned
{
    public string Parent { get; init; } = "";
    public string Child { get; init; } = "";
    public string Name { get; init; } = "";
    public string? Cap { get; init; }
}

/// <summary><c>tasks::activity::ActivityAppended { entry }</c> (activity.rs:131); the row is kept raw here — Core's <c>ActivityEntry</c> decodes it.</summary>
public sealed record ActivityAppended
{
    public JsonValue Entry { get; init; } = JsonValue.Null.Instance;
}
