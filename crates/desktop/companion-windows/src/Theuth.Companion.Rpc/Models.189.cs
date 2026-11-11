using System.Text.Json.Serialization;

namespace Theuth.Companion.Rpc;

/// <summary>
/// 189w: one entry of <c>load_session</c>'s <c>history</c> (<c>HistoryEntry</c>, crates/agent/src/lib.rs:3003, an enum tagged by
/// <c>role</c>). The Ask catch-up reads only the role, the text and the turn: <c>user</c> / <c>assistant</c> / <c>reasoning</c> /
/// <c>tool</c> / <c>compacted</c> / <c>error</c> / <c>interrupted</c> rows share this flat shape, every other field is ignored.
/// </summary>
public sealed record TranscriptEntry
{
    public string Role { get; init; } = "";

    /// <summary>The row's text; null for a tool card (it has none).</summary>
    public string? Text { get; init; }

    /// <summary>
    /// A user row that opens a turn: its own seq. An assistant row: the seq of the user row that opened the turn it belongs to
    /// (design 29 §4.3). Null for a steer, a harness note and every other role. <c>TurnStarted.turn_seq</c> is the same number.
    /// </summary>
    public long? Turn { get; init; }
}

/// <summary>
/// 189w: <c>load_session</c>'s reply (<c>Loaded</c>, crates/agent/src/lib.rs:1264) — the part the Ask catch-up reads: the last
/// entries of the transcript (an in-flight round is not stored yet) and whether a turn is in flight.
/// </summary>
public sealed record SessionTail
{
    public ValueList<TranscriptEntry> History { get; init; } = new();

    /// <summary>A turn is in flight; its events keep streaming.</summary>
    public bool Running { get; init; }
}