using Microsoft.UI.Dispatching;
using Microsoft.UI.Xaml;
using Microsoft.Windows.AppNotifications;

namespace Theuth.Companion;

/// <summary>
/// The companion process (design 134 §5.1, §5.5). W1: logging, crash reports, the alive mutex, the log window
/// (<c>AutomationId="log"</c>) and the engine probe; <c>--probe N</c> connects, logs the counts and exits 0 after N s;
/// <c>--register-smoke</c> checks <c>AppNotificationManager.Register()</c> for build.ps1. Later waves hook in where marked.
/// </summary>
public partial class App : Application
{
    /// <summary>The value <see cref="Program.Main"/> returns.</summary>
    public static int ExitCode { get; set; }

    private DispatcherQueue? dispatcher;
    private LogWindow? logWindow;
    private EngineProbe? probe;
    private Notifications.NotificationsInstaller? w3; // W3

    /// <summary>The engine state every surface renders (design 134 §5.4); created in <see cref="OnLaunched"/>, UI thread only.</summary>
    public Store? Store { get; private set; }
    private Mutex? aliveMutex;
    private bool quitting;

    public App()
    {
        InitializeComponent();
        CrashReport.Install(this);
    }

    protected override void OnLaunched(LaunchActivatedEventArgs args)
    {
        dispatcher = DispatcherQueue.GetForCurrentThread();
        Log.Start(dispatcher, toStderr: Flags.Probe is not null || Flags.RegisterSmoke, probeFile: Flags.Probe is not null);
        Log.Write($"launch exe={Environment.ProcessPath} version={AppInfo.Version} probe={Flags.Describe(Flags.Probe)} shareable={Flags.Shareable.ToString().ToLowerInvariant()} background={Flags.Background.ToString().ToLowerInvariant()}");
        Log.Write($"runtime {AppInfo.WinUI}; .NET {Environment.Version}; {System.Runtime.InteropServices.RuntimeInformation.OSDescription}; log {Log.DailyFile}");

        if (Flags.RegisterSmoke)
        {
            RunRegisterSmoke();
            return;
        }

        // §5.5 / D10: the Tauri shell's cheap alive check (W5's companion/windows.rs).
        aliveMutex = new Mutex(initiallyOwned: true, AppInfo.AliveMutexName, out bool createdNew);
        if (!createdNew) Log.Write($"another companion holds {AppInfo.AliveMutexName}");

        // W3: AppNotificationManager.Register() (Notifier) runs here — before any window (§4.4, C1).
        Notifications.NotificationsInstaller.RegisterNotifications(); // W3
        // W2: the log window is the probe's only (the W1 gate reads AutomationId "log"); otherwise the surfaces are the UI.
        if (Flags.Probe is not null)
        {
            logWindow = new LogWindow();
            logWindow.Closed += (_, _) => Quit(ExitCode);
            logWindow.ShowWithoutActivating(); // a probe never takes the foreground
        }
        Store = Store.Create(dispatcher, EngineProbe.Locate);
        probe = new EngineProbe(Store, dispatcher, Flags.Probe, Quit);
        probe.Start();
        // W2: the rail, panel, tray and hotkeys bind to Store here (before Start, to see the first refetch).
        Ui.Surfaces.Start(Store, dispatcher, Quit);
        // W3: notifier (policy on the Store's items), breakpoint sources, Settings window, Run key, --probe-notify — after the
        // surfaces (it wires into Ui.Surfaces.Current's hooks), before Start (to see the first refetch).
        w3 = Notifications.NotificationsInstaller.Install(this, Store, dispatcher); // W3
        Store.Start();
    }

    /// <summary>Ends the process with <paramref name="code"/> (on the UI thread).</summary>
    public void Quit(int code)
    {
        if (quitting) return;
        quitting = true;
        ExitCode = code;
        probe?.Dispose();
        w3?.Dispose(); // W3: hooks off, AppNotificationManager.Unregister()
        Ui.Surfaces.Current?.Dispose(); // W2: tray icon removed, hotkeys released
        Store?.Close();
        Log.Write($"terminate (exit {code})");
        try { aliveMutex?.ReleaseMutex(); } catch (ApplicationException) { }
        aliveMutex?.Dispose();
        Exit();
    }

    /// <summary>
    /// build.ps1's smoke (§5.2, §5.7): with the five runtime-MSIX files beside the exe, <c>Register()</c> must succeed on
    /// the trimmed 2.5.1 set (0x8007007E without them, spike S2/A1). The registration is removed again (<c>UnregisterAll</c>):
    /// a build folder must not leave an AUMID behind. W3's Notifier owns the real registration.
    /// </summary>
    private void RunRegisterSmoke()
    {
        int code;
        try
        {
            Log.Write($"register-smoke IsSupported={AppNotificationManager.IsSupported()}");
            var manager = AppNotificationManager.Default;
            manager.Register();
            Log.Write($"register-smoke Register() ok Setting={manager.Setting}");
            manager.UnregisterAll();
            Log.Write("register-smoke UnregisterAll() ok");
            code = 0;
        }
        catch (Exception e)
        {
            Log.Write($"register-smoke Register() FAILED hr=0x{e.HResult:X8} {e.GetType().Name}: {e.Message}");
            code = 1;
        }
        dispatcher!.TryEnqueue(() => Quit(code));
    }
}