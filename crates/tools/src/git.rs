//! Git repos as a source of skills and MCP servers: clone, pull, list. Runs
//! the user's `git` binary (their keys, config and proxies already work);
//! see design doc 04 §2.2 for why not `git2`/`gix`.

use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};
use tokio::process::Command;

/// A checked-out repo directly under a source directory.
#[derive(Clone, Debug)]
pub struct Repo {
    /// Directory name (what `add_*_repo` chose).
    pub name: String,
    /// Absolute path.
    pub path: PathBuf,
}

/// Outcome of one `git pull` (`status`: `updated` | `unchanged` | `error: …`).
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, schemars::JsonSchema)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS), ts(export))]
pub struct SyncResult {
    /// Repo directory name.
    pub name: String,
    /// `updated`, `unchanged`, or `error: <message>`.
    pub status: String,
}

/// Immediate subdirectories of `dir` that contain `.git`, sorted by name.
/// Missing `dir` → empty.
pub fn repos(dir: &Path) -> Vec<Repo> {
    let Ok(entries) = std::fs::read_dir(dir) else {
        return Vec::new();
    };
    let mut out: Vec<Repo> = entries
        .flatten()
        .map(|e| e.path())
        .filter(|p| p.join(".git").exists())
        .filter_map(|path| {
            let name = path.file_name()?.to_str()?.to_owned();
            Some(Repo { name, path })
        })
        .collect();
    out.sort_by(|a, b| a.name.cmp(&b.name));
    out
}

/// The repo (if any) whose directory contains `path`, among `repos(dir)`.
pub fn owner(dir: &Path, path: &Path) -> Option<String> {
    repos(dir)
        .into_iter()
        .find(|r| path.starts_with(&r.path))
        .map(|r| r.name)
}

/// Directory name for a clone: the URL's last path segment minus `.git`.
pub fn repo_name(url: &str) -> String {
    let tail = url
        .trim_end_matches('/')
        .rsplit(['/', ':'])
        .next()
        .unwrap_or(url);
    tail.strip_suffix(".git").unwrap_or(tail).to_owned()
}

/// `[a-z0-9][a-z0-9._-]*`, no path separators: safe as a file/dir name.
pub fn valid_name(name: &str) -> bool {
    let mut chars = name.chars();
    chars
        .next()
        .is_some_and(|c| c.is_ascii_lowercase() || c.is_ascii_digit())
        && chars
            .all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || matches!(c, '.' | '_' | '-'))
        && name != ".."
}

/// `git clone --depth 1 <url> <dest>`. `dest` must not exist.
pub async fn clone(url: &str, dest: &Path) -> anyhow::Result<()> {
    anyhow::ensure!(!dest.exists(), "{} already exists", dest.display());
    run(None, &["clone", "--depth", "1", url], Some(dest)).await?;
    Ok(())
}

/// `git pull --ff-only` in `path`; `updated` when HEAD moved.
pub async fn pull(path: &Path) -> anyhow::Result<bool> {
    let before = run(Some(path), &["rev-parse", "HEAD"], None).await?;
    run(Some(path), &["pull", "--ff-only", "--quiet"], None).await?;
    let after = run(Some(path), &["rev-parse", "HEAD"], None).await?;
    Ok(before != after)
}

/// `origin` URL of the repo at `path`, if any.
pub async fn remote_url(path: &Path) -> Option<String> {
    run(Some(path), &["remote", "get-url", "origin"], None)
        .await
        .ok()
        .filter(|s| !s.is_empty())
}

/// Pull every repo under `dir`; one result per repo, in name order.
pub async fn sync_all(dir: &Path) -> Vec<SyncResult> {
    let mut out = Vec::new();
    for r in repos(dir) {
        let status = match pull(&r.path).await {
            Ok(true) => "updated".to_owned(),
            Ok(false) => "unchanged".to_owned(),
            Err(e) => format!("error: {e:#}"),
        };
        out.push(SyncResult {
            name: r.name,
            status,
        });
    }
    out
}

/// Run git with `args` (plus `extra` path appended), trimmed stdout on success,
/// stderr as the error otherwise.
async fn run(cwd: Option<&Path>, args: &[&str], extra: Option<&Path>) -> anyhow::Result<String> {
    let mut cmd = Command::new("git");
    cmd.args(args);
    if let Some(p) = extra {
        cmd.arg(p);
    }
    if let Some(c) = cwd {
        cmd.current_dir(c);
    }
    platform::process::hide_console(cmd.as_std_mut());
    let out = cmd
        .output()
        .await
        .map_err(|e| anyhow::anyhow!("git not runnable: {e}"))?;
    anyhow::ensure!(
        out.status.success(),
        "git {} failed: {}",
        args.join(" "),
        String::from_utf8_lossy(&out.stderr).trim()
    );
    Ok(String::from_utf8_lossy(&out.stdout).trim().to_owned())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn names() {
        assert_eq!(
            repo_name("https://github.com/anthropics/skills.git"),
            "skills"
        );
        assert_eq!(repo_name("git@github.com:skymoore/brush"), "brush");
        assert_eq!(repo_name("https://x.y/a/b/"), "b");
        assert!(valid_name("my-skills.v2"));
        assert!(!valid_name("Bad"));
        assert!(!valid_name("../x"));
        assert!(!valid_name("-x"));
        assert!(!valid_name(""));
    }

    #[tokio::test]
    async fn clone_pull_and_list_against_a_local_bare_repo() {
        let guard = crate::TestDir::new("git");
        let root = guard.0.clone();
        let src = root.join("src");
        let bare = root.join("bare.git");
        let dir = root.join("skills");
        std::fs::create_dir_all(&src).unwrap();
        let sh = |args: &[&str], cwd: &Path| {
            let out = std::process::Command::new("git")
                .args(args)
                .current_dir(cwd)
                .env("GIT_AUTHOR_NAME", "t")
                .env("GIT_AUTHOR_EMAIL", "t@t")
                .env("GIT_COMMITTER_NAME", "t")
                .env("GIT_COMMITTER_EMAIL", "t@t")
                .output()
                .unwrap();
            assert!(
                out.status.success(),
                "{}",
                String::from_utf8_lossy(&out.stderr)
            );
        };
        sh(&["init", "-q", "-b", "main"], &src);
        std::fs::write(src.join("a.md"), "one").unwrap();
        sh(&["add", "."], &src);
        sh(&["commit", "-q", "-m", "one"], &src);
        sh(
            &[
                "clone",
                "-q",
                "--bare",
                src.to_str().unwrap(),
                bare.to_str().unwrap(),
            ],
            &root,
        );

        clone(bare.to_str().unwrap(), &dir.join("pack"))
            .await
            .unwrap();
        assert_eq!(
            repos(&dir)
                .iter()
                .map(|r| r.name.as_str())
                .collect::<Vec<_>>(),
            ["pack"]
        );
        assert_eq!(owner(&dir, &dir.join("pack/a.md")).as_deref(), Some("pack"));
        assert_eq!(
            remote_url(&dir.join("pack")).await.as_deref(),
            bare.to_str()
        );
        assert!(!pull(&dir.join("pack")).await.unwrap());

        std::fs::write(src.join("a.md"), "two").unwrap();
        sh(&["commit", "-q", "-am", "two"], &src);
        sh(&["push", "-q", bare.to_str().unwrap(), "main"], &src);
        assert_eq!(
            sync_all(&dir).await,
            [SyncResult {
                name: "pack".into(),
                status: "updated".into()
            }]
        );
        assert!(
            clone(bare.to_str().unwrap(), &dir.join("pack"))
                .await
                .is_err()
        );
    }
}
