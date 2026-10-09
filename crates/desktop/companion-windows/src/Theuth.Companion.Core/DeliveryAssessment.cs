namespace Theuth.Companion.Core;

// 153 §13.1 / §13.8 / §13.9 / §7.4 — any-app dictation, the pure half of the Windows twin (W153D). The app reads the focused element
// with UI Automation (AnyApp/FocusReader.cs) into a FocusFacts; everything that decides lives here so the Mac runs the tests.

/// <summary>UI Automation control type ids (<c>UIA_*ControlTypeId</c>, 50000–50040) and the role words the design's copy uses — a
/// port of <c>crates/computer/src/ax.rs</c> <c>uia_role</c> (§13.1: "ax.rs:163's uia_role mapping reused for the names").</summary>
public static class UiaControlType
{
    public const int Button = 50000, Calendar = 50001, CheckBox = 50002, ComboBox = 50003, Edit = 50004, Hyperlink = 50005,
        Image = 50006, ListItem = 50007, List = 50008, Menu = 50009, MenuBar = 50010, MenuItem = 50011, ProgressBar = 50012,
        RadioButton = 50013, ScrollBar = 50014, Slider = 50015, Spinner = 50016, StatusBar = 50017, Tab = 50018, TabItem = 50019,
        Text = 50020, ToolBar = 50021, ToolTip = 50022, Tree = 50023, TreeItem = 50024, Custom = 50025, Group = 50026,
        Thumb = 50027, DataGrid = 50028, DataItem = 50029, Document = 50030, SplitButton = 50031, Window = 50032, Pane = 50033,
        Header = 50034, HeaderItem = 50035, Table = 50036, TitleBar = 50037, Separator = 50038, SemanticZoom = 50039, AppBar = 50040;

    /// <summary>The role word: the macOS vocabulary where one exists (<c>field</c>, <c>button</c>, <c>text</c>, <c>link</c>), else the
    /// localized control type lowercased without spaces, else <c>element</c> — exactly <c>uia_role</c>.</summary>
    public static string Role(int controlType, string? localized)
    {
        string word = controlType switch
        {
            Button or SplitButton => "button",
            CheckBox => "checkbox",
            ComboBox => "combobox",
            Edit => "field",
            Hyperlink => "link",
            Image => "image",
            ListItem => "listitem",
            List => "list",
            Menu => "menu",
            MenuBar => "menubar",
            MenuItem => "menuitem",
            ProgressBar => "progress",
            RadioButton => "radio",
            ScrollBar => "scrollbar",
            Slider => "slider",
            Spinner => "spinner",
            StatusBar => "statusbar",
            Tab => "tabgroup",
            TabItem => "tab",
            Text => "text",
            ToolBar or AppBar => "toolbar",
            ToolTip => "tooltip",
            Tree => "tree",
            TreeItem => "treeitem",
            Group => "group",
            DataGrid or Table => "table",
            DataItem => "cell",
            Document => "document",
            Window => "window",
            Pane => "pane",
            Header => "header",
            HeaderItem => "headeritem",
            TitleBar => "titlebar",
            Separator => "separator",
            _ => "",
        };
        if (word.Length > 0) return word;
        var fallback = new string((localized ?? "").Where(c => !char.IsWhiteSpace(c)).Select(char.ToLowerInvariant).ToArray());
        return fallback.Length > 0 ? fallback : "element";
    }
}

/// <summary>What the companion read about the focused element at the press (§13.1), before the mic is live. Windows fills it from
/// <c>IUIAutomation.GetFocusedElement</c>: the control type, <c>IsPassword</c>, <c>IsEnabled</c>, ValuePattern (+ IsReadOnly),
/// TextPattern (+ the <c>UIA_IsReadOnlyAttributeId</c> of the caret's range), the selection, the process.</summary>
public sealed record FocusFacts
{
    /// <summary>False when no focused element could be read: nothing focused, a GPU surface, a UIA timeout, an elevated target (UIPI).</summary>
    public bool Readable { get; init; } = true;

    /// <summary>The <c>UIA_*ControlTypeId</c>; 0 when unread.</summary>
    public int ControlType { get; init; }

    public string? LocalizedControlType { get; init; }

    /// <summary><c>UIA_IsPasswordPropertyId</c>: a secure field — delivered by the lease, never Return (§13.1, §7.4).</summary>
    public bool IsPassword { get; init; }

    public bool IsEnabled { get; init; } = true;

    public bool HasValuePattern { get; init; }

    /// <summary><c>ValuePattern.IsReadOnly</c>; null when the pattern is absent or did not answer.</summary>
    public bool? ValueReadOnly { get; init; }

    public bool HasTextPattern { get; init; }

    /// <summary>The caret range's <c>UIA_IsReadOnlyAttributeId</c>; null when unsupported (mixed or not answered).</summary>
    public bool? TextReadOnly { get; init; }

    /// <summary>The element's text can be read back (TextPattern or ValuePattern.Value answered): direct insertion needs its read-back (W-m).</summary>
    public bool CanReadBack { get; init; }

    /// <summary>Characters selected (TextPattern.GetSelection); 0 = a caret only; null = unknown.</summary>
    public int? SelectionLength { get; init; }

    /// <summary>The element belongs to the companion or theuth's own app (§13.8: "Use Ctrl+D here").</summary>
    public bool Ours { get; init; }

    /// <summary>The target process runs elevated and we do not (UIPI: reads and SendInput are blocked → unknown, §13.7).</summary>
    public bool Elevated { get; init; }

    public int ProcessId { get; init; }

    /// <summary>The app's display name ("Notepad") for the pill and the card.</summary>
    public string App { get; init; } = "";

    /// <summary>The executable's file name ("notepad.exe"): the <c>AppProfile.id</c> on Windows.</summary>
    public string Exe { get; init; } = "";

    public string Role => UiaControlType.Role(ControlType, LocalizedControlType);
}

/// <summary>§13.1's three verdicts.</summary>
public enum TargetVerdict
{
    /// <summary>An editable text element: direct insertion where it is allowed (W-m), else the lease.</summary>
    Editable,
    /// <summary>Nothing certain (panes, custom surfaces, terminals, unreadable or elevated targets): the lease + verification.</summary>
    Unknown,
    /// <summary>Certainly not a text target: refuse with a hint, record nothing.</summary>
    CertainlyNot,
}

/// <summary>How one utterance is delivered.</summary>
public enum DeliveryMethod
{
    /// <summary>Windows: <c>SendInput</c> with <c>KEYEVENTF_UNICODE</c>, ≤ 200 single-line characters (§13.2).</summary>
    Direct,
    /// <summary>The clipboard lease: snapshot → write → Ctrl+V → settle (§13.3).</summary>
    Lease,
    Refuse,
}

/// <summary>Why a target was refused (the hint's wording).</summary>
public enum RefusalReason
{
    None,
    /// <summary>A certainly-not role (button, menu item, static text, …): "Nothing to type into — ‹role›".</summary>
    NotATextTarget,
    /// <summary>A read-only field (ValuePattern.IsReadOnly / the text's read-only attribute, and nothing says writable).</summary>
    ReadOnly,
    /// <summary>A disabled field or control.</summary>
    Disabled,
    /// <summary>The companion's own windows or theuth's Composer (which has Ctrl+D).</summary>
    OwnWindow,
    /// <summary>The session is locked (WTS_SESSION_LOCK, §13.8): no delivery while locked.</summary>
    Locked,
}

/// <summary>The press-time decision about one target (§13.1), carried by the gesture.</summary>
public sealed record Assessment(TargetVerdict Verdict, bool Secure, string Role, RefusalReason Refusal, bool Rewrite, bool CanDirect, string App)
{
    public bool Refused => Verdict == TargetVerdict.CertainlyNot;
}

/// <summary>The kinds of §13.5's failure card (typed, logged, counted).</summary>
public enum DeliveryFailure
{
    NoEditableTarget,
    AccessibilityNotTrusted,
    ClipboardSnapshotFailed,
    ClipboardWriteFailed,
    PasteCommandFailed,
    TargetUnavailable,
    RestoreFailed,
    PasteNotLanded,
}

/// <summary>
/// 153 §13.1 target assessment — three-way, refusing only when certain — plus the per-utterance delivery choice (§13.2), the
/// gesture cap (§13.8) and the Return rule for "send it" (§7.4). Pure: the app passes what UIA said and the lock state.
/// </summary>
public static class DeliveryAssessment
{
    /// <summary>§13.2 (Windows): direct insertion is SendInput KEYEVENTF_UNICODE for at most this many characters per utterance.</summary>
    public const int DirectMaxChars = 60; // Windows: §13.2 says 200; a paced 107-char run garbled in Win11 Notepad (live round 4) — longer text takes the lease

    /// <summary>§13.8: more than this many characters in one gesture go to the card instead.</summary>
    public const int MaxGestureChars = 20_000;

    /// <summary>§3.2: the assessment's budget at the press, before the mic is live.</summary>
    public const int BudgetMs = 150;

    /// <summary>§3.2: the refusal hint's lifetime.</summary>
    public const int HintMs = 2500;

    /// <summary>§3.2: "✓ 3 lines into Mail" stays this long.</summary>
    public const int DoneMs = 1200;

    /// <summary>Roles that are certainly not text targets: §13.1's Windows column (Button, MenuItem, Text, Slider, CheckBox, Image,
    /// ListItem) plus the Windows forms of the macOS column (radio, menu, menubar, toolbar, table row) and their plain kin.</summary>
    private static readonly HashSet<int> CertainlyNotRoles = new()
    {
        UiaControlType.Button, UiaControlType.SplitButton, UiaControlType.MenuItem, UiaControlType.Menu, UiaControlType.MenuBar,
        UiaControlType.Text, UiaControlType.Slider, UiaControlType.CheckBox, UiaControlType.RadioButton, UiaControlType.Image,
        UiaControlType.ListItem, UiaControlType.TreeItem, UiaControlType.TabItem, UiaControlType.Hyperlink,
        UiaControlType.ToolBar, UiaControlType.AppBar, UiaControlType.ScrollBar, UiaControlType.Thumb, UiaControlType.ProgressBar,
        UiaControlType.StatusBar, UiaControlType.TitleBar, UiaControlType.Separator, UiaControlType.ToolTip, UiaControlType.Header,
        UiaControlType.HeaderItem,
    };

    /// <summary>The roles whose text patterns make them editable (§13.1: Edit/Document; ComboBox as the macOS AXComboBox twin).</summary>
    private static readonly HashSet<int> TextRoles = new() { UiaControlType.Edit, UiaControlType.Document, UiaControlType.ComboBox };

    /// <summary>Is this a certainly-not role (exposed for the tests' role table)?</summary>
    public static bool IsCertainlyNotRole(int controlType) => CertainlyNotRoles.Contains(controlType);

    /// <summary>
    /// The verdict for the focused element. <paramref name="locked"/>: the session is locked (WTS); <paramref name="terminal"/>: the
    /// app's profile says terminal (terminals take the lease: their text surfaces are "unknown", §13.2).
    /// </summary>
    public static Assessment Assess(FocusFacts f, bool locked = false, bool terminal = false)
    {
        string app = f.App.Length > 0 ? f.App : f.Exe;
        Assessment Refuse(RefusalReason why, string role) => new(TargetVerdict.CertainlyNot, false, role, why, false, false, app);
        Assessment Unknown(string role, bool secure = false) => new(TargetVerdict.Unknown, secure, role, RefusalReason.None, false, false, app);

        if (locked) return Refuse(RefusalReason.Locked, "locked screen");
        if (!f.Readable) return Unknown(f.Elevated ? "elevated window" : "unknown field");
        if (f.Ours) return Refuse(RefusalReason.OwnWindow, f.Role);
        int ct = f.ControlType;
        bool writable = (f.HasValuePattern && f.ValueReadOnly == false) || (f.HasTextPattern && f.TextReadOnly == false);
        bool readOnly = (f.HasValuePattern && f.ValueReadOnly == true) || (f.HasTextPattern && f.TextReadOnly == true);
        bool textRole = TextRoles.Contains(ct);
        string role = f.Role;

        // The certainly-not rows come first: a settable value never un-refuses them (a slider stays refused — H3-D1). Windows Terminal's
        // control reports Text WITH a TextPattern: not certain, so it falls through to unknown.
        bool textWithPattern = ct == UiaControlType.Text && f.HasTextPattern;
        if (!textWithPattern && CertainlyNotRoles.Contains(ct)) return Refuse(RefusalReason.NotATextTarget, role);
        if (f.Elevated) return Unknown("elevated window", f.IsPassword);

        TargetVerdict verdict;
        if (textRole)
        {
            // A text role that is not editable (read-only, disabled) is not certain → unknown: the lease and the verification decide (H3-D1).
            verdict = !f.IsEnabled || (readOnly && !writable) ? TargetVerdict.Unknown
                : writable || f.HasTextPattern ? TargetVerdict.Editable : TargetVerdict.Unknown; // §13.1: "Document without patterns" → unknown
        }
        else if (writable && f.IsEnabled && !textWithPattern) verdict = TargetVerdict.Editable; // the macOS "value settable" row, outside the certainly-not rows
        else verdict = TargetVerdict.Unknown; // Pane, Custom, Window, Group, lists, Text+TextPattern, unread (0): refuse only when certain

        // Terminals and Monaco-style editors: editable → unknown (the clipboard lease, never direct insertion); it never un-refuses.
        if (terminal && verdict == TargetVerdict.Editable) verdict = TargetVerdict.Unknown;

        // §13.1: secure → text allowed (the lease, no read-back), Return refused (§7.4).
        bool secure = f.IsPassword;
        bool editable = verdict == TargetVerdict.Editable;
        bool rewrite = editable && !secure && f.SelectionLength is > 0;
        bool canDirect = editable && !secure && f.CanReadBack;
        return new(verdict, secure, role, RefusalReason.None, rewrite, canDirect, app);
    }

    /// <summary>
    /// §13.2 for one utterance: refuse a certainly-not target; direct only for an editable, readable, non-secure target, a single line of
    /// at most <see cref="DirectMaxChars"/> characters without control characters, and only until a read-back has failed in this gesture
    /// (<paramref name="fellBack"/>: "this and every later utterance" take the lease); everything else takes the lease. Windows: while a
    /// key is physically held (<paramref name="keysHeld"/> — the chord of a hold), the lease's one atomic Ctrl+V burst is used, because
    /// paced typing cannot stay atomic against the held chord's auto-repeat.
    /// </summary>
    public static DeliveryMethod Method(Assessment a, string text, bool fellBack, bool keysHeld = false)
    {
        if (a.Refused) return DeliveryMethod.Refuse;
        if (!a.CanDirect || a.Secure || fellBack || keysHeld) return DeliveryMethod.Lease;
        if (text.Length == 0 || text.Length > DirectMaxChars) return DeliveryMethod.Lease;
        foreach (char c in text) if (char.IsControl(c)) return DeliveryMethod.Lease; // Enter/Tab typed would send or move focus
        return DeliveryMethod.Direct;
    }

    /// <summary>§13.8: does <paramref name="next"/> still fit in the gesture after <paramref name="deliveredChars"/>?</summary>
    public static bool FitsGesture(int deliveredChars, string next) => deliveredChars + next.Length <= MaxGestureChars;

    /// <summary>
    /// §7.4 any-app "send it": Return only if the target was assessed editable and is not secure, focus is still the captured element,
    /// and no physical modifier is down. The caption when it is withheld; null when Return may be pressed.
    /// </summary>
    public static string? ReturnWithheld(Assessment a, bool focusSame, bool modifierDown)
    {
        if (a.Secure) return "not sent — secure field";
        if (!focusSame) return "not sent — focus moved";
        if (a.Verdict == TargetVerdict.CertainlyNot) return "not sent — nothing to type into"; // H3-D2: editable and unknown may take Return
        if (modifierDown) return "not sent — a key is held";
        return null;
    }
}

/// <summary>§13.8 / §13.9: the session-lock gate. The app feeds <c>WM_WTSSESSION_CHANGE</c> (WTS_SESSION_LOCK 0x7 / UNLOCK 0x8);
/// nothing is delivered while locked.</summary>
public sealed class SessionLockGate
{
    public const int WtsSessionLock = 0x7;
    public const int WtsSessionUnlock = 0x8;

    public bool Locked { get; private set; }

    public SessionLockGate(bool lockedAtStart = false) => Locked = lockedAtStart;

    /// <summary>A <c>WM_WTSSESSION_CHANGE</c> wParam; true when the lock state changed.</summary>
    public bool OnSessionChange(int wtsCode)
    {
        bool before = Locked;
        if (wtsCode == WtsSessionLock) Locked = true;
        else if (wtsCode == WtsSessionUnlock) Locked = false;
        return before != Locked;
    }

    /// <summary>May an utterance be delivered now?</summary>
    public bool MayDeliver => !Locked;
}

/// <summary>Per-app profiles on Windows: <c>AppProfile.id</c> is the exe name (wave A); ids match with or without ".exe", any case.
/// Built-in terminal defaults (a writer's call, as-built): their text surfaces take the lease.</summary>
public static class AppMatch
{
    /// <summary>The user's default row (the any_app row): an app with no row of its own gets it, re-labelled with the app's id and name.</summary>
    public const string DefaultRowId = "any_app";

    /// <summary>Terminals and Monaco-style editors (H3-D1: the Mac list is Terminal, iTerm2, Ghostty, Warp, kitty, Alacritty, WezTerm, VS Code,
    /// Cursor, Zed): their surfaces take the clipboard lease, never direct insertion. <c>terminal ||= heuristics</c> — a profile cannot clear it.</summary>
    public static readonly IReadOnlyList<string> DefaultTerminals = new[]
    {
        "WindowsTerminal", "WindowsTerminalPreview", "OpenConsole", "conhost", "cmd", "powershell", "pwsh", "wezterm-gui", "wezterm", "alacritty",
        "mintty", "putty", "ConEmu", "ConEmu64", "Hyper", "Tabby", "Code", "Code - Insiders", "VSCodium", "Cursor", "Zed",
    };

    public static string Normalize(string exe)
    {
        var s = (exe ?? "").Trim();
        int slash = Math.Max(s.LastIndexOf('\\'), s.LastIndexOf('/'));
        if (slash >= 0) s = s[(slash + 1)..];
        if (s.EndsWith(".exe", StringComparison.OrdinalIgnoreCase)) s = s[..^4];
        return s.ToLowerInvariant();
    }

    public static bool Same(string profileId, string exe) => Normalize(profileId).Length > 0 && Normalize(profileId) == Normalize(exe);

    /// <summary>Is <paramref name="exe"/> a terminal? <c>terminal ||= heuristics</c>: the profile's flag OR the defaults.</summary>
    public static bool IsTerminal(string exe, bool? profileTerminal) =>
        profileTerminal == true || DefaultTerminals.Any(t => Same(t, exe));
}

/// <summary>The copy of the any-app states (§3.2 table, §13.5 card), one place so both the pill and the card say the same thing.</summary>
public static class AnyAppCopy
{
    /// <summary>"scratch that" with nothing of ours to take back (H3-D2, the Mac's words).</summary>
    public const string ScratchHint = "Nothing of ours to scratch";

    /// <summary>The pill at the press: "Dictating into Mail" · "Terminal · unknown field — paste" · "Nothing to type into — button".</summary>
    public static string Pill(Assessment a)
    {
        string app = a.App.Length > 0 ? a.App : "this app";
        return a.Refusal switch
        {
            RefusalReason.Locked => "Screen locked — nothing typed",
            RefusalReason.OwnWindow => "Use Ctrl+D here",
            RefusalReason.ReadOnly => $"Nothing to type into — read-only {a.Role}",
            RefusalReason.Disabled => $"Nothing to type into — disabled {a.Role}",
            RefusalReason.NotATextTarget => $"Nothing to type into — {a.Role}",
            _ when a.Rewrite => $"Rewrite in {app}",
            _ when a.Verdict == TargetVerdict.Unknown => $"{app} · {(a.Role == "elevated window" ? a.Role : "unknown field")} — paste",
            _ when a.Secure => $"Dictating into {app} · secure field",
            _ => $"Dictating into {app}",
        };
    }

    /// <summary>§13.6: "Rewrite · 312 chars selected · say what to change".</summary>
    public static string RewritePill(int selected) => $"Rewrite · {selected} chars selected · say what to change";

    /// <summary>The listening caption under the pill: "Ctrl+Alt+D held · shape on" or "… locked".</summary>
    /// <summary>The any-app pill's chord segment, the Mac trailer (H3-D2): "Ctrl+Alt+D held" / "Ctrl+Alt+D locked".</summary>
    public static string Chord(string chord, bool locked) => $"{chord} {(locked ? "locked" : "held")}";

    public static string Held(string chord, bool locked, bool shape) =>
        $"{chord} {(locked ? "locked" : "held")} · shape {(shape ? "on" : "off")}";

    /// <summary>The hint row: "Esc cancel · double-tap lock · say “send it”".</summary>
    public const string Hints = "Esc cancel · double-tap lock · say “send it”";

    /// <summary>§3.2 done: "✓ 3 lines into Mail" (lines = delivered utterances).</summary>
    public static string Done(int lines, string app) =>
        $"✓ {lines} {(lines == 1 ? "line" : "lines")} into {(app.Length > 0 ? app : "the app")}";

    /// <summary>The paused caption when focus left the target app (§3.2).</summary>
    public static string Paused(string app) => $"Come back to {(app.Length > 0 ? app : "the app")} to continue · Esc";

    /// <summary>§13.5's card title: "Couldn't type into Terminal · paste not landed".</summary>
    public static string CardTitle(string app, DeliveryFailure kind) =>
        $"Couldn't type into {(app.Length > 0 ? app : "the app")} · {Kind(kind)}";

    /// <summary>The kind as the card says it.</summary>
    public static string Kind(DeliveryFailure kind) => kind switch
    {
        DeliveryFailure.NoEditableTarget => "nothing to type into",
        DeliveryFailure.AccessibilityNotTrusted => "accessibility off",
        DeliveryFailure.ClipboardSnapshotFailed => "clipboard unavailable",
        DeliveryFailure.ClipboardWriteFailed => "clipboard write failed",
        DeliveryFailure.PasteCommandFailed => "paste failed",
        DeliveryFailure.TargetUnavailable => "the app went away",
        DeliveryFailure.RestoreFailed => "clipboard not restored",
        DeliveryFailure.PasteNotLanded => "paste not landed",
        _ => "failed",
    };

    /// <summary>§13.5's footer: "checked at 150 · 350 · 1000 ms — field unchanged · clipboard restored".</summary>
    public static string CardFooter(IReadOnlyList<int> checkedAtMs, string fieldState, bool? clipboardRestored)
    {
        var parts = new List<string>();
        if (checkedAtMs.Count > 0) parts.Add($"checked at {string.Join(" · ", checkedAtMs)} ms — {fieldState}");
        else if (fieldState.Length > 0) parts.Add(fieldState);
        if (clipboardRestored is { } r) parts.Add(r ? "clipboard restored" : "clipboard not restored");
        return string.Join(" · ", parts);
    }
}

/// <summary>The separator between delivered utterances (the Composer splice's rule, <c>UI/lib/dictation.ts:49-51</c>, as a lead space):
/// a space before the utterance unless the text already before the caret ends in whitespace (or is unknown / empty) or the utterance
/// starts with whitespace or closing punctuation. Lead-only spacing keeps stage 9's rule for terminals (no trailing space after a
/// <c>/command</c> or <c>@mention</c>, so a completion menu stays open until the next utterance).</summary>
public static class AnyAppSpacing
{
    /// <summary>The text to deliver for <paramref name="utterance"/>; <paramref name="beforeCaret"/>: what the field holds before the caret
    /// (null = unknown); <paramref name="previous"/>: this gesture's previously delivered utterance (null for the first).</summary>
    public static string Lead(string utterance, string? beforeCaret, string? previous)
    {
        if (utterance.Length == 0) return utterance;
        char first = utterance[0];
        if (char.IsWhiteSpace(first) || first is '.' or ',' or ';' or ':' or '!' or '?' or ')' or ']' or '}') return utterance;
        string? left = previous ?? beforeCaret;
        if (previous is not null && beforeCaret is { Length: > 0 }) left = beforeCaret; // the field's own text wins when it was read
        if (string.IsNullOrEmpty(left) || char.IsWhiteSpace(left[^1])) return utterance;
        return " " + utterance;
    }
}