//! Ranking and the map (60 §7): aider's algorithm over the file graph —
//! referencer → definer edges from resolved `refs` (confidence ≥ 0.5) and
//! `imports`, weighted by identifier rarity, personalised PageRank (own
//! sparse power iteration), rank spread per `(file, identifier)`, rendered
//! top-down to a token budget.

use std::collections::{HashMap, HashSet};

use sqlx::{FromRow, SqlitePool};

use crate::store;
use crate::tools::estimate_tokens;

/// Damping factor.
const DAMPING: f64 = 0.85;
/// Power-iteration cap.
const MAX_ITER: usize = 50;
/// Stop when the L1 change of one iteration is below this.
const TOLERANCE: f64 = 1e-6;
/// Self-edge weight for files that define something nobody references.
const SELF_EDGE: f64 = 0.1;
/// Weight of an `imports` edge before `mul` (60 §22): a `pub use` re-export
/// otherwise carries the crate root's whole rank to one module.
const IMPORT_EDGE: f64 = 0.1;
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
    /// Referenced identifier (or [`IMPORT`]).
    pub name: String,
    /// References grouped under this edge.
    pub n: i64,
}

/// The file graph as the store gives it.
pub struct Graph {
    /// Parsed files.
    pub files: Vec<FileNode>,
    /// `files.id` → index into `files`.
    idx: HashMap<i64, usize>,
    /// Every symbol of every parsed file.
    pub symbols: Vec<SymNode>,
    /// Raw edges.
    edges: Vec<EdgeRow>,
}

/// What a `code_map` call personalises toward.
#[derive(Default)]
pub struct Focus {
    /// Identifiers: ×10 on their edges.
    pub idents: HashSet<String>,
    /// Indexes into [`Graph::files`]: ×50 on their out-edges, personalisation mass.
    pub files: HashSet<usize>,
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
    /// `confidence ≥ 0.5` across files grouped per (R, D, name), and
    /// `imports` edges as [`IMPORT`].
    pub async fn load(pool: &SqlitePool) -> anyhow::Result<Self> {
        let files: Vec<FileNode> =
            sqlx::query_as("SELECT id, path, language, lines, rank FROM files WHERE language <> 'other' ORDER BY id")
                .fetch_all(pool)
                .await?;
        let symbols: Vec<SymNode> =
            sqlx::query_as("SELECT id, file_id, name, kind, line_start, signature FROM symbols")
                .fetch_all(pool)
                .await?;
        let mut edges: Vec<EdgeRow> = sqlx::query_as(
            "SELECT r.file_id AS src, s.file_id AS dst, r.name, count(*) AS n FROM refs r JOIN symbols s ON s.id = r.to_symbol \
             WHERE r.confidence >= 0.5 AND r.file_id <> s.file_id GROUP BY r.file_id, s.file_id, r.name",
        )
        .fetch_all(pool)
        .await?;
        // `mod x;` rows are module-tree edges, not uses (60 §21): a crate
        // root with few `use`s would otherwise pour its rank into them.
        let imports: Vec<(i64, i64)> = sqlx::query_as(
            "SELECT DISTINCT file_id, target_file FROM imports WHERE target_file IS NOT NULL AND target_file <> file_id \
             AND specifier NOT LIKE 'mod:%'",
        )
        .fetch_all(pool)
        .await?;
        edges.extend(imports.into_iter().map(|(src, dst)| EdgeRow {
            src,
            dst,
            name: IMPORT.to_owned(),
            n: 1,
        }));
        let idx = files.iter().enumerate().map(|(i, f)| (f.id, i)).collect();
        Ok(Self {
            files,
            idx,
            symbols,
            edges,
        })
    }

    /// Split `focus` entries into files (an entry that names an indexed
    /// file or directory, root-relative) and identifiers (the rest).
    pub fn focus(&self, entries: &[String]) -> Focus {
        let mut f = Focus::default();
        for e in entries {
            let e = e.trim().trim_start_matches("./").trim_end_matches('/');
            if e.is_empty() {
                continue;
            }
            let hits: Vec<usize> = self
                .files
                .iter()
                .enumerate()
                .filter(|(_, n)| {
                    n.path == e
                        || n.path
                            .strip_prefix(e)
                            .is_some_and(|rest| rest.starts_with('/'))
                })
                .map(|(i, _)| i)
                .collect();
            if hits.is_empty() {
                // An identifier names its definers too (60 §21): a symbol
                // referenced only inside its own file has no edge to boost.
                f.idents.insert(e.to_owned());
                f.files.extend(
                    self.symbols
                        .iter()
                        .filter(|s| s.name == e)
                        .filter_map(|s| self.idx.get(&s.file_id)),
                );
            } else {
                f.files.extend(hits);
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
        // Weighted edges (src, dst, name, weight), then self-edges.
        let mut edges: Vec<(usize, usize, &str, f64)> = Vec::with_capacity(self.edges.len() + n);
        let mut incoming = vec![false; n];
        for e in &self.edges {
            let (Some(&s), Some(&d)) = (self.idx.get(&e.src), self.idx.get(&e.dst)) else {
                continue;
            };
            #[allow(clippy::cast_precision_loss)]
            let base = if e.name == IMPORT {
                IMPORT_EDGE
            } else {
                (e.n.max(1) as f64).sqrt()
            };
            edges.push((s, d, e.name.as_str(), mul(&e.name, s) * base));
            incoming[d] = true;
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
        // Personalisation (also the dangling distribution) and the start vector.
        #[allow(clippy::cast_precision_loss)]
        let uniform = 1.0 / n as f64;
        let p: Vec<f64> = if focus.files.is_empty() {
            vec![uniform; n]
        } else {
            let share = 1.0 / focus.files.len() as f64;
            (0..n)
                .map(|i| if focus.files.contains(&i) { share } else { 0.0 })
                .collect()
        };
        let warm: f64 = self.files.iter().map(|f| f.rank).sum();
        let mut x: Vec<f64> = if focus.files.is_empty() && warm > 0.0 {
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
}

/// aider's `is_snake or is_camel`: an underscore beside letters, or mixed case.
fn is_snake_or_camel(s: &str) -> bool {
    let snake = s.contains('_') && s.chars().any(char::is_alphabetic);
    let camel = s.chars().any(char::is_uppercase) && s.chars().any(char::is_lowercase);
    snake || camel
}

/// Recompute the unpersonalised ranks and store them in `files.rank` and
/// `symbols.rank` (60 §7). Returns `(files, iterations)`.
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
        SymNode {
            id,
            file_id,
            name: name.to_owned(),
            kind: "function".into(),
            line_start: id,
            signature: format!("fn {name}()"),
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

    /// Four files: `core` is used by everyone, `util` by two, `app` and
    /// `island` by nobody (`island` defines something → self-edge).
    fn toy() -> Graph {
        let files = vec![
            file(1, "src/core.rs"),
            file(2, "src/util.rs"),
            file(3, "src/app.rs"),
            file(4, "src/island.rs"),
        ];
        let symbols = vec![
            sym(1, 1, "open_database"),
            sym(2, 2, "helper"),
            sym(3, 3, "main"),
            sym(4, 4, "alone"),
        ];
        let edges = vec![
            edge(2, 1, "open_database", 3),
            edge(3, 1, "open_database", 1),
            edge(4, 1, "open_database", 1),
            edge(3, 2, "helper", 2),
            edge(4, 2, "helper", 1),
            edge(3, 1, IMPORT, 1),
        ];
        let idx = files.iter().enumerate().map(|(i, f)| (f.id, i)).collect();
        Graph {
            files,
            idx,
            symbols,
            edges,
        }
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
        let f = g.focus(&[
            "src/util.rs".to_owned(),
            "helper".to_owned(),
            "src/".to_owned(),
        ]);
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
        let r = g.rank(&g.focus(&["helper".to_owned()]));
        assert!(
            helper(&r) > 1.5 * helper(&base),
            "{} vs {}",
            helper(&r),
            helper(&base)
        );
        // File focus on `app.rs`: it takes the teleport mass (top file) and its ×50 out-edges lift
        // `helper` (which it references twice) — `open_database` keeps first place on its own
        // long-name ×10 plus the mass `util` and `island` send it.
        let r = g.rank(&g.focus(&["src/app.rs".to_owned()]));
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
        let r = g.rank(&g.focus(&["src/island.rs".to_owned()]));
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
