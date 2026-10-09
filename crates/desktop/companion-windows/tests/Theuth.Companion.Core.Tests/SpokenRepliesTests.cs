namespace Theuth.Companion.Core.Tests;

/// <summary>Spoken replies (Sky 2026-10-09 15:58/16:02): opt-in defaults, the one shared speaker, the key routing.</summary>
public sealed class SpokenRepliesTests
{
    [Fact]
    public void BothSpeakSwitchesDefaultOff()
    {
        var d = new SettingsStore();
        Assert.False(PointSettings.Load(d).Speak);
        Assert.False(CompanionSettings.Load(d).AskSpeak);
        Assert.False(new PointSettings().Speak);
        Assert.False(new CompanionSettings().AskSpeak);
    }

    [Fact]
    public void AStoredVoiceKeySpeakCountsOnlyWhenSetExplicitly()
    {
        // An old settings.json where every Voice key edit wrote back voiceKeySpeak:true (the old default) → still off.
        var d = new SettingsStore();
        d.Set("voiceKeySpeak", true);
        Assert.False(PointSettings.Load(d).Speak);
        // The user flipped the switch: honoured both ways.
        var on = new PointSettings { Speak = true, SpeakSet = true };
        on.Save(d);
        Assert.True(d.Bool("voiceKeySpeakSet"));
        Assert.True(PointSettings.Load(d).Speak);
        var off = new PointSettings { Speak = false, SpeakSet = true };
        off.Save(d);
        Assert.False(PointSettings.Load(d).Speak);
        Assert.True(PointSettings.Load(d).SpeakSet);
    }

    [Fact]
    public void AskSpeakRoundTrips()
    {
        var d = new SettingsStore();
        var s = new CompanionSettings { AskSpeak = true };
        Assert.NotEqual(new CompanionSettings(), s);
        s.Save(d);
        Assert.True(d.Bool("askSpeak"));
        Assert.True(CompanionSettings.Load(d).AskSpeak);
        Assert.Contains("askSpeak", CompanionSettings.Key.All);
    }

    [Fact]
    public void TheSwitchesAreIndependentAndGateFinalRepliesOnly()
    {
        Assert.False(SpeakerModel.ShouldSpeak(SpeechSource.VoiceKey, voiceKeySpeak: false, askSpeak: true, "hello"));
        Assert.True(SpeakerModel.ShouldSpeak(SpeechSource.VoiceKey, voiceKeySpeak: true, askSpeak: false, "hello"));
        Assert.True(SpeakerModel.ShouldSpeak(SpeechSource.Ask, voiceKeySpeak: false, askSpeak: true, "hello"));
        Assert.False(SpeakerModel.ShouldSpeak(SpeechSource.Ask, voiceKeySpeak: true, askSpeak: false, "hello"));
        Assert.False(SpeakerModel.ShouldSpeak(SpeechSource.Ask, true, true, "   "));
        Assert.False(SpeakerModel.ShouldSpeak(SpeechSource.VoiceKey, true, true, "hello", replaying: true));
    }

    [Fact]
    public void OneSpeakerAcrossVoiceKeyAndAsk()
    {
        var m = new SpeakerModel();
        Assert.False(m.IsSpeaking);
        Assert.Null(m.Stop(SpeechStop.Esc)); // nothing speaking: a no-op
        var a = m.Begin("req-1", "desk", SpeechSource.VoiceKey);
        m.Named("u-1");
        Assert.Equal("u-1", m.Speaking!.Id);
        var b = m.Begin("req-2", "desk", SpeechSource.Ask); // an Ask reply while the voice key's answer still speaks
        Assert.Equal("u-1", m.Displaced!.Id); // the caller cuts it: never two at once
        Assert.Equal(SpeechSource.Ask, m.Speaking!.Source);
        Assert.NotEqual(a, b);
    }

    [Fact]
    public void EscNewAskAndNewGestureStopTheCurrentSpeech()
    {
        foreach (var why in new[] { SpeechStop.Esc, SpeechStop.NewAsk, SpeechStop.NewGesture, SpeechStop.Glyph })
        {
            var m = new SpeakerModel();
            m.Begin("req", "desk", SpeechSource.Ask);
            m.Named("u-9");
            var was = m.Stop(why);
            Assert.Equal("u-9", was!.Id);
            Assert.False(m.IsSpeaking);
            Assert.Null(m.Displaced);
        }
    }

    [Fact]
    public void FinishedSilencesOnlyTheCurrentUtterance()
    {
        var m = new SpeakerModel();
        m.Begin("req", "desk", SpeechSource.VoiceKey);
        m.Named("u-1");
        Assert.False(m.Finished("u-old")); // a late finish of an earlier utterance changes nothing
        Assert.True(m.IsSpeaking);
        Assert.True(m.Finished("u-1"));
        Assert.False(m.IsSpeaking);
    }

    [Fact]
    public void ThePanelKeyAlwaysOpensThePanelAndTheAskKeyOnlyTheAskBar()
    {
        // Ruling 2: no "panel when something needs you, else the Ask bar" any more.
        foreach (var needs in new[] { 0, 1, 7 })
        {
            Assert.Equal(KeySurface.Panel, KeyRouting.Route(KeyAction.OpenPanel, needs, askBarAvailable: true));
            Assert.Equal(KeySurface.Panel, KeyRouting.Route(KeyAction.OpenPanel, needs, askBarAvailable: false));
            Assert.Equal(KeySurface.AskBar, KeyRouting.Route(KeyAction.OpenAskBar, needs, askBarAvailable: true));
        }
        Assert.Equal(KeySurface.None, KeyRouting.Route(KeyAction.OpenAskBar, 0, askBarAvailable: false));
        Assert.Equal(KeySurface.Agents, KeyRouting.Route(KeyAction.OpenAgents, 0, true));
        Assert.Equal(KeySurface.None, KeyRouting.Route(KeyAction.VoiceKey, 0, true));
    }
}
