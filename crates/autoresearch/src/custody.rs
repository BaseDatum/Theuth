//! W1 MEASURE — evaluator custody and post-hoc scope (design doc 140 §5.3):
//! copy the evaluator scripts and `evaluator_paths` into
//! `<root>/<slug>/evaluator/<segment>/` with sha256 per file, verify the
//! worktree's copies before every scored sample, diff the worktree against the
//! incumbent for `deviations` and `off_limits` hits.
//!
//! Layout of `evaluator/<segment>/`: repo files keep their repo-relative path;
//! setup scripts from `<root>/drafts/<slug>/` keep the name the command uses;
//! `commands.json` freezes the three command texts. The custody map keys repo
//! files by their relative path and custody-only files by `custody/<name>`
//! ([`verify`] skips those: they never live in the worktree; the runner runs
//! the custody copy).

use std::collections::BTreeMap;
use std::io::Read as _;
use std::path::{Path, PathBuf};

use sha2::{Digest as _, Sha256};

use crate::error::{Error, Result};
use crate::types::StudySpec;

/// Key prefix of a file that lives only in custody (a setup script, `commands.json`).
pub const CUSTODY_ONLY: &str = "custody/";

/// Copy the evaluator into `evaluator_dir` and return `{path: sha256}` (140 §5.3 Custody).
/// Commands that are not repo paths are frozen as text under `commands.json`.
pub fn take(spec: &StudySpec, repo: &Path, evaluator_dir: &Path) -> Result<BTreeMap<String, String>> {
    std::fs::create_dir_all(evaluator_dir)?;
    let drafts = drafts_dir(evaluator_dir, &spec.slug);
    let mut custody = BTreeMap::new();

    let commands = [
        ("bench", Some(spec.bench.command.as_str())),
        ("checks", spec.checks.as_ref().map(|c| c.command.as_str())),
        ("holdout", spec.holdout.as_ref().map(|h| h.command.as_str())),
    ];
    for (_, cmd) in &commands {
        let Some(cmd) = cmd else { continue };
        for token in cmd.split_whitespace() {
            let Some(rel) = relative(token) else { continue };
            if repo.join(rel).is_file() {
                copy_file(repo, rel, evaluator_dir, &mut custody, "")?;
            } else if drafts.join(rel).is_file() {
                copy_file(&drafts, rel, evaluator_dir, &mut custody, CUSTODY_ONLY)?;
            }
        }
    }
    for entry in &spec.evaluator_paths {
        let Some(rel) = relative(entry) else {
            return Err(Error::refused(format!("evaluator_paths: `{entry}` is not a relative repo path")));
        };
        let src = repo.join(rel);
        if src.is_dir() {
            for file in walk(&src)? {
                let rel = file.strip_prefix(repo).expect("under repo");
                copy_file(repo, rel, evaluator_dir, &mut custody, "")?;
            }
        } else if src.is_file() {
            copy_file(repo, rel, evaluator_dir, &mut custody, "")?;
        } else {
            return Err(Error::refused(format!("evaluator_paths: `{entry}` does not exist in the repo")));
        }
    }
    let frozen: BTreeMap<&str, &str> = commands.iter().filter_map(|(k, v)| v.map(|v| (*k, v))).collect();
    let path = evaluator_dir.join("commands.json");
    std::fs::write(&path, serde_json::to_vec_pretty(&frozen)?)?;
    custody.insert(format!("{CUSTODY_ONLY}commands.json"), sha256_file(&path)?);
    Ok(custody)
}

/// Hash the worktree's copies of every custody path; `Ok(vec![])` = intact,
/// otherwise the changed paths (the refusal names them).
pub fn verify(custody: &BTreeMap<String, String>, worktree: &Path) -> Result<Vec<String>> {
    let mut changed = Vec::new();
    for (rel, sha) in custody {
        if rel.starts_with(CUSTODY_ONLY) {
            continue;
        }
        let path = worktree.join(rel);
        let same = match sha256_file(&path) {
            Ok(now) => &now == sha,
            Err(Error::Io(e)) if e.kind() == std::io::ErrorKind::NotFound => false,
            Err(e) => return Err(e),
        };
        if !same {
            changed.push(rel.clone());
        }
    }
    Ok(changed)
}

/// What the worktree changed vs the incumbent (140 §5.3 Scope).
#[derive(Clone, Debug, Default, PartialEq)]
pub struct ScopeDiff {
    /// Every changed path.
    pub changed: Vec<String>,
    /// Paths outside `scope`.
    pub deviations: Vec<String>,
    /// Paths matching `off_limits` (a keep is impossible).
    pub off_limits: Vec<String>,
    /// `git diff --numstat` totals, whitespace-only excluded.
    pub added: u32,
    /// See `added`.
    pub removed: u32,
}

/// `git status --porcelain=v2` + `git diff --numstat` of the worktree, classified against the spec's globs.
pub fn scope_diff(_spec: &StudySpec, _worktree: &Path) -> Result<ScopeDiff> {
    Err(Error::NotBuilt("W2"))
}

/// sha256 hex of a file.
pub fn sha256_file(path: &Path) -> Result<String> {
    let mut file = std::fs::File::open(path)?;
    let mut hasher = Sha256::new();
    let mut buf = [0u8; 64 * 1024];
    loop {
        let n = file.read(&mut buf)?;
        if n == 0 {
            break;
        }
        hasher.update(&buf[..n]);
    }
    Ok(format!("{:x}", hasher.finalize()))
}

/// `<root>/drafts/<slug>/` from `<root>/<slug>/evaluator/<segment>/`.
fn drafts_dir(evaluator_dir: &Path, slug: &str) -> PathBuf {
    evaluator_dir
        .ancestors()
        .nth(3)
        .map_or_else(|| PathBuf::from("drafts"), |root| root.join("drafts"))
        .join(slug)
}

/// A token as a repo-relative path: no absolute paths, no `..`, no flags; trailing `/` dropped.
pub(crate) fn relative(token: &str) -> Option<&Path> {
    let t = token.trim_end_matches('/');
    if t.is_empty() || t.starts_with('-') || t.starts_with('/') || t.starts_with('$') {
        return None;
    }
    let p = Path::new(t);
    p.components()
        .all(|c| matches!(c, std::path::Component::Normal(_)))
        .then_some(p)
}

/// Copy `base/rel` to `evaluator_dir/rel`, record its hash under `prefix + rel`.
fn copy_file(
    base: &Path,
    rel: &Path,
    evaluator_dir: &Path,
    custody: &mut BTreeMap<String, String>,
    prefix: &str,
) -> Result<()> {
    let key = format!("{prefix}{}", rel.to_string_lossy());
    if custody.contains_key(&key) {
        return Ok(());
    }
    let dst = evaluator_dir.join(rel);
    if let Some(parent) = dst.parent() {
        std::fs::create_dir_all(parent)?;
    }
    std::fs::copy(base.join(rel), &dst)?;
    custody.insert(key, sha256_file(&dst)?);
    Ok(())
}

/// Every regular file under `dir`, recursively (sorted).
fn walk(dir: &Path) -> Result<Vec<PathBuf>> {
    let mut out = Vec::new();
    let mut stack = vec![dir.to_path_buf()];
    while let Some(d) = stack.pop() {
        for entry in std::fs::read_dir(&d)? {
            let path = entry?.path();
            if path.is_dir() {
                stack.push(path);
            } else if path.is_file() {
                out.push(path);
            }
        }
    }
    out.sort();
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::types::CommandSpec;

    fn spec(slug: &str) -> StudySpec {
        let mut s = StudySpec {
            slug: slug.into(),
            evaluator_paths: vec!["fixtures/".into(), "test.sh".into()],
            ..StudySpec::default()
        };
        s.bench.command = "bash bench.sh --n 3".into();
        s.checks = Some(CommandSpec {
            command: "bash test.sh".into(),
            ..CommandSpec::default()
        });
        s
    }

    fn layout() -> (tempfile::TempDir, PathBuf, PathBuf, PathBuf) {
        let tmp = tempfile::tempdir().unwrap();
        let root = tmp.path().join("autoresearch");
        let repo = tmp.path().join("repo");
        std::fs::create_dir_all(repo.join("fixtures/deep")).unwrap();
        std::fs::write(repo.join("fixtures/a.json"), "[1]").unwrap();
        std::fs::write(repo.join("fixtures/deep/b.json"), "[2]").unwrap();
        std::fs::write(repo.join("test.sh"), "exit 0").unwrap();
        std::fs::write(repo.join("sorter.py"), "pass").unwrap();
        std::fs::create_dir_all(root.join("drafts/toy")).unwrap();
        std::fs::write(root.join("drafts/toy/bench.sh"), "echo METRIC t=1").unwrap();
        let evaluator = root.join("toy/evaluator/0");
        (tmp, repo, evaluator, root)
    }

    #[test]
    fn take_copies_scripts_and_paths_and_verify_catches_edits() {
        let (_tmp, repo, evaluator, _root) = layout();
        let custody = take(&spec("toy"), &repo, &evaluator).unwrap();
        let keys: Vec<&str> = custody.keys().map(String::as_str).collect();
        assert_eq!(
            keys,
            vec![
                "custody/bench.sh",
                "custody/commands.json",
                "fixtures/a.json",
                "fixtures/deep/b.json",
                "test.sh"
            ]
        );
        assert!(evaluator.join("bench.sh").is_file());
        assert!(evaluator.join("fixtures/deep/b.json").is_file());
        assert_eq!(custody["fixtures/a.json"], sha256_file(&repo.join("fixtures/a.json")).unwrap());
        let frozen: BTreeMap<String, String> =
            serde_json::from_slice(&std::fs::read(evaluator.join("commands.json")).unwrap()).unwrap();
        assert_eq!(frozen["bench"], "bash bench.sh --n 3");
        assert_eq!(frozen["checks"], "bash test.sh");

        // The worktree is the repo here: intact.
        assert_eq!(verify(&custody, &repo).unwrap(), Vec::<String>::new());
        std::fs::write(repo.join("fixtures/a.json"), "[1,2]").unwrap();
        std::fs::remove_file(repo.join("test.sh")).unwrap();
        assert_eq!(verify(&custody, &repo).unwrap(), vec!["fixtures/a.json", "test.sh"]);
        // Scope files are not custody.
        std::fs::write(repo.join("sorter.py"), "changed").unwrap();
        assert_eq!(verify(&custody, &repo).unwrap().len(), 2);
    }

    #[test]
    fn non_path_commands_are_frozen_as_text_only() {
        let (_tmp, repo, evaluator, _root) = layout();
        let mut s = spec("toy");
        s.bench.command = "cargo bench --bench parse".into();
        s.checks = None;
        s.evaluator_paths = vec![];
        let custody = take(&s, &repo, &evaluator).unwrap();
        assert_eq!(custody.len(), 1);
        assert!(custody.contains_key("custody/commands.json"));
        assert_eq!(verify(&custody, &repo).unwrap(), Vec::<String>::new());
    }

    #[test]
    fn missing_evaluator_path_is_refused() {
        let (_tmp, repo, evaluator, _root) = layout();
        let mut s = spec("toy");
        s.evaluator_paths = vec!["nope/".into()];
        assert!(matches!(take(&s, &repo, &evaluator), Err(Error::Refused(_))));
        s.evaluator_paths = vec!["../etc".into()];
        assert!(matches!(take(&s, &repo, &evaluator), Err(Error::Refused(_))));
    }

    #[test]
    fn new_segment_is_a_fresh_copy() {
        let (_tmp, repo, evaluator, root) = layout();
        let before = take(&spec("toy"), &repo, &evaluator).unwrap();
        std::fs::write(repo.join("fixtures/a.json"), "[9]").unwrap();
        let after = take(&spec("toy"), &repo, &root.join("toy/evaluator/1")).unwrap();
        assert_ne!(before["fixtures/a.json"], after["fixtures/a.json"]);
        assert_eq!(verify(&after, &repo).unwrap(), Vec::<String>::new());
    }
}
