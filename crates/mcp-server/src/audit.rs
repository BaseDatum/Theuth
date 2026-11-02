//! 158 §12.5 (W-j): `<data_dir>/mcp/server-audit.jsonl`, one [`AuditLine`] per tool call, args
//! hashed, rotated to `server-audit.jsonl.1` at 50 MB. The HTTP server and `t mcp` append to the
//! same file (`O_APPEND`, one `write` per line).

use std::collections::BTreeMap;
use std::io::Write;
use std::path::{Path, PathBuf};

use serde_json::Value;
use sha2::{Digest, Sha256};

use crate::AuditLine;

/// Rotate when the file is past this.
pub const ROTATE_BYTES: u64 = 50 * 1024 * 1024;

/// The audit file.
#[derive(Clone, Debug)]
pub struct Audit {
    path: PathBuf,
}

impl Audit {
    /// `<data_dir>/mcp/server-audit.jsonl`.
    pub fn new(data_dir: &Path) -> Self {
        Self::at(data_dir.join("mcp").join("server-audit.jsonl"))
    }

    /// At an explicit path (tests).
    pub fn at(path: PathBuf) -> Self {
        Self { path }
    }

    /// Append one line; a failed write is logged, never a tool error.
    pub fn append(&self, line: &AuditLine) {
        if let Err(e) = self.try_append(line) {
            tracing::warn!(path = %self.path.display(), error = %e, "mcp audit: write failed");
        }
    }

    fn try_append(&self, line: &AuditLine) -> std::io::Result<()> {
        if let Some(dir) = self.path.parent() {
            std::fs::create_dir_all(dir)?;
        }
        if std::fs::metadata(&self.path).is_ok_and(|m| m.len() > ROTATE_BYTES) {
            std::fs::rename(&self.path, self.path.with_extension("jsonl.1"))?;
        }
        let mut text = serde_json::to_string(line)?;
        text.push('\n');
        std::fs::OpenOptions::new()
            .create(true)
            .append(true)
            .open(&self.path)?
            .write_all(text.as_bytes())
    }

    /// The last `limit` lines, newest first; a missing file is empty, a torn line is skipped.
    pub fn tail(&self, limit: usize) -> anyhow::Result<Vec<AuditLine>> {
        let text = match std::fs::read_to_string(&self.path) {
            Ok(t) => t,
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(Vec::new()),
            Err(e) => return Err(e.into()),
        };
        Ok(text
            .lines()
            .rev()
            .filter_map(|l| serde_json::from_str(l).ok())
            .take(limit)
            .collect())
    }
}

/// SHA-256 hex of the arguments with object keys sorted at every level, so the same call hashes
/// the same whatever order the client sent its keys in.
pub fn args_sha256(args: &Value) -> String {
    let canonical = serde_json::to_vec(&sorted(args)).unwrap_or_default();
    hex::encode(Sha256::digest(canonical))
}

fn sorted(v: &Value) -> Value {
    match v {
        Value::Object(m) => {
            let b: BTreeMap<&String, Value> = m.iter().map(|(k, v)| (k, sorted(v))).collect();
            serde_json::to_value(b).unwrap_or(Value::Null)
        }
        Value::Array(a) => Value::Array(a.iter().map(sorted).collect()),
        other => other.clone(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn line(tool: &str) -> AuditLine {
        AuditLine {
            at: "2026-10-10T12:00:00Z".into(),
            transport: "http".into(),
            client: "test".into(),
            tool: tool.into(),
            args_sha256: args_sha256(&json!({"b": 1, "a": [{"y": 2, "x": 1}]})),
            ok: true,
            ms: 3,
        }
    }

    /// Lines append as JSON objects with the seven fields, tail is newest first, and the hash
    /// does not depend on key order.
    #[test]
    fn appends_and_tails() {
        let dir = tempfile::tempdir().unwrap();
        let audit = Audit::at(dir.path().join("mcp/server-audit.jsonl"));
        audit.append(&line("meeting_list"));
        audit.append(&line("task_get"));
        let raw = std::fs::read_to_string(dir.path().join("mcp/server-audit.jsonl")).unwrap();
        let first: Value = serde_json::from_str(raw.lines().next().unwrap()).unwrap();
        let mut keys: Vec<_> = first.as_object().unwrap().keys().cloned().collect();
        keys.sort();
        assert_eq!(
            keys,
            [
                "args_sha256",
                "at",
                "client",
                "ms",
                "ok",
                "tool",
                "transport"
            ]
        );
        assert_eq!(first["args_sha256"].as_str().unwrap().len(), 64);
        let tail = audit.tail(1).unwrap();
        assert_eq!(tail.len(), 1);
        assert_eq!(tail[0].tool, "task_get");
        assert_eq!(
            args_sha256(&json!({"a": [{"x": 1, "y": 2}], "b": 1})),
            first["args_sha256"]
        );
    }

    /// Past 50 MB the file moves aside and a fresh one starts.
    #[test]
    fn rotates_past_the_cap() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("server-audit.jsonl");
        let f = std::fs::File::create(&path).unwrap();
        f.set_len(ROTATE_BYTES + 1).unwrap();
        let audit = Audit::at(path.clone());
        audit.append(&line("x"));
        assert!(dir.path().join("server-audit.jsonl.1").exists());
        assert_eq!(audit.tail(10).unwrap().len(), 1);
    }
}
