//! `read_file`, `write_file`, `list_dir` over `tokio::fs`.
//!
//! Paths are resolved against the process cwd; `~` is not expanded.

use std::path::{Path, PathBuf};

use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

use crate::{Risk, Tool, ToolCx, ToolOutput};

/// Output cap for `read_file`.
const MAX_BYTES: usize = 64 * 1024;

/// Arguments for [`ReadFile`].
#[derive(Debug, Serialize, Deserialize, JsonSchema)]
pub struct ReadFileArgs {
    /// File path, absolute or relative to the process cwd (no `~`).
    pub path: PathBuf,
    /// 1-based first line to return.
    #[serde(default)]
    pub offset: Option<usize>,
    /// Maximum number of lines to return.
    #[serde(default)]
    pub limit: Option<usize>,
}

/// Read a text file.
pub struct ReadFile;

#[async_trait::async_trait]
impl Tool for ReadFile {
    const NAME: &'static str = "read_file";
    const RISK: Risk = Risk::ReadOnly;
    type Args = ReadFileArgs;

    fn description(&self) -> String {
        "Read a text file. Optional 1-based `offset` line and `limit` line count. Output is capped at 64 KiB. Paths are relative to the process cwd; `~` is not expanded.".into()
    }

    async fn call(&self, a: ReadFileArgs, _: ToolCx) -> anyhow::Result<ToolOutput> {
        let bytes = tokio::fs::read(&a.path)
            .await
            .map_err(|e| anyhow::anyhow!("{}: {e}", a.path.display()))?;
        let text = String::from_utf8_lossy(&bytes);
        let mut out: String = match (a.offset, a.limit) {
            (None, None) => text.into_owned(),
            (offset, limit) => text
                .lines()
                .skip(offset.unwrap_or(1).saturating_sub(1))
                .take(limit.unwrap_or(usize::MAX))
                .collect::<Vec<_>>()
                .join("\n"),
        };
        if out.len() > MAX_BYTES {
            let mut cut = MAX_BYTES;
            while !out.is_char_boundary(cut) {
                cut -= 1;
            }
            out.truncate(cut);
            out.push_str("\n[truncated at 64 KiB; use offset/limit to read more]");
        }
        Ok(ToolOutput::ok(out))
    }
}

/// Arguments for [`WriteFile`].
#[derive(Debug, Serialize, Deserialize, JsonSchema)]
pub struct WriteFileArgs {
    /// File path, absolute or relative to the process cwd (no `~`).
    pub path: PathBuf,
    /// Full new content; the file is replaced.
    pub content: String,
}

/// Write (create or replace) a text file, creating parent directories.
pub struct WriteFile;

#[async_trait::async_trait]
impl Tool for WriteFile {
    const NAME: &'static str = "write_file";
    const RISK: Risk = Risk::Mutating;
    type Args = WriteFileArgs;

    fn description(&self) -> String {
        "Create or overwrite a file with the given content; parent directories are created. Paths are relative to the process cwd; `~` is not expanded.".into()
    }

    async fn call(&self, a: WriteFileArgs, _: ToolCx) -> anyhow::Result<ToolOutput> {
        if let Some(parent) = a.path.parent().filter(|p| !p.as_os_str().is_empty()) {
            tokio::fs::create_dir_all(parent)
                .await
                .map_err(|e| anyhow::anyhow!("{}: {e}", parent.display()))?;
        }
        tokio::fs::write(&a.path, &a.content)
            .await
            .map_err(|e| anyhow::anyhow!("{}: {e}", a.path.display()))?;
        Ok(ToolOutput::ok(format!(
            "wrote {} bytes to {}",
            a.content.len(),
            a.path.display()
        )))
    }
}

/// Arguments for [`ListDir`].
#[derive(Debug, Serialize, Deserialize, JsonSchema)]
pub struct ListDirArgs {
    /// Directory path, absolute or relative to the process cwd (no `~`).
    pub path: PathBuf,
}

/// List a directory, one entry per line, directories suffixed with `/`.
pub struct ListDir;

#[async_trait::async_trait]
impl Tool for ListDir {
    const NAME: &'static str = "list_dir";
    const RISK: Risk = Risk::ReadOnly;
    type Args = ListDirArgs;

    fn description(&self) -> String {
        "List a directory: one entry per line, sorted, directories end with `/`. Paths are relative to the process cwd; `~` is not expanded.".into()
    }

    async fn call(&self, a: ListDirArgs, _: ToolCx) -> anyhow::Result<ToolOutput> {
        Ok(ToolOutput::ok(list_dir(&a.path).await?))
    }
}

async fn list_dir(path: &Path) -> anyhow::Result<String> {
    let mut rd = tokio::fs::read_dir(path)
        .await
        .map_err(|e| anyhow::anyhow!("{}: {e}", path.display()))?;
    let mut entries = Vec::new();
    while let Some(entry) = rd.next_entry().await? {
        let mut name = entry.file_name().to_string_lossy().into_owned();
        if entry.file_type().await.map(|t| t.is_dir()).unwrap_or(false) {
            name.push('/');
        }
        entries.push(name);
    }
    entries.sort();
    Ok(entries.join("\n"))
}
