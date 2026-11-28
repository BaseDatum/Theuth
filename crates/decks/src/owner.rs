//! Single-owner Deck runtime supervisor & per-Deck actor loop.
//!
//! Invariants enforced here:
//! - Exactly one `DeckOwner` task owns at most one `DeckCheckout` per Deck.
//! - Stream steps (`on_step`/`collect`) and interactive actions (`on_action`) are serialized
//!   strictly at cooperative turn boundaries — Python never runs concurrently for a Deck.
//! - `run_epoch` increments on start, stop, and revision upgrade so stale frames/events are fenced.
//! - Streaming keeps the checkout and Python globals alive with zero UI views open.
//! - Snapshot failures preserve the durable last-good snapshot and mark the Deck `stale = true`.
//! - Tool approvals (`ApprovalRequested` / `ApprovalResolved`) for a Deck's `SessionId` are
//!   surfaced in real time on the per-Deck subscription hub without blocking the turn driver.

use std::collections::HashMap;
use std::path::PathBuf;
use std::str::FromStr;
use std::sync::Arc;
use std::time::Duration;

use agent::repl::MontyPool;
use chrono::Utc;
use document::deck::{
    CreateDeck, DeckCall, DeckCallReply, DeckCreated, DeckDocError, DeckDocSnapshot, DeckOp,
    DeckScriptRef, DeckSection, SectionKind, SectionRevision,
};
use document::{DocId, DocKind, DocumentActor, ListDocs, Principal};
use kameo::actor::ActorRef;
use kameo::message::{Context, Message};
use parking_lot::Mutex;
use runtime::{Runtime, SessionId};
use serde_json::{Value, json};
use tokio::sync::{mpsc, oneshot};
use tokio::time::Instant;
use tools::{ApprovalId, ApprovalRequested, ApprovalResolved, Decision, ToolRegistry};

use crate::hub::{DeckHub, MAX_SNAPSHOT_BYTES};
use crate::model::{
    DeckChanged, DeckDetail, DeckRuntimeEvent, DeckRuntimeStatus, DeckSnapshot, DeckSummary,
    DeclaredAction, InteractErrorCode, InteractResponse, LiveFrame, PendingApproval,
    RuntimeControlOp, RuntimeState, ScriptMode, ScriptRevision, ScriptStatus,
};
use crate::runtime::{DECK_POOL_MAX, DeckCheckout, EmittedPayload, TurnContext, TurnEffects};
use crate::store::{DeckRecord, DeckStore};

/// Actor registry name for [`DeckSupervisor`].
pub const DECKS_ACTOR: &str = "decks";

/// Default timeout for an interactive action call if not specified by caller.
const DEFAULT_INTERACT_TIMEOUT_MS: u64 = 15_000;
const MAX_INTERACT_TIMEOUT_MS: u64 = 60_000;

/// Shared state read by both the approval bus watcher and per-Deck owners.
#[derive(Clone)]
pub struct SharedDeckState {
    pub store: DeckStore,
    pub hub: Arc<DeckHub>,
    pub runtime: Runtime,
    pub document: ActorRef<DocumentActor>,
    pub registry: ActorRef<ToolRegistry>,
    pub cwd: PathBuf,
    pub pending_approvals: Arc<Mutex<HashMap<String, PendingApproval>>>,
    pub session_index: Arc<Mutex<HashMap<SessionId, String>>>,
    pub live_statuses: Arc<Mutex<HashMap<String, DeckRuntimeStatus>>>,
}

impl SharedDeckState {
    pub fn register_session(&self, deck_id: &str, session_id: SessionId) {
        self.session_index
            .lock()
            .insert(session_id, deck_id.to_owned());
    }

    pub fn deck_for_session(&self, session_id: SessionId) -> Option<String> {
        self.session_index.lock().get(&session_id).cloned()
    }

    pub fn on_approval_requested(&self, ev: &ApprovalRequested) {
        let Some(deck_id) = self.deck_for_session(ev.session) else {
            return;
        };
        let mut redacted_args = ev.args.clone();
        tools::secrets::redact_value(&mut redacted_args);
        let pending = PendingApproval {
            approval_id: ev.id,
            session_id: ev.session,
            tool: ev.tool.clone(),
            risk: ev.risk,
            args: redacted_args,
        };
        self.pending_approvals
            .lock()
            .insert(deck_id.clone(), pending.clone());
        self.hub.broadcast(
            &deck_id,
            DeckRuntimeEvent::ApprovalPending {
                approval: pending.clone(),
            },
        );
        let current = self.live_statuses.lock().get(&deck_id).cloned();
        if let Some(mut st) = current {
            st.state = RuntimeState::WaitingApproval;
            st.pending_approval = Some(pending);
            self.live_statuses
                .lock()
                .insert(deck_id.clone(), st.clone());
            self.hub
                .broadcast(&deck_id, DeckRuntimeEvent::Status { status: st });
        }
    }

    pub fn on_approval_resolved(&self, ev: &ApprovalResolved) {
        let deck_id = {
            let mut guard = self.pending_approvals.lock();
            let found = guard
                .iter()
                .find(|(_, p)| p.approval_id == ev.id)
                .map(|(k, _)| k.clone());
            if let Some(ref id) = found {
                guard.remove(id);
            }
            found
        };
        let Some(deck_id) = deck_id else {
            return;
        };
        let approved = ev.decision == Decision::Allow && !ev.timed_out && !ev.dismissed;
        self.hub.broadcast(
            &deck_id,
            DeckRuntimeEvent::ApprovalCleared {
                approval_id: ev.id,
                approved,
            },
        );
        let current = self.live_statuses.lock().get(&deck_id).cloned();
        if let Some(mut st) = current {
            st.pending_approval = None;
            if st.state == RuntimeState::WaitingApproval {
                st.state = if st.checkout_alive {
                    RuntimeState::Streaming
                } else {
                    RuntimeState::Stopped
                };
            }
            self.live_statuses
                .lock()
                .insert(deck_id.clone(), st.clone());
            self.hub
                .broadcast(&deck_id, DeckRuntimeEvent::Status { status: st });
        }
    }
}

/// Commands processed strictly sequentially by a single Deck's `DeckOwner` task.
enum OwnerCmd {
    Control {
        op: RuntimeControlOp,
        reply: oneshot::Sender<Result<DeckRuntimeStatus, String>>,
    },
    Interact {
        request_id: String,
        caller_id: String,
        action: String,
        args: Value,
        expected_rev: Option<u64>,
        reply: oneshot::Sender<InteractResponse>,
    },
    RevisionApproved {
        script: ScriptRevision,
        reply: oneshot::Sender<Result<DeckRuntimeStatus, String>>,
    },
    GetState {
        reply: oneshot::Sender<(DeckRuntimeStatus, Option<DeckSnapshot>, Option<LiveFrame>)>,
    },
    Shutdown,
}

/// Top-level Kameo actor supervising all Deck owners, document operations, scripts, and subscriptions.
pub struct DeckSupervisor {
    shared: SharedDeckState,
    pool: Option<Arc<MontyPool>>,
    owners: HashMap<String, mpsc::Sender<OwnerCmd>>,
}

impl kameo::Actor for DeckSupervisor {
    type Args = Self;
    type Error = anyhow::Error;

    async fn on_start(args: Self::Args, _actor_ref: ActorRef<Self>) -> Result<Self, Self::Error> {
        Ok(args)
    }

    async fn on_stop(
        &mut self,
        _actor_ref: kameo::actor::WeakActorRef<Self>,
        _reason: kameo::error::ActorStopReason,
    ) -> Result<(), Self::Error> {
        for (_, tx) in self.owners.drain() {
            let _ = tx.try_send(OwnerCmd::Shutdown);
        }
        Ok(())
    }
}

impl DeckSupervisor {
    pub fn new(
        store: DeckStore,
        runtime: Runtime,
        document: ActorRef<DocumentActor>,
        registry: ActorRef<ToolRegistry>,
        cwd: PathBuf,
        pool: Option<Arc<MontyPool>>,
    ) -> (Self, SharedDeckState) {
        let shared = SharedDeckState {
            store,
            hub: Arc::new(DeckHub::new()),
            runtime,
            document,
            registry,
            cwd,
            pending_approvals: Arc::new(Mutex::new(HashMap::new())),
            session_index: Arc::new(Mutex::new(HashMap::new())),
            live_statuses: Arc::new(Mutex::new(HashMap::new())),
        };
        let sup = Self {
            shared: shared.clone(),
            pool,
            owners: HashMap::new(),
        };
        (sup, shared)
    }

    async fn resolve_pool(&mut self) -> Result<Arc<MontyPool>, String> {
        if let Some(p) = &self.pool {
            return Ok(p.clone());
        }
        let arc = agent::repl::open_default_pool(DECK_POOL_MAX)
            .await
            .ok_or_else(|| "failed to initialize Deck Monty pool".to_owned())?;
        self.pool = Some(arc.clone());
        Ok(arc)
    }

    async fn ensure_owner(&mut self, deck_id: &str) -> Result<mpsc::Sender<OwnerCmd>, String> {
        if let Some(tx) = self.owners.get(deck_id)
            && !tx.is_closed()
        {
            return Ok(tx.clone());
        }
        let rec = self
            .shared
            .store
            .ensure_deck(deck_id)
            .await
            .map_err(|e| format!("{e:#}"))?;
        self.shared.register_session(deck_id, rec.session_id);
        let pool = self.resolve_pool().await?;
        let (tx, rx) = mpsc::channel(64);
        let owner = DeckOwner::load(deck_id.to_owned(), rec, self.shared.clone(), pool).await?;
        tokio::spawn(owner.run(rx));
        self.owners.insert(deck_id.to_owned(), tx.clone());
        Ok(tx)
    }

    async fn get_runtime_snapshot(
        &mut self,
        deck_id: &str,
    ) -> Result<(DeckRuntimeStatus, Option<DeckSnapshot>, Option<LiveFrame>), String> {
        if let Some(tx) = self.owners.get(deck_id)
            && !tx.is_closed()
        {
            let (rtx, rrx) = oneshot::channel();
            if tx.send(OwnerCmd::GetState { reply: rtx }).await.is_ok()
                && let Ok(res) = rrx.await
            {
                return Ok(res);
            }
        }
        // Cold read from SQLite without spinning up a Monty pool if not yet running.
        let rec = self
            .shared
            .store
            .ensure_deck(deck_id)
            .await
            .map_err(|e| format!("{e:#}"))?;
        self.shared.register_session(deck_id, rec.session_id);
        let log = self
            .shared
            .store
            .get_run_log(deck_id)
            .await
            .unwrap_or_default();
        let snap = self
            .shared
            .store
            .get_last_snapshot(deck_id)
            .await
            .unwrap_or(None);
        let pending_approval = self.shared.pending_approvals.lock().get(deck_id).cloned();
        let status = DeckRuntimeStatus {
            deck_id: deck_id.to_owned(),
            session_id: rec.session_id,
            state: if pending_approval.is_some() {
                RuntimeState::WaitingApproval
            } else {
                RuntimeState::Stopped
            },
            run_epoch: 0,
            active_rev: rec.active_rev,
            pending_rev: rec.pending_rev,
            stale: log.stale,
            last_ok_snapshot_id: log.last_ok_snapshot_id,
            last_ok_at: log.last_ok_at,
            last_error: log.last_error,
            last_error_at: log.last_error_at,
            consecutive_failures: log.consecutive_failures,
            step_count: 0,
            checkout_alive: false,
            pending_approval,
        };
        Ok((status, snap, None))
    }

    async fn mirror_script_ref(&self, deck_id: &str) {
        let Ok(Some(rec)) = self.shared.store.get_deck(deck_id).await else {
            return;
        };
        let active = match rec.active_rev {
            Some(r) => self
                .shared
                .store
                .get_script_revision(deck_id, r)
                .await
                .ok()
                .flatten(),
            None => None,
        };
        let schedule = active.as_ref().and_then(|s| {
            s.schedule_cron
                .clone()
                .or_else(|| s.interval_secs.map(|secs| format!("every {secs}s")))
        });
        let script_ref = DeckScriptRef {
            active_rev: rec.active_rev,
            pending_rev: rec.pending_rev,
            mode: active.as_ref().map(|s| s.mode.as_str().to_owned()),
            schedule,
            updated_at: Utc::now().timestamp_millis(),
        };
        let _ = self
            .shared
            .document
            .ask(DeckCall {
                doc: DocId(deck_id.to_owned()),
                by: Principal::Helper,
                op: DeckOp::UpdateScriptRef { script_ref },
            })
            .send()
            .await;
    }

    async fn read_doc(&self, deck_id: &str, by: Principal) -> Result<DeckDocSnapshot, String> {
        let reply = self
            .shared
            .document
            .ask(DeckCall {
                doc: DocId(deck_id.to_owned()),
                by,
                op: DeckOp::Read,
            })
            .send()
            .await
            .map_err(|e| format!("{e}"))?;
        match reply {
            DeckCallReply::Snapshot(s) => Ok(s),
            _ => Err("unexpected reply from document actor".into()),
        }
    }

    async fn publish_changed(&self, deck_id: &str, title: String, deleted: bool) {
        self.shared
            .runtime
            .publish(DeckChanged {
                deck_id: deck_id.to_owned(),
                title,
                updated_ms: Utc::now().timestamp_millis(),
                deleted,
            })
            .await;
    }
}

// ---------------------------------------------------------------------------
// Per-Deck single runtime owner task
// ---------------------------------------------------------------------------

struct DeckOwner {
    deck_id: String,
    session_id: SessionId,
    shared: SharedDeckState,
    pool: Arc<MontyPool>,
    checkout: Option<DeckCheckout>,
    active_script: Option<ScriptRevision>,
    pending_rev: Option<u64>,
    state: RuntimeState,
    run_epoch: u64,
    seq: u64,
    step_count: u64,
    last_snapshot: Option<DeckSnapshot>,
    live_frame: Option<LiveFrame>,
    last_state: Value,
    stale: bool,
    last_ok_snapshot_id: Option<String>,
    last_ok_at: Option<String>,
    last_error: Option<String>,
    last_error_at: Option<String>,
    consecutive_failures: u32,
    next_wake: Option<Instant>,
}

impl DeckOwner {
    async fn load(
        deck_id: String,
        rec: DeckRecord,
        shared: SharedDeckState,
        pool: Arc<MontyPool>,
    ) -> Result<Self, String> {
        let active_script = shared
            .store
            .get_active_script(&deck_id)
            .await
            .map_err(|e| format!("{e:#}"))?;
        let last_snapshot = shared
            .store
            .get_last_snapshot(&deck_id)
            .await
            .map_err(|e| format!("{e:#}"))?;
        let checkpoint = shared
            .store
            .get_checkpoint(&deck_id)
            .await
            .map_err(|e| format!("{e:#}"))?;
        let log = shared
            .store
            .get_run_log(&deck_id)
            .await
            .map_err(|e| format!("{e:#}"))?;

        let owner = Self {
            deck_id,
            session_id: rec.session_id,
            shared,
            pool,
            checkout: None,
            active_script,
            pending_rev: rec.pending_rev,
            state: RuntimeState::Stopped,
            run_epoch: 0,
            seq: last_snapshot.as_ref().map(|s| s.seq).unwrap_or(0),
            step_count: 0,
            last_snapshot,
            live_frame: None,
            last_state: checkpoint.map(|c| c.state).unwrap_or(Value::Null),
            stale: log.stale,
            last_ok_snapshot_id: log.last_ok_snapshot_id,
            last_ok_at: log.last_ok_at,
            last_error: log.last_error,
            last_error_at: log.last_error_at,
            consecutive_failures: log.consecutive_failures,
            next_wake: None,
        };
        owner.sync_live_status();
        Ok(owner)
    }

    fn status(&self) -> DeckRuntimeStatus {
        let pending_approval = self
            .shared
            .pending_approvals
            .lock()
            .get(&self.deck_id)
            .cloned();
        let state = if pending_approval.is_some() {
            RuntimeState::WaitingApproval
        } else {
            self.state
        };
        DeckRuntimeStatus {
            deck_id: self.deck_id.clone(),
            session_id: self.session_id,
            state,
            run_epoch: self.run_epoch,
            active_rev: self.active_script.as_ref().map(|s| s.rev),
            pending_rev: self.pending_rev,
            stale: self.stale,
            last_ok_snapshot_id: self.last_ok_snapshot_id.clone(),
            last_ok_at: self.last_ok_at.clone(),
            last_error: self.last_error.clone(),
            last_error_at: self.last_error_at.clone(),
            consecutive_failures: self.consecutive_failures,
            step_count: self.step_count,
            checkout_alive: self.checkout.as_ref().is_some_and(DeckCheckout::is_alive),
            pending_approval,
        }
    }

    fn sync_live_status(&self) {
        self.shared
            .live_statuses
            .lock()
            .insert(self.deck_id.clone(), self.status());
    }

    fn broadcast_status(&self) {
        let st = self.status();
        self.shared
            .live_statuses
            .lock()
            .insert(self.deck_id.clone(), st.clone());
        self.shared
            .hub
            .broadcast(&self.deck_id, DeckRuntimeEvent::Status { status: st });
    }

    fn turn_context(&self) -> TurnContext {
        TurnContext {
            last_state: self.last_state.clone(),
            last_snapshot: self
                .last_snapshot
                .as_ref()
                .map(|s| s.data.clone())
                .unwrap_or(Value::Null),
        }
    }

    async fn run(mut self, mut rx: mpsc::Receiver<OwnerCmd>) {
        loop {
            match self.next_wake {
                Some(deadline) => {
                    tokio::select! {
                        biased;
                        cmd = rx.recv() => {
                            let Some(cmd) = cmd else { break };
                            if self.handle_cmd(cmd).await {
                                break;
                            }
                        }
                        _ = tokio::time::sleep_until(deadline) => {
                            self.next_wake = None;
                            self.handle_tick().await;
                        }
                    }
                }
                None => {
                    let Some(cmd) = rx.recv().await else { break };
                    if self.handle_cmd(cmd).await {
                        break;
                    }
                }
            }
        }
        // Clean shutdown: checkpoint & release checkout.
        let _ = self.stop_checkout().await;
        self.shared.hub.broadcast(
            &self.deck_id,
            DeckRuntimeEvent::Closed {
                reason: "deck_owner_stopped".into(),
            },
        );
    }

    /// Returns `true` when the owner loop should exit.
    async fn handle_cmd(&mut self, cmd: OwnerCmd) -> bool {
        match cmd {
            OwnerCmd::Shutdown => {
                let _ = self.stop_checkout().await;
                return true;
            }
            OwnerCmd::GetState { reply } => {
                let _ = reply.send((
                    self.status(),
                    self.last_snapshot.clone(),
                    self.live_frame.clone(),
                ));
            }
            OwnerCmd::Control { op, reply } => {
                let res = self.handle_control(op).await;
                let _ = reply.send(res);
            }
            OwnerCmd::Interact {
                request_id,
                caller_id,
                action,
                args,
                expected_rev,
                reply,
            } => {
                if reply.is_closed() {
                    // Caller disconnected before execution began — do not replay mutation.
                    return false;
                }
                let resp = self
                    .handle_interact(request_id, caller_id, action, args, expected_rev)
                    .await;
                let _ = reply.send(resp);
            }
            OwnerCmd::RevisionApproved { script, reply } => {
                let res = self.handle_revision_approved(script).await;
                let _ = reply.send(res);
            }
        }
        false
    }

    async fn refresh_scripts_from_store(&mut self) {
        if let Ok(Some(rec)) = self.shared.store.get_deck(&self.deck_id).await {
            self.pending_rev = rec.pending_rev;
        }
        if let Ok(active) = self.shared.store.get_active_script(&self.deck_id).await {
            self.active_script = active;
        }
    }

    async fn handle_revision_approved(
        &mut self,
        script: ScriptRevision,
    ) -> Result<DeckRuntimeStatus, String> {
        let was_running = matches!(
            self.state,
            RuntimeState::Streaming | RuntimeState::ScheduledIdle
        );
        let was_paused = self.state == RuntimeState::Paused;
        self.active_script = Some(script);
        self.refresh_scripts_from_store().await;

        // Fence old revision & drop old checkout cleanly so new revision loads fresh with last_state.
        if self.checkout.is_some() {
            let _ = self.stop_checkout().await;
        }
        if was_running {
            return self.handle_control(RuntimeControlOp::Start).await;
        }
        if was_paused {
            self.state = RuntimeState::Stopped;
        }
        self.broadcast_status();
        Ok(self.status())
    }

    async fn handle_control(&mut self, op: RuntimeControlOp) -> Result<DeckRuntimeStatus, String> {
        self.refresh_scripts_from_store().await;
        match op {
            RuntimeControlOp::Start => {
                let Some(script) = self.active_script.clone() else {
                    return Err(
                        "no approved script revision for this Deck (pending revisions require owner approval before starting)".into(),
                    );
                };
                if script.status != ScriptStatus::Approved {
                    return Err("script revision is not approved".into());
                }
                if self.state == RuntimeState::Paused && self.checkout.is_some() {
                    return self.resume_stream(&script).await;
                }
                if matches!(
                    self.state,
                    RuntimeState::Streaming | RuntimeState::ScheduledIdle
                ) && self
                    .checkout
                    .as_ref()
                    .is_some_and(|c| c.script_rev() == script.rev && c.is_alive())
                {
                    return Ok(self.status());
                }

                self.run_epoch += 1;
                self.state = RuntimeState::Starting;
                self.broadcast_status();

                if let Err(err) = self.ensure_checkout(&script).await {
                    self.record_failure(&err).await;
                    self.state = RuntimeState::Error;
                    self.broadcast_status();
                    return Err(err);
                }

                let _ = self
                    .shared
                    .store
                    .set_stream_enabled(&self.deck_id, true)
                    .await;
                self.state = match script.mode {
                    ScriptMode::Oneshot => RuntimeState::ScheduledIdle,
                    ScriptMode::Streaming | ScriptMode::Hybrid => RuntimeState::Streaming,
                };
                // Execute first step right away so callers see fresh data immediately.
                self.next_wake = None;
                self.execute_one_step(&script).await;
                if self.next_wake.is_none()
                    && matches!(
                        self.state,
                        RuntimeState::Streaming | RuntimeState::ScheduledIdle
                    )
                {
                    self.schedule_next_wake(&script, None);
                }
                self.broadcast_status();
                Ok(self.status())
            }
            RuntimeControlOp::Pause => {
                // Retain live checkout & Python globals; only pause periodic stepping.
                self.next_wake = None;
                let _ = self
                    .shared
                    .store
                    .set_stream_enabled(&self.deck_id, false)
                    .await;
                if self.checkout.as_ref().is_some_and(DeckCheckout::is_alive) {
                    self.state = RuntimeState::Paused;
                } else {
                    self.state = RuntimeState::Stopped;
                }
                self.broadcast_status();
                Ok(self.status())
            }
            RuntimeControlOp::Resume => {
                let Some(script) = self.active_script.clone() else {
                    return Err("no approved script revision to resume".into());
                };
                self.resume_stream(&script).await
            }
            RuntimeControlOp::Stop => {
                self.next_wake = None;
                let _ = self
                    .shared
                    .store
                    .set_stream_enabled(&self.deck_id, false)
                    .await;
                self.stop_checkout().await?;
                self.run_epoch += 1;
                self.state = RuntimeState::Stopped;
                self.broadcast_status();
                Ok(self.status())
            }
            RuntimeControlOp::RunOnce => {
                let Some(script) = self.active_script.clone() else {
                    return Err(
                        "no approved script revision for this Deck (pending revisions require owner approval)".into(),
                    );
                };
                if script.status != ScriptStatus::Approved {
                    return Err("script revision is not approved".into());
                }
                let was_stopped = self.state == RuntimeState::Stopped;
                if was_stopped {
                    self.run_epoch += 1;
                }
                if let Err(err) = self.ensure_checkout(&script).await {
                    self.record_failure(&err).await;
                    self.broadcast_status();
                    return Err(err);
                }
                let step_err = self.execute_one_step(&script).await;
                if was_stopped && script.mode == ScriptMode::Oneshot {
                    let _ = self.stop_checkout().await;
                }
                self.broadcast_status();
                if let Some(err) = step_err {
                    return Err(err);
                }
                Ok(self.status())
            }
        }
    }

    async fn resume_stream(
        &mut self,
        script: &ScriptRevision,
    ) -> Result<DeckRuntimeStatus, String> {
        let _ = self
            .shared
            .store
            .set_stream_enabled(&self.deck_id, true)
            .await;
        if self
            .checkout
            .as_ref()
            .is_none_or(|c| !c.is_alive() || c.script_rev() != script.rev)
        {
            self.run_epoch += 1;
            if let Err(err) = self.ensure_checkout(script).await {
                self.record_failure(&err).await;
                self.state = RuntimeState::Error;
                self.broadcast_status();
                return Err(err);
            }
        }
        self.state = match script.mode {
            ScriptMode::Oneshot => RuntimeState::ScheduledIdle,
            ScriptMode::Streaming | ScriptMode::Hybrid => RuntimeState::Streaming,
        };
        // Wake immediately on next loop turn to advance the resumed checkout.
        self.next_wake = Some(Instant::now());
        self.broadcast_status();
        Ok(self.status())
    }

    async fn handle_tick(&mut self) {
        if !matches!(
            self.state,
            RuntimeState::Streaming | RuntimeState::ScheduledIdle
        ) {
            return;
        }
        self.refresh_scripts_from_store().await;
        let Some(script) = self.active_script.clone() else {
            self.state = RuntimeState::Stopped;
            self.broadcast_status();
            return;
        };
        if let Err(err) = self.ensure_checkout(&script).await {
            self.record_failure(&err).await;
            self.schedule_backoff();
            self.broadcast_status();
            return;
        }
        let step_err = self.execute_one_step(&script).await;
        if matches!(
            self.state,
            RuntimeState::Streaming | RuntimeState::ScheduledIdle
        ) {
            if step_err.is_some() {
                self.schedule_backoff();
            } else {
                // `execute_one_step` may have set `self.next_wake` from cooperative `deck_sleep`.
                if self.next_wake.is_none() {
                    self.schedule_next_wake(&script, None);
                }
            }
        }
        self.broadcast_status();
    }

    fn schedule_backoff(&mut self) {
        let shift = self.consecutive_failures.min(6);
        let secs = (1u64 << shift).clamp(2, 60);
        self.next_wake = Some(Instant::now() + Duration::from_secs(secs));
    }

    fn schedule_next_wake(&mut self, script: &ScriptRevision, sleep_override: Option<f64>) {
        if let Some(secs) = sleep_override {
            self.next_wake = Some(Instant::now() + Duration::from_secs_f64(secs));
            return;
        }
        if let Some(expr) = script.schedule_cron.as_deref()
            && let Ok(cron) = croner::Cron::from_str(expr)
            && let Ok(next_dt) = cron.find_next_occurrence(&Utc::now(), false)
        {
            let delta = (next_dt - Utc::now())
                .to_std()
                .unwrap_or(Duration::from_secs(60));
            self.next_wake = Some(Instant::now() + delta.max(Duration::from_secs(1)));
            return;
        }
        let interval = script
            .interval_secs
            .unwrap_or(match script.mode {
                ScriptMode::Streaming | ScriptMode::Hybrid => 3,
                ScriptMode::Oneshot => 60,
            })
            .max(1);
        self.next_wake = Some(Instant::now() + Duration::from_secs(interval));
    }

    async fn ensure_checkout(&mut self, script: &ScriptRevision) -> Result<(), String> {
        let compatible = self.checkout.as_ref().is_some_and(|c| {
            c.is_alive() && c.script_rev() == script.rev && c.code_sha256() == script.code_sha256
        });
        if compatible {
            if let Some(c) = self.checkout.as_mut() {
                match c.maybe_refresh_or_rollover().await {
                    Ok(_) => return Ok(()),
                    Err(e) => {
                        tracing::warn!(deck = %self.deck_id, "rollover failed ({e}); cold-restarting");
                        self.checkout = None;
                    }
                }
            }
        } else if self.checkout.is_some() {
            let _ = self.stop_checkout().await;
        }

        let cp = self
            .shared
            .store
            .get_checkpoint(&self.deck_id)
            .await
            .ok()
            .flatten();
        let turn_cx = self.turn_context();
        let dump_path = self.shared.store.monty_dump_path(self.session_id);
        let mut collected_emits = Vec::new();
        let (checkout, _mode, effects) = DeckCheckout::open_or_restore(
            self.pool.clone(),
            self.shared.registry.clone(),
            self.session_id,
            &self.shared.cwd,
            &self.shared.store.spill_dir(),
            dump_path,
            script,
            cp.as_ref(),
            &turn_cx,
            &mut |p| collected_emits.push(p.clone()),
        )
        .await?;
        self.checkout = Some(checkout);
        // Record initial dump compatibility stamp in SQLite.
        let _ = self
            .shared
            .store
            .save_checkpoint(
                &self.deck_id,
                script.rev,
                &script.code_sha256,
                effects.checkpoint_state.as_ref(),
                Some((script.rev, &script.code_sha256)),
            )
            .await;
        self.apply_turn_effects(script, effects, None).await;
        Ok(())
    }

    async fn stop_checkout(&mut self) -> Result<(), String> {
        let Some(checkout) = self.checkout.take() else {
            return Ok(());
        };
        let rev = checkout.script_rev();
        let sha = checkout.code_sha256().to_owned();
        let saved = checkout.stop_and_release().await.unwrap_or(false);
        if saved {
            let _ = self
                .shared
                .store
                .save_checkpoint(&self.deck_id, rev, &sha, None, Some((rev, &sha)))
                .await;
        }
        Ok(())
    }

    async fn execute_one_step(&mut self, script: &ScriptRevision) -> Option<String> {
        let turn_cx = self.turn_context();
        let Some(checkout) = self.checkout.as_mut() else {
            let err = "no active checkout".to_owned();
            self.record_failure(&err).await;
            return Some(err);
        };
        let res = checkout.run_step(&turn_cx, &mut |_| {}).await;
        match res {
            Ok(outcome) => {
                self.step_count += 1;
                let had_durable_emit = outcome.effects.emits.iter().any(|e| e.durable);
                let sleep_override = outcome.effects.next_sleep_secs;
                let fallback_return = if !had_durable_emit && !outcome.return_value.is_null() {
                    Some(outcome.return_value)
                } else {
                    None
                };
                self.apply_turn_effects(script, outcome.effects, fallback_return)
                    .await;
                if matches!(
                    self.state,
                    RuntimeState::Streaming | RuntimeState::ScheduledIdle
                ) {
                    self.schedule_next_wake(script, sleep_override);
                }
                // Keep dump compatibility stamp fresh after periodic heap checkpoint.
                let _ = self
                    .shared
                    .store
                    .save_checkpoint(
                        &self.deck_id,
                        script.rev,
                        &script.code_sha256,
                        None,
                        Some((script.rev, &script.code_sha256)),
                    )
                    .await;
                None
            }
            Err(err) => {
                self.record_failure(&err).await;
                Some(err)
            }
        }
    }

    async fn apply_turn_effects(
        &mut self,
        script: &ScriptRevision,
        effects: TurnEffects,
        fallback_snapshot: Option<Value>,
    ) {
        if let Some(state) = effects.checkpoint_state {
            self.last_state = state.clone();
            let _ = self
                .shared
                .store
                .save_checkpoint(
                    &self.deck_id,
                    script.rev,
                    &script.code_sha256,
                    Some(&state),
                    Some((script.rev, &script.code_sha256)),
                )
                .await;
        }
        for emit in effects.emits {
            self.handle_emit(script, emit).await;
        }
        if let Some(data) = fallback_snapshot {
            let len = serde_json::to_vec(&data)
                .map(|b| b.len())
                .unwrap_or(usize::MAX);
            if len <= MAX_SNAPSHOT_BYTES {
                self.handle_emit(
                    script,
                    EmittedPayload {
                        data,
                        durable: true,
                        section: None,
                    },
                )
                .await;
            }
        }
    }

    async fn handle_emit(&mut self, script: &ScriptRevision, emit: EmittedPayload) {
        if !self.shared.hub.admit_emit(&self.deck_id) {
            tracing::debug!(deck = %self.deck_id, "deck emit rate-limited");
            return;
        }
        self.seq += 1;
        let now = Utc::now().to_rfc3339();
        if emit.durable {
            let snap = DeckSnapshot {
                snapshot_id: format!("snap_{}", uuid::Uuid::now_v7().simple()),
                deck_id: self.deck_id.clone(),
                script_rev: script.rev,
                run_epoch: self.run_epoch,
                seq: self.seq,
                data: emit.data,
                section: emit.section,
                created_at: now.clone(),
            };
            if let Err(e) = self.shared.store.save_snapshot(&snap).await {
                tracing::warn!(deck = %self.deck_id, "save_snapshot failed: {e:#}");
            }
            self.stale = false;
            self.consecutive_failures = 0;
            self.last_error = None;
            self.last_ok_snapshot_id = Some(snap.snapshot_id.clone());
            self.last_ok_at = Some(now);
            self.last_snapshot = Some(snap.clone());
            self.shared
                .hub
                .broadcast(&self.deck_id, DeckRuntimeEvent::Snapshot { snapshot: snap });
        } else {
            let frame = LiveFrame {
                deck_id: self.deck_id.clone(),
                script_rev: script.rev,
                run_epoch: self.run_epoch,
                seq: self.seq,
                data: emit.data,
                section: emit.section,
                emitted_at: now,
            };
            self.live_frame = Some(frame.clone());
            self.shared
                .hub
                .broadcast(&self.deck_id, DeckRuntimeEvent::LiveFrame { frame });
        }
    }

    async fn record_failure(&mut self, error: &str) {
        let redacted = tools::secrets::redact(error).into_owned();
        if let Ok(log) = self
            .shared
            .store
            .record_failure(&self.deck_id, &redacted)
            .await
        {
            self.stale = log.stale;
            self.consecutive_failures = log.consecutive_failures;
            self.last_error = log.last_error;
            self.last_error_at = log.last_error_at;
        } else {
            self.stale = true;
            self.consecutive_failures += 1;
            self.last_error = Some(redacted);
            self.last_error_at = Some(Utc::now().to_rfc3339());
        }
    }

    async fn handle_interact(
        &mut self,
        request_id: String,
        caller_id: String,
        action: String,
        args: Value,
        expected_rev: Option<u64>,
    ) -> InteractResponse {
        self.refresh_scripts_from_store().await;

        // 1. Idempotency check BEFORE any validation/execution so duplicate request_ids never replay.
        if let Ok(Some(cached)) = self
            .shared
            .store
            .get_idempotent_action(&self.deck_id, &request_id)
            .await
        {
            return cached;
        }

        let err_resp =
            |code: InteractErrorCode, msg: String, rev: u64, epoch: u64| InteractResponse {
                request_id: request_id.clone(),
                action: action.clone(),
                ok: false,
                result: None,
                error: Some(msg),
                error_code: Some(code),
                private: true,
                cached: false,
                run_epoch: epoch,
                script_rev: rev,
            };

        // 2. Approved script required.
        let Some(script) = self.active_script.clone() else {
            return err_resp(
                InteractErrorCode::NoApprovedScript,
                "no approved script revision for this Deck".into(),
                0,
                self.run_epoch,
            );
        };
        if script.status != ScriptStatus::Approved {
            return err_resp(
                InteractErrorCode::NoApprovedScript,
                "active script is not approved".into(),
                script.rev,
                self.run_epoch,
            );
        }

        // 3. Revision fencing.
        if let Some(exp) = expected_rev
            && exp != script.rev
        {
            return err_resp(
                InteractErrorCode::RevisionMismatch,
                format!("expected script rev {exp}, active rev is {}", script.rev),
                script.rev,
                self.run_epoch,
            );
        }

        // 4. Approval pause check.
        if self
            .shared
            .pending_approvals
            .lock()
            .contains_key(&self.deck_id)
        {
            return err_resp(
                InteractErrorCode::WaitingApproval,
                "Deck is waiting for a pending tool approval".into(),
                script.rev,
                self.run_epoch,
            );
        }

        // 5. Declared action lookup.
        let Some(decl) = script.declared_actions.iter().find(|a| a.name == action) else {
            return err_resp(
                InteractErrorCode::UndeclaredAction,
                format!(
                    "action {action:?} is not declared by script rev {}",
                    script.rev
                ),
                script.rev,
                self.run_epoch,
            );
        };

        // 6. Paused guard if action opts out.
        if self.state == RuntimeState::Paused && !decl.allow_while_paused {
            return err_resp(
                InteractErrorCode::Paused,
                format!("action {action:?} cannot run while stream is paused"),
                script.rev,
                self.run_epoch,
            );
        }

        // 7. JSON Schema validation of `args`.
        if let Err(reason) = validate_action_args(decl, &args) {
            return err_resp(
                InteractErrorCode::InvalidArgs,
                reason,
                script.rev,
                self.run_epoch,
            );
        }

        // 8. Ensure checkout is open on the SAME Deck checkout (never a second concurrent checkout!).
        if self.run_epoch == 0 {
            self.run_epoch = 1;
        }
        if let Err(err) = self.ensure_checkout(&script).await {
            let resp = err_resp(
                InteractErrorCode::ExecutionError,
                err,
                script.rev,
                self.run_epoch,
            );
            return resp;
        }

        // 9. Execute `on_action(action, args, ctx)` on the SAME checkout.
        let turn_cx = self.turn_context();
        let ctx_val = json!({
            "deck_id": self.deck_id,
            "request_id": request_id,
            "caller_id": caller_id,
            "script_rev": script.rev,
            "run_epoch": self.run_epoch,
            "state": self.state,
        });
        let outcome = match self.checkout.as_mut() {
            Some(c) => {
                c.run_action(&action, &args, &ctx_val, &turn_cx, &mut |_| {})
                    .await
            }
            None => Err("checkout missing".into()),
        };

        let resp = match outcome {
            Ok(out) => {
                // Only explicit `deck_emit(...)` calls inside Python broadcast to shared subscribers!
                // The return value is strictly private to this caller.
                self.apply_turn_effects(&script, out.effects, None).await;
                if decl.mutates_state
                    && let Some(c) = self.checkout.as_mut()
                    && c.checkpoint_heap().await.is_ok()
                {
                    let _ = self
                        .shared
                        .store
                        .save_checkpoint(
                            &self.deck_id,
                            script.rev,
                            &script.code_sha256,
                            None,
                            Some((script.rev, &script.code_sha256)),
                        )
                        .await;
                }
                InteractResponse {
                    request_id: request_id.clone(),
                    action: action.clone(),
                    ok: true,
                    result: Some(out.return_value),
                    error: None,
                    error_code: None,
                    private: true,
                    cached: false,
                    run_epoch: self.run_epoch,
                    script_rev: script.rev,
                }
            }
            Err(err) => err_resp(
                InteractErrorCode::ExecutionError,
                err,
                script.rev,
                self.run_epoch,
            ),
        };

        let _ = self
            .shared
            .store
            .save_idempotent_action(&self.deck_id, &caller_id, &resp)
            .await;
        self.broadcast_status();
        resp
    }
}

fn validate_action_args(decl: &DeclaredAction, args: &Value) -> Result<(), String> {
    let validator = jsonschema::validator_for(&decl.schema)
        .map_err(|e| format!("invalid action schema for {:?}: {e}", decl.name))?;
    if let Err(err) = validator.validate(args) {
        return Err(format!("invalid args for {:?}: {err}", decl.name));
    }
    Ok(())
}

// ---------------------------------------------------------------------------
// Supervisor Actor Messages
// ---------------------------------------------------------------------------

#[derive(Clone, Debug)]
pub struct ListDecks {
    pub limit: Option<u32>,
}

impl Message<ListDecks> for DeckSupervisor {
    type Reply = Result<Vec<DeckSummary>, String>;

    async fn handle(&mut self, m: ListDecks, _: &mut Context<Self, Self::Reply>) -> Self::Reply {
        let docs = self
            .shared
            .document
            .ask(ListDocs {
                kind: Some(DocKind::Deck),
                limit: Some(m.limit.unwrap_or(100)),
            })
            .send()
            .await
            .map_err(|e| format!("{e}"))?;

        let mut out = Vec::with_capacity(docs.len());
        for d in docs {
            let id = d.id.as_str().to_owned();
            let (status, _, _) = self.get_runtime_snapshot(&id).await?;
            let doc_snap = self.read_doc(&id, Principal::Helper).await.ok();
            out.push(DeckSummary {
                id,
                title: d.title,
                description: doc_snap
                    .as_ref()
                    .map(|s| s.description.clone())
                    .unwrap_or_default(),
                section_count: doc_snap.as_ref().map(|s| s.sections.len()).unwrap_or(0),
                session_id: status.session_id,
                state: status.state,
                stale: status.stale,
                active_rev: status.active_rev,
                pending_rev: status.pending_rev,
                updated_ms: d.updated_ms,
            });
        }
        Ok(out)
    }
}

#[derive(Clone, Debug)]
pub struct CreateDeckMsg {
    pub title: Option<String>,
    pub description: Option<String>,
    pub example: bool,
    pub by: Principal,
}

impl Message<CreateDeckMsg> for DeckSupervisor {
    type Reply = Result<DeckDetail, String>;

    async fn handle(
        &mut self,
        m: CreateDeckMsg,
        _: &mut Context<Self, Self::Reply>,
    ) -> Self::Reply {
        let detail = if m.example {
            crate::example::create_workspace_pulse_example(self, m.by).await?
        } else {
            let created: DeckCreated = self
                .shared
                .document
                .ask(CreateDeck {
                    title: m.title,
                    description: m.description,
                    by: m.by.clone(),
                })
                .send()
                .await
                .map_err(|e| format!("{e}"))?;
            let deck_id = created.doc.as_str().to_owned();
            let rec = self
                .shared
                .store
                .ensure_deck(&deck_id)
                .await
                .map_err(|e| format!("{e:#}"))?;
            self.shared.register_session(&deck_id, rec.session_id);
            self.publish_changed(&deck_id, created.title, false).await;
            self.build_detail(&deck_id, m.by).await?
        };
        Ok(detail)
    }
}

#[derive(Clone, Debug)]
pub struct GetDeck {
    pub deck_id: String,
    pub by: Principal,
}

impl Message<GetDeck> for DeckSupervisor {
    type Reply = Result<DeckDetail, String>;

    async fn handle(&mut self, m: GetDeck, _: &mut Context<Self, Self::Reply>) -> Self::Reply {
        self.build_detail(&m.deck_id, m.by).await
    }
}

impl DeckSupervisor {
    pub(crate) async fn build_detail(
        &mut self,
        deck_id: &str,
        by: Principal,
    ) -> Result<DeckDetail, String> {
        let doc = self.read_doc(deck_id, by).await?;
        let (runtime, last_snapshot, live_frame) = self.get_runtime_snapshot(deck_id).await?;
        let active_script = self
            .shared
            .store
            .get_active_script(deck_id)
            .await
            .map_err(|e| format!("{e:#}"))?;
        let pending_script = self
            .shared
            .store
            .get_pending_script(deck_id)
            .await
            .map_err(|e| format!("{e:#}"))?;
        Ok(DeckDetail {
            doc,
            runtime,
            active_script,
            pending_script,
            last_snapshot,
            live_frame,
        })
    }

    pub(crate) fn shared(&self) -> &SharedDeckState {
        &self.shared
    }

    pub(crate) async fn doc_op(
        &self,
        deck_id: &str,
        by: Principal,
        op: DeckOp,
    ) -> Result<DeckCallReply, DeckDocError> {
        self.shared
            .document
            .ask(DeckCall {
                doc: DocId(deck_id.to_owned()),
                by,
                op,
            })
            .send()
            .await
            .map_err(|e| match e {
                kameo::error::SendError::HandlerError(doc_err) => doc_err,
                other => DeckDocError::Internal(format!("{other}")),
            })
    }
}

#[derive(Clone, Debug)]
pub struct SectionAddMsg {
    pub deck_id: String,
    pub kind: SectionKind,
    pub title: String,
    pub content: String,
    pub col_span: Option<u32>,
    pub data_key: Option<String>,
    pub index: Option<usize>,
    pub by: Principal,
}

impl Message<SectionAddMsg> for DeckSupervisor {
    type Reply = Result<DeckSection, DeckDocError>;

    async fn handle(
        &mut self,
        m: SectionAddMsg,
        _: &mut Context<Self, Self::Reply>,
    ) -> Self::Reply {
        match self
            .doc_op(
                &m.deck_id,
                m.by,
                DeckOp::SectionAdd {
                    kind: m.kind,
                    title: m.title,
                    content: m.content,
                    col_span: m.col_span,
                    data_key: m.data_key,
                    index: m.index,
                },
            )
            .await?
        {
            DeckCallReply::Section(s) => Ok(s),
            _ => Err(DeckDocError::Internal("unexpected reply".into())),
        }
    }
}

#[derive(Clone, Debug)]
pub struct SectionPatchMsg {
    pub deck_id: String,
    pub section_id: String,
    pub expected_rev: Option<u64>,
    pub title: Option<String>,
    pub content: Option<String>,
    pub col_span: Option<u32>,
    pub data_key: Option<Option<String>>,
    pub summary: Option<String>,
    pub by: Principal,
}

impl Message<SectionPatchMsg> for DeckSupervisor {
    type Reply = Result<DeckSection, DeckDocError>;

    async fn handle(
        &mut self,
        m: SectionPatchMsg,
        _: &mut Context<Self, Self::Reply>,
    ) -> Self::Reply {
        match self
            .doc_op(
                &m.deck_id,
                m.by,
                DeckOp::SectionPatch {
                    section_id: m.section_id,
                    expected_rev: m.expected_rev,
                    title: m.title,
                    content: m.content,
                    col_span: m.col_span,
                    data_key: m.data_key,
                    summary: m.summary,
                },
            )
            .await?
        {
            DeckCallReply::Section(s) => Ok(s),
            _ => Err(DeckDocError::Internal("unexpected reply".into())),
        }
    }
}

#[derive(Clone, Debug)]
pub struct SectionMoveMsg {
    pub deck_id: String,
    pub section_id: String,
    pub to_index: usize,
    pub by: Principal,
}

impl Message<SectionMoveMsg> for DeckSupervisor {
    type Reply = Result<Vec<String>, DeckDocError>;

    async fn handle(
        &mut self,
        m: SectionMoveMsg,
        _: &mut Context<Self, Self::Reply>,
    ) -> Self::Reply {
        match self
            .doc_op(
                &m.deck_id,
                m.by,
                DeckOp::SectionMove {
                    section_id: m.section_id,
                    to_index: m.to_index,
                },
            )
            .await?
        {
            DeckCallReply::Order(o) => Ok(o),
            _ => Err(DeckDocError::Internal("unexpected reply".into())),
        }
    }
}

#[derive(Clone, Debug)]
pub struct SectionArchiveMsg {
    pub deck_id: String,
    pub section_id: String,
    pub archived: bool,
    pub expected_rev: Option<u64>,
    pub by: Principal,
}

impl Message<SectionArchiveMsg> for DeckSupervisor {
    type Reply = Result<DeckSection, DeckDocError>;

    async fn handle(
        &mut self,
        m: SectionArchiveMsg,
        _: &mut Context<Self, Self::Reply>,
    ) -> Self::Reply {
        match self
            .doc_op(
                &m.deck_id,
                m.by,
                DeckOp::SectionArchive {
                    section_id: m.section_id,
                    archived: m.archived,
                    expected_rev: m.expected_rev,
                },
            )
            .await?
        {
            DeckCallReply::Section(s) => Ok(s),
            _ => Err(DeckDocError::Internal("unexpected reply".into())),
        }
    }
}

#[derive(Clone, Debug)]
pub struct SectionHistoryMsg {
    pub deck_id: String,
    pub section_id: String,
    pub by: Principal,
}

impl Message<SectionHistoryMsg> for DeckSupervisor {
    type Reply = Result<Vec<SectionRevision>, DeckDocError>;

    async fn handle(
        &mut self,
        m: SectionHistoryMsg,
        _: &mut Context<Self, Self::Reply>,
    ) -> Self::Reply {
        match self
            .doc_op(
                &m.deck_id,
                m.by,
                DeckOp::SectionHistory {
                    section_id: m.section_id,
                },
            )
            .await?
        {
            DeckCallReply::History(h) => Ok(h),
            _ => Err(DeckDocError::Internal("unexpected reply".into())),
        }
    }
}

#[derive(Clone, Debug)]
pub struct FocusSectionMsg {
    pub deck_id: String,
    pub section_id: Option<String>,
    pub by: Principal,
}

impl Message<FocusSectionMsg> for DeckSupervisor {
    type Reply = Result<(), DeckDocError>;

    async fn handle(
        &mut self,
        m: FocusSectionMsg,
        _: &mut Context<Self, Self::Reply>,
    ) -> Self::Reply {
        self.doc_op(
            &m.deck_id,
            m.by,
            DeckOp::Focus {
                section_id: m.section_id,
            },
        )
        .await?;
        Ok(())
    }
}

#[derive(Clone, Debug)]
pub struct ProposeScriptMsg {
    pub deck_id: String,
    pub code: String,
    pub declared_actions: Vec<DeclaredAction>,
    pub mode: ScriptMode,
    pub interval_secs: Option<u64>,
    pub schedule_cron: Option<String>,
    pub proposed_by: String,
    pub change_summary: String,
}

impl Message<ProposeScriptMsg> for DeckSupervisor {
    type Reply = Result<ScriptRevision, String>;

    async fn handle(
        &mut self,
        m: ProposeScriptMsg,
        _: &mut Context<Self, Self::Reply>,
    ) -> Self::Reply {
        if let Some(expr) = m.schedule_cron.as_deref() {
            croner::Cron::from_str(expr)
                .map_err(|e| format!("invalid cron expression {expr:?}: {e}"))?;
        }
        // Validate declared action JSON schemas up front.
        for a in &m.declared_actions {
            if a.name.trim().is_empty() {
                return Err("declared action name cannot be empty".into());
            }
            jsonschema::validator_for(&a.schema)
                .map_err(|e| format!("invalid JSON schema for action {:?}: {e}", a.name))?;
        }
        let rev = self
            .shared
            .store
            .propose_script(
                &m.deck_id,
                m.code,
                m.declared_actions,
                m.mode,
                m.interval_secs,
                m.schedule_cron,
                m.proposed_by,
                m.change_summary,
            )
            .await
            .map_err(|e| format!("{e:#}"))?;
        self.mirror_script_ref(&m.deck_id).await;
        if let Ok((st, _, _)) = self.get_runtime_snapshot(&m.deck_id).await {
            self.shared
                .hub
                .broadcast(&m.deck_id, DeckRuntimeEvent::Status { status: st });
        }
        Ok(rev)
    }
}

#[derive(Clone, Debug)]
pub struct ApproveScriptMsg {
    pub deck_id: String,
    pub rev: u64,
    pub approved_by: String,
}

impl Message<ApproveScriptMsg> for DeckSupervisor {
    type Reply = Result<ScriptRevision, String>;

    async fn handle(
        &mut self,
        m: ApproveScriptMsg,
        _: &mut Context<Self, Self::Reply>,
    ) -> Self::Reply {
        let script = self
            .shared
            .store
            .approve_script(&m.deck_id, m.rev, &m.approved_by)
            .await
            .map_err(|e| format!("{e:#}"))?;
        self.mirror_script_ref(&m.deck_id).await;
        if let Some(tx) = self.owners.get(&m.deck_id)
            && !tx.is_closed()
        {
            let (rtx, rrx) = oneshot::channel();
            let _ = tx
                .send(OwnerCmd::RevisionApproved {
                    script: script.clone(),
                    reply: rtx,
                })
                .await;
            let _ = rrx.await;
        } else if let Ok((st, _, _)) = self.get_runtime_snapshot(&m.deck_id).await {
            self.shared
                .hub
                .broadcast(&m.deck_id, DeckRuntimeEvent::Status { status: st });
        }
        Ok(script)
    }
}

#[derive(Clone, Debug)]
pub struct RejectScriptMsg {
    pub deck_id: String,
    pub rev: u64,
    pub reason: Option<String>,
}

impl Message<RejectScriptMsg> for DeckSupervisor {
    type Reply = Result<ScriptRevision, String>;

    async fn handle(
        &mut self,
        m: RejectScriptMsg,
        _: &mut Context<Self, Self::Reply>,
    ) -> Self::Reply {
        let script = self
            .shared
            .store
            .reject_script(&m.deck_id, m.rev, m.reason)
            .await
            .map_err(|e| format!("{e:#}"))?;
        self.mirror_script_ref(&m.deck_id).await;
        Ok(script)
    }
}

#[derive(Clone, Debug)]
pub struct ReadScriptMsg {
    pub deck_id: String,
    pub rev: Option<u64>,
}

impl Message<ReadScriptMsg> for DeckSupervisor {
    type Reply = Result<Option<ScriptRevision>, String>;

    async fn handle(
        &mut self,
        m: ReadScriptMsg,
        _: &mut Context<Self, Self::Reply>,
    ) -> Self::Reply {
        match m.rev {
            Some(r) => self
                .shared
                .store
                .get_script_revision(&m.deck_id, r)
                .await
                .map_err(|e| format!("{e:#}")),
            None => {
                if let Some(p) = self
                    .shared
                    .store
                    .get_pending_script(&m.deck_id)
                    .await
                    .map_err(|e| format!("{e:#}"))?
                {
                    return Ok(Some(p));
                }
                if let Some(a) = self
                    .shared
                    .store
                    .get_active_script(&m.deck_id)
                    .await
                    .map_err(|e| format!("{e:#}"))?
                {
                    return Ok(Some(a));
                }
                self.shared
                    .store
                    .get_latest_script(&m.deck_id)
                    .await
                    .map_err(|e| format!("{e:#}"))
            }
        }
    }
}

#[derive(Clone, Debug)]
pub struct RuntimeControlMsg {
    pub deck_id: String,
    pub op: RuntimeControlOp,
}

impl Message<RuntimeControlMsg> for DeckSupervisor {
    type Reply = kameo::reply::DelegatedReply<Result<DeckRuntimeStatus, String>>;

    async fn handle(
        &mut self,
        m: RuntimeControlMsg,
        ctx: &mut Context<Self, Self::Reply>,
    ) -> Self::Reply {
        let (reply, tx_reply) = ctx.reply_sender();
        let Some(tx_reply) = tx_reply else {
            return reply;
        };
        let tx = match self.ensure_owner(&m.deck_id).await {
            Ok(tx) => tx,
            Err(e) => {
                tx_reply.send(Err(e));
                return reply;
            }
        };
        tokio::spawn(async move {
            let (rtx, rrx) = oneshot::channel();
            if tx
                .send(OwnerCmd::Control {
                    op: m.op,
                    reply: rtx,
                })
                .await
                .is_err()
            {
                tx_reply.send(Err("deck owner channel closed".to_owned()));
                return;
            }
            let out = rrx
                .await
                .map_err(|_| "deck owner dropped control reply".to_owned())
                .and_then(|r| r);
            tx_reply.send(out);
        });
        reply
    }
}

#[derive(Clone, Debug)]
pub struct InteractMsg {
    pub deck_id: String,
    pub request_id: String,
    pub caller_id: String,
    pub action: String,
    pub args: Value,
    pub expected_rev: Option<u64>,
    pub timeout_ms: Option<u64>,
}

impl Message<InteractMsg> for DeckSupervisor {
    type Reply = kameo::reply::DelegatedReply<Result<InteractResponse, String>>;

    async fn handle(
        &mut self,
        m: InteractMsg,
        ctx: &mut Context<Self, Self::Reply>,
    ) -> Self::Reply {
        let (reply, tx_reply) = ctx.reply_sender();
        let Some(tx_reply) = tx_reply else {
            return reply;
        };
        let request_id = if m.request_id.trim().is_empty() {
            format!("req_{}", uuid::Uuid::now_v7().simple())
        } else {
            m.request_id
        };
        let timeout = Duration::from_millis(
            m.timeout_ms
                .unwrap_or(DEFAULT_INTERACT_TIMEOUT_MS)
                .clamp(500, MAX_INTERACT_TIMEOUT_MS),
        );
        let tx = match self.ensure_owner(&m.deck_id).await {
            Ok(tx) => tx,
            Err(e) => {
                tx_reply.send(Err(e));
                return reply;
            }
        };
        let (rtx, rrx) = oneshot::channel();
        let cmd = OwnerCmd::Interact {
            request_id: request_id.clone(),
            caller_id: m.caller_id,
            action: m.action.clone(),
            args: m.args,
            expected_rev: m.expected_rev,
            reply: rtx,
        };
        if tx.try_send(cmd).is_err() {
            tx_reply.send(Ok(InteractResponse {
                request_id,
                action: m.action,
                ok: false,
                result: None,
                error: Some("Deck runtime is busy; retry shortly".into()),
                error_code: Some(InteractErrorCode::Busy),
                private: true,
                cached: false,
                run_epoch: 0,
                script_rev: 0,
            }));
            return reply;
        }
        let action = m.action;
        tokio::spawn(async move {
            let res = match tokio::time::timeout(timeout, rrx).await {
                Ok(Ok(resp)) => Ok(resp),
                Ok(Err(_)) => Err("deck owner dropped interact reply".into()),
                Err(_) => Ok(InteractResponse {
                    request_id,
                    action,
                    ok: false,
                    result: None,
                    error: Some(format!("action timed out after {} ms", timeout.as_millis())),
                    error_code: Some(InteractErrorCode::TimedOut),
                    private: true,
                    cached: false,
                    run_epoch: 0,
                    script_rev: 0,
                }),
            };
            tx_reply.send(res);
        });
        reply
    }
}

#[derive(Clone, Debug)]
pub struct ListSnapshotsMsg {
    pub deck_id: String,
    pub limit: Option<u32>,
}

impl Message<ListSnapshotsMsg> for DeckSupervisor {
    type Reply = Result<Vec<DeckSnapshot>, String>;

    async fn handle(
        &mut self,
        m: ListSnapshotsMsg,
        _: &mut Context<Self, Self::Reply>,
    ) -> Self::Reply {
        self.shared
            .store
            .list_snapshots(&m.deck_id, m.limit.unwrap_or(20))
            .await
            .map_err(|e| format!("{e:#}"))
    }
}

#[derive(Clone, Debug)]
pub struct GetSnapshotMsg {
    pub deck_id: String,
    pub snapshot_id: String,
}

impl Message<GetSnapshotMsg> for DeckSupervisor {
    type Reply = Result<Option<DeckSnapshot>, String>;

    async fn handle(
        &mut self,
        m: GetSnapshotMsg,
        _: &mut Context<Self, Self::Reply>,
    ) -> Self::Reply {
        self.shared
            .store
            .get_snapshot(&m.deck_id, &m.snapshot_id)
            .await
            .map_err(|e| format!("{e:#}"))
    }
}

#[derive(Clone, Debug)]
pub struct SubscribeDeckMsg {
    pub deck_id: String,
}

impl Message<SubscribeDeckMsg> for DeckSupervisor {
    type Reply = Result<(Arc<DeckHub>, u64, mpsc::Receiver<DeckRuntimeEvent>), String>;

    async fn handle(
        &mut self,
        m: SubscribeDeckMsg,
        _: &mut Context<Self, Self::Reply>,
    ) -> Self::Reply {
        let (status, last_snapshot, live_frame) = self.get_runtime_snapshot(&m.deck_id).await?;
        let sync = DeckRuntimeEvent::Sync {
            status,
            last_snapshot,
            live_frame,
        };
        let (sub_id, rx) = self.shared.hub.subscribe(&m.deck_id, sync);
        Ok((self.shared.hub.clone(), sub_id, rx))
    }
}

/// Bus subscriber forwarding `ApprovalRequested` / `ApprovalResolved` to [`SharedDeckState`].
pub struct DeckApprovalWatcher {
    shared: SharedDeckState,
}

impl DeckApprovalWatcher {
    pub fn new(shared: SharedDeckState) -> Self {
        Self { shared }
    }
}

impl kameo::Actor for DeckApprovalWatcher {
    type Args = Self;
    type Error = anyhow::Error;

    async fn on_start(args: Self::Args, _actor_ref: ActorRef<Self>) -> Result<Self, Self::Error> {
        Ok(args)
    }
}

impl Message<ApprovalRequested> for DeckApprovalWatcher {
    type Reply = ();

    async fn handle(
        &mut self,
        ev: ApprovalRequested,
        _: &mut Context<Self, Self::Reply>,
    ) -> Self::Reply {
        self.shared.on_approval_requested(&ev);
    }
}

impl Message<ApprovalResolved> for DeckApprovalWatcher {
    type Reply = ();

    async fn handle(
        &mut self,
        ev: ApprovalResolved,
        _: &mut Context<Self, Self::Reply>,
    ) -> Self::Reply {
        self.shared.on_approval_resolved(&ev);
    }
}

/// Helper to surface an approval ID cleanly in tests and diagnostics.
pub fn pending_approval_id(shared: &SharedDeckState, deck_id: &str) -> Option<ApprovalId> {
    shared
        .pending_approvals
        .lock()
        .get(deck_id)
        .map(|p| p.approval_id)
}
