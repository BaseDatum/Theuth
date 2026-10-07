using System.Text.Json.Serialization;
using Theuth.Companion.Rpc;

namespace Theuth.Companion.Core;

/// <summary><c>tasks::activity::ActivityEntry</c> (<c>crates/tasks/src/activity.rs</c>); <see cref="Kind"/> is kept as its wire word.</summary>
public sealed record ActivityEntry
{
    [JsonRequired] public long Id { get; init; }
    [JsonRequired] public ulong TsMs { get; init; }
    [JsonRequired] public string Kind { get; init; } = "";

    /// <summary><c>agent:&lt;name&gt;</c> | <c>user</c> | <c>user@&lt;client&gt;</c> (E4) | <c>system</c>.</summary>
    [JsonRequired] public string Actor { get; init; } = "";

    [JsonRequired] public string Subject { get; init; } = "";
    [JsonRequired] public string Summary { get; init; } = "";
    public double? Cost { get; init; }
}

/// <summary><c>activity_list</c>'s one positional argument (<c>kinds: null</c> = every kind); nulls are omitted, keys sorted.</summary>
public sealed record ActivityFilter
{
    public ValueList<string>? Kinds { get; init; }
    public uint? Limit { get; init; }
    public ulong? SinceMs { get; init; }
}

public enum ReceiptKind { Ask, Answer }

/// <summary>One line in the Done zone: an ask handed to theuth, or a keystroke answer the engine recorded (E4).</summary>
public sealed record Receipt
{
    /// <summary><c>activity:&lt;row id&gt;</c> for engine entries, <c>ask:&lt;uuid&gt;</c> for local asks.</summary>
    public required string Id { get; init; }

    public required ReceiptKind Kind { get; init; }
    public required DateTimeOffset At { get; init; }

    /// <summary>"theuth" for asks; the subject's agent or kind word for answers.</summary>
    public required string Who { get; init; }

    public required string Text { get; set; }

    /// <summary><c>theuth://c/&lt;session&gt;</c> target when known.</summary>
    public string? Session { get; init; }

    public const string CompanionActor = "user@companion";

    /// <summary>Keystroke answers only: <c>actor = user@companion</c> (W0 E4). Other actors are not this surface's receipts.</summary>
    public static Receipt? FromActivity(ActivityEntry e)
    {
        if (e.Actor != CompanionActor) return null;
        return new Receipt
        {
            Id = $"activity:{e.Id}",
            Kind = ReceiptKind.Answer,
            At = DateTimeOffset.FromUnixTimeMilliseconds((long)e.TsMs),
            Who = e.Subject,
            Text = e.Summary,
        };
    }
}

/// <summary>The last 24 h of receipts, newest first, deduplicated by id; <see cref="Clear"/> hides one for good (E on the Done zone).</summary>
public sealed class ReceiptModel
{
    /// <summary>Seconds.</summary>
    public const double Window = 24 * 60 * 60;

    private readonly List<Receipt> entries = new();
    private readonly HashSet<string> cleared = new(StringComparer.Ordinal);

    public IReadOnlyList<Receipt> Entries => entries;
    public IReadOnlySet<string> Cleared => cleared;

    /// <summary>Insert or replace (same id), keep newest first, drop what is older than the window.</summary>
    public void Add(Receipt r, DateTimeOffset now)
    {
        entries.RemoveAll(x => x.Id == r.Id);
        entries.Add(r);
        Prune(now);
    }

    public void Merge(IEnumerable<ActivityEntry> activity, DateTimeOffset now)
    {
        foreach (var e in activity)
        {
            if (Receipt.FromActivity(e) is not { } r) continue;
            entries.RemoveAll(x => x.Id == r.Id);
            entries.Add(r);
        }
        Prune(now);
    }

    /// <summary>Streaming reply text for an ask receipt.</summary>
    public void Update(string id, string text)
    {
        var r = entries.FirstOrDefault(x => x.Id == id);
        if (r is not null) r.Text = text;
    }

    public void Clear(string id)
    {
        cleared.Add(id);
        entries.RemoveAll(x => x.Id == id);
    }

    public void Prune(DateTimeOffset now)
    {
        var cutoff = now.AddSeconds(-Window);
        entries.RemoveAll(x => x.At < cutoff || cleared.Contains(x.Id));
        entries.Sort((a, b) =>
        {
            int c = b.At.CompareTo(a.At);
            return c != 0 ? c : string.CompareOrdinal(b.Id, a.Id);
        });
    }

    public bool IsEmpty => entries.Count == 0;
    public int Count => entries.Count;
}
