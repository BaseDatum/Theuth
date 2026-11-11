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
internal static partial class AskProbe
{
    // 189w D4: the reply is markdown, cut where a stream cuts it — mid-mark ("**w3" … "**"), mid-line, between fence lines — so the replay
    // shows the unclosed marks closed while they stream, the held-back marker lines, and the blocks that stay put while one is written.
    private static readonly string[] Pieces =
    {
        "## Handed to **w3",
        "** — the docs ship tonight\n\nThe release notes are next; `crates/rpc` stays with w2.\n\n- rail: **done**\n- panel: `done`\n",
        "  - nested: see [the notes](https://example.com/notes)\n\n```rust\nfn ship() {\n    println!(\"docs\");\n}\n```\n\n",
        "1. then the notes\n2. then `snake_case_name`",
    };

    /// <summary>What the transcript holds for the replayed turn: the streamed text and one line the stream never delivered — the finish replaces the streamed text with this.</summary>
    public static readonly string RecordedFinalText = string.Concat(Pieces) + "\n\nNothing else is waiting.";

    /// <summary>The replayed desk's transcript as <c>load_session</c> returns it (the recorded desk exists on no engine).</summary>
    public static readonly SessionTail RecordedTranscript = new()
    {
        Running = false,
        History = new ValueList<TranscriptEntry>
        {
            new() { Role = "user", Text = "ship the docs", Turn = 12 },
            new() { Role = "tool" },
            new() { Role = "assistant", Text = RecordedFinalText, Turn = 12 },
        },
    };

    private static string Delta(string text) => $$"""{"type":"TextDelta","session":"replay-desk","turn":"t-replay","text":{{System.Text.Json.JsonSerializer.Serialize(text)}}}""";

    /// <summary>A desk reply in the engine's wire shapes (<c>AgentEvent::TextDelta</c> / <c>TurnFinished</c>, crates/agent/src/lib.rs); no model answers on the scratch engine, so the words are a fixture.</summary>
    public static readonly string[] RecordedReply =
    {
        Delta(Pieces[0]),
        Delta(Pieces[1]),
        Delta(Pieces[2]),
        Delta(Pieces[3]),
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
            if (AskFlags.Replay) _ = Replay(store, install);
            if (AskFlags.CatchUp) After(dispatcher, 1.5, () => _ = CatchUpProbe(store));
            if (AskFlags.Send is { } typed) After(dispatcher, 1.5, () => TypedSend(store, install, typed));
            if (AskFlags.Fixture is { } fx) AskFixtures.Run(fx, store, install, dispatcher); // 153 W153B
            if (AskFlags.Live is { } live) After(dispatcher, 2, () => AskFixtures.Live(live, store, install, dispatcher)); // 153 W153B (after the desk resolves)
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
        if (AskFlags.Press) install.Model.ProbeSpeechPress(pcm, () => AfterStop(store, install)); // 153 W153B: inside a voice press → SetText / SendNow
        else install.Model.ProbeSamples(pcm, () => AfterStop(store, install));
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

    private static async Task Replay(Store store, AskBarInstall install)
    {
        if (AskFlags.Pictures is { } picturesMode)
        {
            await ReplayPictures(store, install, picturesMode); // 192w P6: thinking, picture fences and a stub fence_snapshot
            return;
        }
        AskLog.Write($"ask-replay start: {RecordedReply.Length} recorded lines for session replay-desk, 450 ms apart (markdown reply, cut mid-mark; the transcript carries one more line)");
        install.Bar.ReplyRendered += line => AskLog.Write($"ask-replay markdown: {line}");
        store.ReplayTranscript("replay-desk", RecordedTranscript);
        var queue = new Queue<string>(RecordedReply);
        var first = queue.Dequeue();
        store.ReplayDeskReply("replay-desk", "ship the docs", new[] { first });
        AskLog.Write($"ask-replay receipt: {Describe(store.Receipt)}");
        int step = 1;
        while (queue.Count > 0)
        {
            await Task.Delay(450);
            var line = queue.Dequeue();
            store.HandleEvent(System.Text.Encoding.UTF8.GetBytes(line));
            step++;
            AskLog.Write($"ask-replay receipt: {Describe(store.Receipt)} finished={Word(store.Ask.Router.Reply?.Finished ?? false)}");
            // Evidence: the bar's own tree as a PNG after the second piece (a code span and a list streaming), after the fourth (a
            // fence closed, a numbered list open) and once the finish replaced the stream with the transcript.
            string? shot = step switch { 2 => "ask-replay-1-streaming", 4 => "ask-replay-2-streaming", 5 => "ask-replay-3-finished", _ => null };
            if (shot is not null)
            {
                await Task.Delay(300);
                var path = AskFlags.Png189(shot);
                var ok = await install.Bar.CapturePng(path);
                AskLog.Write($"ask-replay shot: {path} ok={Word(ok)}");
            }
        }
        var reply = store.Ask.Router.Reply;
        AskLog.Write($"ask-replay done: finished={Word(reply?.Finished ?? false)} streamed={reply?.StreamedChars} chars, final text {reply?.Text.Length} chars, replaced-by-transcript={Word(reply?.Text == RecordedFinalText)}");
    }

    // MARK: --ask-catchup: the real load_session (the scratch engine has no model, but its desk has a transcript)

    private static async Task CatchUpProbe(Store store)
    {
        try
        {
            var desk = await store.ResolveDesk();
            var tail = await store.Client.LoadSessionAsync(desk, AskCatchUp.Limit);
            var roles = string.Join(",", tail.History.Select(h => $"{h.Role}{(h.Turn is { } t ? "#" + t : "")}"));
            AskLog.Write($"ask-catchup: load_session({desk}, null, {AskCatchUp.Limit}) → {tail.History.Count} entries running={Word(tail.Running)} roles=[{roles}]");
            foreach (var h in tail.History.Where(h => h.Text is { Length: > 0 }).TakeLast(3))
            {
                var text = h.Text!.Replace('\n', ' ');
                AskLog.Write($"ask-catchup: {h.Role} turn={(h.Turn?.ToString() ?? "null")}: {(text.Length > 100 ? text[..100] + "…" : text)}");
            }
            var pick = AskCatchUp.Pick(tail.History, "", null);
            AskLog.Write($"ask-catchup: newest-assistant pick text={(pick.Text is null ? "none" : pick.Text.Length + " chars")} error={(pick.Error is null ? "none" : "\"" + pick.Error + "\"")}");
        }
        catch (Exception e)
        {
            AskLog.Write($"ask-catchup: failed: {(e is RpcException r ? r.ToString() : e.Message)}");
        }
    }

    /// <summary><c>--ask-send "text"</c>: a typed Enter through the Ask bar once the desk resolved — the focus claim, then <c>send_input</c>, then whatever the desk answers.</summary>
    private static void TypedSend(Store store, AskBarInstall install, string text)
    {
        AskLog.Write($"ask-send: typing \"{text}\" and pressing Enter (the 3 s hold, then desk_open → set_event_focus → send_input)");
        install.Model.FieldEdited(text);
        install.Model.Submit();
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