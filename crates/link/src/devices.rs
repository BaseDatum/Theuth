//! Paired-device registry (31 §6.3, replicated per 69 decision 3): the
//! `devices` and `pending` tables of `<data_dir>/link.sqlite`, mirrored in
//! memory so conn tasks answer without awaiting. Every mutation goes to the
//! store's ordered writer; rows other engines wrote arrive by replication and
//! land here through [`Devices::reload`] / [`Pending::reload`].
//!
//! `devices.yaml` (pre-69) is imported once — when the table is empty — as
//! active rows, then left on disk and never read again.

use std::path::Path;
use std::sync::Arc;

use serde::{Deserialize, Serialize};

use crate::identity::{b64, now_ms, unb64};
use crate::store::{Op, PendingRow, Store};
use crate::types::{Device, DeviceInfo};

/// One device row. `pub` is base64url in YAML/sqlite, raw bytes in memory.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Row {
    /// First 8 chars of `b64url(pub)`.
    pub id: String,
    /// Device static public key.
    #[serde(rename = "pub", with = "pubkey")]
    pub key: [u8; 32],
    /// Display name.
    pub name: String,
    /// `ios` | `macos` | `web` | …
    pub platform: String,
    /// Unix ms.
    pub paired_at_ms: u64,
    /// Unix ms of the last conn close; `0` never.
    pub last_seen_ms: u64,
    /// Unix ms of revocation; `None` while active. Never deleted (69 decision 3).
    #[serde(default, skip_serializing_if = "Option::is_none")]
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

mod pubkey {
    use serde::{Deserialize, Deserializer, Serializer, de::Error};

    pub fn serialize<S: Serializer>(key: &[u8; 32], s: S) -> Result<S::Ok, S::Error> {
        s.serialize_str(&super::b64(key))
    }

    pub fn deserialize<'de, D: Deserializer<'de>>(d: D) -> Result<[u8; 32], D::Error> {
        let s = String::deserialize(d)?;
        super::unb64(&s)
            .and_then(|v| v.try_into().ok())
            .ok_or_else(|| D::Error::custom("pub: expected 32 base64url bytes"))
    }
}

/// The registry: every row (revoked included) in pairing order. Small
/// (`max_devices` 16), so lookups are linear scans. `store: None` = memory
/// only (the store failed to open, or a test).
pub struct Devices {
    rows: Vec<Row>,
    store: Option<Store>,
}

impl Devices {
    /// Memory only.
    pub fn empty() -> Self {
        Self {
            rows: Vec::new(),
            store: None,
        }
    }

    /// Load the table; when it is empty and `<link_dir>/devices.yaml` exists,
    /// import its rows as active (once — the file is left in place, unread
    /// afterwards).
    pub async fn open(store: Store, link_dir: &Path) -> Self {
        let mut rows = store.devices().await.unwrap_or_else(|e| {
            tracing::warn!("link.sqlite: devices: {e:#}");
            Vec::new()
        });
        if rows.is_empty() {
            let yaml = link_dir.join("devices.yaml");
            if let Ok(text) = std::fs::read_to_string(&yaml) {
                let imported = serde_yaml::from_str::<Option<Vec<Row>>>(&text)
                    .unwrap_or_else(|e| {
                        tracing::warn!("{}: {e}; not imported", yaml.display());
                        None
                    })
                    .unwrap_or_default();
                for r in &imported {
                    if let Err(e) = store.apply(&Op::PutDevice(r.clone())).await {
                        tracing::warn!("link.sqlite: import {}: {e:#}", r.id);
                    }
                }
                if !imported.is_empty() {
                    tracing::info!(
                        "link: imported {} device(s) from devices.yaml",
                        imported.len()
                    );
                }
                rows = imported;
            }
        }
        Self {
            rows,
            store: Some(store),
        }
    }

    /// Replace the mirror with what the table holds now (replicated rows).
    pub fn reload(&mut self, rows: Vec<Row>) {
        self.rows = rows;
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

    /// The key belongs to a revoked row (the row stays in the table; 69 §5: a
    /// revoked device is refused, not treated as a stranger).
    pub fn is_revoked(&self, key: &[u8; 32]) -> bool {
        self.rows.iter().any(|r| &r.key == key && !r.active())
    }

    /// Active row by `Device.id`.
    pub fn by_id(&self, id: &str) -> Option<&Row> {
        self.rows().find(|r| r.id == id)
    }

    /// Upsert (a re-pairing of a revoked key reactivates it) and persist.
    pub fn insert(&mut self, row: Row) {
        self.rows.retain(|r| r.key != row.key);
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

    #[tokio::test]
    async fn yaml_is_imported_once_and_revoke_keeps_the_row() {
        let dir = tempfile::tempdir().unwrap();
        let link_dir = dir.path().join("link");
        std::fs::create_dir_all(&link_dir).unwrap();
        let key = [7u8; 32];
        let legacy = row(key, "iPhone");
        std::fs::write(
            link_dir.join("devices.yaml"),
            serde_yaml::to_string(std::slice::from_ref(&legacy)).unwrap(),
        )
        .unwrap();

        let store = Store::open(&dir.path().join(DB)).await.unwrap();
        let mut d = Devices::open(store.clone(), &link_dir).await;
        assert_eq!(
            d.rows().cloned().collect::<Vec<_>>(),
            std::slice::from_ref(&legacy)
        );
        assert_eq!(
            store.devices().await.unwrap(),
            std::slice::from_ref(&legacy),
            "imported into the table"
        );
        assert!(link_dir.join("devices.yaml").exists(), "left in place");

        assert!(d.rename(&legacy.id, "Phone"));
        assert!(d.touch_last_seen(&key, 5));
        assert_eq!(d.remove(&legacy.id).map(|r| r.name), Some("Phone".into()));
        assert!(d.by_key(&key).is_none(), "revoked = unknown");
        assert!(d.by_id(&legacy.id).is_none());
        assert!(d.remove(&legacy.id).is_none(), "revoking twice is a no-op");
        assert_eq!(d.all().len(), 1, "the row stays");
        let rows = settle(&store, |r| r[0].revoked_ms.is_some()).await;
        assert_eq!((rows[0].name.as_str(), rows[0].last_seen_ms), ("Phone", 5));

        // A second open does not re-import: the table is not empty.
        std::fs::write(
            link_dir.join("devices.yaml"),
            serde_yaml::to_string(&[row([8u8; 32], "x")]).unwrap(),
        )
        .unwrap();
        let d2 = Devices::open(store.clone(), &link_dir).await;
        assert_eq!(d2.all().len(), 1);
        assert!(d2.is_empty(), "no active rows");

        // Re-pairing a revoked key reactivates it.
        let mut d2 = d2;
        d2.insert(row(key, "again"));
        assert_eq!(d2.by_key(&key).map(|r| r.name.as_str()), Some("again"));
        assert_eq!(d2.len(), 1);
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
