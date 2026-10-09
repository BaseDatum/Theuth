using System.Text.Json;
using Microsoft.UI.Dispatching;
using Theuth.Companion.Core;
using Theuth.Companion.Island;
using Theuth.Companion.Rpc;
using Theuth.Companion.Voice;

namespace Theuth.Companion.AnyApp;

/// <summary>
/// 153 §3.2 / §13 / §13.9 — dictate into any app, the Windows twin of wave D. A gesture (the dictate-anywhere key once W153C's PIN-C lands;
/// until then the <c>--probe-anyapp*</c> flags) runs: the assessment at the press (UIA, before the mic is live) → refuse with a hint, or listen
/// (the companion's own PCM into <c>dictation_start(16000, {surface: any_app, destination: any_app(exe), app})</c>) → every closed utterance is
/// delivered as it closes, serially: direct (SendInput KEYEVENTF_UNICODE, ≤ 200 single-line chars, read back at 150/350 ms) or the clipboard
/// lease (snapshot → write with the history hints and the session marker → Ctrl+V → settle at 500 ms, restore only if still ours) with the
/// verification triple → release flushes the open utterance, "send it" presses Return where §7.4 allows it → "✓ N lines into ‹app›", or the
/// failure card with the words that did not land. A non-empty selection makes the gesture a rewrite (§13.6). Nothing is delivered while the
/// session is locked (WTS). Everything runs on the UI thread; UIA and the clipboard run on their own worker threads.
/// </summary>
internal sealed class AnyAppController
{
    private readonly Store store;
    private readonly DispatcherQueue dispatcher;
    private readonly AnyAppSettings settings;
    private readonly FocusReader reader = new();
    private readonly SessionWindow window;
    private readonly ClipboardIo clipboard;
    private readonly SessionLockGate lockGate;
    private readonly VoiceRecorder recorder;
    private readonly DictationLease dictation = new();
    private readonly DispatcherQueueTimer pillTimer;
    private FailureCardWindow? card;
    private RewritePanelWindow? panel;
    private RewriteState? rewrite;
    private Gesture? live;
    private int generation;
    private Task? starting;

    public AnyAppController(Store store, DispatcherQueue dispatcher, AnyAppSettings settings)
    {
        this.store = store;
        this.dispatcher = dispatcher;
        this.settings = settings;
        window = new SessionWindow();
        clipboard = new ClipboardIo(window.Handle);
        bool? lockedNow = AnyAppNative.SessionLocked();
        lockGate = new SessionLockGate(lockedNow == true);
        window.SessionChange += OnSessionChange;
        recorder = new VoiceRecorder(dispatcher);
        recorder.OnChunk = OnChunk;
        recorder.OnFailed = why => dispatcher.TryEnqueue(() =>
        {
            Log.Write($"anyapp: mic failed: {why}");
            if (live is { Source: GestureSource.Key }) Cancel($"mic: {why}", why);
        });
        pillTimer = dispatcher.CreateTimer();
        pillTimer.IsRepeating = false;
        pillTimer.Tick += (_, _) => { pillTimer.Stop(); if (live is null) Pill(null); };
        store.DictationWire += OnDictationWire;
        _ = reader.WarmAsync();
        Log.Write($"anyapp: installed — {settings.Describe()} · session {(lockedNow is null ? "lock state unknown" : lockedNow == true ? "LOCKED" : "unlocked")} · chord {Chord}");
    }

    /// <summary>The running controller (the probes and, after PIN-C, the key events reach it here).</summary>
    public static AnyAppController? Current { get; set; }

    public AnyAppCounters Counters { get; } = new();

    public bool Busy => live is not null;

    public bool Locked => lockGate.Locked;

    public SessionWindow Window => window;

    public ClipboardIo Clipboard => clipboard;

    public FocusReader Reader => reader;

    /// <summary>The dictate-anywhere chord as the key table has it (W153C's row, resolved by name so this compiles against any skeleton).</summary>
    public string Chord
    {
        get
        {
            foreach (var name in new[] { "DictateAnywhere", "DictateAnywhereKey", "AnyApp" })
            {
                if (Enum.TryParse<KeyAction>(name, out var a) && store.Keys.ChordFor(a) is { } c) return c.Symbol;
            }
            return "Ctrl+Alt+D";
        }
    }

    // MARK: the gesture (key events after PIN-C; the probes before)

    private enum GestureSource { Key, Fixture }

    private sealed class Gesture
    {
        public int Generation;
        public string Id = "";
        public GestureSource Source;
        public FocusCapture Capture = FocusCapture.None("");
        public Assessment Assessment = new(TargetVerdict.Unknown, false, "", RefusalReason.None, false, false, "");
        public AppProfile Profile = new();
        public ClipboardLease Lease = new("");
        public readonly Queue<(int Utterance, string Text)> Pending = new();
        public readonly HashSet<int> Delivered = new();
        public readonly List<FailureCardModel> Failures = new();
        public readonly List<string> Instruction = new();
        public bool Delivering;
        public bool FellBack;
        public int DeliveredChars;
        public int Lines;
        public string? Previous;
        public bool SendIt;
        public bool Ended;
        public bool Cancelled;
        public bool LockedHold;
        public string? DictationId;
        public string Live = "";
        public string Partial = "";
        public TaskCompletionSource<bool> Finished = new(TaskCreationOptions.RunContinuationsAsynchronously);
        public TaskCompletionSource Drained = new(TaskCreationOptions.RunContinuationsAsynchronously);
        public bool RewriteMode;
        public string Selection = "";
        public bool Paused;
        public DateTimeOffset Pressed = DateTimeOffset.Now;
    }

    private sealed record RewriteState(FocusCapture Capture, Assessment Assessment, AppProfile Profile, string Selection, List<(string Instruction, string Result)> Thread)
    {
        public string? Proposal { get; set; }
    }

    /// <summary>
    /// The press (§3.2 "assessing", ≤ 150 ms before the mic is live): read the focused element, assess it, refuse with a hint or start
    /// listening. <paramref name="fixture"/>: no microphone — utterances come from <see cref="Feed"/> (the probe). False when refused or busy.
    /// </summary>
    public async Task<bool> BeginAsync(string source, bool fixture = false)
    {
        if (live is not null) { Log.Write($"anyapp: press ({source}) ignored — a gesture is live"); return false; }
        if (!settings.Enabled) { Hint("Dictate anywhere is off — Settings › Dictation"); return false; }
        int gen = ++generation;
        var pressed = DateTimeOffset.Now;
        var capture = await reader.CaptureAsync();
        if (gen != generation || live is not null) return false;
        var profile = settings.ProfileFor(capture.Facts.Exe, capture.Facts.App);
        bool terminal = AppMatch.IsTerminal(capture.Facts.Exe, settings.Find(capture.Facts.Exe)?.Terminal);
        var a = DeliveryAssessment.Assess(capture.Facts, lockGate.Locked, terminal);
        LastAssessment = a;
        LastCapture = capture;
        Log.Write($"anyapp: press ({source}) → {Describe(capture, a)} · read {capture.ReadMs} ms");
        Counters.Gestures++;
        bool refine = panel is { IsOpen: true } && rewrite is not null;
        if (a.Refused && !refine)
        {
            Counters.Refused++;
            Hint(AnyAppCopy.Pill(a));
            return false;
        }
        var g = new Gesture
        {
            Generation = gen, Id = Guid.NewGuid().ToString("N")[..12], Source = fixture ? GestureSource.Fixture : GestureSource.Key,
            Capture = capture, Assessment = a, Profile = profile, Pressed = pressed,
        };
        g.Lease = new ClipboardLease(g.Id, settings.LeaveTextOnClipboard);
        if (refine)
        {
            g.RewriteMode = true;
            g.Selection = rewrite!.Selection;
            Log.Write($"anyapp: press while the rewrite panel is up → refine ({rewrite.Thread.Count} earlier turn(s))");
        }
        else if (a.Rewrite)
        {
            var sel = await reader.SelectionAsync(capture);
            if (gen != generation) return false;
            if (sel is { Length: > 0 })
            {
                g.RewriteMode = true;
                g.Selection = sel;
            }
        }
        live = g;
        RenderListening(g);
        if (g.Source == GestureSource.Key) starting = StartDictationAsync(g);
        return true;
    }

    /// <summary>The last press's assessment and capture (the probe's refusal card, the logs).</summary>
    public Assessment? LastAssessment { get; private set; }

    public FocusCapture? LastCapture { get; private set; }

    /// <summary>The live gesture is a rewrite (a selection was captured at the press).</summary>
    public bool InRewrite => live is { RewriteMode: true };

    /// <summary>The probe's <c>--probe-anyapp-lease</c>: this gesture delivers by the lease even where direct typing would do.</summary>
    public void ForceLease()
    {
        if (live is not { } g) return;
        g.FellBack = true;
        Log.Write("anyapp: lease forced for this gesture (probe)");
    }

    /// <summary>The probe's stand-in for the engine's stream: one closed (final) utterance.</summary>
    public void Feed(string text, bool sendIt = false)
    {
        if (live is not { } g) return;
        int u = g.Delivered.Count + g.Pending.Count + g.Instruction.Count;
        if (sendIt) g.SendIt = true;
        OnFinal(g, u, text);
    }

    /// <summary>Release (hold ≥ 200 ms), the tap that ends a lock, or the engine's "send it" stop (§3.2 "finishing").</summary>
    public async Task EndAsync(string reason)
    {
        if (live is not { Ended: false } g) return;
        g.Ended = true;
        Pill(new VoiceView("sending", g.Live, $"{AnyAppCopy.Pill(g.Assessment)} · finishing…", null, null, g.LockedHold, true));
        Log.Write($"anyapp: end ({reason}) — {g.Pending.Count} queued, {g.Lines} delivered");
        if (g.Source == GestureSource.Key) await StopDictationAsync(g);
        if (g.RewriteMode)
        {
            await RunRewriteAsync(g);
            Finish(g, null);
            return;
        }
        if (g.Delivering || g.Pending.Count > 0)
        {
            var first = await Task.WhenAny(g.Drained.Task, Task.Delay(30_000));
            if (first != g.Drained.Task) Log.Write("anyapp: deliveries still running after 30 s — finishing with what landed");
        }
        if (g.Cancelled) return;
        string? caption = null;
        if (g.SendIt && g.Lines > 0)
        {
            bool same = await reader.FocusSameAsync(g.Capture);
            caption = DeliveryAssessment.ReturnWithheld(g.Assessment, same, Typist.ModifierDown);
            if (caption is null)
            {
                bool ok = Typist.Return();
                Log.Write($"anyapp: send it → Return into {g.Assessment.App} ({(ok ? "sent" : "SendInput refused")})");
            }
            else Log.Write($"anyapp: send it → no Return ({caption})");
        }
        Finish(g, caption);
    }

    /// <summary>Esc, a note under 800 ms, the mic failing, the lock: nothing more is typed; a holding lease restores the clipboard now.</summary>
    public void Cancel(string reason, string? hint = null)
    {
        if (live is not { } g) return;
        g.Cancelled = true;
        g.Pending.Clear();
        Log.Write($"anyapp: cancel ({reason}) — {g.Lines} line(s) were delivered");
        if (g.Source == GestureSource.Key)
        {
            recorder.Cancel();
            var id = dictation.Close();
            if (id is not null) _ = store.Dictation.StopAsync(id).ContinueWith(t => { if (t.IsFaulted) Log.Write($"anyapp: dictation_stop failed: {t.Exception?.GetBaseException().Message}"); }, TaskScheduler.Default);
        }
        _ = SettleNowAsync(g);
        live = null;
        g.Drained.TrySetResult();
        if (hint is not null) Hint(hint);
        else Pill(null);
    }

    private void Finish(Gesture g, string? caption)
    {
        if (live == g) live = null;
        Counters.Log();
        if (g.Failures.Count > 0)
        {
            ShowCard(g);
            Pill(null);
            return;
        }
        if (g.RewriteMode) { Pill(null); return; }
        string done = g.Lines > 0 ? AnyAppCopy.Done(g.Lines, g.Assessment.App) : "Nothing recognized";
        Pill(new VoiceView("done", "", caption is null ? done : $"{done} · {caption}", null, null, false, false));
        After(DeliveryAssessment.DoneMs);
        Log.Write($"anyapp: done — {done}{(caption is null ? "" : $" · {caption}")} ({g.DeliveredChars} chars, {(DateTimeOffset.Now - g.Pressed).TotalMilliseconds:0} ms)");
    }

    // MARK: the engine dictation (the companion's PCM, design 54 §8 + 153 opts)

    private async Task StartDictationAsync(Gesture g)
    {
        int lg = dictation.Begin();
        var destination = Destination.AnyApp(g.Capture.Facts.Exe.Length > 0 ? g.Capture.Facts.Exe : g.Assessment.App);
        var opts = new DictationOpts
        {
            Surface = Surface.AnyApp,
            Destination = destination,
            App = g.Profile,
            Commands = g.Profile.Commands,
            PressAgoMs = (uint)Math.Max(0, (DateTimeOffset.Now - g.Pressed).TotalMilliseconds),
        };
        string id;
        try { id = await store.Client.DictationStartAsync(AskAudio.SampleRate, opts); } // always 16 000: null would open the ENGINE host's mic (130 W6 rule)
        catch (Exception e)
        {
            Log.Write($"anyapp: dictation_start failed: {Why(e)}");
            if (dictation.StartFailed(lg) && live == g) Cancel("dictation refused", Why(e).Contains("already", StringComparison.OrdinalIgnoreCase) ? "Already dictating" : Why(e));
            return;
        }
        var mic = await recorder.StartAsync();
        var kept = dictation.Started(lg, id);
        if (kept == LeaseVerdict.Stop || live != g)
        {
            if (kept == LeaseVerdict.Keep) dictation.Close();
            Log.Write($"anyapp: dictation {id} started after the gesture ended — stopping it");
            _ = store.Dictation.StopAsync(id);
            recorder.Cancel();
            return;
        }
        g.DictationId = id;
        if (mic is { } err) { Log.Write($"anyapp: mic: {err}"); Cancel("no microphone", err); return; }
        Log.Write($"anyapp: dictation {id} live (surface any_app, destination {destination.App}, terminal {g.Profile.Terminal}), mic {recorder.Description}");
    }

    private async Task StopDictationAsync(Gesture g)
    {
        recorder.Stop();
        if (starting is { } s) await Task.WhenAny(s, Task.Delay(PointFlow.StartWaitMs));
        var id = dictation.Close();
        if (id is null) { g.Finished.TrySetResult(false); return; }
        try { await store.Dictation.StopAsync(id); }
        catch (Exception e) { Log.Write($"anyapp: dictation_stop failed: {Why(e)}"); g.Finished.TrySetResult(false); return; }
        var first = await Task.WhenAny(g.Finished.Task, Task.Delay(PointFlow.FinalTextMs));
        if (first != g.Finished.Task)
        {
            Log.Write($"anyapp: final text not flushed within {PointFlow.FinalTextMs} ms — delivering the open words");
            if (g.Partial.Length > 0) OnFinal(g, int.MaxValue, g.Partial);
        }
    }

    private void OnChunk(short[] pcm)
    {
        if (live is not { DictationId: { } id }) return;
        _ = store.Dictation.AudioAsync(id, pcm).ContinueWith(t => { if (t.IsFaulted) Log.Write($"anyapp: dictation_audio failed: {t.Exception?.GetBaseException().Message}"); }, TaskScheduler.Default);
    }

    private void OnDictationWire(string json)
    {
        if (live is not { DictationId: { } id } g) return;
        DictationEvent w;
        try { w = DictationEvent.Decode(System.Text.Encoding.UTF8.GetBytes(json)); } catch (Exception) { return; }
        if (w.Id != id) return;
        switch (w.Type)
        {
            case "DictationText":
                if (w.Command?.IsSend == true && !g.SendIt) { g.SendIt = true; Log.Write("anyapp: \"send it\" armed (the engine settles and stops)"); }
                var text = w.DisplayText ?? "";
                if (w.IsFinal == true) { g.Partial = ""; OnFinal(g, w.Utterance ?? 0, text); }
                else { g.Partial = text; RenderListening(g); }
                break;
            case "DictationFinished":
                Log.Write($"anyapp: DictationFinished ({w.Reason?.Reason ?? "?"}{(w.Reason?.Message is { } m ? $": {m}" : "")})");
                g.Finished.TrySetResult(true);
                if (!g.Ended) _ = EndAsync("the engine finished (send it / max length)");
                break;
        }
    }

    // MARK: delivery (§13.2–§13.4), one utterance at a time

    private void OnFinal(Gesture g, int utterance, string text)
    {
        if (g.RewriteMode)
        {
            if (text.Trim().Length > 0) g.Instruction.Add(text.Trim());
            g.Live = string.Join(" ", g.Instruction);
            RenderListening(g);
            return;
        }
        if (g.Delivered.Contains(utterance))
        {
            // "scratch that" re-sends a delivered utterance empty: not undoable by re-delivery after a paste (§7.4) — say so; Ctrl+Z is the user's.
            if (text.Length == 0) Log.Write($"anyapp: \"scratch that\" for utterance {utterance}, already delivered — not re-typed (Ctrl+Z in {g.Assessment.App} undoes it)");
            return;
        }
        var kept = g.Pending.Where(p => p.Utterance != utterance).ToList(); // a re-decoded final replaces its queued text
        g.Pending.Clear();
        foreach (var p in kept) g.Pending.Enqueue(p);
        if (text.Trim().Length == 0) return;
        g.Pending.Enqueue((utterance, text));
        _ = PumpAsync(g);
    }

    private async Task PumpAsync(Gesture g)
    {
        if (g.Delivering) return;
        g.Delivering = true;
        if (g.Drained.Task.IsCompleted) g.Drained = new TaskCompletionSource(TaskCreationOptions.RunContinuationsAsynchronously);
        try
        {
            while (g.Pending.Count > 0 && !g.Cancelled)
            {
                var (u, raw) = g.Pending.Peek();
                if (!lockGate.MayDeliver)
                {
                    FailAll(g, DeliveryFailure.TargetUnavailable, "screen locked", "nothing is typed while the session is locked");
                    break;
                }
                if (!AnyAppNative.IsWindow(g.Capture.TopHwnd) && g.Capture.TopHwnd != 0)
                {
                    FailAll(g, DeliveryFailure.TargetUnavailable, null, $"{g.Assessment.App} closed");
                    break;
                }
                if (!await WaitForFocusAsync(g)) { FailAll(g, DeliveryFailure.TargetUnavailable, "focus moved", $"focus left {g.Assessment.App}"); break; }
                g.Pending.Dequeue();
                if (!DeliveryAssessment.FitsGesture(g.DeliveredChars, raw))
                {
                    Fail(g, DeliveryFailure.NoEditableTarget, raw, "over 20 000 characters in one gesture", canRetry: false, detail: "too long");
                    continue;
                }
                string text = g.Profile.Polish ? await PolishAsync(g, raw) : raw;
                var before = await reader.ReadAsync(g.Capture);
                string delivered = AnyAppSpacing.Lead(text, before.Readable ? before.BeforeCaret : null, g.Previous);
                var method = DeliveryAssessment.Method(g.Assessment, delivered, g.FellBack);
                var verdict = method == DeliveryMethod.Direct ? await DirectAsync(g, delivered, before) : await LeaseAsync(g, delivered);
                g.Delivered.Add(u);
                if (verdict is PasteVerdict.Landed or PasteVerdict.Unknown or PasteVerdict.UserActed)
                {
                    g.Lines++;
                    g.DeliveredChars += delivered.Length;
                    g.Previous = delivered;
                    g.Live = (g.Live.Length > 0 ? g.Live + " " : "") + raw;
                    if (!g.Ended) RenderListening(g);
                }
            }
        }
        catch (Exception e)
        {
            Log.Write($"anyapp: delivery failed: {e.GetType().Name}: {e.Message}");
        }
        finally
        {
            g.Delivering = false;
            if (g.Pending.Count == 0 || g.Cancelled) g.Drained.TrySetResult();
        }
        if (!g.Cancelled && g.Pending.Count > 0 && !g.Delivering) _ = PumpAsync(g);
    }

    /// <summary>§3.2 "paused": focus left the target app — wait until it is back (Esc cancels); after the gesture ended, give up after 10 s.</summary>
    private async Task<bool> WaitForFocusAsync(Gesture g)
    {
        if (await reader.FocusSameAsync(g.Capture)) return true;
        g.Paused = true;
        Log.Write($"anyapp: paused — focus left {g.Assessment.App}");
        var since = DateTimeOffset.Now;
        Pill(new VoiceView("listening", g.Live, AnyAppCopy.Paused(g.Assessment.App), null, null, g.LockedHold, false));
        while (!g.Cancelled)
        {
            await Task.Delay(300);
            if (await reader.FocusSameAsync(g.Capture))
            {
                g.Paused = false;
                Log.Write($"anyapp: resumed — focus back in {g.Assessment.App}");
                if (!g.Ended) RenderListening(g);
                return true;
            }
            if (g.Ended && DateTimeOffset.Now - since > TimeSpan.FromSeconds(10)) return false;
        }
        return false;
    }

    private async Task<string> PolishAsync(Gesture g, string text)
    {
        try
        {
            var call = store.Client.PolishAsync(text, Destination.AnyApp(g.Capture.Facts.Exe), null);
            var first = await Task.WhenAny(call, Task.Delay(4000));
            if (first != call) { Log.Write("anyapp: polish over its 4 s budget — the shaped text goes"); return text; }
            var reply = await call;
            if (reply.Text is { Length: > 0 } p) { Log.Write($"anyapp: polished by {reply.Model} in {reply.Ms} ms"); return p; }
            Log.Write($"anyapp: polish refused ({reply.Refused ?? "no reason"}) — the shaped text goes");
        }
        catch (Exception e) { Log.Write($"anyapp: polish failed ({Why(e)}) — the shaped text goes"); }
        return text;
    }

    /// <summary>§13.2: SendInput typing, read back at 150 (and 350) ms; not landed → this utterance and every later one take the lease.</summary>
    private async Task<PasteVerdict> DirectAsync(Gesture g, string text, FieldRead before)
    {
        bool ok = Typist.Type(text);
        var at = DateTimeOffset.Now;
        var verdict = ok ? await VerifyAsync(g, before, text, null, VerifyMode.Direct, at) : PasteVerdict.NotLanded;
        Log.Write($"anyapp: direct {text.Length} chars into {g.Assessment.App} → {verdict}");
        Counters.Direct++;
        if (verdict == PasteVerdict.Landed) { Counters.Landed++; return verdict; }
        g.FellBack = true;
        if (verdict != PasteVerdict.NotLanded) { Counters.Count(verdict); return verdict; }
        Log.Write("anyapp: the read-back did not see the words — the lease takes this and every later utterance (W-m)");
        return await LeaseAsync(g, text);
    }

    private sealed class LeaseRun
    {
        public ClipSnapshot? Snapshot;
        public FieldRead Before = FieldRead.Unreadable;
        public DateTimeOffset PastedAt;
        public Task<PasteVerdict>? Verify;
        public Task? Settle;
        public bool? Restored;
        public bool Failed;
        public readonly List<int> CheckedAt = new();
    }

    /// <summary>§13.3: the lease's effects, run in order; the settle and the verification run side by side; returns the verdict.</summary>
    private async Task<PasteVerdict> LeaseAsync(Gesture g, string text)
    {
        if (!g.Lease.Begin(text, out var fx)) { Log.Write("anyapp: lease slot busy (unexpected) — utterance kept for the card"); Fail(g, DeliveryFailure.ClipboardWriteFailed, text, "", canRetry: true); return PasteVerdict.NotLanded; }
        var run = new LeaseRun();
        int gen = g.Lease.Generation;
        await RunEffectsAsync(g, gen, run, fx);
        if (run.Settle is { } s) await s;
        var verdict = run.Verify is { } v ? await v : (run.Failed ? PasteVerdict.NotLanded : PasteVerdict.Unknown);
        Counters.Leases++;
        if (run.Restored == true) Counters.Restored++;
        Counters.Count(verdict);
        Log.Write($"anyapp: lease {g.Id}:{gen} {text.Length} chars into {g.Assessment.App} → {verdict} · clipboard {(run.Restored switch { true => "restored", false => "not restored", _ => "left as is" })} · snapshot {run.Snapshot?.Describe() ?? "none"}");
        if (verdict == PasteVerdict.NotLanded && !run.Failed)
        {
            string footer = AnyAppCopy.CardFooter(run.CheckedAt, PasteVerifier.FieldState(verdict), run.Restored);
            Fail(g, DeliveryFailure.PasteNotLanded, text, footer, canRetry: true);
        }
        return verdict;
    }

    private async Task RunEffectsAsync(Gesture g, int gen, LeaseRun run, IReadOnlyList<LeaseEffect> first)
    {
        var queue = new Queue<LeaseEffect>(first);
        while (queue.Count > 0)
        {
            var e = queue.Dequeue();
            IReadOnlyList<LeaseEffect> next = Array.Empty<LeaseEffect>();
            switch (e)
            {
                case LeaseEffect.TakeSnapshot:
                    run.Snapshot = await clipboard.SnapshotAsync();
                    next = g.Lease.Snapshotted(gen, run.Snapshot is not null);
                    break;
                case LeaseEffect.Write w:
                    next = g.Lease.Written(gen, await clipboard.WriteAsync(w.Text, w.Marker));
                    break;
                case LeaseEffect.Paste:
                    run.Before = settings.CheckPasteLanded ? await reader.ReadAsync(g.Capture) : FieldRead.Unreadable;
                    // UIPI silently drops SendInput into an elevated window (and SendInput cannot say so): report it as the paste failing.
                    bool ok = !g.Capture.Facts.Elevated && Typist.Paste();
                    run.PastedAt = DateTimeOffset.Now;
                    next = g.Lease.Pasted(gen, ok);
                    break;
                case LeaseEffect.ArmSettle a:
                    run.Settle = SettleAfterAsync(g, gen, run, a.Ms);
                    break;
                case LeaseEffect.Verify v:
                    run.Verify = VerifyAsync(g, run.Before, v.Text, run.Snapshot?.Text, VerifyMode.Lease, run.PastedAt, run.CheckedAt);
                    break;
                case LeaseEffect.Restore:
                    run.Restored = run.Snapshot is { } snap && await clipboard.RestoreAsync(snap);
                    next = g.Lease.Restored(gen, run.Restored == true);
                    break;
                case LeaseEffect.DropSnapshot:
                    run.Restored ??= false;
                    break;
                case LeaseEffect.Failed f:
                    run.Failed = true;
                    Fail(g, f.Kind, f.Text, AnyAppCopy.CardFooter(Array.Empty<int>(), "", run.Restored), canRetry: f.Kind != DeliveryFailure.RestoreFailed,
                        detail: f.Kind == DeliveryFailure.PasteCommandFailed && g.Capture.Facts.Elevated ? "elevated window" : null);
                    break;
                case LeaseEffect.Done d:
                    if (d.Restored) run.Restored = true;
                    break;
            }
            foreach (var n in next) queue.Enqueue(n);
        }
    }

    private async Task SettleAfterAsync(Gesture g, int gen, LeaseRun run, int ms)
    {
        if (ms > 0) await Task.Delay(ms);
        var marker = await clipboard.MarkerAsync();
        bool ours = marker == g.Lease.MarkerFor(gen);
        var fx = g.Lease.SettleDue(gen, ours);
        if (fx.Count == 0) return; // a stale generation: the gesture settled already
        if (!ours) Log.Write($"anyapp: settle {g.Id}:{gen} — the clipboard is not ours any more (the user copied): left alone");
        await RunEffectsAsync(g, gen, run, fx);
    }

    /// <summary>The gesture is cancelled while a lease holds: settle now (restore if still ours).</summary>
    private async Task SettleNowAsync(Gesture g)
    {
        if (g.Lease.Phase != LeasePhase.Holding) return;
        int gen = g.Lease.Generation;
        var marker = await clipboard.MarkerAsync();
        var fx = g.Lease.SettleNow(marker == g.Lease.MarkerFor(gen));
        await RunEffectsAsync(g, gen, new LeaseRun(), fx);
    }

    private async Task<PasteVerdict> VerifyAsync(Gesture g, FieldRead before, string text, string? restored, VerifyMode mode, DateTimeOffset t0, List<int>? checkedAt = null)
    {
        if (mode == VerifyMode.Lease && !settings.CheckPasteLanded) return PasteVerdict.Unknown;
        if (g.Assessment.Secure) return PasteVerdict.Unknown; // a secure field shows bullets, never our words: nothing to verify (§13.4 unknown)
        var reads = new List<FieldRead>();
        foreach (int at in PasteVerifier.Schedule(mode))
        {
            var wait = t0.AddMilliseconds(at) - DateTimeOffset.Now;
            if (wait > TimeSpan.Zero) await Task.Delay(wait);
            reads.Add(await reader.ReadAsync(g.Capture));
            checkedAt?.Add(at);
            if (PasteVerifier.Step(before, reads, text, restored, mode) is { } v) return v;
        }
        return PasteVerifier.Verdict(before, reads, text, restored, mode);
    }

    // MARK: failures → the card (§13.5)

    private void Fail(Gesture g, DeliveryFailure kind, string text, string footer, bool canRetry, string? detail = null)
    {
        Counters.Fail(kind);
        Log.Write($"anyapp: failure {kind}{(detail is null ? "" : $" ({detail})")} — {text.Length} chars kept for the card");
        var model = new FailureCardModel(g.Assessment.App, kind, text, footer, canRetry && g.Capture.Element is not null, detail);
        g.Failures.Add(model);
        if (live != g) ShowCard(g); // the gesture is over: the card comes now
    }

    private void FailAll(Gesture g, DeliveryFailure kind, string? detail, string footer)
    {
        var words = string.Join(" ", g.Pending.Select(p => p.Text));
        g.Pending.Clear();
        if (words.Length > 0) Fail(g, kind, words, footer, canRetry: kind != DeliveryFailure.TargetUnavailable || detail == "focus moved", detail);
    }

    private FailureCardModel? cardModel;
    private FocusCapture? cardCapture;
    private Assessment? cardAssessment;

    private void ShowCard(Gesture g)
    {
        if (g.Failures.Count == 0) return;
        var f = g.Failures;
        var model = f[0] with { Text = string.Join(" ", f.Select(x => x.Text)), Footer = f[^1].Footer.Length > 0 ? f[^1].Footer : f[0].Footer };
        g.Failures.Clear();
        ShowCard(model, g.Capture, g.Assessment);
    }

    /// <summary>Show (or replace) the card — also the probe's fixture path.</summary>
    public void ShowCard(FailureCardModel model, FocusCapture? capture, Assessment? assessment)
    {
        cardModel = model;
        cardCapture = capture;
        cardAssessment = assessment;
        if (card is null)
        {
            card = new FailureCardWindow();
            card.CopyRequested += () => _ = CardCopyAsync();
            card.RetryRequested += () => _ = CardRetryAsync();
            card.OpenRequested += () => _ = CardOpenAsync();
            card.Dismissed += how => { Log.Write($"anyapp-card: closed by {how}"); cardModel = null; };
        }
        card.Show(model);
    }

    public FailureCardWindow? Card => card;

    private async Task CardCopyAsync()
    {
        if (cardModel is not { } m || card is null) return;
        bool ok = await clipboard.CopyPlainAsync(m.Text);
        Log.Write($"anyapp-card: Copy → {(ok ? "the words are on the clipboard (no markers)" : "FAILED")}");
        card.SetStatus(ok ? "Copied — paste it where you want it" : "Couldn't use the clipboard");
    }

    private async Task CardOpenAsync()
    {
        if (cardModel is not { } m || card is null) return;
        var uri = new Uri("theuth://compose?text=" + Uri.EscapeDataString(m.Text));
        bool ok = await Ui.Links.Open(uri);
        Log.Write($"anyapp-card: Open in theuth → {(ok ? "theuth://compose" : "FAILED")}");
        if (ok) card.Dismiss("open");
        else card.SetStatus("Couldn't open theuth");
    }

    /// <summary>Try again (§13.5): focus back in the same element, then the lease once more; the card closes when the words land.</summary>
    private async Task CardRetryAsync()
    {
        if (cardModel is not { } m || card is null || cardCapture is not { } c || cardAssessment is not { } a) return;
        if (live is not null) { card.SetStatus("Finish the dictation first"); return; }
        card.SetStatus("Trying again…");
        if (!await reader.RestoreFocusAsync(c))
        {
            Log.Write($"anyapp-card: Try again — could not put focus back in {a.App}");
            card.SetStatus($"Click into {a.App} where the words should go, then Try again");
            return;
        }
        var g = new Gesture { Generation = ++generation, Id = Guid.NewGuid().ToString("N")[..12], Source = GestureSource.Fixture, Capture = c, Assessment = a, FellBack = true };
        g.Lease = new ClipboardLease(g.Id, settings.LeaveTextOnClipboard);
        var verdict = await LeaseAsync(g, m.Text);
        Log.Write($"anyapp-card: Try again → {verdict}");
        if (verdict is PasteVerdict.Landed or PasteVerdict.Unknown or PasteVerdict.UserActed && g.Failures.Count == 0)
        {
            card.Dismiss("landed");
            return;
        }
        g.Failures.Clear();
        card.SetStatus("Still not landed — Copy, or Open in theuth");
    }

    // MARK: rewrite on the selection (§13.6)

    private async Task RunRewriteAsync(Gesture g)
    {
        var instruction = string.Join(" ", g.Instruction).Trim();
        if (instruction.Length == 0) { Hint("Nothing recognized — say what to change"); return; }
        var state = rewrite is not null && panel is { IsOpen: true }
            ? rewrite
            : new RewriteState(g.Capture, g.Assessment, g.Profile, g.Selection, new List<(string, string)>());
        rewrite = state;
        await RewriteAsync(state, instruction);
    }

    /// <summary>One rewrite turn: the spoken instruction is the user message, the selection goes in through the engine's template.</summary>
    public async Task RewriteAsync(string instruction, string selection, FocusCapture capture, Assessment a, AppProfile profile)
    {
        rewrite = new RewriteState(capture, a, profile, selection, new List<(string, string)>());
        await RewriteAsync(rewrite, instruction);
    }

    private async Task RewriteAsync(RewriteState state, string instruction)
    {
        EnsurePanel();
        panel!.Show(new RewriteModel(state.Assessment.App, instruction, state.Selection, null, "rewriting…", null, true, false));
        var thread = state.Thread.Count > 0 ? state.Thread : null;
        PolishReply? reply = null;
        string? error = null;
        var started = DateTimeOffset.Now;
        try
        {
            var call = store.Client.RewriteAsync(instruction, state.Selection, state.Profile, thread);
            var first = await Task.WhenAny(call, Task.Delay(20_000));
            if (first == call) reply = await call;
            else error = "no answer in 20 s";
        }
        catch (Exception e) { error = Why(e); }
        int ms = (int)(DateTimeOffset.Now - started).TotalMilliseconds;
        if (reply is { Text: { Length: > 0 } text })
        {
            state.Thread.Add((instruction, text));
            state.Proposal = text;
            panel.Show(new RewriteModel(state.Assessment.App, instruction, state.Selection, text, $"Replace puts this over your selection · hold {Chord} to refine", $"{reply.Model ?? "model"} · {reply.Ms} ms", false, false));
            Log.Write($"anyapp: rewrite → {text.Length} chars by {reply.Model} in {reply.Ms} ms (turn {state.Thread.Count})");
        }
        else
        {
            var why = error ?? reply?.Refused ?? "no reply";
            panel.Show(new RewriteModel(state.Assessment.App, instruction, state.Selection, state.Proposal, $"couldn't rewrite — {why}", $"{ms} ms", false, true));
            Log.Write($"anyapp: rewrite failed after {ms} ms — {why}");
        }
    }

    private void EnsurePanel()
    {
        if (panel is not null) return;
        panel = new RewritePanelWindow();
        panel.ReplaceRequested += () => _ = ReplaceAsync();
        panel.Dismissed += how => { Log.Write($"anyapp-rewrite: discarded ({how})"); rewrite = null; };
    }

    public RewritePanelWindow? Panel => panel;

    /// <summary>Replace: focus back in the captured element first (hiding alone races the paste — A:L165), then deliver over the selection.</summary>
    private async Task ReplaceAsync()
    {
        if (rewrite is not { Proposal: { Length: > 0 } proposal } state || panel is null) return;
        panel.Retire();
        if (!await reader.RestoreFocusAsync(state.Capture))
        {
            Log.Write($"anyapp-rewrite: Replace — could not put focus back in {state.Assessment.App}");
            panel.Show(new RewriteModel(state.Assessment.App, state.Thread[^1].Instruction, state.Selection, proposal, $"Couldn't return to {state.Assessment.App} — click into it and press Replace again", null, false, true));
            return;
        }
        var g = new Gesture { Generation = ++generation, Id = Guid.NewGuid().ToString("N")[..12], Source = GestureSource.Fixture, Capture = state.Capture, Assessment = state.Assessment, Profile = state.Profile };
        g.Lease = new ClipboardLease(g.Id, settings.LeaveTextOnClipboard);
        var before = await reader.ReadAsync(g.Capture);
        var method = DeliveryAssessment.Method(g.Assessment, proposal, false);
        var verdict = method == DeliveryMethod.Direct ? await DirectAsync(g, proposal, before) : await LeaseAsync(g, proposal);
        Log.Write($"anyapp-rewrite: Replace → {method} · {verdict}");
        rewrite = null;
        if (g.Failures.Count > 0) ShowCard(g);
    }

    // MARK: lock (§13.8, §13.9)

    private void OnSessionChange(int code, bool synthetic)
    {
        bool changed = lockGate.OnSessionChange(code);
        string what = code switch { SessionLockGate.WtsSessionLock => "WTS_SESSION_LOCK", SessionLockGate.WtsSessionUnlock => "WTS_SESSION_UNLOCK", _ => $"0x{code:X}" };
        Log.Write($"anyapp: WM_WTSSESSION_CHANGE {what}{(synthetic ? " (synthetic, the probe)" : "")} → {(lockGate.Locked ? "locked: no delivery" : "unlocked")}{(changed ? "" : " (no change)")}");
        if (lockGate.Locked && live is { } g && !g.Cancelled)
        {
            // The mic must not keep listening on a locked screen: end the gesture; undelivered words go to the card.
            if (g.Source == GestureSource.Key) { recorder.Stop(); var id = dictation.Close(); if (id is not null) _ = store.Dictation.StopAsync(id); }
            FailAll(g, DeliveryFailure.TargetUnavailable, "screen locked", "nothing is typed while the session is locked");
            g.Ended = true;
            Finish(g, null);
        }
    }

    // MARK: the pill (the island's voice view, top-centre of the pointer's display — 149 §P)

    private void RenderListening(Gesture g)
    {
        if (g.Paused) return;
        string head = g.RewriteMode ? AnyAppCopy.RewritePill(g.Selection.Length) : AnyAppCopy.Pill(g.Assessment);
        string chord = g.Source == GestureSource.Fixture ? "probe" : Chord;
        string status = $"{head} · {AnyAppCopy.Held(chord, g.LockedHold, shape: true)}";
        string words = (g.Live + (g.Partial.Length > 0 ? " " + g.Partial : "")).Trim();
        Pill(new VoiceView("listening", words, status, null, words.Length == 0 ? AnyAppCopy.Hints : null, g.LockedHold, false));
    }

    private void Hint(string text)
    {
        Log.Write($"anyapp: hint \"{text}\"");
        Pill(new VoiceView("hint", "", text, null, text, false, false));
        After(DeliveryAssessment.HintMs);
    }

    private void After(int ms)
    {
        pillTimer.Stop();
        pillTimer.Interval = TimeSpan.FromMilliseconds(ms);
        pillTimer.Start();
    }

    private static void Pill(VoiceView? v)
    {
        if (WW3Install.Current?.Island is not { } island) return;
        island.SetVoice(v);
    }

    /// <summary>Ctrl+Alt+D double-tapped: the hold is locked (hands-free) — the pill says so (after PIN-C, from the key events).</summary>
    public void ShowLock()
    {
        if (live is not { } g) return;
        g.LockedHold = true;
        RenderListening(g);
    }

    // MARK: helpers

    private static string Describe(FocusCapture c, Assessment a) =>
        $"{a.Verdict}{(a.Secure ? " secure" : "")}{(a.Refused ? $" refused={a.Refusal}" : "")}{(a.Rewrite ? " rewrite" : "")} role={a.Role} app=\"{a.App}\" exe={c.Facts.Exe} " +
        $"ct={c.Facts.ControlType} value={(c.Facts.HasValuePattern ? (c.Facts.ValueReadOnly == true ? "ro" : "rw") : "-")} text={(c.Facts.HasTextPattern ? (c.Facts.TextReadOnly == true ? "ro" : "rw") : "-")} " +
        $"password={c.Facts.IsPassword} elevated={c.Facts.Elevated} direct={a.CanDirect} sel={c.Facts.SelectionLength?.ToString() ?? "-"} pill=\"{AnyAppCopy.Pill(a)}\"";

    private static string Why(Exception e) => e is RpcException r ? r.ToString() : e.Message;
}

/// <summary>§16.1 #26: the delivery counters the debug row reads (pasteNotLanded ≤ 1 and clipboard restored = N after N lease gestures).</summary>
internal sealed class AnyAppCounters
{
    public int Gestures, Refused, Direct, Leases, Landed, NotLanded, Unknown, UserActed, Restored;
    public readonly Dictionary<DeliveryFailure, int> Failures = new();

    public void Count(PasteVerdict v)
    {
        switch (v)
        {
            case PasteVerdict.Landed: Landed++; break;
            case PasteVerdict.NotLanded: NotLanded++; break;
            case PasteVerdict.UserActed: UserActed++; break;
            default: Unknown++; break;
        }
    }

    public void Fail(DeliveryFailure k) => Failures[k] = Failures.GetValueOrDefault(k) + 1;

    public override string ToString() =>
        $"gestures={Gestures} refused={Refused} direct={Direct} leases={Leases} landed={Landed} notLanded={NotLanded} unknown={Unknown} userActed={UserActed} restored={Restored}/{Leases}" +
        (Failures.Count > 0 ? " failures=" + string.Join(",", Failures.Select(kv => $"{kv.Key}:{kv.Value}")) : "");

    public void Log() => Theuth.Companion.Log.Write($"anyapp: counters {this}");
}
