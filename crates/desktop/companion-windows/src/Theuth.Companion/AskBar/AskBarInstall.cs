using Microsoft.UI.Dispatching;
using Theuth.Companion.Core;
using Theuth.Companion.Voice;

namespace Theuth.Companion.AskBar;

/// <summary>
/// W4's start (the marked line in <c>App.OnLaunched</c>; Swift <c>AppDelegate.installAskBar</c>): the Ask bar window and
/// its model over the Store, then the probes — <c>--ask-probe N</c>, <c>--mic-probe N</c>, <c>--ask-replay</c> — or
/// <c>--ask</c> (open at launch). W2 reaches the bar through <see cref="Bar"/>: <see cref="AskBarWindow.HotkeyDown"/> /
/// <see cref="AskBarWindow.HotkeyUp"/> for the hold-to-talk half of Ctrl+Shift+Space; Ctrl+L in the panel already arrives
/// through <c>Store.AskBarRequested</c>.
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
            automated: AskFlags.Automated);
        bar.Bind(model);
        var install = new AskBarInstall(bar, model);
        if (AskFlags.MicProbe is { } mic) MicProbe.Run(dispatcher, mic, AskFlags.MicMixFormat, quit);
        else if (AskFlags.Probe is { } seconds) AskProbe.Run(seconds, store, install, dispatcher, quit);
        else if (AskFlags.Ask) model.Open();
        return install;
    }
}
