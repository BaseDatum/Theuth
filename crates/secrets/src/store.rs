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
pub const TABLES: &[&str] = &["vault", "vault_current"];

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
}

/// Open or create the file and run the migrations (WAL, 4 connections, 5 s busy).
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
    sqlx::migrate!("./migrations").run(&pool).await.context("run migrations")?;
    Ok(pool)
}

/// `<name>#<version>`: the replicated primary key.
pub fn id(name: &str, version: u32) -> String {
    format!("{name}#{version}")
}

/// Every row, by name then newest version first, and the current pointer per name.
pub async fn rows(pool: &SqlitePool) -> anyhow::Result<(Vec<Row>, HashMap<String, u32>)> {
    let rows = sqlx::query(
        "SELECT name, version, ciphertext, note, source, updated_ms FROM vault ORDER BY name, version DESC",
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
    sqlx::query("INSERT INTO meta (k, v) VALUES (?, ?) ON CONFLICT(k) DO UPDATE SET v = excluded.v")
        .bind(k)
        .bind(v)
        .execute(pool)
        .await?;
    Ok(())
}
