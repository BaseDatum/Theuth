using System.Collections.Concurrent;
using System.Net;
using System.Net.WebSockets;
using System.Text;
using System.Text.Json;
using Microsoft.AspNetCore.Builder;
using Microsoft.AspNetCore.Hosting;
using Microsoft.AspNetCore.Hosting.Server;
using Microsoft.AspNetCore.Hosting.Server.Features;
using Microsoft.AspNetCore.Http;
using Microsoft.Extensions.DependencyInjection;
using Microsoft.Extensions.Logging;

namespace Theuth.Companion.Rpc.Tests;

/// <summary>
/// A loopback "engine": Kestrel accepting the companion's WebSocket upgrade, recording headers and requests, answering
/// through <see cref="Respond"/> (default: <c>subscribe_events</c> → "sub-1", <c>attention_list</c> → [], else null).
/// </summary>
internal sealed class FakeEngine : IAsyncDisposable
{
    private readonly WebApplication app;
    private WebSocket? current;
    private readonly SemaphoreSlim sendLock = new(1, 1);

    public Uri Url { get; }
    public ConcurrentQueue<string> Requests { get; } = new();
    public ConcurrentQueue<(string? Authorization, string? Client)> Upgrades { get; } = new();
    public int Connections;

    /// <summary>Request → reply text (null: no reply); runs on the server's receive loop.</summary>
    public Func<JsonElement, string?> Respond { get; set; }

    /// <summary>Called right after a reply went out (same loop), e.g. to push a notification behind it.</summary>
    public Func<JsonElement, WebSocket, Task>? AfterReply { get; set; }

    private FakeEngine(WebApplication app, Uri url)
    {
        this.app = app;
        Url = url;
        Respond = DefaultRespond;
    }

    public static string Reply(JsonElement request, string resultJson) =>
        $$"""{"jsonrpc":"2.0","id":{{request.GetProperty("id").GetRawText()}},"result":{{resultJson}}}""";

    public static string? DefaultRespond(JsonElement request) => request.GetProperty("method").GetString() switch
    {
        "subscribe_events" => Reply(request, "\"sub-1\""),
        "attention_list" => Reply(request, "[]"),
        _ => Reply(request, "null"),
    };

    public static async Task<FakeEngine> StartAsync()
    {
        // The EMPTY builder, on purpose (149 WW1 R1): CreateSlimBuilder adds appsettings*.json with reloadOnChange, whose
        // PhysicalFileProvider change token can fire forever on macOS under load — the test host then spins inside
        // FileConfigurationProvider..ctor → ChangeToken.OnChange (hang dump 2026-10-09 03:22). No file configuration,
        // no watcher: Kestrel core on a loopback port and nothing else.
        var builder = WebApplication.CreateEmptyBuilder(new WebApplicationOptions());
        builder.Logging.ClearProviders();
        builder.WebHost.UseKestrelCore();
        builder.WebHost.ConfigureKestrel(o => o.Listen(IPAddress.Loopback, 0));
        var app = builder.Build();
        FakeEngine? engine = null;
        app.UseWebSockets();
        app.Run(async ctx =>
        {
            if (!ctx.WebSockets.IsWebSocketRequest) { ctx.Response.StatusCode = 400; return; }
            engine!.Upgrades.Enqueue((ctx.Request.Headers.Authorization.ToString(), ctx.Request.Headers["X-Theuth-Client"].ToString()));
            using var ws = await ctx.WebSockets.AcceptWebSocketAsync();
            Interlocked.Increment(ref engine.Connections);
            engine.current = ws;
            await engine.ServeAsync(ws, ctx.RequestAborted);
        });
        await app.StartAsync().WaitAsync(Bound);
        var address = app.Services.GetRequiredService<IServer>().Features.Get<IServerAddressesFeature>()!.Addresses.First();
        engine = new FakeEngine(app, new Uri(address.Replace("http://", "ws://") + "/"));
        return engine;
    }

    /// <summary>Every await in these tests is bounded (R1): a regression fails in 15 s instead of hanging the test host.</summary>
    public static readonly TimeSpan Bound = TimeSpan.FromSeconds(15);
    private async Task ServeAsync(WebSocket ws, CancellationToken ct)
    {
        var buffer = new byte[64 * 1024];
        using var message = new MemoryStream();
        try
        {
            while (ws.State == WebSocketState.Open)
            {
                var r = await ws.ReceiveAsync(buffer, ct);
                if (r.MessageType == WebSocketMessageType.Close) return;
                message.Write(buffer, 0, r.Count);
                if (!r.EndOfMessage) continue;
                var text = Encoding.UTF8.GetString(message.ToArray());
                message.SetLength(0);
                Requests.Enqueue(text);
                using var doc = JsonDocument.Parse(text);
                var reply = Respond(doc.RootElement);
                if (reply is null) continue;
                await SendAsync(ws, reply);
                if (AfterReply is { } after) await after(doc.RootElement, ws);
            }
        }
        catch (Exception) { /* the client went away */ }
    }

    public async Task SendAsync(WebSocket ws, string text)
    {
        await sendLock.WaitAsync();
        try { await ws.SendAsync(Encoding.UTF8.GetBytes(text), WebSocketMessageType.Text, true, CancellationToken.None); }
        finally { sendLock.Release(); }
    }

    /// <summary>Kill the current connection without a close handshake (an engine crash).</summary>
    public void Drop() => current?.Abort();

    public IEnumerable<JsonElement> RequestsFor(string method) =>
        Requests.Select(r => JsonDocument.Parse(r).RootElement).Where(r => r.GetProperty("method").GetString() == method);

    public async ValueTask DisposeAsync()
    {
        current?.Abort();
        using var cts = new CancellationTokenSource(Bound);
        try { await app.StopAsync(cts.Token); }
        catch (OperationCanceledException) { /* a connection that would not drain; the host is disposed below */ }
        await app.DisposeAsync().AsTask().WaitAsync(Bound);
    }
}

/// <summary>Records <see cref="RpcClient"/> callbacks and lets a test await a state or an event count.</summary>
internal sealed class Recorder
{
    private readonly object gate = new();
    public readonly List<RpcClient.State> States = new();
    public readonly List<string> Events = new();

    public void OnState(RpcClient.State s) { lock (gate) States.Add(s); }

    public void OnEvent(byte[] e) { lock (gate) Events.Add(Encoding.UTF8.GetString(e)); }

    public RpcClient.State[] StatesSnapshot() { lock (gate) return States.ToArray(); }

    public string[] EventsSnapshot() { lock (gate) return Events.ToArray(); }

    public async Task WaitAsync(Func<bool> condition, double seconds = 10)
    {
        var until = DateTime.UtcNow.AddSeconds(seconds);
        while (!condition())
        {
            if (DateTime.UtcNow > until) throw new TimeoutException("condition not met in time");
            await Task.Delay(20);
        }
    }

    public Task WaitOpenCountAsync(int n, double seconds = 10) =>
        WaitAsync(() => StatesSnapshot().Count(s => s == RpcClient.State.Open) >= n, seconds);
}

/// <summary>The JSON-RPC client the Swift companion only verified live: upgrade headers, positional params, the
/// subscription, errors, loss + redial + resubscribe, close.</summary>
[Collection("loopback engine")] // 149 WW1: serialised with RpcClient149Tests (two Kestrel hosts in parallel hung the test host)
public sealed class RpcClientTests
{
    private static string Params(JsonElement request) => request.GetProperty("params").GetRawText();

    private static Task<T> B<T>(Task<T> t) => t.WaitAsync(FakeEngine.Bound);

    private static Task B(Task t) => t.WaitAsync(FakeEngine.Bound);

    [Fact]
    public async Task UpgradeCarriesTheBearerTokenAndTheCompanionClientHeader()
    {
        await using var engine = await FakeEngine.StartAsync();
        var rec = new Recorder();
        await using var client = new RpcClient(new EngineLocation(engine.Url, "tok-64"), rec.OnState, rec.OnEvent);
        client.Start();
        await rec.WaitOpenCountAsync(1);
        var items = await B(client.AttentionListAsync());
        Assert.Empty(items);
        Assert.True(engine.Upgrades.TryPeek(out var up));
        Assert.Equal("Bearer tok-64", up.Authorization);
        Assert.Equal("companion", up.Client);
        Assert.Equal("""{"id":1,"jsonrpc":"2.0","method":"attention_list","params":[]}""", engine.Requests.Single());
        Assert.Equal(new[] { RpcClient.State.Connecting, RpcClient.State.Open }, rec.StatesSnapshot());
    }

    [Fact]
    public async Task CallsArePositionalInTheEnginesOrder()
    {
        await using var engine = await FakeEngine.StartAsync();
        engine.Respond = r => r.GetProperty("method").GetString() switch
        {
            "answer_ask" or "resolve_approval" => FakeEngine.Reply(r, "true"),
            "send_input" => FakeEngine.Reply(r, "\"s-1\""),
            _ => FakeEngine.DefaultRespond(r),
        };
        var rec = new Recorder();
        await using var client = new RpcClient(new EngineLocation(engine.Url, "t"), rec.OnState, rec.OnEvent);
        client.Start();
        await rec.WaitOpenCountAsync(1);

        Assert.True(await B(client.AnswerAskAsync("ask-1", null)));
        Assert.True(await B(client.AnswerAskAsync("ask-1", new Answer[] { new Answer.Choice(1) })));
        Assert.True(await B(client.ResolveApprovalAsync("ap-1", Decision.Deny)));
        Assert.Equal("s-1", await B(client.SendInputAsync("s-1", "hi")));
        await B(client.AttentionSnoozeAsync("ask:1", 1_791_450_600_000));
        await B(client.AttentionDismissAsync("show:1"));
        await B(client.TaskDecideAsync("decision:7", "B"));
        await B(client.TaskMoveAsync("task-9", "todo", null));
        Assert.Null(await B(client.PacketGetAsync("decision:7")));

        string P(string method, int nth = 0) => Params(engine.RequestsFor(method).ElementAt(nth));
        Assert.Equal("""["ask-1",null]""", P("answer_ask"));
        Assert.Equal("""["ask-1",[{"index":1,"kind":"choice"}]]""", P("answer_ask", 1));
        Assert.Equal("""["ap-1","deny"]""", P("resolve_approval"));
        Assert.Equal("""["s-1","hi",[],null,null]""", P("send_input"));
        Assert.Equal("""["ask:1",1791450600000]""", P("attention_snooze"));
        Assert.Equal("""["show:1"]""", P("attention_dismiss"));
        Assert.Equal("""["decision:7","B"]""", P("task_decide"));
        Assert.Equal("""["task-9","todo",null]""", P("task_move"));
        Assert.Equal("""["decision:7"]""", P("packet_get"));
        var ids = engine.Requests.Select(r => JsonDocument.Parse(r).RootElement.GetProperty("id").GetInt32()).ToArray();
        Assert.Equal(Enumerable.Range(1, ids.Length), ids); // numeric ids, one per call
    }

    [Fact]
    public async Task TheSubscriptionDeliversOnlyItsOwnItemsEvenRightBehindTheReply()
    {
        await using var engine = await FakeEngine.StartAsync();
        engine.AfterReply = async (request, ws) =>
        {
            if (request.GetProperty("method").GetString() != "subscribe_events") return;
            // The first item follows the reply at once (jsonrpsee); a foreign subscription's item must be dropped.
            await engine.SendAsync(ws, """{"jsonrpc":"2.0","method":"subscribe_events","params":{"subscription":"sub-1","result":{"type":"PendingPrompts","approvals":[],"asks":[],"secrets":[]}}}""");
            await engine.SendAsync(ws, """{"jsonrpc":"2.0","method":"subscribe_events","params":{"subscription":"sub-2","result":{"type":"Lagged","count":1}}}""");
            await engine.SendAsync(ws, """{"jsonrpc":"2.0","method":"subscribe_events","params":{"subscription":"sub-1","result":{"type":"Lagged","count":2}}}""");
        };
        var rec = new Recorder();
        await using var client = new RpcClient(new EngineLocation(engine.Url, "t"), rec.OnState, rec.OnEvent);
        client.SubscribeEvents();
        client.Start();
        await rec.WaitAsync(() => rec.EventsSnapshot().Length >= 2);
        await Task.Delay(100);
        var events = rec.EventsSnapshot().Select(e => WireEvent.Decode(e)).ToArray();
        Assert.Equal(new[] { "PendingPrompts", "Lagged" }, events.Select(e => e.Type));
        Assert.Equal(2ul, Assert.IsType<Lagged>(events[1].Payload).Count);
        Assert.Equal("[null,null]", Params(engine.RequestsFor("subscribe_events").Single()));
    }

    [Fact]
    public async Task AnErrorReplyThrowsTheEnginesCodeAndMessage()
    {
        await using var engine = await FakeEngine.StartAsync();
        engine.Respond = r => r.GetProperty("method").GetString() == "mesh_status"
            ? $$$"""{"jsonrpc":"2.0","id":{{{r.GetProperty("id").GetRawText()}}},"error":{"code":-32601,"message":"Method not found"}}"""
            : FakeEngine.DefaultRespond(r);
        var rec = new Recorder();
        await using var client = new RpcClient(new EngineLocation(engine.Url, "t"), rec.OnState, rec.OnEvent);
        client.Start();
        await rec.WaitOpenCountAsync(1);
        var e = await Assert.ThrowsAsync<RpcException>(() => B(client.CallRawAsync("mesh_status")));
        Assert.Equal(-32601, e.Code);
        Assert.Equal("rpc -32601: Method not found", e.ToString());
    }

    [Fact]
    public async Task ACallBeforeTheSocketOpensIsNotConnected()
    {
        var rec = new Recorder();
        await using var client = new RpcClient(new EngineLocation(new Uri("ws://127.0.0.1:9/"), "t"), rec.OnState, rec.OnEvent);
        var e = await Assert.ThrowsAsync<RpcException>(() => B(client.AttentionListAsync()));
        Assert.Equal(-32300, e.Code);
    }

    [Fact]
    public async Task ALostSocketFailsPendingCallsThenRedialsAndResubscribes()
    {
        await using var engine = await FakeEngine.StartAsync();
        engine.Respond = r => r.GetProperty("method").GetString() == "list_activity" ? null : FakeEngine.DefaultRespond(r);
        var rec = new Recorder();
        await using var client = new RpcClient(new EngineLocation(engine.Url, "t"), rec.OnState, rec.OnEvent);
        client.SubscribeEvents();
        client.Start();
        await rec.WaitOpenCountAsync(1);
        await rec.WaitAsync(() => engine.RequestsFor("subscribe_events").Any());
        var hanging = client.ListActivityAsync();
        await rec.WaitAsync(() => engine.RequestsFor("list_activity").Any());
        engine.Drop();
        var e = await Assert.ThrowsAsync<RpcException>(() => hanging);
        Assert.Equal("connection closed", e.Message);
        await rec.WaitOpenCountAsync(2); // after the 1 s backoff
        await rec.WaitAsync(() => engine.RequestsFor("subscribe_events").Count() >= 2);
        Assert.Equal(2, engine.Connections);
        var states = rec.StatesSnapshot();
        Assert.Equal(RpcClient.State.Reconnecting, states[2]);
        Assert.Equal(RpcClient.State.Open, states[^1]);
        Assert.Empty(await B(client.AttentionListAsync())); // the new socket serves calls
    }

    [Fact]
    public async Task NoTokenYetWaitsAndRetriesWithBackoff()
    {
        await using var engine = await FakeEngine.StartAsync();
        int calls = 0;
        var rec = new Recorder();
        await using var client = new RpcClient(
            () => Interlocked.Increment(ref calls) == 1 ? null : new EngineLocation(engine.Url, "t"),
            rec.OnState, rec.OnEvent);
        client.Start();
        await rec.WaitOpenCountAsync(1);
        Assert.Equal(
            new[] { RpcClient.State.Connecting, RpcClient.State.Reconnecting, RpcClient.State.Reconnecting, RpcClient.State.Open },
            rec.StatesSnapshot());
        Assert.Equal(2, calls); // re-located on every dial
    }

    [Fact]
    public async Task CloseFailsPendingCallsAndReportsClosed()
    {
        await using var engine = await FakeEngine.StartAsync();
        engine.Respond = r => r.GetProperty("method").GetString() == "agents_list" ? null : FakeEngine.DefaultRespond(r);
        var rec = new Recorder();
        var client = new RpcClient(new EngineLocation(engine.Url, "t"), rec.OnState, rec.OnEvent);
        client.Start();
        await rec.WaitOpenCountAsync(1);
        var hanging = client.AgentsListAsync();
        await rec.WaitAsync(() => engine.RequestsFor("agents_list").Any());
        client.Close();
        var e = await Assert.ThrowsAsync<RpcException>(() => hanging);
        Assert.Equal("closed", e.Message);
        Assert.Equal(RpcClient.State.Closed, rec.StatesSnapshot()[^1]);
        await Task.Delay(1500);
        Assert.Equal(RpcClient.State.Closed, rec.StatesSnapshot()[^1]); // no redial after close
        await client.DisposeAsync();
    }
}