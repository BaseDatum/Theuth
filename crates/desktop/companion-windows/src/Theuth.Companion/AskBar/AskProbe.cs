using Microsoft.UI.Dispatching;
using Theuth.Companion.Core;
using Theuth.Companion.Rpc;

namespace Theuth.Companion.AskBar;

/// <summary>
/// <c>--ask-probe N [--synthetic-dictation | --synthetic-speech "text"] [--ask-replay]</c> (design 134 §7 W4; Swift
/// <c>AskProbe</c>): the Ask bar on screen, theuth's desk resolved through <c>mesh_status</c> + <c>desk_open("theuth")</c>
/// without sending, every step to <c>logs\ask-probe.log</c>, quit after N s with exit 0. The synthetic dictation streams
/// 2 s of a 440 Hz tone (or the system voice reading <c>text</c>, 16 kHz mono i16 after <see cref="AskAudio.Pcm16Mono16k"/>)
/// through <c>dictation_start/audio/stop</c> and logs every <c>Dictation*</c> event; the field fills with the transcript.
/// Unlike the Swift probe the bar is activated (the <c>windows-build-b</c> harness types into it) and a typed Enter sends.
/// </summary>
internal static class AskProbe
{
    /// <summary>A desk reply in the engine's wire shapes (<c>AgentEvent::TextDelta</c> / <c>TurnFinished</c>, crates/agent/src/lib.rs); no model answers on the scratch engine, so the words are a fixture.</summary>
    public static readonly string[] RecordedReply =
    {
        """{"type":"TextDelta","session":"replay-desk","turn":"t-replay","text":"Handed to w3 — "}""",
        """{"type":"TextDelta","session":"replay-desk","turn":"t-replay","text":"the docs ship tonight.\n"}""",
        """{"type":"TextDelta","session":"replay-desk","turn":"t-replay","text":"Then the release notes; nothing else is waiting.\n\n- rail: done\n- panel: done\n"}""",
        """{"type":"TurnFinished","session":"replay-desk","turn":"t-replay","stop_reason":"end_turn","usage":{"input":812,"output":41},"context":853}""",
    };

    public static void Run(double seconds, Store store, AskBarInstall install, DispatcherQueue dispatcher, Action<int> quit)
    {
        AskLog.Start();
        AskLog.Write($"ask-probe start seconds={Flags.Describe(seconds)} synthetic-dictation={Word(AskFlags.SyntheticDictation)} synthetic-speech={(AskFlags.SyntheticSpeech is null ? "no" : "\"" + AskFlags.SyntheticSpeech + "\"")} replay={Word(AskFlags.Replay)} shareable={Word(Flags.Shareable)} exe={Environment.ProcessPath}");
        bool resolved = false;
        store.PropertyChanged += (_, e) =>
        {
            if (e.PropertyName != nameof(StoreModel.Connection) || store.Connection != RpcClient.State.Open || resolved) return;
            resolved = true;
            _ = ResolveAsync(store, install);
        };
        store.DictationWire += line => AskLog.Write($"event {line}");
        install.Model.StartRefused += why => AskLog.Write($"dictation_start refused: {why} → state error={store.Ask.Dictation.Error ?? "nil"} phase={store.Ask.Dictation.Phase.Kind}");
        After(dispatcher, 1.5, () =>
        {
            install.Model.Open(); // activated: the harness types into ask-field (the Swift probe showed the bar without key)
            var w = install.Bar.AppWindow;
            AskLog.Write($"ask-probe bar shown activated=true visible={Word(w.IsVisible)} pos={w.Position.X},{w.Position.Y} size={w.Size.Width}x{w.Size.Height} foreground={(install.Bar.IsForeground ? "ask-bar" : "other")}");
            if (AskFlags.Replay) _ = Replay(store);
        });
        After(dispatcher, seconds, () =>
        {
            var d = store.Ask.Dictation;
            AskLog.Write($"ask-probe done after {Flags.Describe(seconds)} s — quitting (connected={Word(store.Connection == RpcClient.State.Open)} receipts={store.Ask.Receipts.Count} field=\"{install.Model.FieldText}\" dictation phase={d.Phase.Kind} liveText=\"{d.LiveText}\" error={d.Error ?? "nil"} receipt={Describe(store.Receipt)}) exit 0");
            store.Close();
            After(dispatcher, 2, () => quit(0)); // two seconds with the last state on screen for a UIA read
        });
    }

    private static async Task ResolveAsync(Store store, AskBarInstall install)
    {
        try
        {
            var desk = await store.ResolveDesk();
            var hub = store.Ask.Router.Desk is AskDesk.Ready r ? Word(r.Hub) : "?";
            AskLog.Write($"ask-probe desk={desk} this-engine-is-hub={hub} (nothing sent)");
        }
        catch (Exception e)
        {
            AskLog.Write($"ask-probe desk unresolved: {(e is RpcException r ? r.ToString() : e.Message)}");
        }
        if (AskFlags.SyntheticDictation) RunTone(store, install);
        else if (AskFlags.SyntheticSpeech is { } text) await RunSpeech(store, install, text);
    }

    // MARK: --synthetic-dictation (W6): 2 s of a 440 Hz tone through dictation_start/audio/stop, every event logged

    private static void RunTone(Store store, AskBarInstall install)
    {
        AskLog.Write("synthetic-dictation start: 2 s of 440 Hz at 16 kHz, 100 ms chunks, microphone untouched, nothing sent");
        install.Model.ProbeTone(2, () => AfterStop(store, install));
    }

    /// <summary><c>--synthetic-speech "text"</c>: the system voice reads the text (a WAV from <c>SpeechSynthesizer</c>) and the same path carries it.</summary>
    private static async Task RunSpeech(Store store, AskBarInstall install, string text)
    {
        short[] pcm;
        try
        {
            using var synth = new Windows.Media.SpeechSynthesis.SpeechSynthesizer();
            var stream = await synth.SynthesizeTextToStreamAsync(text);
            var bytes = new byte[(int)stream.Size];
            using (var reader = new Windows.Storage.Streams.DataReader(stream.GetInputStreamAt(0)))
            {
                await reader.LoadAsync((uint)bytes.Length);
                reader.ReadBytes(bytes);
            }
            var wav = AskAudio.ParseWav(bytes);
            pcm = AskAudio.Pcm16Mono16k(bytes) ?? throw new InvalidOperationException($"unreadable WAV ({stream.ContentType}, {bytes.Length} bytes)");
            AskLog.Write($"synthetic-speech start: voice \"{synth.Voice.DisplayName}\" ({synth.Voice.Language}) said \"{text}\" → {stream.ContentType} {bytes.Length} bytes {(wav is null ? "?" : $"{wav.SampleRate} Hz/{wav.Channels} ch/{wav.BitsPerSample}-bit")} → {pcm.Length} samples at 16 kHz ({pcm.Length / 16000.0:F2} s, rms {AskAudio.Rms(pcm):F3}), {(pcm.Length + AskAudio.ChunkSamples - 1) / AskAudio.ChunkSamples} chunks; microphone untouched, nothing sent");
        }
        catch (Exception e)
        {
            AskLog.Write($"synthetic-speech: synthesizer failed ({e.GetType().Name}: {e.Message}) — falling back to the 440 Hz tone");
            RunTone(store, install);
            return;
        }
        install.Model.ProbeSamples(pcm, () => AfterStop(store, install));
    }

    private static void AfterStop(Store store, AskBarInstall install)
    {
        AskLog.Write("stop sent; waiting for DictationFinished");
        After(store.Dispatcher, 3, () =>
        {
            var d = store.Ask.Dictation;
            AskLog.Write($"after stop: phase={d.Phase.Kind} utterances=[{string.Join(", ", d.Utterances.Select(u => $"\"{u.Text}\"{(u.Final ? " final" : "")}"))}] liveText=\"{d.LiveText}\" error={d.Error ?? "nil"} field=\"{install.Model.FieldText}\"");
        });
    }

    // MARK: --ask-replay: the recorded desk stream through the real path (the scratch engine has no model to answer)

    private static async Task Replay(Store store)
    {
        AskLog.Write($"ask-replay start: {RecordedReply.Length} recorded lines for session replay-desk, 400 ms apart");
        var queue = new Queue<string>(RecordedReply);
        var first = queue.Dequeue();
        store.ReplayDeskReply("replay-desk", "ship the docs", new[] { first });
        AskLog.Write($"ask-replay receipt: {Describe(store.Receipt)}");
        while (queue.Count > 0)
        {
            await Task.Delay(400);
            var line = queue.Dequeue();
            store.HandleEvent(System.Text.Encoding.UTF8.GetBytes(line));
            AskLog.Write($"ask-replay receipt: {Describe(store.Receipt)} finished={Word(store.Ask.Router.Reply?.Finished ?? false)}");
        }
    }

    private static string Describe(StoreModel.ReceiptRow? r) =>
        AskBarModel.ReceiptLines(r, 0) is { } l ? $"\"{l.Text}\" / \"{l.Trailing}\"" : "none";

    private static string Word(bool b) => b ? "true" : "false";

    private static void After(DispatcherQueue dispatcher, double seconds, Action action)
    {
        var timer = dispatcher.CreateTimer();
        timer.Interval = TimeSpan.FromSeconds(seconds);
        timer.IsRepeating = false;
        timer.Tick += (_, _) => action();
        timer.Start();
        Keep.Add(timer);
    }

    private static readonly List<DispatcherQueueTimer> Keep = new();
}
