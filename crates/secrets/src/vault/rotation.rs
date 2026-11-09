//! 183 §6.9 (D7): the keyring behind [`Vault`]. The current key seals every new row; the
//! retained keys open rows not yet swept. Rotation mints a new current key; the sweep
//! re-seals rows 64 per transaction; a key that is no longer current is retired once no row
//! names it, the grace has passed and no [`RetireHold`] is open.
//!
//! Local `meta` (§9.5): `mesh_key` (the current key, as before 183), `mesh_key_info`,
//! `mesh_keys` (the retained keys, each sealed under the engine key with aad
//! `theuth-mesh-key-1 ‖ id`) and `rotation`.

use std::collections::BTreeSet;
use std::sync::RwLockReadGuard;

use data_encoding::BASE32_NOPAD;
use sqlx::Row as _;

use super::*;

/// Rows re-sealed per transaction (§6.9.6).
pub const SWEEP_BATCH: i64 = 64;
/// A holder that did not mint the current key sweeps only this long after it became
/// current here (W-32: the initiator crashed or went offline).
pub const SWEEP_GRACE_MS: u64 = 60_000;
/// A key stays this long after it stopped being current (W-33; longer than the 5 min link PSK).
pub const RETIRE_GRACE_MS: u64 = 10 * 60_000;
/// 183 R1: how long `rotate` holds the first sweep unless a peer acks the key sooner
/// ([`Vault::release_sweep`]), so re-sealed rows never exist under a key one machine holds.
pub const SWEEP_HOLD_MS: u64 = 10_000;
/// 183 R2: a removed machine's name when none is known (never its engine id).
pub const REMOVED_MACHINE: &str = "a removed machine";
/// 183 R1: `retire` re-runs the trial decrypt of `''` rows at most this often.
pub const LEGACY_CHECK_MS: u64 = 60_000;
/// 183 R1: `adopt_key` refuses a key this many epochs above the current one.
pub const MAX_EPOCH_JUMP: u32 = 1000;

const MESH_KEY_INFO: &str = "mesh_key_info";
const MESH_KEYS: &str = "mesh_keys";
const ROTATION: &str = "rotation";

/// W-29: `hex(sha256("theuth-mesh-key-id-1" ‖ key)[0..8])`, the row's `vault.key_id`.
pub fn key_id(key: &[u8; 32]) -> String {
    use sha2::Digest as _;
    let d = sha2::Sha256::new()
        .chain_update(b"theuth-mesh-key-id-1")
        .chain_update(key)
        .finalize();
    d[..8].iter().map(|b| format!("{b:02x}")).collect()
}

/// Keys order by `(epoch, initiator)`; the id breaks the tie between two fresh vaults.
fn order(i: &KeyInfo) -> (u32, &str, &str) {
    (i.epoch, &i.initiator, &i.id)
}

/// One key held here.
#[derive(Clone)]
pub(crate) struct Held {
    pub(crate) key: Zeroizing<[u8; 32]>,
    pub(crate) info: KeyInfo,
    /// When it stopped being current here (retained keys only).
    since_ms: u64,
}

impl Held {
    fn wire(&self) -> WireKey {
        WireKey {
            info: self.info.clone(),
            key: Zeroizing::new(BASE32_NOPAD.encode(&*self.key)),
        }
    }
}

/// The current key and the retained ones.
#[derive(Clone)]
pub(crate) struct Ring {
    pub(crate) current: Held,
    retained: Vec<Held>,
}

impl Ring {
    /// The held key with this id.
    pub(crate) fn get(&self, id: &str) -> Option<&Held> {
        std::iter::once(&self.current)
            .chain(&self.retained)
            .find(|h| h.info.id == id)
    }

    /// Open with the key `key_id` names; `''` tries the legacy key first, then every other
    /// held key (§6.9.4: AEAD rejects a wrong key).
    pub(crate) fn open(
        &self,
        key_id: &str,
        aad: &[u8],
        bytes: &[u8],
    ) -> Option<Zeroizing<Vec<u8>>> {
        if !key_id.is_empty() {
            return open_bytes(&self.get(key_id)?.key, aad, bytes);
        }
        let mut all: Vec<&Held> = std::iter::once(&self.current)
            .chain(&self.retained)
            .collect();
        all.sort_by_key(|h| h.info.epoch);
        all.into_iter().find_map(|h| open_bytes(&h.key, aad, bytes))
    }
}

/// A retained key as `meta["mesh_keys"]` stores it.
#[derive(Serialize, Deserialize)]
struct Stored {
    info: KeyInfo,
    /// base64 of `seal(engine key, aad = theuth-mesh-key-1 ‖ id, key)`.
    sealed: String,
    #[serde(default)]
    since_ms: u64,
}

fn retained_aad(id: &str) -> Vec<u8> {
    [MESH_KEY_AAD, id.as_bytes()].concat()
}

fn decode_key(s: &str) -> anyhow::Result<Zeroizing<[u8; 32]>> {
    let bytes = Zeroizing::new(
        BASE32_NOPAD
            .decode(s.to_ascii_uppercase().as_bytes())
            .context("vault key: not base32")?,
    );
    let key: [u8; 32] = bytes
        .as_slice()
        .try_into()
        .context("vault key: expected 32 bytes")?;
    Ok(Zeroizing::new(key))
}

/// The legacy key's info: what an engine without `mesh_key_info` holds (§9.5).
fn legacy(key: &[u8; 32]) -> KeyInfo {
    KeyInfo {
        id: key_id(key),
        ..KeyInfo::default()
    }
}

/// [`Vault::open`]: the keyring and the rotation record from `meta`.
pub(super) async fn load(
    pool: &SqlitePool,
    engine: &[u8; 32],
    current: Zeroizing<[u8; 32]>,
) -> anyhow::Result<(Ring, Option<RotationState>)> {
    fn json<T: serde::de::DeserializeOwned>(b: Option<Vec<u8>>) -> Option<T> {
        b.and_then(|b| serde_json::from_slice(&b).ok())
    }
    // A downgraded build may have swapped `mesh_key` (a pairing): the info must name it.
    let info = json(store::meta_get(pool, MESH_KEY_INFO).await?)
        .filter(|i: &KeyInfo| i.id == key_id(&current))
        .unwrap_or_else(|| legacy(&current));
    let stored: Vec<Stored> = json(store::meta_get(pool, MESH_KEYS).await?).unwrap_or_default();
    let retained = stored
        .into_iter()
        .filter_map(|s| {
            let sealed = B64.decode(&s.sealed).ok()?;
            let key = open_bytes(engine, &retained_aad(&s.info.id), &sealed)
                .and_then(|k| <[u8; 32]>::try_from(k.as_slice()).ok());
            let Some(key) = key else {
                tracing::warn!(
                    "secrets: retained mesh key {} does not open; dropped",
                    s.info.id
                );
                return None;
            };
            Some(Held {
                key: Zeroizing::new(key),
                info: s.info,
                since_ms: s.since_ms,
            })
        })
        .filter(|h| h.info.id != info.id)
        .collect();
    let rotation = json(store::meta_get(pool, ROTATION).await?);
    Ok((
        Ring {
            current: Held {
                key: current,
                info,
                since_ms: 0,
            },
            retained,
        },
        rotation,
    ))
}

impl Vault {
    pub(crate) fn ring(&self) -> RwLockReadGuard<'_, Ring> {
        self.ring.read().unwrap_or_else(PoisonError::into_inner)
    }

    /// The current key and its id, for a write.
    pub(crate) fn current(&self) -> (Zeroizing<[u8; 32]>, String) {
        let r = self.ring();
        (r.current.key.clone(), r.current.info.id.clone())
    }

    /// This engine's mesh id: the initiator of a rotation sweeps at once (W-32).
    pub fn set_engine(&self, me: &str) {
        *self.me.write().unwrap_or_else(PoisonError::into_inner) = me.to_owned();
    }

    fn me(&self) -> String {
        self.me
            .read()
            .unwrap_or_else(PoisonError::into_inner)
            .clone()
    }

    /// The current key's info (`KeyEpoch.current`).
    pub fn current_key(&self) -> KeyInfo {
        self.ring().current.info.clone()
    }

    /// Every held key id, current first (`KeyEpoch.held`).
    pub fn held_keys(&self) -> Vec<String> {
        let r = self.ring();
        std::iter::once(&r.current)
            .chain(&r.retained)
            .map(|h| h.info.id.clone())
            .collect()
    }

    /// Key ids local rows name that this engine does not hold (W-38): ask a peer.
    pub fn missing_keys(&self) -> Vec<String> {
        self.missing
            .read()
            .unwrap_or_else(PoisonError::into_inner)
            .iter()
            .cloned()
            .collect()
    }

    /// The current key plus every unretired one, current first.
    pub fn keyring(&self) -> Vec<WireKey> {
        let r = self.ring();
        std::iter::once(&r.current)
            .chain(&r.retained)
            .map(Held::wire)
            .collect()
    }

    /// The newest rotation (`meta["rotation"]`); `None` before the first one.
    pub fn rotation(&self) -> Option<RotationState> {
        self.rotation.borrow().clone()
    }

    /// Every change of [`Vault::rotation`] (the plugin publishes `SecretsEvent::Rotation`).
    pub fn watch_rotation(&self) -> tokio::sync::watch::Receiver<Option<RotationState>> {
        self.rotation.subscribe()
    }

    /// Hold off retiring non-current keys until the hold drops (§6.9.11).
    pub fn retire_hold(&self, who: &'static str) -> RetireHold {
        self.holds.fetch_add(1, Ordering::SeqCst);
        RetireHold {
            count: self.holds.clone(),
            _who: who,
        }
    }

    /// §9.5: the legacy key's members are the paired set at the first boot on this build
    /// (`members` = that set plus this engine). Written once; later calls change nothing.
    pub async fn set_legacy_members(&self, members: Vec<String>) -> anyhow::Result<()> {
        let _g = self.keylock.lock().await;
        if self.ring().current.info.epoch != 0
            || store::meta_get(&self.pool, MESH_KEY_INFO).await?.is_some()
        {
            return Ok(());
        }
        let mut ring = self.ring().clone();
        ring.current.info.members = members;
        self.persist(&ring, None).await?;
        *self.ring.write().unwrap_or_else(PoisonError::into_inner) = ring;
        Ok(())
    }

    /// The rotation's peers as the mesh sees them (§14.7 `RotationPeer`), and the removed
    /// machines' names (183 R2; the mesh still holds their tombstoned rows).
    pub async fn set_rotation_peers(&self, peers: Vec<RotationPeer>, removed_names: Vec<String>) {
        self.update_rotation(|r| {
            r.peers = peers;
            // Kept once known: a tombstone that is gone later never blanks a name.
            if removed_names.iter().any(|n| n != REMOVED_MACHINE) || r.removed_names.is_empty() {
                r.removed_names = removed_names;
            }
        })
        .await;
    }

    /// Mint a new current key for `members` after `removed` left (W-28): stored with the
    /// previous key retained, the rotation record written. The sweep runs from
    /// [`Vault::maintain`], after [`SWEEP_HOLD_MS`] or [`Vault::release_sweep`]. 183 R1: the
    /// epoch is above every held one and `floor` (a peer's epoch), saturating: an exhausted
    /// epoch is an error. `reset_devices` is kept in the record until
    /// [`Vault::finish_device_reset`] ran. A failure leaves the old key current, the record `failed`.
    pub async fn rotate(
        &self,
        me: &str,
        members: Vec<String>,
        removed: Vec<String>,
        floor: u32,
        reset_devices: bool,
    ) -> anyhow::Result<KeyInfo> {
        self.set_engine(me);
        let _g = self.keylock.lock().await;
        let now = now_ms();
        let key = Zeroizing::new(rand::random::<[u8; 32]>());
        let epoch = {
            let r = self.ring();
            std::iter::once(&r.current)
                .chain(&r.retained)
                .map(|h| h.info.epoch)
                .max()
                .unwrap_or(0)
                .max(floor)
        };
        let epoch = epoch.checked_add(1).context("vault-key epoch exhausted")?;
        let info = KeyInfo {
            id: key_id(&key),
            epoch,
            initiator: me.to_owned(),
            members,
            minted_ms: now,
        };
        let state = RotationState {
            key: info.clone(),
            removed,
            phase: RotationPhase::Sweeping,
            started_ms: now,
            rows_total: 0,
            rows_done: 0,
            rows_stuck: Vec::new(),
            peers: self.rotation().map(|r| r.peers).unwrap_or_default(),
            old_destroyed: false,
            error: None,
            device_reset_pending: reset_devices,
            removed_names: Vec::new(),
            sweep_hold_until_ms: now.saturating_add(SWEEP_HOLD_MS),
        };
        let held = Held {
            key,
            info: info.clone(),
            since_ms: 0,
        };
        if let Err(e) = self.install(held, Some(state.clone())).await {
            self.rotation.send_replace(Some(RotationState {
                phase: RotationPhase::Failed,
                error: Some(format!("{e:#}")),
                ..state
            }));
            return Err(e);
        }
        self.sweep_until
            .store(state.sweep_hold_until_ms, Ordering::SeqCst);
        tracing::info!("secrets: minted vault key {} (epoch {epoch})", info.id);
        Ok(info)
    }

    /// A peer acked the new key: the sweep may start ([`Vault::rotate`]'s hold ends).
    pub fn release_sweep(&self) {
        self.sweep_until.store(0, Ordering::SeqCst);
    }

    /// Store a key another engine granted (§6.9.5). It becomes current only if it orders above
    /// the current one (`true`); otherwise it is retained, so rows under it open and get swept.
    /// A key already held only gains members.
    pub async fn adopt_key(&self, k: WireKey) -> anyhow::Result<bool> {
        let key = decode_key(&k.key)?;
        anyhow::ensure!(
            key_id(&key) == k.info.id,
            "vault key {}: the id does not match the key",
            k.info.id
        );
        let _g = self.keylock.lock().await;
        let mut ring = self.ring().clone();
        if let Some(h) = std::iter::once(&mut ring.current)
            .chain(&mut ring.retained)
            .find(|h| h.info.id == k.info.id)
        {
            let before = h.info.members.len();
            for m in k.info.members {
                if !h.info.members.contains(&m) {
                    h.info.members.push(m);
                }
            }
            if h.info.members.len() != before {
                self.persist(&ring, None).await?;
                *self.ring.write().unwrap_or_else(PoisonError::into_inner) = ring;
            }
            return Ok(false);
        }
        let cur = ring.current.info.epoch;
        if k.info.epoch.saturating_sub(cur) > MAX_EPOCH_JUMP {
            tracing::warn!(
                "secrets: refused vault key {} (epoch {}): more than {MAX_EPOCH_JUMP} above {cur}",
                k.info.id,
                k.info.epoch
            );
            anyhow::bail!(
                "vault key {}: epoch {} is implausible",
                k.info.id,
                k.info.epoch
            );
        }
        let now = now_ms();
        let held = Held {
            key,
            info: k.info,
            since_ms: now,
        };
        if order(&held.info) <= order(&ring.current.info) {
            tracing::info!(
                "secrets: retained vault key {} (epoch {})",
                held.info.id,
                held.info.epoch
            );
            ring.retained.push(held);
            self.persist(&ring, None).await?;
            *self.ring.write().unwrap_or_else(PoisonError::into_inner) = ring;
            self.refresh().await?;
            return Ok(false);
        }
        let old = &ring.current.info;
        let state = RotationState {
            key: held.info.clone(),
            removed: old
                .members
                .iter()
                .filter(|m| !held.info.members.contains(m))
                .cloned()
                .collect(),
            phase: RotationPhase::Sweeping,
            started_ms: now,
            rows_total: 0,
            rows_done: 0,
            rows_stuck: Vec::new(),
            peers: self.rotation().map(|r| r.peers).unwrap_or_default(),
            old_destroyed: false,
            error: None,
            device_reset_pending: false,
            removed_names: Vec::new(),
            sweep_hold_until_ms: 0,
        };
        tracing::info!(
            "secrets: vault key {} (epoch {}) is current",
            held.info.id,
            held.info.epoch
        );
        self.install(held, Some(state)).await?;
        Ok(true)
    }

    /// Pairing (§6.9.9): take the key owner's keyring, its first key as current whatever its
    /// order (`key_owner` decided), the rest retained beside ours; then sweep at once, as the
    /// single re-seal of 79 F5 did.
    pub async fn adopt_keyring(&self, keys: Vec<WireKey>) -> anyhow::Result<()> {
        let mut held = Vec::new();
        for k in keys {
            let key = decode_key(&k.key)?;
            anyhow::ensure!(
                key_id(&key) == k.info.id,
                "vault key {}: the id does not match the key",
                k.info.id
            );
            held.push(Held {
                key,
                info: k.info,
                since_ms: now_ms(),
            });
        }
        let Some(first) = held.first().cloned() else {
            return Ok(());
        };
        {
            let _g = self.keylock.lock().await;
            let mut ring = self.ring().clone();
            for h in held.into_iter().skip(1) {
                if ring.get(&h.info.id).is_none() && h.info.id != first.info.id {
                    ring.retained.push(h);
                }
            }
            if ring.current.info.id != first.info.id {
                let mut old = std::mem::replace(&mut ring.current, first);
                old.since_ms = now_ms();
                ring.retained.retain(|h| h.info.id != ring.current.info.id);
                ring.retained.push(old);
                ring.current.since_ms = 0;
            }
            self.persist(&ring, None).await?;
            *self.ring.write().unwrap_or_else(PoisonError::into_inner) = ring;
        }
        self.refresh().await?;
        self.sweep().await?;
        Ok(())
    }

    /// `held` becomes current, the current key retained; meta first, then memory.
    async fn install(&self, mut held: Held, state: Option<RotationState>) -> anyhow::Result<()> {
        let mut ring = self.ring().clone();
        held.since_ms = 0;
        let mut old = std::mem::replace(&mut ring.current, held);
        old.since_ms = now_ms();
        ring.retained.retain(|h| h.info.id != ring.current.info.id);
        ring.retained.push(old);
        self.persist(&ring, state.as_ref()).await?;
        *self.ring.write().unwrap_or_else(PoisonError::into_inner) = ring;
        if let Some(s) = state {
            self.rotation.send_replace(Some(s));
        }
        self.refresh().await
    }

    /// One transaction on local `meta` (W-31): the current key, its info, the retained keys
    /// and, when given, the rotation record.
    async fn persist(&self, ring: &Ring, state: Option<&RotationState>) -> anyhow::Result<()> {
        let stored: Vec<Stored> = ring
            .retained
            .iter()
            .map(|h| Stored {
                info: h.info.clone(),
                sealed: B64.encode(seal(&self.key, &retained_aad(&h.info.id), &*h.key)),
                since_ms: h.since_ms,
            })
            .collect();
        let mut rows = vec![
            (MESH_KEY, seal(&self.key, MESH_KEY_AAD, &*ring.current.key)),
            (MESH_KEY_INFO, serde_json::to_vec(&ring.current.info)?),
            (MESH_KEYS, serde_json::to_vec(&stored)?),
        ];
        if let Some(s) = state {
            rows.push((ROTATION, serde_json::to_vec(s)?));
        }
        let mut tx = self.pool.begin_with("BEGIN IMMEDIATE").await?;
        for (k, v) in rows {
            sqlx::query(
                "INSERT INTO meta (k, v) VALUES (?, ?) ON CONFLICT(k) DO UPDATE SET v = excluded.v",
            )
            .bind(k)
            .bind(v)
            .execute(&mut *tx)
            .await?;
        }
        tx.commit().await?;
        Ok(())
    }

    /// Change the rotation record, if there is one; persisted and published when it moved.
    async fn update_rotation(&self, f: impl FnOnce(&mut RotationState)) -> Option<RotationState> {
        let mut changed = None;
        self.rotation.send_if_modified(|r| {
            let Some(r) = r.as_mut() else {
                return false;
            };
            let before = r.clone();
            f(r);
            if *r == before {
                return false;
            }
            changed = Some(r.clone());
            true
        });
        // Display state only: a failed write is logged, and the next change rewrites it.
        if let Some(s) = &changed
            && let Ok(bytes) = serde_json::to_vec(s)
            && let Err(e) = store::meta_put(&self.pool, ROTATION, &bytes).await
        {
            tracing::debug!("secrets: rotation record: {e:#}");
        }
        changed
    }

    /// §6.9.6 sweep: every row not under the current key, 64 per `BEGIN IMMEDIATE`, each a
    /// compare-and-set on `key_id`. A row that opens under no held key is left and named in
    /// `rows_stuck`. Resumable: the rows' `key_id`s are the only progress.
    pub async fn sweep(&self) -> anyhow::Result<RotationState> {
        let legacy_count = || async {
            sqlx::query_as::<_, (i64,)>("SELECT count(*) FROM vault WHERE key_id = ''")
                .fetch_one(&self.pool)
                .await
                .map(|(n,)| n)
        };
        let legacy_before = legacy_count().await?;
        let mut after = String::new();
        let mut stuck = BTreeSet::new();
        loop {
            let Some(last) = self.sweep_batch(&after, &mut stuck).await? else {
                break;
            };
            after = last;
            self.refresh().await?;
            self.progress(&stuck, false).await?;
        }
        // `''` rows re-sealed: what `retire` last learned of them no longer holds.
        if legacy_count().await? != legacy_before {
            *self
                .legacy_cache
                .lock()
                .unwrap_or_else(PoisonError::into_inner) = None;
        }
        self.progress(&stuck, true).await
    }

    /// One batch of [`Vault::sweep`] after row id `after`; `None` when no row is left.
    pub(crate) async fn sweep_batch(
        &self,
        after: &str,
        stuck: &mut BTreeSet<String>,
    ) -> anyhow::Result<Option<String>> {
        let ring = self.ring().clone();
        let cur = &ring.current;
        let mut tx = self.pool.begin_with("BEGIN IMMEDIATE").await?;
        let rows = sqlx::query(
            "SELECT id, name, key_id, ciphertext FROM vault WHERE key_id <> ? AND id > ? ORDER BY id LIMIT ?",
        )
        .bind(&cur.info.id)
        .bind(after)
        .bind(SWEEP_BATCH)
        .fetch_all(&mut *tx)
        .await?;
        let Some(last) = rows.last().map(|r| r.get::<String, _>("id")) else {
            return Ok(None);
        };
        for r in &rows {
            let (id, name, kid): (String, String, String) =
                (r.get("id"), r.get("name"), r.get("key_id"));
            let ct: Vec<u8> = r.get("ciphertext");
            let Some(plain) = ring.open(&kid, id.as_bytes(), &ct) else {
                stuck.insert(name);
                continue;
            };
            sqlx::query("UPDATE vault SET ciphertext = ?, key_id = ? WHERE id = ? AND key_id = ?")
                .bind(seal(&cur.key, id.as_bytes(), &plain))
                .bind(&cur.info.id)
                .bind(&id)
                .bind(&kid)
                .execute(&mut *tx)
                .await?;
        }
        tx.commit().await?;
        Ok(Some(last))
    }

    /// Row counts into the record; `done` = the sweep finished (`waiting`, or `done` when no
    /// other key is held). Without a record (a pairing's sweep) a state is built, not stored.
    async fn progress(
        &self,
        stuck: &BTreeSet<String>,
        done: bool,
    ) -> anyhow::Result<RotationState> {
        let ring = self.ring().clone();
        let (total, under): (i64, i64) =
            sqlx::query_as("SELECT count(*), coalesce(sum(key_id = ?), 0) FROM vault")
                .bind(&ring.current.info.id)
                .fetch_one(&self.pool)
                .await?;
        let apply = |r: &mut RotationState| {
            if r.key.id != ring.current.info.id || r.phase == RotationPhase::Failed {
                return;
            }
            r.rows_total = total as u32;
            r.rows_done = under as u32;
            r.rows_stuck = stuck.iter().cloned().collect();
            r.phase = match (done, ring.retained.is_empty()) {
                (false, _) => RotationPhase::Sweeping,
                (true, false) => RotationPhase::Waiting,
                (true, true) => RotationPhase::Done,
            };
        };
        if let Some(s) = self.update_rotation(apply).await {
            return Ok(s);
        }
        Ok(self.rotation().unwrap_or_else(|| RotationState {
            key: ring.current.info.clone(),
            removed: Vec::new(),
            phase: RotationPhase::Done,
            started_ms: 0,
            rows_total: total as u32,
            rows_done: under as u32,
            rows_stuck: stuck.iter().cloned().collect(),
            peers: Vec::new(),
            old_destroyed: false,
            error: None,
            device_reset_pending: false,
            removed_names: Vec::new(),
            sweep_hold_until_ms: 0,
        }))
    }

    /// The plugin's tick (`now` from the caller, so tests drive the clock): sweep when rows are
    /// under an older key and this engine minted the current one or the 60 s grace passed
    /// (W-32), then retire what can go (W-33). Nothing while only the current key is held: the
    /// `''` rows of §9.5 are never rewritten outside a rotation.
    pub async fn maintain(&self, now: u64) -> anyhow::Result<()> {
        // 183 R1: a crash between the mint and the device-key reset retries here. R4: its
        // failure is logged; the sweep and retire still run this tick.
        if let Err(e) = self.finish_device_reset().await {
            tracing::warn!("secrets: device-key reset still pending: {e:#}");
        }
        let (cur, retained) = {
            let r = self.ring();
            (r.current.info.clone(), !r.retained.is_empty())
        };
        if !retained {
            return Ok(());
        }
        let due = match self.rotation() {
            Some(r) if r.key.id == cur.id => {
                cur.initiator == self.me() || now >= r.started_ms.saturating_add(SWEEP_GRACE_MS)
            }
            _ => true,
        };
        let (left,): (i64,) = sqlx::query_as("SELECT count(*) FROM vault WHERE key_id <> ?")
            .bind(&cur.id)
            .fetch_one(&self.pool)
            .await?;
        if due && left > 0 && now >= self.sweep_until.load(Ordering::SeqCst) {
            self.sweep().await?;
        }
        self.retire(now).await
    }

    /// W-33: destroy each retained key no row names (for `''` rows: none opens with it), at
    /// least [`RETIRE_GRACE_MS`] after it stopped being current, while no hold is open.
    pub(crate) async fn retire(&self, now: u64) -> anyhow::Result<()> {
        if self.holds.load(Ordering::SeqCst) > 0 {
            return Ok(());
        }
        let _g = self.keylock.lock().await;
        let mut ring = self.ring().clone();
        let due: Vec<&Held> = ring
            .retained
            .iter()
            .filter(|h| now >= h.since_ms.saturating_add(RETIRE_GRACE_MS))
            .collect();
        if due.is_empty() {
            return Ok(());
        }
        // 183 R1: the "no row names it / no `''` row opens with it" check and the write are one
        // transaction, so a row landing in between is counted or written after
        // (secure_delete: this connection).
        let mut tx = self.pool.begin_with("BEGIN IMMEDIATE").await?;
        sqlx::query("PRAGMA secure_delete = ON")
            .execute(&mut *tx)
            .await?;
        // Keys a `''` row opened with at the last trial: kept without a new trial for
        // [`LEGACY_CHECK_MS`] (an old build keeps the legacy key alive). A key not known to
        // open one is tried now, before it can go: the cache only ever keeps a key.
        let mut keeps = self.legacy_keeps(now);
        let mut legacy_rows: Option<Vec<(String, Vec<u8>)>> = None;
        let mut gone = Vec::new();
        for h in due {
            let (named,): (i64,) = sqlx::query_as("SELECT count(*) FROM vault WHERE key_id = ?")
                .bind(&h.info.id)
                .fetch_one(&mut *tx)
                .await?;
            if named > 0 || keeps.contains(&h.info.id) {
                continue;
            }
            if legacy_rows.is_none() {
                legacy_rows = Some(
                    sqlx::query_as("SELECT id, ciphertext FROM vault WHERE key_id = ''")
                        .fetch_all(&mut *tx)
                        .await?,
                );
            }
            let rows = legacy_rows.as_ref().expect("just read");
            if rows
                .iter()
                .any(|(id, ct)| open_bytes(&h.key, id.as_bytes(), ct).is_some())
            {
                keeps.insert(h.info.id.clone());
            } else {
                gone.push(h.info.id.clone());
            }
        }
        if legacy_rows.is_some() {
            *self
                .legacy_cache
                .lock()
                .unwrap_or_else(PoisonError::into_inner) = Some((now, keeps));
        }
        if gone.is_empty() {
            return Ok(());
        }
        ring.retained.retain(|h| !gone.contains(&h.info.id));
        let stored: Vec<Stored> = ring
            .retained
            .iter()
            .map(|h| Stored {
                info: h.info.clone(),
                sealed: B64.encode(seal(&self.key, &retained_aad(&h.info.id), &*h.key)),
                since_ms: h.since_ms,
            })
            .collect();
        sqlx::query(
            "INSERT INTO meta (k, v) VALUES (?, ?) ON CONFLICT(k) DO UPDATE SET v = excluded.v",
        )
        .bind(MESH_KEYS)
        .bind(serde_json::to_vec(&stored)?)
        .execute(&mut *tx)
        .await?;
        tx.commit().await?;
        let none_left = ring.retained.is_empty();
        // The dropped `Held`s zeroize their keys.
        *self.ring.write().unwrap_or_else(PoisonError::into_inner) = ring;
        for id in &gone {
            tracing::info!("secrets: retired vault key {id}");
        }
        // The rotation's own old key is the newest retained one: it is gone once none is left.
        if none_left {
            self.update_rotation(|r| {
                r.old_destroyed = true;
                if r.phase == RotationPhase::Waiting {
                    r.phase = RotationPhase::Done;
                }
            })
            .await;
        }
        Ok(())
    }

    /// Retained key ids a `''` row opened with at the last trial, while that trial is younger
    /// than [`LEGACY_CHECK_MS`] of `now`; empty otherwise.
    fn legacy_keeps(&self, now: u64) -> BTreeSet<String> {
        match &*self
            .legacy_cache
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
        {
            Some((at, ids)) if now < at.saturating_add(LEGACY_CHECK_MS) => ids.clone(),
            _ => BTreeSet::new(),
        }
    }

    /// W-36: user secret names and `oauth/<ns>` namespaces with a kept version written no later
    /// than `cutoff_ms` (`None` = all). `mesh/*` and `sandbox/*` are never named.
    pub fn names_written_by(&self, cutoff_ms: Option<u64>) -> (Vec<String>, Vec<String>) {
        let g = self.read();
        let (mut names, mut grants) = (Vec::new(), Vec::new());
        for (name, e) in &g.entries {
            if cutoff_ms.is_some_and(|c| e.versions.iter().all(|v| v.ms > c)) {
                continue;
            }
            if let Some(ns) = name.strip_prefix(OAUTH_PREFIX) {
                grants.push(ns.to_owned());
            } else if !name.contains('/') {
                names.push(name.clone());
            }
        }
        (names, grants)
    }

    /// D11: a new `mesh/device_key` value (a new version of the row, sealed under the current
    /// mesh key, so it replicates to the remaining machines only). `Ok(false)` = there was none.
    pub async fn rotate_device_key(&self) -> anyhow::Result<bool> {
        let Some((v, _)) = self.get_versioned(Self::MESH_DEVICE_KEY) else {
            // 183 R4: a row this engine cannot open is not "none": the reset stays pending.
            let (rows,): (i64,) = sqlx::query_as("SELECT count(*) FROM vault WHERE name = ?")
                .bind(Self::MESH_DEVICE_KEY)
                .fetch_one(&self.pool)
                .await?;
            anyhow::ensure!(
                rows == 0,
                "mesh/device_key: the row opens under no held key"
            );
            return Ok(false);
        };
        let fresh: Zeroizing<[u8; 32]> = Zeroizing::new(rand::random());
        let encoded = Zeroizing::new(URL_SAFE_NO_PAD.encode(*fresh));
        self.set_if_version(Self::MESH_DEVICE_KEY, &encoded, "reset", Some(v))
            .await
            .map_err(|e| anyhow::anyhow!("mesh/device_key: {e}"))?
            .ok_or_else(|| anyhow::anyhow!("mesh/device_key: concurrent write; retry"))?;
        Ok(true)
    }

    /// 183 R1 (D11): finish a pending device-key reset the current rotation recorded: a new
    /// `mesh/device_key` version, then the flag cleared. `Ok(true)` = a key was written.
    pub async fn finish_device_reset(&self) -> anyhow::Result<bool> {
        let pending = || {
            self.rotation()
                .is_some_and(|r| r.device_reset_pending && r.phase != RotationPhase::Failed)
        };
        if !pending() {
            return Ok(false);
        }
        let _g = self.keylock.lock().await;
        // Checked again under the lock: the rpc and the tick may both get here.
        if !pending() {
            return Ok(false);
        }
        let wrote = self.rotate_device_key().await?;
        self.update_rotation(|r| r.device_reset_pending = false)
            .await;
        Ok(wrote)
    }
}
