//! `<data_dir>/acp/agents.yaml`: one entry per runnable external harness
//! (design doc 06 §3.1). Read at every spawn and every `/acp`; no watcher
//! (06 §8.6).

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::time::Duration;

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
    /// Give the child the theuth MCP back-channel (`list_skills`, `load_skill`,
    /// `send_message`; 75 §9, 141 D14; plus the six text tools, 193 §7) in `session/new` |
    /// `session/load` `mcpServers`. **Default on** for every harness — unset
    /// or `mcp: true` sends it, `mcp: false` opts out; read via
    /// [`Entry::mcp_enabled`]. A harness that rejects it is retried once
    /// without it (`Harness::connect`).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub mcp: Option<bool>,
    /// 155 §8: which states this harness reports reliably. Unset = the
    /// built-in table by command ([`Reports::for_command`]); read via
    /// [`Entry::reports`].
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub reports: Option<Reports>,
    /// 06 §8.6: what a peer message does to this harness while it is busy
    /// (`steer`, `steer_window_ms`, `nudge_after_secs`, `max_nudges` at the
    /// entry's top level); read via [`Entry::steer`].
    #[serde(flatten)]
    pub steering: SteerKeys,
}

/// 06 §8.6: how a peer message reaches a harness that is busy with a turn.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SteerMode {
    /// `session/cancel` the turn in flight, then prompt again at once with
    /// the message(s) and a note to resume (what the user's Stop does,
    /// minus the stop). ACP makes `session/cancel` baseline, so every
    /// harness takes it; this is the default.
    #[default]
    Interrupt,
    /// Queue the message until the turn's prompt returns (the pre-06 §8.6
    /// behaviour), for a harness that mishandles `session/cancel`.
    Queue,
}

/// The optional steering keys of an [`Entry`] as written; unset = default.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct SteerKeys {
    /// `interrupt` (default) | `queue`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub steer: Option<SteerMode>,
    /// Messages arriving within this many ms of the first share one
    /// interrupt; default [`Steer::DEFAULT_WINDOW`].
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub steer_window_ms: Option<u64>,
    /// Seconds without streaming (thought, text or tool activity) after a
    /// prompt that carries a message before the harness is nudged; `0`
    /// turns the watch off. Default [`Steer::DEFAULT_NUDGE_AFTER`].
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub nudge_after_secs: Option<u64>,
    /// Nudges per silence before an error is published instead; default
    /// [`Steer::DEFAULT_MAX_NUDGES`].
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub max_nudges: Option<u32>,
}

/// [`SteerKeys`] resolved against the defaults.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Steer {
    /// What a peer message does to a busy harness.
    pub mode: SteerMode,
    /// The batching window of one interrupt.
    pub window: Duration,
    /// The no-activity watch after a message prompt; `None` = off.
    pub nudge_after: Option<Duration>,
    /// Nudges before the error.
    pub max_nudges: u32,
}

impl Steer {
    /// Default `steer_window_ms`.
    pub const DEFAULT_WINDOW: Duration = Duration::from_millis(1500);
    /// Default `nudge_after_secs`.
    pub const DEFAULT_NUDGE_AFTER: Duration = Duration::from_secs(20);
    /// Default `max_nudges`.
    pub const DEFAULT_MAX_NUDGES: u32 = 1;
}

impl Default for Steer {
    fn default() -> Self {
        SteerKeys::default().resolve()
    }
}

impl SteerKeys {
    /// Every unset key at its default.
    #[must_use]
    pub fn resolve(&self) -> Steer {
        Steer {
            mode: self.steer.unwrap_or_default(),
            window: self
                .steer_window_ms
                .map_or(Steer::DEFAULT_WINDOW, Duration::from_millis),
            nudge_after: match self.nudge_after_secs {
                Some(0) => None,
                Some(s) => Some(Duration::from_secs(s)),
                None => Some(Steer::DEFAULT_NUDGE_AFTER),
            },
            max_nudges: self.max_nudges.unwrap_or(Steer::DEFAULT_MAX_NUDGES),
        }
    }
}

/// 155 §8.1 (D7): the capability entry of a harness, as data. All false
/// when unknown; a false gates, never a true (§8.2).
#[derive(
    Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize, schemars::JsonSchema,
)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS), ts(export))]
#[serde(default)]
pub struct Reports {
    /// Sends `session/request_permission` for gated tools.
    pub permission: bool,
    /// `session/prompt` returns when the turn is over (continuation work
    /// theuth already folds into a continuation turn counts as reported).
    pub turn_end: bool,
    /// Advertises `loadSession` and resumes a session id.
    pub session_load: bool,
}

impl Reports {
    /// The §8.1 table by command basename / npx package. Unknown → default.
    #[must_use]
    pub fn for_command(command: &str, args: &[String]) -> Self {
        let base = std::path::Path::new(command)
            .file_name()
            .and_then(|b| b.to_str())
            .unwrap_or(command);
        let base = base.strip_suffix(".exe").unwrap_or(base);
        let name = if matches!(base, "npx" | "bunx" | "pnpx") {
            // The package: the first non-flag argument, its `@version` cut.
            args.iter()
                .find(|a| !a.starts_with('-'))
                .map_or("", |pkg| match pkg.rfind('@') {
                    Some(i) if i > 0 => &pkg[..i],
                    _ => pkg,
                })
        } else {
            base
        };
        Self::for_registry(name)
    }

    /// The same table by public-registry id (`crates/acp/src/registry.rs`).
    #[must_use]
    pub fn for_registry(id: &str) -> Self {
        let known = |session_load| Self {
            permission: true,
            turn_end: true,
            session_load,
        };
        match id {
            // omp returns from `session/prompt` and may keep working; theuth
            // folds that into a continuation turn (§13 item 1).
            "omp" => known(true),
            "opencode" | "opencode2" => known(false),
            "claude-agent-acp" | "@agentclientprotocol/claude-agent-acp" => known(false),
            "codex-acp" | "@agentclientprotocol/codex-acp" => known(false),
            _ => Self::default(),
        }
    }
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

    /// 141 D14: whether `session/new` | `session/load` carries the theuth
    /// MCP back-channel — `mcp` unset is **on**; only `mcp: false` opts out.
    #[must_use]
    pub fn mcp_enabled(&self) -> bool {
        self.mcp.unwrap_or(true)
    }

    /// 155 §8.1: the explicit `reports:` block, else the built-in table.
    #[must_use]
    pub fn reports(&self) -> Reports {
        self.reports
            .unwrap_or_else(|| Reports::for_command(&self.command, &self.args))
    }

    /// 06 §8.6: the steering keys resolved against their defaults.
    #[must_use]
    pub fn steer(&self) -> Steer {
        self.steering.resolve()
    }
}

/// [`Entry::skills_dir`] when unset: what omp, opencode, Codex and Gemini read.
pub const DEFAULT_SKILLS_DIR: &str = ".agents/skills";

/// Kind of an ACP authentication method advertised in `initialize.authMethods`.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize, schemars::JsonSchema)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS), ts(export))]
#[serde(rename_all = "snake_case")]
pub enum AuthMethodKind {
    /// Client authenticates via JSON-RPC `authenticate { methodId }`.
    Agent,
    /// User runs a command in a terminal; never passed to `authenticate`.
    Terminal,
}

/// Wire/UI view of one ACP authentication method from `initialize.authMethods`.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, schemars::JsonSchema)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS), ts(export))]
pub struct AuthMethodInfo {
    /// Method id passed to `authenticate { methodId }`.
    pub id: String,
    /// Human-readable method name.
    pub name: String,
    /// Whether the client calls `authenticate` (`agent`) or the user runs a command (`terminal`).
    pub kind: AuthMethodKind,
}

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
    /// 155 §8.1: [`Entry::reports`].
    #[serde(default)]
    pub reports: Reports,
    /// Authentication methods advertised in `initialize.authMethods` during probe.
    #[serde(default)]
    pub auth_methods: Vec<AuthMethodInfo>,
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
    platform::files::write_atomic(&path, serde_yaml::to_string(entries)?.as_bytes())
        .with_context(|| format!("write {}", path.display()))
}

/// One line for the tool description: `External harnesses: claude — …; codex — …`
/// plus how a busy harness takes a message (06 §8.3, §8.6).
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
        "External harnesses: {}. A send_message to a harness busy with a turn interrupts that turn (within ~2 s) and re-prompts it with the message and a note to resume its work; one with `steer: queue` takes it when the turn ends, retried while the harness is still busy.",
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
                reports: None,
                steering: SteerKeys::default(),
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
                reports: Some(Reports {
                    permission: true,
                    turn_end: true,
                    session_load: false,
                }),
                steering: SteerKeys {
                    steer: Some(SteerMode::Queue),
                    steer_window_ms: Some(500),
                    nudge_after_secs: Some(0),
                    max_nudges: Some(2),
                },
            },
        );
        save(&dir, &entries).unwrap();
        assert_eq!(load(&dir).unwrap(), entries);
        assert!(
            std::fs::read_dir(dir.join(DIR)).unwrap().all(|e| !e
                .unwrap()
                .file_name()
                .to_string_lossy()
                .ends_with(".tmp"))
        );
        let cfg = entries["oc"].config();
        assert_eq!(cfg.command(), Path::new("/usr/local/bin/opencode"));
        assert_eq!(cfg.arguments(), ["acp"]);
        assert_eq!(cfg.environment()["OPENCODE_CONFIG"], "/x.json");
        std::fs::remove_dir_all(&dir).unwrap();
    }

    /// 141 D14: the back-channel is on unless the entry says `mcp: false`.
    #[test]
    fn mcp_back_channel_defaults_on_and_false_opts_out() {
        let entries: BTreeMap<String, Entry> = serde_yaml::from_str(
            "omp:\n  command: omp\nquiet:\n  command: x\n  mcp: false\nloud:\n  command: y\n  mcp: true\n",
        )
        .unwrap();
        assert_eq!(entries["omp"].mcp, None);
        assert!(entries["omp"].mcp_enabled());
        assert!(!entries["quiet"].mcp_enabled());
        assert!(entries["loud"].mcp_enabled());
    }

    /// 155 §8.1: the built-in table by command basename / npx package;
    /// unknown is all false; an explicit block wins; an entry without one
    /// serialises without the key and reads back as `None`.
    #[test]
    fn reports_table_and_optional_block() {
        let all = Reports {
            permission: true,
            turn_end: true,
            session_load: true,
        };
        let no_load = Reports {
            session_load: false,
            ..all
        };
        let args = |a: &[&str]| a.iter().map(|s| (*s).to_owned()).collect::<Vec<_>>();
        assert_eq!(Reports::for_command("omp", &args(&["acp"])), all);
        assert_eq!(Reports::for_command("/opt/bin/omp", &[]), all);
        assert_eq!(
            Reports::for_command("/usr/local/bin/opencode", &args(&["acp"])),
            no_load
        );
        assert_eq!(Reports::for_command("opencode2", &[]), no_load);
        assert_eq!(
            Reports::for_command(
                "npx",
                &args(&["-y", "@agentclientprotocol/claude-agent-acp"])
            ),
            no_load
        );
        assert_eq!(
            Reports::for_command(
                "npx",
                &args(&["-y", "@agentclientprotocol/codex-acp@1.11.0"])
            ),
            no_load
        );
        assert_eq!(Reports::for_command("codex-acp", &[]), no_load);
        assert_eq!(
            Reports::for_command("gemini", &args(&["--acp"])),
            Reports::default()
        );
        assert_eq!(
            Reports::for_command("npx", &args(&["-y", "some-acp"])),
            Reports::default()
        );
        assert_eq!(Reports::for_registry("codex-acp"), no_load);
        assert_eq!(Reports::for_registry("py"), Reports::default());

        let entries: BTreeMap<String, Entry> = serde_yaml::from_str(
            "omp:\n  command: omp\nquiet:\n  command: omp\n  reports:\n    permission: false\n",
        )
        .unwrap();
        assert_eq!(entries["omp"].reports, None);
        assert_eq!(
            entries["omp"].reports(),
            all,
            "table when the block is absent"
        );
        assert_eq!(
            entries["quiet"].reports(),
            Reports::default(),
            "an explicit block is taken whole, unset keys false"
        );
        let yaml = serde_yaml::to_string(&entries).unwrap();
        assert_eq!(yaml.matches("reports:").count(), 1, "{yaml}");
        assert!(yaml.contains("permission: false"), "{yaml}");
    }

    /// 06 §8.6: the steering keys sit at the entry's top level; unset ones
    /// take the defaults, `nudge_after_secs: 0` turns the watch off, an
    /// unknown mode is a parse error, and an entry without them writes none.
    #[test]
    fn steer_keys_default_and_override() {
        let entries: BTreeMap<String, Entry> = serde_yaml::from_str(
            "omp:\n  command: omp\nslow:\n  command: x\n  steer: queue\n  steer_window_ms: 250\n  nudge_after_secs: 0\n  max_nudges: 2\n",
        )
        .unwrap();
        assert_eq!(
            entries["omp"].steer(),
            Steer {
                mode: SteerMode::Interrupt,
                window: Duration::from_millis(1500),
                nudge_after: Some(Duration::from_secs(20)),
                max_nudges: 1,
            }
        );
        assert_eq!(
            entries["slow"].steer(),
            Steer {
                mode: SteerMode::Queue,
                window: Duration::from_millis(250),
                nudge_after: None,
                max_nudges: 2,
            }
        );
        let yaml = serde_yaml::to_string(&entries).unwrap();
        assert_eq!(yaml.matches("steer").count(), 2, "{yaml}");
        assert!(serde_yaml::from_str::<Entry>("command: x\nsteer: later\n").is_err());
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
            "External harnesses: claude — npx -y x; codex — ChatGPT. A send_message to a harness busy with a turn interrupts that turn (within ~2 s) and re-prompts it with the message and a note to resume its work; one with `steer: queue` takes it when the turn ends, retried while the harness is still busy."
        );
    }
}
