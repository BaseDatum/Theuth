//! The CodeMap lenses (73 §5.2–5.3): `codemap_here` / `codemap_fit` /
//! `codemap_change` read the store and never walk the tree; `codemap_touch`
//! stats and re-parses only the named paths (or forces a reconcile). Every
//! reply carries [`LensMeta`]. Roots must already be indexed by a `code_*`
//! call: a panel never triggers a first index.

use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::Instant;

use serde::{Deserialize, Serialize};

use crate::index::{Codeindex, Reconciled, RootIndex};
use crate::rank::{self, Graph};
use crate::store::{self, Neighbour, SymbolRow};
use crate::tools::{self, CodeImpactArgs, CodeImpactTool, Picked};
use crate::{root, walk};

/// Default `codemap_here` rows (73 §5.2).
pub const HERE_LIMIT: u32 = 2000;
/// Default `codemap_fit` nodes, focus included (73 §5.2).
pub const FIT_LIMIT: u32 = 20;
/// `codemap_change` band caps, the popped-out sizes (73 §6.4); the panel
/// cuts further client-side with its own counted bundles.
pub const CHANGE_CAPS: [usize; 4] = [20, 30, 30, 30];
/// `ChangeFile.symbols` cap (73 §5.3).
const CHANGE_SYMBOLS: usize = 12;
/// `MapRow.signature` cap (73 §5.3).
const SIGNATURE_CHARS: usize = 200;

// --- messages ---------------------------------------------------------------

/// `ask` → `Result<HereLens, String>`.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Here {
    /// Indexed root.
    pub root: PathBuf,
    /// Identifiers and/or root-relative paths to personalise toward.
    pub focus: Option<Vec<String>>,
    /// Root-relative paths edited this session.
    pub edited: Option<Vec<String>>,
    /// Rows; default [`HERE_LIMIT`].
    pub limit: Option<u32>,
}

/// `ask` → `Result<FitLens, String>`.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Fit {
    /// Indexed root.
    pub root: PathBuf,
    /// A moniker, a name, `path:line` or a numeric symbol id.
    pub symbol: String,
    /// Nodes incl. the focus; default [`FIT_LIMIT`].
    pub limit: Option<u32>,
}

/// `ask` → `Result<ChangeLens, String>`.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Change {
    /// Indexed root.
    pub root: PathBuf,
    /// `main..HEAD`, `HEAD~1`, `WORKTREE`; neither this nor `files` = `WORKTREE`.
    pub git_range: Option<String>,
    /// Changed files instead of a range.
    pub files: Option<Vec<String>>,
    /// Test files in the direct/transitive bands too (default false).
    pub include_tests: Option<bool>,
}

/// `ask` → `Result<LensMeta, String>`.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Touch {
    /// Indexed root.
    pub root: PathBuf,
    /// Paths to stat and re-parse; `None` = forced full reconcile.
    pub paths: Option<Vec<String>>,
}

// --- reply types (73 §5.3) ---------------------------------------------------

/// Freshness and timing on every lens reply.
#[derive(Clone, Debug, Serialize, Deserialize, schemars::JsonSchema, kameo::Reply)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS), ts(export))]
pub struct LensMeta {
    /// Canonical root.
    pub root: PathBuf,
    /// Per-root sequence the reply reflects.
    #[cfg_attr(feature = "ts", ts(type = "number"))]
    pub seq: u64,
    /// `files` rows.
    #[cfg_attr(feature = "ts", ts(type = "number"))]
    pub indexed_files: i64,
    /// A background reconcile is still writing.
    pub stale: bool,
    /// Files it still has to write.
    pub pending: usize,
    /// The reconcile guard fired on the last walk.
    pub suspect_root: bool,
    /// Why, when `suspect_root`.
    pub hint: Option<String>,
    /// The last walk stopped at the file cap.
    pub truncated_walk: bool,
    /// `meta.git_head`.
    pub git_head: Option<String>,
    /// Engine time for this reply.
    pub ms: u32,
}

/// One ranked definition of the Here lens.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, schemars::JsonSchema)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS), ts(export))]
pub struct MapRow {
    /// Stable identity.
    pub moniker: String,
    /// Root-relative path.
    pub path: String,
    /// 1-based.
    #[cfg_attr(feature = "ts", ts(type = "number"))]
    pub line: i64,
    /// 1-based, inclusive.
    #[cfg_attr(feature = "ts", ts(type = "number"))]
    pub line_end: i64,
    /// Kind.
    pub kind: String,
    /// Name.
    pub name: String,
    /// Definition line, ≤ 200 chars.
    pub signature: String,
    /// Rank (personalised when `HereLens.personalised`).
    pub rank: f64,
    /// `symbols.exported`.
    pub exported: bool,
    /// Distinct files importing or referencing this file.
    pub importers: u32,
    /// Cross-file resolved references to this definition.
    pub callers: u32,
    /// File modified within 48 h.
    pub recent: bool,
    /// Manifest entry point.
    pub entry: bool,
    /// `files.is_test`.
    pub is_test: bool,
}

/// Coverage row: a depth-2 directory with parsed files and no row in `rows`.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, schemars::JsonSchema)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS), ts(export))]
pub struct DirRow {
    /// `crates/tools`, `src/components`.
    pub path: String,
    /// Parsed files under it.
    pub files: u32,
    /// Ranked definitions under it that the cut dropped.
    pub ranked_rows: u32,
    /// Sum of `importers` over its files.
    pub importers: u32,
}

/// `codemap_here`.
#[derive(Clone, Debug, Serialize, Deserialize, schemars::JsonSchema, kameo::Reply)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS), ts(export))]
pub struct HereLens {
    /// Freshness.
    pub meta: LensMeta,
    /// Files by best rank, symbols by line.
    pub rows: Vec<MapRow>,
    /// Ranked definitions beyond `limit`.
    pub dropped: u32,
    /// Directories with no row.
    pub dirs: Vec<DirRow>,
    /// Ranked toward `focus`/`edited`.
    pub personalised: bool,
    /// Served from the map cache.
    pub cache_hit: bool,
}

/// One node of the butterfly.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, schemars::JsonSchema)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS), ts(export))]
pub struct FitNode {
    /// Stable identity; `<lang>:<path>` for a file node.
    pub moniker: String,
    /// Root-relative path.
    pub path: String,
    /// 1-based.
    #[cfg_attr(feature = "ts", ts(type = "number"))]
    pub line_start: i64,
    /// 1-based, inclusive.
    #[cfg_attr(feature = "ts", ts(type = "number"))]
    pub line_end: i64,
    /// Kind; `file` for a module-scope referencer.
    pub kind: String,
    /// Name.
    pub name: String,
    /// Definition line.
    pub signature: String,
    /// `files.is_test`.
    pub is_test: bool,
}

/// One bundled edge of the butterfly.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, schemars::JsonSchema)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS), ts(export))]
pub struct FitEdge {
    /// Moniker.
    pub from: String,
    /// Moniker.
    pub to: String,
    /// References bundled.
    pub count: u32,
    /// `call | type | …`
    pub kind: String,
    /// `same_file | import | package | unique`
    pub provenance: String,
    /// Best confidence in the bundle.
    pub confidence: f64,
}

/// `codemap_fit`.
#[derive(Clone, Debug, Serialize, Deserialize, schemars::JsonSchema, kameo::Reply)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS), ts(export))]
pub struct FitLens {
    /// Freshness.
    pub meta: LensMeta,
    /// The centre.
    pub focus: FitNode,
    /// Left column.
    pub callers: Vec<FitNode>,
    /// Right column.
    pub callees: Vec<FitNode>,
    /// Caller → focus and focus → callee.
    pub edges: Vec<FitEdge>,
    /// Refs named like the focus that stayed ambiguous.
    pub ambiguous: u32,
    /// Callers beyond the cap.
    pub more_callers: u32,
    /// Callees beyond the cap.
    pub more_callees: u32,
    /// Non-empty when `symbol` matched several; `focus` is the best-ranked.
    pub candidates: Vec<FitNode>,
}

/// One changed definition.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, schemars::JsonSchema)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS), ts(export))]
pub struct ChangeSym {
    /// Stable identity.
    pub moniker: String,
    /// Name.
    pub name: String,
    /// Kind.
    pub kind: String,
    /// 1-based.
    #[cfg_attr(feature = "ts", ts(type = "number"))]
    pub line: i64,
}

/// One file of a Change band.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, schemars::JsonSchema)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS), ts(export))]
pub struct ChangeFile {
    /// Root-relative path.
    pub path: String,
    /// `added | modified | deleted`; changed band only.
    pub status: Option<String>,
    /// Changed definitions, ≤ 12.
    pub symbols: Vec<ChangeSym>,
    /// Direct band: changed names it references, or `<import>`.
    pub via: Vec<String>,
    /// `files.is_test`.
    pub is_test: bool,
    /// Tests band: `ref` (names a changed symbol) or `subject` (name-derived).
    pub test_link: Option<String>,
    /// 0 changed, 1 direct, 2–3 transitive.
    pub depth: u8,
}

/// `codemap_change`.
#[derive(Clone, Debug, Serialize, Deserialize, schemars::JsonSchema, kameo::Reply)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS), ts(export))]
pub struct ChangeLens {
    /// Freshness.
    pub meta: LensMeta,
    /// `worktree | range | files`
    pub mode: String,
    /// `(old, new)` for `worktree`/`range`.
    pub range: Option<(String, String)>,
    /// Band 0.
    pub changed: Vec<ChangeFile>,
    /// Band 1.
    pub direct: Vec<ChangeFile>,
    /// Band 2.
    pub transitive: Vec<ChangeFile>,
    /// The test lane.
    pub tests: Vec<ChangeFile>,
    /// Dropped per band.
    pub more: [u32; 4],
    /// Same-name references that could not be attributed.
    pub ambiguous_skipped: u32,
    /// Unresolved imports of the changed files.
    pub unresolved_imports: u32,
    /// Changed paths the index does not know.
    pub unindexed: Vec<String>,
    /// Served from the map cache.
    pub cache_hit: bool,
}

// --- shared -----------------------------------------------------------------

/// Open an already-indexed root; the panel never triggers a first index.
async fn open_indexed(ix: &Codeindex, root: &Path) -> anyhow::Result<Arc<RootIndex>> {
    let root = root::discover(Some(root))?;
    if !ix.indexed(&root) {
        anyhow::bail!("{} is not indexed; run a code_* tool first", root.display());
    }
    ix.open_root(&root).await
}

async fn meta(ri: &RootIndex, r: &Reconciled, t: Instant) -> anyhow::Result<LensMeta> {
    Ok(LensMeta {
        root: ri.root.clone(),
        seq: ri.seq(),
        indexed_files: store::map_state(&ri.pool).await?.0,
        stale: r.stale,
        pending: r.pending,
        suspect_root: r.suspect_root,
        hint: r.hint.map(str::to_owned),
        truncated_walk: r.truncated_walk,
        git_head: store::meta(&ri.pool, "git_head").await?,
        ms: u32::try_from(t.elapsed().as_millis()).unwrap_or(u32::MAX),
    })
}

/// Root-relative, forward slashes; absolute paths under the root stripped;
/// blanks dropped.
fn rel_paths(root: &Path, paths: Option<Vec<String>>) -> Vec<String> {
    paths
        .unwrap_or_default()
        .iter()
        .map(|p| {
            let p = Path::new(p.trim());
            root::rel_str(p.strip_prefix(root).unwrap_or(p))
        })
        .filter(|s| !s.is_empty())
        .collect()
}

fn sorted_key(v: &[String]) -> String {
    let mut s = v.to_vec();
    s.sort();
    s.join("\u{1f}")
}

fn node(s: &SymbolRow) -> FitNode {
    FitNode {
        moniker: s.moniker.clone(),
        path: s.path.clone(),
        line_start: s.line_start,
        line_end: s.line_end,
        kind: s.kind.clone(),
        name: s.name.clone(),
        signature: s.signature.clone(),
        is_test: s.is_test,
    }
}

fn u(n: impl TryInto<u32>) -> u32 {
    n.try_into().unwrap_or(u32::MAX)
}

// --- here -------------------------------------------------------------------

/// Body of a Here reply, cached under the map cache (73 §7).
#[derive(Serialize, Deserialize)]
struct HereBody {
    rows: Vec<MapRow>,
    dropped: u32,
    dirs: Vec<DirRow>,
    personalised: bool,
}

/// `codemap_here`: the stored map cut to `limit` rows with badges; with
/// `focus`/`edited`, RANK's personalised rank over the stored graph. Both
/// forms are cached in the `map_cache:` namespace keyed on the index state.
pub async fn here(ix: &Arc<Codeindex>, m: Here) -> anyhow::Result<HereLens> {
    let t = Instant::now();
    let ri = open_indexed(ix, &m.root).await?;
    let r = ri.snapshot();
    let limit = m.limit.unwrap_or(HERE_LIMIT).max(1) as usize;
    let focus: Vec<String> = m
        .focus
        .unwrap_or_default()
        .into_iter()
        .map(|s| s.trim().to_owned())
        .filter(|s| !s.is_empty())
        .collect();
    let edited = rel_paths(&ri.root, m.edited);
    let personalised = !focus.is_empty() || !edited.is_empty();
    let head = tools::head_of(&ri.root).await?;
    let (state, now) = tools::cache_state(&ri, head.as_deref()).await?;
    let key = format!(
        "{state}:here;n={limit};f={};e={}",
        sorted_key(&focus),
        sorted_key(&edited)
    );
    if !r.stale
        && let Some(hit) = store::map_cache_get(&ri.pool, &key).await?
        && let Ok(b) = serde_json::from_str::<HereBody>(&hit)
    {
        return Ok(HereLens {
            meta: meta(&ri, &r, t).await?,
            rows: b.rows,
            dropped: b.dropped,
            dirs: b.dirs,
            personalised: b.personalised,
            cache_hit: true,
        });
    }
    let facts = store::map_facts(&ri.pool, now - rank::RECENT_NS).await?;
    // `(path, symbol row)` in rank order, plus the paths of every ranked tag
    // beyond the cut (for the coverage rows' `ranked_rows`).
    let (rows, total, tail_paths): (Vec<SymbolRow>, usize, Vec<String>) = if personalised {
        let g = Graph::load(&ri.pool).await?;
        let ranked = g.rank(&g.focus(&focus, &edited));
        let ids: Vec<i64> = ranked.tags.iter().take(limit).map(|(id, _)| *id).collect();
        let by_id: HashMap<i64, SymbolRow> = store::symbol_rows(&ri.pool, &ids)
            .await?
            .into_iter()
            .map(|s| (s.symbol_id, s))
            .collect();
        let rows = ranked
            .tags
            .iter()
            .take(limit)
            .filter_map(|(id, tag)| {
                by_id.get(id).map(|s| SymbolRow {
                    rank: tag.rank,
                    ..s.clone()
                })
            })
            .collect();
        let tail = ranked
            .tags
            .iter()
            .skip(limit)
            .map(|(_, tag)| tag.path.clone())
            .collect();
        (rows, ranked.tags.len(), tail)
    } else {
        let (rows, total) = store::ranked_symbol_rows(&ri.pool, limit as i64).await?;
        // Unpersonalised: the dropped tags' directories come from the stored
        // ranks — one more lookup is the honest count.
        let tail = if total as usize > rows.len() {
            store::ranked_symbol_rows(&ri.pool, total)
                .await?
                .0
                .into_iter()
                .skip(rows.len())
                .map(|s| s.path)
                .collect()
        } else {
            Vec::new()
        };
        (rows, total as usize, tail)
    };
    let entries: HashSet<String> = store::meta(&ri.pool, "entry_points")
        .await?
        .and_then(|s| serde_json::from_str(&s).ok())
        .unwrap_or_default();
    // Files by best rank (first appearance in rank order), symbols by line.
    let mut files: Vec<(&str, Vec<&SymbolRow>)> = Vec::new();
    for s in &rows {
        match files.iter_mut().find(|(p, _)| *p == s.path) {
            Some((_, v)) => v.push(s),
            None => files.push((&s.path, vec![s])),
        }
    }
    let mut out_rows = Vec::with_capacity(rows.len());
    for (path, mut syms) in files {
        syms.sort_by_key(|s| s.line_start);
        for s in syms {
            let mut signature = s.signature.clone();
            if signature.chars().count() > SIGNATURE_CHARS {
                signature = signature.chars().take(SIGNATURE_CHARS).collect();
            }
            out_rows.push(MapRow {
                moniker: s.moniker.clone(),
                path: path.to_owned(),
                line: s.line_start,
                line_end: s.line_end,
                kind: s.kind.clone(),
                name: s.name.clone(),
                signature,
                rank: s.rank,
                exported: s.exported,
                importers: u(facts.importers.get(path).copied().unwrap_or(0)),
                callers: u(facts.callers(path, s.line_start).unwrap_or(0)),
                recent: facts.recent.contains(path),
                entry: entries.contains(path),
                is_test: s.is_test,
            });
        }
    }
    // Coverage: depth-2 components with parsed files and no row.
    let shown: HashSet<&str> = out_rows.iter().map(|r| rank::component(&r.path)).collect();
    let mut dirs: HashMap<&str, DirRow> = HashMap::new();
    for f in &facts.files {
        let c = rank::component(f);
        if c.is_empty() || shown.contains(c) {
            continue;
        }
        let d = dirs.entry(c).or_insert_with(|| DirRow {
            path: c.to_owned(),
            files: 0,
            ranked_rows: 0,
            importers: 0,
        });
        d.files += 1;
        d.importers += u(facts.importers.get(f).copied().unwrap_or(0));
    }
    for p in &tail_paths {
        if let Some(d) = dirs.get_mut(rank::component(p)) {
            d.ranked_rows += 1;
        }
    }
    let mut dirs: Vec<DirRow> = dirs.into_values().collect();
    dirs.sort_by(|a, b| b.files.cmp(&a.files).then_with(|| a.path.cmp(&b.path)));
    let body = HereBody {
        rows: out_rows,
        dropped: u(total.saturating_sub(rows.len())),
        dirs,
        personalised,
    };
    if !r.stale {
        store::map_cache_put(&ri.pool, &state, &key, &serde_json::to_string(&body)?).await?;
    }
    Ok(HereLens {
        meta: meta(&ri, &r, t).await?,
        rows: body.rows,
        dropped: body.dropped,
        dirs: body.dirs,
        personalised,
        cache_hit: false,
    })
}

// --- fit --------------------------------------------------------------------

fn neighbour_node(n: &Neighbour) -> FitNode {
    match (&n.moniker, &n.name, &n.kind) {
        (Some(moniker), Some(name), Some(kind)) => FitNode {
            moniker: moniker.clone(),
            path: n.path.clone(),
            line_start: n.line_start,
            line_end: n.line_end,
            kind: kind.clone(),
            name: name.clone(),
            signature: n.signature.clone(),
            is_test: n.is_test,
        },
        _ => FitNode {
            moniker: format!("{}:{}", n.language, n.path),
            path: n.path.clone(),
            line_start: n.line_start,
            line_end: n.line_end,
            kind: "file".into(),
            name: n.path.rsplit('/').next().unwrap_or(&n.path).to_owned(),
            signature: String::new(),
            is_test: n.is_test,
        },
    }
}

fn edge(from: &str, to: &str, n: &Neighbour) -> FitEdge {
    FitEdge {
        from: from.to_owned(),
        to: to.to_owned(),
        count: u(n.count),
        kind: n.ref_kind.clone(),
        provenance: n.provenance.clone(),
        confidence: n.confidence,
    }
}

/// `codemap_fit`: the butterfly around `symbol` — callers left, callees
/// right, ≤ `limit` nodes with the focus; the remainder is counted.
pub async fn fit(ix: &Arc<Codeindex>, m: Fit) -> anyhow::Result<FitLens> {
    let t = Instant::now();
    let ri = open_indexed(ix, &m.root).await?;
    let r = ri.snapshot();
    let symbol = m.symbol.trim();
    if symbol.is_empty() {
        anyhow::bail!("symbol is empty");
    }
    let (def, candidates) = match tools::pick(&ri.pool, &ri.root, symbol).await? {
        Picked::One(d) => (d, Vec::new()),
        Picked::Several(defs) => {
            let ids: Vec<i64> = defs.iter().map(|d| d.symbol_id).collect();
            let mut rows = store::symbol_rows(&ri.pool, &ids).await?;
            rows.sort_by(|a, b| {
                b.rank
                    .total_cmp(&a.rank)
                    .then_with(|| a.path.cmp(&b.path))
                    .then_with(|| a.line_start.cmp(&b.line_start))
            });
            let best = rows[0].symbol_id;
            let def = defs
                .into_iter()
                .find(|d| d.symbol_id == best)
                .expect("picked id");
            (def, rows.iter().map(node).collect())
        }
        Picked::None => anyhow::bail!("no definition named {symbol:?}"),
    };
    let focus = store::symbol_rows(&ri.pool, &[def.symbol_id])
        .await?
        .pop()
        .map(|s| node(&s))
        .ok_or_else(|| anyhow::anyhow!("definition vanished"))?;
    let callers = store::callers_of(&ri.pool, def.symbol_id).await?;
    let callees = store::callees_of(
        &ri.pool,
        def.symbol_id,
        &def.path,
        def.line_start,
        def.line_end,
    )
    .await?;
    let ambiguous = store::ambiguous_named(&ri.pool, &def.name, None).await?;
    // Split the budget evenly (callers take the odd one); a short side hands
    // its slack to the other.
    let budget = (m.limit.unwrap_or(FIT_LIMIT).max(1) as usize).saturating_sub(1);
    let want_callers = budget / 2 + budget % 2;
    let n_callees = callees
        .len()
        .min(budget / 2 + want_callers.saturating_sub(callers.len()));
    let n_callers = callers.len().min(budget - n_callees);
    let mut edges = Vec::with_capacity(n_callers + n_callees);
    let caller_nodes: Vec<FitNode> = callers[..n_callers]
        .iter()
        .map(|n| {
            let node = neighbour_node(n);
            edges.push(edge(&node.moniker, &focus.moniker, n));
            node
        })
        .collect();
    let callee_nodes: Vec<FitNode> = callees[..n_callees]
        .iter()
        .map(|n| {
            let node = neighbour_node(n);
            edges.push(edge(&focus.moniker, &node.moniker, n));
            node
        })
        .collect();
    Ok(FitLens {
        meta: meta(&ri, &r, t).await?,
        focus,
        callers: caller_nodes,
        callees: callee_nodes,
        edges,
        ambiguous: u(ambiguous),
        more_callers: u(callers.len() - n_callers),
        more_callees: u(callees.len() - n_callees),
        candidates,
    })
}

// --- change -----------------------------------------------------------------

/// Body of a Change reply, cached under the map cache.
#[derive(Serialize, Deserialize)]
struct ChangeBody {
    mode: String,
    range: Option<(String, String)>,
    changed: Vec<ChangeFile>,
    direct: Vec<ChangeFile>,
    transitive: Vec<ChangeFile>,
    tests: Vec<ChangeFile>,
    more: [u32; 4],
    ambiguous_skipped: u32,
    unresolved_imports: u32,
    unindexed: Vec<String>,
}

/// `codemap_change`: `code_impact` over the open index (no reconcile) cut
/// into the four bands with counted remainders; cached on the index state.
pub async fn change(ix: &Arc<Codeindex>, m: Change) -> anyhow::Result<ChangeLens> {
    let t = Instant::now();
    let ri = open_indexed(ix, &m.root).await?;
    let r = ri.snapshot();
    let files = rel_paths(&ri.root, m.files);
    let git_range = m
        .git_range
        .as_deref()
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .map(str::to_owned)
        .or_else(|| files.is_empty().then(|| "WORKTREE".to_owned()));
    let include_tests = m.include_tests.unwrap_or(false);
    let head = tools::head_of(&ri.root).await?;
    let (state, _) = tools::cache_state(&ri, head.as_deref()).await?;
    let key = format!(
        "{state}:change;r={};f={};t={include_tests}",
        git_range.as_deref().unwrap_or(""),
        sorted_key(&files)
    );
    if !r.stale
        && let Some(hit) = store::map_cache_get(&ri.pool, &key).await?
        && let Ok(b) = serde_json::from_str::<ChangeBody>(&hit)
    {
        return Ok(lens_of(b, meta(&ri, &r, t).await?, true));
    }
    let args = CodeImpactArgs {
        files: (!files.is_empty()).then(|| files.iter().map(PathBuf::from).collect()),
        git_range,
        include_tests: Some(include_tests),
        root: Some(ri.root.clone()),
        // The text render is not used; the smallest budget keeps it short.
        budget: Some(1),
    };
    let out = CodeImpactTool(Arc::clone(ix))
        .run(args, Arc::clone(&ri), r)
        .await?;
    if out.is_error {
        anyhow::bail!("{}", out.content);
    }
    let v = out
        .structured
        .ok_or_else(|| anyhow::anyhow!("code_impact returned no structured output"))?;
    let arr = |k: &str| v[k].as_array().cloned().unwrap_or_default();
    let s = |x: &serde_json::Value| x.as_str().unwrap_or_default().to_owned();
    let strs = |k: &str| -> Vec<String> { arr(k).iter().map(s).collect() };
    let (mode, range) = match (s(&v["mode"]).as_str(), &v["range"]) {
        ("files", _) => ("files".to_owned(), None),
        (_, rg) if rg["new"] == "WORKTREE" => {
            let short = head.as_deref().map_or("HEAD", |h| &h[..h.len().min(8)]);
            (
                "worktree".to_owned(),
                Some((short.to_owned(), "WORKTREE".to_owned())),
            )
        }
        (_, rg) => ("range".to_owned(), Some((s(&rg["old"]), s(&rg["new"])))),
    };
    let file = |path: String, depth: u8| ChangeFile {
        is_test: walk::is_test_path(&path),
        path,
        status: None,
        symbols: Vec::new(),
        via: Vec::new(),
        test_link: None,
        depth,
    };
    let changed: Vec<ChangeFile> = arr("changed")
        .iter()
        .map(|c| {
            let mut f = file(s(&c["path"]), 0);
            f.status = c["status"].as_str().map(str::to_owned);
            f.symbols = arr_of(&c["symbols_changed"])
                .iter()
                .take(CHANGE_SYMBOLS)
                .map(|x| ChangeSym {
                    moniker: s(&x["moniker"]),
                    name: s(&x["name"]),
                    kind: s(&x["kind"]),
                    line: x["line"].as_i64().unwrap_or(0),
                })
                .collect();
            f
        })
        .collect();
    let changed_paths: HashSet<String> = changed.iter().map(|f| f.path.clone()).collect();
    // Test files live in the lane, not the bands; a changed file that also
    // references another changed file stays in band 0.
    let direct: Vec<ChangeFile> = arr("direct")
        .iter()
        .filter(|d| d["changed"] != true)
        .map(|d| {
            let mut f = file(s(&d["path"]), 1);
            f.is_test = d["is_test"].as_bool().unwrap_or(f.is_test);
            f.via = arr_of(&d["via"]).iter().map(s).collect();
            f
        })
        .filter(|f| !f.is_test)
        .collect();
    let transitive: Vec<ChangeFile> = arr("transitive")
        .iter()
        .map(|d| {
            file(
                s(&d["path"]),
                u8::try_from(d["depth"].as_u64().unwrap_or(2)).unwrap_or(3),
            )
        })
        .filter(|f| !f.is_test && !changed_paths.contains(&f.path))
        .collect();
    let by_ref: Vec<String> = strs("tests");
    let mut tests: Vec<ChangeFile> = by_ref
        .iter()
        .map(|p| {
            let mut f = file(p.clone(), 0);
            f.is_test = true;
            f.test_link = Some("ref".into());
            f
        })
        .collect();
    for p in strs("tests_by_subject") {
        if !by_ref.contains(&p) {
            let mut f = file(p, 0);
            f.is_test = true;
            f.test_link = Some("subject".into());
            tests.push(f);
        }
    }
    let mut bands = [changed, direct, transitive, tests];
    let mut more = [0u32; 4];
    for (i, b) in bands.iter_mut().enumerate() {
        more[i] = u(b.len().saturating_sub(CHANGE_CAPS[i]));
        b.truncate(CHANGE_CAPS[i]);
    }
    let [changed, direct, transitive, tests] = bands;
    let body = ChangeBody {
        mode,
        range,
        changed,
        direct,
        transitive,
        tests,
        more,
        ambiguous_skipped: u(v["ambiguous_skipped"].as_i64().unwrap_or(0)),
        unresolved_imports: u(v["unresolved_imports"].as_i64().unwrap_or(0)),
        unindexed: strs("unindexed"),
    };
    if !r.stale {
        store::map_cache_put(&ri.pool, &state, &key, &serde_json::to_string(&body)?).await?;
    }
    Ok(lens_of(body, meta(&ri, &r, t).await?, false))
}

fn arr_of(v: &serde_json::Value) -> Vec<serde_json::Value> {
    v.as_array().cloned().unwrap_or_default()
}

fn lens_of(b: ChangeBody, meta: LensMeta, cache_hit: bool) -> ChangeLens {
    ChangeLens {
        meta,
        mode: b.mode,
        range: b.range,
        changed: b.changed,
        direct: b.direct,
        transitive: b.transitive,
        tests: b.tests,
        more: b.more,
        ambiguous_skipped: b.ambiguous_skipped,
        unresolved_imports: b.unresolved_imports,
        unindexed: b.unindexed,
        cache_hit,
    }
}

// --- touch ------------------------------------------------------------------

/// `codemap_touch`: re-parse the named paths, or force a full reconcile
/// when `paths` is `None` (the **Reconcile** button).
pub async fn touch(ix: &Arc<Codeindex>, m: Touch) -> anyhow::Result<LensMeta> {
    let t = Instant::now();
    let ri = open_indexed(ix, &m.root).await?;
    let r = match m.paths {
        Some(paths) => ix.touch(&ri, &paths).await?,
        None => ix.reconcile(&ri, true).await?,
    };
    meta(&ri, &r, t).await
}
