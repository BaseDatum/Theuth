//! W2 RATCHET — the git writer (design doc 140 §6): the `git` CLI with
//! `GIT_TERMINAL_PROMPT=0`, a fixed author, `-c core.hooksPath=/dev/null`,
//! only ever inside the study worktree. Worktree add/remove, base resolution
//! and the clean-tree check, keep (add + commit with trailers, evaluator
//! paths excluded), discard (save the patch, `reset --hard`, `clean -fd`),
//! drift repair, rejected refs, local branches. Never push, rebase or force.
//!
//! Reads use the CLI too (one writer, one spelling of every path). Untracked
//! files enter a diff through `git add -N` (intent-to-add) on the study
//! worktree's index — never the user's — so `diff <incumbent>` is the whole
//! candidate in one pass; the following `reset --hard` or `add -A` absorbs it.

use std::path::Path;
use std::process::{Command, Stdio};

use crate::custody;
use crate::error::{Error, Result};
use crate::types::StudySpec;

/// Author of every commit the plugin makes unless `autoresearch.git.author`.
pub const AUTHOR: &str = "theuth autoresearch <noreply@theuth.local>";

/// Build a `git` command with no author override: `-C cwd`, hooks off,
/// prompts off, signing off; the user's own identity applies (the landing merge).
fn cmd_unsigned(cwd: &Path, args: &[&str]) -> Command {
    let mut c = Command::new("git");
    c.arg("-C")
        .arg(cwd)
        .args([
            "-c",
            "core.hooksPath=/dev/null",
            "-c",
            "commit.gpgsign=false",
            "-c",
            "advice.detachedHead=false",
        ])
        .args(args)
        .env("GIT_TERMINAL_PROMPT", "0")
        .env_remove("GIT_DIR")
        .env_remove("GIT_WORK_TREE")
        .env_remove("GIT_INDEX_FILE")
        .stdin(Stdio::null());
    c
}

/// [`cmd_unsigned`] with author/committer fixed. `author` is `Name <email>`.
fn cmd(cwd: &Path, author: Option<&str>, args: &[&str]) -> Command {
    let mut c = cmd_unsigned(cwd, args);
    let author = author.filter(|a| !a.trim().is_empty()).unwrap_or(AUTHOR);
    let (name, email) = match author.split_once('<') {
        Some((n, e)) => (n.trim(), e.trim_end_matches('>').trim()),
        None => (author.trim(), "noreply@theuth.local"),
    };
    c.env("GIT_AUTHOR_NAME", name)
        .env("GIT_AUTHOR_EMAIL", email)
        .env("GIT_COMMITTER_NAME", name)
        .env("GIT_COMMITTER_EMAIL", email);
    c
}

/// Run and return `(ok, stdout, stderr)` — for commands whose non-zero exit is an answer.
fn run_status(cwd: &Path, author: Option<&str>, args: &[&str]) -> Result<(bool, String, String)> {
    let out = cmd(cwd, author, args).output().map_err(|e| {
        Error::Other(anyhow::anyhow!(
            "git {}: {e}",
            args.first().copied().unwrap_or("")
        ))
    })?;
    Ok((
        out.status.success(),
        String::from_utf8_lossy(&out.stdout).into_owned(),
        String::from_utf8_lossy(&out.stderr).into_owned(),
    ))
}

/// Run; non-zero exit is an error naming the command and git's stderr.
fn run(cwd: &Path, author: Option<&str>, args: &[&str]) -> Result<String> {
    let (ok, stdout, stderr) = run_status(cwd, author, args)?;
    if !ok {
        return Err(Error::Other(anyhow::anyhow!(
            "git {}: {}",
            args.join(" "),
            stderr.trim().lines().last().unwrap_or("failed")
        )));
    }
    Ok(stdout)
}

/// [`run`] without the author override.
fn git(cwd: &Path, args: &[&str]) -> Result<String> {
    run(cwd, None, args)
}

/// A read-only `git <args>` in `cwd` for other modules (custody's status and
/// numstat): the same `-C`, hooks-off, env-scrubbed spelling as every write.
pub fn read(cwd: &Path, args: &[&str]) -> Result<String> {
    git(cwd, args)
}

/// 159 W-f: a `git <args>` in `cwd` for [`crate::refs`] (`update-ref`, `for-each-ref`,
/// `show-ref` under `refs/autoresearch/`), the same spelling as every other call here.
/// Non-zero exit is an error.
pub fn plumbing(cwd: &Path, args: &[&str]) -> Result<String> {
    git(cwd, args)
}

/// `git diff` as bytes (binary patches are not UTF-8).
fn diff_bytes(cwd: &Path, args: &[&str]) -> Result<Vec<u8>> {
    let mut c = cmd(cwd, None, &["diff", "--no-color", "--no-ext-diff"]);
    c.args(args);
    let out = c.output()?;
    if !out.status.success() {
        return Err(Error::Other(anyhow::anyhow!(
            "git diff {}: {}",
            args.join(" "),
            String::from_utf8_lossy(&out.stderr).trim()
        )));
    }
    Ok(out.stdout)
}

/// `:(exclude,glob)<pattern>` pathspecs for `patterns` (140 §6 Revision 2 build junk).
fn exclude_pathspecs(patterns: &[&str]) -> Vec<String> {
    patterns
        .iter()
        .map(|g| format!(":(exclude,glob){g}"))
        .collect()
}

/// `git add -A -N`: untracked, non-ignored files become intent-to-add index
/// entries so `diff <commit>` shows them as new files; build junk stays out of patches.
fn stage_intent(worktree: &Path) -> Result<()> {
    let mut args: Vec<String> = vec![
        "add".into(),
        "-A".into(),
        "-N".into(),
        "--".into(),
        ".".into(),
    ];
    args.extend(exclude_pathspecs(&custody::BUILD_JUNK));
    let argv: Vec<&str> = args.iter().map(String::as_str).collect();
    git(worktree, &argv).map(drop)
}

/// Tracked changes vs `HEAD` plus untracked non-ignored paths (`status --porcelain`), one per
/// line; build junk (`__pycache__/`, `.DS_Store`, …) is never dirt.
pub(crate) fn dirty_paths(repo: &Path) -> Result<Vec<String>> {
    Ok(
        git(repo, &["status", "--porcelain", "--untracked-files=all"])?
            .lines()
            .filter(|l| l.len() > 3)
            .map(|l| l[3..].trim().to_owned())
            .filter(|p| !custody::is_build_junk(p))
            .collect(),
    )
}

fn sha256_hex(bytes: &[u8]) -> String {
    platform::sha256::hex(bytes)
}

/// `autoresearch/<slug>`.
pub fn study_branch(slug: &str) -> String {
    format!("autoresearch/{slug}")
}

/// `autoresearch/<slug>+<k>-<name>` — a finalize changeset branch (140 §11.2).
/// The design wrote `autoresearch/<slug>/<k>-<name>`, which git cannot hold
/// beside the branch `autoresearch/<slug>` (a ref cannot be both a file and a
/// directory); `+` is legal in a ref and no slug contains it.
pub fn changeset_branch(slug: &str, k: u32, name: &str) -> String {
    format!("autoresearch/{slug}+{k}-{name}")
}

/// `refs/autoresearch/<slug>/rejected/<n>`.
pub fn rejected_ref(slug: &str, n: u32) -> String {
    format!("refs/autoresearch/{slug}/rejected/{n}")
}

/// Resolve `base` to a sha in `repo`; refuse a dirty tree unless `include_dirty` (140 §6 Base and branch).
pub fn resolve_base(repo: &Path, base: &str, include_dirty: bool) -> Result<String> {
    if !is_git_repo(repo) {
        return Err(Error::refused(format!(
            "{} is not a git repository (a pure jj repo needs git colocation)",
            repo.display()
        )));
    }
    let spec = format!(
        "{}^{{commit}}",
        if base.trim().is_empty() {
            "HEAD"
        } else {
            base.trim()
        }
    );
    let sha = git(repo, &["rev-parse", "--verify", "-q", &spec])
        .map_err(|_| {
            Error::refused(format!(
                "base `{base}` does not name a commit in {}",
                repo.display()
            ))
        })?
        .trim()
        .to_owned();
    let dirty = dirty_paths(repo)?;
    if !dirty.is_empty() {
        if !include_dirty {
            return Err(Error::refused(format!(
                "the checkout at {} has {} uncommitted path(s) ({}); commit or stash them, or start with \
                 \"include my uncommitted changes\"",
                repo.display(),
                dirty.len(),
                dirty.iter().take(3).cloned().collect::<Vec<_>>().join(", ")
            )));
        }
        let head = git(repo, &["rev-parse", "HEAD"])?.trim().to_owned();
        if head != sha {
            return Err(Error::refused(
                "including uncommitted changes needs base HEAD (they are relative to it)",
            ));
        }
    }
    Ok(sha)
}

/// `git worktree add -b autoresearch/<slug> <worktree> <base_sha>`; returns the worktree's HEAD.
///
/// A dirty user checkout (only legal when [`resolve_base`] was told
/// `include_dirty`, so `base_sha` is its HEAD) is snapshotted onto the study
/// branch as `autoresearch(<slug>): base snapshot` — the user's checkout
/// itself is only read (`diff`, `ls-files`).
pub fn worktree_add(
    repo: &Path,
    spec: &StudySpec,
    worktree: &Path,
    base_sha: &str,
) -> Result<String> {
    if let Some(parent) = worktree.parent() {
        std::fs::create_dir_all(parent)?;
    }
    let branch = study_branch(&spec.slug);
    let wt = worktree.to_string_lossy();
    git(
        repo,
        &["worktree", "add", "-q", "-b", &branch, &wt, base_sha],
    )?;
    let head = git(repo, &["rev-parse", "HEAD"])?.trim().to_owned();
    if head == base_sha && !dirty_paths(repo)?.is_empty() {
        let patch = diff_bytes(repo, &["--binary", "HEAD"])?;
        if !patch.is_empty() {
            apply_bytes(worktree, &patch)?;
        }
        let untracked = git(repo, &["ls-files", "--others", "--exclude-standard", "-z"])?;
        for rel in untracked.split('\0').filter(|p| !p.is_empty()) {
            let dst = worktree.join(rel);
            if let Some(p) = dst.parent() {
                std::fs::create_dir_all(p)?;
            }
            std::fs::copy(repo.join(rel), dst)?;
        }
        git(worktree, &["add", "-A", "--", "."])?;
        let msg = format!("autoresearch({}): base snapshot", spec.slug);
        git(worktree, &["commit", "-q", "--allow-empty", "-m", &msg])?;
    }
    self::head(worktree)
}

/// `git worktree remove --force` plus `git worktree prune` (finished studies, wave candidates, finalize scratch).
pub fn worktree_remove(repo: &Path, worktree: &Path) -> Result<()> {
    if worktree.exists() {
        let wt = worktree.to_string_lossy();
        let (ok, _, _) = run_status(
            repo,
            None,
            &["worktree", "remove", "--force", "--force", &wt],
        )?;
        if !ok && worktree.exists() {
            std::fs::remove_dir_all(worktree)?;
        }
    }
    git(repo, &["worktree", "prune"]).map(drop)
}

/// `HEAD` of a worktree.
pub fn head(worktree: &Path) -> Result<String> {
    Ok(git(worktree, &["rev-parse", "HEAD"])?.trim().to_owned())
}

/// Current branch name of a worktree (`None` when detached).
pub fn branch(worktree: &Path) -> Result<Option<String>> {
    let (ok, out, _) = run_status(worktree, None, &["symbolic-ref", "-q", "--short", "HEAD"])?;
    Ok(ok.then(|| out.trim().to_owned()))
}

/// The commit message trailers of a keep (140 §6 Keep).
#[derive(Clone, Debug, Default, PartialEq)]
pub struct KeepMeta {
    /// `<slug>`.
    pub slug: String,
    /// `<n>`.
    pub n: u32,
    /// `autoresearch(<slug>) #<n>: <description>`.
    pub description: String,
    /// `time_ms 42.3 -> 41.2 (-2.6%, 3.1 sigma, n=3)`.
    pub metric_line: String,
    /// `Autoresearch-Category`.
    pub category: String,
    /// `Autoresearch-Deviations`, when any.
    pub deviations: Vec<String>,
}

impl KeepMeta {
    /// The full commit message (subject, blank line, trailers).
    pub fn message(&self) -> String {
        let subject = self.description.lines().next().unwrap_or("").trim();
        let mut m = format!(
            "autoresearch({}) #{}: {}\n\nAutoresearch-Study: {}\nAutoresearch-Run: {}\nAutoresearch-Metric: {}\nAutoresearch-Category: {}\n",
            self.slug,
            self.n,
            subject,
            self.slug,
            self.n,
            self.metric_line.trim(),
            self.category.trim()
        );
        if !self.deviations.is_empty() {
            m.push_str(&format!(
                "Autoresearch-Deviations: {}\n",
                self.deviations.join(", ")
            ));
        }
        m
    }
}

/// `git add -A -- . ':!<evaluator_paths>' ':(exclude,glob)<build junk>'` + commit with
/// trailers; returns the new sha. Junk (§6 Revision 2) is excluded unless `scope` names it.
pub fn keep(
    worktree: &Path,
    spec: &StudySpec,
    meta: &KeepMeta,
    author: Option<&str>,
) -> Result<String> {
    let mut args: Vec<String> = vec!["add".into(), "-A".into(), "--".into(), ".".into()];
    for p in &spec.evaluator_paths {
        let p = p.trim().trim_end_matches('/');
        if !p.is_empty() {
            args.push(format!(":(exclude){p}"));
        }
    }
    args.extend(exclude_pathspecs(custody::build_junk(spec)));
    let argv: Vec<&str> = args.iter().map(String::as_str).collect();
    run(worktree, author, &argv)?;
    let staged = git(worktree, &["diff", "--cached", "--name-only"])?;
    if staged.trim().is_empty() {
        return Err(Error::refused(
            "nothing to keep: no changes outside the evaluator paths",
        ));
    }
    let msg = meta.message();
    run(worktree, author, &["commit", "-q", "-m", &msg])?;
    head(worktree)
}

/// Write `git diff --binary <incumbent>` (with untracked files via intent-to-add) to `patch_path`; returns its bytes.
fn save_patch(worktree: &Path, incumbent: &str, patch_path: &Path) -> Result<Vec<u8>> {
    stage_intent(worktree)?;
    let patch = diff_bytes(worktree, &["--binary", incumbent, "--", "."])?;
    if let Some(p) = patch_path.parent() {
        std::fs::create_dir_all(p)?;
    }
    std::fs::write(patch_path, &patch)?;
    Ok(patch)
}

/// `reset --hard <sha>` + `clean -fd` (ignored files survive). Finalize uses
/// it so the "final" tree it measures is the kept incumbent, not whatever the
/// study turn edited after `experiment_log` (R-56).
pub fn reset_clean(worktree: &Path, sha: &str) -> Result<()> {
    git(worktree, &["reset", "-q", "--hard", sha])?;
    git(worktree, &["clean", "-fd", "-q"]).map(drop)
}

/// Save `git diff --binary <incumbent>` plus untracked files to `patch_path`,
/// then `reset --hard <incumbent>` and `clean -fd` (ignored files survive). Returns the patch's sha256.
pub fn discard(worktree: &Path, incumbent: &str, patch_path: &Path) -> Result<String> {
    let patch = save_patch(worktree, incumbent, patch_path)?;
    reset_clean(worktree, incumbent)?;
    Ok(sha256_hex(&patch))
}

fn apply_bytes(worktree: &Path, patch: &[u8]) -> Result<()> {
    use std::io::Write as _;
    let mut c = cmd(
        worktree,
        None,
        &["apply", "--binary", "--whitespace=nowarn"],
    );
    c.stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
    let mut child = c.spawn()?;
    child.stdin.take().expect("piped").write_all(patch)?;
    let out = child.wait_with_output()?;
    if !out.status.success() {
        return Err(Error::refused(format!(
            "patch does not apply: {}",
            String::from_utf8_lossy(&out.stderr)
                .trim()
                .lines()
                .last()
                .unwrap_or("")
        )));
    }
    Ok(())
}

/// Re-apply a saved patch (`candidate.patch`, `stray.patch`) to the worktree
/// as uncommitted changes — a discarded idea restored as a new candidate.
pub fn apply_patch(worktree: &Path, patch_path: &Path) -> Result<()> {
    let patch = std::fs::read(patch_path)?;
    if patch.is_empty() {
        return Ok(());
    }
    apply_bytes(worktree, &patch)
}

/// Drift repair at `experiment_begin` (140 §6): HEAD == incumbent, branch ==
/// `autoresearch/<slug>`, clean tree; otherwise save `stray.patch`, reset,
/// and return what was wrong (for the incident row and the brief).
pub fn repair(
    worktree: &Path,
    spec: &StudySpec,
    incumbent: &str,
    stray_patch: &Path,
) -> Result<Option<String>> {
    let want = study_branch(&spec.slug);
    let head = head(worktree)?;
    let branch = branch(worktree)?;
    let dirty = dirty_paths(worktree)?;
    let mut wrong = Vec::new();
    if head != incumbent {
        wrong.push(format!(
            "HEAD was {} instead of the incumbent {}",
            &head[..7.min(head.len())],
            &incumbent[..7.min(incumbent.len())]
        ));
    }
    match &branch {
        Some(b) if *b == want => {}
        Some(b) => wrong.push(format!("branch was `{b}` instead of `{want}`")),
        None => wrong.push(format!("HEAD was detached instead of on `{want}`")),
    }
    if !dirty.is_empty() {
        wrong.push(format!(
            "{} uncommitted path(s): {}",
            dirty.len(),
            dirty.iter().take(5).cloned().collect::<Vec<_>>().join(", ")
        ));
    }
    if wrong.is_empty() {
        return Ok(None);
    }
    let patch = save_patch(worktree, incumbent, stray_patch)?;
    reset_clean(worktree, incumbent)?;
    if branch.as_deref() != Some(want.as_str()) {
        git(worktree, &["checkout", "-q", "-B", &want, incumbent])?;
    }
    let saved = if patch.is_empty() {
        "nothing to save".to_owned()
    } else {
        format!("{} bytes saved to {}", patch.len(), stray_patch.display())
    };
    Ok(Some(format!(
        "worktree drift repaired: {}; {saved}; reset to the incumbent",
        wrong.join("; ")
    )))
}

/// Revert a reviewed keep: incumbent ← previous, the commit kept reachable as
/// `refs/autoresearch/<slug>/rejected/<n>` (140 §9.1).
pub fn reject(worktree: &Path, slug: &str, n: u32, previous: &str) -> Result<()> {
    let r = rejected_ref(slug, n);
    git(worktree, &["update-ref", &r, "HEAD"])?;
    reset_clean(worktree, previous)
}

/// Restore a rejected keep as uncommitted changes on the current incumbent
/// (140 §9.1 Restore: measured again, not trusted from memory). Refuses on conflict.
pub fn restore_rejected(worktree: &Path, slug: &str, n: u32) -> Result<()> {
    let r = rejected_ref(slug, n);
    let (ok, _, err) = run_status(worktree, None, &["cherry-pick", "--no-commit", &r])?;
    if !ok {
        let _ = run_status(worktree, None, &["cherry-pick", "--abort"]);
        let _ = run_status(worktree, None, &["reset", "-q", "--hard", "HEAD"]);
        return Err(Error::refused(format!(
            "rejected #{n} no longer applies to the incumbent: {}",
            err.trim().lines().last().unwrap_or("conflict")
        )));
    }
    git(worktree, &["reset", "-q"]).map(drop)
}

/// Normalised patch hash for the novelty check (140 §7.3): `git diff -w <incumbent>`
/// with `index`/`@@` lines dropped and every line's whitespace removed
/// (context lines carry the tree's whitespace even under `-w`), sha256.
pub fn patch_hash(worktree: &Path, incumbent: &str) -> Result<String> {
    stage_intent(worktree)?;
    let raw = diff_bytes(worktree, &["-w", incumbent, "--", "."])?;
    let text = String::from_utf8_lossy(&raw);
    let mut norm = String::with_capacity(text.len());
    for line in text.lines() {
        if line.starts_with("index ") || line.starts_with("@@") {
            continue;
        }
        norm.extend(line.chars().filter(|c| !c.is_whitespace()));
        norm.push('\n');
    }
    Ok(sha256_hex(norm.as_bytes()))
}

/// `git diff <a> <b>` text, capped at `max_bytes` (the reviewer's and the detail view's diff).
pub fn diff_text(worktree: &Path, a: &str, b: &str, max_bytes: usize) -> Result<String> {
    let raw = diff_bytes(worktree, &[a, b, "--", "."])?;
    let mut text = String::from_utf8_lossy(&raw).into_owned();
    if text.len() > max_bytes {
        let cut = text.floor_char_boundary(max_bytes);
        text.truncate(cut);
        text.push_str(&format!("\n… (diff truncated at {max_bytes} bytes)\n"));
    }
    Ok(text)
}

/// Files touched by commit `sha`.
pub fn files_of(worktree: &Path, sha: &str) -> Result<Vec<String>> {
    Ok(git(
        worktree,
        &[
            "diff-tree",
            "--no-commit-id",
            "--name-only",
            "-r",
            "--root",
            sha,
        ],
    )?
    .lines()
    .map(str::trim)
    .filter(|l| !l.is_empty())
    .map(str::to_owned)
    .collect())
}

/// `git cherry-pick <sha>` in a scratch worktree; `Ok(false)` = conflict (aborted).
pub fn cherry_pick(worktree: &Path, sha: &str) -> Result<bool> {
    Ok(cherry_pick_conflicts(worktree, sha)?.is_empty())
}

/// [`cherry_pick`] that names the conflicting paths (`ls-files -u`, git's
/// `~HEAD`-style suffixes stripped); empty = applied. The pick is aborted on conflict.
pub fn cherry_pick_conflicts(worktree: &Path, sha: &str) -> Result<Vec<String>> {
    let (ok, _, err) = run_status(
        worktree,
        None,
        &[
            "cherry-pick",
            "--allow-empty",
            "--keep-redundant-commits",
            sha,
        ],
    )?;
    if ok {
        return Ok(Vec::new());
    }
    let mut paths: Vec<String> = git(worktree, &["ls-files", "-u"])?
        .lines()
        .filter_map(|l| l.split('\t').nth(1))
        .map(|p| p.split('~').next().unwrap_or(p).to_owned())
        .collect();
    paths.sort();
    paths.dedup();
    let (aborted, _, _) = run_status(worktree, None, &["cherry-pick", "--abort"])?;
    if !aborted {
        reset_clean(worktree, "HEAD")?;
    }
    if err.contains("CONFLICT") || err.contains("could not apply") {
        if paths.is_empty() {
            paths.push(String::new());
        }
        return Ok(paths);
    }
    Err(Error::Other(anyhow::anyhow!(
        "git cherry-pick {sha}: {}",
        err.trim().lines().last().unwrap_or("failed")
    )))
}

/// Create a local branch at `sha` (finalize's per-changeset branches). Never
/// pushed, never a force-move of a branch that is not ours (140 §6, R-16): an
/// existing branch is moved only when its tip carries the plugin's
/// `Autoresearch-Study:` trailer (a re-run finalize), else refused.
pub fn branch_at(repo: &Path, name: &str, sha: &str) -> Result<()> {
    let (exists, _, _) = run_status(
        repo,
        None,
        &[
            "show-ref",
            "--verify",
            "--quiet",
            &format!("refs/heads/{name}"),
        ],
    )?;
    if !exists {
        return git(repo, &["branch", name, sha]).map(drop);
    }
    let tip = git(repo, &["log", "-1", "--format=%B", name])?;
    if !tip.contains("Autoresearch-Study:") {
        return Err(Error::refused(format!(
            "branch `{name}` exists and was not made by autoresearch; not moved"
        )));
    }
    git(repo, &["branch", "-f", name, sha]).map(drop)
}

/// `git worktree add --detach <worktree> <sha>` (finalize scratch, wave candidates).
pub fn worktree_add_detached(repo: &Path, worktree: &Path, sha: &str) -> Result<()> {
    if let Some(parent) = worktree.parent() {
        std::fs::create_dir_all(parent)?;
    }
    let wt = worktree.to_string_lossy();
    git(repo, &["worktree", "add", "-q", "--detach", &wt, sha]).map(drop)
}

/// Current branch of the user's checkout and whether it is clean.
pub fn checkout_state(repo: &Path) -> Result<(Option<String>, bool)> {
    Ok((branch(repo)?, dirty_paths(repo)?.is_empty()))
}

/// Landing helper "Merge into my branch" (140 §11.5): `git merge --no-ff
/// <branch>` into the repo's current branch; refused when the user's tree is
/// dirty or detached. The one write the plugin ever makes in the user's
/// checkout, and only on this explicit press — so the merge commit is
/// authored by the user's own git identity, not the plugin's (R-15);
/// `_author` stays for the `Backend` signature.
pub fn merge_into_checkout(repo: &Path, branch: &str, _author: Option<&str>) -> Result<String> {
    let (current, clean) = checkout_state(repo)?;
    let Some(current) = current else {
        return Err(Error::refused(
            "your checkout is detached; check out a branch first",
        ));
    };
    if !clean {
        return Err(Error::refused(
            "your checkout has uncommitted changes; commit or stash them first",
        ));
    }
    let msg = format!("Merge {branch} into {current} (theuth autoresearch)");
    let out = cmd_unsigned(repo, &["merge", "--no-ff", "-m", &msg, branch])
        .output()
        .map_err(|e| Error::Other(anyhow::anyhow!("git merge: {e}")))?;
    if !out.status.success() {
        let _ = run_status(repo, None, &["merge", "--abort"]);
        let err = String::from_utf8_lossy(&out.stderr);
        return Err(Error::refused(format!(
            "merge of {branch} into {current} did not apply cleanly: {}",
            err.trim().lines().last().unwrap_or("conflict")
        )));
    }
    head(repo)
}

/// Landing helper "Discard study" (140 §11.5): remove the study worktree and
/// every scratch worktree under `study_dir`, then delete `autoresearch/<slug>`,
/// its changeset branches and the rejected refs. Nothing else in the repo.
pub fn discard_study(repo: &Path, slug: &str, study_dir: &Path) -> Result<Vec<String>> {
    let mut removed = Vec::new();
    // `worktree list` prints resolved paths (`/private/var/…` on macOS); compare canonical forms.
    let canon = |p: &Path| p.canonicalize().unwrap_or_else(|_| p.to_path_buf());
    let study_dir = canon(study_dir);
    for wt in git(repo, &["worktree", "list", "--porcelain"])?
        .lines()
        .filter_map(|l| l.strip_prefix("worktree "))
        .map(|p| canon(Path::new(p)))
        .filter(|p| p.starts_with(&study_dir))
    {
        worktree_remove(repo, &wt)?;
    }
    let prefix = study_branch(slug);
    for b in git(repo, &["for-each-ref", "--format=%(refname)", "refs/heads"])?
        .lines()
        .filter_map(|l| l.strip_prefix("refs/heads/"))
        .filter(|b| *b == prefix || b.starts_with(&format!("{prefix}+")))
    {
        git(repo, &["branch", "-D", "-q", b])?;
        removed.push(b.to_owned());
    }
    let rej = format!("refs/autoresearch/{slug}/");
    for r in git(repo, &["for-each-ref", "--format=%(refname)", &rej])?
        .lines()
        .map(str::trim)
        .filter(|r| !r.is_empty())
    {
        git(repo, &["update-ref", "-d", r])?;
        removed.push(r.to_owned());
    }
    Ok(removed)
}

/// Pure-jj repo without git colocation → refused at draft time (140 §6).
pub fn is_git_repo(repo: &Path) -> bool {
    run_status(repo, None, &["rev-parse", "--is-inside-work-tree"])
        .map(|(ok, out, _)| ok && out.trim() == "true")
        .unwrap_or(false)
}

#[cfg(test)]
mod tests {
    use std::collections::BTreeMap;
    use std::path::{Path, PathBuf};

    use super::*;

    fn sh(cwd: &Path, args: &[&str]) -> String {
        // Hooks off here too: the test's own moves (the simulated model/user) are not under test.
        let out = Command::new("git")
            .arg("-C")
            .arg(cwd)
            .args(["-c", "core.hooksPath=/dev/null"])
            .args(args)
            .env("GIT_TERMINAL_PROMPT", "0")
            .output()
            .unwrap();
        assert!(
            out.status.success(),
            "git {args:?}: {}",
            String::from_utf8_lossy(&out.stderr)
        );
        String::from_utf8_lossy(&out.stdout).trim().to_owned()
    }

    fn commit_all(cwd: &Path, msg: &str) -> String {
        sh(cwd, &["add", "-A"]);
        sh(
            cwd,
            &[
                "-c",
                "user.name=u",
                "-c",
                "user.email=u@x",
                "commit",
                "-q",
                "-m",
                msg,
            ],
        );
        sh(cwd, &["rev-parse", "HEAD"])
    }

    /// A user repo with a hook that would leave a marker, an ignored dir, a bare `origin`.
    struct Fixture {
        _tmp: tempfile::TempDir,
        repo: PathBuf,
        remote: PathBuf,
        study: PathBuf,
        spec: StudySpec,
    }

    fn fixture() -> Fixture {
        let tmp = tempfile::tempdir().unwrap();
        let repo = tmp.path().join("repo");
        std::fs::create_dir_all(repo.join("src")).unwrap();
        std::fs::create_dir_all(repo.join("fixtures")).unwrap();
        sh(tmp.path(), &["init", "-q", "-b", "main", "repo"]);
        std::fs::write(repo.join(".gitignore"), "target/\n").unwrap();
        std::fs::write(repo.join("src/lib.rs"), "fn a() {}\nfn b() {}\n").unwrap();
        std::fs::write(repo.join("fixtures/a.txt"), "fixture\n").unwrap();
        std::fs::write(repo.join("bench.sh"), "echo METRIC t=1\n").unwrap();
        let hooks = repo.join(".githooks");
        std::fs::create_dir_all(&hooks).unwrap();
        for h in [
            "pre-commit",
            "post-commit",
            "post-checkout",
            "post-merge",
            "pre-auto-gc",
        ] {
            let p = hooks.join(h);
            std::fs::write(
                &p,
                "#!/bin/sh\necho ran > \"$(git rev-parse --git-common-dir)/HOOK_RAN\"\n",
            )
            .unwrap();
            #[cfg(unix)]
            {
                use std::os::unix::fs::PermissionsExt as _;
                std::fs::set_permissions(&p, std::fs::Permissions::from_mode(0o755)).unwrap();
            }
        }
        commit_all(&repo, "base");
        sh(&repo, &["config", "core.hooksPath", ".githooks"]);
        let remote = tmp.path().join("origin.git");
        sh(tmp.path(), &["init", "-q", "--bare", "origin.git"]);
        sh(
            &repo,
            &["remote", "add", "origin", remote.to_str().unwrap()],
        );
        sh(&repo, &["push", "-q", "origin", "main"]);
        let spec = StudySpec {
            slug: "s".into(),
            repo: repo.to_string_lossy().into_owned(),
            evaluator_paths: vec!["fixtures/".into(), "bench.sh".into()],
            scope: vec!["src/**".into()],
            off_limits: vec!["fixtures/**".into()],
            ..Default::default()
        };
        Fixture {
            study: tmp.path().join("data").join("s").join("worktree"),
            _tmp: tmp,
            repo,
            remote,
            spec,
        }
    }

    /// Every file of the working tree (not `.git`) → bytes, plus HEAD and the porcelain status.
    fn snapshot(repo: &Path) -> BTreeMap<String, Vec<u8>> {
        let mut m = BTreeMap::new();
        let mut stack = vec![repo.to_path_buf()];
        while let Some(d) = stack.pop() {
            for e in std::fs::read_dir(&d).unwrap() {
                let p = e.unwrap().path();
                if p.file_name().is_some_and(|n| n == ".git") {
                    continue;
                }
                if p.is_dir() {
                    stack.push(p);
                } else {
                    m.insert(
                        p.strip_prefix(repo).unwrap().to_string_lossy().into_owned(),
                        std::fs::read(&p).unwrap(),
                    );
                }
            }
        }
        m.insert(
            "<HEAD>".into(),
            sh(repo, &["rev-parse", "HEAD"]).into_bytes(),
        );
        m.insert(
            "<status>".into(),
            sh(repo, &["status", "--porcelain"]).into_bytes(),
        );
        m
    }

    fn hook_ran(repo: &Path) -> bool {
        repo.join(".git").join("HOOK_RAN").exists()
    }

    fn remote_refs(remote: &Path) -> String {
        sh(
            remote,
            &["for-each-ref", "--format=%(refname) %(objectname)"],
        )
    }

    fn meta(n: u32) -> KeepMeta {
        KeepMeta {
            slug: "s".into(),
            n,
            description: format!("experiment {n}"),
            metric_line: "t 2 -> 1 (-50.0%, 4.0 sigma, n=1)".into(),
            category: "algo".into(),
            deviations: Vec::new(),
        }
    }

    /// Twenty experiments (keeps with trailers, discards with saved patches, an
    /// evaluator edit excluded from a keep and caught by repair) leave the
    /// user's checkout byte-identical, run no hook, keep ignored files, push nothing.
    #[test]
    fn simulated_study_never_touches_the_user() {
        let f = fixture();
        let before = snapshot(&f.repo);
        let remote_before = remote_refs(&f.remote);
        let base = resolve_base(&f.repo, "HEAD", false).unwrap();
        let head0 = worktree_add(&f.repo, &f.spec, &f.study, &base).unwrap();
        assert_eq!(head0, base);
        assert_eq!(branch(&f.study).unwrap().as_deref(), Some("autoresearch/s"));
        std::fs::create_dir_all(f.study.join("target")).unwrap();
        std::fs::write(f.study.join("target/cache.bin"), b"cache").unwrap();
        let mut incumbent = base.clone();
        let mut kept = 0;
        let runs = f.study.parent().unwrap().join("runs");
        for n in 1..=20u32 {
            let stray = runs.join(n.to_string()).join("stray.patch");
            let repaired = repair(&f.study, &f.spec, &incumbent, &stray).unwrap();
            if n == 7 {
                // #6 kept with a dirty evaluator file left behind: repair saves and removes it.
                assert!(
                    repaired.is_some(),
                    "evaluator edit left by #6 must be repaired"
                );
                assert!(stray.exists());
                assert_eq!(
                    std::fs::read_to_string(f.study.join("fixtures/a.txt")).unwrap(),
                    "fixture\n"
                );
            } else {
                assert_eq!(repaired, None, "experiment {n}");
            }
            let lib = f.study.join("src/lib.rs");
            let text = std::fs::read_to_string(&lib).unwrap();
            std::fs::write(&lib, format!("{text}fn e{n}() {{}}\n")).unwrap();
            std::fs::write(f.study.join(format!("src/new{n}.rs")), "new\n").unwrap();
            if n == 6 {
                std::fs::write(f.study.join("fixtures/a.txt"), "tampered\n").unwrap();
            }
            let d = crate::custody::scope_diff(&f.spec, &f.study).unwrap();
            assert!(
                d.changed.contains(&"src/lib.rs".to_owned())
                    && d.changed.contains(&format!("src/new{n}.rs"))
            );
            if n == 6 {
                assert_eq!((d.added, d.removed), (3, 1));
                assert_eq!(d.off_limits, vec!["fixtures/a.txt"]);
            } else {
                assert_eq!((d.added, d.removed), (2, 0));
                assert!(d.deviations.is_empty() && d.off_limits.is_empty(), "{d:?}");
            }
            if n % 2 == 0 {
                let sha = keep(&f.study, &f.spec, &meta(n), None).unwrap();
                let shown = sh(
                    &f.study,
                    &["show", "--stat", "--format=%an <%ae>%n%B", &sha],
                );
                assert!(
                    shown.starts_with("theuth autoresearch <noreply@theuth.local>"),
                    "{shown}"
                );
                assert!(shown.contains(&format!("autoresearch(s) #{n}: experiment {n}")));
                assert!(
                    shown.contains("Autoresearch-Study: s")
                        && shown.contains(&format!("Autoresearch-Run: {n}"))
                );
                assert!(
                    shown.contains("Autoresearch-Metric: t 2 -> 1")
                        && shown.contains("Autoresearch-Category: algo")
                );
                assert!(
                    !shown.contains("fixtures/a.txt"),
                    "evaluator path committed: {shown}"
                );
                assert_eq!(
                    files_of(&f.study, &sha).unwrap(),
                    vec!["src/lib.rs".to_owned(), format!("src/new{n}.rs")]
                );
                incumbent = sha;
                kept += 1;
            } else {
                let patch = runs.join(n.to_string()).join("candidate.patch");
                let hash = discard(&f.study, &incumbent, &patch).unwrap();
                assert_eq!(hash.len(), 64);
                assert!(patch.metadata().unwrap().len() > 0);
                assert_eq!(head(&f.study).unwrap(), incumbent);
                assert!(!f.study.join(format!("src/new{n}.rs")).exists());
                assert_eq!(sh(&f.study, &["status", "--porcelain"]), "");
            }
            assert!(
                f.study.join("target/cache.bin").exists(),
                "ignored file lost at {n}"
            );
        }
        assert_eq!(kept, 10);
        assert_eq!(
            sh(&f.study, &["rev-list", "--count", &format!("{base}..HEAD")]),
            "10"
        );
        assert_eq!(snapshot(&f.repo), before, "the user's checkout changed");
        assert!(!hook_ran(&f.repo), "a hook ran");
        assert_eq!(
            remote_refs(&f.remote),
            remote_before,
            "something reached the remote"
        );
        assert_eq!(
            sh(
                &f.repo,
                &[
                    "branch",
                    "--list",
                    "--format=%(refname:short)",
                    "autoresearch/*"
                ]
            ),
            "autoresearch/s"
        );
    }

    #[test]
    fn discard_patch_reapplies_cleanly() {
        let f = fixture();
        let base = resolve_base(&f.repo, "HEAD", false).unwrap();
        worktree_add(&f.repo, &f.spec, &f.study, &base).unwrap();
        std::fs::write(f.study.join("src/lib.rs"), "fn a() {}\nfn b() { 1 }\n").unwrap();
        std::fs::write(f.study.join("src/bin.dat"), [0u8, 159, 146, 150, 10, 0]).unwrap();
        std::fs::create_dir_all(f.study.join("src/deep")).unwrap();
        std::fs::write(f.study.join("src/deep/x.rs"), "deep\n").unwrap();
        std::fs::create_dir_all(f.study.join("target")).unwrap();
        std::fs::write(f.study.join("target/o"), "o").unwrap();
        let want = snapshot(&f.study);
        let patch = f.study.parent().unwrap().join("runs/1/candidate.patch");
        discard(&f.study, &base, &patch).unwrap();
        assert_eq!(sh(&f.study, &["status", "--porcelain"]), "");
        assert!(!f.study.join("src/deep").exists() && !f.study.join("src/bin.dat").exists());
        assert!(f.study.join("target/o").exists());
        apply_patch(&f.study, &patch).unwrap();
        let mut got = snapshot(&f.study);
        got.remove("<status>");
        let mut want = want;
        want.remove("<status>");
        assert_eq!(got, want);
        // An empty patch is a no-op.
        discard(&f.study, &base, &patch).unwrap();
        let empty = f.study.parent().unwrap().join("runs/2/candidate.patch");
        assert_eq!(discard(&f.study, &base, &empty).unwrap(), sha256_hex(b""));
        apply_patch(&f.study, &empty).unwrap();
    }

    #[test]
    fn repair_fixes_every_kind_of_drift() {
        let f = fixture();
        let base = resolve_base(&f.repo, "HEAD", false).unwrap();
        worktree_add(&f.repo, &f.spec, &f.study, &base).unwrap();
        let stray = f.study.parent().unwrap().join("runs/3/stray.patch");
        // The model committed on the branch and left an edit and an untracked file.
        std::fs::write(f.study.join("src/lib.rs"), "committed\n").unwrap();
        commit_all(&f.study, "model commit");
        std::fs::write(f.study.join("src/lib.rs"), "committed+dirty\n").unwrap();
        std::fs::write(f.study.join("src/stray.rs"), "stray\n").unwrap();
        let msg = repair(&f.study, &f.spec, &base, &stray).unwrap().unwrap();
        assert!(
            msg.contains("HEAD was") && msg.contains("uncommitted path"),
            "{msg}"
        );
        assert_eq!(head(&f.study).unwrap(), base);
        assert_eq!(branch(&f.study).unwrap().as_deref(), Some("autoresearch/s"));
        assert_eq!(sh(&f.study, &["status", "--porcelain"]), "");
        let p = std::fs::read_to_string(&stray).unwrap();
        assert!(
            p.contains("committed+dirty") && p.contains("src/stray.rs"),
            "{p}"
        );
        // Detached and on another branch.
        sh(&f.study, &["checkout", "-q", "--detach"]);
        let msg = repair(&f.study, &f.spec, &base, &stray).unwrap().unwrap();
        assert!(msg.contains("detached"), "{msg}");
        assert_eq!(branch(&f.study).unwrap().as_deref(), Some("autoresearch/s"));
        sh(&f.study, &["checkout", "-q", "-b", "elsewhere"]);
        let msg = repair(&f.study, &f.spec, &base, &stray).unwrap().unwrap();
        assert!(msg.contains("`elsewhere`"), "{msg}");
        assert_eq!(branch(&f.study).unwrap().as_deref(), Some("autoresearch/s"));
        assert_eq!(repair(&f.study, &f.spec, &base, &stray).unwrap(), None);
        assert!(!hook_ran(&f.repo));
    }

    #[test]
    fn reject_keeps_the_commit_and_restore_reapplies_it() {
        let f = fixture();
        let base = resolve_base(&f.repo, "HEAD", false).unwrap();
        worktree_add(&f.repo, &f.spec, &f.study, &base).unwrap();
        std::fs::write(f.study.join("src/lib.rs"), "gamed\n").unwrap();
        let sha = keep(&f.study, &f.spec, &meta(4), Some("Custom Author <c@x>")).unwrap();
        assert_eq!(
            sh(&f.study, &["log", "-1", "--format=%an"]),
            "Custom Author"
        );
        reject(&f.study, "s", 4, &base).unwrap();
        assert_eq!(head(&f.study).unwrap(), base);
        assert_eq!(
            sh(&f.study, &["rev-parse", "refs/autoresearch/s/rejected/4"]),
            sha
        );
        assert_eq!(
            std::fs::read_to_string(f.study.join("src/lib.rs")).unwrap(),
            "fn a() {}\nfn b() {}\n"
        );
        restore_rejected(&f.study, "s", 4).unwrap();
        assert_eq!(head(&f.study).unwrap(), base);
        assert_eq!(
            std::fs::read_to_string(f.study.join("src/lib.rs")).unwrap(),
            "gamed\n"
        );
        assert_eq!(sh(&f.study, &["status", "--porcelain"]), "M src/lib.rs");
        // A conflicting restore is refused and leaves the tree clean.
        discard(
            &f.study,
            &base,
            &f.study.parent().unwrap().join("runs/4/candidate.patch"),
        )
        .unwrap();
        std::fs::write(f.study.join("src/lib.rs"), "moved on\n").unwrap();
        let inc = keep(&f.study, &f.spec, &meta(5), None).unwrap();
        let err = restore_rejected(&f.study, "s", 4).unwrap_err().to_string();
        assert!(err.contains("no longer applies"), "{err}");
        assert_eq!(head(&f.study).unwrap(), inc);
        assert_eq!(sh(&f.study, &["status", "--porcelain"]), "");
    }

    #[test]
    fn resolve_base_refuses_dirty_unless_included_then_snapshots_on_the_study_branch() {
        let f = fixture();
        assert!(
            resolve_base(&f.repo, "nope", false)
                .unwrap_err()
                .to_string()
                .contains("does not name a commit")
        );
        std::fs::write(f.repo.join("src/lib.rs"), "user edit\n").unwrap();
        std::fs::write(f.repo.join("src/untracked.rs"), "u\n").unwrap();
        let err = resolve_base(&f.repo, "HEAD", false)
            .unwrap_err()
            .to_string();
        assert!(err.contains("2 uncommitted path(s)"), "{err}");
        assert!(resolve_base(&f.repo, "HEAD~0", true).is_ok());
        let before = snapshot(&f.repo);
        let base = resolve_base(&f.repo, "HEAD", true).unwrap();
        let head = worktree_add(&f.repo, &f.spec, &f.study, &base).unwrap();
        assert_ne!(head, base);
        assert_eq!(
            sh(&f.study, &["log", "-1", "--format=%s"]),
            "autoresearch(s): base snapshot"
        );
        assert_eq!(
            std::fs::read_to_string(f.study.join("src/lib.rs")).unwrap(),
            "user edit\n"
        );
        assert_eq!(
            std::fs::read_to_string(f.study.join("src/untracked.rs")).unwrap(),
            "u\n"
        );
        assert_eq!(sh(&f.study, &["status", "--porcelain"]), "");
        assert_eq!(
            snapshot(&f.repo),
            before,
            "the user's dirty checkout was touched"
        );
        assert_eq!(sh(&f.repo, &["rev-parse", "main"]), base, "main moved");
        assert!(!hook_ran(&f.repo));
        assert!(!is_git_repo(f.repo.parent().unwrap()));
        assert!(is_git_repo(&f.repo));
    }

    #[test]
    fn scope_diff_counts_lines_without_whitespace_and_hashes_stably() {
        let f = fixture();
        let base = resolve_base(&f.repo, "HEAD", false).unwrap();
        worktree_add(&f.repo, &f.spec, &f.study, &base).unwrap();
        std::fs::write(f.study.join("src/lib.rs"), "fn a() {}\nfn c() {}\n").unwrap();
        std::fs::write(f.study.join("README.md"), "out of scope\n").unwrap();
        let d1 = crate::custody::scope_diff(&f.spec, &f.study).unwrap();
        let h1 = patch_hash(&f.study, &base).unwrap();
        assert_eq!((d1.added, d1.removed), (2, 1));
        assert_eq!(d1.deviations, vec!["README.md"]);
        assert!(d1.removed <= d1.added);
        // Whitespace-only edits change neither the counts nor the hash.
        std::fs::write(f.study.join("src/lib.rs"), "fn a()  {}\nfn c() {}\n").unwrap();
        let d2 = crate::custody::scope_diff(&f.spec, &f.study).unwrap();
        assert_eq!((d2.added, d2.removed), (2, 1));
        assert_eq!(patch_hash(&f.study, &base).unwrap(), h1);
        std::fs::write(f.study.join("src/lib.rs"), "fn a() {}\n").unwrap();
        let d3 = crate::custody::scope_diff(&f.spec, &f.study).unwrap();
        assert_ne!(patch_hash(&f.study, &base).unwrap(), h1);
        assert_eq!(
            (d3.added, d3.removed),
            (1, 1),
            "README.md still counts: {d3:?}"
        );
        assert!(d3.removed <= d3.added);
        std::fs::remove_file(f.study.join("README.md")).unwrap();
        let d4 = crate::custody::scope_diff(&f.spec, &f.study).unwrap();
        assert_eq!(
            (d4.added, d4.removed, d4.changed.len()),
            (0, 1, 1),
            "{d4:?}"
        );
        assert!(d4.removed > d4.added);
        let text = diff_text(&f.study, &base, "HEAD", 10_000).unwrap();
        assert_eq!(text, "", "HEAD == base");
        let sha = keep(&f.study, &f.spec, &meta(1), None).unwrap();
        let text = diff_text(&f.study, &base, &sha, 40).unwrap();
        assert!(text.contains("truncated at 40 bytes"));
    }

    /// §6 Revision 2 (140-P live: keeps committed `__pycache__/*.pyc` when the repo had no
    /// `.gitignore`): junk is neither a change, a deviation, dirt nor part of a keep —
    /// unless a `scope` pattern names it.
    #[test]
    fn keeps_never_commit_build_junk_unless_scoped() {
        let f = fixture();
        let base = resolve_base(&f.repo, "HEAD", false).unwrap();
        worktree_add(&f.repo, &f.spec, &f.study, &base).unwrap();
        std::fs::create_dir_all(f.study.join("src/__pycache__")).unwrap();
        std::fs::create_dir_all(f.study.join("node_modules/x")).unwrap();
        std::fs::write(f.study.join("src/lib.rs"), "fn a() {}\nfn c() {}\n").unwrap();
        std::fs::write(
            f.study.join("src/__pycache__/m.cpython-312.pyc"),
            b"\x00junk",
        )
        .unwrap();
        std::fs::write(f.study.join("compiled.pyc"), b"\x00junk").unwrap();
        std::fs::write(f.study.join(".DS_Store"), b"\x00junk").unwrap();
        std::fs::write(
            f.study.join("node_modules/x/index.js"),
            "module.exports = 1\n",
        )
        .unwrap();
        let d = crate::custody::scope_diff(&f.spec, &f.study).unwrap();
        assert_eq!(d.changed, vec!["src/lib.rs"], "{d:?}");
        assert!(d.deviations.is_empty(), "{d:?}");
        assert_eq!((d.added, d.removed), (1, 1));
        let sha = keep(&f.study, &f.spec, &meta(1), None).unwrap();
        assert_eq!(files_of(&f.study, &sha).unwrap(), vec!["src/lib.rs"]);
        // The junk is still on disk but is not drift at the next begin.
        assert!(f.study.join("compiled.pyc").exists());
        let stray = f._tmp.path().join("stray.patch");
        assert_eq!(repair(&f.study, &f.spec, &sha, &stray).unwrap(), None);
        // Explicitly in scope: the user's call, counted and committed.
        let mut scoped = f.spec.clone();
        scoped.scope.push("**/*.pyc".into());
        let d = crate::custody::scope_diff(&scoped, &f.study).unwrap();
        assert_eq!(
            d.changed,
            vec![
                ".DS_Store",
                "compiled.pyc",
                "node_modules/x/index.js",
                "src/__pycache__/m.cpython-312.pyc"
            ],
            "{d:?}"
        );
        assert_eq!(d.deviations, vec![".DS_Store", "node_modules/x/index.js"]);
        let sha2 = keep(&f.study, &scoped, &meta(2), None).unwrap();
        assert_eq!(files_of(&f.study, &sha2).unwrap().len(), 4);
    }

    #[test]
    fn discard_study_removes_worktrees_branches_and_refs_only() {
        let f = fixture();
        let base = resolve_base(&f.repo, "HEAD", false).unwrap();
        worktree_add(&f.repo, &f.spec, &f.study, &base).unwrap();
        std::fs::write(f.study.join("src/lib.rs"), "x\n").unwrap();
        let sha = keep(&f.study, &f.spec, &meta(1), None).unwrap();
        reject(&f.study, "s", 1, &base).unwrap();
        branch_at(&f.repo, "autoresearch/s+1-lib", &sha).unwrap();
        let scratch = f.study.parent().unwrap().join("finalize/cs-1");
        worktree_add_detached(&f.repo, &scratch, &base).unwrap();
        sh(&f.repo, &["branch", "unrelated"]);
        let before = snapshot(&f.repo);
        let removed = discard_study(&f.repo, "s", f.study.parent().unwrap()).unwrap();
        assert_eq!(
            removed,
            vec![
                "autoresearch/s",
                "autoresearch/s+1-lib",
                "refs/autoresearch/s/rejected/1"
            ]
        );
        assert!(!f.study.exists() && !scratch.exists());
        assert_eq!(sh(&f.repo, &["worktree", "list"]).lines().count(), 1);
        assert_eq!(
            sh(&f.repo, &["branch", "--format=%(refname:short)"]),
            "main\nunrelated"
        );
        assert_eq!(snapshot(&f.repo), before);
        worktree_remove(&f.repo, &f.study).unwrap();
    }

    /// R-16: `branch_at` never force-moves a branch the plugin did not make;
    /// its own changeset branch (tip carries the study trailer) may be re-pointed.
    #[test]
    fn branch_at_refuses_to_move_a_user_branch() {
        let f = fixture();
        let base = resolve_base(&f.repo, "HEAD", false).unwrap();
        worktree_add(&f.repo, &f.spec, &f.study, &base).unwrap();
        std::fs::write(f.study.join("src/lib.rs"), "x\n").unwrap();
        let sha = keep(&f.study, &f.spec, &meta(1), None).unwrap();
        sh(&f.repo, &["branch", "mine", &base]);
        let err = branch_at(&f.repo, "mine", &sha).unwrap_err().to_string();
        assert!(err.contains("not made by autoresearch"), "{err}");
        assert_eq!(sh(&f.repo, &["rev-parse", "mine"]), base);
        branch_at(&f.repo, "autoresearch/s+1-lib", &sha).unwrap();
        std::fs::write(f.study.join("src/lib.rs"), "y\n").unwrap();
        let sha2 = keep(&f.study, &f.spec, &meta(2), None).unwrap();
        branch_at(&f.repo, "autoresearch/s+1-lib", &sha2).unwrap();
        assert_eq!(sh(&f.repo, &["rev-parse", "autoresearch/s+1-lib"]), sha2);
        worktree_remove(&f.repo, &f.study).unwrap();
    }

    #[test]
    fn merge_helper_refuses_dirty_and_merges_no_ff() {
        let f = fixture();
        let base = resolve_base(&f.repo, "HEAD", false).unwrap();
        worktree_add(&f.repo, &f.spec, &f.study, &base).unwrap();
        std::fs::write(f.study.join("src/lib.rs"), "better\n").unwrap();
        let sha = keep(&f.study, &f.spec, &meta(1), None).unwrap();
        branch_at(&f.repo, "autoresearch/s+1-lib", &sha).unwrap();
        std::fs::write(f.repo.join("src/lib.rs"), "user wip\n").unwrap();
        let err = merge_into_checkout(&f.repo, "autoresearch/s+1-lib", None)
            .unwrap_err()
            .to_string();
        assert!(err.contains("uncommitted"), "{err}");
        std::fs::write(f.repo.join("src/lib.rs"), "fn a() {}\nfn b() {}\n").unwrap();
        // R-15: the merge commit is the user's own, from their git identity.
        sh(&f.repo, &["config", "user.name", "Sky"]);
        sh(&f.repo, &["config", "user.email", "sky@x"]);
        let merged = merge_into_checkout(&f.repo, "autoresearch/s+1-lib", None).unwrap();
        assert_eq!(sh(&f.repo, &["rev-parse", "HEAD"]), merged);
        assert_eq!(
            sh(&f.repo, &["log", "-1", "--format=%an <%ae>"]),
            "Sky <sky@x>"
        );
        assert_eq!(
            sh(&f.repo, &["rev-list", "--parents", "-1", "HEAD"])
                .split(' ')
                .count(),
            3,
            "a merge commit"
        );
        assert_eq!(
            std::fs::read_to_string(f.repo.join("src/lib.rs")).unwrap(),
            "better\n"
        );
        assert_eq!(
            checkout_state(&f.repo).unwrap(),
            (Some("main".into()), true)
        );
        assert!(!hook_ran(&f.repo));
        assert_eq!(remote_refs(&f.remote).lines().count(), 1, "nothing pushed");
    }

    #[test]
    fn cherry_pick_reports_conflicts_and_aborts() {
        let f = fixture();
        let base = resolve_base(&f.repo, "HEAD", false).unwrap();
        worktree_add(&f.repo, &f.spec, &f.study, &base).unwrap();
        std::fs::write(f.study.join("src/lib.rs"), "one\n").unwrap();
        let c1 = commit_all(&f.study, "one");
        std::fs::write(f.study.join("src/lib.rs"), "two\n").unwrap();
        let c2 = commit_all(&f.study, "two");
        let scratch = f.study.parent().unwrap().join("finalize/cs-1");
        worktree_add_detached(&f.repo, &scratch, &base).unwrap();
        assert!(
            !cherry_pick(&scratch, &c2).unwrap(),
            "two on base conflicts"
        );
        assert_eq!(head(&scratch).unwrap(), base);
        assert_eq!(sh(&scratch, &["status", "--porcelain"]), "");
        assert!(cherry_pick(&scratch, &c1).unwrap());
        assert!(cherry_pick(&scratch, &c2).unwrap());
        assert_eq!(
            std::fs::read_to_string(scratch.join("src/lib.rs")).unwrap(),
            "two\n"
        );
        assert_eq!(
            KeepMeta {
                deviations: vec!["README.md".into()],
                ..meta(2)
            }
            .message()
            .lines()
            .last(),
            Some("Autoresearch-Deviations: README.md")
        );
    }
}
