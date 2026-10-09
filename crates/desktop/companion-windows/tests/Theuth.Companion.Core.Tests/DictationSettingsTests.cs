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

    [Fact]
    public async Task DictionaryLoadingIsRetriedQuietlyAndOtherErrorsAreNot()
    {
        int calls = 0;
        var waits = new List<TimeSpan>();
        var value = await DictionaryRetry.Run(() =>
        {
            calls++;
            return calls < 3 ? Task.FromException<int>(new RpcException(-32000, "dictionary loading")) : Task.FromResult(42);
        }, t => { waits.Add(t); return Task.CompletedTask; });
        Assert.Equal((42, 3, 2), (value, calls, waits.Count));
        await Assert.ThrowsAsync<RpcException>(() => DictionaryRetry.Run(() => Task.FromException<int>(new RpcException(-32000, "unknown suggestion")), _ => Task.CompletedTask));
        int tries = 0;
        await Assert.ThrowsAsync<RpcException>(() => DictionaryRetry.Run(() => { tries++; return Task.FromException(new RpcException(-32000, "dictionary loading")); }, _ => Task.CompletedTask, attempts: 3));
        Assert.Equal(3, tries);
    }

    /// <summary>153 F1: the three captions Settings › Dictation shows from <c>voice_settings()</c> — and the fallbacks for an older engine.</summary>
    [Fact]
    public void VoiceSettingsCaptionsFollowTheEngineAndFallBackWhenItIsOlder()
    {
        Assert.StartsWith("Shown: the engine's presets", DictationSettings.ShapeCaption(fromEngine: true, older: false));
        Assert.Contains("this engine is older", DictationSettings.ShapeCaption(fromEngine: false, older: true));
        Assert.Contains("did not answer", DictationSettings.ShapeCaption(fromEngine: false, older: false));
        Assert.EndsWith("Not known to this companion: Other apps = loud.", DictationSettings.ShapeCaption(true, false, new[] { "Other apps = loud" }));

        Assert.Equal("Polish model: openai/gpt-5.6-mini", DictationSettings.PolishModelLine("openai/gpt-5.6-mini"));
        Assert.Equal("Polish model: Chain (title model, then session)", DictationSettings.PolishModelLine(null));
        Assert.Equal("Polish model: Chain (title model, then session)", DictationSettings.PolishModelLine(" "));

        Assert.Contains("by default the last 200 dictations and up to 64 MiB", DictationSettings.HistoryCaption(null));
        Assert.Contains("the last 50 dictations and up to 16 MiB", DictationSettings.HistoryCaption(new HistoryConfig { Rows = 50, AudioMib = 16 }));
        Assert.Contains("Corrections are learned.", DictationSettings.HistoryCaption(new HistoryConfig()));
        Assert.Contains("Learning from corrections is off.", DictationSettings.HistoryCaption(new HistoryConfig { Learn = false }));
        Assert.StartsWith("History is off", DictationSettings.HistoryCaption(new HistoryConfig { Enabled = false }));

        var f1 = new VoiceSettings { InputDevices = ValueList.Of("Shure MV7") };
        Assert.Equal(new[] { "Shure MV7" }, DictationSettings.MicPreferences(f1, new[] { "stale" }));
        Assert.Empty(DictationSettings.MicPreferences(new VoiceSettings(), new[] { "stale" })); // F1 says "system default only": no stale fallback
        Assert.Equal(new[] { "Headset" }, DictationSettings.MicPreferences(null, new[] { "Headset" }));
        Assert.Empty(DictationSettings.MicPreferences(null, null));
    }
}
