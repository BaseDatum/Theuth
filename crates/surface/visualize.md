---
name: visualize
description: When and how to draw in a reply — the 44-type routing table onto the json-render diagram grammars, behavioural triggers, budgets, the taste gate, which fence (json-render / openui / app / board), the catalog with full prop schemas, worked examples, what not to draw. Load before a reply that explains structure, flow, a lifecycle, a schema, a plan or a quantity.
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
| a lifecycle, modes, retries, transitions | `json-render` `StateMachine` (`kind: initial|final`, `guard`) | `stateDiagram-v2`; "when X it becomes Y, unless…" |
| schema, types, classes, relationships | `json-render` `Entities` (`detail: model|schema|class`) | `erDiagram`/`classDiagram`; field lists |
| who does which step; handoffs | `json-render` `Swimlane` (lanes = owners) | a flowchart with the owner in every label |
| what a change did to a topology | `json-render` `GraphDelta` (`before`/`after` share ids) | two `Graph`s the reader reconciles |
| a how-to or an explanation in steps | `json-render` `Steps`, or `openui` `Steps` | "first… then… finally" |
| options with trade-offs, alternatives | `openui` `Tabs` (one per option), or `json-render` `Table` on shared criteria | three paragraphs |
| a comparison, before/after, a matrix | `json-render` `Table` (`Badge` for verdict cells); two versions of text: `Diff`; rows × columns of intensity: `Heatmap` | "A vs B" prose |
| status of work, checks, tests, a rollout | `MetricGrid` + `Checklist` (or `Progress`); work in stages: `Kanban` | "we have done A and B; C is pending" |
| what happened when: incident, history | `Timeline` (state per item; `scale: time` for real dates); a week or month: `Calendar` | headers + bullets |
| tasks over dates; a plan with overlap | `json-render` `Gantt` (`depends`, `group`, `today`) | dated bullets, mermaid `gantt` |
| quantities by category or over time | `json-render` `BarChart` / `LineChart` (`Bars` for a quick list; `Sparkline` without axes) | numbers buried in sentences |
| numbers, counts, durations, sizes | `MetricGrid`; one value against a limit: `Gauge`; a trend inline: `Metric.spark` | a sentence of numbers |
| two variables; position; priority | `json-render` `ScatterChart` / `Quadrant` | "high effort, high impact" prose |
| where a quantity goes; what adds up | `json-render` `Sankey` / `Waterfall` / `Treemap` / `Funnel` | percentages in a paragraph |
| causes of one effect; a cycle; stacked levels | `json-render` `Fishbone` / `Loop` / `Layers` | a nested bullet list |
| an experience across stages; overlap of sets | `json-render` `Journey` / `Venn` | a story in paragraphs |
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
| a topic map, a brainstorm | mermaid `mindmap` | nested bullets |
| a chart library's chart, a map, 3D, a mockup, a collage, an animation | a page — show{content, name: "x.html"} after page_preview | a Code block of HTML, or an app that calls nothing back |
| an explanation with three or more parts, or one the reader will return to | a board (board.md) | a long reply of fences and prose |

Boxes and arrows — architecture, dependencies, data flow, a pipeline — are a `Graph` in a
json-render block; it draws on every surface and an app can highlight it. Messages between
actors whose steps point at code are a `Sequence`; a state, class or ER picture is a
`StateMachine`/`Entities` for the same reason. Mermaid stays for un-anchored sequence
sketches, `mindmap`, and anything that needs mermaid's own syntax.

## Pick the diagram

Type (the diagram-design taxonomy) → component + the prop that makes it that diagram:

| Showing | Component | Carries the type |
|---|---|---|
| Architecture: components + connections | `Graph` | `groups` as tiers/zones, `tag` (DB, QUEUE), `sub` (port), `icon` |
| Architecture delta: before/after topology | `GraphDelta` | `before`/`after` share ids; the ledger is derived |
| IT current-state: legacy by phase/department | `Graph` `TB` or `Swimlane` | `state: muted` legacy / `danger` end-of-life; or `lanes` = departments |
| Flowchart: decisions with branches | `Graph` `TB` | `kind: circle` start/end, `diamond` decisions, yes/no edge labels |
| Sequence: messages between actors | `Sequence` | `fragments` (`alt|opt|loop|par`), `steps[].note` |
| State machine: states, transitions, guards | `StateMachine` | `kind: initial|final|choice`, `transitions[].guard` |
| ER / data model: entities and relations | `Entities` `detail: model` | `fields[].key`, `relations[].kind: one-many…` |
| Timeline: events positioned in time | `Timeline` `scale: time` | ISO `when`, `until` spans |
| Swimlane: cross-functional handoffs | `Swimlane` | `lanes`, `steps[].lane`; a cross-lane edge is the handoff |
| Quadrant / consultant 2×2 | `Quadrant` | `items[].x/y` 0..1; `quadrants` names the cells (no items = pure 2×2) |
| Radar / spider: entities on 3–5 criteria | `RadarChart` | `axes`, one `series` per entity, `max` |
| Polar: one series over cyclic categories | `RadarChart` `variant: polar` | exactly one series, radial bars |
| Loop / flywheel | `Loop` | `steps` clockwise, `hub` centred |
| Nested: hierarchy as containment | `Tree` `layout: nested` | `depth` drawn as boxes in boxes |
| Tree: parent → children | `Tree` `layout: nodes` | `depth` → connected boxes |
| Org chart: ownership, reporting | `Tree` `layout: org` | `detail` = role line |
| Layer stack: abstraction levels | `Layers` | `layers[].items`, `direction` |
| Exploded axonometric | not drawn | `show` an SVG/PNG made elsewhere |
| Axonometric plan | not drawn | same |
| Venn: overlap between sets | `Venn` | 2–3 `sets`, `overlaps` by set ids |
| Pyramid / funnel: ranking or drop-off | `Funnel` `variant: funnel|pyramid` | `stages[].value` for drop-off, none for a ranking |
| Bar / dumbbell | `BarChart` | `variant: grouped|stacked|dumbbell|horizontal`; `Bars` for a quick list |
| Waterfall: start total → end total | `Waterfall` | `steps[].kind: delta|total|subtotal`; running total computed |
| Treemap (marimekko) | `Treemap` | `items[].value`, `group`; a marimekko is `BarChart` stacked |
| Heatmap: cross-tabulated values | `Heatmap` | `palette: diverging` for signed cells |
| Line / slopegraph / bump | `LineChart` | `variant: line|area|slope|bump|step` (ridgeline, streamgraph: prose) |
| Gantt: tasks on dates | `Gantt` | `start/end`, `group` bands, `depends`, `today` |
| Scatter / bubble / beeswarm | `ScatterChart` | `variant: scatter|bubble|beeswarm`; `size`, `groups` |
| High-level: an end-to-end stack | `Graph` `TB` | one group per tier |
| Process: multi-actor steps with data handoffs | `Swimlane` (`Steps` for one actor) | `steps[].kind: data` = the handed-over artifact |
| Medallion: storage tiers | `Layers` | one layer per tier, `note` = quality level |
| Data flow: who does what per step | `Swimlane` or `Graph` `LR` | `lanes` = roles, `kind: data` steps |
| DP integration: sources → core → consumers | `Graph` `LR` | three `groups` |
| DP security matrix: role × component | `Table` (✓ / — / RW) or `Heatmap` | nothing new |
| Sankey: a quantity splitting and merging | `Sankey` | `flows[].value` = width, `nodes[].stage` = column |
| Fishbone: causes of one effect | `Fishbone` | `effect`, `causes[].category/items` |
| Wardley map | `ScatterChart` + prose | `groups` = evolution stage; links in words |
| Kanban: WIP by state | `Kanban` | `columns[].limit`, `cards[].state: blocked` |
| User journey: does / feels per stage | `Journey` | `stages`, `rows`, `mood`, `pains` |
| Deployment: zones, hosts, replicas, ports | `Graph` | `groups[].parent` (zone › host › pod), `sub` (`:8080 ×3`), `tag` |
| Dependency graph: fan-in, cycles | `Graph` | cycles allowed; nothing new |
| UML class | `Entities` `detail: class` | `methods`, `relations[].kind: inherits|implements|composes|aggregates|depends` |
| Story map: backbone sliced into releases | `Kanban` | `rows` (releases), `cards[].row`, `cut` |
| Database schema: SQL types, keys, FKs | `Entities` `detail: schema` | `fields[].type/key/nullable`, `from_field`/`to_field` |

If a 3-column table says the same thing, use the table. If two types seem to fit, pick the
dominant axis; a pattern below may add primitives, never a second layout.

## Behavioural triggers

When behaviour, state, enforcement or risk carries the meaning, pick the pattern first, then
its diagram:

| Trigger | Recipe |
|---|---|
| Fan-in, queue depth, finite capacity, a bottleneck | `Graph` `LR`: sources → one `kind: hexagon` queue node with `sub` capacity (`8/hour`, `3 slots`) → the service → two outcome edges (`active` admitted, `muted` deferred) |
| The same question/input/governance/output slots repeated per stage | `Table` stages × slots with explicit `—` cells; `Swimlane` only when the rows are owners |
| Loose input becomes a structured durable artifact | `Graph` `LR`: source → named transformation → artifact; provenance as ≤ 2 `kind: note` nodes |
| Two rule traces: pass / fail / skipped / not reached, first divergence | `Table` rule × trace with `PASS` / `FAIL` / `SKIPPED` / `NOT REACHED` + an `Alert` naming the first divergence |
| Trust boundaries with permitted and forbidden paths | `Graph` with `groups[].kind: boundary` (dashed); permitted edges `active`, forbidden `danger` with `arrow: none` stopping at the boundary |
| Controls grouped by where they are enforced | `Layers`: one layer per enforcement surface, `items` = controls, `note` = actor and timing |
| Defences that compensate for prior gaps; residual risk | `Layers` top-down, `note` = residual risk, `state: danger` on the layer with the gap |
| An ID-addressable decomposition that must link to code | `Tree` `layout: nodes` with `ref` on every node |
| One subject through phases, waits, retries, cancellation, terminal outcomes | `StateMachine`: 4–5 phases `LR`, wait/retry states `muted`, terminal states `kind: final` |

## Budgets

Editorial limits, not validator caps. Above the budget, split into an overview and a detail.

| Diagram | Budget |
|---|---|
| any | 9 nodes, 12 arrows or transitions, ≤ 2 `active` units, ≤ 2 notes |
| `GraphDelta` | 8 components, 10 edges, 8 ledger entries |
| `Sequence` | 5 actors, 1 fragment (2 if each is one `opt`/`loop`), 2 notes |
| `Swimlane` / `Layers` / `Funnel` / `Venn` | 5 lanes / 6 layers / 6 stages / 3 sets |
| `Entities` | 8 entities; `class` 7 × 5 members; `schema` 5 tables, 8 fields shown, 6 FKs |
| `Tree` | depth 4; org 12 nodes; nested 6 levels |
| `RadarChart` | 5 axes, 5 series, 1 `active`; polar 8 categories |
| `BarChart` / `Waterfall` / `Treemap` | 8 bars / 8 incl. totals, 1 subtotal / 8 cells |
| `LineChart` / `ScatterChart` / `Quadrant` | 5 series / 30 points / 12 items |
| `Gantt` / `Sankey` | 12 tasks / 3 stages, 8 nodes, 12 flows |
| `Fishbone` / `Journey` | 6 bones × 3 items / 6 stages, 3 rows, 2 pains |
| `Kanban` | 5 columns, 12 cards (4 per column); story map 5 × 3 releases |
| `Graph` deployment / dependency | 3 zones, 6 nodes, 8 paths / 9 nodes, 14 edges, 1 cycle |

## Taste gate

Before the block leaves the draft:

- **Remove test.** Can a node go? Can two that always travel together merge? Is an arrow obvious from the layout? Does colour or shape already say what a label says? Remove it.
- **Would a table or a sentence do?** A list of things is a `Table` or bullets; attribute-only before/after is a `Table`; a one-shape diagram is a sentence.
- **One accent.** `active` on ≤ 2 units; it is editorial, not a flag. Everything else `default`, `muted`, `done`, `danger` by meaning.
- **Legend.** Covers every `state` and `kind` the picture uses, and nothing more (`Graph.legend`, else one line under the block).
- **Labels ≤ 4 words**, never vertical text; the detail goes in `sub`, `note`, `detail` or the tooltip, not the label.
- **Within budget** (table above) — otherwise overview + detail.

## Which fence

- **`json-render`** — a static picture: every diagram above. Draws on every surface (SVG on desktop, text on the TUI, Slack, Signal); a board or app can highlight its units.
- **`openui` fence** — the reader flips `Tabs`/`Steps` or picks `OptionCards` locally, no live data (`Query`/`Mutation`/`@ToAssistant` are inert there).
- **`app`** — live data feeds the props (`Query`), a press must do something (`Action`), or the reader steps through states (`$step` → `highlight`). Recipes: the apps skill.
- **a board** — three or more parts about code in this repository, `ref` on every unit (board.md).

In an `app` or `openui` fence, `BarChart`, `LineChart`, `ScatterChart` and `RadarChart` are the
upstream recharts components with their own signatures; the grammars of those names described
here are json-render and board components. The other fifteen diagrams are apps components under
the same names, `highlight` and `onSelect` appended after the catalog props.

## Shapes for common replies

- **Finished work / a subagent's result:** one sentence on the outcome → `MetricGrid` (files, tests, cost/time) + `Checklist` of what was done and verified → a short prose line per caveat or open item.
- **Investigation / root cause:** one-sentence verdict → `Graph` or `sequenceDiagram` marking where it breaks (`state: "danger"`) → `Timeline` if timing matters → prose for evidence you must quote exactly.
- **Plan or proposal:** `Steps`, or `Gantt` when tasks have dates → `Table` of risks/choices → the question for the user in words.
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
- Every diagram unit that refers to something else does so by id: `transitions[].from/to` → `states[].id`, `relations[].from/to` → `entities[].id` (`from_field`/`to_field` → that entity's `fields[].name`), `steps[].lane` → `lanes[].id`, `flows[].from/to` → `nodes[].id` (no cycles, `value > 0`), `depends[]` → `tasks[].id`; `series[].values` has one value per label; `Gantt` dates are ISO (`2026-10-08`), `end ≥ start`.
- `ref` (on every diagram unit: `Graph` nodes, `Sequence` steps, states, entities, lane steps, tasks, cells, layers…): `path`, `path#L10` or `path#L10-L24` — repository-relative, 1-based, inclusive — or `block:<id>`. No `..`, no scheme, no URL.

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
### Lifecycle (StateMachine)
```json-render
{ "root": "s", "elements": {
  "s": {"type":"StateMachine","props":{"title":"A turn","direction":"LR",
    "states":[
      {"id":"idle","label":"Idle","kind":"initial"},
      {"id":"running","label":"Running","state":"active","ref":"crates/agent/src/loop_.rs#L2561-L2584"},
      {"id":"waiting","label":"Awaiting approval","state":"muted"},
      {"id":"done","label":"Replied","kind":"final"},
      {"id":"cancelled","label":"Cancelled","kind":"final"}],
    "transitions":[
      {"from":"idle","to":"running","label":"input"},
      {"from":"running","to":"running","label":"tool round"},
      {"from":"running","to":"waiting","label":"ask","guard":"policy = ask"},
      {"from":"waiting","to":"running","label":"approved"},
      {"from":"running","to":"done","label":"reply"},
      {"from":"running","to":"cancelled","label":"Esc","style":"dashed"}]}} } }
```
### Schema (Entities)
```json-render
{ "root": "e", "elements": {
  "e": {"type":"Entities","props":{"title":"Sessions","detail":"schema",
    "entities":[
      {"id":"sessions","label":"sessions","kind":"table","fields":[
        {"name":"id","type":"text","key":"pk"},{"name":"title","type":"text","nullable":true},{"name":"created_at","type":"integer"}]},
      {"id":"turns","label":"turns","kind":"table","state":"active","fields":[
        {"name":"id","type":"integer","key":"pk"},{"name":"session_id","type":"text","key":"fk"},{"name":"role","type":"text"}]},
      {"id":"costs","label":"costs","kind":"table","fields":[
        {"name":"turn_id","type":"integer","key":"fk"},{"name":"usd","type":"real"}]}],
    "relations":[
      {"from":"sessions","to":"turns","kind":"one-many","from_field":"id","to_field":"session_id"},
      {"from":"turns","to":"costs","kind":"one-one","from_field":"id","to_field":"turn_id"}]}} } }
```
### Handoff (Swimlane)
```json-render
{ "root": "w", "elements": {
  "w": {"type":"Swimlane","props":{"title":"Release","direction":"LR",
    "lanes":[{"id":"dev","label":"Developer"},{"id":"ci","label":"CI"},{"id":"ops","label":"Ops"}],
    "steps":[
      {"id":"tag","lane":"dev","label":"Tag release","kind":"start"},
      {"id":"build","lane":"ci","label":"Build + sign","sub":"package-macos.sh"},
      {"id":"ok","lane":"ci","label":"Notarised?","kind":"decision"},
      {"id":"artifact","lane":"ci","label":"DMG","kind":"data"},
      {"id":"publish","lane":"ops","label":"Publish","state":"active"},
      {"id":"fix","lane":"dev","label":"Fix signing","state":"danger"}],
    "edges":[
      {"from":"tag","to":"build"},{"from":"build","to":"ok"},
      {"from":"ok","to":"artifact","label":"yes"},{"from":"ok","to":"fix","label":"no","style":"dashed"},
      {"from":"artifact","to":"publish"}]}} } }
```
### Plan (Gantt)
```json-render
{ "root": "p", "elements": {
  "p": {"type":"Gantt","props":{"title":"Wave 123","today":"2026-10-11",
    "tasks":[
      {"id":"c","label":"Contract","start":"2026-10-07","end":"2026-10-08","group":"step 0","state":"done","progress":1},
      {"id":"w1","label":"ELK family","start":"2026-10-08","end":"2026-10-14","group":"workers","depends":["c"],"state":"active","progress":0.4},
      {"id":"w4","label":"Skill","start":"2026-10-09","end":"2026-10-12","group":"workers","depends":["c"]},
      {"id":"close","label":"Integrate","start":"2026-10-15","end":"2026-10-17","group":"step 5","depends":["w1","w4"]}]}} } }
```
### Before / after (GraphDelta)
```json-render
{ "root": "d", "elements": {
  "d": {"type":"GraphDelta","props":{"title":"Add a cache","direction":"LR",
    "before":{
      "nodes":[{"id":"api","label":"API"},{"id":"db","label":"Postgres","kind":"cylinder"}],
      "edges":[{"from":"api","to":"db","label":"reads"}]},
    "after":{
      "nodes":[{"id":"api","label":"API"},{"id":"cache","label":"Redis","kind":"cylinder","state":"active"},{"id":"db","label":"Postgres","kind":"cylinder"}],
      "edges":[{"from":"api","to":"cache"},{"from":"cache","to":"db","label":"miss","style":"dashed"}]}}} } }
```
### Spend (Sankey)
```json-render
{ "root": "k", "elements": {
  "k": {"type":"Sankey","props":{"title":"Spend this week","unit":"USD",
    "nodes":[
      {"id":"opus","label":"Opus","stage":0},{"id":"haiku","label":"Haiku","stage":0},
      {"id":"design","label":"Design 123","stage":1,"state":"active"},{"id":"triage","label":"Bug triage","stage":1},{"id":"titles","label":"Titles","stage":1}],
    "flows":[
      {"from":"opus","to":"design","value":31.4},{"from":"opus","to":"triage","value":8.2},
      {"from":"haiku","to":"triage","value":0.9},{"from":"haiku","to":"titles","value":1.6}]}} } }
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