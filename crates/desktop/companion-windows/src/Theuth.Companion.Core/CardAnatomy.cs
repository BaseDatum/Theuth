using System.Text;
using Theuth.Companion.Rpc;

namespace Theuth.Companion.Core;

/// <summary>
/// E6's <c>AttentionItem.context</c> as text (Revision 1 #5: <c>request</c> · <c>last_reply</c>). The wire DTO is WW1's;
/// the Store hands the model this pair, or null when the engine has no E6 and the interim join applies (§6 E6).
/// </summary>
public sealed record ItemContextText(string? Request, string? LastReply);

/// <summary>Header row: ‹name› · position dots · ‹J K› · ? · ✕ esc (D6).</summary>
public sealed record CardHeader(string Name, int Position, int Count)
{
    public bool CanPrevious => Position > 1;
    public bool CanNext => Position < Count;

    /// <summary>VoiceOver / Narrator: "card 2 of 3".</summary>
    public string Spoken => Count == 0 ? Name : $"{Name}, card {Position} of {Count}";
}

/// <summary>Status line (Revision 1 #6): ‹state› · ‹relative time› · ‹project›, the harness as the icon.</summary>
public sealed record CardStatusLine(string Harness, string StateWord, string Age, string? Project, bool SessionGone)
{
    public string Text
    {
        get
        {
            if (SessionGone) return "session gone";
            var s = $"{StateWord} · {Age}";
            return Project is { Length: > 0 } p ? $"{s} · {p}" : s;
        }
    }
}

/// <summary>Context bubbles: the user's request (faience, right) and the agent's last message (dark, left), ≤ 160 chars each.</summary>
public sealed record CardBubbles(string? Request, string? LastReply)
{
    public bool Any => Request is { Length: > 0 } || LastReply is { Length: > 0 };
}

/// <summary>Footer verbs (Revision 1 #2–3): Stop only while the agent runs or waits; Clear on finished/info; Snooze on asks.</summary>
public enum FooterVerb { Stop, Clear, Snooze }

/// <summary>The reply row: [▣ P] [field] [🎙 V] [↑ ⏎].</summary>
public sealed record ReplyRowSpec(string Placeholder, bool Point, bool Talk, bool Send)
{
    public const string KeyCap = "R"; // Revision 1 #18: the cap reads R; space is the alias
}

/// <summary>An option line "1 ‹label› (Recommended) →".</summary>
public sealed record OptionLine(CardOption Option)
{
    public string Text => Option.Recommended ? $"{Option.Label} (Recommended)" : Option.Label;
    public string KeyCap => Option.Key.ToString();
}

/// <summary>A key hint of the legend row under the card.</summary>
public sealed record KeyHint(string Cap, string Label);

/// <summary>What a card of each kind shows (Swift <c>CardAnatomy</c>): header · status · bubbles · question · options · reply row · footer.</summary>
public sealed record CardAnatomy
{
    public required Card Card { get; init; }
    public required CardHeader Header { get; init; }
    public required CardStatusLine Status { get; init; }
    public required CardBubbles Bubbles { get; init; }
    public required string Question { get; init; }
    public string? Why { get; init; }
    public string? IfWrong { get; init; }
    public required ValueList<OptionLine> Options { get; init; }
    public int MoreOptions { get; init; }
    public required ReplyRowSpec Reply { get; init; }
    public required ValueList<FooterVerb> Footer { get; init; }
    public required ValueList<KeyHint> Keys { get; init; }

    public bool Stops => Footer.Contains(FooterVerb.Stop);
    public bool Clears => Footer.Contains(FooterVerb.Clear);
    public bool Snoozes => Footer.Contains(FooterVerb.Snooze);

    /// <summary>"Next steps" heads the finished card's options (<c>CardModel.swift:122-129</c>).</summary>
    public bool NextStepsHeader => Card.Item.Kind == AttentionKind.Finished && Options.Count > 0;
}

public static class CardAnatomyModel
{
    public const int BubbleMax = 160;

    /// <summary>"just now" under this many seconds (<c>RailModel.ago</c>).</summary>
    public const int JustNowSeconds = 45;

    /// <summary>§3.4 / Revision 1 #8.</summary>
    public const string EmptyTitle = "Nothing needs you.";
    public const string EmptyBody = "Suspiciously quiet — questions and finished work land here.";
    public const string Connecting = "Connecting to the engine…";
    public const string InboxTitle = "Inbox";

    /// <summary>
    /// <paramref name="agent"/>: the item's session in <c>list_activity</c> (null = unknown here); <paramref name="session"/>:
    /// its <c>list_sessions</c> row; <paramref name="context"/>: E6, or null for the interim join (title → request, last → reply).
    /// </summary>
    public static CardAnatomy Make(Card card, int position, int count, AgentInfo? agent, SessionRow? session, ItemContextText? context,
        DateTimeOffset now, bool sessionGone = false)
    {
        var item = card.Item;
        string harness = agent?.Agent ?? session?.Agent ?? "theuth";
        string? project = agent?.Project ?? session?.Project ?? AgentsModel.LastComponent(session?.Cwd);
        var status = new CardStatusLine(harness, StateWord(item.Kind), Age(item.WaitingSince, now), project ?? (harness == "theuth" ? null : harness), sessionGone);

        string? request = context?.Request ?? session?.Title;
        string? reply = context?.LastReply ?? (agent?.Last is { Length: > 0 } l ? l : null);
        // Never echo the question itself as the request bubble (an untitled session's title is its first input).
        if (request is not null && string.Equals(request.Trim(), card.Question.Trim(), StringComparison.Ordinal)) request = null;
        var bubbles = item.Session is null ? new CardBubbles(null, null) : new CardBubbles(Clip(request), Clip(reply));

        bool stops = StopsAgent(item, agent, sessionGone);
        var footer = new List<FooterVerb>();
        if (stops) footer.Add(FooterVerb.Stop);
        if (item.Kind.NeedsYou) footer.Add(FooterVerb.Snooze);
        else if (card.Clearable) footer.Add(FooterVerb.Clear);

        bool replies = card.ReplyAction("") is not null;
        var reply_ = new ReplyRowSpec(
            Placeholder: item.Kind == AttentionKind.Ask ? "Type your answer…" : card.ReplyLabel(),
            Point: replies && item.Session is not null && !sessionGone,
            Talk: replies && !sessionGone,
            Send: replies);

        var keys = new List<KeyHint>();
        foreach (var o in card.Options) keys.Add(new KeyHint(o.Key.ToString(), o.Label));
        if (card.Recommended is not null) keys.Add(new KeyHint("⏎", "recommended"));
        if (count > 1) keys.Add(new KeyHint("J K", "next · previous"));
        if (replies)
        {
            keys.Add(new KeyHint("R", "reply"));
            keys.Add(new KeyHint("V", "talk"));
            if (reply_.Point) keys.Add(new KeyHint("P", "point"));
        }
        if (stops) keys.Add(new KeyHint("X", "stop agent"));
        if (footer.Contains(FooterVerb.Clear)) keys.Add(new KeyHint("E", "clear"));
        if (footer.Contains(FooterVerb.Snooze)) keys.Add(new KeyHint("S", "snooze 30 m"));
        if (item.Kind == AttentionKind.Decision) keys.Add(new KeyHint("O", "board"));
        else if (card.Options.Count > 0 && !card.Clearable && item.Kind != AttentionKind.Review) keys.Add(new KeyHint("O", "open"));
        if (item.Kind == AttentionKind.Review || item.Kind == AttentionKind.Finished) keys.Add(new KeyHint("D", "diff"));
        if (card.SessionUrl is not null) keys.Add(new KeyHint("Ctrl+O", "session"));
        keys.Add(new KeyHint("?", "keys"));
        keys.Add(new KeyHint("Esc", "close"));

        return new CardAnatomy
        {
            Card = card,
            Header = new CardHeader(card.Who, position, count),
            Status = status,
            Bubbles = bubbles,
            Question = card.Question,
            Why = card.Why is { Length: > 0 } w ? w : null,
            IfWrong = null,
            Options = card.Options.Select(o => new OptionLine(o)).ToValueList(),
            MoreOptions = card.MoreOptions,
            Reply = reply_,
            Footer = footer.ToValueList(),
            Keys = keys.ToValueList(),
        };
    }

    /// <summary>
    /// Revision 1 #2: X only on cards of RUNNING sessions — ask · approval · decision · blocked · review in flight. A finished
    /// item is a receipt of a clean <c>end_turn</c>: never. When this engine's activity knows the session, it decides
    /// (live and running/awaiting/approval); when it does not, the kind does.
    /// </summary>
    public static bool StopsAgent(AttentionItem item, AgentInfo? agent, bool sessionGone = false)
    {
        if (sessionGone || item.Session is null) return false;
        if (!(item.Kind.Tag is AttentionKindTag.Ask or AttentionKindTag.Approval or AttentionKindTag.Decision
              or AttentionKindTag.Blocked or AttentionKindTag.Review)) return false;
        if (agent is null) return true;
        if (!agent.Activity.Live) return false;
        return agent.Activity.State.Kind is AgentStateKind.Running or AgentStateKind.Awaiting or AgentStateKind.Approval;
    }

    /// <summary>Which verb X · E · S triggers on this card, if any (the key is inert otherwise).</summary>
    public static FooterVerb? VerbFor(CardAnatomy a, char key) => char.ToUpperInvariant(key) switch
    {
        'X' => a.Stops ? FooterVerb.Stop : null,
        'E' => a.Clears ? FooterVerb.Clear : null,
        'S' => a.Snoozes ? FooterVerb.Snooze : null,
        _ => null,
    };

    /// <summary>The footer pill's text: "Stop agent" · "Clear" · "Snooze" with its key cap.</summary>
    public static (string Label, string Cap) FooterText(FooterVerb v) => v switch
    {
        FooterVerb.Stop => ("Stop agent", "X"),
        FooterVerb.Clear => ("Clear", "E"),
        FooterVerb.Snooze => ("Snooze", "S"),
        _ => ("", ""),
    };

    /// <summary>The hold pill before <c>cancel(session, "tree")</c>: "Stopping ‹name› · Esc undo" (N children when known).</summary>
    public static string StopPill(string who, int children) => children > 0
        ? $"Stopping {who} and {children} {(children == 1 ? "child" : "children")} · Esc undo"
        : $"Stopping {who} · Esc undo";

    public static string StateWord(AttentionKind kind) => kind.Tag switch
    {
        AttentionKindTag.Finished => "Finished",
        AttentionKindTag.Ask => "Waiting for your answer",
        AttentionKindTag.Approval => "Waiting for approval",
        AttentionKindTag.Decision => "Needs a decision",
        AttentionKindTag.Review => "Ready for review",
        AttentionKindTag.Blocked => "Blocked",
        AttentionKindTag.Hire => "Proposed hire",
        AttentionKindTag.Show => "Shown",
        AttentionKindTag.Resume => "Needs a resume",
        AttentionKindTag.Budget => "Budget",
        AttentionKindTag.Secret => "Needs a secret",
        AttentionKindTag.Handoff => "Handoff",
        AttentionKindTag.Node => "Node",
        _ => kind.RawValue,
    };

    /// <summary>"just now" under 45 s, else <see cref="RailModel.Ago"/> ("2 m", "3 h") — the Mac's <c>RailModel.ago</c>.</summary>
    public static string Age(DateTimeOffset since, DateTimeOffset now)
    {
        var s = (now - since).TotalSeconds;
        return s < JustNowSeconds ? "just now" : RailModel.Ago(since, now);
    }

    /// <summary>≤ 160 chars on a char boundary (never inside a surrogate pair), first line only, "…" when cut.</summary>
    public static string? Clip(string? s, int max = BubbleMax)
    {
        if (s is null) return null;
        int nl = s.IndexOf('\n');
        var line = (nl >= 0 ? s[..nl] : s).Trim();
        if (line.Length == 0) return null;
        if (line.Length <= max) return line;
        var sb = new StringBuilder();
        foreach (var r in line.EnumerateRunes())
        {
            if (sb.Length + r.Utf16SequenceLength > max - 1) break;
            sb.Append(r.ToString());
        }
        return sb.ToString().TrimEnd() + "…";
    }
}
