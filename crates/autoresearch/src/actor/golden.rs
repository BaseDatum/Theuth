//! 159 §5.12 item 1: the 140 ledger a scripted study leaves behind, frozen before the
//! variants work. `UPDATE_GOLDEN=1` rewrites `fixtures/golden/ledger-140.json`.

use std::collections::BTreeMap;

use sqlx::{Column, Row, TypeInfo, ValueRef};

use super::*;

/// Column lists as of migration 0002, verbatim from `migrations/0001_init.sql` and
/// `0002_sample_detail.sql`, so later migrations never move this golden.
const TABLES: &[(&str, &str, &str)] = &[
    (
        "studies",
        "slug, state, state_reason, spec, repo, base_sha, branch, worktree, schedule, session, origin, \
         incumbent_sha, incumbent_mean, sigma, segment, playbook, created_at, started_at, paused_ms, \
         finished_at, stop_reason, report_doc, created_by",
        "slug",
    ),
    (
        "segments",
        "slug, segment, opened_at, opened_by, reason, custody, baseline_mean, baseline_n",
        "slug, segment",
    ),
    (
        "experiments",
        "slug, n, wave, slot, segment, parent_sha, state, verdict, idea, category, hypothesis, \
         description, learned, next, decision, justification, deviations, delta, delta_pct, z, \
         n_samples, metric, commit_sha, patch_path, patch_hash, review, line, cost_nanos, started_at, \
         finished_at",
        "slug, n",
    ),
    (
        "samples",
        "id, slug, n, k, tree_sha, role, value, secondaries, cases, asi, exit, wall_ms, load, noisy, \
         lost, log_path, at, slot_holders, failure",
        "id",
    ),
    (
        "ideas",
        "id, slug, text, category, source, priority, status, tried_in, at",
        "id",
    ),
    ("steers", "id, slug, text, by, at, seen_in", "id"),
    ("events", "id, slug, kind, payload, at", "id"),
];

/// TEXT columns holding JSON: parsed so nested keys can be masked.
const JSON_COLS: &[&str] = &[
    "spec",
    "payload",
    "verdict",
    "secondaries",
    "cases",
    "asi",
    "deviations",
    "tried_in",
    "slot_holders",
    "stop_reason",
    "review",
    "decision",
    "state_reason",
    "custody",
];

/// Keys whose values vary run to run (clocks, ids, spend, the machine).
const MASKED: &[&str] = &[
    "at",
    "session",
    "origin",
    "paused_ms",
    "wall_ms",
    "cost_nanos",
    "spend_usd",
    "duration_ms",
    "elapsed_ms",
    "load",
    "slot_holders",
    "token",
];

fn masked_key(k: &str) -> bool {
    MASKED.contains(&k) || k.ends_with("_at") || k == "id"
}

fn mask(v: &mut serde_json::Value) {
    match v {
        serde_json::Value::Object(m) => {
            for (k, v) in m.iter_mut() {
                if masked_key(k) {
                    *v = serde_json::Value::String("<masked>".into());
                } else {
                    mask(v);
                }
            }
        }
        serde_json::Value::Array(a) => a.iter_mut().for_each(mask),
        _ => {}
    }
}

fn cell(row: &sqlx::sqlite::SqliteRow, i: usize) -> serde_json::Value {
    let raw = row.try_get_raw(i).unwrap();
    if raw.is_null() {
        return serde_json::Value::Null;
    }
    let name = row.column(i).name();
    match raw.type_info().name() {
        "INTEGER" => row.get::<i64, _>(i).into(),
        "REAL" => row.get::<f64, _>(i).into(),
        _ => {
            let s: String = row.get(i);
            if JSON_COLS.contains(&name) {
                serde_json::from_str(&s).unwrap_or(serde_json::Value::String(s))
            } else {
                serde_json::Value::String(s)
            }
        }
    }
}

async fn dump(store: &Store) -> BTreeMap<&'static str, Vec<serde_json::Value>> {
    let mut out = BTreeMap::new();
    for (table, cols, pk) in TABLES {
        let rows = sqlx::query(sqlx::AssertSqlSafe(format!(
            "SELECT {cols} FROM {table} ORDER BY {pk}"
        )))
        .fetch_all(store.pool())
        .await
        .unwrap();
        let rows: Vec<serde_json::Value> = rows
            .iter()
            .enumerate()
            .map(|(ordinal, row)| {
                let mut m = serde_json::Map::new();
                for i in 0..row.columns().len() {
                    let name = row.column(i).name();
                    let mut v = cell(row, i);
                    if name == "id" {
                        v = (ordinal + 1).into();
                    } else if masked_key(name) {
                        v = serde_json::Value::String("<masked>".into());
                    } else {
                        mask(&mut v);
                    }
                    m.insert(name.to_owned(), v);
                }
                serde_json::Value::Object(m)
            })
            .collect();
        out.insert(*table, rows);
    }
    out
}

async fn wait_event(store: &Store, kind: &str) {
    for _ in 0..200 {
        if !store
            .list_events("t1", Some(kind), 1)
            .await
            .unwrap()
            .is_empty()
        {
            return;
        }
        tokio::time::sleep(Duration::from_millis(20)).await;
    }
    panic!("no `{kind}` event");
}

/// Pushes exactly what the next step consumes; `drained` proves the count.
fn feed(fake: &Fake, v: &[f64]) {
    fake.values.lock().extend(v.iter().copied());
}

fn drained(fake: &Fake, step: &str) {
    assert!(fake.values.lock().is_empty(), "values left after {step}");
}

/// The script (lower-is-better `time_ms`, `bench.repeats` 3, holdout every keep; the gate
/// is scripted so it never confirms and σ stays 0 — no regime segment):
///
/// - prepare: baseline 100 ×3, base holdout 500 ×3; one user idea.
/// - #1 no gain (100) → discarded
/// - #2 improved (90) → kept `keep0002`; holdout 450 ×3
/// - #3 worse (95) → discarded
/// - #4 bench fails to spawn (lost sample), then an empty queue → crash → crashed
/// - #5 improved (70) but the reviewer says SUSPICIOUS → reverted, `rejected_by_review`
/// - user opens segment 1 ("new machine"): baseline 90 ×3 on the incumbent; one steer
/// - #6 no gain (90) → discarded (the steer is seen here)
/// - #7 improved (85) → kept `keep0007`; holdout 425 ×3
/// - #8 worse (88) → discarded
/// - #9 improved (84) but the model logs `discard` → discarded
/// - #10 no gain (85) → discarded
/// - #11 no gain (85) → discarded
/// - #12 improved (80) → kept `keep0012`; holdout 400 ×3
#[tokio::test]
async fn golden_ledger_140() {
    let fake = Fake::with_values(&[100.0, 100.0, 100.0, 500.0, 500.0, 500.0]);
    let mut h = harness(fake.clone(), AutoresearchConfig::default()).await;
    let mut sp = spec(&h, "t1", 50);
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
    drained(&fake, "prepare");
    let session = SessionId::new();
    binding::on_fired(&mut h.studies, "autoresearch-t1", session)
        .await
        .unwrap();
    idea_op(
        &mut h,
        IdeaChange::Add {
            text: "unroll the inner loop".into(),
            category: "loop".into(),
            priority: None,
        },
    )
    .await;

    let no_gain = |fake: &Fake| fake.decide(Verdict::NoGain, 0.0, 0.2);
    let worse = |fake: &Fake| fake.decide(Verdict::Worse, -5.0, -3.0);

    // #1 no gain
    begin(&mut h.studies, b(session)).await.unwrap();
    no_gain(&fake);
    feed(&fake, &[100.0]);
    run(&mut h.studies, r(session, "hoist the bounds check"))
        .await
        .unwrap();
    let lr = log(&mut h.studies, l(session, Decision::Auto))
        .await
        .unwrap();
    assert_eq!(lr.applied, ExperimentState::Discarded);

    // #2 keep + holdout
    begin(&mut h.studies, b(session)).await.unwrap();
    fake.decide(Verdict::Improved, 10.0, 4.0);
    feed(&fake, &[90.0, 450.0, 450.0, 450.0]);
    run(&mut h.studies, r(session, "skip the copy"))
        .await
        .unwrap();
    let lr = log(&mut h.studies, l(session, Decision::Auto))
        .await
        .unwrap();
    assert_eq!(lr.applied, ExperimentState::Kept);
    assert_eq!(lr.commit.as_deref(), Some("keep0002"));
    drained(&fake, "#2");

    // #3 worse
    begin(&mut h.studies, b(session)).await.unwrap();
    worse(&fake);
    feed(&fake, &[95.0]);
    run(&mut h.studies, r(session, "bigger buffer"))
        .await
        .unwrap();
    let lr = log(&mut h.studies, l(session, Decision::Auto))
        .await
        .unwrap();
    assert_eq!(lr.applied, ExperimentState::Discarded);

    // #4 lost sample, then crash
    begin(&mut h.studies, b(session)).await.unwrap();
    no_gain(&fake);
    fake.fail_next
        .store(true, std::sync::atomic::Ordering::SeqCst);
    let e = run(&mut h.studies, r(session, "unsafe fast path"))
        .await
        .unwrap_err()
        .to_string();
    assert!(e.contains("could not spawn"), "{e}");
    let rep = run(&mut h.studies, r(session, "unsafe fast path"))
        .await
        .unwrap();
    assert_eq!(rep.verdict, Verdict::Crash);
    let lr = log(&mut h.studies, l(session, Decision::Auto))
        .await
        .unwrap();
    assert_eq!(lr.applied, ExperimentState::Crashed);

    // #5 review revert
    begin(&mut h.studies, b(session)).await.unwrap();
    fake.decide(Verdict::Improved, 20.0, 6.0);
    *fake.review.lock() = Some(ReviewVerdict::Flagged {
        reason: "caches the benchmark input".into(),
    });
    feed(&fake, &[70.0]);
    run(&mut h.studies, r(session, "memoise the input"))
        .await
        .unwrap();
    let lr = log(&mut h.studies, l(session, Decision::Auto))
        .await
        .unwrap();
    assert_eq!(lr.applied, ExperimentState::RejectedByReview);
    *fake.review.lock() = None;
    drained(&fake, "#5");

    // segment 1: re-baseline the incumbent, then a steer
    feed(&fake, &[90.0, 90.0, 90.0]);
    control(
        &mut h.studies,
        Control {
            slug: "t1".into(),
            op: ControlOp::Segment {
                reason: "new machine".into(),
            },
            by: By::User,
        },
    )
    .await
    .unwrap();
    wait_event(&h.studies.store, "segment").await;
    drained(&fake, "segment");
    assert_eq!(
        h.studies
            .store
            .get_study("t1")
            .await
            .unwrap()
            .unwrap()
            .segment,
        1
    );
    steer(
        &mut h.studies,
        SteerMsg {
            slug: "t1".into(),
            text: "try SIMD".into(),
            by: By::User,
        },
    )
    .await
    .unwrap();

    // #6 no gain
    begin(&mut h.studies, b(session)).await.unwrap();
    no_gain(&fake);
    feed(&fake, &[90.0]);
    run(&mut h.studies, r(session, "reorder fields"))
        .await
        .unwrap();
    let lr = log(&mut h.studies, l(session, Decision::Auto))
        .await
        .unwrap();
    assert_eq!(lr.applied, ExperimentState::Discarded);

    // #7 keep + holdout
    begin(&mut h.studies, b(session)).await.unwrap();
    fake.decide(Verdict::Improved, 5.0, 3.0);
    feed(&fake, &[85.0, 425.0, 425.0, 425.0]);
    run(&mut h.studies, r(session, "SIMD the hot loop"))
        .await
        .unwrap();
    let lr = log(&mut h.studies, l(session, Decision::Auto))
        .await
        .unwrap();
    assert_eq!(lr.applied, ExperimentState::Kept);
    assert_eq!(lr.commit.as_deref(), Some("keep0007"));
    drained(&fake, "#7");

    // #8 worse
    begin(&mut h.studies, b(session)).await.unwrap();
    worse(&fake);
    feed(&fake, &[88.0]);
    run(&mut h.studies, r(session, "prefetch")).await.unwrap();
    let lr = log(&mut h.studies, l(session, Decision::Auto))
        .await
        .unwrap();
    assert_eq!(lr.applied, ExperimentState::Discarded);

    // #9 improved, but the model distrusts it
    begin(&mut h.studies, b(session)).await.unwrap();
    fake.decide(Verdict::Improved, 1.0, 2.5);
    feed(&fake, &[84.0]);
    run(&mut h.studies, r(session, "drop the assert"))
        .await
        .unwrap();
    let lr = log(&mut h.studies, l(session, Decision::Discard))
        .await
        .unwrap();
    assert_eq!(lr.applied, ExperimentState::Discarded);

    // #10, #11 no gain
    for idea in ["inline the getter", "shrink the enum"] {
        begin(&mut h.studies, b(session)).await.unwrap();
        no_gain(&fake);
        feed(&fake, &[85.0]);
        run(&mut h.studies, r(session, idea)).await.unwrap();
        let lr = log(&mut h.studies, l(session, Decision::Auto))
            .await
            .unwrap();
        assert_eq!(lr.applied, ExperimentState::Discarded);
    }

    // #12 keep + holdout
    begin(&mut h.studies, b(session)).await.unwrap();
    fake.decide(Verdict::Improved, 5.0, 3.0);
    feed(&fake, &[80.0, 400.0, 400.0, 400.0]);
    run(&mut h.studies, r(session, "batch the writes"))
        .await
        .unwrap();
    let lr = log(&mut h.studies, l(session, Decision::Auto))
        .await
        .unwrap();
    assert_eq!(lr.applied, ExperimentState::Kept);
    assert_eq!(lr.commit.as_deref(), Some("keep0012"));
    drained(&fake, "#12");

    let ledger = dump(&h.studies.store).await;
    let mut got = serde_json::to_string_pretty(&ledger).unwrap();
    got.push('\n');
    let dir = h._dir.path();
    for p in [dir.to_path_buf(), dir.canonicalize().unwrap()] {
        got = got.replace(&p.display().to_string(), "<dir>");
    }

    let golden = Path::new(env!("CARGO_MANIFEST_DIR")).join("fixtures/golden/ledger-140.json");
    if std::env::var_os("UPDATE_GOLDEN").is_some() {
        std::fs::write(&golden, &got).expect("write golden");
        return;
    }
    let want = std::fs::read_to_string(&golden)
        .unwrap_or_else(|e| panic!("{}: {e} (run with UPDATE_GOLDEN=1)", golden.display()));
    assert!(
        want == got,
        "ledger differs from {}; rerun with UPDATE_GOLDEN=1 to re-record:\n{}",
        golden.display(),
        want.lines()
            .zip(got.lines())
            .enumerate()
            .filter(|(_, (a, b))| a != b)
            .map(|(i, (a, b))| format!("line {}:\n- {a}\n+ {b}", i + 1))
            .take(20)
            .collect::<Vec<_>>()
            .join("\n")
    );
}
