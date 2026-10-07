using System.Text;

namespace Theuth.Companion.Core.Tests;

/// <summary>Records the dictation calls in order; the start can be held open or made to fail.</summary>
internal sealed class FakeDictationCalls : IDictationCalls
{
    public readonly List<string> Calls = new();
    public readonly List<short[]> Audio = new();
    public TaskCompletionSource<string>? HoldStart;
    public Exception? StartFails;
    public int Starts;

    public Task PrewarmAsync()
    {
        Calls.Add("prewarm");
        return Task.CompletedTask;
    }

    public Task<string> StartAsync()
    {
        Calls.Add("start");
        Starts++;
        if (StartFails is { } e) return Task.FromException<string>(e);
        return HoldStart?.Task ?? Task.FromResult($"d-{Starts}");
    }

    public Task AudioAsync(string id, short[] pcm16)
    {
        Calls.Add($"audio {id} {pcm16.Length}");
        Audio.Add(pcm16);
        return Task.CompletedTask;
    }

    public Task StopAsync(string id)
    {
        Calls.Add($"stop {id}");
        return Task.CompletedTask;
    }
}

/// <summary>The window and the microphone as the model sees them.</summary>
internal sealed class FakeSurface : IAskBarSurface
{
    public readonly List<string> Events = new();
    public string? MicRefusal;
    public bool MicOpen;

    public void Show(bool activate) => Events.Add(activate ? "show+activate" : "show");

    public void Hide(bool returnFocus) => Events.Add(returnFocus ? "hide+return" : "hide");

    public Task<string?> StartMicrophoneAsync()
    {
        Events.Add("mic start");
        if (MicRefusal is null) MicOpen = true;
        return Task.FromResult(MicRefusal);
    }

    public void StopMicrophone()
    {
        Events.Add("mic stop");
        MicOpen = false;
    }

    public void CancelMicrophone()
    {
        Events.Add("mic cancel");
        MicOpen = false;
    }

    public void OpenUrl(Uri url) => Events.Add($"open {url.OriginalString}");
}

/// <summary>
/// The Ask bar's controller (Swift <c>AskBarController</c>) on virtual time: hold-to-talk, the ordered dictation
/// calls, the live field, the probes, the Ask-bar key scope and the receipt row (design 134 §4.3, §7 W4).
/// </summary>
public sealed class AskBarModelTests
{
    private readonly ManualScheduler clock = new();
    private readonly FakeEngine engine = new();
    private readonly FakeDictationCalls calls = new();
    private readonly FakeSurface surface = new();
    private readonly List<string> logs = new();
    private readonly StoreModel store;
    private readonly AskBarModel bar;

    public AskBarModelTests()
    {
        store = new StoreModel(engine, clock, () => clock.Now, logs.Add);
        bar = new AskBarModel(store, calls, surface, clock, () => clock.Now, logs.Add);
    }

    private static byte[] Json(string s) => Encoding.UTF8.GetBytes(s);

    private void Dictation(string id, string text, bool final) =>
        store.HandleEvent(Json("{\"type\":\"DictationText\",\"id\":\"" + id + "\",\"utterance\":0,\"text\":\"" + text + "\",\"final\":" + (final ? "true" : "false") + "}"));

    private void Finished(string id, string? error = null) =>
        store.HandleEvent(Json(error is null
            ? "{\"type\":\"DictationFinished\",\"id\":\"" + id + "\",\"reason\":{\"reason\":\"done\"}}"
            : "{\"type\":\"DictationFinished\",\"id\":\"" + id + "\",\"reason\":{\"reason\":\"error\",\"message\":\"" + error + "\"}}"));

    private static KeyPress Key(string chars, ushort vk = 0, ChordModifiers mods = ChordModifiers.None) => new(chars, vk, mods);

    [Fact]
    public void AHoldPastTheThresholdDictatesAndTheReleaseLandsTheTextInTheHold()
    {
        bar.HotkeyDown(); // from cold: opens the bar and starts the press
        Assert.True(bar.IsOpen);
        Assert.True(bar.OpenedByPress);
        Assert.Equal(new[] { "show+activate" }, surface.Events);
        Assert.Equal(new[] { "prewarm" }, calls.Calls);
        Assert.Equal(AskVoiceMachine.StateKind.Pressed, bar.VoiceState.Kind);

        clock.Advance(0.3); // past HoldThreshold: the microphone opens and dictation_start goes out
        Assert.Equal(AskVoiceMachine.StateKind.Recording, bar.VoiceState.Kind);
        Assert.True(bar.Recording);
        Assert.True(bar.Live);
        Assert.Equal("listening…", bar.Status);
        Assert.Equal("Listening…", bar.Placeholder);
        Assert.Contains("mic start", surface.Events);
        Assert.Equal(new[] { "prewarm", "start" }, calls.Calls);
        Assert.Equal(AskDictation.PhaseKind.Live, store.Ask.Dictation.Phase.Kind);

        bar.Feed(AskAudio.Tone(0));
        bar.Feed(AskAudio.Tone(AskAudio.ChunkSamples));
        Assert.Equal(new[] { "prewarm", "start", "audio d-1 1600", "audio d-1 1600" }, calls.Calls);

        store.HandleEvent(Json("""{"type":"DictationStarted","id":"d-1"}"""));
        Dictation("d-1", "ship the", false);
        Assert.Equal("ship the", bar.Partial);
        Assert.Equal("ship the", bar.FieldText);
        store.HandleEvent(Json("""{"type":"DictationLevel","id":"d-1","levels":[0.1,0.4,0.2]}"""));
        Assert.Equal(0.4f, bar.Level);

        bar.HotkeyUp(); // release: stop the mic, dictation_stop, wait for the finish
        Assert.Equal(AskVoiceMachine.StateKind.Stopping, bar.VoiceState.Kind);
        Assert.False(bar.Recording);
        Assert.Equal("finishing…", bar.Status);
        Assert.Contains("mic stop", surface.Events);
        Assert.Equal("stop d-1", calls.Calls[^1]);

        Dictation("d-1", "ship the docs", true);
        Finished("d-1");
        Assert.Equal(AskVoiceMachine.StateKind.Idle, bar.VoiceState.Kind);
        Assert.False(bar.Live);
        Assert.Null(bar.Status);
        Assert.Equal("ship the docs", bar.Text);
        Assert.Equal("ship the docs", bar.VoiceText);
        Assert.Equal("ship the docs", Assert.IsType<StoreModel.ReceiptRow.Holding>(store.Receipt).Held.Label);
        Assert.True(bar.IsOpen); // the text stays in the field for the 3 s hold

        clock.Advance(3); // the hold commits: desk routing, then send_input
        Assert.Contains("desk_open theuth", engine.Calls);
        Assert.Contains("send_input desk-1 ship the docs", engine.Calls);
    }

    [Fact]
    public void ATapTogglesTheBarAndThePressThatOpenedItDoesNotCloseIt()
    {
        bar.HotkeyDown();
        clock.Advance(0.1);
        bar.HotkeyUp(); // a tap from cold: the bar stays open
        Assert.True(bar.IsOpen);
        Assert.Equal(AskVoiceMachine.StateKind.Idle, bar.VoiceState.Kind);
        Assert.DoesNotContain("mic start", surface.Events);
        clock.Advance(0.5);
        bar.HotkeyDown();
        clock.Advance(0.1);
        bar.HotkeyUp(); // a tap on the open bar closes it and hands the foreground back
        Assert.False(bar.IsOpen);
        Assert.Equal("hide+return", surface.Events[^1]);
        Assert.Equal(0, clock.Pending); // no voice timer left running
    }

    [Fact]
    public void ChunksBeforeTheIdAreHeldAndAReleaseBeforeTheIdStopsAfterTheStart()
    {
        calls.HoldStart = new TaskCompletionSource<string>();
        bar.Open();
        bar.HotkeyDown();
        clock.Advance(0.3);
        bar.Feed(AskAudio.Tone(0));
        bar.Feed(AskAudio.Tone(AskAudio.ChunkSamples));
        Assert.Equal(new[] { "prewarm", "start" }, calls.Calls); // nothing can go out without the id
        bar.HotkeyUp();
        Assert.Equal("finishing…", bar.Status);
        Assert.Equal(new[] { "prewarm", "start" }, calls.Calls);
        calls.HoldStart.SetResult("d-9");
        Assert.Equal(new[] { "prewarm", "start", "audio d-9 1600", "audio d-9 1600", "stop d-9" }, calls.Calls);
        Assert.Equal(AskDictation.PhaseKind.Stopping, store.Ask.Dictation.Phase.Kind);
    }

    [Fact]
    public void ARefusedStartFailsThePressAndClosesTheMicrophone()
    {
        calls.StartFails = new RpcException(-32000, "already dictating");
        string? refused = null;
        bar.StartRefused += why => refused = why;
        bar.Open();
        bar.HotkeyDown();
        clock.Advance(0.3);
        Assert.Equal("dictating elsewhere", bar.Status);
        Assert.Equal("dictating elsewhere", refused);
        Assert.False(bar.Recording);
        Assert.False(bar.Live);
        Assert.Equal(AskVoiceMachine.StateKind.Idle, bar.VoiceState.Kind);
        Assert.Contains("mic cancel", surface.Events);
        Assert.Contains(logs, l => l.Contains("dictation_start failed", StringComparison.Ordinal));
    }

    [Fact]
    public void AMicrophoneThatWillNotOpenFailsThePress()
    {
        surface.MicRefusal = "microphone blocked in Settings › Privacy";
        bar.Open();
        bar.HotkeyDown();
        clock.Advance(0.3);
        Assert.Equal("microphone blocked in Settings › Privacy", bar.Status);
        Assert.Equal(AskVoiceMachine.StateKind.Idle, bar.VoiceState.Kind);
        Assert.Equal("stop d-1", calls.Calls[^1]); // the dictation that was started is stopped again
    }

    [Fact]
    public void AMicrophoneThatDiesMidRecordingFailsThePress()
    {
        bar.Open();
        bar.HotkeyDown();
        clock.Advance(0.3);
        bar.MicrophoneFailed("microphone went away");
        Assert.Equal("microphone went away", bar.Status);
        Assert.Equal(AskVoiceMachine.StateKind.Idle, bar.VoiceState.Kind);
        Assert.Equal("stop d-1", calls.Calls[^1]);
        bar.MicrophoneFailed("again"); // idle: ignored
        Assert.Equal("microphone went away", bar.Status);
    }

    [Fact]
    public void FinishingIsCappedAtFiveSecondsAndTheTextSoFarLands()
    {
        bar.Open();
        bar.HotkeyDown();
        clock.Advance(0.3);
        Dictation("d-1", "nearly there", false);
        bar.HotkeyUp();
        clock.Advance(4.9);
        Assert.Equal(AskVoiceMachine.StateKind.Stopping, bar.VoiceState.Kind);
        clock.Advance(0.2);
        Assert.Equal(AskVoiceMachine.StateKind.Idle, bar.VoiceState.Kind);
        Assert.Equal("nearly there", bar.Text);
        Assert.IsType<StoreModel.ReceiptRow.Holding>(store.Receipt);
    }

    [Fact]
    public void ARecognizedNothingIsAFailureNotASend()
    {
        bar.Open();
        bar.HotkeyDown();
        clock.Advance(0.3);
        bar.HotkeyUp();
        Dictation("d-1", "", true);
        Finished("d-1");
        Assert.Equal("Nothing recognized", bar.Status);
        Assert.Null(store.Receipt);
        Assert.Equal("", bar.Text);
    }

    [Fact]
    public void TypingDuringTheVoiceHoldKeepsEditingInsteadOfSending()
    {
        bar.Open();
        bar.HotkeyDown();
        clock.Advance(0.3);
        bar.HotkeyUp();
        Dictation("d-1", "ship it", true);
        Finished("d-1");
        Assert.IsType<StoreModel.ReceiptRow.Holding>(store.Receipt);
        bar.FieldEdited("ship it now");
        Assert.Equal(StoreModel.ReceiptRow.Undone.Instance, store.Receipt); // the hold is taken back
        Assert.Null(bar.VoiceText);
        Assert.Equal("ship it now", bar.Text);
        clock.Advance(4);
        Assert.DoesNotContain(engine.Calls, c => c.StartsWith("send_input", StringComparison.Ordinal));
        // Later partials never overwrite the typed text.
        Assert.False(store.Ask.Dictation.Handle(new AskDictation.Event.Text("d-1", 1, "late", false)));
    }

    [Fact]
    public void TheSyntheticToneStreamsTwentyChunksInOrderThenStops()
    {
        var done = 0;
        bar.ProbeTone(2, () => done++);
        Assert.Equal(new[] { "prewarm", "start" }, calls.Calls);
        clock.Advance(1.95);
        Assert.Equal(19, calls.Audio.Count);
        clock.Advance(0.2);
        Assert.Equal(20, calls.Audio.Count);
        Assert.Equal(1, done);
        Assert.Equal("stop d-1", calls.Calls[^1]);
        Assert.Equal(23, calls.Calls.Count);
        foreach (var chunk in calls.Audio)
        {
            Assert.Equal(AskAudio.ChunkSamples, chunk.Length);
            Assert.InRange(AskAudio.Rms(chunk), 0.20, 0.22); // 0.3 / √2 — what the engine's DictationLevel reads back
        }
        // consecutive chunks continue the sine without a click
        Assert.Equal(AskAudio.Tone(0, 3200)[1600], calls.Audio[1][0]);
        Assert.True(bar.Live);
        Dictation("d-1", "", true);
        Finished("d-1");
        Assert.False(bar.Live);
        Assert.Equal("", bar.Text); // a pure tone is not speech: an empty closed utterance, nothing sent
        Assert.Null(store.Receipt);
        Assert.Equal(0, clock.Pending);
    }

    [Fact]
    public void TheProbeFeedsSynthesizedSpeechInWholeChunksAndATail()
    {
        var pcm = AskAudio.Tone(0, 4000);
        bar.ProbeSamples(pcm, () => { });
        clock.Advance(0.45);
        Assert.Equal(new[] { 1600, 1600, 800 }, calls.Audio.Select(c => c.Length));
        Assert.Equal("stop d-1", calls.Calls[^1]);
        Assert.Equal(pcm.AsSpan(3200, 800).ToArray(), calls.Audio[2]);
    }

    [Fact]
    public void TheAskBarKeysSendUndoCloseAndOpenTheDesk()
    {
        bar.Open();
        Assert.False(bar.HandleKey(Key("j", 0x4A))); // bare letters type
        Assert.False(bar.HandleKey(Key("?", VirtualKeys.Oem2Slash, ChordModifiers.Shift))); // a ? is text here, not the legend
        bar.FieldEdited("ship the docs");
        Assert.True(bar.HandleKey(Key("", VirtualKeys.Return)));
        Assert.Equal("ship the docs", Assert.IsType<StoreModel.ReceiptRow.Holding>(store.Receipt).Held.Label);
        Assert.True(bar.HandleKey(Key("", VirtualKeys.Escape))); // undo: the text stays
        Assert.Equal(StoreModel.ReceiptRow.Undone.Instance, store.Receipt);
        Assert.True(bar.IsOpen);
        Assert.Equal("ship the docs", bar.Text);
        Assert.True(bar.HandleKey(Key("", VirtualKeys.Escape))); // nothing held: close, hand the foreground back
        Assert.False(bar.IsOpen);
        Assert.Equal("hide+return", surface.Events[^1]);

        bar.Open();
        store.SetReceipt(new StoreModel.ReceiptRow.Reply("theuth", "handed to w3", "desk-1"));
        Assert.True(bar.HandleKey(Key("o", 0x4F, ChordModifiers.Control)));
        Assert.Equal("open theuth://c/desk-1", surface.Events[^1]);
        var legends = 0;
        store.LegendRequested += () => legends++;
        Assert.True(bar.HandleKey(Key("/", VirtualKeys.Oem2Slash, ChordModifiers.Control)));
        Assert.Equal(1, legends);
    }

    [Fact]
    public void SetEventFocusSendsTheSessionsAsOnePositionalList()
    {
        Assert.Equal("""[["desk-1"]]""", AskRpc.SetEventFocusParams(new[] { "desk-1" }));
        Assert.Equal("""[["a","b"]]""", AskRpc.SetEventFocusParams(new[] { "a", "b" }));
        Assert.Equal("[[]]", AskRpc.SetEventFocusParams(Array.Empty<string>()));
    }

    [Fact]
    public void CtrlLInThePanelOpensTheBar()
    {
        store.Perform(Intent.AskBar);
        Assert.True(bar.IsOpen);
        Assert.Equal(new[] { "show+activate" }, surface.Events);
    }

    [Fact]
    public void ABarOpenedByW2ForTheSamePressCountsAsOpenedByThePress()
    {
        store.Perform(Intent.AskBar); // W2's hotkey handler opens the bar through the Store first…
        bar.HotkeyDown(); // …then the same press reaches the bar
        Assert.True(bar.OpenedByPress);
        clock.Advance(0.1);
        bar.HotkeyUp(); // a tap: the bar stays open
        Assert.True(bar.IsOpen);
        clock.Advance(1);
        bar.HotkeyDown(); // a later press on the open bar is a voice press; its tap closes
        Assert.False(bar.OpenedByPress);
        clock.Advance(0.1);
        bar.HotkeyUp();
        Assert.False(bar.IsOpen);
    }

    [Fact]
    public void ClosingDuringARecordingCancelsIt()
    {
        bar.Open();
        bar.HotkeyDown();
        clock.Advance(0.3);
        bar.Close(returnFocus: false);
        Assert.Equal("cancelled", bar.Status);
        Assert.Contains("mic cancel", surface.Events);
        Assert.Equal("stop d-1", calls.Calls[^1]);
        Assert.Equal("hide", surface.Events[^1]);
    }

    [Fact]
    public void TheProbeNeverTouchesTheHotkeyButATypedEnterStillSends()
    {
        var probe = new AskBarModel(store, calls, surface, clock, () => clock.Now, logs.Add, automated: true);
        probe.HotkeyDown();
        clock.Advance(0.5);
        Assert.False(probe.IsOpen);
        Assert.Empty(calls.Calls);
        probe.Open();
        probe.FieldEdited("what is open?");
        probe.Submit();
        Assert.Equal("what is open?", Assert.IsType<StoreModel.ReceiptRow.Holding>(store.Receipt).Held.Label);
        clock.Advance(3);
        Assert.Contains("send_input desk-1 what is open?", engine.Calls);
        Assert.Contains(logs, l => l.StartsWith("probe: typed send", StringComparison.Ordinal));
    }

    [Fact]
    public void TheReceiptRowShowsTheHoldThenTheDeskReplysFirstTwoLines()
    {
        Assert.Null(AskBarModel.ReceiptLines(null, 0));
        store.SubmitAsk("ship the docs");
        Assert.Equal(("theuth · ship the docs — sending", "Esc undo · 3"), AskBarModel.ReceiptLines(store.Receipt, store.HoldRemaining));
        clock.Advance(1.2);
        Assert.Equal("Esc undo · 2", AskBarModel.ReceiptLines(store.Receipt, store.HoldRemaining)!.Value.Trailing);
        clock.Advance(1.9);
        Assert.Equal(("theuth · sent — waiting for the desk", "Open Ctrl+O"), AskBarModel.ReceiptLines(store.Receipt, 0));
        // A recorded desk stream (the engine's TextDelta / TurnFinished shapes, crates/agent/src/lib.rs).
        store.HandleEvent(Json("""{"type":"TextDelta","session":"desk-1","turn":"t-1","text":"## Plan\n- handed to w3 — "}"""));
        store.HandleEvent(Json("""{"type":"TextDelta","session":"desk-1","turn":"t-1","text":"the docs ship tonight\n\n- then the notes\n- then more"}"""));
        Assert.Equal(("theuth · Plan — handed to w3 — the docs ship tonight", "Open Ctrl+O"), AskBarModel.ReceiptLines(store.Receipt, 0));
        store.HandleEvent(Json("""{"type":"TurnFinished","session":"desk-1","turn":"t-1","stop_reason":"end_turn","usage":{"input":1,"output":2},"context":3}"""));
        Assert.True(store.Ask.Router.Reply!.Finished);
        Assert.Equal(("Failed · rpc -32000: no provider", ""), AskBarModel.ReceiptLines(new StoreModel.ReceiptRow.Failed("rpc -32000: no provider"), 0));
        Assert.Equal(("Already answered elsewhere", ""), AskBarModel.ReceiptLines(StoreModel.ReceiptRow.Stale.Instance, 0));
        Assert.Equal(("Taken back", ""), AskBarModel.ReceiptLines(StoreModel.ReceiptRow.Undone.Instance, 0));
        Assert.Equal(("Sent · w2 · A", ""), AskBarModel.ReceiptLines(new StoreModel.ReceiptRow.Sent("w2 · A"), 0));
    }
}
