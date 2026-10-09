namespace Theuth.Companion.Core;

/// <summary>
/// Settings › Voice key (149 §3.8, D19), re-homed from the webview's <c>voiceKey</c> localStorage into the companion's
/// settings store under their own keys (WW3's; the shared <see cref="CompanionSettings"/> is untouched): enabled · include
/// the screen · read what the pointer touched (<c>auto</c> | <c>off</c>) · annotate <c>always</c> | <c>mouse</c> · speak the
/// answer (96's "speak the answer", Revision 1 #10) · the hint counter (shown on the first <see cref="HintUses"/> uses).
/// </summary>
public sealed class PointSettings : IEquatable<PointSettings>
{
    public const int HintUses = 3;

    public bool Enabled { get; set; } = true;

    public bool IncludeScreen { get; set; } = true;

    /// <summary><c>auto</c> (the engine reads the element under the pointer when it can) or <c>off</c>.</summary>
    public string Accessibility { get; set; } = "auto";

    /// <summary><c>always</c> (the whole hold draws) or <c>mouse</c> (only while the left button is held).</summary>
    public string Annotate { get; set; } = "always";

    public bool Speak { get; set; } = true;

    public int HintsShown { get; set; }

    public bool AccessibilityOn => Accessibility != "off";

    public bool AnnotateMouse => Annotate == "mouse";

    public static class Key
    {
        public const string Enabled = "voiceKeyEnabled", IncludeScreen = "voiceKeyIncludeScreen", Accessibility = "voiceKeyAccessibility";
        public const string Annotate = "voiceKeyAnnotate", Speak = "voiceKeySpeak", HintsShown = "voiceKeyHintsShown";
    }

    public static PointSettings Load(SettingsStore d)
    {
        var s = new PointSettings();
        if (d.Contains(Key.Enabled)) s.Enabled = d.Bool(Key.Enabled);
        if (d.Contains(Key.IncludeScreen)) s.IncludeScreen = d.Bool(Key.IncludeScreen);
        if (d.String(Key.Accessibility) is "off") s.Accessibility = "off";
        if (d.String(Key.Annotate) is "mouse") s.Annotate = "mouse";
        if (d.Contains(Key.Speak)) s.Speak = d.Bool(Key.Speak);
        s.HintsShown = (int)Math.Max(0, d.Double(Key.HintsShown));
        return s;
    }

    public void Save(SettingsStore d)
    {
        d.Set(Key.Enabled, Enabled);
        d.Set(Key.IncludeScreen, IncludeScreen);
        d.Set(Key.Accessibility, Accessibility);
        d.Set(Key.Annotate, Annotate);
        d.Set(Key.Speak, Speak);
        d.Set(Key.HintsShown, (double)HintsShown);
        d.Flush();
    }

    public bool Equals(PointSettings? o) =>
        o is not null && Enabled == o.Enabled && IncludeScreen == o.IncludeScreen && Accessibility == o.Accessibility
        && Annotate == o.Annotate && Speak == o.Speak && HintsShown == o.HintsShown;

    public override bool Equals(object? obj) => Equals(obj as PointSettings);

    public override int GetHashCode() => HashCode.Combine(Enabled, IncludeScreen, Accessibility, Annotate, Speak, HintsShown);
}

/// <summary>The pure bits of the point-and-say flow (<c>UI/lib/voice-key.ts</c>): the note line and the refusal hints.</summary>
public static class PointFlow
{
    /// <summary>The wait for the dictation's final flush before sending what we have (<c>FINAL_TEXT_MS</c>).</summary>
    public const int FinalTextMs = 8000;

    /// <summary>How long a hint stays (<c>HINT_MS</c>).</summary>
    public const int HintMs = 2000;

    /// <summary>The desk's error in the card (<c>ERROR_MS</c>).</summary>
    public const int ErrorMs = 4000;

    /// <summary>
    /// 149 NATIVE: the overlay trail is the system accent (the trail while the key is held = recording = accent), also sent
    /// to <c>screen_context</c> so the engine stamps the path in the same colour. The app sets it from <c>SystemAccentColor</c>
    /// at start and on every accent change; until then it is Windows' own default accent (0078D4), not a theuth palette colour.
    /// </summary>
    public static uint TrailRgb { get; set; } = WindowsDefaultAccentRgb;

    /// <summary>Windows' default accent colour, the value before the app reads the live one.</summary>
    public const uint WindowsDefaultAccentRgb = 0x0078d4;

    public static string TrailHex => $"#{TrailRgb:x6}";

    /// <summary>Pack an RGB triple the way the raster and <see cref="TrailHex"/> read it.</summary>
    public static uint PackRgb(byte r, byte g, byte b) => ((uint)r << 16) | ((uint)g << 8) | b;

    /// <summary><c>[voice note, N s] ‹text›</c> (45 §4.4, <c>lib/voice-note.ts</c>): at least one second.</summary>
    public static string NoteText(string text, long durationMs) => $"[voice note, {Math.Max(1, (long)Math.Ceiling(durationMs / 1000.0))} s] {text}";

    /// <summary>The note plus the engine's <c>[screen …]</c> lines (<c>composeBody</c>).</summary>
    public static string ComposeBody(string text, long durationMs, IReadOnlyList<string> lines)
    {
        var note = NoteText(text.Trim(), durationMs);
        return lines.Count == 0 ? note : note + "\n" + string.Join("\n", lines);
    }

    /// <summary>§3.7 / §5.4 D: the standing refusal reasons → what the voice mode says.</summary>
    public static string RefusalHint(string reason) => reason switch
    {
        "talking" or "meeting" => "Meeting running — use the island to ask",
        "already dictating" or "dictating" => "dictating elsewhere",
        "screen" => "Screenshots off — sent without the screen",
        "hub" => "theuth's desk lives on another engine",
        "disconnected" => "Not connected",
        "disabled" => "Voice key is off in Settings",
        _ => reason,
    };

    /// <summary>What the voice mode's status line says while listening: <c>‹clock› · Ctrl+Alt+Space held · theuth's desk</c>.</summary>
    public static string ListeningStatus(long heldMs, string chord, bool locked) =>
        $"{IslandModel.Clock(heldMs)} · {(locked ? "locked — tap to send" : chord + " held")} · theuth's desk";
}