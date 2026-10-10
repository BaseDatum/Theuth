//! ICS subscription fetch (158 §7.3): GET the vault URL `calendar.ics.<id>` with
//! `If-None-Match` / `If-Modified-Since`, 30 s timeout, 10 MB cap; the URL never appears in a
//! log or an error (replaced by host + masked tail, then `Vault::redact`). Errors land on the
//! source row; the previous rows stay until a fetch succeeds.

use std::time::Duration;

use reqwest::StatusCode;
use runtime::Runtime;

use super::store::{self, FetchMeta};
use super::{CalEvent, CalSource, ics};
use crate::Store;

/// Per-request wall clock.
const TIMEOUT: Duration = Duration::from_secs(30);
/// Body cap.
const MAX_BYTES: usize = 10 * 1024 * 1024;
/// `cal_sources.last_error` for a 401/403/404/410.
pub const REJECTED: &str = "URL rejected — regenerate the secret address in your calendar";

/// The vault row holding `source`'s URL.
pub fn secret_name(source_id: &str) -> String {
    format!("calendar.ics.{source_id}")
}

/// `webcal://` → `https://`; HTTPS only, except `localhost` / `127.0.0.1` (tests, local relays).
pub fn normalise_url(raw: &str) -> anyhow::Result<String> {
    let raw = raw.trim();
    let raw = raw
        .strip_prefix("webcal://")
        .map(|r| format!("https://{r}"))
        .unwrap_or_else(|| raw.to_owned());
    let u = url::Url::parse(&raw).map_err(|_| anyhow::anyhow!("not a URL"))?;
    let host = u.host_str().unwrap_or("");
    let local = matches!(host, "localhost" | "127.0.0.1" | "[::1]");
    match u.scheme() {
        "https" => {}
        "http" if local => {}
        "http" => anyhow::bail!("the subscription address must be https://"),
        other => anyhow::bail!("unsupported scheme {other}://"),
    }
    if host.is_empty() {
        anyhow::bail!("the URL has no host");
    }
    Ok(u.to_string())
}

/// What may be shown of a URL: the host and the last four characters of the path.
pub fn masked(url: &str) -> String {
    match url::Url::parse(url) {
        Ok(u) => {
            let path = u.path();
            let tail: String = path
                .chars()
                .rev()
                .take(4)
                .collect::<Vec<_>>()
                .into_iter()
                .rev()
                .collect();
            format!("{}/…{tail}", u.host_str().unwrap_or("?"))
        }
        Err(_) => "<url>".into(),
    }
}

/// Outcome of one conditional GET.
#[derive(Debug)]
pub enum Fetched {
    /// `304`: nothing to do.
    NotModified,
    /// `200`: the rows inside the window and the new validators.
    Rows(Vec<CalEvent>, FetchMeta),
}

/// One conditional GET + parse, for `source_id`'s rows inside `window`. Errors never carry
/// the URL.
pub async fn fetch_url(
    url: &str,
    meta: &FetchMeta,
    source_id: &str,
    window: (chrono::DateTime<chrono::Utc>, chrono::DateTime<chrono::Utc>),
) -> anyhow::Result<Fetched> {
    let shown = masked(url);
    let scrub = |e: String| e.replace(url, &shown);
    let client = reqwest::Client::builder()
        .timeout(TIMEOUT)
        .user_agent("theuth-calendar/1")
        .build()
        .map_err(|e| anyhow::anyhow!(scrub(e.to_string())))?;
    let mut req = client.get(url);
    if let Some(etag) = &meta.etag {
        req = req.header(reqwest::header::IF_NONE_MATCH, etag);
    }
    if let Some(lm) = &meta.last_modified {
        req = req.header(reqwest::header::IF_MODIFIED_SINCE, lm);
    }
    let resp = req
        .send()
        .await
        .map_err(|e| anyhow::anyhow!("{} ({shown})", scrub(e.without_url().to_string())))?;
    let status = resp.status();
    if status == StatusCode::NOT_MODIFIED {
        return Ok(Fetched::NotModified);
    }
    if matches!(
        status,
        StatusCode::UNAUTHORIZED | StatusCode::FORBIDDEN | StatusCode::NOT_FOUND | StatusCode::GONE
    ) {
        anyhow::bail!("{REJECTED}");
    }
    if !status.is_success() {
        anyhow::bail!("HTTP {} from {shown}", status.as_u16());
    }
    let header = |n: reqwest::header::HeaderName| {
        resp.headers()
            .get(n)
            .and_then(|v| v.to_str().ok())
            .map(str::to_owned)
    };
    let new_meta = FetchMeta {
        etag: header(reqwest::header::ETAG),
        last_modified: header(reqwest::header::LAST_MODIFIED),
    };
    let bytes = resp
        .bytes()
        .await
        .map_err(|e| anyhow::anyhow!(scrub(e.without_url().to_string())))?;
    if bytes.len() > MAX_BYTES {
        anyhow::bail!("calendar is larger than 10 MB ({shown})");
    }
    let text = String::from_utf8_lossy(&bytes);
    if !text.trim_start().starts_with("BEGIN:VCALENDAR") {
        anyhow::bail!("not an iCalendar document ({shown})");
    }
    let rows = ics::expand(&text, source_id, window.0, window.1)?;
    Ok(Fetched::Rows(rows, new_meta))
}

/// Fetch and expand one subscription for the store's window; `Ok(None)` = not modified.
/// Reads the URL from the vault and the validators from the store; writes nothing — see
/// [`refresh`] for the full cycle.
pub async fn fetch(rt: &Runtime, source: &CalSource) -> anyhow::Result<Option<Vec<CalEvent>>> {
    let (store, _) = super::handles().await?;
    let url = url_of(rt, &source.id).await?;
    let meta = store::fetch_meta(&store, &source.id).await?;
    match fetch_url(&url, &meta, &source.id, super::window()).await? {
        Fetched::NotModified => Ok(None),
        Fetched::Rows(rows, _) => Ok(Some(rows)),
    }
}

/// The subscription URL from the vault.
pub async fn url_of(rt: &Runtime, source_id: &str) -> anyhow::Result<String> {
    let vault = secrets::handle(rt)
        .await
        .ok_or_else(|| anyhow::anyhow!("secrets vault unavailable"))?;
    vault
        .get(&secret_name(source_id), 0)
        .map(|z| z.to_string())
        .ok_or_else(|| anyhow::anyhow!("the subscription's URL is missing from the vault"))
}

/// The whole cycle for one source: GET, replace the window's rows, record validators or the
/// error on the row. Returns the row as it is afterwards; `Err` only when the row is gone.
pub async fn refresh(rt: &Runtime, store: &Store, source_id: &str) -> anyhow::Result<CalSource> {
    let window = super::window();
    let outcome = async {
        let url = url_of(rt, source_id).await?;
        let meta = store::fetch_meta(store, source_id).await?;
        match fetch_url(&url, &meta, source_id, window).await? {
            Fetched::NotModified => store::mark_fetched(store, source_id, &meta).await,
            Fetched::Rows(rows, new_meta) => {
                let (from, to) = (store::stamp(window.0), store::stamp(window.1));
                store::replace_window(store, source_id, (&from, &to), &rows).await?;
                store::mark_fetched(store, source_id, &new_meta).await
            }
        }
    }
    .await;
    if let Err(e) = outcome {
        let msg = match secrets::handle(rt).await {
            Some(v) => v.redact(&format!("{e:#}")).into_owned(),
            None => format!("{e:#}"),
        };
        tracing::warn!(source = source_id, "calendar: fetch failed: {msg}");
        store::mark_failed(store, source_id, &msg).await?;
    }
    store::source(store, source_id)
        .await?
        .ok_or_else(|| anyhow::anyhow!("calendar source {source_id} is gone"))
}
