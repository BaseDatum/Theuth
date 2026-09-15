# 29 — Rewind: edit a message, regenerate a reply, fork a conversation

Status: draft 2026-09-15 · extends `07-context.md` §8.8 (undo is the
primitive; this doc generalises its cut), `00-architecture.md` §5 (session
store), §7 (three RPC methods), `25-desktop.md` §3 (parity table: three
per-message buttons), `01-config-and-subagents.md` §2 (a fork is not a
child). Facts in §2 were re-read from the tree on 2026-09-15; line numbers
are from that read.

## 0. Decisions in one screen

1. **One server primitive, three verbs.** Edit and regenerate are the same
   operation: *move `cut_at` to a turn start, then start a turn*. Edit
   starts it with new text; regenerate replays the stored opener
   `ChatMessage` verbatim. Fork is a row copy into a new session and touches
   the source only to read. No message tree, no branch pointers, no new
   frame builder: the store shape of 07 §8 is enough.
2. **Turns are the unit, not messages.** A user message that opens a turn
   (`store::is_turn_start`) is the only addressable thing; a reply is
   addressed through the turn it belongs to. Steers and harness notes are
   not addressable. This is what keeps `tool_use`/`tool_result` pairs
   intact and `cut_at` always on a turn start (07 §8.8 invariant).
3. **Message identity on the wire is `turn: Option<i64>`** — the seq of
   the turn's opener — on `HistoryEntry::{User, Assistant}` and on
   `TurnStarted`. Nothing else changes shape. Rejected: per-message uuids
   (a new column and a migration for a handle sqlite already gives us).
4. **Edit and regenerate destroy the old tail**, exactly as a send after
   `/undo` does today (`Session::claim` → `Store::commit_cut`). Want to
   keep it? Fork first. Rejected: auto-fork on edit (ChatGPT-style
   branches) — a branch switcher UI and a tree store for a safety the fork
   button already gives.
5. **Clients truncate optimistically, no reload.** Click regenerate →
   the client drops the turn from its transcript → calls the RPC → the new
   turn streams in through the ordinary `TurnStarted…TurnFinished` events.
   On error the client reloads (`load_session`) to restore. Rejected:
   reload after the reply — races the stream and loses the first deltas
   in the TUI.
6. **Desktop: three 44 px lucide buttons** in the existing action-bar
   slot. Assistant bar: copy · regenerate (`RefreshCw`) · fork (`GitFork`).
   User bubble, hover-revealed: edit (`Pencil`) · fork. Edit is in place:
   the bubble becomes a textarea with Cancel / Send.
7. **TUI: `/edit [n]`, `/regen [n]`, `/fork [n]`.** No `n` → a picker of
   turns (`Menu::Turns`) with the newest selected; `n` is the 1-based turn
   ordinal the picker shows. `/edit` puts the opener's text in the editor
   via the existing `recall` hook; Enter sends, Esc cancels.
8. **Refused mid-turn** (edit, regenerate: `Err("turn in flight")`, the
   `Session::cut` guard). Fork is allowed while running: it copies stored
   rows; the in-flight round is not in the fork.

## 1. Why

Every harness the user compares us with can edit a sent message, retry a
reply, and branch a conversation. We have `/undo` + resend, which is the
same thing with worse ergonomics and no way to keep the old branch. The
store already has everything needed: seq-ordered rows, a cut marker that
the next write commits, and epochs keyed by `session_id` (07 §8). What is
missing is a handle for a turn on the wire and three thin verbs over the
existing cut.

## 2. Facts relied on (recon 2026-09-15)

### 2.1 Store and loop (`crates/agent`)

- Messages are genai `ChatMessage` rows, `messages(session_id, seq, body)`
  PK `(session_id, seq)`, seq = `MAX+1` per session (`Store::append`). No
  per-message id. `epochs(session_id, n, at_seq, tail_from, head, …)`;
  `sessions(…, parent, name, effort, agent, cwd, acp_session, cut_at)`.
- `Store::undo` (`store.rs:295`) sets `cut_at` to the seq of the last
  visible turn start; `redo` (:307) moves it forward; `commit_cut` (:319)
  deletes `messages.seq >= cut_at` and `epochs.at_seq >= cut_at` and
  clears the marker. Every reader filters `seq < COALESCE(cut_at, MAX)`.
- `is_turn_start(prev, m)` (`store.rs:546`): user role whose predecessor
  is nothing or an assistant message. `turn_starts(rows)` (:551) yields
  their indices. Steers (after Tool rows) and harness notes never count.
- `Session::claim` (`loop_.rs:319`) commits a pending cut before any write.
  `Session::cut` (:329) refuses with `"turn in flight"` when
  `inflight.is_some() || state.is_none()`, moves the cut, and rebuilds
  `SessionState` from `store.frame()`.
- `start_turn(inputs, me)` (:346): `claim` → `TurnId` → title on first
  turn → `InputInjected` → `TurnStarted { text, model }` → spawn
  `Turn::run`, which persists each input in `take_input` (:1007) via
  `user_message(Input { text, files })` (:1993) and `Turn::persist`.
  `UserInput { id, text, files }` (:61) is the actor message; `Input`
  (:71) is what one input carries. `Input { text, files }` is built in two
  places (:426, :438), both from `UserInput`.
- `HistoryEntry` (`lib.rs:662`): `User { text }`, `Assistant { text }`,
  `Tool { call_id, name, args, result, files }`, `Compacted { epoch,
  summary, style }`. `render()` (`supervisor.rs:2098`) produces
  `Vec<(i64, HistoryEntry)>`; `page()` (:1502) drops the seq. Only
  `Loaded.first_seq` (`lib.rs:252`) survives.
- `sessions.parent` means *subagent*: approvals inherit, `/parent`
  navigates, `DeleteSession`/`Cancel` cascade, `spawn_subagent` depth
  counts it. A fork must leave it `NULL`.
- `Agents` handlers for `Undo`/`Redo` (`supervisor.rs:761-787`) call
  `self.search.invalidate()`; any row mutation must too.
- `Session::new` sets `titled = !history.is_empty()`; `start_turn` titles
  an untitled session on its first turn, so every session with ≥ 1 turn
  has a title.

### 2.2 RPC (`crates/rpc`)

- `trait Api` (`api.rs`), one `ask` per method; `undo`/`redo` at
  `:131-135`, `load_session(session, before, limit)` at `:158`. New wire
  types need `schemars::JsonSchema` + `#[cfg_attr(feature = "ts",
  derive(ts_rs::TS), ts(export))]` and a line in `export.rs`'s list;
  `bindings/methods.json` is generated from the trait. `just ts` /
  `just ts-desktop` regenerate.

### 2.3 Desktop (`crates/desktop/ui`)

- `CopyButton` in `components/chat/MessageAssistant.tsx:19-41`: shadcn
  `Button variant="ghost" className="size-11 rounded-lg text-fg-secondary"`
  in a `Tooltip`, lucide `Copy`/`Check`, `navigator.clipboard.writeText`.
  Rendered always (not on hover) once streaming closed:
  `{!open && (<div className="-ml-2 flex"><CopyButton text={text} /></div>)}`
  (:57-61). `MessageUser.tsx`: right-aligned bubble, no actions.
- `Message.tsx` is `memo` over `{ entry, approvals }`; no `session` prop.
  `Transcript.tsx` renders `<Message entry={row.entry} approvals=… />`.
- Store: `store/sessions.ts` `SessionSlice.entries: Entry[]`, `Entry.id`
  is client-local (`reduce.ts:15 nextId()`); `fromHistory()` maps
  `HistoryEntry → Entry[]`; `apply()` reduces events; `loadHistory(id,
  loaded, 'replace')` is the `/undo` reload path (`slash.ts`,
  `ctx.reload()`). `markSent(id, text)` suppresses the `TurnStarted` echo
  of an own send.
- `rpc/api.ts`: hand-kept `Api` interface + sorted `METHODS`;
  `api.test.ts` diffs `METHODS` against `bindings/methods.json`.
- Sidebar: `SessionList.tsx` is a TanStack query on `queryKeys.sessions`;
  open = `navigate({ to: '/c/$session' })`; `routes/c.$session.tsx` loads
  on first visit. No `SessionCreated` event; the list is invalidated on
  `TurnFinished`/`TitleChanged`.
- Composer prefill precedent: `QueuedChips onEdit={(t) => setText(t)}`.
  Cross-cutting UI uses `wth:*` window events.

### 2.4 TUI (`crates/tui`)

- `COMMANDS` table (`model.rs:717`), string `match` in `Model::submit`
  (:2480). `/resume` is the end-to-end example: table row → `submit` arm →
  `Model::resume` (:2670) → `Command::Load` → `lib.rs execute` →
  `UiEvent::History` → `apply` (:1849) rebuilds `entries` from
  `loaded.history` through `from_history` (:3702).
- `Entry::User(String)` (:458): no seq. Rendered as the `◆ you` card
  (`ui.rs:814`). No message selection; `anchor`/`scroll` only.
- `Model::recall: Option<String>` (:1298) — "text the editor takes on the
  next render" — drained in `ui.rs:146`; set by `UiEvent::Withdrawn`.
  `Model::withdraw` (:2682) refuses when the editor is not empty.
- `/undo` in `lib.rs:188-191`: `api.undo` → `UiEvent::Undone`, then
  `load_session` → `UiEvent::History`, both from the `execute` arm because
  commands run concurrently. Text typed while `self.streaming` becomes
  `Command::Steer`.
- Pickers: `Menu::{Sessions, Search, Agents, …}` with `menu_key` (:3199).

## 3. Semantics

```mermaid
sequenceDiagram
  participant C as client
  participant A as Agents
  participant S as Session actor
  participant St as Store
  C->>C: drop entries from turn t (optimistic)
  C->>A: regenerate(session, t)  /  edit_turn(session, t, text)
  A->>S: RestartTurn { turn: t, input: None | Some(text, files) }
  S->>S: inflight? → Err("turn in flight")
  S->>St: cut_to(t): is_turn_start at t? set cut_at = t; return opener row
  S->>St: frame() → SessionState::resume
  S->>S: start_turn([Input { text, files, replay }])
  Note over S,St: claim → commit_cut deletes seq ≥ t; take_input appends the opener (seq t again)
  S-->>C: Ok
  S-->>C: TurnStarted { turn_seq: t, text } … TurnFinished
```

**Turn handle.** `turn: i64` is the seq of the user row that opens a turn.
The server validates it (`is_turn_start` at that seq among *visible* rows);
anything else is `Err("not a turn start")`. Clients never compute turn
boundaries.

**Edit** = cut at `turn`, start a turn with the new `text` (+ `files`).
Steers that belonged to the old turn are gone with it.

**Regenerate** = cut at `turn`, start a turn whose first input replays the
opener `ChatMessage` as stored — text, `Binary` parts, skill injection,
everything — so the model sees the same message and the frame stays a
strict prefix of a cached request up to the opener (07 §8.8 "Cache").

**Fork** = new session `id'` with the source's `title, model, effort, cwd`,
`parent = NULL`, `name = NULL`, plus copies of every visible message and
epoch row with `seq < end`, where `end` is the next turn start after
`through` (or `MAX` when `through` is `None` or the last turn). *Through*
means the clicked exchange is in the fork: fork on a user bubble or on its
reply both keep that whole turn. `through` must be a visible turn start.
The source is not modified.

**Refusals.** `Err("turn in flight")` for edit/regenerate while running.
`Err("cannot fork an external harness session")` when `sessions.agent` is
set (the transcript is a translation; the harness owns the state, 06).
`Err("nothing to fork")` on a session with no visible turn.

## 4. Server

### 4.1 Store (`crates/agent/src/store.rs`)

```rust
/// Hide `seq >= turn` (a visible turn start) and return the opener row.
/// `Err` when `turn` is not a visible turn start.
pub async fn cut_to(&self, id: SessionId, turn: i64) -> anyhow::Result<ChatMessage>;

/// New session with the source's title/model/effort/cwd and copies of the
/// visible rows and epochs before the turn after `through` (all when None).
/// Returns the new id. The source is only read.
pub async fn fork(&self, src: SessionId, through: Option<i64>) -> anyhow::Result<SessionId>;
```

- `cut_to`: `let visible = self.messages(id).await?; let i = visible.iter()
  .position(|(s, _)| *s == turn)`; require `is_turn_start(prev, m)`;
  `set_cut(id, Some(turn))`; return `visible[i].1.clone()`. `Store::undo`
  becomes `cut_to(last turn start)` if that reads cleaner when touched; not
  required.
- `fork`: one transaction. `end` from `turn_starts(&visible)`: the first
  start with seq `> through`, else `i64::MAX`. Then

  ```sql
  INSERT INTO sessions (id, created_at, updated_at, title, model, parent, name, effort, cwd)
    SELECT ?new, ?now, ?now, title, model, NULL, NULL, effort, cwd FROM sessions WHERE id = ?src;
  INSERT INTO messages (session_id, seq, body)
    SELECT ?new, seq, body FROM messages WHERE session_id = ?src AND seq < ?end
      AND seq < COALESCE((SELECT cut_at FROM sessions WHERE id = ?src), 9223372036854775807);
  INSERT INTO epochs (session_id, n, at_seq, tail_from, head, created_at)
    SELECT ?new, n, at_seq, tail_from, head, created_at FROM epochs WHERE session_id = ?src
      AND at_seq < ?end AND at_seq < COALESCE((SELECT cut_at …), …);
  ```

  `updated_at = now` puts the fork at the top of the sidebar. Epochs must
  come along or the fork replays raw pre-compaction rows and can overflow
  the context on its first turn (07 §8.4).

### 4.2 Session actor (`crates/agent/src/loop_.rs`)

```rust
/// `ask` → `Result<(), String>`: hide `seq >= turn`, then start a turn.
/// `input: None` replays the hidden opener verbatim (regenerate);
/// `Some` starts it with new text (edit). `Err("turn in flight" | "not a turn start")`.
pub struct RestartTurn { pub turn: i64, pub input: Option<(String, Vec<PathBuf>)> }

pub struct Input {
    pub text: String,
    pub files: Vec<PathBuf>,
    /// Persist this message instead of building one from `text`/`files`
    /// (regenerate). `text` is still its rendering, for `TurnStarted` and the title.
    pub replay: Option<ChatMessage>,
}
```

- Handler: the `cut()` guard; `let opener = store.cut_to(id, turn)?`;
  `state = SessionState::resume(store.frame()?)`; then
  `self.start_turn(vec![(None, input)], me)` with `input` either
  `Input { text, files, replay: None }` or
  `Input { text: text_of(&opener.content), files: vec![], replay: Some(opener) }`.
  `start_turn` runs `claim` → `commit_cut`, so the hidden tail is deleted
  before the opener is re-appended at the same seq.
- `take_input`: `let msg = input.replay.take().unwrap_or_else(|| user_message(input))`.
  The skill note (`skills::injected(&input.text)`) keeps working on the
  replayed text. The two `Input { text, files }` sites gain `replay: None`.
- `TurnStarted` gains `turn_seq: i64`. To know it, `start_turn` persists
  the first input before publishing. `Turn::run`'s prologue (`:1031-1039`:
  `input_text`, `title_chars`, the `take_input` loop) moves into
  `start_turn`: build the `Turn`, set `input_text`/`title_chars` from the
  whole list, `task.take_input(first)`, read `turn_seq` off
  `task.state.history.last()`, publish `TurnStarted { …, turn_seq }`, then
  spawn `task.run(turn_id, rest)` which only loops `take_input` over the
  rest before `rebase`. One sqlite write on the actor task, next to the one
  `claim` already does. Observable change: the opener row exists when
  `TurnStarted` is delivered; nothing else moves. Rejected: `turn_seq` on
  `TurnFinished` — clients would have to remember the turn's start index and
  stamp backwards, and the error/cancel paths would need the same care.

### 4.3 Supervisor and wire (`lib.rs`, `supervisor.rs`)

```rust
/// `ask` → `Result<(), String>`. Regenerate the reply to the turn opened at `turn`.
pub struct Regenerate { pub session: SessionId, pub turn: i64 }
/// `ask` → `Result<(), String>`. Replace the turn opened at `turn` with `text`.
pub struct EditTurn { pub session: SessionId, pub turn: i64, pub text: String, pub files: Vec<PathBuf> }
/// `ask` → `Result<SessionId, String>`. New session with the conversation through `through`.
pub struct ForkSession { pub session: SessionId, pub through: Option<i64> }
```

- `Regenerate`/`EditTurn`: `check_files` (as `Agents::input` does),
  `self.session(Some(id), …)` (resume-or-spawn, the `SendInput` path), then
  `ask(RestartTurn { .. })`. `self.search.invalidate()`.
- `ForkSession`: refuse when `store.session(id)?.agent.is_some()`;
  `store.fork(id, through)`; `search.invalidate()`; return the id. The new
  session is resumed lazily by `Agents::session` on its first input, like
  any stored session. No new event: the caller has the id and opens it.
- `HistoryEntry::User { text, turn: Option<i64> }` — `Some(seq)` when the
  row opens a turn, `None` for steers and harness notes (so clients hide
  edit/fork on them without knowing the rule). `HistoryEntry::Assistant {
  text, turn: Option<i64> }` — the opener seq of the turn it belongs to,
  always `Some` for stored rows. Both `#[serde(default)]`,
  `ts(type = "number | null")` like `first_seq`. `render()` computes it
  with one pass over `turn_starts(messages)` (it already walks rows in
  order). `Tool` and `Compacted` unchanged: no button lives on them.
- `AgentEvent::TurnStarted { …, turn_seq: i64 }`.

### 4.4 RPC (`crates/rpc/src/api.rs`)

```rust
#[method(name = "regenerate")]
async fn regenerate(&self, session: SessionId, turn: i64) -> RpcResult<()>;
#[method(name = "edit_turn")]
async fn edit_turn(&self, session: SessionId, turn: i64, text: String, files: Vec<PathBuf>) -> RpcResult<()>;
#[method(name = "fork_session")]
async fn fork_session(&self, session: SessionId, through: Option<i64>) -> RpcResult<SessionId>;
```

Three `ask`s, 80 methods. `just ts` regenerates `HistoryEntry.ts`,
`AgentEvent.ts`, `methods.json`. The line printer (`tui/src/lib.rs:636`)
and `page_walks_back_by_seq` match `HistoryEntry::User { text, .. }`.

## 5. Desktop

### 5.1 Identity in the store

- `Entry` (`store/sessions.ts`): user and assistant kinds gain
  `turn?: number`. `fromHistory` copies `HistoryEntry.turn`. `apply()`:
  `TurnStarted` stores `slice.turnSeq = e.turn_seq` and stamps the echo
  user entry (or, when `markSent` suppressed the echo, the last user
  entry); every assistant entry created while the turn runs gets
  `turn: slice.turnSeq`. `TurnFinished` clears nothing — the stamp is the
  point.
- `truncate(session, turn)` on the store: `entries.slice(0,
  entries.findIndex(e => e.turn === turn))`. Used by regenerate and edit
  before the RPC; the error path calls the existing `reload()`.

### 5.2 Buttons

- `Message` gains `session: SessionId`; `Transcript` passes it (stable
  string, `memo` unaffected). `running = useSessionField(session,
  'running')`; every button below renders only when `entry.turn != null
  && !running`.
- `MessageAssistant` action bar (the `-ml-2 flex` div): `CopyButton`,
  then `ActionButton icon={RefreshCw} label="Regenerate"` →
  `truncate(session, turn); api.regenerate(session, turn)`, then
  `ActionButton icon={GitFork} label="Fork from here"` → §5.3.
  `ActionButton` is `CopyButton` with the click and icon lifted out (same
  classes, same `Tooltip`); `CopyButton` becomes one call of it.
- `MessageUser`: wrap in `group`; below the bubble a right-aligned row
  `flex justify-end -mr-2 opacity-0 group-hover:opacity-100
  focus-within:opacity-100` with `Pencil` (Edit) and `GitFork` (Fork).
  Opacity, not `hidden`, so the virtualizer's measured height is stable.
  Only user entries with `turn` (openers) get the row; steers never do.
- Edit in place: `editing` state in `MessageUser`; the bubble renders a
  shadcn `Textarea` (autofocus, `whitespace-pre-wrap`, same radius) with
  `Cancel` / `Send` buttons beneath. Enter sends, Shift+Enter newline, Esc
  cancels. Send: `truncate(session, turn); api.edit_turn(session, turn,
  text, [])`. Errors: `toast.error` + `reload()`, the codebase's
  convention.

### 5.3 Fork navigation

`const id = await api.fork_session(session, turn)` →
`qc.invalidateQueries({ queryKey: queryKeys.sessions })` →
`navigate({ to: '/c/$session', params: { session: id } })`. The route's
first-visit `load_session` renders the copy. Toast "forked" is unnecessary:
the URL and the transcript say it.

### 5.4 Slash popup (`slash.ts`)

`/regen` and `/fork` with no argument act on the last turn (the last entry
with `turn`), mirroring the TUI defaults so muscle memory carries (25 §0.8).
`/edit` is not in the desktop popup: the bubble's button is the surface.

## 6. TUI

### 6.1 Entries and identity

`Entry::User(String)` → `Entry::User { text: String, turn: Option<i64> }`.
`from_history` copies `HistoryEntry.turn`. `apply_agent(TurnStarted)`
stamps the echo (or the entry pushed at submit) with `turn_seq`. The
picker and the ordinals read this field only. Mechanical: ~20 sites,
mostly tests.

### 6.2 Commands

`COMMANDS` rows, placed after `/redo`:

```rust
("/edit",  "[n]", "edit turn n's message and regenerate from it (picker without n)"),
("/regen", "[n]", "regenerate the reply to turn n (picker without n)"),
("/fork",  "[n]", "new session with the conversation through turn n (picker without n)"),
```

`n` is the 1-based ordinal of `Entry::User { turn: Some(_) }` in the
current transcript, top to bottom — the number the picker shows. The TUI
loads whole histories (`load_session(id, None, None)`), so ordinals are
complete.

```rust
enum TurnAction { Edit, Regen, Fork }
Menu::Turns { cursor: usize, action: TurnAction }   // rows: "#n  ◆ <first 60 chars of the opener>", newest last, cursor on newest
pub editing: Option<(usize, i64)>,                   // entry index and turn seq of the message being edited
```

`submit` arms: parse `n` → `(idx, turn)`; missing → open the picker;
`self.streaming && action != Fork` → `Entry::Error("turn in flight — /cancel first")`.
Enter in the picker resolves to the same `(idx, turn)` and falls through to:

- **Regen**: `entries.truncate(idx)`; `send(tx, Command::Regenerate { session, turn })`.
- **Edit**: `withdraw`'s `editor_empty` guard; `self.editing = Some((idx,
  turn))`; `self.recall = Some(text)`; notice `editing #n — ⏎ sends, Esc
  cancels`. Then in `submit`, when `editing` is `Some` and the line is not
  a slash command: `entries.truncate(idx)`; push `Entry::User { text,
  turn: None }`; `self.sent = Some(text)`; `send(tx, Command::EditTurn {
  session, turn, text })`; `editing = None`. Esc with `editing` set clears
  it and the editor (before the existing `/cancel` binding).
- **Fork**: `send(tx, Command::Fork { session, through: Some(turn) })`.

`execute` arms (`lib.rs`), following the `/undo` shape:

```rust
Command::Regenerate { session, turn } => match api.regenerate(session, turn).await {
    Ok(()) => return Ok(()),                     // the stream repaints the turn
    Err(e) => { let _ = tx.send(UiEvent::Error(e.to_string()));
                UiEvent::History(session, msg(api.load_session(session, None, None).await)?) }
}
Command::EditTurn { .. }  => same shape over api.edit_turn(session, turn, text, vec![])
Command::Fork { session, through } => {
    let id = msg(api.fork_session(session, through).await)?;
    UiEvent::Forked(id, msg(api.load_session(id, None, None).await)?)
}
```

`apply(UiEvent::Forked(id, loaded))`: `reset()`, `session = Some(id)`,
entries from `loaded` (the `History` arm's body), notice `forked — /rename
to tell them apart`. Pastes and images in the editor are not part of edit
in v0 (text only, like the desktop).

## 7. Docs amended in the same commit (12 §0 rule 3)

- `00-architecture.md` §5, the append-only bullet (line 268): *…and
  compaction opens a new one. `/undo`, edit and regenerate move `cut_at`
  to a turn start; the next write commits it (07 §8.8, 29). Fork copies
  the visible rows and epochs of a session into a new one (29 §3): no
  message tree — rejected because a tree store plus a branch switcher buys
  nothing the fork does not.* §7: 80 methods. §12: a phase-18 row, check
  from §9 below.
- `07-context.md` §8.8: `Store::cut_to` as the general form of `undo`;
  turn start is the only legal cut position, now used by three callers.
- `25-desktop.md` §3: three rows (edit, regenerate, fork) and strike
  "fork" from §2.1's "No RPC" list.
- `01-config-and-subagents.md` §2: one sentence — a fork has
  `parent = NULL`; it is a top-level session even when the source was a
  child.
- README: the three slash commands.

## 8. Not doing, and what each rejection beat

- **Message tree / branch switcher** (ChatGPT `< 2/3 >`). A `parent_seq`
  column, a frame builder that walks a path, a UI to switch branches, and
  every reader of `messages` learning about paths. Fork-to-new-session
  gives the same safety with rows we already store and the sidebar we
  already have.
- **Per-message uuids.** A migration and a column for a handle the seq
  already is. `turn: Option<i64>` on two `HistoryEntry` variants is the
  whole wire change.
- **`forked_from` column / breadcrumb.** Not asked for. The sidebar
  shows two sessions with the same title; `/rename` exists. Add the
  column when someone needs "where did this come from" — it is one
  migration and one `Loaded` field.
- **Edit carrying the original attachments.** `HistoryEntry::User` has no
  `files`; the in-place editor sends text only. When the desktop renders
  user attachments in the bubble, `files` lands on `HistoryEntry::User`
  and the editor re-sends them unchanged.
- **Regenerate with a different model.** `/model` then `/regen` is two
  keystrokes; a model picker on the button is a second model menu.
- **Retitle after editing turn 1.** `titled` stays true; the old title
  stands. `/rename`.
- **`SessionCreated` event.** The forking client has the id; other clients
  see the fork on their next `list_sessions` (the desktop invalidates on
  `TurnFinished`, which the fork's first turn emits).
- **Regenerate keeping the turn's steers.** They are part of the tail the
  user is replacing; replaying them would re-steer a reply that no longer
  exists.

## 9. Checks (headless RPC on a fresh data dir, then both UIs)

1. `send_input` "say A", then "say B". `load_session` → both `User`
   entries carry `turn`, both `Assistant` entries carry the opener's seq;
   `TurnStarted.turn_seq` on the event stream equals the stored `turn`.
2. `regenerate(session, turn_of_B)` → `TurnStarted { text: "say B",
   turn_seq: <same seq> }`, a new reply; `load_session` has exactly two
   turns; `undone == 0`; the sqlite `messages` count is unchanged.
3. `regenerate` with an assistant seq or a steer seq → `-32000 "not a turn
   start"`. `regenerate` while a turn streams → `"turn in flight"`.
4. `edit_turn(session, turn_of_A, "say C")` → the transcript is `C, reply,`
   nothing else (B's turn went with the tail); `redo` → `"nothing to redo"`.
5. `fork_session(session, Some(turn_of_A))` on a session with turns A, B →
   new id, `parent: null`, same title/model; `load_session(new)` shows
   turn A only; the source is byte-identical (`SELECT count(*), max(seq)`).
   `fork_session(session, None)` copies both. Fork a compacted session →
   the `epochs` row is present in the copy and its first turn's request is
   the compaction head + tail, not the raw history (check the `context`
   in `TurnFinished`).
6. `fork_session` on an ACP child → `"cannot fork an external harness
   session"`.
7. Desktop: hover a user bubble → Edit · Fork appear; Edit swaps the bubble
   for a textarea; Send removes the old turn instantly and the new reply
   streams under the edited text; Regenerate under a reply does the same
   with the original text; Fork opens `/c/<new>` with the sidebar row on
   top. Buttons absent while the pulsing dot shows.
8. TUI: `/regen` → picker with the newest turn selected → Enter → the
   last exchange is redrawn live; `/edit 1` → the first message is in the
   editor, status shows `editing #1`, Esc restores an empty editor, Enter
   after a change rewrites the transcript from #1; `/fork` → new session
   id in the header, `/sessions` lists both with the same title; `-p`
   line printer prints the `>` lines with no `turn` noise.

## 10. Order and size

| Step | Crates | Size | Notes |
|---|---|---|---|
| 1 store: `cut_to`, `fork` | agent | ~70 | unit tests beside `undo`'s: cut refuses a steer seq; fork copies epochs and stops at `end` |
| 2 loop: `RestartTurn`, `Input.replay`, `TurnStarted.turn_seq`, first input persisted in `start_turn` | agent | ~60 | |
| 3 wire: three commands, `HistoryEntry.turn`, three `Api` methods, bindings | agent, rpc | ~80 | `just ts` |
| 4 desktop | desktop/ui | ~200 | `ActionButton` refactor, `MessageUser` edit mode, `truncate`, `turn` stamping, `Api`/`METHODS` |
| 5 TUI | tui | ~180 | `Entry::User` struct, `Menu::Turns`, `editing`, three commands and arms |
| 6 docs | 00, 07, 25, 01, README | text | same commit as 3 |

1 → 2 → 3 serial (each needs the previous shape). 4 and 5 in parallel once
3 is green, against the §4.3/§4.4 contract; neither touches `agent`.
`cargo check` green per step; the §9 checks run once at the end.
