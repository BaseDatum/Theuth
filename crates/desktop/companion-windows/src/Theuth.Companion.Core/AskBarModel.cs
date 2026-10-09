using System.ComponentModel;
using System.Runtime.CompilerServices;
using Theuth.Companion.Rpc;

namespace Theuth.Companion.Core;

/// <summary>The engine's streamed dictation as the Ask bar calls it (<see cref="AskRpc"/> in the app, a fake in tests).</summary>
public interface IDictationCalls
{
    /// <summary><c>dictation_prewarm</c>: load the model; never opens a microphone.</summary>
    Task PrewarmAsync();

    /// <summary><c>dictation_start(16000)</c> → the dictation id (never null: null opens the engine's own microphone).</summary>
    Task<string> StartAsync();

    /// <summary><c>dictation_audio(id, base64 LE i16)</c>, ~100 ms per call, in order.</summary>
    Task AudioAsync(string id, short[] pcm16);

    /// <summary><c>dictation_stop(id)</c>: flush the open utterance; <c>DictationFinished</c> follows as an event.</summary>
    Task StopAsync(string id);

    /// <summary>153 §14.1: <c>dictation_start(16000, opts)</c> → the id (surface, destination, press offset). The default drops the
    /// options — an implementation from before 153 keeps working.</summary>
    Task<string> StartAsync(DictationOpts opts) => StartAsync();

    /// <summary>153 §6.1: <c>dictation_correction(id, sent_text)</c> — the text actually sent after dictation <paramref name="id"/>.
    /// The default sends nothing.</summary>
    Task CorrectionAsync(string id, string sentText) => Task.CompletedTask;
}

/// <summary>The Ask bar's window and microphone as <see cref="AskBarModel"/> drives them (the app: AskBarWindow + VoiceRecorder).</summary>
public interface IAskBarSurface
{
    /// <summary>Place and show; <paramref name="activate"/>: take the keyboard (summoned) and remember the previous foreground.</summary>
    void Show(bool activate);

    /// <summary>Hide; <paramref name="returnFocus"/>: hand the foreground back to the window it was taken from.</summary>
    void Hide(bool returnFocus);

    /// <summary>Open the default capture endpoint: null when it streams (chunks arrive through <see cref="AskBarModel.Feed"/>), else why not.</summary>
    Task<string?> StartMicrophoneAsync();

    /// <summary>Stop capturing; the tail under one chunk still arrives through <see cref="AskBarModel.Feed"/>.</summary>
    void StopMicrophone();

    /// <summary>Stop and drop whatever is buffered.</summary>
    void CancelMicrophone();

    /// <summary>A <c>theuth://</c> link for the main window (Ctrl+O on a reply).</summary>
    void OpenUrl(Uri url);
}

/// <summary>
/// The Ask bar (design 134 §4.3, D7; 130 §4.3 / W3 / W6) without a UI framework: the field state the window renders
/// (Swift <c>AskBarState</c>) and the controller logic of Swift's <c>AskBarController</c> — open/close, the Ask-bar key
/// scope, hold-to-talk on the hotkey (<see cref="AskVoiceMachine"/>), the live dictation over the engine's stream
/// (<see cref="AskDictation"/> on the Store; strictly ordered calls, chunks before the id held, a release before the id
/// remembered, the 5 s cap on "finishing…"), the synthetic probes. Sends go through the Store's hold
/// (<see cref="StoreModel.SubmitAsk"/>: 3 s, Esc undoes, then the desk). One owner thread; portable (Mac-tested) — the
/// app's <c>AskBar/AskBarWindow</c> renders it and <c>Voice/VoiceRecorder</c> is its microphone (design 134 W4).
/// </summary>
public sealed class AskBarModel : INotifyPropertyChanged
{
    /// <summary>The voice machine's tick while a press is in progress (Swift: a 0.05 s timer).</summary>
    public static readonly TimeSpan VoiceTick = TimeSpan.FromMilliseconds(50);

    /// <summary>"finishing…" never lasts longer: the text so far lands (Swift's ponytail cap).</summary>
    public static readonly TimeSpan FinishCap = TimeSpan.FromSeconds(5);

    /// <summary>153 §7.3: the status while "send it" is armed — the engine stops after the 1.5 s settle and sends without the hold.</summary>
    public const string SendingSoon = "sending in 1.5 s…";

    private readonly StoreModel store;
    private readonly IDictationCalls calls;
    private readonly IAskBarSurface surface;
    private readonly IStoreScheduler scheduler;
    private readonly Func<DateTimeOffset> clock;
    private readonly Action<string> log;
    private readonly Func<KeyTable> keys;
    private readonly AskVoiceMachine voice = new();
    private IDisposable? voiceTimer;
    private IDisposable? probeTimer;
    private IDisposable? finishCap;
    private DateTimeOffset openedAt = DateTimeOffset.MinValue;

    /// <summary>A bar opened this recently by the same press (W2 opens it through <c>Store.AskBarRequested</c> before the press reaches here) counts as opened by the press.</summary>
    public static readonly TimeSpan OpenedByPressWindow = TimeSpan.FromMilliseconds(100);
    private Task sendTask = Task.CompletedTask;
    private bool prewarmed;
    private readonly List<short[]> pendingChunks = new();
    private bool stopWanted;

    // 153 W153B
    private readonly Func<bool> micOnKeyDown;
    /// <summary>When the current voice press went down (§8.7 <c>press_ago_ms</c>); null for probes.</summary>
    private DateTimeOffset? pressAt;
    /// <summary>§8.5: the microphone opened at the press and buffers into <see cref="pendingChunks"/> until <c>dictation_start</c>.</summary>
    private bool micStartedEarly;
    /// <summary>§7.3: the engine ended the dictation while the key was held — the coming key-up is not a tap.</summary>
    private bool swallowKeyUp;
    /// <summary>§6.1: the dictation whose text is in the field; reported with the sent text at commit, then forgotten.</summary>
    private string? dictatedId;
    /// <summary>§8.5: bumped when a press ends early (tap, failure) — a microphone that finishes opening afterwards is closed again.</summary>
    private int micGeneration;

    private string text = "";
    private string partial = "";
    private bool live;
    private bool recording;
    private float level;
    private string? status;
    private bool isOpen;

    public AskBarModel(StoreModel store, IDictationCalls calls, IAskBarSurface surface, IStoreScheduler scheduler,
        Func<DateTimeOffset>? clock = null, Action<string>? log = null, Func<KeyTable>? keys = null, bool automated = false,
        Func<bool>? micOnKeyDown = null)
    {
        this.micOnKeyDown = micOnKeyDown ?? (() => false);
        this.store = store;
        this.calls = calls;
        this.surface = surface;
        this.scheduler = scheduler;
        this.clock = clock ?? (() => DateTimeOffset.Now);
        this.log = log ?? (_ => { });
        this.keys = keys ?? (() => new KeyTable());
        Automated = automated;
        store.DictationChanged += DictationChanged;
        store.AskBarRequested += Open; // Ctrl+L in the panel (130 W3)
        store.Committing += OnCommitting; // 153 §6.1
        store.EventReceived += OnStreamItem; // 153 §8.2: DictationDevice hints
    }

    /// <summary>
    /// A probe run (Swift <c>AskFlags.automated</c>): the hotkey and the microphone are inert. Unlike the Swift probe a
    /// typed Enter still sends — the W4 gate types a request into the bar and expects <c>send_input</c> to go out
    /// (134 §7 W4); the synthetic dictation itself never sends (no voice press, so no <c>SetText</c>).
    /// </summary>
    public bool Automated { get; }

    // MARK: field state (Swift AskBarState) — PropertyChanged on the owner thread

    public event PropertyChangedEventHandler? PropertyChanged;

    /// <summary>The field. While <see cref="Live"/> it holds the closed utterances; the open one is <see cref="Partial"/>.</summary>
    public string Text { get => text; private set => Set(ref text, value); }

    /// <summary>The open utterance of the live dictation, shown muted after <see cref="Text"/>.</summary>
    public string Partial { get => partial; private set => Set(ref partial, value); }

    /// <summary>The live dictation owns the field (finals + muted partial) until it lands.</summary>
    public bool Live { get => live; private set => Set(ref live, value); }

    public bool Recording { get => recording; private set => Set(ref recording, value); }

    /// <summary>The last <c>DictationLevel</c> peak, 0–1: the mic glyph's pulse.</summary>
    public float Level { get => level; private set => Set(ref level, value); }

    /// <summary>"listening…" · "finishing…" · a failure; null when quiet.</summary>
    public string? Status { get => status; private set => Set(ref status, value); }

    public bool IsOpen { get => isOpen; private set => Set(ref isOpen, value); }

    /// <summary>The dictated text whose hold is running: any other text means the user typed, which cancels it (§4.3).</summary>
    public string? VoiceText { get; private set; }

    /// <summary>The press that opened the bar must not close it on release.</summary>
    public bool OpenedByPress { get; private set; }

    public AskVoiceMachine.State VoiceState => voice.Current;

    /// <summary>The hint under the field: the hold chord from the key table ("hold Ctrl+Shift+Space").</summary>
    public string HoldHint => keys().ChordFor(KeyAction.OpenPanel) is { } c ? $"hold {c.Symbol}" : "hold the hotkey";

    /// <summary>
    /// What the field shows: while <see cref="Live"/> the closed utterances then the open one (the window mutes the
    /// partial), else <see cref="Text"/>. UIA reads this as the transcript (the W4 gate's <c>ui_read("ask-field")</c>).
    /// </summary>
    public string FieldText => Live && Partial.Length > 0 ? (Text.Length > 0 ? Text + " " + Partial : Partial) : Text;

    /// <summary>The field's placeholder: "Listening…" while the dictation has produced nothing yet, else the prompt.</summary>
    public string Placeholder => Live && Text.Length == 0 && Partial.Length == 0 ? "Listening…" : "Ask theuth…";

    /// <summary>
    /// The receipt row's two columns (Swift <c>ReceiptView</c>): the words, and the trailing hint (the undo countdown,
    /// "Open Ctrl+O"); null when there is no receipt. A desk reply shows <c>who · first two lines</c>.
    /// </summary>
    public static (string Text, string Trailing)? ReceiptLines(StoreModel.ReceiptRow? receipt, double remaining)
    {
        switch (receipt)
        {
            case null: return null;
            case StoreModel.ReceiptRow.Holding h: return ($"{h.Held.Who} · {h.Held.Label} — sending", $"Esc undo · {(int)Math.Ceiling(remaining)}");
            case StoreModel.ReceiptRow.Sent s: return ($"Sent · {s.Text}", "");
            case StoreModel.ReceiptRow.Undone: return ("Taken back", "");
            case StoreModel.ReceiptRow.Stale: return ("Already answered elsewhere", "");
            case StoreModel.ReceiptRow.Failed f: return ($"Failed · {f.Error}", "");
            case StoreModel.ReceiptRow.Reply r: return ($"{r.Who} · {r.Text}", r.Session is null ? "" : "Open Ctrl+O");
            default: return null;
        }
    }

    private void Set<T>(ref T field, T value, [CallerMemberName] string? name = null)
    {
        if (EqualityComparer<T>.Default.Equals(field, value)) return;
        field = value;
        PropertyChanged?.Invoke(this, new PropertyChangedEventArgs(name));
    }

    // MARK: open / close

    /// <summary>Summon: show, take the keyboard, prewarm the dictation model once.</summary>
    public void Open()
    {
        if (!IsOpen) openedAt = clock();
        IsOpen = true;
        surface.Show(activate: true);
        Prewarm();
    }

    /// <summary>Probe mode: on screen without taking the keyboard, the field pre-filled (Swift <c>showWithoutKey</c>).</summary>
    public void ShowWithoutKey(string fieldText)
    {
        Text = fieldText;
        IsOpen = true;
        surface.Show(activate: false);
    }

    public void Close(bool returnFocus)
    {
        if (!IsOpen) return;
        IsOpen = false;
        if (voice.IsBusy) Apply(voice.Handle(new AskVoiceMachine.Event.Failed("cancelled")));
        surface.Hide(returnFocus);
    }

    public void Toggle()
    {
        if (IsOpen) Close(returnFocus: true);
        else Open();
    }

    // MARK: keys (the table's Ask-bar scope: Enter sends, Esc, Ctrl+O, Ctrl+/; the field takes the rest)

    /// <summary>
    /// A key in the bar; true when handled. Bare printable keys always type (a <c>?</c> in a question is text, not the
    /// legend alias — the Swift bar swallowed it); the rest is looked up in the Ask-bar scope.
    /// </summary>
    public bool HandleKey(KeyPress press)
    {
        var chord = press.Chord;
        if (chord.IsCharKey && (press.Modifiers & (ChordModifiers.Control | ChordModifiers.Alt | ChordModifiers.Win)) == 0) return false;
        var intent = KeyGrammar.IntentFor(press, KeyScope.AskBar, keys());
        switch (intent.Kind)
        {
            case IntentKind.Send:
                Submit();
                return true;
            case IntentKind.Legend:
                store.Perform(Intent.Legend);
                return true;
            case IntentKind.Escape:
                if (store.HeldNow is not null)
                {
                    store.Perform(Intent.Escape); // undo: the text is still in the field
                    return true;
                }
                Close(returnFocus: true);
                return true;
            case IntentKind.OpenSession:
                if (store.Receipt is StoreModel.ReceiptRow.Reply { Session: { } s }) surface.OpenUrl(DeepLink.Session(s));
                else if (store.Ask.Router.DeskSession is { } d) surface.OpenUrl(DeepLink.Session(d));
                return true;
            default:
                return false;
        }
    }

    /// <summary>Enter: the field goes to theuth's desk through the Store's hold (3 s, Esc undoes; then the desk routing).</summary>
    public void Submit()
    {
        if (Automated) log($"probe: typed send ({Text.Trim().Length} chars)");
        VoiceText = null;
        store.SubmitAsk(Text);
    }

    /// <summary>The user edited the field (not the dictation): typing during a voice hold keeps editing instead of sending.</summary>
    public void FieldEdited(string edited)
    {
        Text = edited;
        if (edited.Trim().Length == 0) dictatedId = null; // 153 §6.1: a cleared field reports nothing
        if (VoiceText is not { } v || edited == v) return;
        VoiceText = null;
        store.Ask.Dictation.Handle(AskDictation.Event.Typed.Instance);
        if (store.HeldNow is not null) store.Perform(Intent.Escape);
    }

    // MARK: the hotkey (W2's RegisterHotKey delivers the key-down; the release is observed by polling the key)

    /// <summary>
    /// The hotkey went down and was routed to the Ask bar (W2's handler opens the Attention panel instead while something
    /// needs you, as Swift's <c>hotkeyDown</c>): a press on a closed bar opens it and starts a press — held past 0.25 s it
    /// dictates straight into the fresh bar; on an open bar the press is a voice press.
    /// </summary>
    public void HotkeyDown()
    {
        if (Automated) return;
        if (!voice.IsBusy)
        {
            pressAt = clock();
            voice.OpenMicOnKeyDown = micOnKeyDown(); // 153 §8.5, Settings › Dictation "Open the mic on key-down"
        }
        if (IsOpen && clock() - openedAt > OpenedByPressWindow)
        {
            OpenedByPress = false;
            VoiceEvent(new AskVoiceMachine.Event.KeyDown(clock()));
            return;
        }
        OpenedByPress = true;
        Open();
        VoiceEvent(new AskVoiceMachine.Event.KeyDown(clock()));
    }

    /// <summary>The hotkey came up: ends a voice press; otherwise a press that did not open the bar closes it.</summary>
    public void HotkeyUp()
    {
        if (Automated) return;
        if (voice.IsBusy)
        {
            VoiceEvent(new AskVoiceMachine.Event.KeyUp(clock()));
            return;
        }
        if (swallowKeyUp)
        {
            // 153 §7.3: the dictation already landed (the engine stopped it on "send it") while the key was held.
            swallowKeyUp = false;
            OpenedByPress = false;
            return;
        }
        if (OpenedByPress)
        {
            OpenedByPress = false;
            return;
        }
        if (IsOpen) Close(returnFocus: true);
    }

    // MARK: voice (D7, W6 live): hold → microphone → dictation_audio every 100 ms → release → dictation_stop
    // → DictationFinished → the text stays in the field → the hold

    private void VoiceEvent(AskVoiceMachine.Event e)
    {
        Apply(voice.Handle(e));
        if (voice.IsBusy && voiceTimer is null) voiceTimer = scheduler.Every(VoiceTick, VoiceTickFired);
        else if (!voice.IsBusy) StopVoiceTimer();
    }

    private void VoiceTickFired()
    {
        Apply(voice.Handle(new AskVoiceMachine.Event.Tick(clock())));
        Recording = voice.IsRecording;
        if (!voice.IsBusy) StopVoiceTimer();
    }

    private void StopVoiceTimer()
    {
        voiceTimer?.Dispose();
        voiceTimer = null;
    }

    private void Apply(IReadOnlyList<AskVoiceMachine.Effect> effects)
    {
        foreach (var fx in effects)
        {
            switch (fx.Kind)
            {
                case AskVoiceMachine.EffectKind.OpenMic:
                    // 153 §8.5: the recorder starts at the press; its chunks wait in pendingChunks for dictation_start at 250 ms.
                    micStartedEarly = true;
                    pendingChunks.Clear();
                    _ = StartMicrophone();
                    break;
                case AskVoiceMachine.EffectKind.Tap:
                    if (micStartedEarly)
                    {
                        // A tap never starts a dictation: the early microphone stops and its buffer is dropped (§8.5).
                        micStartedEarly = false;
                        micGeneration++;
                        surface.CancelMicrophone();
                        pendingChunks.Clear();
                    }
                    pressAt = null;
                    if (OpenedByPress) OpenedByPress = false;
                    else Close(returnFocus: true);
                    break;
                case AskVoiceMachine.EffectKind.StartRecording:
                {
                    Recording = true;
                    Status = "listening…";
                    var micOpen = micStartedEarly;
                    BeginDictation(keepBuffered: micOpen);
                    micStartedEarly = false;
                    if (!micOpen) _ = StartMicrophone();
                    break;
                }
                case AskVoiceMachine.EffectKind.StopDictation:
                    Recording = false;
                    Status = "finishing…";
                    surface.StopMicrophone(); // flushes the tail chunk through Feed
                    EndDictation();
                    break;
                case AskVoiceMachine.EffectKind.SetText:
                    LandedWhileHeld();
                    Status = null;
                    Live = false;
                    VoiceText = fx.Text;
                    Text = fx.Text ?? "";
                    dictatedId = store.Ask.Dictation.LastId;
                    store.SubmitAsk(Text); // the same 3 s hold; typing or Esc keeps editing
                    break;
                case AskVoiceMachine.EffectKind.SendNow:
                    // 153 §7.4: the spoken "send it" was the confirmation — the text goes to the desk with no hold.
                    LandedWhileHeld();
                    Status = null;
                    Live = false;
                    VoiceText = null;
                    Text = fx.Text ?? "";
                    dictatedId = store.Ask.Dictation.LastId;
                    log($"voice: \"send it\" — sent without the hold ({Text.Length} chars)");
                    store.SubmitAskNow(Text);
                    break;
                case AskVoiceMachine.EffectKind.Fail:
                    micGeneration++;
                    micStartedEarly = false;
                    pendingChunks.Clear();
                    pressAt = null;
                    dictatedId = null;
                    Recording = false;
                    Live = false;
                    surface.CancelMicrophone();
                    if (store.Ask.Dictation.Id is { } id) Enqueue(() => calls.StopAsync(id));
                    Status = fx.Text;
                    log($"voice: {fx.Text}");
                    break;
            }
        }
    }

    private async Task StartMicrophone()
    {
        int gen = micGeneration;
        string? why;
        try { why = await surface.StartMicrophoneAsync(); }
        catch (Exception e) { why = $"microphone failed to start: {e.Message}"; }
        if (gen != micGeneration)
        {
            // 153 §8.5: the press ended (a tap, a failure) while the device was still opening — close it again; nothing was sent.
            if (why is null) surface.CancelMicrophone();
            return;
        }
        if (why is not null) VoiceEvent(new AskVoiceMachine.Event.Failed(why));
    }

    // MARK: the engine's dictation (design 54 §8): one id per hold; audio in order; the reducer on the Store

    /// <summary>Raised with the reason when <c>dictation_start</c> is refused (the probe logs it).</summary>
    public event Action<string>? StartRefused;

    /// <summary>Calls run strictly in order (the start, the audio chunks, then the stop).</summary>
    private void Enqueue(Func<Task> op) => sendTask = Chain(sendTask, op);

    private async Task Chain(Task previous, Func<Task> op)
    {
        await previous;
        try { await op(); }
        catch (Exception e) { log($"dictation call failed: {Describe(e)}"); }
    }

    /// <summary>The ordered dictation calls drained (tests and the probe wait on it).</summary>
    public Task Drained => sendTask;

    /// <summary><c>dictation_prewarm</c> once, when the Ask bar first opens (not at launch).</summary>
    public void Prewarm()
    {
        if (prewarmed) return;
        prewarmed = true;
        _ = PrewarmAsync();
    }

    private async Task PrewarmAsync()
    {
        try
        {
            await calls.PrewarmAsync();
            log("dictation_prewarm ok");
        }
        catch (Exception e)
        {
            log($"dictation_prewarm failed: {Describe(e)}");
        }
    }

    /// <summary>
    /// <c>dictation_start(16000, opts)</c>; chunks that arrive before the id are held and flushed in order. <paramref name="keepBuffered"/>:
    /// the microphone opened at the press (153 §8.5) and its buffered chunks go out first.
    /// </summary>
    public void BeginDictation(bool keepBuffered = false)
    {
        store.Ask.Dictation.Handle(AskDictation.Event.Start.Instance);
        Live = true;
        Partial = "";
        if (!keepBuffered) pendingChunks.Clear();
        stopWanted = false;
        Enqueue(async () =>
        {
            string id;
            try
            {
                id = await calls.StartAsync(OptsNow());
            }
            catch (Exception e)
            {
                store.Ask.Dictation.Handle(new AskDictation.Event.StartFailed(Describe(e)));
                var why = store.Ask.Dictation.Error ?? Describe(e);
                log($"dictation_start failed: {Describe(e)}");
                pendingChunks.Clear();
                VoiceEvent(new AskVoiceMachine.Event.Failed(why));
                StartRefused?.Invoke(why);
                return;
            }
            log($"dictation_start → {id}");
            store.Ask.Dictation.Handle(new AskDictation.Event.Started(id));
            foreach (var c in pendingChunks) Send(c, id);
            pendingChunks.Clear();
            if (stopWanted)
            {
                stopWanted = false;
                EndDictation(); // released before the start came back
            }
        });
    }

    /// <summary>One ~100 ms chunk from the microphone (or a probe's synthesizer), on the owner thread.</summary>
    public void Feed(short[] chunk)
    {
        if (store.Ask.Dictation.Id is { } id) Send(chunk, id);
        else if (store.Ask.Dictation.Phase.Kind == AskDictation.PhaseKind.Starting || micStartedEarly) pendingChunks.Add(chunk);
    }

    /// <summary>
    /// 153 §14.1 / §8.7: the Ask bar's dictation options — the companion surface's preset, read by the agent (never polished), and
    /// <c>press_ago_ms</c> = now − the key press on our clock at the moment of the call (null for probes, which have no press).
    /// </summary>
    public DictationOpts OptsNow() => new()
    {
        Surface = Rpc.Surface.Companion,
        Destination = Destination.AgentPrompt,
        PressAgoMs = pressAt is { } p ? (uint)Math.Clamp((clock() - p).TotalMilliseconds, 0, uint.MaxValue) : null,
    };

    /// <summary>The text landed while the key was still down (the engine ended the dictation on "send it"): close the microphone.</summary>
    private void LandedWhileHeld()
    {
        pressAt = null;
        if (!Recording) return;
        Recording = false;
        surface.CancelMicrophone();
    }

    /// <summary>
    /// 153 §8.2 (the Mac hints, H3-B1): <c>DictationDevice</c> for the live dictation shows "mic → ‹name›" / "mic lost — reconnecting…"
    /// in the status line instead of ending it. (The companion streams its own microphone, which the engine never re-arms, so in
    /// practice these arrive only for an engine-mic dictation; the hint is cheap and keeps the clients alike.)
    /// </summary>
    private void OnStreamItem(string type, byte[] data)
    {
        if (type != "DictationDevice" || !Live) return;
        DictationDevice d;
        try { d = DictationDevice.Decode(data); }
        catch (Exception) { return; }
        if (d.Id is null || d.Id != store.Ask.Dictation.Id) return;
        Status = DeviceHint(d);
    }

    /// <summary>The status line for a <c>DictationDevice</c> item: "mic → ‹name›" · "mic lost — reconnecting…".</summary>
    public static string DeviceHint(DictationDevice d) =>
        d.State == DeviceState.Lost || d.Device is null ? "mic lost — reconnecting…" : $"mic → {d.Device}";

    /// <summary>153 §6.1: the Ask text is going out — report what was sent for the dictation whose text the field held.</summary>
    private void OnCommitting(StoreModel.Held h)
    {
        if (h.Action is not CardAction.AskTheuth ask || dictatedId is not { } id) return;
        dictatedId = null;
        log($"dictation_correction({id}) at send ({ask.Text.Length} chars)");
        Enqueue(() => calls.CorrectionAsync(id, ask.Text));
    }

    private void Send(short[] chunk, string id) => Enqueue(() => calls.AudioAsync(id, chunk));

    /// <summary><c>dictation_stop</c> after every queued chunk; the text is final once <c>DictationFinished</c> arrives.</summary>
    public void EndDictation()
    {
        if (store.Ask.Dictation.Id is not { } id)
        {
            stopWanted = store.Ask.Dictation.Phase.Kind == AskDictation.PhaseKind.Starting;
            return;
        }
        store.Ask.Dictation.Handle(AskDictation.Event.Stop.Instance);
        Enqueue(async () =>
        {
            await calls.StopAsync(id);
            log($"dictation_stop({id}) ok");
        });
        // The engine never sent DictationFinished: after the cap the text so far lands.
        finishCap?.Dispose();
        finishCap = scheduler.After(FinishCap, () =>
        {
            finishCap = null;
            if (voice.Current.Kind == AskVoiceMachine.StateKind.Stopping)
            {
                VoiceEvent(new AskVoiceMachine.Event.Dictated(store.Ask.Dictation.LiveText));
            }
        });
    }

    /// <summary>The microphone failed after it opened (unplugged, the device went away): the press fails like a refusal.</summary>
    public void MicrophoneFailed(string why)
    {
        if (voice.IsBusy) VoiceEvent(new AskVoiceMachine.Event.Failed(why));
    }

    /// <summary>The Store reduced a <c>Dictation*</c> event: show the live text, pulse the mic, finish the machine.</summary>
    private void DictationChanged(AskDictation d, bool take)
    {
        if (take && Live)
        {
            Text = d.FinalText;
            Partial = d.PartialText;
        }
        Level = d.Levels.Count == 0 ? 0 : d.Levels.Max();
        if (d.IsActive)
        {
            // 153 §7.3: "send it" armed on a partial — the engine sends once the speaker settles; speech without it disarms.
            if (Live && Recording) Status = d.SendIt ? SendingSoon : "listening…";
            return;
        }
        finishCap?.Dispose();
        finishCap = null;
        if (voice.Current.Kind == AskVoiceMachine.StateKind.Stopping)
        {
            VoiceEvent(d.Error is { } err ? new AskVoiceMachine.Event.Failed(err) : new AskVoiceMachine.Event.Dictated(d.LiveText, d.SendIt));
        }
        else if (d.Error is { } e && Live)
        {
            Apply(new[] { AskVoiceMachine.Effect.Fail(e) }); // an Error finish while still recording
        }
        else if (Live && voice.Current.Kind == AskVoiceMachine.StateKind.Recording)
        {
            // 153 §7.3: the engine stopped the dictation itself (the "send it" settle and silence) while the key is still held:
            // the text lands now (SendNow when the phrase was there), the microphone closes and the release does nothing.
            swallowKeyUp = true;
            VoiceEvent(new AskVoiceMachine.Event.Dictated(d.LiveText, d.SendIt));
        }
        else if (Live && voice.Current.Kind == AskVoiceMachine.StateKind.Idle)
        {
            // A probe's dictation (no voice press): the text lands in the field, nothing is sent.
            Live = false;
            Text = d.LiveText;
            Partial = "";
        }
    }

    // MARK: probes: the same path without a microphone and without a send

    /// <summary>
    /// <c>--synthetic-dictation</c>: <paramref name="seconds"/> of the 440 Hz tone at 16 kHz in 100 ms chunks through
    /// <c>dictation_start/audio/stop</c>, paced in real time (Swift <c>probeDictation</c>).
    /// </summary>
    public void ProbeTone(double seconds, Action done)
    {
        int total = (int)(seconds * 10);
        int sent = 0;
        ProbeStream(() => sent < total ? AskAudio.Tone(sent++ * AskAudio.ChunkSamples) : null, done);
    }

    /// <summary><c>--synthetic-speech</c>: arbitrary 16 kHz mono samples (e.g. synthesized speech) through the same path.</summary>
    public void ProbeSamples(short[] pcm, Action done)
    {
        int offset = 0;
        ProbeStream(() =>
        {
            if (offset >= pcm.Length) return null;
            int n = Math.Min(AskAudio.ChunkSamples, pcm.Length - offset);
            var c = pcm.AsSpan(offset, n).ToArray();
            offset += n;
            return c;
        }, done);
    }

    /// <summary>
    /// 153 W153B (<c>--ask-fixture</c>): the field driven by RECORDED <c>Dictation*</c> events as if a voice press dictated them —
    /// press → (no microphone, no <c>dictation_start</c>) → each event through the Store's reducer every <paramref name="pace"/> →
    /// release before the <c>DictationFinished</c> item → the machine's SetText (the hold) or SendNow ("send it": no hold). Any
    /// other item (a <c>DictionarySuggestion</c>) goes through the Store as well. Every item's <c>id</c> is rewritten to
    /// <paramref name="id"/>. The sends that follow are real (the desk's <c>send_input</c>).
    /// </summary>
    public void ProbeFixture(IReadOnlyList<string> events, string id, TimeSpan pace, Action done)
    {
        var at = clock();
        voice.OpenMicOnKeyDown = false;
        voice.Handle(new AskVoiceMachine.Event.KeyDown(at));
        voice.Handle(new AskVoiceMachine.Event.Tick(at.AddSeconds(AskVoiceMachine.HoldThreshold)));
        store.Ask.Dictation.Handle(AskDictation.Event.Start.Instance);
        store.Ask.Dictation.Handle(new AskDictation.Event.Started(id));
        Live = true;
        Recording = true;
        Status = "listening…";
        Partial = "";
        var queue = new Queue<string>(events);
        probeTimer?.Dispose();
        probeTimer = scheduler.Every(pace, () =>
        {
            if (queue.Count == 0)
            {
                probeTimer?.Dispose();
                probeTimer = null;
                done();
                return;
            }
            var json = queue.Dequeue();
            if (WireEvent.TypeOf(System.Text.Encoding.UTF8.GetBytes(json)) == "DictationFinished" && voice.IsRecording)
            {
                voice.Handle(new AskVoiceMachine.Event.KeyUp(clock())); // the release (its StopDictation effect is not performed: no engine dictation)
                Recording = false;
                Status = "finishing…";
                store.Ask.Dictation.Handle(AskDictation.Event.Stop.Instance);
            }
            store.HandleEvent(System.Text.Encoding.UTF8.GetBytes(WithId(json, id)));
        });
    }

    /// <summary>
    /// 153 W153B (<c>--synthetic-speech "…" --ask-press</c>): real engine dictation of synthesized samples INSIDE a voice press — the
    /// machine goes press → recording (no microphone), the samples stream through <c>dictation_start(16000, opts)</c> /
    /// <c>dictation_audio</c>, then the release (<c>dictation_stop</c>) — so the engine's shaped text and a spoken "send it" land
    /// through the same SetText / SendNow path as a real hold (the engine may end it first on the send-it settle).
    /// </summary>
    public void ProbeSpeechPress(short[] pcm, Action done)
    {
        var at = clock();
        pressAt = at;
        voice.OpenMicOnKeyDown = false;
        voice.Handle(new AskVoiceMachine.Event.KeyDown(at));
        voice.Handle(new AskVoiceMachine.Event.Tick(at.AddSeconds(AskVoiceMachine.HoldThreshold)));
        Recording = true;
        Status = "listening…";
        BeginDictation();
        int offset = 0;
        probeTimer?.Dispose();
        probeTimer = scheduler.Every(TimeSpan.FromMilliseconds(100), () =>
        {
            if (offset < pcm.Length && voice.IsRecording)
            {
                int n = Math.Min(AskAudio.ChunkSamples, pcm.Length - offset);
                Feed(pcm.AsSpan(offset, n).ToArray());
                offset += n;
                return;
            }
            probeTimer?.Dispose();
            probeTimer = null;
            if (voice.IsRecording) Apply(voice.Handle(new AskVoiceMachine.Event.KeyUp(clock()))); // the release → dictation_stop
            done();
        });
    }

    /// <summary>The item with its <c>id</c> member replaced (a recorded dictation id → the probe's).</summary>
    public static string WithId(string json, string id)
    {
        var node = System.Text.Json.Nodes.JsonNode.Parse(json)!.AsObject();
        if (node.ContainsKey("id") && node["type"]?.GetValue<string>() is { } type && type.StartsWith("Dictation", StringComparison.Ordinal) && type != "DictationModel")
        {
            node["id"] = id;
        }
        return node.ToJsonString();
    }

    private void ProbeStream(Func<short[]?> next, Action done)
    {
        Prewarm();
        BeginDictation();
        probeTimer?.Dispose();
        probeTimer = scheduler.Every(TimeSpan.FromMilliseconds(100), () =>
        {
            if (next() is { } chunk)
            {
                Feed(chunk);
                return;
            }
            probeTimer?.Dispose();
            probeTimer = null;
            EndDictation();
            done();
        });
    }

    private static string Describe(Exception e) => e is Rpc.RpcException r ? r.ToString() : e.Message;
}