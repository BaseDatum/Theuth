//! `decide_many` end to end against the real GLiNER2.5-Decide checkpoint in
//! `~/.theuth/models/decision-gliner/` (`$THEUTH_MODELS_DIR` moves the models dir). Skips with
//! a printed reason when any of `model_files(Kind::Gliner)` is missing. A smoke, not an eval:
//! 40 synthetic emails over 5 folders, shape asserted, obvious rows ≥ 80 % right; and the
//! 57 §2.1 reclaim after the tool's own `Release`.

use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::Duration;

use decision::engine::model_files;
use decision::log::Log;
use decision::tool::{Decide, DecideManyArgs, DecideManyTool, Q, RowOut};
use decision::{
    Checkpoint, DecisionConfig, Engine, FamilyGate, Kind, LoadedNow, TOOL_FAMILY, actor_name,
    process_memory,
};
use kameo::actor::Spawn;
use tools::{Tool, ToolCx};

/// The models dir and the gliner checkpoint under it, or `None` with a printed reason.
fn dirs() -> Option<(PathBuf, PathBuf)> {
    let models = std::env::var_os("THEUTH_MODELS_DIR")
        .map(PathBuf::from)
        .or_else(|| std::env::var_os("HOME").map(|h| PathBuf::from(h).join(".theuth/models")))?;
    let dir = models.join("decision-gliner");
    let missing: Vec<_> = model_files(Kind::Gliner)
        .iter()
        .filter(|f| !dir.join(f).is_file())
        .collect();
    if missing.is_empty() {
        Some((models, dir))
    } else {
        eprintln!(
            "SKIP: {} lacks {missing:?} — scripts/fetch-models.sh downloads them",
            dir.display()
        );
        None
    }
}

/// The `tool` family on a uniquely named gliner checkpoint (tests in one binary share the
/// kameo registry), its engine registered, and the tool over the same config.
fn rig(name: &str, models: &Path, dir: PathBuf, tmp: &Path) -> Arc<Decide> {
    let mut cfg = DecisionConfig::default();
    let mut entry = Checkpoint::from(dir.clone());
    entry.kind = Kind::Gliner;
    cfg.checkpoints.insert(name.to_owned(), entry);
    let mut gate = FamilyGate::tool();
    gate.checkpoint = Some(name.to_owned());
    gate.budget = Duration::from_secs(120);
    cfg.families.insert(TOOL_FAMILY.to_owned(), gate);
    let engine = Engine::spawn(
        Engine::new(
            cfg.clone(),
            models.join("decision"),
            Log::new(tmp.join("decisions")),
        )
        .with_checkpoint(name, dir),
    );
    engine.register(actor_name(name)).unwrap();
    Arc::new(Decide::new(cfg, &tmp.join("decisions")))
}

fn cx(dir: &Path) -> ToolCx {
    ToolCx {
        session: runtime::SessionId::new(),
        call_id: "c1".into(),
        quiet: false,
        resources: Arc::new(tools::SessionResources::durable(dir)),
        cancel: tools::CancellationToken::new(),
        promote_after: None,
    }
}

const FOLDERS: [(&str, &str); 5] = [
    ("inv", "invoices and receipts"),
    ("rec", "recruiting and candidates"),
    ("news", "newsletters and announcements"),
    ("me", "personal"),
    ("ops", "urgent operations"),
];

/// 8 obviously-labelled emails per folder: (folder id, sender, subject, body).
fn emails() -> Vec<(&'static str, String, String, String)> {
    let mut out = Vec::new();
    for i in 0..8 {
        out.push((
            "inv",
            "billing@acme-supplies.com".to_owned(),
            format!("Invoice #{} — payment due in 30 days", 4400 + i),
            format!(
                "Please find attached invoice #{} for ${}.00. Payment terms net 30; remit to the account on the receipt.",
                4400 + i,
                1200 + i * 37
            ),
        ));
        out.push((
            "rec",
            "talent@company.com".to_owned(),
            format!("Interview feedback: candidate {} for senior backend role", ["Maria Lopez", "Dev Patel", "Ann Chen", "Luca Rossi", "Sam Okafor", "Ida Berg", "Tom Hale", "Yuki Sato"][i]),
            "Panel notes from today's onsite. Hiring committee meets Thursday; please add your scorecard for the candidate before then. Recruiter: Jen.".to_owned(),
        ));
        out.push((
            "news",
            "digest@weekly-updates.io".to_owned(),
            format!("Weekly digest #{}: {} new posts and the community roundup", 120 + i, 3 + i),
            "This week's newsletter: product announcements, blog highlights, upcoming webinars. You are receiving this because you subscribed. Unsubscribe anytime.".to_owned(),
        ));
        out.push((
            "me",
            ["mom@family.net", "dad@family.net", "sister@family.net", "oldfriend@gmail.com"][i % 4].to_owned(),
            ["Dinner on Saturday?", "Photos from the hike", "Grandma's birthday", "Weekend plans"][i % 4].to_owned(),
            "Hey! Are we still on for the weekend? Mom says hi and wants to know if you are bringing dessert. Love you.".to_owned(),
        ));
        out.push((
            "ops",
            "alerts@pagerduty.com".to_owned(),
            format!("[SEV1] production database cpu at {}% — failover initiated", 95 + i % 5),
            "Incident open: primary db replica lag rising, error rate 12%, on-call paged. Join the incident bridge now. Rollback of deploy 7731 in progress.".to_owned(),
        ));
    }
    out
}

/// 40 synthetic emails over 5 folders through `decide_many`: one row per email in order, the
/// row shape, probabilities summing to 1 over the five ids plus `none`, and the obvious labels
/// right ≥ 80 % of the time (the actual number is printed; this is a smoke, not an eval).
#[tokio::test]
async fn forty_emails_over_five_folders() {
    let Some((models, dir)) = dirs() else {
        return;
    };
    let tmp = tempfile::tempdir().unwrap();
    let decide = rig("decide-e2e", &models, dir, tmp.path());
    let mails = emails();
    let questions: Vec<Q> = mails
        .iter()
        .map(|(_, from, subject, body)| Q::Choice {
            instructions: "Which folder does this email belong in?".into(),
            labels: FOLDERS.iter().map(|(_, l)| (*l).to_owned()).collect(),
            ids: Some(FOLDERS.iter().map(|(id, _)| (*id).to_owned()).collect()),
            state: format!("{from}\n{subject}\n{body}"),
        })
        .collect();
    let t = std::time::Instant::now();
    let out = DecideManyTool(Arc::clone(&decide))
        .call(
            DecideManyArgs {
                questions,
                checkpoint: None,
                family: None,
                capture: None,
                budget_ms: None,
            },
            cx(tmp.path()),
        )
        .await
        .unwrap();
    let wall = t.elapsed();
    assert!(!out.is_error, "{}", out.content);
    let rows: Vec<RowOut> = serde_json::from_value(out.structured.unwrap()).unwrap();
    assert_eq!(rows.len(), 40);
    let mut right = 0usize;
    let mut confidences = Vec::new();
    for ((want, _, subject, _), row) in mails.iter().zip(&rows) {
        assert!(row.error.is_none(), "{subject}: {row:?}");
        let mut keys: Vec<&str> = row.probs.keys().map(String::as_str).collect();
        keys.sort_unstable();
        assert_eq!(keys, ["inv", "me", "news", "none", "ops", "rec"], "{row:?}");
        let sum: f32 = row.probs.values().sum();
        assert!((sum - 1.0).abs() < 1e-3, "{row:?}");
        assert!(!row.calibrated && !row.log_id.is_empty());
        assert!(row.confidence >= 0.0 && row.confidence <= 1.0);
        confidences.push(row.confidence);
        if row.choice.as_deref() == Some(*want) {
            right += 1;
        } else {
            eprintln!("miss: {subject:?} → {:?} (want {want})", row.choice);
        }
    }
    confidences.sort_by(f32::total_cmp);
    eprintln!(
        "decide_many: 40 emails in {wall:?} ({:.0} ms/row); {right}/40 right; confidence min {:.2} p50 {:.2} max {:.2}; {}",
        wall.as_secs_f64() * 1e3 / 40.0,
        confidences[0],
        confidences[20],
        confidences[39],
        out.content
    );
    assert!(right >= 32, "{right}/40 obvious emails right (< 80 %)");
}

/// 57 §2.1 item 3 through the tool (mirror of `tests/gliner.rs::release_reclaims_the_checkpoint`):
/// nothing resident before, the call loads ~1.9 GB, and because no other family runs on the
/// checkpoint the tool releases it as it returns — footprint back within 50 MB of "before".
#[tokio::test]
async fn decide_many_releases_the_checkpoint_it_loaded() {
    let Some((models, dir)) = dirs() else {
        return;
    };
    let tmp = tempfile::tempdir().unwrap();
    let decide = rig("decide-release", &models, dir, tmp.path());
    let engine = kameo::actor::ActorRef::<Engine>::lookup(actor_name("decide-release").as_str())
        .unwrap()
        .unwrap();
    let mb = |b: u64| b / 1_000_000;
    let mut rows = Vec::new();
    for cycle in 1..=2 {
        assert!(engine.ask(LoadedNow).await.unwrap().is_empty());
        let before = process_memory().footprint;
        let peak = Arc::new(parking_lot::Mutex::new(0u64));
        let sampler = {
            let peak = Arc::clone(&peak);
            tokio::spawn(async move {
                loop {
                    let fp = process_memory().footprint;
                    {
                        let mut p = peak.lock();
                        *p = (*p).max(fp);
                    }
                    tokio::time::sleep(Duration::from_millis(50)).await;
                }
            })
        };
        let out = DecideManyTool(Arc::clone(&decide))
            .call(
                DecideManyArgs {
                    questions: vec![
                        Q::Bool {
                            statement: "the message is an invoice".into(),
                            state: "Invoice #12 attached, due in 30 days".into(),
                        };
                        3
                    ],
                    checkpoint: None,
                    family: None,
                    capture: None,
                    budget_ms: None,
                },
                cx(tmp.path()),
            )
            .await
            .unwrap();
        sampler.abort();
        assert!(!out.is_error, "{}", out.content);
        let loaded = *peak.lock();
        assert!(
            engine.ask(LoadedNow).await.unwrap().is_empty(),
            "released by the tool"
        );
        let t = std::time::Instant::now();
        let mut released = process_memory().footprint;
        while released > before + 50_000_000 && t.elapsed() < Duration::from_secs(15) {
            tokio::time::sleep(Duration::from_millis(250)).await;
            released = process_memory().footprint;
        }
        eprintln!(
            "cycle {cycle}: footprint before {} MB, peak during the call {} MB, after {} MB ({:?} after return)",
            mb(before),
            mb(loaded),
            mb(released),
            t.elapsed()
        );
        rows.push((before, loaded, released));
    }
    let (before, loaded, released) = rows[1];
    assert!(
        loaded > before + 1_000_000_000,
        "the load did not show: {rows:?}"
    );
    assert!(
        released <= before + 50_000_000,
        "released {} MB > before {} MB + 50 MB",
        mb(released),
        mb(before)
    );
}
