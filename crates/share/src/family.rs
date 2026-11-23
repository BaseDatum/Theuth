//! 210 §5.7, §6.2 (F-M1, F-M4): this engine's view of the family for sharing: who holds the
//! lease, and one JSON-RPC call to it over `theuth/rpc/1` (`share_delegate`,
//! `share_name_key_ensure`, `share_names_rotate`). The call is the same line protocol as
//! `agent::rpc_forward::forward`, written here so `share` does not depend on `agent`; the
//! holder's `theuth/rpc/1` server (agent) dispatches it as an internal peer.

use std::time::Duration;

use anyhow::Context as _;
use serde::Serialize;
use serde::de::DeserializeOwned;
use tokio::io::{AsyncBufReadExt as _, AsyncReadExt as _, AsyncWriteExt as _, BufReader};

/// One forwarded call's answer budget.
pub const FORWARD_TIMEOUT: Duration = Duration::from_secs(90);

/// Reply line cap.
const MAX_LINE: u64 = 1024 * 1024;

/// Who answers holder-only calls.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Holder {
    /// This engine is the synced lease holder (or a lone engine, its own holder).
    Me,
    /// Another engine, by id.
    Peer(String),
}

/// The lease holder now; `Err` while no engine holds it or before the first sync.
pub async fn holder() -> anyhow::Result<Holder> {
    if mesh::share::is_lease_holder().await {
        return Ok(Holder::Me);
    }
    let m = mesh::actor().context("the mesh is not running")?;
    let s = m
        .ask(mesh::Status)
        .await
        .map_err(|e| anyhow::anyhow!("mesh status: {e}"))?;
    match s.lease.holder {
        Some(h) if h == s.self_id => anyhow::bail!("this engine holds the lease but has not synced yet"),
        Some(h) => Ok(Holder::Peer(h.0)),
        None => anyhow::bail!("no machine of the mesh holds the lease right now"),
    }
}

/// `method(params)` on `peer` over `theuth/rpc/1`, once, no second hop.
pub async fn call<T: DeserializeOwned>(
    peer: &str,
    method: &str,
    params: impl Serialize,
) -> anyhow::Result<T> {
    let transport = replica::actor()
        .context("replication is not running")?
        .ask(replica::GetTransport)
        .await
        .ok()
        .flatten()
        .context("the mesh transport is not up")?;
    let peer = replica::PeerId(peer.to_owned());
    anyhow::ensure!(transport.peers().contains(&peer), "the lease holder {peer} is not connected");
    let exchange = async {
        let stream = transport.open(&peer, replica::ALPN_RPC).await?;
        let (read, mut write) = tokio::io::split(stream);
        let mut line = serde_json::to_string(&serde_json::json!({
            "jsonrpc": "2.0", "id": 1, "method": method, "params": params,
        }))?;
        line.push('\n');
        write.write_all(line.as_bytes()).await?;
        write.flush().await?;
        let mut reply = String::new();
        BufReader::new(read).take(MAX_LINE).read_line(&mut reply).await?;
        anyhow::ensure!(!reply.is_empty(), "{peer} closed the stream without answering");
        Ok::<_, anyhow::Error>(reply)
    };
    let reply = tokio::time::timeout(FORWARD_TIMEOUT, exchange)
        .await
        .with_context(|| format!("{method} on {peer}: no answer within {FORWARD_TIMEOUT:?}"))??;
    let v: serde_json::Value = serde_json::from_str(reply.trim_end()).context("malformed reply")?;
    if let Some(e) = v.get("error") {
        anyhow::bail!(
            "{method} on {peer}: {}",
            e.get("message").and_then(|m| m.as_str()).unwrap_or("error")
        );
    }
    Ok(serde_json::from_value(v.get("result").cloned().unwrap_or_default())?)
}
