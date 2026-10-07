using Microsoft.UI.Xaml;
using Microsoft.Windows.AppLifecycle;

namespace SpikeHost;

/// <summary>Entry point. The mode is chosen by the first "--xxx" command-line argument.</summary>
public partial class App : Application
{
    // Keep spike objects alive for the life of the process (they own P/Invoke delegates).
    private static readonly List<object> Keep = new();

    public App()
    {
        InitializeComponent();
        UnhandledException += (s, e) =>
        {
            SpikeLog.Append($"UNHANDLED hr=0x{e.Exception?.HResult:X8} {e.Exception}");
            e.Handled = true;
        };
    }

    protected override void OnLaunched(LaunchActivatedEventArgs args)
    {
        var argv = Environment.GetCommandLineArgs().Skip(1).ToArray();
        // "----AppNotificationActivated:" is what the toast COM activation passes; it is not a mode.
        var mode = argv.FirstOrDefault(a => a.StartsWith("--", StringComparison.Ordinal) && !a.StartsWith("----", StringComparison.Ordinal)) ?? "";
        var toastActivated = argv.Any(a => a.StartsWith("----AppNotificationActivated", StringComparison.Ordinal));
        if (toastActivated) mode = "--toast";

        try
        {
            switch (mode)
            {
                case "--rail": Keep.Add(new RailSpike(argv)); break;
                case "--toast": Keep.Add(new ToastSpike(argv, toastActivated)); break;
                case "--hotkey-tray": Keep.Add(new HotkeyTraySpike()); break;
                case "--signals": Keep.Add(new SignalsSpike()); break;
                default: Keep.Add(new SmokeSpike()); break;
            }
        }
        catch (Exception ex)
        {
            SpikeLog.Append($"MODE CTOR EXCEPTION hr=0x{ex.HResult:X8} {ex}");
            var w = new LogWindow("SpikeHost crashed in " + mode);
            w.Activate();
        }
    }
}
