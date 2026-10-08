//! The actor driven end to end against a test double for W1/W2/W6 (140 §15
//! `stop`, `restart`, lifecycle, the §4.3 abandon-don't-block invariant, the
//! §8.2 refusal). The double answers what each scenario needs; everything the
//! actor itself does (ledger rows, states, events, files) is real.

use std::collections::{BTreeMap, VecDeque};
use std::path::Path;
use std::sync::Arc;
use std::time::Duration;

use parking_lot::Mutex;
use tokio_util::sync::CancellationToken;

use super::*;
use crate::store::Store;

/// The W1/W2/W6 double: scripted samples, a scripted gate, a scripted reviewer.
#[derive(Default)]
struct Fake {
    values: Mutex<VecDeque<f64>>,
    decide: Mutex<Option<GateStep>>,
    review: Mutex<Option<ReviewVerdict>>,
    custody_changed: Mutex<Vec<String>>,
    scope: Mutex<ScopeDiff>,
    repair: Mutex<Option<String>>,
    spend: Mutex<f64>,
    samples_taken: Mutex<u32>,
    restored: Mutex<Vec<u32>>,
    novelty_refusal: Mutex<Option<String>>,
}

impl Fake {
    fn with_values(v: &[f64]) -> Arc<Self> {
        let f = Self::default();
        *f.values.lock() = v.iter().copied().collect();
        Arc::new(f)
    }
    fn decide(&self, verdict: Verdict, delta_pct: f64, z: f64) {
        *self.decide.lock() = Some(GateStep::Decide {
            verdict,
            delta: delta_pct,
            delta_pct,
            z: Some(z),
        });
    }
}

#[async_trait::async_trait]
impl Backend for Fake {
    fn is_git_repo(&self, _: &Path) -> bool {
        true
    }
    fn resolve_base(&self, _: &Path, _: &str) -> Result<String> {
        Ok("base0000".into())
    }
    fn worktree_add(
        &self,
        _: &Path,
        _: &StudySpec,
        worktree: &Path,
        base_sha: &str,
    ) -> Result<String> {
        std::fs::create_dir_all(worktree)?;
        Ok(base_sha.to_owned())
    }
    fn worktree_remove(&self, _: &Path, _: &Path) -> Result<()> {
        Ok(())
    }
    fn keep(&self, _: &Path, _: &StudySpec, meta: &KeepMeta, _: Option<&str>) -> Result<String> {
        Ok(format!("keep{:04}", meta.n))
    }
    fn discard(&self, _: &Path, _: &str, patch_path: &Path) -> Result<String> {
        std::fs::write(patch_path, "diff")?;
        Ok("patchhash".into())
    }
    fn repair(&self, _: &Path, _: &StudySpec, _: &str, _: &Path) -> Result<Option<String>> {
        Ok(self.repair.lock().take())
    }
    fn reject(&self, _: &Path, _: &str, _: u32, _: &str) -> Result<()> {
        Ok(())
    }
    fn restore_rejected(&self, _: &Path, _: &str, n: u32) -> Result<()> {
        self.restored.lock().push(n);
        Ok(())
    }
    fn patch_hash(&self, _: &Path, _: &str) -> Result<String> {
        Ok("ph".into())
    }
    fn diff_text(&self, _: &Path, _: &str, _: &str, _: usize) -> Result<String> {
        Ok("diff".into())
    }
    async fn sample(
        &self,
        _: &StudySpec,
        _: &crate::config::Bench,
        req: SampleRequest<'_>,
        _: &CancellationToken,
    ) -> Result<Sample> {
        *self.samples_taken.lock() += 1;
        let value = self.values.lock().pop_front();
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
    fn tail(&self, _: &Path) -> String {
        String::new()
    }
    fn custody_take(
        &self,
        _: &StudySpec,
        _: &Path,
        dir: &Path,
    ) -> Result<BTreeMap<String, String>> {
        std::fs::create_dir_all(dir)?;
        Ok([("bench.sh".to_owned(), "abc".to_owned())]
            .into_iter()
            .collect())
    }
    fn custody_verify(&self, _: &BTreeMap<String, String>, _: &Path) -> Result<Vec<String>> {
        Ok(self.custody_changed.lock().clone())
    }
    fn scope_diff(&self, _: &StudySpec, _: &Path) -> Result<ScopeDiff> {
        Ok(self.scope.lock().clone())
    }
    fn pooled_sigma(&self, trees: &[Vec<f64>]) -> f64 {
        let (mut ss, mut df) = (0.0, 0usize);
        for t in trees.iter().filter(|t| t.len() > 1) {
            let m = t.iter().sum::<f64>() / t.len() as f64;
            ss += t.iter().map(|x| (x - m).powi(2)).sum::<f64>();
            df += t.len() - 1;
        }
        if df == 0 {
            0.0
        } else {
            (ss / df as f64).sqrt()
        }
    }
    fn gate_step(&self, _: &GateCfg, _: &GateInput) -> GateStep {
        self.decide.lock().clone().unwrap_or(GateStep::Decide {
            verdict: Verdict::NoGain,
            delta: 0.0,
            delta_pct: 0.0,
            z: Some(0.2),
        })
    }
    fn keep_simpler(&self, _: &GateCfg, _: &GateInput, added: u32, removed: u32) -> bool {
        removed > added
    }
    fn drifted(&self, _: f64, _: f64, _: f64) -> bool {
        false
    }
    fn gate_line(&self, _: &GateStep, _: &str, _: f64, _: f64, _: usize) -> String {
        String::new()
    }
    async fn brief(
        &self,
        _: &Store,
        study: &StudyRecord,
        _: &crate::config::BriefConfig,
        cx: &crate::brief::Context,
    ) -> Result<Brief> {
        Ok(Brief {
            text: format!(
                "<study slug=\"{}\" experiment=\"{}\">{}</study>",
                study.slug,
                cx.n,
                cx.incidents.join(" ")
            ),
            n: cx.n,
            segment: study.segment,
            slots: None,
        })
    }
    async fn novelty(
        &self,
        _: &Store,
        _: &str,
        _: &RunArgs,
        _: &str,
        _: f64,
        _: bool,
    ) -> Result<Novelty> {
        Ok(Novelty {
            refused: self.novelty_refusal.lock().clone(),
            ..Novelty::default()
        })
    }
    async fn mark_tried(&self, _: &Store, _: &str, _: &str, _: u32) -> Result<Vec<i64>> {
        Ok(vec![])
    }
    async fn review(
        &self,
        _: &Store,
        _: &StudyRecord,
        _: u32,
        _: &str,
        _: &runtime::Runtime,
    ) -> Result<ReviewVerdict> {
        Ok(self.review.lock().clone().unwrap_or(ReviewVerdict::Clean {
            reason: "fine".into(),
        }))
    }
    async fn advise(
        &self,
        _: &Store,
        _: &StudyRecord,
        _: Option<&str>,
        _: &runtime::Runtime,
    ) -> Result<Vec<IdeaDraft>> {
        Ok(vec![IdeaDraft {
            text: "try SIMD".into(),
            category: "algorithm".into(),
            priority: None,
        }])
    }
    async fn finalize(
        &self,
        _: &Store,
        study: &StudyRecord,
        _: StopReason,
        _: &runtime::Runtime,
    ) -> Result<(Headline, String)> {
        Ok((
            Headline {
                text: "time_ms 100 → 90 (−10.0 %)".into(),
                base_mean: 100.0,
                final_mean: study.incumbent_mean.unwrap_or(100.0),
                delta_pct: -10.0,
                ..Headline::default()
            },
            "doc-1".into(),
        ))
    }
    async fn record(
        &self,
        _: &Store,
        _: &StudyRecord,
        _: &str,
        _: &runtime::Runtime,
    ) -> Result<String> {
        Ok("records/x".into())
    }
    async fn seed_playbook(&self, _: &str, _: &runtime::Runtime) -> Result<Option<String>> {
        Ok(None)
    }
    async fn spend_usd(&self, _: Option<SessionId>) -> f64 {
        *self.spend.lock()
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

struct Harness {
    _dir: tempfile::TempDir,
    studies: Studies,
}

async fn harness(fake: Arc<Fake>, cfg: AutoresearchConfig) -> Harness {
    let dir = tempfile::tempdir().unwrap();
    let rt = runtime::Runtime::new(figment::Figment::new(), dir.path().to_path_buf());
    let root = dir.path().join("autoresearch");
    let store = Store::open(&root).await.unwrap();
    let studies = Studies {
        data_dir: dir.path().to_path_buf(),
        root,
        cfg,
        agent: AgentDefaults::default(),
        rt,
        store,
        backend: fake,
    };
    Harness { _dir: dir, studies }
}

fn spec(h: &Harness, slug: &str, experiments: u32) -> StudySpec {
    StudySpec {
        slug: slug.into(),
        goal: "faster".into(),
        repo: h._dir.path().display().to_string(),
        metric: MetricSpec {
            name: "time_ms".into(),
            unit: "ms".into(),
            direction: Direction::Lower,
            min_effect: "0.5%".into(),
        },
        bench: BenchSpec {
            command: "bench.sh".into(),
            repeats: 3,
            ..BenchSpec::default()
        },
        budget: BudgetSpec {
            experiments,
            ..BudgetSpec::default()
        },
        ..StudySpec::default()
    }
}

async fn wait_state(store: &Store, slug: &str, pred: impl Fn(&StudyState) -> bool) -> StudyRecord {
    for _ in 0..200 {
        let s = store.get_study(slug).await.unwrap().unwrap();
        if pred(&s.state) {
            return s;
        }
        tokio::time::sleep(Duration::from_millis(20)).await;
    }
    panic!("study `{slug}` never reached the state");
}

/// Create, wait for `running`, bind a session (§8.2) and return it.
async fn started(h: &mut Harness, slug: &str, experiments: u32) -> SessionId {
    let origin = SessionId::new();
    let sp = spec(h, slug, experiments);
    let info = create(
        &mut h.studies,
        Create {
            spec: sp,
            origin,
            start: StartWhen::Now,
        },
    )
    .await
    .unwrap();
    assert_eq!(info.state, StudyState::Preparing);
    wait_state(&h.studies.store, slug, |s| *s == StudyState::Running).await;
    let session = SessionId::new();
    binding::on_fired(&mut h.studies, &format!("autoresearch-{slug}"), session)
        .await
        .unwrap();
    session
}

#[tokio::test]
async fn create_prepares_to_running_with_baseline_sigma_and_schedule_file() {
    let fake = Fake::with_values(&[100.0, 101.0, 99.0]);
    let mut h = harness(fake.clone(), AutoresearchConfig::default()).await;
    started(&mut h, "t1", 10).await;
    let s = h.studies.store.get_study("t1").await.unwrap().unwrap();
    assert_eq!(s.base_sha.as_deref(), Some("base0000"));
    assert_eq!(s.incumbent_sha.as_deref(), Some("base0000"));
    assert_eq!(s.incumbent_mean, Some(100.0));
    assert!((s.sigma.unwrap() - 1.0).abs() < 1e-9, "{:?}", s.sigma);
    assert!(s.started_at.is_some());
    let seg = h.studies.store.get_segment("t1", 0).await.unwrap().unwrap();
    assert_eq!((seg.baseline_mean, seg.baseline_n), (Some(100.0), 3));
    assert_eq!(seg.custody.get("bench.sh").map(String::as_str), Some("abc"));
    let file = h._dir.path().join("schedules/autoresearch-t1.md");
    let text = std::fs::read_to_string(&file).unwrap();
    assert!(
        text.contains("every: 0s")
            && text.contains("max_runs: 20")
            && text.contains("budget: 1.5 USD"),
        "{text}"
    );
    assert_eq!(*fake.samples_taken.lock(), 3);
    let list = status(&mut h.studies, Status::default()).await.unwrap();
    assert_eq!(list.len(), 1);
    assert!(list[0].headline.contains("running"), "{}", list[0].headline);
    // A second study with the same slug is refused.
    let sp = spec(&h, "t1", 10);
    let e = create(
        &mut h.studies,
        Create {
            spec: sp,
            origin: SessionId::new(),
            start: StartWhen::Now,
        },
    )
    .await
    .unwrap_err()
    .to_string();
    assert!(e.contains("taken"), "{e}");
}

#[tokio::test]
async fn noisy_baseline_fails_with_the_numbers() {
    let fake = Fake::with_values(&[100.0, 120.0, 80.0]);
    let mut h = harness(fake, AutoresearchConfig::default()).await;
    let sp = spec(&h, "noisy", 10);
    create(
        &mut h.studies,
        Create {
            spec: sp,
            origin: SessionId::new(),
            start: StartWhen::Now,
        },
    )
    .await
    .unwrap();
    let s = wait_state(&h.studies.store, "noisy", |s| {
        matches!(s, StudyState::Failed { .. })
    })
    .await;
    let StudyState::Failed { reason } = s.state else {
        unreachable!()
    };
    assert!(
        reason.contains("σ = 20.0 % of the metric") && reason.contains("40.0 %"),
        "{reason}"
    );
    assert!(
        !h._dir
            .path()
            .join("schedules/autoresearch-noisy.md")
            .exists()
    );
}

#[tokio::test]
async fn invalid_specs_are_refused_before_anything_is_written() {
    let mut h = harness(Fake::with_values(&[]), AutoresearchConfig::default()).await;
    let mut bad = spec(&h, "Bad Slug", 10);
    let e = create(
        &mut h.studies,
        Create {
            spec: bad.clone(),
            origin: SessionId::new(),
            start: StartWhen::Now,
        },
    )
    .await
    .unwrap_err()
    .to_string();
    assert!(e.contains("slug"), "{e}");
    bad.slug = "ok".into();
    bad.bench.command = String::new();
    let e = create(
        &mut h.studies,
        Create {
            spec: bad,
            origin: SessionId::new(),
            start: StartWhen::Now,
        },
    )
    .await
    .unwrap_err()
    .to_string();
    assert!(e.contains("bench.command"), "{e}");
    assert!(h.studies.store.list_studies().await.unwrap().is_empty());
}

#[tokio::test]
async fn foreign_session_is_refused_by_every_study_tool() {
    let mut h = harness(
        Fake::with_values(&[100.0, 100.0, 100.0]),
        AutoresearchConfig::default(),
    )
    .await;
    started(&mut h, "t1", 10).await;
    let other = SessionId::new();
    for e in [
        begin(
            &mut h.studies,
            Begin {
                session: other,
                slot: None,
            },
        )
        .await
        .unwrap_err()
        .to_string(),
        run(
            &mut h.studies,
            Run {
                session: other,
                slot: None,
                args: RunArgs::default(),
            },
        )
        .await
        .unwrap_err()
        .to_string(),
        log(
            &mut h.studies,
            Log {
                session: other,
                slot: None,
                args: LogArgs::default(),
            },
        )
        .await
        .unwrap_err()
        .to_string(),
        notes(
            &mut h.studies,
            Notes {
                session: other,
                args: NotesArgs::default(),
            },
        )
        .await
        .unwrap_err()
        .to_string(),
    ] {
        assert_eq!(e, "not an autoresearch session");
    }
}

#[tokio::test]
async fn one_experiment_kept_moves_the_incumbent() {
    let fake = Fake::with_values(&[100.0, 100.0, 100.0, 90.0]);
    let mut h = harness(fake.clone(), AutoresearchConfig::default()).await;
    let session = started(&mut h, "t1", 10).await;
    let brief = begin(
        &mut h.studies,
        Begin {
            session,
            slot: None,
        },
    )
    .await
    .unwrap();
    assert_eq!(brief.n, 1);
    assert!(brief.text.contains("experiment=\"1\""));
    // Run before log twice is refused; run without an idea is refused.
    let e = run(
        &mut h.studies,
        Run {
            session,
            slot: None,
            args: RunArgs::default(),
        },
    )
    .await
    .unwrap_err()
    .to_string();
    assert!(e.contains("idea"), "{e}");
    fake.decide(Verdict::Improved, 10.0, 4.0);
    let args = RunArgs {
        idea: "skip the copy".into(),
        category: "allocation".into(),
        hypothesis: "less work".into(),
        ..RunArgs::default()
    };
    let report = run(
        &mut h.studies,
        Run {
            session,
            slot: None,
            args: args.clone(),
        },
    )
    .await
    .unwrap();
    assert_eq!(report.verdict, Verdict::Improved);
    assert_eq!(report.samples.len(), 1);
    assert_eq!(report.delta_pct, Some(10.0));
    let e = run(
        &mut h.studies,
        Run {
            session,
            slot: None,
            args,
        },
    )
    .await
    .unwrap_err()
    .to_string();
    assert!(e.contains("already measured"), "{e}");
    let lr = log(
        &mut h.studies,
        Log {
            session,
            slot: None,
            args: LogArgs {
                description: "removed a clone".into(),
                learned: "copies cost".into(),
                ..LogArgs::default()
            },
        },
    )
    .await
    .unwrap();
    assert_eq!(lr.applied, ExperimentState::Kept);
    assert_eq!(lr.commit.as_deref(), Some("keep0001"));
    assert_eq!(lr.incumbent.sha, "keep0001");
    assert_eq!(lr.incumbent.kept, 1);
    assert_eq!(lr.incumbent.mean, Some(90.0));
    assert!(
        matches!(lr.review, Some(ReviewVerdict::Clean { .. })),
        "{:?}",
        lr.review
    );
    assert_eq!(lr.stop, None);
    assert_eq!(lr.budget_left.experiments, 9);
    // The candidate's samples now belong to the incumbent tree (§5.2 x̄_i).
    let inc = h
        .studies
        .store
        .samples_of_tree("t1", "keep0001")
        .await
        .unwrap();
    assert_eq!(inc.len(), 1);
    let rows = experiments(
        &mut h.studies,
        Experiments {
            slug: "t1".into(),
            before: None,
            limit: 10,
        },
    )
    .await
    .unwrap();
    assert_eq!(rows.len(), 1);
    assert_eq!(rows[0].commit_sha.as_deref(), Some("keep0001"));
    let d = detail(&mut h.studies, Detail { slug: "t1".into() })
        .await
        .unwrap();
    assert_eq!(d.series.incumbent.len(), 2, "baseline step + the keep");
    assert_eq!(d.info.incumbent.kept, 1);
    assert_eq!(d.info.delta_pct.map(|p| p.round()), Some(10.0));
    let x = experiment(
        &mut h.studies,
        Experiment {
            slug: "t1".into(),
            n: 1,
        },
    )
    .await
    .unwrap();
    assert_eq!(x.diff.as_deref(), Some("diff"));
    assert_eq!(x.description, "removed a clone");
}

#[tokio::test]
async fn discard_saves_a_patch_and_refusals_spend_no_sample() {
    let fake = Fake::with_values(&[100.0, 100.0, 100.0, 100.5, 100.0]);
    let mut h = harness(fake.clone(), AutoresearchConfig::default()).await;
    let session = started(&mut h, "t1", 10).await;
    begin(
        &mut h.studies,
        Begin {
            session,
            slot: None,
        },
    )
    .await
    .unwrap();
    let args = RunArgs {
        idea: "x".into(),
        category: "c".into(),
        ..RunArgs::default()
    };
    let r = run(
        &mut h.studies,
        Run {
            session,
            slot: None,
            args: args.clone(),
        },
    )
    .await
    .unwrap();
    assert_eq!(r.verdict, Verdict::NoGain);
    let lr = log(
        &mut h.studies,
        Log {
            session,
            slot: None,
            args: LogArgs::default(),
        },
    )
    .await
    .unwrap();
    assert_eq!(lr.applied, ExperimentState::Discarded);
    assert!(h.studies.root.join("t1/runs/1/candidate.patch").exists());
    assert_eq!(lr.incumbent.sha, "base0000");

    // Custody refusal: no sample spent, counted toward the pause rule.
    *fake.custody_changed.lock() = vec!["bench.sh".into()];
    begin(
        &mut h.studies,
        Begin {
            session,
            slot: None,
        },
    )
    .await
    .unwrap();
    let before = *fake.samples_taken.lock();
    let r = run(
        &mut h.studies,
        Run {
            session,
            slot: None,
            args: args.clone(),
        },
    )
    .await
    .unwrap();
    assert!(
        matches!(&r.verdict, Verdict::Refused { reason } if reason.starts_with("evaluator changed (bench.sh)")),
        "{:?}",
        r.verdict
    );
    assert_eq!(*fake.samples_taken.lock(), before);
    let lr = log(
        &mut h.studies,
        Log {
            session,
            slot: None,
            args: LogArgs::default(),
        },
    )
    .await
    .unwrap();
    assert_eq!(lr.applied, ExperimentState::Refused);
    *fake.custody_changed.lock() = vec![];

    // Off-limits: refused; a keep that touches scope deviations needs a justification.
    *fake.scope.lock() = ScopeDiff {
        off_limits: vec!["test/a.rb".into()],
        ..ScopeDiff::default()
    };
    begin(
        &mut h.studies,
        Begin {
            session,
            slot: None,
        },
    )
    .await
    .unwrap();
    let r = run(
        &mut h.studies,
        Run {
            session,
            slot: None,
            args: args.clone(),
        },
    )
    .await
    .unwrap();
    assert!(
        matches!(&r.verdict, Verdict::Refused { reason } if reason.contains("off-limits test/a.rb"))
    );
    log(
        &mut h.studies,
        Log {
            session,
            slot: None,
            args: LogArgs::default(),
        },
    )
    .await
    .unwrap();
    *fake.scope.lock() = ScopeDiff {
        deviations: vec!["Rakefile".into()],
        ..ScopeDiff::default()
    };
    fake.decide(Verdict::Improved, 5.0, 4.0);
    begin(
        &mut h.studies,
        Begin {
            session,
            slot: None,
        },
    )
    .await
    .unwrap();
    let r = run(
        &mut h.studies,
        Run {
            session,
            slot: None,
            args,
        },
    )
    .await
    .unwrap();
    assert_eq!(r.deviations, vec!["Rakefile".to_owned()]);
    let e = log(
        &mut h.studies,
        Log {
            session,
            slot: None,
            args: LogArgs::default(),
        },
    )
    .await
    .unwrap_err()
    .to_string();
    assert!(e.contains("justification"), "{e}");
    let lr = log(
        &mut h.studies,
        Log {
            session,
            slot: None,
            args: LogArgs {
                justification: Some("build flag".into()),
                ..LogArgs::default()
            },
        },
    )
    .await
    .unwrap();
    assert_eq!(lr.applied, ExperimentState::Kept);
}

#[tokio::test]
async fn abandon_does_not_block_the_next_begin() {
    let fake = Fake::with_values(&[100.0, 100.0, 100.0]);
    let mut h = harness(fake.clone(), AutoresearchConfig::default()).await;
    let session = started(&mut h, "t1", 10).await;
    let b1 = begin(
        &mut h.studies,
        Begin {
            session,
            slot: None,
        },
    )
    .await
    .unwrap();
    assert_eq!(b1.n, 1);
    // The turn ended without experiment_log; the next begin closes #1 as abandoned and says so once.
    *fake.repair.lock() = Some("uncommitted changes".into());
    let b2 = begin(
        &mut h.studies,
        Begin {
            session,
            slot: None,
        },
    )
    .await
    .unwrap();
    assert_eq!(b2.n, 2);
    assert!(
        b2.text.contains("experiment 1 was not logged"),
        "{}",
        b2.text
    );
    assert!(b2.text.contains("uncommitted changes"), "{}", b2.text);
    let e1 = h
        .studies
        .store
        .get_experiment("t1", 1)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(e1.state, ExperimentState::Abandoned);
    assert!(e1.finished_at.is_some());
    let incidents = h
        .studies
        .store
        .list_events("t1", Some("incident"), 10)
        .await
        .unwrap();
    assert_eq!(incidents.len(), 2);
    // TurnFinished with #2 still open closes it too (and counts it as an experiment).
    binding::on_turn_finished(&mut h.studies, session)
        .await
        .unwrap();
    let e2 = h
        .studies
        .store
        .get_experiment("t1", 2)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(e2.state, ExperimentState::Abandoned);
    let info = &status(
        &mut h.studies,
        Status {
            slug: Some("t1".into()),
        },
    )
    .await
    .unwrap()[0];
    assert_eq!(info.experiments, 2);
    // A turn of a session that is not a study is a no-op.
    binding::on_turn_finished(&mut h.studies, SessionId::new())
        .await
        .unwrap();
}

#[tokio::test]
async fn experiment_budget_stops_and_finalizes_with_the_result_card_data() {
    let fake = Fake::with_values(&[100.0, 100.0, 100.0, 95.0]);
    let mut h = harness(fake.clone(), AutoresearchConfig::default()).await;
    let session = started(&mut h, "t1", 1).await;
    begin(
        &mut h.studies,
        Begin {
            session,
            slot: None,
        },
    )
    .await
    .unwrap();
    fake.decide(Verdict::Improved, 5.0, 4.0);
    run(
        &mut h.studies,
        Run {
            session,
            slot: None,
            args: RunArgs {
                idea: "x".into(),
                ..RunArgs::default()
            },
        },
    )
    .await
    .unwrap();
    let lr = log(
        &mut h.studies,
        Log {
            session,
            slot: None,
            args: LogArgs::default(),
        },
    )
    .await
    .unwrap();
    assert_eq!(lr.stop, Some(StopReason::Experiments));
    let s = wait_state(&h.studies.store, "t1", |s| *s == StudyState::Finished).await;
    assert_eq!(s.report_doc.as_deref(), Some("doc-1"));
    assert_eq!(s.stop_reason, Some(StopReason::Experiments));
    assert!(s.finished_at.is_some());
    // Fires after the stop are refused.
    let e = begin(
        &mut h.studies,
        Begin {
            session,
            slot: None,
        },
    )
    .await
    .unwrap_err()
    .to_string();
    assert!(e.contains("finished"), "{e}");
    // The attention source lists the finished study once.
    let items = roster::attention::Source::items(&AttentionSource(h.studies.store.clone())).await;
    assert_eq!(items.len(), 1);
    assert_eq!(items[0].id, "autoresearch:finished:t1");
    assert_eq!(items[0].kind, roster::attention::AttentionKind::Review);
    // Discard after finish removes the worktree and ends in `discarded`.
    let info = control(
        &mut h.studies,
        Control {
            slug: "t1".into(),
            op: ControlOp::Discard,
            by: By::User,
        },
    )
    .await
    .unwrap();
    assert_eq!(info.state, StudyState::Discarded);
}

#[tokio::test]
async fn budget_cap_is_rechecked_at_begin() {
    let fake = Fake::with_values(&[100.0, 100.0, 100.0]);
    let mut h = harness(fake.clone(), AutoresearchConfig::default()).await;
    let session = started(&mut h, "t1", 10).await;
    *fake.spend.lock() = 25.0;
    let e = begin(
        &mut h.studies,
        Begin {
            session,
            slot: None,
        },
    )
    .await
    .unwrap_err()
    .to_string();
    assert!(e.contains("budget exhausted"), "{e}");
    let s = wait_state(&h.studies.store, "t1", |s| *s == StudyState::Finished).await;
    assert_eq!(s.stop_reason, Some(StopReason::Budget));
}

#[tokio::test]
async fn crash_streak_pauses_with_attention_and_resume_counts_paused_time() {
    let mut cfg = AutoresearchConfig::default();
    cfg.pause.crash_streak = 2;
    let fake = Fake::with_values(&[100.0, 100.0, 100.0]); // candidates crash: queue empty → value None
    let mut h = harness(fake.clone(), cfg).await;
    let session = started(&mut h, "t1", 10).await;
    let args = RunArgs {
        idea: "x".into(),
        ..RunArgs::default()
    };
    for n in 1..=2 {
        begin(
            &mut h.studies,
            Begin {
                session,
                slot: None,
            },
        )
        .await
        .unwrap();
        let r = run(
            &mut h.studies,
            Run {
                session,
                slot: None,
                args: args.clone(),
            },
        )
        .await
        .unwrap();
        assert_eq!(r.verdict, Verdict::Crash, "#{n}");
        assert_eq!(r.retries_left, 2);
        // A crash allows another run in the same experiment (up to crash_retries).
        let r2 = run(
            &mut h.studies,
            Run {
                session,
                slot: None,
                args: args.clone(),
            },
        )
        .await
        .unwrap();
        assert_eq!(r2.verdict, Verdict::Crash);
        let lr = log(
            &mut h.studies,
            Log {
                session,
                slot: None,
                args: LogArgs::default(),
            },
        )
        .await
        .unwrap();
        assert_eq!(lr.applied, ExperimentState::Crashed);
    }
    let s = h.studies.store.get_study("t1").await.unwrap().unwrap();
    assert_eq!(
        s.state,
        StudyState::Paused {
            reason: PauseReason::CrashStreak
        }
    );
    let items = roster::attention::Source::items(&AttentionSource(h.studies.store.clone())).await;
    assert_eq!(items.len(), 1);
    assert_eq!(items[0].kind, roster::attention::AttentionKind::Blocked);
    assert!(
        items[0].title.contains("keeps failing"),
        "{}",
        items[0].title
    );
    // Resume (no schedule actor in tests: the ledger state is what Begin enforces).
    let info = control(
        &mut h.studies,
        Control {
            slug: "t1".into(),
            op: ControlOp::Resume,
            by: By::User,
        },
    )
    .await
    .unwrap();
    assert_eq!(info.state, StudyState::Running);
    let ev = h
        .studies
        .store
        .list_events("t1", Some("resumed"), 1)
        .await
        .unwrap();
    assert_eq!(ev[0].payload["after_n"], 2);
    // The crash streak does not re-pause until two more crashes.
    begin(
        &mut h.studies,
        Begin {
            session,
            slot: None,
        },
    )
    .await
    .unwrap();
    run(
        &mut h.studies,
        Run {
            session,
            slot: None,
            args: args.clone(),
        },
    )
    .await
    .unwrap();
    log(
        &mut h.studies,
        Log {
            session,
            slot: None,
            args: LogArgs::default(),
        },
    )
    .await
    .unwrap();
    let s = h.studies.store.get_study("t1").await.unwrap().unwrap();
    assert_eq!(
        s.state,
        StudyState::Paused {
            reason: PauseReason::CrashStreak
        },
        "three in a row ≥ 2: pauses again"
    );
}

#[tokio::test]
async fn plateau_explores_once_then_stops() {
    let mut cfg = AutoresearchConfig::default();
    cfg.plateau.window = 2;
    let fake = Fake::with_values(&[
        100.0, 100.0, 100.0, 100.0, 100.0, 100.0, 100.0, 100.0, 100.0,
    ]);
    let mut h = harness(fake.clone(), cfg).await;
    let session = started(&mut h, "t1", 100).await;
    let args = RunArgs {
        idea: "x".into(),
        ..RunArgs::default()
    };
    let mut stop = None;
    for _ in 1..=4 {
        let b = begin(
            &mut h.studies,
            Begin {
                session,
                slot: None,
            },
        )
        .await
        .unwrap();
        run(
            &mut h.studies,
            Run {
                session,
                slot: None,
                args: args.clone(),
            },
        )
        .await
        .unwrap();
        let lr = log(
            &mut h.studies,
            Log {
                session,
                slot: None,
                args: LogArgs::default(),
            },
        )
        .await
        .unwrap();
        if b.n == 3 {
            // After two no-gains the brief is in explore mode and the advisor's idea is in the backlog.
            let ideas = h
                .studies
                .store
                .list_ideas("t1", Some(IdeaStatus::Open))
                .await
                .unwrap();
            assert_eq!(ideas.len(), 1);
            assert_eq!(ideas[0].source, IdeaSource::Advisor);
            assert_eq!(ideas[0].priority, 2);
        }
        stop = lr.stop;
    }
    assert_eq!(stop, Some(StopReason::Plateau));
    assert_eq!(
        h.studies
            .store
            .list_events("t1", Some("explore"), 10)
            .await
            .unwrap()
            .len(),
        1
    );
}

#[tokio::test]
async fn review_reverts_a_suspicious_keep_and_restore_queues_it() {
    let fake = Fake::with_values(&[100.0, 100.0, 100.0, 50.0]);
    let mut h = harness(fake.clone(), AutoresearchConfig::default()).await;
    let session = started(&mut h, "t1", 10).await;
    begin(
        &mut h.studies,
        Begin {
            session,
            slot: None,
        },
    )
    .await
    .unwrap();
    fake.decide(Verdict::Suspicious, 50.0, 12.0);
    *fake.review.lock() = Some(ReviewVerdict::Flagged {
        reason: "caches the benchmark input".into(),
    });
    run(
        &mut h.studies,
        Run {
            session,
            slot: None,
            args: RunArgs {
                idea: "memo".into(),
                ..RunArgs::default()
            },
        },
    )
    .await
    .unwrap();
    let lr = log(
        &mut h.studies,
        Log {
            session,
            slot: None,
            args: LogArgs::default(),
        },
    )
    .await
    .unwrap();
    assert_eq!(lr.applied, ExperimentState::RejectedByReview);
    assert!(matches!(lr.review, Some(ReviewVerdict::Reverted { .. })));
    assert_eq!(lr.commit, None);
    assert_eq!(lr.incumbent.sha, "base0000");
    assert_eq!(lr.incumbent.mean, Some(100.0));
    let items = roster::attention::Source::items(&AttentionSource(h.studies.store.clone())).await;
    assert_eq!(items.len(), 1);
    assert!(items[0].id.ends_with(":reverted:t1:1") && items[0].title.contains("possible gaming"));
    control(
        &mut h.studies,
        Control {
            slug: "t1".into(),
            op: ControlOp::Restore { n: 1 },
            by: By::User,
        },
    )
    .await
    .unwrap();
    let ideas = h
        .studies
        .store
        .list_ideas("t1", Some(IdeaStatus::Open))
        .await
        .unwrap();
    assert_eq!(ideas.len(), 1);
    assert!(ideas[0].text.contains("Restore experiment #1") && ideas[0].priority == 3);
    let items = roster::attention::Source::items(&AttentionSource(h.studies.store.clone())).await;
    assert!(items.is_empty(), "the row clears once acted on");
    // The next begin re-applies the rejected change itself (restore_rejected) and the brief says so;
    // the run that follows is not a duplicate even though its patch hash equals the dead end's.
    let brief = begin(
        &mut h.studies,
        Begin {
            session,
            slot: None,
        },
    )
    .await
    .unwrap();
    assert!(
        brief.text.contains("restores #1") && brief.text.contains("already applied"),
        "{}",
        brief.text
    );
    assert_eq!(*fake.restored.lock(), vec![1]);
    let applied = h
        .studies
        .store
        .list_events("t1", Some("restore_applied"), 5)
        .await
        .unwrap();
    assert_eq!(applied.len(), 1);
    assert_eq!(applied[0].payload["of"], 1);
    let items = roster::attention::Source::items(&AttentionSource(h.studies.store.clone())).await;
    assert!(items.is_empty(), "no second restore is pending");
    *fake.novelty_refusal.lock() = Some("duplicate of #1".into());
    fake.decide(Verdict::NoGain, 0.0, 0.1);
    let r = run(
        &mut h.studies,
        Run {
            session,
            slot: None,
            args: RunArgs {
                idea: "memo".into(),
                ..RunArgs::default()
            },
        },
    )
    .await
    .unwrap();
    assert!(!matches!(r.verdict, Verdict::Refused { .. }), "{r:?}");
    *fake.novelty_refusal.lock() = None;
    // Restore of an ordinary experiment is refused.
    let e = control(
        &mut h.studies,
        Control {
            slug: "t1".into(),
            op: ControlOp::Restore { n: 7 },
            by: By::User,
        },
    )
    .await
    .unwrap_err()
    .to_string();
    assert!(e.contains("no experiment #7"), "{e}");
}

#[tokio::test]
async fn controls_pause_resume_stop_after_and_steer() {
    let fake = Fake::with_values(&[100.0, 100.0, 100.0]);
    let mut h = harness(fake, AutoresearchConfig::default()).await;
    let session = started(&mut h, "t1", 10).await;
    let c = |op| Control {
        slug: "t1".into(),
        op,
        by: By::User,
    };
    let i = control(&mut h.studies, c(ControlOp::Pause)).await.unwrap();
    assert_eq!(
        i.state,
        StudyState::Paused {
            reason: PauseReason::User
        }
    );
    // A user pause is not an attention row.
    assert!(
        roster::attention::Source::items(&AttentionSource(h.studies.store.clone()))
            .await
            .is_empty()
    );
    let i = control(&mut h.studies, c(ControlOp::Resume)).await.unwrap();
    assert_eq!(i.state, StudyState::Running);
    let id = steer(
        &mut h.studies,
        SteerMsg {
            slug: "t1".into(),
            text: "try SIMD".into(),
            by: By::User,
        },
    )
    .await
    .unwrap();
    assert!(id > 0);
    let idea = idea(
        &mut h.studies,
        IdeaOp {
            slug: "t1".into(),
            op: IdeaChange::Add {
                text: "unroll".into(),
                category: "loop".into(),
                priority: None,
            },
            by: By::User,
        },
    )
    .await
    .unwrap();
    assert_eq!((idea.priority, idea.source), (3, IdeaSource::User));
    let closed = idea_op(&mut h, IdeaChange::Close { id: idea.id }).await;
    assert_eq!(closed.status, IdeaStatus::Closed);
    // The next brief carries the steer once.
    begin(
        &mut h.studies,
        Begin {
            session,
            slot: None,
        },
    )
    .await
    .unwrap();
    let steers = h.studies.store.list_steers("t1", false).await.unwrap();
    assert_eq!(steers[0].seen_in, Some(1));
    // Stop after: the experiment in flight finishes first, then the study finalizes.
    let i = control(&mut h.studies, c(ControlOp::StopAfter))
        .await
        .unwrap();
    assert_eq!(i.state, StudyState::Running);
    run(
        &mut h.studies,
        Run {
            session,
            slot: None,
            args: RunArgs {
                idea: "x".into(),
                ..RunArgs::default()
            },
        },
    )
    .await
    .unwrap();
    let lr = log(
        &mut h.studies,
        Log {
            session,
            slot: None,
            args: LogArgs::default(),
        },
    )
    .await
    .unwrap();
    assert_eq!(lr.stop, Some(StopReason::User));
    wait_state(&h.studies.store, "t1", |s| *s == StudyState::Finished).await;
    let e = control(&mut h.studies, c(ControlOp::Pause))
        .await
        .unwrap_err()
        .to_string();
    assert!(e.contains("cannot pause"), "{e}");
}

async fn idea_op(h: &mut Harness, op: IdeaChange) -> Idea {
    idea(
        &mut h.studies,
        IdeaOp {
            slug: "t1".into(),
            op,
            by: By::User,
        },
    )
    .await
    .unwrap()
}

#[tokio::test]
async fn stop_now_interrupts_the_experiment_in_flight() {
    let fake = Fake::with_values(&[100.0, 100.0, 100.0]);
    let mut h = harness(fake, AutoresearchConfig::default()).await;
    let session = started(&mut h, "t1", 10).await;
    begin(
        &mut h.studies,
        Begin {
            session,
            slot: None,
        },
    )
    .await
    .unwrap();
    let i = control(
        &mut h.studies,
        Control {
            slug: "t1".into(),
            op: ControlOp::StopNow,
            by: By::User,
        },
    )
    .await
    .unwrap();
    assert!(
        matches!(i.state, StudyState::Finalizing | StudyState::Finished),
        "{:?}",
        i.state
    );
    let e1 = h
        .studies
        .store
        .get_experiment("t1", 1)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(e1.state, ExperimentState::Interrupted);
    let s = wait_state(&h.studies.store, "t1", |s| *s == StudyState::Finished).await;
    assert_eq!(s.stop_reason, Some(StopReason::UserNow));
}

#[tokio::test]
async fn restart_recovery_marks_interrupted_and_lost() {
    let fake = Fake::with_values(&[100.0, 100.0, 100.0]);
    let mut h = harness(fake.clone(), AutoresearchConfig::default()).await;
    let session = started(&mut h, "t1", 10).await;
    begin(
        &mut h.studies,
        Begin {
            session,
            slot: None,
        },
    )
    .await
    .unwrap();
    // Simulate the engine dying mid-sample: the row says measuring, the sample never finished.
    let mut e = h
        .studies
        .store
        .get_experiment("t1", 1)
        .await
        .unwrap()
        .unwrap();
    e.state = ExperimentState::Measuring;
    h.studies.store.update_experiment(&e).await.unwrap();
    h.studies
        .store
        .insert_sample(
            "t1",
            &Sample {
                n: 1,
                k: 1,
                tree_sha: "exp:1".into(),
                role: Some(SampleRole::Candidate),
                at: now(),
                ..Sample::default()
            },
        )
        .await
        .unwrap();
    let mut preparing = StudyRecord::new(spec(&h, "p", 5), None);
    preparing.state = StudyState::Preparing;
    h.studies.store.create_study(&preparing).await.unwrap();

    recover(&h.studies.env()).await.unwrap();

    let e = h
        .studies
        .store
        .get_experiment("t1", 1)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(e.state, ExperimentState::Interrupted);
    let s = h.studies.store.list_samples("t1", Some(1)).await.unwrap();
    assert!(s[0].lost && s[0].value.is_none());
    let p = h.studies.store.get_study("p").await.unwrap().unwrap();
    assert!(
        matches!(p.state, StudyState::Failed { .. }),
        "{:?}",
        p.state
    );
    // The next begin repairs the worktree and goes on with #2.
    *fake.repair.lock() = Some("HEAD at exp:1".into());
    let b = begin(
        &mut h.studies,
        Begin {
            session,
            slot: None,
        },
    )
    .await
    .unwrap();
    assert_eq!(b.n, 2);
    assert!(b.text.contains("HEAD at exp:1"));
}

#[tokio::test]
async fn notes_cap_the_playbook_and_add_ideas() {
    let fake = Fake::with_values(&[100.0, 100.0, 100.0]);
    let mut cfg = AutoresearchConfig::default();
    cfg.brief.playbook_bytes = 16;
    let mut h = harness(fake, cfg).await;
    let session = started(&mut h, "t1", 10).await;
    let e = notes(
        &mut h.studies,
        Notes {
            session,
            args: NotesArgs {
                playbook: Some("x".repeat(17)),
                add_ideas: vec![],
            },
        },
    )
    .await
    .unwrap_err()
    .to_string();
    assert!(e.contains("17 bytes") && e.contains("cap is 16"), "{e}");
    let r = notes(
        &mut h.studies,
        Notes {
            session,
            args: NotesArgs {
                playbook: Some("build: just".into()),
                add_ideas: vec![IdeaDraft {
                    text: "cache".into(),
                    category: "caching".into(),
                    priority: Some(9),
                }],
            },
        },
    )
    .await
    .unwrap();
    assert_eq!((r.playbook_bytes, r.ideas_added), (11, 1));
    let ideas = h.studies.store.list_ideas("t1", None).await.unwrap();
    assert_eq!((ideas[0].priority, ideas[0].source), (3, IdeaSource::Model));
}

#[test]
fn cards_render_valid_json_render_fences() {
    let mut study = StudyRecord::new(
        StudySpec {
            slug: "s".into(),
            goal: "g".into(),
            repo: "/r/liquid".into(),
            ..StudySpec::default()
        },
        None,
    );
    study.base_sha = Some("abcdef0123456789".into());
    let start = start_card(&study, 52.1, 0.5, "autoresearch-s");
    assert!(start.contains("```json-render\n"));
    let body = start
        .split("```json-render\n")
        .nth(1)
        .unwrap()
        .split("\n```")
        .next()
        .unwrap();
    let v: serde_json::Value = serde_json::from_str(body).unwrap();
    assert_eq!(v["root"], "c");
    assert_eq!(v["elements"]["g"]["type"], "MetricGrid");
    study.stop_reason = Some(StopReason::Plateau);
    study.report_doc = Some("doc".into());
    let series = ChartSeries {
        experiments: (1..=70)
            .map(|n| ChartPoint {
                n,
                verdict: "no_gain".into(),
                metric: Some(50.0),
                at: String::new(),
            })
            .collect(),
        incumbent: vec![ChartPoint {
            n: 0,
            verdict: String::new(),
            metric: Some(52.1),
            at: String::new(),
        }],
        baseline_mean: Some(52.1),
        ..ChartSeries::default()
    };
    let cs = vec![Changeset {
        k: 1,
        name: "alloc".into(),
        branch: "autoresearch/s/1-alloc".into(),
        files: vec!["a".into()],
        ..Changeset::default()
    }];
    let h = Headline {
        text: "time_ms 52.1 → 41.2 (−20.9 %)".into(),
        base_mean: 52.1,
        final_mean: 41.2,
        delta_pct: -20.9,
        ..Headline::default()
    };
    let result = result_card(&study, &h, &series, &cs, 70, 4);
    let body = result
        .split("```json-render\n")
        .nth(1)
        .unwrap()
        .split("\n```")
        .next()
        .unwrap();
    let v: serde_json::Value = serde_json::from_str(body).unwrap();
    let labels = v["elements"]["l"]["props"]["labels"].as_array().unwrap();
    assert!(labels.len() <= 60, "{}", labels.len());
    assert_eq!(
        v["elements"]["l"]["props"]["series"][1]["values"]
            .as_array()
            .unwrap()
            .len(),
        labels.len()
    );
    assert_eq!(v["elements"]["t"]["type"], "Table");
    assert!(result.contains("Report: `doc`"));
}
