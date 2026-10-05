# 101 — Decision & review surface (DECISION-SURFACE)

Status: decided 2026-10-05 22:47 (Sky's three answers below); building on main `c75e9b58`.
Workers: `feat/101-eng` (crates/tasks, crates/roster, crates/rpc) and `feat/101-ui` (crates/desktop/ui). Contract in §3 is binding for both.

## 1. The ask (Sky, 2026-10-05 22:24)

> We need dedicated UI for the decisions and review in the mobile and desktop app that agents surface to the
> attention interface. Right now it just opens up the task card, which is like information overload right? I need
> the agents to be able to prepare a summary of what the decision is about, maybe add a diagram, and then present
> that in a special UI for decisions that I need to make and for them to be able to point me to exactly what I need
> to review.

Sky's answers (ask_user, 22:47): **Master–detail** layout · **Inline evidence from the agent** for review pointers ·
**Required when you are the audience** (the tool refuses an escalation to the user without a summary, and a hand-over
to the user's review without a summary and at least one pointer).

## 2. Today (verified on main c75e9b58)

- `escalate(task, question, options)` (`crates/tasks/src/tools.rs:1054`) writes the question + options as markdown
  into the thread (`ledger.rs:1445 escalate`), pushes the `decision:<id>` blocker, wakes the manager; when the manager
  is the user it also raises an `AskRequested` card on the *calling* session (`crates/tasks/src/lib.rs:641 ask_user`).
  Nothing structured is stored: no summary, no diagram, no pointers.
- `attention_list` (`crates/roster/src/attention.rs:195–245`) derives one item per blocked-on-decision task with
  `detail = "the question is in the task thread"`, and one `review:<task>` item per task in review with the user as
  reviewer (`detail = "by <owner>"`).
- The desktop inbox (`crates/desktop/ui/src/components/hud/Hud.tsx:197 AttentionCard`) renders the decision as a
  free-text box ("Your decision…") and the review as *Move done / Send back*; the title links to `/tasks/$id`, the
  full task card (`components/tasks/TaskDetail.tsx`: brief, thread, events, record, relations, placement, sessions,
  cost, actions). `/attention` (`components/hud/AttentionView.tsx`) is that same list on its own page.
- The mobile client is the same UI (Tauri iOS); the relay replays no `AskRequested` (facts/wth-ask-user-relay-no-replay),
  so a durable attention item is the only vehicle that reliably reaches a phone.

## 3. Contract — the packet

A **packet** is what an agent prepares for the person who must decide or review. One per decision (`decision:<id>`)
and at most one per task in review (`review:<task>`, the latest replaces). Stored in `tasks.sqlite` and replicated like
tasks. Rust lives in a new `crates/tasks/src/packet.rs`; every type derives `ts_rs::TS` under the `ts` feature so the
desktop gets `src/rpc/bindings/{Packet,PacketKind,PacketOption,Pointer,PointerKind,Check}.ts`.

```rust
#[serde(rename_all = "snake_case")] pub enum PacketKind { Decision, Review }
#[serde(rename_all = "snake_case")] pub enum PointerKind { Artifact, Note, Knowledge, Session, Meeting, Task, Url, Code, Diff }

pub struct PacketOption { pub label: String, pub description: Option<String> }
pub struct Check { pub text: String, pub done: bool, pub note: Option<String> }
pub struct Pointer {
    pub kind: PointerKind,
    /// artifact call_id | note DocId | knowledge id | session id | meeting id | task id | https URL |
    /// code `path#L10-L24` (path relative to `root`) | diff `<base>..<head>` or a free label
    #[serde(rename = "ref")] pub ref_: String,
    pub title: String,
    /// One line: what to look for here.
    pub note: Option<String>,
    /// Inline evidence (Sky's choice): code = the lines verbatim; diff = a unified diff. ≤ 32 KiB. Required for code/diff.
    pub text: Option<String>,
    /// code/diff: absolute repo root on the engine that produced it.
    pub root: Option<String>,
}
pub struct Packet {
    pub key: String,              // `decision:<uuid>` | `review:<task>`
    pub task: TaskId,
    pub kind: PacketKind,
    pub by: String,               // actor: `agent:<name>` | `user`
    #[ts(type = "number")] pub at_ms: u64,
    pub question: Option<String>, // decision: the question as asked
    pub summary: String,          // markdown; ```mermaid / ```json-render / ```openui fences allowed
    pub options: Vec<PacketOption>,   // decision only; may be empty (free-form question)
    pub recommended: Option<u32>,     // index into options
    pub pointers: Vec<Pointer>,
    pub checks: Vec<Check>,           // review: the agent's claimed status against done_when
}
```

TS as ts-rs emits it (the UI worker stubs exactly this until integration regenerates the bindings):

```ts
export type PacketKind = "decision" | "review";
export type PointerKind = "artifact" | "note" | "knowledge" | "session" | "meeting" | "task" | "url" | "code" | "diff";
export type PacketOption = { label: string, description: string | null, };
export type Check = { text: string, done: boolean, note: string | null, };
export type Pointer = { kind: PointerKind, ref: string, title: string, note: string | null, text: string | null, root: string | null, };
export type Packet = { key: string, task: TaskId, kind: PacketKind, by: string, at_ms: number, question: string | null, summary: string, options: Array<PacketOption>, recommended: number | null, pointers: Array<Pointer>, checks: Array<Check>, };
```

Caps (refused by the tools with an error that names the field and the cap): `summary` ≤ 16 KiB; `pointers` ≤ 12,
each `text` ≤ 32 KiB; `options` ≤ 6, label ≤ 80 chars; whole packet JSON ≤ 128 KiB. `url` refs must start with
`http://`/`https://`; `code` refs must parse as `path#L<a>-L<b>` (or `path#L<a>`); `code`/`diff` need `text`.

`AttentionItem` (`crates/roster/src/attention.rs:58`) gains **`pub packet: Option<String>`** — the packet key when
one exists — and its `detail` becomes the first line of the question (decision) or of the summary (review), ≤ 140
chars; today's strings stay as the fallback when no packet exists (items raised before this build).

RPC (`crates/rpc/src/api.rs`, beside `attention_list` :1713): **`packet_get(key: String) -> Option<Packet>`**.
Desktop `api.ts`: `packet_get(key: string): Promise<Packet | null>` plus the entry in the method list.

## 4. Decisions

- **D1 Audience rule (Sky).** `escalate` resolving to the user requires `summary`; `task_update {disposition: review}`
  and `task_move {state: review}` whose effective reviewer is the user require `summary` and ≥ 1 `pointer`. Agent
  audiences: optional, stored when given. One validator in `packet.rs` serves all three tools; the error text tells
  the agent what to add ("escalating to the user needs a `summary`: two to five short paragraphs on what the decision
  is about, a ```mermaid or ```json-render fence when structure helps, and `options` with one-line descriptions").
- **D2 Options keep their shape.** `escalate.options` accepts both `"label"` strings and `{label, description}`
  objects (`#[serde(untagged)]`), so desks mid-flight keep working. New: `summary`, `recommended`, `pointers`.
- **D3 The thread stays the record.** The packet is also written into the thread as markdown (summary, options,
  "Look at:" list of pointer titles + refs, checks) — the `**decision requested**` block grows; a review hand-over
  writes a `**review requested**` block. Agents and the task card read the thread as before.
- **D4 The ask card stays** for channels (Slack/Signal/Telegram render `AskRequested`); it now carries the option
  descriptions. The desktop/mobile primary is the attention surface. Both paths resolve through `answer_decision`;
  the second answer is a no-op error as today.
- **D5 Replication.** `packets` is a replicated table (`replica::attach` list at `crates/tasks/src/lib.rs:1068`),
  so a packet raised on a VM engine reaches the laptop and the phone through the engine they talk to.
- **D6 Master–detail (Sky).** Desktop ≥ md: `/attention` = a 320 px rail (the inbox rows) + the selected item's
  screen; at `/attention` the newest item shows in the pane without a redirect, a row click navigates to
  `/attention/$id`. Phones: `/attention` is the list, `/attention/$id` the full-screen screen with a back chevron.
  Same route layout as Artifacts (`routes/artifacts.tsx` + `artifacts.$callId.tsx`, `components/artifacts/Explorer.tsx`).
- **D7 Every kind gets a screen.** decision and review get the rich screens below; approval, hire, blocked, budget,
  node, handoff, secret get a plain screen: title, detail, the same buttons the row has today (extracted from
  `AttentionCard` so the HUD home keeps its inline card unchanged).
- **D8 Decision screen.** Kind chip · who asks (agent) · task title (small link) · age → the question (large) →
  summary (streamdown with the chat's renderers: mermaid, json-render, openui; `MessageAssistant.tsx:27–38` exports
  `plugins`/`markdownComponents`/`shikiTheme`) → "Look at" pointers → options as option cards (the `AskCard` look:
  radio glyph, label, description, *recommended* badge) + "Type your own…" → one **Answer** button (sends the
  picked label or the typed text through `task_decide`; disabled until there is one) → footer: Open task · Snooze ·
  Dismiss. A pick does not answer by itself (mis-tap protection on phones); there is no confirmation dialog after Answer.
- **D9 Review screen.** Kind chip · task title · by whom · age → summary → **Acceptance**: `done_when` text + the
  agent's `checks` as a read-only checklist → **Review these**: pointers inline → sticky action bar: **Approve → done**
  (`task_move(id, 'done', note | null)`) and **Send back** (a required note, `task_move(id, 'doing', note)`) · Open
  task · Snooze · Dismiss.
- **D10 Pointer rendering (Sky: inline evidence).** `code` → `components/chat/surface/Excerpt` (path/start from the
  ref, `text` verbatim, `note` as caption); `diff` → new `UnifiedDiff` (parse `diff --git` / `+++` / `@@` into files
  and hunks; +/− colouring; per-file collapse with +N −M counts; monospace 12 px; sideways scroll, never mid-word
  breaks); `artifact` → inline preview with `components/artifacts/Viewer.tsx:281 ArtifactView` + Open →
  `/artifacts/$callId`; `note`/`knowledge`/`session`/`meeting`/`task` → a row (icon · title · note) linking to its
  route; `url` → a row with the host, opened the way chat links open externally.
- **D11 Resolution flow.** After Answer / Approve / Send back / Dismiss the item leaves the list: on desktop the
  pane moves to the next newest (or the empty state), on phones the screen goes back to the list. The list refetches
  on `ActivityAppended` as today; `packet_get` is cached per key and invalidated with the inbox.
- **D12 Nav.** `lib/nav.ts sectionOf` lights Attention for `/attention/…` (prefix at a `/` boundary) — test it.
  The HUD home's inbox rows link decision/review titles to `/attention/$id` instead of the task.
- **D13 Prompts.** The desk manual (`crates/roster/src/desk.rs:51 manual`, step 2 and 4) and the two tool
  descriptions say what a packet is and when it is required; theuth's own prompt gets no new text (theuth is the top
  of the line and never escalates).

## 5. Out of scope (follow-ups, not built here)

Channel rendering of the summary/pointers (Slack/Telegram get the ask card as before); live engine readers of files
or diffs (Sky chose inline evidence); approvals with a why; editing a packet after it is raised; a reviewer who is an
agent reading the packet as structured data (it reads the thread).

## 6. Gates

ENG: `cargo test -p tasks -p roster -p rpc --no-fail-fast`, `cargo check --workspace`, rustfmt on touched files
(never `cargo fmt --all`), `just ts` regenerates `bindings/` cleanly. UI: `just ts-desktop` then `bunx vite build`
(regenerates `routeTree.gen.ts`), `bun run check` (tsc), `CI=1 bun run test`. Integration tree: all of the above
plus a vite-dev screenshot pass of `/attention` and `/attention/$id` on a 1280 px and a 390 px viewport.
