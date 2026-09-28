//! Spike (67 §18 item 1): does SQLite's session extension work on the SQLite
//! theuth links — the vendored SQLCipher-on-BoringSSL `libsqlite3-sys` — both
//! on a plain WAL file (how `crates/agent/src/store.rs` opens its store) and
//! on a keyed SQLCipher file? Write-up: `docs/design/67-replica-spike.md`.
//!
//! Run once by hand with `--nocapture` to see the numbers:
//! `cargo test -p replica --test spike_session_ext -- --nocapture`.

use std::time::{Duration, Instant};

use rusqlite::fallible_streaming_iterator::FallibleStreamingIterator;
use rusqlite::hooks::Action;
use rusqlite::session::{ChangesetIter, ConflictAction, ConflictType, Session};
use rusqlite::{Connection, params};

const SCHEMA: &str = "CREATE TABLE t (id TEXT PRIMARY KEY, a TEXT, b INTEGER, hlc TEXT)";
/// Column index of `hlc` in `t`, as the changeset numbers columns.
const HLC_COL: usize = 3;

/// Open `name` under `dir` the way `agent::store::Store::open` does (WAL, 5 s
/// busy timeout); `key` adds SQLCipher's `PRAGMA key` before any other access.
fn open(dir: &std::path::Path, name: &str, key: Option<&str>) -> Connection {
    let conn = Connection::open(dir.join(name)).unwrap();
    if let Some(k) = key {
        conn.pragma_update(None, "key", k).unwrap();
    }
    conn.pragma_update(None, "journal_mode", "WAL").unwrap();
    conn.busy_timeout(Duration::from_secs(5)).unwrap();
    conn.execute_batch(SCHEMA).unwrap();
    conn
}

/// The changeset bytes of every write to `t` made by `f` on `conn`.
fn capture(conn: &Connection, f: impl FnOnce(&Connection)) -> Vec<u8> {
    let mut session = Session::new(conn).unwrap();
    session.attach(Some("t")).unwrap();
    f(conn);
    let mut buf = Vec::new();
    session.changeset_strm(&mut buf).unwrap();
    buf
}

/// Apply `changeset` to `conn`: per-row last-writer-wins on the `hlc` column,
/// higher wins; a delete of a row already gone or an insert of a row already
/// present are settled the same way.
fn apply_lww(conn: &Connection, changeset: &[u8]) {
    let mut input: &[u8] = changeset;
    conn.apply_strm(
        &mut input,
        None::<fn(&str) -> bool>,
        |kind: ConflictType, item: rusqlite::session::ChangesetItem| {
            match kind {
                // Local row differs from the changeset's `old` values (UPDATE/DELETE),
                // or an INSERT hits an existing pk: compare clocks.
                ConflictType::SQLITE_CHANGESET_DATA | ConflictType::SQLITE_CHANGESET_CONFLICT => {
                    let local = item
                        .conflict(HLC_COL)
                        .ok()
                        .and_then(|v| v.as_str().ok().map(str::to_owned));
                    let incoming = item
                        .new_value(HLC_COL)
                        .ok()
                        .and_then(|v| v.as_str().ok().map(str::to_owned));
                    match (incoming, local) {
                        (Some(i), Some(l)) if i > l => ConflictAction::SQLITE_CHANGESET_REPLACE,
                        // A DELETE has no `new` values: the deleter's clock is not in the
                        // changeset, so a delete never beats a row that moved on.
                        _ => ConflictAction::SQLITE_CHANGESET_OMIT,
                    }
                }
                // Row to update/delete is gone here: nothing to do.
                ConflictType::SQLITE_CHANGESET_NOTFOUND => ConflictAction::SQLITE_CHANGESET_OMIT,
                _ => ConflictAction::SQLITE_CHANGESET_ABORT,
            }
        },
    )
    .unwrap();
}

/// `(id, a, b, hlc)` of table `t`.
type Row = (String, Option<String>, Option<i64>, Option<String>);

fn rows(conn: &Connection) -> Vec<Row> {
    let mut st = conn
        .prepare("SELECT id, a, b, hlc FROM t ORDER BY id")
        .unwrap();
    st.query_map([], |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?, r.get(3)?)))
        .unwrap()
        .map(Result::unwrap)
        .collect()
}

fn ops(changeset: &[u8]) -> (usize, usize, usize) {
    let mut input: &[u8] = changeset;
    let input: &mut dyn std::io::Read = &mut input;
    let mut it = ChangesetIter::start_strm(&input).unwrap();
    let (mut i, mut u, mut d) = (0, 0, 0);
    while let Some(item) = it.next().unwrap() {
        match item.op().unwrap().code() {
            Action::SQLITE_INSERT => i += 1,
            Action::SQLITE_UPDATE => u += 1,
            Action::SQLITE_DELETE => d += 1,
            _ => unreachable!(),
        }
    }
    (i, u, d)
}

/// (1) + (2): a session captures inserts, updates and deletes as a changeset;
/// applying it to a second file reproduces the rows. `key` runs the same on a
/// SQLCipher-encrypted pair.
fn capture_and_replay(key: Option<&str>) {
    let dir = tempfile::tempdir().unwrap();
    let a = open(dir.path(), "a.sqlite", key);
    let b = open(dir.path(), "b.sqlite", key);

    let cs = capture(&a, |a| {
        a.execute("INSERT INTO t VALUES ('1', 'one', 1, '0001')", [])
            .unwrap();
        a.execute("INSERT INTO t VALUES ('2', 'two', 2, '0001')", [])
            .unwrap();
        a.execute("INSERT INTO t VALUES ('3', 'three', 3, '0001')", [])
            .unwrap();
        a.execute("UPDATE t SET a = 'TWO', hlc = '0002' WHERE id = '2'", [])
            .unwrap();
        a.execute("DELETE FROM t WHERE id = '3'", []).unwrap();
    });
    assert!(!cs.is_empty(), "session yielded an empty changeset");
    // The session coalesces per row: row 2's insert+update is one INSERT with the final
    // values; row 3's insert+delete cancels out.
    assert_eq!(ops(&cs), (2, 0, 0), "coalesced ops");

    apply_lww(&b, &cs);
    assert_eq!(rows(&a), rows(&b));
    assert_eq!(rows(&b).len(), 2);

    // A second session on top of the replicated state: pure updates and a delete.
    let cs2 = capture(&a, |a| {
        a.execute("UPDATE t SET b = 11, hlc = '0003' WHERE id = '1'", [])
            .unwrap();
        a.execute("DELETE FROM t WHERE id = '2'", []).unwrap();
    });
    assert_eq!(ops(&cs2), (0, 1, 1));
    apply_lww(&b, &cs2);
    assert_eq!(rows(&a), rows(&b));
    assert_eq!(
        rows(&b),
        vec![(
            "1".into(),
            Some("one".into()),
            Some(11),
            Some("0003".into())
        )]
    );
}

#[test]
fn captures_and_replays_plain_wal() {
    capture_and_replay(None);
}

#[test]
fn captures_and_replays_under_sqlcipher_key() {
    capture_and_replay(Some("spike-key"));
    // Sanity: the keyed file is not readable without the key.
    let dir = tempfile::tempdir().unwrap();
    let _ = open(dir.path(), "k.sqlite", Some("spike-key"));
    let plain = Connection::open(dir.path().join("k.sqlite")).unwrap();
    assert!(
        plain.execute_batch("SELECT count(*) FROM t").is_err(),
        "opened a keyed file without its key"
    );
}

/// (3): the same row updated on both sides; the conflict handler keeps the
/// higher `hlc` on both, whichever direction applies first.
#[test]
fn concurrent_update_resolved_by_hlc() {
    let dir = tempfile::tempdir().unwrap();
    let a = open(dir.path(), "a.sqlite", None);
    let b = open(dir.path(), "b.sqlite", None);
    for c in [&a, &b] {
        c.execute("INSERT INTO t VALUES ('1', 'base', 0, '0001')", [])
            .unwrap();
    }

    let from_a = capture(&a, |a| {
        a.execute("UPDATE t SET a = 'A', hlc = '0003' WHERE id = '1'", [])
            .unwrap();
    });
    let from_b = capture(&b, |b| {
        b.execute("UPDATE t SET a = 'B', hlc = '0002' WHERE id = '1'", [])
            .unwrap();
    });

    // B receives A's later write: conflict (old value 'base' != 'B'), higher wins → replace.
    apply_lww(&b, &from_a);
    // A receives B's earlier write: conflict, lower loses → omit.
    apply_lww(&a, &from_b);
    let want = vec![("1".into(), Some("A".into()), Some(0), Some("0003".into()))];
    assert_eq!(rows(&a), want);
    assert_eq!(rows(&b), want);

    // Delete vs update: A deletes, B updates with a higher clock; B's row survives
    // on B (the delete has no clock to beat it) and A learns the row back.
    let del = capture(&a, |a| {
        a.execute("DELETE FROM t WHERE id = '1'", []).unwrap();
    });
    let upd = capture(&b, |b| {
        b.execute("UPDATE t SET b = 9, hlc = '0004' WHERE id = '1'", [])
            .unwrap();
    });
    apply_lww(&b, &del);
    apply_lww(&a, &upd); // NOTFOUND on A: omitted — the update alone cannot resurrect.
    assert_eq!(
        rows(&b).len(),
        1,
        "update with a higher clock survives the delete on B"
    );
    assert_eq!(
        rows(&a).len(),
        0,
        "a lone column update cannot resurrect a deleted row on A"
    );
}

/// (4): numbers. Printed; recorded in docs/design/67-replica-spike.md.
#[test]
fn numbers() {
    let dir = tempfile::tempdir().unwrap();
    let insert = |conn: &Connection, n: usize| {
        let tx = conn.unchecked_transaction().unwrap();
        {
            let mut st = tx.prepare("INSERT INTO t VALUES (?1, ?2, ?3, ?4)").unwrap();
            for i in 0..n {
                st.execute(params![
                    format!("row-{i:06}"),
                    format!("value {i}"),
                    i as i64,
                    "0001"
                ])
                .unwrap();
            }
        }
        tx.commit().unwrap();
    };

    let plain = open(dir.path(), "plain.sqlite", None);
    let t0 = Instant::now();
    insert(&plain, 10_000);
    let without = t0.elapsed();

    let tracked = open(dir.path(), "tracked.sqlite", None);
    let mut session = Session::new(&tracked).unwrap();
    session.attach(Some("t")).unwrap();
    let t0 = Instant::now();
    insert(&tracked, 10_000);
    let with = t0.elapsed();
    let mut cs = Vec::new();
    session.changeset_strm(&mut cs).unwrap();
    drop(session);

    let target = open(dir.path(), "target.sqlite", None);
    let t0 = Instant::now();
    apply_lww(&target, &cs);
    let apply = t0.elapsed();
    assert_eq!(rows(&target).len(), 10_000);

    // Per-1k-row changeset: a fresh session over exactly 1000 inserts.
    let one_k = open(dir.path(), "onek.sqlite", None);
    let cs_1k = capture(&one_k, |c| insert(c, 1_000));

    eprintln!(
        "SPIKE 10k inserts, one transaction, no session: {without:?} ({:.1} µs/row)",
        without.as_secs_f64() * 1e6 / 10_000.0
    );
    eprintln!(
        "SPIKE 10k inserts, one transaction, session attached: {with:?} ({:.1} µs/row)",
        with.as_secs_f64() * 1e6 / 10_000.0
    );
    eprintln!(
        "SPIKE changeset for 10k rows: {} bytes; for 1k rows: {} bytes ({:.1} B/row)",
        cs.len(),
        cs_1k.len(),
        cs_1k.len() as f64 / 1000.0
    );
    eprintln!("SPIKE apply 10k-row changeset to a fresh file: {apply:?}");
}
