# 71 — Whiteboard recon (raw): what dev.fast's Whiteboard does and how

Status: recon, 2026-09-26, read-only survey of `/Users/Sky.Moore/Develop/whiteboard` (dev.fast Whiteboard, MIT; Electron + a Code-OSS fork hosting a React "review canvas"; agents draw through an MCP/CLI SDK). Companion to `71-whiteboard-visuals.md`, which decides what theuth borrows. Every claim carries a file:line relative to the whiteboard repo root; verbatim snippets are ≤6 lines and attributed (MIT, Copyright (c) 2026 dev.fast). Nothing here was modified, built or run.

Sections: A drawing model and protocol · B visual style · C interaction, opening from a terminal, sharing, navigation, the agent instructions, licence · D theuth today (the seams the design reuses).

Summary in ten lines:
1. A review is one ordered `Block[]` tree with 14 block kinds; `section` and `callout` are the only containers; ids are server-assigned; the whole snapshot is stored per version and streamed as NDJSON with a `lastEdit` summary that tells the canvas what to animate.
2. Code is referenced, never pasted: `code_peek.source = {file, start:{side,line}, end:{side,line}, pins?}`; sequence steps, flow-node attachments, call-stack frames and database operations all carry the same `source`; prose links code as `[label](review-source:head/path#L10-L24)`.
3. Layout is renderer-owned: the data carries only `flow_diagram.direction`, `section.defaultCollapsed`, shape hints and styles; no x/y, no columns.
4. The canvas is a document flow, not a freeform canvas: one 860 px column (prose capped at 720 px, diagrams at 1120 px), serif prose (Newsreader 15 px/1.72) with mono chrome (Geist Mono 11 px), a collapsible `section` with an anchored h2, a TOC that is a pill on narrow shells and a rail at ≥ 1360 px.
5. Selecting a diagram unit opens a fullscreen "tour" (diagram left, code stops right); clicking a prose `review-source:` link opens its source in a split pane (Code OSS inline editor, 560 px). These are two independent click-only anchor systems — prose and diagrams never light each other up (§B.7; corrects the first-pass reading of "one `activeAnchor`").
6. Highlight-to-feedback is clipboard-only: ⌘⇧C "Copy for Agent" POSTs the selection to `/copy-context`, which returns markdown with `Session ID`/`Version` header lines the agent maps back to `session_get`/`session_edit`.
7. Progressive drawing is per edit: "write small and often: one paragraph per edit, so the document draws itself"; a 3-minute lease (`session_activity`) with a `focus` description drives the "Agent working…" badge; ending the lease is the only completion signal.
8. The authoring instructions are the product: fixed section order (what/why → requirements → design → implementation), "pick one diagram that best shows the shape of the change", `code_peek` only for "the few spots that carry the mechanism or an invariant; link everything else inline", "always default to attaching/hyperlink code", "read the whole whiteboard back and fix any contradictions/unverified claims".
9. Opening from a terminal: the CLI/MCP finds the desktop through `~/.dev/review-desktop/instances/<key>.json` (loopback URL + token); `session_create` auto-opens the review; plugins for Claude/Codex/Cursor/OpenCode/Pi are MCP config only.
10. Everything dev.fast wrote is MIT; the Code-OSS fork under `apps/review-desktop/code-oss/` carries Microsoft's notices — copy prose/shapes/CSS with attribution, re-implement anything editor-side.

---

# A. Drawing model and protocol

`packages/progressive-review` contains only `tutorial/data.ts` (0 bytes). No code. Progressive semantics live in `packages/review/src/review-api/{document.ts,http.ts,store.ts}`.

### 1. Drawing model (agent SDK primitives)

All blocks: `defineBlock(type, props)` → `z.strictObject({ id?: string, type: literal, ...props })` — `blocks/definition.ts:25-30`. Union: `blocks/index.ts:20-38`:
```ts
export const leafSchema = z.discriminatedUnion("type", [markdown.schema, code.schema, divider.schema, code_peek.schema, sequence.schema, call_stack_diff.schema, database_lens.schema, image.schema, trace_quote.schema, software_map.schema, flow_diagram.schema]);
export type Block = LeafBlock | SectionBlock | CalloutBlock | TutorialBlock;
```
IDs are server-assigned (`document.ts:582-584` throws "IDs are assigned by the server."); prefixes `block-N | diagram-N | step-N | node-N | edge-N | frame-N | case-N | operation-N`, `lens-N` (`document.ts:547-557,594-603`; `store.ts:1207`).

| Primitive `type` | Key fields | Def | Example |
|---|---|---|---|
| `markdown` | `markdown: string` (links `[l](review-source:head/path#L10-L24)`, `review-trace:<traceId>#<eventId>`), `pins?` | `blocks/markdown.ts:4-16` | `tutorial/document.json:5-7` |
| `code` | `language` (default "text"), `text`, `caption?` | `blocks/code.ts:4-9` | — |
| `divider` | none | `blocks/divider.ts:3-6` | — |
| `code_peek` (code excerpt) | `source: DiffSelection`, `caption?` ("Not rendered yet") | `blocks/code_peek.ts:4-11` | `tutorial/document.json:47-60` |
| `section` (container) | `title`, `defaultCollapsed?`, `children: Block[]` | `blocks/section.ts:6-12` | `document.json:9-11` |
| `callout` (container) | `title?`, `tone: info\|warning\|danger\|success` (default info), `children` | `blocks/callout.ts:6-12` | — |
| `sequence` | `title`, `actors: Record<key,label>`, `steps: Step[]`; check: step from/to ∈ actors | `blocks/sequence.ts:36-52` | `document.json:92-130` |
| ↳ unit `step` | `from`,`to`,`label`,`style: call\|return\|async`, exactly one of `source\|explanation\|code` | `blocks/sequence.ts:14-34` | `document.json:104-121` |
| `flow_diagram` | `title`,`description?`,`direction?: right\|down`,`nodes[1..100]`,`edges[..300]`; check unique keys, edge endpoints | `blocks/flow_diagram.ts:56-63,69-88` | authoring-tools.ts:28 description |
| ↳ unit `flow_node` | `key`,`label`,`description?`,`kind?: process\|decision\|terminal`,`attachments[{label,sources[]}]`; insert-only `link:{from\|to,label?,style?}` | `flow_diagram.ts:15-28,40-50` | — |
| ↳ unit `flow_edge` | `from`,`to`,`label?`,`style?: solid\|dashed` | `flow_diagram.ts:30-37` | — |
| `call_stack_diff` | `title`, `base: Frame[]`, `head: Frame[]`; check unique keys, parentKey earlier | `blocks/call_stack_diff.ts:38-45` | — |
| ↳ `Frame` | `key?`,`parentKey?`,`callSite?`,`source`,`contextSources?[..1000]`,`label?`,`via?:{kind: call\|queue\|callback\|rpc, reason}` | `call_stack_diff.ts:12-34` | — |
| `database_lens` | `title`,`actors: Record<key, label\|{label,softwareMapPath?}>`,`stores: Record<key,Store>`(≥1),`useCases[{label,summary?,operations[≥1]}]`(≥1) | `blocks/database_lens.ts:75-97` | `document.json:200-240` |
| ↳ `Store`/`Field`/`Operation` | store `{label,storage: relational\|document,dataStoreKind?,softwareMapPath?,collections{label,key?,fields}}`; field `{label,dataType,nullable?,primaryKey?,references?,example?,fields?}`; op `{kind: read\|write,store,collection,field?,actor,label,detail?,source}` | `database_lens.ts:16-73` | — |
| `image` | `assetId` (upload id), `alt`, `caption?` | `blocks/image.ts:3-10` | — |
| `trace_quote` | `traceId`,`eventId`,`text` | `blocks/trace_quote.ts:3-9` | `document.json:325-328` |
| `software_map` | `mapVersionId`, `focusElementId?` | `blocks/software_map.ts:3-9` | `document.json:347-352` |
| `tutorial` (internal) | `kind: keymap \| conversation{conversation} \| view{view,label} \| feature{feature:'softwareMap',children}` | `blocks/tutorial.ts:10-36` | `document.json:17-33` |

Code reference primitive (`lens-selection.ts:18-40`):
```ts
export const diffSelectionSchema = z.strictObject({ file, start: {side:'base'|'head', line}, end: {side, line}, pins: sourcePinsSchema.optional() })
```
`sourcePinsSchema = {repositoryId, head, base?}` (`source.ts:9-13`).

Side-structures (not in document tree):
- **Diff-view lens** `{id,title,targets:[{kind:'files',patterns[]}|{kind:'ranges',sources: DiffSelection[]}]}` — `review-api/diff-lenses.ts:16-39`; LENS_LIMIT 200.
- **Software map model** (uploaded resource): `{people?,systems?[{containers?[{components?[{codeElements?}]}],dataStores?,external?}],relationships?[{kind:'call'|'semantic',from,to,...}]}` with `coverage:{files,globs}` — `review-api/map-input.ts:41-115`.
- **Uploads** `{id:uuid,repositoryId,kind:'image',base64} | {kind:'trace',trace} | {kind:'map',pins,side,model}` — `review-api/local-data.ts:100-119`.

Not present: tables (only via markdown GFM), lists (markdown), free annotations/layers/groups, explicit positions.

### 2. Protocol shapes

#### Document / snapshot (`review-api/store.ts:156-178`)
```ts
export interface Snapshot { reviewId; version: number; title; kind?: 'scratchpad'; pins?: Pins; target?: ReviewTarget; document: Block[]; lenses?: Lens[]; createdAt; origin?; lastEdit?: EditSummary; staleSources?; sourceUnavailable?; shared? }
```
- Top level = ordered `Block[]`; ordering = array order; containers `section`/`callout`/`tutorial(feature)` nest via `children`. Diagrams hold units in `steps` or `nodes`+`edges` (`document.ts:506-513`).
- `ReviewTarget = {kind:'worktree',repositoryId,base?} | {kind:'commits',repositoryId,head,base?}` (`document.ts:111-125`).
- Versioning: every accepted mutating command = new integer `version`; whole snapshot stored; `REVIEW_SCHEMA_VERSION = 5` (`review-protocol/src/contracts.ts:15`). No expected-version param; retries idempotent by `commandId` (README §API).

#### Commands (`store.ts:76-134`, tools `authoring-tools.ts:86-104`)
| operation.type | fields |
|---|---|
| `create` | `title?`,`target?`,`pins?`,`pullRequestUrl?`,`repositoryId?`,`reuseExisting?`,`kind?:'scratchpad'` (+tool-only `open?`) |
| `edit` | `reviewId`, `edit: Edit` |
| `lens` | `reviewId`, `edit: LensEdit` (insert/update/remove, `afterId?`) |
| `set_target` / `rename` / `repin` / `restore` / `attention{view|dismiss|restore}` / `delete` | see store.ts:80-133 |
Envelope: `{commandId: uuid, leaseId?: uuid, operation}`.

#### Edits (`document.ts:358-380`)
```ts
insert {content, parentId?, afterId?} | replace {targetId, content: Block} | update {targetId, changes: Record<string,json>} | move {targetId, parentId?, afterId?} | remove {targetId}
```
`content` may be a Block, a `step`, a `flow_node` (+`link`), or a `flow_edge` (`document.ts:351-356`). Update = field patch, omitted keeps, `null` removes. Response `Applied {targetId, type, children?: {id,type}[], linkId?}` (`document.ts:405-418`).

#### lastEdit / EditSummary (`document.ts:387-402`)
```ts
export interface EditSummary { type; targetId; blockId; kind: Element['type']|'lens'; unit?; linkId?; fields?: string[]; units?: string[] }
```
`units` = `drawOrder()` (`document.ts:475-503`): sequence steps in order; flow nodes in order, each edge emitted as soon as both endpoints drawn.

#### Read routes / tools (README §API; `authoring-tools.ts`)
| Tool (agent sees `session_*`) | HTTP |
|---|---|
| `whiteboard_status` | status-tool |
| `capabilities` | GET /capabilities |
| `get_instructions {topic?}` | GET /instructions |
| `activity {action,leaseId,scope?,focus?}` | POST /:id/activity |
| `create/edit/lens_edit/rename/repin/restore/set_target/attention/delete` | POST /commands |
| `list` / `get {targetId?,full?,version?,format?}` / `history` / `lens_get` | GET / , /:id/inspect, /:id/history, /:id/lenses |
| `open` | POST /:id/open |
| `environment`, `workspace_cleanup`, `register_repository {path}`, `resolve_pins {repositoryId,base,head}`, `upload` | POST … |
| `source {source: FileLineRange}`, `file`, `tree`, `diff {paths?,format:'files'|'patch',context?,maxBytes?}`, `commits` | /:id/source,/file,/tree,/diff,/commits |
Streams: `GET /watch` (catalog NDJSON), `GET /watch?subscriptions=…` (multiplexed, array of `{value}|{error}|null`), `GET /:id/watch` (snapshot+activity NDJSON) — `http.ts:392-598`.

#### Activity / leases (`activity.ts:15-28,49`)
`{action: begin|renew|end, leaseId: uuid, scope?: document|lenses, focus?: {description(1-160), targetId?}|null}`; TTL 180 000 ms; 409 for other writers.

#### review-protocol (desktop⇄canvas, `review-protocol/src/contracts.ts`)
| Message | Line |
|---|---|
| `ReviewDesktopDiscoverySchema {version:3,instanceId,url(http://127.0.0.1:port),appPid,serverPid,token,startedAt,cliPath?,cliRuntimePath?,key?,channel?}` | 723-747 |
| `ReviewRuntimeConfigSchema {serverUrl,reviewId,token,wasmUrl,appVersion,theme,host:'desktop'}` | 89-97 |
| `ReviewVerbRequestSchema` names: authoringCapabilities, joinDiscord, showReviewView{view}, openSourceTree, openDiff{path}, reveal{path,startLine,endLine,side?,pins?}, focusCanvas, focusWindow, captureScreenshot, openReviewRevision, openReview{reviewUuid,active}, openApiReview{reviewId,title} | 1175-1228 |
| `ReviewDesktopVerbFrame {event:'desktop-verb',id,request}` / `ReviewDesktopVerbResult {id,response}` | 1238-1251 |
| `ReviewSurfaceEventSchema`: editorSelectionChanged{reviewId,path,range,sideContext,anchor?{x,y}}, themeChanged{theme}, showReviewView{view} | 1283-1304 |
| `reviewViewSchema = review|commits|diff|map|trace` | 37-43 |
| `ReviewTutorialOpenResponseSchema {kind:'api',reviewUuid,title}` | 854-858 |
| `REVIEW_TUTORIAL_STEP_IDS` (openPeek, gotoDefinition, showHover…) + `TutorialProgressV1 {version:1,checked,dismissed}` | 405-428 |
| Diff/inline-editor factories: `ReviewInlineEditorSpec {path,side,ranges,heightMode:'capped'|'content',pins?}`, `ReviewDiffLens`, `ReviewDiffProgress`, `REVIEW_DIFF_LAYOUTS=['split','unified']` | 108-297 |

#### trace-protocol (`trace-protocol/src/contracts.ts`)
`ReviewAgentTraceEventSchema` kinds: `user{text,at?}`, `assistant{markdown,thinking?,at?}`, `tool{tool,verb,title,filePath?,additions?,deletions?,command?,input?,output?,error?}`, `separator{label}` (40-70); `ReviewAgentTraceSessionSchema {sessionId,title?,harness: claude-code|codex|opencode|pi|unknown, available, source:'r2'|null, subagents?, commits[{sha,subject}]}` (74-84). Hosted store API `/api/trace/v1` (`store-api.ts:5`): createStore, beginUpload{harness,objects[{name,size,sha256}]}→presigned URLs + `baseGeneration`, completeUpload{commits,branch?,author?}→`generation` (60-197). Object names `main.jsonl.gz | subagents/*.jsonl.gz` (44-46).

#### review-share-protocol (`review-share-protocol/src/index.ts`)
`SHARE_FORMAT='review-share/1'` (3); manifest `{format,reviewId,version,title,snapshot: objectId, presentation: objectId, objects[{id=sha256,size,sha256}], resources[{id,kind:image|trace|map,mimeType,object}], repository:{cloneUrl}}` (89-108); link `/s/<uuid>#<capability43>` (161-171). Limits: 32 MiB/object, 128 MiB total, 2048 objects (5-11).

### 3. Streaming semantics (progressive drawing)
- **Granularity = one edit per version.** Agent inserts blocks one at a time (authoring.md: "Write incrementally. The user sees you write on the canvas in real-time"). No partial-block/chunked text; a markdown block lands whole.
- **Transport:** `GET /:id/watch` NDJSON; `watch()` (`http.ts:1302-1352`) sends `JSON.stringify(read())+"\n"` on subscribe and on each store commit; `dirty` flag + `desiredSize` check ⇒ **coalescing**: a slow reader skips intermediate versions and gets the latest snapshot (README: "The stream coalesces updates when a reader falls behind; reconnecting starts with the current saved snapshot").
- **Semantics are replace-whole-snapshot**, not patch: every line is the full `Snapshot`. The canvas diffs by React identity (ids) and uses `lastEdit` to animate only what changed: `fields` → e.g. `defaultCollapsed` patch draws nothing; `unit` → trace where it attaches; `units` → whole diagram traced in draw order; `linkId` → node then edge (README "Each accepted edit is one saved version…").
- **Activity** rides the same stream (`http.ts:566-572`); activity-only ticks reuse the cached document.
- **Partial output / crash:** lease expires after 3 min → review reads as complete ("Ending a lease… is the only completion signal"); versions already saved persist; no rollback. Retries reuse `commandId` (idempotent receipt).
- Scratchpad inserts prepend (`store.ts:1273` placement `first`).

### 4. Agent plugins & hook-in
- All plugins launch the installed CLI: claude `.mcp.json` → `sh -c 'exec "$HOME/.local/bin/whiteboard" mcp'` (`agent-plugins/claude/.mcp.json:2-7`); codex `bin/whiteboard-mcp` same (`codex/bin/whiteboard-mcp:3`); cursor `mcp.json:2-7`; opencode `index.js:3-17` (win32: `cmd /d /c whiteboard mcp`); pi = SKILL telling agent to run `whiteboard api session_get_instructions '{}'` (`pi/skills/whiteboard/SKILL.md:8-9`).
- `whiteboard`/`review` bin → `dist/cli.js` (`review/package.json:12-15`); `api`/`mcp` subcommands (`cli-runner.ts:830-831`; `agent-cli.ts:38`).
- MCP server name `whiteboard`, `listChanged:true`; catalog fetched live from `GET /authoring`, renamed via `publicTool` (`mcp.ts:65-70,86-96`; `public-tools.ts:37-58`). When Desktop is down, `session_get_instructions` returns `RECOVERY` (`recovery.ts:1-2`).
- **Discovery:** `connectReviewInstance` (`agent-client.ts:43-87`): `DEV_REVIEW_SERVER_DIR` → `<stateDir>/review-server/server.json` (`server-discovery.ts:28-30`), else `~/.dev/review-desktop/instances/<key>.json` (`review-home-paths.ts:32-58`; home `DEV_REVIEW_HOME` default `~/.dev`, `trace-core/src/trace-home.ts:4-18`). File = `ReviewDesktopDiscovery` (loopback URL + bearer token).
- **Opening from terminal:** `session_create` auto-opens when `desktopAvailable` (`http.ts:536-551`), else `session_open` → `POST /:id/open` → `open({reviewId,title})` callback into Desktop (`http.ts:503-509`) → verb `openApiReview{reviewId,title}` (`contracts.ts:1220-1226`). 409 "The desktop is not connected." without Desktop.
- **Tutorial:** Desktop-only routes `GET /tutorial/status`, `POST /tutorial/prepare`, `GET /tutorial/review`, `POST /tutorial/open`, `DELETE /tutorial` (`server/desktop-server.ts:388-425`); `createNativeTutorial` uploads `trace.json`, base/head `software-map.json`, rewrites alias ids, then `create` with `origin:{tutorial:true}` from `tutorial/document.json` (`tutorial-service.ts:54-112`); stamped by contentHash at `~/.dev/tutorial/stamp.json` (`:115-124`).
- **Prompt rules** (`instructions/authoring.md`): register repo → create → spawn subagent for `file-lenses` → `session_activity scope:document` → `session_diff` → first what/why section immediately; sections in order what/why, requirements, design (pick ONE of sequence/flow_diagram/database_lens; code_peek for contracts), implementation (call_stack_diff rooted at user entry; code_peek only for mechanism). `scratchpad.md`: fixed id `scratchpad`, every source carries own `pins`, newest on top, `session_open` once after first insert.

### 5. Package graph & exports
```
json ─┬─> trace-protocol ─┬─> review-protocol (also @dev.fast/diffr)
      ├─> trace-core (+local-vcs, trace-protocol)
      └─> review ── uses all: json, review-protocol, review-share-protocol, trace-core, trace-protocol, local-vcs, diffr
review-share-protocol: zod only
agent-plugins/*: no workspace deps (runtime: installed `whiteboard` bin)
```
Evidence: `review/package.json:85-91`, `review-protocol/package.json:22-27`, `trace-core/package.json:25-31`.
- `review` exports only `./runtime`, `./sharing` (`package.json:30-39`); bins `review`,`whiteboard`.
- `review-protocol/src/index.ts`: re-exports diffr, `@dev.fast/json`, contracts, review-api-client, structural-diff, code-peek-diff, bug-report, trace-protocol schemas, `parseZod`, `parseReview*` helpers (1-186).
- `trace-protocol/src/index.ts`: contracts, store-api, store-routes (1-5).
- `trace-core/src/index.ts`: CLI runners (trace enable/sync/hooks), StoreClient, `devReviewHome`, `writePrivateJsonAtomic`, `withFileLock`, `loadReviewAgentTrace`, hook installers per harness (`installClaudeTraceHook`, `installCodexTraceHook`, `installOpenCodeTraceExtension`, `installPiTraceExtension`) (1-200).
- `json/src/index.ts`: `JsonValue/JsonObject/JsonArray`, `jsonValueSchema`, `isJsonObject`, `parseJsonText`, `json*` accessors, `is*Value` guards.

### 6. Layout hints vs renderer
Data carries almost no layout: `flow_diagram.direction?: 'right'|'down'` (`flow_diagram.ts:59`), `section.defaultCollapsed?` (`section.ts:9`), `callout.tone`, `flow_node.kind` (shape hint), `step.style`, `flow_edge.style`. Ordering is array order; diagram unit placement is by key/actor references. No x/y/size/columns/grid. Renderer decides: `heightMode 'capped'|'content'` and diff `split|unified` are canvas-side (`contracts.ts:108,116`); only UI event with coordinates is `editorSelectionChanged.anchor{x,y}` (`contracts.ts:1289`). `call_stack_diff` frames use `parentKey` for tree nesting and `key` for base/head alignment (`call_stack_diff.ts:14-16`). Canvas source (code-oss contrib in `apps/review-desktop/code-oss`) not inspected — grep timed out; layout algorithm `[INFERENCE]` renderer-internal.

---

# B. Visual style (renderer `packages/review/app/src`)

Root: `/Users/Sky.Moore/Develop/whiteboard`. Paths relative. `apps/review-desktop` contains no renderer CSS of interest; all UI lives in `packages/review/app/src`.

### 1. Colour tokens

Dark block: `styles.css:2-708` on `.review-canvas-root` (neutrals bridge `--vscode-*` with dark fallbacks, comment `:17-26`). Light block: `styles.css:1792-1918` on `.review-app--theme-light` (`color-scheme: light` `:1802`; dark sets `color-scheme: dark` `:635`).

| token | dark | light | use |
|---|---|---|---|
| `--bg` | `var(--vscode-editor-background,#0c0f15)` :27 | `…,#ffffff` :1809 | page/canvas bg |
| `--surface` | `#0c0f15` :28 | `#ffffff` :1810 | cards, participant labels |
| `--surface-raised` | `var(--vscode-editorWidget-background,#1a1f29)` :29 | `#ffffff` :1811 | popovers |
| `--control-bg` | `#1a1f29` :30 | `#e9ebef` :1812 | inputs |
| `--tray` | `#12161e` :6 | `#f3f4f6` :1796 | diagram header bg, inline code bg |
| `--well` | `#1a1f29` :4 | `#e9ebef` :1794 | sunken areas |
| `--ghost` | `#343a46` :3 | `#cdd1d8` :1793 | ghost strokes |
| `--ink` | `var(--vscode-editor-foreground,#eef0f4)` :31 | `#15181e` :1813 | body text |
| `--ink-muted` | `#9199a8` :32 | `#5d6472` :1814 | captions, edge labels |
| `--ink-faint` | `#5f6776` :33 | `#9aa0ab` :1815 | gutters, meta |
| `--rule` | `var(--vscode-panel-border,#262c37)` :34 | `#d8dbe1` :1816 | borders |
| `--rule-soft` | `#39404d` :35 | `#b9bec7` :1817 | node strokes, connectors |
| `--accent` | `#5b7cff` :54 | `#2b55e6` :1836 | links, active edges/dots |
| `--accent-soft` | `#1a2240` :55 | `#eef2ff` :1837 | accent bg |
| `--on-accent` | `#ffffff` :56 | `#ffffff` :1838 | text on accent |
| `--diagram-active` | `#5b7cff` :68 | `#2b55e6` :1850 | active diagram element |
| `--edge-muted` | `#6f7786` :66 | `#9aa0ab` :1848 | inactive sequence edges |
| `--edge-implied` | `#8c8880` :67 | `#b9bec7` :1849 | implied edges |
| `--change-added` | `#4ed990` :69 | `#1b9a57` :1851 | added node/name |
| `--change-removed` | `#f26a55` :70 | `#d8402c` :1852 | removed |
| `--change-modified` | `#f2c14e` :71 | `#c98a0b` :1853 | modified |
| `--diff-added-bg` | `#132019` :73 | `#eef7f2` :1855 | added node fill |
| `--diff-modified-bg` | `#1d1b12` :75 | `#fbf5e8` :1857 | modified fill |
| `--diff-removed-bg` | `#211517` :77 | `#fcefed` :1859 | removed fill |
| `--marker-tint` | `rgba(91,124,255,.14)` :9 | `rgba(43,85,230,.08)` :1799 | selected row / node fill |
| `--marker-glow` | `rgba(91,124,255,.3)` :10 | `rgba(43,85,230,.22)` :1800 | glow |
| `--link-open-wash` | `rgba(91,124,255,.15)` :90 | `rgba(43,85,230,.12)` :1868 | open anchor link bg |
| `--accent-shadow` | `rgba(91,124,255,.2)` :86 | `rgba(43,85,230,.08)` :1864 | hover ring |
| `--shadow-color` | `rgb(0 0 0/45%)` :78 | `rgba(21,24,30,.08)` :1860 | card shadow |
| `--text-selection-bg` | `rgba(91,124,255,.35)` :60 | `rgba(43,85,230,.16)` :1842 | ::selection |
| `--canvas-grid` | `#1a1f2a` :62 | `#e6e8ec` :1844 | grid dots (`--board-grid` svg :11/:1801) |
| `--rail-1..4` | `#f26a55 #5b7cff #4ed990 #c8a4f5` :101-104 | `#d8402c #2b55e6 #1b9a57 #6d43b3` :1878-1881 | multi-rail colours |
| `--syntax-comment/type/function/string/number/operator` | `#778090 #6fc3e6 #b7c4ff #d9a85c #d48fd6 #9199a8` :126-131 | `#6b7382 #0c6e8e #1f3f99 #8a5a1c #8b3f8a #5d6472` :1908-1913 | code highlighting |
| `--syntax-inserted/deleted` | `#4ed990 / #f26a55` :132-133 | `#167a47 / #c93a27` :1914-1915 | diff text |
| `--kw --fn --str --num --com --op` | `#f97cc0 #7cc5f5 #7cf5b0 #f3cf6b #565c66 #9ba1ac` :105-110 | `#9c3f80 #2370a4 #2e9e66 #b0791f #767672 #555555` :1882-1887 | legacy syntax |
| `--topbar-bg` | `rgba(12,15,21,.94)` :82 | `#ffffff` :1863 | top bar |

Diagram aliases `:703-707`: `--diagram-border: var(--rule)`, `--diagram-surface: var(--surface)`, `--diagram-header-bg: var(--tray)`, `--diagram-canvas-bg: var(--bg)`. Chrome ramp `:658-679`: `--chrome-font: var(--font-mono)`, `--chrome-font-size: 11px`, `-small: 10px`, weights 400/600, `--chrome-tracking .04em`, control height 22px, radius 5px, icon 14px. `--affordance-rest-opacity: .45` :39. Selection ring `--selection: var(--accent)` :44.

### 2. Typography

- Families `styles.css:13-15`: `--font-serif: "Newsreader", Georgia, serif`; `--font-mono: "Geist Mono", ui-monospace, monospace`; `--font-display: var(--font-mono)`. Faces in `fonts/fonts.css`: Geist Mono 400/500/600, Newsreader 400, 400 italic, 600 (woff2, `font-display: swap`).
- App shell is mono: `.review-app { font-family: var(--font-mono) }` :4183. Document is serif: `.review-document { font-family: var(--font-serif); font-size: 15px; line-height: 1.72 }` :4471-4473; `h1,h2,h3,p,li,blockquote` forced serif :4482-4489.
- Heading scale (later rules win): h1 `34px/40px` w500, `margin: 28px auto 18px` :4491-4499; h2 `22px/28px` w500, `margin: 40px auto 12px` :4724-4731; h3 `17px/23px` w600, `margin: 30px auto 10px` :4733-4740. (Earlier overridden ramp h1 42/h2 25/h3 18 at :3036-3057; `scroll-margin-top: 24px` on h2/h3 :3044-3047.)
- Body `p, li` 15px/1.72 :4742-4748; `li + li` 8px :4751.
- Inline code: mono `0.9em` on `--tray` :3088-3090; tables mono 13px/1.55 :4882-4885.
- Code surfaces: mono 12px / 19px line (`.panel-static-code-surface` :3352-3355); inline editor 20px line height (`LINE_HEIGHT = 20` DocumentCodeView.tsx:18).
- Diagram labels: mono 11.5px w650 (`.sequence-message-label` :3702-3704); flow figure `font: 12px/1.5 var(--font-mono)` flow-diagram.css:7; edge labels 9px :7758.
- Micro-labels: mono 10px w700 tracking .08em (`.review-trace-kicker` :6629-6632, kind badges :3167-3171).

### 3. Layout

- **Document flow, not freeform**: `.review-document` is a flex child `flex: 1 1 860px; max-width: 860px; margin: 0 auto 96px` :4458-4462 inside `.review-view-region--review` (flex, centered, `overflow-y: auto`, padding `var(--review-page-top) 52px calc(120px + var(--review-toc-tail))`) :4189-4201. Only software_map uses a pannable canvas.
- Column vars :4449-4457: `--review-prose-max-width: 720px`; `--review-inline-diagram-max-width: 1120px` (1000px when peek open :4477); inline padding `clamp(20px, calc((100cqi - 720px)*.122 + 20px), 64px)`; block padding 28px / 72px.
- Every prose/code block child gets `width: min(100%, --review-prose-max-width); margin-inline: auto` :4763-4788; diagrams may exceed to the 1120px cap. Consecutive `.code-peek + .code-peek` gap 14px :4796-4799.
- **App grid** `.review-app { grid-template-columns: minmax(0,1fr) 0 0 }` :626-629; peek open → `minmax(0,1fr) 10px var(--side-peek-width, 560px)` :1930-1932 (resizer 10px, `.side-panel` :3093). That right column is the split view where source (Code OSS inline editor) opens beside the document.
- **Section chrome** (`ReviewSection` review-components.tsx:145-233): `<section.review-section data-review-section={title}>` → `.review-section-header` (flex, baseline, gap 10px, `margin-block: 40px 12px` :4806-4812) with absolute `.review-section-toggle` chevron at `left:-24px; top:5px; 18×18` :4818-4829 (rest opacity, full on hover/collapsed :4842-4850), `<h2 id={id}>` anchor target, optional `.review-section-meta` summary when collapsed; body `.review-section-body[hidden]`. Collapse persists via `useReviewUiState(..., {scope:'session', namespace:'section'})` :160-167; TOC dispatches `review-section-expand` DOM event to expand before scroll :186-195. Collapsed h2 dims to `--ink-muted` :4853.
- **TOC**: `.review-toc` absolute pill `top: calc(32px + --review-page-top); left: 24px; 32×32; radius 8px` growing to 248px card :4238-4262; wide shell becomes always-open rail `.review-toc--rail` :4341; active link `--accent` :4443-4447. Component `ReviewToc` review-toc.tsx:37.
- Diagram header shared: `DiagramHeader {kind,title,meta,action}` diagram-header.tsx:3; `.diagram-header` bg `--tray`, title `--ink` :5567-5627.

### 4. Code next to diagrams

- Block dispatch blocks.tsx: `CodePeekBlock` :131 → `CodePeekCard source={node.source}` (CodePeek.tsx:120) → `<section className="code-peek" data-code-rendering="inline-editor">` :181 wrapping `DocumentCodeView path={source.file} side ranges heightMode` :182-190. `.code-peek { color: var(--ink); font-family: var(--font-mono) }` styles.css:3302-3307.
- `DocumentCodeView` (DocumentCodeView.tsx:24) renders `<div.review-inline-editor data-review-inline-editor={path} data-review-inline-editor-active>` :309-313 and mounts a Code OSS editor through `inlineEditorFactory.create({path,title,description,side,pins,ranges,heightMode,active,…})` :236-252. Gutter/line numbers/highlight range are drawn by the host editor, not CSS here; height estimate = lines×20 + 40px header, capped at 18 lines (`heightMode:'capped'`) :18-22, :323-339. Frame: `.review-inline-editor { border:1px solid var(--rule); radius 6px; bg var(--bg) }` :3309-3317. Fallback static surface: `.panel-static-code-line { grid-template-columns: 42px 14px 1fr; min-height:19px }` :3357-3363, gutter `--ink-faint` right-aligned :3366-3370.
- Caption/title = `title` prop (file path for groups CodePeek.tsx:88; `source.file` for cards :183-186).
- **`review-source:` links in markdown**: `MarkdownBlock` blocks.tsx:56-90 passes `renderLink`; href looked up in `data.anchors.get(`${node.id}:${href}`)` :82 → `<AnchorLink anchor>` :85. `AnchorLink` (review-components.tsx:321-347) → `ProsePeekAnchor href=#review-anchor-{id}` with `data-review-anchor-id`, `data-review-anchor-open` :296-299 and `openPeek({kind:'peek', anchor, content:{kind:'source', source: anchor.peek}})` :338-342. Open-state CSS: `.review-document a[data-review-anchor-open] { background: var(--link-open-wash) }` :3080-3083. Unresolved hrefs fall through to plain `<a>` (agent-markdown.tsx:394-397). codeRefs counted by `/^review-source:/i` block-document-derivations.ts:31.
- **Selection → source sync mechanism**: diagrams hold `{anchor, revealRequest}` state (diagrams.tsx:266-271, flow-diagram.tsx:61-64). Sequence edge click `onEdgeClick → openTour(edge.data.message.id)` :474-479; flow node select `onSelect={(item)=>open(item.key)}` flow-diagram.tsx:115. `openTour` opens `DiagramTourOverlay` portal :335-346 → fullscreen grid `.diagram-tour-overlay { grid-template-columns: minmax(0,1fr) 10px var(--diagram-tour-pane-width, 424px) }` styles.css:3543-3557 (stage left, panel right). `GuidedTourPanel` (review-components.tsx:659) renders each stop as `<section className={isActive?'tour-stop active':'tour-stop'} data-review-anchor-id>` :908-916 with `CodePeekCard source active heightMode="content"` :1031-1034; scroll-sync `syncActiveStopToScroll` :904 and `onNativeFocus → onActiveAnchorChange(id,{reveal:false})` :925-928 push the anchor back to the diagram, which recolours edges (`activeTourAnchor === message.id` diagrams.tsx:440). Call-tree rows carry `data-review-anchor-id` + `aria-current` call-tree-view.tsx:82-83 and `DocumentCallTree` opens `openPeek({kind:'peek', anchor, content:{source}})` :224-230.

### 5. Diagram styling

- **Sequence** (diagrams.tsx, ReactFlow): lane min 176px, `messageTop 112`, `messageGap 76`, min width 320 :401-408. Edge colour `sequenceMessageColor(active) → var(--accent) : var(--edge-muted)` :70-72; return messages dashed `6 4`, async open arrow vs closed :447-456; className `sequence-message clickable [active]`, active zIndex 2 :469-473. CSS: `.sequence-message { stroke-width: 1.6 }` :3656 (later 1.25px :5549), `.active { stroke-width: 2.4 }` :3673-3675; hit area stroke 18 :3660-3666; `.sequence-message-dot` 11px circle border 1.5px `--rule-soft` bg `--surface`, `.active` border+bg `--accent` :3677-3691; label 11.5px w650 `--ink-muted`, active `--accent` w700 :3693-3714; hover ring `0 0 0 2px --accent-shadow, 0 6px 14px --shadow-color-strong` :3716-3721. Participant label: 1px `--rule-soft`, radius 6, bg `--surface` :5052-5056. On tour stage dots become 18px numbered badges, active `--on-accent` on `--accent` :3624-3639. Figure class `sequence-diagram sequence-tour [--active]` :620-624.
- **Flow** (flow-graph.tsx / whiteboard.css): node classes `lens-flow-node--{added|removed|modified|unchanged} --{kind} is-selected is-viewed` flow-graph.tsx:368-378; shape `rect` radius 6 (terminal = pill) :424-431. `.flow-node-shape :is(rect,polygon) { fill: --surface; stroke: --rule-soft; stroke-width: 1 }` whiteboard.css:986-989; added/removed/modified fill `--diff-*-bg` + stroke `--change-*` :996-1009; selected/focus `stroke: --accent; stroke-width: 1.5` :1011-1015 plus `fill: --marker-tint` flow-diagram.css:62-64; decision nodes dashed `--ink-muted` :1039-1042; caption 10px `--ink-faint` :1051-1054. Edge `.lens-flow-edge { stroke: --ink-muted; stroke-width: 1.4 }` :1056-1060, dashed `6 4` when `data.dashed` flow-graph.tsx:466; label 9px mono with 4px `--tray` halo :1066-1069 / styles.css:7757-7761. Draw-in motion `data-motion=queued|stroke` :2110-2128 (stroke phase `--accent` 1.6). Figure: 1px `--rule-soft`, radius 8, `margin 18px 0 4px`, bg `--bg` flow-diagram.css:1-9; footer legend 6px dots `--change-*` :46-61; inline height 560 (top-down) / 420 (left-right) flow-diagram.tsx:120-122.
- **Call stack diff** (call-tree-view.tsx): `.review-call-row--{added|removed|modified}` :81; CSS row 26px mono 12px :7802-7813; added name `--change-added`, removed `--change-removed` + line-through at 50% mix, modified `--change-modified` :7820-7834; connectors `--rule-soft` 1px :7839-7843, hovered call edge `--ink` 2px :7878-7883; current row `[aria-current=true] { background: --marker-tint }` + 2px `--accent` left bar :7781-7796; hover `color-mix(--ink 3%)` :7797-7799; viewed count `.diff-count-added/-removed` :114-119.
- **Database lens** `DatabaseLens` database-lens.tsx:247 (default height 560 :248) shares tour stage/overlay rules :3588-3608 and `.database-lens-header .diagram-header-actions` :5673-5675.

### 6. Responsive rules

- `@media (max-width: 720px)`: side peek column collapses to `minmax(0,1fr) 0 0`, resizer hidden :1934-1941, :5765-5768.
- `@media (max-width: 760px)`: `.review-commits-view` padding 24px/16px :426-429.
- `@media (max-width: 620px)`: find widget stretches `left/right: 8px` :584-588.
- `@media (max-height: 560px)`: `.side-panel { inset:0; height:100% }` :5865-5869.
- `@media (max-width: 960px)`: onboarding columns stack :6163-6166.
- `@container review-canvas (max-width: 1080px)`: flow tour overlay switches to stacked rows `minmax(180px,40%) 1fr` flow-diagram.css:83-90.
- `prefers-reduced-motion: reduce` kills transitions on toc, side-panel, tutorial progress, drawn check, courier, flow motion :1634, :1758, :3140, :4326; whiteboard.css:144, :208, :284, :1752, :2330.
- Fluid: document inline padding via `clamp(...100cqi...)` :4450-4454; TOC pill→rail switch by shell width :4238-4341; sequence lanes flex to width with 176px floor diagrams.tsx:401-404.

### 7. Anchor, side peek and tour — as built (second pass, 2026-09-26; supersedes any "one activeAnchor" phrasing above)

- **Two anchor systems, not one.** (a) The right panel is a zustand store with a single slot `active: ReviewPanel | null` (`review-panel-store.ts:10-13, 30-34`); `ReviewPanel = PeekPanel | TourPanel` (`review-panel-model.ts:40-53`); only `TourPanel` carries `activeAnchor: string; revealRequest: number` (`:46-51`). (b) Each diagram owns its own `useState<{anchor, revealRequest} | null>` (`diagrams.tsx:272-275`, `flow-diagram.tsx:60-63`, `database-lens.tsx:312`) and portals a fullscreen `DiagramTourOverlay` (`diagram-tour.tsx:59-83`, `role="dialog" aria-modal`).
- **Prose → code only.** A `review-source:` link becomes `<a href="#review-anchor-{id}" data-review-anchor-id data-review-anchor-open>` (`review-components.tsx:294-298, 334`); click → `openPeek({kind:'peek', anchor, content:{kind:'source'}})` (`:337-341`); the link reads back `state.active.anchor?.id === anchor.id` to wash itself (`:328-331`; CSS `styles.css:3080-3083`). Nothing reads prose anchor ids into diagrams (`data.anchors` is consumed only at `blocks.tsx:82`). Side peek width `DEFAULT_SIDE_PEEK_WIDTH = 560`, min 360, max 920 (`App.tsx:91-97`); grid `minmax(0,1fr) 10px var(--side-peek-width,560px)` (`styles.css:1930-1932`). One panel, one content, no stack (`openPeek` replaces, store `:34`).
- **Diagram → code only.** Sequence: click on edge hit-area / dot / label, Enter/Space on label → `openTour(message.id)` (`diagrams.tsx:770-827`); flow: `onNodeClick → select`, Enter/Space (`flow-graph.tsx:147-149, 394-399`), `FlowDiagram.open(nodeKey)` picks the node's first stop (`flow-diagram.tsx:79-89`). Diagrams **always** open the fullscreen tour (`position: fixed; inset: 0`, `styles.css:3543-3557`), never the 560 px peek; the choice is by caller, not state. Prose does not react to diagram selection (no reader of `data-review-anchor-id` besides the authoring cursor `cursor-element.ts:12-13`).
- **No hover coupling anywhere.** The only `onMouseMove/onMouseLeave` in the renderer is `trace-ruler.tsx:383-384` (unrelated); hover styling is local CSS (`styles.css:3076` link underline, `:3716-3720` label ring). All coupling is click / keyboard / focus / scroll.
- **Anchor ids are per block, never shared.** Markdown link id = `${blockId}:${href}` (`review-api/document.ts:270`, regex `/^review-source:(base|head)\/(.+)#L(\d+)(?:-L(\d+))?$/i` `:236-239`); flow attachment id = `${blockId}:${nodeKey}:${index}:${sourceIndex}` (`:281`); `code_peek`/`step` id = the element id (`:322-329`). Same source range in a link and a `code_peek` ≠ same anchor; `CodePeekBlock` does not consult `data.anchors` at all (`blocks.tsx:131-133`). `#review-anchor-*` is an inert href — no element carries that id.
- **Tour stops.** `GuidedTourStop {anchor: PeekAnchor, label, detail?, content}` (`review-panel-model.ts:26-31`); content ∈ `source{DiffSelection} | inline-code{language?,text} | explanation{text?} | trace-quote` (`:3-13`). Sequence: one stop per step in step order, `inline-code` if `step.code`, else `source`, else `explanation` (`diagrams.tsx:153-168`). Flow: one stop per attachment **source** in `block.nodes` order; a node without sources gets one `explanation` stop "This node has no code attachments." (`flow-diagram.tsx:14-45`). Left stage = the same inline figure re-rendered `fullscreen=true` with the active unit recoloured (`flow-diagram.tsx:167`, `diagrams.tsx:350-357, 434-435`), stage auto-scrolls to the active message (`diagrams.tsx:510-550`). Pane default 594 px, min 360, max 760, main ≥ 480 (`diagram-tour.tsx:97-105`).
- **Anchor setters:** click on stop nav pill Prev/Next (`review-components.tsx:862-887`) → `onActiveAnchorChange(id,{reveal:true})`; panel scroll → `syncActiveStopToScroll` (`:830-847`) → `{reveal:false}` using `activeTargetForScroll(targets, scrollerTop, scrollerTop + height/2)` (`scroll-active-tracking.ts:26-46`: first target at or below the scroller top; active if in the top half, else the previous one); focus inside a stop's inline editor → `onNativeFocus` (`:925-928`, `CodePeek.tsx:199`, `DocumentCodeView.tsx:117-119`). **No ArrowUp/ArrowDown handlers**; Escape closes (`diagram-tour.tsx:108-113`, `review-components.tsx:81-91`); **no focus restoration** on close and no focus trap in the `role=dialog`. Tour state persists `{tourId, activeAnchor}` (`review-view-state.ts:221-238, 262-289`).
- **Fonts.** `packages/review/app/src/fonts/`: `fonts.css`, `geist-mono-0..5.woff2` (400/500/600), `newsreader-6..11.woff2` (400 italic → 6-8; 400 and 600 upright → 9-11), both `font-display: swap`; `newsreader-OFL.txt:1-3` "Copyright 2020 The Newsreader Project Authors (http://github.com/productiontype/Newsreader) / … SIL Open Font License, Version 1.1"; `geistmono-OFL.txt:1-3` "Copyright 2024 The Geist Project Authors … SIL Open Font License, Version 1.1".

---

# C. Interaction, opening, sharing, navigation, instructions, licence

### 1. Highlight-to-feedback loop (clipboard, not a tool)

**Transport = clipboard.** There is no comment/feedback MCP tool, socket, or file. README.md:60: "If you see anything you don't like, highlight it in your clipboard and give it your agent, and it can re-draw on the Whiteboard". `instructions/scratchpad.md:3`: "The user reads it in the app and copies blocks back to you." `authoring-tools.ts:37` (restore desc) mentions "Comments are not rolled back" — legacy vocabulary; no comment tool exists in the catalog (`authoring-tools.ts:98-212`).

**Selection sources (three):**
- *Prose/block text in the canvas*: `packages/review/app/src/agent-text-selection.ts:96` listens to `selectionchange`; `:86-91` emits `select({ target:{kind:"text", quote}, title: quote.slice(0,100), anchor, anchorElement })`. `:60-77` rejects a drag that crosses an ineligible node (e.g. a diagram between paragraphs).
- *Code/diff range in the Code-OSS editor*: `apps/review-desktop/code-oss/src/vs/review/contrib/verbs/reviewApiSelection.ts:10-27` builds `{event:"editorSelectionChanged", reviewId, anchor, path, sideContext, isEmpty, range:{fromLine,toLine}, apiSource:{reviewId,version,commit,pins?}}`; `reviewDiffSelection.ts:4-7` expands intersected replacements to both sides "so the copied excerpt is a real diff" (→ `selectedDiff`). Wire schema: `packages/review-protocol/src/contracts.ts:1283-1294` (`ReviewSurfaceEventSchema.editorSelectionChanged`), `:1256-1268` (`ReviewSelectedDiffSchema` rows kind unchanged|added|deleted), `:1270-1281` (`ReviewApiSelectionSourceSchema`). `reviewCanvasPart.ts:207-211` forwards events only for the shown review.
- *Diagram node*: nodes are not selectable for feedback; clicking a node/code peek navigates (see §4). Only text quotes and code ranges feed the loop.

**Canvas side:** `packages/review/app/src/agent-selection.tsx:118-143` subscribes to `session.surface` and maps `editorSelectionChanged` → `target:{kind:"code", path, side, startLine, endLine}`, `selectedDiff`, `apiSource`, title `path:from–to`. `:153-198` `copy()` POSTs `/copy-context` with `{...selection, revision}`, parses `{text}`, calls `copyText(text)`, toast "Selection copied to clipboard. Paste into your agent to chat about it." (`:186-189`). Shortcut ⌘⇧C and Escape clears (`:200-216`); floating button `Copy for Agent` with `aria-keyshortcuts="Meta+Shift+C"` (`:240-243`), anchored inside `.review-document` so it scrolls with text (`:88-95`).

**Shape:** `packages/review/src/agent-selection.ts:8-25`
```ts
export const AgentSelectionSchema = z.strictObject({
  target: z.discriminatedUnion("kind", [
    z.strictObject({ kind: z.literal("text"), quote: z.string() }),
    z.strictObject({ kind: z.literal("code"), path, side: z.enum(["base","head"]), startLine, endLine }),
  ]),
  title: z.string().max(500), detail: z.string().max(24000).optional(), revision, selectedDiff, apiSource });
```
`selectionMarkdown` (`:28-46`): text → `> quoted lines`; selectedDiff → fenced ```` ```diff ```` with `Base:/Head:/Range:` header (`:48-83`, fence length grows past inner backticks `:63-70`).

**Server side:** `packages/review/src/review-api/http.ts:936-1013` `POST /:id/copy-context`: validates `AgentSelectionSchema`, rejects cross-review `apiSource` (`:949`), reads snapshot at `apiSource.version ?? query.version` (`:952-955`), quotes code via `data.quote(pins, {side,file,fromLine,toLine})` producing `## head: path:10-20 (commit)` + 4-space-indented source (`:960-983`). Response header lines (`:996-1013`): `Selected text|code from Whiteboard: <title>`, `Session ID: <reviewId>`, `Version: <n>`, optional `Selected commit`, `Selected repository ID/base/head`. This header is what lets the agent map the paste back to `sessionId` + version for `session_get`/`session_edit`. Shared (read-only) reviews still allow copy-context (`http.ts:174-178`).

**`session_activity` lease (how the agent 'owns' the doc, and how the user sees live writing):**
- Tool description `authoring-tools.ts:83`: "Acquire an exclusive authoring session for one scope of a review: begin with a fresh leaseId, pass that leaseId on every write in that scope, and end when finished… The lease expires after 3 minutes without an accepted edit or renewal. Another session gets a conflict while this lease is active. Use focus to show current work… End the session only when the review is finished: readers treat a review with content and no live session as ready."
- Schema `review-api/activity.ts:19-28`: `{action: begin|renew|end, leaseId: uuid, scope?: "document"|"lenses", focus?: {description ≤160, targetId?} | null}`. Scopes `:14-16`. `ACTIVITY_TTL_MS = 180_000` `:46-48`. SQLite table `authoring_sessions(review_id, scope, lease_id, expires_at, focus)` upsert `:219-229`. `assertWrite` 409s "being authored by another session" / "Authoring session ended or expired" `:129-153`. Every accepted write `extend`s expiry inside the write txn `:158-176`. Listeners notified on every change/expiry `:239, 257`.
- Snapshot to UI `:36-45`: `{workingCount, expiresAt, scopes?, focuses?}`.
- UI: `app/src/authoring-activity.tsx:48-52` joins focus descriptions with ` · `; `:92-97` badge text = description || "Agent working…" / "N agents working…"; `:132-137` "While an agent is writing, marker ink sweeps through the word [Whiteboard]. The document is ready once it has content and no authoring session is live" (`:158-161`). `authoring-cursor.ts:47-64` derives a per-scope cursor from `focus.targetId`; `courier.tsx:43-46` finds `[data-lens-id=…]` rows for the lenses courier. Error boundary `review-document-boundary.tsx:60-63`: "Your coding agent is writing the canvas now… Edit the review through the Whiteboard MCP tools or `whiteboard api`".
- Edit granularity rules that make live drawing work: `authoring-tools.ts:28` (edit desc) "While a reader may be watching, write small and often: one paragraph per edit, so the document draws itself as you go. Insert a new diagram whole… Change a diagram already on the board one unit at a time… Link each added flow_node to a node already drawn, so it arrives attached".

### 2. Opening a review from a terminal agent

**Public vocabulary:** internal tools are `review_*` (`authoring-tools.ts:48`), rewritten to `session_*`/`sessionId` at the agent boundary by `public-tools.ts:10-16, 37-59` (also `Review`→`Whiteboard`). `callPublicTool` rejects `reviewId` (`:95`).

**MCP entry:** `whiteboard mcp` → `review-api/mcp.ts:serveReviewMcp` (`:50-58`): stdio `Server{name:"whiteboard"}` (`:66-70`), catalog fetched from server `GET /authoring` and mapped through `publicTool` (`:87-89`); latches to one instance key across restarts (`:79-84`). `tools/list` always includes `session_get_instructions` + `whiteboard_status` even when Desktop is down (`:114-118`), and prepends `mcpAuthoringGuidance` to the instructions tool (`:30-49`, `:120-127`): "Call session_get_instructions before creating or editing a Whiteboard… session_create opens the new review in Desktop when it is available, so call session_open only for an existing review… Never read or write Whiteboard files or SQL. Reuse commandId and identical input after a lost response."

**CLI (`src/cli.ts`):** `maybeDelegateToDesktopCli` (`:61-155`) spawns the Desktop-bundled CLI (`discovery.cliPath`, Electron-as-Node `cliRuntimePath` with `ELECTRON_RUN_AS_NODE=1`) unless argv has `api|mcp|server|instances|--state-dir` (`:70-79`). `selectedDiscoveryFile` (`:159-197`): `DEV_REVIEW_INSTANCE` → `~/.dev/review-desktop/default-instance` → only live `instances/*.json` → `stable.json` → legacy `server.json`.

**Discovery (`src/desktop-discovery.ts`):** record parsed with `parseReviewDesktopDiscovery` and protocol-version mismatch error (`:46-85`); health = `GET {url}/health` with 1.5 s timeout, requires `ok && instanceId match && desktopAttached` (`:88-108`); `requireHealthyReviewDesktop` (`:121-129`); `selectReviewInstance` precedence env → default → only-running → stable (`:262-296`); instance keys `stable|preview|dev-*` (`:135-140`). Discovery record fields incl. `url, cliPath, cliRuntimePath, key, channel` in `review-protocol/src/contracts.ts:739-743`.

**`session_open` / auto-open:** tool `review_open` (`authoring-tools.ts:132-139`): "Show an existing review immediately and prepare current pinned checkouts in the background" → `POST /:reviewId/open`. `session_create` (`:25`) "When Desktop is available the review opens there… set open:false to author in the background without taking over Desktop." Desktop verbs `openReview`/`openApiReview`/`showReviewView` in `contracts.ts:1178-1236`.

**Launch form** `src/connect-prompts.ts:5-8`: `REVIEW_MCP_LAUNCH = { command:"sh", args:["-c", 'exec "$HOME/.local/bin/whiteboard" mcp'] }`; Windows `cmd /d /c whiteboard mcp` (`:15-18`). The Desktop writes the `whiteboard` shim to `~/.local/bin` (`packages/review/README.md:72-75`).

**Plugins (`packages/agent-plugins/`)** — MCP config only, no hooks/skills except Pi:
- claude `.mcp.json:1-8` `mcpServers.whiteboard = {command:"sh", args:["-c","exec \"$HOME/.local/bin/whiteboard\" mcp"]}`; installed via `claude plugin marketplace add devdotfast/whiteboard; claude plugin install whiteboard@devfast --scope user` (`connect-prompts.ts:109-111`); Windows uses `claude mcp add -s user whiteboard -- cmd /d /c whiteboard mcp` (`:97`).
- codex `.mcp.json` + `bin/whiteboard-mcp(.cmd)` (dir listing); `codex plugin marketplace add devdotfast/whiteboard; codex plugin add whiteboard@devfast` (`:113-115`).
- cursor `mcp.json` + `.cursor-plugin/plugin.json`; deeplink `cursor-deeplink.ts:2-11` = `cursor://anysphere.cursor-deeplink/mcp/install?name=whiteboard&config=<base64 {command,args}>`; prompt says open via `open`/`xdg-open`/`cmd /c start`, then fall back to merging into `~/.cursor/mcp.json` (`connect-prompts.ts:129-137`).
- opencode `index.js:2-18` plugin sets `config.mcp.whiteboard = {type:"local", command:[…], enabled:true}`.
- pi `skills/whiteboard/SKILL.md:1-8`: "Before authoring, run `whiteboard api session_get_instructions '{}'` and follow the result. Pass `'{"topic":"scratchpad"}'`… or `'{"topic":"trace-archaeology"}'`"; installed `pi install npm:@dev.fast/pi-whiteboard` (`connect-prompts.ts:120-123`).
- Trace search (optional) registers `fff` MCP for claude/codex (`connect-prompts.ts:74-91`).
- Verify step for all: call `session_get_instructions`, "Do not author anything yet." (`:145-151`).

### 3. Sharing

`packages/review-share-protocol/src/index.ts`: `SHARE_FORMAT="review-share/1"` (`:3`), limits manifest 2 MiB / object 32 MiB / total 128 MiB / 2048 objects (`:5-11`). Manifest (`:84-105`): `{format, reviewId, version, title, snapshot: sha256, presentation: sha256, objects:[{id,size,sha256}], resources:[{id, kind: image|trace|map, mimeType: image/png|application/json, object}], repository:{cloneUrl}}`; `cloneUrl` must be canonical `https://github.com/o/r.git` (`normalizeGitHubRemote` `:46-80`). Link `shareLink(origin,id,cap)` → `https://<origin>/s/<uuid>#<43-char capability>` (`:154-176`), capability in the fragment.

What's shared (`src/sharing/export.ts`): the stored snapshot minus `target/staleSources/sourceUnavailable/lastEdit` (`:54-61`), i.e. document + title + pins + lenses; `presentation = {maps}` (`:162-163`); resources (images/traces/maps) as objects (`:121-152`). Worktree targets refused: "Pin this review to commits before sharing it." (`:63-64`); "A document without source pins of its own cannot be shared." (`:105-109`). Source text is **not** bundled — only validated (`:155-160`); importer re-clones `manifest.repository.cloneUrl` at `snapshot.pins` and detaches at head (`src/sharing/import.ts:335-348`). Shared IDs `shared-<sha256>` (`contracts.ts:1174`). README.md:85 caveat: updates after sharing don't propagate.

### 4. Navigation

- **TOC**: `app/src/review-toc.tsx:37-40` renders `ReviewTocEntry[]` (from `review-document-headings`), numbered; wide shell ≥1360 px keeps a rail beside prose else breadcrumb pill/drawer (`:20-26, 49-58`); `aria-label="Contents"`, Escape closes (`:273-278`).
- **Find**: ⌘F in canvas (`code-oss/src/vs/review/browser/parts/canvas/reviewFind.contribution.ts:28-30`); Enter/Shift+Enter navigate (`app/src/review-find.tsx:369-376`).
- **Desktop keys**: ⌘B toggle file tree (`contrib/explorer/reviewFileTree.contribution.ts:35-40`; hint `desktop-entry.tsx:52`), ⌘, settings (`contrib/settings/reviewSettings.contribution.ts:20-25`), ⌘⇧C copy-for-agent (§1).
- **Diagram → code**: canvas posts verb `reveal {path,startLine,endLine,side,pins,highlight:true,preserveFocus:false}` (`app/src/host/review-host.tsx:32-44`; schema `contracts.ts:1138-1160`); callers `CodePeek.tsx:98,201,246`; flow nodes activate on Enter/Space (`flow-graph.tsx:394-398`); `openDiff {path, previousPath}` (`review-host.tsx:27-30`); Markdown `review-source:head/file.ts#L10-L24` links (`instructions/authoring.md:37`, renderer test `agent-markdown.test.tsx:14-27`).
- **Minimap**: only the software map has one (react-flow-style `MiniMap` props, `software-map/SoftwareMap.tsx:1682-1690` [INFERENCE on library]); keyboard Enter/Tab/Escape (`:1543-1580`).
- **File lenses**: `review-api/diff-lenses.ts:16-38` `lens = {id, title, targets:[{kind:"files", patterns}|{kind:"ranges", sources}]}`, `LENS_LIMIT=200` (`:42`), edits insert/update/remove with `afterId` (`:46-62`); patterns matched against changed-file list only (`file-lenses.ts:85-89`); `uncategorizedSources` = changed lines no lens covers (`:14-18`). Diff view renders `.diff-lens-section[data-lens-id]` per lens (`app/src/DiffView.tsx:254-257`) plus `automatic-uncategorized` (`lens-courier.browser.test.tsx:257`). Lenses live beside the document, versioned with it, own `lenses` lease (`authoring-tools.ts:29`).

### 5. Instructions & tool guidance — verbatim rules

`packages/review/instructions/authoring.md`:
1. `:1` "you are writing an interactive rfc-style whiteboard, for consumption by a staff engineer."
2. `:4-10` "follow these first six steps exactly, without any extraneous tool calls. - register the repository - create the whiteboard… - trigger a subagent… `session_get_instructions({topic:\"file-lenses\"})`… - begin `session_activity` with scope `scope: \"document\"` - read the diff with `session_diff` - immediately after reading the diff, without any other tool calls - put down a first pass at the what/why section."
3. `:12-15` "each of these should be written as a top-level `section`, in this order: what/why … requirements … design … implementation".
4. `:16-19` "pick one diagram that best shows the shape of the change: `sequence` if participants interact over time… `flow_diagram` if the interesting part is branches, retries or state transitions… `database_lens` if the interesting part is a schema change".
5. `:20` "if the change is mostly a new/changed contract, show the key types / interfaces as `code_peek`(s)".
6. `:23` "`call_stack_diff` for the old vs. new path through user flows. always root the flows in the user/agent entry point (eg a button click, CLI command, etc.), including unchanged nodes along the way."
7. `:24` "`code_peek` for the few spots that carry the mechanism or an invariant; link everything else inline".
8. `:26` "before finishing, read the whole whiteboard back and fix any contradictions/unverified claims."
9. `:33` "IMPORTANT: Write incrementally. The user sees you write on the canvas in real-time. Show them visual progress every few seconds."
10. `:35` "keep whiteboards short and sweet when possible (esp. for small changes.) feel free to omit sections."
11. `:36-37` "when something (a phrase in the prose, diagram node, etc.) describes actual code in the codebase, always default to attaching/hyperlink code. - Link repository code as `[label](review-source:head/src/file.ts#L10-L24)`; use `base` for old code."

`instructions/trace-archaeology.md`:
12. `:3` "Agent-written commits record `Agent-Session: <id>` trailers. Use the `whiteboard trace` CLI to resolve and pull those sessions. Use FFF to find candidate events. Use `whiteboard trace show` for exact evidence."
13. `:11` "Enable only user-authorized repositories and origins."
14. `:13` "Without authorization, leave capture off and continue read-only investigation."
15. `:15` "FFF setup is human-owned. If FFF is unavailable, report the setup gap. Do not replace or reconfigure it."
16. `:32` "Add `--history` only when the current provenance does not explain the decision".
17. `:44` "Read the absolute normalized file paths from the response's `paths` array. Do not derive them from the corpus layout."
18. `:48,55` "Treat each result only as a candidate locator." … "Ignore physical line 1 because it is trace metadata. For a match on line `<L>`, use event index `<L> - 2`."
19. `:63` "Search results are not final evidence."
20. `:65` "Check the current code before you explain the result. The trace can describe a decision that the author later reversed."
21. `:109-113` Evidence Rules: "Treat traces as historical evidence, not current specifications." "Prefer `--json` when you parse command output." "Cite session, trace, and event locators for event evidence." "Cite commit IDs and source lines for current code evidence." "Report missing or uncertain provenance. Never invent an explanation."

`instructions/file-lenses.md`: 22. `:8-10` "categorize all the changes into buckets - leaving nothing in uncategorized changes by the end - first, categorize away non-implementation code: tests, docs, generated files and lockfiles… - then… split it by the part of the design each file serves… keep each lens small enough to read in one sitting, and don't split a file across lenses unless it holds two unrelated changes."

`instructions/scratchpad.md`: 23. `:12` "Answer in chat when one sentence or one code line does it. Do not use the scratchpad to review a change; author a session instead". 24. `:17` "A reference without pins is rejected, since the scratchpad has none to lend." 25. `:23` "Keep it a log, not a document: the newest thought sits at the top, above older ones. Do not build sections, outlines or status markers unless the user asks for structure."

`session_edit` tool description (`review-api/authoring-tools.ts:28`, sizes: no byte limit stated; structural minimums only): 26. "Sections require `children` (not `blocks`); Markdown uses `markdown` (not `text`)." 27. "Use nodes and edges arrays (not children); at least one node is required, edges may be empty, and edge endpoints name unique node keys." 28. "Both base and head arrays are required; either may be empty. Each frame requires source; optional key/parentKey express nesting, and parentKey must name an earlier frame on that side." 29. "steps reference actor keys and each needs exactly one of source, explanation, or code." 30. "Include at least one store and use case, with at least one operation per use case; each operation requires source and must reference declared keys." 31. "Source paths are repository-relative; line numbers are 1-based and inclusive. Replace example paths and lines with verified source ranges." 32. "Omitted placement appends; on the scratchpad it lands at the top, so insert a multi-block thought bottom-up or chain each block with afterId." 33. "Insert a new diagram whole… Change a diagram already on the board one unit at a time… a separate flow_edge is only for two nodes that already exist. Removing a flow_node removes its edges."
Other limits: focus description ≤160 chars (`activity.ts:8`); lens patterns ≤1000, range sources ≤10000, lenses ≤200 (`diff-lenses.ts:14-21,42`); selection title ≤500, detail ≤24000 (`agent-selection.ts:18-19`). `session_lens_edit` (`authoring-tools.ts:29`): "Write one lens per call while a reader may be watching… Keep adding lenses until [uncategorized] is empty or what remains is deliberate." `session_get_instructions` (`:59-64`) routes topics default | file-lenses | trace-archaeology | scratchpad.

### 6. Licence

- Root `LICENSE:1-3`: MIT, "Copyright (c) 2026 dev.fast". `packages/review/LICENSE`, `packages/review-protocol/LICENSE`, `packages/agent-plugins/{pi,opencode}/LICENSE` exist (glob/dir listing; contents not read — [INFERENCE] same MIT).
- `apps/review-desktop/LICENSE:1-3` MIT dev.fast; `:23-28`: "The code-oss/ directory contains a fork of Code - OSS (https://github.com/microsoft/vscode), Copyright (c) 2015 - present Microsoft Corporation, used under the MIT License. See code-oss/LICENSE.txt and code-oss/ThirdPartyNotices.txt". README.md:108-112 repeats this. No root `THIRD_PARTY_NOTICES.md` exists (glob returned only `code-oss/LICENSE.txt`); the notices file is `apps/review-desktop/code-oss/ThirdPartyNotices.txt` (3440 lines, Microsoft's upstream list starting `@fig/autocomplete-shared 1.1.2` MIT, `:1-33`).
- `apps/review-desktop/UPSTREAM:1-4` pins microsoft/vscode `8a7abeba…`, lists security backports, pruned extensions (`:120-140`), and every intentional divergence (`:180+`). Electron itself is MIT (upstream dependency; not separately noticed in repo — [INFERENCE]).

**What may be copied verbatim with attribution (MIT permits all of it; obligation = keep copyright + permission notice, `LICENSE:11-12`):**
- Instruction prose (`instructions/*.md`), tool descriptions (`authoring-tools.ts`), MCP guidance (`mcp.ts:30-49`) — dev.fast MIT; attribute "Copyright (c) 2026 dev.fast, MIT".
- Type/zod shapes (`agent-selection.ts`, `activity.ts`, `diff-lenses.ts`, `review-share-protocol/src/index.ts`, `review-protocol/src/contracts.ts`) — dev.fast MIT; shapes are also thin enough that re-implementation avoids any notice burden.
- CSS/React in `packages/review/app/src` — dev.fast MIT; copy with notice.
- Anything under `apps/review-desktop/code-oss/` (including `src/vs/review/**` fork additions) — dual provenance: dev.fast additions MIT, but files derived from VS Code carry Microsoft's copyright and the full `ThirdPartyNotices.txt` obligation. Re-implement rather than copy unless the design doc's product is itself a Code-OSS fork.
**Recommend re-implementing** (not licence-blocked, but tightly coupled to Whiteboard's server/SQLite/Hono): `http.ts` copy-context route, `ReviewActivity` class, sharing export/import. **Recommend borrowing by design only**: clipboard header format (`Session ID`/`Version` lines), lease TTL + focus semantics, `review-source:` link scheme, section-order/diagram-choice rules.

### Gaps / not observed
- Desktop-side handler that turns `reveal` into an editor open (`reviewCanvasPart.ts` beyond `:207-211`) not read.
- `packages/review/src/evidence.ts:4-15` is only quote normalisation (`normalizeQuoteText`, `textIncludesQuote`) — supports trace-quote validation, not user feedback.
- Cursor `mcp.json` and codex `.mcp.json` contents not read; connect-prompts.ts:5 asserts they match `REVIEW_MCP_LAUNCH` byte for byte.

---

# D. theuth today — the seams the design reuses (repo `theuth-wbdoc` @ 9f772b9)

### 1. Artifacts

**Record.** `crates/artifact/src/lib.rs:137-171`:
```rust
pub struct Artifact {
    pub id: String,        // sha256[..16] hex; blob file stem
    pub session: SessionId, pub call_id: String, pub name: String, pub title: String,
    pub kind: Kind, pub mime: String, pub bytes: u64,
    pub version: u32,      // per (session, name), 1-based
    pub path: PathBuf, pub source: Option<PathBuf>, pub preview: Option<PathBuf>, pub pages: Option<u32>, pub origin: Option<PathBuf>, pub created_at: i64 }
```
- `Kind` (`lib.rs:69-96`, serde lowercase): `Markdown, Text, Csv, Image, Svg, Pdf, Html, App, Archive, Mermaid, File`. Derived from extension only in `kind.rs:20-42` (`md|markdown`→Markdown, `mmd|mermaid`→Mermaid, code exts→Text, unknown→File); `App` never derived — only `Commit` (`lib.rs:225-247`) creates one. Mime via `mime_guess` w/ fallbacks `kind.rs:45-70` (`text/vnd.mermaid`, `text/x-openui`).
- Size cap `MAX_BYTES = 32 MiB` (`lib.rs:44`). Records are immutable; “same name again in this session makes a new version” (`tools.rs:24-26`). Record key is `call_id`, not blob id (46 §1.2, `docs/design/46-apps.md:107`).
- Storage: `<data_dir>/artifacts/<id>.<ext>` + sqlite index (`lib.rs:1-4`); content-addressed blob store `store::State`. Markdown shows also bind a `document` (`store.rs:1058-1101` `bind_document`: `Lookup{Binding::Artifact{session,name}}` → `import_markdown` or `Ensure`, then `Snapshot{"artifact-v<N>"}`); a user edit export becomes the next version through `edit_version` (`store.rs:1125`, `lib.rs:648-664`). Design ruling: viewer stays read-only streamdown, document is storage only (`docs/design/59-documents.md:721-733`).
- Event `ArtifactShown { session, call_id, artifact }` tagged `type` (`lib.rs:171-181`).

**Tool `show`** (`tools.rs:46-87`): `NAME="show"`, `Risk::ReadOnly`, `DIRECT=true`. Args `ShowArgs` (`tools.rs:16-30`): `path?`, `content?` (requires `name`), `name?`, `title?` (default name / first md heading), `source?: markdown|typst` for `.pdf`. Output: receipt text `shown report.pdf (v1 · 3 pages · 84 KB) · source report.md` (`kind.rs:120-150`), `structured = Artifact`, `files = preview png`. Prompt block `PROMPT` (`tools.rs:33`) registered `RegisterPrompt{plugin:"artifact"}` at `lib.rs:795`.
**Tool `artifact_read`** (`tools.rs:118-233`): by `call_id`, `offset/limit/tail/ranges` like `read_file`; header names `v<N> of <M>`, `latest is <call_id>`, `editable as document <id> (doc_read)`.

**RPC** (`crates/rpc/src/api.rs`): `list_artifacts(session?, descendants?)` L1330; `export_artifact(id, dest)` L1338; `get_app(record_call_id)` L1344; `publish_app(session,name,source)` L1350; `get_artifact(call_id)` L1367 (falls back to mesh peer fetch L4125); `get_artifact_listing` L1373; `artifact_search_match(call_id, query)` L1378. Blob bytes via `get_attachment`-style path read restricted to `attachments|browser|computer|artifacts|spill` (L4237). No per-version RPC: versions are derived client-side by filtering `list_artifacts` on `(session,name)` (`Viewer.tsx:374-377`, `ArtifactPanel.tsx:40-46`).

**Desktop viewer** `components/artifacts/Viewer.tsx`: `ArtifactView` L280 → `Body` L288 dispatch: `app`→`AppView`; `image|svg`→`Zoom`+`Img`; `pdf`→`PdfPages` (pdf.js) on touch else `<iframe>` (L266-276); `archive`→`ArchiveView`; `file`→download button; text loaded via `useText(path)` (`ArtifactCard.tsx:86-104`), then `markdown`→`Markdown` (L46-61: `<Streamdown mode="static" plugins={plugins} shikiTheme linkSafety off>`; http links → OS opener), `mermaid`→`MermaidView`, `text`→Streamdown of `fenced(text, ext)` (`MessageAssistant.tsx:36-40`), `csv`→`Csv` table (500 rows), default (`html`)→sandboxed `<iframe srcDoc={CSP+text}>` (CSP L37). Because `plugins` is the chat’s (`MessageAssistant.tsx:25-32`), ```json-render/```mermaid/```openui fences inside a markdown artifact render exactly as in chat, but with default `FenceContext` (static settled) since no provider wraps them (`JsonRenderFence.tsx:14`).
- `Viewer` route component L347: `get_artifact(callId)` exact record; version `Select` navigates to sibling call_id (L448-462); actions Reveal / Open chat / Popout / `ArtifactMenu explorer` / details rail (260 px, `lg:` else stacked below); mobile `Esc` = back (L362-369); `Frame` L488: 52 px header, back chevron `md:hidden`. `/artifacts` route: `isPopout() ? Outlet : Explorer` (`routes/artifacts.tsx:19`).

**ArtifactCard** (`components/chat/ArtifactCard.tsx`): routed from `MessageTool.tsx:129` for tools `show|app|task_draft|agent_draft`. States L391-520: artifact present → title/meta/`v{n}` chip/actions (Reveal, Open in panel, Popout, ⋯ menu); `result.isError` → red block, first line + “Show errors”; else skeleton with `Progress` while `tool.pending` (`progressLabel` L272: `writing report.pdf… 3.2 KB` / `showing x…` / `drafting <app>… KB`) or `compiling…`. `ArtifactMenu` L137-227: Copy (TEXT_KINDS), Save as/Download, Open, Open in explorer, Attach to this chat / Attach to chat… (via `attachTo`+`toRef`), Copy path. `TEXT_KINDS` L57-69.

**ArtifactPanel** (`ArtifactPanel.tsx`): Radix `Sheet` right side, `DEFAULT_W=640`, `MIN_W=360`, drag strip to ≤50 vw (L262-278, L288-296); phone: `w-full`. `Content` L78: opened on head version → “latest mode” follows new versions; else pinned; version Select; app drafts (`AppDraft`) attach to same view. Auto-open once per `turnSeq` while `document.hasFocus()` on first `ArtifactShown`, or earlier on first renderable `app` draft chunk via `ToolCallProgress` (L190-232); close suppresses for the turn.

**Streaming.** `show` content is NOT visible mid-call: `ToolCallProgress` carries cumulative partial args (coalesced 250 ms / ≥512 B, `crates/agent/src/lib.rs:1574-1587`), the store keeps only `{name, writing, bytes}` from `partialShow` (`store/reduce.ts:86-101`, `reduce.ts:588-595`); the artifact appears only at `ArtifactShown` (`reduce.ts:607-615`). Only `app` calls decode partial `source` into an `AppDraft` (`reduce.ts:597-605`, `partialApp` L106-131) and render progressively (46 §8.1 as-built `docs/design/46-apps.md:700-711`; only `app` opts into Anthropic eager input streaming, `agent::loop_::direct_tool`). Store shape `progress: Map<call_id,{name?,writing?,bytes}>`, `appDrafts: Map<call_id, AppDraft>` (`store/sessions.ts:213-219`, `AppDraft` L48-60).

### 2. Notes / documents

- A “note” is a `document` crate doc of `DocKind::Note` (`crates/document/src/meta.rs:83-97`: `Knowledge, Artifact, MeetingNote, Note, AgentCharter`), bound by `Binding` (`meta.rs:118-134`: `Knowledge(String) | Artifact{session,name} | MeetingNote(String) | Note | AgentCharter(String)`). Storage: Yjs state + snapshots + derived `documents/<id>/note.md` (`api.rs:776`), Y state ≤8 MiB, update ≤48 KiB (`59 §4.7`, `api.rs:752-759`). Helper process `theuth-documents-<version>` (`api.rs:785-789`).
- RPC (`api.rs:733-837`): `doc_list(kind?,limit?)`, `doc_resolve(binding)`, `doc_ensure`, `doc_markdown`, `doc_open(doc, sv?)`, `doc_update`, `doc_awareness`, `doc_subscribe`, `doc_snapshots`, `doc_restore`, `doc_undo_agent`, `doc_create(title, markdown?)`, `doc_rename`, `doc_delete`, `documents_status`, members/links.
- Agent tools (`crates/document/src/tools.rs`): `doc_read` L167 (`format: outline|markdown`, `blocks`, `depth`; outline = one line per block id-first; markdown = `<!-- id -->` before each block), `doc_insert` L237 (`after/before/at`), `doc_replace` L294 (block ids → markdown), `doc_delete` L334, `doc_move` L374, `doc_create` L440. `doc` arg resolves `DocId | knowledge:<id> | meeting:<id> | agent-charter:<name>` (L67-105). Edits stamped `Principal::Agent{session}` (L109-114). Prompt `RegisterPrompt{plugin:"document"}` L44. **Section model = BlockNote block ids, no `## ` section/TOC model**; headings are blocks (`doc_move` “regroup under headings”, L369).
- Editor: `components/editor/Editor.tsx:77` BlockNote `withCollaboration` over `EngineProvider`, schema `createDocumentSchema(frontmatterBlock, codeBlock)` L33; slash menu = headings 1–3, lists, code, table, quote, divider (L38-51) + custom items `Diagram` (codeBlock `language: mermaid`) and `Ask theuth…` → `onAsk(editor.getTextCursorPosition().block.id)` (L118-147). Coarse pointer → `MobileToolbar`, no floating menus (L155-171).
- Custom code block `components/editor/CodeBlock.tsx:161` (`createReactBlockSpec` over BlockNote’s default config): `diagramOf(language)` (`codeLanguages.ts:21-28`: `mermaid|mmd`, `json-render|jsonrender`) → when not selected and parses, renders `MermaidFence`/`JsonRenderFence` with `isIncomplete={false}` (L72-74); click/“Edit source” returns to source. Only 2 diagram languages; `openui` not supported in notes.
- Routes: `routes/notes.tsx` (list), `routes/notes_.$id.tsx` → `NotePage` (`components/notes/NotePage.tsx:29-33`); `/notes/$id?new` selects title. `NotePage` `ask(block)` L111-118: `prefillDraft(chat.session, \`about block ${block}: \`, [], true)` then opens the `NoteChat` panel (and dispatches `theuth:note-ask` to reseed if already open). `NoteChat.tsx:23`: bottom panel, drag-resizable, `Transcript compact` + `Composer pinned={doc}` — every message leads with `[attached note <id> "title" · meta]`; first message creates a session titled like the note (`onCreated`). Store `lib/note-chat.ts:11-40`: zustand + localStorage key `note-chat`, per note `{open, height(320 default,160 min), session}`.
- Knowledge (`components/knowledge/Document.tsx`) is a separate CodeMirror/doc view; not needed for board.

### 3. Fence streaming in chat & the ask-from-surface path

- `MessageAssistant.tsx:174-249`: `<FenceContext value={{running, hasLaterAssistant, session}}>` wrapping `<Streamdown mode={open?undefined:'static'} isAnimating={open} plugins shikiTheme>`; `hasLaterAssistant` computed by forward scan of entries (L191-199). Actions row when closed: read aloud, copy, regenerate, fork, remove-from-context. `plugins` L25-32: `renderers: [{language:'json-render', component: JsonRenderFence}, {mermaid: MermaidFence}, {openui: OpenUIFence}]`, `code` from `@streamdown/code`, `shikiTheme ['github-light','github-dark']`.
- `JsonRenderFence.tsx:65-79`: `isIncomplete ? parsePartial(code) : parseSpec(code)`; open fence → progressive `Renderer loading` inside `Boundary`; closed → `FenceFallback` on error. `FenceFallback` L23-35: `hasLaterAssistant`→`null`; `running`→pulse skeleton; else one dim line `◌ diagram omitted · show source` with `<details>`. Exported as `ErrorCard` for `Mermaid.tsx`.
- `surface/parse.ts`: `parseJson` strict then `jsonrepair` (L33-39); `parsePartial` L168-195 drops unknown/half-typed elements, prunes dangling children, `tree()` check; caps `MAX_ELEMENTS 256, MAX_DEPTH 32, MAX_GRAPH {120,240,12,80}` (L11-14) mirroring `crates/surface/src/lib.rs:33-49` (`MAX_FENCE_BYTES 256 KiB`, `MAX_MERMAID_BYTES 128 KiB`). Rust `surface::parse`/`check`/`partial_show` (`lib.rs:80-136`); one correction round (30 §5).
- Render queue `lib/render-queue.ts:18-70`: `createRenderQueue(render, intervalMs=80)` — one in flight, one trailing request replaces older, `isCurrent()` guard, `dispose()`. Used by `Mermaid.tsx` (L1-4: mermaid 12 lazy chunk, `securityLevel:'strict'`, `htmlLabels:false`, ELK default). Design 65 §4.3.
- Streamdown usage sites: chat (`MessageAssistant.tsx:222`), artifact markdown/text (`Viewer.tsx:55,317`), `SkillCard`/`SkillBody`.

**`@ToAssistant` / provenance path (reuse for select-to-ask).** No typed provenance field exists on the wire: `UserInput` (`crates/agent/src/loop_.rs:126-146`) is `{id, text, files, ticket, scope, defer}`; `Input.peer: Option<PeerMeta>` is only for peer sessions (L160-162). Provenance is a **text header** the client prepends:
```ts
// components/apps/lifecycle.ts:344-357
export function assistantInput(app: {name; version; record_call_id}, action_id, message, context, form?) {
  const head = `[app ${app.name} v${app.version} · ${app.record_call_id}${action_id ? ` · action ${action_id}` : ""}]`;
  let out = `${head}\n${message.trim() || "(button pressed)"}`; /* + "\n\ncontext:\n…" + "\n\nform …:\n{json}" */
```
`AppHost.tsx:430-452`: on `continue_conversation` → `assistantInput(...)`, then if `sessions[session].running` → `applyEvents([InputQueued])` + `api.steer(session, text, id, [])`, else `st.markSent(session,text)` + `api.send_input(session, text, [])`. RPC `send_input(session?, text, files, delivery?)` `api.rs:162-169`, `steer(session,text,id?,files,delivery?)` L173-180. Transcript renders it back as an `AppInputCard` via `parseAppInput` (`lifecycle.ts:377-399`, regex `APP_HEAD` L371; `Message.tsx:31-33`). Reply fences reject `@ToAssistant` (`crates/app/src/fence.rs:80-98`); task/agent draft cards use `@ToAssistant("draft <id>: <choice>")` (`crates/app/src/draft.rs:170-181`). Attachments marker grammar (66 §3.3) is the other precedent: `lib/attach.ts:48-52` `[attached <kind> <id> "<title>" · <meta>]`, mirrored in Rust `crates/agent/src/attach.rs:85,132,153`, prompt block `RegisterPrompt{plugin:"attachments"}` (`supervisor.rs:7019`, `attach.rs:17`).

**Composer quoting.** No “quote selection / reply-to” feature exists in `Transcript.tsx`/`Message.tsx`/`Composer.tsx` (grep `quote|selection|replyTo` → only textarea `selectionStart` and `lib/selection.ts` list multi-select). Existing prefill hooks: `prefillDraft(session|null, text, refs, panel)` (`Composer.tsx:62-64`, drafts store keyed `panel:<session>`), `attachTo(session, ref)` → `theuth:attach` CustomEvent (`attach.ts:243-246`, listener `Composer.tsx:352`), `openAttachPicker`, drag type `application/x-theuth-attach`, `@` trigger `atTrigger` (`attach.ts:257-263`). `Composer` props `pinned?: AttachRef` (L157, marker leads every message), `compact`, `onCreated`, `sessionTitle` (L146-163).

### 4. `app` tool / OpenUI

- `crates/app/src/tool.rs:22-37` `AppArgs { name, source, title?, edit: bool, base_call_id? }`; validated by headless helper then `artifact::Commit{kind: App, source: ("app.json", meta), expected_base}` (`lib.rs` doc, 46 §3.2-3.3). Prompt `prompt()` L94 registered `RegisterPrompt{plugin:"app"}` (`crates/app/src/lib.rs:350`); compact signatures for 31 components (L52-86) + `EXAMPLE` L43-50. Actions list L103: `@Run, @Set, @Reset, @OpenUrl, @ToAssistant(message, context?)`.
- `publish_app(session, name, source)` (`api.rs:1350`, impl L4090) commits a reply fence as an app record under `ui-<uuid v7>` call id with no model turn; `OpenUIFence.tsx:48-60` `openAsApp` → panel. `get_app` returns `AppDoc` (artifact, source, hash, bindings). `app_close` L1364.
- Rendering: `components/apps/AppView.tsx:25` lazy `AppHost` (Suspense); `preloadAppView()` L11 on `ToolCallPending name==='app'`. `AppHost` owns runtime, activations, `onAction` (L420-453). Opened in the **panel** (`ArtifactPanel` `AppView` L165-171) or explorer route (`Viewer.tsx:293`), never its own route. Reply fences: `OpenUIFence.tsx:96` `AppRenderer isStreaming={isIncomplete} provider={staticProvider}` (all calls reject, L26).
- OpenUI `CodeBlock(language: string, codeString: string)` — syntax-highlighted only, no line numbers/ranges/annotations (`packages/apps/generated/catalog.txt:30`); `TextContent(text, size?)` markdown text block with 5 sizes (L24); `MarkDownRenderer(textMarkdown, variant?)` L25. Positional args only; `MAX_SOURCE` in `crates/app/src/meta.rs`.

### 5. Mobile

- `canPopout() = container() !== 'ios'` (`lib/popout.ts:86`; rule `docs/design/70-as-built-MOBILE-FIXES.md:7`): every pop-out affordance hidden on iOS shell; narrow desktop still pops out. `container(): 'macos'|'linux'|'windows'|'ios'|'web'` (`lib/platform.ts:2-13`), `isDesktop()` L16, `touch()` = `(pointer: coarse)` L22 (also `lib/hotkeys.ts:32`).
- `useIsMobile()` `hooks/use-mobile.ts:5-19`: `window.innerWidth < 768` via matchMedia. Used for: Viewer Esc-back & rail stacking (`Viewer.tsx:353,362`), DocumentPage rail `max-h-[40%]` below editor (`editor/DocumentPage.tsx:152`), meeting page stacking (POPOUT-2 doc L37).
- `usePointerCoarse()` `lib/mobile.ts:17` (re-reads on change); `useKeyboardInset()` L38 for `position: fixed` bars above the iOS keyboard; safe areas via CSS vars `--safe-top/right/bottom/left` (`routes/__root.tsx:23`).
- Panels/sheets on phone: `ArtifactPanel` Sheet is `w-full` under `sm` (L290); `Sheet` sets `pt-[var(--safe-top)]` (comment L289); `NoteChat` is a bottom panel with drag handle; 44 px targets on coarse (`CodeBlock.tsx:88`, 30 §8.3 fullscreen controls). Editor mobile rules `docs/design/59-documents.md:947-985`: coarse → no side/formatting/slash menus, `MobileToolbar` fixed at `--kb-inset`, focused block `scrollIntoView`, `font-size ≥16px` inputs, Enter inserts block. PDF: pdf.js on touch (WKWebView iframe shows only page 1; canvas edge ≤3000 px, `Viewer.tsx:191-262`). Pinch-zoom `Zoom` component (L104-190) because shell locks viewport scale.
- Design 58 is not in `docs/design/` (only referenced: cross-platform floors in `docs/design/cross-platform/*`; 59 §2.1 says “58 is the newest”). Relevant floors seen: `platform` leaf crate (`00-architecture.md:760`), Windows/Linux daemon (`cross-platform/step-14…:121`). No scrolling-document-specific rule found beyond 59 §10 and 70-MOBILE-FIXES.

### 6. Popout

`lib/popout.ts:10-19`:
```ts
export type PopoutTarget =
  | { kind: 'chat'; session: string }
  | { kind: 'artifact'; callId: string }
  | { kind: 'note'; doc: string }            // /notes/$id
  | { kind: 'meeting'; meeting: string }     // /meetings/$id
  | { kind: 'meeting-note'; meeting: string }// /meetings/$id?pane=note
  | { kind: 'pty'; session: string; job: string }
```
Adding a kind = 4 edits in this file: the union (L10), `idOf` (L21-31), `pathOf` (L33-43, route path), `popoutTarget(href)` parser (L52-67, `head` segment → target); label is `popout-<kind>-<safe id>` (`popoutLabel` L48), URL `<path>?window=<label>` (L50); `POPOUT` read once at boot (L70). Then: the route’s component must render alone under `Popped` (`routes/__root.tsx:16-24`, `SidebarProvider open=false`, safe-area padding) — e.g. `routes/artifacts.tsx:19` `component: isPopout() ? Outlet : Explorer`; page sets `document.title` when popped (`Viewer.tsx:358-360`); hide its own popout button (`!popped && <PopoutButton/>`); optional close-on-delete hook in `App.tsx:298-302` (`popoutSession`/`popoutMeeting`). Tauri host `open_window` command focuses-if-open (L89-93). Precedent write-up: `docs/design/70-as-built-POPOUT-2.md:9-15`.

### 7. Codeindex envelopes (`crates/codeindex/src/tools.rs`)

All return `ToolOutput{structured, text}` via `structured()`; every envelope carries `root, indexed_files, stale, pending, dropped, truncated_walk, content` (`Envelope` L77; e.g. L866-873).
- **`code_symbol`** (L717-887; description L725): `{definition, body, callers, ambiguous, callees, cochange, sections, …}`. `definition` = serialized `Definition` (`store.rs:740-756`):
```rust
pub struct Definition { pub symbol_id: i64, pub name: String, pub kind: String,
    pub path: String,        // root-relative
    pub line_start: i64, pub line_end: i64,   // 1-based, inclusive
    pub signature: String }  // definition line
```
`body`: string of source lines `line_start..=line_end` (`body_lines` L658-676), ≤`BODY_MAX 200` else head 120 + `… N lines cut …` + tail 60, then `budget_render` may drop more (L811-816) — **so `body` is not a faithful excerpt for pinning; pin by `path:line_start-line_end` and re-read**. `callers: [{path, count, refs:[{line, from_symbol, kind, provenance, confidence}]}]` (L679-702); `callees: [{name, target, count, provenance, confidence}]` (L705-724); `cochange: [{path, count}]`. Ambiguous name → `{candidates:[{symbol_id, path, line, kind, signature}], content}` (L780-799). Symbol arg accepts name | `path:line` | symbol_id (`CodeSymbolArgs` L628-631).
- **`code_map`** (L546-624; description L556): `{rows: Tag[], content: string, estimated_tokens, focus_files?, focus_idents?, pagerank_iterations?}`; `Tag` (`rank.rs:100-112`): `{path, line (1-based), kind, name, signature, rank: f64}`; `paths_only` → `rows: [{path, rank, lines, language}]` (L596-599). `content` = text map fitted to budget (`rank::render`).
- **`code_impact`** (L989-1231; description L997): `{mode: "files"|"range", range: {old,new}?, include_tests, changed: [{path, status, symbols_changed: [{name, kind, line}]}], unindexed: [path], direct: [{path, via: [name|"<import>"], is_test, changed}], transitive: [{path, depth≤3}], tests: [path], ambiguous_skipped, unresolved_imports, content}` (L1213-1229). Other tools: `code_find` L161, `code_outline` L264 (`Node{name,kind,line_start,…}` L231), `code_refs` L402. All `DIRECT=false`; prompt `RegisterPrompt{plugin:"codeindex"}` L45.

### 8. Prompt & skill plumbing

- Per-plugin prompt blocks: `reg.ask(RegisterPrompt{plugin: "<name>", text})` in each crate’s start/register: artifact `artifact/src/lib.rs:795`, app `app/src/lib.rs:350` (`tool::prompt()`), surface `agent/src/supervisor.rs:7026` (`surface::prompt_block()`), attachments `supervisor.rs:7019`, agent `supervisor.rs:7003`, document `document/src/tools.rs:44`, codeindex `codeindex/src/tools.rs:45`, knowledge/routes/schedule/scrape/secrets/tasks/roster/meeting/computer/search/tools similarly. Convention: `pub const PROMPT: &str = "## <Heading>\n…"` in `<crate>/src/tools.rs`, static (cache-safe), registered when `<plugin>.enabled`.
- Skills: templates are `include_str!` markdown with `{{catalog}}` placeholders — `crates/surface/visualize.md` (front matter `name: visualize`, “Choose the block” routing table L14-43) filled by `surface::visualize_skill()` (`prompt.rs:11, 265-286`, test `skill_is_generated_and_valid` L424-428); `crates/app/apps.md` filled by `app::skill()` (`lib.rs:52-58`, `{{catalog}}` from `packages/apps/generated/catalog.txt`, `{{hash}}`; test L384). Written to `<data_dir>/skills/<name>.md` on every start by `first_run` (`crates/theuth/src/main.rs:716-733`: `harness.md, visualize.md, pdf.md, loops.md, apps.md`, rewritten when content differs). Skills index/always bodies: `crates/agent/src/skills.rs:261,279`. A new `board` skill → add `("board.md", board::skill())` to that array + a `RegisterPrompt{plugin:"board"}` block in the owning crate.

### Gaps / notes for the board design
- No streaming path exists for a *file* artifact’s content; only `app` drafts stream source. A streamed board would follow the `app` pattern (`ToolCallProgress` → `partialApp`-style envelope repair → `AppDraft`-like store entry → panel auto-open) or be a reply-fence language (renderer in `plugins.renderers`, `isIncomplete` progressive parse like `JsonRenderFence`).
- No typed provenance on `send_input`; select-to-ask should reuse the text-header convention (`[app …]` / `[attached …]`) + `parse*` on the client, plus a static prompt block explaining it.
- Code excerpt pinning: `code_symbol.definition.{path,line_start,line_end}`; `body` is lossy past 200 lines.
- Notes have block ids, not `##` sections; `doc_read format=markdown` yields `<!-- id -->` anchors.

### 9. Addressable diagram units and the read-only code seam (second pass, 2026-09-26)
- **json-render `Graph` (desktop, own SVG + ELK):** every node `<g>` carries `data-node={n.id}` and, when `onSelect` is given, `role="button" tabIndex=0` with click and Enter/Space (`components/chat/surface/Graph.tsx:150-155`); `GraphSvg` takes `highlight?: readonly string[]` and paints a highlighted node `active` unless its own state is `danger`/`done` (`Graph.tsx:22-27`, 65 §3.5). The renderer is ours, so a shared anchor can drive `highlight` and read `data-node` today.
- **mermaid 12.0.0 `sequenceDiagram` (the installed build, `node_modules/mermaid/dist/chunks/mermaid.core/sequenceDiagram-*.mjs`):** a message arrow is `<line data-et="message" data-id="i<id>" data-from data-to>` (`:3630-3633`); notes and control structures use the same `data-id="i<id>"` (`:2960`, `:3430`); lifelines `data-et="life-line" data-id=<actor.name>` (`:2193`). `id` is the **ordinal in one shared `messages` array** (`this.state.records.messages.length.toString()`, `:1364, 1388, 1504, 1604`) — so a note or a `loop` shifts every later id. The label is a separate `<text class="messageText">` appended to the diagram root with **no id** (`:3578-3591`). Nothing in the mermaid grammar lets a statement carry an attribute, so a step cannot name its code from inside the diagram; from outside it is addressable only by position and only the arrow, not the label.
- **No read-only file/symbol RPC for the desktop.** `crates/rpc/src/api.rs` exposes `codeindex_stats(root)` (L841-842) and the document methods; nothing reads a file range or resolves a symbol for a renderer. Codeindex actor messages are `Stats` and `Reconcile` (`crates/codeindex/src/lib.rs:126, 133`); symbol resolution (`Definition{path, line_start, line_end, …}`, `store.rs:740-756`) is reachable only through the agent tool `code_symbol`. Font tokens today: `--font-sans` only (`crates/desktop/ui/src/styles.css:187`); no `--font-serif`, no bundled font files.
