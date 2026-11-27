//! 210 §5.7 (D21, D22): the mesh's side of public web sharing. The mesh share key
//! (`mesh/share_key`) behind a signing handle, the lease-holder predicate that alone may mint
//! it, who may receive a delegation, and removal events. Generic: no `share-proto`; the share
//! plugin builds and signs its records through [`ShareKey`]. [`crate::net::Net::dial_alpn`] is
//! the transport half (D18).

#[cfg(target_os = "ios")]
use crate::identity::{PublicKey, SecretKey, Signature};
#[cfg(not(target_os = "ios"))]
use iroh::{PublicKey, SecretKey, Signature};
use secrets::Vault;
use tokio::sync::broadcast;
use zeroize::Zeroizing;

use crate::types::EngineId;

/// The mesh share key, loaded from the vault: an Ed25519 secret whose public half is the mesh
/// id at every share edge. The bytes never leave this handle except inside
/// [`ShareKey::with_secret`]'s borrow; `Debug` shows the mesh id and row version only.
pub struct ShareKey {
    secret: SecretKey,
    version: u32,
}

impl ShareKey {
    fn from_row((version, bytes): (u32, Zeroizing<[u8; 32]>)) -> Self {
        Self {
            secret: SecretKey::from_bytes(&bytes),
            version,
        }
    }

    /// The mesh id: the key's public half.
    pub fn mesh_id(&self) -> PublicKey {
        self.secret.public()
    }

    /// The `mesh/share_key` row version this key was read at (the rekey's compare-and-set
    /// precondition, [`Vault::replace_mesh_share_key`]).
    pub fn version(&self) -> u32 {
        self.version
    }

    /// Ed25519 over `msg` (the caller prepends its record's domain tag).
    pub fn sign(&self, msg: &[u8]) -> Signature {
        self.secret.sign(msg)
    }

    /// Run `f` with the secret borrowed, for signers that take a `&SecretKey`
    /// (`share_proto::Delegation::sign`, `Rekey::sign`). `f` must not keep or copy it.
    pub fn with_secret<R>(&self, f: impl FnOnce(&SecretKey) -> R) -> R {
        f(&self.secret)
    }
}

impl std::fmt::Debug for ShareKey {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("ShareKey")
            .field("mesh_id", &self.mesh_id())
            .field("version", &self.version)
            .finish_non_exhaustive()
    }
}

/// This engine is the synced lease holder: the hub flag, the first sync done, and the mesh
/// actor's lease naming this engine (the predicate of `crates/link/src/rpc_support.rs`, since
/// `is_hub()` alone is true on both sides of a dual-hub window). A lone engine holds its own
/// lease; without a running mesh actor this is `false`.
pub async fn is_lease_holder() -> bool {
    if !(crate::is_hub() && crate::synced()) {
        return false;
    }
    let Some(mesh) = crate::actor() else {
        return false;
    };
    mesh.ask(crate::Status)
        .await
        .is_ok_and(|s| s.lease.holder.as_ref() == Some(&s.self_id))
}

/// The mesh share key when the row exists (any engine of the mesh); `Ok(None)` = not minted.
pub fn share_key(vault: &Vault) -> anyhow::Result<Option<ShareKey>> {
    Ok(vault.mesh_share_key_if_set()?.map(ShareKey::from_row))
}

/// The mesh share key, minted when missing only by the synced lease holder
/// ([`is_lease_holder`]), with `set_if_version(…, None)` and a read-back. Any other engine
/// without the row gets an error and writes nothing.
// TODO(210-M2): the settle rule (`crate::wait_synced`, then `rekey_settle_secs`, then a
// re-read) for the dual-holder window; M1 is one engine as its own holder.
pub async fn ensure_share_key(vault: &Vault) -> anyhow::Result<ShareKey> {
    if let Some(k) = share_key(vault)? {
        return Ok(k);
    }
    let holder = is_lease_holder().await;
    Ok(ShareKey::from_row(vault.mesh_share_key(holder).await?))
}

/// 210 §5.7 (F-M1): the holder signs a delegation only for itself or a live peer (paired:
/// neither tombstoned nor quarantined). `false` before the mesh bound its endpoint, except for
/// this engine's own id once it is known.
pub fn may_delegate(engine: &str) -> bool {
    crate::self_id().as_deref() == Some(engine) || crate::is_paired(engine) == Some(true)
}

/// 210 §5.7 (D22): tombstoned engines as they leave the paired set, the rekey trigger
/// ([`crate::net::Net::subscribe_removals`]); `None` before the mesh bound its endpoint.
// TODO(210-M2): the share plugin's rekey state machine subscribes and reconciles with
// [`crate::RemovedPeers`] on start and on lease gain.
pub fn subscribe_removals() -> Option<broadcast::Receiver<EngineId>> {
    crate::net().map(|n| n.subscribe_removals())
}

#[cfg(all(test, not(target_os = "ios")))]
mod tests {
    use super::*;

    /// Without a running mesh actor no engine is the lease holder, so nothing mints; once the
    /// row exists (minted by a holder) any engine loads it, and the handle signs under the
    /// mesh id.
    #[tokio::test]
    async fn only_the_holder_mints_and_the_handle_signs() {
        let dir = tempfile::tempdir().unwrap();
        let vault = Vault::open(dir.path(), secrets::KeyStore::File)
            .await
            .unwrap();
        assert!(!is_lease_holder().await);
        assert!(ensure_share_key(&vault).await.is_err());
        assert!(share_key(&vault).unwrap().is_none(), "nothing written");

        vault.mesh_share_key(true).await.unwrap();
        let key = ensure_share_key(&vault).await.unwrap();
        assert_eq!(key.version(), 1);
        assert_eq!(share_key(&vault).unwrap().unwrap().mesh_id(), key.mesh_id());
        let sig = key.sign(b"tag\0body");
        assert!(key.mesh_id().verify(b"tag\0body", &sig).is_ok());
    }
}
