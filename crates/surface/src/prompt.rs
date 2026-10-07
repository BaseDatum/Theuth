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
const EXAMPLES: [(&str, &str); 57] = [
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
    (
        "Sequence",
        r#"{"type": "Sequence", "props": {"title": "One fence", "actors": [{"id": "model", "label": "Model"}, {"id": "parse", "label": "surface::parse"}, {"id": "validate", "label": "validate"}], "steps": [{"from": "model", "to": "parse", "label": "fence body", "ref": "crates/surface/src/lib.rs#L90-L112"}, {"from": "parse", "to": "validate", "label": "check(value)", "ref": "crates/surface/src/validate.rs#L32"}, {"from": "validate", "to": "parse", "label": "Spec | errors", "style": "return"}]}}"#,
    ),
    (
        "Steps",
        r#"{"type": "Steps", "props": {"items": [{"title": "Install the CLI", "detail": "brew install theuth"}, {"title": "Sign in"}, {"title": "Run the first session"}]}}"#,
    ),
    (
        "Callout",
        r#"{"type": "Callout", "props": {"title": "Faster builds", "text": "Use cargo check while iterating.", "variant": "tip"}}"#,
    ),
    (
        "Code",
        r#"{"type": "Code", "props": {"lang": "toml", "title": "Cargo.toml", "text": "[dependencies]\nserde = \"1\""}}"#,
    ),
    (
        "Diff",
        r#"{"type": "Diff", "props": {"from": "v1.2", "to": "v1.3", "before": "timeout = 30", "after": "timeout = 60"}}"#,
    ),
    (
        "Excerpt",
        r#"{"type": "Excerpt", "props": {"path": "crates/surface/src/lib.rs", "start": 90, "lang": "rust", "symbol": "parse", "text": "pub fn parse(body: &str) -> Result<Parsed, Vec<SpecError>> {\n    if body.len() > MAX_FENCE_BYTES {\n        return Err(vec![err(\n            \"\",", "mark": [[91, 92]], "caption": "the byte cap runs before any JSON work"}}"#,
    ),
    (
        "Tree",
        r#"{"type": "Tree", "props": {"items": [{"label": "crates/"}, {"label": "surface/", "depth": 1, "detail": "catalog"}, {"label": "spec.rs", "depth": 2}, {"label": "docs/"}]}}"#,
    ),
    (
        "Glossary",
        r#"{"type": "Glossary", "props": {"items": [{"term": "Fence", "text": "A triple-backtick block with a language tag."}, {"term": "Surface", "text": "What a fence renders into."}]}}"#,
    ),
    (
        "Quote",
        r#"{"type": "Quote", "props": {"text": "Show first, then tell.", "source": "design doc 30"}}"#,
    ),
    (
        "Image",
        r#"{"type": "Image", "props": {"src": "https://example.com/arch.png", "alt": "Architecture diagram", "caption": "Three crates, one bus"}}"#,
    ),
    (
        "LinkList",
        r#"{"type": "LinkList", "props": {"items": [{"title": "Design doc 30", "url": "https://example.com/docs/30", "note": "Surfaces"}, {"title": "Tracking issue", "url": "https://example.com/issues/67"}]}}"#,
    ),
    (
        "Tags",
        r#"{"type": "Tags", "props": {"items": [{"text": "rust"}, {"text": "reviewed", "variant": "success"}, {"text": "blocked", "variant": "destructive"}]}}"#,
    ),
    (
        "Gauge",
        r#"{"type": "Gauge", "props": {"value": 72, "max": 100, "label": "Disk used"}}"#,
    ),
    (
        "Sparkline",
        r#"{"type": "Sparkline", "props": {"label": "p99 ms", "values": [180, 175, 190, 210, 160, 150]}}"#,
    ),
    (
        "Heatmap",
        r#"{"type": "Heatmap", "props": {"rows": ["Mon", "Tue"], "cols": ["build", "test", "deploy"], "cells": [[3, 12, 1], [0, 8, 2]]}}"#,
    ),
    (
        "Person",
        r#"{"type": "Person", "props": {"name": "Ada Lovelace", "role": "Release owner", "contact": "ada@example.com"}}"#,
    ),
    (
        "FileList",
        r#"{"type": "FileList", "props": {"items": [{"path": "crates/surface/", "kind": "dir"}, {"path": "crates/surface/src/spec.rs", "kind": "rust", "size": "31 KB"}]}}"#,
    ),
    (
        "Kanban",
        r#"{"type": "Kanban", "props": {"columns": [{"title": "Todo", "cards": [{"title": "Golden files", "tag": "tests"}]}, {"title": "Doing", "cards": [{"title": "Renderer arms", "detail": "17 to go"}]}, {"title": "Done", "cards": [{"title": "Spec types"}]}]}}"#,
    ),
    (
        "Calendar",
        r#"{"type": "Calendar", "props": {"title": "Rollout week", "days": [{"day": "Mon 22", "label": "schema", "state": "done"}, {"day": "Wed 24", "label": "canary", "state": "active"}, {"day": "Fri 26", "label": "full", "state": "pending"}]}}"#,
    ), // ---- 123 diagram grammars (Appendix A, verbatim) ----
    (
        "StateMachine",
        r#"{"type": "StateMachine", "props": {"states": [{"id": "todo", "label": "Todo", "kind": "initial"}, {"id": "done", "label": "Done", "kind": "final"}], "transitions": [{"from": "todo", "to": "done", "label": "finish", "guard": "checks green"}]}}"#,
    ),
    (
        "Entities",
        r#"{"type": "Entities", "props": {"entities": [{"id": "user", "label": "User", "fields": [{"name": "id", "type": "uuid", "key": "pk"}]}, {"id": "session", "label": "Session", "fields": [{"name": "user_id", "type": "uuid", "key": "fk"}]}], "relations": [{"from": "user", "to": "session", "kind": "one-many", "from_field": "id", "to_field": "user_id"}], "detail": "schema"}}"#,
    ),
    (
        "Swimlane",
        r#"{"type": "Swimlane", "props": {"lanes": [{"id": "dev", "label": "Developer"}, {"id": "ci", "label": "CI"}], "steps": [{"id": "push", "lane": "dev", "label": "Push", "kind": "start"}, {"id": "test", "lane": "ci", "label": "Run tests"}], "edges": [{"from": "push", "to": "test"}]}}"#,
    ),
    (
        "GraphDelta",
        r#"{"type": "GraphDelta", "props": {"before": {"nodes": [{"id": "api", "label": "API"}, {"id": "db", "label": "Postgres"}], "edges": [{"from": "api", "to": "db"}]}, "after": {"nodes": [{"id": "api", "label": "API"}, {"id": "cache", "label": "Redis"}, {"id": "db", "label": "Postgres"}], "edges": [{"from": "api", "to": "cache"}, {"from": "cache", "to": "db"}]}}}"#,
    ),
    (
        "BarChart",
        r#"{"type": "BarChart", "props": {"labels": ["Mon", "Tue", "Wed"], "series": [{"name": "Tests", "values": [12, 18, 9]}, {"name": "Fails", "values": [1, 0, 2]}], "unit": "runs"}}"#,
    ),
    (
        "LineChart",
        r#"{"type": "LineChart", "props": {"labels": ["v1", "v2", "v3"], "series": [{"name": "p95 ms", "values": [410, 380, 290]}], "yLabel": "ms"}}"#,
    ),
    (
        "ScatterChart",
        r#"{"type": "ScatterChart", "props": {"points": [{"x": 2, "y": 40, "label": "api"}, {"x": 5, "y": 12, "label": "worker"}], "xLabel": "Effort", "yLabel": "Value"}}"#,
    ),
    (
        "RadarChart",
        r#"{"type": "RadarChart", "props": {"axes": ["Speed", "Cost", "Safety", "Coverage"], "series": [{"name": "Plan A", "values": [4, 2, 5, 3]}], "max": 5}}"#,
    ),
    (
        "Waterfall",
        r#"{"type": "Waterfall", "props": {"steps": [{"label": "Start", "value": 100, "kind": "total"}, {"label": "Hires", "value": 12}, {"label": "Leavers", "value": -5}, {"label": "End", "value": 107, "kind": "total"}], "unit": "people"}}"#,
    ),
    (
        "Funnel",
        r#"{"type": "Funnel", "props": {"stages": [{"label": "Visited", "value": 1200}, {"label": "Signed up", "value": 300}, {"label": "Paid", "value": 45}]}}"#,
    ),
    (
        "Gantt",
        r#"{"type": "Gantt", "props": {"tasks": [{"id": "a", "label": "Kernel", "start": "2026-10-08", "end": "2026-10-10"}, {"id": "b", "label": "Charts", "start": "2026-10-10", "end": "2026-10-14", "depends": ["a"], "state": "active"}], "today": "2026-10-11"}}"#,
    ),
    (
        "Quadrant",
        r#"{"type": "Quadrant", "props": {"xLabel": "Effort", "yLabel": "Impact", "items": [{"label": "Cache", "x": 0.2, "y": 0.8, "state": "active"}, {"label": "Rewrite", "x": 0.9, "y": 0.7}]}}"#,
    ),
    (
        "Treemap",
        r#"{"type": "Treemap", "props": {"items": [{"label": "agent", "value": 362, "group": "crates"}, {"label": "ui", "value": 210, "group": "desktop"}]}}"#,
    ),
    (
        "Sankey",
        r#"{"type": "Sankey", "props": {"nodes": [{"id": "opus", "label": "Opus", "stage": 0}, {"id": "s1", "label": "Session 1", "stage": 1}], "flows": [{"from": "opus", "to": "s1", "value": 12.5}], "unit": "USD"}}"#,
    ),
    (
        "Venn",
        r#"{"type": "Venn", "props": {"sets": [{"id": "a", "label": "Has tests"}, {"id": "b", "label": "Has docs"}], "overlaps": [{"sets": ["a", "b"], "label": "Release-ready"}]}}"#,
    ),
    (
        "Loop",
        r#"{"type": "Loop", "props": {"steps": [{"label": "Measure"}, {"label": "Decide"}, {"label": "Build"}, {"label": "Ship"}], "hub": "Learning"}}"#,
    ),
    (
        "Layers",
        r#"{"type": "Layers", "props": {"layers": [{"label": "Apps", "items": ["desktop", "TUI"]}, {"label": "Engine", "items": ["agent", "tools"]}, {"label": "Store", "items": ["sqlite"]}]}}"#,
    ),
    (
        "Journey",
        r#"{"type": "Journey", "props": {"stages": ["Install", "First chat"], "rows": [{"label": "Does", "cells": ["downloads", "asks"]}], "mood": [0.2, 0.8], "pains": [{"stage": 0, "text": "notarisation wait"}]}}"#,
    ),
    (
        "Fishbone",
        r#"{"type": "Fishbone", "props": {"effect": "Slow replies", "causes": [{"category": "Model", "items": ["long context"]}, {"category": "Tools", "items": ["cold ELK"]}]}}"#,
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
This chat renders pictures, and a reader takes in a picture in one glance where the same content as prose takes ten. **Default to showing.** Most substantive replies contain at least one rendered block; a reply that is only paragraphs and bullets is the exception you chose for a reason, not the habit. Words are for what a picture cannot carry: the reason, a caveat, an exact value the reader must copy, an instruction, the decision you need from them.\n\
\n\
Three fenced blocks render inline, merged into your reply with the same margins as your prose (the terminal draws json-render and mermaid; other channels read every block as text):\n\
- ```json-render: structured data, boxes-and-arrows, and the small explanatory pieces. Catalog: ";

/// Everything of the block after the catalog.
const BLOCK_TAIL: &str = "\n\
- ```openui: richer static components with local interaction, rendered inline exactly like json-render (not as an app card; the reader can open one as an app if they want): Steps/StepsItem, Tabs/TabItem, Callout, Card/CardHeader, Table/Col, BarChart/LineChart/Series, MetricIndicatorInline, ListBlock/ListItem, Tag, Accordion/AccordionItem, CodeBlock, Chips/ChipItem, OptionCards/OptionCard, Buttons/Button with @Set/@OpenUrl. No Query/Mutation/@ToAssistant in a fence — a live screen is the `app` tool.\n\
- ```mermaid: sequenceDiagram (un-anchored sketches), mindmap (flowchart only when you need mermaid syntax; boxes and arrows are otherwise a Graph; state, class and ER pictures are StateMachine / Entities).\n\
- A **page** (`show{content, name: \"x.html\"}`): one self-contained HTML document with its own `<style>`/`<script>`, CDN libraries allowed, rendered live inline in the theuth app in a sandboxed frame themed like the chat. Only for what no catalog component draws: a chart library's chart (Chart.js, ECharts, Plotly, D3), a map, 3D, a mockup, an image collage, an animation, a layout you must control pixel by pixel. Interfaces are the standing case: when the work is designing or changing a screen, dialog, form, dashboard or component — in theuth or in any product — mock it as a page (`show{content, name: \"mock.html\"}`) before or while you build it, so the reader sees the proposed screen in the chat and reacts to a picture, not a description: fake data, real layout, the theme variables, states or variants side by side. A clickable flow with live data is an `app`; the faithful look of a screen is a page. Nothing comes back from a page — a button that should reach you is an `app`, not a page. Rules: fluid width, no horizontal padding on the outermost element, no outer card, border or banner title (the page is part of your reply); charts at fixed pixel heights; let content set the height, never `100vh` or `height:100%` on `html`/`body`; use the theme variables (`--background --foreground --muted --card --border --primary --accent --chart-1..6 --radius --font-sans --font-mono`, the same names as T3 Code) with fallbacks, e.g. `var(--foreground, #1a1612)`; local images by absolute path are inlined for you; pass `height` only to cap a long page. Check a scripted page with `page_preview` before you show it. The reader sees the page where the card is — do not announce or restate it; add only what it does not say.\n\
\n\
Pick by the shape of the answer:\n\
| The answer is… | Draw |\n\
|---|---|\n\
| components, dependencies, data flow, pipeline, call graph | json-render Graph (state per node for health/progress) |\n\
| how code in this repository works, or what a change did, in three or more parts | a board (see Boards) — not an inline fence |\n\
| who calls whom in what order | mermaid sequenceDiagram |\n\
| lifecycle, modes, retries, transitions | json-render StateMachine (kind: initial/final, guard per transition) |\n\
| schema, types, classes, relationships | json-render Entities (detail: model / schema / class) |\n\
| who does which step; handoffs | json-render Swimlane |\n\
| what a change did to a topology | json-render GraphDelta (before / after share ids) |\n\
| how-to or explanation in steps | json-render Steps, or openui Steps |\n\
| options with trade-offs, alternatives | openui Tabs (one per option), or json-render Table on shared criteria |\n\
| a comparison, before/after, a matrix | json-render Table (Badge for verdict cells); two versions of text or config: Diff; rows × columns of intensity: Heatmap |\n\
| status of work, checks, tests, a rollout | json-render MetricGrid + Checklist (or Progress); work in stages: Kanban |\n\
| what happened when: incident, history | json-render Timeline (state per item; scale: time for real dates); days of a week or month: Calendar |\n\
| tasks over dates; a plan with overlap | json-render Gantt (depends, group, today) |\n\
| quantities by category or over time | json-render BarChart / LineChart (Bars for a quick list; Sparkline for a trend without axes) |\n\
| numbers, counts, durations, sizes | json-render MetricGrid; one value against a limit: Gauge; a trend inline: Metric with spark |\n\
| two variables; position; priority | json-render ScatterChart / Quadrant |\n\
| where a quantity goes; what adds up | json-render Sankey / Waterfall / Treemap / Funnel |\n\
| causes of one effect; a cycle; stacked levels | json-render Fishbone / Loop / Layers |\n\
| an experience across stages; overlap of sets | json-render Journey / Venn |\n\
| a warning, a blocker, the one thing to act on | json-render Alert (severity) |\n\
| an aside: a tip, a note, a question, a must-know | json-render Callout or openui Callout |\n\
| facts about one thing (config, host, PR) | json-render KeyValueList |\n\
| terms and what they mean | json-render Glossary |\n\
| a snippet the reader will read, not run | json-render Code (copy button) — commands to run stay a plain code block |\n\
| files, directories, a repository layout | json-render FileList (flat) or Tree (nested) |\n\
| labels, categories, a set of states | json-render Tags |\n\
| references, further reading | json-render LinkList |\n\
| a picture you have a URL for | json-render Image |\n\
| an interface you are designing or changing — a screen, dialog, form, dashboard, component | an HTML mock as a page: `show{content, name: \"mock.html\"}` — fake data, real layout, variants side by side; a clickable flow with live data is an `app` |\n\
| a chart from a chart library, a map, 3D, a mockup, a collage, an animation | a page: `show{content, name: \"x.html\"}` (check with `page_preview`) — never for what a component above draws |\n\
| who: an owner, a contact, a reviewer | json-render Person |\n\
| something someone said, verbatim | json-render Quote |\n\
\n\
Shapes for the replies you write most:\n\
- **Finished work / a subagent's result:** one sentence on the outcome → MetricGrid (files, tests, cost/time) + Checklist of what was done and verified → a short prose line per caveat or open item.\n\
- **Investigation / root cause:** one-sentence verdict → Graph or sequenceDiagram marking where it breaks (state: \"danger\") → Timeline if timing matters → prose for evidence you must quote exactly.\n\
- **Plan or proposal:** Steps, or Gantt when tasks have dates (Calendar for days) → Table of risks/choices → the question for the user in words.\n\
- **\"How does X work\":** Graph or sequenceDiagram first, then prose only for what the arrows don't say — unless X is code in this repository and the answer has three or more parts: that is a board (see Boards).\n\
- **Status check / \"how's it going\":** MetricGrid + Checklist/Progress; prose only for what changed or is stuck.\n\
\n\
Before you send, scan your draft. Three or more parallel bullets, a pipe table of statuses or comparisons, numbers in a sentence, \"first… then… finally\", \"if X then Y state\", \"A vs B\": turn each into its block and delete the prose it replaces. Keep a markdown table or list only when the reader will copy it, or when it is plain reference data with no status, verdict or number to see.\n\
\n\
Rules: one picture per idea, and a reply may hold several ideas; up to ~12 nodes or ~15 rows per block (split beyond that); labels ≤ 4 words; the takeaway goes in one sentence before the block, and prose never narrates what the picture already shows. Do not draw a one-line answer, commands to run, or text the user asked to copy. In a voice-note reply, draw nothing.\n\
Closed vocabularies — write them exactly: every diagram unit's `state` is default | active | done | muted | danger (danger marks the break; one or two `active` units at most); `direction` is LR | TB; Entities `key` is pk | fk | uk and `relations[].kind` is one-one | one-many | many-many | inherits | implements | composes | aggregates | depends | references; Callout `variant` is note | tip | question | important; Table cells are strings. Every prop sits inside `props`.\n\
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
        "integer" if s["minimum"].is_number() && s["maximum"].is_number() => {
            format!("integer {}..{}", s["minimum"], s["maximum"])
        }
        "integer" if s["minimum"].as_f64().is_some_and(|m| m > 0.0) => {
            format!("integer ≥ {}", s["minimum"])
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
        assert!(block.contains("No Query/Mutation/@ToAssistant in a fence"));
        assert!(block.contains("rendered inline exactly like json-render"));
        assert!(block.contains("| terms and what they mean | json-render Glossary |"));
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

    /// 120 §10.1: the page paragraph sits after the three fences and before the pick table,
    /// and the table has the page row after `Image`.
    #[test]
    fn block_names_pages_after_the_fences() {
        let block = prompt_block();
        let mermaid = block.find("\n- ```mermaid: ").unwrap();
        let page = block
            .find("\n- A **page** (`show{content, name: \"x.html\"}`)")
            .unwrap();
        let pick = block.find("\nPick by the shape of the answer:").unwrap();
        assert!(mermaid < page && page < pick);
        assert!(block.contains("a button that should reach you is an `app`, not a page"));
        assert!(block.contains("Interfaces are the standing case"));
        assert!(block.contains("| an interface you are designing or changing — a screen, dialog, form, dashboard, component |"));
        assert!(block.contains("Check a scripted page with `page_preview` before you show it."));
        let image = block
            .find("| a picture you have a URL for | json-render Image |")
            .unwrap();
        let row = block
            .find("| a chart from a chart library, a map, 3D, a mockup, a collage, an animation | a page: `show{content, name: \"x.html\"}` (check with `page_preview`) — never for what a component above draws |")
            .unwrap();
        let person = block
            .find("| who: an owner, a contact, a reviewer |")
            .unwrap();
        assert!(image < row && row < person);
        let skill = visualize_skill();
        assert!(skill.contains("\n## Pages\n"));
        assert!(skill.contains("| a chart library's chart, a map, 3D, a mockup, a collage, an animation | a page — show{content, name: \"x.html\"} after page_preview | a Code block of HTML, or an app that calls nothing back |\n| an explanation with three or more parts"));
        assert!(skill.contains("```html\n<!doctype html>"));
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
        assert!(msg.contains(
            "\nBlock 1 (mermaid), errors:\n- line:2: Parse error on line 2\nline 2: A --> B[\n"
        ));
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
        assert!(skill.contains("### Timeline\nProps: items: {when: string, title: string, detail?: string, id?: string, ref?: string, state?: done|active|pending (default pending), until?: string}[], scale?: list|time, title?: string\n"), "{skill}");
        assert!(
            skill.contains("### Heading\nProps: text: string, level?: integer 1..4 (default 2)\n")
        );
        assert!(skill.contains("### Stack\nProps: direction?: vertical|horizontal (default vertical), gap?: none|sm|md|lg; children: element ids\n"));
        assert!(skill.contains("### Separator\nProps: none\n"));
        // Every worked example in the skill validates; the catalog examples are checked below.
        let errs = check(&skill);
        assert!(errs.is_empty(), "{errs:?}");
        // 123 §8.1: the skill loads on demand, the cap is discipline — over it, move prose to
        // the design doc, never cut the routing table. 123 measured 48,036 B at its cap of 48 KiB;
        // design 120's pages section (54fe3155) joined the same skill afterwards (51,350 B), so the
        // cap is now 56 KiB.
        assert!(skill.len() < 56 * 1024, "skill is {} bytes", skill.len());
        // 123 §8.1 (g): the six grammar examples are real elements of their component, so a
        // model copying one copies something the validator accepts.
        let roots: Vec<String> = crate::fences::json_render_fences(&skill)
            .map(|(_, body)| {
                let v: Value = serde_json::from_str(body).unwrap();
                v["elements"][v["root"].as_str().unwrap()]["type"]
                    .as_str()
                    .unwrap()
                    .to_owned()
            })
            .collect();
        for name in [
            "StateMachine",
            "Entities",
            "Swimlane",
            "Gantt",
            "GraphDelta",
            "Sankey",
        ] {
            assert!(
                roots.iter().any(|r| r == name),
                "no worked example for {name}: {roots:?}"
            );
        }
    }

    /// 123 §3: every one of the 44 taxonomy types routes somewhere in "Pick the diagram".
    #[test]
    fn skill_routes_every_taxonomy_type() {
        let skill = visualize_skill();
        let (_, rest) = skill.split_once("\n## Pick the diagram\n").unwrap();
        let (table, _) = rest.split_once("\n## Behavioural triggers\n").unwrap();
        let types = [
            "Architecture:",
            "Architecture delta",
            "IT current-state",
            "Flowchart",
            "Sequence:",
            "State machine",
            "ER / data model",
            "Timeline:",
            "Swimlane:",
            "Quadrant",
            "Radar",
            "Polar",
            "Loop",
            "Nested",
            "Tree:",
            "Org chart",
            "Layer stack",
            "Exploded axonometric",
            "Axonometric plan",
            "Venn",
            "Pyramid / funnel",
            "Bar / dumbbell",
            "Waterfall",
            "Treemap",
            "Heatmap",
            "Line / slopegraph / bump",
            "Gantt",
            "Scatter / bubble / beeswarm",
            "High-level",
            "Process",
            "Medallion",
            "Data flow",
            "DP integration",
            "DP security matrix",
            "Sankey",
            "Fishbone",
            "Wardley map",
            "Kanban",
            "User journey",
            "Deployment",
            "Dependency graph",
            "UML class",
            "Story map",
            "Database schema",
        ];
        assert_eq!(types.len(), 44);
        for t in types {
            assert!(table.contains(&format!("\n| {t}")), "{t}");
        }
        assert_eq!(table.matches("\n| ").count() - 1, 44, "header row excluded");
        let (_, rest) = rest.split_once("\n## Behavioural triggers\n").unwrap();
        let (triggers, _) = rest.split_once("\n## Budgets\n").unwrap();
        assert_eq!(triggers.matches("\n| ").count() - 1, 9);
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
