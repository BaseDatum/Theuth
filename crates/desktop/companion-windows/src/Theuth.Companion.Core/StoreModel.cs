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

        /// <summary>W3/W4: the desk's streamed answer ("theuth · handed to w3 — …" · Open).</summary>
        public sealed record Reply(string Who, string Text, string? Session) : ReceiptRow;
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
    }

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

    public void HandleState(RpcClient.State state) => Connection = state;

    public void HandleEvent(byte[] data)
    {
        WireEvent e;
        try { e = WireEvent.Decode(data); }
        catch (Exception)
        {
            // 149 WW1: a typed kind that failed to decode still reaches the raw hook (WW3 decodes meeting events itself).
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
        if (h.Action is CardAction.AskTheuth ask)
        {
            await CommitAsk(ask.Text);
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

    /// <summary>After the hold: resolve theuth's desk once, <c>send_input(desk, text, [], null, null)</c>, then collect the reply.</summary>
    private async Task CommitAsk(string text)
    {
        try
        {
            var session = await ResolveDesk();
            await engine.SendInputAsync(session, text);
            var id = $"ask:{Guid.NewGuid().ToString().ToUpperInvariant()}";
            Ask.Router.Sent(session);
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
            FocusDeltas();
        }
    }

    /// <summary>129 §3.2: this connection wants the desk's deltas only; re-applied on every <c>PendingPrompts</c>; pre-129 engines refuse, fine.</summary>
    private void FocusDeltas()
    {
        if (Ask.Router.DeskSession is not { } s) return;
        _ = FocusDeltasAsync(s);
    }

    private async Task FocusDeltasAsync(string session)
    {
        try
        {
            await engine.SetEventFocusAsync(new[] { session });
            if (AskProbe) log($"set_event_focus([{(session.Length > 8 ? session[..8] : session)}…]) ok");
        }
        catch (Exception e)
        {
            log($"set_event_focus failed: {Describe(e)}");
        }
    }

    /// <summary>Every event, raw: <c>TextDelta</c>/<c>TurnFinished</c>/<c>Error</c> on the desk feed the reply (a failed turn ends the
    /// receipt with "theuth couldn't reply — …", W6 item 7); <c>Dictation*</c> the live dictation.</summary>
    private void AskObserve(byte[] data)
    {
        StreamDelta d;
        try { d = StreamDelta.Decode(data); }
        catch (Exception) { return; }
        if (d.Type == "PendingPrompts")
        {
            FocusDeltas();
            return;
        }
        if (d.Type.StartsWith("Dictation", StringComparison.Ordinal))
        {
            DictationObserve(data);
            return;
        }
        if (Ask.Router.Reply is not { Finished: false } || d.Session is not { } session) return;
        ReplyEvent e;
        switch (d.Type)
        {
            case "TextDelta": e = new ReplyEvent.TextDelta(session, d.Text ?? ""); break;
            case "TurnFinished": e = new ReplyEvent.TurnFinished(session, d.StopReason); break;
            case "Error": e = new ReplyEvent.Error(session, d.Message ?? ""); break;
            default: return;
        }
        if (!Ask.Router.Observe(e) || Ask.Router.Reply is not { } reply) return;
        var text = reply.FirstTwoLines;
        if (Ask.Current is { } id) Ask.Receipts.Update(id, text);
        OnPropertyChanged(nameof(Ask));
        SetReceipt(new ReceiptRow.Reply("theuth", text.Length == 0 ? "…" : text, reply.Session));
        if (reply.Failed) log($"ask failed on the desk: {(d.Type == "Error" ? d.Message : $"stop_reason={d.StopReason}")}");
        if (reply.Finished) ExpireReceipt(12);
    }

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
            case "DictationText": take = Ask.Dictation.Handle(new AskDictation.Event.Text(id, w.Utterance ?? 0, w.Text ?? "", w.IsFinal ?? false)); break;
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
