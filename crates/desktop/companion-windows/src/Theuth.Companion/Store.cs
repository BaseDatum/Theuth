using Microsoft.UI.Dispatching;
using Theuth.Companion.Core;
using Theuth.Companion.Rpc;

namespace Theuth.Companion;

/// <summary>
/// The app's Store (design 134 §5.4): <see cref="StoreModel"/> — the port of the Swift Store — owned by the UI thread
/// (App's <see cref="DispatcherQueue"/>). The JSON-RPC client runs on the thread pool; its state and event callbacks are
/// marshalled here with <c>TryEnqueue</c> before the model sees them, timers are <c>DispatcherQueueTimer</c>s, and async
/// work resumes on the UI thread through the DispatcherQueueSynchronizationContext Program.Main installs — so every
/// <c>PropertyChanged</c> and hook is raised on the UI thread. <c>public sealed partial</c>: W2–W4 add members in their own
/// <c>Store.W2.cs</c> / <c>Store.W3.cs</c> / <c>Store.W4.cs</c> without editing this file.
/// </summary>
public sealed partial class Store : StoreModel
{
    private Store(RpcClient client, DispatcherQueue dispatcher)
        : base(new RpcStoreEngine(client), new DispatcherScheduler(dispatcher), () => DateTimeOffset.Now, Log.Write)
    {
        Client = client;
        Dispatcher = dispatcher;
    }

    /// <summary>The UI thread's queue; marshal anything from another thread here before touching the Store.</summary>
    public DispatcherQueue Dispatcher { get; }

    /// <summary>The engine connection (calls the Store does not wrap, e.g. W4's dictation stream).</summary>
    public RpcClient Client { get; }

    /// <summary>
    /// A Store whose client locates the engine with <paramref name="locate"/> before every dial (engine.json + token,
    /// <c>X-Theuth-Client: companion</c>). Subscribe to its events, then <see cref="Start"/>.
    /// </summary>
    public static Store Create(DispatcherQueue dispatcher, Func<EngineLocation?> locate)
    {
        Store? store = null;
        var client = new RpcClient(
            locate,
            s => dispatcher.TryEnqueue(() => store?.HandleState(s)),
            e => dispatcher.TryEnqueue(() => store?.HandleEvent(e)));
        store = new Store(client, dispatcher);
        return store;
    }

    /// <summary>Subscribe to the event stream and dial (redialling until <see cref="Close"/>).</summary>
    public void Start()
    {
        Client.SubscribeEvents();
        Client.Start();
    }

    public void Close() => Client.Close();
}

/// <summary><see cref="IStoreScheduler"/> on <c>DispatcherQueueTimer</c>s of the UI thread.</summary>
internal sealed class DispatcherScheduler(DispatcherQueue dispatcher) : IStoreScheduler
{
    // Running timers stay referenced until they end or are cancelled.
    private readonly HashSet<Handle> live = new();

    public IDisposable After(TimeSpan delay, Action action) => Start(delay, repeating: false, action);

    public IDisposable Every(TimeSpan interval, Action action) => Start(interval, repeating: true, action);

    private Handle Start(TimeSpan interval, bool repeating, Action action)
    {
        var timer = dispatcher.CreateTimer();
        timer.Interval = interval;
        timer.IsRepeating = repeating;
        var handle = new Handle(this, timer);
        timer.Tick += (_, _) =>
        {
            if (!repeating) handle.Dispose();
            action();
        };
        live.Add(handle);
        timer.Start();
        return handle;
    }

    private sealed class Handle(DispatcherScheduler owner, DispatcherQueueTimer timer) : IDisposable
    {
        public void Dispose()
        {
            timer.Stop();
            owner.live.Remove(this);
        }
    }
}
