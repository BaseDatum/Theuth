using Theuth.Companion.Rpc;

namespace Theuth.Companion.Core;

/// <summary>The kind of a rail glyph (130 §4.1 table). Colour and shape both carry the state.</summary>
public enum MarkStateKind { NeedsYou, Working, Idle, Finished, Error, Lapsed }

/// <summary>One glyph's state; <see cref="NeedsYou(AttentionKind)"/> carries the item's kind.</summary>
public readonly record struct MarkState(MarkStateKind Kind, AttentionKind? Item = null)
{
    public static MarkState NeedsYou(AttentionKind kind) => new(MarkStateKind.NeedsYou, kind);
    public static MarkState Working => new(MarkStateKind.Working);
    public static MarkState Idle => new(MarkStateKind.Idle);
    public static MarkState Finished => new(MarkStateKind.Finished);
    public static MarkState Error => new(MarkStateKind.Error);
    public static MarkState Lapsed => new(MarkStateKind.Lapsed);
}

public sealed record Mark
{
    public required string Id { get; init; }
    public required MarkState State { get; init; }

    /// <summary>Agent name, or the item's title when it has no agent.</summary>
    public required string Label { get; init; }

    /// <summary>One line for the tooltip / Narrator: "&lt;label&gt;, &lt;state&gt;, &lt;since&gt;".</summary>
    public required string Detail { get; init; }

    /// <summary>Waiting since (needs-you, finished) or running for (working).</summary>
    public DateTimeOffset? Since { get; init; }

    /// <summary>The attention item behind a needs-you / finished mark.</summary>
    public string? ItemId { get; init; }

    public string? Session { get; init; }

    public bool IsNeedsYou => State.Kind == MarkStateKind.NeedsYou;
}

public sealed record Rail
{
    /// <summary>Zone 1, oldest first.</summary>
    public required ValueList<Mark> NeedsYou { get; init; }

    /// <summary>Zone 2: working · idle · finished · error · lapsed.</summary>
    public required ValueList<Mark> Agents { get; init; }

    /// <summary>Marks beyond <see cref="RailModel.MaxMarks"/>, shown as <c>+N</c>.</summary>
    public required int Overflow { get; init; }

    public required int NeedsYouCount { get; init; }
    public required int WorkingCount { get; init; }

    /// <summary>Nothing needs you and nothing is working: the 4 epx sliver after 8 s.</summary>
    public bool Quiet => NeedsYouCount == 0 && WorkingCount == 0;

    public IReadOnlyList<Mark> Marks => NeedsYou.Concat(Agents).ToList();

    public static Rail Empty { get; } = new() { NeedsYou = new(), Agents = new(), Overflow = 0, NeedsYouCount = 0, WorkingCount = 0 };
}

/// <summary>
/// Settings › Rail "Rail presence" (W6, R2-3). <see cref="SliverWhenQuiet"/> is W1's behaviour; <see cref="AlwaysSliver"/>
/// leaves the tray's gold ●N as the only indicator and the rail at 4 epx unless the pointer is over it.
/// </summary>
public enum RailPresence { Expanded, SliverWhenQuiet, AlwaysSliver }

public static class RailPresenceText
{
    public static string Title(this RailPresence p) => p switch
    {
        RailPresence.Expanded => "Expanded",
        RailPresence.SliverWhenQuiet => "Sliver when quiet",
        RailPresence.AlwaysSliver => "Always a sliver",
        _ => "",
    };

    /// <summary>The persisted word (Swift's raw value): <c>expanded</c> · <c>sliverWhenQuiet</c> · <c>alwaysSliver</c>.</summary>
    public static string RawValue(this RailPresence p) => p switch
    {
        RailPresence.Expanded => "expanded",
        RailPresence.SliverWhenQuiet => "sliverWhenQuiet",
        RailPresence.AlwaysSliver => "alwaysSliver",
        _ => "",
    };

    public static RailPresence? FromRaw(string? raw) => raw switch
    {
        "expanded" => RailPresence.Expanded,
        "sliverWhenQuiet" => RailPresence.SliverWhenQuiet,
        "alwaysSliver" => RailPresence.AlwaysSliver,
        _ => null,
    };
}

/// <summary>The collapse rule the rail controller runs; pure so the three presences are testable. Times in seconds.</summary>
public static class RailCollapse
{
    public const double Width = 26;
    public const double SliverWidth = 4;

    /// <summary>Quiet this long before the sliver (<see cref="RailPresence.SliverWhenQuiet"/>).</summary>
    public const double QuietDelay = 8;

    /// <summary>The pointer left an <see cref="RailPresence.AlwaysSliver"/> rail: it stays expanded this long.</summary>
    public const double HoverLinger = 1;

    /// <summary><c>null</c>: stay expanded. Else collapse after this many seconds (0 = now). <paramref name="afterHover"/>: the pointer just left.</summary>
    public static double? CollapseDelay(RailPresence p, bool quiet, bool hovering, bool afterHover)
    {
        if (hovering) return null;
        return p switch
        {
            RailPresence.Expanded => null,
            RailPresence.SliverWhenQuiet => quiet ? QuietDelay : null,
            RailPresence.AlwaysSliver => afterHover ? HoverLinger : 0,
            _ => null,
        };
    }

    /// <summary>A collapsed rail expands when something needs you or works only in <see cref="RailPresence.SliverWhenQuiet"/>.</summary>
    public static bool ExpandsOnActivity(RailPresence p) => p == RailPresence.SliverWhenQuiet;

    public static double WidthFor(bool collapsed) => collapsed ? SliverWidth : Width;
}

public static class RailModel
{
    public const int MaxMarks = 12;

    /// <summary><c>railState(attention, agents, activity, now)</c> of 130 §5.2.</summary>
    public static Rail Derive(IReadOnlyList<AttentionItem> items, IReadOnlyList<AgentSummary> agents, IReadOnlyList<AgentInfo> activity, DateTimeOffset now)
    {
        var needs = items.Where(i => i.Kind.NeedsYou)
            .OrderBy(i => i.WaitingSince).ThenBy(i => i.Id, StringComparer.Ordinal)
            .Select(item => new Mark
            {
                Id = item.Id,
                State = MarkState.NeedsYou(item.Kind),
                Label = item.Agent ?? item.Title,
                Detail = $"{item.Agent ?? item.Title}, {item.Kind.RawValue}, {Ago(item.WaitingSince, now)}",
                Since = item.WaitingSince,
                ItemId = item.Id,
                Session = item.Session,
            })
            .ToList();

        var zone = new List<Mark>();
        var deskSessions = new HashSet<string>(StringComparer.Ordinal);
        foreach (var a in agents)
        {
            if (a.Desk is { } d) deskSessions.Add(d);
            MarkState state;
            switch (a.Status)
            {
                case AgentStatus.Working: state = MarkState.Working; break;
                case AgentStatus.Idle or AgentStatus.Blocked: state = MarkState.Idle; break;
                case AgentStatus.Paused: state = MarkState.Lapsed; break;
                case AgentStatus.Unreachable or AgentStatus.Invalid: state = MarkState.Error; break;
                default: continue; // proposed: already a `hire:` item in zone 1
            }
            zone.Add(new Mark
            {
                Id = $"agent:{a.File.Name}",
                State = state,
                Label = a.File.Name,
                Detail = $"{a.File.Name}, {Word(state)}, {a.File.Model ?? ""}",
                Since = null,
                ItemId = null,
                Session = a.Desk,
            });
        }
        foreach (var s in activity)
        {
            if (!s.Activity.Live || deskSessions.Contains(s.Id)) continue;
            var state = s.Activity.State.Kind switch
            {
                AgentStateKind.Running => MarkState.Working,
                AgentStateKind.Error => MarkState.Error,
                _ => MarkState.Idle, // idle · awaiting · approval · cancelled
            };
            var label = s.Agent ?? s.Model;
            var since = s.Activity.TurnStartedAt;
            zone.Add(new Mark
            {
                Id = $"session:{s.Id}",
                State = state,
                Label = label,
                Detail = $"{label}, {Word(state)}" + (since is { } t ? $", running {Ago(t, now)}" : ""),
                Since = since,
                ItemId = null,
                Session = s.Id,
            });
        }
        foreach (var f in items.Where(i => i.Kind == AttentionKind.Finished).OrderBy(i => i.WaitingSince))
        {
            zone.Add(new Mark
            {
                Id = f.Id,
                State = MarkState.Finished,
                Label = f.Agent ?? f.Title,
                Detail = $"{f.Agent ?? f.Title}, finished, {Ago(f.WaitingSince, now)}",
                Since = f.WaitingSince,
                ItemId = f.Id,
                Session = f.Session,
            });
        }
        zone = zone.Select((m, offset) => (m, offset))
            .OrderBy(x => Rank(x.m.State)).ThenBy(x => x.offset)
            .Select(x => x.m)
            .ToList();

        int working = zone.Count(m => m.State == MarkState.Working);
        int total = needs.Count + zone.Count;
        int overflow = Math.Max(0, total - MaxMarks);
        var keepNeeds = needs.Take(MaxMarks).ToValueList();
        var keepZone = zone.Take(Math.Max(0, MaxMarks - keepNeeds.Count)).ToValueList();
        return new Rail
        {
            NeedsYou = keepNeeds,
            Agents = keepZone,
            Overflow = overflow,
            NeedsYouCount = needs.Count,
            WorkingCount = working,
        };
    }

    private static int Rank(MarkState s) => s.Kind switch
    {
        MarkStateKind.Working => 0,
        MarkStateKind.Idle => 1,
        MarkStateKind.Finished => 2,
        MarkStateKind.Error => 3,
        MarkStateKind.Lapsed => 4,
        _ => -1, // needs-you
    };

    private static string Word(MarkState s) => s.Kind switch
    {
        MarkStateKind.Working => "working",
        MarkStateKind.Idle => "idle",
        MarkStateKind.Finished => "finished",
        MarkStateKind.Error => "error",
        MarkStateKind.Lapsed => "lapsed",
        _ => s.Item?.RawValue ?? "",
    };

    /// <summary>"40 s" · "4 m" · "2 h" · "3 d".</summary>
    public static string Ago(DateTimeOffset from, DateTimeOffset now)
    {
        long s = Math.Max(0, (long)(now - from).TotalSeconds);
        if (s < 60) return $"{s} s";
        if (s < 3600) return $"{s / 60} m";
        if (s < 86400) return $"{s / 3600} h";
        return $"{s / 86400} d";
    }
}
