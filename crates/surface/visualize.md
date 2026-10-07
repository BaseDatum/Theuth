---
name: visualize
description: When and how to draw in a reply — Mermaid diagram types, the json-render catalog with full prop schemas, worked examples, what not to draw. Load before a reply that explains structure, flow or a comparison.
---

# Visualize: show first, then tell

A picture is read in one glance; the same content as prose is read in ten.
When the thing you are explaining has a shape, give it the shape. Default to
showing: most substantive replies carry at least one rendered block, and a
reply of only paragraphs is the exception. Words are for what a picture cannot
carry. All three fence kinds (`json-render`, `openui`, `mermaid`) render inline
in the reply, merged with the prose; an `openui` fence is not an app card.

## Choose the block

| The answer is… | Draw | Not |
|---|---|---|
| components, dependencies, data flow, a pipeline, a call graph | `json-render` `Graph` (`direction: "TB"` for a hierarchy; a state per node for health/progress) | a bullet list of arrows, or a mermaid flowchart |
| who calls whom in what order | `sequenceDiagram` | numbered prose steps |
| participants exchanging messages over time, each step tied to code | `json-render` `Sequence` (`ref` on every step that runs code) | a `sequenceDiagram` whose arrows cannot point at a file |
| a lifecycle, modes, retries, transitions | `stateDiagram-v2` | "when X it becomes Y, unless…" |
| schema, types, relationships | `erDiagram` or `classDiagram` | field lists |
| a how-to or an explanation in steps | `json-render` `Steps`, or `openui` `Steps` | "first… then… finally" |
| options with trade-offs, alternatives | `openui` `Tabs` (one per option), or `json-render` `Table` on shared criteria | three paragraphs |
| a comparison, before/after, a matrix | `json-render` `Table` (`Badge` for verdict cells); two versions of text: `Diff`; rows × columns of intensity: `Heatmap` | "A vs B" prose |
| status of work, checks, tests, a rollout | `MetricGrid` + `Checklist` (or `Progress`); work in stages: `Kanban` | "we have done A and B; C is pending" |
| what happened when: incident, history, plan | `Timeline` (state per item); a week or month of dates: `Calendar` | headers + bullets |
| numbers, counts, durations, sizes | `MetricGrid` or `Bars`; one value against a limit: `Gauge`; a trend without axes: `Sparkline` or `Metric.spark`; `openui` `BarChart` / `LineChart` for a series | numbers buried in sentences |
| a warning, a blocker, the one thing to act on | `json-render` `Alert` (severity) | a bold sentence in the middle of a paragraph |
| an aside: a tip, a note, a question, a must-know | `json-render` `Callout` (variant) or `openui` `Callout` | a parenthesis |
| facts about one thing (a config, a host, a PR) | `json-render` `KeyValueList` | a colon-separated list |
| terms and their meanings | `Glossary` | "X means…; Y means…" |
| a snippet to read, not run | `Code` (copy button); commands to run stay a plain code block | inline backticks for six lines |
| a passage of real code with its location | `Excerpt` (`path`, 1-based `start`, `mark` the rows that carry the claim, `symbol`); text from `read_file`, never retyped | a `Code` block with the path in `title` |
| files, directories, a repository layout | `FileList` (flat) or `Tree` (nested by `depth`) | a bullet list of paths |
| labels, categories, states | `Tags` | comma-separated words |
| references, further reading | `LinkList` | bare URLs in a sentence |
| a picture you have a URL for | `Image` (`http(s)` or `data:image`) | a link the reader must click |
| an owner, a contact, a reviewer | `Person` | "talk to Ada (release owner, ada@…)" |
| something someone said, verbatim | `Quote` | quotation marks inside a paragraph |
| a schedule, a chronology, a topic map | mermaid `gantt`, `timeline`, `mindmap` | dated bullets |
| a chart library's chart, a map, 3D, a mockup, a collage, an animation | a page — show{content, name: "x.html"} after page_preview | a Code block of HTML, or an app that calls nothing back |
| an explanation with three or more parts, or one the reader will return to | a board (board.md) | a long reply of fences and prose |

Boxes and arrows — architecture, dependencies, data flow, a pipeline — are a `Graph` in a
json-render block; it draws on every surface and an app can highlight it. Messages between
actors whose steps point at code are a `Sequence`. Mermaid stays for un-anchored sequence
sketches, state, class and ER diagrams, gantt, timeline and mindmap, and for anything that
needs mermaid's own syntax.

## Shapes for common replies

- **Finished work / a subagent's result:** one sentence on the outcome → `MetricGrid` (files, tests, cost/time) + `Checklist` of what was done and verified → a short prose line per caveat or open item.
- **Investigation / root cause:** one-sentence verdict → `Graph` or `sequenceDiagram` marking where it breaks (`state: "error"`) → `Timeline` if timing matters → prose for evidence you must quote exactly.
- **Plan or proposal:** `Steps` or `Timeline` → `Table` of risks/choices → the question for the user in words.
- **"How does X work":** `Graph` or `sequenceDiagram` first, then prose only for what the arrows don't say.
- **Status check / "how's it going":** `MetricGrid` + `Checklist`/`Progress`; prose only for what changed or is stuck.

Do not draw: one fact; code or commands; anything the user will copy; a
diagram of the diagram you are about to show. In a voice-note reply, draw
nothing. Never describe the picture in prose — one sentence of takeaway above
it, then the block.

## Mermaid styling: outline, never fill
Diagrams render on light and dark themes and on phones; filled node backgrounds fight the theme's text color and become unreadable. Encode meaning in the **stroke**, never the fill:
- Every `classDef` uses `fill:none` (or `fill:transparent`), a colored `stroke`, and `stroke-width:2.5px`. Add `stroke-dasharray:6 3` for a secondary/soft category (e.g. advisory or non-gating).
- Palette (mid-saturation, legible on both themes): red `#dc2626` = human gate / blocker, blue `#2563eb` = mutating action, amber `#d97706` = warning / non-gating, green `#16a34a` = verified / done, gray `#6b7280` = external or out of scope. Two or three classes per diagram, not five.
- In `sequenceDiagram`, use `rect rgba(0,0,0,0)` for stage grouping so the band is a boundary, not a tint; notes carry the label.
- Never rely on color alone: put the category into the node text (PREFLIGHT, HEALTH — shim, exit 2) and finish with a one-line legend: "red outline = gates · blue outline = mutations · amber dashed = non-gating".

Example:
    classDef gate fill:none,stroke:#dc2626,stroke-width:2.5px;
    classDef soft fill:none,stroke:#d97706,stroke-width:2.5px,stroke-dasharray:6 3;
    classDef mut  fill:none,stroke:#2563eb,stroke-width:2.5px;
    class PRE,CONF gate;
    class HLT,SMK soft;
    class AUR,K8S,DNS mut;

## Mermaid rules that avoid errors
- Quote labels that contain `(`, `)`, `[`, `]`, `{`, `}`, `:` or `|`: `A["fetch(url)"]`.
- Syntax traps that break the parser: no double quotes inside `[...]` node labels (write `set 0`, not `"0"`), no `;` inside any label or note (mermaid reads it as end-of-statement), and no `(`/`)` inside `([...])` stadium labels.
- Node ids are bare words; `end` is reserved — write `End` or `finish`.
- Edge labels: `A -->|label| B` or `A -- label --> B`, no spaces inside `|…|` delimiters.
- One diagram, one direction; ≤ 12 nodes; split otherwise.
- `subgraph name … end` for grouping; `%%` for comments; no HTML in labels.

## json-render rules that avoid errors
- Exactly `{ "root": "<id>", "elements": { … } }`; nothing else at the top.
- Every id in any `children` list is a key of `elements`; each element appears in at most one `children`.
- `type` is one of the catalog names below, spelled exactly; `props` match its schema; unknown props are errors.
- Only `Card`, `Stack`, `Grid`, `Tabs` and `Accordion` take `children`; `Tabs.labels` and `Accordion.titles` have one entry per child.
- Strict JSON: double quotes, no trailing commas, no comments, no `True`/`None`.
- Prefer flat: a `Card` with a `Table` beats nested `Stack`s. ≤ 256 elements.
- `Graph`: every `edges[].from`/`to` and `nodes[].group` names an existing `nodes[].id` / `groups[].id`; ids unique; ≤ 120 nodes, ≤ 240 edges, ≤ 12 groups; labels ≤ 80 characters. `from == to` is a self-loop.
- `Heatmap.cells` has one row per `rows` entry and one value per `cols` entry. `Image.src` is `http(s)://` or `data:image/`; `LinkList` urls are `http(s)://`.
- `Sequence`: 2–12 `actors` with unique ids, 1–40 `steps`; every `steps[].from`/`to` names an `actors[].id`; labels ≤ 80 characters.
- `Excerpt`: `path` is repository-relative (no leading `/`, no `..`, no `http:`); `text` is 1–120 lines and ≤ 16 KiB; every `mark` pair `[from, to]` is ordered and inside `start..start+lines-1`.
- `ref` (on `Graph` nodes and `Sequence` steps): `path`, `path#L10` or `path#L10-L24` — repository-relative, 1-based, inclusive — or `block:<id>`. No `..`, no scheme, no URL.

## Catalog
<!-- generated from crates/surface: names, props, one example each -->
Each entry is one element of `elements`; `?` marks an optional prop.

{{catalog}}

## Worked examples
### Architecture answer
Takeaway sentence, then:
```json-render
{ "root": "g", "elements": {
  "g": {"type":"Graph","props":{
    "direction":"LR",
    "groups":[{"id":"clients","label":"Clients"},{"id":"core","label":"Core"}],
    "nodes":[
      {"id":"tui","label":"TUI","kind":"actor","group":"clients"},
      {"id":"desktop","label":"Desktop","kind":"actor","group":"clients"},
      {"id":"rpc","label":"RPC","kind":"rounded","group":"core"},
      {"id":"agent","label":"Agent","group":"core","state":"active"},
      {"id":"tools","label":"Tools","group":"core"},
      {"id":"store","label":"sessions.sqlite","kind":"cylinder"}],
    "edges":[
      {"from":"tui","to":"agent","label":"UserInput"},
      {"from":"desktop","to":"rpc","label":"ws RPC"},
      {"from":"rpc","to":"agent"},
      {"from":"agent","to":"tools"},
      {"from":"agent","to":"store","style":"dashed"}]}} } }
```
### Status board
```json-render
{ "root": "c", "elements": {
  "c": {"type":"Card","props":{"title":"Rollout · eu-west"},"children":["p","l"]},
  "p": {"type":"Progress","props":{"value":7,"max":12,"label":"hosts migrated"}},
  "l": {"type":"Checklist","props":{"items":[
    {"text":"schema migration","done":true},
    {"text":"canary 5%","done":true},
    {"text":"full traffic","done":false}]}} } }
```
### Comparison
```json-render
{ "root": "t", "elements": {
  "t": {"type":"Table","props":{
    "columns":["", "SQLite", "Postgres", "DynamoDB"],
    "rows":[
      ["Setup", "none", "a server", "an account"],
      ["Concurrency", "one writer", "many", "many"],
      ["Query power", "SQL", "SQL + extensions", "key/value"],
      ["Fits here", "yes", "later", "no"]]}} } }
```
### Sequence
```mermaid
sequenceDiagram
  participant U as User
  participant A as Agent
  participant T as Tool
  U->>A: request
  A->>T: call(args)
  alt approved
    T-->>A: result
    A-->>U: reply
  else denied
    A-->>U: asks for approval
  end
```
### Lifecycle
```mermaid
stateDiagram-v2
  [*] --> Idle
  Idle --> Running: input
  Running --> Running: tool round
  Running --> Idle: reply
  Running --> Cancelled: Esc
  Cancelled --> Idle
```
### Metrics
```json-render
{ "root": "m", "elements": {
  "m": {"type":"MetricGrid","props":{"items":[
    {"label":"Requests / s","value":"1 240","trend":"up","delta":"+12%"},
    {"label":"Error rate","value":"0.4%","trend":"down","delta":"-0.2 pt"},
    {"label":"p99","value":"180 ms","trend":"flat"},
    {"label":"Hosts","value":"12"}]}} } }
```

## Pages

A page is the escape hatch, picked last: catalog → `openui`/`app` → page. Reach for it only
for what no catalog component draws — a chart library's chart (Chart.js, ECharts, Plotly, D3),
a map, 3D, a mockup, an image collage, an animation, a layout you must control pixel by pixel.
`show{content, name: "x.html", height?}` renders one self-contained HTML document — its own
`<style>`/`<script>`, CDN libraries allowed — live inline in the theuth app (desktop, web,
phone) in a sandboxed frame themed like the chat; Slack/Telegram/Signal get the file, the TUI
the source. Nothing comes back from a page: a button that should reach you is an `app`, not a
page.

Rules:
- Fluid width; no horizontal padding on the outermost element; no outer card, border or banner
  title — the page is part of your reply.
- Charts at fixed pixel heights. Let content set the height; never `100vh` or `height:100%` on
  `html`/`body`. Pass `height` only to cap a long page (80–2000; the rest scrolls inside).
- Local images by absolute path (`<img src="/tmp/shot.png">`) are inlined for you, ≤ 10 MiB
  each, ≤ 25 MiB per page; a missing or non-image file fails the `show`.
- Use the theme variables with fallbacks (`var(--foreground, #1a1612)`) or set your own `:root`
  defaults: the stored file has no theme when opened outside theuth.
- Check a scripted page with `page_preview` before you show it; it returns the content height
  and the console. The reader sees the page where the card is — do not announce or restate it;
  add only what it does not say.

Theme variables (the same names as T3 Code; every value is a resolved colour):
`--background --foreground --muted --muted-foreground --card --card-foreground --popover
--popover-foreground --secondary --secondary-foreground --border --input --ring --primary
--primary-foreground --accent --accent-foreground --accent-surface --accent-surface-foreground
--destructive --destructive-foreground --destructive-surface --warning --warning-foreground
--warning-surface --success --success-foreground --info --info-foreground --code-background
--code-foreground --chart-1 --chart-2 --chart-3 --chart-4 --chart-5 --chart-6 --radius
--font-sans --font-mono`.

Example — a Chart.js bar chart from the CDN, no outer card, fallbacks for every variable used:
```html
<!doctype html>
<meta charset="utf-8">
<style>
  :root { --foreground: #1a1612; --muted-foreground: #6b6560; --chart-1: #b8860b; --font-sans: system-ui, sans-serif; }
  body { margin: 0; color: var(--foreground); font: 14px/1.5 var(--font-sans); }
</style>
<canvas id="c" height="320"></canvas>
<script src="https://cdn.jsdelivr.net/npm/chart.js@4"></script>
<script>
  const css = (v, d) => getComputedStyle(document.documentElement).getPropertyValue(v).trim() || d;
  new Chart(document.getElementById("c"), { type: "bar",
    data: { labels: ["Q1", "Q2", "Q3"], datasets: [{ label: "Revenue", data: [12, 19, 7], backgroundColor: css("--chart-1", "#b8860b") }] },
    options: { responsive: true, maintainAspectRatio: false, color: css("--muted-foreground", "#6b6560") } });
</script>
```
