//! Symbol refs and multi-line-string protection for the text-ref tools (193
//! §D6), through the [`tools::textref::Syntax`] hook. `tools` cannot depend
//! on this crate, so the plugin installs the hook at start.

use std::path::Path;
use std::sync::Arc;

use tools::textref::{SymbolSpan, Syntax, SyntaxInfo};
use tree_sitter::Node;

use crate::lang::Language;
use crate::parse::{self, Parsers};

/// [`Syntax`] over the codeindex tags queries and grammars.
pub struct SyntaxHook;

/// Install the hook; a no-op if one is already installed.
pub fn register() {
    if !tools::textref::set_syntax(Arc::new(SyntaxHook)) {
        tracing::debug!("text-ref syntax hook already installed");
    }
}

impl Syntax for SyntaxHook {
    fn analyze(&self, path: &Path, text: &str) -> Option<SyntaxInfo> {
        let lang = Language::from_ext(path.extension()?.to_str()?);
        let cfg = lang.tags()?;
        let src = text.as_bytes();
        let mut parsers = Parsers::default();
        // One tree: the one `parse_tree` already built for the import query.
        let (parsed, tree) = parse::parse_tree(&mut parsers, cfg, lang, src)?;
        let tree = match tree {
            Some(t) => t,
            None => {
                parsers.tree.set_language(&cfg.language).ok()?;
                parsers.tree.parse(text, None)?
            }
        };
        let root = tree.root_node();

        let mut names: Vec<String> = Vec::with_capacity(parsed.symbols.len());
        let mut symbols = Vec::with_capacity(parsed.symbols.len());
        for s in &parsed.symbols {
            let name = match s.parent.and_then(|p| names.get(p)) {
                Some(parent) => format!("{parent}.{}", s.name),
                None => s.name.clone(),
            };
            names.push(name.clone());
            let (bs, be) = (s.byte_start as usize, s.byte_end as usize);
            let line_start = s.line_start as usize;
            let start = root
                .descendant_for_byte_range(bs, be)
                .map_or(line_start, |n| widened_start(lang, outermost(n), src))
                .min(line_start);
            symbols.push(SymbolSpan {
                name,
                kind: s.kind.to_owned(),
                start,
                end: s.line_end as usize,
            });
        }
        Some(SyntaxInfo {
            symbols,
            string_lines: string_lines(lang, root),
            has_errors: root.has_error(),
        })
    }
}

/// Climb while the parent covers exactly the same bytes.
fn outermost(mut n: Node<'_>) -> Node<'_> {
    while let Some(p) = n.parent() {
        if p.byte_range() != n.byte_range() {
            break;
        }
        n = p;
    }
    n
}

/// 1-based first line of `def` with its declaration wrapper (Go
/// `type_declaration`, TS/JS `const`/`let`/`var`), decorators/`export` and the
/// doc comments and attributes directly above it (no blank line between, and
/// each starting its own line).
fn widened_start(lang: Language, def: Node<'_>, src: &[u8]) -> usize {
    let decl: &[&str] = match lang {
        Language::Go => &["type_declaration"],
        Language::TypeScript | Language::Tsx | Language::JavaScript => {
            &["lexical_declaration", "variable_declaration"]
        }
        _ => &[],
    };
    let wrap = match lang {
        Language::Python => "decorated_definition",
        Language::TypeScript | Language::Tsx | Language::JavaScript => "export_statement",
        _ => "",
    };
    let mut node = def;
    // Tag on `type_spec` / `variable_declarator`: the declaration is the parent
    // or grandparent. A grouped `type (...)` / `const a, b` is left alone so
    // sibling symbols do not start on the same line.
    let up = def
        .parent()
        .into_iter()
        .chain(def.parent().and_then(|p| p.parent()))
        .find(|a| decl.contains(&a.kind()) && a.named_child_count() == 1);
    if let Some(a) = up {
        node = a;
    }
    if let Some(p) = node.parent()
        && p.kind() == wrap
    {
        node = p;
    }
    while let Some(prev) = node.prev_named_sibling() {
        // A line comment's span may end with its newline (column 0 of the next row).
        let end = prev.end_position();
        let end_row = if end.column == 0 && end.row > prev.start_position().row {
            end.row - 1
        } else {
            end.row
        };
        if end_row + 1 < node.start_position().row
            || !starts_line(src, prev.start_byte())
            || !attached(lang, prev, src)
        {
            break;
        }
        node = prev;
    }
    node.start_position().row + 1
}

/// Only spaces/tabs between the previous newline (or file start) and `byte`.
fn starts_line(src: &[u8], byte: usize) -> bool {
    src[..byte]
        .iter()
        .rev()
        .take_while(|&&b| b != b'\n')
        .all(|&b| b == b' ' || b == b'\t')
}

/// Is `n` an attribute, doc comment or decorator of the definition below it?
fn attached(lang: Language, n: Node<'_>, src: &[u8]) -> bool {
    let text = || n.utf8_text(src).unwrap_or("");
    match (lang, n.kind()) {
        (Language::Rust, "attribute_item") => true,
        (Language::Rust, "line_comment") => {
            let t = text();
            t.starts_with("///") && !t.starts_with("////")
        }
        (Language::Rust, "block_comment") => block_doc(text()),
        (Language::TypeScript | Language::Tsx | Language::JavaScript, "decorator") => true,
        (Language::TypeScript | Language::Tsx | Language::JavaScript, "comment") => {
            block_doc(text())
        }
        // godoc: any `//` run directly above.
        (Language::Go, "comment") => true,
        _ => false,
    }
}

/// `/** … */`, not `/***` or `/**/`.
fn block_doc(t: &str) -> bool {
    t.starts_with("/**") && !t.starts_with("/***") && !t.starts_with("/**/")
}

/// 1-based lines after the opening row of every multi-line string literal.
fn string_lines(lang: Language, root: Node<'_>) -> Vec<usize> {
    let kinds: &[&str] = match lang {
        Language::Rust => &["string_literal", "raw_string_literal"],
        Language::Python => &["string"],
        Language::TypeScript | Language::Tsx | Language::JavaScript => {
            &["string", "template_string"]
        }
        Language::Go => &["raw_string_literal", "interpreted_string_literal"],
        Language::Other => &[],
    };
    let mut out = Vec::new();
    let mut stack = vec![root];
    while let Some(n) = stack.pop() {
        if kinds.contains(&n.kind()) {
            let (a, b) = (n.start_position().row, n.end_position().row);
            out.extend(a + 2..=b + 1);
            continue;
        }
        let mut cur = n.walk();
        stack.extend(n.children(&mut cur));
    }
    out.sort_unstable();
    out.dedup();
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use tools::text::{Indent, IndentArgs};
    use tools::textref::find_symbol;
    use tools::{Tool, ToolCx};

    fn info(path: &str, text: &str) -> SyntaxInfo {
        SyntaxHook.analyze(Path::new(path), text).unwrap()
    }

    fn sym<'a>(i: &'a SyntaxInfo, name: &str) -> &'a SymbolSpan {
        i.symbols
            .iter()
            .find(|s| s.name == name)
            .unwrap_or_else(|| panic!("no {name} in {:?}", i.symbols))
    }

    #[test]
    fn rust_doc_and_multiline_attribute_widen() {
        let src = "use a;\n\n/// one\n/// two\n#[derive(\n    Debug,\n)]\n#[cfg(test)]\nfn f() {}\n\n// not doc\nfn g() {}\n\n/// detached\n\nfn h() {}\n";
        let i = info("x.rs", src);
        assert_eq!((sym(&i, "f").start, sym(&i, "f").end), (3, 9));
        assert_eq!(sym(&i, "g").start, 12);
        assert_eq!(sym(&i, "h").start, 16);
    }

    #[test]
    fn rust_impl_method_is_qualified() {
        let src = "struct Reader;\n\nimpl Reader {\n    /// load it\n    fn load(&self) {}\n}\n";
        let i = info("x.rs", src);
        let m = sym(&i, "Reader.load");
        assert_eq!((m.start, m.end), (4, 5));
        assert_eq!(
            find_symbol(Some(&i), "x.rs", "Reader.load").unwrap(),
            (4, 5)
        );
    }

    #[test]
    fn python_decorators_widen() {
        let src = "@decorator\n@other(x)\ndef f():\n    pass\n\nclass A:\n    @staticmethod\n    @other(y)\n    def m():\n        pass\n";
        let i = info("x.py", src);
        assert_eq!((sym(&i, "f").start, sym(&i, "f").end), (1, 4));
        let m = sym(&i, "A.m");
        assert_eq!((m.start, m.end), (7, 10));
    }

    #[test]
    fn ambiguous_method_lists_both() {
        let src = "struct A;\nstruct B;\nimpl A {\n    fn load(&self) {}\n}\nimpl B {\n    fn load(&self) {}\n}\n";
        let i = info("x.rs", src);
        let e = find_symbol(Some(&i), "x.rs", "load")
            .unwrap_err()
            .to_string();
        assert!(e.contains("A.load") && e.contains("B.load"), "{e}");
        assert!(find_symbol(Some(&i), "x.rs", "B.load").is_ok());
    }

    #[test]
    fn rust_block_doc_attached_inner_and_four_slash_not() {
        let i = info(
            "x.rs",
            "/** doc */\nfn f() {}\n\n//! inner\n//// four\nfn g() {}\n",
        );
        assert_eq!(sym(&i, "f").start, 1);
        assert_eq!(sym(&i, "g").start, 6);
    }

    #[test]
    fn ts_jsdoc_and_export_const_arrow_widen() {
        let i = info(
            "x.ts",
            "/** doc */\nfunction f() {}\n\nconst x = 1;\n/** doc */\nexport const h = () => 1;\n",
        );
        assert_eq!(sym(&i, "f").start, 1);
        assert_eq!(sym(&i, "h").start, 5);
        let i = info("x.ts", "// plain\nconst k = () => 1;\n");
        assert_eq!(sym(&i, "k").start, 2);
    }

    #[test]
    fn js_arrow_const_resolves() {
        let i = info("x.js", "const g = () => 2;\n");
        assert_eq!((sym(&i, "g").start, sym(&i, "g").end), (1, 1));
        assert_eq!(find_symbol(Some(&i), "x.js", "g").unwrap(), (1, 1));
    }

    #[test]
    fn go_doc_widens_func_and_type() {
        let i = info(
            "x.go",
            "package p\n\n// doc f\nfunc f() {}\n\n// doc T\ntype T struct{}\n",
        );
        assert_eq!(sym(&i, "f").start, 3);
        assert_eq!(sym(&i, "T").start, 6);
    }

    #[test]
    fn go_trailing_comment_is_not_next_funcs_doc() {
        let i = info("x.go", "package p\n\nfunc a() {} // a\nfunc b() {}\n");
        assert_eq!(sym(&i, "a").start, 3);
        assert_eq!(sym(&i, "b").start, 4);
    }

    #[test]
    fn python_decorator_across_blank_line_stays_decorated() {
        // tree-sitter-python keeps the decorator in the `decorated_definition`
        // even with a blank line before the `def`, so the span starts at it.
        let i = info("x.py", "@dec\n\ndef f():\n    pass\n");
        assert_eq!((sym(&i, "f").start, sym(&i, "f").end), (1, 4));
    }

    #[test]
    fn has_errors_reflects_tree() {
        assert!(info("x.rs", "fn f( {}\n").has_errors);
        assert!(!info("x.rs", "fn f() {}\n").has_errors);
    }

    #[test]
    fn no_parser_is_none() {
        assert!(
            SyntaxHook
                .analyze(Path::new("notes.txt"), "fn f() {}")
                .is_none()
        );
        assert!(SyntaxHook.analyze(Path::new("Makefile"), "x").is_none());
        assert!(find_symbol(None, "notes.txt", "f").is_err());
    }

    #[test]
    fn rust_string_lines() {
        let src = "fn f() {\n    let s = \"a\n    b\n\";\n    let r = r#\"x\n  y\"#;\n    let one = \"z\";\n}\n";
        assert_eq!(info("x.rs", src).string_lines, vec![3, 4, 6]);
    }

    #[test]
    fn python_string_lines() {
        let src = "def f():\n    s = \"\"\"a\n  b\n\"\"\"\n    return s\n";
        assert_eq!(info("x.py", src).string_lines, vec![3, 4]);
    }

    fn cx() -> ToolCx {
        ToolCx {
            session: runtime::SessionId::new(),
            call_id: String::new(),
            quiet: true,
            resources: tools::SessionResources::durable_arc(&std::env::temp_dir()),
            cancel: tools::CancellationToken::new(),
            promote_after: None,
        }
    }

    /// Indent `symbol` of `name` by one unit through the real tool and hook.
    async fn indent(name: &str, before: &str, symbol: &str) -> (String, String) {
        register();
        let dir = crate::TestDir::new("syntax-hook");
        let path = dir.0.join(name);
        std::fs::write(&path, before).unwrap();
        let args = IndentArgs {
            path: path.clone(),
            range: symbol.into(),
            by: Some(1),
            r#match: None,
        };
        let out = Indent.call(args, cx()).await.unwrap();
        assert!(!out.is_error, "{}", out.content);
        (std::fs::read_to_string(&path).unwrap(), out.content)
    }

    fn kept(receipt: &str) {
        assert!(
            receipt.contains("lines inside a multi-line string kept"),
            "{receipt}"
        );
    }

    #[tokio::test]
    async fn indent_keeps_rust_string() {
        let (got, r) = indent(
            "a.rs",
            "fn f() {\n    let s = \"a\n  b\n\";\n    g();\n}\n",
            "f",
        )
        .await;
        assert_eq!(
            got,
            "    fn f() {\n        let s = \"a\n  b\n\";\n        g();\n    }\n"
        );
        kept(&r);
    }

    #[tokio::test]
    async fn indent_keeps_python_string() {
        let (got, r) = indent(
            "a.py",
            "def f():\n    s = \"\"\"a\n  b\n\"\"\"\n    return s\n",
            "f",
        )
        .await;
        assert_eq!(
            got,
            "    def f():\n        s = \"\"\"a\n  b\n\"\"\"\n        return s\n"
        );
        kept(&r);
    }

    #[tokio::test]
    async fn indent_keeps_typescript_template() {
        let (got, r) = indent(
            "a.ts",
            "function f() {\n    const s = `a\n  b\n`;\n    return s;\n}\n",
            "f",
        )
        .await;
        assert_eq!(
            got,
            "    function f() {\n        const s = `a\n  b\n`;\n        return s;\n    }\n"
        );
        kept(&r);
    }

    #[tokio::test]
    async fn indent_keeps_go_raw_string() {
        let (got, r) = indent("a.go", "func f() {\n\ts := `a\n  b\n`\n\t_ = s\n}\n", "f").await;
        assert_eq!(got, "\tfunc f() {\n\t\ts := `a\n  b\n`\n\t\t_ = s\n\t}\n");
        kept(&r);
    }
}
