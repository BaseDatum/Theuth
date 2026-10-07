using Microsoft.UI.Xaml;
using Microsoft.UI.Xaml.Automation;
using Microsoft.UI.Xaml.Controls;
using Microsoft.Windows.AppLifecycle;
using Microsoft.Windows.AppNotifications;
using Microsoft.Windows.AppNotifications.Builder;

namespace SpikeHost;

/// <summary>
/// S2: app notifications (toasts) from an unpackaged, self-contained app.
/// Order per Microsoft Learn: subscribe NotificationInvoked -> Register() -> GetActivatedEventArgs() -> Show().
/// For an unpackaged app Register() writes the AUMID + COM activator (CLSID -> this exe with
/// "----AppNotificationActivated:") under HKCU\Software\Classes and sets the process AUMID.
/// </summary>
internal sealed class ToastSpike
{
    private readonly LogWindow _log;
    private readonly AppNotificationManager _mgr;

    public ToastSpike(string[] argv, bool activatedByToast)
    {
        SpikeLog.Init("toast");
        SpikeLog.Append($"S2 activatedByToast(commandline)={activatedByToast}");

        var again = new Button { Content = "Show toasts again" };
        AutomationProperties.SetAutomationId(again, "ToastAgain");
        var unreg = new Button { Content = "UnregisterAll" };
        AutomationProperties.SetAutomationId(unreg, "ToastUnregister");
        var header = new StackPanel { Orientation = Orientation.Horizontal, Spacing = 12, Margin = new Thickness(12) };
        header.Children.Add(again);
        header.Children.Add(unreg);

        _log = new LogWindow("SpikeHost S2", header);
        Ui.Place(_log, 900, 620);
        _log.Activate();

        _mgr = AppNotificationManager.Default;
        again.Click += (s, e) => ShowToasts();
        unreg.Click += (s, e) =>
        {
            try { _mgr.UnregisterAll(); SpikeLog.Append("UnregisterAll() ok"); }
            catch (Exception ex) { LogEx("UnregisterAll", ex); }
        };

        SpikeLog.Append($"AUMID before Register: {Native.ProcessAumid()}");
        try { SpikeLog.Append($"AppNotificationManager.IsSupported() = {AppNotificationManager.IsSupported()}"); }
        catch (Exception ex) { LogEx("IsSupported", ex); }

        // 1. handler first, so an activation that launched us is not lost
        _mgr.NotificationInvoked += OnInvoked;

        // 2. register
        bool registered = false;
        try
        {
            _mgr.Register();
            registered = true;
            SpikeLog.Append("Register() ok");
        }
        catch (Exception ex) { LogEx("Register", ex); }
        SpikeLog.Append($"AUMID after Register: {Native.ProcessAumid()}");
        try { SpikeLog.Append($"AppNotificationManager.Setting = {_mgr.Setting}"); }
        catch (Exception ex) { LogEx("Setting", ex); }

        // 3. how were we activated?
        try
        {
            var act = AppInstance.GetCurrent().GetActivatedEventArgs();
            SpikeLog.Append($"GetActivatedEventArgs().Kind = {act.Kind}");
            if (act.Kind == ExtendedActivationKind.AppNotification && act.Data is AppNotificationActivatedEventArgs a)
                SpikeLog.Append($"  activation data: argument='{a.Argument}' {Fmt(a.Arguments)} input={Fmt(a.UserInput)}");
        }
        catch (Exception ex) { LogEx("GetActivatedEventArgs", ex); }

        // 4. show
        if (registered && !activatedByToast) ShowToasts();
    }

    private void ShowToasts()
    {
        try
        {
            var normal = new AppNotificationBuilder()
                .AddArgument("action", "body")
                .AddArgument("spike", "s2-normal")
                .AddText("SpikeHost S2: approval needed")
                .AddText("Subagent wants to run `cargo publish`. Approve?")
                .AddButton(new AppNotificationButton("Approve").AddArgument("action", "approve").AddArgument("spike", "s2-normal"))
                .AddButton(new AppNotificationButton("Deny").AddArgument("action", "deny").AddArgument("spike", "s2-normal"))
                .BuildNotification();
            SpikeLog.Append("normal payload: " + Compact(normal.Payload));
            _mgr.Show(normal);
            SpikeLog.Append($"Show(normal) ok id={normal.Id}");
        }
        catch (Exception ex) { LogEx("Show(normal)", ex); }

        try
        {
            bool urgentOk = AppNotificationBuilder.IsUrgentScenarioSupported();
            SpikeLog.Append($"AppNotificationBuilder.IsUrgentScenarioSupported() = {urgentOk}");
            var b = new AppNotificationBuilder()
                .AddArgument("action", "body")
                .AddArgument("spike", "s2-urgent")
                .AddText("SpikeHost S2: URGENT")
                .AddText("Scenario=Urgent: should break through focus assist.")
                .AddButton(new AppNotificationButton("Approve").AddArgument("action", "approve").AddArgument("spike", "s2-urgent"))
                .AddButton(new AppNotificationButton("Deny").AddArgument("action", "deny").AddArgument("spike", "s2-urgent"));
            if (urgentOk) b = b.SetScenario(AppNotificationScenario.Urgent);
            var urgent = b.BuildNotification();
            SpikeLog.Append("urgent payload: " + Compact(urgent.Payload));
            _mgr.Show(urgent);
            SpikeLog.Append($"Show(urgent) ok id={urgent.Id}");
        }
        catch (Exception ex) { LogEx("Show(urgent)", ex); }
    }

    private void OnInvoked(AppNotificationManager sender, AppNotificationActivatedEventArgs args)
    {
        SpikeLog.Append($"NotificationInvoked thread={Environment.CurrentManagedThreadId} argument='{args.Argument}' {Fmt(args.Arguments)} input={Fmt(args.UserInput)}");
        _log.DispatcherQueue.TryEnqueue(() =>
        {
            try { _log.Activate(); } catch { }
        });
    }

    private static string Fmt(IDictionary<string, string>? d)
        => d is null ? "null" : "{" + string.Join(", ", d.Select(kv => kv.Key + "=" + kv.Value)) + "}";

    private static string Compact(string xml) => xml.Replace("\r", "").Replace("\n", "");

    private static void LogEx(string what, Exception ex)
        => SpikeLog.Append($"{what} EXCEPTION hr=0x{ex.HResult:X8} {ex.GetType().Name}: {ex.Message}");
}
