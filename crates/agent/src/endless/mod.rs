//! 137 — Endless: an optional summary-tree memory mode per session. Units
//! are derived from `messages` rows ([`units`]), a background pump
//! ([`pump`]) summarises them into a binary tree of ≤ `node_bytes` lines
//! ([`tree`], [`compactor`], [`guards`]), and a slack fold ([`fold`]) turns the
//! tree into the fixed-size view every turn starts from. [`Endless`] is the
//! one handle the loop, the tools and the RPC call (137-contract.md §4).

use std::collections::HashMap;
use std::sync::Arc;
use std::time::Duration;

use chrono::{DateTime, Utc};
use parking_lot::{Mutex, RwLock};
use runtime::{Runtime, SessionId};
use serde::{Deserialize, Serialize};

use crate::store::Store;

pub mod compactor;
pub mod fold;
pub mod guards;
pub mod ledger;
pub mod lines;
pub mod pump;
pub mod tail;
pub mod tool_pin;
#[cfg(test)]
mod tests;
#[cfg(test)]
pub(crate) mod eval_tests;
pub mod tree;
pub mod units;

pub use fold::{ViewBlock, ViewRender};
pub use tree::{Kind, Node, Unit};

/// `agent.endless` (137 §11.1). Every key optional in YAML; the whole struct
/// is what Settings › Memory reads and writes (`endless_settings_get/set`).
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, schemars::JsonSchema)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS), ts(export))]
#[serde(default)]
pub struct EndlessConfig {
    /// New chat ∞ switch initial state.
    pub default_chat: bool,
    /// Desks without an explicit `memory:` field (R1-D7: on).
    pub default_desk: bool,
    /// Meeting sessions (R1-D6).
    pub default_meeting: bool,
    /// View budget in bytes (16_000..=256_000; R1-D2: 128 KB).
    pub view_bytes: u32,
    /// View budget for a meeting head's `## Earlier in this meeting` (R1-D6).
    pub meeting_view_bytes: u32,
    /// R1-D4: at switch-on the tree covers only the last this many units.
    pub backfill_max_units: u32,
    /// One summary line's ceiling in bytes.
    pub node_bytes: u32,
    /// Slack band of the fold (0.0..=0.5), D5.
    pub slack: f32,
    /// How long a turn waits for the view to be fully built (D10).
    pub settle_secs: u32,
    /// Compactor calls in flight per session.
    pub jobs: u8,
    /// Attempts per node before the best candidate wins (D7).
    pub tries: u8,
    /// Delay before a failed node is retried.
    pub retry_secs: u32,
    /// Tool-result cap in chars before a unit is cut head ⅔ / tail ⅓ (§4).
    pub cap_chars: u32,
    /// Compactor model; `None` = the session's own model (D8).
    pub model: Option<String>,
    /// 194: the compactor model's effort; `None` = thinking off (R1 Q1). Only
    /// with `model` (validated against it, dropped when a new `model` does not take it).
    #[cfg_attr(feature = "ts", ts(optional = nullable))]
    pub effort: Option<String>,
    /// 137F E6: one attempt's bound in seconds for a cloud compactor (10..=3600).
    #[cfg_attr(feature = "ts", ts(as = "Option<u32>", optional))]
    pub attempt_secs: u32,
    /// 137F E6: one attempt's bound in seconds for a local compactor (30..=7200).
    #[cfg_attr(feature = "ts", ts(as = "Option<u32>", optional))]
    pub local_attempt_secs: u32,
    /// 137F E8: a local compactor sees at most this many bytes of the `<chat>` view, newest kept (4_000..=256_000).
    #[cfg_attr(feature = "ts", ts(as = "Option<u32>", optional))]
    pub local_ctx_bytes: u32,
    /// 137F E9: compactor calls in flight per local endpoint, engine-wide (1..=16).
    #[cfg_attr(feature = "ts", ts(as = "Option<u8>", optional))]
    pub local_jobs: u8,
    /// 137G G2: unsummarized lines the view shows verbatim, newest first, in bytes (4_000..=128_000); older ones fold into one placeholder line.
    #[cfg_attr(feature = "ts", ts(as = "Option<u32>", optional))]
    pub raw_bytes: u32,
    /// 137G G3: the compactor's `<chat>` context cap in bytes for every model (8_000..=256_000); a local one takes the smaller of this and `local_ctx_bytes`.
    #[cfg_attr(feature = "ts", ts(as = "Option<u32>", optional))]
    pub compactor_ctx_bytes: u32,
    /// 137G G6: consecutive unsummarized units one compactor call summarizes (1..=64).
    #[cfg_attr(feature = "ts", ts(as = "Option<u8>", optional))]
    pub batch_units: u8,
    /// 137G G11: USD one compactor call's prompt may cost (0.001..=10); over it the context shrinks, else the line folds mechanically.
    #[cfg_attr(feature = "ts", ts(as = "Option<f64>", optional))]
    pub max_usd_per_call: f64,
    /// 137G G11: USD a chat's compactor may spend per UTC day (0..=1000, 0 = no cap); reaching it pauses settling (`paused_by: "spend"`).
    #[cfg_attr(feature = "ts", ts(as = "Option<f64>", optional))]
    pub max_usd_per_session_day: f64,
    /// 201 §4.2: how the verbatim tail is sized (`fixed` | `scaled`).
    #[cfg_attr(feature = "ts", ts(as = "Option<TailMode>", optional))]
    pub tail_mode: TailMode,
    /// 201 §4.2 fixed mode: verbatim context kept after a trim (8_000..=256_000).
    #[cfg_attr(feature = "ts", ts(as = "Option<u32>", optional))]
    pub tail_low_bytes: u32,
    /// 201 §4.2 fixed mode: where a trim is due (16_000..=512_000); above the low mark.
    #[cfg_attr(feature = "ts", ts(as = "Option<u32>", optional))]
    pub tail_high_bytes: u32,
    /// 201 §4.2 scaled mode: low mark as a share of the session model's turn allowance (0.02..=0.40).
    #[cfg_attr(feature = "ts", ts(as = "Option<f64>", optional))]
    pub tail_low_share: f64,
    /// 201 §4.2 scaled mode: high mark as a share (0.05..=0.60); above the low share.
    #[cfg_attr(feature = "ts", ts(as = "Option<f64>", optional))]
    pub tail_high_share: f64,
    /// 201 §4.3: idle seconds after which a trim is taken early (0..=86_400);
    /// 0 = the transport's cache lifetime.
    #[cfg_attr(feature = "ts", ts(as = "Option<u32>", optional))]
    pub cold_after_secs: u32,
    /// 201 §7.1: compactor profile (`auto` | `standard` | `small`).
    #[cfg_attr(feature = "ts", ts(as = "Option<Profile>", optional))]
    pub profile: Profile,
    /// 201 §6: share of `view_bytes` at which merges start being built (0.50..=1.00).
    #[cfg_attr(feature = "ts", ts(as = "Option<f64>", optional))]
    pub merge_at: f64,
    /// 201 §9: the state ledger and its tool.
    #[cfg_attr(feature = "ts", ts(as = "Option<bool>", optional))]
    pub ledger: bool,
    /// 201 §9: ledger snapshot budget in bytes (1_000..=16_000).
    #[cfg_attr(feature = "ts", ts(as = "Option<u32>", optional))]
    pub ledger_bytes: u32,
    /// 201 §9: summary-model flags shown as unconfirmed suggestions.
    #[cfg_attr(feature = "ts", ts(as = "Option<bool>", optional))]
    pub ledger_suggestions: bool,
    /// 201 §10: recall hits appended to the input, in bytes (0..=8_000); 0 = off.
    #[cfg_attr(feature = "ts", ts(as = "Option<u32>", optional))]
    pub prefetch_bytes: u32,
    /// 201 §10: a zoomed line stays unmerged for one generation.
    #[cfg_attr(feature = "ts", ts(as = "Option<bool>", optional))]
    pub age_by_use: bool,
    /// 201 §8: one-line description when an image leaves the tail (`auto` | `off`).
    #[cfg_attr(feature = "ts", ts(as = "Option<DescribeImages>", optional))]
    pub describe_images: DescribeImages,
}

impl Default for EndlessConfig {
    fn default() -> Self {
        Self {
            default_chat: false,
            default_desk: true,
            default_meeting: false,
            view_bytes: 128_000,
            meeting_view_bytes: 32_000,
            backfill_max_units: 2_000,
            node_bytes: 512,
            slack: 0.125,
            settle_secs: 45,
            jobs: 8,
            tries: 5,
            retry_secs: 10,
            cap_chars: 30_000,
            model: None,
            effort: None,
            attempt_secs: 120,
            local_attempt_secs: 600,
            local_ctx_bytes: 24_000,
            local_jobs: 1,
            raw_bytes: 32_000,
            compactor_ctx_bytes: 48_000,
            batch_units: 16,
            max_usd_per_call: 0.10,
            max_usd_per_session_day: 5.0,
            tail_mode: TailMode::Fixed,
            tail_low_bytes: 24_000,
            tail_high_bytes: 64_000,
            tail_low_share: 0.10,
            tail_high_share: 0.25,
            cold_after_secs: 0,
            profile: Profile::Auto,
            merge_at: 0.75,
            ledger: true,
            ledger_bytes: 4_000,
            ledger_suggestions: true,
            prefetch_bytes: 2_000,
            age_by_use: true,
            describe_images: DescribeImages::Auto,
        }
    }
}

impl EndlessConfig {
    /// The fold parameters, clamped to the documented ranges.
    pub fn fold_cfg(&self) -> fold::FoldCfg {
        self.fold_cfg_for(self.view_bytes)
    }

    /// Fold parameters for a per-session / per-render budget.
    pub fn fold_cfg_for(&self, view_bytes: u32) -> fold::FoldCfg {
        fold::FoldCfg {
            view_bytes: view_bytes.clamp(16_000, 256_000),
            slack: self.slack.clamp(0.0, 0.5),
            raw_bytes: self.raw_bytes.clamp(4_000, 128_000),
        }
    }

    /// 201 §12.1: this config with every 201 range applied (the clamp at
    /// use; the 137 keys clamp where they are read, as before).
    pub fn clamped(&self) -> Self {
        let mut c = self.clone();
        c.tail_low_bytes = c.tail_low_bytes.clamp(TAIL_LOW_BYTES.0, TAIL_LOW_BYTES.1);
        c.tail_high_bytes = c
            .tail_high_bytes
            .clamp(TAIL_HIGH_BYTES.0, TAIL_HIGH_BYTES.1);
        c.tail_low_share = clamp_f(c.tail_low_share, TAIL_LOW_SHARE);
        c.tail_high_share = clamp_f(c.tail_high_share, TAIL_HIGH_SHARE);
        c.cold_after_secs = c.cold_after_secs.min(86_400);
        c.merge_at = clamp_f(c.merge_at, (0.50, 1.00));
        c.ledger_bytes = c.ledger_bytes.clamp(1_000, 16_000);
        c.prefetch_bytes = c.prefetch_bytes.min(8_000);
        c
    }

    /// 201 §12.1: what `endless_settings_set` refuses among the 201 keys —
    /// a value out of its range, or a pair whose high mark is not above its low one.
    pub fn validate_201(&self) -> Result<(), String> {
        let u = |key: &str, v: u32, (lo, hi): (u32, u32)| {
            if (lo..=hi).contains(&v) {
                Ok(())
            } else {
                Err(format!("{key} {v} out of range {lo}..={hi}"))
            }
        };
        let f = |key: &str, v: f64, (lo, hi): (f64, f64)| {
            if (lo..=hi).contains(&v) {
                Ok(())
            } else {
                Err(format!("{key} {v} out of range {lo}..={hi}"))
            }
        };
        u("tail_low_bytes", self.tail_low_bytes, TAIL_LOW_BYTES)?;
        u("tail_high_bytes", self.tail_high_bytes, TAIL_HIGH_BYTES)?;
        f("tail_low_share", self.tail_low_share, TAIL_LOW_SHARE)?;
        f("tail_high_share", self.tail_high_share, TAIL_HIGH_SHARE)?;
        u("cold_after_secs", self.cold_after_secs, (0, 86_400))?;
        f("merge_at", self.merge_at, (0.50, 1.00))?;
        u("ledger_bytes", self.ledger_bytes, (1_000, 16_000))?;
        u("prefetch_bytes", self.prefetch_bytes, (0, 8_000))?;
        tail_pair_ok(
            f64::from(self.tail_low_bytes),
            f64::from(self.tail_high_bytes),
            "tail_low_bytes",
            "tail_high_bytes",
        )?;
        tail_pair_ok(
            self.tail_low_share,
            self.tail_high_share,
            "tail_low_share",
            "tail_high_share",
        )
    }

    /// This config with the desk agent's `tail_*:` keys applied (201 §4.2;
    /// each key that is set replaces the config's).
    pub fn with_agent_tail(&self, a: &AgentTail) -> Self {
        let mut c = self.clone();
        if let Some(m) = a.mode {
            c.tail_mode = m;
        }
        if let Some(v) = a.low_bytes {
            c.tail_low_bytes = v;
        }
        if let Some(v) = a.high_bytes {
            c.tail_high_bytes = v;
        }
        if let Some(v) = a.low_share {
            c.tail_low_share = v;
        }
        if let Some(v) = a.high_share {
            c.tail_high_share = v;
        }
        if let Some(p) = a.profile {
            c.profile = p;
        }
        c
    }

    /// This config with a per-agent or per-session tail override applied
    /// (201 §4.2: the mode and its two marks, in that mode's unit).
    pub fn with_tail(&self, o: Option<&TailOverride>) -> Self {
        let mut c = self.clone();
        if let Some(o) = o {
            c.tail_mode = o.mode;
            match o.mode {
                TailMode::Fixed => {
                    c.tail_low_bytes = o.low as u32;
                    c.tail_high_bytes = o.high as u32;
                }
                TailMode::Scaled => {
                    c.tail_low_share = o.low;
                    c.tail_high_share = o.high;
                }
            }
        }
        c
    }

    /// 201 §4.2: the tail's (low, high) marks in bytes for a session whose
    /// model allows `allowance_bytes` per turn. `fixed` takes the byte keys,
    /// `scaled` the shares of `allowance_bytes`; both clamped to
    /// 8_000..=512_000 with high above low, and scaled down together
    /// (keeping their ratio) when high exceeds half of `allowance_bytes`.
    /// Pure; phase P2 calls it.
    pub fn tail_marks(&self, allowance_bytes: u64) -> (u32, u32) {
        let c = self.clamped();
        let (low, high) = match c.tail_mode {
            TailMode::Fixed => (f64::from(c.tail_low_bytes), f64::from(c.tail_high_bytes)),
            TailMode::Scaled => {
                let a = allowance_bytes as f64;
                (a * c.tail_low_share, a * c.tail_high_share)
            }
        };
        let low = low.clamp(TAIL_MARK.0, TAIL_MARK.1 - 1.0);
        let high = high.clamp(low + 1.0, TAIL_MARK.1);
        let half = allowance_bytes as f64 / 2.0;
        let (low, high) = if high > half {
            let k = half / high;
            (low * k, high * k)
        } else {
            (low, high)
        };
        let low = low.floor() as u32;
        (low, (high.floor() as u32).max(low + 1))
    }
}

/// 201 §12.1 ranges.
const TAIL_LOW_BYTES: (u32, u32) = (8_000, 256_000);
const TAIL_HIGH_BYTES: (u32, u32) = (16_000, 512_000);
const TAIL_LOW_SHARE: (f64, f64) = (0.02, 0.40);
const TAIL_HIGH_SHARE: (f64, f64) = (0.05, 0.60);
/// 201 §4.2: both marks, in bytes, before the allowance scale-down.
const TAIL_MARK: (f64, f64) = (8_000.0, 512_000.0);

/// Clamp; a NaN reads as the low end.
fn clamp_f(v: f64, (lo, hi): (f64, f64)) -> f64 {
    if v.is_nan() { lo } else { v.clamp(lo, hi) }
}

/// 201 §12.1: a high mark not above its low mark is refused, naming both.
fn tail_pair_ok(low: f64, high: f64, low_key: &str, high_key: &str) -> Result<(), String> {
    if high > low {
        Ok(())
    } else {
        Err(format!("{high_key} {high} must be above {low_key} {low}"))
    }
}

/// 201 §4.2: how the verbatim tail is sized.
#[derive(
    Clone, Copy, Debug, Default, PartialEq, Eq, Hash, Serialize, Deserialize, schemars::JsonSchema,
)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS), ts(export))]
#[serde(rename_all = "snake_case")]
pub enum TailMode {
    /// `tail_low_bytes` / `tail_high_bytes`.
    #[default]
    Fixed,
    /// `tail_low_share` / `tail_high_share` of the session model's turn allowance.
    Scaled,
}

impl TailMode {
    /// The stored string form.
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Fixed => "fixed",
            Self::Scaled => "scaled",
        }
    }
}

impl std::fmt::Display for TailMode {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(self.as_str())
    }
}

impl std::str::FromStr for TailMode {
    type Err = String;
    fn from_str(s: &str) -> Result<Self, String> {
        match s {
            "fixed" => Ok(Self::Fixed),
            "scaled" => Ok(Self::Scaled),
            _ => Err(format!("unknown tail mode: {s:?}")),
        }
    }
}

/// 201 §7.1: the compactor profile. Named `CompactorProfile` on the wire
/// (TypeScript, JSON schema): `meeting` already exports a `Profile`.
#[derive(
    Clone, Copy, Debug, Default, PartialEq, Eq, Hash, Serialize, Deserialize, schemars::JsonSchema,
)]
#[cfg_attr(
    feature = "ts",
    derive(ts_rs::TS),
    ts(export, rename = "CompactorProfile")
)]
#[serde(rename = "CompactorProfile", rename_all = "snake_case")]
pub enum Profile {
    /// `small` for a local model or a known window of at most 32_000 tokens, else `standard`.
    #[default]
    Auto,
    /// 137G as built, with fixes.
    Standard,
    /// One unit per call, short prompt and reply (§7.2).
    Small,
}

impl Profile {
    /// The stored string form.
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Auto => "auto",
            Self::Standard => "standard",
            Self::Small => "small",
        }
    }
}

impl std::fmt::Display for Profile {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(self.as_str())
    }
}

impl std::str::FromStr for Profile {
    type Err = String;
    fn from_str(s: &str) -> Result<Self, String> {
        match s {
            "auto" => Ok(Self::Auto),
            "standard" => Ok(Self::Standard),
            "small" => Ok(Self::Small),
            _ => Err(format!("unknown compactor profile: {s:?}")),
        }
    }
}

/// 201 §8: whether an image leaving the tail gets a one-line description.
#[derive(
    Clone, Copy, Debug, Default, PartialEq, Eq, Hash, Serialize, Deserialize, schemars::JsonSchema,
)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS), ts(export))]
#[serde(rename_all = "snake_case")]
pub enum DescribeImages {
    /// Described by the compactor chain.
    #[default]
    Auto,
    /// Never described; the unit keeps its name only.
    Off,
}

impl DescribeImages {
    /// The stored string form.
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Auto => "auto",
            Self::Off => "off",
        }
    }
}

/// 201 §4.2: a desk agent's `tail_*:` keys from `agents/<name>.md`, any
/// subset; applied to its desk session below the session's own override.
#[derive(Clone, Copy, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct AgentTail {
    /// `tail_mode:`.
    pub mode: Option<TailMode>,
    /// `tail_low_bytes:`.
    pub low_bytes: Option<u32>,
    /// `tail_high_bytes:`.
    pub high_bytes: Option<u32>,
    /// `tail_low_share:`.
    pub low_share: Option<f64>,
    /// `tail_high_share:`.
    pub high_share: Option<f64>,
    /// `profile:` (201 §7.1, P3).
    pub profile: Option<Profile>,
}

/// 201 §4.2: a per-session (or per-agent) tail override: the mode and its two
/// marks in that mode's unit — bytes for `fixed`, shares of the turn
/// allowance for `scaled`. Stored on the session row beside `endless_view_bytes`.
#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize, schemars::JsonSchema)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS), ts(export))]
pub struct TailOverride {
    /// How the tail is sized.
    pub mode: TailMode,
    /// Low mark: bytes (8_000..=256_000) or share (0.02..=0.40).
    pub low: f64,
    /// High mark: bytes (16_000..=512_000) or share (0.05..=0.60); above `low`.
    pub high: f64,
}

impl TailOverride {
    /// The §12.1 ranges of `mode` and high above low; `Err` names the values.
    pub fn validate(&self) -> Result<(), String> {
        let (lk, hk, lr, hr) = match self.mode {
            TailMode::Fixed => (
                "tail_low_bytes",
                "tail_high_bytes",
                (f64::from(TAIL_LOW_BYTES.0), f64::from(TAIL_LOW_BYTES.1)),
                (f64::from(TAIL_HIGH_BYTES.0), f64::from(TAIL_HIGH_BYTES.1)),
            ),
            TailMode::Scaled => (
                "tail_low_share",
                "tail_high_share",
                TAIL_LOW_SHARE,
                TAIL_HIGH_SHARE,
            ),
        };
        for (k, v, (lo, hi)) in [(lk, self.low, lr), (hk, self.high, hr)] {
            if !(lo..=hi).contains(&v) {
                return Err(format!("{k} {v} out of range {lo}..={hi}"));
            }
        }
        tail_pair_ok(self.low, self.high, lk, hk)
    }
}

/// R1-D3: the compactor model id — `agent.endless.model` → `agent.title_model`
/// → the session's model. What Settings and the Memory panel display.
pub fn resolved_compactor_model(cfg: &crate::AgentConfig, session_model: &str) -> String {
    cfg.endless
        .model
        .clone()
        .or_else(|| cfg.title_model.clone())
        .unwrap_or_else(|| session_model.to_owned())
}

/// R1-D6: which parts become units (`sessions.endless_filter`, fixed at switch-on).
#[derive(
    Clone, Copy, Debug, Default, PartialEq, Eq, Hash, Serialize, Deserialize, schemars::JsonSchema,
)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS), ts(export))]
#[serde(rename_all = "snake_case")]
pub enum UnitFilter {
    /// Every part (§4 table).
    #[default]
    All,
    /// Meetings: `user` texts starting with `[meeting` and `tool` calls
    /// `meeting_topic` / `meeting_brief`; everything else dropped.
    Meeting,
}

impl UnitFilter {
    /// The stored string form.
    pub fn as_str(self) -> &'static str {
        match self {
            Self::All => "all",
            Self::Meeting => "meeting",
        }
    }

    /// Whether `unit` enters the tree.
    pub fn keeps(self, unit: &tree::Unit) -> bool {
        match self {
            Self::All => true,
            Self::Meeting => match unit.kind {
                tree::Kind::User => unit.text.starts_with("[meeting"),
                tree::Kind::Tool => {
                    unit.text.starts_with("meeting_topic ")
                        || unit.text.starts_with("meeting_brief ")
                }
                _ => false,
            },
        }
    }
}

impl std::str::FromStr for UnitFilter {
    type Err = String;
    fn from_str(s: &str) -> Result<Self, String> {
        match s {
            "all" => Ok(Self::All),
            "meeting" => Ok(Self::Meeting),
            _ => Err(format!("unknown unit filter: {s:?}")),
        }
    }
}

/// A session's memory mode (`sessions.memory`, D1).
#[derive(
    Clone, Copy, Debug, Default, PartialEq, Eq, Hash, Serialize, Deserialize, schemars::JsonSchema,
)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS), ts(export))]
#[serde(rename_all = "snake_case")]
pub enum MemoryMode {
    /// 07/11 epochs: carried transcript, folded into summary heads.
    #[default]
    Epochs,
    /// 137: the summary tree and its view.
    Endless,
}

impl MemoryMode {
    /// The stored string form.
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Epochs => "epochs",
            Self::Endless => "endless",
        }
    }
}

impl std::fmt::Display for MemoryMode {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(self.as_str())
    }
}

impl std::str::FromStr for MemoryMode {
    type Err = String;
    fn from_str(s: &str) -> Result<Self, String> {
        match s {
            "epochs" => Ok(Self::Epochs),
            "endless" => Ok(Self::Endless),
            _ => Err(format!("unknown context mode: {s:?}")),
        }
    }
}

/// One line of the view (or of a zoom / recall result) as the UI sees it.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, schemars::JsonSchema)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS), ts(export))]
pub struct ViewLine {
    /// First unit the line covers.
    #[cfg_attr(feature = "ts", ts(type = "number"))]
    pub id: u64,
    /// Units covered (a power of two).
    #[cfg_attr(feature = "ts", ts(type = "number"))]
    pub n: u64,
    /// Tree level (`n == 2^level`).
    pub level: u8,
    /// The line (placeholder or verbatim unit when not built, D10).
    pub text: String,
    /// A built summary line (not a placeholder).
    pub built: bool,
    /// The message's row time for an `n == 1` line (W3: zoom shows its date); `None` otherwise.
    #[serde(default)]
    #[cfg_attr(feature = "ts", ts(optional = nullable))]
    pub at: Option<DateTime<Utc>>,
    /// 137G: a line made by the mechanical fold (an excerpt, not a model summary).
    #[serde(default)]
    #[cfg_attr(feature = "ts", ts(as = "Option<bool>", optional))]
    pub mechanical: bool,
}

/// Guard trips by kind (§6).
#[derive(
    Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize, schemars::JsonSchema,
)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS), ts(export))]
pub struct GuardTrips {
    /// G1 trips.
    pub copy: u32,
    /// G2 trips.
    pub grounding: u32,
    /// G3 trips.
    pub status: u32,
    /// G4 trips.
    pub format: u32,
    /// G5 trips.
    pub empty: u32,
    /// Size-limit retries.
    pub size: u32,
}

/// Per-session compactor totals (§6, the Memory panel header).
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize, schemars::JsonSchema)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS), ts(export))]
pub struct EndlessStats {
    /// Compactor attempts recorded.
    pub calls: u32,
    /// Retries by guard.
    pub guard_trips: GuardTrips,
    /// Prompt tokens, cache buckets included.
    #[cfg_attr(feature = "ts", ts(type = "number"))]
    pub input_tokens: u64,
    /// Prompt tokens read from cache.
    #[cfg_attr(feature = "ts", ts(type = "number"))]
    pub cache_read_tokens: u64,
    /// Prompt tokens written to cache.
    #[cfg_attr(feature = "ts", ts(type = "number"))]
    pub cache_write_tokens: u64,
    /// Completion tokens.
    #[cfg_attr(feature = "ts", ts(type = "number"))]
    pub output_tokens: u64,
    /// Catalog-rate estimate in USD.
    pub usd_estimate: f64,
}

/// Backfill progress (D14) and cost (R1-D4): also the shape of
/// [`Endless::backfill_estimate`] before a backfill starts.
#[derive(Clone, Copy, Debug, Default, PartialEq, Serialize, Deserialize, schemars::JsonSchema)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS), ts(export))]
pub struct Backfill {
    /// History units with a level-0 line.
    #[cfg_attr(feature = "ts", ts(type = "number"))]
    pub done: u64,
    /// History units to cover (after the `backfill_max_units` cap).
    #[cfg_attr(feature = "ts", ts(type = "number"))]
    pub total: u64,
    /// Remaining × measured seconds per unit (5 s before any node is built).
    #[cfg_attr(feature = "ts", ts(type = "number"))]
    pub eta_secs: u64,
    /// Remaining × measured USD per unit ($0.02 before any node is built).
    #[serde(default)]
    pub usd_estimate: f64,
}

/// A view part that keeps failing to build (137F). Engine → UI.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, schemars::JsonSchema)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS), ts(export))]
pub struct Stalled {
    /// Consecutive failed attempts of the failing node that blocks most (the frontier first).
    pub failures: u32,
    /// The last error, one line, ≤ 300 chars (secrets never appear: it is the provider's message).
    pub error: String,
    /// RFC 3339 UTC time of the pump's own next retry; `None` while a retry is running.
    #[cfg_attr(feature = "ts", ts(optional = nullable))]
    pub next_retry_at: Option<String>,
    /// The compactor model id that failed.
    pub model: String,
    /// The failing node is the level-0 frontier: newer lines wait behind it.
    pub blocking: bool,
}

/// Unsummarized lines waiting for the compactor (137G). Engine → UI. TS name
/// `EndlessBacklog`: `dreaming::Backlog` owns `Backlog.ts`.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize, schemars::JsonSchema)]
#[cfg_attr(
    feature = "ts",
    derive(ts_rs::TS),
    ts(export, rename = "EndlessBacklog")
)]
pub struct Backlog {
    /// Level-0 lines not yet built.
    pub lines: u32,
    /// Of those, how many the view currently shows folded into one placeholder line (the raw budget).
    pub folded: u32,
    /// Estimate at the measured throughput; 0 when paused or unknown.
    #[cfg_attr(feature = "ts", ts(type = "number"))]
    pub eta_secs: u64,
    /// Settling is paused for this session (no compactor calls).
    pub paused: bool,
    /// Who paused: `"user"` (`endless_pause`) or `"spend"` (the G11 day cap); `None` while running.
    #[serde(default)]
    #[cfg_attr(feature = "ts", ts(optional = nullable))]
    pub paused_by: Option<String>,
}

/// RPC `session_memory` (§11.2).
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize, schemars::JsonSchema)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS), ts(export))]
pub struct SessionMemory {
    /// The session's mode.
    pub mode: MemoryMode,
    /// Empty for an epochs session.
    pub view: Vec<ViewLine>,
    /// Some view part is still being built.
    pub settling: bool,
    /// Parts rendered per D10 right now.
    pub unbuilt: u32,
    /// Units so far (`T`).
    #[cfg_attr(feature = "ts", ts(type = "number"))]
    pub units: u64,
    /// Compactor totals.
    pub stats: EndlessStats,
    /// Compactor model id; `""` for an epochs session.
    pub model: String,
    /// The view budget in force (R1b: the session's own, else `agent.endless.view_bytes`).
    pub view_bytes: u32,
    /// D14 progress while history is being covered.
    #[cfg_attr(feature = "ts", ts(optional = nullable))]
    pub backfill: Option<Backfill>,
    /// 137F: the failing node that blocks most; `None` when nothing is failing.
    #[serde(default)]
    #[cfg_attr(feature = "ts", ts(optional = nullable))]
    pub stalled: Option<Stalled>,
    /// 137G: the unsettled backlog; `None` when nothing waits and the session is not paused.
    #[serde(default)]
    #[cfg_attr(feature = "ts", ts(optional = nullable))]
    pub backlog: Option<Backlog>,
    /// 137G: this chat's own compactor pick (`set_session_compactor_model`); `None` = inherit.
    #[serde(default)]
    #[cfg_attr(feature = "ts", ts(optional = nullable))]
    pub model_override: Option<String>,
    /// 137G: which link chose `model`: `"chat"` | `"agent"` | `"settings"` | `"title"` | `"session"`;
    /// `""` for an epochs session.
    #[serde(default)]
    #[cfg_attr(feature = "ts", ts(as = "Option<String>", optional))]
    pub model_source: String,
    /// 194: the effort `model` runs with — this chat's own pick
    /// (`set_session_compactor_effort`), else the effort of the link that chose
    /// `model` (the agent's `memory_effort`, `agent.endless.effort`); `None` =
    /// thinking off. An effort `model` does not take is dropped (`None`).
    #[serde(default)]
    #[cfg_attr(feature = "ts", ts(optional = nullable))]
    pub effort: Option<String>,
    /// 194 R1: this chat's own compactor effort (`set_session_compactor_effort`),
    /// stored beside `model_override`; `None` = inherit. The panel's chips show it.
    #[serde(default)]
    #[cfg_attr(feature = "ts", ts(optional = nullable))]
    pub effort_override: Option<String>,
    /// 137G G11: this chat's compactor spend in the current UTC day (USD, `model_calls` kind `endless`).
    #[serde(default)]
    #[cfg_attr(feature = "ts", ts(as = "Option<f64>", optional))]
    pub spend_today_usd: f64,
    /// 201 §4.2: the tail sizing in force for this session (the session's
    /// override, else `agent.endless.tail_mode`).
    #[serde(default)]
    #[cfg_attr(feature = "ts", ts(as = "Option<TailMode>", optional))]
    pub tail_mode: TailMode,
    /// 201 §4.2: the low mark in force, in bytes ([`EndlessConfig::tail_marks`]).
    /// In `scaled` mode 0 until phase P2 knows the session model's allowance.
    #[serde(default)]
    #[cfg_attr(feature = "ts", ts(as = "Option<u32>", optional))]
    pub tail_low: u32,
    /// 201 §4.2: the high mark in force, in bytes; as `tail_low`.
    #[serde(default)]
    #[cfg_attr(feature = "ts", ts(as = "Option<u32>", optional))]
    pub tail_high: u32,
    /// 201 §4.2: this chat's own tail override (`set_session_tail`); `None` = inherit.
    #[serde(default)]
    #[cfg_attr(feature = "ts", ts(optional = nullable))]
    pub tail_override: Option<TailOverride>,
    /// 201 §4.1: bytes of the verbatim tail now. Phase P2 fills it; 0 until then.
    #[serde(default)]
    #[cfg_attr(feature = "ts", ts(as = "Option<u32>", optional))]
    pub tail_bytes: u32,
    /// 201 §4.3: trims taken so far (the view's generation). Phase P2 fills it; 0 until then.
    #[serde(default)]
    #[cfg_attr(feature = "ts", ts(as = "Option<u32>", optional))]
    pub generation: u32,
    /// 201 §5: units whose line is still a draft. Phase P1 fills it; 0 until then.
    #[serde(default)]
    #[cfg_attr(feature = "ts", ts(as = "Option<u32>", optional))]
    pub drafts: u32,
    /// 201 §7.1: the resolved compactor profile, never `auto`. Phase P3
    /// resolves it; `standard` until then.
    #[serde(default = "standard_profile")]
    #[cfg_attr(feature = "ts", ts(as = "Option<Profile>", optional))]
    pub profile: Profile,
    /// 201 §7.1: why `profile` was chosen, short text ("the model is local").
    /// Phase P3 fills it; `""` until then.
    #[serde(default)]
    #[cfg_attr(feature = "ts", ts(as = "Option<String>", optional))]
    pub profile_reason: String,
    /// 201 §10 (P6b): `zoom` calls during the generation in force (the page
    /// faults that keep lines unmerged at the next trim).
    #[serde(default)]
    #[cfg_attr(feature = "ts", ts(as = "Option<u32>", optional))]
    pub zooms: u32,
    /// 201 §11 (P6b): a tool-list change waits for the next trim (new tools
    /// announced and callable through the runtime, removed ones refusing).
    #[serde(default)]
    #[cfg_attr(feature = "ts", ts(as = "Option<bool>", optional))]
    pub tools_pending: bool,
    /// 201 §9 (P6a): the state ledger's open entries per section, its
    /// suggestions and the frozen snapshot's bytes; `None` when the ledger is
    /// off, for a meeting or an epochs session.
    #[serde(default)]
    #[cfg_attr(feature = "ts", ts(optional = nullable))]
    pub ledger: Option<LedgerStatus>,
}

/// 201 §9 (P6a): [`SessionMemory::ledger`].
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize, schemars::JsonSchema)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS), ts(export))]
pub struct LedgerStatus {
    pub goals: u32,
    pub decisions: u32,
    pub constraints: u32,
    pub open_tasks: u32,
    pub artefacts: u32,
    /// Unconfirmed suggestions.
    pub suggested: u32,
    /// Bytes of the snapshot frozen with the current generation (0 before one).
    pub bytes: u32,
}

/// [`SessionMemory::profile`] when absent on the wire: the resolved profile is never `auto`.
fn standard_profile() -> Profile {
    Profile::Standard
}

/// Bus event on the session stream, ≤ 1 per 500 ms per session (§11.2).
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, schemars::JsonSchema)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS), ts(export))]
#[schemars(transform = wire::serde_type_tag::<Self>)]
#[serde(tag = "type")]
pub struct EndlessProgress {
    /// The session.
    pub session: SessionId,
    /// Nodes built.
    #[cfg_attr(feature = "ts", ts(type = "number"))]
    pub built: u64,
    /// History units to cover.
    #[cfg_attr(feature = "ts", ts(type = "number"))]
    pub total: u64,
    /// Some view part is still being built.
    pub settling: bool,
    /// First failure of a node (reported once per node).
    pub error: Option<String>,
    /// D14 progress while history is being covered.
    #[cfg_attr(feature = "ts", ts(optional = nullable))]
    pub backfill: Option<Backfill>,
    /// 137F: as [`SessionMemory::stalled`]; emitted whenever it appears, its count changes or it clears.
    #[serde(default)]
    #[cfg_attr(feature = "ts", ts(optional = nullable))]
    pub stalled: Option<Stalled>,
    /// 137G: as [`SessionMemory::backlog`].
    #[serde(default)]
    #[cfg_attr(feature = "ts", ts(optional = nullable))]
    pub backlog: Option<Backlog>,
}

impl wire::WireEvent for EndlessProgress {
    const TOPIC: &'static str = "agent.endless-progress";
}

/// Reply of [`Endless::set_mode`].
#[derive(Clone, Debug, PartialEq)]
pub struct SetMode {
    /// The state after the switch.
    pub memory: SessionMemory,
    /// `/endless off`: the `<context_summary epoch=n style="endless">` head
    /// text the caller writes as the next epoch head (8.6).
    pub head: Option<String>,
}

/// What `attach` registers for a live session.
#[derive(Clone)]
pub struct Attached {
    /// Compactor model id.
    pub model: models::Resolved,
    /// The session's reasoning effort.
    pub effort: Option<genai::chat::ReasoningEffort>,
    /// The agent name for the COMPACT prompt (`theuth` or the desk).
    pub agent: String,
    /// A private tree's own store; `None` = the engine's.
    pub store: Option<Store>,
}

pub(crate) struct Inner {
    pub(crate) store: Store,
    pub(crate) rt: Runtime,
    pub(crate) client: genai::Client,
    pub(crate) auth: crate::supervisor::Auth,
    /// The model catalog; replaced live by [`Endless::set_catalog`] (R1-16).
    pub(crate) catalog: RwLock<Arc<models::Catalog>>,
    /// `agent.endless` in force; replaced live by [`Endless::set_config`].
    pub(crate) cfg: Arc<RwLock<EndlessConfig>>,
    /// `agent.title_model` (R1-D3 chain); replaced live by [`Endless::set_title_model`].
    pub(crate) title_model: RwLock<Option<String>>,
    /// Live sessions and their pumps.
    pub(crate) sessions: Mutex<HashMap<SessionId, pump::Handle>>,
    /// 137F E9: one queue per local endpoint, engine-wide.
    pub(crate) queues: pump::Queues,
    /// Tests inject scripted compactors here, by resolved model id; `None` = the model (D8).
    pub(crate) compactor_for_tests: Option<pump::CompactorFactory>,
    /// 201 §4.2: desk sessions' per-agent tail keys (set at every desk open).
    pub(crate) agent_tails: Mutex<HashMap<SessionId, AgentTail>>,
    /// 201 (P5): chats whose next turn boundary trims down to the low mark
    /// (`endless_fold` / `/compact` while a turn ran).
    pub(crate) trim_asked: Mutex<std::collections::HashSet<SessionId>>,
    /// 201 §11 (P6b): each endless chat's tool part of the request prefix,
    /// frozen per generation (process-local; a restart rebuilds it cold).
    pub(crate) tool_pins: Mutex<HashMap<SessionId, tool_pin::ToolPin>>,
}

/// The one handle for the endless mode (137-contract.md §4). Cheap to clone.
#[derive(Clone)]
pub struct Endless {
    inner: Arc<Inner>,
}

impl Endless {
    /// Construct once per engine, beside the other side-call slots (the
    /// supervisor; `Auth` is crate-private). W2 wires the call site.
    #[cfg_attr(not(test), allow(dead_code))]
    pub(crate) fn new(
        store: Store,
        rt: Runtime,
        client: genai::Client,
        auth: crate::supervisor::Auth,
        catalog: Arc<models::Catalog>,
        cfg: EndlessConfig,
        title_model: Option<String>,
    ) -> Self {
        Self {
            inner: Arc::new(Inner {
                store,
                rt,
                client,
                auth,
                catalog: RwLock::new(catalog),
                cfg: Arc::new(RwLock::new(cfg)),
                title_model: RwLock::new(title_model),
                sessions: Mutex::new(HashMap::new()),
                queues: pump::Queues::default(),
                compactor_for_tests: None,
                agent_tails: Mutex::new(HashMap::new()),
                trim_asked: Mutex::new(std::collections::HashSet::new()),
                tool_pins: Mutex::new(HashMap::new()),
            }),
        }
    }

    /// The config in force.
    pub fn config(&self) -> EndlessConfig {
        self.inner.cfg.read().clone()
    }

    /// Settings › Memory: the new `agent.endless`, in force from now on —
    /// every pump reads the shared config; a changed view budget or slack
    /// re-folds at the next render (the fold cache is keyed by its parameters).
    /// A session whose compactor changes with it is re-kicked (137F E2).
    pub fn set_config(&self, cfg: EndlessConfig) {
        *self.inner.cfg.write() = cfg;
        pump::reresolve(&self.inner, None);
    }

    /// `SetTitleModel`: the R1-D3 chain's second link, live (137F E1).
    pub fn set_title_model(&self, model: Option<String>) {
        *self.inner.title_model.write() = model;
        pump::reresolve(&self.inner, None);
    }

    /// `SetModel` / `SetEffort` on a session: the chain's last link, live
    /// (137F E1). `model: None` keeps the session's model.
    pub fn set_session_model(
        &self,
        session: SessionId,
        model: Option<models::Resolved>,
        effort: Option<genai::chat::ReasoningEffort>,
    ) {
        if let Some(h) = self.inner.sessions.lock().get_mut(&session) {
            if let Some(m) = model {
                h.attached.model = m;
            }
            h.attached.effort = effort;
        }
        pump::reresolve(&self.inner, Some(session));
    }

    /// A provider was added, refreshed or removed, or a model's limits set
    /// (the catalog and the gateways changed): every session's model takes
    /// its new catalog entry (177 E6) and its compactor is re-resolved (R1-16).
    pub fn set_catalog(&self, catalog: Arc<models::Catalog>) {
        for h in self.inner.sessions.lock().values_mut() {
            if let Some((_, m)) = catalog.get(&h.attached.model.id) {
                h.attached.model.model = Some(m.clone());
            }
        }
        *self.inner.catalog.write() = catalog;
        pump::reresolve(&self.inner, None);
    }

    /// 177 E9: one real summary call for `model` on a fixed sample; writes no memory.
    pub async fn test_summary(&self, model: &str) -> crate::SummaryTest {
        pump::test_summary(&self.inner, model).await
    }

    /// `settle` returned early because the frontier keeps failing (R1-15).
    pub fn frontier_failing(&self, session: SessionId) -> bool {
        pump::frontier_failing(&self.inner, session)
    }

    /// Engine start (109): one pump per `sessions.memory = 'endless'` session.
    /// Returns how many pumps started.
    pub async fn resume_all(&self) -> anyhow::Result<usize> {
        pump::resume_all(&self.inner).await
    }

    /// Register a live session (idempotent); starts its pump when the mode is `endless`.
    pub async fn attach(&self, session: SessionId, attached: Attached) -> anyhow::Result<()> {
        pump::attach(&self.inner, session, attached).await
    }

    /// The actor stopped: stop the pump and drop in-memory state.
    pub fn detach(&self, session: SessionId) {
        pump::detach(&self.inner, session);
    }

    /// `sessions.memory` (a store read; works for unattached sessions).
    pub async fn mode(&self, session: SessionId) -> anyhow::Result<MemoryMode> {
        self.store_for(session).session_memory(session).await
    }

    /// The loop's switch (D14): attached ∧ `endless` ∧ backfill finished ∧
    /// filter ≠ `Meeting` (a meeting keeps head D + its verbatim tail; only the
    /// pump and `render_with_budget` serve it).
    pub fn active(&self, session: SessionId) -> bool {
        self.inner
            .sessions
            .lock()
            .get(&session)
            .is_some_and(pump::Handle::active)
    }

    /// §9: the session has a tree the three tools can read — attached ∧
    /// `endless`, any filter, backfill or not. A meeting session's toolset
    /// lists `zoom`/`recall`/`date`; this is their gate (W4 hand-off 3).
    pub fn served(&self, session: SessionId) -> bool {
        self.inner
            .sessions
            .lock()
            .get(&session)
            .is_some_and(pump::Handle::served)
    }

    /// 201 P1 (D5): returns `true` at once — every unit has a line from its
    /// ingest, so no turn waits on the summary model. Kept for callers and
    /// old configs (`settle_secs` is accepted and unused); it wakes the pump.
    pub async fn settle(&self, session: SessionId, max: Duration) -> bool {
        pump::settle(&self.inner, session, max).await
    }

    /// Test seam: wait, at most `max`, until the session's drafts are
    /// refined and its view built (the 137 `settle` wait).
    #[cfg(test)]
    pub(crate) async fn quiet_for_tests(&self, session: SessionId, max: Duration) -> bool {
        pump::quiet_for_tests(&self.inner, session, max).await
    }

    /// 8.1: the view for this turn; freezes it as the `<chat>` context of the
    /// units the turn commits (D6). Once per turn, after `settle`, before the
    /// input row is logged.
    pub async fn render_for_turn(&self, session: SessionId) -> anyhow::Result<ViewRender> {
        pump::render_for_turn(&self.inner, session, None, false).await
    }

    /// 137G G1/G5: [`Endless::render_for_turn`] scaled to at most
    /// `max_bytes` of view budget (`view_bytes + raw_bytes` shrink together);
    /// `again` re-renders the current turn's view (a refused request) instead
    /// of opening a new turn.
    pub async fn render_for_turn_within(
        &self,
        session: SessionId,
        max_bytes: usize,
        again: bool,
    ) -> anyhow::Result<ViewRender> {
        pump::render_for_turn(&self.inner, session, Some(max_bytes), again).await
    }

    /// 201 §3 (P2): the generation in force, `None` before the first.
    pub async fn generation(&self, session: SessionId) -> anyhow::Result<Option<pump::Generation>> {
        pump::generation(&self.inner, session).await
    }

    /// 201 §4.3 (P2): start (or, `again`, re-render smaller) a generation
    /// whose tail begins at message `first_seq` (`None`: an empty tail).
    pub async fn start_generation(
        &self,
        session: SessionId,
        first_seq: Option<i64>,
        max_bytes: usize,
        again: bool,
    ) -> anyhow::Result<pump::Generation> {
        pump::start_generation(&self.inner, session, first_seq, max_bytes, again).await
    }

    /// 201 §6 (P2): a turn of the current generation opens (compactor
    /// context, status numbers).
    pub fn turn_opened(&self, session: SessionId, tail_bytes: u32, allowance: u64) {
        pump::turn_opened(&self.inner, session, tail_bytes, allowance);
    }

    /// 201 §4.2 (P2): the session's tail marks `(low, high)` in bytes for a
    /// turn `allowance` — the session override over the config.
    pub fn tail_marks(&self, session: SessionId, allowance: u64) -> (u32, u32) {
        let tail = self
            .inner
            .sessions
            .lock()
            .get(&session)
            .and_then(pump::Handle::tail);
        pump::with_agent_tail(&self.inner, session, &self.config())
            .with_tail(tail.as_ref())
            .tail_marks(allowance)
    }

    /// 201 §4.2: the desk agent's tail keys for `session` (all `None` clears).
    pub fn set_agent_tail(&self, session: SessionId, tail: AgentTail) {
        {
            let mut m = self.inner.agent_tails.lock();
            if tail == AgentTail::default() {
                m.remove(&session);
            } else {
                m.insert(session, tail);
            }
        }
        pump::set_agent_profile(&self.inner, session, tail.profile);
    }

    /// 8.3 / 201 §6 "wake on append": rows reached `messages`; the session's
    /// pump wakes, derives the units and refines their drafts during the turn.
    pub fn on_rows_committed(&self, session: SessionId) {
        pump::nudge(&self.inner, session);
    }

    /// R1-D2 / R1b: the session's view budget (a user control, persisted in
    /// `sessions.endless_view_bytes`; clamped 16_000..=256_000 at use). The
    /// fold re-fits at the next render: a smaller budget merges down, a
    /// larger one re-folds at the new budget (never cuts text).
    pub async fn set_view_bytes(&self, session: SessionId, bytes: u32) -> anyhow::Result<()> {
        pump::set_view_bytes(&self.inner, session, Some(bytes)).await
    }

    /// 201 §4.2: the session's tail override (persisted in
    /// `sessions.endless_tail_mode/low/high`); `None` clears it. Validated by
    /// the caller ([`TailOverride::validate`]). Nothing reads it for the
    /// frame until phase P2.
    pub async fn set_tail(
        &self,
        session: SessionId,
        tail: Option<TailOverride>,
    ) -> anyhow::Result<()> {
        pump::set_tail(&self.inner, session, tail).await
    }

    /// R1-D1: the same render as the turn's, without freezing a context —
    /// a native child's first message.
    pub async fn render_snapshot(&self, session: SessionId) -> anyhow::Result<Vec<ViewBlock>> {
        pump::render_snapshot(&self.inner, session).await
    }

    /// R1-D6: the view re-folded to `bytes`, as lines (a meeting head's
    /// `## Earlier in this meeting`); works for a `UnitFilter::Meeting`
    /// session, whose `active()` is false.
    pub async fn render_with_budget(
        &self,
        session: SessionId,
        bytes: u32,
    ) -> anyhow::Result<Vec<ViewLine>> {
        pump::render_with_budget(&self.inner, session, bytes).await
    }

    /// R1-D4: ETA and cost of switching the session on (or of the backfill under way).
    pub async fn backfill_estimate(&self, session: SessionId) -> anyhow::Result<Backfill> {
        pump::backfill_estimate(&self.inner, session).await
    }

    /// `zoom(id, n)` (§9).
    pub async fn zoom(&self, session: SessionId, id: u64, n: u64) -> Result<Vec<ViewLine>, String> {
        pump::zoom(&self.inner, session, id, n).await
    }

    /// 201 §8 (P4): `zoom(id, 1)` on an artefact unit: its line and its
    /// binary re-read from the row (`Err(line)`: no longer stored); `None`
    /// for any other unit.
    pub async fn artefact(
        &self,
        session: SessionId,
        id: u64,
    ) -> Option<Result<(String, genai::chat::Binary), String>> {
        pump::artefact(&self.inner, session, id).await
    }

    /// 201 §8 (P4): a trim moved rows `[from_seq, to_seq)` out of the tail:
    /// describe their images in the background ([`pump::describe_left`]).
    pub fn describe_left(
        &self,
        session: SessionId,
        from_seq: i64,
        to_seq: i64,
        vision: Option<models::Resolved>,
    ) -> Option<tokio::task::JoinHandle<()>> {
        pump::describe_left(&self.inner, session, from_seq, to_seq, vision)
    }

    /// `recall(query, limit)` (§9).
    pub async fn recall(
        &self,
        session: SessionId,
        query: &str,
        limit: u8,
    ) -> Result<Vec<ViewLine>, String> {
        pump::recall(&self.inner, session, query, limit).await
    }

    /// `date(id)` (§9): the unit's row time.
    pub async fn date(&self, session: SessionId, id: u64) -> Result<Option<DateTime<Utc>>, String> {
        pump::date(&self.inner, session, id).await
    }

    /// `/endless on|off` (8.6); the caller guarantees the session is idle. A
    /// session with a transcript backfills under R1-D4; `filter` (R1-D6) and
    /// the base are fixed at switch-on.
    pub async fn set_mode(
        &self,
        session: SessionId,
        mode: MemoryMode,
        epoch: u32,
        filter: UnitFilter,
    ) -> anyhow::Result<SetMode> {
        pump::set_mode(&self.inner, session, mode, filter, epoch, None).await
    }

    /// [`Endless::set_mode`] whose `/endless off` head is scaled to at most
    /// `max_bytes` of view budget (the session model's turn allowance, R1-7).
    pub async fn set_mode_within(
        &self,
        session: SessionId,
        mode: MemoryMode,
        epoch: u32,
        filter: UnitFilter,
        max_bytes: usize,
    ) -> anyhow::Result<SetMode> {
        pump::set_mode(&self.inner, session, mode, filter, epoch, Some(max_bytes)).await
    }

    /// RPC `session_memory` body data.
    pub async fn memory(&self, session: SessionId) -> anyhow::Result<SessionMemory> {
        pump::memory(&self.inner, session).await
    }

    /// RPC `endless_retry` (137F): reset every failed node's backoff, wake
    /// the pump, return the fresh state. An epochs session is untouched.
    pub async fn retry(&self, session: SessionId) -> anyhow::Result<SessionMemory> {
        pump::retry(&self.inner, session).await
    }

    /// RPC `endless_fold` (137G): every unbuilt level-0 unit becomes a
    /// mechanical excerpt node at once (no model call); the pump merges on.
    /// An epochs session is untouched.
    pub async fn fold(&self, session: SessionId) -> anyhow::Result<SessionMemory> {
        pump::fold_all(&self.inner, session).await
    }

    /// 201 (P5): `endless_fold` / `/compact` on a busy chat: its next turn
    /// boundary trims the tail down to the low mark, under the high mark too.
    pub fn request_trim(&self, session: SessionId) {
        self.inner.trim_asked.lock().insert(session);
    }

    /// 201 (P5): take the [`Endless::request_trim`] mark (once).
    pub fn take_trim_request(&self, session: SessionId) -> bool {
        self.inner.trim_asked.lock().remove(&session)
    }

    /// 201 §11 (P6b): the session's frozen tool part (a copy).
    pub fn tool_pin(&self, session: SessionId) -> Option<tool_pin::ToolPin> {
        self.inner.tool_pins.lock().get(&session).cloned()
    }

    /// 201 §11 (P6b): store (or, `None`, drop) the session's frozen tool part.
    pub fn set_tool_pin(&self, session: SessionId, pin: Option<tool_pin::ToolPin>) {
        let mut pins = self.inner.tool_pins.lock();
        match pin {
            Some(p) => pins.insert(session, p),
            None => pins.remove(&session),
        };
    }

    /// 201 §11 (P6b): the refusal for a call of a tool removed since the
    /// session's tool part was frozen; `None` for any other.
    pub fn tool_refusal(&self, session: SessionId, name: &str) -> Option<String> {
        self.inner.tool_pins.lock().get(&session)?.refusal(name)
    }

    /// 201 §10 (P6b): the `<recall>` prefetch body for a new input — the
    /// best `recall` hits over its rare words whose units are not in the
    /// tail, up to `prefetch_bytes`; `None` when off, in a meeting, before a
    /// generation, or without a hit.
    pub async fn prefetch(&self, session: SessionId, input: &str) -> Option<String> {
        let bytes = self.inner.cfg.read().clone().clamped().prefetch_bytes as usize;
        pump::prefetch(&self.inner, session, input, bytes).await
    }

    /// The session's tree is meeting-filtered (R1-D6): `endless_fold` keeps
    /// its 137G meaning there.
    pub fn meeting(&self, session: SessionId) -> bool {
        self.inner
            .sessions
            .lock()
            .get(&session)
            .is_some_and(pump::Handle::meeting)
    }

    /// 201 §9 (P6a): whether the `ledger` tool is listed for `session` — the
    /// current generation's switch (frozen at its trim), else the config's
    /// (the first generation will take it); never for a meeting.
    pub async fn ledger_listed(&self, session: SessionId) -> bool {
        pump::ledger_listed(&self.inner, session).await
    }

    /// 201 §9 (P6a): one `ledger` tool call; the change note or a refusal.
    pub async fn ledger_op(&self, session: SessionId, op: &ledger::Op) -> Result<String, String> {
        pump::ledger_op(&self.inner, session, op).await
    }

    /// 201 §9 (P6a): the ledger changes made since the last turn's input,
    /// as one `<ledger>` note body (taken: each is sent once).
    pub async fn ledger_notes(&self, session: SessionId) -> Option<String> {
        pump::ledger_notes(&self.inner, session).await
    }

    /// RPC `endless_pause` (137G): `true` stops new compactor calls and
    /// cancels those in flight (no failures), keeping the backlog; `false`
    /// resumes. Persisted in `sessions.endless_paused`.
    pub async fn pause(&self, session: SessionId, paused: bool) -> anyhow::Result<SessionMemory> {
        pump::pause(&self.inner, session, paused).await
    }

    /// RPC `set_session_compactor_model` (137G): this chat's own compactor
    /// (`None` clears = inherit); refused when the id does not resolve;
    /// persisted in `sessions.endless_model`; live (re-resolve + kick, 137F).
    /// 194: the chat's own effort goes with a cleared model, and with a new
    /// one that does not take it.
    pub async fn set_model(
        &self,
        session: SessionId,
        model: Option<String>,
    ) -> anyhow::Result<SessionMemory> {
        pump::set_model(&self.inner, session, model, false, None).await
    }

    /// RPC `set_session_compactor_effort` (194): this chat's own compactor
    /// effort (`None` clears); refused when the model in force does not take
    /// it; persisted in `sessions.endless_effort`; live.
    pub async fn set_effort(
        &self,
        session: SessionId,
        effort: Option<String>,
    ) -> anyhow::Result<SessionMemory> {
        pump::set_effort(&self.inner, session, effort).await
    }

    /// The roster at desk open (137G): the agent's `memory_model` default
    /// for this desk session (`None` clears) with its `memory_effort` (194;
    /// cleared with the model); persisted, live.
    pub async fn set_agent_model(
        &self,
        session: SessionId,
        model: Option<String>,
        effort: Option<String>,
    ) -> anyhow::Result<SessionMemory> {
        pump::set_model(&self.inner, session, model, true, effort).await
    }

    /// The constant VIEW_DOC appended to the system prompt in endless sessions (§9).
    pub fn view_doc() -> &'static str {
        VIEW_DOC
    }

    fn store_for(&self, session: SessionId) -> Store {
        self.inner.store_for(session)
    }
}

impl Inner {
    /// The session's store: a private tree's own, else the engine's.
    pub(crate) fn store_for(&self, session: SessionId) -> Store {
        self.sessions
            .lock()
            .get(&session)
            .and_then(|h| h.attached.store.clone())
            .unwrap_or_else(|| self.store.clone())
    }
}

/// §9 VIEW_DOC; 201 §4.4: the messages after the view are word for word.
pub const VIEW_DOC: &str = "The view: the session before the messages that follow it, oldest first,
inside <chat> tags, as one-line summaries. Each line is  id+n|text  — the n
messages from id on, summarized (newlines shown as spaces). A summary tags
each item with its kind: user, talk, tool, echo, work, note. A short message
is its own line, word for word. Recent lines cover one message each; the
older the messages, the more a line covers. The messages after the view are
the latest ones, word for word, as they were sent; the lines under <chat>
summarise everything before them.
Navigating: zoom(id, n) opens line id+n into the two lines it was made from;
zoom(id, 1) gives message id in full. recall(words) searches every message's
summary line. Zoom or recall whenever a summary only mentions something you
need — a decision, a past attempt, an exact error, where a file is — before
you act, guess or ask. date(id) gives the date and time of message id. Older
messages leave the word-for-word part as the session grows, so say in your
reply what you learned that will matter later.";
