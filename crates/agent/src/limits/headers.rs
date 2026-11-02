//! Response-header parsers (160 §5.4, recon-limits §3–§4): one response's
//! lower-case headers → rate buckets, usage windows, credits and a 429
//! deadline. Pure; the hub merges the result.

use super::{Balance, RateLimit, UsageWindow};

/// What one response said about its provider's limits.
#[derive(Debug, Default, PartialEq)]
pub(crate) struct Parsed {
    pub rates: Vec<RateLimit>,
    pub windows: Vec<UsageWindow>,
    pub balance: Option<Balance>,
    /// The server's `date` header, ms (orders concurrent observations).
    pub date_ms: Option<u64>,
    /// A 429 (or a rejected subscription window): limited until this instant, ms.
    pub limited_until_ms: Option<u64>,
}

impl Parsed {
    pub(crate) fn is_empty(&self) -> bool {
        self.rates.is_empty()
            && self.windows.is_empty()
            && self.balance.is_none()
            && self.limited_until_ms.is_none()
    }
}

/// Wait assumed after a 429 that names no reset (Anthropic's spend-cap 429 has none).
const DEFAULT_429_MS: u64 = 60_000;

/// Anthropic API-key buckets: wire infix, row id, label (verified live, recon §4).
const ANTHROPIC_BUCKETS: [(&str, &str, &str); 4] = [
    ("requests", "requests", "Requests / min"),
    ("tokens", "tokens", "Tokens / min"),
    ("input-tokens", "input_tokens", "Input tokens / min"),
    ("output-tokens", "output_tokens", "Output tokens / min"),
];

/// OpenAI-style `x-ratelimit-{limit,remaining,reset}-<suffix>` buckets (OpenAI,
/// Groq, xAI, gateways); `""` is OpenRouter's suffix-less form.
const GENERIC_BUCKETS: [(&str, &str, &str); 4] = [
    ("-requests", "requests", "Requests"),
    ("-tokens", "tokens", "Tokens"),
    ("-project-tokens", "project_tokens", "Project tokens"),
    ("", "requests", "Requests"),
];

/// Parse every known header family. `model` scopes the rate buckets
/// ([`model_group`]); `now_ms` anchors relative resets.
pub(crate) fn parse(model: &str, status: u16, h: &[(String, String)], now_ms: u64) -> Parsed {
    let scope = Some(model_group(model));
    let mut out = Parsed {
        date_ms: get(h, "date").and_then(http_date_ms),
        ..Parsed::default()
    };
    for (wire, id, label) in ANTHROPIC_BUCKETS {
        let p = format!("anthropic-ratelimit-{wire}-");
        if let Some(rate) = bucket(
            h,
            &format!("{p}limit"),
            &format!("{p}remaining"),
            &format!("{p}reset"),
            id,
            label,
            &scope,
            now_ms,
        ) {
            out.rates.push(rate);
        }
    }
    for (suffix, id, label) in GENERIC_BUCKETS {
        if out.rates.iter().any(|r| r.id == id) {
            continue;
        }
        if let Some(rate) = bucket(
            h,
            &format!("x-ratelimit-limit{suffix}"),
            &format!("x-ratelimit-remaining{suffix}"),
            &format!("x-ratelimit-reset{suffix}"),
            id,
            label,
            &scope,
            now_ms,
        ) {
            out.rates.push(rate);
        }
    }
    unified(h, &mut out);
    codex(h, &mut out);
    if status == 429 {
        let retry = get(h, "retry-after-ms")
            .and_then(|v| v.trim().parse::<f64>().ok())
            .map(|ms| now_ms + ms.max(0.0) as u64)
            .or_else(|| get(h, "retry-after").and_then(|v| retry_after_ms(v, now_ms)));
        let exhausted = out
            .rates
            .iter()
            .filter(|r| r.remaining == 0)
            .filter_map(|r| r.resets_at_ms)
            .max();
        let until = retry
            .or(exhausted)
            .or(out.limited_until_ms)
            .unwrap_or(now_ms + DEFAULT_429_MS);
        out.limited_until_ms = Some(until);
    }
    out
}

/// The model class a provider scopes rate limits by (Anthropic: per model
/// family, recon §3.1): `claude-haiku-5-5` → `claude-haiku`; others keep the
/// slug minus a provider prefix, `:effort` and a trailing date.
pub(crate) fn model_group(model: &str) -> String {
    let slug = model.rsplit('/').next().unwrap_or(model);
    let slug = slug.split(':').next().unwrap_or(slug);
    if let Some(rest) = slug.strip_prefix("claude-")
        && let Some(family) = rest
            .split('-')
            .find(|s| !s.is_empty() && s.chars().all(|c| c.is_ascii_alphabetic()))
    {
        return format!("claude-{family}");
    }
    // `-20251001` or `-2024-08-06`.
    let digits = |s: &str| !s.is_empty() && s.chars().all(|c| c.is_ascii_digit());
    if let Some((head, date)) = slug.rsplit_once('-')
        && date.len() == 8
        && digits(date)
    {
        return head.to_owned();
    }
    let b = slug.as_bytes();
    if b.len() > 11 && b[b.len() - 11] == b'-' {
        let tail = &slug[slug.len() - 10..];
        let parts: Vec<&str> = tail.split('-').collect();
        if parts.len() == 3 && parts.iter().all(|p| digits(p)) && parts[0].len() == 4 {
            return slug[..slug.len() - 11].to_owned();
        }
    }
    slug.to_owned()
}

/// One `limit` / `remaining` / `reset` triple; `None` unless both counts parse.
#[allow(clippy::too_many_arguments)]
fn bucket(
    h: &[(String, String)],
    limit: &str,
    remaining: &str,
    reset: &str,
    id: &str,
    label: &str,
    scope: &Option<String>,
    now_ms: u64,
) -> Option<RateLimit> {
    let limit = count(get(h, limit)?)?;
    let remaining = count(get(h, remaining)?)?;
    Some(RateLimit {
        id: id.to_owned(),
        label: label.to_owned(),
        scope: scope.clone(),
        limit,
        remaining,
        resets_at_ms: get(h, reset).and_then(|v| reset_ms(v, now_ms)),
    })
}

/// Anthropic subscription windows: `anthropic-ratelimit-unified-<w>-{utilization,
/// reset,status}`, utilization a fraction, reset epoch seconds (recon §3.2).
fn unified(h: &[(String, String)], out: &mut Parsed) {
    const P: &str = "anthropic-ratelimit-unified-";
    for (name, value) in h {
        let Some(w) = name
            .strip_prefix(P)
            .and_then(|r| r.strip_suffix("-utilization"))
        else {
            continue;
        };
        let Some(used) = value.trim().parse::<f64>().ok().filter(|f| f.is_finite()) else {
            continue;
        };
        let resets_at_ms = get(h, &format!("{P}{w}-reset")).and_then(epoch_ms);
        let (id, label, window_secs) = match w {
            "5h" => ("five_hour".to_owned(), "5-hour".to_owned(), Some(18_000)),
            "7d" => ("seven_day".to_owned(), "Weekly".to_owned(), Some(604_800)),
            other => match other.strip_prefix("7d_") {
                Some(s) => (
                    format!("seven_day_{s}"),
                    format!("Weekly · {s}"),
                    Some(604_800),
                ),
                None => (other.to_owned(), other.to_owned(), None),
            },
        };
        if get(h, &format!("{P}{w}-status")).is_some_and(|s| s.eq_ignore_ascii_case("rejected")) {
            out.limited_until_ms = out.limited_until_ms.max(resets_at_ms);
        }
        out.windows.push(UsageWindow {
            id,
            label,
            used_fraction: used,
            resets_at_ms,
            window_secs,
        });
    }
    if get(h, "anthropic-ratelimit-unified-status")
        .is_some_and(|s| s.eq_ignore_ascii_case("rejected"))
    {
        let reset = get(h, "anthropic-ratelimit-unified-reset").and_then(epoch_ms);
        out.limited_until_ms = out.limited_until_ms.max(reset);
    }
}

/// Codex subscription headers (codex-rs `rate_limits.rs`): `x-<id>-{primary,
/// secondary}-{used-percent,window-minutes,reset-at}` (`<id>` = `codex` or an
/// extra metered feature) and `x-codex-credits-*`.
fn codex(h: &[(String, String)], out: &mut Parsed) {
    for (name, value) in h {
        let Some(rest) = name.strip_prefix("x-") else {
            continue;
        };
        let Some((limit_id, slot)) = ["primary", "secondary"].iter().find_map(|slot| {
            rest.strip_suffix(&format!("-{slot}-used-percent"))
                .map(|id| (id, *slot))
        }) else {
            continue;
        };
        let Some(pct) = value.trim().parse::<f64>().ok().filter(|f| f.is_finite()) else {
            continue;
        };
        let p = format!("x-{limit_id}-{slot}-");
        let window_secs = get(h, &format!("{p}window-minutes"))
            .and_then(count)
            .map(|m| m * 60);
        let span = window_label(window_secs);
        let (id, label) = if limit_id == "codex" {
            (slot.to_owned(), span)
        } else {
            let name = get(h, &format!("x-{limit_id}-limit-name")).unwrap_or(limit_id);
            (format!("{limit_id}:{slot}"), format!("{name} · {span}"))
        };
        out.windows.push(UsageWindow {
            id,
            label,
            used_fraction: pct / 100.0,
            resets_at_ms: get(h, &format!("{p}reset-at")).and_then(epoch_ms),
            window_secs,
        });
    }
    let has = get(h, "x-codex-credits-has-credits").map(truthy);
    let unlimited = get(h, "x-codex-credits-unlimited").is_some_and(truthy);
    let balance = get(h, "x-codex-credits-balance").and_then(|b| b.trim().parse::<f64>().ok());
    if has == Some(true) || unlimited || balance.is_some() {
        out.balance = Some(Balance {
            label: if unlimited {
                "Credits · unlimited"
            } else {
                "Credits"
            }
            .to_owned(),
            currency: "credits".to_owned(),
            remaining: balance,
            limit: None,
            used: None,
        });
    }
}

/// `5-hour`, `Weekly`, `Daily`, `30-day`, `90-minute` … from a window length.
pub(crate) fn window_label(secs: Option<u64>) -> String {
    match secs {
        Some(18_000) => "5-hour".to_owned(),
        Some(604_800) => "Weekly".to_owned(),
        Some(86_400) => "Daily".to_owned(),
        Some(s) if s > 0 && s % 86_400 == 0 => format!("{}-day", s / 86_400),
        Some(s) if s > 0 && s % 3_600 == 0 => format!("{}-hour", s / 3_600),
        Some(s) if s > 0 => format!("{}-minute", s.div_ceil(60)),
        _ => "Window".to_owned(),
    }
}

/// First header named `name` (names are lower-case).
fn get<'a>(h: &'a [(String, String)], name: &str) -> Option<&'a str> {
    h.iter().find(|(k, _)| k == name).map(|(_, v)| v.as_str())
}

fn truthy(v: &str) -> bool {
    matches!(v.trim().to_ascii_lowercase().as_str(), "true" | "1" | "yes")
}

/// A non-negative integer count (`"45000"`, `"1.2e4"` tolerated).
fn count(v: &str) -> Option<u64> {
    let v = v.trim();
    v.parse::<u64>().ok().or_else(|| {
        v.parse::<f64>()
            .ok()
            .filter(|f| f.is_finite() && *f >= 0.0)
            .map(|f| f as u64)
    })
}

/// A bucket reset in any of the forms providers use: RFC 3339 (Anthropic),
/// a Go duration (`6m0s`, `1.5s`, `20ms` — OpenAI/Groq), or a bare number
/// (epoch ms, epoch s, else seconds from now).
pub(crate) fn reset_ms(v: &str, now_ms: u64) -> Option<u64> {
    let v = v.trim();
    if let Ok(t) = chrono::DateTime::parse_from_rfc3339(v) {
        return u64::try_from(t.timestamp_millis()).ok();
    }
    if let Ok(n) = v.parse::<f64>() {
        if !n.is_finite() || n < 0.0 {
            return None;
        }
        return Some(if n >= 1e12 {
            n as u64
        } else if n >= 1e9 {
            (n * 1000.0) as u64
        } else {
            now_ms + (n * 1000.0) as u64
        });
    }
    go_duration_ms(v).map(|d| now_ms + d)
}

/// `retry-after`: delay seconds or an HTTP date.
fn retry_after_ms(v: &str, now_ms: u64) -> Option<u64> {
    let v = v.trim();
    match v.parse::<f64>() {
        Ok(s) if s.is_finite() && s >= 0.0 => Some(now_ms + (s * 1000.0) as u64),
        _ => http_date_ms(v),
    }
}

/// Epoch seconds (Codex `reset-at`, Anthropic unified resets).
fn epoch_ms(v: &str) -> Option<u64> {
    v.trim()
        .parse::<f64>()
        .ok()
        .filter(|f| f.is_finite() && *f > 0.0)
        .map(|s| (s * 1000.0) as u64)
}

/// RFC 2822 (`date`, `retry-after`).
fn http_date_ms(v: &str) -> Option<u64> {
    chrono::DateTime::parse_from_rfc2822(v.trim())
        .ok()
        .and_then(|t| u64::try_from(t.timestamp_millis()).ok())
}

/// A Go `time.Duration` string: `2m59.56s`, `6m0s`, `1.5s`, `20ms`, `1h`.
pub(crate) fn go_duration_ms(v: &str) -> Option<u64> {
    let mut rest = v.trim();
    if rest.is_empty() {
        return None;
    }
    let mut total = 0.0_f64;
    while !rest.is_empty() {
        let n_end = rest
            .find(|c: char| !(c.is_ascii_digit() || c == '.'))
            .unwrap_or(rest.len());
        let n: f64 = rest[..n_end].parse().ok()?;
        rest = &rest[n_end..];
        let (scale, len) = [
            ("ms", 1.0),
            ("us", 0.001),
            ("µs", 0.001),
            ("ns", 0.000_001),
            ("h", 3_600_000.0),
            ("m", 60_000.0),
            ("s", 1_000.0),
        ]
        .iter()
        .find(|(u, _)| rest.starts_with(u))
        .map(|(u, s)| (*s, u.len()))?;
        total += n * scale;
        rest = &rest[len..];
    }
    Some(total.round() as u64)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn h(pairs: &[(&str, &str)]) -> Vec<(String, String)> {
        pairs
            .iter()
            .map(|(k, v)| ((*k).to_owned(), (*v).to_owned()))
            .collect()
    }

    const NOW: u64 = 1_793_629_470_000; // 2026-11-02T14:24:30Z

    /// The 26-header 200 captured live (recon §4), relevant names verbatim.
    fn anthropic_live() -> Vec<(String, String)> {
        h(&[
            ("date", "Mon, 02 Nov 2026 14:24:34 GMT"),
            ("anthropic-ratelimit-requests-limit", "45000"),
            ("anthropic-ratelimit-requests-remaining", "44999"),
            ("anthropic-ratelimit-requests-reset", "2026-11-02T14:24:35Z"),
            ("anthropic-ratelimit-tokens-limit", "12000000"),
            ("anthropic-ratelimit-tokens-remaining", "12000000"),
            ("anthropic-ratelimit-tokens-reset", "2026-11-02T14:24:36Z"),
            ("anthropic-ratelimit-input-tokens-limit", "10000000"),
            ("anthropic-ratelimit-input-tokens-remaining", "10000000"),
            (
                "anthropic-ratelimit-input-tokens-reset",
                "2026-11-02T14:24:36Z",
            ),
            ("anthropic-ratelimit-output-tokens-limit", "2000000"),
            ("anthropic-ratelimit-output-tokens-remaining", "2000000"),
            (
                "anthropic-ratelimit-output-tokens-reset",
                "2026-11-02T14:24:36Z",
            ),
            (
                "anthropic-organization-id",
                "00000000-0000-0000-0000-000000000000",
            ),
            ("request-id", "req_x"),
        ])
    }

    #[test]
    fn anthropic_api_key_buckets_scope_by_model_group() {
        let p = parse("claude-haiku-5-5", 200, &anthropic_live(), NOW);
        assert_eq!(p.rates.len(), 4, "requests, tokens, input, output");
        let req = &p.rates[0];
        assert_eq!(
            (req.id.as_str(), req.limit, req.remaining),
            ("requests", 45_000, 44_999)
        );
        assert_eq!(req.scope.as_deref(), Some("claude-haiku"));
        assert_eq!(req.resets_at_ms, Some(1_793_629_475_000));
        assert_eq!(p.rates[3].id, "output_tokens");
        assert_eq!(p.date_ms, Some(1_793_629_474_000));
        assert!(p.windows.is_empty() && p.limited_until_ms.is_none());
    }

    #[test]
    fn a_429_is_limited_until_retry_after_else_the_exhausted_reset_else_a_minute() {
        let mut hs = anthropic_live();
        hs.push(("retry-after".into(), "17".into()));
        assert_eq!(
            parse("claude-opus-5-5", 429, &hs, NOW).limited_until_ms,
            Some(NOW + 17_000)
        );
        let mut hs = anthropic_live();
        hs.retain(|(k, _)| k != "anthropic-ratelimit-output-tokens-remaining");
        hs.push((
            "anthropic-ratelimit-output-tokens-remaining".into(),
            "0".into(),
        ));
        assert_eq!(
            parse("claude-opus-5-5", 429, &hs, NOW).limited_until_ms,
            Some(1_793_629_476_000),
            "no retry-after: the empty bucket's reset"
        );
        assert_eq!(
            parse("m", 429, &[], NOW).limited_until_ms,
            Some(NOW + 60_000),
            "the spend-cap 429 names no reset"
        );
        assert_eq!(
            parse("m", 200, &anthropic_live(), NOW).limited_until_ms,
            None
        );
    }

    #[test]
    fn anthropic_unified_windows_are_fractions_with_epoch_resets() {
        // claude-code issue #12829 dump (recon §3.2).
        let hs = h(&[
            ("anthropic-ratelimit-unified-status", "allowed"),
            ("anthropic-ratelimit-unified-5h-status", "allowed"),
            ("anthropic-ratelimit-unified-5h-reset", "1764554400"),
            (
                "anthropic-ratelimit-unified-5h-utilization",
                "0.018416969696969696",
            ),
            ("anthropic-ratelimit-unified-7d-status", "allowed"),
            ("anthropic-ratelimit-unified-7d-reset", "1764615600"),
            (
                "anthropic-ratelimit-unified-7d-utilization",
                "0.7370692663445869",
            ),
            ("anthropic-ratelimit-unified-7d_oi-status", "rejected"),
            ("anthropic-ratelimit-unified-7d_oi-reset", "1764615700"),
            ("anthropic-ratelimit-unified-7d_oi-utilization", "1.0"),
            (
                "anthropic-ratelimit-unified-representative-claim",
                "five_hour",
            ),
        ]);
        let p = parse("claude-opus-5-5", 200, &hs, NOW);
        let ids: Vec<_> = p.windows.iter().map(|w| w.id.as_str()).collect();
        assert_eq!(ids, ["five_hour", "seven_day", "seven_day_oi"]);
        assert!((p.windows[1].used_fraction - 0.737).abs() < 1e-3);
        assert_eq!(p.windows[0].resets_at_ms, Some(1_764_554_400_000));
        assert_eq!(p.windows[0].window_secs, Some(18_000));
        assert_eq!(p.windows[2].label, "Weekly · oi");
        assert_eq!(
            p.limited_until_ms,
            Some(1_764_615_700_000),
            "a rejected window limits until its reset"
        );
    }

    #[test]
    fn codex_windows_extra_features_and_credits() {
        let hs = h(&[
            ("x-codex-primary-used-percent", "42.5"),
            ("x-codex-primary-window-minutes", "300"),
            ("x-codex-primary-reset-at", "1793640000"),
            ("x-codex-secondary-used-percent", "80"),
            ("x-codex-secondary-window-minutes", "10080"),
            ("x-codex-secondary-reset-at", "1794000000"),
            ("x-codex-other-primary-used-percent", "5"),
            ("x-codex-other-primary-window-minutes", "60"),
            ("x-codex-other-limit-name", "GPT-6 Pro"),
            ("x-codex-credits-has-credits", "true"),
            ("x-codex-credits-unlimited", "false"),
            ("x-codex-credits-balance", "12.5"),
        ]);
        let p = parse("gpt-6.1-sol", 200, &hs, NOW);
        let got: Vec<_> = p
            .windows
            .iter()
            .map(|w| (w.id.as_str(), w.label.as_str()))
            .collect();
        assert_eq!(
            got,
            [
                ("primary", "5-hour"),
                ("secondary", "Weekly"),
                ("codex-other:primary", "GPT-6 Pro · 1-hour")
            ]
        );
        assert!((p.windows[0].used_fraction - 0.425).abs() < 1e-9);
        assert_eq!(p.windows[1].resets_at_ms, Some(1_794_000_000_000));
        assert_eq!(p.windows[1].window_secs, Some(604_800));
        let b = p.balance.expect("credits");
        assert_eq!((b.label.as_str(), b.remaining), ("Credits", Some(12.5)));
    }

    #[test]
    fn generic_x_ratelimit_durations_and_project_tokens() {
        let hs = h(&[
            ("x-ratelimit-limit-requests", "60"),
            ("x-ratelimit-remaining-requests", "59"),
            ("x-ratelimit-reset-requests", "1s"),
            ("x-ratelimit-limit-tokens", "150000"),
            ("x-ratelimit-remaining-tokens", "149984"),
            ("x-ratelimit-reset-tokens", "6m0s"),
            ("x-ratelimit-limit-project-tokens", "60000"),
            ("x-ratelimit-remaining-project-tokens", "57000"),
            ("x-ratelimit-reset-project-tokens", "20ms"),
        ]);
        let p = parse("openai/gpt-4o-2024-08-06", 200, &hs, NOW);
        let got: Vec<_> = p
            .rates
            .iter()
            .map(|r| (r.id.as_str(), r.remaining, r.resets_at_ms.map(|t| t - NOW)))
            .collect();
        assert_eq!(
            got,
            [
                ("requests", 59, Some(1_000)),
                ("tokens", 149_984, Some(360_000)),
                ("project_tokens", 57_000, Some(20))
            ]
        );
        assert_eq!(p.rates[0].scope.as_deref(), Some("gpt-4o"));
    }

    #[test]
    fn go_durations() {
        assert_eq!(go_duration_ms("6m0s"), Some(360_000));
        assert_eq!(go_duration_ms("1.5s"), Some(1_500));
        assert_eq!(go_duration_ms("20ms"), Some(20));
        assert_eq!(go_duration_ms("2m59.56s"), Some(179_560));
        assert_eq!(go_duration_ms("1h"), Some(3_600_000));
        assert_eq!(go_duration_ms("soon"), None);
        assert_eq!(go_duration_ms("5x"), None);
    }

    #[test]
    fn model_groups() {
        assert_eq!(model_group("claude-haiku-5-5"), "claude-haiku");
        assert_eq!(model_group("anthropic/claude-opus-5-5"), "claude-opus");
        assert_eq!(model_group("claude-3-5-haiku-20241022"), "claude-haiku");
        assert_eq!(model_group("claude-haiku-4-5-20251001"), "claude-haiku");
        assert_eq!(model_group("gpt-6.1-sol:high"), "gpt-6.1-sol");
        assert_eq!(model_group("openai/gpt-4o-2024-08-06"), "gpt-4o");
        assert_eq!(model_group("llama-3.3-70b"), "llama-3.3-70b");
    }

    #[test]
    fn empty_headers_parse_to_nothing() {
        assert!(parse("m", 200, &h(&[("date", "x"), ("server", "cf")]), NOW).is_empty());
    }
}
