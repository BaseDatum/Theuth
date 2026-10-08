//! The generator's writes to the map store (147 §9 tables, §8 provenance):
//! elements with their edges and anchors, perspectives, the `jobs` row, and
//! the reads the resume logic needs (which generated perspectives exist,
//! which elements still lack prose). Row queries behind the panel's reads
//! are W1's (`skeleton.rs`); nothing here is read by the UI directly.
//!
//! owner: W2 — design §6.7 (resume), §8, §9

use sqlx::SqlitePool;

use super::{GraphProps, MapEdge, MapFields, MapJob, MapOrigin, MapProvenance};

/// One element as the generator stores it.
#[derive(Clone, Debug)]
pub struct ElementRow {
    /// `<perspective>/<node>`.
    pub id: String,
    /// Perspective id.
    pub perspective: String,
    /// Node id (§4.2).
    pub node: String,
    /// Parent element id; `None` for the perspective root.
    pub parent: Option<String>,
    /// 0 for the root.
    pub depth: u8,
    /// Sibling order.
    pub order_no: u32,
    /// Display name.
    pub name: String,
    /// Shortest covering path.
    pub path: Option<String>,
    /// `unit | file | symbol | cluster | external | bundle | perspective`.
    pub kind: String,
    /// Member node ids.
    pub members: Vec<String>,
    /// No diagram.
    pub leaf: bool,
    /// Who wrote it.
    pub origin: MapOrigin,
    /// §5.4.
    pub facts_hash: String,
    /// The diagram whose nodes are the children.
    pub diagram: Option<GraphProps>,
    /// Prose.
    pub fields: MapFields,
    /// How it was written; `None` for a structure-only element.
    pub provenance: Option<MapProvenance>,
    /// Edges between the children.
    pub edges: Vec<MapEdge>,
    /// `(path, line_start, line_end)`.
    pub anchors: Vec<(String, Option<i64>, Option<i64>)>,
    /// The validation error when the diagram fell back to structure.
    pub error: Option<String>,
}

/// Upsert one element and replace its edges and anchors; a changed diagram
/// keeps the previous one in `prev_diagram` (§8 "map changed: +2 nodes").
pub async fn write_element(pool: &SqlitePool, e: &ElementRow) -> anyhow::Result<()> {
    let diagram = e.diagram.as_ref().map(serde_json::to_string).transpose()?;
    let members = serde_json::to_string(&e.members)?;
    let p = e.provenance.as_ref();
    let call_ids = p.map(|p| serde_json::to_string(&p.call_ids)).transpose()?;
    let mut tx = pool.begin().await?;
    sqlx::query(
        "INSERT INTO elements (id, perspective, node, parent, depth, order_no, name, path, kind, members, leaf, origin, \
           stale, facts_hash, diagram, prev_diagram, description, context, constraint_, concern, todo, note, model, git_head, \
           index_state, generated_ms, call_ids, cost_nanos, currency, input_tokens, output_tokens, repaired, error) \
         VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, 0, ?, ?, NULL, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?) \
         ON CONFLICT(id) DO UPDATE SET \
           perspective = excluded.perspective, node = excluded.node, parent = excluded.parent, depth = excluded.depth, \
           order_no = excluded.order_no, name = excluded.name, path = excluded.path, kind = excluded.kind, \
           members = excluded.members, leaf = excluded.leaf, origin = excluded.origin, stale = 0, \
           facts_hash = excluded.facts_hash, \
           prev_diagram = CASE WHEN elements.diagram IS NOT excluded.diagram THEN elements.diagram ELSE elements.prev_diagram END, \
           diagram = excluded.diagram, description = excluded.description, context = excluded.context, \
           constraint_ = excluded.constraint_, concern = excluded.concern, todo = excluded.todo, note = excluded.note, \
           model = excluded.model, git_head = excluded.git_head, index_state = excluded.index_state, \
           generated_ms = excluded.generated_ms, call_ids = excluded.call_ids, cost_nanos = excluded.cost_nanos, \
           currency = excluded.currency, input_tokens = excluded.input_tokens, output_tokens = excluded.output_tokens, \
           repaired = excluded.repaired, error = excluded.error",
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
    .bind(i64::from(e.leaf))
    .bind(e.origin.as_str())
    .bind(&e.facts_hash)
    .bind(diagram)
    .bind(empty_as_null(&e.fields.description))
    .bind(&e.fields.context)
    .bind(&e.fields.constraint)
    .bind(&e.fields.concern)
    .bind(&e.fields.todo)
    .bind(&e.fields.note)
    .bind(p.map(|p| p.model.as_str()))
    .bind(p.and_then(|p| p.git_head.as_deref()))
    .bind(p.map(|p| p.index_state.as_str()))
    .bind(p.map(|p| i64::try_from(p.generated_ms).unwrap_or(i64::MAX)))
    .bind(call_ids)
    .bind(p.map(|p| i64::try_from(p.cost_nanos).unwrap_or(i64::MAX)))
    .bind(p.map(|p| p.currency.as_str()))
    .bind(p.map(|p| i64::from(p.input_tokens)))
    .bind(p.map(|p| i64::from(p.output_tokens)))
    .bind(p.map(|p| i64::from(p.repaired)))
    .bind(&e.error)
    .execute(&mut *tx)
    .await?;
    sqlx::query("DELETE FROM edges WHERE element = ?")
        .bind(&e.id)
        .execute(&mut *tx)
        .await?;
    for edge in &e.edges {
        sqlx::query(
            "INSERT OR REPLACE INTO edges (element, from_node, to_node, label, backed, evidence) VALUES (?, ?, ?, ?, ?, ?)",
        )
        .bind(&e.id)
        .bind(&edge.from)
        .bind(&edge.to)
        .bind(&edge.label)
        .bind(i64::from(edge.backed))
        .bind(serde_json::to_string(&edge.evidence)?)
        .execute(&mut *tx)
        .await?;
    }
    sqlx::query("DELETE FROM anchors WHERE element = ?")
        .bind(&e.id)
        .execute(&mut *tx)
        .await?;
    for (path, start, end) in &e.anchors {
        sqlx::query(
            "INSERT OR REPLACE INTO anchors (element, path, line_start, line_end) VALUES (?, ?, ?, ?)",
        )
        .bind(&e.id)
        .bind(path)
        .bind(start)
        .bind(end)
        .execute(&mut *tx)
        .await?;
    }
    tx.commit().await?;
    Ok(())
}

fn empty_as_null(s: &str) -> Option<&str> {
    (!s.trim().is_empty()).then_some(s)
}

/// Upsert a perspective row (`origin: generated`).
pub async fn write_perspective(
    pool: &SqlitePool,
    id: &str,
    kind: &str,
    title: &str,
    why: &str,
    order_no: u32,
    eligible: &serde_json::Value,
) -> anyhow::Result<()> {
    let now = now_ms();
    sqlx::query(
        "INSERT INTO perspectives (id, kind, title, origin, why, order_no, eligible, created_ms, updated_ms) \
         VALUES (?, ?, ?, 'generated', ?, ?, ?, ?, ?) \
         ON CONFLICT(id) DO UPDATE SET kind = excluded.kind, title = excluded.title, origin = 'generated', \
           why = excluded.why, order_no = excluded.order_no, eligible = excluded.eligible, updated_ms = excluded.updated_ms",
    )
    .bind(id)
    .bind(kind)
    .bind(title)
    .bind(why)
    .bind(i64::from(order_no))
    .bind(eligible.to_string())
    .bind(now)
    .bind(now)
    .execute(pool)
    .await?;
    Ok(())
}

/// Remove a perspective's elements, edges and anchors (before a stage-B
/// rewrite); the perspective row stays.
pub async fn clear_perspective(pool: &SqlitePool, id: &str) -> anyhow::Result<()> {
    let mut tx = pool.begin().await?;
    for sql in [
        "DELETE FROM edges WHERE element IN (SELECT id FROM elements WHERE perspective = ?)",
        "DELETE FROM anchors WHERE element IN (SELECT id FROM elements WHERE perspective = ?)",
        "DELETE FROM elements WHERE perspective = ?",
    ] {
        sqlx::query(sql).bind(id).execute(&mut *tx).await?;
    }
    tx.commit().await?;
    Ok(())
}

/// Stage C's write: name, prose and provenance on an existing element; the
/// element is fresh again.
pub async fn write_prose(
    pool: &SqlitePool,
    id: &str,
    name: Option<&str>,
    fields: &MapFields,
    p: &MapProvenance,
) -> anyhow::Result<()> {
    sqlx::query(
        "UPDATE elements SET name = coalesce(?, name), description = ?, context = ?, constraint_ = ?, concern = ?, \
           todo = ?, note = ?, model = ?, git_head = ?, index_state = ?, generated_ms = ?, call_ids = ?, \
           cost_nanos = ?, currency = ?, input_tokens = ?, output_tokens = ?, repaired = ?, origin = 'generated', \
           stale = 0, facts_hash = ?, error = NULL WHERE id = ?",
    )
    .bind(name)
    .bind(empty_as_null(&fields.description))
    .bind(&fields.context)
    .bind(&fields.constraint)
    .bind(&fields.concern)
    .bind(&fields.todo)
    .bind(&fields.note)
    .bind(&p.model)
    .bind(&p.git_head)
    .bind(&p.index_state)
    .bind(i64::try_from(p.generated_ms).unwrap_or(i64::MAX))
    .bind(serde_json::to_string(&p.call_ids)?)
    .bind(i64::try_from(p.cost_nanos).unwrap_or(i64::MAX))
    .bind(&p.currency)
    .bind(i64::from(p.input_tokens))
    .bind(i64::from(p.output_tokens))
    .bind(i64::from(p.repaired))
    .bind(&p.facts_hash)
    .bind(id)
    .execute(pool)
    .await?;
    Ok(())
}

/// Record a failed element: the error for the job log and the badge.
pub async fn write_error(pool: &SqlitePool, id: &str, error: &str) -> anyhow::Result<()> {
    sqlx::query("UPDATE elements SET error = ?, repaired = 1 WHERE id = ?")
        .bind(error)
        .bind(id)
        .execute(pool)
        .await?;
    Ok(())
}

/// Replace the stored diagram of `id` (edge labels and node names updated).
pub async fn write_diagram(
    pool: &SqlitePool,
    id: &str,
    g: &GraphProps,
    edges: &[MapEdge],
) -> anyhow::Result<()> {
    let mut tx = pool.begin().await?;
    sqlx::query("UPDATE elements SET diagram = ? WHERE id = ?")
        .bind(serde_json::to_string(g)?)
        .bind(id)
        .execute(&mut *tx)
        .await?;
    for edge in edges {
        sqlx::query("UPDATE edges SET label = ?, backed = ?, evidence = ? WHERE element = ? AND from_node = ? AND to_node = ?")
            .bind(&edge.label)
            .bind(i64::from(edge.backed))
            .bind(serde_json::to_string(&edge.evidence)?)
            .bind(id)
            .bind(&edge.from)
            .bind(&edge.to)
            .execute(&mut *tx)
            .await?;
    }
    tx.commit().await?;
    Ok(())
}

/// The stored diagram and edges of `id`.
pub async fn read_diagram(
    pool: &SqlitePool,
    id: &str,
) -> anyhow::Result<Option<(GraphProps, Vec<MapEdge>)>> {
    let row: Option<(Option<String>,)> =
        sqlx::query_as("SELECT diagram FROM elements WHERE id = ?")
            .bind(id)
            .fetch_optional(pool)
            .await?;
    let Some((Some(diagram),)) = row else {
        return Ok(None);
    };
    let g: GraphProps = serde_json::from_str(&diagram)?;
    let rows: Vec<(String, String, String, i64, String)> = sqlx::query_as(
        "SELECT from_node, to_node, label, backed, evidence FROM edges WHERE element = ?",
    )
    .bind(id)
    .fetch_all(pool)
    .await?;
    let edges = rows
        .into_iter()
        .map(|(from, to, label, backed, evidence)| {
            Ok(MapEdge {
                from,
                to,
                label,
                backed: backed != 0,
                evidence: serde_json::from_str(&evidence)?,
            })
        })
        .collect::<anyhow::Result<Vec<_>>>()?;
    Ok(Some((g, edges)))
}

/// Upsert the `jobs` row from a [`MapJob`].
pub async fn write_job(
    pool: &SqlitePool,
    job: &MapJob,
    finished: bool,
    error_log: Option<&str>,
) -> anyhow::Result<()> {
    sqlx::query(
        "INSERT INTO jobs (id, session, started_ms, finished_ms, stage, done, total, calls, cost_nanos, currency, error, error_log) \
         VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?) \
         ON CONFLICT(id) DO UPDATE SET finished_ms = excluded.finished_ms, stage = excluded.stage, done = excluded.done, \
           total = excluded.total, calls = excluded.calls, cost_nanos = excluded.cost_nanos, currency = excluded.currency, \
           error = excluded.error, error_log = coalesce(excluded.error_log, jobs.error_log)",
    )
    .bind(&job.id)
    .bind(job.session.to_string())
    .bind(i64::try_from(job.started_ms).unwrap_or(i64::MAX))
    .bind(finished.then(now_ms))
    .bind(&job.stage)
    .bind(i64::from(job.done))
    .bind(i64::from(job.total))
    .bind(i64::from(job.calls))
    .bind(i64::try_from(job.cost_nanos).unwrap_or(i64::MAX))
    .bind(&job.currency)
    .bind(&job.error)
    .bind(error_log)
    .execute(pool)
    .await?;
    Ok(())
}

/// One stored perspective the resume logic looks at.
#[derive(Clone, Debug)]
pub struct StoredPerspective {
    /// Perspective id.
    pub id: String,
    /// Catalogue kind.
    pub kind: String,
    /// Title.
    pub title: String,
    /// Stage A's reason.
    pub why: String,
    /// `perspectives.order_no`.
    pub order_no: u32,
    /// The root element is stale or has no prose: stage B must run again.
    pub needs_top: bool,
}

/// `(id, kind, title, why, order_no, root stale, root description)`.
type PerspectiveSql = (
    String,
    String,
    String,
    Option<String>,
    i64,
    Option<i64>,
    Option<String>,
);

/// Generated perspectives, catalogue order.
pub async fn generated_perspectives(pool: &SqlitePool) -> anyhow::Result<Vec<StoredPerspective>> {
    let rows: Vec<PerspectiveSql> = sqlx::query_as(
        "SELECT p.id, p.kind, p.title, p.why, p.order_no, e.stale, e.description FROM perspectives p \
         LEFT JOIN elements e ON e.id = p.id WHERE p.origin = 'generated' ORDER BY p.order_no",
    )
    .fetch_all(pool)
    .await?;
    Ok(rows
        .into_iter()
        .map(
            |(id, kind, title, why, order_no, stale, description)| StoredPerspective {
                id,
                kind,
                title,
                why: why.unwrap_or_default(),
                order_no: u32::try_from(order_no).unwrap_or(0),
                needs_top: stale.is_none_or(|s| s != 0)
                    || description.as_deref().is_none_or(str::is_empty),
            },
        )
        .collect())
}

/// Elements of `perspective` the model describes (§6.2 stage C): every
/// depth-1 row and every deeper row with a diagram, down to `max_depth`,
/// that is stale or without prose; bundles and files are never described.
pub async fn pending_prose(
    pool: &SqlitePool,
    perspective: &str,
    max_depth: u8,
) -> anyhow::Result<Vec<String>> {
    let rows: Vec<(String,)> = sqlx::query_as(
        "SELECT id FROM elements WHERE perspective = ? AND depth BETWEEN 1 AND ? AND kind NOT IN ('bundle', 'file') \
         AND (depth = 1 OR diagram IS NOT NULL) \
         AND (stale <> 0 OR description IS NULL OR description = '') ORDER BY depth, order_no",
    )
    .bind(perspective)
    .bind(i64::from(max_depth))
    .fetch_all(pool)
    .await?;
    Ok(rows.into_iter().map(|(id,)| id).collect())
}

/// The identity columns of one stored element.
#[derive(Clone, Debug)]
pub struct Header {
    /// Perspective id.
    pub perspective: String,
    /// Node id.
    pub node: String,
    /// Parent element id.
    pub parent: Option<String>,
    /// Depth, 0 for the root.
    pub depth: u8,
    /// Kind string.
    pub kind: String,
    /// Display name.
    pub name: String,
    /// Shortest covering path.
    pub path: Option<String>,
    /// Member node ids.
    pub members: Vec<String>,
    /// Anchor paths.
    pub anchors: Vec<String>,
}

/// `(perspective, node, parent, depth, kind, name, path, members)`.
type HeaderSql = (
    String,
    String,
    Option<String>,
    i64,
    String,
    String,
    Option<String>,
    String,
);

/// The [`Header`] of one element, when stored.
pub async fn element_header(pool: &SqlitePool, id: &str) -> anyhow::Result<Option<Header>> {
    let row: Option<HeaderSql> = sqlx::query_as(
        "SELECT perspective, node, parent, depth, kind, name, path, members FROM elements WHERE id = ?",
    )
    .bind(id)
    .fetch_optional(pool)
    .await?;
    let Some((perspective, node, parent, depth, kind, name, path, members)) = row else {
        return Ok(None);
    };
    let anchors: Vec<(String,)> =
        sqlx::query_as("SELECT path FROM anchors WHERE element = ? ORDER BY path")
            .bind(id)
            .fetch_all(pool)
            .await?;
    Ok(Some(Header {
        perspective,
        node,
        parent,
        depth: u8::try_from(depth).unwrap_or(u8::MAX),
        kind,
        name,
        path,
        members: serde_json::from_str(&members).unwrap_or_default(),
        anchors: anchors.into_iter().map(|(p,)| p).collect(),
    }))
}

/// `(id, node, name, path)` of the children of `parent`, in order.
pub async fn children_of(
    pool: &SqlitePool,
    parent: &str,
) -> anyhow::Result<Vec<(String, String, String, Option<String>)>> {
    Ok(sqlx::query_as(
        "SELECT id, node, name, path FROM elements WHERE parent = ? ORDER BY order_no",
    )
    .bind(parent)
    .fetch_all(pool)
    .await?)
}

/// Unix milliseconds now.
pub fn now_ms() -> i64 {
    use std::time::{SystemTime, UNIX_EPOCH};
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| i64::try_from(d.as_millis()).unwrap_or(i64::MAX))
        .unwrap_or(0)
}
