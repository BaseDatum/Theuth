using Microsoft.UI.Dispatching;
using Theuth.Companion.Core;
using Theuth.Companion.Voice;

namespace Theuth.Companion.AskBar;

/// <summary>
/// W4's start (the marked line in <c>App.OnLaunched</c>, after W2's <c>Ui.Surfaces.Start</c>; Swift <c>AppDelegate.installAskBar</c>):
/// the Ask bar window and its model over the Store, the hooks W2 left on <see cref="Ui.Surfaces.Current"/> —
/// <c>AskBarAvailable</c> (the Ask-bar key opens the bar), <c>Hotkeys.Pressed</c>/<c>Released</c> (the
/// hold-to-talk half of the panel key, ruling 2) — then the probes <c>--ask-probe N</c>, <c>--mic-probe N</c>, <c>--ask-replay</c>, or
/// <c>--ask</c> (open at launch). Ctrl+L in the panel and the tray's "Ask theuth…" arrive through <c>Store.AskBarRequested</c>.
/// </summary>
internal sealed class AskBarInstall
{
    /// <summary>A panel-key press held this long is a talk, not a tap (the Ask bar's own 0.25 s press rule starts from here).</summary>
    public const int HoldToTalkMs = 250;

    public AskBarWindow Bar { get; }

    public AskBarModel Model { get; }

    /// <summary>153 W153B: the dictionary suggestion card (§6.4), anchored under the Ask bar while it is on screen.</summary>
    public Ui.SuggestionCardWindow? Suggestions { get; private set; }

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
            keys: () => store.Keys, automated: AskFlags.Automated,
            micOnKeyDown: () => Settings.SettingsModel.Shared.Dictation.MicOnKeyDown); // 153 §8.5
        bar.Bind(model);
        var install = new AskBarInstall(bar, model);
        // 153 §8.1: the companion's recorder follows the engine's voice.input_devices — read at every connect.
        store.PropertyChanged += (_, e) =>
        {
            if (e.PropertyName == nameof(StoreModel.Connection) && store.Connection == Rpc.RpcClient.State.Open) _ = RefreshMicPreference(store);
        };
        try
        {
            install.Suggestions = Ui.SuggestionCardWindow.Install(store, () => bar.IsShown ? bar.ScreenRect : null); // 153 §6.4
        }
        catch (Exception e)
        {
            Log.Write($"suggestion card: install failed: {e.GetType().Name}: {e.Message}");
        }
        store.InstallSpokenReplies(() => Settings.SettingsModel.Shared.Value.AskSpeak); // spoken replies: an Ask reply is read aloud once final when the switch is on
        if (Ui.Surfaces.Current is { } surfaces)
        {
            surfaces.AskBarAvailable = () => true;
            // Ruling 2 (Sky 2026-10-09 16:02): a TAP of the panel key opens the Attention panel (W2's OnHotkey, always); the Ask bar no longer
            // rides on it. "The hold still talks": a press still held after HoldToTalkMs becomes the bar's voice press — the bar opens
            // over the panel and dictates to theuth; the release sends. The Ask-bar key opens the bar outright (Store.AskBarRequested).
            DispatcherQueueTimer? hold = null;
            surfaces.Hotkeys.Pressed += action =>
            {
                if (model.Automated) return;
                if (action == KeyAction.OpenAskBar)
                {
                    // A press on the Ask-bar key: the bar is open now (OnHotkey ran first) — a hold dictates into it as before.
                    bar.HotkeyDown(store.Keys.ChordFor(KeyAction.OpenAskBar)?.KeyCode ?? VirtualKeys.Space);
                    return;
                }
                if (action != KeyAction.OpenPanel) return;
                hold?.Stop();
                var t = hold = dispatcher.CreateTimer();
                t.Interval = TimeSpan.FromMilliseconds(HoldToTalkMs);
                t.IsRepeating = false;
                t.Tick += (_, _) =>
                {
                    t.Stop();
                    if (hold != t) return;
                    hold = null;
                    Log.Write($"ask bar: the panel key is still held after {HoldToTalkMs} ms — talking");
                    bar.HotkeyDown(store.Keys.ChordFor(KeyAction.OpenPanel)?.KeyCode ?? VirtualKeys.Space);
                };
                t.Start();
            };
            surfaces.Hotkeys.Released += (action, _) =>
            {
                if (action is KeyAction.OpenPanel or KeyAction.OpenAskBar)
                {
                    hold?.Stop();
                    hold = null;
                    bar.HotkeyUp();
                }
            };
            Log.Write("ask bar: installed (the Ask-bar key opens it; the panel key opens the panel, held ≥ 250 ms it talks; Ctrl+L; tray menu)");
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

    /// <summary>153 §8.1: <c>voice_status.input_devices</c> → <see cref="MicPreference.Names"/>; an engine before 153 leaves the default.</summary>
    private static async Task RefreshMicPreference(Store store)
    {
        try
        {
            var names = (await store.Client.VoiceStatusAsync()).InputDevices;
            MicPreference.Names = names;
            Log.Write($"ask bar: microphone preference from the engine: {(names.Count == 0 ? "system default" : string.Join(" → ", names))}");
        }
        catch (Exception e)
        {
            Log.Write($"ask bar: voice_status failed ({e.Message}) — the recorder uses the system default");
        }
    }
}
