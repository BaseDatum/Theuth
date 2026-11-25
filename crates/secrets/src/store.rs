//! `<data_dir>/secrets.sqlite`: the `vault` / `vault_current` rows the mesh
//! replicates (67 §5.4) and the local-only `meta` table. Row bytes are
//! opaque here; [`crate::vault`] seals and opens them.

use std::collections::HashMap;
use std::path::Path;
use std::time::Duration;

use anyhow::Context as _;
use sqlx::Row as _;
use sqlx::sqlite::{SqliteConnectOptions, SqliteJournalMode, SqlitePool, SqlitePoolOptions};

/// Sqlite file under `data_dir`.
pub const DB: &str = "secrets.sqlite";

/// The replicated tables, for `replica::attach`.
pub const TABLES: &[&str] = &["vault", "vault_current", "secret_requests"];

/// One `vault` row as stored.
pub struct Row {
    /// Secret name (`oauth/<ns>` for grants).
    pub name: String,
    /// Per-name monotonic version.
    pub version: u32,
    /// `nonce ‖ XChaCha20-Poly1305(mesh key, aad = id)` of the value.
    pub ciphertext: Vec<u8>,
    /// Per-name note as of this version.
    pub note: String,
    /// Who wrote it (`user`, `cli`, `restore v3`, …).
    pub source: String,
    /// Unix ms of the write.
    pub updated_ms: u64,
    /// 183 §6.9.4: the id of the mesh key that sealed `ciphertext`; `''` = the legacy key.
    pub key_id: String,
    /// Absolute expiry (Unix ms) set by this write; `None` = never. The current version's is the secret's.
    pub expires_ms: Option<u64>,
}

/// Open or create the file and run the migrations (WAL, 4 connections, 5 s busy).
///
/// Two processes may open a fresh file at once — a VM's boot-started engine
/// and the host's `first_run` CLI (79 F5) — and the migrator's "list applied,
/// then create" is not atomic on its own ("table … already exists"). `BEGIN
/// IMMEDIATE` takes the write lock first, so the loser waits on `busy_timeout`
/// and then sees the winner's rows.
pub async fn open(path: &Path) -> anyhow::Result<SqlitePool> {
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
    let mut tx = pool
        .begin_with("BEGIN IMMEDIATE")
        .await
        .context("lock for migrations")?;
    // `run_direct`, not `run(&mut tx)`: the `Acquire` blanket impl trips
    // "implementation of `Send` is not general enough" in the plugin's `start`.
    sqlx::migrate!("./migrations")
        .run_direct(None, &mut *tx, false)
        .await
        .context("run migrations")?;
    tx.commit().await.context("commit migrations")?;
    Ok(pool)
}

/// `<name>#<version>`: the replicated primary key.
pub fn id(name: &str, version: u32) -> String {
    format!("{name}#{version}")
}

/// Every row, by name then newest version first, and the current pointer per name.
pub async fn rows(pool: &SqlitePool) -> anyhow::Result<(Vec<Row>, HashMap<String, u32>)> {
    let rows = sqlx::query(
        "SELECT name, version, ciphertext, note, source, updated_ms, key_id, expires_ms FROM vault ORDER BY name, version DESC",
    )
    .fetch_all(pool)
    .await?
    .into_iter()
    .map(|r| Row {
        name: r.get("name"),
        version: r.get::<i64, _>("version") as u32,
        ciphertext: r.get("ciphertext"),
        note: r.get("note"),
        source: r.get("source"),
        updated_ms: r.get::<i64, _>("updated_ms") as u64,
        key_id: r.get("key_id"),
        expires_ms: r.get::<Option<i64>, _>("expires_ms").map(|v| v as u64),
    })
    .collect();
    let current = sqlx::query("SELECT name, version FROM vault_current")
        .fetch_all(pool)
        .await?
        .into_iter()
        .map(|r| (r.get("name"), r.get::<i64, _>("version") as u32))
        .collect();
    Ok((rows, current))
}

/// The local-only `meta` value under `k`.
pub async fn meta_get(pool: &SqlitePool, k: &str) -> anyhow::Result<Option<Vec<u8>>> {
    Ok(sqlx::query("SELECT v FROM meta WHERE k = ?")
        .bind(k)
        .fetch_optional(pool)
        .await?
        .map(|r| r.get("v")))
}

/// Set the local-only `meta` value under `k`.
pub async fn meta_put(pool: &SqlitePool, k: &str, v: &[u8]) -> anyhow::Result<()> {
    sqlx::query(
        "INSERT INTO meta (k, v) VALUES (?, ?) ON CONFLICT(k) DO UPDATE SET v = excluded.v",
    )
    .bind(k)
    .bind(v)
    .execute(pool)
    .await?;
    Ok(())
}

/// Delete every local-only `meta` row whose key starts with `prefix`.
pub async fn meta_delete_prefix(pool: &SqlitePool, prefix: &str) -> anyhow::Result<()> {
    sqlx::query("DELETE FROM meta WHERE substr(k, 1, length(?1)) = ?1")
        .bind(prefix)
        .execute(pool)
        .await?;
    Ok(())
}

/// Delete the local-only `meta` row `k`.
pub async fn meta_delete(pool: &SqlitePool, k: &str) -> anyhow::Result<()> {
    sqlx::query("DELETE FROM meta WHERE k = ?")
        .bind(k)
        .execute(pool)
        .await?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    /// 79 F5: the guest engine and the host's `first_run` CLI open a fresh
    /// vault at the same moment; every opener must come up with the schema.
    #[tokio::test]
    async fn concurrent_first_opens_all_succeed() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("secrets.sqlite");
        let mut opens = tokio::task::JoinSet::new();
        for _ in 0..8 {
            let p = path.clone();
            opens.spawn(async move { open(&p).await });
        }
        for pool in opens.join_all().await {
            let pool = pool.unwrap();
            assert!(meta_get(&pool, "nothing").await.unwrap().is_none());
            let (rows, _) = rows(&pool).await.unwrap();
            assert!(rows.is_empty());
        }
    }

    /// 183 §9.5: a row written without `key_id` (every pre-183 row, and every row a pre-183
    /// peer replicates) reads back as `''`, the legacy key.
    #[tokio::test]
    async fn a_row_without_key_id_reads_as_the_legacy_key() {
        let dir = tempfile::tempdir().unwrap();
        let pool = open(&dir.path().join(DB)).await.unwrap();
        sqlx::query(
            "INSERT INTO vault (id, name, version, ciphertext, note, source, updated_ms) \
             VALUES ('gh#1', 'gh', 1, x'00', '', 'user', 1)",
        )
        .execute(&pool)
        .await
        .unwrap();
        let (rows, _) = rows(&pool).await.unwrap();
        assert_eq!(rows[0].key_id, "");
    }
}
