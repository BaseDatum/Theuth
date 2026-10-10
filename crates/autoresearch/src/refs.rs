//! 159 W1 (consumed by W4): the front and cell archive refs, `refs/autoresearch/<slug>/<kind>/<key>`
//! (159 W-f), outside `refs/heads` so no branch can sit under the study branch's name.
//! Owner: W1. The skeleton implements the four plumbing calls; W1 may extend this file.

use std::path::Path;

use crate::error::Result;
use crate::git;

/// Which archive a ref belongs to.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum RefKind {
    /// W1: a Pareto front member, key `f<n>`.
    Front,
    /// W4: a population cell elite, key `i<island>:<bin>.<bin>`.
    Cell,
}

impl RefKind {
    fn word(self) -> &'static str {
        match self {
            Self::Front => "front",
            Self::Cell => "cell",
        }
    }
}

/// `refs/autoresearch/<slug>/<kind>/<key>`. A cell key `i<island>:<bin>.<bin>` is written
/// `i<island>/<bin>.<bin>` — git refuses `:` in a ref name — and [`list`] maps it back.
pub fn name(slug: &str, kind: RefKind, key: &str) -> String {
    format!(
        "refs/autoresearch/{slug}/{}/{}",
        kind.word(),
        encode(kind, key)
    )
}

fn encode(kind: RefKind, key: &str) -> String {
    match kind {
        RefKind::Front => key.to_owned(),
        RefKind::Cell => key.replacen(':', "/", 1),
    }
}

fn decode(kind: RefKind, component: &str) -> String {
    match kind {
        RefKind::Front => component.to_owned(),
        RefKind::Cell => component.replacen('/', ":", 1),
    }
}

/// Point the ref at `sha` (created when absent).
pub fn update(repo: &Path, slug: &str, kind: RefKind, key: &str, sha: &str) -> Result<()> {
    git::plumbing(repo, &["update-ref", &name(slug, kind, key), sha]).map(|_| ())
}

/// Delete the ref; a missing ref is not an error.
pub fn delete(repo: &Path, slug: &str, kind: RefKind, key: &str) -> Result<()> {
    let r = name(slug, kind, key);
    if git::plumbing(repo, &["show-ref", "--verify", "--quiet", &r]).is_err() {
        return Ok(());
    }
    git::plumbing(repo, &["update-ref", "-d", &r]).map(|_| ())
}

/// Every `(key, sha)` under `refs/autoresearch/<slug>/<kind>/`, sorted by key.
pub fn list(repo: &Path, slug: &str, kind: RefKind) -> Result<Vec<(String, String)>> {
    let prefix = format!("refs/autoresearch/{slug}/{}/", kind.word());
    let out = git::plumbing(
        repo,
        &["for-each-ref", "--format=%(refname) %(objectname)", &prefix],
    )?;
    Ok(out
        .lines()
        .filter_map(|l| {
            let (r, sha) = l.split_once(' ')?;
            Some((decode(kind, r.strip_prefix(&prefix)?), sha.to_owned()))
        })
        .collect())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn names_follow_w_f() {
        assert_eq!(
            name("s", RefKind::Front, "f1"),
            "refs/autoresearch/s/front/f1"
        );
        assert_eq!(
            name("s", RefKind::Cell, "i0:1.2"),
            "refs/autoresearch/s/cell/i0/1.2"
        );
    }

    #[test]
    fn update_list_delete_round_trip() {
        let dir = tempfile::tempdir().unwrap();
        let repo = dir.path();
        let sh = |args: &[&str]| {
            let o = std::process::Command::new("git")
                .args(args)
                .current_dir(repo)
                .output()
                .unwrap();
            assert!(o.status.success(), "{}", String::from_utf8_lossy(&o.stderr));
            String::from_utf8_lossy(&o.stdout).trim().to_owned()
        };
        sh(&["init", "-q"]);
        sh(&[
            "-c",
            "user.name=t",
            "-c",
            "user.email=t@t",
            "commit",
            "-q",
            "--allow-empty",
            "-m",
            "x",
        ]);
        let sha = sh(&["rev-parse", "HEAD"]);
        update(repo, "s", RefKind::Front, "f1", &sha).unwrap();
        update(repo, "s", RefKind::Cell, "i0:0.0", &sha).unwrap();
        assert_eq!(
            list(repo, "s", RefKind::Front).unwrap(),
            vec![("f1".to_owned(), sha.clone())]
        );
        delete(repo, "s", RefKind::Front, "f1").unwrap();
        delete(repo, "s", RefKind::Front, "f1").unwrap();
        assert!(list(repo, "s", RefKind::Front).unwrap().is_empty());
        assert_eq!(
            list(repo, "s", RefKind::Cell).unwrap(),
            vec![("i0:0.0".to_owned(), sha)]
        );
    }
}
