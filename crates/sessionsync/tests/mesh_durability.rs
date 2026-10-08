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
