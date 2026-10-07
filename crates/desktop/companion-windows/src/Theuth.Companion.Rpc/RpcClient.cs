using System.Net.WebSockets;
using System.Text;
using System.Text.Json;

namespace Theuth.Companion.Rpc;

/// <summary>The engine's JSON-RPC error object (Swift <c>RpcClient.RpcError</c>); prints as <c>rpc &lt;code&gt;: &lt;message&gt;</c>.</summary>
public sealed class RpcException(int code, string message) : Exception(message)
{
    public int Code { get; } = code;

    /// <summary>A call made while no socket is open.</summary>
    public static RpcException NotConnected => new(-32300, "not connected");

    public override string ToString() => $"rpc {Code}: {Message}";
}

/// <summary>
/// JSON-RPC 2.0 over one <see cref="ClientWebSocket"/> (jsonrpsee framing; mirrors <c>UI/rpc/client.ts</c> and
/// <c>SWIFT/TheuthKit/RpcClient.swift</c>): positional params, numeric ids, <c>subscribe_events</c> items as notifications
/// keyed by <c>params.subscription</c>. The upgrade carries <c>Authorization: Bearer &lt;token&gt;</c> and
/// <c>X-Theuth-Client: companion</c> (E4 stamps every call <c>user@companion</c>). Lost sockets are redialled with
/// capped exponential backoff (1 s → 30 s) and the event subscription is re-issued on every open, so the first item
/// (<c>PendingPrompts</c>) arrives again — callers resync on it. Pending calls fail when the socket is lost. The engine is
/// located afresh on every dial (<c>locate</c>), so a companion launched before the engine wrote <c>engine.json</c>, or
/// an engine restarted on a new port, is found on the next attempt.
/// Threading: everything runs on the thread pool; <c>onState</c> and <c>onEvent</c> are called from it (events in
/// arrival order, from the one receive loop) — the app marshals to its UI thread.
/// </summary>
public sealed partial class RpcClient : IAsyncDisposable
{
    public enum State { Connecting, Open, Reconnecting, Closed }

    private sealed class Pending(Action<byte[]>? onResult)
    {
        public readonly TaskCompletionSource<byte[]> Result = new(TaskCreationOptions.RunContinuationsAsynchronously);
        public readonly Action<byte[]>? OnResult = onResult;
    }

    private readonly Func<EngineLocation?> locate;
    private readonly Action<State> onState;
    private readonly Action<byte[]> onEvent;
    private readonly object gate = new();
    private readonly SemaphoreSlim sendLock = new(1, 1);
    private readonly Dictionary<int, Pending> pending = new();
    private ClientWebSocket? socket;
    private CancellationTokenSource? cts;
    private Task? loop;
    private bool open;
    private bool closed = true;
    private bool wantsEvents;
    private bool subscribing;
    private int attempt;
    private int nextId = 1;
    private string? subscriptionId;

    /// <summary>A fixed location (tests, probes).</summary>
    public RpcClient(EngineLocation location, Action<State> onState, Action<byte[]> onEvent)
        : this(() => location, onState, onEvent) { }

    /// <summary><paramref name="locate"/> runs before every dial; null (no token yet) waits and retries like a lost socket.</summary>
    public RpcClient(Func<EngineLocation?> locate, Action<State> onState, Action<byte[]> onEvent)
    {
        this.locate = locate;
        this.onState = onState;
        this.onEvent = onEvent;
    }

    // MARK: lifecycle

    /// <summary>Dial; keeps redialling until <see cref="Close"/>.</summary>
    public void Start()
    {
        CancellationTokenSource c;
        lock (gate)
        {
            if (!closed) return;
            closed = false;
            attempt = 0;
            cts = c = new CancellationTokenSource();
        }
        loop = Task.Run(() => RunAsync(c.Token));
    }

    public void Close()
    {
        ClientWebSocket? s;
        CancellationTokenSource? c;
        lock (gate)
        {
            closed = true;
            s = socket;
            socket = null;
            open = false;
            subscriptionId = null;
            subscribing = false;
            c = cts;
            cts = null;
        }
        c?.Cancel();
        if (s is not null) _ = CloseQuietlyAsync(s, WebSocketCloseStatus.EndpointUnavailable);
        FailPending(new RpcException(-32300, "closed"));
        Notify(State.Closed);
    }

    public async ValueTask DisposeAsync()
    {
        Close();
        if (loop is { } l)
        {
            try { await l.WaitAsync(TimeSpan.FromSeconds(2)).ConfigureAwait(false); }
            catch (Exception) { /* the loop ends on its own once cancelled */ }
        }
    }

    /// <summary>Ask for the event stream (<c>subscribe_events(null, null)</c>); issued now if open, and after every reconnect.</summary>
    public void SubscribeEvents()
    {
        bool isOpen;
        lock (gate)
        {
            wantsEvents = true;
            isOpen = open;
        }
        if (isOpen) _ = IssueSubscriptionAsync();
    }

    private async Task RunAsync(CancellationToken ct)
    {
        while (true)
        {
            int a;
            lock (gate)
            {
                if (closed || ct.IsCancellationRequested) return;
                a = attempt;
            }
            Notify(a == 0 ? State.Connecting : State.Reconnecting);

            EngineLocation? location;
            try { location = locate(); }
            catch (Exception) { location = null; }

            if (location is not null)
            {
                var ws = new ClientWebSocket();
                ws.Options.Proxy = null; // the engine is local; never route it through a system proxy
                ws.Options.SetRequestHeader("Authorization", $"Bearer {location.Token}");
                ws.Options.SetRequestHeader("X-Theuth-Client", "companion");
                bool connected = false;
                try
                {
                    await ws.ConnectAsync(location.Url, ct).ConfigureAwait(false);
                    connected = true;
                }
                catch (Exception) when (!ct.IsCancellationRequested) { }
                catch (Exception)
                {
                    ws.Dispose();
                    return;
                }

                if (connected)
                {
                    bool subscribe;
                    lock (gate)
                    {
                        if (closed)
                        {
                            ws.Abort();
                            ws.Dispose();
                            return;
                        }
                        socket = ws;
                        open = true;
                        attempt = 0;
                        subscriptionId = null;
                        subscribing = false;
                        subscribe = wantsEvents;
                    }
                    Notify(State.Open);
                    if (subscribe) _ = IssueSubscriptionAsync();
                    await ReceiveLoopAsync(ws, ct).ConfigureAwait(false);
                    Lost(ws);
                    lock (gate)
                    {
                        if (closed) return;
                    }
                }
                else
                {
                    ws.Dispose();
                }
            }

            // Capped exponential backoff (1 s → 30 s), then dial again (re-locating the engine).
            Notify(State.Reconnecting);
            double delay;
            lock (gate)
            {
                delay = Math.Min(30.0, Math.Pow(2.0, attempt));
                attempt += 1;
            }
            try { await Task.Delay(TimeSpan.FromSeconds(delay), ct).ConfigureAwait(false); }
            catch (OperationCanceledException) { return; }
        }
    }

    private async Task ReceiveLoopAsync(ClientWebSocket ws, CancellationToken ct)
    {
        var buffer = new byte[64 * 1024];
        using var message = new MemoryStream();
        try
        {
            while (ws.State == WebSocketState.Open)
            {
                var r = await ws.ReceiveAsync(buffer, ct).ConfigureAwait(false);
                if (r.MessageType == WebSocketMessageType.Close) return;
                message.Write(buffer, 0, r.Count);
                if (!r.EndOfMessage) continue;
                var data = message.ToArray();
                message.SetLength(0);
                Dispatch(data);
            }
        }
        catch (Exception)
        {
            // Lost: the caller fails the pending calls and redials.
        }
    }

    private void Lost(ClientWebSocket ws)
    {
        bool current;
        lock (gate)
        {
            current = ReferenceEquals(socket, ws);
            if (current)
            {
                socket = null;
                open = false;
                subscriptionId = null;
                subscribing = false;
            }
        }
        try { ws.Abort(); } catch (Exception) { }
        ws.Dispose();
        if (current) FailPending(new RpcException(-32300, "connection closed"));
    }

    private static async Task CloseQuietlyAsync(ClientWebSocket ws, WebSocketCloseStatus status)
    {
        try
        {
            using var timeout = new CancellationTokenSource(TimeSpan.FromSeconds(1));
            await ws.CloseOutputAsync(status, null, timeout.Token).ConfigureAwait(false);
        }
        catch (Exception) { }
        finally
        {
            try { ws.Abort(); } catch (Exception) { }
            ws.Dispose();
        }
    }

    private void FailPending(Exception e)
    {
        Pending[] all;
        lock (gate)
        {
            all = pending.Values.ToArray();
            pending.Clear();
        }
        foreach (var p in all) p.Result.TrySetException(e);
    }

    private void Notify(State s)
    {
        try { onState(s); }
        catch (Exception) { /* a callback must not break the loop */ }
    }

    // MARK: requests

    /// <summary>One request; decodes the <c>result</c> with the wire codec (a JSON null result throws).</summary>
    public async Task<T> CallAsync<T>(string method, params object?[] parameters) =>
        TheuthJson.Deserialize<T>(await CallRawAsync(method, parameters).ConfigureAwait(false));

    /// <summary>One request whose result may be JSON <c>null</c> (Swift <c>T?</c>).</summary>
    public async Task<T?> CallOrNullAsync<T>(string method, params object?[] parameters) where T : class =>
        TheuthJson.DeserializeOrNull<T>(await CallRawAsync(method, parameters).ConfigureAwait(false));

    /// <summary>One request; the raw UTF-8 JSON of its <c>result</c> (<c>null</c> when absent).</summary>
    public Task<byte[]> CallRawAsync(string method, params object?[] parameters) => RawAsync(method, parameters, null);

    private async Task<byte[]> RawAsync(string method, object?[] parameters, Action<byte[]>? onResult)
    {
        ClientWebSocket ws;
        int id;
        lock (gate)
        {
            if (!open || socket is null) throw RpcException.NotConnected;
            ws = socket;
            id = nextId++;
        }
        var body = EncodeRequest(id, method, parameters); // throws before anything is pending, as Swift's encode does
        var p = new Pending(onResult);
        lock (gate)
        {
            if (!open || !ReferenceEquals(socket, ws)) throw RpcException.NotConnected;
            pending[id] = p;
        }
        try
        {
            await sendLock.WaitAsync().ConfigureAwait(false);
            try
            {
                await ws.SendAsync(body, WebSocketMessageType.Text, endOfMessage: true, CancellationToken.None).ConfigureAwait(false);
            }
            finally
            {
                sendLock.Release();
            }
        }
        catch (Exception e)
        {
            lock (gate) pending.Remove(id);
            p.Result.TrySetException(e);
        }
        return await p.Result.Task.ConfigureAwait(false);
    }

    /// <summary><c>{"id":N,"jsonrpc":"2.0","method":"…","params":[…]}</c> — keys sorted, as the Swift encoder writes them.</summary>
    internal static byte[] EncodeRequest(int id, string method, IReadOnlyList<object?> parameters)
    {
        using var buffer = new MemoryStream();
        using (var w = new Utf8JsonWriter(buffer))
        {
            w.WriteStartObject();
            w.WriteNumber("id", id);
            w.WriteString("jsonrpc", "2.0");
            w.WriteString("method", method);
            w.WritePropertyName("params");
            TheuthJson.WriteParams(w, parameters);
            w.WriteEndObject();
        }
        return buffer.ToArray();
    }

    private async Task IssueSubscriptionAsync()
    {
        lock (gate)
        {
            if (!open || subscriptionId is not null || subscribing) return;
            subscribing = true;
        }
        try
        {
            // The id is recorded inside the dispatch of the reply, before the next frame (the first item) is read.
            await RawAsync("subscribe_events", new object?[] { null, null }, result =>
            {
                using var doc = JsonDocument.Parse(result);
                var sid = Describe(doc.RootElement);
                lock (gate) subscriptionId = sid;
            }).ConfigureAwait(false);
        }
        catch (Exception)
        {
            // Lost mid-handshake: the reconnect path re-issues.
        }
        finally
        {
            lock (gate) subscribing = false;
        }
    }

    private void Dispatch(byte[] data)
    {
        JsonDocument doc;
        try { doc = JsonDocument.Parse(data); }
        catch (JsonException) { return; }
        using (doc)
        {
            var root = doc.RootElement;
            if (root.ValueKind != JsonValueKind.Object) return;
            if (root.TryGetProperty("params", out var prm) && prm.ValueKind == JsonValueKind.Object
                && prm.TryGetProperty("subscription", out var sub))
            {
                string? mine;
                lock (gate) mine = subscriptionId;
                if (mine is null || Describe(sub) != mine || !prm.TryGetProperty("result", out var item)) return;
                var bytes = Encoding.UTF8.GetBytes(item.GetRawText());
                try { onEvent(bytes); }
                catch (Exception) { /* a callback must not break the receive loop */ }
                return;
            }
            if (!root.TryGetProperty("id", out var idElement) || idElement.ValueKind != JsonValueKind.Number
                || !idElement.TryGetInt32(out var id)) return;
            Pending? p;
            lock (gate)
            {
                if (!pending.Remove(id, out p)) return;
            }
            if (root.TryGetProperty("error", out var err) && err.ValueKind == JsonValueKind.Object)
            {
                int code = err.TryGetProperty("code", out var c) && c.ValueKind == JsonValueKind.Number && c.TryGetInt32(out var ci) ? ci : -1;
                string message = err.TryGetProperty("message", out var m) && m.ValueKind == JsonValueKind.String ? m.GetString()! : "error";
                p.Result.TrySetException(new RpcException(code, message));
                return;
            }
            var result = root.TryGetProperty("result", out var r) ? Encoding.UTF8.GetBytes(r.GetRawText()) : "null"u8.ToArray();
            try { p.OnResult?.Invoke(result); }
            catch (Exception) { }
            p.Result.TrySetResult(result);
        }
    }

    /// <summary>A subscription id as text: a string's value, a number's literal (Swift's <c>"\(v)"</c>).</summary>
    private static string Describe(JsonElement e) => e.ValueKind == JsonValueKind.String ? e.GetString()! : e.GetRawText();
}

// MARK: the twelve calls the companion makes (params positional, in crates/rpc/src/api.rs trait order)

public sealed partial class RpcClient
{
    public Task<ValueList<AttentionItem>> AttentionListAsync() => CallAsync<ValueList<AttentionItem>>("attention_list");

    public Task AttentionSnoozeAsync(string id, ulong untilMs) => CallRawAsync("attention_snooze", id, untilMs);

    public Task AttentionDismissAsync(string id) => CallRawAsync("attention_dismiss", id);

    public Task<ValueList<AgentSummary>> AgentsListAsync() => CallAsync<ValueList<AgentSummary>>("agents_list");

    public Task<ValueList<AgentInfo>> ListActivityAsync() => CallAsync<ValueList<AgentInfo>>("list_activity");

    public Task<Packet?> PacketGetAsync(string key) => CallOrNullAsync<Packet>("packet_get", key);

    /// <summary><c>true</c> if the ask was still pending (E5: the engine's own stale check).</summary>
    public Task<bool> AnswerAskAsync(string id, IReadOnlyList<Answer>? answers) =>
        CallAsync<bool>("answer_ask", id, answers is null ? null : new ValueList<Answer>(answers));

    public Task<bool> ResolveApprovalAsync(string id, Decision decision) => CallAsync<bool>("resolve_approval", id, decision);

    public Task TaskDecideAsync(string decision, string answer) => CallRawAsync("task_decide", decision, answer);

    /// <summary><paramref name="state"/> is a <c>TaskState</c> word: <c>done</c> | <c>todo</c> | …</summary>
    public Task TaskMoveAsync(string id, string state, string? note) => CallRawAsync("task_move", id, state, note);

    /// <summary><c>send_input(session, text, [], null, null)</c> → the session id.</summary>
    public Task<string> SendInputAsync(string session, string text) =>
        CallAsync<string>("send_input", session, text, Array.Empty<string>(), null, null);
}
