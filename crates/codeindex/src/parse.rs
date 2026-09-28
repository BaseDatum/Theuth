//! One file → tags → `symbols` and `refs` rows (60 §5.2), plus the import
//! statements (`lang::imports`). Parents and enclosing definitions are
//! computed by byte range after the file's tags are collected; tags from an
//! erroring tree are kept.

use std::cmp::Reverse;
use std::collections::HashMap;

use tree_sitter_tags::{TagsConfiguration, TagsContext};

use crate::lang::Language;
use crate::lang::imports::{self, Import};

/// Signature cap (60 §4).
const SIGNATURE_CHARS: usize = 200;
/// Bytes checked for NUL (60 §5.1).
const NUL_CHECK: usize = 8 * 1024;
/// Go predeclared type names: a `reference.type` to one of these is dropped
/// (there is no definition to resolve to; `none` rows would be noise).
const GO_BUILTIN_TYPES: &[&str] = &[
    "any",
    "bool",
    "byte",
    "comparable",
    "complex64",
    "complex128",
    "error",
    "float32",
    "float64",
    "int",
    "int8",
    "int16",
    "int32",
    "int64",
    "rune",
    "string",
    "uint",
    "uint8",
    "uint16",
    "uint32",
    "uint64",
    "uintptr",
];

/// The parsers one worker owns: the tags runner (which holds its own
/// `Parser` and exposes no tree) and a second `Parser` for the import query.
pub struct Parsers {
    /// tree-sitter-tags context.
    pub tags: TagsContext,
    /// For [`imports::imports`].
    pub tree: tree_sitter::Parser,
}

impl Default for Parsers {
    fn default() -> Self {
        Self {
            tags: TagsContext::new(),
            tree: tree_sitter::Parser::new(),
        }
    }
}

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
    /// The receiver/path text before the name when it is a plain identifier
    /// chain (`a::b` in `a::b::c()`, `obj` in `obj.f()`); `None` otherwise.
    pub qualifier: Option<String>,
}

/// Parse output for one file.
#[derive(Debug, Default)]
pub struct ParsedFile<'c> {
    /// Definitions, parents before children.
    pub symbols: Vec<Symbol<'c>>,
    /// References in source order.
    pub refs: Vec<Ref<'c>>,
    /// Import statements in source order.
    pub imports: Vec<Import>,
    /// Line count.
    pub lines: i64,
    /// tree-sitter `has_errors`.
    pub has_errors: bool,
}

/// Parse `src` with `cfg`. `None`: binary (NUL in the first 8 KiB) or not
/// UTF-8 — the file is skipped (60 §5.1).
pub fn parse<'c>(
    p: &mut Parsers,
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
    let imports = match imports::query(lang) {
        Some(q) if p.tree.set_language(&cfg.language).is_ok() => p
            .tree
            .parse(src, None)
            .map_or_else(Vec::new, |t| imports::imports(lang, q, t.root_node(), src)),
        _ => Vec::new(),
    };
    let (tags, has_errors) = match p.tags.generate_tags(cfg, src, None) {
        Ok(t) => t,
        Err(e) => {
            tracing::debug!(error = %e, "tags failed");
            return Some(ParsedFile {
                lines,
                imports,
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
    if lang == Language::Rust {
        attach_impl_methods(&mut symbols, &refs, cfg, src);
    }
    // Second sweep for references: innermost enclosing definition by range.
    let mut out_refs: Vec<Ref<'c>> = Vec::with_capacity(refs.len());
    let mut di = 0;
    open.clear();
    for r in &refs {
        let p = r.name_range.start;
        while di < defs.len() && defs[di].range.start <= p {
            while open
                .last()
                .is_some_and(|&(_, end)| end <= defs[di].range.start)
            {
                open.pop();
            }
            open.push((di, defs[di].range.end));
            di += 1;
        }
        while open.last().is_some_and(|&(_, end)| end <= p) {
            open.pop();
        }
        let name = text(r.name_range.clone());
        let mut kind = cfg.syntax_type_name(r.syntax_type_id);
        if lang == Language::Go && kind == "type" && GO_BUILTIN_TYPES.contains(&name.as_str()) {
            continue;
        }
        // Rust `name!(..)`: the shipped query tags macro invocations as
        // `reference.call`; a macro can only resolve to a macro (60 §21).
        if lang == Language::Rust && kind == "call" && src.get(r.name_range.end) == Some(&b'!') {
            kind = "macro";
        }
        out_refs.push(Ref {
            qualifier: qualifier(&src[r.range.start..p]),
            name,
            kind,
            line: r.span.start.row as i64 + 1,
            from: open.last().map(|&(i, _)| i),
        });
    }
    Some(ParsedFile {
        symbols,
        refs: out_refs,
        imports,
        lines,
        has_errors,
    })
}

/// Rust `impl` blocks are not definition tags, so methods have no parent
/// (60 §19.3 deviation 10). The `reference.implementation` tags span the
/// whole block and name the type (`impl Foo {`) or the trait (`impl Tr for
/// Foo {`, the type follows ` for `): a method inside such a block gets the
/// same-file `class` symbol of that type as its parent, so `code_outline`
/// groups it and 60 §6.2 rule 2 can pick `Foo::new` among many `new`.
fn attach_impl_methods(
    symbols: &mut [Symbol<'_>],
    refs: &[tree_sitter_tags::Tag],
    cfg: &TagsConfiguration,
    src: &[u8],
) {
    let text = |r: std::ops::Range<usize>| String::from_utf8_lossy(&src[r]).into_owned();
    let impls: Vec<(std::ops::Range<usize>, String)> = refs
        .iter()
        .filter(|r| cfg.syntax_type_name(r.syntax_type_id) == "implementation")
        .map(|r| {
            let line = text(r.line_range.clone());
            let owner = line
                .split_once(" for ")
                .map(|(_, t)| {
                    t.trim_start()
                        .chars()
                        .take_while(|c| c.is_alphanumeric() || *c == '_')
                        .collect::<String>()
                })
                .filter(|t| !t.is_empty())
                .unwrap_or_else(|| text(r.name_range.clone()));
            (r.range.clone(), owner)
        })
        .collect();
    if impls.is_empty() {
        return;
    }
    let types: Vec<(String, usize)> = symbols
        .iter()
        .enumerate()
        .filter(|(_, s)| s.kind == "class")
        .map(|(i, s)| (s.name.clone(), i))
        .collect();
    // The type must come first in source order: `store::apply` inserts in
    // 500-row batches and `parent_id` is checked at each statement's end.
    for (mi, s) in symbols
        .iter_mut()
        .enumerate()
        .filter(|(_, s)| s.kind == "method" && s.parent.is_none())
    {
        let at = s.byte_start as usize;
        let owner = impls
            .iter()
            .filter(|(r, _)| r.start <= at && at < r.end)
            .min_by_key(|(r, _)| r.end - r.start)
            .map(|(_, o)| o.as_str());
        if let Some(o) = owner
            && let Some(&(_, i)) = types.iter().find(|(n, ti)| n == o && *ti < mi)
        {
            s.parent = Some(i);
        }
    }
}

/// The text between a reference tag's start and its name, when it is an
/// identifier chain ending in `::` or `.`: `a::b::` → `a::b`, `self.` →
/// `self`, `::tools::` → `::tools` (global path, kept for the resolver).
/// Generics and casts in a `::` prefix (`Vec::<T>::`) → `None`; a `.`
/// prefix that is not a plain chain (`f().`, `a.b().`) → `"."`: still a
/// receiver call, one the resolver must not take for a bare name.
fn qualifier(prefix: &[u8]) -> Option<String> {
    let (chain, sep): (&[u8], &[u8]) = if let Some(c) = prefix.strip_suffix(b"::") {
        (c, b"::")
    } else {
        (prefix.strip_suffix(b".")?, b".")
    };
    let is_ident = |s: &[u8]| {
        s.first()
            .is_some_and(|&c| c.is_ascii_alphabetic() || c == b'_')
            && s.iter().all(|&c| c.is_ascii_alphanumeric() || c == b'_')
    };
    let chain = std::str::from_utf8(chain).ok()?;
    let sep = std::str::from_utf8(sep).ok()?;
    let body = if sep == "::" {
        chain.strip_prefix("::").unwrap_or(chain)
    } else {
        chain
    };
    if !body.is_empty() && body.split(sep).all(|s| is_ident(s.as_bytes())) {
        Some(chain.to_owned())
    } else {
        (sep == ".").then(|| RECEIVER.to_owned())
    }
}

/// Qualifier of a `.`-call whose receiver is not a plain identifier chain.
pub const RECEIVER: &str = ".";

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

/// One moniker per symbol, `<language>:<path>#<Qualified.Name>(<kind>)`
/// (60 §8): the qualified name is the parent chain joined by `.`, so it
/// survives line moves; a same-name-same-kind sibling in the same file gets
/// `#2`, `#3`, … by source order.
pub fn monikers(language: &str, path: &str, symbols: &[Symbol<'_>]) -> Vec<String> {
    fn qualified(symbols: &[Symbol<'_>], i: usize, depth: usize, out: &mut String) {
        // ponytail: depth cap guards a parent cycle from `attach_impl_methods`
        // (type defined inside its own method body); never seen, cheap to bound.
        if let Some(p) = symbols[i].parent
            && depth < 32
        {
            qualified(symbols, p, depth + 1, out);
            out.push('.');
        }
        out.push_str(&symbols[i].name);
    }
    let mut out = Vec::with_capacity(symbols.len());
    for i in 0..symbols.len() {
        let mut m = format!("{language}:{path}#");
        qualified(symbols, i, 0, &mut m);
        m.push('(');
        m.push_str(symbols[i].kind);
        m.push(')');
        out.push(m);
    }
    let mut seen: HashMap<&str, usize> = HashMap::with_capacity(out.len());
    let ordinal: Vec<usize> = out
        .iter()
        .map(|m| {
            let n = seen.entry(m.as_str()).or_insert(0);
            *n += 1;
            *n
        })
        .collect();
    for (m, n) in out.iter_mut().zip(ordinal) {
        if n > 1 {
            m.push('#');
            m.push_str(&n.to_string());
        }
    }
    out
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

#[cfg(test)]
mod tests {
    use super::{Symbol, monikers, qualifier};

    #[test]
    fn monikers_follow_the_parent_chain_and_number_siblings() {
        let sym = |name: &str, kind: &'static str, parent: Option<usize>| Symbol {
            name: name.into(),
            kind,
            parent,
            line_start: 1,
            line_end: 1,
            byte_start: 0,
            byte_end: 0,
            signature: String::new(),
            docs: None,
            exported: false,
        };
        // `impl` methods may point at a type declared later in the file (parent index > child).
        let symbols = [
            sym("render", "method", Some(2)),
            sym("render", "method", Some(2)),
            sym("Live", "class", None),
            sym("inner", "module", None),
            sym("render", "function", Some(3)),
            sym("render", "function", None),
        ];
        assert_eq!(
            monikers("typescript", "src/a.ts", &symbols),
            [
                "typescript:src/a.ts#Live.render(method)",
                "typescript:src/a.ts#Live.render(method)#2",
                "typescript:src/a.ts#Live(class)",
                "typescript:src/a.ts#inner(module)",
                "typescript:src/a.ts#inner.render(function)",
                "typescript:src/a.ts#render(function)",
            ]
        );
    }

    #[test]
    fn qualifier_accepts_plain_chains_only() {
        assert_eq!(qualifier(b"a::b::").as_deref(), Some("a::b"));
        assert_eq!(qualifier(b"::tools::").as_deref(), Some("::tools"));
        assert_eq!(qualifier(b"self.items.").as_deref(), Some("self.items"));
        assert_eq!(qualifier(b"pkg.").as_deref(), Some("pkg"));
        assert_eq!(qualifier(b""), None);
        assert_eq!(qualifier(b"impl Foo for "), None);
        assert_eq!(qualifier(b"Vec::<T>::"), None);
        assert_eq!(qualifier(b"f(x).").as_deref(), Some("."));
        assert_eq!(qualifier(b"a.b().").as_deref(), Some("."));
        assert_eq!(qualifier(b"f(x)::"), None);
    }
}
