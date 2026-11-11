using System.Text.Json;

namespace Theuth.Companion.Rpc.Tests;

/// <summary>
/// Design 192w: the one picture call — <c>fence_snapshot(lang, source, width, appearance, scale)</c> (the engine contract: positional
/// params, an <c>outcome</c> / <c>png</c> / <c>width</c> / <c>height</c> / <c>error</c> result, <c>-32601</c> from an engine without it)
/// and the <c>ReasoningDelta</c> / <c>ReasoningDone</c> shapes the thinking section reads, against the loopback engine.
/// </summary>
[Collection("loopback engine")]
public sealed class RpcClient192Tests
{
    private static Task<T> B<T>(Task<T> t) => t.WaitAsync(FakeEngine.Bound);

    private const string TinyPng = "iVBORw0KGgoAAAANSUhEUgAAAAEAAAABCAYAAAAfFcSJAAAADUlEQVR42mNkYPhfDwAChwGA60e6kgAAAABJRU5ErkJggg==";

    [Fact]
    public async Task FenceSnapshotSendsFivePositionalParamsAndDecodesADoneAnswer()
    {
        await using var engine = await FakeEngine.StartAsync();
        engine.Respond = r => r.GetProperty("method").GetString() == "fence_snapshot"
            ? FakeEngine.Reply(r, $$"""{"outcome":"done","png":"{{TinyPng}}","width":557,"height":190,"error":null}""")
            : FakeEngine.DefaultRespond(r);
        var rec = new Recorder();
        await using var client = new RpcClient(new EngineLocation(engine.Url, "t"), rec.OnState, rec.OnEvent);
        client.Start();
        await rec.WaitOpenCountAsync(1);

        var snap = await B(client.FenceSnapshotAsync("mermaid", "graph LR\n  A --> B", 446, "dark", 1.25));

        var p = engine.RequestsFor("fence_snapshot").Single().GetProperty("params");
        Assert.Equal(5, p.GetArrayLength());
        Assert.Equal(("mermaid", "graph LR\n  A --> B", 446, "dark", 1.25), (p[0].GetString(), p[1].GetString(), p[2].GetInt32(), p[3].GetString(), p[4].GetDouble()));
        Assert.Equal(JsonValueKind.Number, p[2].ValueKind);
        Assert.Equal(("done", TinyPng, 557.0, 190.0, null), (snap.Outcome, snap.Png, snap.Width, snap.Height, snap.Error));
    }

    [Fact]
    public async Task AFailedAnswerCarriesItsOutcomeAndErrorAndNoPicture()
    {
        await using var engine = await FakeEngine.StartAsync();
        engine.Respond = r => r.GetProperty("method").GetString() == "fence_snapshot"
            ? FakeEngine.Reply(r, """{"outcome":"failed","png":null,"width":null,"height":null,"error":"mermaid: parse error"}""")
            : FakeEngine.DefaultRespond(r);
        var rec = new Recorder();
        await using var client = new RpcClient(new EngineLocation(engine.Url, "t"), rec.OnState, rec.OnEvent);
        client.Start();
        await rec.WaitOpenCountAsync(1);
        var snap = await B(client.FenceSnapshotAsync("mermaid", "oops", 400, "light", 1));
        Assert.Equal(("failed", null, null, null, "mermaid: parse error"), (snap.Outcome, snap.Png, snap.Width, snap.Height, snap.Error));
        Assert.Equal("light", engine.RequestsFor("fence_snapshot").Single().GetProperty("params")[3].GetString());
    }

    [Fact]
    public async Task AnEngineWithoutFenceSnapshotAnswersMinus32601AndTheCallerSeesIt()
    {
        await using var engine = await FakeEngine.StartAsync();
        engine.Respond = r => r.GetProperty("method").GetString() == "fence_snapshot"
            ? $$$"""{"jsonrpc":"2.0","id":{{{r.GetProperty("id").GetRawText()}}},"error":{"code":-32601,"message":"Method not found"}}"""
            : FakeEngine.DefaultRespond(r);
        var rec = new Recorder();
        await using var client = new RpcClient(new EngineLocation(engine.Url, "t"), rec.OnState, rec.OnEvent);
        client.Start();
        await rec.WaitOpenCountAsync(1);
        var e = await Assert.ThrowsAsync<RpcException>(() => B(client.FenceSnapshotAsync("openui", "root = Stack([])", 400, "dark", 1)));
        Assert.Equal(-32601, e.Code);
    }

    [Fact]
    public void ReasoningDeltaCarriesTheTextAndReasoningDoneTheMilliseconds()
    {
        var delta = StreamDelta.Decode("""{"type":"ReasoningDelta","session":"desk-1","turn":"0199c3a2-0000-7000-8000-00000000000d","text":"Let me look at the log."}"""u8);
        Assert.Equal(("ReasoningDelta", "desk-1", "Let me look at the log.", null), (delta.Type, delta.Session, delta.Text, delta.Ms));
        var done = StreamDelta.Decode("""{"type":"ReasoningDone","session":"desk-1","turn":"0199c3a2-0000-7000-8000-00000000000d","ms":3200}"""u8);
        Assert.Equal(("ReasoningDone", "desk-1", 3200L), (done.Type, done.Session, done.Ms));
        Assert.Null(StreamDelta.Decode("""{"type":"TextDelta","session":"desk-1","turn":"t","text":"hi"}"""u8).Ms);
    }
}
