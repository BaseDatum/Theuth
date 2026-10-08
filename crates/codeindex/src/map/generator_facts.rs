//! The facts the generator hands the model (147 §5 as the generator reads
//! it, §6.2 inputs): units with sizes, unit→unit edges with evidence, the
//! perspective detectors, drill-down sub-skeletons (§5.5) and entry-point
//! call chains (stage D). Everything comes from the index pool — `files`,
//! `imports`, `refs`, `symbols`, `meta` — plus the Cargo manifests the
//! resolver already models; nothing is walked, no source file is read.
//!
//! owner: W2 — design §5.1–§5.5 (as consumed by §6), §6.2

use std::collections::{BTreeMap, BTreeSet, HashMap};
use std::path::{Path, PathBuf};

use serde::Serialize;
use sqlx::SqlitePool;

use super::detect::{self, Detection, PERSPECTIVES};
use super::skeleton::{self, EDGE_MIN_SHARE, LARGE_FOLDER_FILES, LEAF_FILES, REF_MIN_CONFIDENCE};
use super::{DIAGRAM_NODES, MapEvidence};
use crate::resolve::rust::CargoWorkspace;
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
pub const CONCERN_FILE_LINES: u32 = 5_000;
/// In-degree over this trips a `concern` on a non-foundation unit (§4.3).
pub const CONCERN_IN_DEGREE: usize = 15;

/// One `files` row the facts aggregate over.
#[derive(Clone, Debug)]
pub struct FileRow {
    /// Root-relative path.
    pub path: String,
    /// Sync key.
    pub mtime_ns: i64,
    /// Sync key.
    pub size: i64,
    /// `files.lines`.
    pub lines: u32,
    /// `language <> 'other'`.
    pub parsed: bool,
    /// `files.is_test`.
    pub test: bool,
    /// `files.rank`.
    pub rank: f64,
}

/// File→file evidence aggregated from `imports` and `refs`.
#[derive(Clone, Copy, Debug, Default)]
pub struct FileEdge {
    /// Distinct `imports` rows.
    pub imports: u32,
    /// Line of one import.
    pub import_line: i64,
    /// `refs` rows with confidence ≥ [`REF_MIN_CONFIDENCE`].
    pub refs: u32,
    /// Line of one ref.
    pub ref_line: i64,
    /// Σ confidence over `refs`.
    pub confidence: f64,
}

/// One unit (§5.1): a package, a top-level directory, a large sub-folder.
#[derive(Clone, Debug, Serialize)]
pub struct Unit {
    /// `unit:<path>`.
    pub node: String,
    /// Root-relative directory.
    pub path: String,
    /// Basename.
    pub name: String,
    /// Parsed files under it.
    pub files: u32,
    /// Σ `files.lines`.
    pub lines: u32,
    /// Test files under it.
    pub tests: u32,
    /// Σ rank / Σ all, 0–1.
    pub share: f32,
    /// An entry point lives inside.
    pub entry: bool,
    /// A file changed within `rank::RECENT_NS`.
    pub recent: bool,
    /// The enclosing unit's path when nested (a large sub-folder).
    pub parent: Option<String>,
}

impl Unit {
    /// ≤ [`LEAF_FILES`] parsed files: a door to a file list, not a diagram.
    pub fn leaf(&self) -> bool {
        self.files <= LEAF_FILES
    }
}

/// One aggregated edge between two nodes with its evidence.
#[derive(Clone, Debug)]
pub struct UnitEdge {
    /// Node id.
    pub from: String,
    /// Node id.
    pub to: String,
    /// Imports + refs rows (+1 per manifest dep): the thresholding weight.
    pub weight: u32,
    /// What the index holds.
    pub evidence: Vec<MapEvidence>,
}

impl UnitEdge {
    /// `manifest | imports | refs | call | bindings | process` of the strongest evidence.
    pub fn kind(&self) -> &'static str {
        self.evidence.first().map_or("inferred", evidence_kind)
    }
}

/// The stored string of one evidence kind (`facts_hash` edge tuples, prompts).
pub fn evidence_kind(e: &MapEvidence) -> &'static str {
    match e {
        MapEvidence::Manifest { .. } => "manifest",
        MapEvidence::Imports { .. } => "imports",
        MapEvidence::Refs { .. } => "refs",
        MapEvidence::Call { .. } => "call",
        MapEvidence::Bindings { .. } => "bindings",
        MapEvidence::Process { .. } => "process",
        MapEvidence::Inferred { .. } => "inferred",
    }
}

/// The count rendered beside a label (`334 Cargo deps`, `37 imports`).
pub fn evidence_summary(evidence: &[MapEvidence]) -> String {
    let mut manifests = 0;
    let mut imports = 0;
    let mut refs = 0;
    let mut other = Vec::new();
    for e in evidence {
        match e {
            MapEvidence::Manifest { .. } => manifests += 1,
            MapEvidence::Imports { count, .. } => imports += count,
            MapEvidence::Refs { count, .. } => refs += count,
            MapEvidence::Call { site, .. } => other.push(format!("call {site}")),
            MapEvidence::Bindings { generator, .. } => other.push(format!("{generator} bindings")),
            MapEvidence::Process { verb, .. } => other.push(format!("process {verb}")),
            MapEvidence::Inferred { reason } => other.push(format!("inferred: {reason}")),
        }
    }
    let mut parts = Vec::new();
    if manifests > 0 {
        parts.push(format!("{manifests} manifest dep{}", plural(manifests)));
    }
    if imports > 0 {
        parts.push(format!("{imports} import{}", plural(imports)));
    }
    if refs > 0 {
        parts.push(format!("{refs} ref{}", plural(refs)));
    }
    parts.extend(other);
    parts.join(" · ")
}

fn plural(n: u32) -> &'static str {
    if n == 1 { "" } else { "s" }
}

/// One child of an element in the drill-down structure (§5.5).
#[derive(Clone, Debug, Serialize)]
pub struct Child {
    /// `unit:<path>` or `sym:<moniker>`.
    pub node: String,
    /// Basename (symbol name for a symbol).
    pub name: String,
    /// Root-relative path.
    pub path: String,
    /// `unit | file | symbol`.
    pub kind: &'static str,
    /// Parsed files under it (1 for a file).
    pub files: u32,
    /// Σ lines.
    pub lines: u32,
    /// Σ rank / Σ all.
    pub share: f32,
    /// No diagram of its own.
    pub leaf: bool,
    /// Symbol range, for `sym:` children.
    pub range: Option<(i64, i64)>,
}

/// The index-decided structure below one element.
#[derive(Clone, Debug, Default)]
pub struct SubSkeleton {
    /// ≤ [`DIAGRAM_NODES`] children by size.
    pub children: Vec<Child>,
    /// The remainder, counted into one bundle.
    pub bundled: Vec<Child>,
    /// Edges among `children`.
    pub edges: Vec<UnitEdge>,
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
#[derive(Debug)]
pub struct Facts {
    /// Canonical root.
    pub root: PathBuf,
    /// `profile::render` of the root.
    pub profile: String,
    /// `map_state` as `"<files>:<max mtime_ns>"`.
    pub index_state: String,
    /// `meta.git_head`.
    pub git_head: Option<String>,
    /// Every unit, top-level and nested, by share descending.
    pub units: Vec<Unit>,
    /// Edges among top-level units after thresholding (§5.2).
    pub edges: Vec<UnitEdge>,
    /// Edges folded into "+N minor edges" by the threshold.
    pub folded: u32,
    /// The twelve detectors.
    pub detections: Vec<Detection>,
    /// `(title, description)` of imported perspectives (§13.2).
    pub imported: Vec<(String, String)>,
    /// `meta.entry_points`.
    pub entry_points: Vec<String>,
    files: Vec<FileRow>,
    /// `(from file index, to file index)` → evidence.
    file_edges: HashMap<(usize, usize), FileEdge>,
    /// Cargo path deps: package dir → package dir.
    manifest_deps: Vec<(String, String)>,
    total_rank: f64,
}

impl Facts {
    /// Read the index (and the map store's imported perspectives) for `root`.
    pub async fn load(index: &SqlitePool, map: &SqlitePool, root: &Path) -> anyhow::Result<Self> {
        let rows: Vec<FileSql> = sqlx::query_as(
            "SELECT id, path, mtime_ns, size, language, lines, is_test, rank FROM files ORDER BY path",
        )
        .fetch_all(index)
        .await?;
        let mut by_id = HashMap::with_capacity(rows.len());
        let files: Vec<FileRow> = rows
            .into_iter()
            .enumerate()
            .map(
                |(i, (id, path, mtime_ns, size, language, lines, is_test, rank))| {
                    by_id.insert(id, i);
                    FileRow {
                        path,
                        mtime_ns,
                        size,
                        lines: u32::try_from(lines).unwrap_or(u32::MAX),
                        parsed: language != "other",
                        test: is_test != 0,
                        rank,
                    }
                },
            )
            .collect();
        let total_rank: f64 = files.iter().map(|f| f.rank).sum();

        let mut file_edges: HashMap<(usize, usize), FileEdge> = HashMap::new();
        let imports: Vec<(i64, i64, i64, i64)> = sqlx::query_as(
            "SELECT file_id, target_file, count(*), min(line) FROM imports \
             WHERE target_file IS NOT NULL AND target_file <> file_id AND specifier NOT LIKE 'mod:%' \
             GROUP BY file_id, target_file",
        )
        .fetch_all(index)
        .await?;
        for (a, b, n, line) in imports {
            if let (Some(&a), Some(&b)) = (by_id.get(&a), by_id.get(&b)) {
                let e = file_edges.entry((a, b)).or_default();
                e.imports = u32::try_from(n).unwrap_or(u32::MAX);
                e.import_line = line;
            }
        }
        let refs: Vec<(i64, i64, i64, i64, f64)> = sqlx::query_as(
            "SELECT r.file_id, s.file_id, count(*), min(r.line), sum(r.confidence) FROM refs r \
             JOIN symbols s ON s.id = r.to_symbol \
             WHERE r.to_symbol IS NOT NULL AND r.confidence >= ? AND s.file_id <> r.file_id \
             GROUP BY r.file_id, s.file_id",
        )
        .bind(REF_MIN_CONFIDENCE)
        .fetch_all(index)
        .await?;
        for (a, b, n, line, conf) in refs {
            if let (Some(&a), Some(&b)) = (by_id.get(&a), by_id.get(&b)) {
                let e = file_edges.entry((a, b)).or_default();
                e.refs = u32::try_from(n).unwrap_or(u32::MAX);
                e.ref_line = line;
                e.confidence = conf;
            }
        }

        let packages: Vec<(String, String)> = sqlx::query_as(
            "SELECT package, path FROM files WHERE package IS NOT NULL AND language <> 'other'",
        )
        .fetch_all(index)
        .await?;
        let entry_points: Vec<String> = store::meta(index, "entry_points")
            .await?
            .and_then(|s| serde_json::from_str(&s).ok())
            .unwrap_or_default();
        let (count, max_mtime) = store::map_state(index).await?;
        let git_head = store::meta(index, "git_head").await?;

        let cargo = CargoWorkspace::load(root);
        let cargo_dirs: HashMap<String, String> = cargo
            .iter()
            .flat_map(|ws| ws.packages.iter().map(|p| (p.name.clone(), p.dir.clone())))
            .collect();
        let manifest_deps = cargo
            .map(|ws| {
                let mut out: Vec<(String, String)> = ws
                    .packages
                    .iter()
                    .flat_map(|p| {
                        p.deps.values().filter_map(|q| {
                            let to = cargo_dirs.get(q.as_str())?;
                            (!p.dir.is_empty() && !to.is_empty() && p.dir != *to)
                                .then(|| (p.dir.clone(), to.clone()))
                        })
                    })
                    .collect();
                out.sort_unstable();
                out.dedup();
                out
            })
            .unwrap_or_default();

        let detections = match detect::perspectives(index).await {
            Ok(d) => d,
            Err(e) => {
                tracing::warn!(
                    "codemap: detectors unavailable ({e:#}); only overall-architecture is eligible"
                );
                Vec::new()
            }
        };
        let imported: Vec<(String, Option<String>)> = sqlx::query_as(
            "SELECT p.title, (SELECT e.description FROM elements e WHERE e.id = p.id) FROM perspectives p \
             WHERE p.origin = 'imported' ORDER BY p.order_no",
        )
        .fetch_all(map)
        .await
        .unwrap_or_default();

        let mut facts = Self {
            root: root.to_path_buf(),
            profile: crate::profile::render(&crate::profile::detect(root)),
            index_state: format!("{count}:{max_mtime}"),
            git_head,
            units: Vec::new(),
            edges: Vec::new(),
            folded: 0,
            detections: with_architecture(detections),
            imported: imported
                .into_iter()
                .map(|(t, d)| (t, d.unwrap_or_default()))
                .collect(),
            entry_points,
            files,
            file_edges,
            manifest_deps,
            total_rank,
        };
        facts.units = facts.discover_units(&packages, &cargo_dirs);
        let (edges, folded) = facts.thresholded_edges();
        facts.edges = edges;
        facts.folded = folded;
        Ok(facts)
    }

    /// A facts set over hand-built rows (tests, no pool).
    #[cfg(test)]
    #[allow(clippy::too_many_arguments)]
    pub(crate) fn synthetic(
        root: &Path,
        files: Vec<FileRow>,
        file_edges: Vec<(usize, usize, FileEdge)>,
        manifest_deps: Vec<(String, String)>,
        packages: Vec<(String, String)>,
        cargo_dirs: HashMap<String, String>,
        detections: Vec<Detection>,
        entry_points: Vec<String>,
    ) -> Self {
        let total_rank = files.iter().map(|f| f.rank).sum();
        let mut facts = Self {
            root: root.to_path_buf(),
            profile: "synthetic project".to_owned(),
            index_state: format!("{}:1", files.len()),
            git_head: Some("0000000".to_owned()),
            units: Vec::new(),
            edges: Vec::new(),
            folded: 0,
            detections: with_architecture(detections),
            imported: Vec::new(),
            entry_points,
            files,
            file_edges: file_edges
                .into_iter()
                .map(|(a, b, e)| ((a, b), e))
                .collect(),
            manifest_deps,
            total_rank,
        };
        facts.units = facts.discover_units(&packages, &cargo_dirs);
        let (edges, folded) = facts.thresholded_edges();
        facts.edges = edges;
        facts.folded = folded;
        facts
    }

    /// §5.1: packages (a Cargo member's manifest directory; else the shortest
    /// common directory of a `files.package`, minus a trailing `src`),
    /// top-level directories with parsed files that hold no package, large
    /// sub-folders (≥ [`LARGE_FOLDER_FILES`] parsed files) inside a unit.
    fn discover_units(
        &self,
        packages: &[(String, String)],
        cargo_dirs: &HashMap<String, String>,
    ) -> Vec<Unit> {
        let mut dirs: BTreeSet<String> = BTreeSet::new();
        let mut common: BTreeMap<&str, String> = BTreeMap::new();
        for (pkg, path) in packages {
            if let Some(dir) = cargo_dirs.get(pkg) {
                if !dir.is_empty() {
                    dirs.insert(dir.clone());
                }
                continue;
            }
            let dir = parent_dir(path);
            common
                .entry(pkg.as_str())
                .and_modify(|c| *c = common_prefix_dir(c, dir))
                .or_insert_with(|| dir.to_owned());
        }
        // ponytail: npm/python members are their files' common directory with a
        // trailing `src` dropped; read `package.json` workspaces if a monorepo splits oddly.
        dirs.extend(
            common
                .into_values()
                .map(|d| d.strip_suffix("/src").map_or(d.clone(), str::to_owned))
                .filter(|d| !d.is_empty()),
        );
        // Top-level directories with parsed files that are not a package's parent.
        let mut top: BTreeSet<&str> = BTreeSet::new();
        for f in self.files.iter().filter(|f| f.parsed) {
            if let Some((first, _)) = f.path.split_once('/') {
                top.insert(first);
            }
        }
        for t in top {
            let holds_package = dirs
                .iter()
                .any(|d| d == t || d.starts_with(&format!("{t}/")));
            if !holds_package {
                dirs.insert(t.to_owned());
            }
        }
        // Large sub-folders: every directory under a unit with ≥ LARGE_FOLDER_FILES parsed files.
        let mut counts: HashMap<String, u32> = HashMap::new();
        for f in self.files.iter().filter(|f| f.parsed) {
            let mut end = 0;
            while let Some(i) = f.path[end..].find('/') {
                end += i;
                *counts.entry(f.path[..end].to_owned()).or_default() += 1;
                end += 1;
            }
        }
        let base: Vec<String> = dirs.iter().cloned().collect();
        let mut large: Vec<(String, u32)> = counts
            .into_iter()
            .filter(|(dir, n)| {
                *n >= LARGE_FOLDER_FILES && base.iter().any(|u| dir.starts_with(&format!("{u}/")))
            })
            .collect();
        large.sort_unstable();
        let count_under = |dir: &str| {
            let prefix = format!("{dir}/");
            self.files
                .iter()
                .filter(|f| f.parsed && f.path.starts_with(&prefix))
                .count()
        };
        for (dir, n) in large {
            // A folder holding everything its enclosing unit holds (`src/`) is the unit, not a sub-folder.
            let enclosing = dirs
                .iter()
                .filter(|u| dir.starts_with(&format!("{u}/")))
                .max_by_key(|u| u.len())
                .cloned();
            if enclosing.is_some_and(|u| count_under(&u) == n as usize) {
                continue;
            }
            dirs.insert(dir);
        }
        let all: Vec<String> = dirs.into_iter().collect();
        let mut units: Vec<Unit> = all
            .iter()
            .map(|path| {
                let prefix = format!("{path}/");
                let mut files = 0;
                let mut lines = 0;
                let mut tests = 0;
                let mut rank = 0.0;
                let mut recent = false;
                for f in self.files.iter().filter(|f| f.path.starts_with(&prefix)) {
                    rank += f.rank;
                    if f.parsed {
                        files += 1;
                        lines += f.lines;
                        if f.test {
                            tests += 1;
                        }
                    }
                    recent |= is_recent(f.mtime_ns);
                }
                let parent = all
                    .iter()
                    .filter(|u| path.starts_with(&format!("{u}/")))
                    .max_by_key(|u| u.len())
                    .cloned();
                Unit {
                    node: format!("unit:{path}"),
                    name: path.rsplit('/').next().unwrap_or(path).to_owned(),
                    path: path.clone(),
                    files,
                    lines,
                    tests,
                    share: self.share(rank),
                    entry: self.entry_points.iter().any(|e| e.starts_with(&prefix)),
                    recent,
                    parent,
                }
            })
            .filter(|u| u.files > 0)
            .collect();
        units.sort_by(|a, b| {
            b.share
                .total_cmp(&a.share)
                .then_with(|| a.path.cmp(&b.path))
        });
        units
    }

    fn share(&self, rank: f64) -> f32 {
        if self.total_rank > 0.0 {
            (rank / self.total_rank) as f32
        } else {
            0.0
        }
    }

    /// Units with no enclosing unit: the top-level partition's input.
    pub fn top_units(&self) -> Vec<&Unit> {
        self.units.iter().filter(|u| u.parent.is_none()).collect()
    }

    /// The unit for a path (by node id).
    pub fn unit(&self, node: &str) -> Option<&Unit> {
        self.units.iter().find(|u| u.node == node)
    }

    /// Units directly inside `path`.
    pub fn nested_units(&self, path: &str) -> Vec<&Unit> {
        self.units
            .iter()
            .filter(|u| u.parent.as_deref() == Some(path))
            .collect()
    }

    /// `(path, mtime_ns, size)` of every file under `paths` (a file path matches itself).
    pub fn files_under(&self, paths: &[String]) -> Vec<(String, i64, i64)> {
        self.files
            .iter()
            .filter(|f| {
                paths
                    .iter()
                    .any(|p| f.path == *p || f.path.starts_with(&format!("{p}/")))
            })
            .map(|f| (f.path.clone(), f.mtime_ns, f.size))
            .collect()
    }

    /// §5.4 over an element's anchors and its diagram's edges.
    pub fn facts_hash(&self, anchors: &[String], edges: &[UnitEdge]) -> String {
        let tuples: Vec<(String, String, String, u32)> = edges
            .iter()
            .map(|e| (e.from.clone(), e.to.clone(), e.kind().to_owned(), e.weight))
            .collect();
        skeleton::facts_hash(&self.files_under(anchors), &tuples)
    }

    /// Edges among `nodes` (node id → covering paths), one per ordered pair
    /// with evidence, weight-sorted. Files map to the deepest covering path.
    pub fn edges_among(&self, nodes: &[(String, Vec<String>)]) -> Vec<UnitEdge> {
        let owner = |path: &str| -> Option<usize> {
            let mut best: Option<(usize, usize)> = None;
            for (i, (_, paths)) in nodes.iter().enumerate() {
                for p in paths {
                    if (path == p || path.starts_with(&format!("{p}/")))
                        && best.is_none_or(|(_, len)| p.len() > len)
                    {
                        best = Some((i, p.len()));
                    }
                }
            }
            best.map(|(i, _)| i)
        };
        let owners: Vec<Option<usize>> = self.files.iter().map(|f| owner(&f.path)).collect();
        let mut agg: HashMap<(usize, usize), PairAgg> = HashMap::new();
        for (&(a, b), e) in &self.file_edges {
            let (Some(ua), Some(ub)) = (owners[a], owners[b]) else {
                continue;
            };
            if ua == ub {
                continue;
            }
            let slot = agg.entry((ua, ub)).or_default();
            slot.0.imports += e.imports;
            slot.0.refs += e.refs;
            slot.0.confidence += e.confidence;
            if e.imports > 0 && slot.1.is_none() {
                slot.1 = Some((e.import_line, a));
            }
            if e.refs > 0 && slot.2.is_none() {
                slot.2 = Some((e.ref_line, a));
            }
        }
        let mut out: Vec<UnitEdge> = agg
            .into_iter()
            .map(|((ua, ub), (e, imp, rf))| {
                let mut evidence = Vec::new();
                for (from, to) in &self.manifest_deps {
                    let covers = |paths: &[String], d: &str| {
                        paths
                            .iter()
                            .any(|p| p == d || d.starts_with(&format!("{p}/")))
                    };
                    if covers(&nodes[ua].1, from) && covers(&nodes[ub].1, to) {
                        evidence.push(MapEvidence::Manifest {
                            from: from.clone(),
                            to: to.clone(),
                            kind: "cargo-path".to_owned(),
                        });
                    }
                }
                let manifests = u32::try_from(evidence.len()).unwrap_or(u32::MAX);
                if let Some((line, a)) = imp {
                    evidence.push(MapEvidence::Imports {
                        count: e.imports,
                        sample: format!("{}#L{line}", self.files[a].path),
                    });
                }
                if let Some((line, a)) = rf {
                    evidence.push(MapEvidence::Refs {
                        count: e.refs,
                        sample: format!("{}#L{line}", self.files[a].path),
                        confidence: e.confidence / f64::from(e.refs.max(1)),
                    });
                }
                UnitEdge {
                    from: nodes[ua].0.clone(),
                    to: nodes[ub].0.clone(),
                    weight: e.imports + e.refs + manifests,
                    evidence,
                }
            })
            .collect();
        // Manifest-only pairs (a path dep with no resolved import yet).
        for (from, to) in &self.manifest_deps {
            let (Some(ua), Some(ub)) = (owner(from), owner(to)) else {
                continue;
            };
            if ua == ub
                || out
                    .iter()
                    .any(|e| e.from == nodes[ua].0 && e.to == nodes[ub].0)
            {
                continue;
            }
            out.push(UnitEdge {
                from: nodes[ua].0.clone(),
                to: nodes[ub].0.clone(),
                weight: 1,
                evidence: vec![MapEvidence::Manifest {
                    from: from.clone(),
                    to: to.clone(),
                    kind: "cargo-path".to_owned(),
                }],
            });
        }
        out.sort_by(|a, b| {
            b.weight
                .cmp(&a.weight)
                .then_with(|| (&a.from, &a.to).cmp(&(&b.from, &b.to)))
        });
        out
    }

    /// §5.2 thresholding over the top-level units: per source, edges under
    /// [`EDGE_MIN_SHARE`] of its out-weight fold into the count unless a
    /// manifest backs them. Returns `(kept, folded)`.
    fn thresholded_edges(&self) -> (Vec<UnitEdge>, u32) {
        let nodes: Vec<(String, Vec<String>)> = self
            .top_units()
            .iter()
            .map(|u| (u.node.clone(), vec![u.path.clone()]))
            .collect();
        let all = self.edges_among(&nodes);
        let mut out_weight: HashMap<String, f64> = HashMap::new();
        for e in &all {
            *out_weight.entry(e.from.clone()).or_default() += f64::from(e.weight);
        }
        let mut kept = Vec::with_capacity(all.len());
        let mut folded = 0;
        for e in all {
            let total = out_weight.get(e.from.as_str()).copied().unwrap_or(0.0);
            let manifest = e
                .evidence
                .iter()
                .any(|x| matches!(x, MapEvidence::Manifest { .. }));
            if manifest || total <= 0.0 || f64::from(e.weight) / total >= EDGE_MIN_SHARE {
                kept.push(e);
            } else {
                folded += 1;
            }
        }
        (kept, folded)
    }

    /// Evidence joining two member sets (a group edge's `backed`).
    pub fn evidence_between(&self, from: &[String], to: &[String]) -> Vec<MapEvidence> {
        let nodes = vec![
            ("a".to_owned(), from.to_vec()),
            ("b".to_owned(), to.to_vec()),
        ];
        self.edges_among(&nodes)
            .into_iter()
            .find(|e| e.from == "a" && e.to == "b")
            .map(|e| e.evidence)
            .unwrap_or_default()
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

    /// §5.5: the structure below a set of anchors. For several anchors (a
    /// cluster) the children are the units themselves; for one unit the
    /// children are its nested units and its modules — sub-directories with
    /// more than [`LEAF_FILES`] parsed files, else their files one by one.
    pub fn sub_skeleton(&self, anchors: &[String]) -> SubSkeleton {
        let mut children: Vec<Child> = if anchors.len() != 1 {
            anchors
                .iter()
                .filter_map(|p| self.unit(&format!("unit:{p}")))
                .map(|u| self.unit_child(u))
                .collect()
        } else {
            let path = &anchors[0];
            let nested = self.nested_units(path);
            let covered = |p: &str| {
                nested
                    .iter()
                    .any(|u| p.starts_with(&format!("{}/", u.path)))
            };
            // Pass-through directories (`src/`) are flattened: descend while the
            // unit holds exactly one sub-directory and no loose files.
            let mut prefix = format!("{path}/");
            let (mut dirs, mut loose): (BTreeMap<String, Vec<usize>>, Vec<usize>) =
                Default::default();
            loop {
                dirs.clear();
                loose.clear();
                for (i, f) in self.files.iter().enumerate() {
                    if !f.parsed || !f.path.starts_with(&prefix) || covered(&f.path) {
                        continue;
                    }
                    let rest = &f.path[prefix.len()..];
                    match rest.split_once('/') {
                        Some((dir, _)) => dirs.entry(format!("{prefix}{dir}")).or_default().push(i),
                        None => loose.push(i),
                    }
                }
                if dirs.len() == 1
                    && loose.is_empty()
                    && let Some(only) = dirs.keys().next()
                    && nested
                        .iter()
                        .all(|u| u.path.starts_with(&format!("{only}/")))
                {
                    prefix = format!("{only}/");
                    continue;
                }
                break;
            }
            let mut out: Vec<Child> = nested.iter().map(|u| self.unit_child(u)).collect();
            for (dir, idx) in dirs {
                if idx.len() > LEAF_FILES as usize {
                    out.push(self.dir_child(&dir, &idx));
                } else {
                    loose.extend(idx);
                }
            }
            out.extend(loose.into_iter().map(|i| self.file_child(i)));
            out
        };
        children.sort_by(|a, b| {
            b.share
                .total_cmp(&a.share)
                .then_with(|| b.lines.cmp(&a.lines))
                .then_with(|| a.path.cmp(&b.path))
        });
        let bundled = if children.len() > DIAGRAM_NODES {
            children.split_off(DIAGRAM_NODES)
        } else {
            Vec::new()
        };
        let nodes: Vec<(String, Vec<String>)> = children
            .iter()
            .map(|c| (c.node.clone(), vec![c.path.clone()]))
            .collect();
        let edges = self.edges_among(&nodes);
        SubSkeleton {
            children,
            bundled,
            edges,
        }
    }

    fn unit_child(&self, u: &Unit) -> Child {
        Child {
            node: u.node.clone(),
            name: u.name.clone(),
            path: u.path.clone(),
            kind: "unit",
            files: u.files,
            lines: u.lines,
            share: u.share,
            leaf: u.leaf(),
            range: None,
        }
    }

    fn dir_child(&self, dir: &str, idx: &[usize]) -> Child {
        let lines = idx.iter().map(|&i| self.files[i].lines).sum();
        let rank = idx.iter().map(|&i| self.files[i].rank).sum();
        Child {
            node: format!("unit:{dir}"),
            name: dir.rsplit('/').next().unwrap_or(dir).to_owned(),
            path: dir.to_owned(),
            kind: "unit",
            files: u32::try_from(idx.len()).unwrap_or(u32::MAX),
            lines,
            share: self.share(rank),
            leaf: idx.len() <= LEAF_FILES as usize,
            range: None,
        }
    }

    fn file_child(&self, i: usize) -> Child {
        let f = &self.files[i];
        Child {
            node: format!("unit:{}", f.path),
            name: f.path.rsplit('/').next().unwrap_or(&f.path).to_owned(),
            path: f.path.clone(),
            kind: "file",
            files: 1,
            lines: f.lines,
            share: self.share(f.rank),
            leaf: true,
            range: None,
        }
    }

    /// The `concern` triggers of §4.3 for an element over `anchors`: files
    /// over [`CONCERN_FILE_LINES`] lines and, for a top-level unit, an
    /// in-degree over [`CONCERN_IN_DEGREE`].
    pub fn concern_triggers(&self, anchors: &[String]) -> Vec<String> {
        let mut out: Vec<String> = self
            .files
            .iter()
            .filter(|f| f.lines > CONCERN_FILE_LINES)
            .filter(|f| {
                anchors
                    .iter()
                    .any(|p| f.path == *p || f.path.starts_with(&format!("{p}/")))
            })
            .map(|f| format!("{} has {} lines", f.path, f.lines))
            .collect();
        for p in anchors {
            let node = format!("unit:{p}");
            let indeg = self.edges.iter().filter(|e| e.to == node).count();
            if indeg > CONCERN_IN_DEGREE {
                out.push(format!("{p} has in-degree {indeg}"));
            }
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
        seeds.extend(self.entry_points.iter().map(|p| (p.clone(), None)));
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
/// missing detector row (W1's detectors not run) counts as ineligible.
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

fn is_recent(mtime_ns: i64) -> bool {
    use std::time::{SystemTime, UNIX_EPOCH};
    let now = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| i64::try_from(d.as_nanos()).unwrap_or(i64::MAX))
        .unwrap_or(0);
    now - mtime_ns < crate::rank::RECENT_NS
}

/// `a/b/c.rs` → `a/b`; a root file → `""`.
fn parent_dir(path: &str) -> &str {
    path.rsplit_once('/').map_or("", |(d, _)| d)
}

/// The longest common directory prefix of two directories, by whole segments.
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

/// `files` row: `(id, path, mtime_ns, size, language, lines, is_test, rank)`.
type FileSql = (i64, String, i64, i64, String, i64, i64, f64);
/// Per node pair while aggregating: the sums plus one import and one ref sample `(line, file index)`.
type PairAgg = (FileEdge, Option<(i64, usize)>, Option<(i64, usize)>);
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

#[cfg(test)]
pub(crate) mod tests {
    use super::*;

    /// A small synthetic repo: three crates, one top directory, a large folder.
    pub(crate) fn file(path: &str, lines: u32, rank: f64) -> FileRow {
        FileRow {
            path: path.to_owned(),
            mtime_ns: 1,
            size: i64::from(lines) * 30,
            lines,
            parsed: !path.ends_with(".md"),
            test: path.contains("/tests/"),
            rank,
        }
    }

    pub(crate) fn synthetic() -> Facts {
        let mut files = vec![
            file("crates/app/src/main.rs", 300, 0.3),
            file("crates/app/src/cli.rs", 200, 0.1),
            file("crates/app/src/config.rs", 50, 0.05),
            file("crates/core/src/lib.rs", 900, 0.2),
            file("crates/core/src/store.rs", 400, 0.1),
            file("crates/core/src/model.rs", 6000, 0.05),
            file("crates/util/src/lib.rs", 100, 0.1),
            file("docs/guide.md", 40, 0.0),
            file("docs/build.py", 20, 0.01),
        ];
        for i in 0..LARGE_FOLDER_FILES {
            files.push(file(&format!("crates/core/src/gen/g{i}.rs"), 10, 0.001));
        }
        let idx = |p: &str| files.iter().position(|f| f.path == p).unwrap();
        let edge = |imports: u32, refs: u32| FileEdge {
            imports,
            import_line: 3,
            refs,
            ref_line: 20,
            confidence: f64::from(refs) * 0.9,
        };
        let file_edges = vec![
            (
                idx("crates/app/src/main.rs"),
                idx("crates/core/src/lib.rs"),
                edge(2, 10),
            ),
            (
                idx("crates/app/src/cli.rs"),
                idx("crates/core/src/store.rs"),
                edge(1, 4),
            ),
            (
                idx("crates/app/src/main.rs"),
                idx("crates/util/src/lib.rs"),
                edge(1, 1),
            ),
            (
                idx("crates/core/src/lib.rs"),
                idx("crates/util/src/lib.rs"),
                edge(3, 30),
            ),
            (
                idx("crates/core/src/lib.rs"),
                idx("crates/core/src/store.rs"),
                edge(1, 12),
            ),
            (
                idx("crates/core/src/store.rs"),
                idx("crates/core/src/model.rs"),
                edge(1, 8),
            ),
        ];
        let packages: Vec<(String, String)> = files
            .iter()
            .filter(|f| f.path.starts_with("crates/"))
            .map(|f| (f.path.split('/').nth(1).unwrap().to_owned(), f.path.clone()))
            .collect();
        let cargo_dirs: HashMap<String, String> = ["app", "core", "util"]
            .into_iter()
            .map(|n| (n.to_owned(), format!("crates/{n}")))
            .collect();
        Facts::synthetic(
            Path::new("/synthetic"),
            files,
            file_edges,
            vec![
                ("crates/app".to_owned(), "crates/core".to_owned()),
                ("crates/app".to_owned(), "crates/util".to_owned()),
                ("crates/core".to_owned(), "crates/util".to_owned()),
            ],
            packages,
            cargo_dirs,
            vec![Detection {
                kind: "dependency-map".to_owned(),
                eligible: true,
                hits: 3,
                samples: vec!["Cargo.toml#L1".to_owned()],
                confidence: 0.9,
            }],
            vec!["crates/app/src/main.rs".to_owned()],
        )
    }

    #[test]
    fn units_edges_and_sub_skeletons_follow_section_5() {
        let f = synthetic();
        let top: Vec<&str> = f.top_units().iter().map(|u| u.path.as_str()).collect();
        assert_eq!(top, ["crates/app", "crates/core", "crates/util", "docs"]);
        let nested: Vec<&str> = f
            .nested_units("crates/core")
            .iter()
            .map(|u| u.path.as_str())
            .collect();
        assert_eq!(nested, ["crates/core/src/gen"]);
        assert!(f.unit("unit:docs").unwrap().leaf());
        assert!(f.unit("unit:crates/app").unwrap().entry);

        let e = f
            .edges
            .iter()
            .find(|e| e.from == "unit:crates/app" && e.to == "unit:crates/core")
            .unwrap();
        assert_eq!(e.kind(), "manifest");
        assert_eq!(
            evidence_summary(&e.evidence),
            "1 manifest dep · 3 imports · 14 refs"
        );
        assert!(f.edges.iter().all(|e| e.from != e.to));
        assert!(
            f.evidence_between(&["crates/app".into()], &["crates/util".into()])
                .len()
                >= 2
        );
        assert!(
            f.evidence_between(&["crates/util".into()], &["crates/app".into()])
                .is_empty()
        );

        let sub = f.sub_skeleton(&["crates/core".to_owned()]);
        let kids: Vec<&str> = sub.children.iter().map(|c| c.path.as_str()).collect();
        assert_eq!(
            kids,
            [
                "crates/core/src/lib.rs",
                "crates/core/src/store.rs",
                "crates/core/src/model.rs",
                "crates/core/src/gen"
            ]
        );
        assert!(
            sub.edges
                .iter()
                .any(|e| e.from == "unit:crates/core/src/lib.rs"
                    && e.to == "unit:crates/core/src/store.rs")
        );
        let cluster = f.sub_skeleton(&["crates/app".to_owned(), "crates/core".to_owned()]);
        assert_eq!(cluster.children.len(), 2);
        assert_eq!(cluster.edges.len(), 1);
        assert_eq!(
            f.concern_triggers(&["crates/core".to_owned()]),
            ["crates/core/src/model.rs has 6000 lines"]
        );
        let h1 = f.facts_hash(&["crates/core".to_owned()], &sub.edges);
        assert_ne!(h1, f.facts_hash(&["crates/app".to_owned()], &sub.edges));
        assert_eq!(f.detections[0].kind, "overall-architecture");
        assert!(f.detections[0].eligible);
        assert_eq!(f.detections.len(), 12);
    }
}
