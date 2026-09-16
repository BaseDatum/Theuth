//! `wth --link`: pair a phone from a box with no TUI (design doc 31 §6.6).
//! An RPC client of the running daemon: `link_pair`, print the QR and the
//! key, wait for `LinkPaired` on `subscribe_events`, exit.

use std::path::Path;

use jsonrpsee::core::client::{ClientT, Subscription, SubscriptionClientT};
use jsonrpsee::rpc_params;
use jsonrpsee::ws_client::{HeaderMap, HeaderValue, WsClientBuilder};

/// Connect to `rpc.listen` with `<data_dir>/token`, start a pairing, print
/// it, wait for the scan. Exits 1 with a hint when no daemon answers.
pub async fn run(data_dir: &Path, config: &figment::Figment) -> anyhow::Result<()> {
    let listen = config
        .extract_inner::<rpc::RpcConfig>("rpc")
        .unwrap_or_default()
        .listen;
    let Ok(client) = connect(data_dir, &listen).await else {
        eprintln!("no harness on {listen}; start `wth --headless` first");
        std::process::exit(1);
    };

    // Subscribe before `link_pair` so the events it publishes are not missed.
    let mut events: Subscription<serde_json::Value> = client
        .subscribe(
            "subscribe_events",
            rpc_params![serde_json::Value::Null, serde_json::Value::Null],
            "unsubscribe_events",
        )
        .await?;
    let pairing: link::Pairing = client.request("link_pair", rpc_params![]).await?;
    println!("{}", link::qr(&pairing.key));
    println!("{}", pairing.key);
    println!("scan with your phone (expires in 5:00)");

    while let Some(ev) = events.next().await {
        let ev = ev?;
        match ev.get("type").and_then(|t| t.as_str()) {
            Some("LinkPaired") => {
                let name = ev
                    .pointer("/device/name")
                    .and_then(|n| n.as_str())
                    .unwrap_or("device");
                println!("paired: {name}");
                return Ok(());
            }
            Some("LinkPairing") if ev.get("key").is_none_or(serde_json::Value::is_null) => {
                println!("pairing expired");
                return Ok(());
            }
            _ => {}
        }
    }
    anyhow::bail!("harness closed the connection")
}

/// A WebSocket client authenticated with the daemon's bearer token.
async fn connect(data_dir: &Path, listen: &str) -> anyhow::Result<jsonrpsee::ws_client::WsClient> {
    let token = std::fs::read_to_string(data_dir.join("token"))?;
    let mut headers = HeaderMap::new();
    headers.insert(
        "Authorization",
        HeaderValue::from_str(&format!("Bearer {}", token.trim()))?,
    );
    Ok(WsClientBuilder::default()
        .set_headers(headers)
        .build(format!("ws://{listen}"))
        .await?)
}
