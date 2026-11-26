//! Paired-device registry (31 §6.3, replicated per 69 decision 3): the
//! `devices` and `pending` tables of `<data_dir>/link.sqlite`, mirrored in
//! memory so conn tasks answer without awaiting. Every mutation goes to the
//! store's ordered writer; rows other engines wrote (or deleted) arrive by
//! replication and land here through [`Devices::reload`] / [`Pending::reload`].
//!
//! Lifetime of a device row: paired → revoked (`revoked_ms`, refused by every
//! hub) → deleted (a replicated row delete, always preceded by a revoke so a
//! delete that loses to a concurrent column write leaves the row revoked).
//! Revoked rows are deleted [`REVOKED_TTL_MS`] after their revocation, expired
//! `pending` rows on every refresh ([`crate::store::Op::Purge`]).

use std::collections::HashSet;
use std::path::Path;
use std::sync::Arc;

use crate::identity::{b64, now_ms};
use crate::store::{Op, PendingRow, Store};
use crate::types::{Device, DeviceInfo};

/// A revoked row is deleted this long after `revoked_ms` (30 days).
pub const REVOKED_TTL_MS: u64 = 30 * 24 * 60 * 60 * 1000;

/// Pre-69 per-engine device list. Never read; deleted once at start ([`remove_legacy`]).
const LEGACY_DEVICES: &str = "devices.yaml";

/// Delete the pre-69 device list under `link_dir` (`<data_dir>/link`), logging once when it was
/// there. `link/identity` stays: it is the engine identity the engine uplink and the `nodes`
/// plugin (this harness as a device of other nodes) still use. `true` when a file was removed.
pub fn remove_legacy(link_dir: &Path) -> bool {
    let path = link_dir.join(LEGACY_DEVICES);
    match std::fs::remove_file(&path) {
        Ok(()) => {
            tracing::info!("link: removed legacy device list {}", path.display());
            true
        }
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => false,
        Err(e) => {
            tracing::warn!("link: remove {}: {e}", path.display());
            false
        }
    }
}

/// One device row (`static_pub` is base64url in sqlite, raw bytes here).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Row {
    /// First 8 chars of `b64url(pub)`.
    pub id: String,
    /// Device static public key.
    pub key: [u8; 32],
    /// Display name.
    pub name: String,
    /// `ios` | `macos` | `web` | …
    pub platform: String,
    /// Unix ms.
    pub paired_at_ms: u64,
    /// Unix ms of the last conn close; `0` never.
    pub last_seen_ms: u64,
    /// Unix ms of revocation; `None` while active.
    pub revoked_ms: Option<u64>,
}

impl Row {
    /// UI shape with `online` supplied by the caller.
    pub fn device(&self, online: bool) -> Device {
        Device {
            id: self.id.clone(),
            name: self.name.clone(),
            platform: self.platform.clone(),
            paired_at_ms: self.paired_at_ms,
            last_seen_ms: self.last_seen_ms,
            online,
        }
    }

    /// Not revoked.
    pub fn active(&self) -> bool {
        self.revoked_ms.is_none()
    }

    /// `devices_list` shape.
    pub fn info(&self) -> DeviceInfo {
        DeviceInfo {
            id: self.id.clone(),
            name: self.name.clone(),
            platform: self.platform.clone(),
            paired_ms: self.paired_at_ms,
            last_seen_ms: self.last_seen_ms,
            revoked: self.revoked_ms.is_some(),
        }
    }
}

/// `Device.id` for a static public key.
pub fn device_id(key: &[u8; 32]) -> String {
    let mut s = b64(key);
    s.truncate(8);
    s
}

/// The registry: every row (revoked included) in pairing order. Small
/// (`max_devices` 16), so lookups are linear scans. `store: None` = memory
/// only (the store failed to open, or a test).
pub struct Devices {
    rows: Vec<Row>,
    store: Option<Store>,
    /// Inserted here, not yet seen in the table: a reload keeps them.
    unflushed: HashSet<String>,
    /// Deleted here, still in the table: a reload drops them.
    deleted: HashSet<String>,
}

impl Devices {
    /// Memory only.
    pub fn empty() -> Self {
        Self {
            rows: Vec::new(),
            store: None,
            unflushed: HashSet::new(),
            deleted: HashSet::new(),
        }
    }

    /// Load the table.
    pub async fn open(store: Store) -> Self {
        let rows = store.devices().await.unwrap_or_else(|e| {
            tracing::warn!("link.sqlite: devices: {e:#}");
            Vec::new()
        });
        Self {
            rows,
            store: Some(store),
            unflushed: HashSet::new(),
            deleted: HashSet::new(),
        }
    }

    /// The table's rows differ from the mirror in a way a reload must pick up: a row (or a
    /// version of one) the mirror lacks, or a mirror row the table no longer has (deleted on
    /// another engine, or purged). A table that merely lags a local insert or delete is not.
    pub fn stale(&self, table: &[Row]) -> bool {
        table
            .iter()
            .any(|r| !self.deleted.contains(&r.id) && !self.rows.contains(r))
            || self
                .rows
                .iter()
                .any(|m| !self.unflushed.contains(&m.id) && !table.iter().any(|r| r.id == m.id))
    }

    /// Replace the mirror with what the table holds now (replicated rows), keeping local inserts
    /// and deletes the table has not caught up with. Returns the keys of rows that disappeared
    /// (deleted elsewhere or purged): their conns must close.
    pub fn reload(&mut self, table: Vec<Row>) -> Vec<[u8; 32]> {
        self.unflushed
            .retain(|id| !table.iter().any(|r| &r.id == id));
        self.deleted.retain(|id| table.iter().any(|r| &r.id == id));
        let gone = self
            .rows
            .iter()
            .filter(|m| !self.unflushed.contains(&m.id) && !table.iter().any(|r| r.id == m.id))
            .map(|m| m.key)
            .collect();
        let kept: Vec<Row> = self
            .rows
            .drain(..)
            .filter(|m| self.unflushed.contains(&m.id))
            .collect();
        self.rows = table
            .into_iter()
            .filter(|r| !self.deleted.contains(&r.id))
            .collect();
        self.rows.extend(kept);
        gone
    }

    /// Active rows, in pairing order.
    pub fn rows(&self) -> impl Iterator<Item = &Row> {
        self.rows.iter().filter(|r| r.active())
    }

    /// Every row, revoked included.
    pub fn all(&self) -> &[Row] {
        &self.rows
    }

    /// Active row count.
    pub fn len(&self) -> usize {
        self.rows().count()
    }

    /// No active rows.
    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }

    /// Active row by static key; a revoked device is unknown.
    pub fn by_key(&self, key: &[u8; 32]) -> Option<&Row> {
        self.rows().find(|r| &r.key == key)
    }

    /// The key belongs to a revoked row (69 §5: a revoked device is refused, not treated as a
    /// stranger).
    pub fn is_revoked(&self, key: &[u8; 32]) -> bool {
        self.rows.iter().any(|r| &r.key == key && !r.active())
    }

    /// Active row by `Device.id`.
    pub fn by_id(&self, id: &str) -> Option<&Row> {
        self.rows().find(|r| r.id == id)
    }

    /// Any row, revoked included, by `Device.id`.
    pub fn any_by_id(&self, id: &str) -> Option<&Row> {
        self.rows.iter().find(|r| r.id == id)
    }

    /// Upsert (a re-pairing of a revoked key reactivates it) and persist.
    pub fn insert(&mut self, row: Row) {
        self.rows.retain(|r| r.key != row.key);
        self.deleted.remove(&row.id);
        self.unflushed.insert(row.id.clone());
        self.write(Op::PutDevice(row.clone()));
        self.rows.push(row);
    }

    /// Revoke by `Device.id` (`revoked_ms = now`); the row stays. Returns the
    /// row when it was active.
    pub fn remove(&mut self, id: &str) -> Option<Row> {
        let row = self.rows.iter_mut().find(|r| r.id == id && r.active())?;
        let at_ms = now_ms();
        row.revoked_ms = Some(at_ms);
        let row = row.clone();
        self.write(Op::Revoke {
            id: id.to_owned(),
            at_ms,
        });
        Some(row)
    }

    /// Delete the row `id` everywhere (a replicated row delete), revoking it first when active so
    /// a delete that loses to a concurrent write on another engine leaves it revoked, never
    /// active. Returns the row as it was.
    pub fn delete(&mut self, id: &str) -> Option<Row> {
        let i = self.rows.iter().position(|r| r.id == id)?;
        let row = self.rows.remove(i);
        if row.active() {
            self.write(Op::Revoke {
                id: id.to_owned(),
                at_ms: now_ms(),
            });
        }
        self.unflushed.remove(id);
        self.deleted.insert(id.to_owned());
        self.write(Op::DeleteDevice(id.to_owned()));
        Some(row)
    }

    /// Rename an active row; `false` when unknown.
    pub fn rename(&mut self, id: &str, name: &str) -> bool {
        let Some(row) = self.rows.iter_mut().find(|r| r.id == id && r.active()) else {
            return false;
        };
        row.name = name.to_owned();
        self.write(Op::Rename {
            id: id.to_owned(),
            name: name.to_owned(),
        });
        true
    }

    /// `last_seen_ms = at_ms` for the active row with `key`; `false` when unknown.
    pub fn touch_last_seen(&mut self, key: &[u8; 32], at_ms: u64) -> bool {
        let Some(row) = self.rows.iter_mut().find(|r| &r.key == key && r.active()) else {
            return false;
        };
        row.last_seen_ms = at_ms;
        let id = row.id.clone();
        self.write(Op::TouchLastSeen { id, at_ms });
        true
    }

    fn write(&self, op: Op) {
        if let Some(s) = &self.store {
            s.write(op);
        }
    }
}

/// Pending-pairing TTL (31 §4.4, 69 decision 3).
pub const PENDING_TTL_MS: u64 = 300_000;

/// One live pending row, PSK open.
#[derive(Clone, Debug)]
struct Live {
    id: String,
    psk: [u8; 32],
    expires_ms: u64,
}

/// The replicated `pending` rows (69 decision 4): any engine creates one
/// when it shows a QR; the hub takes it when the device's `PairHello` names
/// it (or when it is the only live one). PSKs are sealed under the mesh key
/// in the table and open here; without the vault nothing can be created or
/// opened.
pub struct Pending {
    rows: Vec<Live>,
    store: Option<Store>,
    vault: Option<Arc<secrets::Vault>>,
}

impl Pending {
    /// Wrap; `reload` with the table's rows to pick up what other engines wrote.
    pub fn new(store: Option<Store>, vault: Option<Arc<secrets::Vault>>) -> Self {
        Self {
            rows: Vec::new(),
            store,
            vault,
        }
    }

    /// A fresh row: `(id, psk, expires_ms)`, persisted sealed. `Err` when the
    /// vault is unavailable (nothing to seal with).
    pub fn create(&mut self, name: &str) -> Result<(String, [u8; 32], u64), String> {
        self.sweep();
        let vault = self
            .vault
            .as_ref()
            .ok_or("secrets vault unavailable; cannot seal the pairing secret")?;
        let id = b64(&rand::random::<[u8; 16]>());
        let psk: [u8; 32] = rand::random();
        let now = now_ms();
        let expires_ms = now + PENDING_TTL_MS;
        let row = PendingRow {
            id: id.clone(),
            psk_sealed: vault.seal_with_mesh_key(id.as_bytes(), &psk),
            name: name.to_owned(),
            created_ms: now,
            expires_ms,
        };
        if let Some(s) = &self.store {
            s.write(Op::PutPending(row));
        }
        self.rows.push(Live {
            id: id.clone(),
            psk,
            expires_ms,
        });
        Ok((id, psk, expires_ms))
    }

    /// Consume the live row `id`: its PSK, and the row is gone.
    pub fn take(&mut self, id: &str) -> Option<[u8; 32]> {
        self.sweep();
        let i = self.rows.iter().position(|r| r.id == id)?;
        let row = self.rows.remove(i);
        self.delete(&row.id);
        Some(row.psk)
    }

    /// The one live row when there is exactly one (a hint-less `PairHello`).
    pub fn single_live(&mut self) -> Option<(String, [u8; 32])> {
        self.sweep();
        match self.rows.as_slice() {
            [one] => Some((one.id.clone(), one.psk)),
            _ => None,
        }
    }

    /// Drop expired rows (and their table rows).
    pub fn sweep(&mut self) {
        let now = now_ms();
        let expired: Vec<String> = self
            .rows
            .iter()
            .filter(|r| r.expires_ms <= now)
            .map(|r| r.id.clone())
            .collect();
        for id in &expired {
            self.delete(id);
        }
        self.rows.retain(|r| r.expires_ms > now);
    }

    /// Live row count.
    pub fn len(&self) -> usize {
        let now = now_ms();
        self.rows.iter().filter(|r| r.expires_ms > now).count()
    }

    /// No live rows.
    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }

    /// Expiry of a live row.
    pub fn expires_ms(&self, id: &str) -> Option<u64> {
        self.rows.iter().find(|r| r.id == id).map(|r| r.expires_ms)
    }

    /// Replace the mirror with the table's rows, opening each PSK; rows that
    /// do not open (another mesh key) are skipped.
    pub fn reload(&mut self, rows: Vec<PendingRow>) {
        let Some(vault) = &self.vault else {
            return;
        };
        self.rows = rows
            .into_iter()
            .filter_map(|r| {
                let psk: [u8; 32] = vault
                    .open_with_mesh_key(r.id.as_bytes(), &r.psk_sealed)?
                    .as_slice()
                    .try_into()
                    .ok()?;
                Some(Live {
                    id: r.id,
                    psk,
                    expires_ms: r.expires_ms,
                })
            })
            .collect();
    }

    fn delete(&self, id: &str) {
        if let Some(s) = &self.store {
            s.write(Op::DeletePending(id.to_owned()));
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::store::DB;

    fn row(key: [u8; 32], name: &str) -> Row {
        Row {
            id: device_id(&key),
            key,
            name: name.into(),
            platform: "ios".into(),
            paired_at_ms: 1,
            last_seen_ms: 0,
            revoked_ms: None,
        }
    }

    /// Wait for the writer task to drain (tests only: real callers never await it).
    async fn settle(store: &Store, want: impl Fn(&[Row]) -> bool) -> Vec<Row> {
        for _ in 0..50 {
            let rows = store.devices().await.unwrap();
            if want(&rows) {
                return rows;
            }
            tokio::time::sleep(std::time::Duration::from_millis(20)).await;
        }
        panic!("writer did not settle");
    }

    #[test]
    fn the_legacy_device_list_is_deleted_once_and_the_identity_stays() {
        let dir = tempfile::tempdir().unwrap();
        let link_dir = dir.path().join("link");
        std::fs::create_dir_all(&link_dir).unwrap();
        std::fs::write(link_dir.join("devices.yaml"), "- id: x\n").unwrap();
        std::fs::write(link_dir.join("identity"), [1u8; 32]).unwrap();

        assert!(remove_legacy(&link_dir));
        assert!(!link_dir.join("devices.yaml").exists());
        assert!(
            link_dir.join("identity").exists(),
            "the engine identity stays"
        );
        assert!(!remove_legacy(&link_dir), "nothing left: a no-op");
        assert!(
            !remove_legacy(&dir.path().join("absent")),
            "no link dir: a no-op"
        );
    }

    #[tokio::test]
    async fn revoke_keeps_the_row_and_delete_revokes_then_removes_it() {
        let dir = tempfile::tempdir().unwrap();
        let store = Store::open(&dir.path().join(DB)).await.unwrap();
        let mut d = Devices::open(store.clone()).await;
        let key = [7u8; 32];
        let phone = row(key, "iPhone");
        d.insert(phone.clone());
        settle(&store, |r| r.len() == 1).await;

        assert!(d.rename(&phone.id, "Phone"));
        assert!(d.touch_last_seen(&key, 5));
        assert_eq!(d.remove(&phone.id).map(|r| r.name), Some("Phone".into()));
        assert!(d.by_key(&key).is_none(), "revoked = unknown");
        assert!(d.is_revoked(&key));
        assert!(d.by_id(&phone.id).is_none());
        assert!(d.remove(&phone.id).is_none(), "revoking twice is a no-op");
        assert_eq!(d.all().len(), 1, "the row stays");
        let rows = settle(&store, |r| r[0].revoked_ms.is_some()).await;
        assert_eq!((rows[0].name.as_str(), rows[0].last_seen_ms), ("Phone", 5));

        // Re-pairing a revoked key reactivates it.
        d.insert(row(key, "again"));
        assert_eq!(d.by_key(&key).map(|r| r.name.as_str()), Some("again"));
        settle(&store, |r| r[0].revoked_ms.is_none()).await;

        // Delete an ACTIVE row: gone from the mirror at once, and a reload of a table that still
        // lags the delete does not bring it back.
        let table = store.devices().await.unwrap();
        assert_eq!(d.delete(&phone.id).map(|r| r.name), Some("again".into()));
        assert!(d.all().is_empty());
        assert!(
            !d.is_revoked(&key) && d.by_key(&key).is_none(),
            "unknown now"
        );
        assert!(!d.stale(&table), "the table only lags the local delete");
        assert!(d.reload(table).is_empty());
        assert!(d.all().is_empty());
        settle(&store, <[Row]>::is_empty).await;
        assert!(d.delete(&phone.id).is_none(), "deleting twice is a no-op");
    }

    /// The delete replicates as a row delete; it is preceded by a revoke, so when it loses to a
    /// newer write on another engine (the hub touching `last_seen_ms` as the conn closes) the row
    /// survives there REVOKED — the device still cannot reconnect.
    #[tokio::test]
    async fn a_delete_replicates_and_a_lost_delete_leaves_the_row_revoked() {
        struct Side {
            _dir: tempfile::TempDir,
            store: Store,
            engine: Arc<replica::Engine>,
            handle: replica::ReplicaHandle,
        }
        async fn side() -> Side {
            let dir = tempfile::tempdir().unwrap();
            let store = Store::open(&dir.path().join(DB)).await.unwrap();
            let engine = replica::Engine::open(&dir.path().join("replica.sqlite"))
                .await
                .unwrap();
            let handle = engine
                .attach(store.pool(), crate::store::TABLES)
                .await
                .unwrap();
            Side {
                _dir: dir,
                store,
                engine,
                handle,
            }
        }
        async fn captured(from: &Side) -> Vec<replica::ChangeBatch> {
            let mut out = Vec::new();
            while let Some(batch) = from.handle.flush().await.unwrap() {
                out.push(batch);
            }
            out
        }
        let (a, b) = (side().await, side().await);
        let mut da = Devices::open(a.store.clone()).await;
        for key in [[7u8; 32], [8u8; 32]] {
            da.insert(row(key, "iPad"));
        }
        settle(&a.store, |r| r.len() == 2).await;
        for batch in captured(&a).await {
            b.engine.apply(&batch).await.unwrap();
        }
        assert_eq!(b.store.devices().await.unwrap().len(), 2);

        // 7: deleted on A, nothing else written → gone on B too.
        // 8: deleted on A, then B (the hub) writes `last_seen_ms` later, before it hears of it.
        let (gone, kept) = (device_id(&[7u8; 32]), device_id(&[8u8; 32]));
        da.delete(&gone);
        da.delete(&kept);
        settle(&a.store, <[Row]>::is_empty).await;
        let deletes = captured(&a).await;
        tokio::time::sleep(std::time::Duration::from_millis(5)).await;
        b.store
            .apply(&Op::TouchLastSeen {
                id: kept.clone(),
                at_ms: 9,
            })
            .await
            .unwrap();
        captured(&b).await;
        for batch in &deletes {
            b.engine.apply(batch).await.unwrap();
        }

        let on_b = b.store.devices().await.unwrap();
        assert_eq!(on_b.len(), 1, "{on_b:?}");
        assert_eq!(on_b[0].id, kept);
        assert!(
            on_b[0].revoked_ms.is_some(),
            "survived the lost delete revoked"
        );
        let mut db = Devices::empty();
        assert!(db.stale(&on_b));
        db.reload(on_b);
        assert!(db.is_revoked(&[8u8; 32]) && db.by_key(&[8u8; 32]).is_none());
    }

    #[tokio::test]
    async fn purge_drops_expired_pending_and_long_revoked_devices() {
        let dir = tempfile::tempdir().unwrap();
        let store = Store::open(&dir.path().join(DB)).await.unwrap();
        let now = 100 * REVOKED_TTL_MS;
        let rows = [
            ([1u8; 32], None),
            ([2u8; 32], Some(now - REVOKED_TTL_MS - 1)),
            ([3u8; 32], Some(now - REVOKED_TTL_MS + 1)),
        ];
        for (key, revoked_ms) in rows {
            let r = Row {
                revoked_ms,
                ..row(key, "x")
            };
            store.apply(&Op::PutDevice(r)).await.unwrap();
        }
        for (id, expires_ms) in [("old", now - 1), ("edge", now), ("live", now + 1)] {
            store
                .apply(&Op::PutPending(PendingRow {
                    id: id.into(),
                    psk_sealed: vec![1],
                    name: String::new(),
                    created_ms: 0,
                    expires_ms,
                }))
                .await
                .unwrap();
        }

        store.apply(&Op::Purge { now_ms: now }).await.unwrap();

        let ids: Vec<String> = store
            .devices()
            .await
            .unwrap()
            .into_iter()
            .map(|r| r.id)
            .collect();
        assert_eq!(ids, [device_id(&[1u8; 32]), device_id(&[3u8; 32])]);
        let pending: Vec<String> = store
            .pending()
            .await
            .unwrap()
            .into_iter()
            .map(|r| r.id)
            .collect();
        assert_eq!(pending, ["live"]);
    }

    #[tokio::test]
    async fn pending_rows_seal_take_and_expire() {
        let dir = tempfile::tempdir().unwrap();
        let vault = Arc::new(
            secrets::Vault::open(dir.path(), secrets::KeyStore::File)
                .await
                .unwrap(),
        );
        let store = Store::open(&dir.path().join(DB)).await.unwrap();
        let mut p = Pending::new(Some(store.clone()), Some(vault.clone()));
        assert!(p.single_live().is_none());

        let (id, psk, exp) = p.create("Mac").unwrap();
        assert!(exp > now_ms() && exp <= now_ms() + PENDING_TTL_MS);
        assert_eq!(p.single_live(), Some((id.clone(), psk)));
        let (id2, psk2, _) = p.create("Mac").unwrap();
        assert!(p.single_live().is_none(), "two live rows: no fallback");
        assert_eq!(p.len(), 2);

        // Another engine (a second mirror over the same table) sees both, sealed under the mesh key.
        let rows = loop {
            let rows = store.pending().await.unwrap();
            if rows.len() == 2 {
                break rows;
            }
            tokio::time::sleep(std::time::Duration::from_millis(20)).await;
        };
        assert!(
            rows.iter()
                .all(|r| r.psk_sealed != psk && r.psk_sealed != psk2),
            "never plaintext"
        );
        let mut other = Pending::new(Some(store.clone()), Some(vault.clone()));
        other.reload(rows.clone());
        assert_eq!(other.take(&id2), Some(psk2));
        assert_eq!(other.take(&id2), None, "consumed");
        assert_eq!(other.single_live(), Some((id.clone(), psk)));

        // A different mesh key opens nothing.
        let far = tempfile::tempdir().unwrap();
        let other_vault = Arc::new(
            secrets::Vault::open(far.path(), secrets::KeyStore::File)
                .await
                .unwrap(),
        );
        let mut stranger = Pending::new(None, Some(other_vault));
        stranger.reload(rows);
        assert!(stranger.is_empty());

        // Expiry: a row whose time is up opens fine but is swept, not served.
        let old = PendingRow {
            id: "old".into(),
            psk_sealed: vault.seal_with_mesh_key(b"old", &[3u8; 32]),
            name: String::new(),
            created_ms: 0,
            expires_ms: 1,
        };
        let mut stale = Pending::new(Some(store.clone()), Some(vault));
        stale.reload(vec![old]);
        assert_eq!(stale.rows.len(), 1, "opened");
        assert!(stale.single_live().is_none(), "swept");
        assert!(stale.take("old").is_none());
        assert_eq!(p.take(&id), Some(psk));
    }
}
