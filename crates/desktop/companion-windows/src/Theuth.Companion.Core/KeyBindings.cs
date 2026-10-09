using System.Globalization;
using System.Text;
using System.Text.Json;

namespace Theuth.Companion.Core;

/// <summary>
/// Chord modifiers, Windows form (design 134 D6: "⌘ becomes Ctrl"; the Mac's ⌃ and ⌘ both land on Ctrl, ⌥ on Alt).
/// The values are Win32's <c>MOD_ALT</c> · <c>MOD_CONTROL</c> · <c>MOD_SHIFT</c> · <c>MOD_WIN</c>, so a chord's modifiers go
/// to <c>RegisterHotKey</c> as they are (add <c>MOD_NOREPEAT</c>). Win chords are the OS's: <see cref="KeyTable.Check"/> refuses them.
/// </summary>
[Flags]
public enum ChordModifiers : byte
{
    None = 0,
    Alt = 0x1,
    Control = 0x2,
    Shift = 0x4,
    Win = 0x8,
}

/// <summary>Win32 virtual-key codes the table, the aliases and the key names use (<c>winuser.h</c> <c>VK_*</c>).</summary>
public static class VirtualKeys
{
    public const ushort Back = 0x08, Tab = 0x09, Return = 0x0D, Escape = 0x1B, Space = 0x20;
    public const ushort PageUp = 0x21, PageDown = 0x22, End = 0x23, Home = 0x24;
    public const ushort Left = 0x25, Up = 0x26, Right = 0x27, Down = 0x28, Insert = 0x2D, Delete = 0x2E;
    public const ushort F1 = 0x70, F24 = 0x87;
    public const ushort OemComma = 0xBC, OemPeriod = 0xBE, Oem2Slash = 0xBF;

    /// <summary><c>VK_OEM_3</c>: the backtick / tilde key on a US layout (149 §8: the meeting push-to-talk Alt+`).</summary>
    public const ushort Oem3Backtick = 0xC0;

    /// <summary>
    /// The numeric keypad's Enter: Windows reports it as <c>VK_RETURN</c> with the extended-key flag
    /// (<c>KeyStatus.IsExtendedKey</c>); the companion encodes it as <c>VK_RETURN | 0x100</c> so it stays a key of its own,
    /// as the Mac's ⌅ (keycode 76) is next to ⏎ (36).
    /// </summary>
    public const ushort NumpadEnter = 0x10D;

    /// <summary>'0'…'9' and 'A'…'Z' are their ASCII codes.</summary>
    public static ushort Letter(char c) => char.ToUpperInvariant(c);
}

/// <summary>
/// One key combination, UI-framework-free (port of <c>SWIFT/CompanionCore/KeyBindings.swift</c>, Windows form).
/// Printable keys match by the typed character (layout-independent, what the user sees on the cap); everything else
/// (Enter, Esc, Space, arrows, F-keys) by the virtual-key code, which is also what <c>RegisterHotKey</c> needs.
/// Local-scope chords are stored lower-cased without Shift (<see cref="NormalizedLocal"/>).
/// </summary>
public sealed class Chord : IEquatable<Chord>
{
    public Chord(string chars = "", ushort keyCode = 0, ChordModifiers modifiers = ChordModifiers.None)
    {
        Chars = chars ?? "";
        KeyCode = keyCode;
        Modifiers = modifiers;
    }

    /// <summary>The typed character(s) ignoring modifiers (AppKit's <c>charactersIgnoringModifiers</c>; on Windows
    /// <c>ToUnicodeEx</c> with no modifier state); "" for a special key.</summary>
    public string Chars { get; }

    /// <summary>Win32 virtual-key code (<see cref="VirtualKeys"/>).</summary>
    public ushort KeyCode { get; }

    public ChordModifiers Modifiers { get; }

    /// <summary>A key that types a visible character: letters, digits, punctuation, symbols. Space and control or
    /// private-use characters are not.</summary>
    public bool IsCharKey
    {
        get
        {
            if (Chars.Length == 0 || new StringInfo(Chars).LengthInTextElements != 1) return false;
            var r = Rune.GetRuneAt(Chars, 0);
            return Rune.IsLetter(r) || Rune.IsNumber(r) || Rune.IsPunctuation(r) || Rune.IsSymbol(r);
        }
    }

    public bool IsFunctionKey => KeyNames.FunctionKeys.ContainsKey(KeyCode);

    /// <summary>Panel/Ask-bar chords: case and Shift do not distinguish keys (W1's <c>K</c> is <c>previous</c>).</summary>
    public Chord NormalizedLocal =>
        IsCharKey ? new Chord(Chars.ToLowerInvariant(), KeyCode, Modifiers & ~ChordModifiers.Shift) : this;

    public bool Equals(Chord? b)
    {
        if (b is null) return false;
        if (Modifiers != b.Modifiers) return false;
        return IsCharKey || b.IsCharKey ? string.Equals(Chars, b.Chars, StringComparison.Ordinal) : KeyCode == b.KeyCode;
    }

    public override bool Equals(object? obj) => obj is Chord c && Equals(c);

    public override int GetHashCode() => IsCharKey ? HashCode.Combine(Modifiers, Chars) : HashCode.Combine(Modifiers, KeyCode);

    public static bool operator ==(Chord? a, Chord? b) => a is null ? b is null : a.Equals(b);

    public static bool operator !=(Chord? a, Chord? b) => !(a == b);

    /// <summary>"Ctrl+Shift+Space" · "Ctrl+O" · "J" · "Enter": modifiers in Windows' order (Win, Ctrl, Alt, Shift), then the cap.</summary>
    public string Symbol
    {
        get
        {
            var s = new StringBuilder();
            if (Modifiers.HasFlag(ChordModifiers.Win)) s.Append("Win+");
            if (Modifiers.HasFlag(ChordModifiers.Control)) s.Append("Ctrl+");
            if (Modifiers.HasFlag(ChordModifiers.Alt)) s.Append("Alt+");
            if (Modifiers.HasFlag(ChordModifiers.Shift)) s.Append("Shift+");
            return s.Append(KeyNames.Name(KeyCode, Chars)).ToString();
        }
    }

    public override string ToString() => Symbol;

    // The keys the table and the aliases use.
    public static Chord ReturnKey { get; } = new("\r", VirtualKeys.Return);
    public static Chord NumpadEnter { get; } = new("\r", VirtualKeys.NumpadEnter);
    public static Chord Escape { get; } = new("\u001b", VirtualKeys.Escape);
    public static Chord Space { get; } = new(" ", VirtualKeys.Space);
    public static Chord Down { get; } = new("", VirtualKeys.Down);
    public static Chord Up { get; } = new("", VirtualKeys.Up);
}

/// <summary>Key caps for the special keys; letters and digits are their own cap.</summary>
public static class KeyNames
{
    public static readonly IReadOnlyDictionary<ushort, string> FunctionKeys =
        Enumerable.Range(0, VirtualKeys.F24 - VirtualKeys.F1 + 1).ToDictionary(i => (ushort)(VirtualKeys.F1 + i), i => $"F{i + 1}");

    private static readonly Dictionary<ushort, string> Special = new()
    {
        [VirtualKeys.Return] = "Enter", [VirtualKeys.NumpadEnter] = "Num Enter", [VirtualKeys.Escape] = "Esc",
        [VirtualKeys.Space] = "Space", [VirtualKeys.Tab] = "Tab", [VirtualKeys.Back] = "Backspace",
        [VirtualKeys.Delete] = "Delete", [VirtualKeys.Insert] = "Insert",
        // 149 NATIVE: arrow keys written the Windows way (words), not arrow characters.
        [VirtualKeys.Left] = "Left", [VirtualKeys.Right] = "Right", [VirtualKeys.Down] = "Down", [VirtualKeys.Up] = "Up",
        [VirtualKeys.Home] = "Home", [VirtualKeys.End] = "End", [VirtualKeys.PageUp] = "PgUp", [VirtualKeys.PageDown] = "PgDn",
    };

    public static string Name(ushort keyCode, string chars)
    {
        if (Special.TryGetValue(keyCode, out var s) || FunctionKeys.TryGetValue(keyCode, out s)) return s;
        if (new Chord(chars).IsCharKey) return chars.ToUpperInvariant();
        if (chars.Length == 0) return $"key {keyCode}";
        return string.Concat(chars.EnumerateRunes().Select(r => $"U+{r.Value:X4}"));
    }
}

public enum KeyScope
{
    /// <summary>Global hotkeys (<c>RegisterHotKey</c>): fire in any app.</summary>
    Anywhere,

    /// <summary>While the Attention panel is active.</summary>
    Panel,

    /// <summary>While the Ask bar is active.</summary>
    AskBar,

    /// <summary>149 §8: while the agents panel is active.</summary>
    Agents,

    /// <summary>149 §8: the island — key only in its request box.</summary>
    Island,

    /// <summary>149 §8: between the voice key's press and the send (point-and-say).</summary>
    PointAndSay,
}

/// <summary>Every action a key can trigger; <see cref="KeyText.RawValue(KeyAction)"/> is the persisted id.</summary>
public enum KeyAction
{
    OpenPanel, OpenAskBar,
    Answer1, Answer2, Answer3, AnswerRecommended, Escape, Next, Previous, Snooze, Clear,
    OpenDiff, OpenBoard, OpenSession, Reply, AskBar, Legend, Settings,
    AskSend, AskEscape, AskOpenSession, AskLegend,

    // 149 §8 (★ rows). anywhere:
    OpenAgents, VoiceKey, MeetingPtt, MeetingEye,
    // panel:
    TalkReply, PointAttach, StopAgent,
    // askBar:
    AskPoint,
    // agents:
    AgentsNext, AgentsPrevious, AgentsOpen, AgentsOpenSession, AgentsTalk, AgentsStop, AgentsEscape,
    // island:
    IslandMute, IslandStop, IslandDismiss, IslandDeeper, IslandSend, IslandEscape,
    // pointAndSay:
    PointCancel,
}

public static class KeyText
{
    public static string Title(this KeyScope s) => s switch
    {
        KeyScope.Anywhere => "Anywhere",
        KeyScope.Panel => "Attention panel",
        KeyScope.AskBar => "Ask bar",
        KeyScope.Agents => "Agents panel",
        KeyScope.Island => "Meeting island",
        KeyScope.PointAndSay => "Point and say",
        _ => "",
    };

    /// <summary>The persisted word (Swift's raw value): <c>anywhere</c> · <c>panel</c> · <c>askBar</c>.</summary>
    public static string RawValue(this KeyScope s) => Camel(s.ToString());

    /// <summary>The persisted id (Swift's raw value): <c>openPanel</c>, <c>answer1</c>, <c>askLegend</c> …</summary>
    public static string RawValue(this KeyAction a) => Camel(a.ToString());

    public static KeyAction? ActionFromRaw(string raw)
    {
        foreach (var a in Enum.GetValues<KeyAction>())
        {
            if (a.RawValue() == raw) return a;
        }
        return null;
    }

    /// <summary>What the Attention panel / Ask bar does; null for the global rows (the app's hotkey handlers).</summary>
    public static Intent? Intent(this KeyAction a) => a switch
    {
        KeyAction.OpenPanel or KeyAction.OpenAskBar => null,
        KeyAction.Answer1 => Core.Intent.Answer(1),
        KeyAction.Answer2 => Core.Intent.Answer(2),
        KeyAction.Answer3 => Core.Intent.Answer(3),
        KeyAction.AnswerRecommended => Core.Intent.AnswerRecommended,
        KeyAction.Escape or KeyAction.AskEscape => Core.Intent.Escape,
        KeyAction.Next => Core.Intent.Next,
        KeyAction.Previous => Core.Intent.Previous,
        KeyAction.Snooze => Core.Intent.Snooze,
        KeyAction.Clear => Core.Intent.Clear,
        KeyAction.OpenDiff => Core.Intent.OpenDiff,
        KeyAction.OpenBoard => Core.Intent.OpenBoard,
        KeyAction.OpenSession or KeyAction.AskOpenSession => Core.Intent.OpenSession,
        KeyAction.Reply => Core.Intent.Reply,
        KeyAction.AskBar => Core.Intent.AskBar,
        KeyAction.Legend or KeyAction.AskLegend => Core.Intent.Legend,
        KeyAction.Settings => Core.Intent.Settings,
        KeyAction.AskSend => Core.Intent.Send,
        KeyAction.AgentsEscape or KeyAction.IslandEscape or KeyAction.PointCancel => Core.Intent.Escape,
        KeyAction.AgentsNext => Core.Intent.Next,
        KeyAction.AgentsPrevious => Core.Intent.Previous,
        KeyAction.AgentsOpen or KeyAction.AgentsOpenSession => Core.Intent.OpenSession,
        _ => null,
    };

    private static string Camel(string pascal) => pascal.Length == 0 ? pascal : char.ToLowerInvariant(pascal[0]) + pascal[1..];
}

public sealed record KeyBinding(KeyAction Action, KeyScope Scope, string Label, Chord? DefaultChord, Chord? Chord, bool Fixed = false)
{
    /// <summary>The effective chord is <see cref="Chord"/>: the override when there is one, else the default; null = unassigned.
    /// <see cref="Fixed"/>: Esc is how the recorder cancels and how every surface closes — never rebound.</summary>
    public KeyAction Id => Action;

    public bool IsDefault => Chord == DefaultChord;
}

public enum VerdictKind
{
    Ok,

    /// <summary>The row already has this chord.</summary>
    Unchanged,

    /// <summary>A Win-key chord (any scope), a system chord the app passes in, or a bare key with no modifier (global scope).
    /// Swift's <c>reservedByMacOS</c>.</summary>
    ReservedByWindows,

    /// <summary>Same scope, another row: the recorder offers "Use here instead" (<see cref="KeyTable.Move"/>).</summary>
    UsedBy,
}

public readonly record struct Verdict(VerdictKind Kind, KeyAction? Owner = null)
{
    public static Verdict Ok => new(VerdictKind.Ok);
    public static Verdict Unchanged => new(VerdictKind.Unchanged);
    public static Verdict ReservedByWindows => new(VerdictKind.ReservedByWindows);
    public static Verdict UsedBy(KeyAction owner) => new(VerdictKind.UsedBy, owner);
}

/// <summary>At launch: the chord that registered (or none) and one line for Settings › Keys and the legend; never a dialog.</summary>
public sealed record GlobalResolution(Chord? Chord, string? Notice);

/// <summary>
/// The one data list behind the panel grammar, the legend and Settings › Keys (W6 item 4). Defaults reproduce
/// W1/W3's behaviour exactly (⌘ → Ctrl); overrides persist as <c>{actionId: chord | null}</c> (null = unassigned).
/// </summary>
public sealed class KeyTable : IEquatable<KeyTable>
{
    public Dictionary<KeyAction, Chord?> Overrides { get; private set; }

    public KeyTable() => Overrides = new();

    public KeyTable(IDictionary<KeyAction, Chord?> overrides) => Overrides = new(overrides);

    public KeyTable Clone() => new(Overrides);

    private static KeyBinding Row(KeyAction a, KeyScope s, string label, Chord? chord, bool isFixed = false) =>
        new(a, s, label, chord, chord, isFixed);

    private static Chord Key(string c, ushort code, ChordModifiers m = ChordModifiers.None) => new(c, code, m);

    /// <summary>The table (130 §4.2 · §4.3 · D6; 134 §4.2), scope order anywhere · panel · askBar.</summary>
    public static readonly IReadOnlyList<KeyBinding> Defaults = new[]
    {
        Row(KeyAction.OpenPanel, KeyScope.Anywhere, "Open the panel · hold to talk", Key(" ", VirtualKeys.Space, ChordModifiers.Control | ChordModifiers.Shift)),
        Row(KeyAction.OpenAskBar, KeyScope.Anywhere, "Open the Ask bar", null),
        Row(KeyAction.Answer1, KeyScope.Panel, "Answer with option 1", Key("1", '1')),
        Row(KeyAction.Answer2, KeyScope.Panel, "Answer with option 2", Key("2", '2')),
        Row(KeyAction.Answer3, KeyScope.Panel, "Answer with option 3", Key("3", '3')),
        Row(KeyAction.AnswerRecommended, KeyScope.Panel, "Answer with the recommended option", Chord.ReturnKey),
        Row(KeyAction.Next, KeyScope.Panel, "Next item", Key("j", 'J')),
        Row(KeyAction.Previous, KeyScope.Panel, "Previous item", Key("k", 'K')),
        Row(KeyAction.Snooze, KeyScope.Panel, "Snooze 30 m", Key("s", 'S')),
        Row(KeyAction.Clear, KeyScope.Panel, "Clear", Key("e", 'E')),
        Row(KeyAction.OpenDiff, KeyScope.Panel, "Open the diff", Key("d", 'D')),
        Row(KeyAction.OpenBoard, KeyScope.Panel, "Open the board", Key("o", 'O')),
        Row(KeyAction.OpenSession, KeyScope.Panel, "Open the session", Key("o", 'O', ChordModifiers.Control)),
        Row(KeyAction.Reply, KeyScope.Panel, "Reply in words", Key("r", 'R')),
        Row(KeyAction.AskBar, KeyScope.Panel, "Jump to the Ask bar", Key("l", 'L', ChordModifiers.Control)),
        Row(KeyAction.Legend, KeyScope.Panel, "Keyboard shortcuts", Key("/", VirtualKeys.Oem2Slash, ChordModifiers.Control)),
        Row(KeyAction.Settings, KeyScope.Panel, "Settings", Key(",", VirtualKeys.OemComma, ChordModifiers.Control)),
        Row(KeyAction.Escape, KeyScope.Panel, "Undo the hold, else close", Chord.Escape, isFixed: true),
        Row(KeyAction.AskSend, KeyScope.AskBar, "Send to theuth", Chord.ReturnKey),
        Row(KeyAction.AskOpenSession, KeyScope.AskBar, "Open the desk's session", Key("o", 'O', ChordModifiers.Control)),
        Row(KeyAction.AskLegend, KeyScope.AskBar, "Keyboard shortcuts", Key("/", VirtualKeys.Oem2Slash, ChordModifiers.Control)),
        Row(KeyAction.AskEscape, KeyScope.AskBar, "Undo the hold, else close", Chord.Escape, isFixed: true),
        // 149 §8 ★ rows, Windows defaults (⌃⇧A → Ctrl+Shift+A; the voice key ⌃S → Ctrl+Alt+Space, D16 / 7.4 #3; ⌥` → Alt+`; ⌥⇧E → Alt+Shift+E).
        Row(KeyAction.OpenAgents, KeyScope.Anywhere, "Open the agents panel", Key("a", 'A', ChordModifiers.Control | ChordModifiers.Shift)),
        Row(KeyAction.VoiceKey, KeyScope.Anywhere, "Voice key — point and say (hold; double-tap locks)", Key(" ", VirtualKeys.Space, ChordModifiers.Control | ChordModifiers.Alt)),
        Row(KeyAction.MeetingPtt, KeyScope.Anywhere, "Meeting push-to-talk (while a meeting is live)", Key("`", VirtualKeys.Oem3Backtick, ChordModifiers.Alt)),
        Row(KeyAction.MeetingEye, KeyScope.Anywhere, "Meeting eye — hide from capture (while live)", Key("e", 'E', ChordModifiers.Alt | ChordModifiers.Shift)),
        Row(KeyAction.TalkReply, KeyScope.Panel, "Hold to talk into the reply", Key("v", 'V')),
        Row(KeyAction.PointAttach, KeyScope.Panel, "Point — attach the screen", Key("p", 'P')),
        Row(KeyAction.StopAgent, KeyScope.Panel, "Stop the agent (hold)", Key("x", 'X')),
        Row(KeyAction.AskPoint, KeyScope.AskBar, "Point — attach the screen", Key("p", 'P')),
        Row(KeyAction.AgentsNext, KeyScope.Agents, "Next row", Key("j", 'J')),
        Row(KeyAction.AgentsPrevious, KeyScope.Agents, "Previous row", Key("k", 'K')),
        Row(KeyAction.AgentsOpen, KeyScope.Agents, "Open the row's session", Chord.ReturnKey),
        Row(KeyAction.AgentsOpenSession, KeyScope.Agents, "Open the session in theuth", Key("o", 'O', ChordModifiers.Control)),
        Row(KeyAction.AgentsTalk, KeyScope.Agents, "Hold to talk into the row", Key("v", 'V')),
        Row(KeyAction.AgentsStop, KeyScope.Agents, "Stop the agent (hold)", Key("x", 'X')),
        Row(KeyAction.AgentsEscape, KeyScope.Agents, "Undo the hold, else close", Chord.Escape, isFixed: true),
        Row(KeyAction.IslandMute, KeyScope.Island, "Mute (pause the meeting)", Key("m", 'M')),
        // §8 says ⌃⇧M; local scopes store chords without Shift (NormalizedLocal), so Ctrl+M — a typed Ctrl+Shift+M resolves to it (C did the same).
        Row(KeyAction.IslandStop, KeyScope.Island, "Stop the meeting (hold)", Key("m", 'M', ChordModifiers.Control)),
        Row(KeyAction.IslandDismiss, KeyScope.Island, "Dismiss the topic", Key("e", 'E')),
        Row(KeyAction.IslandDeeper, KeyScope.Island, "Deeper", Key("d", 'D')),
        Row(KeyAction.IslandSend, KeyScope.Island, "Send the request", Chord.ReturnKey),
        Row(KeyAction.IslandEscape, KeyScope.Island, "Leave the request box", Chord.Escape, isFixed: true),
        Row(KeyAction.PointCancel, KeyScope.PointAndSay, "Cancel", Chord.Escape, isFixed: true),
    };

    /// <summary>Fixed synonyms, never rebound: ↓/↑ walk, keypad Enter answers, <c>?</c> opens the legend. Shown in the legend.</summary>
    public static readonly IReadOnlyList<(Chord Chord, KeyAction Action)> Aliases = new (Chord, KeyAction)[]
    {
        (Chord.Down, KeyAction.Next), (Chord.Up, KeyAction.Previous), (Chord.NumpadEnter, KeyAction.AnswerRecommended),
        (Key("?", VirtualKeys.Oem2Slash), KeyAction.Legend), (Key("?", VirtualKeys.Oem2Slash), KeyAction.AskLegend),
        // 149 §8: space = focus the reply field (alias of R, Revision 1 #18); Ctrl+P = point while typing in the Ask bar; ↓/↑ walk the agents rows.
        (Chord.Space, KeyAction.Reply), (Key("p", 'P', ChordModifiers.Control), KeyAction.AskPoint),
        (Chord.Down, KeyAction.AgentsNext), (Chord.Up, KeyAction.AgentsPrevious),
    };

    /// <summary>Every row with its effective chord, table order.</summary>
    public IReadOnlyList<KeyBinding> Bindings =>
        Defaults.Select(b => Overrides.TryGetValue(b.Action, out var o) ? b with { Chord = o } : b).ToList();

    public IReadOnlyList<KeyBinding> BindingsIn(KeyScope scope) => Bindings.Where(b => b.Scope == scope).ToList();

    public KeyBinding Binding(KeyAction action) => Bindings.First(b => b.Action == action);

    public Chord? ChordFor(KeyAction action) => Binding(action).Chord;

    /// <summary>The row holding <paramref name="chord"/> in <paramref name="scope"/>, if any (the table first, then the fixed aliases).</summary>
    public KeyAction? ActionFor(Chord chord, KeyScope scope)
    {
        var c = scope == KeyScope.Anywhere ? chord : chord.NormalizedLocal;
        var b = BindingsIn(scope).FirstOrDefault(x => x.Chord == c);
        if (b is not null) return b.Action;
        foreach (var (ac, action) in Aliases)
        {
            if (ac == c && ScopeOf(action) == scope) return action;
        }
        return null;
    }

    public static KeyScope ScopeOf(KeyAction action) => Defaults.First(d => d.Action == action).Scope;

    /// <summary>Never two actions on one chord in one scope (across scopes is fine: panel J and Ask-bar J never coexist).</summary>
    public bool IsUniquePerScope => Enum.GetValues<KeyScope>().All(s =>
    {
        var cs = BindingsIn(s).Select(b => b.Chord).OfType<Chord>().ToList();
        return cs.Distinct().Count() == cs.Count;
    });

    // MARK: the collision policy (130 W6 brief item 4d; 134 §4.6: Win-key chords refused)

    /// <summary><paramref name="reserved"/>: system chords the app knows are taken (tests stub them).</summary>
    public Verdict Check(Chord chord, KeyAction action, IReadOnlyCollection<Chord>? reserved = null)
    {
        var scope = ScopeOf(action);
        var c = scope == KeyScope.Anywhere ? chord : chord.NormalizedLocal;
        if (ChordFor(action) == c) return Verdict.Unchanged;
        if (c.Modifiers.HasFlag(ChordModifiers.Win)) return Verdict.ReservedByWindows;
        if (scope == KeyScope.Anywhere
            && ((reserved?.Contains(c) ?? false) || ((c.Modifiers & ~ChordModifiers.Shift) == ChordModifiers.None && !c.IsFunctionKey)))
        {
            return Verdict.ReservedByWindows;
        }
        var other = BindingsIn(scope).FirstOrDefault(b => b.Chord == c && b.Action != action);
        return other is not null ? Verdict.UsedBy(other.Action) : Verdict.Ok;
    }

    /// <summary>Assign without checks (null = unassigned). Local chords are normalized.</summary>
    public void Set(Chord? chord, KeyAction action)
    {
        var c = ScopeOf(action) == KeyScope.Anywhere ? chord : chord?.NormalizedLocal;
        if (c == Binding(action).DefaultChord) Overrides.Remove(action);
        else Overrides[action] = c;
    }

    /// <summary>"Use here instead": the chord moves to <paramref name="action"/>; the row that had it goes unassigned.</summary>
    public void Move(Chord chord, KeyAction action)
    {
        var v = Check(chord, action);
        if (v.Kind == VerdictKind.UsedBy && v.Owner is { } other) Set(null, other);
        Set(chord, action);
    }

    public void Reset(KeyAction action) => Overrides.Remove(action);

    public void ResetAll() => Overrides = new();

    // MARK: persistence (CompanionSettings.Keys)

    /// <summary><c>{"next":{"chars":"n","keyCode":78,"modifiers":0},"clear":null}</c>; <c>{}</c> for the defaults.</summary>
    public string Json
    {
        get
        {
            using var buffer = new MemoryStream();
            using (var w = new Utf8JsonWriter(buffer))
            {
                w.WriteStartObject();
                foreach (var (action, chord) in Overrides.OrderBy(kv => kv.Key.RawValue(), StringComparer.Ordinal))
                {
                    w.WritePropertyName(action.RawValue());
                    if (chord is null)
                    {
                        w.WriteNullValue();
                        continue;
                    }
                    w.WriteStartObject();
                    w.WriteString("chars", chord.Chars);
                    w.WriteNumber("keyCode", chord.KeyCode);
                    w.WriteNumber("modifiers", (byte)chord.Modifiers);
                    w.WriteEndObject();
                }
                w.WriteEndObject();
            }
            return Encoding.UTF8.GetString(buffer.ToArray());
        }
    }

    /// <summary>Reads <see cref="Json"/>; unknown action ids are skipped and anything malformed yields the defaults (as Swift's <c>try?</c>).</summary>
    public KeyTable(string json)
    {
        Overrides = new();
        Dictionary<KeyAction, Chord?> read = new();
        try
        {
            using var doc = JsonDocument.Parse(json);
            if (doc.RootElement.ValueKind != JsonValueKind.Object) return;
            foreach (var p in doc.RootElement.EnumerateObject())
            {
                Chord? chord = null;
                if (p.Value.ValueKind != JsonValueKind.Null)
                {
                    var o = p.Value;
                    if (o.ValueKind != JsonValueKind.Object) return;
                    if (!o.TryGetProperty("chars", out var ch) || ch.ValueKind != JsonValueKind.String) return;
                    if (!o.TryGetProperty("keyCode", out var kc) || !kc.TryGetUInt16(out var code)) return;
                    if (!o.TryGetProperty("modifiers", out var md) || !md.TryGetByte(out var mods)) return;
                    chord = new Chord(ch.GetString()!, code, (ChordModifiers)mods);
                }
                if (KeyText.ActionFromRaw(p.Name) is { } a) read[a] = chord;
            }
        }
        catch (JsonException)
        {
            return;
        }
        Overrides = read;
    }

    // MARK: another app's global hotkey (brief item 4d, third bullet)

    /// <summary>
    /// At launch: the saved global chord if it registers, else the default with a notice, else none.
    /// <paramref name="canRegister"/> is the registrar (<c>RegisterHotKey</c> answering 0 when another app holds the chord; tests stub it).
    /// </summary>
    public static GlobalResolution ResolveGlobal(Chord? saved, Chord? def, Func<Chord, bool> canRegister)
    {
        if (saved is null) return new GlobalResolution(null, null);
        if (canRegister(saved)) return new GlobalResolution(saved, null);
        if (def is not null && def != saved && canRegister(def))
        {
            return new GlobalResolution(def, $"{saved.Symbol} is held by another app — using {def.Symbol}");
        }
        return new GlobalResolution(null, $"{saved.Symbol} is held by another app — no hotkey until you pick another");
    }

    public bool Equals(KeyTable? other)
    {
        if (other is null || other.Overrides.Count != Overrides.Count) return false;
        foreach (var (k, v) in Overrides)
        {
            if (!other.Overrides.TryGetValue(k, out var w) || v != w) return false;
        }
        return true;
    }

    public override bool Equals(object? obj) => obj is KeyTable t && Equals(t);

    public override int GetHashCode()
    {
        int h = Overrides.Count;
        foreach (var (k, v) in Overrides) h ^= HashCode.Combine(k, v);
        return h;
    }

    public static bool operator ==(KeyTable? a, KeyTable? b) => a is null ? b is null : a.Equals(b);

    public static bool operator !=(KeyTable? a, KeyTable? b) => !(a == b);
}