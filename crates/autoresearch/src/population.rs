//! 159 W4 — search, not a ratchet: cells, islands, sampled parents, elite refs (159 §9).
//! Owner: W4. Signatures of the per-kind plug-ins are C's; refs come from `crate::refs` (W1).
//!
//! Two layers. The pure [`Archive`] model (cells keyed by island plus binned descriptors, the
//! parent weights, acceptance, migration offers) is what the simulations drive. The env side
//! maps it onto `archive` rows of kind `cell`, the refs `refs/autoresearch/<slug>/cell/<key>`
//! and the study's incumbent (the champion): [`parent_of`] picks and re-measures a parent for a
//! slot, [`promote`] replaces wave promotion (the archive acceptance of every slot of a wave,
//! then migration, then the champion), [`settle`] is the acceptance of one slot.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::time::Duration;

use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use tokio_util::sync::CancellationToken;

use crate::actor::{self, Env};
use crate::error::{Error, Result};
use crate::finalize::Headline;
use crate::refs::{self, RefKind};
use crate::runner::SampleRequest;
use crate::setup::SetupRow;
use crate::store::{ExperimentRecord, Store, StudyPatch, StudyRecord, now};
use crate::types::*;
use crate::wave::Promotion;
use crate::{git, schedule_file};

/// Not reached by a 140 study.
const OWNER: &str = "W4";
/// `archive.kind` of every row here.
const KIND: &str = "cell";
/// `events.kind` written by [`settle`]: `{wave, n, key, outcome}`.
pub const EVENT_ARCHIVE: &str = "archive";
/// Share of parent picks that are uniform over the island (159 §9 "Parent sampling").
const UNIFORM_SHARE: f64 = 0.1;
/// Default `migrate_every` (waves).
const MIGRATE_EVERY: u32 = 4;

// ───────────────────────────── spec ─────────────────────────────

/// Where a descriptor's value comes from (159 §9 "Cells").
#[derive(
    Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize, schemars::JsonSchema,
)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS), ts(export))]
#[serde(rename_all = "snake_case")]
pub enum DescriptorSource {
    /// A `METRIC <name>` line (the primary itself when `name` is the metric).
    #[default]
    Secondary,
    /// An `ASI <name>=<number>` line.
    Asi,
    /// The case (by sorted id) the tree improves most over the base — GEPA's per-case front in cell form.
    CaseBest,
}

/// One binned behaviour descriptor: `{source, name, bins: [edges]}`. A value lands in bin
/// `|{e ∈ bins : e ≤ v}|`, so `k` edges make `k + 1` bins.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize, schemars::JsonSchema)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS), ts(export))]
#[serde(default)]
pub struct Descriptor {
    /// Source of the value.
    pub source: DescriptorSource,
    /// Metric / ASI key; ignored for `case_best`.
    pub name: String,
    /// Ascending bin edges.
    pub bins: Vec<f64>,
}

impl Descriptor {
    fn label(&self) -> String {
        match self.source {
            DescriptorSource::CaseBest => "case_best".to_owned(),
            _ => self.name.clone(),
        }
    }
}

/// `PopulationSpec` with its defaults resolved (159 §9 "Islands", "Re-measures").
#[derive(Clone, Debug, PartialEq)]
pub struct Cfg {
    /// Independent archives.
    pub islands: u8,
    /// Zero, one or two descriptors.
    pub descriptors: Vec<Descriptor>,
    /// Migration period in waves.
    pub migrate_every: u32,
    /// Re-measure a picked elite whose newest sample is older than this many experiments.
    pub remeasure_after: u32,
}

/// Resolve the spec's block: islands = `parallel` clamped to 2–4, migration every 4 waves,
/// `remeasure_after` from the gate config unless set; at most two descriptors, edges sorted.
pub fn cfg(spec: &StudySpec, gate_remeasure_after: u32) -> Cfg {
    let p = spec.population.clone().unwrap_or_default();
    let islands = if (2..=4).contains(&p.islands) {
        p.islands
    } else {
        spec.parallel.clamp(2, 4)
    };
    let mut descriptors: Vec<Descriptor> = p.descriptors.into_iter().take(2).collect();
    for d in &mut descriptors {
        d.bins.retain(|e| e.is_finite());
        d.bins.sort_by(f64::total_cmp);
        d.bins.dedup();
    }
    Cfg {
        islands,
        descriptors,
        migrate_every: if p.migrate_every == 0 {
            MIGRATE_EVERY
        } else {
            p.migrate_every
        },
        remeasure_after: if p.remeasure_after == 0 {
            gate_remeasure_after
        } else {
            p.remeasure_after
        },
    }
}

// ───────────────────────────── cells ─────────────────────────────

/// The bin of `v`: how many edges are ≤ it. NaN lands in bin 0.
pub fn bin(v: f64, edges: &[f64]) -> usize {
    edges.iter().filter(|e| v >= **e).count()
}

/// `i<island>:<bin>[.<bin>]`; `i<island>:0` with no descriptors.
pub fn cell_key(island: u8, bins: &[usize]) -> String {
    if bins.is_empty() {
        return format!("i{island}:0");
    }
    let b: Vec<String> = bins.iter().map(usize::to_string).collect();
    format!("i{island}:{}", b.join("."))
}

/// The island of a cell key (`i2:…` → 2).
pub fn island_of(key: &str) -> Option<u8> {
    key.strip_prefix('i')?.split(':').next()?.parse().ok()
}

/// `key` moved to `island` (same bins).
pub fn rekey(key: &str, island: u8) -> String {
    match key.split_once(':') {
        Some((_, bins)) => format!("i{island}:{bins}"),
        None => format!("i{island}:0"),
    }
}

fn mean(v: &[f64]) -> f64 {
    if v.is_empty() {
        0.0
    } else {
        v.iter().sum::<f64>() / v.len() as f64
    }
}

/// Per-case means over clean samples (`Sample.cases`, the id's last value).
fn case_means(samples: &[Sample]) -> BTreeMap<String, f64> {
    let mut acc: BTreeMap<String, Vec<f64>> = BTreeMap::new();
    for s in samples.iter().filter(|s| !s.noisy && !s.lost) {
        for (id, v) in &s.cases {
            if v.is_finite() {
                acc.entry(id.clone()).or_default().push(*v);
            }
        }
    }
    acc.into_iter().map(|(id, v)| (id, mean(&v))).collect()
}

/// The descriptor values of a tree from its clean samples, keyed by descriptor label. A
/// descriptor no sample reports is left out (its bin is 0). `base_cases` are the base tree's
/// per-case means for `case_best`.
pub fn descriptor_values(
    cfg: &Cfg,
    metric: &MetricSpec,
    samples: &[Sample],
    base_cases: &BTreeMap<String, f64>,
) -> BTreeMap<String, f64> {
    let clean: Vec<&Sample> = samples.iter().filter(|s| !s.noisy && !s.lost).collect();
    let mut out = BTreeMap::new();
    for d in &cfg.descriptors {
        let v = match d.source {
            DescriptorSource::Secondary => {
                let vals: Vec<f64> = clean
                    .iter()
                    .filter_map(|s| {
                        if d.name == metric.name {
                            s.value
                        } else {
                            s.secondaries.get(&d.name).copied()
                        }
                    })
                    .filter(|v| v.is_finite())
                    .collect();
                (!vals.is_empty()).then(|| mean(&vals))
            }
            DescriptorSource::Asi => {
                let vals: Vec<f64> = clean
                    .iter()
                    .filter_map(|s| s.asi.get(&d.name).and_then(|t| t.trim().parse().ok()))
                    .filter(|v: &f64| v.is_finite())
                    .collect();
                (!vals.is_empty()).then(|| mean(&vals))
            }
            DescriptorSource::CaseBest => {
                let own = case_means(samples);
                // Improvement over the base per case (direction-normalised); the raw value's
                // own direction when the base has no case rows.
                let mut best: Option<(usize, f64)> = None;
                for (i, (id, v)) in own.iter().enumerate() {
                    let score = match base_cases.get(id) {
                        Some(b) => crate::gate::normalise(metric.direction, *v, *b),
                        None => crate::gate::normalise(metric.direction, *v, 0.0),
                    };
                    if best.is_none_or(|(_, s)| score > s) {
                        best = Some((i, score));
                    }
                }
                best.map(|(i, _)| i as f64)
            }
        };
        if let Some(v) = v {
            out.insert(d.label(), v);
        }
    }
    out
}

/// The bins of a tree's descriptor values, in descriptor order.
pub fn bins_of(cfg: &Cfg, values: &BTreeMap<String, f64>) -> Vec<usize> {
    cfg.descriptors
        .iter()
        .map(|d| values.get(&d.label()).map_or(0, |v| bin(*v, &d.bins)))
        .collect()
}

// ───────────────────────────── the model ─────────────────────────────

/// A tiny deterministic RNG (splitmix64): the parent pick is reproducible per experiment.
#[derive(Clone, Debug)]
pub struct Rng(pub u64);

impl Rng {
    /// Seeded from a string (sha256 prefix).
    pub fn seeded(s: &str) -> Self {
        let h = Sha256::digest(s.as_bytes());
        Self(u64::from_le_bytes(h[..8].try_into().expect("8 bytes")))
    }

    fn next_u64(&mut self) -> u64 {
        self.0 = self.0.wrapping_add(0x9E37_79B9_7F4A_7C15);
        let mut z = self.0;
        z = (z ^ (z >> 30)).wrapping_mul(0xBF58_476D_1CE4_E5B9);
        z = (z ^ (z >> 27)).wrapping_mul(0x94D0_49BB_1331_11EB);
        z ^ (z >> 31)
    }

    /// Uniform in [0, 1).
    pub fn uniform(&mut self) -> f64 {
        (self.next_u64() >> 11) as f64 / (1u64 << 53) as f64
    }

    /// Standard normal (Box–Muller).
    pub fn normal(&mut self) -> f64 {
        let u1 = self.uniform().max(f64::MIN_POSITIVE);
        let u2 = self.uniform();
        (-2.0 * u1.ln()).sqrt() * (std::f64::consts::TAU * u2).cos()
    }
}

/// One live elite (an `archive` row of kind `cell`).
#[derive(Clone, Debug, PartialEq)]
pub struct Elite {
    /// `i<island>:<bin>[.<bin>]`.
    pub key: String,
    /// Island.
    pub island: u8,
    /// The tree.
    pub sha: String,
    /// The experiment that landed it (0 = the base).
    pub n: u32,
    /// Headline mean.
    pub score: f64,
    /// Descriptor values.
    pub descriptor: BTreeMap<String, f64>,
    /// Children sampled from it.
    pub children: u32,
}

/// What acceptance did with a child.
#[derive(Clone, Debug, PartialEq)]
pub enum Outcome {
    /// Took an empty cell.
    Entered,
    /// Beat the cell's elite (experiment `n`).
    Replaced {
        /// The elite it replaced.
        n: u32,
    },
    /// Not archived.
    Rejected(String),
}

impl Outcome {
    fn word(&self) -> &'static str {
        match self {
            Self::Entered => "entered",
            Self::Replaced { .. } => "replaced",
            Self::Rejected(_) => "rejected",
        }
    }

    fn accepted(&self) -> bool {
        !matches!(self, Self::Rejected(_))
    }
}

/// `s(rank)`: a logistic in the elite's score rank within its island — the top elite near 1,
/// the median at 0.5 (159 §9 "Parent sampling").
pub fn rank_weight(rank: usize, n: usize) -> f64 {
    if n <= 1 {
        return 1.0;
    }
    let m = (n as f64 - 1.0) / 2.0;
    let slope = 6.0 / n as f64;
    1.0 / (1.0 + ((rank as f64 - m) * slope).exp())
}

/// The archive: every live elite over `islands` islands.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Archive {
    /// Island count.
    pub islands: u8,
    /// Every elite, unordered.
    pub cells: Vec<Elite>,
}

impl Archive {
    /// An empty archive.
    pub fn new(islands: u8) -> Self {
        Self {
            islands,
            cells: Vec::new(),
        }
    }

    /// The elite holding `key`.
    pub fn get(&self, key: &str) -> Option<&Elite> {
        self.cells.iter().find(|c| c.key == key)
    }

    fn position(&self, key: &str) -> Option<usize> {
        self.cells.iter().position(|c| c.key == key)
    }

    /// Indices of `island`'s elites, best first (ties by lowest `n`).
    pub fn ranked(&self, island: u8, direction: Direction) -> Vec<usize> {
        let mut idx: Vec<usize> = (0..self.cells.len())
            .filter(|&i| self.cells[i].island == island)
            .collect();
        idx.sort_by(|&a, &b| {
            let (ea, eb) = (&self.cells[a], &self.cells[b]);
            better(direction, ea.score, eb.score).then_with(|| ea.n.cmp(&eb.n))
        });
        idx
    }

    /// The elite with the best headline mean over every island (ties by lowest `n`).
    pub fn champion(&self, direction: Direction) -> Option<&Elite> {
        self.cells
            .iter()
            .min_by(|a, b| better(direction, a.score, b.score).then_with(|| a.n.cmp(&b.n)))
    }

    /// The parent weights of `island`'s elites, best first: `s(rank) / (1 + children)`.
    pub fn weights(&self, island: u8, direction: Direction) -> Vec<(usize, f64)> {
        let ranked = self.ranked(island, direction);
        let n = ranked.len();
        ranked
            .into_iter()
            .enumerate()
            .map(|(rank, i)| {
                (
                    i,
                    rank_weight(rank, n) / (1.0 + f64::from(self.cells[i].children)),
                )
            })
            .collect()
    }

    /// Sample a parent on `island` (weighted; uniform with probability [`UNIFORM_SHARE`]) and
    /// count the child against it. `None` when the island is empty.
    pub fn pick_parent(
        &mut self,
        island: u8,
        direction: Direction,
        rng: &mut Rng,
    ) -> Option<usize> {
        let w = self.weights(island, direction);
        if w.is_empty() {
            return None;
        }
        let i = if rng.uniform() < UNIFORM_SHARE {
            w[(rng.uniform() * w.len() as f64) as usize % w.len()].0
        } else {
            let total: f64 = w.iter().map(|(_, x)| x).sum();
            let mut r = rng.uniform() * total;
            let mut pick = w[w.len() - 1].0;
            for (i, x) in &w {
                if r < *x {
                    pick = *i;
                    break;
                }
                r -= x;
            }
            pick
        };
        self.cells[i].children += 1;
        Some(i)
    }

    /// Put `child` into its cell: an empty cell takes it; an occupied one when `beats(holder)`.
    pub fn accept(&mut self, child: Elite, beats: impl FnOnce(&Elite) -> bool) -> Outcome {
        match self.position(&child.key) {
            None => {
                self.cells.push(child);
                Outcome::Entered
            }
            Some(i) => {
                let holder = &self.cells[i];
                if beats(holder) {
                    let n = holder.n;
                    self.cells[i] = child;
                    Outcome::Replaced { n }
                } else {
                    Outcome::Rejected(format!("cell {} held by #{}", child.key, holder.n))
                }
            }
        }
    }

    /// Migration offers (159 §9 "Islands"): island i's best elite, re-keyed to island i + 1.
    /// `(from index, offered elite)`; the caller decides against the holder with [`Self::accept`].
    pub fn offers(&self, direction: Direction) -> Vec<(usize, Elite)> {
        if self.islands < 2 {
            return Vec::new();
        }
        (0..self.islands)
            .filter_map(|i| {
                let from = *self.ranked(i, direction).first()?;
                let to = (i + 1) % self.islands;
                let mut e = self.cells[from].clone();
                e.key = rekey(&e.key, to);
                e.island = to;
                e.children = 0;
                Some((from, e))
            })
            .collect()
    }
}

/// `Ordering` so that the better score sorts first (`a` better than `b` → `Less`).
fn better(direction: Direction, a: f64, b: f64) -> std::cmp::Ordering {
    match direction {
        Direction::Lower => a.total_cmp(&b),
        Direction::Higher => b.total_cmp(&a),
    }
}

// ───────────────────────────── ledger ↔ model ─────────────────────────────

fn elite_of(r: &ArchiveRow, metric: &str) -> Elite {
    Elite {
        key: r.key.clone(),
        island: r.island.or_else(|| island_of(&r.key)).unwrap_or(0),
        sha: r.sha.clone(),
        n: r.n,
        score: r.scores.get(metric).copied().unwrap_or(f64::NAN),
        descriptor: r.descriptor.clone(),
        children: r.children,
    }
}

fn row_of(slug: &str, metric: &str, e: &Elite) -> ArchiveRow {
    ArchiveRow {
        slug: slug.to_owned(),
        kind: KIND.to_owned(),
        key: e.key.clone(),
        sha: e.sha.clone(),
        n: e.n,
        ref_name: refs::name(slug, RefKind::Cell, &e.key),
        scores: BTreeMap::from([(metric.to_owned(), e.score)]),
        descriptor: e.descriptor.clone(),
        island: Some(e.island),
        children: e.children,
        at: now(),
    }
}

async fn load(store: &Store, study: &StudyRecord, islands: u8) -> Result<Archive> {
    let rows = store.archive_list(&study.slug, Some(KIND)).await?;
    Ok(Archive {
        islands,
        cells: rows
            .iter()
            .map(|r| elite_of(r, &study.spec.metric.name))
            .collect(),
    })
}

/// Write the model back: changed or new cells are upserted (ref updated when the sha moved),
/// vanished cells removed with their ref.
async fn save(
    env: &Env,
    repo: &Path,
    study: &StudyRecord,
    before: &Archive,
    after: &Archive,
) -> Result<()> {
    let slug = study.slug.as_str();
    let metric = study.spec.metric.name.as_str();
    for e in &after.cells {
        match before.get(&e.key) {
            Some(b) if b == e => {}
            Some(b) => {
                env.store.archive_put(&row_of(slug, metric, e)).await?;
                if b.sha != e.sha {
                    refs::update(repo, slug, RefKind::Cell, &e.key, &e.sha)?;
                }
            }
            None => {
                env.store.archive_put(&row_of(slug, metric, e)).await?;
                refs::update(repo, slug, RefKind::Cell, &e.key, &e.sha)?;
            }
        }
    }
    for b in &before.cells {
        if after.get(&b.key).is_none() {
            env.store.archive_remove(slug, KIND, &b.key).await?;
            refs::delete(repo, slug, RefKind::Cell, &b.key)?;
        }
    }
    Ok(())
}

/// Clean candidate / incumbent / baseline / final values of a tree (never holdout rows).
async fn tree_values(store: &Store, slug: &str, sha: &str) -> Result<(Vec<Sample>, Vec<f64>)> {
    let samples = store.samples_of_tree(slug, sha).await?;
    let values = actor::bench_values(&samples);
    Ok((samples, values))
}

async fn base_cases(
    store: &Store,
    study: &StudyRecord,
    cfg: &Cfg,
) -> Result<BTreeMap<String, f64>> {
    if !cfg
        .descriptors
        .iter()
        .any(|d| d.source == DescriptorSource::CaseBest)
    {
        return Ok(BTreeMap::new());
    }
    let Some(base) = study.base_sha.as_deref() else {
        return Ok(BTreeMap::new());
    };
    Ok(case_means(&store.samples_of_tree(&study.slug, base).await?))
}

/// The base as the first elite of every island (the archive's seed).
async fn seed(
    env: &Env,
    repo: &Path,
    study: &StudyRecord,
    cfg: &Cfg,
    a: &mut Archive,
) -> Result<()> {
    let sha = actor::incumbent_of(study)?;
    let (samples, values) = tree_values(&env.store, &study.slug, &sha).await?;
    let score = if values.is_empty() {
        study.incumbent_mean.unwrap_or(0.0)
    } else {
        mean(&values)
    };
    let descriptor = descriptor_values(
        cfg,
        &study.spec.metric,
        &samples,
        &base_cases(&env.store, study, cfg).await?,
    );
    let bins = bins_of(cfg, &descriptor);
    let before = a.clone();
    for island in 0..cfg.islands {
        a.cells.push(Elite {
            key: cell_key(island, &bins),
            island,
            sha: sha.clone(),
            n: 0,
            score,
            descriptor: descriptor.clone(),
            children: 0,
        });
    }
    save(env, repo, study, &before, a).await
}

fn study_cfg(env: &Env, study: &StudyRecord) -> Cfg {
    cfg(&study.spec, env.cfg.gate.remeasure_after)
}

fn island_of_slot(cfg: &Cfg, slot: Option<u8>) -> u8 {
    slot.map_or(0, |k| k.saturating_sub(1) % cfg.islands.max(1))
}

// ───────────────────────────── sampling trees ─────────────────────────────

/// `count` scored samples of `sha` in a scratch worktree (removed afterwards), rows under
/// experiment `n` with sample indices from `k0`. Elite re-measures and the extra confirm
/// samples of an archive decision come through here.
#[allow(clippy::too_many_arguments)]
async fn sample_tree(
    env: &Env,
    study: &StudyRecord,
    sha: &str,
    n: u32,
    k0: u32,
    count: u32,
    role: SampleRole,
    cancel: &CancellationToken,
) -> Result<Vec<Sample>> {
    let repo = actor::worktree_of(study)?;
    let dir = env
        .root
        .join(&study.slug)
        .join(format!("pop-{}-{n}", sha.get(..8).unwrap_or(sha)));
    if dir.exists() {
        env.backend.worktree_remove(&repo, &dir)?;
    }
    env.backend.worktree_add_detached(&repo, &dir, sha)?;
    let r = sample_in(env, study, &dir, sha, n, k0, count, role, cancel).await;
    if let Err(e) = env.backend.worktree_remove(&repo, &dir) {
        tracing::warn!(slug = %study.slug, "scratch worktree not removed: {e}");
    }
    r
}

#[allow(clippy::too_many_arguments)]
async fn sample_in(
    env: &Env,
    study: &StudyRecord,
    worktree: &Path,
    sha: &str,
    n: u32,
    k0: u32,
    count: u32,
    role: SampleRole,
    cancel: &CancellationToken,
) -> Result<Vec<Sample>> {
    let slug = study.slug.as_str();
    let evaluator = env.evaluator_dir(slug, study.segment);
    let runs = env.runs_dir(slug, n);
    std::fs::create_dir_all(&runs)?;
    if cancel.is_cancelled() {
        return Err(Error::refused("the study is stopping"));
    }
    let mut out = Vec::with_capacity(count as usize);
    for k in k0..k0 + count {
        let req = SampleRequest {
            slug,
            n,
            k,
            tree_sha: sha,
            role,
            worktree,
            evaluator: &evaluator,
            runs_dir: &runs,
        };
        out.push(actor::scored_sample(env, &study.spec, req, cancel, None).await?);
    }
    Ok(out)
}

// ───────────────────────────── parent_of ─────────────────────────────

/// `Search::Population`: the sampled parent elite for `slot` (weight = s(rank) / (1 + children));
/// island = (slot − 1) mod islands. Seeds the archive with the base when empty. A picked elite
/// whose newest sample is older than `remeasure_after` experiments gets one fresh sample first
/// (the winner's curse drains at the rate elites are used).
pub async fn parent_of(env: &Env, study: &StudyRecord, slot: Option<u8>) -> Result<String> {
    let cfg = study_cfg(env, study);
    let repo = actor::worktree_of(study)?;
    let direction = study.spec.metric.direction;
    let mut a = load(&env.store, study, cfg.islands).await?;
    if a.cells.is_empty() {
        seed(env, &repo, study, &cfg, &mut a).await?;
    }
    let before = a.clone();
    let island = island_of_slot(&cfg, slot);
    let first = env.store.next_experiment_n(&study.slug).await?;
    let n = first + u32::from(slot.unwrap_or(1).saturating_sub(1));
    let mut rng = Rng::seeded(&format!("{}/{n}", study.slug));
    let Some(i) = a.pick_parent(island, direction, &mut rng) else {
        return actor::incumbent_of(study);
    };
    let elite = a.cells[i].clone();
    save(env, &repo, study, &before, &a).await?;
    let newest = env
        .store
        .samples_of_tree(&study.slug, &elite.sha)
        .await?
        .iter()
        .filter(|s| !s.lost && s.value.is_some())
        .map(|s| s.n)
        .max()
        .unwrap_or(0);
    if n.saturating_sub(newest) > cfg.remeasure_after {
        let cancel = actor::cancel_token(&env.root, &study.slug);
        sample_tree(
            env,
            study,
            &elite.sha,
            n,
            90,
            1,
            SampleRole::Incumbent,
            &cancel,
        )
        .await?;
    }
    Ok(elite.sha)
}

// ───────────────────────────── settle and promote ─────────────────────────────

/// What the gate says about `candidate` against `reference` on the samples so far (the 140 gate
/// for scalar studies). `Suspicious` is not an acceptance: no reviewer runs at archive
/// acceptance (W4-f), so a jump the gate flags stays out until one does.
enum Gate {
    /// Decided: `(beats, why)`.
    Decided(bool, String),
    /// More samples: `incumbent_sample` = one of the reference too.
    More { incumbent_sample: bool },
}

fn gate_once(
    env: &Env,
    gcfg: &GateCfg,
    sigma: f64,
    candidate: &[Sample],
    reference: &[Sample],
) -> Gate {
    let input = GateInput {
        candidate: actor::clean_values(candidate),
        incumbent: actor::bench_values(reference),
        sigma,
        largest_accepted_delta: None,
        incumbent_age: 0,
        keep_simpler: false,
        secondaries: actor::secondary_means(candidate, reference),
    };
    match env.backend.gate_step(gcfg, &input) {
        GateStep::Decide {
            verdict: Verdict::Suspicious,
            ..
        } => Gate::Decided(false, "suspicious; not reviewed".into()),
        GateStep::Decide { verdict, z, .. } => Gate::Decided(
            verdict.is_keep_candidate(),
            format!(
                "{}{}",
                verdict.tag(),
                z.map(|z| format!(" z {z:.2}")).unwrap_or_default()
            ),
        ),
        GateStep::Wait { .. } => Gate::Decided(false, "waiting".into()),
        GateStep::Confirm {
            incumbent_sample, ..
        } => Gate::More { incumbent_sample },
    }
}

/// The child's tree as a commit in a scratch worktree: a kept slot's commit as is; a discarded
/// slot's saved patch re-applied on the incumbent, its checks run, then committed. `Ok(Err(why))`
/// when the checks fail.
async fn materialise(
    env: &Env,
    study: &StudyRecord,
    exp: &ExperimentRecord,
    cancel: &CancellationToken,
) -> Result<std::result::Result<(String, PathBuf), String>> {
    let slug = study.slug.as_str();
    let repo = actor::worktree_of(study)?;
    let dir = env.root.join(slug).join(format!("pop-child-{}", exp.n));
    if dir.exists() {
        env.backend.worktree_remove(&repo, &dir)?;
    }
    if let Some(sha) = exp
        .commit_sha
        .as_deref()
        .filter(|_| exp.state == ExperimentState::Kept)
    {
        env.backend.worktree_add_detached(&repo, &dir, sha)?;
        return Ok(Ok((sha.to_owned(), dir)));
    }
    let Some(patch) = exp.patch_path.as_deref() else {
        return Ok(Err("no saved patch".into()));
    };
    let incumbent = actor::incumbent_of(study)?;
    env.backend.worktree_add_detached(&repo, &dir, &incumbent)?;
    git::apply_patch(&dir, &env.root.join(patch))?;
    if let Some(c) = study.spec.checks.as_ref() {
        let timeout = schedule_file::duration(&c.timeout, Duration::from_secs(300));
        let evaluator = env.evaluator_dir(slug, study.segment);
        let runs = env.runs_dir(slug, exp.n);
        std::fs::create_dir_all(&runs)?;
        let req = SampleRequest {
            slug,
            n: exp.n,
            k: 1,
            tree_sha: &incumbent,
            role: SampleRole::Candidate,
            worktree: &dir,
            evaluator: &evaluator,
            runs_dir: &runs,
        };
        let (exit, _, _) = env
            .backend
            .command(&c.command, timeout, req, cancel)
            .await?;
        if exit != Some(0) {
            env.backend.worktree_remove(&repo, &dir).ok();
            return Ok(Err(format!("checks failed (exit {exit:?})")));
        }
    }
    let meta = crate::wave::keep_meta(study, exp);
    let sha = env
        .backend
        .keep(&dir, &study.spec, &meta, env.cfg.git.author.as_deref())?;
    Ok(Ok((sha, dir)))
}

/// After a decision: archive acceptance of one slot (159 §9 "Acceptance"). An empty cell takes
/// the child when its checks pass and no guard fails against its parent; an occupied cell when
/// the row's rule says it beats the elite. Updates the experiment (state, line, `parent_ref`,
/// `commit_sha` of a materialised stepping stone) and the model; the caller saves the model.
pub async fn settle(
    env: &Env,
    study: &StudyRecord,
    before: &Archive,
    a: &mut Archive,
    exp: &mut ExperimentRecord,
    cancel: &CancellationToken,
) -> Result<Outcome> {
    let cfg = study_cfg(env, study);
    let gcfg = actor::gate_cfg(&env.cfg, &study.spec)?;
    let slug = study.slug.as_str();
    let sigma = study.sigma.unwrap_or(0.0);
    // The parent cell as it was when the wave was cut (`before`: an elite replaced earlier in
    // this settle still names it). The same tree may hold a cell on several islands (the seeded
    // base): the slot's island first.
    let island = island_of_slot(&cfg, exp.slot);
    exp.parent_ref = before
        .cells
        .iter()
        .filter(|c| c.sha == exp.parent_sha)
        .min_by_key(|c| c.island != island)
        .map(|c| refs::name(slug, RefKind::Cell, &c.key));
    let parent_samples = env.store.samples_of_tree(slug, &exp.parent_sha).await?;
    let outcome = settle_inner(
        env,
        study,
        &cfg,
        &gcfg,
        sigma,
        a,
        exp,
        &parent_samples,
        cancel,
    )
    .await?;
    let wave = exp.wave.unwrap_or(0);
    match &outcome {
        Outcome::Entered | Outcome::Replaced { .. } => {
            exp.line = format!("{}; {}", archive_line(&outcome, exp), exp.line);
        }
        Outcome::Rejected(why) if exp.state == ExperimentState::Kept => {
            // Like wave promotion's `lose`: the commit's diff becomes the saved patch.
            if let (Some(sha), Ok(wt)) = (exp.commit_sha.as_deref(), actor::worktree_of(study)) {
                let patch = env.runs_dir(slug, exp.n).join("candidate.patch");
                if let Ok(diff) = env.backend.diff_text(&wt, &exp.parent_sha, sha, 4 << 20) {
                    std::fs::create_dir_all(patch.parent().unwrap())?;
                    std::fs::write(&patch, diff)?;
                    exp.patch_path = Some(format!("{slug}/runs/{}/candidate.patch", exp.n));
                }
            }
            exp.state = ExperimentState::Discarded;
            exp.line = format!(
                "lost wave {wave}: archive rejected it ({why}); {}",
                exp.line
            );
        }
        Outcome::Rejected(why) => {
            exp.line = format!("archive rejected it ({why}); {}", exp.line);
        }
    }
    env.store.update_experiment(exp).await?;
    env.store
        .append_event(
            slug,
            EVENT_ARCHIVE,
            &serde_json::json!({
                "wave": wave, "n": exp.n, "outcome": outcome.word(),
                "key": a.cells.iter().find(|c| c.n == exp.n).map(|c| c.key.clone()),
                "why": match &outcome { Outcome::Rejected(w) => Some(w.clone()), _ => None },
            }),
        )
        .await?;
    Ok(outcome)
}

fn archive_line(outcome: &Outcome, exp: &ExperimentRecord) -> String {
    let key = exp
        .parent_ref
        .as_deref()
        .and_then(|r| r.rsplit_once("/cell/"))
        .map(|(_, k)| k.replacen('/', ":", 1))
        .unwrap_or_default();
    match outcome {
        Outcome::Entered => format!("entered an empty cell (parent {key})"),
        Outcome::Replaced { n } => format!("replaced #{n} in its cell (parent {key})"),
        Outcome::Rejected(w) => w.clone(),
    }
}

#[allow(clippy::too_many_arguments)]
async fn settle_inner(
    env: &Env,
    study: &StudyRecord,
    cfg: &Cfg,
    gcfg: &GateCfg,
    sigma: f64,
    a: &mut Archive,
    exp: &mut ExperimentRecord,
    parent_samples: &[Sample],
    cancel: &CancellationToken,
) -> Result<Outcome> {
    let slug = study.slug.as_str();
    let qualifies = match (exp.state, exp.verdict.as_ref()) {
        (ExperimentState::Kept, _) => exp.commit_sha.is_some(),
        (ExperimentState::Discarded, Some(Verdict::NoGain | Verdict::Worse)) => {
            exp.patch_path.is_some()
        }
        _ => false,
    };
    if !qualifies {
        return Ok(Outcome::Rejected(format!(
            "{} ({})",
            exp.state.tag(),
            exp.verdict.as_ref().map_or("no verdict", Verdict::tag)
        )));
    }
    let mut samples: Vec<Sample> = env
        .store
        .list_samples(slug, Some(exp.n))
        .await?
        .into_iter()
        .filter(|s| s.role == Some(SampleRole::Candidate) && !s.lost && s.value.is_some())
        .collect();
    if actor::clean_values(&samples).is_empty() {
        return Ok(Outcome::Rejected("no clean sample".into()));
    }
    let descriptor = descriptor_values(
        cfg,
        &study.spec.metric,
        &samples,
        &base_cases(&env.store, study, cfg).await?,
    );
    // A descriptor no sample reports is a refusal, not bin 0: the bench forgot its METRIC line.
    if let Some(d) = cfg
        .descriptors
        .iter()
        .find(|d| !descriptor.contains_key(&d.label()))
    {
        return Ok(Outcome::Rejected(format!(
            "descriptor `{}` not reported",
            d.label()
        )));
    }
    let island = island_of_slot(cfg, exp.slot);
    let key = cell_key(island, &bins_of(cfg, &descriptor));
    let holder = a.get(&key).cloned();

    // Decide before any git work: an occupied cell by the gate against its elite (extra
    // candidate samples when it asks, in the materialised tree), an empty cell by the guards
    // against the parent.
    let mut tree: Option<(String, PathBuf)> = None;
    let repo = actor::worktree_of(study)?;
    match &holder {
        Some(h) => {
            let mut reference = env.store.samples_of_tree(slug, &h.sha).await?;
            let mut k = 50u32;
            let mut k_ref = 80u32;
            let mut rounds = 0u32;
            let (ok, why) = loop {
                let incumbent_sample = match gate_once(env, gcfg, sigma, &samples, &reference) {
                    Gate::Decided(ok, why) => break (ok, why),
                    Gate::More { incumbent_sample } => incumbent_sample,
                };
                if rounds > u32::from(gcfg.confirm_runs) {
                    break (false, "gate undecided".to_owned());
                }
                rounds += 1;
                // R-25 for elites: a holder judged on one lucky draw gets a fresh sample when the
                // gate asks for the reference, not only when it is picked as a parent.
                if incumbent_sample {
                    reference.extend(
                        sample_tree(
                            env,
                            study,
                            &h.sha,
                            exp.n,
                            k_ref,
                            1,
                            SampleRole::Incumbent,
                            cancel,
                        )
                        .await?,
                    );
                    k_ref += 1;
                    continue;
                }
                if tree.is_none() {
                    match materialise(env, study, exp, cancel).await? {
                        Ok(t) => tree = Some(t),
                        Err(why) => break (false, why),
                    }
                }
                let (sha, dir) = tree.as_ref().expect("materialised");
                let more = u32::from(gcfg.confirm_runs).max(1);
                samples.extend(
                    sample_in(
                        env,
                        study,
                        dir,
                        sha,
                        exp.n,
                        k,
                        more,
                        SampleRole::Candidate,
                        cancel,
                    )
                    .await?,
                );
                k += more;
            };
            if !ok {
                if let Some((_, dir)) = tree.take() {
                    env.backend.worktree_remove(&repo, &dir).ok();
                }
                return Ok(Outcome::Rejected(format!(
                    "cell {key} held by #{} ({why})",
                    h.n
                )));
            }
        }
        None => {
            let input = GateInput {
                candidate: actor::clean_values(&samples),
                incumbent: actor::bench_values(parent_samples),
                sigma,
                largest_accepted_delta: None,
                incumbent_age: 0,
                keep_simpler: false,
                secondaries: actor::secondary_means(&samples, parent_samples),
            };
            let failed = crate::gate::guard_failures(gcfg, &input);
            if !failed.is_empty() {
                return Ok(Outcome::Rejected(format!("guard: {}", failed.join(", "))));
            }
        }
    }
    let (sha, dir) = match tree {
        Some(t) => t,
        None => match materialise(env, study, exp, cancel).await? {
            Ok(t) => t,
            Err(why) => return Ok(Outcome::Rejected(why)),
        },
    };
    env.backend.worktree_remove(&repo, &dir).ok();
    if exp.state != ExperimentState::Kept {
        // A stepping stone: the discarded slot's tree is now a commit and its samples follow it,
        // but the gate's word stands — `state`/`verdict` stay as measured, so no 140 reader
        // (plateau, explore, the brief's keeps) counts it as a keep. The archive event and the
        // row's line say it entered.
        actor::retag_samples(&env.store, slug, exp.n, &sha).await?;
        exp.commit_sha = Some(sha.clone());
    }
    let child = Elite {
        key,
        island,
        sha,
        n: exp.n,
        score: mean(&actor::clean_values(&samples)),
        descriptor,
        children: 0,
    };
    Ok(a.accept(child, |_| true))
}

/// The replacement of wave promotion for `search: population` (159 §9 "Seams"): every closed
/// slot of `wave` goes through [`settle`], open slots are abandoned first, then migration every
/// `migrate_every` waves, then the champion (best headline mean over every island) becomes the
/// incumbent and the study branch is reset to it. Writes the `wave_promoted` event wave.rs's
/// `settle` keys on, removes the candidate worktrees, refreshes σ. Idempotent per wave through
/// that event (the caller checks it under wave.rs's settle lock).
pub async fn promote(env: &Env, study: &StudyRecord, wave: u32) -> Result<Promotion> {
    let slug = study.slug.as_str();
    let mut study = env
        .store
        .get_study(slug)
        .await?
        .ok_or_else(|| Error::refused(format!("no study `{slug}`")))?;
    let cfg = study_cfg(env, &study);
    let direction = study.spec.metric.direction;
    let repo = actor::worktree_of(&study)?;
    let mut exps: Vec<ExperimentRecord> = env
        .store
        .list_experiments(slug, None, u32::MAX)
        .await?
        .into_iter()
        .filter(|e| e.wave == Some(wave))
        .collect();
    if exps.is_empty() {
        return Err(Error::refused(format!("study `{slug}` has no wave {wave}")));
    }
    exps.sort_by_key(|e| e.n);
    let mut a = load(&env.store, &study, cfg.islands).await?;
    if a.cells.is_empty() {
        seed(env, &repo, &study, &cfg, &mut a).await?;
    }
    let before = a.clone();
    let cancel = actor::cancel_token(&env.root, slug);
    let mut promotion = Promotion::default();
    for mut exp in exps {
        // 159 W-q: a `Waiting` slot (a later window decides it) is not abandoned by the promotion.
        if exp.state.is_dangling() {
            abandon(env, exp).await?;
            continue;
        }
        // A Stop-now during the settle: the remaining slots keep the gate's word and are not
        // archived; the wave still closes below so wave.rs's settle never loops on it.
        if cancel.is_cancelled() {
            break;
        }
        let snapshot = a.clone();
        if !settle(env, &study, &before, &mut a, &mut exp, &cancel)
            .await?
            .accepted()
        {
            continue;
        }
        // 159 W4 H5: the reviewer at acceptance (§9.1). A flag under `revert` undoes the
        // acceptance: the archive is as it was before this slot, so `save` writes no cell.
        if exp.state == ExperimentState::Kept && review(env, &study, &mut exp, &repo).await? {
            a = snapshot;
            continue;
        }
        promotion.promoted.push(exp.n);
    }
    if cfg.migrate_every > 0 && wave > 0 && wave.is_multiple_of(cfg.migrate_every) {
        let gcfg = actor::gate_cfg(&env.cfg, &study.spec)?;
        let sigma = study.sigma.unwrap_or(0.0);
        for (_, offered) in a.offers(direction) {
            let ok = match a.get(&offered.key) {
                None => true,
                Some(h) if h.sha == offered.sha => false,
                Some(h) => {
                    // On the samples both trees already have: an undecided gate is no migration.
                    let cand = env.store.samples_of_tree(slug, &offered.sha).await?;
                    let reference = env.store.samples_of_tree(slug, &h.sha).await?;
                    matches!(
                        gate_once(env, &gcfg, sigma, &cand, &reference),
                        Gate::Decided(true, _)
                    )
                }
            };
            if ok {
                let from = offered.n;
                let out = a.accept(offered.clone(), |_| true);
                env.store
                    .append_event(
                        slug,
                        EVENT_ARCHIVE,
                        &serde_json::json!({ "wave": wave, "n": from, "outcome": "migrated", "key": offered.key, "why": out.word() }),
                    )
                    .await?;
            }
        }
    }
    // The champion: the study branch and the incumbent point at it. The incumbent mean is the
    // champion's mean over every sample it has (re-measures included), not its archive score.
    let mut patch = StudyPatch::default();
    if let Some(champ) = a.champion(direction).cloned()
        && study.incumbent_sha.as_deref() != Some(champ.sha.as_str())
    {
        git::reset_clean(&repo, &champ.sha)?;
        let (_, values) = tree_values(&env.store, slug, &champ.sha).await?;
        let m = if values.is_empty() {
            champ.score
        } else {
            mean(&values)
        };
        patch.incumbent_sha = Some(champ.sha.clone());
        patch.incumbent_mean = Some(Some(m));
        study.incumbent_sha = Some(champ.sha);
        study.incumbent_mean = Some(m);
    }
    save(env, &repo, &study, &before, &a).await?;
    for k in 1..=study.spec.parallel.max(2) {
        let dir = crate::wave::cand_dir(&env.root, slug, k);
        if dir.exists()
            && let Err(e) = env.backend.worktree_remove(&repo, &dir)
        {
            tracing::warn!(%slug, k, "candidate worktree not removed: {e}");
        }
    }
    // Only the columns this promotion owns: the settle took minutes and other sessions wrote
    // the playbook, a pause or a stop meanwhile (R-W4-2).
    patch.sigma = Some(Some(actor::segment_sigma(env, &study).await?));
    env.store.patch_study(slug, &patch).await?;
    promotion.incumbent = study.incumbent_sha.clone();
    env.store
        .append_event(
            slug,
            crate::wave::EVENT_PROMOTED,
            &serde_json::json!({ "wave": wave, "promoted": promotion.promoted, "incumbent": promotion.incumbent }),
        )
        .await?;
    actor::clear_phase(&env.root, slug);
    Ok(promotion)
}

/// §9.1 on an accepted keep — wave.rs's `review` without the branch reset: a slot's commit is
/// not on the study branch, so the flag only records the rejected ref and the saved patch.
/// `Ok(true)` when a flag reverted the acceptance (`on_suspicious: revert`).
async fn review(
    env: &Env,
    study: &StudyRecord,
    exp: &mut ExperimentRecord,
    repo: &Path,
) -> Result<bool> {
    let Some(model) = env.cfg.reviewer(&study.spec.models, &env.agent) else {
        return Ok(false);
    };
    let slug = study.slug.as_str();
    let n = exp.n;
    actor::progress(env, slug, n, &exp.started_at, Phase::Review, 0, 1).await;
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
    let (rv, reverted) = match rv {
        ReviewVerdict::Flagged { reason }
            if env.cfg.review.on_suspicious == crate::config::OnSuspicious::Revert =>
        {
            let sha = exp.commit_sha.clone().unwrap_or_default();
            let patch = env.runs_dir(slug, n).join("candidate.patch");
            let diff = env
                .backend
                .diff_text(repo, &exp.parent_sha, &sha, 4 << 20)?;
            std::fs::create_dir_all(patch.parent().unwrap())?;
            std::fs::write(&patch, diff)?;
            exp.patch_path = Some(format!("{slug}/runs/{n}/candidate.patch"));
            git::plumbing(repo, &["update-ref", &git::rejected_ref(slug, n), &sha])?;
            exp.state = ExperimentState::RejectedByReview;
            exp.line = format!("rejected by review ({reason}); {}", exp.line);
            env.store
                .append_event(
                    slug,
                    "review",
                    &serde_json::json!({ "n": n, "reason": reason, "reverted": true }),
                )
                .await?;
            env.store
                .append_event(
                    slug,
                    EVENT_ARCHIVE,
                    &serde_json::json!({ "wave": exp.wave, "n": n, "outcome": "rejected_by_review", "why": reason }),
                )
                .await?;
            (ReviewVerdict::Reverted { reason }, true)
        }
        other => (other, false),
    };
    exp.review = Some(rv);
    env.store.update_experiment(exp).await?;
    Ok(reverted)
}

/// Close a slot nobody logged (wave.rs `abandon`, kept private there).
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

// ───────────────────────────── views ─────────────────────────────

/// `KindView::Archive` for the Study view: every live elite, the champion starred.
pub async fn view(store: &Store, study: &StudyRecord) -> Result<KindView> {
    let cfg = cfg(&study.spec, 0);
    let rows = store.archive_list(&study.slug, Some(KIND)).await?;
    let champion = study.incumbent_sha.as_deref();
    let mut labels = cfg.descriptors.iter().map(Descriptor::label);
    Ok(KindView::Archive {
        axes: [
            labels.next().unwrap_or_default(),
            labels.next().unwrap_or_default(),
        ],
        cells: rows
            .into_iter()
            .map(|r| CellRow {
                champion: Some(r.sha.as_str()) == champion,
                key: r.key,
                island: r.island,
                sha: r.sha,
                n: r.n,
                scores: r.scores,
                descriptor: r.descriptor,
                children: r.children,
            })
            .collect(),
        islands: cfg.islands,
    })
}

/// The brief's "Archive" section: the elites per island, the champion (unconfirmed until
/// finalize re-measures it), the descriptor grid, and how acceptance differs from a keep.
pub async fn brief_section(store: &Store, study: &StudyRecord) -> Result<String> {
    let cfg = cfg(&study.spec, 0);
    let rows = store.archive_list(&study.slug, Some(KIND)).await?;
    let metric = &study.spec.metric;
    let mut out = String::from("\n<archive>\nThis study is a population search, not a ratchet. ");
    out.push_str(&format!(
        "{} islands; a cell is an island plus {}. Each cell keeps one elite; your slot is cut from a sampled elite, not from the champion. ",
        cfg.islands,
        grid_words(&cfg)
    ));
    out.push_str(
        "A change that lands in an empty cell is kept as a stepping stone even when it is worse than the champion \
         (checks must pass, guards must hold); an occupied cell is taken only by clearing the gate against its elite. \
         Reaching behaviour the grid has not covered is as valuable as a straight improvement.\n",
    );
    if rows.is_empty() {
        out.push_str("No elite yet: the base seeds every island.\n");
    } else {
        let champion = study.incumbent_sha.as_deref();
        out.push_str(&format!(
            "Elites (`{}` {}):\n",
            metric.name,
            dir_word(metric.direction)
        ));
        for r in &rows {
            let score = r.scores.get(&metric.name).copied().unwrap_or(f64::NAN);
            let desc: Vec<String> = r
                .descriptor
                .iter()
                .map(|(k, v)| format!("{k}={}", trim(*v)))
                .collect();
            out.push_str(&format!(
                "- {} · #{} · {} {}{} · children {}{}\n",
                r.key,
                r.n,
                trim(score),
                metric.unit,
                if desc.is_empty() {
                    String::new()
                } else {
                    format!(" · {}", desc.join(" "))
                },
                r.children,
                if Some(r.sha.as_str()) == champion {
                    " · champion (unconfirmed until finalize re-measures it)"
                } else {
                    ""
                }
            ));
        }
    }
    out.push_str("</archive>\n");
    Ok(out)
}

fn grid_words(cfg: &Cfg) -> String {
    if cfg.descriptors.is_empty() {
        return "no descriptor (one cell per island)".to_owned();
    }
    cfg.descriptors
        .iter()
        .map(|d| {
            let edges: Vec<String> = d.bins.iter().map(|e| trim(*e)).collect();
            format!("`{}` binned at [{}]", d.label(), edges.join(", "))
        })
        .collect::<Vec<_>>()
        .join(" × ")
}

fn dir_word(d: Direction) -> &'static str {
    match d {
        Direction::Lower => "lower is better",
        Direction::Higher => "higher is better",
    }
}

fn trim(v: f64) -> String {
    if !v.is_finite() {
        return "?".into();
    }
    let s = format!("{v:.2}");
    s.trim_end_matches('0').trim_end_matches('.').to_owned()
}

/// Finalize's headline: `None` — the scalar confirmation re-measures the champion (the study
/// branch) against the base with fresh interleaved samples, which is exactly the 159 §9 claim.
pub async fn headline(_store: &Store, _study: &StudyRecord) -> Result<Option<Headline>> {
    Ok(None)
}

/// Draft card line 1: "Population search: 3 islands, cells over `peak_kb` × `time_ms`; the
/// champion is re-measured before it counts."
pub fn card_line(spec: &StudySpec) -> String {
    let cfg = cfg(spec, 0);
    let over = if cfg.descriptors.is_empty() {
        "one cell per island".to_owned()
    } else {
        format!(
            "cells over {}",
            cfg.descriptors
                .iter()
                .map(|d| format!("`{}`", d.label()))
                .collect::<Vec<_>>()
                .join(" × ")
        )
    };
    format!(
        "Population search: {} islands, {over}; the champion is re-measured before it counts.",
        cfg.islands
    )
}

/// `autoresearch.archive` rows of kind `cell`.
pub async fn archive(store: &Store, slug: &str) -> Result<Vec<ArchiveRow>> {
    store.archive_list(slug, Some(KIND)).await
}

// ───────────────────────────── setup ─────────────────────────────

/// Setup rows this kind adds to the draft panel: one `population` row.
pub const SETUP_ROWS: &[SetupRow] = &[SetupRow {
    row: "population",
    label: "Population",
    leaves: &[
        "population.islands",
        "population.descriptors",
        "population.migrate_every",
        "population.remeasure_after",
    ],
}];

/// One-line value of the `population` row: `3 islands · cells over `peak_kb` [100, 200] ×
/// `time_ms` [10, 20] · migrate every 4 waves`. Empty when the spec has no block.
pub fn setup_value(spec: &StudySpec, row: &str) -> String {
    if row != "population" || spec.population.is_none() {
        return String::new();
    }
    let cfg = cfg(spec, 0);
    let mut parts = vec![format!("{} islands", cfg.islands)];
    parts.push(if cfg.descriptors.is_empty() {
        "one cell per island".to_owned()
    } else {
        format!(
            "cells over {}",
            cfg.descriptors
                .iter()
                .map(|d| {
                    let edges: Vec<String> = d.bins.iter().map(|e| trim(*e)).collect();
                    format!(
                        "{}`{}` [{}]",
                        match d.source {
                            DescriptorSource::Secondary => "",
                            DescriptorSource::Asi => "asi:",
                            DescriptorSource::CaseBest => "",
                        },
                        d.label(),
                        edges.join(", ")
                    )
                })
                .collect::<Vec<_>>()
                .join(" × ")
        )
    });
    parts.push(format!("migrate every {} waves", cfg.migrate_every));
    if cfg.remeasure_after > 0 {
        parts.push(format!("re-measure after {}", cfg.remeasure_after));
    }
    parts.join(" · ")
}

/// Set one `population.*` leaf from the panel's text. Descriptors: `name: e1, e2; asi:name: e1`
/// or `case: e1, e2` — one or two, `;`-separated, ascending edges.
pub fn setup_set(spec: &mut StudySpec, leaf: &str, v: &str) -> std::result::Result<(), String> {
    let p = spec.population.get_or_insert_with(Default::default);
    let v = v.trim();
    match leaf {
        "population" | "population.islands" => {
            let n: u8 = v
                .parse()
                .map_err(|_| format!("population.islands: `{v}` is not a number"))?;
            if !(2..=4).contains(&n) {
                return Err("population.islands must be 2–4".into());
            }
            p.islands = n;
        }
        "population.migrate_every" => {
            p.migrate_every = v
                .parse()
                .map_err(|_| format!("population.migrate_every: `{v}` is not a number"))?;
        }
        "population.remeasure_after" => {
            p.remeasure_after = v
                .parse()
                .map_err(|_| format!("population.remeasure_after: `{v}` is not a number"))?;
        }
        "population.descriptors" => p.descriptors = parse_descriptors(v)?,
        other => return Err(format!("unknown field `{other}`")),
    }
    Ok(())
}

/// `peak_kb: 100, 200, 400; asi:depth: 2, 4` → two descriptors.
pub fn parse_descriptors(text: &str) -> std::result::Result<Vec<Descriptor>, String> {
    let mut out = Vec::new();
    for part in text.split(';').map(str::trim).filter(|s| !s.is_empty()) {
        let (head, edges) = part
            .rsplit_once(':')
            .ok_or_else(|| format!("descriptor `{part}`: expected `name: edge, edge, …`"))?;
        let head = head.trim();
        let (source, name) = match head.split_once(':') {
            Some((s, n)) if s.trim().eq_ignore_ascii_case("asi") => {
                (DescriptorSource::Asi, n.trim().to_owned())
            }
            Some((s, n)) if s.trim().eq_ignore_ascii_case("secondary") => {
                (DescriptorSource::Secondary, n.trim().to_owned())
            }
            _ if head.eq_ignore_ascii_case("case") || head.eq_ignore_ascii_case("case_best") => {
                (DescriptorSource::CaseBest, String::new())
            }
            _ => (DescriptorSource::Secondary, head.to_owned()),
        };
        if source != DescriptorSource::CaseBest && name.is_empty() {
            return Err(format!("descriptor `{part}`: empty name"));
        }
        let mut bins = Vec::new();
        for e in edges.split(',').map(str::trim).filter(|s| !s.is_empty()) {
            let x: f64 = e
                .parse()
                .map_err(|_| format!("descriptor `{name}`: edge `{e}` is not a number"))?;
            if !x.is_finite() {
                return Err(format!("descriptor `{name}`: edge `{e}` is not finite"));
            }
            bins.push(x);
        }
        if bins.is_empty() {
            return Err(format!("descriptor `{part}`: at least one edge"));
        }
        if bins.windows(2).any(|w| w[0] >= w[1]) {
            return Err(format!("descriptor `{name}`: edges must ascend"));
        }
        out.push(Descriptor { source, name, bins });
    }
    if out.len() > 2 {
        return Err("at most two descriptors".into());
    }
    Ok(out)
}

// ───────────────────────────── not this kind's seams ─────────────────────────────

/// `Scoring` is `bench` for a population study; the scoring dispatch never names this module.
pub async fn sample(
    _env: &Env,
    _study: &StudyRecord,
    _req: SampleRequest<'_>,
    _cancel: &CancellationToken,
) -> Result<Sample> {
    Err(Error::NotBuilt(OWNER))
}

/// The rule is the row's (`z_gate` for scalar studies); the rule dispatch never names this module.
pub async fn step(
    _env: &Env,
    _study: &StudyRecord,
    _cfg: &GateCfg,
    _input: &GateInput,
    _candidate: &[Sample],
    _reference: &[Sample],
) -> Result<GateStep> {
    Err(Error::NotBuilt(OWNER))
}

#[cfg(test)]
mod tests;
