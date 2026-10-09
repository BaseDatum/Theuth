using System.Text.Json;
using Microsoft.UI.Dispatching;
using Theuth.Companion.Core;
using Theuth.Companion.Island;
using Theuth.Companion.Rpc;
using Theuth.Companion.Ui;
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
/// <para>153 wave C (§10.1, §10.7, §12, §15): ONE machine for three mode keys — the voice key (<see cref="PointMode.Ask"/>), Dictate
/// anywhere (<see cref="PointMode.Dictate"/>) and Command (<see cref="PointMode.Command"/>). Another mode's key while a gesture is
/// live is <see cref="PointEvent.SwitchMode"/>: the recording continues, the pill relabels, the destination changes at release; each
/// held key has its own release watch (the release of either ends the hold). Each mode's key has its own gesture (§12.6: Hold ·
/// double-tap locks, or Automatic). Dictation's capture and delivery are the AnyApp controller's (W153D): this controller reports
/// the dictate key's gesture through <see cref="Hotkeys.DictateAnywhere"/> and hands words across a switch. Command sends
/// <c>[command, N s] …</c> to theuth's desk and renders the step-typed card (<see cref="CommandCard"/>): rows, the 12-step cap
/// (<c>cancel(desk, "root")</c>), approvals in the card answered by a click, Esc (deny), Enter when the card is key, or the voice
/// (<see cref="SpokenApproval"/> over a short <c>dictation_start</c>, Surface Companion). Every answer is the ONE
/// <see cref="AnswerCardState"/>: collapsed in the island's voice view, expanded (click, scroll) into <see cref="AnswerCardWindow"/>;
/// Esc while thinking freezes it with "stopped"; hides are generation-checked; the 6 s timer runs only collapsed ∧ silent ∧ finished.</para>
/// </summary>
internal sealed class PointController : IDisposable
{
    private const int EscHotkeyId = 0x0149;
    /// <summary>R3: Esc with the voice key's modifiers (Ctrl+Alt+Esc for the default) — the Esc a person can press while still holding the chord.</summary>
    private const int EscHeldHotkeyId = 0x014A;
    private const int VkEscape = 0x1B;
    /// <summary>153 §10.6 (writer's call): Enter approves only after the box has been up this long — a Return meant for another app never approves.</summary>
    private const ulong EnterApproveGuardMs = 500;

    private readonly Store store;
    private readonly DispatcherQueue dispatcher;
    private readonly IslandController island;
    private readonly Hotkeys hotkeys;
    private readonly Func<PointSettings> settings;
    private readonly OverlayWindows overlays;
    private readonly PointerSampler sampler;
    private readonly VoiceRecorder recorder;
    private readonly AskDictation dictation = new();
    /// <summary>Spoken replies (2026-10-09): the ONE shared speaker — the voice key's answer and Ask replies go through it (<see cref="Voice.Speaker"/>).</summary>
    private readonly Voice.Speaker speech;
    private readonly DateTimeOffset epoch = DateTimeOffset.Now;
    private readonly DispatcherQueueTimer tick;
    private readonly DispatcherQueueTimer hintTimer;
    private readonly DispatcherQueueTimer doneTimer;
    private readonly DispatcherQueueTimer secondTimer;
    private readonly DispatcherQueueTimer listenTimer;
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
    private bool escHeldRegistered;
    private int gesture;
    private TaskCompletionSource<string>? finalText;
    private bool disposed;

    // 153 wave C
    private PointMode mode = PointMode.Ask;          // the live (or last) gesture's mode
    private PointMode pressMode = PointMode.Ask;     // the key that opens the next gesture
    private Gesture gestureKind = Gesture.HoldLock;  // the press grammar of the live gesture (its opening key's)
    private readonly HashSet<KeyAction> held = new();
    private int modeGestureId;
    private ulong gestureStartMs;
    private bool ownCapture;                          // this controller runs the mic + dictation (ask/command), not AnyApp (dictate)
    private string prefixWords = "";                  // words handed over by a switch out of dictation
    private readonly AnswerCardState card = new();
    private CommandCardState command = CommandCardState.Empty;
    private int cardGen;
    private int windowGen;
    private bool detached;                            // the card lives in the AnswerCardWindow (since an expand)
    private AnswerCardWindow? window;
    private ApprovalListen? listen;
    private string? listenDictation;
    private bool probing;                             // --probe-command / --probe-card / --probe-states: never acting
    private bool optsRefusedLogged;

    public PointController(Store store, DispatcherQueue dispatcher, IslandController island, Hotkeys hotkeys, Func<PointSettings> settings, Voice.Speaker speaker)
    {
        this.store = store;
        speech = speaker;
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
            if (listenDictation is not null && !state.IsGesture) { CloseListen($"mic failed: {why}"); return; }
            ShowHint(why);
            // (b): a dead microphone mid-hold ends the gesture, which stops the engine dictation it started.
            if (state.IsGesture) Step(new PointEvent.Cancel());
        });
        tick = Timer(() => Step(new PointEvent.Tick(Now)));
        hintTimer = Timer(() => { hint = null; if (state is PointState.Idle && !card.Visible) island.SetVoice(null); else RenderVoice(); });
        doneTimer = Timer(OnDoneTimer);
        secondTimer = Timer(OnSecond);
        listenTimer = Timer(() => CloseListen($"{SpokenApproval.ListenSeconds} s with no answer"));
        hotkeys.Pressed += OnHotkeyPressed;
        hotkeys.Released += OnHotkeyReleased;
        hotkeys.Chorded += OnChorded;
        island.Window.Hotkey += id => { if (id is EscHotkeyId or EscHeldHotkeyId) OnEsc(); }; // plain Esc, or Esc while the chord is still held (R3)
        island.Window.VoiceTapped += () => Expand("click");
        island.Window.VoiceAction += a => AnswerApproval(a == "approve" ? Decision.Allow : Decision.Deny, "click");
        island.ModeKeysChanged += ApplyModeKeys;
        speech.Changed += OnSpeechChanged;
        store.DictationWire += OnDictationWire;
        store.PropertyChanged += OnStoreChanged;
        store.EventReceived += OnEvent; // WW1's raw stream hook (Store.WW1.cs)
        var mk = island.ModeKeys;
        if (mk.ModifierOnlyKeys || !mk.DictateAnywhereEnabled || !mk.CommandKeyEnabled) ApplyModeKeys();
        else hotkeys.ModeGate = a => island.ModeKeys.Enabled(a);
        Log.Write($"point-and-say: installed (voice key {VoiceKeyChord}, dictate anywhere {KeyText(KeyAction.DictateAnywhere)}, command {KeyText(KeyAction.Command)}; gestures ask={mk.GestureFor(PointMode.Ask).RawValue()} dictate={mk.GestureFor(PointMode.Dictate).RawValue()} command={mk.GestureFor(PointMode.Command).RawValue()}; annotate={settings().Annotate}; screen={(settings().IncludeScreen ? "on" : "off")}; speak={(settings().Speak ? "on" : "off")})");
        PointProbe.Install153(dispatcher, this);
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

    /// <summary>The chord a mode key is registered as (the AltGr fallback included), for the copy ("Ctrl+Alt+C held").</summary>
    public string KeyText(KeyAction a) => (hotkeys.EffectiveChord(a) ?? store.Keys.ChordFor(a))?.Symbol ?? "unassigned";

    private string ModeKeyText => KeyText(KeyTable.ActionOf(mode));

    public PointState State => state;

    public PointMode Mode => mode;

    public AnswerCardState Card => card;

    public CommandCardState Command => command;

    private ulong Now => (ulong)Math.Max(0, (DateTimeOffset.Now - epoch).TotalMilliseconds);

    /// <summary>The controller's clock (ms since start), for the probes.</summary>
    internal ulong Clock => Now;

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

    /// <summary>Settings › Keys changed a gesture, a row's switch or the modifier-only option: the hotkeys re-apply with the new gate.</summary>
    private void ApplyModeKeys()
    {
        var mk = island.ModeKeys;
        hotkeys.ModeGate = a => island.ModeKeys.Enabled(a);
        hotkeys.ModifierOnlyEnabled = mk.ModifierOnlyKeys;
        if (hotkeys.AnyHeld) { Log.Write("point: mode keys changed while a key is held — re-applying after the release"); dispatcher.TryEnqueue(DispatcherQueuePriority.Low, ApplyModeKeys); return; }
        var before = hotkeys.Notice;
        hotkeys.Apply(hotkeys.Applied ?? store.Keys);
        if (hotkeys.Notice != before) store.HotkeyNotice = hotkeys.Notice;
        Log.Write($"point: mode keys applied (dictate anywhere {(mk.DictateAnywhereEnabled ? "on" : "off")}, command {(mk.CommandKeyEnabled ? "on" : "off")}, modifier-only {(mk.ModifierOnlyKeys ? "on" : "off")})");
    }

    // MARK: the keys (153 §10.1: one machine, three mode keys)

    private void OnHotkeyPressed(KeyAction a)
    {
        if (KeyTable.ModeOf(a) is not { } m) return;
        if (state.IsGesture)
        {
            held.Add(a);
            if (m != mode)
            {
                Step(new PointEvent.SwitchMode(m));
                return;
            }
            Step(new PointEvent.Pressed()); // the same key: the lock's send, the second tap, or an auto-repeat the machine ignores
            return;
        }
        pressMode = m;
        held.Clear();
        held.Add(a);
        Step(new PointEvent.Pressed());
    }

    private void OnHotkeyReleased(KeyAction a, TimeSpan heldFor)
    {
        if (KeyTable.ModeOf(a) is null) return;
        if (!held.Remove(a)) return; // not a key of this gesture
        Step(new PointEvent.Released()); // VoiceKeyGate twin: the release of either held mode key ends the hold
    }

    /// <summary>153 §12.6 chord protection: a modifier-only key's hold saw another key — that was a shortcut, not the key.</summary>
    private void OnChorded(KeyAction a)
    {
        if (KeyTable.ModeOf(a) is null || !held.Contains(a)) return;
        held.Remove(a);
        Log.Write($"point: {a.RawValue()} chorded with another key — cancelling the gesture (not a release)");
        if (state.IsGesture) Step(new PointEvent.Cancel());
    }

    /// <summary>§5.4 D: the standing refusal — off, disconnected, a meeting talking, dictation busy.</summary>
    private string? Refused()
    {
        var s = settings();
        if (!s.Enabled && pressMode == PointMode.Ask) return "disabled";
        if (store.Connection != RpcClient.State.Open) return "disconnected";
        if (island.Snapshot.State == "Recording" && island.Snapshot.Listening) return "talking";
        if (store.Ask.Dictation.IsActive) return "already dictating";
        return null;
    }

    /// <summary>Simulated press/release for the probe (<c>--point-probe</c>) and tests on build-B.</summary>
    public void Simulate(PointEvent e) => Step(e);

    /// <summary>153 probes: a simulated press/release of a mode key, through the same routing as the real hotkey.</summary>
    public void SimulateKey(KeyAction a, bool down)
    {
        if (down) OnHotkeyPressed(a);
        else OnHotkeyReleased(a, TimeSpan.Zero);
    }

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
            if (state is PointState.Locked or PointState.Toggled) Step(new PointEvent.Pressed());
            return;
        }
        // Idle/Replying → Armed (Start) → a tap (Released < HOLD) → Tap1 → a second press inside the window → Locked.
        pressMode = PointMode.Ask;
        held.Clear();
        var t = Now;
        Step(new PointEvent.Pressed());
        if (state is not PointState.Armed) return; // refused
        gestureKind = Gesture.HoldLock; // the glyph's lock is the hold/double-tap grammar whatever the key's gesture
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
        var refused = e is PointEvent.Pressed && !state.IsGesture ? Refused() : null;
        // §10.5 / §12.2: a press while the desk works on this card's answer opens a follow-up gesture (it steers the turn).
        bool followUp = state is PointState.Sending && deskSession is not null && working && pressMode != PointMode.Dictate;
        var g = state.IsGesture ? gestureKind : island.ModeKeys.GestureFor(pressMode);
        var (next, effects) = PointMachine.Step(state, e, Now, refused, g, followUp);
        if (next != state || e is PointEvent.SwitchMode) Log.Write($"point: {state.Json} --{EventName(e)}--> {next.Json}{(effects.Count > 0 ? " fx=" + string.Join(",", effects.Select(f => f.GetType().Name)) : "")} mode={(e is PointEvent.Pressed && !state.IsGesture ? pressMode : mode).RawValue()} gesture={g.RawValue()}");
        state = next;
        foreach (var fx in effects) Run(fx);
        if (state is PointState.Idle && !(card.Visible && card.Phase == CardPhase.Failed)) UnregisterEsc(); // (Sending, Cancel) has no effect of its own: Esc must not stay registered system-wide while idle
        // The machine's timers: Armed → a tick at the hold threshold (200 ms, or 400 ms Automatic); Tap1 → a tick just past DOUBLE_TAP_MS.
        switch (state)
        {
            case PointState.Armed a:
                ulong at = a.T0 + PointMachine.HoldThreshold(gestureKind);
                After(tick, (int)(at - Math.Min(Now, at)) + 1);
                break;
            case PointState.Tap1 t: After(tick, (int)(t.Released + PointMachine.DoubleTapMs - Math.Min(Now, t.Released + PointMachine.DoubleTapMs)) + 2); break;
            default: tick.Stop(); break;
        }
    }

    private static string EventName(PointEvent e) => e is PointEvent.SwitchMode s ? $"SwitchMode({s.To.RawValue()})" : e.GetType().Name;

    private void Run(PointEffect fx)
    {
        switch (fx)
        {
            case PointEffect.Start: Start(); break;
            case PointEffect.ShowLock:
                locked = true;
                if (!ownCapture) hotkeys.RaiseGesture(new ModeGesture.Locked(modeGestureId, mode));
                else RenderVoice();
                break;
            case PointEffect.Stop s:
                if (!ownCapture) StopDictate(s.Reason);
                else _ = StopAndSend(s.Reason);
                break;
            case PointEffect.Discard d: Discard(d.Reason, d.Hint); break;
            case PointEffect.Conceal: Conceal(); break;
            case PointEffect.Refuse r:
                ShowHint(PointFlow.RefusalHint(r.Reason));
                Log.Write($"point: refused — {r.Reason} ({pressMode.RawValue()})");
                if (pressMode != PointMode.Ask) hotkeys.RaiseGesture(new ModeGesture.Refused(modeGestureId + 1, pressMode, r.Reason));
                break;
            case PointEffect.SwitchMode sw: SwitchTo(sw.To); break;
        }
    }

    // MARK: effects

    private void Start()
    {
        bool followUp = card.Visible && card.Phase != CardPhase.Failed && deskSession is not null && pressMode != PointMode.Dictate && SourceOf(pressMode) == card.Source;
        mode = pressMode;
        gestureKind = island.ModeKeys.GestureFor(mode);
        modeGestureId++;
        gestureStartMs = Now;
        prefixWords = "";
        CloseListen("a new gesture");
        if (mode == PointMode.Dictate)
        {
            gesture++;
            locked = false;
            live = "";
            hint = null;
            ownCapture = false;
            doneTimer.Stop();
            hintTimer.Stop();
            speech.Stop(SpeechStop.NewGesture);
            HideCard("a dictate-anywhere gesture");
            RegisterEsc();
            Log.Write($"point: start — dictate anywhere gesture {modeGestureId} ({KeyText(KeyAction.DictateAnywhere)}, {gestureKind.RawValue()}) → the AnyApp controller{(hotkeys.HasDictateAnywhereSubscriber ? "" : " (none subscribed)")}");
            hotkeys.RaiseGesture(new ModeGesture.Started(modeGestureId, PointMode.Dictate, gestureStartMs));
            if (!hotkeys.HasDictateAnywhereSubscriber)
            {
                ShowHint("Dictate anywhere isn't available in this build");
                dispatcher.TryEnqueue(() => { if (state.IsGesture && mode == PointMode.Dictate) Step(new PointEvent.Cancel()); });
            }
            return;
        }
        StartOwnCapture(followUp, resetCard: true);
        Log.Write($"point: start — {mode.RawValue()} gesture {modeGestureId} ({ModeKeyText}, {gestureKind.RawValue()}){(followUp ? " follow-up on the card" : "")} overlays={overlays.Windows.Count} annotate={settings().Annotate}");
        if (mode == PointMode.Command) hotkeys.RaiseGesture(new ModeGesture.Started(modeGestureId, PointMode.Command, gestureStartMs));
    }

    /// <summary>The ask/command capture: overlays, the sampler, Esc, the mic + <c>dictation_start</c>, the card in listening.</summary>
    private void StartOwnCapture(bool followUp, bool resetCard)
    {
        gesture++;
        ownCapture = true;
        locked = false;
        live = "";
        if (!followUp) answer = "";
        hint = null;
        if (!followUp) working = false;
        doneTimer.Stop();
        hintTimer.Stop();
        var s = settings();
        speech.Stop(SpeechStop.NewGesture); // a new voice-key gesture cuts whatever is being read aloud (voice key or Ask)
        lastStatusSecond = uint.MaxValue;
        UpdateOverlays();
        sampler.Start(s.AnnotateMouse);
        RegisterEsc();
        dictation.Handle(AskDictation.Event.Start.Instance);
        dictationId = null;
        finalText = new TaskCompletionSource<string>(TaskCreationOptions.RunContinuationsAsynchronously);
        if (resetCard)
        {
            if (!followUp)
            {
                command = CommandCardState.Empty;
                if (detached) HideCard("a new gesture");
            }
            cardGen = card.Begin(SourceOf(mode), Now, followUp);
            if (mode == PointMode.Command) Observe(new CommandInput.Listening(Now));
        }
        RenderVoice();
        int g = lease.Begin();
        starting = StartDictation(g);
    }

    private static CardSource SourceOf(PointMode m) => m == PointMode.Command ? CardSource.Command : CardSource.VoiceKey;

    /// <summary>The screen goes with the voice key per Settings › Voice key, with a command only per Settings › Command "Include the screen" (H3-C1, default off).</summary>
    private bool IncludeScreen(PointMode m) => m == PointMode.Command ? island.ModeKeys.CommandIncludeScreen : settings().IncludeScreen;

    private bool overlaysShown;

    /// <summary>The pointer trail is drawn only when the screen goes with the request (pointing means nothing otherwise).</summary>
    private void UpdateOverlays()
    {
        bool want = ownCapture && state.IsGesture && (mode == PointMode.Ask || IncludeScreen(mode));
        if (want && !overlaysShown) overlays.Reveal();
        else if (!want && overlaysShown) overlays.Hide();
        overlaysShown = want;
    }

    /// <summary>
    /// 153 §10.1 (R2): the live gesture changes mode. Ask ↔ Command keep this controller's recording and relabel; into Dictate the
    /// words so far go to the AnyApp controller and the mic + the engine's one dictation are freed for it; out of Dictate the AnyApp
    /// controller hands its words back (<see cref="Hotkeys.TakeDictateWords"/>) and this controller records the rest.
    /// </summary>
    private void SwitchTo(PointMode to)
    {
        var from = mode;
        if (from == to) return;
        Log.Write($"point: switch mode {from.RawValue()} → {to.RawValue()} mid-gesture (state {state.Json}; the recording continues; held={string.Join("+", held.Select(h => h.RawValue()))})");
        if (from != PointMode.Dictate && to != PointMode.Dictate)
        {
            mode = to;
            card.Relabel(SourceOf(to));
            if (to == PointMode.Command && command.Current is not { Phase: CommandPhase.Listening }) Observe(new CommandInput.Listening(gestureStartMs));
            UpdateOverlays();
            UnregisterEsc();
            RegisterEsc(); // the held-Esc twin follows the new key's modifiers
            RenderVoice();
            hotkeys.RaiseGesture(new ModeGesture.Switched(modeGestureId, from, to, ""));
            return;
        }
        if (to == PointMode.Dictate)
        {
            var words = Join(prefixWords, dictation.LiveText.Length > 0 ? dictation.LiveText : live);
            StopOwnCapture();
            HideCard("switched to dictate anywhere");
            mode = PointMode.Dictate;
            ownCapture = false;
            Log.Write($"point: handed {words.Length} chars to the AnyApp controller (dictation freed)");
            hotkeys.RaiseGesture(new ModeGesture.Switched(modeGestureId, from, PointMode.Dictate, words));
            if (!hotkeys.HasDictateAnywhereSubscriber)
            {
                ShowHint("Dictate anywhere isn't available in this build");
                dispatcher.TryEnqueue(() => { if (state.IsGesture && mode == PointMode.Dictate) Step(new PointEvent.Cancel()); });
            }
            return;
        }
        var handed = hotkeys.TakeWords(modeGestureId);
        hotkeys.RaiseGesture(new ModeGesture.Switched(modeGestureId, PointMode.Dictate, to, ""));
        mode = to;
        StartOwnCapture(followUp: false, resetCard: true);
        prefixWords = handed;
        live = handed;
        RenderVoice();
        Log.Write($"point: took {handed.Length} chars from the AnyApp controller; {to.RawValue()} records the rest");
    }

    /// <summary>An engine before 153 refusing <c>dictation_start</c>'s trailing opts (invalid params / unknown shape) — never a refusal such as "already dictating".</summary>
    private static bool OptsRefused(RpcException e) =>
        e.Code is -32602 or -32601 || (e.Message.Contains("param", StringComparison.OrdinalIgnoreCase) && !e.Message.Contains("dictat", StringComparison.OrdinalIgnoreCase));

    private static string Join(string a, string b) => string.Join(" ", new[] { a.Trim(), b.Trim() }.Where(x => x.Length > 0));

    /// <summary>A switch into dictation: everything this controller captured stops (no send); the engine's one dictation is freed.</summary>
    private void StopOwnCapture()
    {
        sampler.Stop();
        overlays.Hide();
        overlaysShown = false;
        recorder.Cancel();
        if (lease.Close() is { } id)
        {
            _ = store.Dictation.StopAsync(id).ContinueWith(t => { if (t.IsFaulted) Log.Write($"point: dictation_stop failed: {t.Exception?.GetBaseException().Message}"); }, TaskScheduler.Default);
        }
        dictationId = null;
        ownCapture = false;
    }

    /// <summary>The dictate-anywhere gesture ended with a release/tap: the AnyApp controller delivers; this machine goes back to idle.</summary>
    private void StopDictate(string reason)
    {
        held.Clear();
        UnregisterEsc();
        ulong heldMs = Now > gestureStartMs ? Now - gestureStartMs : 0;
        Log.Write($"point: dictate anywhere gesture {modeGestureId} stopped ({reason}, {heldMs} ms) → the AnyApp controller delivers");
        hotkeys.RaiseGesture(new ModeGesture.Stopped(modeGestureId, PointMode.Dictate, reason, heldMs));
        dispatcher.TryEnqueue(() => { if (state is PointState.Sending && mode == PointMode.Dictate) Step(new PointEvent.Cancel()); });
    }

    private async Task StartDictation(int g)
    {
        string id;
        try
        {
            // 153 BC hand-off (Windows twin): the companion's dictation_start opts — [16000, {surface: companion, destination:
            // agent_prompt, commands: [], press_ago_ms}]; an engine before 153 refuses the trailing param → the one-argument call.
            var opts = new DictationOpts { Surface = Surface.Companion, Destination = Destination.AgentPrompt, PressAgoMs = (uint)Math.Min(uint.MaxValue, Now > gestureStartMs ? Now - gestureStartMs : 0) };
            try { id = await store.Client.DictationStartAsync(AskAudio.SampleRate, opts); }
            catch (RpcException pe) when (OptsRefused(pe))
            {
                if (!optsRefusedLogged) { optsRefusedLogged = true; Log.Write($"point: dictation_start with opts refused ({pe.Code}) — an engine before 153: the one-argument call"); }
                id = await store.Dictation.StartAsync();
            }
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
        if (dictationId is { } id)
        {
            _ = store.Dictation.AudioAsync(id, pcm).ContinueWith(t => { if (t.IsFaulted) Log.Write($"point: dictation_audio failed: {t.Exception?.GetBaseException().Message}"); }, TaskScheduler.Default);
            return;
        }
        if (listenDictation is { } lid)
        {
            _ = store.Dictation.AudioAsync(lid, pcm).ContinueWith(t => { if (t.IsFaulted) Log.Write($"point: approval listen audio failed: {t.Exception?.GetBaseException().Message}"); }, TaskScheduler.Default);
        }
    }

    private uint lastStatusSecond = uint.MaxValue;

    private void OnSample(PointSample s)
    {
        if (overlaysShown)
        {
            overlays.Append(s.X, s.Y, s.Pen);
            overlays.Present();
        }
        // The voice mode's status clock changes once a second; re-render only then (not at 60 Hz).
        uint second = s.TMs / 1000;
        if (second != lastStatusSecond && state.IsGesture)
        {
            lastStatusSecond = second;
            RenderVoice();
        }
    }

    private async Task StopAndSend(string reason)
    {
        int g = gesture;
        var sendMode = mode;
        held.Clear();
        sampler.Stop();
        tick.Stop();
        UnregisterEsc(); // Esc is registered only between press and send; the Sending state still takes it from the island
        RegisterEsc();
        var samples = sampler.Samples.ToArray();
        long durationMs = samples.Length > 0 ? samples[^1].TMs : (long)(Now > gestureStartMs ? Now - gestureStartMs : 0);
        // D13: every overlay hidden and one dispatcher turn awaited before the engine captures.
        overlays.Hide();
        overlaysShown = false;
        await Yield();
        recorder.Stop();
        card.Released(Now);
        var s = settings();
        var screenTask = IncludeScreen(sendMode) ? ScreenContext(samples, durationMs, s.AccessibilityOn) : Task.FromResult<(IReadOnlyList<string>, IReadOnlyList<string>)>((Array.Empty<string>(), Array.Empty<string>()));
        var textTask = FinalText();
        RenderVoice();
        await Task.WhenAll(screenTask, textTask);
        if (g != gesture || state is not PointState.Sending) return;
        var text = Join(prefixWords, textTask.Result);
        var (lines, files) = screenTask.Result;
        Log.Write($"point: stop ({reason}, {sendMode.RawValue()}) — {samples.Length} samples over {durationMs} ms, text {text.Length} chars, screen lines={lines.Count} files={files.Count}");
        if (text.Trim().Length == 0)
        {
            card.FinalText("", Now);
            HideCard("nothing recognized");
            ShowHint("Nothing recognized");
            Step(new PointEvent.Cancel());
            AuditDictation("stop (nothing recognized)");
            return;
        }
        card.FinalText(text, Now);
        string target;
        try
        {
            target = await DeskAsync();
        }
        catch (Exception e)
        {
            Log.Write($"point: desk_open failed: {Why(e)}");
            ShowHint(PointFlow.RefusalHint("hub"));
            HideCard("desk_open failed");
            Step(new PointEvent.Cancel());
            AuditDictation("stop (desk_open failed)");
            return;
        }
        if (g != gesture) return;
        // §10.2 (W-l): the command profile's marker; after a cap stop the line says "continue" (§10.4).
        var body = sendMode == PointMode.Command
            ? CommandCard.ComposeBody(CommandCard.ContinueText(command, text), durationMs, lines)
            : PointFlow.ComposeBody(text, durationMs, lines);
        var clientId = Guid.NewGuid().ToString();
        if (sendMode == PointMode.Command) Observe(new CommandInput.Heard(text, Now));
        try
        {
            // `send_input(session, text, files, delivery, id)`: a busy desk is steered by the engine's default delivery.
            await store.Client.CallAsync<string>("send_input", target, body, files, null, clientId);
        }
        catch (Exception e)
        {
            var why = Why(e);
            Log.Write($"point: send_input failed: {why}");
            if (g != gesture) return;
            // D13: a failed send is the failed state — it stays until Esc (Replying keeps Esc registered).
            deskSession = target;
            card.Failed($"send_input: {why}", Now);
            if (sendMode == PointMode.Command) Observe(new CommandInput.Error(why));
            working = false;
            Step(new PointEvent.PanelDone());
            RenderVoice();
            AuditDictation("stop (send_input failed)");
            return;
        }
        if (g != gesture) return;
        deskSession = target;
        card.Session = target;
        working = true;
        live = text;
        card.TurnStarted(Now);
        Log.Write($"point: sent to desk {target} ({body.Length} chars, {files.Count} files, {sendMode.RawValue()}) id={clientId}: {Clip(body.Split('\n')[0], 120)}");
        After(secondTimer, 1000);
        RenderVoice();
        AuditDictation("send");
    }

    private static string Clip(string s, int max) => s.Length <= max ? s : s[..(max - 1)] + "…";

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
        if (id is null)
        {
            dictationId = null;
            return dictation.LiveText;
        }
        // dictationId stays set until the final text is in: OnDictationWire filters DictationFinished by it.
        dictation.Handle(AskDictation.Event.Stop.Instance);
        try { await store.Dictation.StopAsync(id); }
        catch (Exception e) { Log.Write($"point: dictation_stop failed: {Why(e)}"); dictationId = null; return dictation.LiveText; }
        var done = finalText?.Task ?? Task.FromResult(dictation.LiveText);
        var winner = await Task.WhenAny(done, Task.Delay(PointFlow.FinalTextMs));
        if (winner != done) Log.Write($"point: final text not flushed within {PointFlow.FinalTextMs} ms — sending the live text");
        dictationId = null;
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
        held.Clear();
        if (!ownCapture && mode == PointMode.Dictate)
        {
            tick.Stop();
            UnregisterEsc();
            gesture++;
            Log.Write($"point: dictate anywhere gesture {modeGestureId} discarded ({reason})");
            hotkeys.RaiseGesture(new ModeGesture.Discarded(modeGestureId, PointMode.Dictate, reason, hintText));
            if (hintText is { } dh && reason != "cancel") ShowHint(dh);
            return;
        }
        sampler.Stop();
        tick.Stop();
        overlays.Hide();
        overlaysShown = false;
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
        speech.Stop(SpeechStop.Esc); // 149 R3: Esc also cuts the spoken answer
        deskSession = null;
        CloseListen($"discard ({reason})");
        HideCard($"discard ({reason})");
        if (mode == PointMode.Command) hotkeys.RaiseGesture(new ModeGesture.Discarded(modeGestureId, PointMode.Command, reason, hintText));
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
        AuditDictation($"discard ({reason})");
    }

    /// <summary>
    /// 149 voice-key fix, the permanent proof line (d): ~1.2 s after a gesture ends, ask the engine what is running —
    /// <c>point: dictation_status after … → active=none</c> is the invariant holding; an id there is a leak to chase.
    /// </summary>
    private void AuditDictation(string after)
    {
        _ = Task.Run(async () =>
        {
            await Task.Delay(1200);
            string verdict;
            try
            {
                var raw = await store.Client.CallRawAsync("dictation_status");
                string? active = null;
                try
                {
                    using var doc = JsonDocument.Parse(raw);
                    if (doc.RootElement.ValueKind == JsonValueKind.Object && doc.RootElement.TryGetProperty("active", out var a) && a.ValueKind == JsonValueKind.String) active = a.GetString();
                }
                catch (JsonException) { }
                verdict = active is null ? "active=none" : $"active={active} (LEAK: a dictation is still running)";
            }
            catch (Exception e) { verdict = $"unavailable ({Why(e)})"; }
            Log.Write($"point: dictation_status after {after} → {verdict}");
        });
    }

    private void Conceal()
    {
        doneTimer.Stop();
        secondTimer.Stop();
        UnregisterEsc();
        working = false;
        CloseListen("conceal");
        // 149 R3: the card is gone but the voice may still be mid-sentence — SpeechPlayer follows deskSession, so keep it set
        // until the utterance finishes (SpeechFinished) or the next gesture; a null here would drop the rest of the packets.
        if (!speech.IsSpeaking) deskSession = null;
        HideCard("conceal");
        island.SetVoice(null);
    }

    /// <summary><c>--speech-probe</c>: open the desk, ask the engine to speak a line, and let <see cref="SpeechPlayer"/> report.</summary>
    public async Task ProbeSpeakAsync(string text)
    {
        try
        {
            var target = await DeskAsync();
            deskSession = target;
            speech.Speak(target, text, SpeechSource.VoiceKey); // the shared speaker: SpeechStarted names the utterance, the player reports
            Log.Write($"speech-probe: speak({target}, {text.Length} chars) through the shared speaker");
        }
        catch (Exception e) { Log.Write($"speech-probe: failed: {Why(e)}"); }
    }

    private void ShowHint(string h)
    {
        hint = h;
        if (detached && card.Visible) { RenderCard(); After(hintTimer, PointFlow.HintMs); return; }
        island.SetVoice(View(state is PointState.Idle && !card.Visible ? "hint" : Phase()));
        After(hintTimer, PointFlow.HintMs);
    }

    // MARK: the desk's turn → the answer card (§12) and the command card (§10)

    private void OnEvent(string type, byte[] json)
    {
        if (type.StartsWith("Speech", StringComparison.Ordinal)) return; // the shared Speaker plays these; they never reach the card
        if (type == "ApprovalResolved")
        {
            // ApprovalResolved carries no session: CommandCard matches it to the open box by id.
            if (command.Approval is not null && CommandCard.FromEvent(type, json, deskSession ?? "", Now) is { } resolved)
            {
                Observe(resolved);
                RenderVoice();
            }
            return;
        }
        if (deskSession is null) return;
        StreamDelta d;
        try { d = StreamDelta.Decode(json); } catch (Exception) { return; }
        if (d.Session != deskSession) return;
        bool commandCard = card.Source == CardSource.Command;
        if (commandCard && CommandCard.FromEvent(type, json, deskSession, Now) is { } input) Observe(input);
        switch (type)
        {
            case "TurnStarted":
                working = true;
                toolName = null;
                card.TurnStarted(Now);
                After(secondTimer, 1000);
                RenderVoice();
                break;
            case "TextDelta":
                answer += d.Text ?? "";
                card.TextDelta(d.Text ?? "", Now);
                RenderVoice();
                break;
            case "ToolCallStarted":
                working = true;
                toolName = ToolNameOf(json);
                card.ToolStarted(CallIdOf(json), toolName ?? "tool");
                RenderVoice();
                break;
            case "ToolCallFinished":
                toolName = null;
                card.ToolFinished(CallIdOf(json), IsErrorOf(json));
                RenderVoice();
                break;
            case "ApprovalRequested":
                // §10.6: the box is in the card (command); the voice key's card shows the Attention panel's approval as before.
                if (commandCard)
                {
                    Log.Write($"point: approval in the command card — {command.Approval?.Question} {command.Approval?.RiskLine}");
                    if (!detached) Expand("approval");
                    else window?.TakeKey();
                    RenderVoice();
                }
                break;
            case "Error":
                answer = d.Message ?? "the desk failed";
                working = false;
                secondTimer.Stop();
                card.Failed(d.Message ?? "the desk failed", Now);
                RenderVoice();
                if (state is PointState.Sending) Step(new PointEvent.PanelDone()); // failed never auto-hides: no done timer (D13)
                break;
            case "TurnFinished":
                working = false;
                secondTimer.Stop();
                card.TurnFinished(d.StopReason, Now);
                bool speakIt = card.Phase == CardPhase.Answer && !card.Latest.Stopped
                    && SpeakerModel.ShouldSpeak(SpeechSource.VoiceKey, settings().Speak, askSpeak: false, answer, replaying || probing);
                Log.Write($"point: desk turn finished ({d.StopReason}) — {answer.Length} chars, card {card.Phase.ToString().ToLowerInvariant()}{(commandCard ? $", command {command.Phase.ToString().ToLowerInvariant()} {CommandCard.Summary(command.Current?.Steps ?? Array.Empty<CommandStep>())}" : "")}{(speakIt ? ", speaking" : settings().Speak ? "" : " (Voice key › Speak the answer is off)")}");
                toolName = null;
                if (speakIt) speech.Speak(deskSession, answer, SpeechSource.VoiceKey); // final text only, never a partial
                RenderVoice();
                if (state is PointState.Sending) Step(new PointEvent.PanelDone()); // Sending → Replying
                ArmDone();
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

    private static string CallIdOf(byte[] json)
    {
        try
        {
            using var doc = JsonDocument.Parse(json);
            return doc.RootElement.TryGetProperty("call_id", out var n) && n.ValueKind == JsonValueKind.String ? n.GetString() ?? "" : "";
        }
        catch (JsonException) { return ""; }
    }

    private static bool IsErrorOf(byte[] json)
    {
        try
        {
            using var doc = JsonDocument.Parse(json);
            return doc.RootElement.TryGetProperty("is_error", out var n) && n.ValueKind == JsonValueKind.True;
        }
        catch (JsonException) { return false; }
    }

    /// <summary>One command-card step; its effects run here (the cap's cancel, the approval listen, <c>resolve_approval</c>).</summary>
    private void Observe(CommandInput input)
    {
        var (next, effects) = CommandCard.Observe(command, input);
        command = next;
        card.ApprovalOpen = command.Approval is not null; // H3-C1: no 6 s timer while a box is open
        foreach (var fx in effects)
        {
            switch (fx)
            {
                case CommandEffect.CancelTurn c:
                    Log.Write($"point: command {(c.Why == "cap" ? $"cap — {CommandCard.MaxSteps} steps ran, the {CommandCard.MaxSteps + 1}th call cancels the turn" : "stopped (Esc)")} → cancel({deskSession}, root)");
                    if (!probing && deskSession is { } ds) _ = store.Client.CancelAsync(ds, CancelScope.Root).ContinueWith(t => { if (t.IsFaulted) Log.Write($"point: cancel failed: {t.Exception?.GetBaseException().Message}"); }, TaskScheduler.Default);
                    if (c.Why == "cap") card.Stop(Now);
                    break;
                case CommandEffect.OpenListen o:
                    _ = StartListen(o.ApprovalId);
                    break;
                case CommandEffect.CloseListen:
                    CloseListen("the box closed");
                    break;
                case CommandEffect.ResolveApproval r:
                    Log.Write($"point: resolve_approval({r.Id}, {r.Decision.RawValue()}) via {r.Via}");
                    if (!probing)
                    {
                        _ = store.Client.ResolveApprovalAsync(r.Id, r.Decision).ContinueWith(t => dispatcher.TryEnqueue(() =>
                        {
                            if (t.IsFaulted) Log.Write($"point: resolve_approval failed: {t.Exception?.GetBaseException().Message}");
                            else Log.Write($"point: resolve_approval({r.Id}) → {(t.Result ? "applied" : "already answered (the Attention panel or the timeout was first)")}");
                        }), TaskScheduler.Default);
                    }
                    break;
            }
        }
    }

    /// <summary>
    /// <c>--point-replay</c>: the answer card through the real path without a model — a recorded desk sequence (TurnStarted ·
    /// ToolCallStarted/Finished · TextDelta × n · TurnFinished) fed into <c>Store.HandleEvent</c> (so it arrives through
    /// <c>EventReceived</c> like the engine's stream) for a fake desk session after a simulated send. <c>speak</c> is skipped.
    /// 153: <paramref name="asMode"/> picks the card (Command → the step-typed card; approvals render and are never resolved).
    /// </summary>
    public async Task ReplayAsync(string text, IReadOnlyList<string> jsonLines, int gapMs = 350, PointMode asMode = PointMode.Ask, string session = "replay-desk")
    {
        gesture++;
        replaying = true;
        probing = true;
        mode = asMode;
        HideCard("replay");
        command = CommandCardState.Empty;
        cardGen = card.Begin(SourceOf(asMode), Now);
        if (asMode == PointMode.Command) Observe(new CommandInput.Listening(Now));
        state = new PointState.Sending(Now);
        deskSession = session;
        card.Session = session;
        desk = deskSession;
        live = text;
        answer = "";
        working = true;
        hint = null;
        card.Released(Now);
        card.FinalText(text, Now);
        if (asMode == PointMode.Command) Observe(new CommandInput.Heard(text, Now));
        RenderVoice();
        Log.Write($"point-replay: {jsonLines.Count} events for {deskSession} ({asMode.RawValue()})");
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
        if (w.Id is not { } id) return;
        if (listenDictation == id)
        {
            OnListenWire(w);
            return;
        }
        if (dictationId != id) return;
        switch (w.Type)
        {
            case "DictationText":
                // 153 §4.4 / BC row: the companion shows `shaped ?? text` (an end command alone shapes to "").
                dictation.Handle(new AskDictation.Event.Text(id, w.Utterance ?? 0, w.DisplayText ?? "", w.IsFinal ?? false));
                live = Join(prefixWords, dictation.LiveText);
                RenderVoice();
                break;
            case "DictationLevel":
                dictation.Handle(new AskDictation.Event.Level(id, w.Levels ?? new ValueList<float>()));
                break;
            case "DictationFinished":
                dictation.Handle(new AskDictation.Event.Finished(id, w.Reason?.Reason == "error" ? w.Reason.Message ?? "dictation failed" : null));
                live = Join(prefixWords, dictation.LiveText);
                finalText?.TrySetResult(dictation.FinalText.Length > 0 ? dictation.FinalText : dictation.LiveText);
                break;
        }
    }

    // MARK: §10.6 approvals: keys, clicks and the voice

    /// <summary>The box's answer from Enter / Esc / a click / the voice; Enter counts only after the box has been up a moment.</summary>
    private void AnswerApproval(Decision decision, string via)
    {
        if (command.Approval is not { Answered: null } box) return;
        if (via == "enter" && Now < box.AtMs + EnterApproveGuardMs)
        {
            Log.Write($"point: Enter {Now - box.AtMs} ms after the box appeared — ignored (guard {EnterApproveGuardMs} ms)");
            return;
        }
        Observe(new CommandInput.Answer(decision, via));
        RenderVoice();
    }

    /// <summary>
    /// §10.6: while the box is up, a short dictation on the companion's mic (<c>dictation_start</c> with Surface Companion — an engine
    /// before 153 takes the one-argument call) listens for <see cref="SpokenApproval.ListenSeconds"/>; the first final utterance
    /// is decided by <see cref="SpokenApproval.Decide"/>. Never during a gesture (the gesture owns the mic) and never in a probe.
    /// </summary>
    private async Task StartListen(string approvalId)
    {
        if (probing || listen is not null || state.IsGesture) return;
        listen = new ApprovalListen(approvalId, Now);
        After(listenTimer, SpokenApproval.ListenSeconds * 1000 + 50);
        string id;
        try
        {
            try { id = await store.Client.DictationStartAsync(AskAudio.SampleRate, new DictationOpts { Surface = Surface.Companion, Destination = Destination.AgentPrompt }); }
            catch (RpcException e) when (OptsRefused(e))
            {
                Log.Write($"point: dictation_start with opts refused ({e.Code}) — an engine before 153: the one-argument call");
                id = await store.Dictation.StartAsync();
            }
        }
        catch (Exception e)
        {
            Log.Write($"point: approval listen could not start: {Why(e)} (the keys and the buttons still answer)");
            dispatcher.TryEnqueue(() => { if (listen?.ApprovalId == approvalId) { listen = null; listenTimer.Stop(); } });
            return;
        }
        var mic = await recorder.StartAsync();
        await OnDispatcher(() =>
        {
            if (listen is not { } l || l.ApprovalId != approvalId || l.Closed || state.IsGesture)
            {
                recorder.Cancel();
                _ = store.Dictation.StopAsync(id).ContinueWith(_ => { }, TaskScheduler.Default);
                return;
            }
            listenDictation = id;
            if (mic is { } err)
            {
                Log.Write($"point: approval listen: mic {err}");
                CloseListen("no microphone");
                return;
            }
            Log.Write($"point: approval listen open — dictation {id}, {SpokenApproval.ListenSeconds} s, say “yes” / “no”");
        });
    }

    private void OnListenWire(DictationEvent w)
    {
        if (listen is not { } l) return;
        if (w.Type == "DictationText" && w.IsFinal == true)
        {
            var heard = w.Text ?? "";
            var decision = l.Final(heard, Now);
            Log.Write($"point: approval listen heard “{heard}” → {(decision is { } d ? d.RawValue() : "no match")}");
            if (decision is { } dd)
            {
                AnswerApproval(dd, "voice");
                CloseListen("answered by voice");
            }
            else if (SpokenApproval.Words(heard).Count > 0)
            {
                Observe(new CommandInput.Unheard());
                RenderVoice();
            }
        }
        else if (w.Type == "DictationFinished")
        {
            CloseListen("the dictation ended");
        }
    }

    private void CloseListen(string why)
    {
        listenTimer.Stop();
        if (listen is null && listenDictation is null) return;
        listen?.Close();
        listen = null;
        if (listenDictation is { } id)
        {
            listenDictation = null;
            if (!state.IsGesture) recorder.Cancel();
            _ = store.Dictation.StopAsync(id).ContinueWith(t => { if (t.IsFaulted) Log.Write($"point: approval listen stop failed: {t.Exception?.GetBaseException().Message}"); }, TaskScheduler.Default);
        }
        Log.Write($"point: approval listen closed ({why})");
    }

    // MARK: the card: the island's voice view (collapsed) and the answer card window (expanded, §12.3)

    private string Phase() => state switch
    {
        PointState.Armed or PointState.Holding or PointState.Locked or PointState.Tap1 or PointState.Toggled => "listening",
        PointState.Sending => working ? "working" : "sending",
        PointState.Replying => "done",
        _ => card.Visible ? CardPhaseWord() : "hint",
    };

    private string CardPhaseWord() => card.Phase switch
    {
        CardPhase.Listening => "listening",
        CardPhase.Finishing => "sending",
        CardPhase.Thinking => "working",
        _ => "done",
    };

    private void RenderVoice()
    {
        if (detached && card.Visible)
        {
            RenderCard();
            return;
        }
        if (!island.Window.InVoiceMode && state is PointState.Idle && !card.Visible) return;
        if (!ownCapture && state.IsGesture) return; // dictation's pill is the AnyApp controller's
        island.SetVoice(View(Phase()));
    }

    private VoiceView View(string phase)
    {
        long heldMs = sampler.Samples.Count > 0 ? sampler.Samples[^1].TMs : (long)(state.IsGesture && Now > gestureStartMs ? Now - gestureStartMs : 0);
        bool cmd = card.Source == CardSource.Command && card.Visible;
        bool failed = card.Visible && card.Phase == CardPhase.Failed;
        string status = phase switch
        {
            "listening" => cmd ? $"{IslandModel.Clock(heldMs)} · {(locked ? "locked — tap to send" : ModeKeyText + " held")}" : PointFlow.ListeningStatus(heldMs, ModeKeyText, locked),
            "sending" => cmd ? "heard → working" : "finishing…",
            "working" => cmd ? CommandCard.StatusCopy(command, ModeKeyText) : card.Copy(ModeKeyText, Now),
            "done" => cmd ? CommandCard.StatusCopy(command, ModeKeyText) : failed ? card.Copy(ModeKeyText, Now) : card.Speaking ? "click to stop" : card.Latest.Stopped ? "stopped" : "theuth's desk",
            _ => hint ?? "",
        };
        string? title = cmd ? CommandCard.Header(command) : null;
        string? cardText = null;
        if (phase is "working" or "done")
        {
            cardText = cmd ? CommandReply() : failed ? AnswerCardState.FailedCopy(card.Latest.Error) : card.CollapsedText; // §12.2: the receipt's two lines
            if (cmd && cardText.Length == 0 && command.Phase is not (CommandPhase.Done or CommandPhase.Failed)) cardText = null;
        }
        var steps = cmd ? StepRows() : null;
        var box = cmd ? Box() : null;
        string footer = failed ? "Esc dismiss" : working ? "click to expand · Esc stop" : "click to expand · Esc hides";
        return new VoiceView(phase, phase == "hint" ? "" : live.Length > 0 ? live : dictation.LiveText, status, cardText, hint, locked, working,
            deskSession is { } ds ? $"theuth://c/{ds}" : null, title, steps, box, failed, footer);
    }

    private string CommandReply()
    {
        var cur = command.Current;
        if (cur is null) return "";
        if (cur.Phase == CommandPhase.Failed) return CommandCard.FailedLine(cur.Error);
        var reply = cur.Reply.Trim();
        if (cur.Phase == CommandPhase.Done) return reply.Length > 0 ? $"{reply}\n{CommandCard.Summary(cur.Steps)}" : CommandCard.Summary(cur.Steps);
        return reply;
    }

    private IReadOnlyList<CardStepRow> StepRows() =>
        (command.Current?.Steps ?? Array.Empty<CommandStep>()).Select(s => new CardStepRow(s.Verb, s.Text, s.Status, s.Note)).ToList();

    private CardApprovalBox? Box() => command.Approval is { } a
        ? new CardApprovalBox(a.Question, a.RiskLine, a.Context, a.Line, a.Answered is not null)
        : null;

    /// <summary>The window's content for the card's state (§12.2 anatomy; the command card's rows and box, §10.3/§10.6).</summary>
    private AnswerCardContent Content()
    {
        bool cmd = card.Source == CardSource.Command;
        var x = card.Current;
        bool failed = card.Phase == CardPhase.Failed;
        bool thinking = card.Phase == CardPhase.Thinking || (cmd && command.Phase is CommandPhase.Heard or CommandPhase.Working or CommandPhase.NeedsOk);
        string status = cmd ? CommandCard.StatusCopy(command, ModeKeyText) : hint ?? card.Copy(ModeKeyText, Now);
        var tone = failed || (cmd && command.Phase == CommandPhase.Failed) ? CardTone.Failed
            : command.Approval is { Answered: null } && cmd ? CardTone.Caution
            : thinking ? CardTone.Working
            : cmd && command.Current?.Verdict.Kind == CommandVerdict.Ok ? CardTone.Success
            : CardTone.Normal;
        string body = cmd ? CommandReply() : failed ? AnswerCardState.FailedCopy(x.Error) + (x.Answer.Length > 0 ? "\n\n" + x.Answer : "") : x.Answer;
        string collapsed = cmd ? CommandReply() : failed ? AnswerCardState.FailedCopy(x.Error) : card.CollapsedText;
        var ctx = new CardKeyContext(command.Approval is { Answered: null } && cmd, card.Expanded, thinking, card.Speaking, false);
        return new AnswerCardContent
        {
            Title = cmd ? CommandCard.Header(command) : "theuth desk",
            Subtitle = cmd ? null : card.Thread.Count > 1 ? $"{card.Index + 1} of {card.Thread.Count}" : card.Phase == CardPhase.Answer && card.Finished && !x.Stopped ? "replied just now" : null,
            ThreadIndex = card.Index,
            ThreadCount = card.Thread.Count,
            Request = x.Request.Length > 0 ? x.Request : null,
            Body = body,
            Collapsed = collapsed,
            Status = status,
            Tone = tone,
            Busy = thinking,
            Chips = cmd ? Array.Empty<ToolChip>() : x.Tools,
            Steps = cmd ? StepRows() : Array.Empty<CardStepRow>(),
            StepSummary = null,
            Approval = cmd ? Box() : null,
            Footer = CardKeys.Footer(ctx, !cmd && !card.Expanded && card.Phase == CardPhase.Answer) + (cmd && command.Phase == CommandPhase.Done ? $" · hold {ModeKeyText} follow up" : ""),
            FollowUp = deskSession is not null && !failed,
            FollowUpHint = cmd ? $"add to the command… (or hold {ModeKeyText})" : $"follow up… (or hold {ModeKeyText})",
            Speaking = card.Speaking,
            OpenUrl = deskSession is { } ds ? $"theuth://c/{ds}" : null,
        };
    }

    private AnswerCardWindow Window()
    {
        if (window is { } w) return w;
        w = window = AnswerCardWindow.Shared();
        w.KeyIntent += OnCardKey;
        w.ActionInvoked += (gen, id) =>
        {
            if (gen != windowGen) return;
            if (id is "approve" or "deny") AnswerApproval(id == "approve" ? Decision.Allow : Decision.Deny, "click");
        };
        w.FollowUpSubmitted += (gen, text) => { if (gen == windowGen) _ = SendTypedFollowUp(text); };
        w.ExpandedChanged += (gen, expanded) =>
        {
            if (gen != windowGen) return;
            card.SetExpanded(expanded);
            ArmDone();
            RenderCard();
        };
        w.Dismissed += gen => { if (gen == windowGen) Dismiss("the close cap"); };
        w.SpeakerClicked += gen => { if (gen == windowGen) speech.Stop(SpeechStop.Glyph); };
        return w;
    }

    /// <summary>§12.3: the collapsed card detaches into the answer card window, expanded, at the island area; the island's voice view goes.</summary>
    public void Expand(string via)
    {
        if (!card.Visible || state.IsGesture && card.Phase == CardPhase.Listening && via != "approval") return;
        var w = Window();
        if (!detached)
        {
            detached = true;
            windowGen = w.Begin(card.Source == CardSource.Command ? "command" : "voice");
            island.SetVoice(null);
        }
        bool expand = via != "approval" || card.Expanded;
        card.SetExpanded(expand);
        w.Present(windowGen, Content(), expanded: expand);
        if (via is "click" or "approval") w.TakeKey();
        Log.Write($"point: card {(expand ? "expanded" : "detached")} into the answer card window ({via}) — {card.Phase.ToString().ToLowerInvariant()}, thread {card.Index + 1}/{card.Thread.Count}");
        ArmDone();
    }

    private void RenderCard()
    {
        if (!detached || window is not { } w) return;
        w.Present(windowGen, Content(), expanded: card.Expanded);
    }

    /// <summary>The card goes away (generation-checked on the window) and returns to the island next time.</summary>
    private void HideCard(string why)
    {
        if (card.Visible) card.Hide(card.Generation);
        if (detached && window is { } w) w.Hide(windowGen, why);
        detached = false;
    }

    private void Dismiss(string via)
    {
        Log.Write($"point: card dismissed ({via})");
        if (state is PointState.Replying) { Step(new PointEvent.Esc()); return; }
        if (state is PointState.Sending && working && deskSession is not null) { FreezeStop(); return; }
        HideCard(via);
        if (!state.IsGesture) island.SetVoice(null);
    }

    private void OnCardKey(int gen, CardKeyIntent intent)
    {
        if (gen != windowGen) return;
        Log.Write($"point: card key {intent}");
        switch (intent)
        {
            case CardKeyIntent.Approve: AnswerApproval(Decision.Allow, "enter"); break;
            case CardKeyIntent.Deny: AnswerApproval(Decision.Deny, "esc"); break;
            case CardKeyIntent.Stop: FreezeStop(); break;
            case CardKeyIntent.StopSpeaking: speech.Stop(SpeechStop.Esc); break;
            case CardKeyIntent.Dismiss: Dismiss("Esc"); break;
            case CardKeyIntent.Open:
            case CardKeyIntent.List: // writer's call: the history list view is the desk session in theuth (the Ask bar's receipts own activity_list)
                if (deskSession is { } ds) _ = Links.Open(new Uri($"theuth://c/{ds}"));
                break;
            case CardKeyIntent.Next:
            case CardKeyIntent.Previous:
                if (card.Move(intent == CardKeyIntent.Next ? 1 : -1)) RenderCard();
                break;
        }
    }

    /// <summary>§12.2 / §10.5: typing into the expanded card's field sends with the card's marker family.</summary>
    private async Task SendTypedFollowUp(string text)
    {
        if (deskSession is not { } target) return;
        bool cmd = card.Source == CardSource.Command;
        var body = cmd ? CommandCard.CommandText(CommandCard.ContinueText(command, text), 0) : PointFlow.NoteText(text, 0);
        cardGen = card.Begin(card.Source, Now, followUp: true);
        if (cmd) Observe(new CommandInput.Listening(Now));
        card.FinalText(text, Now);
        if (cmd) Observe(new CommandInput.Heard(text, Now));
        RenderCard();
        if (probing) { Log.Write($"point: typed follow-up (probe, not sent): {body}"); return; }
        try
        {
            await store.Client.CallAsync<string>("send_input", target, body, Array.Empty<string>(), null, Guid.NewGuid().ToString());
            working = true;
            card.TurnStarted(Now);
            if (state is PointState.Replying or PointState.Idle) state = new PointState.Sending(Now);
            After(secondTimer, 1000);
            Log.Write($"point: typed follow-up sent to {target}: {Clip(body, 120)}");
        }
        catch (Exception e)
        {
            card.Failed($"send_input: {Why(e)}", Now);
            Log.Write($"point: typed follow-up failed: {Why(e)}");
        }
        RenderCard();
    }

    /// <summary>§12.4 "stop freezes": Esc while the desk works → <c>cancel(desk, "root")</c>; the card keeps the partial answer with "stopped".</summary>
    private void FreezeStop()
    {
        if (deskSession is not { } ds) return;
        Log.Write($"point: Esc while thinking — cancel({ds}, root); the card freezes with “stopped”");
        if (card.Source == CardSource.Command) Observe(new CommandInput.Stop(Now)); // its CancelTurn effect sends the cancel
        else if (!probing) _ = store.Client.CancelAsync(ds, CancelScope.Root).ContinueWith(t => { if (t.IsFaulted) Log.Write($"point: cancel failed: {t.Exception?.GetBaseException().Message}"); }, TaskScheduler.Default);
        card.Stop(Now);
        working = false;
        secondTimer.Stop();
        if (state is PointState.Sending) Step(new PointEvent.PanelDone()); // Replying: a second Esc conceals
        RenderVoice();
        ArmDone();
    }

    /// <summary>The 6 s timer runs only while the card is collapsed ∧ silent ∧ finished (§12.4); otherwise it waits for that.</summary>
    private void ArmDone()
    {
        if (card.DoneArmed && state is PointState.Replying or PointState.Idle) After(doneTimer, (int)AnswerCardState.DoneMs);
        else doneTimer.Stop();
    }

    private void OnDoneTimer()
    {
        if (!card.DoneArmed) return; // expanded or speaking since: never vanish (re-armed when that ends)
        Log.Write($"point: done timer — card hidden after {AnswerCardState.DoneMs / 1000} s (collapsed, silent, finished)");
        if (state is PointState.Replying) Step(new PointEvent.PanelDone()); // Replying → Idle + Conceal
        else Conceal();
    }

    /// <summary>The thinking copy's elapsed seconds tick once a second (§12.4: never looks frozen).</summary>
    private void OnSecond()
    {
        if (!working || !card.Visible) return;
        RenderVoice();
        After(secondTimer, 1000);
    }

    private void OnSpeechChanged()
    {
        bool ours = speech.Speaking is { Source: SpeechSource.VoiceKey } cur && cur.Session == deskSession;
        if (ours && !card.Speaking) card.SpeechStarted(Now);
        else if (!ours && card.Speaking) card.SpeechEnded(Now);
        else return;
        RenderVoice();
        ArmDone();
    }

    // MARK: Esc

    /// <summary>The registered Esc (plain, or with the held chord's modifiers): the approval box denies first (§15), then speech, then stop, then the machine.</summary>
    private void OnEsc()
    {
        if (command.Approval is { Answered: null } && card.Source == CardSource.Command) { AnswerApproval(Decision.Deny, "esc"); return; }
        if (card.Speaking) { speech.Stop(SpeechStop.Esc); return; } // speaking leaves on Esc; the card stays (§12.4)
        if (state is PointState.Sending && working && deskSession is not null) { FreezeStop(); return; }
        if (detached && card.Visible && card.Expanded && !state.IsGesture && window is { } w)
        {
            // H3-C1 order (deny → list → collapse → dismiss): an expanded card folds back first.
            w.SetExpanded(windowGen, false);
            return;
        }
        if (!state.IsGesture && card.Visible && card.Phase == CardPhase.Failed && state is not PointState.Replying) { Dismiss("Esc"); return; }
        Step(new PointEvent.Esc());
    }

    private void RegisterEsc()
    {
        if (!escRegistered)
        {
            escRegistered = PInvoke.RegisterHotKey(island.Window.Hwnd, EscHotkeyId, HOT_KEY_MODIFIERS.MOD_NOREPEAT, VkEscape);
            if (!escRegistered) Log.Write("point: RegisterHotKey(Esc) failed — Esc cancels only with the island focused");
        }
        if (!escHeldRegistered)
        {
            // R3: while the chord is held, Esc arrives as <modifiers>+Esc; register that too (never Ctrl+Shift+Esc — Task Manager).
            // 153: the live gesture's key (the AltGr fallback included) — Ctrl+Alt+Esc for all three defaults.
            var voice = hotkeys.EffectiveChord(KeyTable.ActionOf(mode)) ?? store.Keys.ChordFor(KeyTable.ActionOf(mode));
            if (EscDuringHold.Modifiers(voice) is { } mods)
            {
                escHeldRegistered = PInvoke.RegisterHotKey(island.Window.Hwnd, EscHeldHotkeyId, (HOT_KEY_MODIFIERS)((uint)mods) | HOT_KEY_MODIFIERS.MOD_NOREPEAT, VkEscape);
                if (!escHeldRegistered) Log.Write($"point: RegisterHotKey({new Chord("\u001b", VirtualKeys.Escape, mods).Symbol}) failed {Interop.Win32.LastError()} — Esc cancels once the modifiers are released");
            }
            else if (EscDuringHold.SkipReason(voice) is { } why && !escHeldSkipLogged)
            {
                escHeldSkipLogged = true;
                Log.Write($"point: Esc during the hold: {why}");
            }
        }
    }

    private bool escHeldSkipLogged;

    private void UnregisterEsc()
    {
        if (escRegistered)
        {
            PInvoke.UnregisterHotKey(island.Window.Hwnd, EscHotkeyId);
            escRegistered = false;
        }
        if (escHeldRegistered)
        {
            PInvoke.UnregisterHotKey(island.Window.Hwnd, EscHeldHotkeyId);
            escHeldRegistered = false;
        }
    }

    // MARK: 153 probes (never acting: no send, no resolve, no cancel, no listen)

    /// <summary>
    /// <c>--probe-command</c> / <c>--probe-states</c> / <c>--probe-card</c>: the real reducer, card state, island view and window driven from
    /// recorded sequences; each step logs <c>probe153: …</c> and writes a PNG of both surfaces under <c>%TEMP%\153-*.png</c>.
    /// </summary>
    internal void BeginProbe(PointMode asMode, string session)
    {
        probing = true;
        replaying = true;
        gesture++;
        mode = asMode;
        HideCard("probe");
        command = CommandCardState.Empty;
        cardGen = card.Begin(SourceOf(asMode), Now);
        deskSession = session;
        card.Session = session;
        if (asMode == PointMode.Command) Observe(new CommandInput.Listening(Now));
        state = new PointState.Holding(Now);
        locked = false;
        ownCapture = true;
        live = "";
        answer = "";
        working = false;
        RenderVoice();
    }

    /// <summary>One probe step: feed card inputs, then render.</summary>
    internal void ProbeStep(Action<AnswerCardState> cardInput, CommandInput? cmd = null, PointState? newState = null, string? liveText = null, bool? isWorking = null)
    {
        cardInput(card);
        if (cmd is not null) Observe(cmd);
        if (newState is not null) state = newState;
        if (liveText is not null) live = liveText;
        if (isWorking is { } wk) working = wk;
        RenderVoice();
    }

    /// <summary>Probe: feed an engine event through the real stream path (Store.HandleEvent → OnEvent).</summary>
    internal void ProbeEvent(string json) => store.HandleEvent(System.Text.Encoding.UTF8.GetBytes(json));

    internal void ProbeExpand(bool expanded)
    {
        if (expanded) Expand("probe");
        else if (detached) { card.SetExpanded(false); RenderCard(); }
    }

    internal void ProbeDetachCollapsed()
    {
        var w = Window();
        if (!detached) { detached = true; windowGen = w.Begin("probe"); island.SetVoice(null); }
        card.SetExpanded(false);
        w.Present(windowGen, Content(), expanded: false);
    }

    internal void ProbeAttach()
    {
        if (detached && window is { } w) w.Hide(windowGen, "probe: back to the island");
        detached = false;
        card.SetExpanded(false);
        RenderVoice();
    }

    internal async Task ProbeCapture(string name)
    {
        var dir = Path.GetTempPath();
        string islandPng = Path.Combine(dir, $"153-{name}-island.png"), windowPng = Path.Combine(dir, $"153-{name}-card.png");
        bool a = await island.Window.CapturePng(islandPng);
        bool b = window is { IsShown: true } w && await w.CapturePng(windowPng);
        Log.Write($"probe153: {name} — card {card.Phase.ToString().ToLowerInvariant()} expanded={card.Expanded} visible={card.Visible} command={command.Phase.ToString().ToLowerInvariant()} copy=“{(card.Source == CardSource.Command ? CommandCard.StatusCopy(command, ModeKeyText) : card.Copy(ModeKeyText, Now))}” island={(a ? islandPng : "hidden")} window={(b ? windowPng : "hidden")}");
    }

    internal void EndProbe()
    {
        HideCard("probe end");
        island.SetVoice(null);
        state = new PointState.Idle();
        deskSession = null;
        working = false;
        probing = false;
        replaying = false;
        Log.Write("probe153: done");
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
        hotkeys.Chorded -= OnChorded;
        island.ModeKeysChanged -= ApplyModeKeys;
        speech.Changed -= OnSpeechChanged;
        store.DictationWire -= OnDictationWire;
        store.PropertyChanged -= OnStoreChanged;
        store.EventReceived -= OnEvent;
        CloseListen("dispose");
        if (lease.Close() is { } id) _ = store.Dictation.StopAsync(id).ContinueWith(_ => { }, TaskScheduler.Default);
        dictationId = null;
        UnregisterEsc();
        sampler.Dispose();
        recorder.Cancel();
        overlays.Dispose();
    }
}
