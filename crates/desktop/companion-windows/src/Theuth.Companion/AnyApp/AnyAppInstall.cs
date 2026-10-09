using Microsoft.UI.Dispatching;
using Theuth.Companion.Core;

namespace Theuth.Companion.AnyApp;

/// <summary>
/// 153 W153D's start — the ONE hookup line in <c>App.OnLaunched</c> (after 149 WW3's island, whose voice view is the pill, before
/// <c>Store.Start()</c>): the any-app controller over the Store, its hidden window (WTS lock notifications + clipboard owner) and the
/// probes. The dictate-anywhere key is W153C's (PIN-C): its gestures arrive through <c>Hotkeys.DictateAnywhere</c> and the words of a
/// switch out of dictation leave through <c>Hotkeys.TakeDictateWords</c>; the <c>--probe-anyapp*</c> flags and the <c>--probe-anyapp-watch</c>
/// file trigger still drive the same controller without a key or a voice.
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
            // PIN-C: the dictate-anywhere key is W153C's (RegisterHotKey + the one PointMachine in PointController); its gestures arrive here.
            if (Ui.Surfaces.Current?.Hotkeys is { } hotkeys)
            {
                hotkeys.DictateAnywhere += controller.OnGesture;
                hotkeys.TakeDictateWords = controller.TakeWords;
                Log.Write("anyapp: subscribed to Hotkeys.DictateAnywhere and answering TakeDictateWords (PIN-C)");
            }
            else Log.Write("anyapp: no hotkey layer (no W2 surfaces) — the probes only");
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
