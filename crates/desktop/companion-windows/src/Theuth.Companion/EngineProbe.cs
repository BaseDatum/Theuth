using Microsoft.UI.Dispatching;
using Theuth.Companion.Core;
using Theuth.Companion.Rpc;

namespace Theuth.Companion;

/// <summary>
/// W1's engine connection: the Swift companion's <c>--probe N</c> path (<c>AppDelegate.runProbe</c> + the Store's refetch)
/// without surfaces. It locates the engine before every dial (<c>engine.json</c> + token; <c>X-Theuth-Client: companion</c>
/// on the upgrade), subscribes to events, logs the first <c>PendingPrompts</c>, refetches <c>attention_list</c> ·
/// <c>agents_list</c> · <c>list_activity</c> on the events the Store refetches on (250 ms debounce; PendingPrompts,
/// AttentionChanged and Lagged at once) and logs the counts with the derived rail and headline, in the Swift W1 probe shape.
/// With N it says done and exits 0 after N s; without, it keeps logging (the W1 shell; W2's Store replaces it).
/// Everything here runs on the UI thread (the client's callbacks are marshalled with <c>TryEnqueue</c>, §5.4).
/// </summary>
internal sealed class EngineProbe : IDisposable
{
    private readonly DispatcherQueue dispatcher;
    private readonly double? seconds;
    private readonly Action<int> quit;
    private readonly string prefix;
    private RpcClient? client;
    private DispatcherQueueTimer? refetchTimer;
    private DispatcherQueueTimer? doneTimer;
    private DispatcherQueueTimer? lingerTimer;
    private bool closed;
    private bool loggedPending;
    private bool fetching;
    private bool fetchAgain;
    private bool open;
    private int refetches;

    public EngineProbe(DispatcherQueue dispatcher, double? seconds, Action<int> quit)
    {
        this.dispatcher = dispatcher;
        this.seconds = seconds;
        this.quit = quit;
        prefix = seconds is null ? "state" : "probe";
    }

    public void Start()
    {
        client = new RpcClient(
            Locate,
            s => dispatcher.TryEnqueue(() => OnState(s)),
            e => dispatcher.TryEnqueue(() => OnEvent(e)));
        client.SubscribeEvents();
        client.Start();
        if (seconds is { } s)
        {
            doneTimer = dispatcher.CreateTimer();
            doneTimer.Interval = TimeSpan.FromSeconds(s);
            doneTimer.IsRepeating = false;
            doneTimer.Tick += (_, _) => Finish();
            doneTimer.Start();
        }
    }

    /// <summary>Runs before every dial (on the client's thread): re-discovered, so an engine that starts later or moves port is found.</summary>
    private static EngineLocation? Locate()
    {
        try
        {
            var loc = EngineLocation.Discover();
            Log.Write($"engine {loc.Url.AbsoluteUri} (token {loc.Token.Length} chars)");
            return loc;
        }
        catch (EngineLocation.NoTokenException e)
        {
            Log.Write($"engine: {e.Message} — waiting for the engine");
            return null;
        }
        catch (Exception e)
        {
            Log.Write($"engine: discovery failed: {e.Message}");
            return null;
        }
    }

    private void OnState(RpcClient.State s)
    {
        open = s == RpcClient.State.Open;
        Log.Write($"connection: {s.ToString().ToLowerInvariant()}");
    }

    private void OnEvent(byte[] data)
    {
        WireEvent e;
        try { e = WireEvent.Decode(data); }
        catch (Exception) { return; }
        switch (e.Payload)
        {
            case PendingPrompts p:
                if (!loggedPending)
                {
                    loggedPending = true;
                    var first = p.Approvals.FirstOrDefault() is { } a ? $"{a.Tool} {a.Risk.RawValue()}"
                        : p.Asks.FirstOrDefault() is { } q ? q.Questions.FirstOrDefault()?.Text ?? ""
                        : "none";
                    Log.Write($"{prefix} PendingPrompts approvals={p.Approvals.Count} asks={p.Asks.Count} secrets={p.Secrets.Count} first={first}");
                }
                ScheduleRefetch(immediate: true);
                break;
            case AttentionChanged or Lagged:
                ScheduleRefetch(immediate: true);
                break;
            default:
                if (!e.IsOther || WireEvent.AttentionTriggers.Contains(e.Type)) ScheduleRefetch(immediate: false);
                break;
        }
    }

    /// <summary>Debounced 250 ms (130 §5.2); never on a timer of its own.</summary>
    private void ScheduleRefetch(bool immediate)
    {
        refetchTimer?.Stop();
        if (immediate)
        {
            Refetch();
            return;
        }
        refetchTimer ??= CreateRefetchTimer();
        refetchTimer.Start();
    }

    private DispatcherQueueTimer CreateRefetchTimer()
    {
        var t = dispatcher.CreateTimer();
        t.Interval = TimeSpan.FromMilliseconds(250);
        t.IsRepeating = false;
        t.Tick += (_, _) => Refetch();
        return t;
    }

    private async void Refetch()
    {
        if (client is null) return;
        if (fetching)
        {
            fetchAgain = true;
            return;
        }
        fetching = true;
        try
        {
            var a = client.AttentionListAsync();
            var g = client.AgentsListAsync();
            var l = client.ListActivityAsync();
            await Task.WhenAll(a, g, l);
            var items = a.Result;
            var agents = g.Result;
            var activity = l.Result;
            var rail = RailModel.Derive(items, agents, activity, DateTimeOffset.Now);
            var panel = new PanelModel(items, rail.WorkingCount);
            refetches++;
            Log.Write($"{prefix} attention_list={items.Count} [{string.Join(",", items.Select(i => i.Kind.RawValue))}] " +
                      $"agents_list={agents.Count} list_activity={activity.Count} live={activity.Count(x => x.Activity.Live)} " +
                      $"rail: needsYou={rail.NeedsYouCount} working={rail.WorkingCount} marks={rail.Marks.Count} " +
                      $"quiet={(rail.Quiet ? "true" : "false")} headline=\"{panel.Headline}\"");
        }
        catch (Exception e)
        {
            Log.Write($"refetch failed: {(e is RpcException r ? r.ToString() : e.GetType().Name + ": " + e.Message)}");
        }
        finally
        {
            fetching = false;
        }
        if (fetchAgain)
        {
            fetchAgain = false;
            Refetch();
        }
    }

    private void Finish()
    {
        var connected = open ? "true" : "false";
        Log.Write($"probe done after {Flags.Describe(seconds)} s — quitting (engine connected={connected}, refetches={refetches}) exit 0");
        CloseClient();
        // Two seconds with the last line on screen, so a UIA read of the log sees it; then exit 0 (§5.5).
        lingerTimer = dispatcher.CreateTimer();
        lingerTimer.Interval = TimeSpan.FromSeconds(2);
        lingerTimer.IsRepeating = false;
        lingerTimer.Tick += (_, _) => quit(0);
        lingerTimer.Start();
    }

    private void CloseClient()
    {
        if (closed) return;
        closed = true;
        client?.Close();
    }

    public void Dispose()
    {
        refetchTimer?.Stop();
        doneTimer?.Stop();
        lingerTimer?.Stop();
        CloseClient();
    }
}
