//! Replay (44 §3.5): route + params + the session's credentials →
//! `Fetcher::request` → drift check → records. Errors carry status and
//! reason, never the request.

use std::time::{Duration, Instant};

use runtime::SessionId;
use serde_json::{Map, Value};
use zeroize::Zeroizing;

use crate::classify::{decode, drift, fill};
use crate::{CallReply, Routes};

/// Run `route_id` with `params` for `session`.
///
/// # Errors
/// Malformed id, unknown route, `replay: false`, missing path params,
/// transport failure or timeout, and drift — which also marks the route
/// `stale`.
pub async fn call(
    routes: &Routes,
    session: SessionId,
    route_id: &str,
    params: &Map<String, Value>,
) -> anyhow::Result<CallReply> {
    let Some((domain, _)) = route_id.split_once(':') else {
        anyhow::bail!("bad route id {route_id:?}: expected domain:METHOD:path");
    };
    let site = routes.store.load(domain).await?;
    let Some(route) = site.routes.iter().find(|r| r.id == route_id) else {
        anyhow::bail!("unknown route {route_id:?}; site_routes lists the learned ones");
    };
    if !route.replay {
        anyhow::bail!("route {route_id} is not replayable: POST body is not captured (44 §2)");
    }
    let url = fill(route, params).map_err(|e| anyhow::anyhow!(e))?;

    let mut headers: Zeroizing<Vec<(String, String)>> = Zeroizing::new(vec![
        (
            "accept".to_owned(),
            "application/json, text/plain, */*".to_owned(),
        ),
        ("accept-language".to_owned(), "en-US,en;q=0.9".to_owned()),
        ("referer".to_owned(), format!("https://{domain}/")),
    ]);
    let mut unarmed = Vec::new();
    {
        let mut creds = routes.creds.lock().await;
        let armed: &[(String, Zeroizing<String>)] =
            match creds.get_mut(&(session, domain.to_owned())) {
                Some(e) => {
                    e.touched = Instant::now();
                    &e.headers
                }
                None => &[],
            };
        for name in &route.auth_headers {
            match armed.iter().find(|(n, _)| n == name) {
                Some((_, v)) => headers.push((name.clone(), v.as_str().to_owned())),
                None => unarmed.push(name.clone()),
            }
        }
    }

    let resp = tokio::time::timeout(
        Duration::from_millis(routes.cfg.call_timeout_ms),
        routes
            .fetcher
            .request(&route.method, url.as_str(), &headers, None),
    )
    .await
    .map_err(|_| {
        anyhow::anyhow!(
            "site_call timed out after {} ms",
            routes.cfg.call_timeout_ms
        )
    })??;

    let content_type = resp
        .headers
        .iter()
        .find(|(n, _)| n == "content-type")
        .map(|(_, v)| {
            v.split_once(';')
                .map_or(v.as_str(), |(t, _)| t)
                .trim()
                .to_ascii_lowercase()
        });
    let body = String::from_utf8_lossy(&resp.body);
    let checked = decode(&body, content_type.as_deref())
        .ok_or_else(|| "body is not JSON".to_owned())
        .and_then(|v| drift(route, &v));
    match checked {
        Ok(records) => {
            routes.store.mark(domain, route_id, true).await?;
            Ok(CallReply {
                url: url.into(),
                status: resp.status,
                count: records.len(),
                pointer: route.collection.clone(),
                records,
                unarmed,
            })
        }
        Err(reason) => {
            routes.store.mark(domain, route_id, false).await?;
            let auth_hint = if matches!(resp.status, 401 | 403) && !unarmed.is_empty() {
                format!(
                    " — {} not armed: open the site in this session and site_learn again",
                    unarmed.join(", ")
                )
            } else {
                String::new()
            };
            anyhow::bail!(
                "route drifted (status {}, {reason}); re-learn with site_learn{auth_hint}",
                resp.status
            )
        }
    }
}
