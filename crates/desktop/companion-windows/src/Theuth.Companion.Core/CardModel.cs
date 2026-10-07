using Theuth.Companion.Rpc;

namespace Theuth.Companion.Core;

/// <summary>What a keystroke commits (after the hold). The app maps each case to exactly one RPC.</summary>
public abstract record CardAction
{
    private CardAction() { }

    public sealed record AnswerAsk(string AskId, ValueList<Answer> Answers) : CardAction;

    public sealed record Decide(string Decision, string Answer) : CardAction;

    public sealed record Approve(string ApprovalId, Decision Decision) : CardAction;

    public sealed record ReviewApprove(string Task) : CardAction;

    /// <summary>Needs a note first; the app collects it and holds <see cref="ReviewSendBackNote"/>.</summary>
    public sealed record ReviewSendBack(string Task) : CardAction;

    public sealed record ReviewSendBackNote(string Task, string Note) : CardAction;

    public sealed record SendNextStep(string Session, string Text) : CardAction;

    public sealed record Dismiss(string Id) : CardAction;

    public sealed record Snooze(string Id) : CardAction;

    /// <summary>W3 R on an approval (R1-5: Decision is Allow|Deny): <c>resolve_approval(Deny)</c> then <c>steer(session, note)</c>.</summary>
    public sealed record DenyWithNote(string ApprovalId, string? Session, string Note) : CardAction;

    /// <summary>W3 Ask bar: <c>send_input(theuth's desk, text)</c> after the hold; no item, so no stale guard.</summary>
    public sealed record AskTheuth(string Text) : CardAction;

    /// <summary>134 W2, 130 §4.2 per-kind table — Hire: <c>agent_approve(name)</c> (the engine stamps the actor, E4).</summary>
    public sealed record HireAgent(string Name) : CardAction;

    /// <summary>134 W2 — Hire → Decline: <c>agent_remove(name, null)</c>.</summary>
    public sealed record DeclineHire(string Name) : CardAction;

    /// <summary>134 W2 — Blocked → Unblock: <c>task_move(task, todo)</c> (the ledger allows blocked → todo).</summary>
    public sealed record Unblock(string Task) : CardAction;

    /// <summary>Not sendable yet: the app must collect the note and hold <see cref="ReviewSendBackNote"/> instead.</summary>
    public bool NeedsNote => this is ReviewSendBack;
}

public sealed record CardOption(int Key, string Label, string? Detail, bool Recommended, CardAction Action);

public sealed record Chip(string Text, bool? Ok);

/// <summary>The focused card, in the order a person decides (130 §4.2): who · how long → question → why → ≤ 3 options.</summary>
public sealed record Card
{
    public required AttentionItem Item { get; init; }
    public required string Who { get; init; }
    public string? Model { get; init; }
    public required string Question { get; set; }
    public string? Why { get; set; }
    public ValueList<CardOption> Options { get; set; } = new();
    public ValueList<Chip> Chips { get; set; } = new();

    /// <summary>Options beyond the first three (shown as "+N more in theuth").</summary>
    public int MoreOptions { get; set; }

    /// <summary>The item can be cleared with E.</summary>
    public bool Clearable { get; set; }

    public CardOption? Recommended => Options.FirstOrDefault(o => o.Recommended);

    public Uri? SessionUrl => Item.Session is { } s ? DeepLink.Session(s) : null;

    public Uri AttentionUrl => DeepLink.Attention(Item.Id);

    /// <summary><paramref name="packet"/> for decisions/reviews (fetched with <c>packet_get</c>), <paramref name="approval"/> for approvals (from the stream).</summary>
    public static Card Make(AttentionItem item, Packet? packet = null, ApprovalRequested? approval = null, string? agentModel = null)
    {
        var card = new Card
        {
            Item = item,
            Who = item.Agent ?? ShortSession(item.Session) ?? "theuth",
            Model = agentModel,
            Question = item.Title,
            Why = item.Detail,
        };
        switch (item.Kind.Tag)
        {
            case AttentionKindTag.Ask:
            {
                if (item.Ask is not { } ask || ask.Questions.Count != 1)
                {
                    card.Why = item.Ask is { } a ? $"{a.Questions.Count} questions — answer them in theuth" : item.Detail;
                    return card;
                }
                var q = ask.Questions[0];
                card.Question = q.Text;
                card.Options = q.Options.Take(3).Select((c, i) => new CardOption(
                    i + 1, c.Label, c.Description, q.Recommended == i,
                    new CardAction.AnswerAsk(ask.Id, ValueList.Of<Answer>(new Answer.Choice(i))))).ToValueList();
                card.MoreOptions = Math.Max(0, q.Options.Count - 3);
                break;
            }
            case AttentionKindTag.Decision:
            {
                if (packet is not { } p || item.Decision is not { } decision) return card;
                card.Question = p.Question ?? item.Title;
                card.Why = p.Summary.Length == 0 ? item.Detail : p.Summary;
                card.Options = p.Options.Take(3).Select((o, i) => new CardOption(
                    i + 1, o.Label, o.Description, p.PreferredOption == i,
                    new CardAction.Decide(decision, o.Label))).ToValueList();
                card.MoreOptions = Math.Max(0, p.Options.Count - 3);
                break;
            }
            case AttentionKindTag.Approval:
            {
                if (item.Approval is not { } id) return card;
                // P13: a destructive call has no pre-pick. "Allow for this session" is not on the wire — left out.
                bool destructive = approval?.Risk == Risk.Destructive;
                if (approval is { } a)
                {
                    var cmd = a.Args["command"]?.AsString is { } c ? $" · {c}" : "";
                    card.Why = $"{a.Tool} · {a.Risk.RawValue()}{cmd}";
                }
                card.Options = ValueList.Of(
                    new CardOption(1, "Allow", null, !destructive, new CardAction.Approve(id, Decision.Allow)),
                    new CardOption(2, "Deny", null, false, new CardAction.Approve(id, Decision.Deny)));
                break;
            }
            case AttentionKindTag.Review:
            {
                if (item.Task is not { } task) return card;
                if (packet is { } p)
                {
                    card.Why = p.Summary.Length == 0 ? item.Detail : p.Summary;
                    card.Chips = p.Checks.Select(c => new Chip(c.Text, c.Done)).ToValueList();
                }
                // P13: failing checks remove the pre-pick.
                bool failing = packet?.Checks.Any(c => !c.Done) ?? false;
                card.Options = ValueList.Of(
                    new CardOption(1, "Approve", null, !failing, new CardAction.ReviewApprove(task)),
                    new CardOption(2, "Send back…", "with a note", false, new CardAction.ReviewSendBack(task)));
                break;
            }
            case AttentionKindTag.Finished:
            {
                card.Why = item.Summary;
                card.Clearable = true;
                if (item.Session is { } s)
                {
                    card.Options = item.NextSteps.Take(2).Select((t, i) => new CardOption(
                        i + 1, t, null, i == 0, new CardAction.SendNextStep(s, t))).ToValueList();
                }
                break;
            }
            case AttentionKindTag.Hire:
            {
                // 134 W2 (130 §4.2): Hire · Decline (· Open = O, as on every card). No pre-pick: a hire spends budget (P13).
                if (item.Agent is not { } name) return card;
                card.Options = ValueList.Of(
                    new CardOption(1, "Hire", null, false, new CardAction.HireAgent(name)),
                    new CardOption(2, "Decline", null, false, new CardAction.DeclineHire(name)));
                break;
            }
            case AttentionKindTag.Blocked:
            {
                // 134 W2 (130 §4.2): "one line + Open in theuth (and Unblock …)"; a decision blocker is the decision kind.
                if (item.Task is not { } task || item.Decision is not null) return card;
                card.Options = ValueList.Of(new CardOption(1, "Unblock", "back to todo", false, new CardAction.Unblock(task)));
                break;
            }
            case AttentionKindTag.Show:
                card.Clearable = true;
                break;
        }
        return card;
    }

    internal static string? ShortSession(string? s) => s is null ? null : s.Length <= 8 ? s : s[..8];
}
