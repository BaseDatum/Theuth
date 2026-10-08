//! The facts the generator hands the model (147 §6.2 inputs), read off W1's
//! [`Skeleton`] so the unit set, the `unit:<path>` ids, the edges and the
//! facts hash the model works with are exactly the structure rows the panel
//! shows and `recompute` maintains. This file adds what the stages need on
//! top: the drill-down below one element capped for a diagram, the evidence
//! joining two member sets, top-ranked signatures, concern triggers and the
//! entry-point call chains of stage D.
//!
//! owner: W2 — design §6.2 (as consumed by §6), §5.5

use std::collections::BTreeSet;
use std::path::PathBuf;

use serde::Serialize;
use sqlx::SqlitePool;

use super::detect::{Detection, PERSPECTIVES};
use super::skeleton::{self, Child, LEAF_FILES, Measures, REF_MIN_CONFIDENCE, Skeleton};
use super::{DIAGRAM_NODES, DIAGRAM_NODES_HARD, MapEdge, MapEvidence};
use crate::index::RootIndex;
use crate::store;

/// Top-ranked signatures per unit the model sees (§5.1).
pub const TOP_SYMBOLS: usize = 5;
/// `symbols.signature` cut for the facts.
pub const SIGNATURE_CHARS: usize = 200;
/// Call-chain depth for stage D.
pub const FLOW_DEPTH: usize = 6;
/// Call sites per chain for stage D.
pub const FLOW_SITES: usize = 30;
/// Callees followed per symbol when walking a chain.
const FLOW_FANOUT: i64 = 6;
/// Entry candidates examined before keeping the longest chains.
const FLOW_CANDIDATES: usize = 8;
/// Files over this many lines trip a `concern` (§4.3).
pub const CONCERN_FILE_LINES: i64 = 5_000;
/// In-degree over this trips a `concern` on a non-foundation unit (§4.3).
pub const CONCERN_IN_DEGREE: usize = 15;

/// The index-decided structure below one element, capped for a diagram.
#[derive(Clone, Debug, Default)]
pub struct SubSkeleton {
    /// The drawn children, by share.
    pub children: Vec<Child>,
    /// The remainder, counted into one bundle.
    pub bundled: Vec<Child>,
    /// Edges among `children` (W1's aggregation and mechanical labels).
    pub edges: Vec<MapEdge>,
    /// Edges folded by the §5.2 threshold.
    pub minor: u32,
}

/// One symbol on a call chain.
#[derive(Clone, Debug, Serialize)]
pub struct FlowSymbol {
    /// `sym:<moniker>`.
    pub node: String,
    /// `symbols.moniker`.
    pub moniker: String,
    /// Name.
    pub name: String,
    /// Root-relative path.
    pub path: String,
    /// 1-based.
    pub line_start: i64,
    /// 1-based, inclusive.
    pub line_end: i64,
    /// Signature, cut to [`SIGNATURE_CHARS`].
    pub signature: String,
}

/// One call site on a chain.
#[derive(Clone, Debug, Serialize)]
pub struct CallSite {
    /// `path#L`.
    pub site: String,
    /// Caller `sym:` node.
    pub from: String,
    /// Callee `sym:` node.
    pub to: String,
}

/// One entry point's chain (stage D input).
#[derive(Clone, Debug)]
pub struct Flow {
    /// The entry symbol (first of `symbols`).
    pub entry: FlowSymbol,
    /// Distinct symbols reached, entry first.
    pub symbols: Vec<FlowSymbol>,
    /// Call sites in walk order, ≤ [`FLOW_SITES`].
    pub calls: Vec<CallSite>,
}

/// Everything the stages read; loaded once per job.
pub struct Facts {
    /// W1's skeleton: files, edges, members, detectors.
    pub sk: Skeleton,
    /// Canonical root.
    pub root: PathBuf,
    /// `profile::render` of the root.
    pub profile: String,
    /// `map_state` as `"<files>:<max mtime_ns>"`.
    pub index_state: String,
    /// `meta.git_head`.
    pub git_head: Option<String>,
    /// The twelve detectors, `overall-architecture` always eligible.
    pub detections: Vec<Detection>,
    /// `(title, description)` of imported perspectives (§13.2).
    pub imported: Vec<(String, String)>,
    /// The top level (§5.1): the units the model partitions.
    pub top: Vec<Child>,
    /// Edges among `top` after W1's thresholding (§5.2).
    pub edges: Vec<MapEdge>,
    /// Edges folded into "+N minor edges".
    pub folded: u32,
}

impl Facts {
    /// Read the skeleton of `ri` and the map store's imported perspectives.
    pub async fn load(ri: &RootIndex, map: &SqlitePool) -> anyhow::Result<Self> {
        let sk = Skeleton::load(ri).await?;
        let (count, max_mtime) = store::map_state(&ri.pool).await?;
        let git_head = store::meta(&ri.pool, "git_head").await?;
        let imported: Vec<(String, Option<String>)> = sqlx::query_as(
            "SELECT p.title, (SELECT e.description FROM elements e WHERE e.id = p.id) FROM perspectives p \
             WHERE p.origin = 'imported' ORDER BY p.order_no",
        )
        .fetch_all(map)
        .await
        .unwrap_or_default();
        let top = sk.top_level();
        let (edges, folded) = sk.edges_between(&top);
        Ok(Self {
            detections: with_architecture(sk.detections.clone()),
            sk,
            root: ri.root.clone(),
            profile: crate::profile::render(&crate::profile::detect(&ri.root)),
            index_state: format!("{count}:{max_mtime}"),
            git_head,
            imported: imported
                .into_iter()
                .map(|(t, d)| (t, d.unwrap_or_default()))
                .collect(),
            top,
            edges,
            folded,
        })
    }

    /// §5.1 measures of a child.
    pub fn measures(&self, c: &Child) -> Measures {
        self.sk.measures(&c.files)
    }

    /// A top-level unit by node id.
    pub fn top_unit(&self, node: &str) -> Option<&Child> {
        self.top.iter().find(|c| c.node == node)
    }

    /// A model-made group as the skeleton sees it (W1's `model_child`): a
    /// single `unit:` by its path, a cluster or bundle through its members'
    /// files. `None` when nothing of it exists in the index any more.
    pub fn group_child(&self, node: &str, name: &str, members: &[String]) -> Option<Child> {
        if let Some(c) = self.sk.child_of_node(node) {
            return Some(Child {
                name: name.to_owned(),
                ..c
            });
        }
        if members.is_empty() {
            return None;
        }
        let mut files: Vec<usize> = members
            .iter()
            .filter_map(|m| m.strip_prefix("unit:"))
            .flat_map(|p| self.sk.files_under(p))
            .collect();
        files.sort_unstable();
        files.dedup();
        let path = members
            .iter()
            .filter_map(|m| m.strip_prefix("unit:"))
            .map(str::to_owned)
            .reduce(|a, b| common_prefix_dir(&a, &b))
            .unwrap_or_default();
        Some(Child {
            node: node.to_owned(),
            name: name.to_owned(),
            path,
            kind: "unit",
            files,
            member: false,
        })
    }

    /// The children below one element (§5.5): a cluster's or bundle's are its
    /// members, a unit's are W1's `children_of`; `kind` is the element's.
    pub fn children_for(&self, kind: &str, path: Option<&str>, members: &[String]) -> Vec<Child> {
        match kind {
            "cluster" | "bundle" => {
                let mut kids: Vec<Child> = members
                    .iter()
                    .filter_map(|m| self.sk.child_of_node(m))
                    .collect();
                kids.sort_by(|a, b| {
                    let (sa, sb) = (self.measures(a).share, self.measures(b).share);
                    sb.total_cmp(&sa).then_with(|| a.path.cmp(&b.path))
                });
                kids
            }
            "unit" => path.map(|p| self.sk.children_of(p)).unwrap_or_default(),
            _ => Vec::new(),
        }
    }

    /// Cap `children` at `cap` (the rest is the bundle) and take W1's edges
    /// among the kept ones — the same numbers `recompute` writes.
    pub fn sub_skeleton(&self, mut children: Vec<Child>, cap: usize) -> SubSkeleton {
        let bundled = if children.len() > cap {
            children.split_off(cap)
        } else {
            Vec::new()
        };
        let (edges, minor) = if children.is_empty() {
            (Vec::new(), 0)
        } else {
            self.sk.edges_between(&children)
        };
        SubSkeleton {
            children,
            bundled,
            edges,
            minor,
        }
    }

    /// The diagram cap for an element: the model's own partitions keep up to
    /// the hard cap (W1 applies the same rule to generated rows).
    pub fn cap(generated: bool) -> usize {
        if generated {
            DIAGRAM_NODES_HARD
        } else {
            DIAGRAM_NODES
        }
    }

    /// A unit of ≤ [`LEAF_FILES`] parsed files is a leaf (§5.1 item 4).
    pub fn is_leaf(c: &Child) -> bool {
        c.kind == "file" || c.files.len() <= LEAF_FILES as usize
    }

    /// The evidence the index holds from `a` to `b` (a group edge's `backed`).
    pub fn evidence_between(&self, a: &Child, b: &Child) -> Vec<MapEvidence> {
        let (edges, _) = self.sk.edges_between(&[a.clone(), b.clone()]);
        edges
            .into_iter()
            .find(|e| e.from == a.node && e.to == b.node)
            .map(|e| e.evidence)
            .unwrap_or_default()
    }

    /// §5.4 exactly as `recompute` stamps it: the files of the element and
    /// `edge_facts` of its diagram's edges.
    pub fn facts_hash(&self, files: &[usize], edges: &[MapEdge]) -> String {
        let facts: Vec<(String, i64, i64)> = files
            .iter()
            .map(|&i| {
                let f = &self.sk.files[i];
                (f.path.clone(), f.mtime_ns, f.size)
            })
            .collect();
        skeleton::facts_hash(&facts, &skeleton::edge_facts(edges))
    }

    /// Indices of every file in the index (the perspective root's files).
    pub fn all_files(&self) -> Vec<usize> {
        (0..self.sk.files.len()).collect()
    }

    /// Top-ranked signatures under `path`.
    pub async fn top_symbols(&self, index: &SqlitePool, path: &str) -> Vec<String> {
        let rows: Vec<(String,)> = sqlx::query_as(
            "SELECT s.signature FROM symbols s JOIN files f ON f.id = s.file_id \
             WHERE (f.path = ? OR f.path LIKE ?) AND f.is_test = 0 ORDER BY s.rank DESC, s.id LIMIT ?",
        )
        .bind(path)
        .bind(format!("{}/%", path.replace('%', "\\%")))
        .bind(TOP_SYMBOLS as i64)
        .fetch_all(index)
        .await
        .unwrap_or_default();
        rows.into_iter()
            .map(|(s,)| cut(&s, SIGNATURE_CHARS))
            .collect()
    }

    /// The `concern` triggers of §4.3 for an element: files over
    /// [`CONCERN_FILE_LINES`] lines and, for a top-level unit, an in-degree
    /// over [`CONCERN_IN_DEGREE`].
    pub fn concern_triggers(&self, files: &[usize], node: &str) -> Vec<String> {
        let mut out: Vec<String> = files
            .iter()
            .map(|&i| &self.sk.files[i])
            .filter(|f| f.lines > CONCERN_FILE_LINES)
            .map(|f| format!("{} has {} lines", f.path, f.lines))
            .collect();
        let indeg = self.edges.iter().filter(|e| e.to == node).count();
        if indeg > CONCERN_IN_DEGREE {
            out.push(format!("{node} has in-degree {indeg}"));
        }
        out
    }

    /// Stage D input for a flow perspective: the ≤ `limit` longest call
    /// chains from entry points and the perspective's detector sites.
    pub async fn flows(
        &self,
        index: &SqlitePool,
        detection: Option<&Detection>,
        limit: usize,
    ) -> Vec<Flow> {
        let mut seeds: Vec<(String, Option<i64>)> = Vec::new();
        if let Some(d) = detection {
            for s in &d.samples {
                if let Some((path, line)) = s.rsplit_once("#L")
                    && let Ok(line) = line.parse::<i64>()
                {
                    seeds.push((path.to_owned(), Some(line)));
                }
            }
        }
        let mut entries: Vec<&String> = self.sk.entry_points.iter().collect();
        entries.sort();
        seeds.extend(entries.into_iter().map(|p| (p.clone(), None)));
        seeds.dedup();
        let mut flows = Vec::new();
        let mut seen_entries = BTreeSet::new();
        for (path, line) in seeds.into_iter().take(FLOW_CANDIDATES) {
            let Some(entry) = symbol_at(index, &path, line).await else {
                continue;
            };
            if !seen_entries.insert(entry.1.moniker.clone()) {
                continue;
            }
            flows.push(walk_chain(index, entry).await);
        }
        flows.retain(|f| !f.calls.is_empty());
        flows.sort_by(|a, b| {
            b.calls
                .len()
                .cmp(&a.calls.len())
                .then_with(|| a.entry.moniker.cmp(&b.entry.moniker))
        });
        flows.truncate(limit);
        flows
    }
}

/// The twelve detectors with `overall-architecture` always eligible; a
/// missing detector row counts as ineligible.
fn with_architecture(mut d: Vec<Detection>) -> Vec<Detection> {
    for kind in PERSPECTIVES {
        if !d.iter().any(|x| x.kind == kind) {
            d.push(Detection {
                kind: kind.to_owned(),
                eligible: false,
                hits: 0,
                samples: Vec::new(),
                confidence: 0.0,
            });
        }
    }
    if let Some(a) = d.iter_mut().find(|x| x.kind == PERSPECTIVES[0]) {
        a.eligible = true;
    }
    d.sort_by_key(|x| {
        PERSPECTIVES
            .iter()
            .position(|k| *k == x.kind)
            .unwrap_or(usize::MAX)
    });
    d
}

/// The longest common directory prefix of two paths, by whole segments.
fn common_prefix_dir(a: &str, b: &str) -> String {
    let mut out = Vec::new();
    for (x, y) in a.split('/').zip(b.split('/')) {
        if x != y {
            break;
        }
        out.push(x);
    }
    out.join("/")
}

/// `s` cut to `n` characters with a trailing `…`.
pub fn cut(s: &str, n: usize) -> String {
    if s.chars().count() <= n {
        return s.to_owned();
    }
    let mut out: String = s.chars().take(n.saturating_sub(1)).collect();
    out.push('…');
    out
}

/// `symbols` row: `(id, moniker, name, path, line_start, line_end, signature)`.
type SymbolRow = (i64, String, String, String, i64, i64, String);
/// One callee: `(ref line, site path, id, moniker, name, path, line_start, line_end, signature)`.
type CalleeSql = (i64, String, i64, String, String, String, i64, i64, String);

fn flow_symbol((_, moniker, name, path, line_start, line_end, signature): SymbolRow) -> FlowSymbol {
    FlowSymbol {
        node: format!("sym:{moniker}"),
        moniker,
        name,
        path,
        line_start,
        line_end,
        signature: cut(&signature, SIGNATURE_CHARS),
    }
}

/// The symbol enclosing `line` in `path`, or the file's entry symbol
/// (`main` first, then exported by rank) when `line` is `None`.
async fn symbol_at(index: &SqlitePool, path: &str, line: Option<i64>) -> Option<(i64, FlowSymbol)> {
    let row: Option<SymbolRow> = match line {
        Some(line) => sqlx::query_as(
            "SELECT s.id, s.moniker, s.name, f.path, s.line_start, s.line_end, s.signature \
             FROM symbols s JOIN files f ON f.id = s.file_id \
             WHERE f.path = ? AND s.line_start <= ? AND s.line_end >= ? \
             ORDER BY (s.line_end - s.line_start) LIMIT 1",
        )
        .bind(path)
        .bind(line)
        .bind(line)
        .fetch_optional(index)
        .await
        .ok()?,
        None => sqlx::query_as(
            "SELECT s.id, s.moniker, s.name, f.path, s.line_start, s.line_end, s.signature \
             FROM symbols s JOIN files f ON f.id = s.file_id \
             WHERE f.path = ? AND s.kind IN ('function', 'method', 'class', 'struct', 'module') \
             ORDER BY (s.name = 'main') DESC, s.exported DESC, s.rank DESC LIMIT 1",
        )
        .bind(path)
        .fetch_optional(index)
        .await
        .ok()?,
    };
    row.map(|r| (r.0, flow_symbol(r)))
}

/// Breadth-first over `refs` from `entry`: depth ≤ [`FLOW_DEPTH`], ≤
/// [`FLOW_SITES`] sites, best confidence first, test files skipped.
async fn walk_chain(index: &SqlitePool, (entry_id, entry): (i64, FlowSymbol)) -> Flow {
    let mut symbols = vec![entry.clone()];
    let mut seen: BTreeSet<i64> = BTreeSet::from([entry_id]);
    let mut calls = Vec::new();
    let mut frontier = vec![(entry_id, entry.node.clone())];
    for _ in 0..FLOW_DEPTH {
        let mut next = Vec::new();
        for (id, from_node) in frontier {
            let rows: Vec<CalleeSql> = sqlx::query_as(
                "SELECT r.line, fa.path, sb.id, sb.moniker, sb.name, fb.path, sb.line_start, sb.line_end, sb.signature \
                 FROM refs r JOIN files fa ON fa.id = r.file_id \
                 JOIN symbols sb ON sb.id = r.to_symbol JOIN files fb ON fb.id = sb.file_id \
                 WHERE r.from_symbol = ? AND r.to_symbol <> r.from_symbol AND r.confidence >= ? AND fb.is_test = 0 \
                 ORDER BY r.confidence DESC, r.line LIMIT ?",
            )
            .bind(id)
            .bind(REF_MIN_CONFIDENCE)
            .bind(FLOW_FANOUT)
            .fetch_all(index)
            .await
            .unwrap_or_default();
            for (line, site_path, sid, moniker, name, path, line_start, line_end, signature) in rows
            {
                if calls.len() >= FLOW_SITES {
                    return Flow {
                        entry,
                        symbols,
                        calls,
                    };
                }
                let to_node = format!("sym:{moniker}");
                if seen.insert(sid) {
                    symbols.push(flow_symbol((
                        sid, moniker, name, path, line_start, line_end, signature,
                    )));
                    next.push((sid, to_node.clone()));
                }
                if !calls
                    .iter()
                    .any(|c: &CallSite| c.from == from_node && c.to == to_node)
                {
                    calls.push(CallSite {
                        site: format!("{site_path}#L{line}"),
                        from: from_node.clone(),
                        to: to_node,
                    });
                }
            }
        }
        if next.is_empty() {
            break;
        }
        frontier = next;
    }
    Flow {
        entry,
        symbols,
        calls,
    }
}
