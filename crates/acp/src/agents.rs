//! `<data_dir>/acp/agents.yaml`: one entry per runnable external harness
//! (design doc 06 §3.1). Read at every spawn and every `/acp`; no watcher
//! (06 §8.6).

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use agent_client_protocol::AcpAgentConfig;
use anyhow::Context;
use serde::{Deserialize, Serialize};

/// Directory under the data dir holding `agents.yaml`, `registry.json`, `bin/`.
pub const DIR: &str = "acp";
/// The entries file inside [`DIR`].
pub const AGENTS_FILE: &str = "agents.yaml";

/// One harness: how to start it in ACP mode. `command`/`args`/`env` are
/// exactly [`AcpAgentConfig`] (which denies unknown fields, so this is our own
/// struct, 06 §8.4).
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
pub struct Entry {
    /// Executable or command name.
    pub command: String,
    /// Arguments that put the harness into ACP mode.
    #[serde(default)]
    pub args: Vec<String>,
    /// Extra environment for the child (it inherits ours too).
    #[serde(default)]
    pub env: BTreeMap<String, String>,
    /// Shown to the model in the `spawn_subagent` description.
    #[serde(default)]
    pub description: Option<String>,
    /// Model label shown beside the harness name when the harness does not
    /// report one in its session `configOptions` (06 §8.2). Never sent to the
    /// harness: it is a display fallback the user sets by hand.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub model: Option<String>,
    /// Write theuth skills into the child's cwd for it to load (design doc
    /// 75 §4); default true.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub skills: Option<bool>,
    /// Where, relative to the child's cwd; default [`DEFAULT_SKILLS_DIR`]
    /// (`.claude/skills` for Claude Code).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub skills_dir: Option<PathBuf>,
    /// Give the child the theuth MCP shim (`load_skill`, `list_skills`,
    /// `send_message`; 75 §9) in `session/new.mcpServers`; default true,
    /// `mcp: false` opts out.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub mcp: Option<bool>,
}

impl Entry {
    /// The SDK launch configuration for this entry. `OPENCODE_PRINT_LOGS=1`
    /// is set unless the entry's `env` says otherwise: the child's stderr
    /// lands in `<data_dir>/logs/acp-<name>.log` (06 §8.4), and opencode
    /// (1.x and 2.x) prints its log there only with this variable or the
    /// removed-in-2.x `--print-logs`; other harnesses ignore it.
    #[must_use]
    pub fn config(&self) -> AcpAgentConfig {
        AcpAgentConfig::new(&self.command)
            .args(self.args.iter().cloned())
            .env("OPENCODE_PRINT_LOGS", "1")
            .envs(self.env.iter().map(|(k, v)| (k.clone(), v.clone())))
    }
}

/// [`Entry::skills_dir`] when unset: what omp, opencode, Codex and Gemini read.
pub const DEFAULT_SKILLS_DIR: &str = ".agents/skills";

/// Wire/UI shape for `/acp` and RPC.
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, schemars::JsonSchema)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS), ts(export))]
pub struct EntryInfo {
    /// Key in `agents.yaml`.
    pub name: String,
    /// Executable or command name.
    pub command: String,
    /// Arguments.
    pub args: Vec<String>,
    /// Optional description.
    pub description: Option<String>,
    /// `initialize` probe: `None` = not probed, `Some("ok" | "login required: <cmd>" | "spawn failed: <why>")`.
    pub status: Option<String>,
}

/// `<data_dir>/acp/agents.yaml`.
#[must_use]
pub fn path(data_dir: &Path) -> PathBuf {
    data_dir.join(DIR).join(AGENTS_FILE)
}

/// Every entry, by name; a missing (or empty) file is an empty map.
pub fn load(data_dir: &Path) -> anyhow::Result<BTreeMap<String, Entry>> {
    let path = path(data_dir);
    match std::fs::read_to_string(&path) {
        Ok(text) if text.trim().is_empty() => Ok(BTreeMap::new()),
        Ok(text) => {
            serde_yaml::from_str(&text).with_context(|| format!("parse {}", path.display()))
        }
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(BTreeMap::new()),
        Err(e) => Err(e).with_context(|| format!("read {}", path.display())),
    }
}

/// Write every entry; creates the directory, atomic (tmp + rename).
pub fn save(data_dir: &Path, entries: &BTreeMap<String, Entry>) -> anyhow::Result<()> {
    let path = path(data_dir);
    let dir = path.parent().context("agents.yaml has a parent")?;
    std::fs::create_dir_all(dir).with_context(|| format!("create {}", dir.display()))?;
    let tmp = path.with_extension("yaml.tmp");
    std::fs::write(&tmp, serde_yaml::to_string(entries)?)
        .and_then(|()| std::fs::rename(&tmp, &path))
        .with_context(|| format!("write {}", path.display()))
}

/// One line for the tool description: `External harnesses: claude — …; codex — …`
/// plus the note that a busy harness takes a message late (06 §8.3).
/// With none configured it says so and how the user adds one, so the model
/// points at `/acp` instead of guessing a name. An entry without a description
/// shows its command line.
#[must_use]
pub fn describe(entries: &BTreeMap<String, Entry>) -> String {
    if entries.is_empty() {
        return "External harnesses: none configured; the user adds them with /acp (ACP registry) or in <data_dir>/acp/agents.yaml. Omit `agent` until then.".to_owned();
    }
    let items: Vec<String> = entries
        .iter()
        .map(|(name, e)| match &e.description {
            Some(d) if !d.trim().is_empty() => format!("{name} — {}", d.trim()),
            _ => {
                let mut line = format!("{name} — {}", e.command);
                for a in &e.args {
                    line.push(' ');
                    line.push_str(a);
                }
                line
            }
        })
        .collect();
    format!(
        "External harnesses: {}. A harness still busy with its previous task takes a send_message late: the prompt is retried until it is accepted.",
        items.join("; ")
    )
}

/// Error text for a name that is not in `entries`: lists the configured names,
/// or says how to add one when there are none.
#[must_use]
pub fn unknown(name: &str, entries: &BTreeMap<String, Entry>) -> String {
    if entries.is_empty() {
        return format!(
            "unknown agent `{name}`; none configured - add one with /acp or in {DIR}/{AGENTS_FILE}"
        );
    }
    let names: Vec<&str> = entries.keys().map(String::as_str).collect();
    format!("unknown agent `{name}`; configured: {}", names.join(", "))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn tempdir() -> PathBuf {
        let d = std::env::temp_dir().join(format!(
            "theuth-acp-{}-{:?}",
            std::process::id(),
            std::thread::current().id()
        ));
        let _ = std::fs::remove_dir_all(&d);
        d
    }

    #[test]
    fn load_missing_is_empty_and_save_round_trips() {
        let dir = tempdir();
        assert!(load(&dir).unwrap().is_empty());
        let mut entries = BTreeMap::new();
        entries.insert(
            "claude".to_owned(),
            Entry {
                command: "npx".into(),
                args: vec!["-y".into(), "@agentclientprotocol/claude-agent-acp".into()],
                env: BTreeMap::new(),
                description: None,
                model: None,
                skills: None,
                skills_dir: None,
                mcp: None,
            },
        );
        entries.insert(
            "oc".to_owned(),
            Entry {
                command: "/usr/local/bin/opencode".into(),
                args: vec!["acp".into()],
                env: [("OPENCODE_CONFIG".to_owned(), "/x.json".to_owned())].into(),
                description: Some("local build".into()),
                model: Some("gpt-5.4".into()),
                skills: Some(false),
                skills_dir: Some(".claude/skills".into()),
                mcp: None,
            },
        );
        save(&dir, &entries).unwrap();
        assert_eq!(load(&dir).unwrap(), entries);
        assert!(!dir.join(DIR).join("agents.yaml.tmp").exists());
        let cfg = entries["oc"].config();
        assert_eq!(cfg.command(), Path::new("/usr/local/bin/opencode"));
        assert_eq!(cfg.arguments(), ["acp"]);
        assert_eq!(cfg.environment()["OPENCODE_CONFIG"], "/x.json");
        std::fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn describe_line() {
        assert!(describe(&BTreeMap::new()).contains("none configured"));
        let entries: BTreeMap<String, Entry> = serde_yaml::from_str(
            "claude:\n  command: npx\n  args: [-y, x]\ncodex:\n  command: codex-acp\n  description: ChatGPT\n",
        )
        .unwrap();
        assert_eq!(
            describe(&entries),
            "External harnesses: claude — npx -y x; codex — ChatGPT. A harness still busy with its previous task takes a send_message late: the prompt is retried until it is accepted."
        );
    }
}
