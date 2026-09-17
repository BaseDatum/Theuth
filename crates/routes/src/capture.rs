//! Browser side of a learn (42 §3.2): navigate, drain
//! `browser_network_capture`, read the jar. The `browser` actor is looked up
//! at call time (38 §0.3); no actor → a clear error, never a panic.

use kameo::actor::ActorRef;
use runtime::SessionId;
use serde_json::{Value, json};
use url::Url;

use crate::classify::Exchange;

fn browser_actor() -> anyhow::Result<ActorRef<browser::Browsers>> {
    ActorRef::<browser::Browsers>::lookup(browser::BROWSER_ACTOR)
        .ok()
        .flatten()
        .ok_or_else(|| anyhow::anyhow!("browser not available; enable browser: to learn routes"))
}

async fn call(session: SessionId, name: &str, args: Value) -> anyhow::Result<String> {
    let result = browser_actor()?
        .ask(browser::Call {
            session,
            name: name.to_owned(),
            args,
        })
        .await
        .map_err(|e| anyhow::anyhow!("{e}"))?;
    let text = result["content"]
        .as_array()
        .into_iter()
        .flatten()
        .filter_map(|c| c["text"].as_str())
        .collect::<Vec<_>>()
        .join("\n");
    if result["isError"].as_bool() == Some(true) {
        anyhow::bail!(text.strip_prefix("Error: ").unwrap_or(&text).to_owned());
    }
    Ok(text)
}

/// `browser_navigate { url, waitUntil: "networkidle0" }` on this session's isolate.
pub async fn navigate(session: SessionId, url: &Url) -> anyhow::Result<()> {
    call(
        session,
        "browser_navigate",
        json!({ "url": url.as_str(), "waitUntil": "networkidle0" }),
    )
    .await?;
    Ok(())
}

/// Drain the session's capture buffer (`browser_network_capture {}`, which
/// clears by default) into rows.
pub async fn exchanges(session: SessionId) -> anyhow::Result<Vec<Exchange>> {
    let text = call(session, "browser_network_capture", json!({})).await?;
    let text = text.trim();
    if text.is_empty() {
        return Ok(Vec::new());
    }
    serde_json::from_str(text)
        .map_err(|e| anyhow::anyhow!("bad browser_network_capture reply: {e}"))
}

/// `Cookie` header value for `domain` from the session's jar
/// (`browser_storage_state {}`): every cookie whose `domain` (leading dot
/// stripped) is `domain` or a subdomain of it, as `name=value; …`. `None`
/// on any error or when nothing matches. The value is a credential: the
/// caller wraps it and never logs it.
pub async fn cookie_header(session: SessionId, domain: &str) -> Option<String> {
    let text = call(session, "browser_storage_state", json!({}))
        .await
        .ok()?;
    let state: Value = serde_json::from_str(&text).ok()?;
    let header = state["cookies"]
        .as_array()?
        .iter()
        .filter(|c| {
            c["domain"]
                .as_str()
                .map(|d| d.trim_start_matches('.'))
                .is_some_and(|d| {
                    d == domain || d.strip_suffix(domain).is_some_and(|p| p.ends_with('.'))
                })
        })
        .filter_map(|c| Some(format!("{}={}", c["name"].as_str()?, c["value"].as_str()?)))
        .collect::<Vec<_>>()
        .join("; ");
    (!header.is_empty()).then_some(header)
}
