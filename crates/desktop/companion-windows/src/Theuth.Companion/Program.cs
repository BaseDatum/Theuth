using Microsoft.UI.Dispatching;
using Microsoft.UI.Xaml;

namespace Theuth.Companion;

/// <summary>
/// The entry point (the XAML-generated Main is off: <c>DISABLE_XAML_GENERATED_MAIN</c>). Main returns
/// <see cref="App.ExitCode"/> — 0 always (design 134 §5.5) except a failed <c>--register-smoke</c>.
/// </summary>
public static class Program
{
    [STAThread]
    public static int Main(string[] args)
    {
        WinRT.ComWrappersSupport.InitializeComWrappers();
        // W2: single instance, before any XAML (§5.5, A9) — a second launch redirects its activation to the running companion,
        // lets it take the foreground and exits 0 (Ui/SingleInstance.cs).
        if (!Ui.SingleInstance.Claim(args)) return 0;
        Application.Start(p =>
        {
            var context = new DispatcherQueueSynchronizationContext(DispatcherQueue.GetForCurrentThread());
            SynchronizationContext.SetSynchronizationContext(context);
            new App();
        });
        return App.ExitCode;
    }
}
