//! 158 W1 owns this file (stub from C158).
//!
//! Meeting detection (158 §4): which app holds the microphone, the rules engine that turns a
//! sustained hold into one `MeetingEvent::Detected` prompt, and the `meeting_detection*` RPC
//! delegates. The contract shapes live here; `sensor.rs` is the per-OS mic-holder probe and
//! `rules.rs` the state machine. Every function below is inert until W1 fills it: nothing polls,
//! nothing opens a device, nothing is published.

pub mod rules;
pub mod sensor;

use runtime::Runtime;
use serde::{Deserialize, Serialize};

use crate::ProcessInfo;
use crate::calendar::{EventId, EventMatch};

/// `meeting.detect.*` (158 §6.1, §4.3). Nested in [`crate::MeetingConfig`]; every key defaults.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, schemars::JsonSchema)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS), ts(export))]
#[serde(default)]
pub struct DetectConfig {
    /// Run the mic sensor and the rules engine at all.
    pub enabled: bool,
    /// App ids (§4.3: bundle id on macOS, image name on Windows, `application.name` on Linux)
    /// that never prompt. "Never for <App>" appends here.
    pub ignore: Vec<String>,
    /// Sensor tick in seconds (§4.2: 2, 5 on Linux).
    pub poll_secs: u32,
}

impl Default for DetectConfig {
    fn default() -> Self {
        Self {
            enabled: true,
            ignore: DEFAULT_IGNORE.iter().map(|s| (*s).to_owned()).collect(),
            poll_secs: if cfg!(target_os = "linux") { 5 } else { 2 },
        }
    }
}

/// §4.3's deny-list: theuth's own processes, dictation apps, screen recorders, voice assistants.
/// W1 completes the identifiers it verifies per OS; the engine's own pid is skipped by the sensor.
pub const DEFAULT_IGNORE: &[&str] = &[
    "io.theuth.mac.companion",
    "io.theuth.mac",
    "Theuth.Companion.exe",
    "theuth-desktop.exe",
    "com.apple.CoreSpeech",
    "com.apple.Siri",
    "com.apple.VoiceMemos",
    "com.electron.wispr-flow",
    "com.superwhisper.macOS",
    "com.prakashjoshipax.VoiceInk",
    "com.goodsnooze.MacWhisper",
    "com.obsproject.obs-studio",
    "obs64.exe",
    "com.loom.desktop",
    "Loom.exe",
    "so.cap.desktop",
    "com.timpler.screenstudio",
    "pl.maketheweb.cleanshotx",
    "com.wulkano.kap",
];

/// Why a `Detected` prompt went away (§4.4 rules 5 and 8).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize, schemars::JsonSchema)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS), ts(export))]
pub enum Cleared {
    /// The user said "Not now": a 10-minute cooldown for that app.
    NotNow,
    /// The user said "Never for <App>": the app id joined `meeting.detect.ignore`.
    Never,
    /// The app released the mic for 30 s with nobody answering.
    Released,
    /// A meeting started (from the prompt or by hand).
    Started,
}

/// The current prompt (§4.5 `Detected`, §15.1): what `meeting_detection` returns and what
/// `Status.detection` carries so a companion connecting mid-prompt can render it.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, schemars::JsonSchema)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS), ts(export))]
pub struct Detection {
    /// The longest-held eligible app.
    pub app: ProcessInfo,
    /// Its §4.3 app id.
    pub app_id: String,
    /// RFC 3339: when the hold began.
    pub since: String,
    /// Other eligible holders at the same time (§4.4 rule 4).
    #[serde(default)]
    pub also: Vec<ProcessInfo>,
    /// The calendar match, once W2 fills it.
    #[serde(default)]
    pub event: Option<EventMatch>,
    /// The remembered or default project, once W3 fills it.
    #[serde(default)]
    pub project: Option<String>,
    /// The cached pre-meeting brief bullets, once W3 fills it.
    #[serde(default)]
    pub brief: Option<Vec<String>>,
}

/// How a meeting was started (`StartMeeting.source`, §15.1). Exported to TS as `StartSource`:
/// `voice::Source` already owns `Source.ts` in the flat bindings directory.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, schemars::JsonSchema)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS), ts(export, rename = "StartSource"))]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum Source {
    /// The start form or an RPC with no source.
    Manual,
    /// The detection prompt's "Start recording".
    Detected {
        /// The prompted app's id.
        app_id: String,
    },
    /// An `Upcoming` prompt or an event picked by hand.
    Calendar {
        /// The event.
        event: EventId,
    },
}

/// `meeting_detection() -> Option<Detection>` (§15.2, W1). Stub: no prompt ever.
pub async fn rpc_detection(_rt: &Runtime) -> anyhow::Result<Option<Detection>> {
    Ok(None)
}

/// `meeting_detection_dismiss(app_id, never)` (§15.2, W1): "Not now" (cooldown) or "Never for"
/// (append to `meeting.detect.ignore`). Stub: refused until W1 builds the rules engine.
pub async fn rpc_detection_dismiss(
    _rt: &Runtime,
    _app_id: String,
    _never: bool,
) -> anyhow::Result<()> {
    Err(anyhow::anyhow!("158 W1: not built yet"))
}
