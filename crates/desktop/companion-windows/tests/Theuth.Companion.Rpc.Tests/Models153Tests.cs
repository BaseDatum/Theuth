using System.Text.Json;

namespace Theuth.Companion.Rpc.Tests;

/// <summary>
/// Design 153 W153B (the PIN): hand-written JSON fixtures for every new record and event shape (153 §14.1/§14.4), checked
/// against the wave-A Rust types (integrate/153 b7ec00b9), including old-engine payloads without the new fields and
/// newer-engine words this build does not know (they read as null / round-trip, never throw).
/// </summary>
public sealed class Models153Tests
{
    // MARK: DictationText + shaped / command / actions (§14.4 row 2)

    [Fact]
    public void DictationTextCarriesShapedCommandAndActions()
    {
        var e = WireEvent.Decode("""{"type":"DictationText","id":"d1","utterance":0,"text":"ship it tonight send it","final":false,"shaped":"ship it tonight","command":{"command":"send","near_miss":false},"actions":[]}""");
        var d = Assert.IsType<DictationEvent>(e.Payload);
        Assert.Equal(("ship it tonight send it", "ship it tonight", false), (d.Text, d.Shaped, d.IsFinal));
        Assert.True(d.Command!.IsSend);
        Assert.False(d.Command.NearMiss);
        Assert.Empty(d.Actions!);
        Assert.Equal("ship it tonight", d.DisplayText);
    }

    [Fact]
    public void AnOldEngineDictationTextHasNoNewFieldsAndShowsTheRawText()
    {
        var d = DictationEvent.Decode("""{"type":"DictationText","id":"d1","utterance":2,"text":"hello there","final":true}"""u8);
        Assert.Null(d.Shaped);
        Assert.Null(d.Command);
        Assert.Null(d.Actions);
        Assert.Equal("hello there", d.DisplayText);
    }

    [Fact]
    public void AnEmptyShapedWinsOverTheRawText()
    {
        // "send it" alone shapes to "" (the phrase is removed) and "scratch that" re-sends the previous utterance empty.
        var sendOnly = DictationEvent.Decode("""{"type":"DictationText","id":"d1","utterance":1,"text":"send it","final":true,"shaped":"","command":{"command":"send","near_miss":false},"actions":[]}"""u8);
        Assert.Equal("", sendOnly.DisplayText);
        var scratched = DictationEvent.Decode("""{"type":"DictationText","id":"d1","utterance":0,"text":"","final":true,"shaped":"","command":null,"actions":[]}"""u8);
        Assert.Equal("", scratched.DisplayText);
        Assert.Null(scratched.Command);
        // H3-B1's rule: an empty shaped without a command falls back to the raw text.
        var bare = DictationEvent.Decode("""{"type":"DictationText","id":"d1","utterance":3,"text":".","final":true,"shaped":"","command":null,"actions":[]}"""u8);
        Assert.Equal(".", bare.DisplayText);
    }

    [Fact]
    public void EndCommandVariantsDecode()
    {
        var near = TheuthJson.Deserialize<EndCommand>("""{"command":"send","near_miss":true}""");
        Assert.True(near.IsSend);
        Assert.True(near.NearMiss);
        var scratch = TheuthJson.Deserialize<EndCommand>("""{"command":"scratch_that"}""");
        Assert.True(scratch.IsScratchThat);
        Assert.False(scratch.IsSend);
        var future = TheuthJson.Deserialize<EndCommand>("""{"command":"undo_that","extra":1}""");
        Assert.Equal("undo_that", future.Command);
        Assert.False(future.IsSend || future.IsScratchThat);
        Assert.Equal(EndCommand.Send(true), near);
        Assert.Equal(EndCommand.ScratchThat, scratch);
    }

    [Fact]
    public void ActionsStayWordsAndParseTolerantly()
    {
        var d = DictationEvent.Decode("""{"type":"DictationText","id":"d1","utterance":0,"text":"a new line b tab","final":true,"shaped":"a\nB\t","actions":["new_line","tab","new_paragraph","next_year"]}"""u8);
        Assert.Equal(new ShapeAction?[] { ShapeAction.NewLine, ShapeAction.Tab, ShapeAction.NewParagraph, null }, d.Actions!.Select(WireWords153.ParseAction).ToArray());
        Assert.Equal("a\nB\t", d.DisplayText);
    }

    // MARK: the two new events

    [Fact]
    public void DictationDeviceDecodesEveryState()
    {
        var changed = Assert.IsType<DictationDevice>(WireEvent.Decode("""{"type":"DictationDevice","id":"d1","device":"Microphone Array (Realtek)","state":"changed"}""").Payload);
        Assert.Equal(("d1", "Microphone Array (Realtek)", (DeviceState?)DeviceState.Changed), (changed.Id, changed.Device, changed.State));
        var lost = DictationDevice.Decode("""{"type":"DictationDevice","id":"d1","device":null,"state":"lost"}"""u8);
        Assert.Equal(((string?)null, (DeviceState?)DeviceState.Lost), (lost.Device, lost.State));
        var future = DictationDevice.Decode("""{"type":"DictationDevice","id":"d1","state":"rebooting","why":{"x":1}}"""u8);
        Assert.Null(future.State);
        Assert.Null(future.Device);
    }

    [Fact]
    public void DictionarySuggestionDecodesAndToleratesANewSurface()
    {
        var s = Assert.IsType<DictionarySuggestion>(WireEvent.Decode("""{"type":"DictionarySuggestion","id":"0199c3a2-7f00-7000-8000-000000000001","heard":"parakeet tee dee tee","typed":"Parakeet TDT","occurrences":2,"surface":"companion","has_audio":true}""").Payload);
        Assert.Equal(("0199c3a2-7f00-7000-8000-000000000001", "parakeet tee dee tee", "Parakeet TDT", 2u, (Surface?)Surface.Companion, true),
            (s.Id, s.Heard, s.Typed, s.Occurrences, s.Surface, s.HasAudio));
        var future = DictionarySuggestion.Decode("""{"type":"DictionarySuggestion","id":"s2","heard":"a","typed":"b","occurrences":3,"surface":"watch","has_audio":false}"""u8);
        Assert.Null(future.Surface);
        Assert.Equal(3u, future.Occurrences);
    }

    [Fact]
    public void TheDictationEventKindsStillRouteToDictationEvent()
    {
        foreach (var t in new[] { "DictationStarted", "DictationLevel", "DictationText", "DictationFinished" })
        {
            Assert.IsType<DictationEvent>(WireEvent.Decode($$"""{"type":"{{t}}","id":"d1"}""").Payload);
        }
    }

    // MARK: what the companion sends (DictationOpts, Destination, AppProfile)

    [Fact]
    public void DictationOptsForACompanionFieldEncodeSnakeCase()
    {
        var opts = new DictationOpts { Surface = Surface.Companion, Destination = Destination.AgentPrompt, PressAgoMs = 240 };
        Assert.Equal("""{"surface":"companion","destination":{"kind":"agent_prompt"},"commands":[],"press_ago_ms":240}""", TheuthJson.Serialize(opts));
        Assert.Equal("""{"commands":[]}""", TheuthJson.Serialize(new DictationOpts()));
        var anyApp = new DictationOpts
        {
            Surface = Surface.AnyApp,
            Destination = Destination.AnyApp("WindowsTerminal.exe"),
            App = new AppProfile { Id = "WindowsTerminal.exe", Name = "Terminal", Terminal = true, Commands = ValueList.Of("/compact") },
            Commands = ValueList.Of("/compact"),
        };
        Assert.Equal("""{"surface":"any_app","destination":{"kind":"any_app","app":"WindowsTerminal.exe"},"app":{"id":"WindowsTerminal.exe","name":"Terminal","terminal":true,"polish":false,"learn":false,"commands":["/compact"]},"commands":["/compact"]}""",
            TheuthJson.Serialize(anyApp));
    }

    [Fact]
    public void DestinationsEncodeTheirKindTag()
    {
        Assert.Equal("""{"kind":"agent_prompt"}""", TheuthJson.Serialize(Destination.AgentPrompt));
        Assert.Equal("""{"kind":"document"}""", TheuthJson.Serialize(Destination.Document));
        Assert.Equal("""{"kind":"message_reply"}""", TheuthJson.Serialize(Destination.MessageReply));
        Assert.Equal("""{"kind":"any_app","app":"Code.exe"}""", TheuthJson.Serialize(Destination.AnyApp("Code.exe")));
    }

    [Fact]
    public void AppProfileAlwaysWritesItsRequiredFieldsAndRoundTrips()
    {
        var p = new AppProfile { Id = "notepad.exe", Name = "Notepad", PolishPrompt = "plain prose" };
        var json = TheuthJson.Serialize(p);
        Assert.Equal("""{"id":"notepad.exe","name":"Notepad","terminal":false,"polish":false,"learn":false,"polish_prompt":"plain prose","commands":[]}""", json);
        Assert.Equal(p, TheuthJson.Deserialize<AppProfile>(json));
        Assert.Empty(TheuthJson.Deserialize<AppProfile>("""{"id":"a","name":"A","terminal":false,"polish":false,"learn":false,"commands":null}""").Commands);
    }

    [Fact]
    public void WireWordsAreSpelledOut()
    {
        Assert.Equal(new[] { "composer", "tui", "companion", "phone", "any_app" }, Enum.GetValues<Surface>().Select(s => s.Word()).ToArray());
        Assert.Equal(new[] { "full", "plain", "off" }, Enum.GetValues<ShapePreset>().Select(p => p.Word()).ToArray());
        Assert.Equal(new[] { "add", "not_now", "never" }, Enum.GetValues<SuggestionAnswer>().Select(a => a.Word()).ToArray());
        // The shared converter agrees with the spelled-out words (DictationOpts.Surface goes through it).
        foreach (var s in Enum.GetValues<Surface>()) Assert.Equal($"\"{s.Word()}\"", TheuthJson.Serialize(s));
        foreach (var a in Enum.GetValues<SuggestionAnswer>()) Assert.Equal($"\"{a.Word()}\"", TheuthJson.Serialize(a));
        Assert.Equal(Surface.AnyApp, WireWords153.ParseSurface("any_app"));
        Assert.Null(WireWords153.ParseSurface("Companion"));
        Assert.Null(WireWords153.ParseSurface(null));
    }

    // MARK: replies (dictation_status, voice_status, list_input_devices, history, polish)

    [Fact]
    public void DictationStateDecodesNewAndOldShapes()
    {
        var now = TheuthJson.Deserialize<DictationState>("""{"model":{"state":"ready"},"active":"d1","input_device":"Microphone Array (Realtek)","last":{"surface":"companion","press_to_first_pcm_ms":42,"press_to_first_partial_ms":310,"press_to_first_final_ms":890,"stop_to_final_ms":420,"polish_ms":null,"device":"Microphone Array (Realtek)"}}""");
        Assert.Equal(("ready", "d1", "Microphone Array (Realtek)"), (now.Model!.State, now.Active, now.InputDevice));
        Assert.Equal(((Surface?)Surface.Companion, (uint?)42u, (uint?)310u, (uint?)890u, (uint?)420u, (uint?)null), (now.Last!.Surface, now.Last.PressToFirstPcmMs, now.Last.PressToFirstPartialMs, now.Last.PressToFirstFinalMs, now.Last.StopToFinalMs, now.Last.PolishMs));
        var old = TheuthJson.Deserialize<DictationState>("""{"model":{"state":"failed","message":"no model at C:\\m"},"active":null}""");
        Assert.Equal(("failed", "no model at C:\\m"), (old.Model!.State, old.Model.Message));
        Assert.Null(old.InputDevice);
        Assert.Null(old.Last);
    }

    [Fact]
    public void VoiceStatusInputDevicesAreOptional()
    {
        var old = TheuthJson.Deserialize<VoiceStatus>("""{"model":{"state":"ready"},"accelerator":"CPUExecutionProvider","voice":"peter_yearsley","voices":["a"],"speaking":null,"queued":[]}""");
        Assert.Empty(old.InputDevices);
        var now = TheuthJson.Deserialize<VoiceStatus>("""{"model":{"state":"loading"},"voice":"v","voices":[],"speaking":null,"input_devices":["Shure MV7","Microphone Array (Realtek)"]}""");
        Assert.Equal(new[] { "Shure MV7", "Microphone Array (Realtek)" }, now.InputDevices);
    }

    [Fact]
    public void InputDevicesDecode()
    {
        var list = TheuthJson.Deserialize<ValueList<InputDevice>>("""[{"name":"Microphone Array (Realtek)","default":true},{"name":"Headset (USB)","default":false,"host":"wasapi"}]""");
        Assert.Equal(new[] { ("Microphone Array (Realtek)", true), ("Headset (USB)", false) }, list.Select(d => (d.Name, d.Default)).ToArray());
    }

    [Fact]
    public void HistoryRowsDecode()
    {
        var rows = TheuthJson.Deserialize<ValueList<HistoryRow>>("""
            [{"id":"0199c3a2-0000-7000-8000-00000000000a","started_ms":1760033000000,"finished_ms":1760033004000,"surface":"companion","destination":"agent_prompt","app":null,
              "raw":"ship it tonight send it","shaped":"ship it tonight","polished":null,"delivered":"ship it tonight","press_ms":42,"first_partial_ms":310,
              "first_final_ms":890,"final_ms":1300,"reason":"done","audio_bytes":128000,"has_audio":true},
             {"id":"b","started_ms":1,"finished_ms":null,"surface":"smartwatch","destination":"any_app:notepad.exe","app":"notepad.exe","raw":"x","shaped":"x",
              "polished":"X.","delivered":null,"press_ms":null,"first_partial_ms":null,"first_final_ms":null,"final_ms":null,"reason":"error:microphone: device lost","audio_bytes":0,"has_audio":false}]
            """);
        Assert.Equal(("ship it tonight", "ship it tonight", (Surface?)Surface.Companion, true, 128000ul), (rows[0].Shaped, rows[0].Delivered, rows[0].Surface, rows[0].HasAudio, rows[0].AudioBytes));
        Assert.Equal((uint?)1300u, rows[0].FinalMs);
        Assert.Null(rows[1].Surface);
        Assert.Null(rows[1].FinishedMs);
        Assert.Equal(("any_app:notepad.exe", "X.", "error:microphone: device lost"), (rows[1].Destination, rows[1].Polished, rows[1].Reason));
    }

    [Fact]
    public void PolishReplyAndModelInfoDecode()
    {
        var refused = TheuthJson.Deserialize<PolishReply>("""{"text":null,"model":null,"ms":0,"refused":"agent prompts are never polished"}""");
        Assert.Equal(((string?)null, "agent prompts are never polished"), (refused.Text, refused.Refused));
        var ok = TheuthJson.Deserialize<PolishReply>("""{"text":"Ship it tonight.","model":"openai/gpt-5.6-mini","ms":812,"refused":null}""");
        Assert.Equal(("Ship it tonight.", "openai/gpt-5.6-mini", 812u), (ok.Text, ok.Model, ok.Ms));
        var m = TheuthJson.Deserialize<ModelInfo>("""{"id":"openai/gpt-5.6-mini","name":"GPT-5.6 mini","provider":"openai","context":400000,"reasoning":true,"image":true,"efforts":["low"],"budget_min":null}""");
        Assert.Equal(("openai/gpt-5.6-mini", "GPT-5.6 mini", "openai"), (m.Id, m.Name, m.Provider));
    }

    // MARK: dictionary round trip (Entry, Scope, Variant)

    [Fact]
    public void EntriesRoundTripVerbatimIncludingUnknownWords()
    {
        const string learned = """{"id":"e1","canonical":"Parakeet TDT","kind":"term","spoken":[],"variants":[{"heard":"parakeet tee dee tee","metaphone":["PRKT","PRKT"],"seen":2,"last_ms":1760033000000}],"scope":{"scope":"surface","surface":"companion"},"source":"learned","tags":[]}""";
        Assert.Equal(learned, TheuthJson.Serialize(TheuthJson.Deserialize<Entry>(learned)));
        const string future = """{"id":"e2","canonical":"k8s","kind":"acronym","spoken":["kates"],"variants":[],"scope":{"scope":"app","app":"Code.exe"},"source":"synced","tags":["keep_case"]}""";
        var f = TheuthJson.Deserialize<Entry>(future);
        Assert.Equal(("acronym", "synced", "app", "Code.exe"), (f.Kind, f.Source, f.Scope.Kind, f.Scope.App));
        Assert.Equal(future, TheuthJson.Serialize(f));
    }

    [Fact]
    public void ANewUserEntryWritesEveryField()
    {
        var e = new Entry { Canonical = "GitHub", Spoken = ValueList.Of("git hub"), Variants = ValueList.Of(new Variant { Heard = "get hub" }) };
        Assert.Equal("""{"id":"","canonical":"GitHub","kind":"term","spoken":["git hub"],"variants":[{"heard":"get hub","metaphone":["",""],"seen":0,"last_ms":0}],"scope":{"scope":"global"},"source":"user","tags":[]}""",
            TheuthJson.Serialize(e));
        Assert.Equal("""{"scope":"surface","surface":"any_app"}""", TheuthJson.Serialize(Scope.ForSurface(Surface.AnyApp)));
        Assert.Equal("""{"scope":"app","app":"slack.exe"}""", TheuthJson.Serialize(Scope.ForApp("slack.exe")));
    }

    [Fact]
    public void EndPhrasesDefaultLikeTheEngine()
    {
        Assert.Equal("""{"send":["send it"],"scratch":["scratch that"],"literal":"literal"}""", TheuthJson.Serialize(new EndPhrases()));
        Assert.Equal(new EndPhrases(), TheuthJson.Deserialize<EndPhrases>("{}"));
        var custom = TheuthJson.Deserialize<EndPhrases>("""{"send":[],"scratch":null}""");
        Assert.Empty(custom.Send);
        Assert.Empty(custom.Scratch);
        Assert.Equal("literal", custom.Literal);
    }
}