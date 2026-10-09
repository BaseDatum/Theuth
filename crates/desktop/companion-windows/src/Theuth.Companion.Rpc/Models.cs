using System.Text;
using System.Text.Json;
using System.Text.Json.Serialization;

namespace Theuth.Companion.Rpc;

// Hand-written mirrors of the Rust wire shapes the companion uses (design 130 §5.6, design 134 §5.3), ported from
// SWIFT/TheuthKit/Models.swift. Only the fields the companion reads are declared; unknown keys are ignored. The
// fixtures under crates/desktop/companion/Tests/TheuthKitTests/Fixtures are the contract test for both companions.
// Decoding follows the Swift Codable rules: a non-optional field is [JsonRequired] (missing or null → error), an
// optional field reads missing/null as null, and Swift's `decodeIfPresent(...) ?? []` keeps an empty default.

// MARK: roster::attention (crates/roster/src/attention.rs)

/// <summary>The tag of an <see cref="AttentionKind"/>; <see cref="Other"/> carries the unknown word.</summary>
public enum AttentionKindTag
{
    Approval, Decision, Hire, Blocked, Review, Budget, Node, Handoff, Secret, Ask, Show, Resume,
    /// <summary>The W0 E3 addition.</summary>
    Finished,
    /// <summary>Anything newer than this companion.</summary>
    Other,
}

/// <summary><c>AttentionKind</c>, snake_case on the wire; <c>finished</c> is W0 E3; any other word decodes to <c>Other(word)</c>.</summary>
[JsonConverter(typeof(AttentionKindConverter))]
public readonly record struct AttentionKind
{
    private AttentionKind(AttentionKindTag tag, string? other)
    {
        Tag = tag;
        OtherWord = other;
    }

    public AttentionKindTag Tag { get; }

    /// <summary>The unknown word of an <c>Other</c> kind; null for the known kinds.</summary>
    public string? OtherWord { get; }

    public static AttentionKind Approval { get; } = new(AttentionKindTag.Approval, null);
    public static AttentionKind Decision { get; } = new(AttentionKindTag.Decision, null);
    public static AttentionKind Hire { get; } = new(AttentionKindTag.Hire, null);
    public static AttentionKind Blocked { get; } = new(AttentionKindTag.Blocked, null);
    public static AttentionKind Review { get; } = new(AttentionKindTag.Review, null);
    public static AttentionKind Budget { get; } = new(AttentionKindTag.Budget, null);
    public static AttentionKind Node { get; } = new(AttentionKindTag.Node, null);
    public static AttentionKind Handoff { get; } = new(AttentionKindTag.Handoff, null);
    public static AttentionKind Secret { get; } = new(AttentionKindTag.Secret, null);
    public static AttentionKind Ask { get; } = new(AttentionKindTag.Ask, null);
    public static AttentionKind Show { get; } = new(AttentionKindTag.Show, null);
    public static AttentionKind Resume { get; } = new(AttentionKindTag.Resume, null);
    public static AttentionKind Finished { get; } = new(AttentionKindTag.Finished, null);

    public static AttentionKind Other(string word) => new(AttentionKindTag.Other, word);

    private static readonly Dictionary<string, AttentionKind> Known = new(StringComparer.Ordinal)
    {
        ["approval"] = Approval, ["decision"] = Decision, ["hire"] = Hire, ["blocked"] = Blocked,
        ["review"] = Review, ["budget"] = Budget, ["node"] = Node, ["handoff"] = Handoff,
        ["secret"] = Secret, ["ask"] = Ask, ["show"] = Show, ["resume"] = Resume, ["finished"] = Finished,
    };

    /// <summary>The wire word → kind; unknown words become <c>Other(word)</c>.</summary>
    public static AttentionKind FromRaw(string word) => Known.TryGetValue(word, out var k) ? k : Other(word);

    /// <summary>The wire word.</summary>
    public string RawValue => Tag switch
    {
        AttentionKindTag.Other => OtherWord ?? "",
        AttentionKindTag.Approval => "approval",
        AttentionKindTag.Decision => "decision",
        AttentionKindTag.Hire => "hire",
        AttentionKindTag.Blocked => "blocked",
        AttentionKindTag.Review => "review",
        AttentionKindTag.Budget => "budget",
        AttentionKindTag.Node => "node",
        AttentionKindTag.Handoff => "handoff",
        AttentionKindTag.Secret => "secret",
        AttentionKindTag.Ask => "ask",
        AttentionKindTag.Show => "show",
        AttentionKindTag.Resume => "resume",
        AttentionKindTag.Finished => "finished",
        _ => "",
    };

    /// <summary>Needs a human action (gold). <c>show</c> is a receipt; <c>finished</c> is the second zone.</summary>
    public bool NeedsYou => Tag is not (AttentionKindTag.Show or AttentionKindTag.Finished or AttentionKindTag.Other);

    public override string ToString() => RawValue;
}

public sealed class AttentionKindConverter : JsonConverter<AttentionKind>
{
    public override AttentionKind Read(ref Utf8JsonReader reader, Type typeToConvert, JsonSerializerOptions options)
    {
        if (reader.TokenType != JsonTokenType.String) throw new JsonException("attention kind: expected a string");
        return AttentionKind.FromRaw(reader.GetString()!);
    }

    public override void Write(Utf8JsonWriter writer, AttentionKind value, JsonSerializerOptions options) =>
        writer.WriteStringValue(value.RawValue);
}

public sealed record AttentionItem
{
    [JsonRequired] public string Id { get; init; } = "";
    [JsonRequired] public AttentionKind Kind { get; init; }
    [JsonRequired] public string Title { get; init; } = "";
    public string? Detail { get; init; }
    [JsonRequired] public ulong AtMs { get; init; }
    public string? Task { get; init; }
    public string? Agent { get; init; }
    public string? Session { get; init; }
    public string? Approval { get; init; }
    public string? Decision { get; init; }
    public string? Packet { get; init; }
    public AskRequested? Ask { get; init; }
    public uint? Turn { get; init; }

    // W0 (optional on older engines):
    public ulong? WaitingSinceMs { get; init; }
    public string? Summary { get; init; }

    [JsonConverter(typeof(NullAsEmptyListConverter<string>))]
    public ValueList<string> NextSteps { get; init; } = new();

    /// <summary>149 E6: the item's conversation (<c>request</c> / <c>last_reply</c>); null on engines before W0 — see Core <c>AttentionContext</c>.</summary>
    public ItemContext? Context { get; init; }

    /// <summary>149 C: the item's priority word (<c>low</c> · <c>normal</c> · <c>high</c> · …; kept verbatim) — null when the engine has none.</summary>
    public string? Priority { get; init; }

    /// <summary>149 C: the engine's rank; 5 when absent (the value for an item without a priority).</summary>
    public uint Rank { get; init; } = 5;

    /// <summary>When the wait began: E2's <c>waiting_since_ms</c>, else <c>at_ms</c>.</summary>
    [JsonIgnore]
    public DateTimeOffset WaitingSince => DateTimeOffset.FromUnixTimeMilliseconds((long)(WaitingSinceMs ?? AtMs));
}

/// <summary>W0 E1 — optional on older engines.</summary>
public sealed record AttentionChanged
{
    [JsonRequired] public uint Count { get; init; }
    [JsonRequired] public ValueList<string> IdsAdded { get; init; } = new();
    [JsonRequired] public ValueList<string> IdsRemoved { get; init; } = new();
    [JsonRequired] public ulong AtMs { get; init; }
}

// MARK: roster::types (crates/roster/src/types.rs)

public enum AgentStatus { Idle, Working, Blocked, Paused, Proposed, Unreachable, Invalid }

public sealed record AgentFile
{
    [JsonRequired] public string Name { get; init; } = "";
    [JsonRequired] public string Title { get; init; } = "";
    public string? Model { get; init; }
    [JsonRequired] public bool Paused { get; init; }
}

public sealed record AgentSummary
{
    [JsonRequired] public AgentFile File { get; init; } = new();
    [JsonRequired] public AgentStatus Status { get; init; }
    public string? Desk { get; init; }
    [JsonRequired] public uint OpenTasks { get; init; }
    [JsonRequired] public double SpentMonth { get; init; }
    public string? Error { get; init; }
}

// MARK: agent (crates/agent/src/lib.rs)

public enum AgentStateKind { Idle, Running, Awaiting, Approval, Error, Cancelled }

/// <summary><c>#[serde(tag = "state", rename_all = "snake_case")]</c>: idle · running · awaiting(from) · approval(tool) · error · cancelled.</summary>
[JsonConverter(typeof(AgentStateConverter))]
public readonly record struct AgentState(AgentStateKind Kind, string? From = null, string? Tool = null)
{
    public static AgentState Idle => new(AgentStateKind.Idle);
    public static AgentState Running => new(AgentStateKind.Running);
    public static AgentState Error => new(AgentStateKind.Error);
    public static AgentState Cancelled => new(AgentStateKind.Cancelled);
    public static AgentState Awaiting(string from) => new(AgentStateKind.Awaiting, From: from);
    public static AgentState Approval(string tool) => new(AgentStateKind.Approval, Tool: tool);

    /// <summary>The wire word.</summary>
    public string Word => Kind switch
    {
        AgentStateKind.Idle => "idle",
        AgentStateKind.Running => "running",
        AgentStateKind.Awaiting => "awaiting",
        AgentStateKind.Approval => "approval",
        AgentStateKind.Error => "error",
        AgentStateKind.Cancelled => "cancelled",
        _ => "",
    };
}

public sealed class AgentStateConverter : JsonConverter<AgentState>
{
    public override AgentState Read(ref Utf8JsonReader reader, Type typeToConvert, JsonSerializerOptions options)
    {
        using var doc = JsonDocument.ParseValue(ref reader);
        var o = doc.RootElement;
        if (o.ValueKind != JsonValueKind.Object || !o.TryGetProperty("state", out var s) || s.ValueKind != JsonValueKind.String)
        {
            throw new JsonException("agent state: expected {\"state\": …}");
        }
        string Field(string name) =>
            o.TryGetProperty(name, out var v) && v.ValueKind == JsonValueKind.String
                ? v.GetString()!
                : throw new JsonException($"agent state: missing {name}");
        return s.GetString() switch
        {
            "idle" => AgentState.Idle,
            "running" => AgentState.Running,
            "awaiting" => AgentState.Awaiting(Field("from")),
            "approval" => AgentState.Approval(Field("tool")),
            "error" => AgentState.Error,
            "cancelled" => AgentState.Cancelled,
            var other => throw new JsonException($"unknown state {other}"),
        };
    }

    public override void Write(Utf8JsonWriter writer, AgentState value, JsonSerializerOptions options)
    {
        writer.WriteStartObject();
        if (value.Kind == AgentStateKind.Awaiting) writer.WriteString("from", value.From);
        writer.WriteString("state", value.Word);
        if (value.Kind == AgentStateKind.Approval) writer.WriteString("tool", value.Tool);
        writer.WriteEndObject();
    }
}

public sealed record SessionActivity
{
    [JsonRequired] public AgentState State { get; init; } = AgentState.Idle;
    public DateTimeOffset? LastActive { get; init; }
    public DateTimeOffset? TurnStartedAt { get; init; }
    [JsonRequired] public ushort Jobs { get; init; }
    [JsonRequired] public ushort Ptys { get; init; }
    [JsonRequired] public ushort Children { get; init; }
    public ushort? Asks { get; init; }
    [JsonRequired] public bool Live { get; init; }

    /// <summary>W0 E2 — optional on older engines.</summary>
    public ulong? WaitingSinceMs { get; init; }
}

public sealed record AgentInfo
{
    [JsonRequired] public string Id { get; init; } = "";
    [JsonRequired] public string Name { get; init; } = "";
    [JsonRequired] public string Parent { get; init; } = "";
    [JsonRequired] public byte Depth { get; init; }
    public string? Agent { get; init; }
    [JsonRequired] public string Model { get; init; } = "";
    [JsonRequired] public AgentState State { get; init; } = AgentState.Idle;
    public DateTimeOffset? StartedAt { get; init; }
    public DateTimeOffset? FinishedAt { get; init; }
    [JsonRequired] public string Last { get; init; } = "";
    public string? Project { get; init; }

    /// <summary>The wire's <c>activity</c>: <c>#[serde(default)]</c>, absent on engines older than 62.</summary>
    [JsonPropertyName("activity")]
    public SessionActivity? WireActivity { get; init; }

    /// <summary><c>activity</c>, or on a pre-62 engine one built from <see cref="State"/> (which duplicates <c>activity.state</c>).</summary>
    [JsonIgnore]
    public SessionActivity Activity
    {
        get => WireActivity ?? new SessionActivity { State = State };
        init => WireActivity = value;
    }
}

// MARK: tools::approval (crates/tools/src/approval.rs)

public enum Risk { ReadOnly, Mutating, Destructive }

/// <summary><c>Allow | Deny</c> only — there is no "allow for this session" on the wire.</summary>
public enum Decision { Allow, Deny }

public static class WireWords
{
    /// <summary><c>read_only</c> · <c>mutating</c> · <c>destructive</c>.</summary>
    public static string RawValue(this Risk risk) => risk switch
    {
        Risk.ReadOnly => "read_only",
        Risk.Mutating => "mutating",
        Risk.Destructive => "destructive",
        _ => "",
    };

    /// <summary><c>allow</c> · <c>deny</c>.</summary>
    public static string RawValue(this Decision decision) => decision == Decision.Allow ? "allow" : "deny";
}

public sealed record ApprovalRequested
{
    [JsonRequired] public string Id { get; init; } = "";
    [JsonRequired] public string Session { get; init; } = "";
    [JsonRequired] public string Tool { get; init; } = "";
    [JsonRequired] public JsonValue Args { get; init; } = JsonValue.Null.Instance;
    [JsonRequired] public Risk Risk { get; init; }
}

public sealed record ApprovalResolved
{
    [JsonRequired] public string Id { get; init; } = "";
    [JsonRequired] public Decision Decision { get; init; }
    [JsonRequired] public bool TimedOut { get; init; }
}

// MARK: tools::ask (crates/tools/src/ask.rs)

public sealed record Choice
{
    [JsonRequired] public string Label { get; init; } = "";
    public string? Description { get; init; }
}

public sealed record Question
{
    [JsonRequired] public string Id { get; init; } = "";

    /// <summary>The wire's <c>question</c> (a member cannot share its record's name in C#).</summary>
    [JsonRequired, JsonPropertyName("question")]
    public string Text { get; init; } = "";

    [JsonConverter(typeof(NullAsEmptyListConverter<Choice>))]
    public ValueList<Choice> Options { get; init; } = new();

    public int? Recommended { get; init; }
}

/// <summary><c>#[serde(tag = "kind", rename_all = "snake_case")]</c>: choice(index) · text · skipped.</summary>
[JsonConverter(typeof(AnswerConverter))]
public abstract record Answer
{
    private Answer() { }

    public sealed record Choice(int Index) : Answer;

    public sealed record Text(string Value) : Answer;

    public sealed record Skipped : Answer
    {
        public static readonly Skipped Instance = new();
    }
}

/// <summary>Encodes like serde with sorted keys: <c>{"index":1,"kind":"choice"}</c>, <c>{"kind":"text","text":"hi"}</c>, <c>{"kind":"skipped"}</c>.</summary>
public sealed class AnswerConverter : JsonConverter<Answer>
{
    public override bool CanConvert(Type typeToConvert) => typeof(Answer).IsAssignableFrom(typeToConvert);

    public override Answer Read(ref Utf8JsonReader reader, Type typeToConvert, JsonSerializerOptions options)
    {
        using var doc = JsonDocument.ParseValue(ref reader);
        var o = doc.RootElement;
        if (o.ValueKind != JsonValueKind.Object || !o.TryGetProperty("kind", out var k) || k.ValueKind != JsonValueKind.String)
        {
            throw new JsonException("answer: expected {\"kind\": …}");
        }
        switch (k.GetString())
        {
            case "choice":
                if (!o.TryGetProperty("index", out var i) || !i.TryGetInt32(out var index)) throw new JsonException("answer: missing index");
                return new Answer.Choice(index);
            case "text":
                if (!o.TryGetProperty("text", out var t) || t.ValueKind != JsonValueKind.String) throw new JsonException("answer: missing text");
                return new Answer.Text(t.GetString()!);
            case "skipped":
                return Answer.Skipped.Instance;
            case var other:
                throw new JsonException($"unknown answer {other}");
        }
    }

    public override void Write(Utf8JsonWriter writer, Answer value, JsonSerializerOptions options)
    {
        writer.WriteStartObject();
        switch (value)
        {
            case Answer.Choice c:
                writer.WriteNumber("index", c.Index);
                writer.WriteString("kind", "choice");
                break;
            case Answer.Text t:
                writer.WriteString("kind", "text");
                writer.WriteString("text", t.Value);
                break;
            default:
                writer.WriteString("kind", "skipped");
                break;
        }
        writer.WriteEndObject();
    }
}

public sealed record AskRequested
{
    [JsonRequired] public string Id { get; init; } = "";
    [JsonRequired] public string Session { get; init; } = "";
    [JsonRequired] public ValueList<Question> Questions { get; init; } = new();
}

public sealed record AskResolved
{
    [JsonRequired] public string Id { get; init; } = "";
    [JsonRequired] public bool Dismissed { get; init; }
    [JsonRequired] public bool TimedOut { get; init; }
}

// MARK: tasks::packet (crates/tasks/src/packet.rs)

public enum PacketKind { Decision, Review }

public sealed record PacketOption
{
    [JsonRequired] public string Label { get; init; } = "";
    public string? Description { get; init; }
}

public sealed record Check
{
    [JsonRequired] public string Text { get; init; } = "";
    [JsonRequired] public bool Done { get; init; }
    public string? Note { get; init; }
}

public sealed record Pointer
{
    [JsonRequired] public string Kind { get; init; } = "";
    [JsonRequired] public string Ref { get; init; } = "";
    [JsonRequired] public string Title { get; init; } = "";
    public string? Note { get; init; }
    public string? Text { get; init; }
}

public sealed record Recommendation
{
    [JsonRequired] public string Text { get; init; } = "";
    public uint? Option { get; init; }
}

public sealed record Packet
{
    [JsonRequired] public string Key { get; init; } = "";
    [JsonRequired] public string Task { get; init; } = "";
    [JsonRequired] public PacketKind Kind { get; init; }
    [JsonRequired] public string By { get; init; } = "user";
    [JsonRequired] public ulong AtMs { get; init; }
    public string? Question { get; init; }
    [JsonRequired] public string Summary { get; init; } = "";
    public string? Board { get; init; }

    [JsonConverter(typeof(NullAsEmptyListConverter<PacketOption>))]
    public ValueList<PacketOption> Options { get; init; } = new();

    public uint? Recommended { get; init; }

    [JsonConverter(typeof(NullAsEmptyListConverter<Pointer>))]
    public ValueList<Pointer> Pointers { get; init; } = new();

    [JsonConverter(typeof(NullAsEmptyListConverter<Check>))]
    public ValueList<Check> Checks { get; init; } = new();

    public Recommendation? Recommendation { get; init; }
    public string? To { get; init; }

    /// <summary>149 C: what goes wrong if the recommendation is wrong (<c>if_wrong</c>); the card's last line.</summary>
    public string? IfWrong { get; init; }

    /// <summary>The pre-pick: the forwarding manager's <c>recommendation.option</c> wins over <c>recommended</c> (103 F2).</summary>
    [JsonIgnore]
    public int? PreferredOption => Recommendation?.Option is uint o ? (int)o : Recommended is uint r ? (int)r : null;
}

// MARK: rpc (crates/rpc/src/api.rs) and the stream

public sealed record PendingPrompts
{
    [JsonRequired] public ValueList<ApprovalRequested> Approvals { get; init; } = new();
    [JsonRequired] public ValueList<AskRequested> Asks { get; init; } = new();
    [JsonRequired] public ValueList<JsonValue> Secrets { get; init; } = new();
}

public sealed record TurnFinished
{
    [JsonRequired] public string Session { get; init; } = "";
    [JsonRequired] public string Turn { get; init; } = "";
    [JsonRequired] public string StopReason { get; init; } = "";
    [JsonRequired] public uint Context { get; init; }
}

public sealed record Lagged
{
    [JsonRequired] public ulong Count { get; init; }
}

/// <summary>
/// One <c>subscribe_events</c> item, keyed by <c>type</c> (Swift <c>WireEvent</c>). <see cref="Payload"/> is the typed
/// shape for the eight kinds the companion reads (<see cref="Rpc.PendingPrompts"/>, <see cref="Rpc.ApprovalRequested"/>,
/// <see cref="Rpc.ApprovalResolved"/>, <see cref="Rpc.AskRequested"/>, <see cref="Rpc.AskResolved"/>,
/// <see cref="Rpc.TurnFinished"/>, <see cref="Rpc.AttentionChanged"/>, <see cref="Rpc.Lagged"/>); everything else is
/// <see cref="Other(string)"/>: the type word with a null payload.
/// </summary>
public sealed record WireEvent(string Type, object? Payload)
{
    public static WireEvent Other(string type) => new(type, null);

    public bool IsOther => Payload is null;

    private static readonly Dictionary<string, Type> Typed = new(StringComparer.Ordinal)
    {
        ["PendingPrompts"] = typeof(PendingPrompts),
        ["ApprovalRequested"] = typeof(ApprovalRequested),
        ["ApprovalResolved"] = typeof(ApprovalResolved),
        ["AskRequested"] = typeof(AskRequested),
        ["AskResolved"] = typeof(AskResolved),
        ["TurnFinished"] = typeof(TurnFinished),
        ["AttentionChanged"] = typeof(AttentionChanged),
        ["Lagged"] = typeof(Lagged),
        // 149 WW1: the agents panel, the island and the voice key read these (every field optional: never throws).
        ["SubagentSpawned"] = typeof(SubagentSpawned),
        ["ActivityAppended"] = typeof(ActivityAppended),
        ["DictationStarted"] = typeof(DictationEvent),
        ["DictationLevel"] = typeof(DictationEvent),
        ["DictationText"] = typeof(DictationEvent),
        ["DictationFinished"] = typeof(DictationEvent),
        ["Started"] = typeof(MeetingEvent),
        ["State"] = typeof(MeetingEvent),
        ["Level"] = typeof(MeetingEvent),
        ["Utterance"] = typeof(MeetingEvent),
        ["TopicAdded"] = typeof(MeetingEvent),
        ["TopicRemoved"] = typeof(MeetingEvent),
        ["Brief"] = typeof(MeetingEvent),
        ["Listening"] = typeof(MeetingEvent),
        ["ApprovalWaiting"] = typeof(MeetingEvent),
        ["PostProgress"] = typeof(MeetingEvent),
        ["ReviewReady"] = typeof(MeetingEvent),
        ["Summarized"] = typeof(MeetingEvent),
        ["Deleted"] = typeof(MeetingEvent),
        ["OracleDeciding"] = typeof(MeetingEvent),
        ["OracleDecision"] = typeof(MeetingEvent),
    };

    /// <summary>Decodes one stream item; throws when it has no string <c>type</c> or a typed kind does not decode.</summary>
    public static WireEvent Decode(ReadOnlySpan<byte> utf8)
    {
        var type = TypeOf(utf8) ?? throw new JsonException("event without a type");
        if (!Typed.TryGetValue(type, out var t)) return Other(type);
        var payload = JsonSerializer.Deserialize(utf8, t, TheuthJson.Options) ?? throw new JsonException($"null {type}");
        return new WireEvent(type, payload);
    }

    public static WireEvent Decode(string json) => Decode(Encoding.UTF8.GetBytes(json));

    /// <summary>The item's <c>type</c>, or null when it has none.</summary>
    public static string? TypeOf(ReadOnlySpan<byte> utf8)
    {
        var reader = new Utf8JsonReader(utf8);
        try
        {
            using var doc = JsonDocument.ParseValue(ref reader);
            return doc.RootElement.ValueKind == JsonValueKind.Object
                   && doc.RootElement.TryGetProperty("type", out var t)
                   && t.ValueKind == JsonValueKind.String
                ? t.GetString()
                : null;
        }
        catch (JsonException)
        {
            return null;
        }
    }

    /// <summary>The 13 kinds <c>UI/components/hud/queries.ts</c> re-fetches <c>attention_list</c> on, for engines without E1.</summary>
    public static readonly IReadOnlySet<string> AttentionTriggers = new HashSet<string>(StringComparer.Ordinal)
    {
        "ActivityAppended", "ApprovalRequested", "ApprovalResolved", "ArtifactShown", "AskRequested", "AskResolved",
        "NodeAdded", "NodeOnline", "NodeRemoved", "NodeRevoked", "SecretRequested", "SecretResolved", "Takeover",
    };
}

// MARK: stream items the Swift app decodes ad hoc (Store+Ask.swift); typed here for W4 (design 134 §5.3).

/// <summary>
/// <c>TextDelta</c> / <c>TurnFinished</c> / <c>Error</c> as the Ask bar reads them: the type, the session, the text, the
/// turn's <c>stop_reason</c> (provider reason or <c>cancelled</c> / <c>error</c>) and the <c>Error</c> event's <c>message</c> (134 W6 item 7).
/// </summary>
public sealed record StreamDelta
{
    [JsonRequired] public string Type { get; init; } = "";
    public string? Session { get; init; }
    public string? Text { get; init; }
    [JsonPropertyName("stop_reason")] public string? StopReason { get; init; }
    public string? Message { get; init; }

    public static StreamDelta Decode(ReadOnlySpan<byte> utf8) => TheuthJson.Deserialize<StreamDelta>(utf8);
}

/// <summary>
/// <c>DictationStarted</c> · <c>DictationText</c> · <c>DictationLevel</c> · <c>DictationFinished</c> (design 54 §8, the
/// engine's streamed dictation), the shape <c>Store+Ask.swift</c>'s <c>DictationWire</c> reads.
/// </summary>
public sealed record DictationEvent
{
    public sealed record FinishReason
    {
        [JsonRequired] public string Reason { get; init; } = "";
        public string? Message { get; init; }
    }

    [JsonRequired] public string Type { get; init; } = "";
    public string? Id { get; init; }
    public int? Utterance { get; init; }
    public string? Text { get; init; }

    [JsonPropertyName("final")]
    public bool? IsFinal { get; init; }

    public ValueList<float>? Levels { get; init; }
    public FinishReason? Reason { get; init; }

    public static DictationEvent Decode(ReadOnlySpan<byte> utf8) => TheuthJson.Deserialize<DictationEvent>(utf8);
}
