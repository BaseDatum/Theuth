use super::*;

#[rpc(server, client)]
pub trait SessionsApi {
    /// Create an idle `owner`-scope root (50 §2.3; the caller authenticated
    /// as the owner) so per-session state can be set before the first
    /// message; `send_input` to it afterwards.
    #[method(name = "new_session")]
    async fn new_session(&self) -> RpcResult<SessionId>;
    /// Every session, newest first.
    #[method(name = "list_sessions")]
    async fn list_sessions(&self) -> RpcResult<Vec<SessionSummary>>;
    /// The subtree under `session` (children, grandchildren, …), pre-order,
    /// each with its live state (19 §2.1).
    #[method(name = "list_agents", with_extensions)]
    async fn list_agents(&self, session: SessionId) -> RpcResult<Vec<AgentInfo>>;
    /// Rendered history of one session plus whether a turn is in flight.
    /// `before`/`limit` page the entries (25 §7): those with `seq < before`,
    /// the last `limit` of them; both omitted = everything. `settled_only`
    /// (48 §5) reads through the idle session actor; while busy the reply
    /// has `settled: false` and `context_editable: false` with a reason.
    #[method(name = "load_session", with_extensions)]
    async fn load_session(
        &self,
        session: SessionId,
        before: Option<i64>,
        limit: Option<u32>,
        settled_only: Option<bool>,
    ) -> RpcResult<Loaded>;
    /// Model-usage summary of `session` as an accounting owner (51 §6.5):
    /// direct, descendants, total, children, coverage and a revision that
    /// `ModelUsageChanged` bumps. `Err` when the session owns no ledger.
    #[method(name = "session_cost", with_extensions)]
    async fn session_cost(&self, session: SessionId) -> RpcResult<SessionCost>;
    /// Model-call spend of the durable ledger (128 Rev 3): the last `days`
    /// local calendar days (clamped to 1..=31, oldest first), the month to
    /// date, and the window's top owners by root session. Re-read on
    /// `ModelUsageChanged`.
    #[method(name = "spend_report")]
    async fn spend_report(&self, days: u32) -> RpcResult<SpendReport>;
    /// 160 §3.2: the full-history cost cube from memory — `since` null, of
    /// another epoch or older than the change log → a full snapshot; else the
    /// hours changed since (empty when current). `CostCubeChanged` announces
    /// new revisions. The empty full cube until the engine's first build lands.
    #[method(name = "cost_cube")]
    async fn cost_cube(&self, since: Option<agent::CostCubeCursor>) -> RpcResult<agent::CostCube>;
    /// 160 §4: ledger rows newest first (native attempts and placed reported
    /// money), keyset-paged by `before`; Σ over all pages = the cube's total
    /// for the same filter.
    #[method(name = "cost_calls")]
    async fn cost_calls(&self, query: agent::CostCallsQuery) -> RpcResult<agent::CostCallsPage>;
    /// 160 §5: every provider's usage limits and theuth's budget caps, from
    /// the engine's in-memory snapshot (never a provider call).
    #[method(name = "usage_limits")]
    async fn usage_limits(&self) -> RpcResult<agent::limits::UsageLimits>;
    /// 160 §5: ask the limit pollers to refresh soon (`provider` or all); returns at once.
    #[method(name = "usage_limits_refresh")]
    async fn usage_limits_refresh(&self, provider: Option<String>) -> RpcResult<()>;
    /// Set a session's title by hand; `TitleChanged` follows on the stream.
    #[method(name = "rename_session", with_extensions)]
    async fn rename_session(&self, session: SessionId, title: String) -> RpcResult<()>;
    /// Pin or unpin a session; pinned sessions lead `list_sessions`.
    #[method(name = "pin_session", with_extensions)]
    async fn pin_session(&self, session: SessionId, pinned: bool) -> RpcResult<()>;
    /// Delete a session and its subagents for good; a `SessionDeleted` per removed session
    /// follows on the stream. Refused while any of them is mid-turn.
    #[method(name = "delete_session", with_extensions)]
    async fn delete_session(&self, session: SessionId) -> RpcResult<()>;
    /// Take `session` here (68 §4.2): `reason` is `user` (continue here, at
    /// once), `orphan` (a resumed checkout), `write` (owner unreachable ≥
    /// `sessions.takeover_secs`) or `moved` (181: the owner asked, see
    /// `session_move`). Returns the new owner/epoch;
    /// `SessionOwnerChanged` follows on the stream.
    #[method(name = "session_take", with_extensions)]
    async fn session_take(&self, session: SessionId, reason: String) -> RpcResult<SessionOwner>;
    /// This engine's replication summary (68 §10): id, sessions owned here
    /// / elsewhere, log head, last snapshot.
    #[method(name = "sessions_status")]
    async fn sessions_status(&self) -> RpcResult<SessionsStatus>;
    /// Rebind a private root to this connection within the reconnect window
    /// (51 §4.3): same `view`, the current `lease_secret`; rotates the secret.
    #[method(name = "resume_private", with_extensions)]
    async fn resume_private(
        &self,
        session: SessionId,
        view: String,
        lease_secret: String,
    ) -> RpcResult<SessionOpen>;
    /// Release a root this connection owns (51 §4.3): private → the tree
    /// closes now and `CloseResult` says how far cleanup got; durable →
    /// `revoked: false`, nothing to clean.
    #[method(name = "leave_session", with_extensions)]
    async fn leave_session(
        &self,
        session: SessionId,
        lease_secret: Option<String>,
    ) -> RpcResult<CloseResult>;
    /// Set (or clear with `None`) the project label of `session`'s root
    /// (51 §3); `SessionMetadataChanged` follows on the stream.
    #[method(name = "set_session_project", with_extensions)]
    async fn set_session_project(
        &self,
        session: SessionId,
        project: Option<String>,
    ) -> RpcResult<()>;
    /// Every styled project's icon and colour (51 §3 as-built); projects
    /// without a row use the client's neutral default.
    #[method(name = "list_project_styles")]
    async fn list_project_styles(&self) -> RpcResult<Vec<ProjectStyle>>;
    /// Set project `name`'s icon (lucide name) and colour (palette key), both
    /// allow-listed; both `None` clears the style. No event, like
    /// `pin_session`: clients refetch `list_project_styles`.
    #[method(name = "set_project_style")]
    async fn set_project_style(
        &self,
        name: String,
        icon: Option<String>,
        color: Option<String>,
    ) -> RpcResult<()>;
}

#[async_trait]
impl SessionsApiServer for Server {

    async fn new_session(&self) -> RpcResult<SessionId> {
        owner_root().await
    }

    async fn list_sessions(&self) -> RpcResult<Vec<SessionSummary>> {
        ask::<Agents, _>(agent::AGENT_ACTOR, ListSessions).await
    }

    async fn list_agents(&self, ext: &Extensions, session: SessionId) -> RpcResult<Vec<AgentInfo>> {
        self.authorize_fwd(ext, session).await?;
        if let Some(r) = forward::to_creator(ext, session, "list_agents", (session,)).await? {
            return Ok(r);
        }
        ask::<Agents, _>(agent::AGENT_ACTOR, ListAgents { session }).await
    }

    async fn load_session(
        &self,
        ext: &Extensions,
        session: SessionId,
        before: Option<i64>,
        limit: Option<u32>,
        settled_only: Option<bool>,
    ) -> RpcResult<Loaded> {
        self.authorize_fwd(ext, session).await?;
        if let Some(r) = forward::to_creator(
            ext,
            session,
            "load_session",
            (session, before, limit, settled_only),
        )
        .await?
        {
            return Ok(r);
        }
        ask::<Agents, _>(
            agent::AGENT_ACTOR,
            LoadSession {
                session,
                before,
                limit,
                settled_only: settled_only.unwrap_or(false),
            },
        )
        .await
    }

    async fn session_cost(&self, ext: &Extensions, session: SessionId) -> RpcResult<SessionCost> {
        self.authorize(ext, session).await?;
        ask::<Agents, _>(agent::AGENT_ACTOR, SessionCostQuery(session))
            .await?
            .ok_or_else(|| failed(format!("{session}: no accounting owner")))
    }

    async fn spend_report(&self, days: u32) -> RpcResult<SpendReport> {
        ask::<Agents, _>(agent::AGENT_ACTOR, SpendReportQuery(days)).await
    }

    async fn cost_cube(&self, since: Option<agent::CostCubeCursor>) -> RpcResult<agent::CostCube> {
        Ok(cost_service()?.cube(since.as_ref()))
    }

    async fn cost_calls(&self, query: agent::CostCallsQuery) -> RpcResult<agent::CostCallsPage> {
        cost_service()?
            .calls(&query)
            .await
            .map_err(|e| failed(format!("{e:#}")))
    }

    async fn usage_limits(&self) -> RpcResult<agent::limits::UsageLimits> {
        Ok(agent::limits::LimitsHub::global().snapshot())
    }

    async fn usage_limits_refresh(&self, provider: Option<String>) -> RpcResult<()> {
        agent::limits::LimitsHub::global().kick(provider.as_deref());
        Ok(())
    }

    async fn session_take(
        &self,
        ext: &Extensions,
        session: SessionId,
        reason: String,
    ) -> RpcResult<SessionOwner> {
        self.authorize(ext, session).await?;
        let reason = sessionsync::TakeReason::parse(&reason).ok_or_else(|| {
            ErrorObjectOwned::owned(
                jsonrpsee::types::error::INVALID_PARAMS_CODE,
                format!("reason {reason:?}: expected user | orphan | write | moved"),
                None::<()>,
            )
        })?;
        let (owner, epoch) = ask::<sessionsync::SessionSync, _>(
            sessionsync::SESSIONS_ACTOR,
            sessionsync::TakeSession {
                id: session,
                reason,
                // The explicit `reason` is the caller's intent (`user` resumes).
                by_user: false,
                // 181: `moved` counts only from the session's owner (the engine forwarding it).
                caller: ext.get::<forward::Forwarded>().map(|f| f.0.clone()),
            },
        )
        .await?;
        Ok(SessionOwner {
            owner,
            epoch,
            here: true,
        })
    }

    async fn sessions_status(&self) -> RpcResult<SessionsStatus> {
        let store = ask::<Agents, _>(agent::AGENT_ACTOR, agent::DurableStore).await?;
        let sessions = ask::<Agents, _>(agent::AGENT_ACTOR, ListSessions).await?;
        let here = sessions.iter().filter(|s| s.here).count();
        Ok(SessionsStatus {
            engine: store.engine().to_owned(),
            here: u32::try_from(here).unwrap_or(u32::MAX),
            elsewhere: u32::try_from(sessions.len() - here).unwrap_or(u32::MAX),
            log_seq: u64::try_from(store.log_seq()).unwrap_or_default(),
            // ponytail: worker L owns snapshots; surface its timestamp here once it records one.
            last_snapshot: None,
        })
    }

    async fn rename_session(
        &self,
        ext: &Extensions,
        session: SessionId,
        title: String,
    ) -> RpcResult<()> {
        self.authorize_fwd(ext, session).await?;
        if let Some(r) = forward::route(ext, session, "rename_session", (session, &title)).await? {
            return Ok(r);
        }
        ask::<Agents, _>(agent::AGENT_ACTOR, RenameSession { session, title }).await
    }

    async fn pin_session(
        &self,
        ext: &Extensions,
        session: SessionId,
        pinned: bool,
    ) -> RpcResult<()> {
        self.authorize_fwd(ext, session).await?;
        if let Some(r) = forward::route(ext, session, "pin_session", (session, pinned)).await? {
            return Ok(r);
        }
        ask::<Agents, _>(agent::AGENT_ACTOR, PinSession { session, pinned }).await
    }

    async fn delete_session(&self, ext: &Extensions, session: SessionId) -> RpcResult<()> {
        self.authorize_fwd(ext, session).await?;
        if let Some(r) = forward::route(ext, session, "delete_session", (session,)).await? {
            return Ok(r);
        }
        ask::<Agents, _>(agent::AGENT_ACTOR, DeleteSession { session }).await
    }

    async fn resume_private(
        &self,
        ext: &Extensions,
        session: SessionId,
        view: String,
        lease_secret: String,
    ) -> RpcResult<SessionOpen> {
        let peer = peer(ext)?;
        let open = ask::<Agents, _>(
            agent::AGENT_ACTOR,
            ResumePrivate {
                session,
                view,
                lease_secret,
                peer: (peer.id(), peer.closed_token()),
            },
        )
        .await?;
        self.track(&open, peer.id());
        Ok(open)
    }

    async fn leave_session(
        &self,
        ext: &Extensions,
        session: SessionId,
        lease_secret: Option<String>,
    ) -> RpcResult<CloseResult> {
        let peer = peer(ext)?.id();
        ask::<Agents, _>(
            agent::AGENT_ACTOR,
            LeaveSession {
                session,
                lease_secret,
                peer,
            },
        )
        .await
    }

    async fn set_session_project(
        &self,
        ext: &Extensions,
        session: SessionId,
        project: Option<String>,
    ) -> RpcResult<()> {
        self.authorize_fwd(ext, session).await?;
        if let Some(r) =
            forward::route(ext, session, "set_session_project", (session, &project)).await?
        {
            return Ok(r);
        }
        ask::<Agents, _>(agent::AGENT_ACTOR, SetSessionProject { session, project }).await
    }

    async fn list_project_styles(&self) -> RpcResult<Vec<ProjectStyle>> {
        ask::<Agents, _>(agent::AGENT_ACTOR, ListProjectStyles).await
    }

    async fn set_project_style(
        &self,
        name: String,
        icon: Option<String>,
        color: Option<String>,
    ) -> RpcResult<()> {
        ask::<Agents, _>(agent::AGENT_ACTOR, SetProjectStyle { name, icon, color }).await
    }
}

pub(super) trait SessionStateExt {
    fn track(&self, open: &SessionOpen, peer: u64);
    fn wait_for_row<'a>(&'a self, session: SessionId, engine: &'a str)-> impl std::future::Future<Output = ()> + Send + 'a;
}

impl SessionStateExt for Server {
    /// Record a freshly opened private tree as `peer`'s: every known member
    /// of its root moves to `peer` (a resume rebinds the whole tree), and
    /// the opened session and root join.
    fn track(&self, open: &SessionOpen, peer: u64) {
        if open.retention != Retention::Private {
            return;
        }
        let mut map = self.private.write();
        for v in map.values_mut() {
            if v.0 == open.root {
                v.1 = peer;
            }
        }
        map.insert(open.root, (open.root, peer));
        map.insert(open.session, (open.root, peer));
    }

    /// 183 §5.6: wait, at most [`CREATED_ROW_WAIT`], for a session `engine`
    /// just created to replicate here (§13 row 11: on timeout the caller
    /// answers anyway; calls for it are forwarded by id meanwhile). A row
    /// still missing after [`CREATED_ROW_LAG`] is a WARN: normal replication
    /// takes a few seconds and logs nothing.
    async fn wait_for_row(&self, session: SessionId, engine: &str) {
        let Ok(store) = ask::<Agents, _>(agent::AGENT_ACTOR, agent::DurableStore).await else {
            return;
        };
        if row_within(&store, session, CREATED_ROW_WAIT).await {
            return;
        }
        let engine = engine.to_owned();
        tokio::spawn(async move {
            if !row_within(&store, session, CREATED_ROW_LAG - CREATED_ROW_WAIT).await {
                tracing::warn!(
                    "sessions: new row {session} from {engine} not here after {}s; forwarding by id",
                    CREATED_ROW_LAG.as_secs()
                );
            }
        });
    }

}

/// `session`'s row is in `store` within `wait` (polled every 100 ms).
async fn row_within(
    store: &agent::store::Store,
    session: SessionId,
    wait: std::time::Duration,
) -> bool {
    let deadline = Instant::now() + wait;
    loop {
        if store.session(session).await.ok().flatten().is_some() {
            return true;
        }
        if Instant::now() >= deadline {
            return false;
        }
        tokio::time::sleep(std::time::Duration::from_millis(100)).await;
    }
}

/// 183 §5.6: how long `create_session{engine}` waits for the new row before it
/// answers (the chat works by id meanwhile; the wait only spares a lookup).
const CREATED_ROW_WAIT: std::time::Duration = std::time::Duration::from_secs(1);

/// 183 §5.6: a new row from another engine not here after this long is a WARN.
const CREATED_ROW_LAG: std::time::Duration = std::time::Duration::from_secs(6);
