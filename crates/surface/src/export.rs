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
use crate::render::num;
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
        // ---- 123 part-of-whole + misc (W3) ----
        Component::Kanban(p) => kanban(p, out),
        Component::Heatmap(p) => heatmap(p, out),
        Component::Treemap(p) => treemap(p, out),
        Component::Sankey(p) => sankey(p, out),
        Component::Venn(p) => venn(p, out),
        Component::Loop(p) => loop_(p, out),
        Component::Layers(p) => layers(p, out),
        Component::Journey(p) => journey(p, out),
        Component::Fishbone(p) => fishbone(p, out),
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

/// Numbered `from → to: label` steps; 123 §4.4 (W3): `*[alt: label]*` / `*[else]*` / `*[end]*`
/// lines around a fragment's steps, `· note` indented under its step, state glyphs.
fn sequence(p: &SequenceProps, out: &mut String) {
    let label_of: BTreeMap<&str, &str> = p
        .actors
        .iter()
        .map(|a| (a.id.as_str(), a.label.as_str()))
        .collect();
    let name = |id: &'_ str| -> String { label_of.get(id).copied().unwrap_or(id).to_owned() };
    let fragments = p.fragments.as_deref().unwrap_or_default();
    if let Some(t) = &p.title {
        let _ = writeln!(out, "**{t}**\n");
    }
    for (i, s) in p.steps.iter().enumerate() {
        for f in fragments {
            if f.from == i {
                let kind = format!("{:?}", f.kind).to_lowercase();
                match f.label.as_deref().filter(|l| !l.is_empty()) {
                    Some(l) => {
                        let _ = writeln!(out, "*[{kind}: {l}]*");
                    }
                    None => {
                        let _ = writeln!(out, "*[{kind}]*");
                    }
                }
            }
            if f.r#else == Some(i) && f.from < i && i <= f.to {
                let _ = writeln!(out, "*[else]*");
            }
        }
        let _ = writeln!(
            out,
            "{}. {} → {}: {}{}",
            i + 1,
            name(&s.from),
            name(&s.to),
            glyph(s.state),
            s.label
        );
        if let Some(n) = s.note.as_deref().filter(|n| !n.is_empty()) {
            let _ = writeln!(out, "   · {n}");
        }
        if fragments.iter().any(|f| f.to == i) {
            let _ = writeln!(out, "*[end]*");
        }
    }
    out.push('\n');
}

// ---- 123 part-of-whole + misc (W3): GFM mirrors of the §4.4 lowerings ----

/// `✗ ` / `● ` / `◉ ` / `○ ` before a unit label with a non-default state; nothing otherwise.
fn glyph(state: Option<GraphState>) -> &'static str {
    match state {
        Some(GraphState::Danger) => "✗ ",
        Some(GraphState::Done) => "● ",
        Some(GraphState::Active) => "◉ ",
        Some(GraphState::Muted) => "○ ",
        _ => "",
    }
}

/// `**Title**` paragraph when the component has one.
fn title(t: Option<&str>, out: &mut String) {
    if let Some(t) = t.filter(|t| !t.is_empty()) {
        let _ = writeln!(out, "**{t}**\n");
    }
}

/// Kanban: `**Column (n/limit ⚠)**` headers with `- ⊘ card` / `- ✓ card` lines; a story map
/// lists `**Row**` sub-headers with `Column: card` lines and a `---` after row `cut`.
fn kanban(p: &KanbanProps, out: &mut String) {
    title(p.title.as_deref(), out);
    let head = |c: &KanbanColumn| -> String {
        let n = c.cards.len();
        match c.limit {
            Some(l) if n > l => format!("{} ({n}/{l} ⚠)", c.title),
            Some(l) => format!("{} ({n}/{l})", c.title),
            None => format!("{} ({n})", c.title),
        }
    };
    let card = |c: &KanbanCard| -> String {
        let mark = match c.state {
            Some(KanbanState::Blocked) => "⊘ ",
            Some(KanbanState::Done) => "✓ ",
            _ => "",
        };
        let mut s = format!("{mark}{}", c.title);
        if let Some(t) = &c.tag {
            let _ = write!(s, " `{t}`");
        }
        if let Some(d) = &c.detail {
            let _ = write!(s, " — {d}");
        }
        s
    };
    match p.rows.as_deref() {
        None => {
            for c in &p.columns {
                let _ = writeln!(out, "**{}**\n", head(c));
                for k in &c.cards {
                    let _ = writeln!(out, "- {}", card(k));
                }
                out.push('\n');
            }
        }
        Some(rows) => {
            let _ = writeln!(
                out,
                "{}\n",
                p.columns.iter().map(head).collect::<Vec<_>>().join(" · ")
            );
            for r in 0..=rows.len() {
                let cards: Vec<(&KanbanColumn, &KanbanCard)> = p
                    .columns
                    .iter()
                    .flat_map(|c| c.cards.iter().map(move |k| (c, k)))
                    .filter(|(_, k)| k.row.map_or(rows.len(), |i| i.min(rows.len())) == r)
                    .collect();
                if cards.is_empty() {
                    continue;
                }
                let _ = writeln!(
                    out,
                    "**{}**\n",
                    rows.get(r).map_or("(no row)", String::as_str)
                );
                for (c, k) in cards {
                    let _ = writeln!(out, "- {}: {}", c.title, card(k));
                }
                out.push('\n');
                if p.cut == Some(r) {
                    out.push_str("---\n\n");
                }
            }
        }
    }
}

/// Heatmap: a GFM table of the values; `diverging` prints them signed.
fn heatmap(p: &HeatmapProps, out: &mut String) {
    title(p.title.as_deref(), out);
    let diverging = p.palette == Some(HeatmapPalette::Diverging);
    let columns: Vec<String> = std::iter::once(String::new())
        .chain(p.cols.iter().cloned())
        .collect();
    let rows: Vec<Vec<String>> = p
        .rows
        .iter()
        .zip(&p.cells)
        .map(|(r, cs)| {
            std::iter::once(format!("**{r}**"))
                .chain(cs.iter().map(|v| {
                    if diverging && *v > 0.0 {
                        format!("+{}", num(*v))
                    } else {
                        num(*v)
                    }
                }))
                .collect()
        })
        .collect();
    table(&columns, &rows, out);
}

/// Treemap: `| Cell | Value | Share |` descending, the group as a `**Group**` row when any.
fn treemap(p: &TreemapProps, out: &mut String) {
    title(p.title.as_deref(), out);
    let total: f64 = p.items.iter().map(|c| c.value).sum();
    let share = |v: f64| {
        if total > 0.0 {
            format!("{}%", (v / total * 100.0).round() as i64)
        } else {
            "–".to_owned()
        }
    };
    let mut groups: Vec<(Option<&str>, Vec<&TreemapCell>)> = Vec::new();
    for c in &p.items {
        let key = c.group.as_deref();
        match groups.iter_mut().find(|(k, _)| *k == key) {
            Some((_, cells)) => cells.push(c),
            None => groups.push((key, vec![c])),
        }
    }
    let sum = |cells: &[&TreemapCell]| cells.iter().map(|c| c.value).sum::<f64>();
    groups.sort_by(|a, b| {
        b.0.is_some()
            .cmp(&a.0.is_some())
            .then(sum(&b.1).total_cmp(&sum(&a.1)))
    });
    let headed = groups.iter().any(|(k, _)| k.is_some());
    let mut rows = Vec::new();
    for (key, mut cells) in groups {
        cells.sort_by(|a, b| b.value.total_cmp(&a.value));
        if headed {
            let t = sum(&cells);
            rows.push(vec![
                format!("**{}**", key.unwrap_or("(ungrouped)")),
                num(t),
                share(t),
            ]);
        }
        for c in cells {
            let indent = if headed { "· " } else { "" };
            rows.push(vec![
                format!("{indent}{}{}", glyph(c.state), c.label),
                num(c.value),
                share(c.value),
            ]);
        }
    }
    table(
        &["Cell".to_owned(), "Value".to_owned(), "Share".to_owned()],
        &rows,
        out,
    );
}

/// Sankey: `- **Node** (total unit)` per node in authored order with `  - → To: value` lines.
fn sankey(p: &SankeyProps, out: &mut String) {
    title(p.title.as_deref(), out);
    let unit = p
        .unit
        .as_deref()
        .map(|u| format!(" {u}"))
        .unwrap_or_default();
    let label = |id: &str| -> String {
        p.nodes
            .iter()
            .find(|n| n.id == id)
            .map(|n| n.label.clone())
            .unwrap_or_else(|| id.to_owned())
    };
    for n in &p.nodes {
        let inflow: f64 = p
            .flows
            .iter()
            .filter(|f| f.to == n.id)
            .map(|f| f.value)
            .sum();
        let outflow: f64 = p
            .flows
            .iter()
            .filter(|f| f.from == n.id)
            .map(|f| f.value)
            .sum();
        let _ = writeln!(
            out,
            "- **{}{}** ({}{unit})",
            glyph(n.state),
            n.label,
            num(inflow.max(outflow))
        );
        for f in p.flows.iter().filter(|f| f.from == n.id) {
            let _ = write!(out, "  - → {}: {}{unit}", label(&f.to), num(f.value));
            if let Some(l) = f.label.as_deref().filter(|l| !l.is_empty()) {
                let _ = write!(out, " ({l})");
            }
            out.push('\n');
        }
    }
    out.push('\n');
}

/// Venn: `- **Set** (size)` per set, then `- A ∩ B: label` per overlap.
fn venn(p: &VennProps, out: &mut String) {
    title(p.title.as_deref(), out);
    let label = |id: &str| -> String {
        p.sets
            .iter()
            .find(|s| s.id == id)
            .map(|s| s.label.clone())
            .unwrap_or_else(|| id.to_owned())
    };
    for s in &p.sets {
        let _ = write!(out, "- **{}**", s.label);
        if let Some(n) = s.size {
            let _ = write!(out, " ({})", num(n));
        }
        out.push('\n');
    }
    for o in p.overlaps.as_deref().unwrap_or_default() {
        let names: Vec<String> = o.sets.iter().map(|id| label(id)).collect();
        let _ = writeln!(out, "- {}: {}", names.join(" ∩ "), o.label);
    }
    out.push('\n');
}

/// Loop: `1. a → 2. b → … ↺ 1.` on one line, the hub in italics, notes as a list.
fn loop_(p: &LoopProps, out: &mut String) {
    title(p.title.as_deref(), out);
    let chain: Vec<String> = p
        .steps
        .iter()
        .enumerate()
        .map(|(i, s)| format!("{}. {}{}", i + 1, glyph(s.state), s.label))
        .collect();
    if !chain.is_empty() {
        let _ = writeln!(out, "{} → ↺ 1.\n", chain.join(" → "));
    }
    if let Some(h) = p.hub.as_deref().filter(|h| !h.is_empty()) {
        let _ = writeln!(out, "*⟳ {h}*\n");
    }
    let notes: Vec<String> = p
        .steps
        .iter()
        .enumerate()
        .filter_map(|(i, s)| s.note.as_deref().map(|n| format!("- {}. {n}", i + 1)))
        .collect();
    if !notes.is_empty() {
        let _ = writeln!(out, "{}\n", notes.join("\n"));
    }
}

/// Layers: `- **Layer**: items — *note*` in reading order (`bottom-up` reversed).
fn layers(p: &LayersProps, out: &mut String) {
    title(p.title.as_deref(), out);
    let mut layers: Vec<&Layer> = p.layers.iter().collect();
    if p.direction == Some(LayersDirection::BottomUp) {
        layers.reverse();
    }
    for l in layers {
        let _ = write!(out, "- **{}{}**", glyph(l.state), l.label);
        let items = l.items.as_deref().unwrap_or_default();
        if !items.is_empty() {
            let _ = write!(out, ": {}", items.join(", "));
        }
        if let Some(n) = l.note.as_deref().filter(|n| !n.is_empty()) {
            let _ = write!(out, " — *{n}*");
        }
        out.push('\n');
    }
    out.push('\n');
}

/// Journey: a stage × row GFM table with a `Mood` row (`☹ ○ ☺`), pains as `- ▲ stage: text`.
fn journey(p: &JourneyProps, out: &mut String) {
    title(p.title.as_deref(), out);
    let columns: Vec<String> = std::iter::once(String::new())
        .chain(p.stages.iter().cloned())
        .collect();
    let mut rows: Vec<Vec<String>> = p
        .rows
        .iter()
        .map(|r| {
            std::iter::once(format!("**{}**", r.label))
                .chain((0..p.stages.len()).map(|i| r.cells.get(i).cloned().unwrap_or_default()))
                .collect()
        })
        .collect();
    if let Some(m) = &p.mood {
        rows.push(
            std::iter::once("**Mood**".to_owned())
                .chain((0..p.stages.len()).map(|i| {
                    m.get(i)
                        .map(|v| {
                            if *v < -1.0 / 3.0 {
                                "☹"
                            } else if *v > 1.0 / 3.0 {
                                "☺"
                            } else {
                                "○"
                            }
                        })
                        .unwrap_or("")
                        .to_owned()
                }))
                .collect(),
        );
    }
    table(&columns, &rows, out);
    let pains = p.pains.as_deref().unwrap_or_default();
    for pain in pains {
        let stage = p.stages.get(pain.stage).map_or("?", String::as_str);
        let _ = writeln!(out, "- ▲ {stage}: {}", pain.text);
    }
    if !pains.is_empty() {
        out.push('\n');
    }
}

/// Fishbone: `**→ effect**`, then `- **Category**: items`.
fn fishbone(p: &FishboneProps, out: &mut String) {
    title(p.title.as_deref(), out);
    let _ = writeln!(out, "**→ {}**\n", p.effect);
    for c in &p.causes {
        let _ = writeln!(out, "- **{}**: {}", c.category, c.items.join(", "));
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

    // ---- 123 part-of-whole + misc (W3) ----

    /// §4.5: no W3 diagram fixture falls through to the `*TypeName*` arm.
    #[test]
    fn w3_fixtures_lower_to_gfm_not_their_type_name() {
        for (name, md) in [
            ("Treemap", lowered(fixture!("valid-treemap"))),
            ("Sankey", lowered(fixture!("valid-sankey"))),
            ("Venn", lowered(fixture!("valid-venn"))),
            ("Loop", lowered(fixture!("valid-loop"))),
            ("Layers", lowered(fixture!("valid-layers"))),
            ("Journey", lowered(fixture!("valid-journey"))),
            ("Fishbone", lowered(fixture!("valid-fishbone"))),
            ("Kanban", lowered(fixture!("valid-kanban-rows"))),
            ("Heatmap", lowered(fixture!("valid-heatmap-diverging"))),
        ] {
            assert!(!md.contains(&format!("*{name}*")), "{name}:\n{md}");
            assert!(md.starts_with("**"), "{name} opens with its title:\n{md}");
        }
        let treemap = lowered(fixture!("valid-treemap"));
        assert!(treemap.contains("| Cell | Value | Share |\n| --- | --- | --- |\n| **crates** | 53400 | 64% |\n| · ◉ agent | 36200 | 44% |"), "{treemap}");
        let sankey = lowered(fixture!("valid-sankey"));
        assert!(
            sankey.contains("- **Opus** (16.7 USD)\n  - → Design 123: 12.5 USD\n"),
            "{sankey}"
        );
        assert!(sankey.contains("- → Docs: 2.4 USD (drafts)"), "{sankey}");
        let venn = lowered(fixture!("valid-venn"));
        assert!(
            venn.contains("- **Has tests** (41)\n")
                && venn.contains("- Has tests ∩ Has docs: Documented + tested\n"),
            "{venn}"
        );
        let lp = lowered(fixture!("valid-loop"));
        assert!(lp.contains("1. Measure → 2. Decide → 3. ◉ Build → 4. Ship → 5. ○ Observe → ↺ 1.\n\n*⟳ Learning*\n\n- 1. 20 prompts"), "{lp}");
        let layers = lowered(fixture!("valid-layers"));
        assert!(layers.contains("- **Surfaces**: desktop, TUI, Slack, Signal\n- **◉ Agent**: loop, tools, skills\n- **Surface crate**: parse, validate, render — *one catalog*\n"), "{layers}");
        let journey = lowered(fixture!("valid-journey"));
        assert!(
            journey.contains("|  | Install | First chat | First diagram | First board |\n"),
            "{journey}"
        );
        assert!(
            journey.contains("| **Mood** | ○ | ☺ | ☺ | ☺ |\n\n- ▲ Install: notarisation wait\n"),
            "{journey}"
        );
        let fish = lowered(fixture!("valid-fishbone"));
        assert!(
            fish.contains("**→ Slow replies**\n\n- **Model**: long context, cold cache\n"),
            "{fish}"
        );
        let kanban = lowered(fixture!("valid-kanban-rows"));
        assert!(
            kanban.contains(
                "Onboard (3/2 ⚠) · Chat (3/2 ⚠) · Boards (1)\n\n**Must**\n\n- Onboard: ✓ Install\n"
            ),
            "{kanban}"
        );
        assert!(
            kanban.contains("- Chat: ⊘ Draw a chart\n- Boards: Open a board\n\n---\n\n**Could**"),
            "{kanban}"
        );
        let plain = lowered(fixture!("valid-kanban"));
        assert!(plain.starts_with("**") && plain.contains("- "), "{plain}");
        let heat = lowered(fixture!("valid-heatmap-diverging"));
        assert!(heat.contains("| **chat** | -12 | -4 | +3 | +8 |"), "{heat}");
        assert!(!lowered(fixture!("valid-heatmap")).contains('+'));
    }

    /// §4.4 Sequence ext.: fragment frames, notes and state glyphs in the GFM form.
    #[test]
    fn sequence_fragments_lower_to_framed_steps() {
        let md = lowered(fixture!("valid-sequence-fragments"));
        assert!(md.contains("*[alt: errors]*\n3. surface → Loop: ✗ located errors\n   · path + message\n4. Loop → Model: <surface_error>\n*[else]*\n5. Model → Loop: ● corrected block\n*[end]*\n*[loop: ≤ 3 rounds]*\n6. Loop → Loop: give up after 3\n   · final=true\n*[end]*"), "{md}");
    }

    /// The Sankey validator refuses cycles and self-flows before any lowering (d3-sankey on the
    /// desktop would throw on a circular link; the Rust check is the one that names it).
    #[test]
    fn sankey_cycle_and_self_flow_fixtures_are_refused() {
        let cycle = parse(fixture!("broken-sankey-cycle")).expect_err("cycle refused");
        assert!(
            cycle
                .iter()
                .any(|e| e.path == "/elements/s/props/flows/4"
                    && e.message.starts_with("circular flow")),
            "{cycle:?}"
        );
        let self_flow = parse(fixture!("broken-sankey-self-flow")).expect_err("self flow refused");
        assert!(
            self_flow
                .iter()
                .any(|e| e.path == "/elements/s/props/flows/4/to"),
            "{self_flow:?}"
        );
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