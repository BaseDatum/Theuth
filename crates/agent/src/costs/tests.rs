//! 160 cube / calls tests over a real store built through its API.

use std::collections::{BTreeMap, BTreeSet, HashMap};
use std::path::Path;
use std::sync::Arc;

use runtime::{SessionId, TurnId};

use super::cube::Cube;
use super::place::place;
use super::*;
use crate::cost::{CallKind, CallStatus, CostObservation, MeterUpdate, Money, TokenTotals};
use crate::store::Store;
use crate::store::accounting::{MeterInput, TurnInput};

const HAIKU: &str = "anthropic/claude-haiku-4-5";

struct Rig {
    store: Store,
    catalog: Arc<models::Catalog>,
    prices: Prices,
    _dir: crate::TestDir,
}

async fn rig() -> Rig {
    let dir = crate::TestDir::new("costs");
    let store = Store::open(&dir.0.join("sessions.sqlite"), None)
        .await
        .unwrap();
    let catalog = Arc::new(models::Catalog::load(Path::new("/unused")));
    let prices = Prices::new(&catalog);
    Rig {
        store,
        catalog,
        prices,
        _dir: dir,
    }
}

fn usd(v: f64) -> Money {
    Money::from_f64(v, "USD").unwrap()
}

async fn call(
    store: &Store,
    owner: SessionId,
    kind: CallKind,
    model: &str,
    status: CallStatus,
    obs: CostObservation,
) {
    let provider = model.split('/').next().unwrap();
    let c = store
        .reserve_call(owner, "r", 0, None, kind, Some(provider), Some(model))
        .await
        .unwrap();
    store.mark_dispatched(&c).await.unwrap();
    store.settle_call(&c, status, obs).await.unwrap();
}

fn tokens(i: u32, o: u32, cr: u32, cw: u32) -> CostObservation {
    CostObservation {
        input_tokens: Some(i),
        output_tokens: Some(o),
        cache_read_tokens: Some(cr),
        cache_write_tokens: Some(cw),
        ..Default::default()
    }
}

async fn meter(store: &Store, owner: SessionId, ext: &str, total: f64) {
    store
        .apply_meter(
            owner,
            MeterUpdate {
                origin: "omp".into(),
                external_session: ext.into(),
                cumulative: usd(total),
                confirmed_new_session: true,
                report_id: None,
            },
        )
        .await
        .unwrap();
}

async fn turn(store: &Store, owner: SessionId) {
    store
        .record_remote_turn(
            owner,
            TurnId::new(),
            "omp",
            Some("anthropic/claude-haiku-4-5:high"),
            &TokenTotals {
                input: 1000,
                output: 100,
                cache_read: 600,
                cache_write: 0,
                unknown_calls: 0,
            },
            false,
        )
        .await
        .unwrap();
}

/// A user root with a native child and an omp child, a desk root, and
/// every money class: known, estimated, repriced, unpriced, failed.
async fn fixture(r: &Rig) -> (SessionId, SessionId, SessionId) {
    let s = &r.store;
    let (me, sub, omp, desk) = (
        SessionId::new(),
        SessionId::new(),
        SessionId::new(),
        SessionId::new(),
    );
    s.create_session(me, "m", None, None, Some("P"))
        .await
        .unwrap();
    s.set_title(me, "my chat").await.unwrap();
    s.create_session(sub, "m", Some(me), Some("sub"), None)
        .await
        .unwrap();
    s.create_session(omp, "m", Some(me), Some("omp"), None)
        .await
        .unwrap();
    s.create_session(desk, "m", None, None, None).await.unwrap();
    s.set_desk(desk, Some("builder")).await.unwrap();
    let known = CostObservation {
        returned: Some(usd(0.10)),
        ..tokens(1000, 10, 800, 0)
    };
    call(s, me, CallKind::Round, HAIKU, CallStatus::Settled, known).await;
    let est = CostObservation {
        estimated: Some(usd(0.02)),
        ..tokens(500, 50, 0, 100)
    };
    call(s, sub, CallKind::Endless, HAIKU, CallStatus::Settled, est).await;
    // Tokens, no money: repriced at read time.
    call(
        s,
        sub,
        CallKind::Title,
        HAIKU,
        CallStatus::Settled,
        tokens(2000, 300, 1000, 500),
    )
    .await;
    // Tokens, unknown model: unpriced.
    call(
        s,
        desk,
        CallKind::Round,
        "nowhere/none-1",
        CallStatus::Settled,
        tokens(10, 1, 0, 0),
    )
    .await;
    call(
        s,
        desk,
        CallKind::Round,
        HAIKU,
        CallStatus::Failed,
        CostObservation::default(),
    )
    .await;
    meter(s, omp, "e1", 0.0).await;
    turn(s, omp).await;
    meter(s, omp, "e1", 0.20).await;
    meter(s, omp, "e1", 0.35).await;
    (me, omp, desk)
}

type Flat = BTreeMap<(u32, String, String, String, String, String, u8), [u64; 10]>;

/// A cube answer keyed by names, so two epochs compare.
fn flat(c: &CostCube, roots: &HashMap<u32, CostRoot>) -> Flat {
    let x = &c.cells;
    (0..x.hour.len())
        .map(|i| {
            (
                (
                    x.hour[i],
                    roots[&x.root[i]].id.clone(),
                    c.models[x.model[i] as usize].id.clone(),
                    c.providers[x.provider[i] as usize].clone(),
                    c.kinds[x.kind[i] as usize].clone(),
                    c.vias[x.via[i] as usize].clone(),
                    x.basis[i],
                ),
                [
                    x.nanos[i],
                    u64::from(x.calls[i]),
                    u64::from(x.failed[i]),
                    u64::from(x.repriced[i]),
                    x.input[i],
                    x.output[i],
                    x.cache_read[i],
                    x.cache_write[i],
                    x.cache_saved_nanos[i],
                    x.approx_nanos[i],
                ],
            )
        })
        .collect()
}

fn roots_of(c: &CostCube) -> HashMap<u32, CostRoot> {
    c.roots.iter().map(|e| (e.ix, e.root.clone())).collect()
}

fn full_flat(c: &CostCube) -> Flat {
    assert!(c.full);
    flat(c, &roots_of(c))
}

/// A client: applies full answers and deltas like the UI does.
#[derive(Default)]
struct Client {
    epoch: String,
    rev: u64,
    roots: HashMap<u32, CostRoot>,
    cells: Flat,
}

impl Client {
    fn apply(&mut self, c: &CostCube) {
        if c.full {
            self.roots = roots_of(c);
            self.cells = flat(c, &self.roots);
        } else {
            for e in &c.roots {
                self.roots.insert(e.ix, e.root.clone());
            }
            let hours: BTreeSet<u32> = c.hours.iter().copied().collect();
            self.cells.retain(|k, _| !hours.contains(&k.0));
            self.cells.extend(flat(c, &self.roots));
        }
        self.epoch = c.epoch.clone();
        self.rev = c.rev;
    }

    fn cursor(&self) -> CostCubeCursor {
        CostCubeCursor {
            epoch: self.epoch.clone(),
            rev: self.rev,
        }
    }
}

fn total(f: &Flat, basis: Option<u8>) -> u64 {
    f.iter()
        .filter(|(k, _)| basis.is_none_or(|b| k.6 == b))
        .map(|(_, v)| v[0])
        .sum()
}

async fn sql_i64(store: &Store, q: &'static str) -> i64 {
    sqlx::query_scalar(q).fetch_one(store.pool()).await.unwrap()
}

/// §3.4: every leg sums to its SQL truth; repricing is the estimate
/// formula; cache savings and unpriced reasons land; owners follow D6.
#[tokio::test]
async fn legs_reconcile_with_the_ledger() {
    let r = rig().await;
    let (me, _, desk) = fixture(&r).await;
    let cube = Cube::build(&r.store, &r.catalog, &r.prices, "e")
        .await
        .unwrap();
    let a = cube.answer(None);
    let f = full_flat(&a);

    let native = sql_i64(
        &r.store,
        "SELECT SUM(COALESCE(returned_nanos, estimated_nanos, 0)) FROM model_calls WHERE status <> 'not_dispatched'",
    )
    .await as u64;
    let rate = r.prices.rate(HAIKU).unwrap();
    let repriced = Prices::estimate(rate, 2000, 300, 1000, 500);
    assert!(repriced > 0);
    assert_eq!(total(&f, Some(0)), 100_000_000, "known");
    assert_eq!(
        total(&f, Some(2)),
        20_000_000 + repriced,
        "estimated + repriced"
    );
    assert_eq!(total(&f, Some(3)), 0, "unpriced carries no money");
    assert_eq!(total(&f, Some(0)) + total(&f, Some(2)), native + repriced);
    let attributed = sql_i64(&r.store, "SELECT SUM(attributed_nanos) FROM remote_meters").await;
    assert_eq!(total(&f, Some(1)), attributed as u64, "reported");

    let calls: u64 = f.iter().filter(|(k, _)| k.6 != 1).map(|(_, v)| v[1]).sum();
    let rows = sql_i64(
        &r.store,
        "SELECT COUNT(*) FROM model_calls WHERE status <> 'not_dispatched'",
    )
    .await;
    assert_eq!(calls, rows as u64);
    let repriced_n: u64 = f.values().map(|v| v[3]).sum();
    assert_eq!(repriced_n, 1);
    let failed: u64 = f.values().map(|v| v[2]).sum();
    assert_eq!(failed, 1);
    // Turn tokens on the reported leg, model without the effort suffix.
    let rep: Vec<_> = f.iter().filter(|(k, _)| k.6 == 1).collect();
    assert!(
        rep.iter()
            .all(|(k, _)| k.2 == HAIKU && k.4 == "harness" && k.5 == "omp")
    );
    assert_eq!(rep.iter().map(|(_, v)| v[1]).sum::<u64>(), 1, "one turn");
    assert_eq!(rep.iter().map(|(_, v)| v[9]).sum::<u64>(), 0, "all exact");
    // cache_saved = cache_read × (input − cache_read rate).
    let saved: u64 = f.values().map(|v| v[8]).sum();
    let want = Prices::cache_saved(Some(rate), 800 + 1000 + 600);
    assert_eq!(saved, want);
    // Unpriced: the unknown model's settle and the failure.
    let reasons: Vec<(&str, u32)> = a
        .meta
        .unpriced
        .iter()
        .map(|r| (r.reason.as_str(), r.calls))
        .collect();
    assert_eq!(reasons.iter().map(|r| r.1).sum::<u32>(), 2, "{reasons:?}");
    assert_eq!(a.meta.price_table, r.prices.hash);
    // Owners and the model dimension.
    let roots = roots_of(&a);
    let owner = |id: SessionId| {
        roots
            .values()
            .find(|x| x.id == id.to_string())
            .map(|x| (x.owner_kind, x.owner.clone(), x.project.clone(), x.deleted))
            .unwrap()
    };
    assert_eq!(
        owner(me),
        (
            crate::SpendOwnerKind::User,
            "you".into(),
            Some("P".into()),
            false
        )
    );
    assert_eq!(owner(desk).1, "builder");
    let m = a.models.iter().find(|m| m.id == HAIKU).unwrap();
    assert_eq!(
        (m.name.as_str(), m.provider.as_str()),
        ("claude-haiku-4-5", "anthropic")
    );
}

/// 160 H1: the books survive a root delete; the cube shows the root as
/// deleted under its owner and the totals do not move.
#[tokio::test]
async fn root_delete_keeps_the_books_under_its_owner() {
    let r = rig().await;
    let (_, _, desk) = fixture(&r).await;
    let svc = CostService::new(r.store.clone(), r.catalog.clone(), "e".into());
    svc.step().await.unwrap();
    let before = full_flat(&svc.cube(None));
    r.store.delete_session(desk).await.unwrap();
    let tomb: (Option<String>, i64) =
        sqlx::query_as("SELECT desk, unattended FROM cost_root_tombstones WHERE root_id = ?")
            .bind(desk.to_string())
            .fetch_one(r.store.pool())
            .await
            .unwrap();
    assert_eq!(tomb, (Some("builder".into()), 0));
    svc.step().await.unwrap();
    let a = svc.cube(None);
    assert_eq!(full_flat(&a), before, "nothing erased");
    let root = a
        .roots
        .iter()
        .find(|e| e.root.id == desk.to_string())
        .unwrap();
    assert!(root.root.deleted);
    assert_eq!(root.root.title, "");
    assert_eq!(
        (root.root.owner_kind, root.root.owner.as_str()),
        (crate::SpendOwnerKind::Agent, "builder")
    );
    assert!(root.root.created_at_ms > 0);
}

/// Review 160-ENG #1: a meter with no turn model falls back to its owner's
/// `harness_model`; deleting the root keeps that model, not `unknown` (H1:
/// the books stay, breakdowns included). #3: an unchanged total adds no H2 row.
#[tokio::test]
async fn root_delete_keeps_a_meters_fallback_model() {
    let r = rig().await;
    let s = &r.store;
    let (root, child) = (SessionId::new(), SessionId::new());
    s.create_session(root, "m", None, None, None).await.unwrap();
    s.create_session(child, "m", Some(root), Some("omp"), None)
        .await
        .unwrap();
    sqlx::query("UPDATE sessions SET harness_model = 'anthropic/claude-fable-5-1' WHERE id = ?")
        .bind(child.to_string())
        .execute(s.pool())
        .await
        .unwrap();
    meter(s, child, "e9", 0.0).await;
    meter(s, child, "e9", 0.40).await;
    let reports = sql_i64(s, "SELECT COUNT(*) FROM remote_meter_reports").await;
    meter(s, child, "e9", 0.40).await;
    let svc = CostService::new(s.clone(), r.catalog.clone(), "e".into());
    svc.step().await.unwrap();
    let before = full_flat(&svc.cube(None));
    assert!(
        before
            .keys()
            .any(|k| k.2 == "anthropic/claude-fable-5-1" && k.6 == 1),
        "{before:?}"
    );
    s.delete_session(root).await.unwrap();
    svc.step().await.unwrap();
    assert_eq!(
        full_flat(&svc.cube(None)),
        before,
        "the model survives the delete"
    );
    assert_eq!(
        sql_i64(s, "SELECT COUNT(*) FROM remote_meter_reports").await,
        reports,
        "an unchanged total is not kept again"
    );
}

/// Review 160-ENG #4 / #5: a call reserved long before it settles, and a call
/// settling after its root was deleted, both reach the cube incrementally.
#[tokio::test]
async fn late_settles_reach_the_cube() {
    let r = rig().await;
    let s = &r.store;
    let (old, gone) = (SessionId::new(), SessionId::new());
    s.create_session(old, "m", None, None, None).await.unwrap();
    s.create_session(gone, "m", None, None, None).await.unwrap();
    let reserve = |owner| {
        s.reserve_call(
            owner,
            "r",
            0,
            None,
            CallKind::Round,
            Some("anthropic"),
            Some(HAIKU),
        )
    };
    let slow = reserve(old).await.unwrap();
    s.mark_dispatched(&slow).await.unwrap();
    let three_hours_ago = (chrono::Utc::now() - chrono::Duration::hours(3)).to_rfc3339();
    sqlx::query("UPDATE model_calls SET created_at = ? WHERE call_id = ?")
        .bind(three_hours_ago)
        .bind(&slow.0)
        .execute(s.pool())
        .await
        .unwrap();
    let orphan = reserve(gone).await.unwrap();
    s.mark_dispatched(&orphan).await.unwrap();
    let svc = CostService::new(s.clone(), r.catalog.clone(), "e".into());
    svc.step().await.unwrap();
    s.delete_session(gone).await.unwrap();
    svc.step().await.unwrap();
    let est = |v| CostObservation {
        estimated: Some(usd(v)),
        ..tokens(10, 1, 0, 0)
    };
    s.settle_call(&slow, CallStatus::Settled, est(0.25))
        .await
        .unwrap();
    s.settle_call(&orphan, CallStatus::Settled, est(0.5))
        .await
        .unwrap();
    svc.step().await.unwrap();
    let flat = full_flat(&svc.cube(None));
    assert_eq!(total(&flat, Some(2)), 750_000_000, "{flat:?}");
    let unpriced: u64 = flat
        .iter()
        .filter(|(k, _)| k.6 == 3)
        .map(|(_, v)| v[1])
        .sum();
    assert_eq!(unpriced, 0, "no in-flight cell left: {flat:?}");
}

/// §3.3: after a scripted mix of events the incrementally refreshed cube
/// equals a full rebuild, and a client fed only deltas holds the same cells.
#[tokio::test]
async fn incremental_equals_full_rebuild() {
    let r = rig().await;
    let (me, omp, desk) = fixture(&r).await;
    let s = &r.store;
    let mut cube = Cube::build(s, &r.catalog, &r.prices, "e").await.unwrap();
    let mut client = Client::default();
    client.apply(&cube.answer(None));
    let check = |cube: &Cube, client: &mut Client, step: &str| {
        let d = cube.answer(Some(&client.cursor()));
        assert!(!d.full, "{step}: a delta");
        client.apply(&d);
    };
    let mut steps: Vec<&str> = Vec::new();
    for step in [
        "settle", "fail", "meter", "turn", "retitle", "new root", "delete",
    ] {
        match step {
            "settle" => {
                call(
                    s,
                    me,
                    CallKind::Round,
                    HAIKU,
                    CallStatus::Settled,
                    CostObservation {
                        estimated: Some(usd(0.5)),
                        ..tokens(10, 10, 0, 0)
                    },
                )
                .await
            }
            "fail" => {
                call(
                    s,
                    omp,
                    CallKind::Round,
                    HAIKU,
                    CallStatus::Cancelled,
                    CostObservation::default(),
                )
                .await
            }
            "meter" => meter(s, omp, "e1", 0.75).await,
            "turn" => turn(s, omp).await,
            "retitle" => s.set_title(me, "renamed").await.unwrap(),
            "new root" => {
                let n = SessionId::new();
                s.create_session(n, "m", None, None, None).await.unwrap();
                call(
                    s,
                    n,
                    CallKind::Round,
                    HAIKU,
                    CallStatus::Settled,
                    tokens(5, 5, 0, 0),
                )
                .await;
            }
            _ => {
                s.delete_session(desk).await.unwrap();
            }
        }
        assert_eq!(
            cube.refresh(s, &r.catalog, &r.prices).await.unwrap(),
            Some(true),
            "{step}"
        );
        check(&cube, &mut client, step);
        steps.push(step);
        let full = Cube::build(s, &r.catalog, &r.prices, "e")
            .await
            .unwrap()
            .answer(None);
        let inc = cube.answer(None);
        assert_eq!(full_flat(&inc), full_flat(&full), "after {steps:?}");
        assert_eq!(client.cells, full_flat(&full), "client after {steps:?}");
        let names = |c: &CostCube| -> BTreeSet<(String, String, bool)> {
            c.roots
                .iter()
                .map(|e| (e.root.id.clone(), e.root.title.clone(), e.root.deleted))
                .collect()
        };
        assert_eq!(names(&inc), names(&full), "roots after {steps:?}");
        let client_roots: BTreeSet<_> = client
            .roots
            .values()
            .map(|x| (x.id.clone(), x.title.clone(), x.deleted))
            .collect();
        assert_eq!(client_roots, names(&full), "client roots after {steps:?}");
    }
    // Nothing new: the refresh reports no change and the rev stays.
    let rev = cube.rev;
    assert_eq!(
        cube.refresh(s, &r.catalog, &r.prices).await.unwrap(),
        Some(false)
    );
    assert_eq!(cube.rev, rev);
}

/// §3.2: empty delta when current, a delta after a change, full on an
/// epoch mismatch, a rev ahead of the cube, or one older than the log.
#[tokio::test]
async fn delta_answers() {
    let r = rig().await;
    let (me, _, _) = fixture(&r).await;
    let s = &r.store;
    let mut cube = Cube::build(s, &r.catalog, &r.prices, "e").await.unwrap();
    let at = |rev: u64, epoch: &str| CostCubeCursor {
        epoch: epoch.into(),
        rev,
    };
    let epoch = cube.epoch.clone();
    let d = cube.answer(Some(&at(cube.rev, &epoch)));
    assert!(!d.full && d.hours.is_empty() && d.cells.hour.is_empty() && d.roots.is_empty());
    assert!(cube.answer(None).full);
    assert!(cube.answer(Some(&at(cube.rev, "other"))).full);
    assert!(cube.answer(Some(&at(cube.rev + 1, &epoch))).full);

    let r1 = cube.rev;
    call(
        s,
        me,
        CallKind::Round,
        HAIKU,
        CallStatus::Settled,
        tokens(1, 1, 0, 0),
    )
    .await;
    cube.refresh(s, &r.catalog, &r.prices).await.unwrap();
    let d = cube.answer(Some(&at(r1, &epoch)));
    let hour = hour_of(u64::try_from(chrono::Utc::now().timestamp_millis()).unwrap());
    assert!(!d.full);
    assert!(d.hours.contains(&hour), "{:?}", d.hours);
    assert!(d.cells.hour.iter().all(|h| d.hours.contains(h)));
    assert!(!d.cells.hour.is_empty());
    // A rev older than the retained log.
    cube.log.pop_front();
    assert!(cube.answer(Some(&at(r1, &epoch))).full);
    assert!(!cube.answer(Some(&at(cube.rev, &epoch))).full);
}

fn meter_input(
    id: &str,
    owner: &str,
    baseline: i64,
    attributed: i64,
    reports: &[(i64, &str)],
) -> MeterInput {
    let reports: Vec<_> = reports
        .iter()
        .map(|(a, at)| serde_json::json!({"amount": a, "currency": "USD", "at": at}))
        .collect();
    MeterInput {
        id: id.into(),
        owner_id: owner.into(),
        root_id: "root".into(),
        origin: "omp".into(),
        currency: "USD".into(),
        baseline_nanos: baseline,
        attributed_nanos: attributed,
        updated_at: "2026-10-01T12:30:00+00:00".into(),
        source_metadata: (!reports.is_empty())
            .then(|| serde_json::json!({"reports": reports}).to_string()),
        harness_model: Some("anthropic/claude-fable-5-1".into()),
    }
}

fn turn_input(owner: &str, at: &str, input: i64) -> TurnInput {
    TurnInput {
        owner_id: owner.into(),
        root_id: "root".into(),
        harness: "omp".into(),
        model: Some(format!("{HAIKU}:high")),
        input_tokens: input,
        output_tokens: 10,
        cache_read_tokens: 0,
        cache_write_tokens: 0,
        at: at.into(),
    }
}

/// recon §2.4: every meter's pieces sum to `attributed_nanos` — full
/// history (exact increments, a lower total ignored), a window-limited
/// meter (pre-window remainder over turns), no reports (turns, else the
/// last update), imported baseline; plus the H2 rows merge with the ring.
#[test]
fn placement_conserves_attributed() {
    let prices = Prices::new(&models::Catalog::load(Path::new("/unused")));
    let meters = vec![
        meter_input(
            "full",
            "a",
            0,
            300,
            &[
                (0, "2026-10-01T10:00:00+00:00"),
                (100, "2026-10-01T11:00:00+00:00"),
                (50, "2026-10-01T11:30:00+00:00"),
                (300, "2026-10-01T13:00:00+00:00"),
            ],
        ),
        meter_input(
            "window",
            "b",
            0,
            1000,
            &[
                (500, "2026-10-01T12:00:00+00:00"),
                (1000, "2026-10-01T14:00:00+00:00"),
            ],
        ),
        meter_input("turns", "b", 0, 777, &[]),
        meter_input("bare", "c", 0, 42, &[]),
        meter_input(
            "imported",
            "d",
            400,
            100,
            &[
                (400, "2026-10-01T10:00:00+00:00"),
                (500, "2026-10-01T15:00:00+00:00"),
            ],
        ),
    ];
    let turns = vec![
        turn_input("b", "2026-10-01T09:00:00+00:00", 1000),
        turn_input("b", "2026-10-01T11:00:00+00:00", 3000),
        turn_input("b", "2026-10-01T13:00:00+00:00", 5000),
    ];
    // H2 rows: one duplicates the ring, one is older than the ring kept.
    let reports = vec![
        (
            "full".to_owned(),
            100,
            "USD".to_owned(),
            "2026-10-01T11:00:00+00:00".to_owned(),
        ),
        (
            "window".to_owned(),
            0,
            "USD".to_owned(),
            "2026-10-01T08:00:00+00:00".to_owned(),
        ),
        (
            "window".to_owned(),
            200,
            "USD".to_owned(),
            "2026-10-01T10:00:00+00:00".to_owned(),
        ),
    ];
    let pieces = place(&meters, &reports, &turns, &prices);
    let mut sums: HashMap<&str, u64> = HashMap::new();
    for p in &pieces {
        *sums.entry(p.meter.as_str()).or_default() += p.nanos;
        assert!(!p.model.contains(':'));
    }
    for m in &meters {
        assert_eq!(sums[m.id.as_str()], m.attributed_nanos as u64, "{}", m.id);
    }
    let of = |id: &str| pieces.iter().filter(|p| p.meter == id).collect::<Vec<_>>();
    let full: Vec<(u64, u64, bool)> = of("full")
        .iter()
        .map(|p| (p.at_ms, p.nanos, p.exact))
        .collect();
    let t = |s: &str| ms(s).unwrap();
    assert_eq!(
        full,
        vec![
            (t("2026-10-01T11:00:00+00:00"), 100, true),
            (t("2026-10-01T13:00:00+00:00"), 200, true)
        ]
    );
    // With the H2 rows the window meter's history is complete: all exact.
    assert!(of("window").iter().all(|p| p.exact));
    assert_eq!(of("window").len(), 3);
    // No reports: over the owner's three turns, weighted, approximate.
    let tp = of("turns");
    assert_eq!(tp.len(), 3);
    assert!(tp.iter().all(|p| !p.exact));
    assert!(tp[2].nanos > tp[0].nanos);
    // No reports, no turns: at the last update.
    assert_eq!(of("bare")[0].at_ms, t("2026-10-01T12:30:00+00:00"));
    // Imported baseline: only the growth after the first report.
    assert_eq!(
        of("imported")
            .iter()
            .map(|p| (p.nanos, p.exact))
            .collect::<Vec<_>>(),
        vec![(100, true)]
    );
    // Sorted by (at, id).
    assert!(
        pieces
            .windows(2)
            .all(|w| (w[0].at_ms, w[0].id()) <= (w[1].at_ms, w[1].id()))
    );
}

/// §4: pages never repeat or skip a row (including rows sharing one
/// millisecond across a page edge), Σ over pages = the cube's total for
/// the same filter, and filters narrow both alike.
#[tokio::test]
async fn cost_calls_pages_cover_the_cube() {
    let r = rig().await;
    let (me, _, _) = fixture(&r).await;
    let s = &r.store;
    for i in 0..12 {
        call(
            s,
            me,
            CallKind::Round,
            HAIKU,
            CallStatus::Settled,
            CostObservation {
                estimated: Some(usd(0.001 * f64::from(i + 1))),
                ..tokens(1, 1, 0, 0)
            },
        )
        .await;
    }
    // Seven rows in one millisecond, a whole second and a µs stamp.
    sqlx::query(
        "UPDATE model_calls SET created_at = '2026-10-01T10:00:00.123+00:00' \
         WHERE call_id IN (SELECT call_id FROM model_calls ORDER BY call_id LIMIT 7)",
    )
    .execute(s.pool())
    .await
    .unwrap();
    sqlx::query(
        "UPDATE model_calls SET created_at = '2026-10-01T10:00:01+00:00' \
         WHERE call_id IN (SELECT call_id FROM model_calls ORDER BY call_id LIMIT 1 OFFSET 7)",
    )
    .execute(s.pool())
    .await
    .unwrap();
    sqlx::query(
        "UPDATE model_calls SET created_at = '2026-10-01T10:00:01.000500+00:00' \
         WHERE call_id IN (SELECT call_id FROM model_calls ORDER BY call_id LIMIT 1 OFFSET 8)",
    )
    .execute(s.pool())
    .await
    .unwrap();
    let svc = CostService::new(s.clone(), r.catalog.clone(), "e".into());
    svc.step().await.unwrap();
    let cube = full_flat(&svc.cube(None));
    let all_rows = sql_i64(s, "SELECT COUNT(*) FROM model_calls").await as usize
        + svc.pieces().await.unwrap().len();

    for (bases, limit) in [
        (None, 3),
        (Some(vec![CostBasis::Estimated]), 2),
        (Some(vec![CostBasis::Reported, CostBasis::Unpriced]), 1),
        (None, 1000),
    ] {
        let mut q = CostCallsQuery {
            bases: bases.clone(),
            limit: Some(limit),
            ..Default::default()
        };
        let mut seen: BTreeSet<String> = BTreeSet::new();
        let mut sum = 0u64;
        let mut last: Option<(u64, String)> = None;
        loop {
            let page = svc.calls(&q).await.unwrap();
            assert!(page.calls.len() <= limit as usize);
            for c in &page.calls {
                assert!(seen.insert(c.id.clone()), "duplicate {}", c.id);
                let k = (c.at_ms, c.id.clone());
                assert!(last.as_ref().is_none_or(|l| k < *l), "newest first");
                last = Some(k);
                if let Some(b) = &bases {
                    assert!(b.contains(&c.basis));
                }
                sum += c.nanos.unwrap_or(0);
            }
            match page.next {
                Some(n) => q.before = Some(n),
                None => break,
            }
        }
        let codes: Option<Vec<u8>> = bases.as_ref().map(|b| b.iter().map(|b| b.code()).collect());
        let want: u64 = cube
            .iter()
            .filter(|(k, _)| codes.as_ref().is_none_or(|c| c.contains(&k.6)))
            .map(|(_, v)| v[0])
            .sum();
        assert_eq!(sum, want, "Σ pages = cube for {bases:?}");
        if bases.is_none() {
            assert_eq!(seen.len(), all_rows, "no gaps");
        }
    }
    // A model filter narrows the rows.
    let page = svc
        .calls(&CostCallsQuery {
            models: Some(vec!["nowhere/none-1".into()]),
            ..Default::default()
        })
        .await
        .unwrap();
    assert_eq!(page.calls.len(), 1);
    assert_eq!(page.calls[0].basis, CostBasis::Unpriced);
    assert!(page.calls[0].reason.is_some());
}

/// 160 R2: every index a `cost_calls` scan can be driven by returns exactly
/// the rows of an unindexed reference query, for single filters, value lists
/// (one UNION ALL branch per value), pairs, bases and time windows; and the
/// cube picks the selective index (a rare provider) over the time index.
#[tokio::test]
async fn call_drivers_return_the_reference_rows() {
    use crate::store::accounting::{CallDriver, CallFilter};
    let r = rig().await;
    let s = &r.store;
    let roots: Vec<SessionId> = (0..3).map(|_| SessionId::new()).collect();
    let mut owners = Vec::new();
    for (i, root) in roots.iter().enumerate() {
        s.create_session(*root, "m", None, None, None)
            .await
            .unwrap();
        let child = SessionId::new();
        s.create_session(child, "m", Some(*root), Some(&format!("c{i}")), None)
            .await
            .unwrap();
        owners.extend([*root, child]);
    }
    let models = [
        HAIKU,
        "anthropic/claude-haiku-4-5-20251001",
        "openai/gpt-5.1",
        "amazon-bedrock/anthropic.claude-opus-5-5",
        "nowhere/none-1",
    ];
    let kinds = [
        CallKind::Round,
        CallKind::Title,
        CallKind::CodeMap,
        CallKind::Endless,
    ];
    for i in 0..120usize {
        let obs = match i % 4 {
            0 => CostObservation {
                estimated: Some(usd(0.01)),
                ..tokens(10, 1, 0, 0)
            },
            1 => tokens(10, 1, 0, 0),
            2 => CostObservation::default(),
            _ => CostObservation {
                returned: Some(usd(0.02)),
                ..tokens(10, 1, 0, 0)
            },
        };
        let status = if i % 11 == 0 {
            CallStatus::Failed
        } else {
            CallStatus::Settled
        };
        call(
            s,
            owners[i % owners.len()],
            kinds[i % 7 % 4],
            models[i % 5],
            status,
            obs,
        )
        .await;
    }
    // Distinct, ordered stamps so a page edge is never ambiguous.
    sqlx::query(
        "UPDATE model_calls SET created_at = strftime('%Y-%m-%dT%H:%M:%S+00:00', '2026-10-01', \
         '+' || (SELECT COUNT(*) FROM model_calls o WHERE o.call_id < model_calls.call_id) || ' minutes')",
    )
    .execute(s.pool())
    .await
    .unwrap();
    type Row = (
        String,
        String,
        String,
        Option<String>,
        Option<String>,
        String,
        i64,
    );
    let reference: Vec<Row> = sqlx::query_as(
        "SELECT m.call_id, m.created_at, c.root_id, m.provider, m.requested_model, m.kind, \
         CASE WHEN m.returned_nanos IS NOT NULL THEN 0 WHEN m.estimated_nanos IS NOT NULL THEN 2 \
              WHEN (m.input_tokens IS NOT NULL OR m.output_tokens IS NOT NULL) \
                   AND m.requested_model IN (SELECT value FROM json_each(?)) THEN 2 ELSE 3 END \
         FROM model_calls m NOT INDEXED JOIN cost_owners c ON c.id = m.owner_id \
         WHERE m.status <> 'not_dispatched' ORDER BY m.created_at DESC",
    )
    .bind(&r.prices.priced_json)
    .fetch_all(s.pool())
    .await
    .unwrap();
    let v = |xs: &[&str]| Some(xs.iter().map(|x| (*x).to_owned()).collect::<Vec<_>>());
    let root0 = roots[0].to_string();
    let root1 = roots[1].to_string();
    let filters: Vec<CostCallsQuery> = vec![
        CostCallsQuery::default(),
        CostCallsQuery {
            providers: v(&["openai"]),
            ..Default::default()
        },
        CostCallsQuery {
            providers: v(&["anthropic", "amazon-bedrock"]),
            ..Default::default()
        },
        CostCallsQuery {
            models: v(&[HAIKU, "anthropic/claude-haiku-4-5-20251001"]),
            ..Default::default()
        },
        CostCallsQuery {
            kinds: v(&["title"]),
            ..Default::default()
        },
        CostCallsQuery {
            kinds: v(&["codemap", "round"]),
            providers: v(&["anthropic"]),
            ..Default::default()
        },
        CostCallsQuery {
            roots: v(&[&root0]),
            ..Default::default()
        },
        CostCallsQuery {
            roots: v(&[&root0, &root1]),
            models: v(&["openai/gpt-5.1"]),
            ..Default::default()
        },
        CostCallsQuery {
            bases: Some(vec![CostBasis::Unpriced]),
            ..Default::default()
        },
        CostCallsQuery {
            bases: Some(vec![CostBasis::Known]),
            ..Default::default()
        },
        CostCallsQuery {
            bases: Some(vec![CostBasis::Estimated, CostBasis::Known]),
            kinds: v(&["round"]),
            ..Default::default()
        },
        CostCallsQuery {
            providers: v(&[]),
            ..Default::default()
        },
    ];
    let windows = [
        ("9999".to_owned(), None),
        ("2026-10-01T01".to_owned(), Some("2026-10-01T00:30")),
    ];
    for q in &filters {
        let codes: Option<Vec<i64>> = q
            .bases
            .as_ref()
            .map(|b| b.iter().map(|b| i64::from(b.code())).collect());
        let ok = |l: &Option<Vec<String>>, x: Option<&str>| {
            l.as_ref()
                .is_none_or(|l| x.is_some_and(|x| l.iter().any(|v| v == x)))
        };
        let mut drivers = vec![CallDriver::Time];
        for (present, d) in [
            (q.roots.is_some(), CallDriver::Roots),
            (q.providers.is_some(), CallDriver::Provider),
            (q.models.is_some(), CallDriver::Model),
            (q.kinds.is_some(), CallDriver::Kind),
            (
                q.bases.as_deref() == Some(&[CostBasis::Unpriced][..]),
                CallDriver::Unpriced,
            ),
            (
                q.bases.as_deref() == Some(&[CostBasis::Known][..]),
                CallDriver::Known,
            ),
        ] {
            if present {
                drivers.push(d);
            }
        }
        for (upper, lower) in &windows {
            let want: Vec<&str> = reference
                .iter()
                .filter(|row| {
                    row.1.as_str() < upper.as_str()
                        && lower.is_none_or(|l| row.1.as_str() >= l)
                        && ok(&q.roots, Some(&row.2))
                        && ok(&q.providers, row.3.as_deref())
                        && ok(&q.models, row.4.as_deref())
                        && ok(&q.kinds, Some(&row.5))
                        && codes.as_ref().is_none_or(|c| c.contains(&row.6))
                })
                .map(|row| row.0.as_str())
                .collect();
            for &driver in &drivers {
                for limit in [1u32, 7, 1000] {
                    let f = CallFilter {
                        roots: q.roots.as_deref(),
                        providers: q.providers.as_deref(),
                        models: q.models.as_deref(),
                        kinds: q.kinds.as_deref(),
                        bases: q.bases.as_deref(),
                        priced: &r.prices.priced_json,
                        driver,
                    };
                    let got: Vec<String> = s
                        .cost_call_rows(&f, upper, *lower, limit)
                        .await
                        .unwrap()
                        .into_iter()
                        .map(|row| row.call_id)
                        .collect();
                    let expect: Vec<&str> = want.iter().copied().take(limit as usize).collect();
                    assert_eq!(got, expect, "{q:?} {driver:?} limit {limit} window {upper}");
                }
            }
        }
    }
    let cube = Cube::build(s, &r.catalog, &r.prices, "e").await.unwrap();
    let rare = CostCallsQuery {
        providers: v(&["openai"]),
        ..Default::default()
    };
    assert_eq!(cube.call_driver(&rare, None, 201), CallDriver::Provider);
    assert_eq!(
        cube.call_driver(&CostCallsQuery::default(), None, 201),
        CallDriver::Time
    );
    let known = CostCallsQuery {
        bases: Some(vec![CostBasis::Known]),
        ..Default::default()
    };
    assert_eq!(
        cube.call_driver(&known, None, 201),
        CallDriver::Known,
        "30 known of 120 calls"
    );
}

/// 160 H5: spend_report adds the reported leg to today, the month and the
/// owner, without adding calls.
#[tokio::test]
async fn spend_report_includes_reported_money() {
    let r = rig().await;
    fixture(&r).await;
    let svc = CostService::new(r.store.clone(), r.catalog.clone(), "e".into());
    let now = u64::try_from(chrono::Utc::now().timestamp_millis()).unwrap();
    let bare = r.store.spend_report(now, 1, &[]).await.unwrap();
    let with = svc.spend_report(now, 1).await.unwrap();
    assert!(
        (with.days[0].usd - bare.days[0].usd - 0.35).abs() < 1e-9,
        "{with:?}"
    );
    assert!((with.month.usd - bare.month.usd - 0.35).abs() < 1e-9);
    assert_eq!(with.days[0].calls, bare.days[0].calls);
    let you = |r: &crate::SpendReport| r.owners.iter().find(|o| o.name == "you").unwrap().usd;
    assert!((you(&with) - you(&bare) - 0.35).abs() < 1e-9);
}

/// Measure the cube on a copy of a real ledger (`THEUTH_160_DB`, never the
/// live file): build time, cells, JSON size, delta refresh, first page.
#[tokio::test]
#[ignore = "needs THEUTH_160_DB"]
async fn measure_on_a_real_ledger() {
    let Ok(path) = std::env::var("THEUTH_160_DB") else {
        return;
    };
    let store = Store::open(Path::new(&path), None).await.unwrap();
    let catalog = models::Catalog::load(Path::new("/unused"));
    let prices = Prices::new(&catalog);
    let t = std::time::Instant::now();
    let mut cube = Cube::build(&store, &catalog, &prices, "m").await.unwrap();
    let build = t.elapsed();
    let t = std::time::Instant::now();
    let full = cube.answer(None);
    let answer = t.elapsed();
    let json = serde_json::to_string(&full).unwrap();
    let f = full_flat(&full);
    let t = std::time::Instant::now();
    let unchanged = cube.refresh(&store, &catalog, &prices).await.unwrap();
    let refresh_idle = t.elapsed();
    // A dirty root (the newest call's): the incremental path re-aggregates it.
    sqlx::query(
        "UPDATE cost_owners SET revision = revision + 1 WHERE root_id = \
         (SELECT c.root_id FROM model_calls m JOIN cost_owners c ON c.id = m.owner_id \
          ORDER BY m.created_at DESC LIMIT 1)",
    )
    .execute(store.pool())
    .await
    .unwrap();
    let t = std::time::Instant::now();
    let dirty = cube.refresh(&store, &catalog, &prices).await.unwrap();
    let refresh_dirty = t.elapsed();
    let t = std::time::Instant::now();
    let delta = cube.answer(Some(&CostCubeCursor {
        epoch: cube.epoch.clone(),
        rev: cube.rev,
    }));
    let delta_answer = t.elapsed();
    assert_eq!(full_flat(&cube.answer(None)), f, "refresh moved nothing");

    // SQL truth (§3.4).
    let native = sql_i64(
        &store,
        "SELECT SUM(COALESCE(returned_nanos, estimated_nanos, 0)) FROM model_calls WHERE status <> 'not_dispatched'",
    )
    .await as u64;
    let rows = sql_i64(
        &store,
        "SELECT COUNT(*) FROM model_calls WHERE status <> 'not_dispatched'",
    )
    .await as u64;
    let attributed =
        sql_i64(&store, "SELECT SUM(attributed_nanos) FROM remote_meters").await as u64;
    let endless_sql = sql_i64(
        &store,
        "SELECT SUM(estimated_nanos) FROM model_calls WHERE kind = 'endless'",
    )
    .await as u64;
    let (endless_calls_nanos, attempts): (f64, i64) =
        sqlx::query_as("SELECT COALESCE(SUM(usd_estimate), 0) * 1e9, COUNT(*) FROM endless_calls")
            .fetch_one(store.pool())
            .await
            .unwrap();
    type Tok = (String, Option<i64>, Option<i64>, Option<i64>, Option<i64>);
    let unpriced_tok: Vec<Tok> = sqlx::query_as(
        "SELECT requested_model, input_tokens, output_tokens, cache_read_tokens, cache_write_tokens \
         FROM model_calls WHERE status <> 'not_dispatched' AND returned_nanos IS NULL AND estimated_nanos IS NULL \
         AND (input_tokens IS NOT NULL OR output_tokens IS NOT NULL)",
    )
    .fetch_all(store.pool())
    .await
    .unwrap();
    let n = |v: Option<i64>| v.map_or(0, |v| v as u64);
    let repriced: u64 = unpriced_tok
        .iter()
        .filter_map(|(m, i, o, cr, cw)| {
            prices
                .rate(m)
                .map(|r| Prices::estimate(r, n(*i), n(*o), n(*cr), n(*cw)))
        })
        .sum();

    let svc = CostService::new(store.clone(), Arc::new(catalog), "m".into());
    svc.step().await.unwrap();
    let t = std::time::Instant::now();
    let page = svc.calls(&CostCallsQuery::default()).await.unwrap();
    let first_page = t.elapsed();
    let t = std::time::Instant::now();
    let page2 = svc
        .calls(&CostCallsQuery {
            before: page.next.clone(),
            ..Default::default()
        })
        .await
        .unwrap();
    let second_page = t.elapsed();
    let t = std::time::Instant::now();
    let filtered = svc
        .calls(&CostCallsQuery {
            bases: Some(vec![CostBasis::Reported]),
            ..Default::default()
        })
        .await
        .unwrap();
    let reported_page = t.elapsed();

    let usd = |n: u64| n as f64 / 1e9;
    let calls: u64 = f.iter().filter(|(k, _)| k.6 != 1).map(|(_, v)| v[1]).sum();
    let repriced_calls: u64 = f.values().map(|v| v[3]).sum();
    let endless: u64 = f
        .iter()
        .filter(|(k, _)| k.6 == 2 && k.4 == "endless")
        .map(|(_, v)| v[0])
        .sum();
    let saved: u64 = f.values().map(|v| v[8]).sum();
    let saved_native: u64 = f.iter().filter(|(k, _)| k.6 != 1).map(|(_, v)| v[8]).sum();
    let approx: u64 = f.values().map(|v| v[9]).sum();
    let (known, reported, estimated) = (total(&f, Some(0)), total(&f, Some(1)), total(&f, Some(2)));

    // Month to date in local time, as the HUD buckets it.
    use chrono::{Datelike, TimeZone};
    let now = chrono::Local::now();
    let month_start = chrono::Local
        .with_ymd_and_hms(now.year(), now.month(), 1, 0, 0, 0)
        .single()
        .unwrap();
    let from_hour = (month_start.timestamp() / 3600) as u32;
    let cube_mtd: u64 = f
        .iter()
        .filter(|(k, _)| k.0 >= from_hour)
        .map(|(_, v)| v[0])
        .sum();
    let now_ms = u64::try_from(now.timestamp_millis()).unwrap();
    let rep = svc.spend_report(now_ms, 7).await.unwrap();

    println!(
        "MEASURE build={build:?} full_answer={answer:?} cells={} roots={} models={} json_bytes={} \
         refresh_idle={refresh_idle:?} ({unchanged:?}) refresh_dirty_root={refresh_dirty:?} ({dirty:?}) delta_answer={delta_answer:?} \
         first_page={first_page:?} ({} rows) second_page={second_page:?} ({} rows) reported_only_page={reported_page:?} ({} rows)",
        full.cells.hour.len(),
        full.roots.len(),
        full.models.len(),
        json.len(),
        page.calls.len(),
        page2.calls.len(),
        filtered.calls.len(),
    );
    println!(
        "MEASURE cube known=${:.4} estimated=${:.4} reported=${:.4} calls={calls} repriced_calls={repriced_calls} \
         endless_estimated=${:.6} cache_saved=${:.2} (native ${:.2}) approx=${:.2} pieces={} first_call_ms={:?} \
         pre_ledger={} unpriced={:?} delta_cells={}",
        usd(known),
        usd(estimated),
        usd(reported),
        usd(endless),
        usd(saved),
        usd(saved_native),
        usd(approx),
        cube.pieces.len(),
        full.meta.first_call_ms,
        full.meta.pre_ledger_sessions,
        full.meta.unpriced,
        delta.cells.hour.len(),
    );
    println!(
        "MEASURE sql native=${:.4} repriced=${:.4} ({} rows with tokens, no money) calls={rows} attributed=${:.4} \
         endless model_calls=${:.6} endless_calls=${:.6} ({attempts} attempts)",
        usd(native),
        usd(repriced),
        unpriced_tok.len(),
        usd(attributed),
        usd(endless_sql),
        endless_calls_nanos / 1e9,
    );
    println!(
        "MEASURE mtd cube=${:.6} spend_report=${:.6} ({})",
        usd(cube_mtd),
        rep.month.usd,
        rep.month.day
    );
    assert_eq!(known + estimated, native + repriced, "native + repriced");
    assert_eq!(reported, attributed, "reported");
    assert_eq!(calls, rows, "calls");
    assert_eq!(endless, endless_sql, "endless");
    assert!((endless_calls_nanos - endless as f64).abs() <= attempts as f64 + 1.0);
    assert!(
        (usd(cube_mtd) - rep.month.usd).abs() < 1e-6,
        "HUD = cube MTD"
    );
}

/// A vendor harness's bare model id is filed under its vendor, not `unknown`.
#[test]
fn harness_model_names_the_vendor() {
    use super::harness_model;
    assert_eq!(harness_model("claude-acp", "default".into()), "anthropic/default");
    assert_eq!(harness_model("codex-acp", "gpt-5.6".into()), "openai/gpt-5.6");
    assert_eq!(harness_model("omp", "rwx/glm".into()), "rwx/glm");
    assert_eq!(harness_model("omp", "glm".into()), "glm");
    assert_eq!(harness_model("claude-acp", "anthropic/opus".into()), "anthropic/opus");
}
