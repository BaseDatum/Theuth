//! One file → tags → `symbols` and `refs` rows (60 §5.2). Parents and
//! enclosing definitions are computed by byte range after the file's tags
//! are collected; tags from an erroring tree are kept.

use std::cmp::Reverse;

use tree_sitter_tags::{TagsConfiguration, TagsContext};

use crate::lang::Language;

/// Signature cap (60 §4).
const SIGNATURE_CHARS: usize = 200;
/// Bytes checked for NUL (60 §5.1).
const NUL_CHECK: usize = 8 * 1024;

/// A definition tag.
#[derive(Clone, Debug)]
pub struct Symbol<'c> {
    /// Name text.
    pub name: String,
    /// Tags syntax type (`function`, `class`, …).
    pub kind: &'c str,
    /// Index into the file's `symbols` of the innermost enclosing definition.
    pub parent: Option<usize>,
    /// 1-based.
    pub line_start: i64,
    /// 1-based, inclusive.
    pub line_end: i64,
    /// Definition node start.
    pub byte_start: i64,
    /// Definition node end.
    pub byte_end: i64,
    /// The name's line, trimmed, ≤ 200 chars.
    pub signature: String,
    /// `@doc` capture text.
    pub docs: Option<String>,
    /// 60 §4 heuristic.
    pub exported: bool,
}

/// A reference tag.
#[derive(Clone, Debug)]
pub struct Ref<'c> {
    /// Referenced name.
    pub name: String,
    /// `call` | `type` | `class` | `implementation`.
    pub kind: &'c str,
    /// 1-based.
    pub line: i64,
    /// Index of the innermost enclosing definition; `None` at module scope.
    pub from: Option<usize>,
}

/// Parse output for one file.
#[derive(Debug, Default)]
pub struct ParsedFile<'c> {
    /// Definitions, parents before children.
    pub symbols: Vec<Symbol<'c>>,
    /// References in source order.
    pub refs: Vec<Ref<'c>>,
    /// Line count.
    pub lines: i64,
    /// tree-sitter `has_errors`.
    pub has_errors: bool,
}

/// Parse `src` with `cfg`. `None`: binary (NUL in the first 8 KiB) or not
/// UTF-8 — the file is skipped (60 §5.1).
pub fn parse<'c>(
    ctx: &mut TagsContext,
    cfg: &'c TagsConfiguration,
    lang: Language,
    src: &[u8],
) -> Option<ParsedFile<'c>> {
    if src[..src.len().min(NUL_CHECK)].contains(&0) || std::str::from_utf8(src).is_err() {
        return None;
    }
    let lines = if src.is_empty() {
        0
    } else {
        src.iter().filter(|&&b| b == b'\n').count() as i64 + i64::from(src[src.len() - 1] != b'\n')
    };
    let (tags, has_errors) = match ctx.generate_tags(cfg, src, None) {
        Ok(t) => t,
        Err(e) => {
            tracing::debug!(error = %e, "tags failed");
            return Some(ParsedFile {
                lines,
                has_errors: true,
                ..Default::default()
            });
        }
    };
    let mut defs = Vec::new();
    let mut refs = Vec::new();
    for tag in tags.flatten() {
        if tag.is_definition {
            defs.push(tag);
        } else {
            refs.push(tag);
        }
    }
    // Parents first: earlier start, and for equal starts the wider range.
    defs.sort_by_key(|t| (t.range.start, Reverse(t.range.end)));
    refs.sort_by_key(|t| t.name_range.start);

    let text = |r: std::ops::Range<usize>| String::from_utf8_lossy(&src[r]).into_owned();
    let line_of = LineIndex::new(src);
    let mut symbols: Vec<Symbol<'c>> = Vec::with_capacity(defs.len());
    // Stack of (index, end byte) of open definitions.
    let mut open: Vec<(usize, usize)> = Vec::new();
    for (i, d) in defs.iter().enumerate() {
        while open.last().is_some_and(|&(_, end)| end <= d.range.start) {
            open.pop();
        }
        let name = text(d.name_range.clone());
        let kind = cfg.syntax_type_name(d.syntax_type_id);
        let signature = clip(text(d.line_range.clone()), SIGNATURE_CHARS);
        let exported = exported(lang, &name, &signature);
        symbols.push(Symbol {
            parent: open.last().map(|&(i, _)| i),
            line_start: d.span.start.row as i64 + 1,
            line_end: line_of.row(d.range.end.saturating_sub(1)) as i64 + 1,
            byte_start: d.range.start as i64,
            byte_end: d.range.end as i64,
            exported,
            signature,
            docs: d.docs.clone(),
            kind,
            name,
        });
        open.push((i, d.range.end));
    }
    // Second sweep for references: innermost enclosing definition by range.
    let mut out_refs: Vec<Ref<'c>> = Vec::with_capacity(refs.len());
    let mut di = 0;
    open.clear();
    for r in &refs {
        let p = r.name_range.start;
        while di < defs.len() && defs[di].range.start <= p {
            while open.last().is_some_and(|&(_, end)| end <= defs[di].range.start) {
                open.pop();
            }
            open.push((di, defs[di].range.end));
            di += 1;
        }
        while open.last().is_some_and(|&(_, end)| end <= p) {
            open.pop();
        }
        out_refs.push(Ref {
            name: text(r.name_range.clone()),
            kind: cfg.syntax_type_name(r.syntax_type_id),
            line: r.span.start.row as i64 + 1,
            from: open.last().map(|&(i, _)| i),
        });
    }
    Some(ParsedFile {
        symbols,
        refs: out_refs,
        lines,
        has_errors,
    })
}

/// 60 §4: rust `pub`, ts/js `export`, go Uppercase, py not `_`-prefixed.
/// `Other` never reaches here.
fn exported(lang: Language, name: &str, signature: &str) -> bool {
    match lang {
        Language::Rust => signature.starts_with("pub"),
        Language::TypeScript | Language::Tsx | Language::JavaScript => {
            signature.starts_with("export")
        }
        Language::Go => name.chars().next().is_some_and(char::is_uppercase),
        Language::Python => !name.starts_with('_'),
        Language::Other => false,
    }
}

fn clip(mut s: String, max_chars: usize) -> String {
    if let Some((i, _)) = s.char_indices().nth(max_chars) {
        s.truncate(i);
        s.push('…');
    }
    s
}

/// Byte offset → 0-based row.
struct LineIndex(Vec<usize>);

impl LineIndex {
    fn new(src: &[u8]) -> Self {
        Self(
            src.iter()
                .enumerate()
                .filter_map(|(i, &b)| (b == b'\n').then_some(i))
                .collect(),
        )
    }
    fn row(&self, byte: usize) -> usize {
        self.0.partition_point(|&nl| nl < byte)
    }
}
