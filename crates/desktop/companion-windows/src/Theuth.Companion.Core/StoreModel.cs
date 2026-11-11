using System.ComponentModel;
using System.Runtime.CompilerServices;
using Theuth.Companion.Rpc;

namespace Theuth.Companion.Core;

/// <summary>
/// The engine state every surface renders, plus the hold queue — the port of <c>SWIFT/Companion/Store.swift</c> and the
/// store half of <c>AskBar/Store+Ask.swift</c> (design 134 §5.4). One owner thread (the app's UI thread) calls every
/// member and receives every <see cref="PropertyChanged"/> and event; the engine client's callbacks must be marshalled
/// to it before <see cref="HandleState"/> / <see cref="HandleEvent"/> (the app's <c>Store</c> does it with
/// <c>DispatcherQueue.TryEnqueue</c>). Async work resumes on the owner thread through its SynchronizationContext.
/// Portable: engine, timers and clock are injected, so the debounce, hold/undo and answered-elsewhere paths run on the Mac.
/// </summary>
public partial class StoreModel : INotifyPropertyChanged
{
    /// <summary>What a committed keystroke does, with the words the receipt row shows.</summary>
    public sealed record Held(string ItemId, string Who, string Label, CardAction Action);

    /// <summary>The panel's receipt row (Swift <c>Store.Receipt</c>).</summary>
    public abstract record ReceiptRow
    {
        private ReceiptRow() { }

        public sealed record Holding(Held Held) : ReceiptRow;

        public sealed record Sent(string Text) : ReceiptRow;

        public sealed record Undone : ReceiptRow
        {
            public static readonly Undone Instance = new();
        }

        /// <summary>"Already answered elsewhere": gone from the re-read list, or the engine said <c>false</c>. Never retried.</summary>
        public sealed record Stale : ReceiptRow
        {
            public static readonly Stale Instance = new();
        }

        public sealed record Failed(string Error) : ReceiptRow;

        /// <summary>
        /// W3/W4: the desk's streamed answer ("theuth · handed to w3 — …" · Open). <paramref name="Text"/> is the receipt line (the first two
        /// lines, marks stripped). 189w D4: <paramref name="Markdown"/> is the whole reply so far — what the Ask bar renders as markdown while
        /// it arrives; empty while waiting and for a failed or empty turn — and <paramref name="Streaming"/> says the turn is still going.
        /// </summary>
        public sealed record Reply(string Who, string Text, string? Session, string Markdown = "", bool Streaming = false) : ReceiptRow;
    }

    public static readonly TimeSpan RefetchDebounce = TimeSpan.FromMilliseconds(250);
    public static readonly TimeSpan TickInterval = TimeSpan.FromMilliseconds(100);

    private readonly IStoreEngine engine;
    private readonly IStoreScheduler scheduler;
    private readonly Func<DateTimeOffset> clock;
    private readonly Action<string> log;
    private UndoQueue<Held> undo;
    private IDisposable? refetchTimer;
    private IDisposable? tickTimer;
    private IDisposable? receiptTimer;
    private bool fetching;
    private bool fetchAgain;
    private bool activityLogged;

    // 189w: the one writer of the connection's event focus, the Ask's catch-up from the transcript, its watchdog.
    private readonly EventFocus focus;
    private readonly AskWatchdog watchdog = new();
    private IDisposable? watchdogTimer;
    private IDisposable? settleTimer;
    private bool catchUpRunning;
    private (string Trigger, bool Settled)? catchUpAgain;
    private int opens;

    private ValueList<AttentionItem> items = new();
    private ValueList<AgentSummary> agents = new();
    private ValueList<AgentInfo> activity = new();
    private readonly Dictionary<string, ApprovalRequested> approvals = new(StringComparer.Ordinal);
    private readonly Dictionary<string, Packet> packets = new(StringComparer.Ordinal);
    private Rail rail = Rail.Empty;
    private RpcClient.State connection = RpcClient.State.Connecting;
    private ReceiptRow? receipt;
    private DateTimeOffset now;
    private string? noteFor;

    public StoreModel(IStoreEngine engine, IStoreScheduler scheduler, Func<DateTimeOffset>? clock = null, Action<string>? log = null)
    {
        this.engine = engine;
        this.scheduler = scheduler;
        this.clock = clock ?? (() => DateTimeOffset.Now);
        this.log = log ?? (_ => { });
        now = this.clock();
        undo = new UndoQueue<Held>(UndoDelay);
        focus = new EventFocus(sessions => engine.SetEventFocusAsync(sessions), this.log);
    }

    /// <summary>
    /// 189w D1: the connection's event focus — the union of every claim (the Ask's desk, the voice key's desk). Nothing else may call
    /// <c>set_event_focus</c>: a second writer replaces the set and the first one's deltas vanish.
    /// </summary>
    public EventFocus Focus => focus;

    /// <summary>
    /// 189w D1: the voice key's / command mode's claim on <paramref name="session"/> (its own <c>desk_open</c>, before its
    /// <c>send_input</c>), merged into the union and sent. Never throws: a refused focus is logged and the send goes on.
    /// </summary>
    public Task ClaimDeltasAsync(string owner, string session, string reason) => focus.ClaimAsync(owner, new[] { session }, reason);

    // MARK: published state (PropertyChanged on the owner thread)

    public event PropertyChangedEventHandler? PropertyChanged;

    public IReadOnlyList<AttentionItem> Items => items;
    public IReadOnlyList<AgentSummary> Agents => agents;
    public IReadOnlyList<AgentInfo> Activity => activity;
    public IReadOnlyDictionary<string, ApprovalRequested> Approvals => approvals;
    public IReadOnlyDictionary<string, Packet> Packets => packets;
    public Rail Rail { get => rail; private set => Set(ref rail, value); }

    /// <summary>Mutated in place; every mutation the Store makes raises <c>Panel</c>.</summary>
    public PanelModel Panel { get; } = new(Array.Empty<AttentionItem>());

    public RpcClient.State Connection { get => connection; private set => Set(ref connection, value); }
    public ReceiptRow? Receipt { get => receipt; private set => Set(ref receipt, value); }

    /// <summary>Advanced every 100 ms during a hold (the countdown).</summary>
    public DateTimeOffset Now { get => now; private set => Set(ref now, value); }

    /// <summary>A review "Send back…" is collecting its note for this task.</summary>
    public string? NoteFor { get => noteFor; set => Set(ref noteFor, value); }

    /// <summary>W3/W4: Ask bar routing, the Done zone's receipts, the R field, the live dictation. Mutated in place; raises <c>Ask</c>.</summary>
    public AskModel Ask { get; } = new();

    /// <summary>Settings › Undo delay, seconds (0–5); read at every hold (W3 sets it from the settings).</summary>
    public double UndoDelay { get; set; } = UndoQueue<Held>.DefaultDelay;

    /// <summary>W4: <c>--ask-probe</c> — log the desk routing and the receipts refetch.</summary>
    public bool AskProbe { get; set; }

    public double HoldRemaining => undo.Remaining(Now);

    public HeldPreview? HeldNow => undo.Held is { } h ? new HeldPreview(h.Action, h.Since) : null;

    public sealed record HeldPreview(Held Held, DateTimeOffset Since);

    // MARK: hooks (the owner thread)

    /// <summary>Esc with nothing held: the panel closes.</summary>
    public event Action? CloseRequested;

    /// <summary>D · O · Ctrl+O: a <c>theuth://</c> link for the main window (W2 opens it).</summary>
    public event Action<Uri>? OpenUrlRequested;

    /// <summary>Every <c>PendingPrompts</c> (the first item of each (re)subscription).</summary>
    public event Action<PendingPrompts>? PendingPromptsReceived;

    /// <summary>A full refetch landed (lists, rail, panel, packets).</summary>
    public event Action? Refetched;

    public event Action? AskBarRequested;
    public event Action? LegendRequested;
    public event Action? SettingsRequested;

    /// <summary>W4: a <c>Dictation*</c> event was reduced; <c>true</c>: the field should show <c>LiveText</c>.</summary>
    public event Action<AskDictation, bool>? DictationChanged;

    /// <summary>W4: the raw <c>Dictation*</c> line, for the probe.</summary>
    public event Action<string>? DictationWire;

    protected void OnPropertyChanged([CallerMemberName] string? name = null) =>
        PropertyChanged?.Invoke(this, new PropertyChangedEventArgs(name));

    private void Set<T>(ref T field, T value, [CallerMemberName] string? name = null)
    {
        field = value;
        OnPropertyChanged(name);
    }

    /// <summary>The injected log (the app's <c>Log.Write</c>); named so it does not shadow the app's <c>Log</c> class in Store partials.</summary>
    protected void LogLine(string line) => log(line);

    // MARK: connection and events

    public void HandleState(RpcClient.State state)
    {
        Connection = state;
        if (state != RpcClient.State.Open) return;
        opens++;
        if (opens == 1) return; // the first dial is not a reconnect
        // 189w D1/D2: a new connection has no focus set, and everything between the loss and now was missed.
        LogLine($"connection: re-opened (#{opens})");
        if (focus.Union.Count > 0) _ = focus.SendAsync("reconnect");
        RequestCatchUp(CatchUpTrigger.Reconnect);
    }

    public void HandleEvent(byte[] data)
    {
        WireEvent e;
        try { e = WireEvent.Decode(data); }
        catch (Exception)
        {
            // 149 WW1: a typed kind that failed to decode still reaches the raw hook (WW3 decodes meeting events itself).
            // 189w: and the Ask's reader — a TurnFinished that does not decode as the typed shape must still end a waiting receipt.
            AskObserve(data);
            if (WireEvent.TypeOf(data) is { } t) EventReceived?.Invoke(t, data);
            return;
        }
        switch (e.Payload)
        {
            case PendingPrompts p:
                approvals.Clear();
                foreach (var a in p.Approvals) approvals.TryAdd(a.Id, a);
                OnPropertyChanged(nameof(Approvals));
                PendingPromptsReceived?.Invoke(p);
                ScheduleRefetch(immediate: true);
                break;
            case ApprovalRequested a:
                approvals[a.Id] = a;
                OnPropertyChanged(nameof(Approvals));
                ScheduleRefetch();
                break;
            case ApprovalResolved r:
                approvals.Remove(r.Id);
                OnPropertyChanged(nameof(Approvals));
                ScheduleRefetch();
                break;
            case AskRequested or AskResolved or AttentionChanged or TurnFinished:
                ScheduleRefetch();
                break;
            case Lagged:
                ScheduleRefetch(immediate: true);
                RequestCatchUp(CatchUpTrigger.Lagged); // 189w D2: the engine dropped events — a delta of the reply may be among them
                break;
            default:
                if (WireEvent.AttentionTriggers.Contains(e.Type) || e.Type == "TurnStarted" || e.Type == "SubagentSpawned") ScheduleRefetch();
                break;
        }
        Observe149(e); // 149 WW1: agents / meeting / dictation / global-key plumbing (StoreModel.WW1.cs)
        AskObserve(data);
        EventReceived?.Invoke(e.Type, data); // 149 WW1: the raw hook, last, on the owner thread
    }

    /// <summary>Debounced 250 ms (130 §5.2); never on a timer of its own.</summary>
    private void ScheduleRefetch(bool immediate = false)
    {
        refetchTimer?.Dispose();
        refetchTimer = null;
        if (immediate)
        {
            _ = Refetch();
            return;
        }
        refetchTimer = scheduler.After(RefetchDebounce, () =>
        {
            refetchTimer = null;
            _ = Refetch();
        });
    }

    /// <summary>The three lists at once, then the packets; one in flight, a request during it runs once after it.</summary>
    public async Task Refetch()
    {
        if (fetching)
        {
            fetchAgain = true;
            return;
        }
        fetching = true;
        try
        {
            var a = engine.AttentionListAsync();
            var g = engine.AgentsListAsync();
            var l = engine.ListActivityAsync();
            await Task.WhenAll(a, g, l);
            Apply(a.Result, g.Result, l.Result);
            await LoadPackets();
            Refetched?.Invoke();
            _ = Refetch149Async(); // 149 WW1: sessions · sessions_status · mesh_status (StoreModel.WW1.cs)
        }
        catch (Exception e)
        {
            log($"refetch failed: {Describe(e)}");
        }
        finally
        {
            fetching = false;
        }
        if (fetchAgain)
        {
            fetchAgain = false;
            await Refetch();
        }
    }

    private void Apply(ValueList<AttentionItem> newItems, ValueList<AgentSummary> newAgents, ValueList<AgentInfo> newActivity)
    {
        items = newItems;
        agents = newAgents;
        activity = newActivity;
        OnPropertyChanged(nameof(Items));
        OnPropertyChanged(nameof(Agents));
        OnPropertyChanged(nameof(Activity));
        Rail = RailModel.Derive(items, agents, activity, clock());
        Panel.WorkingCount = Rail.WorkingCount;
        Panel.Update(items);
        OnPropertyChanged(nameof(Panel));
        _ = AskRefetchReceipts();
    }

    /// <summary><c>packet_get</c> for every decision/review in the list that has none cached yet.</summary>
    private async Task LoadPackets()
    {
        var keys = items.Select(i => i.Packet).OfType<string>().Distinct(StringComparer.Ordinal).Where(k => !packets.ContainsKey(k)).ToList();
        bool changed = false;
        foreach (var key in keys)
        {
            try
            {
                if (await engine.PacketGetAsync(key) is { } p)
                {
                    packets[key] = p;
                    changed = true;
                }
            }
            catch (Exception) { }
        }
        if (changed) OnPropertyChanged(nameof(Packets));
    }

    // MARK: the card

    public Card CardFor(AttentionItem item) => Card.Make(
        item,
        item.Packet is { } k && packets.TryGetValue(k, out var p) ? p : null,
        item.Approval is { } a && approvals.TryGetValue(a, out var ap) ? ap : null,
        agents.FirstOrDefault(x => x.File.Name == item.Agent)?.File.Model);

    public Card? FocusedCard => Panel.Focused is { } i ? CardFor(i) : null;

    public void FocusOn(string id)
    {
        Panel.FocusOn(id);
        OnPropertyChanged(nameof(Panel));
    }

    // MARK: keys → actions

    public void Perform(Intent intent)
    {
        if (PerformAsk(intent)) return;
        switch (intent.Kind)
        {
            case IntentKind.Answer:
            {
                if (FocusedCard is not { } c || c.Options.FirstOrDefault(o => o.Key == intent.Option) is not { } o) return;
                Pick(o, c);
                break;
            }
            case IntentKind.AnswerRecommended:
            {
                if (FocusedCard is not { } c || c.Recommended is not { } o) return; // P13: nothing pre-picked → Enter is inert
                Pick(o, c);
                break;
            }
            case IntentKind.Escape:
                if (NoteFor is not null)
                {
                    NoteFor = null;
                    return;
                }
                if (undo.Cancel())
                {
                    Receipt = ReceiptRow.Undone.Instance;
                    ExpireReceipt(1.5);
                    return;
                }
                CloseRequested?.Invoke();
                break;
            case IntentKind.Next:
                Panel.Move(1);
                OnPropertyChanged(nameof(Panel));
                break;
            case IntentKind.Previous:
                Panel.Move(-1);
                OnPropertyChanged(nameof(Panel));
                break;
            case IntentKind.Snooze:
            {
                if (FocusedCard is not { } c) return;
                Hold(new Held(c.Item.Id, c.Who, "snoozed 30 m", new CardAction.Snooze(c.Item.Id)));
                break;
            }
            case IntentKind.Clear:
            {
                if (FocusedCard is not { } c || !c.Clearable) return;
                Hold(new Held(c.Item.Id, c.Who, "cleared", new CardAction.Dismiss(c.Item.Id)));
                break;
            }
            case IntentKind.OpenDiff or IntentKind.OpenBoard:
                if (FocusedCard is { } card) OpenUrlRequested?.Invoke(card.AttentionUrl);
                break;
            case IntentKind.OpenSession:
                if (FocusedCard?.SessionUrl is { } url) OpenUrlRequested?.Invoke(url);
                break;
            case IntentKind.Legend:
                LegendRequested?.Invoke();
                break;
            case IntentKind.Settings:
                SettingsRequested?.Invoke();
                break;
            default:
                break; // none · send; reply · askBar are PerformAsk's
        }
    }

    public void Pick(CardOption option, Card card)
    {
        if (option.Action is CardAction.ReviewSendBack sb)
        {
            NoteFor = sb.Task;
            return;
        }
        Hold(new Held(card.Item.Id, card.Who, option.Label, option.Action));
    }

    /// <summary>The typed send-back note; empty is refused (the note is required).</summary>
    public void SubmitNote(string note)
    {
        var text = note.Trim();
        if (NoteFor is not { } task || text.Length == 0 || FocusedCard is not { } c) return;
        NoteFor = null;
        Hold(new Held(c.Item.Id, c.Who, "sent back", new CardAction.ReviewSendBackNote(task, text)));
    }

    /// <summary>
    /// Hold <paramref name="h"/> for <see cref="UndoDelay"/> s, then commit it. A hold while another is held commits the
    /// displaced one at once (130 §5.4, <see cref="UndoQueue{TAction}"/>).
    /// </summary>
    public void Hold(Held h)
    {
        var displaced = undo.Held?.Action;
        undo = new UndoQueue<Held>(UndoDelay); // the delay setting is read at every hold
        if (displaced is not null) _ = Commit(displaced);
        undo.Hold(h, clock());
        Receipt = new ReceiptRow.Holding(h);
        receiptTimer?.Dispose();
        receiptTimer = null;
        tickTimer?.Dispose();
        Now = clock();
        tickTimer = scheduler.Every(TickInterval, Tick);
    }

    private void Tick()
    {
        Now = clock();
        if (undo.Tick(Now) is { } fired)
        {
            tickTimer?.Dispose();
            tickTimer = null;
            _ = Commit(fired);
        }
        else if (undo.Held is null)
        {
            tickTimer?.Dispose();
            tickTimer = null;
        }
    }

    /// <summary>E5: re-read, then send; the engine's <c>false</c> is "already answered elsewhere", never retried.</summary>
    private async Task Commit(Held h)
    {
        Committing?.Invoke(h); // 153 W153B (§6.1): the fields report dictation_correction for the text that is going out
        if (h.Action is CardAction.AskTheuth ask)
        {
            await CommitAsk(ask.Text);
            return;
        }
        if (h.Action.ItemFree)
        {
            await CommitItemFree(h); // 149 WW2
            return;
        }
        try
        {
            var latest = await engine.AttentionListAsync();
            if (!StaleGuard.Allows(h.ItemId, latest))
            {
                Receipt = ReceiptRow.Stale.Instance;
                Apply(latest, agents, activity);
                ExpireReceipt(3);
                return;
            }
            bool ok = true;
            switch (h.Action)
            {
                case CardAction.AnswerAsk a: ok = await engine.AnswerAskAsync(a.AskId, a.Answers); break;
                case CardAction.Decide d: await engine.TaskDecideAsync(d.Decision, d.Answer); break;
                case CardAction.Approve ap: ok = await engine.ResolveApprovalAsync(ap.ApprovalId, ap.Decision); break;
                case CardAction.ReviewApprove ra: await engine.TaskMoveAsync(ra.Task, "done", null); break;
                case CardAction.ReviewSendBackNote rn: await engine.TaskMoveAsync(rn.Task, "todo", rn.Note); break;
                case CardAction.ReviewSendBack: return;
                case CardAction.SendNextStep ns:
                    await engine.SendInputAsync(ns.Session, ns.Text);
                    await engine.AttentionDismissAsync(h.ItemId);
                    break;
                case CardAction.Dismiss dm: await engine.AttentionDismissAsync(dm.Id); break;
                case CardAction.Snooze sn:
                    var until = (ulong)((clock().ToUnixTimeMilliseconds() / 1000.0 + KeyGrammar.SnoozeInterval) * 1000);
                    await engine.AttentionSnoozeAsync(sn.Id, until);
                    break;
                case CardAction.DenyWithNote dn:
                    ok = await engine.ResolveApprovalAsync(dn.ApprovalId, Decision.Deny);
                    if (ok && dn.Session is { } s) await engine.SteerAsync(s, dn.Note);
                    break;
                case CardAction.HireAgent ha: await engine.AgentApproveAsync(ha.Name); break; // 134 W2
                case CardAction.DeclineHire dh: await engine.AgentRemoveAsync(dh.Name, null); break; // 134 W2
                case CardAction.Unblock ub: await engine.TaskMoveAsync(ub.Task, "todo", null); break; // 134 W2
                default: return;
            }
            Receipt = ok ? new ReceiptRow.Sent($"{h.Who} · {h.Label}") : ReceiptRow.Stale.Instance;
            log($"committed {h.Action} ok={(ok ? "true" : "false")}");
        }
        catch (Exception e)
        {
            Receipt = new ReceiptRow.Failed(Describe(e));
            log($"commit failed: {Describe(e)}");
        }
        _ = Refetch();
        ExpireReceipt(3);
    }

    /// <summary>149 WW2 (D3): stop a tree · pause a desk · talk into a row — no item, so no E5 re-read; the hold was the confirmation.</summary>
    private async Task CommitItemFree(Held h)
    {
        try
        {
            switch (h.Action)
            {
                case CardAction.StopTree st: await engine.CancelAsync(st.Session, CancelScope.Tree); break;
                case CardAction.PauseDesk pd: await engine.AgentPauseAsync(pd.Name, true); break;
                case CardAction.SendInputTo si: // 149 WW3: the parked P screenshots ride with the reply
                    if (TakeReplyAttachments(si.Session) is { Count: > 0 } files) await engine.SendInputAsync(si.Session, si.Text, files);
                    else await engine.SendInputAsync(si.Session, si.Text);
                    break;
                default: return;
            }
            Receipt = new ReceiptRow.Sent(h.Action switch
            {
                CardAction.StopTree => $"{h.Who} · stopped",
                CardAction.PauseDesk => $"{h.Who} · paused",
                _ => $"{h.Who} · sent",
            });
            log($"committed {h.Action}");
        }
        catch (Exception e)
        {
            Receipt = new ReceiptRow.Failed(Describe(e));
            log($"commit failed: {Describe(e)}");
        }
        _ = Refetch();
        ExpireReceipt(3);
    }

    /// <summary>The receipt row clears after <paramref name="seconds"/> unless something is held by then.</summary>
    public void ExpireReceipt(double seconds)
    {
        receiptTimer?.Dispose();
        receiptTimer = scheduler.After(TimeSpan.FromSeconds(seconds), () =>
        {
            receiptTimer = null;
            if (undo.Held is null) Receipt = null;
        });
    }

    /// <summary>W3/W4: the Ask bar's receipts share this row.</summary>
    public void SetReceipt(ReceiptRow? r) => Receipt = r;

    public void SetReceiptCount(int n)
    {
        Panel.ReceiptCount = n;
        OnPropertyChanged(nameof(Panel));
    }

    /// <summary>
    /// A deliberate tap on a notification action (D8, W3): the same commit path, no hold; an optional note goes to the
    /// session as <c>send_input</c> (R1-5's "deny with a note").
    /// </summary>
    public void AnswerNow(Held h, string? note = null, string? session = null)
    {
        _ = Commit(h);
        if (note is null || session is null) return;
        _ = SendQuietly(session, note);
    }

    private async Task SendQuietly(string session, string text)
    {
        try { await engine.SendInputAsync(session, text); }
        catch (Exception) { }
    }

    // MARK: R · Ctrl+L · the Done zone (Store+Ask.swift)

    /// <summary>Intents the Ask half owns; true when handled.</summary>
    private bool PerformAsk(Intent intent)
    {
        switch (intent.Kind)
        {
            case IntentKind.Reply:
                if (FocusedCard is { } c && c.ReplyAction("") is not null)
                {
                    Ask.ReplyFor = c.Item.Id;
                    OnPropertyChanged(nameof(Ask));
                }
                return true;
            case IntentKind.AskBar:
                AskBarRequested?.Invoke();
                return true;
            case IntentKind.Escape when Ask.ReplyFor is not null:
                Ask.ReplyFor = null;
                OnPropertyChanged(nameof(Ask));
                return true;
            case IntentKind.Clear when FocusedReceipt is not null:
                Ask.Receipts.Clear(FocusedReceipt!.Id);
                OnPropertyChanged(nameof(Ask));
                SetReceiptCount(Ask.Receipts.Count);
                return true;
            case IntentKind.OpenSession or IntentKind.OpenBoard when FocusedReceipt is not null:
                if (FocusedReceipt!.Session is { } s) OpenUrlRequested?.Invoke(DeepLink.Session(s));
                return true;
            default:
                return false;
        }
    }

    public Core.Receipt? FocusedReceipt =>
        Panel.ReceiptFocus is int i && i < Ask.Receipts.Entries.Count ? Ask.Receipts.Entries[i] : null;

    /// <summary>The typed R reply; empty is refused. Goes through the same hold and E5 guard as a keystroke.</summary>
    public void SubmitReply(string text)
    {
        var t = text.Trim();
        if (t.Length == 0 || FocusedCard is not { } c || c.ReplyAction(t) is not { } action) return;
        Ask.ReplyFor = null;
        OnPropertyChanged(nameof(Ask));
        Hold(new Held(c.Item.Id, c.Who, "replied", action));
    }

    /// <summary>Enter in the Ask bar: the same hold (Esc undoes), then the desk routing.</summary>
    public void SubmitAsk(string text)
    {
        var t = text.Trim();
        if (t.Length == 0) return;
        Hold(new Held("", "theuth", t, new CardAction.AskTheuth(t)));
    }

    /// <summary>
    /// After the hold: resolve theuth's desk, claim its deltas, <c>send_input(desk, text, [], null, null)</c>, then collect the reply.
    /// 189w D1: the claim goes out on EVERY ask (the desk may have moved; the connection may be a new one) and is answered before the
    /// input leaves, so no delta of this reply can meet a stale focus set; the desk's first resolve already sent one, which counts.
    /// </summary>
    private async Task CommitAsk(string text)
    {
        long mark = focus.Requested;
        try
        {
            var session = await ResolveDesk();
            focus.Claim("ask", new[] { session });
            await focus.EnsureSinceAsync(mark, "ask");
            await engine.SendInputAsync(session, text);
            var id = $"ask:{Guid.NewGuid().ToString().ToUpperInvariant()}";
            AskSent(session, text);
            Ask.Current = id;
            Ask.Receipts.Add(new Core.Receipt { Id = id, Kind = ReceiptKind.Ask, At = clock(), Who = "theuth", Text = text, Session = session }, clock());
            OnPropertyChanged(nameof(Ask));
            SetReceiptCount(Ask.Receipts.Count);
            SetReceipt(new ReceiptRow.Reply("theuth", "sent — waiting for the desk", session));
            log($"ask sent to desk {session} ({text.Length} chars)");
        }
        catch (Exception e)
        {
            Ask.Router.Forget(); // a refused or closed desk is resolved again on the next send
            OnPropertyChanged(nameof(Ask));
            SetReceipt(new ReceiptRow.Failed(Describe(e)));
            ExpireReceipt(6);
            log($"ask failed: {Describe(e)}");
        }
    }

    /// <summary>
    /// <c>send_input</c> went out: collect the desk's reply from now until its turn finishes, and watch the stream for silence
    /// (<see cref="AskWatchdog"/>). Also the start of a <c>--ask-replay</c>.
    /// </summary>
    protected void AskSent(string session, string text)
    {
        Ask.Router.Sent(session, text);
        if (Ask.Router.Reply is { } r) r.SentAt = clock();
        catchUpAgain = null;
        settleTimer?.Dispose();
        settleTimer = null;
        watchdog.Arm(clock());
        ArmWatchdog(AskWatchdog.Interval);
    }

    /// <summary><c>mesh_status</c> → on the hub <c>desk_open("theuth")</c>; off it, a replicated desk id or a refusal. Cached.
    /// Public for W4's <c>--ask-probe</c>, which resolves the desk without sending (Swift <c>AskProbe.run</c>).</summary>
    public async Task<string> ResolveDesk()
    {
        if (Ask.Router.DeskSession is { } known) return known;
        if (Ask.Router.Desk is AskDesk.Unavailable u) throw new InvalidOperationException(u.Reason);
        try
        {
            MeshStatus? mesh = null;
            try { mesh = await engine.MeshStatusAsync(); }
            catch (Exception) { } // engines without a mesh actor are their own hub
            switch (AskRouter.Plan(mesh, agents))
            {
                case AskPlan.Use use:
                    Ask.Router.Resolved(use.Session, hub: false);
                    if (AskProbe) log($"desk from agents_list = {use.Session} hub=false (hub is {mesh?.HubName ?? "?"})");
                    return use.Session;
                case AskPlan.Refuse refuse:
                    Ask.Router.Refused(refuse.Reason);
                    if (AskProbe) log($"desk refused: {refuse.Reason}");
                    throw new InvalidOperationException(refuse.Reason);
                default:
                    var s = await engine.DeskOpenAsync("theuth");
                    Ask.Router.Resolved(s, hub: true);
                    if (AskProbe) log($"desk_open(theuth) = {s} hub=true self={mesh?.SelfName ?? "no mesh"} holder={mesh?.LeaseInfo.Holder ?? "nil"}");
                    return s;
            }
        }
        finally
        {
            OnPropertyChanged(nameof(Ask));
            FocusDeltas("resolve");
        }
    }

    /// <summary>
    /// 129 §3.2 / 189w D1: this connection wants the desk's deltas (the Ask's claim, sent through <see cref="Focus"/>, the only writer);
    /// re-applied on the desk's first resolve, on every <c>PendingPrompts</c>, on every ask and on every reconnect; pre-129 engines refuse, fine.
    /// </summary>
    private void FocusDeltas(string reason)
    {
        if (Ask.Router.DeskSession is not { } s) return;
        focus.Claim("ask", new[] { s });
        _ = focus.SendAsync(reason);
    }

    /// <summary>
    /// Every event, raw: <c>TextDelta</c>/<c>TurnFinished</c>/<c>Error</c> on the desk feed the reply (a failed turn ends the
    /// receipt with "theuth couldn't reply — …", W6 item 7); <c>Dictation*</c> the live dictation. 189w D2: any event for the desk
    /// restarts the watchdog's silence; a normal <c>TurnFinished</c> is settled by the transcript (<see cref="RequestCatchUp"/>), not by the stream.
    /// </summary>
    private void AskObserve(byte[] data)
    {
        StreamDelta d;
        try { d = StreamDelta.Decode(data); }
        catch (Exception) { return; }
        if (d.Type == "PendingPrompts")
        {
            FocusDeltas("pending-prompts");
            return;
        }
        if (d.Type.StartsWith("Dictation", StringComparison.Ordinal))
        {
            DictationObserve(data);
            return;
        }
        if (Ask.Router.Reply is not { Finished: false } reply || d.Session != reply.Session) return;
        watchdog.Touch(clock());
        switch (d.Type)
        {
            case "TurnStarted":
                if (d.TurnSeq is { } seq && reply.Turn is null && !reply.SawFinish)
                {
                    reply.Turn = seq;
                    log($"ask: the desk started turn_seq={seq}");
                }
                return;
            case "TextDelta":
            {
                var piece = d.Text ?? "";
                if (!Ask.Router.Observe(new ReplyEvent.TextDelta(reply.Session, piece))) return;
                if (reply.StreamedChars == 0) log($"ask: first TextDelta ({piece.Length} chars) {Since(reply)} after send");
                reply.StreamedChars += piece.Length;
                ShowReply(reply);
                return;
            }
            case "Error":
                if (Ask.Router.Observe(new ReplyEvent.Error(reply.Session, d.Message ?? ""))) FinishAsk(reply, "Error event", $"ask failed on the desk: {d.Message}");
                return;
            case "TurnFinished":
                if (AskRouter.IsFailure(d.StopReason))
                {
                    if (Ask.Router.Observe(new ReplyEvent.TurnFinished(reply.Session, d.StopReason)))
                        FinishAsk(reply, $"TurnFinished {d.StopReason}", $"ask failed on the desk: stop_reason={d.StopReason}");
                    return;
                }
                reply.SawFinish = true;
                log($"ask: TurnFinished ({d.StopReason ?? "?"}) for the desk after {reply.StreamedChars} streamed chars — catching up from the transcript");
                RequestCatchUp(CatchUpTrigger.TurnFinished);
                return;
            default:
                return;
        }
    }

    /// <summary>"1234 ms" since the ask's send_input returned.</summary>
    private string Since(AskReply reply) => reply.SentAt is { } at ? $"{(clock() - at).TotalMilliseconds:0} ms" : "? ms";

    /// <summary>The receipt row and the Done zone follow the reply: the first two lines, and (189w D4) the whole markdown while it streams.</summary>
    private void ShowReply(AskReply reply)
    {
        var text = reply.FirstTwoLines;
        if (Ask.Current is { } id) Ask.Receipts.Update(id, text);
        OnPropertyChanged(nameof(Ask));
        string markdown = reply.Failed || reply.NoReply ? "" : reply.Text;
        SetReceipt(new ReceiptRow.Reply("theuth", text.Length == 0 ? "…" : text, reply.Session, markdown, Streaming: !reply.Finished));
    }

    /// <summary>
    /// The ask is over (a reply, a failure, or "finished without a reply"): stop watching, show it, let the row linger, speak it.
    /// 189w D6 logs the finish — how it ended, how many characters, how many of them streamed.
    /// </summary>
    private void FinishAsk(AskReply reply, string how, string? failedLog)
    {
        watchdog.Disarm();
        watchdogTimer?.Dispose();
        watchdogTimer = null;
        settleTimer?.Dispose();
        settleTimer = null;
        catchUpAgain = null;
        ShowReply(reply);
        if (failedLog is not null) log(failedLog);
        log($"ask finished ({how}): {reply.Text.Length} chars, {reply.StreamedChars} streamed, {(reply.Failed ? "failed" : reply.NoReply ? "no reply" : "ok")}");
        ExpireReceipt(reply.Failed || reply.NoReply ? 12 : AskCatchUp.LingerSeconds(reply.Text));
        // Spoken replies (2026-10-09): the whole reply, once final and successful — never a partial, never a failed turn.
        if (!reply.Failed && !reply.NoReply && reply.Text.Trim().Length > 0) AskReplyFinished?.Invoke(reply.Session, reply.Text);
    }

    // MARK: 189w D2 — the Ask catches up from the transcript

    private void ArmWatchdog(TimeSpan after)
    {
        watchdogTimer?.Dispose();
        watchdogTimer = scheduler.After(after, OnWatchdog);
    }

    /// <summary>The watchdog's timer fired: catch up when the desk has been silent for the whole interval, else wait out the rest of it.</summary>
    private void OnWatchdog()
    {
        watchdogTimer = null;
        if (Ask.Router.Reply is not { Finished: false })
        {
            watchdog.Disarm();
            return;
        }
        var verdict = watchdog.Decide(clock());
        if (!verdict.Armed) return;
        if (verdict.CatchUp)
        {
            log($"ask watchdog: no event for the desk in {AskWatchdog.Interval.TotalSeconds:0} s — catching up from the transcript");
            watchdog.CaughtUp(clock());
            RequestCatchUp(CatchUpTrigger.Watchdog);
            ArmWatchdog(AskWatchdog.Interval);
        }
        else
        {
            ArmWatchdog(verdict.Wait);
        }
    }

    /// <summary>
    /// Read the desk's transcript for the ask that is collecting (D2): after a <c>TurnFinished</c>, a <c>Lagged</c>, a reconnect and the
    /// watchdog. One read at a time; a trigger that arrives meanwhile runs once after it (a finish outranks the others). Nothing when no ask collects.
    /// </summary>
    private void RequestCatchUp(string trigger, bool settled = false)
    {
        if (Ask.Router.Reply is not { Finished: false } reply) return;
        if (catchUpRunning)
        {
            if (catchUpAgain is null || trigger == CatchUpTrigger.TurnFinished) catchUpAgain = (trigger, settled);
            return;
        }
        _ = CatchUpAsync(reply, trigger, settled);
    }

    private async Task CatchUpAsync(AskReply reply, string trigger, bool settled)
    {
        catchUpRunning = true;
        try
        {
            SessionTail? tail = null;
            string? unread = null;
            try { tail = await LoadTailWithin(reply.Session, AskCatchUp.Deadline); }
            catch (Exception e) { unread = Describe(e); }
            if (!ReferenceEquals(Ask.Router.Reply, reply) || reply.Finished)
            {
                log($"ask catch-up ({trigger}) dropped: the ask moved on while load_session was out");
                return;
            }
            watchdog.CaughtUp(clock());
            var decision = AskCatchUp.Decide(trigger, tail, reply.AskText, reply.Turn, reply.Text, settled);
            log($"ask catch-up ({trigger}{(settled ? ", re-check" : "")}): {(tail is null ? $"transcript unread ({unread})" : $"{tail.History.Count} entries")}, "
                + $"{(decision.Text ?? "").Length} chars from the transcript ({reply.StreamedChars} streamed), running={(tail is { Running: true } ? "true" : "false")}, via={decision.Via} → {decision.Action}");
            if (decision.Turn is { } turn && reply.Turn is null) reply.Turn = turn;
            switch (decision.Action)
            {
                case CatchUpAction.Finish:
                    reply.Complete(decision.Text ?? reply.Text); // the transcript's text replaces what streamed
                    FinishAsk(reply, $"{trigger} · {decision.Via}", null);
                    break;
                case CatchUpAction.Fail:
                    if (decision.NoReply) reply.FinishWithoutReply();
                    else reply.Fail(decision.Error ?? "the turn failed");
                    FinishAsk(reply, $"{trigger} · {decision.Via}", reply.Failed ? $"ask failed on the desk: {reply.Error}" : null);
                    break;
                case CatchUpAction.Settle:
                    settleTimer?.Dispose();
                    settleTimer = scheduler.After(AskCatchUp.SettleDelay, () =>
                    {
                        settleTimer = null;
                        RequestCatchUp(trigger, settled: true);
                    });
                    break;
            }
        }
        finally
        {
            catchUpRunning = false;
            if (catchUpAgain is { } again)
            {
                catchUpAgain = null;
                RequestCatchUp(again.Trigger, again.Settled);
            }
        }
    }

    /// <summary><c>load_session</c> with a deadline: an engine that never answers must not leave the ask waiting (it counts as unreadable).</summary>
    private async Task<SessionTail> LoadTailWithin(string session, TimeSpan deadline)
    {
        var load = LoadTailAsync(session, AskCatchUp.Limit);
        var late = new TaskCompletionSource<SessionTail>();
        using var timer = scheduler.After(deadline, () => late.TrySetException(new TimeoutException($"load_session did not answer within {deadline.TotalSeconds:0} s")));
        _ = load.ContinueWith(static t => { _ = t.Exception; }, TaskContinuationOptions.OnlyOnFaulted); // observed even when the deadline won
        var first = await Task.WhenAny(load, late.Task);
        return await first;
    }

    /// <summary>The transcript read (<c>load_session</c>); a seam for the app's <c>--ask-replay</c>, whose desk exists nowhere.</summary>
    protected virtual Task<SessionTail> LoadTailAsync(string session, uint limit) => engine.LoadSessionAsync(session, limit);

    /// <summary>An Ask theuth reply (typed or dictated) is final: (session, the full text). The app speaks it when Settings › Ask bar says so.</summary>
    public event Action<string, string>? AskReplyFinished;

    /// <summary>Reduce one <c>Dictation*</c> event (W6 live dictation).</summary>
    private void DictationObserve(byte[] data)
    {
        DictationWire?.Invoke(System.Text.Encoding.UTF8.GetString(data));
        DictationEvent w;
        try { w = DictationEvent.Decode(data); }
        catch (Exception) { return; }
        if (w.Id is not { } id) return;
        bool take;
        switch (w.Type)
        {
            case "DictationStarted": take = Ask.Dictation.Handle(new AskDictation.Event.Started(id)); break;
            case "DictationText": take = Ask.Dictation.Handle(new AskDictation.Event.Text(id, w.Utterance ?? 0, w.DisplayText ?? "", w.IsFinal ?? false, w.Command?.IsSend == true)); break; // 153 W153B: shaped text + "send it"
            case "DictationLevel": take = Ask.Dictation.Handle(new AskDictation.Event.Level(id, w.Levels ?? new ValueList<float>())); break;
            case "DictationFinished":
                take = Ask.Dictation.Handle(new AskDictation.Event.Finished(id, w.Reason?.Reason == "error" ? w.Reason.Message ?? "dictation failed" : null));
                break;
            default: return;
        }
        OnPropertyChanged(nameof(Ask));
        DictationChanged?.Invoke(Ask.Dictation, take);
    }

    /// <summary>The last 24 h of <c>user@companion</c> activity (E4) → the Done zone; runs after every refetch.</summary>
    private async Task AskRefetchReceipts()
    {
        var since = (ulong)Math.Max(0, (clock().ToUnixTimeMilliseconds() / 1000.0 - ReceiptModel.Window) * 1000);
        try
        {
            var entries = await engine.ActivityListAsync(new ActivityFilter { SinceMs = since, Limit = 200 });
            Ask.Receipts.Merge(entries, clock());
            OnPropertyChanged(nameof(Ask));
            SetReceiptCount(Ask.Receipts.Count);
            if (AskProbe && !activityLogged)
            {
                activityLogged = true;
                log($"activity_list(24 h) = {entries.Count} entries, {entries.Count(x => x.Actor == Core.Receipt.CompanionActor)} by {Core.Receipt.CompanionActor} → receipts={Ask.Receipts.Count}");
            }
        }
        catch (Exception e)
        {
            log($"activity_list failed: {Describe(e)}");
        }
    }

    /// <summary>An error as the Swift log prints it: <c>rpc &lt;code&gt;: &lt;message&gt;</c> for engine errors.</summary>
    protected static string Describe(Exception e) => e is RpcException r ? r.ToString() : e.Message;
}