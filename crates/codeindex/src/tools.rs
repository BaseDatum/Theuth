//! `code_find`, `code_outline`, `code_refs`, `code_map`, `code_symbol`,
//! `code_impact` (60 §8). Risk `ReadOnly`, `DIRECT = false`,
//! `CONSUMES_SECRETS = false`: arguments are names, paths, ranges and budgets.

use std::collections::{HashMap, HashSet};
use std::path::PathBuf;
use std::sync::Arc;

use kameo::actor::ActorRef;
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use tools::registry::ToolRegistry;
use tools::{RegisterPrompt, RegisterTool, Risk, Tool, ToolCx, ToolOutput};

use crate::git::{self, Change, Git, Span};
use crate::index::{Codeindex, Reconciled, RootIndex};
use crate::lang::Language;
use crate::parse::{Parsers, parse};
use crate::rank::{self, Graph, Tag};
use crate::store::{self, Definition, OutlineRow, RefHit};
use crate::{root, walk};

/// Plugin prompt block (60 §8): the six tools.
pub const PROMPT: &str = "code_map, code_find, code_outline, code_refs, code_symbol and code_impact are runtime functions over a \
tree-sitter index of a source tree (default root: the git repo of the cwd; first call indexes). Use them before \
grep/read_file when the question is structural: where something is defined (code_find(name=\"X\") → rows of {path, \
line_start, kind, signature}), what a file contains (code_outline(path=\"src/x.rs\") → imports and nested symbols), who \
references a symbol (code_refs(symbol=\"X\") → refs with provenance/confidence saying how sure; `ambiguous` counts the \
same-name references that could not be attributed; several definitions → `candidates`, pick one by symbol_id or \
path:line), a ranked map of the tree to brief a subagent (code_map(focus=[\"X\", \"src/y.rs\"], budget=1500) → the \
most-referenced definitions grouped by file, personalised toward focus identifiers and paths), everything about one \
definition in one call (code_symbol(symbol=\"X\") → definition, body, callers, callees, co-changed files), or what a \
diff touches (code_impact(git_range=\"main..HEAD\") or files=[...] → changed definitions, the files that reference or \
import them, transitive importers, tests). Results are values with a `content` text render; put m[\"content\"] into a \
brief instead of re-reading files.";

/// Default output budget in estimated tokens (60 §0.9).
const DEFAULT_BUDGET: u32 = 4000;
/// Rows fetched before the budget cut.
const FETCH_LIMIT: i64 = 1000;

/// Register the tools and the prompt block.
pub async fn register(reg: &ActorRef<ToolRegistry>, ix: Arc<Codeindex>) {
    let _ = reg
        .ask(RegisterPrompt {
            plugin: "codeindex",
            text: PROMPT.to_owned(),
        })
        .await;
    let _ = reg
        .ask(RegisterTool(Arc::new(CodeFindTool(Arc::clone(&ix)))))
        .await;
    let _ = reg
        .ask(RegisterTool(Arc::new(CodeOutlineTool(Arc::clone(&ix)))))
        .await;
    let _ = reg
        .ask(RegisterTool(Arc::new(CodeRefsTool(Arc::clone(&ix)))))
        .await;
    let _ = reg
        .ask(RegisterTool(Arc::new(CodeMapTool(Arc::clone(&ix)))))
        .await;
    let _ = reg
        .ask(RegisterTool(Arc::new(CodeSymbolTool(Arc::clone(&ix)))))
        .await;
    let _ = reg.ask(RegisterTool(Arc::new(CodeImpactTool(ix)))).await;
}

fn structured<T: Serialize>(text: String, data: T) -> ToolOutput {
    ToolOutput {
        structured: serde_json::to_value(data).ok(),
        ..ToolOutput::ok(text)
    }
}

/// Freshness fields every envelope carries (60 §8).
#[derive(Serialize)]
struct Envelope {
    root: PathBuf,
    indexed_files: i64,
    stale: bool,
    pending: usize,
    dropped: usize,
    truncated_walk: bool,
    suspect_root: bool,
    hint: Option<&'static str>,
}

impl Envelope {
    fn new(ri: &RootIndex, r: &Reconciled, dropped: usize) -> Self {
        Self {
            root: ri.root.clone(),
            indexed_files: r.indexed_files,
            stale: r.stale,
            pending: r.pending,
            dropped,
            truncated_walk: r.truncated_walk,
            suspect_root: r.suspect_root,
            hint: r.hint,
        }
    }
}

/// Trailer of `content` when the answer is not fresh: the background count,
/// or the reconcile guard's hint.
fn stale_note(r: &Reconciled) -> String {
    match (r.stale, r.hint) {
        (true, Some(h)) => format!("\n[stale: {h}]"),
        (true, None) => format!("\n[stale: {} files still indexing]", r.pending),
        _ => String::new(),
    }
}

/// Estimated tokens for code: `chars / 3` (60 §2.B).
pub(crate) fn estimate_tokens(chars: usize) -> u32 {
    u32::try_from(chars / 3).unwrap_or(u32::MAX)
}

/// Keep rows in priority order until `budget` estimated tokens are used; one
/// `… +N more` line for the rest. Returns (text, kept count, dropped count).
fn budget_render(lines: &[String], budget: u32) -> (String, usize, usize) {
    let mut out = String::new();
    let mut kept = 0;
    for l in lines {
        if kept > 0 && estimate_tokens(out.len() + l.len() + 1) > budget {
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
        out.push_str(&format!("\n… +{dropped} more"));
    }
    (out, kept, dropped)
}

/// Open the root and reconcile: the shared front half of every call.
async fn open(
    ix: &Arc<Codeindex>,
    root_arg: Option<&std::path::Path>,
) -> anyhow::Result<(Arc<RootIndex>, Reconciled)> {
    let root = root::discover(root_arg)?;
    let ri = ix.open_root(&root).await?;
    let r = ix.reconcile(&ri, false).await?;
    Ok((ri, r))
}

/// Arguments for [`CodeFindTool`].
#[derive(Debug, Serialize, Deserialize, JsonSchema)]
pub struct CodeFindArgs {
    /// Symbol name, or a prefix of it (and of signature/docs words) unless `exact`.
    pub name: String,
    /// Restrict to one kind: function, method, class, interface, module, macro, type, constant.
    #[serde(default)]
    pub kind: Option<String>,
    /// Exact name match only (default false: prefix search).
    #[serde(default)]
    pub exact: Option<bool>,
    /// Repository root; default: the git top-level of the process cwd, else the cwd.
    #[serde(default)]
    pub root: Option<PathBuf>,
    /// Output budget in estimated tokens (chars/3); rows beyond it are dropped and counted. Default 4000.
    #[serde(default)]
    pub budget: Option<u32>,
}

/// `code_find`: where is `name` defined.
pub struct CodeFindTool(pub Arc<Codeindex>);

#[async_trait::async_trait]
impl Tool for CodeFindTool {
    const NAME: &'static str = "code_find";
    const RETURNS: &'static str = "dict";
    const RISK: Risk = Risk::ReadOnly;
    const DIRECT: bool = false;
    const CONSUMES_SECRETS: bool = false;
    type Args = CodeFindArgs;

    fn description(&self) -> String {
        "Find where a symbol is defined in a source tree (tree-sitter index; reconciled on every call). Returns {rows: [{symbol_id, name, kind, path, line_start, line_end, signature, exported, rank}], root, indexed_files, stale, pending, dropped, content}. Exact-name hits first. Prefix search over name, signature and docs unless exact: true; kind narrows (function, method, class, interface, module, macro, type, constant).".into()
    }

    async fn call(&self, a: CodeFindArgs, _: ToolCx) -> anyhow::Result<ToolOutput> {
        let (ri, r) = match open(&self.0, a.root.as_deref()).await {
            Ok(v) => v,
            Err(e) => return Ok(ToolOutput::err(format!("{e:#}"))),
        };
        let name = a.name.trim();
        if name.is_empty() {
            return Ok(ToolOutput::err("name is empty"));
        }
        let rows = store::find(
            &ri.pool,
            name,
            a.kind.as_deref().map(str::trim).filter(|k| !k.is_empty()),
            a.exact.unwrap_or(false),
            FETCH_LIMIT,
        )
        .await?;
        let lines: Vec<String> = rows
            .iter()
            .map(|s| {
                format!(
                    "{}:{} {} {}  {}",
                    s.path, s.line_start, s.kind, s.name, s.signature
                )
            })
            .collect();
        let (mut text, kept, dropped) = budget_render(&lines, a.budget.unwrap_or(DEFAULT_BUDGET));
        if rows.is_empty() {
            text = format!("no definition named {name:?}");
        }
        text.push_str(&stale_note(&r));
        let env = Envelope::new(&ri, &r, dropped);
        Ok(structured(
            text.clone(),
            serde_json::json!({
                "root": env.root, "indexed_files": env.indexed_files, "stale": env.stale,
                "pending": env.pending, "dropped": env.dropped, "truncated_walk": env.truncated_walk,
                "suspect_root": env.suspect_root, "hint": env.hint,
                "rows": &rows[..kept],
                "content": text,
            }),
        ))
    }
}

/// Arguments for [`CodeOutlineTool`].
#[derive(Debug, Serialize, Deserialize, JsonSchema)]
pub struct CodeOutlineArgs {
    /// File path, relative to the root (or absolute inside it).
    pub path: PathBuf,
    /// Repository root; default: the git top-level of the process cwd, else the cwd.
    #[serde(default)]
    pub root: Option<PathBuf>,
    /// Output budget in estimated tokens (chars/3); rows beyond it are dropped and counted. Default 4000.
    #[serde(default)]
    pub budget: Option<u32>,
}

/// `code_outline`: what a file contains, nested by enclosing definition.
pub struct CodeOutlineTool(pub Arc<Codeindex>);

/// One outline node (60 §8).
#[derive(Serialize)]
struct Node {
    name: String,
    kind: String,
    line_start: i64,
    line_end: i64,
    signature: String,
    exported: bool,
    children: Vec<Node>,
}

/// Rows (source order, parents first) → tree, dropping rows past `kept`.
fn nest(rows: &[OutlineRow], kept: usize) -> Vec<Node> {
    fn build(rows: &[OutlineRow], parent: Option<i64>, kept: usize) -> Vec<Node> {
        rows.iter()
            .take(kept)
            .filter(|r| r.parent_id == parent)
            .map(|r| Node {
                name: r.name.clone(),
                kind: r.kind.clone(),
                line_start: r.line_start,
                line_end: r.line_end,
                signature: r.signature.clone(),
                exported: r.exported,
                children: build(rows, Some(r.id), kept),
            })
            .collect()
    }
    // ponytail: O(n²) over a file's symbols; fine at hundreds, index by parent if a file has thousands.
    build(rows, None, kept)
}

#[async_trait::async_trait]
impl Tool for CodeOutlineTool {
    const NAME: &'static str = "code_outline";
    const RETURNS: &'static str = "dict";
    const RISK: Risk = Risk::ReadOnly;
    const DIRECT: bool = false;
    const CONSUMES_SECRETS: bool = false;
    type Args = CodeOutlineArgs;

    fn description(&self) -> String {
        "Outline one source file from the tree-sitter index: {path, language, lines, parse_errors, imports: [{specifier, target, names, line, provenance}], symbols: [{name, kind, line_start, line_end, signature, exported, children}], root, indexed_files, stale, pending, dropped, content}. Symbols nest by enclosing definition (methods under their class/impl block owner); imports carry the resolved target path and how it was resolved (cargo|mod|tsconfig|package|relative|gomod|external|unresolved).".into()
    }

    async fn call(&self, a: CodeOutlineArgs, _: ToolCx) -> anyhow::Result<ToolOutput> {
        let (ri, r) = match open(&self.0, a.root.as_deref()).await {
            Ok(v) => v,
            Err(e) => return Ok(ToolOutput::err(format!("{e:#}"))),
        };
        let rel = match root::under_root(&ri.root, &a.path) {
            Ok(p) => root::rel_str(&p),
            Err(e) => return Ok(ToolOutput::err(format!("{e:#}"))),
        };
        let Some(file) = store::file(&ri.pool, &rel).await? else {
            let hint = if r.stale {
                format!("; {} files still indexing, call again", r.pending)
            } else {
                String::new()
            };
            return Ok(ToolOutput::err(format!(
                "{rel} is not indexed (ignored, over max_file_bytes, or binary{hint})"
            )));
        };
        let rows = store::outline(&ri.pool, file.id).await?;
        let imports = store::file_imports(&ri.pool, file.id).await?;
        // Depth-first source order: children follow their parent.
        let mut depth = std::collections::HashMap::with_capacity(rows.len());
        let lines: Vec<String> = rows
            .iter()
            .map(|s| {
                let d = s
                    .parent_id
                    .and_then(|p| depth.get(&p).copied())
                    .map_or(0, |d: usize| d + 1);
                depth.insert(s.id, d);
                format!(
                    "{}{}-{} {} {}  {}",
                    "  ".repeat(d),
                    s.line_start,
                    s.line_end,
                    s.kind,
                    s.name,
                    s.signature
                )
            })
            .collect();
        let header = format!(
            "{} ({}, {} lines{}{}, {} imports)",
            file.path,
            file.language,
            file.lines,
            if file.parse_errors {
                ", parse errors"
            } else {
                ""
            },
            if walk::is_test_path(&file.path) {
                ", test"
            } else {
                ""
            },
            imports.len(),
        );
        let budget = a
            .budget
            .unwrap_or(DEFAULT_BUDGET)
            .saturating_sub(estimate_tokens(header.len()));
        let (body, kept, dropped) = budget_render(&lines, budget);
        let mut text = header;
        if !body.is_empty() {
            text.push('\n');
            text.push_str(&body);
        }
        text.push_str(&stale_note(&r));
        let env = Envelope::new(&ri, &r, dropped);
        Ok(structured(
            text.clone(),
            serde_json::json!({
                "root": env.root, "indexed_files": env.indexed_files, "stale": env.stale,
                "pending": env.pending, "dropped": env.dropped, "truncated_walk": env.truncated_walk,
                "suspect_root": env.suspect_root, "hint": env.hint,
                "path": file.path, "language": file.language, "lines": file.lines,
                "parse_errors": file.parse_errors,
                "imports": imports.iter().map(|i| serde_json::json!({
                    "specifier": i.specifier, "target": i.target, "line": i.line, "provenance": i.provenance,
                    "names": i.names.as_deref().and_then(|n| serde_json::from_str::<serde_json::Value>(n).ok()),
                })).collect::<Vec<_>>(),
                "symbols": nest(&rows, kept),
                "content": text,
            }),
        ))
    }
}

/// Arguments for [`CodeRefsTool`].
#[derive(Debug, Serialize, Deserialize, JsonSchema)]
pub struct CodeRefsArgs {
    /// Symbol: a name (`ToolRegistry`), a definition site (`path/to/file.rs:123`), or a numeric symbol_id from code_find.
    pub symbol: String,
    /// `in` (default): who references this definition. `out`: what this definition references.
    #[serde(default)]
    pub direction: Option<String>,
    /// Restrict to one reference kind: call, macro, type, class, implementation.
    #[serde(default)]
    pub kind: Option<String>,
    /// Repository root; default: the git top-level of the process cwd, else the cwd.
    #[serde(default)]
    pub root: Option<PathBuf>,
    /// Output budget in estimated tokens (chars/3); rows beyond it are dropped and counted. Default 4000.
    #[serde(default)]
    pub budget: Option<u32>,
}

/// `code_refs`: who references a symbol, with provenance and confidence.
pub struct CodeRefsTool(pub Arc<Codeindex>);

/// What `symbol` names: one definition, several (the caller picks), or none.
enum Picked {
    One(Definition),
    Several(Vec<Definition>),
    None,
}

/// Numeric → `symbols.id`; `path:line` → innermost definition there; else exact name.
async fn pick(
    pool: &sqlx::SqlitePool,
    root: &std::path::Path,
    symbol: &str,
) -> anyhow::Result<Picked> {
    if let Ok(id) = symbol.parse::<i64>() {
        return Ok(store::definition_by_id(pool, id)
            .await?
            .map_or(Picked::None, Picked::One));
    }
    if let Some((path, line)) = symbol.rsplit_once(':')
        && let Ok(line) = line.parse::<i64>()
    {
        let rel = root::under_root(root, std::path::Path::new(path)).map(|p| root::rel_str(&p))?;
        return Ok(store::definition_at(pool, &rel, line)
            .await?
            .map_or(Picked::None, Picked::One));
    }
    let mut defs = store::definitions_named(pool, symbol).await?;
    Ok(match defs.len() {
        0 => Picked::None,
        1 => Picked::One(defs.remove(0)),
        _ => Picked::Several(defs),
    })
}

#[async_trait::async_trait]
impl Tool for CodeRefsTool {
    const NAME: &'static str = "code_refs";
    const RETURNS: &'static str = "dict";
    const RISK: Risk = Risk::ReadOnly;
    const DIRECT: bool = false;
    const CONSUMES_SECRETS: bool = false;
    type Args = CodeRefsArgs;

    fn description(&self) -> String {
        "Who references a symbol (direction=in, default) or what a definition references (direction=out), from the tree-sitter index. Returns {definition: {symbol_id, name, kind, path, line_start, line_end, signature}, refs: [{path, line, name, from_symbol, kind, provenance, confidence, target}], ambiguous: n, by_file: [{path, count}], root, indexed_files, stale, pending, dropped, content}. provenance same_file|import = 1.0, package = 0.8, unique = 0.6; `ambiguous` = same-name references that could not be attributed to one definition (method calls on receivers mostly). A bare name matching several definitions returns candidates: [{symbol_id, path, line, kind}] and no refs: call again with the symbol_id or path:line.".into()
    }

    async fn call(&self, a: CodeRefsArgs, _: ToolCx) -> anyhow::Result<ToolOutput> {
        let (ri, r) = match open(&self.0, a.root.as_deref()).await {
            Ok(v) => v,
            Err(e) => return Ok(ToolOutput::err(format!("{e:#}"))),
        };
        let symbol = a.symbol.trim();
        if symbol.is_empty() {
            return Ok(ToolOutput::err("symbol is empty"));
        }
        let outward = match a.direction.as_deref().map(str::trim).unwrap_or("in") {
            "in" | "" => false,
            "out" => true,
            d => {
                return Ok(ToolOutput::err(format!(
                    "direction must be in or out, not {d:?}"
                )));
            }
        };
        let kind = a.kind.as_deref().map(str::trim).filter(|k| !k.is_empty());
        let budget = a.budget.unwrap_or(DEFAULT_BUDGET);
        let env = |dropped: usize| Envelope::new(&ri, &r, dropped);
        let def = match pick(&ri.pool, &ri.root, symbol).await {
            Ok(Picked::One(d)) => d,
            Ok(Picked::None) => {
                let hint = if r.stale {
                    format!("; {} files still indexing", r.pending)
                } else {
                    String::new()
                };
                return Ok(ToolOutput::err(format!(
                    "no definition named {symbol:?}{hint}"
                )));
            }
            Ok(Picked::Several(defs)) => {
                let ambiguous = store::ambiguous_named(&ri.pool, symbol, kind).await?;
                let lines: Vec<String> = defs
                    .iter()
                    .map(|d| {
                        format!(
                            "{}:{} {} {}  {}",
                            d.path, d.line_start, d.kind, d.name, d.signature
                        )
                    })
                    .collect();
                let (body, kept, dropped) = budget_render(&lines, budget);
                let text = format!(
                    "{symbol:?} has {} definitions; pass a symbol_id or path:line to pick one ({ambiguous} same-name references are ambiguous)\n{body}",
                    defs.len()
                );
                let e = env(dropped);
                return Ok(structured(
                    text.clone(),
                    serde_json::json!({
                        "root": e.root, "indexed_files": e.indexed_files, "stale": e.stale,
                        "pending": e.pending, "dropped": e.dropped, "truncated_walk": e.truncated_walk,
                        "suspect_root": e.suspect_root, "hint": e.hint,
                        "candidates": defs[..kept].iter().map(|d| serde_json::json!({
                            "symbol_id": d.symbol_id, "path": d.path, "line": d.line_start, "kind": d.kind, "signature": d.signature,
                        })).collect::<Vec<_>>(),
                        "refs": [], "ambiguous": ambiguous, "by_file": [],
                        "content": text,
                    }),
                ));
            }
            Err(e) => return Ok(ToolOutput::err(format!("{e:#}"))),
        };
        let refs = if outward {
            store::refs_from(&ri.pool, def.symbol_id, kind, FETCH_LIMIT).await?
        } else {
            store::refs_to(&ri.pool, def.symbol_id, kind, FETCH_LIMIT).await?
        };
        let ambiguous = if outward {
            0
        } else {
            store::ambiguous_named(&ri.pool, &def.name, kind).await?
        };
        let mut by_file: Vec<(String, usize)> = Vec::new();
        for h in &refs {
            match by_file.iter_mut().find(|(p, _)| *p == h.path) {
                Some((_, n)) => *n += 1,
                None => by_file.push((h.path.clone(), 1)),
            }
        }
        by_file.sort_by(|a, b| b.1.cmp(&a.1).then_with(|| a.0.cmp(&b.0)));
        let header = format!(
            "{} {} {}:{}-{}  {}\n{} {} reference{}{}",
            def.kind,
            def.name,
            def.path,
            def.line_start,
            def.line_end,
            def.signature,
            refs.len(),
            if outward { "outgoing" } else { "incoming" },
            if refs.len() == 1 { "" } else { "s" },
            if ambiguous > 0 {
                format!(", {ambiguous} ambiguous same-name references not attributed")
            } else {
                String::new()
            },
        );
        let lines: Vec<String> = refs
            .iter()
            .map(|h| {
                let scope = h
                    .from_symbol
                    .as_deref()
                    .map_or(String::new(), |s| format!(" in {s}"));
                let target = h
                    .target
                    .as_deref()
                    .map_or(String::new(), |t| format!(" → {t}"));
                format!(
                    "{}:{} {} {}{}{} [{} {:.1}]",
                    h.path, h.line, h.kind, h.name, target, scope, h.provenance, h.confidence
                )
            })
            .collect();
        let (body, kept, dropped) =
            budget_render(&lines, budget.saturating_sub(estimate_tokens(header.len())));
        let mut text = header;
        if !body.is_empty() {
            text.push('\n');
            text.push_str(&body);
        }
        text.push_str(&stale_note(&r));
        let e = env(dropped);
        Ok(structured(
            text.clone(),
            serde_json::json!({
                "root": e.root, "indexed_files": e.indexed_files, "stale": e.stale,
                "pending": e.pending, "dropped": e.dropped, "truncated_walk": e.truncated_walk,
                "suspect_root": e.suspect_root, "hint": e.hint,
                "definition": def,
                "direction": if outward { "out" } else { "in" },
                "refs": &refs[..kept],
                "ambiguous": ambiguous,
                "by_file": by_file.iter().map(|(p, n)| serde_json::json!({"path": p, "count": n})).collect::<Vec<_>>(),
                "content": text,
            }),
        ))
    }
}

/// Arguments for [`CodeMapTool`].
#[derive(Debug, Serialize, Deserialize, JsonSchema)]
pub struct CodeMapArgs {
    /// Identifiers and/or root-relative paths (files or directories) to personalise the map toward.
    #[serde(default)]
    pub focus: Option<Vec<String>>,
    /// Ranked files only ({path, rank, lines, language}), no definitions.
    #[serde(default)]
    pub paths_only: Option<bool>,
    /// Repository root; default: the git top-level of the process cwd, else the cwd.
    #[serde(default)]
    pub root: Option<PathBuf>,
    /// Output budget in estimated tokens (chars/3); rows beyond it are dropped and counted. Default 4000.
    #[serde(default)]
    pub budget: Option<u32>,
}

/// `code_map`: the ranked map of a tree (60 §7), optionally personalised.
pub struct CodeMapTool(pub Arc<Codeindex>);

#[async_trait::async_trait]
impl Tool for CodeMapTool {
    const NAME: &'static str = "code_map";
    const RETURNS: &'static str = "dict";
    const RISK: Risk = Risk::ReadOnly;
    const DIRECT: bool = false;
    const CONSUMES_SECRETS: bool = false;
    type Args = CodeMapArgs;

    fn description(&self) -> String {
        "Ranked map of a source tree (aider-style PageRank over the file graph: files referenced from many places rank high). Returns {rows: [{path, line, kind, name, signature, rank}], root, indexed_files, stale, pending, dropped, content}; content is the map as text (path, then indented `line: signature` per definition) fitted to budget — put it in a subagent brief. focus=[identifiers or paths] personalises toward them (their definers and importers rise); paths_only=true returns ranked files {path, rank, lines, language} instead.".into()
    }

    async fn call(&self, a: CodeMapArgs, _: ToolCx) -> anyhow::Result<ToolOutput> {
        let (ri, r) = match open(&self.0, a.root.as_deref()).await {
            Ok(v) => v,
            Err(e) => return Ok(ToolOutput::err(format!("{e:#}"))),
        };
        let budget = a.budget.unwrap_or(DEFAULT_BUDGET);
        let focus: Vec<String> = a
            .focus
            .unwrap_or_default()
            .into_iter()
            .filter(|s| !s.trim().is_empty())
            .collect();
        let paths_only = a.paths_only.unwrap_or(false);
        // Without focus the stored ranks are the answer (60 §7); a focus is a fresh personalised run.
        let ranked = if focus.is_empty() && !paths_only {
            None
        } else {
            let g = Graph::load(&ri.pool).await?;
            let f = g.focus(&focus);
            let rk = if focus.is_empty() {
                None
            } else {
                Some(g.rank(&f))
            };
            Some((g, f, rk))
        };
        let mut extra = serde_json::Map::new();
        if let Some((g, f, rk)) = &ranked {
            extra.insert(
                "focus_files".into(),
                f.files
                    .iter()
                    .map(|&i| g.files[i].path.clone())
                    .collect::<Vec<_>>()
                    .into(),
            );
            extra.insert(
                "focus_idents".into(),
                f.idents.iter().cloned().collect::<Vec<_>>().into(),
            );
            if let Some(rk) = rk {
                extra.insert("pagerank_iterations".into(), rk.iterations.into());
            }
        }
        let (mut text, rows, dropped) = if paths_only {
            let (g, _, rk) = ranked.as_ref().expect("loaded for paths_only");
            let mut files: Vec<(&rank::FileNode, f64)> = match rk {
                Some(rk) => g.files.iter().zip(rk.files.iter().copied()).collect(),
                None => g.files.iter().map(|f| (f, f.rank)).collect(),
            };
            files.sort_by(|a, b| b.1.total_cmp(&a.1).then_with(|| a.0.path.cmp(&b.0.path)));
            let lines: Vec<String> = files
                .iter()
                .map(|(f, r)| format!("{}  {r:.5}  {} lines  {}", f.path, f.lines, f.language))
                .collect();
            let (text, kept, dropped) = budget_render(&lines, budget);
            let rows: Vec<serde_json::Value> = files[..kept]
                .iter()
                .map(|(f, r)| serde_json::json!({"path": f.path, "rank": r, "lines": f.lines, "language": f.language}))
                .collect();
            (text, rows, dropped)
        } else {
            let tags: Vec<Tag> = match &ranked {
                Some((_, _, Some(rk))) => rk.tags.iter().map(|(_, t)| t.clone()).collect(),
                _ => store::ranked_tags(&ri.pool).await?,
            };
            let (text, kept) = rank::render(&tags, budget);
            let rows = tags[..kept]
                .iter()
                .map(|t| serde_json::to_value(t).unwrap_or_default())
                .collect();
            (text, rows, tags.len() - kept)
        };
        if text.is_empty() {
            text =
                "no ranked definitions: the index has no resolved cross-file references yet".into();
        }
        if r.stale {
            text.push_str(&format!("\n[stale: {} files still indexing]", r.pending));
        }
        let e = Envelope::new(&ri, &r, dropped);
        let mut v = serde_json::json!({
            "root": e.root, "indexed_files": e.indexed_files, "stale": e.stale,
            "pending": e.pending, "dropped": e.dropped, "truncated_walk": e.truncated_walk,
            "estimated_tokens": estimate_tokens(text.len()),
            "rows": rows,
            "content": text,
        });
        if let Some(m) = v.as_object_mut() {
            m.extend(extra);
        }
        Ok(structured(text, v))
    }
}

/// Arguments for [`CodeSymbolTool`].
#[derive(Debug, Serialize, Deserialize, JsonSchema)]
pub struct CodeSymbolArgs {
    /// Symbol: a name (`open_db`), a definition site (`path/to/file.rs:123`), or a numeric symbol_id from code_find.
    pub symbol: String,
    /// Sections to return; default all five: definition, body, callers, callees, cochange.
    #[serde(default)]
    pub sections: Option<Vec<String>>,
    /// Repository root; default: the git top-level of the process cwd, else the cwd.
    #[serde(default)]
    pub root: Option<PathBuf>,
    /// Output budget in estimated tokens (chars/3), split equally over the sections. Default 4000.
    #[serde(default)]
    pub budget: Option<u32>,
}

/// `code_symbol`: everything about one definition in one call (60 §8).
pub struct CodeSymbolTool(pub Arc<Codeindex>);

/// Section names, in output order.
const SECTIONS: [&str; 5] = ["definition", "body", "callers", "callees", "cochange"];
/// Co-change partners shown (60 §8).
const COCHANGE_TOP: i64 = 10;
/// Body lines shown whole; longer bodies show head/tail around a cut marker.
const BODY_MAX: usize = 200;
const BODY_HEAD: usize = 120;
const BODY_TAIL: usize = 60;

/// Source lines `line_start..=line_end` of `path` under `root`, cut to
/// head + marker + tail past [`BODY_MAX`].
fn body_lines(root: &std::path::Path, d: &Definition) -> anyhow::Result<Vec<String>> {
    let src = std::fs::read_to_string(root.join(&d.path))?;
    #[allow(clippy::cast_sign_loss)]
    let (s, e) = (
        (d.line_start.max(1) - 1) as usize,
        d.line_end.max(d.line_start) as usize,
    );
    let lines: Vec<String> = src
        .lines()
        .skip(s)
        .take(e.saturating_sub(s))
        .map(str::to_owned)
        .collect();
    if lines.len() <= BODY_MAX {
        return Ok(lines);
    }
    let cut = lines.len() - BODY_HEAD - BODY_TAIL;
    let mut out = Vec::with_capacity(BODY_HEAD + BODY_TAIL + 1);
    out.extend_from_slice(&lines[..BODY_HEAD]);
    out.push(format!("… {cut} lines cut …"));
    out.extend_from_slice(&lines[lines.len() - BODY_TAIL..]);
    Ok(out)
}

/// `[{path, count, refs: [...]}]` for the callers section, most references first.
fn group_by_file(refs: &[RefHit]) -> Vec<serde_json::Value> {
    let mut groups: Vec<(&str, Vec<&RefHit>)> = Vec::new();
    for h in refs {
        match groups.iter_mut().find(|(p, _)| *p == h.path) {
            Some((_, v)) => v.push(h),
            None => groups.push((h.path.as_str(), vec![h])),
        }
    }
    groups.sort_by(|a, b| b.1.len().cmp(&a.1.len()).then_with(|| a.0.cmp(b.0)));
    groups
        .iter()
        .map(|(p, v)| {
            serde_json::json!({
                "path": p, "count": v.len(),
                "refs": v.iter().map(|h| serde_json::json!({
                    "line": h.line, "from_symbol": h.from_symbol, "kind": h.kind, "provenance": h.provenance, "confidence": h.confidence,
                })).collect::<Vec<_>>(),
            })
        })
        .collect()
}

/// `[{name, target, count, provenance, confidence}]` for the callees section, most calls first.
fn group_by_target(refs: &[RefHit]) -> Vec<(String, String, usize, String, f64)> {
    let mut groups: Vec<(String, String, usize, String, f64)> = Vec::new();
    for h in refs {
        let Some(t) = h.target.as_deref() else {
            continue;
        };
        match groups
            .iter_mut()
            .find(|(n, tg, ..)| *n == h.name && tg == t)
        {
            Some(g) => {
                g.2 += 1;
                if h.confidence > g.4 {
                    g.3 = h.provenance.clone();
                    g.4 = h.confidence;
                }
            }
            None => groups.push((
                h.name.clone(),
                t.to_owned(),
                1,
                h.provenance.clone(),
                h.confidence,
            )),
        }
    }
    groups.sort_by(|a, b| b.2.cmp(&a.2).then_with(|| a.1.cmp(&b.1)));
    groups
}

#[async_trait::async_trait]
impl Tool for CodeSymbolTool {
    const NAME: &'static str = "code_symbol";
    const RETURNS: &'static str = "dict";
    const RISK: Risk = Risk::ReadOnly;
    const DIRECT: bool = false;
    const CONSUMES_SECRETS: bool = false;
    type Args = CodeSymbolArgs;

    fn description(&self) -> String {
        "Everything about one definition in one call: {definition: {symbol_id, name, kind, path, line_start, line_end, signature}, body: str (source, ≤ 200 lines else head/tail with a cut marker), callers: [{path, count, refs: [{line, from_symbol, kind, provenance, confidence}]}], ambiguous: n, callees: [{name, target, count, provenance, confidence}], cochange: [{path, count}] (files committed together with the definition's file, last 300 first-parent commits), tests: [path] (tests whose subject file is the definition's file), root, indexed_files, stale, pending, dropped, content}. sections=[definition|body|callers|callees|cochange] selects (default all); the budget is split equally over them. A bare name with several definitions returns candidates and no sections: call again with the symbol_id or path:line.".into()
    }

    async fn call(&self, a: CodeSymbolArgs, _: ToolCx) -> anyhow::Result<ToolOutput> {
        let (ri, r) = match open(&self.0, a.root.as_deref()).await {
            Ok(v) => v,
            Err(e) => return Ok(ToolOutput::err(format!("{e:#}"))),
        };
        let symbol = a.symbol.trim();
        if symbol.is_empty() {
            return Ok(ToolOutput::err("symbol is empty"));
        }
        let wanted: Vec<&str> = match &a.sections {
            None => SECTIONS.to_vec(),
            Some(v) => {
                let mut w = Vec::new();
                for s in v {
                    let s = s.trim();
                    let Some(&known) = SECTIONS.iter().find(|k| **k == s) else {
                        return Ok(ToolOutput::err(format!(
                            "unknown section {s:?}; one of {}",
                            SECTIONS.join(", ")
                        )));
                    };
                    if !w.contains(&known) {
                        w.push(known);
                    }
                }
                if w.is_empty() {
                    return Ok(ToolOutput::err("sections is empty"));
                }
                w
            }
        };
        let budget = a.budget.unwrap_or(DEFAULT_BUDGET);
        let env = |dropped: usize| Envelope::new(&ri, &r, dropped);
        let def = match pick(&ri.pool, &ri.root, symbol).await {
            Ok(Picked::One(d)) => d,
            Ok(Picked::None) => {
                let hint = if r.stale {
                    format!("; {} files still indexing", r.pending)
                } else {
                    String::new()
                };
                return Ok(ToolOutput::err(format!(
                    "no definition named {symbol:?}{hint}"
                )));
            }
            Ok(Picked::Several(defs)) => {
                let lines: Vec<String> = defs
                    .iter()
                    .map(|d| {
                        format!(
                            "{}:{} {} {}  {}",
                            d.path, d.line_start, d.kind, d.name, d.signature
                        )
                    })
                    .collect();
                let (body, kept, dropped) = budget_render(&lines, budget);
                let text = format!(
                    "{symbol:?} has {} definitions; pass a symbol_id or path:line to pick one\n{body}",
                    defs.len()
                );
                let e = env(dropped);
                return Ok(structured(
                    text.clone(),
                    serde_json::json!({
                        "root": e.root, "indexed_files": e.indexed_files, "stale": e.stale,
                        "pending": e.pending, "dropped": e.dropped, "truncated_walk": e.truncated_walk,
                        "suspect_root": e.suspect_root, "hint": e.hint,
                        "candidates": defs[..kept].iter().map(|d| serde_json::json!({
                            "symbol_id": d.symbol_id, "path": d.path, "line": d.line_start, "kind": d.kind, "signature": d.signature,
                        })).collect::<Vec<_>>(),
                        "content": text,
                    }),
                ));
            }
            Err(e) => return Ok(ToolOutput::err(format!("{e:#}"))),
        };
        #[allow(clippy::cast_possible_truncation)]
        let per = budget / wanted.len() as u32;
        let mut out = serde_json::Map::new();
        let mut text = String::new();
        let mut dropped = 0;
        for &s in &wanted {
            let block = match s {
                "definition" => {
                    out.insert("definition".into(), serde_json::to_value(&def)?);
                    format!(
                        "{} {} {}:{}-{}  {}",
                        def.kind, def.name, def.path, def.line_start, def.line_end, def.signature
                    )
                }
                "body" => match body_lines(&ri.root, &def) {
                    Ok(lines) => {
                        let (b, _, d) = budget_render(&lines, per);
                        dropped += d;
                        out.insert("body".into(), b.clone().into());
                        format!(
                            "-- body {}:{}-{} --\n{b}",
                            def.path, def.line_start, def.line_end
                        )
                    }
                    Err(e) => {
                        out.insert("body".into(), serde_json::Value::Null);
                        format!("-- body: unreadable ({e}) --")
                    }
                },
                "callers" => {
                    let refs = store::refs_to(&ri.pool, def.symbol_id, None, FETCH_LIMIT).await?;
                    let ambiguous = store::ambiguous_named(&ri.pool, &def.name, None).await?;
                    let mut grouped = group_by_file(&refs);
                    let lines: Vec<String> = refs
                        .iter()
                        .map(|h| {
                            let scope = h
                                .from_symbol
                                .as_deref()
                                .map_or(String::new(), |s| format!(" in {s}"));
                            format!(
                                "{}:{} {}{} [{} {:.1}]",
                                h.path, h.line, h.kind, scope, h.provenance, h.confidence
                            )
                        })
                        .collect();
                    let (b, kept, d) = budget_render(&lines, per);
                    dropped += d;
                    if kept < refs.len() {
                        grouped = group_by_file(&refs[..kept]);
                    }
                    out.insert("callers".into(), grouped.into());
                    out.insert("ambiguous".into(), ambiguous.into());
                    let note = if ambiguous > 0 {
                        format!(", {ambiguous} ambiguous same-name references not attributed")
                    } else {
                        String::new()
                    };
                    format!(
                        "-- callers: {} reference{}{note} --\n{b}",
                        refs.len(),
                        if refs.len() == 1 { "" } else { "s" }
                    )
                }
                "callees" => {
                    let refs =
                        store::refs_in_span(&ri.pool, &def.path, def.line_start, def.line_end)
                            .await?;
                    let groups = group_by_target(&refs);
                    let lines: Vec<String> = groups
                        .iter()
                        .map(|(n, t, c, p, cf)| {
                            let times = if *c > 1 {
                                format!(" ×{c}")
                            } else {
                                String::new()
                            };
                            format!("{n} → {t}{times} [{p} {cf:.1}]")
                        })
                        .collect();
                    let (b, kept, d) = budget_render(&lines, per);
                    dropped += d;
                    out.insert(
                        "callees".into(),
                        groups[..kept]
                            .iter()
                            .map(|(n, t, c, p, cf)| serde_json::json!({"name": n, "target": t, "count": c, "provenance": p, "confidence": cf}))
                            .collect::<Vec<_>>()
                            .into(),
                    );
                    format!(
                        "-- callees: {} resolved target{} --\n{b}",
                        groups.len(),
                        if groups.len() == 1 { "" } else { "s" }
                    )
                }
                _ => {
                    let file = store::file(&ri.pool, &def.path).await?;
                    let partners = match file {
                        Some(f) => store::cochange_partners(&ri.pool, f.id, COCHANGE_TOP).await?,
                        None => Vec::new(),
                    };
                    let lines: Vec<String> =
                        partners.iter().map(|(p, n)| format!("{p} ×{n}")).collect();
                    let (b, kept, d) = budget_render(&lines, per);
                    dropped += d;
                    out.insert(
                        "cochange".into(),
                        partners[..kept]
                            .iter()
                            .map(|(p, n)| serde_json::json!({"path": p, "count": n}))
                            .collect::<Vec<_>>()
                            .into(),
                    );
                    if partners.is_empty() {
                        format!(
                            "-- cochange: no commits changed {} together with another indexed file --",
                            def.path
                        )
                    } else {
                        format!(
                            "-- cochange: files committed together with {} (top {}) --\n{b}",
                            def.path,
                            partners.len()
                        )
                    }
                }
            };
            if !text.is_empty() {
                text.push('\n');
            }
            text.push_str(block.trim_end());
        }
        // 60 §8 `test_subject`: the tests whose subject is the definition's file.
        let tests: Vec<String> = store::test_subjects(&ri.pool)
            .await?
            .into_iter()
            .filter(|(_, s)| *s == def.path)
            .map(|(t, _)| t)
            .collect();
        if !tests.is_empty() {
            text.push_str(&format!(
                "\n-- tests: {} file{} (subject {}) --\n{}",
                tests.len(),
                if tests.len() == 1 { "" } else { "s" },
                def.path,
                tests.join("\n")
            ));
        }
        text.push_str(&stale_note(&r));
        let e = env(dropped);
        let mut v = serde_json::json!({
            "root": e.root, "indexed_files": e.indexed_files, "stale": e.stale,
            "pending": e.pending, "dropped": e.dropped, "truncated_walk": e.truncated_walk,
            "suspect_root": e.suspect_root, "hint": e.hint,
            "sections": wanted,
            "tests": tests,
            "content": text,
        });
        if let Some(m) = v.as_object_mut() {
            m.extend(out);
        }
        Ok(structured(text, v))
    }
}

/// Arguments for [`CodeImpactTool`].
#[derive(Debug, Serialize, Deserialize, JsonSchema)]
pub struct CodeImpactArgs {
    /// Changed files (root-relative or absolute under the root); every definition in them counts as changed. Exactly one of files / git_range.
    #[serde(default)]
    pub files: Option<Vec<PathBuf>>,
    /// Revision range: `A..B`, `A...B` (merge-base..B), a single revision `R` (= `R^..R`: `HEAD~1`, `main`, a sha), or `WORKTREE` (HEAD against the working tree). Hunks are mapped to definitions by line.
    #[serde(default)]
    pub git_range: Option<String>,
    /// Keep test files in `direct`/`transitive` (default false; they always appear in `tests`).
    #[serde(default)]
    pub include_tests: Option<bool>,
    /// Repository root; default: the git top-level of the process cwd, else the cwd.
    #[serde(default)]
    pub root: Option<PathBuf>,
    /// Output budget in estimated tokens (chars/3) for `content`; rows beyond it are dropped and counted. Default 4000.
    #[serde(default)]
    pub budget: Option<u32>,
}

/// `code_impact`: what a change touches (60 §8) — changed definitions, their
/// referencers, transitive importers, tests.
pub struct CodeImpactTool(pub Arc<Codeindex>);

/// Importer BFS depth for `transitive`.
const TRANSITIVE_DEPTH: usize = 3;

/// A definition as `code_impact` maps hunks onto it: from the index
/// (`id` set) or from a parse of the range's new-side blob (`id` unset —
/// the index describes the working tree, whose lines may have moved).
struct Sym {
    id: Option<i64>,
    name: String,
    kind: String,
    line_start: i64,
    line_end: i64,
}

/// The definitions `spans` touch: any intersecting a [`Span::Lines`]/
/// [`Span::All`], the innermost containing a [`Span::Point`]. Source order,
/// no duplicates.
fn touched<'a>(rows: &'a [Sym], spans: &[Span]) -> Vec<&'a Sym> {
    let mut out: Vec<(usize, &Sym)> = Vec::new();
    for sp in spans {
        match sp {
            Span::Point(_) => {
                if let Some(r) = rows
                    .iter()
                    .enumerate()
                    .filter(|(_, r)| sp.touches(r.line_start, r.line_end))
                    .min_by_key(|(_, r)| r.line_end - r.line_start)
                {
                    out.push(r);
                }
            }
            _ => out.extend(
                rows.iter()
                    .enumerate()
                    .filter(|(_, r)| sp.touches(r.line_start, r.line_end)),
            ),
        }
    }
    out.sort_by_key(|(i, _)| *i);
    out.dedup_by_key(|(i, _)| *i);
    out.into_iter().map(|(_, r)| r).collect()
}

/// Parse each change's new-side bytes with the file's language; `None` when
/// there are none or the language has no tags.
fn parse_new_sides(changes: &[Change]) -> Vec<Option<Vec<Sym>>> {
    let mut parsers = Parsers::default();
    changes
        .iter()
        .map(|c| {
            let src = c.new.as_deref()?;
            let lang = Language::from_ext(std::path::Path::new(&c.path).extension()?.to_str()?);
            let p = parse(&mut parsers, lang.tags()?, lang, src)?;
            Some(
                p.symbols
                    .iter()
                    .map(|s| Sym {
                        id: None,
                        name: s.name.clone(),
                        kind: s.kind.to_owned(),
                        line_start: s.line_start,
                        line_end: s.line_end,
                    })
                    .collect(),
            )
        })
        .collect()
}

/// `ids` plus every Go file sharing a package with a Go file in it (a Go
/// import targets the package's first file only, 60 §20 deviation 5).
fn widen_go(ids: &HashSet<i64>, files: &[store::FileRef]) -> HashSet<i64> {
    let pkgs: HashSet<&str> = files
        .iter()
        .filter(|f| ids.contains(&f.id) && f.language == "go")
        .filter_map(|f| f.package.as_deref())
        .collect();
    let mut out = ids.clone();
    if !pkgs.is_empty() {
        out.extend(
            files
                .iter()
                .filter(|f| {
                    f.language == "go" && f.package.as_deref().is_some_and(|p| pkgs.contains(p))
                })
                .map(|f| f.id),
        );
    }
    out
}

#[async_trait::async_trait]
impl Tool for CodeImpactTool {
    const NAME: &'static str = "code_impact";
    const RETURNS: &'static str = "dict";
    const RISK: Risk = Risk::ReadOnly;
    const DIRECT: bool = false;
    const CONSUMES_SECRETS: bool = false;
    type Args = CodeImpactArgs;

    fn description(&self) -> String {
        "What a change touches, from the tree-sitter index and gix: give files=[paths] (every definition in them counts as changed) or git_range (\"main..HEAD\", \"HEAD~1\", \"WORKTREE\" = uncommitted changes; hunks are mapped to definitions by line). Returns {changed: [{path, status, symbols_changed: [{name, kind, line}]}], unindexed: [path], direct: [{path, via: [name], is_test}] (files whose resolved references or imports point into the changed definitions/files), transitive: [{path, depth}] (importers of direct files, depth ≤ 3), tests: [path] (test files among changed ∪ direct ∪ transitive that reference the change), tests_by_subject: [path] (tests whose subject file — by naming convention or most-imported file — is changed or direct; content lists each test once, marked (ref)/(subject)), ambiguous_skipped: n (same-name references not counted), unresolved_imports: n, root, indexed_files, stale, pending, dropped, content}. include_tests=true keeps test files in direct/transitive.".into()
    }

    async fn call(&self, a: CodeImpactArgs, _: ToolCx) -> anyhow::Result<ToolOutput> {
        let (ri, r) = match open(&self.0, a.root.as_deref()).await {
            Ok(v) => v,
            Err(e) => return Ok(ToolOutput::err(format!("{e:#}"))),
        };
        let range = a
            .git_range
            .as_deref()
            .map(str::trim)
            .filter(|s| !s.is_empty());
        let (mode, changes, range_ids): (&str, Vec<Change>, Option<(Option<String>, String)>) =
            match (&a.files, range) {
                (Some(_), Some(_)) => {
                    return Ok(ToolOutput::err("pass files or git_range, not both"));
                }
                (None, None) => {
                    return Ok(ToolOutput::err(
                        "pass files=[...] or git_range=\"A..B\" | \"HEAD~1\" | \"WORKTREE\"",
                    ));
                }
                (Some(files), None) => {
                    let mut out = Vec::with_capacity(files.len());
                    for p in files {
                        match root::under_root(&ri.root, p) {
                            Ok(rel) => out.push(Change {
                                path: root::rel_str(&rel),
                                status: git::Status::Modified,
                                spans: vec![Span::All],
                                new: None,
                            }),
                            Err(e) => return Ok(ToolOutput::err(format!("{e:#}"))),
                        }
                    }
                    ("files", out, None)
                }
                (None, Some(spec)) => {
                    let (root, spec) = (ri.root.clone(), spec.to_owned());
                    let indexed: Vec<String> =
                        store::known_files(&ri.pool).await?.into_keys().collect();
                    let res = tokio::task::spawn_blocking(move || -> anyhow::Result<_> {
                        let Some(g) = Git::open(&root)? else {
                            anyhow::bail!(
                                "{} is not in a git repository; pass files=[...]",
                                root.display()
                            )
                        };
                        let range = g.range(&spec)?;
                        let ids = match &range {
                            git::Range::Worktree => {
                                (Some("HEAD".to_owned()), "WORKTREE".to_owned())
                            }
                            git::Range::Commits { old, new } => {
                                (old.map(|o| o.to_string()), new.to_string())
                            }
                        };
                        Ok((
                            g.changes(&range, &root, indexed.iter().map(String::as_str))?,
                            ids,
                        ))
                    })
                    .await?;
                    match res {
                        Ok((c, ids)) => ("range", c, Some(ids)),
                        Err(e) => return Ok(ToolOutput::err(format!("{e:#}"))),
                    }
                }
            };
        let (changes, new_sides) = tokio::task::spawn_blocking(move || {
            let parsed = parse_new_sides(&changes);
            (changes, parsed)
        })
        .await?;
        let files = store::all_files(&ri.pool).await?;
        let by_path: HashMap<&str, &store::FileRef> =
            files.iter().map(|f| (f.path.as_str(), f)).collect();
        let by_id: HashMap<i64, &store::FileRef> = files.iter().map(|f| (f.id, f)).collect();

        // Changed files → changed definitions.
        let mut changed_rows: Vec<serde_json::Value> = Vec::new();
        let mut changed_lines: Vec<String> = Vec::new();
        let mut unindexed: Vec<&str> = Vec::new();
        let mut changed_files: HashSet<i64> = HashSet::new();
        let mut symbol_ids: Vec<i64> = Vec::new();
        let mut names: Vec<&str> = Vec::new();
        let mut sym_names: Vec<String> = Vec::new();
        for (c, blob_syms) in changes.iter().zip(&new_sides) {
            let Some(f) = by_path.get(c.path.as_str()) else {
                if c.status == git::Status::Deleted {
                    changed_rows.push(serde_json::json!({"path": c.path, "status": c.status, "symbols_changed": []}));
                    changed_lines.push(format!("{} [deleted]", c.path));
                } else {
                    unindexed.push(&c.path);
                }
                continue;
            };
            changed_files.insert(f.id);
            let rows: Vec<Sym> = store::outline(&ri.pool, f.id)
                .await?
                .into_iter()
                .map(|r| Sym {
                    id: Some(r.id),
                    name: r.name,
                    kind: r.kind,
                    line_start: r.line_start,
                    line_end: r.line_end,
                })
                .collect();
            // Spans are lines of the new side; map them on its own parse when we have
            // one, then find the index's rows by (name, kind) in the same file.
            let hit: Vec<&Sym> = touched(blob_syms.as_deref().unwrap_or(&rows), &c.spans);
            for h in &hit {
                match h.id {
                    Some(id) => symbol_ids.push(id),
                    None => symbol_ids.extend(
                        rows.iter()
                            .filter(|r| r.name == h.name && r.kind == h.kind)
                            .filter_map(|r| r.id),
                    ),
                }
            }
            sym_names.extend(hit.iter().map(|s| s.name.clone()));
            let status = serde_json::to_value(c.status)?;
            let list = if hit.is_empty() {
                if rows.is_empty() {
                    " (no symbols)".to_owned()
                } else {
                    " (no definition in the hunks)".to_owned()
                }
            } else {
                format!(
                    ": {}",
                    hit.iter()
                        .map(|s| format!("{} ({}:{})", s.name, s.kind, s.line_start))
                        .collect::<Vec<_>>()
                        .join(", ")
                )
            };
            changed_lines.push(format!(
                "{} [{}]{list}",
                c.path,
                status.as_str().unwrap_or("?")
            ));
            changed_rows.push(serde_json::json!({
                "path": c.path, "status": status,
                "symbols_changed": hit.iter().map(|s| serde_json::json!({"name": s.name, "kind": s.kind, "line": s.line_start})).collect::<Vec<_>>(),
            }));
        }
        symbol_ids.sort_unstable();
        symbol_ids.dedup();
        sym_names.sort_unstable();
        sym_names.dedup();
        names.extend(sym_names.iter().map(String::as_str));

        // Direct: resolved refs into the changed definitions (≥ 0.5), plus importers of the changed
        // files. A changed file referencing *another* changed file's definitions is listed too,
        // flagged `changed` (the coupling inside a diff is what a reviewer reads first).
        let mut direct: HashMap<i64, Vec<String>> = HashMap::new();
        for (file_id, name, target) in store::referencers(&ri.pool, &symbol_ids).await? {
            if file_id != target && by_id.contains_key(&file_id) {
                direct.entry(file_id).or_default().push(name);
            }
        }
        let edges = store::import_edges(&ri.pool).await?;
        let mut importers: HashMap<i64, Vec<i64>> = HashMap::new();
        for &(src, dst) in &edges {
            importers.entry(dst).or_default().push(src);
        }
        let targets = widen_go(&changed_files, &files);
        for &(src, dst) in &edges {
            if targets.contains(&dst) && by_id.contains_key(&src) {
                let via = direct.entry(src).or_default();
                if !via.iter().any(|v| v == IMPORT_VIA) {
                    via.push(IMPORT_VIA.to_owned());
                }
            }
        }
        // Transitive: importers of direct files, depth ≤ 3, minus changed and direct.
        let mut transitive: HashMap<i64, usize> = HashMap::new();
        let mut frontier: HashSet<i64> = direct.keys().copied().collect();
        for depth in 1..=TRANSITIVE_DEPTH {
            let mut next = HashSet::new();
            for t in widen_go(&frontier, &files) {
                for &src in importers.get(&t).map_or(&[][..], Vec::as_slice) {
                    if !changed_files.contains(&src)
                        && !direct.contains_key(&src)
                        && !transitive.contains_key(&src)
                        && by_id.contains_key(&src)
                    {
                        transitive.insert(src, depth);
                        next.insert(src);
                    }
                }
            }
            if next.is_empty() {
                break;
            }
            frontier = next;
        }
        let include_tests = a.include_tests.unwrap_or(false);
        let is_test = |id: &i64| by_id.get(id).is_some_and(|f| f.is_test);
        let mut tests: Vec<&str> = changed_files
            .iter()
            .chain(direct.keys())
            .chain(transitive.keys())
            .filter(|id| is_test(id))
            .map(|id| by_id[id].path.as_str())
            .collect();
        tests.sort_unstable();
        tests.dedup();
        // 60 §8 `test_subject`: tests whose subject is a changed or direct file —
        // API-level tests that never name the changed definition.
        let subjects = store::test_subjects(&ri.pool).await?;
        let impacted: HashSet<&str> = changed_files
            .iter()
            .chain(direct.keys())
            .map(|id| by_id[id].path.as_str())
            .collect();
        let tests_by_subject: Vec<&str> = subjects
            .iter()
            .filter(|(_, s)| impacted.contains(s.as_str()))
            .map(|(t, _)| t.as_str())
            .collect();
        let mut all_tests: Vec<(&str, bool, bool)> = tests
            .iter()
            .map(|t| (*t, true, tests_by_subject.binary_search(t).is_ok()))
            .collect();
        all_tests.extend(
            tests_by_subject
                .iter()
                .filter(|t| tests.binary_search(t).is_err())
                .map(|t| (*t, false, true)),
        );
        all_tests.sort_unstable();
        let mut direct: Vec<(&str, Vec<String>, bool, bool)> = direct
            .into_iter()
            .filter(|(id, _)| include_tests || !is_test(id))
            .map(|(id, mut via)| {
                via.sort_unstable();
                via.dedup();
                (
                    by_id[&id].path.as_str(),
                    via,
                    is_test(&id),
                    changed_files.contains(&id),
                )
            })
            .collect();
        // Files outside the change first, then by how much of it they use.
        direct.sort_by(|a, b| {
            a.3.cmp(&b.3)
                .then_with(|| b.1.len().cmp(&a.1.len()))
                .then_with(|| a.0.cmp(b.0))
        });
        let mut transitive: Vec<(&str, usize)> = transitive
            .into_iter()
            .filter(|(id, _)| include_tests || !is_test(id))
            .map(|(id, d)| (by_id[&id].path.as_str(), d))
            .collect();
        transitive.sort_by(|a, b| a.1.cmp(&b.1).then_with(|| a.0.cmp(b.0)));
        let ambiguous_skipped = store::impact_skipped(&ri.pool, &symbol_ids, &names).await?;
        let changed_ids: Vec<i64> = changed_files.iter().copied().collect();
        let unresolved_imports = store::unresolved_imports_in(&ri.pool, &changed_ids).await?;
        unindexed.sort_unstable();

        let header = format!(
            "{}: {} changed file{}, {} definition{}; direct {}, transitive {}, tests {}; {ambiguous_skipped} ambiguous skipped, {unresolved_imports} unresolved imports{}",
            match &range_ids {
                Some((Some(o), n)) =>
                    format!("{}..{}", &o[..o.len().min(12)], &n[..n.len().min(12)]),
                Some((None, n)) => format!("(root)..{}", &n[..n.len().min(12)]),
                None => "files".to_owned(),
            },
            changed_rows.len(),
            if changed_rows.len() == 1 { "" } else { "s" },
            symbol_ids.len(),
            if symbol_ids.len() == 1 { "" } else { "s" },
            direct.len(),
            transitive.len(),
            all_tests.len(),
            if unindexed.is_empty() {
                String::new()
            } else {
                format!(", {} unindexed", unindexed.len())
            },
        );
        let mut lines: Vec<String> = Vec::new();
        lines.push("-- changed --".into());
        lines.extend(changed_lines);
        if !unindexed.is_empty() {
            lines.push("-- unindexed (ignored or unknown language) --".into());
            lines.extend(unindexed.iter().map(|p| (*p).to_owned()));
        }
        lines.push(format!("-- direct: {} files --", direct.len()));
        lines.extend(direct.iter().map(|(p, via, t, c)| {
            format!(
                "{p}{}{} via {}",
                if *c { " [changed]" } else { "" },
                if *t { " [test]" } else { "" },
                via.join(", ")
            )
        }));
        lines.push(format!("-- transitive: {} files --", transitive.len()));
        lines.extend(transitive.iter().map(|(p, d)| format!("{p} (depth {d})")));
        lines.push(format!("-- tests: {} files --", all_tests.len()));
        lines.extend(all_tests.iter().map(|(p, r, s)| {
            format!(
                "{p} ({})",
                match (r, s) {
                    (true, true) => "ref, subject",
                    (true, false) => "ref",
                    _ => "subject",
                }
            )
        }));
        let (body, _, dropped) = budget_render(
            &lines,
            a.budget
                .unwrap_or(DEFAULT_BUDGET)
                .saturating_sub(estimate_tokens(header.len())),
        );
        let mut text = format!("{header}\n{body}");
        text.push_str(&stale_note(&r));
        let e = Envelope::new(&ri, &r, dropped);
        Ok(structured(
            text.clone(),
            serde_json::json!({
                "root": e.root, "indexed_files": e.indexed_files, "stale": e.stale,
                "pending": e.pending, "dropped": e.dropped, "truncated_walk": e.truncated_walk,
                "suspect_root": e.suspect_root, "hint": e.hint,
                "mode": mode,
                "range": range_ids.map(|(o, n)| serde_json::json!({"old": o, "new": n})),
                "include_tests": include_tests,
                "changed": changed_rows,
                "unindexed": unindexed,
                "direct": direct.iter().map(|(p, via, t, c)| serde_json::json!({"path": p, "via": via, "is_test": t, "changed": c})).collect::<Vec<_>>(),
                "transitive": transitive.iter().map(|(p, d)| serde_json::json!({"path": p, "depth": d})).collect::<Vec<_>>(),
                "tests": tests,
                "tests_by_subject": tests_by_subject,
                "ambiguous_skipped": ambiguous_skipped,
                "unresolved_imports": unresolved_imports,
                "content": text,
            }),
        ))
    }
}

/// `via` entry of a direct file that imports a changed file.
const IMPORT_VIA: &str = "<import>";

const _: () = {
    assert!(!<CodeFindTool as Tool>::DIRECT);
    assert!(!<CodeOutlineTool as Tool>::DIRECT);
    assert!(!<CodeRefsTool as Tool>::DIRECT);
    assert!(!<CodeMapTool as Tool>::DIRECT);
    assert!(!<CodeSymbolTool as Tool>::DIRECT);
    assert!(!<CodeImpactTool as Tool>::DIRECT);
};

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn budget_cuts_at_row_boundaries_and_counts() {
        let lines: Vec<String> = (0..10)
            .map(|i| format!("row {i} xxxxxxxxxxxxxxxxxxxx"))
            .collect();
        let (text, kept, dropped) = budget_render(&lines, 20);
        assert_eq!(kept + dropped, 10);
        assert!(kept >= 1 && dropped > 0, "{kept} {dropped}");
        assert!(text.ends_with(&format!("… +{dropped} more")));
        let (text, kept, dropped) = budget_render(&lines, 100_000);
        assert_eq!((kept, dropped), (10, 0));
        assert!(!text.contains("more"));
    }
}
