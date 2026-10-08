//! 137 R1-D6: the meeting hybrid of the endless memory mode. The meeting's
//! frame shape does not change — head D ([`crate::head`]) + the verbatim tail,
//! compacted at no model cost — but with `memory: endless` the agent's summary
//! tree runs over the meeting's own units (transcript batches, requests, taps,
//! `meeting_topic` / `meeting_brief` calls; nothing else) and its rendered view
//! enriches the head as `## Earlier in this meeting`. No settle, no wait: a
//! batch is answered at once and the tree trails.
//!
//! The agent's `Endless` handle is reachable only inside crates/agent, so this
//! module talks to it through [`Memory`], a port the binary installs once with
//! [`install`] (137-as-built-W4 hand-off); tests use a double. Without a port
//! the mode is refused at start and the meeting runs as before.

use std::sync::{Arc, OnceLock};
use std::time::Duration;

use agent::{MemoryMode, ViewLine};
use runtime::SessionId;

use crate::head::Earlier;

/// The direct tools the meeting session gains with the mode on (137 §9).
pub const TOOLS: [&str; 3] = ["zoom", "recall", "date"];

/// How long a head push waits for the view before reusing the last one. The
/// render is a fold over the in-memory tree — milliseconds; this bounds a
/// stalled port, never the compactor (the head never waits for it).
pub const RENDER_WAIT: Duration = Duration::from_secs(2);

/// 137 §9 VIEW_DOC, the meeting variant (R1-D6 e): scoped to the meeting's
/// units, the fresh-turn sentences dropped — the recent batches follow the head
/// verbatim — and the last sentence as the design asks. Rides in the head, not
/// the system prompt: the agent has no per-session prompt block, and the
/// meeting's standing rules already live in the head.
pub const VIEW_DOC: &str = "The view: this meeting so far, oldest first, inside <chat> tags, as one-line \
summaries. Each line is  id+n|text  — the n messages from id on, summarized \
(newlines shown as spaces). A summary tags each item with its kind: user (transcript \
batches, requests, taps), tool (your meeting_topic and meeting_brief calls). A short \
message is its own line, word for word. Recent lines cover one message each; the older \
the messages, the more a line covers. A message not summarized yet shows verbatim. The \
most recent messages also follow this head in full. \
Navigating: zoom(id, n) opens line id+n into the two lines it was made from; zoom(id, 1) \
gives message id in full. recall(words) searches every message's summary line. date(id) \
gives the date and time of message id. Zoom or recall when a topic was raised earlier in \
the meeting and the summary only hints at it.";

/// The two `agent.endless` keys the meeting uses (137 R1-D2, R1-D6 f), taken
/// from the agent's own `EndlessConfig` so one YAML section serves both.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Config {
    /// A meeting started without `memory` uses the endless mode.
    pub default_meeting: bool,
    /// The `## Earlier in this meeting` section's budget in bytes.
    pub meeting_view_bytes: u32,
}

impl Default for Config {
    fn default() -> Self {
        Self::from(&agent::EndlessConfig::default())
    }
}

impl From<&agent::EndlessConfig> for Config {
    fn from(c: &agent::EndlessConfig) -> Self {
        Self {
            default_meeting: c.default_meeting,
            meeting_view_bytes: c.meeting_view_bytes,
        }
    }
}

impl Config {
    /// `agent.endless` from the merged config; the agent's defaults when absent.
    pub fn load(rt: &runtime::Runtime) -> Self {
        Self::from(
            &rt.section::<agent::EndlessConfig>("agent.endless")
                .unwrap_or_default(),
        )
    }

    /// The mode a meeting starts in: the start option, else the configured default.
    pub fn resolve(&self, requested: Option<MemoryMode>) -> MemoryMode {
        requested.unwrap_or(if self.default_meeting {
            MemoryMode::Endless
        } else {
            MemoryMode::Epochs
        })
    }
}

/// What the meeting needs from `agent::endless::Endless` (137-contract.md §4
/// plus W1's follow-up `render_with_budget` / `UnitFilter::Meeting`).
#[async_trait::async_trait]
pub trait Memory: Send + Sync + 'static {
    /// `Endless::set_mode(session, MemoryMode::Endless, UnitFilter::Meeting)`:
    /// the tree over the meeting's units, no settle anywhere.
    async fn enable_meeting(&self, session: SessionId) -> Result<(), String>;
    /// `Endless::render_with_budget(session, bytes)`: the view, oldest first;
    /// unbuilt parts verbatim (batches are small).
    async fn render(&self, session: SessionId, bytes: u32) -> Result<Vec<ViewLine>, String>;
}

static PORT: OnceLock<Arc<dyn Memory>> = OnceLock::new();

/// Install the agent bridge, once; later calls are ignored.
pub fn install(port: Arc<dyn Memory>) {
    let _ = PORT.set(port);
}

/// The installed bridge, if any.
pub fn port() -> Option<Arc<dyn Memory>> {
    PORT.get().cloned()
}

/// The live allowlist plus the three memory tools (R1-D6 e), each once.
pub fn toolset(base: &agent::Toolset) -> agent::Toolset {
    let mut t = base.clone();
    for name in TOOLS {
        if !t.tools.iter().any(|n| n == name) {
            t.tools.push(name.to_owned());
        }
    }
    t
}

/// The head's `## Earlier in this meeting` input: the view within [`RENDER_WAIT`],
/// else `None` (the caller keeps the last one). Never waits for the compactor.
pub async fn earlier(port: &dyn Memory, session: SessionId, budget: u32) -> Option<Earlier> {
    match tokio::time::timeout(RENDER_WAIT, port.render(session, budget)).await {
        Ok(Ok(lines)) => Some(Earlier { lines, budget }),
        Ok(Err(e)) => {
            tracing::warn!(%session, "meeting: endless view: {e}");
            None
        }
        Err(_) => {
            tracing::warn!(%session, "meeting: endless view: no reply in {RENDER_WAIT:?}");
            None
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A fake tree: answers with `lines`, fails, or never answers.
    enum Double {
        View(Vec<ViewLine>),
        Down,
        Stalled,
    }

    #[async_trait::async_trait]
    impl Memory for Double {
        async fn enable_meeting(&self, _: SessionId) -> Result<(), String> {
            Ok(())
        }
        async fn render(&self, _: SessionId, _: u32) -> Result<Vec<ViewLine>, String> {
            match self {
                Self::View(lines) => Ok(lines.clone()),
                Self::Down => Err("pump not attached".into()),
                Self::Stalled => std::future::pending().await,
            }
        }
    }

    /// The head gets the view when it answers; an error or a stall keeps the last one
    /// (`None` here) instead of delaying the batch.
    #[tokio::test(start_paused = true)]
    async fn earlier_is_the_view_or_none_without_waiting_on_the_compactor() {
        let s = SessionId::new();
        let line = ViewLine {
            id: 0,
            n: 1,
            level: 0,
            text: "user: [meeting 00:00:00] started. context: none".into(),
            built: true,
            at: None,
        };
        assert_eq!(
            earlier(&Double::View(vec![line.clone()]), s, 32_000).await,
            Some(Earlier {
                lines: vec![line],
                budget: 32_000
            })
        );
        assert_eq!(earlier(&Double::Down, s, 32_000).await, None);
        let t = tokio::time::Instant::now();
        assert_eq!(earlier(&Double::Stalled, s, 32_000).await, None);
        assert_eq!(t.elapsed(), RENDER_WAIT, "gives up at the deadline");
    }

    #[tokio::test]
    async fn config_reads_the_agent_section_and_defaults() {
        use figment::Figment;
        use figment::providers::{Format, Yaml};
        let rt = |yaml: &str| {
            runtime::Runtime::new(
                Figment::new().merge(Yaml::string(yaml)),
                std::env::temp_dir(),
            )
        };
        assert_eq!(
            Config::load(&rt("meeting:\n  enabled: true\n")),
            Config::default()
        );
        let c = Config::load(&rt(
            "agent:\n  endless:\n    view_bytes: 128000\n    default_meeting: true\n    meeting_view_bytes: 8000\n",
        ));
        assert_eq!(
            c,
            Config {
                default_meeting: true,
                meeting_view_bytes: 8_000
            }
        );
        assert_eq!(c.resolve(None), MemoryMode::Endless);
        assert_eq!(c.resolve(Some(MemoryMode::Epochs)), MemoryMode::Epochs);
        assert_eq!(Config::default().resolve(None), MemoryMode::Epochs);
        assert_eq!(
            Config::default().resolve(Some(MemoryMode::Endless)),
            MemoryMode::Endless
        );
        assert_eq!(
            Config::default(),
            Config {
                default_meeting: false,
                meeting_view_bytes: 32_000
            }
        );
    }

    /// `meeting_start` carries `memory`; absent means "the configured default".
    #[test]
    fn the_start_option_plumbs_memory() {
        let s: crate::StartMeeting =
            serde_json::from_str(r#"{"process":null,"consent":true,"memory":"endless"}"#).unwrap();
        assert_eq!(s.memory, Some(MemoryMode::Endless));
        let s: crate::StartMeeting =
            serde_json::from_str(r#"{"process":null,"consent":true}"#).unwrap();
        assert_eq!(s.memory, None);
        let m = crate::store::Manifest::new(
            crate::MeetingId::new(),
            crate::system_audio(),
            "2026-10-08T03:00:00+02:00".into(),
        );
        assert_eq!(m.memory, MemoryMode::Epochs);
        // Manifests written before 137 have no `memory`.
        let mut v = serde_json::to_value(&m).unwrap();
        v.as_object_mut().unwrap().remove("memory");
        let old: crate::store::Manifest = serde_json::from_value(v).unwrap();
        assert_eq!(old.memory, MemoryMode::Epochs);
    }

    #[test]
    fn toolset_gains_the_three_tools_once() {
        let base = crate::MeetingConfig::default().live_toolset();
        let on = toolset(&base);
        assert_eq!(on.tools.len(), base.tools.len() + 3);
        assert!(on.tools.starts_with(&base.tools));
        for name in TOOLS {
            assert_eq!(on.tools.iter().filter(|n| *n == name).count(), 1);
        }
        assert_eq!(
            (&on.prompts, on.runtime, on.hint_limit),
            (&base.prompts, base.runtime, base.hint_limit)
        );
        assert_eq!(toolset(&on), on, "idempotent");
    }

    #[test]
    fn view_doc_ends_with_the_meeting_sentence() {
        assert!(VIEW_DOC.ends_with(
            "Zoom or recall when a topic was raised earlier in the meeting and the summary only hints at it."
        ));
        assert!(VIEW_DOC.starts_with("The view: this meeting so far"));
    }
}
