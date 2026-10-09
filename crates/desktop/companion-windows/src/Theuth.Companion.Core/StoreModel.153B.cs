namespace Theuth.Companion.Core;

/// <summary>
/// Design 153 W153B's Store members (the 149 partial pattern): the commit hook the companion fields use to report
/// <c>dictation_correction</c> (§6.1 — observation in our own fields, at send), and the no-hold sends of the spoken "send it"
/// (§7.4 — the phrase was the confirmation). Raised and called on the Store's owner thread.
/// </summary>
public partial class StoreModel
{
    /// <summary>
    /// A held action is about to be committed: its hold ran out without Esc, or it was sent at once (<see cref="AnswerNow"/>). The
    /// Ask bar and the row mics match it against the text they dictated and report <c>dictation_correction(id, sent_text)</c>
    /// (153 §6.1); nothing is reported for an undone hold.
    /// </summary>
    public event Action<Held>? Committing;

    /// <summary>153 §7.4: "send it" in the Ask bar — the text goes to theuth's desk now, no 3 s hold (Swift <c>commitAsk</c> without the hold).</summary>
    public void SubmitAskNow(string text)
    {
        var t = text.Trim();
        if (t.Length == 0) return;
        AnswerNow(new Held("", "theuth", t, new CardAction.AskTheuth(t)));
    }

    /// <summary>153 §7.4: "send it" into a row mic — <c>send_input(session, text)</c> now, no hold; the receipt names <paramref name="who"/>.</summary>
    public void SendInputNow(string who, string session, string text)
    {
        var t = text.Trim();
        if (t.Length == 0) return;
        AnswerNow(new Held("", who, "sending", new CardAction.SendInputTo(session, t)));
    }

    /// <summary>153 §7.4: "send it" into a card's V — the reply goes now, no hold (the same E5 re-read and receipt as a typed reply).</summary>
    public void SubmitReplyNow(string text)
    {
        var t = text.Trim();
        if (t.Length == 0 || FocusedCard is not { } c || c.ReplyAction(t) is not { } action) return;
        Ask.ReplyFor = null;
        OnPropertyChanged(nameof(Ask));
        AnswerNow(new Held(c.Item.Id, c.Who, "replied", action));
    }

    /// <summary>The text a held action sends, for the correction match: the Ask text, a row's input, a card reply's text; null for the rest.</summary>
    public static string? SentText(CardAction action) => action switch
    {
        CardAction.AskTheuth a => a.Text,
        CardAction.SendInputTo s => s.Text,
        CardAction.SendNextStep n => n.Text,
        _ => null,
    };

    /// <summary>The session a held action sends into (row mic / card reply), for the correction match; null for the Ask bar's desk send.</summary>
    public static string? SentSession(CardAction action) => action switch
    {
        CardAction.SendInputTo s => s.Session,
        CardAction.SendNextStep n => n.Session,
        _ => null,
    };
}
