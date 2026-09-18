//! The `.app.json` sidecar stored in `Artifact::source` (46 §3.2): what the
//! source was validated against and what it binds to. Content-addressed by
//! its own hash (the artifact store does that); opaque bytes to `artifact`.
//! Reading it back is bounded and never executes anything it names.

use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};
use tools::{McpEntry, ToolInspection, ToolSource};

use crate::helper::{CheckResult, StatementInfo};
use crate::{AppBinding, AppDiagnostic};

/// Largest `app` source (46 §7).
pub const MAX_SOURCE: usize = 128 * 1024;
/// Largest metadata sidecar (46 §7).
pub const MAX_META: usize = 256 * 1024;
/// Largest one integration entry-point snapshot.
pub const MAX_SNAPSHOT: usize = 32 * 1024;
/// Snapshot bytes per app, all servers together.
pub const MAX_SNAPSHOTS: usize = 96 * 1024;
/// Largest tool arguments object an app may send (serialized).
pub const MAX_ARGS: usize = 64 * 1024;
/// Largest serialized tool response forwarded to a client (46 §7).
pub const MAX_RESPONSE: usize = 1024 * 1024;

/// Directories never snapshotted: build outputs and dependency trees.
const SKIPPED_DIRS: [&str; 6] = ["node_modules", "dist", "build", "target", ".git", ".venv"];

/// The sidecar.
#[derive(Clone, Debug, Default, Serialize, Deserialize)]
pub struct AppMeta {
    /// Library compatibility hash the helper validated against.
    pub library_hash: String,
    /// Every statement.
    #[serde(default)]
    pub statements: Vec<StatementInfo>,
    /// Query/Mutation bindings with pinned identities.
    #[serde(default)]
    pub bindings: Vec<AppBinding>,
    /// Publish-time diagnostics (empty: a committed app was clean).
    #[serde(default)]
    pub diagnostics: Vec<AppDiagnostic>,
    /// Declared `$state` defaults.
    #[serde(default)]
    pub state: serde_json::Value,
    /// Unreachable statements at publish.
    #[serde(default)]
    pub orphaned: Vec<String>,
    /// The bound tools as inspected at publish: spec, scrubbed source, identity.
    #[serde(default)]
    pub tools: Vec<ToolInspection>,
    /// Entry-point text of model-authored MCP integrations, for inspection.
    #[serde(default)]
    pub snapshots: Vec<SourceSnapshot>,
}

/// One integration's entry point as text (46 §3.2). Display only; never loaded.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct SourceSnapshot {
    /// MCP server name.
    pub server: String,
    /// Clone under `mcp/repos/`.
    pub repo: String,
    /// Entry file, relative to the clone, when found.
    pub entry: Option<String>,
    /// Its text, secrets redacted, when found and within [`MAX_SNAPSHOT`].
    pub source: Option<String>,
    /// Why `source` is absent.
    pub note: Option<String>,
}

impl AppMeta {
    /// Metadata for a clean check against `tools`, keeping only the
    /// inspections the bindings use.
    pub fn new(
        check: &CheckResult,
        tools: &[ToolInspection],
        snapshots: Vec<SourceSnapshot>,
    ) -> Self {
        let bound: Vec<ToolInspection> = tools
            .iter()
            .filter(|t| check.bindings.iter().any(|b| b.tool == t.spec.name))
            .cloned()
            .collect();
        Self {
            library_hash: check.library_hash.clone(),
            statements: check.statements.clone(),
            bindings: check.bindings.clone(),
            diagnostics: check.diagnostics.clone(),
            state: check.state.clone(),
            orphaned: check.orphaned.clone(),
            tools: bound,
            snapshots,
        }
    }

    /// Serialize within [`MAX_META`]: snapshots are dropped first (with a
    /// note), then it is an error.
    pub fn to_bytes(&mut self) -> Result<Vec<u8>, String> {
        let mut bytes = serde_json::to_vec(&*self).map_err(|e| e.to_string())?;
        if bytes.len() > MAX_META && !self.snapshots.is_empty() {
            for s in &mut self.snapshots {
                s.source = None;
                s.note = Some("snapshot omitted: metadata would exceed 256 KiB".into());
            }
            bytes = serde_json::to_vec(&*self).map_err(|e| e.to_string())?;
        }
        if bytes.len() > MAX_META {
            return Err(format!(
                "app metadata is {} bytes; the limit is {MAX_META}",
                bytes.len()
            ));
        }
        Ok(bytes)
    }
}

/// Read a file of at most `max` bytes; larger or unreadable is `Err`.
pub async fn read_bounded(path: &Path, max: usize) -> Result<Vec<u8>, String> {
    let path = path.to_path_buf();
    tokio::task::spawn_blocking(move || read_bounded_sync(&path, max))
        .await
        .map_err(|e| e.to_string())?
}

/// [`read_bounded`], blocking. The open handle is what is checked and read,
/// so a file that grows or is swapped after a size check cannot slip past:
/// it must be a regular file and at most `max` bytes as actually read.
pub fn read_bounded_sync(path: &Path, max: usize) -> Result<Vec<u8>, String> {
    use std::io::Read;
    let here = || path.display();
    let file = std::fs::File::open(path).map_err(|e| format!("{}: {e}", here()))?;
    let meta = file.metadata().map_err(|e| format!("{}: {e}", here()))?;
    if !meta.is_file() {
        return Err(format!("{}: not a regular file", here()));
    }
    let mut buf = Vec::with_capacity(meta.len().min(max as u64 + 1) as usize);
    file.take(max as u64 + 1)
        .read_to_end(&mut buf)
        .map_err(|e| format!("{}: {e}", here()))?;
    if buf.len() > max {
        return Err(format!("{}: larger than {max} bytes", here()));
    }
    Ok(buf)
}

/// Parse a sidecar read with [`read_bounded`].
pub fn parse(bytes: &[u8]) -> Result<AppMeta, String> {
    serde_json::from_slice(bytes).map_err(|e| format!("app metadata: {e}"))
}

/// Entry-point snapshots for every distinct model-authored MCP server among
/// `inspections` (46 §3.2, §5): `Stdio` entries with a `repo` under
/// `<data_dir>/mcp/repos/`. The entry point is the first of `args` then
/// `command` that names a regular file inside that clone (never a symlink
/// out of it, never under [`SKIPPED_DIRS`]), read up to [`MAX_SNAPSHOT`] and
/// redacted. Nothing is executed; a missing entry is reported, not guessed.
pub fn snapshots(data_dir: &Path, inspections: &[ToolInspection]) -> Vec<SourceSnapshot> {
    let mut out: Vec<SourceSnapshot> = Vec::new();
    let mut total = 0usize;
    for t in inspections {
        let ToolSource::Mcp {
            server,
            entry:
                McpEntry::Stdio {
                    command,
                    args,
                    cwd,
                    repo: Some(repo),
                    ..
                },
        } = &t.source
        else {
            continue;
        };
        if out.iter().any(|s| &s.server == server) {
            continue;
        }
        let mut snap = SourceSnapshot {
            server: server.clone(),
            repo: repo.clone(),
            entry: None,
            source: None,
            note: None,
        };
        if repo.is_empty() || repo.contains(['/', '\\']) || repo == "." || repo == ".." {
            snap.note = Some("repo name is not a directory name".into());
            out.push(snap);
            continue;
        }
        let root = data_dir.join("mcp").join("repos").join(repo);
        let Ok(root) = root.canonicalize() else {
            snap.note = Some(format!("mcp/repos/{repo} is not present on this harness"));
            out.push(snap);
            continue;
        };
        let base = cwd
            .as_ref()
            .and_then(|c| c.canonicalize().ok())
            .filter(|c| c.starts_with(&root))
            .unwrap_or_else(|| root.clone());
        match args
            .iter()
            .chain(std::iter::once(command))
            .find_map(|c| entry_file(&root, &base, c))
        {
            None => {
                snap.note = Some(format!(
                    "entry point not found among the entry's arguments; inspect mcp/repos/{repo}"
                ));
            }
            Some((rel, path)) => {
                snap.entry = Some(rel);
                let budget = MAX_SNAPSHOT.min(MAX_SNAPSHOTS.saturating_sub(total));
                match read_bounded_sync(&path, budget) {
                    Ok(bytes) => {
                        total += bytes.len();
                        snap.source = Some(
                            tools::secrets::redact(&String::from_utf8_lossy(&bytes)).into_owned(),
                        );
                    }
                    Err(e) => snap.note = Some(format!("entry not snapshotted: {e}")),
                }
            }
        }
        out.push(snap);
    }
    out
}

/// `candidate` as a regular file inside `root` (relative to `base`), with
/// its root-relative display path.
fn entry_file(root: &Path, base: &Path, candidate: &str) -> Option<(String, PathBuf)> {
    if candidate.is_empty() || candidate.starts_with('-') {
        return None;
    }
    let raw = Path::new(candidate);
    let joined = if raw.is_absolute() {
        raw.to_path_buf()
    } else {
        base.join(raw)
    };
    let path = joined.canonicalize().ok()?;
    if !path.starts_with(root) || !path.is_file() {
        return None;
    }
    let rel = path.strip_prefix(root).ok()?;
    if rel
        .components()
        .any(|c| SKIPPED_DIRS.contains(&c.as_os_str().to_str().unwrap_or_default()))
    {
        return None;
    }
    Some((rel.to_string_lossy().into_owned(), path))
}

#[cfg(test)]
mod tests {
    use super::*;
    use tools::{PolicyAction, Risk, ToolSpec};

    fn mcp(server: &str, repo: Option<&str>, command: &str, args: &[&str]) -> ToolInspection {
        ToolInspection {
            spec: ToolSpec {
                name: format!("{server}_tool"),
                description: String::new(),
                schema: serde_json::json!({"type": "object"}),
                risk: Risk::ReadOnly,
                direct: false,
                returns: "str".into(),
            },
            source: ToolSource::Mcp {
                server: server.into(),
                entry: McpEntry::Stdio {
                    command: command.into(),
                    args: args.iter().map(|a| (*a).to_owned()).collect(),
                    cwd: None,
                    env: Vec::new(),
                    repo: repo.map(str::to_owned),
                },
            },
            identity: "id".into(),
            action: PolicyAction::Allow,
        }
    }

    #[test]
    fn snapshots_take_the_entry_inside_the_clone_only() {
        let dir = tempfile::tempdir().unwrap();
        let repo = dir.path().join("mcp/repos/argo");
        std::fs::create_dir_all(repo.join("node_modules/x")).unwrap();
        std::fs::write(repo.join("server.ts"), "export const token = 'plain';").unwrap();
        std::fs::write(repo.join("node_modules/x/index.js"), "dep").unwrap();
        let outside = dir.path().join("secret.txt");
        std::fs::write(&outside, "nope").unwrap();

        let snaps = snapshots(
            dir.path(),
            &[
                mcp("argo", Some("argo"), "bun", &["run", "server.ts"]),
                mcp("argo", Some("argo"), "bun", &["run", "server.ts"]), // second tool, same server
                mcp("dep", Some("argo"), "node", &["node_modules/x/index.js"]),
                mcp("leak", Some("argo"), "cat", &[outside.to_str().unwrap()]),
                mcp("gone", Some("missing"), "x", &[]),
                mcp("native", None, "x", &[]),
            ],
        );
        assert_eq!(snaps.len(), 4, "{snaps:?}");
        assert_eq!(snaps[0].entry.as_deref(), Some("server.ts"));
        assert_eq!(
            snaps[0].source.as_deref(),
            Some("export const token = 'plain';")
        );
        assert!(
            snaps[1].entry.is_none() && snaps[1].note.is_some(),
            "node_modules skipped"
        );
        assert!(snaps[2].entry.is_none(), "outside the clone is never read");
        assert!(snaps[3].note.as_deref().unwrap().contains("not present"));
    }

    #[test]
    fn metadata_drops_snapshots_before_failing_the_cap() {
        let big = SourceSnapshot {
            server: "s".into(),
            repo: "r".into(),
            entry: Some("e".into()),
            source: Some("x".repeat(MAX_META)),
            note: None,
        };
        let meta = AppMeta {
            snapshots: vec![big],
            ..AppMeta::default()
        };
        let mut meta = meta;
        let bytes = meta.to_bytes().unwrap();
        let back = parse(&bytes).unwrap();
        assert!(back.snapshots[0].source.is_none());
        assert!(
            back.snapshots[0]
                .note
                .as_deref()
                .unwrap()
                .contains("omitted")
        );

        let meta = AppMeta {
            library_hash: "h".repeat(MAX_META + 1),
            ..AppMeta::default()
        };
        let mut meta = meta;
        assert!(meta.to_bytes().is_err());
    }

    #[tokio::test]
    async fn read_bounded_refuses_large_files_without_reading_them() {
        let dir = tempfile::tempdir().unwrap();
        let p = dir.path().join("f");
        std::fs::write(&p, [0u8; 10]).unwrap();
        assert!(read_bounded(&p, 9).await.is_err());
        assert_eq!(read_bounded(&p, 10).await.unwrap().len(), 10);
    }
}
