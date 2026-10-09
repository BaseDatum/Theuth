using Microsoft.UI.Dispatching;
using Theuth.Companion.Core;
using Theuth.Companion.Rpc;

namespace Theuth.Companion.Voice;

/// <summary>
/// 149 WW2 (§3.2 per-row mic, §3.3 V on a card; ruling 3): hold-to-talk into ONE session with the companion's own microphone
/// and the engine's streamed dictation — <c>dictation_prewarm</c> · <c>dictation_start(16000)</c> · <c>dictation_audio</c> ·
/// <c>dictation_stop</c> (the Ask bar's calls, <c>Store.Dictation</c>), the Store's <see cref="AskDictation"/> reducer for the
/// live text and levels, then the caller sends the text with <c>send_input(session, text)</c> after the 3 s hold. One owner
/// thread. The Ask bar is untouched: it keeps routing through theuth's desk (D21).
/// 153 W153B: <c>dictation_start(16000, {surface: companion, destination: agent_prompt, press_ago_ms})</c>; the live text is the
/// engine's <c>shaped</c> (the Store's reducer); <see cref="SendIt"/> tells the caller the spoken "send it" ended the dictation
/// (§7.4: send at once, no hold); at the commit of the row's send the text actually sent goes back as <c>dictation_correction</c>
/// (§6.1); a dictation the engine ends while the key is still held (the "send it" settle) closes the recorder.
/// </summary>
internal sealed class RowTalk
{
    private readonly Store store;
    private readonly DispatcherQueue dispatcher;
    private readonly VoiceRecorder recorder;
    private readonly List<short[]> pending = new();
    private Task chain = Task.CompletedTask;
    private bool prewarmed;
    private bool stopWanted;
    private DispatcherQueueTimer? finishCap;
    /// <summary>153 §8.7: when the press began (press_ago_ms at dictation_start).</summary>
    private DateTimeOffset pressAt;
    /// <summary>153 §6.1: the last dictation that produced text, and the session it went into — reported once at the commit.</summary>
    private (string Id, string Session)? dictated;

    public RowTalk(Store store, DispatcherQueue dispatcher)
    {
        this.store = store;
        this.dispatcher = dispatcher;
        recorder = new VoiceRecorder(dispatcher);
        recorder.OnChunk = Feed;
        recorder.OnFailed = why => Fail(why);
        store.DictationChanged += OnDictation;
        store.Committing += OnCommitting;
    }

    /// <summary>153 §7.4: the dictation that just finished ended on the spoken "send it" — the caller sends without the hold.</summary>
    public bool SendIt { get; private set; }

    /// <summary>Between <see cref="Begin"/> and the finish.</summary>
    public bool Busy { get; private set; }

    /// <summary>What is being dictated into (a session id); the caller's key.</summary>
    public string? Target { get; private set; }

    /// <summary>The live text (finals + the open partial) while recording.</summary>
    public event Action<string>? LiveText;

    /// <summary>The last <c>DictationLevel</c> packet while recording.</summary>
    public event Action<IReadOnlyList<float>>? Levels;

    /// <summary>The dictation ended: the text (may be empty) and the error, if any (the §7.4 #8 line when the mic is off).</summary>
    public event Action<string, string?>? Finished;

    public void Prewarm()
    {
        if (prewarmed) return;
        prewarmed = true;
        Enqueue(async () =>
        {
            try { await store.Dictation.PrewarmAsync(); }
            catch (Exception e) { Log.Write($"row talk: dictation_prewarm failed: {e.Message}"); }
        });
    }

    /// <summary>The press: open the mic and the dictation; chunks before the id are held and flushed in order.</summary>
    public void Begin(string target)
    {
        if (Busy) return;
        Busy = true;
        Target = target;
        SendIt = false;
        pressAt = DateTimeOffset.Now;
        stopWanted = false;
        pending.Clear();
        store.Ask.Dictation.Handle(AskDictation.Event.Start.Instance);
        _ = StartMicAsync();
        Enqueue(async () =>
        {
            string id;
            try
            {
                id = await store.Dictation.StartAsync(new DictationOpts
                {
                    Surface = Surface.Companion,
                    Destination = Destination.AgentPrompt,
                    PressAgoMs = (uint)Math.Clamp((DateTimeOffset.Now - pressAt).TotalMilliseconds, 0, uint.MaxValue),
                });
            }
            catch (Exception e)
            {
                var why = e.Message;
                dispatcher.TryEnqueue(() =>
                {
                    store.Ask.Dictation.Handle(new AskDictation.Event.StartFailed(why));
                    Fail(store.Ask.Dictation.Error ?? why);
                });
                return;
            }
            dispatcher.TryEnqueue(() =>
            {
                store.Ask.Dictation.Handle(new AskDictation.Event.Started(id));
                foreach (var c in pending) Send(c, id);
                pending.Clear();
                if (stopWanted)
                {
                    stopWanted = false;
                    End();
                }
            });
        });
    }

    private async Task StartMicAsync()
    {
        var err = await recorder.StartAsync();
        if (err is null) return;
        dispatcher.TryEnqueue(() => Fail(err == VoiceRecorder.BlockedStatus ? AgentsModel.MicDenied : err));
    }

    private void Feed(short[] chunk)
    {
        if (!Busy) return;
        if (store.Ask.Dictation.Id is { } id) Send(chunk, id);
        else if (store.Ask.Dictation.Phase.Kind == AskDictation.PhaseKind.Starting) pending.Add(chunk);
    }

    private void Send(short[] chunk, string id) => Enqueue(() => store.Dictation.AudioAsync(id, chunk));

    /// <summary>The release: stop the mic, flush, <c>dictation_stop</c>; <c>DictationFinished</c> (or the 5 s cap) ends it.</summary>
    public void End()
    {
        if (!Busy) return;
        recorder.Stop();
        if (store.Ask.Dictation.Id is not { } id)
        {
            stopWanted = store.Ask.Dictation.Phase.Kind == AskDictation.PhaseKind.Starting;
            if (!stopWanted) Finish(store.Ask.Dictation.LiveText, store.Ask.Dictation.Error);
            return;
        }
        store.Ask.Dictation.Handle(AskDictation.Event.Stop.Instance);
        Enqueue(async () =>
        {
            try { await store.Dictation.StopAsync(id); }
            catch (Exception e) { Log.Write($"row talk: dictation_stop failed: {e.Message}"); }
        });
        finishCap?.Stop();
        finishCap = dispatcher.CreateTimer();
        finishCap.Interval = AskBarModel.FinishCap;
        finishCap.IsRepeating = false;
        finishCap.Tick += (_, _) =>
        {
            finishCap?.Stop();
            finishCap = null;
            if (Busy) Finish(store.Ask.Dictation.LiveText, null);
        };
        finishCap.Start();
    }

    /// <summary>Esc during the press: drop everything.</summary>
    public void Cancel()
    {
        if (!Busy) return;
        recorder.Cancel();
        if (store.Ask.Dictation.Id is { } id)
        {
            store.Ask.Dictation.Handle(AskDictation.Event.Stop.Instance);
            Enqueue(async () =>
            {
                try { await store.Dictation.StopAsync(id); }
                catch (Exception) { }
            });
        }
        Busy = false;
        Target = null;
        finishCap?.Stop();
        finishCap = null;
    }

    private void Fail(string why)
    {
        if (!Busy) return;
        recorder.Cancel();
        Finish("", why);
    }

    private void Finish(string text, string? error)
    {
        if (!Busy) return;
        Busy = false;
        finishCap?.Stop();
        finishCap = null;
        recorder.Cancel(); // 153 §7.3: the engine may end it (the "send it" settle) while the key is still held — the mic closes either way
        var target = Target;
        Target = null;
        SendIt = error is null && store.Ask.Dictation.SendIt;
        dictated = error is null && text.Trim().Length > 0 && target is not null && store.Ask.Dictation.LastId is { } id ? (id, target) : null;
        Log.Write($"row talk: finished target={target} chars={text.Length} error={error ?? "none"} sendIt={(SendIt ? "true" : "false")}");
        Finished?.Invoke(text.Trim(), error);
    }

    /// <summary>
    /// 153 §6.1: a held send is going out — when it carries text into the session this row mic dictated into, report the sent text
    /// for that dictation, once. (The agents panel's target is the row's session; a card's V targets the card's session.)
    /// </summary>
    private void OnCommitting(StoreModel.Held h)
    {
        if (dictated is not { } d || StoreModel.SentText(h.Action) is not { } sent) return;
        if (StoreModel.SentSession(h.Action) is { } session && session != d.Session) return;
        if (StoreModel.SentSession(h.Action) is null && h.Action is not CardAction.SendNextStep) return;
        dictated = null;
        Log.Write($"row talk: dictation_correction({d.Id}) at send ({sent.Length} chars)");
        Enqueue(() => store.Dictation.CorrectionAsync(d.Id, sent));
    }

    private void OnDictation(AskDictation d, bool take)
    {
        if (!Busy) return;
        LiveText?.Invoke(d.LiveText);
        Levels?.Invoke(d.Levels);
        if (!d.IsActive) Finish(d.LiveText, d.Error);
    }

    private void Enqueue(Func<Task> step)
    {
        chain = chain.ContinueWith(_ => step(), TaskScheduler.Default).Unwrap();
    }
}
