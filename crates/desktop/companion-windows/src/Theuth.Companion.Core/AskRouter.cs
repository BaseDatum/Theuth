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

    /// <summary>
    /// <c>send_input</c> went out: collect the desk's reply from now until its turn finishes. <paramref name="askText"/> is what was
    /// sent — the catch-up finds the ask's own user row in the transcript by it (189w D2).
    /// </summary>
    public void Sent(string session, string askText = "") => Reply = new AskReply(session) { AskText = askText };

    /// <summary>
    /// Feed <c>TextDelta</c> / <c>TurnFinished</c> / <c>Error</c> from the event stream; true when the receipt text changed.
    /// 134 W6 item 7: the desk's <c>Error</c> event (the engine's "non-fatal or fatal error text for the UI" — a model resolver
    /// error, a refused tool) or a <c>TurnFinished</c> whose <c>stop_reason</c> is terminal (<see cref="IsFailure"/>: <c>error</c> ·
    /// <c>cancelled</c> · <c>suspended</c> · <c>timed_out</c> · <c>interrupted</c>, 189w D3) ends the reply as <see cref="AskReply.Failed"/>,
    /// text or no text — otherwise the receipt would say "sent — waiting for the desk" forever. Any other stop reason is a normal finish.
    /// 189w D2: the Store no longer ends an ask on a normal finish from here — it catches up from the transcript first
    /// (<see cref="AskCatchUp"/>) — but the rule stays for a caller that feeds the stream alone.
    /// </summary>
    public bool Observe(ReplyEvent e)
    {
        if (Reply is not { } r || r.Finished) return false;
        switch (e)
        {
            case ReplyEvent.TextDelta d:
                if (d.Session != r.Session) return false;
                r.Text += d.Text;
                break;
            case ReplyEvent.Error err:
                if (err.Session != r.Session) return false;
                r.Error = err.Message;
                r.Finished = true;
                break;
            case ReplyEvent.TurnFinished f:
                if (f.Session != r.Session) return false;
                if (IsFailure(f.StopReason))
                {
                    r.Error ??= FailureReason(f.StopReason!);
                    r.Finished = true;
                    break;
                }
                // A turn that ended before anything streamed is the one we steered into, not the reply.
                if (r.Text.Length == 0) return false;
                r.Finished = true;
                break;
            default:
                return false;
        }
        return true;
    }

    /// <summary>
    /// The engine's non-provider stop reasons that mean "no reply is coming" (loop_.rs: cancelled · timed_out · interrupted · error,
    /// and <c>suspended</c> — a turn cut by the engine's drain, loop_.rs:522, 5956: it resumes on the next start, it does not answer now).
    /// Anything else — <c>end_turn</c>, <c>stop</c>, a provider's own word — is a normal finish.
    /// </summary>
    public static bool IsFailure(string? stopReason) => stopReason is "error" or "cancelled" or "suspended" or "timed_out" or "interrupted";

    /// <summary>The readable reason a terminal <paramref name="stopReason"/> gives (the receipt shows it after "theuth couldn't reply — ").</summary>
    public static string FailureReason(string stopReason) => stopReason switch
    {
        "error" => "the desk's turn ended with an error",
        "timed_out" => "the turn was timed out",
        "interrupted" => "the turn was interrupted",
        "suspended" => "the turn was suspended — theuth is restarting",
        _ => "the turn was cancelled",
    };
}

public abstract record ReplyEvent
{
    private ReplyEvent() { }

    public sealed record TextDelta(string Session, string Text) : ReplyEvent;

    /// <summary><c>TurnFinished</c>; <paramref name="StopReason"/> is the provider's reason or <c>cancelled</c> / <c>error</c> (null from older decoders).</summary>
    public sealed record TurnFinished(string Session, string? StopReason = null) : ReplyEvent;

    /// <summary>The engine's <c>Error { session, turn, message }</c> event.</summary>
    public sealed record Error(string Session, string Message) : ReplyEvent;
}

/// <summary>The desk's streamed answer; the receipt row shows <see cref="FirstTwoLines"/>.</summary>
public sealed class AskReply(string session)
{
    /// <summary>The receipt line for a failed turn, before the clipped reason.</summary>
    public const string FailedPrefix = "theuth couldn't reply — ";

    /// <summary>How much of the error's first line the receipt shows.</summary>
    public const int FailedClip = 120;

    public string Session { get; } = session;
    public string Text { get; set; } = "";
    public bool Finished { get; set; }

    /// <summary>Set when the turn failed (<see cref="ReplyEvent.Error"/> or a failing stop reason): the reason, unclipped.</summary>
    public string? Error { get; set; }

    public bool Failed => Error is not null;

    /// <summary>189w D2: what the ask sent — the catch-up finds the ask's own user row in the transcript by this text.</summary>
    public string AskText { get; set; } = "";

    /// <summary>
    /// 189w D2: the ask's turn id — the seq of the user row that opened it (<c>TurnStarted.turn_seq</c>, or the row a catch-up
    /// matched by text). Assistant rows of the transcript carry the same number.
    /// </summary>
    public long? Turn { get; set; }

    /// <summary>189w D2: the turn finished and the transcript holds no assistant text for it; the receipt says <see cref="AskCatchUp.NoReplyText"/> and keeps Open.</summary>
    public bool NoReply { get; set; }

    /// <summary>189w D6: characters that arrived as <c>TextDelta</c>s (the log's "first TextDelta" line and the finish line).</summary>
    public int StreamedChars { get; set; }

    /// <summary>189w D6: when <c>send_input</c> returned — the first delta's latency is measured from here.</summary>
    public DateTimeOffset? SentAt { get; set; }

    /// <summary>189w D2: a <c>TurnFinished</c> (normal) was seen and a catch-up decides it; no second one is started for the same finish.</summary>
    public bool SawFinish { get; set; }

    /// <summary>192w P3: the model's reasoning for this ask (<c>ReasoningDelta</c>) — the "Thinking" section until the first answer text, then "Thought for N s".</summary>
    public ThinkingState Thinking { get; } = new();

    /// <summary>Records the transcript's text as THE reply (it replaces whatever streamed) and ends the ask.</summary>
    public void Complete(string text)
    {
        Text = text;
        Finished = true;
    }

    /// <summary>Ends the ask as failed with <paramref name="why"/> (shown after <see cref="FailedPrefix"/>).</summary>
    public void Fail(string why)
    {
        Error = why;
        Finished = true;
    }

    /// <summary>Ends the ask: the turn finished and left nothing to show (<see cref="AskCatchUp.NoReplyText"/>).</summary>
    public void FinishWithoutReply()
    {
        NoReply = true;
        Finished = true;
    }

    private static readonly char[] NewLines = { '\n', '\r', '\u000B', '\u000C', '\u0085', '\u2028', '\u2029' };

    /// <summary>
    /// The first two non-empty lines, markdown bullets and heading marks stripped, joined with " — "; for a failed turn
    /// <see cref="FailedPrefix"/> + the error's first line clipped to <see cref="FailedClip"/> chars.
    /// </summary>
    public string FirstTwoLines
    {
        get
        {
            if (Error is { } error)
            {
                var first = error.Split(NewLines, StringSplitOptions.RemoveEmptyEntries).Select(TrimSpaces).FirstOrDefault(l => l.Length > 0) ?? "unknown error";
                return FailedPrefix + (first.Length > FailedClip ? first[..(FailedClip - 1)].TrimEnd() + "…" : first);
            }
            if (NoReply) return AskCatchUp.NoReplyText;
            // 192w: a picture fence is "(diagram)" here, closed or still arriving — the line never carries its source.
            var lines = PictureFence.Flatten(Text).Split(NewLines, StringSplitOptions.RemoveEmptyEntries)
                .Select(l => TrimSpaces(AnswerMarkdown.Plain(StripMarker(TrimSpaces(l)), open: !Finished)))
                .Where(l => l.Length > 0)
                .Take(2);
            return string.Join(" — ", lines);
        }
    }

    /// <summary>
    /// A line without its block marker — heading hashes, a bullet, a quote mark, each only as a MARKER (followed by a space), so the
    /// opening <c>**</c> of a bold start is not eaten ("**Done** — x" stays a bold start; the old <c>TrimStart('*')</c> left "Done** — x").
    /// A line of nothing but rule characters (<c>---</c>, <c>***</c>) is empty.
    /// </summary>
    private static string StripMarker(string line)
    {
        var l = line;
        while (true)
        {
            int h = 0;
            while (h < l.Length && h < 6 && l[h] == '#') h++;
            if (h > 0 && h < l.Length && l[h] == ' ') { l = TrimSpaces(l[(h + 1)..]); continue; }
            if (l.Length >= 2 && l[0] is '-' or '*' or '•' or '>' && l[1] == ' ') { l = TrimSpaces(l[2..]); continue; }
            break;
        }
        return l.Length > 0 && l.All(c => c is '-' or '*' or '_' or '=' or '#' or '>' or ' ') ? "" : l;
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