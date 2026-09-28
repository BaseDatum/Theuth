//! Languages, extension map and one [`TagsConfiguration`] per language (60 §5.2–5.3).
//!
//! The grammar crates' shipped `TAGS_QUERY` is the whole per-language parse
//! logic; TS/TSX = the JS query + the TS supplement (60 §2.B). Edits: §5.3
//! `(#is-not? local)` on bare-identifier `@reference.call` patterns (not on
//! receiver calls — 60 §20), a small locals query for the grammars that ship
//! none, and a few prepended patterns so a reference tag's range covers its
//! qualifier (`a::b()`, `obj.f()`, `pkg.T`) for 60 §6.2 rule 2.

use std::sync::LazyLock;

use serde::{Deserialize, Serialize};
use tree_sitter_tags::TagsConfiguration;

pub mod imports;

/// Parsed languages (60 §0.4). `Other` files get a `files` row and no parse.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Language {
    /// `.rs`
    Rust,
    /// `.ts`
    TypeScript,
    /// `.tsx`
    Tsx,
    /// `.js`, `.mjs`, `.cjs`, `.jsx`
    JavaScript,
    /// `.py`
    Python,
    /// `.go`
    Go,
    /// Everything else: listed, not parsed.
    Other,
}

impl Language {
    /// Language by file extension (60 §5.1).
    pub fn from_ext(ext: &str) -> Self {
        match ext {
            "rs" => Self::Rust,
            "ts" => Self::TypeScript,
            "tsx" => Self::Tsx,
            "js" | "mjs" | "cjs" | "jsx" => Self::JavaScript,
            "py" => Self::Python,
            "go" => Self::Go,
            _ => Self::Other,
        }
    }

    /// The `files.language` column value.
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Rust => "rust",
            Self::TypeScript => "typescript",
            Self::Tsx => "tsx",
            Self::JavaScript => "javascript",
            Self::Python => "python",
            Self::Go => "go",
            Self::Other => "other",
        }
    }

    /// The config-file name (`languages:` list, 60 §9). `typescript` covers TSX.
    pub fn config_name(self) -> &'static str {
        match self {
            Self::Tsx => "typescript",
            l => l.as_str(),
        }
    }

    /// The tags configuration, built once per language. `None`: `Other`, or
    /// the query failed to compile against the grammar (warned once; language
    /// disabled, 60 §11).
    pub fn tags(self) -> Option<&'static TagsConfiguration> {
        macro_rules! lazy {
            ($l:expr) => {{
                static CFG: LazyLock<Option<TagsConfiguration>> =
                    LazyLock::new(|| build_or_warn($l));
                CFG.as_ref()
            }};
        }
        match self {
            Self::Rust => lazy!(Language::Rust),
            Self::TypeScript => lazy!(Language::TypeScript),
            Self::Tsx => lazy!(Language::Tsx),
            Self::JavaScript => lazy!(Language::JavaScript),
            Self::Python => lazy!(Language::Python),
            Self::Go => lazy!(Language::Go),
            Self::Other => None,
        }
    }

    /// Build the configuration. `locals: false` skips §5.3 (the "without locals"
    /// measurement in 60 §16 row 1); production always uses `true`.
    pub fn build(self, locals: bool) -> Result<TagsConfiguration, tree_sitter_tags::Error> {
        let (lang, tags, locals_q): (tree_sitter::Language, String, &str) = match self {
            Self::Rust => (
                tree_sitter_rust::LANGUAGE.into(),
                format!("{RUST_PRE}\n{}\n{RUST_POST}", tree_sitter_rust::TAGS_QUERY),
                RUST_LOCALS,
            ),
            Self::TypeScript | Self::Tsx => {
                let l = if self == Self::Tsx {
                    tree_sitter_typescript::LANGUAGE_TSX
                } else {
                    tree_sitter_typescript::LANGUAGE_TYPESCRIPT
                };
                (
                    l.into(),
                    format!(
                        "{JS_PRE}\n{}\n{}",
                        tree_sitter_javascript::TAGS_QUERY,
                        tree_sitter_typescript::TAGS_QUERY
                    ),
                    TS_LOCALS,
                )
            }
            Self::JavaScript => (
                tree_sitter_javascript::LANGUAGE.into(),
                format!("{JS_PRE}\n{}", tree_sitter_javascript::TAGS_QUERY),
                tree_sitter_javascript::LOCALS_QUERY,
            ),
            Self::Python => (
                tree_sitter_python::LANGUAGE.into(),
                PY_TAGS.to_owned(),
                PY_LOCALS,
            ),
            Self::Go => (
                tree_sitter_go::LANGUAGE.into(),
                format!("{GO_PRE}\n{}", tree_sitter_go::TAGS_QUERY),
                "",
            ),
            Self::Other => return Err(tree_sitter_tags::Error::InvalidCapture("other".into())),
        };
        if locals && !locals_q.is_empty() {
            TagsConfiguration::new(lang, &not_local_calls(&tags), locals_q)
        } else {
            TagsConfiguration::new(lang, &tags, "")
        }
    }
}

fn build_or_warn(l: Language) -> Option<TagsConfiguration> {
    match l.build(true) {
        Ok(c) => Some(c),
        Err(e) => {
            tracing::warn!(language = l.as_str(), error = %e, "tags query failed to compile; language disabled");
            None
        }
    }
}

/// Additions to the shipped Rust query. Prepended, so they win the
/// one-tag-per-node tie (lower pattern index): the shipped
/// `(declaration_list (function_item))` pattern calls every function inside
/// a `mod { }` body a `method`; these say `impl`/`trait` bodies are methods
/// and `mod` bodies are functions. Appended: `const`/`static` items as
/// `definition.constant` (60 §4 lists the kind; §16 row 1 expects
/// `MAX_BYTES` in an outline), `a::b()` path calls as `reference.call`
/// (the shipped query only sees bare `b()`; `Registry::new()` would be no
/// reference at all), and `impl<T> Foo<T> {` as an implementation tag (the
/// shipped pattern wants a bare `type_identifier`) so its methods get an
/// owner (60 §20.3 deviation 8).
const RUST_PRE: &str = "\
(impl_item body: (declaration_list (function_item name: (identifier) @name) @definition.method))
(trait_item body: (declaration_list (function_item name: (identifier) @name) @definition.method))
(mod_item body: (declaration_list (function_item name: (identifier) @name) @definition.function))
";
const RUST_POST: &str = "\
(const_item name: (identifier) @name) @definition.constant
(static_item name: (identifier) @name) @definition.constant
(call_expression function: (scoped_identifier name: (identifier) @name)) @reference.call
(impl_item type: (generic_type type: (type_identifier) @name) !trait) @reference.implementation
";

/// Prepended to the JS query (and so to TS/TSX): the shipped method-call
/// pattern puts `@reference.call` on the *arguments* node, so the tag's range
/// starts at the name and the receiver text (`obj.`) is not in it. This one
/// tags the whole `call_expression` for the same name range and wins the tie,
/// so `parse` can read the qualifier (60 §6.2 rule 2) from the tag's range.
const JS_PRE: &str = "\
(call_expression function: (member_expression property: (property_identifier) @name)) @reference.call
";

/// Prepended to the Go query: `pkg.Type` as one `reference.type` tag spanning
/// the qualifier (the shipped `(type_identifier)` pattern sees only `Type`).
const GO_PRE: &str = "\
(qualified_type package: (package_identifier) name: (type_identifier) @name) @reference.type
";

/// The shipped Python query, with its one `reference.call` alternation split
/// in two so the §5.3 locals predicate can go on bare calls only: `x.f()` is
/// a different namespace from a local `f`. Otherwise verbatim
/// (`tree-sitter-python` 0.25 `queries/tags.scm`).
const PY_TAGS: &str = "\
(module (expression_statement (assignment left: (identifier) @name) @definition.constant))
(class_definition name: (identifier) @name) @definition.class
(function_definition name: (identifier) @name) @definition.function
(call function: (identifier) @name) @reference.call
(call function: (attribute attribute: (identifier) @name)) @reference.call
";

/// Rust ships no locals query: parameters, closure parameters and `let`
/// bindings are locals inside functions, closures and blocks (60 §5.3).
const RUST_LOCALS: &str = "\
(function_item) @local.scope
(closure_expression) @local.scope
(block) @local.scope
(parameter pattern: (identifier) @local.definition)
(closure_parameters (identifier) @local.definition)
(let_declaration pattern: (identifier) @local.definition)
";

/// Python ships no locals query: parameters and assignments inside
/// functions and lambdas (60 §5.3).
const PY_LOCALS: &str = "\
(function_definition) @local.scope
(lambda) @local.scope
(parameters (identifier) @local.definition)
(default_parameter name: (identifier) @local.definition)
(typed_parameter (identifier) @local.definition)
(typed_default_parameter name: (identifier) @local.definition)
(lambda_parameters (identifier) @local.definition)
(assignment left: (identifier) @local.definition)
";

/// TS: the grammar's own two-line parameter query plus the JS scopes and
/// `variable_declarator` definitions it relies on (the grammar's
/// `tree-sitter.json` concatenates them the same way), plus destructured
/// bindings (`const { resolve, reject } = …`, `const [a, b] = …`) which the
/// upstream query misses (60 §21).
const TS_LOCALS: &str = "\
[
  (statement_block)
  (function_expression)
  (arrow_function)
  (function_declaration)
  (method_definition)
] @local.scope
(variable_declarator name: (identifier) @local.definition)
(variable_declarator name: (object_pattern (shorthand_property_identifier_pattern) @local.definition))
(variable_declarator name: (array_pattern (identifier) @local.definition))
(required_parameter (identifier) @local.definition)
(optional_parameter (identifier) @local.definition)
";

/// Receiver shapes: a method name is its own namespace, so a local variable
/// `len` must not hide `x.len()` (60 §19.3 deviation 11, reversed in step 2).
const RECEIVER_NODES: [&str; 4] = [
    "(field_expression",
    "(member_expression",
    "(attribute",
    "(selector_expression",
];

/// Append `(#is-not? local)` to every top-level pattern carrying
/// `@reference.call` except the receiver shapes in [`RECEIVER_NODES`]. A
/// pattern that is already a group `((…) @x (#pred))` gets the predicate
/// inside; a bare node pattern is wrapped in a group. Comments (`;`) and
/// strings are skipped when balancing parentheses.
pub fn not_local_calls(query: &str) -> String {
    let mut out = String::with_capacity(query.len() + 256);
    let b = query.as_bytes();
    let mut i = 0;
    while i < b.len() {
        match b[i] {
            b';' => {
                let end = memchr_nl(b, i);
                out.push_str(&query[i..end]);
                i = end;
            }
            b'(' | b'[' => {
                let mut end = expr_end(b, i);
                // Trailing captures on the same line: `) @name) @definition.class`.
                loop {
                    let mut j = end;
                    while j < b.len() && (b[j] == b' ' || b[j] == b'\t') {
                        j += 1;
                    }
                    if j < b.len() && b[j] == b'@' {
                        j += 1;
                        while j < b.len()
                            && (b[j].is_ascii_alphanumeric() || b[j] == b'.' || b[j] == b'_')
                        {
                            j += 1;
                        }
                        end = j;
                    } else {
                        break;
                    }
                }
                let expr = &query[i..end];
                if expr.contains("@reference.call")
                    && !RECEIVER_NODES.iter().any(|n| expr.contains(n))
                {
                    let inner = expr[1..].trim_start();
                    if b[i] == b'(' && inner.starts_with('(') && expr.ends_with(')') {
                        // Already a group: insert before the closing paren.
                        out.push_str(&expr[..expr.len() - 1]);
                        out.push_str(" (#is-not? local))");
                    } else {
                        out.push('(');
                        out.push_str(expr);
                        out.push_str(" (#is-not? local))");
                    }
                } else {
                    out.push_str(expr);
                }
                i = end;
            }
            _ => {
                out.push(b[i] as char);
                i += 1;
            }
        }
    }
    out
}

fn memchr_nl(b: &[u8], from: usize) -> usize {
    b[from..]
        .iter()
        .position(|&c| c == b'\n')
        .map_or(b.len(), |p| from + p)
}

/// End (exclusive) of the balanced S-expression starting at `b[start]`.
fn expr_end(b: &[u8], start: usize) -> usize {
    let mut depth = 0usize;
    let mut i = start;
    while i < b.len() {
        match b[i] {
            b'"' => {
                i += 1;
                while i < b.len() && b[i] != b'"' {
                    if b[i] == b'\\' {
                        i += 1;
                    }
                    i += 1;
                }
            }
            b';' => i = memchr_nl(b, i),
            b'(' | b'[' => depth += 1,
            b')' | b']' => {
                depth -= 1;
                if depth == 0 {
                    return i + 1;
                }
            }
            _ => {}
        }
        i += 1;
    }
    b.len()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_language_compiles_with_and_without_locals() {
        for l in [
            Language::Rust,
            Language::TypeScript,
            Language::Tsx,
            Language::JavaScript,
            Language::Python,
            Language::Go,
        ] {
            l.build(false)
                .unwrap_or_else(|e| panic!("{l:?} without locals: {e}"));
            l.build(true)
                .unwrap_or_else(|e| panic!("{l:?} with locals: {e}"));
            assert!(l.tags().is_some(), "{l:?}");
        }
    }

    #[test]
    fn not_local_wraps_bare_and_grouped_patterns() {
        let q = "; c\n(a (b) @name) @reference.call\n(\n  (c) @reference.call\n  (#not-match? @name \"x\")\n)\n(d) @definition.function\n(call_expression function: (field_expression field: (field_identifier) @name)) @reference.call\n";
        let out = not_local_calls(q);
        assert_eq!(
            out,
            "; c\n((a (b) @name) @reference.call (#is-not? local))\n(\n  (c) @reference.call\n  (#not-match? @name \"x\")\n (#is-not? local))\n(d) @definition.function\n(call_expression function: (field_expression field: (field_identifier) @name)) @reference.call\n"
        );
    }
}
