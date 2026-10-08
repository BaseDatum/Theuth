using System.ComponentModel;
using System.Security.Principal;
using Microsoft.UI.Dispatching;
using Microsoft.Windows.AppNotifications;
using Microsoft.Windows.AppNotifications.Builder;
using Theuth.Companion.Core;
using Theuth.Companion.Rpc;
using Theuth.Companion.Settings;

namespace Theuth.Companion.Notifications;

/// <summary>
/// 130 D8 / §4.4 / R1-4 on Windows (design 134 §4.4, D8): the companion's toasts. Policy decisions come from Core's
/// <see cref="NotificationPolicy"/>; this class registers with <c>AppNotificationManager</c> (handler attached first,
/// <c>Register()</c> before any window — <see cref="RegisterEarly"/>), posts/withdraws, and routes button taps into the
/// Store's answer path (<see cref="Store.AnswerNow"/>, no hold) — then a receipt toast. <c>NotificationInvoked</c> arrives
/// on a background thread and is marshalled to the UI thread; a click while the companion is not running relaunches it
/// (<c>Kind=Launch</c>) and the arguments arrive here after <c>Register()</c>, so they are held until the first refetch.
/// Nothing asks for authorization: Windows has none for normal toasts; the Urgent opt-in makes Windows ask once.
/// </summary>
internal sealed class Notifier : IDisposable
{
    public static string ProbeLog { get; } = Path.Combine(Log.Dir, "probe-notify.log");

    /// <summary>Activations that arrived before the Store had items (the relaunch path).</summary>
    private static readonly List<AppNotificationActivatedEventArgs> Early = new();
    private static readonly object EarlyGate = new();
    private static Notifier? current;
    private static bool registered;
    private static bool urgentSupported;

    private readonly Store store;
    private readonly DispatcherQueue dispatcher;
    private readonly SettingsModel settings;
    private readonly NotificationPolicy policy;
    private readonly AppNotificationManager manager = AppNotificationManager.Default;
    /// <summary>Posted banner id → (tag, group), for withdrawal.</summary>
    private readonly Dictionary<string, (string Tag, string Group)> live = new(StringComparer.Ordinal);
    /// <summary>Banner id → the items it covers (Swift's userInfo["items"]).</summary>
    private readonly Dictionary<string, ValueList<string>> itemsOf = new(StringComparer.Ordinal);
    /// <summary>A toast action is in flight: the next Store receipt becomes a receipt toast.</summary>
    private (string Who, string Label)? receiptFor;
    private readonly List<(string Action, string? Banner, string? Item, string? Text)> pendingActivations = new();
    private bool seeded;
    private bool refetchedOnce;
    private int posted;
    private int withdrawn;

    /// <summary>What the live policy posted per category while running (the probe's "new-while-running→posted").</summary>
    public Dictionary<string, int> PostedByCategory { get; } = new(StringComparer.Ordinal);

    public DateTimeOffset? PausedUntil { get; private set; }

    /// <summary>Pause state or the Windows setting changed (Settings › Notifications redraws).</summary>
    public event Action? Changed;

    public bool InFlood => policy.InFlood;
    public int HeldCount => policy.HeldCount;
    public (int Posted, int Withdrawn) Counts => (posted, withdrawn);
    public static bool UrgentSupported => urgentSupported;

    public Notifier(Store store, DispatcherQueue dispatcher, SettingsModel settings)
    {
        this.store = store;
        this.dispatcher = dispatcher;
        this.settings = settings;
        policy = new NotificationPolicy(settings.Value.Notify);
        current = this;
    }

    // MARK: registration (before any window — §4.4, C1)

    /// <summary>
    /// Attach <c>NotificationInvoked</c>, then <c>Register()</c>: for an unpackaged app this writes the AUMID/CLSID under
    /// HKCU\Software\Classes pointing at this exe and sets up COM activation (C1). Logged, never thrown — a failed
    /// registration (0x8007007E without the five runtime-MSIX files, spike S2) leaves the companion without toasts.
    /// </summary>
    public static void RegisterEarly()
    {
        if (registered) return;
        try
        {
            if (IsElevated()) Log.Write("notifications: WARNING the companion is elevated — Windows drops toasts from elevated processes (C2)");
            var m = AppNotificationManager.Default;
            m.NotificationInvoked += OnInvokedStatic;
            m.Register();
            registered = true;
            urgentSupported = AppNotificationBuilder.IsUrgentScenarioSupported();
            Log.Write($"notifications: Register() ok Setting={m.Setting} urgentSupported={(urgentSupported ? "true" : "false")} exe={Environment.ProcessPath}");
        }
        catch (Exception e)
        {
            Log.Write($"notifications: Register() FAILED hr=0x{e.HResult:X8} {e.GetType().Name}: {e.Message}");
        }
    }

    public static bool Registered => registered;

    /// <summary>Call before exit so a later click launches a new process (quickstart: Unregister, not UnregisterAll).</summary>
    public static void UnregisterAtExit()
    {
        if (!registered) return;
        try { AppNotificationManager.Default.Unregister(); Log.Write("notifications: Unregister() ok"); }
        catch (Exception e) { Log.Write($"notifications: Unregister() failed: {e.Message}"); }
        registered = false;
    }

    private static bool IsElevated()
    {
        try
        {
            using var id = WindowsIdentity.GetCurrent();
            return new WindowsPrincipal(id).IsInRole(WindowsBuiltInRole.Administrator);
        }
        catch (Exception) { return false; }
    }

    /// <summary>Background thread (C1). Held until a Notifier exists, then marshalled to the UI thread.</summary>
    private static void OnInvokedStatic(AppNotificationManager sender, AppNotificationActivatedEventArgs args)
    {
        Log.Write($"notifications: NotificationInvoked thread={Environment.CurrentManagedThreadId} argument='{args.Argument}' input={args.UserInput.Count}");
        Notifier? n;
        lock (EarlyGate)
        {
            n = current;
            if (n is null) { Early.Add(args); return; }
        }
        n.dispatcher.TryEnqueue(() => Guard.Run("toast activation", () => n.Invoked(args)));
    }

    // MARK: start

    public void Start()
    {
        store.PropertyChanged += OnStoreChanged;
        store.Refetched += OnRefetched;
        settings.Changed += OnSettingsChanged;
        List<AppNotificationActivatedEventArgs> early;
        lock (EarlyGate)
        {
            early = Early.ToList();
            Early.Clear();
        }
        foreach (var a in early) Invoked(a);
        Changed?.Invoke();
    }

    private void OnSettingsChanged(CompanionSettings old, CompanionSettings now)
    {
        if (old.Notify != now.Notify)
        {
            policy.Prefs = now.Notify;
            Log.Write($"notifications: prefs {Describe(now.Notify)}");
        }
    }

    public static string Describe(NotificationPrefs p) =>
        $"(blocking: {p.Blocking.RawValue()}, deferrable: {p.Deferrable.RawValue()}, error: {p.Error.RawValue()}, soundOnErrors: {(p.SoundOnErrors ? "true" : "false")}, breakThroughDnd: {(p.TimeSensitiveAvailable ? "true" : "false")})";

    private void OnStoreChanged(object? sender, PropertyChangedEventArgs e) => Guard.Run($"notifier on {e.PropertyName}", () => StoreChanged(e));

    private void StoreChanged(PropertyChangedEventArgs e)
    {
        switch (e.PropertyName)
        {
            case nameof(Store.Items):
                // The first snapshot seeds the policy (items already waiting are not "new"), as the Swift Notifier's dropFirst.
                var items = store.Items;
                Run(p => p.Update(items, DateTimeOffset.Now));
                if (!seeded)
                {
                    seeded = true;
                    _ = ReconcileAsync(items);
                }
                break;
            case nameof(Store.Connection):
                var s = store.Connection;
                if (s == RpcClient.State.Connecting) return;
                Run(p => p.Engine(s == RpcClient.State.Open, DateTimeOffset.Now));
                break;
            case nameof(Store.Receipt):
                Receipt(store.Receipt);
                break;
        }
    }

    private void OnRefetched()
    {
        refetchedOnce = true;
        if (pendingActivations.Count == 0) return;
        var pending = pendingActivations.ToList();
        pendingActivations.Clear();
        foreach (var (action, banner, item, text) in pending) Handle(action, banner, item, text, fromRelaunch: true);
    }

    // MARK: breakpoints · tick · pause (BreakpointSource drives these)

    public void Breakpoint() => Run(p => p.Breakpoint(DateTimeOffset.Now));

    public void Tick(DateTimeOffset now)
    {
        Run(p => p.Tick(now));
        if (PausedUntil != policy.PausedUntil)
        {
            PausedUntil = policy.PausedUntil;
            Changed?.Invoke();
        }
    }

    public void Pause(DateTimeOffset? until)
    {
        policy.Pause(until);
        PausedUntil = until;
        Log.Write($"notifications: pause until {(until is { } u ? u.ToString("yyyy-MM-dd HH:mm") : "—")}");
        Changed?.Invoke();
    }

    public static DateTimeOffset Tomorrow => new(DateTime.Today.AddDays(1));

    /// <summary>The Windows setting as a sentence for Settings › Notifications.</summary>
    public string SettingText
    {
        get
        {
            try
            {
                return manager.Setting switch
                {
                    AppNotificationSetting.Enabled => "enabled",
                    AppNotificationSetting.DisabledForApplication => "off for this app in Settings › System › Notifications",
                    AppNotificationSetting.DisabledForUser => "off for this user in Settings › System › Notifications",
                    AppNotificationSetting.DisabledByGroupPolicy => "disabled by group policy",
                    AppNotificationSetting.DisabledByManifest => "disabled by the app manifest",
                    _ => "unsupported",
                };
            }
            catch (Exception e) { return $"unknown ({e.Message})"; }
        }
    }

    /// <summary>
    /// The one place a dialog may come from, user-initiated (Settings' opt-in switch, <c>--request-notifications</c>):
    /// one Urgent toast — Windows then shows "Important Notification Request … Allow / Not now" once per app.
    /// </summary>
    public void RequestUrgentConsent()
    {
        if (!registered) return;
        if (!urgentSupported)
        {
            Log.Write("notifications: urgent scenario not supported on this Windows — the opt-in has no effect");
            return;
        }
        var b = new Banner("consent-" + Stamp(), BannerCategory.Error, "theuth companion",
            "Errors will break through Do not disturb. Windows asks once whether to allow that.", "theuth", BannerLevel.TimeSensitive, false, new ValueList<string>());
        Show(b, Array.Empty<string>(), ToastBuilder.TagFor(b.Id), ToastBuilder.GroupFor(b.Thread), "consent");
    }

    // MARK: policy → toasts

    private void Run(Func<NotificationPolicy, PolicyOutput> step)
    {
        var output = step(policy);
        if (output.IsEmpty) return;
        foreach (var id in output.Withdraw) Withdraw(id);
        foreach (var b in output.Post) Post(b);
    }

    /// <summary>The live path: group = the agent, tag = the banner id, buttons from the card.</summary>
    private void Post(Banner b)
    {
        var tag = ToastBuilder.TagFor(b.Id);
        var group = ToastBuilder.GroupFor(b.Thread);
        itemsOf[b.Id] = b.ItemIds;
        if (Show(b, OptionLabels(b), tag, group, "live"))
        {
            live[b.Id] = (tag, group);
            PostedByCategory[NotificationReplay.Camel(b.Category)] = PostedByCategory.GetValueOrDefault(NotificationReplay.Camel(b.Category)) + 1;
        }
    }

    /// <summary>Real option titles from the card (≤ 3, those that need no note); none for batch/error/flood.</summary>
    public IReadOnlyList<string> OptionLabels(Banner b)
    {
        if (b.ItemIds.Count != 1 || b.Category is BannerCategory.Batch or BannerCategory.Error) return Array.Empty<string>();
        var item = store.Items.FirstOrDefault(i => i.Id == b.ItemIds[0]);
        if (item is null) return Array.Empty<string>();
        return store.CardFor(item).Options.Where(o => !o.Action.NeedsNote).Select(o => o.Label).ToList();
    }

    /// <summary>Build and <c>Show</c>; logged with the Swift post line. Public for the probe's per-segment groups.</summary>
    public bool Show(Banner b, IReadOnlyList<string> options, string tag, string group, string origin)
    {
        var line = ToastBuilder.Describe(b, tag, group);
        if (!registered)
        {
            Log.Write($"notifications: NOT registered — dropped {line}");
            return false;
        }
        try
        {
            var n = ToastBuilder.Build(b, options, tag, group, urgentSupported);
            manager.Show(n);
            posted++;
            Log.Write($"notifications: [{origin}] {line} buttons={options.Count} toastId={n.Id}");
            return true;
        }
        catch (Exception e)
        {
            Log.Write($"notifications: [{origin}] {line} ERROR hr=0x{e.HResult:X8} {e.Message}");
            return false;
        }
    }

    /// <summary>W6 item 1: the pin tip's view of W3's rules — paused (nothing is posted) and the deferrable class switched off.</summary>
    public bool IsPaused => policy.IsPaused(DateTimeOffset.Now);

    public bool DeferrableOff => settings.Value.Notify.Deferrable == NotifyMode.Off;

    /// <summary>Posts the first-run pin tip (<see cref="PinTip"/>); logged like every toast. False when not registered or <c>Show</c> threw.</summary>
    public bool ShowPinTip()
    {
        if (!registered)
        {
            Log.Write("notifications: NOT registered — pin tip dropped");
            return false;
        }
        try
        {
            var n = ToastBuilder.BuildPinTip();
            manager.Show(n);
            posted++;
            Log.Write($"notifications: [tip] post tip level=passive sound=false thread={PinTip.Group} id={PinTip.Tag} tag={PinTip.Tag} group={PinTip.Group} title=\"{PinTip.Title}\" button=\"{PinTip.ButtonLabel}\" uri={PinTip.SettingsUri} toastId={n.Id}");
            return true;
        }
        catch (Exception e)
        {
            Log.Write($"notifications: [tip] pin tip ERROR hr=0x{e.HResult:X8} {e.Message}");
            return false;
        }
    }

    /// <summary>An item answered elsewhere (or a drained batch): the toast leaves the centre too (D8 — matters more than on macOS).</summary>
    private void Withdraw(string id)
    {
        itemsOf.Remove(id);
        if (!live.Remove(id, out var where)) return;
        _ = Remove(where.Tag, where.Group, id);
    }

    /// <summary><c>RemoveByTagAndGroupAsync</c>, logged. Public for the probe.</summary>
    public async Task Remove(string tag, string group, string id)
    {
        try
        {
            await manager.RemoveByTagAndGroupAsync(tag, group);
            withdrawn++;
            Log.Write($"notifications: withdraw {id} (tag={tag} group={group})");
        }
        catch (Exception e)
        {
            Log.Write($"notifications: withdraw {id} failed: {e.Message}");
        }
    }

    /// <summary>Every toast of ours in the centre: (tag, group, id).</summary>
    public async Task<IReadOnlyList<AppNotification>> AllAsync()
    {
        try { return (await manager.GetAllAsync()).ToList(); }
        catch (Exception e)
        {
            Log.Write($"notifications: GetAllAsync failed: {e.Message}");
            return Array.Empty<AppNotification>();
        }
    }

    public async Task RemoveAllAsync(string why)
    {
        try
        {
            await manager.RemoveAllAsync();
            Log.Write($"notifications: RemoveAllAsync ({why})");
        }
        catch (Exception e) { Log.Write($"notifications: RemoveAllAsync failed: {e.Message}"); }
    }

    /// <summary>
    /// After the first snapshot: toasts left in the centre by an earlier run whose items are gone (batches, flood, engine,
    /// receipts, probe groups, answered items) are removed — the relaunch form of answered-elsewhere withdrawal.
    /// </summary>
    private async Task ReconcileAsync(IReadOnlyList<AttentionItem> items)
    {
        var keep = new HashSet<string>(items.Select(i => ToastBuilder.TagFor(i.Id)), StringComparer.Ordinal);
        var all = await AllAsync();
        int removed = 0;
        foreach (var n in all)
        {
            if (keep.Contains(n.Tag) && !n.Group.StartsWith("probe-", StringComparison.Ordinal)) continue;
            try { await manager.RemoveByTagAndGroupAsync(n.Tag, n.Group); removed++; }
            catch (Exception) { }
        }
        if (all.Count > 0) Log.Write($"notifications: reconcile at start: {all.Count} in the centre, {removed} stale removed");
    }

    // MARK: taps → the Store's answer path (immediately, no hold) → receipt

    /// <summary>
    /// W2's single instance redirected a second launch's activation here (Ui.Surfaces.Redirected): a toast's arguments when the
    /// click reached a fresh process instead of our COM server; W2 handles plain launches itself.
    /// </summary>
    public void Redirected(Microsoft.Windows.AppLifecycle.AppActivationArguments e)
    {
        if (e.Kind == Microsoft.Windows.AppLifecycle.ExtendedActivationKind.AppNotification && e.Data is AppNotificationActivatedEventArgs a)
        {
            Log.Write($"notifications: redirected toast activation argument='{a.Argument}'");
            Invoked(a);
        }
    }

    private void Invoked(AppNotificationActivatedEventArgs args)
    {
        var a = args.Arguments;
        string action = a.TryGetValue(ToastBuilder.ArgAction, out var ac) ? ac : "open";
        string? banner = a.TryGetValue(ToastBuilder.ArgBanner, out var bn) ? bn : null;
        string? item = a.TryGetValue(ToastBuilder.ArgItem, out var it) ? it : null;
        string? text = args.UserInput.TryGetValue(ToastBuilder.InputText, out var t) ? t.Trim() : null;
        if (!refetchedOnce && store.Items.Count == 0 && action is not ("later" or "open"))
        {
            // Relaunched by the click (Kind=Launch): the Store has nothing yet — answer after the first refetch.
            Log.Write($"notifications: action {action} held until the first refetch (relaunch path)");
            pendingActivations.Add((action, banner, item, text));
            return;
        }
        Handle(action, banner, item, text, fromRelaunch: false);
    }

    private void Handle(string action, string? bannerId, string? itemId, string? text, bool fromRelaunch)
    {
        // The items a banner covers: the live map; else the toast's item argument; else the banner id itself (a single-item
        // banner's id IS its item id — the replay and the relaunch path have no map).
        var ids = bannerId is not null && itemsOf.TryGetValue(bannerId, out var set) ? set
            : itemId is not null ? ValueList.Of(itemId)
            : bannerId is not null && store.Items.Any(i => i.Id == bannerId) ? ValueList.Of(bannerId)
            : new ValueList<string>();
        var item = ids.Count == 1 ? store.Items.FirstOrDefault(i => i.Id == ids[0]) : null;
        Log.Write($"notifications: action {action} banner={bannerId ?? "-"} items=[{string.Join(",", ids)}] text={(text is null ? "none" : $"{text.Length} chars")}{(fromRelaunch ? " (after relaunch)" : "")}");
        switch (action)
        {
            case "open":
                store.RequestOpen(item?.Id ?? ids.FirstOrDefault());
                return;
            case "later":
                return;
            case "clear":
                if (item is null) return;
                Answer(item, "cleared", new CardAction.Dismiss(item.Id));
                return;
            case "reply" or "deny-note" or "send-back":
            {
                if (item is null || string.IsNullOrEmpty(text)) return;
                var card = store.CardFor(item);
                if (card.ReplyAction(text) is not { } reply) return;
                var label = action switch { "deny-note" => "denied with note", "send-back" => "sent back", _ => "replied" };
                Answer(item, label, reply);
                return;
            }
            default:
            {
                if (item is null) return;
                int n = Array.IndexOf(ToastBuilder.OptionActions, action);
                if (n < 0) return;
                var options = store.CardFor(item).Options.Where(o => !o.Action.NeedsNote).ToList();
                if (n >= options.Count) return;
                Answer(item, options[n].Label, options[n].Action);
                return;
            }
        }
    }

    private void Answer(AttentionItem item, string label, CardAction action)
    {
        var who = store.CardFor(item).Who;
        receiptFor = (who, label);
        store.AnswerNow(new StoreModel.Held(item.Id, who, label, action), null, item.Session);
    }

    /// <summary>The Store's outcome for the tapped action → a small receipt toast (withdrawn again after 8 s: Windows keeps toasts).</summary>
    private void Receipt(StoreModel.ReceiptRow? r)
    {
        if (receiptFor is not { } rf || r is null) return;
        string body;
        switch (r)
        {
            case StoreModel.ReceiptRow.Sent _: body = $"{rf.Label} — sent"; break;
            case StoreModel.ReceiptRow.Stale _: body = "already answered elsewhere"; break;
            case StoreModel.ReceiptRow.Failed f: body = $"failed: {f.Error}"; break;
            default: return; // holding · undone · the Ask bar's streamed reply
        }
        receiptFor = null;
        var b = new Banner("receipt-" + Stamp(), BannerCategory.Batch, rf.Who, body, rf.Who, BannerLevel.Passive, false, new ValueList<string>());
        var tag = ToastBuilder.TagFor(b.Id);
        var group = ToastBuilder.GroupFor(b.Thread);
        if (!Show(b, Array.Empty<string>(), tag, group, "receipt")) return;
        var timer = dispatcher.CreateTimer();
        timer.Interval = TimeSpan.FromSeconds(8);
        timer.IsRepeating = false;
        timer.Tick += (_, _) => _ = Remove(tag, group, b.Id);
        timer.Start();
    }

    private static string Stamp() => ((ulong)DateTimeOffset.Now.ToUnixTimeMilliseconds()).ToString();

    // MARK: probe log (--probe-notify)

    public void ProbeLine(string s)
    {
        Log.Write($"probe-notify {s}");
        try { File.AppendAllText(ProbeLog, $"{DateTime.Now:yyyy-MM-dd HH:mm:ss.fff} {s}{Environment.NewLine}"); }
        catch (Exception e) when (e is IOException or UnauthorizedAccessException) { }
    }

    public void Dispose()
    {
        store.PropertyChanged -= OnStoreChanged;
        store.Refetched -= OnRefetched;
        settings.Changed -= OnSettingsChanged;
        lock (EarlyGate) { if (current == this) current = null; }
    }
}