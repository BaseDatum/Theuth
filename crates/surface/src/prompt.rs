//! What the model reads about surfaces (design doc 30 §5, §6): the `RegisterPrompt` block,
//! the `<surface_error>` correction message and the generated `visualize` skill.

use serde_json::Value;

use crate::SpecError;
use crate::catalog::{catalog, catalog_compact};
use crate::validate::CONTAINERS;

/// The `visualize` skill template; `{{catalog}}` is filled by [`visualize_skill`].
const SKILL: &str = include_str!("../visualize.md");

/// One minimal element per component, in catalog order; the `### Name` example in the skill.
/// Containers reference children `a`/`b`, which the example leaves to the reader.
const EXAMPLES: [(&str, &str); 19] = [
    (
        "Card",
        r#"{"type": "Card", "props": {"title": "Release readiness"}, "children": ["a", "b"]}"#,
    ),
    (
        "Stack",
        r#"{"type": "Stack", "props": {"direction": "horizontal", "gap": "md"}, "children": ["a", "b"]}"#,
    ),
    (
        "Grid",
        r#"{"type": "Grid", "props": {"columns": 2}, "children": ["a", "b"]}"#,
    ),
    ("Separator", r#"{"type": "Separator", "props": {}}"#),
    (
        "Heading",
        r#"{"type": "Heading", "props": {"text": "Rollout", "level": 2}}"#,
    ),
    (
        "Text",
        r#"{"type": "Text", "props": {"text": "Two of three gates pass.", "variant": "muted"}}"#,
    ),
    (
        "Badge",
        r#"{"type": "Badge", "props": {"text": "blocked", "variant": "destructive"}}"#,
    ),
    (
        "Alert",
        r#"{"type": "Alert", "props": {"title": "Certificate expires", "message": "Renew before Friday.", "type": "warning"}}"#,
    ),
    (
        "Table",
        r#"{"type": "Table", "props": {"columns": ["Check", "Status"], "rows": [["clippy", "pass"], ["signing", "pending"]]}}"#,
    ),
    (
        "KeyValueList",
        r#"{"type": "KeyValueList", "props": {"items": [{"key": "Region", "value": "eu-west"}, {"key": "Hosts", "value": "12"}]}}"#,
    ),
    (
        "Metric",
        r#"{"type": "Metric", "props": {"label": "p99", "value": "180 ms", "trend": "down", "delta": "-20 ms"}}"#,
    ),
    (
        "MetricGrid",
        r#"{"type": "MetricGrid", "props": {"items": [{"label": "Tests", "value": "412 / 412"}, {"label": "Coverage", "value": "81%", "trend": "up"}]}}"#,
    ),
    (
        "Progress",
        r#"{"type": "Progress", "props": {"value": 7, "max": 12, "label": "hosts migrated"}}"#,
    ),
    (
        "Bars",
        r#"{"type": "Bars", "props": {"items": [{"label": "build", "value": 41}, {"label": "tests", "value": 130}]}}"#,
    ),
    (
        "Timeline",
        r#"{"type": "Timeline", "props": {"items": [{"when": "Mon", "title": "schema migration", "state": "done"}, {"when": "Wed", "title": "canary 5%", "detail": "eu-west first", "state": "active"}, {"when": "Fri", "title": "full traffic"}]}}"#,
    ),
    (
        "Checklist",
        r#"{"type": "Checklist", "props": {"items": [{"text": "schema migration", "done": true}, {"text": "full traffic", "done": false}]}}"#,
    ),
    (
        "Tabs",
        r#"{"type": "Tabs", "props": {"labels": ["Summary", "Checks"]}, "children": ["a", "b"]}"#,
    ),
    (
        "Accordion",
        r#"{"type": "Accordion", "props": {"titles": ["Why", "How"]}, "children": ["a", "b"]}"#,
    ),
    (
        "Graph",
        r#"{"type": "Graph", "props": {"direction": "LR", "groups": [{"id": "edge", "label": "Edge"}], "nodes": [{"id": "web", "label": "Browser", "kind": "actor", "group": "edge"}, {"id": "api", "label": "API", "kind": "rounded", "state": "active"}, {"id": "db", "label": "Postgres", "kind": "cylinder"}], "edges": [{"from": "web", "to": "api", "label": "HTTPS"}, {"from": "api", "to": "db", "style": "dashed"}]}}"#,
    ),
];

/// The `## Show, don't tell` prompt block (doc §6.2) with the compact catalog filled in.
pub fn prompt_block() -> String {
    format!(
        "## Show, don't tell\n\
Your reply can draw. Two fenced blocks render as pictures in the terminal and the desktop app, and read as text everywhere else:\n\
- ```mermaid — sequenceDiagram, stateDiagram-v2, classDiagram, erDiagram (and flowchart when you need mermaid's own syntax). Use it when the answer is an order of messages, which states lead where, or a class or data model.\n\
- ```json-render — one JSON object {{root, elements:{{id:{{type, props, children}}}}}}. Use it when the answer is structured data: a comparison, a status board, before/after, metrics, a plan with states, a timeline — and for boxes and arrows: architecture, dependencies, data flow, a pipeline are a `Graph` (nodes, edges, groups, a state per node); it draws on every surface and an app can highlight it. Catalog: {catalog}\n\
Draw when a reader would sketch it on a whiteboard to explain it; that is most architecture, flow, dependency, sequence, state and comparison answers. One picture per idea, ≤ 12 nodes or ≤ 8 rows; labels ≤ 4 words; put the takeaway in one sentence before the block and do not repeat in prose what the picture already shows. Do not draw a one-line answer, a list of two, code, or anything the user asked to copy or paste. Prose is still right for reasoning, caveats and instructions.\n\
Every block must be complete and closed. In json-render every `children` id must be an element, every `type` must be in the catalog, JSON must be strict (double quotes, no trailing commas, no comments). If a block fails to render you will receive a <surface_error> message with the exact path of each error: answer with the corrected block only.",
        catalog = catalog_compact()
    )
}

/// The `<surface_error>` message (doc §5) for the fences [`crate::check`] rejected.
pub fn surface_error(fences: &[(usize, Vec<SpecError>)]) -> String {
    let mut out = format!(
        "<surface_error>\nYour reply contains {n} `json-render`/`mermaid` block(s) that cannot be rendered. \
The user saw the raw source instead. Reply with the corrected block(s) only — no apology, no explanation \
of the fix, keep any prose that was already correct out of this reply.\n",
        n = fences.len()
    );
    for (ordinal, errs) in fences {
        out.push_str(&format!("\nBlock {ordinal} (json-render), errors:\n"));
        for e in errs {
            out.push_str(&format!("- {}: {}\n", e.path, e.message));
        }
    }
    out.push_str(&format!(
        "\nCatalog: {}\n</surface_error>",
        catalog_compact()
    ));
    out
}

/// The `visualize` skill: `visualize.md` with its `## Catalog` section generated from the
/// component schemas — `### Name`, one line of props, one example element each.
pub fn visualize_skill() -> String {
    let mut section = String::new();
    for (e, (name, example)) in catalog().entries.iter().zip(EXAMPLES) {
        debug_assert_eq!(e.name, name);
        let props = fields(&e.props);
        let children = if CONTAINERS.contains(&name) {
            "; children: element ids"
        } else {
            ""
        };
        section.push_str(&format!(
            "### {name}\nProps: {}{children}\n```json\n{example}\n```\n\n",
            if props.is_empty() {
                "none".to_owned()
            } else {
                props
            }
        ));
    }
    SKILL.replace("{{catalog}}", section.trim_end())
}

/// `text: string, level?: integer 1..4 (default 2)` — required props first in declaration
/// order, optional (`?`) after, sorted; the default read off the schema description.
fn fields(schema: &Value) -> String {
    let required: Vec<&str> = schema["required"]
        .as_array()
        .map(|a| a.iter().filter_map(Value::as_str).collect())
        .unwrap_or_default();
    let Some(props) = schema["properties"].as_object() else {
        return String::new();
    };
    let mut optional: Vec<&str> = props
        .keys()
        .map(String::as_str)
        .filter(|k| !required.contains(k))
        .collect();
    optional.sort_unstable();
    required
        .iter()
        .map(|k| (*k, ""))
        .chain(optional.iter().map(|k| (*k, "?")))
        .map(|(k, opt)| {
            let p = &props[k];
            let default = p["description"]
                .as_str()
                .and_then(|d| d.split_once("default "))
                .map(|(_, d)| format!(" (default {})", d.trim_end_matches('.').trim_matches('`')))
                .unwrap_or_default();
            format!("{k}{opt}: {}{default}", ty(p))
        })
        .collect::<Vec<_>>()
        .join(", ")
}

/// A prop schema as the model should read it: `string`, `up|down|flat`, `integer 1..6`,
/// `{key: string, value: string}[]`; `Option<T>` is `T`.
fn ty(s: &Value) -> String {
    if let Some(any) = s["anyOf"].as_array() {
        return any
            .iter()
            .find(|m| m["type"] != "null")
            .map_or_else(|| "any".to_owned(), ty);
    }
    if let Some(one) = s["oneOf"].as_array() {
        return one
            .iter()
            .filter_map(|m| m["const"].as_str())
            .collect::<Vec<_>>()
            .join("|");
    }
    let t = match &s["type"] {
        Value::String(t) => t.as_str(),
        Value::Array(a) => a
            .iter()
            .filter_map(Value::as_str)
            .find(|t| *t != "null")
            .unwrap_or("any"),
        _ => "any",
    };
    match t {
        "array" => format!("{}[]", ty(&s["items"])),
        "object" => format!("{{{}}}", fields(s)),
        "integer" if s["minimum"].is_number() => {
            format!("integer {}..{}", s["minimum"], s["maximum"])
        }
        other => other.to_owned(),
    }
}

#[cfg(test)]
mod tests {
    use serde_json::json;

    use super::*;
    use crate::{check, parse};

    #[test]
    fn prompt_block_names_every_component() {
        let block = prompt_block();
        assert!(block.starts_with("## Show, don't tell\n"));
        for e in &catalog().entries {
            assert!(block.contains(&format!("{}(", e.name)), "{}", e.name);
        }
        assert!(block.contains("<surface_error>"));
    }

    #[test]
    fn surface_error_lists_every_fence_and_path() {
        let fences = vec![
            (
                1,
                vec![
                    SpecError {
                        path: "/elements/t/type".into(),
                        message: "unknown component \"DataTable\"".into(),
                    },
                    SpecError {
                        path: "/root".into(),
                        message: "\"main\" is not an element id".into(),
                    },
                ],
            ),
            (
                3,
                vec![SpecError {
                    path: "/elements/r/children/1".into(),
                    message: "\"m2\" is not an element id".into(),
                }],
            ),
        ];
        let msg = surface_error(&fences);
        assert!(msg.starts_with("<surface_error>\nYour reply contains 2 `json-render`"));
        assert!(msg.ends_with("</surface_error>"));
        assert!(msg.contains("\nBlock 1 (json-render), errors:\n- /elements/t/type: unknown component \"DataTable\"\n- /root: \"main\" is not an element id\n"));
        assert!(msg.contains("\nBlock 3 (json-render), errors:\n- /elements/r/children/1: \"m2\" is not an element id\n"));
        assert!(msg.contains(&format!("\nCatalog: {}\n", catalog_compact())));
    }

    #[test]
    fn skill_is_generated_and_valid() {
        let skill = visualize_skill();
        assert!(skill.starts_with("---\nname: visualize\n"));
        assert!(!skill.contains("{{"), "unfilled placeholder");
        for e in &catalog().entries {
            assert!(skill.contains(&format!("\n### {}\n", e.name)), "{}", e.name);
        }
        assert!(skill.contains("### Timeline\nProps: items: {when: string, title: string, detail?: string, state?: done|active|pending (default pending)}[]\n"));
        assert!(
            skill.contains("### Heading\nProps: text: string, level?: integer 1..4 (default 2)\n")
        );
        assert!(skill.contains("### Stack\nProps: direction?: vertical|horizontal (default vertical), gap?: none|sm|md|lg; children: element ids\n"));
        assert!(skill.contains("### Separator\nProps: none\n"));
        // Every worked example in the skill validates; the catalog examples are checked below.
        let errs = check(&skill);
        assert!(errs.is_empty(), "{errs:?}");
    }

    #[test]
    fn every_catalog_example_parses() {
        for (name, example) in EXAMPLES {
            let el: Value = serde_json::from_str(example).unwrap_or_else(|e| panic!("{name}: {e}"));
            assert_eq!(el["type"], name);
            let mut elements = serde_json::Map::new();
            for c in el["children"].as_array().into_iter().flatten() {
                elements.insert(
                    c.as_str().unwrap().to_owned(),
                    json!({"type": "Text", "props": {"text": "…"}}),
                );
            }
            elements.insert("x".into(), el);
            let body = json!({"root": "x", "elements": elements}).to_string();
            parse(&body).unwrap_or_else(|e| panic!("{name}: {e:?}"));
        }
    }
}
