//! The engine's implementations of the share seams (210 §5.3, §5.6, §5.7, §6.2): the mesh
//! endpoint dials the edge, the vault holds `mesh/share_key` and the name keys, and the
//! own-listener registry names theuth's ports. Each adapter resolves the mesh endpoint and
//! vault when it is used, so plugin start order does not matter and an engine without the mesh
//! plugin answers "not running" instead of failing to start.

use std::sync::Arc;
use std::time::Duration;

use anyhow::Context as _;
use iroh::endpoint::QuicTransportConfig;
use share_proto::{EndpointAddr, PublicKey, Signed, limits};

use crate::edge::{DelegationSource, EdgeConn, EdgeDialer};
use crate::engine::Hooks;
use crate::keys::{self, NameKey, NameKeyStore};
use crate::sources::{OwnListeners, ProcessTree};

/// Per-stream receive window: 256 visitor streams × 256 KiB = the 64 MiB connection window.
const STREAM_WINDOW: u32 = 256 * 1024;

/// The tunnel's QUIC transport (§5.3): the edge opens visitor streams, so this side allows
/// [`limits::VISITOR_STREAMS_PER_TUNNEL`] concurrent peer-opened bi-streams, with windows sized
/// so all of them can move data at once. The mesh endpoint's own config is untouched.
pub fn tunnel_transport() -> QuicTransportConfig {
    let streams = limits::VISITOR_STREAMS_PER_TUNNEL;
    QuicTransportConfig::builder()
        .max_concurrent_bidi_streams(streams.into())
        .stream_receive_window(STREAM_WINDOW.into())
        .receive_window((STREAM_WINDOW * streams).into())
        .send_window(u64::from(STREAM_WINDOW) * u64::from(streams))
        .build()
}

/// [`EdgeDialer`] over `mesh`'s [`mesh::net::Net::dial_alpn`]: no `join_network`, no
/// discovery publish (D18).
pub struct MeshDialer;

#[async_trait::async_trait]
impl EdgeDialer for MeshDialer {
    async fn dial(&self, addr: EndpointAddr, alpns: &[&[u8]]) -> anyhow::Result<Arc<dyn EdgeConn>> {
        let net = mesh::net().context("the mesh endpoint is not running")?;
        // TODO(210-M4): offer every major (`ConnectOptions::with_additional_alpns` in `mesh`).
        let alpn = alpns.first().context("no share ALPN to offer")?;
        let conn = net.dial_alpn(addr, alpn, tunnel_transport()).await?;
        Ok(Arc::new(conn))
    }
}

/// The vault behind the mesh endpoint (the mesh plugin hands it over once `secrets` answers).
fn vault() -> anyhow::Result<Arc<secrets::Vault>> {
    mesh::net()
        .context("the mesh endpoint is not running")?
        .vault()
        .context("the vault is not open yet")
}

/// [`DelegationSource`] for the family (§5.7, F-M1): the synced lease holder reads (or mints)
/// `mesh/share_key` and signs its own delegation; any other engine asks the holder with
/// `share_delegate` over `theuth/rpc/1`, and the holder signs only for a live peer.
pub struct MeshDelegation;

/// This engine's endpoint id.
fn self_key() -> anyhow::Result<PublicKey> {
    mesh::self_id()
        .context("the mesh identity is not known yet")?
        .parse()
        .context("mesh self id")
}

/// The holder side of `share_delegate`: sign for `engine` when it is this engine or a live peer,
/// `issued_ms = min(own clock, edge_now_ms)` (§5.7). Errors on a non-holder.
pub async fn delegate_as_holder(
    engine: PublicKey,
    edge: PublicKey,
    domain: &str,
    edge_now_ms: u64,
) -> anyhow::Result<Signed> {
    anyhow::ensure!(
        mesh::share::is_lease_holder().await,
        "only the mesh's lease holder signs delegations"
    );
    let key = mesh::share::ensure_share_key(&*vault()?).await?;
    let live = mesh::share::may_delegate(&engine.to_string());
    key.with_secret(|sk| {
        crate::holder::issue(sk, engine, edge, domain, edge_now_ms, platform::time::now_ms(), live)
    })
}

#[async_trait::async_trait]
impl DelegationSource for MeshDelegation {
    async fn delegate(
        &self,
        edge: PublicKey,
        domain: &str,
        edge_now_ms: u64,
    ) -> anyhow::Result<Signed> {
        let engine = self_key()?;
        match crate::family::holder().await? {
            crate::family::Holder::Me => delegate_as_holder(engine, edge, domain, edge_now_ms).await,
            crate::family::Holder::Peer(h) => {
                crate::family::call(
                    &h,
                    "share_delegate",
                    serde_json::json!({
                        "engine": engine.to_string(),
                        "edge": edge.to_string(),
                        "domain": domain,
                        "edge_now_ms": edge_now_ms,
                    }),
                )
                .await
            }
        }
    }
}

/// How long a non-holder waits for the holder's name-key row to replicate (§6.2 step 1).
const REPLICATE_WAIT: Duration = Duration::from_secs(120);

/// [`NameKeyStore`] for the family (§6.2 step 1, F-M4): reads the vault row; a missing row is
/// created only by the lease holder (`share_name_key_ensure`, with the settle rule), and a
/// non-holder waits for it to replicate before posting its CSR.
pub struct FamilyNameKeys(pub VaultNameKeys);

/// The holder side of `share_name_key_ensure`: the settled row (written here if missing).
pub async fn name_key_as_holder(store: &VaultNameKeys, domain: &str, name: &str) -> anyhow::Result<NameKey> {
    anyhow::ensure!(
        mesh::share::is_lease_holder().await,
        "only the mesh's lease holder creates name keys"
    );
    crate::holder::ensure_name_key(store, &crate::holder::MeshSettle, domain, name).await
}

#[async_trait::async_trait]
impl NameKeyStore for FamilyNameKeys {
    async fn get(&self, domain: &str, name: &str) -> anyhow::Result<Option<NameKey>> {
        self.0.get(domain, name).await
    }

    async fn create(&self, domain: &str, name: &str, _key: NameKey) -> anyhow::Result<NameKey> {
        match crate::family::holder().await? {
            crate::family::Holder::Me => name_key_as_holder(&self.0, domain, name).await,
            crate::family::Holder::Peer(h) => {
                let spki: String = crate::family::call(
                    &h,
                    "share_name_key_ensure",
                    serde_json::json!({ "domain": domain, "name": name }),
                )
                .await?;
                let vault = self.0.vault()?;
                let deadline = tokio::time::Instant::now() + REPLICATE_WAIT;
                loop {
                    vault.refresh().await?;
                    if let Some(k) = self.0.get(domain, name).await?
                        && crate::holder::spki_hex(&crate::holder::row_spki(&k, name, domain)?) == spki
                    {
                        return Ok(k);
                    }
                    anyhow::ensure!(
                        tokio::time::Instant::now() < deadline,
                        "the name key for {name}.{domain} has not reached this machine yet"
                    );
                    tokio::time::sleep(Duration::from_secs(1)).await;
                }
            }
        }
    }
}

/// [`NameKeyStore`] over the vault rows `share/<domain>/<name>/tls` (§6.2, §8.3), written with
/// `set_if_version(…, None)` so two engines minting at once settle on one key.
// The holder's raw store; engines use [`FamilyNameKeys`] over it.
/// `None` resolves the vault behind the mesh endpoint on each call.
pub struct VaultNameKeys(pub Option<Arc<secrets::Vault>>);

impl VaultNameKeys {
    fn vault(&self) -> anyhow::Result<Arc<secrets::Vault>> {
        match &self.0 {
            Some(v) => Ok(v.clone()),
            None => vault(),
        }
    }
}

/// Settle on the row after a lost compare-and-set: the replica may need a moment.
const SETTLE: Duration = Duration::from_millis(200);

fn parse_row(row: &str, value: &str) -> anyhow::Result<NameKey> {
    serde_json::from_str(value).with_context(|| format!("vault row {row} is not a name key"))
}

#[async_trait::async_trait]
impl NameKeyStore for VaultNameKeys {
    async fn get(&self, domain: &str, name: &str) -> anyhow::Result<Option<NameKey>> {
        let row = keys::vault_row(domain, name);
        self.vault()?
            .get_versioned(&row)
            .map(|(_, v)| parse_row(&row, &v))
            .transpose()
    }

    async fn create(&self, domain: &str, name: &str, key: NameKey) -> anyhow::Result<NameKey> {
        let row = keys::vault_row(domain, name);
        let vault = self.vault()?;
        let json = serde_json::to_string(&key)?;
        match vault
            .set_if_version(&row, &json, "share", None)
            .await
            .map_err(anyhow::Error::msg)?
        {
            Some(_) => Ok(key),
            None => {
                // Another engine wrote it first: theirs wins.
                vault.refresh().await?;
                for _ in 0..10 {
                    if let Some((_, v)) = vault.get_versioned(&row) {
                        return parse_row(&row, &v);
                    }
                    tokio::time::sleep(SETTLE).await;
                    vault.refresh().await?;
                }
                anyhow::bail!("vault row {row} exists but cannot be read")
            }
        }
    }
}

/// The holder side of a name-key rotation (§6.3): replace `share/<domain>/<name>/tls` with a
/// fresh key of the next generation (compare-and-set on the row version), settle, re-read.
pub async fn rotate_name_key_as_holder(domain: &str, name: &str) -> anyhow::Result<NameKey> {
    anyhow::ensure!(
        mesh::share::is_lease_holder().await,
        "only the mesh's lease holder rotates name keys"
    );
    let vault = vault()?;
    let row = keys::vault_row(domain, name);
    let (v, cur) = vault
        .get_versioned(&row)
        .with_context(|| format!("no name key for {name}.{domain}"))?;
    let cur = parse_row(&row, &cur)?;
    let (key_pem, csr_der) = keys::name_key(name, domain)?;
    let next = NameKey {
        r#gen: cur.r#gen + 1,
        key_pem,
        csr_der,
    };
    let json = serde_json::to_string(&next)?;
    vault
        .set_if_version(&row, &json, "share", Some(v))
        .await
        .map_err(anyhow::Error::msg)?;
    crate::holder::Settle::settle(&crate::holder::MeshSettle).await;
    vault.refresh().await?;
    let (_, settled) = vault.get_versioned(&row).context("name key row vanished")?;
    parse_row(&row, &settled)
}

/// The mesh id (public half of `mesh/share_key`) when the row exists on this machine.
pub fn mesh_id() -> Option<String> {
    let v = vault().ok()?;
    mesh::share::share_key(&v).ok().flatten().map(|k| k.mesh_id().to_string())
}

/// Every seam filled with the engine's adapters.
pub fn engine_hooks() -> Hooks {
    Hooks {
        dialer: Some(Arc::new(MeshDialer)),
        delegation: Some(Arc::new(MeshDelegation)),
        name_keys: Some(Arc::new(FamilyNameKeys(VaultNameKeys(None)))),
        own_ports: Arc::new(OwnListeners),
        process: Arc::new(ProcessTree),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The first writer's key wins: a second `ensure` (another engine minting at once, or a
    /// restart) gets the stored row back, never a second key for the same name.
    #[tokio::test]
    async fn name_key_row_is_written_once_and_read_back() {
        let dir = tempfile::tempdir().unwrap();
        let vault = secrets::Vault::open(dir.path(), secrets::KeyStore::File)
            .await
            .unwrap();
        let store = VaultNameKeys(Some(Arc::new(vault)));
        assert_eq!(store.get("tun.zip", "sky").await.unwrap(), None);
        let first = keys::ensure(&store, "tun.zip", "sky").await.unwrap();
        assert_eq!(first.r#gen, 1);
        let (pem, csr) = keys::name_key("sky", "tun.zip").unwrap();
        let lost = store
            .create(
                "tun.zip",
                "sky",
                NameKey {
                    r#gen: 1,
                    key_pem: pem,
                    csr_der: csr,
                },
            )
            .await
            .unwrap();
        assert_eq!(lost, first);
        assert_eq!(store.get("tun.zip", "sky").await.unwrap(), Some(first));
    }
}
