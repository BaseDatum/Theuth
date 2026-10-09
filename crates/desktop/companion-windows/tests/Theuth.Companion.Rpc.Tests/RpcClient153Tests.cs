using System.Text.Json;

namespace Theuth.Companion.Rpc.Tests;

/// <summary>
/// Design 153 W153B (the PIN): every 153 wrapper against the loopback engine — positional params in api.rs order
/// (integrate/153 b7ec00b9), reply decoding, an older engine's -32601, and the two new stream kinds reaching the callback.
/// In the loopback collection: one Kestrel host at a time (149 WW1 R1).
/// </summary>
[Collection("loopback engine")]
public sealed class RpcClient153Tests
{
    private static string Params(JsonElement request) => request.GetProperty("params").GetRawText();

    private static Task<T> B<T>(Task<T> t) => t.WaitAsync(FakeEngine.Bound);

    private static Task B(Task t) => t.WaitAsync(FakeEngine.Bound);

    private const string EntryJson = """{"id":"e1","canonical":"Parakeet TDT","kind":"term","spoken":[],"variants":[{"heard":"parakeet tee dee tee","metaphone":["PRKT","PRKT"],"seen":2,"last_ms":7}],"scope":{"scope":"global"},"source":"learned","tags":[]}""";

    [Fact]
    public async Task Every153CallEncodesItsParamsAndDecodesItsResult()
    {
        await using var engine = await FakeEngine.StartAsync();
        engine.Respond = r => r.GetProperty("method").GetString() switch
        {
            "dictation_start" => FakeEngine.Reply(r, "\"0199c3a2-0000-7000-8000-00000000000d\""),
            "dictation_status" => FakeEngine.Reply(r, """{"model":{"state":"ready"},"active":null,"input_device":"Microphone Array (Realtek)","last":{"surface":"companion","press_to_first_pcm_ms":42,"press_to_first_partial_ms":310,"press_to_first_final_ms":890,"stop_to_final_ms":420,"polish_ms":null,"device":"Microphone Array (Realtek)"}}"""),
            "voice_status" => FakeEngine.Reply(r, """{"model":{"state":"ready"},"accelerator":"","voice":"v","voices":[],"speaking":null,"queued":[],"input_devices":["Shure MV7"]}"""),
            "dictation_history" => FakeEngine.Reply(r, """[{"id":"d1","started_ms":1,"finished_ms":2,"surface":"companion","destination":"agent_prompt","app":null,"raw":"a","shaped":"A.","polished":null,"delivered":"A.","press_ms":1,"first_partial_ms":2,"first_final_ms":3,"final_ms":4,"reason":"done","audio_bytes":0,"has_audio":false}]"""),
            "dictation_audio_get" => FakeEngine.Reply(r, "\"AAAA\""),
            "dictionary_list" => FakeEngine.Reply(r, "[" + EntryJson + "]"),
            "dictionary_put" => FakeEngine.Reply(r, EntryJson),
            "dictionary_delete" => FakeEngine.Reply(r, "true"),
            "list_input_devices" => FakeEngine.Reply(r, """[{"name":"Microphone Array (Realtek)","default":true}]"""),
            "polish_model" => FakeEngine.Reply(r, "null"),
            "set_polish_model" => FakeEngine.Reply(r, """{"id":"openai/gpt-5.6-mini","name":"GPT-5.6 mini","provider":"openai","context":400000,"reasoning":false,"image":false,"efforts":[],"budget_min":null}"""),
            "polish" => FakeEngine.Reply(r, """{"text":"Teh → The.","model":"openai/gpt-5.6-mini","ms":640,"refused":null}"""),
            "rewrite" => FakeEngine.Reply(r, """{"text":null,"model":null,"ms":0,"refused":"no model"}"""),
            _ => FakeEngine.DefaultRespond(r),
        };
        var rec = new Recorder();
        await using var client = new RpcClient(new EngineLocation(engine.Url, "t"), rec.OnState, rec.OnEvent);
        client.Start();
        await rec.WaitOpenCountAsync(1);

        var opts = new DictationOpts { Surface = Surface.Companion, Destination = Destination.AgentPrompt, PressAgoMs = 240 };
        Assert.Equal("0199c3a2-0000-7000-8000-00000000000d", await B(client.DictationStartAsync(16_000, opts)));
        Assert.Equal("0199c3a2-0000-7000-8000-00000000000d", await B(client.DictationStartAsync(16_000, null)));
        var status = await B(client.DictationStatusAsync());
        Assert.Equal(("Microphone Array (Realtek)", (uint?)310u), (status.InputDevice, status.Last!.PressToFirstPartialMs));
        Assert.Equal(new[] { "Shure MV7" }, (await B(client.VoiceStatusAsync())).InputDevices);
        await B(client.DictationCorrectionAsync("d1", "ship it tonight"));
        Assert.Equal("A.", (await B(client.DictationHistoryAsync(20)))[0].Shaped);
        Assert.Single(await B(client.DictationHistoryAsync()));
        await B(client.DictationHistoryForgetAsync("d1"));
        await B(client.DictationHistoryForgetAsync(null));
        Assert.Equal("AAAA", await B(client.DictationAudioGetAsync("d1", 0)));
        Assert.Equal("AAAA", await B(client.DictationAudioGetAsync("d1", null)));
        Assert.Equal("learned", (await B(client.DictionaryListAsync()))[0].Source);
        Assert.Single(await B(client.DictionaryListAsync(Scope.ForSurface(Surface.Companion))));
        var put = await B(client.DictionaryPutAsync(new Entry { Canonical = "Parakeet TDT", Variants = ValueList.Of(new Variant { Heard = "parakeet tee dee tee" }) }));
        Assert.Equal("e1", put.Id);
        Assert.True(await B(client.DictionaryDeleteAsync("e1")));
        await B(client.DictionarySuggestionAnswerAsync("s-1", SuggestionAnswer.NotNow));
        await B(client.DictionarySuggestionAnswerAsync("s-2", SuggestionAnswer.Add));
        Assert.True((await B(client.ListInputDevicesAsync()))[0].Default);
        await B(client.SetInputDevicesAsync(new[] { "Shure MV7", "Microphone Array (Realtek)" }));
        await B(client.SetInputDevicesAsync(Array.Empty<string>()));
        await B(client.SetShapeAsync(Surface.Companion, ShapePreset.Plain));
        await B(client.SetEndPhrasesAsync(new EndPhrases { Send = ValueList.Of("send it", "ship it") }));
        Assert.Null(await B(client.PolishModelAsync()));
        Assert.Equal("GPT-5.6 mini", (await B(client.SetPolishModelAsync("openai/gpt-5.6-mini")))!.Name);
        await B(client.SetPolishModelAsync(null));
        Assert.Equal(640u, (await B(client.PolishAsync("teh text", Destination.Document))).Ms);
        var rw = await B(client.RewriteAsync("shorter", "a long text", new AppProfile { Id = "notepad.exe", Name = "Notepad" }, new[] { ("formal", "A formal text.") }));
        Assert.Equal("no model", rw.Refused);
        await B(client.RewriteAsync("shorter", "x", new AppProfile { Id = "a", Name = "A" }));

        string P(string method, int nth = 0) => Params(engine.RequestsFor(method).ElementAt(nth));
        Assert.Equal("""[16000,{"surface":"companion","destination":{"kind":"agent_prompt"},"commands":[],"press_ago_ms":240}]""", P("dictation_start"));
        Assert.Equal("[16000]", P("dictation_start", 1)); // an engine before 153 takes exactly one param
        Assert.Equal("[]", P("dictation_status"));
        Assert.Equal("[]", P("voice_status"));
        Assert.Equal("""["d1","ship it tonight"]""", P("dictation_correction"));
        Assert.Equal("[20]", P("dictation_history"));
        Assert.Equal("[null]", P("dictation_history", 1));
        Assert.Equal("""["d1"]""", P("dictation_history_forget"));
        Assert.Equal("[null]", P("dictation_history_forget", 1));
        Assert.Equal("""["d1",0]""", P("dictation_audio_get"));
        Assert.Equal("""["d1",null]""", P("dictation_audio_get", 1));
        Assert.Equal("[null]", P("dictionary_list"));
        Assert.Equal("""[{"scope":"surface","surface":"companion"}]""", P("dictionary_list", 1));
        Assert.Equal("""[{"id":"","canonical":"Parakeet TDT","kind":"term","spoken":[],"variants":[{"heard":"parakeet tee dee tee","metaphone":["",""],"seen":0,"last_ms":0}],"scope":{"scope":"global"},"source":"user","tags":[]}]""", P("dictionary_put"));
        Assert.Equal("""["e1"]""", P("dictionary_delete"));
        Assert.Equal("""["s-1","not_now"]""", P("dictionary_suggestion_answer"));
        Assert.Equal("""["s-2","add"]""", P("dictionary_suggestion_answer", 1));
        Assert.Equal("[]", P("list_input_devices"));
        Assert.Equal("""[["Shure MV7","Microphone Array (Realtek)"]]""", P("set_input_devices"));
        Assert.Equal("[[]]", P("set_input_devices", 1));
        Assert.Equal("""["companion","plain"]""", P("set_shape"));
        Assert.Equal("""[{"send":["send it","ship it"],"scratch":["scratch that"],"literal":"literal"}]""", P("set_end_phrases"));
        Assert.Equal("[]", P("polish_model"));
        Assert.Equal("""["openai/gpt-5.6-mini"]""", P("set_polish_model"));
        Assert.Equal("[null]", P("set_polish_model", 1));
        Assert.Equal("""["teh text",{"kind":"document"},null]""", P("polish"));
        Assert.Equal("""["shorter","a long text",{"id":"notepad.exe","name":"Notepad","terminal":false,"polish":false,"learn":false,"commands":[]},[["formal","A formal text."]]]""", P("rewrite"));
        Assert.Equal("""["shorter","x",{"id":"a","name":"A","terminal":false,"polish":false,"learn":false,"commands":[]},null]""", P("rewrite", 1));
    }

    [Fact]
    public async Task AnEngineBefore153AnswersTheNewMethodsWithMethodNotFound()
    {
        await using var engine = await FakeEngine.StartAsync();
        engine.Respond = r => r.GetProperty("method").GetString() is "dictation_correction" or "dictionary_suggestion_answer" or "list_input_devices"
            ? $$$"""{"jsonrpc":"2.0","id":{{{r.GetProperty("id").GetRawText()}}},"error":{"code":-32601,"message":"Method not found"}}"""
            : FakeEngine.DefaultRespond(r);
        var rec = new Recorder();
        await using var client = new RpcClient(new EngineLocation(engine.Url, "t"), rec.OnState, rec.OnEvent);
        client.Start();
        await rec.WaitOpenCountAsync(1);
        Assert.Equal(-32601, (await Assert.ThrowsAsync<RpcException>(() => B(client.DictationCorrectionAsync("d1", "x")))).Code);
        Assert.Equal(-32601, (await Assert.ThrowsAsync<RpcException>(() => B(client.DictionarySuggestionAnswerAsync("s1", SuggestionAnswer.Never)))).Code);
        Assert.Equal(-32601, (await Assert.ThrowsAsync<RpcException>(() => B(client.ListInputDevicesAsync()))).Code);
        // The connection survives: an older call still works.
        Assert.Empty(await B(client.AttentionListAsync()));
    }

    [Fact]
    public async Task TheNewDictationStreamKindsReachTheEventCallbackAndDecodeTyped()
    {
        await using var engine = await FakeEngine.StartAsync();
        engine.AfterReply = async (r, ws) =>
        {
            if (r.GetProperty("method").GetString() != "subscribe_events") return;
            foreach (var item in new[]
            {
                """{"type":"DictationText","id":"d1","utterance":0,"text":"ship it send it","final":false,"shaped":"ship it","command":{"command":"send","near_miss":false},"actions":[]}""",
                """{"type":"DictationDevice","id":"d1","device":"Headset (USB)","state":"changed"}""",
                """{"type":"DictionarySuggestion","id":"s1","heard":"get hub","typed":"GitHub","occurrences":2,"surface":"companion","has_audio":false}""",
            })
            {
                await engine.SendAsync(ws, """{"jsonrpc":"2.0","method":"subscribe_events","params":{"subscription":"sub-1","result":""" + item + "}}");
            }
        };
        var rec = new Recorder();
        await using var client = new RpcClient(new EngineLocation(engine.Url, "t"), rec.OnState, rec.OnEvent);
        client.SubscribeEvents();
        client.Start();
        await rec.WaitAsync(() => rec.EventsSnapshot().Length >= 3);
        var events = rec.EventsSnapshot().Select(e => WireEvent.Decode(e)).ToArray();
        Assert.Equal("ship it", Assert.IsType<DictationEvent>(events[0].Payload).DisplayText);
        Assert.Equal(DeviceState.Changed, Assert.IsType<DictationDevice>(events[1].Payload).State);
        Assert.Equal("GitHub", Assert.IsType<DictionarySuggestion>(events[2].Payload).Typed);
    }
}