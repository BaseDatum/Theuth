//! Page issues (design doc 122 D1): why a page did not read as content and
//! which escalation rung comes next. Contract stub — `detect` says `None`
//! until W1 lands the marker table.

use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

/// What kind of wall or failure the page is.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum IssueKind {
    /// Bot wall: Cloudflare challenge, Turnstile, hCaptcha, AWS WAF, DataDome, Akamai, or a bare 401/403.
    AccessChallenge,
    /// HTTP 429.
    RateLimited,
    /// A sign-in page.
    LoginRequired,
    /// A cookie / consent interstitial.
    ConsentWall,
    /// A 503 or "taking longer than usual" error page.
    ErrorPage,
    /// An SPA shell with no readable prose.
    EmptyShell,
    /// A body the fetcher refuses to convert (binary, oversize container).
    Unsupported,
}

/// The escalation rung the model should take next.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum Next {
    /// Call again after `retry_after`.
    Retry,
    /// `rendered: true` (the browser tier).
    Rendered,
    /// `browser_import_cookies`.
    ImportCookies,
    /// `desktop_handoff`.
    Handoff,
}

/// A diagnosed page issue.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct PageIssue {
    /// What the page is.
    pub kind: IssueKind,
    /// Human-readable cause: `"HTTP 403 · cloudflare challenge"`, `"consent url"`, `"login title"`.
    pub detail: String,
    /// From `Retry-After` on 429/503; serialised as whole seconds.
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        with = "duration_secs"
    )]
    #[schemars(with = "Option<u64>")]
    pub retry_after: Option<std::time::Duration>,
    /// Rungs to try, in order.
    pub next: Vec<Next>,
}

/// `Option<Duration>` as integer seconds on the wire.
mod duration_secs {
    use std::time::Duration;

    use serde::{Deserialize, Deserializer, Serialize, Serializer};

    pub fn serialize<S: Serializer>(d: &Option<Duration>, s: S) -> Result<S::Ok, S::Error> {
        d.map(|d| d.as_secs()).serialize(s)
    }

    pub fn deserialize<'de, D: Deserializer<'de>>(d: D) -> Result<Option<Duration>, D::Error> {
        Ok(Option::<u64>::deserialize(d)?.map(Duration::from_secs))
    }
}

/// Diagnose a fetched page. `prose_chars` is `excerpt::prose_chars(markdown)`
/// of the converted body; word thresholds use `CHARS_PER_WORD = 6`.
///
/// Contract stub (122 §7.1): always `None`; W1 lands the D1 marker table.
#[must_use]
pub fn detect(
    _status: u16,
    _headers: &reqwest::header::HeaderMap,
    _final_url: &url::Url,
    _body: &str,
    _prose_chars: usize,
) -> Option<PageIssue> {
    None
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn retry_after_serialises_as_seconds() {
        let issue = PageIssue {
            kind: IssueKind::RateLimited,
            detail: "HTTP 429".into(),
            retry_after: Some(std::time::Duration::from_secs(120)),
            next: vec![Next::Retry],
        };
        let v = serde_json::to_value(&issue).unwrap();
        assert_eq!(
            v,
            serde_json::json!({"kind": "rate_limited", "detail": "HTTP 429", "retry_after": 120, "next": ["retry"]})
        );
        let back: PageIssue = serde_json::from_value(v).unwrap();
        assert_eq!(back, issue);
        let none: PageIssue =
            serde_json::from_str(r#"{"kind":"consent_wall","detail":"consent url","next":["rendered","import_cookies"]}"#)
                .unwrap();
        assert_eq!(none.retry_after, None);
        assert!(
            !serde_json::to_string(&none)
                .unwrap()
                .contains("retry_after")
        );
    }
}
