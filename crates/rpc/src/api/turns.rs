use std::collections::HashSet;
use std::path::PathBuf;

use agent::rpc_forward as forward;
use agent::rpc_support::{owner_root, session_draining, RpcServerExt};
use agent::todo::TodoSnapshot;
use agent::{
    AgentInfo, Agents, Broadcast, BroadcastOutcome, BroadcastReceipt, BroadcastRequest,
    BroadcastTargets, Cancel, CancelScope, Compact, CompactStyle, ContextBreakdown,
    ContextItems, ContextRemoval, ContextSnapshot, ContextTarget, Delivery, EditTurn,
    ForkSession, GetTodos, ListActivity, ListQueued, MemoryMode, PeerDeliver, PeerEnvelope,
    PeerReceipt, QueuedInput, Redo, Regenerate, RemoveContext, SendInput, SessionContextBreakdown,
    SessionMemory, Steer, Undo, ViewLine, Withdraw,
};
use jsonrpsee::Extensions;
use jsonrpsee::core::{RpcResult, async_trait};
use jsonrpsee::proc_macros::rpc;
use jsonrpsee::types::ErrorObjectOwned;
use kameo::error::SendError;
use rpc_core::{Server, ASK_FAILED, UNAUTHORIZED, ask, lookup, peer, send};
use runtime::{InputId, SessionId};
use tools::Retention;

#[rpc(server, client)]
pub trait TurnsApi {
    /// Start a turn; `session: None` creates an `owner`-scope root first.
    /// `files` are image files (17 §2) attached to the message. `delivery`
    /// (default `steer`) says how it lands on a busy session (62 §0.3). `id`
    /// is the client's idempotency key: a retry with the same id routes once.
    /// Returns once queued.
    #[method(name = "send_input", with_extensions)]
    async fn send_input(
        &self,
        session: Option<SessionId>,
        text: String,
        files: Vec<PathBuf>,
        delivery: Option<Delivery>,
        id: Option<InputId>,
    ) -> RpcResult<SessionId>;
    /// Queue a user message after the current tool batch (or, with
    /// `delivery: next_turn`, after the turn); returns the id (minted when
    /// omitted). `files` are image files (17 §2).
    #[method(name = "steer", with_extensions)]
    async fn steer(
        &self,
        session: SessionId,
        text: String,
        id: Option<InputId>,
        files: Vec<PathBuf>,
        delivery: Option<Delivery>,
    ) -> RpcResult<InputId>;
    /// A peer message for `session` forwarded by the engine of its sender
    /// (69 P): the owner's intake. Mesh-internal — accepted only over
    /// `theuth/rpc/1` (the `Forwarded` marker), never from a client; the
    /// supervisor refuses when the session is not owned here (no take).
    /// Returns the owner's receipt (`accepted`); a refusal is the error the
    /// sender records as a `failed` receipt.
    #[method(name = "peer_deliver", with_extensions)]
    async fn peer_deliver(
        &self,
        session: SessionId,
        envelope: PeerEnvelope,
    ) -> RpcResult<PeerReceipt>;
    /// One message to many sessions (62 §0.1): the selector is resolved on
    /// the harness, each row is authorized like `send_input`, and the reply
    /// carries one outcome per session. `dry_run: true` previews the rows.
    #[method(name = "broadcast", with_extensions)]
    async fn broadcast(&self, req: BroadcastRequest) -> RpcResult<BroadcastReceipt>;
    /// Every session with a live actor or a running job plus every root, with
    /// [`AgentInfo::activity`]; the broadcast picker's source (62 §0.6).
    #[method(name = "list_activity", with_extensions)]
    async fn list_activity(&self) -> RpcResult<Vec<AgentInfo>>;
    /// Pull a queued message back out; returns its text.
    #[method(name = "withdraw", with_extensions)]
    async fn withdraw(&self, session: SessionId, id: InputId) -> RpcResult<String>;
    /// Messages waiting for the current tool batch to end.
    #[method(name = "list_queued", with_extensions)]
    async fn list_queued(&self, session: SessionId) -> RpcResult<Vec<QueuedInput>>;
    /// Abort the in-flight turn; `scope` `tree` also stops every running descendant (133 §4; default `root`).
    /// 190 (D5): `reason` is accepted and ignored: an interject or flush cancels a contact's
    /// turn like any other.
    #[method(name = "cancel", with_extensions)]
    async fn cancel(
        &self,
        session: SessionId,
        scope: Option<CancelScope>,
        reason: Option<agent::CancelReason>,
    ) -> RpcResult<()>;
    /// Fold the conversation now (design doc 07 §8.6 / 11 §4.2); returns the
    /// new epoch. `style: None` uses the configured default. Refused while a
    /// turn is running.
    #[method(name = "compact", with_extensions)]
    async fn compact(&self, session: SessionId, style: Option<CompactStyle>) -> RpcResult<u32>;
    /// Hide the last turn from the frame; returns its first line.
    #[method(name = "undo", with_extensions)]
    async fn undo(&self, session: SessionId) -> RpcResult<String>;
    /// Restore the last undone turn; returns its first line.
    #[method(name = "redo", with_extensions)]
    async fn redo(&self, session: SessionId) -> RpcResult<String>;
    /// Inventory of the active context (design doc 48 §3.2): revision,
    /// editability, and every removable message/binary/head with its
    /// protocol companions. No bodies. Native sessions only.
    #[method(name = "context_items", with_extensions)]
    async fn context_items(&self, session: SessionId) -> RpcResult<ContextSnapshot>;
    /// Where the session's frame tokens go (70 COMPACT-SIZE): head images and
    /// text, tail messages/tokens, system, the calibrated total estimate the
    /// gauge shows after a compaction, and the provider's last billed context.
    /// Live figures from an idle actor; the stored frame otherwise.
    #[method(name = "session_context_breakdown", with_extensions)]
    async fn session_context_breakdown(&self, session: SessionId) -> RpcResult<ContextBreakdown>;
    /// 137 §11.2: the session's memory mode, its view, settle state,
    /// compactor stats and backfill progress; `mode: epochs` + an empty view
    /// for an epochs session.
    #[method(name = "session_memory", with_extensions)]
    async fn session_memory(&self, session: SessionId) -> RpcResult<SessionMemory>;
    /// 137F: reset the backoff of every failed view part, re-resolve the
    /// compactor, wake the pump; the fresh `SessionMemory` (an epochs session's unchanged).
    #[method(name = "endless_retry", with_extensions)]
    async fn endless_retry(&self, session: SessionId) -> RpcResult<SessionMemory>;
    /// 137G: fold every unbuilt level-0 line mechanically (an excerpt, no model
    /// call, instant); the fresh `SessionMemory` (an epochs session's unchanged).
    #[method(name = "endless_fold", with_extensions)]
    async fn endless_fold(&self, session: SessionId) -> RpcResult<SessionMemory>;
    /// 137G: `paused = true` stops the session's compactor calls (in-flight ones
    /// cancelled, the backlog kept; persisted), `false` resumes; the fresh `SessionMemory`.
    #[method(name = "endless_pause", with_extensions)]
    async fn endless_pause(&self, session: SessionId, paused: bool) -> RpcResult<SessionMemory>;
    /// 137G: this chat's own compactor model (`None` = inherit the chain: agent
    /// default → `agent.endless.model` → title model → session model); the id
    /// must resolve; persisted, live; the fresh `SessionMemory`.
    #[method(name = "set_session_compactor_model", with_extensions)]
    async fn set_session_compactor_model(
        &self,
        session: SessionId,
        model: Option<String>,
    ) -> RpcResult<SessionMemory>;
    /// 137 §9 `zoom`: the two children of line `id+n` (`n == 1`: the whole message).
    #[method(name = "endless_zoom", with_extensions)]
    async fn endless_zoom(&self, session: SessionId, id: u64, n: u64) -> RpcResult<Vec<ViewLine>>;
    /// 137 §9 `recall`: FTS over the level-0 lines (raw fallback under 2 hits); `limit` default 8.
    #[method(name = "endless_recall", with_extensions)]
    async fn endless_recall(
        &self,
        session: SessionId,
        query: String,
        limit: Option<u8>,
    ) -> RpcResult<Vec<ViewLine>>;
    /// 137 8.6 `/endless on|off` over RPC; idle sessions only (error `busy`).
    #[method(name = "set_session_memory", with_extensions)]
    async fn set_session_memory(
        &self,
        session: SessionId,
        mode: MemoryMode,
    ) -> RpcResult<SessionMemory>;
    /// 137 R1-D2b: the session's view budget in bytes (16_000..=256_000);
    /// idle or running — it takes effect at the next turn boundary.
    #[method(name = "set_session_view_bytes", with_extensions)]
    async fn set_session_view_bytes(
        &self,
        session: SessionId,
        bytes: u32,
    ) -> RpcResult<SessionMemory>;
    /// Remove `targets` from the active context at `expected_revision`
    /// (design doc 48 §3.2). Atomic: stale revision, unknown targets or an
    /// incomplete protocol group refuse and write nothing. Refused while a
    /// turn is running; a `ContextChanged` follows on the stream.
    #[method(name = "remove_context", with_extensions)]
    async fn remove_context(
        &self,
        session: SessionId,
        expected_revision: String,
        targets: Vec<ContextTarget>,
    ) -> RpcResult<ContextRemoval>;
    /// Regenerate the reply to the turn opened at `turn` (a `HistoryEntry`
    /// `turn` handle): drop it and everything after, replay the stored opener
    /// (design doc 29 §3). Refused while a turn is running.
    #[method(name = "regenerate", with_extensions)]
    async fn regenerate(&self, session: SessionId, turn: i64) -> RpcResult<()>;
    /// Replace the turn opened at `turn` with `text` (+ `files`): drop it and
    /// everything after, start a turn with the new input (design doc 29 §3).
    /// Refused while a turn is running.
    #[method(name = "edit_turn", with_extensions)]
    async fn edit_turn(
        &self,
        session: SessionId,
        turn: i64,
        text: String,
        files: Vec<PathBuf>,
    ) -> RpcResult<()>;
    /// New session with a copy of the conversation through the turn opened at
    /// `through` (all of it when `None`); returns its id (design doc 29 §3).
    /// The source is untouched. Refused for external harness sessions.
    #[method(name = "fork_session", with_extensions)]
    async fn fork_session(
        &self,
        session: SessionId,
        through: Option<i64>,
        reply: Option<u32>,
    ) -> RpcResult<SessionId>;
    /// The session's current plan (47 §5.1): the latest committed snapshot,
    /// readable while a turn runs; `TodoChanged` carries later versions.
    /// An unknown session or a failed read is an error, never an empty list.
    #[method(name = "get_todos", with_extensions)]
    async fn get_todos(&self, session: SessionId) -> RpcResult<TodoSnapshot>;
    /// 109 §10.3: resume `session`'s open turn now (the `resume: ask` mode's
    /// Resume button). `false` when the session has no open turn.
    #[method(name = "resume_turn", with_extensions)]
    async fn resume_turn(&self, session: SessionId) -> RpcResult<bool>;
}

#[async_trait]
impl TurnsApiServer for Server {

    async fn send_input(
        &self,
        ext: &Extensions,
        session: Option<SessionId>,
        text: String,
        files: Vec<PathBuf>,
        delivery: Option<Delivery>,
        id: Option<InputId>,
    ) -> RpcResult<SessionId> {
        self.authorize_opt_fwd(ext, session).await?;
        let session = match session {
            Some(s) => s,
            None => owner_root().await?,
        };
        if let Some(r) = forward::route(
            ext,
            session,
            "send_input",
            (Some(session), &text, forward::ByName(&files), &delivery, id),
        )
        .await?
        {
            return Ok(r);
        }
        let files = forward::resolve_files(ext, &self.rt.data_dir, files).await?;
        ask::<Agents, _>(
            agent::AGENT_ACTOR,
            SendInput {
                session: Some(session),
                text,
                files,
                delivery: delivery.unwrap_or_default(),
                id,
                channel: None,
            },
        )
        .await
        .map_err(|e| session_draining(session, e))
    }

    async fn steer(
        &self,
        ext: &Extensions,
        session: SessionId,
        text: String,
        id: Option<InputId>,
        files: Vec<PathBuf>,
        delivery: Option<Delivery>,
    ) -> RpcResult<InputId> {
        self.authorize_fwd(ext, session).await?;
        let id = id.unwrap_or_default();
        if let Some(r) = forward::route(
            ext,
            session,
            "steer",
            (session, &text, Some(id), forward::ByName(&files), &delivery),
        )
        .await?
        {
            return Ok(r);
        }
        let files = forward::resolve_files(ext, &self.rt.data_dir, files).await?;
        ask::<Agents, _>(
            agent::AGENT_ACTOR,
            Steer {
                session,
                id,
                text,
                files,
                delivery: delivery.unwrap_or_default(),
                channel: None,
            },
        )
        .await
        .map_err(|e| session_draining(session, e))?;
        Ok(id)
    }

    async fn peer_deliver(
        &self,
        ext: &Extensions,
        session: SessionId,
        envelope: PeerEnvelope,
    ) -> RpcResult<PeerReceipt> {
        self.authorize(ext, session).await?;
        let Some(forward::Forwarded(peer)) = ext.get::<forward::Forwarded>() else {
            return Err(ErrorObjectOwned::owned(
                UNAUTHORIZED,
                "peer_deliver is mesh-internal",
                None::<()>,
            ));
        };
        let forwarder = Some(peer.0.clone());
        // 183 §6.5: a forwarder that does not own `envelope.from` is unauthorized.
        lookup::<Agents>(agent::AGENT_ACTOR)?
            .ask(PeerDeliver {
                session,
                envelope,
                forwarder,
            })
            .await
            .map_err(|e: SendError<PeerDeliver, _>| match e {
                SendError::HandlerError(agent::PeerDeliverError::NotOwner(r)) => {
                    ErrorObjectOwned::owned(UNAUTHORIZED, r.to_string(), None::<()>)
                }
                other => ErrorObjectOwned::owned(ASK_FAILED, other.to_string(), None::<()>),
            })
    }

    async fn broadcast(
        &self,
        ext: &Extensions,
        req: BroadcastRequest,
    ) -> RpcResult<BroadcastReceipt> {
        // The supervisor owns the selector, so authorization is a two-step:
        // resolve as a dry run, drop what this device may not see (51 §4.2),
        // then send to the permitted ids by explicit list. The refused rows
        // are spliced back in selector order with their title emptied.
        let mut preview = ask::<Agents, _>(
            agent::AGENT_ACTOR,
            Broadcast(BroadcastRequest {
                dry_run: true,
                ..req.clone()
            }),
        )
        .await?;
        // Unknown ids (`Failed`) have nothing to authorize and stay as reported.
        let mut permitted = Vec::with_capacity(preview.rows.len());
        for row in &mut preview.rows {
            if matches!(row.outcome, BroadcastOutcome::Failed { .. }) {
                continue;
            }
            if self.authorize(ext, row.session).await.is_ok() {
                permitted.push(row.session);
            } else {
                row.title.clear();
                row.outcome = BroadcastOutcome::Skipped {
                    reason: "not visible from this device".into(),
                };
            }
        }
        if req.dry_run {
            return Ok(preview);
        }
        let allowed: HashSet<SessionId> = permitted.iter().copied().collect();
        let mut sent = ask::<Agents, _>(
            agent::AGENT_ACTOR,
            Broadcast(BroadcastRequest {
                targets: BroadcastTargets::Sessions { ids: permitted },
                ..req
            }),
        )
        .await?;
        // `Sessions` keeps the given order, so the sent rows zip with the
        // permitted preview rows.
        let mut rows = std::mem::take(&mut sent.rows).into_iter();
        sent.rows = preview
            .rows
            .into_iter()
            .map(|row| {
                if allowed.contains(&row.session) {
                    rows.next().unwrap_or(row)
                } else {
                    row
                }
            })
            .collect();
        Ok(sent)
    }

    async fn list_activity(&self, _ext: &Extensions) -> RpcResult<Vec<AgentInfo>> {
        ask::<Agents, _>(agent::AGENT_ACTOR, ListActivity).await
    }

    async fn withdraw(
        &self,
        ext: &Extensions,
        session: SessionId,
        id: InputId,
    ) -> RpcResult<String> {
        self.authorize_fwd(ext, session).await?;
        if let Some(r) = forward::route(ext, session, "withdraw", (session, id)).await? {
            return Ok(r);
        }
        ask::<Agents, _>(agent::AGENT_ACTOR, Withdraw { session, id }).await
    }

    async fn list_queued(
        &self,
        ext: &Extensions,
        session: SessionId,
    ) -> RpcResult<Vec<QueuedInput>> {
        self.authorize(ext, session).await?;
        ask::<Agents, _>(agent::AGENT_ACTOR, ListQueued { session }).await
    }

    async fn cancel(
        &self,
        ext: &Extensions,
        session: SessionId,
        scope: Option<CancelScope>,
        reason: Option<agent::CancelReason>,
    ) -> RpcResult<()> {
        self.authorize_fwd(ext, session).await?;
        // 190 (D5): the reason is ignored, so a forwarded cancel is the plain two-parameter
        // call: an owner engine on 186 takes it as a Stop and never refuses it.
        let routed = forward::route(ext, session, "cancel", (session, &scope)).await?;
        if let Some(r) = routed {
            return Ok(r);
        }
        ask::<Agents, _>(
            agent::AGENT_ACTOR,
            Cancel {
                session,
                keep: None,
                scope: scope.unwrap_or_default(),
                reason: reason.unwrap_or_default(),
            },
        )
        .await
    }

    async fn compact(
        &self,
        ext: &Extensions,
        session: SessionId,
        style: Option<CompactStyle>,
    ) -> RpcResult<u32> {
        self.authorize_fwd(ext, session).await?;
        if let Some(r) = forward::route(ext, session, "compact", (session, &style)).await? {
            return Ok(r);
        }
        ask::<Agents, _>(
            agent::AGENT_ACTOR,
            Compact {
                session,
                style,
                head: None,
            },
        )
        .await
    }

    async fn undo(&self, ext: &Extensions, session: SessionId) -> RpcResult<String> {
        self.authorize_fwd(ext, session).await?;
        if let Some(r) = forward::route(ext, session, "undo", (session,)).await? {
            return Ok(r);
        }
        ask::<Agents, _>(agent::AGENT_ACTOR, Undo { session }).await
    }

    async fn redo(&self, ext: &Extensions, session: SessionId) -> RpcResult<String> {
        self.authorize_fwd(ext, session).await?;
        if let Some(r) = forward::route(ext, session, "redo", (session,)).await? {
            return Ok(r);
        }
        ask::<Agents, _>(agent::AGENT_ACTOR, Redo { session }).await
    }

    async fn context_items(
        &self,
        ext: &Extensions,
        session: SessionId,
    ) -> RpcResult<ContextSnapshot> {
        self.authorize(ext, session).await?;
        ask::<Agents, _>(agent::AGENT_ACTOR, ContextItems { session }).await
    }

    async fn session_context_breakdown(
        &self,
        ext: &Extensions,
        session: SessionId,
    ) -> RpcResult<ContextBreakdown> {
        self.authorize(ext, session).await?;
        ask::<Agents, _>(agent::AGENT_ACTOR, SessionContextBreakdown { session }).await
    }

    // 137 §11.2 (137-contract.md §3): forwarded to the owning engine like
    // `compact`, else asked of the supervisor.
    async fn session_memory(
        &self,
        ext: &Extensions,
        session: SessionId,
    ) -> RpcResult<SessionMemory> {
        self.authorize_fwd(ext, session).await?;
        if let Some(r) = forward::route(ext, session, "session_memory", (session,)).await? {
            return Ok(r);
        }
        ask::<Agents, _>(agent::AGENT_ACTOR, agent::SessionMemoryQuery { session }).await
    }

    async fn endless_retry(
        &self,
        ext: &Extensions,
        session: SessionId,
    ) -> RpcResult<SessionMemory> {
        self.authorize_fwd(ext, session).await?;
        if let Some(r) = forward::route(ext, session, "endless_retry", (session,)).await? {
            return Ok(r);
        }
        ask::<Agents, _>(agent::AGENT_ACTOR, agent::EndlessRetry { session }).await
    }

    async fn endless_fold(&self, ext: &Extensions, session: SessionId) -> RpcResult<SessionMemory> {
        self.authorize_fwd(ext, session).await?;
        if let Some(r) = forward::route(ext, session, "endless_fold", (session,)).await? {
            return Ok(r);
        }
        ask::<Agents, _>(agent::AGENT_ACTOR, agent::EndlessFold { session }).await
    }

    async fn endless_pause(
        &self,
        ext: &Extensions,
        session: SessionId,
        paused: bool,
    ) -> RpcResult<SessionMemory> {
        self.authorize_fwd(ext, session).await?;
        if let Some(r) = forward::route(ext, session, "endless_pause", (session, paused)).await? {
            return Ok(r);
        }
        ask::<Agents, _>(agent::AGENT_ACTOR, agent::EndlessPause { session, paused }).await
    }

    async fn set_session_compactor_model(
        &self,
        ext: &Extensions,
        session: SessionId,
        model: Option<String>,
    ) -> RpcResult<SessionMemory> {
        self.authorize_fwd(ext, session).await?;
        if let Some(r) = forward::route(
            ext,
            session,
            "set_session_compactor_model",
            (session, model.clone()),
        )
        .await?
        {
            return Ok(r);
        }
        ask::<Agents, _>(
            agent::AGENT_ACTOR,
            agent::SetSessionCompactorModel { session, model },
        )
        .await
    }

    async fn endless_zoom(
        &self,
        ext: &Extensions,
        session: SessionId,
        id: u64,
        n: u64,
    ) -> RpcResult<Vec<ViewLine>> {
        self.authorize_fwd(ext, session).await?;
        if let Some(r) = forward::route(ext, session, "endless_zoom", (session, id, n)).await? {
            return Ok(r);
        }
        ask::<Agents, _>(agent::AGENT_ACTOR, agent::EndlessZoom { session, id, n }).await
    }

    async fn endless_recall(
        &self,
        ext: &Extensions,
        session: SessionId,
        query: String,
        limit: Option<u8>,
    ) -> RpcResult<Vec<ViewLine>> {
        self.authorize_fwd(ext, session).await?;
        if let Some(r) =
            forward::route(ext, session, "endless_recall", (session, &query, limit)).await?
        {
            return Ok(r);
        }
        ask::<Agents, _>(
            agent::AGENT_ACTOR,
            agent::EndlessRecall {
                session,
                query,
                limit,
            },
        )
        .await
    }

    async fn set_session_memory(
        &self,
        ext: &Extensions,
        session: SessionId,
        mode: MemoryMode,
    ) -> RpcResult<SessionMemory> {
        self.authorize_fwd(ext, session).await?;
        if let Some(r) = forward::route(ext, session, "set_session_memory", (session, mode)).await?
        {
            return Ok(r);
        }
        ask::<Agents, _>(
            agent::AGENT_ACTOR,
            agent::SetSessionMemory {
                session,
                mode,
                filter: agent::UnitFilter::All,
            },
        )
        .await
        .map(|r| r.memory)
    }

    async fn set_session_view_bytes(
        &self,
        ext: &Extensions,
        session: SessionId,
        bytes: u32,
    ) -> RpcResult<SessionMemory> {
        self.authorize_fwd(ext, session).await?;
        if let Some(r) =
            forward::route(ext, session, "set_session_view_bytes", (session, bytes)).await?
        {
            return Ok(r);
        }
        ask::<Agents, _>(
            agent::AGENT_ACTOR,
            agent::SetSessionViewBytes { session, bytes },
        )
        .await
    }

    async fn remove_context(
        &self,
        ext: &Extensions,
        session: SessionId,
        expected_revision: String,
        targets: Vec<ContextTarget>,
    ) -> RpcResult<ContextRemoval> {
        self.authorize_fwd(ext, session).await?;
        if let Some(r) = forward::route(
            ext,
            session,
            "remove_context",
            (session, &expected_revision, &targets),
        )
        .await?
        {
            return Ok(r);
        }
        ask::<Agents, _>(
            agent::AGENT_ACTOR,
            RemoveContext {
                session,
                expected_revision,
                targets,
            },
        )
        .await
    }

    async fn regenerate(&self, ext: &Extensions, session: SessionId, turn: i64) -> RpcResult<()> {
        self.authorize_fwd(ext, session).await?;
        if let Some(r) = forward::route(ext, session, "regenerate", (session, turn)).await? {
            return Ok(r);
        }
        ask::<Agents, _>(agent::AGENT_ACTOR, Regenerate { session, turn }).await
    }

    async fn edit_turn(
        &self,
        ext: &Extensions,
        session: SessionId,
        turn: i64,
        text: String,
        files: Vec<PathBuf>,
    ) -> RpcResult<()> {
        self.authorize_fwd(ext, session).await?;
        if let Some(r) = forward::route(
            ext,
            session,
            "edit_turn",
            (session, turn, &text, forward::ByName(&files)),
        )
        .await?
        {
            return Ok(r);
        }
        let files = forward::resolve_files(ext, &self.rt.data_dir, files).await?;
        ask::<Agents, _>(
            agent::AGENT_ACTOR,
            EditTurn {
                session,
                turn,
                text,
                files,
            },
        )
        .await
    }

    async fn fork_session(
        &self,
        ext: &Extensions,
        session: SessionId,
        through: Option<i64>,
        reply: Option<u32>,
    ) -> RpcResult<SessionId> {
        let source = self.authorize(ext, session).await?;
        let forked = ask::<Agents, _>(
            agent::AGENT_ACTOR,
            ForkSession {
                session,
                through,
                reply,
            },
        )
        .await?;
        // A private fork is a new private root owned by the same connection
        // (51 §4.1): admit its events to this peer only.
        if source.retention == Retention::Private {
            let owner = peer(ext)?.id();
            self.private.write().insert(forked, (forked, owner));
        }
        Ok(forked)
    }

    async fn get_todos(&self, ext: &Extensions, session: SessionId) -> RpcResult<TodoSnapshot> {
        self.authorize_fwd(ext, session).await?;
        if let Some(r) = forward::to_creator(ext, session, "get_todos", (session,)).await? {
            return Ok(r);
        }
        ask::<Agents, _>(agent::AGENT_ACTOR, GetTodos { session }).await
    }

    async fn resume_turn(&self, ext: &Extensions, session: SessionId) -> RpcResult<bool> {
        self.authorize_fwd(ext, session).await?;
        // 139 §9.2 (D18): an action, so it runs on the owner (I1). 183 §4.1:
        // every session's press forwards to a connected owner (`-32010` when
        // it is not); here it resumes this engine's own open turn. Idempotent
        // against W2's own resume-on-take: a turn already in flight answers
        // "resumed lazily".
        if let Some(r) = forward::route(ext, session, "resume_turn", (session,)).await? {
            return Ok(r);
        }
        let agents = lookup::<Agents>(agent::AGENT_ACTOR)?;
        let Some(turn) = send(&agents, agent::supervisor::OpenTurn(session)).await? else {
            return Ok(false);
        };
        // `cause: None` = the boot rule from the journal (§6.3) on the owner.
        send(
            &agents,
            agent::supervisor::ResumeTurn {
                session,
                turn,
                cause: None,
                from: None,
            },
        )
        .await?;
        Ok(true)
    }
}

/// Build this domain's RPC module.
pub fn rpc_module(server: rpc_core::Server) -> jsonrpsee::RpcModule<rpc_core::Server> {
    TurnsApiServer::into_rpc(server)
}
