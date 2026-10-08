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
pub fn take(
    spec: &StudySpec,
    repo: &Path,
    evaluator_dir: &Path,
) -> Result<BTreeMap<String, String>> {
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
            return Err(Error::refused(format!(
                "evaluator_paths: `{entry}` is not a relative repo path"
            )));
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
            return Err(Error::refused(format!(
                "evaluator_paths: `{entry}` does not exist in the repo"
            )));
        }
    }
    let frozen: BTreeMap<&str, &str> = commands
        .iter()
        .filter_map(|(k, v)| v.map(|v| (*k, v)))
        .collect();
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
///
/// HEAD of the study worktree is the incumbent. `changed` includes untracked files;
/// `deviations` = paths outside `scope` (an empty `scope` means everything is in scope);
/// `off_limits` = paths matching `off_limits` **or** listed in `evaluator_paths` (either
/// makes a keep impossible, 140 §5.3); `added`/`removed` from `git diff --numstat -w
/// --ignore-blank-lines HEAD` plus the non-blank lines of untracked files as `added`.
pub fn scope_diff(spec: &StudySpec, worktree: &Path) -> Result<ScopeDiff> {
    let status = git(
        worktree,
        &["status", "--porcelain=v2", "--untracked-files=all", "-z"],
    )?;
    let mut changed: Vec<String> = Vec::new();
    let mut untracked: Vec<String> = Vec::new();
    let mut fields = status.split('\0').filter(|s| !s.is_empty());
    while let Some(entry) = fields.next() {
        let Some((kind, rest)) = entry.split_once(' ') else {
            continue;
        };
        match kind {
            // `1 <XY> <sub> <mH> <mI> <mW> <hH> <hI> <path>`
            "1" => changed.extend(rest.splitn(8, ' ').nth(7).map(str::to_owned)),
            // `2 <XY> <sub> <mH> <mI> <mW> <hH> <hI> <X><score> <path>` then `<origPath>` as its own NUL field
            "2" => {
                changed.extend(rest.splitn(9, ' ').nth(8).map(str::to_owned));
                changed.extend(fields.next().map(str::to_owned));
            }
            // `u <XY> <sub> <m1> <m2> <m3> <mW> <h1> <h2> <h3> <path>`
            "u" => changed.extend(rest.splitn(10, ' ').nth(9).map(str::to_owned)),
            "?" => {
                changed.push(rest.to_owned());
                untracked.push(rest.to_owned());
            }
            _ => {}
        }
    }
    changed.sort();
    changed.dedup();

    let (mut added, mut removed) = (0u32, 0u32);
    for line in git(
        worktree,
        &["diff", "--numstat", "-w", "--ignore-blank-lines", "HEAD"],
    )?
    .lines()
    {
        let mut cols = line.split('\t');
        let (Some(a), Some(r)) = (cols.next(), cols.next()) else {
            continue;
        };
        added += a.parse::<u32>().unwrap_or(0); // `-` for binary
        removed += r.parse::<u32>().unwrap_or(0);
    }
    for rel in &untracked {
        if let Ok(text) = std::fs::read_to_string(worktree.join(rel)) {
            added += text.lines().filter(|l| !l.trim().is_empty()).count() as u32;
        }
    }

    let scope = globs(&spec.scope)?;
    let off = globs(&spec.off_limits)?;
    let evaluator: Vec<&Path> = spec
        .evaluator_paths
        .iter()
        .filter_map(|e| relative(e))
        .collect();
    let deviations = changed
        .iter()
        .filter(|p| !spec.scope.is_empty() && !scope.is_match(p))
        .cloned()
        .collect();
    let off_limits = changed
        .iter()
        .filter(|p| off.is_match(p) || evaluator.iter().any(|e| Path::new(p).starts_with(e)))
        .cloned()
        .collect();
    Ok(ScopeDiff {
        changed,
        deviations,
        off_limits,
        added,
        removed,
    })
}

/// `git <args>` in `cwd`, stdout as text; a non-zero exit is `Refused` with stderr.
fn git(cwd: &Path, args: &[&str]) -> Result<String> {
    let out = std::process::Command::new("git")
        .args(["-c", "core.hooksPath=/dev/null"])
        .args(args)
        .current_dir(cwd)
        .env("GIT_TERMINAL_PROMPT", "0")
        .output()?;
    if !out.status.success() {
        return Err(Error::refused(format!(
            "git {}: {}",
            args.join(" "),
            String::from_utf8_lossy(&out.stderr).trim()
        )));
    }
    Ok(String::from_utf8_lossy(&out.stdout).into_owned())
}

/// The spec's globs as one matcher; a plain path or `dir/` also matches everything under it.
fn globs(patterns: &[String]) -> Result<globset::GlobSet> {
    let mut set = globset::GlobSetBuilder::new();
    for p in patterns {
        let p = p.trim_end_matches('/');
        for pat in [p.to_owned(), format!("{p}/**")] {
            set.add(
                globset::GlobBuilder::new(&pat)
                    .literal_separator(true)
                    .build()
                    .map_err(|e| Error::refused(format!("glob `{p}`: {e}")))?,
            );
        }
    }
    set.build()
        .map_err(|e| Error::refused(format!("globs: {e}")))
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
        assert_eq!(
            custody["fixtures/a.json"],
            sha256_file(&repo.join("fixtures/a.json")).unwrap()
        );
        let frozen: BTreeMap<String, String> =
            serde_json::from_slice(&std::fs::read(evaluator.join("commands.json")).unwrap())
                .unwrap();
        assert_eq!(frozen["bench"], "bash bench.sh --n 3");
        assert_eq!(frozen["checks"], "bash test.sh");

        // The worktree is the repo here: intact.
        assert_eq!(verify(&custody, &repo).unwrap(), Vec::<String>::new());
        std::fs::write(repo.join("fixtures/a.json"), "[1,2]").unwrap();
        std::fs::remove_file(repo.join("test.sh")).unwrap();
        assert_eq!(
            verify(&custody, &repo).unwrap(),
            vec!["fixtures/a.json", "test.sh"]
        );
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
        assert!(matches!(
            take(&s, &repo, &evaluator),
            Err(Error::Refused(_))
        ));
        s.evaluator_paths = vec!["../etc".into()];
        assert!(matches!(
            take(&s, &repo, &evaluator),
            Err(Error::Refused(_))
        ));
    }

    fn sh(cwd: &Path, cmd: &str) {
        let st = std::process::Command::new("/bin/bash")
            .args(["-c", cmd])
            .current_dir(cwd)
            .env("GIT_AUTHOR_NAME", "t")
            .env("GIT_AUTHOR_EMAIL", "t@t")
            .env("GIT_COMMITTER_NAME", "t")
            .env("GIT_COMMITTER_EMAIL", "t@t")
            .status()
            .unwrap();
        assert!(st.success(), "{cmd}");
    }

    #[test]
    fn scope_diff_classifies_paths_and_counts_lines() {
        let tmp = tempfile::tempdir().unwrap();
        let wt = tmp.path();
        std::fs::create_dir_all(wt.join("lib/liquid")).unwrap();
        std::fs::create_dir_all(wt.join("test")).unwrap();
        std::fs::create_dir_all(wt.join("fixtures")).unwrap();
        std::fs::write(wt.join("lib/liquid/a.rb"), "a\nb\nc\nd\n").unwrap();
        std::fs::write(wt.join("lib/other.rb"), "x\n").unwrap();
        std::fs::write(wt.join("test/a_test.rb"), "t\n").unwrap();
        std::fs::write(wt.join("fixtures/in.json"), "[]\n").unwrap();
        std::fs::write(wt.join("Rakefile"), "task\n").unwrap();
        sh(wt, "git init -q && git add -A && git commit -qm base");
        let spec = StudySpec {
            scope: vec!["lib/liquid/**".into()],
            off_limits: vec!["test/**".into()],
            evaluator_paths: vec!["fixtures/".into(), "Rakefile".into()],
            ..StudySpec::default()
        };
        assert_eq!(scope_diff(&spec, wt).unwrap(), ScopeDiff::default());

        // In scope: 4 lines → 2 lines (one of them whitespace-only re-indented: not counted).
        std::fs::write(wt.join("lib/liquid/a.rb"), "  a\nz\n").unwrap();
        // Outside scope, untracked: 2 non-blank lines added.
        std::fs::write(wt.join("lib/new.rb"), "n1\n\nn2\n").unwrap();
        // Off-limits and evaluator paths.
        std::fs::write(wt.join("test/a_test.rb"), "tt\n").unwrap();
        std::fs::write(wt.join("fixtures/in.json"), "[1]\n").unwrap();
        std::fs::write(wt.join("Rakefile"), "task2\n").unwrap();
        let d = scope_diff(&spec, wt).unwrap();
        assert_eq!(
            d.changed,
            vec![
                "Rakefile",
                "fixtures/in.json",
                "lib/liquid/a.rb",
                "lib/new.rb",
                "test/a_test.rb"
            ]
        );
        assert_eq!(
            d.deviations,
            vec![
                "Rakefile",
                "fixtures/in.json",
                "lib/new.rb",
                "test/a_test.rb"
            ]
        );
        assert_eq!(
            d.off_limits,
            vec!["Rakefile", "fixtures/in.json", "test/a_test.rb"]
        );
        // a.rb: +1 (z) −3 (b, c, d; `a` → `  a` ignored); test +1 −1; fixtures +1 −1; Rakefile +1 −1; new.rb +2.
        assert_eq!((d.added, d.removed), (6, 6));

        // An empty scope makes nothing a deviation; a staged rename is one changed pair.
        let open = StudySpec::default();
        sh(wt, "git add -A && git mv lib/other.rb lib/moved.rb");
        let d = scope_diff(&open, wt).unwrap();
        assert!(d.deviations.is_empty());
        assert!(
            d.changed.contains(&"lib/moved.rb".to_owned())
                && d.changed.contains(&"lib/other.rb".to_owned())
        );
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
