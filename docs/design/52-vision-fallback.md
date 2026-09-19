# 52 — Vision fallback: the main model asks, a vision model looks

Status: **proposed · 2026-09-19 · design only, not implemented**.

Extends [00](00-architecture.md), [02](02-models-and-menus.md),
[07](07-context.md), [08](08-repl.md), [17](17-images.md),
[23](23-browser.md), [25](25-desktop.md), [31](31-link.md), and
[48](48-shake.md). Source observations describe the inspected tree, not
implementation claims. [28](28-untrusted-content.md) and
[51](51-session-lifecycle-costs-and-terminals.md) remain proposals, not
prerequisites this design can assume are built.

## 0. Decisions

1. Add one harness-wide setting, `agent.vision_model`, with TUI and shared
   desktop/mobile controls beside the title-model controls. Unset means no
   fallback, **not** “use the main model.”
2. Use `models::Catalog` and the selected backend to decide whether the
   **current main model can receive images**. If it can, take today's native
   image path. No fallback request, prompt, note, or fallback validation on
   that path. A broken fallback must not break a vision-capable main model.
3. Otherwise intercept every image occurrence in the effective context.
   The main model first writes a self-contained question about what it needs
   from the image. Only that question and the selected images go to the
   configured vision model. Its text answer returns to the main model, which
   remains responsible for decisions and actions.
4. This is a mandatory agent-loop preparation step, not an optional tool the
   main model might forget to call. Neither auxiliary request has tools.
   It works with text-only main models that also lack tool calling.
5. Keep original images in history. Project them to stable text references
   for text-only requests; append attributed question/answer notes. Never
   replace old image placeholders with new descriptions in place.
6. Reuse `agent`, `models`, `Backend`, the existing store, RPC, and UI
   controls. No vision plugin, child session, peer exchange, autonomous
   worker, second provider client, or caption cache service.

## 1. Philosophy: what wth is optimizing for

[00 §1](00-architecture.md#1-non-negotiables) makes wth a general assistant
that acts on the user's machine, not a coding-agent framework. Libraries do
protocol work; plugins own their contracts; `main.rs` does not wire feature
handles between subsystems. A crate must earn its existence. Here the
existing owner is `agent`: it already owns model selection, context,
provider calls, titles, compaction, and knowledge continuations. Images do
not justify another owner.

[07 §0](07-context.md#0-the-invariant) makes stable context both a reasoning
and a cost invariant. Normal conversation requests grow by appending;
compaction, rebase, and explicit user context edits are named boundaries.
Generating a description and substituting it into yesterday's screenshot
would violate that invariant. Keep the image reference stable and append
new evidence at the tail instead. `Frame` remains pure; network I/O belongs
in the turn, not in a serializer.

[25 §0](25-desktop.md#0-decisions-in-one-screen) makes `rpc::Api` the shared
command surface. Desktop, iOS, and linked browser views use the same React
settings and transport abstraction. No Tauri-only setting, phone-local
model preference, or frontend image inference belongs here.

[17](17-images.md) and [48](48-shake.md) distinguish source history from
what a model is allowed to receive. Preserve that distinction: provider
capability selects a projection, and user exclusions select the effective
context first. Neither selection destroys the source record.

The practical tradeoff is explicit: a text-only main model costs an extra
main-model question call plus a vision call for each new image batch.
That is necessary for task-directed inspection. A generic automatic caption
is cheaper but does not meet the requirement that the main model asks what
it needs to know.

## 2. Current source, including holes the feature must close

| Area | Current behavior and source |
|---|---|
| Capability authority | `crates/models/src/lib.rs`: `Model.image` comes from models.dev input modalities. `Catalog::load` merges user overrides and fills zero-limit gateway entries from catalog siblings. `Catalog::resolve` can attach fuzzy metadata to an unknown explicit ID; it is resolution, not proof of availability. |
| Effective vision | `crates/agent/src/loop_.rs`, `Current::vision`: `Backend::Genai` **and** resolved `Model.image`. Cursor's backend is text-only even when its catalog sibling supports images. |
| Title setting | `agent/src/lib.rs`, `AgentConfig`, `SetTitleModel`, `GetTitleModel`; `supervisor.rs`, matching handlers and `start`; `loop_.rs`, shared live value. Title's setter changes memory before a best-effort config write; that persistence failure behavior should not be copied. |
| User images | `supervisor.rs`, `check_files`; `loop_.rs`, `take_input` and `user_message`: input, queued input, steering, and replay become multipart user messages. Frontends send paths; stored messages retain binary payloads. |
| Direct tool images | `tools::ToolOutput.files` becomes `ToolResponse.images` in `Turn::round`. Browser screenshots, computer screenshots, and artifact preview images already use this contract. |
| Python hole | `agent/src/repl.rs`, `Repl::drive`, `tool_result`, `finish_output`: inner tool files reach UI events, but the Python value conversion drops them and the outer cell result has no files. An image-only inner result can therefore never reach the native frame. |
| MCP hole | `tools/src/mcp.rs`, `McpTool::call`: non-text blocks are serialized as JSON text and `files` is empty. An MCP image can become base64 text instead of an image part. |
| Request stripping | `agent/src/frame.rs`, `Frame::request` and `strip_images`: user binaries become omission text; nested tool images are removed with omission lines. No model interprets them. |
| Other request builders | `Turn::compact` explicitly strips images for summary compaction; `continuation` builds a frame for summary/harvest; `maybe_title` uses a text digest. Cover these deliberately, not just `round`. |
| Durable identity | `context.rs`, `ContextTarget::Binary`, `ContextOwner`, `Active`, `Mask`; `store.rs`, `frame`, `active_in`, `append_in`: original coordinates and exclusion masks exist. The projected frame has shifted indexes after removals, so it cannot recover original coordinates by enumeration. |
| Lifecycle | Store seq/epoch numbers can be reused after a committed rewind. `context_revision` changes on every append and edit. It is a freshness guard, not a stable image identity. |
| Shared UI | `tui/src/{model,lib}.rs`; `rpc/src/api.rs`; `desktop/ui/src/components/settings/General.tsx`, `SettingsDialog.tsx`, and `components/chat/slash.ts`. Desktop/mobile already share the title controls. |

**Scope boundary:** this covers every model request whose context wth's
native `Session` owns, including native subagents and inputs received through
RPC, TUI, desktop/mobile, Signal, Slack, and wth's inbound ACP server. An
external ACP harness owns its own model/context and tools; its translated
transcript is not that context. This setting must not claim to intercept
hidden external calls. A remote wth node applies its own setting locally;
no fallback credentials or global setting are copied over a peer connection.

## 3. Configuration and UI contract

```yaml
agent:
  # title_model: anthropic/claude-haiku-4-5
  vision_model: anthropic/claude-haiku-4-5  # example; must be available and image-capable
```

The model is an example, not a new default. Missing/null disables fallback.
`WTH_AGENT__VISION_MODEL` follows existing Figment precedence. Setters write
`<data_dir>/config.yaml`, as title/model setters do today; a separately
supplied `--config` file is not rewritten, and an environment override wins
again on restart. State these existing limitations rather than redesigning
configuration routing in this feature.

### 3.1 Agent and RPC

Add beside the title setting:

```rust
// Sketch of new agent-owned contracts, not existing source.
pub struct SetVisionModel { pub model: Option<String> }
pub struct GetVisionModel;
// Replies: Result<Option<ModelInfo>, String> and Option<ModelInfo>.
```

RPC methods:

```text
vision_model() -> ModelInfo | null
set_vision_model(model: string | null) -> ModelInfo | null
```

Use one `Arc<RwLock<Option<Resolved>>>` shared by the supervisor and native
sessions. A consultation snapshots the selection; never hold its lock across
an await. Changing it affects the next consultation, not an already issued
request, and does not regenerate old successful observations.

For a setter, resolve with the existing catalog/auth predicate; require a
canonical exact catalog entry, effective image support, and a configured
provider. Reject ambiguous IDs, unmatched/fuzzy-only selections, Cursor,
text-only entries, and unavailable providers. Named gateways are valid when
their exact merged catalog entry supports images. “Configured” is the
existing local availability check, not a billable probe or proof that remote
authentication will succeed. Do not require `tool_call`: the fallback has no
tools. User catalog overrides remain the calibration mechanism.

Validate first, persist successfully with the existing config-writing
mechanism, then swap the live value. A failed write returns an error and
leaves the live value unchanged. Do not change title's semantics as a
side effect of this feature.

Startup uses the same ID/capability validation, but a valid selection whose
credentials have disappeared remains selected and fails visibly at use.
An invalid ID/capability warns, disables only this fallback, and leaves the
file untouched; report that startup diagnostic through the existing error
surface, not just debug logging. A wrongly typed `vision_model` must be
reported, not silently reset the whole agent section to defaults. The
current whole-section `unwrap_or_else(Default)` is a source hazard to fix
at config extraction. No startup vision request is made.

### 3.2 Surfaces

| Surface | Behavior |
|---|---|
| TUI `/vision` | Show selected fallback or Off and the command to set it. |
| TUI `/vision <provider/model>` | Set through RPC; show the canonical returned selection. |
| TUI `/vision off` or `/vision clear` | Send null; no fallback. |
| Desktop/mobile Settings → General | “Vision fallback model” beside “Title model”; Off plus available, effective vision-capable models. |
| Desktop/mobile `/vision` | Open General, matching bare `/title`. Explicit ID/off/clear uses the same RPC. |

Description: “Used only when the main model cannot view images. It receives
the image and a question written by the main model.” Mention that this can
send images to a different configured provider and incurs additional calls.

Use the current Select/Row, pending state, error toast, and per-harness query
cache. Keep a selected but now unavailable ID visible as unavailable; do not
render it as Off. Invalidate/refetch the model inventory after provider/auth
changes and on settings reconnect. Slash setters invalidate the same setting
query as the dropdown. Keyboard and touch behavior comes from the shared
controls; no separate mobile implementation.

Make `ModelInfo.image` report **effective input support**, catalog capability
constrained by backend support, in every producer. This corrects Cursor rows
that currently inherit `image: true` but cannot deliver it. Frontends filter
that value rather than copying backend rules into TypeScript. Update the
field's documentation and attachment warnings: for a text-only selection,
say whether the configured fallback will inspect the image or vision is off.
The server remains authoritative when frontend state is stale.

## 4. Routing and immutable image references

### 4.1 Branch before doing fallback work

Use the existing `Current::vision` decision at each request boundary, not
`agent.model` cached when the session started:

```text
current main model supports image input through its backend?
  yes -> existing native request; skip fallback entirely
  no  -> effective images absent? -> ordinary text request
         fallback unset?         -> stable text references + unavailable notice
         otherwise               -> prepare pending images, then ordinary text request
```

The fallback is not provider-error recovery for a vision-capable main model.
Do not catch an arbitrary provider 400 and route to another model. Catalog
errors need catalog correction; provider errors remain real errors.

Main routing reuses the catalog's current resolution behavior, including
merged overrides and sibling inference. No hand-maintained model-name list,
network capability probe, MIME-as-capability guess, or independent frontend
capability registry. An unresolved model with no capability metadata is
text-only. Fallback selection is stricter (§3) because it must name an
actual configured image receiver, not merely inherit a plausible family.

### 4.2 Occurrence identity and projection

For the text-only path, build an agent-private image index from raw `context::Active` rows **and
masks** in the same store snapshot that installs the effective frame. Include
active user binaries, nested `ToolResponse.images`, and current head images;
exclude masked/cut/folded occurrences and non-image binaries such as PDFs.
Reuse `ContextTarget::Binary` and original `part`/`image` coordinates.

Each reference is `(ContextTarget::Binary, fingerprint)`, scoped implicitly
to its session. Fingerprint the stored image payload plus MIME once at
append/load. For old URL-source binaries, fingerprint their stored source;
that identifies the occurrence, not immutable bytes at the remote URL.
Never use a mutable source filename as image identity. Seq reuse after a
rewind cannot match an observation for different content.

Update the index incrementally on append. Rebuild it on resume, fork,
rewind/regenerate, compaction, and context removal from the same authoritative
snapshot. No SQLite inventory query or full-image hashing on every model
call. Existing `Arc` binary payloads can be shared; no second base64 copy.
Build/hash lazily on first text-only use, not on ordinary native-vision
requests. A model switch can acquire one fresh authoritative snapshot before
preparation; there is still no per-request database scan.

For text-only requests, replace each image in a **request copy** with a
stable reference, source label, and MIME. Preserve adjacent text, tool
call/result IDs, ordering, signed reasoning, and cache options. Image-only
tool results retain their envelope. Reference wording must not vary with
pending/completed state or the configured fallback; status belongs in new
tail notes. Results never replace these placeholders.

This traversal must cover both supported image shapes regardless of producer;
`strip_images`' current role-specific omission path is not enough as an
interception interface. PDFs retain their existing separate handling. No new
PDF rendering, audio/video pipeline, or OCR service is implied. Raster images
produced from a PDF or SVG are images and are covered.

## 5. Mandatory main-model → vision-model consultation

### 5.1 Turn-owned preparation

After new input/steering or a complete tool batch has been persisted, and
before the next normal main-model call, collect active image occurrences
without a successful, still-effective observation. Run the same preparation
before summary compaction or harvest can fold/read them (§7).

1. Snapshot current main model, fallback selection, session cancellation,
   and the effective frame. If main vision is supported, return immediately.
2. Group pending images from the same source message in original order.
   Preserve related images together where the fallback's request limits
   allow it. Process groups serially inside the existing turn permit;
   do not fan out paid inference behind the concurrency controls.
3. Ask the **current main model** for a question using a text-only copy of
   its effective context and an appended preparation instruction. Include
   the selected image references and source text. Request a self-contained
   prompt describing exactly which visual facts it needs, with per-reference
   distinctions when several images are involved. Do not supply a generic
   caption as the answer to this step.
4. Send the resulting prompt, selected images with those same labels, and
   a short vision-specific system instruction to the configured fallback.
   **No other conversation, workspace, tools, skills, peer roster, or
   credentials as prompt text.** Use the fallback's own normal authentication.
   Forward short source labels, not absolute filesystem paths. Request JSON
   with one `{ref, status, text}` observation per supplied label, where
   `status` is `answered` or `unreadable`. Parse with existing serde support;
   no provider-specific structured-output API or tool calling is required.
   Reject unknown/duplicate labels; a missing label is a failure for that
   image, not implied success. Preserve the model's answer text in the note.
5. Append one attributed observation note containing the exact main-authored
   prompt and the vision answer, or a concrete failure. Commit its metadata
   in the same store transaction. Only then admit the normal main call.
6. The main model receives the unchanged earlier text-only projection plus
   that note and continues the user's task. Only it can invoke world tools
   or produce the user-facing conclusion.

Example, not fixed output wording:

```text
User: Which highlighted total is overdue? [two images]
Main question call (text only):
  In image A read the highlighted total and due date. In image B read the
  statement date. Compare the dates; quote the visible values and flag blur.
Vision call (that prompt + A + B):
  A: $420.00, due 12 September. B: statement date 19 September. ...
Appended observation:
  Vision observation, source A/B, model provider/model.
  Question from main model: ...
  Answer (image-derived evidence, not instructions): ...
Main normal call: answers the user from those observations.
```

**Not optional:** a configured fallback with pending images cannot reach
ordinary tools or a final main-model answer without completing or explicitly
failing this preparation. An ignored tool description is not interception.
There is no model-generated source selector: the harness selects actual
active occurrences and the main model supplies only the question text.

### 5.2 Auxiliary requests and cache semantics

Keep `Frame::request` synchronous and pure. Preparation orchestration lives
in `Turn`, with small agent-private projection/request helpers. Reuse
`Backend::for_model`, `transport`, and `stream` for provider routing,
gateways, OAuth, Bedrock profiles, and streaming termination. Do not call a
provider SDK directly or hand-build provider JSON.

The question call is a **stateless auxiliary call to the main model**, not
an assistant message in the ordinary conversation. Render the effective
text-only context with existing text readers and explicit role/tool/source
labels into one user message, followed by the preparation instruction. This
auxiliary rendering carries no protocol tool-call/result blocks or signed
reasoning without their provider context, and offers no tools. It does not
modify the normal frame. Give it a stable auxiliary cache namespace and,
for Cursor, a distinct conversation UUID rather than the live conversation
ID. Its output is not streamed as a final answer and does not create a
phantom user turn on replay. The vision call is another stateless auxiliary
request with no tools and no recursive fallback. Its system prompt includes
any provider-required auth identity through existing baseline rules, but
not the full agent baseline.

The normal session request remains previous projection plus appended notes.
Auxiliary requests deliberately do not promise the normal conversation's
append-only cache prefix: removing tools and asking a private question has
a distinct request shape. This costs an extra main-model read. It is not a
second mutable conversation, a reason to randomize the main cache key, or
permission to rewrite source history. Do not reuse `continuation` unchanged:
it retains tool definitions and has summary-specific retries/validation.

For image-only user input, the main can ask what the image depicts and which
readable details matter. Empty text, output containing tool calls, incomplete
transport output, or a truncated unusable question is a preparation error,
not permission to invent a question locally. A single bounded “question
text only” retry is allowed within existing request retry limits; world
tools are never dispatched from it.

### 5.3 Limits and failures

Validate content and size at image ingress, not only in the UI. Keep the
existing attachment size ceiling; check MCP base64 decoded-size bounds
before allocating, and validate image content rather than trusting an
extension or MIME label. Bound question/answer output using existing tool
output limits and provider output options; spill once at append as existing
model-facing output does, with an honest truncation/retrieval marker.

Scrub known secret text from the main's question before sending it to the
fallback; the note records the actual sent question, not an unsanitized
copy. Do not truncate a question into a different task: retry within the
bound or report that it is too long.

Honor the selected fallback's catalog context/output limits. If a related
image group does not fit, split deterministically into fitting groups and
ask the main a question for each, then let it combine the observations.
Never silently drop an image or resize it in a way that loses the requested
detail. An individually unsupported/oversize image fails explicitly.
Unknown provider aggregate limits can still reject a request; surface that
error rather than introducing a speculative retry/routing chain.

Reuse the existing cancellation token, transient retry budget, stream-idle
handling, and bounded auxiliary-call timeout (`SUMMARISE_TIMEOUT`), with
vision-specific error labels. Do not spawn a detached task like titles.
Count time spent preparing against the owning turn/child deadline. User
cancellation stops preparation and prevents further dispatch; a response
arriving after cancellation cannot append a success note.

An error note names affected references and the selected provider/model:
missing auth, provider rejection, unreadable image, no usable question,
empty answer, timeout, or size failure. The main is told it has **not** seen
those images and must explain the limitation rather than guess. Do not
send the original images to the text-only main model as an error fallback.
A store write failure stops the turn: an uncommitted observation cannot be
used as if it were durable.

Failures suppress reattempts for that occurrence within the current turn,
including todo continuations, not forever. A later explicit user turn may
retry; startup/resume alone never initiates inference. Successful observations
are reused across turns, regardless of later fallback selection changes.
There is no automatic caption refresh or model-driven reinspection tool in
this proposal. Reattaching an image creates a new occurrence and consultation.

## 6. Persistence, replay, and visibility

Add one nullable agent-owned `vision` metadata column to `messages`, using
the next available migration number. Existing messages default to null.
For a consultation note, store a small versioned record with source references
and fingerprints, actual question-model ID, actual fallback-model ID, and
outcome (`answered` or `failed`). Question and answer text live once in the
message body; no duplicate image bytes or generic metadata bag.
Use per-source outcomes when a batch contains both answers and failures.
Record the selected fallback ID even when dispatch fails; absence of a
question is explicit when the main question call itself failed.

One atomic store operation appends the user-role note and its metadata,
touches the session, and bumps context revision. A normal user message
quoting “Vision observation” cannot mark an image handled: only the typed
metadata does. Use a per-turn in-memory failure set for retry suppression;
no durable work queue or “running” row is needed.

Load metadata alongside the active frame. A successful observation counts
only while its note is effective and matches the source occurrence. Honor
cut visibility and masks in every reader. Fork copies applicable metadata;
committed cuts/delete clean it up with its message. A crash between a
provider reply and commit can repeat the paid request on the next user run;
no exactly-once provider claim. A crash after commit reuses the note.
Malformed non-null metadata fails closed for observation reuse with a visible
store error; it never turns a different source into an answered image.

The small metadata addition is intentional. Encoding completion in prose
would let quoted text forge it; persisting an assistant question followed by
a synthetic user answer would also change `store::is_turn_start`'s role-based
turn boundaries. The single user-role note avoids adding a synthetic
assistant turn, but **manual maintenance can still append after an actual
assistant reply**. Make turn-boundary readers explicitly exclude typed
vision-note rows (undo/redo, edit/regenerate, fork, history turn handles),
and skip those rows when locating the preceding conversational role. Do not
rely on role adjacency or a `<vision>` prefix. Retain ordinary input/steer
semantics and add a regression check for manual harvest after an assistant.

For `/shake`, original coordinates always come from raw rows plus masks.
Removing an image forbids future forwarding of that occurrence but does not
erase an independently stored observation, summary, or later answer, matching
48's “not semantic forgetting” contract. The confirmation should mention
vision observations among derived content that may remain. Removing the
observation while retaining the image makes it pending again on the next
user turn. No archived image is fetched to satisfy an old note.

Reuse transcript note/card conventions. Expose consultation metadata on the
relevant history entry and publish one agent-owned progress event for
`questioning`, `inspecting`, and terminal completion/failure, with session,
turn, source references, and selected model; no binary payloads or prompts
in diagnostic logs. TUI and shared React render progress in the existing
status area and an expandable question/answer observation in history.
Reconnect reloads the durable note rather than treating missing events as
completion. Headless/print/channel frontends must not mistake auxiliary
question text for the assistant's final reply.

## 7. All image paths and lifecycle interactions

### 7.1 Normalize producers, intercept once

- **Attachments and steering:** existing validation and binary persistence
  stay ahead of preparation, including queued/replayed input. A steer that
  arrives during inspection waits at the existing batch boundary; its images
  are prepared before the next ordinary call.
- **Direct tool output:** preserve `ToolOutput.files` and tool response text;
  the shared request preparation sees the persisted binaries. No fallback
  logic in browser/computer/artifact tools.
- **Python:** collect files from every completed inner `Invoke` before
  `tool_result` consumes the output. Return them on the outer cell result,
  including a cell that later raises an exception. Preserve invocation and
  file order and distinct screenshot occurrences; do not deduplicate only
  by path because a tool may overwrite that path. Snapshot mutable output
  files when the inner result arrives, not after later inner calls overwrite
  them. Keep Python's text/structured return values unchanged. Every cell
  completion/error exit drains its own collection; no image leaks into the
  next cell. Cancellation starts no further inference.
- **MCP:** decode actual MCP image blocks to bounded image files using the
  existing tools-owned data-directory/output path convention. Remove image
  base64 from textual and model-facing structured projections; keep ordinary
  text/structured results. Preserve source block order in file labels.
  Malformed image blocks produce an explicit tool error, not raw JSON
  base64 fed to the main model. Never regex-scan arbitrary tool text for
  alleged images. URI-only resources are not fetched by this adapter; a
  resolved image must first enter the normal attachment contract.
- **Historical URL binaries:** pass an active image URL only as the existing
  provider image source, never as a new harness-side fetch with arbitrary
  local-file/network access. Warn in implementation notes that remote bytes
  can change; new uploads and normalized tool images use stored bytes.
- **Future plugins:** any image represented through the existing binary/tool
  files contract is covered without registering a new interceptor. An
  arbitrary path mentioned in prose is not an attachment.

Fixing Python/MCP normalization also improves the native-vision path. That
is lossless transport repair, not running the fallback layer on that path.

### 7.2 Compaction, harvest, titles, and model changes

| Operation | Required behavior |
|---|---|
| Automatic summary compaction | Prepare pending images before the fold; build the summary continuation from the same text-only projection plus observation notes. Remove the current separate blind `strip_images` call as the summary policy. |
| Manual summary/harvest | Use the same preparation under the claimed session state and cancellation scope. Image-derived evidence may inform the summary/harvest; no recursive consultation from inside `Frame`. |
| Overflow | Do not recursively compact inside preparation. If its main question request cannot fit, record the explicit failure; existing compaction recovery can then proceed with that limitation visible. No successful-inspection claim. |
| `/compact cut`, `/clear`, `/shake` | Explicit lossy/removal operations perform no new vision call solely to preserve content the user is discarding. |
| Snap | Keep current eligibility: fallback does not make a text-only main model eligible to create snap heads. An existing snap head loaded after switching from a vision model contains images and must be inspected like any other active head. |
| Title generation | The existing digest is text-only. Do not add images or invoke fallback for a title. Titles can use the main answer; no extra title workflow. |
| Model switch | Re-evaluate `Current::vision` at the next request boundary. Vision → text prepares any active images without observations; text → vision sends original active images and performs no further fallback. Existing notes remain history. |
| Mid-consultation switch | An already dispatched provider call may complete for its captured model; do not dispatch the next auxiliary call if the now-current main model supports vision. Commit any completed valid observation at most once, then branch afresh. |
| Fallback change/off | Selection changes affect the next consultation. Turning Off while a question is being prepared prevents a not-yet-dispatched vision call. Requests already sent cannot be recalled; they remain cancellable with the turn. |
| Native child | Its own selected main model determines the branch; same harness-wide fallback selection, same permit/deadline. `agent.communication: false` does not disable this internal model adaptation. |

Preparation appends notes, so compute compaction's `keep_from` and token
estimates **after** preparation, not with stale indexes. Do not consult a
note's own text as another image source. Auxiliary token usage is added to
turn usage, but never overwrites the main-context meter, main cache-hit
baseline, or `state.used`. Only the main request's projection counts toward
its estimated context; provider usage remains authoritative when supplied.
Missing auxiliary usage is unknown, not zero-cost proof. If 51's call ledger
lands, give these requests distinct question/vision purposes there; do not
implement a parallel cost ledger in this feature.

## 8. Trust and disclosure

The vision model is an interpreter, not an agent. It gets no tools, shell,
network-fetch capability, writable memory, subagent authority, or approval
handle. Its instruction says to answer the supplied visual question, quote
observed text as data, distinguish inference, and report unreadable details.
Instructions embedded in pixels are not instructions to wth.

Label the returned note as image-derived, fallible evidence, never a system
message or user authorization. Retain source provenance and scrub known
secret text before UI/model output using the existing redaction facility.
Do not expand `{{secret:...}}` in the main-generated question. Ordinary
provider authentication stays out of prompts and logs.

Configuring a fallback explicitly selects another inference destination.
The UI must disclose that screenshots may contain private information and
that text redaction cannot remove secrets embedded in pixels. This uses the
same consent model as selecting the main/title provider, not a second
approval policy. Subsequent world tools still take `Invoke` and its gate.

28's proposed taint/sink enforcement is **not currently implemented** in the
inspected `ToolSpec`, `ToolOutput`, or gate. Do not claim these labels prevent
prompt injection or that the current gate tracks image taint. If 28 lands,
image observation provenance must feed that same mechanism; no competing
vision-only trust engine.

## 9. Implementation map and rejected alternatives

Implementation should proceed in this order; this document changes no code:

1. **Setting and capability contract:** `agent/src/{lib,supervisor,backend,loop_}.rs`,
   `rpc/src/api.rs`, first-run config in `wth/src/main.rs`. Reuse catalog
   metadata; unify effective capability for the loop and `ModelInfo`.
2. **Lossless ingress:** `agent/src/repl.rs`, `tools/src/mcp.rs`, existing
   attachment/file helpers. Prove images reach durable user/tool binaries
   before adding inference.
3. **Occurrence projection and replay:** `agent/src/{context,store,frame}.rs`
   plus one migration. Original-coordinate index, typed note metadata,
   atomic append, fork/cut/removal behavior. Keep raw history unchanged.
4. **Consultation:** a small `agent/src/vision.rs` module if it keeps the loop
   readable; `Turn` owns sequencing, cancellation, errors, permit, and usage.
   Integrate `round`, `compact`, `harvest`, and model-switch boundaries.
5. **UI parity:** `tui/src/{model,lib,ui}.rs`; shared React General, slash
   commands, progress/history, query keys, and attachment warning. Extend
   `desktop/ui/src/rpc/api.ts`'s hand-maintained interface **and** `METHODS`.
   `just ts-desktop` regenerates/copies Rust wire bindings and method names;
   it does not generate those TypeScript method signatures.
6. **Docs at implementation:** amend 00's agent/config sections, 02's
   capability semantics, 07's frame/auxiliary-call rules, 08's Python file
   propagation, 17's text-only behavior, 23's tool image path, 25's parity
   table, 48's derived-content warning, and README/config examples. Mark
   implemented only after recording the checks below.

Rejected:

- **Generic caption before the main model participates:** misses task-specific
  OCR, geometry, comparison, and uncertainty questions.
- **Optional `inspect_image` tool:** cannot guarantee every image is routed;
  excludes main models without tool calling. A mandatory tool-call barrier
  adds unrelated dispatch/final-answer suppression machinery.
- **A vision subagent/peer:** gives a read-only translation tools and a
  lifecycle it does not need, and incorrectly couples images to the
  communication switch.
- **Calls inside `Frame::request` or a provider adapter:** puts asynchronous
  state into a pure projection, makes auxiliary calls recurse, and misses
  lifecycle ownership.
- **Rewrite source images to captions:** loses native vision after model
  switches and invalidates the cached prefix when captions arrive.
- **Per-tool fallback implementations or frontend inference:** duplicate
  routing and miss replay, head images, Python, or MCP.
- **Always add a vision prompt/tool even for vision-capable mains:** violates
  the bypass requirement and changes otherwise unrelated request prefixes.
- **In-memory-only completion or prose markers:** restart repeats work or
  quoted text forges state. One nullable typed column is enough.
- **New dependency, OCR stack, prompt router, configurable strategy list,
  background captioner, or global content cache:** no present requirement.

## 10. Acceptance and verification plan

These are implementation gates, **not checks run for this design document**.
Use a synthetic image with a fact that is absent from filenames, user text,
and tool text. Capture test-provider requests at the transport seam; merely
seeing a plausible final answer is not proof that routing was correct.

| Gate | Observable proof |
|---|---|
| Native bypass | Image-capable main receives original image parts; fallback request count is zero even with a broken/unavailable fallback configured. No fallback prompt/note added. |
| Main-authored question | Text-only main sees references, no binary/base64 image; its captured question after the specified secret scrub is exactly the text sent with those images to fallback. Normal main call then receives the attributed answer and recovers the image-only fact. |
| No tool-call dependency | Repeat with a text-only main offering no tool calling; consultation still works. Auxiliary tool calls are never dispatched. |
| Input parity | TUI attachment, desktop/mobile upload, steering/queued input, and inbound ACP image each exercise the same server preparation; image-only input also works. |
| Tool parity | Direct native screenshot, screenshot inside Python, Python cell error after screenshot, multiple screenshots reusing one filename, and MCP image block all survive as distinct correct images. Raw MCP image base64 never enters a main text request. |
| Multiple/head images | Related images retain labels/order; all pending occurrences are attempted, including snap-head images and images already present on resume/model switch. Split groups do not silently lose images. |
| Capability validation | Known text-only, unknown/no metadata, Cursor with a vision-capable catalog sibling, configured gateway, explicit user override, ambiguous ID, and unavailable provider follow §§3–4. |
| Settings | Set/off/clear/restart through TUI and shared General. Invalid selection and failed config persistence leave live selection unchanged. An unavailable saved selection remains visible. Verify desktop and a narrow/mobile viewport with keyboard/touch controls. |
| Stable prefix | After observation arrives, earlier text-only projected messages are byte-identical; answer is appended. Auxiliary request IDs/cache keys do not perturb ordinary main caching. |
| Replay and coordinates | Resume/fork reuse committed observations; partial `/shake` does not shift references to a different image; undo plus reused seq with different bytes cannot reuse stale observations. Masked images are never forwarded. Manual harvest's vision note creates no phantom turn and does not hide the next real user turn. |
| Compaction | Summary and harvest consume observations, not silent omission; compaction indexes are computed after appended notes. Existing snap-head inspection works; clear/cut/removal triggers no preservation calls. |
| Error/cancel | Missing auth, malformed image, size rejection, empty question/answer, provider error, timeout, store failure, and cancellation are explicit. No repeated failure loop within one turn, post-cancel write, detached call, or raw-image leak. |
| Switches | Main/fallback changes between question and image dispatch honor the boundary rules; communication Off does not block fallback. |
| Privacy and accounting | Fallback receives selected images and question only, no tools/full frame. Both auxiliary calls contribute reported usage without changing the main context meter or cache baseline. Question text is not a user-facing final answer. |

Keep focused regression checks for routing, producer normalization, stable
projection/original-coordinate matching, and persistence/cancellation races.
Existing `wiremock` provider fixtures and store test conventions suffice;
no new test framework. Use a throwaway live run for provider integration,
then actual TUI and shared desktop/mobile UI interaction. Remove synthetic
attachments/scripts after the run. Report provider/transport or packaged-iOS
paths not actually exercised rather than calling them verified.

### Design-document verification

This proposal was grounded in the source paths/symbols in §2 and the design
invariants in §1. It does not claim that the fallback exists, that model calls
were made, or that an implementation/UI test passed. The current-source holes
(Python/MCP image loss, Cursor metadata mismatch, and projected-coordinate
loss) are required implementation work, not silently assumed infrastructure.

Document checks: all 16 relative design links resolve, all 17 explicitly
checked implementation paths exist, and Markdown code fences are balanced.
The architecture document links this proposal as unimplemented. No build or
runtime/UI tests were run for this documentation-only change.
