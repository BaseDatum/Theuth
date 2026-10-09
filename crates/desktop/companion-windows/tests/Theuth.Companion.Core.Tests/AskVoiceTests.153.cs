using System.Text;

namespace Theuth.Companion.Core.Tests;

/// <summary>Records the 153 dictation calls (start with its opts, correction) beside the 149 ones.</summary>
internal sealed class FakeDictationCalls153 : IDictationCalls
{
    public readonly List<string> Calls = new();
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
        return Task.FromResult($"d-{Starts}");
    }

    public Task<string> StartAsync(DictationOpts opts)
    {
        Calls.Add($"start {TheuthJson.Serialize(opts)}");
        Starts++;
        return Task.FromResult($"d-{Starts}");
    }

    public Task AudioAsync(string id, short[] pcm16)
    {
        Calls.Add($"audio {id} {pcm16.Length}");
        return Task.CompletedTask;
    }

    public Task StopAsync(string id)
    {
        Calls.Add($"stop {id}");
        return Task.CompletedTask;
    }

    public Task CorrectionAsync(string id, string sentText)
    {
        Calls.Add($"correction {id} {sentText}");
        return Task.CompletedTask;
    }
}

/// <summary>
/// Design 153 wave B, the C# twin of the Swift <c>AskVoiceMachineTests</c> additions (proof 13): the spoken "send it" skips the
/// 3 s hold (§7.4), the microphone opens at the key-down (§8.5) and a tap never starts a dictation, the companion field shows
/// <c>shaped</c> (§4.4), sends <c>dictation_start</c> opts (§14.1, §8.7) and reports <c>dictation_correction</c> at send (§6.1).
/// </summary>
public sealed partial class AskVoiceTests
{
    // Down / Up / Tick / T0 are the 149 cases' helpers (AskTests.cs).

    // MARK: the machine (pure)

    [Fact]
    public void SendItSkipsTheHold()
    {
        var m = new AskVoiceMachine();
        _ = m.Handle(Down(0));
        Assert.Equal(new[] { AskVoiceMachine.Effect.StartRecording }, m.Handle(Tick(0.25)));
        Assert.Equal(new[] { AskVoiceMachine.Effect.StopDictation }, m.Handle(Up(2)));
        Assert.Equal(new[] { AskVoiceMachine.Effect.SendNow("ship it tonight") }, m.Handle(new AskVoiceMachine.Event.Dictated(" ship it tonight ", SendIt: true)));
        Assert.Equal(AskVoiceMachine.State.Idle, m.Current);
        // Without the phrase the same dictation lands in the hold, as in 149.
        _ = m.Handle(Down(3));
        _ = m.Handle(Tick(3.3));
        _ = m.Handle(Up(4));
        Assert.Equal(new[] { AskVoiceMachine.Effect.SetText("ship it tonight") }, m.Handle(new AskVoiceMachine.Event.Dictated("ship it tonight")));
    }

    [Fact]
    public void SendItWithNothingElseRecognizedSendsNothing()
    {
        var m = new AskVoiceMachine();
        _ = m.Handle(Down(0));
        _ = m.Handle(Tick(1));
        _ = m.Handle(Up(2));
        Assert.Equal(new[] { AskVoiceMachine.Effect.Fail("Nothing recognized") }, m.Handle(new AskVoiceMachine.Event.Dictated("  ", SendIt: true)));
    }

    [Fact]
    public void TheEngineEndingTheDictationWhileTheKeyIsHeldLandsTheText()
    {
        var m = new AskVoiceMachine();
        _ = m.Handle(Down(0));
        _ = m.Handle(Tick(0.3));
        Assert.True(m.IsRecording);
        // §7.3: the "send it" settle and silence stop the dictation on the engine; the key is still down.
        Assert.Equal(new[] { AskVoiceMachine.Effect.SendNow("ship it") }, m.Handle(new AskVoiceMachine.Event.Dictated("ship it", SendIt: true)));
        Assert.Equal(AskVoiceMachine.State.Idle, m.Current);
        Assert.Empty(m.Handle(Up(3))); // the release afterwards is nothing
    }

    [Fact]
    public void OpenMicOnKeyDownStartsTheRecorderAtThePressAndTheDictationAtTheThreshold()
    {
        var m = new AskVoiceMachine { OpenMicOnKeyDown = true };
        Assert.Equal(new[] { AskVoiceMachine.Effect.OpenMic }, m.Handle(Down(0)));
        Assert.Empty(m.Handle(Tick(0.2)));
        Assert.Equal(new[] { AskVoiceMachine.Effect.StartRecording }, m.Handle(Tick(0.25)));
        // A tap: the effect is Tap — never StartRecording — so no dictation_start goes out for it (the app drops the buffer).
        var tap = new AskVoiceMachine { OpenMicOnKeyDown = true };
        Assert.Equal(new[] { AskVoiceMachine.Effect.OpenMic }, tap.Handle(Down(0)));
        Assert.Equal(new[] { AskVoiceMachine.Effect.Tap }, tap.Handle(Up(0.2)));
        // Off (the machine's default) is the 149 machine effect for effect.
        Assert.Empty(new AskVoiceMachine().Handle(Down(0)));
    }

    // MARK: the reducer: shaped text and the end command

    private static byte[] Json(string s) => Encoding.UTF8.GetBytes(s);

    private static string Text(string id, int utt, string text, string? shaped, bool final, string? command = null) =>
        "{\"type\":\"DictationText\",\"id\":\"" + id + "\",\"utterance\":" + utt + ",\"text\":\"" + text + "\",\"final\":" + (final ? "true" : "false")
        + (shaped is null ? "" : ",\"shaped\":\"" + shaped + "\"")
        + (command is null ? "" : ",\"command\":{\"command\":\"" + command + "\"" + (command == "send" ? ",\"near_miss\":false" : "") + "}")
        + ",\"actions\":[]}";

    [Fact]
    public void TheFieldShowsShapedTextAndFollowsTheLastPiecesCommand()
    {
        var clock = new ManualScheduler();
        var store = new StoreModel(new FakeEngine(), clock, () => clock.Now, _ => { });
        var d = store.Ask.Dictation;
        d.Handle(AskDictation.Event.Start.Instance);
        d.Handle(new AskDictation.Event.Started("d1"));
        store.HandleEvent(Json(Text("d1", 0, "hi dana comma the deploy is done period", "Hi Dana, the deploy is done.", true)));
        store.HandleEvent(Json(Text("d1", 1, "ship it send it", "Ship it", false, "send")));
        Assert.Equal("Hi Dana, the deploy is done. Ship it", d.LiveText);
        Assert.True(d.SendIt);
        // More speech without the phrase disarms it (§7.1 Armed → Idle on a final without the phrase).
        store.HandleEvent(Json(Text("d1", 1, "ship it send it and the notes", "Ship it send it and the notes", true)));
        Assert.False(d.SendIt);
        // "scratch that": the engine re-sends the previous utterance empty — it disappears from the field.
        store.HandleEvent(Json(Text("d1", 1, "", "", true)));
        Assert.Equal("Hi Dana, the deploy is done.", d.LiveText);
        // An engine before 153 sends no shaped: the raw text shows.
        store.HandleEvent(Json(Text("d1", 2, "plain words", null, true)));
        Assert.Equal("Hi Dana, the deploy is done. plain words", d.LiveText);
        store.HandleEvent(Json("""{"type":"DictationFinished","id":"d1","reason":{"reason":"done"}}"""));
        Assert.False(d.IsActive);
        Assert.Equal("d1", d.LastId);
    }

    // MARK: the Ask bar (virtual time)

    private sealed class Rig
    {
        public readonly ManualScheduler Clock = new();
        public readonly FakeEngine Engine = new();
        public readonly FakeDictationCalls153 Calls = new();
        public readonly FakeSurface Surface = new();
        public readonly List<string> Logs = new();
        public readonly StoreModel Store;
        public readonly AskBarModel Bar;

        public Rig(bool micOnKeyDown = false)
        {
            Store = new StoreModel(Engine, Clock, () => Clock.Now, Logs.Add);
            Bar = new AskBarModel(Store, Calls, Surface, Clock, () => Clock.Now, Logs.Add, micOnKeyDown: () => micOnKeyDown);
        }

        public void Event(string json) => Store.HandleEvent(Json(json));

        public void Finished(string id) => Event("{\"type\":\"DictationFinished\",\"id\":\"" + id + "\",\"reason\":{\"reason\":\"done\"}}");

        /// <summary>Press, hold past the threshold, the engine's DictationStarted.</summary>
        public void PressAndTalk()
        {
            Bar.HotkeyDown();
            Clock.Advance(0.3);
            Event("""{"type":"DictationStarted","id":"d-1"}""");
        }
    }

    [Fact]
    public void SpokenSendItSendsTheShapedTextWithoutTheHold()
    {
        var rig = new Rig();
        rig.PressAndTalk();
        Assert.Contains("""start {"surface":"companion","destination":{"kind":"agent_prompt"},"commands":[],"press_ago_ms":250}""", rig.Calls.Calls);
        rig.Event(Text("d-1", 0, "ship it tonight send it", "Ship it tonight", false, "send"));
        Assert.Equal("Ship it tonight", rig.Bar.FieldText);
        Assert.Equal(AskBarModel.SendingSoon, rig.Bar.Status);
        rig.Bar.HotkeyUp();
        rig.Event(Text("d-1", 0, "ship it tonight send it", "Ship it tonight", true, "send"));
        rig.Finished("d-1");
        Assert.Equal("Ship it tonight", rig.Bar.Text);
        Assert.Null(rig.Store.HeldNow); // no 3 s hold
        Assert.Contains("send_input desk-1 Ship it tonight", rig.Engine.Calls);
        // The send is the commit: the correction report goes out with the text actually sent (unchanged here).
        Assert.Contains("correction d-1 Ship it tonight", rig.Calls.Calls);
        Assert.Contains(rig.Logs, l => l.Contains("sent without the hold", StringComparison.Ordinal));
    }

    [Fact]
    public void WithoutThePhraseTheShapedTextWaitsInTheHold()
    {
        var rig = new Rig();
        rig.PressAndTalk();
        rig.Event(Text("d-1", 0, "version two point five ships", "Version 2.5 ships.", true));
        rig.Bar.HotkeyUp();
        rig.Finished("d-1");
        Assert.Equal("Version 2.5 ships.", rig.Bar.Text);
        Assert.NotNull(rig.Store.HeldNow);
        Assert.DoesNotContain(rig.Engine.Calls, c => c.StartsWith("send_input", StringComparison.Ordinal));
        Assert.DoesNotContain(rig.Calls.Calls, c => c.StartsWith("correction", StringComparison.Ordinal));
        rig.Clock.Advance(3.1);
        Assert.Contains("send_input desk-1 Version 2.5 ships.", rig.Engine.Calls);
        Assert.Contains("correction d-1 Version 2.5 ships.", rig.Calls.Calls);
    }

    [Fact]
    public void ACorrectionReportsTheEditedTextAtSendAndAClearedFieldReportsNothing()
    {
        var rig = new Rig();
        rig.PressAndTalk();
        rig.Event(Text("d-1", 0, "try parakeet tee dee tee", "Try parakeet tee dee tee", true));
        rig.Bar.HotkeyUp();
        rig.Finished("d-1");
        Assert.True(rig.Bar.HandleKey(new KeyPress("", VirtualKeys.Escape, ChordModifiers.None))); // undo the hold: keep editing
        rig.Bar.FieldEdited("Try Parakeet TDT");
        rig.Bar.Submit();
        rig.Clock.Advance(3.1);
        Assert.Contains("correction d-1 Try Parakeet TDT", rig.Calls.Calls);
        Assert.Single(rig.Calls.Calls, c => c.StartsWith("correction", StringComparison.Ordinal)); // the undone hold reported nothing

        // A second dictation, then the field cleared and something else typed: no report.
        rig.PressAndTalk();
        rig.Event(Text("d-2", 0, "never mind", "Never mind.", true).Replace("d-2", "d-2"));
        rig.Event("""{"type":"DictationStarted","id":"d-2"}""");
        rig.Bar.HotkeyUp();
        rig.Finished("d-2");
        Assert.True(rig.Bar.HandleKey(new KeyPress("", VirtualKeys.Escape, ChordModifiers.None)));
        rig.Bar.FieldEdited("");
        rig.Bar.FieldEdited("what is open?");
        rig.Bar.Submit();
        rig.Clock.Advance(3.1);
        Assert.Contains("send_input desk-1 what is open?", rig.Engine.Calls);
        Assert.Single(rig.Calls.Calls, c => c.StartsWith("correction", StringComparison.Ordinal));
    }

    [Fact]
    public void TheEngineStoppingOnSendItWhileTheKeyIsHeldSendsAndTheReleaseDoesNotClose()
    {
        var rig = new Rig();
        rig.Bar.Open();
        rig.Clock.Advance(1); // the bar was already open: this press does not own it
        rig.Bar.HotkeyDown();
        rig.Clock.Advance(0.3);
        rig.Event("""{"type":"DictationStarted","id":"d-1"}""");
        rig.Event(Text("d-1", 0, "ship it send it", "Ship it", true, "send"));
        rig.Finished("d-1"); // §7.3: the engine's settle + silence stop it; the key is still down
        Assert.Contains("send_input desk-1 Ship it", rig.Engine.Calls);
        Assert.False(rig.Bar.Recording);
        Assert.Contains("mic cancel", rig.Surface.Events);
        rig.Bar.HotkeyUp();
        Assert.True(rig.Bar.IsOpen); // the release was not taken for a tap
    }

    [Fact]
    public void TheMicOpensAtThePressAndATapNeverStartsADictation()
    {
        var rig = new Rig(micOnKeyDown: true);
        rig.Bar.HotkeyDown();
        Assert.Contains("mic start", rig.Surface.Events);
        Assert.DoesNotContain(rig.Calls.Calls, c => c.StartsWith("start", StringComparison.Ordinal));
        rig.Bar.Feed(AskAudio.Tone(0)); // buffered: no id yet
        rig.Clock.Advance(0.3); // the threshold: dictation_start, then the buffered chunk in order
        Assert.Equal(new[] { "prewarm", """start {"surface":"companion","destination":{"kind":"agent_prompt"},"commands":[],"press_ago_ms":250}""", "audio d-1 1600" }, rig.Calls.Calls);
        Assert.Single(rig.Surface.Events, e => e == "mic start"); // the recorder is not opened twice

        var tap = new Rig(micOnKeyDown: true);
        tap.Bar.HotkeyDown();
        tap.Bar.Feed(AskAudio.Tone(0));
        tap.Clock.Advance(0.1);
        tap.Bar.HotkeyUp();
        Assert.Contains("mic cancel", tap.Surface.Events);
        tap.Clock.Advance(1);
        Assert.DoesNotContain(tap.Calls.Calls, c => c.StartsWith("start", StringComparison.Ordinal) || c.StartsWith("audio", StringComparison.Ordinal));
    }

    [Fact]
    public void AFixtureProbeDrivesTheFieldThroughTheRealReducer()
    {
        var rig = new Rig();
        rig.Bar.Open();
        var done = false;
        rig.Bar.ProbeFixture(new[]
        {
            Text("rec-1", 0, "ship it tonight send", "Ship it tonight send", false),
            Text("rec-1", 0, "ship it tonight send it", "Ship it tonight", true, "send"),
            """{"type":"DictationFinished","id":"rec-1","reason":{"reason":"done"}}""",
        }, "probe-1", TimeSpan.FromMilliseconds(300), () => done = true);
        rig.Clock.Advance(0.3);
        Assert.Equal("Ship it tonight send", rig.Bar.FieldText);
        rig.Clock.Advance(0.6);
        Assert.Contains("send_input desk-1 Ship it tonight", rig.Engine.Calls);
        rig.Clock.Advance(0.3);
        Assert.True(done);
        Assert.DoesNotContain(rig.Calls.Calls, c => c.StartsWith("start", StringComparison.Ordinal)); // no engine dictation for a probe
        Assert.Equal("""{"type":"DictationText","id":"p","utterance":0}""", AskBarModel.WithId("""{"type":"DictationText","id":"x","utterance":0}""", "p"));
        Assert.Equal("""{"type":"DictionarySuggestion","id":"s1"}""", AskBarModel.WithId("""{"type":"DictionarySuggestion","id":"s1"}""", "p"));
    }

    [Fact]
    public void ADeviceChangeShowsTheMacHintInsteadOfEndingTheDictation()
    {
        var rig = new Rig();
        rig.PressAndTalk();
        rig.Event("""{"type":"DictationDevice","id":"d-1","device":"Headset (USB)","state":"changed"}""");
        Assert.Equal("mic → Headset (USB)", rig.Bar.Status);
        Assert.True(rig.Bar.Live);
        rig.Event("""{"type":"DictationDevice","id":"d-1","device":null,"state":"lost"}""");
        Assert.Equal("mic lost — reconnecting…", rig.Bar.Status);
        rig.Event("""{"type":"DictationDevice","id":"other","device":"X","state":"changed"}""");
        Assert.Equal("mic lost — reconnecting…", rig.Bar.Status); // another dictation's device is not ours
    }
}
