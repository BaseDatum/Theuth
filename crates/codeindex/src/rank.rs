//! Ranking and the map (60 §7): aider's algorithm over the file graph —
//! referencer → definer edges from resolved `refs` (confidence ≥ 0.5) and
//! `imports`, weighted by identifier rarity, personalised PageRank (own
//! sparse power iteration), rank spread per `(file, identifier)`, rendered
//! top-down to a token budget. Re-exports route through to their definer,
//! barrels and type-only files take damped in-edges (60 §17), and the
//! personalisation vector is seeded from focus, edited files, their
//! co-change partners, or — without any signal — manifest entry points.

use std::collections::{HashMap, HashSet};
use std::path::Path;

use sqlx::{FromRow, SqlitePool};

use crate::tools::estimate_tokens;
use crate::{store, walk};

/// Damping factor.
const DAMPING: f64 = 0.85;
/// Power-iteration cap.
const MAX_ITER: usize = 50;
/// Stop when the L1 change of one iteration is below this.
const TOLERANCE: f64 = 1e-6;
/// Self-edge weight for files that define something nobody references.
const SELF_EDGE: f64 = 0.1;
/// Weight of an `imports` edge before `mul` (60 §22): a use of a module is
/// weaker evidence than a reference to one of its names.
const IMPORT_EDGE: f64 = 0.1;
/// Edges into a barrel keep this share: the mass the passthrough could not
/// attribute (glob imports, names the barrel defines itself).
const BARREL_IN: f64 = 0.3;
/// Edges into a type-heavy file keep this share. Empryo uses 0.3; a Rust
/// type module is what a brief most often needs to show, so halve, don't bury.
const TYPE_IN: f64 = 0.5;
/// Barrel: re-exported names above this share of tags (definitions + re-exports).
const BARREL_SHARE: f64 = 0.6;
/// Type-heavy: types/traits/interfaces/consts at or above this share, no functions.
const TYPE_SHARE: f64 = 0.8;
/// Teleport weight of a `focus` path or an identifier's definer.
const SEED_MENTIONED: f64 = 1.0;
/// Teleport weight of an edited file: twice a mention, one decade below the
/// ×50/×10 edge multipliers so the seed steers, the edges rank.
const SEED_EDITED: f64 = 2.0;
/// Teleport weight of a co-change partner at 3+ shared commits (linear below).
const SEED_COCHANGE: f64 = 0.5;
/// Extra teleport weight of an entry point over the uniform 1.0 when nothing
/// else is given: an entry counts double. Empryo's +4·base put leaf crate
/// roots nobody references at #5 on this repository (§ as-built numbers).
const SEED_ENTRY: f64 = 1.0;
/// Shared commits at which a co-change partner reaches [`SEED_COCHANGE`].
const COCHANGE_CAP: i64 = 3;
/// Pseudo-identifier of an `imports` edge.
const IMPORT: &str = "<import>";
/// Pseudo-identifier of a self-edge.
const SELF: &str = "<self>";
/// Budget tolerance of the map fit (aider: 15 %).
const FIT_SLACK: f64 = 0.15;

/// One graph node: a parsed file.
#[derive(Clone, Debug, FromRow)]
pub struct FileNode {
    /// `files.id`.
    pub id: i64,
    /// Root-relative.
    pub path: String,
    /// `files.language`.
    pub language: String,
    /// Line count.
    pub lines: i64,
    /// Stored unpersonalised rank (warm start).
    pub rank: f64,
}

/// One definition the map may render.
#[derive(Clone, Debug, FromRow)]
pub struct SymNode {
    /// `symbols.id`.
    pub id: i64,
    /// `files.id`.
    pub file_id: i64,
    /// Name.
    pub name: String,
    /// Kind.
    pub kind: String,
    /// 1-based.
    pub line_start: i64,
    /// Definition line.
    pub signature: String,
}

/// `(referencer file, definer file, identifier, count)` before weighting.
#[derive(Clone, Debug, FromRow)]
pub struct EdgeRow {
    /// `files.id` of the referencing file.
    pub src: i64,
    /// `files.id` of the defining file.
    pub dst: i64,
    /// Referenced identifier.
    pub name: String,
    /// References grouped under this edge.
    pub n: i64,
}

/// One resolved, non-`mod:` `imports` row.
#[derive(Clone, Debug)]
pub struct ImportRow {
    /// `files.id` of the importing file.
    pub src: i64,
    /// `files.id` the row resolved to.
    pub dst: i64,
    /// Imported names; `None` = glob.
    pub names: Option<Vec<String>>,
    /// `src` resolved a reference to one of the names, or to anything `dst`
    /// defines. Otherwise the row is a re-export: `pub use`, `export …
    /// from`, `__init__.py` passthrough — `src` republishes, it does not use.
    pub used: bool,
}

/// The file graph as the store gives it.
pub struct Graph {
    /// Parsed files.
    pub files: Vec<FileNode>,
    /// `files.id` → index into `files`.
    idx: HashMap<i64, usize>,
    /// Every symbol of every parsed file.
    pub symbols: Vec<SymNode>,
    /// Raw `refs` edges.
    edges: Vec<EdgeRow>,
    /// Resolved `imports` rows.
    imports: Vec<ImportRow>,
    /// `cochanges` rows `(file_a, file_b, count)`.
    cochanges: Vec<(i64, i64, i64)>,
    /// Indexes of manifest entry points (60 §17 item 9).
    pub entries: HashSet<usize>,
}

/// What a `code_map` call personalises toward.
#[derive(Default)]
pub struct Focus {
    /// Identifiers: ×10 on their edges.
    pub idents: HashSet<String>,
    /// Indexes into [`Graph::files`] (mentioned paths, identifier definers,
    /// edited files): ×50 on their out-edges, one self-edge per definition.
    pub files: HashSet<usize>,
    /// Teleport weights by file index; empty = uniform plus entry seeds.
    pub seeds: HashMap<usize, f64>,
}

/// One ranked definition (60 §8 `code_map.rows`).
#[derive(Clone, Debug, serde::Serialize)]
pub struct Tag {
    /// Root-relative path.
    pub path: String,
    /// 1-based line of the definition.
    pub line: i64,
    /// Kind.
    pub kind: String,
    /// Name.
    pub name: String,
    /// Definition line.
    pub signature: String,
    /// Spread rank.
    pub rank: f64,
}

/// Result of one ranking.
pub struct Ranked {
    /// File rank by index into [`Graph::files`].
    pub files: Vec<f64>,
    /// Definitions by rank, descending; `symbol_id` beside each for persistence.
    pub tags: Vec<(i64, Tag)>,
    /// Power iterations run.
    pub iterations: usize,
}

impl Graph {
    /// Read the graph: parsed files, their symbols, `refs` edges with
    /// `confidence ≥ 0.5` across files grouped per (R, D, name), resolved
    /// `imports` rows with their used/re-export flag, co-change pairs and
    /// the manifest entry points.
    pub async fn load(pool: &SqlitePool) -> anyhow::Result<Self> {
        let files: Vec<FileNode> =
            sqlx::query_as("SELECT id, path, language, lines, rank FROM files WHERE language <> 'other' ORDER BY id")
                .fetch_all(pool)
                .await?;
        let symbols: Vec<SymNode> =
            sqlx::query_as("SELECT id, file_id, name, kind, line_start, signature FROM symbols")
                .fetch_all(pool)
                .await?;
        let edges: Vec<EdgeRow> = sqlx::query_as(
            "SELECT r.file_id AS src, s.file_id AS dst, r.name, count(*) AS n FROM refs r JOIN symbols s ON s.id = r.to_symbol \
             WHERE r.confidence >= 0.5 AND r.file_id <> s.file_id GROUP BY r.file_id, s.file_id, r.name",
        )
        .fetch_all(pool)
        .await?;
        // `used`: the file resolved a reference to the imported name, or to
        // anything the target defines (`Bar::new()` resolves to `Bar`'s file
        // under the name `new`). Both come from `edges` — no `refs` scan
        // (a DISTINCT over 149k refs cost 0.3 s, correlated EXISTS 4–9 s).
        // An imported name never referenced was imported to be re-exported.
        let resolved: HashSet<(i64, i64)> = edges.iter().map(|e| (e.src, e.dst)).collect();
        let referenced: HashSet<(i64, &str)> =
            edges.iter().map(|e| (e.src, e.name.as_str())).collect();
        // `mod x;` rows are module-tree edges, not uses (60 §21): a crate
        // root with few `use`s would otherwise pour its rank into them.
        let rows: Vec<(i64, i64, Option<String>)> = sqlx::query_as(
            "SELECT file_id, target_file, names FROM imports WHERE target_file IS NOT NULL AND target_file <> file_id \
             AND specifier NOT LIKE 'mod:%'",
        )
        .fetch_all(pool)
        .await?;
        let imports = rows
            .into_iter()
            .map(|(src, dst, names)| {
                let names: Option<Vec<String>> = names.and_then(|s| serde_json::from_str(&s).ok());
                let used = resolved.contains(&(src, dst))
                    || names
                        .iter()
                        .flatten()
                        .any(|n| referenced.contains(&(src, n.as_str())));
                ImportRow {
                    src,
                    dst,
                    names,
                    used,
                }
            })
            .collect();
        let cochanges: Vec<(i64, i64, i64)> =
            sqlx::query_as("SELECT file_a, file_b, count FROM cochanges")
                .fetch_all(pool)
                .await?;
        let idx: HashMap<i64, usize> = files.iter().enumerate().map(|(i, f)| (f.id, i)).collect();
        // Entry points: manifests on disk (60 §17 item 9) plus Go `package main`.
        let manifests: Vec<String> = sqlx::query_scalar(
            "SELECT path FROM files WHERE path = 'Cargo.toml' OR path LIKE '%/Cargo.toml' \
             OR path = 'package.json' OR path LIKE '%/package.json' \
             OR path = 'pyproject.toml' OR path LIKE '%/pyproject.toml' ORDER BY path",
        )
        .fetch_all(pool)
        .await?;
        let mut entry_paths: Vec<String> = match store::meta(pool, "root").await? {
            Some(root) => walk::entry_points(Path::new(&root), &manifests),
            None => Vec::new(),
        };
        entry_paths.extend(
            sqlx::query_scalar::<_, String>(
                "SELECT path FROM files WHERE language = 'go' AND package = 'main'",
            )
            .fetch_all(pool)
            .await?,
        );
        let by_path: HashMap<&str, usize> = files
            .iter()
            .enumerate()
            .map(|(i, f)| (f.path.as_str(), i))
            .collect();
        // Fixture manifests under `tests/` name entry points of a fixture, not of the repo.
        let entries = entry_paths
            .iter()
            .filter(|p| !walk::is_test_path(p))
            .filter_map(|p| by_path.get(p.as_str()).copied())
            .collect();
        Ok(Self {
            files,
            idx,
            symbols,
            edges,
            imports,
            cochanges,
            entries,
        })
    }

    /// Root-relative entry points, sorted.
    pub fn entry_paths(&self) -> Vec<&str> {
        let mut v: Vec<&str> = self
            .entries
            .iter()
            .map(|&i| self.files[i].path.as_str())
            .collect();
        v.sort_unstable();
        v
    }

    /// Indexes of the files `e` names: an indexed path, or every file under
    /// an indexed directory (root-relative).
    fn paths(&self, e: &str) -> Vec<usize> {
        self.files
            .iter()
            .enumerate()
            .filter(|(_, n)| {
                n.path == e
                    || n.path
                        .strip_prefix(e)
                        .is_some_and(|rest| rest.starts_with('/'))
            })
            .map(|(i, _)| i)
            .collect()
    }

    /// Split `focus` entries into files (an entry that names an indexed
    /// file or directory, root-relative) and identifiers (the rest); add
    /// `edited` files at [`SEED_EDITED`] and their co-change partners at up
    /// to [`SEED_COCHANGE`]. No entries at all → default seeds (entry points).
    pub fn focus(&self, entries: &[String], edited: &[String]) -> Focus {
        let mut f = Focus::default();
        let clean = |e: &str| {
            e.trim()
                .trim_start_matches("./")
                .trim_end_matches('/')
                .to_owned()
        };
        for e in entries.iter().map(|e| clean(e)).filter(|e| !e.is_empty()) {
            let hits = self.paths(&e);
            let hits = if hits.is_empty() {
                // An identifier names its definers too (60 §21): a symbol
                // referenced only inside its own file has no edge to boost.
                f.idents.insert(e.clone());
                self.symbols
                    .iter()
                    .filter(|s| s.name == e)
                    .filter_map(|s| self.idx.get(&s.file_id).copied())
                    .collect()
            } else {
                hits
            };
            for i in hits {
                f.files.insert(i);
                let w = f.seeds.entry(i).or_default();
                *w = w.max(SEED_MENTIONED);
            }
        }
        let mut edited_ids: HashSet<i64> = HashSet::new();
        for e in edited.iter().map(|e| clean(e)).filter(|e| !e.is_empty()) {
            for i in self.paths(&e) {
                f.files.insert(i);
                f.seeds.insert(i, SEED_EDITED);
                edited_ids.insert(self.files[i].id);
            }
        }
        if !edited_ids.is_empty() {
            for &(a, b, count) in &self.cochanges {
                for (me, other) in [(a, b), (b, a)] {
                    if !edited_ids.contains(&me) {
                        continue;
                    }
                    let Some(&j) = self.idx.get(&other) else {
                        continue;
                    };
                    if f.files.contains(&j) {
                        continue;
                    }
                    #[allow(clippy::cast_precision_loss)]
                    let w = SEED_COCHANGE * count.min(COCHANGE_CAP) as f64 / COCHANGE_CAP as f64;
                    let s = f.seeds.entry(j).or_default();
                    *s = s.max(w);
                }
            }
        }
        f
    }

    /// Weighted personalised PageRank, then rank spread onto `(file, name)`
    /// and the symbols so named (60 §7).
    pub fn rank(&self, focus: &Focus) -> Ranked {
        let n = self.files.len();
        if n == 0 {
            return Ranked {
                files: Vec::new(),
                tags: Vec::new(),
                iterations: 0,
            };
        }
        // Identifier rarity: files defining each name.
        let mut definers: HashMap<&str, HashSet<i64>> = HashMap::new();
        for s in &self.symbols {
            definers
                .entry(s.name.as_str())
                .or_default()
                .insert(s.file_id);
        }
        let mul = |name: &str, src: usize| -> f64 {
            let mut mul = 1.0;
            if focus.idents.contains(name) {
                mul *= 10.0;
            }
            if name.len() >= 8 && is_snake_or_camel(name) {
                mul *= 10.0;
            }
            if name.starts_with('_') {
                mul *= 0.1;
            }
            if definers.get(name).is_some_and(|d| d.len() > 5) {
                mul *= 0.1;
            }
            if focus.files.contains(&src) {
                mul *= 50.0;
            }
            mul
        };
        // Re-exports (60 §17 bullet 1): `(file, name) → definer` from rows the
        // file imports but never references, plus glob re-exports by file.
        let mut reexport: HashMap<(usize, &str), usize> = HashMap::new();
        let mut glob_reexport: HashMap<usize, Vec<usize>> = HashMap::new();
        let mut reexported = vec![0usize; n];
        for r in self.imports.iter().filter(|r| !r.used) {
            let (Some(&s), Some(&d)) = (self.idx.get(&r.src), self.idx.get(&r.dst)) else {
                continue;
            };
            match &r.names {
                Some(names) => {
                    for name in names {
                        reexport.insert((s, name.as_str()), d);
                    }
                    reexported[s] += names.len();
                }
                None => {
                    glob_reexport.entry(s).or_default().push(d);
                    reexported[s] += 1;
                }
            }
        }
        let shape = self.shapes(&reexported);
        // A `refs` edge names a definition the target owns: full weight
        // unless the target is type-only. An `<import>` edge into a barrel
        // is what the passthrough could not attribute: residual weight.
        let damp_ref = |d: usize| -> f64 {
            if shape[d] == Shape::TypeHeavy {
                TYPE_IN
            } else {
                1.0
            }
        };
        let damp_import = |d: usize| -> f64 {
            match shape[d] {
                Shape::Barrel => BARREL_IN,
                Shape::TypeHeavy => TYPE_IN,
                Shape::Plain => 1.0,
            }
        };
        // Weighted edges (src, dst, name, weight), then self-edges.
        let mut edges: Vec<(usize, usize, &str, f64)> = Vec::with_capacity(self.edges.len() + n);
        let mut incoming = vec![false; n];
        for e in &self.edges {
            let (Some(&s), Some(&d)) = (self.idx.get(&e.src), self.idx.get(&e.dst)) else {
                continue;
            };
            #[allow(clippy::cast_precision_loss)]
            let base = (e.n.max(1) as f64).sqrt();
            edges.push((s, d, e.name.as_str(), mul(&e.name, s) * base * damp_ref(d)));
            incoming[d] = true;
        }
        // Import edges: a row the file uses lands on the definer of each
        // name it imports when the target re-exports it (one level), else on
        // the target; a row the file re-exports is not an edge at all — the
        // file does not use what it republishes.
        let mut imported: HashSet<(usize, usize)> = HashSet::new();
        for r in self.imports.iter().filter(|r| r.used) {
            let (Some(&s), Some(&t)) = (self.idx.get(&r.src), self.idx.get(&r.dst)) else {
                continue;
            };
            let route = |name: &str| -> usize {
                reexport.get(&(t, name)).copied().unwrap_or_else(|| {
                    glob_reexport
                        .get(&t)
                        .and_then(|gs| {
                            gs.iter().find(|&&g| {
                                definers
                                    .get(name)
                                    .is_some_and(|f| f.contains(&self.files[g].id))
                            })
                        })
                        .copied()
                        .unwrap_or(t)
                })
            };
            let targets: Vec<usize> = match &r.names {
                Some(names) => names.iter().map(|nm| route(nm)).collect(),
                None => vec![t],
            };
            for d in targets {
                if d != s && imported.insert((s, d)) {
                    edges.push((s, d, IMPORT, mul(IMPORT, s) * IMPORT_EDGE * damp_import(d)));
                    incoming[d] = true;
                }
            }
        }
        // A focus file's own definitions are what the caller wants to see:
        // one self-edge per definition, weighted like a single reference,
        // so they take a share of the file's (personalised) rank.
        for s in &self.symbols {
            if let Some(&i) = self.idx.get(&s.file_id)
                && focus.files.contains(&i)
            {
                edges.push((i, i, s.name.as_str(), mul(&s.name, i)));
                incoming[i] = true;
            }
        }
        let has_defs: HashSet<usize> = self
            .symbols
            .iter()
            .filter_map(|s| self.idx.get(&s.file_id).copied())
            .collect();
        for (i, _) in incoming
            .iter()
            .enumerate()
            .filter(|(i, inc)| !**inc && has_defs.contains(i))
        {
            edges.push((i, i, SELF, SELF_EDGE));
        }
        // CSR by source, values = weight / out-weight.
        edges.sort_by_key(|e| e.0);
        let mut row = vec![0usize; n + 1];
        for e in &edges {
            row[e.0 + 1] += 1;
        }
        for i in 0..n {
            row[i + 1] += row[i];
        }
        let mut val: Vec<f64> = edges.iter().map(|e| e.3).collect();
        for i in 0..n {
            let total: f64 = val[row[i]..row[i + 1]].iter().sum();
            if total > 0.0 {
                for v in &mut val[row[i]..row[i + 1]] {
                    *v /= total;
                }
            }
        }
        // Personalisation (also the dangling distribution) and the start
        // vector: the seeds, or uniform with entry points at 1 + SEED_ENTRY.
        #[allow(clippy::cast_precision_loss)]
        let p: Vec<f64> = if focus.seeds.is_empty() {
            let total = n as f64 + SEED_ENTRY * self.entries.len() as f64;
            (0..n)
                .map(|i| {
                    (1.0 + if self.entries.contains(&i) {
                        SEED_ENTRY
                    } else {
                        0.0
                    }) / total
                })
                .collect()
        } else {
            let total: f64 = focus.seeds.values().sum();
            (0..n)
                .map(|i| focus.seeds.get(&i).copied().unwrap_or(0.0) / total)
                .collect()
        };
        let warm: f64 = self.files.iter().map(|f| f.rank).sum();
        let mut x: Vec<f64> = if focus.seeds.is_empty() && warm > 0.0 {
            self.files.iter().map(|f| f.rank / warm).collect()
        } else {
            p.clone()
        };
        let mut next = vec![0.0; n];
        let mut iterations = 0;
        while iterations < MAX_ITER {
            iterations += 1;
            next.iter_mut().for_each(|v| *v = 0.0);
            let mut dangling = 0.0;
            for i in 0..n {
                if row[i] == row[i + 1] {
                    dangling += x[i];
                    continue;
                }
                for k in row[i]..row[i + 1] {
                    next[edges[k].1] += x[i] * val[k];
                }
            }
            let mut delta = 0.0;
            for j in 0..n {
                let v = (1.0 - DAMPING) * p[j] + DAMPING * (next[j] + dangling * p[j]);
                delta += (v - x[j]).abs();
                x[j] = v;
            }
            if delta < TOLERANCE {
                break;
            }
        }
        // Spread each file's rank over its out-edges onto (definer, name).
        let mut mass: HashMap<(usize, &str), f64> = HashMap::new();
        for i in 0..n {
            for k in row[i]..row[i + 1] {
                let (_, d, name, _) = edges[k];
                *mass.entry((d, name)).or_default() += x[i] * val[k];
            }
        }
        let mut by_def: HashMap<(usize, &str), Vec<&SymNode>> = HashMap::new();
        for s in &self.symbols {
            if let Some(&i) = self.idx.get(&s.file_id) {
                by_def.entry((i, s.name.as_str())).or_default().push(s);
            }
        }
        let mut tags: Vec<(i64, Tag)> = Vec::new();
        for (key, m) in &mass {
            for s in by_def.get(key).into_iter().flatten() {
                tags.push((
                    s.id,
                    Tag {
                        path: self.files[key.0].path.clone(),
                        line: s.line_start,
                        kind: s.kind.clone(),
                        name: s.name.clone(),
                        signature: s.signature.clone(),
                        rank: *m,
                    },
                ));
            }
        }
        tags.sort_by(|a, b| {
            b.1.rank
                .total_cmp(&a.1.rank)
                .then_with(|| a.1.path.cmp(&b.1.path))
                .then(a.1.line.cmp(&b.1.line))
        });
        Ranked {
            files: x,
            tags,
            iterations,
        }
    }

    /// Classify every file by what it defines versus re-exports
    /// (`reexported[i]` = re-exported names of file `i`).
    fn shapes(&self, reexported: &[usize]) -> Vec<Shape> {
        let n = self.files.len();
        let (mut defs, mut fns, mut types) = (vec![0usize; n], vec![0usize; n], vec![0usize; n]);
        for s in &self.symbols {
            let Some(&i) = self.idx.get(&s.file_id) else {
                continue;
            };
            defs[i] += 1;
            match s.kind.as_str() {
                "function" | "method" => fns[i] += 1,
                "class" | "interface" | "type" | "constant" => types[i] += 1,
                _ => {}
            }
        }
        (0..n)
            .map(|i| Shape::of(defs[i], fns[i], types[i], reexported[i]))
            .collect()
    }
}

/// What a file mostly is, for in-edge damping (60 §17 bullet 1).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Shape {
    /// Definitions with behaviour, or nothing notable.
    Plain,
    /// Mostly re-exports: ≥ 1 re-export and no function, or re-exports
    /// above [`BARREL_SHARE`] of its tags.
    Barrel,
    /// Types/traits/interfaces/consts at or above [`TYPE_SHARE`] of its
    /// definitions and no function.
    TypeHeavy,
}

impl Shape {
    #[allow(clippy::cast_precision_loss)]
    fn of(defs: usize, fns: usize, types: usize, reexported: usize) -> Self {
        if reexported >= 1 && fns == 0
            || reexported as f64 > BARREL_SHARE * (reexported + defs) as f64
        {
            Self::Barrel
        } else if defs > 0 && fns == 0 && types as f64 >= TYPE_SHARE * defs as f64 {
            Self::TypeHeavy
        } else {
            Self::Plain
        }
    }
}

/// aider's `is_snake or is_camel`: an underscore beside letters, or mixed case.
fn is_snake_or_camel(s: &str) -> bool {
    let snake = s.contains('_') && s.chars().any(char::is_alphabetic);
    let camel = s.chars().any(char::is_uppercase) && s.chars().any(char::is_lowercase);
    snake || camel
}

/// Recompute the unpersonalised ranks (entry-point seeded, 60 §17 item 9)
/// and store them in `files.rank` and `symbols.rank` (60 §7), the entry
/// points as JSON in `meta.entry_points`. Returns `(files, iterations)`.
pub async fn persist(pool: &SqlitePool) -> anyhow::Result<(usize, usize)> {
    let g = Graph::load(pool).await?;
    let r = g.rank(&Focus::default());
    let files: Vec<(i64, f64)> = g
        .files
        .iter()
        .zip(&r.files)
        .map(|(f, &x)| (f.id, x))
        .collect();
    let symbols: Vec<(i64, f64)> = r.tags.iter().map(|(id, t)| (*id, t.rank)).collect();
    store::update_ranks(pool, &files, &symbols).await?;
    store::set_meta(
        pool,
        "entry_points",
        &serde_json::to_string(&g.entry_paths())?,
    )
    .await?;
    Ok((files.len(), r.iterations))
}

/// aider's tree: `path`, then `  line: signature` per ranked tag in line
/// order; files by their best tag. Renders the most tags whose text fits
/// `budget` (chars/3), accepting up to 15 % over. Returns `(text, kept)`.
pub fn render(tags: &[Tag], budget: u32) -> (String, usize) {
    let tree = |k: usize| -> String {
        let mut files: Vec<(&str, Vec<&Tag>)> = Vec::new();
        for t in &tags[..k] {
            match files.iter_mut().find(|(p, _)| *p == t.path) {
                Some((_, v)) => v.push(t),
                None => files.push((t.path.as_str(), vec![t])),
            }
        }
        let mut out = String::new();
        for (p, v) in &mut files {
            v.sort_by_key(|t| t.line);
            out.push_str(p);
            out.push('\n');
            for t in v.iter() {
                out.push_str(&format!("  {}: {}\n", t.line, t.signature));
            }
        }
        out
    };
    let budget_f = f64::from(budget);
    let (mut lo, mut hi) = (0usize, tags.len());
    let mut best: Option<(String, usize, u32)> = None;
    #[allow(clippy::cast_possible_truncation, clippy::cast_sign_loss)]
    let mut mid = ((budget_f / 25.0) as usize)
        .clamp(1, tags.len().max(1))
        .min(tags.len());
    while lo <= hi && !tags.is_empty() {
        let text = tree(mid);
        let tokens = estimate_tokens(text.len());
        let err = (f64::from(tokens) - budget_f).abs() / budget_f.max(1.0);
        let better = best.as_ref().is_none_or(|(_, _, t)| tokens > *t);
        if (tokens <= budget && better) || err < FIT_SLACK {
            best = Some((text, mid, tokens));
            if err < FIT_SLACK {
                break;
            }
        }
        if tokens < budget {
            lo = mid + 1;
        } else {
            let Some(h) = mid.checked_sub(1) else { break };
            hi = h;
        }
        mid = (lo + hi) / 2;
        if mid > tags.len() || mid == 0 && lo > 0 {
            break;
        }
    }
    match best {
        Some((text, k, _)) => (text, k),
        None => (String::new(), 0),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn file(id: i64, path: &str) -> FileNode {
        FileNode {
            id,
            path: path.to_owned(),
            language: "rust".into(),
            lines: 10,
            rank: 0.0,
        }
    }

    fn sym(id: i64, file_id: i64, name: &str) -> SymNode {
        kind_sym(id, file_id, name, "function")
    }

    fn kind_sym(id: i64, file_id: i64, name: &str, kind: &str) -> SymNode {
        SymNode {
            id,
            file_id,
            name: name.to_owned(),
            kind: kind.into(),
            line_start: id,
            signature: format!("{kind} {name}"),
        }
    }

    fn edge(src: i64, dst: i64, name: &str, n: i64) -> EdgeRow {
        EdgeRow {
            src,
            dst,
            name: name.to_owned(),
            n,
        }
    }

    fn import(src: i64, dst: i64, names: &[&str], used: bool) -> ImportRow {
        ImportRow {
            src,
            dst,
            names: (!names.is_empty()).then(|| names.iter().map(|s| (*s).to_owned()).collect()),
            used,
        }
    }

    fn graph(
        files: Vec<FileNode>,
        symbols: Vec<SymNode>,
        edges: Vec<EdgeRow>,
        imports: Vec<ImportRow>,
    ) -> Graph {
        let idx = files.iter().enumerate().map(|(i, f)| (f.id, i)).collect();
        Graph {
            files,
            idx,
            symbols,
            edges,
            imports,
            cochanges: Vec::new(),
            entries: HashSet::new(),
        }
    }

    /// Four files: `core` is used by everyone, `util` by two, `app` and
    /// `island` by nobody (`island` defines something → self-edge).
    fn toy() -> Graph {
        graph(
            vec![
                file(1, "src/core.rs"),
                file(2, "src/util.rs"),
                file(3, "src/app.rs"),
                file(4, "src/island.rs"),
            ],
            vec![
                sym(1, 1, "open_database"),
                sym(2, 2, "helper"),
                sym(3, 3, "main"),
                sym(4, 4, "alone"),
            ],
            vec![
                edge(2, 1, "open_database", 3),
                edge(3, 1, "open_database", 1),
                edge(4, 1, "open_database", 1),
                edge(3, 2, "helper", 2),
                edge(4, 2, "helper", 1),
            ],
            vec![import(3, 1, &["open_database"], true)],
        )
    }

    /// `lib.rs` defines `Runtime` (used by `app`) and re-exports `SideCall`
    /// from `side_call.rs`, which nobody references (60 §17 bullet 1).
    /// `index.rs` is a pure barrel over `impl_.rs`; `app` imports through it.
    fn reexports() -> Graph {
        graph(
            vec![
                file(1, "src/lib.rs"),
                file(2, "src/side_call.rs"),
                file(3, "src/app.rs"),
                file(4, "src/index.rs"),
                file(5, "src/impl_.rs"),
            ],
            vec![
                kind_sym(1, 1, "Runtime", "class"),
                sym(2, 1, "spawn_runtime"),
                kind_sym(3, 2, "SideCall", "class"),
                sym(4, 3, "main"),
                sym(5, 5, "real_thing"),
            ],
            vec![
                edge(3, 1, "spawn_runtime", 2),
                edge(3, 5, "real_thing", 1),
                edge(2, 1, "Runtime", 1),
            ],
            vec![
                import(1, 2, &["SideCall"], false),
                import(3, 1, &["spawn_runtime"], true),
                import(4, 5, &["real_thing"], false),
                import(3, 4, &["real_thing"], true),
            ],
        )
    }

    #[test]
    fn reexport_edges_route_to_the_definer_and_barrels_are_damped() {
        let g = reexports();
        let r = g.rank(&Focus::default());
        let at = |p: &str| g.files.iter().position(|f| f.path == p).unwrap();
        // Before: every import row was an edge, so `lib.rs` poured its rank
        // down the re-export and `side_call.rs` came second.
        let mut old = reexports();
        old.imports.iter_mut().for_each(|i| i.used = true);
        let o = old.rank(&Focus::default());
        assert!(
            o.files[at("src/side_call.rs")] > o.files[at("src/impl_.rs")],
            "{:?}",
            o.files
        );
        // After: the re-export is no edge; `side_call.rs` ranks like any unreferenced file.
        assert!(
            r.files[at("src/lib.rs")] > 2.0 * r.files[at("src/side_call.rs")],
            "{:?}",
            r.files
        );
        assert!(
            r.files[at("src/side_call.rs")] < 1.1 * r.files[at("src/app.rs")],
            "{:?}",
            r.files
        );
        // `app`'s import of `real_thing` through the barrel lands on `impl_.rs`, not `index.rs`.
        assert!(
            r.files[at("src/impl_.rs")] > r.files[at("src/index.rs")],
            "{:?}",
            r.files
        );
        assert!(!r.tags.iter().any(|(_, t)| t.path == "src/side_call.rs"));
        let shapes = g.shapes(&[0, 0, 0, 1, 0]);
        assert_eq!(
            shapes,
            [
                Shape::Plain,
                Shape::TypeHeavy,
                Shape::Plain,
                Shape::Barrel,
                Shape::Plain
            ]
        );
    }

    #[test]
    fn shape_thresholds() {
        // ≥ 1 re-export and no function → barrel; > 60 % re-exports → barrel.
        assert_eq!(Shape::of(2, 0, 2, 1), Shape::Barrel);
        assert_eq!(Shape::of(2, 1, 1, 4), Shape::Barrel);
        assert_eq!(Shape::of(2, 1, 1, 3), Shape::Plain);
        // ≥ 80 % types and no function → type-heavy; one method flips it.
        assert_eq!(Shape::of(5, 0, 4, 0), Shape::TypeHeavy);
        assert_eq!(Shape::of(5, 0, 3, 0), Shape::Plain);
        assert_eq!(Shape::of(5, 1, 4, 0), Shape::Plain);
        assert_eq!(Shape::of(0, 0, 0, 0), Shape::Plain);
    }

    #[test]
    fn type_heavy_in_edges_are_damped() {
        // `types.rs` (two structs) and `svc.rs` (one fn) are each referenced once by `app`.
        let mk = |kind: &str| {
            graph(
                vec![
                    file(1, "src/types.rs"),
                    file(2, "src/svc.rs"),
                    file(3, "src/app.rs"),
                ],
                vec![
                    kind_sym(1, 1, "Config", kind),
                    kind_sym(2, 1, "Options", kind),
                    sym(3, 2, "serve"),
                    sym(4, 3, "main"),
                ],
                vec![edge(3, 1, "Config", 1), edge(3, 2, "serve", 1)],
                Vec::new(),
            )
        };
        let typed = mk("class").rank(&Focus::default());
        let plain = mk("function").rank(&Focus::default());
        // Same multipliers on both edges: equal without damping, half with.
        assert!(
            (plain.files[0] - plain.files[1]).abs() < 1e-9,
            "{:?}",
            plain.files
        );
        assert!(
            typed.files[0] < plain.files[0] && typed.files[0] < typed.files[1],
            "{:?}",
            typed.files
        );
    }

    #[test]
    fn seeds_from_edited_cochange_and_entry_points() {
        let mut g = toy();
        g.cochanges = vec![(2, 4, 3), (1, 2, 1)];
        let f = g.focus(&["src/core.rs".to_owned()], &["src/util.rs".to_owned()]);
        assert_eq!(f.seeds[&0], SEED_MENTIONED);
        assert_eq!(f.seeds[&1], SEED_EDITED);
        assert_eq!(f.seeds[&3], SEED_COCHANGE, "island: 3 shared commits");
        assert!(!f.seeds.contains_key(&2));
        assert!(f.files.contains(&0) && f.files.contains(&1) && !f.files.contains(&3));
        // An edited file outranks a mentioned one of equal standing.
        let g2 = toy();
        let r = g2.rank(&g2.focus(&["src/app.rs".to_owned()], &["src/island.rs".to_owned()]));
        assert!(r.files[3] > r.files[2], "{:?}", r.files);
        // No signal: entry points get 1 + SEED_ENTRY of the teleport mass.
        let mut g3 = toy();
        g3.entries = [2].into_iter().collect();
        let base = toy().rank(&Focus::default());
        let seeded = g3.rank(&Focus::default());
        assert!(
            seeded.files[2] > base.files[2],
            "{:?} vs {:?}",
            seeded.files,
            base.files
        );
        assert!((seeded.files.iter().sum::<f64>() - 1.0).abs() < 1e-6);
        // Unknown edited paths are ignored, not an error.
        assert!(toy().focus(&[], &["nope.rs".to_owned()]).seeds.is_empty());
    }

    #[test]
    fn ranks_the_most_referenced_file_first_and_converges() {
        let g = toy();
        let r = g.rank(&Focus::default());
        assert!(
            r.iterations > 1 && r.iterations < MAX_ITER,
            "{}",
            r.iterations
        );
        assert!((r.files.iter().sum::<f64>() - 1.0).abs() < 1e-6);
        assert!(
            r.files[0] > r.files[1] && r.files[1] > r.files[2],
            "{:?}",
            r.files
        );
        assert!(r.files[3] > 0.0, "an isolated definer keeps a rank");
        assert_eq!(r.tags[0].1.name, "open_database");
        assert_eq!(r.tags[1].1.name, "helper");
        // `main` and `alone` receive no spread: nothing references them.
        assert_eq!(r.tags.len(), 2);
    }

    #[test]
    fn focus_flips_the_order() {
        let g = toy();
        let f = g.focus(
            &[
                "src/util.rs".to_owned(),
                "helper".to_owned(),
                "src/".to_owned(),
            ],
            &[],
        );
        assert_eq!(f.files.len(), 4, "`src/` names every file");
        assert!(f.idents.contains("helper") && !f.idents.contains("src/util.rs"));
        let base = g.rank(&Focus::default());
        let helper = |r: &Ranked| {
            r.tags
                .iter()
                .find(|(_, t)| t.name == "helper")
                .unwrap()
                .1
                .rank
        };
        // Identifier focus: ×10 on `helper`'s edges — its share of the spread rises.
        let r = g.rank(&g.focus(&["helper".to_owned()], &[]));
        assert!(
            helper(&r) > 1.5 * helper(&base),
            "{} vs {}",
            helper(&r),
            helper(&base)
        );
        // File focus on `app.rs`: it takes the teleport mass (top file) and its ×50 out-edges lift
        // `helper` (which it references twice) — `open_database` keeps first place on its own
        // long-name ×10 plus the mass `util` and `island` send it.
        let r = g.rank(&g.focus(&["src/app.rs".to_owned()], &[]));
        assert_eq!(
            r.files
                .iter()
                .enumerate()
                .max_by(|a, b| a.1.total_cmp(b.1))
                .unwrap()
                .0,
            2,
            "{:?}",
            r.files
        );
        assert!(
            helper(&r) > 1.5 * helper(&base),
            "{} vs {}",
            helper(&r),
            helper(&base)
        );
        assert_eq!(r.tags[0].1.name, "open_database", "{:?}", r.tags);
        // Personalising toward the island puts it first.
        let r = g.rank(&g.focus(&["src/island.rs".to_owned()], &[]));
        assert_eq!(
            r.files
                .iter()
                .enumerate()
                .max_by(|a, b| a.1.total_cmp(b.1))
                .unwrap()
                .0,
            3,
            "{:?}",
            r.files
        );
    }

    #[test]
    fn render_fits_the_budget_within_slack() {
        let tags: Vec<Tag> = (0..200)
            .map(|i| Tag {
                path: format!("src/f{}.rs", i % 7),
                line: i,
                kind: "function".into(),
                name: format!("fn_{i}"),
                signature: format!("pub fn fn_{i}(a: u32, b: &str) -> Result<()>"),
                rank: 1.0 / (i as f64 + 1.0),
            })
            .collect();
        let (text, kept) = render(&tags, 300);
        let tokens = estimate_tokens(text.len());
        assert!(
            (200..=345).contains(&tokens),
            "{tokens} tokens for {kept} tags"
        );
        assert!(kept > 0 && kept < 200);
        assert!(text.starts_with("src/f0.rs\n  0: pub fn fn_0"));
        let (text, kept) = render(&tags, 1_000_000);
        assert_eq!(kept, 200);
        assert_eq!(text.lines().count(), 207);
        assert_eq!(render(&[], 100), (String::new(), 0));
    }

    #[test]
    fn snake_or_camel() {
        assert!(is_snake_or_camel("open_db") && is_snake_or_camel("ToolRegistry"));
        assert!(
            !is_snake_or_camel("new") && !is_snake_or_camel(IMPORT) && !is_snake_or_camel("HTTP")
        );
    }
}
