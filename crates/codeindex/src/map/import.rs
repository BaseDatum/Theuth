//! Import of an existing `<root>/.omm/` (147 §4.6, §13.2): read at skeleton
//! time into `imported:<dir>` perspectives with `origin: Imported`; every
//! `diagram.mmd` through `surface::graph::from_mermaid` with any ids allowed,
//! node labels' second line matched against `files.path` (hit → `unit:`
//! anchor, miss → `exists: false`), fields copied, `meta.yaml`
//! `git_commit`/`updated` into provenance (`model: "imported"`). Read-only:
//! never regenerated, exported or marked stale.
//!
//! owner: W4 — design §4.6, §13.2

use std::collections::{BTreeMap, HashSet};
use std::path::Path;

use anyhow::Context as _;
use chrono::{DateTime, NaiveDateTime, Utc};
use sqlx::{SqliteConnection, SqlitePool};
use surface::graph::from_mermaid;

use super::export::OMM_DIR;
use super::{MapEvidence, MapFields, MapOrigin, element_id};
use crate::index::RootIndex;

/// omm's prose files, in reading order (§4.3), and the `elements` column each lands in.
pub(super) const FIELD_FILES: [(&str, &str); 6] = [
    ("description.md", "description"),
    ("context.md", "context"),
    ("constraint.md", "constraint_"),
    ("concern.md", "concern"),
    ("todo.md", "todo"),
    ("note.md", "note"),
];

/// One `.omm/` directory as read from disk.
struct OmmElement {
    /// Directory name (omm's node id in the parent diagram).
    dir: String,
    /// Path under `.omm/`, `/`-joined.
    rel: String,
    /// `meta.yaml` `title`, when present.
    title: Option<String>,
    git_commit: Option<String>,
    updated_ms: Option<u64>,
    fields: MapFields,
    diagram: Option<String>,
    children: Vec<OmmElement>,
}

/// Whether `omm_dir`'s `config.yaml` carries our `generator:` key (§10.2's
/// export refusal: a foreign `.omm/` is never overwritten in v0). A missing
/// directory or config is not ours.
pub fn is_ours(omm_dir: &Path) -> anyhow::Result<bool> {
    let cfg = omm_dir.join("config.yaml");
    if !cfg.is_file() {
        return Ok(false);
    }
    let text = std::fs::read_to_string(&cfg).with_context(|| format!("read {}", cfg.display()))?;
    Ok(yaml_scalar(&text, "generator").is_some_and(|g| g.starts_with("theuth")))
}

/// Read `<root>/.omm/` (when present) into `pool`'s `imported:<dir>`
/// perspectives, replacing the previous import. Returns the elements
/// imported (perspective roots included); 0 when there is no `.omm/`.
/// Called by `skeleton::recompute`.
pub async fn import(ri: &RootIndex, pool: &SqlitePool) -> anyhow::Result<u32> {
    let omm = ri.root.join(OMM_DIR);
    let perspectives = if omm.is_dir() {
        read_children(&omm, "")?
    } else {
        Vec::new()
    };
    let mut tx = pool.begin().await?;
    for sql in [
        "DELETE FROM edges WHERE element IN (SELECT id FROM elements WHERE perspective LIKE 'imported:%')",
        "DELETE FROM anchors WHERE element IN (SELECT id FROM elements WHERE perspective LIKE 'imported:%')",
        "DELETE FROM elements WHERE perspective LIKE 'imported:%'",
        "DELETE FROM perspectives WHERE origin = 'imported'",
    ] {
        sqlx::query(sql).execute(&mut *tx).await?;
    }
    let mut order: i64 =
        sqlx::query_scalar("SELECT COALESCE(MAX(order_no), 0) + 1 FROM perspectives")
            .fetch_one(&mut *tx)
            .await?;
    let now = u64::try_from(super::store::now_ms()).unwrap_or(0);
    let mut count = 0;
    for p in &perspectives {
        let pid = format!("imported:{}", p.dir);
        let title = p.title.clone().unwrap_or_else(|| humanize(&p.dir));
        sqlx::query(
            "INSERT INTO perspectives (id, kind, title, origin, why, order_no, eligible, created_ms, updated_ms) \
             VALUES (?, 'imported', ?, 'imported', ?, ?, NULL, ?, ?)",
        )
        .bind(&pid)
        .bind(&title)
        .bind(format!("read from {OMM_DIR}/{}", p.dir))
        .bind(order)
        .bind(i64::try_from(now).unwrap_or(i64::MAX))
        .bind(i64::try_from(now).unwrap_or(i64::MAX))
        .execute(&mut *tx)
        .await?;
        order += 1;
        let mut taken = HashSet::new();
        let root_node = format!("perspective:{pid}");
        let mut w = Writer {
            ri,
            tx: &mut tx,
            pid: &pid,
            taken: &mut taken,
            now,
            count: 0,
        };
        w.element(p, &root_node, title, None, None, 0, 0).await?;
        count += w.count;
    }
    tx.commit().await?;
    Ok(count)
}

struct Writer<'a> {
    ri: &'a RootIndex,
    tx: &'a mut SqliteConnection,
    pid: &'a str,
    /// Node ids used in this perspective (element ids must be unique).
    taken: &'a mut HashSet<String>,
    now: u64,
    count: u32,
}

/// A diagram node's label split into omm's two lines: name, then the path or hint.
fn split_label(label: &str) -> (String, Option<String>) {
    let norm = label
        .replace("\\n", "\n")
        .replace("<br/>", "\n")
        .replace("<br>", "\n");
    let mut lines = norm.lines().map(str::trim).filter(|l| !l.is_empty());
    let name = lines.next().unwrap_or("").to_owned();
    let second = lines.collect::<Vec<_>>().join(" ");
    (name, (!second.is_empty()).then_some(second))
}

/// A label's second line that reads as a root-relative path: no spaces, a `/` or a `.`.
fn path_like(s: &str) -> Option<String> {
    let s = s.trim_end_matches('/');
    (!s.is_empty()
        && !s.contains(char::is_whitespace)
        && (s.contains('/') || s.contains('.'))
        && !s.starts_with('~')
        && !s.contains("://"))
    .then(|| s.to_owned())
}

impl Writer<'_> {
    /// `path` (a file or a directory prefix) is in `files` now.
    async fn indexed(&mut self, path: &str) -> anyhow::Result<bool> {
        let hit: Option<i64> = sqlx::query_scalar(
            "SELECT 1 FROM files WHERE path = ?1 OR (path > ?1 || '/' AND path < ?1 || '0') LIMIT 1",
        )
        .bind(path)
        .fetch_optional(&self.ri.pool)
        .await?;
        Ok(hit.is_some())
    }

    /// Node id for a diagram node: `unit:<path>` when its label path is indexed, else `ext:<slug>`.
    async fn node_id(&mut self, slug: &str, path: Option<&str>) -> anyhow::Result<String> {
        if let Some(p) = path
            && self.indexed(p).await?
        {
            return Ok(format!("unit:{p}"));
        }
        Ok(format!("ext:{slug}"))
    }

    /// Write `e` and, recursively, its children. `node`/`name`/`path` were
    /// decided by the parent's diagram (or the perspective for the root).
    #[allow(clippy::too_many_arguments)]
    async fn element(
        &mut self,
        e: &OmmElement,
        node: &str,
        name: String,
        path: Option<String>,
        parent: Option<&str>,
        depth: u8,
        order_no: i64,
    ) -> anyhow::Result<()> {
        let id = if depth == 0 {
            self.pid.to_owned()
        } else {
            element_id(self.pid, node)
        };
        // Diagram: parse, then remap omm's slug ids to ours and decide each child's node.
        let mut child_nodes: BTreeMap<String, (String, String, Option<String>)> = BTreeMap::new();
        let mut diagram = None;
        let mut edges: BTreeMap<(String, String), (String, bool)> = BTreeMap::new();
        let mut error = None;
        if let Some(src) = &e.diagram {
            match from_mermaid(src, None) {
                Ok(mut g) => {
                    let mut ids: BTreeMap<String, String> = BTreeMap::new();
                    for n in &mut g.nodes {
                        let (label, second) = split_label(&n.label);
                        let p = second.as_deref().and_then(path_like);
                        let mut nid = self.node_id(&n.id, p.as_deref()).await?;
                        if e.children.iter().any(|c| c.dir == n.id) {
                            if !self.taken.insert(nid.clone()) {
                                nid = format!("ext:{}/{}", e.rel, n.id).replace("//", "/");
                                self.taken.insert(nid.clone());
                            }
                            child_nodes
                                .insert(n.id.clone(), (nid.clone(), label.clone(), p.clone()));
                        }
                        ids.insert(n.id.clone(), nid.clone());
                        n.id = nid;
                        n.label = platform::text::ellipsize(
                            if label.is_empty() { &n.id } else { &label },
                            80,
                        )
                        .into_owned();
                        n.sub = second
                            .as_deref()
                            .map(|s| platform::text::ellipsize(s, 40).into_owned());
                        n.r#ref = p;
                    }
                    for ed in &mut g.edges {
                        if let Some(f) = ids.get(&ed.from) {
                            ed.from = f.clone();
                        }
                        if let Some(t) = ids.get(&ed.to) {
                            ed.to = t.clone();
                        }
                        let dashed = ed.style == Some(surface::GraphEdgeStyle::Dashed);
                        let label = ed.label.clone().unwrap_or_default();
                        edges
                            .entry((ed.from.clone(), ed.to.clone()))
                            .and_modify(|(l, d)| {
                                if !label.is_empty() && !l.contains(&label) {
                                    if !l.is_empty() {
                                        l.push_str(" · ");
                                    }
                                    l.push_str(&label);
                                }
                                *d &= dashed;
                            })
                            .or_insert((label, dashed));
                    }
                    diagram = Some(g);
                }
                Err(err) => {
                    tracing::warn!(element = %id, "codemap import: {err}");
                    error = Some(err.to_string());
                }
            }
        }
        let leaf = diagram.is_none();
        let kind = if depth == 0 {
            "perspective"
        } else if node.starts_with("unit:") {
            "unit"
        } else {
            "external"
        };
        let generated_ms = e.updated_ms.unwrap_or(self.now);
        sqlx::query(
            "INSERT INTO elements (id, perspective, node, parent, depth, order_no, name, path, kind, members, leaf, origin, \
             stale, facts_hash, diagram, description, context, constraint_, concern, todo, note, model, git_head, generated_ms, \
             repaired, error) \
             VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, '[]', ?, ?, 0, NULL, ?, ?, ?, ?, ?, ?, ?, 'imported', ?, ?, 0, ?)",
        )
        .bind(&id)
        .bind(self.pid)
        .bind(node)
        .bind(parent)
        .bind(i64::from(depth))
        .bind(order_no)
        .bind(&name)
        .bind(&path)
        .bind(kind)
        .bind(leaf)
        .bind(MapOrigin::Imported.as_str())
        .bind(diagram.as_ref().map(|g| serde_json::to_string(g).unwrap_or_default()))
        .bind(&e.fields.description)
        .bind(&e.fields.context)
        .bind(&e.fields.constraint)
        .bind(&e.fields.concern)
        .bind(&e.fields.todo)
        .bind(&e.fields.note)
        .bind(&e.git_commit)
        .bind(i64::try_from(generated_ms).unwrap_or(i64::MAX))
        .bind(&error)
        .execute(&mut *self.tx)
        .await?;
        if let Some(p) = &path {
            sqlx::query("INSERT OR IGNORE INTO anchors (element, path, line_start, line_end) VALUES (?, ?, NULL, NULL)")
                .bind(&id)
                .bind(p)
                .execute(&mut *self.tx)
                .await?;
        }
        let evidence = serde_json::to_string(&[MapEvidence::Inferred {
            reason: format!("imported from {OMM_DIR}/{}/diagram.mmd", e.rel),
        }])?;
        for ((from, to), (label, _)) in &edges {
            sqlx::query(
                "INSERT OR IGNORE INTO edges (element, from_node, to_node, label, backed, evidence) VALUES (?, ?, ?, ?, 0, ?)",
            )
            .bind(&id)
            .bind(from)
            .bind(to)
            .bind(label)
            .bind(&evidence)
            .execute(&mut *self.tx)
            .await?;
        }
        self.count += 1;
        // Children: diagram order first, then directories the diagram does not draw.
        let mut ordered: Vec<&OmmElement> = Vec::with_capacity(e.children.len());
        if let Some(g) = &diagram {
            for n in &g.nodes {
                let slug = child_nodes
                    .iter()
                    .find(|(_, (nid, _, _))| *nid == n.id)
                    .map(|(s, _)| s.clone());
                if let Some(c) = slug.and_then(|s| e.children.iter().find(|c| c.dir == s)) {
                    ordered.push(c);
                }
            }
        }
        for c in &e.children {
            if !ordered.iter().any(|o| std::ptr::eq(*o, c)) {
                ordered.push(c);
            }
        }
        for (i, c) in ordered.into_iter().enumerate() {
            let (cnode, cname, cpath) = match child_nodes.get(&c.dir) {
                Some((n, l, p)) => (n.clone(), l.clone(), p.clone()),
                None => {
                    let mut n = format!("ext:{}", c.dir);
                    if !self.taken.insert(n.clone()) {
                        n = format!("ext:{}/{}", e.rel, c.dir).replace("//", "/");
                        self.taken.insert(n.clone());
                    }
                    (n, c.title.clone().unwrap_or_else(|| humanize(&c.dir)), None)
                }
            };
            let cname = if cname.is_empty() {
                c.title.clone().unwrap_or_else(|| humanize(&c.dir))
            } else {
                cname
            };
            Box::pin(self.element(c, &cnode, cname, cpath, Some(&id), depth + 1, i as i64)).await?;
        }
        Ok(())
    }
}

/// The element directories under `dir`, sorted by name, each read recursively.
fn read_children(dir: &Path, rel: &str) -> anyhow::Result<Vec<OmmElement>> {
    let mut out = Vec::new();
    let mut names: Vec<String> = std::fs::read_dir(dir)
        .with_context(|| format!("read {}", dir.display()))?
        .filter_map(Result::ok)
        .filter(|e| e.file_type().is_ok_and(|t| t.is_dir()))
        .filter_map(|e| e.file_name().into_string().ok())
        .filter(|n| !n.starts_with('.'))
        .collect();
    names.sort();
    for name in names {
        let d = dir.join(&name);
        if !is_element_dir(&d) {
            continue;
        }
        let crel = if rel.is_empty() {
            name.clone()
        } else {
            format!("{rel}/{name}")
        };
        let meta = std::fs::read_to_string(d.join("meta.yaml")).unwrap_or_default();
        let mut fields = MapFields::default();
        for (file, column) in FIELD_FILES {
            let Ok(text) = std::fs::read_to_string(d.join(file)) else {
                continue;
            };
            let text = text.trim().to_owned();
            if text.is_empty() {
                continue;
            }
            match column {
                "description" => fields.description = text,
                "context" => fields.context = Some(text),
                "constraint_" => fields.constraint = Some(text),
                "concern" => fields.concern = Some(text),
                "todo" => fields.todo = Some(text),
                _ => fields.note = Some(text),
            }
        }
        out.push(OmmElement {
            title: yaml_scalar(&meta, "title").map(str::to_owned),
            git_commit: yaml_scalar(&meta, "git_commit").map(str::to_owned),
            updated_ms: yaml_scalar(&meta, "updated").and_then(iso_to_ms),
            fields,
            diagram: std::fs::read_to_string(d.join("diagram.mmd"))
                .ok()
                .filter(|s| !s.trim().is_empty()),
            children: read_children(&d, &crel)?,
            dir: name,
            rel: crel,
        });
    }
    Ok(out)
}

/// An element directory holds at least one of omm's files.
fn is_element_dir(d: &Path) -> bool {
    d.join("meta.yaml").is_file()
        || d.join("diagram.mmd").is_file()
        || FIELD_FILES.iter().any(|(f, _)| d.join(f).is_file())
}

/// `kebab-case-dir` → `Kebab case dir` (the first char, whatever its width).
fn humanize(dir: &str) -> String {
    let s = dir.replace(['-', '_'], " ");
    let mut chars = s.chars();
    match chars.next() {
        Some(c) => c.to_uppercase().chain(chars).collect(),
        None => s,
    }
}

/// A top-level `key: value` scalar of a flat YAML document; `None` for a
/// missing key, an empty value or a block scalar (`|`, `>`). Quotes stripped.
pub(super) fn yaml_scalar<'a>(text: &'a str, key: &str) -> Option<&'a str> {
    text.lines().find_map(|line| {
        let rest = line.strip_prefix(key)?.strip_prefix(':')?;
        let v = rest.trim();
        if v.is_empty() || v == "|" || v == ">" || v.starts_with("#") {
            return None;
        }
        Some(v.trim_matches(|c| c == '"' || c == '\''))
    })
}

/// `YYYY-MM-DDTHH:MM(:SS)?(.fff)?Z?` → Unix ms (UTC; a missing `Z` is still UTC).
pub(super) fn iso_to_ms(s: &str) -> Option<u64> {
    let s = s.trim().trim_end_matches('Z');
    let at = ["%Y-%m-%dT%H:%M:%S%.f", "%Y-%m-%dT%H:%M%.f"]
        .iter()
        .find_map(|f| NaiveDateTime::parse_from_str(s, f).ok())?;
    u64::try_from(at.and_utc().timestamp_millis()).ok()
}

/// Unix ms → `YYYY-MM-DDTHH:MM:SS.fffZ`.
pub(super) fn ms_to_iso(ms: u64) -> String {
    i64::try_from(ms)
        .ok()
        .and_then(DateTime::<Utc>::from_timestamp_millis)
        .unwrap_or(DateTime::UNIX_EPOCH)
        .format("%Y-%m-%dT%H:%M:%S%.3fZ")
        .to_string()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn iso_round_trips() {
        for s in [
            "2026-03-25T12:30:49.487Z",
            "1970-01-01T00:00:00.000Z",
            "2000-02-29T23:59:59.999Z",
        ] {
            assert_eq!(ms_to_iso(iso_to_ms(s).unwrap()), s);
        }
        assert_eq!(iso_to_ms("2026-03-25T12:30:49Z"), Some(1_774_441_849_000));
        assert_eq!(iso_to_ms("garbage"), None);
    }

    #[test]
    fn yaml_scalars_and_labels() {
        let y = "created: 2026-03-25T12:30:49.487Z\ngit_commit: 2fb887a\nkind: nested-class\ntitle: 'store'\nprev_diagram: |\n  graph LR\nchildren:\n  - a\n";
        assert_eq!(yaml_scalar(y, "git_commit"), Some("2fb887a"));
        assert_eq!(yaml_scalar(y, "title"), Some("store"));
        assert_eq!(yaml_scalar(y, "prev_diagram"), None);
        assert_eq!(yaml_scalar(y, "children"), None);
        assert_eq!(yaml_scalar(y, "missing"), None);
        assert_eq!(
            split_label("Store Engine\\nsrc/lib/store.ts"),
            ("Store Engine".into(), Some("src/lib/store.ts".into()))
        );
        assert_eq!(
            split_label("Web Viewer<br/>localhost:3000").1,
            Some("localhost:3000".into())
        );
        assert_eq!(path_like("src/commands/"), Some("src/commands".into()));
        assert_eq!(path_like("Claude Code / Codex / Cursor"), None);
        assert_eq!(path_like("~/.omm/credentials.json"), None);
        assert_eq!(humanize("overall-architecture"), "Overall architecture");
        assert_eq!(humanize("élan_vital"), "Élan vital");
        assert_eq!(humanize(""), "");
    }
}
