using Microsoft.UI.Dispatching;
using Theuth.Companion.Breakpoints;
using Theuth.Companion.Core;
using Theuth.Companion.Settings;

namespace Theuth.Companion.Notifications;

/// <summary>
/// W3's one hook into <see cref="App"/> (the Swift <c>NotificationsInstaller</c>): <see cref="RegisterNotifications"/> before
/// any window, then <see cref="Install"/> once the Store exists — builds the <see cref="Notifier"/>, the breakpoint
/// sources, the double-tap Alt hook, the Settings window controller, reconciles the Run key and companion.json, wires
/// Settings into the Store (undo delay) and runs <c>--probe-notify</c> / <c>--request-notifications</c> / <c>--settings</c>.
/// W2's tray menu reaches Pause / Settings… through <see cref="Shared"/>.
/// </summary>
internal sealed class NotificationsInstaller : IDisposable
{
    public static NotificationsInstaller? Shared { get; private set; }

    public Notifier Notifier { get; }
    public SettingsController Settings { get; }
    public BreakpointSource Breakpoints { get; }
    public DoubleTapSource DoubleTap { get; } = new();

    private readonly Store store;
    private readonly DispatcherQueue dispatcher;
    private NotifyProbe? probe;

    /// <summary>Before any window (§4.4, C1): the handler, then <c>AppNotificationManager.Register()</c>.</summary>
    public static void RegisterNotifications() => Notifier.RegisterEarly();

    /// <summary>After the Store is created and before <c>Store.Start()</c>, so the first refetch seeds the policy.</summary>
    public static NotificationsInstaller Install(App app, Store store, DispatcherQueue dispatcher)
    {
        var i = new NotificationsInstaller(app, store, dispatcher);
        Shared = i;
        i.Start();
        return i;
    }

    private NotificationsInstaller(App app, Store store, DispatcherQueue dispatcher)
    {
        this.store = store;
        this.dispatcher = dispatcher;
        var model = SettingsModel.Shared;
        Notifier = new Notifier(store, dispatcher, model);
        Breakpoints = new BreakpointSource(dispatcher);
        Settings = new SettingsController(store, Notifier, DoubleTap);
        quit = app.Quit;
    }

    private readonly Action<int> quit;

    private void Start()
    {
        var model = SettingsModel.Shared;
        Log.Write($"settings: {model.Value.GetType().Name} loaded from {SettingsStore.DefaultPath} undoDelay={model.Value.UndoDelay} launchAtLogin={(model.Value.LaunchAtLogin ? "true" : "false")} doubleTapAlt={(model.Value.DoubleTapAlt ? "true" : "false")} notify={Notifier.Describe(model.Value.Notify)} keyOverrides={model.Value.Keys.Overrides.Count}");
        store.ApplySettings(model.Value);
        model.Changed += OnSettingsChanged;
        store.SettingsRequested += () => Settings.Show(activate: !W3Flags.AnyProbe);
        store.OpenItemRequested += id => Log.Write($"open requested item={id ?? "oldest"} (W2's panel opens here)");

        Notifier.Start();
        Breakpoints.Breakpoint += Notifier.Breakpoint;
        Breakpoints.Tick += Notifier.Tick;
        Breakpoints.Start();
        DoubleTap.DoubleTap += () =>
        {
            if (W3Flags.AnyProbe) return;
            Log.Write("double-tap Alt → toggle the panel (W2's panel toggles here)");
        };
        if (model.Value.DoubleTapAlt && !DoubleTap.Install()) Log.Write("double-tap Alt: on in settings but the hook could not be installed");

        LoginItem.Reconcile(model.Value);

        if (W3Flags.RequestNotifications)
        {
            // The one non-UI place the urgent consent may be raised — the user runs it (Swift: --request-notifications).
            Log.Write($"request-notifications: setting={Notifier.SettingText} urgentSupported={(Notifier.UrgentSupported ? "true" : "false")} optIn={(model.Value.Notify.TimeSensitiveAvailable ? "true" : "false")}");
            Notifier.RequestUrgentConsent();
        }
        if (W3Flags.ProbeNotify is { } seconds)
        {
            probe = new NotifyProbe(store, Notifier, Settings, Breakpoints, dispatcher, quit);
            probe.Run(seconds, W3Flags.Stay);
        }
        if (W3Flags.SettingsPage is { } page)
        {
            Settings.Show(activate: !W3Flags.AnyProbe, page: page.Length == 0 ? null : page);
        }
    }

    private void OnSettingsChanged(CompanionSettings old, CompanionSettings now)
    {
        if (old.UndoDelay != now.UndoDelay) store.ApplySettings(now);
        if (old.LaunchAtLogin != now.LaunchAtLogin) LoginItem.Apply(now.LaunchAtLogin, now);
        if (old.DoubleTapAlt != now.DoubleTapAlt && !now.DoubleTapAlt) DoubleTap.Uninstall();
        if (old.Keys != now.Keys) Log.Write("settings: key table changed (W2's Hotkeys re-applies the global rows here)");
        if (old.RailSide != now.RailSide || old.RailDisplay != now.RailDisplay || old.RailPresence != now.RailPresence) Log.Write("settings: rail settings changed (W2's rail re-lays out here)");
    }

    /// <summary>At exit: hooks off, the COM activator unregistered (a later click launches a new process).</summary>
    public void Dispose()
    {
        Breakpoints.Dispose();
        DoubleTap.Dispose();
        Notifier.Dispose();
        Notifier.UnregisterAtExit();
        if (Shared == this) Shared = null;
    }
}
