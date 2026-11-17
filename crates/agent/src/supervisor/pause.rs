//! Design doc 179 (PAUSE-SUSPEND): pausing an agent suspends its desk's
//! session tree the way a graceful quit does; un-pausing resumes it.
//!
//! - [`PauseTree`]: the tree (desk + every descendant) is held first —
//!   [`Agents::route`] / `enqueue_peer` keep every input for it in
//!   `Agents::paused`, a human's is refused (`input`) — its reminder timers
//!   stop (the rows stay), then each live session is drained off the mailbox
//!   as a release drain is (`transfer::drain_session`, reason [`PAUSED`]):
//!   `suspended{paused, ran_ms}`, no new model call, the in-flight one
//!   finishes inside `durability.drain_secs`, its jobs go, the actor stops.
//! - [`ResumeTree`]: waits for a pause still draining, then lifts the hold;
//!   every open turn of the tree resumes through `resume_turn` (desk first,
//!   one `resume_parallel` semaphore, 178's remaining caps), its reminders
//!   re-arm, held inputs replay in arrival order, then the `accepted`
//!   exchanges addressed to the tree (what a restart kept durable).
//! - D8: at boot every paused agent's desk tree is held from the agent files
//!   ([`paused_desks`], before reminders re-arm and spared results go) and
//!   its exchanges are spared from the restart accounting; the quit drain
//!   leaves a held tree's journal alone, so a pause's `ran_ms` stays.

use std::collections::HashSet;
use std::path::Path;
use std::sync::Arc;
use std::time::{Duration, Instant};

use kameo::actor::ActorRef;
use kameo::message::{Context, Message};
use kameo::reply::DelegatedReply;
use runtime::{SessionId, TurnId};
use tokio::sync::Semaphore;
use tools::{JOBS_ACTOR, Jobs};

use super::transfer::{DrainSession, drain_session};
use super::{Agents, Held, LiveActor};
use crate::communication::PeerAddress;
use crate::store::Store;
use crate::store::durability::OpenTurnRow;

/// The `suspended{reason}` and `TurnSuspended.reason` of a pause.
pub const PAUSED: &str = "paused";

/// `ask` → [`PauseReport`]: suspend `desk`'s session tree and hold every
/// input to it until [`ResumeTree`]. Idempotent: a held tree with nothing
/// running suspends nothing.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct PauseTree {
    /// The agent's desk session (the tree's root).
    pub desk: SessionId,
}

/// What [`PauseTree`] suspended.
#[derive(Clone, Debug, Default, PartialEq, Eq, kameo::Reply)]
pub struct PauseReport {
    /// `(session, turn)` of every turn the pause left open, desk first.
    pub suspended: Vec<(SessionId, TurnId)>,
}

/// `ask` → [`ResumeReport`]: lift `desk`'s hold, resume every open turn of
/// its tree, replay what waited. Sent while the pause still drains, the
/// reply comes once the drained actors are stopped.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct ResumeTree {
    /// The agent's desk session.
    pub desk: SessionId,
}

/// What [`ResumeTree`] did.
#[derive(Clone, Debug, Default, PartialEq, Eq, kameo::Reply)]
pub struct ResumeReport {
    /// `(session, turn)` handed to `resume_turn`, desk first.
    pub resumed: Vec<(SessionId, TurnId)>,
    /// Held inputs and kept exchanges routed again.
    pub replayed: u32,
}

/// A pause still draining: its drain deadline and the un-pauses waiting.
pub(super) struct Pausing {
    deadline: Instant,
    waiting: Vec<kameo::reply::ReplySender<ResumeReport>>,
}

/// `ask` → `bool` (179 D9): a turn of `desk`'s tree is open — the Agents
/// view's `suspended mid-turn` while the agent is paused.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct TreeSuspended(pub SessionId);

/// `ask` → `Vec<(desk, agent)>`: every held tree, with the agent its desk
/// row is marked for (the roster reconciles holds it did not place: a boot
/// hold, an invalid or removed file).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct HeldDesks;

/// `ask` → `()`: the agent of `desk` is gone (its file removed). The hold
/// lifts without a resume: the tree's open turns are closed `cancelled`, so
/// no later boot resumes them with nobody's charter.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct ReleaseTree {
    /// The removed agent's desk session.
    pub desk: SessionId,
}

/// Internal `ask` from the pause's drain task: the drained actors are
/// stopped — forget the ones still registered (a session re-created since
/// is kept), then answer the un-pauses that arrived meanwhile.
struct TreeStopped {
    desk: SessionId,
    /// `(session, the actor stopped, a turn was cut)`.
    stopped: Vec<(SessionId, LiveActor, bool)>,
}

/// The agent file's front matter (`agents/<name>.md`, 67 §6.1) as the
/// roster parses it — gray_matter YAML, CRLF line ends read as LF:
/// `(data, body)`. `Err` is the user-facing `front matter: …` reason.
pub fn front_matter<T: serde::de::DeserializeOwned>(
    text: &str,
) -> Result<(Option<T>, String), String> {
    let text = text.replace("\r\n", "\n");
    let parsed = gray_matter::Matter::<gray_matter::engine::YAML>::new()
        .parse::<T>(&text)
        .map_err(|e| format!("front matter: {e}"))?;
    Ok((parsed.data, parsed.content))
}

/// 179 D8: the agent file `text` says `paused` — [`front_matter`]'s
/// reading of the key, the roster's own; a front matter that does not parse
/// is no pause.
pub fn agent_file_paused(text: &str) -> bool {
    #[derive(serde::Deserialize)]
    struct Paused {
        paused: Option<bool>,
    }
    front_matter::<Paused>(text)
        .ok()
        .and_then(|(d, _)| d)
        .and_then(|d| d.paused)
        .unwrap_or(false)
}

/// 179 D8 at boot: `(agent, desk)` of every agent whose file says paused
/// and whose desk session exists here (`theuth` is reserved: no roster file).
pub(super) async fn paused_desks(store: &Store, data_dir: &Path) -> Vec<(String, SessionId)> {
    let Ok(dir) = std::fs::read_dir(data_dir.join("agents")) else {
        return Vec::new();
    };
    let mut out = Vec::new();
    for entry in dir.flatten() {
        let path = entry.path();
        let Some(name) = path
            .file_stem()
            .and_then(|s| s.to_str())
            .filter(|_| path.extension().is_some_and(|x| x == "md"))
        else {
            continue;
        };
        if name == "theuth" || !std::fs::read_to_string(&path).is_ok_and(|t| agent_file_paused(&t))
        {
            continue;
        }
        match store.desk_session(name).await {
            Ok(Some(desk)) => out.push((name.to_owned(), desk)),
            Ok(None) => {}
            Err(e) => tracing::warn!(agent = %name, "179: desk session: {e:#}"),
        }
    }
    out
}

/// 182 M2a: on an engine with paired mesh peers, every desk tree with an open
/// turn here is held at boot like a paused one — the roster lifts each hold
/// once the first sync with the mesh is done (it resumes the tree, or closes
/// the turn when the task it served moved to another engine meanwhile).
/// Without peers nothing is held. `(agent, desk)`, `already` skipped.
pub(super) async fn sync_held(
    store: &Store,
    data_dir: &Path,
    already: &[(String, SessionId)],
) -> Vec<(String, SessionId)> {
    if !mesh::has_stored_peers(data_dir).await {
        return Vec::new();
    }
    let open = match store.open_turns(crate::store::Scope::Here).await {
        Ok(open) => open,
        Err(e) => {
            tracing::error!("182: open turns: {e:#}");
            return Vec::new();
        }
    };
    let mut out: Vec<(String, SessionId)> = Vec::new();
    for (session, _, _) in open {
        let root = store.root_of(session).await.unwrap_or(session);
        if already.iter().chain(out.iter()).any(|(_, d)| *d == root) {
            continue;
        }
        if let Ok(Some(row)) = store.session(root).await
            && let Some(agent) = row.desk
        {
            out.push((agent, root));
        }
    }
    out
}

/// 179 D8, the engine start's restart accounting: every `accepted` exchange
/// fails `interrupted_by_restart` (50 §7) except those addressed to a paused
/// agent's tree, which its un-pause delivers. Returns [`paused_desks`] plus
/// (182 M2a) [`sync_held`].
pub(super) async fn boot_accounting(store: &Store, data_dir: &Path) -> Vec<(String, SessionId)> {
    let mut paused = paused_desks(store, data_dir).await;
    for (agent, desk) in sync_held(store, data_dir, &paused).await {
        tracing::info!(%agent, %desk, "182: desk tree with an open turn held until the first sync with the mesh");
        paused.push((agent, desk));
    }
    let mut spare = Vec::new();
    for (agent, desk) in &paused {
        tracing::info!(%agent, %desk, "179: desk tree held at boot");
        spare.extend(tree_in(store, *desk).await);
    }
    match store
        .fail_interrupted_except("interrupted_by_restart", &spare)
        .await
    {
        Ok(i) if !i.exchanges.is_empty() || !i.tasks.is_empty() => tracing::info!(
            "communication: {} accepted exchange(s) and {} task(s) failed: interrupted_by_restart",
            i.exchanges.len(),
            i.tasks.len()
        ),
        Ok(_) => {}
        Err(e) => tracing::error!("communication restart accounting: {e:#}"),
    }
    // 191 (D4): the holds lived in memory and the restart failed them; their mirrored rows go.
    match store.clear_own_contact_holds().await {
        Ok(0) => {}
        Ok(n) => tracing::info!("191: {n} first-contact hold row(s) of the last run removed"),
        Err(e) => tracing::error!("191: remove the last run's hold rows: {e:#}"),
    }
    // 199 (Restart): `publishing` rows resolved by inode before `files/partial/` is emptied;
    // unfinished downloads restart from byte 0. The files of the exchanges failed above
    // fail at the file service's first pass (no notice; failed receipts).
    #[cfg(unix)]
    crate::communication::files::service::recover(store, data_dir).await;
    paused
}

/// `desk` and every descendant (advisors included), pre-order.
pub(super) async fn tree_in(store: &Store, desk: SessionId) -> Vec<SessionId> {
    let mut tree = vec![desk];
    match store.descendants_incl(desk, true).await {
        Ok(rows) => tree.extend(rows.into_iter().map(|(_, s)| s.id)),
        Err(e) => tracing::error!(%desk, "179: descendants: {e:#}"),
    }
    tree
}

/// The same actor (a session stopped by a drain may have been re-created since).
fn same(a: &LiveActor, b: &LiveActor) -> bool {
    match (a, b) {
        (LiveActor::Native(x), LiveActor::Native(y)) => x.id() == y.id(),
        (LiveActor::Acp(x), LiveActor::Acp(y)) => x.id() == y.id(),
        _ => false,
    }
}

impl Agents {
    async fn tree_of(&self, desk: SessionId) -> Vec<SessionId> {
        tree_in(&self.store, desk).await
    }

    /// The held tree `session` belongs to, if any (free while no agent is paused).
    pub(super) async fn paused_root(&self, session: SessionId) -> Option<SessionId> {
        if self.paused.is_empty() {
            return None;
        }
        if self.paused.contains_key(&session) {
            return Some(session);
        }
        let root = self.store.root_of(session).await.ok()?;
        self.paused.contains_key(&root).then_some(root)
    }

    /// The agent `root` (a held desk) is marked for; "the agent" without one.
    pub(super) async fn agent_of_desk(&self, root: SessionId) -> String {
        match self.store.session(root).await {
            Ok(Some(row)) => row.desk.unwrap_or_else(|| "the agent".into()),
            _ => "the agent".into(),
        }
    }

    /// Keep `held` for `session` of the held tree `root` until [`ResumeTree`].
    pub(super) fn hold(&mut self, root: SessionId, session: SessionId, held: Held) {
        tracing::debug!(%session, %root, "179: input held: the agent is paused");
        self.paused.entry(root).or_default().push((session, held));
    }

    /// The roots currently held (the quit drain skips their turns).
    pub(super) fn held_roots(&self) -> HashSet<SessionId> {
        self.paused.keys().copied().collect()
    }

    /// 179: a `cancelled` turn end of `session` is a pause's stop, not a
    /// turn end — while its tree is held, or (an ACP child's cancel can land
    /// after the stop and a quick un-pause) the one its stop left pending in
    /// `acp_cut`, paired with [`TreeStopped`] in either order.
    pub(super) async fn cut_by_pause(&mut self, session: SessionId) -> bool {
        if self.paused_root(session).await.is_some() {
            let acp = matches!(self.sessions.get(&session), Some(l) if matches!(l.actor, LiveActor::Acp(_)));
            if !self.acp_cut.remove(&session) && acp {
                self.acp_cut.insert(session);
            }
            return true;
        }
        self.acp_cut.remove(&session)
    }

    /// The boot pass's open turns minus the held trees' (ACP `(parent,
    /// child)` pairs and native rows alike): those wait for the un-pause.
    pub(super) async fn unpaused_turns(
        &self,
        acp: Vec<(Option<SessionId>, SessionId)>,
        native: Vec<OpenTurnRow>,
    ) -> (Vec<(Option<SessionId>, SessionId)>, Vec<OpenTurnRow>) {
        if self.paused.is_empty() {
            return (acp, native);
        }
        let mut keep_acp = Vec::with_capacity(acp.len());
        for row in acp {
            if self.paused_root(row.1).await.is_none() {
                keep_acp.push(row);
            }
        }
        let mut keep = Vec::with_capacity(native.len());
        for row in native {
            if self.paused_root(row.0).await.is_none() {
                keep.push(row);
            }
        }
        (keep_acp, keep)
    }

    /// Resume each of `sessions`' open turns. A live native actor (one
    /// loaded while paused, by a model change say) resumes its suspended
    /// turn in place; a live ACP child's harness continues on its own.
    async fn resume_open_turns(&mut self, sessions: &[SessionId]) -> Vec<(SessionId, TurnId)> {
        let slots = Arc::new(Semaphore::new(self.cfg.durability.resume_parallel.max(1)));
        let mut resumed = Vec::new();
        for &session in sessions {
            let open = match self.live(session) {
                Some(LiveActor::Acp(_)) => None,
                _ => self.store.open_turn(session).await.ok().flatten(),
            };
            let Some((turn, _)) = open else {
                // Nothing re-prompts it: a cut marker would only swallow its
                // next real cancel. A re-prompted child's clears on its start.
                self.acp_cut.remove(&session);
                continue;
            };
            self.resume_turn(session, turn, Some(slots.clone()), None, None)
                .await;
            resumed.push((session, turn));
        }
        resumed
    }

    /// [`ResumeTree`]'s body once no pause of `desk` drains.
    async fn resume_tree(&mut self, desk: SessionId) -> ResumeReport {
        let held = self.paused.remove(&desk).unwrap_or_default();
        let tree = self.tree_of(desk).await;
        let resumed = self.resume_open_turns(&tree).await;
        // The pause stopped the tree's reminder timers; the rows stayed.
        for &session in &tree {
            self.rearm_reminders_for(Some(session)).await;
        }
        let mut replayed = held.len() as u32;
        let in_memory: HashSet<uuid::Uuid> = held
            .iter()
            .filter_map(|(_, h)| match h {
                Held::Peer(p) => Some(p.record.receipt.id),
                Held::Routed { .. } => None,
            })
            .collect();
        for (session, h) in held {
            self.replay(session, vec![h]).await;
        }
        // D6 durable: what a local peer or child sent the tree and nobody
        // consumed — across a restart too (spared by the boot accounting).
        for &session in &tree {
            let rows = self
                .store
                .pending_exchanges(session, 64)
                .await
                .unwrap_or_else(|e| {
                    tracing::error!(%session, "179: pending exchanges: {e:#}");
                    Vec::new()
                });
            for r in rows {
                let Some(from) = PeerAddress::local(&r.from) else {
                    continue;
                };
                if in_memory.contains(&r.receipt.id)
                    || r.request_scope.is_some()
                    || self.enqueued.contains(&r.receipt.id)
                {
                    continue;
                }
                // Admitted again under the policy as it is now: an admission
                // from before a restart carries a generation that no longer
                // exists, and the intake would suppress it as revoked.
                self.mirror(from).await;
                let generation = self.communication.0.write().route(from, session);
                if let Ok(g) = generation
                    && let Err(e) = self
                        .store
                        .readmit_exchange(r.receipt.id, g.revision() as i64)
                        .await
                {
                    tracing::warn!(%session, id = %r.receipt.id, "179: exchange not re-admitted: {e:#}");
                }
                replayed += 1;
                self.deliver_spared(r).await;
            }
        }
        tracing::info!(%desk, turns = ?resumed, replayed, "179: un-paused: session tree resumed");
        ResumeReport { resumed, replayed }
    }
}

impl Message<PauseTree> for Agents {
    type Reply = DelegatedReply<PauseReport>;

    /// Off the mailbox, as `DrainSession`: a turn ending inside the window
    /// asks this actor. The hold is in place before anything is drained.
    async fn handle(&mut self, m: PauseTree, ctx: &mut Context<Self, Self::Reply>) -> Self::Reply {
        let (reply, tx) = ctx.reply_sender();
        self.paused.entry(m.desk).or_default();
        let deadline = Instant::now() + Duration::from_secs(self.cfg.durability.drain_secs);
        self.pausing.entry(m.desk).or_insert_with(|| Pausing {
            deadline,
            waiting: Vec::new(),
        });
        let tree = self.tree_of(m.desk).await;
        // Reminder rows stay; a timer firing into the held tree would only
        // land in memory. `ResumeTree` re-arms them.
        if let Ok(Some(jobs)) = ActorRef::<Jobs>::lookup(JOBS_ACTOR) {
            for &session in &tree {
                let _ = jobs.tell(tools::CancelReminders { session }).await;
            }
        }
        let live: Vec<_> = tree
            .iter()
            .filter_map(|s| self.live(*s).map(|l| (*s, l.clone())))
            .collect();
        let (store, rt, me) = (self.store.clone(), self.rt.clone(), ctx.actor_ref().clone());
        let desk = m.desk;
        tokio::spawn(async move {
            // Every session drains at once against the one deadline.
            let drains: Vec<_> = live
                .into_iter()
                .map(|(session, actor)| {
                    let (store, rt) = (store.clone(), rt.clone());
                    let drain = DrainSession {
                        session,
                        reason: PAUSED,
                        deadline,
                    };
                    tokio::spawn(async move {
                        let r = drain_session(&store, &rt, Some(actor.clone()), drain).await;
                        let turn = match r.suspended {
                            true => store.open_turn(session).await.ok().flatten().map(|t| t.0),
                            false => None,
                        };
                        (session, actor, turn)
                    })
                })
                .collect();
            let mut stopped = Vec::with_capacity(drains.len());
            let mut suspended = Vec::new();
            for d in drains {
                if let Ok((session, actor, turn)) = d.await {
                    stopped.push((session, actor, turn.is_some()));
                    suspended.extend(turn.map(|t| (session, t)));
                }
            }
            tracing::info!(%desk, turns = ?suspended, "179: paused: session tree suspended");
            let _ = me.ask(TreeStopped { desk, stopped }).await;
            if let Some(tx) = tx {
                tx.send(PauseReport { suspended });
            }
        });
        reply
    }
}

impl Message<TreeStopped> for Agents {
    type Reply = ();

    async fn handle(&mut self, m: TreeStopped, _: &mut Context<Self, Self::Reply>) {
        for (session, actor, cut) in m.stopped {
            // An ACP child's cancel arrives as a `TurnFinished` — before or
            // after this stop; `cut_by_pause` pairs the two.
            if cut && matches!(actor, LiveActor::Acp(_)) && !self.acp_cut.remove(&session) {
                self.acp_cut.insert(session);
            }
            if !self
                .sessions
                .get(&session)
                .is_some_and(|l| same(&l.actor, &actor))
            {
                continue;
            }
            self.sessions.remove(&session);
            self.running.remove(&session);
            // 178 F5: the timer and run clock go with the actor; the resume
            // arms the turn again with what is left of its cap.
            if let Some(timer) = self.child_turns.remove(&session).and_then(|ct| ct.timer) {
                timer.abort();
            }
            crate::durability::run_clock::stop_session(session);
            self.endless.detach(session);
        }
        // Un-pauses that arrived while the tree drained resume it now; when
        // the wait for this stop timed out ([`UnpauseOverdue`]) the hold is
        // already lifted and what stopped late resumes here.
        let Some(Pausing { waiting, .. }) = self.pausing.remove(&m.desk) else {
            return;
        };
        if !waiting.is_empty() {
            let report = self.resume_tree(m.desk).await;
            for tx in waiting {
                tx.send(report.clone());
            }
        } else if !self.paused.contains_key(&m.desk) {
            let tree = self.tree_of(m.desk).await;
            let resumed = self.resume_open_turns(&tree).await;
            tracing::info!(desk = %m.desk, turns = ?resumed, "179: the late stop is done; its turns resumed");
        }
    }
}

/// How long past `drain_secs` a queued un-pause waits for the pause's stop.
const UNPAUSE_GRACE: Duration = Duration::from_secs(5);

/// Self-sent [`UNPAUSE_GRACE`] past a pause's drain deadline once an
/// un-pause waits on it.
struct UnpauseOverdue {
    desk: SessionId,
}

impl Message<UnpauseOverdue> for Agents {
    type Reply = ();

    /// A stop stuck past its window must not park the roster: resume what
    /// is stopped and answer; [`TreeStopped`] resumes the rest when it lands.
    /// A timer of an earlier pause of the desk finds a later deadline.
    async fn handle(&mut self, m: UnpauseOverdue, _: &mut Context<Self, Self::Reply>) {
        let Some(p) = self.pausing.get_mut(&m.desk) else {
            return;
        };
        if p.waiting.is_empty() || Instant::now() < p.deadline + UNPAUSE_GRACE {
            return;
        }
        let waiting = std::mem::take(&mut p.waiting);
        tracing::warn!(desk = %m.desk, "179: the pause's stop overran its drain window; resuming what is stopped");
        let report = self.resume_tree(m.desk).await;
        for tx in waiting {
            tx.send(report.clone());
        }
    }
}

impl Message<ResumeTree> for Agents {
    type Reply = DelegatedReply<ResumeReport>;

    async fn handle(&mut self, m: ResumeTree, ctx: &mut Context<Self, Self::Reply>) -> Self::Reply {
        let (reply, tx) = ctx.reply_sender();
        // MUST-FIX 1: resuming while the drained actors still run would skip
        // them as live, and their stop would then leave the turns unheld and
        // never resumed — wait for the stop, at most the drain window and
        // [`UNPAUSE_GRACE`].
        if let Some(p) = self.pausing.get_mut(&m.desk) {
            tracing::info!(desk = %m.desk, "179: un-pause while the pause drains; resuming after the stop");
            if let Some(tx) = tx {
                p.waiting.push(tx);
            }
            let (me, desk, at) = (ctx.actor_ref().clone(), m.desk, p.deadline + UNPAUSE_GRACE);
            tokio::spawn(async move {
                tokio::time::sleep_until(at.into()).await;
                let _ = me.tell(UnpauseOverdue { desk }).await;
            });
            return reply;
        }
        let report = self.resume_tree(m.desk).await;
        if let Some(tx) = tx {
            tx.send(report);
        }
        reply
    }
}

impl Message<TreeSuspended> for Agents {
    type Reply = bool;

    async fn handle(&mut self, m: TreeSuspended, _: &mut Context<Self, Self::Reply>) -> bool {
        self.store
            .tree_has_open_turn(m.0)
            .await
            .unwrap_or_else(|e| {
                tracing::error!(desk = %m.0, "179: tree open turn: {e:#}");
                false
            })
    }
}

impl Message<HeldDesks> for Agents {
    type Reply = Vec<(SessionId, Option<String>)>;

    async fn handle(&mut self, _: HeldDesks, _: &mut Context<Self, Self::Reply>) -> Self::Reply {
        let mut out = Vec::with_capacity(self.paused.len());
        for &desk in self.paused.keys() {
            let agent = self
                .store
                .session(desk)
                .await
                .ok()
                .flatten()
                .and_then(|r| r.desk);
            out.push((desk, agent));
        }
        out
    }
}

impl Message<ReleaseTree> for Agents {
    type Reply = ();

    async fn handle(&mut self, m: ReleaseTree, _: &mut Context<Self, Self::Reply>) {
        if self.paused.remove(&m.desk).is_none() {
            return;
        }
        // A stop still draining finds no entry and resumes nothing.
        self.pausing.remove(&m.desk);
        let mut closed = Vec::new();
        for session in self.tree_of(m.desk).await {
            if let Ok(Some((turn, _))) = self.store.open_turn(session).await {
                match self.store.finish_turn(session, turn, "cancelled").await {
                    Ok(()) => closed.push((session, turn)),
                    Err(e) => tracing::warn!(%session, %turn, "179: finish_turn: {e:#}"),
                }
            }
        }
        tracing::info!(desk = %m.desk, turns = ?closed, "179: agent removed while paused: hold lifted, its open turns closed");
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn agent_file_paused_reads_the_front_matter_only() {
        assert!(agent_file_paused(
            "---\ntitle: Nap\npaused: true\n---\nCharter.\n"
        ));
        assert!(
            !agent_file_paused("---\ntitle: Ops\n---\npaused: true\n"),
            "body text"
        );
        assert!(!agent_file_paused("paused: true\n"), "no front matter");
        assert!(!agent_file_paused("---\npaused: [\n---\nbroken\n"));
    }
}
