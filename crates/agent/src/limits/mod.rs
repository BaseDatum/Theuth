//! Usage limits (160 §5.2) — ENG's neutral stub: the frozen types and a hub
//! that reports nothing. LIM's tree replaces this file whole at merge.

use std::sync::LazyLock;

use serde::{Deserialize, Serialize};

/// Health of one provider's limits.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize, schemars::JsonSchema)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS), ts(export))]
#[serde(rename_all = "snake_case")]
pub enum LimitStatus {
    /// Within limits.
    Ok,
    /// Close to a limit.
    Warn,
    /// At a limit.
    Limited,
    /// The newest observation is old.
    Stale,
    /// The provider exposes nothing.
    Unavailable,
    /// No credential.
    NotConfigured,
    /// A hard failure.
    Error,
}

/// A subscription usage window (Codex primary/secondary, Claude 5-hour/weekly…).
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, schemars::JsonSchema)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS), ts(export))]
pub struct UsageWindow {
    /// `primary` | `secondary` | `five_hour` | `seven_day` | `seven_day_opus` | …
    pub id: String,
    /// `5-hour` | `Weekly` | `Weekly · Opus`.
    pub label: String,
    /// 0.0 = unused, 1.0 = exhausted (percent / 100); may exceed 1.0.
    pub used_fraction: f64,
    /// Reset time.
    #[cfg_attr(feature = "ts", ts(type = "number | null"))]
    pub resets_at_ms: Option<u64>,
    /// Window length.
    #[cfg_attr(feature = "ts", ts(type = "number | null"))]
    pub window_secs: Option<u64>,
}

/// A per-minute API rate-limit bucket observed in response headers.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, schemars::JsonSchema)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS), ts(export))]
pub struct RateLimit {
    /// `requests` | `tokens` | `input_tokens` | `output_tokens` | `project_tokens`.
    pub id: String,
    /// `Requests / min`.
    pub label: String,
    /// Model class the bucket applies to when the provider scopes it (Anthropic: per model group).
    pub scope: Option<String>,
    /// Bucket size.
    #[cfg_attr(feature = "ts", ts(type = "number"))]
    pub limit: u64,
    /// Left in the bucket.
    #[cfg_attr(feature = "ts", ts(type = "number"))]
    pub remaining: u64,
    /// Reset time.
    #[cfg_attr(feature = "ts", ts(type = "number | null"))]
    pub resets_at_ms: Option<u64>,
}

/// Credits / key limit / prepaid balance.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, schemars::JsonSchema)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS), ts(export))]
pub struct Balance {
    /// Display label.
    pub label: String,
    /// `USD` or `credits`.
    pub currency: String,
    /// Left.
    pub remaining: Option<f64>,
    /// Cap.
    pub limit: Option<f64>,
    /// Used.
    pub used: Option<f64>,
}

/// One provider's limits snapshot.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, schemars::JsonSchema)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS), ts(export))]
pub struct ProviderLimits {
    /// Stable row key, e.g. "anthropic:api_key", "openai:oauth", "amazon-bedrock".
    pub id: String,
    /// Provider id.
    pub provider: String,
    /// "Anthropic API", "ChatGPT · Codex", "Amazon Bedrock".
    pub label: String,
    /// "api_key" | "oauth" | "aws" | "gateway" | "login".
    pub auth: String,
    /// Truncated account id / email exactly as the source states it — never a credential.
    pub account: Option<String>,
    /// Plan name.
    pub plan: Option<String>,
    /// Health.
    pub status: LimitStatus,
    /// Human-readable detail.
    pub message: Option<String>,
    /// Subscription windows.
    pub windows: Vec<UsageWindow>,
    /// Rate-limit buckets.
    pub rates: Vec<RateLimit>,
    /// Credits / balance.
    pub balance: Option<Balance>,
    /// "response headers" | "usage endpoint" | "headers + usage endpoint" | "none".
    pub source: String,
    /// Newest observation.
    #[cfg_attr(feature = "ts", ts(type = "number | null"))]
    pub observed_at_ms: Option<u64>,
}

/// A theuth budget cap.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, schemars::JsonSchema)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS), ts(export))]
pub struct BudgetLimit {
    /// "agent:<name>" | "routine:<name>" | "session:<id>".
    pub id: String,
    /// Display label.
    pub label: String,
    /// "month" | "night" | "run".
    pub period: String,
    /// Cap in dollars.
    pub cap_usd: f64,
    /// Spent in dollars.
    pub spent_usd: f64,
    /// Reset time.
    #[cfg_attr(feature = "ts", ts(type = "number | null"))]
    pub resets_at_ms: Option<u64>,
}

/// `usage_limits()` answer.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, schemars::JsonSchema)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS), ts(export))]
pub struct UsageLimits {
    /// Provider rows.
    pub providers: Vec<ProviderLimits>,
    /// Budget rows.
    pub budgets: Vec<BudgetLimit>,
    /// Snapshot time.
    #[cfg_attr(feature = "ts", ts(type = "number"))]
    pub generated_at_ms: u64,
}

/// The engine-wide limits cache. Cheap to clone (Arc inside).
#[derive(Clone, Debug, Default)]
pub struct LimitsHub {}

impl LimitsHub {
    /// The process-wide hub. Production code uses only this one; tests build their own with `default()`.
    pub fn global() -> &'static LimitsHub {
        static HUB: LazyLock<LimitsHub> = LazyLock::new(LimitsHub::default);
        &HUB
    }

    /// The current snapshot, from memory (microseconds; never touches the network).
    pub fn snapshot(&self) -> UsageLimits {
        UsageLimits {
            providers: Vec::new(),
            budgets: Vec::new(),
            generated_at_ms: u64::try_from(chrono::Utc::now().timestamp_millis()).unwrap_or(0),
        }
    }

    /// One response's headers (lower-case names) from a model call to `provider` with `auth`,
    /// for `model` (scope derivation). Called on every response, success or failure.
    pub fn observe_headers(
        &self,
        _provider: &str,
        _auth: &str,
        _model: &str,
        _status: u16,
        _headers: &[(String, String)],
        _now_ms: u64,
    ) {
    }

    /// Ask the pollers to refresh soon (debounced per source; returns immediately).
    pub fn kick(&self, _provider: Option<&str>) {}
}
