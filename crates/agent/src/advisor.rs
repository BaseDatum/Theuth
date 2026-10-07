//! 125 §4: the `Advisors` actor — one `Link` per advised session, holding the
//! cursor and delivered-prefix fingerprint over the advised transcript, the
//! emission guard, the coalesced backlog, the failure latches and the
//! counters `advisor_status` reports. The advisor itself is an ordinary
//! native `Session` whose row carries `kind = advisor`; this actor only tells
//! it what to read (one update = one turn), decides where each `advise` note
//! lands, and watches its `AgentEvent`s.
//!
//! Actor discipline: `Agents` may `ask` this actor; this actor only ever
//! `tell`s `Agents` (routing, cancel, restart), so the two never wait on each
//! other. The advised loop `ask`s [`AdvisorStep`] (replied at once unless the
//! D5 catch-up wait parks it) and [`Advise`] (replied with the ack string).

pub mod advise;
pub mod guard;
pub mod identity;
pub mod render;

#[cfg(test)]
mod tests;

use std::collections::HashMap;
use std::sync::{Arc, LazyLock};
use std::time::Duration;

use kameo::actor::{ActorRef, Spawn};
use kameo::message::{Context, Message};
use kameo::reply::{DelegatedReply, ReplySender};
use regex::Regex;
use runtime::{Runtime, SessionId};

use crate::store::Store;
use crate::supervisor::Agents;
use crate::{
    AGENT_ACTOR, AdvisorConfig, AdvisorLink, AdvisorNoteCounts, AdvisorSeverity, AdvisorState,
    AdvisorStatus, Advisory, AdvisoryNote, AgentEvent, SyncBacklog,
};
use guard::{Guard, Verdict};
use render::{UpdateOpts, counted, fingerprint, render_update};

/// Registered name of the actor.
pub const ADVISORS_ACTOR: &str = "advisors";

/// D5: the longest a terminal step waits for the backlog.
pub const CATCH_UP_WAIT: Duration = Duration::from_secs(30);

/// Retriable failures before a backlog is dropped; dropped backlogs before `halted`.
const MAX_FAILURES: u32 = 3;
const MAX_DROPPED: u32 = 3;
/// Pause between retries of a failed review.
const RETRY_AFTER: Duration = Duration::from_secs(1);
/// Assistant text kept per review for the quarantine scan.
const TEXT_CAP: usize = 64 * 1024;

/// Card text of the second-quarantine warning.
pub const QUARANTINE_WARNING: &str = "advisor quarantined twice — reset";

static PERMANENT: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r"(?i)invalid_request|model.*not.*found|is not supported|does not exist").unwrap()
});
static QUOTA: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r"(?i)\b429\b|rate limit|quota|usage limit|insufficient_quota").unwrap()
});
static OVERRIDE: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r"(?i)ignore (all )?(prior|previous|earlier) (user )?instructions").unwrap()
});
static DESTRUCTIVE: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r"\brm\b[^\n]*(-[a-zA-Z]*r[a-zA-Z]*f|-[a-zA-Z]*f[a-zA-Z]*r)").unwrap()
});
static DENIAL: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"(?i)deny (this|it|the request) if (asked|questioned)").unwrap());

/// A quarantine pattern in `text` that the update it answers does not contain.
pub fn injected_pattern(text: &str, update: &str) -> bool {
    [&*OVERRIDE, &*DESTRUCTIVE, &*DENIAL]
        .iter()
        .any(|re| re.find(text).is_some_and(|m| !update.contains(m.as_str())))
}

/// How an unknown-tool refusal reads in `ToolCallFinished.output` (`run_batch`).
const UNKNOWN_TOOL: &str = "is not available in this session";

// ---- messages from `Agents` --------------------------------------------------

/// `ask` → [`AdvisorStatus`]: a link was created or re-picked by `Agents`
/// (the advisor session exists); seed the cursor at the current transcript
/// length (D1: no replay) and persist.
pub struct Attach {
    /// The link as `Agents` wrote it (cursor and state are seeded here).
    pub link: AdvisorLink,
    /// The advisor model's context window, 0 when unknown.
    pub context_limit: u32,
}

/// `ask` → `()`: `set_advisor(Off)` — cancel the review, delete the link.
pub struct Detach(pub SessionId);

/// `ask` → [`AdvisorStatus`] of an advised session (`Off` when none).
pub struct StatusOf(pub SessionId);

/// `tell`: live `advisor.*` tuning changed.
pub struct SetDefaults(pub AdvisorConfig);

// ---- messages from the advised loop -----------------------------------------

/// `ask` → `()`: one agent-core step of `session` landed in the store
/// (125 §4.1 item 4, §4.12). Replied at once; a terminal step
/// (`will_continue: false`) is parked while the backlog is at or above
/// `advisor.sync_backlog`, at most [`CATCH_UP_WAIT`] (the caller's timeout).
pub struct AdvisorStep {
    /// The advised session.
    pub session: SessionId,
    /// Opener seq of the advised turn under review.
    pub turn: Option<i64>,
    /// More tool steps follow in the advised turn.
    pub will_continue: bool,
}

/// `ask` → ack string: the advisor session called `advise`.
pub struct Advise {
    /// The advisor session calling.
    pub advisor: SessionId,
    /// The note text.
    pub note: String,
    /// Its severity (omitted at the tool = nit).
    pub severity: AdvisorSeverity,
}

/// Internal `tell`: retry the failed review of `advisor` (1 s later).
struct Retry(SessionId);

// ---- messages to `Agents` (handled in `supervisor/advisor.rs`) --------------

/// `tell`: route one update into the advisor session as an automatic input.
pub struct AdvisorInput {
    /// The advisor session.
    pub advisor: SessionId,
    /// The rendered update.
    pub text: String,
}

/// `tell`: hand a card to the advised session's actor — `steer` lands it as
/// a steer (opens a turn when idle), else passively (125 §4.10).
pub struct DeliverAdvisory {
    /// The advised session.
    pub session: SessionId,
    /// The typed card.
    pub advisory: Advisory,
    /// Its model-facing text (`advise::card_text`).
    pub text: String,
    /// Steer (concern outside the immune window, blocker) vs aside/preserve.
    pub steer: bool,
}

// ---- state ------------------------------------------------------------------

/// What to do once the advisor's cancelled turn has returned.
enum Reset {
    /// `RestartTurn` at the advisor's first opener with this text.
    Replay(String),
}

/// The advisor turn in flight.
struct Review {
    step: u32,
    turn: Option<i64>,
    will_continue: bool,
    /// Nits and immune-window concerns held for one card at the end.
    held: Vec<AdvisoryNote>,
    /// Any `advise` call landed.
    advised: bool,
    /// The turn asked for a tool it was not given.
    unknown_tool: bool,
    /// A note matched a quarantine pattern.
    tainted: bool,
    /// Assistant text streamed so far (for the quarantine scan).
    text: String,
}

impl Review {
    fn new(step: u32, turn: Option<i64>, will_continue: bool) -> Self {
        Self {
            step,
            turn,
            will_continue,
            held: Vec::new(),
            advised: false,
            unknown_tool: false,
            tainted: false,
            text: String::new(),
        }
    }
}

struct Link {
    row: AdvisorLink,
    prefix: Vec<u64>,
    guard: Guard,
    steps: u32,
    immune_until: u32,
    /// Steps landed since the last update was sent.
    backlog: u32,
    pending_turn: Option<i64>,
    pending_continue: bool,
    reviewing: Option<Review>,
    last_update: String,
    /// Seq of the advisor's newest update row (`TurnStarted.turn_seq`).
    last_opener: Option<i64>,
    preface: Option<String>,
    failures: u32,
    dropped: u32,
    quarantines: u32,
    reprimed: bool,
    after_cancel: Option<Reset>,
    /// A paused batch to regenerate on re-pick.
    retained: bool,
    waiters: Vec<ReplySender<()>>,
    reasoning: bool,
    notes: AdvisorNoteCounts,
    last_note: Option<AdvisoryNote>,
    tokens_in: u64,
    tokens_out: u64,
    context_used: u32,
    context_limit: u32,
    detail: Option<String>,
    last_error: Option<String>,
}

impl Link {
    fn new(row: AdvisorLink, max_notes: u8, context_limit: u32) -> Self {
        Self {
            row,
            prefix: Vec::new(),
            guard: Guard::new(max_notes),
            steps: 0,
            immune_until: 0,
            backlog: 0,
            pending_turn: None,
            pending_continue: false,
            reviewing: None,
            last_update: String::new(),
            last_opener: None,
            preface: None,
            failures: 0,
            dropped: 0,
            quarantines: 0,
            reprimed: false,
            after_cancel: None,
            retained: false,
            waiters: Vec::new(),
            reasoning: true,
            notes: AdvisorNoteCounts::default(),
            last_note: None,
            tokens_in: 0,
            tokens_out: 0,
            context_used: 0,
            context_limit,
            detail: None,
            last_error: None,
        }
    }

    fn release_waiters(&mut self) {
        for w in self.waiters.drain(..) {
            w.send(());
        }
    }

    fn status(&self, cost: Option<crate::cost::CostBucket>) -> AdvisorStatus {
        AdvisorStatus {
            session: self.row.session,
            advisor: Some(self.row.advisor),
            state: self.row.state,
            model: Some(self.row.model.clone()),
            effort: self.row.effort.clone(),
            notes: self.notes.clone(),
            context_used: self.context_used,
            context_limit: self.context_limit,
            tokens_in: self.tokens_in,
            tokens_out: self.tokens_out,
            cost,
            step: self.steps,
            backlog: self.backlog,
            last_note: self.last_note.clone(),
            detail: self.detail.clone(),
        }
    }
}

/// The actor. Start with [`Advisors::start`].
pub struct Advisors {
    store: Store,
    rt: Runtime,
    cfg: AdvisorConfig,
    /// `Agents::redact` over the rendered update.
    redact: Arc<dyn Fn(&str) -> String + Send + Sync>,
    /// `AgentConfig.compact_at`: the advisor re-primes at this fraction of its window.
    compact_at: f32,
    links: HashMap<SessionId, Link>,
    by_advisor: HashMap<SessionId, SessionId>,
}

impl kameo::Actor for Advisors {
    type Args = Self;
    type Error = std::convert::Infallible;

    async fn on_start(a: Self, _: ActorRef<Self>) -> Result<Self, Self::Error> {
        Ok(a)
    }
}

impl Advisors {
    /// Load every link (125 §4.14), spawn, register and subscribe. Runs
    /// before `resume_scan` (which the activity registration triggers).
    pub async fn start(
        store: Store,
        rt: Runtime,
        cfg: AdvisorConfig,
        compact_at: f32,
        redact: Arc<dyn Fn(&str) -> String + Send + Sync>,
    ) -> anyhow::Result<ActorRef<Self>> {
        let mut me = Self {
            store,
            rt: rt.clone(),
            cfg,
            redact,
            compact_at,
            links: HashMap::new(),
            by_advisor: HashMap::new(),
        };
        me.load().await?;
        let actor = Self::spawn_with_mailbox(me, runtime::subscriber_mailbox());
        actor.register(ADVISORS_ACTOR)?;
        rt.subscribe::<Self, AgentEvent>(&actor).await;
        Ok(actor)
    }

    /// Boot: re-seed every cursor at the current rendered length with an
    /// empty prefix, mark the next update with the restart preface,
    /// `reviewing` → `idle`; `paused`/`halted` stay.
    async fn load(&mut self) -> anyhow::Result<()> {
        for mut row in self.store.advisor_links().await? {
            let was = row.cursor;
            let count = self.counted_rows(row.session).await?.len() as i64;
            row.cursor = count;
            if row.state == AdvisorState::Reviewing {
                row.state = AdvisorState::Idle;
            }
            self.store.put_advisor_link(&row).await?;
            let mut link = Link::new(row, self.cfg.max_notes_per_update, 0);
            link.preface = Some(format!(
                "_(engine restarted: steps between {was} and now were not reviewed)_"
            ));
            link.retained = link.row.state == AdvisorState::Paused;
            self.by_advisor.insert(link.row.advisor, link.row.session);
            self.links.insert(link.row.session, link);
        }
        Ok(())
    }

    async fn counted_rows(
        &self,
        session: SessionId,
    ) -> anyhow::Result<Vec<(i64, crate::HistoryEntry)>> {
        Ok(self
            .store
            .rows_since(session, 0)
            .await?
            .into_iter()
            .filter(|(_, e)| counted(e))
            .collect())
    }

    fn agents(&self) -> Option<ActorRef<Agents>> {
        ActorRef::<Agents>::lookup(AGENT_ACTOR).ok().flatten()
    }

    async fn tell_agents<M>(&self, m: M)
    where
        Agents: Message<M>,
        M: Send + 'static,
    {
        if let Some(a) = self.agents()
            && let Err(e) = a.tell(m).await
        {
            tracing::warn!("advisor → agents: {e}");
        }
    }

    async fn publish(&self, session: SessionId) {
        let Some(link) = self.links.get(&session) else {
            return;
        };
        let cost = self
            .store
            .session_cost(link.row.advisor)
            .await
            .ok()
            .flatten()
            .map(|c| c.total);
        let status = link.status(cost);
        self.rt
            .publish(AgentEvent::AdvisorChanged { session, status })
            .await;
    }

    async fn persist(&self, session: SessionId) {
        if let Some(link) = self.links.get(&session)
            && let Err(e) = self.store.put_advisor_link(&link.row).await
        {
            tracing::warn!(session = %session, "put_advisor_link: {e:#}");
        }
    }

    async fn set_state(&mut self, session: SessionId, state: AdvisorState, detail: Option<String>) {
        if let Some(link) = self.links.get_mut(&session) {
            link.row.state = state;
            link.detail = detail;
            if state != AdvisorState::Reviewing {
                link.release_waiters();
            }
        }
        self.persist(session).await;
        self.publish(session).await;
    }

    fn threshold(&self) -> Option<u32> {
        match self.cfg.sync_backlog {
            SyncBacklog::Off => None,
            SyncBacklog::One => Some(1),
            SyncBacklog::Three => Some(3),
            SyncBacklog::Five => Some(5),
        }
    }

    /// Render and send everything past the cursor as one update; detects a
    /// rewritten delivered prefix first and replays instead (125 §4.7).
    async fn send_update(&mut self, session: SessionId) {
        let rows = match self.counted_rows(session).await {
            Ok(rows) => rows,
            Err(e) => {
                tracing::warn!(session = %session, "advisor rows: {e:#}");
                return;
            }
        };
        let redact = self.redact.clone();
        let Some(link) = self.links.get_mut(&session) else {
            return;
        };
        let rewritten = link.prefix.len() > rows.len()
            || link
                .prefix
                .iter()
                .zip(rows.iter())
                .any(|(fp, (_, e))| *fp != fingerprint(e));
        if rewritten {
            // Replay the bounded current transcript: rows after the latest
            // compaction head, or everything.
            let from = rows
                .iter()
                .rposition(|(_, e)| matches!(e, crate::HistoryEntry::Compacted { .. }))
                .unwrap_or(0);
            let text = redact(&render_update(
                &rows[from..],
                UpdateOpts {
                    steps: link.backlog.max(1),
                    will_continue: link.pending_continue,
                    preface: Some("_(transcript rewritten — replaying)_"),
                    reasoning: link.reasoning,
                },
            ));
            link.prefix = rows.iter().map(|(_, e)| fingerprint(e)).collect();
            link.row.cursor = rows.len() as i64;
            link.row.state = AdvisorState::Reviewing;
            link.guard.begin_update();
            link.last_update = text.clone();
            link.backlog = 0;
            link.release_waiters();
            let advisor = link.row.advisor;
            let busy = link.reviewing.is_some();
            if busy {
                link.after_cancel = Some(Reset::Replay(text.clone()));
            } else {
                link.reviewing = Some(Review::new(
                    link.steps,
                    link.pending_turn,
                    link.pending_continue,
                ));
            }
            if busy {
                self.tell_agents(crate::Cancel {
                    session: advisor,
                    keep: None,
                })
                .await;
            } else {
                self.restart_with(session, text).await;
            }
            self.persist(session).await;
            self.publish(session).await;
            return;
        }
        let count = usize::try_from(link.row.cursor)
            .unwrap_or(0)
            .min(rows.len());
        let fresh = &rows[count..];
        if fresh.is_empty() {
            link.backlog = 0;
            link.release_waiters();
            return;
        }
        let preface = link.preface.take();
        let text = redact(&render_update(
            fresh,
            UpdateOpts {
                steps: link.backlog.max(1),
                will_continue: link.pending_continue,
                preface: preface.as_deref(),
                reasoning: link.reasoning,
            },
        ));
        link.prefix
            .extend(fresh.iter().map(|(_, e)| fingerprint(e)));
        link.row.cursor = rows.len() as i64;
        link.row.state = AdvisorState::Reviewing;
        link.guard.begin_update();
        link.last_update = text.clone();
        link.reviewing = Some(Review::new(
            link.steps,
            link.pending_turn,
            link.pending_continue,
        ));
        link.backlog = 0;
        link.release_waiters();
        let advisor = link.row.advisor;
        self.persist(session).await;
        self.publish(session).await;
        self.tell_agents(AdvisorInput { advisor, text }).await;
    }

    /// `RestartTurn` the advisor at its first opener with `text` — or, when
    /// it never had a turn, send `text` as a plain update.
    async fn restart_with(&self, session: SessionId, text: String) {
        let Some(link) = self.links.get(&session) else {
            return;
        };
        let advisor = link.row.advisor;
        match self.store.first_turn(advisor).await {
            Ok(Some(turn)) => {
                self.tell_agents(crate::EditTurn {
                    session: advisor,
                    turn,
                    text,
                    files: Vec::new(),
                })
                .await;
            }
            Ok(None) => self.tell_agents(AdvisorInput { advisor, text }).await,
            Err(e) => tracing::warn!(session = %advisor, "first_turn: {e:#}"),
        }
    }

    /// Deliver `notes` as one card.
    async fn deliver(&mut self, session: SessionId, notes: Vec<AdvisoryNote>, steer: bool) {
        if notes.is_empty() {
            return;
        }
        let Some(link) = self.links.get_mut(&session) else {
            return;
        };
        let (step, turn) = link
            .reviewing
            .as_ref()
            .map_or((link.steps, link.pending_turn), |r| (r.step, r.turn));
        if steer {
            link.immune_until = link.steps + 1 + u32::from(self.cfg.immune_turns);
        }
        let advisory = Advisory {
            advisor: link.row.advisor,
            step,
            turn,
            at_ms: chrono::Utc::now().timestamp_millis(),
            notes,
        };
        let text = advise::card_text(&advisory.notes);
        self.tell_agents(DeliverAdvisory {
            session,
            advisory,
            text,
            steer,
        })
        .await;
    }

    /// The advisor's turn ended (125 §4.13): decide under the borrow, then act.
    async fn finish_review(&mut self, session: SessionId, stop_reason: &str) {
        enum Act {
            Restart(String),
            Halt(String),
            Pause(String),
            RetryLater(SessionId),
            Dropped(String),
            Quarantine { warn: bool, text: String },
            Good { held: Vec<AdvisoryNote>, next: bool },
        }
        let Some(link) = self.links.get_mut(&session) else {
            return;
        };
        let Some(review) = link.reviewing.take() else {
            return;
        };
        let again = Review::new(review.step, review.turn, review.will_continue);
        let act = if let Some(Reset::Replay(text)) = link.after_cancel.take() {
            link.reviewing = Some(again);
            Act::Restart(text)
        } else if stop_reason == "error" {
            let msg = link.last_error.take().unwrap_or_default();
            let first = msg.lines().next().unwrap_or("").to_owned();
            let overflow = crate::compact::is_overflow(&msg);
            if PERMANENT.is_match(&msg) {
                Act::Halt(format!("advisor halted: {first}"))
            } else if QUOTA.is_match(&msg) {
                link.retained = true;
                Act::Pause(format!("advisor paused: {first}"))
            } else if overflow && !link.reprimed {
                link.reprimed = true;
                link.reviewing = Some(again);
                Act::Restart(format!(
                    "### Session update\n_(advisor context re-primed at step {})_\n{}",
                    link.steps,
                    link.last_update.trim_start_matches("### Session update")
                ))
            } else {
                if msg.contains("refus") && msg.to_lowercase().contains("reasoning") {
                    link.reasoning = false;
                }
                link.failures += 1;
                if link.failures < MAX_FAILURES && !overflow {
                    link.reviewing = Some(review);
                    Act::RetryLater(link.row.advisor)
                } else {
                    link.failures = 0;
                    link.dropped += 1;
                    link.reprimed = false;
                    if link.dropped >= MAX_DROPPED {
                        Act::Halt(format!("advisor halted: {first}"))
                    } else {
                        Act::Dropped(format!("review dropped: {first}"))
                    }
                }
            }
        } else {
            let user_stopped = matches!(stop_reason, "cancelled" | "interrupted" | "timed_out");
            let tainted = review.tainted
                || (review.unknown_tool && !review.advised)
                || injected_pattern(&review.text, &link.last_update);
            if tainted && !user_stopped {
                link.quarantines += 1;
                if link.quarantines >= 3 {
                    Act::Halt("advisor halted: quarantined repeatedly".into())
                } else {
                    link.reviewing = Some(again);
                    Act::Quarantine {
                        warn: link.quarantines == 2,
                        text: link.last_update.clone(),
                    }
                }
            } else {
                if !user_stopped {
                    link.failures = 0;
                    link.dropped = 0;
                    link.quarantines = 0;
                    link.reprimed = false;
                }
                Act::Good {
                    held: review.held,
                    next: !user_stopped,
                }
            }
        };
        match act {
            Act::Restart(text) => self.restart_with(session, text).await,
            Act::Halt(detail) => {
                self.set_state(session, AdvisorState::Halted, Some(detail))
                    .await
            }
            Act::Pause(detail) => {
                self.set_state(session, AdvisorState::Paused, Some(detail))
                    .await
            }
            Act::RetryLater(advisor) => {
                let me = ActorRef::<Self>::lookup(ADVISORS_ACTOR).ok().flatten();
                tokio::spawn(async move {
                    tokio::time::sleep(RETRY_AFTER).await;
                    if let Some(me) = me {
                        let _ = me.tell(Retry(advisor)).await;
                    }
                });
            }
            Act::Dropped(detail) => {
                self.set_state(session, AdvisorState::Idle, Some(detail))
                    .await;
                self.maybe_next(session).await;
            }
            Act::Quarantine { warn, text } => {
                if warn {
                    self.deliver(
                        session,
                        vec![AdvisoryNote {
                            severity: AdvisorSeverity::Nit,
                            note: QUARANTINE_WARNING.into(),
                        }],
                        false,
                    )
                    .await;
                }
                self.restart_with(session, text).await;
            }
            Act::Good { held, next } => {
                self.deliver(session, held, false).await;
                self.set_state(session, AdvisorState::Idle, None).await;
                if next {
                    self.maybe_next(session).await;
                }
            }
        }
    }

    async fn maybe_next(&mut self, session: SessionId) {
        if self
            .links
            .get(&session)
            .is_some_and(|l| l.backlog > 0 && l.row.state == AdvisorState::Idle)
        {
            self.send_update(session).await;
        }
    }
}

impl Message<Attach> for Advisors {
    type Reply = AdvisorStatus;

    async fn handle(&mut self, m: Attach, _: &mut Context<Self, Self::Reply>) -> AdvisorStatus {
        let session = m.link.session;
        let repick = self.links.get(&session).map(|l| l.row.advisor) == Some(m.link.advisor);
        if repick {
            let retained = {
                let link = self.links.get_mut(&session).unwrap();
                link.row.model = m.link.model;
                link.row.effort = m.link.effort;
                link.context_limit = m.context_limit;
                link.failures = 0;
                link.dropped = 0;
                link.quarantines = 0;
                std::mem::take(&mut link.retained)
            };
            self.set_state(session, AdvisorState::Idle, None).await;
            if retained {
                // D6: the paused batch is regenerated.
                let (advisor, opener, text) = {
                    let link = self.links.get_mut(&session).unwrap();
                    link.reviewing = Some(Review::new(
                        link.steps,
                        link.pending_turn,
                        link.pending_continue,
                    ));
                    link.row.state = AdvisorState::Reviewing;
                    (link.row.advisor, link.last_opener, link.last_update.clone())
                };
                match opener {
                    Some(turn) => {
                        self.tell_agents(crate::Regenerate {
                            session: advisor,
                            turn,
                        })
                        .await;
                    }
                    None => self.tell_agents(AdvisorInput { advisor, text }).await,
                }
                self.publish(session).await;
            } else {
                self.maybe_next(session).await;
            }
        } else {
            let mut row = m.link;
            row.cursor = self
                .counted_rows(session)
                .await
                .map(|r| r.len() as i64)
                .unwrap_or(0);
            row.state = AdvisorState::Idle;
            if let Err(e) = self.store.put_advisor_link(&row).await {
                tracing::warn!(session = %session, "put_advisor_link: {e:#}");
            }
            self.by_advisor.insert(row.advisor, session);
            self.links.insert(
                session,
                Link::new(row, self.cfg.max_notes_per_update, m.context_limit),
            );
            self.publish(session).await;
        }
        let link = &self.links[&session];
        let cost = self
            .store
            .session_cost(link.row.advisor)
            .await
            .ok()
            .flatten()
            .map(|c| c.total);
        link.status(cost)
    }
}

impl Message<Detach> for Advisors {
    type Reply = ();

    async fn handle(&mut self, Detach(session): Detach, _: &mut Context<Self, Self::Reply>) {
        let Some(mut link) = self.links.remove(&session) else {
            return;
        };
        self.by_advisor.remove(&link.row.advisor);
        link.release_waiters();
        link.guard.reset();
        if link.reviewing.is_some() {
            self.tell_agents(crate::Cancel {
                session: link.row.advisor,
                keep: None,
            })
            .await;
        }
        if let Err(e) = self.store.delete_advisor_link(session).await {
            tracing::warn!(session = %session, "delete_advisor_link: {e:#}");
        }
        self.rt
            .publish(AgentEvent::AdvisorChanged {
                session,
                status: AdvisorStatus {
                    session,
                    ..AdvisorStatus::default()
                },
            })
            .await;
    }
}

impl Message<StatusOf> for Advisors {
    type Reply = AdvisorStatus;

    async fn handle(
        &mut self,
        StatusOf(session): StatusOf,
        _: &mut Context<Self, Self::Reply>,
    ) -> AdvisorStatus {
        match self.links.get(&session) {
            Some(link) => {
                let cost = self
                    .store
                    .session_cost(link.row.advisor)
                    .await
                    .ok()
                    .flatten()
                    .map(|c| c.total);
                link.status(cost)
            }
            None => AdvisorStatus {
                session,
                ..AdvisorStatus::default()
            },
        }
    }
}

impl Message<SetDefaults> for Advisors {
    type Reply = ();

    async fn handle(&mut self, SetDefaults(cfg): SetDefaults, _: &mut Context<Self, Self::Reply>) {
        for link in self.links.values_mut() {
            link.guard.set_budget(cfg.max_notes_per_update);
        }
        self.cfg = cfg;
        // A lower threshold or `off` releases every parked terminal step.
        let threshold = self.threshold();
        for link in self.links.values_mut() {
            if threshold.is_none_or(|t| link.backlog < t) {
                link.release_waiters();
            }
        }
    }
}

impl Message<AdvisorStep> for Advisors {
    type Reply = DelegatedReply<()>;

    async fn handle(
        &mut self,
        m: AdvisorStep,
        ctx: &mut Context<Self, Self::Reply>,
    ) -> Self::Reply {
        let (reply, tx) = ctx.reply_sender();
        let threshold = self.threshold();
        let Some(link) = self.links.get_mut(&m.session) else {
            if let Some(tx) = tx {
                tx.send(());
            }
            return reply;
        };
        link.steps += 1;
        link.backlog += 1;
        link.pending_turn = m.turn;
        link.pending_continue = m.will_continue;
        let busy = link.reviewing.is_some();
        let active = matches!(link.row.state, AdvisorState::Idle | AdvisorState::Reviewing);
        let park =
            !m.will_continue && busy && active && threshold.is_some_and(|t| link.backlog >= t);
        match tx {
            Some(tx) if park => link.waiters.push(tx),
            Some(tx) => tx.send(()),
            None => {}
        }
        if !busy && active {
            self.send_update(m.session).await;
        } else {
            self.publish(m.session).await;
        }
        reply
    }
}

impl Message<Advise> for Advisors {
    type Reply = String;

    async fn handle(&mut self, m: Advise, _: &mut Context<Self, Self::Reply>) -> String {
        let Some(&session) = self.by_advisor.get(&m.advisor) else {
            return guard::ACK_NOISE.to_owned();
        };
        let link = self.links.get_mut(&session).unwrap();
        let mut review = link
            .reviewing
            .take()
            .unwrap_or_else(|| Review::new(link.steps, link.pending_turn, link.pending_continue));
        review.advised = true;
        if injected_pattern(&m.note, &link.last_update) {
            review.tainted = true;
            link.reviewing = Some(review);
            return guard::ACK_NOISE.to_owned();
        }
        let pending: Vec<u8> = review
            .held
            .iter()
            .map(|n| guard::rank(n.severity))
            .collect();
        let verdict = link.guard.admit(&m.note, m.severity, &pending);
        let displaced = match verdict {
            Verdict::Accept { displaced } => displaced,
            Verdict::Empty => {
                link.reviewing = Some(review);
                return guard::ACK_EMPTY.to_owned();
            }
            Verdict::Noise => {
                link.reviewing = Some(review);
                return guard::ACK_NOISE.to_owned();
            }
            Verdict::Duplicate => {
                link.reviewing = Some(review);
                return guard::ACK_DUPLICATE.to_owned();
            }
            Verdict::BudgetSpent => {
                link.reviewing = Some(review);
                return guard::ACK_BUDGET.to_owned();
            }
        };
        if let Some(i) = displaced {
            review.held.remove(i);
        }
        let note = AdvisoryNote {
            severity: m.severity,
            note: m.note.trim().to_owned(),
        };
        match m.severity {
            AdvisorSeverity::Nit => link.notes.nit += 1,
            AdvisorSeverity::Concern => link.notes.concern += 1,
            AdvisorSeverity::Blocker => link.notes.blocker += 1,
        }
        link.last_note = Some(note.clone());
        let immune = link.steps < link.immune_until;
        let steer = match m.severity {
            AdvisorSeverity::Nit => false,
            AdvisorSeverity::Concern => !immune,
            AdvisorSeverity::Blocker => true,
        };
        let ack = if steer {
            link.reviewing = Some(review);
            self.deliver(session, vec![note], true).await;
            guard::ACK_ACCEPTED
        } else {
            review.held.push(note);
            let mid = review.will_continue;
            link.reviewing = Some(review);
            if mid {
                guard::ACK_DEFERRED
            } else {
                guard::ACK_ACCEPTED
            }
        };
        self.publish(session).await;
        ack.to_owned()
    }
}

impl Message<Retry> for Advisors {
    type Reply = ();

    async fn handle(&mut self, Retry(advisor): Retry, _: &mut Context<Self, Self::Reply>) {
        let Some(&session) = self.by_advisor.get(&advisor) else {
            return;
        };
        let Some(link) = self.links.get(&session) else {
            return;
        };
        if link.reviewing.is_none() || link.failures == 0 {
            return;
        }
        match link.last_opener {
            Some(turn) => {
                self.tell_agents(crate::Regenerate {
                    session: advisor,
                    turn,
                })
                .await;
            }
            None => {
                let text = link.last_update.clone();
                self.tell_agents(AdvisorInput { advisor, text }).await;
            }
        }
    }
}

impl Message<AgentEvent> for Advisors {
    type Reply = ();

    async fn handle(&mut self, e: AgentEvent, _: &mut Context<Self, Self::Reply>) {
        let Some(session) = e.session() else {
            return;
        };
        if let Some(&advised) = self.by_advisor.get(&session) {
            match e {
                AgentEvent::TurnStarted { turn_seq, .. } => {
                    if let Some(l) = self.links.get_mut(&advised) {
                        l.last_opener = Some(turn_seq);
                    }
                }
                AgentEvent::TextDelta { text, .. } => {
                    if let Some(r) = self
                        .links
                        .get_mut(&advised)
                        .and_then(|l| l.reviewing.as_mut())
                        && r.text.len() < TEXT_CAP
                    {
                        r.text.push_str(&text);
                    }
                }
                AgentEvent::ToolCallFinished {
                    output, is_error, ..
                } => {
                    if is_error
                        && output.contains(UNKNOWN_TOOL)
                        && let Some(r) = self
                            .links
                            .get_mut(&advised)
                            .and_then(|l| l.reviewing.as_mut())
                    {
                        r.unknown_tool = true;
                    }
                }
                AgentEvent::Error { message, .. } => {
                    if let Some(l) = self.links.get_mut(&advised) {
                        l.last_error = Some(message);
                    }
                }
                AgentEvent::ContextUsed { context, .. } => {
                    let Some(l) = self.links.get_mut(&advised) else {
                        return;
                    };
                    l.context_used = context;
                    let limit = l.context_limit;
                    if limit > 0
                        && self.compact_at > 0.0
                        && context as f32 >= self.compact_at * limit as f32
                        && !l.reprimed
                        && l.reviewing.is_some()
                        && l.after_cancel.is_none()
                    {
                        l.reprimed = true;
                        let text = format!(
                            "### Session update\n_(advisor context re-primed at step {})_\n{}",
                            l.steps,
                            l.last_update.trim_start_matches("### Session update")
                        );
                        l.after_cancel = Some(Reset::Replay(text));
                        self.tell_agents(crate::Cancel {
                            session,
                            keep: None,
                        })
                        .await;
                    }
                }
                AgentEvent::TurnFinished {
                    stop_reason, usage, ..
                } => {
                    if let Some(l) = self.links.get_mut(&advised) {
                        l.tokens_in += u64::from(usage.prompt);
                        l.tokens_out += u64::from(usage.completion);
                    }
                    self.finish_review(advised, &stop_reason).await;
                }
                AgentEvent::SessionDeleted { .. } => {
                    self.by_advisor.remove(&session);
                    self.links.remove(&advised);
                }
                _ => {}
            }
            return;
        }
        if !self.links.contains_key(&session) {
            return;
        }
        match e {
            // The user's Stop: the review goes with it, held notes are preserved as a card.
            AgentEvent::TurnFinished { stop_reason, .. } if stop_reason == "cancelled" => {
                let (advisor, held, reviewing) = {
                    let l = self.links.get_mut(&session).unwrap();
                    l.backlog = 0;
                    l.release_waiters();
                    let held = l.reviewing.as_mut().map(|r| std::mem::take(&mut r.held));
                    (
                        l.row.advisor,
                        held.unwrap_or_default(),
                        l.reviewing.is_some(),
                    )
                };
                if reviewing {
                    self.tell_agents(crate::Cancel {
                        session: advisor,
                        keep: None,
                    })
                    .await;
                }
                self.deliver(session, held, false).await;
            }
            AgentEvent::SessionDeleted { .. } => {
                if let Some(l) = self.links.remove(&session) {
                    self.by_advisor.remove(&l.row.advisor);
                }
            }
            _ => {}
        }
    }
}
