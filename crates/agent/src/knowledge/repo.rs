//! Local git history of the bundle via `gix` — no `git` binary (design doc 10 §2.4).
//!
//! All functions are synchronous and blocking; the store calls them inside
//! `spawn_blocking`. Every call opens a thread-local view of the shared
//! repository, so a `Repo` may be used from any thread.

use std::path::{Path, PathBuf};

use anyhow::Context as _;
use gix::bstr::ByteSlice as _;
use gix::diff::blob::unified_diff::{ConsumeBinaryHunk, ContextSize};
use gix::diff::blob::{Algorithm, InternedInput, UnifiedDiff};
use gix::objs::tree::EntryKind;
use gix::{Commit, ObjectId, Repository};

use super::types::Change;

/// Committer identity; the user's git config is never read.
pub const SIGNATURE: (&str, &str) = ("wth", "wth@localhost");

/// A normal `.git` at the bundle root, loose objects only.
pub struct Repo {
    root: PathBuf,
    inner: gix::ThreadSafeRepository,
}

impl Repo {
    /// `gix::open(root)` or `gix::init(root)`, then [`Self::reconcile`].
    pub fn open(root: &Path) -> anyhow::Result<Self> {
        // Isolated: no global/system config, no environment — the bundle's
        // history must not depend on the user's git setup.
        let opts = gix::open::Options::isolated();
        let inner = match gix::ThreadSafeRepository::open_opts(root, opts.clone()) {
            Ok(repo) => repo,
            Err(_) => gix::ThreadSafeRepository::init_opts(
                root,
                gix::create::Kind::WithWorktree,
                gix::create::Options::default(),
                opts,
            )
            .with_context(|| format!("init knowledge repo at {}", root.display()))?,
        };
        let repo = Self {
            root: root.to_path_buf(),
            inner,
        };
        repo.reconcile()?;
        Ok(repo)
    }

    /// Hash every file under the root (skipping `.git/`), build the full tree;
    /// when it differs from HEAD's tree commit `reconcile` and return its
    /// abbreviated id.
    pub fn reconcile(&self) -> anyhow::Result<Option<String>> {
        let repo = self.inner.to_thread_local();
        let head = head_commit(&repo)?;
        let empty = ObjectId::empty_tree(repo.object_hash());
        let mut editor = repo.edit_tree(empty)?;
        let mut dirs = vec![self.root.clone()];
        while let Some(dir) = dirs.pop() {
            for entry in std::fs::read_dir(&dir)? {
                let path = entry?.path();
                if path.is_dir() {
                    if path.file_name() != Some(".git".as_ref()) {
                        dirs.push(path);
                    }
                } else if path.is_file() {
                    let blob = repo.write_blob(std::fs::read(&path)?)?;
                    editor.upsert(rel(path.strip_prefix(&self.root)?)?, EntryKind::Blob, blob)?;
                }
            }
        }
        let tree = editor.write()?.detach();
        if tree == base_tree(&repo, head.as_ref())? {
            return Ok(None);
        }
        commit(&repo, tree, head.map(|c| c.id), "reconcile").map(Some)
    }

    /// Commit `written` (read from disk) and `removed` (root-relative paths)
    /// on top of HEAD's tree; returns the abbreviated commit id. An unborn
    /// HEAD starts from the empty tree.
    pub fn commit(
        &self,
        written: &[PathBuf],
        removed: &[PathBuf],
        message: &str,
    ) -> anyhow::Result<String> {
        let repo = self.inner.to_thread_local();
        let head = head_commit(&repo)?;
        let base = base_tree(&repo, head.as_ref())?;
        let mut editor = repo.edit_tree(base)?;
        for path in written {
            let blob = repo.write_blob(std::fs::read(self.root.join(path))?)?;
            editor.upsert(rel(path)?, EntryKind::Blob, blob)?;
        }
        for path in removed {
            editor.remove(rel(path)?)?;
        }
        let tree = editor.write()?.detach();
        if let Some(head) = &head
            && tree == base
        {
            return Ok(abbrev(&head.id));
        }
        commit(&repo, tree, head.map(|c| c.id), message)
    }

    /// Commits that changed `path` (root-relative; `None` = every commit),
    /// newest first, at most `limit`.
    pub fn history(&self, path: Option<&Path>, limit: usize) -> anyhow::Result<Vec<Change>> {
        let repo = self.inner.to_thread_local();
        touching(&repo, path, limit)?
            .iter()
            .map(|c| {
                let time = chrono::DateTime::from_timestamp(c.time()?.seconds, 0)
                    .context("commit time out of range")?
                    .with_timezone(&chrono::Local);
                Ok(Change {
                    commit: abbrev(&c.id),
                    time: time.format("%Y-%m-%d %H:%M").to_string(),
                    message: c
                        .message_raw_sloppy()
                        .lines()
                        .next()
                        .unwrap_or_default()
                        .to_str_lossy()
                        .into_owned(),
                })
            })
            .collect()
    }

    /// The blob at `path` in `commit` (abbreviated or full id); `None` when
    /// the path is absent there.
    pub fn show(&self, path: &Path, commit: &str) -> anyhow::Result<Option<Vec<u8>>> {
        let repo = self.inner.to_thread_local();
        let tree = repo.rev_parse_single(commit)?.object()?.peel_to_tree()?;
        Ok(match tree.lookup_entry_by_path(path)? {
            Some(entry) => Some(entry.object()?.detach().data),
            None => None,
        })
    }

    /// Unified diff of the latest change to `path` (its newest commit vs that
    /// commit's first parent); `None` when the path has fewer than two states.
    pub fn last_diff(&self, path: &Path) -> anyhow::Result<Option<String>> {
        let repo = self.inner.to_thread_local();
        let commits = touching(&repo, Some(path), 1)?;
        let Some(commit) = commits.first() else {
            return Ok(None);
        };
        let Some(parent) = commit.parent_ids().next() else {
            return Ok(None);
        };
        let old = blob_data(&parent.object()?.into_commit(), path)?;
        let new = blob_data(commit, path)?;
        let input = InternedInput::new(old.as_slice(), new.as_slice());
        let diff = gix::diff::blob::diff_with_slider_heuristics(Algorithm::Histogram, &input);
        let hunks = UnifiedDiff::new(
            &diff,
            &input,
            ConsumeBinaryHunk::new(Vec::new(), "\n"),
            ContextSize::symmetrical(3),
        )
        .consume()?;
        let rel = rel(path)?;
        Ok(Some(format!(
            "--- a/{rel}\n+++ b/{rel}\n{}",
            String::from_utf8_lossy(&hunks)
        )))
    }

    /// Abbreviated id of the commit *before* the latest change to `path`
    /// (the `knowledge_restore` default); `None` when there is none.
    pub fn previous(&self, path: &Path) -> anyhow::Result<Option<String>> {
        let repo = self.inner.to_thread_local();
        Ok(touching(&repo, Some(path), 1)?
            .first()
            .and_then(|c| c.parent_ids().next())
            .map(|p| abbrev(&p)))
    }
}

/// 7-hex-char abbreviation; fixed width so ids stay stable in logs and messages.
fn abbrev(id: &gix::oid) -> String {
    id.to_hex_with_len(7).to_string()
}

/// A root-relative path as a `/`-separated tree path.
fn rel(path: &Path) -> anyhow::Result<String> {
    let mut out = String::new();
    for c in path.components() {
        let s = c
            .as_os_str()
            .to_str()
            .with_context(|| format!("non-UTF-8 path {}", path.display()))?;
        if !out.is_empty() {
            out.push('/');
        }
        out.push_str(s);
    }
    Ok(out)
}

/// HEAD's commit; `None` while HEAD is unborn.
fn head_commit(repo: &Repository) -> anyhow::Result<Option<Commit<'_>>> {
    Ok(match repo.head()?.id() {
        Some(id) => Some(id.object()?.into_commit()),
        None => None,
    })
}

/// HEAD's tree, or the empty tree while HEAD is unborn.
fn base_tree(repo: &Repository, head: Option<&Commit<'_>>) -> anyhow::Result<ObjectId> {
    Ok(match head {
        Some(c) => c.tree_id()?.detach(),
        None => ObjectId::empty_tree(repo.object_hash()),
    })
}

/// Write a commit as `wth <wth@localhost>` and advance HEAD; returns the abbreviated id.
fn commit(
    repo: &Repository,
    tree: ObjectId,
    parent: Option<ObjectId>,
    message: &str,
) -> anyhow::Result<String> {
    let time = gix::date::Time::now_local_or_utc().format(gix::date::time::Format::Raw)?;
    let sig = gix::actor::SignatureRef {
        name: SIGNATURE.0.into(),
        email: SIGNATURE.1.into(),
        time: &time,
    };
    let id = repo.commit_as(sig, sig, "HEAD", message, tree, parent)?;
    Ok(abbrev(&id))
}

/// The tree entry at `path` in `commit`'s tree; `None` when absent.
fn entry_at<'r>(
    commit: &Commit<'r>,
    path: &Path,
) -> anyhow::Result<Option<gix::object::tree::Entry<'r>>> {
    Ok(commit.tree()?.lookup_entry_by_path(path)?)
}

/// Blob bytes at `path` in `commit`; empty when absent.
fn blob_data(commit: &Commit<'_>, path: &Path) -> anyhow::Result<Vec<u8>> {
    Ok(match entry_at(commit, path)? {
        Some(entry) => entry.object()?.detach().data,
        None => Vec::new(),
    })
}

/// Commits reachable from HEAD that change `path` (every commit when `None`),
/// newest first, at most `limit`.
fn touching<'r>(
    repo: &'r Repository,
    path: Option<&Path>,
    limit: usize,
) -> anyhow::Result<Vec<Commit<'r>>> {
    let mut out = Vec::new();
    let Some(head) = head_commit(repo)? else {
        return Ok(out);
    };
    if limit == 0 {
        return Ok(out);
    }
    for info in repo.rev_walk([head.id]).all()? {
        let commit = info?.object()?;
        if let Some(path) = path {
            let now = entry_at(&commit, path)?.map(|e| e.object_id());
            let before = match commit.parent_ids().next() {
                Some(p) => entry_at(&p.object()?.into_commit(), path)?.map(|e| e.object_id()),
                None => None,
            };
            if now == before {
                continue;
            }
        }
        out.push(commit);
        if out.len() >= limit {
            break;
        }
    }
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn tmp() -> PathBuf {
        let dir = std::env::temp_dir().join(format!("wth-repo-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }

    fn put(root: &Path, rel: &str, body: &str) -> PathBuf {
        let path = root.join(rel);
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::write(path, body).unwrap();
        PathBuf::from(rel)
    }

    #[test]
    fn open_empty_dir_leaves_head_unborn() {
        let root = tmp();
        let repo = Repo::open(&root).unwrap();
        assert!(root.join(".git").is_dir());
        assert!(repo.history(None, 10).unwrap().is_empty());
        assert_eq!(repo.reconcile().unwrap(), None);
    }

    #[test]
    fn open_with_files_reconciles_once() {
        let root = tmp();
        put(&root, "a.md", "a\n");
        put(&root, "sub/b.md", "b\n");
        let repo = Repo::open(&root).unwrap();
        let history = repo.history(None, 10).unwrap();
        assert_eq!(history.len(), 1);
        assert_eq!(history[0].message, "reconcile");
        assert_eq!(history[0].commit.len(), 7);
        assert_eq!(
            repo.show(Path::new("sub/b.md"), &history[0].commit)
                .unwrap(),
            Some(b"b\n".to_vec())
        );
        // Nothing changed on disk: reopening does not add a commit.
        let repo = Repo::open(&root).unwrap();
        assert_eq!(repo.history(None, 10).unwrap().len(), 1);
        // Edited on disk while "down": exactly one reconcile commit.
        put(&root, "a.md", "a2\n");
        assert!(repo.reconcile().unwrap().is_some());
        assert_eq!(repo.history(None, 10).unwrap().len(), 2);
        assert_eq!(repo.reconcile().unwrap(), None);
    }

    #[test]
    fn commit_history_show_diff_previous() {
        let root = tmp();
        let repo = Repo::open(&root).unwrap();
        let a = put(&root, "a.md", "old\n");
        let first = repo.commit(std::slice::from_ref(&a), &[], "add a").unwrap();
        let b = put(&root, "b.md", "bee\n");
        let second = repo
            .commit(std::slice::from_ref(&b), &[], "add b\n\nbody")
            .unwrap();
        put(&root, "a.md", "new\n");
        let third = repo
            .commit(std::slice::from_ref(&a), &[], "edit a")
            .unwrap();
        assert_ne!(first, second);
        assert_ne!(second, third);

        let all = repo.history(None, 10).unwrap();
        let ids: Vec<_> = all.iter().map(|c| c.commit.as_str()).collect();
        assert_eq!(ids, [third.as_str(), second.as_str(), first.as_str()]);
        let messages: Vec<_> = all.iter().map(|c| c.message.as_str()).collect();
        assert_eq!(messages, ["edit a", "add b", "add a"]);
        assert!(
            all.iter()
                .all(|c| c.commit.len() == 7 && c.time.len() == 16)
        );
        assert_eq!(repo.history(None, 2).unwrap().len(), 2);

        let only_a: Vec<_> = repo
            .history(Some(&a), 10)
            .unwrap()
            .into_iter()
            .map(|c| c.commit)
            .collect();
        assert_eq!(only_a, [third.clone(), first.clone()]);
        let only_b: Vec<_> = repo
            .history(Some(&b), 10)
            .unwrap()
            .into_iter()
            .map(|c| c.commit)
            .collect();
        assert_eq!(only_b, std::slice::from_ref(&second));

        assert_eq!(repo.show(&a, &first).unwrap(), Some(b"old\n".to_vec()));
        assert_eq!(repo.show(&a, &third).unwrap(), Some(b"new\n".to_vec()));
        assert_eq!(repo.show(&b, &first).unwrap(), None);
        assert_eq!(repo.show(&b, &second).unwrap(), Some(b"bee\n".to_vec()));

        let diff = repo.last_diff(&a).unwrap().unwrap();
        assert!(diff.starts_with("--- a/a.md\n+++ b/a.md\n@@"), "{diff}");
        assert!(diff.contains("\n-old\n"), "{diff}");
        assert!(diff.contains("\n+new\n"), "{diff}");
        // b was added on top of an existing commit: old side is empty.
        let diff_b = repo.last_diff(&b).unwrap().unwrap();
        assert!(diff_b.contains("\n+bee\n"), "{diff_b}");
        assert_eq!(repo.last_diff(Path::new("nope.md")).unwrap(), None);

        assert_eq!(repo.previous(&a).unwrap(), Some(second.clone()));
        assert_eq!(repo.previous(&b).unwrap(), Some(first.clone()));

        // No effective change: HEAD is returned and nothing is committed.
        assert_eq!(
            repo.commit(std::slice::from_ref(&a), &[], "noop").unwrap(),
            third
        );
        assert_eq!(repo.history(None, 10).unwrap().len(), 3);

        // Removal shows up in the path's history; show is None there.
        std::fs::remove_file(root.join(&a)).unwrap();
        let fourth = repo.commit(&[], std::slice::from_ref(&a), "rm a").unwrap();
        assert_eq!(repo.show(&a, &fourth).unwrap(), None);
        assert_eq!(repo.history(Some(&a), 10).unwrap()[0].commit, fourth);
        assert_eq!(repo.previous(&a).unwrap(), Some(third));

        if let Ok(out) = std::process::Command::new("git")
            .args(["-C", root.to_str().unwrap(), "log", "--oneline"])
            .output()
            && out.status.success()
        {
            assert_eq!(out.stdout.lines().count(), 4);
        }
    }

    #[test]
    fn root_commit_has_no_diff_or_previous() {
        let root = tmp();
        let a = put(&root, "a.md", "only\n");
        let repo = Repo::open(&root).unwrap();
        assert_eq!(repo.last_diff(&a).unwrap(), None);
        assert_eq!(repo.previous(&a).unwrap(), None);
        assert_eq!(repo.history(Some(&a), 10).unwrap().len(), 1);
    }
}
