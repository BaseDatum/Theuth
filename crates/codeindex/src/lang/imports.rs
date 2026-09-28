//! Import statements → `imports` rows (60 §5.2): one `tree_sitter::Query` per
//! language that captures the import *statements*; the names are read from the
//! captured node's children here, which is shorter than one query pattern per
//! clause shape. Runs on a second parse of the file: `tree-sitter-tags` owns its
//! tree and exposes no handle to it.

use std::sync::LazyLock;

use tree_sitter::{Node, Query, QueryCursor, StreamingIterator};

use super::Language;

/// What kind of statement the row came from.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Kind {
    /// `use`, `import`, `from … import`, `require`, Go `import`.
    Use,
    /// Rust `mod x;` — `specifier = "mod:x"`, `names = [#[path] value]` when present.
    Mod,
    /// Rust `extern crate x;`.
    Extern,
}

/// One `imports` row before resolution.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Import {
    /// As written, minus the last path segment for Rust `use` (60 §4):
    /// `tools::registry`, `./foo`, `pkg.mod`, `github.com/x/y`, `mod:foo`.
    pub specifier: String,
    /// Names the statement brings into scope (aliases applied); `None` for
    /// globs (`use a::*`, `from a import *`, `export * from`, Go `import .`).
    pub names: Option<Vec<String>>,
    /// 1-based line of the statement.
    pub line: i64,
    /// Statement kind.
    pub kind: Kind,
}

const RUST: &str = "\
(use_declaration argument: (_) @arg) @stmt
(mod_item name: (identifier) @name !body) @stmt
(extern_crate_declaration name: (identifier) @name alias: (identifier)? @alias) @stmt
";

const JS: &str = "\
(import_statement source: (string (string_fragment) @src)) @stmt
(export_statement source: (string (string_fragment) @src)) @stmt
(call_expression function: (identifier) @fn arguments: (arguments . (string (string_fragment) @src))) @stmt
(call_expression function: (import) arguments: (arguments . (string (string_fragment) @src))) @stmt
";

const PY: &str = "\
(import_statement) @stmt
(import_from_statement) @stmt
";

const GO: &str = "\
(import_spec name: (_)? @alias path: (interpreted_string_literal) @src) @stmt
";

/// The compiled query for `lang`; `None` for `Other` or a query that does not
/// compile against the grammar (warned once).
pub fn query(lang: Language) -> Option<&'static Query> {
    macro_rules! lazy {
        ($l:expr, $q:expr) => {{
            static Q: LazyLock<Option<Query>> = LazyLock::new(|| {
                Query::new(&$l.into(), $q)
            .inspect_err(|e| tracing::warn!(error = %e, "import query failed to compile"))
                    .ok()
            });
            Q.as_ref()
        }};
    }
    match lang {
        Language::Rust => lazy!(tree_sitter_rust::LANGUAGE, RUST),
        Language::TypeScript => lazy!(tree_sitter_typescript::LANGUAGE_TYPESCRIPT, JS),
        Language::Tsx => lazy!(tree_sitter_typescript::LANGUAGE_TSX, JS),
        Language::JavaScript => lazy!(tree_sitter_javascript::LANGUAGE, JS),
        Language::Python => lazy!(tree_sitter_python::LANGUAGE, PY),
        Language::Go => lazy!(tree_sitter_go::LANGUAGE, GO),
        Language::Other => None,
    }
}

/// Every import of `tree` (rooted at `root`), in source order.
pub fn imports(lang: Language, query: &Query, root: Node<'_>, src: &[u8]) -> Vec<Import> {
    let text = |n: Node<'_>| String::from_utf8_lossy(&src[n.byte_range()]).into_owned();
    let idx = |name: &str| query.capture_index_for_name(name);
    let (stmt_i, src_i, name_i, alias_i, arg_i, fn_i) = (
        idx("stmt"),
        idx("src"),
        idx("name"),
        idx("alias"),
        idx("arg"),
        idx("fn"),
    );
    let mut out = Vec::new();
    let mut cursor = QueryCursor::new();
    let mut matches = cursor.matches(query, root, src);
    while let Some(m) = matches.next() {
        let cap = |i: Option<u32>| i.and_then(|i| m.nodes_for_capture_index(i).next());
        let Some(stmt) = cap(stmt_i) else { continue };
        let line = stmt.start_position().row as i64 + 1;
        match lang {
            Language::Rust => match stmt.kind() {
                "use_declaration" => {
                    let Some(arg) = cap(arg_i) else { continue };
                    let mut leaves = Vec::new();
                    rust_flatten(arg, Vec::new(), src, &mut leaves);
                    // Inside `mod tests { … }` (an inline module, depth k) the
                    // first k `super`s climb back to the file's own module.
                    let depth = inline_mod_depth(stmt);
                    for (mut segs, alias, glob) in leaves {
                        let mut climbed = 0;
                        while climbed < depth && segs.first().is_some_and(|s| s == "super") {
                            segs.remove(0);
                            climbed += 1;
                        }
                        if climbed > 0 && segs.first().is_none_or(|s| s != "super" && s != "self") {
                            segs.insert(0, "self".into());
                        }
                        if glob {
                            out.push(Import {
                                specifier: segs.join("::"),
                                names: None,
                                line,
                                kind: Kind::Use,
                            });
                            continue;
                        }
                        if segs.len() > 1 && segs.last().is_some_and(|s| s == "self") {
                            segs.pop();
                        }
                        let Some(last) = segs.pop() else { continue };
                        let specifier = if segs.is_empty() {
                            last.clone()
                        } else {
                            segs.join("::")
                        };
                        out.push(Import {
                            specifier,
                            names: Some(vec![alias.unwrap_or(last)]),
                            line,
                            kind: Kind::Use,
                        });
                    }
                }
                "mod_item" => {
                    let Some(name) = cap(name_i) else { continue };
                    out.push(Import {
                        specifier: format!("mod:{}", text(name)),
                        names: rust_path_attr(stmt, src).map(|p| vec![p]),
                        line,
                        kind: Kind::Mod,
                    });
                }
                _ => {
                    let Some(name) = cap(name_i) else { continue };
                    let name = text(name);
                    out.push(Import {
                        names: Some(vec![cap(alias_i).map_or_else(|| name.clone(), text)]),
                        specifier: name,
                        line,
                        kind: Kind::Extern,
                    });
                }
            },
            Language::TypeScript | Language::Tsx | Language::JavaScript => {
                let Some(source) = cap(src_i) else { continue };
                if let Some(f) = cap(fn_i)
                    && &src[f.byte_range()] != b"require"
                {
                    continue;
                }
                out.push(Import {
                    specifier: text(source),
                    names: js_names(stmt, src),
                    line,
                    kind: Kind::Use,
                });
            }
            Language::Python => py_imports(stmt, src, line, &mut out),
            Language::Go => {
                let Some(source) = cap(src_i) else { continue };
                let specifier = text(source).trim_matches('"').to_owned();
                let names = match cap(alias_i).map(text).as_deref() {
                    Some(".") => None,
                    Some("_") => Some(Vec::new()),
                    Some(a) => Some(vec![a.to_owned()]),
                    None => Some(vec![
                        specifier
                            .rsplit('/')
                            .next()
                            .unwrap_or(&specifier)
                            .to_owned(),
                    ]),
                };
                out.push(Import {
                    specifier,
                    names,
                    line,
                    kind: Kind::Use,
                });
            }
            Language::Other => {}
        }
    }
    out
}

/// Leaves of a `use` tree: (segments, alias, is_glob).
fn rust_flatten(
    node: Node<'_>,
    prefix: Vec<String>,
    src: &[u8],
    out: &mut Vec<(Vec<String>, Option<String>, bool)>,
) {
    let text = |n: Node<'_>| String::from_utf8_lossy(&src[n.byte_range()]).into_owned();
    let with = |p: &[String], more: Vec<String>| {
        let mut v = p.to_vec();
        v.extend(more);
        v
    };
    match node.kind() {
        "scoped_identifier" | "identifier" | "crate" | "super" | "self" | "metavariable" => {
            out.push((with(&prefix, rust_segments(node, src)), None, false));
        }
        "use_as_clause" => {
            let Some(path) = node.child_by_field_name("path") else {
                return;
            };
            let alias = node.child_by_field_name("alias").map(text);
            out.push((with(&prefix, rust_segments(path, src)), alias, false));
        }
        "use_list" => {
            let mut c = node.walk();
            for child in node.named_children(&mut c) {
                if child.kind() != "line_comment" && child.kind() != "block_comment" {
                    rust_flatten(child, prefix.clone(), src, out);
                }
            }
        }
        "scoped_use_list" => {
            let p = node
                .child_by_field_name("path")
                .map_or_else(Vec::new, |p| rust_segments(p, src));
            if let Some(list) = node.child_by_field_name("list") {
                rust_flatten(list, with(&prefix, p), src, out);
            }
        }
        "use_wildcard" => {
            // The grammar gives `use_wildcard` no `path` field: its one named child is the path.
            let p = node
                .named_child(0)
                .map_or_else(Vec::new, |p| rust_segments(p, src));
            out.push((with(&prefix, p), None, true));
        }
        _ => {}
    }
}

/// How many `mod x { … }` bodies enclose `node`.
fn inline_mod_depth(node: Node<'_>) -> usize {
    let mut n = 0;
    let mut p = node.parent();
    while let Some(a) = p {
        if a.kind() == "mod_item" {
            n += 1;
        }
        p = a.parent();
    }
    n
}

/// `a::b::c` → `[a, b, c]`; a global path `::a::b` → `["", a, b]` (the empty
/// head tells the resolver to skip local modules: `::tools` is the crate even
/// when the file has a `mod tools;`).
fn rust_segments(node: Node<'_>, src: &[u8]) -> Vec<String> {
    if node.kind() == "scoped_identifier" {
        let mut v = match node.child_by_field_name("path") {
            Some(p) => rust_segments(p, src),
            None if src[node.byte_range()].starts_with(b"::") => vec![String::new()],
            None => Vec::new(),
        };
        if let Some(n) = node.child_by_field_name("name") {
            v.push(String::from_utf8_lossy(&src[n.byte_range()]).into_owned());
        }
        v
    } else {
        vec![String::from_utf8_lossy(&src[node.byte_range()]).into_owned()]
    }
}

/// `#[path = "…"]` on the attribute items right before a `mod x;`.
fn rust_path_attr(mod_item: Node<'_>, src: &[u8]) -> Option<String> {
    let mut prev = mod_item.prev_named_sibling();
    while let Some(a) = prev {
        if a.kind() != "attribute_item" {
            return None;
        }
        let attr = a.named_child(0)?;
        if attr
            .named_child(0)
            .is_some_and(|n| &src[n.byte_range()] == b"path")
            && let Some(v) = attr.child_by_field_name("value")
            && v.kind() == "string_literal"
        {
            return Some(
                String::from_utf8_lossy(&src[v.byte_range()])
                    .trim_matches('"')
                    .to_owned(),
            );
        }
        prev = a.prev_named_sibling();
    }
    None
}

/// Names an `import`/`export … from`/`require` statement brings into scope.
fn js_names(stmt: Node<'_>, src: &[u8]) -> Option<Vec<String>> {
    let text = |n: Node<'_>| String::from_utf8_lossy(&src[n.byte_range()]).into_owned();
    let mut names = Vec::new();
    match stmt.kind() {
        "import_statement" => {
            let Some(clause) = stmt.child_by_field_name("import").or_else(|| {
                let mut c = stmt.walk();
                stmt.named_children(&mut c)
                    .find(|n| n.kind() == "import_clause")
            }) else {
                return Some(names);
            };
            let mut c = clause.walk();
            for part in clause.named_children(&mut c) {
                match part.kind() {
                    "identifier" => names.push(text(part)),
                    "namespace_import" => {
                        if let Some(id) = part.named_child(0) {
                            names.push(text(id));
                        }
                    }
                    "named_imports" => {
                        let mut cc = part.walk();
                        for spec in part.named_children(&mut cc) {
                            if spec.kind() != "import_specifier" {
                                continue;
                            }
                            if let Some(n) = spec
                                .child_by_field_name("alias")
                                .or_else(|| spec.child_by_field_name("name"))
                            {
                                names.push(text(n));
                            }
                        }
                    }
                    _ => {}
                }
            }
            Some(names)
        }
        "export_statement" => {
            let mut c = stmt.walk();
            for part in stmt.named_children(&mut c) {
                match part.kind() {
                    "export_clause" => {
                        let mut cc = part.walk();
                        for spec in part.named_children(&mut cc) {
                            if let Some(n) = spec
                                .child_by_field_name("alias")
                                .or_else(|| spec.child_by_field_name("name"))
                            {
                                names.push(text(n));
                            }
                        }
                    }
                    "namespace_export" => {
                        if let Some(id) = part.named_child(0) {
                            names.push(text(id));
                        }
                    }
                    _ => {}
                }
            }
            // `export * from "./a"` has neither clause: a glob.
            (!names.is_empty()).then_some(names)
        }
        // `const x = require("./m")` / `const { a, b } = require("./m")`.
        "call_expression" => {
            let decl = stmt
                .parent()
                .filter(|p| p.kind() == "variable_declarator")?;
            let target = decl.child_by_field_name("name")?;
            match target.kind() {
                "identifier" => Some(vec![text(target)]),
                "object_pattern" => {
                    let mut c = target.walk();
                    for p in target.named_children(&mut c) {
                        match p.kind() {
                            "shorthand_property_identifier_pattern" => names.push(text(p)),
                            "pair_pattern" => {
                                if let Some(v) = p.child_by_field_name("value") {
                                    names.push(text(v));
                                }
                            }
                            _ => {}
                        }
                    }
                    Some(names)
                }
                _ => None,
            }
        }
        _ => None,
    }
}

/// `import a.b [as c], d` / `from [..]m import x [as y], *`.
fn py_imports(stmt: Node<'_>, src: &[u8], line: i64, out: &mut Vec<Import>) {
    let text = |n: Node<'_>| String::from_utf8_lossy(&src[n.byte_range()]).into_owned();
    let alias_of = |n: Node<'_>| -> (String, Option<String>) {
        if n.kind() == "aliased_import" {
            (
                n.child_by_field_name("name").map(text).unwrap_or_default(),
                n.child_by_field_name("alias").map(text),
            )
        } else {
            (text(n), None)
        }
    };
    let mut c = stmt.walk();
    if stmt.kind() == "import_statement" {
        for n in stmt.children_by_field_name("name", &mut c) {
            let (module, alias) = alias_of(n);
            let first = module.split('.').next().unwrap_or(&module).to_owned();
            out.push(Import {
                specifier: module,
                names: Some(vec![alias.unwrap_or(first)]),
                line,
                kind: Kind::Use,
            });
        }
        return;
    }
    let Some(module) = stmt.child_by_field_name("module_name").map(text) else {
        return;
    };
    let mut names = Vec::new();
    for n in stmt.children_by_field_name("name", &mut c) {
        let (name, alias) = alias_of(n);
        names.push(alias.unwrap_or(name));
    }
    let mut c = stmt.walk();
    let glob = stmt
        .named_children(&mut c)
        .any(|n| n.kind() == "wildcard_import");
    out.push(Import {
        specifier: module,
        names: (!glob).then_some(names),
        line,
        kind: Kind::Use,
    });
}

#[cfg(test)]
mod tests {
    use super::*;

    fn run(lang: Language, src: &str) -> Vec<(String, Option<Vec<String>>, Kind)> {
        let mut p = tree_sitter::Parser::new();
        let l: tree_sitter::Language = match lang {
            Language::Rust => tree_sitter_rust::LANGUAGE.into(),
            Language::TypeScript => tree_sitter_typescript::LANGUAGE_TYPESCRIPT.into(),
            Language::JavaScript => tree_sitter_javascript::LANGUAGE.into(),
            Language::Python => tree_sitter_python::LANGUAGE.into(),
            Language::Go => tree_sitter_go::LANGUAGE.into(),
            _ => unreachable!(),
        };
        p.set_language(&l).unwrap();
        let tree = p.parse(src, None).unwrap();
        imports(lang, query(lang).unwrap(), tree.root_node(), src.as_bytes())
            .into_iter()
            .map(|i| (i.specifier, i.names, i.kind))
            .collect()
    }
    fn s(v: &[&str]) -> Option<Vec<String>> {
        Some(v.iter().map(|s| (*s).to_owned()).collect())
    }

    #[test]
    fn rust_use_shapes() {
        let got = run(
            Language::Rust,
            "use tools::registry::ToolRegistry;\nuse a::{b, c::{D, E as F}, g::*, self};\nuse crate::x;\nuse foo;\npub use std::fmt as f;\nmod store;\n#[cfg(test)]\n#[path = \"other.rs\"]\nmod tests;\nmod inline { use super::*; use super::super::y; mod deeper { use super::super::z::*; } }\nextern crate serde as sd;\nuse ::tools::{Z, w::V};\n",
        );
        assert_eq!(
            got,
            vec![
                ("tools::registry".into(), s(&["ToolRegistry"]), Kind::Use),
                ("a".into(), s(&["b"]), Kind::Use),
                ("a::c".into(), s(&["D"]), Kind::Use),
                ("a::c".into(), s(&["F"]), Kind::Use),
                ("a::g".into(), None, Kind::Use),
                ("a".into(), s(&["a"]), Kind::Use),
                ("crate".into(), s(&["x"]), Kind::Use),
                ("foo".into(), s(&["foo"]), Kind::Use),
                ("std".into(), s(&["f"]), Kind::Use),
                ("mod:store".into(), None, Kind::Mod),
                ("mod:tests".into(), s(&["other.rs"]), Kind::Mod),
                ("self".into(), None, Kind::Use),
                ("super".into(), s(&["y"]), Kind::Use),
                ("self::z".into(), None, Kind::Use),
                ("serde".into(), s(&["sd"]), Kind::Extern),
                ("::tools".into(), s(&["Z"]), Kind::Use),
                ("::tools::w".into(), s(&["V"]), Kind::Use),
            ]
        );
    }

    #[test]
    fn ts_import_shapes() {
        let got = run(
            Language::TypeScript,
            "import { a, b as c } from \"./m\";\nimport D from './d';\nimport * as ns from \"@lib/util\";\nimport './side';\nimport type { T } from './t';\nexport * from './all';\nexport { x as y } from './x';\nconst q = await import('./dyn');\n",
        );
        assert_eq!(
            got,
            vec![
                ("./m".into(), s(&["a", "c"]), Kind::Use),
                ("./d".into(), s(&["D"]), Kind::Use),
                ("@lib/util".into(), s(&["ns"]), Kind::Use),
                ("./side".into(), Some(vec![]), Kind::Use),
                ("./t".into(), s(&["T"]), Kind::Use),
                ("./all".into(), None, Kind::Use),
                ("./x".into(), s(&["y"]), Kind::Use),
                ("./dyn".into(), None, Kind::Use),
            ]
        );
        let got = run(
            Language::JavaScript,
            "const fs = require(\"fs\");\nconst { a, b: c } = require('./m');\nfoo(\"not an import\");\n",
        );
        assert_eq!(
            got,
            vec![
                ("fs".into(), s(&["fs"]), Kind::Use),
                ("./m".into(), s(&["a", "c"]), Kind::Use)
            ]
        );
    }

    #[test]
    fn py_import_shapes() {
        let got = run(
            Language::Python,
            "import os, sys as system\nimport a.b\nfrom ..store import Store, open_db as od\nfrom . import x\nfrom pkg.mod import *\n",
        );
        assert_eq!(
            got,
            vec![
                ("os".into(), s(&["os"]), Kind::Use),
                ("sys".into(), s(&["system"]), Kind::Use),
                ("a.b".into(), s(&["a"]), Kind::Use),
                ("..store".into(), s(&["Store", "od"]), Kind::Use),
                (".".into(), s(&["x"]), Kind::Use),
                ("pkg.mod".into(), None, Kind::Use),
            ]
        );
    }

    #[test]
    fn go_import_shapes() {
        let got = run(
            Language::Go,
            "package main\nimport (\n\t\"fmt\"\n\tst \"example.com/m/pkg/store\"\n\t_ \"embed\"\n\t. \"strings\"\n)\n",
        );
        assert_eq!(
            got,
            vec![
                ("fmt".into(), s(&["fmt"]), Kind::Use),
                ("example.com/m/pkg/store".into(), s(&["st"]), Kind::Use),
                ("embed".into(), Some(vec![]), Kind::Use),
                ("strings".into(), None, Kind::Use),
            ]
        );
    }
}
