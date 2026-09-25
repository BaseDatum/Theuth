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
use crate::meta::{DocId, DocMeta, Member};

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
///   ties → local;
/// - `links`: union by `id`; both sides → a set `revoked_ms` wins (revocation is monotone), else the
///   greater `created_ms`, ties → local; an empty `secret_hash_sealed` never replaces a set one;
/// - `authors`: union; both sides → the newer-`updated_ms` side's entry;
/// - `last_export` stays local (the derived file is this engine's).
///
/// `id` must match; a mismatch is a caller bug and changes nothing.
pub fn merge_meta(local: &mut DocMeta, incoming: &DocMeta) -> bool {
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
    for l in &incoming.links {
        match local.links.iter_mut().find(|x| x.id == l.id) {
            Some(mine) => {
                let take = match (mine.revoked_ms, l.revoked_ms) {
                    (None, Some(_)) => true,
                    (Some(_), None) => false,
                    _ => l.created_ms > mine.created_ms,
                };
                let hash = if mine.secret_hash_sealed.is_empty() { l.secret_hash_sealed.clone() } else { mine.secret_hash_sealed.clone() };
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
    *local != before
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
        assert_eq!(parse(&dir.join(file_name(&id, MirrorFile::Meta))), Some((id.clone(), MirrorFile::Meta)));
        assert_eq!(parse(&dir.join(file_name(&id, MirrorFile::Snapshot))), Some((id.clone(), MirrorFile::Snapshot)));
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
        incoming.members = vec![member("a", "Ann R", 1, 9), member("b", "Bobby", 2, 1), member("c", "Cid", 3, 3)];
        // l1 revoked elsewhere (hash blanked on the wire); l3 new.
        incoming.links = vec![link("l1", 1, Some(7), ""), link("l3", 3, None, "hash3")];
        incoming.authors.insert("blk1".into(), "c".into());
        incoming.authors.insert("blk2".into(), "c".into());

        assert!(merge_meta(&mut local, &incoming));
        assert_eq!(local.title, "new");
        assert_eq!(local.updated_ms, 20);
        assert_eq!(local.last_export.as_deref(), Some("local"));
        let names: Vec<&str> = local.members.iter().map(|m| m.name.as_str()).collect();
        assert_eq!(names, vec!["Ann R", "Bob", "Cid"]);
        let l1 = local.links.iter().find(|l| l.id == "l1").unwrap();
        assert_eq!(l1.revoked_ms, Some(7));
        assert_eq!(l1.secret_hash_sealed, "hash1", "a blanked wire hash never erases the key");
        assert_eq!(local.links.len(), 3);
        assert_eq!(local.authors["blk1"], "c");
        assert_eq!(local.authors["blk2"], "c");

        // Older incoming: scalars and authors stay, new members/links still join; a revoke still lands.
        let mut older = meta(&id, "older", 5);
        older.members = vec![member("d", "Dee", 4, 4)];
        older.links = vec![link("l2", 2, Some(8), "")];
        older.authors.insert("blk1".into(), "d".into());
        assert!(merge_meta(&mut local, &older));
        assert_eq!(local.title, "new");
        assert_eq!(local.authors["blk1"], "c");
        assert_eq!(local.members.len(), 4);
        assert_eq!(local.links.iter().find(|l| l.id == "l2").unwrap().revoked_ms, Some(8));

        // Idempotent.
        let snap = local.clone();
        assert!(!merge_meta(&mut local, &incoming));
        assert!(!merge_meta(&mut local, &older));
        assert_eq!(local, snap);
        // Wrong id: untouched.
        assert!(!merge_meta(&mut local, &meta(&DocId::new(), "x", 99)));
    }
}
