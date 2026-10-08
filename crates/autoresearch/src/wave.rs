//! W5 WAVES — parallel mode (design doc 140 §10): N candidate worktrees from
//! the incumbent, distinct categories per slot, children mapped through the
//! parent session + slot, the bench queue, promotion of the best keep-candidate
//! and cherry-picks of disjoint winners each re-confirmed, candidate worktrees
//! removed. Each slot is an experiment with `wave` and `slot` set.
//!
//! The actor's `begin_env` / `run_env` / `log_env` branch here when the study
//! has `parallel > 1` (the supervisor's fire) or the call names a slot (a
//! child). The two process-wide tables are the children the study session
//! spawned (`AgentEvent::SubagentSpawned`, bound to a slot at
//! `experiment_begin{slot}`) and a per-study settle lock: the last slot's log
//! and the supervisor's turn end both try to promote, one wins and the other
//! sees the `wave_promoted` event.

use std::collections::{BTreeSet, HashMap};
use std::path::{Path, PathBuf};
use std::sync::{Arc, LazyLock};
use std::time::Duration;

use parking_lot::Mutex;
use runtime::SessionId;

use crate::actor::{self, Env};
use crate::config::OnSuspicious;
use crate::error::{Error, Result};
use crate::git::KeepMeta;
use crate::runner::SampleRequest;
use crate::store::{ExperimentRecord, StudyRecord, now};
use crate::types::*;
use crate::{Studies, binding, schedule_file};

/// `events.kind` written by [`open`]: `{wave, slots: [Slot]}`.
pub const EVENT_OPEN: &str = "wave_open";
/// `events.kind` written by [`promote`]: `{wave, promoted: [n], incumbent}`.
pub const EVENT_PROMOTED: &str = "wave_promoted";

/// A child the study session spawned; `slot` once it called `experiment_begin{slot}`.
#[derive(Clone, Copy, Debug)]
struct Child {
    parent: SessionId,
    slot: Option<u8>,
}

static CHILDREN: LazyLock<Mutex<HashMap<SessionId, Child>>> = LazyLock::new(Default::default);
static SETTLING: LazyLock<Mutex<HashMap<String, Arc<tokio::sync::Mutex<()>>>>> =
    LazyLock::new(Default::default);

fn settle_lock(root: &Path, slug: &str) -> Arc<tokio::sync::Mutex<()>> {
    SETTLING
        .lock()
        .entry(format!("{}/{slug}", root.display()))
        .or_default()
        .clone()
}

/// `<study dir>/cand-<k>` (140 §12.6).
pub fn cand_dir(root: &Path, slug: &str, k: u8) -> PathBuf {
    crate::study_dir(root, slug).join(format!("cand-{k}"))
}

// ───────────────────────────── children ─────────────────────────────

/// Remember `child` as spawned by `parent` (the bound study session).
pub fn register_child(parent: SessionId, child: SessionId) {
    CHILDREN.lock().insert(child, Child { parent, slot: None });
}

/// The slot `session` bound itself to with `experiment_begin{slot}` (the tools fill `slot` from this).
pub fn slot_of(session: SessionId) -> Option<u8> {
    CHILDREN.lock().get(&session).and_then(|c| c.slot)
}

/// The parent of a registered child; `session` itself otherwise.
fn parent_of(session: SessionId) -> SessionId {
    CHILDREN.lock().get(&session).map_or(session, |c| c.parent)
}

/// `AgentEvent`s the wave cares about (called from `binding`'s subscription):
/// a child of a bound wave study gets the study toolset minus `spawn_subagent`
/// (140 §8.3); a child's turn end closes its slot; the supervisor's turn end
/// settles the wave (abandoned slots count as discard, the wave is promoted).
pub async fn on_agent_event(a: &mut Studies, e: &agent::AgentEvent) -> Result<()> {
    match e {
        agent::AgentEvent::SubagentSpawned { parent, child, .. } => {
            let Some(study) = a.store.study_of_session(*parent).await? else {
                return Ok(());
            };
            if study.spec.parallel < 2 {
                return Ok(());
            }
            register_child(*parent, *child);
            match agents() {
                Some(agents) => {
                    if let Err(e) = agents
                        .ask(agent::SetToolset {
                            session: *child,
                            toolset: Some(binding::toolset(1)),
                        })
                        .await
                    {
                        tracing::warn!(slug = %study.slug, "wave child toolset not applied: {e}");
                    }
                }
                None => {
                    tracing::warn!(slug = %study.slug, "agent actor not running; wave child unconfined")
                }
            }
            Ok(())
        }
        agent::AgentEvent::TurnFinished { session, .. } => {
            let child = CHILDREN.lock().remove(session);
            let env = a.env();
            if let Some(c) = child {
                if let (Some(k), Some(study)) = (c.slot, a.store.study_of_session(c.parent).await?)
                    && let Some(exp) = a.store.dangling_experiment(&study.slug, Some(k)).await?
                    && exp.state != ExperimentState::Measuring
                {
                    abandon(&env, exp).await?;
                }
                return Ok(());
            }
            if let Some(study) = a.store.study_of_session(*session).await?
                && study.spec.parallel > 1
                && matches!(study.state, StudyState::Running | StudyState::Paused { .. })
            {
                // Confirm samples take minutes: off the mailbox, like Begin/Run/Log.
                tokio::spawn(async move {
                    if let Err(e) = settle(&env, &study).await {
                        tracing::warn!(slug = %study.slug, "wave not settled: {e}");
                    }
                });
            }
            Ok(())
        }
        _ => Ok(()),
    }
}

fn agents() -> Option<kameo::actor::ActorRef<agent::Agents>> {
    kameo::actor::ActorRef::<agent::Agents>::lookup(agent::AGENT_ACTOR)
        .ok()
        .flatten()
}

// ───────────────────────────── resolve ─────────────────────────────

/// The study, the slot's open experiment and its worktree for a call from
/// `session` naming `slot`: the session is a registered child of the study
/// session, or the study session itself; anything else is refused (140 §8.2).
pub(crate) async fn resolve_env(
    env: &Env,
    session: SessionId,
    slot: u8,
) -> Result<(StudyRecord, ExperimentRecord, PathBuf)> {
    let study = env
        .store
        .study_of_session(parent_of(session))
        .await?
        .ok_or_else(binding::not_a_study_session)?;
    if study.spec.parallel < 2 {
        return Err(Error::refused(
            "this study runs one experiment per fire; drop `slot`",
        ));
    }
    let exp = env
        .store
        .dangling_experiment(&study.slug, Some(slot))
        .await?
        .ok_or_else(|| Error::refused(format!("slot {slot} has no open experiment")))?;
    let dir = cand_dir(&env.root, &study.slug, slot);
    Ok((study, exp, dir))
}

/// Resolve `(session, slot)` to the slot's worktree and experiment number (140 §10 step 2).
pub async fn resolve(
    a: &Studies,
    session: runtime::SessionId,
    slot: u8,
) -> Result<(std::path::PathBuf, u32)> {
    let (_, exp, dir) = resolve_env(&a.env(), session, slot).await?;
    Ok((dir, exp.n))
}

/// Bind a child to its slot at `experiment_begin{slot}`; the study session itself may act as any slot.
pub(crate) fn bind_slot(session: SessionId, slot: u8) {
    if let Some(c) = CHILDREN.lock().get_mut(&session) {
        c.slot = Some(slot);
    }
}

// ───────────────────────────── open ─────────────────────────────

/// The next wave number: one more than the highest wave in the ledger.
pub(crate) async fn next_wave(env: &Env, slug: &str) -> Result<u32> {
    Ok(env
        .store
        .list_experiments(slug, None, u32::MAX)
        .await?
        .iter()
        .filter_map(|e| e.wave)
        .max()
        .unwrap_or(0)
        + 1)
}

/// [`open`] on an [`Env`].
pub(crate) async fn open_env(env: &Env, study: &StudyRecord, wave: u32) -> Result<Vec<Slot>> {
    let slug = study.slug.as_str();
    let n_slots = study.spec.parallel.max(2);
    let incumbent = actor::incumbent_of(study)?;
    let study_wt = actor::worktree_of(study)?;
    let first = env.store.next_experiment_n(slug).await?;
    let categories = assign_categories(env, study, wave, usize::from(n_slots)).await?;
    let mut slots = Vec::with_capacity(usize::from(n_slots));
    for (i, (category, idea_id)) in categories.into_iter().enumerate() {
        let k = i as u8 + 1;
        let dir = cand_dir(&env.root, slug, k);
        if dir.exists() {
            env.backend.worktree_remove(&study_wt, &dir)?;
        }
        env.backend
            .worktree_add_detached(&study_wt, &dir, &incumbent)?;
        let n = first + i as u32;
        let mut exp = ExperimentRecord::open(slug, n, study.segment, &incumbent);
        exp.wave = Some(wave);
        exp.slot = Some(k);
        env.store.insert_experiment(&exp).await?;
        std::fs::create_dir_all(env.runs_dir(slug, n))?;
        slots.push(Slot {
            k,
            worktree: dir.display().to_string(),
            category,
            idea_id,
            n,
        });
    }
    env.store
        .append_event(
            slug,
            EVENT_OPEN,
            &serde_json::json!({ "wave": wave, "slots": slots }),
        )
        .await?;
    Ok(slots)
}

/// Open a wave: create the candidate worktrees and allocate slots (140 §10 step 1).
pub async fn open(a: &mut Studies, slug: &str, wave: u32) -> Result<Vec<Slot>> {
    let env = a.env();
    let study = study_by_slug(&env, slug).await?;
    open_env(&env, &study, wave).await
}

async fn study_by_slug(env: &Env, slug: &str) -> Result<StudyRecord> {
    env.store
        .get_study(slug)
        .await?
        .ok_or_else(|| Error::refused(format!("no study `{slug}`")))
}

/// Distinct categories per slot: the user's ★ ideas first (one slot each),
/// then a rotation (by wave number) over the categories seen in the study's
/// experiments and backlog; slots past the known categories are a free choice.
async fn assign_categories(
    env: &Env,
    study: &StudyRecord,
    wave: u32,
    n: usize,
) -> Result<Vec<(String, Option<IdeaId>)>> {
    let slug = study.slug.as_str();
    let ideas = env.store.list_ideas(slug, Some(IdeaStatus::Open)).await?;
    let mut exps = env.store.list_experiments(slug, None, u32::MAX).await?;
    exps.reverse(); // oldest first: a stable rotation order
    let mut seen: Vec<String> = Vec::new();
    for c in exps
        .iter()
        .map(|e| e.category.trim())
        .chain(ideas.iter().map(|i| i.category.trim()))
        .filter(|c| !c.is_empty())
    {
        if !seen.iter().any(|s| s == c) {
            seen.push(c.to_owned());
        }
    }
    if !seen.is_empty() {
        let shift = (wave as usize).saturating_sub(1) % seen.len();
        seen.rotate_left(shift);
    }
    let mut out: Vec<(String, Option<IdeaId>)> = Vec::with_capacity(n);
    let mut taken: BTreeSet<String> = BTreeSet::new();
    for idea in ideas.iter().filter(|i| i.source == IdeaSource::User) {
        if out.len() == n {
            break;
        }
        let c = idea.category.trim();
        if !c.is_empty() && taken.insert(c.to_owned()) {
            out.push((c.to_owned(), Some(idea.id)));
        }
    }
    for c in seen {
        if out.len() == n {
            break;
        }
        if taken.insert(c.clone()) {
            out.push((c, None));
        }
    }
    while out.len() < n {
        out.push((String::new(), None));
    }
    Ok(out)
}

/// The slots of `wave` as [`open`] recorded them.
pub(crate) async fn slots_of(env: &Env, slug: &str, wave: u32) -> Result<Vec<Slot>> {
    for e in env.store.list_events(slug, Some(EVENT_OPEN), 100).await? {
        if e.payload.get("wave").and_then(|v| v.as_u64()) == Some(u64::from(wave)) {
            return Ok(serde_json::from_value(
                e.payload.get("slots").cloned().unwrap_or_default(),
            )?);
        }
    }
    Ok(Vec::new())
}

/// The `<wave>` block appended to the supervisor's brief (140 §10 step 2).
pub(crate) fn supervisor_block(
    study: &StudyRecord,
    wave: u32,
    slots: &[Slot],
    ideas: &[Idea],
) -> String {
    let mut out = format!(
        "\n<wave n=\"{wave}\" slots=\"{}\">\n\
         This fire is a wave: {} candidates are tried in parallel from the incumbent, one per slot, each in its own worktree. \
         You are the supervisor: do not edit or measure anything yourself. Spawn one native child per slot with spawn_subagent \
         (cwd = the slot's worktree; no `tools` cap; the same model), run them concurrently and wait for all of them. \
         Tell each child, verbatim with its values: \"You are slot K of autoresearch study {slug}. Call experiment_begin with slot=K \
         first and follow the brief it returns. Work only inside WORKTREE. Make one coherent change in category CATEGORY \
         (the slot's idea, if any, is the first thing to try), test it quickly, then call experiment_run and experiment_log. \
         Never run git commands that write, never edit evaluator files, never stop to ask.\" \
         When every slot has logged, the harness promotes the best keep onto the study branch and re-confirms it; the \
         other keeps are cherry-picked after it only when their files do not overlap and a confirm sample still clears the gate. \
         Then end your turn.\n",
        slots.len(),
        slots.len(),
        slug = study.slug,
    );
    for s in slots {
        out.push_str(&slot_line(s, ideas));
    }
    out.push_str("</wave>\n");
    out
}

/// The `<slot>` block appended to a child's brief.
pub(crate) fn child_block(wave: u32, slot: &Slot, ideas: &[Idea]) -> String {
    format!(
        "\n<slot k=\"{}\" wave=\"{wave}\" experiment=\"{}\">\n\
         You are slot {} of this wave. Work only inside {} (your cwd). {} Call experiment_run (slot is filled in for you) \
         after your change, then experiment_log, then end your turn; the supervisor waits for you.\n\
         {}</slot>\n",
        slot.k,
        slot.n,
        slot.k,
        slot.worktree,
        if slot.category.is_empty() {
            "Pick a category no other slot has.".to_owned()
        } else {
            format!("Your category is `{}`.", slot.category)
        },
        slot_line(slot, ideas),
    )
}

fn slot_line(s: &Slot, ideas: &[Idea]) -> String {
    let mut line = format!(
        "slot {}: experiment #{} · worktree {}",
        s.k, s.n, s.worktree
    );
    line.push_str(&if s.category.is_empty() {
        " · category: free (not one another slot has)".to_owned()
    } else {
        format!(" · category {}", s.category)
    });
    if let Some(id) = s.idea_id
        && let Some(i) = ideas.iter().find(|i| i.id == id)
    {
        line.push_str(&format!(" · ★ idea #{id}: {}", i.text.trim()));
    }
    line.push('\n');
    line
}

// ───────────────────────────── settle and promote ─────────────────────────────

/// What a promotion did.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Promotion {
    /// Experiments now kept on the study branch, in promotion order.
    pub promoted: Vec<u32>,
    /// The incumbent afterwards.
    pub incumbent: Option<String>,
}

/// Every open slot of `wave` closed?
pub(crate) async fn wave_closed(env: &Env, slug: &str, wave: u32) -> Result<bool> {
    Ok(env
        .store
        .list_experiments(slug, None, u32::MAX)
        .await?
        .iter()
        .filter(|e| e.wave == Some(wave))
        .all(|e| {
            !matches!(
                e.state,
                ExperimentState::Open | ExperimentState::Measuring | ExperimentState::Measured
            )
        }))
}

async fn promoted_already(env: &Env, slug: &str, wave: u32) -> Result<bool> {
    Ok(env
        .store
        .list_events(slug, Some(EVENT_PROMOTED), 100)
        .await?
        .iter()
        .any(|e| e.payload.get("wave").and_then(|v| v.as_u64()) == Some(u64::from(wave))))
}

/// Close a slot nobody logged (140 §4.3 `abandoned`), its change counts as a discard.
async fn abandon(env: &Env, mut exp: ExperimentRecord) -> Result<()> {
    let interrupted = exp.state == ExperimentState::Measuring;
    exp.state = if interrupted {
        ExperimentState::Interrupted
    } else {
        ExperimentState::Abandoned
    };
    exp.finished_at = Some(now());
    env.store.update_experiment(&exp).await?;
    if interrupted {
        actor::mark_lost(&env.store, &exp.slug, exp.n).await?;
    }
    env.store
        .append_event(
            &exp.slug,
            "incident",
            &serde_json::json!({ "n": exp.n, "slot": exp.slot, "what": exp.state }),
        )
        .await?;
    env.rt
        .publish(StudyEvent::ExperimentFinished {
            slug: exp.slug.clone(),
            n: exp.n,
            verdict: exp.verdict.clone().unwrap_or(Verdict::Refused {
                reason: if interrupted {
                    "interrupted"
                } else {
                    "abandoned"
                }
                .into(),
            }),
            metric: exp.metric,
            delta_pct: exp.delta_pct,
            z: exp.z,
        })
        .await;
    Ok(())
}

/// Promote every wave of `study` that has not been promoted yet (slots still
/// open are abandoned first), then the §8.4 stop rules. Returns the stop
/// reason when the study is finalizing and the one-line notes for the brief.
pub(crate) async fn settle(
    env: &Env,
    study: &StudyRecord,
) -> Result<(Option<StopReason>, Vec<String>)> {
    let slug = study.slug.as_str();
    let all = env.store.list_experiments(slug, None, u32::MAX).await?;
    let mut waves: Vec<u32> = all.iter().filter_map(|e| e.wave).collect();
    waves.sort_unstable();
    waves.dedup();
    // A slot whose run outlived its wave's promotion (the child's tool call was dropped mid-sample)
    // lands back in `measured`: close it, it cannot be promoted any more.
    for e in all.into_iter().filter(|e| {
        e.slot.is_some() && matches!(e.state, ExperimentState::Open | ExperimentState::Measured)
    }) {
        if promoted_already(env, slug, e.wave.unwrap_or(0)).await? {
            abandon(env, e).await?;
        }
    }
    let mut notes = Vec::new();
    let mut did = false;
    for wave in waves {
        if promoted_already(env, slug, wave).await? {
            continue;
        }
        let p = promote_env(env, slug, wave).await?;
        did = true;
        notes.push(match p.promoted.len() {
            0 => format!("wave {wave} promoted nothing; its unlogged slots were discarded."),
            1 => format!("wave {wave} promoted experiment {}.", p.promoted[0]),
            _ => format!(
                "wave {wave} promoted experiments {}.",
                p.promoted
                    .iter()
                    .map(u32::to_string)
                    .collect::<Vec<_>>()
                    .join(", ")
            ),
        });
    }
    if !did {
        return Ok((None, notes));
    }
    let spend = env.backend.spend_usd(study.session).await;
    Ok((actor::apply_rules(env, slug, spend).await?, notes))
}

/// Promote after every slot logged (140 §10 step 4); removes the candidate worktrees.
pub async fn promote(a: &mut Studies, slug: &str, wave: u32) -> Result<()> {
    promote_env(&a.env(), slug, wave).await.map(drop)
}

/// [`promote`] on an [`Env`]: serialised per study, idempotent per wave.
pub(crate) async fn promote_env(env: &Env, slug: &str, wave: u32) -> Result<Promotion> {
    let lock = settle_lock(&env.root, slug);
    let _g = lock.lock().await;
    if promoted_already(env, slug, wave).await? {
        return Ok(Promotion::default());
    }
    let mut study = study_by_slug(env, slug).await?;
    let exps: Vec<ExperimentRecord> = env
        .store
        .list_experiments(slug, None, u32::MAX)
        .await?
        .into_iter()
        .filter(|e| e.wave == Some(wave))
        .collect();
    if exps.is_empty() {
        return Err(Error::refused(format!("study `{slug}` has no wave {wave}")));
    }
    let study_wt = actor::worktree_of(&study)?;
    let mut candidates = Vec::new();
    for e in exps.iter().cloned() {
        match e.state {
            ExperimentState::Open | ExperimentState::Measuring | ExperimentState::Measured => {
                abandon(env, e).await?;
            }
            ExperimentState::Kept if e.commit_sha.is_some() => candidates.push(e),
            _ => {}
        }
    }
    // Best first: direction-normalised Δ, then z.
    candidates.sort_by(|a, b| {
        b.delta
            .unwrap_or(f64::MIN)
            .total_cmp(&a.delta.unwrap_or(f64::MIN))
            .then_with(|| b.z.unwrap_or(0.0).total_cmp(&a.z.unwrap_or(0.0)))
    });

    let mut promotion = Promotion::default();
    let mut touched: Vec<(u32, BTreeSet<String>)> = Vec::new();
    for mut exp in candidates {
        let cand_sha = exp.commit_sha.clone().unwrap_or_default();
        let incumbent = actor::incumbent_of(&study)?;
        let files: BTreeSet<String> = env
            .backend
            .files_of(&study_wt, &cand_sha)?
            .into_iter()
            .collect();
        if let Some((m, _)) = touched.iter().find(|(_, f)| !f.is_disjoint(&files)) {
            let shared: Vec<&String> = files
                .iter()
                .filter(|f| touched.iter().any(|(_, t)| t.contains(*f)))
                .collect();
            lose(
                env,
                &study_wt,
                &mut exp,
                &cand_sha,
                format!(
                    "lost wave {wave}: overlaps experiment {m} on {}; retry it on the new incumbent",
                    shared
                        .iter()
                        .map(|s| s.as_str())
                        .collect::<Vec<_>>()
                        .join(", ")
                ),
            )
            .await?;
            continue;
        }
        if !env.backend.cherry_pick(&study_wt, &cand_sha)? {
            let m = touched.last().map_or(0, |(m, _)| *m);
            lose(
                env,
                &study_wt,
                &mut exp,
                &cand_sha,
                format!("lost wave {wave}: conflicts with experiment {m} on the study branch"),
            )
            .await?;
            continue;
        }
        let new_sha = env.backend.head(&study_wt)?;
        let combined = !touched.is_empty();
        if combined {
            // The ratchet's parent is the commit it was picked onto (the reviewer diffs parent..commit).
            exp.parent_sha = incumbent.clone();
        }
        let prior = (!combined).then_some(cand_sha.as_str());
        match confirm(
            env, &study, &mut exp, &study_wt, &new_sha, &incumbent, prior,
        )
        .await?
        {
            Ok(mean_c) => {
                let previous_mean = study.incumbent_mean;
                if !combined {
                    // The slot's samples were measured against this same incumbent: same tree, one pool.
                    actor::retag_samples(&env.store, slug, exp.n, &new_sha).await?;
                }
                exp.commit_sha = Some(new_sha.clone());
                exp.metric = Some(mean_c);
                exp.line = format!("{} · promoted from wave {wave}", exp.line);
                study.incumbent_sha = Some(new_sha.clone());
                study.incumbent_mean = Some(mean_c);
                env.store.update_experiment(&exp).await?;
                env.store.update_study(&study).await?;
                review(
                    env,
                    &mut study,
                    &mut exp,
                    &study_wt,
                    &incumbent,
                    previous_mean,
                )
                .await?;
                if exp.state == ExperimentState::Kept {
                    promotion.promoted.push(exp.n);
                    touched.push((exp.n, files));
                }
            }
            Err(why) => {
                // Back to the incumbent; the pick's diff is saved as the candidate patch.
                let patch = env.runs_dir(slug, exp.n).join("candidate.patch");
                match env.backend.discard(&study_wt, &incumbent, &patch) {
                    Ok(_) => {
                        exp.patch_path = Some(format!("{slug}/runs/{}/candidate.patch", exp.n))
                    }
                    Err(e) => tracing::warn!(%slug, n = exp.n, "reset after a failed confirm: {e}"),
                }
                exp.commit_sha = Some(cand_sha.clone());
                exp.state = ExperimentState::Discarded;
                exp.line = format!("lost wave {wave}: {why}; {}", exp.line);
                exp.finished_at = Some(now());
                env.store.update_experiment(&exp).await?;
            }
        }
    }

    for k in 1..=study.spec.parallel.max(2) {
        let dir = cand_dir(&env.root, slug, k);
        if dir.exists()
            && let Err(e) = env.backend.worktree_remove(&study_wt, &dir)
        {
            tracing::warn!(%slug, k, "candidate worktree not removed: {e}");
        }
    }
    study.sigma = Some(actor::segment_sigma(env, &study).await?);
    env.store.update_study(&study).await?;
    promotion.incumbent = study.incumbent_sha.clone();
    env.store
        .append_event(
            slug,
            EVENT_PROMOTED,
            &serde_json::json!({ "wave": wave, "promoted": promotion.promoted, "incumbent": promotion.incumbent }),
        )
        .await?;
    actor::clear_phase(&env.root, slug);
    Ok(promotion)
}

/// A keep-candidate that is not promoted: its commit's diff becomes the saved patch.
async fn lose(
    env: &Env,
    study_wt: &Path,
    exp: &mut ExperimentRecord,
    cand_sha: &str,
    why: String,
) -> Result<()> {
    let patch = env.runs_dir(&exp.slug, exp.n).join("candidate.patch");
    if let Ok(diff) = env
        .backend
        .diff_text(study_wt, &exp.parent_sha, cand_sha, 4 << 20)
    {
        let _ = std::fs::write(&patch, diff);
        exp.patch_path = Some(format!("{}/runs/{}/candidate.patch", exp.slug, exp.n));
    }
    exp.state = ExperimentState::Discarded;
    exp.line = format!("{why}; {}", exp.line);
    exp.finished_at = Some(now());
    env.store.update_experiment(exp).await
}

/// One confirm sample (more only when the gate asks, up to `confirm_runs`) of
/// `tree` on the study branch against `incumbent`. `prior` names the tree whose
/// samples already count (the slot's candidate commit, same base); a combined
/// pick has none and runs `checks` again. `Ok(Ok(mean))` clears the gate,
/// `Ok(Err(why))` does not.
async fn confirm(
    env: &Env,
    study: &StudyRecord,
    exp: &mut ExperimentRecord,
    study_wt: &Path,
    tree: &str,
    incumbent: &str,
    prior: Option<&str>,
) -> Result<std::result::Result<f64, String>> {
    let slug = study.slug.as_str();
    let gcfg = actor::gate_cfg(&env.cfg, &study.spec)?;
    let evaluator = env.evaluator_dir(slug, study.segment);
    let runs = env.runs_dir(slug, exp.n);
    std::fs::create_dir_all(&runs)?;
    let cancel = actor::cancel_token(&env.root, slug);
    let inc_samples = env.store.samples_of_tree(slug, incumbent).await?;
    let combined = prior.is_none();
    let prior: Vec<Sample> = match prior {
        Some(t) => env.store.samples_of_tree(slug, t).await?,
        None => Vec::new(),
    };
    let mut taken = prior;
    let mut k = exp.n_samples;
    let mut decision = None;
    let max_more = 1 + u32::from(env.cfg.gate.confirm_runs);
    for _ in 0..max_more {
        k += 1;
        actor::progress(
            env,
            slug,
            exp.n,
            &exp.started_at,
            Phase::Sampling,
            k as u8,
            k as u8,
        )
        .await;
        let s = actor::bench_sample(
            env,
            &study.spec,
            SampleRequest {
                slug,
                n: exp.n,
                k,
                tree_sha: tree,
                role: SampleRole::Candidate,
                worktree: study_wt,
                evaluator: &evaluator,
                runs_dir: &runs,
            },
            &cancel,
        )
        .await?;
        env.store.insert_sample(slug, &s).await?;
        exp.n_samples += 1;
        if s.value.is_none() {
            return Ok(Err(format!(
                "the confirm sample crashed (exit {})",
                s.exit.map_or("killed".to_owned(), |e| e.to_string())
            )));
        }
        taken.push(s);
        let input = GateInput {
            candidate: actor::clean_values(&taken),
            incumbent: actor::clean_values(&inc_samples),
            sigma: study.sigma.unwrap_or(0.0),
            largest_accepted_delta: None,
            incumbent_age: 0,
            keep_simpler: false,
            secondaries: actor::secondary_means(&taken, &inc_samples),
        };
        match env.backend.gate_step(&gcfg, &input) {
            GateStep::Confirm { .. } => {}
            d @ GateStep::Decide { .. } => {
                decision = Some(d);
                break;
            }
        }
    }
    let cand_vals = actor::clean_values(&taken);
    let mean_c = actor::mean(&cand_vals);
    let inc_vals = actor::clean_values(&inc_samples);
    let mean_i = if inc_vals.is_empty() {
        study.incumbent_mean.unwrap_or(0.0)
    } else {
        actor::mean(&inc_vals)
    };
    let Some(GateStep::Decide {
        verdict,
        delta,
        delta_pct,
        z,
    }) = decision
    else {
        return Ok(Err("the confirm samples did not decide the gate".into()));
    };
    let line = env.backend.gate_line(
        &GateStep::Decide {
            verdict: verdict.clone(),
            delta,
            delta_pct,
            z,
        },
        &study.spec.metric.unit,
        mean_c,
        mean_i,
        cand_vals.len(),
    );
    if !verdict.is_keep_candidate() {
        return Ok(Err(format!(
            "confirm did not clear the gate ({})",
            verdict.tag()
        )));
    }
    if combined && let Some(c) = study.spec.checks.as_ref() {
        actor::progress(env, slug, exp.n, &exp.started_at, Phase::Checks, 0, 1).await;
        let (exit, _, _) = env
            .backend
            .command(
                &c.command,
                schedule_file::duration(&c.timeout, Duration::from_secs(300)),
                SampleRequest {
                    slug,
                    n: exp.n,
                    k: 0,
                    tree_sha: tree,
                    role: SampleRole::Candidate,
                    worktree: study_wt,
                    evaluator: &evaluator,
                    runs_dir: &runs,
                },
                &cancel,
            )
            .await?;
        if exit != Some(0) {
            return Ok(Err(format!(
                "checks failed on the combined tree (exit {exit:?})"
            )));
        }
    }
    exp.verdict = Some(verdict);
    exp.delta = Some(delta);
    exp.delta_pct = Some(delta_pct);
    exp.z = z;
    exp.line = if line.is_empty() {
        format!("confirmed {delta_pct:+.2} % ({} samples)", cand_vals.len())
    } else {
        line
    };
    Ok(Ok(mean_c))
}

/// §9.1 on a promoted keep, exactly as the sequential log does it.
async fn review(
    env: &Env,
    study: &mut StudyRecord,
    exp: &mut ExperimentRecord,
    study_wt: &Path,
    previous: &str,
    previous_mean: Option<f64>,
) -> Result<()> {
    let Some(model) = env.cfg.reviewer(&study.spec.models, &env.agent) else {
        return Ok(());
    };
    let slug = study.slug.clone();
    let n = exp.n;
    actor::progress(env, &slug, n, &exp.started_at, Phase::Review, 0, 1).await;
    let rv = match env
        .backend
        .review(&env.store, study, n, &model, &env.rt)
        .await
    {
        Ok(v) => v,
        Err(e) => ReviewVerdict::Error {
            reason: e.to_string(),
        },
    };
    let rv = match rv {
        ReviewVerdict::Flagged { reason }
            if env.cfg.review.on_suspicious == OnSuspicious::Revert =>
        {
            let sha = exp.commit_sha.clone().unwrap_or_default();
            let patch = env.runs_dir(&slug, n).join("candidate.patch");
            if let Ok(diff) = env.backend.diff_text(study_wt, previous, &sha, 4 << 20) {
                let _ = std::fs::write(&patch, diff);
                exp.patch_path = Some(format!("{slug}/runs/{n}/candidate.patch"));
            }
            env.backend.reject(study_wt, &slug, n, previous)?;
            study.incumbent_sha = Some(previous.to_owned());
            study.incumbent_mean = previous_mean;
            exp.state = ExperimentState::RejectedByReview;
            env.store
                .append_event(
                    &slug,
                    "review",
                    &serde_json::json!({ "n": n, "reason": reason, "reverted": true }),
                )
                .await?;
            ReviewVerdict::Reverted { reason }
        }
        other => other,
    };
    exp.review = Some(rv);
    env.store.update_experiment(exp).await?;
    env.store.update_study(study).await
}

/// The commit message metadata of a slot keep (the candidate commit; the promoted pick carries it over).
pub(crate) fn keep_meta(study: &StudyRecord, exp: &ExperimentRecord) -> KeepMeta {
    KeepMeta {
        slug: study.slug.clone(),
        n: exp.n,
        description: exp.description.clone(),
        metric_line: actor::metric_line(study, exp),
        category: exp.category.clone(),
        deviations: exp.deviations.clone(),
    }
}

#[cfg(test)]
mod tests {
    //! Real git on `fixtures/toy-sort`; the bench, the reviewer and the
    //! children are doubles (scripted sample values; the test plays the
    //! supervisor and each child through `begin_env` / `run_env` / `log_env`).

    use std::collections::VecDeque;
    use std::process::Command;
    use std::sync::atomic::{AtomicU32, Ordering};

    use tokio_util::sync::CancellationToken;

    use super::*;
    use crate::actor::Backend;
    use crate::store::Store;
    use crate::{config::AgentDefaults, git};

    /// Real git; scripted bench values; concurrency peak for the queue test.
    #[derive(Default)]
    struct Bench {
        values: Mutex<VecDeque<f64>>,
        running: AtomicU32,
        peak: AtomicU32,
        delay_ms: u64,
    }

    impl Bench {
        fn with(values: &[f64]) -> Arc<Self> {
            let b = Self::default();
            *b.values.lock() = values.iter().copied().collect();
            Arc::new(b)
        }
    }

    #[async_trait::async_trait]
    impl Backend for Bench {
        async fn sample(
            &self,
            _: &StudySpec,
            _: &crate::config::Bench,
            req: SampleRequest<'_>,
            _: &CancellationToken,
        ) -> Result<Sample> {
            let running = self.running.fetch_add(1, Ordering::SeqCst) + 1;
            self.peak.fetch_max(running, Ordering::SeqCst);
            if self.delay_ms > 0 {
                tokio::time::sleep(Duration::from_millis(self.delay_ms)).await;
            }
            let value = self.values.lock().pop_front();
            self.running.fetch_sub(1, Ordering::SeqCst);
            Ok(Sample {
                n: req.n,
                k: req.k,
                tree_sha: req.tree_sha.to_owned(),
                role: Some(req.role),
                value,
                exit: Some(if value.is_some() { 0 } else { 1 }),
                wall_ms: 1,
                at: now(),
                ..Sample::default()
            })
        }
        async fn command(
            &self,
            _: &str,
            _: Duration,
            _: SampleRequest<'_>,
            _: &CancellationToken,
        ) -> Result<(Option<i32>, String, u64)> {
            Ok((Some(0), String::new(), 1))
        }
        async fn review(
            &self,
            _: &Store,
            _: &StudyRecord,
            _: u32,
            _: &str,
            _: &runtime::Runtime,
        ) -> Result<ReviewVerdict> {
            Ok(ReviewVerdict::Clean {
                reason: "fine".into(),
            })
        }
        async fn seed_playbook(&self, _: &str, _: &runtime::Runtime) -> Result<Option<String>> {
            Ok(None)
        }
        async fn spend_usd(&self, _: Option<SessionId>) -> f64 {
            0.0
        }
        async fn code_map(
            &self,
            _: &runtime::Runtime,
            _: &str,
            _: &[String],
            _: u32,
        ) -> Option<String> {
            None
        }
    }

    fn sh(cwd: &Path, args: &[&str]) -> String {
        let out = Command::new("git")
            .arg("-C")
            .arg(cwd)
            .args(["-c", "core.hooksPath=/dev/null"])
            .args(args)
            .env("GIT_TERMINAL_PROMPT", "0")
            .output()
            .unwrap();
        assert!(
            out.status.success(),
            "git {args:?}: {}",
            String::from_utf8_lossy(&out.stderr)
        );
        String::from_utf8_lossy(&out.stdout).trim().to_owned()
    }

    struct H {
        _tmp: tempfile::TempDir,
        studies: Studies,
        session: SessionId,
    }

    /// A toy-sort repo, a running study with `parallel` slots, the study session bound.
    async fn harness(bench: Arc<Bench>, parallel: u8) -> H {
        let tmp = tempfile::tempdir().unwrap();
        let repo = tmp.path().join("repo");
        let src = Path::new(env!("CARGO_MANIFEST_DIR")).join("fixtures/toy-sort");
        std::fs::create_dir_all(repo.join("fixtures")).unwrap();
        for f in ["bench.sh", "main.py", "sorter.py", "test.sh", "holdout.sh"] {
            std::fs::copy(src.join(f), repo.join(f)).unwrap();
        }
        for f in ["input.txt", "expected.txt"] {
            std::fs::copy(src.join("fixtures").join(f), repo.join("fixtures").join(f)).unwrap();
        }
        sh(tmp.path(), &["init", "-q", "-b", "main", "repo"]);
        sh(&repo, &["add", "-A"]);
        sh(
            &repo,
            &[
                "-c",
                "user.name=u",
                "-c",
                "user.email=u@x",
                "commit",
                "-q",
                "-m",
                "base",
            ],
        );
        let data_dir = tmp.path().join("data");
        let rt = runtime::Runtime::new(figment::Figment::new(), data_dir.clone());
        let root = data_dir.join("autoresearch");
        let store = Store::open(&root).await.unwrap();
        let mut studies = Studies {
            data_dir,
            root,
            cfg: crate::config::AutoresearchConfig::default(),
            agent: AgentDefaults::default(),
            rt,
            store,
            backend: bench,
        };
        let spec = StudySpec {
            slug: "wave".into(),
            goal: "faster".into(),
            repo: repo.display().to_string(),
            metric: MetricSpec {
                name: "time_ms".into(),
                unit: "ms".into(),
                direction: Direction::Lower,
                min_effect: "0.5%".into(),
            },
            bench: BenchSpec {
                command: "bench.sh".into(),
                repeats: 3,
                exclusive: true,
                ..BenchSpec::default()
            },
            scope: vec!["*.py".into()],
            budget: BudgetSpec {
                experiments: 50,
                ..BudgetSpec::default()
            },
            parallel,
            ..StudySpec::default()
        };
        actor::create(
            &mut studies,
            Create {
                spec,
                origin: SessionId::new(),
                start: StartWhen::Now,
            },
        )
        .await
        .unwrap();
        for _ in 0..300 {
            let s = studies.store.get_study("wave").await.unwrap().unwrap();
            if s.state == StudyState::Running {
                break;
            }
            if let StudyState::Failed { reason } = &s.state {
                panic!("study failed: {reason}");
            }
            tokio::time::sleep(Duration::from_millis(20)).await;
        }
        let session = SessionId::new();
        binding::on_fired(&mut studies, "autoresearch-wave", session)
            .await
            .unwrap();
        H {
            _tmp: tmp,
            studies,
            session,
        }
    }

    async fn study(h: &H) -> StudyRecord {
        h.studies.store.get_study("wave").await.unwrap().unwrap()
    }

    /// The off-mailbox settle of `wave` has recorded its event.
    async fn wait_promoted(h: &H, wave: u32) {
        for _ in 0..250 {
            if promoted_already(&h.studies.env(), "wave", wave)
                .await
                .unwrap()
            {
                return;
            }
            tokio::time::sleep(Duration::from_millis(20)).await;
        }
        panic!("wave {wave} never promoted");
    }

    fn append(dir: &str, file: &str, text: &str) {
        let p = Path::new(dir).join(file);
        let mut s = std::fs::read_to_string(&p).unwrap();
        s.push_str(text);
        std::fs::write(p, s).unwrap();
    }

    fn turn_finished(session: SessionId) -> agent::AgentEvent {
        agent::AgentEvent::TurnFinished {
            session,
            turn: runtime::TurnId::new(),
            stop_reason: "end_turn".into(),
            usage: agent::Usage::default(),
            context: 0,
        }
    }

    /// One child: `experiment_begin{slot}`, an edit, `experiment_run`, `experiment_log`.
    async fn child_round(
        h: &H,
        child: SessionId,
        k: u8,
        file: &str,
        log: bool,
    ) -> Option<LogReport> {
        let env = h.studies.env();
        register_child(h.session, child);
        let brief = actor::begin_env(
            &env,
            Begin {
                session: child,
                slot: Some(k),
            },
        )
        .await
        .unwrap();
        assert_eq!(slot_of(child), Some(k));
        let mine = &brief.slots.as_ref().unwrap()[0];
        assert_eq!(mine.k, k);
        assert!(
            brief.text.contains(&format!("<slot k=\"{k}\"")),
            "{}",
            brief.text
        );
        append(&mine.worktree, file, &format!("# slot {k}\n"));
        let r = actor::run_env(
            &env,
            Run {
                session: child,
                slot: slot_of(child),
                args: RunArgs {
                    idea: format!("idea of slot {k}"),
                    category: format!("c{k}"),
                    hypothesis: "h".into(),
                    ..RunArgs::default()
                },
            },
        )
        .await
        .unwrap();
        assert_eq!(r.verdict, Verdict::Improved, "{}", r.line);
        if !log {
            return None;
        }
        Some(
            actor::log_env(
                &env,
                Log {
                    session: child,
                    slot: slot_of(child),
                    args: LogArgs {
                        description: format!("slot {k} change"),
                        learned: "l".into(),
                        ..LogArgs::default()
                    },
                },
            )
            .await
            .unwrap(),
        )
    }

    #[tokio::test]
    async fn three_slot_wave_promotes_best_combines_disjoint_and_drops_overlap() {
        // baseline ×3 · slot 1 · slot 2 · slot 3 · confirm #1 · confirm #2 (on the new incumbent)
        let bench = Bench::with(&[100.0, 100.0, 100.0, 90.0, 95.0, 97.0, 90.0, 85.0]);
        let mut h = harness(bench.clone(), 3).await;
        let env = h.studies.env();
        let base = study(&h).await.incumbent_sha.unwrap();
        let study_wt = PathBuf::from(study(&h).await.worktree.unwrap());

        // The supervisor's begin opens the wave.
        let brief = actor::begin_env(
            &env,
            Begin {
                session: h.session,
                slot: None,
            },
        )
        .await
        .unwrap();
        let slots = brief.slots.clone().unwrap();
        assert_eq!(slots.len(), 3);
        assert!(
            brief.text.contains("<wave n=\"1\" slots=\"3\">"),
            "{}",
            brief.text
        );
        for (i, s) in slots.iter().enumerate() {
            assert_eq!(s.k as usize, i + 1);
            assert_eq!(s.n as usize, i + 1);
            let dir = PathBuf::from(&s.worktree);
            assert_eq!(dir, cand_dir(&h.studies.root, "wave", s.k));
            assert_eq!(git::head(&dir).unwrap(), base, "candidate at the incumbent");
        }
        let rows = h
            .studies
            .store
            .list_experiments("wave", None, 10)
            .await
            .unwrap();
        assert_eq!(rows.len(), 3);
        for r in &rows {
            assert_eq!(r.wave, Some(1));
            assert_eq!(r.parent_sha, base);
            assert_eq!(r.state, ExperimentState::Open);
        }
        assert_eq!(
            rows.iter().map(|r| r.slot).collect::<Vec<_>>(),
            vec![Some(3), Some(2), Some(1)]
        );
        // A session nobody spawned is refused; a slot a supervisor did not open is refused.
        let e = actor::begin_env(
            &env,
            Begin {
                session: SessionId::new(),
                slot: Some(1),
            },
        )
        .await
        .unwrap_err()
        .to_string();
        assert!(e.contains("not an autoresearch session"), "{e}");
        let e = resolve(&h.studies, h.session, 7)
            .await
            .unwrap_err()
            .to_string();
        assert!(e.contains("slot 7 has no open experiment"), "{e}");
        assert_eq!(
            resolve(&h.studies, h.session, 2).await.unwrap(),
            (cand_dir(&h.studies.root, "wave", 2), 2)
        );

        // Slot 1 and slot 3 touch sorter.py (overlap), slot 2 touches main.py (disjoint).
        let (c1, c2, c3) = (SessionId::new(), SessionId::new(), SessionId::new());
        let r1 = child_round(&h, c1, 1, "sorter.py", true).await.unwrap();
        assert_eq!(r1.applied, ExperimentState::Kept);
        assert_eq!(r1.stop, None);
        assert_eq!(
            study(&h).await.incumbent_sha.unwrap(),
            base,
            "a slot keep does not move the incumbent"
        );
        assert_eq!(git::head(&study_wt).unwrap(), base);
        let r2 = child_round(&h, c2, 2, "main.py", true).await.unwrap();
        assert_eq!(r2.applied, ExperimentState::Kept);
        assert_eq!(study(&h).await.incumbent_sha.unwrap(), base);
        let r3 = child_round(&h, c3, 3, "sorter.py", true).await.unwrap();

        // The last log promoted: #1 (Δ 10) onto the branch, #2 (disjoint) after it, #3 (overlaps #1) lost.
        let after = study(&h).await;
        let incumbent = after.incumbent_sha.clone().unwrap();
        assert_ne!(incumbent, base);
        assert_eq!(git::head(&study_wt).unwrap(), incumbent);
        assert_eq!(
            sh(&study_wt, &["rev-list", "--count", "HEAD"]),
            "3",
            "base + two picks"
        );
        assert_eq!(after.incumbent_mean, Some(85.0));
        assert_eq!(r3.applied, ExperimentState::Discarded);
        assert_eq!(r3.incumbent.sha, incumbent);
        assert_eq!(r3.incumbent.kept, 2);
        let e1 = h
            .studies
            .store
            .get_experiment("wave", 1)
            .await
            .unwrap()
            .unwrap();
        let e2 = h
            .studies
            .store
            .get_experiment("wave", 2)
            .await
            .unwrap()
            .unwrap();
        let e3 = h
            .studies
            .store
            .get_experiment("wave", 3)
            .await
            .unwrap()
            .unwrap();
        assert_eq!(e1.state, ExperimentState::Kept);
        assert_eq!(e2.state, ExperimentState::Kept);
        assert_eq!(e2.commit_sha.as_deref(), Some(incumbent.as_str()));
        assert_eq!(
            e2.parent_sha,
            e1.commit_sha.clone().unwrap(),
            "the combined pick's parent is the first promotion"
        );
        let log = sh(&study_wt, &["log", "--format=%H%n%B", "HEAD"]);
        assert!(log.contains(&format!("{}\n", e1.commit_sha.clone().unwrap())));
        assert!(
            log.contains("Autoresearch-Run: 1") && log.contains("Autoresearch-Run: 2"),
            "{log}"
        );
        assert!(e1.line.contains("promoted from wave 1"), "{}", e1.line);
        assert!(matches!(e1.review, Some(ReviewVerdict::Clean { .. })));
        assert!(matches!(e2.review, Some(ReviewVerdict::Clean { .. })));
        assert_eq!(e3.state, ExperimentState::Discarded);
        assert!(
            e3.line.contains("overlaps experiment 1 on sorter.py"),
            "{}",
            e3.line
        );
        let patch = h.studies.root.join(e3.patch_path.unwrap());
        assert!(
            std::fs::read_to_string(&patch)
                .unwrap()
                .contains("# slot 3")
        );
        // Samples follow the promoted trees: slot sample + confirm for #1, one confirm for #2.
        assert_eq!(
            h.studies
                .store
                .samples_of_tree("wave", &e1.commit_sha.unwrap())
                .await
                .unwrap()
                .len(),
            2
        );
        assert_eq!(
            h.studies
                .store
                .samples_of_tree("wave", &incumbent)
                .await
                .unwrap()
                .len(),
            1
        );
        assert!(
            bench.values.lock().is_empty(),
            "exactly eight samples were spent"
        );
        // Every candidate worktree is gone.
        for k in 1..=3 {
            assert!(!cand_dir(&h.studies.root, "wave", k).exists());
        }
        assert!(!sh(&study_wt, &["worktree", "list"]).contains("cand-"));
        // The ledger says what happened.
        let ev = h
            .studies
            .store
            .list_events("wave", Some(EVENT_PROMOTED), 10)
            .await
            .unwrap();
        assert_eq!(ev.len(), 1);
        assert_eq!(ev[0].payload["promoted"], serde_json::json!([1, 2]));
        assert_eq!(ev[0].payload["incumbent"], serde_json::json!(incumbent));
        // No slot is left open; the children's bindings go with their turns.
        assert!(wave_closed(&env, "wave", 1).await.unwrap());
        for c in [c1, c2, c3] {
            on_agent_event(&mut h.studies, &turn_finished(c))
                .await
                .unwrap();
            assert_eq!(slot_of(c), None);
        }
    }

    #[tokio::test]
    async fn abandoned_slot_is_discarded_at_the_supervisors_turn_end_and_the_next_wave_opens() {
        // baseline ×3 · slot 1 · slot 2 (measured, never logged) · confirm #1
        let bench = Bench::with(&[100.0, 100.0, 100.0, 90.0, 96.0, 90.0]);
        let mut h = harness(bench.clone(), 2).await;
        let env = h.studies.env();
        let base = study(&h).await.incumbent_sha.unwrap();
        let brief = actor::begin_env(
            &env,
            Begin {
                session: h.session,
                slot: None,
            },
        )
        .await
        .unwrap();
        assert_eq!(brief.slots.as_ref().unwrap().len(), 2);
        let (c1, c2) = (SessionId::new(), SessionId::new());
        child_round(&h, c1, 1, "sorter.py", true).await.unwrap();
        // Slot 2 measured and never logged; its child's turn ends first, then the supervisor's.
        child_round(&h, c2, 2, "main.py", false).await;
        on_agent_event(&mut h.studies, &turn_finished(c2))
            .await
            .unwrap();
        let e2 = h
            .studies
            .store
            .get_experiment("wave", 2)
            .await
            .unwrap()
            .unwrap();
        assert_eq!(e2.state, ExperimentState::Abandoned);
        assert_eq!(
            slot_of(c2),
            None,
            "the child's binding is dropped with its turn"
        );
        assert_eq!(
            study(&h).await.incumbent_sha.unwrap(),
            base,
            "not promoted before the wave settles"
        );
        on_agent_event(&mut h.studies, &turn_finished(h.session))
            .await
            .unwrap();
        wait_promoted(&h, 1).await;
        let after = study(&h).await;
        let incumbent = after.incumbent_sha.clone().unwrap();
        assert_ne!(incumbent, base);
        let e1 = h
            .studies
            .store
            .get_experiment("wave", 1)
            .await
            .unwrap()
            .unwrap();
        assert_eq!(e1.state, ExperimentState::Kept);
        assert_eq!(e1.commit_sha.as_deref(), Some(incumbent.as_str()));
        assert!(!cand_dir(&h.studies.root, "wave", 1).exists());
        assert!(!cand_dir(&h.studies.root, "wave", 2).exists());
        let incidents = h
            .studies
            .store
            .list_events("wave", Some("incident"), 10)
            .await
            .unwrap();
        assert!(
            incidents
                .iter()
                .any(|e| e.payload["n"] == 2 && e.payload["slot"] == 2),
            "{incidents:?}"
        );
        // Settling again is a no-op.
        on_agent_event(&mut h.studies, &turn_finished(h.session))
            .await
            .unwrap();
        tokio::time::sleep(Duration::from_millis(200)).await;
        assert_eq!(
            h.studies
                .store
                .list_events("wave", Some(EVENT_PROMOTED), 10)
                .await
                .unwrap()
                .len(),
            1
        );
        // The next fire opens wave 2 from the new incumbent; categories rotate over what was seen.
        let brief = actor::begin_env(
            &env,
            Begin {
                session: h.session,
                slot: None,
            },
        )
        .await
        .unwrap();
        let slots = brief.slots.unwrap();
        assert_eq!(slots.iter().map(|s| s.n).collect::<Vec<_>>(), vec![3, 4]);
        // Seen: c1 (slot 1), c2 (slot 2 measured before it was abandoned); wave 2 rotates by one.
        assert_eq!(
            slots
                .iter()
                .map(|s| s.category.as_str())
                .collect::<Vec<_>>(),
            vec!["c2", "c1"]
        );
        for s in &slots {
            let e = h
                .studies
                .store
                .get_experiment("wave", s.n)
                .await
                .unwrap()
                .unwrap();
            assert_eq!(
                (e.wave, e.slot, e.parent_sha.as_str()),
                (Some(2), Some(s.k), incumbent.as_str())
            );
            assert_eq!(git::head(Path::new(&s.worktree)).unwrap(), incumbent);
        }
        assert_eq!(bench.values.lock().len(), 0);
    }

    #[tokio::test]
    async fn exclusive_samples_take_the_bench_queue_one_at_a_time() {
        let bench = Arc::new(Bench {
            delay_ms: 30,
            ..Bench::default()
        });
        *bench.values.lock() = (0..6).map(|_| 100.0).collect();
        let tmp = tempfile::tempdir().unwrap();
        let dir = tmp.path().to_path_buf();
        let rt = runtime::Runtime::new(figment::Figment::new(), tmp.path().to_path_buf());
        let root = tmp.path().join("autoresearch");
        let studies = Studies {
            data_dir: tmp.path().to_path_buf(),
            root: root.clone(),
            cfg: crate::config::AutoresearchConfig::default(),
            agent: AgentDefaults::default(),
            rt,
            store: Store::open(&root).await.unwrap(),
            backend: bench.clone(),
        };
        let env = studies.env();
        let run = |exclusive: bool| {
            let env = env.clone();
            let bench = bench.clone();
            let dir = dir.clone();
            async move {
                bench.peak.store(0, Ordering::SeqCst);
                let spec = StudySpec {
                    bench: BenchSpec {
                        exclusive,
                        ..BenchSpec::default()
                    },
                    ..StudySpec::default()
                };
                let one = |k: u32| {
                    let env = &env;
                    let spec = &spec;
                    let tmp = dir.as_path();
                    async move {
                        actor::bench_sample(
                            env,
                            spec,
                            SampleRequest {
                                slug: "q",
                                n: 1,
                                k,
                                tree_sha: "t",
                                role: SampleRole::Candidate,
                                worktree: tmp,
                                evaluator: tmp,
                                runs_dir: tmp,
                            },
                            &CancellationToken::new(),
                        )
                        .await
                        .unwrap()
                    }
                };
                tokio::join!(one(1), one(2), one(3));
                bench.peak.load(Ordering::SeqCst)
            }
        };
        assert_eq!(run(true).await, 1, "exclusive: serialised");
        assert!(run(false).await >= 2, "not exclusive: concurrent");
    }
}
