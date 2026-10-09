using System.Text.Json;
using Microsoft.UI.Dispatching;
using Theuth.Companion.Core;
using Theuth.Companion.Island;
using Theuth.Companion.Rpc;
using Theuth.Companion.Voice;
using Windows.Win32;
using Windows.Win32.UI.Input.KeyboardAndMouse;

namespace Theuth.Companion.PointAndSay;

/// <summary>
/// Native point-and-say (149 §3.7, §5.4 D, §7.4 #3–#5): the voice key (Ctrl+Alt+Space, <c>Hotkeys</c>' RegisterHotKey
/// down + 50 ms release poll) drives <see cref="PointMachine"/>; its effects run here —
/// <c>Start</c>: overlays up on every monitor, the 60 Hz sampler, the companion's own PCM into <c>dictation_start(16000)</c>,
/// the island's voice mode, Esc registered; <c>Stop</c>: sampler off, overlays hidden and one dispatcher turn awaited, then
/// <c>dictation_stop</c> (final text, capped at <see cref="PointFlow.FinalTextMs"/>) and <c>screen_context</c> concurrently,
/// <c>desk_open("theuth")</c> (cached), <c>send_input(desk, body, files, null, id)</c> (the engine steers a busy desk itself),
/// then the desk's <c>TurnStarted / ToolCall* / TextDelta / TurnFinished</c> drive the answer card and <c>speak</c> reads the
/// reply when the setting is on (Revision 1 #10); <c>Discard</c>/<c>Refuse</c>: everything down, the hint for 2 s.
/// </summary>
internal sealed class PointController : IDisposable
{
    private const int EscHotkeyId = 0x0149;
    private const int VkEscape = 0x1B;

    private readonly Store store;
    private readonly DispatcherQueue dispatcher;
    private readonly IslandController island;
    private readonly Hotkeys hotkeys;
    private readonly Func<PointSettings> settings;
    private readonly OverlayWindows overlays;
    private readonly PointerSampler sampler;
    private readonly VoiceRecorder recorder;
    private readonly AskDictation dictation = new();
    /// <summary>149 R3: plays the desk's client-sink speech (SpeechStarted/Audio/Finished for <see cref="deskSession"/>).</summary>
    private readonly SpeechPlayer speech;
    private readonly DateTimeOffset epoch = DateTimeOffset.Now;
    private readonly DispatcherQueueTimer tick;
    private readonly DispatcherQueueTimer hintTimer;
    private readonly DispatcherQueueTimer doneTimer;
    private PointState state = new PointState.Idle();
    private string? dictationId;
    /// <summary>149 voice-key fix (b): the one engine dictation this gesture owns, from the start call to the stop (<see cref="DictationLease"/>).</summary>
    private readonly DictationLease lease = new();
    /// <summary>The in-flight <c>dictation_start</c> (+ mic open) of the current gesture, so the send path can wait for it (bounded) before stopping.</summary>
    private Task? starting;
    private string? desk;
    private string? deskEngine;
    private string? deskSession; // the session whose turn the card follows
    private string live = "";
    private string answer = "";
    private string? hint;
    private string? toolName;
    private bool working;
    private bool replaying;
    private bool locked;
    private bool escRegistered;
    private int gesture;
    private TaskCompletionSource<string>? finalText;
    private bool disposed;

    public PointController(Store store, DispatcherQueue dispatcher, IslandController island, Hotkeys hotkeys, Func<PointSettings> settings)
    {
        this.store = store;
        this.dispatcher = dispatcher;
        this.island = island;
        this.hotkeys = hotkeys;
        this.settings = settings;
        // 149 NATIVE: the trail is the system accent (recording = accent), read live and re-read when Windows' accent/theme changes.
        PointFlow.TrailRgb = AccentRgb();
        overlays = new OverlayWindows(PointFlow.TrailRgb, excludeFromCapture: !Flags.Shareable);
        if (Ui.SystemEffects.Current is { } fx) fx.ThemeChanged += () =>
        {
            PointFlow.TrailRgb = AccentRgb();
            overlays.SetColor(PointFlow.TrailRgb);
            Log.Write($"point: trail colour → {PointFlow.TrailHex} (system accent)");
        };
        sampler = new PointerSampler(dispatcher);
        sampler.Sampled += OnSample;
        recorder = new VoiceRecorder(dispatcher);
        recorder.OnChunk = OnChunk;
        recorder.OnFailed = why => dispatcher.TryEnqueue(() =>
        {
            Log.Write($"point: mic failed: {why}");
            ShowHint(why);
            // (b): a dead microphone mid-hold ends the gesture, which stops the engine dictation it started.
            if (state is PointState.Armed or PointState.Holding or PointState.Locked) Step(new PointEvent.Cancel());
        });
        tick = Timer(() => Step(new PointEvent.Tick(Now)));
        hintTimer = Timer(() => { hint = null; if (state is PointState.Idle) island.SetVoice(null); else RenderVoice(); });
        doneTimer = Timer(() => Step(new PointEvent.PanelDone()));
        hotkeys.Pressed += OnHotkeyPressed;
        hotkeys.Released += OnHotkeyReleased;
        island.Window.Hotkey += id => { if (id == EscHotkeyId) Step(new PointEvent.Esc()); };
        store.DictationWire += OnDictationWire;
        store.PropertyChanged += OnStoreChanged;
        speech = new SpeechPlayer(() => deskSession) { Dump = Environment.GetCommandLineArgs().Contains("--speech-dump") };
        store.EventReceived += OnEvent; // WW1's raw stream hook (Store.WW1.cs)
        Log.Write($"point-and-say: installed (voice key {VoiceKeyChord}; annotate={settings().Annotate}; screen={(settings().IncludeScreen ? "on" : "off")}; speak={(settings().Speak ? "on" : "off")})");
    }

    /// <summary>The key-table action of the voice key (WW1's row); resolved by name so the controller compiles against any skeleton.</summary>
    public static KeyAction? VoiceKeyAction { get; } = ResolveVoiceKey();

    private static KeyAction? ResolveVoiceKey()
    {
        foreach (var name in new[] { "VoiceKey", "PointAndSay", "Point", "PointAndSayKey" })
        {
            if (Enum.TryParse<KeyAction>(name, out var a)) return a;
        }
        return null;
    }

    public string VoiceKeyChord => VoiceKeyAction is { } a && store.Keys.ChordFor(a) is { } c ? c.Symbol : "Ctrl+Alt+Space";

    public PointState State => state;

    private ulong Now => (ulong)Math.Max(0, (DateTimeOffset.Now - epoch).TotalMilliseconds);

    private static string Why(Exception e) => e is RpcException r ? r.ToString() : e.Message;

    /// <summary>The eye (Revision 1 #9): mirrors the island's capture exclusion onto the overlays.</summary>
    public void SetHiddenFromCapture(bool hidden) => overlays.SetHiddenFromCapture(hidden);

    private DispatcherQueueTimer Timer(Action a)
    {
        var t = dispatcher.CreateTimer();
        t.IsRepeating = false;
        t.Tick += (_, _) => a();
        return t;
    }

    private void After(DispatcherQueueTimer t, int ms)
    {
        t.Stop();
        t.Interval = TimeSpan.FromMilliseconds(Math.Max(1, ms));
        t.Start();
    }

    // MARK: the key

    private void OnHotkeyPressed(KeyAction a)
    {
        if (VoiceKeyAction is not { } vk || a != vk) return;
        Step(new PointEvent.Pressed());
    }

    private void OnHotkeyReleased(KeyAction a, TimeSpan held)
    {
        if (VoiceKeyAction is not { } vk || a != vk) return;
        Step(new PointEvent.Released());
    }

    /// <summary>§5.4 D: the standing refusal — off, disconnected, a meeting talking, dictation busy.</summary>
    private string? Refused()
    {
        var s = settings();
        if (!s.Enabled) return "disabled";
        if (store.Connection != RpcClient.State.Open) return "disconnected";
        if (island.Snapshot.State == "Recording" && island.Snapshot.Listening) return "talking";
        if (store.Ask.Dictation.IsActive) return "already dictating";
        return null;
    }

    /// <summary>Simulated press/release for the probe (<c>--point-probe</c>) and tests on build-B.</summary>
    public void Simulate(PointEvent e) => Step(e);

    /// <summary>
    /// 149 R2 — <c>Surfaces.PointRequested(card)</c>: P on a card's reply row (card != null) captures the screen NOW without a
    /// gesture — one sample at the pointer, overlays hidden, <c>screen_context</c>, the PNGs parked on the card's session
    /// (<c>Store.AttachToReply</c>) so the reply row says "N screenshots attached" and the send carries them; the rail's point
    /// glyph (card == null) starts a point-and-say gesture LOCKED, as if the voice key had been double-tapped — Esc cancels, a tap
    /// of the voice key or the glyph again ends it and sends.
    /// </summary>
    public void OnPointRequested(string? cardSession)
    {
        if (cardSession is not null) { _ = AttachScreenAsync(cardSession); return; }
        if (state.IsGesture || state is PointState.Sending)
        {
            // The glyph again while locked/holding: the tap that sends (Locked → Pressed = send).
            if (state is PointState.Locked) Step(new PointEvent.Pressed());
            return;
        }
        // Idle/Replying → Armed (Start) → a tap (Released < HOLD) → Tap1 → a second press inside the window → Locked.
        var t = Now;
        Step(new PointEvent.Pressed());
        if (state is not PointState.Armed) return; // refused
        var (s1, _) = PointMachine.Step(state, new PointEvent.Released(), t, null);
        var (s2, fx2) = PointMachine.Step(s1, new PointEvent.Pressed(), t + 1, null);
        state = s2;
        tick.Stop();
        foreach (var fx in fx2) Run(fx);
        Log.Write($"point: glyph → {state.Json} (locked gesture; Esc cancels, the voice key or the glyph again sends)");
    }

    private async Task AttachScreenAsync(string session)
    {
        if (!PInvoke.GetCursorPos(out var p)) return;
        var sample = new PointSample(0, p.X, p.Y, true);
        overlays.Hide();
        await Yield();
        var (lines, files) = await ScreenContext(new[] { sample }, 0, settings().AccessibilityOn);
        store.AttachToReply(session, files);
        Log.Write($"point: P on card session {session} → {files.Count} file(s) parked for the reply{(lines.Count > 0 ? "; " + lines[0] : "")}");
    }

    private void Step(PointEvent e)
    {
        if (disposed) return;
        var refused = e is PointEvent.Pressed ? Refused() : null;
        var (next, effects) = PointMachine.Step(state, e, Now, refused);
        if (next != state) Log.Write($"point: {state.Json} --{e.GetType().Name}--> {next.Json}{(effects.Count > 0 ? " fx=" + string.Join(",", effects.Select(f => f.GetType().Name)) : "")}");
        state = next;
        foreach (var fx in effects) Run(fx);
        // The machine's timers: Armed → a tick at HOLD_MS; Tap1 → a tick just past DOUBLE_TAP_MS.
        switch (state)
        {
            case PointState.Armed a: After(tick, (int)(a.T0 + PointMachine.HoldMs - Math.Min(Now, a.T0 + PointMachine.HoldMs)) + 1); break;
            case PointState.Tap1 t: After(tick, (int)(t.Released + PointMachine.DoubleTapMs - Math.Min(Now, t.Released + PointMachine.DoubleTapMs)) + 2); break;
            default: tick.Stop(); break;
        }
    }

    private void Run(PointEffect fx)
    {
        switch (fx)
        {
            case PointEffect.Start: Start(); break;
            case PointEffect.ShowLock: locked = true; RenderVoice(); break;
            case PointEffect.Stop s: _ = StopAndSend(s.Reason); break;
            case PointEffect.Discard d: Discard(d.Reason, d.Hint); break;
            case PointEffect.Conceal: Conceal(); break;
            case PointEffect.Refuse r: ShowHint(PointFlow.RefusalHint(r.Reason)); Log.Write($"point: refused — {r.Reason}"); break;
        }
    }

    // MARK: effects

    private void Start()
    {
        gesture++;
        locked = false;
        live = "";
        answer = "";
        hint = null;
        working = false;
        deskSession = null;
        doneTimer.Stop();
        hintTimer.Stop();
        var s = settings();
        _ = store.Client.CallRawAsync("stop_speaking", (object?)null).ContinueWith(_ => { }, TaskScheduler.Default);
        speech.Stop();
        lastStatusSecond = uint.MaxValue;
        overlays.Reveal();
        sampler.Start(s.AnnotateMouse);
        RegisterEsc();
        dictation.Handle(AskDictation.Event.Start.Instance);
        dictationId = null;
        finalText = new TaskCompletionSource<string>(TaskCreationOptions.RunContinuationsAsynchronously);
        island.SetVoice(View("listening"));
        int g = lease.Begin();
        starting = StartDictation(g);
        Log.Write($"point: start — overlays={overlays.Windows.Count} annotate={s.Annotate}");
    }

    private async Task StartDictation(int g)
    {
        string id;
        try
        {
            id = await store.Dictation.StartAsync();
        }
        catch (Exception e)
        {
            var why = Why(e);
            Log.Write($"point: dictation_start failed: {why}");
            await OnDispatcher(() =>
            {
                // (a)/(b): a refusal ends the gesture for the rest of this hold; a stale refusal (older press) is ignored.
                if (!lease.StartFailed(g)) return;
                dictation.Handle(new AskDictation.Event.StartFailed(why));
                ShowHint(why.Contains("already", StringComparison.OrdinalIgnoreCase) ? PointFlow.RefusalHint("already dictating") : why);
                Step(new PointEvent.Cancel());
            });
            return;
        }
        var mic = await recorder.StartAsync();
        await OnDispatcher(() =>
        {
            if (lease.Started(g, id) == LeaseVerdict.Stop)
            {
                // The gesture ended (sent, cancelled, Esc, disconnected) while the start was in flight: this dictation has no owner — stop it now.
                Log.Write($"point: dictation {id} started after the gesture ended — stopping it");
                _ = store.Dictation.StopAsync(id).ContinueWith(t => { if (t.IsFaulted) Log.Write($"point: late dictation_stop failed: {t.Exception?.GetBaseException().Message}"); }, TaskScheduler.Default);
                recorder.Cancel();
                return;
            }
            dictationId = id;
            dictation.Handle(new AskDictation.Event.Started(id));
            if (mic is { } err)
            {
                // (b): no microphone → nothing to dictate; end the gesture, which stops the dictation just started.
                Log.Write($"point: mic: {err}");
                ShowHint(err);
                Step(new PointEvent.Cancel());
            }
            else Log.Write($"point: dictation {id} live, mic {recorder.Description}");
        });
    }

    /// <summary>Runs <paramref name="a"/> on the UI thread and completes when it ran (so <see cref="starting"/> covers the lease update).</summary>
    private Task OnDispatcher(Action a)
    {
        var tcs = new TaskCompletionSource(TaskCreationOptions.RunContinuationsAsynchronously);
        if (!dispatcher.TryEnqueue(() => { try { a(); } finally { tcs.TrySetResult(); } })) tcs.TrySetResult();
        return tcs.Task;
    }

    /// <summary>149 voice-key fix: the engine went away mid-gesture — end it (the stop is best effort; the engine's side died with the connection).</summary>
    private void OnStoreChanged(object? sender, System.ComponentModel.PropertyChangedEventArgs e)
    {
        if (e.PropertyName != nameof(Store.Connection) || store.Connection == RpcClient.State.Open) return;
        if (state is PointState.Idle) return;
        Log.Write($"point: engine {store.Connection.ToString().ToLowerInvariant()} mid-gesture — cancelling");
        ShowHint(PointFlow.RefusalHint("disconnected"));
        Step(new PointEvent.Cancel());
    }

    private void OnChunk(short[] pcm)
    {
        if (dictationId is not { } id) return;
        _ = store.Dictation.AudioAsync(id, pcm).ContinueWith(t => { if (t.IsFaulted) Log.Write($"point: dictation_audio failed: {t.Exception?.GetBaseException().Message}"); }, TaskScheduler.Default);
    }

    private uint lastStatusSecond = uint.MaxValue;

    private void OnSample(PointSample s)
    {
        overlays.Append(s.X, s.Y, s.Pen);
        overlays.Present();
        // The voice mode's status clock changes once a second; re-render only then (not at 60 Hz).
        uint second = s.TMs / 1000;
        if (second != lastStatusSecond && state is PointState.Armed or PointState.Holding or PointState.Locked or PointState.Tap1)
        {
            lastStatusSecond = second;
            RenderVoice();
        }
    }

    private async Task StopAndSend(string reason)
    {
        int g = gesture;
        sampler.Stop();
        tick.Stop();
        UnregisterEsc(); // Esc is registered only between press and send; the Sending state still takes it from the island
        RegisterEsc();
        var samples = sampler.Samples.ToArray();
        long durationMs = samples.Length > 0 ? samples[^1].TMs : 0;
        // D13: every overlay hidden and one dispatcher turn awaited before the engine captures.
        overlays.Hide();
        await Yield();
        recorder.Stop();
        var s = settings();
        var screenTask = s.IncludeScreen ? ScreenContext(samples, durationMs, s.AccessibilityOn) : Task.FromResult<(IReadOnlyList<string>, IReadOnlyList<string>)>((Array.Empty<string>(), Array.Empty<string>()));
        var textTask = FinalText();
        island.SetVoice(View("sending"));
        await Task.WhenAll(screenTask, textTask);
        if (g != gesture || state is not PointState.Sending) return;
        var text = textTask.Result;
        var (lines, files) = screenTask.Result;
        Log.Write($"point: stop ({reason}) — {samples.Length} samples over {durationMs} ms, text {text.Length} chars, screen lines={lines.Count} files={files.Count}");
        if (text.Trim().Length == 0)
        {
            ShowHint("Nothing recognized");
            Step(new PointEvent.Cancel());
            return;
        }
        string target;
        try
        {
            target = await DeskAsync();
        }
        catch (Exception e)
        {
            Log.Write($"point: desk_open failed: {Why(e)}");
            ShowHint(PointFlow.RefusalHint("hub"));
            Step(new PointEvent.Cancel());
            return;
        }
        if (g != gesture) return;
        var body = PointFlow.ComposeBody(text, durationMs, lines);
        var clientId = Guid.NewGuid().ToString();
        try
        {
            // `send_input(session, text, files, delivery, id)`: a busy desk is steered by the engine's default delivery.
            await store.Client.CallAsync<string>("send_input", target, body, files, null, clientId);
        }
        catch (Exception e)
        {
            var why = Why(e);
            Log.Write($"point: send_input failed: {why}");
            ShowHint(why);
            Step(new PointEvent.Cancel());
            return;
        }
        if (g != gesture) return;
        deskSession = target;
        working = true;
        answer = "";
        live = text;
        Log.Write($"point: sent to desk {target} ({body.Length} chars, {files.Count} files) id={clientId}");
        island.SetVoice(View("working"));
    }

    private Task Yield()
    {
        var tcs = new TaskCompletionSource(TaskCreationOptions.RunContinuationsAsynchronously);
        dispatcher.TryEnqueue(DispatcherQueuePriority.Low, () => tcs.TrySetResult());
        return tcs.Task;
    }

    private async Task<string> FinalText()
    {
        // (b) on the send path: a release before dictation_start returned — wait for the start (bounded) so the stop below owns the id;
        // if it is still in flight after the bound, Close() marks the lease and the late Started answer stops the id itself.
        if (lease.Starting && starting is { } s)
        {
            var first = await Task.WhenAny(s, Task.Delay(PointFlow.StartWaitMs));
            if (first != s) Log.Write($"point: dictation_start still in flight after {PointFlow.StartWaitMs} ms — the late start will stop itself");
        }
        var id = lease.Close();
        dictationId = null;
        if (id is null) return dictation.LiveText;
        dictation.Handle(AskDictation.Event.Stop.Instance);
        try { await store.Dictation.StopAsync(id); }
        catch (Exception e) { Log.Write($"point: dictation_stop failed: {Why(e)}"); return dictation.LiveText; }
        var done = finalText?.Task ?? Task.FromResult(dictation.LiveText);
        var winner = await Task.WhenAny(done, Task.Delay(PointFlow.FinalTextMs));
        if (winner != done) Log.Write($"point: final text not flushed within {PointFlow.FinalTextMs} ms — sending the live text");
        return done.IsCompletedSuccessfully ? done.Result : dictation.LiveText;
    }

    private async Task<(IReadOnlyList<string> Lines, IReadOnlyList<string> Files)> ScreenContext(PointSample[] samples, long durationMs, bool accessibility, string? color = null)
    {
        try
        {
            var req = new
            {
                samples = samples.Select(p => new { t_ms = p.TMs, x = (float)p.X, y = (float)p.Y, pen = p.Pen }).ToArray(),
                color = color ?? PointFlow.TrailHex,
                since_ms = durationMs,
                accessibility,
            };
            var reply = await store.Client.CallAsync<JsonElement>("screen_context", req);
            var lines = new List<string>();
            var files = new List<string>();
            if (reply.ValueKind == JsonValueKind.Object)
            {
                if (reply.TryGetProperty("lines", out var ls) && ls.ValueKind == JsonValueKind.Array)
                    foreach (var l in ls.EnumerateArray()) if (l.ValueKind == JsonValueKind.String) lines.Add(l.GetString() ?? "");
                if (reply.TryGetProperty("files", out var fs) && fs.ValueKind == JsonValueKind.Array)
                    foreach (var f in fs.EnumerateArray()) if (f.ValueKind == JsonValueKind.String) files.Add(f.GetString() ?? "");
            }
            Log.Write($"point: screen_context → {lines.Count} line(s), {files.Count} file(s){(lines.Count > 0 ? ": " + lines[0] : "")}");
            return (lines, files);
        }
        catch (Exception e)
        {
            Log.Write($"point: screen_context failed: {Why(e)}");
            dispatcher.TryEnqueue(() => ShowHint(PointFlow.RefusalHint("screen")));
            return (Array.Empty<string>(), Array.Empty<string>());
        }
    }

    /// <summary><c>desk_open("theuth")</c> once per engine (§0.12).</summary>
    private async Task<string> DeskAsync()
    {
        var engine = store.SelfEngineId;
        if (desk is { } d && deskEngine == engine) return d;
        var id = await store.Client.CallAsync<string>("desk_open", "theuth");
        desk = id;
        deskEngine = engine;
        return id;
    }

    private void Discard(string reason, string? hintText)
    {
        sampler.Stop();
        tick.Stop();
        overlays.Hide();
        UnregisterEsc();
        recorder.Cancel();
        // (b): stop the dictation this gesture owns; a start still in flight is stopped by its own answer (lease Closed).
        if (lease.Close() is { } id)
        {
            _ = store.Dictation.StopAsync(id).ContinueWith(t => { if (t.IsFaulted) Log.Write($"point: dictation_stop failed: {t.Exception?.GetBaseException().Message}"); }, TaskScheduler.Default);
        }
        dictationId = null;
        gesture++;
        working = false;
        speech.Stop(); // 149 R3: Esc also cuts the spoken answer (stop_speaking goes with the next Start)
        _ = store.Client.CallRawAsync("stop_speaking", (object?)null).ContinueWith(_ => { }, TaskScheduler.Default);
        deskSession = null;
        if (hintText is { } h)
        {
            var s = settings();
            // The first HINT_USES misses show the hint; later ones stay quiet (voice-key.ts hintsShown).
            if (s.HintsShown < PointSettings.HintUses || reason != "short") ShowHint(h);
            else island.SetVoice(null);
            if (reason is "short" or "double-tap-missed") { s.HintsShown++; island.SavePointSettings(s); }
        }
        else
        {
            island.SetVoice(null);
        }
        Log.Write($"point: discard ({reason})");
    }

    private void Conceal()
    {
        doneTimer.Stop();
        UnregisterEsc();
        working = false;
        // 149 R3: the card is gone but the voice may still be mid-sentence — SpeechPlayer follows deskSession, so keep it set
        // until the utterance finishes (SpeechFinished) or the next gesture; a null here would drop the rest of the packets.
        if (speech.Utterance is null) deskSession = null;
        island.SetVoice(null);
    }

    /// <summary><c>--speech-probe</c>: open the desk, ask the engine to speak a line, and let <see cref="SpeechPlayer"/> report.</summary>
    public async Task ProbeSpeakAsync(string text)
    {
        try
        {
            var target = await DeskAsync();
            deskSession = target;
            var id = await store.Client.CallAsync<string>("speak", target, text);
            Log.Write($"speech-probe: speak({target}, {text.Length} chars) → {id}");
        }
        catch (Exception e) { Log.Write($"speech-probe: failed: {Why(e)}"); }
    }

    private void ShowHint(string h)
    {
        hint = h;
        island.SetVoice(View(state is PointState.Idle ? "hint" : Phase()));
        After(hintTimer, PointFlow.HintMs);
    }

    // MARK: the desk's turn → the answer card

    private void OnEvent(string type, byte[] json)
    {
        if (deskSession is null) return;
        if (speech.Handle(type, json)) return; // 149 R3: the speech packets never reach the card
        StreamDelta d;
        try { d = StreamDelta.Decode(json); } catch (Exception) { return; }
        if (d.Session != deskSession) return;
        switch (type)
        {
            case "TurnStarted":
                working = true;
                toolName = null;
                RenderVoice();
                break;
            case "TextDelta":
                answer += d.Text ?? "";
                RenderVoice();
                break;
            case "ToolCallStarted":
                working = true;
                toolName = ToolNameOf(json);
                RenderVoice();
                break;
            case "ToolCallFinished":
                toolName = null;
                RenderVoice();
                break;
            case "Error":
                answer = d.Message ?? "the desk failed";
                working = false;
                RenderVoice();
                After(doneTimer, PointFlow.ErrorMs);
                Step(new PointEvent.PanelDone());
                break;
            case "TurnFinished":
                working = false;
                Log.Write($"point: desk turn finished ({d.StopReason}) — {answer.Length} chars{(settings().Speak ? ", speaking" : "")}");
                toolName = null;
                if (settings().Speak && answer.Trim().Length > 0 && !replaying)
                {
                    var spoken = answer;
                    _ = store.Client.CallRawAsync("speak", deskSession, spoken).ContinueWith(t => { if (t.IsFaulted) Log.Write($"point: speak failed: {t.Exception?.GetBaseException().Message}"); }, TaskScheduler.Default);
                }
                RenderVoice();
                Step(new PointEvent.PanelDone()); // Sending → Replying
                After(doneTimer, (int)PointMachine.DoneMs); // Replying → Idle + Conceal
                break;
        }
    }

    private static string? ToolNameOf(byte[] json)
    {
        try
        {
            using var doc = JsonDocument.Parse(json);
            return doc.RootElement.TryGetProperty("name", out var n) && n.ValueKind == JsonValueKind.String ? n.GetString() : null;
        }
        catch (JsonException) { return null; }
    }

    /// <summary>
    /// <c>--point-replay</c>: the answer card through the real path without a model — a recorded desk sequence (TurnStarted ·
    /// ToolCallStarted/Finished · TextDelta × n · TurnFinished) fed into <c>Store.HandleEvent</c> (so it arrives through
    /// <c>EventReceived</c> like the engine's stream) for a fake desk session after a simulated send. <c>speak</c> is skipped.
    /// </summary>
    public async Task ReplayAsync(string text, IReadOnlyList<string> jsonLines, int gapMs = 350)
    {
        gesture++;
        replaying = true;
        state = new PointState.Sending(Now);
        deskSession = "replay-desk";
        desk = deskSession;
        live = text;
        answer = "";
        working = true;
        hint = null;
        island.SetVoice(View("working"));
        Log.Write($"point-replay: {jsonLines.Count} events for {deskSession}");
        foreach (var line in jsonLines)
        {
            await Task.Delay(gapMs);
            store.HandleEvent(System.Text.Encoding.UTF8.GetBytes(line));
        }
    }

    private void OnDictationWire(string json)
    {
        DictationEvent w;
        try { w = DictationEvent.Decode(System.Text.Encoding.UTF8.GetBytes(json)); } catch (Exception) { return; }
        if (w.Id is not { } id || dictationId != id) return;
        switch (w.Type)
        {
            case "DictationText":
                dictation.Handle(new AskDictation.Event.Text(id, w.Utterance ?? 0, w.Text ?? "", w.IsFinal ?? false));
                live = dictation.LiveText;
                RenderVoice();
                break;
            case "DictationLevel":
                dictation.Handle(new AskDictation.Event.Level(id, w.Levels ?? new ValueList<float>()));
                break;
            case "DictationFinished":
                dictation.Handle(new AskDictation.Event.Finished(id, w.Reason?.Reason == "error" ? w.Reason.Message ?? "dictation failed" : null));
                live = dictation.LiveText;
                finalText?.TrySetResult(dictation.FinalText.Length > 0 ? dictation.FinalText : dictation.LiveText);
                break;
        }
    }

    // MARK: the voice view

    private string Phase() => state switch
    {
        PointState.Armed or PointState.Holding or PointState.Locked or PointState.Tap1 => "listening",
        PointState.Sending => working ? "working" : "sending",
        PointState.Replying => "done",
        _ => "hint",
    };

    private void RenderVoice()
    {
        if (!island.Window.InVoiceMode && state is PointState.Idle) return;
        island.SetVoice(View(Phase()));
    }

    private VoiceView View(string phase)
    {
        long held = sampler.Samples.Count > 0 ? sampler.Samples[^1].TMs : 0;
        string status = phase switch
        {
            "listening" => PointFlow.ListeningStatus(held, VoiceKeyChord, locked),
            "sending" => "sending to theuth's desk…",
            "working" => toolName is { } tn ? $"theuth is working… · {tn}" : "theuth is working…",
            "done" => "theuth's desk",
            _ => hint ?? "",
        };
        string? card = phase is "working" or "done" ? answer : null;
        return new VoiceView(phase, phase == "hint" ? "" : live.Length > 0 ? live : dictation.LiveText, status, card, hint, locked, working,
            deskSession is { } ds ? $"theuth://c/{ds}" : null);
    }

    // MARK: Esc

    private void RegisterEsc()
    {
        if (escRegistered) return;
        escRegistered = PInvoke.RegisterHotKey(island.Window.Hwnd, EscHotkeyId, HOT_KEY_MODIFIERS.MOD_NOREPEAT, VkEscape);
        if (!escRegistered) Log.Write("point: RegisterHotKey(Esc) failed — Esc cancels only with the island focused");
    }

    private void UnregisterEsc()
    {
        if (!escRegistered) return;
        PInvoke.UnregisterHotKey(island.Window.Hwnd, EscHotkeyId);
        escRegistered = false;
    }

    // MARK: 149 R3 — capture assertion (the Mac W2 probe's twin)

    /// <summary>
    /// <c>--capture-probe</c>: start a gesture, draw a loop with the pointer, then (A) call <c>screen_context</c> with the overlays
    /// UP and capture-visible (WDA_NONE) — the overlay's malachite stroke must be in the PNG — and (B) run the real Stop sequence
    /// (hide every overlay, WDA_EXCLUDEFROMCAPTURE, one dispatcher turn) before <c>screen_context</c> — it must be gone.
    /// The engine itself paints the pointer path into the PNG from the samples (<c>computer/context.rs</c>, <c>TRAIL_ALPHA</c>
    /// 0.85, in the request's <c>color</c>): the first run of this probe counted ~3 900 "trail" pixels in BOTH captures for that
    /// reason. So the probe asks the engine to paint in MAGENTA (<c>#ff00ff</c>) and counts the trail colour (the overlay) and magenta
    /// (the engine) separately — malachite must go to 0 after the Stop sequence while magenta stays (the engine drew it).
    /// </summary>
    public async Task<string> CaptureProbeAsync()
    {
        if (!PInvoke.GetCursorPos(out var origin)) return "no cursor";
        Step(new PointEvent.Pressed());
        if (state is not PointState.Armed) { Log.Write($"capture-probe: refused ({state.Json})"); return "refused"; }
        await Task.Delay(250);
        // A loop ~140 px across, 48 steps 25 ms apart — the sampler (16 ms) records it, the overlay draws it.
        for (int i = 0; i <= 48; i++)
        {
            double a = 2 * Math.PI * i / 48;
            PInvoke.SetCursorPos(origin.X + (int)(70 * Math.Cos(a)), origin.Y + (int)(70 * Math.Sin(a)));
            await Task.Delay(25);
        }
        await Task.Delay(200);
        var samples = sampler.Samples.ToArray();
        long durationMs = samples.Length > 0 ? samples[^1].TMs : 0;
        // The engine paints the path it is given and crops around it: hand it the loop shifted 300 px right, so its magenta
        // stroke and its crop sit beside the overlay's real loop, which stays countable in the display overview (second run of
        // this probe: with the true samples the engine's stroke covered the overlay's exactly — 69 vs 55 trail px, no signal).
        var shifted = samples.Select(s => new PointSample(s.TMs, s.X + 300, s.Y, s.Pen)).ToArray();
        // (A) overlays up and visible to capture
        overlays.SetHiddenFromCapture(false);
        await Yield();
        var (_, filesUp) = await ScreenContext(shifted, durationMs, false, ProbeColor);
        var up = await CountTrail(filesUp);
        // (B) the real Stop sequence: WDA_EXCLUDEFROMCAPTURE + hidden + one dispatcher turn, then the engine captures
        overlays.SetHiddenFromCapture(true);
        overlays.Hide();
        await Yield();
        var (_, filesDown) = await ScreenContext(shifted, durationMs, false, ProbeColor);
        var down = await CountTrail(filesDown);
        var line = $"capture-probe: {samples.Length} samples over {durationMs} ms; overlays UP → trail px {up.Trail} (engine magenta {up.Green}) in {up.Files} file(s) {up.Size}; STOP sequence → trail px {down.Trail} (engine magenta {down.Green}) in {down.Files} file(s) {down.Size}";
        Log.Write(line);
        overlays.SetHiddenFromCapture(!Flags.Shareable);
        Step(new PointEvent.Cancel());
        return line;
    }

    /// <summary>±24 of 255 per channel: a 0.85-alpha stroke drifts from the pure colour by up to 0.15 × the ground.</summary>
    public const int TrailTolerance = 24;
    /// <summary>What the probe asks the engine to paint the path in, so the overlay's malachite is countable on its own.</summary>
    public const string ProbeColor = "#ff00ff";

    /// <summary>The system accent packed for the raster (<c>SystemAccentColor</c> through <see cref="Ui.Theme.Accent"/>).</summary>
    private static uint AccentRgb()
    {
        var c = Ui.Theme.Accent();
        return PointFlow.PackRgb(c.R, c.G, c.B);
    }

    private static async Task<(int Trail, int Green, int Files, string Size)> CountTrail(IReadOnlyList<string> files)
    {
        int trail = 0, green = 0, n = 0; string size = "";
        foreach (var path in files)
        {
            try
            {
                using var stream = File.OpenRead(path);
                var decoder = await global::Windows.Graphics.Imaging.BitmapDecoder.CreateAsync(stream.AsRandomAccessStream());
                var data = await decoder.GetPixelDataAsync(global::Windows.Graphics.Imaging.BitmapPixelFormat.Bgra8, global::Windows.Graphics.Imaging.BitmapAlphaMode.Ignore,
                    new global::Windows.Graphics.Imaging.BitmapTransform(), global::Windows.Graphics.Imaging.ExifOrientationMode.IgnoreExifOrientation, global::Windows.Graphics.Imaging.ColorManagementMode.DoNotColorManage);
                var px = data.DetachPixelData();
                size += $"{(n > 0 ? "+" : "")}{decoder.PixelWidth}x{decoder.PixelHeight}";
                n++;
                int trailBefore = trail;
                int tr = (int)(PointFlow.TrailRgb >> 16 & 0xff), tg = (int)(PointFlow.TrailRgb >> 8 & 0xff), tb = (int)(PointFlow.TrailRgb & 0xff);
                for (int i = 0; i + 3 < px.Length; i += 4)
                {
                    int b = px[i], g = px[i + 1], r = px[i + 2];
                    if (Near(r, g, b, tr, tg, tb, 0x10) || Near(r, g, b, tr, tg, tb, 0xf0)) trail++;
                    if (Near(r, g, b, 0xff, 0x00, 0xff, 0x10) || Near(r, g, b, 0xff, 0x00, 0xff, 0xf0)) green++; // the engine's magenta
                }
                Log.Write($"capture-probe: {Path.GetFileName(path)} {decoder.PixelWidth}x{decoder.PixelHeight} trail={trail - trailBefore}");
            }
            catch (Exception e) { Log.Write($"capture-probe: {path}: {e.Message}"); }
        }
        return (trail, green, n, size);

        static bool Near(int r, int g, int b, int tr, int tg, int tb, int ground)
        {
            int cr = (int)(tr * TrailRaster.TrailAlpha + ground * (1 - TrailRaster.TrailAlpha));
            int cg = (int)(tg * TrailRaster.TrailAlpha + ground * (1 - TrailRaster.TrailAlpha));
            int cb = (int)(tb * TrailRaster.TrailAlpha + ground * (1 - TrailRaster.TrailAlpha));
            return Math.Abs(r - cr) <= TrailTolerance && Math.Abs(g - cg) <= TrailTolerance && Math.Abs(b - cb) <= TrailTolerance;
        }
    }

    public void Dispose()
    {
        if (disposed) return;
        disposed = true;
        hotkeys.Pressed -= OnHotkeyPressed;
        hotkeys.Released -= OnHotkeyReleased;
        store.DictationWire -= OnDictationWire;
        store.PropertyChanged -= OnStoreChanged;
        store.EventReceived -= OnEvent;
        if (lease.Close() is { } id) _ = store.Dictation.StopAsync(id).ContinueWith(_ => { }, TaskScheduler.Default);
        dictationId = null;
        speech.Dispose();
        UnregisterEsc();
        sampler.Dispose();
        recorder.Cancel();
        overlays.Dispose();
    }
}