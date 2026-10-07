---
name: board
description: "How to draw a board — a long-form explanation of code the reader scrolls, points at and tours; the answer to 'how does X work' or 'walk me through this change' about code in the working tree. Board or chat, the fixed section order, one diagram per idea, the code-link and ref anchor grammar, Excerpt economy, block shapes, code_map/code_symbol/code_impact recipes, and the read-back that verifies every claim. Load before doc_create(kind=\"board\")."
---

Method adapted from dev.fast Whiteboard, MIT © 2026 dev.fast.

# Board: an explanation the reader can point at

You are writing an interactive RFC-style board, for consumption by a staff engineer. A board is a
note (`doc_*` tools) the reader opens as a page: `##` sections, prose whose claims about code are
links, `Excerpt` blocks that quote the few lines that carry a mechanism, and one diagram per idea
whose units point at code. The reader hovers a sentence and the diagram unit and the quoted lines
light up together; clicks open the code beside the sentence; tours a diagram stop by stop; asks
about a place on the board and you edit that place. Every one of those mechanisms works only where
you left an anchor — an un-anchored node is a picture, an un-anchored sentence is a claim the reader
cannot check.

## Board or chat

Answer in chat when one sentence or one code line does it. Draw a board when the question is about
code in the working tree and the answer needs a diagram and more than a paragraph: "how does X work",
"walk me through this change", "what did commit N change", "explain this codebase", "what does this PR
do". That board replaces the inline `Graph`/`Sequence` fence the Show-first shapes would otherwise
produce — do not draw the same picture in chat as well. A comparison of two options with no code to
point at stays inline (`Table`, `Tabs`). Keep boards short and sweet when possible (especially for
small changes); feel free to omit sections.

Every `doc_*` tool is python-only here: `r = await doc_create(...)`, `await doc_insert(...)`.

## The first steps, in this order, without other tool calls between them

1. Orient: `code_map(focus=[<the crate or directory the question names>])` (or `code_impact(git_range=…)`
   for a change). One call.
2. `doc_create(title=<what the reader asked, as a title>, kind="board", root=<absolute repository root>)`.
   `root` is the directory every `code:` link, `ref` and `Excerpt.path` is relative to. Bind the id: `b = r["doc"]`.
3. Immediately put down a first pass at the **What and why** section — heading block, then one or two
   paragraphs — with `doc_insert`. The reader is already watching.

Then research and write the rest, one block per `doc_insert`, in reading order.

## Sections, in this order

`## What and why` → `## Map` → `## How it works` → `## Decisions and trade-offs` → `## Checks` → `## Open questions`

Each is a level-2 heading block with exactly that text. Omit any that does not apply; never reorder,
never invent a seventh (`## Integration`, `## Summary`, `## Files changed` go under **How it works** as
`###`, or are dropped). Sub-headings (`###`) are free inside a section — for a diff board, one `###`
per cluster of the change (see Clusters). The `# Title` block is created for you.

- **What and why** — the question in the reader's words and the one-paragraph answer; for a change, the
  intent in the author's words (commit messages, PR text). No code yet.
- **Map** — where the mechanism lives: a `Graph` of the files or components involved, `ref` on every node,
  one sentence on the shape. ≤ 12 nodes.
- **How it works** — the mechanism, rooted at the user/agent entry point (a button click, a CLI command, a
  tool call), including unchanged hops along the way. The diagram of the section (usually a `Sequence`),
  then prose where every paragraph has at least one anchored claim, then the `Excerpt`s that carry the
  mechanism or an invariant: **three per board is the norm** (the entry point, the decision, the invariant —
  never more than three in one section), each its own `doc_insert` with the lines read by `lines()` in the
  same call. A board with no `Excerpt` is a slide deck: every claim in it costs the reader a file open.
- **Decisions and trade-offs** — why it is this way and not another; `Table` on shared criteria, `Callout`
  for the one thing to know, `Diff` for before/after of a contract.
- **Checks** — what proves it: tests by path (`code:` links), a `Checklist` of what you ran.
- **Open questions** — what you could not verify, ambiguous symbols, unresolved imports.

## Mini-boards for decisions and reviews

`escalate` to the user and a hand-over to the user's review (`task_update` / `task_move` → `review`) carry a
`board`: a short board whose reader is the person deciding or reviewing, embedded in their Decision / Review
screen with this reader (anchors, hover lighting, pinned excerpts, tour). Same grammar as above, a fixed
skeleton instead of the six sections; the tool refuses a board whose first three `##` are not these, in this order.

**Decision** — `## What you're deciding` → `## Why it matters to you` → `## The picture` → `## Options` →
`## Look at` → `## Open questions` (optional).

- **What you're deciding** — the question and its scope in two sentences.
- **Why it matters to you** — for the decider: what changes for them, what happens if nothing or if wrong, urgency, reversibility.
- **The picture** — ONE diagram of the whole situation, `ref` on every unit.
- **Options** — one `###` per option: consequence, cost/risk, reversibility, your recommendation (the buttons stay on the screen).
- **Look at** — the `Excerpt`s that carry the evidence, in reading order.

**Review** — `## What was done` → `## Why it matters to you` → `## The picture` → `## Against the criteria` →
`## Walk through` → `## Open questions` (optional).

- **What was done** — the change in the reviewer's words, two sentences.
- **Why it matters to you** — what it changes for them, the risk if it is wrong, what reverting costs.
- **The picture** — ONE diagram of the change in its surroundings, `ref` on every unit.
- **Against the criteria** — each `done_when` line with its evidence anchor (`[label](code:path#La-Lb)`).
- **Walk through** — the `Excerpt`s in reading order: what to look at and what to look for; the tour steps through them.

The lens: write for the person deciding — what, why it matters to them, the whole picture; diagram and link
heavy; short. One block per section is enough. Every diagram unit has a `ref` (`path#La-Lb`, ideally the lines an
`Excerpt` quotes, or `block:<id>`; a unit that is not code is left out or named in the label of the unit that
calls it); every claim is a link (`code:` links for code; PRs, dashboards, runbooks, invoices, artifacts as plain
markdown links the reader opens outside). `root` is the repository the anchors resolve against; a scratch
directory when there is no repository. Then pass the id: `escalate(..., board=r["doc"])` /
`task_update(..., board=r["doc"])`.

Bundles: N related decisions (ten DNS records, a set of hosts, a batch of PRs) are ONE `escalate` with `items`
(`{id, title, description?, options?, recommended?}` each, ≤ 20) and one board. Its `## Options` section then
holds one `###` per item — not per option — whose heading slug equals the item `id` (`### a-record` for
`id: "a-record"`; lowercase, spaces → `-`), with that item's consequence, cost/risk, reversibility and your
recommended pick; the Decision screen lights the item's section when the item is hovered and offers *Approve
all as recommended*. Forwarding a report's decision needs no new board: `escalate` the same task with
`question: ""` and your `recommendation` — the report's board is inherited.

A decision mini-board, six blocks — the frontmatter `doc_create` wrote, then one `doc_insert` per section:

````markdown
---
root: /Users/me/src/theuth
---

## What you're deciding

Whether the retry loop in [`fetch()`](code:crates/net/src/fetch.rs#L40-L58) keeps retrying five times or stops at three.

## Why it matters to you

Five retries doubled last month's API bill ([invoice](https://billing.example.com/2026-09)); three cuts it back and
risks a rare miss on a flaky morning. Nothing happens until you answer; either way is one line and reverts in a minute.

## The picture

```json-render
{ "root": "g", "elements": { "g": {"type": "Graph", "props": {"direction": "LR", "nodes": [
  {"id": "sync", "label": "sync()", "ref": "crates/net/src/sync.rs#L12-L30"},
  {"id": "fetch", "label": "fetch() retries ×5", "ref": "crates/net/src/fetch.rs#L40-L58", "state": "danger"},
  {"id": "get", "label": "get() → billing API", "ref": "crates/net/src/http.rs#L5-L20"}],
  "edges": [{"from": "sync", "to": "fetch", "label": "every 5 min"}, {"from": "fetch", "to": "get", "label": "up to 5 billed calls"}]}} } }
```

## Options

### Cap at 3 (recommended)
Consequence: the bill returns to August's level. Cost/risk: ~2 misses a month, retried on the next tick. Reversible: one constant.

### Keep 5
Consequence: no misses. Cost/risk: the bill stays doubled. Reversible: nothing to do.

## Look at

```json-render
{ "root": "e", "elements": { "e": {"type": "Excerpt", "props": {
  "path": "crates/net/src/fetch.rs", "start": 40, "lang": "rust", "symbol": "fetch",
  "text": "pub async fn fetch(url: &str) -> Result<Body> {\n    for attempt in 0..5 {\n        if let Ok(b) = get(url).await {\n            return Ok(b);\n        }",
  "mark": [[41, 41]], "caption": "the five is here; every attempt is a billed call"}} } }
```
````

## Write incrementally

Write small and often. The reader sees the board draw itself in real time: one paragraph or one
diagram per `doc_insert`, visible progress every few seconds, never a whole board in one call. A
heading and the paragraph under it may share one call; a diagram or an `Excerpt` is always its own.
Insert a diagram whole — the renderer draws its units in order by itself. To change a diagram
already on the board, replace its block (`doc_replace(doc, blocks=[id], markdown=…)`); do not edit
it unit by unit. `doc_insert` and `doc_replace` refuse a `json-render` block that does not validate
and return the error's path (`/props/mark/0: L3 is before L9`): fix that block and call again;
nothing was inserted.

## One diagram per idea

Pick one diagram that best shows the shape of the thing; a second picture of the same subject makes
the reader reconcile two. The routing extends the `visualize` skill's table:

| The section's shape is… | Draw |
|---|---|
| participants interacting over time — a request, a tool call, a turn | `Sequence`, `ref` on every step (a return points at the line that returns) |
| components, dependencies, data flow; branches and decisions | `Graph` (`direction: "LR"` for flow, `"TB"` for a hierarchy), `ref` on every node that is code |
| a lifecycle: states, transitions, retries, guards | `StateMachine`, `ref` on every state that is code (`kind: initial|final`, `guard` on the transition) |
| a schema, the key types, a class shape | `Entities` (`detail: schema|model|class`), `ref` on every entity; a schema change: `Entities` with `state` per table, not a `Table` + `Graph` |
| who does which step; a handoff between crates, processes or teams | `Swimlane` (lanes = owners), `ref` on every step |
| the map of a change: what it added, removed, rewired | `GraphDelta` (`before`/`after` share ids; the ledger is derived), `ref` on every `after` node |
| a plan with dates and dependencies | `Gantt`, `ref` on a task that is a file or an issue block |
| a contract: the key types or signatures of a new/changed interface | `Excerpt` with `symbol`, one per type |
| an un-anchored sketch with nothing to point at | mermaid (a picture; use only when no unit needs a `ref`) |

Lifecycles are a `StateMachine`, schemas an `Entities`, handoffs a `Swimlane`, plans a `Gantt`, a
change's topology a `GraphDelta` — every unit with `ref`, so prose, unit and code light each other
(`data-unit` is how the board finds them). Mermaid stays for un-anchored sketches. One diagram per
idea stands: pick the grammar that carries the section's shape and let it carry the section.

Diagrams draw at full column width; one picture can carry a section.

## Anchor everything

When something — a phrase in the prose, a diagram node, a sequence step — describes actual code in
the repository, always attach the code:

- **Prose**: `[label](code:path#L10-L24)` — the path relative to `root`, `#L<a>-L<b>` 1-based inclusive,
  `#L10` for one line, no range only for a whole file (Map nodes). Never `path:10` — a `:` in a path is
  refused. The label is the thing named (`parse()`, `the byte cap`, `dispatch`), not "here". A link is
  never a dead end: the reader opens the lines beside the sentence even when no `Excerpt` covers them.
- **Diagram units**: `ref` is mandatory on every `Graph` node and every `Sequence` step — a `return`
  step points at the line that returns; a node that stands for a directory points at a file in it
  (`mod.rs`, `lib.rs`, `index.ts`: a directory is not an anchor); a node that is not code (a person, a
  browser, an external service) is drawn as an actor of a `Sequence` or left out of a `Graph` — `"ref":
  "crates/surface/src/lib.rs#L90-L112"`: the bare anchor, `path#L<a>-L<b>`, with **no `code:` prefix**
  (that prefix belongs to prose links only; `code:` in a `ref` is refused as a scheme), or `block:<blockId>`
  to point at a paragraph of this board. A unit without a `ref` is drawn but cannot light up, be pinned or
  be toured — when a `ref` is refused, fix it; never drop it.
- **Excerpts**: `Excerpt(path, text, start, symbol?, lang?, caption?, mark?)` for the few spots that
  carry the mechanism or an invariant — at most three per section; link everything else inline.
  `text` is the file's lines verbatim from one `read_file(path, offset=start, limit=n, numbers=False)` call —
  `start` is the `offset` you passed, `text` is what came back (the `lines` helper below); never retyped,
  never sliced out of an earlier whole-file read (an off-by-one puts the doc comment under the wrong
  number), never taken from `code_symbol.body` (it is cut past 200 lines and may be reflowed);
  ≤ 120 lines, ≤ 16 KiB. `symbol` is the exact name of the definition that starts at `start` (from
  `code_symbol(...).definition.name` or `code_outline`); it lets the reader see `moved → L88–L101` next
  month, and a name that is not defined there makes the block refused as missing — when unsure, omit it.
  `mark` the rows that carry the claim; `caption` is the claim in one sentence, in your voice.
- **Verified ranges**: source paths are repository-relative; line numbers are 1-based and inclusive.
  Replace every example path and line with a verified source range — from `code_symbol(...).definition`
  (`path`, `line_start`, `line_end`, `name`) or from the `read_file` you just did.
- **Callers below confidence**: a `code_symbol` caller or callee with `confidence < 0.5` is not stated
  as fact — say "probably called from", or leave it out.
- No side: a board pins one working tree (`root`); before/after is a `Diff` block, never two links.

## Clusters (diff boards)

For "what does this change do", categorize every changed file into buckets before you write, leaving
nothing uncategorized: first categorize away non-implementation code — tests, docs, generated files
and lockfiles — then split what remains by the part of the design each file serves. Keep each cluster
small enough to read in one sitting; do not split a file across clusters unless it holds two unrelated
changes. One `###` per cluster inside **How it works**, tests/docs/generated first.

## Block shapes

Every diagram or excerpt is one fenced block whose language is always `json-render` — the component
name goes in `type`, never after the backticks (a ```` ```Excerpt ```` fence is refused):

    ```json-render
    { "root": "x", "elements": { "x": { "type": "…", "props": { … } } } }
    ```

Rules that avoid errors: exactly `{root, elements}` at the top; `type` spelled as in the catalog; props
match the schema, unknown props are errors; strict JSON (double quotes, no trailing commas, no comments,
no `True`/`None`); every `children` id is an element key. Full catalog: the `visualize` skill.

`Excerpt` — quoted lines with their location; `mark` rows inside `start..start+lines-1`:
```json-render
{ "root": "e", "elements": { "e": {"type": "Excerpt", "props": {
  "path": "crates/surface/src/lib.rs", "start": 90, "lang": "rust", "symbol": "parse",
  "text": "pub fn parse(body: &str) -> Result<Parsed, Vec<SpecError>> {\n    if body.len() > MAX_FENCE_BYTES {\n        return Err(vec![err(\n            \"\",",
  "mark": [[91, 92]], "caption": "the byte cap runs before any JSON work"}} } }
```

`Sequence` — 2–12 actors, 1–40 steps, `from`/`to` name actor ids, `style` `call` (default) | `return` | `async`, `ref` on every step, returns included:
```json-render
{ "root": "s", "elements": { "s": {"type": "Sequence", "props": {
  "title": "One fence",
  "actors": [{"id": "model", "label": "Model"}, {"id": "parse", "label": "surface::parse"}, {"id": "validate", "label": "validate"}],
  "steps": [
    {"from": "model", "to": "parse", "label": "fence body", "ref": "crates/surface/src/lib.rs#L90-L112"},
    {"from": "parse", "to": "validate", "label": "check(value)", "ref": "crates/surface/src/validate.rs#L32"},
    {"from": "validate", "to": "parse", "label": "Spec | errors", "style": "return", "ref": "crates/surface/src/validate.rs#L268"}]}} } }
```

`Graph` — the map or the flow; `ref` on every node that is code; node `kind` is one of `box` (default) | `rounded` | `pill` | `diamond` | `cylinder` | `actor`, `state` one of `default` | `active` | `done` | `muted` | `danger` for changed / unchanged, `direction` `LR` | `TB`, `group` by crate — no other values exist:
```json-render
{ "root": "g", "elements": { "g": {"type": "Graph", "props": {
  "direction": "LR",
  "groups": [{"id": "surface", "label": "crates/surface"}],
  "nodes": [
    {"id": "lib", "label": "lib.rs", "group": "surface", "ref": "crates/surface/src/lib.rs#L90", "state": "active"},
    {"id": "validate", "label": "validate.rs", "group": "surface", "ref": "crates/surface/src/validate.rs"},
    {"id": "render", "label": "render.rs", "group": "surface", "ref": "crates/surface/src/render.rs"}],
  "edges": [{"from": "lib", "to": "validate", "label": "check"}, {"from": "lib", "to": "render"}]}} } }
```

`Callout` — the one thing to know (`variant` `note` | `tip` | `question` | `important`):
```json-render
{ "root": "c", "elements": { "c": {"type": "Callout", "props": {"title": "One correction round", "text": "A fence that fails validation is sent back once with the error's path; the model answers with the block only.", "variant": "note"}} } }
```

`Diff` — before/after of a contract or a config; `from`/`to` name the versions:
```json-render
{ "root": "d", "elements": { "d": {"type": "Diff", "props": {"from": "before", "to": "after", "before": "pub fn check(text: &str) -> bool", "after": "pub fn check(text: &str) -> Vec<(usize, Vec<SpecError>)>"}} } }
```

`Table` — decisions on shared criteria:
```json-render
{ "root": "t", "elements": { "t": {"type": "Table", "props": {"columns": ["", "strict serde", "jsonrepair"], "rows": [["When", "first", "only after strict fails"], ["Cost", "one parse", "a second pass"], ["Risk", "rejects a trailing comma", "may repair into a different shape"]]}} } }
```

`Steps` — an ordered procedure the reader follows:
```json-render
{ "root": "p", "elements": { "p": {"type": "Steps", "props": {"items": [{"title": "Read the outline", "detail": "doc_read(doc)"}, {"title": "Replace the smallest run of blocks"}, {"title": "Read back"}]}} } }
```

## Recipes

Tools are async functions in the runtime; every `doc_*` result is a dict. Read lines with this helper so
the `read_file` header and any footer never land in an `Excerpt`:

```python
import json, os
async def lines(path, start, n):
    raw = await read_file(path=os.path.join(root, path), offset=start, limit=n, numbers=False)
    return "\n".join(l for l in raw.split("\n")[1:] if not l.startswith("[read_file"))
def block(el):  # one json-render fence around one element
    return "```json-render\n" + json.dumps({"root": "x", "elements": {"x": el}}) + "\n```"
```

**`code_map` → the Map section**
```python
m = await code_map(focus=["crates/surface"], budget=1500)          # rows: {path, line, kind, name, signature, rank}
top = sorted((r for r in m["rows"] if r["rank"] > 0), key=lambda r: -r["rank"])[:12]
nodes = [{"id": r["path"].replace("/", "_").replace(".", "_"), "label": r["path"].rsplit("/", 1)[-1],
          "group": r["path"].split("/")[1], "ref": f'{r["path"]}#L{r["line"]}'} for r in top]
groups = [{"id": g, "label": g} for g in sorted({n["group"] for n in nodes})]
edges = []  # from the imports between `top` you know from code_symbol/code_refs
await doc_insert(doc=b, markdown="## Map\n\nOne sentence on the shape.\n\n" +
                 block({"type": "Graph", "props": {"direction": "LR", "groups": groups, "nodes": nodes, "edges": edges}}))
```
What the reader gets: hover a node → every paragraph link that shares its path washes; click → the file's
lines beside the prose; **Tour** walks the twelve files in rank order.

**`code_symbol` → a pinned excerpt and its callers**
```python
s = await code_symbol(symbol="crates/surface/src/lib.rs:90"); d = s["definition"]   # {path, line_start, line_end, name, kind, signature}
n = min(d["line_end"] - d["line_start"] + 1, 60)
text = await lines(d["path"], d["line_start"], n)
await doc_insert(doc=b, markdown=block({"type": "Excerpt", "props": {
    "path": d["path"], "start": d["line_start"], "lang": "rust", "symbol": d["name"], "text": text,
    "mark": [[d["line_start"] + 1, d["line_start"] + 3]], "caption": "strict parse, then jsonrepair, then validate"}}))
callers = [c for c in s.get("callers", []) if c["refs"] and c["refs"][0]["confidence"] >= 0.5][:4]
await doc_insert(doc=b, markdown="Called from " + ", ".join(
    f'[{c["path"].rsplit("/", 1)[-1]}](code:{c["path"]}#L{c["refs"][0]["line"]})' for c in callers) + ".")
```
The caption is the claim; the marked rows are its evidence; each caller link opens its call site beside the
paragraph; `symbol` lets the reader's stale chip say `moved → L88–L101` next month.

**`code_impact` → a diff board**
```python
i = await code_impact(git_range="main..HEAD", include_tests=True)
# i["changed"]: [{path, status, symbols_changed: [{name, kind, line}]}]; i["direct"]: [{path, via, is_test}];
# i["transitive"]: [{path, depth}]; i["tests"]: [path]; i["ambiguous_skipped"], i["unresolved_imports"]
r = await doc_create(title="What this branch changes", kind="board", root=root); b = r["doc"]
# What and why — the commit messages, in the author's words
# Map — Graph: changed nodes state="danger", direct state="active", transitive state="muted", group by crate, ref on every node
# How it works — Sequence rooted at the user/agent entry point (unchanged actors included), one step per hop,
#                ref = the NEW code of that hop; one ### per cluster; Excerpt(symbol) + Diff(before=git show <base>:<path>
#                via the shell, after=lines(...)) for the symbol that carries the mechanism
# Checks — Checklist of i["tests"], done=True only for tests you ran green
# Open questions — i["ambiguous_skipped"], i["unresolved_imports"]
```

## Read-back, before you finish

The reader trusts a board only if its claims were checked. The turn is not finished until the read-back
has run — a board handed over without it is a draft. Budget for it: the read-back takes four or five
tool rounds, so start it while you still have them. Do this, every board:

1. `doc_read(doc=b, format="markdown")`.
2. For every `Excerpt`: `await lines(path, start, len(text.split("\n")))` must equal `text` exactly. If not,
   re-read and `doc_replace` the block with the current lines, or delete it.
3. For every `code:` link and every `ref`: `code_symbol(symbol="path:line")` or `read_file(path, offset=line, limit=1)`
   must return something at that place; fix the range or remove the link.
4. Headings are the six names, in order; no section holds two diagrams of the same subject; no section has
   more than three `Excerpt`s; every `Graph` node and `Sequence` step that describes code has a `ref`.
5. Fix contradictions between prose and diagram.

Then end with one sentence in chat and the board's link, `theuth://notes/<id>`. Do not paste the board
into chat. When the reader wants the board as a file, `doc_export(doc, format="pdf"|"markdown")` hands
it over as a card (diagrams as text, excerpts as numbered code); the app's own Export menu is the one
that keeps the diagrams as pictures.

## Answering from a board

A message beginning `[board <doc> · block <id> · path:a-b | node <id> | step <n> | stale <k>]` followed
by a `> quote` is the reader pointing at a place on a board. `doc_read(doc, blocks=[<id>], format="markdown")`
(the outline form ignores `blocks`) and `code_symbol(symbol="path:line")` first; then edit the board in place — `doc_replace` of the block
that owns the place — and say one line in chat. `stale <k>` means those anchors no longer match the
tree: re-read them and re-issue the `Excerpt`s with the current lines and ranges.
