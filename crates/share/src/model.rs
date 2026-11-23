//! The share record (210 §5.1) and the wire events that carry it (§7.4).

use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

/// What a share serves (§5.6). `artifact` is P2.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS), ts(export))]
#[serde(rename_all = "snake_case")]
pub enum Source {
    /// A loopback port: `http://127.0.0.1:<port>` or `[::1]`.
    Port(u16),
    /// Files under an absolute directory.
    Dir(String),
}

/// Who may open a share.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS), ts(export))]
#[serde(rename_all = "snake_case")]
pub enum Access {
    /// Anyone with the link.
    #[default]
    Public,
    /// An engine-generated password (§7.5); the hash lives in `share_secrets`.
    Password,
}

impl Access {
    /// Stored form.
    pub fn as_str(self) -> &'static str {
        match self {
            Access::Public => "public",
            Access::Password => "password",
        }
    }
}

/// Lifecycle of a share (§5.1).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS), ts(export))]
#[serde(rename_all = "snake_case")]
pub enum ShareState {
    /// No certificate yet (§7.6).
    Pending,
    /// Routed at the edge.
    Live,
    /// The engine lost the edge for 10 s.
    Offline,
    /// Ended; see `stopped_by`.
    Stopped,
}

impl ShareState {
    /// Stored form.
    pub fn as_str(self) -> &'static str {
        match self {
            ShareState::Pending => "pending",
            ShareState::Live => "live",
            ShareState::Offline => "offline",
            ShareState::Stopped => "stopped",
        }
    }

    /// Parse the stored form.
    pub fn parse(s: &str) -> Option<Self> {
        Some(match s {
            "pending" => ShareState::Pending,
            "live" => ShareState::Live,
            "offline" => ShareState::Offline,
            "stopped" => ShareState::Stopped,
            _ => return None,
        })
    }
}

/// Why a share stopped (§5.1).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS), ts(export))]
#[serde(rename_all = "snake_case")]
pub enum StoppedBy {
    /// The owner pressed Stop (any surface) or `share_stop`.
    User,
    /// The expiry passed.
    Expiry,
    /// `kill {edge_ban}`.
    EdgeBan,
    /// `kill {budget}`.
    Budget,
    /// The master switch went off.
    SwitchOff,
    /// The source stopped answering or left its allowed set.
    SourceGone,
}

impl StoppedBy {
    /// Stored form.
    pub fn as_str(self) -> &'static str {
        match self {
            StoppedBy::User => "user",
            StoppedBy::Expiry => "expiry",
            StoppedBy::EdgeBan => "edge_ban",
            StoppedBy::Budget => "budget",
            StoppedBy::SwitchOff => "switch_off",
            StoppedBy::SourceGone => "source_gone",
        }
    }

    /// Parse the stored form.
    pub fn parse(s: &str) -> Option<Self> {
        Some(match s {
            "user" => StoppedBy::User,
            "expiry" => StoppedBy::Expiry,
            "edge_ban" => StoppedBy::EdgeBan,
            "budget" => StoppedBy::Budget,
            "switch_off" => StoppedBy::SwitchOff,
            "source_gone" => StoppedBy::SourceGone,
            _ => return None,
        })
    }
}

/// Who asked for the share: a tool call or Settings.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS), ts(export))]
#[serde(untagged)]
pub enum CreatedBy {
    /// `share_publish` in a session.
    Call {
        /// Session id.
        session: String,
        /// Provider call id.
        call_id: String,
    },
    /// The literal `"settings"`.
    Settings(String),
}

/// `share_terms_get` answer (TERMS-FIRST): the configured edge's current terms.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS), ts(export))]
pub struct EdgeTerms {
    /// The edge's endpoint id, what `share_terms_accept` takes.
    pub edge: String,
    /// The edge's domain.
    pub domain: String,
    /// The terms page.
    pub url: String,
    /// The current revision; empty when the edge has no terms.
    pub rev: String,
    /// This mesh accepted `rev` (always for an edge without terms).
    pub accepted: bool,
}

/// The owner's approval that let it go live (§7.2).
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS), ts(export))]
pub struct ApprovedBy {
    /// The approval row.
    pub approval_id: String,
    /// The surface that answered it.
    pub surface: String,
}

/// One share (§5.1), the row of the replicated `shares` table.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS), ts(export))]
pub struct Share {
    /// UUIDv7.
    pub id: String,
    /// The hosting engine's endpoint id.
    pub engine: String,
    /// The edge's endpoint id.
    pub edge: String,
    /// The edge's domain `D`.
    pub domain: String,
    /// `<name>.D` or `<label>.<name>.D`.
    pub host: String,
    /// `/` or `/app1`.
    pub path: String,
    /// `https://{host}{path}/` exactly as approved.
    pub url: String,
    /// What it serves.
    pub source: Source,
    /// Who may open it.
    pub access: Access,
    /// Interstitial on.
    pub interstitial: bool,
    /// Created, Unix ms.
    #[cfg_attr(feature = "ts", ts(type = "number"))]
    pub created_ms: u64,
    /// First live, Unix ms.
    #[cfg_attr(feature = "ts", ts(type = "number | null"))]
    pub live_ms: Option<u64>,
    /// Expiry, Unix ms; counts from `live_ms`.
    #[cfg_attr(feature = "ts", ts(type = "number | null"))]
    pub expires_ms: Option<u64>,
    /// Who asked.
    pub created_by: CreatedBy,
    /// The approval.
    pub approved_by: ApprovedBy,
    /// Lifecycle.
    pub state: ShareState,
    /// When it stopped.
    #[cfg_attr(feature = "ts", ts(type = "number | null"))]
    pub stopped_ms: Option<u64>,
    /// Why it stopped.
    pub stopped_by: Option<StoppedBy>,
}

/// Event `share.changed@1` (§7.4): the full record after any change, local or replicated.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS), ts(export))]
#[schemars(transform = wire::serde_type_tag::<Self>)]
#[serde(tag = "type")]
pub struct ShareChanged {
    /// The record.
    pub share: Share,
}

impl wire::WireEvent for ShareChanged {
    const TOPIC: &'static str = "share.changed";
}

/// Event `share.traffic@1` (§7.4): this engine's counters, at most one per share per 5 s.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS), ts(export))]
#[schemars(transform = wire::serde_type_tag::<Self>)]
#[serde(tag = "type")]
pub struct ShareTraffic {
    /// The share.
    pub id: String,
    /// Distinct visitors since live.
    #[cfg_attr(feature = "ts", ts(type = "number"))]
    pub visitors: u64,
    /// Requests since live.
    #[cfg_attr(feature = "ts", ts(type = "number"))]
    pub requests: u64,
    /// Last request, Unix ms.
    #[cfg_attr(feature = "ts", ts(type = "number | null"))]
    pub last_ms: Option<u64>,
}

impl wire::WireEvent for ShareTraffic {
    const TOPIC: &'static str = "share.traffic";
}

/// The client connection state of §5.9 as Settings shows it.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS), ts(export))]
#[serde(tag = "state", rename_all = "snake_case")]
pub enum EdgeState {
    /// Nothing needs the edge.
    Idle,
    /// Dialing; `attempt` counts failures since the last success.
    Dialing {
        /// Failed attempts so far.
        attempt: u32,
    },
    /// `GET /v1/info`.
    Info,
    /// Asking the lease holder for a delegation.
    Delegating,
    /// `POST /v1/hello`.
    Hello,
    /// Welcomed; events open, routes sent.
    Attached,
    /// `drain {secs}` received.
    Draining,
    /// A terminal answer; retried only on a user action or config change.
    Blocked {
        /// The edge's code (`client_too_old`, `banned`, …).
        reason: String,
    },
    /// This engine was removed from the mesh.
    Removed,
}

/// Event `share.status@1`: the edge connection of this engine changed (Settings › Sharing).
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS), ts(export))]
#[schemars(transform = wire::serde_type_tag::<Self>)]
#[serde(tag = "type")]
pub struct ShareStatus {
    /// The edge's endpoint id.
    pub edge: String,
    /// Its domain once known.
    pub domain: Option<String>,
    /// Connection state.
    pub state: EdgeState,
    /// `holder clock − edge now_ms` when `|Δ| > 5 min` (the clock warning of §5.7).
    #[cfg_attr(feature = "ts", ts(type = "number | null"))]
    pub clock_off_ms: Option<i64>,
}

impl wire::WireEvent for ShareStatus {
    const TOPIC: &'static str = "share.status";
}
