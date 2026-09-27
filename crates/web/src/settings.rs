//! Settings › Search, the `web` half (design doc 16 §8): the live search
//! defaults as a wire struct, the messages that read / persist / apply them
//! on the [`crate::actor::WebActor`], and a one-provider probe.

use serde::{Deserialize, Serialize};

use crate::WebConfig;
use crate::provider::Recency;

/// Search defaults the user edits; a projection of [`WebConfig`].
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, schemars::JsonSchema, kameo::Reply)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS), ts(export))]
pub struct SearchDefaults {
    /// Hits `search_web` reads when the call passes no `read`, 0..=8.
    pub read_top: usize,
    /// Whole read-phase deadline in milliseconds.
    pub read_budget_ms: u64,
    /// Cap on one browser-rendered read in milliseconds.
    pub render_cap_ms: u64,
    /// `recent` used when the call passes none.
    pub recent_default: Option<Recency>,
    /// Keyless engines run alongside external providers (`true`) or only
    /// when none serves the category (`false`).
    pub keyless_fallback: bool,
    /// A degraded search is an error result.
    pub degraded_is_error: bool,
}

impl From<&WebConfig> for SearchDefaults {
    fn from(c: &WebConfig) -> Self {
        Self {
            read_top: c.read_top,
            read_budget_ms: c.read_budget_ms,
            render_cap_ms: c.render_cap_ms,
            recent_default: c.recent_default,
            keyless_fallback: c.keyless_fallback,
            degraded_is_error: c.degraded_is_error,
        }
    }
}

impl SearchDefaults {
    /// Bounds a client may not exceed; `Err` names the field.
    pub fn validate(&self) -> Result<(), String> {
        if self.read_top > crate::tools::MAX_READ {
            return Err(format!("read_top must be 0..={}", crate::tools::MAX_READ));
        }
        if !(500..=120_000).contains(&self.read_budget_ms) {
            return Err("read_budget_ms must be 500..=120000".into());
        }
        if !(500..=60_000).contains(&self.render_cap_ms) {
            return Err("render_cap_ms must be 500..=60000".into());
        }
        Ok(())
    }

    /// Write into `cfg`.
    pub fn apply(&self, cfg: &mut WebConfig) {
        cfg.read_top = self.read_top;
        cfg.read_budget_ms = self.read_budget_ms;
        cfg.render_cap_ms = self.render_cap_ms;
        cfg.recent_default = self.recent_default;
        cfg.keyless_fallback = self.keyless_fallback;
        cfg.degraded_is_error = self.degraded_is_error;
    }

    /// `(key, yaml scalar)` pairs for the `web:` section.
    #[must_use]
    pub fn yaml_pairs(&self) -> Vec<(&'static str, String)> {
        vec![
            ("read_top", self.read_top.to_string()),
            ("read_budget_ms", self.read_budget_ms.to_string()),
            ("render_cap_ms", self.render_cap_ms.to_string()),
            (
                "recent_default",
                self.recent_default
                    .map_or_else(|| "null".to_owned(), |r| crate::aggregate::recency_name(r).to_owned()),
            ),
            ("keyless_fallback", self.keyless_fallback.to_string()),
            ("degraded_is_error", self.degraded_is_error.to_string()),
        ]
    }
}

/// Outcome of one probe query through one provider (Settings › Search "Test").
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize, schemars::JsonSchema)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS), ts(export))]
pub struct ProviderProbe {
    /// Provider name.
    pub name: String,
    /// The provider answered.
    pub ok: bool,
    /// Wall time in milliseconds.
    pub took_ms: u64,
    /// Rows it returned.
    pub rows: usize,
    /// Error text (or the failure kind) when not ok; first hit title when ok.
    pub detail: String,
}

/// `ask` → [`SearchDefaults`] from the live config.
#[derive(Clone, Copy, Debug, Default)]
pub struct GetDefaults;

/// `ask` → `Result<SearchDefaults, String>`: validate, persist to the `web:`
/// section of config.yaml, then apply live. A failed write changes nothing.
#[derive(Clone, Debug)]
pub struct SetDefaults(pub SearchDefaults);

/// `ask` → `Vec<ProviderStatus>` for every registered provider, registration order.
#[derive(Clone, Copy, Debug, Default)]
pub struct Status;

/// `ask` → `Result<ProviderProbe, String>`: one general query through the
/// named provider under the per-provider timeout; `Err` when unknown.
#[derive(Clone, Debug)]
pub struct Probe {
    /// Registered provider name.
    pub name: String,
    /// Query text.
    pub query: String,
}
