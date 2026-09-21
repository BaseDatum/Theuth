//! `glob`: find files by name pattern (12 §10). Same walker and conventions
//! as `grep` (gitignore respected, hidden skipped, no symlink following);
//! results are ordered newest-modified first so the file the user just
//! touched is on top.

use std::path::{Path, PathBuf};
use std::time::SystemTime;

use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

use crate::{Risk, Tool, ToolCx, ToolOutput};

/// Default result cap for `glob`.
const GLOB_LIMIT: u32 = 500;

/// Arguments for [`Glob`].
#[derive(Debug, Serialize, Deserialize, JsonSchema)]
pub struct GlobArgs {
    /// Gitignore-style glob. `*` does not cross `/`, `**` does. Without a
    /// `/` it matches file names anywhere under `path` (`*.rs`); with one it
    /// matches paths relative to `path` (`src/**/*.rs`).
    pub pattern: String,
    /// Directory to search; default: the process cwd.
    #[serde(default)]
    pub path: Option<PathBuf>,
    /// Maximum number of paths to return; default 500.
    #[serde(default)]
    pub limit: Option<u32>,
    /// Include dot-files and dot-directories.
    #[serde(default)]
    pub hidden: bool,
}

/// Find files by glob pattern.
pub struct Glob;

#[async_trait::async_trait]
impl Tool for Glob {
    const NAME: &'static str = "glob";
    const RISK: Risk = Risk::ReadOnly;
    type Args = GlobArgs;
    const RETURNS: &'static str = "dict";

    fn description(&self) -> String {
        "Find files by glob pattern: `*` matches within one path segment, `**` crosses directories; a pattern without `/` matches file names anywhere under `path` (e.g. `*.rs`), one with `/` matches paths relative to `path` (e.g. `src/**/*.rs`). Respects .gitignore and skips hidden files unless `hidden` is true; directories and symlinks are not listed. Returns paths relative to `path` (default: cwd), newest-modified first, up to `limit` (default 500).".into()
    }

    async fn call(&self, a: GlobArgs, _: ToolCx) -> anyhow::Result<ToolOutput> {
        let root = match a.path {
            Some(p) => p,
            None => std::env::current_dir()?,
        };
        let limit = a.limit.unwrap_or(GLOB_LIMIT).max(1) as usize;
        let shown = root.display().to_string();
        let pattern = a.pattern;
        let (files, truncated) = tokio::task::spawn_blocking({
            let pattern = pattern.clone();
            move || glob(&root, &pattern, limit, a.hidden)
        })
        .await??;
        Ok(render(files, truncated, limit, &pattern, &shown))
    }
}

/// `glob` result: relative paths, and whether the cap cut the walk short.
type Found = (Vec<String>, bool);

/// Walk `root` (gitignore respected, hidden files skipped unless `hidden`,
/// symlinks not followed) and collect files matching `pattern`, relative to
/// `root`, newest modification time first (ties by path). Stops after
/// `limit + 1` matches; the flag says whether that happened. The message of
/// `Err` is what the model reads.
fn glob(root: &Path, pattern: &str, limit: usize, hidden: bool) -> anyhow::Result<Found> {
    anyhow::ensure!(
        root.is_dir(),
        "{}: not a directory; `path` must be a directory to search under",
        root.display()
    );
    let matcher = globset::GlobBuilder::new(pattern)
        .literal_separator(true)
        .build()
        .map_err(|e| anyhow::anyhow!("invalid glob `{pattern}`: {e}"))?
        .compile_matcher();
    // ripgrep's `-g` rule, done with one matcher and two candidates: a
    // pattern without `/` is tested against the file name, one with `/`
    // against the path relative to `root`.
    let by_name = !pattern.contains('/');
    let mut walk = ignore::WalkBuilder::new(root);
    walk.hidden(!hidden);
    let mut found: Vec<(SystemTime, String)> = Vec::new();
    let mut truncated = false;
    for entry in walk.build() {
        let Ok(entry) = entry else { continue };
        if !entry.file_type().is_some_and(|t| t.is_file()) {
            continue;
        }
        let path = entry.path();
        let rel = path.strip_prefix(root).unwrap_or(path);
        let hit = if by_name {
            path.file_name().is_some_and(|n| matcher.is_match(n))
        } else {
            matcher.is_match(rel)
        };
        if !hit {
            continue;
        }
        if found.len() == limit {
            truncated = true;
            break;
        }
        let mtime = entry
            .metadata()
            .and_then(|m| m.modified().map_err(Into::into))
            .unwrap_or(SystemTime::UNIX_EPOCH);
        found.push((mtime, rel.display().to_string()));
    }
    found.sort_by(|(ta, pa), (tb, pb)| tb.cmp(ta).then_with(|| pa.cmp(pb)));
    Ok((found.into_iter().map(|(_, p)| p).collect(), truncated))
}

/// Render the tool result: one path per line, a cap header when truncated,
/// a plain no-match line, `structured = {"files", "truncated"}`.
fn render(
    files: Vec<String>,
    truncated: bool,
    limit: usize,
    pattern: &str,
    root: &str,
) -> ToolOutput {
    let mut out = ToolOutput::ok(String::new());
    out.content = if files.is_empty() {
        format!("no files match {pattern} under {root}")
    } else if truncated {
        format!(
            "[glob: showing {limit} of {limit}+ matches; raise limit or narrow the pattern]\n{}",
            files.join("\n")
        )
    } else {
        files.join("\n")
    };
    out.structured = Some(serde_json::json!({ "files": files, "truncated": truncated }));
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::Duration;

    /// Temp tree with a `.git` marker so `.gitignore` is honoured (the walker
    /// only reads it inside a repository).
    fn tree(files: &[&str]) -> PathBuf {
        let root = std::env::temp_dir().join(format!("theuth-glob-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir_all(root.join(".git")).unwrap();
        for f in files {
            let p = root.join(f);
            std::fs::create_dir_all(p.parent().unwrap()).unwrap();
            std::fs::write(&p, f).unwrap();
        }
        root
    }

    fn run(root: &Path, pattern: &str, limit: Option<u32>, hidden: bool) -> Found {
        glob(root, pattern, limit.unwrap_or(GLOB_LIMIT) as usize, hidden).unwrap()
    }

    fn sorted(mut v: Vec<String>) -> Vec<String> {
        v.sort();
        v
    }

    #[test]
    fn name_pattern_matches_anywhere_and_path_pattern_is_anchored() {
        let root = tree(&["a.rs", "src/b.rs", "src/deep/c.rs", "src/d.txt"]);
        let (star, _) = run(&root, "*.rs", None, false);
        assert_eq!(sorted(star), ["a.rs", "src/b.rs", "src/deep/c.rs"]);
        let (dstar, _) = run(&root, "**/*.rs", None, false);
        assert_eq!(sorted(dstar), ["a.rs", "src/b.rs", "src/deep/c.rs"]);
        let (anchored, _) = run(&root, "src/*.rs", None, false);
        assert_eq!(anchored, ["src/b.rs"]);
        let (deep, _) = run(&root, "src/**/*.rs", None, false);
        assert_eq!(sorted(deep), ["src/b.rs", "src/deep/c.rs"]);
    }

    #[test]
    fn gitignore_is_respected() {
        let root = tree(&["keep.rs", "target/gen.rs", ".gitignore"]);
        std::fs::write(root.join(".gitignore"), "target/\n").unwrap();
        let (files, _) = run(&root, "*.rs", None, false);
        assert_eq!(files, ["keep.rs"]);
    }

    #[test]
    fn hidden_excluded_unless_asked() {
        let root = tree(&["shown.rs", ".hidden.rs", ".dir/inner.rs"]);
        let (files, _) = run(&root, "*.rs", None, false);
        assert_eq!(files, ["shown.rs"]);
        let (files, _) = run(&root, "*.rs", None, true);
        assert_eq!(sorted(files), [".dir/inner.rs", ".hidden.rs", "shown.rs"]);
    }

    #[test]
    fn limit_truncates_with_header() {
        let root = tree(&["a.rs", "b.rs", "c.rs", "d.rs"]);
        let (files, truncated) = run(&root, "*.rs", Some(2), false);
        assert_eq!(files.len(), 2);
        assert!(truncated);
        let out = render(files, truncated, 2, "*.rs", "r");
        assert!(
            out.content.starts_with(
                "[glob: showing 2 of 2+ matches; raise limit or narrow the pattern]\n"
            ),
            "{}",
            out.content
        );
        assert_eq!(out.structured.unwrap()["truncated"], true);
        let (files, truncated) = run(&root, "*.rs", Some(4), false);
        assert_eq!((files.len(), truncated), (4, false));
    }

    #[test]
    fn newest_first_then_path() {
        let root = tree(&["old.rs", "new.rs", "tie_b.rs", "tie_a.rs"]);
        let t0 = SystemTime::UNIX_EPOCH + Duration::from_secs(1_000_000);
        let set = |f: &str, t: SystemTime| {
            std::fs::File::options()
                .write(true)
                .open(root.join(f))
                .unwrap()
                .set_modified(t)
                .unwrap()
        };
        set("old.rs", t0);
        set("tie_b.rs", t0 + Duration::from_secs(10));
        set("tie_a.rs", t0 + Duration::from_secs(10));
        set("new.rs", t0 + Duration::from_secs(20));
        let (files, _) = run(&root, "*.rs", None, false);
        assert_eq!(files, ["new.rs", "tie_a.rs", "tie_b.rs", "old.rs"]);
    }

    #[tokio::test]
    async fn no_match_is_a_plain_message_not_an_error() {
        let root = tree(&["a.txt"]);
        let a = GlobArgs {
            pattern: "*.rs".into(),
            path: Some(root.clone()),
            limit: None,
            hidden: false,
        };
        let cx = ToolCx {
            session: crate::SessionId::new(),
            call_id: String::new(),
            quiet: false,
            resources: crate::SessionResources::durable_arc(&std::env::temp_dir()),
        };
        let out = Tool::call(&Glob, a, cx).await.unwrap();
        assert!(!out.is_error);
        assert_eq!(
            out.content,
            format!("no files match *.rs under {}", root.display())
        );
        assert_eq!(
            out.structured.unwrap(),
            serde_json::json!({ "files": [], "truncated": false })
        );
    }

    #[test]
    fn non_directory_path_errors() {
        let root = tree(&["a.rs"]);
        let err = glob(&root.join("a.rs"), "*.rs", 10, false)
            .unwrap_err()
            .to_string();
        assert!(err.contains("not a directory"), "{err}");
        let err = glob(&root.join("missing"), "*.rs", 10, false)
            .unwrap_err()
            .to_string();
        assert!(err.contains("not a directory"), "{err}");
    }
}
