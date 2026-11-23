//! 210 §5.7, §6.2 (F-M1, F-M4): what only the lease holder does for the family. It signs
//! delegations for itself and live peers with the clock rule, creates name keys with the settle
//! rule, and heals a `csr_mismatch` left by two holders in one sync gap. Other engines reach it with
//! `share_delegate` / `share_name_key_ensure` over `theuth/rpc/1` (`crate::rpc_share`).

use std::time::Duration;

use share_proto::{Delegation, PublicKey, SecretKey, Signed, SpkiHash, limits};

use crate::keys::{self, NameKey, NameKeyStore};

/// `csr_heal_secs` (P0-6 decides it): how long the edge's SPKI may differ from the settled vault
/// row before the holder rotates the name to the row's CSR.
pub const CSR_HEAL_MS: u64 = 5 * 60_000;

/// Renewal jitter: a renewal lands within this much before the 12 h mark.
pub const RENEW_JITTER_MS: u64 = 30 * 60_000;

/// The holder's answer to `share_delegate {engine, edge, domain, edge_now_ms}`: a delegation for
/// `engine` signed with `mesh_key`, `issued_ms = min(own_now_ms, edge_now_ms)`, only for a live
/// engine (itself or a paired peer, never a tombstoned or quarantined one).
pub fn issue(
    mesh_key: &SecretKey,
    engine: PublicKey,
    edge: PublicKey,
    domain: &str,
    edge_now_ms: u64,
    own_now_ms: u64,
    live: bool,
) -> anyhow::Result<Signed> {
    anyhow::ensure!(live, "engine {engine} is not a live member of this mesh");
    let d = Delegation::new(
        domain.to_owned(),
        edge,
        mesh_key.public(),
        engine,
        own_now_ms,
        edge_now_ms,
    );
    Ok(d.sign(mesh_key)?)
}

/// When to renew a delegation issued at `issued_ms`: 12 h in, minus up to
/// [`RENEW_JITTER_MS`] chosen by `seed` (so a family does not renew in one burst).
pub fn renew_at(issued_ms: u64, seed: u64) -> u64 {
    issued_ms + limits::DELEGATION_RENEW_MS - seed % RENEW_JITTER_MS
}

/// The settle rule's wait (§5.7): `mesh::wait_synced()` and then `rekey_settle_secs`.
#[async_trait::async_trait]
pub trait Settle: Send + Sync {
    /// Return once a write made now has had its chance to meet a concurrent one.
    async fn settle(&self);
}

/// The engine's settle: `wait_synced`, then [`limits::REKEY_SETTLE_SECS`].
pub struct MeshSettle;

#[async_trait::async_trait]
impl Settle for MeshSettle {
    async fn settle(&self) {
        mesh::wait_synced().await;
        tokio::time::sleep(Duration::from_secs(limits::REKEY_SETTLE_SECS)).await;
    }
}

/// `share.name_key.ensure` on the holder (§6.2 step 1): an existing row is returned; otherwise a
/// fresh P-256 key and CSR are written with `set_if_version(…, None)`, the write settles, and the
/// row is read again, so the answer is the settled row whoever wrote it.
pub async fn ensure_name_key(
    store: &dyn NameKeyStore,
    settle: &dyn Settle,
    domain: &str,
    name: &str,
) -> anyhow::Result<NameKey> {
    if let Some(k) = store.get(domain, name).await? {
        return Ok(k);
    }
    let (key_pem, csr_der) = keys::name_key(name, domain)?;
    store
        .create(
            domain,
            name,
            NameKey {
                r#gen: 1,
                key_pem,
                csr_der,
            },
        )
        .await?;
    settle.settle().await;
    store
        .get(domain, name)
        .await?
        .ok_or_else(|| anyhow::anyhow!("name key {domain}/{name} vanished after the write"))
}

/// SPKI hash of a name key row's CSR.
pub fn row_spki(row: &NameKey, name: &str, domain: &str) -> anyhow::Result<SpkiHash> {
    Ok(share_proto::check_csr(&row.csr_der, name, domain)?.spki)
}

/// Lowercase hex of an SPKI hash (the `share_name_key_ensure` answer).
pub fn spki_hex(h: &SpkiHash) -> String {
    data_encoding::HEXLOWER.encode(&h.0)
}

/// What the holder does about a name whose edge SPKI (welcome `names[].spki`, or a
/// `csr_mismatch {spki}`) is not the vault row's.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum CsrHeal {
    /// Edge and vault agree (or the edge has no CSR yet).
    InSync,
    /// They differ since `since`; wait for the vault to settle.
    Wait {
        /// First seen.
        since: u64,
    },
    /// They differed for [`CSR_HEAL_MS`]: post the row's CSR with `?rotate=true`.
    Rotate,
}

/// The `csr_mismatch` heal rule (§6.2, P0-6): after `heal_ms` of disagreement the holder rotates
/// the name to the settled vault row's CSR; the losing key of a dual-holder window never serves.
pub fn csr_heal(
    edge_spki: Option<&SpkiHash>,
    row_spki: &SpkiHash,
    since: Option<u64>,
    now: u64,
    heal_ms: u64,
) -> CsrHeal {
    match edge_spki {
        None => CsrHeal::InSync,
        Some(e) if e == row_spki => CsrHeal::InSync,
        Some(_) => match since {
            Some(s) if now.saturating_sub(s) >= heal_ms => CsrHeal::Rotate,
            Some(s) => CsrHeal::Wait { since: s },
            None => CsrHeal::Wait { since: now },
        },
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn delegation_uses_the_earlier_clock_and_refuses_the_dead() {
        let k = crate::rekey::fresh_secret();
        let engine = crate::rekey::fresh_secret().public();
        let edge = crate::rekey::fresh_secret().public();
        // Holder ahead of the edge: issued at the edge's time, never not-yet-valid.
        let s = issue(&k, engine, edge, "tun.zip", 1_000_000, 1_000_000 + 600_000, true).unwrap();
        let d = Delegation::open(&s).unwrap();
        assert_eq!(d.issued_ms, 1_000_000);
        assert_eq!(d.expires_ms, 1_000_000 + limits::DELEGATION_TTL_MS);
        // Holder behind: its own clock.
        let s = issue(&k, engine, edge, "tun.zip", 2_000_000, 1_500_000, true).unwrap();
        assert_eq!(Delegation::open(&s).unwrap().issued_ms, 1_500_000);
        assert!(issue(&k, engine, edge, "tun.zip", 1, 1, false).is_err());
        let at = renew_at(0, 7);
        assert!(at <= limits::DELEGATION_RENEW_MS && at > limits::DELEGATION_RENEW_MS - RENEW_JITTER_MS);
    }

    #[test]
    fn csr_heal_waits_then_rotates() {
        let a = SpkiHash([1; 32]);
        let b = SpkiHash([2; 32]);
        assert_eq!(csr_heal(Some(&a), &a, None, 5, 10), CsrHeal::InSync);
        assert_eq!(csr_heal(None, &a, None, 5, 10), CsrHeal::InSync);
        assert_eq!(csr_heal(Some(&b), &a, None, 5, 10), CsrHeal::Wait { since: 5 });
        assert_eq!(csr_heal(Some(&b), &a, Some(5), 14, 10), CsrHeal::Wait { since: 5 });
        assert_eq!(csr_heal(Some(&b), &a, Some(5), 15, 10), CsrHeal::Rotate);
    }
}
