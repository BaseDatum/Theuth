//! The skeleton — what the index decides, zero tokens (147 §5): units, sizes,
//! edges with evidence, entry points, the structure-only view (§5.6), the
//! drill-down structure (§5.5), facts hashes (§5.4) and the three store reads
//! behind `codemap_overview` / `codemap_element` / `codemap_search`.
//! Runs at the end of every `reconcile`/`touch` that wrote (`index.rs`) once
//! a root has a map file; the first pass runs inline on the first read.
//!
//! owner: W1 — design §5, §8, §10.2 (overview/element/search)

use std::collections::{BTreeMap, HashMap, HashSet};
use std::path::Path;
use std::time::Instant;

use sha2::Digest;
use sqlx::SqlitePool;
use surface::{GraphEdge, GraphEdgeStyle, GraphKind, GraphLegendEntry, GraphNode};

use super::detect::{self, DetectedEdge, Detection};
use super::store::{self as mstore, AnchorRow, ElementRow, PerspectiveRow, StructureElement};
use super::{
    CHANGED_ELEMENTS, DIAGRAM_NODES, DIAGRAM_NODES_HARD, Element, GraphProps, LISTING_SYMBOLS,
    MAX_DEPTH, MapAnchor, MapChild, MapEdge, MapElement, MapEvidence, MapHit, MapIndex, MapListing,
    MapMeta, MapOrigin, MapPerspective, MapState, MapTreeRow, Overview, ProfileLine, SEARCH_LIMIT,
    Search, element_id,
};
use crate::event::CodeindexEvent;
use crate::index::{Codeindex, RootIndex};
use crate::lens::{self, MapRow};
use crate::resolve::rust::CargoWorkspace;
use crate::store::{self, FileEdge, FileFact};
use crate::{profile, rank, walk};

/// Sub-folders with at least this many parsed files become a unit (§5.1 item 3).
pub const LARGE_FOLDER_FILES: u32 = 40;
/// A unit of at most this many parsed files is a leaf (§5.1 item 4).
pub const LEAF_FILES: u32 = 2;
/// Edges below this share of a unit's out-weight fold into the bundle (§5.2).
pub const EDGE_MIN_SHARE: f64 = 0.02;
/// Cross-file refs below this confidence are not evidence (§5.2).
pub const REF_MIN_CONFIDENCE: f64 = 0.5;
/// The perspective the skeleton owns outright (§5.6); the model repartitions
/// its top level, the skeleton keeps everything below.
pub const STRUCTURE_PERSPECTIVE: &str = "overall-architecture";
/// `GraphNode.sub` cap (surface `MAX_SUB_CHARS`).
const SUB_CHARS: usize = 40;
/// `GraphNode.label` cap (surface `MAX_GRAPH_LABEL_CHARS`).
const LABEL_CHARS: usize = 80;
/// `MapRow.signature` cap (73 §5.3).
const SIGNATURE_CHARS: usize = 200;

// --- the in-memory skeleton -------------------------------------------------------

/// One drill-down child the index decides (§5.5): a directory (`unit`) or a
/// file, with the parsed files under it.
#[derive(Clone, Debug)]
pub struct Child {
    /// `unit:<path>`.
    pub node: String,
    /// Basename.
    pub name: String,
    /// Root-relative path (a directory or a file).
    pub path: String,
    /// `unit` | `file`.
    pub kind: &'static str,
    /// Indices into [`Skeleton::files`].
    pub files: Vec<usize>,
    /// A workspace member (manifest directory).
    pub member: bool,
}

/// Per-unit measures (§5.1) for a set of files.
#[derive(Clone, Copy, Debug, Default)]
pub struct Measures {
    /// Parsed files.
    pub files: u32,
    /// Σ `files.lines`.
    pub lines: u32,
    /// Σ `symbols`.
    pub symbols: u32,
    /// Test files.
    pub tests: u32,
    /// Σ `files.rank` / Σ all, 0–1.
    pub share: f32,
    /// Any file within [`rank::RECENT_NS`].
    pub recent: bool,
    /// Any manifest entry point inside.
    pub entry: bool,
}

/// Everything the skeleton reads from the index, once per pass. `pub` so the
/// generator (W2) assembles its stage A/B facts from the same numbers.
pub struct Skeleton {
    /// Parsed files by path.
    pub files: Vec<FileFact>,
    /// File→file import/ref edges (§5.2).
    pub edges: Vec<FileEdge>,
    /// Manifest dependencies `(from dir, to dir, kind)` between members (§5.2).
    pub manifest_deps: Vec<(String, String, String)>,
    /// Workspace member directories (root-relative, no trailing slash), sorted.
    pub members: Vec<String>,
    /// `meta.entry_points`.
    pub entry_points: HashSet<String>,
    /// Detector summaries, catalogue order (§7).
    pub detections: Vec<Detection>,
    /// Bindings and process/socket edges between unit paths (§5.2).
    pub inferred: Vec<DetectedEdge>,
    /// `recent` threshold.
    pub recent_ns: i64,
    total_rank: f64,
    members_set: HashSet<String>,
}

impl Skeleton {
    /// Read the skeleton of `ri`: files, edges, members and their manifest
    /// deps, entry points, detectors, detector edges. Store reads plus the
    /// members' manifests (already indexed files).
    pub async fn load(ri: &RootIndex) -> anyhow::Result<Self> {
        let pool = &ri.pool;
        let files = store::file_facts(pool).await?;
        let edges = store::file_edges(pool, REF_MIN_CONFIDENCE).await?;
        let total_rank: f64 = files.iter().map(|f| f.rank).sum();
        let manifests: Vec<String> = sqlx::query_scalar(
            "SELECT path FROM files WHERE path LIKE '%/Cargo.toml' OR path LIKE '%/package.json' \
             OR path LIKE '%/pyproject.toml' OR path LIKE '%/go.mod' ORDER BY path",
        )
        .fetch_all(pool)
        .await?;
        let mut members: Vec<String> = manifests
            .iter()
            .filter(|m| !walk::is_test_path(m) && !walk::is_test_data(m))
            .filter_map(|m| m.rsplit_once('/').map(|(d, _)| d.to_owned()))
            .filter(|d| !d.contains("node_modules"))
            .collect();
        members.sort_unstable();
        members.dedup();
        let manifest_deps = manifest_deps(&ri.root, &members).await;
        let entry_points: HashSet<String> = store::meta(pool, "entry_points")
            .await?
            .and_then(|s| serde_json::from_str::<Vec<String>>(&s).ok())
            .unwrap_or_default()
            .into_iter()
            .collect();
        let detections = detect::perspectives(pool).await?;
        let now_ns = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map_or(0, |d| i64::try_from(d.as_nanos()).unwrap_or(i64::MAX));
        let mut me = Self {
            files,
            edges,
            manifest_deps,
            members_set: members.iter().cloned().collect(),
            members,
            entry_points,
            detections,
            inferred: Vec::new(),
            recent_ns: now_ns - rank::RECENT_NS,
            total_rank,
        };
        // Detector edges run over the unit paths two levels deep.
        let mut units: Vec<String> = Vec::new();
        for c in me.top_level() {
            if c.kind == "unit" {
                units.push(c.path.clone());
                units.extend(
                    me.children_of(&c.path)
                        .into_iter()
                        .filter(|d| d.kind == "unit")
                        .map(|d| d.path),
                );
            }
        }
        units.sort_unstable();
        units.dedup();
        me.inferred = detect::inferred_edges(pool, &units).await?;
        Ok(me)
    }

    /// The §5.1 measures of `files` (indices).
    pub fn measures(&self, files: &[usize]) -> Measures {
        let mut m = Measures::default();
        let mut rank = 0.0;
        for &i in files {
            let f = &self.files[i];
            m.files += 1;
            m.lines += u32::try_from(f.lines).unwrap_or(u32::MAX);
            m.symbols += u32::try_from(f.symbols).unwrap_or(u32::MAX);
            m.tests += u32::from(f.is_test);
            m.recent |= f.mtime_ns >= self.recent_ns;
            m.entry |= self.entry_points.contains(&f.path);
            rank += f.rank;
        }
        m.share = if self.total_rank > 0.0 {
            (rank / self.total_rank) as f32
        } else {
            0.0
        };
        m
    }

    /// Indices of the files under `path` (a directory prefix or the file itself).
    pub fn files_under(&self, path: &str) -> Vec<usize> {
        if path.is_empty() {
            return (0..self.files.len()).collect();
        }
        let prefix = format!("{path}/");
        // `files` is sorted by path: the file itself, then the contiguous
        // `<path>/` run (`a.rs` sorts between `a` and `a/`, so two lookups).
        let mut out: Vec<usize> = Vec::new();
        if let Ok(i) = self.files.binary_search_by(|f| f.path.as_str().cmp(path)) {
            out.push(i);
        }
        let start = self
            .files
            .partition_point(|f| f.path.as_str() < prefix.as_str());
        out.extend(
            self.files[start..]
                .iter()
                .enumerate()
                .take_while(|(_, f)| f.path.starts_with(&prefix))
                .map(|(i, _)| start + i),
        );
        out
    }

    /// The top level (§5.1 items 1–2): workspace members not nested in
    /// another member, then the top-level directories and files that no
    /// member covers. Members come first by share.
    pub fn top_level(&self) -> Vec<Child> {
        let top_members: Vec<&String> = self
            .members
            .iter()
            .filter(|m| {
                !self
                    .members
                    .iter()
                    .any(|o| o != *m && m.starts_with(&format!("{o}/")))
            })
            .collect();
        let mut out: Vec<Child> = Vec::new();
        let mut covered: Vec<bool> = vec![false; self.files.len()];
        for m in &top_members {
            let files = self.files_under(m);
            for &i in &files {
                covered[i] = true;
            }
            if files.is_empty() {
                continue;
            }
            out.push(self.child(m, files, true));
        }
        let rest: Vec<usize> = (0..self.files.len()).filter(|&i| !covered[i]).collect();
        out.extend(self.group(&rest, ""));
        self.sort(&mut out);
        out
    }

    /// The children of the directory `path` (§5.5): sub-directories with more
    /// than [`LEAF_FILES`] files (or a member) as units, smaller ones merged
    /// into their files, direct files as leaves; a directory holding exactly
    /// one sub-directory and nothing else collapses into it.
    pub fn children_of(&self, path: &str) -> Vec<Child> {
        let files = self.files_under(path);
        let mut prefix = path.to_owned();
        let mut depth = 0;
        loop {
            let mut kids = self.group(&files, &prefix);
            // Collapse single-directory chains (`crates/x/src`).
            if kids.len() == 1 && kids[0].kind == "unit" && depth < 8 {
                prefix = kids.remove(0).path;
                depth += 1;
                continue;
            }
            self.sort(&mut kids);
            return kids;
        }
    }

    /// A node id → the child it names, when its path still exists in the index.
    pub fn child_of_node(&self, node: &str) -> Option<Child> {
        let path = node.strip_prefix("unit:")?;
        let files = self.files_under(path);
        if files.is_empty() {
            return None;
        }
        Some(self.child(path, files, self.members_set.contains(path)))
    }

    fn child(&self, path: &str, files: Vec<usize>, member: bool) -> Child {
        let is_file = files.len() == 1 && self.files[files[0]].path == path;
        Child {
            node: format!("unit:{path}"),
            name: path.rsplit('/').next().unwrap_or(path).to_owned(),
            path: path.to_owned(),
            kind: if is_file { "file" } else { "unit" },
            files,
            member,
        }
    }

    /// Group `files` by their first path segment under `prefix`.
    fn group(&self, files: &[usize], prefix: &str) -> Vec<Child> {
        let cut = if prefix.is_empty() {
            0
        } else {
            prefix.len() + 1
        };
        let mut dirs: BTreeMap<&str, Vec<usize>> = BTreeMap::new();
        let mut direct: Vec<usize> = Vec::new();
        for &i in files {
            let rest = &self.files[i].path[cut..];
            match rest.split_once('/') {
                Some((d, _)) => dirs.entry(d).or_default().push(i),
                None => direct.push(i),
            }
        }
        let mut out: Vec<Child> = Vec::new();
        for (d, idx) in dirs {
            let path = if prefix.is_empty() {
                d.to_owned()
            } else {
                format!("{prefix}/{d}")
            };
            let member = self.members_set.contains(&path);
            if !member && idx.len() <= LEAF_FILES as usize {
                direct.extend(idx);
            } else {
                out.push(self.child(&path, idx, member));
            }
        }
        for i in direct {
            let p = self.files[i].path.clone();
            out.push(self.child(&p, vec![i], false));
        }
        out
    }

    fn sort(&self, kids: &mut [Child]) {
        kids.sort_by(|a, b| {
            let (sa, sb) = (self.measures(&a.files).share, self.measures(&b.files).share);
            b.member
                .cmp(&a.member)
                .then(sb.partial_cmp(&sa).unwrap_or(std::cmp::Ordering::Equal))
                .then_with(|| a.path.cmp(&b.path))
        });
    }

    /// The edges between `children` with their evidence (§5.2), the minor
    /// edges folded per source (below [`EDGE_MIN_SHARE`] of its out-weight)
    /// and the edges that touched a child outside `children` are not here —
    /// the caller counts those through [`Self::edges_touching`].
    pub fn edges_between(&self, children: &[Child]) -> (Vec<MapEdge>, u32) {
        let mut owner: HashMap<&str, usize> = HashMap::new();
        for (ci, c) in children.iter().enumerate() {
            for &i in &c.files {
                owner.insert(self.files[i].path.as_str(), ci);
            }
        }
        // A member directory's owner: the child whose path covers it.
        let dir_owner = |dir: &str| -> Option<usize> {
            children
                .iter()
                .enumerate()
                .filter(|(_, c)| dir == c.path || dir.starts_with(&format!("{}/", c.path)))
                .max_by_key(|(_, c)| c.path.len())
                .map(|(i, _)| i)
        };
        #[derive(Default)]
        struct Acc {
            imports: u32,
            import_sample: Option<String>,
            refs: u32,
            ref_sample: Option<String>,
            conf: f64,
            manifest: Vec<MapEvidence>,
            detected: Vec<MapEvidence>,
        }
        let mut acc: BTreeMap<(usize, usize), Acc> = BTreeMap::new();
        for e in &self.edges {
            let (Some(&a), Some(&b)) = (owner.get(e.from.as_str()), owner.get(e.to.as_str()))
            else {
                continue;
            };
            if a == b {
                continue;
            }
            let x = acc.entry((a, b)).or_default();
            let n = u32::try_from(e.count).unwrap_or(u32::MAX);
            if e.kind == "imports" {
                x.imports += n;
                x.import_sample
                    .get_or_insert_with(|| format!("{}#L{}", e.from, e.line));
            } else {
                x.conf = (x.conf * f64::from(x.refs) + e.confidence * f64::from(n))
                    / f64::from(x.refs + n);
                x.refs += n;
                x.ref_sample
                    .get_or_insert_with(|| format!("{}#L{}", e.from, e.line));
            }
        }
        for (from, to, kind) in &self.manifest_deps {
            let (Some(a), Some(b)) = (dir_owner(from), dir_owner(to)) else {
                continue;
            };
            if a == b {
                continue;
            }
            acc.entry((a, b))
                .or_default()
                .manifest
                .push(MapEvidence::Manifest {
                    from: from.clone(),
                    to: to.clone(),
                    kind: kind.clone(),
                });
        }
        for d in &self.inferred {
            let (Some(a), Some(b)) = (dir_owner(&d.from), dir_owner(&d.to)) else {
                continue;
            };
            if a == b {
                continue;
            }
            acc.entry((a, b))
                .or_default()
                .detected
                .push(d.evidence.clone());
        }
        // Weight and threshold per source.
        let weight = |x: &Acc| {
            f64::from(x.imports + x.refs)
                + 10.0 * x.manifest.len() as f64
                + 5.0 * x.detected.len() as f64
        };
        let mut out_weight: HashMap<usize, f64> = HashMap::new();
        for ((a, _), x) in &acc {
            *out_weight.entry(*a).or_default() += weight(x);
        }
        let mut edges = Vec::with_capacity(acc.len());
        let mut minor = 0u32;
        for ((a, b), x) in acc {
            let w = weight(&x);
            if w < EDGE_MIN_SHARE * out_weight[&a] {
                minor += 1;
                continue;
            }
            let mut evidence: Vec<MapEvidence> = Vec::new();
            let mut label: Vec<String> = Vec::new();
            let plural = |n: u32, one: &str, many: &str| {
                if n == 1 {
                    format!("1 {one}")
                } else {
                    format!("{n} {many}")
                }
            };
            if !x.manifest.is_empty() {
                let n = u32::try_from(x.manifest.len()).unwrap_or(u32::MAX);
                let cargo = x.manifest.iter().any(
                    |m| matches!(m, MapEvidence::Manifest { kind, .. } if kind == "cargo-path"),
                );
                label.push(if cargo {
                    plural(n, "Cargo dep", "Cargo deps")
                } else {
                    plural(n, "workspace dep", "workspace deps")
                });
                evidence.extend(x.manifest);
            }
            if x.imports > 0 {
                label.push(plural(x.imports, "import", "imports"));
                evidence.push(MapEvidence::Imports {
                    count: x.imports,
                    sample: x.import_sample.unwrap_or_default(),
                });
            }
            if x.refs > 0 {
                label.push(plural(x.refs, "ref", "refs"));
                evidence.push(MapEvidence::Refs {
                    count: x.refs,
                    sample: x.ref_sample.unwrap_or_default(),
                    confidence: (x.conf * 100.0).round() / 100.0,
                });
            }
            let mut detected_labels: Vec<String> = x
                .detected
                .iter()
                .map(|d| match d {
                    MapEvidence::Bindings { generator, .. } => format!("{generator} bindings"),
                    MapEvidence::Process { verb, .. } => format!("{verb}s"),
                    _ => "detected".to_owned(),
                })
                .collect();
            detected_labels.sort_unstable();
            detected_labels.dedup();
            label.extend(detected_labels);
            evidence.extend(x.detected);
            edges.push(MapEdge {
                from: children[a].node.clone(),
                to: children[b].node.clone(),
                label: label.join(" · "),
                backed: true,
                evidence,
            });
        }
        (edges, minor)
    }

    /// Edges (file pairs or manifest deps) with exactly one endpoint inside
    /// `kept` and the other inside `hidden`: the count a bundle footnote shows.
    pub fn edges_touching(&self, kept: &[Child], hidden: &[Child]) -> u32 {
        let mut side: HashMap<&str, bool> = HashMap::new();
        for c in kept {
            for &i in &c.files {
                side.insert(self.files[i].path.as_str(), true);
            }
        }
        for c in hidden {
            for &i in &c.files {
                side.insert(self.files[i].path.as_str(), false);
            }
        }
        let mut n = 0u32;
        for e in &self.edges {
            if let (Some(a), Some(b)) = (side.get(e.from.as_str()), side.get(e.to.as_str()))
                && a != b
            {
                n += 1;
            }
        }
        n
    }
}

/// Cargo path deps between members (through `CargoWorkspace`, which reads the
/// manifests) and npm workspace deps (member `package.json`s naming another
/// member's `name`). pyproject local path deps are not read (`ponytail:` add
/// `[tool.uv.sources]`/poetry `path =` when a Python monorepo shows up).
async fn manifest_deps(root: &Path, members: &[String]) -> Vec<(String, String, String)> {
    let root = root.to_path_buf();
    let members = members.to_vec();
    tokio::task::spawn_blocking(move || {
        let mut out: Vec<(String, String, String)> = Vec::new();
        if let Some(ws) = CargoWorkspace::load(&root) {
            let dir_of: HashMap<&str, &str> = ws
                .packages
                .iter()
                .map(|p| (p.name.as_str(), p.dir.as_str()))
                .collect();
            for p in &ws.packages {
                if p.dir.is_empty() {
                    continue;
                }
                let mut targets: Vec<&str> = p
                    .deps
                    .values()
                    .filter_map(|name| dir_of.get(name.as_str()).copied())
                    .filter(|d| !d.is_empty() && *d != p.dir)
                    .collect();
                targets.sort_unstable();
                targets.dedup();
                for d in targets {
                    out.push((p.dir.clone(), d.to_owned(), "cargo-path".to_owned()));
                }
            }
        }
        let npm: Vec<(String, serde_json::Value)> = members
            .iter()
            .filter_map(|d| {
                let v: serde_json::Value =
                    serde_json::from_slice(&std::fs::read(root.join(d).join("package.json")).ok()?)
                        .ok()?;
                Some((d.clone(), v))
            })
            .collect();
        let name_dir: HashMap<&str, &str> = npm
            .iter()
            .filter_map(|(d, v)| Some((v.get("name")?.as_str()?, d.as_str())))
            .collect();
        for (d, v) in &npm {
            let mut targets: Vec<&str> = ["dependencies", "devDependencies", "peerDependencies"]
                .iter()
                .filter_map(|k| v.get(k)?.as_object())
                .flat_map(|o| o.keys())
                .filter_map(|k| name_dir.get(k.as_str()).copied())
                .filter(|t| *t != d.as_str())
                .collect();
            targets.sort_unstable();
            targets.dedup();
            for t in targets {
                out.push((d.clone(), t.to_owned(), "npm-workspace".to_owned()));
            }
        }
        out.sort();
        out.dedup();
        out
    })
    .await
    .unwrap_or_default()
}

// --- diagrams --------------------------------------------------------------------

/// Cut `s` to `max` characters, marking the cut with `…` (left-cut when `left`).
fn clip(s: &str, max: usize, left: bool) -> String {
    let n = s.chars().count();
    if n <= max {
        return s.to_owned();
    }
    if left {
        let tail: String = s.chars().skip(n - (max - 1)).collect();
        format!("…{tail}")
    } else {
        let head: String = s.chars().take(max - 1).collect();
        format!("{head}…")
    }
}

/// One drawn node of a structure diagram.
struct DiagramNode<'a> {
    node: &'a str,
    name: &'a str,
    path: Option<&'a str>,
    kind: &'a str,
    bundle_of: u32,
}

/// The structure diagram (§5.6): the children as nodes (`label` = name,
/// `sub` = path, `ref` = path), the aggregated edges with mechanical labels,
/// a counted bundle with `tag: +N`, and a legend line for folded and hidden
/// edges.
fn structure_diagram(
    nodes: &[DiagramNode<'_>],
    edges: &[MapEdge],
    minor: u32,
    hidden: u32,
) -> GraphProps {
    let mut legend: Vec<GraphLegendEntry> = Vec::new();
    if minor > 0 {
        legend.push(GraphLegendEntry {
            label: format!("+{minor} minor edges folded (< 2 % of a source's weight)"),
            state: None,
            style: None,
            kind: None,
        });
    }
    if hidden > 0 {
        legend.push(GraphLegendEntry {
            label: format!("{hidden} edges to the bundle hidden"),
            state: None,
            style: None,
            kind: None,
        });
    }
    GraphProps {
        nodes: nodes
            .iter()
            .map(|n| GraphNode {
                id: n.node.to_owned(),
                label: clip(n.name, LABEL_CHARS, false),
                kind: Some(match n.kind {
                    "file" => GraphKind::Rounded,
                    "bundle" => GraphKind::Note,
                    _ => GraphKind::Box,
                }),
                group: None,
                state: None,
                r#ref: n.path.map(str::to_owned),
                sub: n.path.map(|p| clip(p, SUB_CHARS, true)),
                tag: (n.bundle_of > 0).then(|| format!("+{}", n.bundle_of)),
                icon: None,
            })
            .collect(),
        edges: edges
            .iter()
            .map(|e| GraphEdge {
                from: e.from.clone(),
                to: e.to.clone(),
                label: Some(clip(&e.label, LABEL_CHARS, false)),
                style: Some(if e.backed {
                    GraphEdgeStyle::Solid
                } else {
                    GraphEdgeStyle::Dashed
                }),
                state: None,
                arrow: None,
            })
            .collect(),
        groups: None,
        direction: None,
        legend: (!legend.is_empty()).then_some(legend),
        title: None,
    }
}

/// §5.4: SHA-256 hex over the sorted `(path, mtime_ns, size)` of every file
/// under an element's anchors followed by the sorted `(from, to, kind, count)`
/// of its sub-skeleton edges. Pure; the one definition W1 and W2 share.
pub fn facts_hash(files: &[(String, i64, i64)], edges: &[(String, String, String, u32)]) -> String {
    let mut files: Vec<&(String, i64, i64)> = files.iter().collect();
    files.sort_unstable();
    let mut edges: Vec<&(String, String, String, u32)> = edges.iter().collect();
    edges.sort_unstable();
    let mut h = sha2::Sha256::new();
    for (p, m, s) in files {
        h.update(p.as_bytes());
        h.update(b"\0");
        h.update(m.to_le_bytes());
        h.update(s.to_le_bytes());
    }
    h.update(b"\n");
    for (f, t, k, n) in edges {
        h.update(f.as_bytes());
        h.update(b"\0");
        h.update(t.as_bytes());
        h.update(b"\0");
        h.update(k.as_bytes());
        h.update(b"\0");
        h.update(n.to_le_bytes());
    }
    format!("{:x}", h.finalize())
}

/// The `(from, to, kind, count)` rows of `edges` for [`facts_hash`].
pub fn edge_facts(edges: &[MapEdge]) -> Vec<(String, String, String, u32)> {
    let mut out = Vec::new();
    for e in edges {
        for ev in &e.evidence {
            let (kind, n) = match ev {
                MapEvidence::Manifest { .. } => ("manifest", 1),
                MapEvidence::Imports { count, .. } => ("imports", *count),
                MapEvidence::Refs { count, .. } => ("refs", *count),
                MapEvidence::Call { .. } => ("call", 1),
                MapEvidence::Bindings { .. } => ("bindings", 1),
                MapEvidence::Process { .. } => ("process", 1),
                MapEvidence::Inferred { .. } => continue,
            };
            out.push((e.from.clone(), e.to.clone(), kind.to_owned(), n));
        }
    }
    out
}

// --- the pass --------------------------------------------------------------------

/// One element the pass will write.
struct Put {
    element: StructureElement,
    edges: Vec<MapEdge>,
    anchors: Vec<AnchorRow>,
}

/// What one pass decided for every perspective.
#[derive(Default)]
struct Plan {
    puts: Vec<Put>,
    visited: HashSet<String>,
}

impl Skeleton {
    /// Plan the sub-tree of one element: its children (capped + bundled),
    /// its diagram, its edges and anchors, then recurse into the kept
    /// children down to [`MAX_DEPTH`].
    #[allow(clippy::too_many_arguments)]
    fn plan_element(
        &self,
        plan: &mut Plan,
        existing: &HashMap<String, ElementRow>,
        perspective: &str,
        node: &str,
        parent: Option<&str>,
        depth: u8,
        order_no: u32,
        name: &str,
        path: Option<&str>,
        kind: &str,
        files: &[usize],
        members: Vec<String>,
        children: Vec<Child>,
    ) {
        let id = if depth == 0 {
            perspective.to_owned()
        } else {
            element_id(perspective, node)
        };
        plan.visited.insert(id.clone());
        let generated = existing
            .get(&id)
            .is_some_and(|r| r.origin() == MapOrigin::Generated);
        // §5.1 item 4: a unit of ≤ LEAF_FILES parsed files is a leaf — the
        // generator's `Facts::is_leaf`, so recompute adds no rows under it.
        let leaf = matches!(kind, "file" | "bundle" | "external" | "symbol")
            || (kind == "unit" && files.len() <= LEAF_FILES as usize)
            || depth >= MAX_DEPTH
            || children.is_empty();
        // Cap + bundle; a model partition keeps its own (≤ hard cap) children.
        let cap = if generated {
            DIAGRAM_NODES_HARD
        } else {
            DIAGRAM_NODES
        };
        let (kept, hidden): (Vec<Child>, Vec<Child>) = if children.len() > cap {
            let mut c = children;
            let rest = c.split_off(cap);
            (c, rest)
        } else {
            (children, Vec::new())
        };
        let (edges, minor) = if leaf {
            (Vec::new(), 0)
        } else {
            self.edges_between(&kept)
        };
        let hidden_edges = if hidden.is_empty() {
            0
        } else {
            self.edges_touching(&kept, &hidden)
        };
        let bundle_node = format!("bundle:{node}");
        let mut nodes: Vec<DiagramNode<'_>> = kept
            .iter()
            .map(|c| DiagramNode {
                node: &c.node,
                name: &c.name,
                path: Some(&c.path),
                kind: c.kind,
                bundle_of: 0,
            })
            .collect();
        let bundle_name = format!("+{} more", hidden.len());
        if !hidden.is_empty() {
            nodes.push(DiagramNode {
                node: &bundle_node,
                name: &bundle_name,
                path: None,
                kind: "bundle",
                bundle_of: u32::try_from(hidden.len()).unwrap_or(u32::MAX),
            });
        }
        let diagram = (!leaf).then(|| structure_diagram(&nodes, &edges, minor, hidden_edges));
        let file_facts: Vec<(String, i64, i64)> = files
            .iter()
            .map(|&i| {
                let f = &self.files[i];
                (f.path.clone(), f.mtime_ns, f.size)
            })
            .collect();
        let hash = facts_hash(&file_facts, &edge_facts(&edges));
        let mut anchors: Vec<AnchorRow> = match kind {
            "perspective" => Vec::new(),
            "bundle" | "cluster" => members
                .iter()
                .filter_map(|m| m.strip_prefix("unit:"))
                .map(|p| (p.to_owned(), None, None))
                .collect(),
            _ => path
                .map(|p| (p.to_owned(), None, None))
                .into_iter()
                .collect(),
        };
        anchors.sort();
        anchors.dedup();
        plan.puts.push(Put {
            element: StructureElement {
                id: id.clone(),
                perspective: perspective.to_owned(),
                node: node.to_owned(),
                parent: parent.map(str::to_owned),
                depth,
                order_no,
                name: name.to_owned(),
                path: path.map(str::to_owned),
                kind: kind.to_owned(),
                members,
                leaf,
                facts_hash: hash,
                diagram,
            },
            edges,
            anchors,
        });
        if leaf {
            return;
        }
        // A generated element's children are the model's rows: recurse into
        // them by node, never add or remove.
        for (i, c) in kept.iter().enumerate() {
            let child_id = element_id(perspective, &c.node);
            let grand = match existing.get(&child_id) {
                Some(r) if r.origin() == MapOrigin::Generated && r.kind == "cluster" => r
                    .members()
                    .iter()
                    .filter_map(|m| self.child_of_node(m))
                    .collect(),
                _ if c.kind == "unit" && depth + 1 < MAX_DEPTH => self.children_of(&c.path),
                _ => Vec::new(),
            };
            let (kind, members) = match existing.get(&child_id) {
                Some(r) if r.origin() == MapOrigin::Generated => (r.kind.as_str(), r.members()),
                _ => (c.kind, Vec::new()),
            };
            self.plan_element(
                plan,
                existing,
                perspective,
                &c.node,
                Some(&id),
                depth + 1,
                u32::try_from(i).unwrap_or(u32::MAX),
                &c.name,
                Some(&c.path),
                kind,
                &c.files,
                members,
                grand,
            );
        }
        if !hidden.is_empty() {
            let files: Vec<usize> = hidden
                .iter()
                .flat_map(|c| c.files.iter().copied())
                .collect();
            self.plan_element(
                plan,
                existing,
                perspective,
                &bundle_node,
                Some(&id),
                depth + 1,
                u32::try_from(kept.len()).unwrap_or(u32::MAX),
                &bundle_name,
                None,
                "bundle",
                &files,
                hidden.iter().map(|c| c.node.clone()).collect(),
                Vec::new(),
            );
        }
    }

    /// Plan one perspective from its root: a structure root gets the top
    /// level (§5.6); a generated root keeps the model's children (clusters,
    /// units, externals) and the skeleton fills in below them (§5.5).
    fn plan_perspective(
        &self,
        plan: &mut Plan,
        existing: &HashMap<String, ElementRow>,
        p: &PerspectiveRow,
    ) {
        let root_node = format!("perspective:{}", p.id);
        let all: Vec<usize> = (0..self.files.len()).collect();
        let root = existing.get(&p.id);
        let children: Vec<Child> = match root {
            Some(r) if r.origin() == MapOrigin::Generated => existing
                .values()
                .filter(|e| e.parent.as_deref() == Some(p.id.as_str()))
                .collect::<Vec<_>>()
                .into_iter()
                .filter_map(|e| self.model_child(e))
                .collect(),
            _ => self.top_level(),
        };
        let mut children = children;
        if root.is_some_and(|r| r.origin() == MapOrigin::Generated) {
            children.sort_by_key(|c| {
                existing
                    .get(&element_id(&p.id, &c.node))
                    .map_or(i64::MAX, |e| e.order_no)
            });
        }
        self.plan_element(
            plan,
            existing,
            &p.id,
            &root_node,
            None,
            0,
            0,
            &p.title,
            None,
            "perspective",
            &all,
            Vec::new(),
            children,
        );
    }

    /// A model-written child row as a skeleton child: a `unit:` by its path,
    /// a `cluster:` or the root `bundle:` through its members' files, an
    /// `ext:` with no files.
    fn model_child(&self, e: &ElementRow) -> Option<Child> {
        if let Some(c) = self.child_of_node(&e.node) {
            return Some(Child {
                name: e.name.clone(),
                ..c
            });
        }
        if e.kind == "cluster" || e.kind == "bundle" {
            let mut files: Vec<usize> = e
                .members()
                .iter()
                .filter_map(|m| m.strip_prefix("unit:"))
                .flat_map(|p| self.files_under(p))
                .collect();
            files.sort_unstable();
            files.dedup();
            return Some(Child {
                node: e.node.clone(),
                name: e.name.clone(),
                path: e.path.clone().unwrap_or_default(),
                // A bundle stays a leaf (§5.6): `"unit"` would make its
                // `children_of("")` the whole top level.
                kind: if e.kind == "bundle" { "bundle" } else { "unit" },
                files,
                member: false,
            });
        }
        if e.kind == "external" || e.node.starts_with("ext:") {
            return Some(Child {
                node: e.node.clone(),
                name: e.name.clone(),
                path: e.path.clone().unwrap_or_default(),
                kind: "file",
                files: Vec::new(),
                member: false,
            });
        }
        None
    }
}

/// The skeleton pass after a write (§5, §8): recompute units, edges and the
/// structure view; refresh `MapChild` counts, anchors' `exists` and bundle
/// counts; recompute every element's facts hash and mark `stale`; read
/// `<root>/.omm/` through `import::import`; publish `CodeMapChanged` when
/// anything changed. Returns the element ids that changed.
pub async fn recompute(ix: &Codeindex, ri: &RootIndex) -> anyhow::Result<Vec<String>> {
    let t = Instant::now();
    let pool = ix.map_pool(ri).await?;
    let sk = Skeleton::load(ri).await?;
    let load_ms = t.elapsed().as_millis();
    match super::import::import(ri, &pool).await {
        Ok(n) if n > 0 => {
            tracing::debug!(root = %ri.root.display(), elements = n, "codemap .omm/ imported")
        }
        Ok(_) => {}
        Err(e) => tracing::debug!(error = format!("{e:#}"), "codemap .omm/ import skipped"),
    }
    let now = mstore::now_ms();
    let mut perspectives = mstore::perspectives(&pool).await?;
    if !perspectives.iter().any(|p| p.id == STRUCTURE_PERSPECTIVE) {
        let row = PerspectiveRow {
            id: STRUCTURE_PERSPECTIVE.to_owned(),
            kind: STRUCTURE_PERSPECTIVE.to_owned(),
            title: detect::title(STRUCTURE_PERSPECTIVE),
            origin: MapOrigin::Structure.as_str().to_owned(),
            why: None,
            order_no: 0,
            eligible: None,
            created_ms: now,
            updated_ms: now,
        };
        perspectives.insert(0, row);
    }
    let existing: HashMap<String, ElementRow> = mstore::all_elements(&pool)
        .await?
        .into_iter()
        .map(|e| (e.id.clone(), e))
        .collect();
    let t_plan = Instant::now();
    let mut plan = Plan::default();
    for p in &perspectives {
        if p.origin() == MapOrigin::Imported {
            continue;
        }
        sk.plan_perspective(&mut plan, &existing, p);
    }
    let plan_ms = t_plan.elapsed().as_millis();
    let deletes: Vec<String> = existing
        .values()
        .filter(|e| {
            e.origin() == MapOrigin::Structure
                && !plan.visited.contains(&e.id)
                && perspectives
                    .iter()
                    .any(|p| p.id == e.perspective && p.origin() != MapOrigin::Imported)
        })
        .map(|e| e.id.clone())
        .collect();

    let mut changed: Vec<String> = Vec::new();
    let mut stale: Vec<String> = Vec::new();
    let mut tx = pool.begin().await?;
    for p in &mut perspectives {
        if p.origin() == MapOrigin::Imported {
            continue;
        }
        p.eligible = sk
            .detections
            .iter()
            .find(|d| d.kind == p.kind)
            .map(serde_json::to_string)
            .transpose()?;
        p.updated_ms = now;
        mstore::upsert_perspective(&mut tx, p).await?;
    }
    for put in &plan.puts {
        let e = &put.element;
        match existing.get(&e.id) {
            Some(old) if old.origin() == MapOrigin::Generated => {
                mstore::put_structure(&mut tx, e).await?;
                mstore::replace_anchors(&mut tx, &e.id, &put.anchors).await?;
                let moved = old.facts_hash.as_deref() != Some(e.facts_hash.as_str());
                if moved && !old.stale {
                    mstore::set_stale(&mut tx, &e.id, true).await?;
                    stale.push(e.id.clone());
                    changed.push(e.id.clone());
                }
            }
            Some(old) => {
                let same = old.facts_hash.as_deref() == Some(e.facts_hash.as_str())
                    && old.parent == e.parent
                    && old.order_no == i64::from(e.order_no)
                    && old.name == e.name
                    && old.leaf == e.leaf;
                if !same {
                    mstore::put_structure(&mut tx, e).await?;
                    mstore::replace_edges(&mut tx, &e.id, &put.edges).await?;
                    mstore::replace_anchors(&mut tx, &e.id, &put.anchors).await?;
                    changed.push(e.id.clone());
                }
            }
            None => {
                mstore::put_structure(&mut tx, e).await?;
                mstore::replace_edges(&mut tx, &e.id, &put.edges).await?;
                mstore::replace_anchors(&mut tx, &e.id, &put.anchors).await?;
                changed.push(e.id.clone());
            }
        }
    }
    if !deletes.is_empty() {
        mstore::delete_elements(&mut tx, &deletes).await?;
        changed.extend(deletes);
    }
    tx.commit().await?;
    let (files, max_mtime) = store::map_state(&ri.pool).await?;
    mstore::set_meta(&pool, "index_state", &format!("{files}:{max_mtime}")).await?;
    if let Some(h) = store::meta(&ri.pool, "git_head").await? {
        mstore::set_meta(&pool, "git_head", &h).await?;
    }
    let total_ms = t.elapsed().as_millis();
    mstore::set_meta(&pool, "skeleton_ms", &total_ms.to_string()).await?;
    mstore::set_meta(
        &pool,
        "skeleton_phases_ms",
        &format!(
            "load {load_ms} · plan {plan_ms} · write {}",
            total_ms.saturating_sub(load_ms + plan_ms)
        ),
    )
    .await?;
    mstore::set_meta(&pool, "detections", &serde_json::to_string(&sk.detections)?).await?;
    tracing::debug!(root = %ri.root.display(), elements = plan.puts.len(), changed = changed.len(), stale = stale.len(), load_ms, plan_ms, ms = total_ms, "codemap skeleton");
    if !changed.is_empty() {
        let more = changed.len().saturating_sub(CHANGED_ELEMENTS);
        ix.publish(CodeindexEvent::CodeMapChanged {
            root: ri.root.clone(),
            elements: changed.iter().take(CHANGED_ELEMENTS).cloned().collect(),
            stale: stale.clone(),
            more,
            seq: ri.next_seq(),
        })
        .await;
    }
    Ok(changed)
}

// --- reads (§10.2) ------------------------------------------------------------------

/// The map pool of an indexed root, after the first skeleton pass when the
/// map file is new or empty (§10.2: "the first pass runs inline if missing").
async fn opened(
    ix: &Codeindex,
    root: &Path,
) -> anyhow::Result<(std::sync::Arc<RootIndex>, SqlitePool)> {
    let ri = lens::open_indexed(ix, root).await?;
    let fresh = !ix.has_map(&ri);
    let pool = ix.map_pool(&ri).await?;
    if fresh || mstore::perspectives(&pool).await?.is_empty() {
        recompute(ix, &ri).await?;
    }
    Ok((ri, pool))
}

async fn meta(ri: &RootIndex, pool: &SqlitePool, t: Instant) -> anyhow::Result<MapMeta> {
    let (files, max_mtime) = store::map_state(&ri.pool).await?;
    let job = mstore::running_job(pool).await?;
    let (generated, missing, errors) = mstore::state_counts(pool).await?;
    let state = if job.is_some() {
        MapState::Generating
    } else if generated == 0 {
        MapState::Structure
    } else if missing > 0 || errors > 0 {
        MapState::Partial
    } else {
        MapState::Ready
    };
    Ok(MapMeta {
        root: ri.root.clone(),
        seq: ri.seq(),
        schema: mstore::MAP_SCHEMA_VERSION,
        state,
        index_state: format!("{files}:{max_mtime}"),
        git_head: store::meta(&ri.pool, "git_head").await?,
        stale_elements: u32::try_from(mstore::stale_count(pool).await?).unwrap_or(u32::MAX),
        job,
        ms: u32::try_from(t.elapsed().as_millis()).unwrap_or(u32::MAX),
    })
}

/// Pre-order rows of one perspective from its stored elements.
fn tree_rows(rows: &[ElementRow]) -> Vec<MapTreeRow> {
    let mut kids: HashMap<Option<&str>, Vec<&ElementRow>> = HashMap::new();
    for r in rows {
        kids.entry(r.parent.as_deref()).or_default().push(r);
    }
    for v in kids.values_mut() {
        v.sort_by(|a, b| a.order_no.cmp(&b.order_no).then_with(|| a.id.cmp(&b.id)));
    }
    let mut out = Vec::with_capacity(rows.len());
    let mut stack: Vec<&ElementRow> = kids.get(&None).cloned().unwrap_or_default();
    stack.reverse();
    while let Some(r) = stack.pop() {
        let children = kids.get(&Some(r.id.as_str())).map_or(0, Vec::len);
        out.push(MapTreeRow {
            id: r.id.clone(),
            node: r.node.clone(),
            parent: r.parent.clone(),
            depth: u8::try_from(r.depth).unwrap_or(u8::MAX),
            name: r.name.clone(),
            path: r.path.clone(),
            kind: r.kind.clone(),
            children: u32::try_from(children).unwrap_or(u32::MAX),
            leaf: r.leaf,
            stale: r.stale,
            origin: r.origin(),
            marks: Vec::new(),
        });
        if let Some(c) = kids.get(&Some(r.id.as_str())) {
            stack.extend(c.iter().rev());
        }
    }
    out
}

/// `codemap_overview`: every perspective with its tree; `MapState::Structure`
/// after an inline first pass when the map file is missing (< 1 s). Store
/// reads only — never reconciles, never walks.
pub async fn overview(ix: &Codeindex, m: Overview) -> anyhow::Result<MapIndex> {
    let t = Instant::now();
    let (ri, pool) = opened(ix, &m.root).await?;
    let mut perspectives = Vec::new();
    for p in mstore::perspectives(&pool).await? {
        let rows = mstore::elements_of(&pool, &p.id).await?;
        let stale = rows.iter().filter(|r| r.stale).count();
        perspectives.push(MapPerspective {
            id: p.id.clone(),
            kind: p.kind.clone(),
            title: p.title.clone(),
            origin: p.origin(),
            elements: u32::try_from(rows.len()).unwrap_or(u32::MAX),
            stale: u32::try_from(stale).unwrap_or(u32::MAX),
            tree: tree_rows(&rows),
        });
    }
    let root = ri.root.clone();
    let profile =
        tokio::task::spawn_blocking(move || ProfileLine::from(&profile::detect(&root))).await?;
    Ok(MapIndex {
        meta: meta(&ri, &pool, t).await?,
        perspectives,
        profile,
    })
}

/// Live `(files, lines, rank)` under one anchor path (a file or a directory).
async fn anchor_stats(pool: &SqlitePool, path: &str) -> anyhow::Result<(i64, i64, f64)> {
    let prefix = format!("{path}/");
    Ok(sqlx::query_as(
        "SELECT count(*), coalesce(sum(lines), 0), coalesce(sum(rank), 0.0) FROM files \
         WHERE language <> 'other' AND (path = ?1 OR substr(path, 1, ?2) = ?3)",
    )
    .bind(path)
    .bind(i64::try_from(prefix.len()).unwrap_or(i64::MAX))
    .bind(&prefix)
    .fetch_one(pool)
    .await?)
}

async fn child_of(
    index: &SqlitePool,
    total_rank: f64,
    r: &ElementRow,
    anchors: &[AnchorRow],
) -> anyhow::Result<MapChild> {
    let (mut files, mut lines, mut rank) = (0i64, 0i64, 0f64);
    for (p, _, _) in anchors {
        let (f, l, k) = anchor_stats(index, p).await?;
        files += f;
        lines += l;
        rank += k;
    }
    Ok(MapChild {
        id: r.id.clone(),
        node: r.node.clone(),
        name: r.name.clone(),
        path: r.path.clone(),
        kind: r.kind.clone(),
        files: u32::try_from(files).unwrap_or(u32::MAX),
        lines: u32::try_from(lines).unwrap_or(u32::MAX),
        share: if total_rank > 0.0 {
            (rank / total_rank) as f32
        } else {
            0.0
        },
        leaf: r.leaf,
        stale: r.stale,
        thumbnail: if r.leaf { None } else { r.diagram() },
    })
}

/// The live listing of a leaf (§4.5 `MapListing`): files under its anchors
/// by rank, the top [`LISTING_SYMBOLS`] definitions with lens badges.
async fn listing(
    ri: &RootIndex,
    anchors: &[AnchorRow],
    total_rank: f64,
    entries: &HashSet<String>,
) -> anyhow::Result<MapListing> {
    let mut files: Vec<(String, u32, f32)> = Vec::new();
    let mut rows: Vec<store::ListingRow> = Vec::new();
    let mut total = 0i64;
    let mut importers: HashMap<String, i64> = HashMap::new();
    let now_ns = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0, |d| i64::try_from(d.as_nanos()).unwrap_or(i64::MAX));
    for (p, _, _) in anchors {
        let prefix = format!("{p}/");
        let fs: Vec<(String, i64, f64)> = sqlx::query_as(
            "SELECT path, lines, rank FROM files WHERE language <> 'other' AND (path = ?1 OR substr(path, 1, ?2) = ?3) ORDER BY rank DESC, path",
        )
        .bind(p)
        .bind(i64::try_from(prefix.len()).unwrap_or(i64::MAX))
        .bind(&prefix)
        .fetch_all(&ri.pool)
        .await?;
        files.extend(fs.into_iter().map(|(path, lines, rank)| {
            (
                path,
                u32::try_from(lines).unwrap_or(u32::MAX),
                if total_rank > 0.0 {
                    (rank / total_rank) as f32
                } else {
                    0.0
                },
            )
        }));
        let key = if files.iter().any(|(f, _, _)| f == p) {
            p.clone()
        } else {
            prefix
        };
        let (r, n, imp) = store::unit_listing(&ri.pool, &key, LISTING_SYMBOLS as i64).await?;
        rows.extend(r);
        total += n;
        importers.extend(imp);
    }
    files.sort_by(|a, b| {
        b.2.partial_cmp(&a.2)
            .unwrap_or(std::cmp::Ordering::Equal)
            .then_with(|| a.0.cmp(&b.0))
    });
    files.dedup_by(|a, b| a.0 == b.0);
    rows.sort_by(|a, b| {
        b.rank
            .partial_cmp(&a.rank)
            .unwrap_or(std::cmp::Ordering::Equal)
    });
    rows.dedup_by(|a, b| a.moniker == b.moniker);
    rows.truncate(LISTING_SYMBOLS);
    let symbols: Vec<MapRow> = rows
        .iter()
        .map(|s| MapRow {
            moniker: s.moniker.clone(),
            path: s.path.clone(),
            line: s.line_start,
            line_end: s.line_end,
            kind: s.kind.clone(),
            name: s.name.clone(),
            signature: clip(&s.signature, SIGNATURE_CHARS, false),
            rank: s.rank,
            exported: s.exported,
            importers: u32::try_from(importers.get(&s.path).copied().unwrap_or(0))
                .unwrap_or(u32::MAX),
            callers: u32::try_from(s.callers).unwrap_or(u32::MAX),
            recent: s.mtime_ns >= now_ns - rank::RECENT_NS,
            entry: entries.contains(&s.path),
            is_test: s.is_test,
        })
        .collect();
    Ok(MapListing {
        more: u32::try_from(
            total
                .saturating_sub(i64::try_from(symbols.len()).unwrap_or(0))
                .max(0),
        )
        .unwrap_or(u32::MAX),
        files,
        symbols,
    })
}

/// `codemap_element`: one element, its diagram (or the structure view when
/// nothing generated), children with thumbnails, edges with evidence,
/// `also_in`, and the live listing for leaves and depth > [`super::MAX_DEPTH`].
pub async fn element(ix: &Codeindex, m: Element) -> anyhow::Result<MapElement> {
    let t = Instant::now();
    let (ri, pool) = opened(ix, &m.root).await?;
    let Some(row) = mstore::element(&pool, &m.element).await? else {
        anyhow::bail!("codemap: unknown element {}", m.element);
    };
    let total_rank: f64 = sqlx::query_scalar("SELECT coalesce(sum(rank), 0.0) FROM files")
        .fetch_one(&ri.pool)
        .await?;
    let mut crumbs: Vec<(String, String)> = vec![(row.id.clone(), row.name.clone())];
    let mut up = row.parent.clone();
    while let Some(pid) = up {
        let Some(p) = mstore::element(&pool, &pid).await? else {
            break;
        };
        crumbs.push((p.id.clone(), p.name.clone()));
        up = p.parent;
    }
    crumbs.reverse();
    let stored = mstore::anchors_of(&pool, &row.id).await?;
    let mut anchors = Vec::with_capacity(stored.len());
    for (p, s, e) in &stored {
        let (n, _, _) = anchor_stats(&ri.pool, p).await?;
        anchors.push(MapAnchor {
            path: p.clone(),
            line_start: *s,
            line_end: *e,
            exists: n > 0,
        });
    }
    let mut children = Vec::new();
    for c in mstore::children_of(&pool, &row.id).await? {
        let a = mstore::anchors_of(&pool, &c.id).await?;
        children.push(child_of(&ri.pool, total_rank, &c, &a).await?);
    }
    let diagram = row.diagram();
    let mermaid = diagram.as_ref().map(surface::graph::to_mermaid);
    let listing = if row.leaf {
        let entries: HashSet<String> = store::meta(&ri.pool, "entry_points")
            .await?
            .and_then(|s| serde_json::from_str::<Vec<String>>(&s).ok())
            .unwrap_or_default()
            .into_iter()
            .collect();
        Some(listing(&ri, &stored, total_rank, &entries).await?)
    } else {
        None
    };
    Ok(MapElement {
        meta: meta(&ri, &pool, t).await?,
        id: row.id.clone(),
        node: row.node.clone(),
        perspective: row.perspective.clone(),
        parent: row.parent.clone(),
        crumbs,
        name: row.name.clone(),
        path: row.path.clone(),
        kind: row.kind.clone(),
        members: row.members(),
        anchors,
        fields: row.fields(),
        edges: mstore::edges_of(&pool, &row.id).await?,
        diagram,
        mermaid,
        children,
        also_in: mstore::also_in(&pool, &row.node, &row.id).await?,
        stale: row.stale,
        origin: row.origin(),
        provenance: row.provenance(),
        listing,
    })
}

/// `codemap_search`: `elements.name LIKE` for a name, `anchors_path` prefix
/// match for a path → the element chain (§11.3, §14 < 20 ms).
pub async fn search(ix: &Codeindex, m: Search) -> anyhow::Result<Vec<MapHit>> {
    let (ri, pool) = opened(ix, &m.root).await?;
    let q = m.query.trim();
    let limit = i64::from(m.limit.unwrap_or(SEARCH_LIMIT).clamp(1, 200));
    if q.is_empty() {
        return Ok(Vec::new());
    }
    let q = q
        .strip_prefix(&format!("{}/", ri.root.display()))
        .unwrap_or(q);
    let mut rows: Vec<ElementRow> = Vec::new();
    if q.contains('/') || q.contains('.') {
        rows.extend(mstore::elements_anchoring(&pool, q.trim_end_matches('/'), limit).await?);
    }
    rows.extend(mstore::search_names(&pool, q, limit).await?);
    let mut seen = HashSet::new();
    rows.retain(|r| seen.insert(r.id.clone()));
    rows.truncate(usize::try_from(limit).unwrap_or(usize::MAX));
    let by_id: HashMap<String, ElementRow> = mstore::all_elements(&pool)
        .await?
        .into_iter()
        .map(|e| (e.id.clone(), e))
        .collect();
    Ok(rows
        .into_iter()
        .map(|r| {
            let mut chain = vec![r.id.clone()];
            let mut up = r.parent.clone();
            while let Some(pid) = up {
                chain.push(pid.clone());
                up = by_id.get(&pid).and_then(|p| p.parent.clone());
            }
            chain.reverse();
            MapHit {
                element: r.id,
                name: r.name,
                path: r.path,
                chain,
            }
        })
        .collect())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn facts_hash_is_order_free_and_sensitive_to_sync_keys() {
        let a = ("a.rs".to_owned(), 1_i64, 10_i64);
        let b = ("b.rs".to_owned(), 2_i64, 20_i64);
        let e = (
            "u1".to_owned(),
            "u2".to_owned(),
            "imports".to_owned(),
            3_u32,
        );
        let h1 = facts_hash(&[a.clone(), b.clone()], std::slice::from_ref(&e));
        let h2 = facts_hash(&[b.clone(), a.clone()], std::slice::from_ref(&e));
        assert_eq!(h1, h2);
        assert_eq!(h1.len(), 64);
        let touched = ("a.rs".to_owned(), 5_i64, 10_i64);
        assert_ne!(
            h1,
            facts_hash(&[touched, b.clone()], std::slice::from_ref(&e))
        );
        let recount = (
            "u1".to_owned(),
            "u2".to_owned(),
            "imports".to_owned(),
            4_u32,
        );
        assert_ne!(h1, facts_hash(&[a, b], &[recount]));
    }

    #[test]
    fn clip_marks_the_cut() {
        assert_eq!(clip("abc", 5, false), "abc");
        assert_eq!(clip("abcdefgh", 5, false), "abcd…");
        assert_eq!(clip("abcdefgh", 5, true), "…efgh");
    }

    use std::sync::Arc;

    use crate::{Codeindex, CodeindexConfig};

    struct Temp(std::path::PathBuf);
    impl Drop for Temp {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.0);
        }
    }
    fn temp(tag: &str) -> Temp {
        let d =
            std::env::temp_dir().join(format!("theuth-skeleton-{tag}-{}", uuid::Uuid::now_v7()));
        std::fs::create_dir_all(&d).unwrap();
        Temp(d)
    }
    fn write(root: &Path, rel: &str, body: &str) {
        let p = root.join(rel);
        std::fs::create_dir_all(p.parent().unwrap()).unwrap();
        std::fs::write(p, body).unwrap();
    }

    /// A Cargo workspace of `n` crates; `c0` depends on `c1..=c3` by path and
    /// uses them, `c1` uses `c2`.
    fn workspace(root: &Path, n: usize) {
        write(
            root,
            "Cargo.toml",
            "[workspace]\nmembers = [\"crates/*\"]\nresolver = \"2\"\n",
        );
        for i in 0..n {
            let deps: String = match i {
                0 => (1..=3.min(n - 1))
                    .map(|j| format!("c{j} = {{ path = \"../c{j}\" }}\n"))
                    .collect(),
                1 if n > 2 => "c2 = { path = \"../c2\" }\n".to_owned(),
                _ => String::new(),
            };
            write(
                root,
                &format!("crates/c{i}/Cargo.toml"),
                &format!(
                    "[package]\nname = \"c{i}\"\nversion = \"0.1.0\"\nedition = \"2021\"\n\n[dependencies]\n{deps}"
                ),
            );
            let uses: String = match i {
                0 => (1..=3.min(n - 1))
                    .map(|j| format!("use c{j}::f{j};\npub fn call{j}() {{ f{j}(); }}\n"))
                    .collect(),
                1 if n > 2 => "use c2::f2;\npub fn call2() { f2(); }\n".to_owned(),
                _ => String::new(),
            };
            write(
                root,
                &format!("crates/c{i}/src/lib.rs"),
                &format!("{uses}pub fn f{i}() -> u32 {{ {i} }}\npub struct S{i};\n"),
            );
            write(
                root,
                &format!("crates/c{i}/src/util.rs"),
                &format!("pub fn util{i}() {{}}\n"),
            );
            write(
                root,
                &format!("crates/c{i}/src/extra.rs"),
                &format!("pub fn extra{i}() {{}}\n"),
            );
            // The referenced crates are also the big ones (7 files): their
            // share puts them in the kept twelve whatever the rank does with
            // referenced-but-dangling files.
            if (1..=3).contains(&i) {
                for k in 0..4 {
                    write(
                        root,
                        &format!("crates/c{i}/src/more{k}.rs"),
                        &format!("pub fn more{i}_{k}() {{}}\n"),
                    );
                }
            }
        }
    }

    async fn indexed(data: &Path, root: &Path) -> (Arc<Codeindex>, Arc<RootIndex>) {
        let ix = Arc::new(Codeindex::new(CodeindexConfig::default(), data).unwrap());
        let root = root.canonicalize().unwrap();
        let ri = ix.open_root(&root).await.unwrap();
        let r = ix.reconcile(&ri, true).await.unwrap();
        assert!(r.reparsed > 0, "{r:?}");
        (ix, ri)
    }

    fn validate(g: &GraphProps, what: &str) {
        let body = serde_json::json!({ "root": "g", "elements": { "g": { "type": "Graph", "props": g } } });
        if let Err(errs) = surface::parse(&body.to_string()) {
            panic!("{what}: {errs:?}");
        }
    }

    #[tokio::test]
    async fn skeleton_caps_and_bundles() {
        let (data, repo) = (temp("data"), temp("repo"));
        workspace(&repo.0, 15);
        let (ix, ri) = indexed(&data.0, &repo.0).await;
        assert!(!ix.has_map(&ri), "no map before the first open");
        let ov = overview(
            &ix,
            Overview {
                root: ri.root.clone(),
            },
        )
        .await
        .unwrap();
        assert!(ix.has_map(&ri));
        assert_eq!(ov.meta.state, MapState::Structure);
        assert_eq!(ov.perspectives.len(), 1);
        let p = &ov.perspectives[0];
        assert_eq!(p.id, STRUCTURE_PERSPECTIVE);
        assert_eq!(p.origin, MapOrigin::Structure);
        let root_row = &p.tree[0];
        assert_eq!((root_row.depth, root_row.kind.as_str()), (0, "perspective"));
        assert_eq!(
            root_row.children,
            DIAGRAM_NODES as u32 + 1,
            "12 units + one bundle"
        );
        let top: Vec<&MapTreeRow> = p.tree.iter().filter(|r| r.depth == 1).collect();
        assert_eq!(top.len(), 13);
        let bundle = top.iter().find(|r| r.kind == "bundle").expect("bundle row");
        assert_eq!(bundle.name, "+3 more");
        assert_eq!(
            bundle.node,
            format!("bundle:perspective:{STRUCTURE_PERSPECTIVE}")
        );
        assert!(
            top.iter().filter(|r| r.kind == "unit").all(|r| r
                .path
                .as_deref()
                .unwrap()
                .starts_with("crates/c"))
        );
        // c1 and c2 are referenced (c0 → c1..c3, c1 → c2): they rank among the kept twelve.
        let paths: Vec<&str> = top.iter().filter_map(|r| r.path.as_deref()).collect();
        assert!(paths.contains(&"crates/c1"), "{paths:?}");
        assert!(paths.contains(&"crates/c2"), "{paths:?}");

        let root = element(
            &ix,
            Element {
                root: ri.root.clone(),
                element: STRUCTURE_PERSPECTIVE.to_owned(),
            },
        )
        .await
        .unwrap();
        let g = root.diagram.as_ref().expect("structure diagram");
        assert_eq!(g.nodes.len(), 13);
        assert!(g.nodes.len() <= super::super::DIAGRAM_NODES_HARD);
        validate(g, "root diagram");
        assert!(root.mermaid.as_deref().unwrap().starts_with("flowchart"));
        assert_eq!(root.children.len(), 13);
        let bundle_child = root.children.iter().find(|c| c.kind == "bundle").unwrap();
        assert_eq!(bundle_child.files, 9, "3 crates × 3 files");
        assert!(bundle_child.leaf && bundle_child.thumbnail.is_none());
        let c1 = root
            .edges
            .iter()
            .filter(|e| e.from == "unit:crates/c1")
            .collect::<Vec<_>>();
        assert_eq!(c1.len(), 1, "{:?}", root.edges);
        assert_eq!(c1[0].to, "unit:crates/c2");
        for e in &c1 {
            assert!(e.backed);
            assert!(
                e.evidence.iter().any(
                    |v| matches!(v, MapEvidence::Manifest { kind, .. } if kind == "cargo-path")
                ),
                "{e:?}"
            );
            assert!(
                e.evidence
                    .iter()
                    .any(|v| matches!(v, MapEvidence::Imports { count, .. } if *count >= 1)),
                "{e:?}"
            );
            assert!(
                e.label.contains("1 Cargo dep") && e.label.contains("import"),
                "{}",
                e.label
            );
        }
        // Children carry thumbnails; a crate's drill-down is its collapsed `src/`.
        let unit = root
            .children
            .iter()
            .find(|c| c.path.as_deref() == Some("crates/c1"))
            .unwrap();
        assert_eq!((unit.files, unit.leaf), (7, false));
        let thumb = unit.thumbnail.as_ref().expect("thumbnail");
        assert_eq!(thumb.nodes.len(), 7);
        validate(thumb, "c1 thumbnail");
        let c0 = element(
            &ix,
            Element {
                root: ri.root.clone(),
                element: unit.id.clone(),
            },
        )
        .await
        .unwrap();
        assert_eq!(c0.crumbs.len(), 2);
        assert_eq!(c0.anchors[0].path, "crates/c1");
        assert!(c0.anchors[0].exists);
        assert!(c0.children.iter().all(|c| c.kind == "file" && c.leaf));
        // A file is a leaf with a listing.
        let lib = c0
            .children
            .iter()
            .find(|c| c.path.as_deref() == Some("crates/c1/src/lib.rs"))
            .unwrap();
        let lib = element(
            &ix,
            Element {
                root: ri.root.clone(),
                element: lib.id.clone(),
            },
        )
        .await
        .unwrap();
        let listing = lib.listing.expect("leaf listing");
        assert_eq!(listing.files.len(), 1);
        assert!(
            listing.symbols.iter().any(|s| s.name == "f1"),
            "{:?}",
            listing.symbols
        );
        assert!(lib.diagram.is_none());
        // The bundle lists its members' files.
        let b = element(
            &ix,
            Element {
                root: ri.root.clone(),
                element: bundle_child.id.clone(),
            },
        )
        .await
        .unwrap();
        assert_eq!(b.members.len(), 3);
        assert_eq!(b.listing.unwrap().files.len(), 9);
        // Search by name and by path.
        let hits = search(
            &ix,
            Search {
                root: ri.root.clone(),
                query: "c1".into(),
                limit: None,
            },
        )
        .await
        .unwrap();
        assert!(
            hits.iter().any(|h| h.element == unit.id
                && h.chain == [STRUCTURE_PERSPECTIVE.to_owned(), unit.id.clone()]),
            "{hits:?}"
        );
        let hits = search(
            &ix,
            Search {
                root: ri.root.clone(),
                query: "crates/c1/src/util.rs".into(),
                limit: Some(5),
            },
        )
        .await
        .unwrap();
        assert_eq!(
            hits[0].path.as_deref(),
            Some("crates/c1/src/util.rs"),
            "{hits:?}"
        );
        assert_eq!(hits[0].chain.len(), 3);
        // A generated root keeps its bundle: recompute re-plans the bundle row
        // (its files, anchors, hash) instead of dropping it.
        let pool = ix.map_pool(&ri).await.unwrap();
        sqlx::query("UPDATE elements SET origin = 'generated' WHERE id = ?")
            .bind(STRUCTURE_PERSPECTIVE)
            .execute(&pool)
            .await
            .unwrap();
        recompute(&ix, &ri).await.unwrap();
        let root = element(
            &ix,
            Element {
                root: ri.root.clone(),
                element: STRUCTURE_PERSPECTIVE.to_owned(),
            },
        )
        .await
        .unwrap();
        assert_eq!(root.children.len(), 13, "{:?}", root.children);
        let b = root.children.iter().find(|c| c.kind == "bundle").unwrap();
        assert_eq!((b.files, b.id.as_str()), (9, bundle_child.id.as_str()));
    }

    #[tokio::test]
    async fn facts_hash_changes_on_touch() {
        let (data, repo) = (temp("data"), temp("repo"));
        workspace(&repo.0, 3);
        let (ix, ri) = indexed(&data.0, &repo.0).await;
        overview(
            &ix,
            Overview {
                root: ri.root.clone(),
            },
        )
        .await
        .unwrap();
        let pool = ix.map_pool(&ri).await.unwrap();
        let id = element_id(STRUCTURE_PERSPECTIVE, "unit:crates/c1");
        let before = mstore::element(&pool, &id).await.unwrap().unwrap();
        assert!(!before.stale);
        // Same bytes re-written: `(mtime_ns, size)` moves, the hash moves (§5.4).
        tokio::time::sleep(std::time::Duration::from_millis(20)).await;
        let lib = "crates/c1/src/lib.rs";
        let src = std::fs::read_to_string(repo.0.join(lib)).unwrap();
        std::fs::write(
            repo.0.join(lib),
            format!("{src}\npub fn touched_1() {{}}\n"),
        )
        .unwrap();
        let r = ix.touch(&ri, &[lib.to_owned()]).await.unwrap();
        assert_eq!(r.reparsed, 1);
        let after = mstore::element(&pool, &id).await.unwrap().unwrap();
        assert_ne!(before.facts_hash, after.facts_hash);
        assert!(
            !after.stale,
            "a structure element is rewritten, never stale"
        );
        let untouched = element_id(STRUCTURE_PERSPECTIVE, "unit:crates/c2");
        let other = mstore::element(&pool, &untouched).await.unwrap().unwrap();
        assert_eq!(
            other.facts_hash,
            mstore::element(&pool, &untouched)
                .await
                .unwrap()
                .unwrap()
                .facts_hash
        );
        // A model-written element goes stale instead (§8).
        sqlx::query("UPDATE elements SET origin = 'generated', description = 'The c1 crate.', model = 'test' WHERE id = ?")
            .bind(&id)
            .execute(&pool)
            .await
            .unwrap();
        tokio::time::sleep(std::time::Duration::from_millis(20)).await;
        std::fs::write(
            repo.0.join(lib),
            format!("{src}\npub fn touched_2() {{}}\n"),
        )
        .unwrap();
        ix.touch(&ri, &[lib.to_owned()]).await.unwrap();
        let written = mstore::element(&pool, &id).await.unwrap().unwrap();
        assert!(written.stale, "hash moved → stale");
        assert_eq!(
            written.facts_hash, after.facts_hash,
            "the generation's hash stays until regenerated"
        );
        assert_eq!(written.description.as_deref(), Some("The c1 crate."));
        let ov = overview(
            &ix,
            Overview {
                root: ri.root.clone(),
            },
        )
        .await
        .unwrap();
        assert_eq!(ov.meta.stale_elements, 1);
        assert_eq!(
            ov.meta.state,
            MapState::Structure,
            "no generated perspective yet"
        );
        assert_eq!(ov.perspectives[0].stale, 1);
        assert!(
            ov.perspectives[0]
                .tree
                .iter()
                .any(|r| r.id == id && r.stale)
        );
    }

    #[tokio::test]
    async fn touch_burst_defers_rank_persist() {
        let (data, repo) = (temp("data"), temp("repo"));
        workspace(&repo.0, 8);
        let (ix, ri) = indexed(&data.0, &repo.0).await;
        overview(
            &ix,
            Overview {
                root: ri.root.clone(),
            },
        )
        .await
        .unwrap();
        // A touched file is deleted and re-inserted: `files.rank` is 0.0 until
        // the next persist — the observable the deferral leaves behind.
        let rank_of = |path: &str| {
            let pool = ri.pool.clone();
            let path = path.to_owned();
            async move {
                sqlx::query_scalar::<_, f64>("SELECT rank FROM files WHERE path = ?")
                    .bind(path)
                    .fetch_one(&pool)
                    .await
                    .unwrap()
            }
        };
        let mut ms: Vec<u128> = Vec::new();
        for i in 0..6 {
            let rel = format!("crates/c{i}/src/util.rs");
            let src = std::fs::read_to_string(repo.0.join(&rel)).unwrap();
            std::fs::write(repo.0.join(&rel), format!("{src}pub fn burst{i}() {{}}\n")).unwrap();
            let t = Instant::now();
            let r = ix.touch(&ri, &[rel]).await.unwrap();
            ms.push(t.elapsed().as_millis());
            assert_eq!(r.reparsed, 1);
        }
        // The first four touches persisted the rank; the fifth and sixth were a burst.
        assert_eq!(
            store::meta(&ri.pool, crate::index::RANK_DEFERRED)
                .await
                .unwrap()
                .as_deref(),
            Some("1")
        );
        assert!(
            rank_of("crates/c0/src/util.rs").await > 0.0,
            "persisted before the burst"
        );
        assert_eq!(
            rank_of("crates/c5/src/util.rs").await,
            0.0,
            "not ranked during the burst"
        );
        let t = Instant::now();
        while store::meta(&ri.pool, crate::index::RANK_DEFERRED)
            .await
            .unwrap()
            .as_deref()
            != Some("0")
        {
            assert!(
                t.elapsed() < std::time::Duration::from_secs(10),
                "trailing rank never ran"
            );
            tokio::time::sleep(std::time::Duration::from_millis(50)).await;
        }
        assert!(
            rank_of("crates/c5/src/util.rs").await > 0.0,
            "the trailing run ranked it"
        );
        // The skeleton rode the same run: the touched file's element moved.
        let pool = ix.map_pool(&ri).await.unwrap();
        let c5 = mstore::element(&pool, &element_id(STRUCTURE_PERSPECTIVE, "unit:crates/c5"))
            .await
            .unwrap()
            .unwrap();
        assert!(c5.facts_hash.is_some());
        let burst_ms = ms[4..].iter().sum::<u128>() / 2;
        let plain_ms = ms[..4].iter().sum::<u128>() / 4;
        println!(
            "touch_burst_defers_rank_persist: plain {plain_ms} ms/file, burst {burst_ms} ms/file, per touch {ms:?}"
        );
        assert!(
            burst_ms <= plain_ms.max(100),
            "a burst touch must not cost the rank: {ms:?}"
        );
    }

    /// `cargo test -p codeindex structure_view_of_real_repos -- --ignored --nocapture`:
    /// index `~/Develop/theuth` and `~/Develop/oh-my-mermaid` into a scratch data
    /// dir and print their structure views (147 §15 W1 proof).
    #[tokio::test(flavor = "multi_thread", worker_threads = 4)]
    #[ignore = "indexes two real repositories"]
    async fn structure_view_of_real_repos() {
        let home = std::env::var("HOME").unwrap();
        for repo in ["theuth", "oh-my-mermaid"] {
            let root = Path::new(&home).join("Develop").join(repo);
            if !root.is_dir() {
                println!("{repo}: not present, skipped");
                continue;
            }
            let data = temp("real");
            let ix = Arc::new(Codeindex::new(CodeindexConfig::default(), &data.0).unwrap());
            let root = root.canonicalize().unwrap();
            let ri = ix.open_root(&root).await.unwrap();
            let t = Instant::now();
            let r = ix.reconcile(&ri, true).await.unwrap();
            let index_ms = t.elapsed().as_millis();
            let c = store::counts(&ri.pool).await.unwrap();
            let t = Instant::now();
            let ov = overview(
                &ix,
                Overview {
                    root: ri.root.clone(),
                },
            )
            .await
            .unwrap();
            let overview_ms = t.elapsed().as_millis();
            let map_pool = ix.map_pool(&ri).await.unwrap();
            let skeleton_ms = mstore::meta(&map_pool, "skeleton_ms")
                .await
                .unwrap()
                .unwrap_or_default();
            let phases = mstore::meta(&map_pool, "skeleton_phases_ms")
                .await
                .unwrap()
                .unwrap_or_default();
            let p = &ov.perspectives[0];
            let top: Vec<&MapTreeRow> = p.tree.iter().filter(|r| r.depth == 1).collect();
            let bundle = top
                .iter()
                .find(|r| r.kind == "bundle")
                .map(|r| r.name.clone());
            let root_el = element(
                &ix,
                Element {
                    root: ri.root.clone(),
                    element: p.id.clone(),
                },
            )
            .await
            .unwrap();
            let g = root_el.diagram.as_ref().unwrap();
            validate(g, repo);
            println!("== {repo} ({})", root.display());
            println!(
                "  index: {} files ({} parsed), {} symbols, {} refs, imports {} ({} unresolved), reparsed {} in {index_ms} ms",
                c.files,
                c.parsed_files,
                c.symbols,
                c.refs,
                c.imports,
                c.unresolved_imports,
                r.reparsed
            );
            println!(
                "  skeleton: {} elements in {} ({skeleton_ms} ms pass: {phases}; overview {overview_ms} ms); top level {} nodes, bundle {:?}",
                p.elements,
                p.id,
                top.len(),
                bundle
            );
            println!(
                "  root diagram: {} nodes, {} edges drawn; legend {:?}",
                g.nodes.len(),
                g.edges.len(),
                g.legend
                    .as_ref()
                    .map(|l| l.iter().map(|e| e.label.clone()).collect::<Vec<_>>())
            );
            for row in &top {
                println!(
                    "    {:<40} {:<6} children {} leaf {}",
                    row.path.clone().unwrap_or_else(|| row.name.clone()),
                    row.kind,
                    row.children,
                    row.leaf
                );
            }
            for e in root_el.edges.iter().take(12) {
                println!("    edge {} → {}: {}", e.from, e.to, e.label);
            }
            let detections: Vec<detect::Detection> = serde_json::from_str(
                &mstore::meta(&ix.map_pool(&ri).await.unwrap(), "detections")
                    .await
                    .unwrap()
                    .unwrap(),
            )
            .unwrap();
            println!(
                "  eligible perspectives: {:?}",
                detections
                    .iter()
                    .filter(|d| d.eligible)
                    .map(|d| format!("{} ({})", d.kind, d.hits))
                    .collect::<Vec<_>>()
            );
            if repo == "oh-my-mermaid" {
                assert!(
                    c.imports - c.unresolved_imports > 100,
                    "omm must resolve > 100 of its imports: {} of {}",
                    c.imports - c.unresolved_imports,
                    c.imports
                );
            }
        }
    }
}
