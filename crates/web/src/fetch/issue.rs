//! Page issues (design doc 122 D1, revision 1 R1-D2): why a page did not
//! read as content and which escalation rung comes next. `detect` runs the
//! marker table top to bottom; the first row that matches wins. Markers are
//! exact substrings with size gates so a benign page that embeds a widget
//! (a Turnstile form inside a long article, github.com's beacon) is not an
//! issue.

use std::time::Duration;

use reqwest::header::HeaderMap;
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use url::Url;

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
    /// Cause: `HTTP <status> · ` when not 2xx, then the fixed vocabulary
    /// (`cloudflare`, `cloudflare:turnstile`, `cloudflare:1020`, `akamai`,
    /// `datadome`, `aws-waf`, `hcaptcha`, `perimeterx`, `imperva`, `recaptcha`,
    /// `kasada`, `antibot-title`, `consent:<platform|url|title>`, `login`,
    /// `error-page`, `empty-shell`); a bare 401/403/429 is `HTTP <status>`.
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

/// Word thresholds of D1 are given in words; the signature carries chars.
pub const CHARS_PER_WORD: usize = 6;

const ANTIBOT_TITLES: &[&str] = &[
    "just a moment",
    "attention required",
    "access denied",
    "checking your browser",
    "please wait",
    "one more step",
    "verify you are human",
    "bot verification",
    "security check",
    "ddos protection",
];

const CONSENT_TITLES: &[&str] = &[
    "before you continue",
    "your privacy choices",
    "we value your privacy",
    "we care about your privacy",
    "cookie consent",
    "consent required",
];

const CONSENT_URLS: &[&str] = &[
    "://consent.",
    "/consent?",
    "/consent/",
    "collectconsent",
    "consentcheck",
    "/cmp/",
    "guce.advertising.com",
];

/// Consent platforms named in `detail` as `consent:<platform>` when their
/// markup is in the body.
const CONSENT_PLATFORMS: &[&str] = &[
    "onetrust",
    "cookiebot",
    "trustarc",
    "didomi",
    "quantcast",
    "sourcepoint",
    "usercentrics",
    "evidon",
];

const LOGIN_TITLES: &[&str] = &["sign in", "log in", "login"];

const SPA_MARKERS: &[&str] = &[
    "id=\"__next\"",
    "id=\"root\"",
    "id=\"app\"",
    "__next_data__",
    "nuxt",
    "ng-app",
    "react-app",
];

impl IssueKind {
    /// The fixed `next` table of D1.
    #[must_use]
    pub fn next(self) -> Vec<Next> {
        match self {
            Self::AccessChallenge => vec![Next::Rendered, Next::ImportCookies, Next::Handoff],
            Self::RateLimited | Self::ErrorPage => vec![Next::Retry],
            Self::LoginRequired => vec![Next::ImportCookies, Next::Handoff],
            Self::ConsentWall => vec![Next::Rendered, Next::ImportCookies],
            Self::EmptyShell => vec![Next::Rendered],
            Self::Unsupported => Vec::new(),
        }
    }
}

/// `Retry-After` as a duration: integer seconds or an HTTP-date (RFC 7231);
/// negative or unparsable → `None`.
#[must_use]
pub fn retry_after(headers: &HeaderMap) -> Option<Duration> {
    let v = headers
        .get(reqwest::header::RETRY_AFTER)?
        .to_str()
        .ok()?
        .trim();
    if let Ok(secs) = v.parse::<i64>() {
        return u64::try_from(secs).ok().map(Duration::from_secs);
    }
    let at = chrono::DateTime::parse_from_rfc2822(v).ok()?;
    (at.with_timezone(&chrono::Utc) - chrono::Utc::now())
        .to_std()
        .ok()
}

/// Diagnose a fetched page. `prose_chars` is `excerpt::prose_chars(markdown)`
/// of the converted body; word thresholds use [`CHARS_PER_WORD`]. `body` is
/// the decoded response body (HTML, not markdown): the markers are markup.
#[must_use]
pub fn detect(
    status: u16,
    headers: &HeaderMap,
    final_url: &Url,
    body: &str,
    prose_chars: usize,
) -> Option<PageIssue> {
    let lower = body.to_ascii_lowercase();
    let size = body.len();
    let title = title_of(&lower);
    let title_starts = |list: &[&str]| list.iter().any(|t| title.starts_with(t));
    let has = |m: &str| lower.contains(m);
    let words = |n: usize| n * CHARS_PER_WORD;
    let ok = (200..300).contains(&status);
    let issue = |kind: IssueKind, vocab: &str| {
        let detail = match (ok, vocab.is_empty()) {
            (true, _) => vocab.to_owned(),
            (false, true) => format!("HTTP {status}"),
            (false, false) => format!("HTTP {status} · {vocab}"),
        };
        Some(PageIssue {
            kind,
            detail,
            retry_after: matches!(status, 429 | 503)
                .then(|| retry_after(headers))
                .flatten(),
            next: kind.next(),
        })
    };
    let challenge = |vocab: &str| issue(IssueKind::AccessChallenge, vocab);

    // 1–7d: vendor markers.
    if headers.contains_key("cf-mitigated") {
        return challenge("cloudflare");
    }
    if has("_cf_chl_opt")
        || has("challenge-platform/h/b/orchestrate/")
        || has("challenge-platform/h/g/orchestrate/")
    {
        return challenge("cloudflare");
    }
    if size < 100_000 && (has("cf-turnstile") || has("challenges.cloudflare.com/turnstile")) {
        return challenge("cloudflare:turnstile");
    }
    if size < 50_000 && has("hcaptcha.com") && has("h-captcha") {
        return challenge("hcaptcha");
    }
    if has("awswaf-captcha")
        || has("aws-waf-client-browser")
        || (size < 10_000 && has("interstitial-spinner") && has("verifying your connection"))
    {
        return challenge("aws-waf");
    }
    if has("geo.captcha-delivery.com") || has("captcha-delivery.com/captcha") {
        return challenge("datadome");
    }
    if title == "challenge page" || (size < 5_000 && has("bazadebezolkohpepadr")) {
        return challenge("akamai");
    }
    if size < 50_000 && (has("_pxhd") || has("px-captcha") || has("perimeterx")) {
        return challenge("perimeterx");
    }
    if size < 50_000 && (has("_incapsula_resource") || has("incapsula") || has("visid_incap")) {
        return challenge("imperva");
    }
    if size < 30_000
        && prose_chars < words(50)
        && has("google.com/recaptcha/api.js")
        && has("g-recaptcha")
    {
        return challenge("recaptcha");
    }
    if size < 30_000 && (has("kpsdk") || has("/ips.js")) {
        return challenge("kasada");
    }
    // 8: antibot title.
    if prose_chars < words(200) && title_starts(ANTIBOT_TITLES) {
        let vocab = if has("cf-ray") || has("cloudflare") {
            if has("error 1020") || has("you have been blocked") {
                "cloudflare:1020"
            } else {
                "cloudflare"
            }
        } else {
            "antibot-title"
        };
        return challenge(vocab);
    }
    // 9–11: status rows.
    if status == 429 {
        return issue(IssueKind::RateLimited, "");
    }
    if matches!(status, 401 | 403) {
        return (body.trim().is_empty() || looks_like_html(&lower))
            .then(|| issue(IssueKind::AccessChallenge, ""))
            .flatten();
    }
    if status == 503 || (has("this is taking longer than usual") && has("refresh")) {
        return issue(IssueKind::ErrorPage, "error-page");
    }
    // 12–13: consent.
    let url = final_url.as_str().to_ascii_lowercase();
    if CONSENT_URLS.iter().any(|p| url.contains(p)) {
        return issue(IssueKind::ConsentWall, &consent_vocab(&lower, "url"));
    }
    if prose_chars <= words(50) && title_starts(CONSENT_TITLES) {
        return issue(IssueKind::ConsentWall, &consent_vocab(&lower, "title"));
    }
    // 14: login.
    if prose_chars < words(200) && title_starts(LOGIN_TITLES) {
        return issue(IssueKind::LoginRequired, "login");
    }
    // 15–16: empty shell.
    if prose_chars < words(50) && size > 5_000 && has("<script") {
        return issue(IssueKind::EmptyShell, "empty-shell");
    }
    if prose_chars < words(800) && size > 50_000 {
        let normalised = lower.replace('\'', "\"");
        if SPA_MARKERS.iter().any(|m| normalised.contains(m)) {
            return issue(IssueKind::EmptyShell, "empty-shell");
        }
    }
    None
}

/// `consent:<platform>` when a known platform's markup is in the body, else `consent:<fallback>`.
fn consent_vocab(lower: &str, fallback: &str) -> String {
    let platform = CONSENT_PLATFORMS
        .iter()
        .find(|p| lower.contains(*p))
        .copied()
        .unwrap_or(fallback);
    format!("consent:{platform}")
}

/// Trimmed `<title>` text of an already-lowercased body; empty when absent.
fn title_of(lower: &str) -> String {
    let Some(open) = lower.find("<title") else {
        return String::new();
    };
    let Some(gt) = lower[open..].find('>') else {
        return String::new();
    };
    let start = open + gt + 1;
    let end = lower[start..]
        .find("</title")
        .map_or(lower.len(), |e| start + e);
    super::collapse_ws(&lower[start..end])
}

fn looks_like_html(lower: &str) -> bool {
    ["<html", "<!doctype html", "<body", "<head", "<title"]
        .iter()
        .any(|m| lower.contains(m))
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
