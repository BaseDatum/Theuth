//! 210 §7.4 item 2 (G4): the tray's "Stop public sharing (N)" calls the local engine's `share_stop("all")` from the
//! shell itself, over the same plain-HTTP JSON-RPC `models.rs` uses (the daemon serves HTTP next to WebSocket;
//! `?token=` authorises, and a token holder is the owner). So Stop all works with the main window hidden, closed
//! (close only hides it) or its webview wedged. The outcome goes to a native notification and, as
//! [`STOPPED_EVENT`], to the webview, whose strip then says when the edge stop is still pending.

use std::time::Duration;

use serde::{Deserialize, Serialize};
use tauri::{AppHandle, Emitter, Manager};
use tauri_plugin_notification::NotificationExt;

use crate::launch;

/// The webview's `store/public-shares` takes the [`Stopped`] from this event (`lib/tauri.ts` `onShareStoppedAll`).
pub(crate) const STOPPED_EVENT: &str = "share-stopped-all";

/// `share_stop("all")` stops the local shares, then tries the edge three times (0 / 0.5 / 2 s, FIX-A M7): well under this.
const RPC_TIMEOUT: Duration = Duration::from_secs(30);

/// `share::rpc_share::Stopped`.
#[derive(Debug, Clone, PartialEq, Deserialize, Serialize)]
pub(crate) struct Stopped {
    pub stopped: u64,
    /// Why the edge did not take the mesh-wide stop yet; the engine sends it again at the next welcome.
    #[serde(default)]
    pub edge_pending: Option<String>,
}

/// Run Stop all in the background: find the local engine (never spawns one), call it, tell the user.
pub(crate) fn stop_all(app: &AppHandle) {
    let app = app.clone();
    tauri::async_runtime::spawn(async move {
        let peek = {
            let app = app.clone();
            tokio::task::spawn_blocking(move || app.state::<launch::Launch>().peek())
        };
        let result = match peek.await {
            Ok(Some(conn)) => call_stop_all(&conn.url).await,
            Ok(None) => Err(anyhow::anyhow!("theuth is not running")),
            Err(e) => Err(e.into()),
        };
        if let Ok(s) = &result
            && let Err(e) = app.emit(STOPPED_EVENT, s)
        {
            tracing::warn!(error = %e, "emit {STOPPED_EVENT}");
        }
        let (title, body) = outcome_text(&result);
        if let Err(e) = app.notification().builder().title(title).body(body).show() {
            tracing::warn!(error = %e, "share stop notification");
        }
    });
}

/// `share_stop("all")` as one JSON-RPC call over plain HTTP to the daemon at `ws_url` (`ws://<listen>/?token=<token>`).
pub(crate) async fn call_stop_all(ws_url: &str) -> anyhow::Result<Stopped> {
    #[derive(Deserialize)]
    struct Reply {
        #[serde(default)]
        result: Option<Stopped>,
        #[serde(default)]
        error: Option<RpcError>,
    }
    #[derive(Deserialize)]
    struct RpcError {
        message: String,
    }
    let url = ws_url.replacen("ws://", "http://", 1);
    let reply: Reply = reqwest::Client::builder()
        .timeout(RPC_TIMEOUT)
        .build()?
        .post(url)
        .json(&serde_json::json!({ "jsonrpc": "2.0", "id": 1, "method": "share_stop", "params": ["all"] }))
        .send()
        .await?
        .error_for_status()?
        .json()
        .await?;
    match (reply.result, reply.error) {
        (_, Some(e)) => anyhow::bail!("{}", e.message),
        (Some(s), None) => Ok(s),
        (None, None) => anyhow::bail!("share_stop answered nothing"),
    }
}

/// Notification title and body for the outcome.
pub(crate) fn outcome_text(r: &anyhow::Result<Stopped>) -> (String, String) {
    match r {
        Ok(Stopped { stopped, edge_pending: None }) => (
            "Public sharing stopped".into(),
            match stopped {
                1 => "Stopped 1 public share.".into(),
                n => format!("Stopped {n} public shares."),
            },
        ),
        Ok(Stopped { stopped, edge_pending: Some(why) }) => (
            "Public sharing: edge stop pending".into(),
            format!("Stopped {stopped} here. The edge has not confirmed the stop ({why}); theuth retries when it reconnects."),
        ),
        Err(e) => ("Stop public sharing failed".into(), format!("{e:#}")),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use tokio::io::{AsyncReadExt, AsyncWriteExt};

    /// One HTTP/1.1 exchange: answers `body`, returns the daemon URL and the request it saw.
    async fn daemon_once(body: &'static str) -> (String, tokio::task::JoinHandle<String>) {
        let l = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let url = format!("ws://{}/?token=tok", l.local_addr().unwrap());
        let seen = tokio::spawn(async move {
            let (mut s, _) = l.accept().await.unwrap();
            let mut req = Vec::new();
            let mut buf = [0u8; 4096];
            // Headers, then the JSON body (one small request: read until the body's closing brace).
            while !req.ends_with(b"}") {
                let n = s.read(&mut buf).await.unwrap();
                if n == 0 {
                    break;
                }
                req.extend_from_slice(&buf[..n]);
            }
            let head = format!(
                "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",
                body.len()
            );
            s.write_all(head.as_bytes()).await.unwrap();
            s.write_all(body.as_bytes()).await.unwrap();
            String::from_utf8(req).unwrap()
        });
        (url, seen)
    }

    #[tokio::test]
    async fn stop_all_posts_share_stop_all_with_the_token_and_reads_edge_pending() {
        let (url, seen) =
            daemon_once(r#"{"jsonrpc":"2.0","id":1,"result":{"stopped":2,"edge_pending":"edge detached"}}"#).await;
        let got = call_stop_all(&url).await.unwrap();
        assert_eq!(got, Stopped { stopped: 2, edge_pending: Some("edge detached".into()) });
        let req = seen.await.unwrap();
        assert!(req.starts_with("POST /?token=tok HTTP/1.1"), "{req}");
        assert!(req.contains(r#""method":"share_stop""#) && req.contains(r#""params":["all"]"#), "{req}");
        let (title, body) = outcome_text(&Ok(got));
        assert_eq!(title, "Public sharing: edge stop pending");
        assert!(body.contains("Stopped 2 here") && body.contains("edge detached"), "{body}");
    }

    #[tokio::test]
    async fn stop_all_reports_an_rpc_error_and_a_clean_stop() {
        let (url, _) = daemon_once(r#"{"jsonrpc":"2.0","id":1,"error":{"code":-32000,"message":"no share engine"}}"#).await;
        let err = call_stop_all(&url).await.unwrap_err();
        assert_eq!(err.to_string(), "no share engine");
        assert_eq!(
            outcome_text(&Ok(Stopped { stopped: 1, edge_pending: None })),
            ("Public sharing stopped".into(), "Stopped 1 public share.".into())
        );
    }
}
