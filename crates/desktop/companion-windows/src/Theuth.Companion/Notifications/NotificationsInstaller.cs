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
    private PinTipScheduler? pinTip; // W6 item 1

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

        // W2's surfaces (Ui.Surfaces.Current, started just before us): the tray menu's Pause ▸ and Launch at login rows, the
        // panel for toast bodies and double-tap Alt, redirected toast activations.
        var ui = Ui.Surfaces.Current;
        if (ui is not null)
        {
            ui.PauseRequested += what => Notifier.Pause(what switch
            {
                "1h" => DateTimeOffset.Now.AddHours(1),
                "tomorrow" => Notifier.Tomorrow,
                _ => null,
            });
            ui.LaunchAtLoginToggled += on => model.Update(s => s.LaunchAtLogin = on);
            ui.LaunchAtLoginState = () => model.Value.LaunchAtLogin;
            ui.PausedUntil = () => Notifier.PausedUntil; // W6 item 9: the submenu's check marks
            ui.Redirected += Notifier.Redirected;
            store.OpenItemRequested += id => ui.OpenPanelOn(id);
        }
        else
        {
            store.OpenItemRequested += id => Log.Write($"open requested item={id ?? "oldest"} — no surfaces in this build");
        }

        Notifier.Start();
        Breakpoints.Breakpoint += Notifier.Breakpoint;
        Breakpoints.Tick += Notifier.Tick;
        Breakpoints.Start();
        DoubleTap.DoubleTap += () =>
        {
            if (W3Flags.AnyProbe) return;
            if (Ui.Surfaces.Current is { } s) s.TogglePanel();
            else Log.Write("double-tap Alt — no surfaces in this build");
        };
        if (model.Value.DoubleTapAlt && !DoubleTap.Install()) Log.Write("double-tap Alt: on in settings but the hook could not be installed");

        LoginItem.Reconcile(model.Value);

        // W6 item 1 (§4.5): the first-run pin tip — armed from the tray's first NIM_ADD, not during probes (they exit in seconds).
        if (!W3Flags.AnyProbe && ui is not null)
        {
            pinTip = new PinTipScheduler(Notifier, model, dispatcher, () => ui.TrayAddedAt);
            pinTip.Start();
        }

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
        if (old.LaunchAtLogin != now.LaunchAtLogin) LoginItem.Apply(now.LaunchAtLogin, now);
        if (old.DoubleTapAlt != now.DoubleTapAlt && !now.DoubleTapAlt) DoubleTap.Uninstall();
        bool surfaces = old.Keys != now.Keys || old.UndoDelay != now.UndoDelay
            || old.RailSide != now.RailSide || old.RailDisplay != now.RailDisplay || old.RailPresence != now.RailPresence;
        if (!surfaces) return;
        // W2 applies keys (hotkeys + grammar + legend), the undo delay and the rail live; without surfaces the Store gets the delay.
        if (Ui.Surfaces.Current is { } ui) ui.ApplySettings(now);
        else store.ApplySettings(now);
        Log.Write($"settings: applied live (keys={(old.Keys != now.Keys ? "changed" : "same")} undo={now.UndoDelay} rail={now.RailPresence.RawValue()} {now.RailSide})");
    }

    /// <summary>At exit: hooks off, the COM activator unregistered (a later click launches a new process).</summary>
    public void Dispose()
    {
        pinTip?.Dispose();
        Breakpoints.Dispose();
        DoubleTap.Dispose();
        Notifier.Dispose();
        Notifier.UnregisterAtExit();
        if (Shared == this) Shared = null;
    }
}