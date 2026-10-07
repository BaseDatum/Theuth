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
        // W2: single instance goes here, before any XAML — AppInstance.FindOrRegisterForKey("io.theuth.companion"); when it
        // is not current, RedirectActivationToAsync(AppInstance.GetCurrent().GetActivatedEventArgs()) and return 0 (§5.5, A9).
        Application.Start(_ =>
        {
            var context = new DispatcherQueueSynchronizationContext(DispatcherQueue.GetForCurrentThread());
            SynchronizationContext.SetSynchronizationContext(context);
            _ = new App();
        });
        return App.ExitCode;
    }
}
