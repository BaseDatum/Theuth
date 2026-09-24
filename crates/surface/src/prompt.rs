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

/// The `## Show first, then tell` prompt block (doc §6.2): [`BLOCK_HEAD`], the compact
/// catalog, [`BLOCK_TAIL`]. Concatenated, not `format!`ed, so the worked example's JSON
/// braces stay literal.
pub fn prompt_block() -> String {
    [BLOCK_HEAD, &catalog_compact(), BLOCK_TAIL].concat()
}

/// Everything of the block before the catalog line's `Catalog: `.
const BLOCK_HEAD: &str = "## Show first, then tell\n\
This chat renders pictures, and a reader takes in a picture in one glance where the same content as prose takes ten. **Default to showing.** Words are for what a picture cannot carry: the reason, a caveat, an exact value the reader must copy, an instruction, the decision you need from them. Most substantive replies should contain at least one block; a reply that is only paragraphs and bullets should be the exception you chose, not the habit.\n\
\n\
Three fenced blocks render (the terminal and desktop draw them, other channels read them as text):\n\
- ```json-render: structured data and boxes-and-arrows. Catalog: ";

/// Everything of the block after the catalog.
const BLOCK_TAIL: &str = "\n\
- ```openui: richer static cards with local interaction: Steps, Tabs, Callout, Card, Table, BarChart/LineChart, MetricIndicatorInline, Buttons with @Set/@OpenUrl. No Query/Mutation/@ToAssistant in a fence.\n\
- ```mermaid: sequenceDiagram, stateDiagram-v2, classDiagram, erDiagram, gantt, timeline, mindmap (flowchart only when you need mermaid syntax; boxes and arrows are otherwise a Graph).\n\
\n\
Pick by the shape of the answer:\n\
| The answer is… | Draw |\n\
|---|---|\n\
| components, dependencies, data flow, pipeline, call graph | json-render Graph (state per node for health/progress) |\n\
| who calls whom in what order | mermaid sequenceDiagram |\n\
| lifecycle, modes, retries, transitions | mermaid stateDiagram-v2 |\n\
| schema, types, relationships | mermaid erDiagram / classDiagram |\n\
| how-to or explanation in steps | openui Steps |\n\
| options with trade-offs, alternatives | openui Tabs (one per option), or json-render Table on shared criteria |\n\
| a comparison, before/after, a matrix | json-render Table (Badge for verdict cells) |\n\
| status of work, checks, tests, a rollout | json-render MetricGrid + Checklist (or Progress) |\n\
| what happened when: incident, history, plan | json-render Timeline (state per item) |\n\
| numbers, counts, durations, sizes | json-render MetricGrid / Bars, openui BarChart / LineChart for series |\n\
| a warning, a blocker, the one thing to act on | openui Callout or json-render Alert |\n\
| facts about one thing (config, host, PR) | json-render KeyValueList |\n\
\n\
Shapes for the replies you write most:\n\
- **Finished work / a subagent's result:** one sentence on the outcome → MetricGrid (files, tests, cost/time) + Checklist of what was done and verified → a short prose line per caveat or open item.\n\
- **Investigation / root cause:** one-sentence verdict → Graph or sequenceDiagram marking where it breaks (state: \"error\") → Timeline if timing matters → prose for evidence you must quote exactly.\n\
- **Plan or proposal:** Steps or Timeline → Table of risks/choices → the question for the user in words.\n\
- **\"How does X work\":** Graph or sequenceDiagram first, then prose only for what the arrows don't say.\n\
- **Status check / \"how's it going\":** MetricGrid + Checklist/Progress; prose only for what changed or is stuck.\n\
\n\
Before you send, scan your draft. Three or more parallel bullets, a pipe table of statuses or comparisons, numbers in a sentence, \"first… then… finally\", \"if X then Y state\", \"A vs B\": turn each into its block and delete the prose it replaces. Keep a markdown table or list only when the reader will copy it, or when it is plain reference data with no status, verdict or number to see.\n\
\n\
Rules: one picture per idea, and a reply may hold several ideas; up to ~12 nodes or ~15 rows per block (split beyond that); labels ≤ 4 words; the takeaway goes in one sentence before the block, and prose never narrates what the picture already shows. Do not draw a one-line answer, code, commands, or text the user asked to copy. In a voice-note reply, draw nothing.\n\
Mermaid styling: outline, never fill. Every classDef is `fill:none,stroke:<color>,stroke-width:2.5px` (add `stroke-dasharray:6 3` for a soft category); red #dc2626 gate/blocker, blue #2563eb mutation, amber #d97706 warning/non-gating, green #16a34a done, gray #6b7280 external; at most three classes; `rect rgba(0,0,0,0)` for sequence bands; put the category in the node text and end with a one-line legend. No `\"` or `;` inside labels, no parentheses inside `([...])`. Full guide: the visualize skill.\n\
\n\
Every block must be complete and closed. In json-render every `children` id must be an element, every `type` must be in the catalog, and the JSON must be strict (double quotes, no trailing commas, no comments). If a block fails to render you will receive a <surface_error> with the exact path of each error: answer with the corrected block only.\n\
\n\
When you report finished work, a subagent's result or an investigation, report visually first as above.\n\
\n\
Worked example. Instead of:\n\
> I updated the parser and the tests. All 412 tests pass, coverage went from 78% to 81%. The migration is done and staging is deployed, but prod is still pending approval.\n\
\n\
write the one-sentence takeaway, then:\n\
```json-render\n\
{\"root\":\"r\",\"elements\":{\"r\":{\"type\":\"Stack\",\"children\":[\"m\",\"c\"]},\n\
\"m\":{\"type\":\"MetricGrid\",\"props\":{\"items\":[{\"label\":\"Tests\",\"value\":\"412/412\"},{\"label\":\"Coverage\",\"value\":\"81%\",\"delta\":\"+3%\",\"trend\":\"up\"}]}},\n\
\"c\":{\"type\":\"Checklist\",\"props\":{\"items\":[{\"text\":\"Parser updated\",\"done\":true},{\"text\":\"Staging deployed\",\"done\":true},{\"text\":\"Prod approval\",\"done\":false}]}}}}\n\
```";

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

    /// (d) of the brief: the rendered block carries the catalog, and its worked json-render
    /// example is strict JSON that the crate's own validator accepts.
    #[test]
    fn prompt_block_names_every_component_and_its_example_validates() {
        let block = prompt_block();
        assert!(block.starts_with("## Show first, then tell\n"));
        assert!(block.contains(&format!("Catalog: {}\n", catalog_compact())));
        for e in &catalog().entries {
            assert!(block.contains(&format!("{}(", e.name)), "{}", e.name);
        }
        assert!(block.contains("<surface_error>"));
        assert!(block.contains("No Query/Mutation/@ToAssistant in a fence."));
        assert!(block.contains("\nMermaid styling: outline, never fill. "));
        assert!(block.ends_with("}}}}\n```"));

        let (_, rest) = block.split_once("```json-render\n").unwrap();
        let (body, _) = rest.split_once("\n```").unwrap();
        let json: Value = serde_json::from_str(body).unwrap();
        assert_eq!(json["root"], "r");
        assert_eq!(json["elements"]["c"]["type"], "Checklist");
        parse(body).unwrap_or_else(|e| panic!("{e:?}"));
        assert!(check(&block).is_empty(), "{:?}", check(&block));
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
        assert!(skill.contains("\n## Mermaid styling: outline, never fill\n"));
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
