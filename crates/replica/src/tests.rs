//! Two in-process replicas on temp files; batches are handed over directly
//! (the transport is `tests/`' business).

use std::sync::Arc;
use std::time::Duration;

use sqlx::Row as _;
use sqlx::sqlite::{SqliteConnectOptions, SqliteJournalMode, SqlitePool, SqlitePoolOptions};

use crate::engine::{Engine, ReplicaHandle, now_ms};

const SCHEMA: &str =
    "CREATE TABLE tasks (id TEXT PRIMARY KEY, title TEXT, state TEXT, n INTEGER, hlc TEXT)";

pub(crate) struct Side {
    pub(crate) engine: Arc<Engine>,
    pub(crate) pool: SqlitePool,
    pub(crate) handle: ReplicaHandle,
    pub(crate) dir: tempfile::TempDir,
}

pub(crate) async fn open_pool(path: &std::path::Path) -> SqlitePool {
    let opts = SqliteConnectOptions::new()
        .filename(path)
        .create_if_missing(true)
        .journal_mode(SqliteJournalMode::Wal)
        .busy_timeout(Duration::from_secs(5));
    SqlitePoolOptions::new()
        .max_connections(4)
        .connect_with(opts)
        .await
        .unwrap()
}

pub(crate) async fn side_with(schema: &str, tables: &[&str]) -> Side {
    let dir = tempfile::tempdir().unwrap();
    let engine = Engine::open(&dir.path().join("replica.sqlite"))
        .await
        .unwrap();
    let pool = open_pool(&dir.path().join("data.sqlite")).await;
    sqlx::query(sqlx::AssertSqlSafe(schema.to_owned()))
        .execute(&pool)
        .await
        .unwrap();
    let handle = engine.attach(&pool, tables).await.unwrap();
    Side {
        engine,
        pool,
        handle,
        dir,
    }
}

async fn side() -> Side {
    side_with(SCHEMA, &["tasks"]).await
}

/// Ship everything pending on `from` to `to`.
async fn pump(from: &Side, to: &Side) {
    while let Some(batch) = from.handle.flush().await.unwrap() {
        to.engine.apply(&batch).await.unwrap();
    }
    // Re-announcements and other emitted batches are on the feed, not in the log:
    // replay whatever `from` holds that `to` lacks.
    let have = to.engine.store.version_vector().await.unwrap();
    for (origin, seq) in from.engine.store.version_vector().await.unwrap() {
        let after = have.get(&origin).copied().unwrap_or(0);
        if seq > after {
            for bytes in from
                .engine
                .store
                .batches_after(&origin, after)
                .await
                .unwrap()
            {
                let batch: crate::ChangeBatch = postcard::from_bytes(&bytes).unwrap();
                to.engine.apply(&batch).await.unwrap();
            }
        }
    }
}

async fn exchange(a: &Side, b: &Side) {
    pump(a, b).await;
    pump(b, a).await;
}

type Row = (String, Option<String>, Option<String>, Option<i64>);

async fn rows(pool: &SqlitePool) -> Vec<Row> {
    sqlx::query("SELECT id, title, state, n FROM tasks ORDER BY id")
        .fetch_all(pool)
        .await
        .unwrap()
        .into_iter()
        .map(|r| (r.get(0), r.get(1), r.get(2), r.get(3)))
        .collect()
}

async fn exec(pool: &SqlitePool, sql: &str) {
    sqlx::query(sqlx::AssertSqlSafe(sql.to_owned()))
        .execute(pool)
        .await
        .unwrap();
}

#[tokio::test]
async fn insert_on_a_appears_on_b() {
    let (a, b) = (side().await, side().await);
    exec(
        &a.pool,
        "INSERT INTO tasks (id, title, state, n) VALUES ('t1', 'write it', 'todo', 3)",
    )
    .await;
    pump(&a, &b).await;
    assert_eq!(
        rows(&b.pool).await,
        vec![(
            "t1".into(),
            Some("write it".into()),
            Some("todo".into()),
            Some(3)
        )]
    );
    // The row's hlc on B is the stamp A gave the insert.
    let hlc: String = sqlx::query("SELECT hlc FROM tasks WHERE id = 't1'")
        .fetch_one(&b.pool)
        .await
        .unwrap()
        .get(0);
    assert_eq!(crate::Hlc::parse(&hlc).unwrap().origin, *a.engine.origin());
    // Nothing echoes back: the applied rows are not B's writes.
    assert!(b.handle.flush().await.unwrap().is_none());
}

#[tokio::test]
async fn different_columns_both_survive() {
    let (a, b) = (side().await, side().await);
    exec(
        &a.pool,
        "INSERT INTO tasks (id, title, state, n) VALUES ('t1', 'one', 'todo', 1)",
    )
    .await;
    exchange(&a, &b).await;

    exec(
        &a.pool,
        "UPDATE tasks SET title = 'renamed' WHERE id = 't1'",
    )
    .await;
    exec(&b.pool, "UPDATE tasks SET state = 'doing' WHERE id = 't1'").await;
    exchange(&a, &b).await;

    let want = vec![(
        "t1".into(),
        Some("renamed".into()),
        Some("doing".into()),
        Some(1),
    )];
    assert_eq!(rows(&a.pool).await, want);
    assert_eq!(rows(&b.pool).await, want);
}

#[tokio::test]
async fn same_column_higher_hlc_wins_on_both() {
    let (a, b) = (side().await, side().await);
    exec(
        &a.pool,
        "INSERT INTO tasks (id, title) VALUES ('t1', 'one')",
    )
    .await;
    exchange(&a, &b).await;

    // B's write carries an explicit stamp a minute ahead; A's gets the clock's now.
    exec(&a.pool, "UPDATE tasks SET title = 'from A' WHERE id = 't1'").await;
    let later = b.handle.stamp(now_ms() + 60_000).to_text();
    sqlx::query("UPDATE tasks SET title = 'from B', hlc = ? WHERE id = 't1'")
        .bind(&later)
        .execute(&b.pool)
        .await
        .unwrap();
    // Whichever direction applies first, the outcome is the same.
    pump(&b, &a).await;
    pump(&a, &b).await;

    let want = vec![("t1".into(), Some("from B".into()), None, None)];
    assert_eq!(rows(&a.pool).await, want);
    assert_eq!(rows(&b.pool).await, want);
}

#[tokio::test]
async fn newer_delete_beats_update_older_update_loses_to_row() {
    let (a, b) = (side().await, side().await);
    exec(
        &a.pool,
        "INSERT INTO tasks (id, title, state) VALUES ('t1', 'one', 'todo')",
    )
    .await;
    exec(
        &a.pool,
        "INSERT INTO tasks (id, title, state) VALUES ('t2', 'two', 'todo')",
    )
    .await;
    exchange(&a, &b).await;

    // t1: B updates, then A deletes later → gone on both.
    exec(&b.pool, "UPDATE tasks SET state = 'doing' WHERE id = 't1'").await;
    b.handle.flush().await.unwrap();
    tokio::time::sleep(Duration::from_millis(2)).await;
    exec(&a.pool, "DELETE FROM tasks WHERE id = 't1'").await;

    // t2: A deletes, then B updates later → the update voids the delete; both keep the full row.
    exec(&a.pool, "DELETE FROM tasks WHERE id = 't2'").await;
    a.handle.flush().await.unwrap();
    tokio::time::sleep(Duration::from_millis(2)).await;
    exec(&b.pool, "UPDATE tasks SET state = 'review' WHERE id = 't2'").await;

    exchange(&a, &b).await;
    exchange(&a, &b).await; // the re-announced row

    let want = vec![("t2".into(), Some("two".into()), Some("review".into()), None)];
    assert_eq!(
        rows(&a.pool).await,
        want,
        "A: t1 gone, t2 back with every column"
    );
    assert_eq!(rows(&b.pool).await, want, "B: t1 gone, t2 kept");
}

#[tokio::test]
async fn reapply_is_idempotent_and_seq_survives_restart() {
    let (a, b) = (side().await, side().await);
    exec(
        &a.pool,
        "INSERT INTO tasks (id, title, n) VALUES ('t1', 'one', 1)",
    )
    .await;
    let batch = a.handle.flush().await.unwrap().unwrap();
    assert!(b.engine.apply(&batch).await.unwrap());
    assert!(
        !b.engine.apply(&batch).await.unwrap(),
        "second apply is a no-op"
    );
    exec(&b.pool, "UPDATE tasks SET n = 2 WHERE id = 't1'").await;
    pump(&b, &a).await;
    assert!(!b.engine.apply(&batch).await.unwrap());
    let want = vec![("t1".into(), Some("one".into()), None, Some(2))];
    assert_eq!(rows(&a.pool).await, want);
    assert_eq!(
        rows(&b.pool).await,
        want,
        "replaying the old insert does not undo the newer n"
    );

    // A reopened bookkeeping file keeps the origin and the feed position.
    let origin = a.engine.origin().clone();
    let reopened = Engine::open(&a.dir.path().join("replica.sqlite"))
        .await
        .unwrap();
    assert_eq!(*reopened.origin(), origin);
    let vv = reopened.store.version_vector().await.unwrap();
    assert!(vv[&origin] >= batch.seq);
    let next = reopened.emit_locked(Vec::new()).await.unwrap();
    assert!(
        next.seq > vv[&origin],
        "seq continues above what the file holds"
    );
}

#[tokio::test]
async fn attach_rejects_bad_shapes() {
    let dir = tempfile::tempdir().unwrap();
    let engine = Engine::open(&dir.path().join("replica.sqlite"))
        .await
        .unwrap();
    let pool = open_pool(&dir.path().join("data.sqlite")).await;
    exec(&pool, "CREATE TABLE no_hlc (id TEXT PRIMARY KEY, a TEXT)").await;
    exec(&pool, "CREATE TABLE no_pk (a TEXT, hlc TEXT)").await;
    exec(
        &pool,
        "CREATE TABLE ok (id INTEGER PRIMARY KEY, a TEXT, hlc TEXT)",
    )
    .await;
    let e = engine
        .attach(&pool, &["no_hlc"])
        .await
        .unwrap_err()
        .to_string();
    assert!(e.contains("no_hlc") && e.contains("hlc"), "{e}");
    let e = engine
        .attach(&pool, &["no_pk"])
        .await
        .unwrap_err()
        .to_string();
    assert!(e.contains("no_pk") && e.contains("primary key"), "{e}");
    let e = engine
        .attach(&pool, &["missing"])
        .await
        .unwrap_err()
        .to_string();
    assert!(e.contains("missing"), "{e}");
    engine.attach(&pool, &["ok"]).await.unwrap();
    let e = engine.attach(&pool, &["ok"]).await.unwrap_err().to_string();
    assert!(e.contains("already attached"), "{e}");
}

/// Rows of `origin` in `a`'s outbox.
async fn outbox_rows(a: &Side, origin: &str) -> u64 {
    let vv = a
        .engine
        .store
        .batches_after(&crate::PeerId(origin.into()), 0)
        .await
        .unwrap();
    vv.len() as u64
}

/// 69 audit: the outbox pruned only this engine's own batches; every relayed
/// origin grew forever. Now each origin is pruned below what every known peer
/// holds of it, always keeping the newest batch, and nothing is pruned while
/// a known peer has not said hello since start.
#[tokio::test]
async fn prunes_every_origin_below_what_all_peers_hold() {
    let a = side().await;
    let batch = |origin: &str, seq: u64| crate::ChangeBatch {
        origin: crate::PeerId(origin.into()),
        seq,
        changes: Vec::new(),
    };
    // Own feed: seq 1..=3. Relayed: B 1..=3, C 1..=2, D 1.
    for _ in 0..3 {
        a.engine.emit_locked(Vec::new()).await.unwrap();
    }
    for (origin, n) in [("B", 3), ("C", 2), ("D", 1)] {
        for seq in 1..=n {
            assert!(a.engine.apply(&batch(origin, seq)).await.unwrap());
        }
    }
    let me = a.engine.origin().0.clone();
    let vv = |pairs: &[(&str, u64)]| -> std::collections::HashMap<crate::PeerId, u64> {
        pairs
            .iter()
            .map(|(o, s)| (crate::PeerId((*o).to_owned()), *s))
            .collect()
    };
    let (b, c, d) = (
        crate::PeerId("B".into()),
        crate::PeerId("C".into()),
        crate::PeerId("D".into()),
    );

    // B and C said hello; D (a known origin) has not: nothing goes.
    a.engine
        .learn(&b, &vv(&[(&me, 3), ("B", 3), ("C", 2), ("D", 1)]));
    a.engine.learn(&c, &vv(&[(&me, 2), ("B", 2), ("C", 2)]));
    assert_eq!(a.engine.prune_outbox().await.unwrap(), 0);
    assert_eq!(
        (outbox_rows(&a, &me).await, outbox_rows(&a, "B").await),
        (3, 3)
    );

    // D says hello: every origin drops below the minimum any peer holds.
    // me: min(3, 2, 1) = 1 → nothing below 1. B: min(3, 2, 0) = 0. C: min(2, 2, 2) = 2 → #1 goes.
    a.engine.learn(&d, &vv(&[(&me, 1), ("C", 2), ("D", 1)]));
    assert_eq!(a.engine.prune_outbox().await.unwrap(), 1);
    assert_eq!(outbox_rows(&a, "C").await, 1);
    // B holds everything: me → floor 3 keeps only #3, B → floor 3, C already at #2, D floor 1 keeps #1.
    a.engine
        .learn(&c, &vv(&[(&me, 3), ("B", 3), ("C", 2), ("D", 1)]));
    a.engine
        .learn(&d, &vv(&[(&me, 3), ("B", 3), ("C", 2), ("D", 1)]));
    assert_eq!(a.engine.prune_outbox().await.unwrap(), 4);
    for origin in [me.as_str(), "B", "C", "D"] {
        assert_eq!(
            outbox_rows(&a, origin).await,
            1,
            "{origin}: the newest batch always stays"
        );
    }
    // The version vector (what `Hello` advertises) is untouched by pruning.
    let held = a.engine.store.version_vector().await.unwrap();
    assert_eq!(held[&crate::PeerId(me)], 3);
    assert_eq!(held[&b], 3);
}

/// 69 audit: `captured`/`applied`/`echo` count column changes; a local write
/// that repeats an applied `(table, pk, column, value)` within 500 ms is an
/// echo, an ordinary edit is not. Before: no counter existed.
#[tokio::test]
async fn counters_see_captures_applies_and_echoes() {
    let (a, b) = (side().await, side().await);
    exec(
        &a.pool,
        "INSERT INTO tasks (id, title, n) VALUES ('t1', 'one', 1)",
    )
    .await;
    pump(&a, &b).await;
    let (sa, sb) = (
        a.engine.stats().await.unwrap(),
        b.engine.stats().await.unwrap(),
    );
    assert_eq!(
        (sa.captured, sa.applied, sa.echo, sa.outbox_rows),
        (3, 0, 0, 1),
        "{sa:?}"
    );
    assert_eq!(
        (sb.captured, sb.applied, sb.echo, sb.outbox_rows),
        (0, 3, 0, 1),
        "{sb:?}"
    );

    // B writes the row back as it stands (a mirror re-scanning its own projection would):
    // the same-value update logs nothing, so drop and re-insert to force the capture.
    exec(&b.pool, "DELETE FROM tasks WHERE id = 't1'").await;
    exec(
        &b.pool,
        "INSERT INTO tasks (id, title, n) VALUES ('t1', 'one', 1)",
    )
    .await;
    b.handle.flush().await.unwrap();
    let sb = b.engine.stats().await.unwrap();
    assert_eq!(
        (sb.captured, sb.echo),
        (4, 3),
        "delete + three echoed columns: {sb:?}"
    );

    // A real edit on B is not an echo.
    exec(&b.pool, "UPDATE tasks SET n = 2 WHERE id = 't1'").await;
    b.handle.flush().await.unwrap();
    let sb = b.engine.stats().await.unwrap();
    assert_eq!((sb.captured, sb.echo), (5, 3), "{sb:?}");
}

/// 180 D1b: one table of a batch that cannot be written here (B's `agents`
/// has a UNIQUE A's lacks, and a local row holds the value) no longer aborts
/// the batch: `cost_owners`, behind it alphabetically, lands; the failed
/// table's rows and shadow clocks roll back and its changes are parked, not
/// dropped. Once the local row is gone, the next batch touching `agents`
/// retries them and they land.
#[tokio::test]
async fn apply_with_one_failing_table_still_lands_cost_owners() {
    async fn side_of(agents: &str) -> Side {
        let dir = tempfile::tempdir().unwrap();
        let engine = Engine::open(&dir.path().join("replica.sqlite"))
            .await
            .unwrap();
        let pool = open_pool(&dir.path().join("data.sqlite")).await;
        exec(&pool, agents).await;
        exec(
            &pool,
            "CREATE TABLE cost_owners (id TEXT PRIMARY KEY, root_id TEXT NOT NULL, hlc TEXT)",
        )
        .await;
        let handle = engine
            .attach(&pool, &["agents", "cost_owners"])
            .await
            .unwrap();
        Side {
            engine,
            pool,
            handle,
            dir,
        }
    }
    async fn n_of(pool: &SqlitePool, id: &str) -> Option<i64> {
        sqlx::query_scalar("SELECT n FROM agents WHERE id = ?")
            .bind(id)
            .fetch_optional(pool)
            .await
            .unwrap()
    }
    async fn parked(side: &Side) -> i64 {
        sqlx::query_scalar("SELECT COUNT(*) FROM parked")
            .fetch_one(side.engine.store.pool())
            .await
            .unwrap()
    }
    let a = side_of("CREATE TABLE agents (id TEXT PRIMARY KEY, n INTEGER, hlc TEXT)").await;
    let b = side_of("CREATE TABLE agents (id TEXT PRIMARY KEY, n INTEGER UNIQUE, hlc TEXT)").await;
    exec(
        &b.pool,
        "INSERT INTO agents (id, n) VALUES ('x', 1), ('y', -1)",
    )
    .await;
    b.handle.flush().await.unwrap();
    exec(&a.pool, "INSERT INTO agents (id, n) VALUES ('x', -1)").await;
    exec(
        &a.pool,
        "INSERT INTO cost_owners (id, root_id) VALUES ('s', 's')",
    )
    .await;
    let batch = a.handle.flush().await.unwrap().unwrap();
    let tables: std::collections::BTreeSet<&str> =
        batch.changes.iter().map(|c| c.table.as_str()).collect();
    assert_eq!(tables.len(), 2, "one batch carries both tables");

    assert!(b.engine.apply(&batch).await.unwrap());
    let owner: Option<String> =
        sqlx::query_scalar("SELECT root_id FROM cost_owners WHERE id = 's'")
            .fetch_optional(&b.pool)
            .await
            .unwrap();
    assert_eq!(owner.as_deref(), Some("s"));
    assert_eq!(
        n_of(&b.pool, "x").await,
        Some(1),
        "the failing table rolled back"
    );
    let mut conn = b.engine.store.pool().acquire().await.unwrap();
    let clocks = crate::store::col_clocks(&mut conn, "agents", "x")
        .await
        .unwrap();
    assert!(
        clocks
            .get("n")
            .is_none_or(|h| h.origin == *b.engine.origin()),
        "A's clock for the unwritten column was rolled back"
    );
    drop(conn);
    assert_eq!(parked(&b).await, 1, "kept for retry, not dropped");
    assert!(
        !b.engine.apply(&batch).await.unwrap(),
        "the batch is recorded"
    );

    // What blocked it goes away; A's next `agents` change triggers the retry.
    exec(&b.pool, "DELETE FROM agents WHERE id = 'y'").await;
    b.handle.flush().await.unwrap();
    exec(&a.pool, "INSERT INTO agents (id, n) VALUES ('z', 5)").await;
    let next = a.handle.flush().await.unwrap().unwrap();
    assert!(b.engine.apply(&next).await.unwrap());
    assert_eq!(
        n_of(&b.pool, "x").await,
        Some(-1),
        "the parked change landed"
    );
    assert_eq!(n_of(&b.pool, "z").await, Some(5));
    assert_eq!(parked(&b).await, 0);
}

/// A side whose data file runs `schema` (one statement each) and attaches `tables`.
async fn side_of_schema(schema: &[&str], tables: &[&str]) -> Side {
    let dir = tempfile::tempdir().unwrap();
    let engine = Engine::open(&dir.path().join("replica.sqlite"))
        .await
        .unwrap();
    let pool = open_pool(&dir.path().join("data.sqlite")).await;
    for stmt in schema {
        exec(&pool, stmt).await;
    }
    let handle = engine.attach(&pool, tables).await.unwrap();
    Side {
        engine,
        pool,
        handle,
        dir,
    }
}

async fn parked_count(side: &Side) -> i64 {
    sqlx::query_scalar("SELECT COUNT(*) FROM parked")
        .fetch_one(side.engine.store.pool())
        .await
        .unwrap()
}

/// 180 follow-up 1 (three origins): B applies X's child before its parent,
/// so B's trigger writes a stand-in parent; Y's partial update of the parent
/// then raises the stand-in row's `hlc` to a real stamp on B. B's snapshot
/// must ship the stand-in's placeholder columns under the stand-in stamp —
/// not under Y's — or they overwrite the parent's real row on X.
#[tokio::test]
async fn trigger_stand_in_never_ships_placeholder_columns_under_a_real_stamp() {
    const SCHEMA: [&str; 2] = [
        "CREATE TABLE owners (id TEXT PRIMARY KEY, parent TEXT REFERENCES owners(id), \
         kind TEXT NOT NULL, note TEXT, hlc TEXT)",
        "CREATE TRIGGER owners_stand_in BEFORE INSERT ON owners \
         WHEN NEW.parent IS NOT NULL AND NOT EXISTS (SELECT 1 FROM owners WHERE id = NEW.parent) \
         BEGIN INSERT OR IGNORE INTO owners (id, parent, kind, hlc) \
         VALUES (NEW.parent, NULL, 'stand-in', '0000000000000000-00000000-0'); END",
    ];
    let (x, y, b) = (
        side_of_schema(&SCHEMA, &["owners"]).await,
        side_of_schema(&SCHEMA, &["owners"]).await,
        side_of_schema(&SCHEMA, &["owners"]).await,
    );
    exec(
        &x.pool,
        "INSERT INTO owners (id, parent, kind) VALUES ('g', NULL, 'real'), ('p', 'g', 'real')",
    )
    .await;
    let parents = x.handle.flush().await.unwrap().unwrap();
    exec(
        &x.pool,
        "INSERT INTO owners (id, parent, kind) VALUES ('c', 'p', 'real')",
    )
    .await;
    let child = x.handle.flush().await.unwrap().unwrap();

    // Y holds the parents and edits P's note: a partial update.
    assert!(y.engine.apply(&parents).await.unwrap());
    exec(&y.pool, "UPDATE owners SET note = 'from y' WHERE id = 'p'").await;
    let note = y.handle.flush().await.unwrap().unwrap();

    // B gets the child first (stand-in P), then Y's note.
    assert!(b.engine.apply(&child).await.unwrap());
    assert!(b.engine.apply(&note).await.unwrap());
    let (kind, hlc): (String, String) =
        sqlx::query_as("SELECT kind, hlc FROM owners WHERE id = 'p'")
            .fetch_one(&b.pool)
            .await
            .unwrap();
    assert_eq!(kind, "stand-in");
    assert_ne!(
        hlc,
        crate::Hlc::stand_in().to_text(),
        "the row hlc is real now"
    );

    let snapshot = b.engine.snapshot_batch().await.unwrap().unwrap();
    for col in ["parent", "kind"] {
        let c = snapshot
            .changes
            .iter()
            .find(|c| c.pk == "p" && c.column.as_deref() == Some(col))
            .unwrap();
        assert_eq!(c.hlc, crate::Hlc::stand_in(), "{col}");
    }
    assert!(x.engine.apply(&snapshot).await.unwrap());
    let real: (Option<String>, String) =
        sqlx::query_as("SELECT parent, kind FROM owners WHERE id = 'p'")
            .fetch_one(&x.pool)
            .await
            .unwrap();
    assert_eq!(real, (Some("g".to_owned()), "real".to_owned()));
}

/// 180 follow-up 2: a parked entry that cannot be retried (here: unreadable)
/// is that entry's problem — the batch that triggered the retry still
/// applies and `apply` returns `Ok`, so the stream does not end.
#[tokio::test]
async fn parked_retry_error_does_not_end_the_stream() {
    let (a, b) = (side().await, side().await);
    sqlx::query(
        "INSERT INTO parked (origin, seq, tbl, changes, parked_at) VALUES ('z', 1, 'tasks', x'ff', ?)",
    )
    .bind(i64::try_from(now_ms()).unwrap())
    .execute(b.engine.store.pool())
    .await
    .unwrap();
    exec(
        &a.pool,
        "INSERT INTO tasks (id, title, state, n) VALUES ('t1', 'x', 'todo', 1)",
    )
    .await;
    let batch = a.handle.flush().await.unwrap().unwrap();
    assert!(b.engine.apply(&batch).await.unwrap());
    assert_eq!(rows(&b.pool).await.len(), 1);
    assert_eq!(parked_count(&b).await, 0, "the unreadable entry is dropped");
}

/// 180 follow-up 3: a table that fails for good parks every batch it gets;
/// at most 256 per `(origin, table)` are kept (oldest dropped), and entries
/// older than 7 days expire.
#[tokio::test]
async fn parked_is_bounded_and_ages_out() {
    let a = side_of_schema(
        &["CREATE TABLE agents (id TEXT PRIMARY KEY, n INTEGER, hlc TEXT)"],
        &["agents"],
    )
    .await;
    let b = side_of_schema(
        &["CREATE TABLE agents (id TEXT PRIMARY KEY, n INTEGER CHECK (n >= 0), hlc TEXT)"],
        &["agents"],
    )
    .await;
    exec(&b.pool, "INSERT INTO agents (id, n) VALUES ('x', 1)").await;
    b.handle.flush().await.unwrap();
    // Every batch A emits (by flush or by its own poll) reaches B.
    let mut feed = a.engine.subscribe();
    async fn ship(
        a: &Side,
        b: &Side,
        feed: &mut tokio::sync::broadcast::Receiver<crate::ChangeBatch>,
    ) {
        a.handle.flush().await.unwrap();
        while let Ok(batch) = feed.try_recv() {
            assert!(b.engine.apply(&batch).await.unwrap());
        }
    }
    for i in 1..=260 {
        exec(
            &a.pool,
            &format!(
                "INSERT INTO agents (id, n) VALUES ('x', -{i}) ON CONFLICT(id) DO UPDATE SET n = -{i}"
            ),
        )
        .await;
        ship(&a, &b, &mut feed).await;
    }
    assert_eq!(parked_count(&b).await, 256);
    let oldest: i64 = sqlx::query_scalar("SELECT MIN(seq) FROM parked")
        .fetch_one(b.engine.store.pool())
        .await
        .unwrap();
    assert_eq!(oldest, 5, "the four oldest went");

    sqlx::query("UPDATE parked SET parked_at = 0")
        .execute(b.engine.store.pool())
        .await
        .unwrap();
    exec(&a.pool, "UPDATE agents SET n = -999 WHERE id = 'x'").await;
    ship(&a, &b, &mut feed).await;
    assert_eq!(
        parked_count(&b).await,
        1,
        "only the new one is younger than 7 days"
    );
}

/// 182 M1: B ran an older schema when A wrote `home`; B skipped the column
/// and recorded the batch. After B's migration and re-attach nothing ships
/// it again — until A re-announces its rows, under their own clocks.
#[tokio::test]
async fn a_column_a_peer_attached_late_arrives_on_reannounce() {
    let a = side_with(
        "CREATE TABLE tasks (id TEXT PRIMARY KEY, title TEXT, home TEXT, hlc TEXT)",
        &["tasks"],
    )
    .await;
    let b = side_with(
        "CREATE TABLE tasks (id TEXT PRIMARY KEY, title TEXT, hlc TEXT)",
        &["tasks"],
    )
    .await;
    exec(
        &a.pool,
        "INSERT INTO tasks (id, title, home) VALUES ('t1', 'x', 'eng-a'), ('t2', 'y', 'eng-c')",
    )
    .await;
    exchange(&a, &b).await;
    // B upgrades and restarts: the column appears, the fresh attach picks it up.
    exec(&b.pool, "ALTER TABLE tasks ADD COLUMN home TEXT").await;
    let Side { pool, dir, .. } = b;
    let engine = Engine::open(&dir.path().join("replica.sqlite"))
        .await
        .unwrap();
    let handle = engine.attach(&pool, &["tasks"]).await.unwrap();
    let b = Side {
        engine,
        pool,
        handle,
        dir,
    };
    exchange(&a, &b).await;
    let home = |pool: SqlitePool, id: &'static str| async move {
        sqlx::query_scalar::<_, Option<String>>("SELECT home FROM tasks WHERE id = ?")
            .bind(id)
            .fetch_one(&pool)
            .await
            .unwrap()
    };
    assert_eq!(
        home(b.pool.clone(), "t1").await,
        None,
        "the batch counted as applied"
    );
    // A re-announces the rows homed on it — `home` only, under its clock.
    assert_eq!(
        a.handle
            .reannounce("tasks", "home = ?", "eng-a", Some(&["home"]))
            .await
            .unwrap(),
        1
    );
    exchange(&a, &b).await;
    assert_eq!(home(b.pool.clone(), "t1").await.as_deref(), Some("eng-a"));
    assert_eq!(
        home(b.pool.clone(), "t2").await,
        None,
        "only the filtered rows"
    );
    // A newer write on B wins over a re-announce of the old value.
    exec(
        &b.pool,
        "UPDATE tasks SET home = 'eng-b', hlc = NULL WHERE id = 't1'",
    )
    .await;
    while b.handle.flush().await.unwrap().is_some() {}
    exchange(&a, &b).await;
    a.handle
        .reannounce("tasks", "id = ?", "t1", Some(&["home"]))
        .await
        .unwrap();
    exchange(&a, &b).await;
    assert_eq!(home(a.pool.clone(), "t1").await.as_deref(), Some("eng-b"));
    assert_eq!(home(b.pool.clone(), "t1").await.as_deref(), Some("eng-b"));
}

/// B ran an older schema when A wrote `os_name`, skipped it and recorded the
/// batch. A restamping its own unchanged stats row ships every column under a
/// fresh clock, so B converges.
#[tokio::test]
async fn a_restamped_row_reaches_a_peer_that_dropped_its_columns() {
    let a = side_with(
        "CREATE TABLE engine_stats (id TEXT PRIMARY KEY, os TEXT, os_name TEXT, hlc TEXT)",
        &["engine_stats"],
    )
    .await;
    let b = side_with(
        "CREATE TABLE engine_stats (id TEXT PRIMARY KEY, os TEXT, hlc TEXT)",
        &["engine_stats"],
    )
    .await;
    exec(
        &a.pool,
        "INSERT INTO engine_stats (id, os, os_name) VALUES ('me', 'linux', 'Arch Linux')",
    )
    .await;
    exchange(&a, &b).await;
    exec(&b.pool, "ALTER TABLE engine_stats ADD COLUMN os_name TEXT").await;
    let Side { pool, dir, .. } = b;
    let engine = Engine::open(&dir.path().join("replica.sqlite"))
        .await
        .unwrap();
    let handle = engine.attach(&pool, &["engine_stats"]).await.unwrap();
    let b = Side {
        engine,
        pool,
        handle,
        dir,
    };
    // An unchanged rewrite logs nothing: B stays empty.
    exec(
        &a.pool,
        "UPDATE engine_stats SET os_name = 'Arch Linux' WHERE id = 'me'",
    )
    .await;
    exchange(&a, &b).await;
    let row = |pool: SqlitePool| async move {
        sqlx::query_as::<_, (Option<String>, Option<String>)>(
            "SELECT os, os_name FROM engine_stats WHERE id = 'me'",
        )
        .fetch_one(&pool)
        .await
        .unwrap()
    };
    assert_eq!(row(b.pool.clone()).await, (Some("linux".into()), None));

    let mut feed = a.handle.subscribe();
    assert!(a.handle.restamp_own_stats("me").await.unwrap());
    let batch = feed.recv().await.unwrap();
    let stamp: String = sqlx::query_scalar("SELECT hlc FROM engine_stats WHERE id = 'me'")
        .fetch_one(&a.pool)
        .await
        .unwrap();
    let mut cols: Vec<_> = batch
        .changes
        .iter()
        .map(|c| {
            assert_eq!(c.hlc.to_text(), stamp, "one fresh stamp, also the row's");
            c.column.clone().unwrap()
        })
        .collect();
    cols.sort();
    assert_eq!(cols, ["os", "os_name"], "every column, changed or not");
    exchange(&a, &b).await;
    assert_eq!(
        row(b.pool.clone()).await,
        (Some("linux".into()), Some("Arch Linux".into()))
    );
    assert!(!a.handle.restamp_own_stats("nobody").await.unwrap());
}

/// Restamping gives a local copy a fresh clock over every column: on a
/// multi-writer table a stale engine would clobber newer writes, so any
/// table but `engine_stats` is refused and nothing is emitted.
#[tokio::test]
async fn restamp_refuses_a_multi_writer_table() {
    let a = side().await;
    exec(&a.pool, "INSERT INTO tasks (id, title) VALUES ('t1', 'x')").await;
    while a.handle.flush().await.unwrap().is_some() {}
    let mut feed = a.handle.subscribe();
    let err = a.handle.restamp("tasks", "t1").await.unwrap_err();
    assert!(err.to_string().contains("engine_stats"), "{err:#}");
    assert!(feed.try_recv().is_err(), "nothing emitted");
}

/// 182: batches that arrive before a table's plugin attached (tasks attaches
/// after the replica at boot) are parked, not skipped: at `attach` the row,
/// its later state change and the wake all land, in order.
#[tokio::test]
async fn batch_for_an_unattached_table_applies_after_attach() {
    const TABLES: &str = "CREATE TABLE tasks (id TEXT PRIMARY KEY, title TEXT NOT NULL, state TEXT, n INTEGER, hlc TEXT); \
         CREATE TABLE wakes (id TEXT PRIMARY KEY, task TEXT, hlc TEXT)";
    let a = side_with(TABLES, &["tasks", "wakes"]).await;
    let b = side_with(
        &format!("{TABLES}; CREATE TABLE agents (id TEXT PRIMARY KEY, n INTEGER, hlc TEXT)"),
        &["agents"],
    )
    .await;
    exec(
        &a.pool,
        "INSERT INTO tasks (id, title, state) VALUES ('t1', 'x', 'todo')",
    )
    .await;
    exec(&a.pool, "INSERT INTO wakes (id, task) VALUES ('w1', 't1')").await;
    pump(&a, &b).await;
    exec(&a.pool, "UPDATE tasks SET state = 'doing' WHERE id = 't1'").await;
    pump(&a, &b).await;
    let count = |sql: &'static str| {
        let pool = b.pool.clone();
        async move {
            sqlx::query_scalar::<_, i64>(sql)
                .fetch_one(&pool)
                .await
                .unwrap()
        }
    };
    assert_eq!(
        count("SELECT COUNT(*) FROM tasks").await,
        0,
        "nothing lands before attach"
    );
    assert!(parked_count(&b).await >= 2, "parked, not skipped");
    b.engine.attach(&b.pool, &["tasks", "wakes"]).await.unwrap();
    let state: Option<String> = sqlx::query_scalar("SELECT state FROM tasks WHERE id = 't1'")
        .fetch_one(&b.pool)
        .await
        .unwrap();
    assert_eq!(
        state.as_deref(),
        Some("doing"),
        "the row and its later state change"
    );
    assert_eq!(
        count("SELECT COUNT(*) FROM wakes WHERE task = 't1'").await,
        1
    );
    assert_eq!(parked_count(&b).await, 0, "the parked entries applied");
}

/// Incident (333 renamed to "P50"): a row written before the attach has no
/// shadow clock. A peer's wall-zero stamp (a stand-in re-announce, a
/// `mesh_meta` default) must not replace its value — it only fills a column
/// that is NULL here. A real stamp still wins as before.
#[tokio::test]
async fn a_wall_zero_change_only_fills_an_unclocked_null_column() {
    let b = side_of_schema(
        &[
            "CREATE TABLE mesh_meta (key TEXT PRIMARY KEY, value TEXT, note TEXT, hlc TEXT)",
            "INSERT INTO mesh_meta (key, value, note, hlc) \
             VALUES ('name', 'home', NULL, '000001a000000000-00000000-me')",
        ],
        &["mesh_meta"],
    )
    .await;
    let change = |column: &str, value: &str, hlc: &crate::Hlc| crate::Change {
        table: "mesh_meta".into(),
        pk: "name".into(),
        column: Some(column.into()),
        value: crate::value::Val::Text(value.into()).encode(),
        hlc: hlc.clone(),
        deleted: false,
    };
    let zero = crate::Hlc {
        wall_ms: 0,
        counter: 0x9539_530e,
        origin: crate::PeerId("p50".into()),
    };
    let stand_in = crate::Hlc::stand_in();
    let batch = crate::ChangeBatch {
        origin: crate::PeerId("p50".into()),
        seq: 1,
        changes: vec![
            change("value", "P50", &zero),
            change("value", "P50", &stand_in),
            change("note", "filled", &stand_in),
        ],
    };
    assert!(b.engine.apply(&batch).await.unwrap());
    let row = || async {
        sqlx::query_as::<_, (Option<String>, Option<String>)>(
            "SELECT value, note FROM mesh_meta WHERE key = 'name'",
        )
        .fetch_one(&b.pool)
        .await
        .unwrap()
    };
    assert_eq!(
        row().await,
        (Some("home".into()), Some("filled".into())),
        "local value kept, NULL column filled"
    );

    let real = crate::Hlc {
        wall_ms: now_ms(),
        counter: 0,
        origin: crate::PeerId("p50".into()),
    };
    let batch = crate::ChangeBatch {
        origin: crate::PeerId("p50".into()),
        seq: 2,
        changes: vec![change("value", "renamed", &real)],
    };
    assert!(b.engine.apply(&batch).await.unwrap());
    assert_eq!(
        row().await.0.as_deref(),
        Some("renamed"),
        "a real write wins"
    );
}
