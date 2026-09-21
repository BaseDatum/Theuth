//! `code_find`, `code_outline`, `code_refs` (60 §8). Risk `ReadOnly`,
//! `DIRECT = false`, `CONSUMES_SECRETS = false`: arguments are names, paths
//! and budgets.

use std::path::PathBuf;
use std::sync::Arc;

use kameo::actor::ActorRef;
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use tools::registry::ToolRegistry;
use tools::{RegisterPrompt, RegisterTool, Risk, Tool, ToolCx, ToolOutput};

use crate::index::{Codeindex, Reconciled, RootIndex};
use crate::store::{self, Definition, OutlineRow};
use crate::{root, walk};

/// Plugin prompt block (60 §8), the tools of steps 1–2.
pub const PROMPT: &str = "code_find, code_outline and code_refs are runtime functions over a tree-sitter index of a \
source tree (default root: the git repo of the cwd; first call indexes). Use them before grep/read_file when the \
question is structural: where something is defined (code_find(name=\"X\") → rows of {path, line_start, kind, \
signature}), what a file contains (code_outline(path=\"src/x.rs\") → imports and nested symbols), who references a \
symbol (code_refs(symbol=\"X\") → refs with provenance/confidence saying how sure; `ambiguous` counts the same-name \
references that could not be attributed; several definitions → `candidates`, pick one by symbol_id or path:line). \
Results are values with a `content` text render; put m[\"content\"] into a brief instead of re-reading files.";

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
    let _ = reg.ask(RegisterTool(Arc::new(CodeRefsTool(ix)))).await;
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
        }
    }
}

/// Estimated tokens for code: `chars / 3` (60 §2.B).
fn estimate_tokens(chars: usize) -> u32 {
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
            .map(|s| format!("{}:{} {} {}  {}", s.path, s.line_start, s.kind, s.name, s.signature))
            .collect();
        let (mut text, kept, dropped) = budget_render(&lines, a.budget.unwrap_or(DEFAULT_BUDGET));
        if rows.is_empty() {
            text = format!("no definition named {name:?}");
        }
        if r.stale {
            text.push_str(&format!("\n[stale: {} files still indexing]", r.pending));
        }
        let env = Envelope::new(&ri, &r, dropped);
        Ok(structured(
            text.clone(),
            serde_json::json!({
                "root": env.root, "indexed_files": env.indexed_files, "stale": env.stale,
                "pending": env.pending, "dropped": env.dropped, "truncated_walk": env.truncated_walk,
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
                let d = s.parent_id.and_then(|p| depth.get(&p).copied()).map_or(0, |d: usize| d + 1);
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
            if file.parse_errors { ", parse errors" } else { "" },
            if walk::is_test_path(&file.path) { ", test" } else { "" },
            imports.len(),
        );
        let budget = a.budget.unwrap_or(DEFAULT_BUDGET).saturating_sub(estimate_tokens(header.len()));
        let (body, kept, dropped) = budget_render(&lines, budget);
        let mut text = header;
        if !body.is_empty() {
            text.push('\n');
            text.push_str(&body);
        }
        if r.stale {
            text.push_str(&format!("\n[stale: {} files still indexing]", r.pending));
        }
        let env = Envelope::new(&ri, &r, dropped);
        Ok(structured(
            text.clone(),
            serde_json::json!({
                "root": env.root, "indexed_files": env.indexed_files, "stale": env.stale,
                "pending": env.pending, "dropped": env.dropped, "truncated_walk": env.truncated_walk,
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
    /// Restrict to one reference kind: call, type, class, implementation.
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
async fn pick(pool: &sqlx::SqlitePool, root: &std::path::Path, symbol: &str) -> anyhow::Result<Picked> {
    if let Ok(id) = symbol.parse::<i64>() {
        return Ok(store::definition_by_id(pool, id).await?.map_or(Picked::None, Picked::One));
    }
    if let Some((path, line)) = symbol.rsplit_once(':')
        && let Ok(line) = line.parse::<i64>()
    {
        let rel = root::under_root(root, std::path::Path::new(path)).map(|p| root::rel_str(&p))?;
        return Ok(store::definition_at(pool, &rel, line).await?.map_or(Picked::None, Picked::One));
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
            d => return Ok(ToolOutput::err(format!("direction must be in or out, not {d:?}"))),
        };
        let kind = a.kind.as_deref().map(str::trim).filter(|k| !k.is_empty());
        let budget = a.budget.unwrap_or(DEFAULT_BUDGET);
        let env = |dropped: usize| Envelope::new(&ri, &r, dropped);
        let def = match pick(&ri.pool, &ri.root, symbol).await {
            Ok(Picked::One(d)) => d,
            Ok(Picked::None) => {
                let hint = if r.stale { format!("; {} files still indexing", r.pending) } else { String::new() };
                return Ok(ToolOutput::err(format!("no definition named {symbol:?}{hint}")));
            }
            Ok(Picked::Several(defs)) => {
                let ambiguous = store::ambiguous_named(&ri.pool, symbol, kind).await?;
                let lines: Vec<String> = defs
                    .iter()
                    .map(|d| format!("{}:{} {} {}  {}", d.path, d.line_start, d.kind, d.name, d.signature))
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
        let ambiguous = if outward { 0 } else { store::ambiguous_named(&ri.pool, &def.name, kind).await? };
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
            if ambiguous > 0 { format!(", {ambiguous} ambiguous same-name references not attributed") } else { String::new() },
        );
        let lines: Vec<String> = refs
            .iter()
            .map(|h| {
                let scope = h.from_symbol.as_deref().map_or(String::new(), |s| format!(" in {s}"));
                let target = h.target.as_deref().map_or(String::new(), |t| format!(" → {t}"));
                format!("{}:{} {} {}{}{} [{} {:.1}]", h.path, h.line, h.kind, h.name, target, scope, h.provenance, h.confidence)
            })
            .collect();
        let (body, kept, dropped) = budget_render(&lines, budget.saturating_sub(estimate_tokens(header.len())));
        let mut text = header;
        if !body.is_empty() {
            text.push('\n');
            text.push_str(&body);
        }
        if r.stale {
            text.push_str(&format!("\n[stale: {} files still indexing]", r.pending));
        }
        let e = env(dropped);
        Ok(structured(
            text.clone(),
            serde_json::json!({
                "root": e.root, "indexed_files": e.indexed_files, "stale": e.stale,
                "pending": e.pending, "dropped": e.dropped, "truncated_walk": e.truncated_walk,
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

const _: () = {
    assert!(!<CodeFindTool as Tool>::DIRECT);
    assert!(!<CodeOutlineTool as Tool>::DIRECT);
    assert!(!<CodeRefsTool as Tool>::DIRECT);
};

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn budget_cuts_at_row_boundaries_and_counts() {
        let lines: Vec<String> = (0..10).map(|i| format!("row {i} xxxxxxxxxxxxxxxxxxxx")).collect();
        let (text, kept, dropped) = budget_render(&lines, 20);
        assert_eq!(kept + dropped, 10);
        assert!(kept >= 1 && dropped > 0, "{kept} {dropped}");
        assert!(text.ends_with(&format!("… +{dropped} more")));
        let (text, kept, dropped) = budget_render(&lines, 100_000);
        assert_eq!((kept, dropped), (10, 0));
        assert!(!text.contains("more"));
    }
}
