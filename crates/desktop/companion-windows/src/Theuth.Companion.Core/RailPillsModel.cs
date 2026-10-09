using Theuth.Companion.Rpc;

namespace Theuth.Companion.Core;

/// <summary>The four pills of the 149 rail (design 149 §3.1, D1; Windows form §7.4 #2), top to bottom.</summary>
public enum RailPillKind { Inbox, Agents, Actions, More }

/// <summary>One dot on the agents pill: a live agent (or a session that needs you). <see cref="Hollow"/> = owned elsewhere (E7 fallback).</summary>
public sealed record AgentDot(string Id, MarkState State, string Label, string Detail, string? Session, bool Hollow)
{
    public bool Gold => State.Kind == MarkStateKind.NeedsYou;
    public bool Spins => State.Kind == MarkStateKind.Working;
}

/// <summary>What the rail draws (Swift <c>RailPillsModel</c>): the inbox count, the agent dots, the actions pill's recording state.</summary>
public sealed record RailPills
{
    /// <summary>Gold badge on the inbox pill when &gt; 0 (gold only then, 149 §3.1).</summary>
    public required int InboxCount { get; init; }

    public bool InboxGold => InboxCount > 0;

    /// <summary>false → a carnelian 4 epx dot on the inbox pill (today's <c>RailPanel.swift:148-156</c>).</summary>
    public required bool Connected { get; init; }

    /// <summary>≤ <see cref="RailPillsModel.MaxDots"/> dots, needs-you first, then working · idle · finished · error · lapsed.</summary>
    public required ValueList<AgentDot> Dots { get; init; }

    /// <summary>Dots beyond the cap, drawn as <c>+N</c> under the last dot.</summary>
    public required int Overflow { get; init; }

    /// <summary>The Ask bar (or a row mic) records: the actions pill shows the level bars instead of the mic glyph.</summary>
    public required bool Recording { get; init; }

    /// <summary>The last <c>DictationLevel</c> packet, oldest first (one bar per value, newest right, D8).</summary>
    public required ValueList<float> Levels { get; init; }

    /// <summary>The hovered dot's id (grows 1.4×, 120 ms) or null.</summary>
    public string? Hovered { get; init; }

    /// <summary>Nothing needs you and nothing works: the collapse rule (<see cref="RailCollapse"/>) may fold the stack to the sliver.</summary>
    public required bool Quiet { get; init; }

    public static RailPills Empty { get; } = new()
    {
        InboxCount = 0, Connected = false, Dots = new(), Overflow = 0, Recording = false, Levels = new(), Quiet = true,
    };
}

/// <summary>Geometry of the pill stack in effective pixels (D1: 52 pt capsules, 12 pt gaps; the Mac's points are Windows' epx).</summary>
public readonly record struct PillSlot(RailPillKind Kind, double Top, double Height);

public static class RailPillsModel
{
    public const double PillWidth = 52;
    public const double Gap = 12;
    public const double Radius = 26;
    public const double MoreHeight = 44;
    public const double DotSize = 12;
    public const double DotGap = 12;
    public const double DotPad = 14;
    public const int MaxDots = 12;

    /// <summary>Hover grow (<c>scaleEffect(1.4)</c>, 120 ms).</summary>
    public const double HoverScale = 1.4;
    public const int HoverMs = 120;

    /// <summary>The working arc's period (0.38 s); under Reduce Motion the rail keeps the opacity beat instead.</summary>
    public const double SpinnerSeconds = 0.38;

    /// <summary>Room around the stack so a hovered dot's 1.4× grow is not clipped by the window: the window is <see cref="PillWidth"/> + 2 × this.</summary>
    public const double Margin = 6;

    public static double WindowWidth => PillWidth + 2 * Margin;

    /// <summary>
    /// Pills from the rail (<see cref="RailModel.Derive"/> unchanged): one dot per agent — gold when an item needs you for that
    /// session (a needs-you mark and an agent mark of one session fold into one gold dot), spinner when working, hollow when
    /// <paramref name="remoteSessions"/> owns it. Needs-you first, then the agent zone's order.
    /// </summary>
    public static RailPills Derive(Rail rail, bool connected, bool recording, IReadOnlyList<float>? levels,
        string? hovered = null, IReadOnlySet<string>? remoteSessions = null)
    {
        var dots = new List<AgentDot>();
        var seen = new HashSet<string>(StringComparer.Ordinal);
        foreach (var m in rail.NeedsYou)
        {
            if (m.Session is { } s && !seen.Add(s)) continue;
            dots.Add(new AgentDot(m.Id, m.State, m.Label, m.Detail, m.Session, false));
        }
        foreach (var m in rail.Agents)
        {
            if (m.Session is { } s && !seen.Add(s)) continue; // the needs-you dot already stands for this session
            bool hollow = m.Session is { } rs && remoteSessions is not null && remoteSessions.Contains(rs);
            dots.Add(new AgentDot(m.Id, m.State, m.Label, m.Detail, m.Session, hollow));
        }
        int overflow = rail.Overflow + Math.Max(0, dots.Count - MaxDots);
        return new RailPills
        {
            InboxCount = rail.NeedsYouCount,
            Connected = connected,
            Dots = dots.Take(MaxDots).ToValueList(),
            Overflow = overflow,
            Recording = recording,
            Levels = (levels ?? Array.Empty<float>()).ToValueList(),
            Hovered = hovered,
            Quiet = rail.Quiet,
        };
    }

    /// <summary>The agents pill grows with its dots (≥ a 52 epx circle; the pill stays when there are no agents, 149 §3.1).</summary>
    public static double AgentsPillHeight(int dots, int overflow = 0)
    {
        if (dots <= 0) return PillWidth;
        double h = DotPad * 2 + dots * DotSize + (dots - 1) * DotGap + (overflow > 0 ? DotGap : 0);
        return Math.Max(PillWidth, h);
    }

    /// <summary>Top and height of every pill, top to bottom, from y = 0 (the window's top edge).</summary>
    public static IReadOnlyList<PillSlot> Layout(int dots, int overflow = 0)
    {
        var slots = new List<PillSlot>(4);
        double y = 0;
        void Add(RailPillKind k, double h)
        {
            slots.Add(new PillSlot(k, y, h));
            y += h + Gap;
        }
        Add(RailPillKind.Inbox, PillWidth);
        Add(RailPillKind.Agents, AgentsPillHeight(dots, overflow));
        Add(RailPillKind.Actions, PillWidth);
        Add(RailPillKind.More, MoreHeight);
        return slots;
    }

    public static double StackHeight(int dots, int overflow = 0)
    {
        var l = Layout(dots, overflow);
        var last = l[^1];
        return last.Top + last.Height;
    }

    /// <summary>The dot's scale: 1.4 while hovered (also under Reduce Motion — a scale without animation is still a state).</summary>
    public static double DotScale(bool hovered) => hovered ? HoverScale : 1;

    /// <summary>Narrator / tooltip line for a pill.</summary>
    public static string Describe(RailPillKind kind, RailPills p) => kind switch
    {
        RailPillKind.Inbox => p.InboxCount == 0 ? "Inbox, nothing needs you" : p.InboxCount == 1 ? "Inbox, 1 needs you" : $"Inbox, {p.InboxCount} need you",
        RailPillKind.Agents => p.Dots.Count == 0 ? "Agents, none running" : $"Agents, {p.Dots.Count + p.Overflow}",
        RailPillKind.Actions => p.Recording ? "Listening" : "Ask theuth, talk, point",
        RailPillKind.More => "More",
        _ => "",
    };
}
