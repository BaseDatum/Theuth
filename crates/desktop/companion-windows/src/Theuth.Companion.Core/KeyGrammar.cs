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
