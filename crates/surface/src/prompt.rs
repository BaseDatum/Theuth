//! What the model reads about surfaces (design doc 30 §5, §6): the `RegisterPrompt` block,
//! the `<surface_error>` correction message and the generated `visualize` skill.

use serde_json::Value;

use crate::FailedFence;
use crate::catalog::{catalog, catalog_compact};
use crate::validate::CONTAINERS;

/// The `visualize` skill template; `{{catalog}}` is filled by [`visualize_skill`].
const SKILL: &str = include_str!("../visualize.md");

/// One minimal element per component, in catalog order; the `### Name` example in the skill.
/// Containers reference children `a`/`b`, which the example leaves to the reader.
const EXAMPLES: [(&str, &str); 18] = [
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
];

/// The `## Show, don't tell` prompt block (doc §6.2) with the compact catalog filled in.
pub fn prompt_block() -> String {
    format!(
        "## Show, don't tell\n\
Your reply can draw. Three fenced blocks render as pictures in the terminal and the desktop app, and read as text everywhere else:\n\
- ```mermaid — flowchart, sequenceDiagram, stateDiagram-v2, classDiagram, erDiagram. Use it when the answer is a shape: how things connect, what calls what, what happens in what order, which states lead where.\n\
- ```json-render — one JSON object {{root, elements:{{id:{{type, props, children}}}}}}. Use it when the answer is structured data: a comparison, a status board, before/after, metrics, a plan with states, a timeline. Catalog: {catalog}\n\
- ```openui — OpenUI Lang, the same statements as the `app` tool, for a richer static card in the reply: Tabs, Steps, Callout, MetricIndicatorInline, Card, Table, charts. `$state`, `@Set`, `@OpenUrl` and inputs work in place; do not use Query, Mutation or @ToAssistant in a fence (the user can open it as an app). Use it for a multi-step explanation (Steps), options with trade-offs (Tabs), a status update (MetricIndicatorInline + Callout), instead of paragraphs.\n\
Draw when a reader would sketch it on a whiteboard to explain it; that is most architecture, flow, dependency, sequence, state and comparison answers. One picture per idea, ≤ 12 nodes or ≤ 8 rows; labels ≤ 4 words; put the takeaway in one sentence before the block and do not repeat in prose what the picture already shows. Do not draw a one-line answer, a list of two, code, or anything the user asked to copy or paste. Prose is still right for reasoning, caveats and instructions.\n\
Every block must be complete and closed. In json-render every `children` id must be an element, every `type` must be in the catalog, JSON must be strict (double quotes, no trailing commas, no comments). If a block fails to render you will receive a <surface_error> message with the exact path of each error: answer with the corrected block only.",
        catalog = catalog_compact()
    )
}

/// The `<surface_error>` message (doc §5) for the fences that still fail this round. Each
/// block is named by kind and ordinal with its located errors; the json-render catalog line
/// is appended only when a json-render block is among them (openui failures carry their own
/// signatures from the checker; mermaid needs neither).
pub fn surface_error(fences: &[FailedFence]) -> String {
    let mut out = format!(
        "<surface_error>\nYour reply contains {n} `json-render`/`mermaid`/`openui` block(s) that cannot be rendered. \
The user was shown nothing in its place. Reply with the corrected block(s) only — no apology, no explanation \
of the fix, keep any prose that was already correct out of this reply.\n",
        n = fences.len()
    );
    for f in fences {
        out.push_str(&format!("\nBlock {} ({}), errors:\n", f.ordinal, f.lang));
        for e in &f.errors {
            out.push_str(&format!("- {}: {}\n", e.path, e.message));
        }
    }
    if fences.iter().any(|f| f.lang == "json-render") {
        out.push_str(&format!("\nCatalog: {}\n", catalog_compact()));
    }
    out.push_str("</surface_error>");
    out
}

/// The note after the last correction round (doc §5 as-built): the model moves on in prose
/// and is not checked again for this reply.
pub fn surface_error_final(attempts: u8) -> String {
    format!(
        "<surface_error final=\"true\">\nThe block still does not render after {attempts} attempts. \
Do not try again in this reply and do not mention the failure; continue with prose. \
You may attempt a diagram again later in the conversation.\n</surface_error>"
    )
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
    use crate::{SpecError, check, parse};

    #[test]
    fn prompt_block_names_every_component() {
        let block = prompt_block();
        assert!(block.starts_with("## Show, don't tell\n"));
        for e in &catalog().entries {
            assert!(block.contains(&format!("{}(", e.name)), "{}", e.name);
        }
        assert!(block.contains("<surface_error>"));
        assert!(block.contains("\n- ```openui — OpenUI Lang, the same statements as the `app` tool"));
        assert!(block.contains("do not use Query, Mutation or @ToAssistant in a fence"));
    }

    #[test]
    fn surface_error_lists_every_fence_and_path() {
        let e = |path: &str, message: &str| SpecError {
            path: path.into(),
            message: message.into(),
        };
        let fences = vec![
            FailedFence {
                lang: "json-render".into(),
                ordinal: 1,
                errors: vec![
                    e("/elements/t/type", "unknown component \"DataTable\""),
                    e("/root", "\"main\" is not an element id"),
                ],
            },
            FailedFence {
                lang: "mermaid".into(),
                ordinal: 1,
                errors: vec![e("line:2", "Parse error on line 2\nline 2: A --> B[")],
            },
        ];
        let msg = surface_error(&fences);
        assert!(msg.starts_with("<surface_error>\nYour reply contains 2 `json-render`"));
        assert!(msg.ends_with("</surface_error>"));
        assert!(msg.contains("\nBlock 1 (json-render), errors:\n- /elements/t/type: unknown component \"DataTable\"\n- /root: \"main\" is not an element id\n"));
        assert!(msg.contains("\nBlock 1 (mermaid), errors:\n- line:2: Parse error on line 2\nline 2: A --> B[\n"));
        assert!(msg.contains(&format!("\nCatalog: {}\n", catalog_compact())));
        // No json-render among the failures: no json-render catalog.
        let msg = surface_error(&fences[1..]);
        assert!(!msg.contains("Catalog:"), "{msg}");
        let last = surface_error_final(3);
        assert!(last.starts_with("<surface_error final=\"true\">\n"));
        assert!(last.contains("after 3 attempts") && last.ends_with("</surface_error>"));
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
