//! Git signals from `gix` (60 §0.11): changed paths with line hunks for a
//! revision range or the working tree (`code_impact`), and the first-parent
//! co-change walk that fills `cochanges`. Nothing here writes to the repo.

use std::collections::{HashMap, HashSet};
use std::path::Path;

use anyhow::{Context, bail};
use gix::bstr::{BStr, ByteSlice};
use gix::diff::blob::{Algorithm, InternedInput};
use gix::{ObjectId, Repository};

/// Commits touching this many indexed files count toward co-change (qartez's bounds, 60 §0.11).
const COCHANGE_FILES: std::ops::RangeInclusive<usize> = 2..=20;
/// Object cache for the tree diffs of the co-change walk.
const CACHE_BYTES: usize = 16 * 1024 * 1024;
/// Bytes sniffed for a NUL before diffing a blob by lines.
const SNIFF: usize = 8192;

/// The repository owning an index root, with the root's path prefix inside
/// the work tree (the root may be a subdirectory of the checkout).
pub struct Git {
    repo: Repository,
    /// `""` or `sub/dir/`.
    prefix: String,
}

/// What `code_impact` diffs.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Range {
    /// HEAD against the working tree (`WORKTREE`).
    Worktree,
    /// `old` tree (a commit; `None` = the empty tree) → `new` commit.
    Commits {
        /// Old side; `None` for a root commit.
        old: Option<ObjectId>,
        /// New side.
        new: ObjectId,
    },
}

/// New-side lines of one hunk (1-based, inclusive).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Span {
    /// Lines added or replaced.
    Lines(i64, i64),
    /// A pure deletion after this line: the innermost definition containing it changed.
    Point(i64),
    /// The whole file (added, or binary).
    All,
}

/// How a path changed.
#[derive(Clone, Copy, Debug, PartialEq, Eq, serde::Serialize)]
#[serde(rename_all = "lowercase")]
pub enum Status {
    /// Not in the old tree.
    Added,
    /// In both, different blob.
    Modified,
    /// Not in the new tree.
    Deleted,
}

/// One changed file, root-relative.
#[derive(Clone, Debug)]
pub struct Change {
    /// Root-relative, forward slashes.
    pub path: String,
    /// How.
    pub status: Status,
    /// Empty for a deletion.
    pub spans: Vec<Span>,
    /// New-side bytes (the range's `new` blob or the file on disk), so the
    /// caller can map spans onto *that* text; `None` for a deletion.
    pub new: Option<Vec<u8>>,
}

/// `(path, status, old blob, new blob)` of one tree-diff entry.
type TreeChange = (String, Status, Option<ObjectId>, Option<ObjectId>);
/// A `cochanges` row: `(file_a, file_b, count)`.
pub type Pair = (i64, i64, i64);

impl Git {
    /// The repository containing `root` (canonical), or `None` when there is none.
    pub fn open(root: &Path) -> anyhow::Result<Option<Self>> {
        let Ok(mut repo) = gix::discover(root) else {
            return Ok(None);
        };
        let Some(workdir) = repo.workdir().map(Path::to_path_buf) else {
            return Ok(None);
        };
        let workdir = workdir.canonicalize().unwrap_or(workdir);
        let rel = root
            .strip_prefix(&workdir)
            .with_context(|| format!("{} is outside {}", root.display(), workdir.display()))?;
        let mut prefix = crate::root::rel_str(rel);
        if !prefix.is_empty() {
            prefix.push('/');
        }
        repo.object_cache_size_if_unset(CACHE_BYTES);
        Ok(Some(Self { repo, prefix }))
    }

    /// HEAD's commit id; `None` while HEAD is unborn.
    pub fn head(&self) -> Option<ObjectId> {
        self.repo.head_id().ok().map(|id| id.detach())
    }

    /// Tree path → root-relative path, `None` outside the root.
    fn rel(&self, loc: &BStr) -> Option<String> {
        loc.to_str()
            .ok()?
            .strip_prefix(self.prefix.as_str())
            .map(str::to_owned)
    }

    fn rev(&self, spec: &str) -> anyhow::Result<ObjectId> {
        let spec = if spec.is_empty() { "HEAD" } else { spec };
        Ok(self
            .repo
            .rev_parse_single(spec)
            .with_context(|| format!("unknown revision {spec:?}"))?
            .object()?
            .peel_to_commit()
            .with_context(|| format!("{spec:?} is not a commit"))?
            .id)
    }

    /// `A..B`, `A...B` (merge-base..B), a single `R` (`R^..R`), or `WORKTREE`.
    /// An empty side is HEAD.
    pub fn range(&self, spec: &str) -> anyhow::Result<Range> {
        let spec = spec.trim();
        if spec == "WORKTREE" {
            return Ok(Range::Worktree);
        }
        if spec.is_empty() {
            bail!("git_range is empty");
        }
        if let Some((a, b)) = spec.split_once("...") {
            let (a, b) = (self.rev(a)?, self.rev(b)?);
            let base = self
                .repo
                .merge_base(a, b)?
                .with_context(|| format!("no merge base for {spec}"))?
                .detach();
            return Ok(Range::Commits {
                old: Some(base),
                new: b,
            });
        }
        if let Some((a, b)) = spec.split_once("..") {
            return Ok(Range::Commits {
                old: Some(self.rev(a)?),
                new: self.rev(b)?,
            });
        }
        let new = self.rev(spec)?;
        let old = self
            .repo
            .find_object(new)?
            .try_into_commit()?
            .parent_ids()
            .next()
            .map(|p| p.detach());
        Ok(Range::Commits { old, new })
    }

    fn tree(&self, commit: Option<ObjectId>) -> anyhow::Result<gix::Tree<'_>> {
        Ok(match commit {
            Some(id) => self.repo.find_object(id)?.try_into_commit()?.tree()?,
            None => self.repo.empty_tree(),
        })
    }

    /// Blob changes between two trees, root-relative.
    fn tree_changes(
        &self,
        old: &gix::Tree<'_>,
        new: &gix::Tree<'_>,
    ) -> anyhow::Result<Vec<TreeChange>> {
        let mut opts = gix::diff::Options::default();
        opts.track_rewrites(None);
        use gix::diff::tree_with_rewrites::Change as C;
        let mut out = Vec::new();
        for ch in self.repo.diff_tree_to_tree(old, new, opts)? {
            if !ch.entry_mode().is_blob() {
                continue;
            }
            let Some(path) = self.rel(ch.location()) else {
                continue;
            };
            out.push(match &ch {
                C::Addition { id, .. } => (path, Status::Added, None, Some(*id)),
                C::Deletion { id, .. } => (path, Status::Deleted, Some(*id), None),
                C::Modification {
                    previous_id, id, ..
                } => (path, Status::Modified, Some(*previous_id), Some(*id)),
                // Rewrite tracking is off above.
                C::Rewrite { .. } => continue,
            });
        }
        Ok(out)
    }

    fn blob(&self, id: ObjectId) -> anyhow::Result<Vec<u8>> {
        Ok(self.repo.find_object(id)?.detach().data)
    }

    /// Changed files of `range` with their new-side hunks. For [`Range::Worktree`]
    /// every HEAD blob under the root is hashed against the file on disk (no
    /// `status` machinery: the index already walked the tree), and `indexed`
    /// paths absent from HEAD are additions.
    pub fn changes<'a>(
        &self,
        range: &Range,
        root: &Path,
        indexed: impl Iterator<Item = &'a str>,
    ) -> anyhow::Result<Vec<Change>> {
        match range {
            Range::Commits { old, new } => {
                let (old, new) = (self.tree(*old)?, self.tree(Some(*new))?);
                let mut out = Vec::new();
                for (path, status, before, after) in self.tree_changes(&old, &new)? {
                    let new = after.map(|b| self.blob(b)).transpose()?;
                    let spans = match (before, &new) {
                        (Some(a), Some(b)) => hunks(&self.blob(a)?, b),
                        (None, Some(_)) => vec![Span::All],
                        _ => Vec::new(),
                    };
                    out.push(Change {
                        path,
                        status,
                        spans,
                        new,
                    });
                }
                Ok(out)
            }
            Range::Worktree => {
                let Some(head) = self.head() else {
                    bail!("HEAD is unborn")
                };
                let tree = self.tree(Some(head))?;
                // git's own shortcut (`git status`): an index entry holding HEAD's
                // blob whose stat still matches the file is unchanged — no read,
                // no hash. Racy entries (written within the index's own second)
                // fall through to the hash.
                let index = self.repo.index_or_empty()?;
                let racy_at = index.timestamp();
                let opts = gix::index::entry::stat::Options::default();
                let mut seen = HashSet::new();
                let mut out = Vec::new();
                for e in tree.traverse().breadthfirst.files()? {
                    if !e.mode.is_blob() {
                        continue;
                    }
                    let Some(path) = self.rel(e.filepath.as_bstr()) else {
                        continue;
                    };
                    seen.insert(path.clone());
                    let full = root.join(&path);
                    if let Some(ie) = index.entry_by_path(e.filepath.as_bstr())
                        && ie.id == e.oid
                        && !ie.stat.is_racy(racy_at, opts)
                        && let Ok(meta) = gix::index::fs::Metadata::from_path_no_follow(&full)
                        && let Ok(st) = gix::index::entry::Stat::from_fs(&meta)
                        && ie.stat.matches(&st, opts)
                    {
                        continue;
                    }
                    match std::fs::read(&full) {
                        Ok(data) => {
                            if gix::objs::compute_hash(
                                self.repo.object_hash(),
                                gix::objs::Kind::Blob,
                                &data,
                            )? == e.oid
                            {
                                continue;
                            }
                            let spans = hunks(&self.blob(e.oid)?, &data);
                            out.push(Change {
                                path,
                                status: Status::Modified,
                                spans,
                                new: Some(data),
                            });
                        }
                        Err(_) => out.push(Change {
                            path,
                            status: Status::Deleted,
                            spans: Vec::new(),
                            new: None,
                        }),
                    }
                }
                for p in indexed {
                    if !seen.contains(p) {
                        let new = std::fs::read(root.join(p)).ok();
                        out.push(Change {
                            path: p.to_owned(),
                            status: Status::Added,
                            spans: vec![Span::All],
                            new,
                        });
                    }
                }
                out.sort_by(|a, b| a.path.cmp(&b.path));
                Ok(out)
            }
        }
    }

    /// The blob at `path` in HEAD, `None` when absent (or HEAD unborn).
    pub fn head_blob(&self, path: &str) -> anyhow::Result<Option<Vec<u8>>> {
        let Some(head) = self.head() else {
            return Ok(None);
        };
        let tree = self.tree(Some(head))?;
        Ok(
            match tree.lookup_entry_by_path(Path::new(&format!("{}{path}", self.prefix)))? {
                Some(entry) => Some(entry.object()?.detach().data),
                None => None,
            },
        )
    }

    /// First-parent walk from HEAD over at most `limit` commits; a commit
    /// touching 2..=20 `indexed` files adds 1 to each unordered pair. Returns
    /// `(file_a, file_b, count)` with `file_a < file_b`, and the commits walked.
    pub fn cochanges(
        &self,
        limit: usize,
        indexed: &HashMap<String, i64>,
    ) -> anyhow::Result<(Vec<Pair>, usize)> {
        let Some(head) = self.head() else {
            return Ok((Vec::new(), 0));
        };
        let mut pairs: HashMap<(i64, i64), i64> = HashMap::new();
        let mut walked = 0;
        for info in self
            .repo
            .rev_walk([head])
            .first_parent_only()
            .all()?
            .take(limit)
        {
            let info = info?;
            walked += 1;
            let new = info.object()?.tree()?;
            let old = self.tree(info.parent_ids.first().copied())?;
            let mut ids: Vec<i64> = self
                .tree_changes(&old, &new)?
                .into_iter()
                .filter_map(|(p, ..)| indexed.get(&p).copied())
                .collect();
            ids.sort_unstable();
            ids.dedup();
            if !COCHANGE_FILES.contains(&ids.len()) {
                continue;
            }
            for (i, &a) in ids.iter().enumerate() {
                for &b in &ids[i + 1..] {
                    *pairs.entry((a, b)).or_default() += 1;
                }
            }
        }
        let mut out: Vec<(i64, i64, i64)> =
            pairs.into_iter().map(|((a, b), n)| (a, b, n)).collect();
        out.sort_unstable();
        Ok((out, walked))
    }
}

fn is_binary(data: &[u8]) -> bool {
    data[..data.len().min(SNIFF)].contains(&0)
}

/// Line hunks of `old` → `new` as new-side spans; a NUL in either side makes
/// the whole file one span.
pub fn hunks(old: &[u8], new: &[u8]) -> Vec<Span> {
    if is_binary(old) || is_binary(new) {
        return vec![Span::All];
    }
    let input = InternedInput::new(old, new);
    let diff = gix::diff::blob::diff_with_slider_heuristics(Algorithm::Histogram, &input);
    diff.hunks()
        .map(|h| {
            if h.after.is_empty() {
                Span::Point(i64::from(h.after.start).max(1))
            } else {
                Span::Lines(i64::from(h.after.start) + 1, i64::from(h.after.end))
            }
        })
        .collect()
}

impl Span {
    /// Does a definition spanning `start..=end` count as changed by this span?
    /// A [`Span::Point`] matches only the innermost containing definition,
    /// which the caller picks; here it is "contains the point".
    pub fn touches(self, start: i64, end: i64) -> bool {
        match self {
            Span::Lines(s, e) => s <= end && start <= e,
            Span::Point(l) => start <= l && l <= end,
            Span::All => true,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::process::Command;

    struct Tmp(std::path::PathBuf);
    impl Drop for Tmp {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.0);
        }
    }

    fn git(dir: &Path, args: &[&str]) {
        let st = Command::new("git")
            .args(args)
            .current_dir(dir)
            .env("GIT_AUTHOR_NAME", "t")
            .env("GIT_AUTHOR_EMAIL", "t@x")
            .env("GIT_COMMITTER_NAME", "t")
            .env("GIT_COMMITTER_EMAIL", "t@x")
            .status()
            .unwrap();
        assert!(st.success(), "git {args:?}");
    }

    /// Two commits: `a.rs` + `b.rs` + `c.txt`, then `a.rs` edited (one line
    /// replaced, one deleted) and `b.rs` touched; then a dirty `c.txt`.
    fn repo() -> Tmp {
        let d = std::env::temp_dir().join(format!("codeindex-git-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&d);
        std::fs::create_dir_all(&d).unwrap();
        git(&d, &["init", "-q", "-b", "main"]);
        std::fs::write(
            d.join("a.rs"),
            "fn one() {\n    1\n}\nfn two() {\n    2\n}\n",
        )
        .unwrap();
        std::fs::write(d.join("b.rs"), "fn b() {}\n").unwrap();
        std::fs::write(d.join("c.txt"), "c\n").unwrap();
        git(&d, &["add", "."]);
        git(&d, &["commit", "-q", "-m", "one"]);
        std::fs::write(d.join("a.rs"), "fn one() {\n    11\n}\nfn two() {\n}\n").unwrap();
        std::fs::write(d.join("b.rs"), "fn b() { 1 }\n").unwrap();
        git(&d, &["add", "."]);
        git(&d, &["commit", "-q", "-m", "two"]);
        std::fs::write(d.join("c.txt"), "changed\n").unwrap();
        std::fs::write(d.join("new.rs"), "fn n() {}\n").unwrap();
        Tmp(d)
    }

    #[test]
    fn ranges_hunks_worktree_and_cochange() {
        let t = repo();
        let root = t.0.canonicalize().unwrap();
        let g = Git::open(&root).unwrap().expect("a repo");
        assert_eq!(g.prefix, "");
        let head = g.head().unwrap();
        // `HEAD~1` is the root commit: `R^..R` with the empty tree as old side.
        let r = g.range("HEAD~1").unwrap();
        let Range::Commits {
            old: None,
            new: first,
        } = r
        else {
            panic!("{r:?}")
        };
        assert_ne!(first, head);
        assert_eq!(
            g.range("HEAD~1..HEAD").unwrap(),
            Range::Commits {
                old: Some(first),
                new: head
            }
        );
        assert_eq!(
            g.range("HEAD").unwrap(),
            Range::Commits {
                old: Some(first),
                new: head
            }
        );
        assert_eq!(
            g.range("..").unwrap(),
            Range::Commits {
                old: Some(head),
                new: head
            }
        );
        assert_eq!(
            g.range("main...HEAD").unwrap(),
            Range::Commits {
                old: Some(head),
                new: head
            }
        );
        assert_eq!(
            g.range(&format!("{first}...HEAD")).unwrap(),
            Range::Commits {
                old: Some(first),
                new: head
            }
        );
        assert_eq!(g.range("WORKTREE").unwrap(), Range::Worktree);
        assert!(g.range("nope").is_err());
        let new = first;

        let ch = g
            .changes(&g.range("HEAD").unwrap(), &root, std::iter::empty())
            .unwrap();
        let a = ch.iter().find(|c| c.path == "a.rs").unwrap();
        assert_eq!(a.status, Status::Modified);
        assert_eq!(
            a.spans,
            [Span::Lines(2, 2), Span::Point(4)],
            "{:?}",
            a.spans
        );
        assert!(Span::Lines(2, 2).touches(1, 3) && !Span::Lines(2, 2).touches(4, 6));
        assert!(Span::Point(4).touches(4, 5));
        assert_eq!(
            ch.iter().find(|c| c.path == "b.rs").unwrap().spans,
            [Span::Lines(1, 1)]
        );
        let ch = g
            .changes(
                &g.range(&new.to_string()).unwrap(),
                &root,
                std::iter::empty(),
            )
            .unwrap();
        assert!(
            ch.iter()
                .all(|c| c.status == Status::Added && c.spans == [Span::All])
                && ch.len() == 3,
            "{ch:?}"
        );

        let ch = g
            .changes(&Range::Worktree, &root, ["new.rs", "a.rs"].into_iter())
            .unwrap();
        let paths: Vec<(&str, Status)> = ch.iter().map(|c| (c.path.as_str(), c.status)).collect();
        assert_eq!(
            paths,
            [("c.txt", Status::Modified), ("new.rs", Status::Added)],
            "{ch:?}"
        );
        assert_eq!(g.head_blob("b.rs").unwrap().unwrap(), b"fn b() { 1 }\n");
        assert!(g.head_blob("new.rs").unwrap().is_none());

        let indexed: HashMap<String, i64> = [("a.rs", 1), ("b.rs", 2), ("c.txt", 3)]
            .into_iter()
            .map(|(p, i)| (p.to_owned(), i))
            .collect();
        let (pairs, walked) = g.cochanges(300, &indexed).unwrap();
        assert_eq!(walked, 2);
        assert_eq!(pairs, [(1, 2, 2), (1, 3, 1), (2, 3, 1)]);
        let (pairs, walked) = g.cochanges(1, &indexed).unwrap();
        assert_eq!((walked, pairs), (1, vec![(1, 2, 1)]));
        assert!(hunks(b"a\0b", b"x").contains(&Span::All));
    }
}
