//! Two (then three) in-process replicas over a `tokio::io::duplex` transport:
//! converge live, after a partition with writes on both sides, and through a
//! relay that never talks to the origin.

use std::collections::HashMap;
use std::sync::Arc;
use std::time::Duration;

use parking_lot::Mutex;
use replica::{ByteStream, Engine, PeerEvent, PeerId, Transport};
use sqlx::sqlite::{SqliteConnectOptions, SqliteJournalMode, SqlitePool, SqlitePoolOptions};
use sqlx::Row as _;
use tokio::sync::{broadcast, mpsc};

type Inbound = (PeerId, Box<dyn ByteStream>);

/// The wire between every `Loop`: who is up, and each peer's accept queue.
#[derive(Default)]
struct Switch {
    inboxes: Mutex<HashMap<PeerId, mpsc::UnboundedSender<Inbound>>>,
    /// Pairs that may talk; `open` fails and `peers` omits anything else.
    links: Mutex<Vec<(PeerId, PeerId)>>,
}

impl Switch {
    fn linked(&self, a: &PeerId, b: &PeerId) -> bool {
        self.links
            .lock()
            .iter()
            .any(|(x, y)| (x == a && y == b) || (x == b && y == a))
    }
}

struct Loop {
    me: PeerId,
    switch: Arc<Switch>,
    inbox: tokio::sync::Mutex<mpsc::UnboundedReceiver<Inbound>>,
    events: broadcast::Sender<PeerEvent>,
}

impl Loop {
    fn new(name: &str, switch: &Arc<Switch>) -> Arc<Self> {
        let me = PeerId(name.to_owned());
        let (tx, rx) = mpsc::unbounded_channel();
        switch.inboxes.lock().insert(me.clone(), tx);
        Arc::new(Self {
            me,
            switch: switch.clone(),
            inbox: tokio::sync::Mutex::new(rx),
            events: broadcast::channel(64).0,
        })
    }
}

#[async_trait::async_trait]
impl Transport for Loop {
    fn peers(&self) -> Vec<PeerId> {
        let inboxes = self.switch.inboxes.lock();
        inboxes
            .keys()
            .filter(|p| **p != self.me && self.switch.linked(&self.me, p))
            .cloned()
            .collect()
    }

    async fn open(&self, peer: &PeerId, _alpn: &str) -> anyhow::Result<Box<dyn ByteStream>> {
        anyhow::ensure!(self.switch.linked(&self.me, peer), "{} is unreachable", peer);
        let (mine, theirs) = tokio::io::duplex(1 << 20);
        let inboxes = self.switch.inboxes.lock();
        let to = inboxes.get(peer).ok_or_else(|| anyhow::anyhow!("no such peer"))?;
        to.send((self.me.clone(), Box::new(theirs)))
            .map_err(|_| anyhow::anyhow!("peer gone"))?;
        Ok(Box::new(mine))
    }

    async fn accept(&self, _alpn: &str) -> anyhow::Result<Inbound> {
        self.inbox
            .lock()
            .await
            .recv()
            .await
            .ok_or_else(|| anyhow::anyhow!("switch closed"))
    }

    fn subscribe(&self) -> broadcast::Receiver<PeerEvent> {
        self.events.subscribe()
    }
}

/// Connect or cut `a`–`b` and tell both sides.
fn link(switch: &Switch, a: &Loop, b: &Loop, up: bool) {
    let mut links = switch.links.lock();
    links.retain(|(x, y)| !((x == &a.me && y == &b.me) || (x == &b.me && y == &a.me)));
    if up {
        links.push((a.me.clone(), b.me.clone()));
    }
    drop(links);
    let ev = |p: &PeerId| {
        if up {
            PeerEvent::Connected(p.clone())
        } else {
            PeerEvent::Disconnected(p.clone())
        }
    };
    let _ = a.events.send(ev(&b.me));
    let _ = b.events.send(ev(&a.me));
}

struct Node {
    engine: Arc<Engine>,
    pool: SqlitePool,
    transport: Arc<Loop>,
    _dir: tempfile::TempDir,
}

async fn node(name: &str, switch: &Arc<Switch>) -> Node {
    let dir = tempfile::tempdir().unwrap();
    let engine = Engine::open(&dir.path().join("replica.sqlite")).await.unwrap();
    let opts = SqliteConnectOptions::new()
        .filename(dir.path().join("data.sqlite"))
        .create_if_missing(true)
        .journal_mode(SqliteJournalMode::Wal)
        .busy_timeout(Duration::from_secs(5));
    let pool = SqlitePoolOptions::new().max_connections(4).connect_with(opts).await.unwrap();
    sqlx::query("CREATE TABLE tasks (id TEXT PRIMARY KEY, title TEXT, state TEXT, hlc TEXT)")
        .execute(&pool)
        .await
        .unwrap();
    engine.attach(&pool, &["tasks"]).await.unwrap();
    let transport = Loop::new(name, switch);
    engine.register_transport(transport.clone());
    Node {
        engine,
        pool,
        transport,
        _dir: dir,
    }
}

async fn rows(pool: &SqlitePool) -> Vec<(String, Option<String>, Option<String>)> {
    sqlx::query("SELECT id, title, state FROM tasks ORDER BY id")
        .fetch_all(pool)
        .await
        .unwrap()
        .into_iter()
        .map(|r| (r.get(0), r.get(1), r.get(2)))
        .collect()
}

async fn exec(pool: &SqlitePool, sql: &str) {
    sqlx::query(sqlx::AssertSqlSafe(sql.to_owned())).execute(pool).await.unwrap();
}

/// Poll until every pool shows `want` (capture polls at 200 ms, so a few
/// rounds) or fail after 10 s.
async fn converge(pools: &[&SqlitePool], want: &[(String, Option<String>, Option<String>)]) {
    let deadline = tokio::time::Instant::now() + Duration::from_secs(10);
    loop {
        let mut all = true;
        for p in pools {
            if rows(p).await != want {
                all = false;
            }
        }
        if all {
            return;
        }
        if tokio::time::Instant::now() > deadline {
            for (i, p) in pools.iter().enumerate() {
                eprintln!("pool {i}: {:?}", rows(p).await);
            }
            panic!("did not converge on {want:?}");
        }
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
}

fn row(id: &str, title: &str, state: &str) -> (String, Option<String>, Option<String>) {
    (id.into(), Some(title.into()), Some(state.into()))
}

#[tokio::test]
async fn converges_live_and_after_partition() {
    let switch = Arc::new(Switch::default());
    let a = node("A", &switch).await;
    let b = node("B", &switch).await;
    link(&switch, &a.transport, &b.transport, true);

    // Live: a write on A lands on B; B's edit comes back.
    exec(&a.pool, "INSERT INTO tasks (id, title, state) VALUES ('t1', 'one', 'todo')").await;
    converge(&[&a.pool, &b.pool], &[row("t1", "one", "todo")]).await;
    exec(&b.pool, "UPDATE tasks SET state = 'doing' WHERE id = 't1'").await;
    converge(&[&a.pool, &b.pool], &[row("t1", "one", "doing")]).await;

    // Partition: writes on both sides, different columns of t1 and a new row each.
    link(&switch, &a.transport, &b.transport, false);
    tokio::time::sleep(Duration::from_millis(50)).await;
    exec(&a.pool, "UPDATE tasks SET title = 'renamed on A' WHERE id = 't1'").await;
    exec(&a.pool, "INSERT INTO tasks (id, title, state) VALUES ('t2', 'from A', 'todo')").await;
    exec(&b.pool, "UPDATE tasks SET state = 'review' WHERE id = 't1'").await;
    exec(&b.pool, "INSERT INTO tasks (id, title, state) VALUES ('t3', 'from B', 'todo')").await;
    tokio::time::sleep(Duration::from_millis(500)).await;
    assert_ne!(rows(&a.pool).await, rows(&b.pool).await, "partitioned sides differ");

    // Reconnect: Hello exchange catches both up.
    link(&switch, &a.transport, &b.transport, true);
    converge(
        &[&a.pool, &b.pool],
        &[
            row("t1", "renamed on A", "review"),
            row("t2", "from A", "todo"),
            row("t3", "from B", "todo"),
        ],
    )
    .await;
    assert_ne!(a.engine.origin(), b.engine.origin());
}

#[tokio::test]
async fn relays_through_a_middle_peer() {
    let switch = Arc::new(Switch::default());
    let a = node("A", &switch).await;
    let b = node("B", &switch).await;
    let c = node("C", &switch).await;
    link(&switch, &a.transport, &b.transport, true);
    link(&switch, &b.transport, &c.transport, true);

    exec(&a.pool, "INSERT INTO tasks (id, title, state) VALUES ('t1', 'one', 'todo')").await;
    converge(&[&a.pool, &b.pool, &c.pool], &[row("t1", "one", "todo")]).await;
    // C edits what it only ever heard about from B; A sees it.
    exec(&c.pool, "UPDATE tasks SET state = 'done' WHERE id = 't1'").await;
    converge(&[&a.pool, &b.pool, &c.pool], &[row("t1", "one", "done")]).await;

    // A batch C holds from A reaches a late joiner D that only knows C.
    let d = node("D", &switch).await;
    link(&switch, &c.transport, &d.transport, true);
    converge(&[&d.pool], &[row("t1", "one", "done")]).await;
    // 69 audit counters (column changes): B forwarded A's insert (2) to C and C's update (1) to A;
    // C forwarded A's insert to D; A, a leaf, relayed nothing.
    let (a_stats, b_stats, c_stats) =
        (a.engine.stats().await.unwrap(), b.engine.stats().await.unwrap(), c.engine.stats().await.unwrap());
    assert_eq!(a_stats.relayed, 0, "{a_stats:?}");
    assert!(b_stats.relayed >= 3, "{b_stats:?}");
    assert!(c_stats.relayed >= 2, "{c_stats:?}");
    assert_eq!(a_stats.echo + b_stats.echo + c_stats.echo, 0, "no echo anywhere");
}

/// A table that existed before it joined replication (68 decision 8:
/// `cost_owners`): its rows carry no `hlc` and no trigger ever saw them.
/// `announce_unstamped` ships them once; a second call finds nothing.
#[tokio::test]
async fn pre_existing_rows_are_announced_once() {
    let switch = Arc::new(Switch::default());
    let a = node("A", &switch).await;
    let b = node("B", &switch).await;
    link(&switch, &a.transport, &b.transport, true);
    for n in [&a, &b] {
        exec(&n.pool, "CREATE TABLE owners (id TEXT PRIMARY KEY, root TEXT, hlc TEXT)").await;
    }
    exec(&a.pool, "INSERT INTO owners (id, root) VALUES ('o1', 'r1'), ('o2', 'r1')").await;
    let ha = a.engine.attach(&a.pool, &["owners"]).await.unwrap();
    let _hb = b.engine.attach(&b.pool, &["owners"]).await.unwrap();
    tokio::time::sleep(Duration::from_millis(500)).await;
    async fn on_b(pool: &SqlitePool) -> Vec<String> {
        sqlx::query_scalar::<_, String>("SELECT id FROM owners ORDER BY id")
            .fetch_all(pool)
            .await
            .unwrap()
    }

    assert_eq!(ha.announce_unstamped().await.unwrap(), 2);
    let deadline = tokio::time::Instant::now() + Duration::from_secs(10);
    while on_b(&b.pool).await.len() < 2 {
        assert!(tokio::time::Instant::now() < deadline, "B never received the announced rows");
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
    assert_eq!(on_b(&b.pool).await, ["o1", "o2"]);
    let stamped: i64 = sqlx::query_scalar("SELECT count(*) FROM owners WHERE hlc IS NULL")
        .fetch_one(&a.pool)
        .await
        .unwrap();
    assert_eq!(stamped, 0);
    assert_eq!(ha.announce_unstamped().await.unwrap(), 0);
}

/// 69 smoke: once every known peer holds a batch the outbox prunes it, so a peer that joins
/// later (or lost its replica) finds its catch-up point below what is held. It gets the whole
/// state as one batch first; the remaining tail (single columns) then lands as a no-op instead of
/// a partial row. Before the fix C ended with `title = NULL` (the insert was pruned away).
#[tokio::test]
async fn a_late_joiner_behind_the_pruned_outbox_gets_a_snapshot() {
    let switch = Arc::new(Switch::default());
    let a = node("A", &switch).await;
    let b = node("B", &switch).await;
    link(&switch, &a.transport, &b.transport, true);
    // B writes too: the prune knows peers by the origins in the outbox.
    exec(&b.pool, "INSERT INTO tasks (id, title, state) VALUES ('b1', 'from B', 'todo')").await;
    exec(&a.pool, "INSERT INTO tasks (id, title, state) VALUES ('t1', 'one', 'todo')").await;
    converge(&[&a.pool, &b.pool], &[row("b1", "from B", "todo"), row("t1", "one", "todo")]).await;
    for state in ["doing", "review", "done"] {
        exec(&a.pool, &format!("UPDATE tasks SET state = '{state}' WHERE id = 't1'")).await;
        converge(&[&a.pool, &b.pool], &[row("b1", "from B", "todo"), row("t1", "one", state)]).await;
    }
    exec(&a.pool, "INSERT INTO tasks (id, title, state) VALUES ('t2', 'two', 'todo'), ('t3', 'three', 'todo')").await;
    exec(&a.pool, "DELETE FROM tasks WHERE id = 't3'").await;
    let want = [row("b1", "from B", "todo"), row("t1", "one", "done"), row("t2", "two", "todo")];
    converge(&[&a.pool, &b.pool], &want).await;
    // The prune ran: A's own feed no longer starts at seq 1.
    let book = SqlitePoolOptions::new()
        .max_connections(1)
        .connect_with(SqliteConnectOptions::new().filename(a._dir.path().join("replica.sqlite")))
        .await
        .unwrap();
    let oldest: i64 = sqlx::query_scalar("SELECT MIN(seq) FROM outbox WHERE origin = ?")
        .bind(&a.engine.origin().0)
        .fetch_one(&book)
        .await
        .unwrap();
    assert!(oldest > 1, "A's outbox was not pruned (oldest seq {oldest})");

    // C knows only A and never saw the insert batches.
    let c = node("C", &switch).await;
    link(&switch, &a.transport, &c.transport, true);
    converge(&[&c.pool], &want).await;
    // A later write on A still reaches C; the tombstone travelled (t3 stays dead on C).
    exec(&a.pool, "UPDATE tasks SET state = 'archived' WHERE id = 't2'").await;
    converge(
        &[&a.pool, &b.pool, &c.pool],
        &[row("b1", "from B", "todo"), row("t1", "one", "done"), row("t2", "two", "archived")],
    )
    .await;
    let c_stats = c.engine.stats().await.unwrap();
    assert_eq!(c_stats.echo, 0, "{c_stats:?}");
}

/// 69 smoke: a peer's single-column write to a row that exists here must land on a table with a
/// NOT NULL column — SQLite checks NOT NULL before the upsert's conflict target, so the old
/// `INSERT … ON CONFLICT DO UPDATE` with a column subset never applied one.
#[tokio::test]
async fn a_partial_update_lands_on_a_not_null_table() {
    let switch = Arc::new(Switch::default());
    let a = node("A", &switch).await;
    let b = node("B", &switch).await;
    link(&switch, &a.transport, &b.transport, true);
    for n in [&a, &b] {
        exec(&n.pool, "CREATE TABLE notes (id TEXT PRIMARY KEY, kind TEXT NOT NULL, body TEXT, hlc TEXT)").await;
        n.engine.attach(&n.pool, &["notes"]).await.unwrap();
    }
    exec(&a.pool, "INSERT INTO notes (id, kind, body) VALUES ('n1', 'note', 'first')").await;
    async fn body(pool: &SqlitePool) -> Option<String> {
        sqlx::query_scalar::<_, Option<String>>("SELECT body FROM notes WHERE id = 'n1'")
            .fetch_optional(pool)
            .await
            .unwrap()
            .flatten()
    }
    let deadline = tokio::time::Instant::now() + Duration::from_secs(10);
    while body(&b.pool).await.as_deref() != Some("first") {
        assert!(tokio::time::Instant::now() < deadline, "insert never reached B");
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
    exec(&b.pool, "UPDATE notes SET body = 'edited on B' WHERE id = 'n1'").await;
    let deadline = tokio::time::Instant::now() + Duration::from_secs(10);
    while body(&a.pool).await.as_deref() != Some("edited on B") {
        assert!(tokio::time::Instant::now() < deadline, "B's column write never landed on A: {:?}", body(&a.pool).await);
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
}
