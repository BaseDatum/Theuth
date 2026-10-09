using Microsoft.UI.Dispatching;
using Theuth.Companion.Core;

namespace Theuth.Companion.AnyApp;

/// <summary>
/// 153 W153D's start — the ONE hookup line in <c>App.OnLaunched</c> (after 149 WW3's island, whose voice view is the pill, before
/// <c>Store.Start()</c>): the any-app controller over the Store, its hidden window (WTS lock notifications + clipboard owner) and the
/// probes. Until W153C's PIN-C the dictate-anywhere key is not ours to register (COMMON seam): <c>--probe-anyapp*</c> flags and the
/// <c>--probe-anyapp-watch</c> file trigger drive the controller instead.
/// </summary>
internal static class AnyAppInstall
{
    public static AnyAppController? Install(Store store, DispatcherQueue dispatcher)
    {
        try
        {
            var settings = new AnyAppSettings(Settings.SettingsModel.Shared.Store);
            var controller = new AnyAppController(store, dispatcher, settings);
            AnyAppController.Current = controller;
            AppDomain.CurrentDomain.ProcessExit += (_, _) => Log.Write($"anyapp: exit — counters {controller.Counters}");
            AnyAppProbe.Install(controller, dispatcher);
            return controller;
        }
        catch (Exception e)
        {
            Log.Write($"anyapp: not installed — {e.GetType().Name}: {e.Message}");
            return null;
        }
    }
}
