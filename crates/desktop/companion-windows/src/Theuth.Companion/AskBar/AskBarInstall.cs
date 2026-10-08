using Microsoft.UI.Dispatching;
using Theuth.Companion.Core;
using Theuth.Companion.Voice;

namespace Theuth.Companion.AskBar;

/// <summary>
/// W4's start (the marked line in <c>App.OnLaunched</c>, after W2's <c>Ui.Surfaces.Start</c>; Swift <c>AppDelegate.installAskBar</c>):
/// the Ask bar window and its model over the Store, the hooks W2 left on <see cref="Ui.Surfaces.Current"/> —
/// <c>AskBarAvailable</c> (the hotkey opens the bar when nothing needs you), <c>Hotkeys.Pressed</c>/<c>Released</c> (the
/// hold-to-talk half of Ctrl+Shift+Space) — then the probes <c>--ask-probe N</c>, <c>--mic-probe N</c>, <c>--ask-replay</c>, or
/// <c>--ask</c> (open at launch). Ctrl+L in the panel and the tray's "Ask theuth…" arrive through <c>Store.AskBarRequested</c>.
/// </summary>
internal sealed class AskBarInstall
{
    public AskBarWindow Bar { get; }

    public AskBarModel Model { get; }

    private AskBarInstall(AskBarWindow bar, AskBarModel model)
    {
        Bar = bar;
        Model = model;
    }

    public static AskBarInstall Install(Store store, DispatcherQueue dispatcher, Action<int> quit)
    {
        store.AskProbe = AskFlags.Probe is not null || AskFlags.Replay;
        var bar = new AskBarWindow(store, dispatcher);
        var model = new AskBarModel(store, store.Dictation, bar, new DispatcherScheduler(dispatcher), () => DateTimeOffset.Now, Log.Write,
            keys: () => store.Keys, automated: AskFlags.Automated);
        bar.Bind(model);
        var install = new AskBarInstall(bar, model);
        if (Ui.Surfaces.Current is { } surfaces)
        {
            surfaces.AskBarAvailable = () => true;
            // W2's OnHotkey runs first: when nothing needs you it opens the bar through Store.AskBarRequested; the same press then
            // becomes the voice press here (a hold past 250 ms dictates straight into the fresh bar, a tap just opens it).
            var requested = DateTimeOffset.MinValue;
            store.AskBarRequested += () => requested = DateTimeOffset.Now;
            surfaces.Hotkeys.Pressed += action =>
            {
                if (action != KeyAction.OpenPanel || model.Automated) return;
                if (DateTimeOffset.Now - requested > TimeSpan.FromMilliseconds(100)) return; // the press went to the panel
                bar.HotkeyDown(store.Keys.ChordFor(KeyAction.OpenPanel)?.KeyCode ?? VirtualKeys.Space);
            };
            surfaces.Hotkeys.Released += (action, _) =>
            {
                if (action == KeyAction.OpenPanel) bar.HotkeyUp();
            };
            Log.Write("ask bar: installed (hotkey → bar when nothing needs you; Ctrl+L; tray menu)");
        }
        else
        {
            Log.Write("ask bar: installed without W2's surfaces (no hotkey)");
        }
        if (AskFlags.MicProbe is { } mic) MicProbe.Run(dispatcher, mic, AskFlags.MicMixFormat, quit);
        else if (AskFlags.Probe is { } seconds) AskProbe.Run(seconds, store, install, dispatcher, quit);
        else if (AskFlags.Ask) model.Open();
        return install;
    }
}
