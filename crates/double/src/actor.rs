//! The `Double` actor (126 §4): a bus client, like the RPC bridge
//! (crates/rpc/src/bridge.rs) — `rt.subscribe::<Double, T>` for
//! `ApprovalRequested` / `AskRequested` / `ApprovalResolved` / `AskResolved`
//! / `AgentEvent`; the registry and `ask_user` never learn it exists. Per
//! card: key (§3) → class row (L0 on first sight) → predict (§7.1) → write the
//! `examples` row and both `predictions` rows → publish `DoublePredicted`
//! (the prequential invariant: the row is on disk before the chip). At L2 a
//! grace timer runs; on expiry the double answers through `tools::ResolveBy`
//! / `agent::answer_ask_by` with `Answerer::Double`; `TakeOver` cancels it.
//! Labels and the ladder (§6, §9) run on the resolution events.

use std::collections::HashMap;
use std::sync::Arc;
use std::time::Duration;

use agent::AgentEvent;
use chrono::{DateTime, Utc};
use kameo::actor::{ActorRef, Spawn};
use kameo::message::{Context, Message};
use roster::attention::{AttentionItem, AttentionKind};
use runtime::{Runtime, SessionId};
use tools::{
    Answerer, ApprovalRequested, ApprovalResolved, AskRequested, AskResolved, Decision, Question,
    Risk,
};

use crate::class::{Card, CardKind, Requester, class_key};
use crate::config::DoubleConfig;
use crate::events::{DoubleAnswered, DoublePredicted};
use crate::labels::{self, Journal, Turns};
use crate::ladder::{self, Scored};
use crate::predict::{self, ClassStats, NONE, Oracle};
use crate::store::{BASELINE, ClassRow, ExampleRow, PredictionRow, Store};
use crate::types::{
    DoubleClass, DoubleClassDetail, DoubleExample, DoubleRun, DoubleStatus, ForgetTarget, Kpis,
    LabelSource, Outcome, RunId,
};
use crate::DOUBLE_ACTOR;

/// `predictions.version` before Tier A has activated a version (W2).
pub const V0: &str = "v0-zero-shot";
/// The approval card's two option labels, as rendered.
pub const ALLOW: &str = "Allow";
/// See [`ALLOW`].
pub const DENY: &str = "Deny";
/// An ask card's "nobody answered" default (`Answer::Skipped`).
pub const SKIPPED: &str = "Skipped";
/// A grace never ends later than this before the card's own deadline (§4.4).
pub const DEADLINE_MARGIN: Duration = Duration::from_secs(2);
/// "answered for you" inbox items stay this long.
pub const ACTED_TTL: chrono::Duration = chrono::Duration::hours(24);

/// `ask` → `bool`: cancel a running grace (`double_take_over`); the owner then
/// answers through the ordinary `resolve_approval` / `answer_ask`. `false`
/// when no grace was running for `question_id`.
#[derive(Clone, Debug)]
pub struct TakeOver(pub String);

/// `ask` → [`DoubleStatus`] (`double_status`).
#[derive(Clone, Copy, Debug, Default)]
pub struct Status;

/// `ask` → `Vec<DoubleClass>` (`double_classes`).
#[derive(Clone, Copy, Debug, Default)]
pub struct Classes;

/// `ask` → `Result<DoubleClassDetail, String>` (`double_class`).
#[derive(Clone, Debug)]
pub struct ClassDetail(pub String);

/// `ask` → [`DoubleStatus`]: pause / resume learning (`double_pause`).
/// Predictions and rungs continue; labels stop accruing.
#[derive(Clone, Copy, Debug)]
pub struct Pause(pub bool);

/// `ask` → `Result<u32, String>`: examples removed (`double_forget`).
#[derive(Clone, Debug)]
pub struct Forget(pub ForgetTarget);

/// `ask` → `Result<DoubleClass, String>` (`double_set_cap`).
#[derive(Clone, Debug)]
pub struct SetCap {
    /// The §3 key.
    pub class: String,
    /// New cap; clamped to the hard cap for the class's risk.
    pub rung: u8,
    /// Per-class L2 opt-in for a mutating class.
    pub mutating_opt_in: bool,
}

/// `tell`: re-publish `DoublePredicted` for every pending card (§4.4, a
/// client (re)subscribed).
#[derive(Clone, Copy, Debug, Default)]
pub struct Republish;

/// `ask` → `Vec<AttentionItem>`: the `AttentionKind::Double` items — a card in
/// grace ("double answers Allow in 14 s — take over") and the cards answered
/// in the last [`ACTED_TTL`]. The roster's inbox merges them (hand-off).
#[derive(Clone, Copy, Debug, Default)]
pub struct ListAttention;

/// Internal: the L2 grace ran out (or L3: at once).
#[derive(Clone, Debug)]
struct GraceExpired(String);

/// The raised card, kept for the answer path and the inbox row.
#[derive(Clone, Debug)]
enum Payload {
    Approval(ApprovalRequested),
    Ask(AskRequested),
}

/// A card the owner has not answered yet.
struct PendingCard {
    class: String,
    session: SessionId,
    example_id: String,
    options: Vec<String>,
    /// Only for a single-question ask: the `ask_user` call id, to pair the tool output.
    call_id: Option<String>,
    payload: Payload,
    predicted: Option<DoublePredicted>,
    asked_at: DateTime<Utc>,
    grace_until: Option<DateTime<Utc>>,
    grace: Option<tokio::task::JoinHandle<()>>,
    taken_over: bool,
    acted: bool,
}

/// A card the double answered, for the inbox.
struct Acted {
    item: AttentionItem,
    at: DateTime<Utc>,
}

/// The actor.
pub struct Double {
    rt: Runtime,
    store: Store,
    cfg: DoubleConfig,
    oracle: Arc<dyn Oracle>,
    journal: Option<Journal>,
    paused: bool,
    approval_timeout: Duration,
    ask_timeout: Duration,
    pending: HashMap<String, PendingCard>,
    /// Single-question asks answered by the owner whose label waits for the
    /// `ask_user` tool output (`ToolCallFinished`), by call id.
    awaiting_output: HashMap<String, PendingCard>,
    /// In-flight `ask_user` calls per session, oldest first.
    ask_calls: HashMap<SessionId, Vec<String>>,
    acted: Vec<Acted>,
    turns: Turns,
    /// The last user message per session (the state's `goal:` line).
    goals: HashMap<SessionId, String>,
    mcp: HashMap<String, bool>,
    requesters: HashMap<SessionId, Requester>,
}

impl kameo::Actor for Double {
    type Args = Self;
    type Error = std::convert::Infallible;

    async fn on_start(args: Self, _: ActorRef<Self>) -> Result<Self, Self::Error> {
        Ok(args)
    }
}

/// Spawn the actor on a subscriber mailbox, register it as [`DOUBLE_ACTOR`]
/// and subscribe it to the five event types. `cfg.enabled` is the caller's
/// business (`DoublePlugin::start` returned early when it is off).
pub async fn start(
    rt: &Runtime,
    store: Store,
    cfg: DoubleConfig,
    oracle: Arc<dyn Oracle>,
) -> anyhow::Result<ActorRef<Double>> {
    let tools: tools::ToolsConfig = rt.section("tools").unwrap_or_default();
    let journal = Journal::open(&rt.data_dir).await;
    if let Some(j) = &journal
        && !j.has_questions().await
    {
        tracing::info!("double: sessions.sqlite has no `questions` table yet; labels from bus events only");
    }
    let actor = Double::spawn_with_mailbox(
        Double {
            rt: rt.clone(),
            store,
            cfg,
            oracle,
            journal,
            paused: false,
            approval_timeout: Duration::from_secs(tools.approval_timeout_secs),
            ask_timeout: Duration::from_secs(tools.ask_timeout_secs),
            pending: HashMap::new(),
            awaiting_output: HashMap::new(),
            ask_calls: HashMap::new(),
            acted: Vec::new(),
            turns: Turns::default(),
            goals: HashMap::new(),
            mcp: HashMap::new(),
            requesters: HashMap::new(),
        },
        runtime::subscriber_mailbox(),
    );
    actor.register(DOUBLE_ACTOR)?;
    rt.subscribe::<Double, ApprovalRequested>(&actor).await;
    rt.subscribe::<Double, ApprovalResolved>(&actor).await;
    rt.subscribe::<Double, AskRequested>(&actor).await;
    rt.subscribe::<Double, AskResolved>(&actor).await;
    rt.subscribe::<Double, AgentEvent>(&actor).await;
    Ok(actor)
}

/// The option label of a `Decision`.
pub fn decision_label(d: Decision) -> &'static str {
    match d {
        Decision::Allow => ALLOW,
        Decision::Deny => DENY,
    }
}

/// Inverse of [`decision_label`].
pub fn label_decision(label: &str) -> Option<Decision> {
    match label {
        ALLOW => Some(Decision::Allow),
        DENY => Some(Decision::Deny),
        _ => None,
    }
}

/// The card text of an approval: the shell command, else `tool args-json`.
pub fn approval_text(tool: &str, args: &serde_json::Value) -> String {
    match args.get("command").and_then(|c| c.as_str()) {
        Some(cmd) if tool == "shell" => cmd.to_owned(),
        _ => format!("{tool} {args}"),
    }
}

/// The card text of an ask set: each question and its options.
pub fn ask_text(questions: &[Question]) -> String {
    questions
        .iter()
        .map(|q| {
            let opts: Vec<&str> = q.options.iter().map(|c| c.label.as_str()).collect();
            if opts.is_empty() {
                q.question.clone()
            } else {
                format!("{} [{}]", q.question, opts.join(" / "))
            }
        })
        .collect::<Vec<_>>()
        .join("\n")
}

/// A `(key, kind)` the ladder never answers: freeform or multi-question asks.
fn observe_only(kind: CardKind, options: &[String], single: bool) -> bool {
    kind == CardKind::Ask && (options.is_empty() || !single)
}

impl Double {
    fn grace(&self) -> Duration {
        Duration::from_secs(u64::from(self.cfg.ladder.grace_secs))
    }

    /// Who raised the card: a schedule run → `routine:<name>`; a desk session
    /// → `desk:<agent>`; a child → `subagent`; else `root`. Cached per session;
    /// an unreachable actor means `root`.
    async fn requester(&mut self, session: SessionId) -> Requester {
        if let Some(r) = self.requesters.get(&session) {
            return r.clone();
        }
        let mut r = Requester::Root;
        if let Ok(Some(sched)) = ActorRef::<schedule::Schedule>::lookup(schedule::SCHEDULE_ACTOR)
            && let Ok(list) = sched.ask(schedule::ListSchedules).await
            && let Some(s) = list.into_iter().find(|s| s.session == Some(session))
        {
            r = Requester::Routine(s.name);
        } else if let Ok(Some(agents)) = ActorRef::<agent::Agents>::lookup(agent::AGENT_ACTOR)
            && let Ok(store) = agents.ask(agent::DurableStore).await
            && let Ok(Some(row)) = store.session(session).await
        {
            r = match (row.desk, row.parent) {
                (Some(desk), _) => Requester::Desk(desk),
                (None, Some(_)) => Requester::Subagent,
                (None, None) => Requester::Root,
            };
        }
        self.requesters.insert(session, r.clone());
        r
    }

    /// Whether the registry knows `tool` as an MCP tool; `__` in the name is
    /// the fallback when the registry does not answer.
    async fn is_mcp(&mut self, session: SessionId, tool: &str) -> bool {
        if let Some(m) = self.mcp.get(tool) {
            return *m;
        }
        let mut m = tool.contains("__");
        if let Ok(Some(reg)) = ActorRef::<tools::ToolRegistry>::lookup(tools::REGISTRY_ACTOR)
            && let Ok(Ok(ins)) = reg
                .ask(tools::InspectTool {
                    session,
                    name: tool.to_owned(),
                    args: None,
                })
                .await
        {
            m = matches!(ins.source, tools::ToolSource::Mcp { .. });
        }
        self.mcp.insert(tool.to_owned(), m);
        m
    }

    /// The class row, created at L0 on first sight (§3).
    async fn class_row(&self, key: &str, kind: CardKind) -> anyhow::Result<ClassRow> {
        if let Some(c) = self.store.class(key).await? {
            return Ok(c);
        }
        let (cap, _) = ladder::hard_cap(&self.cfg.ladder.caps, key, self.cfg.ladder.mutating_opt_in);
        let c = ClassRow {
            key: key.to_owned(),
            kind,
            rung: 0,
            cap,
            mutating_opt_in: self.cfg.ladder.mutating_opt_in,
            n: 0,
            stats: serde_json::json!({}),
            updated_at: Utc::now(),
        };
        self.store.put_class(&c).await?;
        Ok(c)
    }

    /// The whole §4.2 raise path for one card.
    #[allow(clippy::too_many_arguments)]
    async fn on_card(
        &mut self,
        ctx: &ActorRef<Self>,
        kind: CardKind,
        question_id: String,
        session: SessionId,
        card: Card<'_>,
        options: Vec<String>,
        text: String,
        single: bool,
        call_id: Option<String>,
        payload: Payload,
        timeout: Duration,
    ) -> anyhow::Result<()> {
        if let Some(pc) = self.pending.get(&question_id) {
            // A re-publish (109 resume) of a card we already predicted.
            if let Some(p) = &pc.predicted {
                self.rt.publish(p.clone()).await;
            }
            return Ok(());
        }
        if self
            .store
            .example_for_question(&question_id)
            .await?
            .is_some()
        {
            // Predicted before a restart; the ledger row stands, the chip is gone.
            return Ok(());
        }
        let now = Utc::now();
        let requester = self.requester(session).await;
        let key = class_key(&card, &requester);
        let mut class = self.class_row(&key, kind).await?;
        let mut stats = ClassStats::from_json(&class.stats);
        let state = predict::state_text(
            &text,
            &key,
            &requester.to_string(),
            &stats,
            self.goals.get(&session).map(String::as_str),
        );
        let memory = predict::memory(&self.store, &key).await?;
        let p = predict::predict(
            self.oracle.as_ref(),
            kind,
            &options,
            state.clone(),
            &memory,
            &stats,
            now,
        )
        .await;
        let example_id = uuid::Uuid::now_v7().to_string();
        self.store
            .put_example(&ExampleRow {
                id: example_id.clone(),
                class: key.clone(),
                question_id: question_id.clone(),
                session_id: Some(session.to_string()),
                requester: requester.to_string(),
                card_text: state,
                options: options.clone(),
                embedding: p.embedding.clone(),
                asked_at: now,
            })
            .await?;
        class.n += 1;
        class.updated_at = now;
        let (cap, _) = ladder::hard_cap(&self.cfg.ladder.caps, &key, class.mutating_opt_in);
        class.cap = class.cap.min(cap);
        class.rung = class.rung.min(class.cap);
        self.store.put_class(&class).await?;

        if observe_only(kind, &options, single) {
            // Freeform / multi-question: example + labels, no prediction, no chip.
            self.turns.card(session, &question_id, now);
            self.pending.insert(
                question_id,
                PendingCard {
                    class: key,
                    session,
                    example_id,
                    options,
                    call_id,
                    payload,
                    predicted: None,
                    asked_at: now,
                    grace_until: None,
                    grace: None,
                    taken_over: true,
                    acted: false,
                },
            );
            return Ok(());
        }

        let rung = ladder::effective_rung(class.rung, p.escalated);
        let grace_until = match rung {
            2 => {
                let deadline = now + chrono::Duration::from_std(timeout)? - chrono::Duration::from_std(DEADLINE_MARGIN)?;
                Some((now + chrono::Duration::from_std(self.grace())?).min(deadline))
            }
            3 => Some(now),
            _ => None,
        };
        let version = match self.store.active_version().await? {
            Some(v) => v.id,
            None => V0.to_owned(),
        };
        // §7.1 step 6: both ledger rows BEFORE the chip.
        let baseline_top = p
            .oracle_probs
            .iter()
            .max_by(|a, b| a.1.total_cmp(&b.1))
            .cloned()
            .unwrap_or((NONE.to_owned(), 0.0));
        self.store
            .put_prediction(&PredictionRow {
                example_id: example_id.clone(),
                version: BASELINE.to_owned(),
                choice: baseline_top.0,
                probs: p.oracle_probs.clone(),
                p_top: baseline_top.1,
                rung_at_time: class.rung,
                acted: false,
                grace_until: None,
                outcome: Outcome::Unlabelled,
                at: now,
            })
            .await?;
        self.store
            .put_prediction(&PredictionRow {
                example_id: example_id.clone(),
                version: version.clone(),
                choice: p.choice.clone(),
                probs: p.probs.clone(),
                p_top: p.p,
                rung_at_time: class.rung,
                acted: false,
                grace_until,
                outcome: Outcome::Unlabelled,
                at: now,
            })
            .await?;
        let rows = self.ledger(Some(&key), false).await;
        stats.last_30d = Some(ladder::kpis(&rows, stats.median_answer_ms));
        let predicted = DoublePredicted {
            question_id: question_id.clone(),
            session: Some(session),
            kind,
            class: key.clone(),
            choice: p.choice.clone(),
            index: p.index,
            p: p.p,
            probs: p.probs.clone(),
            rung: class.rung,
            will_act_at: grace_until.map(|t| t.to_rfc3339()),
            version,
            explain: crate::events::Explain {
                promote_hint: ladder::promote_hint(class.rung, class.cap, &rows),
                ..p.explain
            },
        };
        let grace = grace_until.map(|until| {
            let me = ctx.clone();
            let qid = question_id.clone();
            let wait = (until - now).to_std().unwrap_or_default();
            tokio::spawn(async move {
                tokio::time::sleep(wait).await;
                let _ = me.tell(GraceExpired(qid)).await;
            })
        });
        self.turns.card(session, &question_id, now);
        self.pending.insert(
            question_id,
            PendingCard {
                class: key,
                session,
                example_id,
                options,
                call_id,
                payload,
                predicted: Some(predicted.clone()),
                asked_at: now,
                grace_until,
                grace,
                taken_over: false,
                acted: false,
            },
        );
        self.rt.publish(predicted).await;
        Ok(())
    }

    /// The last-30-days ledger (`None` = overall).
    async fn ledger(&self, class: Option<&str>, baseline: bool) -> Vec<Scored> {
        let since = Utc::now() - chrono::Duration::days(ladder::WINDOW_DAYS);
        ladder::ledger(&self.store, class, baseline, since)
            .await
            .unwrap_or_else(|e| {
                tracing::warn!("double ledger: {e:#}");
                Vec::new()
            })
    }

    /// The owner answered `pc` with `choice` (§6 direct / override) and the
    /// ladder is re-evaluated. A different answer on a card the double was
    /// about to act on (grace ran) is an override: weight 3, one rung down.
    async fn owner_answered(&mut self, pc: &PendingCard, choice: &str, now: DateTime<Utc>) {
        if self.paused {
            return;
        }
        let overridden = pc.grace_until.is_some()
            && pc.predicted.as_ref().is_some_and(|p| p.choice != choice);
        let (source, weight) = if overridden {
            (LabelSource::Override, labels::W_OVERRIDE)
        } else {
            (LabelSource::Direct, labels::W_DIRECT)
        };
        if let Err(e) = labels::label(&self.store, &pc.example_id, source, choice, weight, now).await {
            tracing::warn!("double label: {e:#}");
            return;
        }
        let answer_ms = u32::try_from((now - pc.asked_at).num_milliseconds().max(0)).ok();
        self.reladder(&pc.class, Some((choice, answer_ms)), overridden).await;
    }

    /// Re-evaluate one class after a label: stats, rung (demotion on an
    /// override wins), `l2_since`, `last_30d`.
    async fn reladder(&mut self, key: &str, answered: Option<(&str, Option<u32>)>, demote: bool) {
        let Ok(Some(mut class)) = self.store.class(key).await else {
            return;
        };
        let mut stats = ClassStats::from_json(&class.stats);
        if let Some((choice, ms)) = answered {
            stats.record(choice, ms);
        }
        let rows = self.ledger(Some(key), false).await;
        let now = Utc::now();
        let (cap, _) = ladder::hard_cap(&self.cfg.ladder.caps, key, class.mutating_opt_in);
        let cap = class.cap.min(cap);
        let before = class.rung;
        class.rung = if demote {
            ladder::demote(class.rung)
        } else {
            ladder::next_rung(class.rung, cap, &rows, stats.l2_since, now)
        }
        .min(cap);
        if class.rung >= 2 && before < 2 {
            stats.l2_since = Some(now);
        } else if class.rung < 2 {
            stats.l2_since = None;
        }
        if class.rung != before {
            tracing::info!(class = key, from = before, to = class.rung, demote, "double ladder");
        }
        stats.last_30d = Some(ladder::kpis(&rows, stats.median_answer_ms));
        class.stats = serde_json::to_value(&stats).unwrap_or_default();
        class.updated_at = now;
        if let Err(e) = self.store.put_class(&class).await {
            tracing::warn!("double class: {e:#}");
        }
    }

    /// A hindsight negative for the owner's answer on each of `question_ids`
    /// (§6.1): never on an overridden decision, never without a direct label.
    async fn hindsight(&mut self, question_ids: &[String], source: LabelSource, weight: f32) {
        if self.paused {
            return;
        }
        let now = Utc::now();
        for qid in question_ids {
            let Ok(Some(ex)) = self.store.example_for_question(qid).await else {
                continue;
            };
            let Ok(ls) = self.store.labels(&ex.id).await else {
                continue;
            };
            if ls.iter().any(|l| l.source == LabelSource::Override) {
                continue;
            }
            let Some(direct) = ls.iter().find(|l| l.source == LabelSource::Direct) else {
                continue;
            };
            let choice = direct.choice.clone();
            if let Err(e) = labels::label(&self.store, &ex.id, source, &choice, weight, now).await {
                tracing::warn!("double hindsight: {e:#}");
            }
            self.reladder(&ex.class, None, false).await;
        }
    }

    /// Act on `qid` (L2 grace expired / L3): mark the ledger row `acted`
    /// first — never an acted answer without its prediction row (§14) — then
    /// answer as the double.
    async fn act(&mut self, qid: &str) {
        let Some(pc) = self.pending.get_mut(qid) else {
            return;
        };
        let Some(pred) = pc.predicted.clone() else {
            return;
        };
        if pc.taken_over || pc.acted {
            return;
        }
        let rows = self.store.predictions(&pc.example_id).await.unwrap_or_default();
        let Some(mut row) = rows.into_iter().find(|r| r.version == pred.version) else {
            tracing::error!(qid, "double: no prediction row for a card in grace; not acting");
            pc.taken_over = true;
            return;
        };
        row.acted = true;
        if let Err(e) = self.store.put_prediction(&row).await {
            tracing::warn!("double act: {e:#}");
            return;
        }
        pc.acted = true;
        let sent = match &pc.payload {
            Payload::Approval(a) => match label_decision(&pred.choice) {
                Some(decision) => match ActorRef::<tools::Approval>::lookup(tools::APPROVAL_ACTOR) {
                    Ok(Some(approval)) => approval
                        .ask(tools::ResolveBy {
                            id: a.id,
                            decision,
                            by: Answerer::Double,
                        })
                        .await
                        .unwrap_or(false),
                    _ => false,
                },
                None => false,
            },
            Payload::Ask(a) => match pred.index {
                Some(i) => agent::answer_ask_by(a.id, labels::ask_answers(i), Answerer::Double)
                    .await
                    .unwrap_or(false),
                None => false,
            },
        };
        if !sent {
            tracing::warn!(qid, "double: the answer was not delivered");
        }
        let now = Utc::now();
        let (session, class) = (pc.session, pc.class.clone());
        let item = self.pending.get(qid).and_then(|pc| self.item(pc, now, true));
        if let Some(item) = item {
            self.acted.push(Acted { item, at: now });
        }
        self.acted.retain(|a| now - a.at <= ACTED_TTL);
        let ev = DoubleAnswered {
            question_id: qid.to_owned(),
            session: Some(session),
            class,
            choice: pred.choice,
            version: pred.version,
            undo_until: None,
        };
        self.rt.publish(ev).await;
    }

    /// The inbox row for a card in grace or just answered.
    fn item(&self, pc: &PendingCard, now: DateTime<Utc>, answered: bool) -> Option<AttentionItem> {
        let pred = pc.predicted.as_ref()?;
        let what = match &pc.payload {
            Payload::Approval(a) => format!("`{}`", approval_text(&a.tool, &a.args).chars().take(80).collect::<String>()),
            Payload::Ask(a) => a
                .questions
                .first()
                .map(|q| q.question.chars().take(80).collect::<String>())
                .unwrap_or_default(),
        };
        let title = if answered {
            format!("double answered {} for {what}", pred.choice)
        } else {
            let left = pc
                .grace_until
                .map_or(0, |t| (t - now).num_seconds().max(0));
            format!("double answers {} for {what} in {left} s — take over", pred.choice)
        };
        let (approval, ask) = match &pc.payload {
            Payload::Approval(a) => (Some(a.id), None),
            Payload::Ask(a) => (None, Some(a.clone())),
        };
        Some(AttentionItem {
            id: format!("double:{}", pred.question_id),
            kind: AttentionKind::Double,
            title,
            detail: pc.grace_until.map(|t| t.to_rfc3339()),
            at_ms: u64::try_from(now.timestamp_millis()).unwrap_or_default(),
            task: None,
            agent: None,
            session: Some(pc.session.to_string()),
            approval,
            decision: None,
            packet: None,
            ask,
            turn: None,
        })
    }

    /// `DoubleClass` for one row (both curves over the last 30 days).
    async fn class_view(&self, c: &ClassRow) -> DoubleClass {
        let stats = ClassStats::from_json(&c.stats);
        let (hard, reason) = ladder::hard_cap(&self.cfg.ladder.caps, &c.key, c.mutating_opt_in);
        let cap = c.cap.min(hard);
        let current = self.ledger(Some(&c.key), false).await;
        let baseline = self.ledger(Some(&c.key), true).await;
        let last = self
            .store
            .examples_of_class(&c.key, 1)
            .await
            .ok()
            .and_then(|v| v.first().map(|e| e.asked_at.to_rfc3339()));
        DoubleClass {
            key: c.key.clone(),
            kind: c.kind,
            n: c.n,
            rung: c.rung.min(cap),
            cap,
            cap_reason: reason.filter(|_| cap < self.cfg.ladder.caps.read_only),
            mutating_opt_in: c.mutating_opt_in,
            median_answer_ms: stats.median_answer_ms,
            kpis: ladder::kpis(&current, stats.median_answer_ms),
            baseline: ladder::kpis(&baseline, stats.median_answer_ms),
            last_decision_at: last,
        }
    }

    async fn status(&self) -> DoubleStatus {
        let classes = self.store.classes().await.unwrap_or_default();
        let mut medians: Vec<u32> = classes
            .iter()
            .map(|c| ClassStats::from_json(&c.stats).median_answer_ms)
            .filter(|m| *m > 0)
            .collect();
        medians.sort_unstable();
        let median = medians.get(medians.len() / 2).copied().unwrap_or(0);
        let last_run = self
            .store
            .runs(1)
            .await
            .ok()
            .and_then(|r| r.into_iter().next())
            .map(|r| DoubleRun {
                id: RunId(r.id),
                tier: r.tier,
                venue: r.venue,
                state: r.state,
                started_at: r.started_at.to_rfc3339(),
                ended_at: r.ended_at.map(|t| t.to_rfc3339()),
                result: r.result,
            });
        DoubleStatus {
            enabled: true,
            paused: self.paused,
            active_version: self.store.active_version().await.ok().flatten().map(|v| v.id),
            last_run,
            next_window: None,
            venue: self.cfg.training.venue.clone(),
            approvals_off_sessions: 0,
            kpis: ladder::kpis(&self.ledger(None, false).await, median),
            baseline: ladder::kpis(&self.ledger(None, true).await, median),
        }
    }

    /// Settle a pending card the owner answered: cancel the grace, label, reladder.
    async fn settle(&mut self, qid: &str, choice: Option<&str>, timed_out: bool) {
        let Some(mut pc) = self.pending.remove(qid) else {
            return;
        };
        if let Some(h) = pc.grace.take() {
            h.abort();
        }
        let now = Utc::now();
        if pc.acted {
            return;
        }
        if timed_out {
            self.turns.drop_card(pc.session, qid);
            if !self.paused {
                let default = match pc.payload {
                    Payload::Approval(_) => DENY,
                    Payload::Ask(_) => SKIPPED,
                };
                if let Err(e) = labels::label(
                    &self.store,
                    &pc.example_id,
                    LabelSource::Timeout,
                    default,
                    labels::W_TIMEOUT,
                    now,
                )
                .await
                {
                    tracing::warn!("double timeout label: {e:#}");
                }
            }
            return;
        }
        match choice {
            Some(c) => self.owner_answered(&pc, c, now).await,
            None => {
                // An answered ask without the choice in hand: the journal, else the tool output.
                let Payload::Ask(a) = &pc.payload else {
                    return;
                };
                let from_journal = match &self.journal {
                    Some(j) => j
                        .question(qid)
                        .await
                        .and_then(|r| r.answer)
                        .and_then(|ans| labels::ask_choice(&a.questions, &ans)),
                    None => None,
                };
                match (from_journal, pc.call_id.clone()) {
                    (Some((label, _)), _) => self.owner_answered(&pc, &label, now).await,
                    (None, Some(call)) => {
                        self.awaiting_output.insert(call, pc);
                    }
                    (None, None) => {}
                }
            }
        }
    }
}

impl Message<ApprovalRequested> for Double {
    type Reply = ();

    async fn handle(&mut self, m: ApprovalRequested, ctx: &mut Context<Self, ()>) {
        if !self.cfg.capture.approvals {
            return;
        }
        let mcp = self.is_mcp(m.session, &m.tool).await;
        let card = Card::Approval {
            tool: &m.tool,
            args: &m.args,
            risk: m.risk,
            mcp,
        };
        let text = approval_text(&m.tool, &m.args);
        let timeout = self.approval_timeout;
        let me = ctx.actor_ref().clone();
        if let Err(e) = self
            .on_card(
                &me,
                CardKind::Approval,
                m.id.0.to_string(),
                m.session,
                card,
                vec![ALLOW.to_owned(), DENY.to_owned()],
                text,
                true,
                None,
                Payload::Approval(m.clone()),
                timeout,
            )
            .await
        {
            tracing::warn!("double approval card: {e:#}");
        }
    }
}

impl Message<AskRequested> for Double {
    type Reply = ();

    async fn handle(&mut self, m: AskRequested, ctx: &mut Context<Self, ()>) {
        if !self.cfg.capture.asks {
            return;
        }
        let options = labels::ask_options(&m.questions);
        let single = labels::ask_single(&m.questions).is_some();
        let card = Card::Ask { options: &options };
        let text = ask_text(&m.questions);
        let call_id = self
            .ask_calls
            .get_mut(&m.session)
            .and_then(|v| if v.is_empty() { None } else { Some(v.remove(0)) });
        let timeout = self.ask_timeout;
        let me = ctx.actor_ref().clone();
        if let Err(e) = self
            .on_card(
                &me,
                CardKind::Ask,
                m.id.0.to_string(),
                m.session,
                card,
                options,
                text,
                single,
                call_id,
                Payload::Ask(m.clone()),
                timeout,
            )
            .await
        {
            tracing::warn!("double ask card: {e:#}");
        }
    }
}

impl Message<ApprovalResolved> for Double {
    type Reply = ();

    async fn handle(&mut self, m: ApprovalResolved, _: &mut Context<Self, ()>) {
        let qid = m.id.0.to_string();
        self.settle(&qid, Some(decision_label(m.decision)), m.timed_out)
            .await;
    }
}

impl Message<AskResolved> for Double {
    type Reply = ();

    async fn handle(&mut self, m: AskResolved, _: &mut Context<Self, ()>) {
        let qid = m.id.0.to_string();
        if m.dismissed {
            // The owner closed the set: the turn is cancelled; no label.
            if let Some(mut pc) = self.pending.remove(&qid) {
                if let Some(h) = pc.grace.take() {
                    h.abort();
                }
                self.turns.drop_card(pc.session, &qid);
            }
            return;
        }
        self.settle(&qid, None, m.timed_out).await;
    }
}

impl Message<AgentEvent> for Double {
    type Reply = ();

    async fn handle(&mut self, ev: AgentEvent, _: &mut Context<Self, ()>) {
        match ev {
            AgentEvent::TurnStarted { session, text, .. } => {
                self.turns.started(session);
                let targets = self.turns.previous(session);
                self.goals.insert(session, text.clone());
                if targets.is_empty() || self.paused {
                    return;
                }
                let ask = decision::Ask::new(
                    "double.correction",
                    decision::Question::Bool {
                        statement: "This message is a correction of the assistant's previous action."
                            .to_owned(),
                        state: text.chars().take(predict::GOAL_CHARS).collect(),
                    },
                );
                let p_true = match self.oracle.decide(ask).await {
                    Ok(d) => d
                        .probabilities
                        .iter()
                        .find(|(l, _)| l == "true")
                        .map_or(0.0, |(_, p)| *p),
                    Err(_) => 0.0,
                };
                if let Some(w) = labels::correction_weight(p_true) {
                    self.hindsight(&targets, LabelSource::HindsightCorrection, w)
                        .await;
                }
            }
            AgentEvent::ContextChanged {
                session, reason, ..
            } if reason == "rewind" => {
                let targets = self.turns.rewind(session);
                self.hindsight(&targets, LabelSource::HindsightRewind, labels::W_REWIND)
                    .await;
            }
            AgentEvent::TurnFinished {
                session,
                stop_reason,
                ..
            } => {
                let targets = self.turns.cancelled(session, Utc::now());
                if stop_reason == "cancelled" {
                    self.hindsight(&targets, LabelSource::HindsightInterrupt, labels::W_INTERRUPT)
                        .await;
                }
            }
            AgentEvent::ToolCallStarted {
                session,
                call_id,
                name,
                ..
            } if name == "ask_user" => {
                self.ask_calls.entry(session).or_default().push(call_id);
            }
            AgentEvent::ToolCallFinished {
                call_id, output, ..
            } => {
                if let Some(pc) = self.awaiting_output.remove(&call_id)
                    && let Payload::Ask(a) = &pc.payload
                    && let Some((label, _)) = labels::ask_choice_from_output(&a.questions, &output)
                {
                    self.owner_answered(&pc, &label, Utc::now()).await;
                }
            }
            _ => {}
        }
    }
}

impl Message<GraceExpired> for Double {
    type Reply = ();

    async fn handle(&mut self, m: GraceExpired, _: &mut Context<Self, ()>) {
        self.act(&m.0).await;
    }
}

impl Message<TakeOver> for Double {
    type Reply = bool;

    async fn handle(&mut self, m: TakeOver, _: &mut Context<Self, bool>) -> bool {
        let Some(pc) = self.pending.get_mut(&m.0) else {
            return false;
        };
        let running = !pc.acted && !pc.taken_over && pc.grace.is_some();
        if let Some(h) = pc.grace.take() {
            h.abort();
        }
        pc.taken_over = true;
        running
    }
}

impl Message<Status> for Double {
    type Reply = DoubleStatus;

    async fn handle(&mut self, _: Status, _: &mut Context<Self, Self::Reply>) -> Self::Reply {
        self.status().await
    }
}

impl Message<Pause> for Double {
    type Reply = DoubleStatus;

    async fn handle(&mut self, m: Pause, _: &mut Context<Self, Self::Reply>) -> Self::Reply {
        self.paused = m.0;
        self.status().await
    }
}

impl Message<Classes> for Double {
    type Reply = Vec<DoubleClass>;

    async fn handle(&mut self, _: Classes, _: &mut Context<Self, Self::Reply>) -> Self::Reply {
        let mut out = Vec::new();
        for c in self.store.classes().await.unwrap_or_default() {
            out.push(self.class_view(&c).await);
        }
        out
    }
}

impl Message<ClassDetail> for Double {
    type Reply = Result<DoubleClassDetail, String>;

    async fn handle(&mut self, m: ClassDetail, _: &mut Context<Self, Self::Reply>) -> Self::Reply {
        let row = self
            .store
            .class(&m.0)
            .await
            .map_err(|e| e.to_string())?
            .ok_or_else(|| format!("no class {:?}", m.0))?;
        let class = self.class_view(&row).await;
        let epoch = DateTime::<Utc>::UNIX_EPOCH;
        let current = ladder::ledger(&self.store, Some(&m.0), false, epoch)
            .await
            .map_err(|e| e.to_string())?;
        let baseline = ladder::ledger(&self.store, Some(&m.0), true, epoch)
            .await
            .map_err(|e| e.to_string())?;
        let mut examples = Vec::new();
        for e in self
            .store
            .examples_of_class(&m.0, 50)
            .await
            .map_err(|e| e.to_string())?
        {
            let ls: Vec<(LabelSource, String, f32)> = self
                .store
                .labels(&e.id)
                .await
                .unwrap_or_default()
                .into_iter()
                .map(|l| (l.source, l.choice, l.weight))
                .collect();
            let preds = self.store.predictions(&e.id).await.unwrap_or_default();
            let live = preds.iter().find(|p| p.version != BASELINE);
            examples.push(DoubleExample {
                id: e.id.clone(),
                question_id: e.question_id,
                asked_at: e.asked_at.to_rfc3339(),
                card_excerpt: e.card_text.chars().take(predict::EXCERPT_CHARS).collect(),
                options: e.options,
                chose: ladder::summarise(&ls, false).0,
                predicted: live.map(|p| p.choice.clone()),
                outcome: live.map_or(Outcome::Unlabelled, |p| p.outcome),
            });
        }
        let recent = self.ledger(Some(&m.0), false).await;
        Ok(DoubleClassDetail {
            promote_hint: ladder::promote_hint(class.rung, class.cap, &recent),
            class,
            curve: ladder::curve(&current, &baseline, row_median(&row)),
            examples,
        })
    }
}

fn row_median(c: &ClassRow) -> u32 {
    ClassStats::from_json(&c.stats).median_answer_ms
}

impl Message<Forget> for Double {
    type Reply = Result<u32, String>;

    async fn handle(&mut self, m: Forget, _: &mut Context<Self, Self::Reply>) -> Self::Reply {
        let pool = self.store.pool();
        let n = match m.0 {
            ForgetTarget::Class { key } => {
                let n = sqlx::query("DELETE FROM examples WHERE class = ?")
                    .bind(&key)
                    .execute(pool)
                    .await
                    .map_err(|e| e.to_string())?
                    .rows_affected();
                if let Ok(Some(mut c)) = self.store.class(&key).await {
                    c.n = 0;
                    c.rung = 0;
                    c.stats = serde_json::json!({});
                    c.updated_at = Utc::now();
                    self.store.put_class(&c).await.map_err(|e| e.to_string())?;
                }
                n
            }
            ForgetTarget::Examples { ids } => {
                let mut n = 0;
                for id in ids {
                    n += sqlx::query("DELETE FROM examples WHERE id = ?")
                        .bind(&id)
                        .execute(pool)
                        .await
                        .map_err(|e| e.to_string())?
                        .rows_affected();
                }
                n
            }
        };
        Ok(u32::try_from(n).unwrap_or(u32::MAX))
    }
}

impl Message<SetCap> for Double {
    type Reply = Result<DoubleClass, String>;

    async fn handle(&mut self, m: SetCap, _: &mut Context<Self, Self::Reply>) -> Self::Reply {
        let mut c = self
            .store
            .class(&m.class)
            .await
            .map_err(|e| e.to_string())?
            .ok_or_else(|| format!("no class {:?}", m.class))?;
        c.mutating_opt_in = m.mutating_opt_in;
        let (hard, _) = ladder::hard_cap(&self.cfg.ladder.caps, &c.key, c.mutating_opt_in);
        c.cap = m.rung.min(hard).min(3);
        c.rung = c.rung.min(c.cap);
        c.updated_at = Utc::now();
        self.store.put_class(&c).await.map_err(|e| e.to_string())?;
        Ok(self.class_view(&c).await)
    }
}

impl Message<Republish> for Double {
    type Reply = ();

    async fn handle(&mut self, _: Republish, _: &mut Context<Self, ()>) {
        let evs: Vec<DoublePredicted> = self
            .pending
            .values()
            .filter(|pc| !pc.acted)
            .filter_map(|pc| pc.predicted.clone())
            .collect();
        for ev in evs {
            self.rt.publish(ev).await;
        }
    }
}

impl Message<ListAttention> for Double {
    type Reply = Vec<AttentionItem>;

    async fn handle(&mut self, _: ListAttention, _: &mut Context<Self, Self::Reply>) -> Self::Reply {
        let now = Utc::now();
        self.acted.retain(|a| now - a.at <= ACTED_TTL);
        let mut out: Vec<AttentionItem> = self
            .pending
            .values()
            .filter(|pc| pc.grace_until.is_some() && !pc.acted && !pc.taken_over)
            .filter_map(|pc| self.item(pc, now, false))
            .collect();
        out.extend(self.acted.iter().rev().map(|a| a.item.clone()));
        out
    }
}
