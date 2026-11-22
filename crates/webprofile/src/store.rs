//! `<data_dir>/webprofile.sqlite` (209 §4.1): the replicated table `web_sites`, one row per cookie
//! and one per origin's localStorage snapshot, so replica's per-row last-writer-wins is per-cookie
//! LWW and a change replicates as itself. Each row seals one [`Entry`] — the site, the profile
//! epoch, the writer, the write time and the cookie or snapshot — with the vault's mesh key
//! (`aad = "web_sites\n" ‖ id`), so a row is one atomic value. `sealed = NULL` is a tombstone: a
//! removal is a column write, never a `DELETE` (a delete loses to a newer column write), and
//! tombstones are deleted 30 days after their write.
//!
//! Row ids are keyed hashes under an id key derived from the current mesh key
//! (`HMAC-SHA256(mesh key, "theuth/webprofile/id/v1")`): every engine holding the mesh keyring
//! computes the same ids with nothing created per engine, and the file at rest names no site. When
//! the mesh key rotates (183 §6.9) the ids change with it: the first operation that sees the new
//! key — or the plugin's [`Profile::maintain`] within a minute — moves every row to its new id
//! (re-sealed under the new key, the old id tombstoned), and every later full read folds rows a
//! lagging engine still wrote under the old ids ([`Profile::load`] with every site). Every engine
//! computes the same new ids, so concurrent moves converge. While any readable row still opens only
//! under a retained key — from the moment the store sees a new key, at open, or when a peer's row
//! arrives sealed under one — the store holds a [`secrets::RetireHold`], so the vault cannot
//! retire that key before the rows moved; [`crate::watch_keys`] notices a new key at once.
//!
//! The row `id = "epoch"` seals the profile epoch; entries written under an older epoch are dead.
//! The epoch never goes down: the local table `web_profile_local` keeps the highest one seen, a
//! clear writes that plus one, and a replicated epoch row lower than it is written upward again by
//! [`Profile::maintain`]. `web_profile_forget` writes a forget marker per site (id keyed
//! `["forget", site]`, no `site_id`): for 30 days ([`caps::FORGOTTEN_MS`]) an entry of that site
//! is dead when it was written before the marker or by a writer whose `basis_ms` (its last full
//! read) is before it — a writer naming no basis counts as basis 0 (`dead`).
//! Writes of one operation are one transaction; the full scans ([`Profile::load`] of every site,
//! [`Profile::pull_page`], the rotation move) run beside the actor, so every write they make is
//! conditional on the row still holding what they read. `hlc` is left NULL for the replica to
//! stamp at capture; a local trigger records when each tombstone was written, so tombstones are
//! purged after 30 days with or without replication.

use std::collections::{BTreeMap, BTreeSet, HashMap, HashSet};
use std::path::Path;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex, PoisonError};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use anyhow::Context as _;
use base64::Engine as _;
use base64::engine::general_purpose::URL_SAFE_NO_PAD;
use serde::{Deserialize, Serialize};
use sha2::{Digest as _, Sha256};
use sqlx::Row as _;
use sqlx::sqlite::{SqliteConnectOptions, SqliteJournalMode, SqlitePool, SqlitePoolOptions};
use zeroize::Zeroizing;

use crate::{
    Cookie, CookieChange, ForgottenSite, LoadedProfile, LoadedSite, OriginStorage, ProfileError,
    ProfileMode, PullResult, PulledCookie, PulledStorage, Push, PushResult, SiteJar, SiteSummary,
    caps, merge,
};

/// Sqlite file under `data_dir`.
pub const DB: &str = "webprofile.sqlite";
/// The tables `replica::attach` takes.
pub const TABLES: &[&str] = &["web_sites"];
/// Id of the epoch row.
pub const EPOCH_ID: &str = "epoch";
/// Context of the id key derived from the mesh key (209 §4.1).
pub const ID_CONTEXT: &[u8] = b"theuth/webprofile/id/v1";
/// `aad` prefix of a sealed row.
const AAD: &[u8] = b"web_sites\n";
/// Format version of an [`Entry`]; a row of another version is skipped (and counted).
const ENTRY_V: u32 = 1;
/// AEAD overhead of a sealed row before base64: 24-byte nonce and 16-byte tag.
const SEAL_OVERHEAD: u64 = 40;

/// Unix ms now.
pub fn now_ms() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |d| d.as_millis() as u64)
}

/// HMAC-SHA256 (RFC 2104) over `sha2`, the one hash the crate already carries.
fn hmac_sha256(key: &[u8], msg: &[u8]) -> [u8; 32] {
    let mut k0 = Zeroizing::new([0u8; 64]);
    if key.len() > 64 {
        let d: [u8; 32] = Sha256::digest(key).into();
        k0[..32].copy_from_slice(&d);
    } else {
        k0[..key.len()].copy_from_slice(key);
    }
    let pad = |b: u8| Zeroizing::new(k0.iter().map(|k| k ^ b).collect::<Vec<u8>>());
    let inner: [u8; 32] = Sha256::new()
        .chain_update(&*pad(0x36))
        .chain_update(msg)
        .finalize()
        .into();
    Sha256::new()
        .chain_update(&*pad(0x5c))
        .chain_update(inner)
        .finalize()
        .into()
}

fn aad(id: &str) -> Vec<u8> {
    [AAD, id.as_bytes()].concat()
}

/// The id key of one mesh key and the keyed hashes rows are named by (209 §4.1).
pub struct Ids {
    /// `secrets::vault::key_id` of the mesh key it was derived from.
    key_id: String,
    key: Zeroizing<[u8; 32]>,
}

impl Ids {
    /// The ids under `mesh_key`: id key = HMAC-SHA256(mesh key, [`ID_CONTEXT`]).
    pub fn of(mesh_key: &[u8; 32]) -> Self {
        Self {
            key_id: secrets::vault::key_id(mesh_key),
            key: Zeroizing::new(hmac_sha256(mesh_key, ID_CONTEXT)),
        }
    }

    /// First 32 hex of HMAC-SHA256(id key, JSON array of `parts`).
    fn hash(&self, parts: &[&str]) -> String {
        let msg = serde_json::to_vec(parts).unwrap_or_default();
        let mut s = hex::encode(hmac_sha256(&*self.key, &msg));
        s.truncate(32);
        s
    }

    /// `site_id` of `site` (eTLD+1): groups, lists and forgets a site without opening a row.
    pub fn site(&self, site: &str) -> String {
        self.hash(&["site", site])
    }

    /// Row id of the cookie `(name, domain, path)` of `site` (the key as [`merge::norm`] has it).
    pub fn cookie(&self, site: &str, name: &str, domain: &str, path: &str) -> String {
        let (name, domain, path) = merge::norm(name, domain, path);
        self.hash(&["cookie", site, &name, &domain, &path])
    }

    /// Row id of `origin`'s localStorage snapshot.
    pub fn storage(&self, site: &str, origin: &str) -> String {
        self.hash(&["storage", site, origin])
    }

    /// Row id of `site`'s forget marker (no `site_id`, so it never counts as a live row).
    pub fn forget(&self, site: &str) -> String {
        self.hash(&["forget", site])
    }

    fn of_entry(&self, e: &Entry) -> String {
        match &e.item {
            Item::Cookie(c) => self.cookie(&e.site, &c.name, &c.domain, &c.path),
            Item::Storage(s) => self.storage(&e.site, &s.origin),
            Item::Epoch {} => EPOCH_ID.to_owned(),
            Item::Forget {} => self.forget(&e.site),
        }
    }
}

/// The plaintext of one row. No `Debug`: it holds a value.
#[derive(Clone, Serialize, Deserialize)]
struct Entry {
    /// [`ENTRY_V`].
    v: u32,
    /// Lower-case eTLD+1 (`""` on the epoch row).
    site: String,
    /// The profile epoch it was written under (the epoch itself on the epoch row).
    epoch: u64,
    /// `tile:<device>` | `obscura:<engine>`.
    writer: String,
    /// Unix ms of the write.
    updated_ms: u64,
    /// When the writer last read the whole profile ([`Push::basis_ms`]); absent for a tile.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    basis_ms: Option<u64>,
    item: Item,
}

#[derive(Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
enum Item {
    Cookie(Cookie),
    Storage(OriginStorage),
    Epoch {},
    /// The site's forget marker: `updated_ms` is when it was forgotten.
    Forget {},
}

impl Item {
    /// Its plaintext size for the per-site caps.
    fn bytes(&self) -> usize {
        serde_json::to_vec(self).map_or(0, |v| v.len())
    }
}

/// Whether an entry of a site written at `updated_ms` by a writer that read the profile at
/// `basis_ms` is dead under the site's forget marker of `forgot_ms` (209 §4.1): written before the
/// forget, or by a writer whose last full read came before it. A writer naming no read time (an
/// old client) counts as basis 0, so it never brings a forgotten site back.
pub(crate) fn dead(updated_ms: u64, basis_ms: Option<u64>, forgot_ms: Option<u64>) -> bool {
    let Some(f) = forgot_ms else { return false };
    updated_ms < f || basis_ms.unwrap_or(0) < f
}

/// Whether a forget marker written at `forgot_ms` still applies at `now` ([`caps::FORGOTTEN_MS`]).
fn marker_live(forgot_ms: u64, now: u64) -> bool {
    forgot_ms.saturating_add(caps::FORGOTTEN_MS) >= now
}

/// Key of the highest epoch seen in `web_profile_local`.
const EPOCH_SEEN: &str = "epoch_seen";

/// Re-count `web_site_stats` from the rows (after a clear or a scan tombstoned rows the triggers
/// counted while they were live).
const RESTAT: [&str; 2] = [
    "DELETE FROM web_site_stats",
    "INSERT INTO web_site_stats (site_id, n, sealed_bytes)
     SELECT site_id, count(*), sum(length(sealed)) FROM web_sites
     WHERE site_id IS NOT NULL AND sealed IS NOT NULL GROUP BY site_id",
];

/// Only the version, read first so a row of a newer format is skipped rather than misread.
#[derive(Deserialize)]
struct Head {
    v: u32,
}

/// What opening a sealed row gave.
enum Opened {
    Entry(Box<Entry>),
    /// A format version this build does not know.
    Unknown,
    /// No held key opens it, a wrong aad, damaged bytes or a body that does not parse.
    Unreadable,
}

/// One row as stored.
struct Raw {
    id: String,
    site_id: Option<String>,
    sealed: Option<String>,
}

fn raw(r: sqlx::sqlite::SqliteRow) -> Raw {
    Raw {
        id: r.get("id"),
        site_id: r.get("site_id"),
        sealed: r.get("sealed"),
    }
}

/// The live rows of one site as a push sees them.
#[derive(Default)]
struct SiteRows {
    /// Row id → its entry and the item's size.
    live: HashMap<String, (Entry, usize)>,
    /// Tombstoned row ids.
    removed: HashSet<String>,
    /// When the site was last forgotten (its marker's `updated_ms`), in this epoch.
    forgot: Option<u64>,
}

/// The profile store: pool, vault and the ids of the current mesh key. The actor owns one and
/// shares it with the tasks that scan.
pub struct Profile {
    pool: SqlitePool,
    vault: Arc<secrets::Vault>,
    ids: Mutex<Arc<Ids>>,
    /// Held while rows may still open only under a retained mesh key: the vault retires no key
    /// meanwhile (209 §4.1 rotation). Released by [`Self::fold`] only when a check taken under
    /// the hold, after the move, finds no row under a retained key.
    hold: Mutex<Option<secrets::RetireHold>>,
    /// A write saw a new mesh key, or a move failed: [`Self::fold`] is owed.
    owed: AtomicBool,
    /// One move at a time (the minute tick, the key watcher and a peer's batch may each ask).
    folding: tokio::sync::Mutex<()>,
    /// One epoch bump at a time.
    bumping: tokio::sync::Mutex<()>,
}

impl Profile {
    /// Open or create the file, run the migrations, delete tombstones older than
    /// [`caps::TOMBSTONE_MS`], and move rows a key rotation left under old ids (holding the
    /// vault's retired keys until they moved). A failed purge or move is logged and left to
    /// [`Self::maintain`] (the hold stays open), so a busy file at start does not take the profile
    /// down until a restart.
    pub async fn open(path: &Path, vault: Arc<secrets::Vault>) -> anyhow::Result<Self> {
        let opts = SqliteConnectOptions::new()
            .filename(path)
            .create_if_missing(true)
            .journal_mode(SqliteJournalMode::Wal)
            .busy_timeout(Duration::from_secs(5));
        let pool = SqlitePoolOptions::new()
            .max_connections(4)
            .connect_with(opts)
            .await
            .with_context(|| format!("open {}", path.display()))?;
        sqlx::migrate!("./migrations")
            .run(&pool)
            .await
            .context("run migrations")?;
        let ids = Mutex::new(Arc::new(Ids::of(&vault.mesh_key())));
        let hold = Mutex::new(Some(vault.retire_hold("webprofile")));
        let p = Self {
            pool,
            vault,
            ids,
            hold,
            owed: AtomicBool::new(false),
            folding: tokio::sync::Mutex::new(()),
            bumping: tokio::sync::Mutex::new(()),
        };
        if let Err(e) = p.purge(now_ms()).await {
            tracing::warn!("webprofile: tombstones not purged at open: {e}");
        }
        if let Err(e) = p.fold().await {
            p.owed.store(true, Ordering::SeqCst);
            tracing::warn!("webprofile: rows not moved at open ({e}); maintenance retries");
        }
        Ok(p)
    }

    /// The underlying pool (`replica::attach` takes it).
    pub fn pool(&self) -> &SqlitePool {
        &self.pool
    }

    /// The vault the rows are sealed with.
    pub fn vault(&self) -> &Arc<secrets::Vault> {
        &self.vault
    }

    /// Keep the vault from retiring any key until [`Self::fold`] found every readable row under
    /// the current one.
    fn hold(&self) {
        let mut hold = self.hold.lock().unwrap_or_else(PoisonError::into_inner);
        if hold.is_none() {
            *hold = Some(self.vault.retire_hold("webprofile"));
        }
    }

    /// Whether a hold is open (a move to the current key is owed or not yet checked).
    pub fn holding(&self) -> bool {
        self.hold
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .is_some()
    }

    /// Whether a move to the current mesh key is owed: the actor then runs [`Self::maintain`] on
    /// its own task.
    pub fn fold_owed(&self) -> bool {
        self.owed.load(Ordering::SeqCst)
    }

    /// The ids of the current mesh key, and whether the key changed since the last call. A change
    /// takes the retire hold at once, before any await, and marks the move owed; the move itself
    /// runs beside the actor ([`Self::maintain`]), and full reads fold rows under old ids meanwhile.
    fn ids(&self) -> (Arc<Ids>, bool) {
        let key = self.vault.mesh_key();
        let mut held = self.ids.lock().unwrap_or_else(PoisonError::into_inner);
        if held.key_id == secrets::vault::key_id(&key) {
            return (held.clone(), false);
        }
        self.hold();
        self.owed.store(true, Ordering::SeqCst);
        *held = Arc::new(Ids::of(&key));
        (held.clone(), true)
    }

    /// The plugin's minute tick and [`crate::watch_keys`]: purge old tombstones, write a replicated
    /// epoch lower than the highest seen upward again, and — while the vault holds more than one
    /// mesh key, after a key change or a failed move — move every row to the current key's ids
    /// under the retire hold ([`Self::fold`]).
    pub async fn maintain(&self) -> Result<(), ProfileError> {
        self.purge(now_ms()).await?;
        self.raise_epoch().await?;
        let (_, changed) = self.ids();
        if changed {
            tracing::info!("webprofile: the mesh key changed; moving rows to its ids");
        }
        if changed || self.fold_owed() || self.holding() || self.vault.held_keys().len() > 1 {
            self.fold().await?;
        }
        Ok(())
    }

    /// The `site_id` of `site` under the current mesh key.
    pub fn site_id(&self, site: &str) -> String {
        self.ids().0.site(site)
    }

    fn open_entry(&self, id: &str, sealed: &str) -> Opened {
        let Some(plain) = URL_SAFE_NO_PAD
            .decode(sealed)
            .ok()
            .and_then(|b| self.vault.open_with_mesh_key(&aad(id), &b))
        else {
            return Opened::Unreadable;
        };
        match serde_json::from_slice::<Head>(&plain) {
            Ok(h) if h.v == ENTRY_V => {}
            Ok(_) => return Opened::Unknown,
            Err(_) => return Opened::Unreadable,
        }
        match serde_json::from_slice::<Entry>(&plain) {
            Ok(e) => Opened::Entry(Box::new(e)),
            Err(e) => {
                tracing::warn!("webprofile: row {id} does not parse: {e}");
                Opened::Unreadable
            }
        }
    }

    /// Seal `entry` for row `id`: base64url(nonce ‖ XChaCha20-Poly1305(mesh key, aad, JSON)).
    fn seal(&self, id: &str, entry: &Entry) -> Result<String, ProfileError> {
        let plain = Zeroizing::new(
            serde_json::to_vec(entry)
                .map_err(|e| ProfileError::Unavailable(format!("serialise row {id}: {e}")))?,
        );
        Ok(URL_SAFE_NO_PAD.encode(self.vault.seal_with_mesh_key(&aad(id), &plain)))
    }

    /// The current profile epoch: the highest of the replicated epoch row and the highest epoch
    /// this engine has seen (`0` before any sign out everywhere). A replicated epoch higher than
    /// the one seen is recorded.
    pub async fn epoch(&self) -> Result<u64, ProfileError> {
        let replicated = self.replicated_epoch().await?;
        let seen = self.seen_epoch().await?;
        if replicated > seen {
            sqlx::query(
                "INSERT INTO web_profile_local (key, value) VALUES (?, ?)
                 ON CONFLICT (key) DO UPDATE SET value = max(value, excluded.value)",
            )
            .bind(EPOCH_SEEN)
            .bind(replicated as i64)
            .execute(&self.pool)
            .await
            .map_err(db)?;
        }
        Ok(replicated.max(seen))
    }

    /// The epoch the replicated row holds (`0` when there is none or it does not open here).
    async fn replicated_epoch(&self) -> Result<u64, ProfileError> {
        let sealed: Option<Option<String>> =
            sqlx::query_scalar("SELECT sealed FROM web_sites WHERE id = ?")
                .bind(EPOCH_ID)
                .fetch_optional(&self.pool)
                .await
                .map_err(db)?;
        Ok(match sealed.flatten() {
            Some(s) => match self.open_entry(EPOCH_ID, &s) {
                Opened::Entry(e) => e.epoch,
                _ => 0,
            },
            None => 0,
        })
    }

    /// The highest epoch this engine has seen.
    async fn seen_epoch(&self) -> Result<u64, ProfileError> {
        let seen: Option<i64> =
            sqlx::query_scalar("SELECT value FROM web_profile_local WHERE key = ?")
                .bind(EPOCH_SEEN)
                .fetch_optional(&self.pool)
                .await
                .map_err(db)?;
        Ok(seen.unwrap_or(0).max(0) as u64)
    }

    fn epoch_entry(epoch: u64) -> Entry {
        Entry {
            v: ENTRY_V,
            site: String::new(),
            epoch,
            writer: String::new(),
            updated_ms: now_ms(),
            basis_ms: None,
            item: Item::Epoch {},
        }
    }

    /// Set the epoch to `epoch` and tombstone every row (sign out everywhere writes `n + 1`, 209
    /// §4.1, §4.4 `web_profile_clear`); the site stats are counted again. Refused when `epoch` is
    /// below the current one: the epoch never goes down.
    pub async fn set_epoch(&self, epoch: u64) -> Result<(), ProfileError> {
        let current = self.epoch().await?;
        if epoch < current {
            return Err(ProfileError::Rejected(format!(
                "epoch {epoch} is below the current {current}"
            )));
        }
        let sealed = self.seal(EPOCH_ID, &Self::epoch_entry(epoch))?;
        let mut tx = self.pool.begin_with("BEGIN IMMEDIATE").await.map_err(db)?;
        put(&mut tx, EPOCH_ID, None, Some(&sealed)).await?;
        sqlx::query(
            "UPDATE web_sites SET sealed = NULL, hlc = NULL WHERE id != ? AND sealed IS NOT NULL",
        )
        .bind(EPOCH_ID)
        .execute(&mut *tx)
        .await
        .map_err(db)?;
        sqlx::query(
            "INSERT INTO web_profile_local (key, value) VALUES (?, ?)
             ON CONFLICT (key) DO UPDATE SET value = max(value, excluded.value)",
        )
        .bind(EPOCH_SEEN)
        .bind(epoch as i64)
        .execute(&mut *tx)
        .await
        .map_err(db)?;
        for q in RESTAT {
            sqlx::query(q).execute(&mut *tx).await.map_err(db)?;
        }
        tx.commit().await.map_err(db)
    }

    /// A replicated epoch row lower than the highest epoch seen here (two engines cleared apart
    /// and last-writer-wins kept the lower number) is written again with the highest one.
    async fn raise_epoch(&self) -> Result<(), ProfileError> {
        let (replicated, seen) = (self.replicated_epoch().await?, self.seen_epoch().await?);
        if replicated >= seen {
            return Ok(());
        }
        tracing::info!(
            replicated,
            seen,
            "webprofile: the replicated epoch went down; writing the highest seen"
        );
        let sealed = self.seal(EPOCH_ID, &Self::epoch_entry(seen))?;
        let mut tx = self.pool.begin_with("BEGIN IMMEDIATE").await.map_err(db)?;
        put(&mut tx, EPOCH_ID, None, Some(&sealed)).await?;
        tx.commit().await.map_err(db)
    }

    /// Sign out everywhere (209 §4.4 `web_profile_clear`): the epoch goes to the highest seen plus
    /// one and every row is tombstoned. Returns the new epoch.
    pub async fn clear(&self) -> Result<u64, ProfileError> {
        let _one = self.bumping.lock().await;
        let epoch = self.epoch().await?.saturating_add(1);
        self.set_epoch(epoch).await?;
        Ok(epoch)
    }

    /// Forget `site` (a registrable domain) now; see [`Self::forget_at`].
    pub async fn forget(&self, site: &str, writer: &str) -> Result<u64, ProfileError> {
        self.forget_at(site, writer, now_ms()).await
    }

    /// Forget `site` at `now` (209 §4.4 `web_profile_forget`): every row of its `site_id` is
    /// tombstoned and its forget marker written, so an entry of the site written before `now`
    /// (a row this engine has not seen yet, a lagging engine's) is dead wherever it lands, and so
    /// is a stale writer's later one (`dead`). Returns the current epoch.
    pub async fn forget_at(&self, site: &str, writer: &str, now: u64) -> Result<u64, ProfileError> {
        let ids = self.ids().0;
        let epoch = self.epoch().await?;
        let marker = Entry {
            v: ENTRY_V,
            site: site.to_owned(),
            epoch,
            writer: writer.to_owned(),
            updated_ms: now,
            basis_ms: None,
            item: Item::Forget {},
        };
        let id = ids.forget(site);
        let sealed = self.seal(&id, &marker)?;
        let mut tx = self.pool.begin().await.map_err(db)?;
        sqlx::query(
            "UPDATE web_sites SET sealed = NULL, hlc = NULL WHERE site_id = ? AND sealed IS NOT NULL",
        )
        .bind(ids.site(site))
        .execute(&mut *tx)
        .await
        .map_err(db)?;
        put(&mut tx, &id, None, Some(&sealed)).await?;
        tx.commit().await.map_err(db)?;
        Ok(epoch)
    }

    /// When `site` was last forgotten in epoch `epoch` or later (its marker's `updated_ms`), when
    /// that marker still applies at `now` ([`caps::FORGOTTEN_MS`]).
    async fn forgot(
        &self,
        ids: &Ids,
        site: &str,
        epoch: u64,
        now: u64,
    ) -> Result<Option<u64>, ProfileError> {
        let id = ids.forget(site);
        let sealed: Option<Option<String>> =
            sqlx::query_scalar("SELECT sealed FROM web_sites WHERE id = ?")
                .bind(&id)
                .fetch_optional(&self.pool)
                .await
                .map_err(db)?;
        Ok(match sealed.flatten().map(|s| self.open_entry(&id, &s)) {
            Some(Opened::Entry(e))
                if matches!(e.item, Item::Forget {})
                    && e.site == site
                    && e.epoch >= epoch
                    && marker_live(e.updated_ms, now) =>
            {
                Some(e.updated_ms)
            }
            _ => None,
        })
    }

    /// Every forget marker of epoch `epoch` or later that still applies at `now`, newest per site,
    /// sorted by site (`web_profile_pull`'s `forgotten`). Reads only the marker rows (no
    /// `site_id`).
    async fn forgotten(&self, epoch: u64, now: u64) -> Result<Vec<ForgottenSite>, ProfileError> {
        let rows: Vec<(String, String)> = sqlx::query_as(
            "SELECT id, sealed FROM web_sites
             WHERE site_id IS NULL AND id != ? AND sealed IS NOT NULL",
        )
        .bind(EPOCH_ID)
        .fetch_all(&self.pool)
        .await
        .map_err(db)?;
        let mut at: BTreeMap<String, u64> = BTreeMap::new();
        for (id, sealed) in rows {
            if let Opened::Entry(e) = self.open_entry(&id, &sealed)
                && matches!(e.item, Item::Forget {})
                && e.epoch >= epoch
                && marker_live(e.updated_ms, now)
            {
                let t = at.entry(e.site).or_default();
                *t = (*t).max(e.updated_ms);
            }
        }
        Ok(at
            .into_iter()
            .map(|(site, forgotten_ms)| ForgottenSite { site, forgotten_ms })
            .collect())
    }

    /// Live sites, live rows and their plaintext bytes, estimated from the local stats table
    /// (never by re-reading the rows).
    async fn totals(&self) -> Result<(usize, u64), ProfileError> {
        let r = sqlx::query(
            "SELECT count(*), coalesce(sum(n), 0), coalesce(sum(sealed_bytes), 0)
             FROM web_site_stats WHERE n > 0",
        )
        .fetch_one(&self.pool)
        .await
        .map_err(db)?;
        let sites = r.get::<i64, _>(0).max(0) as usize;
        let rows = r.get::<i64, _>(1).max(0) as u64;
        let bytes = r.get::<i64, _>(2).max(0) as u64;
        Ok((sites, (bytes * 3 / 4).saturating_sub(rows * SEAL_OVERHEAD)))
    }

    /// `web_profile_status` (209 §4.4): the current epoch and the live sites the local stats
    /// count. No row is opened.
    pub async fn status(&self) -> Result<(u64, u32), ProfileError> {
        let epoch = self.epoch().await?;
        let (sites, _) = self.totals().await?;
        Ok((epoch, sites as u32))
    }

    /// The rows of `site` under `ids`, live entries of epoch `epoch` or later opened; entries dead
    /// under the site's forget marker (as it applies at `now`) count as not there.
    async fn site_rows(
        &self,
        ids: &Ids,
        site: &str,
        epoch: u64,
        now: u64,
    ) -> Result<SiteRows, ProfileError> {
        let rows = sqlx::query("SELECT id, site_id, sealed FROM web_sites WHERE site_id = ?")
            .bind(ids.site(site))
            .fetch_all(&self.pool)
            .await
            .map_err(db)?;
        let mut out = SiteRows {
            forgot: self.forgot(ids, site, epoch, now).await?,
            ..SiteRows::default()
        };
        for r in rows.into_iter().map(raw) {
            let Some(sealed) = &r.sealed else {
                out.removed.insert(r.id);
                continue;
            };
            if let Opened::Entry(e) = self.open_entry(&r.id, sealed)
                && e.epoch >= epoch
                && e.site == site
                && !dead(e.updated_ms, e.basis_ms, out.forgot)
            {
                let bytes = e.item.bytes();
                out.live.insert(r.id, (*e, bytes));
            }
        }
        Ok(out)
    }

    /// Merge a push into the rows (209 §4.2) at now; see [`Self::push_at`].
    pub async fn push(&self, msg: Push) -> Result<PushResult, ProfileError> {
        self.push_at(msg, now_ms()).await
    }

    /// Merge a push made at `now` (Unix ms) into the rows (209 §4.2). Refused with
    /// [`ProfileError::StaleEpoch`] when `msg.epoch` is older than the current epoch and
    /// [`ProfileError::UnknownEpoch`] when it is newer. Per site, each change becomes a write of
    /// its own row: a set seals the cookie, a remove (or a set already expired) tombstones it, a
    /// snapshot newer than the stored one replaces it and one over
    /// [`caps::ORIGIN_STORAGE_BYTES`] tombstones it (dropped whole, never cut). A set whose
    /// `same_site` is `""` (unknown: WebKit cannot tell `None` from unspecified) keeps the stored
    /// cookie's attribute. A change that leaves the row as it is writes nothing. The caps of 209
    /// §4.1 apply per site (a site over a cap is skipped, logged by count, never by value; over
    /// the byte cap the oldest snapshots go first) and per profile; when nothing could be written
    /// because of the profile caps the push fails with [`ProfileError::Full`]. A site forgotten
    /// after the writer's `basis_ms` (no basis: any forget marker of the site) takes no set or
    /// snapshot from this push, whatever its time; it is listed in `refused`, and removes still
    /// apply. The caller checked the push ([`merge::validate`]).
    pub async fn push_at(&self, msg: Push, now: u64) -> Result<PushResult, ProfileError> {
        let ids = self.ids().0;
        let current = self.epoch().await?;
        if msg.epoch < current {
            return Err(ProfileError::StaleEpoch { current });
        }
        if msg.epoch > current {
            return Err(ProfileError::UnknownEpoch { current });
        }
        let epoch = current;
        let now_s = (now / 1000) as i64;
        let mut cookies = merge::group(msg.changes);
        let mut storage = merge::group_storage(msg.storage);
        let sites: BTreeSet<String> = cookies.keys().chain(storage.keys()).cloned().collect();
        let (mut live, mut total) = self.totals().await?;
        let mut out = PushResult {
            applied: 0,
            sites: Vec::new(),
            epoch,
            profile: ProfileMode::Shared,
            refused: Vec::new(),
        };
        let mut refused_full = false;
        for site in sites {
            let rows = self.site_rows(&ids, &site, epoch, now).await?;
            let mut changes = cookies.remove(&site).unwrap_or_default();
            let mut snaps = storage.remove(&site).unwrap_or_default();
            if dead(now, msg.basis_ms, rows.forgot) {
                let before = changes.len() + snaps.len();
                changes.retain(|c| matches!(c, CookieChange::Remove { .. }));
                snaps.clear();
                if before > changes.len() {
                    tracing::info!(
                        "webprofile: {site} was forgotten since {} read it: {} sets and snapshots dropped",
                        msg.writer,
                        before - changes.len()
                    );
                    out.refused.push(site.clone());
                }
            }
            let applied = changes.len() + snaps.len();
            // Row id → its next item (`None` = tombstone), in push order.
            let mut next: BTreeMap<String, Option<Item>> = BTreeMap::new();
            for c in changes {
                match c {
                    CookieChange::Set { mut cookie } => {
                        let id = ids.cookie(&site, &cookie.name, &cookie.domain, &cookie.path);
                        if cookie.same_site.is_empty()
                            && let Some((e, _)) = rows.live.get(&id)
                            && let Item::Cookie(stored) = &e.item
                        {
                            cookie.same_site.clone_from(&stored.same_site);
                        }
                        let item =
                            (!merge::expired(&cookie, now_s)).then_some(Item::Cookie(cookie));
                        next.insert(id, item);
                    }
                    CookieChange::Remove { name, domain, path } => {
                        next.insert(ids.cookie(&site, &name, &domain, &path), None);
                    }
                }
            }
            for s in snaps {
                let id = ids.storage(&site, &s.origin);
                let have = match next.get(&id) {
                    Some(Some(Item::Storage(o))) => Some(o.captured_ms),
                    Some(_) => None,
                    None => rows.live.get(&id).and_then(|(e, _)| match &e.item {
                        Item::Storage(o) => Some(o.captured_ms),
                        _ => None,
                    }),
                };
                if have.is_some_and(|ms| ms > s.captured_ms) {
                    continue;
                }
                let item = (s.bytes() <= caps::ORIGIN_STORAGE_BYTES).then_some(Item::Storage(s));
                next.insert(id, item);
            }
            // Only what changes the row: an unchanged set writes nothing; a remove of a row this
            // engine has not seen still tombstones it, so it wins over an older set elsewhere.
            next.retain(|id, item| match (rows.live.get(id), item) {
                (Some((e, _)), Some(i)) => e.item != *i,
                (Some(_), None) | (None, Some(_)) => true,
                (None, None) => !rows.removed.contains(id),
            });
            if next.is_empty() {
                continue;
            }
            // The site after the writes: row id → (is a cookie, snapshot capture time, bytes).
            let mut after: HashMap<String, (bool, u64, usize)> = rows
                .live
                .iter()
                .map(|(id, (e, b))| (id.clone(), shape(&e.item, *b)))
                .collect();
            for (id, item) in &next {
                match item {
                    Some(i) => after.insert(id.clone(), shape(i, i.bytes())),
                    None => after.remove(id),
                };
            }
            let n_cookies = after.values().filter(|(c, ..)| *c).count();
            if n_cookies > caps::SITE_COOKIES {
                tracing::warn!(
                    "webprofile: {site}: {n_cookies} cookies > {}; push for this site refused",
                    caps::SITE_COOKIES
                );
                continue;
            }
            let mut bytes: usize = after.values().map(|(.., b)| b).sum();
            while bytes > caps::ROW_BYTES {
                let oldest = after
                    .iter()
                    .filter(|(_, (c, ..))| !*c)
                    .min_by_key(|(_, (_, at, _))| *at)
                    .map(|(id, (.., b))| (id.clone(), *b));
                let Some((id, b)) = oldest else { break };
                after.remove(&id);
                bytes -= b;
                next.insert(id, None);
            }
            if bytes > caps::ROW_BYTES {
                tracing::warn!(
                    "webprofile: {site}: {bytes} bytes > {}; push for this site refused",
                    caps::ROW_BYTES
                );
                continue;
            }
            let old: usize = rows.live.values().map(|(_, b)| b).sum();
            let (was_live, is_live) = (!rows.live.is_empty(), !after.is_empty());
            let new_live = live + usize::from(is_live) - usize::from(was_live);
            let new_total = (total + bytes as u64).saturating_sub(old as u64);
            if (is_live && !was_live && new_live > caps::SITES)
                || (new_total > caps::PROFILE_BYTES as u64 && bytes > old)
            {
                tracing::warn!("webprofile: {site} refused: {}", crate::PROFILE_FULL);
                refused_full = true;
                continue;
            }
            let mut tx = self.pool.begin().await.map_err(db)?;
            for (id, item) in next {
                let sealed = match item {
                    Some(item) => Some(self.seal(
                        &id,
                        &Entry {
                            v: ENTRY_V,
                            site: site.clone(),
                            epoch,
                            writer: msg.writer.clone(),
                            updated_ms: now,
                            basis_ms: msg.basis_ms,
                            item,
                        },
                    )?),
                    None => None,
                };
                put(&mut tx, &id, Some(&ids.site(&site)), sealed.as_deref()).await?;
            }
            tx.commit().await.map_err(db)?;
            live = new_live;
            total = new_total;
            out.applied += applied as u32;
            out.sites.push(site);
        }
        if out.sites.is_empty() && refused_full {
            return Err(ProfileError::Full);
        }
        Ok(out)
    }

    /// Live sites, every one or those of `sites` (any host; grouped by its site), sorted by site:
    /// expired cookies dropped, session cookies dropped from a site none of whose rows was written
    /// in [`caps::SESSION_COOKIE_IDLE_MS`] (209 §11 Q1). Reading every site also moves rows a
    /// lagging engine wrote under an old mesh key's ids and tombstones rows of an old epoch or
    /// dead under a forget marker.
    pub async fn load(&self, sites: Option<&[String]>) -> Result<LoadedProfile, ProfileError> {
        let wanted = sites.map(|s| s.iter().map(|h| crate::site_of(h)).collect());
        let now = now_ms();
        let (epoch, entries) = self.live(wanted.as_ref(), now).await?;
        Ok(LoadedProfile {
            epoch,
            sites: assemble(entries, now),
        })
    }

    /// The epoch and the live entries of every site (`None`: a full read, see [`Self::scan`]) or
    /// of the registrable domains `wanted`.
    async fn live(
        &self,
        wanted: Option<&BTreeSet<String>>,
        now: u64,
    ) -> Result<(u64, Vec<Entry>), ProfileError> {
        let ids = self.ids().0;
        let epoch = self.epoch().await?;
        let entries = match wanted {
            None => self.scan(&ids, epoch).await?,
            Some(wanted) => {
                let mut v = Vec::new();
                for site in wanted {
                    let rows = self.site_rows(&ids, site, epoch, now).await?;
                    v.extend(rows.live.into_values().map(|(e, _)| e));
                }
                v
            }
        };
        Ok((epoch, entries))
    }

    /// The first page of `web_profile_pull`; see [`Self::pull_page`].
    pub async fn pull(&self, sites: Option<&[String]>) -> Result<PullResult, ProfileError> {
        self.pull_page(sites, None).await
    }

    /// One page of `web_profile_pull` (209 §4.4, D5): the live cookies and snapshots of `sites`
    /// (any hosts, grouped by registrable domain) or of every site the profile holds, from the
    /// site `cursor` on, each with its site and its row's writer and time; the same rules as
    /// [`Self::load`]. Sites go whole and in order until the next would carry the page past
    /// [`caps::PULL_BYTES`] of cookie and snapshot JSON (a page has at least one site); `next`
    /// names where the following page starts. Every page also lists the forget markers that
    /// apply (`forgotten`) and `now_ms`, read before any row: the client's `basis_ms`. A site
    /// asked for and not answered has nothing (forgotten, cleared, never signed in); a full pull
    /// names every site the profile holds, also one whose cookies all expired. The only read whose
    /// values leave the engine; it logs nothing.
    pub async fn pull_page(
        &self,
        sites: Option<&[String]>,
        cursor: Option<&str>,
    ) -> Result<PullResult, ProfileError> {
        let now = now_ms();
        let wanted: Option<BTreeSet<String>> = sites.map(|s| {
            s.iter()
                .map(|h| crate::site_of(h))
                .filter(|s| !s.is_empty())
                .collect()
        });
        let ids = self.ids().0;
        let epoch = self.epoch().await?;
        let mut page = Page {
            out: PullResult {
                epoch,
                forgotten: self.forgotten(epoch, now).await?,
                now_ms: now,
                ..PullResult::default()
            },
            bytes: 0,
        };
        let from = |site: &str| cursor.is_none_or(|c| site >= c);
        match wanted {
            None => {
                let mut by_site: BTreeMap<String, Vec<Entry>> = BTreeMap::new();
                for e in self.scan(&ids, epoch).await? {
                    by_site.entry(e.site.clone()).or_default().push(e);
                }
                for (site, entries) in by_site.into_iter().filter(|(s, _)| from(s)) {
                    if !page.add(site, entries, now) {
                        break;
                    }
                }
            }
            Some(wanted) => {
                for site in wanted.into_iter().filter(|s| from(s)) {
                    let rows = self.site_rows(&ids, &site, epoch, now).await?;
                    let entries = rows.live.into_values().map(|(e, _)| e).collect();
                    if !page.add(site, entries, now) {
                        break;
                    }
                }
            }
        }
        Ok(page.out)
    }

    /// The Settings list (209 §4.4 `web_profile_sites`): counts, never values.
    pub async fn sites(&self) -> Result<Vec<SiteSummary>, ProfileError> {
        Ok(self
            .load(None)
            .await?
            .sites
            .into_iter()
            .map(|s| SiteSummary {
                cookies: s.jar.cookies.len() as u32,
                origins: s
                    .jar
                    .origins
                    .iter()
                    .filter(|o| !o.local_storage.is_empty())
                    .count() as u32,
                site: s.jar.site,
                updated_ms: s.updated_ms,
                writer: s.writer,
            })
            .collect())
    }

    /// The sites a peer's batch changed (row ids `ids`; `None` = unknown, every live site) and the
    /// epoch, for [`crate::WebProfileChanged`]; `None` when no site and not the epoch changed. A
    /// tombstone names its site through any live row of the same `site_id`.
    pub async fn changed(
        &self,
        ids: Option<&[String]>,
    ) -> Result<Option<(Vec<String>, u64)>, ProfileError> {
        let Some(ids) = ids else {
            let p = self.load(None).await?;
            return Ok(Some((
                p.sites.into_iter().map(|s| s.jar.site).collect(),
                p.epoch,
            )));
        };
        let epoch = self.epoch().await?;
        let mut sites = BTreeSet::new();
        let mut epoch_changed = false;
        for id in ids {
            if id == EPOCH_ID {
                epoch_changed = true;
                continue;
            }
            let row = sqlx::query("SELECT id, site_id, sealed FROM web_sites WHERE id = ?")
                .bind(id)
                .fetch_optional(&self.pool)
                .await
                .map_err(db)?
                .map(raw);
            let Some(row) = row else { continue };
            let found = match &row.sealed {
                Some(s) => match self.open_entry(&row.id, s) {
                    Opened::Entry(e) => Some(e.site),
                    _ => None,
                },
                None => self.site_by_id(row.site_id.as_deref()).await?,
            };
            sites.extend(found);
        }
        Ok((!sites.is_empty() || epoch_changed).then(|| (sites.into_iter().collect(), epoch)))
    }

    /// The site of any live row of `site_id`.
    async fn site_by_id(&self, site_id: Option<&str>) -> Result<Option<String>, ProfileError> {
        let Some(site_id) = site_id else {
            return Ok(None);
        };
        let rows = sqlx::query(
            "SELECT id, site_id, sealed FROM web_sites WHERE site_id = ? AND sealed IS NOT NULL",
        )
        .bind(site_id)
        .fetch_all(&self.pool)
        .await
        .map_err(db)?;
        Ok(rows.into_iter().map(raw).find_map(|r| {
            match self.open_entry(&r.id, r.sealed.as_deref()?) {
                Opened::Entry(e) => Some(e.site),
                _ => None,
            }
        }))
    }

    /// Every live entry, deduplicated per row identity (the newest write wins). Rows found under
    /// an id other than the current mesh key's (a rotation, or a lagging engine) move to it —
    /// written there unless the row there now holds a write at least as new, or a newer tombstone
    /// — and their old id is tombstoned; rows of an older epoch and rows dead under their site's
    /// forget marker (`dead`) are tombstoned. Each tombstone is written only while the row still
    /// holds what was read, so a write that landed meanwhile (a push, a peer's row) stays. Forget
    /// markers move like any row and are not returned. Rows of an unknown format version are
    /// skipped and counted.
    async fn scan(&self, ids: &Ids, epoch: u64) -> Result<Vec<Entry>, ProfileError> {
        let now = now_ms();
        let rows = sqlx::query(
            "SELECT id, site_id, sealed FROM web_sites WHERE id != ? AND sealed IS NOT NULL",
        )
        .bind(EPOCH_ID)
        .fetch_all(&self.pool)
        .await
        .map_err(db)?;
        let mut unknown = 0usize;
        let mut opened: Vec<(String, String, Entry)> = Vec::with_capacity(rows.len());
        for r in rows.into_iter().map(raw) {
            let Some(sealed) = r.sealed else { continue };
            match self.open_entry(&r.id, &sealed) {
                Opened::Entry(e) => opened.push((r.id, sealed, *e)),
                Opened::Unknown => unknown += 1,
                Opened::Unreadable => {}
            }
        }
        // Site → when it was last forgotten in this epoch (markers that still apply).
        let mut forgot: HashMap<String, u64> = HashMap::new();
        for (_, _, e) in &opened {
            if matches!(e.item, Item::Forget {})
                && e.epoch >= epoch
                && marker_live(e.updated_ms, now)
            {
                let at = forgot.entry(e.site.clone()).or_default();
                *at = (*at).max(e.updated_ms);
            }
        }
        // Identity (current id) → the newest entry and whether it already sits at that id.
        let mut best: HashMap<String, (Entry, bool)> = HashMap::new();
        // Rows to tombstone: id and the sealed text read (the write is conditional on it).
        let mut gone: Vec<(String, String)> = Vec::new();
        let mut targets: BTreeSet<String> = BTreeSet::new();
        for (id, sealed, e) in opened {
            let marker = matches!(e.item, Item::Forget {});
            if e.epoch < epoch
                || (!marker && dead(e.updated_ms, e.basis_ms, forgot.get(&e.site).copied()))
            {
                gone.push((id, sealed));
                continue;
            }
            let canonical = ids.of_entry(&e);
            let home = canonical == id;
            if !home {
                gone.push((id, sealed));
                targets.insert(canonical.clone());
            }
            let newer = best.get(&canonical).is_none_or(|(b, b_home)| {
                e.updated_ms > b.updated_ms || (e.updated_ms == b.updated_ms && home && !b_home)
            });
            if newer {
                best.insert(canonical, (e, home));
            }
        }
        if unknown > 0 {
            tracing::warn!("webprofile: {unknown} rows of an unknown format version skipped");
        }
        if !gone.is_empty() {
            tracing::info!(
                rows = gone.len(),
                "webprofile: rows moved to the current key's ids, of an old epoch or forgotten"
            );
            let gone_ids: HashSet<String> = gone.iter().map(|(id, _)| id.clone()).collect();
            let mut tx = self.pool.begin_with("BEGIN IMMEDIATE").await.map_err(db)?;
            for (id, sealed) in &gone {
                sqlx::query(
                    "UPDATE web_sites SET sealed = NULL, hlc = NULL WHERE id = ? AND sealed = ?",
                )
                .bind(id)
                .bind(sealed)
                .execute(&mut *tx)
                .await
                .map_err(db)?;
            }
            for id in targets {
                let Some((e, false)) = best.get(&id).map(|(e, h)| (e.clone(), *h)) else {
                    continue;
                };
                // What the current id holds now, inside this transaction: a newer tombstone keeps
                // the removal; a live write at least as new (a push, a peer's row that landed
                // after the read above) is kept and answered instead.
                let stamp: Option<(Option<String>, Option<String>)> =
                    sqlx::query_as("SELECT sealed, hlc FROM web_sites WHERE id = ?")
                        .bind(&id)
                        .fetch_optional(&mut *tx)
                        .await
                        .map_err(db)?;
                match stamp {
                    Some((None, hlc)) if !gone_ids.contains(&id) => {
                        let at = hlc
                            .as_deref()
                            .and_then(replica::Hlc::parse)
                            .map_or(u64::MAX, |h| h.wall_ms);
                        if at >= e.updated_ms {
                            best.remove(&id);
                            continue;
                        }
                    }
                    Some((Some(there), _)) => {
                        if let Opened::Entry(t) = self.open_entry(&id, &there)
                            && t.updated_ms >= e.updated_ms
                        {
                            if t.epoch >= epoch {
                                best.insert(id, (*t, true));
                            } else {
                                best.remove(&id);
                            }
                            continue;
                        }
                    }
                    _ => {}
                }
                let sealed = self.seal(&id, &e)?;
                let site_id = (!matches!(e.item, Item::Forget {})).then(|| ids.site(&e.site));
                put(&mut tx, &id, site_id.as_deref(), Some(&sealed)).await?;
            }
            for q in RESTAT {
                sqlx::query(q).execute(&mut *tx).await.map_err(db)?;
            }
            tx.commit().await.map_err(db)?;
        }
        Ok(best
            .into_values()
            .map(|(e, _)| e)
            .filter(|e| !matches!(e.item, Item::Forget {}))
            .collect())
    }

    /// Move every row to the current mesh key's ids under the retire hold: take the hold, move
    /// ([`Self::scan`]), re-seal in place every row that still opens only under a retained key
    /// (the epoch row, rows of an unknown format version, a write that landed meanwhile), then
    /// count the rows still under a retained key — a check taken under the hold, after the move —
    /// and release the hold only when there are none, so a peer's row that arrived during the move
    /// keeps the old key alive until the next pass. On an error the hold stays and
    /// [`Self::maintain`] retries.
    async fn fold(&self) -> Result<(), ProfileError> {
        let _one = self.folding.lock().await;
        self.owed.store(false, Ordering::SeqCst);
        self.hold();
        let result = async {
            let (ids, _) = self.ids();
            let epoch = self.epoch().await?;
            self.scan(&ids, epoch).await?;
            self.reseal().await?;
            self.under_retained_keys().await
        }
        .await;
        match result {
            Ok(0) if !self.fold_owed() => {
                *self.hold.lock().unwrap_or_else(PoisonError::into_inner) = None;
                Ok(())
            }
            Ok(left) => {
                if left > 0 {
                    tracing::info!(
                        rows = left,
                        "webprofile: rows still under a retained mesh key; holding it"
                    );
                }
                Ok(())
            }
            Err(e) => {
                self.owed.store(true, Ordering::SeqCst);
                Err(e)
            }
        }
    }

    /// Re-seal under the current mesh key every row that opens only under a retained one; each
    /// write only while the row still holds what was read.
    async fn reseal(&self) -> Result<(), ProfileError> {
        let rows: Vec<(String, String)> =
            sqlx::query_as("SELECT id, sealed FROM web_sites WHERE sealed IS NOT NULL")
                .fetch_all(&self.pool)
                .await
                .map_err(db)?;
        let resealed: Vec<(String, String, String)> = rows
            .into_iter()
            .filter_map(|(id, sealed)| {
                let bytes = URL_SAFE_NO_PAD.decode(&sealed).ok()?;
                let r = self.vault.reseal_with_mesh_key(&aad(&id), &bytes)?;
                Some((id, sealed, URL_SAFE_NO_PAD.encode(r)))
            })
            .collect();
        if resealed.is_empty() {
            return Ok(());
        }
        let mut tx = self.pool.begin_with("BEGIN IMMEDIATE").await.map_err(db)?;
        for (id, old, new) in &resealed {
            sqlx::query("UPDATE web_sites SET sealed = ?, hlc = NULL WHERE id = ? AND sealed = ?")
                .bind(new)
                .bind(id)
                .bind(old)
                .execute(&mut *tx)
                .await
                .map_err(db)?;
        }
        tx.commit().await.map_err(db)
    }

    /// Rows that open only under a retained mesh key (not yet moved to the current one).
    pub async fn under_retained_keys(&self) -> Result<usize, ProfileError> {
        let rows: Vec<(String, String)> =
            sqlx::query_as("SELECT id, sealed FROM web_sites WHERE sealed IS NOT NULL")
                .fetch_all(&self.pool)
                .await
                .map_err(db)?;
        Ok(rows
            .into_iter()
            .filter(|(id, sealed)| {
                URL_SAFE_NO_PAD
                    .decode(sealed)
                    .ok()
                    .is_some_and(|b| self.vault.reseal_with_mesh_key(&aad(id), &b).is_some())
            })
            .count())
    }

    /// A peer's batch changed rows `ids` (`None` = unknown): when any opens only under a retained
    /// key — a lagging engine still writing under the key this one moved off — hold that key and
    /// move them now, before the vault can retire it (209 §4.1 rotation).
    pub async fn rekey_replicated(&self, ids: Option<&[String]>) -> Result<(), ProfileError> {
        let stale = match ids {
            None => true,
            Some(ids) => {
                let mut stale = false;
                for id in ids {
                    let sealed: Option<Option<String>> =
                        sqlx::query_scalar("SELECT sealed FROM web_sites WHERE id = ?")
                            .bind(id)
                            .fetch_optional(&self.pool)
                            .await
                            .map_err(db)?;
                    if let Some(b) = sealed
                        .flatten()
                        .and_then(|s| URL_SAFE_NO_PAD.decode(s).ok())
                        && self.vault.reseal_with_mesh_key(&aad(id), &b).is_some()
                    {
                        stale = true;
                        break;
                    }
                }
                stale
            }
        };
        if stale {
            self.hold();
            tracing::info!("webprofile: a peer wrote rows under a retained mesh key; moving them");
            self.fold().await?;
        }
        Ok(())
    }

    /// Delete tombstones written more than [`caps::TOMBSTONE_MS`] before `now`: by their `hlc`
    /// when the replica stamped one, else by the local time a trigger recorded when the tombstone
    /// was written here (a profile with no replication attached). Run at open and by
    /// [`Self::maintain`].
    pub async fn purge(&self, now: u64) -> Result<(), ProfileError> {
        let cutoff = now.saturating_sub(caps::TOMBSTONE_MS);
        sqlx::query(
            "DELETE FROM web_sites
             WHERE sealed IS NULL AND id != ?
               AND ((hlc IS NOT NULL AND hlc < ?)
                    OR id IN (SELECT id FROM web_sites_removed WHERE removed_ms < ?))",
        )
        .bind(EPOCH_ID)
        .bind(format!("{cutoff:016x}"))
        .bind(cutoff as i64)
        .execute(&self.pool)
        .await
        .map_err(db)?;
        Ok(())
    }
}

/// One page of a pull being filled.
struct Page {
    out: PullResult,
    /// Cookie and snapshot JSON so far.
    bytes: usize,
}

impl Page {
    /// Add `site` whole (its live `entries`, by the rules of [`assemble`]), unless the page already
    /// holds a site and this one would carry it past [`caps::PULL_BYTES`]: then `next` is `site`
    /// and `false` says the page is full.
    fn add(&mut self, site: String, entries: Vec<Entry>, now: u64) -> bool {
        let (cookies, storage) = site_answer(&site, entries, now);
        let size: usize = cookies
            .iter()
            .map(|c| serde_json::to_vec(c).map_or(0, |v| v.len()))
            .chain(
                storage
                    .iter()
                    .map(|s| serde_json::to_vec(s).map_or(0, |v| v.len())),
            )
            .sum();
        if !self.out.sites.is_empty() && self.bytes + size > caps::PULL_BYTES {
            self.out.next = Some(site);
            return false;
        }
        self.bytes += size;
        self.out.sites.push(site);
        self.out.cookies.extend(cookies);
        self.out.storage.extend(storage);
        true
    }
}

/// `(is a cookie, snapshot capture time, bytes)` of an item.
fn shape(item: &Item, bytes: usize) -> (bool, u64, usize) {
    match item {
        Item::Cookie(_) => (true, 0, bytes),
        Item::Storage(s) => (false, s.captured_ms, bytes),
        Item::Epoch {} | Item::Forget {} => (false, 0, 0),
    }
}

/// Group live entries into sites, sorted: expired cookies dropped, session cookies of a site idle
/// past [`caps::SESSION_COOKIE_IDLE_MS`] dropped, cookies and origins sorted; the site's
/// `updated_ms` and `writer` are those of its newest row.
fn assemble(entries: Vec<Entry>, now: u64) -> Vec<LoadedSite> {
    let mut by_site: BTreeMap<String, Vec<Entry>> = BTreeMap::new();
    for e in entries {
        by_site.entry(e.site.clone()).or_default().push(e);
    }
    let now_s = (now / 1000) as i64;
    by_site
        .into_iter()
        .filter_map(|(site, entries)| {
            let newest = entries.iter().max_by_key(|e| e.updated_ms)?;
            let (updated_ms, writer) = (newest.updated_ms, newest.writer.clone());
            let idle = now.saturating_sub(updated_ms) > caps::SESSION_COOKIE_IDLE_MS;
            let mut jar = SiteJar::new(site);
            for e in entries {
                match e.item {
                    Item::Cookie(c) => {
                        if !merge::expired(&c, now_s) && !(idle && c.expires.is_none()) {
                            jar.cookies.push(c);
                        }
                    }
                    Item::Storage(s) => jar.origins.push(s),
                    Item::Epoch {} | Item::Forget {} => {}
                }
            }
            if jar.cookies.is_empty() && jar.origins.is_empty() {
                return None;
            }
            jar.cookies.sort_by_key(merge::norm_cookie);
            jar.origins.sort_by(|a, b| a.origin.cmp(&b.origin));
            Some(LoadedSite {
                jar,
                updated_ms,
                writer,
            })
        })
        .collect()
}

/// One site of `web_profile_pull`, by the rules of [`assemble`]: each cookie and snapshot with the
/// site and its own row's writer and time, cookies sorted by name, domain and path, snapshots by
/// origin.
fn site_answer(
    site: &str,
    entries: Vec<Entry>,
    now: u64,
) -> (Vec<PulledCookie>, Vec<PulledStorage>) {
    let now_s = (now / 1000) as i64;
    let newest = entries.iter().map(|e| e.updated_ms).max().unwrap_or(0);
    let idle = now.saturating_sub(newest) > caps::SESSION_COOKIE_IDLE_MS;
    let (mut cookies, mut storage) = (Vec::new(), Vec::new());
    for e in entries {
        match e.item {
            Item::Cookie(c) => {
                if !merge::expired(&c, now_s) && !(idle && c.expires.is_none()) {
                    cookies.push(PulledCookie {
                        cookie: c,
                        site: site.to_owned(),
                        writer: e.writer,
                        updated_ms: e.updated_ms,
                    });
                }
            }
            Item::Storage(s) => storage.push(PulledStorage {
                storage: s,
                site: site.to_owned(),
                writer: e.writer,
                updated_ms: e.updated_ms,
            }),
            Item::Epoch {} | Item::Forget {} => {}
        }
    }
    cookies.sort_by_key(|c| merge::norm_cookie(&c.cookie));
    storage.sort_by(|a, b| a.storage.origin.cmp(&b.storage.origin));
    (cookies, storage)
}

/// Write one row: `sealed = None` is a tombstone; `hlc` NULL for the replica to stamp.
async fn put(
    tx: &mut sqlx::SqliteConnection,
    id: &str,
    site_id: Option<&str>,
    sealed: Option<&str>,
) -> Result<(), ProfileError> {
    sqlx::query(
        "INSERT INTO web_sites (id, site_id, sealed, hlc) VALUES (?, ?, ?, NULL)
         ON CONFLICT(id) DO UPDATE SET site_id = excluded.site_id, sealed = excluded.sealed,
           hlc = NULL",
    )
    .bind(id)
    .bind(site_id)
    .bind(sealed)
    .execute(tx)
    .await
    .map_err(db)?;
    Ok(())
}

fn db(e: sqlx::Error) -> ProfileError {
    ProfileError::Unavailable(format!("webprofile.sqlite: {e}"))
}
