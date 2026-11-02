//! Background pollers (160 §5.3): what each usage endpoint is, how often it
//! may be asked, and how its JSON maps onto windows and balances. The
//! schedule and the parsers are pure (tested with an explicit clock); the
//! fetch is the only network code in the module.

use std::time::Duration;

use serde_json::Value;

use super::headers::window_label;
use super::{Balance, UsageWindow};
use crate::supervisor::Auth;

const MIN: u64 = 60_000;
/// Per-request budget (§5.3); the whole poll, token refresh included, gets 3×.
pub(crate) const TIMEOUT: Duration = Duration::from_secs(5);
/// Cool-down after a 429: doubles from 5 to at most 30 minutes.
const COOLDOWN_FIRST: u64 = 5 * MIN;
const COOLDOWN_MAX: u64 = 30 * MIN;
/// The cheapest current Haiku ($0.10 / $0.50 per M, recon §4): one probe ≈ $1.5e-6.
pub(crate) const PROBE_MODEL: &str = "claude-haiku-5-5";

/// One refresh job. `Local` re-reads credentials, budgets and recent
/// providers (no network); `Probe` is the opt-in paid Anthropic call.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Source {
    Local,
    Codex,
    Claude,
    OpenRouter,
    OpencodeGo,
    DeepSeek,
    Probe,
}

impl Source {
    pub(crate) const ALL: [Source; 7] = [
        Source::Local,
        Source::Codex,
        Source::Claude,
        Source::OpenRouter,
        Source::OpencodeGo,
        Source::DeepSeek,
        Source::Probe,
    ];

    /// The name `kick(provider)` and the `limits.<provider>` config key use.
    pub(crate) fn provider(self) -> &'static str {
        match self {
            Source::Local => "theuth",
            Source::Codex => "openai",
            Source::Claude | Source::Probe => "anthropic",
            Source::OpenRouter => "openrouter",
            Source::OpencodeGo => "opencode-go",
            Source::DeepSeek => "deepseek",
        }
    }

    /// The provider row a usage endpoint feeds.
    pub(crate) fn row(self) -> Option<&'static str> {
        match self {
            Source::Codex => Some("openai:oauth"),
            Source::Claude => Some("anthropic:oauth"),
            Source::OpenRouter => Some("openrouter:api_key"),
            Source::OpencodeGo => Some("opencode-go:api_key"),
            Source::DeepSeek => Some("deepseek:api_key"),
            Source::Local | Source::Probe => None,
        }
    }

    /// `(background cadence, minimum spacing of kicked refreshes)`, ms (§5.3).
    pub(crate) fn cadence(self) -> (u64, u64) {
        match self {
            Source::Codex => (60 * MIN, 5 * MIN),
            Source::Probe => (10 * MIN, 10 * MIN),
            _ => (10 * MIN, MIN),
        }
    }
}

/// How a poll ended, for the schedule.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Outcome {
    Done,
    /// The endpoint answered 429: back off, never retry inside the poll.
    Throttled,
}

/// When a source runs next: the background cadence ±20 % jitter, kicks no
/// closer together than `kick_min`, and a doubling cool-down after a 429.
#[derive(Clone, Debug)]
pub(crate) struct Schedule {
    every: u64,
    kick_min: u64,
    pub(crate) next_due: u64,
    last_run: Option<u64>,
    cooldown: u64,
    blocked_until: u64,
}

impl Schedule {
    /// `last_run`: the source's restored observation, so engine restarts never
    /// poll sooner than the cadence allows (review 160-LIM #7).
    pub(crate) fn new(source: Source, first_due: u64, last_run: Option<u64>) -> Self {
        let (every, kick_min) = source.cadence();
        Self {
            every,
            kick_min,
            next_due: last_run.map_or(first_due, |t| first_due.max(t.saturating_add(every))),
            last_run,
            cooldown: 0,
            blocked_until: 0,
        }
    }

    /// A refresh was asked for: run as soon as the spacing and any cool-down allow.
    pub(crate) fn kick(&mut self, now: u64) {
        let earliest = self
            .last_run
            .map_or(now, |t| t + self.kick_min)
            .max(self.blocked_until)
            .max(now);
        self.next_due = self.next_due.min(earliest);
    }

    /// A run finished at `now`; `jitter` in `-0.2..=0.2`.
    pub(crate) fn ran(&mut self, now: u64, outcome: Outcome, jitter: f64) {
        self.last_run = Some(now);
        let base = (self.every as f64 * (1.0 + jitter.clamp(-0.2, 0.2))) as u64;
        match outcome {
            Outcome::Done => {
                self.cooldown = 0;
                self.blocked_until = 0;
                self.next_due = now + base;
            }
            Outcome::Throttled => {
                self.cooldown = if self.cooldown == 0 {
                    COOLDOWN_FIRST
                } else {
                    (self.cooldown * 2).min(COOLDOWN_MAX)
                };
                self.blocked_until = now + self.cooldown;
                self.next_due = now + base.max(self.cooldown);
            }
        }
    }
}

/// ±20 % from the clock's sub-second nanos (the retry loop's trick, `loop_.rs`).
pub(crate) fn jitter() -> f64 {
    let nanos = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0, |d| d.subsec_nanos());
    f64::from(nanos % 4001) / 10_000.0 - 0.2
}

/// What one successful poll learned.
#[derive(Clone, Debug, Default, PartialEq)]
pub(crate) struct PollData {
    pub windows: Vec<UsageWindow>,
    pub balance: Option<Balance>,
    pub plan: Option<String>,
    pub account: Option<String>,
    /// The source says calls are refused now (credits gone, balance empty).
    pub limited: bool,
}

/// Why a poll produced nothing.
#[derive(Clone, Debug, PartialEq)]
pub(crate) enum PollError {
    /// 429 from the usage endpoint.
    Throttled,
    /// Network, 5xx, a non-hub engine's expiring token: keep last-good, `stale`.
    Transient(String),
    /// Expired login, rejected key, a body of the wrong shape: `error`.
    Hard(String),
}

/// `Tokens.account_id` as the row shows it: never more than 12 characters.
pub(crate) fn truncate_account(id: &str) -> String {
    let mut chars = id.chars();
    let head: String = chars.by_ref().take(12).collect();
    if chars.next().is_some() {
        format!("{head}…")
    } else {
        head
    }
}

/// An `oauth::Store::access` failure, mapped to the status rules (recon §2.4):
/// the token endpoint unreachable or answering 5xx/429 is transient (Code
/// flow: `<ns>: token endpoint: …` / `… returned 503 …`; Poll flow:
/// `<ns>: refresh: …`); a 4xx or a missing grant is a dead login.
fn token_error(ns: &str, e: &str) -> PollError {
    let returned = e
        .split_once(" returned ")
        .and_then(|(_, s)| s.split_whitespace().next())
        .and_then(|s| s.parse::<u16>().ok());
    if e.contains("hub engine") {
        PollError::Transient("the hub engine refreshes this login; showing last values".into())
    } else if e.contains(": refresh: ")
        || e.contains(": token endpoint: ")
        || e.contains("refreshed elsewhere")
        || returned.is_some_and(|s| s == 429 || s >= 500)
    {
        PollError::Transient("token refresh failed; showing last values".into())
    } else {
        PollError::Hard(format!("login expired — run /login {ns}"))
    }
}

/// `oauth::Store::access` in a detached task: refresh tokens rotate, so the
/// poll's timeout must never cancel a refresh between the server's rotation
/// and the save (review 160-LIM #5); a timeout only stops waiting for it.
async fn access(auth: &Auth, p: oauth::Provider) -> Result<oauth::Tokens, String> {
    let store = auth.oauth.clone();
    tokio::spawn(async move { store.access(&p).await })
        .await
        .map_err(|e| format!("token refresh task: {e}"))?
}

fn env_key(name: &str) -> Option<String> {
    std::env::var(name).ok().filter(|v| !v.is_empty())
}

/// The vault key `ns`, else env `var`.
pub(crate) fn key(auth: &Auth, ns: &str, var: &str) -> Option<String> {
    auth.key(ns).or_else(|| env_key(var))
}

/// The Anthropic API key the engine uses (vault `anthropic`, else env).
pub(crate) fn anthropic_key(auth: &Auth) -> Option<String> {
    key(auth, "anthropic", "ANTHROPIC_API_KEY")
}

/// Whether `source`'s credential exists (cheap: no refresh, no network).
pub(crate) fn has_credential(source: Source, auth: &Auth) -> bool {
    match source {
        Source::Local => true,
        Source::Codex => auth.oauth.load("openai").is_some(),
        Source::Claude => auth.oauth.load("anthropic").is_some(),
        Source::OpenRouter => auth.openrouter_key().is_some(),
        Source::OpencodeGo => key(auth, "opencode_go", "OPENCODE_GO_API_KEY").is_some(),
        Source::DeepSeek => key(auth, "deepseek", "DEEPSEEK_API_KEY").is_some(),
        Source::Probe => anthropic_key(auth).is_some(),
    }
}

/// Ask `source`'s usage endpoint once. `Ok(None)`: no credential.
pub(crate) async fn fetch(
    source: Source,
    auth: &Auth,
    http: &reqwest::Client,
    now_ms: u64,
) -> Result<Option<PollData>, PollError> {
    let bearer = |t: &str| ("authorization", format!("Bearer {t}"));
    match source {
        Source::Codex => {
            if auth.oauth.load("openai").is_none() {
                return Ok(None);
            }
            let t = access(auth, oauth::OPENAI)
                .await
                .map_err(|e| token_error("openai", &e))?;
            let mut headers = vec![
                bearer(&t.access_token),
                ("originator", "codex_cli_rs".to_owned()),
            ];
            if let Some(id) = &t.account_id {
                headers.push(("chatgpt-account-id", id.clone()));
            }
            let v = get_json(
                http,
                "https://chatgpt.com/backend-api/wham/usage",
                &headers,
                Some("openai"),
            )
            .await?;
            let mut data = parse_codex(&v, now_ms).map_err(PollError::Hard)?;
            data.account = t.account_id.as_deref().map(truncate_account);
            Ok(Some(data))
        }
        Source::Claude => {
            if auth.oauth.load("anthropic").is_none() {
                return Ok(None);
            }
            let t = access(auth, oauth::ANTHROPIC)
                .await
                .map_err(|e| token_error("anthropic", &e))?;
            let headers = [
                bearer(&t.access_token),
                ("anthropic-beta", "oauth-2025-04-20".to_owned()),
            ];
            let v = get_json(
                http,
                "https://api.anthropic.com/api/oauth/usage",
                &headers,
                Some("anthropic"),
            )
            .await?;
            let mut data = parse_claude(&v).map_err(PollError::Hard)?;
            data.account = t.account_id.as_deref().map(truncate_account);
            Ok(Some(data))
        }
        Source::OpenRouter => {
            let Some(k) = auth.openrouter_key() else {
                return Ok(None);
            };
            let v = get_json(
                http,
                "https://openrouter.ai/api/v1/key",
                &[bearer(&k)],
                None,
            )
            .await?;
            parse_openrouter(&v).map(Some).map_err(PollError::Hard)
        }
        Source::OpencodeGo => {
            let Some(k) = key(auth, "opencode_go", "OPENCODE_GO_API_KEY") else {
                return Ok(None);
            };
            let v = get_json(
                http,
                "https://opencode.ai/zen/go/v1/usage",
                &[bearer(&k)],
                None,
            )
            .await?;
            parse_opencode_go(&v).map(Some).map_err(PollError::Hard)
        }
        Source::DeepSeek => {
            let Some(k) = key(auth, "deepseek", "DEEPSEEK_API_KEY") else {
                return Ok(None);
            };
            let v = get_json(
                http,
                "https://api.deepseek.com/user/balance",
                &[bearer(&k)],
                None,
            )
            .await?;
            parse_deepseek(&v).map(Some).map_err(PollError::Hard)
        }
        Source::Local | Source::Probe => Ok(None),
    }
}

/// The opt-in probe (§5.3): one `max_tokens: 1` Haiku call; its response
/// headers are the observation. Returns `(status, lower-case headers)`.
pub(crate) async fn probe(
    key: &str,
    http: &reqwest::Client,
) -> Result<(u16, Vec<(String, String)>), PollError> {
    let body = serde_json::json!({
        "model": PROBE_MODEL,
        "max_tokens": 1,
        "thinking": {"type": "disabled"},
        "messages": [{"role": "user", "content": "hi"}],
    });
    let resp = http
        .post("https://api.anthropic.com/v1/messages")
        .timeout(TIMEOUT)
        .header("x-api-key", key)
        .header("anthropic-version", "2023-06-01")
        .json(&body)
        .send()
        .await
        .map_err(|_| PollError::Transient("probe: Anthropic unreachable".into()))?;
    Ok((resp.status().as_u16(), super::lower(resp.headers())))
}

async fn get_json(
    http: &reqwest::Client,
    url: &str,
    headers: &[(&str, String)],
    oauth_ns: Option<&str>,
) -> Result<Value, PollError> {
    let mut req = http
        .get(url)
        .timeout(TIMEOUT)
        .header("accept", "application/json")
        .header("user-agent", concat!("theuth/", env!("CARGO_PKG_VERSION")));
    for (k, v) in headers {
        req = req.header(*k, v);
    }
    let resp = req.send().await.map_err(|e| {
        PollError::Transient(if e.is_timeout() {
            "usage endpoint timed out".into()
        } else {
            "usage endpoint unreachable".into()
        })
    })?;
    match resp.status().as_u16() {
        200..=299 => resp
            .json::<Value>()
            .await
            .map_err(|_| PollError::Hard("usage endpoint answered with a non-JSON body".into())),
        429 => Err(PollError::Throttled),
        s @ (401 | 403) => Err(PollError::Hard(match oauth_ns {
            Some(ns) => format!("login expired — run /login {ns}"),
            None => format!("the key was rejected ({s})"),
        })),
        s => Err(PollError::Transient(format!("usage endpoint returned {s}"))),
    }
}

// region: parsers — defensive: unknown fields ignored, a missing core → Err

fn f64_at(v: &Value, key: &str) -> Option<f64> {
    let x = v.get(key)?;
    x.as_f64()
        .or_else(|| x.as_str().and_then(|s| s.trim().parse().ok()))
        .filter(|f| f.is_finite())
}

fn u64_at(v: &Value, key: &str) -> Option<u64> {
    f64_at(v, key).filter(|f| *f >= 0.0).map(|f| f as u64)
}

fn iso_ms(v: &Value, key: &str) -> Option<u64> {
    let s = v.get(key)?.as_str()?;
    chrono::DateTime::parse_from_rfc3339(s)
        .ok()
        .and_then(|t| u64::try_from(t.timestamp_millis()).ok())
}

/// One codex-rs `RateLimitWindowSnapshot`; `reset_at` (unix s) preferred
/// over `reset_after_seconds`.
fn codex_window(w: &Value, id: String, name: Option<&str>, now_ms: u64) -> Option<UsageWindow> {
    let used = f64_at(w, "used_percent")?;
    let window_secs = u64_at(w, "limit_window_seconds").filter(|s| *s > 0);
    let resets_at_ms = u64_at(w, "reset_at")
        .filter(|s| *s > 0)
        .map(|s| s.saturating_mul(1000))
        .or_else(|| {
            u64_at(w, "reset_after_seconds").map(|s| now_ms.saturating_add(s.saturating_mul(1000)))
        });
    let span = window_label(window_secs);
    Some(UsageWindow {
        id,
        label: name.map_or(span.clone(), |n| format!("{n} · {span}")),
        used_fraction: used / 100.0,
        resets_at_ms,
        window_secs,
    })
}

/// `GET /backend-api/wham/usage` (codex-rs `RateLimitStatusPayload`).
pub(crate) fn parse_codex(v: &Value, now_ms: u64) -> Result<PollData, String> {
    let rl = v.get("rate_limit").filter(|r| r.is_object());
    let plan = v
        .get("plan_type")
        .and_then(Value::as_str)
        .filter(|p| !p.is_empty() && *p != "unknown")
        .map(str::to_owned);
    if rl.is_none() && plan.is_none() {
        return Err("Codex usage: unexpected response shape".into());
    }
    let mut data = PollData {
        plan,
        ..PollData::default()
    };
    let slots = [
        ("primary_window", "primary"),
        ("secondary_window", "secondary"),
    ];
    if let Some(rl) = rl {
        for (key, id) in slots {
            if let Some(w) = rl
                .get(key)
                .and_then(|w| codex_window(w, id.into(), None, now_ms))
            {
                data.windows.push(w);
            }
        }
        data.limited = rl.get("limit_reached").and_then(Value::as_bool) == Some(true);
    }
    for extra in v
        .get("additional_rate_limits")
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
    {
        let feature = extra
            .get("metered_feature")
            .or_else(|| extra.get("limit_name"))
            .and_then(Value::as_str)
            .unwrap_or("extra");
        let name = extra.get("limit_name").and_then(Value::as_str);
        let Some(rl) = extra.get("rate_limit") else {
            continue;
        };
        for (key, slot) in slots {
            if let Some(w) = rl
                .get(key)
                .and_then(|w| codex_window(w, format!("{feature}:{slot}"), name, now_ms))
            {
                data.windows.push(w);
            }
        }
    }
    if let Some(c) = v.get("credits").filter(|c| c.is_object()) {
        let has = c.get("has_credits").and_then(Value::as_bool) == Some(true);
        let unlimited = c.get("unlimited").and_then(Value::as_bool) == Some(true);
        let remaining = f64_at(c, "balance");
        if has || unlimited || remaining.is_some() {
            data.balance = Some(Balance {
                label: if unlimited {
                    "Credits · unlimited"
                } else {
                    "Credits"
                }
                .into(),
                currency: "credits".into(),
                remaining,
                limit: None,
                used: None,
            });
        }
    }
    Ok(data)
}

/// Minor units with an exponent (`{"amount_minor":1234,"exponent":2}` = 12.34).
fn minor(v: &Value) -> Option<f64> {
    let amount = f64_at(v, "amount_minor")?;
    let exp = u64_at(v, "exponent").unwrap_or(2).min(9) as i32;
    Some(amount / 10f64.powi(exp))
}

/// `GET /api/oauth/usage` (recon §3.2): utilization in percent, ISO resets;
/// model-scoped weekly caps only in `limits[kind=weekly_scoped]`.
pub(crate) fn parse_claude(v: &Value) -> Result<PollData, String> {
    let mut data = PollData::default();
    let fixed = [
        ("five_hour", "5-hour", 18_000),
        ("seven_day", "Weekly", 604_800),
        ("seven_day_opus", "Weekly · Opus", 604_800),
        ("seven_day_sonnet", "Weekly · Sonnet", 604_800),
    ];
    let mut known = false;
    for (key, label, secs) in fixed {
        known |= v.get(key).is_some();
        if let Some(w) = v.get(key).filter(|w| w.is_object())
            && let Some(pct) = f64_at(w, "utilization")
        {
            data.windows.push(UsageWindow {
                id: key.into(),
                label: label.into(),
                used_fraction: pct / 100.0,
                resets_at_ms: iso_ms(w, "resets_at"),
                window_secs: Some(secs),
            });
        }
    }
    for l in v
        .get("limits")
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
    {
        known = true;
        let Some(pct) = f64_at(l, "percent") else {
            continue;
        };
        let (id, label, secs) = match l.get("kind").and_then(Value::as_str) {
            Some("session") => ("five_hour".to_owned(), "5-hour".to_owned(), 18_000),
            Some("weekly_all") => ("seven_day".to_owned(), "Weekly".to_owned(), 604_800),
            Some("weekly_scoped") => {
                let name = l
                    .pointer("/scope/model/display_name")
                    .and_then(Value::as_str)
                    .unwrap_or("model");
                (
                    format!("weekly_scoped:{name}"),
                    format!("Weekly · {name}"),
                    604_800,
                )
            }
            _ => continue,
        };
        if data.windows.iter().any(|w| w.id == id) {
            continue;
        }
        data.windows.push(UsageWindow {
            id,
            label,
            used_fraction: pct / 100.0,
            resets_at_ms: iso_ms(l, "resets_at"),
            window_secs: Some(secs),
        });
    }
    if !known {
        return Err("Claude usage: unexpected response shape".into());
    }
    // `spend` supersedes `extra_usage` when present (recon §3.2).
    if let Some(s) = v
        .get("spend")
        .filter(|s| s.get("enabled").and_then(Value::as_bool) == Some(true))
    {
        let used = s.get("used").and_then(minor);
        let limit = s.get("limit").and_then(minor);
        let currency = s
            .pointer("/used/currency")
            .or_else(|| s.pointer("/limit/currency"))
            .and_then(Value::as_str)
            .unwrap_or("USD");
        data.balance = Some(Balance {
            label: "Extra usage".into(),
            currency: currency.into(),
            remaining: limit.zip(used).map(|(l, u)| (l - u).max(0.0)),
            limit,
            used,
        });
    } else if let Some(x) = v
        .get("extra_usage")
        .filter(|x| x.get("is_enabled").and_then(Value::as_bool) == Some(true))
    {
        let used = f64_at(x, "used_credits").map(|c| c / 100.0);
        let limit = f64_at(x, "monthly_limit").map(|c| c / 100.0);
        data.balance = Some(Balance {
            label: "Extra usage".into(),
            currency: x
                .get("currency")
                .and_then(Value::as_str)
                .unwrap_or("USD")
                .into(),
            remaining: limit.zip(used).map(|(l, u)| (l - u).max(0.0)),
            limit,
            used,
        });
    }
    Ok(data)
}

/// `GET /api/v1/key` (recon §3.5): credits are USD.
pub(crate) fn parse_openrouter(v: &Value) -> Result<PollData, String> {
    let d = v
        .get("data")
        .filter(|d| d.is_object())
        .ok_or("OpenRouter key: unexpected response shape")?;
    let limit = f64_at(d, "limit");
    let mut data = PollData {
        balance: Some(Balance {
            label: if limit.is_some() {
                "Key limit"
            } else {
                "Usage"
            }
            .into(),
            currency: "USD".into(),
            remaining: f64_at(d, "limit_remaining"),
            limit,
            used: f64_at(d, "usage"),
        }),
        plan: (d.get("is_free_tier").and_then(Value::as_bool) == Some(true))
            .then(|| "free tier".to_owned()),
        ..PollData::default()
    };
    if let Some(f) = d.get("free_model_daily_requests")
        && let (Some(used), Some(cap)) = (f64_at(f, "used"), f64_at(f, "limit"))
        && cap > 0.0
    {
        data.windows.push(UsageWindow {
            id: "free_daily".into(),
            label: "Free requests · daily".into(),
            used_fraction: used / cap,
            resets_at_ms: None,
            window_secs: Some(86_400),
        });
    }
    data.limited = data
        .balance
        .as_ref()
        .and_then(|b| b.remaining)
        .is_some_and(|r| r <= 0.0);
    Ok(data)
}

/// `GET /zen/go/v1/usage` (recon §3.6): `usage.{rolling,weekly,monthly}`.
pub(crate) fn parse_opencode_go(v: &Value) -> Result<PollData, String> {
    let u = v
        .get("usage")
        .filter(|u| u.is_object())
        .ok_or("OpenCode Go usage: unexpected response shape")?;
    let mut data = PollData::default();
    for (key, label, secs) in [
        ("rolling", "5-hour", Some(18_000)),
        ("weekly", "Weekly", Some(604_800)),
        ("monthly", "Monthly", None),
    ] {
        let Some(w) = u.get(key) else { continue };
        let Some(pct) = f64_at(w, "percent") else {
            continue;
        };
        let rate_limited = w.get("status").and_then(Value::as_str) == Some("rate-limited");
        let used = pct / 100.0;
        data.windows.push(UsageWindow {
            id: key.into(),
            label: label.into(),
            used_fraction: if rate_limited { used.max(1.0) } else { used },
            resets_at_ms: iso_ms(w, "resetsAt"),
            window_secs: secs,
        });
    }
    if data.windows.is_empty() {
        return Err("OpenCode Go usage: no windows in the response".into());
    }
    Ok(data)
}

/// `GET /user/balance` (recon §3.10): string amounts, USD preferred over CNY.
pub(crate) fn parse_deepseek(v: &Value) -> Result<PollData, String> {
    let infos = v
        .get("balance_infos")
        .and_then(Value::as_array)
        .ok_or("DeepSeek balance: unexpected response shape")?;
    let info = infos
        .iter()
        .find(|i| i.get("currency").and_then(Value::as_str) == Some("USD"))
        .or_else(|| infos.first());
    Ok(PollData {
        balance: info.map(|i| Balance {
            label: "Balance".into(),
            currency: i
                .get("currency")
                .and_then(Value::as_str)
                .unwrap_or("USD")
                .into(),
            remaining: f64_at(i, "total_balance"),
            limit: None,
            used: None,
        }),
        limited: v.get("is_available").and_then(Value::as_bool) == Some(false),
        ..PollData::default()
    })
}

// endregion

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    const NOW: u64 = 1_793_629_470_000;

    #[test]
    fn schedule_cadence_kick_spacing_and_429_cooldown() {
        let mut s = Schedule::new(Source::Claude, NOW, None);
        s.ran(NOW, Outcome::Done, 0.0);
        assert_eq!(s.next_due, NOW + 10 * MIN, "10 min background");
        s.kick(NOW + 10_000);
        assert_eq!(s.next_due, NOW + MIN, "kicks no closer than 1 per min");
        s.kick(NOW + 2 * MIN);
        assert_eq!(s.next_due, NOW + MIN, "a later kick never delays");
        s.ran(NOW + MIN, Outcome::Done, 0.2);
        assert_eq!(s.next_due, NOW + MIN + 12 * MIN, "+20 % jitter");
        s.ran(NOW + MIN, Outcome::Done, -0.9);
        assert_eq!(s.next_due, NOW + MIN + 8 * MIN, "jitter clamps at -20 %");

        // 429s: 5 → 10 → 20 → 30 → 30 min cool-down; kicks wait it out.
        let mut t = NOW;
        let mut cool = Vec::new();
        for _ in 0..5 {
            s.ran(t, Outcome::Throttled, 0.0);
            cool.push(s.blocked_until - t);
            s.kick(t + 1);
            assert_eq!(
                s.next_due, s.blocked_until,
                "a kick waits out the cool-down"
            );
            t = s.next_due;
        }
        assert_eq!(cool, [5 * MIN, 10 * MIN, 20 * MIN, 30 * MIN, 30 * MIN]);
        s.ran(t, Outcome::Done, 0.0);
        s.kick(t + 2 * MIN);
        assert_eq!(s.next_due, t + 2 * MIN, "a success clears the cool-down");

        let mut c = Schedule::new(Source::Codex, NOW, None);
        c.ran(NOW, Outcome::Done, 0.0);
        assert_eq!(c.next_due, NOW + 60 * MIN, "Codex polls hourly");
        c.kick(NOW + MIN);
        assert_eq!(c.next_due, NOW + 5 * MIN, "≤ 1 per 5 min on kick");

        // Review 160-LIM #7: a restart 2 min after the last poll keeps both rules.
        let start = NOW + 2 * MIN;
        let mut r = Schedule::new(Source::Codex, start + 20_000, Some(NOW));
        assert_eq!(
            r.next_due,
            NOW + 60 * MIN,
            "background cadence survives a restart"
        );
        r.kick(start);
        assert_eq!(r.next_due, NOW + 5 * MIN, "kick spacing survives a restart");
        let fresh = Schedule::new(Source::Codex, start + 20_000, Some(NOW - 2 * 60 * MIN));
        assert_eq!(
            fresh.next_due,
            start + 20_000,
            "an old observation waits only the boot delay"
        );
    }

    #[test]
    fn jitter_stays_in_band() {
        for _ in 0..100 {
            let j = jitter();
            assert!((-0.2..=0.2).contains(&j), "{j}");
        }
    }

    #[test]
    fn codex_payload_maps_windows_plan_and_credits() {
        // Shaped like codex-rs `RateLimitStatusPayload` (recon §3.4).
        let v = json!({
            "plan_type": "pro",
            "rate_limit": {
                "allowed": true, "limit_reached": false,
                "primary_window": {"used_percent": 37, "limit_window_seconds": 18000,
                                   "reset_after_seconds": 3600, "reset_at": 1793633070},
                "secondary_window": {"used_percent": 81, "limit_window_seconds": 604800,
                                     "reset_after_seconds": 90000, "reset_at": 0}
            },
            "credits": {"has_credits": true, "unlimited": false, "balance": "120"},
            "additional_rate_limits": [{"limit_name": "GPT-6 Pro", "metered_feature": "pro",
                "rate_limit": {"primary_window": {"used_percent": 3, "limit_window_seconds": 86400,
                                                  "reset_after_seconds": 10, "reset_at": 0}}}],
            "spend_control": {"reached": false},
            "brand_new_field": [1, 2, 3]
        });
        let d = parse_codex(&v, NOW).unwrap();
        assert_eq!(d.plan.as_deref(), Some("pro"));
        let w: Vec<_> = d
            .windows
            .iter()
            .map(|w| (w.id.as_str(), w.label.as_str(), w.resets_at_ms))
            .collect();
        assert_eq!(
            w,
            [
                ("primary", "5-hour", Some(1_793_633_070_000)),
                ("secondary", "Weekly", Some(NOW + 90_000_000)),
                ("pro:primary", "GPT-6 Pro · Daily", Some(NOW + 10_000)),
            ]
        );
        assert!((d.windows[1].used_fraction - 0.81).abs() < 1e-9);
        assert_eq!(d.balance.unwrap().remaining, Some(120.0));
        assert!(!d.limited);
        assert!(parse_codex(&json!({"detail": "nope"}), NOW).is_err());
    }

    #[test]
    fn claude_usage_payload() {
        // recon §3.2 reconstruction.
        let v = json!({
            "five_hour":  {"utilization": 16, "resets_at": "2026-11-02T19:00:00.123456+00:00"},
            "seven_day":  {"utilization": 18, "resets_at": "2026-11-05T11:00:00+00:00"},
            "seven_day_opus": null, "seven_day_sonnet": null,
            "limits": [
                {"kind":"session","group":"session","percent":16,"severity":"normal","resets_at":"2026-11-02T19:00:00+00:00","scope":null,"is_active":true},
                {"kind":"weekly_all","group":"weekly","percent":18,"severity":"normal","resets_at":"2026-11-05T11:00:00+00:00","scope":null,"is_active":true},
                {"kind":"weekly_scoped","group":"weekly","percent":28,"severity":"normal","resets_at":"2026-11-05T11:00:00+00:00",
                 "scope":{"model":{"display_name":"Fable","id":null},"surface":null},"is_active":true}
            ],
            "extra_usage": {"is_enabled":false,"monthly_limit":null,"used_credits":null,"utilization":null,"currency":null},
            "spend": {"enabled":true,"used":{"amount_minor":1234,"currency":"USD","exponent":2},
                      "limit":{"amount_minor":5000,"currency":"USD","exponent":2}}
        });
        let d = parse_claude(&v).unwrap();
        let ids: Vec<_> = d.windows.iter().map(|w| w.id.as_str()).collect();
        assert_eq!(ids, ["five_hour", "seven_day", "weekly_scoped:Fable"]);
        assert!((d.windows[0].used_fraction - 0.16).abs() < 1e-9);
        assert_eq!(d.windows[0].resets_at_ms, Some(1_793_646_000_123));
        assert_eq!(d.windows[2].label, "Weekly · Fable");
        let b = d.balance.unwrap();
        assert_eq!((b.used, b.limit), (Some(12.34), Some(50.0)));
        assert!((b.remaining.unwrap() - 37.66).abs() < 1e-9);
        assert!(parse_claude(&json!({"error": {"type": "x"}})).is_err());
    }

    #[test]
    fn openrouter_opencode_go_and_deepseek() {
        let d = parse_openrouter(&json!({"data": {"label": "sk-or-v1-abc…", "limit": 20,
            "limit_remaining": 7.5, "usage": 12.5, "is_free_tier": false,
            "free_model_daily_requests": {"used": 10, "limit": 50, "remaining": 40}}}))
        .unwrap();
        let b = d.balance.unwrap();
        assert_eq!(
            (b.label.as_str(), b.remaining, b.limit, b.used),
            ("Key limit", Some(7.5), Some(20.0), Some(12.5))
        );
        assert_eq!(d.windows[0].used_fraction, 0.2);
        assert!(parse_openrouter(&json!({"error": "x"})).is_err());

        let d = parse_opencode_go(&json!({"usage": {
            "rolling": {"status": "ok", "percent": 40, "resetsAt": "2026-11-02T19:00:00Z"},
            "weekly": {"status": "rate-limited", "percent": 97, "resetsAt": "2026-11-05T00:00:00Z"},
            "monthly": {"status": "ok", "percent": 50, "resetsAt": "2026-12-01T00:00:00Z"}}}))
        .unwrap();
        let used: Vec<_> = d.windows.iter().map(|w| w.used_fraction).collect();
        assert_eq!(used, [0.4, 1.0, 0.5], "rate-limited counts as exhausted");
        assert!(parse_opencode_go(&json!({"usage": {}})).is_err());

        let d = parse_deepseek(&json!({"is_available": false, "balance_infos": [
            {"currency": "CNY", "total_balance": "110.00", "granted_balance": "10.00", "topped_up_balance": "100.00"},
            {"currency": "USD", "total_balance": "0.00", "granted_balance": "0", "topped_up_balance": "0"}]}))
        .unwrap();
        let b = d.balance.unwrap();
        assert_eq!((b.currency.as_str(), b.remaining), ("USD", Some(0.0)));
        assert!(d.limited);
        assert!(parse_deepseek(&json!({})).is_err());
    }

    #[test]
    fn token_errors_map_to_stale_or_error() {
        assert!(matches!(
            token_error(
                "openai",
                "openai: token expiring; the hub engine refreshes it — retry shortly"
            ),
            PollError::Transient(_)
        ));
        assert_eq!(
            token_error("openai", "openai: login expired; run /login openai"),
            PollError::Hard("login expired — run /login openai".into())
        );
        assert_eq!(
            token_error(
                "openai",
                "openai: refresh returned 400 Bad Request; run /login openai"
            ),
            PollError::Hard("login expired — run /login openai".into())
        );
        assert_eq!(truncate_account("0123456789abcdef"), "0123456789ab…");
        assert_eq!(truncate_account("short"), "short");
    }

    /// Review 160-LIM #2: OPENAI / ANTHROPIC are `Flow::Code`; a network blip
    /// or a 5xx/429 at the token endpoint is transient, a 4xx is a dead login.
    #[test]
    fn code_flow_token_errors_split_transient_from_hard() {
        for e in [
            "openai: token endpoint: error sending request for url (https://auth.openai.com/oauth/token); run /login openai",
            "openai: token endpoint returned 503 Service Unavailable; run /login openai",
            "openai: token endpoint returned 502 Bad Gateway; run /login openai",
            "openai: token endpoint returned 429 Too Many Requests; run /login openai",
        ] {
            assert!(
                matches!(token_error("openai", e), PollError::Transient(_)),
                "{e}"
            );
        }
        for e in [
            "anthropic: token endpoint returned 400 Bad Request; run /login anthropic",
            "anthropic: token endpoint returned 401 Unauthorized; run /login anthropic",
            "anthropic: token endpoint returned 403 Forbidden; run /login anthropic",
            "anthropic: not logged in; run /login anthropic",
        ] {
            assert_eq!(
                token_error("anthropic", e),
                PollError::Hard("login expired — run /login anthropic".into()),
                "{e}"
            );
        }
    }

    /// Review 160-LIM #1: a usage payload's absurd seconds saturate.
    #[test]
    fn codex_window_absurd_seconds_saturate() {
        let w = codex_window(
            &json!({"used_percent": 1, "reset_at": 1e300}),
            "primary".into(),
            None,
            NOW,
        )
        .unwrap();
        assert_eq!(w.resets_at_ms, Some(u64::MAX));
        let w = codex_window(
            &json!({"used_percent": 1, "reset_after_seconds": 1e300}),
            "primary".into(),
            None,
            NOW,
        )
        .unwrap();
        assert_eq!(w.resets_at_ms, Some(u64::MAX));
    }

    /// Review 160-LIM #5: a poll that gives up waiting leaves the refresh to
    /// finish and save the rotated grant (no forced /login).
    #[tokio::test]
    async fn a_timed_out_poll_never_cancels_a_token_refresh() {
        use wiremock::{Mock, MockServer, ResponseTemplate, matchers::method};
        let server = MockServer::start().await;
        Mock::given(method("POST"))
            .respond_with(
                ResponseTemplate::new(200)
                    .set_delay(Duration::from_millis(300))
                    .set_body_json(
                        json!({"access_token": "a2", "refresh_token": "r2", "expires_in": 3600}),
                    ),
            )
            .mount(&server)
            .await;
        let dir = crate::TestDir::new("limits-refresh");
        let auth = Auth::stub(&dir.0);
        let old = oauth::Tokens {
            access_token: "a1".into(),
            refresh_token: Some("r1".into()),
            expires_at: 0,
            account_id: None,
        };
        auth.oauth.save("openai", &old).await.unwrap();
        let mut p = oauth::OPENAI;
        p.token_url = format!("{}/token", server.uri()).into();
        let waited = tokio::time::timeout(Duration::from_millis(50), access(&auth, p)).await;
        assert!(waited.is_err(), "the poll gave up waiting");
        tokio::time::sleep(Duration::from_millis(800)).await;
        let t = auth.oauth.load("openai").unwrap();
        assert_eq!(
            t.refresh_token.as_deref(),
            Some("r2"),
            "rotated grant saved"
        );
    }
}
