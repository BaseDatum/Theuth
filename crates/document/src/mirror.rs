//! The replicated mirror of the document store (69 §5, contract `69-contract-docs.md` §3).
//!
//! `<data_dir>/documents/mirror/` is flat — one `<id>.meta.json` and one `<id>.snapshot.yjs` per
//! document — because `replica::files::mirror` is flat, non-recursive and last-writer-wins on whole
//! bytes: it must never see the per-document directories (`updates.log` is fsynced per update and
//! truncated at each snapshot; kept `snapshots/` are history, not state). The store writes both files
//! on every snapshot and meta change ([`crate::store::Store::mirror_write`]); the replica ships them as
//! rows of `doc_files` and projects peers' rows back into the directory.
//!
//! **Merge-on-apply**: this watcher sees every file the replica projects and tells the actor
//! ([`MirrorChanged`]); the actor skips the store's own writes by sha ([`crate::store::Store::last_projected`])
//! and *merges* a foreign snapshot into the document (`apply_update` into the open `Doc`, or load →
//! apply → re-snapshot when closed) and a foreign `meta.json` by [`merge_meta`]. Foreign bytes never
//! overwrite a local file: a snapshot is a Yjs state, and two states join.

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

use kameo::actor::ActorRef;
use notify::Watcher;
use tokio::sync::mpsc;

use crate::actor::{DocumentActor, MirrorChanged};
use crate::meta::{DocId, DocMeta, Member, ShareLink};

const DEBOUNCE: Duration = Duration::from_millis(300);

/// Which mirror file a path is.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum MirrorFile {
    /// `<id>.meta.json`.
    Meta,
    /// `<id>.snapshot.yjs`.
    Snapshot,
}

/// `<id>.meta.json` / `<id>.snapshot.yjs` → its document and kind; temp files (`.`-prefixed) and
/// anything else are `None`.
pub fn parse(path: &Path) -> Option<(DocId, MirrorFile)> {
    let name = path.file_name()?.to_str()?;
    if name.starts_with('.') {
        return None;
    }
    if let Some(id) = name.strip_suffix(".meta.json") {
        return DocId::parse(id).map(|id| (id, MirrorFile::Meta));
    }
    if let Some(id) = name.strip_suffix(".snapshot.yjs") {
        return DocId::parse(id).map(|id| (id, MirrorFile::Snapshot));
    }
    None
}

/// The mirror file name of `id`.
pub fn file_name(id: &DocId, kind: MirrorFile) -> String {
    match kind {
        MirrorFile::Meta => format!("{id}.meta.json"),
        MirrorFile::Snapshot => format!("{id}.snapshot.yjs"),
    }
}

/// Watch `mirror/` (flat) for the process lifetime; each changed mirror file reaches the actor once
/// per 300 ms burst as [`MirrorChanged`].
pub fn start(mirror_dir: &Path, actor: ActorRef<DocumentActor>) -> anyhow::Result<()> {
    std::fs::create_dir_all(mirror_dir)?;
    let (tx, mut rx) = mpsc::unbounded_channel::<PathBuf>();
    let mut watcher = notify::recommended_watcher(move |res: notify::Result<notify::Event>| {
        if let Ok(ev) = res {
            for p in ev.paths {
                let _ = tx.send(p);
            }
        }
    })?;
    watcher.watch(mirror_dir, notify::RecursiveMode::NonRecursive)?;
    tokio::spawn(async move {
        let _watcher = watcher;
        let mut due: HashMap<PathBuf, Instant> = HashMap::new();
        loop {
            let next = due.values().min().copied();
            tokio::select! {
                p = rx.recv() => {
                    let Some(p) = p else { break };
                    if parse(&p).is_some() {
                        due.insert(p, Instant::now() + DEBOUNCE);
                    }
                }
                _ = async {
                    match next {
                        Some(t) => tokio::time::sleep_until(tokio::time::Instant::from_std(t)).await,
                        None => std::future::pending().await,
                    }
                } => {
                    let now = Instant::now();
                    let ready: Vec<PathBuf> = due.iter().filter(|(_, t)| **t <= now).map(|(p, _)| p.clone()).collect();
                    for p in ready {
                        due.remove(&p);
                        let _ = actor.tell(MirrorChanged { path: p }).await;
                    }
                }
            }
        }
    });
    Ok(())
}

/// The `meta.json` merge rule (contract §3.2), applied to `local` in place; `true` when it changed.
///
/// - scalars (`title`, `kind`, `binding`, `derived`, `fragment`, `agent`, `created_ms`) come from the
///   side with the newer `updated_ms`; `updated_ms` becomes the max;
/// - `members`: union by `pub`; both sides → the one with the greater `max(added_ms, last_seen_ms)`,
///   ties → local; `revoked` (Z184-R2): union by `pub`, the greatest time wins, and a member whose
///   `added_ms` is not later than its key's revoke is dropped (revocation is monotone);
/// - `links`: union by `id`; both sides → a set `revoked_ms` wins (revocation is monotone), else the
///   greater `created_ms`, ties → local; the seal (`secret_hash_sealed`) is local's if it `opens`,
///   else incoming's if that opens, else local's unless empty (184 §7.4: a live seal is never
///   replaced by a dead one, a dead one gives way to one that opens);
/// - `authors`: union; both sides → the newer-`updated_ms` side's entry;
/// - `home`/`home_ms` (184 §9.1, W-a): the greater `(home_ms, home)` wins, `None` below any id;
/// - `serve_epoch`/`served_by` (184 §4.4): the greater `(serve_epoch, served_by)` wins;
/// - `last_export` stays local (the derived file is this engine's).
///
/// `opens` says whether a link's `secret_hash_sealed` opens under this engine's current key.
/// `id` must match; a mismatch is a caller bug and changes nothing.
pub fn merge_meta(
    local: &mut DocMeta,
    incoming: &DocMeta,
    opens: &dyn Fn(&ShareLink) -> bool,
) -> bool {
    if local.id != incoming.id {
        return false;
    }
    let before = local.clone();
    let theirs_newer = incoming.updated_ms > local.updated_ms;
    if theirs_newer {
        local.title = incoming.title.clone();
        local.kind = incoming.kind;
        local.binding = incoming.binding.clone();
        local.derived = incoming.derived.clone();
        local.fragment = incoming.fragment.clone();
        local.agent = incoming.agent.clone();
        local.created_ms = incoming.created_ms;
        local.updated_ms = incoming.updated_ms;
    }
    for m in &incoming.members {
        match local.members.iter_mut().find(|x| x.pub_ == m.pub_) {
            Some(mine) => {
                if member_clock(m) > member_clock(mine) {
                    *mine = m.clone();
                }
            }
            None => local.members.push(m.clone()),
        }
    }
    // Z184-R2: a revoke is monotone (greatest time wins) and drops every member admitted no later.
    for (pub_, at) in &incoming.revoked {
        let mine = local.revoked.entry(pub_.clone()).or_insert(*at);
        *mine = (*mine).max(*at);
    }
    let revoked = &local.revoked;
    local
        .members
        .retain(|m| revoked.get(&m.pub_).is_none_or(|at| m.added_ms > *at));
    for l in &incoming.links {
        match local.links.iter_mut().find(|x| x.id == l.id) {
            Some(mine) => {
                let take = match (mine.revoked_ms, l.revoked_ms) {
                    (None, Some(_)) => true,
                    (Some(_), None) => false,
                    _ => l.created_ms > mine.created_ms,
                };
                let hash = if !mine.secret_hash_sealed.is_empty() && opens(mine) {
                    mine.secret_hash_sealed.clone()
                } else if (!l.secret_hash_sealed.is_empty() && opens(l))
                    || mine.secret_hash_sealed.is_empty()
                {
                    l.secret_hash_sealed.clone()
                } else {
                    mine.secret_hash_sealed.clone()
                };
                if take {
                    *mine = l.clone();
                }
                mine.secret_hash_sealed = hash;
            }
            None => local.links.push(l.clone()),
        }
    }
    for (block, author) in &incoming.authors {
        match local.authors.get(block) {
            Some(_) if !theirs_newer => {}
            _ => {
                local.authors.insert(block.clone(), author.clone());
            }
        }
    }
    if home_key(incoming) > home_key(local) {
        local.home = incoming.home.clone();
        local.home_ms = incoming.home_ms;
    }
    if fence_key(incoming) > fence_key(local) {
        local.serve_epoch = incoming.serve_epoch;
        local.served_by = incoming.served_by.clone();
    }
    *local != before
}

/// 184 W-a: the home register's order, `None` below any id.
fn home_key(m: &DocMeta) -> (i64, Option<&str>) {
    (m.home_ms, m.home.as_ref().map(|e| e.0.as_str()))
}

/// 184 §4.4: the fence's order.
fn fence_key(m: &DocMeta) -> (i64, Option<&str>) {
    (m.serve_epoch, m.served_by.as_ref().map(|e| e.0.as_str()))
}

fn member_clock(m: &Member) -> i64 {
    m.added_ms.max(m.last_seen_ms)
}

/// The link the merge keeps for tests and the doc: see [`merge_meta`].
#[cfg(test)]
fn link(id: &str, created_ms: i64, revoked_ms: Option<i64>, hash: &str) -> crate::meta::ShareLink {
    crate::meta::ShareLink {
        id: id.to_owned(),
        role: crate::meta::Role::Editor,
        secret_hash_sealed: hash.to_owned(),
        name: None,
        created_ms,
        expires_ms: None,
        revoked_ms,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::meta::{AgentStyle, Binding, Role};

    fn meta(id: &DocId, title: &str, updated_ms: i64) -> DocMeta {
        DocMeta {
            id: id.clone(),
            kind: crate::meta::DocKind::Note,
            title: title.into(),
            binding: Binding::Note,
            derived: format!("documents/{id}/note.md"),
            fragment: "document-store".into(),
            members: vec![],
            links: vec![],
            agent: AgentStyle::default(),
            last_export: Some("local".into()),
            authors: Default::default(),
            created_ms: 1,
            updated_ms,
            home: None,
            home_ms: 0,
            serve_epoch: 0,
            served_by: None,
            revoked: Default::default(),
        }
    }

    fn member(pub_: &str, name: &str, added_ms: i64, last_seen_ms: i64) -> Member {
        Member {
            pub_: pub_.into(),
            name: name.into(),
            color: String::new(),
            role: Role::Editor,
            added_ms,
            via: None,
            last_seen_ms,
        }
    }

    #[test]
    fn mirror_names_parse_and_skip_temp_files() {
        let id = DocId::for_binding(&Binding::Knowledge("a".into()));
        let dir = Path::new("/x/mirror");
        assert_eq!(
            parse(&dir.join(file_name(&id, MirrorFile::Meta))),
            Some((id.clone(), MirrorFile::Meta))
        );
        assert_eq!(
            parse(&dir.join(file_name(&id, MirrorFile::Snapshot))),
            Some((id.clone(), MirrorFile::Snapshot))
        );
        assert_eq!(parse(&dir.join(format!(".{id}.snapshot.yjs.tmp-1"))), None);
        assert_eq!(parse(&dir.join("index.sqlite")), None);
        assert_eq!(parse(&dir.join("nope.meta.json")), None);
    }

    #[test]
    fn meta_merge_is_a_join_that_keeps_local_export_state() {
        let id = DocId::new();
        let mut local = meta(&id, "old", 10);
        local.members = vec![member("a", "Ann", 1, 5), member("b", "Bob", 2, 2)];
        local.links = vec![link("l1", 1, None, "hash1"), link("l2", 2, None, "hash2")];
        local.authors.insert("blk1".into(), "a".into());

        let mut incoming = meta(&id, "new", 20);
        incoming.last_export = Some("theirs".into());
        // Ann seen later elsewhere; Bob unchanged; Cid new.
        incoming.members = vec![
            member("a", "Ann R", 1, 9),
            member("b", "Bobby", 2, 1),
            member("c", "Cid", 3, 3),
        ];
        // l1 revoked elsewhere (hash blanked on the wire); l3 new.
        incoming.links = vec![link("l1", 1, Some(7), ""), link("l3", 3, None, "hash3")];
        incoming.authors.insert("blk1".into(), "c".into());
        incoming.authors.insert("blk2".into(), "c".into());

        assert!(merge_meta(&mut local, &incoming, &|_| false));
        assert_eq!(local.title, "new");
        assert_eq!(local.updated_ms, 20);
        assert_eq!(local.last_export.as_deref(), Some("local"));
        let names: Vec<&str> = local.members.iter().map(|m| m.name.as_str()).collect();
        assert_eq!(names, vec!["Ann R", "Bob", "Cid"]);
        let l1 = local.links.iter().find(|l| l.id == "l1").unwrap();
        assert_eq!(l1.revoked_ms, Some(7));
        assert_eq!(
            l1.secret_hash_sealed, "hash1",
            "a blanked wire hash never erases the key"
        );
        assert_eq!(local.links.len(), 3);
        assert_eq!(local.authors["blk1"], "c");
        assert_eq!(local.authors["blk2"], "c");

        // Older incoming: scalars and authors stay, new members/links still join; a revoke still lands.
        let mut older = meta(&id, "older", 5);
        older.members = vec![member("d", "Dee", 4, 4)];
        older.links = vec![link("l2", 2, Some(8), "")];
        older.authors.insert("blk1".into(), "d".into());
        assert!(merge_meta(&mut local, &older, &|_| false));
        assert_eq!(local.title, "new");
        assert_eq!(local.authors["blk1"], "c");
        assert_eq!(local.members.len(), 4);
        assert_eq!(
            local
                .links
                .iter()
                .find(|l| l.id == "l2")
                .unwrap()
                .revoked_ms,
            Some(8)
        );

        // Idempotent.
        let snap = local.clone();
        assert!(!merge_meta(&mut local, &incoming, &|_| false));
        assert!(!merge_meta(&mut local, &older, &|_| false));
        assert_eq!(local, snap);
        // Wrong id: untouched.
        assert!(!merge_meta(
            &mut local,
            &meta(&DocId::new(), "x", 99),
            &|_| false
        ));
    }

    /// 184 §9.1, §7.4: home and fence are registers with their own clocks, independent of
    /// `updated_ms`; the seal that opens wins over a dead one, never the other way round.
    #[test]
    fn meta_merge_home_fence_and_the_seal_that_opens() {
        let id = DocId::new();
        let e = |s: &str| Some(mesh::EngineId(s.into()));
        let mut local = meta(&id, "t", 50);
        local.home = e("a");
        local.home_ms = 10;
        local.serve_epoch = 3;
        local.served_by = e("a");
        local.links = vec![link("l1", 1, None, "dead"), link("l2", 1, None, "live")];

        // Older `updated_ms`, newer home and fence: both registers still move.
        let mut incoming = meta(&id, "t", 5);
        incoming.home = e("b");
        incoming.home_ms = 11;
        incoming.serve_epoch = 4;
        incoming.served_by = e("b");
        incoming.links = vec![link("l1", 1, None, "fresh"), link("l2", 1, None, "stale")];
        let opens = |l: &ShareLink| matches!(l.secret_hash_sealed.as_str(), "live" | "fresh");
        assert!(merge_meta(&mut local, &incoming, &opens));
        assert_eq!((local.home.clone(), local.home_ms), (e("b"), 11));
        assert_eq!((local.serve_epoch, local.served_by.clone()), (4, e("b")));
        assert_eq!(local.updated_ms, 50, "scalars still follow updated_ms");
        let seal = |m: &DocMeta, id: &str| {
            m.links
                .iter()
                .find(|l| l.id == id)
                .unwrap()
                .secret_hash_sealed
                .clone()
        };
        assert_eq!(
            seal(&local, "l1"),
            "fresh",
            "a seal that opens replaces a dead one"
        );
        assert_eq!(seal(&local, "l2"), "live", "a live seal is never replaced");

        // Ties: the greater engine id wins; an absent home (old engine's copy) never beats a set one.
        let mut tie = meta(&id, "t", 5);
        tie.home = e("a");
        tie.home_ms = 11;
        tie.serve_epoch = 4;
        tie.served_by = e("c");
        assert!(merge_meta(&mut local, &tie, &opens));
        assert_eq!(local.home, e("b"), "home tie: b > a");
        assert_eq!(local.served_by, e("c"), "fence tie: c > b");
        let old = meta(&id, "t", 99);
        merge_meta(&mut local, &old, &opens);
        assert_eq!((local.home.clone(), local.serve_epoch), (e("b"), 4));

        // Neither opens: today's rule (local unless empty).
        let mut a = meta(&id, "t", 1);
        a.links = vec![link("l1", 1, None, "x")];
        let mut b = meta(&id, "t", 1);
        b.links = vec![link("l1", 1, None, "y")];
        merge_meta(&mut a, &b, &|_| false);
        assert_eq!(seal(&a, "l1"), "x");
    }

    /// Z184-R2: a revoke is a tombstone. Another engine's copy that still lists the member (the
    /// replica, a hand-back, a Handover) never brings it back, in either merge direction; the
    /// greatest revoke time wins; a later join (newer `added_ms`) stays.
    #[test]
    fn a_revoked_member_stays_revoked_until_it_joins_again() {
        let id = DocId::new();
        let mut server = meta(&id, "t", 10);
        server.revoked.insert("g".into(), 50);
        let mut stale = meta(&id, "t", 20);
        stale.members = vec![member("g", "G", 5, 40), member("h", "H", 6, 6)];

        let mut s = server.clone();
        assert!(merge_meta(&mut s, &stale, &|_| false));
        let names = |m: &DocMeta| m.members.iter().map(|x| x.pub_.clone()).collect::<Vec<_>>();
        assert_eq!(names(&s), ["h"], "the server keeps G out");
        let mut other = stale.clone();
        assert!(merge_meta(&mut other, &server, &|_| false));
        assert_eq!(names(&other), ["h"], "the stale copy drops G");
        assert_eq!(other.revoked["g"], 50);

        let mut older = meta(&id, "t", 1);
        older.revoked.insert("g".into(), 30);
        merge_meta(&mut other, &older, &|_| false);
        assert_eq!(other.revoked["g"], 50, "the greatest revoke time wins");

        // G joins again later: its new membership survives every copy's tombstone.
        let mut rejoined = meta(&id, "t", 60);
        rejoined.members = vec![member("g", "G", 51, 51)];
        assert!(merge_meta(&mut other, &rejoined, &|_| false));
        assert_eq!(names(&other), ["h", "g"]);
    }
}
