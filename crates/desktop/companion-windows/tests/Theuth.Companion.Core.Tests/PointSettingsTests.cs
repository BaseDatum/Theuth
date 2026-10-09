namespace Theuth.Companion.Core.Tests;

public sealed class PointSettingsTests
{
    [Fact]
    public void DefaultsRoundTripAndTolerateJunk()
    {
        var store = new SettingsStore();
        var s = PointSettings.Load(store);
        Assert.Equal(new PointSettings(), s);
        Assert.True(s.Enabled && s.IncludeScreen && !s.Speak && s.AccessibilityOn && !s.AnnotateMouse); // Speak is opt-in since 2026-10-09 (Sky): off by default
        s.Annotate = "mouse";
        s.Accessibility = "off";
        s.IncludeScreen = false;
        s.HintsShown = 2;
        s.Save(store);
        var back = PointSettings.Load(store);
        Assert.Equal(s, back);
        Assert.True(back.AnnotateMouse);
        Assert.False(back.AccessibilityOn);
        store.Set(PointSettings.Key.Annotate, "sideways");
        store.Set(PointSettings.Key.Accessibility, "maybe");
        var junk = PointSettings.Load(store);
        Assert.Equal("always", junk.Annotate);
        Assert.Equal("auto", junk.Accessibility);
    }

    [Fact]
    public void BodyAndHintsMatchTheWebview()
    {
        Assert.Equal("[voice note, 1 s] hi", PointFlow.NoteText("hi", 10));
        Assert.Equal("[voice note, 3 s] hi", PointFlow.NoteText("hi", 2001));
        Assert.Equal("[voice note, 2 s] what is this", PointFlow.ComposeBody("  what is this ", 1500, Array.Empty<string>()));
        Assert.Equal("[voice note, 2 s] x\n[screen 1/1 …]\n[screen 2/2 …]", PointFlow.ComposeBody("x", 1500, new[] { "[screen 1/1 …]", "[screen 2/2 …]" }));
        Assert.Equal("Meeting running — use the island to ask", PointFlow.RefusalHint("talking"));
        Assert.Equal("dictating elsewhere", PointFlow.RefusalHint("already dictating"));
        Assert.Equal("Screenshots off — sent without the screen", PointFlow.RefusalHint("screen"));
        Assert.Equal("boom", PointFlow.RefusalHint("boom"));
        // 149 NATIVE: the trail is the system accent — Windows' default until the app reads the live one, then whatever it sets.
        var before = PointFlow.TrailRgb;
        try
        {
            PointFlow.TrailRgb = PointFlow.WindowsDefaultAccentRgb;
            Assert.Equal("#0078d4", PointFlow.TrailHex);
            PointFlow.TrailRgb = PointFlow.PackRgb(0x12, 0xab, 0xef);
            Assert.Equal("#12abef", PointFlow.TrailHex);
            Assert.Equal(0x12abefu, PointFlow.TrailRgb);
        }
        finally { PointFlow.TrailRgb = before; }
        Assert.Equal("00:03 · Ctrl+Alt+Space held · theuth's desk", PointFlow.ListeningStatus(3200, "Ctrl+Alt+Space", false));
        Assert.Equal("00:03 · locked — tap to send · theuth's desk", PointFlow.ListeningStatus(3200, "Ctrl+Alt+Space", true));
    }
}