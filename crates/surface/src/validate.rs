//! Located validation of a parsed fence body: every error is collected, none stops the walk.

use std::collections::{BTreeMap, BTreeSet};

use serde_json::{Map, Value, json};

use crate::anchor;
use crate::catalog::catalog;
use crate::spec::*;
use crate::{
    LOOP_STEPS, MAX_BAR_LABELS, MAX_CHART_SERIES, MAX_ENTITIES, MAX_ENTITY_FIELDS,
    MAX_FISHBONE_CAUSES, MAX_FISHBONE_ITEMS, MAX_FUNNEL_STAGES, MAX_GANTT_TASKS,
    MAX_GRAPH_GROUP_DEPTH, MAX_GRAPH_LEGEND, MAX_JOURNEY_ROWS, MAX_JOURNEY_STAGES, MAX_LANE_EDGES,
    MAX_LANE_STEPS, MAX_LANES, MAX_LAYERS, MAX_LINE_LABELS, MAX_QUADRANT_ITEMS, MAX_RADAR_SERIES,
    MAX_RELATIONS, MAX_SANKEY_FLOWS, MAX_SANKEY_NODES, MAX_SCATTER_GROUPS, MAX_SCATTER_POINTS,
    MAX_SEQUENCE_FRAGMENTS, MAX_SEQUENCE_NOTES, MAX_STATES, MAX_SUB_CHARS, MAX_TAG_CHARS,
    MAX_TRANSITIONS, MAX_TREEMAP_GROUPS, MAX_TREEMAP_ITEMS, MAX_WATERFALL_STEPS, RADAR_AXES,
    VENN_SETS,
};
use crate::{
    MAX_DEPTH, MAX_ELEMENTS, MAX_EXCERPT_BYTES, MAX_EXCERPT_LINES, MAX_GRAPH_EDGES,
    MAX_GRAPH_GROUPS, MAX_GRAPH_LABEL_CHARS, MAX_GRAPH_NODES, SEQUENCE_ACTORS, SEQUENCE_STEPS,
    SpecError,
};

/// Components whose `children` may be non-empty.
pub(crate) const CONTAINERS: [&str; 5] = ["Card", "Stack", "Grid", "Tabs", "Accordion"];

pub(crate) fn err(path: &str, message: impl Into<String>) -> SpecError {
    SpecError {
        path: path.to_owned(),
        message: message.into(),
    }
}

/// JSON Pointer escaping of one reference token.
fn esc(token: &str) -> String {
    token.replace('~', "~0").replace('/', "~1")
}

/// Validate a JSON value as a [`Spec`]; `Err` carries every problem found.
pub(crate) fn validate(v: &Value) -> Result<Spec, Vec<SpecError>> {
    let mut errs = Vec::new();
    let Some(top) = v.as_object() else {
        return Err(vec![err(
            "",
            "expected an object with \"root\" and \"elements\"",
        )]);
    };
    for k in top.keys().filter(|k| *k != "root" && *k != "elements") {
        errs.push(err(
            &format!("/{}", esc(k)),
            "unknown key; only \"root\" and \"elements\" are allowed",
        ));
    }
    let root = match top.get("root") {
        Some(Value::String(s)) => Some(s.as_str()),
        Some(_) => {
            errs.push(err("/root", "expected string"));
            None
        }
        None => {
            errs.push(err("/root", "missing"));
            None
        }
    };
    let raw = match top.get("elements") {
        Some(Value::Object(m)) => m,
        Some(_) => {
            errs.push(err("/elements", "expected an object of elements by id"));
            return Err(errs);
        }
        None => {
            errs.push(err("/elements", "missing"));
            return Err(errs);
        }
    };
    if raw.len() > MAX_ELEMENTS {
        errs.push(err(
            "/",
            format!("{} elements, limit {MAX_ELEMENTS}", raw.len()),
        ));
    }

    let empty = Value::Object(Map::new());
    let mut elements = BTreeMap::new();
    let mut tree: BTreeMap<&str, Vec<&str>> = BTreeMap::new();
    for (id, el) in raw {
        let p = format!("/elements/{}", esc(id));
        let Some(obj) = el.as_object() else {
            errs.push(err(
                &p,
                "expected an object with \"type\", \"props\", \"children\"",
            ));
            continue;
        };
        for k in obj
            .keys()
            .filter(|k| !matches!(k.as_str(), "type" | "props" | "children"))
        {
            errs.push(err(
                &format!("{p}/{}", esc(k)),
                "unknown key; only \"type\", \"props\" and \"children\" are allowed",
            ));
        }
        let name = match obj.get("type") {
            Some(Value::String(s)) => Some(s.as_str()),
            Some(_) => {
                errs.push(err(&format!("{p}/type"), "expected string"));
                None
            }
            None => {
                errs.push(err(&format!("{p}/type"), "missing"));
                None
            }
        };
        let entry = name.and_then(|n| {
            let e = catalog().entry(n);
            if e.is_none() {
                errs.push(err(
                    &format!("{p}/type"),
                    format!("unknown component {n:?}; catalog: {}", catalog().names),
                ));
            }
            e
        });

        let mut kids = Vec::new();
        let mut n_children = 0;
        match obj.get("children") {
            None => {}
            Some(Value::Array(a)) => {
                n_children = a.len();
                if let Some(n) = name
                    && entry.is_some()
                    && !a.is_empty()
                    && !CONTAINERS.contains(&n)
                {
                    errs.push(err(
                        &format!("{p}/children"),
                        format!("{n} cannot have children"),
                    ));
                }
                for (i, c) in a.iter().enumerate() {
                    match c.as_str() {
                        Some(cid) if raw.contains_key(cid) => kids.push(cid),
                        Some(cid) => errs.push(err(
                            &format!("{p}/children/{i}"),
                            format!("{cid:?} is not an element id"),
                        )),
                        None => errs.push(err(&format!("{p}/children/{i}"), "expected string")),
                    }
                }
            }
            Some(_) => errs.push(err(
                &format!("{p}/children"),
                "expected an array of element ids",
            )),
        }
        let children: Vec<String> = kids.iter().map(|c| (*c).to_owned()).collect();
        tree.insert(id.as_str(), kids);

        let Some(entry) = entry else { continue };
        let props = obj.get("props").unwrap_or(&empty);
        let before = errs.len();
        walk(props, &entry.props, &format!("{p}/props"), &mut errs);
        if errs.len() != before {
            continue;
        }
        let component: Component =
            match serde_json::from_value(json!({ "type": entry.name, "props": props })) {
                Ok(c) => c,
                Err(e) => {
                    errs.push(err(&format!("{p}/props"), e.to_string()));
                    continue;
                }
            };
        match &component {
            Component::Table(t) => {
                for (i, row) in t
                    .rows
                    .iter()
                    .enumerate()
                    .filter(|(_, r)| r.len() != t.columns.len())
                {
                    errs.push(err(
                        &format!("{p}/props/rows/{i}"),
                        format!("{} cells, {} columns", row.len(), t.columns.len()),
                    ));
                }
            }
            Component::Tabs(t) if t.labels.len() != n_children => {
                errs.push(err(
                    &format!("{p}/props/labels"),
                    format!("{} labels, {n_children} children", t.labels.len()),
                ));
            }
            Component::Accordion(a) if a.titles.len() != n_children => {
                errs.push(err(
                    &format!("{p}/props/titles"),
                    format!("{} titles, {n_children} children", a.titles.len()),
                ));
            }
            Component::Graph(g) => graph(g, &format!("{p}/props"), &mut errs),
            Component::Sequence(s) => sequence(s, &format!("{p}/props"), &mut errs),
            Component::Excerpt(x) => excerpt(x, &format!("{p}/props"), &mut errs),
            Component::Timeline(t) => timeline(t, &format!("{p}/props"), &mut errs),
            Component::Tree(t) => tree_props(t, &format!("{p}/props"), &mut errs),
            Component::Kanban(k) => kanban(k, &format!("{p}/props"), &mut errs),
            // ---- 123 ELK family (W1) ----
            Component::StateMachine(m) => state_machine(m, &format!("{p}/props"), &mut errs),
            Component::Entities(e) => entities(e, &format!("{p}/props"), &mut errs),
            Component::Swimlane(s) => swimlane(s, &format!("{p}/props"), &mut errs),
            Component::GraphDelta(d) => {
                let p = format!("{p}/props");
                title(&p, d.title.as_deref(), &mut errs);
                for (side, g) in [("before", &d.before), ("after", &d.after)] {
                    graph_side(
                        &g.nodes,
                        &g.edges,
                        g.groups.as_deref(),
                        &format!("{p}/{side}"),
                        &mut errs,
                    );
                }
            }
            // ---- 123 chart family (W2) ----
            Component::BarChart(c) => bar_chart(c, &format!("{p}/props"), &mut errs),
            Component::LineChart(c) => line_chart(c, &format!("{p}/props"), &mut errs),
            Component::ScatterChart(c) => scatter_chart(c, &format!("{p}/props"), &mut errs),
            Component::RadarChart(c) => radar_chart(c, &format!("{p}/props"), &mut errs),
            Component::Waterfall(w) => waterfall(w, &format!("{p}/props"), &mut errs),
            Component::Funnel(f) => funnel(f, &format!("{p}/props"), &mut errs),
            Component::Gantt(g) => gantt(g, &format!("{p}/props"), &mut errs),
            Component::Quadrant(q) => quadrant(q, &format!("{p}/props"), &mut errs),
            // ---- 123 part-of-whole + misc (W3) ----
            Component::Treemap(t) => treemap(t, &format!("{p}/props"), &mut errs),
            Component::Sankey(s) => sankey(s, &format!("{p}/props"), &mut errs),
            Component::Venn(v) => venn(v, &format!("{p}/props"), &mut errs),
            Component::Loop(l) => loop_(l, &format!("{p}/props"), &mut errs),
            Component::Layers(l) => layers(l, &format!("{p}/props"), &mut errs),
            Component::Journey(j) => journey(j, &format!("{p}/props"), &mut errs),
            Component::Fishbone(f) => fishbone(f, &format!("{p}/props"), &mut errs),
            Component::Heatmap(h) => {
                if h.cells.len() != h.rows.len() {
                    errs.push(err(
                        &format!("{p}/props/cells"),
                        format!("{} rows of cells, {} rows", h.cells.len(), h.rows.len()),
                    ));
                }
                for (i, row) in h
                    .cells
                    .iter()
                    .enumerate()
                    .filter(|(_, r)| r.len() != h.cols.len())
                {
                    errs.push(err(
                        &format!("{p}/props/cells/{i}"),
                        format!("{} cells, {} columns", row.len(), h.cols.len()),
                    ));
                }
            }
            Component::Image(i) if !is_image_src(&i.src) => {
                errs.push(err(
                    &format!("{p}/props/src"),
                    "src must be http(s) or data:image",
                ));
            }
            Component::LinkList(l) => {
                for (i, _) in l.items.iter().enumerate().filter(|(_, l)| !is_http(&l.url)) {
                    errs.push(err(
                        &format!("{p}/props/items/{i}/url"),
                        "url must be http(s)",
                    ));
                }
            }
            _ => {}
        }
        elements.insert(
            id.clone(),
            Element {
                component,
                children,
            },
        );
    }

    let mut parent: BTreeMap<&str, &str> = BTreeMap::new();
    for (pid, kids) in &tree {
        for k in kids {
            if let Some(first) = parent.insert(*k, *pid) {
                errs.push(err(
                    "/",
                    format!("element {k:?} has two parents ({first}, {pid})"),
                ));
            }
        }
    }
    if let Some(root) = root {
        if raw.contains_key(root) {
            let mut seen = BTreeSet::new();
            reach(root, &tree, &mut seen, &mut Vec::new(), &mut errs);
            for id in raw.keys().filter(|id| !seen.contains(id.as_str())) {
                errs.push(err(
                    &format!("/elements/{}", esc(id)),
                    "not reachable from root",
                ));
            }
        } else {
            errs.push(err("/root", format!("{root:?} is not an element id")));
        }
    }

    match root {
        Some(root) if errs.is_empty() => Ok(Spec {
            root: root.to_owned(),
            elements,
        }),
        _ => Err(errs),
    }
}

/// `http://` or `https://`, case-insensitive.
fn is_http(url: &str) -> bool {
    let l = url.get(..8).unwrap_or(url).to_ascii_lowercase();
    l.starts_with("http://") || l.starts_with("https://")
}

/// [`is_http`] or an inline `data:image/…` URL: what `<img src>` may load.
fn is_image_src(src: &str) -> bool {
    is_http(src)
        || src
            .get(..11)
            .unwrap_or(src)
            .eq_ignore_ascii_case("data:image/")
}

/// A `Graph` or `Sequence` label: ≤ [`MAX_GRAPH_LABEL_CHARS`].
fn label(path: String, text: &str, errs: &mut Vec<SpecError>) {
    let n = text.chars().count();
    if n > MAX_GRAPH_LABEL_CHARS {
        errs.push(err(
            &path,
            format!("{n} characters, limit {MAX_GRAPH_LABEL_CHARS}"),
        ));
    }
}

/// A `ref` (71 §2.3): an anchor string, or a located error.
fn anchor_ref(path: String, r: Option<&str>, errs: &mut Vec<SpecError>) {
    if let Some(r) = r
        && let Err(msg) = anchor::parse(r)
    {
        errs.push(err(&path, msg));
    }
}

/// `n > max` → `{n} {unit}, limit {max}` at `path`.
fn cap(path: String, n: usize, max: usize, unit: &str, errs: &mut Vec<SpecError>) {
    if n > max {
        errs.push(err(&path, format!("{n} {unit}, limit {max}")));
    }
}

/// `n` outside `r` → `{n} {unit}, expected a..=b` at `path`.
fn within(
    path: String,
    n: usize,
    r: std::ops::RangeInclusive<usize>,
    unit: &str,
    errs: &mut Vec<SpecError>,
) {
    if !r.contains(&n) {
        errs.push(err(
            &path,
            format!("{n} {unit}, expected {}..={}", r.start(), r.end()),
        ));
    }
}

/// A text ≤ `max` characters (`sub`, `tag`, `guard`).
fn short(path: String, text: Option<&str>, max: usize, errs: &mut Vec<SpecError>) {
    if let Some(t) = text {
        let n = t.chars().count();
        if n > max {
            errs.push(err(&path, format!("{n} characters, limit {max}")));
        }
    }
}

/// An optional `title` ≤ [`MAX_GRAPH_LABEL_CHARS`].
fn title(p: &str, text: Option<&str>, errs: &mut Vec<SpecError>) {
    if let Some(t) = text {
        label(format!("{p}/title"), t, errs);
    }
}

/// Collect `ids` as a set; a repeat is `duplicate {what} id` at `{p}/{i}/id`.
fn unique<'a>(
    p: &str,
    ids: impl Iterator<Item = &'a str>,
    what: &str,
    errs: &mut Vec<SpecError>,
) -> BTreeSet<&'a str> {
    let mut set = BTreeSet::new();
    for (i, id) in ids.enumerate() {
        if !set.insert(id) {
            errs.push(err(
                &format!("{p}/{i}/id"),
                format!("duplicate {what} id {id:?}"),
            ));
        }
    }
    set
}

/// `id ∉ set` → `unknown {what} "id"` at `path`.
fn resolves(path: String, id: &str, set: &BTreeSet<&str>, what: &str, errs: &mut Vec<SpecError>) {
    if !set.contains(id) {
        errs.push(err(&path, format!("unknown {what} {id:?}")));
    }
}

/// An ISO date `YYYY-MM-DD`, optionally `T` (or a space) then `hh:mm[:ss[.frac]][Z|±hh:mm]`
/// (RFC 3339), as a comparable `[y, m, d, h, min, s]`. `None` when it is not one.
// ponytail: the offset is validated, not applied — two datetimes compare in their own zones.
pub(crate) fn iso_datetime(s: &str) -> Option<[u32; 6]> {
    let b = s.as_bytes();
    let num = |r: std::ops::Range<usize>| -> Option<u32> {
        let t = b.get(r)?;
        if t.iter().all(u8::is_ascii_digit) {
            std::str::from_utf8(t).ok()?.parse().ok()
        } else {
            None
        }
    };
    if b.len() < 10 || b[4] != b'-' || b[7] != b'-' {
        return None;
    }
    let (y, m, d) = (num(0..4)?, num(5..7)?, num(8..10)?);
    let days = match m {
        1 | 3 | 5 | 7 | 8 | 10 | 12 => 31,
        4 | 6 | 9 | 11 => 30,
        2 if y % 4 == 0 && (y % 100 != 0 || y % 400 == 0) => 29,
        2 => 28,
        _ => return None,
    };
    if d == 0 || d > days {
        return None;
    }
    if b.len() == 10 {
        return Some([y, m, d, 0, 0, 0]);
    }
    if !matches!(b[10], b'T' | b't' | b' ') || b.len() < 16 || b[13] != b':' {
        return None;
    }
    let (h, min) = (num(11..13)?, num(14..16)?);
    let mut i = 16;
    let mut sec = 0;
    if b.get(i) == Some(&b':') {
        sec = num(17..19)?;
        i = 19;
        if b.get(i) == Some(&b'.') {
            i += 1;
            let start = i;
            while b.get(i).is_some_and(u8::is_ascii_digit) {
                i += 1;
            }
            if i == start {
                return None;
            }
        }
    }
    match b.get(i) {
        None => {}
        Some(b'Z' | b'z') if i + 1 == b.len() => {}
        Some(b'+' | b'-') if i + 6 == b.len() && b[i + 3] == b':' => {
            if num(i + 1..i + 3)? > 23 || num(i + 4..i + 6)? > 59 {
                return None;
            }
        }
        _ => return None,
    }
    (h < 24 && min < 60 && sec < 60).then_some([y, m, d, h, min, sec])
}

/// [`iso_datetime`] or a located error.
fn iso(path: String, s: &str, errs: &mut Vec<SpecError>) -> Option<[u32; 6]> {
    let t = iso_datetime(s);
    if t.is_none() {
        errs.push(err(
            &path,
            format!("{s:?} is not an ISO date (YYYY-MM-DD) or datetime (RFC 3339)"),
        ));
    }
    t
}

/// `Graph` checks serde cannot express (65 §3.2, 123 §4.1): the shared [`graph_side`] checks,
/// then the legend cap and labels.
fn graph(g: &GraphProps, p: &str, errs: &mut Vec<SpecError>) {
    title(p, g.title.as_deref(), errs);
    graph_side(&g.nodes, &g.edges, g.groups.as_deref(), p, errs);
    let legend = g.legend.as_deref().unwrap_or_default();
    cap(
        format!("{p}/legend"),
        legend.len(),
        MAX_GRAPH_LEGEND,
        "legend entries",
        errs,
    );
    for (i, l) in legend.iter().enumerate() {
        label(format!("{p}/legend/{i}/label"), &l.label, errs);
    }
}

/// One graph (a `Graph` or a `GraphDelta` side): unique node and group ids, edge endpoints and
/// node groups that exist, group `parent` resolving without a cycle and within
/// [`MAX_GRAPH_GROUP_DEPTH`], the caps, label/`sub`/`tag` lengths and the `ref` grammar.
/// Self-loops are fine.
fn graph_side(
    nodes: &[GraphNode],
    edges: &[GraphEdge],
    groups: Option<&[GraphGroup]>,
    p: &str,
    errs: &mut Vec<SpecError>,
) {
    cap(
        format!("{p}/nodes"),
        nodes.len(),
        MAX_GRAPH_NODES,
        "nodes",
        errs,
    );
    cap(
        format!("{p}/edges"),
        edges.len(),
        MAX_GRAPH_EDGES,
        "edges",
        errs,
    );
    let groups = groups.unwrap_or_default();
    cap(
        format!("{p}/groups"),
        groups.len(),
        MAX_GRAPH_GROUPS,
        "groups",
        errs,
    );
    let group_ids = unique(
        &format!("{p}/groups"),
        groups.iter().map(|g| g.id.as_str()),
        "group",
        errs,
    );
    let parent_of: BTreeMap<&str, &str> = groups
        .iter()
        .filter_map(|g| Some((g.id.as_str(), g.parent.as_deref()?)))
        .collect();
    for (i, grp) in groups.iter().enumerate() {
        label(format!("{p}/groups/{i}/label"), &grp.label, errs);
        let Some(parent) = grp.parent.as_deref() else {
            continue;
        };
        let path = format!("{p}/groups/{i}/parent");
        if !group_ids.contains(parent) {
            errs.push(err(&path, format!("unknown group {parent:?}")));
            continue;
        }
        // Walk up: a return to `grp.id` is a cycle; a chain past the depth cap is too deep.
        let mut depth = 1;
        let mut cur = parent;
        loop {
            depth += 1;
            if cur == grp.id {
                errs.push(err(&path, format!("group {:?} contains itself", grp.id)));
                break;
            }
            if depth > MAX_GRAPH_GROUP_DEPTH {
                errs.push(err(
                    &path,
                    format!("nesting depth {depth}, limit {MAX_GRAPH_GROUP_DEPTH}"),
                ));
                break;
            }
            match parent_of.get(cur) {
                Some(next) => cur = next,
                None => break,
            }
        }
    }
    let node_ids = unique(
        &format!("{p}/nodes"),
        nodes.iter().map(|n| n.id.as_str()),
        "node",
        errs,
    );
    for (i, n) in nodes.iter().enumerate() {
        if let Some(grp) = &n.group {
            resolves(
                format!("{p}/nodes/{i}/group"),
                grp,
                &group_ids,
                "group",
                errs,
            );
        }
        label(format!("{p}/nodes/{i}/label"), &n.label, errs);
        short(
            format!("{p}/nodes/{i}/sub"),
            n.sub.as_deref(),
            MAX_SUB_CHARS,
            errs,
        );
        short(
            format!("{p}/nodes/{i}/tag"),
            n.tag.as_deref(),
            MAX_TAG_CHARS,
            errs,
        );
        anchor_ref(format!("{p}/nodes/{i}/ref"), n.r#ref.as_deref(), errs);
    }
    for (i, e) in edges.iter().enumerate() {
        for (end, id) in [("from", &e.from), ("to", &e.to)] {
            resolves(format!("{p}/edges/{i}/{end}"), id, &node_ids, "node", errs);
        }
        if let Some(l) = &e.label {
            label(format!("{p}/edges/{i}/label"), l, errs);
        }
    }
}

/// `Sequence` checks (71 §2.4, 123 §4.1): 2..=12 actors with unique ids, 1..=40 steps whose
/// `from`/`to` name an actor, the label length and the `ref` grammar; ≤ 2 notes; ≤ 2 fragments
/// over in-range, ordered, disjoint step indices, `else` inside an `alt`.
fn sequence(s: &SequenceProps, p: &str, errs: &mut Vec<SpecError>) {
    within(
        format!("{p}/actors"),
        s.actors.len(),
        SEQUENCE_ACTORS,
        "actors",
        errs,
    );
    within(
        format!("{p}/steps"),
        s.steps.len(),
        SEQUENCE_STEPS,
        "steps",
        errs,
    );
    title(p, s.title.as_deref(), errs);
    let ids = unique(
        &format!("{p}/actors"),
        s.actors.iter().map(|a| a.id.as_str()),
        "actor",
        errs,
    );
    for (i, a) in s.actors.iter().enumerate() {
        label(format!("{p}/actors/{i}/label"), &a.label, errs);
    }
    let mut notes = 0;
    for (i, st) in s.steps.iter().enumerate() {
        for (end, id) in [("from", &st.from), ("to", &st.to)] {
            resolves(format!("{p}/steps/{i}/{end}"), id, &ids, "actor", errs);
        }
        label(format!("{p}/steps/{i}/label"), &st.label, errs);
        anchor_ref(format!("{p}/steps/{i}/ref"), st.r#ref.as_deref(), errs);
        if let Some(n) = &st.note {
            label(format!("{p}/steps/{i}/note"), n, errs);
            notes += 1;
            if notes > MAX_SEQUENCE_NOTES {
                errs.push(err(
                    &format!("{p}/steps/{i}/note"),
                    format!("more than {MAX_SEQUENCE_NOTES} notes"),
                ));
            }
        }
    }
    let fragments = s.fragments.as_deref().unwrap_or_default();
    cap(
        format!("{p}/fragments"),
        fragments.len(),
        MAX_SEQUENCE_FRAGMENTS,
        "fragments",
        errs,
    );
    for (i, f) in fragments.iter().enumerate() {
        let fp = format!("{p}/fragments/{i}");
        if f.to >= s.steps.len() {
            errs.push(err(
                &format!("{fp}/to"),
                format!("step {} does not exist ({} steps)", f.to, s.steps.len()),
            ));
        } else if f.from > f.to {
            errs.push(err(
                &format!("{fp}/from"),
                format!("from {} is after to {}", f.from, f.to),
            ));
        }
        if let Some(e) = f.r#else {
            if f.kind != SequenceFragmentKind::Alt {
                errs.push(err(&format!("{fp}/else"), "else needs kind \"alt\""));
            } else if e <= f.from || e > f.to {
                errs.push(err(
                    &format!("{fp}/else"),
                    format!("else {e} is outside {}..={}", f.from + 1, f.to),
                ));
            }
        }
        if let Some(l) = &f.label {
            label(format!("{fp}/label"), l, errs);
        }
        if let Some((j, _)) = fragments[..i]
            .iter()
            .enumerate()
            .find(|(_, g)| f.from <= g.to && g.from <= f.to)
        {
            errs.push(err(&fp, format!("overlaps fragment {j}")));
        }
    }
}

/// `Timeline` (123 §4.1): `scale: time` needs ISO `when`/`until`; unique ids; the `ref` grammar.
fn timeline(t: &TimelineProps, p: &str, errs: &mut Vec<SpecError>) {
    title(p, t.title.as_deref(), errs);
    let timed = t.scale == Some(TimelineScale::Time);
    unique(
        &format!("{p}/items"),
        t.items.iter().filter_map(|i| i.id.as_deref()),
        "item",
        errs,
    );
    for (i, it) in t.items.iter().enumerate() {
        anchor_ref(format!("{p}/items/{i}/ref"), it.r#ref.as_deref(), errs);
        if timed {
            let from = iso(format!("{p}/items/{i}/when"), &it.when, errs);
            if let Some(u) = &it.until
                && let Some(to) = iso(format!("{p}/items/{i}/until"), u, errs)
                && from.is_some_and(|f| to < f)
            {
                errs.push(err(
                    &format!("{p}/items/{i}/until"),
                    format!("until {u:?} is before when {:?}", it.when),
                ));
            }
        }
    }
}

/// `Tree` (123 §4.1): unique ids and the `ref` grammar.
fn tree_props(t: &TreeProps, p: &str, errs: &mut Vec<SpecError>) {
    title(p, t.title.as_deref(), errs);
    unique(
        &format!("{p}/items"),
        t.items.iter().filter_map(|i| i.id.as_deref()),
        "item",
        errs,
    );
    for (i, it) in t.items.iter().enumerate() {
        anchor_ref(format!("{p}/items/{i}/ref"), it.r#ref.as_deref(), errs);
    }
}

/// `Kanban` (123 §4.1): `cards[].row` and `cut` index `rows`; unique card ids; the `ref` grammar.
fn kanban(k: &KanbanProps, p: &str, errs: &mut Vec<SpecError>) {
    title(p, k.title.as_deref(), errs);
    let rows = k.rows.as_ref().map_or(0, Vec::len);
    if let Some(c) = k.cut
        && c >= rows
    {
        errs.push(err(
            &format!("{p}/cut"),
            format!("row {c} does not exist ({rows} rows)"),
        ));
    }
    let mut ids = BTreeSet::new();
    for (ci, col) in k.columns.iter().enumerate() {
        for (i, card) in col.cards.iter().enumerate() {
            let cp = format!("{p}/columns/{ci}/cards/{i}");
            if let Some(id) = &card.id
                && !ids.insert(id.as_str())
            {
                errs.push(err(
                    &format!("{cp}/id"),
                    format!("duplicate card id {id:?}"),
                ));
            }
            if let Some(r) = card.row
                && r >= rows
            {
                errs.push(err(
                    &format!("{cp}/row"),
                    format!("row {r} does not exist ({rows} rows)"),
                ));
            }
            anchor_ref(format!("{cp}/ref"), card.r#ref.as_deref(), errs);
        }
    }
}

// ---- 123 ELK family (W1) ----

/// `StateMachine` (123 §4.3): caps, unique ids, ≤ 1 `initial`, transitions that resolve,
/// `guard`/`sub` ≤ 40, labels, refs.
fn state_machine(m: &StateMachineProps, p: &str, errs: &mut Vec<SpecError>) {
    title(p, m.title.as_deref(), errs);
    cap(
        format!("{p}/states"),
        m.states.len(),
        MAX_STATES,
        "states",
        errs,
    );
    cap(
        format!("{p}/transitions"),
        m.transitions.len(),
        MAX_TRANSITIONS,
        "transitions",
        errs,
    );
    let ids = unique(
        &format!("{p}/states"),
        m.states.iter().map(|s| s.id.as_str()),
        "state",
        errs,
    );
    let mut initial = None;
    for (i, s) in m.states.iter().enumerate() {
        label(format!("{p}/states/{i}/label"), &s.label, errs);
        short(
            format!("{p}/states/{i}/sub"),
            s.sub.as_deref(),
            MAX_SUB_CHARS,
            errs,
        );
        anchor_ref(format!("{p}/states/{i}/ref"), s.r#ref.as_deref(), errs);
        if s.kind == Some(MachineStateKind::Initial) {
            match initial {
                None => initial = Some(i),
                Some(first) => errs.push(err(
                    &format!("{p}/states/{i}/kind"),
                    format!("second initial state; the first is states/{first}"),
                )),
            }
        }
    }
    for (i, t) in m.transitions.iter().enumerate() {
        for (end, id) in [("from", &t.from), ("to", &t.to)] {
            resolves(
                format!("{p}/transitions/{i}/{end}"),
                id,
                &ids,
                "state",
                errs,
            );
        }
        if let Some(l) = &t.label {
            label(format!("{p}/transitions/{i}/label"), l, errs);
        }
        short(
            format!("{p}/transitions/{i}/guard"),
            t.guard.as_deref(),
            MAX_SUB_CHARS,
            errs,
        );
    }
}

/// `Entities` (123 §4.3): caps, unique entity ids and field names, `methods` only on
/// `class|interface`, relations whose ends and `*_field`s resolve, labels, refs.
fn entities(e: &EntitiesProps, p: &str, errs: &mut Vec<SpecError>) {
    title(p, e.title.as_deref(), errs);
    cap(
        format!("{p}/entities"),
        e.entities.len(),
        MAX_ENTITIES,
        "entities",
        errs,
    );
    cap(
        format!("{p}/relations"),
        e.relations.len(),
        MAX_RELATIONS,
        "relations",
        errs,
    );
    let ids = unique(
        &format!("{p}/entities"),
        e.entities.iter().map(|x| x.id.as_str()),
        "entity",
        errs,
    );
    let mut fields_of: BTreeMap<&str, BTreeSet<&str>> = BTreeMap::new();
    for (i, en) in e.entities.iter().enumerate() {
        let ep = format!("{p}/entities/{i}");
        label(format!("{ep}/label"), &en.label, errs);
        anchor_ref(format!("{ep}/ref"), en.r#ref.as_deref(), errs);
        cap(
            format!("{ep}/fields"),
            en.fields.len(),
            MAX_ENTITY_FIELDS,
            "fields",
            errs,
        );
        let mut names = BTreeSet::new();
        for (j, f) in en.fields.iter().enumerate() {
            if !names.insert(f.name.as_str()) {
                errs.push(err(
                    &format!("{ep}/fields/{j}/name"),
                    format!("duplicate field name {:?}", f.name),
                ));
            }
        }
        if en.methods.is_some()
            && !matches!(en.kind, Some(EntityKind::Class | EntityKind::Interface))
        {
            errs.push(err(
                &format!("{ep}/methods"),
                "methods need kind \"class\" or \"interface\"",
            ));
        }
        fields_of.entry(en.id.as_str()).or_insert(names);
    }
    for (i, r) in e.relations.iter().enumerate() {
        let rp = format!("{p}/relations/{i}");
        for (end, id, field) in [("from", &r.from, &r.from_field), ("to", &r.to, &r.to_field)] {
            resolves(format!("{rp}/{end}"), id, &ids, "entity", errs);
            if let Some(f) = field
                && let Some(names) = fields_of.get(id.as_str())
                && !names.contains(f.as_str())
            {
                errs.push(err(
                    &format!("{rp}/{end}_field"),
                    format!("unknown field {f:?} on {id:?}"),
                ));
            }
        }
        if let Some(l) = &r.label {
            label(format!("{rp}/label"), l, errs);
        }
    }
}

/// `Swimlane` (123 §4.3): caps, unique lane and step ids, `step.lane` and edge ends that resolve,
/// labels, `sub` ≤ 40, refs.
fn swimlane(s: &SwimlaneProps, p: &str, errs: &mut Vec<SpecError>) {
    title(p, s.title.as_deref(), errs);
    cap(
        format!("{p}/lanes"),
        s.lanes.len(),
        MAX_LANES,
        "lanes",
        errs,
    );
    cap(
        format!("{p}/steps"),
        s.steps.len(),
        MAX_LANE_STEPS,
        "steps",
        errs,
    );
    cap(
        format!("{p}/edges"),
        s.edges.len(),
        MAX_LANE_EDGES,
        "edges",
        errs,
    );
    let lanes = unique(
        &format!("{p}/lanes"),
        s.lanes.iter().map(|l| l.id.as_str()),
        "lane",
        errs,
    );
    for (i, l) in s.lanes.iter().enumerate() {
        label(format!("{p}/lanes/{i}/label"), &l.label, errs);
    }
    let steps = unique(
        &format!("{p}/steps"),
        s.steps.iter().map(|st| st.id.as_str()),
        "step",
        errs,
    );
    for (i, st) in s.steps.iter().enumerate() {
        resolves(
            format!("{p}/steps/{i}/lane"),
            &st.lane,
            &lanes,
            "lane",
            errs,
        );
        label(format!("{p}/steps/{i}/label"), &st.label, errs);
        short(
            format!("{p}/steps/{i}/sub"),
            st.sub.as_deref(),
            MAX_SUB_CHARS,
            errs,
        );
        anchor_ref(format!("{p}/steps/{i}/ref"), st.r#ref.as_deref(), errs);
    }
    for (i, e) in s.edges.iter().enumerate() {
        for (end, id) in [("from", &e.from), ("to", &e.to)] {
            resolves(format!("{p}/edges/{i}/{end}"), id, &steps, "step", errs);
        }
        if let Some(l) = &e.label {
            label(format!("{p}/edges/{i}/label"), l, errs);
        }
    }
}

// ---- 123 chart family (W2) ----

/// Every series has `n` values (`{m} values, {n} {unit}`) and a label-length name.
fn series_lengths(series: &[Series], n: usize, unit: &str, p: &str, errs: &mut Vec<SpecError>) {
    for (i, s) in series.iter().enumerate() {
        label(format!("{p}/series/{i}/name"), &s.name, errs);
        if s.values.len() != n {
            errs.push(err(
                &format!("{p}/series/{i}/values"),
                format!("{} values, {n} {unit}", s.values.len()),
            ));
        }
    }
}

/// `BarChart` (123 §4.3): ≤ 8 series × ≤ 40 labels, equal lengths, `dumbbell` = exactly 2 series.
fn bar_chart(c: &BarChartProps, p: &str, errs: &mut Vec<SpecError>) {
    title(p, c.title.as_deref(), errs);
    cap(
        format!("{p}/labels"),
        c.labels.len(),
        MAX_BAR_LABELS,
        "labels",
        errs,
    );
    cap(
        format!("{p}/series"),
        c.series.len(),
        MAX_CHART_SERIES,
        "series",
        errs,
    );
    series_lengths(&c.series, c.labels.len(), "labels", p, errs);
    if c.variant == Some(BarVariant::Dumbbell) && c.series.len() != 2 {
        errs.push(err(
            &format!("{p}/series"),
            format!("{} series, dumbbell needs exactly 2", c.series.len()),
        ));
    }
}

/// `LineChart` (123 §4.3): ≤ 8 series × ≤ 60 labels, equal lengths, `slope` = exactly 2 labels,
/// `bump` values integer ranks ≥ 1.
fn line_chart(c: &LineChartProps, p: &str, errs: &mut Vec<SpecError>) {
    title(p, c.title.as_deref(), errs);
    cap(
        format!("{p}/labels"),
        c.labels.len(),
        MAX_LINE_LABELS,
        "labels",
        errs,
    );
    cap(
        format!("{p}/series"),
        c.series.len(),
        MAX_CHART_SERIES,
        "series",
        errs,
    );
    series_lengths(&c.series, c.labels.len(), "labels", p, errs);
    match c.variant {
        Some(LineVariant::Slope) if c.labels.len() != 2 => errs.push(err(
            &format!("{p}/labels"),
            format!("{} labels, slope needs exactly 2", c.labels.len()),
        )),
        Some(LineVariant::Bump) => {
            for (i, s) in c.series.iter().enumerate() {
                for (j, v) in s.values.iter().enumerate() {
                    if v.fract() != 0.0 || *v < 1.0 {
                        errs.push(err(
                            &format!("{p}/series/{i}/values/{j}"),
                            format!("{v} is not a rank (integer ≥ 1)"),
                        ));
                    }
                }
            }
        }
        _ => {}
    }
}

/// `ScatterChart` (123 §4.3): ≤ 200 points, ≤ 8 groups with unique ids, `group` resolves,
/// `bubble` needs `size` everywhere, labels, refs.
fn scatter_chart(c: &ScatterChartProps, p: &str, errs: &mut Vec<SpecError>) {
    title(p, c.title.as_deref(), errs);
    cap(
        format!("{p}/points"),
        c.points.len(),
        MAX_SCATTER_POINTS,
        "points",
        errs,
    );
    let groups = c.groups.as_deref().unwrap_or_default();
    cap(
        format!("{p}/groups"),
        groups.len(),
        MAX_SCATTER_GROUPS,
        "groups",
        errs,
    );
    let ids = unique(
        &format!("{p}/groups"),
        groups.iter().map(|g| g.id.as_str()),
        "group",
        errs,
    );
    for (i, g) in groups.iter().enumerate() {
        label(format!("{p}/groups/{i}/label"), &g.label, errs);
    }
    let bubble = c.variant == Some(ScatterVariant::Bubble);
    for (i, pt) in c.points.iter().enumerate() {
        if let Some(g) = &pt.group {
            resolves(format!("{p}/points/{i}/group"), g, &ids, "group", errs);
        }
        if bubble && pt.size.is_none() {
            errs.push(err(
                &format!("{p}/points/{i}/size"),
                "missing; bubble needs size on every point",
            ));
        }
        if let Some(l) = &pt.label {
            label(format!("{p}/points/{i}/label"), l, errs);
        }
        anchor_ref(format!("{p}/points/{i}/ref"), pt.r#ref.as_deref(), errs);
    }
}

/// `RadarChart` (123 §4.3): 3..=8 axes, ≤ 5 series of `axes.len()` values, `polar` = 1 series.
fn radar_chart(c: &RadarChartProps, p: &str, errs: &mut Vec<SpecError>) {
    title(p, c.title.as_deref(), errs);
    within(format!("{p}/axes"), c.axes.len(), RADAR_AXES, "axes", errs);
    cap(
        format!("{p}/series"),
        c.series.len(),
        MAX_RADAR_SERIES,
        "series",
        errs,
    );
    series_lengths(&c.series, c.axes.len(), "axes", p, errs);
    if c.variant == Some(RadarVariant::Polar) && c.series.len() != 1 {
        errs.push(err(
            &format!("{p}/series"),
            format!("{} series, polar needs exactly 1", c.series.len()),
        ));
    }
}

/// `Waterfall` (123 §4.3): ≤ 20 steps, labels.
fn waterfall(w: &WaterfallProps, p: &str, errs: &mut Vec<SpecError>) {
    title(p, w.title.as_deref(), errs);
    cap(
        format!("{p}/steps"),
        w.steps.len(),
        MAX_WATERFALL_STEPS,
        "steps",
        errs,
    );
    for (i, s) in w.steps.iter().enumerate() {
        label(format!("{p}/steps/{i}/label"), &s.label, errs);
    }
}

/// `Funnel` (123 §4.3): ≤ 10 stages, values on all stages or none, labels.
fn funnel(f: &FunnelProps, p: &str, errs: &mut Vec<SpecError>) {
    title(p, f.title.as_deref(), errs);
    cap(
        format!("{p}/stages"),
        f.stages.len(),
        MAX_FUNNEL_STAGES,
        "stages",
        errs,
    );
    let with = f.stages.iter().filter(|s| s.value.is_some()).count();
    if with != 0 && with != f.stages.len() {
        for (i, _) in f
            .stages
            .iter()
            .enumerate()
            .filter(|(_, s)| s.value.is_none())
        {
            errs.push(err(
                &format!("{p}/stages/{i}/value"),
                "missing; values go on every stage or none",
            ));
        }
    }
    for (i, s) in f.stages.iter().enumerate() {
        label(format!("{p}/stages/{i}/label"), &s.label, errs);
    }
}

/// `Gantt` (123 §4.3): ≤ 60 tasks with unique ids, ISO `start`/`end`/`today`, `end ≥ start`,
/// `depends` resolving and not self, labels, refs.
fn gantt(g: &GanttProps, p: &str, errs: &mut Vec<SpecError>) {
    title(p, g.title.as_deref(), errs);
    cap(
        format!("{p}/tasks"),
        g.tasks.len(),
        MAX_GANTT_TASKS,
        "tasks",
        errs,
    );
    if let Some(t) = &g.today {
        iso(format!("{p}/today"), t, errs);
    }
    let ids = unique(
        &format!("{p}/tasks"),
        g.tasks.iter().map(|t| t.id.as_str()),
        "task",
        errs,
    );
    for (i, t) in g.tasks.iter().enumerate() {
        let tp = format!("{p}/tasks/{i}");
        label(format!("{tp}/label"), &t.label, errs);
        anchor_ref(format!("{tp}/ref"), t.r#ref.as_deref(), errs);
        let start = iso(format!("{tp}/start"), &t.start, errs);
        let end = iso(format!("{tp}/end"), &t.end, errs);
        if let (Some(s), Some(e)) = (start, end)
            && e < s
        {
            errs.push(err(
                &format!("{tp}/end"),
                format!("end {:?} is before start {:?}", t.end, t.start),
            ));
        }
        for (j, d) in t.depends.iter().flatten().enumerate() {
            if d == &t.id {
                errs.push(err(
                    &format!("{tp}/depends/{j}"),
                    format!("task {d:?} depends on itself"),
                ));
            } else {
                resolves(format!("{tp}/depends/{j}"), d, &ids, "task", errs);
            }
        }
    }
}

/// `Quadrant` (123 §4.3): ≤ 40 items with unique ids, `quadrants` exactly 4, ticks exactly 2,
/// labels, refs (`x`/`y` ∈ 0..=1 come from the schema).
fn quadrant(q: &QuadrantProps, p: &str, errs: &mut Vec<SpecError>) {
    title(p, q.title.as_deref(), errs);
    cap(
        format!("{p}/items"),
        q.items.len(),
        MAX_QUADRANT_ITEMS,
        "items",
        errs,
    );
    for (name, want, list) in [
        ("quadrants", 4, &q.quadrants),
        ("xTicks", 2, &q.x_ticks),
        ("yTicks", 2, &q.y_ticks),
    ] {
        if let Some(l) = list
            && l.len() != want
        {
            errs.push(err(
                &format!("{p}/{name}"),
                format!("{} {name}, expected exactly {want}", l.len()),
            ));
        }
    }
    unique(
        &format!("{p}/items"),
        q.items.iter().filter_map(|i| i.id.as_deref()),
        "item",
        errs,
    );
    for (i, it) in q.items.iter().enumerate() {
        label(format!("{p}/items/{i}/label"), &it.label, errs);
        anchor_ref(format!("{p}/items/{i}/ref"), it.r#ref.as_deref(), errs);
    }
}

// ---- 123 part-of-whole + misc (W3) ----

/// `Treemap` (123 §4.3): ≤ 60 cells over ≤ 12 groups, `value > 0`, labels, refs.
fn treemap(t: &TreemapProps, p: &str, errs: &mut Vec<SpecError>) {
    title(p, t.title.as_deref(), errs);
    cap(
        format!("{p}/items"),
        t.items.len(),
        MAX_TREEMAP_ITEMS,
        "items",
        errs,
    );
    let groups: BTreeSet<&str> = t.items.iter().filter_map(|c| c.group.as_deref()).collect();
    cap(
        format!("{p}/items"),
        groups.len(),
        MAX_TREEMAP_GROUPS,
        "groups",
        errs,
    );
    for (i, c) in t.items.iter().enumerate() {
        if !(c.value > 0.0) {
            errs.push(err(
                &format!("{p}/items/{i}/value"),
                format!("{} is not > 0", c.value),
            ));
        }
        label(format!("{p}/items/{i}/label"), &c.label, errs);
        anchor_ref(format!("{p}/items/{i}/ref"), c.r#ref.as_deref(), errs);
    }
}

/// `Sankey` (123 §4.3): ≤ 24 nodes with unique ids, ≤ 48 flows whose ends resolve, `from ≠ to`,
/// `value > 0`, no cycle — a DFS names the flow that closes one (`circular flow a → b → a`).
fn sankey(s: &SankeyProps, p: &str, errs: &mut Vec<SpecError>) {
    title(p, s.title.as_deref(), errs);
    cap(
        format!("{p}/nodes"),
        s.nodes.len(),
        MAX_SANKEY_NODES,
        "nodes",
        errs,
    );
    cap(
        format!("{p}/flows"),
        s.flows.len(),
        MAX_SANKEY_FLOWS,
        "flows",
        errs,
    );
    let ids = unique(
        &format!("{p}/nodes"),
        s.nodes.iter().map(|n| n.id.as_str()),
        "node",
        errs,
    );
    for (i, n) in s.nodes.iter().enumerate() {
        label(format!("{p}/nodes/{i}/label"), &n.label, errs);
    }
    let mut out: BTreeMap<&str, Vec<(usize, &str)>> = BTreeMap::new();
    for (i, f) in s.flows.iter().enumerate() {
        for (end, id) in [("from", &f.from), ("to", &f.to)] {
            resolves(format!("{p}/flows/{i}/{end}"), id, &ids, "node", errs);
        }
        if f.from == f.to {
            errs.push(err(
                &format!("{p}/flows/{i}/to"),
                format!("flow from {:?} to itself", f.from),
            ));
        } else {
            out.entry(f.from.as_str())
                .or_default()
                .push((i, f.to.as_str()));
        }
        if !(f.value > 0.0) {
            errs.push(err(
                &format!("{p}/flows/{i}/value"),
                format!("{} is not > 0", f.value),
            ));
        }
        if let Some(l) = &f.label {
            label(format!("{p}/flows/{i}/label"), l, errs);
        }
    }
    // Iterative DFS from every node in authored order; the first back edge is the error.
    let mut done: BTreeSet<&str> = BTreeSet::new();
    for root in s.nodes.iter().map(|n| n.id.as_str()) {
        if done.contains(root) {
            continue;
        }
        let mut stack: Vec<(&str, usize)> = vec![(root, 0)];
        let mut on: Vec<&str> = vec![root];
        while let Some((node, next)) = stack.last_mut() {
            let edges = out.get(*node).map(Vec::as_slice).unwrap_or_default();
            let Some(&(fi, to)) = edges.get(*next) else {
                done.insert(*node);
                stack.pop();
                on.pop();
                continue;
            };
            *next += 1;
            if let Some(start) = on.iter().position(|n| *n == to) {
                let cycle: Vec<&str> = on[start..].iter().chain([&to]).copied().collect();
                errs.push(err(
                    &format!("{p}/flows/{fi}"),
                    format!("circular flow {}", cycle.join(" → ")),
                ));
                // Report one cycle per component: everything on the stack counts as visited.
                done.extend(on.iter().copied());
                break;
            }
            if !done.contains(to) {
                stack.push((to, 0));
                on.push(to);
            }
        }
    }
}

/// `Venn` (123 §4.3): exactly 2 or 3 sets with unique ids; each overlap names 2..=3 distinct
/// known sets; no two overlaps name the same sets.
fn venn(v: &VennProps, p: &str, errs: &mut Vec<SpecError>) {
    title(p, v.title.as_deref(), errs);
    within(format!("{p}/sets"), v.sets.len(), VENN_SETS, "sets", errs);
    let ids = unique(
        &format!("{p}/sets"),
        v.sets.iter().map(|s| s.id.as_str()),
        "set",
        errs,
    );
    for (i, s) in v.sets.iter().enumerate() {
        label(format!("{p}/sets/{i}/label"), &s.label, errs);
    }
    let mut seen: BTreeSet<Vec<&str>> = BTreeSet::new();
    for (i, o) in v.overlaps.iter().flatten().enumerate() {
        let op = format!("{p}/overlaps/{i}");
        label(format!("{op}/label"), &o.label, errs);
        let mut key: Vec<&str> = o.sets.iter().map(String::as_str).collect();
        key.sort_unstable();
        key.dedup();
        if key.len() != o.sets.len() || !VENN_SETS.contains(&key.len()) {
            errs.push(err(
                &format!("{op}/sets"),
                format!("{} sets, expected 2 or 3 distinct", o.sets.len()),
            ));
        }
        for (j, id) in o.sets.iter().enumerate() {
            resolves(format!("{op}/sets/{j}"), id, &ids, "set", errs);
        }
        if !seen.insert(key) {
            errs.push(err(&format!("{op}/sets"), "duplicate overlap"));
        }
    }
}

/// `Loop` (123 §4.3): 2..=8 steps, labels, refs.
fn loop_(l: &LoopProps, p: &str, errs: &mut Vec<SpecError>) {
    title(p, l.title.as_deref(), errs);
    within(
        format!("{p}/steps"),
        l.steps.len(),
        LOOP_STEPS,
        "steps",
        errs,
    );
    for (i, s) in l.steps.iter().enumerate() {
        label(format!("{p}/steps/{i}/label"), &s.label, errs);
        anchor_ref(format!("{p}/steps/{i}/ref"), s.r#ref.as_deref(), errs);
    }
}

/// `Layers` (123 §4.3): ≤ 12 layers × ≤ 12 items, labels, refs.
fn layers(l: &LayersProps, p: &str, errs: &mut Vec<SpecError>) {
    title(p, l.title.as_deref(), errs);
    cap(
        format!("{p}/layers"),
        l.layers.len(),
        MAX_LAYERS,
        "layers",
        errs,
    );
    for (i, layer) in l.layers.iter().enumerate() {
        label(format!("{p}/layers/{i}/label"), &layer.label, errs);
        anchor_ref(format!("{p}/layers/{i}/ref"), layer.r#ref.as_deref(), errs);
        cap(
            format!("{p}/layers/{i}/items"),
            layer.items.as_ref().map_or(0, Vec::len),
            MAX_LAYERS,
            "items",
            errs,
        );
    }
}

/// `Journey` (123 §4.3): ≤ 8 stages, ≤ 4 rows of `stages.len()` cells, `mood` one per stage in
/// −1..=1, `pains[].stage` in range.
fn journey(j: &JourneyProps, p: &str, errs: &mut Vec<SpecError>) {
    title(p, j.title.as_deref(), errs);
    let n = j.stages.len();
    cap(format!("{p}/stages"), n, MAX_JOURNEY_STAGES, "stages", errs);
    cap(
        format!("{p}/rows"),
        j.rows.len(),
        MAX_JOURNEY_ROWS,
        "rows",
        errs,
    );
    for (i, r) in j.rows.iter().enumerate() {
        if r.cells.len() != n {
            errs.push(err(
                &format!("{p}/rows/{i}/cells"),
                format!("{} cells, {n} stages", r.cells.len()),
            ));
        }
    }
    if let Some(m) = &j.mood {
        if m.len() != n {
            errs.push(err(
                &format!("{p}/mood"),
                format!("{} moods, {n} stages", m.len()),
            ));
        }
        for (i, v) in m
            .iter()
            .enumerate()
            .filter(|(_, v)| !(-1.0..=1.0).contains(*v))
        {
            errs.push(err(
                &format!("{p}/mood/{i}"),
                format!("{v} is outside -1..=1"),
            ));
        }
    }
    for (i, pain) in j.pains.iter().flatten().enumerate() {
        if pain.stage >= n {
            errs.push(err(
                &format!("{p}/pains/{i}/stage"),
                format!("stage {} does not exist ({n} stages)", pain.stage),
            ));
        }
        label(format!("{p}/pains/{i}/text"), &pain.text, errs);
    }
}

/// `Fishbone` (123 §4.3): ≤ 8 categories × ≤ 6 items, labels.
fn fishbone(f: &FishboneProps, p: &str, errs: &mut Vec<SpecError>) {
    title(p, f.title.as_deref(), errs);
    label(format!("{p}/effect"), &f.effect, errs);
    cap(
        format!("{p}/causes"),
        f.causes.len(),
        MAX_FISHBONE_CAUSES,
        "causes",
        errs,
    );
    for (i, c) in f.causes.iter().enumerate() {
        label(format!("{p}/causes/{i}/category"), &c.category, errs);
        cap(
            format!("{p}/causes/{i}/items"),
            c.items.len(),
            MAX_FISHBONE_ITEMS,
            "items",
            errs,
        );
    }
}

/// `Excerpt` checks (71 §2.5): a root-relative path, 1..=120 lines and ≤ 16 KiB of text,
/// every `mark` pair ordered and inside `start..=end`.
fn excerpt(x: &ExcerptProps, p: &str, errs: &mut Vec<SpecError>) {
    if let Err(msg) = anchor::check_path(&x.path) {
        errs.push(err(&format!("{p}/path"), msg));
    }
    let lines = x.text.lines().count();
    if lines == 0 {
        errs.push(err(&format!("{p}/text"), "empty"));
    } else if lines > MAX_EXCERPT_LINES {
        errs.push(err(
            &format!("{p}/text"),
            format!("{lines} lines, limit {MAX_EXCERPT_LINES}"),
        ));
    }
    if x.text.len() > MAX_EXCERPT_BYTES {
        errs.push(err(
            &format!("{p}/text"),
            format!("{} bytes, limit {MAX_EXCERPT_BYTES}", x.text.len()),
        ));
    }
    let start = x.start.unwrap_or(1);
    let end = start.saturating_add(u32::try_from(lines.saturating_sub(1)).unwrap_or(u32::MAX));
    for (i, [from, to]) in x.mark.iter().flatten().enumerate() {
        if from > to {
            errs.push(err(
                &format!("{p}/mark/{i}"),
                format!("[{from}, {to}] is not ordered"),
            ));
        } else if *from < start || *to > end {
            errs.push(err(
                &format!("{p}/mark/{i}"),
                format!("[{from}, {to}] is outside the quoted lines {start}-{end}"),
            ));
        }
    }
}

/// Depth-first from `id`: marks `seen`, reports a cycle back into `stack` and nesting past [`MAX_DEPTH`].
fn reach<'a>(
    id: &'a str,
    tree: &BTreeMap<&'a str, Vec<&'a str>>,
    seen: &mut BTreeSet<&'a str>,
    stack: &mut Vec<&'a str>,
    errs: &mut Vec<SpecError>,
) {
    if stack.contains(&id) {
        errs.push(err("/", format!("cycle through {id:?}")));
        return;
    }
    if !seen.insert(id) {
        return;
    }
    if stack.len() >= MAX_DEPTH {
        errs.push(err(
            "/",
            format!("depth {} at {id:?}, limit {MAX_DEPTH}", stack.len() + 1),
        ));
        return;
    }
    stack.push(id);
    for k in tree.get(id).into_iter().flatten() {
        reach(k, tree, seen, stack, errs);
    }
    stack.pop();
}

/// Check `v` against the subset of JSON Schema schemars emits for the props structs:
/// `type` (string or list), `const`/`enum`, `anyOf`/`oneOf` (how it spells a nullable
/// or documented enum), `minimum`/`maximum`, `items`, `properties` + `required` +
/// `additionalProperties: false`. Anything else is left to serde.
fn walk(v: &Value, schema: &Value, path: &str, errs: &mut Vec<SpecError>) {
    if let Some(alts) = schema
        .get("anyOf")
        .or_else(|| schema.get("oneOf"))
        .and_then(Value::as_array)
    {
        let mut scratch = Vec::new();
        for alt in alts {
            scratch.clear();
            walk(v, alt, path, &mut scratch);
            if scratch.is_empty() {
                return;
            }
        }
        let mut choices = Vec::new();
        literals(schema, &mut choices);
        if choices.is_empty() {
            errs.append(&mut scratch);
        } else {
            errs.push(err(path, format!("expected one of {}", choices.join(", "))));
        }
        return;
    }
    let kind = match v {
        Value::Null => "null",
        Value::Bool(_) => "boolean",
        Value::Number(n) if n.is_f64() => "number",
        Value::Number(_) => "integer",
        Value::String(_) => "string",
        Value::Array(_) => "array",
        Value::Object(_) => "object",
    };
    let allowed: Vec<&str> = match &schema["type"] {
        Value::String(s) => vec![s.as_str()],
        Value::Array(a) => a.iter().filter_map(Value::as_str).collect(),
        _ => Vec::new(),
    };
    if !allowed.is_empty()
        && !allowed
            .iter()
            .any(|t| *t == kind || (*t == "number" && kind == "integer"))
    {
        let want = allowed
            .iter()
            .filter(|t| **t != "null")
            .copied()
            .collect::<Vec<_>>()
            .join(" or ");
        errs.push(err(path, format!("expected {want}")));
        return;
    }
    let literal_ok = match (schema.get("const"), schema["enum"].as_array()) {
        (Some(c), _) => c == v,
        (None, Some(choices)) => choices.contains(v),
        (None, None) => true,
    };
    if !literal_ok {
        let mut choices = Vec::new();
        literals(schema, &mut choices);
        errs.push(err(path, format!("expected one of {}", choices.join(", "))));
        return;
    }
    match v {
        Value::Number(n) => {
            let x = n.as_f64().unwrap_or_default();
            let (lo, hi) = (schema.get("minimum"), schema.get("maximum"));
            if lo.and_then(Value::as_f64).is_some_and(|lo| x < lo)
                || hi.and_then(Value::as_f64).is_some_and(|hi| x > hi)
            {
                let bound = |b: Option<&Value>| b.map(Value::to_string).unwrap_or_default();
                errs.push(err(path, format!("expected {}..={}", bound(lo), bound(hi))));
            }
        }
        Value::Array(a) if schema["items"].is_object() => {
            for (i, x) in a.iter().enumerate() {
                walk(x, &schema["items"], &format!("{path}/{i}"), errs);
            }
        }
        Value::Object(o) => {
            let props = schema["properties"].as_object();
            let closed = schema["additionalProperties"] == Value::Bool(false);
            for (k, x) in o {
                match props.and_then(|p| p.get(k)) {
                    Some(s) => walk(x, s, &format!("{path}/{}", esc(k)), errs),
                    None if closed => errs.push(err(&format!("{path}/{}", esc(k)), "unknown prop")),
                    None => {}
                }
            }
            for r in schema["required"]
                .as_array()
                .into_iter()
                .flatten()
                .filter_map(Value::as_str)
            {
                if !o.contains_key(r) {
                    errs.push(err(&format!("{path}/{}", esc(r)), "missing"));
                }
            }
        }
        _ => {}
    }
}

/// Every non-null `const`/`enum` literal in `schema`, through `anyOf`/`oneOf`, JSON-encoded.
fn literals(schema: &Value, out: &mut Vec<String>) {
    if let Some(c) = schema.get("const")
        && !c.is_null()
    {
        out.push(c.to_string());
    }
    for c in schema["enum"]
        .as_array()
        .into_iter()
        .flatten()
        .filter(|c| !c.is_null())
    {
        out.push(c.to_string());
    }
    for alt in ["anyOf", "oneOf"]
        .iter()
        .filter_map(|k| schema[*k].as_array())
        .flatten()
    {
        literals(alt, out);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn errors(body: &str) -> Vec<SpecError> {
        crate::parse(body).expect_err("should not validate")
    }

    fn paths(body: &str) -> Vec<String> {
        errors(body).into_iter().map(|e| e.path).collect()
    }

    #[test]
    fn structural_errors_are_located_and_all_reported() {
        let errs = errors(
            r#"{"root":"main","elements":{
                "r":{"type":"Card","children":["h","x"]},
                "h":{"type":"Heading","props":{"text":"t","level":9},"children":["r"]},
                "t":{"type":"DataTable","props":{}},
                "g":{"type":"Grid","props":{"columns":0}}
            },"extra":1}"#,
        );
        let got: Vec<String> = errs.iter().map(ToString::to_string).collect();
        let expect = [
            ("/extra", "unknown key"),
            ("/root", "\"main\" is not an element id"),
            ("/elements/r/children/1", "\"x\" is not an element id"),
            ("/elements/h/children", "Heading cannot have children"),
            ("/elements/h/props/level", "expected 1..=4"),
            (
                "/elements/t/type",
                "unknown component \"DataTable\"; catalog: Accordion, Alert, Badge",
            ),
            ("/elements/g/props/columns", "expected 1..=6"),
        ];
        for (path, fragment) in expect {
            assert!(
                errs.iter()
                    .any(|e| e.path == path && e.message.contains(fragment)),
                "missing {path}: {fragment} in {got:#?}"
            );
        }
        assert!(
            !got.iter().any(|m| m.contains("reachable")),
            "no reachability errors without a valid root: {got:#?}"
        );
    }

    #[test]
    fn graph_errors() {
        assert_eq!(
            paths(
                r#"{"root":"a","elements":{"a":{"type":"Stack","children":["b"]},"b":{"type":"Stack","children":["a"]}}}"#
            ),
            ["/"]
        );
        let errs = errors(
            r#"{"root":"a","elements":{"a":{"type":"Stack","children":["b"]},"b":{"type":"Stack","children":["a"]}}}"#,
        );
        assert!(errs[0].message.contains("cycle through"), "{errs:?}");
        assert_eq!(
            paths(r#"{"root":"a","elements":{"a":{"type":"Separator"},"b":{"type":"Separator"}}}"#),
            ["/elements/b"]
        );
        let errs = errors(
            r#"{"root":"a","elements":{"a":{"type":"Stack","children":["x","x"]},"x":{"type":"Separator"}}}"#,
        );
        assert_eq!(errs.len(), 1, "{errs:?}");
        assert!(
            errs[0].path == "/" && errs[0].message.contains("two parents (a, a)"),
            "{errs:?}"
        );
    }

    #[test]
    fn prop_errors_carry_field_paths() {
        let body = r#"{"root":"r","elements":{"r":{"type":"Stack","props":{"gap":"huge","bogus":1},"children":["t","m","k","tb"]},
            "t":{"type":"Table","props":{"columns":["a","b"],"rows":[["1","2"],["3"],["4","5","6"]]}},
            "m":{"type":"MetricGrid","props":{"items":[{"label":"x","value":"1"},{"label":"y"}]}},
            "k":{"type":"KeyValueList","props":{"items":[{"key":"a","value":2}]}},
            "tb":{"type":"Tabs","props":{"labels":["one","two"]},"children":["s"]},
            "s":{"type":"Separator"}}}"#;
        let mut got = paths(body);
        got.sort();
        assert_eq!(
            got,
            [
                "/elements/k/props/items/0/value",
                "/elements/m/props/items/1/value",
                "/elements/r/props/bogus",
                "/elements/r/props/gap",
                "/elements/t/props/rows/1",
                "/elements/t/props/rows/2",
                "/elements/tb/props/labels",
            ]
        );
        let errs = errors(body);
        let msg = |p: &str| {
            errs.iter()
                .find(|e| e.path == p)
                .map(|e| e.message.as_str())
                .unwrap_or_default()
        };
        assert_eq!(msg("/elements/k/props/items/0/value"), "expected string");
        assert_eq!(msg("/elements/m/props/items/1/value"), "missing");
        assert_eq!(msg("/elements/r/props/bogus"), "unknown prop");
        assert!(
            msg("/elements/r/props/gap").starts_with("expected one of \"none\""),
            "{errs:?}"
        );
        assert_eq!(msg("/elements/t/props/rows/2"), "3 cells, 2 columns");
        assert_eq!(msg("/elements/tb/props/labels"), "2 labels, 1 children");
    }

    #[test]
    fn limits() {
        let mut elements = Map::new();
        let ids: Vec<String> = (0..MAX_ELEMENTS + 1).map(|i| format!("e{i}")).collect();
        elements.insert("r".into(), json!({ "type": "Stack", "children": ids }));
        for id in &ids {
            elements.insert(id.clone(), json!({ "type": "Separator" }));
        }
        let errs = errors(&json!({ "root": "r", "elements": elements }).to_string());
        assert_eq!(errs.len(), 1, "{errs:?}");
        assert_eq!(
            errs[0].to_string(),
            format!("/: {} elements, limit {MAX_ELEMENTS}", MAX_ELEMENTS + 2)
        );

        let mut elements = Map::new();
        for i in 0..MAX_DEPTH + 1 {
            let mut el = json!({ "type": "Stack" });
            if i < MAX_DEPTH {
                el["children"] = json!([format!("d{}", i + 1)]);
            }
            elements.insert(format!("d{i}"), el);
        }
        let errs = errors(&json!({ "root": "d0", "elements": elements }).to_string());
        assert_eq!(errs.len(), 1, "{errs:?}");
        assert_eq!(
            errs[0].to_string(),
            format!(
                "/: depth {} at \"d{MAX_DEPTH}\", limit {MAX_DEPTH}",
                MAX_DEPTH + 1
            )
        );
        assert!(
            crate::parse(&format!("{{{}", " ".repeat(crate::MAX_FENCE_BYTES))).unwrap_err()[0]
                .message
                .contains("byte limit")
        );
    }

    #[test]
    fn graph_caps_and_labels() {
        let nodes: Vec<Value> = (0..=MAX_GRAPH_NODES)
            .map(|i| json!({ "id": format!("n{i}"), "label": "x" }))
            .collect();
        let body = json!({ "root": "g", "elements": { "g": { "type": "Graph", "props": { "nodes": nodes, "edges": [] } } } });
        let errs = errors(&body.to_string());
        assert_eq!(
            errs.iter().map(ToString::to_string).collect::<Vec<_>>(),
            [format!(
                "/elements/g/props/nodes: {} nodes, limit {MAX_GRAPH_NODES}",
                MAX_GRAPH_NODES + 1
            )]
        );
        let long = "x".repeat(MAX_GRAPH_LABEL_CHARS + 1);
        let body = json!({ "root": "g", "elements": { "g": { "type": "Graph", "props": {
            "groups": [{ "id": "a", "label": "A" }, { "id": "a", "label": long }],
            "nodes": [{ "id": "n", "label": long, "group": "a" }, { "id": "m", "label": "ok" }],
            "edges": [{ "from": "n", "to": "n", "label": long }, { "from": "zz", "to": "m" }]
        } } } });
        let mut got = paths(&body.to_string());
        got.sort();
        assert_eq!(
            got,
            [
                "/elements/g/props/edges/0/label",
                "/elements/g/props/edges/1/from",
                "/elements/g/props/groups/1/id",
                "/elements/g/props/groups/1/label",
                "/elements/g/props/nodes/0/label",
            ]
        );
        let errs = errors(&body.to_string());
        assert!(
            errs.iter()
                .any(|e| e.to_string() == "/elements/g/props/edges/1/from: unknown node \"zz\""),
            "{errs:?}"
        );
        assert!(
            errs.iter().any(
                |e| e.to_string() == "/elements/g/props/nodes/0/label: 81 characters, limit 80"
            ),
            "{errs:?}"
        );
    }

    #[test]
    fn heatmap_shape_and_url_schemes() {
        let body = json!({ "root": "s", "elements": {
            "s": { "type": "Stack", "children": ["h", "i", "l"] },
            "h": { "type": "Heatmap", "props": { "rows": ["a", "b"], "cols": ["x", "y"], "cells": [[1, 2], [3]] } },
            "i": { "type": "Image", "props": { "src": "javascript:alert(1)", "alt": "x" } },
            "l": { "type": "LinkList", "props": { "items": [{ "title": "ok", "url": "HTTPS://a.example" }, { "title": "no", "url": "file:///etc/passwd" }] } }
        } });
        let mut got = paths(&body.to_string());
        got.sort();
        assert_eq!(
            got,
            [
                "/elements/h/props/cells/1",
                "/elements/i/props/src",
                "/elements/l/props/items/1/url",
            ]
        );
        let ragged = json!({ "root": "h", "elements": { "h": { "type": "Heatmap", "props": { "rows": ["a"], "cols": ["x"], "cells": [[1], [2]] } } } });
        assert_eq!(
            errors(&ragged.to_string())[0].to_string(),
            "/elements/h/props/cells: 2 rows of cells, 1 rows"
        );
        let ok = json!({ "root": "i", "elements": { "i": { "type": "Image", "props": { "src": "data:image/png;base64,AA==", "alt": "dot" } } } });
        assert!(crate::parse(&ok.to_string()).is_ok());
    }

    #[test]
    fn sequence_and_excerpt_errors_are_located_with_messages() {
        let body = json!({ "root": "r", "elements": {
            "r": { "type": "Stack", "children": ["s", "x", "g"] },
            "s": { "type": "Sequence", "props": {
                "actors": [{ "id": "a", "label": "A" }, { "id": "a", "label": "A again" }, { "id": "b", "label": "B" }],
                "steps": [
                    { "from": "a", "to": "zz", "label": "go", "ref": "a/b.rs#L9-L3" },
                    { "from": "b", "to": "a", "label": "ok", "style": "return", "ref": "block:p1" }
                ] } },
            "x": { "type": "Excerpt", "props": {
                "path": "/abs/x.rs", "start": 10, "text": "a\nb\nc",
                "mark": [[10, 12], [12, 11], [9, 10], [11, 13]] } },
            "g": { "type": "Graph", "props": { "nodes": [{ "id": "n", "label": "N", "ref": "http://x/y.rs" }], "edges": [] } }
        } });
        let errs = errors(&body.to_string());
        let mut got: Vec<String> = errs.iter().map(ToString::to_string).collect();
        got.sort();
        assert_eq!(
            got,
            [
                "/elements/g/props/nodes/0/ref: path must not carry a scheme: http:",
                "/elements/s/props/actors/1/id: duplicate actor id \"a\"",
                "/elements/s/props/steps/0/ref: L3 is before L9",
                "/elements/s/props/steps/0/to: unknown actor \"zz\"",
                "/elements/x/props/mark/1: [12, 11] is not ordered",
                "/elements/x/props/mark/2: [9, 10] is outside the quoted lines 10-12",
                "/elements/x/props/mark/3: [11, 13] is outside the quoted lines 10-12",
                "/elements/x/props/path: path must be root-relative, not absolute: \"/abs/x.rs\"",
            ]
        );

        let one_actor = json!({ "root": "s", "elements": { "s": { "type": "Sequence", "props": {
            "actors": [{ "id": "a", "label": "A" }], "steps": [] } } } });
        let mut got = errors(&one_actor.to_string())
            .iter()
            .map(ToString::to_string)
            .collect::<Vec<_>>();
        got.sort();
        assert_eq!(
            got,
            [
                "/elements/s/props/actors: 1 actors, expected 2..=12",
                "/elements/s/props/steps: 0 steps, expected 1..=40",
            ]
        );

        let long = (0..=MAX_EXCERPT_LINES)
            .map(|_| "x")
            .collect::<Vec<_>>()
            .join("\n");
        let big = json!({ "root": "x", "elements": { "x": { "type": "Excerpt", "props": { "path": "a.rs", "text": long } } } });
        assert_eq!(
            errors(&big.to_string())[0].to_string(),
            format!(
                "/elements/x/props/text: {} lines, limit {MAX_EXCERPT_LINES}",
                MAX_EXCERPT_LINES + 1
            )
        );
        let empty = json!({ "root": "x", "elements": { "x": { "type": "Excerpt", "props": { "path": "a.rs", "text": "" } } } });
        assert_eq!(
            errors(&empty.to_string())[0].to_string(),
            "/elements/x/props/text: empty"
        );
        let zero = json!({ "root": "x", "elements": { "x": { "type": "Excerpt", "props": { "path": "a.rs", "text": "a", "start": 0 } } } });
        assert_eq!(paths(&zero.to_string()), ["/elements/x/props/start"]);
        let ok = json!({ "root": "x", "elements": { "x": { "type": "Excerpt", "props": { "path": "a.rs", "text": "one\n", "mark": [[1, 1]] } } } });
        assert!(crate::parse(&ok.to_string()).is_ok());
    }

    /// One element of `typ` with `props` as a whole spec.
    fn body(typ: &str, props: Value) -> String {
        json!({ "root": "x", "elements": { "x": { "type": typ, "props": props } } }).to_string()
    }

    fn messages(typ: &str, props: Value) -> Vec<String> {
        let mut got: Vec<String> = errors(&body(typ, props))
            .iter()
            .map(ToString::to_string)
            .collect();
        got.sort();
        got
    }

    #[test]
    fn iso_datetime_accepts_dates_and_rfc3339_and_refuses_the_rest() {
        assert_eq!(iso_datetime("2026-10-07"), Some([2026, 10, 7, 0, 0, 0]));
        assert_eq!(iso_datetime("2024-02-29"), Some([2024, 2, 29, 0, 0, 0]));
        assert_eq!(
            iso_datetime("2026-10-07T09:12Z"),
            Some([2026, 10, 7, 9, 12, 0])
        );
        assert_eq!(
            iso_datetime("2026-10-07T09:12:30.250+02:00"),
            Some([2026, 10, 7, 9, 12, 30])
        );
        assert_eq!(
            iso_datetime("2026-10-07 09:12:30"),
            Some([2026, 10, 7, 9, 12, 30])
        );
        for bad in [
            "Oct 7",
            "2026-13-01",
            "2026-02-30",
            "2023-02-29",
            "2026-10-07T24:00",
            "2026-10-07T09:12:30+2:00",
            "2026-10-07T09:12:30.",
            "2026-10-7",
        ] {
            assert_eq!(iso_datetime(bad), None, "{bad}");
        }
        assert!(iso_datetime("2026-10-07T10:00Z") > iso_datetime("2026-10-07"));
    }

    /// 123 §4.3 message shapes: the three the design names, plus the cycle naming its closing flow.
    #[test]
    fn diagram_messages_follow_the_design_shapes() {
        assert_eq!(
            messages(
                "StateMachine",
                json!({ "states": [{ "id": "a", "label": "A" }], "transitions": [{ "from": "a", "to": "x" }] })
            ),
            ["/elements/x/props/transitions/0/to: unknown state \"x\""]
        );
        assert_eq!(
            messages(
                "BarChart",
                json!({ "labels": ["a", "b", "c", "d", "e", "f"], "series": [{ "name": "s", "values": [1, 2, 3, 4, 5, 6] }, { "name": "t", "values": [1, 2, 3, 4, 5] }] })
            ),
            ["/elements/x/props/series/1/values: 5 values, 6 labels"]
        );
        assert_eq!(
            messages(
                "Sankey",
                json!({ "nodes": [{ "id": "a", "label": "A" }, { "id": "b", "label": "B" }, { "id": "c", "label": "C" }],
                "flows": [{ "from": "a", "to": "b", "value": 1 }, { "from": "b", "to": "c", "value": 1 }, { "from": "c", "to": "a", "value": 1 }] })
            ),
            ["/elements/x/props/flows/2: circular flow a → b → c → a"]
        );
        // Two sides of a GraphDelta are checked with the side in the path.
        assert_eq!(
            messages(
                "GraphDelta",
                json!({ "before": { "nodes": [{ "id": "a", "label": "A" }], "edges": [{ "from": "a", "to": "zz" }] }, "after": { "nodes": [], "edges": [] } })
            ),
            ["/elements/x/props/before/edges/0/to: unknown node \"zz\""]
        );
    }

    #[test]
    fn graph_group_nesting_cycles_and_depth_are_located_on_parent() {
        let g = |groups: Value| json!({ "nodes": [], "edges": [], "groups": groups });
        assert_eq!(
            messages(
                "Graph",
                g(
                    json!([{ "id": "a", "label": "A", "parent": "b" }, { "id": "b", "label": "B", "parent": "a" }])
                )
            ),
            [
                "/elements/x/props/groups/0/parent: group \"a\" contains itself",
                "/elements/x/props/groups/1/parent: group \"b\" contains itself",
            ]
        );
        let chain = json!([{ "id": "a", "label": "A" }, { "id": "b", "label": "B", "parent": "a" }, { "id": "c", "label": "C", "parent": "b" }, { "id": "d", "label": "D", "parent": "c" }]);
        assert_eq!(
            messages("Graph", g(chain)),
            ["/elements/x/props/groups/3/parent: nesting depth 4, limit 3"]
        );
        let three = json!([{ "id": "a", "label": "A" }, { "id": "b", "label": "B", "parent": "a" }, { "id": "c", "label": "C", "parent": "b" }]);
        assert!(crate::parse(&body("Graph", g(three))).is_ok());
    }

    #[test]
    fn sequence_fragments_and_notes_are_bounded() {
        let seq = |fragments: Value| {
            json!({
            "actors": [{ "id": "a", "label": "A" }, { "id": "b", "label": "B" }],
            "steps": [{ "from": "a", "to": "b", "label": "1" }, { "from": "b", "to": "a", "label": "2" }, { "from": "a", "to": "b", "label": "3" }],
            "fragments": fragments })
        };
        assert_eq!(
            messages(
                "Sequence",
                seq(
                    json!([{ "kind": "alt", "from": 1, "to": 0 }, { "kind": "opt", "from": 2, "to": 3 }])
                )
            ),
            [
                "/elements/x/props/fragments/0/from: from 1 is after to 0",
                "/elements/x/props/fragments/1/to: step 3 does not exist (3 steps)",
            ]
        );
        assert_eq!(
            messages(
                "Sequence",
                seq(
                    json!([{ "kind": "loop", "from": 0, "to": 0, "else": 0 }, { "kind": "par", "from": 1, "to": 1 }, { "kind": "par", "from": 2, "to": 2 }])
                )
            ),
            [
                "/elements/x/props/fragments/0/else: else needs kind \"alt\"",
                "/elements/x/props/fragments: 3 fragments, limit 2",
            ]
        );
        assert_eq!(
            messages(
                "Sequence",
                seq(
                    json!([{ "kind": "alt", "from": 0, "to": 2, "else": 0 }, { "kind": "par", "from": 2, "to": 2 }])
                )
            ),
            [
                "/elements/x/props/fragments/0/else: else 0 is outside 1..=2",
                "/elements/x/props/fragments/1: overlaps fragment 0",
            ]
        );
        assert!(crate::parse(&body("Sequence", seq(json!([{ "kind": "alt", "from": 0, "to": 1, "else": 1 }, { "kind": "loop", "from": 2, "to": 2 }])))).is_ok());
    }

    #[test]
    fn chart_variants_pin_their_arity_rules() {
        let two =
            json!([{ "name": "a", "values": [1, 2, 3] }, { "name": "b", "values": [2, 3, 4] }]);
        let ok = json!({ "labels": ["x", "y", "z"], "series": two });
        assert!(crate::parse(&body("BarChart", ok.clone())).is_ok());
        let mut dumbbell = ok.clone();
        dumbbell["variant"] = json!("dumbbell");
        assert!(
            crate::parse(&body("BarChart", dumbbell)).is_ok(),
            "two series is a dumbbell"
        );
        let mut polar = json!({ "axes": ["a", "b", "c"], "series": two, "variant": "polar" });
        assert_eq!(
            messages("RadarChart", polar.clone()),
            ["/elements/x/props/series: 2 series, polar needs exactly 1"]
        );
        polar["series"] = json!([{ "name": "a", "values": [1, 2, 3] }]);
        assert!(crate::parse(&body("RadarChart", polar)).is_ok());
        assert_eq!(
            messages(
                "LineChart",
                json!({ "labels": ["a", "b"], "series": [{ "name": "r", "values": [1, 0] }], "variant": "bump" })
            ),
            ["/elements/x/props/series/0/values/1: 0 is not a rank (integer ≥ 1)"]
        );
        assert_eq!(
            messages(
                "Funnel",
                json!({ "stages": [{ "label": "a", "value": 10 }, { "label": "b" }] })
            ),
            ["/elements/x/props/stages/1/value: missing; values go on every stage or none"]
        );
        assert!(
            crate::parse(&body(
                "Funnel",
                json!({ "stages": [{ "label": "a" }, { "label": "b" }] })
            ))
            .is_ok()
        );
    }

    #[test]
    fn gantt_venn_kanban_and_timeline_rules() {
        assert_eq!(
            messages(
                "Gantt",
                json!({ "today": "soon", "tasks": [{ "id": "a", "label": "A", "start": "2026-10-10", "end": "2026-10-09", "depends": ["a", "b"] }] })
            ),
            [
                "/elements/x/props/tasks/0/depends/0: task \"a\" depends on itself",
                "/elements/x/props/tasks/0/depends/1: unknown task \"b\"",
                "/elements/x/props/tasks/0/end: end \"2026-10-09\" is before start \"2026-10-10\"",
                "/elements/x/props/today: \"soon\" is not an ISO date (YYYY-MM-DD) or datetime (RFC 3339)",
            ]
        );
        assert_eq!(
            messages(
                "Venn",
                json!({ "sets": [{ "id": "a", "label": "A" }, { "id": "b", "label": "B" }],
                "overlaps": [{ "sets": ["a", "b"], "label": "ab" }, { "sets": ["b", "a"], "label": "ba" }, { "sets": ["a", "a"], "label": "aa" }] })
            ),
            [
                "/elements/x/props/overlaps/1/sets: duplicate overlap",
                "/elements/x/props/overlaps/2/sets: 2 sets, expected 2 or 3 distinct",
            ]
        );
        assert_eq!(
            messages(
                "Kanban",
                json!({ "rows": ["r0"], "cut": 1, "columns": [{ "title": "c", "cards": [{ "title": "k", "row": 1, "id": "k" }, { "title": "k2", "id": "k" }] }] })
            ),
            [
                "/elements/x/props/columns/0/cards/0/row: row 1 does not exist (1 rows)",
                "/elements/x/props/columns/0/cards/1/id: duplicate card id \"k\"",
                "/elements/x/props/cut: row 1 does not exist (1 rows)",
            ]
        );
        // `scale: list` (the default) keeps free-text `when`; `time` requires ISO and ordered spans.
        let items = json!([{ "when": "Q1", "title": "a" }, { "when": "2026-10-02", "until": "2026-10-01", "title": "b" }]);
        assert!(crate::parse(&body("Timeline", json!({ "items": items }))).is_ok());
        assert_eq!(
            messages("Timeline", json!({ "items": items, "scale": "time" })),
            [
                "/elements/x/props/items/0/when: \"Q1\" is not an ISO date (YYYY-MM-DD) or datetime (RFC 3339)",
                "/elements/x/props/items/1/until: until \"2026-10-01\" is before when \"2026-10-02\"",
            ]
        );
    }
}
