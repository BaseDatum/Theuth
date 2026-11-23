//! URL canonicalisation for dedupe keys and agent-facing links (design doc
//! 16 §7: ported `normalize/url.rs`).
//!
//! Agent-facing URLs are never truncated; tracking parameters are stripped so
//! the same document from different engines merges; Bing click redirects are
//! unwrapped first.

use url::Url;

/// Cap for the breadcrumb [`display_url`].
const DISPLAY_MAX_CHARS: usize = 60;

/// Tracking / analytics query parameters (exact, case-insensitive); `utm_*` by prefix.
const TRACKING_PARAMS: &[&str] = &[
    "fbclid", "gclid", "gbraid", "wbraid", "dclid", "msclkid", "twclid", "yclid", "mc_cid",
    "mc_eid", "ref", "ref_src", "_ga", "_gl", "igshid", "mkt_tok", "vero_id", "wickedid",
];

/// A canonicalised URL: dedupe key plus the form shown to the agent.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Canonical {
    /// Equality key: lowercased scheme/host, no fragment or tracking params,
    /// non-root trailing slash dropped, query pairs sorted.
    pub key: String,
    /// Agent-facing URL: same stripping, but first-seen path shape and query order.
    pub pretty: String,
}

/// Canonicalise `raw`; `None` when it does not parse or is not http(s).
#[must_use]
pub fn canonicalize(raw: &str) -> Option<Canonical> {
    let trimmed = raw.trim();
    if trimmed.is_empty() {
        return None;
    }
    let unwrapped = unwrap_bing_redirect(trimmed);
    let mut url = Url::parse(unwrapped.as_deref().unwrap_or(trimmed)).ok()?;
    let scheme = url.scheme().to_ascii_lowercase();
    if scheme != "http" && scheme != "https" {
        return None;
    }
    url.set_scheme(&scheme).ok()?;
    let host = url.host_str()?.to_ascii_lowercase();
    url.set_host(Some(&host)).ok()?;
    url.set_fragment(None);
    // Default ports are already dropped by the parser.

    let pairs: Vec<(String, String)> = url
        .query_pairs()
        .filter(|(k, _)| !is_tracking_param(k))
        .map(|(k, v)| (k.into_owned(), v.into_owned()))
        .collect();
    let pretty_path = if url.path().is_empty() {
        "/".to_owned()
    } else {
        url.path().to_owned()
    };
    let key_path = if pretty_path == "/" {
        "/"
    } else {
        pretty_path.strip_suffix('/').unwrap_or(&pretty_path)
    };

    apply(&mut url, &pretty_path, &pairs);
    let pretty = url.to_string();
    let mut sorted = pairs;
    sorted.sort();
    apply(&mut url, key_path, &sorted);
    Some(Canonical {
        key: url.to_string(),
        pretty,
    })
}

fn apply(url: &mut Url, path: &str, pairs: &[(String, String)]) {
    url.set_path(path);
    if pairs.is_empty() {
        url.set_query(None);
    } else {
        url.query_pairs_mut()
            .clear()
            .extend_pairs(pairs.iter().map(|(k, v)| (k.as_str(), v.as_str())));
    }
}

/// Dedupe key for `raw`, or the trimmed input when it cannot be canonicalised.
#[must_use]
pub fn canonical_key(raw: &str) -> String {
    canonicalize(raw).map_or_else(|| raw.trim().to_owned(), |c| c.key)
}

/// SERP-style breadcrumb: `<host sans www.> › <seg> › <seg>`, capped at 60 chars
/// (last kept char becomes `…`). Unparsable input yields an empty string.
#[must_use]
pub fn display_url(raw: &str) -> String {
    let Ok(url) = Url::parse(raw.trim()) else {
        return String::new();
    };
    let host = url.host_str().unwrap_or("").to_ascii_lowercase();
    let mut out = host.strip_prefix("www.").unwrap_or(&host).to_owned();
    for seg in url.path().split('/').filter(|s| !s.is_empty()) {
        out.push_str(" › ");
        out.push_str(seg);
    }
    if out.chars().count() > DISPLAY_MAX_CHARS {
        out = out.chars().take(DISPLAY_MAX_CHARS - 1).collect();
        out.push('…');
    }
    out
}

/// `true` for a tracking / analytics query parameter (`utm_*`, `fbclid`, …).
/// Shared with `routes` (44 §3.3) so both strip the same list.
#[must_use]
pub fn is_tracking_param(name: &str) -> bool {
    let lower = name.to_ascii_lowercase();
    lower.starts_with("utm_") || TRACKING_PARAMS.contains(&lower.as_str())
}

/// Unwrap a Bing click redirect (`bing.com/ck/a?...&u=a1<base64url>`); the
/// decoded payload must be an absolute http(s) URL. `None` for anything else.
pub(crate) fn unwrap_bing_redirect(raw: &str) -> Option<String> {
    if !raw.to_ascii_lowercase().contains("bing.com/ck/a") {
        return None;
    }
    let parsed = Url::parse(raw).ok()?;
    let u = parsed
        .query_pairs()
        .find(|(k, _)| k.eq_ignore_ascii_case("u"))
        .map(|(_, v)| v.into_owned())?;
    let decoded = String::from_utf8(base64url_decode(u.strip_prefix("a1")?)?).ok()?;
    (decoded.starts_with("http://") || decoded.starts_with("https://")).then_some(decoded)
}

/// Decode URL-safe base64 without padding; `None` on any foreign byte.
fn base64url_decode(s: &str) -> Option<Vec<u8>> {
    let mut out = Vec::with_capacity(s.len() * 3 / 4);
    let (mut acc, mut bits) = (0u32, 0u8);
    for b in s.bytes() {
        let v = match b {
            b'A'..=b'Z' => b - b'A',
            b'a'..=b'z' => b - b'a' + 26,
            b'0'..=b'9' => b - b'0' + 52,
            b'-' => 62,
            b'_' => 63,
            b'=' => break,
            _ => return None,
        };
        acc = (acc << 6) | u32::from(v);
        bits += 6;
        if bits >= 8 {
            bits -= 8;
            out.push(u8::try_from((acc >> bits) & 0xff).ok()?);
        }
    }
    (!out.is_empty()).then_some(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn bing_redirect_unwraps_to_target() {
        // base64url("https://example.com/page") without padding
        let raw = "https://www.bing.com/ck/a?!&&p=abc&u=a1aHR0cHM6Ly9leGFtcGxlLmNvbS9wYWdl&ntb=1";
        assert_eq!(
            canonicalize(raw).map(|c| c.key).as_deref(),
            Some("https://example.com/page")
        );
    }

    #[test]
    fn breadcrumb_strips_www_and_caps() {
        assert_eq!(
            display_url("https://www.go.dev/doc/tutorial/"),
            "go.dev › doc › tutorial"
        );
        let long = display_url(&format!("https://example.com/{}", "a/".repeat(60)));
        assert_eq!(long.chars().count(), 60);
        assert!(long.ends_with('…'));
    }
}
