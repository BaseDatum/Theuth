using System.Text.Json;

namespace Theuth.Companion.Rpc.Tests;

/// <summary>Design 149 WW1: every new typed call against the loopback engine — positional params in api.rs order, result decoding, the engine's error code.
/// One collection with <see cref="RpcClientTests"/>: two loopback Kestrel hosts starting in parallel hung the test host (blame dump, 2026-10-09).</summary>
[Collection("loopback engine")]
public sealed class RpcClient149Tests
{
    private static string Params(JsonElement request) => request.GetProperty("params").GetRawText();

    [Fact]
    public async Task EveryNewCallEncodesItsParamsAndDecodesItsResult()
    {
        await using var engine = await FakeEngine.StartAsync();
        engine.Respond = r => r.GetProperty("method").GetString() switch
        {
            "list_sessions" => FakeEngine.Reply(r, """[{"id":"s1","created_at":"2026-10-09T00:00:00Z","updated_at":"2026-10-09T00:00:00Z","title":"fix it","model":"m","parent":null,"name":null,"effort":null,"agent":"omp","cwd":"/w","pinned":false,"running":true,"owner":"eng-b","here":false}]"""),
            "list_agents" => FakeEngine.Reply(r, """[{"id":"c1","name":"ww1","parent":"s1","depth":1,"agent":"omp","model":"m","state":{"state":"running"},"last":"building"}]"""),
            "sessions_status" => FakeEngine.Reply(r, """{"engine":"eng-a","here":2,"elsewhere":1,"log_seq":5,"last_snapshot":null}"""),
            "send_input" or "steer" or "desk_open" => FakeEngine.Reply(r, "\"s-9\""),
            "screen_context" => FakeEngine.Reply(r, """{"lines":["[screen display 1 pointer 10,20 app=Code]"],"files":["C:\\Users\\u\\.theuth\\computer\\shot.png"]}"""),
            "put_attachment" => FakeEngine.Reply(r, "\"C:\\\\Users\\\\u\\\\att\\\\a.png\""),
            "meeting_status" => FakeEngine.Reply(r, """{"state":"Recording","meeting":"m1","session":"s1","elapsed_ms":4000,"degraded":[{"SttDown":"System"}],"cursor":null,"topics":[],"approval_waiting":false,"supported":{"kind":"Full"},"accelerator":"cpu"}"""),
            "meeting_live" => FakeEngine.Reply(r, """[{"track":"Mic","t0_ms":0,"t1_ms":500,"text":"hi","request":false}]"""),
            "meeting_briefs" => FakeEngine.Reply(r, """[{"id":"b1","topic":"t1","title":"T","bullets":["x"],"source":"Auto","at_ms":1,"parent":null}]"""),
            "meeting_topics" => FakeEngine.Reply(r, """[{"id":"t1","label":"L","at_ms":1,"brief":"b1"}]"""),
            "meeting_oracle_mode" => FakeEngine.Reply(r, """{"mode":"off","effective":"off","reason":null,"enabled":false,"installed":false,"budget_ms":0,"escalate_budget_ms":0}"""),
            "meeting_permissions" => FakeEngine.Reply(r, """{"microphone":"Granted","audio_capture":"Denied"}"""),
            "brief_get" => FakeEngine.Reply(r, """{"id":"cb1","title":"PR","bullets":[],"aliases":[],"tags":[],"source":"user","created_at":"","updated_at":"","uses":0,"last_used_at":null}"""),
            "brief_search" => FakeEngine.Reply(r, """[]"""),
            "speak" => FakeEngine.Reply(r, "\"sp-1\""),
            _ => FakeEngine.DefaultRespond(r),
        };
        var rec = new Recorder();
        await using var client = new RpcClient(new EngineLocation(engine.Url, "t"), rec.OnState, rec.OnEvent);
        client.Start();
        await rec.WaitOpenCountAsync(1);

        var sessions = await client.ListSessionsAsync();
        Assert.Equal(("s1", true, "eng-b", false, "omp"), (sessions[0].Id, sessions[0].Running, sessions[0].Owner, sessions[0].Here, sessions[0].Agent));
        var agents = await client.ListAgentsAsync("s1");
        Assert.Equal(AgentStateKind.Running, agents[0].State.Kind);
        var status = await client.SessionsStatusAsync();
        Assert.Equal(("eng-a", 2u, 1u), (status.Engine, status.Here, status.Elsewhere));
        await client.AgentPauseAsync("ops", true);
        await client.CancelAsync("s1", CancelScope.Tree);
        await client.CancelAsync("s1", null);
        Assert.Equal("s-9", await client.SendInputAsync(null, "look here", new[] { "C:\\a.png" }, Delivery.NextTurn, "in-1"));
        Assert.Equal("s-9", await client.SendInputAsync("s1", "plain", Array.Empty<string>()));
        Assert.Equal("s-9", await client.SteerAsync("s1", "now", new[] { "C:\\b.png" }));
        var shot = await client.ScreenContextAsync(new ScreenContextReq { Samples = new ValueList<PointerSample> { new(0, 10, 20), new(16, 11, 21) }, Color = "#ffb000", SinceMs = 2500, Accessibility = false });
        Assert.Single(shot.Lines);
        Assert.EndsWith("shot.png", shot.Files[0]);
        Assert.EndsWith("a.png", await client.PutAttachmentAsync("AAAA", "png"));
        var meeting = await client.MeetingStatusAsync();
        Assert.True(meeting.IsLive);
        Assert.Equal(new Degradation("SttDown", "System"), meeting.Degraded[0]);
        Assert.Equal("me", (await client.MeetingLiveAsync("m1"))[0].Who);
        Assert.Equal("b1", (await client.MeetingBriefsAsync("m1"))[0].Id);
        Assert.Equal("L", (await client.MeetingTopicsAsync())[0].Label);
        await client.MeetingCursorAsync("b1");
        await client.MeetingCursorAsync(null);
        await client.MeetingPauseAsync();
        await client.MeetingResumeAsync();
        await client.MeetingStopAsync();
        await client.MeetingTapAsync("t1");
        await client.MeetingDismissAsync("t1");
        await client.MeetingDeeperAsync("b1");
        await client.MeetingRequestAsync("what is 149?");
        await client.MeetingPttAsync(true);
        Assert.False((await client.MeetingOracleModeAsync()).Enabled);
        Assert.Equal("Denied", (await client.MeetingPermissionsAsync()).AudioCapture);
        Assert.Equal("cb1", (await client.BriefGetAsync("cb1")).Id);
        Assert.Empty(await client.BriefSearchAsync("pr", 5));
        Assert.Empty(await client.BriefSearchAsync("pr"));
        Assert.Equal("sp-1", await client.SpeakAsync("s1", "done"));
        await client.StopSpeakingAsync("sp-1");
        await client.StopSpeakingAsync();

        string P(string method, int nth = 0) => Params(engine.RequestsFor(method).ElementAt(nth));
        Assert.Equal("[]", P("list_sessions"));
        Assert.Equal("""["s1"]""", P("list_agents"));
        Assert.Equal("[]", P("sessions_status"));
        Assert.Equal("""["ops",true]""", P("agent_pause"));
        Assert.Equal("""["s1","tree"]""", P("cancel"));
        Assert.Equal("""["s1",null]""", P("cancel", 1));
        Assert.Equal("""[null,"look here",["C:\\a.png"],"next_turn","in-1"]""", P("send_input"));
        Assert.Equal("""["s1","plain",[],null,null]""", P("send_input", 1));
        Assert.Equal("""["s1","now",null,["C:\\b.png"],null]""", P("steer"));
        Assert.Equal("""[{"samples":[{"t_ms":0,"x":10,"y":20,"pen":true},{"t_ms":16,"x":11,"y":21,"pen":true}],"color":"#ffb000","since_ms":2500,"accessibility":false}]""", P("screen_context"));
        Assert.Equal("""["AAAA","png"]""", P("put_attachment"));
        Assert.Equal("[]", P("meeting_status"));
        Assert.Equal("""["m1"]""", P("meeting_live"));
        Assert.Equal("""["m1"]""", P("meeting_briefs"));
        Assert.Equal("[]", P("meeting_topics"));
        Assert.Equal("""["b1"]""", P("meeting_cursor"));
        Assert.Equal("[null]", P("meeting_cursor", 1));
        Assert.Equal("[]", P("meeting_pause"));
        Assert.Equal("[]", P("meeting_resume"));
        Assert.Equal("[]", P("meeting_stop"));
        Assert.Equal("""["t1"]""", P("meeting_tap"));
        Assert.Equal("""["t1"]""", P("meeting_dismiss"));
        Assert.Equal("""["b1"]""", P("meeting_deeper"));
        Assert.Equal("""["what is 149?"]""", P("meeting_request"));
        Assert.Equal("[true]", P("meeting_ptt"));
        Assert.Equal("[]", P("meeting_oracle_mode"));
        Assert.Equal("[]", P("meeting_permissions"));
        Assert.Equal("""["cb1"]""", P("brief_get"));
        Assert.Equal("""["pr",5]""", P("brief_search"));
        Assert.Equal("""["pr",null]""", P("brief_search", 1));
        Assert.Equal("""["s1","done"]""", P("speak"));
        Assert.Equal("""["sp-1"]""", P("stop_speaking"));
        Assert.Equal("[null]", P("stop_speaking", 1));
    }

    [Fact]
    public async Task AnOlderEngineAnswersMethodNotFoundWithItsCode()
    {
        await using var engine = await FakeEngine.StartAsync();
        engine.Respond = r => r.GetProperty("method").GetString() is "sessions_status" or "meeting_status"
            ? $$$"""{"jsonrpc":"2.0","id":{{{r.GetProperty("id").GetRawText()}}},"error":{"code":-32601,"message":"Method not found"}}"""
            : FakeEngine.DefaultRespond(r);
        var rec = new Recorder();
        await using var client = new RpcClient(new EngineLocation(engine.Url, "t"), rec.OnState, rec.OnEvent);
        client.Start();
        await rec.WaitOpenCountAsync(1);
        var t = TimeSpan.FromSeconds(20);
        Assert.Equal(-32601, (await Assert.ThrowsAsync<RpcException>(() => client.SessionsStatusAsync().WaitAsync(t))).Code);
        Assert.Equal(-32601, (await Assert.ThrowsAsync<RpcException>(() => client.MeetingStatusAsync().WaitAsync(t))).Code);
        // A null result where a value is expected is a JsonException, not a crash of the receive loop: the next call still works.
        await Assert.ThrowsAsync<JsonException>(() => client.ListSessionsAsync().WaitAsync(t));
        Assert.Empty(await client.AttentionListAsync().WaitAsync(t));
    }

    [Fact]
    public async Task TheNewStreamKindsReachTheEventCallbackVerbatim()
    {
        await using var engine = await FakeEngine.StartAsync();
        engine.AfterReply = async (r, ws) =>
        {
            if (r.GetProperty("method").GetString() != "subscribe_events") return;
            foreach (var item in new[]
            {
                """{"type":"TopicAdded","meeting":"m1","topic":{"id":"t1","label":"x","at_ms":1}}""",
                """{"type":"DictationLevel","id":"d1","levels":[0.5]}""",
                """{"type":"SubagentSpawned","parent":"p","child":"c","name":"n"}""",
                """{"type":"Unknown149","whatever":1}""",
            })
            {
                await engine.SendAsync(ws, """{"jsonrpc":"2.0","method":"subscribe_events","params":{"subscription":"sub-1","result":""" + item + "}}");
            }
        };
        var rec = new Recorder();
        await using var client = new RpcClient(new EngineLocation(engine.Url, "t"), rec.OnState, rec.OnEvent);
        client.SubscribeEvents();
        client.Start();
        await rec.WaitAsync(() => rec.EventsSnapshot().Length >= 4);
        var types = rec.EventsSnapshot().Select(e => WireEvent.Decode(e)).Select(e => (e.Type, e.Payload?.GetType().Name)).ToArray();
        Assert.Equal(new[] { ("TopicAdded", "MeetingEvent"), ("DictationLevel", "DictationEvent"), ("SubagentSpawned", "SubagentSpawned"), ("Unknown149", (string?)null) }, types);
    }
}