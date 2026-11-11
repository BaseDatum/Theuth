//! 191 P2 end to end on loopback (the design's rig cases): our machines are one mesh (paired
//! in their mesh stores; `replicate` plays the record replica between them), a contact's
//! machines are another, every engine its own endpoint, store and agent. A "stale" or
//! "offline" contact machine is one whose own roster state is written back to an older one.

use std::collections::{BTreeMap, BTreeSet};

use super::*;
use crate::contacts::{records, rosters};

/// Poll `cond` (it may `.await`) until it holds, 20 s at most.
macro_rules! until_true {
    ($what:expr, $cond:expr) => {{
        let mut ok = false;
        for _ in 0..400 {
            if $cond {
                ok = true;
                break;
            }
            tokio::time::sleep(Duration::from_millis(50)).await;
        }
        assert!(ok, "timed out waiting for {}", $what);
    }};
}

fn entry(id: &str) -> mesh::pairing::PeerEntry {
    mesh::pairing::PeerEntry {
        id: mesh::EngineId(id.to_owned()),
        name: id[..8].to_owned(),
        addrs: Vec::new(),
        hub_pref: mesh::HubPref::Eligible,
        added_ms: mesh::identity::now_ms(),
        removed_ms: None,
    }
}

/// `a` and `b` are machines of one mesh.
async fn mesh_pair(a: &Engine, b: &Engine) {
    a.net.merge_peers(&[entry(&b.eid())], None).await.unwrap();
    b.net.merge_peers(&[entry(&a.eid())], None).await.unwrap();
}

async fn mesh_add(a: &Engine, id: &str) {
    a.net.merge_peers(&[entry(id)], None).await.unwrap();
}

async fn mesh_remove(a: &Engine, id: &str) {
    a.net
        .remove_peer(&mesh::EngineId(id.to_owned()))
        .await
        .unwrap();
}

fn admits(e: &Engine, other: &str) -> bool {
    matches!(e.ctx.gate(other), IrohGate::Contact { .. })
}

fn fake_eid() -> String {
    iroh::SecretKey::generate().public().to_string()
}

/// The one live record of `e` and its roster state.
async fn state(e: &Engine) -> (String, Option<rosters::Local>) {
    let r = e.ctx.records().await.unwrap();
    let id = r
        .contacts
        .iter()
        .find(|c| c.removed_ms.is_none())
        .expect("a live record")
        .id
        .clone();
    let l = records::local_of(&r, &id);
    (id, l)
}

async fn version(e: &Engine) -> Option<u64> {
    state(e).await.1.map(|l| l.head().version)
}

/// `e`'s own heads row of its record: `(version, conflict)`.
async fn my_head(e: &Engine) -> Option<(u64, bool)> {
    let r = e.ctx.records().await.unwrap();
    r.heads
        .iter()
        .find(|h| h.machine.as_deref() == Some(e.eid().as_str()))
        .map(|h| (h.version.unwrap_or(0) as u64, h.conflict == Some(1)))
}

/// Write `l` back as `e`'s state (and its heads row): the machine was offline (stale). The
/// card fetches the last pushes started land first.
async fn rewind(e: &Engine, record: &str, l: &rosters::Local) {
    tokio::time::sleep(Duration::from_millis(1500)).await;
    let w = vec![
        records::local_row(record, l),
        records::head_row(record, &e.eid(), l, false),
    ];
    e.ctx
        .agents()
        .unwrap()
        .ask(agent::communication::network::WriteContactRecords(w))
        .await
        .unwrap();
    e.ctx.reconcile_now().await.unwrap();
}

/// Each of `engines` learns the others' direct addresses (what the relay, discovery or a
/// past connection gives a real machine; these engines have none of them), restaged.
async fn introduce(engines: &[&Engine]) {
    for a in engines {
        for b in engines.iter().filter(|b| b.eid() != a.eid()) {
            a.ctx
                .agents()
                .unwrap()
                .ask(agent::communication::network::SetContactAddrs {
                    eid: b.eid(),
                    addrs: b.addrs().iter().map(ToString::to_string).collect(),
                })
                .await
                .unwrap();
        }
        let _ = a.ctx.tick().await;
    }
}

/// Our n1 and n2 (one mesh) and a contact `c` paired with n1: `c` bases on n1's version 1
/// (n1 and n2), n2 gets the record, co-signs, and verifies `c` from its pin by a proof path.
async fn our_two_and(c: &Engine) -> (Engine, Engine) {
    let lan = Lan::default();
    let n1 = engine("n1", &lan, true, cfg(600)).await;
    let n2 = engine("n2", &lan, true, cfg(600)).await;
    mesh_pair(&n1, &n2).await;
    introduce(&[&n1, &n2, c]).await;
    paired(&n1, c).await;
    until_true!("c bases on n1's roster and admits n2", admits(c, &n2.eid()));
    replicate(&n1, &n2).await;
    assert!(n2.ctx.tick().await);
    assert!(
        !admits(&n2, &c.eid()),
        "based on n1, not verified on n2 yet: never the pin alone"
    );
    n2.ctx.roster_round().await;
    assert!(admits(&n2, &c.eid()), "n2 verified c from its pin");
    until_true!(
        "c has n2's co-signature",
        state(c)
            .await
            .1
            .is_some_and(|l| l.signers.contains(&n2.eid()))
    );
    (n1, n2)
}

/// Rig 4a and 8: removing the pairing machine n1 (on n2) signs a roster without it, pushed;
/// the contact refuses n1 and reaches n2. Offline across ten more versions (only the last
/// eight carried), it jumps to the first one a member it trusts signed and replays to the
/// latest: n1 and n2 back, the passing n5s refused.
#[tokio::test]
async fn removing_the_pairing_machine_leaves_the_others_and_a_stale_contact_catches_up() {
    let lan = Lan::default();
    let c = engine("c", &lan, true, cfg(600)).await;
    let (n1, n2) = our_two_and(&c).await;
    assert_eq!(version(&c).await, Some(1));
    mesh_remove(&n2, &n1.eid()).await;
    assert!(n2.ctx.tick().await, "a membership change signs");
    until_true!(
        "c accepted version 2 without n1",
        version(&c).await == Some(2) && !admits(&c, &n1.eid()) && admits(&c, &n2.eid())
    );
    assert_eq!(c.ctx.gate(&n1.eid()), IrohGate::Refuse);
    assert!(c.ctx.refresh_offers(&n2.eid()).await, "C reaches n2");

    // Case 8: n1 is back; ten changes (an n5 added, removed, five times): versions 3..13.
    mesh_add(&n2, &n1.eid()).await;
    assert!(n2.ctx.tick().await);
    let (record, Some(at3)) = ({
        until_true!("c at 3", version(&c).await == Some(3));
        state(&c).await
    }) else {
        panic!("c has a state")
    };
    let mut gone = Vec::new();
    for _ in 0..5 {
        let n5 = fake_eid();
        mesh_add(&n2, &n5).await;
        assert!(n2.ctx.tick().await);
        mesh_remove(&n2, &n5).await;
        assert!(n2.ctx.tick().await);
        gone.push(n5);
    }
    until_true!("the pushes landed", version(&c).await == Some(13));
    // C was offline all along: back at version 3. Its card from n2 carries 6..13 only.
    rewind(&c, &record, &at3).await;
    assert_eq!(version(&c).await, Some(3));
    let carried = n2.ctx.carried_to(&c.eid()).await.unwrap();
    let versions: BTreeSet<u64> = carried.chains[0]
        .versions
        .iter()
        .map(|v| v.version)
        .collect();
    assert_eq!(versions, (6..=13).collect(), "the last eight");
    assert!(c.ctx.refresh_offers(&n2.eid()).await);
    assert_eq!(version(&c).await, Some(13), "jumped to 6, replayed to 13");
    assert!(admits(&c, &n1.eid()) && admits(&c, &n2.eid()));
    for n5 in &gone {
        assert_eq!(c.ctx.gate(n5), IrohGate::Refuse);
    }
    // Our side keeps the hop chain from the pin plus the last eight, nothing else.
    let r = n2.ctx.records().await.unwrap();
    let kept: BTreeSet<u64> = records::chain_copies(&r, &state(&n2).await.0)
        .iter()
        .map(|c| c.version)
        .collect();
    assert!(
        kept.contains(&1) && kept.contains(&13) && !kept.contains(&4),
        "{kept:?}"
    );
}

/// Rig 9: a new contact machine c2 after our pin n1 retired: it has the record and c1's
/// heads row but no state, dials n2 found only through c1's untrusted hints, verifies the
/// proof path from the pin itself and ends at our latest: n2 admitted, n1 refused. With n2
/// gone too, a third machine cannot bootstrap: it denies, and never admits n1.
#[tokio::test]
async fn a_fresh_contact_machine_bootstraps_from_the_retired_pin_or_denies() {
    let lan = Lan::default();
    let c1 = engine("c1", &lan, true, cfg(600)).await;
    let (n1, n2) = our_two_and(&c1).await;
    mesh_remove(&n2, &n1.eid()).await;
    assert!(n2.ctx.tick().await);
    until_true!("c1 at version 2", version(&c1).await == Some(2));
    n1.net.shutdown().await;
    // c2 joins c1's mesh: c1's roster (signed for n2) now lists it.
    let c2 = engine("c2", &lan, true, cfg(600)).await;
    mesh_pair(&c1, &c2).await;
    introduce(&[&c1, &c2, &n2]).await;
    assert!(c1.ctx.tick().await);
    until_true!("n2 admits c2", admits(&n2, &c2.eid()));
    replicate(&c1, &c2).await;
    assert!(c2.ctx.tick().await);
    assert!(!admits(&c2, &n1.eid()) && !admits(&c2, &n2.eid()));
    assert_eq!(
        c2.ctx.contact_list()[0].problem.as_deref(),
        Some(rosters::NOT_VERIFIED)
    );
    c2.ctx.roster_round().await;
    assert_eq!(
        version(&c2).await,
        Some(2),
        "verified from the pin, through n2"
    );
    assert!(admits(&c2, &n2.eid()));
    assert_eq!(c2.ctx.gate(&n1.eid()), IrohGate::Refuse);
    // n2 gone too: a third machine cannot verify anything.
    n2.net.shutdown().await;
    let c3 = engine("c3", &lan, true, cfg(600)).await;
    mesh_pair(&c1, &c3).await;
    introduce(&[&c1, &c3]).await;
    replicate(&c1, &c3).await;
    assert!(c3.ctx.tick().await);
    c3.ctx.roster_round().await;
    assert_eq!(version(&c3).await, None);
    assert_eq!(
        c3.ctx.gate(&n1.eid()),
        IrohGate::Refuse,
        "never the retired pin"
    );
    assert_eq!(
        c3.ctx.contact_list()[0].problem.as_deref(),
        Some(rosters::NOT_VERIFIED)
    );
}

/// Rig 10: c3 took a rogue version 3 from the removed n1 while stale (our chain at 2): no
/// conflict while our chain stops short of 3 (c3 alone follows it: the limit). Once we sign
/// an honest version 3, c1's path reaches it with another hash: conflict, and every machine
/// of the contact denies, with the problem said. Version 3 asked of n2 before it existed is
/// an inflation observation there.
#[tokio::test]
async fn a_divergence_is_a_conflict_only_once_our_chain_reaches_it() {
    let lan = Lan::default();
    let c1 = engine("c1", &lan, true, cfg(600)).await;
    let c3 = engine("c3", &lan, true, cfg(600)).await;
    mesh_pair(&c1, &c3).await;
    let (n1, n2) = our_two_and(&c1).await;
    introduce(&[&n1, &n2, &c1, &c3]).await;
    replicate(&c1, &c3).await;
    assert!(c3.ctx.tick().await);
    c3.ctx.roster_round().await;
    assert_eq!(version(&c3).await, Some(1), "c3 verified from the pin");
    let (record, Some(at1)) = state(&c3).await else {
        panic!("a state")
    };
    mesh_remove(&n2, &n1.eid()).await;
    assert!(n2.ctx.tick().await);
    until_true!(
        "c1 and c3 at 2",
        version(&c1).await == Some(2) && version(&c3).await == Some(2)
    );
    // c3 was off: still at 1. The removed n1, modified, signs 3 = {n1, Z1}; c3 takes it.
    rewind(&c3, &record, &at1).await;
    let chain = at1.chain.clone();
    let rogue = rosters::sign(&chain, 3, &[n1.eid(), fake_eid()], &n1.eid(), &|m| {
        n1.net.sign(m)
    });
    let carried = rosters::Carried {
        chains: vec![rosters::carried(
            &chain,
            std::slice::from_ref(&rogue),
            &BTreeSet::new(),
            &BTreeMap::new(),
        )],
    };
    assert!(c3.ctx.ingest(&carried, &n1.eid()).await);
    assert_eq!(state(&c3).await.1.unwrap().head().hash, rogue.hash());
    // c1 sees c3's head: its path from n2 stops at 2 short of 3: no conflict.
    replicate(&c3, &c1).await;
    assert!(c1.ctx.tick().await);
    c1.ctx.roster_round().await;
    assert_eq!(my_head(&c1).await, Some((2, false)));
    assert!(
        admits(&c1, &n2.eid()),
        "no deny while our chain stops short"
    );
    // An honest version 3 on our side: c1 takes it; the poisoned c3 refuses the push.
    mesh_add(&n2, &fake_eid()).await;
    assert!(n2.ctx.tick().await);
    until_true!(
        "c1 at an honest 3",
        state(&c1)
            .await
            .1
            .is_some_and(|l| l.head().version == 3 && l.head().hash != rogue.hash())
    );
    c1.ctx.roster_round().await;
    assert_eq!(my_head(&c1).await, Some((3, true)), "conflict at 3");
    assert!(!admits(&c1, &n2.eid()));
    replicate(&c1, &c3).await;
    assert!(c3.ctx.tick().await);
    for c in [&c1, &c3] {
        assert_eq!(
            c.ctx.contact_list()[0].problem.as_deref(),
            Some(rosters::DISAGREE)
        );
        assert!(!admits(c, &n1.eid()) && !admits(c, &n2.eid()));
    }
    // n2 saw c1 ask for version 3 above its latest (2), then a head at 3 it never signed.
    assert!(n2.ctx.rosters.lock().seen.values().any(|(_, n)| *n >= 1));
}

/// Rig 12 and 13: n1 and n2 partitioned each sign a different version 2 (n5 through one, n6
/// through the other); a contact machine on each. Healed, the machines' heads differ at one
/// version: each fetches a path that holds both contents, the lower switches to the higher
/// hash, no conflict is ever set, and our side signs version 3 with n5 and n6. A machine
/// that missed both version-2 signings jumps from 1 to 3 through an old member's copy.
#[tokio::test]
async fn a_same_version_fork_converges_without_a_conflict() {
    let lan = Lan::default();
    let c1 = engine("c1", &lan, true, cfg(600)).await;
    let c2 = engine("c2", &lan, true, cfg(600)).await;
    mesh_pair(&c1, &c2).await;
    let (n1, n2) = our_two_and(&c1).await;
    introduce(&[&n1, &n2, &c1, &c2]).await;
    replicate(&c1, &c2).await;
    assert!(c2.ctx.tick().await);
    c2.ctx.roster_round().await;
    let (record, Some(at1)) = state(&c2).await else {
        panic!("a state")
    };
    assert_eq!(at1.head().version, 1);
    let (n5, n6) = (fake_eid(), fake_eid());
    mesh_add(&n1, &n5).await;
    mesh_add(&n2, &n6).await;
    assert!(n1.ctx.tick().await && n2.ctx.tick().await);
    let copies = |e: &Engine| {
        let e = e.ctx.clone();
        async move {
            let r = e.records().await.unwrap();
            records::chain_copies(&r, &r.contacts[0].id)
        }
    };
    let a = copies(&n1)
        .await
        .into_iter()
        .find(|c| c.version == 2)
        .unwrap();
    let b = copies(&n2)
        .await
        .into_iter()
        .find(|c| c.version == 2)
        .unwrap();
    assert_ne!(a.hash(), b.hash(), "a fork at version 2");
    let (low, high) = if a.hash() < b.hash() { (a, b) } else { (b, a) };
    until_true!(
        "both pushes in",
        version(&c1).await == Some(2) && version(&c2).await == Some(2)
    );
    // c1 holds the lower, c2 the higher (as if each heard one side only).
    let mut on_low = at1.clone();
    rosters::advance(&mut on_low, std::slice::from_ref(&low), &mut 0, None);
    rewind(&c1, &record, &on_low).await;
    let mut on_high = at1.clone();
    rosters::advance(&mut on_high, std::slice::from_ref(&high), &mut 0, None);
    rewind(&c2, &record, &on_high).await;
    // Healed: our rows meet.
    replicate(&n1, &n2).await;
    replicate(&n2, &n1).await;
    replicate(&c1, &c2).await;
    replicate(&c2, &c1).await;
    let _ = c1.ctx.tick().await;
    let _ = c2.ctx.tick().await;
    c1.ctx.roster_round().await;
    c2.ctx.roster_round().await;
    for c in [&c1, &c2] {
        assert_eq!(state(c).await.1.unwrap().head().hash, high.hash());
        assert_eq!(
            my_head(c).await.map(|h| h.1),
            Some(false),
            "never a conflict"
        );
        assert!(admits(c, &n1.eid()) && admits(c, &n2.eid()));
    }
    // Our side: both learn both newcomers and sign 3 with them.
    mesh_add(&n1, &n6).await;
    mesh_add(&n2, &n5).await;
    assert!(n1.ctx.tick().await);
    until_true!(
        "c1 and c2 at 3",
        version(&c1).await == Some(3) && version(&c2).await == Some(3)
    );
    let l = state(&c1).await.1.unwrap();
    assert!(l.head().members.contains(&n5) && l.head().members.contains(&n6));
    // Rig 13: c2 had missed both version-2 signings: from 1 it jumps to 3.
    rewind(&c2, &record, &at1).await;
    assert!(c2.ctx.refresh_offers(&n1.eid()).await);
    assert_eq!(version(&c2).await, Some(3));
    replicate(&c2, &c1).await;
    let _ = c1.ctx.tick().await;
    c1.ctx.roster_round().await;
    assert_eq!(my_head(&c1).await, Some((3, false)));
}

/// Rig 14: the pin n1, modified, holds a chain to version 20 our other machines never
/// signed. A fresh machine with a peer at our real head adopts nothing from it ("cannot
/// verify"); with no peer at all it follows the verified path (the limit class). A synced
/// n2 asked for version 20 twice raises the alert, and denies nothing.
#[tokio::test]
async fn an_inflated_path_from_the_pin_is_never_adopted_beside_a_peer_and_alerts() {
    use agent::store::contacts::RecordWrite;
    let lan = Lan::default();
    let c1 = engine("c1", &lan, true, cfg(600)).await;
    let c4 = engine("c4", &lan, true, cfg(600)).await;
    mesh_pair(&c1, &c4).await;
    let (n1, n2) = our_two_and(&c1).await;
    introduce(&[&n1, &n2, &c1, &c4]).await;
    let (chain, _) = state(&n1).await;
    // n1's own rows gain versions 2..20 of {n1, Z1}, signed by n1 (never replicated).
    let z1 = fake_eid();
    let rogue: Vec<RecordWrite> = (2..=20u64)
        .map(|v| {
            records::roster_row(&rosters::sign(
                &chain,
                v,
                &[n1.eid(), z1.clone()],
                &n1.eid(),
                &|m| n1.net.sign(m),
            ))
        })
        .collect();
    n1.ctx
        .agents()
        .unwrap()
        .ask(agent::communication::network::WriteContactRecords(rogue))
        .await
        .unwrap();
    // c4 beside c1 (at our real head, 1): the path from n1 verifies to 20, but no peer
    // holds it and n2's path agrees with nothing of it.
    replicate(&c1, &c4).await;
    assert!(c4.ctx.tick().await);
    c4.ctx.roster_round().await;
    assert_eq!(version(&c4).await, None);
    assert_eq!(
        c4.ctx.contact_list()[0].problem.as_deref(),
        Some(rosters::NOT_VERIFIED)
    );
    assert!(!admits(&c4, &n1.eid()));
    // A machine with no peer at all (nothing based): its pin's card leads it to 20.
    let c5 = engine("c5", &lan, true, cfg(600)).await;
    introduce(&[&c1, &c5]).await;
    replicate(&c1, &c5).await;
    let heads: Vec<RecordWrite> = c5
        .ctx
        .records()
        .await
        .unwrap()
        .heads
        .iter()
        .map(|h| RecordWrite::delete(agent::store::contacts::RecordTable::Heads, h.id.clone()))
        .collect();
    c5.ctx
        .agents()
        .unwrap()
        .ask(agent::communication::network::WriteContactRecords(heads))
        .await
        .unwrap();
    assert!(c5.ctx.tick().await);
    let from_pin = n1.ctx.carried_to(&c1.eid()).await.unwrap();
    assert!(c5.ctx.ingest(&from_pin, &n1.eid()).await);
    assert_eq!(version(&c5).await, Some(20), "the limit class");
    // The synced n2, asked twice (two refreshes apart) for 20 above its latest (1): alerted.
    let q = format!("chain={chain}&pin={}&from=0&to=20", n1.eid());
    n2.ctx.roster_path(&c1.eid(), &q).await.unwrap();
    for (at, _) in n2.ctx.rosters.lock().seen.values_mut() {
        *at -= crate::contacts::members::INFLATION_GAP;
    }
    n2.ctx.roster_path(&c1.eid(), &q).await.unwrap();
    let list = n2.ctx.contact_list();
    assert_eq!(
        list[0].problem.as_deref(),
        Some(rosters::inflated(&list[0].name).as_str())
    );
    let items = super::super::FirstContactItems(Arc::downgrade(&n2.ctx))
        .items()
        .await;
    assert!(items.iter().any(|i| i.id.starts_with("contact_roster:")));
    assert!(admits(&n2, &c1.eid()), "an alert only: nothing denied");
}

/// Rig 4b: a modified n1 signs {n1, Z1} at a higher version; the contact follows it (the
/// stated limit: n1 and Z1 admitted, n2 refused). The remedy: n1 removed from our mesh, the
/// contact removed on both sides, a fresh pairing from n2: a new record and a new chain at
/// version 1; n1 and Z1 stay refused, the old record is never revived and its rows go, and
/// its deny entries do not block the new pairing.
#[tokio::test]
async fn a_rogue_member_is_the_limit_and_the_remedy_is_a_fresh_pairing() {
    let lan = Lan::default();
    let c = engine("c", &lan, true, cfg(600)).await;
    let (n1, n2) = our_two_and(&c).await;
    let (old_chain, _) = state(&n1).await;
    let z1 = fake_eid();
    let rogue = rosters::sign(&old_chain, 2, &[n1.eid(), z1.clone()], &n1.eid(), &|m| {
        n1.net.sign(m)
    });
    let carried = rosters::Carried {
        chains: vec![rosters::carried(
            &old_chain,
            std::slice::from_ref(&rogue),
            &BTreeSet::new(),
            &BTreeMap::new(),
        )],
    };
    assert!(c.ctx.ingest(&carried, &n1.eid()).await);
    assert!(admits(&c, &n1.eid()) && admits(&c, &z1), "the stated limit");
    assert!(!admits(&c, &n2.eid()));
    // The remedy, on both sides.
    mesh_remove(&n2, &n1.eid()).await;
    let ours = n2.ctx.contact_list()[0].name.clone();
    n2.ctx
        .remove_contact(ContactRemove {
            name: ours,
            block: false,
        })
        .await
        .unwrap();
    let theirs = c.ctx.contact_list()[0].name.clone();
    c.ctx
        .remove_contact(ContactRemove {
            name: theirs,
            block: false,
        })
        .await
        .unwrap();
    let old_c = c.ctx.records().await.unwrap().contacts[0].id.clone();
    paired(&n2, &c).await;
    until_true!("c bases on the new chain", admits(&c, &n2.eid()));
    let r = c.ctx.records().await.unwrap();
    let fresh = r.contacts.iter().find(|x| x.removed_ms.is_none()).unwrap();
    assert_ne!(fresh.id, old_c, "a new record");
    assert!(
        r.contacts
            .iter()
            .any(|x| x.id == old_c && x.removed_ms.is_some())
    );
    let l = records::local_of(&r, &fresh.id).unwrap();
    assert_ne!(l.chain, old_chain, "a new chain");
    assert_eq!(
        (l.head().version, l.head().members.clone()),
        (1, vec![n2.eid()])
    );
    assert_eq!(c.ctx.gate(&n1.eid()), IrohGate::Refuse);
    assert_eq!(c.ctx.gate(&z1), IrohGate::Refuse);
    // Our side: the removed record's chain is never kept, signed or carried.
    let r = n2.ctx.records().await.unwrap();
    assert!(records::chain_copies(&r, &old_chain).is_empty());
}

/// Rig 7: a record from before any roster (C pinned n1 only, nothing based): a copy signed
/// by n2 alone is ignored (n2 is not its pin); n1's copy is its base, and then n1 and n2
/// are admitted.
#[tokio::test]
async fn an_existing_contact_bases_on_its_pins_copy_only() {
    use agent::store::contacts::{RecordTable, RecordWrite};
    let lan = Lan::default();
    let c = engine("c", &lan, true, cfg(600)).await;
    let (n1, n2) = our_two_and(&c).await;
    // Back to a P1 record on c: no state, no heads row.
    tokio::time::sleep(Duration::from_millis(1500)).await;
    let r = c.ctx.records().await.unwrap();
    let mut w: Vec<RecordWrite> = r
        .heads
        .iter()
        .map(|h| RecordWrite::delete(RecordTable::Heads, h.id.clone()))
        .collect();
    w.extend(
        r.local
            .iter()
            .map(|l| RecordWrite::delete(RecordTable::Local, l.record.clone())),
    );
    c.ctx
        .agents()
        .unwrap()
        .ask(agent::communication::network::WriteContactRecords(w))
        .await
        .unwrap();
    c.ctx.reconcile_now().await.unwrap();
    assert!(
        admits(&c, &n1.eid()) && !admits(&c, &n2.eid()),
        "its pin only"
    );
    let (chain, _) = state(&n2).await;
    let r = n2.ctx.records().await.unwrap();
    let by_n2: Vec<rosters::Copy> = records::chain_copies(&r, &chain)
        .into_iter()
        .filter(|x| x.signer == n2.eid())
        .collect();
    let only_n2 = rosters::Carried {
        chains: vec![rosters::carried(
            &chain,
            &by_n2,
            &BTreeSet::new(),
            &BTreeMap::new(),
        )],
    };
    assert!(
        !c.ctx.ingest(&only_n2, &n1.eid()).await,
        "n2 is not its pin"
    );
    assert_eq!(version(&c).await, None);
    let from_n1 = n1.ctx.carried_to(&c.eid()).await.unwrap();
    assert!(c.ctx.ingest(&from_n1, &n1.eid()).await);
    let l = state(&c).await.1.unwrap();
    assert_eq!((l.base.as_str(), l.head().version), (n1.eid().as_str(), 1));
    assert!(admits(&c, &n1.eid()) && admits(&c, &n2.eid()));
}

/// The proof-path route serves only the caller's own records from one of that record's
/// pins (another chain, a pin that is not one, a stranger: refused).
#[tokio::test]
async fn the_proof_path_serves_only_the_callers_own_record_from_its_pin() {
    use hyper::StatusCode;
    let lan = Lan::default();
    let c = engine("c", &lan, true, cfg(600)).await;
    let (n1, _n2) = our_two_and(&c).await;
    let (chain, _) = state(&n1).await;
    let ok = n1
        .ctx
        .roster_path(&c.eid(), &format!("chain={chain}&pin={}&from=0", n1.eid()))
        .await
        .unwrap();
    assert_eq!(
        (ok.chain.as_str(), ok.versions[0].version),
        (chain.as_str(), 1)
    );
    let refused = |q: String| {
        let (n1, c) = (n1.ctx.clone(), c.eid());
        async move { n1.roster_path(&c, &q).await.unwrap_err().0 }
    };
    assert_eq!(
        refused(format!("chain={chain}&pin={}&from=0", c.eid())).await,
        StatusCode::FORBIDDEN,
        "not a pin of that record"
    );
    assert_eq!(
        refused(format!(
            "chain={}&pin={}&from=0",
            uuid::Uuid::now_v7(),
            n1.eid()
        ))
        .await,
        StatusCode::NOT_FOUND,
        "not the caller's record"
    );
    let stranger = fake_eid();
    assert_eq!(
        n1.ctx
            .roster_path(&stranger, &format!("chain={chain}&pin={}", n1.eid()))
            .await
            .unwrap_err()
            .0,
        StatusCode::UNAUTHORIZED
    );
    // A push body over 192 KiB: 413, over the wire.
    let big = bytes::Bytes::from(vec![b' '; rosters::NOTICE_BYTES_MAX + 1]);
    let (s, _) = crate::contacts::offers::call_with(
        &c.net,
        &n1.eid(),
        hyper::Method::POST,
        crate::contacts::offers::NOTICE_PATH,
        Some(big),
    )
    .await
    .unwrap();
    assert_eq!(s, StatusCode::PAYLOAD_TOO_LARGE);
}

/// A stand-in contact machine: every JSON-RPC call answers `code` (or a result), counted.
struct Answers(Option<i32>, Arc<std::sync::atomic::AtomicUsize>);

impl mesh::net::ContactAlpn for Answers {
    fn admit(&self, _: &str) -> bool {
        true
    }

    fn stream(
        &self,
        _: String,
        stream: Box<dyn replica::ByteStream>,
    ) -> std::pin::Pin<Box<dyn Future<Output = ()> + Send>> {
        let (code, hits) = (self.0, self.1.clone());
        Box::pin(async move {
            let svc = hyper::service::service_fn(move |_req| {
                hits.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
                let body = match code {
                    Some(c) => serde_json::json!({"jsonrpc": "2.0", "id": "x",
                        "error": {"code": c, "message": "refused"}}),
                    None => serde_json::json!({"jsonrpc": "2.0", "id": "x", "result": {"ok": 1}}),
                };
                async move {
                    Ok::<_, std::convert::Infallible>(hyper::Response::new(
                        http_body_util::Full::new(bytes::Bytes::from(body.to_string())),
                    ))
                }
            });
            let _ = hyper::server::conn::http1::Builder::new()
                .serve_connection(hyper_util::rt::TokioIo::new(stream), svc)
                .await;
        })
    }
}

/// 191 (D3, S4): a member answering `NOT_OWNER` (or unreachable) moves the call to the next
/// member, which is then tried first; any other answer after admission is final (never sent
/// again elsewhere).
#[tokio::test]
async fn not_owner_moves_the_call_to_the_next_member() {
    let stand_in = |code: Option<i32>| async move {
        let dir = tempfile::tempdir().unwrap();
        let net = node("m", dir.path()).await;
        let hits = Arc::new(std::sync::atomic::AtomicUsize::new(0));
        net.set_contacts(Arc::new(Answers(code, hits.clone())));
        (net, hits, dir)
    };
    let (owner_elsewhere, h1, _d1) = stand_in(Some(agent::communication::mesh::NOT_OWNER)).await;
    let (owner, h2, _d2) = stand_in(None).await;
    let (busy, h3, _d3) = stand_in(Some(a2a_types::error_code::TASK_NOT_FOUND)).await;
    let dir = tempfile::tempdir().unwrap();
    let me = node("me", dir.path()).await;
    let addr = |n: &Arc<mesh::net::Net>| {
        n.a2a_reach()
            .0
            .iter()
            .filter_map(|a| a.parse().ok())
            .collect::<Vec<std::net::SocketAddr>>()
    };
    let t = reach::IrohTransport {
        net: me.clone(),
        eid: owner_elsewhere.id.0.clone(),
        hints: reach::Hints {
            addrs: addr(&owner_elsewhere),
            relay: None,
        },
        export: None,
        members: vec![owner.id.0.clone()],
        answered: Default::default(),
    };
    // The stand-ins' addresses, as the gate's hints would give them.
    struct Hints(Vec<(String, Vec<std::net::SocketAddr>)>);
    impl mesh::net::ContactAlpn for Hints {
        fn admit(&self, _: &str) -> bool {
            false
        }
        fn stream(
            &self,
            _: String,
            _: Box<dyn replica::ByteStream>,
        ) -> std::pin::Pin<Box<dyn Future<Output = ()> + Send>> {
            Box::pin(async {})
        }
        fn hints(&self, remote: &str) -> Vec<std::net::SocketAddr> {
            self.0
                .iter()
                .find(|(e, _)| e == remote)
                .map(|(_, a)| a.clone())
                .unwrap_or_default()
        }
    }
    me.set_contacts(Arc::new(Hints(vec![
        (owner.id.0.clone(), addr(&owner)),
        (busy.id.0.clone(), addr(&busy)),
    ])));
    let v = t
        .call(&Default::default(), "SendMessage", serde_json::json!({}))
        .await
        .unwrap();
    assert_eq!(v, serde_json::json!({"ok": 1}));
    assert_eq!(
        (
            h1.load(std::sync::atomic::Ordering::SeqCst),
            h2.load(std::sync::atomic::Ordering::SeqCst)
        ),
        (1, 1)
    );
    assert_eq!(t.answered.lock().as_deref(), Some(owner.id.0.as_str()));
    // The member that answered goes first next time.
    t.call(&Default::default(), "GetTask", serde_json::json!({}))
        .await
        .unwrap();
    assert_eq!(h1.load(std::sync::atomic::Ordering::SeqCst), 1);
    // An answer after admission (TASK_NOT_FOUND) is final: the next member is not tried.
    // (Both have a live connection by now, so the order is as given.)
    let transport = |members: Vec<String>| reach::IrohTransport {
        net: me.clone(),
        eid: busy.id.0.clone(),
        hints: reach::Hints::default(),
        export: None,
        members,
        answered: Default::default(),
    };
    let _ = transport(Vec::new())
        .call(&Default::default(), "GetTask", serde_json::json!({}))
        .await;
    let before = (
        h2.load(std::sync::atomic::Ordering::SeqCst),
        h3.load(std::sync::atomic::Ordering::SeqCst),
    );
    let e = transport(vec![owner.id.0.clone()])
        .call(&Default::default(), "GetTask", serde_json::json!({}))
        .await
        .unwrap_err();
    assert_eq!(e.code, a2a_types::error_code::TASK_NOT_FOUND);
    assert_eq!(
        (
            h2.load(std::sync::atomic::Ordering::SeqCst),
            h3.load(std::sync::atomic::Ordering::SeqCst)
        ),
        (before.0, before.1 + 1)
    );
}
