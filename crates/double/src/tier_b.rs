//! The Tier B adapter between W3's trainer ([`crate::train_b`]) and W2's
//! scheduler ([`crate::sched`]) and mesh protocol ([`crate::mesh`]) (126 I).
//!
//! - [`to_train_b`]: a mesh bundle (rows + every label) → the trainer's bundle
//!   (one net label per row, Tier A's `net_target` rule).
//! - [`LocalTierB`] / [`NodeTierB`]: the two Tier B [`Trainer`]s; both end in
//!   [`Ctx::finish`], W2's gate over the candidate and the version directory.
//! - [`NodeRunner`]: the node side of `theuth/double/1`.
//! - [`factory`]: `(tier, venue) → trainer`, Tier A delegated to
//!   [`sched::tier_a_factory`].

use std::path::{Path, PathBuf};
use std::sync::Arc;

use chrono::{DateTime, Duration as ChronoDuration, Utc};
use decision::candle::lora::{Base, TRAIN_BUILD};
use parking_lot::Mutex;
use replica::PeerId;
use runtime::Runtime;
use tokio::sync::mpsc;

use crate::config::DoubleConfig;
use crate::events::DoubleTrainingState;
use crate::mesh;
use crate::sched::{self, Finished, Trainer};
use crate::store::{Store, VersionRow};
use crate::train_a;
use crate::train_b::{
    self, ADAPTER_FILE, CandidateVersion, Hooks, Never, OwnerActivity, PlatformIdle, TierB,
    TrainConfig, TrainOutcome,
};
use crate::types::{LabelSource, RunState, Tier};
use crate::venue::Resolved;
use crate::versions;

/// `<data_dir>/double/runs`.
fn runs_dir(data_dir: &Path) -> PathBuf {
    data_dir.join(crate::store::DIR).join("runs")
}

/// The `default` decision checkpoint directory (`decision.checkpoints.default`,
/// else `decision.model_dir`, else `<data_dir>/models/decision`).
pub fn checkpoint_dir(decision_cfg: &decision::DecisionConfig, data_dir: &Path) -> PathBuf {
    decision_cfg.model_dir_in(data_dir)
}

/// sha256 hex of `<checkpoint_dir>/model.safetensors` (the `.sha256` sidecar
/// when current).
pub fn base_hash(checkpoint_dir: &Path) -> anyhow::Result<String> {
    Ok(platform::sha256::cached(
        &checkpoint_dir.join("model.safetensors"),
    )?)
}

/// One mesh row → one trainer row: the net label over `options ++ [none]`
/// (Tier A's rule), its argmax as the choice; `None` when nothing positive
/// remains or `asked_at` does not parse.
fn to_row(r: &mesh::BundleRow) -> Option<train_b::BundleRow> {
    let asked_at = DateTime::parse_from_rfc3339(&r.asked_at)
        .ok()?
        .with_timezone(&Utc);
    let mut options = r.options.clone();
    options.push(train_a::NONE.to_owned());
    let labels: Vec<train_a::Label> = r
        .labels
        .iter()
        .map(|(s, c, w)| {
            (
                LabelSource::parse(s).unwrap_or(LabelSource::Excluded),
                c.clone(),
                *w,
            )
        })
        .collect();
    let net = train_a::net_target(&options, &labels)?;
    let (best, weight) =
        net.iter()
            .copied()
            .enumerate()
            .fold(
                (0, f32::NEG_INFINITY),
                |m, (i, v)| {
                    if v > m.1 { (i, v) } else { m }
                },
            );
    let source = if labels.iter().all(|(s, _, _)| *s == LabelSource::Override) {
        LabelSource::Override
    } else {
        LabelSource::Direct
    };
    Some(train_b::BundleRow {
        question_id: r.id.clone(),
        class: r.class.clone(),
        card_text: r.card_text.clone(),
        options: r.options.clone(),
        choice: options[best].clone(),
        source,
        weight,
        asked_at,
        golden: r.golden,
    })
}

/// A mesh bundle as the trainer reads it (golden rows kept, flagged; the
/// trainer skips them and the gate re-scores them).
pub fn to_train_b(b: &mesh::Bundle) -> train_b::Bundle {
    train_b::Bundle {
        manifest: train_b::BundleManifest {
            base_hash: b.manifest.base_hash.clone(),
            run_id: b.manifest.run_id.clone(),
        },
        rows: b.rows.iter().filter_map(to_row).collect(),
    }
}

/// Open the frozen base on the configured device.
fn open_base(decision_cfg: &decision::DecisionConfig, ckpt: &Path) -> anyhow::Result<Base> {
    let gpu = decision_cfg.device.gpu();
    let dev = gpu.open().map_err(anyhow::Error::msg)?;
    Base::open(ckpt, dev, gpu.dtype())
}

/// `DoubleTrainingState` without awaiting (the trainer's thread is blocking).
fn publish(h: &tokio::runtime::Handle, rt: &Runtime, ev: DoubleTrainingState) {
    let rt = rt.clone();
    h.spawn(async move { rt.publish(ev).await });
}

fn progress_message(p: &train_b::Progress) -> String {
    format!(
        "step {}/{} epoch {}/{} loss {:.4}",
        p.step, p.total_steps, p.epoch, p.epochs, p.loss
    )
}

/// `train_b::Metrics` → the gate's shape (hold-out; no Brier).
fn proxy_metrics(m: &train_b::Metrics) -> train_a::Metrics {
    train_a::Metrics {
        n: m.n_holdout as u32,
        accuracy: f64::from(m.accuracy),
        log_loss: f64::from(m.holdout_log_loss),
        brier: 0.0,
        ece: f64::from(m.ece),
    }
}

/// What every Tier B trainer on the laptop shares.
#[derive(Clone)]
struct Ctx {
    rt: Runtime,
    store: Store,
    models: PathBuf,
    cfg: DoubleConfig,
    data_dir: PathBuf,
    decision_cfg: decision::DecisionConfig,
    base_hash: String,
}

impl Ctx {
    fn ckpt(&self) -> PathBuf {
        checkpoint_dir(&self.decision_cfg, &self.data_dir)
    }

    async fn bundle(&self, run_id: &str) -> anyhow::Result<train_b::Bundle> {
        let b = mesh::build_bundle(
            &self.store,
            run_id,
            Tier::B,
            &self.base_hash,
            &self.cfg.training.tiers.b,
        )
        .await?;
        Ok(to_train_b(&b))
    }

    /// W2's gate over the candidate (§7.3 (a)–(c)), then the version directory
    /// — a Tier B version carries the incumbent's Tier A artefacts so
    /// `versions::load_active` keeps working. `golden` are the bundle's golden
    /// rows, re-scored with the adapter.
    async fn finish(
        &self,
        c: CandidateVersion,
        golden: Vec<train_b::BundleRow>,
    ) -> anyhow::Result<Option<Finished>> {
        let now = Utc::now();
        let active = versions::load_active(&self.store, &self.models).await?;
        let examples = train_a::load_examples(&self.store).await?;
        let since = now - ChronoDuration::days(7);
        let golden_ex: Vec<&train_a::Example> = examples.iter().filter(|e| e.golden).collect();
        let recent: Vec<&train_a::Example> = examples
            .iter()
            .filter(|e| !e.golden && e.asked_at >= since)
            .collect();
        let art = active.artifacts.as_ref();
        let i_proxy = train_a::metrics(&train_a::score(art, &recent, now));
        let i_golden = train_a::metrics(&train_a::score(art, &golden_ex, now));
        let c_proxy = proxy_metrics(&c.manifest.metrics);
        // ponytail: ECE over the hold-out stands in for the design's golden ∪ 7-day
        // ECE, which would need a second scoring pass over the adapter.
        let c_all = c_proxy;

        let (decision_cfg, ckpt, cand) = (self.decision_cfg.clone(), self.ckpt(), c.clone());
        let rescored = tokio::task::spawn_blocking(move || {
            let base = open_base(&decision_cfg, &ckpt)?;
            train_b::rescore_golden(&base, &cand, &golden)
        })
        .await?;
        let (c_golden, verdict) = match rescored {
            Ok(g) => {
                let c_golden = train_a::Metrics {
                    n: g.n as u32,
                    accuracy: f64::from(g.accuracy),
                    log_loss: f64::from(g.log_loss),
                    brier: 0.0,
                    ece: 0.0,
                };
                (
                    c_golden,
                    train_a::gate(c_proxy, i_proxy, c_golden, i_golden, c_all),
                )
            }
            Err(e) => (
                train_a::Metrics::default(),
                Err(format!("rejected: golden re-score failed: {e:#}")),
            ),
        };
        let mut metrics =
            train_a::metrics_json(c_proxy, c_golden, c_all, i_proxy, i_golden, &verdict);
        if let serde_json::Value::Object(m) = &mut metrics {
            m.insert("tier_b".into(), serde_json::to_value(&c.manifest)?);
        }

        let m = &c.manifest;
        let data_range = format!(
            "{}..{}",
            m.data_range[0].to_rfc3339(),
            m.data_range[1].to_rfc3339()
        );
        let paths = versions::write(
            &self.models,
            &versions::Manifest {
                version: c.id.clone(),
                tier: Tier::B,
                base_hash: m.base_hash.clone(),
                data_range: data_range.clone(),
                venue: m.venue.clone(),
                metrics: metrics.clone(),
                trained_at: m.trained_at.to_rfc3339(),
                duration_s: m.duration_s as u32,
                seed: Some(m.seed),
            },
            active
                .artifacts
                .as_ref()
                .unwrap_or(&train_a::Artifacts::empty()),
        )?;
        let adapter = paths.dir.join(ADAPTER_FILE);
        std::fs::copy(c.dir.join(ADAPTER_FILE), &adapter)?;
        std::fs::write(paths.dir.join("tier_b.json"), serde_json::to_vec_pretty(m)?)?;
        let _ = std::fs::remove_dir_all(&c.dir);
        Ok(Some(Finished {
            version: VersionRow {
                id: c.id.clone(),
                tier: Tier::B,
                base_hash: m.base_hash.clone(),
                adapter_path: Some(adapter.display().to_string()),
                head_path: Some(paths.heads.display().to_string()),
                knn_path: Some(paths.knn.display().to_string()),
                metrics,
                data_range,
                trained_at: m.trained_at,
                venue: m.venue.clone(),
                duration_s: m.duration_s as u32,
                active: false,
            },
            verdict,
        }))
    }
}

/// Tier B on this machine: `TierB::train` under `OwnerActivity`, resuming
/// from `<data_dir>/double/runs/tier-b/checkpoint` across steps.
pub struct LocalTierB {
    ctx: Ctx,
    run_id: String,
    venue: String,
}

#[async_trait::async_trait]
impl Trainer for LocalTierB {
    fn steps(&self) -> u32 {
        // ponytail: a resume budget, not epochs — every call to `TierB::train`
        // either finishes or pauses on owner activity / the nightly cap; 16
        // resumes before the scheduler gives up on the run.
        16
    }

    fn bind(&mut self, run_id: &str, venue: &str) {
        self.run_id = run_id.to_owned();
        self.venue = venue.to_owned();
    }

    async fn step(&mut self, _: u32) -> anyhow::Result<Option<Finished>> {
        let bundle = self.ctx.bundle(&self.run_id).await?;
        let golden: Vec<train_b::BundleRow> =
            bundle.rows.iter().filter(|r| r.golden).cloned().collect();
        let ctx = self.ctx.clone();
        let (run_id, venue) = (self.run_id.clone(), self.venue.clone());
        let h = tokio::runtime::Handle::current();
        let outcome = tokio::task::spawn_blocking(move || {
            let base = open_base(&ctx.decision_cfg, &ctx.ckpt())?;
            let cfg = TrainConfig {
                venue: "local".into(),
                ..TrainConfig::from_double(&ctx.cfg)
            };
            let out = runs_dir(&ctx.data_dir).join("tier-b");
            let pause = OwnerActivity::new(Box::new(PlatformIdle));
            let mut on_progress = |p: &train_b::Progress| {
                publish(
                    &h,
                    &ctx.rt,
                    DoubleTrainingState {
                        run_id: run_id.clone(),
                        tier: Tier::B,
                        venue: venue.clone(),
                        state: RunState::Running,
                        progress: Some(p.step as f32 / p.total_steps.max(1) as f32),
                        message: progress_message(p),
                    },
                );
            };
            TierB::train(
                &bundle,
                &cfg,
                &base,
                &out,
                Hooks {
                    pause: &pause,
                    on_progress: &mut on_progress,
                },
            )
        })
        .await??;
        match outcome {
            TrainOutcome::Paused { .. } => Ok(None),
            TrainOutcome::Finished(c) => self.ctx.finish(*c, golden).await,
        }
    }
}

/// Tier B on a mesh GPU node: ship the bundle, take the adapter back, gate it
/// here.
pub struct NodeTierB {
    ctx: Ctx,
    peer: PeerId,
    run_id: String,
    venue: String,
}

#[async_trait::async_trait]
impl Trainer for NodeTierB {
    fn steps(&self) -> u32 {
        1
    }

    fn bind(&mut self, run_id: &str, venue: &str) {
        self.run_id = run_id.to_owned();
        self.venue = venue.to_owned();
    }

    async fn step(&mut self, _: u32) -> anyhow::Result<Option<Finished>> {
        let ctx = &self.ctx;
        let mesh_bundle = mesh::build_bundle(
            &ctx.store,
            &self.run_id,
            Tier::B,
            &ctx.base_hash,
            &ctx.cfg.training.tiers.b,
        )
        .await?;
        let golden: Vec<train_b::BundleRow> = mesh_bundle
            .rows
            .iter()
            .filter(|r| r.golden)
            .filter_map(to_row)
            .collect();
        let transport = replica::transport(&ctx.rt)
            .await
            .ok_or_else(|| anyhow::anyhow!("mesh transport unavailable"))?;
        let h = tokio::runtime::Handle::current();
        let (run_id, venue) = (self.run_id.clone(), self.venue.clone());
        let result = mesh::train_on(
            transport.as_ref(),
            &self.peer,
            &mesh_bundle,
            |state, progress, message| {
                publish(
                    &h,
                    &ctx.rt,
                    DoubleTrainingState {
                        run_id: run_id.clone(),
                        tier: Tier::B,
                        venue: venue.clone(),
                        state,
                        progress,
                        message,
                    },
                );
            },
        )
        .await?;
        let adapter = result
            .adapter
            .ok_or_else(|| anyhow::anyhow!("the node returned no adapter"))?;
        let manifest: train_b::Manifest = serde_json::from_str(&result.manifest_json)?;
        let dir = runs_dir(&ctx.data_dir).join(&self.run_id);
        std::fs::create_dir_all(&dir)?;
        std::fs::write(dir.join(ADAPTER_FILE), adapter)?;
        std::fs::write(dir.join(train_b::MANIFEST_FILE), &result.manifest_json)?;
        let c = CandidateVersion {
            id: format!("b-{}", manifest.trained_at.format("%Y%m%dT%H%M%SZ")),
            dir,
            manifest,
        };
        ctx.finish(c, golden).await
    }
}

/// `(tier, venue) → trainer`: Tier A as W2 built it, Tier B locally or on a
/// node; `None` for Tier C, `off` and unreachable nodes.
pub fn factory(
    rt: Runtime,
    store: Store,
    models: PathBuf,
    cfg: DoubleConfig,
    data_dir: PathBuf,
    decision_cfg: decision::DecisionConfig,
    base_hash: String,
) -> Arc<sched::Factory> {
    let tier_a = sched::tier_a_factory(store.clone(), models.clone(), base_hash.clone());
    let ctx = Ctx {
        rt,
        store,
        models,
        cfg,
        data_dir,
        decision_cfg,
        base_hash,
    };
    Arc::new(move |tier, venue| match (tier, venue) {
        (Tier::A, Resolved::Local) => tier_a(tier, venue),
        (Tier::B, Resolved::Local) => Some(Box::new(LocalTierB {
            ctx: ctx.clone(),
            run_id: String::new(),
            venue: String::new(),
        }) as Box<dyn Trainer>),
        (Tier::B, Resolved::Node { peer, .. }) => Some(Box::new(NodeTierB {
            ctx: ctx.clone(),
            peer: peer.clone(),
            run_id: String::new(),
            venue: String::new(),
        }) as Box<dyn Trainer>),
        _ => None,
    })
}

/// The node side of `theuth/double/1`: one Tier B run at a time against this
/// machine's own copy of the checkpoint, under `<data_dir>/double/runs/<run_id>`.
pub struct NodeRunner {
    data_dir: PathBuf,
    cfg: DoubleConfig,
    decision_cfg: decision::DecisionConfig,
    /// The run in flight.
    busy: Mutex<Option<String>>,
}

impl NodeRunner {
    /// Serve bundles for `cfg` against `decision_cfg`'s default checkpoint.
    pub fn new(
        data_dir: PathBuf,
        cfg: DoubleConfig,
        decision_cfg: decision::DecisionConfig,
    ) -> Self {
        Self {
            data_dir,
            cfg,
            decision_cfg,
            busy: Mutex::new(None),
        }
    }
}

#[async_trait::async_trait]
impl mesh::Runner for NodeRunner {
    async fn accept(&self, m: &mesh::BundleManifest) -> Result<(), String> {
        if !TRAIN_BUILD {
            return Err("this node was built without `decision/train`".into());
        }
        if m.tier != Tier::B {
            return Err(format!(
                "tier {} is not available on this node",
                m.tier.as_str()
            ));
        }
        let ckpt = checkpoint_dir(&self.decision_cfg, &self.data_dir);
        if !ckpt.is_dir() {
            return Err(format!("no checkpoint at {}", ckpt.display()));
        }
        let have = base_hash(&ckpt).map_err(|e| format!("checkpoint hash: {e:#}"))?;
        if have != m.base_hash {
            return Err(format!(
                "checkpoint mismatch: node has {have}, bundle wants {}",
                m.base_hash
            ));
        }
        let mut busy = self.busy.lock();
        if let Some(id) = busy.as_ref() {
            return Err(format!("busy with run {id}"));
        }
        *busy = Some(m.run_id.clone());
        Ok(())
    }

    async fn run(
        &self,
        bundle: mesh::Bundle,
        progress: mpsc::Sender<(f32, String)>,
    ) -> anyhow::Result<mesh::TrainResult> {
        let run_id = bundle.manifest.run_id.clone();
        let bundle = to_train_b(&bundle);
        let cfg = TrainConfig {
            venue: format!("node:{}", platform::process::hostname()),
            ..TrainConfig::from_double(&self.cfg)
        };
        let ckpt = checkpoint_dir(&self.decision_cfg, &self.data_dir);
        let out = runs_dir(&self.data_dir).join(&run_id);
        let decision_cfg = self.decision_cfg.clone();
        let outcome = tokio::task::spawn_blocking(move || {
            let base = open_base(&decision_cfg, &ckpt)?;
            let mut on_progress = |p: &train_b::Progress| {
                let _ = progress.blocking_send((
                    p.step as f32 / p.total_steps.max(1) as f32,
                    progress_message(p),
                ));
            };
            TierB::train(
                &bundle,
                &cfg,
                &base,
                &out,
                Hooks {
                    pause: &Never,
                    on_progress: &mut on_progress,
                },
            )
        })
        .await??;
        match outcome {
            TrainOutcome::Finished(c) => Ok(mesh::TrainResult {
                run_id,
                adapter: Some(std::fs::read(c.dir.join(ADAPTER_FILE))?),
                heads: None,
                manifest_json: serde_json::to_string(&c.manifest)?,
                metrics_json: serde_json::to_string(&c.manifest.metrics)?,
            }),
            TrainOutcome::Paused { step, .. } => {
                anyhow::bail!("run cap reached on the node after step {step}")
            }
        }
    }

    async fn cleanup(&self, run_id: &str) {
        let _ = std::fs::remove_dir_all(runs_dir(&self.data_dir).join(run_id));
        let mut busy = self.busy.lock();
        if busy.as_deref() == Some(run_id) {
            *busy = None;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn row(labels: Vec<(&str, &str, f32)>) -> mesh::BundleRow {
        mesh::BundleRow {
            id: "e1".into(),
            class: "approval:shell:read_only".into(),
            requester: "root".into(),
            card_text: "ls".into(),
            options: vec!["allow".into(), "deny".into()],
            labels: labels
                .into_iter()
                .map(|(s, c, w)| (s.to_owned(), c.to_owned(), w))
                .collect(),
            asked_at: "2026-10-01T00:00:00Z".into(),
            golden: false,
        }
    }

    #[test]
    fn net_label_argmax_and_source() {
        let r = to_row(&row(vec![
            ("direct", "allow", 1.0),
            ("hindsight_rewind", "allow", 0.5),
        ]))
        .unwrap();
        assert_eq!(
            (r.choice.as_str(), r.weight, r.source),
            ("allow", 0.5, LabelSource::Direct)
        );
        let r = to_row(&row(vec![("override", "none", 1.0)])).unwrap();
        assert_eq!(
            (r.choice.as_str(), r.source),
            ("none", LabelSource::Override)
        );
        assert!(to_row(&row(vec![("hindsight_rewind", "allow", 0.5)])).is_none());
        assert!(to_row(&row(vec![("excluded", "allow", 1.0)])).is_none());
        let mut bad = row(vec![("direct", "allow", 1.0)]);
        bad.asked_at = "yesterday".into();
        assert!(to_row(&bad).is_none());
    }
}
