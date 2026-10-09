using System.Text;
using Theuth.Companion.Rpc;

namespace Theuth.Companion.Core;

/// <summary>
/// A key event stripped of the UI framework: the typed characters, the virtual-key code and the modifiers
/// (port of Swift's <c>KeyPress</c>; ⌘ → Ctrl). W2 builds one from WinUI's <c>KeyDown</c>
/// (<c>VirtualKey</c>, <c>KeyStatus.IsExtendedKey</c> for <see cref="VirtualKeys.NumpadEnter"/>, the modifier state).
/// </summary>
public readonly record struct KeyPress(string Chars, ushort KeyCode, ChordModifiers Modifiers)
{
    public KeyPress(string chars, ushort keyCode = 0, bool ctrl = false)
        : this(chars, keyCode, ctrl ? ChordModifiers.Control : ChordModifiers.None) { }

    /// <summary>Swift's <c>command</c>.</summary>
    public bool Ctrl => Modifiers.HasFlag(ChordModifiers.Control);

    public Chord Chord => new(Chars, KeyCode, Modifiers);
}

public enum IntentKind
{
    None,

    /// <summary>1 · 2 · 3 (<see cref="Intent.Option"/>, 1-based).</summary>
    Answer,

    /// <summary>Enter: the recommended option.</summary>
    AnswerRecommended,

    /// <summary>Esc: undo during the hold, otherwise close.</summary>
    Escape,
    Next,
    Previous,

    /// <summary>S: snooze 30 m.</summary>
    Snooze,

    /// <summary>E: clear a finished receipt.</summary>
    Clear,

    /// <summary>D · O · Ctrl+O: deep links into the main window.</summary>
    OpenDiff,
    OpenBoard,
    OpenSession,

    /// <summary>R: reply in words (W3).</summary>
    Reply,

    /// <summary>Ctrl+L: jump to the Ask bar (W3).</summary>
    AskBar,

    /// <summary>Ctrl+/ (W6): the shortcuts legend.</summary>
    Legend,

    /// <summary>Ctrl+,: Settings.</summary>
    Settings,

    /// <summary>Enter in the Ask bar: send.</summary>
    Send,
}

/// <summary>What a keystroke means in the panel (130 §4.2 table). No side effects — the app maps intents to actions.</summary>
public readonly record struct Intent(IntentKind Kind, int Option = 0)
{
    public static Intent Answer(int option) => new(IntentKind.Answer, option);
    public static Intent AnswerRecommended => new(IntentKind.AnswerRecommended);
    public static Intent Escape => new(IntentKind.Escape);
    public static Intent Next => new(IntentKind.Next);
    public static Intent Previous => new(IntentKind.Previous);
    public static Intent Snooze => new(IntentKind.Snooze);
    public static Intent Clear => new(IntentKind.Clear);
    public static Intent OpenDiff => new(IntentKind.OpenDiff);
    public static Intent OpenBoard => new(IntentKind.OpenBoard);
    public static Intent OpenSession => new(IntentKind.OpenSession);
    public static Intent Reply => new(IntentKind.Reply);
    public static Intent AskBar => new(IntentKind.AskBar);
    public static Intent Legend => new(IntentKind.Legend);
    public static Intent Settings => new(IntentKind.Settings);
    public static Intent Send => new(IntentKind.Send);
    public static Intent None => new(IntentKind.None);
}

public static class KeyGrammar
{
    /// <summary>Seconds.</summary>
    public const double SnoozeInterval = 30 * 60;

    private static readonly KeyTable DefaultTable = new();

    /// <summary>
    /// The chord looked up in <paramref name="table"/> for <paramref name="scope"/> (W6: one data list, 130 R2). While a text
    /// field has focus (the send-back note, the R field) single keys are typed, not actions: only Esc passes.
    /// </summary>
    public static Intent IntentFor(KeyPress key, KeyScope scope = KeyScope.Panel, KeyTable? table = null, bool textFieldFocused = false)
    {
        var action = (table ?? DefaultTable).ActionFor(key.Chord, scope);
        if (action is not { } a || a.Intent() is not { } intent) return Intent.None;
        if (textFieldFocused && intent != Intent.Escape) return Intent.None;
        return intent;
    }
}

/// <summary>153 §15 ★ pointAndSay rows "fixed in the card": what a key does while the AnswerCard is key.</summary>
public enum CardKeyIntent
{
    None,

    /// <summary>⏎: expand ↔ collapse (§12.3).</summary>
    ToggleExpand,

    /// <summary>⎋ while expanded: fold back to the collapsed card at the same place.</summary>
    Collapse,

    /// <summary>⎋ while collapsed and settled: hide the card.</summary>
    Dismiss,

    /// <summary>⎋ while the desk is working (thinking): <c>cancel(desk, "root")</c> — the card freezes with "stopped" (§12.4).</summary>
    Stop,

    /// <summary>⎋ while the answer is being read aloud: stop the speech, keep the card (§12.4 speaking → answer).</summary>
    StopSpeaking,

    /// <summary>O: open the desk session in theuth.</summary>
    Open,

    /// <summary>J / → : the next request of this card's thread.</summary>
    Next,

    /// <summary>K / ← : the previous request.</summary>
    Previous,

    /// <summary>Ctrl+L (⌘L): the last answers from history.</summary>
    List,

    /// <summary>⏎ with the approval box up: <c>resolve_approval(id, Allow)</c> (§10.6).</summary>
    Approve,

    /// <summary>⎋ with the approval box up: <c>resolve_approval(id, Deny)</c>.</summary>
    Deny,

    /// <summary>⏎ in the follow-up field: send it to the desk with the card's marker.</summary>
    SendFollowUp,

    /// <summary>R1: ⎋ or Ctrl+L with the list open — back to the card.</summary>
    CloseList,

    /// <summary>R1: ⏎ with the list open — show the selected answer in the card.</summary>
    PickRow,
}

/// <summary>What the card shows when a key arrives (the precedence of §15: approval box › speech › stop › collapse › dismiss).</summary>
public readonly record struct CardKeyContext(bool ApprovalShown = false, bool Expanded = false, bool Working = false, bool Speaking = false, bool FollowUpFocused = false, bool ListOpen = false);

/// <summary>
/// 153 §15 / §12.2: the card's fixed keys — ⏎ expand/collapse · O open · J/K (and ←/→) thread · Ctrl+L list · ⎋ — and, while the
/// approval box is shown, ⏎ approve · ⎋ deny, which take precedence over expand/dismiss. In the follow-up field typed keys are
/// text: only ⏎ (send) and ⎋ act. Pure; the window maps intents to actions.
/// </summary>
public static class CardKeys
{
    public static CardKeyIntent IntentFor(KeyPress key, CardKeyContext ctx)
    {
        bool enter = key.KeyCode is VirtualKeys.Return or VirtualKeys.NumpadEnter;
        bool esc = key.KeyCode == VirtualKeys.Escape;
        bool plain = (key.Modifiers & ~ChordModifiers.Shift) == ChordModifiers.None;
        if (ctx.ApprovalShown && plain)
        {
            if (enter) return CardKeyIntent.Approve;
            if (esc) return CardKeyIntent.Deny;
        }
        if (ctx.ListOpen)
        {
            // R1 / the Mac's order: the list answers ⎋, Ctrl+L, ⏎ and the walk keys before the card does.
            if (esc || (key.Modifiers == ChordModifiers.Control && key.KeyCode == 'L')) return CardKeyIntent.CloseList;
            if (enter && plain) return CardKeyIntent.PickRow;
            if (key.Modifiers == ChordModifiers.None)
            {
                if (key.KeyCode is 'J' or VirtualKeys.Down) return CardKeyIntent.Next;
                if (key.KeyCode is 'K' or VirtualKeys.Up) return CardKeyIntent.Previous;
            }
            return CardKeyIntent.None;
        }
        if (esc)
        {
            if (ctx.Speaking) return CardKeyIntent.StopSpeaking;
            if (ctx.Working) return CardKeyIntent.Stop;
            return ctx.Expanded ? CardKeyIntent.Collapse : CardKeyIntent.Dismiss;
        }
        if (ctx.FollowUpFocused) return enter && plain ? CardKeyIntent.SendFollowUp : CardKeyIntent.None;
        if (enter && plain) return CardKeyIntent.ToggleExpand;
        if (key.Modifiers == ChordModifiers.Control && key.KeyCode == 'L') return CardKeyIntent.List;
        if (key.Modifiers != ChordModifiers.None) return CardKeyIntent.None;
        return key.KeyCode switch
        {
            'O' => CardKeyIntent.Open,
            'J' or VirtualKeys.Right => CardKeyIntent.Next,
            'K' or VirtualKeys.Left => CardKeyIntent.Previous,
            _ => CardKeyIntent.None,
        };
    }

    /// <summary>
    /// The key caps the card's footer shows for its state (Windows words; §3.3/§3.4 copy). Expanded = the Mac's
    /// <c>AnswerCard.swift:145</c> row: "‹mode key› follow up · J K thread · Ctrl+L list · O open · Esc collapse" (<paramref name="keyText"/>
    /// = Ctrl+Alt+C for a command card, else the voice key); the list: "Ctrl+L back · Esc collapse".
    /// </summary>
    public static string Footer(CardKeyContext ctx, bool collapsedAnswer, string keyText = "Ctrl+Alt+Space") =>
        ctx.ApprovalShown ? "Yes Enter · No Esc · or say “yes” / “no”"
        : ctx.ListOpen ? "Ctrl+L back · Esc collapse"
        : ctx.Working ? "Esc stop"
        : ctx.Expanded ? $"{keyText} follow up · J K thread · Ctrl+L list · O open · Esc collapse"
        : collapsedAnswer ? "Enter expand · O open · Esc" : "Esc";
}

/// <summary><c>theuth://</c> links the main window handles (W1S; on Windows the shell's deep-link plugin, W5).</summary>
public static class DeepLink
{
    // Foundation's CharacterSet.urlPathAllowed (ASCII letters, digits and these) plus ':' — item ids carry ':'
    // (`decision:<task>:<id>`), which is legal in a path; only spaces and the like are escaped.
    private const string PathAllowed = "!$&'()*+,-./:=@_~";

    public static Uri Attention(string id) => new($"theuth://attention/{EscapePath(id)}");

    public static Uri Session(string id) => new($"theuth://c/{id}");

    public static readonly Uri New = new("theuth://new");

    private static string EscapePath(string s)
    {
        var sb = new StringBuilder();
        foreach (var b in Encoding.UTF8.GetBytes(s))
        {
            char c = (char)b;
            if (b < 0x80 && (char.IsAsciiLetterOrDigit(c) || PathAllowed.Contains(c))) sb.Append(c);
            else sb.Append('%').Append(b.ToString("X2"));
        }
        return sb.ToString();
    }
}

/// <summary>E5: an answer goes out only if the item is still in the latest list.</summary>
public static class StaleGuard
{
    public static bool Allows(string id, IEnumerable<AttentionItem> latest) => latest.Any(i => i.Id == id);
}