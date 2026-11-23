//! `glob`: find files by name pattern (12 §10). Same walker and conventions
//! as `grep` (gitignore respected, hidden skipped, no symlink following);
//! results are ordered newest-modified first so the file the user just
//! touched is on top.

use std::path::{Path, PathBuf};
use std::sync::Mutex;
use std::sync::atomic::{AtomicUsize, Ordering::Relaxed};
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
    /// Directory to search; default: the working directory.
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
        "Find files by glob pattern: `*` matches within one path segment, `**` crosses directories; a pattern without `/` matches file names anywhere under `path` (e.g. `*.rs`), one with `/` matches paths relative to `path` (e.g. `src/**/*.rs`). Respects .gitignore and skips hidden files unless `hidden` is true; directories and symlinks are not listed. Returns paths relative to `path` (default: cwd), newest-modified first, up to `limit` (default 500). When an ancestor .gitignore would hide `path` itself, the walk ignores ancestor rules and says so in a header line.".into()
    }

    async fn call(&self, a: GlobArgs, cx: ToolCx) -> anyhow::Result<ToolOutput> {
        let root = match a.path {
            Some(p) => crate::seat::resolve(cx.session, &p),
            None => crate::seat::cwd(cx.session)?,
        };
        crate::seat::refuse_cell_record(&cx.resources.root, &root)?;
        let limit = a.limit.unwrap_or(GLOB_LIMIT).max(1) as usize;
        let shown = root.display().to_string();
        let pattern = a.pattern;
        let ((files, truncated), stats) = tokio::task::spawn_blocking({
            let pattern = pattern.clone();
            move || glob(&root, &pattern, limit, a.hidden)
        })
        .await??;
        Ok(render(
            files,
            truncated,
            stats.ancestor_ignore_bypassed,
            limit,
            &pattern,
            &shown,
        ))
    }
}

/// `glob` result: relative paths, and whether the cap cut the walk short.
type Found = (Vec<String>, bool);

/// What the walk did, beyond the matches.
#[derive(Debug, Default)]
struct WalkStats {
    /// Entries the walker yielded below the root (pruned ones are not yielded);
    /// only the pruning test reads it.
    #[cfg_attr(not(test), allow(dead_code))]
    visited: usize,
    /// The first walk yielded nothing because an ancestor .gitignore hides the
    /// root; the result comes from a retry without ancestor ignore rules.
    ancestor_ignore_bypassed: bool,
}

/// Worker threads for the parallel walkers (`glob`, `grep`).
pub(crate) fn walk_threads() -> usize {
    std::thread::available_parallelism()
        .map_or(4, |c| c.get())
        .min(8)
}

/// Retry rule shared by `glob` and `grep`: the walk yielded nothing below
/// `root` although the directory is not empty, so an ancestor .gitignore
/// (e.g. a `*` in a dotfiles home repo) must be hiding the root itself.
pub(crate) fn hidden_by_ancestor(root: &Path, visited: usize) -> bool {
    visited == 0 && std::fs::read_dir(root).is_ok_and(|mut rd| rd.next().is_some())
}

/// Literal leading directory of an anchored pattern (`src/**/*.rs` → `src`,
/// `crates/tools/src/*.rs` → `crates/tools/src`); `None` when the pattern has
/// no `/` or its first component already holds a wildcard.
fn anchored_prefix(pattern: &str) -> Option<PathBuf> {
    if !pattern.contains('/') {
        return None;
    }
    let lit: PathBuf = pattern
        .split('/')
        .take_while(|c| !c.contains(['*', '?', '[', '{']))
        .collect();
    (!lit.as_os_str().is_empty()).then_some(lit)
}

/// Walk `root` (gitignore respected, hidden files skipped unless `hidden`,
/// symlinks not followed) and collect files matching `pattern`, relative to
/// `root`, newest modification time first (ties by path). Stops after
/// `limit + 1` matches; the flag says whether that happened. The message of
/// `Err` is what the model reads.
fn glob(
    root: &Path,
    pattern: &str,
    limit: usize,
    hidden: bool,
) -> anyhow::Result<(Found, WalkStats)> {
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
    let prefix = anchored_prefix(pattern);
    let walk = |parents: bool| {
        let mut walk = ignore::WalkBuilder::new(root);
        walk.hidden(!hidden)
            .parents(parents)
            .threads(walk_threads());
        if let Some(prefix) = &prefix {
            // Never enter a directory the anchored pattern cannot reach:
            // only ancestors of the literal prefix and directories under it.
            let (root, prefix) = (root.to_owned(), prefix.clone());
            walk.filter_entry(move |e| {
                if e.depth() == 0 || !e.file_type().is_some_and(|t| t.is_dir()) {
                    return true;
                }
                let rel = e.path().strip_prefix(&root).unwrap_or(e.path());
                rel.starts_with(&prefix) || prefix.starts_with(rel)
            });
        }
        let found: Mutex<Vec<(SystemTime, String)>> = Mutex::new(Vec::new());
        let hits = AtomicUsize::new(0);
        let visited = AtomicUsize::new(0);
        walk.build_parallel().run(|| {
            Box::new(|entry| {
                use ignore::WalkState::{Continue, Quit};
                let Ok(entry) = entry else { return Continue };
                if entry.depth() > 0 {
                    visited.fetch_add(1, Relaxed);
                }
                if hits.load(Relaxed) > limit {
                    return Quit;
                }
                if !entry.file_type().is_some_and(|t| t.is_file()) {
                    return Continue;
                }
                let path = entry.path();
                let rel = path.strip_prefix(root).unwrap_or(path);
                let hit = if by_name {
                    path.file_name().is_some_and(|n| matcher.is_match(n))
                } else {
                    matcher.is_match(rel)
                };
                if !hit {
                    return Continue;
                }
                // Keep at most `limit + 1` matches; the extra one proves truncation.
                if hits.fetch_add(1, Relaxed) > limit {
                    return Quit;
                }
                let mtime = entry
                    .metadata()
                    .and_then(|m| m.modified().map_err(Into::into))
                    .unwrap_or(SystemTime::UNIX_EPOCH);
                found
                    .lock()
                    .unwrap()
                    .push((mtime, rel.display().to_string()));
                Continue
            })
        });
        (found.into_inner().unwrap(), visited.into_inner())
    };
    let (mut found, visited) = walk(true);
    let mut stats = WalkStats {
        visited,
        ancestor_ignore_bypassed: false,
    };
    if hidden_by_ancestor(root, visited) {
        let (retry, visited) = walk(false);
        if visited > 0 {
            found = retry;
            stats = WalkStats {
                visited,
                ancestor_ignore_bypassed: true,
            };
        }
    }
    let truncated = found.len() > limit;
    found.truncate(limit);
    found.sort_by(|(ta, pa), (tb, pb)| tb.cmp(ta).then_with(|| pa.cmp(pb)));
    Ok((
        (found.into_iter().map(|(_, p)| p).collect(), truncated),
        stats,
    ))
}

/// Render the tool result: one path per line, a cap header when truncated,
/// an ancestor-ignore header when the retry walk was used, a plain no-match
/// line, `structured = {"files", "truncated"[, "ancestor_ignore_bypassed"]}`.
fn render(
    files: Vec<String>,
    truncated: bool,
    bypassed: bool,
    limit: usize,
    pattern: &str,
    root: &str,
) -> ToolOutput {
    let mut out = ToolOutput::ok(String::new());
    let body = if files.is_empty() {
        format!("no files match {pattern} under {root}")
    } else if truncated {
        format!(
            "[glob: showing {limit} of {limit}+ matches; raise limit or narrow the pattern]\n{}",
            files.join("\n")
        )
    } else {
        files.join("\n")
    };
    let mut structured = serde_json::json!({ "files": files, "truncated": truncated });
    out.content = if bypassed {
        structured["ancestor_ignore_bypassed"] = true.into();
        format!(
            "[glob: an ancestor .gitignore hides {root}; walked without ancestor ignore rules]\n{body}"
        )
    } else {
        body
    };
    out.structured = Some(structured);
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::Duration;

    /// Temp tree with a `.git` marker so `.gitignore` is honoured (the walker
    /// only reads it inside a repository).
    fn tree(files: &[&str]) -> (tempfile::TempDir, PathBuf) {
        let guard = crate::test_dir("glob");
        let root = guard.path().to_path_buf();
        std::fs::create_dir_all(root.join(".git")).unwrap();
        for f in files {
            let p = root.join(f);
            std::fs::create_dir_all(p.parent().unwrap()).unwrap();
            std::fs::write(&p, f).unwrap();
        }
        (guard, root)
    }

    fn run(root: &Path, pattern: &str, limit: Option<u32>, hidden: bool) -> Found {
        glob(root, pattern, limit.unwrap_or(GLOB_LIMIT) as usize, hidden)
            .unwrap()
            .0
    }

    fn sorted(mut v: Vec<String>) -> Vec<String> {
        v.sort();
        v
    }

    #[test]
    fn name_pattern_matches_anywhere_and_path_pattern_is_anchored() {
        let (_g, root) = tree(&["a.rs", "src/b.rs", "src/deep/c.rs", "src/d.txt"]);
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
        let (_g, root) = tree(&["keep.rs", "target/gen.rs", ".gitignore"]);
        std::fs::write(root.join(".gitignore"), "target/\n").unwrap();
        let (files, _) = run(&root, "*.rs", None, false);
        assert_eq!(files, ["keep.rs"]);
    }

    #[test]
    fn hidden_excluded_unless_asked() {
        let (_g, root) = tree(&["shown.rs", ".hidden.rs", ".dir/inner.rs"]);
        let (files, _) = run(&root, "*.rs", None, false);
        assert_eq!(files, ["shown.rs"]);
        let (files, _) = run(&root, "*.rs", None, true);
        assert_eq!(sorted(files), [".dir/inner.rs", ".hidden.rs", "shown.rs"]);
    }

    #[test]
    fn limit_truncates_with_header() {
        let (_g, root) = tree(&["a.rs", "b.rs", "c.rs", "d.rs"]);
        let (files, truncated) = run(&root, "*.rs", Some(2), false);
        assert_eq!(files.len(), 2);
        assert!(truncated);
        let out = render(files, truncated, false, 2, "*.rs", "r");
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
        let (_g, root) = tree(&["old.rs", "new.rs", "tie_b.rs", "tie_a.rs"]);
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
        let (_g, root) = tree(&["a.txt"]);
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
            cancel: tokio_util::sync::CancellationToken::new(),
            promote_after: None,
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
        let (_g, root) = tree(&["a.rs"]);
        let err = glob(&root.join("a.rs"), "*.rs", 10, false)
            .unwrap_err()
            .to_string();
        assert!(err.contains("not a directory"), "{err}");
        let err = glob(&root.join("missing"), "*.rs", 10, false)
            .unwrap_err()
            .to_string();
        assert!(err.contains("not a directory"), "{err}");
    }

    #[test]
    fn cap_holds_under_parallel_walk() {
        let files: Vec<String> = (0..300).map(|i| format!("d{}/f{i}.rs", i % 7)).collect();
        let refs: Vec<&str> = files.iter().map(String::as_str).collect();
        let (_g, root) = tree(&refs);
        let (found, truncated) = run(&root, "*.rs", Some(50), false);
        assert_eq!((found.len(), truncated), (50, true));
        let out = render(found, truncated, false, 50, "*.rs", "r");
        assert!(out.content.starts_with("[glob: showing 50 of 50+ matches;"));
        let (found, truncated) = run(&root, "*.rs", Some(300), false);
        assert_eq!((found.len(), truncated), (300, false));
    }

    #[test]
    fn anchored_pattern_prunes_but_matches_the_same() {
        let (_g, root) = tree(&["src/a.rs", "other/b.rs", "src/deep/c.rs"]);
        let ((anchored, _), pruned) = glob(&root, "src/**/*.rs", 500, false).unwrap();
        assert_eq!(sorted(anchored), ["src/a.rs", "src/deep/c.rs"]);
        let ((all, _), full) = glob(&root, "**/*.rs", 500, false).unwrap();
        assert_eq!(all.len(), 3);
        assert!(
            pruned.visited < full.visited,
            "pruned {} vs full {}",
            pruned.visited,
            full.visited
        );
        assert_eq!(
            anchored_prefix("crates/tools/src/*.rs").unwrap(),
            Path::new("crates/tools/src")
        );
        assert_eq!(anchored_prefix("**/x.rs"), None);
        assert_eq!(anchored_prefix("*.rs"), None);
        assert_eq!(anchored_prefix("{src,lib}/*.rs"), None);
    }

    #[tokio::test]
    async fn ancestor_gitignore_hiding_the_root_is_bypassed_once() {
        let (_g, outer) = tree(&["inner/a.rs", "inner/b.txt"]);
        std::fs::write(outer.join(".gitignore"), "*\n").unwrap();
        let inner = outer.join("inner");
        let ((files, _), stats) = glob(&inner, "*.rs", 500, false).unwrap();
        assert_eq!(files, ["a.rs"]);
        assert!(stats.ancestor_ignore_bypassed);
        let out = render(files, false, true, 500, "*.rs", "inner");
        assert_eq!(
            out.content,
            "[glob: an ancestor .gitignore hides inner; walked without ancestor ignore rules]\na.rs"
        );
        assert_eq!(out.structured.unwrap()["ancestor_ignore_bypassed"], true);
        // The root's own .gitignore is not an ancestor rule: plain no-match.
        let a = GlobArgs {
            pattern: "*.rs".into(),
            path: Some(outer.clone()),
            limit: None,
            hidden: false,
        };
        let cx = ToolCx {
            session: crate::SessionId::new(),
            call_id: String::new(),
            quiet: false,
            resources: crate::SessionResources::durable_arc(&std::env::temp_dir()),
            cancel: tokio_util::sync::CancellationToken::new(),
            promote_after: None,
        };
        let out = Tool::call(&Glob, a, cx).await.unwrap();
        assert_eq!(
            out.content,
            format!("no files match *.rs under {}", outer.display())
        );
        assert_eq!(
            out.structured.unwrap(),
            serde_json::json!({ "files": [], "truncated": false })
        );
    }

    /// Benchmark harness (D7): `GLOB_BENCH_ROOT=<dir> [GLOB_BENCH_PATTERN=*.zzz_none]
    /// cargo test -p tools --lib -- glob::tests::bench_walk --ignored --nocapture`.
    #[test]
    #[ignore]
    fn bench_walk() {
        let root = PathBuf::from(std::env::var("GLOB_BENCH_ROOT").expect("GLOB_BENCH_ROOT"));
        let pattern = std::env::var("GLOB_BENCH_PATTERN").unwrap_or_else(|_| "*.zzz_none".into());
        for i in 1..=3 {
            let t = std::time::Instant::now();
            let ((files, truncated), stats) =
                glob(&root, &pattern, GLOB_LIMIT as usize, false).unwrap();
            println!(
                "bench_walk run {i}: {} ms, {} files found (truncated={truncated}, visited={}, ancestor_bypass={}) pattern={pattern} root={}",
                t.elapsed().as_millis(),
                files.len(),
                stats.visited,
                stats.ancestor_ignore_bypassed,
                root.display()
            );
        }
    }
}
