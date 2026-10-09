using Theuth.Companion.Core;
using Theuth.Companion.Rpc;

namespace Theuth.Companion;

/// <summary>
/// 149 WW2's Store members (design 149 §3.1–3.5): the agents panel's content (<see cref="AgentsPanel"/>, re-derived on every
/// refetch from <c>list_activity</c> ∪ <c>agents_list</c> ∪ the sessions WW1's Store fetches), the remote-session set the
/// rail's dots read (hollow), the hovered dot, the card anatomy join, and the row/card verbs (D3) that go through the one hold:
/// ✕ → <c>cancel(session, "tree")</c> / <c>agent_pause</c>, the row mic's text → <c>send_input(session, text)</c>, X on a card.
/// UI thread only, like the rest of the Store.
/// </summary>
public sealed partial class Store
{
    private AgentsPanel agentsPanel = AgentsPanel.Empty;
    private IReadOnlySet<string> remoteSessions = new HashSet<string>(StringComparer.Ordinal);
    private string? hoveredSession;
    private bool ww2Installed;

    /// <summary>The agents panel's rows and host groups (Swift <c>AgentsModel</c>).</summary>
    public AgentsPanel AgentsPanel
    {
        get => agentsPanel;
        private set
        {
            agentsPanel = value;
            OnPropertyChanged();
        }
    }

    /// <summary>Sessions owned by another engine (E7 fallback): the rail draws them hollow.</summary>
    public IReadOnlySet<string> RemoteSessions
    {
        get => remoteSessions;
        private set
        {
            remoteSessions = value;
            OnPropertyChanged();
        }
    }

    /// <summary>The session whose rail dot is hovered (null = none): the agents panel highlights its row (§3.1).</summary>
    public string? HoveredSession
    {
        get => hoveredSession;
        set
        {
            if (hoveredSession == value) return;
            hoveredSession = value;
            OnPropertyChanged();
        }
    }

    /// <summary>WW1's <c>list_sessions</c> rows (<see cref="StoreModel.Sessions"/>) in the Core's row shape, refreshed with them.</summary>
    public IReadOnlyList<SessionRow> SessionRows { get; private set; } = Array.Empty<SessionRow>();

    /// <summary>Called once by the composition root: derive the agents panel on every refetch (WW1's <c>SessionsRefetched</c> follows each <c>Refetch</c>).</summary>
    public void InstallWw2()
    {
        if (ww2Installed) return;
        ww2Installed = true;
        Refetched += DeriveAgents;
        SessionsRefetched += DeriveAgents;
        DeriveAgents();
    }

    private static SessionRow Row(SessionSummary s) =>
        new(s.Id, s.Title, s.Owner, s.Running, s.Agent, s.Cwd, s.Project, s.Parent, s.Desk, s.Name, s.UpdatedAt, s.Archived);

    private void DeriveAgents()
    {
        SessionRows = Sessions.Select(Row).ToList();
        var mesh = Mesh;
        string selfId = SelfEngineId;
        string selfName = mesh?.SelfName is { Length: > 0 } n ? n : Environment.MachineName;
        string? meshName = null; // mesh_status carries no mesh name in the Core DTO: the group says "on 🌐 mesh"
        IReadOnlyList<HostPeer> peers = mesh?.Peers.Select(p => new HostPeer(p.Id, p.Name)).ToList() ?? new List<HostPeer>();
        AgentsPanel = AgentsModel.Derive(Activity, Agents, SessionRows, Items, selfId, selfName, peers, DateTimeOffset.Now, meshName);
        var remote = new HashSet<string>(StringComparer.Ordinal);
        foreach (var r in AgentsPanel.Rows)
        {
            if (r.Remote && r.Session is { } s) remote.Add(s);
        }
        if (!remote.SetEquals(remoteSessions)) RemoteSessions = remote;
    }

    public SessionRow? SessionRow(string? id) => id is null ? null : SessionRows.FirstOrDefault(s => s.Id == id);

    public AgentInfo? ActivityOf(string? session) => session is null ? null : Activity.FirstOrDefault(a => a.Id == session);

    /// <summary>E6 context (WW1 decodes <c>AttentionItem.context</c>) or the §6 interim join, through WW1's <see cref="StoreModel.ContextFor"/>.</summary>
    public ItemContextText? ContextText(AttentionItem item)
    {
        var r = ContextFor(item);
        return r.IsEmpty ? null : new ItemContextText(r.Request, r.LastReply);
    }

    /// <summary>The focused card's anatomy (§3.3): header position, status line, bubbles, options, reply row, footer verbs.</summary>
    public CardAnatomy? FocusedAnatomy
    {
        get
        {
            if (FocusedCard is not { } card) return null;
            var item = card.Item;
            var agent = ActivityOf(item.Session);
            bool gone = item.Session is not null && agent is null && Activity.Count > 0 && SessionRows.Count > 0 && SessionRow(item.Session) is null;
            return CardAnatomyModel.Make(card, Panel.Focus + 1, Panel.Items.Count, agent, SessionRow(item.Session), ContextText(item), DateTimeOffset.Now, gone);
        }
    }

    // MARK: the verbs (D3) — every one goes through the hold: 3 s, Esc undoes, then the call; a cancel is final.

    /// <summary>Agents-panel ✕ / card X: <c>cancel(session, "tree")</c> behind the hold, or <c>agent_pause</c> for a desk row.</summary>
    public void StopRow(AgentRow row)
    {
        if (!row.CanStop) return;
        if (row.DeskName is { } desk)
        {
            Hold(new Held("", desk, row.StopPill, new CardAction.PauseDesk(desk)));
            return;
        }
        if (row.Session is not { } s) return;
        Hold(new Held("", row.Name, row.StopPill, new CardAction.StopTree(s)));
    }

    /// <summary>X on a card (Revision 1 #2: only while the agent runs or waits).</summary>
    public void StopCardAgent(CardAnatomy a)
    {
        if (!a.Stops || a.Card.Item.Session is not { } s) return;
        int children = ActivityOf(s)?.Activity.Children ?? 0;
        Hold(new Held("", a.Card.Who, CardAnatomyModel.StopPill(a.Card.Who, children), new CardAction.StopTree(s)));
    }

    /// <summary>The row mic's text (ruling 3): <c>send_input(session, text)</c> after the hold; the receipt names the row.</summary>
    public void SendToRow(AgentRow row, string text)
    {
        var t = text.Trim();
        if (t.Length == 0 || row.Session is not { } s || !row.CanTalk) return;
        Hold(new Held("", row.Name, "sent · Esc undo", new CardAction.SendInputTo(s, t)));
    }

    /// <summary>The footer pill / key E · S on a card: E clears (finished / info only), S snoozes 30 m (never answers).</summary>
    public void FooterVerb(CardAnatomy a, FooterVerb verb)
    {
        switch (verb)
        {
            case Core.FooterVerb.Stop: StopCardAgent(a); break;
            case Core.FooterVerb.Clear: if (a.Clears) Perform(Intent.Clear); break;
            case Core.FooterVerb.Snooze: if (a.Snoozes) Perform(Intent.Snooze); break;
        }
    }
}
