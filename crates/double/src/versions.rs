//! Versioning and rollback (126 §8.5). `<data_dir>/double/models/<version>/`
//! holds `heads.json`, `knn.bin`, `temperatures.json`, `manifest.json` and —
//! for Tier B/C — `adapter.safetensors`; `<data_dir>/double/models/active` is
//! a symlink-free pointer file naming the active version, rewritten by an
//! atomic rename. The `versions` table carries the same fact (`active = 1`);
//! [`activate`] writes both, pointer first, so a crash between the two leaves
//! the pointer ahead of the table and [`load_active`] trusts the table.
//! `baseline` is never a directory: it is the zero-shot oracle alone.

use std::path::{Path, PathBuf};

use anyhow::Context as _;
use serde::{Deserialize, Serialize};

use crate::store::{BASELINE, Store, VersionRow};
use crate::train_a::Artifacts;
use crate::types::Tier;

/// `<data_dir>/double/models`.
pub fn models_dir(data_dir: &Path) -> PathBuf {
    data_dir.join(crate::store::DIR).join("models")
}

/// `models/<version>`.
pub fn version_dir(models: &Path, id: &str) -> PathBuf {
    models.join(id)
}

/// File names inside a version directory.
pub const HEADS: &str = "heads.json";
/// The flat k-NN index.
pub const KNN: &str = "knn.bin";
/// Per-class temperatures.
pub const TEMPERATURES: &str = "temperatures.json";
/// Tier, base hash, data range, venue, metrics, trained_at.
pub const MANIFEST: &str = "manifest.json";
/// Tier B/C adapter slot (W3 writes it).
pub const ADAPTER: &str = "adapter.safetensors";
/// The pointer file.
pub const ACTIVE: &str = "active";

/// `manifest.json`.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Manifest {
    /// `versions.id`.
    pub version: String,
    /// Which tier produced it.
    pub tier: Tier,
    /// Base checkpoint hash it was trained against.
    pub base_hash: String,
    /// The labelled corpus range (`<first asked_at>..<last label at>`).
    pub data_range: String,
    /// `local` | `node:<name>`.
    pub venue: String,
    /// Gate metrics.
    pub metrics: serde_json::Value,
    /// RFC 3339.
    pub trained_at: String,
    /// Seconds the fit took.
    pub duration_s: u32,
    /// Tier B seed (W3); `None` for Tier A.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub seed: Option<u64>,
}

/// Where a version's files landed.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Paths {
    /// The version directory.
    pub dir: PathBuf,
    /// `heads.json`.
    pub heads: PathBuf,
    /// `knn.bin`.
    pub knn: PathBuf,
}

/// Write a Tier A version directory (atomically: into `<id>.tmp`, then renamed).
/// Byte-identical input yields byte-identical files (§7.5).
pub fn write(models: &Path, manifest: &Manifest, artifacts: &Artifacts) -> anyhow::Result<Paths> {
    let dir = version_dir(models, &manifest.version);
    let tmp = models.join(format!("{}.tmp", manifest.version));
    let _ = std::fs::remove_dir_all(&tmp);
    std::fs::create_dir_all(&tmp).with_context(|| format!("mkdir {}", tmp.display()))?;
    std::fs::write(tmp.join(HEADS), artifacts.heads_json()?)?;
    std::fs::write(tmp.join(KNN), artifacts.knn_bin())?;
    std::fs::write(tmp.join(TEMPERATURES), artifacts.temperatures_json()?)?;
    std::fs::write(tmp.join(MANIFEST), serde_json::to_vec_pretty(manifest)?)?;
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::rename(&tmp, &dir).with_context(|| format!("rename into {}", dir.display()))?;
    Ok(Paths {
        heads: dir.join(HEADS),
        knn: dir.join(KNN),
        dir,
    })
}

/// Read a version directory back.
pub fn read(models: &Path, id: &str) -> anyhow::Result<(Manifest, Artifacts)> {
    let dir = version_dir(models, id);
    let manifest: Manifest = serde_json::from_slice(&std::fs::read(dir.join(MANIFEST))?)
        .with_context(|| format!("{}/{MANIFEST}", dir.display()))?;
    let artifacts = Artifacts::read(
        &std::fs::read(dir.join(HEADS))?,
        &std::fs::read(dir.join(KNN))?,
        &std::fs::read(dir.join(TEMPERATURES))?,
    )?;
    Ok((manifest, artifacts))
}

/// The pointer file's content, if any.
pub fn pointer(models: &Path) -> Option<String> {
    let s = std::fs::read_to_string(models.join(ACTIVE)).ok()?;
    let s = s.trim();
    (!s.is_empty()).then(|| s.to_owned())
}

/// Rewrite the pointer atomically (`active.tmp` → rename over `active`).
pub fn set_pointer(models: &Path, id: &str) -> std::io::Result<()> {
    std::fs::create_dir_all(models)?;
    let tmp = models.join(format!("{ACTIVE}.{}.tmp", std::process::id()));
    std::fs::write(&tmp, format!("{id}\n"))?;
    std::fs::rename(tmp, models.join(ACTIVE))
}

/// Make `id` active: the version must exist in `versions` (and on disk unless
/// it is `baseline`). Pointer first, table second. Returns the previously
/// active id (`None` = baseline only).
pub async fn activate(
    store: &Store,
    models: &Path,
    id: &str,
) -> anyhow::Result<Option<String>> {
    let previous = store.active_version().await?.map(|v| v.id);
    if id == BASELINE {
        set_pointer(models, BASELINE)?;
        sqlx::query("UPDATE versions SET active = 0 WHERE active = 1")
            .execute(store.pool())
            .await?;
        return Ok(previous);
    }
    anyhow::ensure!(
        version_dir(models, id).join(MANIFEST).is_file(),
        "version {id:?} has no directory under {}",
        models.display()
    );
    set_pointer(models, id)?;
    store.activate_version(id).await?;
    Ok(previous)
}

/// `double_rollback(version)`: [`activate`] with the row read for the event.
pub async fn rollback(
    store: &Store,
    models: &Path,
    id: &str,
) -> anyhow::Result<(Option<String>, serde_json::Value)> {
    let row = store
        .versions()
        .await?
        .into_iter()
        .find(|v| v.id == id)
        .ok_or_else(|| anyhow::anyhow!("version {id:?} not found"))?;
    let previous = activate(store, models, id).await?;
    Ok((previous, row.metrics))
}

/// The version predictions name right now: the active `versions` row with
/// its artefacts loaded, or `baseline` with none. Every prediction row W1
/// writes carries `id`.
#[derive(Clone, Debug)]
pub struct ActiveVersion {
    /// `versions.id`; `baseline` before the first promotion.
    pub id: String,
    /// The row, when a trained version is active.
    pub row: Option<VersionRow>,
    /// Heads, k-NN index and temperatures; `None` for `baseline`.
    pub artifacts: Option<Artifacts>,
}

impl ActiveVersion {
    /// The zero-shot oracle alone.
    pub fn baseline() -> Self {
        Self {
            id: BASELINE.to_owned(),
            row: None,
            artifacts: None,
        }
    }
}

/// Load the active version from the table (the pointer is repaired to match).
pub async fn load_active(store: &Store, models: &Path) -> anyhow::Result<ActiveVersion> {
    let Some(row) = store.active_version().await? else {
        if pointer(models).as_deref() != Some(BASELINE) && models.is_dir() {
            set_pointer(models, BASELINE)?;
        }
        return Ok(ActiveVersion::baseline());
    };
    if pointer(models).as_deref() != Some(row.id.as_str()) {
        set_pointer(models, &row.id)?;
    }
    let (_, artifacts) = read(models, &row.id)?;
    Ok(ActiveVersion {
        id: row.id.clone(),
        row: Some(row),
        artifacts: Some(artifacts),
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::train_a::Artifacts;
    use chrono::Utc;

    async fn store() -> (tempfile::TempDir, Store) {
        let dir = tempfile::tempdir().unwrap();
        let s = Store::open(&dir.path().join("double.sqlite")).await.unwrap();
        (dir, s)
    }

    fn row(id: &str) -> VersionRow {
        VersionRow {
            id: id.into(),
            tier: Tier::A,
            base_hash: "h".into(),
            adapter_path: None,
            head_path: None,
            knn_path: None,
            metrics: serde_json::json!({"n": 1}),
            data_range: "a..b".into(),
            trained_at: Utc::now(),
            venue: "local".into(),
            duration_s: 1,
            active: false,
        }
    }

    fn manifest(id: &str) -> Manifest {
        Manifest {
            version: id.into(),
            tier: Tier::A,
            base_hash: "h".into(),
            data_range: "a..b".into(),
            venue: "local".into(),
            metrics: serde_json::json!({}),
            trained_at: "2026-10-07T00:00:00Z".into(),
            duration_s: 1,
            seed: None,
        }
    }

    #[tokio::test]
    async fn rollback_restores_the_exact_pointer_and_row() {
        let (dir, s) = store().await;
        let models = models_dir(dir.path());
        for id in ["v1", "v2"] {
            write(&models, &manifest(id), &Artifacts::empty()).unwrap();
            s.put_version(&row(id)).await.unwrap();
        }
        assert_eq!(activate(&s, &models, "v1").await.unwrap(), None);
        assert_eq!(pointer(&models).as_deref(), Some("v1"));
        assert_eq!(activate(&s, &models, "v2").await.unwrap(), Some("v1".into()));
        assert_eq!(pointer(&models).as_deref(), Some("v2"));

        let (prev, metrics) = rollback(&s, &models, "v1").await.unwrap();
        assert_eq!(prev, Some("v2".into()));
        assert_eq!(metrics, serde_json::json!({"n": 1}));
        assert_eq!(pointer(&models).as_deref(), Some("v1"));
        assert_eq!(s.active_version().await.unwrap().unwrap().id, "v1");
        let active = load_active(&s, &models).await.unwrap();
        assert_eq!(active.id, "v1");
        assert!(active.artifacts.is_some());

        // Back to the zero-shot oracle.
        let (prev, _) = rollback(&s, &models, BASELINE).await.unwrap();
        assert_eq!(prev, Some("v1".into()));
        assert_eq!(pointer(&models).as_deref(), Some(BASELINE));
        assert!(s.active_version().await.unwrap().is_none());
        assert_eq!(load_active(&s, &models).await.unwrap().id, BASELINE);

        // Unknown or directory-less versions are refused and change nothing.
        assert!(rollback(&s, &models, "nope").await.is_err());
        s.put_version(&row("v3")).await.unwrap();
        assert!(activate(&s, &models, "v3").await.is_err());
        assert_eq!(pointer(&models).as_deref(), Some(BASELINE));
    }

    #[test]
    fn write_is_byte_identical_for_identical_input() {
        let dir = tempfile::tempdir().unwrap();
        let a = write(&dir.path().join("a"), &manifest("v"), &Artifacts::empty()).unwrap();
        let b = write(&dir.path().join("b"), &manifest("v"), &Artifacts::empty()).unwrap();
        for f in [HEADS, KNN, TEMPERATURES, MANIFEST] {
            assert_eq!(
                std::fs::read(a.dir.join(f)).unwrap(),
                std::fs::read(b.dir.join(f)).unwrap(),
                "{f}"
            );
        }
    }
}
