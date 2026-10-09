using Theuth.Companion.Rpc;

namespace Theuth.Companion.Core;

/// <summary>
/// The Core's view of a <c>list_sessions</c> row (149 §3.2, D2) — the fields the agents panel reads. The wire DTO
/// (<c>SessionSummary</c>, WW1's) is mapped onto this by the Store so the model stays testable without it.
/// <paramref name="Owner"/>: the mesh engine id that holds the live actor; <c>""</c> on a lone engine.
/// </summary>
public sealed record SessionRow(
    string Id, string Title, string Owner, bool Running, string? Agent = null, string? Cwd = null, string? Project = null,
    string? Parent = null, string? Desk = null, string? Name = null, DateTimeOffset? UpdatedAt = null, bool Archived = false);

/// <summary>A paired engine from <c>mesh_status.peers</c>: id → display name.</summary>
public sealed record HostPeer(string Id, string Name);

/// <summary>The dot of an agents-panel row (130 §4.1 shapes + the hollow remote dot of 149 §3.2).</summary>
public enum AgentRowState { NeedsYou, Working, Idle, Done, Error, Lapsed, Remote }

/// <summary>One row: status · harness icon · name · ⑂N · what it is doing · mic · ✕ (Swift <c>AgentRow</c>).</summary>
public sealed record AgentRow
{
    public required string Id { get; init; }
    public string? Session { get; init; }
    public required string Name { get; init; }

    /// <summary><c>AgentInfo.agent</c> / <c>SessionSummary.agent</c>; <c>theuth</c> when null (D18).</summary>
    public required string Harness { get; init; }

    public required AgentRowState State { get; init; }
    public int Children { get; init; }

    /// <summary>The muted "what it is doing" line: <c>AgentInfo.last</c>, else "running 4 m", else "".</summary>
    public string Last { get; init; } = "";

    /// <summary>Engine id of the owner (<c>""</c>/self = this machine).</summary>
    public required string Host { get; init; }
    public required string HostName { get; init; }

    /// <summary>This engine has live state for the row (false for remote rows and sleeping desks).</summary>
    public required bool Live { get; init; }

    /// <summary>A turn is in flight (running · awaiting · approval): ✕ is enabled.</summary>
    public required bool Running { get; init; }
    public int Depth { get; init; }

    /// <summary>The roster agent whose desk this is ("theuth · desk"); ✕ pauses it instead of cancelling.</summary>
    public string? DeskName { get; init; }

    /// <summary>The needs-you attention item joined by session, if any.</summary>
    public string? ItemId { get; init; }
    public string? Project { get; init; }

    public bool Remote => State == AgentRowState.Remote;

    /// <summary>Hold the mic → dictate into this session (ruling 3): needs a session this engine owns.</summary>
    public bool CanTalk => !Remote && Session is not null;

    /// <summary>✕ stop: a running session (<c>cancel(session, "tree")</c>) or an awake desk (<c>agent_pause</c>).</summary>
    public bool CanStop => !Remote && (DeskName is not null ? Live : Running && Session is not null);

    /// <summary>Why the mic / ✕ are disabled: "open it on ‹node›" · "nothing is running".</summary>
    public string? DisabledReason => Remote ? $"open it on {HostName}" : CanStop ? null : "nothing is running";

    /// <summary>The hold label: "Stopping ‹name› and N children" · "Pausing ‹desk›" (the receipt row appends "Esc undo · N" itself).</summary>
    public string StopLabel => DeskName is { } d
        ? $"Pausing {d}"
        : Children > 0
            ? $"Stopping {Name} and {Children} {(Children == 1 ? "child" : "children")}"
            : $"Stopping {Name}";

    /// <summary>The hold pill as the design words it: "Stopping ‹name› and N children · Esc undo".</summary>
    public string StopPill => $"{StopLabel} · Esc undo";

    /// <summary>Narrator line.</summary>
    public string Detail
    {
        get
        {
            var word = State switch
            {
                AgentRowState.NeedsYou => "needs you",
                AgentRowState.Working => "working",
                AgentRowState.Idle => "idle",
                AgentRowState.Done => "done",
                AgentRowState.Error => "error",
                AgentRowState.Lapsed => "paused",
                AgentRowState.Remote => $"on {HostName}",
                _ => "",
            };
            var s = $"{Name}, {word}";
            if (Children > 0) s += $", {Children} children";
            if (Last.Length > 0) s += $", {Last}";
            return s;
        }
    }
}

public sealed record HostGroup(string Id, string Title, bool Local, ValueList<AgentRow> Rows);

/// <summary>The agents panel's content (Swift <c>AgentsModel</c> output).</summary>
public sealed record AgentsPanel
{
    public required ValueList<HostGroup> Groups { get; init; }

    public IReadOnlyList<AgentRow> Rows => Groups.SelectMany(g => g.Rows).ToList();

    public bool IsEmpty => Groups.All(g => g.Rows.Count == 0);

    /// <summary>Ctrl+Shift+A opens scrolled to the first working row (§8); the first row when none works.</summary>
    public int FirstWorking
    {
        get
        {
            var rows = Rows;
            for (int i = 0; i < rows.Count; i++)
            {
                if (rows[i].State == AgentRowState.Working) return i;
            }
            return 0;
        }
    }

    public int IndexOf(string rowId)
    {
        var rows = Rows;
        for (int i = 0; i < rows.Count; i++)
        {
            if (rows[i].Id == rowId) return i;
        }
        return -1;
    }

    public int IndexOfSession(string session)
    {
        var rows = Rows;
        for (int i = 0; i < rows.Count; i++)
        {
            if (rows[i].Session == session) return i;
        }
        return -1;
    }

    public static AgentsPanel Empty { get; } = new() { Groups = new() };
}

public static class AgentsModel
{
    /// <summary>§3.2 empty state.</summary>
    public const string EmptyTitle = "No agents running.";
    public const string EmptyBody = "Desks appear here when they wake; Ask theuth starts one.";
    public const string Connecting = "Connecting to the engine…";

    /// <summary>§7.4 #8: the global desktop-apps microphone switch is off.</summary>
    public const string MicDenied = "Microphone is off for desktop apps — Settings › Privacy › Microphone";

    /// <summary>The local group's title (the Mac says "this Mac").</summary>
    public const string LocalTitle = "this PC";

    /// <summary>A remote session counts as a row while it was touched this recently (there is no live state for it here, D2).</summary>
    public static readonly TimeSpan RemoteRecent = TimeSpan.FromMinutes(10);

    /// <summary>
    /// Rows and host groups from <c>list_activity</c> ∪ <c>agents_list</c> ∪ <c>list_sessions</c> + <c>mesh_status</c> (D2).
    /// Local = <see cref="SessionRow.Owner"/> <c>""</c> or <paramref name="selfId"/>; one group per other owner, named through
    /// <paramref name="peers"/>. Roots first, live children indented under their parent; remote rows hollow, never a spinner.
    /// </summary>
    public static AgentsPanel Derive(IReadOnlyList<AgentInfo> activity, IReadOnlyList<AgentSummary> agents, IReadOnlyList<SessionRow> sessions,
        IReadOnlyList<AttentionItem> items, string selfId, string selfName, IReadOnlyList<HostPeer> peers, DateTimeOffset now, string? meshName = null)
    {
        var bySession = sessions.ToDictionary(s => s.Id, StringComparer.Ordinal);
        var needs = new Dictionary<string, string>(StringComparer.Ordinal);
        foreach (var i in items)
        {
            if (i.Kind.NeedsYou && i.Session is { } s && !needs.ContainsKey(s)) needs[s] = i.Id;
        }
        var deskBySession = new Dictionary<string, AgentSummary>(StringComparer.Ordinal);
        foreach (var a in agents)
        {
            if (a.Desk is { } d) deskBySession[d] = a;
        }

        // Local rows from this engine's supervisor.
        var local = new List<AgentRow>();
        var live = activity.Where(a => a.Activity.Live).ToList();
        var liveIds = new HashSet<string>(live.Select(a => a.Id), StringComparer.Ordinal);
        AgentRow RowFor(AgentInfo a, int depth)
        {
            bySession.TryGetValue(a.Id, out var row);
            deskBySession.TryGetValue(a.Id, out var desk);
            string name = desk is not null ? $"{desk.File.Name} · desk"
                : a.Name.Length > 0 ? a.Name
                : row?.Title is { Length: > 0 } t ? t
                : a.Agent ?? a.Model;
            var kind = a.Activity.State.Kind;
            AgentRowState state =
                needs.ContainsKey(a.Id) || kind is AgentStateKind.Awaiting or AgentStateKind.Approval ? AgentRowState.NeedsYou
                : kind == AgentStateKind.Running ? AgentRowState.Working
                : kind == AgentStateKind.Error ? AgentRowState.Error
                : AgentRowState.Idle;
            string last = a.Last.Length > 0 ? FirstLine(a.Last)
                : a.Activity.TurnStartedAt is { } t0 && kind == AgentStateKind.Running ? $"running {RailModel.Ago(t0, now)}"
                : "";
            return new AgentRow
            {
                Id = $"session:{a.Id}",
                Session = a.Id,
                Name = name,
                Harness = a.Agent ?? row?.Agent ?? "theuth",
                State = state,
                Children = a.Activity.Children,
                Last = last,
                Host = selfId,
                HostName = selfName,
                Live = true,
                Running = kind is AgentStateKind.Running or AgentStateKind.Awaiting or AgentStateKind.Approval,
                Depth = depth,
                DeskName = desk?.File.Name,
                ItemId = needs.TryGetValue(a.Id, out var item) ? item : null,
                Project = a.Project ?? row?.Project ?? LastComponent(row?.Cwd),
            };
        }
        var roots = live.Where(a => a.Parent.Length == 0 || !liveIds.Contains(a.Parent))
            .Select(a => RowFor(a, 0))
            .OrderBy(r => Rank(r.State)).ThenBy(r => r.Name, StringComparer.OrdinalIgnoreCase).ToList();
        var childrenOf = live.Where(a => a.Parent.Length > 0 && liveIds.Contains(a.Parent))
            .GroupBy(a => a.Parent, StringComparer.Ordinal)
            .ToDictionary(g => g.Key, g => g.ToList(), StringComparer.Ordinal);
        void Walk(AgentRow parent, int depth)
        {
            local.Add(parent);
            if (parent.Session is null || !childrenOf.TryGetValue(parent.Session, out var kids)) return;
            foreach (var k in kids.OrderBy(a => a.StartedAt ?? DateTimeOffset.MinValue)) Walk(RowFor(k, depth + 1), depth + 1);
        }
        foreach (var r in roots) Walk(r, 0);

        // Desks with no live session: still rows ("theuth · desk"), from the roster's status, needs-you first.
        var sleeping = new List<AgentRow>();
        foreach (var a in agents)
        {
            if (a.Status == AgentStatus.Proposed) continue; // a `hire:` item, not an agent
            if (a.Desk is { } d && liveIds.Contains(d)) continue;
            var state = a.Status switch
            {
                AgentStatus.Working => AgentRowState.Working,
                AgentStatus.Paused => AgentRowState.Lapsed,
                AgentStatus.Unreachable or AgentStatus.Invalid => AgentRowState.Error,
                _ => AgentRowState.Idle,
            };
            string? item = a.Desk is { } ds && needs.TryGetValue(ds, out var it) ? it : null;
            if (item is not null) state = AgentRowState.NeedsYou;
            sleeping.Add(new AgentRow
            {
                Id = $"agent:{a.File.Name}",
                Session = a.Desk,
                Name = $"{a.File.Name} · desk",
                Harness = "theuth",
                State = state,
                Last = a.Error ?? (a.OpenTasks > 0 ? $"{a.OpenTasks} open {(a.OpenTasks == 1 ? "task" : "tasks")}" : ""),
                Host = selfId,
                HostName = selfName,
                Live = a.Desk is not null && a.Status != AgentStatus.Paused,
                Running = a.Status == AgentStatus.Working,
                DeskName = a.File.Name,
                ItemId = item,
            });
        }

        local.AddRange(sleeping.OrderBy(r => Rank(r.State)).ThenBy(r => r.Name, StringComparer.OrdinalIgnoreCase));
        var groups = new List<HostGroup> { new(selfId, LocalTitle, true, local.ToValueList()) };

        // Remote rows: owned elsewhere, recent, hollow — no spinner is ever faked (D2).
        var peerName = peers.ToDictionary(p => p.Id, p => p.Name, StringComparer.Ordinal);
        var remote = sessions
            .Where(s => s.Owner.Length > 0 && s.Owner != selfId && !s.Archived && s.Desk is null)
            .Where(s => s.Running || (s.UpdatedAt is { } u && now - u <= RemoteRecent))
            .GroupBy(s => s.Owner, StringComparer.Ordinal)
            .OrderBy(g => peerName.TryGetValue(g.Key, out var n) ? n : g.Key, StringComparer.OrdinalIgnoreCase);
        foreach (var g in remote)
        {
            string node = peerName.TryGetValue(g.Key, out var n) ? n : Short(g.Key);
            var rows = g.OrderByDescending(s => s.Running).ThenByDescending(s => s.UpdatedAt ?? DateTimeOffset.MinValue)
                .Select(s => new AgentRow
                {
                    Id = $"session:{s.Id}",
                    Session = s.Id,
                    Name = s.Name is { Length: > 0 } nm ? nm : s.Title.Length > 0 ? s.Title : Short(s.Id),
                    Harness = s.Agent ?? "theuth",
                    State = AgentRowState.Remote,
                    Last = $"on {node}",
                    Host = g.Key,
                    HostName = node,
                    Live = false,
                    Running = false,
                    Depth = s.Parent is null ? 0 : 1,
                    Project = s.Project ?? LastComponent(s.Cwd),
                })
                .ToValueList();
            groups.Add(new HostGroup(g.Key, $"{node} · on 🌐 {meshName ?? "mesh"}", false, rows));
        }
        return new AgentsPanel { Groups = groups.ToValueList() };
    }

    /// <summary>The counts the local group header can show: "3 working · 1 needs you · 2 on the mesh".</summary>
    public static string Counts(AgentsPanel p)
    {
        var rows = p.Rows;
        var parts = new List<string>();
        int working = rows.Count(r => r.State == AgentRowState.Working);
        int needs = rows.Count(r => r.State == AgentRowState.NeedsYou);
        int remote = rows.Count(r => r.Remote);
        if (needs > 0) parts.Add(needs == 1 ? "1 needs you" : $"{needs} need you");
        if (working > 0) parts.Add($"{working} working");
        if (remote > 0) parts.Add($"{remote} on the mesh");
        return string.Join(" · ", parts);
    }

    private static int Rank(AgentRowState s) => s switch
    {
        AgentRowState.NeedsYou => 0,
        AgentRowState.Working => 1,
        AgentRowState.Idle => 2,
        AgentRowState.Done => 3,
        AgentRowState.Error => 4,
        AgentRowState.Lapsed => 5,
        _ => 6,
    };

    internal static string FirstLine(string s) => CardAnatomyModel.Clip(s) ?? "";

    internal static string? LastComponent(string? path)
    {
        if (string.IsNullOrEmpty(path)) return null;
        var t = path.TrimEnd('/', '\\');
        int i = Math.Max(t.LastIndexOf('/'), t.LastIndexOf('\\'));
        var last = i >= 0 ? t[(i + 1)..] : t;
        return last.Length == 0 ? null : last;
    }

    private static string Short(string id) => id.Length <= 8 ? id : id[..8];
}