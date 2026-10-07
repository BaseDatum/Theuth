using Theuth.Companion.Rpc;

namespace Theuth.Companion.Core;

/// <summary><c>mesh_status</c> subset: this engine, the lease holder, peer names (<c>crates/mesh/src/types.rs</c>).</summary>
public sealed record MeshStatus
{
    public sealed record Peer
    {
        [System.Text.Json.Serialization.JsonRequired] public string Id { get; init; } = "";
        [System.Text.Json.Serialization.JsonRequired] public string Name { get; init; } = "";
        [System.Text.Json.Serialization.JsonRequired] public bool Reachable { get; init; }
    }

    public sealed record Lease
    {
        public string? Holder { get; init; }
    }

    [System.Text.Json.Serialization.JsonRequired] public string SelfId { get; init; } = "";
    [System.Text.Json.Serialization.JsonRequired] public string SelfName { get; init; } = "";
    [System.Text.Json.Serialization.JsonRequired] public ValueList<Peer> Peers { get; init; } = new();

    /// <summary>The wire's <c>lease</c> (a member cannot share its nested record's name in C#).</summary>
    [System.Text.Json.Serialization.JsonRequired, System.Text.Json.Serialization.JsonPropertyName("lease")]
    public Lease LeaseInfo { get; init; } = new();

    /// <summary><c>desk_open</c> is answered only by the lease holder (<c>api.rs</c>, <c>roster/src/lib.rs</c>).</summary>
    public bool IsHub => LeaseInfo.Holder == SelfId;

    /// <summary>The hub's display name, or null while no engine holds the lease.</summary>
    public string? HubName
    {
        get
        {
            if (LeaseInfo.Holder is not { } h) return null;
            if (h == SelfId) return SelfName;
            return Peers.FirstOrDefault(p => p.Id == h)?.Name ?? (h.Length <= 8 ? h : h[..8]);
        }
    }
}

/// <summary>Where theuth's desk stands for this companion (Swift <c>AskRouter.Desk</c>).</summary>
public abstract record AskDesk
{
    private AskDesk() { }

    public sealed record Unknown : AskDesk
    {
        public static readonly Unknown Instance = new();
    }

    public sealed record Ready(string Session, bool Hub) : AskDesk;

    /// <summary>Not reachable from here; the reason is shown in the receipt row.</summary>
    public sealed record Unavailable(string Reason) : AskDesk;
}

/// <summary>What <see cref="AskRouter.Plan"/> decides (Swift <c>AskRouter.Plan</c>).</summary>
public abstract record AskPlan
{
    private AskPlan() { }

    public sealed record OpenHere : AskPlan
    {
        public static readonly OpenHere Instance = new();
    }

    public sealed record Use(string Session) : AskPlan;

    public sealed record Refuse(string Reason) : AskPlan;
}

/// <summary>Where a request goes (D7, R4): always theuth's desk, resolved once per session. No agent picker.</summary>
public sealed class AskRouter
{
    public AskDesk Desk { get; private set; } = AskDesk.Unknown.Instance;

    /// <summary>The reply being collected since the last send; null until a send.</summary>
    public AskReply? Reply { get; private set; }

    public string? DeskSession => Desk is AskDesk.Ready r ? r.Session : null;

    /// <summary>
    /// <c>mesh_status</c> says whether <c>desk_open</c> may be called here. Off the hub the only hope is a replicated
    /// <c>agents_list</c> row carrying theuth's desk (<c>AgentSummary.desk</c>); else the request cannot go anywhere.
    /// </summary>
    public static AskPlan Plan(MeshStatus? mesh, IReadOnlyList<AgentSummary> agents)
    {
        if (mesh is null || mesh.IsHub) return AskPlan.OpenHere.Instance; // no mesh: a single engine is its own hub
        if (agents.FirstOrDefault(a => a.File.Name == "theuth")?.Desk is { } s) return new AskPlan.Use(s);
        return new AskPlan.Refuse($"theuth's desk lives on {mesh.HubName ?? "another engine"} — open theuth there");
    }

    public void Resolved(string session, bool hub) => Desk = new AskDesk.Ready(session, hub);

    public void Refused(string reason) => Desk = new AskDesk.Unavailable(reason);

    /// <summary>The desk closed or the connection was lost: resolve again on the next send.</summary>
    public void Forget() => Desk = AskDesk.Unknown.Instance;

    /// <summary><c>send_input</c> went out: collect the desk's reply from now until its turn finishes.</summary>
    public void Sent(string session) => Reply = new AskReply(session);

    /// <summary>Feed <c>TextDelta</c> / <c>TurnFinished</c> from the event stream; true when the receipt text changed.</summary>
    public bool Observe(ReplyEvent e)
    {
        if (Reply is not { } r || r.Finished) return false;
        switch (e)
        {
            case ReplyEvent.TextDelta d:
                if (d.Session != r.Session) return false;
                r.Text += d.Text;
                break;
            case ReplyEvent.TurnFinished f:
                // A turn that ended before anything streamed is the one we steered into, not the reply.
                if (f.Session != r.Session || r.Text.Length == 0) return false;
                r.Finished = true;
                break;
            default:
                return false;
        }
        return true;
    }
}

public abstract record ReplyEvent
{
    private ReplyEvent() { }

    public sealed record TextDelta(string Session, string Text) : ReplyEvent;

    public sealed record TurnFinished(string Session) : ReplyEvent;
}

/// <summary>The desk's streamed answer; the receipt row shows <see cref="FirstTwoLines"/>.</summary>
public sealed class AskReply(string session)
{
    public string Session { get; } = session;
    public string Text { get; set; } = "";
    public bool Finished { get; set; }

    private static readonly char[] NewLines = { '\n', '\r', '\u000B', '\u000C', '\u0085', '\u2028', '\u2029' };

    /// <summary>The first two non-empty lines, markdown bullets and heading marks stripped, joined with " — ".</summary>
    public string FirstTwoLines
    {
        get
        {
            var lines = Text.Split(NewLines, StringSplitOptions.RemoveEmptyEntries)
                .Select(l => TrimSpaces(TrimSpaces(l).TrimStart('#', '-', '*', '>', ' ')))
                .Where(l => l.Length > 0)
                .Take(2);
            return string.Join(" — ", lines);
        }
    }

    /// <summary>Swift's <c>.whitespaces</c>: spaces and tabs (Unicode Zs + tab), not newlines.</summary>
    private static string TrimSpaces(string s)
    {
        int a = 0, b = s.Length;
        while (a < b && IsSpace(s[a])) a++;
        while (b > a && IsSpace(s[b - 1])) b--;
        return s[a..b];
    }

    private static bool IsSpace(char c) => c == '\t' || char.GetUnicodeCategory(c) == System.Globalization.UnicodeCategory.SpaceSeparator;
}

/// <summary>Everything W3 keeps on the store: routing, the Done zone, the R field.</summary>
public sealed class AskModel
{
    public AskRouter Router { get; set; } = new();
    public ReceiptModel Receipts { get; set; } = new();

    /// <summary>The item whose card shows the R field.</summary>
    public string? ReplyFor { get; set; }

    /// <summary>The receipt of the ask in flight; its text follows the desk's stream.</summary>
    public string? Current { get; set; }

    /// <summary>W6: the live dictation feeding the Ask bar's field.</summary>
    public AskDictation Dictation { get; set; } = new();
}

public static class CardReplies
{
    /// <summary>
    /// R, reply in words: the kind's own path. Ask → a free-text <see cref="Answer"/> (one question only — multi-question
    /// asks go to theuth); decision → <c>task_decide(decision, text)</c>; approval → <c>resolve_approval(Deny)</c> +
    /// <c>steer(session, note)</c> (R1-5); review → <c>task_move(id, "todo", note)</c>; finished → <c>send_input(session, text)</c>.
    /// Null when the kind takes no words.
    /// </summary>
    public static CardAction? ReplyAction(this Card card, string text)
    {
        var item = card.Item;
        switch (item.Kind.Tag)
        {
            case AttentionKindTag.Ask:
                if (item.Ask is not { } ask || ask.Questions.Count != 1) return null;
                return new CardAction.AnswerAsk(ask.Id, ValueList.Of<Answer>(new Answer.Text(text)));
            case AttentionKindTag.Decision:
                return item.Decision is { } d ? new CardAction.Decide(d, text) : null;
            case AttentionKindTag.Approval:
                return item.Approval is { } a ? new CardAction.DenyWithNote(a, item.Session, text) : null;
            case AttentionKindTag.Review:
                return item.Task is { } t ? new CardAction.ReviewSendBackNote(t, text) : null;
            case AttentionKindTag.Finished:
                return item.Session is { } s ? new CardAction.SendNextStep(s, text) : null;
            default:
                return null;
        }
    }

    /// <summary>What R shows above the field.</summary>
    public static string ReplyLabel(this Card card) => card.Item.Kind.Tag switch
    {
        AttentionKindTag.Approval => "Deny with a note (R1-5: a reply denies)",
        AttentionKindTag.Review => "Send back with a note",
        AttentionKindTag.Finished => $"Tell {card.Who} what next",
        _ => $"Reply to {card.Who}",
    };
}
