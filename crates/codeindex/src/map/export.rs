//! Export to the repository (147 §13.1, S3) and the `code_overview` text
//! form (§12.1): `codemap_export(root)` writes `<root>/.omm/` in
//! oh-my-mermaid's layout — `config.yaml`, one directory per element nested
//! by containment, `diagram.mmd` (a `flowchart` whose node ids are the child
//! directory names and whose labels are omm's two lines `Name\npath`), the
//! present prose files, `meta.yaml` with omm's fields plus a `theuth:` block.
//! Refuses when a foreign `.omm/` exists (`import::is_ours`).
//!
//! owner: W4 — design §12.1 (text form), §13.1

use std::collections::{BTreeMap, HashSet};
use std::path::Path;

use anyhow::Context as _;
use sqlx::{Row as _, SqlitePool};
use surface::{GraphDirection, GraphEdgeStyle, GraphProps};

use super::import::{FIELD_FILES, is_ours, ms_to_iso};
use super::{Export, MapElement, MapExport, MapIndex, MapOrigin, MapState, store};
use crate::index::Codeindex;
use crate::{root, store as index_store};

/// `config.yaml` `generator:` value that marks an `.omm/` as ours.
pub const GENERATOR: &str = "theuth-147";
/// `config.yaml` `version:`.
pub const OMM_VERSION: &str = "0.1.0";
/// The exported directory under the root.
pub const OMM_DIR: &str = ".omm";

/// `codemap_export`: write `<root>/.omm/`; structure-only elements export
/// their structure diagram and no prose; imported perspectives are skipped.
pub async fn export(ix: &Codeindex, m: Export) -> anyhow::Result<MapExport> {
    let root = root::discover(Some(&m.root))?;
    anyhow::ensure!(
        ix.indexed(&root),
        "{} is not indexed; run a code_* tool first",
        root.display()
    );
    let ri = ix.open_root(&root).await?;
    anyhow::ensure!(
        index_store::meta(&ri.pool, "last_reconcile_at")
            .await?
            .is_some(),
        "{} is not indexed; run a code_* tool first",
        root.display()
    );
    let git_head = index_store::meta(&ri.pool, "git_head").await?;
    let pool = ix.map_pool(&ri).await?;
    write_omm(&pool, &root, git_head.as_deref()).await
}

/// One `elements` row as the export needs it.
struct Row {
    id: String,
    node: String,
    parent: Option<String>,
    name: String,
    path: Option<String>,
    facts_hash: Option<String>,
    diagram: Option<GraphProps>,
    fields: Vec<(&'static str, String)>,
    model: Option<String>,
    generated_ms: Option<i64>,
    structure: bool,
}

/// Write `pool`'s map for `root` under `<root>/.omm/` (the testable core of
/// [`export`]). Refuses a foreign `.omm/`; replaces our own.
pub async fn write_omm(
    pool: &SqlitePool,
    root: &Path,
    git_head: Option<&str>,
) -> anyhow::Result<MapExport> {
    let dir = root.join(OMM_DIR);
    if dir.exists() {
        anyhow::ensure!(
            is_ours(&dir)?,
            "{} exists and was not written by theuth (config.yaml has no `generator: {GENERATOR}`); refusing to overwrite it",
            dir.display()
        );
        std::fs::remove_dir_all(&dir).with_context(|| format!("remove {}", dir.display()))?;
    }
    std::fs::create_dir_all(&dir).with_context(|| format!("create {}", dir.display()))?;
    let mut out = MapExport {
        dir: dir.clone(),
        elements: 0,
        files: 0,
        skipped: Vec::new(),
    };
    std::fs::write(
        dir.join("config.yaml"),
        format!(
            "version: {}\ngenerator: {GENERATOR}\nlanguage: English\n",
            yq(OMM_VERSION)
        ),
    )?;
    out.files += 1;
    let perspectives = sqlx::query("SELECT id, title, origin FROM perspectives ORDER BY order_no")
        .fetch_all(pool)
        .await?;
    for p in perspectives {
        let pid: String = p.get("id");
        let origin: String = p.get("origin");
        if origin == MapOrigin::Imported.as_str() {
            out.skipped
                .push(format!("{pid}: imported perspective (read-only)"));
            continue;
        }
        let rows = sqlx::query(
            "SELECT id, node, parent, name, path, origin, facts_hash, diagram, description, context, constraint_, concern, \
             todo, note, model, generated_ms FROM elements WHERE perspective = ? ORDER BY depth, order_no",
        )
        .bind(&pid)
        .fetch_all(pool)
        .await?;
        let mut by_parent: BTreeMap<Option<String>, Vec<Row>> = BTreeMap::new();
        let mut root_row = None;
        for r in rows {
            let id: String = r.get("id");
            let diagram = match r.get::<Option<String>, _>("diagram") {
                Some(json) => match serde_json::from_str::<GraphProps>(&json) {
                    Ok(g) => Some(g),
                    Err(e) => {
                        out.skipped.push(format!("{id}: diagram unreadable ({e})"));
                        None
                    }
                },
                None => None,
            };
            let origin: String = r.get("origin");
            let row = Row {
                node: r.get("node"),
                parent: r.get("parent"),
                name: r.get("name"),
                path: r.get("path"),
                facts_hash: r.get("facts_hash"),
                diagram,
                fields: FIELD_FILES
                    .iter()
                    .filter_map(|(file, col)| {
                        r.get::<Option<String>, _>(*col)
                            .filter(|s| !s.trim().is_empty())
                            .map(|s| (*file, s))
                    })
                    .collect(),
                model: r.get("model"),
                generated_ms: r.get("generated_ms"),
                structure: origin == MapOrigin::Structure.as_str(),
                id,
            };
            if row.id == pid {
                root_row = Some(row);
            } else {
                by_parent.entry(row.parent.clone()).or_default().push(row);
            }
        }
        let Some(root_row) = root_row else {
            out.skipped
                .push(format!("{pid}: perspective without a root element"));
            continue;
        };
        let edges = sqlx::query(
            "SELECT element, from_node, to_node, label, backed FROM edges WHERE element = ?1 OR element LIKE ?1 || '/%'",
        )
        .bind(&pid)
        .fetch_all(pool)
        .await?;
        let mut edge_rows: BTreeMap<String, Vec<(String, String, String, bool)>> = BTreeMap::new();
        for e in edges {
            edge_rows.entry(e.get("element")).or_default().push((
                e.get("from_node"),
                e.get("to_node"),
                e.get("label"),
                e.get("backed"),
            ));
        }
        let title: String = p.get("title");
        let mut w = Writer {
            by_parent: &by_parent,
            edge_rows: &edge_rows,
            git_head,
            now: ms_to_iso(u64::try_from(store::now_ms()).unwrap_or(0)),
            out: &mut out,
        };
        w.element(&root_row, &dir.join(&pid), &title, &[])?;
    }
    Ok(out)
}

struct Writer<'a> {
    by_parent: &'a BTreeMap<Option<String>, Vec<Row>>,
    edge_rows: &'a BTreeMap<String, Vec<(String, String, String, bool)>>,
    git_head: Option<&'a str>,
    now: String,
    out: &'a mut MapExport,
}

impl Writer<'_> {
    fn element(
        &mut self,
        row: &Row,
        dir: &Path,
        title: &str,
        parent_path: &[String],
    ) -> anyhow::Result<()> {
        std::fs::create_dir_all(dir).with_context(|| format!("create {}", dir.display()))?;
        let children = self
            .by_parent
            .get(&Some(row.id.clone()))
            .map(Vec::as_slice)
            .unwrap_or_default();
        // Child directory names: the kebab-cased name, collisions suffixed `-2`, `-3`, ….
        let mut taken = HashSet::new();
        let slugs: Vec<String> = children
            .iter()
            .map(|c| {
                let base = slug(&c.name);
                let mut s = base.clone();
                let mut n = 2;
                while !taken.insert(s.clone()) {
                    s = format!("{base}-{n}");
                    n += 1;
                }
                s
            })
            .collect();
        let slug_of = |node: &str| {
            children
                .iter()
                .position(|c| c.node == node)
                .map(|i| slugs[i].clone())
        };
        let mut files = 0;
        let (mut backed, mut inferred) = (0, 0);
        let mut last_field = "description";
        if let Some(g) = &row.diagram
            && !g.nodes.is_empty()
        {
            let table = self
                .edge_rows
                .get(&row.id)
                .map(Vec::as_slice)
                .unwrap_or_default();
            let mut ids: BTreeMap<&str, String> = BTreeMap::new();
            let mut free = taken.clone();
            for n in &g.nodes {
                let s = slug_of(&n.id).unwrap_or_else(|| {
                    let base = slug(if n.label.is_empty() { &n.id } else { &n.label });
                    let mut s = base.clone();
                    let mut k = 2;
                    while !free.insert(s.clone()) {
                        s = format!("{base}-{k}");
                        k += 1;
                    }
                    s
                });
                ids.insert(&n.id, s);
            }
            let dir_word = match g.direction.unwrap_or(GraphDirection::LR) {
                GraphDirection::LR => "LR",
                GraphDirection::TB => "TB",
            };
            let mut mmd = format!("flowchart {dir_word}\n");
            for n in &g.nodes {
                let child = children.iter().find(|c| c.node == n.id);
                let name = child.map_or(n.label.as_str(), |c| c.name.as_str());
                let path = child.and_then(|c| c.path.as_deref()).or(n.sub.as_deref());
                let label = match path {
                    Some(p) if !p.is_empty() => format!("{name}\\n{p}"),
                    _ => name.to_owned(),
                };
                mmd.push_str(&format!("    {}[{}]\n", ids[n.id.as_str()], quoted(&label)));
            }
            mmd.push('\n');
            for e in &g.edges {
                let (Some(f), Some(t)) = (ids.get(e.from.as_str()), ids.get(e.to.as_str())) else {
                    continue;
                };
                let row_edge = table.iter().find(|(a, b, _, _)| *a == e.from && *b == e.to);
                let label = row_edge
                    .map(|(_, _, l, _)| l.as_str())
                    .or(e.label.as_deref())
                    .unwrap_or("");
                let dashed = row_edge.is_some_and(|(_, _, _, b)| !*b)
                    || e.style == Some(GraphEdgeStyle::Dashed);
                if dashed {
                    inferred += 1;
                } else {
                    backed += 1;
                }
                let arrow = if dashed { "-.->" } else { "-->" };
                if label.is_empty() {
                    mmd.push_str(&format!("    {f} {arrow} {t}\n"));
                } else {
                    mmd.push_str(&format!("    {f} {arrow}|{}| {t}\n", quoted(label)));
                }
            }
            std::fs::write(dir.join("diagram.mmd"), mmd)?;
            files += 1;
            last_field = "diagram";
        }
        if !row.structure {
            for (file, text) in &row.fields {
                let mut body = text.trim().to_owned();
                body.push('\n');
                std::fs::write(dir.join(file), body)?;
                files += 1;
            }
        }
        let written_at = row.generated_ms.map_or_else(
            || self.now.clone(),
            |ms| ms_to_iso(u64::try_from(ms).unwrap_or(0)),
        );
        let mut meta = format!(
            "created: {written_at}\nupdated: {}\nupdate_count: 1\nlast_field: {last_field}\n",
            self.now
        );
        if let Some(h) = self.git_head {
            meta.push_str(&format!("git_commit: {h}\n"));
        }
        meta.push_str(&format!(
            "kind: {}\ntitle: {}\n",
            if parent_path.is_empty() {
                "perspective"
            } else {
                "nested-class"
            },
            yq(title)
        ));
        meta.push_str(&yaml_list("children", &slugs));
        meta.push_str(&yaml_list("parentPath", parent_path));
        meta.push_str(&format!(
            "theuth:\n  element: {}\n  facts_hash: {}\n  model: {}\n  backed_edges: {backed}\n  inferred_edges: {inferred}\n",
            yq(&row.id),
            yq(row.facts_hash.as_deref().unwrap_or("")),
            yq(row.model.as_deref().unwrap_or(if row.structure { "structure" } else { "" })),
        ));
        std::fs::write(dir.join("meta.yaml"), meta)?;
        files += 1;
        self.out.files += files;
        self.out.elements += 1;
        let mut child_path = parent_path.to_vec();
        child_path.push(
            dir.file_name()
                .map(|s| s.to_string_lossy().into_owned())
                .unwrap_or_default(),
        );
        for (c, s) in children.iter().zip(&slugs) {
            self.element(c, &dir.join(s), &c.name, &child_path)?;
        }
        Ok(())
    }
}

/// omm's directory name for an element: lowercase ASCII words joined by `-`.
fn slug(name: &str) -> String {
    let mut s = String::with_capacity(name.len());
    for c in name.chars() {
        if c.is_ascii_alphanumeric() {
            s.push(c.to_ascii_lowercase());
        } else if !s.ends_with('-') && !s.is_empty() {
            s.push('-');
        }
    }
    let s = s.trim_end_matches('-').to_owned();
    if s.is_empty() {
        "element".to_owned()
    } else {
        s
    }
}

/// A mermaid label inside `"…"`: the quote and angle brackets as entity codes.
fn quoted(label: &str) -> String {
    format!(
        "\"{}\"",
        label
            .replace('"', "#quot;")
            .replace('<', "#lt;")
            .replace('>', "#gt;")
    )
}

/// A YAML double-quoted scalar.
fn yq(s: &str) -> String {
    format!(
        "\"{}\"",
        s.replace('\\', "\\\\")
            .replace('"', "\\\"")
            .replace('\n', " ")
    )
}

fn yaml_list(key: &str, items: &[String]) -> String {
    if items.is_empty() {
        return format!("{key}: []\n");
    }
    let mut out = format!("{key}:\n");
    for i in items {
        out.push_str(&format!("  - {}\n", yq(i)));
    }
    out
}

// --- code_overview text form (§12.1) ------------------------------------------

/// `code_overview`'s text form (§12.1): the perspective tree as indented
/// lines (omm's `tree`), then the element's prose and mermaid when one is
/// asked for, cut at `budget` bytes on a line boundary. Returns the text and
/// the lines dropped.
pub fn code_overview_text(
    index: &MapIndex,
    element: Option<&MapElement>,
    budget: usize,
) -> anyhow::Result<(String, u32)> {
    let m = &index.meta;
    let state = match m.state {
        MapState::None => "no index",
        MapState::Structure => "structure only — open the CodeMap to generate names and prose",
        MapState::Generating => "generating",
        MapState::Ready => "ready",
        MapState::Partial => "partial",
    };
    let elements: u32 = index.perspectives.iter().map(|p| p.elements).sum();
    let mut head = format!(
        "{} · {state} · {} perspectives · {elements} elements",
        m.root
            .file_name()
            .map(|s| s.to_string_lossy().into_owned())
            .unwrap_or_default(),
        index.perspectives.len()
    );
    if m.stale_elements > 0 {
        head.push_str(&format!(" · {} stale", m.stale_elements));
    }
    if let Some(j) = &m.job {
        head.push_str(&format!(" · job {} {}/{}", j.stage, j.done, j.total));
    }
    let mut lines = vec![head];
    for p in &index.perspectives {
        let mut l = format!("{}  {}  ({} elements", p.id, p.title, p.elements);
        if p.stale > 0 {
            l.push_str(&format!(", {} stale", p.stale));
        }
        l.push(')');
        if p.origin != MapOrigin::Generated {
            l.push_str(&format!("  [{}]", p.origin.as_str()));
        }
        lines.push(l);
        for r in p.tree.iter().filter(|r| r.depth > 0) {
            let mut l = format!("{}{}", "  ".repeat(usize::from(r.depth)), r.name);
            if let Some(path) = &r.path {
                l.push_str(&format!("  {path}"));
            }
            if r.leaf {
                l.push_str("  leaf");
            } else {
                l.push_str(&format!("  ({} inside)", r.children));
            }
            if r.stale {
                l.push_str("  stale");
            }
            if r.origin == MapOrigin::Imported {
                l.push_str("  imported");
            }
            lines.push(l);
        }
    }
    if let Some(e) = element {
        lines.push(String::new());
        let crumbs: Vec<&str> = e.crumbs.iter().map(|(_, n)| n.as_str()).collect();
        let mut title = format!(
            "## {}",
            if crumbs.is_empty() {
                e.name.as_str()
            } else {
                ""
            }
        );
        if !crumbs.is_empty() {
            title.push_str(&crumbs.join(" › "));
        }
        title.push_str(&format!("  [{}]", e.id));
        if e.stale {
            title.push_str("  stale");
        }
        if e.origin == MapOrigin::Imported {
            title.push_str("  imported");
        }
        lines.push(title);
        if let Some(p) = &e.path {
            lines.push(p.clone());
        }
        if !e.fields.description.is_empty() {
            lines.push(e.fields.description.clone());
        }
        for (k, v) in [
            ("context", &e.fields.context),
            ("constraint", &e.fields.constraint),
            ("concern", &e.fields.concern),
            ("todo", &e.fields.todo),
            ("note", &e.fields.note),
        ] {
            if let Some(v) = v {
                lines.push(format!("{k}: {v}"));
            }
        }
        if !e.anchors.is_empty() {
            let shown: Vec<String> = e
                .anchors
                .iter()
                .take(8)
                .map(|a| match (a.line_start, a.line_end) {
                    (Some(s), Some(t)) => format!("{}#L{s}-L{t}", a.path),
                    (Some(s), None) => format!("{}#L{s}", a.path),
                    _ => a.path.clone(),
                } + if a.exists { "" } else { " (missing)" })
                .collect();
            let more = e.anchors.len().saturating_sub(8);
            lines.push(format!(
                "anchors: {}{}",
                shown.join(", "),
                if more > 0 {
                    format!(" +{more}")
                } else {
                    String::new()
                }
            ));
        }
        for c in &e.children {
            let mut l = format!("- {}  [{}]", c.name, c.id);
            if let Some(p) = &c.path {
                l.push_str(&format!("  {p}"));
            }
            if c.files > 0 {
                l.push_str(&format!("  {} files / {} lines", c.files, c.lines));
            }
            if c.leaf {
                l.push_str("  leaf");
            }
            if c.stale {
                l.push_str("  stale");
            }
            lines.push(l);
        }
        for ed in &e.edges {
            let why = ed
                .evidence
                .iter()
                .map(|ev| match ev {
                    super::MapEvidence::Manifest { kind, .. } => format!("manifest {kind}"),
                    super::MapEvidence::Imports { count, .. } => format!("{count} imports"),
                    super::MapEvidence::Refs { count, .. } => format!("{count} refs"),
                    super::MapEvidence::Call { site, .. } => format!("call {site}"),
                    super::MapEvidence::Bindings { generator, .. } => {
                        format!("bindings {generator}")
                    }
                    super::MapEvidence::Process { verb, site } => format!("{verb} {site}"),
                    super::MapEvidence::Inferred { reason } => format!("inferred: {reason}"),
                })
                .collect::<Vec<_>>()
                .join(", ");
            lines.push(format!(
                "{} → {} · {} · {}{}",
                ed.from,
                ed.to,
                ed.label,
                if ed.backed { "backed" } else { "inferred" },
                if why.is_empty() {
                    String::new()
                } else {
                    format!(" ({why})")
                }
            ));
        }
        if let Some(pv) = &e.provenance {
            lines.push(format!(
                "written by {}{} · {}{}",
                pv.model,
                pv.git_head
                    .as_deref()
                    .map(|h| format!(" at {h}"))
                    .unwrap_or_default(),
                ms_to_iso(pv.generated_ms),
                if pv.repaired { " · repaired" } else { "" }
            ));
        }
        if let Some(mm) = &e.mermaid {
            lines.push("```mermaid".to_owned());
            lines.extend(mm.lines().map(str::to_owned));
            lines.push("```".to_owned());
        }
    }
    let mut out = String::new();
    let mut kept = 0;
    for l in &lines {
        if kept > 0 && out.len() + l.len() + 1 > budget {
            break;
        }
        if kept > 0 {
            out.push('\n');
        }
        out.push_str(l);
        kept += 1;
    }
    let dropped = lines.len() - kept;
    if dropped > 0 {
        if lines[..kept]
            .iter()
            .filter(|l| l.starts_with("```"))
            .count()
            % 2
            == 1
        {
            out.push_str("\n```");
        }
        out.push_str(&format!("\n… +{dropped} lines"));
    }
    Ok((out, u32::try_from(dropped).unwrap_or(u32::MAX)))
}

#[cfg(test)]
mod tests {
    use std::path::PathBuf;
    use std::sync::Arc;

    use super::super::import::{import, is_ours};
    use super::*;
    use crate::index::RootIndex;
    use crate::{CodeindexConfig, index::Codeindex};

    struct TempDir(PathBuf);
    impl Drop for TempDir {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.0);
        }
    }
    fn tempdir() -> TempDir {
        let d = std::env::temp_dir().join(format!("theuth-omm-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir_all(&d).unwrap();
        TempDir(d)
    }
    fn copy_dir(from: &Path, to: &Path) {
        std::fs::create_dir_all(to).unwrap();
        for e in std::fs::read_dir(from).unwrap() {
            let e = e.unwrap();
            let dst = to.join(e.file_name());
            if e.file_type().unwrap().is_dir() {
                copy_dir(&e.path(), &dst);
            } else {
                std::fs::copy(e.path(), dst).unwrap();
            }
        }
    }

    /// The rust fixture indexed in a scratch data dir, plus an empty map pool beside it.
    async fn scratch() -> (TempDir, TempDir, Arc<RootIndex>, SqlitePool) {
        let data = tempdir();
        let repo = tempdir();
        copy_dir(
            &Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/rust"),
            &repo.0,
        );
        let ix = Arc::new(Codeindex::new(CodeindexConfig::default(), &data.0).unwrap());
        let root = repo.0.canonicalize().unwrap();
        let ri = ix.open_root(&root).await.unwrap();
        let r = ix.reconcile(&ri, true).await.unwrap();
        assert!(r.reparsed > 0, "{r:?}");
        let pool = store::open(&store::path(&data.0.join(crate::DIR), &root), &root)
            .await
            .unwrap();
        (data, repo, ri, pool)
    }

    fn graph(nodes: &[(&str, &str, &str)], edges: &[(&str, &str, &str, bool)]) -> GraphProps {
        serde_json::from_value(serde_json::json!({
            "nodes": nodes.iter().map(|(id, label, sub)| serde_json::json!({"id": id, "label": label, "sub": sub, "ref": sub})).collect::<Vec<_>>(),
            "edges": edges.iter().map(|(f, t, l, dashed)| serde_json::json!({"from": f, "to": t, "label": l, "style": if *dashed { "dashed" } else { "solid" }})).collect::<Vec<_>>(),
        }))
        .unwrap()
    }

    #[allow(clippy::too_many_arguments)]
    async fn element(
        pool: &SqlitePool,
        id: &str,
        node: &str,
        parent: Option<&str>,
        depth: i64,
        order: i64,
        name: &str,
        path: Option<&str>,
        kind: &str,
        diagram: Option<&GraphProps>,
        description: &str,
        context: Option<&str>,
    ) {
        sqlx::query(
            "INSERT INTO elements (id, perspective, node, parent, depth, order_no, name, path, kind, members, leaf, origin, stale, \
             facts_hash, diagram, description, context, model, generated_ms) \
             VALUES (?, 'overall-architecture', ?, ?, ?, ?, ?, ?, ?, '[]', ?, 'generated', 0, 'fh', ?, ?, ?, 'test-model', 1759946700000)",
        )
        .bind(id)
        .bind(node)
        .bind(parent)
        .bind(depth)
        .bind(order)
        .bind(name)
        .bind(path)
        .bind(kind)
        .bind(diagram.is_none())
        .bind(diagram.map(|g| serde_json::to_string(g).unwrap()))
        .bind(description)
        .bind(context)
        .execute(pool)
        .await
        .unwrap();
    }

    async fn edge(
        pool: &SqlitePool,
        element: &str,
        from: &str,
        to: &str,
        label: &str,
        backed: bool,
    ) {
        let ev = if backed {
            r#"[{"type":"manifest","from":"crates/b","to":"crates/a","kind":"cargo-path"}]"#
        } else {
            r#"[{"type":"inferred","reason":"test"}]"#
        };
        sqlx::query("INSERT INTO edges (element, from_node, to_node, label, backed, evidence) VALUES (?, ?, ?, ?, ?, ?)")
            .bind(element)
            .bind(from)
            .bind(to)
            .bind(label)
            .bind(backed)
            .bind(ev)
            .execute(pool)
            .await
            .unwrap();
    }

    /// A two-level generated map over the rust fixture: a perspective whose
    /// root draws two crates, crate `a` drawn as its two files, crate `b` a leaf.
    async fn seed(pool: &SqlitePool) {
        sqlx::query(
            "INSERT INTO perspectives (id, kind, title, origin, order_no, created_ms, updated_ms) \
             VALUES ('overall-architecture', 'overall-architecture', 'Overall architecture', 'generated', 1, 1, 1)",
        )
        .execute(pool)
        .await
        .unwrap();
        let top = graph(
            &[
                ("unit:crates/a", "Store crate", "crates/a"),
                ("unit:crates/b", "CLI", "crates/b"),
            ],
            &[("unit:crates/b", "unit:crates/a", "uses store", false)],
        );
        let a = graph(
            &[
                (
                    "unit:crates/a/src/lib.rs",
                    "Lib: facade",
                    "crates/a/src/lib.rs",
                ),
                (
                    "unit:crates/a/src/store.rs",
                    "Store",
                    "crates/a/src/store.rs",
                ),
            ],
            &[(
                "unit:crates/a/src/lib.rs",
                "unit:crates/a/src/store.rs",
                "re-exports",
                true,
            )],
        );
        element(
            pool,
            "overall-architecture",
            "perspective:overall-architecture",
            None,
            0,
            0,
            "Overall architecture",
            None,
            "perspective",
            Some(&top),
            "Two crates: a store and a CLI over it.",
            Some("Why: a workspace."),
        )
        .await;
        element(
            pool,
            "overall-architecture/unit:crates/a",
            "unit:crates/a",
            Some("overall-architecture"),
            1,
            0,
            "Store crate",
            Some("crates/a"),
            "unit",
            Some(&a),
            "The store, with a \"quoted\" word.",
            None,
        )
        .await;
        element(
            pool,
            "overall-architecture/unit:crates/b",
            "unit:crates/b",
            Some("overall-architecture"),
            1,
            1,
            "CLI",
            Some("crates/b"),
            "unit",
            None,
            "The binary.",
            None,
        )
        .await;
        element(
            pool,
            "overall-architecture/unit:crates/a/src/lib.rs",
            "unit:crates/a/src/lib.rs",
            Some("overall-architecture/unit:crates/a"),
            2,
            0,
            "Lib: facade",
            Some("crates/a/src/lib.rs"),
            "file",
            None,
            "",
            None,
        )
        .await;
        element(
            pool,
            "overall-architecture/unit:crates/a/src/store.rs",
            "unit:crates/a/src/store.rs",
            Some("overall-architecture/unit:crates/a"),
            2,
            1,
            "Store",
            Some("crates/a/src/store.rs"),
            "file",
            None,
            "",
            None,
        )
        .await;
        edge(
            pool,
            "overall-architecture",
            "unit:crates/b",
            "unit:crates/a",
            "uses store",
            true,
        )
        .await;
        edge(
            pool,
            "overall-architecture/unit:crates/a",
            "unit:crates/a/src/lib.rs",
            "unit:crates/a/src/store.rs",
            "re-exports",
            false,
        )
        .await;
    }

    async fn nodes_of(pool: &SqlitePool, perspective: &str) -> std::collections::BTreeSet<String> {
        sqlx::query("SELECT node FROM elements WHERE perspective = ?")
            .bind(perspective)
            .fetch_all(pool)
            .await
            .unwrap()
            .iter()
            .map(|r| r.get::<String, _>("node"))
            .collect()
    }

    async fn edges_of(
        pool: &SqlitePool,
        element: &str,
    ) -> std::collections::BTreeSet<(String, String, String)> {
        sqlx::query("SELECT from_node, to_node, label FROM edges WHERE element = ?")
            .bind(element)
            .fetch_all(pool)
            .await
            .unwrap()
            .iter()
            .map(|r| (r.get("from_node"), r.get("to_node"), r.get("label")))
            .collect()
    }

    #[tokio::test]
    async fn export_writes_omm_layout_that_reimports_equal() {
        let (_data, _repo, ri, pool) = scratch().await;
        seed(&pool).await;
        let out = write_omm(&pool, &ri.root, Some("abc1234")).await.unwrap();
        assert_eq!((out.elements, out.skipped.as_slice()), (5, &[][..]));
        let omm = ri.root.join(OMM_DIR);
        assert!(is_ours(&omm).unwrap());
        // omm's layout: dir per element named by the kebab-cased name, prose files, meta.yaml, diagram.mmd for groups only.
        let oa = omm.join("overall-architecture");
        for f in ["diagram.mmd", "description.md", "context.md", "meta.yaml"] {
            assert!(oa.join(f).is_file(), "{f}");
        }
        assert!(oa.join("store-crate/diagram.mmd").is_file());
        assert!(oa.join("store-crate/description.md").is_file());
        assert!(
            !oa.join("cli/diagram.mmd").exists(),
            "a leaf has no diagram"
        );
        assert!(oa.join("cli/meta.yaml").is_file());
        assert!(oa.join("store-crate/lib-facade/meta.yaml").is_file());
        assert!(
            !oa.join("store-crate/lib-facade/description.md").exists(),
            "empty prose is not written"
        );
        assert_eq!(out.files, 1 + 4 + 3 + 2 + 1 + 1);
        let meta = std::fs::read_to_string(oa.join("meta.yaml")).unwrap();
        for must in [
            "kind: perspective",
            "title: \"Overall architecture\"",
            "git_commit: abc1234",
            "children:\n  - \"store-crate\"\n  - \"cli\"",
            "parentPath: []",
            "theuth:\n  element: \"overall-architecture\"",
            "model: \"test-model\"",
            "backed_edges: 1",
            "last_field: diagram",
        ] {
            assert!(meta.contains(must), "meta.yaml lacks {must:?}:\n{meta}");
        }
        let nested = std::fs::read_to_string(oa.join("store-crate/meta.yaml")).unwrap();
        assert!(
            nested.contains("kind: nested-class")
                && nested.contains("parentPath:\n  - \"overall-architecture\""),
            "{nested}"
        );
        let mmd = std::fs::read_to_string(oa.join("diagram.mmd")).unwrap();
        assert_eq!(
            mmd,
            "flowchart LR\n    store-crate[\"Store crate\\ncrates/a\"]\n    cli[\"CLI\\ncrates/b\"]\n\n    cli -->|\"uses store\"| store-crate\n"
        );
        let mmd_a = std::fs::read_to_string(oa.join("store-crate/diagram.mmd")).unwrap();
        assert!(
            mmd_a.contains("lib-facade -.->|\"re-exports\"| store"),
            "inferred edges are dashed:\n{mmd_a}"
        );
        assert_eq!(
            std::fs::read_to_string(oa.join("store-crate/description.md")).unwrap(),
            "The store, with a \"quoted\" word.\n"
        );
        // Re-import: the same node and edge sets, under the imported perspective.
        let n = import(&ri, &pool).await.unwrap();
        assert_eq!(n, 5);
        let want: std::collections::BTreeSet<String> = [
            "perspective:imported:overall-architecture",
            "unit:crates/a",
            "unit:crates/b",
            "unit:crates/a/src/lib.rs",
            "unit:crates/a/src/store.rs",
        ]
        .iter()
        .map(|s| (*s).to_owned())
        .collect();
        assert_eq!(nodes_of(&pool, "imported:overall-architecture").await, want);
        let mut original = nodes_of(&pool, "overall-architecture").await;
        original.remove("perspective:overall-architecture");
        original.insert("perspective:imported:overall-architecture".into());
        assert_eq!(
            nodes_of(&pool, "imported:overall-architecture").await,
            original
        );
        assert_eq!(
            edges_of(&pool, "imported:overall-architecture").await,
            edges_of(&pool, "overall-architecture").await
        );
        assert_eq!(
            edges_of(&pool, "imported:overall-architecture/unit:crates/a").await,
            edges_of(&pool, "overall-architecture/unit:crates/a").await
        );
        let row = sqlx::query("SELECT name, path, description, git_head, model, diagram FROM elements WHERE id = 'imported:overall-architecture/unit:crates/a'")
            .fetch_one(&pool)
            .await
            .unwrap();
        assert_eq!(row.get::<String, _>("name"), "Store crate");
        assert_eq!(
            row.get::<Option<String>, _>("path").as_deref(),
            Some("crates/a")
        );
        assert_eq!(
            row.get::<String, _>("description"),
            "The store, with a \"quoted\" word."
        );
        assert_eq!(
            row.get::<Option<String>, _>("git_head").as_deref(),
            Some("abc1234")
        );
        assert_eq!(row.get::<String, _>("model"), "imported");
        let g: GraphProps = serde_json::from_str(&row.get::<String, _>("diagram")).unwrap();
        assert_eq!(
            g.nodes.iter().map(|n| n.id.as_str()).collect::<Vec<_>>(),
            ["unit:crates/a/src/lib.rs", "unit:crates/a/src/store.rs"]
        );
        assert_eq!(g.nodes[0].sub.as_deref(), Some("crates/a/src/lib.rs"));
        assert_eq!(g.edges[0].style, Some(GraphEdgeStyle::Dashed));
    }

    #[tokio::test]
    async fn export_refuses_foreign_omm() {
        let (_data, _repo, ri, pool) = scratch().await;
        seed(&pool).await;
        let omm = ri.root.join(OMM_DIR);
        std::fs::create_dir_all(omm.join("data-flow")).unwrap();
        std::fs::write(
            omm.join("config.yaml"),
            "version: 0.1.0\ncloud:\n  project_slug: x\n",
        )
        .unwrap();
        std::fs::write(omm.join("data-flow/description.md"), "theirs\n").unwrap();
        assert!(!is_ours(&omm).unwrap());
        let err = write_omm(&pool, &ri.root, None)
            .await
            .unwrap_err()
            .to_string();
        assert!(
            err.contains("not written by theuth") && err.contains("generator: theuth-147"),
            "{err}"
        );
        assert_eq!(
            std::fs::read_to_string(omm.join("data-flow/description.md")).unwrap(),
            "theirs\n",
            "the foreign tree is untouched"
        );
        // Ours → replaced wholesale; stale directories do not linger.
        std::fs::write(
            omm.join("config.yaml"),
            format!("version: \"0.1.0\"\ngenerator: {GENERATOR}\n"),
        )
        .unwrap();
        assert!(is_ours(&omm).unwrap());
        let out = write_omm(&pool, &ri.root, None).await.unwrap();
        assert_eq!(out.elements, 5);
        assert!(!omm.join("data-flow").exists());
        assert!(!is_ours(&ri.root.join("nowhere")).unwrap());
    }

    #[tokio::test]
    async fn imported_omm_is_read_only() {
        let (_data, _repo, ri, pool) = scratch().await;
        // A hand-written .omm/ without diagrams: titles from meta.yaml, prose copied, nodes free (`ext:`).
        let omm = ri.root.join(OMM_DIR);
        std::fs::create_dir_all(omm.join("data-flow/store")).unwrap();
        std::fs::write(omm.join("config.yaml"), "version: 0.1.0\n").unwrap();
        std::fs::write(omm.join("data-flow/meta.yaml"), "created: 2026-03-25T12:30:49.487Z\nupdated: 2026-03-25T12:30:57.358Z\nupdate_count: 2\nlast_field: description\ngit_commit: 2fb887a\nchildren:\n  - store\n").unwrap();
        std::fs::write(omm.join("data-flow/description.md"), "Where data goes.\n").unwrap();
        std::fs::write(omm.join("data-flow/constraint.md"), "One yaml dep.\n").unwrap();
        std::fs::write(omm.join("data-flow/store/meta.yaml"), "created: 2026-03-25T12:31:15.920Z\nupdated: 2026-03-25T12:31:15.921Z\nkind: nested-class\ntitle: store\nchildren: []\nparentPath: []\n").unwrap();
        std::fs::write(
            omm.join("data-flow/store/description.md"),
            "Central data engine.\n",
        )
        .unwrap();
        assert_eq!(import(&ri, &pool).await.unwrap(), 2);
        let p = sqlx::query(
            "SELECT kind, title, origin FROM perspectives WHERE id = 'imported:data-flow'",
        )
        .fetch_one(&pool)
        .await
        .unwrap();
        assert_eq!(
            (
                p.get::<String, _>("kind"),
                p.get::<String, _>("title"),
                p.get::<String, _>("origin")
            ),
            ("imported".into(), "Data flow".into(), "imported".into())
        );
        let rows = sqlx::query("SELECT id, node, name, parent, depth, leaf, stale, origin, model, git_head, generated_ms, description, constraint_ FROM elements WHERE perspective = 'imported:data-flow' ORDER BY depth")
            .fetch_all(&pool)
            .await
            .unwrap();
        assert_eq!(rows.len(), 2);
        assert_eq!(rows[0].get::<String, _>("id"), "imported:data-flow");
        assert_eq!(
            rows[0].get::<String, _>("node"),
            "perspective:imported:data-flow"
        );
        assert_eq!(rows[0].get::<String, _>("description"), "Where data goes.");
        assert_eq!(
            rows[0].get::<Option<String>, _>("constraint_").as_deref(),
            Some("One yaml dep.")
        );
        assert_eq!(
            rows[0].get::<Option<String>, _>("git_head").as_deref(),
            Some("2fb887a")
        );
        assert_eq!(rows[0].get::<i64, _>("generated_ms"), 1_774_441_857_358);
        assert_eq!(
            rows[1].get::<String, _>("id"),
            "imported:data-flow/ext:store"
        );
        assert_eq!(rows[1].get::<String, _>("name"), "store");
        assert_eq!(
            rows[1].get::<Option<String>, _>("parent").as_deref(),
            Some("imported:data-flow")
        );
        for r in &rows {
            assert!(r.get::<bool, _>("leaf") && !r.get::<bool, _>("stale"));
            assert_eq!(
                (r.get::<String, _>("origin"), r.get::<String, _>("model")),
                ("imported".into(), "imported".into())
            );
        }
        // Read-only: never exported (and the export refuses to overwrite a foreign .omm/ anyway) …
        let err = write_omm(&pool, &ri.root, None)
            .await
            .unwrap_err()
            .to_string();
        assert!(err.contains("not written by theuth"), "{err}");
        std::fs::remove_dir_all(&omm).unwrap();
        let out = write_omm(&pool, &ri.root, None).await.unwrap();
        assert_eq!(out.elements, 0);
        assert_eq!(
            out.skipped,
            ["imported:data-flow: imported perspective (read-only)"]
        );
        assert!(
            !ri.root.join(OMM_DIR).join("imported:data-flow").exists()
                && !ri.root.join(OMM_DIR).join("data-flow").exists()
        );
        // … and a later import replaces, never duplicates; a vanished .omm/ clears it.
        assert_eq!(import(&ri, &pool).await.unwrap(), 0);
        assert!(nodes_of(&pool, "imported:data-flow").await.is_empty());
        let n: i64 =
            sqlx::query_scalar("SELECT COUNT(*) FROM perspectives WHERE origin = 'imported'")
                .fetch_one(&pool)
                .await
                .unwrap();
        assert_eq!(n, 0);
    }

    #[test]
    fn code_overview_text_shows_the_perspective_tree() {
        let index: MapIndex = serde_json::from_str(include_str!(concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/../desktop/ui/src/components/codemap/__fixtures__/map-overview.json"
        )))
        .unwrap();
        let element: MapElement = serde_json::from_str(include_str!(concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/../desktop/ui/src/components/codemap/__fixtures__/map-element-content-stores.json"
        )))
        .unwrap();
        let (text, dropped) = code_overview_text(&index, None, 1 << 20).unwrap();
        assert_eq!(dropped, 0);
        let lines: Vec<&str> = text.lines().collect();
        assert!(
            lines[0].contains("ready") && lines[0].contains("2 perspectives"),
            "{}",
            lines[0]
        );
        assert!(
            lines[1].starts_with("overall-architecture  Overall architecture  ("),
            "{}",
            lines[1]
        );
        assert!(
            lines
                .iter()
                .any(|l| l.starts_with("  RPC facade  crates/rpc  leaf")),
            "{text}"
        );
        assert!(
            lines.iter().any(|l| l.starts_with("request-lifecycle  ")),
            "{text}"
        );
        let deepest = index
            .perspectives
            .iter()
            .flat_map(|p| &p.tree)
            .map(|r| r.depth)
            .max()
            .unwrap();
        assert!(
            lines
                .iter()
                .any(|l| l.starts_with(&"  ".repeat(usize::from(deepest)))
                    && !l.starts_with(&"  ".repeat(usize::from(deepest) + 1))),
            "depth {deepest} indented"
        );
        assert!(!text.contains("```"));
        let (text, dropped) = code_overview_text(&index, Some(&element), 1 << 20).unwrap();
        assert_eq!(dropped, 0);
        assert!(text.contains("\n## Overall architecture › Content stores  [overall-architecture/cluster:26b565b0]\n"), "{text}");
        assert!(text.contains("unit:crates/knowledge → unit:crates/document · concepts are block documents · backed (manifest cargo-path)"), "{text}");
        assert!(text.contains("- Document authority  [overall-architecture/unit:crates/document]  crates/document  15 files"), "{text}");
        assert!(
            text.contains("written by anthropic/claude-opus-4 at 5cddad8e · 2025-10-08T"),
            "{text}"
        );
        assert!(
            text.contains("```mermaid\n") && text.trim_end().ends_with("```"),
            "{text}"
        );
        // A tight budget cuts on a line boundary, counts the rest and closes an open fence.
        let full_lines = text.lines().count();
        let (cut, dropped) = code_overview_text(&index, Some(&element), text.len() - 20).unwrap();
        assert!(dropped > 0 && (dropped as usize) < full_lines);
        assert!(
            cut.ends_with(&format!("\n```\n… +{dropped} lines")),
            "{cut}"
        );
    }
}
