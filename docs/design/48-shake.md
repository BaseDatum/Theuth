# 48 — Shake: explicit removal from context

Status: implemented · proposed and built 2026-09-18 · as-built reference.
Extends [07](07-context.md), [11](11-compaction-styles.md), [17](17-images.md),
[20](20-documents.md), [25](25-desktop.md), [29](29-rewind.md), and [31](31-link.md).
Source: `crates/agent/migrations/0010_context.sql`,
`crates/agent/src/{context,store,loop_,supervisor,lib}.rs`, `crates/rpc/src/api.rs`,
`crates/tui/src/{model,lib,ui}.rs`, `crates/desktop/ui/src/components/chat/{ContextPicker,
Attachments,Message,MessageAssistant,SessionMenu}.tsx`, `slash.ts`,
`store/{sessions,reduce}.ts`, `routes/c.$session.tsx`. Type definitions in
source override any sketch here. §7 records what was verified and what was
not; §9 lists where the build differs from the original proposal.

## 0. Decisions

1. **One operation, two entrances.** `/shake` in the TUI and desktop/mobile
   composer, and small remove buttons in the desktop/mobile transcript, call
   the same agent-owned context-removal command.
2. **Remove selected content, keep later conversation.** This is neither
   `/undo`, regeneration, `/clear`, nor automatic compaction. No model call,
   summary generation, knowledge harvest, or automatic continuation occurs.
3. **Explicit user edits may invalidate the prefix cache.** Log a named
   `context_edit` boundary; normal turns remain append-only between explicit
   edits, rebases, and compactions. Do not change the cache key or add random
   cache-busting data. Changed request bytes already express the edit.
4. **Keep the original transcript, change its context projection.** Persist
   small exclusion masks beside existing messages and epoch heads. Removed
   content is absent from subsequent conversation requests, not merely hidden
   with CSS. A collapsed archive marker explains why the transcript changed.
5. **Target occurrences, not files.** Removing one image does not remove a
   second occurrence of the same path or bytes. Binary content can be a user
   attachment, nested tool-result image, native PDF, or snap-head image.
6. **Delete whole messages honestly.** A whole-message removal includes any
   tool-call/result message group required for a valid request. Preview the
   additional messages. Do not quietly substitute “hide the visible text”
   while retaining its reasoning, arguments, or image payloads.
7. **Idle native sessions only.** The session actor, not a frontend's running
   flag, owns admission. Native roots and children work; translated external
   ACP sessions refuse because theuth does not own their actual context.
8. **Desktop and mobile are one UI.** Reuse the shared React transcript,
   shadcn controls, lucide icons, RPC client, and per-harness store. TUI uses
   its existing command/menu flow; no terminal image renderer is needed.

## 1. Why this fits theuth

[00 §1](00-architecture.md#1-non-negotiables) defines a general assistant,
not a coding-agent product: functional names, library-first glue, typed
plugin-owned contracts, no god-wiring, no new crate without a present need.
The owner here is already `agent`; context removal earns no plugin, service,
background sweeper, dependency, or configurable policy engine.

[07 §0–2](07-context.md#0-the-invariant) rejects automatic in-place pruning
because it repeatedly rewrites an expensive prefix behind the user's back.
The user deliberately removing a screenshot is different. This feature
adds **user-authorized `context_edit`** to the named exceptions; it does not
relax the normal append-only rule or expose a model-callable pruning tool.
Exclusion metadata changes only on explicit commands. Project it once when
loading/changing context, not by rescanning SQLite before every model call.

[25 §0](25-desktop.md#0-decisions-in-one-screen) makes an `Api` method the
shared command surface and requires explicit TUI/desktop parity. [31](31-link.md)
keeps that same client behind local and encrypted remote transports. Neither
a Tauri IPC shortcut nor a mobile-specific delete implementation belongs here.

[29](29-rewind.md) already distinguishes the durable transcript from the
active suffix. Reuse that distinction, its native-session gate, and its
session actor. Do **not** reuse its suffix cut: deleting an image in turn 2
must not discard turns 3–20.

### Pre-implementation observations

The tree as it stood before the build; each row motivated a design rule.
The implication column is now satisfied by the code named in §3–§5.

| Observation | Source and implication |
|---|---|
| `messages` stores whole `ChatMessage` JSON under `(session_id, seq)`. `Store::frame` returns the latest visible epoch head plus rows from `tail_from`. | `crates/agent/src/store.rs`, `append`, `messages_from`, `epochs`, `frame`. Selective removal needs a durable projection of both head and tail. |
| `undo`/`redo` move `cut_at`; `commit_cut` deletes the hidden suffix. Seq and epoch numbers can subsequently be reused. | Same file, `undo`, `redo`, `cut_to`, `commit_cut`. A bare seq is not a stale-client-safe mutation handle. |
| Turn boundaries are inferred from original role adjacency. | Same file, `is_turn_start`, `turn_starts`. Keep the original rows for rewind identity; do not infer turns from filtered context. |
| A running task owns `SessionState`; `state` is absent until `TurnDone`. Manual compaction/harvest also claim it. | `crates/agent/src/loop_.rs`, `Session::cut`, `claim`, `TurnDone`. `TurnFinished` or `Agents.running` alone does not prove idle. |
| `Frame::request` consumes already assembled head/history and then applies provider options/text-only stripping. | `crates/agent/src/frame.rs`. Removal must precede provider adaptation, estimates, compaction, and harvest. |
| User files become `ContentPart::Binary`; tool images become `ToolResponse.images`. An assistant row can contain prose, signed reasoning, and several calls. | `loop_.rs`, `user_message`, `Turn::round`; `src/chat/{content_part/common.rs,tool/tool_response.rs}` in the `skymoore/rust-genai` fork pinned by `Cargo.lock`. UI rows are not protocol messages. |
| A summary is an independent persisted message; the summarizer sees the entire current tail, including retained rows. Snap heads contain generated PNGs. | `loop_.rs`, `compact`, `summarise`, `continuation`; `snap.rs`. Removing a source row cannot surgically erase its meaning from a summary. |
| `HistoryEntry` has turn handles for user/assistant text, bare file paths on tools, no per-image identities. Live user files are only known to the sending desktop. | `agent/src/lib.rs`, `HistoryEntry`; `supervisor.rs`, `render`; `desktop/ui/src/store/{sessions,reduce}.ts`. Add authoritative attachment metadata, not a button on optimistic-only data. |
| React `Entry.id` is process-local. Several displayed tool entries share one stored response row; injected user rows may become several cards. | `desktop/ui/src/store/reduce.ts`, `nextId`, `fromHistory`; `supervisor.rs`, `render`; `desktop/ui/src/lib/injected.ts`. Never use display indexes, paths, or React keys as deletion targets. |
| Both command tables lack `/shake`; desktop/mobile share `slash.ts` and message components. | `tui/src/model.rs`; `desktop/ui/src/components/chat/`. Extend both existing dispatchers. |
| History reloads are currently unversioned, and old context-meter values can survive replacement. | `desktop/ui/src/store/sessions.ts`, `loadHistory`; `routes/c.$session.tsx`. A post-edit reload requires freshness checks and new estimate metadata. |

## 2. User contract

### 2.1 Slash commands

| Input | Behavior |
|---|---|
| `/shake` | Open the context picker; nothing selected. |
| `/shake images` | Open review with all image occurrences in the active head/tail selected. Includes user images, screenshots in tool responses, and snap-head PNGs; excludes PDFs. |
| `/shake documents` | Open review with active native PDF parts selected. Extracted PDF/Office Markdown is ordinary text, not a removable binary attachment. |
| `/shake messages` | Open the picker filtered to messages; nothing selected. User selects exact messages, not an implicit delete-all. |
| `/shake tools` | Open review with active tool exchanges selected, including their protocol-required message groups. |
| `/shake summary` | Select the current epoch head as a whole, retaining its existing tail boundary. Review warns that summarized earlier context will be lost. |

All paths end with one **Remove from context** confirmation. Escape/Cancel
makes no change. The bare command never removes everything. Zero matches
shows “No images in current context” (or the selected category), does not
write a revision, and makes no model call.

These are literal categories, not natural-language instructions to an LLM.
Unknown arguments show usage and do not fall through to a skill or ordinary
chat input. Do not implement a selector language, regexes, age thresholds,
“keep latest N”, or a background pruning preference. The picker covers exact
selection; additional shorthand should follow a demonstrated need.

The inventory is server-derived across the **whole active context**, not
just the most recent 60 displayed entries. It includes harness-injected
messages hidden by the ordinary transcript presentation. List role/source,
a short text preview or attachment name/MIME, and whether removal includes
other messages. No base64 bodies in inventory replies.

The review identifies snap images explicitly: “Summary image: removing this
loses part of the earlier conversation.” `/shake images` does not secretly
keep them. Display counts distinguish attachments, messages, and protocol
companions; a selected whole message subsumes its selected attachments.

### 2.2 What “removed” means

After success:

- The selected occurrence/message is not in the effective head/tail supplied
  to the next conversation call, compaction, or knowledge harvest.
- Unselected text and image parts are unchanged. Later messages remain in
  order, except protocol companions explicitly included in review.
- Session id/title/settings, current todo/runtime state, files, children,
  jobs, approvals, and other sessions are not deleted or canceled.
- New input can add a new occurrence later. This is a one-time edit, not a
  persistent ban on images, content, or particular file paths.
- Removed messages/attachments become collapsed **Removed from context**
  markers in transcript history, with an explicit “View original” disclosure.
  Originals are archive-only, never silently re-injected by viewing them.
- This action has no undo stack. `/undo` and `/redo` still mean turn visibility;
  they do not restore removed payloads. To reintroduce something, explicitly
  attach/send it again. Confirmation states this before committing.

**Not data erasure or semantic forgetting.** Provider requests already sent
cannot be recalled. Later replies, independent summary text, snap pixels,
vision observations (52: the `<vision_observation>` note a text-only main
model received about an image is its own row — removing the image leaves it
standing, and it was already sent to the fallback provider), knowledge,
runtime variables, spill files, source/attachment files, titles, children and
pre-existing forks may contain copies or derived facts. The feature does not
scrub those. Search remains transcript search, including archive content, but
results must visibly identify removed material and must not turn a hit into a
model input automatically.

The reverse also holds (52 §6, built 2026-09-19): an observation counts for
an image only while its note row is visible, in the tail and not removed
(`Store::vision_frame`). Removing the note with `/shake messages` while the
image stays makes that image pending again, so the next user turn on a
text-only main model consults the fallback afresh — a paid call the
confirmation does not currently warn about. Removing the image forbids
forwarding that occurrence; it does not erase the note.

A transcript entry folded before `tail_from` is already outside the active
context. Its control says **Already outside current context** rather than
claiming to save tokens. It has no active removal target. The current head
is independently removable through the picker. `/shake summary` does not
replay old raw rows or promise to erase those facts elsewhere.

### 2.3 Desktop/mobile controls

- **Image:** a small `X` icon in the attachment corner, separate from the
  zoom button. Accessible name “Remove image from context: <name>”. Native
  sibling buttons; never nest delete inside the existing thumbnail button.
- **Message:** `Trash2` in the existing user/assistant action bar and a
  corresponding action on tool/peer cards. Tooltip/label **Remove from
  context**, not “Delete file” or “Delete conversation”.
- **Confirmation:** use the existing dialog primitives. Name the target,
  list protocol companions where required, warn about summary images, and
  state that original history is retained and later replies stay. Confirm
  and Cancel; no model-generated explanation.
- **Touch/keyboard:** at least 44 × 44 px hit area, visible on coarse
  pointers, revealable on focus as well as hover, focus ring, Enter/Space
  activation, Escape cancellation. The glyph can be small; the hit target
  cannot. Do not copy the draft composer's 20 px attachment X.
- **State:** disable while running, pending, disconnected, external-harness,
  or lacking an authoritative target. The server still rechecks. Refusal
  says “Session is busy; wait or stop it, then retry.” Cancel is not an
  atomic cancel-and-delete operation.
- **Success/error:** no optimistic transcript splicing. Mark pending, await
  acknowledgement, install/reload authoritative state, announce success in
  the existing toast/live region. Failure retains content. Restore focus to
  the next attachment/action or composer; don't leave it on an unmounted
  virtual row. Preserve the current scroll anchor when possible.

Desktop, iOS and browser use these same components. **Shake context…** in
the session menu (`SessionMenu.tsx`) opens the same picker as `/shake`, so
phone users do not have to type a slash command.

`show`/artifact previews and specialized scrape cards need explicit mapping
from server context metadata. An image on screen is not necessarily in the
model frame. Show the control only on an actual model-visible occurrence;
removing a model preview does not delete its artifact or hide the artifact
browser. Queued/unsent attachments retain their existing draft/withdraw
behavior; they are not persisted-context removal targets.

### 2.4 TUI

`/shake` is in `COMMANDS`, `Model::submit` (`shake_cmd`), `Command::
{ContextItems,RemoveContext}`, `UiEvent::ContextItems`, and the `lib.rs`
executor; `Menu::Shake` follows the existing menu conventions: Up/Down
selects a row, Space toggles it, Enter reviews then confirms, Escape backs
out of review then closes. Text previews for images/PDFs; no terminal
graphics dependency.

The confirmation shows the same exact targets and companion messages as the
GUI. The result is a UI notice, not a persisted user message or model turn.
Carry removal membership through `from_history` and injected-card rendering.
Use the existing refresh-flag-to-command pattern for remote edits. Handle
notifications in the non-TTY line printer too; interactive selection requires
a TUI or GUI, not stdin prompts in print mode.

## 3. Durable identity and RPC

### 3.1 Addresses

Stored coordinates, guarded by a durable session revision. Agent-owned wire
types, as built in `crates/agent/src/context.rs` (serde `kind` tags, snake
case; `seq` exported as a TS `number`):

```rust
enum ContextOwner {
    Message { seq: i64 },
    Head { epoch: u32 },
}
enum ContextTarget {
    Message { seq: i64 },
    Head { epoch: u32 },
    Binary {
        owner: ContextOwner,
        part: u32,
        image: Option<u32>, // Some: original ToolResponse.images index
    },
}
```

`part` and `image` always address the **original stored arrays**, never the
filtered arrays. `None` addresses a direct `Binary`; `Some` requires a
`ToolResponse` part. Classify by stored MIME/type, not UI file extension.
Session id accompanies every request; identical coordinates in another
session/harness are unrelated. Do not identify a target by filename, hash,
`turn`, `first_seq`, `call_id` alone, or assistant reply ordinal.

`sessions.context_revision INTEGER NOT NULL DEFAULT 0` (migration
`0010_context.sql`). The revision travels as a decimal string on the wire,
avoiding JS integer precision loss. `store.rs` increments it in the same
statement/transaction as every append, epoch insertion, exclusion edit,
undo/redo `cut_at` change and committed cut. Monotonic within a session;
never reset when seq/epoch values are reused. No increment for a no-op edit.
`fork` copies the source value into the new row, after which the two sessions
bump independently.

This deliberately conservative versioning invalidates an old confirmation
when a new message arrives. No extra history hashes or UUID per part. The
existing append path must commit its row and revision together; a failed
write must never mint a deletable target from `Turn::persist`'s guessed seq.

### 3.2 Two methods, existing transport

```text
context_items(session) -> ContextSnapshot
remove_context(session, expected_revision, targets[]) -> ContextRemoval
```

`ContextSnapshot { revision, editable, reason, epoch, tail_from, items,
estimated_tokens }`. Each `ContextItem { target, kind, label, remove_with }`
carries a bounded label (80 chars) and the exact companion-message targets a
whole-message deletion requires. `ContextKind` is `message | image | document
| tool | summary`; snap-head frames are labelled `Summary image: <name>`.
Protocol groups are built once per snapshot (`Groups`). No image bytes or
giant tool outputs. The actor fills `editable`/`reason`; the store fills the
rest. `estimated_tokens` is `compact::est` over the effective head + tail,
never provider usage.

Every `HistoryEntry` variant gained `context: Option<ContextEntry>` with
`{ target, active, removed, remove_with, attachments }`; each
`ContextAttachment { target, name, mime, path, removed }` is a binary
occurrence with its own `ContextTarget::Binary`. A user/assistant entry maps
to its actual stored message, not its turn opener. All displayed pieces of
one injected message share that message target. Tool cards identify their
backing protocol group; one Tool row can back several cards. The `Compacted`
entry carries the head's `ContextTarget::Head` with snap frames as attachments.

`remove_context` accepts concrete targets, not a bulk category to evaluate
later. Clients expand selections using server `remove_with` metadata and
review the resulting set. The server independently computes closure and
requires it to be included; it never silently deletes additional content.
Malformed, foreign, stale, inactive, or incomplete sets fail atomically.
Deduplicate targets; whole-message removal dominates attachment removal.

`ContextRemoval { revision, messages, images, documents, heads,
estimated_tokens }` returns the committed revision and actual counts
(companions counted in `messages`; attachments subsumed by a whole-message
removal not counted). `AgentEvent::ContextChanged { session, revision, reason }`
with `reason ∈ {"context_edit", "settled", "rewind"}` notifies every attached
client **after commit and installation in live state**. It contains no
removed bodies. `AgentEvent::session` covers it; Rust schema/TS exports, the
desktop `Api` and sorted `METHODS`, and the generated binding copies include
both methods. The RPC bridge/Link transports carry it unchanged; the Slack
channel deliberately ignores it.

`Loaded` gained `context_revision`, `context_editable`, `context_reason`,
`context_estimate: Option<u32>` and `settled`; `LoadSession`/`load_session`
gained `settled_only`. The estimate is never described as provider usage;
recorded turn usage/costs remain historical facts.

### 3.3 Durable attachment presentation

A removal control must survive resume and work for another client. Replace
optimistic-only user attachment presentation with authoritative occurrence
metadata in history. Propagate it through user, tool, peer and specialized
card rendering; keep optimistic files only until persistence is reconciled.

For a stored binary without a safe existing attachment path, the history
builder (`supervisor.rs`) materializes its base64 bytes as
`<data_dir>/attachments/<sha256[..16]>.<ext>` through `context::save_blob`,
the one naming rule shared with RPC `put_attachment` and the ACP prompt path,
and returns the path in `ContextAttachment.path`. It is served through the
existing guarded `get_attachment` method. Lazy per loaded page, not a sweep;
idempotent for an existing file; the RPC crate calls into `agent::context`,
not the reverse.
A failed preview still shows a named removable chip. Never fetch arbitrary
remote URLs solely to populate the picker. No full binary payload in
`Loaded`, events, or command inventory.

Unstored live cards have no removal target. After `TurnDone` restores state,
publish a coalesced `ContextChanged` notification for the settled history so
clients reconcile live cards with persisted identities; do not reload on
every delta. Manual compact/harvest handback follows the same rule. If queued
input immediately starts a new turn, metadata reports busy, not editable.

## 4. Storage and projection

### 4.1 Schema: masks on the existing rows

Migration `0010_context.sql` adds `sessions.context_revision` and nullable
`excluded TEXT` columns to `messages` and `epochs`. `NULL` means no
exclusions. The mask JSON is `{"whole":bool,"binaries":[{"part":n,"image":n|null}]}`
(`context::Mask`); message protocol companions receive their own `whole`
masks in the same transaction. Original `body` and `head` JSON stays
unchanged. No replacement copies of retained base64 data, separate message
tree, general patch language, or append-only edit ledger.

Raw transcript readers remain explicit: turn boundaries, undo/redo, archive
history, and search use original rows plus membership metadata. Effective
readers apply masks: `Store::frame`, active context inventory, replay and
model-facing derived state. Avoid changing `Store::messages` globally and
thereby changing turn boundaries as an accidental side effect.

Projection rules:

| Selection | Effective frame |
|---|---|
| User/direct binary or PDF | Drop exactly that original part; preserve every other part. |
| Tool-response image | Drop exactly that image; preserve response text, call id/name, sibling images/responses and assistant calls. |
| Last meaningful content part removed | Omit the now-empty model message. Do not send an invalid empty content array or replace the removed data with a large stub. A tool response whose image-only body empties retains its required envelope with a short neutral “Image removed by the user.” text. |
| User message | Omit the entire row, including all attachments. Raw row remains for turn/rewind identity. |
| Assistant or Tool message | Omit its whole protocol-connected message group (§4.2), including reasoning/signature parts in omitted assistant rows. |
| Current head | Omit the head but retain the epoch number and `tail_from`; older messages must not re-enter the frame. |
| Binary in head | Drop only the selected head part; keep the remaining head. If no meaningful head remains, omit it while retaining the boundary. |

For partial snap removal, adjust only the effective head's frame-count/caption
metadata so it does not claim missing frames are present; mark it partial.
Do not rewrite its remaining rasterized content. This is part of the explicit
edit, not a per-request mutation.

Resolve the epoch/boundary **before** projecting the head. `head == None`
after exclusion must not mean “epoch zero; replay everything.”

### 4.2 Tool protocol closure

A stored assistant row can contain text plus calls A and B; the following
Tool row can contain responses A and B. A delete button on that assistant
or either tool card therefore previews removal of both raw rows, including
both tool results and the assistant prose. Later assistant replies stay.

Compute connected components of message rows joined by tool-call/result
identity within the session's active frame. Selecting a whole assistant/tool
message selects that component. Iterate to closure for unusual multi-row
responses. Do not join unrelated repeated call ids blindly; ambiguous or
malformed pairings refuse with a repair-needed error, not a guessed deletion.

This coarser whole-message group is intentional: it preserves untouched
signed assistant messages rather than editing their internals. Per-image
removal is still fine-grained and does not remove the surrounding exchange.
No synthetic “tool ran successfully” result and no reliance on
`Turn::repair_dangling`, which only repairs a missing final tool response.

`Active::validate` checks the projected frame's protocol structure before
the commit, strictly over the whole post-edit frame: no empty message, calls
only in assistant rows and results only in tool rows, unique call ids on
each side, every call batch answered by the tool rows that immediately
follow it, no result without its call, no dangling trailing call. A failing
edit refuses and writes nothing. This is a theuth-side structural check, not a
run through the genai provider serializers (see §9); provider shape was
smoked separately (§7).

### 4.3 Atomic application and live state

`Agents` → native `Session` (`RemoveContextNow`) → `Store::remove_context`.
In the session actor:

1. `Session::idle()` requires `inflight.is_none()` and `state.is_some()`; no
   borrowed running state. `claim` is not called, so no unrelated pending
   undo cut is committed. Refusal text: “Session is busy; wait or stop it,
   then retry.”
2. `BEGIN IMMEDIATE` transaction: read/check revision (stale refuses first),
   load the persisted active frame (`Active`), resolve all targets, compute
   the `Plan`, apply and `validate`. Unknown targets, stale revision, decode
   errors and protocol errors write nothing.
3. Persist masks with `UPDATE … WHERE seq/n` (a vanished row aborts) and
   `context_revision + 1 RETURNING` in the same transaction; then one
   `tracing::info!(…, "context_edit")` boundary line with the counts.
   `cut_at`, hidden redo rows, all unselected rows and epoch chronology are
   preserved. Failure returns an error; nothing continues into an in-memory
   deletion.
4. `SessionState::install` replaces the idle actor's effective head/history
   before the next message is handled. Live todos, runtime, title cadence and
   unrelated state stay (`resume` is not called); content-derived skill
   membership is recomputed from the effective history.
5. `prev_call` and pressure bookkeeping clear, `used` is seeded from the
   `compact::est` convention, and the committed `ContextRemoval` plus
   `ContextChanged { reason: "context_edit" }` are published. Next real
   provider usage replaces the estimate. Actual usage totals are kept.

If installation fails after a durable commit, invalidate the live actor and
reload from the committed store before admitting another turn; never run its
old frame. A crash before commit leaves the old projection; after commit,
resume reconstructs the new one.

An input/background completion processed first can make the session busy
and refuse the edit. If the edit runs first, later input uses the new state.
No pause scheduler, cancel-and-edit queue, or “apply eventually” operation.
Incoming child/job output may legitimately introduce new content later.

Title jobs read the revision with their digest and commit through
`Store::set_title_at_revision`; a title captured before an edit is discarded
(`Ok(false)`, logged). Requests already prepared or issued before the edit
are not recalled. This exception is explicit, not a promise that
provider-side data was erased.

### 4.4 Epochs, rewind and derived consumers

- **Compaction:** future summary/snap generation and harvest use effective
  state. Keep old heads unless the user selects them. Do not automatically
  harvest content before removing it. Do not manufacture a `CompactStyle::Shake`.
- **Summary limits:** even a row retained after `tail_from` may already have
  informed the current summary. Removing it does not erase those derived
  sentences. Head removal is explicit and drops all of that head at the same
  boundary. Never delete epochs and fall back to older raw history: that can
  undo `/clear` and inflate the window.
- **Undo/redo:** raw role adjacency still defines turns; masks stay attached
  to their rows/heads. Restoring a hidden turn/epoch restores its exclusions
  too. `cut_to`/`commit_cut` bump the revision and the actor publishes
  `ContextChanged { reason: "rewind" }` so other clients cannot operate on
  old views.
- **Regenerate:** uses the effective opener. A removed or text-empty opener
  refuses with “opener was removed from context; edit it instead”; its
  original binaries are not reattached. Explicit edit/send supplies new
  content and may intentionally reintroduce it.
- **Fork:** copy masks with the copied rows/epochs in the same transaction,
  with turn/reply addressing still based on raw history. Mid-reply tool-call
  stripping must remap retained binary coordinates or materialize the effective
  final copied row; never apply old positions to a shortened part vector.
  Snapshot source revision and copied data consistently. Already existing
  forks and children are independent and unchanged.
- **Committed cuts/session deletion:** masks disappear with their owning rows;
  no side table cleanup. The surviving session revision does not go backward.
- **Model switch/text-only mode:** exclusions precede `strip_images`; switching
  back to a vision model cannot restore removed binaries.
- **Skills/title/knowledge inputs:** model-facing derivations use effective
  messages. Archive search deliberately remains raw and labeled. Runtime data
  and already-harvested knowledge are not purged or silently replayed as part
  of the edit.

## 5. Synchronization, failure and performance

`ContextChanged` marks only the affected harness/session slice stale. The
visible client reloads authoritative history; hidden slices reload on next
visit. Reconnect, `Lagged`, mobile foreground reconnection and returning to
an unsubscribed harness invalidate previously loaded state. React Query
invalidation alone is insufficient: the transcript is in Zustand.

Capture the initiating harness's API/store before awaits. An RPC completing
after a harness switch must not modify the newly active harness's session.

History, context inventory and removal replies carry their read revision;
read each snapshot consistently. Reject stale asynchronous replacements and
pre-edit pagination responses. Pagination must not split a shared seq group
(tool siblings/dividers) and then skip its remainder with `before=seq`.
Include the whole boundary group, even if it exceeds the nominal page size.
Preserve source-message identity through injected-card splitting.

A mutation-triggered reload must not overwrite a racing live stream. Built as
`load_session(…, settled_only: true)`: the supervisor reads the history
through the session actor (`ContextHistoryNow { settled_only }`), which
refuses with the busy text unless it owns idle state; the reply carries
`settled: true` only then. A plain load is allowed at any time and reports
`context_editable: false` with a reason while a turn runs. `TurnDone` and
compact/harvest handback publish a coalesced `ContextChanged { reason:
"settled" }`, which is the retry signal for a client whose settled reload was
refused. Clients (`store/reduce.ts`, `tui/model.rs`) keep a load generation
(`historyGeneration`) as well as the revision; a page or snapshot requested
under an older generation — after a context edit, reconnect, harness switch
or newer activity — is discarded, and `ContextChanged` at an already
installed revision is a no-op unless the slice is stale. No text-fragment
deduplication against a partially persisted turn is attempted.

On stale confirmation, reload and require review again; never retarget by
position. On a disconnected mutation with unknown outcome, reload before
retrying. Do not automatically replay a destructive RPC. With a current
revision, an exact already-removed stored target returns zero changes and no
new event; validate its original identity before treating it as a no-op.
Other inactive/unknown targets refuse, and a stale revision refuses first.

One projection pass on load/edit, no new per-token work. Masks contain only
coordinates. Inventory labels are bounded; attachment bytes use existing
lazy preview loading. Build call/result groups once, not one full-history
scan per item. Existing virtualized transcript, frame-coalesced events and
per-harness stores remain. No separate thumbnail cache or token estimator.

## 6. Implementation map

Each row is a complete integration boundary and is built.

| Order | Files/symbols | Work |
|---|---|---|
| 1 | `agent/migrations/0010_context.sql`; `agent/src/context.rs`; `agent/src/store.rs` | Revision/masks (`Mask`), `Active` frame with `Plan`/`apply`/`validate`, `Groups` protocol closure, `project`/`project_head`, inventory/`entry`/`attachments`, `remove_context`, `context_items`, `context_history`, `set_title_at_revision`, fork copying masks. Raw readers (`messages`, turn starts, undo/redo) unchanged; `frame` reads effective rows. |
| 2 | `agent/src/{lib,supervisor,loop_}.rs` | `ContextItems`/`RemoveContext`/`LoadSession.settled_only` messages; `Session::remove_context`, `context_history`, `SessionState::install`, `ContextChanged` publication (edit/settled/rewind); `HistoryEntry.context`, page-time attachment materialization; regenerate refusal; revision-guarded titles. `Frame` stays a pure adapter. |
| 3 | `rpc/src/api.rs`; generated `bindings/` and desktop copies | `context_items`, `remove_context`, `load_session(settled_only)`; schema/TS exports for the context types and event; `save_blob` delegated to `agent::context`. |
| 4 | `tui/src/{model,lib,ui}.rs` | `/shake` in `COMMANDS`, `ShakeCategory`, `Menu::Shake` picker/review (`ShakeStage`), `Command::ContextItems`/`RemoveContext` executor, `removal_line`, revision/stale tracking with settled reload, line-printer `context changed (<reason>)`. |
| 5 | `desktop/ui/src/components/chat/{slash,ContextPicker,Attachments,Message,MessageAssistant,MessageUser,MessageTool,SessionMenu}.ts(x)` | `/shake` categories, `ContextPicker` (pick → review → one `remove_context`, `expand` closure), `RemoveButton` with “Already outside current context”, attachment corner `X`/chip `X`, tombstone with archive-only “View original”, **Shake context…** menu item. |
| 6 | `desktop/ui/src/{store/sessions,store/reduce,rpc/api,routes/c.$session}.ts(x)` | `contextRevision`/`contextEditable`/`contextStale`/`contextEstimated`/`historyGeneration`; `ContextChanged` reducer; settled reload with coalescing and busy retry; harness-scoped invalidation. |
| 7 | [00](00-architecture.md), [02](02-models-and-menus.md), [07](07-context.md), [17](17-images.md), [25](25-desktop.md), [29](29-rewind.md), [31](31-link.md), README | As-built notes: cache exception, command/parity tables, history/attachment identity and replay semantics. |

[47 todos](47-todos.md) landed concurrently; [46 apps](46-apps.md) remains a
separate design. `install` preserves live todos and `todo_version` (covered
by `context_edit_is_idle_only_and_installs_committed_frame`),
and the `TodoChanged` event is independent of `ContextChanged`.

## 7. Acceptance and verification

### Acceptance checks

1. **Exact image selection:** user text + images A/B, tool text + images C/D,
   then a later reply. Remove A and C. Captured effective request retains
   text, B/D, call/result ids and the later reply; A/C absent. Reusing the
   same path twice removes only the selected occurrence. A second edit still
   addresses original positions. PDFs are excluded from `/shake images`.
2. **Whole message/protocol group:** remove a plain user or assistant message
   without truncating later conversation. With assistant text + signed
   reasoning + calls A/B and batched responses, review lists companions and
   deletion removes the complete group. No orphan response or retained hidden
   content from the selected raw assistant row. An incomplete set refuses.
3. **Head/boundary:** remove one snap PNG and then the whole head. Resume uses
   the edited head or no head at the same `tail_from`; no folded/cleared rows
   return. Existing summary-derived facts remain unless their head is removed.
4. **Persistence/replay:** restart, undo/redo, regenerate and fork after image
   removal. No path resurrects the removed occurrence. Empty-opener regenerate
   refuses. Forking mid-reply does not shift masks onto a different part.
5. **Transaction/race:** fail the write; all content remains. Crash after
   commit; new projection resumes. Race edit against input, compaction,
   harvest and queued child/job delivery; either edit commits first or returns
   busy/stale, never a mixed frame. An old seq reused by regeneration cannot
   be deleted by a stale button. ACP sessions refuse without local mutation.
6. **Real frontends:** exercise `/shake` and categories in the actual TUI and
   shared desktop app; single-image/message controls on desktop and a coarse
   pointer/mobile viewport. Keyboard focus, 44 px touch targets, lightbox
   sibling controls, error/busy state and screen-reader labels work. A user
   attachment sent by a different client is removable after resume.
7. **Multi-client:** edit on desktop while the TUI/phone shows the session;
   all converge. Race old-page prepend and streaming events with reload; no
   removed item returns and no new delta disappears. Switch harnesses during
   the RPC, disconnect during commit, reconnect/foreground, and return to an
   inactive harness. Context meter shows an estimate, not old provider usage.
8. **Actual provider shape:** capture the post-edit serialized request and
   smoke it against representative Anthropic, OpenAI and Gemini/Bedrock
   adapters, including signed reasoning, image-only messages and tool images.
   Verify unchanged retained blocks and valid ordering; do not infer success
   from a UI tombstone or from the model saying it forgot an image.
9. **Cache invariant:** frames append normally before the explicit edit;
   the edit produces one logged boundary; subsequent frames append normally
   again. Session/cache key is unchanged. Do not demand a guaranteed cache
   hit or exact savings from providers; usage is measured on the next call.

Regression tests live beside the code (`context.rs`, `store.rs`, `loop_.rs`,
`tui/model.rs`, `reduce.test.ts`, `ContextPicker.test.ts`): occurrence
indices, protocol closure and strict validation, revision/transaction races,
replay persistence, pagination shared-seq groups and UI generation races. UI
and transport acceptance used the real frontends and daemon, not source-text
assertions or mock forwarding tests.

### Verification performed (Main, 2026-09-18)

Observed by the integrating agent on the implemented tree, not reported by
the implementing subagents:

- Final `cargo check --workspace --all-targets` passed. `cargo test -p agent
  -p rpc -p tui -p tools`: **245 passed across 8 suites**. Generated bindings
  export and `cargo build -p theuth` passed. Desktop `bun run check`, `bun run
  test`, and `bun run build` passed: **22 test files, 131 tests**. The Vite
  bundle reports its large-chunk warning; no build or type errors remain.
- **Provider shape (check 8, local only):** a throwaway `context_smoke`
  example drove the real genai `Client` over HTTP to a loopback capture with
  fixture responses, for Anthropic, OpenAI, Gemini and Bedrock adapters, in
  three phases each: pre-edit, after image A/C deletion, after whole tool-group
  deletion. All four passed: exact markers absent/retained, call/result and
  signature constraints where the adapter serializes them, OpenAI session
  cache key unchanged. This is request serialization against local fixtures —
  **not** provider acceptance, billing, or cache-hit measurement. Adapter
  limits observed: Bedrock drops tool images and signatures itself, Gemini
  maps calls by name, OpenAI has no signed-thinking slot; the feature does not
  claim to preserve what an adapter never serializes.
- **Daemon RPC (checks 1–5):** on an isolated `theuth` daemon, `context_items`
  and `remove_context` removed exact direct and nested (tool-response) image
  occurrences with siblings and PDFs retained; a stale revision refused; a
  whole tool group removed; head omission kept epoch 1 and `tail_from` 4; an
  external-harness session answered non-editable; a fork retained masks.
  Restarting the daemon and reloading showed the masks persisted.
- **Shared React UI (checks 6–7, DOM only):** in a real browser at 1280×900
  and then 390×844 with a coarse pointer, image A was removed via the corner
  control; `/shake tools` review listed the assistant companion; confirming
  tombstoned the tool and assistant rows and preserved later replies; focus
  returned to the composer `TEXTAREA`. Every removal target measured 44×44 px,
  opacity 1 on mobile, no nested buttons. Mobile removal of image B worked. A
  remote HTTP client removing C while the mobile view was shown converged via
  `ContextChanged` (C's control gone, archive chip present). An open
  confirmation for A followed by a remote removal of D was refused as stale
  and A stayed. Switching the active harness to an isolated offline scope,
  mutating the original harness while unsubscribed, and switching back
  reloaded the masks with no old images restored — a harness-scope lifecycle
  test, not a Noise/relay pairing. **No screenshots or visual review were
  performed**; these are DOM/accessibility-tree measurements.
- **TUI (checks 6–7):** the real binary in a PTY (`--serve` on an isolated
  copy, `--resume`): `/shake images` reviewed 4 images, Enter committed, the
  notice reported 4 images removed, later conversation and the PDF stayed, no
  model turn ran. A remote HTTP removal of user seq 4 while the TUI was
  visible produced the collapsed removed card; the later assistant row stayed.
- **Unit coverage added:** injected SQLite revision-write failure rolls back;
  reopen durability; concurrent same-revision edits commit exactly one; busy
  actor makes no change; strict orphan/duplicate/role/order protocol checks;
  snap/head boundary; fork/rewind with original indices; pagination of a
  shared seq; UI generation races.

**Not verified:** any real provider call, network bill, or prefix-cache hit
after an edit (check 9's savings clause); visual rendering; iOS device
build; a real Link/relay pairing. Check 9's structural half — normal appends
before, one `context_edit` log line at the edit, normal appends after, cache
key unchanged — is covered by the local smoke and unit tests.

Smoke state lived under temporary directories and no user data was touched;
the `context_smoke` example was throwaway and is not part of the tree.
Concurrent todo/activity work by other agents was preserved throughout; the
only shared edits made for it were compile unblockers (tools registry lock
scoped before an await, PTY `Running` visibility matching the actor reply, a
`TodoCard.test.ts` fixture annotation), and none of that feature is
attributed to shake.

## 8. Deliberately not included

- Automatic/model-driven shaking, thresholds, policies, new slash grammar.
- A new compaction algorithm or a model call to decide what the user meant.
- Semantic redaction across summaries, knowledge, files, providers or sessions.
- Permanent transcript/file deletion, garbage collection, a new undo ledger.
- A context-source registry, event-sourced conversation tree, or new plugin.
- Native-context mutation of external ACP harnesses without their support.
- Separate mobile logic, new image renderer, transport, or dependency.

The feature is explicit context editing. These omissions keep its promise
narrow and testable without weakening the requested desktop, mobile or TUI
behavior.

## 9. Differences from the original proposal

- **Protocol validation is structural, not adapter-driven.** §4.2 proposed
  validating the projected request through the existing genai adapters. The
  build validates with `Active::validate` inside the store transaction — a
  strict theuth-side check of message emptiness, call/result placement, ids and
  ordering — and does not serialize through provider adapters on the commit
  path. Adapter output for the post-edit frame was captured by the local
  `context_smoke` example (§7) and matched. Neither path certifies remote
  acceptance: the serializers do not encode provider-side policy either, so
  running them in the transaction would not have validated it. Structural
  check plus captured adapter output is the local guarantee; no real
  provider acceptance was tested.
- **`ContextChanged.reason` has three values** (`context_edit`, `settled`,
  `rewind`); the proposal named only the edit boundary. Clients treat
  `settled` at an unchanged revision as a retry signal, not a change.
- **Fork revision** copies the source's `context_revision` into the fork row
  rather than starting at 0; the domains are independent afterwards, which is
  what the proposal required.
- **Estimate convention:** `context_estimate`/`estimated_tokens` is the
  existing `compact::est` content-only estimate (no tool schema or system
  prompt share); the desktop meter flags it `contextEstimated` until the next
  provider usage arrives.
- **Adapter limits are not features.** Bedrock drops tool images and
  signatures, Gemini matches calls by name, OpenAI has no signed-thinking
  slot; the projection preserves what the stored row holds, and each adapter
  then serializes what it always did.

Everything else in §0–§5 is built as written.
