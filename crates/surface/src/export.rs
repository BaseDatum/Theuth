//! Print lowering (design doc 72): a note's markdown as cmarker/Typst can render it.
//!
//! Every `json-render` fence becomes markdown — an image when the client rendered it
//! (`images`, keyed by the block id the `<!-- id -->` annotation names), a compact textual
//! form otherwise; the annotations go; `code:` links flatten to `label (path#La-Lb)`.

use std::collections::BTreeMap;
use std::fmt::Write as _;
use std::path::Path;

use crate::fences::fences;
use crate::parse;
use crate::spec::*;

/// `text` is the annotated markdown of a document (`<!-- id -->` before each top-level block);
/// `images` maps a fence's block id to a rendered SVG on disk. Prose, Mermaid fences and
/// fences that fail to parse are byte-identical.
pub fn print_markdown(text: &str, images: &BTreeMap<String, impl AsRef<Path>>) -> String {
    let mut out = String::with_capacity(text.len());
    let mut pos = 0;
    for f in fences(text).filter(|f| f.closed) {
        let prose = &text[pos..f.range.start];
        pos = f.range.end;
        let id = annotation_before(prose);
        strip_annotations(prose, &mut out);
        if f.lang != "json-render" {
            out.push_str(&text[f.range.clone()]);
            continue;
        }
        let Ok(parsed) = parse(f.body) else {
            out.push_str(&text[f.range.clone()]);
            continue;
        };
        let root = parsed.spec.elements.get(&parsed.spec.root);
        match id.and_then(|id| images.get(id)) {
            Some(path) if root.is_some_and(|e| is_diagram(&e.component)) => {
                let _ = write!(out, "![]({})\n\n", path.as_ref().display());
                if let Some(c) = root.and_then(|e| caption(&e.component)) {
                    let _ = write!(out, "*{c}*");
                }
            }
            _ => out.push_str(lower(&parsed.spec).trim_end()),
        }
    }
    strip_annotations(&text[pos..], &mut out);
    out
}

/// Number of ATX headings outside fences.
pub fn heading_count(text: &str) -> usize {
    let mut n = 0;
    let mut pos = 0;
    let mut count = |s: &str| n += s.lines().filter(|l| is_heading(l)).count();
    for f in fences(text) {
        count(&text[pos..f.range.start]);
        pos = f.range.end;
    }
    count(&text[pos..]);
    n
}

fn is_heading(line: &str) -> bool {
    let t = line.trim_start();
    let hashes = t.bytes().take_while(|b| *b == b'#').count();
    (1..=6).contains(&hashes) && t[hashes..].starts_with([' ', '\t'])
}

fn is_diagram(c: &Component) -> bool {
    matches!(c, Component::Graph(_) | Component::Sequence(_))
}

/// The line under a diagram image.
fn caption(c: &Component) -> Option<String> {
    match c {
        Component::Graph(g) => Some(format!(
            "Graph: {} nodes, {} edges",
            g.nodes.len(),
            g.edges.len()
        )),
        Component::Sequence(s) => Some(match &s.title {
            Some(t) if !t.is_empty() => format!("Sequence: {t}"),
            _ => format!("Sequence: {} steps", s.steps.len()),
        }),
        _ => None,
    }
}

/// The block id of the last `<!-- id -->` annotation in `prose` when nothing but blank lines
/// follows it.
fn annotation_before(prose: &str) -> Option<&str> {
    prose
        .lines()
        .rev()
        .find(|l| !l.trim().is_empty())
        .and_then(annotation_id)
}

/// `<!-- abc123 -->` or `<!-- abc123 by Sky -->` → `abc123`.
fn annotation_id(line: &str) -> Option<&str> {
    let inner = line
        .trim()
        .strip_prefix("<!--")?
        .strip_suffix("-->")?
        .trim();
    let id = inner.split_whitespace().next()?;
    let rest = inner[id.len()..].trim_start();
    (rest.is_empty() || rest.starts_with("by ")).then_some(id)
}

/// `prose` without annotation lines, `code:` links flattened.
fn strip_annotations(prose: &str, out: &mut String) {
    for line in prose.split_inclusive('\n') {
        if annotation_id(line).is_none() {
            flatten_code_links(line, out);
        }
    }
}

/// `[label](code:path#L1-L2)` → `label (path#L1-L2)`.
fn flatten_code_links(mut line: &str, out: &mut String) {
    while let Some(at) = line.find("](code:") {
        let Some(open) = line[..at].rfind('[') else {
            break;
        };
        let Some(close) = line[at + 7..].find(')') else {
            break;
        };
        let label = &line[open + 1..at];
        let target = &line[at + 7..at + 7 + close];
        out.push_str(&line[..open]);
        let _ = write!(out, "{label} ({target})");
        line = &line[at + 7 + close + 1..];
    }
    out.push_str(line);
}

/// One fence as markdown, root first.
pub fn lower(spec: &Spec) -> String {
    let mut out = String::new();
    element(spec, &spec.root, &mut out, 0);
    out
}

fn element(spec: &Spec, id: &str, out: &mut String, depth: usize) {
    if depth > crate::MAX_DEPTH {
        return;
    }
    let Some(e) = spec.elements.get(id) else {
        return;
    };
    let kids = |out: &mut String| {
        for k in &e.children {
            element(spec, k, out, depth + 1);
        }
    };
    match &e.component {
        Component::Card(p) => {
            if let Some(t) = &p.title {
                let _ = writeln!(out, "**{t}**\n");
            }
            if let Some(d) = &p.description {
                let _ = writeln!(out, "{d}\n");
            }
            kids(out);
        }
        Component::Stack(_) | Component::Grid(_) => kids(out),
        Component::Tabs(p) => sections(spec, &p.labels, &e.children, out, depth),
        Component::Accordion(p) => sections(spec, &p.titles, &e.children, out, depth),
        Component::Heading(p) => {
            let _ = writeln!(out, "**{}**\n", p.text);
        }
        Component::Text(p) => {
            let _ = writeln!(out, "{}\n", p.text);
        }
        Component::Graph(p) => graph(p, out),
        Component::Sequence(p) => sequence(p, out),
        Component::Excerpt(p) => excerpt(p, out),
        Component::Code(p) => {
            let _ = writeln!(
                out,
                "```{}\n{}\n```\n",
                p.lang.as_deref().unwrap_or(""),
                p.text.trim_end()
            );
        }
        Component::Steps(p) => {
            for (n, s) in p.items.iter().enumerate() {
                let _ = writeln!(out, "{}. **{}**", n + 1, s.title);
                if let Some(d) = &s.detail {
                    let _ = writeln!(out, "   {d}");
                }
            }
            out.push('\n');
        }
        Component::Table(p) => table(&p.columns, &p.rows, out),
        Component::KeyValueList(p) => {
            for i in &p.items {
                let _ = writeln!(out, "- **{}:** {}", i.key, i.value);
            }
            out.push('\n');
        }
        Component::Checklist(p) => {
            for i in &p.items {
                let _ = writeln!(out, "- [{}] {}", if i.done { 'x' } else { ' ' }, i.text);
            }
            out.push('\n');
        }
        Component::Timeline(p) => {
            for i in &p.items {
                let _ = write!(out, "- **{}** {}", i.when, i.title);
                if let Some(d) = &i.detail {
                    let _ = write!(out, ": {d}");
                }
                out.push('\n');
            }
            out.push('\n');
        }
        Component::Callout(p) => quote(p.title.as_deref(), &p.text, out),
        Component::Alert(p) => quote(p.title.as_deref(), &p.message, out),
        Component::Quote(p) => quote(None, &p.text, out),
        Component::Metric(p) => {
            metric(p, out);
            out.push('\n');
        }
        Component::MetricGrid(p) => {
            for m in &p.items {
                metric(m, out);
            }
            out.push('\n');
        }
        // ---- 123 ELK family (W1) ----
        Component::StateMachine(p) => crate::elk_family::state_machine_md(p, out),
        Component::Entities(p) => crate::elk_family::entities_md(p, out),
        Component::Swimlane(p) => crate::elk_family::swimlane_md(p, out),
        Component::GraphDelta(p) => {
            crate::elk_family::graph_delta_md(p, out);
            graph(
                &crate::elk_family::side_as_graph(&p.after, p.direction),
                out,
            );
        }
        c => {
            let _ = writeln!(out, "*{}*\n", type_name(c));
        }
    }
}

fn sections(spec: &Spec, labels: &[String], kids: &[String], out: &mut String, depth: usize) {
    for (i, k) in kids.iter().enumerate() {
        if let Some(l) = labels.get(i) {
            let _ = writeln!(out, "**{l}**\n");
        }
        element(spec, k, out, depth + 1);
    }
}

fn quote(title: Option<&str>, text: &str, out: &mut String) {
    if let Some(t) = title {
        let _ = writeln!(out, "> **{t}**\n>");
    }
    for l in text.lines() {
        let _ = writeln!(out, "> {l}");
    }
    out.push('\n');
}

fn metric(p: &MetricProps, out: &mut String) {
    let _ = write!(out, "- **{}:** {}", p.label, p.value);
    if let Some(d) = &p.delta {
        let _ = write!(out, " ({d})");
    }
    out.push('\n');
}

fn table(columns: &[String], rows: &[Vec<String>], out: &mut String) {
    let cell = |s: &str| s.replace('|', "\\|").replace('\n', " ");
    let n = columns.len().max(1);
    let row = |cells: &[String], out: &mut String| {
        out.push('|');
        for i in 0..n {
            let _ = write!(
                out,
                " {} |",
                cells.get(i).map(|c| cell(c)).unwrap_or_default()
            );
        }
        out.push('\n');
    };
    row(columns, out);
    let _ = writeln!(out, "|{}", " --- |".repeat(n));
    for r in rows {
        row(r, out);
    }
    out.push('\n');
}

/// `- **A** → B (label), C` per node, grouped like the TUI's adjacency list.
fn graph(p: &GraphProps, out: &mut String) {
    let label_of: BTreeMap<&str, &str> = p
        .nodes
        .iter()
        .map(|n| (n.id.as_str(), n.label.as_str()))
        .collect();
    let _ = writeln!(
        out,
        "Diagram: {} nodes, {} edges\n",
        p.nodes.len(),
        p.edges.len()
    );
    let node = |n: &GraphNode, indent: &str, out: &mut String| {
        let _ = write!(out, "{indent}- **{}**", n.label);
        let targets: Vec<String> = p
            .edges
            .iter()
            .filter(|e| e.from == n.id)
            .map(|e| {
                let to = label_of.get(e.to.as_str()).copied().unwrap_or(&e.to);
                match &e.label {
                    Some(l) if !l.is_empty() => format!("{to} ({l})"),
                    _ => to.to_owned(),
                }
            })
            .collect();
        if !targets.is_empty() {
            let _ = write!(out, " → {}", targets.join(", "));
        }
        out.push('\n');
    };
    let groups = p.groups.as_deref().unwrap_or_default();
    for g in groups {
        let _ = writeln!(out, "- **{}**", g.label);
        for n in p.nodes.iter().filter(|n| n.group.as_deref() == Some(&g.id)) {
            node(n, "  ", out);
        }
    }
    for n in p.nodes.iter().filter(|n| {
        !n.group
            .as_deref()
            .is_some_and(|g| groups.iter().any(|x| x.id == g))
    }) {
        node(n, "", out);
    }
    out.push('\n');
}

/// Numbered `from → to: label` steps.
fn sequence(p: &SequenceProps, out: &mut String) {
    let label_of: BTreeMap<&str, &str> = p
        .actors
        .iter()
        .map(|a| (a.id.as_str(), a.label.as_str()))
        .collect();
    let name = |id: &'_ str| -> String { label_of.get(id).copied().unwrap_or(id).to_owned() };
    if let Some(t) = &p.title {
        let _ = writeln!(out, "**{t}**\n");
    }
    for (i, s) in p.steps.iter().enumerate() {
        let _ = writeln!(
            out,
            "{}. {} → {}: {}",
            i + 1,
            name(&s.from),
            name(&s.to),
            s.label
        );
    }
    out.push('\n');
}

/// A fenced block in the excerpt's language, `path#La-Lb` as its first line comment.
fn excerpt(p: &ExcerptProps, out: &mut String) {
    let start = p.start.unwrap_or(1);
    let end = start + u32::try_from(p.text.lines().count().saturating_sub(1)).unwrap_or(u32::MAX);
    let lang = p.lang.as_deref().unwrap_or("");
    let (open, close) = comment_delims(lang);
    let _ = writeln!(
        out,
        "```{lang}\n{open} {}#L{start}-L{end}{close}\n{}\n```",
        p.path,
        p.text.trim_end()
    );
    if let Some(c) = &p.caption {
        let _ = writeln!(out, "\n*{c}*");
    }
    out.push('\n');
}

fn comment_delims(lang: &str) -> (&'static str, &'static str) {
    match lang {
        "py" | "python" | "rb" | "ruby" | "sh" | "bash" | "zsh" | "fish" | "yaml" | "yml"
        | "toml" | "make" | "makefile" | "dockerfile" | "r" | "pl" | "perl" | "nix" => ("#", ""),
        "html" | "xml" | "svg" | "md" | "markdown" | "vue" | "svelte" => ("<!--", " -->"),
        "css" | "scss" | "less" => ("/*", " */"),
        "sql" | "lua" | "hs" | "haskell" | "ex" | "exs" | "elixir" => ("--", ""),
        "lisp" | "clj" | "clojure" | "el" | "scm" => (";;", ""),
        _ => ("//", ""),
    }
}

fn type_name(c: &Component) -> &'static str {
    match c {
        Component::Card(_) => "Card",
        Component::Stack(_) => "Stack",
        Component::Grid(_) => "Grid",
        Component::Separator(_) => "Separator",
        Component::Heading(_) => "Heading",
        Component::Text(_) => "Text",
        Component::Badge(_) => "Badge",
        Component::Alert(_) => "Alert",
        Component::Table(_) => "Table",
        Component::KeyValueList(_) => "KeyValueList",
        Component::Metric(_) => "Metric",
        Component::MetricGrid(_) => "MetricGrid",
        Component::Progress(_) => "Progress",
        Component::Bars(_) => "Bars",
        Component::Timeline(_) => "Timeline",
        Component::Checklist(_) => "Checklist",
        Component::Tabs(_) => "Tabs",
        Component::Accordion(_) => "Accordion",
        Component::Graph(_) => "Graph",
        Component::Sequence(_) => "Sequence",
        Component::Steps(_) => "Steps",
        Component::Callout(_) => "Callout",
        Component::Code(_) => "Code",
        Component::Diff(_) => "Diff",
        Component::Excerpt(_) => "Excerpt",
        Component::Tree(_) => "Tree",
        Component::Glossary(_) => "Glossary",
        Component::Quote(_) => "Quote",
        Component::Image(_) => "Image",
        Component::LinkList(_) => "LinkList",
        Component::Tags(_) => "Tags",
        Component::Gauge(_) => "Gauge",
        Component::Sparkline(_) => "Sparkline",
        Component::Heatmap(_) => "Heatmap",
        Component::Person(_) => "Person",
        Component::FileList(_) => "FileList",
        Component::Kanban(_) => "Kanban",
        Component::Calendar(_) => "Calendar",
        // ---- 123 ELK family (W1) ----
        Component::StateMachine(_) => "StateMachine",
        Component::Entities(_) => "Entities",
        Component::Swimlane(_) => "Swimlane",
        Component::GraphDelta(_) => "GraphDelta",
        // ---- 123 chart family (W2) ----
        Component::BarChart(_) => "BarChart",
        Component::LineChart(_) => "LineChart",
        Component::ScatterChart(_) => "ScatterChart",
        Component::RadarChart(_) => "RadarChart",
        Component::Waterfall(_) => "Waterfall",
        Component::Funnel(_) => "Funnel",
        Component::Gantt(_) => "Gantt",
        Component::Quadrant(_) => "Quadrant",
        // ---- 123 part-of-whole + misc (W3) ----
        Component::Treemap(_) => "Treemap",
        Component::Sankey(_) => "Sankey",
        Component::Venn(_) => "Venn",
        Component::Loop(_) => "Loop",
        Component::Layers(_) => "Layers",
        Component::Journey(_) => "Journey",
        Component::Fishbone(_) => "Fishbone",
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    macro_rules! fixture {
        ($name:literal) => {
            include_str!(concat!(
                env!("CARGO_MANIFEST_DIR"),
                "/fixtures/json-render/",
                $name,
                ".json"
            ))
        };
    }

    fn lowered(fixture: &str) -> String {
        lower(&parse(fixture).unwrap().spec)
    }

    #[test]
    fn graph_lowers_to_a_bullet_list_with_edges() {
        let md = lowered(fixture!("graph-architecture"));
        assert!(md.starts_with("Diagram: "), "{md}");
        assert!(md.contains("- **"), "{md}");
        assert!(md.contains(" → "), "{md}");
    }

    #[test]
    fn sequence_lowers_to_numbered_steps() {
        let md = lowered(fixture!("valid-sequence"));
        let p = parse(fixture!("valid-sequence")).unwrap().spec;
        let Component::Sequence(s) = &p.elements[&p.root].component else {
            panic!()
        };
        assert!(md.contains(&format!("{}. ", s.steps.len())), "{md}");
        assert!(md.contains(" → ") && md.contains(": "), "{md}");
    }

    #[test]
    fn excerpt_lowers_to_a_fence_with_a_location_comment() {
        let md = lowered(fixture!("valid-excerpt"));
        let p = parse(fixture!("valid-excerpt")).unwrap().spec;
        let Component::Excerpt(e) = &p.elements[&p.root].component else {
            panic!()
        };
        let start = e.start.unwrap_or(1);
        assert!(
            md.starts_with(&format!("```{}\n", e.lang.as_deref().unwrap_or(""))),
            "{md}"
        );
        assert!(md.contains(&format!(" {}#L{start}-L", e.path)), "{md}");
        assert!(md.contains(e.text.lines().next().unwrap()), "{md}");
    }

    #[test]
    fn lists_tables_and_quotes_lower_to_markdown() {
        let steps = lowered(fixture!("valid-steps"));
        assert!(steps.starts_with("1. **"), "{steps}");
        let table = lowered(fixture!("valid-release-readiness"));
        assert!(table.contains("| --- |"), "{table}");
        let check = lowered(fixture!("valid-status-board"));
        assert!(
            check.contains("- [x] ") || check.contains("- [ ] "),
            "{check}"
        );
        let kv = lowered(fixture!("valid-key-value"));
        assert!(kv.contains("- **") && kv.contains(":** "), "{kv}");
        let tl = lowered(fixture!("valid-timeline"));
        assert!(tl.starts_with("- **"), "{tl}");
        let callout = lowered(fixture!("valid-callout"));
        assert!(callout.starts_with("> "), "{callout}");
        let alerts = lowered(fixture!("valid-alerts"));
        assert!(alerts.contains("> "), "{alerts}");
        let metrics = lowered(fixture!("valid-metrics"));
        assert!(metrics.contains("- **"), "{metrics}");
    }

    #[test]
    fn unknown_components_lower_to_their_type_name() {
        assert_eq!(lowered(fixture!("valid-gauge")).trim(), "*Gauge*");
        assert!(lowered(fixture!("valid-tabs")).contains("**"));
    }

    /// 123 §4.5 (W1): the four ELK-family components have GFM arms, never the `*TypeName*` fallthrough.
    #[test]
    fn elk_family_lowers_to_gfm() {
        let sm = lowered(fixture!("valid-state-machine"));
        assert!(
            sm.contains("- **Doing** → Blocked: wait [dependency open]"),
            "{sm}"
        );
        let en = lowered(fixture!("valid-entities"));
        assert!(
            en.contains("| field | type | key |") && en.contains("| user_id | uuid | fk |"),
            "{en}"
        );
        assert!(
            en.contains("- users → sessions (one-many: id → user_id)"),
            "{en}"
        );
        let sw = lowered(fixture!("valid-swimlane"));
        assert!(
            sw.contains("**CI**\n\n- Build + test (just check)")
                && sw.contains("- Green? → Fix: no"),
            "{sw}"
        );
        let gd = lowered(fixture!("valid-graph-delta"));
        assert!(
            gd.contains(
                "Before: 3 nodes · After: 4 nodes\n\n- **+** Redis\n\nDiagram: 4 nodes, 3 edges"
            ),
            "{gd}"
        );
        for md in [sm, en, sw, gd] {
            assert!(
                !md.contains("*StateMachine*")
                    && !md.contains("*Entities*")
                    && !md.contains("*Swimlane*")
                    && !md.contains("*GraphDelta*")
            );
        }
    }

    #[test]
    fn every_valid_fixture_lowers_to_something() {
        for entry in
            std::fs::read_dir(concat!(env!("CARGO_MANIFEST_DIR"), "/fixtures/json-render")).unwrap()
        {
            let path = entry.unwrap().path();
            let name = path.file_name().unwrap().to_string_lossy().into_owned();
            if !name.starts_with("valid-") && !name.starts_with("graph-") || name.contains("broken")
            {
                continue;
            }
            let md = lowered(&std::fs::read_to_string(&path).unwrap());
            assert!(!md.trim().is_empty(), "{name}");
        }
    }

    const DOC: &str = "\
<!-- a1 by Sky -->
# Title

<!-- b2 -->
See [check](code:crates/surface/src/lib.rs#L150-L164) and [lib.rs](code:crates/surface/src/lib.rs).

<!-- c3 -->
```json-render
{\"root\":\"g\",\"elements\":{\"g\":{\"type\":\"Graph\",\"props\":{\"nodes\":[{\"id\":\"a\",\"label\":\"A\"},{\"id\":\"b\",\"label\":\"B\"}],\"edges\":[{\"from\":\"a\",\"to\":\"b\",\"label\":\"calls\"}]}}}}
```

<!-- d4 -->
```json-render
{\"root\":\"s\",\"elements\":{\"s\":{\"type\":\"Sequence\",\"props\":{\"actors\":[{\"id\":\"x\",\"label\":\"X\"},{\"id\":\"y\",\"label\":\"Y\"}],\"steps\":[{\"from\":\"x\",\"to\":\"y\",\"label\":\"ping\"}]}}}}
```

<!-- e5 -->
```mermaid
graph LR
```
## Two
### Three
";

    #[test]
    fn print_markdown_swaps_assets_in_and_lowers_the_rest() {
        let images = BTreeMap::from([("c3".to_owned(), "/tmp/x/c3.svg")]);
        let out = print_markdown(DOC, &images);
        assert!(!out.contains("<!--"), "{out}");
        assert!(
            out.contains("![](/tmp/x/c3.svg)\n\n*Graph: 2 nodes, 1 edges*"),
            "{out}"
        );
        assert!(out.contains("1. X → Y: ping"), "{out}");
        assert!(out.contains("```mermaid\ngraph LR\n```"), "{out}");
        assert!(out.contains("See check (crates/surface/src/lib.rs#L150-L164) and lib.rs (crates/surface/src/lib.rs)."), "{out}");
        assert!(!out.contains("json-render"), "{out}");
        assert_eq!(heading_count(DOC), 3);
        assert_eq!(heading_count("```\n# not\n```\n# yes\n"), 1);
    }

    #[test]
    fn print_markdown_falls_back_to_text_without_an_asset() {
        let out = print_markdown(DOC, &BTreeMap::<String, &str>::new());
        assert!(
            out.contains("Diagram: 2 nodes, 1 edges\n\n- **A** → B (calls)\n- **B**"),
            "{out}"
        );
    }
}
