//! Per-provider health from the last calls (design doc 16 §4.6): what
//! Settings › Search shows as "Status" and what `search_web` folds into
//! `degraded` / `reasons`. Shared across searches like [`crate::aggregate::Drift`].

use std::collections::HashMap;
use std::sync::Mutex;

use serde::{Deserialize, Serialize};

use crate::aggregate::FanoutOutcome;

/// Wire form of one provider's last known state.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize, schemars::JsonSchema)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS), ts(export))]
pub struct ProviderStatus {
    /// Provider name (`ProviderMeta::name`).
    pub name: String,
    /// Registered from `search-providers` (keyed or self-hosted), not a built-in engine.
    pub external: bool,
    /// Whether the provider answers searches at all (render-only ones do not).
    pub searches: bool,
    /// Last call succeeded; `None` when never called.
    pub ok: Option<bool>,
    /// Failure class of the last error: `rate_limited`, `captcha`, `challenge`,
    /// `blocked`, `parse_error`, `timeout`, `unauthorized`, `http`; empty when ok.
    pub kind: String,
    /// Human detail: `"12 rows"` or the error text.
    pub detail: String,
    /// RFC 3339 of the last successful call.
    pub last_ok_at: Option<String>,
    /// RFC 3339 of the last failed call.
    pub last_error_at: Option<String>,
}

/// Classify a failure text the aggregator produced (`ProviderError`'s
/// `Display` or the drift message) into a stable kind.
#[must_use]
pub fn error_kind(detail: &str) -> &'static str {
    let d = detail.to_ascii_lowercase();
    if d.contains("rate limit") || d.contains("429") || d.contains("too many requests") {
        "rate_limited"
    } else if d.contains("captcha") {
        "captcha"
    } else if d.contains("challenge") || d.contains("202") {
        "challenge"
    } else if d.starts_with("blocked") || d.contains("403") || d.contains("consent") {
        "blocked"
    } else if d.starts_with("bad response") || d.contains("shape may have changed") {
        "parse_error"
    } else if d.contains("timed out") {
        "timeout"
    } else if d.starts_with("unauthorized") || d.contains("401") {
        "unauthorized"
    } else {
        "http"
    }
}

/// Last outcome per provider name.
#[derive(Debug, Default)]
pub struct Health(Mutex<HashMap<String, ProviderStatus>>);

impl Health {
    /// Record one fan-out.
    pub fn observe(&self, outcome: &FanoutOutcome) {
        let Ok(mut map) = self.0.lock() else {
            return;
        };
        let now = chrono::Utc::now().to_rfc3339_opts(chrono::SecondsFormat::Secs, true);
        for name in &outcome.ok {
            let rows = outcome
                .hits
                .iter()
                .find(|(n, _)| n == name)
                .map_or(0, |(_, h)| h.len());
            let s = map.entry(name.clone()).or_default();
            s.name.clone_from(name);
            s.ok = Some(true);
            s.kind.clear();
            s.detail = format!("{rows} rows");
            s.last_ok_at = Some(now.clone());
        }
        for (name, why) in &outcome.failed {
            let s = map.entry(name.clone()).or_default();
            s.name.clone_from(name);
            s.ok = Some(false);
            s.kind = error_kind(why).to_owned();
            s.detail.clone_from(why);
            s.last_error_at = Some(now.clone());
        }
    }

    /// Snapshot for `name`, if it was ever called.
    #[must_use]
    pub fn get(&self, name: &str) -> Option<ProviderStatus> {
        self.0.lock().ok()?.get(name).cloned()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn kinds_from_error_text() {
        assert_eq!(error_kind("rate limited"), "rate_limited");
        assert_eq!(error_kind("blocked: b_captcha"), "captcha");
        assert_eq!(error_kind("blocked: HTTP 202 challenge"), "challenge");
        assert_eq!(error_kind("blocked: HTTP 403"), "blocked");
        assert_eq!(
            error_kind("no results; response shape may have changed"),
            "parse_error"
        );
        assert_eq!(error_kind("bad response: missing field"), "parse_error");
        assert_eq!(error_kind("timed out after 4000ms"), "timeout");
        assert_eq!(
            error_kind("unauthorized: check the provider's secret"),
            "unauthorized"
        );
        assert_eq!(error_kind("HTTP 502"), "http");
    }

    #[test]
    fn observe_keeps_last_ok_and_last_error() {
        let h = Health::default();
        let mut o = FanoutOutcome::default();
        o.ok.push("google".into());
        o.hits.push(("google".into(), vec![Default::default(); 3]));
        o.failed.push(("brave".into(), "rate limited".into()));
        h.observe(&o);
        let g = h.get("google").unwrap();
        assert_eq!((g.ok, g.detail.as_str()), (Some(true), "3 rows"));
        assert!(g.last_ok_at.is_some() && g.last_error_at.is_none());
        let b = h.get("brave").unwrap();
        assert_eq!((b.ok, b.kind.as_str()), (Some(false), "rate_limited"));
        assert!(h.get("qwant").is_none());
    }
}
