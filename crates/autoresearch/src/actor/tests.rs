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
/// `sample` honours `hang`: with it set, samples block until `release` fires
/// (the engine-dies-mid-sample scenario).
struct Fake {
    values: Mutex<VecDeque<f64>>,
    decide: Mutex<Option<GateStep>>,
    review: Mutex<Option<ReviewVerdict>>,
    custody_changed: Mutex<Vec<String>>,
    scope: Mutex<ScopeDiff>,
    repair: Mutex<Option<String>>,
    /// `session_cost`: `Err` = the ledger is unreachable.
    spend: Mutex<std::result::Result<f64, String>>,
    samples_taken: Mutex<u32>,
    restored: Mutex<Vec<u32>>,
    novelty_refusal: Mutex<Option<String>>,
    include_dirty: Mutex<Option<bool>>,
    discards: Mutex<u32>,
    applied: Mutex<u32>,
    patch_hash: Mutex<String>,
    head: Mutex<String>,
    parent: Mutex<String>,
    report_user: Mutex<Option<String>>,
    hang: std::sync::atomic::AtomicBool,
    /// With `hang`: a cancel surfaces as the slot's wait error instead of a lost sample.
    hang_errs: std::sync::atomic::AtomicBool,
    release: tokio::sync::Notify,
    fail_next: std::sync::atomic::AtomicBool,
    last_input: Mutex<Option<GateInput>>,
    confirm_once: Mutex<Option<GateStep>>,
    tried: Mutex<Vec<(u32, String)>>,
    custody_from: Mutex<Vec<std::path::PathBuf>>,
    /// What `finalize` returns as the report document (a path = the no-documents-actor fallback).
    doc: Mutex<Option<String>>,
}

impl Default for Fake {
    fn default() -> Self {
        Self {
            values: Default::default(),
            decide: Default::default(),
            review: Default::default(),
            custody_changed: Default::default(),
            // The real diff is never empty when the model edited the worktree; an
            // empty one is refused before novelty (140-P), so the double says "one file".
            scope: Mutex::new(ScopeDiff {
                changed: vec!["lib/a.rs".into()],
                ..ScopeDiff::default()
            }),
            repair: Default::default(),
            spend: Mutex::new(Ok(0.0)),
            samples_taken: Default::default(),
            restored: Default::default(),
            novelty_refusal: Default::default(),
            include_dirty: Default::default(),
            discards: Default::default(),
            applied: Default::default(),
            patch_hash: Mutex::new("ph".into()),
            head: Mutex::new("base0000".into()),
            parent: Mutex::new(String::new()),
            report_user: Default::default(),
            hang: Default::default(),
            hang_errs: Default::default(),
            release: Default::default(),
            fail_next: Default::default(),
            last_input: Default::default(),
            confirm_once: Default::default(),
            tried: Default::default(),
            custody_from: Default::default(),
            doc: Mutex::new(Some("doc-1".into())),
        }
    }
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
    fn resolve_base(&self, _: &Path, _: &str, include_dirty: bool) -> Result<String> {
        *self.include_dirty.lock() = Some(include_dirty);
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
        // `include_dirty`: the real one commits a `base snapshot` and returns its sha.
        Ok(if *self.include_dirty.lock() == Some(true) {
            "snapshot".to_owned()
        } else {
            base_sha.to_owned()
        })
    }
    fn worktree_remove(&self, _: &Path, _: &Path) -> Result<()> {
        Ok(())
    }
    fn keep(&self, _: &Path, _: &StudySpec, meta: &KeepMeta, _: Option<&str>) -> Result<String> {
        Ok(format!("keep{:04}", meta.n))
    }
    fn discard(&self, _: &Path, _: &str, patch_path: &Path) -> Result<String> {
        std::fs::write(patch_path, "diff")?;
        *self.discards.lock() += 1;
        Ok("patchhash".into())
    }
    fn apply_patch(&self, _: &Path, patch_path: &Path) -> Result<()> {
        assert!(
            patch_path.exists(),
            "the stash must exist before it is re-applied"
        );
        *self.applied.lock() += 1;
        Ok(())
    }
    fn discard_study(&self, _: &Path, slug: &str, _: &Path) -> Result<Vec<String>> {
        Ok(vec![format!("autoresearch/{slug}")])
    }
    fn head(&self, _: &Path) -> Result<String> {
        Ok(self.head.lock().clone())
    }
    fn commit_parent(&self, _: &Path, _: &str) -> Result<String> {
        Ok(self.parent.lock().clone())
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
        Ok(self.patch_hash.lock().clone())
    }
    fn diff_text(&self, _: &Path, _: &str, _: &str, _: usize) -> Result<String> {
        Ok("diff".into())
    }
    async fn sample(
        &self,
        _: &StudySpec,
        _: &crate::config::Bench,
        req: SampleRequest<'_>,
        cancel: &CancellationToken,
    ) -> Result<Sample> {
        *self.samples_taken.lock() += 1;
        if self
            .fail_next
            .swap(false, std::sync::atomic::Ordering::SeqCst)
        {
            return Err(Error::refused("bench could not spawn"));
        }
        let mut lost = false;
        if self.hang.load(std::sync::atomic::Ordering::SeqCst) {
            tokio::select! {
                _ = self.release.notified() => {}
                _ = cancel.cancelled() => lost = true,
            }
            if lost && self.hang_errs.load(std::sync::atomic::Ordering::SeqCst) {
                return Err(Error::refused(
                    "cancelled while waiting for the load precondition",
                ));
            }
        }
        let value = if lost {
            None
        } else {
            self.values.lock().pop_front()
        };
        Ok(Sample {
            n: req.n,
            k: req.k,
            tree_sha: req.tree_sha.to_owned(),
            role: Some(req.role),
            value,
            exit: if lost {
                None
            } else {
                Some(if value.is_some() { 0 } else { 1 })
            },
            lost,
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
        tree: &Path,
        dir: &Path,
    ) -> Result<BTreeMap<String, String>> {
        self.custody_from.lock().push(tree.to_path_buf());
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
    fn gate_step(&self, _: &GateCfg, input: &GateInput) -> GateStep {
        *self.last_input.lock() = Some(input.clone());
        // A scripted `Confirm` fires once, then the scripted decision (or no gain).
        if let Some(step) = self.confirm_once.lock().take() {
            return step;
        }
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
    async fn mark_tried(&self, _: &Store, _: &str, idea: &str, n: u32) -> Result<Vec<i64>> {
        self.tried.lock().push((n, idea.to_owned()));
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
        store: &Store,
        study: &StudyRecord,
        reason: StopReason,
        _: &AutoresearchConfig,
        _: &Path,
        _: &runtime::Runtime,
    ) -> Result<(Headline, Option<String>)> {
        let headline = Headline {
            text: "time_ms 100 → 90 (−10.0 %)".into(),
            base_mean: 100.0,
            final_mean: study.incumbent_mean.unwrap_or(100.0),
            delta_pct: -10.0,
            ..Headline::default()
        };
        // W2 records the changesets and the report steps; the actor's §11.3 turn, the
        // result card and `StudyDetail.changesets` read them back (140-P).
        let rows = store.list_experiments(&study.slug, None, u32::MAX).await?;
        let cs = vec![Changeset {
            k: 1,
            name: "a".into(),
            branch: format!("autoresearch/{}+1-a", study.slug),
            files: vec!["a".into()],
            experiments: vec![1],
            delta_alone_pct: Some(-10.0),
            delta_together_pct: Some(-10.0),
            ..Changeset::default()
        }];
        store
            .append_event(
                &study.slug,
                crate::finalize::EVENT,
                &serde_json::json!({ "step": "changesets", "changesets": cs, "notes": [] }),
            )
            .await?;
        let report = crate::finalize::assemble(study, reason, headline.clone(), cs, &rows, vec![]);
        let doc = self.doc.lock().clone();
        store
            .append_event(
                &study.slug,
                crate::finalize::EVENT,
                &serde_json::json!({ "step": "report", "doc": doc, "report": report }),
            )
            .await?;
        Ok((headline, doc))
    }
    async fn report_prose(
        &self,
        _: &StudyRecord,
        _: &str,
        _: &str,
        user: String,
    ) -> Result<String> {
        *self.report_user.lock() = Some(user);
        Ok("The study asked for speed and found it.".into())
    }
    async fn session_cost(&self, _: Option<SessionId>) -> Result<f64> {
        self.spend.lock().clone().map_err(Error::refused)
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
        agent: AgentDefaults {
            communication: true, // waves need spawn_subagent (140-P)
            ..AgentDefaults::default()
        },
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

/// Create, wait for `running`, bind a session (§8.2) and return it, seated as
/// the schedule's fire seats it (F4E: the worktree is the write root).
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
    let c = binding::confinement(h.studies.store.clone())(&format!("autoresearch-{slug}"))
        .await
        .unwrap()
        .unwrap();
    let worktree = c
        .write_root
        .expect("a running study fires with its worktree as write root");
    assert_eq!(
        worktree,
        h.studies.root.join(slug).join("worktree"),
        "write root is the study worktree"
    );
    binding::seat(session, &worktree);
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

/// 140-P: the schedule file appears only once the study is `running` — a fire that
/// follows the write at once must find a study the confinement resolver owns.
#[tokio::test]
async fn study_is_running_before_its_schedule_file_exists() {
    let fake = Fake::with_values(&[100.0, 101.0, 99.0]);
    let mut h = harness(fake.clone(), AutoresearchConfig::default()).await;
    let sp = spec(&h, "t1", 10);
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
    let file = h._dir.path().join("schedules/autoresearch-t1.md");
    for _ in 0..500 {
        if file.exists() {
            break;
        }
        tokio::time::sleep(Duration::from_millis(5)).await;
    }
    assert!(file.exists(), "schedule file never written");
    let s = h.studies.store.get_study("t1").await.unwrap().unwrap();
    assert_eq!(s.state, StudyState::Running, "file written before running");
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
    // 140-P: a wave study needs `agent.communication` (spawn_subagent).
    let mut wave = spec(&h, "wave", 10);
    wave.parallel = 2;
    h.studies.agent.communication = false;
    let e = create(
        &mut h.studies,
        Create {
            spec: wave,
            origin: SessionId::new(),
            start: StartWhen::Now,
        },
    )
    .await
    .unwrap_err()
    .to_string();
    assert!(e.contains("agent.communication"), "{e}");
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
    binding::on_turn_finished(&mut h.studies, session, "end_turn")
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
    binding::on_turn_finished(&mut h.studies, SessionId::new(), "end_turn")
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
    // R-56: §11 waits for the study turn to end; the study is `finalizing` meanwhile.
    let s = h.studies.store.get_study("t1").await.unwrap().unwrap();
    assert_eq!(s.state, StudyState::Finalizing);
    tokio::time::sleep(Duration::from_millis(50)).await;
    assert_eq!(
        h.studies
            .store
            .get_study("t1")
            .await
            .unwrap()
            .unwrap()
            .state,
        StudyState::Finalizing,
        "finalize does not start while the turn is live"
    );
    binding::on_turn_finished(&mut h.studies, session, "end_turn")
        .await
        .unwrap();
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
    *fake.spend.lock() = Ok(25.0);
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
    // The crash streak counts from the resume (R-57): one more crash does not
    // re-pause; the second does.
    for (n, expect) in [
        (3, StudyState::Running),
        (
            4,
            StudyState::Paused {
                reason: PauseReason::CrashStreak,
            },
        ),
    ] {
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
        assert_eq!(s.state, expect, "after crash #{n}");
    }
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
    let fake = Fake::with_values(&[100.0, 100.0, 100.0, 50.0, 50.0]);
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
    // The reviewer says SUSPICIOUS again: the user's Restore stands — flagged, not reverted.
    fake.decide(Verdict::Suspicious, 50.0, 12.0);
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
    assert_eq!(lr.applied, ExperimentState::Kept);
    assert!(
        matches!(lr.review, Some(ReviewVerdict::Flagged { .. })),
        "{:?}",
        lr.review
    );
    assert!(lr.commit.is_some());
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
    binding::on_turn_finished(&mut h.studies, session, "end_turn")
        .await
        .unwrap();
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
    // §4.3: never measured → `abandoned`, not `interrupted` (R-66).
    assert_eq!(e1.state, ExperimentState::Abandoned);
    // F4E: the write root holds through the stop (Finalizing) and is gone once finished.
    if i.state == StudyState::Finalizing {
        assert!(tools::seat::seat_of(session).write_root.is_some());
    }
    let s = wait_state(&h.studies.store, "t1", |s| *s == StudyState::Finished).await;
    assert_eq!(s.stop_reason, Some(StopReason::UserNow));
    assert_eq!(tools::seat::seat_of(session), tools::Seat::default());
}

/// A run blocked inside its benchmark, as another task (the model's tool call).
async fn hanging_run(
    h: &Harness,
    fake: &Arc<Fake>,
    session: SessionId,
) -> tokio::task::JoinHandle<Result<RunReport>> {
    fake.hang.store(true, std::sync::atomic::Ordering::SeqCst);
    let env = h.studies.env();
    let handle = tokio::spawn(async move {
        run_env(
            &env,
            Run {
                session,
                slot: None,
                args: RunArgs {
                    idea: "slow".into(),
                    ..RunArgs::default()
                },
            },
        )
        .await
    });
    // The pending sample row (R-34) is the sign the benchmark was spawned.
    for _ in 0..200 {
        if !h
            .studies
            .store
            .list_samples("t1", Some(1))
            .await
            .unwrap()
            .is_empty()
        {
            return handle;
        }
        tokio::time::sleep(Duration::from_millis(10)).await;
    }
    panic!("the run never inserted its pending sample row");
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
    // The engine dies mid-sample: the row says measuring, the pending sample row has no value.
    let handle = hanging_run(&h, &fake, session).await;
    let pending = h.studies.store.list_samples("t1", Some(1)).await.unwrap();
    assert_eq!(pending.len(), 1);
    assert!(pending[0].value.is_none() && pending[0].exit.is_none() && !pending[0].lost);
    let mut preparing = StudyRecord::new(spec(&h, "p", 5), None);
    preparing.state = StudyState::Preparing;
    h.studies.store.create_study(&preparing).await.unwrap();
    // 140-P: the loop ended and took the file with it (or the user deleted it).
    let file = h._dir.path().join("schedules/autoresearch-t1.md");
    std::fs::remove_file(&file).unwrap();

    recover(&h.studies.env()).await.unwrap();

    let text = std::fs::read_to_string(&file).expect("schedule file re-written at recovery");
    assert!(text.contains("enabled: true"), "{text}");

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
    // The late sample (the engine is gone in production) cannot reopen the closed row.
    fake.release.notify_waiters();
    fake.hang.store(false, std::sync::atomic::Ordering::SeqCst);
    assert!(handle.await.unwrap().is_err());
    let e = h
        .studies
        .store
        .get_experiment("t1", 1)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(e.state, ExperimentState::Interrupted);
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

// ───────────────────────────── 140-R fix round regressions ─────────────────────────────

fn b(session: SessionId) -> Begin {
    Begin {
        session,
        slot: None,
    }
}

fn r(session: SessionId, idea: &str) -> Run {
    Run {
        session,
        slot: None,
        args: RunArgs {
            idea: idea.into(),
            category: "c".into(),
            ..RunArgs::default()
        },
    }
}

fn l(session: SessionId, decision: Decision) -> Log {
    Log {
        session,
        slot: None,
        args: LogArgs {
            description: "d".into(),
            learned: "l".into(),
            decision,
            ..LogArgs::default()
        },
    }
}

/// 140-P live: the commit trailer read `8996 -> 39.0 (+99.6%)`; §6 wants the raw change.
#[test]
fn metric_line_reads_the_raw_change() {
    let mut study = StudyRecord::new(StudySpec::default(), None);
    study.incumbent_mean = Some(8996.0);
    let mut exp = ExperimentRecord::open("s", 2, 0, "inc");
    exp.metric = Some(39.0);
    exp.delta_pct = Some(99.6); // direction-normalised, positive = better
    exp.z = Some(15.3);
    exp.n_samples = 3;
    let line = super::metric_line(&study, &exp);
    assert!(
        line.contains("8996 -> 39.0 (-99.6%, 15.3 sigma, n=3)"),
        "{line}"
    );
}

/// 140-P live: one fire ran seven experiments. A turn that already began an
/// experiment cannot begin another; the next turn can.
#[tokio::test]
async fn second_begin_in_the_same_turn_is_refused() {
    let fake = Fake::with_values(&[100.0, 100.0, 100.0, 100.0, 100.0]);
    let mut h = harness(fake.clone(), AutoresearchConfig::default()).await;
    let session = started(&mut h, "t1", 10).await;
    let envelope = "<schedule name=\"autoresearch-t1\">\nrun\n</schedule>";
    binding::on_turn_started(&mut h.studies, session, envelope)
        .await
        .unwrap();
    begin(&mut h.studies, b(session)).await.unwrap();
    fake.decide(Verdict::NoGain, 0.0, 0.1);
    run(&mut h.studies, r(session, "x")).await.unwrap();
    log(&mut h.studies, l(session, Decision::Auto))
        .await
        .unwrap();
    let e = begin(&mut h.studies, b(session))
        .await
        .unwrap_err()
        .to_string();
    assert!(e.contains("one experiment per fire"), "{e}");
    assert!(
        h.studies
            .store
            .get_experiment("t1", 2)
            .await
            .unwrap()
            .is_none(),
        "no second experiment was opened"
    );
    // The next fire's turn begins #2.
    binding::on_turn_started(&mut h.studies, session, envelope)
        .await
        .unwrap();
    assert_eq!(begin(&mut h.studies, b(session)).await.unwrap().n, 2);
}

/// 140-P live: a worktree equal to the incumbent (the model edited elsewhere) is
/// refused by name before novelty — no sample, no "duplicate of #1" riddle — and the
/// experiment can be run again once the tree changed.
#[tokio::test]
async fn run_refuses_an_unchanged_worktree_by_name() {
    let fake = Fake::with_values(&[100.0, 100.0, 100.0, 90.0]);
    let mut h = harness(fake.clone(), AutoresearchConfig::default()).await;
    let session = started(&mut h, "t1", 10).await;
    begin(&mut h.studies, b(session)).await.unwrap();
    *fake.scope.lock() = ScopeDiff::default();
    let before = *fake.samples_taken.lock();
    let e = run(&mut h.studies, r(session, "x"))
        .await
        .unwrap_err()
        .to_string();
    assert!(
        e.starts_with("no change in the worktree") && e.contains("absolute path"),
        "{e}"
    );
    assert_eq!(*fake.samples_taken.lock(), before, "no sample spent");
    let x = h
        .studies
        .store
        .get_experiment("t1", 1)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(x.state, ExperimentState::Open, "handed back to the model");
    let ev = h
        .studies
        .store
        .list_events("t1", Some("no_change"), 1)
        .await
        .unwrap();
    assert_eq!(ev.len(), 1);
    // Same experiment, the tree now differs: measured normally.
    *fake.scope.lock() = ScopeDiff {
        changed: vec!["lib/a.rs".into()],
        ..ScopeDiff::default()
    };
    fake.decide(Verdict::Improved, 10.0, 4.0);
    let rep = run(&mut h.studies, r(session, "x")).await.unwrap();
    assert_eq!(rep.verdict, Verdict::Improved);
}

/// R-1: a keep commits the tree as it is now; a tree that no longer matches the
/// measured patch is not kept, and the model is told to measure again.
#[tokio::test]
async fn keep_refuses_a_tree_changed_after_the_measurement() {
    let fake = Fake::with_values(&[100.0, 100.0, 100.0, 90.0, 90.0, 90.0]);
    let mut h = harness(fake.clone(), AutoresearchConfig::default()).await;
    let session = started(&mut h, "t1", 10).await;
    begin(&mut h.studies, b(session)).await.unwrap();
    fake.decide(Verdict::Improved, 10.0, 4.0);
    run(&mut h.studies, r(session, "x")).await.unwrap();
    // The model edits after experiment_run returned.
    *fake.patch_hash.lock() = "edited".into();
    let e = log(&mut h.studies, l(session, Decision::Auto))
        .await
        .unwrap_err()
        .to_string();
    assert!(e.contains("changed since experiment_run"), "{e}");
    let x = h
        .studies
        .store
        .get_experiment("t1", 1)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(
        x.state,
        ExperimentState::Measured,
        "nothing was kept or closed"
    );
    assert_eq!(x.commit_sha, None);
    assert_eq!(*fake.discards.lock(), 0, "the candidate stays in the tree");
    // experiment_run is allowed again and measures the new tree; then the keep goes through.
    let rep = run(&mut h.studies, r(session, "x")).await.unwrap();
    assert_eq!(rep.verdict, Verdict::Improved);
    assert_eq!(
        rep.samples[0].tree_sha, "exp:1r1",
        "a re-run is its own tree for σ"
    );
    let lr = log(&mut h.studies, l(session, Decision::Auto))
        .await
        .unwrap();
    assert_eq!(lr.applied, ExperimentState::Kept);
    assert!(
        h.studies
            .store
            .list_events("t1", Some("keep"), 5)
            .await
            .unwrap()
            .len()
            == 1
    );

    // Custody changed after the measurement: the keep is impossible → refused, counted, reverted.
    begin(&mut h.studies, b(session)).await.unwrap();
    run(&mut h.studies, r(session, "y")).await.unwrap();
    *fake.custody_changed.lock() = vec!["bench.sh".into()];
    let lr = log(&mut h.studies, l(session, Decision::Auto))
        .await
        .unwrap();
    assert_eq!(lr.applied, ExperimentState::Refused);
    assert_eq!(lr.incumbent.sha, "keep0001");
    assert_eq!(
        h.studies
            .store
            .list_events("t1", Some("custody_refusal"), 5)
            .await
            .unwrap()
            .len(),
        1
    );
    *fake.custody_changed.lock() = vec![];
}

/// R-2 / R-23 / R-20: a confirm-phase incumbent sample measures the incumbent tree
/// (candidate stashed and re-applied), under its own `k`, and runs the drift check.
#[tokio::test]
async fn confirm_incumbent_sample_measures_the_incumbent_tree() {
    let fake = Fake::with_values(&[100.0, 100.0, 100.0, 95.0, 100.0, 95.0]);
    let mut h = harness(fake.clone(), AutoresearchConfig::default()).await;
    let session = started(&mut h, "t1", 10).await;
    begin(&mut h.studies, b(session)).await.unwrap();
    *fake.confirm_once.lock() = Some(GateStep::Confirm {
        more: 1,
        incumbent_sample: true,
    });
    fake.decide(Verdict::Improved, 5.0, 3.0);
    let rep = run(&mut h.studies, r(session, "x")).await.unwrap();
    assert_eq!(rep.verdict, Verdict::Improved);
    let inc: Vec<&Sample> = rep
        .samples
        .iter()
        .filter(|s| s.role == Some(SampleRole::Incumbent))
        .collect();
    assert_eq!(inc.len(), 1);
    assert_eq!((inc[0].tree_sha.as_str(), inc[0].k), ("base0000", 101));
    assert_eq!(*fake.discards.lock(), 1, "the candidate was stashed once");
    assert_eq!(*fake.applied.lock(), 1, "and re-applied once");
    assert!(!h.studies.root.join("t1/runs/1/stash-101.patch").exists());
    // The incumbent pool grew by the sample: 4 values, none of them the candidate's.
    let input = fake.last_input.lock().clone().unwrap();
    assert_eq!(input.incumbent, vec![100.0, 100.0, 100.0, 100.0]);
    assert_eq!(input.candidate, vec![95.0, 95.0]);
}

/// R-3: holdout rows share the incumbent's tree sha and must never enter x̄_i.
#[tokio::test]
async fn holdout_values_never_enter_incumbent_statistics() {
    // baseline 100 ×3, base holdout 500 ×3, candidate 90.
    let fake = Fake::with_values(&[100.0, 100.0, 100.0, 500.0, 500.0, 500.0, 90.0]);
    let mut h = harness(fake.clone(), AutoresearchConfig::default()).await;
    let mut sp = spec(&h, "t1", 10);
    sp.holdout = Some(HoldoutSpec {
        command: "holdout.sh".into(),
        every: 1,
        repeats: None,
    });
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
    wait_state(&h.studies.store, "t1", |s| *s == StudyState::Running).await;
    let session = SessionId::new();
    binding::on_fired(&mut h.studies, "autoresearch-t1", session)
        .await
        .unwrap();
    let s = h.studies.store.get_study("t1").await.unwrap().unwrap();
    assert_eq!(
        s.incumbent_mean,
        Some(100.0),
        "the base holdout is not in the mean"
    );
    begin(&mut h.studies, b(session)).await.unwrap();
    fake.decide(Verdict::Improved, 10.0, 4.0);
    let rep = run(&mut h.studies, r(session, "x")).await.unwrap();
    let input = fake.last_input.lock().clone().unwrap();
    assert_eq!(input.incumbent, vec![100.0, 100.0, 100.0]);
    assert!(rep.cases_moved.is_empty());
    let x = experiment(
        &mut h.studies,
        Experiment {
            slug: "t1".into(),
            n: 1,
        },
    )
    .await
    .unwrap();
    assert!(
        x.samples
            .iter()
            .all(|s| s.role != Some(SampleRole::Holdout))
    );
}

/// R-4: a Pause while a run is in flight is not reverted when the run writes its σ.
#[tokio::test]
async fn pause_during_a_run_survives_the_runs_write_back() {
    let fake = Fake::with_values(&[100.0, 100.0, 100.0, 100.0]);
    let mut h = harness(fake.clone(), AutoresearchConfig::default()).await;
    let session = started(&mut h, "t1", 10).await;
    begin(&mut h.studies, b(session)).await.unwrap();
    let handle = hanging_run(&h, &fake, session).await;
    let i = control(
        &mut h.studies,
        Control {
            slug: "t1".into(),
            op: ControlOp::Pause,
            by: By::User,
        },
    )
    .await
    .unwrap();
    assert!(matches!(i.state, StudyState::Paused { .. }));
    fake.hang.store(false, std::sync::atomic::Ordering::SeqCst);
    fake.release.notify_waiters();
    let rep = handle.await.unwrap().unwrap();
    assert_eq!(rep.verdict, Verdict::NoGain);
    let s = h.studies.store.get_study("t1").await.unwrap().unwrap();
    assert!(
        matches!(s.state, StudyState::Paused { .. }),
        "the run's σ write must not flip the state back: {:?}",
        s.state
    );
    assert!(s.sigma.is_some());
}

/// R-14: the money stop does not fail open — an unreadable ledger refuses the fire.
#[tokio::test]
async fn begin_refuses_when_the_spend_is_unknown() {
    let fake = Fake::with_values(&[100.0, 100.0, 100.0]);
    let mut h = harness(fake.clone(), AutoresearchConfig::default()).await;
    let session = started(&mut h, "t1", 10).await;
    *fake.spend.lock() = Err("agent actor is not running".into());
    let e = begin(&mut h.studies, b(session))
        .await
        .unwrap_err()
        .to_string();
    assert!(e.contains("spend unknown"), "{e}");
    assert_eq!(
        h.studies
            .store
            .get_study("t1")
            .await
            .unwrap()
            .unwrap()
            .state,
        StudyState::Running
    );
}

/// R-14: experiment costs are the per-row sums; the study total survives a session re-creation.
#[tokio::test]
async fn study_spend_sums_closed_rows_plus_the_live_tree() {
    let fake = Fake::with_values(&[100.0, 100.0, 100.0, 100.0, 100.0]);
    let mut h = harness(fake.clone(), AutoresearchConfig::default()).await;
    let session = started(&mut h, "t1", 10).await;
    *fake.spend.lock() = Ok(1.0);
    begin(&mut h.studies, b(session)).await.unwrap();
    run(&mut h.studies, r(session, "x")).await.unwrap();
    *fake.spend.lock() = Ok(2.5);
    let lr = log(&mut h.studies, l(session, Decision::Auto))
        .await
        .unwrap();
    let e1 = h
        .studies
        .store
        .get_experiment("t1", 1)
        .await
        .unwrap()
        .unwrap();
    assert!((e1.cost_nanos as f64 / 1e9 - 1.5).abs() < 1e-9);
    assert!(
        (lr.budget_left.usd - 18.5).abs() < 1e-9,
        "{}",
        lr.budget_left.usd
    );
    // The schedule re-created the session: the live tree restarts at 0.2 — nothing is lost or double counted.
    *fake.spend.lock() = Ok(0.2);
    let s = h.studies.store.get_study("t1").await.unwrap().unwrap();
    let total = study_spend(&h.studies.env(), &s).await.unwrap();
    assert!((total - 1.5).abs() < 1e-9, "{total}");
    begin(&mut h.studies, b(session)).await.unwrap();
    *fake.spend.lock() = Ok(0.7);
    let s = h.studies.store.get_study("t1").await.unwrap().unwrap();
    let total = study_spend(&h.studies.env(), &s).await.unwrap();
    assert!((total - 2.0).abs() < 1e-9, "{total}");
}

/// R-20: a confirm-phase incumbent sample does not consume the scheduled §5.2 step-4 re-measure.
#[tokio::test]
async fn scheduled_remeasure_is_counted_by_its_own_event() {
    let mut cfg = AutoresearchConfig::default();
    cfg.gate.remeasure_every = 1;
    let fake = Fake::with_values(&[100.0, 100.0, 100.0, 90.0, 100.0, 90.0, 100.0]);
    let mut h = harness(fake.clone(), cfg).await;
    let session = started(&mut h, "t1", 10).await;
    begin(&mut h.studies, b(session)).await.unwrap();
    *fake.confirm_once.lock() = Some(GateStep::Confirm {
        more: 1,
        incumbent_sample: true,
    });
    fake.decide(Verdict::Improved, 10.0, 4.0);
    run(&mut h.studies, r(session, "x")).await.unwrap();
    log(&mut h.studies, l(session, Decision::Auto))
        .await
        .unwrap();
    let before = *fake.samples_taken.lock();
    begin(&mut h.studies, b(session)).await.unwrap();
    assert_eq!(
        *fake.samples_taken.lock(),
        before + 1,
        "one scheduled re-measure"
    );
    let ev = h
        .studies
        .store
        .list_events("t1", Some("remeasure"), 5)
        .await
        .unwrap();
    assert_eq!(ev.len(), 1);
    let inc = h
        .studies
        .store
        .samples_of_tree("t1", "keep0001")
        .await
        .unwrap();
    assert!(
        inc.iter()
            .any(|s| s.role == Some(SampleRole::Incumbent) && s.k == 0)
    );
}

/// R-21: keep-simpler is judged on means after `confirm_runs` more candidate samples.
#[tokio::test]
async fn keep_simpler_takes_confirm_samples_before_judging() {
    let fake = Fake::with_values(&[100.0, 100.0, 100.0, 100.5, 100.2, 99.9]);
    let mut h = harness(fake.clone(), AutoresearchConfig::default()).await;
    let session = started(&mut h, "t1", 10).await;
    begin(&mut h.studies, b(session)).await.unwrap();
    run(&mut h.studies, r(session, "x")).await.unwrap();
    *fake.scope.lock() = ScopeDiff {
        added: 2,
        removed: 10,
        ..ScopeDiff::default()
    };
    let lr = log(&mut h.studies, l(session, Decision::KeepSimpler))
        .await
        .unwrap();
    assert_eq!(lr.applied, ExperimentState::Kept);
    let cand: Vec<Sample> = h
        .studies
        .store
        .list_samples("t1", Some(1))
        .await
        .unwrap()
        .into_iter()
        .filter(|s| s.role == Some(SampleRole::Candidate))
        .collect();
    assert_eq!(cand.len(), 3, "1 + confirm_runs");
    let input = fake.last_input.lock().clone().unwrap();
    assert_eq!(
        input.candidate.len(),
        1,
        "the gate saw the first sample; keep_simpler saw the means"
    );
    let x = h
        .studies
        .store
        .get_experiment("t1", 1)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(x.n_samples, 3);
    assert_eq!(x.verdict, Some(Verdict::KeepSimpler));
}

/// R-44: the overfit event reaches the stop view (plateau rule) through `after_n`.
#[tokio::test]
async fn overfit_event_feeds_the_stop_view() {
    // baseline 100 ×3, base holdout 100 ×3, candidate 90, incumbent holdout 150 ×3 (worse).
    let fake = Fake::with_values(&[
        100.0, 101.0, 99.0, 100.0, 100.0, 100.0, 90.0, 150.0, 150.0, 150.0,
    ]);
    let mut h = harness(fake.clone(), AutoresearchConfig::default()).await;
    let mut sp = spec(&h, "t1", 10);
    sp.holdout = Some(HoldoutSpec {
        command: "holdout.sh".into(),
        every: 1,
        repeats: None,
    });
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
    wait_state(&h.studies.store, "t1", |s| *s == StudyState::Running).await;
    let session = SessionId::new();
    binding::on_fired(&mut h.studies, "autoresearch-t1", session)
        .await
        .unwrap();
    begin(&mut h.studies, b(session)).await.unwrap();
    fake.decide(Verdict::Improved, 10.0, 4.0);
    run(&mut h.studies, r(session, "x")).await.unwrap();
    log(&mut h.studies, l(session, Decision::Auto))
        .await
        .unwrap();
    let ev = h
        .studies
        .store
        .list_events("t1", Some("overfit"), 5)
        .await
        .unwrap();
    assert_eq!(ev.len(), 1);
    assert_eq!(ev[0].payload["after_n"], 1);
    let s = h.studies.store.get_study("t1").await.unwrap().unwrap();
    let view = stop::view(&h.studies.store, &s, 0.0).await.unwrap();
    assert!(view.overfit, "{view:?}");
}

/// R-45: the report document is the executor's prose followed by the plugin's tables.
#[tokio::test]
async fn finalize_writes_the_report_prose_before_the_tables() {
    let fake = Fake::with_values(&[100.0, 100.0, 100.0, 95.0]);
    let mut h = harness(fake.clone(), AutoresearchConfig::default()).await;
    let session = started(&mut h, "t1", 1).await;
    begin(&mut h.studies, b(session)).await.unwrap();
    *fake.spend.lock() = Ok(0.5); // the tree spent $0.50 between begin and close
    fake.decide(Verdict::Improved, 5.0, 4.0);
    run(&mut h.studies, r(session, "x")).await.unwrap();
    log(&mut h.studies, l(session, Decision::Auto))
        .await
        .unwrap();
    binding::on_turn_finished(&mut h.studies, session, "end_turn")
        .await
        .unwrap();
    wait_state(&h.studies.store, "t1", |s| *s == StudyState::Finished).await;
    let report = std::fs::read_to_string(h.studies.root.join("t1/report.md")).unwrap();
    let prose = report
        .find("The study asked for speed")
        .expect("prose in the report");
    let tables = report.find("**Headline.**").expect("tables in the report");
    assert!(prose < tables, "{report}");
    let user = fake.report_user.lock().clone().unwrap();
    assert!(
        user.contains("<brief>") && user.contains("<tables>"),
        "{user}"
    );
    assert_eq!(
        h.studies
            .store
            .list_events("t1", Some("report_prose"), 5)
            .await
            .unwrap()
            .len(),
        1
    );
    // 140-P: the detail reads W2's changesets record and a finished study's closed spend.
    let d = detail(&mut h.studies, Detail { slug: "t1".into() })
        .await
        .unwrap();
    assert_eq!(d.changesets.len(), 1, "{:?}", d.changesets);
    assert_eq!(d.changesets[0].branch, "autoresearch/t1+1-a");
    let x = h
        .studies
        .store
        .get_experiment("t1", 1)
        .await
        .unwrap()
        .unwrap();
    assert!(
        (d.info.budget_used.usd - x.cost_nanos as f64 / 1e9).abs() < 1e-9,
        "{} vs {}",
        d.info.budget_used.usd,
        x.cost_nanos
    );
}

/// R-49: Begin on a dangling measuring experiment kills the bench and marks its sample lost.
#[tokio::test]
async fn begin_cancels_a_dangling_measurement() {
    let fake = Fake::with_values(&[100.0, 100.0, 100.0]);
    let mut h = harness(fake.clone(), AutoresearchConfig::default()).await;
    let session = started(&mut h, "t1", 10).await;
    begin(&mut h.studies, b(session)).await.unwrap();
    let handle = hanging_run(&h, &fake, session).await;
    let brief = begin(&mut h.studies, b(session)).await.unwrap();
    assert_eq!(brief.n, 2);
    assert!(handle.await.unwrap().is_err(), "the run was cancelled");
    let e1 = h
        .studies
        .store
        .get_experiment("t1", 1)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(e1.state, ExperimentState::Interrupted);
    assert_eq!(
        e1.patch_path.as_deref(),
        Some("t1/runs/1/candidate.patch"),
        "R-63"
    );
    let s = h.studies.store.list_samples("t1", Some(1)).await.unwrap();
    assert!(s.iter().all(|s| s.lost), "{s:?}");
}

/// R-50: a keep whose commit landed but whose ledger write did not is adopted at the next Begin.
#[tokio::test]
async fn keep_commit_without_ledger_is_adopted_at_begin() {
    let fake = Fake::with_values(&[100.0, 100.0, 100.0, 90.0]);
    let mut h = harness(fake.clone(), AutoresearchConfig::default()).await;
    let session = started(&mut h, "t1", 10).await;
    begin(&mut h.studies, b(session)).await.unwrap();
    fake.decide(Verdict::Improved, 10.0, 4.0);
    run(&mut h.studies, r(session, "x")).await.unwrap();
    // The engine died between `git commit` and the ledger write.
    h.studies
        .store
        .append_event(
            "t1",
            "keep_pending",
            &serde_json::json!({ "n": 1, "parent": "base0000" }),
        )
        .await
        .unwrap();
    *fake.head.lock() = "deadbeef".into();
    *fake.parent.lock() = "base0000".into();
    let brief = begin(&mut h.studies, b(session)).await.unwrap();
    assert!(
        brief.text.contains("was kept (commit deadbeef)"),
        "{}",
        brief.text
    );
    let e1 = h
        .studies
        .store
        .get_experiment("t1", 1)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(
        (e1.state, e1.commit_sha.as_deref()),
        (ExperimentState::Kept, Some("deadbeef"))
    );
    let s = h.studies.store.get_study("t1").await.unwrap().unwrap();
    assert_eq!(s.incumbent_sha.as_deref(), Some("deadbeef"));
    assert_eq!(s.incumbent_mean, Some(90.0));
    assert_eq!(
        h.studies
            .store
            .samples_of_tree("t1", "deadbeef")
            .await
            .unwrap()
            .len(),
        1,
        "the candidate's sample was retagged"
    );
    assert_eq!(brief.n, 2);
}

/// R-51: a paused study fires only for the user's RunOnce.
#[tokio::test]
async fn paused_study_accepts_only_a_run_once_fire() {
    let fake = Fake::with_values(&[100.0, 100.0, 100.0]);
    let mut h = harness(fake, AutoresearchConfig::default()).await;
    let session = started(&mut h, "t1", 10).await;
    let c = |op| Control {
        slug: "t1".into(),
        op,
        by: By::User,
    };
    control(&mut h.studies, c(ControlOp::Pause)).await.unwrap();
    let e = begin(&mut h.studies, b(session))
        .await
        .unwrap_err()
        .to_string();
    assert!(e.contains("paused"), "{e}");
    // RunOnce records its intent before asking the schedule actor (absent in tests → Err).
    let _ = control(&mut h.studies, c(ControlOp::RunOnce)).await;
    let brief = begin(&mut h.studies, b(session)).await.unwrap();
    assert_eq!(brief.n, 1);
    let e = begin(&mut h.studies, b(session))
        .await
        .unwrap_err()
        .to_string();
    assert!(e.contains("paused"), "one fire per RunOnce: {e}");
}

/// R-52 / R-60: a failed spawn hands the experiment back open with its pending row lost;
/// a claim is conditional.
#[tokio::test]
async fn failed_measurement_reopens_the_experiment() {
    let fake = Fake::with_values(&[100.0, 100.0, 100.0, 100.0]);
    let mut h = harness(fake.clone(), AutoresearchConfig::default()).await;
    let session = started(&mut h, "t1", 10).await;
    begin(&mut h.studies, b(session)).await.unwrap();
    fake.fail_next
        .store(true, std::sync::atomic::Ordering::SeqCst);
    let e = run(&mut h.studies, r(session, "x"))
        .await
        .unwrap_err()
        .to_string();
    assert!(e.contains("could not spawn"), "{e}");
    let e1 = h
        .studies
        .store
        .get_experiment("t1", 1)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(e1.state, ExperimentState::Open);
    let s = h.studies.store.list_samples("t1", Some(1)).await.unwrap();
    assert!(s.len() == 1 && s[0].lost);
    let rep = run(&mut h.studies, r(session, "x")).await.unwrap();
    assert_eq!(rep.verdict, Verdict::NoGain);
    assert!(
        !h.studies
            .store
            .claim_experiment(
                "t1",
                1,
                &[ExperimentState::Open],
                ExperimentState::Measuring
            )
            .await
            .unwrap(),
        "a measured row cannot be claimed as open"
    );
}

/// R-69: a refused run leaves its idea open; a measured one marks it tried.
#[tokio::test]
async fn refused_runs_do_not_mark_ideas_tried() {
    let fake = Fake::with_values(&[100.0, 100.0, 100.0, 100.0]);
    let mut h = harness(fake.clone(), AutoresearchConfig::default()).await;
    let session = started(&mut h, "t1", 10).await;
    begin(&mut h.studies, b(session)).await.unwrap();
    *fake.novelty_refusal.lock() = Some("duplicate of #0".into());
    run(&mut h.studies, r(session, "dup")).await.unwrap();
    assert!(fake.tried.lock().is_empty());
    log(&mut h.studies, l(session, Decision::Auto))
        .await
        .unwrap();
    *fake.novelty_refusal.lock() = None;
    begin(&mut h.studies, b(session)).await.unwrap();
    run(&mut h.studies, r(session, "fresh")).await.unwrap();
    assert_eq!(fake.tried.lock().clone(), vec![(2, "fresh".to_owned())]);
}

/// R-7 / R-9 / R-11: custody is taken from the worktree, `include_dirty` reaches git,
/// Discard removes branches and refs.
#[tokio::test]
async fn create_and_discard_use_the_worktree_and_the_repo_cleanup() {
    let fake = Fake::with_values(&[100.0, 100.0, 100.0]);
    let mut h = harness(fake.clone(), AutoresearchConfig::default()).await;
    let mut sp = spec(&h, "t1", 10);
    sp.include_dirty = true;
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
    wait_state(&h.studies.store, "t1", |s| *s == StudyState::Running).await;
    assert_eq!(*fake.include_dirty.lock(), Some(true));
    // 140-P: the study's base is the snapshot the baseline measured, not the resolved HEAD.
    let s = h.studies.store.get_study("t1").await.unwrap().unwrap();
    assert_eq!(s.base_sha.as_deref(), Some("snapshot"));
    assert_eq!(s.incumbent_sha.as_deref(), Some("snapshot"));
    let worktree = h.studies.root.join("t1/worktree");
    assert_eq!(fake.custody_from.lock().clone(), vec![worktree]);
    let c = |op| Control {
        slug: "t1".into(),
        op,
        by: By::User,
    };
    control(&mut h.studies, c(ControlOp::Pause)).await.unwrap();
    control(&mut h.studies, c(ControlOp::Discard))
        .await
        .unwrap();
    let ev = h
        .studies
        .store
        .list_events("t1", Some("discarded"), 1)
        .await
        .unwrap();
    assert_eq!(ev[0].payload["removed"][0], "autoresearch/t1");
}

/// R-25: `Confirm { more: 0, incumbent_sample: true }` measures the incumbent once and
/// re-judges the same candidate sample — no extra candidate sample is spent.
#[tokio::test]
async fn incumbent_first_confirm_spends_no_candidate_sample() {
    let fake = Fake::with_values(&[100.0, 100.0, 100.0, 95.0, 100.0]);
    let mut h = harness(fake.clone(), AutoresearchConfig::default()).await;
    let session = started(&mut h, "t1", 10).await;
    begin(&mut h.studies, b(session)).await.unwrap();
    *fake.confirm_once.lock() = Some(GateStep::Confirm {
        more: 0,
        incumbent_sample: true,
    });
    fake.decide(Verdict::Improved, 5.0, 3.0);
    let rep = run(&mut h.studies, r(session, "x")).await.unwrap();
    assert_eq!(rep.verdict, Verdict::Improved);
    let roles: Vec<SampleRole> = rep.samples.iter().filter_map(|s| s.role).collect();
    assert_eq!(roles, vec![SampleRole::Candidate, SampleRole::Incumbent]);
    let input = fake.last_input.lock().clone().unwrap();
    assert_eq!((input.candidate.len(), input.incumbent.len()), (1, 4));
}

/// R-85: Stop now while preparing cancels the baseline with an explicit reason.
#[tokio::test]
async fn stop_now_while_preparing_cancels_the_baseline() {
    // Lost sample (cancel mid-bench) and the slot's wait error (cancel while waiting
    // for the load precondition, 140-P live) both end with the R-85 reason.
    for wait_err in [false, true] {
        stop_now_while_preparing(wait_err).await;
    }
}

async fn stop_now_while_preparing(wait_err: bool) {
    let fake = Fake::with_values(&[100.0, 100.0, 100.0]);
    let mut h = harness(fake.clone(), AutoresearchConfig::default()).await;
    fake.hang.store(true, std::sync::atomic::Ordering::SeqCst);
    fake.hang_errs
        .store(wait_err, std::sync::atomic::Ordering::SeqCst);
    let sp = spec(&h, "t1", 10);
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
    for _ in 0..200 {
        if *fake.samples_taken.lock() > 0 {
            break;
        }
        tokio::time::sleep(Duration::from_millis(10)).await;
    }
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
    assert!(matches!(
        i.state,
        StudyState::Preparing | StudyState::Failed { .. }
    ));
    let s = wait_state(&h.studies.store, "t1", |s| {
        matches!(s, StudyState::Failed { .. })
    })
    .await;
    let StudyState::Failed { reason } = s.state else {
        unreachable!()
    };
    assert!(
        reason.contains("stopped by you while the baseline"),
        "{reason}"
    );
}

/// R-98: a file-path fallback from finalize is not a document id.
#[tokio::test]
async fn report_doc_is_none_when_finalize_has_no_document() {
    let fake = Fake::with_values(&[100.0, 100.0, 100.0, 95.0]);
    *fake.doc.lock() = None;
    let mut h = harness(fake.clone(), AutoresearchConfig::default()).await;
    let session = started(&mut h, "t1", 1).await;
    begin(&mut h.studies, b(session)).await.unwrap();
    fake.decide(Verdict::Improved, 5.0, 4.0);
    run(&mut h.studies, r(session, "x")).await.unwrap();
    log(&mut h.studies, l(session, Decision::Auto))
        .await
        .unwrap();
    binding::on_turn_finished(&mut h.studies, session, "end_turn")
        .await
        .unwrap();
    let s = wait_state(&h.studies.store, "t1", |s| *s == StudyState::Finished).await;
    assert_eq!(s.report_doc, None);
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
    assert!(
        result.contains("Report: [/notes/doc](/notes/doc)"),
        "{result}"
    );
}
