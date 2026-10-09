namespace Theuth.Companion.Core.Tests;

// 149 NATIVE (Sky, 2026-10-09 08:33: "it's supposed to be fully native"): the status → WinUI theme-resource table shared with the
// macOS companion (08:39), the Segoe Fluent Icons glyph table, and the Windows key-cap words.

public sealed class NativeLookTests
{
    [Fact]
    public void SharedStatusTableHoldsForMarksAndRows()
    {
        Assert.Equal(NativeLook.Accent, NativeLook.Brush(NativeLook.Tone(MarkStateKind.NeedsYou)));
        Assert.Equal(NativeLook.Accent, NativeLook.Brush(NativeLook.Tone(AgentRowState.NeedsYou)));
        Assert.Equal(NativeLook.Success, NativeLook.Brush(NativeLook.Tone(MarkStateKind.Finished)));
        Assert.Equal(NativeLook.Success, NativeLook.Brush(NativeLook.Tone(AgentRowState.Done)));
        Assert.Equal(NativeLook.Critical, NativeLook.Brush(NativeLook.Tone(MarkStateKind.Error)));
        Assert.Equal(NativeLook.Critical, NativeLook.Brush(NativeLook.Tone(AgentRowState.Error)));
        Assert.Equal(NativeLook.Caution, NativeLook.Brush(NativeLook.Tone(MarkStateKind.Lapsed))); // Lapsed = AgentStatus.Paused
        Assert.Equal(NativeLook.Caution, NativeLook.Brush(NativeLook.Tone(AgentRowState.Lapsed)));
        Assert.Equal(NativeLook.TextTertiary, NativeLook.Brush(NativeLook.Tone(AgentRowState.Remote)));
    }

    [Fact]
    public void RunningIsASpinnerWithNoStatusColour()
    {
        var t = NativeLook.Tone(MarkStateKind.Working);
        Assert.Equal(StatusTone.Running, t);
        Assert.Equal(t, NativeLook.Tone(AgentRowState.Working));
        Assert.True(NativeLook.IsSpinner(t));
        Assert.False(NativeLook.IsHollow(t));
        Assert.Equal(NativeLook.TextSecondary, NativeLook.Brush(t));
    }

    [Fact]
    public void RemoteAndIdleAreHollowEverythingElseIsFilled()
    {
        Assert.True(NativeLook.IsHollow(StatusTone.Remote));
        Assert.True(NativeLook.IsHollow(StatusTone.Idle));
        foreach (var t in new[] { StatusTone.NeedsYou, StatusTone.Done, StatusTone.Failed, StatusTone.Paused, StatusTone.Running })
            Assert.False(NativeLook.IsHollow(t));
        Assert.Equal(StatusTone.Idle, NativeLook.Tone(MarkStateKind.Idle));
        Assert.Equal(StatusTone.Idle, NativeLook.Tone(AgentRowState.Idle));
    }

    [Fact]
    public void EveryToneHasABrushAndEveryStateHasATone()
    {
        foreach (var t in Enum.GetValues<StatusTone>()) Assert.EndsWith("Brush", NativeLook.Brush(t));
        foreach (var k in Enum.GetValues<MarkStateKind>()) Assert.True(Enum.IsDefined(NativeLook.Tone(k)));
        foreach (var s in Enum.GetValues<AgentRowState>()) Assert.True(Enum.IsDefined(NativeLook.Tone(s)));
    }

    [Fact]
    public void NoThemeColourIsAHex()
    {
        // Every brush is a resource key the system resolves, never a colour literal.
        foreach (var t in Enum.GetValues<StatusTone>()) Assert.DoesNotContain("#", NativeLook.Brush(t));
        foreach (var key in new[] { NativeLook.Accent, NativeLook.AccentText, NativeLook.AccentTextSecondary, NativeLook.OnAccent, NativeLook.TextPrimary, NativeLook.TextSecondary, NativeLook.TextTertiary, NativeLook.CardBackground, NativeLook.CardStroke, NativeLook.Divider, NativeLook.ControlFill, NativeLook.SolidBackgroundBase, NativeLook.AcrylicInApp })
        {
            Assert.Matches("^[A-Za-z]+Brush$", key);
        }
    }

    [Fact]
    public void AccentTextIsATextResourceNotAFill()
    {
        // R1: accent-coloured text uses the AccentText* resources (readable Light3/Light2 in dark); fills keep AccentFillColorDefault.
        Assert.StartsWith("AccentTextFillColor", NativeLook.AccentText);
        Assert.StartsWith("AccentTextFillColor", NativeLook.AccentTextSecondary);
        Assert.StartsWith("AccentFillColor", NativeLook.Accent);
        Assert.Equal("TextOnAccentFillColorPrimaryBrush", NativeLook.OnAccent);
        Assert.NotEqual(NativeLook.AccentText, NativeLook.Accent);
    }

    [Fact]
    public void GlyphsArePrivateUseCodePointsOfSegoeFluentIcons()
    {
        Assert.Equal("Segoe Fluent Icons", NativeLook.IconFont);
        Assert.Equal("Segoe MDL2 Assets", NativeLook.IconFontFallback);
        Assert.True(NativeLook.Glyphs.Count >= 16);
        foreach (var (name, g) in NativeLook.Glyphs)
        {
            Assert.Equal(1, g.Length);
            Assert.InRange(g[0], (char)0xE700, (char)0xF8FF); // the PUA range both fonts use
            Assert.False(NativeLook.HasIconCharacter(g), name);
        }
        Assert.Equal("\uE720", NativeLook.Glyph.Microphone);
        Assert.Equal("\uE722", NativeLook.Glyph.Camera);
        Assert.Equal("\uE711", NativeLook.Glyph.Cancel);
        Assert.Equal("\uE712", NativeLook.Glyph.More);
        Assert.Equal("\uE74A", NativeLook.Glyph.Up);
        Assert.Equal("\uE72A", NativeLook.Glyph.Forward);
        Assert.Equal("\uE76B", NativeLook.Glyph.ChevronLeft);
        Assert.Equal("\uE76C", NativeLook.Glyph.ChevronRight);
        Assert.Equal("\uE784", NativeLook.Glyph.GripperBarVertical);
        Assert.Equal("\uE7C5", NativeLook.Glyph.Flow);
    }

    [Fact]
    public void KeyCapsSayWindowsWords()
    {
        Assert.Equal("Enter", NativeLook.KeyCapText("⏎"));
        Assert.Equal("Esc", NativeLook.KeyCapText("⎋"));
        Assert.Equal("Esc", NativeLook.KeyCapText("esc"));
        Assert.Equal("Ctrl", NativeLook.KeyCapText("⌃"));
        Assert.Equal("Shift", NativeLook.KeyCapText("⇧"));
        Assert.Equal("Alt", NativeLook.KeyCapText("⌥"));
        Assert.Equal("J", NativeLook.KeyCapText("J"));
        Assert.Equal("Ctrl+Alt+Space", NativeLook.KeyCapText("Ctrl+Alt+Space"));
        foreach (var cap in new[] { "⏎", "⎋", "⌃", "⇧", "⌥", "↑", "esc" }) Assert.False(NativeLook.HasIconCharacter(NativeLook.KeyCapText(cap)));
    }

    [Fact]
    public void IconCharacterDetectorCatchesTheOldStandIns()
    {
        foreach (var s in new[] { "🎙", "📷", "✕", "↑", "⏎", "⠿", "▬", "●", "○", "⋯", "◉", "▣", "⑂", "🌐", "×", "•", "★", "✓" })
            Assert.True(NativeLook.HasIconCharacter(s), s);
        // Typography that is text, not an icon, passes: dashes, the middle dot, the ellipsis, chevron quotes in prose.
        foreach (var s in new[] { "Finished just now · theuth", "sent — waiting", "Listening…", "Ctrl+Shift+Space", "", "1920×1080".Replace("×", "x") })
            Assert.False(NativeLook.HasIconCharacter(s), s);
    }
}