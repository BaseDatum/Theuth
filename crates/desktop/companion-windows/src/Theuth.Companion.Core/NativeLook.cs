namespace Theuth.Companion.Core;

/// <summary>
/// The tone of a status mark under the native look (Sky's ruling of 2026-10-09 08:33, "it's supposed to be fully native";
/// the shared macOS/Windows table of 08:39). Colour comes from the system theme, never from theuth's web palette:
/// needs-you = the accent, running = no colour (a small <c>ProgressRing</c>), done = Success, failed = Critical,
/// paused = Caution, remote/unknown = a hollow ring in tertiary text, idle = neutral text colour.
/// </summary>
public enum StatusTone { NeedsYou, Running, Done, Failed, Paused, Remote, Idle }

/// <summary>
/// Status → WinUI theme-resource key, and the Segoe Fluent Icons glyphs that replace every emoji / text character the
/// surfaces used as an icon. Portable so the table is tested on the Mac; the app looks the keys up live
/// (<c>Theme.Brush</c>) so light/dark and the user's accent are always the system's.
/// </summary>
public static class NativeLook
{
    // Brush resource keys (WinUI 3 common theme resources; all exist in XamlControlsResources' Default/Light dictionaries).
    public const string Accent = "AccentFillColorDefaultBrush";
    public const string AccentSecondary = "AccentFillColorSecondaryBrush";
    public const string AccentTertiary = "AccentFillColorTertiaryBrush";
    public const string OnAccent = "TextOnAccentFillColorPrimaryBrush";
    /// <summary>Accent-coloured TEXT (not a fill): WinUI's text variants (Light3/Light2 in dark, Dark2/Dark1 in light) keep it readable.</summary>
    public const string AccentText = "AccentTextFillColorPrimaryBrush";
    public const string AccentTextSecondary = "AccentTextFillColorSecondaryBrush";
    public const string AccentTextTertiary = "AccentTextFillColorTertiaryBrush";
    public const string Success = "SystemFillColorSuccessBrush";
    public const string Critical = "SystemFillColorCriticalBrush";
    public const string Caution = "SystemFillColorCautionBrush";
    public const string Attention = "SystemFillColorAttentionBrush";
    public const string TextPrimary = "TextFillColorPrimaryBrush";
    public const string TextSecondary = "TextFillColorSecondaryBrush";
    public const string TextTertiary = "TextFillColorTertiaryBrush";
    public const string TextDisabled = "TextFillColorDisabledBrush";
    public const string CardBackground = "CardBackgroundFillColorDefaultBrush";
    public const string CardBackgroundSecondary = "CardBackgroundFillColorSecondaryBrush";
    public const string CardStroke = "CardStrokeColorDefaultBrush";
    public const string Divider = "DividerStrokeColorDefaultBrush";
    public const string ControlFill = "ControlFillColorDefaultBrush";
    public const string ControlFillSecondary = "ControlFillColorSecondaryBrush";
    public const string ControlFillTertiary = "ControlFillColorTertiaryBrush";
    public const string ControlStroke = "ControlStrokeColorDefaultBrush";
    public const string SubtleFill = "SubtleFillColorTransparentBrush";
    public const string SubtleFillSecondary = "SubtleFillColorSecondaryBrush";
    public const string SubtleFillTertiary = "SubtleFillColorTertiaryBrush";
    public const string SolidBackgroundBase = "SolidBackgroundFillColorBaseBrush";
    public const string SolidBackgroundSecondary = "SolidBackgroundFillColorSecondaryBrush";
    public const string AcrylicInApp = "AcrylicInAppFillColorDefaultBrush";
    public const string LayerFill = "LayerFillColorDefaultBrush";

    /// <summary>The shared status table (08:39): mark kinds of the rail and the pills.</summary>
    public static StatusTone Tone(MarkStateKind kind) => kind switch
    {
        MarkStateKind.NeedsYou => StatusTone.NeedsYou,
        MarkStateKind.Working => StatusTone.Running,
        MarkStateKind.Finished => StatusTone.Done,
        MarkStateKind.Error => StatusTone.Failed,
        MarkStateKind.Lapsed => StatusTone.Paused, // Lapsed is AgentStatus.Paused (RailModel)
        _ => StatusTone.Idle,
    };

    /// <summary>The same table for the agents panel's rows.</summary>
    public static StatusTone Tone(AgentRowState state) => state switch
    {
        AgentRowState.NeedsYou => StatusTone.NeedsYou,
        AgentRowState.Working => StatusTone.Running,
        AgentRowState.Done => StatusTone.Done,
        AgentRowState.Error => StatusTone.Failed,
        AgentRowState.Lapsed => StatusTone.Paused,
        AgentRowState.Remote => StatusTone.Remote,
        _ => StatusTone.Idle,
    };

    /// <summary>The brush that paints the mark for a tone (fill of a disc, or the stroke of a hollow ring).</summary>
    public static string Brush(StatusTone tone) => tone switch
    {
        StatusTone.NeedsYou => Accent,
        StatusTone.Running => TextSecondary,
        StatusTone.Done => Success,
        StatusTone.Failed => Critical,
        StatusTone.Paused => Caution,
        StatusTone.Remote => TextTertiary,
        _ => TextSecondary,
    };

    /// <summary>Hollow marks are a stroked ring: remote/unknown (E7 fallback) and idle (nothing happening inside).</summary>
    public static bool IsHollow(StatusTone tone) => tone is StatusTone.Remote or StatusTone.Idle;

    /// <summary>Running is drawn by a native <c>ProgressRing</c>, not a coloured disc.</summary>
    public static bool IsSpinner(StatusTone tone) => tone == StatusTone.Running;

    /// <summary>Segoe Fluent Icons (Windows 11); the MDL2 set has the same code points for every glyph used here.</summary>
    public const string IconFont = "Segoe Fluent Icons";
    public const string IconFontFallback = "Segoe MDL2 Assets";

    /// <summary>WCAG relative luminance of an sRGB colour (0 = black, 1 = white).</summary>
    public static double Luminance(byte r, byte g, byte b)
    {
        static double Lin(byte c)
        {
            double s = c / 255.0;
            return s <= 0.04045 ? s / 12.92 : Math.Pow((s + 0.055) / 1.055, 2.4);
        }
        return 0.2126 * Lin(r) + 0.7152 * Lin(g) + 0.0722 * Lin(b);
    }

    /// <summary>WCAG contrast ratio of two sRGB colours (1–21).</summary>
    public static double Contrast((byte R, byte G, byte B) a, (byte R, byte G, byte B) b)
    {
        double la = Luminance(a.R, a.G, a.B), lb = Luminance(b.R, b.G, b.B);
        return (Math.Max(la, lb) + 0.05) / (Math.Min(la, lb) + 0.05);
    }

    /// <summary>Text on an accent fill below this contrast is unreadable (WCAG AA for small text).</summary>
    public const double MinTextContrast = 4.5;

    /// <summary>
    /// Island fix R1: text on an accent fill uses <see cref="OnAccent"/>; Windows derives it per theme (black in dark, white in
    /// light), which fails on a very dark system accent in dark mode (build-B: fill #2C06AB, black text = 1.7:1). When the theme's
    /// on-accent colour falls below <see cref="MinTextContrast"/> on the fill, the other theme's on-accent colour is used.
    /// Returns true when the theme's own on-accent colour is fine.
    /// </summary>
    public static bool OnAccentReadable((byte R, byte G, byte B) fill, (byte R, byte G, byte B) onAccent) => Contrast(fill, onAccent) >= MinTextContrast;

    /// <summary>White or black, whichever reads better on <paramref name="fill"/> (the fallback when the system on-accent colour fails).</summary>
    public static (byte R, byte G, byte B) BestTextOn((byte R, byte G, byte B) fill) =>
        Contrast(fill, (255, 255, 255)) >= Contrast(fill, (0, 0, 0)) ? ((byte)255, (byte)255, (byte)255) : ((byte)0, (byte)0, (byte)0);

    /// <summary>Old character → glyph (the icon table of the as-built).</summary>
    public static class Glyph
    {
        public const string Microphone = "\uE720";   // 🎙
        public const string Camera = "\uE722";       // 📷 / ▣ P
        public const string Cancel = "\uE711";       // ✕ · ×
        public const string More = "\uE712";         // ⋯ / …
        public const string Up = "\uE74A";           // ↑ (send)
        public const string Forward = "\uE72A";      // → (go / next)
        public const string ChevronLeft = "\uE76B";  // ‹
        public const string ChevronRight = "\uE76C"; // ›
        public const string ChevronUp = "\uE70E";    // ˄ (collapse the island)
        public const string GripperBarVertical = "\uE784"; // ⠿ (drag grip)
        public const string People = "\uE716";
        public const string Flow = "\uE7C5";         // ⑂N children count
        public const string Pause = "\uE769";
        public const string Stop = "\uE71A";
        public const string View = "\uE890";         // ◉ eye (visible in capture)
        public const string Hide = "\uED1A";         // ○ eye (hidden from capture)
        public const string Settings = "\uE713";
        public const string Accept = "\uE8FB";       // ✓
        public const string Globe = "\uE774";        // 🌐
        public const string Refresh = "\uE72C";
        public const string Keyboard = "\uE765";
        public const string Volume = "\uE767";
        public const string Attach = "\uE723";
        public const string Warning = "\uE7BA";
    }

    /// <summary>Every glyph the table carries, for the test that pins them to the Fluent/MDL2 range.</summary>
    public static IReadOnlyList<(string Name, string Glyph)> Glyphs { get; } = new[]
    {
        (nameof(Glyph.Microphone), Glyph.Microphone), (nameof(Glyph.Camera), Glyph.Camera), (nameof(Glyph.Cancel), Glyph.Cancel),
        (nameof(Glyph.More), Glyph.More), (nameof(Glyph.Up), Glyph.Up), (nameof(Glyph.Forward), Glyph.Forward),
        (nameof(Glyph.ChevronLeft), Glyph.ChevronLeft), (nameof(Glyph.ChevronRight), Glyph.ChevronRight), (nameof(Glyph.ChevronUp), Glyph.ChevronUp),
        (nameof(Glyph.GripperBarVertical), Glyph.GripperBarVertical), (nameof(Glyph.People), Glyph.People), (nameof(Glyph.Flow), Glyph.Flow),
        (nameof(Glyph.Pause), Glyph.Pause), (nameof(Glyph.Stop), Glyph.Stop), (nameof(Glyph.View), Glyph.View), (nameof(Glyph.Hide), Glyph.Hide),
        (nameof(Glyph.Settings), Glyph.Settings), (nameof(Glyph.Accept), Glyph.Accept), (nameof(Glyph.Globe), Glyph.Globe),
        (nameof(Glyph.Refresh), Glyph.Refresh), (nameof(Glyph.Keyboard), Glyph.Keyboard), (nameof(Glyph.Volume), Glyph.Volume),
        (nameof(Glyph.Attach), Glyph.Attach), (nameof(Glyph.Warning), Glyph.Warning),
    };

    /// <summary>
    /// Key caps say key names the Windows way ("Ctrl", "Shift", "Alt", "Enter", "Esc"), never ⌃⇧⏎ or ⎋: the symbols the
    /// Mac form and the old captions used, mapped to words.
    /// </summary>
    public static string KeyCapText(string cap) => cap switch
    {
        "⏎" or "↩" or "↵" => "Enter",
        "⎋" => "Esc",
        "⌃" => "Ctrl",
        "⇧" => "Shift",
        "⌥" => "Alt",
        "⌘" => "Win",
        "⌫" => "Backspace",
        "␣" => "Space",
        "↑" => "Up",
        "↓" => "Down",
        "←" => "Left",
        "→" => "Right",
        "esc" => "Esc",
        _ => cap,
    };

    /// <summary>True when a string contains a character the native look forbids as an icon stand-in (emoji, arrows, geometric shapes, braille, dingbats).</summary>
    public static bool HasIconCharacter(string text)
    {
        for (int i = 0; i < text.Length; i++)
        {
            int cp = char.ConvertToUtf32(text, i);
            if (char.IsHighSurrogate(text[i])) i++;
            if (cp is >= 0x2190 and <= 0x21FF) return true;   // arrows
            if (cp is >= 0x2300 and <= 0x23FF) return true;   // technical (⏎ ⎋ ⌃)
            if (cp is >= 0x25A0 and <= 0x25FF) return true;   // geometric shapes (● ○ ▬ ▣)
            if (cp is >= 0x2600 and <= 0x27BF) return true;   // misc symbols + dingbats (✕ ✓ ★)
            if (cp is >= 0x2800 and <= 0x28FF) return true;   // braille (⠿)
            if (cp is >= 0x2440 and <= 0x24FF) return true;   // OCR + enclosed (⑂ ①)
            if (cp is >= 0x1F000 and <= 0x1FAFF) return true; // emoji
            if (cp is 0x00D7 or 0x22EF or 0x2022) return true; // × ⋯ •
        }
        return false;
    }
}