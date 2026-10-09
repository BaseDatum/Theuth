//! The map's own SQLite beside the index (147 §9, O7):
//! `<data_dir>/codeindex/<slug>-<hash8>-map.sqlite`, own schema version, so
//! an index rebuild never deletes paid prose. The DDL and open/create live
//! here for every worker; the row queries are W1's.
//!
//! owner: W1 — design §9, §5 (reads), §8 (stale marks)

use std::path::{Path, PathBuf};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use anyhow::Context as _;
use sqlx::sqlite::{SqliteConnectOptions, SqliteJournalMode, SqlitePool, SqlitePoolOptions};
use sqlx::{FromRow, SqliteConnection};

use super::{GraphProps, MapEdge, MapFields, MapJob, MapOrigin, MapProvenance};
use crate::root;

/// Own schema version (`meta.schema_version`); mismatch = remove and rebuild.
pub const MAP_SCHEMA_VERSION: u32 = 1;

/// §9 verbatim. `CREATE … IF NOT EXISTS` so a reopen is a no-op.
pub const DDL: &[&str] = &[
    "CREATE TABLE IF NOT EXISTS meta (
  key   TEXT PRIMARY KEY,
  value TEXT
)",
    // schema_version, root, index_state, git_head, skeleton_ms
    "CREATE TABLE IF NOT EXISTS perspectives (
  id         TEXT PRIMARY KEY,
  kind       TEXT NOT NULL,
  title      TEXT NOT NULL,
  origin     TEXT NOT NULL,      -- structure | generated | imported
  why        TEXT,
  order_no   INTEGER NOT NULL,
  eligible   TEXT,               -- JSON detector summary
  created_ms INTEGER NOT NULL,
  updated_ms INTEGER NOT NULL
)",
    "CREATE TABLE IF NOT EXISTS elements (
  id            TEXT PRIMARY KEY, -- <perspective>/<node>
  perspective   TEXT NOT NULL,
  node          TEXT NOT NULL,
  parent        TEXT,
  depth         INTEGER NOT NULL,
  order_no      INTEGER NOT NULL,
  name          TEXT NOT NULL,
  path          TEXT,
  kind          TEXT NOT NULL,    -- unit | file | symbol | cluster | external | bundle | perspective
  members       TEXT NOT NULL,    -- JSON node ids
  leaf          INTEGER NOT NULL,
  origin        TEXT NOT NULL,    -- structure | generated | imported
  stale         INTEGER NOT NULL DEFAULT 0,
  facts_hash    TEXT,
  diagram       TEXT,             -- GraphProps JSON
  prev_diagram  TEXT,
  description   TEXT,
  context       TEXT,
  constraint_   TEXT,
  concern       TEXT,
  todo          TEXT,
  note          TEXT,
  model         TEXT,
  git_head      TEXT,
  index_state   TEXT,
  generated_ms  INTEGER,
  call_ids      TEXT,             -- JSON
  cost_nanos    INTEGER,
  currency      TEXT,
  input_tokens  INTEGER,
  output_tokens INTEGER,
  repaired      INTEGER,
  error         TEXT
)",
    "CREATE TABLE IF NOT EXISTS edges (
  element   TEXT NOT NULL,
  from_node TEXT NOT NULL,
  to_node   TEXT NOT NULL,
  label     TEXT NOT NULL,
  backed    INTEGER NOT NULL,
  evidence  TEXT NOT NULL,        -- JSON MapEvidence[]
  PRIMARY KEY (element, from_node, to_node)
)",
    "CREATE TABLE IF NOT EXISTS anchors (
  element    TEXT NOT NULL,
  path       TEXT NOT NULL,
  line_start INTEGER,
  line_end   INTEGER,
  PRIMARY KEY (element, path, line_start)
)",
    "CREATE TABLE IF NOT EXISTS jobs (
  id          TEXT PRIMARY KEY,
  session     TEXT NOT NULL,
  started_ms  INTEGER NOT NULL,
  finished_ms INTEGER,
  stage       TEXT NOT NULL,
  done        INTEGER NOT NULL DEFAULT 0,
  total       INTEGER NOT NULL DEFAULT 0,
  calls       INTEGER NOT NULL DEFAULT 0,
  cost_nanos  INTEGER NOT NULL DEFAULT 0,
  currency    TEXT NOT NULL DEFAULT 'USD',
  error       TEXT,
  error_log   TEXT
)",
    "CREATE INDEX IF NOT EXISTS elements_parent ON elements(perspective, parent, order_no)",
    "CREATE INDEX IF NOT EXISTS elements_node ON elements(node)",
    "CREATE INDEX IF NOT EXISTS elements_stale ON elements(stale)",
    "CREATE INDEX IF NOT EXISTS anchors_path ON anchors(path)",
];

/// `<dir>/<slug>-<hash8>-map.sqlite`: the index's file name (`root::db_file_name`)
/// with `-map` before the extension, so `ls` pairs each index with its map.
pub fn path(dir: &Path, root: &Path) -> PathBuf {
    let index = root::db_file_name(root);
    let stem = index.strip_suffix(".sqlite").unwrap_or(&index);
    dir.join(format!("{stem}-map.sqlite"))
}

/// Open (creating) the map for `root` at `path`; a schema mismatch or an
/// unusable file removes `path`, `-wal`, `-shm` and starts over (60 §0.7's
/// rule — a map schema bump costs one regeneration).
pub async fn open(path: &Path, root: &Path) -> anyhow::Result<SqlitePool> {
    match open_db(path).await {
        Ok(pool) if schema_version(&pool).await == Some(MAP_SCHEMA_VERSION) => return Ok(pool),
        Ok(pool) => {
            if schema_version(&pool).await.is_some() {
                tracing::info!(path = %path.display(), "codemap schema changed; rebuilding");
            }
            pool.close().await;
        }
        Err(e) => {
            tracing::warn!(error = format!("{e:#}"), path = %path.display(), "codemap unusable; rebuilding");
        }
    }
    for suffix in ["", "-wal", "-shm"] {
        let _ = std::fs::remove_file(format!("{}{suffix}", path.display()));
    }
    let pool = open_db(path)
        .await
        .with_context(|| format!("open {}", path.display()))?;
    let now = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |d| d.as_millis());
    for (k, v) in [
        ("schema_version", MAP_SCHEMA_VERSION.to_string()),
        ("root", root.to_string_lossy().into_owned()),
        ("created_ms", now.to_string()),
    ] {
        set_meta(&pool, k, &v).await?;
    }
    Ok(pool)
}

async fn open_db(path: &Path) -> anyhow::Result<SqlitePool> {
    let opts = SqliteConnectOptions::new()
        .filename(path)
        .create_if_missing(true)
        .journal_mode(SqliteJournalMode::Wal)
        .busy_timeout(Duration::from_secs(5));
    let pool = SqlitePoolOptions::new()
        .max_connections(4)
        .connect_with(opts)
        .await?;
    for sql in DDL {
        sqlx::query(*sql).execute(&pool).await?;
    }
    Ok(pool)
}

async fn schema_version(pool: &SqlitePool) -> Option<u32> {
    meta(pool, "schema_version")
        .await
        .ok()
        .flatten()?
        .parse()
        .ok()
}

/// One `meta` value.
pub async fn meta(pool: &SqlitePool, key: &str) -> anyhow::Result<Option<String>> {
    Ok(sqlx::query_scalar("SELECT value FROM meta WHERE key = ?")
        .bind(key)
        .fetch_optional(pool)
        .await?)
}

/// Upsert one `meta` row.
pub async fn set_meta(pool: &SqlitePool, key: &str, value: &str) -> anyhow::Result<()> {
    sqlx::query("INSERT INTO meta (key, value) VALUES (?, ?) ON CONFLICT(key) DO UPDATE SET value = excluded.value")
        .bind(key)
        .bind(value)
        .execute(pool)
        .await?;
    Ok(())
}

/// Unix milliseconds now.
pub fn now_ms() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |d| i64::try_from(d.as_millis()).unwrap_or(i64::MAX))
}

// --- perspectives -------------------------------------------------------------

/// One `perspectives` row.
#[derive(Clone, Debug, FromRow)]
pub struct PerspectiveRow {
    /// Catalogue kind or `imported:<dir>`.
    pub id: String,
    /// Catalogue kind; `imported`.
    pub kind: String,
    /// Display title.
    pub title: String,
    /// `structure | generated | imported`.
    pub origin: String,
    /// Stage A's reason.
    pub why: Option<String>,
    /// Display order; `overall-architecture` is 0.
    pub order_no: i64,
    /// JSON [`super::detect::Detection`].
    pub eligible: Option<String>,
    /// Unix ms.
    pub created_ms: i64,
    /// Unix ms.
    pub updated_ms: i64,
}

impl PerspectiveRow {
    /// The typed origin (`structure` for anything unknown).
    pub fn origin(&self) -> MapOrigin {
        origin_of(&self.origin)
    }
}

fn origin_of(s: &str) -> MapOrigin {
    match s {
        "generated" => MapOrigin::Generated,
        "imported" => MapOrigin::Imported,
        _ => MapOrigin::Structure,
    }
}

/// Every perspective by `order_no`.
pub async fn perspectives(pool: &SqlitePool) -> anyhow::Result<Vec<PerspectiveRow>> {
    Ok(
        sqlx::query_as("SELECT * FROM perspectives ORDER BY order_no, id")
            .fetch_all(pool)
            .await?,
    )
}

/// Insert a perspective, or refresh its title/origin/why/eligible (the
/// creation stamp stays).
pub async fn upsert_perspective(
    conn: &mut SqliteConnection,
    p: &PerspectiveRow,
) -> anyhow::Result<()> {
    sqlx::query(
        "INSERT INTO perspectives (id, kind, title, origin, why, order_no, eligible, created_ms, updated_ms) \
         VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?) \
         ON CONFLICT(id) DO UPDATE SET kind = excluded.kind, title = excluded.title, origin = excluded.origin, \
         why = excluded.why, order_no = excluded.order_no, eligible = excluded.eligible, updated_ms = excluded.updated_ms",
    )
    .bind(&p.id)
    .bind(&p.kind)
    .bind(&p.title)
    .bind(&p.origin)
    .bind(&p.why)
    .bind(p.order_no)
    .bind(&p.eligible)
    .bind(p.created_ms)
    .bind(p.updated_ms)
    .execute(conn)
    .await?;
    Ok(())
}

// --- elements -----------------------------------------------------------------

/// One `elements` row as stored (§9).
#[derive(Clone, Debug, FromRow)]
pub struct ElementRow {
    /// `<perspective>/<node>`.
    pub id: String,
    /// Perspective id.
    pub perspective: String,
    /// Node id (§4.2).
    pub node: String,
    /// Parent element id.
    pub parent: Option<String>,
    /// 0 for the perspective root.
    pub depth: i64,
    /// Sibling order.
    pub order_no: i64,
    /// Display name.
    pub name: String,
    /// Covering path.
    pub path: Option<String>,
    /// `unit | file | symbol | cluster | external | bundle | perspective`.
    pub kind: String,
    /// JSON node ids.
    pub members: String,
    /// No diagram.
    pub leaf: bool,
    /// `structure | generated | imported`.
    pub origin: String,
    /// Facts hash moved since generation.
    pub stale: bool,
    /// Facts hash at the last write.
    pub facts_hash: Option<String>,
    /// `GraphProps` JSON.
    pub diagram: Option<String>,
    /// The previous generation's diagram JSON.
    pub prev_diagram: Option<String>,
    /// Prose.
    pub description: Option<String>,
    /// Prose.
    pub context: Option<String>,
    /// Prose (`constraint` is reserved in SQL).
    pub constraint_: Option<String>,
    /// Prose.
    pub concern: Option<String>,
    /// Prose.
    pub todo: Option<String>,
    /// Prose.
    pub note: Option<String>,
    /// Provenance.
    pub model: Option<String>,
    /// Provenance.
    pub git_head: Option<String>,
    /// Provenance.
    pub index_state: Option<String>,
    /// Provenance.
    pub generated_ms: Option<i64>,
    /// Provenance, JSON.
    pub call_ids: Option<String>,
    /// Provenance.
    pub cost_nanos: Option<i64>,
    /// Provenance.
    pub currency: Option<String>,
    /// Provenance.
    pub input_tokens: Option<i64>,
    /// Provenance.
    pub output_tokens: Option<i64>,
    /// Provenance.
    pub repaired: Option<bool>,
    /// The job's error for this element.
    pub error: Option<String>,
}

impl ElementRow {
    /// The typed origin.
    pub fn origin(&self) -> MapOrigin {
        origin_of(&self.origin)
    }

    /// `members` decoded.
    pub fn members(&self) -> Vec<String> {
        serde_json::from_str(&self.members).unwrap_or_default()
    }

    /// `diagram` decoded; `None` when absent or unreadable.
    pub fn diagram(&self) -> Option<GraphProps> {
        self.diagram
            .as_deref()
            .and_then(|d| serde_json::from_str(d).ok())
    }

    /// The prose fields; empty strings read as absent.
    pub fn fields(&self) -> MapFields {
        let some = |s: &Option<String>| s.clone().filter(|s| !s.trim().is_empty());
        MapFields {
            description: self.description.clone().unwrap_or_default(),
            context: some(&self.context),
            constraint: some(&self.constraint_),
            concern: some(&self.concern),
            todo: some(&self.todo),
            note: some(&self.note),
        }
    }

    /// Provenance when a model (or an import) wrote the row.
    pub fn provenance(&self) -> Option<MapProvenance> {
        let model = self.model.clone()?;
        Some(MapProvenance {
            model,
            git_head: self.git_head.clone(),
            index_state: self.index_state.clone().unwrap_or_default(),
            generated_ms: self
                .generated_ms
                .and_then(|n| u64::try_from(n).ok())
                .unwrap_or(0),
            call_ids: self
                .call_ids
                .as_deref()
                .and_then(|c| serde_json::from_str(c).ok())
                .unwrap_or_default(),
            cost_nanos: self
                .cost_nanos
                .and_then(|n| u64::try_from(n).ok())
                .unwrap_or(0),
            currency: self.currency.clone().unwrap_or_else(|| "USD".to_owned()),
            input_tokens: self
                .input_tokens
                .and_then(|n| u32::try_from(n).ok())
                .unwrap_or(0),
            output_tokens: self
                .output_tokens
                .and_then(|n| u32::try_from(n).ok())
                .unwrap_or(0),
            repaired: self.repaired.unwrap_or(false),
            facts_hash: self.facts_hash.clone().unwrap_or_default(),
        })
    }
}

/// Every element of `perspective`, parents before children (`depth`, then
/// `parent`, then `order_no`).
pub async fn elements_of(pool: &SqlitePool, perspective: &str) -> anyhow::Result<Vec<ElementRow>> {
    Ok(sqlx::query_as(
        "SELECT * FROM elements WHERE perspective = ? ORDER BY depth, parent, order_no, id",
    )
    .bind(perspective)
    .fetch_all(pool)
    .await?)
}

/// Every element of every perspective, parents before children.
pub async fn all_elements(pool: &SqlitePool) -> anyhow::Result<Vec<ElementRow>> {
    Ok(
        sqlx::query_as("SELECT * FROM elements ORDER BY perspective, depth, parent, order_no, id")
            .fetch_all(pool)
            .await?,
    )
}

/// One element by id.
pub async fn element(pool: &SqlitePool, id: &str) -> anyhow::Result<Option<ElementRow>> {
    Ok(sqlx::query_as("SELECT * FROM elements WHERE id = ?")
        .bind(id)
        .fetch_optional(pool)
        .await?)
}

/// The children of `parent` in sibling order.
pub async fn children_of(pool: &SqlitePool, parent: &str) -> anyhow::Result<Vec<ElementRow>> {
    Ok(
        sqlx::query_as("SELECT * FROM elements WHERE parent = ? ORDER BY order_no, id")
            .bind(parent)
            .fetch_all(pool)
            .await?,
    )
}

/// Element ids of other perspectives for the same node ("also in …").
pub async fn also_in(pool: &SqlitePool, node: &str, except: &str) -> anyhow::Result<Vec<String>> {
    Ok(sqlx::query_scalar(
        "SELECT id FROM elements WHERE node = ? AND id <> ? ORDER BY perspective",
    )
    .bind(node)
    .bind(except)
    .fetch_all(pool)
    .await?)
}

/// What the skeleton writes for a structure element (§5.6): the model's
/// columns stay untouched by [`put_structure`].
#[derive(Clone, Debug)]
pub struct StructureElement {
    /// `<perspective>/<node>`.
    pub id: String,
    /// Perspective id.
    pub perspective: String,
    /// Node id.
    pub node: String,
    /// Parent element id.
    pub parent: Option<String>,
    /// 0 for the root.
    pub depth: u8,
    /// Sibling order.
    pub order_no: u32,
    /// Basename or a mechanical name (`+N more`).
    pub name: String,
    /// Covering path.
    pub path: Option<String>,
    /// `unit | file | bundle | perspective`.
    pub kind: String,
    /// Member node ids (bundles).
    pub members: Vec<String>,
    /// No diagram.
    pub leaf: bool,
    /// §5.4.
    pub facts_hash: String,
    /// The structure diagram, when not a leaf.
    pub diagram: Option<GraphProps>,
}

/// Insert or rewrite a structure-origin element. A row the model wrote keeps
/// its name, prose, diagram and provenance: only the live columns (`parent`,
/// `depth`, `order_no`, `path`, `kind`, `members`, `leaf`) move, and `stale`
/// is set by the caller through [`set_stale`].
pub async fn put_structure(
    conn: &mut SqliteConnection,
    e: &StructureElement,
) -> anyhow::Result<()> {
    let members = serde_json::to_string(&e.members)?;
    let diagram = e.diagram.as_ref().map(serde_json::to_string).transpose()?;
    sqlx::query(
        "INSERT INTO elements (id, perspective, node, parent, depth, order_no, name, path, kind, members, leaf, origin, stale, facts_hash, diagram) \
         VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, 'structure', 0, ?, ?) \
         ON CONFLICT(id) DO UPDATE SET \
           parent = excluded.parent, depth = excluded.depth, order_no = excluded.order_no, path = excluded.path, \
           kind = excluded.kind, members = CASE WHEN elements.origin = 'structure' THEN excluded.members ELSE elements.members END, \
           leaf = excluded.leaf, \
           name = CASE WHEN elements.origin = 'structure' THEN excluded.name ELSE elements.name END, \
           facts_hash = CASE WHEN elements.origin = 'structure' THEN excluded.facts_hash ELSE elements.facts_hash END, \
           diagram = CASE WHEN elements.origin = 'structure' THEN excluded.diagram ELSE elements.diagram END, \
           stale = CASE WHEN elements.origin = 'structure' THEN 0 ELSE elements.stale END",
    )
    .bind(&e.id)
    .bind(&e.perspective)
    .bind(&e.node)
    .bind(&e.parent)
    .bind(i64::from(e.depth))
    .bind(i64::from(e.order_no))
    .bind(&e.name)
    .bind(&e.path)
    .bind(&e.kind)
    .bind(members)
    .bind(e.leaf)
    .bind(&e.facts_hash)
    .bind(diagram)
    .execute(conn)
    .await?;
    Ok(())
}

/// Mark one element stale or fresh (§8).
pub async fn set_stale(conn: &mut SqliteConnection, id: &str, stale: bool) -> anyhow::Result<()> {
    sqlx::query("UPDATE elements SET stale = ? WHERE id = ?")
        .bind(stale)
        .bind(id)
        .execute(conn)
        .await?;
    Ok(())
}

/// Delete elements with their edges and anchors.
pub async fn delete_elements(conn: &mut SqliteConnection, ids: &[String]) -> anyhow::Result<()> {
    for id in ids {
        sqlx::query("DELETE FROM edges WHERE element = ?")
            .bind(id)
            .execute(&mut *conn)
            .await?;
        sqlx::query("DELETE FROM anchors WHERE element = ?")
            .bind(id)
            .execute(&mut *conn)
            .await?;
        sqlx::query("DELETE FROM elements WHERE id = ?")
            .bind(id)
            .execute(&mut *conn)
            .await?;
    }
    Ok(())
}

// --- edges and anchors ----------------------------------------------------------

/// Replace the edges of one element.
pub async fn replace_edges(
    conn: &mut SqliteConnection,
    element: &str,
    edges: &[MapEdge],
) -> anyhow::Result<()> {
    sqlx::query("DELETE FROM edges WHERE element = ?")
        .bind(element)
        .execute(&mut *conn)
        .await?;
    for e in edges {
        sqlx::query(
            "INSERT OR REPLACE INTO edges (element, from_node, to_node, label, backed, evidence) VALUES (?, ?, ?, ?, ?, ?)",
        )
        .bind(element)
        .bind(&e.from)
        .bind(&e.to)
        .bind(&e.label)
        .bind(e.backed)
        .bind(serde_json::to_string(&e.evidence)?)
        .execute(&mut *conn)
        .await?;
    }
    Ok(())
}

/// The edges of one element, in `(from, to)` order.
pub async fn edges_of(pool: &SqlitePool, element: &str) -> anyhow::Result<Vec<MapEdge>> {
    let rows: Vec<(String, String, String, bool, String)> = sqlx::query_as(
        "SELECT from_node, to_node, label, backed, evidence FROM edges WHERE element = ? ORDER BY from_node, to_node",
    )
    .bind(element)
    .fetch_all(pool)
    .await?;
    Ok(rows
        .into_iter()
        .map(|(from, to, label, backed, evidence)| MapEdge {
            from,
            to,
            label,
            backed,
            evidence: serde_json::from_str(&evidence).unwrap_or_default(),
        })
        .collect())
}

/// One stored anchor: `(path, line_start, line_end)`.
pub type AnchorRow = (String, Option<i64>, Option<i64>);

/// Replace the anchors of one element.
pub async fn replace_anchors(
    conn: &mut SqliteConnection,
    element: &str,
    anchors: &[AnchorRow],
) -> anyhow::Result<()> {
    sqlx::query("DELETE FROM anchors WHERE element = ?")
        .bind(element)
        .execute(&mut *conn)
        .await?;
    for (path, start, end) in anchors {
        sqlx::query("INSERT OR REPLACE INTO anchors (element, path, line_start, line_end) VALUES (?, ?, ?, ?)")
            .bind(element)
            .bind(path)
            .bind(start)
            .bind(end)
            .execute(&mut *conn)
            .await?;
    }
    Ok(())
}

/// The anchors of one element, by path.
pub async fn anchors_of(pool: &SqlitePool, element: &str) -> anyhow::Result<Vec<AnchorRow>> {
    Ok(sqlx::query_as("SELECT path, line_start, line_end FROM anchors WHERE element = ? ORDER BY path, line_start")
        .bind(element)
        .fetch_all(pool)
        .await?)
}

// --- search, counts, jobs -------------------------------------------------------

/// Elements whose name contains `q` (case-insensitive), shallow first.
pub async fn search_names(
    pool: &SqlitePool,
    q: &str,
    limit: i64,
) -> anyhow::Result<Vec<ElementRow>> {
    let pat = format!("%{}%", q.replace('%', "").replace('_', "\\_"));
    Ok(sqlx::query_as(
        "SELECT * FROM elements WHERE name LIKE ? ESCAPE '\\' COLLATE NOCASE ORDER BY depth, perspective, order_no LIMIT ?",
    )
    .bind(pat)
    .bind(limit)
    .fetch_all(pool)
    .await?)
}

/// Elements whose anchors cover `path` (the anchor equals it or is a
/// directory prefix of it), deepest first.
pub async fn elements_anchoring(
    pool: &SqlitePool,
    path: &str,
    limit: i64,
) -> anyhow::Result<Vec<ElementRow>> {
    Ok(sqlx::query_as(
        "SELECT e.* FROM elements e JOIN anchors a ON a.element = e.id \
         WHERE a.path = ?1 OR substr(?1, 1, length(a.path) + 1) = a.path || '/' \
         GROUP BY e.id ORDER BY e.depth DESC, e.perspective, e.order_no LIMIT ?2",
    )
    .bind(path)
    .bind(limit)
    .fetch_all(pool)
    .await?)
}

/// Stale elements.
pub async fn stale_count(pool: &SqlitePool) -> anyhow::Result<i64> {
    Ok(
        sqlx::query_scalar("SELECT count(*) FROM elements WHERE stale = 1")
            .fetch_one(pool)
            .await?,
    )
}

/// SQL over alias `e`: the elements stage C describes (§6.2) — depth ≥ 1,
/// not a bundle or a file, a depth-1 row or one that opens a diagram. The one
/// predicate `generator_write::pending_prose` and [`state_counts`] share, so a
/// finished run reads `ready`, never `partial` over a leaf nobody describes.
pub const DESCRIBED_SQL: &str =
    "e.depth >= 1 AND e.kind NOT IN ('bundle', 'file') AND (e.depth = 1 OR e.diagram IS NOT NULL)";
/// SQL over alias `e`: no prose yet.
pub const NO_PROSE_SQL: &str = "(e.description IS NULL OR e.description = '')";

/// `(generated perspectives, described-kind elements without prose inside generated perspectives, errored elements)`:
/// what `MapMeta.state` is decided from.
pub async fn state_counts(pool: &SqlitePool) -> anyhow::Result<(i64, i64, i64)> {
    // Composed from two `const` fragments, no caller input.
    let sql = format!(
        "SELECT \
           (SELECT count(*) FROM perspectives WHERE origin = 'generated'), \
           (SELECT count(*) FROM elements e JOIN perspectives p ON p.id = e.perspective \
              WHERE p.origin = 'generated' AND {DESCRIBED_SQL} AND {NO_PROSE_SQL}), \
           (SELECT count(*) FROM elements WHERE error IS NOT NULL)"
    );
    Ok(sqlx::query_as(sqlx::AssertSqlSafe(sql))
        .fetch_one(pool)
        .await?)
}

/// One `jobs` row as `running_job` reads it.
type JobRow = (
    String,
    String,
    i64,
    String,
    i64,
    i64,
    i64,
    i64,
    String,
    Option<String>,
);

/// The running job (no `finished_ms`), newest first.
pub async fn running_job(pool: &SqlitePool) -> anyhow::Result<Option<MapJob>> {
    let row: Option<JobRow> = sqlx::query_as(
        "SELECT id, session, started_ms, stage, done, total, calls, cost_nanos, currency, error \
         FROM jobs WHERE finished_ms IS NULL ORDER BY started_ms DESC LIMIT 1",
    )
    .fetch_optional(pool)
    .await?;
    Ok(row.map(
        |(id, session, started_ms, stage, done, total, calls, cost_nanos, currency, error)| {
            MapJob {
                id,
                session: session.parse().unwrap_or_default(),
                started_ms: u64::try_from(started_ms).unwrap_or(0),
                stage,
                done: u32::try_from(done).unwrap_or(u32::MAX),
                total: u32::try_from(total).unwrap_or(u32::MAX),
                calls: u32::try_from(calls).unwrap_or(u32::MAX),
                cost_nanos: u64::try_from(cost_nanos).unwrap_or(0),
                currency,
                error,
            }
        },
    ))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn map_path_pairs_with_the_index_file() {
        let dir = Path::new("/data/codeindex");
        let root = Path::new("/Users/sky/Develop/theuth");
        let index = root::db_file_name(root);
        let map = path(dir, root);
        let name = map.file_name().unwrap().to_str().unwrap();
        assert!(name.ends_with("-map.sqlite"), "{name}");
        assert_eq!(
            name.strip_suffix("-map.sqlite").unwrap(),
            index.strip_suffix(".sqlite").unwrap()
        );
    }

    #[tokio::test]
    async fn open_creates_every_table_and_rebuilds_on_version_mismatch() {
        let dir = std::env::temp_dir().join(format!("codemap-store-{}", uuid::Uuid::now_v7()));
        std::fs::create_dir_all(&dir).unwrap();
        let root = Path::new("/r");
        let p = path(&dir, root);
        let pool = open(&p, root).await.unwrap();
        let tables: Vec<String> = sqlx::query_scalar(
            "SELECT name FROM sqlite_master WHERE type = 'table' AND name NOT LIKE 'sqlite_%' ORDER BY name",
        )
        .fetch_all(&pool)
        .await
        .unwrap();
        assert_eq!(
            tables,
            [
                "anchors",
                "edges",
                "elements",
                "jobs",
                "meta",
                "perspectives"
            ]
        );
        set_meta(&pool, "schema_version", "0").await.unwrap();
        sqlx::query("INSERT INTO perspectives (id, kind, title, origin, order_no, created_ms, updated_ms) VALUES ('p', 'k', 't', 'structure', 0, 0, 0)")
            .execute(&pool)
            .await
            .unwrap();
        pool.close().await;
        let pool = open(&p, root).await.unwrap();
        let n: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM perspectives")
            .fetch_one(&pool)
            .await
            .unwrap();
        assert_eq!(n, 0, "a version mismatch starts over");
        assert_eq!(schema_version(&pool).await, Some(MAP_SCHEMA_VERSION));
        pool.close().await;
        let _ = std::fs::remove_dir_all(&dir);
    }
}
