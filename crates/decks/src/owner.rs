//! Single-owner Deck runtime supervisor & per-Deck actor loop.
//!
//! Invariants enforced here:
//! - Exactly one `DeckOwner` task owns at most one `DeckCheckout` per Deck.
//! - Stream steps (`on_step`/`collect`) and interactive actions (`on_action`) are serialized
//!   strictly at cooperative turn boundaries — Python never runs concurrently for a Deck.
//! - Read-only queries (`deck_get`, `deck_list`, `deck_subscribe`) read from `SharedDeckState::live_cache`
//!   without ever round-tripping through a potentially blocked `DeckOwner` (229 B4).
//! - `run_epoch` increments on start, stop, deletion, and revision upgrade so stale frames/events
//!   are fenced and late writes after deletion or revision upgrade are discarded (229 H2/H7).
//! - Each Deck runs in its own isolated work directory (`<data_dir>/deck-work/<deck_id>`) with
//!   seated `cwd`, `write_root`, and `SetApprovals(session, true)` pinned (229 H1).
//! - Concurrent tool approvals are queued per Deck and restore the exact pre-approval runtime state
//!   when cleared (229 M1).

use std::collections::{HashMap, HashSet};
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
use document::{DocId, DocKind, DocumentActor, DocumentChanged, ListDocs, Principal};
use kameo::actor::ActorRef;
use kameo::message::{Context, Message};
use parking_lot::Mutex;
use runtime::{Runtime, SessionId};
use serde_json::{Value, json};
use tokio::sync::{mpsc, oneshot};
use tokio::time::Instant;
use tools::{
    ApprovalId, ApprovalRequested, ApprovalResolved, Decision, SetApprovals, ToolRegistry,
};

use crate::hub::{DeckHub, MAX_SNAPSHOT_BYTES};
use crate::model::{
    DeckChanged, DeckDetail, DeckRuntimeEvent, DeckRuntimeStatus, DeckSnapshot, DeckSummary,
    DeclaredAction, InteractErrorCode, InteractResponse, LiveFrame, MAX_INTERVAL_SECS,
    MetricSample, PendingApproval, RuntimeControlOp, RuntimeState, ScriptMode, ScriptRevision,
    ScriptStatus, TriggerSpec, canonical_json_string, clamp_interval_secs,
};
use crate::runtime::{DECK_POOL_MAX, DeckCheckout, EmittedPayload, TurnContext, TurnEffects};
use crate::store::{
    DeckRecord, DeckStore, IdempotencyLookup, sha256_hex, validate_script_proposal,
};
use crate::triggers::{
    TriggerDecision, TriggerState, build_trigger_event, commit_fire_state, dispatch_trigger,
    evaluate_sample, mark_missing_metric, trigger_spec_sha256, verify_routines_exist,
};

/// Actor registry name for [`DeckSupervisor`].
pub const DECKS_ACTOR: &str = "decks";

/// Default timeout for an interactive action call if not specified by caller.
const DEFAULT_INTERACT_TIMEOUT_MS: u64 = 15_000;
const MAX_INTERACT_TIMEOUT_MS: u64 = 60_000;

/// Cached live runtime state served directly by `DeckSupervisor` without awaiting `DeckOwner` (229 B4).
#[derive(Clone, Debug)]
pub struct LiveDeckCache {
    pub status: DeckRuntimeStatus,
    pub last_snapshot: Option<DeckSnapshot>,
    pub live_frame: Option<LiveFrame>,
}

/// Shared state read by the approval/delete watcher, `DeckSupervisor`, and per-Deck owners.
#[derive(Clone)]
pub struct SharedDeckState {
    pub store: DeckStore,
    pub hub: Arc<DeckHub>,
    pub runtime: Runtime,
    pub document: ActorRef<DocumentActor>,
    pub registry: ActorRef<ToolRegistry>,
    /// Queued pending approvals per Deck so concurrent `asyncio.gather` approvals never clobber (229 M1).
    pub pending_approvals: Arc<Mutex<HashMap<String, Vec<PendingApproval>>>>,
    /// Pre-approval runtime state recorded when entering `WaitingApproval` (229 M1).
    pub pre_approval_state: Arc<Mutex<HashMap<String, RuntimeState>>>,
    pub session_index: Arc<Mutex<HashMap<SessionId, String>>>,
    /// Lock-free (non-blocking) snapshot of live Deck state for `GetDeck`/`ListDecks`/`Subscribe` (229 B4).
    pub live_cache: Arc<Mutex<HashMap<String, LiveDeckCache>>>,
    /// Tombstone set preventing late SQLite/hub writes from an in-flight turn after Deck deletion (229 H2).
    pub deleted_decks: Arc<Mutex<HashSet<String>>>,
}

impl SharedDeckState {
    pub fn register_session(&self, deck_id: &str, session_id: SessionId) {
        self.deleted_decks.lock().remove(deck_id);
        self.session_index
            .lock()
            .insert(session_id, deck_id.to_owned());
    }

    pub fn deck_for_session(&self, session_id: SessionId) -> Option<String> {
        self.session_index.lock().get(&session_id).cloned()
    }

    pub fn mark_deleted(&self, deck_id: &str) {
        self.deleted_decks.lock().insert(deck_id.to_owned());
        self.live_cache.lock().remove(deck_id);
        self.pending_approvals.lock().remove(deck_id);
        self.pre_approval_state.lock().remove(deck_id);
    }

    pub fn is_deleted(&self, deck_id: &str) -> bool {
        self.deleted_decks.lock().contains(deck_id)
    }

    pub fn head_pending_approval(&self, deck_id: &str) -> Option<PendingApproval> {
        self.pending_approvals
            .lock()
            .get(deck_id)
            .and_then(|q| q.first().cloned())
    }

    pub fn has_pending_approvals(&self, deck_id: &str) -> bool {
        self.pending_approvals
            .lock()
            .get(deck_id)
            .is_some_and(|q| !q.is_empty())
    }

    pub fn on_approval_requested(&self, ev: &ApprovalRequested) {
        let Some(deck_id) = self.deck_for_session(ev.session) else {
            return;
        };
        if self.is_deleted(&deck_id) {
            return;
        }
        let mut redacted_args = ev.args.clone();
        tools::secrets::redact_value(&mut redacted_args);
        let pending = PendingApproval {
            approval_id: ev.id,
            session_id: ev.session,
            tool: ev.tool.clone(),
            risk: ev.risk,
            args: redacted_args,
        };

        let head = {
            let mut map = self.pending_approvals.lock();
            let q = map.entry(deck_id.clone()).or_default();
            if !q.iter().any(|p| p.approval_id == pending.approval_id) {
                q.push(pending.clone());
            }
            q.first().cloned()
        };

        self.hub.broadcast(
            &deck_id,
            DeckRuntimeEvent::ApprovalPending {
                approval: pending.clone(),
            },
        );

        let updated_status = {
            let mut cache = self.live_cache.lock();
            if let Some(entry) = cache.get_mut(&deck_id) {
                if entry.status.state != RuntimeState::WaitingApproval {
                    self.pre_approval_state
                        .lock()
                        .entry(deck_id.clone())
                        .or_insert(entry.status.state);
                }
                entry.status.state = RuntimeState::WaitingApproval;
                entry.status.pending_approval = head;
                Some(entry.status.clone())
            } else {
                None
            }
        };
        if let Some(st) = updated_status {
            self.hub
                .broadcast(&deck_id, DeckRuntimeEvent::Status { status: st });
        }
    }

    pub fn on_approval_resolved(&self, ev: &ApprovalResolved) {
        let (deck_id, next_head) = {
            let mut map = self.pending_approvals.lock();
            let mut found = None;
            let mut next = None;
            for (k, q) in map.iter_mut() {
                if let Some(pos) = q.iter().position(|p| p.approval_id == ev.id) {
                    q.remove(pos);
                    found = Some(k.clone());
                    next = q.first().cloned();
                    break;
                }
            }
            if let Some(ref id) = found
                && next.is_none()
            {
                map.remove(id);
            }
            (found, next)
        };
        let Some(deck_id) = deck_id else {
            return;
        };
        if self.is_deleted(&deck_id) {
            return;
        }
        let approved = ev.decision == Decision::Allow && !ev.timed_out && !ev.dismissed;
        self.hub.broadcast(
            &deck_id,
            DeckRuntimeEvent::ApprovalCleared {
                approval_id: ev.id,
                approved,
            },
        );

        if let Some(ref next_pending) = next_head {
            self.hub.broadcast(
                &deck_id,
                DeckRuntimeEvent::ApprovalPending {
                    approval: next_pending.clone(),
                },
            );
        }

        let updated_status = {
            let mut cache = self.live_cache.lock();
            if let Some(entry) = cache.get_mut(&deck_id) {
                if let Some(next_pending) = next_head {
                    entry.status.state = RuntimeState::WaitingApproval;
                    entry.status.pending_approval = Some(next_pending);
                } else {
                    let restored = self.pre_approval_state.lock().remove(&deck_id);
                    entry.status.pending_approval = None;
                    if entry.status.state == RuntimeState::WaitingApproval {
                        entry.status.state = restored.unwrap_or(if entry.status.checkout_alive {
                            RuntimeState::Streaming
                        } else {
                            RuntimeState::Stopped
                        });
                    }
                }
                Some(entry.status.clone())
            } else {
                if next_head.is_none() {
                    self.pre_approval_state.lock().remove(&deck_id);
                }
                None
            }
        };
        if let Some(st) = updated_status {
            self.hub
                .broadcast(&deck_id, DeckRuntimeEvent::Status { status: st });
        }
    }
}

/// Error returned when a mutating `deck_interact` RPC call lacks a live human owner press.
pub(crate) const UNAUTHORIZED_INTERACT_ERR: &str =
    "unauthorized: human owner press required for mutating deck_interact";

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
        allow_mutating: bool,
        reply: oneshot::Sender<Result<InteractResponse, String>>,
    },
    RevisionApproved {
        script: ScriptRevision,
    },
    Delete,
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
        pool: Option<Arc<MontyPool>>,
    ) -> (Self, SharedDeckState) {
        let shared = SharedDeckState {
            store,
            hub: Arc::new(DeckHub::new()),
            runtime,
            document,
            registry,
            pending_approvals: Arc::new(Mutex::new(HashMap::new())),
            pre_approval_state: Arc::new(Mutex::new(HashMap::new())),
            session_index: Arc::new(Mutex::new(HashMap::new())),
            live_cache: Arc::new(Mutex::new(HashMap::new())),
            deleted_decks: Arc::new(Mutex::new(HashSet::new())),
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

    /// Verify the Deck document exists in `DocumentActor` before creating SQLite metadata or owners (229 H2).
    async fn verify_deck_exists(&self, deck_id: &str) -> Result<DeckDocSnapshot, String> {
        if self.shared.is_deleted(deck_id) {
            return Err(format!("deck `{deck_id}` has been deleted"));
        }
        self.read_doc(deck_id, Principal::Helper).await
    }

    /// Seat isolated per-Deck working dir, write root, and forced approvals for a Deck session (229 H1).
    async fn configure_deck_session(
        &self,
        deck_id: &str,
        session_id: SessionId,
    ) -> Result<PathBuf, String> {
        let work_dir = self
            .shared
            .store
            .deck_work_dir(deck_id)
            .map_err(|e| format!("{e:#}"))?;
        tools::seat::set_cwd(session_id, Some(work_dir.clone()));
        tools::seat::set_write_root(session_id, Some(work_dir.clone()));
        let _ = self
            .shared
            .registry
            .ask(SetApprovals {
                session: Some(session_id),
                enabled: true,
                clear: false,
            })
            .await;
        Ok(work_dir)
    }

    async fn ensure_owner(&mut self, deck_id: &str) -> Result<mpsc::Sender<OwnerCmd>, String> {
        self.verify_deck_exists(deck_id).await?;
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
        let work_dir = self.configure_deck_session(deck_id, rec.session_id).await?;
        let pool = self.resolve_pool().await?;
        let (tx, rx) = mpsc::channel(64);
        let owner =
            DeckOwner::load(deck_id.to_owned(), rec, work_dir, self.shared.clone(), pool).await?;
        tokio::spawn(owner.run(rx));
        self.owners.insert(deck_id.to_owned(), tx.clone());
        Ok(tx)
    }

    /// Read live runtime status/snapshot/frame directly from `live_cache` or SQLite
    /// WITHOUT ever awaiting `DeckOwner` (prevents global supervisor deadlock during approvals, 229 B4).
    /// Caller must ensure `deck_id` exists in `DocumentActor` first!
    async fn get_runtime_snapshot(
        &mut self,
        deck_id: &str,
    ) -> Result<(DeckRuntimeStatus, Option<DeckSnapshot>, Option<LiveFrame>), String> {
        if let Some(cached) = self.shared.live_cache.lock().get(deck_id).cloned() {
            let mut st = cached.status;
            if let Some(pa) = self.shared.head_pending_approval(deck_id) {
                st.state = RuntimeState::WaitingApproval;
                st.pending_approval = Some(pa);
            }
            return Ok((st, cached.last_snapshot, cached.live_frame));
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
        let pending_approval = self.shared.head_pending_approval(deck_id);
        let status = DeckRuntimeStatus {
            deck_id: deck_id.to_owned(),
            session_id: rec.session_id,
            state: if pending_approval.is_some() {
                RuntimeState::WaitingApproval
            } else {
                RuntimeState::Stopped
            },
            run_epoch: rec.run_epoch,
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

    async fn deck_title(&self, deck_id: &str) -> String {
        self.read_doc(deck_id, Principal::Helper)
            .await
            .map(|d| d.title)
            .unwrap_or_default()
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

    async fn cleanup_deleted_deck(&mut self, deck_id: &str, title: String) {
        // 1. Mark tombstoned immediately so any in-flight turn in DeckOwner is fenced out.
        self.shared.mark_deleted(deck_id);

        // 2. Stop & reap the DeckOwner if active. Never silently drop Delete if the channel is full.
        if let Some(tx) = self.owners.remove(deck_id)
            && let Err(tokio::sync::mpsc::error::TrySendError::Full(cmd)) =
                tx.try_send(OwnerCmd::Delete)
        {
            tokio::spawn(async move {
                let _ = tx.send(cmd).await;
            });
        }

        // 3. Clean up SQLite rows, `.monty` dump, spill dir, seat, and isolated work dir.
        if let Ok(Some(session_id)) = self.shared.store.delete_deck_state(deck_id).await {
            self.shared.session_index.lock().remove(&session_id);
            tools::seat::forget(session_id);
            let _ = std::fs::remove_file(self.shared.store.monty_dump_path(session_id));
            let _ =
                std::fs::remove_dir_all(self.shared.store.spill_dir().join(session_id.to_string()));
        }
        if let Ok(work_dir) = self.shared.store.deck_work_dir_path(deck_id)
            && work_dir.exists()
        {
            let _ = std::fs::remove_dir_all(work_dir);
        }

        // 4. Close any remaining subscribers AFTER state teardown completes, then prune hub entry.
        self.shared.hub.broadcast(
            deck_id,
            DeckRuntimeEvent::Closed {
                reason: "deck_deleted".into(),
            },
        );
        self.shared.hub.remove_deck(deck_id);

        self.publish_changed(deck_id, title, true).await;
    }
}

// ---------------------------------------------------------------------------
// Per-Deck single runtime owner task
// ---------------------------------------------------------------------------

struct DeckOwner {
    deck_id: String,
    session_id: SessionId,
    work_dir: PathBuf,
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
        work_dir: PathBuf,
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
            work_dir,
            shared,
            pool,
            checkout: None,
            active_script,
            pending_rev: rec.pending_rev,
            state: RuntimeState::Stopped,
            run_epoch: rec.run_epoch,
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
        owner.sync_live_cache();
        Ok(owner)
    }

    fn status(&self) -> DeckRuntimeStatus {
        let pending_approval = self.shared.head_pending_approval(&self.deck_id);
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

    fn sync_live_cache(&self) {
        if self.shared.is_deleted(&self.deck_id) {
            return;
        }
        self.shared.live_cache.lock().insert(
            self.deck_id.clone(),
            LiveDeckCache {
                status: self.status(),
                last_snapshot: self.last_snapshot.clone(),
                live_frame: self.live_frame.clone(),
            },
        );
    }

    fn broadcast_status(&self) {
        if self.shared.is_deleted(&self.deck_id) {
            return;
        }
        let st = self.status();
        self.shared.live_cache.lock().insert(
            self.deck_id.clone(),
            LiveDeckCache {
                status: st.clone(),
                last_snapshot: self.last_snapshot.clone(),
                live_frame: self.live_frame.clone(),
            },
        );
        self.shared
            .hub
            .broadcast(&self.deck_id, DeckRuntimeEvent::Status { status: st });
    }

    /// Returns `true` when the Deck was deleted, `run_epoch` moved, or `script_rev` is no longer active (229 H2).
    async fn is_fenced(&mut self, turn_epoch: u64, script_rev: u64) -> bool {
        if self.shared.is_deleted(&self.deck_id) || self.run_epoch != turn_epoch {
            return true;
        }
        !self
            .shared
            .store
            .is_active_rev(&self.deck_id, script_rev)
            .await
            .unwrap_or(false)
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
        let mut deleted = false;
        loop {
            match self.next_wake {
                Some(deadline) => {
                    tokio::select! {
                        biased;
                        cmd = rx.recv() => {
                            let Some(cmd) = cmd else { break };
                            match self.handle_cmd(cmd).await {
                                CmdFlow::Continue => {}
                                CmdFlow::Stop => break,
                                CmdFlow::Deleted => { deleted = true; break; }
                            }
                        }
                        _ = tokio::time::sleep_until(deadline) => {
                            self.next_wake = None;
                            if self.shared.is_deleted(&self.deck_id) {
                                deleted = true;
                                break;
                            }
                            self.handle_tick().await;
                        }
                    }
                }
                None => {
                    let Some(cmd) = rx.recv().await else { break };
                    match self.handle_cmd(cmd).await {
                        CmdFlow::Continue => {}
                        CmdFlow::Stop => break,
                        CmdFlow::Deleted => {
                            deleted = true;
                            break;
                        }
                    }
                }
            }
        }
        if deleted || self.shared.is_deleted(&self.deck_id) {
            // Drop checkout without writing any checkpoint to SQLite or disk;
            // `DeckSupervisor::cleanup_deleted_deck` broadcasts `Closed { reason: "deck_deleted" }`
            // once SQLite, `.monty`, work_dir, and seat teardown completes.
            if let Some(checkout) = self.checkout.take() {
                drop(checkout);
            }
        } else {
            let _ = self.stop_checkout().await;
            self.shared.hub.broadcast(
                &self.deck_id,
                DeckRuntimeEvent::Closed {
                    reason: "deck_owner_stopped".into(),
                },
            );
        }
    }

    async fn handle_cmd(&mut self, cmd: OwnerCmd) -> CmdFlow {
        match cmd {
            OwnerCmd::Shutdown => {
                let _ = self.stop_checkout().await;
                return CmdFlow::Stop;
            }
            OwnerCmd::Delete => {
                self.next_wake = None;
                self.live_frame = None;
                self.run_epoch += 1;
                self.state = RuntimeState::Stopped;
                if let Some(checkout) = self.checkout.take() {
                    drop(checkout);
                }
                return CmdFlow::Deleted;
            }
            OwnerCmd::Control { op, reply } => {
                let res = if self.shared.is_deleted(&self.deck_id) {
                    Err(format!("deck `{}` has been deleted", self.deck_id))
                } else {
                    self.handle_control(op).await
                };
                let _ = reply.send(res);
            }
            OwnerCmd::Interact {
                request_id,
                caller_id,
                action,
                args,
                expected_rev,
                allow_mutating,
                reply,
            } => {
                if reply.is_closed() {
                    // Caller disconnected before execution began — do not replay mutation.
                    return CmdFlow::Continue;
                }
                let resp = self
                    .handle_interact(
                        request_id,
                        caller_id,
                        action,
                        args,
                        expected_rev,
                        allow_mutating,
                    )
                    .await;
                let _ = reply.send(resp);
            }
            OwnerCmd::RevisionApproved { script } => {
                if !self.shared.is_deleted(&self.deck_id) {
                    self.handle_revision_approved(script).await;
                }
            }
        }
        CmdFlow::Continue
    }

    async fn refresh_scripts_from_store(&mut self) -> Result<(), String> {
        if let Ok(Some(rec)) = self.shared.store.get_deck(&self.deck_id).await {
            self.pending_rev = rec.pending_rev;
        }
        match self.shared.store.get_active_script(&self.deck_id).await {
            Ok(active) => {
                self.active_script = active;
                Ok(())
            }
            Err(e) => {
                self.active_script = None;
                Err(format!("{e:#}"))
            }
        }
    }

    async fn bump_epoch(&mut self) -> u64 {
        self.run_epoch = self.run_epoch.saturating_add(1);
        let _ = self
            .shared
            .store
            .set_run_epoch(&self.deck_id, self.run_epoch)
            .await;
        self.run_epoch
    }

    /// 229 B3 & H7: Approving a new revision stops any previous checkout, clears stale transient
    /// `live_frame`, bumps `run_epoch` (even if paused/stopped), and leaves the Deck in `Stopped`
    /// so "Approve only" NEVER enables recurring streaming or scheduling until explicit owner `Start`.
    async fn handle_revision_approved(&mut self, script: ScriptRevision) {
        self.next_wake = None;
        self.active_script = Some(script);
        let _ = self.refresh_scripts_from_store().await;

        if self.checkout.is_some() {
            let _ = self.stop_checkout().await;
        }
        let _ = self
            .shared
            .store
            .reset_trigger_states_for_deck(&self.deck_id)
            .await;
        self.live_frame = None;
        self.bump_epoch().await;
        self.state = RuntimeState::Stopped;
        self.broadcast_status();
    }

    async fn handle_control(&mut self, op: RuntimeControlOp) -> Result<DeckRuntimeStatus, String> {
        if let Err(err) = self.refresh_scripts_from_store().await {
            self.record_failure(&err).await;
            self.state = RuntimeState::Error;
            self.broadcast_status();
            return Err(err);
        }
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
                if self.state == RuntimeState::Paused
                    && self
                        .checkout
                        .as_ref()
                        .is_some_and(|c| c.is_alive() && c.script_rev() == script.rev)
                {
                    return self.resume_stream(&script).await;
                }
                if matches!(
                    self.state,
                    RuntimeState::Streaming | RuntimeState::ScheduledIdle
                ) && self.checkout.as_ref().is_some_and(|c| {
                    c.script_rev() == script.rev
                        && c.revision_sha256() == script.revision_sha256
                        && c.is_alive()
                }) {
                    return Ok(self.status());
                }

                let turn_epoch = self.bump_epoch().await;
                self.live_frame = None;
                self.state = RuntimeState::Starting;
                self.broadcast_status();

                if let Err(err) = self.ensure_checkout(&script, turn_epoch).await {
                    if !self.is_fenced(turn_epoch, script.rev).await {
                        self.record_failure(&err).await;
                        self.state = RuntimeState::Error;
                        self.broadcast_status();
                    }
                    return Err(err);
                }
                if self.is_fenced(turn_epoch, script.rev).await {
                    return Err("script revision changed or Deck was deleted during start".into());
                }

                let (stream_en, sched_en) = match script.mode {
                    ScriptMode::Oneshot => (false, true),
                    ScriptMode::Streaming | ScriptMode::Hybrid => (
                        true,
                        script.interval_secs.is_some() || script.schedule_cron.is_some(),
                    ),
                };
                let _ = self
                    .shared
                    .store
                    .set_runtime_enabled(&self.deck_id, stream_en, sched_en)
                    .await;
                self.state = match script.mode {
                    ScriptMode::Oneshot => RuntimeState::ScheduledIdle,
                    ScriptMode::Streaming | ScriptMode::Hybrid => RuntimeState::Streaming,
                };
                // Execute first step right away so callers see fresh data immediately.
                self.next_wake = None;
                let step_err = self.execute_one_step(&script, turn_epoch).await;
                if !self.is_fenced(turn_epoch, script.rev).await
                    && matches!(
                        self.state,
                        RuntimeState::Streaming | RuntimeState::ScheduledIdle
                    )
                {
                    if step_err.is_some() {
                        self.schedule_backoff();
                    } else if self.next_wake.is_none() {
                        self.schedule_next_wake(&script, None);
                    }
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
                    .set_runtime_enabled(&self.deck_id, false, false)
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
                if script.status != ScriptStatus::Approved {
                    return Err("script revision is not approved".into());
                }
                self.resume_stream(&script).await
            }
            RuntimeControlOp::Stop => {
                self.next_wake = None;
                let _ = self
                    .shared
                    .store
                    .set_runtime_enabled(&self.deck_id, false, false)
                    .await;
                self.stop_checkout().await?;
                self.live_frame = None;
                self.bump_epoch().await;
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
                let was_stopped = matches!(self.state, RuntimeState::Stopped | RuntimeState::Error);
                if was_stopped {
                    self.bump_epoch().await;
                }
                let turn_epoch = self.run_epoch;
                if let Err(err) = self.ensure_checkout(&script, turn_epoch).await {
                    if !self.is_fenced(turn_epoch, script.rev).await {
                        self.record_failure(&err).await;
                        self.broadcast_status();
                    }
                    return Err(err);
                }
                let step_err = self.execute_one_step(&script, turn_epoch).await;
                if step_err.is_none() && self.state == RuntimeState::Error {
                    self.state = RuntimeState::Stopped;
                }
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
        let (stream_en, sched_en) = match script.mode {
            ScriptMode::Oneshot => (false, true),
            ScriptMode::Streaming | ScriptMode::Hybrid => (
                true,
                script.interval_secs.is_some() || script.schedule_cron.is_some(),
            ),
        };
        let _ = self
            .shared
            .store
            .set_runtime_enabled(&self.deck_id, stream_en, sched_en)
            .await;
        if self.checkout.as_ref().is_none_or(|c| {
            !c.is_alive()
                || c.script_rev() != script.rev
                || c.revision_sha256() != script.revision_sha256
        }) {
            let turn_epoch = self.bump_epoch().await;
            if let Err(err) = self.ensure_checkout(script, turn_epoch).await {
                if !self.is_fenced(turn_epoch, script.rev).await {
                    self.record_failure(&err).await;
                    self.state = RuntimeState::Error;
                    self.broadcast_status();
                }
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
        if let Err(err) = self.refresh_scripts_from_store().await {
            self.record_failure(&err).await;
            self.state = RuntimeState::Error;
            self.broadcast_status();
            return;
        }
        let Some(script) = self.active_script.clone() else {
            self.state = RuntimeState::Stopped;
            self.broadcast_status();
            return;
        };
        let turn_epoch = self.run_epoch;
        if let Err(err) = self.ensure_checkout(&script, turn_epoch).await {
            if !self.is_fenced(turn_epoch, script.rev).await {
                self.record_failure(&err).await;
                self.schedule_backoff();
                self.broadcast_status();
            }
            return;
        }
        let step_err = self.execute_one_step(&script, turn_epoch).await;
        if self.is_fenced(turn_epoch, script.rev).await {
            return;
        }
        if matches!(
            self.state,
            RuntimeState::Streaming | RuntimeState::ScheduledIdle
        ) {
            if step_err.is_some() {
                self.schedule_backoff();
            } else if self.next_wake.is_none() {
                self.schedule_next_wake(&script, None);
            }
        }
        self.broadcast_status();
    }

    fn schedule_backoff(&mut self) {
        let shift = self.consecutive_failures.min(6);
        let secs = (1u64 << shift).clamp(2, 60);
        self.next_wake = checked_wake(Duration::from_secs(secs));
    }

    fn schedule_next_wake(&mut self, script: &ScriptRevision, sleep_override: Option<f64>) {
        if let Some(secs) = sleep_override {
            let clamped = secs.clamp(0.1, 3600.0);
            self.next_wake = checked_wake(Duration::from_secs_f64(clamped));
            return;
        }
        if let Some(expr) = script.schedule_cron.as_deref()
            && let Ok(cron) = croner::Cron::from_str(expr)
            && let Ok(next_dt) = cron.find_next_occurrence(&Utc::now(), false)
        {
            let delta = (next_dt - Utc::now())
                .to_std()
                .unwrap_or(Duration::from_secs(60))
                .clamp(
                    Duration::from_secs(1),
                    Duration::from_secs(MAX_INTERVAL_SECS),
                );
            self.next_wake = checked_wake(delta);
            return;
        }
        let raw_interval = script.interval_secs.unwrap_or(match script.mode {
            ScriptMode::Streaming | ScriptMode::Hybrid => 3,
            ScriptMode::Oneshot => 60,
        });
        let interval = clamp_interval_secs(raw_interval);
        self.next_wake = checked_wake(Duration::from_secs(interval));
    }

    async fn ensure_checkout(
        &mut self,
        script: &ScriptRevision,
        turn_epoch: u64,
    ) -> Result<(), String> {
        let mut allow_warm_restore = true;
        let compatible = self.checkout.as_ref().is_some_and(|c| {
            c.is_alive()
                && c.script_rev() == script.rev
                && c.revision_sha256() == script.revision_sha256
        });
        if compatible {
            if let Some(c) = self.checkout.as_mut() {
                match c.maybe_refresh_or_rollover().await {
                    Ok(_) => return Ok(()),
                    Err(e) => {
                        tracing::warn!(
                            deck = %self.deck_id,
                            "rollover failed ({e}); clearing stale dump and cold-restarting"
                        );
                        self.checkout = None;
                        allow_warm_restore = false;
                        let _ = std::fs::remove_file(
                            self.shared.store.monty_dump_path(self.session_id),
                        );
                        let _ = self.shared.store.clear_monty_dump_meta(&self.deck_id).await;
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
            &self.work_dir,
            &self.shared.store.spill_dir(),
            dump_path,
            script,
            cp.as_ref(),
            allow_warm_restore,
            &turn_cx,
            &mut |p| collected_emits.push(p.clone()),
        )
        .await?;

        if self.is_fenced(turn_epoch, script.rev).await {
            drop(checkout);
            return Err("deck deleted or revision superseded during startup".into());
        }

        let dump_sha = checkout.last_dump_sha256().map(str::to_owned);
        self.checkout = Some(checkout);
        let dump_meta = dump_sha
            .as_deref()
            .map(|sha| (script.rev, script.revision_sha256.as_str(), sha));
        let _ = self
            .shared
            .store
            .save_checkpoint(
                &self.deck_id,
                script.rev,
                &script.code_sha256,
                effects.checkpoint_state.as_ref(),
                dump_meta,
            )
            .await;
        self.apply_turn_effects(script, turn_epoch, effects, None, false)
            .await;
        Ok(())
    }

    async fn stop_checkout(&mut self) -> Result<(), String> {
        let Some(checkout) = self.checkout.take() else {
            return Ok(());
        };
        let rev = checkout.script_rev();
        let code_sha = checkout.code_sha256().to_owned();
        let rev_sha = checkout.revision_sha256().to_owned();
        let dump_sha = checkout.stop_and_release().await.ok().flatten();
        if !self.shared.is_deleted(&self.deck_id)
            && self
                .shared
                .store
                .is_active_rev(&self.deck_id, rev)
                .await
                .unwrap_or(false)
            && let Some(ref dsha) = dump_sha
        {
            let _ = self
                .shared
                .store
                .save_checkpoint(
                    &self.deck_id,
                    rev,
                    &code_sha,
                    None,
                    Some((rev, &rev_sha, dsha)),
                )
                .await;
        }
        Ok(())
    }

    async fn execute_one_step(
        &mut self,
        script: &ScriptRevision,
        turn_epoch: u64,
    ) -> Option<String> {
        let turn_cx = self.turn_context();
        let Some(checkout) = self.checkout.as_mut() else {
            let err = "no active checkout".to_owned();
            if !self.is_fenced(turn_epoch, script.rev).await {
                self.record_failure(&err).await;
            }
            return Some(err);
        };
        let res = checkout.run_step(&turn_cx, &mut |_| {}).await;
        let dump_sha = self
            .checkout
            .as_ref()
            .and_then(|c| c.last_dump_sha256().map(str::to_owned));

        // 229 H2: verify Deck still exists, epoch unchanged, and script.rev is still active BEFORE committing.
        if self.is_fenced(turn_epoch, script.rev).await {
            tracing::info!(
                deck = %self.deck_id,
                rev = script.rev,
                turn_epoch,
                "discarding stale step output after revision upgrade or Deck deletion"
            );
            return None;
        }

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
                self.apply_turn_effects(script, turn_epoch, outcome.effects, fallback_return, true)
                    .await;
                if matches!(
                    self.state,
                    RuntimeState::Streaming | RuntimeState::ScheduledIdle
                ) {
                    self.schedule_next_wake(script, sleep_override);
                }
                if let Some(ref dsha) = dump_sha {
                    let _ = self
                        .shared
                        .store
                        .save_checkpoint(
                            &self.deck_id,
                            script.rev,
                            &script.code_sha256,
                            None,
                            Some((script.rev, &script.revision_sha256, dsha)),
                        )
                        .await;
                }
                None
            }
            Err(err) => {
                self.record_failure(&err).await;
                self.reset_trigger_streaks_on_gap(script, turn_epoch).await;
                Some(err)
            }
        }
    }

    async fn apply_turn_effects(
        &mut self,
        script: &ScriptRevision,
        turn_epoch: u64,
        effects: TurnEffects,
        fallback_snapshot: Option<Value>,
        is_step_turn: bool,
    ) {
        if self.is_fenced(turn_epoch, script.rev).await {
            return;
        }
        let dump_sha = self
            .checkout
            .as_ref()
            .and_then(|c| c.last_dump_sha256().map(str::to_owned));
        if let Some(state) = effects.checkpoint_state {
            self.last_state = state.clone();
            let dump_meta = dump_sha
                .as_deref()
                .map(|sha| (script.rev, script.revision_sha256.as_str(), sha));
            let _ = self
                .shared
                .store
                .save_checkpoint(
                    &self.deck_id,
                    script.rev,
                    &script.code_sha256,
                    Some(&state),
                    dump_meta,
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
        self.process_turn_triggers(script, turn_epoch, &effects.metrics, is_step_turn)
            .await;
        self.sync_live_cache();
    }

    async fn resolve_deck_title(&mut self) -> String {
        let by = Principal::Owner { device: None };
        match self
            .shared
            .document
            .ask(DeckCall {
                doc: DocId(self.deck_id.clone()),
                op: DeckOp::Read,
                by,
            })
            .send()
            .await
        {
            Ok(DeckCallReply::Snapshot(snap)) => snap.title,
            _ => self.deck_id.clone(),
        }
    }

    /// Break in-progress consecutive streaks when a step fails to run (treated as a sample gap).
    async fn reset_trigger_streaks_on_gap(&mut self, script: &ScriptRevision, turn_epoch: u64) {
        if script.triggers.is_empty() || self.is_fenced(turn_epoch, script.rev).await {
            return;
        }
        for spec in &script.triggers {
            let spec_sha = trigger_spec_sha256(spec);
            let Ok(Some(mut state)) = self
                .shared
                .store
                .load_trigger_state(&self.deck_id, &spec.id)
                .await
            else {
                continue;
            };
            if mark_missing_metric(script.rev, &spec_sha, &mut state) {
                let _ = self.shared.store.save_trigger_state(&state).await;
            }
        }
    }

    /// Host-side evaluation and dispatch of `deck_metric(...)` samples strictly after epoch/rev fence.
    async fn process_turn_triggers(
        &mut self,
        script: &ScriptRevision,
        turn_epoch: u64,
        metrics: &[MetricSample],
        is_step_turn: bool,
    ) {
        if script.triggers.is_empty() || self.is_fenced(turn_epoch, script.rev).await {
            return;
        }

        let active_ids: Vec<String> = script.triggers.iter().map(|t| t.id.clone()).collect();
        let _ = self
            .shared
            .store
            .prune_obsolete_trigger_states(&self.deck_id, &active_ids)
            .await;

        let mut deck_title_cache: Option<String> = None;
        let mut reported_metrics: HashSet<&str> = HashSet::with_capacity(metrics.len());

        for sample in metrics {
            reported_metrics.insert(sample.name.as_str());
            for spec in &script.triggers {
                if spec.metric != sample.name {
                    continue;
                }
                if self.is_fenced(turn_epoch, script.rev).await {
                    return;
                }
                let spec_sha = trigger_spec_sha256(spec);
                let mut state = match self
                    .shared
                    .store
                    .load_trigger_state(&self.deck_id, &spec.id)
                    .await
                {
                    Ok(Some(st)) => st,
                    Ok(None) => {
                        TriggerState::fresh(&self.deck_id, &spec.id, script.rev, spec_sha.clone())
                    }
                    Err(e) => {
                        tracing::warn!(
                            deck = %self.deck_id,
                            trigger = %spec.id,
                            "load_trigger_state failed: {e:#}"
                        );
                        continue;
                    }
                };

                match evaluate_sample(spec, script.rev, &spec_sha, &mut state, sample) {
                    TriggerDecision::Hold | TriggerDecision::Cleared => {
                        if let Err(e) = self.shared.store.save_trigger_state(&state).await {
                            tracing::warn!(
                                deck = %self.deck_id,
                                trigger = %spec.id,
                                "save_trigger_state failed: {e:#}"
                            );
                        }
                    }
                    TriggerDecision::Fire(plan) => {
                        let reserved = match self
                            .shared
                            .store
                            .reserve_trigger_fire(
                                &plan.dedup_key,
                                &self.deck_id,
                                &spec.id,
                                script.rev,
                                &spec.routine,
                                plan.fired_at_ms,
                            )
                            .await
                        {
                            Ok(r) => r,
                            Err(e) => {
                                tracing::warn!(
                                    deck = %self.deck_id,
                                    trigger = %spec.id,
                                    "reserve_trigger_fire failed: {e:#}"
                                );
                                continue;
                            }
                        };

                        if !reserved {
                            // Already recorded (e.g. crash/restart replay): advance state without double-firing.
                            commit_fire_state(spec, &mut state, &plan);
                            let _ = self.shared.store.save_trigger_state(&state).await;
                            continue;
                        }

                        if self.is_fenced(turn_epoch, script.rev).await {
                            let _ = self
                                .shared
                                .store
                                .release_trigger_fire(&plan.dedup_key)
                                .await;
                            return;
                        }

                        let title = match &deck_title_cache {
                            Some(t) => t.clone(),
                            None => {
                                let t = self.resolve_deck_title().await;
                                deck_title_cache = Some(t.clone());
                                t
                            }
                        };
                        let event = build_trigger_event(
                            &self.deck_id,
                            &title,
                            script.rev,
                            spec,
                            sample,
                            &plan,
                        );
                        match dispatch_trigger(&spec.routine, event).await {
                            Ok(target) => {
                                let target_str = match target {
                                    schedule::TriggerTarget::Session(s) => format!("session:{s}"),
                                    schedule::TriggerTarget::Task(id) => format!("task:{id}"),
                                };
                                let _ = self
                                    .shared
                                    .store
                                    .finalize_trigger_fire(
                                        &plan.dedup_key,
                                        &self.deck_id,
                                        &target_str,
                                    )
                                    .await;
                                commit_fire_state(spec, &mut state, &plan);
                                let _ = self.shared.store.save_trigger_state(&state).await;
                                tracing::info!(
                                    deck = %self.deck_id,
                                    trigger = %spec.id,
                                    routine = %spec.routine,
                                    target = %target_str,
                                    "deck trigger fired routine"
                                );
                            }
                            Err(err) => {
                                tracing::warn!(
                                    deck = %self.deck_id,
                                    trigger = %spec.id,
                                    routine = %spec.routine,
                                    "deck trigger dispatch refused/failed: {err}"
                                );
                                let _ = self
                                    .shared
                                    .store
                                    .release_trigger_fire(&plan.dedup_key)
                                    .await;
                                let _ = self.shared.store.save_trigger_state(&state).await;
                            }
                        }
                    }
                }
            }
        }

        // On step/collection turns, any trigger whose metric was omitted experiences a sample gap.
        if is_step_turn && !self.is_fenced(turn_epoch, script.rev).await {
            for spec in &script.triggers {
                if reported_metrics.contains(spec.metric.as_str()) {
                    continue;
                }
                let spec_sha = trigger_spec_sha256(spec);
                let Ok(Some(mut state)) = self
                    .shared
                    .store
                    .load_trigger_state(&self.deck_id, &spec.id)
                    .await
                else {
                    continue;
                };
                if mark_missing_metric(script.rev, &spec_sha, &mut state) {
                    let _ = self.shared.store.save_trigger_state(&state).await;
                }
            }
        }
    }

    async fn handle_emit(&mut self, script: &ScriptRevision, emit: EmittedPayload) {
        if self.shared.is_deleted(&self.deck_id) {
            return;
        }
        // Only transient live frames are subject to the burst rate limiter; durable snapshots
        // must never be dropped silently.
        if !emit.durable && !self.shared.hub.admit_emit(&self.deck_id) {
            tracing::debug!(deck = %self.deck_id, "deck live frame emit rate-limited");
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
                let err = format!("save_snapshot failed: {e:#}");
                tracing::warn!(deck = %self.deck_id, "{err}");
                self.record_failure(&err).await;
                self.broadcast_status();
                return;
            }
            self.stale = false;
            self.consecutive_failures = 0;
            self.last_error = None;
            self.last_ok_snapshot_id = Some(snap.snapshot_id.clone());
            self.last_ok_at = Some(now);
            self.last_snapshot = Some(snap.clone());
            self.sync_live_cache();
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
            self.sync_live_cache();
            self.shared
                .hub
                .broadcast(&self.deck_id, DeckRuntimeEvent::LiveFrame { frame });
        }
    }

    async fn record_failure(&mut self, error: &str) {
        if self.shared.is_deleted(&self.deck_id) {
            return;
        }
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
        self.sync_live_cache();
    }

    async fn handle_interact(
        &mut self,
        request_id: String,
        caller_id: String,
        action: String,
        args: Value,
        expected_rev: Option<u64>,
        allow_mutating: bool,
    ) -> Result<InteractResponse, String> {
        let err_resp =
            |code: InteractErrorCode, msg: String, rev: u64, epoch: u64| InteractResponse {
                request_id: request_id.clone(),
                action: action.clone(),
                ok: false,
                result: None,
                error: Some(tools::secrets::redact(&msg).into_owned()),
                error_code: Some(code),
                private: true,
                cached: false,
                run_epoch: epoch,
                script_rev: rev,
            };

        if self.shared.is_deleted(&self.deck_id) {
            return Ok(err_resp(
                InteractErrorCode::ExecutionError,
                format!("deck `{}` has been deleted", self.deck_id),
                0,
                self.run_epoch,
            ));
        }

        if let Err(err) = self.refresh_scripts_from_store().await {
            return Ok(err_resp(
                InteractErrorCode::ExecutionError,
                err,
                0,
                self.run_epoch,
            ));
        }

        // 1. Approved script required.
        let Some(script) = self.active_script.clone() else {
            return Ok(err_resp(
                InteractErrorCode::NoApprovedScript,
                "no approved script revision for this Deck".into(),
                0,
                self.run_epoch,
            ));
        };
        if script.status != ScriptStatus::Approved {
            return Ok(err_resp(
                InteractErrorCode::NoApprovedScript,
                "active script is not approved".into(),
                script.rev,
                self.run_epoch,
            ));
        }

        // 2. Revision fencing.
        if let Some(exp) = expected_rev
            && exp != script.rev
        {
            return Ok(err_resp(
                InteractErrorCode::RevisionMismatch,
                format!("expected script rev {exp}, active rev is {}", script.rev),
                script.rev,
                self.run_epoch,
            ));
        }

        // 3. Declared action lookup & mutating human-press gate (checked BEFORE idempotency replay).
        let Some(decl) = script.declared_actions.iter().find(|a| a.name == action) else {
            return Ok(err_resp(
                InteractErrorCode::UndeclaredAction,
                format!(
                    "action {action:?} is not declared by script rev {}",
                    script.rev
                ),
                script.rev,
                self.run_epoch,
            ));
        };
        if decl.mutates_state && !allow_mutating {
            return Err(UNAUTHORIZED_INTERACT_ERR.to_owned());
        }

        // 4. Caller-scoped idempotency lookup (229 B7): strictly keyed on
        // (deck_id, caller_id, request_id) and verified against (action, args_sha256, script.rev).
        let args_sha256 = sha256_hex(&canonical_json_string(&args));
        match self
            .shared
            .store
            .get_idempotent_action(
                &self.deck_id,
                &caller_id,
                &request_id,
                &action,
                &args_sha256,
                script.rev,
            )
            .await
        {
            Ok(IdempotencyLookup::Hit(cached)) => return Ok(cached),
            Ok(IdempotencyLookup::Conflict(msg)) => {
                return Ok(err_resp(
                    InteractErrorCode::Conflict,
                    msg,
                    script.rev,
                    self.run_epoch,
                ));
            }
            Ok(IdempotencyLookup::Miss) | Err(_) => {}
        }

        // 5. Approval pause check.
        if self.shared.has_pending_approvals(&self.deck_id) {
            return Ok(err_resp(
                InteractErrorCode::WaitingApproval,
                "Deck is waiting for a pending tool approval".into(),
                script.rev,
                self.run_epoch,
            ));
        }

        // 6. Paused guard if action opts out.
        if self.state == RuntimeState::Paused && !decl.allow_while_paused {
            return Ok(err_resp(
                InteractErrorCode::Paused,
                format!("action {action:?} cannot run while stream is paused"),
                script.rev,
                self.run_epoch,
            ));
        }

        // 7. JSON Schema validation of `args`.
        if let Err(reason) = validate_action_args(decl, &args) {
            return Ok(err_resp(
                InteractErrorCode::InvalidArgs,
                reason,
                script.rev,
                self.run_epoch,
            ));
        }

        // 8. Ensure checkout is open on the SAME Deck checkout (never a second concurrent checkout!).
        if self.run_epoch == 0 {
            self.bump_epoch().await;
        }
        let turn_epoch = self.run_epoch;
        if let Err(err) = self.ensure_checkout(&script, turn_epoch).await {
            return Ok(err_resp(
                InteractErrorCode::ExecutionError,
                err,
                script.rev,
                self.run_epoch,
            ));
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

        // 10. Fence out late writes if Deck was deleted or script revision changed mid-action (229 H2).
        if self.is_fenced(turn_epoch, script.rev).await {
            return Ok(err_resp(
                InteractErrorCode::RevisionMismatch,
                "Deck was deleted or script revision changed while action was running".into(),
                script.rev,
                self.run_epoch,
            ));
        }

        let resp = match outcome {
            Ok(out) => {
                // Only explicit `deck_emit(...)` calls inside Python broadcast to shared subscribers!
                // The return value is strictly private to this caller.
                self.apply_turn_effects(&script, turn_epoch, out.effects, None, false)
                    .await;
                if decl.mutates_state
                    && let Some(c) = self.checkout.as_mut()
                    && let Ok(dump_sha) = c.checkpoint_heap().await
                    && !self.is_fenced(turn_epoch, script.rev).await
                {
                    let _ = self
                        .shared
                        .store
                        .save_checkpoint(
                            &self.deck_id,
                            script.rev,
                            &script.code_sha256,
                            None,
                            Some((script.rev, &script.revision_sha256, &dump_sha)),
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
            .save_idempotent_action(&self.deck_id, &caller_id, &args_sha256, &resp)
            .await;
        self.broadcast_status();
        Ok(resp)
    }
}

enum CmdFlow {
    Continue,
    Stop,
    Deleted,
}

fn checked_wake(delta: Duration) -> Option<Instant> {
    let clamped = delta.min(Duration::from_secs(MAX_INTERVAL_SECS));
    Instant::now().checked_add(clamped)
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
            if self.shared.is_deleted(&id) {
                continue;
            }
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
        let mut doc = self.read_doc(deck_id, by).await?;
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
        // Reconcile Yjs mirror `doc.script_ref` with authoritative SQLite state so callers never
        // observe stale active/pending revision metadata if a previous mirror write lagged or failed.
        let schedule = active_script.as_ref().and_then(|s| {
            s.schedule_cron
                .clone()
                .or_else(|| s.interval_secs.map(|secs| format!("every {secs}s")))
        });
        doc.script_ref = DeckScriptRef {
            active_rev: active_script.as_ref().map(|s| s.rev),
            pending_rev: pending_script.as_ref().map(|s| s.rev),
            mode: active_script.as_ref().map(|s| s.mode.as_str().to_owned()),
            schedule,
            updated_at: doc.script_ref.updated_at,
        };
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

/// Tear down runtime & storage state when a `DocKind::Deck` document is deleted (229 H2).
#[derive(Clone, Debug)]
pub struct DeckDeletedMsg {
    pub deck_id: String,
    pub title: String,
}

impl Message<DeckDeletedMsg> for DeckSupervisor {
    type Reply = ();

    async fn handle(
        &mut self,
        m: DeckDeletedMsg,
        _: &mut Context<Self, Self::Reply>,
    ) -> Self::Reply {
        self.cleanup_deleted_deck(&m.deck_id, m.title).await;
    }
}

/// Cold restart recovery (229 H3): repopulate `session_index` & seats for all live Decks,
/// clean up orphaned deleted Decks, and resume previously `stream_enabled`/`schedule_enabled` Decks.
#[derive(Clone, Debug)]
pub struct RecoverEnabledDecksMsg;

impl Message<RecoverEnabledDecksMsg> for DeckSupervisor {
    type Reply = Result<usize, String>;

    async fn handle(
        &mut self,
        _: RecoverEnabledDecksMsg,
        _: &mut Context<Self, Self::Reply>,
    ) -> Self::Reply {
        let records = self
            .shared
            .store
            .list_decks()
            .await
            .map_err(|e| format!("{e:#}"))?;
        let mut resumed = 0usize;
        for rec in records {
            if self.verify_deck_exists(&rec.deck_id).await.is_err() {
                tracing::info!(
                    deck = %rec.deck_id,
                    "skipping recovery of deleted Deck and cleaning up orphaned state"
                );
                self.cleanup_deleted_deck(&rec.deck_id, String::new()).await;
                continue;
            }
            // Index every live Deck session on boot so approval routing and Deck-session guards
            // work even before the first RPC or Start after restart.
            self.shared.register_session(&rec.deck_id, rec.session_id);
            if let Err(e) = self
                .configure_deck_session(&rec.deck_id, rec.session_id)
                .await
            {
                tracing::warn!(
                    deck = %rec.deck_id,
                    "failed to seat Deck session during boot recovery: {e}"
                );
            }

            if !(rec.stream_enabled || rec.schedule_enabled) || rec.active_rev.is_none() {
                continue;
            }
            match self.shared.store.get_active_script(&rec.deck_id).await {
                Ok(Some(script)) if script.status == ScriptStatus::Approved => {}
                Ok(_) => continue,
                Err(e) => {
                    tracing::warn!(
                        deck = %rec.deck_id,
                        "refusing to auto-resume Deck with invalid script digest: {e:#}"
                    );
                    continue;
                }
            }
            match self.ensure_owner(&rec.deck_id).await {
                Ok(tx) => {
                    let deck_id = rec.deck_id.clone();
                    tokio::spawn(async move {
                        let (rtx, rrx) = oneshot::channel();
                        if tx
                            .send(OwnerCmd::Control {
                                op: RuntimeControlOp::Start,
                                reply: rtx,
                            })
                            .await
                            .is_ok()
                            && let Ok(Err(err)) = rrx.await
                        {
                            tracing::warn!(deck = %deck_id, "boot recovery start failed: {err}");
                        }
                    });
                    resumed += 1;
                }
                Err(e) => {
                    tracing::warn!(deck = %rec.deck_id, "boot recovery owner spawn failed: {e}");
                }
            }
        }
        Ok(resumed)
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
            DeckCallReply::Section(s) => {
                let title = self.deck_title(&m.deck_id).await;
                self.publish_changed(&m.deck_id, title, false).await;
                Ok(s)
            }
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
            DeckCallReply::Section(s) => {
                let title = self.deck_title(&m.deck_id).await;
                self.publish_changed(&m.deck_id, title, false).await;
                Ok(s)
            }
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
            DeckCallReply::Order(o) => {
                let title = self.deck_title(&m.deck_id).await;
                self.publish_changed(&m.deck_id, title, false).await;
                Ok(o)
            }
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
            DeckCallReply::Section(s) => {
                let title = self.deck_title(&m.deck_id).await;
                self.publish_changed(&m.deck_id, title, false).await;
                Ok(s)
            }
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
        let title = self.deck_title(&m.deck_id).await;
        self.publish_changed(&m.deck_id, title, false).await;
        Ok(())
    }
}

#[derive(Clone, Debug)]
pub struct ProposeScriptMsg {
    pub deck_id: String,
    pub code: String,
    pub declared_actions: Vec<DeclaredAction>,
    pub triggers: Vec<TriggerSpec>,
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
        let doc = self.verify_deck_exists(&m.deck_id).await?;
        validate_script_proposal(&m.code, &m.declared_actions, &m.triggers, &m.change_summary)
            .map_err(|e| format!("{e:#}"))?;
        if let Some(expr) = m.schedule_cron.as_deref() {
            croner::Cron::from_str(expr)
                .map_err(|e| format!("invalid cron expression {expr:?}: {e}"))?;
        }
        for a in &m.declared_actions {
            jsonschema::validator_for(&a.schema)
                .map_err(|e| format!("invalid JSON schema for action {:?}: {e}", a.name))?;
        }
        verify_routines_exist(&m.triggers).await?;
        let rev = self
            .shared
            .store
            .propose_script(
                &m.deck_id,
                m.code,
                m.declared_actions,
                m.triggers,
                m.mode,
                m.interval_secs,
                m.schedule_cron,
                m.proposed_by,
                m.change_summary,
            )
            .await
            .map_err(|e| format!("{e:#}"))?;
        // Update live_cache pending_rev immediately without blocking on DeckOwner.
        if let Some(entry) = self.shared.live_cache.lock().get_mut(&m.deck_id) {
            entry.status.pending_rev = Some(rev.rev);
        }
        self.mirror_script_ref(&m.deck_id).await;
        if let Ok((st, _, _)) = self.get_runtime_snapshot(&m.deck_id).await {
            self.shared
                .hub
                .broadcast(&m.deck_id, DeckRuntimeEvent::Status { status: st });
        }
        self.publish_changed(&m.deck_id, doc.title, false).await;
        Ok(rev)
    }
}

#[derive(Clone, Debug)]
pub struct ApproveScriptMsg {
    pub deck_id: String,
    pub rev: u64,
    pub expected_digest: String,
    pub approved_by: String,
}

impl Message<ApproveScriptMsg> for DeckSupervisor {
    type Reply = Result<ScriptRevision, String>;

    async fn handle(
        &mut self,
        m: ApproveScriptMsg,
        _: &mut Context<Self, Self::Reply>,
    ) -> Self::Reply {
        let doc = self.verify_deck_exists(&m.deck_id).await?;
        // Re-validate trigger routine targets at approval time before mutating status.
        if let Some(candidate) = self
            .shared
            .store
            .get_script_revision(&m.deck_id, m.rev)
            .await
            .map_err(|e| format!("{e:#}"))?
            && !candidate.triggers.is_empty()
        {
            verify_routines_exist(&candidate.triggers).await?;
        }
        let script = self
            .shared
            .store
            .approve_script(&m.deck_id, m.rev, &m.expected_digest, &m.approved_by)
            .await
            .map_err(|e| format!("{e:#}"))?;
        let _ = self
            .shared
            .store
            .reset_trigger_states_for_deck(&m.deck_id)
            .await;
        self.mirror_script_ref(&m.deck_id).await;

        // Update live_cache immediately and notify DeckOwner non-blockingly (FIFO channel, 229 B4).
        if let Some(entry) = self.shared.live_cache.lock().get_mut(&m.deck_id) {
            entry.status.active_rev = Some(script.rev);
            if entry.status.pending_rev == Some(script.rev) {
                entry.status.pending_rev = None;
            }
            entry.live_frame = None;
        }
        if let Some(tx) = self.owners.get(&m.deck_id).cloned()
            && !tx.is_closed()
        {
            let cmd = OwnerCmd::RevisionApproved {
                script: script.clone(),
            };
            if let Err(mpsc::error::TrySendError::Full(cmd)) = tx.try_send(cmd) {
                tokio::spawn(async move {
                    let _ = tx.send(cmd).await;
                });
            }
        } else if let Ok((st, _, _)) = self.get_runtime_snapshot(&m.deck_id).await {
            self.shared
                .hub
                .broadcast(&m.deck_id, DeckRuntimeEvent::Status { status: st });
        }
        self.publish_changed(&m.deck_id, doc.title, false).await;
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
        let doc = self.verify_deck_exists(&m.deck_id).await?;
        let script = self
            .shared
            .store
            .reject_script(&m.deck_id, m.rev, m.reason)
            .await
            .map_err(|e| format!("{e:#}"))?;
        if let Some(entry) = self.shared.live_cache.lock().get_mut(&m.deck_id)
            && entry.status.pending_rev == Some(m.rev)
        {
            entry.status.pending_rev = None;
        }
        self.mirror_script_ref(&m.deck_id).await;
        if let Ok((st, _, _)) = self.get_runtime_snapshot(&m.deck_id).await {
            self.shared
                .hub
                .broadcast(&m.deck_id, DeckRuntimeEvent::Status { status: st });
        }
        self.publish_changed(&m.deck_id, doc.title, false).await;
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
        self.verify_deck_exists(&m.deck_id).await?;
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
    /// Whether mutating declared actions (`mutates_state: true`) are authorized for this caller.
    /// Live human-press RPC callers and the explicit `DeckInteractTool` agent path set `true`;
    /// non-human-press RPC callers (e.g. internal/forwarded) set `false` and may only invoke
    /// read-only declared actions.
    pub allow_mutating: bool,
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
        if self.shared.has_pending_approvals(&m.deck_id) {
            let (run_epoch, script_rev) = self
                .shared
                .live_cache
                .lock()
                .get(&m.deck_id)
                .map(|c| (c.status.run_epoch, c.status.active_rev.unwrap_or(0)))
                .unwrap_or((0, 0));
            tx_reply.send(Ok(InteractResponse {
                request_id,
                action: m.action,
                ok: false,
                result: None,
                error: Some("Deck is waiting for a pending tool approval".into()),
                error_code: Some(InteractErrorCode::WaitingApproval),
                private: true,
                cached: false,
                run_epoch,
                script_rev,
            }));
            return reply;
        }
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
            allow_mutating: m.allow_mutating,
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
                Ok(Ok(resp)) => resp,
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
        self.verify_deck_exists(&m.deck_id).await?;
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
        self.verify_deck_exists(&m.deck_id).await?;
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
        self.verify_deck_exists(&m.deck_id).await?;
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

/// Bus subscriber forwarding `ApprovalRequested`, `ApprovalResolved`, and `DocumentChanged`
/// (Deck deletion) to [`SharedDeckState`] and [`DeckSupervisor`].
pub struct DeckApprovalWatcher {
    shared: SharedDeckState,
    supervisor: Option<ActorRef<DeckSupervisor>>,
}

impl DeckApprovalWatcher {
    pub fn new(shared: SharedDeckState) -> Self {
        Self {
            shared,
            supervisor: None,
        }
    }

    pub fn with_supervisor(shared: SharedDeckState, supervisor: ActorRef<DeckSupervisor>) -> Self {
        Self {
            shared,
            supervisor: Some(supervisor),
        }
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

impl Message<DocumentChanged> for DeckApprovalWatcher {
    type Reply = ();

    async fn handle(
        &mut self,
        ev: DocumentChanged,
        _: &mut Context<Self, Self::Reply>,
    ) -> Self::Reply {
        if ev.kind == DocKind::Deck && ev.deleted {
            let deck_id = ev.doc.as_str().to_owned();
            // Immediately tombstone so any in-flight turn is fenced out even before supervisor runs.
            self.shared.mark_deleted(&deck_id);
            if let Some(ref sup) = self.supervisor {
                let _ = sup
                    .tell(DeckDeletedMsg {
                        deck_id,
                        title: ev.title,
                    })
                    .send()
                    .await;
            }
        }
    }
}

/// Helper to surface the head pending approval ID cleanly in tests and diagnostics.
pub fn pending_approval_id(shared: &SharedDeckState, deck_id: &str) -> Option<ApprovalId> {
    shared.head_pending_approval(deck_id).map(|p| p.approval_id)
}
