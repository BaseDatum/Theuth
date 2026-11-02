//! Usage limits (160 §5): one engine-wide cache of every provider's rate
//! buckets, subscription windows and balances, plus theuth's own budget caps.
//!
//! Fed by response headers (`observe_headers`, from `Backend::stream`'s
//! [`HeaderTap`] — every model call, success or failure) and by background
//! pollers ([`poll`], started once in `supervisor::start`). The RPC path
//! reads [`LimitsHub::snapshot`]: memory only, never the network. The cache
//! survives restarts in `<data_dir>/limits/snapshot.json`, served as `stale`
//! until fresh data arrives.

mod budgets;
mod headers;
mod poll;

use std::collections::{BTreeMap, HashMap, HashSet};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Arc, LazyLock};
use std::time::Duration;

use genai::chat::{ChatFrameSink, FrameCtx, RawFrameRef};
use serde::{Deserialize, Serialize};
use tokio::sync::Notify;

use crate::supervisor::Auth;
use poll::{Outcome, PollData, PollError, Schedule, Source};

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

const MIN: u64 = 60_000;
/// Header data counts as stale after this long without a call (§5.3).
const HEADERS_STALE_MS: u64 = 15 * MIN;
/// Not-configured providers are shown only with calls this recent (§5.3).
const RECENT_MS: u64 = 30 * 24 * 60 * MIN;
/// The probe runs only while someone looks (a kick this recent) and no
/// real Anthropic call refreshed the buckets for 5 minutes (recon §3.1).
const PROBE_VISIBLE_MS: u64 = 3 * MIN;
const PROBE_IDLE_MS: u64 = 5 * MIN;
/// Snapshot writes trail the last change by this much.
const WRITE_DEBOUNCE: Duration = Duration::from_secs(2);
/// Pollers wait this long after engine start (the engine has better things to do).
const FIRST_POLL_MS: u64 = 20_000;
const DIR: &str = "limits";
const SNAPSHOT: &str = "snapshot.json";

pub(crate) fn now_ms() -> u64 {
    u64::try_from(chrono::Utc::now().timestamp_millis()).unwrap_or(0)
}

/// A response's headers with lower-case names (non-UTF-8 values dropped).
pub(crate) fn lower(h: &reqwest::header::HeaderMap) -> Vec<(String, String)> {
    h.iter()
        .filter_map(|(k, v)| Some((k.as_str().to_ascii_lowercase(), v.to_str().ok()?.to_owned())))
        .collect()
}

/// The engine-wide limits cache. Cheap to clone (Arc inside).
#[derive(Clone, Default)]
pub struct LimitsHub {
    inner: Arc<Inner>,
}

#[derive(Default)]
struct Inner {
    state: parking_lot::Mutex<State>,
    /// One wake-up per running poller, for `kick`.
    sources: parking_lot::Mutex<Vec<(Source, Arc<Notify>)>>,
    /// Woken on every change; the writer persists after a debounce.
    changed: Notify,
    dirty: AtomicBool,
    last_kick_ms: AtomicU64,
    started: AtomicBool,
}

impl std::fmt::Debug for LimitsHub {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("LimitsHub").finish_non_exhaustive()
    }
}

impl LimitsHub {
    /// The process-wide hub. Production code uses only this one; tests build their own with `default()`.
    pub fn global() -> &'static LimitsHub {
        static HUB: LazyLock<LimitsHub> = LazyLock::new(LimitsHub::default);
        &HUB
    }

    /// The current snapshot, from memory (microseconds; never touches the network).
    pub fn snapshot(&self) -> UsageLimits {
        self.inner.state.lock().render(now_ms())
    }

    /// One response's headers (lower-case names) from a model call to `provider` with `auth`,
    /// for `model` (scope derivation). Called on every response, success or failure.
    pub fn observe_headers(
        &self,
        provider: &str,
        auth: &str,
        model: &str,
        status: u16,
        headers: &[(String, String)],
        now_ms: u64,
    ) {
        let changed = self
            .inner
            .state
            .lock()
            .observe(provider, auth, model, status, headers, now_ms);
        if changed {
            self.mark_changed();
        }
    }

    /// Ask the pollers to refresh soon (debounced per source; returns immediately).
    pub fn kick(&self, provider: Option<&str>) {
        self.inner.last_kick_ms.store(now_ms(), Ordering::Relaxed);
        for (source, wake) in self.inner.sources.lock().iter() {
            let hit = match provider {
                None => true,
                Some(p) => p == source.provider(),
            };
            if hit {
                wake.notify_one();
            }
        }
    }

    fn mark_changed(&self) {
        self.inner.dirty.store(true, Ordering::Relaxed);
        self.inner.changed.notify_one();
    }

    /// 160 §5.3: load the last snapshot, then run the pollers and the
    /// snapshot writer until shutdown. Called once, from `supervisor::start`;
    /// a second call (another engine in the same test process) is a no-op.
    pub(crate) fn start(&self, rt: &runtime::Runtime, auth: Auth) {
        if self.inner.started.swap(true, Ordering::Relaxed) {
            return;
        }
        let path = rt.data_dir.join(DIR).join(SNAPSHOT);
        if let Some(snap) = load(&path) {
            *self.inner.state.lock() = State::restore(snap);
        }
        let cfg: LimitsConfig = if rt.config.contains("limits") {
            rt.section("limits").unwrap_or_else(|e| {
                tracing::warn!("limits: bad config section, using defaults: {e}");
                LimitsConfig::default()
            })
        } else {
            LimitsConfig::default()
        };
        let http = match reqwest::Client::builder().timeout(poll::TIMEOUT).build() {
            Ok(c) => c,
            Err(e) => {
                tracing::warn!("limits: no HTTP client ({e}); pollers off");
                return;
            }
        };
        let ctx = Arc::new(Ctx {
            auth,
            http,
            db: open_db(&rt.data_dir.join("sessions.sqlite")),
            data_dir: rt.data_dir.clone(),
        });
        {
            let hub = self.clone();
            let shutdown = rt.shutdown.clone();
            tokio::spawn(async move { hub.writer(path, shutdown).await });
        }
        for source in Source::ALL {
            let on = match source {
                Source::Local => true,
                Source::Probe => cfg.probe(source.provider()),
                _ => cfg.poll(source.provider()),
            };
            if !on {
                tracing::info!("limits: {} poller off by config", source.provider());
                continue;
            }
            let wake = Arc::new(Notify::new());
            self.inner.sources.lock().push((source, wake.clone()));
            let (hub, ctx, shutdown) = (self.clone(), ctx.clone(), rt.shutdown.clone());
            tokio::spawn(async move { hub.run(source, ctx, wake, shutdown).await });
        }
    }

    /// One source's tick loop (the `update` plugin's pattern): sleep until
    /// due or kicked, run, reschedule. One task per source = one in flight.
    async fn run(
        &self,
        source: Source,
        ctx: Arc<Ctx>,
        wake: Arc<Notify>,
        shutdown: tokio_util::sync::CancellationToken,
    ) {
        let first = if source == Source::Local {
            0
        } else {
            FIRST_POLL_MS
        };
        let last = source.row().and_then(|id| {
            let st = self.inner.state.lock();
            st.rows.get(id).and_then(|r| r.observed_at_ms)
        });
        let mut sched = Schedule::new(source, now_ms() + first, last);
        loop {
            let wait = sched.next_due.saturating_sub(now_ms());
            tokio::select! {
                () = shutdown.cancelled() => return,
                () = wake.notified() => {
                    sched.kick(now_ms());
                    continue;
                }
                () = tokio::time::sleep(Duration::from_millis(wait)) => {}
            }
            if now_ms() < sched.next_due {
                continue;
            }
            let outcome = self.run_once(source, &ctx).await;
            sched.ran(now_ms(), outcome, poll::jitter());
        }
    }

    async fn run_once(&self, source: Source, ctx: &Ctx) -> Outcome {
        let now = now_ms();
        match source {
            Source::Local => {
                let presence = presence(&ctx.auth);
                let pieces = match crate::costs::CostService::engine() {
                    Some(costs) => costs.pieces().await.unwrap_or_else(|e| {
                        tracing::debug!("limits: reported pieces: {e:#}");
                        Arc::default()
                    }),
                    None => Arc::default(),
                };
                let budgets = budgets::read(&ctx.db, &ctx.data_dir, now, &pieces).await;
                let recent = budgets::recent_providers(&ctx.db, now).await;
                let mut st = self.inner.state.lock();
                st.apply_presence(presence);
                match budgets {
                    Ok(b) => st.budgets = b,
                    Err(e) => tracing::debug!("limits: budgets: {e:#}"),
                }
                match recent {
                    Ok(r) => st.recent = r,
                    Err(e) => tracing::debug!("limits: recent providers: {e:#}"),
                }
                drop(st);
                self.mark_changed();
                Outcome::Done
            }
            Source::Probe => {
                let kicked = self.inner.last_kick_ms.load(Ordering::Relaxed);
                let idle = self
                    .inner
                    .state
                    .lock()
                    .rows
                    .get("anthropic:api_key")
                    .and_then(|r| r.observed_at_ms)
                    .is_none_or(|t| now.saturating_sub(t) > PROBE_IDLE_MS);
                let Some(key) = poll::anthropic_key(&ctx.auth) else {
                    return Outcome::Done;
                };
                if now.saturating_sub(kicked) > PROBE_VISIBLE_MS || !idle {
                    return Outcome::Done;
                }
                match poll::probe(&key, &ctx.http).await {
                    Ok((status, h)) => {
                        self.observe_headers(
                            "anthropic",
                            "api_key",
                            poll::PROBE_MODEL,
                            status,
                            &h,
                            now_ms(),
                        );
                        if status == 429 {
                            Outcome::Throttled
                        } else {
                            Outcome::Done
                        }
                    }
                    Err(e) => {
                        tracing::debug!("limits: probe: {e:?}");
                        Outcome::Done
                    }
                }
            }
            _ => {
                if !poll::has_credential(source, &ctx.auth) {
                    return Outcome::Done;
                }
                let res = tokio::time::timeout(
                    poll::TIMEOUT * 3,
                    poll::fetch(source, &ctx.auth, &ctx.http, now),
                )
                .await
                .unwrap_or_else(|_| Err(PollError::Transient("usage endpoint timed out".into())));
                let res = match res {
                    Ok(None) => return Outcome::Done,
                    Ok(Some(d)) => Ok(d),
                    Err(e) => Err(e),
                };
                let throttled = matches!(res, Err(PollError::Throttled));
                self.inner.state.lock().apply_poll(source, now_ms(), res);
                self.mark_changed();
                if throttled {
                    Outcome::Throttled
                } else {
                    Outcome::Done
                }
            }
        }
    }

    /// Persist the snapshot after each burst of changes, and once at shutdown.
    async fn writer(&self, path: PathBuf, shutdown: tokio_util::sync::CancellationToken) {
        loop {
            tokio::select! {
                () = shutdown.cancelled() => break,
                () = self.inner.changed.notified() => {}
            }
            tokio::select! {
                () = shutdown.cancelled() => break,
                () = tokio::time::sleep(WRITE_DEBOUNCE) => {}
            }
            self.save(&path);
        }
        self.save(&path);
    }

    fn save(&self, path: &Path) {
        if !self.inner.dirty.swap(false, Ordering::Relaxed) {
            return;
        }
        if let Err(e) = save(path, &self.snapshot()) {
            tracing::warn!("limits: snapshot write {}: {e}", path.display());
        }
    }
}

/// Atomic, owner-only (`platform::files::write_private`).
fn save(path: &Path, snap: &UsageLimits) -> std::io::Result<()> {
    if let Some(dir) = path.parent() {
        platform::files::private_dir(dir)?;
    }
    let bytes = serde_json::to_vec(snap).map_err(std::io::Error::other)?;
    platform::files::write_private(path, &bytes)
}

fn load(path: &Path) -> Option<UsageLimits> {
    let bytes = std::fs::read(path).ok()?;
    serde_json::from_slice(&bytes)
        .inspect_err(|e| tracing::warn!("limits: ignoring {}: {e}", path.display()))
        .ok()
}

/// A lazy read-only pool on the engine's ledger (budgets, recent providers).
fn open_db(path: &Path) -> sqlx::SqlitePool {
    let opts = sqlx::sqlite::SqliteConnectOptions::new()
        .filename(path)
        .read_only(true);
    sqlx::sqlite::SqlitePoolOptions::new()
        .max_connections(1)
        .connect_lazy_with(opts)
}

/// What the pollers share.
struct Ctx {
    auth: Auth,
    http: reqwest::Client,
    db: sqlx::SqlitePool,
    data_dir: PathBuf,
}

/// `limits:` in config: `limits.<provider>.poll: false` turns a poller off;
/// `limits.anthropic.probe: true` allows the paid idle probe.
#[derive(Debug, Default, Deserialize)]
#[serde(transparent)]
struct LimitsConfig(HashMap<String, ProviderConfig>);

#[derive(Debug, Deserialize)]
#[serde(default)]
struct ProviderConfig {
    poll: bool,
    probe: bool,
}

impl Default for ProviderConfig {
    fn default() -> Self {
        Self {
            poll: true,
            probe: false,
        }
    }
}

impl LimitsConfig {
    fn poll(&self, provider: &str) -> bool {
        self.0.get(provider).is_none_or(|c| c.poll)
    }

    fn probe(&self, provider: &str) -> bool {
        self.0.get(provider).is_some_and(|c| c.probe)
    }
}

/// The `ChatFrameSink` `Backend::stream` installs on every genai call: hands
/// the response head to [`LimitsHub::global`]. The fork calls `on_response`
/// before its `is_success()` branch, so a 429 is seen too (genai's errors
/// carry no status on theuth's paths — review 160-LIM #4).
pub(crate) struct HeaderTap {
    provider: String,
    auth: &'static str,
    model: String,
}

impl HeaderTap {
    pub(crate) fn new(provider: &str, auth: &'static str, model: &str) -> Arc<Self> {
        Arc::new(Self {
            provider: provider.to_owned(),
            auth,
            model: model.to_owned(),
        })
    }
}

impl ChatFrameSink for HeaderTap {
    fn on_frame(&self, _ctx: &FrameCtx, _frame: RawFrameRef<'_>) {}

    fn on_response(
        &self,
        _ctx: &FrameCtx,
        status: reqwest::StatusCode,
        headers: &reqwest::header::HeaderMap,
    ) {
        LimitsHub::global().observe_headers(
            &self.provider,
            self.auth,
            &self.model,
            status.as_u16(),
            &lower(headers),
            now_ms(),
        );
    }
}

/// The `auth` a genai call used, for its row: a `RequestOverride` target (or
/// xAI's bearer) is a `/login`, Bedrock is AWS, the rest an API key.
pub(crate) fn auth_mode(
    kind: genai::adapter::AdapterKind,
    target_auth: &genai::resolver::AuthData,
    xai_login: bool,
) -> &'static str {
    use genai::adapter::AdapterKind;
    match (kind, target_auth) {
        (_, genai::resolver::AuthData::RequestOverride { .. }) => "oauth",
        (AdapterKind::Xai, _) if xai_login => "oauth",
        (AdapterKind::BedrockSigv4 | AdapterKind::BedrockApi, _) => "aws",
        _ => "api_key",
    }
}

// region: state — pure: every mutation and the render take `now`

/// One row's accumulated knowledge.
#[derive(Clone, Debug, Default)]
struct Row {
    provider: String,
    label: String,
    auth: String,
    account: Option<String>,
    plan: Option<String>,
    /// Status and message for a provider with no source at all (Bedrock, Cursor, Gemini).
    fixed: Option<String>,
    /// A credential exists (a header-created row always has one).
    configured: bool,
    /// Listed by [`presence`] (else created by headers from an unlisted provider).
    known: bool,
    headers: bool,
    poll_every_ms: Option<u64>,
    windows: Vec<UsageWindow>,
    rates: Vec<RateObs>,
    balance: Option<Balance>,
    observed_at_ms: Option<u64>,
    /// 429 deadlines per model scope (`headers::model_group`; `""` = the whole
    /// row, from a poll): a 2xx clears only its own scope (review 160-LIM #3).
    limited: BTreeMap<String, u64>,
    /// The last poll failed: `Stale` (transient) or `Error` (hard) + message.
    failure: Option<(LimitStatus, String)>,
    /// From the snapshot file: `stale` until fresh data arrives.
    restored: bool,
}

/// A rate bucket and the server time it was observed at (merge order).
#[derive(Clone, Debug)]
struct RateObs {
    rate: RateLimit,
    date_ms: u64,
}

#[derive(Default)]
struct State {
    rows: BTreeMap<String, Row>,
    budgets: Vec<BudgetLimit>,
    /// Providers with calls in the last 30 days.
    recent: HashSet<String>,
}

/// Row key: `<provider>:<auth>`, bare for providers with one way in.
fn row_id(provider: &str, auth: &str) -> String {
    match provider {
        "amazon-bedrock" | "cursor" => provider.to_owned(),
        _ => format!("{provider}:{auth}"),
    }
}

fn label(provider: &str, auth: &str) -> String {
    match (provider, auth) {
        ("anthropic", "oauth") => "Claude · subscription".into(),
        ("anthropic", _) => "Anthropic API".into(),
        ("openai", "oauth") => "ChatGPT · Codex".into(),
        ("openai", _) => "OpenAI API".into(),
        ("amazon-bedrock", _) => "Amazon Bedrock".into(),
        ("cursor", _) => "Cursor".into(),
        ("google", _) => "Google Gemini".into(),
        ("openrouter", _) => "OpenRouter".into(),
        ("opencode", _) => "OpenCode Zen".into(),
        ("opencode-go", _) => "OpenCode Go".into(),
        ("deepseek", _) => "DeepSeek".into(),
        ("xai", _) => "xAI".into(),
        ("groq", _) => "Groq".into(),
        (p, "gateway") => format!("{p} · gateway"),
        (p, _) => p.into(),
    }
}

const RATE_ORDER: [&str; 5] = [
    "requests",
    "tokens",
    "input_tokens",
    "output_tokens",
    "project_tokens",
];

fn hhmm(ms: u64) -> String {
    chrono::DateTime::from_timestamp_millis(i64::try_from(ms).unwrap_or(i64::MAX))
        .map_or_else(String::new, |t| t.format("%Y-%m-%d %H:%M UTC").to_string())
}

/// Replace windows by id, keep the rest.
fn merge_windows(into: &mut Vec<UsageWindow>, new: Vec<UsageWindow>) {
    for w in new {
        match into.iter_mut().find(|x| x.id == w.id) {
            Some(x) => *x = w,
            None => into.push(w),
        }
    }
}

impl State {
    fn row_mut(&mut self, provider: &str, auth: &str) -> &mut Row {
        self.rows
            .entry(row_id(provider, auth))
            .or_insert_with(|| Row {
                provider: provider.to_owned(),
                label: label(provider, auth),
                auth: auth.to_owned(),
                configured: true,
                ..Row::default()
            })
    }

    /// §5.3 header merge: per (scope, bucket) the newest server `date` wins,
    /// a tie goes to the lower `remaining`. `true` when anything changed.
    fn observe(
        &mut self,
        provider: &str,
        auth: &str,
        model: &str,
        status: u16,
        h: &[(String, String)],
        now: u64,
    ) -> bool {
        let p = headers::parse(model, status, h, now);
        let ok = (200..300).contains(&status);
        let scope = headers::model_group(model);
        if p.is_empty() {
            // A success after a 429 lifts its scope's limit; nothing else to learn.
            return match self.rows.get_mut(&row_id(provider, auth)) {
                Some(row) if ok => row.limited.remove(&scope).is_some(),
                _ => false,
            };
        }
        let date = p.date_ms.unwrap_or(now);
        let row = self.row_mut(provider, auth);
        row.headers = true;
        row.configured = true;
        row.restored = false;
        row.failure = None;
        row.observed_at_ms = row.observed_at_ms.max(Some(now));
        for rate in p.rates {
            let obs = RateObs {
                rate,
                date_ms: date,
            };
            match row
                .rates
                .iter_mut()
                .find(|o| o.rate.id == obs.rate.id && o.rate.scope == obs.rate.scope)
            {
                Some(old)
                    if obs.date_ms > old.date_ms
                        || (obs.date_ms == old.date_ms
                            && obs.rate.remaining < old.rate.remaining) =>
                {
                    *old = obs;
                }
                Some(_) => {}
                None => row.rates.push(obs),
            }
        }
        merge_windows(&mut row.windows, p.windows);
        if p.balance.is_some() {
            row.balance = p.balance;
        }
        if let Some(until) = p.limited_until_ms {
            row.limited.insert(scope, until);
        } else if ok {
            row.limited.remove(&scope);
        }
        true
    }

    /// One poll's result: data replaces by window id and clears failures;
    /// a failure keeps the last-good values.
    fn apply_poll(&mut self, source: Source, now: u64, res: Result<PollData, PollError>) {
        let Some(id) = source.row() else { return };
        let Some((provider, auth)) = id.split_once(':') else {
            return;
        };
        let row = self.row_mut(provider, auth);
        row.poll_every_ms = Some(source.cadence().0);
        match res {
            Ok(d) => {
                merge_windows(&mut row.windows, d.windows);
                if d.balance.is_some() {
                    row.balance = d.balance;
                }
                row.plan = d.plan.or(row.plan.take());
                row.account = d.account.or(row.account.take());
                if d.limited {
                    row.limited.insert(String::new(), now + source.cadence().0);
                } else {
                    row.limited.remove("");
                }
                row.observed_at_ms = row.observed_at_ms.max(Some(now));
                row.failure = None;
                row.restored = false;
            }
            Err(PollError::Throttled) => {
                row.failure = Some((
                    LimitStatus::Stale,
                    "usage endpoint throttled (429); showing last values".into(),
                ));
            }
            Err(PollError::Transient(m)) => row.failure = Some((LimitStatus::Stale, m)),
            Err(PollError::Hard(m)) => row.failure = Some((LimitStatus::Error, m)),
        }
    }

    /// Re-read credentials: listed rows get their label, sources and
    /// configured flag; a row no longer listed but once listed is unconfigured.
    fn apply_presence(&mut self, presence: Vec<Present>) {
        let listed: HashSet<String> = presence.iter().map(|p| p.id.clone()).collect();
        for row in self.rows.iter_mut().filter(|(id, _)| !listed.contains(*id)) {
            if row.1.known {
                row.1.configured = false;
            }
        }
        for p in presence {
            let row = self.rows.entry(p.id).or_default();
            row.provider = p.provider;
            row.label = p.label;
            row.auth = p.auth.into();
            row.configured = p.configured;
            row.known = true;
            row.fixed = p.fixed;
            row.headers = p.headers;
            row.poll_every_ms = p.poll.map(|s| s.cadence().0);
            if let Some(account) = p.account {
                row.account = Some(account);
            }
        }
    }

    fn status(row: &Row, now: u64) -> (LimitStatus, Option<String>) {
        if let Some(m) = &row.fixed {
            return (LimitStatus::Unavailable, Some(m.clone()));
        }
        if !row.configured {
            return (
                LimitStatus::NotConfigured,
                Some("no credential on this engine; calls seen in the last 30 days".into()),
            );
        }
        if let Some((LimitStatus::Error, m)) = &row.failure {
            return (LimitStatus::Error, Some(m.clone()));
        }
        if let Some(until) = row.limited.values().copied().filter(|t| *t > now).max() {
            return (
                LimitStatus::Limited,
                Some(format!("limited until {}", hhmm(until))),
            );
        }
        let windows = Self::windows_at(row, now);
        if let Some(w) = windows.iter().find(|w| w.used_fraction >= 1.0) {
            let until = w
                .resets_at_ms
                .map_or_else(String::new, |t| format!(" until {}", hhmm(t)));
            return (
                LimitStatus::Limited,
                Some(format!("{} limit reached{until}", w.label)),
            );
        }
        if row.restored {
            return (
                LimitStatus::Stale,
                Some("last known values from before the engine restarted".into()),
            );
        }
        if let Some((_, m)) = &row.failure {
            return (LimitStatus::Stale, Some(m.clone()));
        }
        let stale_after = row.poll_every_ms.map_or(0, |e| 2 * e).max(if row.headers {
            HEADERS_STALE_MS
        } else {
            0
        });
        match row.observed_at_ms {
            None => {
                let m = if row.poll_every_ms.is_some() {
                    "waiting for the first usage check"
                } else {
                    "no calls observed yet; limits arrive with the next call"
                };
                return (LimitStatus::Stale, Some(m.into()));
            }
            Some(t) if now.saturating_sub(t) > stale_after => {
                return (
                    LimitStatus::Stale,
                    Some(format!("no fresh data since {}", hhmm(t))),
                );
            }
            Some(_) => {}
        }
        if let Some(w) = windows.iter().find(|w| w.used_fraction >= 0.75) {
            return (
                LimitStatus::Warn,
                Some(format!("{} at {:.0} %", w.label, w.used_fraction * 100.0)),
            );
        }
        if let Some(r) = Self::rates_at(row, now)
            .iter()
            .find(|r| r.limit > 0 && r.remaining.saturating_mul(100) <= r.limit.saturating_mul(15))
        {
            let scope = r
                .scope
                .as_deref()
                .map_or_else(String::new, |s| format!(" ({s})"));
            return (
                LimitStatus::Warn,
                Some(format!(
                    "{}{scope}: {:.0} % left",
                    r.label,
                    r.remaining as f64 * 100.0 / r.limit as f64
                )),
            );
        }
        (LimitStatus::Ok, None)
    }

    /// Windows as of `now`: a passed reset reads as unused until re-observed.
    fn windows_at(row: &Row, now: u64) -> Vec<UsageWindow> {
        row.windows
            .iter()
            .map(|w| {
                let mut w = w.clone();
                if w.resets_at_ms.is_some_and(|t| t <= now) {
                    w.used_fraction = 0.0;
                }
                w
            })
            .collect()
    }

    /// Buckets as of `now` (a passed reset reads as full), by scope then bucket.
    fn rates_at(row: &Row, now: u64) -> Vec<RateLimit> {
        let mut rates: Vec<RateLimit> = row
            .rates
            .iter()
            .map(|o| {
                let mut r = o.rate.clone();
                if r.resets_at_ms.is_some_and(|t| t <= now) {
                    r.remaining = r.limit;
                }
                r
            })
            .collect();
        let rank = |id: &str| {
            RATE_ORDER
                .iter()
                .position(|x| *x == id)
                .unwrap_or(RATE_ORDER.len())
        };
        rates.sort_by(|a, b| a.scope.cmp(&b.scope).then(rank(&a.id).cmp(&rank(&b.id))));
        rates
    }

    fn render(&self, now: u64) -> UsageLimits {
        let configured: HashSet<&str> = self
            .rows
            .values()
            .filter(|r| r.configured)
            .map(|r| r.provider.as_str())
            .collect();
        let mut unconfigured_shown = HashSet::new();
        let mut providers: Vec<ProviderLimits> = Vec::new();
        for (id, row) in &self.rows {
            if !row.configured
                && (!self.recent.contains(&row.provider)
                    || configured.contains(row.provider.as_str())
                    || !unconfigured_shown.insert(row.provider.clone()))
            {
                continue;
            }
            // A provider seen only in headers, silent for 30 days: gone.
            if !row.known
                && row
                    .observed_at_ms
                    .is_none_or(|t| now.saturating_sub(t) > RECENT_MS)
            {
                continue;
            }
            let (status, message) = Self::status(row, now);
            let source = match (row.headers, row.poll_every_ms.is_some()) {
                (true, true) => "headers + usage endpoint",
                (true, false) => "response headers",
                (false, true) => "usage endpoint",
                (false, false) => "none",
            };
            providers.push(ProviderLimits {
                id: id.clone(),
                provider: row.provider.clone(),
                label: row.label.clone(),
                auth: row.auth.clone(),
                account: row.account.clone(),
                plan: row.plan.clone(),
                status,
                message,
                windows: Self::windows_at(row, now),
                rates: Self::rates_at(row, now),
                balance: row.balance.clone(),
                source: source.into(),
                observed_at_ms: row.observed_at_ms,
            });
        }
        // Rows with something to show first; within a group, by id.
        providers.sort_by_key(|p| {
            matches!(
                p.status,
                LimitStatus::Unavailable | LimitStatus::NotConfigured
            )
        });
        UsageLimits {
            providers,
            budgets: self.budgets.clone(),
            generated_at_ms: now,
        }
    }

    /// Rebuild from a persisted snapshot: everything `stale` until refreshed.
    fn restore(snap: UsageLimits) -> State {
        let mut st = State {
            budgets: snap.budgets,
            ..State::default()
        };
        for p in snap.providers {
            let poll = Source::ALL
                .into_iter()
                .find(|s| s.row() == Some(p.id.as_str()));
            let row = Row {
                fixed: (p.status == LimitStatus::Unavailable)
                    .then(|| p.message.clone().unwrap_or_default()),
                configured: p.status != LimitStatus::NotConfigured,
                known: false,
                headers: p.source.contains("headers"),
                poll_every_ms: if p.source.contains("usage endpoint") {
                    poll.map(|s| s.cadence().0)
                } else {
                    None
                },
                rates: p
                    .rates
                    .into_iter()
                    .map(|rate| RateObs {
                        rate,
                        date_ms: p.observed_at_ms.unwrap_or(0),
                    })
                    .collect(),
                windows: p.windows,
                balance: p.balance,
                observed_at_ms: p.observed_at_ms,
                restored: true,
                provider: p.provider,
                label: p.label,
                auth: p.auth,
                account: p.account,
                plan: p.plan,
                limited: BTreeMap::new(),
                failure: None,
            };
            st.rows.insert(p.id, row);
        }
        st
    }
}

// endregion

/// One row [`presence`] lists.
#[derive(Clone, Debug)]
struct Present {
    id: String,
    provider: String,
    label: String,
    auth: &'static str,
    configured: bool,
    fixed: Option<String>,
    headers: bool,
    poll: Option<Source>,
    account: Option<String>,
}

/// Every provider row theuth can use on this engine (§5.1), configured or not.
/// No network, no token refresh.
fn presence(auth: &Auth) -> Vec<Present> {
    let row = |provider: &str, a: &'static str, configured: bool| Present {
        id: row_id(provider, a),
        provider: provider.to_owned(),
        label: label(provider, a),
        auth: a,
        configured,
        fixed: None,
        headers: true,
        poll: None,
        account: None,
    };
    let login_account = |ns: &str| {
        auth.oauth
            .load(ns)
            .and_then(|t| t.account_id)
            .map(|a| poll::truncate_account(&a))
    };
    let configured = crate::supervisor::configured(auth.clone());
    let mut out = vec![
        row("anthropic", "api_key", poll::anthropic_key(auth).is_some()),
        Present {
            poll: Some(Source::Claude),
            account: login_account("anthropic"),
            ..row("anthropic", "oauth", auth.oauth.load("anthropic").is_some())
        },
        Present {
            poll: Some(Source::Codex),
            account: login_account("openai"),
            ..row("openai", "oauth", auth.oauth.load("openai").is_some())
        },
        row(
            "openai",
            "api_key",
            poll::key(auth, "openai", "OPENAI_API_KEY").is_some(),
        ),
        Present {
            headers: false,
            fixed: Some(
                "Bedrock exposes no usage meter to theuth; quotas live in AWS Service Quotas"
                    .into(),
            ),
            ..row("amazon-bedrock", "aws", configured("bedrock_sigv4"))
        },
        Present {
            headers: false,
            fixed: Some("Cursor exposes no usage meter theuth can read yet".into()),
            ..row("cursor", "login", auth.oauth.load("cursor").is_some())
        },
        Present {
            headers: false,
            fixed: Some(
                "Gemini sends no rate-limit headers; quotas live in AI Studio / Cloud Console"
                    .into(),
            ),
            ..row(
                "google",
                "api_key",
                poll::key(auth, "gemini", "GEMINI_API_KEY").is_some(),
            )
        },
        Present {
            headers: false,
            poll: Some(Source::OpenRouter),
            ..row("openrouter", "api_key", auth.openrouter_key().is_some())
        },
        Present {
            headers: false,
            poll: Some(Source::OpencodeGo),
            ..row(
                "opencode-go",
                "api_key",
                poll::key(auth, "opencode_go", "OPENCODE_GO_API_KEY").is_some(),
            )
        },
        Present {
            headers: false,
            poll: Some(Source::DeepSeek),
            ..row(
                "deepseek",
                "api_key",
                poll::key(auth, "deepseek", "DEEPSEEK_API_KEY").is_some(),
            )
        },
    ];
    if let Ok(gateways) = auth.gateways.read() {
        out.extend(gateways.keys().map(|name| row(name, "gateway", true)));
    }
    out
}

#[cfg(test)]
mod tests;
