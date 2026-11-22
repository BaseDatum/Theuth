//! `<data_dir>/webprofile.sqlite` (209 §4.1): the replicated table `web_sites`, one row per site,
//! the jar sealed with the vault's mesh key (`aad = "web_sites\n" ‖ id`), the row id a salted
//! hash of the site so the file at rest does not name the sites. The epoch row (`id = "epoch"`)
//! carries the profile epoch; rows written under an older epoch are dead. Forget is a column write
//! (`sealed = NULL`), never a `DELETE`. Writes are serialised by the actor; `hlc` is left NULL for
//! the replica to stamp at capture.

use std::collections::BTreeSet;
use std::path::Path;
use std::sync::Arc;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use anyhow::Context as _;
use base64::Engine as _;
use base64::engine::general_purpose::URL_SAFE_NO_PAD;
use sha2::{Digest as _, Sha256};
use sqlx::Row as _;
use sqlx::sqlite::{SqliteConnectOptions, SqliteJournalMode, SqlitePool, SqlitePoolOptions};
use zeroize::Zeroizing;

use crate::{
    LoadedProfile, LoadedSite, ProfileError, Push, PushResult, SiteJar, SiteSummary, caps, merge,
};

/// Sqlite file under `data_dir`.
pub const DB: &str = "webprofile.sqlite";
/// The tables `replica::attach` takes.
pub const TABLES: &[&str] = &["web_sites"];
/// Id of the epoch row.
pub const EPOCH_ID: &str = "epoch";
/// Vault secret holding the row-id salt (32 random bytes, base64url). The contract names it
/// `webprofile/salt`; the vault only accepts `/` names in its hidden families (`oauth/`, `mesh/`,
/// `sandbox/`), so it lives in the mesh family, which is also never listed nor expandable.
pub const SALT_SECRET: &str = "mesh/webprofile-salt";
/// `aad` prefix of a sealed jar.
const AAD: &[u8] = b"web_sites\n";

/// Unix ms now.
pub fn now_ms() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |d| d.as_millis() as u64)
}

/// The row id of `site`: the first 32 hex of `sha256(salt ‖ "\n" ‖ site)`.
pub fn row_id(salt: &[u8; 32], site: &str) -> String {
    let mut h = Sha256::new();
    h.update(salt);
    h.update(b"\n");
    h.update(site.as_bytes());
    let mut s = hex::encode(h.finalize());
    s.truncate(32);
    s
}

fn aad(id: &str) -> Vec<u8> {
    [AAD, id.as_bytes()].concat()
}

/// One stored row (the jar still sealed).
#[derive(Clone, Debug)]
struct Row {
    id: String,
    sealed: Option<String>,
    epoch: u64,
    bytes: u64,
    updated_ms: u64,
    writer: String,
}

/// The profile store: pool, vault and salt. The actor owns one.
pub struct Profile {
    pool: SqlitePool,
    vault: Arc<secrets::Vault>,
    salt: Zeroizing<[u8; 32]>,
}

impl Profile {
    /// Open or create the file, run the migration, read or create the salt.
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
        let salt = salt(&vault).await?;
        Ok(Self { pool, vault, salt })
    }

    /// The underlying pool (`replica::attach` takes it).
    pub fn pool(&self) -> &SqlitePool {
        &self.pool
    }

    /// The row id of `site` under this profile's salt.
    pub fn id_of(&self, site: &str) -> String {
        row_id(&self.salt, site)
    }

    /// The current profile epoch (`0` before any sign out everywhere).
    pub async fn epoch(&self) -> Result<u64, ProfileError> {
        let row = sqlx::query("SELECT epoch FROM web_sites WHERE id = ?")
            .bind(EPOCH_ID)
            .fetch_optional(&self.pool)
            .await
            .map_err(db)?;
        Ok(row.map_or(0, |r| r.get::<i64, _>(0).max(0) as u64))
    }

    /// Set the epoch row to `epoch` (sign out everywhere writes `n + 1`, 209 §4.1).
    pub async fn set_epoch(&self, epoch: u64) -> Result<(), ProfileError> {
        sqlx::query(
            "INSERT INTO web_sites (id, sealed, epoch, bytes, updated_ms, writer, hlc)
             VALUES (?, NULL, ?, NULL, ?, NULL, NULL)
             ON CONFLICT(id) DO UPDATE SET sealed = NULL, epoch = excluded.epoch,
               updated_ms = excluded.updated_ms, hlc = NULL",
        )
        .bind(EPOCH_ID)
        .bind(epoch as i64)
        .bind(now_ms() as i64)
        .execute(&self.pool)
        .await
        .map_err(db)?;
        Ok(())
    }

    async fn rows(&self) -> Result<Vec<Row>, ProfileError> {
        let rows = sqlx::query(
            "SELECT id, sealed, epoch, bytes, updated_ms, writer FROM web_sites WHERE id != ?",
        )
        .bind(EPOCH_ID)
        .fetch_all(&self.pool)
        .await
        .map_err(db)?;
        Ok(rows.into_iter().map(row).collect())
    }

    async fn row(&self, id: &str) -> Result<Option<Row>, ProfileError> {
        let r = sqlx::query(
            "SELECT id, sealed, epoch, bytes, updated_ms, writer FROM web_sites WHERE id = ?",
        )
        .bind(id)
        .fetch_optional(&self.pool)
        .await
        .map_err(db)?;
        Ok(r.map(row))
    }

    /// The jar of a live row (`sealed` set, written under `epoch` or later); `None` when dead or
    /// when it does not open (another mesh's key, a wrong aad, damaged bytes).
    fn open_row(&self, r: &Row, epoch: u64) -> Option<SiteJar> {
        if r.epoch < epoch {
            return None;
        }
        let bytes = URL_SAFE_NO_PAD.decode(r.sealed.as_deref()?).ok()?;
        let plain = self.vault.open_with_mesh_key(&aad(&r.id), &bytes)?;
        match serde_json::from_slice::<SiteJar>(&plain) {
            Ok(jar) => Some(jar),
            Err(e) => {
                tracing::warn!("webprofile: row {} does not parse: {e}", r.id);
                None
            }
        }
    }

    /// Seal `jar` for row `id`: base64url(nonce ‖ XChaCha20-Poly1305(mesh key, aad, JSON)).
    pub fn seal(&self, id: &str, jar: &SiteJar) -> (String, usize) {
        let plain = Zeroizing::new(serde_json::to_vec(jar).unwrap_or_default());
        let sealed = self.vault.seal_with_mesh_key(&aad(id), &plain);
        (URL_SAFE_NO_PAD.encode(sealed), plain.len())
    }

    /// Open a sealed jar of row `id` (the inverse of [`Profile::seal`]).
    pub fn unseal(&self, id: &str, sealed: &str) -> Option<SiteJar> {
        let bytes = URL_SAFE_NO_PAD.decode(sealed).ok()?;
        let plain = self.vault.open_with_mesh_key(&aad(id), &bytes)?;
        serde_json::from_slice(&plain).ok()
    }

    async fn write(
        &self,
        id: &str,
        sealed: Option<&str>,
        epoch: u64,
        bytes: usize,
        writer: &str,
    ) -> Result<u64, ProfileError> {
        let at = now_ms();
        sqlx::query(
            "INSERT INTO web_sites (id, sealed, epoch, bytes, updated_ms, writer, hlc)
             VALUES (?, ?, ?, ?, ?, ?, NULL)
             ON CONFLICT(id) DO UPDATE SET sealed = excluded.sealed, epoch = excluded.epoch,
               bytes = excluded.bytes, updated_ms = excluded.updated_ms, writer = excluded.writer,
               hlc = NULL",
        )
        .bind(id)
        .bind(sealed)
        .bind(epoch as i64)
        .bind(bytes as i64)
        .bind(at as i64)
        .bind(writer)
        .execute(&self.pool)
        .await
        .map_err(db)?;
        Ok(at)
    }

    /// Merge a push into the rows (209 §4.2): refused with [`ProfileError::StaleEpoch`] when
    /// `msg.epoch` is older than the current epoch; per site, the changes and snapshots are applied
    /// to the current row under the caps of 209 §4.1. A site over a cap is skipped (logged by
    /// count, never by value); when nothing could be written because of the profile caps the push
    /// fails with [`ProfileError::Full`]. A site left with no cookie and no snapshot is written as
    /// forgotten (`sealed = NULL`).
    pub async fn push(&self, msg: Push) -> Result<PushResult, ProfileError> {
        let current = self.epoch().await?;
        if msg.epoch < current {
            return Err(ProfileError::StaleEpoch { current });
        }
        let epoch = msg.epoch.max(current);
        let now = now_ms();
        let now_s = (now / 1000) as i64;
        let mut cookies = merge::group(msg.changes);
        let mut storage = merge::group_storage(msg.storage);
        let sites: BTreeSet<String> = cookies.keys().chain(storage.keys()).cloned().collect();
        // Live totals for the profile caps.
        let all = self.rows().await?;
        let mut live = 0usize;
        let mut total = 0usize;
        for r in &all {
            if r.sealed.is_some() && r.epoch >= epoch {
                live += 1;
                total += r.bytes as usize;
            }
        }
        let mut out = PushResult {
            applied: 0,
            sites: Vec::new(),
            epoch,
        };
        let mut refused_full = false;
        for site in sites {
            let id = self.id_of(&site);
            let row = self.row(&id).await?;
            let (mut jar, old_bytes, was_live) = match &row {
                Some(r) => match self.open_row(r, epoch) {
                    Some(mut jar) => {
                        let idle = now.saturating_sub(r.updated_ms) > caps::SESSION_COOKIE_IDLE_MS;
                        merge::prune(&mut jar, now_s, idle);
                        (jar, r.bytes as usize, true)
                    }
                    None => (SiteJar::new(&site), 0, false),
                },
                None => (SiteJar::new(&site), 0, false),
            };
            jar.site.clone_from(&site);
            let before = jar.clone();
            let changes = cookies.remove(&site).unwrap_or_default();
            let mut applied = merge::apply(&mut jar, &changes, now_s);
            for s in storage.remove(&site).unwrap_or_default() {
                merge::apply_storage(&mut jar, &s);
                applied += 1;
            }
            if jar == before {
                continue;
            }
            if jar.cookies.len() > caps::SITE_COOKIES {
                tracing::warn!(
                    "webprofile: {site}: {} cookies > {}; push for this site refused",
                    jar.cookies.len(),
                    caps::SITE_COOKIES
                );
                continue;
            }
            if jar.cookies.is_empty() && jar.origins.is_empty() {
                if was_live {
                    self.write(&id, None, epoch, 0, &msg.writer).await?;
                    live -= 1;
                    total -= old_bytes;
                    out.applied += applied as u32;
                    out.sites.push(site);
                }
                continue;
            }
            let mut plain = serde_json::to_vec(&jar)
                .map_err(|e| ProfileError::Unavailable(format!("serialise {site}: {e}")))?;
            // Over the row cap: drop the oldest snapshots whole until it fits.
            while plain.len() > caps::ROW_BYTES && !jar.origins.is_empty() {
                let oldest = jar
                    .origins
                    .iter()
                    .enumerate()
                    .min_by_key(|(_, o)| o.captured_ms)
                    .map(|(i, _)| i)
                    .unwrap_or(0);
                jar.origins.remove(oldest);
                plain = serde_json::to_vec(&jar).unwrap_or_default();
            }
            let plain = Zeroizing::new(plain);
            if plain.len() > caps::ROW_BYTES {
                tracing::warn!(
                    "webprofile: {site}: {} bytes > {}; push for this site refused",
                    plain.len(),
                    caps::ROW_BYTES
                );
                continue;
            }
            let new_live = live + usize::from(!was_live);
            let new_total = total - old_bytes + plain.len();
            if new_live > caps::SITES || new_total > caps::PROFILE_BYTES {
                tracing::warn!("webprofile: {site} refused: {}", crate::PROFILE_FULL);
                refused_full = true;
                continue;
            }
            let sealed = URL_SAFE_NO_PAD.encode(self.vault.seal_with_mesh_key(&aad(&id), &plain));
            self.write(&id, Some(&sealed), epoch, plain.len(), &msg.writer)
                .await?;
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

    /// Live rows, every one or those of `sites`, sorted by site: expired cookies dropped, session
    /// cookies dropped from a row idle past [`caps::SESSION_COOKIE_IDLE_MS`] (209 §11 Q1).
    pub async fn load(&self, sites: Option<&[String]>) -> Result<LoadedProfile, ProfileError> {
        let epoch = self.epoch().await?;
        let rows = match sites {
            None => self.rows().await?,
            Some(sites) => {
                let mut v = Vec::new();
                for s in sites {
                    if let Some(r) = self.row(&self.id_of(&crate::site_of(s))).await? {
                        v.push(r);
                    }
                }
                v
            }
        };
        let now = now_ms();
        let now_s = (now / 1000) as i64;
        let mut out: Vec<LoadedSite> = rows
            .iter()
            .filter_map(|r| {
                let mut jar = self.open_row(r, epoch)?;
                let idle = now.saturating_sub(r.updated_ms) > caps::SESSION_COOKIE_IDLE_MS;
                merge::prune(&mut jar, now_s, idle);
                Some(LoadedSite {
                    jar,
                    updated_ms: r.updated_ms,
                    writer: r.writer.clone(),
                })
            })
            .collect();
        out.sort_by(|a, b| a.jar.site.cmp(&b.jar.site));
        Ok(LoadedProfile { epoch, sites: out })
    }

    /// The Settings list (209 §4.4 `web_profile_sites`): counts, never values.
    pub async fn sites(&self) -> Result<Vec<SiteSummary>, ProfileError> {
        Ok(self
            .load(None)
            .await?
            .sites
            .into_iter()
            .map(|s| SiteSummary {
                site: s.jar.site,
                cookies: s.jar.cookies.len() as u32,
                origins: s.jar.origins.len() as u32,
                updated_ms: s.updated_ms,
                writer: s.writer,
            })
            .collect())
    }
}

fn row(r: sqlx::sqlite::SqliteRow) -> Row {
    Row {
        id: r.get("id"),
        sealed: r.get("sealed"),
        epoch: r.get::<i64, _>("epoch").max(0) as u64,
        bytes: r.get::<Option<i64>, _>("bytes").unwrap_or(0).max(0) as u64,
        updated_ms: r.get::<Option<i64>, _>("updated_ms").unwrap_or(0).max(0) as u64,
        writer: r.get::<Option<String>, _>("writer").unwrap_or_default(),
    }
}

fn db(e: sqlx::Error) -> ProfileError {
    ProfileError::Unavailable(format!("webprofile.sqlite: {e}"))
}

/// The row-id salt: read [`SALT_SECRET`], or create it with the lost-the-race pattern of
/// `mesh/device_key` (`Vault::mesh_device_key`) so every engine of the mesh uses the winner's.
pub async fn salt(vault: &secrets::Vault) -> anyhow::Result<Zeroizing<[u8; 32]>> {
    fn decode(v: &str) -> anyhow::Result<Zeroizing<[u8; 32]>> {
        let bytes = Zeroizing::new(URL_SAFE_NO_PAD.decode(v.trim())?);
        let arr: [u8; 32] = bytes
            .as_slice()
            .try_into()
            .map_err(|_| anyhow::anyhow!("{SALT_SECRET}: not 32 bytes"))?;
        Ok(Zeroizing::new(arr))
    }
    if let Some(v) = vault.get(SALT_SECRET, 0) {
        return decode(&v);
    }
    let fresh: Zeroizing<[u8; 32]> = Zeroizing::new(rand::random());
    let encoded = Zeroizing::new(URL_SAFE_NO_PAD.encode(*fresh));
    match vault
        .set_if_version(SALT_SECRET, &encoded, "webprofile", None)
        .await
    {
        Ok(Some(_)) => Ok(fresh),
        Ok(None) => {
            vault.refresh().await?;
            let v = vault
                .get(SALT_SECRET, 0)
                .ok_or_else(|| anyhow::anyhow!("{SALT_SECRET}: lost the race but no row"))?;
            decode(&v)
        }
        Err(e) => Err(anyhow::anyhow!("{SALT_SECRET}: {e}")),
    }
}
