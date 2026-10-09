namespace Theuth.Companion.Core.Tests;

/// <summary>153 W153B: Settings › Dictation's companion-local values and the suggestion card's words (pure).</summary>
public sealed class DictationSettingsTests
{
    [Fact]
    public void DefaultsRoundTripThroughTheStoreUnderTheirOwnKeys()
    {
        var store = new SettingsStore();
        var fresh = DictationSettings.Load(store);
        Assert.True(fresh.MicOnKeyDown); // §8.5: default on
        Assert.Empty(fresh.ShapePresets);
        Assert.Null(fresh.EndPhrases);
        Assert.Null(fresh.PresetFor(Surface.Companion));

        var s = fresh.Clone();
        s.MicOnKeyDown = false;
        s.ShapePresets["companion"] = "plain";
        s.ShapePresets["any_app"] = "off";
        s.EndPhrases = new EndPhrases { Send = ValueList.Of("send it", "ship it") };
        s.Save(store);
        Assert.False(store.Bool(DictationSettings.Key.MicOnKeyDown));
        Assert.Equal("plain", store.String("shapePreset.companion"));
        Assert.Equal("""{"send":["send it","ship it"],"scratch":["scratch that"],"literal":"literal"}""", store.String("endPhrases"));
        var back = DictationSettings.Load(store);
        Assert.Equal(s, back);
        Assert.Equal(ShapePreset.Plain, back.PresetFor(Surface.Companion));
        Assert.Equal(ShapePreset.Off, back.PresetFor(Surface.AnyApp));
        Assert.Null(back.PresetFor(Surface.Tui));

        store.Set("shapePreset.tui", "loud"); // an unknown word is ignored, never throws
        store.Set("endPhrases", "{not json");
        var odd = DictationSettings.Load(store);
        Assert.Null(odd.PresetFor(Surface.Tui));
        Assert.Null(odd.EndPhrases);
    }

    [Fact]
    public void PhrasesParseFromTheField()
    {
        Assert.Equal(new[] { "send it", "ship it" }, DictationSettings.ParsePhrases(" send it, ship it ,, Send It\n"));
        Assert.Empty(DictationSettings.ParsePhrases("  "));
        Assert.Equal("send it, ship it", DictationSettings.JoinPhrases(new[] { "send it", "ship it" }));
    }

    [Fact]
    public void TheLastDictationLineNamesEveryStageThatHappened()
    {
        Assert.Null(DictationSettings.TimingsLine(null));
        var t = new DictationTimings { Surface = Surface.Companion, PressToFirstPcmMs = 42, PressToFirstPartialMs = 310, PressToFirstFinalMs = 890, StopToFinalMs = 420 };
        Assert.Equal("Companion · mic 42 ms · first words 310 ms · final 890 ms · stop → final 420 ms", DictationSettings.TimingsLine(t));
        Assert.Equal("Other apps · polish 800 ms · Microphone Array (Realtek)", DictationSettings.TimingsLine(new DictationTimings { Surface = Surface.AnyApp, PolishMs = 800, Device = "Microphone Array (Realtek)" }));
    }

    [Fact]
    public void TheSuggestionCardSaysFrameDsWords()
    {
        var s = new DictionarySuggestion { Id = "s1", Heard = "parakeet tee dee tee", Typed = "Parakeet TDT", Occurrences = 2, Surface = Surface.Companion };
        Assert.Equal("Dictionary", SuggestionCopy.Header);
        Assert.Equal("2 times this week", SuggestionCopy.CountLabel(s));
        Assert.Equal("You changed “parakeet tee dee tee” → “Parakeet TDT” after dictating.", SuggestionCopy.Sentence(s));
        Assert.Equal("Heard the same way twice (Companion, 7 days). Add it as a replacement?", SuggestionCopy.Detail(s));
        Assert.Equal("Heard the same way 3 times (dictation, 7 days). Add it as a replacement?", SuggestionCopy.Detail(s with { Occurrences = 3, Surface = null }));
        Assert.Equal(TimeSpan.FromSeconds(20), SuggestionCopy.AutoDismiss);
    }

    [Fact]
    public void TheFirstConnectedPreferredMicWinsElseTheDefault()
    {
        var endpoints = new[] { "Microphone Array (Realtek(R) Audio)", "Headset (USB Audio)" };
        Assert.Equal(1, MicPreference.Choose(new[] { "Shure MV7", "headset (usb audio)", "Microphone Array (Realtek(R) Audio)" }, endpoints));
        Assert.Equal(0, MicPreference.Choose(new[] { "Microphone Array (Realtek(R) Audio)" }, endpoints));
        Assert.Null(MicPreference.Choose(new[] { "Shure MV7" }, endpoints));
        Assert.Null(MicPreference.Choose(Array.Empty<string>(), endpoints));
    }
}
