//! Located validation of a parsed fence body: every error is collected, none stops the walk.

use std::collections::{BTreeMap, BTreeSet};

use serde_json::{Map, Value, json};

use crate::anchor;
use crate::catalog::catalog;
use crate::spec::{Component, Element, ExcerptProps, GraphProps, SequenceProps, Spec};
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

/// `Graph` checks serde cannot express (65 §3.2): unique node and group ids, edge endpoints and
/// node groups that exist, the caps, the label length and the `ref` grammar. Self-loops are fine.
fn graph(g: &GraphProps, p: &str, errs: &mut Vec<SpecError>) {
    if g.nodes.len() > MAX_GRAPH_NODES {
        errs.push(err(
            &format!("{p}/nodes"),
            format!("{} nodes, limit {MAX_GRAPH_NODES}", g.nodes.len()),
        ));
    }
    if g.edges.len() > MAX_GRAPH_EDGES {
        errs.push(err(
            &format!("{p}/edges"),
            format!("{} edges, limit {MAX_GRAPH_EDGES}", g.edges.len()),
        ));
    }
    let groups = g.groups.as_deref().unwrap_or_default();
    if groups.len() > MAX_GRAPH_GROUPS {
        errs.push(err(
            &format!("{p}/groups"),
            format!("{} groups, limit {MAX_GRAPH_GROUPS}", groups.len()),
        ));
    }
    let mut group_ids = BTreeSet::new();
    for (i, grp) in groups.iter().enumerate() {
        if !group_ids.insert(grp.id.as_str()) {
            errs.push(err(
                &format!("{p}/groups/{i}/id"),
                format!("duplicate group id {:?}", grp.id),
            ));
        }
        label(format!("{p}/groups/{i}/label"), &grp.label, errs);
    }
    let mut node_ids = BTreeSet::new();
    for (i, n) in g.nodes.iter().enumerate() {
        if !node_ids.insert(n.id.as_str()) {
            errs.push(err(
                &format!("{p}/nodes/{i}/id"),
                format!("duplicate node id {:?}", n.id),
            ));
        }
        if let Some(grp) = &n.group
            && !group_ids.contains(grp.as_str())
        {
            errs.push(err(
                &format!("{p}/nodes/{i}/group"),
                format!("unknown group {grp:?}"),
            ));
        }
        label(format!("{p}/nodes/{i}/label"), &n.label, errs);
        anchor_ref(format!("{p}/nodes/{i}/ref"), n.r#ref.as_deref(), errs);
    }
    for (i, e) in g.edges.iter().enumerate() {
        for (end, id) in [("from", &e.from), ("to", &e.to)] {
            if !node_ids.contains(id.as_str()) {
                errs.push(err(
                    &format!("{p}/edges/{i}/{end}"),
                    format!("unknown node {id:?}"),
                ));
            }
        }
        if let Some(l) = &e.label {
            label(format!("{p}/edges/{i}/label"), l, errs);
        }
    }
}

/// `Sequence` checks (71 §2.4): 2..=12 actors with unique ids, 1..=40 steps whose `from`/`to`
/// name an actor, the label length and the `ref` grammar.
fn sequence(s: &SequenceProps, p: &str, errs: &mut Vec<SpecError>) {
    if !SEQUENCE_ACTORS.contains(&s.actors.len()) {
        errs.push(err(
            &format!("{p}/actors"),
            format!(
                "{} actors, expected {}..={}",
                s.actors.len(),
                SEQUENCE_ACTORS.start(),
                SEQUENCE_ACTORS.end()
            ),
        ));
    }
    if !SEQUENCE_STEPS.contains(&s.steps.len()) {
        errs.push(err(
            &format!("{p}/steps"),
            format!(
                "{} steps, expected {}..={}",
                s.steps.len(),
                SEQUENCE_STEPS.start(),
                SEQUENCE_STEPS.end()
            ),
        ));
    }
    let mut ids = BTreeSet::new();
    for (i, a) in s.actors.iter().enumerate() {
        if !ids.insert(a.id.as_str()) {
            errs.push(err(
                &format!("{p}/actors/{i}/id"),
                format!("duplicate actor id {:?}", a.id),
            ));
        }
        label(format!("{p}/actors/{i}/label"), &a.label, errs);
    }
    for (i, st) in s.steps.iter().enumerate() {
        for (end, id) in [("from", &st.from), ("to", &st.to)] {
            if !ids.contains(id.as_str()) {
                errs.push(err(
                    &format!("{p}/steps/{i}/{end}"),
                    format!("unknown actor {id:?}"),
                ));
            }
        }
        label(format!("{p}/steps/{i}/label"), &st.label, errs);
        anchor_ref(format!("{p}/steps/{i}/ref"), st.r#ref.as_deref(), errs);
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
}
