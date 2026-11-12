//! Ownership transfer (68 §4): **take** a session here, **release** one a
//! taker asked for, **demote** when an applied row shows another engine
//! took one of ours, and the orphan bookkeeping that follows.
//!
//! Everything here runs off the actor's mailbox (a take waits on the network
//! for up to `2 × release_wait_secs`); the actor hands over clones of what
//! is needed in [`Take`].

use std::sync::Arc;
use std::time::{Duration, Instant};

use agent::AgentEvent;
use agent::durability::{DurabilityConfig, ResumeCause, ResumeMode};
use agent::store::Store;
use agent::supervisor::transfer;
use anyhow::Context as _;
use futures::{SinkExt as _, StreamExt as _};
use kameo::actor::ActorRef;
use kameo::message::{Context, Message};
use replica::{ALPN_SESSIONS, PeerId, Transport};
use runtime::{Runtime, SessionId};

use crate::types::{Msg, schema_behind, schema_label};
use crate::{SessionSync, SessionsConfig, TakeReason};

/// How often a taker re-reads its cursor while waiting for the owner's tail.
const CURSOR_POLL: Duration = Duration::from_millis(100);

/// 183 §5.6: how long a session another engine created at this engine's
/// request stays routed to that engine by id after its last use (a call for
/// it, a relayed event of it): the chat keeps reading from its creator while
/// in use, so a lagging replica never answers for it.
pub const EXPECT_FOR: Duration = Duration::from_secs(120);

/// Most chats remembered at once; the least recently used goes first.
const EXPECT_MAX: usize = 256;

/// Why a contact's inbox never changes machines: its contact limits live in
/// this engine's `a2a_contexts` only, so elsewhere it would run unlimited.
pub const INBOX_STAYS: &str = "This chat is a contact's inbox, so it stays on this machine.";

/// Session → (the engine that created it at our request, last use).
static EXPECTED: std::sync::LazyLock<
    parking_lot::Mutex<std::collections::HashMap<SessionId, (PeerId, Instant)>>,
> = std::sync::LazyLock::new(Default::default);

/// 183 §5.6: `engine` just created `session` for a caller of this engine.
/// While [`expected`] names it, the RPC front door forwards calls for
/// `session` there and the event relay follows it before the row lands.
/// Entries unused for [`EXPECT_FOR`] are dropped; at most [`EXPECT_MAX`].
pub fn expect(session: SessionId, engine: PeerId) {
    let mut map = EXPECTED.lock();
    map.retain(|_, (_, at)| at.elapsed() < EXPECT_FOR);
    if map.len() >= EXPECT_MAX
        && let Some(oldest) = map.iter().min_by_key(|(_, (_, at))| *at).map(|(s, _)| *s)
    {
        map.remove(&oldest);
    }
    map.insert(session, (engine, Instant::now()));
}

/// The engine [`expect`]ed to hold `session`, renewing the entry; `None`
/// when there is none or it expired.
pub fn expected(session: SessionId) -> Option<PeerId> {
    let mut map = EXPECTED.lock();
    match map.get_mut(&session) {
        Some((engine, at)) if at.elapsed() < EXPECT_FOR => {
            *at = Instant::now();
            Some(engine.clone())
        }
        Some(_) => {
            map.remove(&session);
            None
        }
        None => None,
    }
}

/// `tell` (from the apply path): an applied row raised the epoch of a
/// session this engine owned — `by` took it. 68 §4.3.
#[derive(Clone, Debug)]
pub struct Demote {
    /// The session.
    pub session: SessionId,
    /// The epoch we owned it in.
    pub old_epoch: u32,
    /// The new owner (origin of the row).
    pub by: PeerId,
}

/// A take's inputs, cloned off the actor (68 §4.2).
pub struct Take {
    /// Publishes `SessionOwnerChanged`.
    pub rt: Runtime,
    /// Timeouts.
    pub config: SessionsConfig,
    /// The durable store.
    pub store: Store,
    /// `None` before the mesh is up: no owner is reachable.
    pub transport: Option<Arc<dyn Transport>>,
    /// Peers with a connection right now.
    pub connected: Vec<PeerId>,
    /// The mesh's view of every peer (`last_seen_ms`); empty without a mesh.
    pub peers: Vec<mesh::Peer>,
    /// 139 Q10: a `Write` take for a client's write here counts as the
    /// user's for the resume policy (`TakeSession::by_user`).
    pub by_user: bool,
    /// 181: the engine that asked for this take over the mesh (`Forwarded`);
    /// `None` for a local client. `Moved` binds to it: only the owner moves.
    pub caller: Option<PeerId>,
}

/// 139 §6.2 step 9: what the taker did about the taken open turn.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum ResumeOnTake {
    /// No turn was open on our copy.
    NoOpenTurn,
    /// `ResumeTurn{cause}` sent to the supervisor (resumes now).
    Resume(ResumeCause),
    /// `ResumePending` published: the Attention item; the user resumes.
    Pending,
    /// `durability.resume = off`: the next input resumes lazily.
    Nothing,
}

/// 139 §6.3 policy table (D5, Revision 1 Q10): a transfer follows
/// `durability.resume`; a takeover resumes only when the user asked for it
/// — `User`, or a `Write` take the front door made for a client's input
/// (`by_user`); system takes (`Write` from plumbing, `Orphan`) leave the item.
/// 181 D3: a move is the user's ask to run it there, so it resumes whatever
/// `durability.resume` says.
fn resume_policy(
    transferred: bool,
    reason: TakeReason,
    by_user: bool,
    mode: ResumeMode,
) -> ResumeOnTake {
    if transferred {
        match mode {
            _ if reason == TakeReason::Moved => ResumeOnTake::Resume(ResumeCause::Transfer),
            ResumeMode::Auto => ResumeOnTake::Resume(ResumeCause::Transfer),
            ResumeMode::Ask => ResumeOnTake::Pending,
            ResumeMode::Off => ResumeOnTake::Nothing,
        }
    } else if reason == TakeReason::User || (reason == TakeReason::Write && by_user) {
        ResumeOnTake::Resume(ResumeCause::Takeover)
    } else {
        ResumeOnTake::Pending
    }
}

impl Take {
    /// Take `id` here; returns the new `(owner, epoch)`. A session already
    /// ours (or unowned, lone engine) is a no-op. The taken open turn, if
    /// any, is resumed or left for the user per [`resume_policy`].
    pub async fn run(self, id: SessionId, reason: TakeReason) -> Result<(String, u32), String> {
        self.take(id, reason).await.map(|(owner, _)| owner)
    }

    /// [`Take::run`] reporting what step 9 did.
    pub(crate) async fn take(
        self,
        id: SessionId,
        reason: TakeReason,
    ) -> Result<((String, u32), ResumeOnTake), String> {
        let me = self.store.engine().to_owned();
        let (epoch, owner) = self
            .store
            .fence(id)
            .await
            .map_err(|e| e.to_string())?
            .ok_or_else(|| format!("{id}: no such session"))?;
        if owner.is_empty() || owner == me {
            return Ok(((owner, epoch), ResumeOnTake::NoOpenTurn));
        }
        // 181: `moved` is the owner's own ask; from anyone else it is the user's take.
        let reason =
            if reason == TakeReason::Moved && self.caller.as_ref().map(|c| &c.0) != Some(&owner) {
                TakeReason::User
            } else {
                reason
            };
        if me.is_empty() {
            return Err("this engine has no mesh identity".to_owned());
        }
        let peer = PeerId(owner.clone());
        let mut transferred = false;
        if self.connected.contains(&peer) {
            match self.release_from(&peer, id).await {
                Ok(None) => transferred = true,
                // The owner keeps it (a contact's inbox): no reason takes it, nothing changed.
                Ok(Some(why)) => return Err(why),
                // The user chose to continue here now: A's tail becomes A's orphans (§4.3).
                Err(e) if reason == TakeReason::User => {
                    tracing::warn!("sessions: take {id} from {owner} without its tail: {e:#}");
                }
                Err(e) => return Err(format!("owner {owner} busy: {e:#}")),
            }
        } else if reason == TakeReason::Moved {
            // 181 D2: a move is a handover, never a takeover.
            return Err(format!("owner {owner} is not connected here"));
        } else if reason == TakeReason::Write {
            // inbox sessions: the take of an unreachable owner stays open until design 191 P1 (contact_inboxes)
            self.unseen_long_enough(&owner)?;
        }
        // Re-read: a concurrent take moved the fence meanwhile.
        match self.store.fence(id).await.map_err(|e| e.to_string())? {
            Some((e, o)) if e == epoch && o == owner => {}
            other => {
                return Err(format!(
                    "{id}: ownership moved to {other:?} during the take"
                ));
            }
        }
        // 139 D19: no taker-side `unload` — after D1 no actor exists here
        // for a session this engine does not own (I1).
        let next = epoch + 1;
        self.store
            .set_owner(id, &me, next)
            .await
            .map_err(|e| format!("{e:#}"))?;
        self.store
            .set_fence(id, next, &me)
            .await
            .map_err(|e| format!("{e:#}"))?;
        self.rt
            .publish(AgentEvent::SessionOwnerChanged {
                session: id,
                owner: me.clone(),
                epoch: next,
                here: true,
                moved_from: (reason == TakeReason::Moved).then(|| owner.clone()),
            })
            .await;
        // 139 R2 D4: the session's reminder rows (replicated here) fire here now.
        if let Ok(Some(agents)) = ActorRef::<agent::Agents>::lookup(agent::AGENT_ACTOR) {
            let _ = agents
                .tell(agent::supervisor::RearmReminders { session: id })
                .await;
        }
        let from = self.peer_name(&owner);
        self.record_move(id, &owner, &from, transferred, reason)
            .await;
        let resumed = self
            .resume_on_take(id, &owner, &from, transferred, reason)
            .await;
        Ok(((me, next), resumed))
    }

    /// An engine's mesh name; the first 8 chars of its id when unknown.
    fn peer_name(&self, engine: &str) -> String {
        self.peers
            .iter()
            .find(|p| p.id.0 == engine)
            .map_or_else(|| engine.chars().take(8).collect(), |p| p.name.clone())
    }

    /// 183 §5.3 (W-8): the one Moved row of this take, written before the
    /// turn resumes. In context only without an open turn; an open turn
    /// reads the extended `<engine_restart/>` instead (§11.2), and a
    /// `Pending` one replaces the old local note (the row says so, `pending`).
    async fn record_move(
        &self,
        id: SessionId,
        owner: &str,
        from: &str,
        transferred: bool,
        reason: TakeReason,
    ) {
        let open = matches!(self.store.open_turn(id).await, Ok(Some(_)));
        let pending = open
            && resume_policy(transferred, reason, self.by_user, self.durability().resume)
                == ResumeOnTake::Pending;
        let m = platform::process::machine();
        let row = self.store.session(id).await.ok().flatten();
        let landing = row
            .as_ref()
            .and_then(|r| r.cwd.as_deref())
            .map(|c| tools::seat::land(id, std::path::Path::new(c)));
        let (cwd, cwd_was) = match landing {
            Some(l) => (l.cwd, l.was),
            None => (std::env::current_dir().unwrap_or_default(), None),
        };
        let python = self.store.ran_python(id).await.unwrap_or(false);
        let (approvals, communication) = self
            .store
            .session_settings(id)
            .await
            .ok()
            .flatten()
            .unwrap_or((None, None));
        let ask = approvals.unwrap_or_else(|| {
            self.rt
                .section::<tools::ToolsConfig>("tools")
                .map_or(true, |c| c.approvals)
        });
        let jobs_left = self.store.jobs_of(id).await.map_or(0, |jobs| {
            jobs.iter()
                .filter(|j| j.state == tools::JobState::Running && j.engine == owner)
                .count() as u32
        });
        let moved = agent::SessionMoved {
            to: m.name.clone(),
            from: from.to_owned(),
            to_id: self.store.engine().to_owned(),
            from_id: owner.to_owned(),
            os: m.os.to_owned(),
            os_name: m.os_name.clone(),
            arch: m.arch.to_owned(),
            host: m.host.clone(),
            cwd: cwd.to_string_lossy().into_owned(),
            cwd_missing: cwd_was.is_some(),
            cwd_was: cwd_was.map(|p| p.to_string_lossy().into_owned()),
            at: chrono::Utc::now(),
            reason: match reason {
                TakeReason::User => "user",
                TakeReason::Moved => "moved",
                TakeReason::Orphan => "orphan",
                TakeReason::Write => "write",
            }
            .to_owned(),
            jobs_left,
            runtime: if python { "reset" } else { "none" }.to_owned(),
            approvals: if ask { "ask" } else { "off" }.to_owned(),
            messaging: communication != Some(false),
            pending,
        };
        // 183 R1 (S5): `append_moved` picks `in_context` inside its own
        // transaction, so an input accepted since the `open` read above
        // cannot land a context-visible row mid-turn.
        match self
            .store
            .append_moved(id, &moved.model_text(), &moved)
            .await
        {
            Ok(_) => {
                if let Ok(revision) = self.store.context_revision(id).await {
                    self.rt
                        .publish(AgentEvent::ContextChanged {
                            session: id,
                            revision,
                            reason: "moved".to_owned(),
                        })
                        .await;
                }
            }
            Err(e) => tracing::warn!("sessions: take {id}: moved row: {e:#}"),
        }
    }

    /// 139 §6.2 step 9 / 9′: the open turn on our copy, after `set_owner`,
    /// by the policy table. `ResumeTurn` is a `tell` so the take's reply is
    /// not delayed by the resume.
    async fn resume_on_take(
        &self,
        id: SessionId,
        from_id: &str,
        from: &str,
        transferred: bool,
        reason: TakeReason,
    ) -> ResumeOnTake {
        let turn = match self.store.open_turn(id).await {
            Ok(Some((turn, _))) => turn,
            Ok(None) => return ResumeOnTake::NoOpenTurn,
            Err(e) => {
                tracing::warn!("sessions: take {id}: open_turn: {e:#}");
                return ResumeOnTake::NoOpenTurn;
            }
        };
        let action = resume_policy(transferred, reason, self.by_user, self.durability().resume);
        match &action {
            ResumeOnTake::Resume(cause) => {
                match ActorRef::<agent::Agents>::lookup(agent::AGENT_ACTOR) {
                    Ok(Some(agents)) => {
                        let _ = agents
                            .tell(agent::supervisor::ResumeTurn {
                                session: id,
                                turn,
                                cause: Some(*cause),
                                from: Some(from.to_owned()),
                            })
                            .await;
                    }
                    _ => tracing::warn!(
                        "sessions: take {id}: no supervisor; turn {turn} resumes on the next start"
                    ),
                }
            }
            ResumeOnTake::Pending => {
                if !transferred {
                    // The user's Resume press is a `ResumeTurn` with no cause:
                    // the supervisor must know it is this takeover (139 §6.3).
                    if let Ok(Some(agents)) = ActorRef::<agent::Agents>::lookup(agent::AGENT_ACTOR)
                    {
                        let _ = agents
                            .tell(agent::supervisor::TakeoverPending {
                                session: id,
                                from: from_id.to_owned(),
                            })
                            .await;
                    }
                }
                self.rt
                    .publish(AgentEvent::ResumePending {
                        session: id,
                        turn,
                        from: from_id.to_owned(),
                    })
                    .await;
            }
            ResumeOnTake::Nothing | ResumeOnTake::NoOpenTurn => {}
        }
        tracing::info!("sessions: take {id} from {from}: open turn {turn} → {action:?}");
        action
    }

    /// `durability:` as the agent reads it; defaults when absent.
    fn durability(&self) -> DurabilityConfig {
        self.rt
            .section::<DurabilityConfig>("durability")
            .unwrap_or_default()
    }

    /// `Release` → `Released{last_seq}` over `theuth/sessions/1`, then wait
    /// until our cursor for the owner reached `last_seq`. The reply is
    /// bounded by `release_wait_secs + durability.drain_secs` (139 Q4: the
    /// owner drains for `release_wait_secs − 1` and then flushes), the
    /// cursor by `release_wait_secs`. `Some(why)`: the owner answered
    /// `Refused` and keeps the session.
    async fn release_from(&self, peer: &PeerId, id: SessionId) -> anyhow::Result<Option<String>> {
        let t = self.transport.as_ref().context("no transport")?;
        let wait = Duration::from_secs(self.config.release_wait_secs);
        let reply_wait = wait + Duration::from_secs(self.durability().drain_secs);
        let mut framed = crate::events::framed(t.open(peer, ALPN_SESSIONS).await?);
        framed
            .send(
                Msg::Release {
                    session: id.to_string(),
                    schema: Some(agent::store::SESSIONS_WIRE_SCHEMA),
                }
                .encode()?,
            )
            .await?;
        let frame = tokio::time::timeout(reply_wait, framed.next())
            .await
            .context("release timed out")?
            .context("owner closed the stream")??;
        let last_seq = match Msg::decode(&frame)? {
            Msg::Released { last_seq, .. } => i64::try_from(last_seq)?,
            Msg::Refused { why, .. } => return Ok(Some(why)),
            other => anyhow::bail!("unexpected reply {other:?}"),
        };
        let deadline = Instant::now() + wait;
        loop {
            let applied = self
                .store
                .cursors()
                .await?
                .into_iter()
                .find(|c| c.origin == peer.0 && c.peer.is_empty())
                .map_or(0, |c| c.seq);
            if applied >= last_seq {
                return Ok(None);
            }
            anyhow::ensure!(
                Instant::now() < deadline,
                "tail not replicated: cursor {applied} < {last_seq}"
            );
            tokio::time::sleep(CURSOR_POLL).await;
        }
    }

    /// 68 §4.2 (c): an unreachable owner must have been unseen for
    /// `takeover_secs`. Unknown to the mesh = never seen = long enough.
    fn unseen_long_enough(&self, owner: &str) -> Result<(), String> {
        let Some(p) = self.peers.iter().find(|p| p.id.0 == owner) else {
            return Ok(());
        };
        if p.reachable {
            return Err(format!(
                "owner {owner} is reachable but has no sessions stream yet; retry"
            ));
        }
        let ago = now_ms().saturating_sub(p.last_seen_ms);
        let need = self.config.takeover_secs * 1000;
        if ago < need {
            return Err(format!(
                "owner {owner} was seen {}s ago; takeover after {}s",
                ago / 1000,
                self.config.takeover_secs
            ));
        }
        Ok(())
    }
}

/// 181 D2: why a move's `ask` (the target's `session_take(moved)`) did not
/// answer with the new owner.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum MoveFailure {
    /// The target answered with an error (its words).
    Refused(String),
    /// No answer within the take's worst case: the move may still land.
    NoAnswer,
}

/// 181 D2, the origin side of "Move to ▸": `id` must be ours, not a desk, an
/// ACP harness, a contact's inbox or a paused agent's tree (`held`: `HeldDesks`,
/// the per-engine pause map the target would not know), and `to` a connected peer; then
/// `ask` — the target's `session_take(id, "moved")`, sent over the mesh by
/// the caller — runs the ordinary take there, which releases (drains) the
/// session here and lands the new epoch. `ask` answers `Ok(None)` when it
/// could not reach `to`. A refusal leaves the session here: the drained turn
/// resumes here ([`continue_here`]). `name` is `to`'s display name.
pub async fn move_to(
    store: &Store,
    id: SessionId,
    to: &PeerId,
    name: &str,
    connected: &[PeerId],
    held: &[(SessionId, Option<String>)],
    ask: impl Future<Output = Result<Option<(String, u32)>, MoveFailure>>,
) -> Result<(String, u32), String> {
    let me = store.engine();
    match store.fence(id).await.map_err(|e| format!("{e:#}"))? {
        Some((_, owner)) if !me.is_empty() && owner == me => {}
        Some(_) => return Err("this session is not owned here".to_owned()),
        None => return Err(format!("{id}: no such session")),
    }
    if to.0 == me {
        return Err("this session already runs here".to_owned());
    }
    // A paused agent's tree (179): its turns are held here and would resume there.
    let root = store.root_of(id).await.unwrap_or(id);
    if let Some((_, agent)) = held.iter().find(|(desk, _)| *desk == id || *desk == root) {
        let agent = agent.as_deref().unwrap_or("the agent");
        return Err(format!("{agent} is paused — un-pause it before moving"));
    }
    let row = store
        .session(id)
        .await
        .map_err(|e| format!("{e:#}"))?
        .ok_or_else(|| format!("{id}: no such session"))?;
    // The front door never takes these either (`forward::classify`): their
    // process or roster state lives on this engine.
    if let Some(harness) = &row.agent {
        return Err(format!(
            "this session runs the {harness} harness as a process on this engine, so it cannot move"
        ));
    }
    if let Some(agent) = &row.desk {
        return Err(format!(
            "this is {agent}'s desk: a desk stays on the engine that runs its agent, so it cannot move"
        ));
    }
    // 191 (D4): a contact's inbox moves only when it is marked (its marker set, readable and
    // naming a live export by id): the marker travels with the row, so its taker runs it capped
    // until a contact message adopts it. Any other inbox keeps its limits here only.
    if store
        .a2a_inbox_identity(id)
        .await
        .map_err(|e| format!("{e:#}"))?
        .is_some()
        && !store.inbox_marked(id).await.map_err(|e| format!("{e:#}"))?
    {
        return Err(INBOX_STAYS.to_owned());
    }
    if !connected.contains(to) {
        return Err(format!("{name} is offline"));
    }
    match ask.await {
        Ok(Some(owned)) => Ok(owned),
        Ok(None) => Err(format!("{name} is offline")),
        Err(MoveFailure::NoAnswer) => Err(format!(
            "no answer from {name} yet — the move may still land"
        )),
        Err(MoveFailure::Refused(why)) => {
            let ours = matches!(store.fence(id).await, Ok(Some((_, o))) if o == me);
            if !ours {
                return Err(format!("{name} refused: {why}"));
            }
            continue_here(store, id).await;
            Err(format!(
                "Move to {name} failed: {name} refused: {why}. It continues here."
            ))
        }
    }
}

/// 181: a move the target refused after our release drain — the session is
/// still ours: stop refusing inputs (D16 hold) and resume the turn the drain
/// suspended (`suspended{release}` newest), here. Returns that turn.
/// ponytail: a target that failed *after* its `set_owner` (a fence write
/// error) would leave two owners at e+1; the take writes nothing after it
/// that can fail but `set_fence`.
pub(crate) async fn continue_here(store: &Store, id: SessionId) -> Option<runtime::TurnId> {
    let agents = ActorRef::<agent::Agents>::lookup(agent::AGENT_ACTOR)
        .ok()
        .flatten();
    if let Some(a) = &agents {
        let _ = a.tell(transfer::ClearDraining(id)).await;
    }
    let (turn, _) = store.open_turn(id).await.ok().flatten()?;
    let plan = store.resume_plan(id, turn).await.ok()?;
    if plan.kind != agent::durability::ResumeKind::Suspended || plan.cause != ResumeCause::Transfer
    {
        return None;
    }
    match agents {
        Some(a) => {
            // A planned local stop that ended: the boot causes' no-op transfer pass.
            let _ = a
                .tell(agent::supervisor::ResumeTurn {
                    session: id,
                    turn,
                    cause: Some(ResumeCause::Shutdown),
                    from: None,
                })
                .await;
        }
        None => {
            tracing::warn!("sessions: move of {id} refused; no supervisor to resume turn {turn}")
        }
    }
    tracing::info!("sessions: move of {id} refused; turn {turn} continues here");
    Some(turn)
}

/// Owner side of a take (68 §4.2 step 1; 139 §6.1 D3): drain the session —
/// its open turn is `turn_finished` or `suspended{release}` before the
/// reply — flush the write connection, answer with `MAX(seq)` of the
/// session's log rows. The session stays readable here; the taker's row
/// flips `owner` when it lands. Inputs are refused meanwhile (D16).
pub async fn release(store: &Store, id: SessionId) -> anyhow::Result<u64> {
    drain_session(store, id).await;
    forget_prompts(id).await;
    store.flush().await?;
    Ok(u64::try_from(store.session_log_seq(id).await?).unwrap_or(0))
}

/// 183 R1 (H1): the drained turn's approvals and asks stop waiting here —
/// their cards resolve on every client (a reload no longer lists them) — and
/// their `questions` rows stay `pending`, so the taker's resume asks again.
async fn forget_prompts(id: SessionId) {
    if let Ok(Some(a)) = kameo::actor::ActorRef::<tools::Approval>::lookup(tools::APPROVAL_ACTOR) {
        let _ = a.ask(tools::ForgetSession(id)).await;
    }
    if let Ok(Some(a)) = kameo::actor::ActorRef::<tools::AskUser>::lookup(tools::ASK_ACTOR) {
        let _ = a.ask(tools::ForgetSession(id)).await;
    }
}

/// Answer one `Release` frame on an accepted stream.
///
/// 183 R1 (E-M3): a taker on an older agent schema (`schema` below ours, or
/// absent: a pre-183 taker) is refused by closing the stream, before the
/// session is drained. It could not apply the rows we hold back from it
/// ([`crate::sync`]), so taking would fork the session from a stale copy.
/// A pre-183 taker reads the closed stream as "owner closed the stream":
/// `Take::take` fails a `moved`, `write` or `orphan` take with "owner busy"
/// and takes nothing; only a `user` take ("continue here", §4.3) goes on
/// without our tail, as it always does when the release fails.
pub(crate) async fn answer_release(
    store: &Store,
    taker: &PeerId,
    schema: Option<u32>,
    session: String,
    framed: &mut crate::sync::Stream,
) -> anyhow::Result<()> {
    let ours = agent::store::SESSIONS_WIRE_SCHEMA;
    if schema_behind(schema, ours) {
        tracing::warn!(
            "sessions: refusing release of {session} to {taker}: its agent schema {} < ours {ours}; \
             it would miss rows it cannot apply",
            schema_label(schema)
        );
        return Ok(());
    }
    let id: SessionId = session.parse()?;
    // A contact's inbox keeps its limits here only, unless it is marked (191, D4): refuse
    // before anything drains.
    let refused = match store.a2a_inbox_identity(id).await {
        Ok(None) => None,
        Ok(Some(_)) => match store.inbox_marked(id).await {
            Ok(true) => None,
            Ok(false) => Some(INBOX_STAYS.to_owned()),
            Err(e) => Some(format!(
                "could not tell whether this contact's inbox is marked, so it stays here: {e:#}"
            )),
        },
        Err(e) => Some(format!(
            "could not tell whether this chat is a contact's inbox, so it stays here: {e:#}"
        )),
    };
    if let Some(why) = refused {
        tracing::info!("sessions: refusing release of {session} to {taker}: {why}");
        framed.send(Msg::Refused { session, why }.encode()?).await?;
        return Ok(());
    }
    let last_seq = release(store, id).await?;
    framed
        .send(Msg::Released { session, last_seq }.encode()?)
        .await?;
    Ok(())
}

/// 68 §4.3: we were the owner in `old_epoch`; `by` now owns it. Stop our
/// actor, orphan our unshipped rows of that epoch (those above what `by`
/// had applied of our log), raise the fence, tell the UI, and leave a
/// local-only note in the transcript. Returns how many rows were orphaned.
/// 139 D16: the taker's row landed, so the release hold on inputs ends.
pub async fn demote(rt: &Runtime, store: &Store, m: Demote) -> anyhow::Result<u64> {
    unload(m.session).await;
    transfer::demoted(m.session).await;
    let me = store.engine();
    let shipped = store
        .cursors()
        .await?
        .into_iter()
        .find(|c| c.origin == me && c.peer == m.by.0)
        .map_or(0, |c| c.seq);
    let n = store
        .log_orphan_after(m.session, m.old_epoch, shipped)
        .await?;
    let (owner, epoch) = store
        .owner(m.session)
        .await?
        .unwrap_or_else(|| (m.by.0.clone(), m.old_epoch + 1));
    store.set_fence(m.session, epoch, &owner).await?;
    rt.publish(AgentEvent::SessionOwnerChanged {
        session: m.session,
        owner,
        epoch,
        here: false,
        moved_from: None,
    })
    .await;
    if n > 0 {
        let from = store
            .log_orphans(m.session)
            .await?
            .into_iter()
            .find(|r| r.epoch == i64::from(m.old_epoch))
            .and_then(|r| replica::Hlc::parse(&r.hlc))
            .and_then(|h| chrono::DateTime::from_timestamp_millis(i64::try_from(h.wall_ms).ok()?))
            .map_or_else(|| "an unknown time".to_owned(), |t| t.to_rfc3339());
        // ponytail: the new owner's next message lands on this seq and
        // replaces the note (owner wins); the orphans stay in `session_orphans`.
        store
            .append_local_note(
                m.session,
                &format!(
                    "{n} turns from {from} were not delivered on {me}; `session_orphans` to view"
                ),
            )
            .await?;
    }
    Ok(n)
}

/// Called after a row of `origin` was applied: if it raised the epoch of a
/// session we owned, the actor demotes (a `tell`, so the apply path never
/// waits on it).
pub(crate) async fn on_applied(
    store: &Store,
    origin: &PeerId,
    session: SessionId,
    before: Option<(u32, String)>,
    epoch: u32,
) {
    let Some((old, owner)) = before else { return };
    if epoch <= old || owner.is_empty() || owner != store.engine() {
        return;
    }
    let m = Demote {
        session,
        old_epoch: old,
        by: origin.clone(),
    };
    match crate::actor() {
        Some(a) => {
            let _ = a.tell(m).await;
        }
        None => {
            tracing::warn!("sessions: {session} taken by {origin} but no sessions actor to demote")
        }
    }
}

/// Stop the live actor of `id`, if the agent supervisor is running
/// (`UnloadSession`, 139 D19: the cancel lands before the stop).
async fn unload(id: SessionId) {
    if let Ok(Some(agents)) = ActorRef::<agent::Agents>::lookup(agent::AGENT_ACTOR) {
        let _ = agents.ask(agent::UnloadSession { session: id }).await;
    }
}

/// 139 §6.1: the owner side of a take drains `id` — `suspended{release}`
/// first, in-flight calls until the window closes, jobs, then the actor —
/// through the supervisor; without one (a pairing run, these tests) the
/// store-only step 1 still leaves the resumable row (I3).
async fn drain_session(store: &Store, id: SessionId) {
    let deadline = Instant::now() + transfer::release_window(release_wait_secs().await);
    match ActorRef::<agent::Agents>::lookup(agent::AGENT_ACTOR) {
        Ok(Some(agents)) => {
            if let Err(e) = agents
                .ask(agent::supervisor::DrainSession {
                    session: id,
                    reason: transfer::RELEASE,
                    deadline,
                })
                .await
            {
                tracing::warn!("sessions: release {id}: drain: {e}");
            }
        }
        _ => {
            transfer::suspend_open_turn(store, id, transfer::RELEASE).await;
        }
    }
}

/// `ask` → `sessions.release_wait_secs`: the owner's own window for a
/// release (139 Q4; `Msg::Release` carries none). `release` runs off the
/// actor's mailbox (an accepted stream), so asking it is safe there.
#[derive(Clone, Copy, Debug, Default)]
pub struct ReleaseWaitSecs;

impl Message<ReleaseWaitSecs> for SessionSync {
    type Reply = u64;

    async fn handle(&mut self, _: ReleaseWaitSecs, _: &mut Context<Self, Self::Reply>) -> u64 {
        self.config.release_wait_secs
    }
}

async fn release_wait_secs() -> u64 {
    match crate::actor() {
        Some(a) => a
            .ask(ReleaseWaitSecs)
            .await
            .unwrap_or(SessionsConfig::default().release_wait_secs),
        None => SessionsConfig::default().release_wait_secs,
    }
}

/// Hourly until shutdown: delete orphaned rows older than `keep_days`
/// (`sessions.orphan_keep_days`, 68 §4.3). The demoted owner's note stays.
pub async fn sweep_orphans(
    store: Store,
    keep_days: u64,
    shutdown: tokio_util::sync::CancellationToken,
) {
    loop {
        let before = now_ms().saturating_sub(keep_days * 86_400_000);
        match store.prune_orphans_before(before).await {
            Ok(n) if n > 0 => {
                tracing::info!("sessions: pruned {n} orphaned log rows older than {keep_days} days")
            }
            Ok(_) => {}
            Err(e) => tracing::warn!("sessions: orphan sweep: {e:#}"),
        }
        tokio::select! {
            () = shutdown.cancelled() => return,
            () = tokio::time::sleep(Duration::from_secs(3600)) => {}
        }
    }
}

fn now_ms() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0, |d| d.as_millis() as u64)
}

#[cfg(test)]
mod tests {
    use std::collections::HashMap;
    use std::path::PathBuf;

    use replica::{ByteStream, PeerEvent};
    use tokio::sync::{broadcast, mpsc};

    use super::*;
    use crate::apply::{Applied, apply_log_row};
    use agent::durability::{ResumeKind, StepKind};

    // ---- the record replica's loopback transport, cut to what a take needs ----

    type Inbound = (PeerId, Box<dyn ByteStream>);

    /// Every `Loop`'s accept queue; everyone is linked to everyone.
    #[derive(Default)]
    struct Switch(parking_lot::Mutex<HashMap<PeerId, mpsc::UnboundedSender<Inbound>>>);

    struct Loop {
        me: PeerId,
        switch: Arc<Switch>,
        inbox: tokio::sync::Mutex<mpsc::UnboundedReceiver<Inbound>>,
        events: broadcast::Sender<PeerEvent>,
    }

    impl Loop {
        fn new(name: &str, switch: &Arc<Switch>) -> Arc<Self> {
            let me = PeerId(name.to_owned());
            let (tx, rx) = mpsc::unbounded_channel();
            switch.0.lock().insert(me.clone(), tx);
            Arc::new(Self {
                me,
                switch: switch.clone(),
                inbox: tokio::sync::Mutex::new(rx),
                events: broadcast::channel(8).0,
            })
        }
    }

    #[async_trait::async_trait]
    impl Transport for Loop {
        fn peers(&self) -> Vec<PeerId> {
            self.switch
                .0
                .lock()
                .keys()
                .filter(|p| **p != self.me)
                .cloned()
                .collect()
        }
        async fn open(&self, peer: &PeerId, _alpn: &str) -> anyhow::Result<Box<dyn ByteStream>> {
            let (mine, theirs) = tokio::io::duplex(1 << 20);
            let inboxes = self.switch.0.lock();
            let to = inboxes
                .get(peer)
                .ok_or_else(|| anyhow::anyhow!("no such peer"))?;
            to.send((self.me.clone(), Box::new(theirs)))
                .map_err(|_| anyhow::anyhow!("peer gone"))?;
            Ok(Box::new(mine))
        }
        async fn accept(&self, _alpn: &str) -> anyhow::Result<Inbound> {
            self.inbox
                .lock()
                .await
                .recv()
                .await
                .ok_or_else(|| anyhow::anyhow!("switch closed"))
        }
        fn subscribe(&self) -> broadcast::Receiver<PeerEvent> {
            self.events.subscribe()
        }
    }

    // ---- two engines, one session, hand-driven shipping ----

    struct Engine {
        store: Store,
        rt: Runtime,
        dir: PathBuf,
    }

    async fn engine(name: &str) -> Engine {
        let dir =
            std::env::temp_dir().join(format!("theuth-owner-{name}-{}", uuid::Uuid::now_v7()));
        std::fs::create_dir_all(&dir).unwrap();
        let store = Store::open(
            &dir.join("sessions.sqlite"),
            Some(mesh::EngineId(name.into())),
        )
        .await
        .unwrap();
        let rt = Runtime::new(figment::Figment::new(), dir.clone());
        Engine { store, rt, dir }
    }

    impl Engine {
        fn id(&self) -> PeerId {
            PeerId(self.store.engine().to_owned())
        }

        /// Apply every row of `from` this engine has not applied yet (what
        /// L's shipper does), and record on `from` how far we got.
        async fn pull(&self, from: &Engine) -> Vec<Applied> {
            let origin = from.id();
            let mut out = Vec::new();
            for row in from.store.log_after(0).await.unwrap() {
                let seq = row.seq;
                let r = apply_log_row(&self.store, &origin, &row.into())
                    .await
                    .unwrap();
                if r != Applied::Duplicate {
                    from.store
                        .set_cursor(&origin.0, self.store.engine(), seq)
                        .await
                        .unwrap();
                }
                out.push(r);
            }
            out
        }

        fn take(
            &self,
            transport: Option<Arc<dyn Transport>>,
            connected: Vec<PeerId>,
            peers: Vec<mesh::Peer>,
        ) -> Take {
            Take {
                rt: self.rt.clone(),
                config: SessionsConfig {
                    release_wait_secs: 2,
                    ..SessionsConfig::default()
                },
                store: self.store.clone(),
                transport,
                connected,
                peers,
                by_user: false,
                caller: None,
            }
        }

        async fn close(self) {
            self.store.close().await;
            let _ = std::fs::remove_dir_all(&self.dir);
        }
    }

    fn peer(id: &str, reachable: bool, last_seen_ms: u64) -> mesh::Peer {
        mesh::Peer {
            id: mesh::EngineId(id.into()),
            name: id.into(),
            addrs: vec![],
            reachable,
            last_seen_ms,
            hub_pref: mesh::HubPref::Eligible,
            path: mesh::PeerPath::Offline,
            rtt_ms: None,
        }
    }

    /// A reachable owner: `Release` → `Released{last_seq}`, the taker waits
    /// for the tail, both sides end at epoch 2 with owner B; nothing is
    /// orphaned on A because everything had shipped.
    #[tokio::test]
    async fn take_from_a_reachable_owner_moves_the_tail_and_the_epoch() {
        let (a, b) = (engine("a").await, engine("b").await);
        let id = SessionId::new();
        a.store
            .create_session(id, "m", None, None, None)
            .await
            .unwrap();
        a.store.append(id, &agent_msg("u1")).await.unwrap();
        b.pull(&a).await;
        assert_eq!(b.store.fence(id).await.unwrap(), Some((1, "a".into())));
        // The tail B has not seen yet: a message and an open turn (139 §6.2).
        a.store.append(id, &agent_msg("u2")).await.unwrap();
        let turn = open_turn_on(&a.store, id).await;
        let tail = a.store.log_seq();

        let switch = Arc::new(Switch::default());
        let (la, lb) = (Loop::new("a", &switch), Loop::new("b", &switch));
        // A's side: serve the stream, then ship (the shipper runs after the release here).
        let shipper = crate::ship::Shipper::new(a.store.clone(), None, &SessionsConfig::default());
        let la2 = la.clone();
        let served = tokio::spawn(async move {
            let (peer, stream) = la2.accept(ALPN_SESSIONS).await.unwrap();
            crate::SessionSync::serve(shipper, peer, stream)
                .await
                .unwrap();
        });
        let (sb, sa, bid) = (b.store.clone(), a.store.clone(), b.id());
        let shipper = tokio::spawn(async move {
            tokio::time::sleep(Duration::from_millis(300)).await;
            for row in sa.log_after(0).await.unwrap() {
                let seq = row.seq;
                if apply_log_row(&sb, &PeerId("a".into()), &row.into())
                    .await
                    .unwrap()
                    == Applied::Applied
                {
                    sa.set_cursor("a", &bid.0, seq).await.unwrap();
                }
            }
        });

        let (got, resumed) = b
            .take(Some(lb.clone()), vec![a.id()], vec![])
            .take(id, TakeReason::Write)
            .await
            .unwrap();
        assert_eq!(got, ("b".into(), 2));
        served.await.unwrap();
        shipper.await.unwrap();
        assert_eq!(
            b.store.messages(id).await.unwrap().len(),
            3,
            "the tail arrived before the take, then the one Moved row (183 §5.3)"
        );
        assert_eq!(b.store.owner(id).await.unwrap(), Some(("b".into(), 2)));
        assert_eq!(b.store.fence(id).await.unwrap(), Some((2, "b".into())));
        // 139 D3/D4: A's release drained the turn — `suspended{release}` is
        // on B at epoch 1 (before B's `set_owner`, or the fence would have
        // dropped it) and B decided to resume it as a transfer from A.
        let plan = b.store.resume_plan(id, turn).await.unwrap();
        assert_eq!(plan.kind, ResumeKind::Suspended);
        assert_eq!(
            plan.cause,
            ResumeCause::Transfer,
            "newest step is suspended{{release}}"
        );
        assert_eq!(resumed, ResumeOnTake::Resume(ResumeCause::Transfer));
        b.store.append(id, &agent_msg("u3 on b")).await.unwrap();

        // B's take row reaches A: A demotes with nothing to orphan.
        assert!(a.pull(&b).await.iter().all(|r| *r == Applied::Applied));
        assert_eq!(a.store.owner(id).await.unwrap(), Some(("b".into(), 2)));
        let n = demote(
            &a.rt,
            &a.store,
            Demote {
                session: id,
                old_epoch: 1,
                by: b.id(),
            },
        )
        .await
        .unwrap();
        assert_eq!(n, 0);
        assert_eq!(a.store.fence(id).await.unwrap(), Some((2, "b".into())));
        assert_eq!(
            a.store.messages(id).await.unwrap().len(),
            4,
            "no note when nothing was lost (u1, u2, B's Moved row, u3)"
        );
        assert!(
            a.store.append(id, &agent_msg("late")).await.is_err(),
            "A is fenced out"
        );
        // 139 I3: the drain's row is the session's last row on A, and it shipped.
        assert!(a.store.session_log_seq(id).await.unwrap() > tail);
        a.close().await;
        b.close().await;
    }

    /// A `Write` take of an unreachable owner waits `takeover_secs` from the
    /// mesh's last sight of it; `User` and `Orphan` take at once.
    #[tokio::test]
    async fn take_from_an_unreachable_owner_respects_takeover_secs() {
        let (a, b) = (engine("a").await, engine("b").await);
        let id = SessionId::new();
        a.store
            .create_session(id, "m", None, None, None)
            .await
            .unwrap();
        b.pull(&a).await;
        let now = now_ms();

        let recent = vec![peer("a", false, now - 100_000)];
        let e = b
            .take(None, vec![], recent.clone())
            .run(id, TakeReason::Write)
            .await
            .unwrap_err();
        assert!(e.contains("takeover after 300s"), "{e}");
        assert_eq!(b.store.owner(id).await.unwrap(), Some(("a".into(), 1)));
        let e = b
            .take(None, vec![], vec![peer("a", true, now)])
            .run(id, TakeReason::Write)
            .await
            .unwrap_err();
        assert!(e.contains("reachable"), "{e}");

        assert_eq!(
            b.take(None, vec![], recent)
                .run(id, TakeReason::Orphan)
                .await
                .unwrap(),
            ("b".into(), 2)
        );
        // Already ours: a no-op at the current epoch.
        assert_eq!(
            b.take(None, vec![], vec![])
                .run(id, TakeReason::User)
                .await
                .unwrap(),
            ("b".into(), 2)
        );

        let c = engine("c").await;
        c.pull(&a).await;
        c.pull(&b).await;
        assert_eq!(c.store.fence(id).await.unwrap(), Some((2, "b".into())));
        let old = vec![peer("b", false, now - 301_000)];
        assert_eq!(
            c.take(None, vec![], old)
                .run(id, TakeReason::Write)
                .await
                .unwrap(),
            ("c".into(), 3)
        );
        let e = c
            .take(None, vec![], vec![])
            .run(SessionId::new(), TakeReason::User)
            .await
            .unwrap_err();
        assert!(e.contains("no such session"), "{e}");
        a.close().await;
        b.close().await;
        c.close().await;
    }

    /// 68 §4.3: A wrote two turns nobody received, B took the session
    /// (`User`, at once). When B's row reaches A, A orphans exactly those
    /// two rows, leaves a local note, and A's stale rows are dropped by B.
    #[tokio::test]
    async fn stale_owner_demotes_with_its_unshipped_rows_orphaned() {
        let (a, b) = (engine("a").await, engine("b").await);
        let id = SessionId::new();
        a.store
            .create_session(id, "m", None, None, None)
            .await
            .unwrap();
        a.store.append(id, &agent_msg("u1")).await.unwrap();
        let turn = open_turn_on(&a.store, id).await;
        b.pull(&a).await;
        // Partition: A keeps writing; B takes.
        a.store.append(id, &agent_msg("u2")).await.unwrap();
        a.store.append(id, &agent_msg("u3")).await.unwrap();
        // 139 D5: the user's take resumes the open turn here as a takeover
        // (no `suspended` row shipped: `kind = Interrupted`).
        let (got, resumed) = b
            .take(None, vec![], vec![peer("a", false, now_ms())])
            .take(id, TakeReason::User)
            .await
            .unwrap();
        assert_eq!(got, ("b".into(), 2));
        assert_eq!(resumed, ResumeOnTake::Resume(ResumeCause::Takeover));
        assert_eq!(
            b.store.resume_plan(id, turn).await.unwrap().kind,
            ResumeKind::Interrupted
        );
        b.store.append(id, &agent_msg("b1")).await.unwrap();

        // A comes back: B's rows land, A demotes.
        let applied = a.pull(&b).await;
        assert!(
            applied.iter().all(|r| *r == Applied::Applied),
            "{applied:?}"
        );
        let n = demote(
            &a.rt,
            &a.store,
            Demote {
                session: id,
                old_epoch: 1,
                by: b.id(),
            },
        )
        .await
        .unwrap();
        assert_eq!(n, 2);
        let orphans = crate::orphans(&a.store, id).await.unwrap();
        assert_eq!(orphans.len(), 1);
        assert_eq!(
            (orphans[0].epoch, orphans[0].to_seq - orphans[0].from_seq),
            (1, 1)
        );
        assert!(!orphans[0].at.is_empty());
        let last = a.store.messages(id).await.unwrap().pop().unwrap().1;
        let text = last.content.joined_texts().unwrap_or_default();
        assert!(
            text.starts_with("2 turns from ") && text.contains("on a;"),
            "{text}"
        );
        assert_eq!(a.store.fence(id).await.unwrap(), Some((2, "b".into())));
        assert_eq!(a.store.owner(id).await.unwrap(), Some(("b".into(), 2)));
        // A's stale rows never ship (orphans leave `log_after`); forced through, B drops them.
        assert!(
            a.store
                .log_after(0)
                .await
                .unwrap()
                .iter()
                .all(|r| r.epoch != 1 || r.seq <= 3)
        );
        for row in a.store.log_orphans(id).await.unwrap() {
            assert_eq!(
                apply_log_row(&b.store, &a.id(), &row.into()).await.unwrap(),
                Applied::StaleEpoch
            );
        }
        assert!(a.store.append(id, &agent_msg("late")).await.is_err());
        // Release on a non-owner is harmless: the session's last seq, nothing stopped.
        assert_eq!(
            release(&b.store, id).await.unwrap(),
            b.store.session_log_seq(id).await.unwrap() as u64
        );
        a.close().await;
        b.close().await;
    }

    /// 183 §5.3: an idle take writes exactly one Moved row, in context, with
    /// the taker's facts and the origin's mesh name; taking again (already
    /// ours) writes none.
    #[tokio::test]
    async fn an_idle_take_writes_one_moved_row_in_context() {
        let (a, b) = (engine("a").await, engine("b").await);
        let id = SessionId::new();
        a.store
            .create_session(id, "m", None, None, None)
            .await
            .unwrap();
        a.store.append(id, &agent_msg("u1")).await.unwrap();
        b.pull(&a).await;
        let mut named = peer("a", false, now_ms());
        named.name = "alpha".into();
        for _ in 0..2 {
            b.take(None, vec![], vec![named.clone()])
                .take(id, TakeReason::User)
                .await
                .unwrap();
        }
        let moved = b.store.moved(id).await.unwrap();
        assert_eq!(moved.len(), 1, "{moved:?}");
        let (seq, m) = moved.iter().next().unwrap();
        assert_eq!(
            (
                m.reason.as_str(),
                m.to_id.as_str(),
                m.from_id.as_str(),
                m.from.as_str(),
                m.pending
            ),
            ("user", "b", "a", "alpha", false)
        );
        assert!(!b.store.context_history(id).await.unwrap().entries[seq].removed);
        let text = b.store.effective_message(id, *seq).await.unwrap().unwrap();
        assert_eq!(
            text.content.joined_texts().unwrap_or_default(),
            m.model_text()
        );
        a.close().await;
        b.close().await;
    }

    /// 183 R1 (E-M3): a taker that reports an older agent schema, or none (a
    /// pre-183 binary: its `Release` is the old frame), is refused by a
    /// closed stream before anything is drained or written; the session stays
    /// owned here. A taker on our schema is answered as before.
    #[tokio::test]
    async fn release_from_an_older_schema_taker_is_refused() {
        use futures::{SinkExt as _, StreamExt as _};
        let a = engine("a").await;
        let id = SessionId::new();
        a.store
            .create_session(id, "m", None, None, None)
            .await
            .unwrap();
        a.store.append(id, &agent_msg("u1")).await.unwrap();
        open_turn_on(&a.store, id).await;
        let ours = agent::store::SESSIONS_WIRE_SCHEMA;
        let before = a.store.session_log_seq(id).await.unwrap();
        for (schema, refused) in [(None, true), (Some(ours - 1), true), (Some(ours), false)] {
            let shipper =
                crate::ship::Shipper::new(a.store.clone(), None, &SessionsConfig::default());
            let (mine, theirs) = tokio::io::duplex(1 << 16);
            let served = tokio::spawn(crate::SessionSync::serve(
                shipper,
                PeerId("b".into()),
                Box::new(theirs),
            ));
            let mut taker = crate::events::framed(Box::new(mine));
            taker
                .send(
                    Msg::Release {
                        session: id.to_string(),
                        schema,
                    }
                    .encode()
                    .unwrap(),
                )
                .await
                .unwrap();
            let reply = taker.next().await;
            served.await.unwrap().unwrap();
            if refused {
                assert!(
                    reply.is_none(),
                    "{schema:?}: no Released, the stream closes"
                );
                assert_eq!(a.store.owner(id).await.unwrap(), Some(("a".into(), 1)));
                assert_eq!(
                    a.store.session_log_seq(id).await.unwrap(),
                    before,
                    "{schema:?}: nothing was drained or written"
                );
            } else {
                let frame = reply.expect("a taker on our schema is answered").unwrap();
                assert!(matches!(Msg::decode(&frame).unwrap(), Msg::Released { .. }));
            }
        }
        a.close().await;
    }

    /// Mark `id` on `store` as a contact's inbox, as an admission does.
    async fn mark_inbox(store: &Store, id: SessionId) {
        let scope = agent::store::communication::A2aScope {
            principal: "peer:bob",
            export: "ops",
        };
        store.create_a2a_context(scope, "c1", id).await.unwrap();
        store
            .mark_a2a_inbox(id, None, "ops", false, None)
            .await
            .unwrap();
    }

    /// 191 (D4): `id`'s inbox marker is `peer:bob|<export>`; the exports `x-live` (live) and
    /// `x-gone` (tombstoned) exist.
    async fn set_marker(store: &Store, id: SessionId, export: &str) {
        use agent::store::contacts::{Cell, RecordTable, RecordWrite};
        store
            .write_contact_records(&[
                RecordWrite::new(
                    RecordTable::Exports,
                    "x-live",
                    vec![("name", Cell::Text("ops".into()))],
                ),
                RecordWrite::new(
                    RecordTable::Exports,
                    "x-gone",
                    vec![("removed_ms", Cell::Int(1))],
                ),
            ])
            .await
            .unwrap();
        store
            .set_contact_inbox(id, &format!("peer:bob|{export}"))
            .await
            .unwrap();
    }

    /// 191 (D4, the `INBOX_STAYS` lift): a marked inbox (its marker names a live export by
    /// id) moves; an unmarked one, a name-valued marker and a tombstoned export's stay.
    #[tokio::test]
    async fn only_a_marked_contact_inbox_moves() {
        let a = engine("a").await;
        let id = SessionId::new();
        a.store
            .create_session(id, "m", None, None, None)
            .await
            .unwrap();
        mark_inbox(&a.store, id).await;
        let xeus = PeerId("x".into());
        let mv = || {
            move_to(
                &a.store,
                id,
                &xeus,
                "xeus",
                std::slice::from_ref(&xeus),
                &[],
                async { Ok(Some(("x".to_owned(), 2))) },
            )
        };
        assert_eq!(mv().await, Err(INBOX_STAYS.to_owned()), "unmarked");
        for export in ["ops", "x-gone", "x-unknown"] {
            set_marker(&a.store, id, export).await;
            assert_eq!(mv().await, Err(INBOX_STAYS.to_owned()), "{export}");
        }
        set_marker(&a.store, id, "x-live").await;
        assert_eq!(mv().await, Ok(("x".to_owned(), 2)), "marked: it moves");
        a.close().await;
    }

    /// A contact's inbox never moves: `move_to` refuses it before asking the
    /// target, and the session stays owned here, unchanged.
    #[tokio::test]
    async fn move_of_a_contact_inbox_is_refused() {
        let a = engine("a").await;
        let id = SessionId::new();
        a.store
            .create_session(id, "m", None, None, None)
            .await
            .unwrap();
        a.store.append(id, &agent_msg("u1")).await.unwrap();
        mark_inbox(&a.store, id).await;
        let before = a.store.session_log_seq(id).await.unwrap();
        let xeus = PeerId("x".into());
        let mut asked = false;
        let got = move_to(
            &a.store,
            id,
            &xeus,
            "xeus",
            std::slice::from_ref(&xeus),
            &[],
            async {
                asked = true;
                Ok(Some(("x".to_owned(), 2)))
            },
        )
        .await;
        assert_eq!(got, Err(INBOX_STAYS.to_owned()));
        assert!(!asked, "the target is never asked");
        assert_eq!(a.store.fence(id).await.unwrap(), Some((1, "a".into())));
        assert_eq!(a.store.owner(id).await.unwrap(), Some(("a".into(), 1)));
        assert_eq!(a.store.session_log_seq(id).await.unwrap(), before);
        a.close().await;
    }

    /// "Continue here" (and any take) of an unmarked contact's inbox from its reachable
    /// owner: the owner answers `Refused` before draining, the taker returns its words, and
    /// neither side changed owner, epoch or log. 191 (D4): once marked, it is released.
    #[tokio::test]
    async fn take_of_a_contact_inbox_is_refused_by_its_owner() {
        let (a, b) = (engine("a").await, engine("b").await);
        let id = SessionId::new();
        a.store
            .create_session(id, "m", None, None, None)
            .await
            .unwrap();
        a.store.append(id, &agent_msg("u1")).await.unwrap();
        b.pull(&a).await;
        mark_inbox(&a.store, id).await;
        let turn = open_turn_on(&a.store, id).await;
        let before = a.store.session_log_seq(id).await.unwrap();

        let switch = Arc::new(Switch::default());
        let (la, lb) = (Loop::new("a", &switch), Loop::new("b", &switch));
        for reason in [TakeReason::User, TakeReason::Write] {
            let shipper =
                crate::ship::Shipper::new(a.store.clone(), None, &SessionsConfig::default());
            let la2 = la.clone();
            let served = tokio::spawn(async move {
                let (peer, stream) = la2.accept(ALPN_SESSIONS).await.unwrap();
                crate::SessionSync::serve(shipper, peer, stream)
                    .await
                    .unwrap();
            });
            let e = b
                .take(Some(lb.clone()), vec![a.id()], vec![])
                .run(id, reason)
                .await
                .unwrap_err();
            served.await.unwrap();
            assert_eq!(e, INBOX_STAYS, "{reason:?}");
            assert_eq!(b.store.fence(id).await.unwrap(), Some((1, "a".into())));
            assert_eq!(b.store.owner(id).await.unwrap(), Some(("a".into(), 1)));
            assert!(b.store.moved(id).await.unwrap().is_empty(), "no Moved row");
            assert_eq!(a.store.owner(id).await.unwrap(), Some(("a".into(), 1)));
            assert_eq!(
                a.store.session_log_seq(id).await.unwrap(),
                before,
                "{reason:?}: nothing drained or written on the owner"
            );
            assert_eq!(
                a.store.open_turn(id).await.unwrap().map(|(t, _)| t),
                Some(turn),
                "the turn stays open on the owner"
            );
        }
        // 191 (D4): a name-valued marker (`peer:bob|ops`, the export's name) or an unreadable
        // one (no `|`) is not a marked inbox: the owner refuses a Write take too, nothing drained.
        set_marker(&a.store, id, "ops").await;
        for marker in [None, Some("unreadable")] {
            if let Some(m) = marker {
                assert!(a.store.set_contact_inbox(id, m).await.unwrap());
            }
            let shipper =
                crate::ship::Shipper::new(a.store.clone(), None, &SessionsConfig::default());
            let la2 = la.clone();
            let served = tokio::spawn(async move {
                let (peer, stream) = la2.accept(ALPN_SESSIONS).await.unwrap();
                crate::SessionSync::serve(shipper, peer, stream)
                    .await
                    .unwrap();
            });
            let e = b
                .take(Some(lb.clone()), vec![a.id()], vec![])
                .run(id, TakeReason::Write)
                .await
                .unwrap_err();
            served.await.unwrap();
            assert_eq!(e, INBOX_STAYS, "{marker:?}");
            assert_eq!(a.store.owner(id).await.unwrap(), Some(("a".into(), 1)));
            assert_eq!(
                a.store.open_turn(id).await.unwrap().map(|(t, _)| t),
                Some(turn),
                "{marker:?}: the turn stays open on the owner"
            );
        }
        // 191 (D4): marked, it is released to the taker.
        set_marker(&a.store, id, "x-live").await;
        let shipper = crate::ship::Shipper::new(a.store.clone(), None, &SessionsConfig::default());
        let la2 = la.clone();
        let served = tokio::spawn(async move {
            let (peer, stream) = la2.accept(ALPN_SESSIONS).await.unwrap();
            crate::SessionSync::serve(shipper, peer, stream)
                .await
                .unwrap();
        });
        b.take(Some(lb.clone()), vec![a.id()], vec![])
            .run(id, TakeReason::User)
            .await
            .unwrap();
        served.await.unwrap();
        assert_eq!(
            b.store.owner(id).await.unwrap().map(|(o, _)| o).as_deref(),
            Some("b")
        );
        a.close().await;
        b.close().await;
    }

    fn agent_msg(t: &str) -> genai::chat::ChatMessage {
        genai::chat::ChatMessage::user(t)
    }

    /// Open a turn of `id` on `store` (its `input_accepted` step) and return it.
    async fn open_turn_on(store: &Store, id: SessionId) -> runtime::TurnId {
        let turn = runtime::TurnId::new();
        store
            .step(
                id,
                turn,
                StepKind::InputAccepted,
                None,
                &serde_json::json!({}),
            )
            .await
            .unwrap();
        turn
    }

    /// Collects the runtime's `AgentEvent`s.
    #[derive(kameo::Actor)]
    struct Sink {
        events: Arc<parking_lot::Mutex<Vec<AgentEvent>>>,
    }

    impl Message<AgentEvent> for Sink {
        type Reply = ();

        async fn handle(&mut self, e: AgentEvent, _: &mut Context<Self, Self::Reply>) {
            self.events.lock().push(e);
        }
    }

    async fn sink(rt: &Runtime) -> Arc<parking_lot::Mutex<Vec<AgentEvent>>> {
        let events = Arc::new(parking_lot::Mutex::new(Vec::new()));
        let actor = kameo::actor::Spawn::spawn(Sink {
            events: events.clone(),
        });
        rt.subscribe::<Sink, AgentEvent>(&actor).await;
        events
    }

    /// 139 §6.3 (D5, Q10): the policy table as ruled.
    #[test]
    fn resume_policy_follows_the_table() {
        use ResumeOnTake::*;
        for reason in [TakeReason::User, TakeReason::Write, TakeReason::Orphan] {
            for by_user in [false, true] {
                assert_eq!(
                    resume_policy(true, reason, by_user, ResumeMode::Auto),
                    Resume(ResumeCause::Transfer)
                );
                assert_eq!(
                    resume_policy(true, reason, by_user, ResumeMode::Ask),
                    Pending
                );
                assert_eq!(
                    resume_policy(true, reason, by_user, ResumeMode::Off),
                    Nothing
                );
            }
        }
        // 181 D3: a move resumes whatever `durability.resume` says.
        for mode in [ResumeMode::Auto, ResumeMode::Ask, ResumeMode::Off] {
            assert_eq!(
                resume_policy(true, TakeReason::Moved, true, mode),
                Resume(ResumeCause::Transfer)
            );
        }
        for mode in [ResumeMode::Auto, ResumeMode::Ask, ResumeMode::Off] {
            assert_eq!(
                resume_policy(false, TakeReason::User, false, mode),
                Resume(ResumeCause::Takeover)
            );
            assert_eq!(
                resume_policy(false, TakeReason::Write, false, mode),
                Pending
            );
            // Q10: the front door's take for a typing client is the user's.
            assert_eq!(
                resume_policy(false, TakeReason::Write, true, mode),
                Resume(ResumeCause::Takeover)
            );
            assert_eq!(
                resume_policy(false, TakeReason::Orphan, false, mode),
                Pending
            );
            assert_eq!(
                resume_policy(false, TakeReason::Orphan, true, mode),
                Pending
            );
        }
    }

    /// 139 D5: a system takeover (`Write` after `takeover_secs`, `Orphan`)
    /// of a session with an open turn does not resume it — `ResumePending`
    /// is published and the transcript gets the local note; a session with
    /// no open turn publishes nothing.
    #[tokio::test]
    async fn system_takeover_leaves_the_open_turn_pending() {
        let (a, b) = (engine("a").await, engine("b").await);
        let events = sink(&b.rt).await;
        let id = SessionId::new();
        a.store
            .create_session(id, "m", None, None, None)
            .await
            .unwrap();
        a.store.append(id, &agent_msg("u1")).await.unwrap();
        let turn = open_turn_on(&a.store, id).await;
        b.pull(&a).await;

        let old = vec![peer("a", false, now_ms() - 301_000)];
        let (got, resumed) = b
            .take(None, vec![], old.clone())
            .take(id, TakeReason::Write)
            .await
            .unwrap();
        assert_eq!((got, resumed), (("b".into(), 2), ResumeOnTake::Pending));
        tokio::time::sleep(Duration::from_millis(50)).await;
        assert!(
            events.lock().iter().any(|e| matches!(
                e,
                AgentEvent::ResumePending { session, turn: t, from } if *session == id && *t == turn && from == "a"
            )),
            "{:?}",
            events.lock()
        );
        // 183 §5.3: the Moved row replaces the local note; an open turn keeps
        // it out of context (the model reads `<engine_restart/>`).
        let moved = b.store.moved(id).await.unwrap();
        assert_eq!(moved.len(), 1);
        let (seq, m) = moved.iter().next().unwrap();
        assert!(
            (
                m.pending,
                m.reason.as_str(),
                m.to_id.as_str(),
                m.from_id.as_str()
            ) == (true, "write", "b", "a")
        );
        assert!(b.store.context_history(id).await.unwrap().entries[seq].removed);
        // The turn is still open on B: the user's Resume runs it.
        assert_eq!(
            b.store.open_turn(id).await.unwrap().map(|(t, _)| t),
            Some(turn)
        );

        // Orphan: the same, from a third engine; no open turn → nothing.
        let c = engine("c").await;
        let c_events = sink(&c.rt).await;
        c.pull(&a).await;
        c.pull(&b).await;
        let closed = SessionId::new();
        b.store
            .create_session(closed, "m", None, None, None)
            .await
            .unwrap();
        c.pull(&b).await;
        let (_, resumed) = c
            .take(None, vec![], vec![peer("b", false, now_ms())])
            .take(id, TakeReason::Orphan)
            .await
            .unwrap();
        assert_eq!(resumed, ResumeOnTake::Pending);
        let (_, resumed) = c
            .take(None, vec![], vec![peer("b", false, now_ms())])
            .take(closed, TakeReason::Orphan)
            .await
            .unwrap();
        assert_eq!(resumed, ResumeOnTake::NoOpenTurn);
        tokio::time::sleep(Duration::from_millis(50)).await;
        let pending: Vec<SessionId> = c_events
            .lock()
            .iter()
            .filter_map(|e| match e {
                AgentEvent::ResumePending { session, .. } => Some(*session),
                _ => None,
            })
            .collect();
        assert_eq!(pending, [id]);
        a.close().await;
        b.close().await;
        c.close().await;
    }

    // ---- 180: a taken session's ledger owner (no record replica here, the bug's precondition) ----

    /// Reserve, dispatch and settle one call on `owner` in `store`.
    async fn book(store: &Store, owner: SessionId) {
        use agent::cost::{CallKind, CallStatus, CostObservation};
        let call = store
            .reserve_call(owner, "req", 0, None, CallKind::Round, Some("p"), Some("m"))
            .await
            .unwrap();
        store.mark_dispatched(&call).await.unwrap();
        store
            .settle_call(&call, CallStatus::Settled, CostObservation::default())
            .await
            .unwrap();
    }

    /// The derived stand-in's `hlc` (`accounting::STAND_IN_HLC`).
    const STAND_IN: &str = "0000000000000000-00000000-0";

    type LedgerRow = (String, Option<String>, String, i64, Option<String>);

    impl Engine {
        async fn ro(&self) -> sqlx::SqlitePool {
            sqlx::SqlitePool::connect(&format!(
                "sqlite://{}?mode=ro",
                self.dir.join("sessions.sqlite").display()
            ))
            .await
            .unwrap()
        }

        /// `(table, parent)` of every `PRAGMA foreign_key_check` row of the accounting tables.
        async fn ledger_orphans(&self) -> Vec<(String, String)> {
            let pool = self.ro().await;
            let rows: Vec<(String, Option<i64>, String, i64)> =
                sqlx::query_as("PRAGMA foreign_key_check")
                    .fetch_all(&pool)
                    .await
                    .unwrap();
            pool.close().await;
            rows.into_iter()
                .filter(|r| {
                    [
                        "cost_owners",
                        "model_calls",
                        "remote_meters",
                        "remote_turns",
                    ]
                    .contains(&r.0.as_str())
                })
                .map(|r| (r.0, r.2))
                .collect()
        }

        /// `(root_id, parent_id, history_coverage, revision, hlc)` of `owner`'s ledger row.
        async fn ledger_row(&self, owner: SessionId) -> Option<LedgerRow> {
            let pool = self.ro().await;
            let row = sqlx::query_as(
                "SELECT root_id, parent_id, history_coverage, revision, hlc FROM cost_owners WHERE id = ?",
            )
            .bind(owner.to_string())
            .fetch_optional(&pool)
            .await
            .unwrap();
            pool.close().await;
            row
        }

        /// Calls booked on `owner` here.
        async fn calls(&self, owner: SessionId) -> usize {
            self.store
                .session_cost(owner)
                .await
                .unwrap()
                .map_or(0, |c| c.direct.calls as usize)
        }
    }

    /// 180 (Sky's report), both directions: a session one engine created
    /// reaches the other through the session log only — its `sessions` and
    /// `model_calls` rows, not its ledger row. The taker reserves and books
    /// on a stand-in derived from the session row, and holds no orphan.
    #[tokio::test]
    async fn taken_session_reserves_without_replicated_owner() {
        let (a, b) = (engine("a").await, engine("b").await);
        for (from, to, next) in [(&a, &b, "b"), (&b, &a, "a")] {
            let id = SessionId::new();
            from.store
                .create_session(id, "m", None, None, None)
                .await
                .unwrap();
            book(&from.store, id).await;
            to.pull(from).await;
            assert_eq!(to.ledger_row(id).await, None, "no record replica here");

            to.take(None, vec![], vec![])
                .run(id, TakeReason::User)
                .await
                .unwrap();
            assert_eq!(to.store.owner(id).await.unwrap(), Some((next.into(), 2)));
            book(&to.store, id).await;
            assert_eq!(to.calls(id).await, 2, "the replicated call and the new one");
            let row = to.ledger_row(id).await.unwrap();
            assert_eq!(
                (row.0, row.1, row.2.as_str(), row.4.as_deref()),
                (id.to_string(), None, "unknown", Some(STAND_IN))
            );
            assert!(to.ledger_orphans().await.is_empty());
        }
        a.close().await;
        b.close().await;
    }

    /// 180 D1a: a grandchild taken alone (takes are per session) derives its
    /// whole lineage from the `sessions` rows, root first; every row points
    /// at the tree's root.
    #[tokio::test]
    async fn taken_child_chain_root_first() {
        let (a, b) = (engine("a").await, engine("b").await);
        let (root, child, grand) = (SessionId::new(), SessionId::new(), SessionId::new());
        a.store
            .create_session(root, "m", None, None, None)
            .await
            .unwrap();
        a.store
            .create_session(child, "m", Some(root), None, None)
            .await
            .unwrap();
        a.store
            .create_session(grand, "m", Some(child), None, None)
            .await
            .unwrap();
        b.pull(&a).await;
        b.take(None, vec![], vec![])
            .run(grand, TakeReason::User)
            .await
            .unwrap();
        book(&b.store, grand).await;

        let r = root.to_string();
        for (id, parent) in [(root, None), (child, Some(root)), (grand, Some(child))] {
            let row = b.ledger_row(id).await.unwrap();
            assert_eq!(
                (row.0, row.1, row.2.as_str(), row.4.as_deref()),
                (
                    r.clone(),
                    parent.map(|p| p.to_string()),
                    "unknown",
                    Some(STAND_IN)
                ),
                "{id}"
            );
        }
        assert!(b.ledger_orphans().await.is_empty());
        a.close().await;
        b.close().await;
    }

    /// a → b → a: whatever B derived and booked, A's own ledger row
    /// (revision, coverage, stamp) is untouched when A takes the session
    /// back, and A books against it. `snapshot`: B bootstraps from A's
    /// snapshot instead of A's log.
    async fn origin_retake(snapshot: bool) {
        let (a, b) = (engine("a").await, engine("b").await);
        let id = SessionId::new();
        a.store
            .create_session(id, "m", None, None, None)
            .await
            .unwrap();
        book(&a.store, id).await;
        let mine = a.ledger_row(id).await.unwrap();
        assert_eq!(mine.2, "complete");
        if snapshot {
            let snap = crate::apply::build_snapshot(&a.store, &a.id())
                .await
                .unwrap();
            crate::apply::apply_snapshot(&b.store, &snap).await.unwrap();
        } else {
            b.pull(&a).await;
        }
        b.take(None, vec![], vec![])
            .run(id, TakeReason::User)
            .await
            .unwrap();
        book(&b.store, id).await;
        a.pull(&b).await;
        assert_eq!(a.store.fence(id).await.unwrap(), Some((2, "b".into())));

        a.take(None, vec![], vec![])
            .run(id, TakeReason::User)
            .await
            .unwrap();
        assert_eq!(a.store.owner(id).await.unwrap(), Some(("a".into(), 3)));
        assert_eq!(
            a.ledger_row(id).await,
            Some(mine),
            "B touched nothing of A's row"
        );
        book(&a.store, id).await;
        assert_eq!(a.calls(id).await, 3);
        assert!(a.ledger_orphans().await.is_empty());
        b.pull(&a).await;
        assert_eq!(b.calls(id).await, 3);
        assert!(b.ledger_orphans().await.is_empty());
        a.close().await;
        b.close().await;
    }

    #[tokio::test]
    async fn origin_retake_keeps_its_row() {
        origin_retake(false).await;
    }

    #[tokio::test]
    async fn origin_retake_keeps_its_row_after_snapshot() {
        origin_retake(true).await;
    }

    // ---- 181: Move to ▸ (the origin's `move_to`, the target's `Moved` take) ----

    /// A moves `id` to B over a loopback mesh: B's `Moved` take is
    /// `move_to`'s `ask` (what `session_move` sends as `session_take(moved)`);
    /// A answers B's `Release` and (`ship`) ships its log right after, as the
    /// shipper does — unshipped, B's take fails after A's drain. Returns the
    /// move's answer and what B did about the open turn.
    async fn move_over(
        a: &Engine,
        b: &Engine,
        id: SessionId,
        ship: bool,
    ) -> (Result<(String, u32), String>, Option<ResumeOnTake>) {
        let switch = Arc::new(Switch::default());
        let (la, lb) = (Loop::new("a", &switch), Loop::new("b", &switch));
        let shipper = crate::ship::Shipper::new(a.store.clone(), None, &SessionsConfig::default());
        let served = tokio::spawn(async move {
            let (peer, stream) = la.accept(ALPN_SESSIONS).await.unwrap();
            crate::SessionSync::serve(shipper, peer, stream)
                .await
                .unwrap();
        });
        let (sb, sa, bid) = (b.store.clone(), a.store.clone(), b.id());
        let ship = tokio::spawn(async move {
            if !ship {
                return;
            }
            tokio::time::sleep(Duration::from_millis(300)).await;
            for row in sa.log_after(0).await.unwrap() {
                let seq = row.seq;
                if apply_log_row(&sb, &PeerId("a".into()), &row.into())
                    .await
                    .unwrap()
                    == Applied::Applied
                {
                    sa.set_cursor("a", &bid.0, seq).await.unwrap();
                }
            }
        });
        let mut resumed = None;
        let ask = async {
            let mut take = b.take(Some(lb.clone()), vec![a.id()], vec![]);
            take.caller = Some(a.id());
            let (owned, r) = take
                .take(id, TakeReason::Moved)
                .await
                .map_err(MoveFailure::Refused)?;
            resumed = Some(r);
            Ok(Some(owned))
        };
        let got = move_to(&a.store, id, &b.id(), "b", &[b.id()], &[], ask).await;
        served.await.unwrap();
        ship.await.unwrap();
        (got, resumed)
    }

    /// A's side once B's take row lands: what the apply path's `Demote` does.
    async fn demote_on(a: &Engine, b: &Engine, id: SessionId, old_epoch: u32) {
        assert!(a.pull(b).await.iter().all(|r| *r != Applied::StaleEpoch));
        let m = Demote {
            session: id,
            old_epoch,
            by: b.id(),
        };
        assert_eq!(demote(&a.rt, &a.store, m).await.unwrap(), 0);
    }

    /// 181 D2: the move is a take by the target — epoch + 1 there, the origin
    /// demoted and fenced out, `SessionOwnerChanged{moved_from}` on the
    /// target only; calls book on the target and 180's owner rows hold.
    #[tokio::test]
    async fn move_hands_the_session_to_the_target() {
        let (a, b) = (engine("a").await, engine("b").await);
        let events = sink(&b.rt).await;
        let id = SessionId::new();
        a.store
            .create_session(id, "m", None, None, None)
            .await
            .unwrap();
        a.store.append(id, &agent_msg("u1")).await.unwrap();
        book(&a.store, id).await;
        b.pull(&a).await;
        a.store
            .append(id, &agent_msg("u2, not on b yet"))
            .await
            .unwrap();

        let (got, resumed) = move_over(&a, &b, id, true).await;
        assert_eq!(got, Ok(("b".into(), 2)));
        assert_eq!(resumed, Some(ResumeOnTake::NoOpenTurn));
        assert_eq!(b.store.fence(id).await.unwrap(), Some((2, "b".into())));
        assert_eq!(
            b.store.messages(id).await.unwrap().len(),
            3,
            "the tail moved with it, then the one Moved row (183 §5.3)"
        );
        tokio::time::sleep(Duration::from_millis(50)).await;
        assert!(
            events.lock().iter().any(|e| matches!(
                e,
                AgentEvent::SessionOwnerChanged { session, owner, epoch: 2, here: true, moved_from: Some(from) }
                    if *session == id && owner == "b" && from == "a"
            )),
            "{:?}",
            events.lock()
        );

        book(&b.store, id).await;
        assert_eq!(b.calls(id).await, 2, "A's call and B's");
        assert!(b.ledger_orphans().await.is_empty());
        demote_on(&a, &b, id, 1).await;
        assert_eq!(a.store.fence(id).await.unwrap(), Some((2, "b".into())));
        assert!(
            a.store.append(id, &agent_msg("late")).await.is_err(),
            "A is fenced out"
        );
        assert_eq!(a.calls(id).await, 2);
        assert!(a.ledger_orphans().await.is_empty());
        a.close().await;
        b.close().await;
    }

    /// 181 D3: a turn open on the origin is suspended by the release drain
    /// and resumed on the target as a transfer — even under
    /// `durability.resume: ask`, where a plain take only leaves it pending.
    #[tokio::test]
    async fn move_mid_turn_resumes_on_the_target() {
        let a = engine("a").await;
        let mut b = engine("b").await;
        b.rt = Runtime::new(
            figment::Figment::from(figment::providers::Serialized::default(
                "durability",
                serde_json::json!({ "resume": "ask" }),
            )),
            b.dir.clone(),
        );
        assert_eq!(
            b.rt.section::<DurabilityConfig>("durability")
                .unwrap()
                .resume,
            ResumeMode::Ask
        );
        let id = SessionId::new();
        a.store
            .create_session(id, "m", None, None, None)
            .await
            .unwrap();
        a.store.append(id, &agent_msg("u1")).await.unwrap();
        let turn = open_turn_on(&a.store, id).await;
        b.pull(&a).await;

        let (got, resumed) = move_over(&a, &b, id, true).await;
        assert_eq!(got, Ok(("b".into(), 2)));
        assert_eq!(resumed, Some(ResumeOnTake::Resume(ResumeCause::Transfer)));
        let plan = b.store.resume_plan(id, turn).await.unwrap();
        assert_eq!(
            (plan.kind, plan.cause),
            (ResumeKind::Suspended, ResumeCause::Transfer),
            "suspended{{release}} by A's drain, shipped before B's epoch"
        );
        demote_on(&a, &b, id, 1).await;
        a.close().await;
        b.close().await;
    }

    /// 181 D2: only the owner moves a session; nothing is asked of the
    /// target otherwise. A `Moved` take never takes over: the owner must be
    /// connected to release.
    #[tokio::test]
    async fn move_refused_when_not_owner() {
        let (a, b) = (engine("a").await, engine("b").await);
        let id = SessionId::new();
        a.store
            .create_session(id, "m", None, None, None)
            .await
            .unwrap();
        b.pull(&a).await;
        let mut asked = false;
        let got = move_to(&b.store, id, &a.id(), "a", &[a.id()], &[], async {
            asked = true;
            Ok(Some(("a".to_owned(), 2)))
        })
        .await;
        assert_eq!(got, Err("this session is not owned here".to_owned()));
        assert!(!asked);
        let got = move_to(&a.store, id, &a.id(), "a", &[], &[], async { Ok(None) }).await;
        assert_eq!(got, Err("this session already runs here".to_owned()));

        let mut take = b.take(None, vec![], vec![peer("a", false, 0)]);
        take.caller = Some(a.id());
        let e = take.run(id, TakeReason::Moved).await.unwrap_err();
        assert!(e.contains("not connected"), "{e}");
        assert_eq!(b.store.fence(id).await.unwrap(), Some((1, "a".into())));
        a.close().await;
        b.close().await;
    }

    /// 181 D2: an unconnected target is offline before anything is sent;
    /// one the mesh could not reach is offline too; a target's refusal comes
    /// back in its words. The session stays put each time.
    #[tokio::test]
    async fn move_to_offline_peer_says_so() {
        let a = engine("a").await;
        let id = SessionId::new();
        a.store
            .create_session(id, "m", None, None, None)
            .await
            .unwrap();
        let xeus = PeerId("x".into());
        let mut asked = false;
        let got = move_to(&a.store, id, &xeus, "xeus", &[], &[], async {
            asked = true;
            Ok(None)
        })
        .await;
        assert_eq!(got, Err("xeus is offline".to_owned()));
        assert!(!asked);
        let connected = std::slice::from_ref(&xeus);
        let got = move_to(&a.store, id, &xeus, "xeus", connected, &[], async {
            Ok(None)
        })
        .await;
        assert_eq!(got, Err("xeus is offline".to_owned()));
        let got = move_to(&a.store, id, &xeus, "xeus", connected, &[], async {
            Err(MoveFailure::NoAnswer)
        })
        .await;
        assert_eq!(
            got,
            Err("no answer from xeus yet — the move may still land".to_owned())
        );
        let got = move_to(&a.store, id, &xeus, "xeus", connected, &[], async {
            Err(MoveFailure::Refused(
                "owner a busy: release timed out".into(),
            ))
        })
        .await;
        assert_eq!(
            got,
            Err("Move to xeus failed: xeus refused: owner a busy: release timed out. It continues here.".to_owned())
        );
        assert_eq!(a.store.fence(id).await.unwrap(), Some((1, "a".into())));
        a.close().await;
    }

    /// 181 D3 / 139 §8: children follow their root as on a take — the root
    /// moves, its child stays owned (and writable) where it runs, both
    /// engines keep the lineage, and each books where it runs with every
    /// ledger row in place on the child's engine.
    #[tokio::test]
    async fn move_keeps_child_sessions_with_their_root() {
        let (a, b) = (engine("a").await, engine("b").await);
        let (root, child) = (SessionId::new(), SessionId::new());
        a.store
            .create_session(root, "m", None, None, None)
            .await
            .unwrap();
        a.store
            .create_session(child, "m", Some(root), None, None)
            .await
            .unwrap();
        book(&a.store, child).await;
        b.pull(&a).await;

        let (got, _) = move_over(&a, &b, root, true).await;
        assert_eq!(got, Ok(("b".into(), 2)));
        demote_on(&a, &b, root, 1).await;
        for e in [&a, &b] {
            assert_eq!(e.store.fence(root).await.unwrap(), Some((2, "b".into())));
            assert_eq!(e.store.fence(child).await.unwrap(), Some((1, "a".into())));
            let row = e.store.session(child).await.unwrap().unwrap();
            assert_eq!(row.parent, Some(root));
        }
        a.store
            .append(child, &agent_msg("child runs on"))
            .await
            .unwrap();
        book(&a.store, child).await;
        book(&b.store, root).await;
        b.pull(&a).await;
        a.pull(&b).await;
        assert_eq!((a.calls(child).await, a.calls(root).await), (2, 1));
        assert!(a.ledger_orphans().await.is_empty());
        // B never took the child: its ledger row reaches B over the record
        // replica (not in this harness), as for any session B only mirrors.
        assert_eq!(b.calls(root).await, 1);
        assert_eq!(
            b.ledger_row(root).await.map(|r| r.0),
            Some(root.to_string())
        );
        a.close().await;
        b.close().await;
    }

    /// `move_to` refusing before anything is asked of the target; the session stays put.
    async fn refused_locally(
        a: &Engine,
        id: SessionId,
        held: &[(SessionId, Option<String>)],
    ) -> String {
        let x = PeerId("x".into());
        let mut asked = false;
        let got = move_to(
            &a.store,
            id,
            &x,
            "xeus",
            std::slice::from_ref(&x),
            held,
            async {
                asked = true;
                Ok(Some(("x".to_owned(), 2)))
            },
        )
        .await;
        assert!(!asked, "{got:?}");
        assert_eq!(a.store.fence(id).await.unwrap(), Some((1, "a".into())));
        got.unwrap_err()
    }

    /// 181 fix round: the pause map is this engine's alone (179), so a paused
    /// agent's desk and every session under it are refused here.
    #[tokio::test]
    async fn move_refused_for_a_paused_agents_session() {
        let a = engine("a").await;
        let (desk, child) = (SessionId::new(), SessionId::new());
        a.store
            .create_session(desk, "m", None, None, None)
            .await
            .unwrap();
        a.store.set_desk(desk, Some("ops")).await.unwrap();
        a.store
            .create_session(child, "m", Some(desk), None, None)
            .await
            .unwrap();
        let held = [(desk, Some("ops".to_owned()))];
        for id in [desk, child] {
            assert_eq!(
                refused_locally(&a, id, &held).await,
                "ops is paused — un-pause it before moving"
            );
        }
        // Not paused: the child moves like any session (the ask runs).
        let x = PeerId("x".into());
        let got = move_to(
            &a.store,
            child,
            &x,
            "xeus",
            std::slice::from_ref(&x),
            &[],
            async { Ok(Some(("x".to_owned(), 2))) },
        )
        .await;
        assert_eq!(got, Ok(("x".into(), 2)));
        a.close().await;
    }

    /// 181 fix round: a roster desk is forwarded, never taken; it does not move.
    #[tokio::test]
    async fn move_refused_for_a_desk_session() {
        let a = engine("a").await;
        let desk = SessionId::new();
        a.store
            .create_session(desk, "m", None, None, None)
            .await
            .unwrap();
        a.store.set_desk(desk, Some("ops")).await.unwrap();
        let e = refused_locally(&a, desk, &[]).await;
        assert_eq!(
            e,
            "this is ops's desk: a desk stays on the engine that runs its agent, so it cannot move"
        );
        a.close().await;
    }

    /// 181 fix round: an ACP harness session is a process on this engine.
    #[tokio::test]
    async fn move_refused_for_a_harness_session() {
        let a = engine("a").await;
        let id = SessionId::new();
        a.store
            .create_session(id, "m", None, None, None)
            .await
            .unwrap();
        a.store.set_acp(id, "omp", None, &[]).await.unwrap();
        let e = refused_locally(&a, id, &[]).await;
        assert_eq!(
            e,
            "this session runs the omp harness as a process on this engine, so it cannot move"
        );
        a.close().await;
    }

    /// 181 fix round: the target fails after A's release drain (here: A's tail
    /// never reaches B). A still owns the session at its epoch, says so, and
    /// the drained `suspended{release}` turn is the one it resumes here.
    #[tokio::test]
    async fn failed_move_resumes_on_the_origin() {
        let (a, b) = (engine("a").await, engine("b").await);
        let id = SessionId::new();
        a.store
            .create_session(id, "m", None, None, None)
            .await
            .unwrap();
        b.pull(&a).await;
        let turn = open_turn_on(&a.store, id).await;

        let (got, resumed) = move_over(&a, &b, id, false).await;
        let e = got.unwrap_err();
        assert!(
            e.starts_with("Move to b failed: b refused: owner a busy: tail not replicated")
                && e.ends_with(". It continues here."),
            "{e}"
        );
        assert_eq!(resumed, None);
        assert_eq!(a.store.fence(id).await.unwrap(), Some((1, "a".into())));
        assert_eq!(b.store.fence(id).await.unwrap(), Some((1, "a".into())));
        let plan = a.store.resume_plan(id, turn).await.unwrap();
        assert_eq!(
            (plan.kind, plan.cause),
            (ResumeKind::Suspended, ResumeCause::Transfer)
        );
        assert_eq!(continue_here(&a.store, id).await, Some(turn));
        a.store
            .append(id, &agent_msg("still writable on a"))
            .await
            .unwrap();
        a.close().await;
        b.close().await;
    }

    /// 181 fix round: `moved` binds to the owner. From anyone else (a local
    /// client: no caller; another peer) it is the user's take — at once, the
    /// open turn resumed as a takeover — never the owner's handover.
    #[tokio::test]
    async fn moved_reason_from_a_non_owner_is_treated_as_user() {
        let (a, b) = (engine("a").await, engine("b").await);
        for caller in [None, Some(PeerId("c".into()))] {
            let id = SessionId::new();
            a.store
                .create_session(id, "m", None, None, None)
                .await
                .unwrap();
            open_turn_on(&a.store, id).await;
            b.pull(&a).await;
            let mut take = b.take(None, vec![], vec![peer("a", false, now_ms())]);
            take.caller = caller;
            let (got, resumed) = take.take(id, TakeReason::Moved).await.unwrap();
            assert_eq!(got, ("b".into(), 2));
            assert_eq!(resumed, ResumeOnTake::Resume(ResumeCause::Takeover));
        }
        a.close().await;
        b.close().await;
    }

    /// 181 fix round, pinned as it is today (no tie-break; `181-notes.md`): A
    /// moves S to B while C takes it too — both from epoch 1, so both write
    /// epoch 2. Each applies the other's row (an equal epoch passes the
    /// fence), neither demotes, each keeps its own fence and writes on, and
    /// each `sessions` row names the *other* engine: two owners at epoch 2.
    #[tokio::test]
    async fn concurrent_take_at_equal_epoch_is_not_tie_broken() {
        let (a, b, c) = (engine("a").await, engine("b").await, engine("c").await);
        let id = SessionId::new();
        a.store
            .create_session(id, "m", None, None, None)
            .await
            .unwrap();
        c.pull(&a).await;
        b.pull(&a).await;
        let (got, _) = move_over(&a, &b, id, true).await;
        assert_eq!(got, Ok(("b".into(), 2)));
        let got = c
            .take(None, vec![], vec![peer("a", false, now_ms())])
            .run(id, TakeReason::User)
            .await;
        assert_eq!(got, Ok(("c".into(), 2)));
        // Each side's take, then its Moved row (183 §5.3).
        assert_eq!(b.pull(&c).await, [Applied::Applied, Applied::Applied]);
        assert_eq!(c.pull(&b).await, [Applied::Applied, Applied::Applied]);
        assert_eq!(b.store.fence(id).await.unwrap(), Some((2, "b".into())));
        assert_eq!(c.store.fence(id).await.unwrap(), Some((2, "c".into())));
        assert_eq!(b.store.owner(id).await.unwrap(), Some(("c".into(), 2)));
        assert_eq!(c.store.owner(id).await.unwrap(), Some(("b".into(), 2)));
        assert!(b.store.append(id, &agent_msg("b writes")).await.is_ok());
        assert!(c.store.append(id, &agent_msg("c writes")).await.is_ok());
        a.close().await;
        b.close().await;
        c.close().await;
    }
}
