using System.Text.Json;

namespace Theuth.Companion.Rpc.Tests;

/// <summary>
/// 153's cross-platform fixtures (the 13 <c>153-*.json</c> files the Mac companion contract wave hand-wrote from the serde attrs,
/// <c>feat/153-cc</c> 557eb083, copied under <c>Fixtures153/</c>): every one decodes with the C# records — one fixture set, two
/// companions. A Swift field the C# record lacks would show up here as a failed equality, not a silent drop.
/// </summary>
public sealed class Fixture153Tests
{
    private static string FixtureDir => Path.Combine(AppContext.BaseDirectory, "Fixtures153");

    private static byte[] Fixture(string name) => File.ReadAllBytes(Path.Combine(FixtureDir, name + ".json"));

    private static T Decode<T>(string name) => TheuthJson.Deserialize<T>(Fixture(name));

    [Fact]
    public void AllThirteenFixturesArePresent()
    {
        var names = Directory.GetFiles(FixtureDir, "153-*.json").Select(Path.GetFileNameWithoutExtension).OrderBy(n => n, StringComparer.Ordinal).ToArray();
        Assert.Equal(new[]
        {
            "153-dictation_events", "153-dictation_history", "153-dictation_opts", "153-dictation_opts_defaults", "153-dictation_status",
            "153-dictation_timings", "153-dictionary_list", "153-end_phrases", "153-input_devices", "153-model_info", "153-polish_reply",
            "153-polish_reply_refused", "153-voice_status",
        }, names);
    }

    [Fact]
    public void DictationEventsDecodeThroughTheStream()
    {
        using var doc = JsonDocument.Parse(Fixture("153-dictation_events"));
        var events = doc.RootElement.EnumerateArray().Select(e => WireEvent.Decode(e.GetRawText())).ToArray();
        Assert.Equal(7, events.Length);
        var t0 = Assert.IsType<DictationEvent>(events[0].Payload);
        Assert.Equal(("hi dana comma send it", "Hi Dana,", "Hi Dana,", false), (t0.Text, t0.Shaped, t0.DisplayText, t0.IsFinal));
        Assert.Equal(EndCommand.Send(false), t0.Command);
        Assert.Empty(t0.Actions!);
        var t1 = Assert.IsType<DictationEvent>(events[1].Payload);
        Assert.True(t1.Command!.IsScratchThat);
        Assert.Equal("", t1.DisplayText);
        Assert.Equal(new ShapeAction?[] { ShapeAction.NewLine }, t1.Actions!.Select(WireWords153.ParseAction).ToArray());
        var t2 = Assert.IsType<DictationEvent>(events[2].Payload);
        Assert.Equal(("plain", (string?)null, (EndCommand?)null, (ValueList<string>?)null), (t2.DisplayText, t2.Shaped, t2.Command, t2.Actions));
        var changed = Assert.IsType<DictationDevice>(events[3].Payload);
        Assert.Equal(("AirPods Pro", (DeviceState?)DeviceState.Changed), (changed.Device, changed.State));
        var lost = Assert.IsType<DictationDevice>(events[4].Payload);
        Assert.Equal(((string?)null, (DeviceState?)DeviceState.Lost), (lost.Device, lost.State));
        var s = Assert.IsType<DictionarySuggestion>(events[5].Payload);
        Assert.Equal(("parakeet tee dee tee", "Parakeet TDT", 2u, (Surface?)Surface.Composer, true), (s.Heard, s.Typed, s.Occurrences, s.Surface, s.HasAudio));
        var done = Assert.IsType<DictationEvent>(events[6].Payload);
        Assert.Equal("done", done.Reason!.Reason);
        // The same items through the one dictation record the Store reduces (Swift's DictationWire carries device/state too).
        var wire = DictationEvent.Decode(System.Text.Encoding.UTF8.GetBytes(doc.RootElement[3].GetRawText()));
        Assert.Equal(("AirPods Pro", (DeviceState?)DeviceState.Changed), (wire.Device, wire.State));
    }

    [Fact]
    public void DictationOptsDecodeAndReEncode()
    {
        var opts = Decode<DictationOpts>("153-dictation_opts");
        Assert.Equal(((Surface?)Surface.AnyApp, "any_app", "com.apple.Terminal", 42u), (opts.Surface, opts.Destination!.Kind, opts.Destination.App, opts.PressAgoMs!.Value));
        Assert.Equal(new AppProfile { Id = "com.apple.Terminal", Name = "Terminal", Terminal = true, Commands = ValueList.Of("/clear") }, opts.App);
        Assert.Equal(new[] { "/clear" }, opts.Commands);
        // Null optionals are omitted when the companion writes it; the engine's serde defaults read the same value.
        Assert.Equal("""{"surface":"any_app","destination":{"kind":"any_app","app":"com.apple.Terminal"},"app":{"id":"com.apple.Terminal","name":"Terminal","terminal":true,"polish":false,"learn":false,"commands":["/clear"]},"commands":["/clear"],"press_ago_ms":42}""",
            TheuthJson.Serialize(opts));
        var defaults = Decode<DictationOpts>("153-dictation_opts_defaults");
        Assert.Equal(new DictationOpts(), defaults);
    }

    [Fact]
    public void StatusAndTimingsDecode()
    {
        var status = Decode<DictationState>("153-dictation_status");
        Assert.Equal(("ready", (string?)null, "MacBook Pro Microphone"), (status.Model!.State, status.Active, status.InputDevice));
        var timings = Decode<DictationTimings>("153-dictation_timings");
        Assert.Equal(timings, status.Last);
        Assert.Equal(((Surface?)Surface.Companion, (uint?)42u, (uint?)310u, (uint?)890u, (uint?)420u, (uint?)null, "MacBook Pro Microphone"),
            (timings.Surface, timings.PressToFirstPcmMs, timings.PressToFirstPartialMs, timings.PressToFirstFinalMs, timings.StopToFinalMs, timings.PolishMs, timings.Device));
        var voice = Decode<VoiceStatus>("153-voice_status");
        Assert.Equal(("failed", "checkpoint missing", "CPUExecutionProvider", "af_heart"), (voice.Model!.State, voice.Model.Message, voice.Accelerator, voice.Voice));
        Assert.Equal(new[] { "AirPods Pro", "MacBook Pro Microphone" }, voice.InputDevices);
        var devices = Decode<ValueList<InputDevice>>("153-input_devices");
        Assert.Equal(new[] { ("AirPods Pro", false), ("MacBook Pro Microphone", true) }, devices.Select(d => (d.Name, d.Default)).ToArray());
    }

    [Fact]
    public void HistoryAndDictionaryDecode()
    {
        var rows = Decode<ValueList<HistoryRow>>("153-dictation_history");
        Assert.Equal(2, rows.Count);
        Assert.Equal(((Surface?)Surface.AnyApp, "any_app:com.apple.Notes", "com.apple.Notes", "Hi Dana, the deploy is done.", 96000ul, true),
            (rows[0].Surface, rows[0].Destination, rows[0].App, rows[0].Polished, rows[0].AudioBytes, rows[0].HasAudio));
        Assert.Equal(((ulong?)null, "error:microphone: no audio from AirPods Pro", (uint?)null), (rows[1].FinishedMs, rows[1].Reason, rows[1].PressMs));
        var entries = Decode<ValueList<Entry>>("153-dictionary_list");
        Assert.Equal(new[] { ("user:Parakeet TDT", EntryKind.Term, EntrySource.User, "global"), ("learned:clear", EntryKind.Command, EntrySource.Learned, "surface"), ("user:rg", EntryKind.Abbrev, EntrySource.User, "app") },
            entries.Select(e => (e.Id, e.Kind, e.Source, e.Scope.Kind)).ToArray());
        Assert.Equal(("tui", "com.apple.Terminal"), (entries[1].Scope.SurfaceWord, entries[2].Scope.App));
        Assert.Equal(new Variant { Heard = "parakeet t d t", Metaphone = ValueList.Of("PRKT", "PRKT"), Seen = 3, LastMs = 1791448800000 }, entries[0].Variants.Single());
        // Round trip: what dictionary_list returns goes back through dictionary_put byte-identical (minified).
        using var doc = JsonDocument.Parse(Fixture("153-dictionary_list"));
        foreach (var (e, raw) in entries.Zip(doc.RootElement.EnumerateArray()))
        {
            Assert.Equal(JsonSerializer.Serialize(raw), TheuthJson.Serialize(e));
        }
    }

    [Fact]
    public void PhrasesModelAndPolishDecode()
    {
        Assert.Equal(new EndPhrases(), Decode<EndPhrases>("153-end_phrases"));
        var m = Decode<ModelInfo>("153-model_info");
        Assert.Equal(("anthropic/claude-haiku", "Claude Haiku", "anthropic"), (m.Id, m.Name, m.Provider));
        var ok = Decode<PolishReply>("153-polish_reply");
        Assert.Equal(("Hi Dana, the deploy is done.", "anthropic/claude-haiku", 820u, (string?)null), (ok.Text, ok.Model, ok.Ms, ok.Refused));
        var refused = Decode<PolishReply>("153-polish_reply_refused");
        Assert.Equal(((string?)null, 3u, "agent_prompt is never polished"), (refused.Text, refused.Ms, refused.Refused));
    }
}
