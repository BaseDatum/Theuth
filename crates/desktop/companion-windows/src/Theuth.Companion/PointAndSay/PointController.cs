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
    private readonly DateTimeOffset epoch = DateTimeOffset.Now;
    private readonly DispatcherQueueTimer tick;
    private readonly DispatcherQueueTimer hintTimer;
    private readonly DispatcherQueueTimer doneTimer;
    private PointState state = new PointState.Idle();
    private string? dictationId;
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
        overlays = new OverlayWindows(PointFlow.TrailRgb, excludeFromCapture: !Flags.Shareable);
        sampler = new PointerSampler(dispatcher);
        sampler.Sampled += OnSample;
        recorder = new VoiceRecorder(dispatcher);
        recorder.OnChunk = OnChunk;
        recorder.OnFailed = why => dispatcher.TryEnqueue(() => { Log.Write($"point: mic failed: {why}"); ShowHint(why); });
        tick = Timer(() => Step(new PointEvent.Tick(Now)));
        hintTimer = Timer(() => { hint = null; if (state is PointState.Idle) island.SetVoice(null); else RenderVoice(); });
        doneTimer = Timer(() => Step(new PointEvent.PanelDone()));
        hotkeys.Pressed += OnHotkeyPressed;
        hotkeys.Released += OnHotkeyReleased;
        island.Window.Hotkey += id => { if (id == EscHotkeyId) Step(new PointEvent.Esc()); };
        store.DictationWire += OnDictationWire;
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
        lastStatusSecond = uint.MaxValue;
        overlays.Reveal();
        sampler.Start(s.AnnotateMouse);
        RegisterEsc();
        dictation.Handle(AskDictation.Event.Start.Instance);
        dictationId = null;
        finalText = new TaskCompletionSource<string>(TaskCreationOptions.RunContinuationsAsynchronously);
        island.SetVoice(View("listening"));
        int g = gesture;
        _ = StartDictation(g);
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
            dispatcher.TryEnqueue(() =>
            {
                if (g != gesture) return;
                dictation.Handle(new AskDictation.Event.StartFailed(why));
                ShowHint(why.Contains("already", StringComparison.OrdinalIgnoreCase) ? PointFlow.RefusalHint("already dictating") : why);
                Step(new PointEvent.Cancel());
            });
            return;
        }
        var mic = await recorder.StartAsync();
        dispatcher.TryEnqueue(() =>
        {
            if (g != gesture)
            {
                _ = store.Dictation.StopAsync(id);
                recorder.Cancel();
                return;
            }
            dictationId = id;
            dictation.Handle(new AskDictation.Event.Started(id));
            if (mic is { } err) { Log.Write($"point: mic: {err}"); ShowHint(err); }
            else Log.Write($"point: dictation {id} live, mic {recorder.Description}");
        });
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
        if (dictationId is { } id)
        {
            dictation.Handle(AskDictation.Event.Stop.Instance);
            try { await store.Dictation.StopAsync(id); }
            catch (Exception e) { Log.Write($"point: dictation_stop failed: {Why(e)}"); return dictation.LiveText; }
            var done = finalText?.Task ?? Task.FromResult(dictation.LiveText);
            var winner = await Task.WhenAny(done, Task.Delay(PointFlow.FinalTextMs));
            if (winner != done) Log.Write($"point: final text not flushed within {PointFlow.FinalTextMs} ms — sending the live text");
            return done.IsCompletedSuccessfully ? done.Result : dictation.LiveText;
        }
        return dictation.LiveText;
    }

    private async Task<(IReadOnlyList<string> Lines, IReadOnlyList<string> Files)> ScreenContext(PointSample[] samples, long durationMs, bool accessibility)
    {
        try
        {
            var req = new
            {
                samples = samples.Select(p => new { t_ms = p.TMs, x = (float)p.X, y = (float)p.Y, pen = p.Pen }).ToArray(),
                color = PointFlow.TrailHex,
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
        if (dictationId is { } id)
        {
            _ = store.Dictation.StopAsync(id).ContinueWith(_ => { }, TaskScheduler.Default);
        }
        dictationId = null;
        gesture++;
        working = false;
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
        deskSession = null;
        island.SetVoice(null);
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

    public void Dispose()
    {
        if (disposed) return;
        disposed = true;
        hotkeys.Pressed -= OnHotkeyPressed;
        hotkeys.Released -= OnHotkeyReleased;
        store.DictationWire -= OnDictationWire;
        store.EventReceived -= OnEvent;
        UnregisterEsc();
        sampler.Dispose();
        recorder.Cancel();
        overlays.Dispose();
    }
}
