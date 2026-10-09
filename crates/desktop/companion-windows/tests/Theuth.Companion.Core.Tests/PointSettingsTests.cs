namespace Theuth.Companion.Core.Tests;

public sealed class PointSettingsTests
{
    [Fact]
    public void DefaultsRoundTripAndTolerateJunk()
    {
        var store = new SettingsStore();
        var s = PointSettings.Load(store);
        Assert.Equal(new PointSettings(), s);
        Assert.True(s.Enabled && s.IncludeScreen && s.Speak && s.AccessibilityOn && !s.AnnotateMouse);
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
        Assert.Equal("#4a8a6e", PointFlow.TrailHex);
        Assert.Equal("00:03 · Ctrl+Alt+Space held · theuth's desk", PointFlow.ListeningStatus(3200, "Ctrl+Alt+Space", false));
        Assert.Equal("00:03 · locked — tap to send · theuth's desk", PointFlow.ListeningStatus(3200, "Ctrl+Alt+Space", true));
    }
}
