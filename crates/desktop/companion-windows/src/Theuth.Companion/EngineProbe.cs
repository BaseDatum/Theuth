using System.ComponentModel;
using Microsoft.UI.Dispatching;
using Theuth.Companion.Core;
using Theuth.Companion.Rpc;

namespace Theuth.Companion;

/// <summary>
/// The Swift companion's <c>--probe N</c> (<c>AppDelegate.runProbe</c>) over the Store: logs every connection change, the
/// first <c>PendingPrompts</c> and, after each refetch, the counts with the derived rail and headline — the Swift W1 probe
/// shape. With N it says done and exits 0 after N s; without, the same lines are logged as <c>state …</c> (the W1 shell
/// until W2's surfaces). <see cref="Locate"/> is the Store's engine discovery (logged before every dial).
/// </summary>
internal sealed class EngineProbe : IDisposable
{
    private readonly Store store;
    private readonly DispatcherQueue dispatcher;
    private readonly double? seconds;
    private readonly Action<int> quit;
    private readonly string prefix;
    private DispatcherQueueTimer? doneTimer;
    private DispatcherQueueTimer? lingerTimer;
    private bool loggedPending;
    private bool open;
    private int refetches;

    public EngineProbe(Store store, DispatcherQueue dispatcher, double? seconds, Action<int> quit)
    {
        this.store = store;
        this.dispatcher = dispatcher;
        this.seconds = seconds;
        this.quit = quit;
        prefix = seconds is null ? "state" : "probe";
    }

    /// <summary>Runs before every dial (on the client's thread): re-discovered, so an engine that starts later or moves port is found.</summary>
    public static EngineLocation? Locate()
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

    /// <summary>Subscribe (before <see cref="Store.Start"/>) and arm the done timer.</summary>
    public void Start()
    {
        store.PropertyChanged += OnChanged;
        store.PendingPromptsReceived += OnPending;
        store.Refetched += OnRefetched;
        if (seconds is { } s)
        {
            doneTimer = dispatcher.CreateTimer();
            doneTimer.Interval = TimeSpan.FromSeconds(s);
            doneTimer.IsRepeating = false;
            doneTimer.Tick += (_, _) => Finish();
            doneTimer.Start();
        }
    }

    private void OnChanged(object? sender, PropertyChangedEventArgs e)
    {
        if (e.PropertyName != nameof(StoreModel.Connection)) return;
        open = store.Connection == RpcClient.State.Open;
        Log.Write($"connection: {store.Connection.ToString().ToLowerInvariant()}");
    }

    private void OnPending(PendingPrompts p)
    {
        if (loggedPending) return;
        loggedPending = true;
        var first = p.Approvals.FirstOrDefault() is { } a ? $"{a.Tool} {a.Risk.RawValue()}"
            : p.Asks.FirstOrDefault() is { } q ? q.Questions.FirstOrDefault()?.Text ?? ""
            : "none";
        Log.Write($"{prefix} PendingPrompts approvals={p.Approvals.Count} asks={p.Asks.Count} secrets={p.Secrets.Count} first={first}");
    }

    private void OnRefetched()
    {
        refetches++;
        var rail = store.Rail;
        Log.Write($"{prefix} attention_list={store.Items.Count} [{string.Join(",", store.Items.Select(i => i.Kind.RawValue))}] " +
                  $"agents_list={store.Agents.Count} list_activity={store.Activity.Count} live={store.Activity.Count(x => x.Activity.Live)} " +
                  $"rail: needsYou={rail.NeedsYouCount} working={rail.WorkingCount} marks={rail.Marks.Count} " +
                  $"quiet={(rail.Quiet ? "true" : "false")} headline=\"{store.Panel.Headline}\"");
    }

    private void Finish()
    {
        var connected = open ? "true" : "false";
        Log.Write($"probe done after {Flags.Describe(seconds)} s — quitting (engine connected={connected}, refetches={refetches}) exit 0");
        store.Close();
        // Two seconds with the last line on screen, so a UIA read of the log sees it; then exit 0 (§5.5).
        lingerTimer = dispatcher.CreateTimer();
        lingerTimer.Interval = TimeSpan.FromSeconds(2);
        lingerTimer.IsRepeating = false;
        lingerTimer.Tick += (_, _) => quit(0);
        lingerTimer.Start();
    }

    public void Dispose()
    {
        doneTimer?.Stop();
        lingerTimer?.Stop();
        store.PropertyChanged -= OnChanged;
        store.PendingPromptsReceived -= OnPending;
        store.Refetched -= OnRefetched;
    }
}
