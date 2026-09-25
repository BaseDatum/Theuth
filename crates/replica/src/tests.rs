//! Two in-process replicas on temp files; batches are handed over directly
//! (the transport is `tests/`' business).

use std::sync::Arc;
use std::time::Duration;

use sqlx::sqlite::{SqliteConnectOptions, SqliteJournalMode, SqlitePool, SqlitePoolOptions};
use sqlx::Row as _;

use crate::engine::{now_ms, Engine, ReplicaHandle};

const SCHEMA: &str = "CREATE TABLE tasks (id TEXT PRIMARY KEY, title TEXT, state TEXT, n INTEGER, hlc TEXT)";

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
    SqlitePoolOptions::new().max_connections(4).connect_with(opts).await.unwrap()
}

pub(crate) async fn side_with(schema: &str, tables: &[&str]) -> Side {
    let dir = tempfile::tempdir().unwrap();
    let engine = Engine::open(&dir.path().join("replica.sqlite")).await.unwrap();
    let pool = open_pool(&dir.path().join("data.sqlite")).await;
    sqlx::query(sqlx::AssertSqlSafe(schema.to_owned())).execute(&pool).await.unwrap();
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
            for bytes in from.engine.store.batches_after(&origin, after).await.unwrap() {
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
    sqlx::query(sqlx::AssertSqlSafe(sql.to_owned())).execute(pool).await.unwrap();
}

#[tokio::test]
async fn insert_on_a_appears_on_b() {
    let (a, b) = (side().await, side().await);
    exec(&a.pool, "INSERT INTO tasks (id, title, state, n) VALUES ('t1', 'write it', 'todo', 3)").await;
    pump(&a, &b).await;
    assert_eq!(rows(&b.pool).await, vec![("t1".into(), Some("write it".into()), Some("todo".into()), Some(3))]);
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
    exec(&a.pool, "INSERT INTO tasks (id, title, state, n) VALUES ('t1', 'one', 'todo', 1)").await;
    exchange(&a, &b).await;

    exec(&a.pool, "UPDATE tasks SET title = 'renamed' WHERE id = 't1'").await;
    exec(&b.pool, "UPDATE tasks SET state = 'doing' WHERE id = 't1'").await;
    exchange(&a, &b).await;

    let want = vec![("t1".into(), Some("renamed".into()), Some("doing".into()), Some(1))];
    assert_eq!(rows(&a.pool).await, want);
    assert_eq!(rows(&b.pool).await, want);
}

#[tokio::test]
async fn same_column_higher_hlc_wins_on_both() {
    let (a, b) = (side().await, side().await);
    exec(&a.pool, "INSERT INTO tasks (id, title) VALUES ('t1', 'one')").await;
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
    exec(&a.pool, "INSERT INTO tasks (id, title, state) VALUES ('t1', 'one', 'todo')").await;
    exec(&a.pool, "INSERT INTO tasks (id, title, state) VALUES ('t2', 'two', 'todo')").await;
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
    assert_eq!(rows(&a.pool).await, want, "A: t1 gone, t2 back with every column");
    assert_eq!(rows(&b.pool).await, want, "B: t1 gone, t2 kept");
}

#[tokio::test]
async fn reapply_is_idempotent_and_seq_survives_restart() {
    let (a, b) = (side().await, side().await);
    exec(&a.pool, "INSERT INTO tasks (id, title, n) VALUES ('t1', 'one', 1)").await;
    let batch = a.handle.flush().await.unwrap().unwrap();
    assert!(b.engine.apply(&batch).await.unwrap());
    assert!(!b.engine.apply(&batch).await.unwrap(), "second apply is a no-op");
    exec(&b.pool, "UPDATE tasks SET n = 2 WHERE id = 't1'").await;
    pump(&b, &a).await;
    assert!(!b.engine.apply(&batch).await.unwrap());
    let want = vec![("t1".into(), Some("one".into()), None, Some(2))];
    assert_eq!(rows(&a.pool).await, want);
    assert_eq!(rows(&b.pool).await, want, "replaying the old insert does not undo the newer n");

    // A reopened bookkeeping file keeps the origin and the feed position.
    let origin = a.engine.origin().clone();
    let reopened = Engine::open(&a.dir.path().join("replica.sqlite")).await.unwrap();
    assert_eq!(*reopened.origin(), origin);
    let vv = reopened.store.version_vector().await.unwrap();
    assert!(vv[&origin] >= batch.seq);
    let next = reopened.emit_locked(Vec::new()).await.unwrap();
    assert!(next.seq > vv[&origin], "seq continues above what the file holds");
}

#[tokio::test]
async fn attach_rejects_bad_shapes() {
    let dir = tempfile::tempdir().unwrap();
    let engine = Engine::open(&dir.path().join("replica.sqlite")).await.unwrap();
    let pool = open_pool(&dir.path().join("data.sqlite")).await;
    exec(&pool, "CREATE TABLE no_hlc (id TEXT PRIMARY KEY, a TEXT)").await;
    exec(&pool, "CREATE TABLE no_pk (a TEXT, hlc TEXT)").await;
    exec(&pool, "CREATE TABLE ok (id INTEGER PRIMARY KEY, a TEXT, hlc TEXT)").await;
    let e = engine.attach(&pool, &["no_hlc"]).await.unwrap_err().to_string();
    assert!(e.contains("no_hlc") && e.contains("hlc"), "{e}");
    let e = engine.attach(&pool, &["no_pk"]).await.unwrap_err().to_string();
    assert!(e.contains("no_pk") && e.contains("primary key"), "{e}");
    let e = engine.attach(&pool, &["missing"]).await.unwrap_err().to_string();
    assert!(e.contains("missing"), "{e}");
    engine.attach(&pool, &["ok"]).await.unwrap();
    let e = engine.attach(&pool, &["ok"]).await.unwrap_err().to_string();
    assert!(e.contains("already attached"), "{e}");
}
