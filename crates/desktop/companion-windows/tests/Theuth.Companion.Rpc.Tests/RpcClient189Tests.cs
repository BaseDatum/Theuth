using System.Text.Json;

namespace Theuth.Companion.Rpc.Tests;

/// <summary>
/// Design 189w: the one transcript read the Ask catch-up makes — <c>load_session(session, null, limit)</c> (api.rs:552-558), the
/// <c>Loaded</c> reply's history and <c>running</c>, and <c>TurnStarted.turn_seq</c>, against the loopback engine.
/// </summary>
[Collection("loopback engine")]
public sealed class RpcClient189Tests
{
    private static Task<T> B<T>(Task<T> t) => t.WaitAsync(FakeEngine.Bound);

    // A Loaded as the engine serialises it (crates/agent/src/lib.rs:1264): the history is the rendered transcript, tagged by role; a
    // tool card has no text; the companion reads role, text and turn and ignores the rest (context, peer, vision, files, usage …).
    private const string LoadedJson = """{"history":[{"role":"user","text":"kill the system settings window","turn":41,"context":null,"peer":null},{"role":"reasoning","text":"it is open","turn":41,"context":null,"thinking_ms":120},{"role":"tool","call_id":"c1","name":"applescript","args":{"script":"quit"},"result":["ok",false],"files":[],"context":null},{"role":"assistant","text":"✓ System Settings closed.","turn":41,"context":null},{"role":"user","text":"a steer","turn":null},{"role":"error","text":"model: boom","context":null},{"role":"interrupted","text":"partial"}],"running":false,"model":"anthropic/claude-sonnet-4-5","effort":null,"queued":[],"undone":0,"agent":null,"harness_commands":[],"name":null,"parent":null,"skills":[],"first_seq":40,"more":true,"context_revision":"r1","context_editable":true,"settled":false,"todos":null,"communication":null}""";

    [Fact]
    public async Task LoadSessionSendsTheSessionNullAndTheLimitAndDecodesTheTail()
    {
        await using var engine = await FakeEngine.StartAsync();
        engine.Respond = r => r.GetProperty("method").GetString() == "load_session" ? FakeEngine.Reply(r, LoadedJson) : FakeEngine.DefaultRespond(r);
        var rec = new Recorder();
        await using var client = new RpcClient(new EngineLocation(engine.Url, "t"), rec.OnState, rec.OnEvent);
        client.Start();
        await rec.WaitOpenCountAsync(1);

        var tail = await B(client.LoadSessionAsync("desk-1", 8));

        Assert.Equal("""["desk-1",null,8]""", engine.RequestsFor("load_session").Single().GetProperty("params").GetRawText());
        Assert.False(tail.Running);
        Assert.Equal(new[] { "user", "reasoning", "tool", "assistant", "user", "error", "interrupted" }, tail.History.Select(h => h.Role).ToArray());
        Assert.Equal(new long?[] { 41, 41, null, 41, null, null, null }, tail.History.Select(h => h.Turn).ToArray());
        Assert.Equal("✓ System Settings closed.", tail.History[3].Text);
        Assert.Null(tail.History[2].Text); // a tool card has none
        Assert.Equal("model: boom", tail.History[5].Text);
    }

    [Fact]
    public async Task ARunningSessionAndAnEmptyHistoryDecode()
    {
        await using var engine = await FakeEngine.StartAsync();
        engine.Respond = r => r.GetProperty("method").GetString() == "load_session"
            ? FakeEngine.Reply(r, """{"history":[],"running":true,"model":"m","effort":null,"queued":[],"undone":0,"agent":null,"name":null,"parent":null,"skills":[],"first_seq":null,"more":false}""")
            : FakeEngine.DefaultRespond(r);
        var rec = new Recorder();
        await using var client = new RpcClient(new EngineLocation(engine.Url, "t"), rec.OnState, rec.OnEvent);
        client.Start();
        await rec.WaitOpenCountAsync(1);
        var tail = await B(client.LoadSessionAsync("desk-1", 8));
        Assert.True(tail.Running);
        Assert.Empty(tail.History);
    }

    [Fact]
    public void TurnStartedCarriesTheSeqOfTheUserRowThatOpenedTheTurn()
    {
        var started = StreamDelta.Decode("""{"type":"TurnStarted","session":"desk-1","turn":"0199c3a2-0000-7000-8000-00000000000d","model":"anthropic/claude-sonnet-4-5","turn_seq":41}"""u8);
        Assert.Equal(("TurnStarted", "desk-1", 41L), (started.Type, started.Session, started.TurnSeq));
        Assert.Null(StreamDelta.Decode("""{"type":"TextDelta","session":"desk-1","turn":"t","text":"hi"}"""u8).TurnSeq);
    }

    [Fact]
    public async Task AnEngineWithoutLoadSessionAnswersMinus32601AndTheCallerSeesIt()
    {
        await using var engine = await FakeEngine.StartAsync();
        engine.Respond = r => r.GetProperty("method").GetString() == "load_session"
            ? $$$"""{"jsonrpc":"2.0","id":{{{r.GetProperty("id").GetRawText()}}},"error":{"code":-32601,"message":"Method not found"}}"""
            : FakeEngine.DefaultRespond(r);
        var rec = new Recorder();
        await using var client = new RpcClient(new EngineLocation(engine.Url, "t"), rec.OnState, rec.OnEvent);
        client.Start();
        await rec.WaitOpenCountAsync(1);
        var e = await Assert.ThrowsAsync<RpcException>(() => B(client.LoadSessionAsync("desk-1", 8)));
        Assert.Equal(-32601, e.Code);
    }
}