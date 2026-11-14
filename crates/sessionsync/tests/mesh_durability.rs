//! Design doc 139 §11 Z: the two-engine matrix, in-process half. Two data
//! dirs, two `Store::open(.., Some(EngineId))`, hand-driven shipping through
//! `apply_log_row` (what the shipper does). No `Agents`: the 109 fakes are
//! `cfg(test)`-private in `agent` (139-as-built-C deviation 6, Revision 1
//! Q13), so the rows that need a running turn (T1–T8) are the scripted
//! two-engine smoke `scripts/durability-mesh-smoke.sh`; here are the rows the
//! store and the log settle on their own — T9 (D15) end to end, and the
//! store-level halves of T1 and T7 (a replica holds the owner's open turn
//! and acts on nothing).

use agent::durability::StepKind;
use agent::store::{Scope, Store};
use replica::PeerId;
use runtime::{SessionId, TurnId};
use sessionsync::apply::{Applied, apply_log_row};

struct Engine {
    store: Store,
    dir: std::path::PathBuf,
}

async fn engine(name: &str) -> Engine {
    let dir = std::env::temp_dir().join(format!("theuth-139z-{name}-{}", uuid::Uuid::now_v7()));
    std::fs::create_dir_all(&dir).unwrap();
    let store = Store::open(
        &dir.join("sessions.sqlite"),
        Some(mesh::EngineId(name.into())),
    )
    .await
    .unwrap();
    Engine { store, dir }
}

impl Engine {
    fn id(&self) -> PeerId {
        PeerId(self.store.engine().to_owned())
    }

    /// Apply every row of `from` not applied here yet; returns what each did.
    async fn pull(&self, from: &Engine) -> Vec<Applied> {
        let origin = from.id();
        let mut out = Vec::new();
        for row in from.store.log_after(0).await.unwrap() {
            let seq = row.seq;
            let r = apply_log_row(&self.store, &origin, &row.into())
                .await
                .unwrap();
            if r != Applied::Duplicate {
                from.store
                    .set_cursor(&origin.0, self.store.engine(), seq)
                    .await
                    .unwrap();
            }
            out.push(r);
        }
        out
    }

    async fn close(self) {
        self.store.close().await;
        let _ = std::fs::remove_dir_all(&self.dir);
    }
}

async fn session_on(store: &Store) -> SessionId {
    let id = SessionId::new();
    store
        .create_session(id, "m", None, None, None)
        .await
        .unwrap();
    id
}

/// Open a turn of `id` on `store` (its `input_accepted` step).
async fn open_turn_on(store: &Store, id: SessionId) -> TurnId {
    let turn = TurnId::new();
    store
        .step(
            id,
            turn,
            StepKind::InputAccepted,
            None,
            &serde_json::json!({}),
        )
        .await
        .unwrap();
    turn
}

/// T9 (D15): `journal_prune(Scope::Here)` on the owner deletes a finished
/// turn's rows and the replica loses them through the log; the replica's
/// own tick deletes nothing of the owner's, before or after.
#[tokio::test]
async fn t9_journal_prune_travels_through_the_log_and_never_touches_a_foreign_journal() {
    let (a, b) = (engine("a").await, engine("b").await);
    let id = session_on(&a.store).await;
    let done = open_turn_on(&a.store, id).await;
    a.store.finish_turn(id, done, "end_turn").await.unwrap();
    let open = open_turn_on(&a.store, id).await;
    let shipped = b.pull(&a).await;
    assert!(
        shipped.iter().all(|r| *r == Applied::Applied),
        "{shipped:?}"
    );
    assert!(
        b.store.resume_plan(id, done).await.is_ok(),
        "B holds the finished turn"
    );

    // B's tick: A's finished turn is not B's to prune.
    assert_eq!(b.store.journal_prune(Scope::Here, 0).await.unwrap(), 0);
    assert!(b.store.resume_plan(id, done).await.is_ok());
    // All-scope would be a fenced write on a replica: refused whole.
    assert!(b.store.journal_prune(Scope::All, 0).await.is_err());
    assert!(b.store.resume_plan(id, done).await.is_ok());

    // A's tick: the finished turn goes, the open one stays, one log row ships the DELETEs.
    let before = a.store.log_seq();
    assert_eq!(a.store.journal_prune(Scope::Here, 0).await.unwrap(), 2);
    assert!(a.store.resume_plan(id, done).await.is_err());
    assert_eq!(
        a.store.open_turn(id).await.unwrap().map(|(t, _)| t),
        Some(open)
    );
    assert_eq!(a.store.log_seq(), before + 1);

    let applied = b.pull(&a).await;
    assert_eq!(
        applied.iter().filter(|r| **r == Applied::Applied).count(),
        1
    );
    assert!(
        b.store.resume_plan(id, done).await.is_err(),
        "B's copy lost the pruned turn through the log"
    );
    assert_eq!(
        b.store.open_turn(id).await.unwrap().map(|(t, _)| t),
        Some(open)
    );
    // Nothing more to ship either way; B's tick is still a no-op.
    assert_eq!(b.store.journal_prune(Scope::Here, 0).await.unwrap(), 0);
    assert!(a.pull(&b).await.is_empty());
    a.close().await;
    b.close().await;
}

/// T1 / T7, store level: B holds A's open turn — display reads see it with
/// `owner = a`, `here = false`; every acting read (`Scope::Here`) is empty;
/// B cannot write the turn; A's view of its own turn is untouched.
#[tokio::test]
async fn t1_t7_replica_displays_the_owners_turn_and_acts_on_nothing() {
    let (a, b) = (engine("a").await, engine("b").await);
    let id = session_on(&a.store).await;
    let turn = open_turn_on(&a.store, id).await;
    b.pull(&a).await;

    let row = b.store.session(id).await.unwrap().unwrap();
    assert_eq!((row.owner.as_str(), row.here), ("a", false));
    assert_eq!(
        b.store.open_turn(id).await.unwrap().map(|(t, _)| t),
        Some(turn)
    );
    assert!(b.store.open_turns(Scope::Here).await.unwrap().is_empty());
    assert_eq!(b.store.open_turns(Scope::All).await.unwrap().len(), 1);
    assert!(b.store.jobs_open(Scope::Here).await.unwrap().is_empty());
    assert!(b.store.child_waits(Scope::Here).await.unwrap().is_empty());
    // B "acting" on the turn is refused by the fence; the journal is unchanged on both.
    assert!(b.store.finish_turn(id, turn, "superseded").await.is_err());
    assert!(
        b.store
            .step(
                id,
                turn,
                StepKind::Suspended,
                None,
                &serde_json::json!({"reason": "shutdown"})
            )
            .await
            .is_err()
    );
    assert_eq!(b.store.log_seq(), 0, "a replica logs nothing of its own");
    assert_eq!(
        a.store.open_turns(Scope::Here).await.unwrap().len(),
        1,
        "A's turn is A's to resume"
    );
    assert!(a.pull(&b).await.is_empty());
    a.close().await;
    b.close().await;
}

/// 0065: a contact's message consumed on A carries its typed provenance
/// (`messages.peer`) to a replica both ways one learns rows, the log and a
/// snapshot, though neither replica holds A's exchange record.
#[tokio::test]
async fn peer_provenance_rides_the_log_and_a_snapshot() {
    use agent::ShareMode;
    use agent::store::communication::AcceptExchange;
    use sessionsync::apply::{apply_snapshot, build_snapshot};
    let (a, b, c) = (engine("a").await, engine("b").await, engine("c").await);
    let chat = session_on(&a.store).await;
    let to = format!("session:{chat}");
    let r = a
        .store
        .accept_exchange(&AcceptExchange {
            sender: "peer:sky/ops",
            recipient: &to,
            sender_session: None,
            recipient_session: Some(chat),
            request_id: Some("r1"),
            reply_to: None,
            request_scope: None,
            generation: 1,
            body: "hi",
        })
        .await
        .unwrap();
    a.store
        .set_exchange_contact(r.id, ShareMode::Full, None)
        .await
        .unwrap();
    a.store
        .consume_exchange(r.id, chat, 1, genai::chat::ChatMessage::user("hi"))
        .await
        .unwrap()
        .unwrap();
    let want = a.store.peer_origins(chat).await.unwrap();
    assert_eq!(
        want.values()
            .map(|o| (o.id, o.contact_mode))
            .collect::<Vec<_>>(),
        [(r.id, Some(ShareMode::Full))]
    );

    b.pull(&a).await;
    assert!(b.store.consumed_exchanges(chat).await.unwrap().is_empty());
    assert_eq!(b.store.peer_origins(chat).await.unwrap(), want, "log");

    let snap = build_snapshot(&a.store, &a.id()).await.unwrap();
    apply_snapshot(&c.store, &snap).await.unwrap();
    assert!(c.store.consumed_exchanges(chat).await.unwrap().is_empty());
    assert_eq!(c.store.peer_origins(chat).await.unwrap(), want, "snapshot");
    a.close().await;
    b.close().await;
    c.close().await;
}

/// One `messages` INSERT captured on a table shaped as 2.20.1 has it (0064:
/// no 0065 `peer`), as that engine's `sqlite3session` writes it.
fn changeset_of_a_0064_message(session: &str, seq: i64, body: &str) -> Vec<u8> {
    use libsqlite3_sys as ffi;
    use std::ffi::CString;
    unsafe fn exec(db: *mut ffi::sqlite3, sql: &str) {
        let sql = CString::new(sql).unwrap();
        let rc = unsafe {
            ffi::sqlite3_exec(
                db,
                sql.as_ptr(),
                None,
                std::ptr::null_mut(),
                std::ptr::null_mut(),
            )
        };
        assert_eq!(rc, ffi::SQLITE_OK);
    }
    unsafe {
        let mut db = std::ptr::null_mut();
        assert_eq!(
            ffi::sqlite3_open(c":memory:".as_ptr(), &mut db),
            ffi::SQLITE_OK
        );
        exec(
            db,
            "CREATE TABLE messages (session_id TEXT NOT NULL, seq INTEGER NOT NULL, body TEXT NOT NULL, \
             excluded TEXT NULL, vision TEXT NULL, at TEXT NULL, thinking_ms INTEGER NULL, \
             advisory TEXT NULL, channel TEXT NULL, moved TEXT NULL, PRIMARY KEY (session_id, seq))",
        );
        let mut s = std::ptr::null_mut();
        assert_eq!(
            ffi::sqlite3session_create(db, c"main".as_ptr(), &mut s),
            ffi::SQLITE_OK
        );
        assert_eq!(
            ffi::sqlite3session_attach(s, c"messages".as_ptr()),
            ffi::SQLITE_OK
        );
        exec(
            db,
            &format!(
                "INSERT INTO messages (session_id, seq, body) VALUES ('{session}', {seq}, '{}')",
                body.replace('\'', "''")
            ),
        );
        let (mut n, mut p) = (0, std::ptr::null_mut());
        assert_eq!(
            ffi::sqlite3session_changeset(s, &mut n, &mut p),
            ffi::SQLITE_OK
        );
        let out = std::slice::from_raw_parts(p.cast::<u8>(), usize::try_from(n).unwrap()).to_vec();
        ffi::sqlite3_free(p);
        ffi::sqlite3session_delete(s);
        ffi::sqlite3_close(db);
        out
    }
}

/// A 2.20.1 peer's row (its `messages` one column short of 0065) still
/// applies on a 65 store: nothing is skipped, the row lands, `peer` is NULL.
#[tokio::test]
async fn a_pre_0065_peers_message_row_applies_with_no_provenance() {
    let (a, b) = (engine("na").await, engine("nb").await);
    let chat = session_on(&a.store).await;
    b.pull(&a).await;
    let body = serde_json::to_string(&genai::chat::ChatMessage::user("from 2.20.1")).unwrap();
    let row = sessionsync::types::LogRow {
        seq: u64::try_from(a.store.log_seq()).unwrap() + 1,
        session_id: chat.to_string(),
        epoch: 1,
        changeset: changeset_of_a_0064_message(&chat.to_string(), 0, &body),
        hlc: String::new(),
    };
    assert_eq!(
        apply_log_row(&b.store, &a.id(), &row).await.unwrap(),
        Applied::Applied
    );
    let got = b.store.messages(chat).await.unwrap();
    assert_eq!(got.len(), 1, "{got:?}");
    assert!(b.store.peer_origins(chat).await.unwrap().is_empty());
    a.close().await;
    b.close().await;
}
