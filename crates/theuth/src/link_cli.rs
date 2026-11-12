//! `t --link`: pair a phone from a box with no TUI (design doc 31 §6.6).
//! An RPC client of the running daemon: `link_pair`, print the QR and the
//! key, wait for `LinkPaired` on `subscribe_events`, exit.

use std::path::Path;

use jsonrpsee::core::client::{ClientT, Subscription, SubscriptionClientT};
use jsonrpsee::rpc_params;
use jsonrpsee::ws_client::{HeaderMap, HeaderValue, WsClient, WsClientBuilder};

/// Connect to the running engine (63 §2.1 record; else `rpc.listen`) with
/// `<data_dir>/token`, start a pairing, print it, wait for the scan. Exits 1
/// with a hint when no daemon answers.
pub async fn run(data_dir: &Path, config: &figment::Figment) -> anyhow::Result<()> {
    let listen = rpc::engine::discover(data_dir)
        .and_then(|r| r.record.listen)
        .unwrap_or_else(|| listen(config));
    let Ok(client) = connect(data_dir, &listen).await else {
        eprintln!("no harness on {listen}; start `t --headless` first");
        std::process::exit(1);
    };
    pair(&client, false).await
}

/// `rpc.listen` from the config, default when unset.
pub(crate) fn listen(config: &figment::Figment) -> String {
    config
        .extract_inner::<rpc::RpcConfig>("rpc")
        .unwrap_or_default()
        .listen
}

/// `link_pair` on the daemon, print QR + key, wait for `LinkPaired`.
/// `wait_for_parent` (32 §5.1 step 5): a `theuth` pairing ends the wait; a
/// phone or laptop is announced, and since a pairing is single use (31
/// §4.5) a fresh key is issued and the wait goes on until that one expires.
pub async fn pair(client: &WsClient, wait_for_parent: bool) -> anyhow::Result<()> {
    // Subscribe before `link_pair` so the events it publishes are not missed.
    let mut events: Subscription<serde_json::Value> = client
        .subscribe(
            "subscribe_events",
            rpc_params![serde_json::Value::Null, serde_json::Value::Null],
            "unsubscribe_events",
        )
        .await?;
    let show = async |client: &WsClient| -> anyhow::Result<()> {
        let pairing: link::Pairing = client.request("link_pair", rpc_params![]).await?;
        println!("{}", link::qr(&pairing.key));
        println!("{}", pairing.key);
        if wait_for_parent {
            println!("waiting for the pairing… (Ctrl+C to skip; later: t --link)");
        } else {
            println!("scan with your phone (expires in 5:00)");
        }
        Ok(())
    };
    show(client).await?;

    // A phone paired: the key is spent; the `LinkPairing { key: None }` that
    // follows `LinkPaired` is the moment to issue the next one.
    let mut reissue = false;
    while let Some(ev) = events.next().await {
        let ev = ev?;
        match ev.get("type").and_then(|t| t.as_str()) {
            Some("LinkPaired") => {
                let name = ev
                    .pointer("/device/name")
                    .and_then(|n| n.as_str())
                    .unwrap_or("device");
                let is_theuth =
                    ev.pointer("/device/platform").and_then(|p| p.as_str()) == Some("theuth");
                if wait_for_parent && is_theuth {
                    println!("paired to {name} as a node");
                    return Ok(());
                }
                println!("paired: {name}");
                if !wait_for_parent {
                    return Ok(());
                }
                reissue = true;
            }
            Some("LinkPairing") if ev.get("key").is_none_or(serde_json::Value::is_null) => {
                if !reissue {
                    println!("pairing expired");
                    return Ok(());
                }
                reissue = false;
                show(client).await?;
            }
            _ => {}
        }
    }
    anyhow::bail!("harness closed the connection")
}

/// A WebSocket client authenticated with the daemon's bearer token.
pub(crate) async fn connect(data_dir: &Path, listen: &str) -> anyhow::Result<WsClient> {
    connect_with(data_dir, listen, WsClientBuilder::default()).await
}

/// [`connect`] with a per-request timeout (jsonrpsee's default is 60 s).
pub(crate) async fn connect_with_timeout(
    data_dir: &Path,
    listen: &str,
    timeout: std::time::Duration,
) -> anyhow::Result<WsClient> {
    connect_with(
        data_dir,
        listen,
        WsClientBuilder::default().request_timeout(timeout),
    )
    .await
}

async fn connect_with(
    data_dir: &Path,
    listen: &str,
    builder: WsClientBuilder,
) -> anyhow::Result<WsClient> {
    let token = std::fs::read_to_string(data_dir.join("token"))?;
    let mut headers = HeaderMap::new();
    headers.insert(
        "Authorization",
        HeaderValue::from_str(&format!("Bearer {}", token.trim()))?,
    );
    Ok(builder
        .set_headers(headers)
        .build(format!("ws://{listen}"))
        .await?)
}
