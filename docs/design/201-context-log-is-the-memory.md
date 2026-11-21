# 201 — Context: the log is the memory (supersedes 137)

Status: design v1 + revisions 1 and 2 (Sky's answers of 2026-11-20 23:53 and the UI requirement of 2026-11-21, §16) · nothing built · 2026-11-20 · written from a review of 137 as built at `5b88548ff` (branch `fix/oauth-usage`, dirty tree). Supersedes **137 — Endless** as the design of record for the summary-tree memory mode. 137, 137F, 137G and the 137 as-builts stay as the record of what runs today; where this document and 137 differ, this document is the target. Product name stays **Fractal context**; internal identifiers keep `endless`. Extends 07 (frames, epochs, compaction), 11 (compaction styles), 52 (vision fallback), 136 (dreaming), 140 (autoresearch).

## The ask

Sky, 2026-11-20, in order, verbatim:

- "I'm wanting to move away from default regular context windows in general and just move to this endless model by default, and have the regular context window management stuff just be legacy."
- "how are images handled in it, what happens if an image goes to a summary model?"
- "is there a way we could improve the processing algorithm so that it's got a smoother pace and would work better with really tiny models for summarization."
- "yeah I think that we need a tail of context that is verbatim too. so it eventually get's compressed, not immediately compressed."
- "and we want it to be balanced to, not breaking prefix cache if possible."
- "the forever solution for context management for our current generation of LLMs and how they work."
- "and I don't mind maintaining the legacy model as a fallback or just for people who want it."

## 0. Decisions in one screen

- **D1 — The log is the memory; the window is a cache over it.** The session's `messages` rows are the truth. Everything the model sees is a view the harness composes from that log: a tail, a ledger, a summary view. Nothing is deleted, only demoted, and everything demoted leaves an address and a hint.
- **D2 — Frame.** `tools · system · ledger snapshot · view · tail · input`, ordered by how rarely each part changes (§3).
- **D3 — Verbatim tail.** The newest messages are carried as the real messages, exactly as they were sent, append-only. It is sized either in fixed bytes (`tail_low_bytes` 24,000 / `tail_high_bytes` 64,000) or as a share of the session model's allowance (`tail_low_share` 0.10 / `tail_high_share` 0.25); `tail_mode` picks which (§4.2). This replaces 137 D4 ("fresh turn, nothing carried").
- **D4 — One planned cache break: the trim.** The tail is trimmed only past the high mark, down to the low mark, in whole turns. The fold runs only on a trim. A trim is taken early when the provider cache is already cold. Between trims every request extends the previous one byte for byte (§4.3).
- **D5 — Draft line first, model line later.** Every unit gets a deterministic line when it is ingested. The tree is always complete. A model failure costs line quality, never progress. `settle` and its 45 s wait are removed (§5).
- **D6 — The tail is the compactor's deadline.** A line is needed only when its message leaves the tail. One steady queue, oldest tail unit first, woken when a message lands; merges are built lazily (§6).
- **D7 — Two compactor profiles.** `standard` (137G as built, with fixes) and `small` for tiny or local models: one unit per call, a short prompt, about 2 KB of nearby context, a short reply, chunks sized from the window, no correction dialogue (§7).
- **D8 — Use a model only where it helps.** Tool calls are templated, tool output is extracted, user text is truncated rather than paraphrased; only assistant text and subagent reports need the model (§7.3).
- **D9 — Artefacts have handles.** Images, PDFs and tool pictures get a unit with a name and, on leaving the tail, a one-line description. They ride the tail as real binaries; `zoom` re-attaches them (§8).
- **D10 — A state ledger.** A small structured record of what is true now (goals, decisions, constraints, open tasks, files touched), updated by appended changes and rewritten only at a trim. The session model writes it through a `ledger` tool; the summary model's flags appear as unconfirmed suggestions (§9).
- **D11 — Age by use, prefetch at the end.** A zoomed line stays unmerged at the next trim; recall hits for the new input are appended after the tail (§10).
- **D12 — Tools follow the same rule.** Full schemas for a few direct tools at the top, a one-line index for the rest, detail fetched into the tail. Tool-list changes are batched to a trim; an unavailable tool refuses rather than disappears (§11).
- **D13 — Default and legacy.** `agent.endless.default_chat` becomes `true` when Sky decides, on the evidence of §13. `epochs` stays a supported mode: a user's choice per session or agent, and what `/endless off` returns to. It is not removed (§12).
- **D14 — No replay benchmark for now.** Verification is the test gates, the numbers the engine logs on every turn, and Sky's own use. The replay benchmark is recorded as deferred, not designed away (§13).
- **D15 — Every new option has a control in the UI** (§12.2): the Context settings page, the chat's Memory panel for per-session values, the agent page for per-agent ones.

## 1. Why: what the review of 137 found

Every row was read in the code on 2026-11-20 unless marked.

| # | Finding | Effect | Where |
|---|---|---|---|
| F1 | No verbatim tail. Once the compactor has run, even the newest message is shown only as its ≤ 512-byte line. Verbatim rendering applies to *unbuilt* units of ≤ 2,048 bytes. | A faster compactor means less short-term fidelity. The model must `zoom` to read its own last reply. | `crates/agent/src/endless/fold.rs#L55-L90`; 137 VIEW_DOC, `docs/design/137-endless.md#L241-L255` |
| F2 | Each turn clears the carried history. | Everything cached during the previous turn is discarded; only the view prefix is read back. | `crates/agent/src/loop_.rs#L4729-L4731` |
| F3 | A long turn is still a classic window. | In-turn overflow falls back to the 07 §8.5 cut. | 137 D11 |
| F4 | Every binary part becomes the text `[image]`, PDFs included, with no name, type or description; a non-assistant binary is kind `user`. | The tree cannot say what a picture showed. | `crates/agent/src/endless/units.rs#L47-L55` |
| F5 | Tool pictures travel in `ToolResponse.images`; nothing under `endless/` reads that field. | A screenshot or a `read_file` PNG leaves no unit at all. | `crates/agent/src/loop_.rs#L7341`; `units.rs#L47` |
| F6 | `zoom` returns the unit's text. | A picture cannot be brought back. | `crates/agent/src/endless/pump.rs#L2397-L2403` |
| F7 | The vision-fallback note is built as a user message. | A model's description is filed as the user's own words (filing per the investigator's trace of `units.rs#L33-L39`). | `crates/agent/src/vision.rs#L673-L724` |
| F8 | The batch path has no thinking check and no retry. | An empty reply marks every unit of the batch refused; each reruns alone (`built=0 solo=N`). | `crates/agent/src/endless/compactor.rs#L753-L805` |
| F9 | After `tries` attempts with no passing line, the shortest failing candidate is stored (`tries` negative). | A line that failed the copy or grounding guard becomes memory. | `compactor.rs#L929-L941` |
| F10 | A failing level-0 unit pins the frontier; merges wait behind it. The mechanical fallback needs 3 hard failures, 10 minutes and another success. | Head-of-line blocking; transient errors never count as hard. | `pump.rs#L78-L100`, `#L1310` |
| F11 | Requests are sized for large models: a 3,529-byte prompt, up to 24–48 KB of `<chat>` context, batches that can approach 48 KB, map chunks of 16 KB regardless of window. | A 4k–16k-window model cannot fit a batch or a chunk. | `pump.rs#L1183`, `#L1294`; `compactor.rs#L215-L217` |
| F12 | Nothing wakes the pump when a message is appended. It wakes on a job finishing, on `settle`, or on a timer of up to 60 s. | Work bursts at the next turn start behind a wait of up to `settle_secs` (45). | `pump.rs#L1157-L1175`, `#L2069-L2110`, `#L2126-L2158` |
| F13 | Merges are eager and carry the frozen context. | About one merge call per unit, whether or not the view needs it. | `pump.rs#L1300-L1322`, `#L1458-L1471` |
| F14 | The design's own smoke "asserts structure, not line quality". No comparison with `epochs` on task success was found. | Every tuning choice so far is taste. | `docs/design/137-endless.md#L340` |

The warning "used its whole 4096-token reply on thinking" is the local single-unit retry at twice `LOCAL_MAX_TOKENS` (2,048): `compactor.rs#L830-L832`, `#L1032`.

## 2. Facts relied on

- Units are derived deterministically from `messages` rows; rows are appended per message, so they exist mid-turn (`pump.rs#L1101-L1155`; `crates/agent/src/store.rs#L2044`).
- A unit that fits `node_bytes` (512) is its own line with no model call; two children that fit together merge for free (`crates/agent/src/endless/tree.rs#L177-L186`).
- A deterministic line already exists: the mechanical fold, head ⅔ + ` … ` + tail ⅓ of the tagged unit (`tree.rs#L188-L189`, used at `pump.rs#L1424-L1431`).
- A size-only guard trip is cut at a word boundary and accepted (`compactor.rs#L621-L634`).
- Direct tool specs are sorted by name; `zoom`, `recall`, `date` are added only while the session has a tree; a changed system prompt is logged as a rebase (`loop_.rs#L5098-L5144`).
- On the Anthropic transport the system block (1 h) and the newest message (5 min) take two of the four cache breakpoints; the view may use `VIEW_MARKS` = 2. OpenAI-like transports get `prompt_cache_key` and cache on the stable prefix (`crates/agent/src/frame.rs#L275-L279`, `#L290-L323`).
- The mode of a new session is the explicit option, else `agent.endless.default_chat` (false today); desks default to endless (`crates/agent/src/supervisor.rs#L13080-L13088`; `crates/agent/src/endless/mod.rs#L101-L102`).
- oMLX reuses an end-of-prompt snapshot only when an earlier prompt is resent byte-identical; a sliding window falls back to 2,048-token blocks (knowledge `facts/omlx-prefix-cache-2048-blocks-hybrid-snapshots`).

## 3. Shape

```
[tools]            direct tools, full schemas, sorted            changes: at a trim, or never
[system]           instructions + one-line index of the rest      changes: at a trim, or never
[ledger snapshot]  what is true now                               rewritten at a trim
[view]             <chat> lines, oldest and coarsest first        rewritten at a trim
[tail]             the real messages, as sent                     append-only between trims
[input]            message · prefetch · ledger changes · date     new every turn
```

A **generation** is the span between two trims. Within a generation the ledger snapshot and the view are frozen bytes: tree improvements, refined lines and merges land in the *next* generation's render. That is what makes every request a byte-for-byte extension of the one before.

Cache marks on Anthropic (four available): system (1 h), one deep view band, the end of the view, the newest tail message (moving, 5 min). 137 gave the view two marks and excluded its final block because that block grew every turn; here the view's end is stable for a whole generation, so it is the most valuable mark.

## 4. The tail

### 4.1 What it is
The messages since the last trim, carried in `state.history` exactly as they were sent: roles, tool calls, tool results, binaries, and each turn's input block with its per-turn items. Not a text rendering. 137's clear of `head` and `history` at turn start (`loop_.rs#L4729-L4731`) moves to the trim.

### 4.2 Budget
The low mark is the guaranteed verbatim context after a trim; the high mark is where a trim is due. `tail_mode` offers both sizings Sky asked for:

- **`fixed`**: `tail_low_bytes` (24,000) and `tail_high_bytes` (64,000). The same on every model, so sessions are comparable and the cost per turn is predictable. Both scale down together when the session model's allowance is too small to hold them (the existing `view_allowance` path).
- **`scaled`**: `tail_low_share` (0.10) and `tail_high_share` (0.25) of the session model's turn allowance, clamped to 8,000..=512,000 bytes. A large-window model keeps far more word for word; a small one keeps little and leans on the view.

The default is `fixed` (this document's choice, a one-line change). The mode is a setting, overridable per agent (`tail_mode:` in `agents/<name>.md`) and per session. The tail always keeps the last whole turn, even past the high mark.

### 4.3 Trim
```
at a turn boundary (before the new input is appended):
  cold = now − last_request_at > cold_after_secs        # the provider cache has expired anyway
  if tail > tail_high  or  (cold and tail > tail_low):
      cut = oldest whole turns until tail ≤ tail_low     # never the last turn
      every unit in cut has a line (the draft if no model line yet)   # §5: never waits
      fold(tree, T_cut); fit()                           # the only place the fold runs
      ledger snapshot = snapshot + changes since the last trim
      generation += 1; render ledger + view once; freeze
```
- **Cost.** Each trim rewrites the remaining tail, so rewritten bytes per new byte are about `low ÷ (high − low)`: 0.6 at the defaults. This is arithmetic, not a measurement; the trim log line of §13 reports the real figure.
- **In turn.** When one turn's frame passes 90 % of the model's allowance, the same trim runs at a round boundary, cutting the oldest whole rounds (a tool call and its results stay together). This replaces 137 D11's cut for endless sessions. Until it is built (§14, P5) D11 stays.
- **Restart.** After an engine restart the tail is rebuilt from rows. If the per-turn items of past input blocks are not stored with the row, that rebuild is one cache miss. Open check O1.

### 4.4 The view document
VIEW_DOC changes from "No message appears in full, not even the last ones" to: the lines under `<chat>` summarise everything before the messages that follow; the messages that follow are word for word; `zoom` and `recall` reach the rest.

## 5. Lines: drafts and refinement

- **At ingest** every unit gets a line with no model call: its own text if it fits (free, as today), else the per-kind deterministic line of §7.3, else the mechanical cut. The node is stored with `quality = draft`.
- **Refinement.** The pump replaces a draft with a model line (`quality = model`) when one passes the guards. A model line that fails the guards after its attempts is discarded and the draft stays: 137's "shortest failing candidate wins" (F9) is removed.
- **Merges** are built from whatever children exist. A merge with a draft child is itself `draft` and is rebuilt when a child is refined, lazily and only if it is still in a view.
- **Never blocks.** Because every unit has a line, there is no frontier to pin, no `settle`, no `stalled` state that holds a turn. `settle_secs`, the 2,048-byte verbatim rule and the folded-run lines of 137G become unnecessary for chats; `raw_bytes` is replaced by the tail budget.
- **No summary model at all.** A session with no reachable compactor runs on drafts. That is lower quality, not a failure, and it is why falling back to `epochs` is the user's choice rather than a necessity.

## 6. The pump

- **Queue.** One priority queue per session: (1) refine the oldest draft still in the tail, oldest first, because it leaves first; (2) refine drafts already in the view, most-viewed generation first; (3) merges the next trim will need.
- **Wake on append.** `Store::append` notifies the session's pump. Summarising runs during the turn; there is no burst at the next turn start.
- **Lazy merges.** A merge is built when the view passes 75 % of `view_bytes`, in the fold's own due order, instead of whenever two children exist.
- **Frozen compactor context per generation**, not per turn: every call between two trims starts with the same bytes.
- **Local endpoints.** The engine-wide queue per endpoint drains one session's run of units before switching sessions, so a single-slot server keeps its prefix. Whether interleaving evicts the prefix on a given server is not measured (O4).
- **Fixes carried over to `standard`.** The batch path checks for a reply spent on thinking and retries once with thinking forced off, as the single-unit path does (F8). A probe at compactor selection (`test_summary`, `pump.rs#L841-L944`) records a model that thinks unasked and refuses it with the existing hint.

## 7. Compactor profiles

### 7.1 Selection
`agent.endless.profile: auto | standard | small`. `auto` picks `small` when `compactor::is_local` is true or the model's known window is at most 32,000 tokens.

### 7.2 The two profiles

| | `standard` (137G with §6 fixes) | `small` |
|---|---|---|
| Units per call | up to `batch_units` (16), ≤ 24 KB | one |
| System prompt | `compact_prompt.txt`, 3,529 bytes | a new short prompt, target ≤ 700 bytes |
| Context | `<chat>` up to `compactor_ctx_bytes` (48 KB) | the last few lines of the same turn and the user's last message, ≤ 2,000 bytes |
| Reply cap | 600 tokens per line | 160 tokens, stop at the first newline |
| Oversized unit | map-reduce in 16 KB chunks | chunks of `(window − prompt − reply) × 0.5`, never above 4 KB |
| Guard trip | correction dialogue, up to `tries` | keep the draft; one retry later when the queue is idle |
| Output shape | free text, numbered lines in a batch | constrained (grammar or JSON schema) where the server supports it |

Expected request size for `small`: about 1.5–2 thousand tokens against roughly 8–10 thousand for a local call today. Both are estimates.

### 7.3 Per-kind lines (both profiles)

| Kind | Draft line | Model? |
|---|---|---|
| `tool` | `tool: <name> <key arguments>`, values cut to fit | no |
| `echo` | first line, error lines, counts ("412 lines, 3 errors"), last line | only when over 2 KB and not an error |
| `user` | verbatim, cut at a sentence boundary with the tail kept | no: the user's words are never paraphrased |
| `talk` | mechanical cut | yes |
| `work` | mechanical cut | yes |
| `note` | verbatim cut | no |

## 8. Artefacts and images

- **Units.** A binary part becomes `user: [image: <name>]` or `user: [document: <name>]` through the existing `binary_tag` (`loop_.rs#L9399`). Each entry of `ToolResponse.images` becomes an `echo: [image: <name>]` unit.
- **In the tail** binaries ride as real parts, so the model keeps seeing the picture for as long as its turn is in the tail.
- **Leaving the tail** an image gets a one-line description appended to its line: from a vision-capable compactor, else from the design-52 vision model, else none. The line records which.
- **Vision-fallback notes** (`<vision_observation>`) become kind `note`, not `user`, and are linked to their image unit.
- **`zoom(id, 1)`** on an artefact unit re-reads the row's binary and returns it as a tool image.
- **Empty captions.** An image-only message does not produce an empty `user:` unit.
- The summary model still never receives image bytes unless it is the describer above. A text-only summary model cannot break on an image.

## 9. The state ledger

- **What.** Five short sections: Goals, Decisions, Constraints, Open tasks, Artefacts (files and things touched, with their last known state).
- **Sources.** Open tasks from the `todo` tool's state and Artefacts from tool-call arguments are deterministic. Goals, Decisions and Constraints are written by the session model through a `ledger` tool (`ledger(section, op: add | close | replace, text)`), a direct tool in endless sessions. The summary model also flags `user` lines that read as a decision or constraint; those appear in the snapshot under "Suggested (unconfirmed)" with the user's words and the line's id, and the session model accepts or drops them with the same tool. An unconfirmed suggestion that is neither accepted nor dropped lapses after two trims.
- **Updates** are appended to the turn's input block as `<ledger>` change notes (which are no units, like `<memory>` notes). The full snapshot is rewritten only at a trim.
- **Budget.** `ledger_bytes` 4,000. Over budget, the oldest closed tasks and superseded decisions move to the log; `recall` reaches them.
- **Promotion to the knowledge base is out of scope** (Sky: "Not yet"). To be decided once the ledger exists and has been seen in use.

## 10. Ageing by use, and prefetch

- **A zoom is a page fault.** The zoomed content is already back in the tail as a tool result. At the next trim the fold leaves a zoomed line unmerged for one generation. The fault rate per generation is logged and is the signal for how hard to fold.
- **Prefetch.** Before a turn, `recall` runs over the new input's rare words and up to `prefetch_bytes` (2,000) of hits are appended to the input block after the tail. No prefix changes.

## 11. Tools

| Tier | Where | Changes the cache? |
|---|---|---|
| Direct tools, full schema | top of the request, sorted by name | yes, everything |
| Every other function | one signature line in the system prompt | yes, everything after it |
| One function's full parameters | fetched with `help`, lands in the tail | no |
| Skills | one line in the index; text loaded into the tail | no |

- **Batch tool-list changes.** A tool that appears mid-generation (an MCP server added) is announced in the input block and callable through the runtime; it joins the top-of-request list and the index at the next trim or on a cold cache.
- **Refuse rather than remove.** `zoom`, `recall` and `date` stay defined for every endless session from creation and return a plain refusal while there is no tree, instead of joining the list when a backfill finishes (`loop_.rs#L5125-L5142`).

## 12. Default, legacy, migration

- **Default.** `agent.endless.default_chat: true` when Sky decides (§13). It is read only at creation, so existing sessions keep their stored mode.
- **Legacy.** `epochs` remains fully supported: selectable per session, per agent (`memory: epochs`) and as the configured default. `/endless off` still writes the view as an epoch head. The 07 §8.5 cut stays for `epochs` sessions and, until P5, for in-turn overflow in endless ones.
- **Existing endless sessions** start their first generation with an empty tail and fill it; nothing is backfilled.
- **New config and its controls:** §12.1 and §12.2.
- **Retired for chats:** `settle_secs`, `raw_bytes`. `jobs`, `batch_units`, `compactor_ctx_bytes`, `tries` apply to `standard` only.

### 12.1 Config keys (`agent.endless`, all optional, applied live like the existing ones)

| Key | Default | Range | Meaning |
|---|---|---|---|
| `tail_mode` | `fixed` | `fixed` \| `scaled` | how the tail is sized (§4.2) |
| `tail_low_bytes` | 24,000 | 8,000–256,000 | fixed mode: verbatim context kept after a trim |
| `tail_high_bytes` | 64,000 | 16,000–512,000 | fixed mode: where a trim is due; always above the low mark |
| `tail_low_share` | 0.10 | 0.02–0.40 | scaled mode: low mark as a share of the session model's turn allowance |
| `tail_high_share` | 0.25 | 0.05–0.60 | scaled mode: high mark; always above the low share |
| `cold_after_secs` | 0 | 0–86,400 | idle time after which a trim is taken early; 0 = the transport's cache lifetime (300 on Anthropic) |
| `profile` | `auto` | `auto` \| `standard` \| `small` | compactor profile (§7) |
| `merge_at` | 0.75 | 0.50–1.00 | share of `view_bytes` at which merges start being built |
| `ledger` | true | bool | the state ledger and its tool (§9) |
| `ledger_bytes` | 4,000 | 1,000–16,000 | ledger snapshot budget |
| `ledger_suggestions` | true | bool | summary-model flags shown as unconfirmed suggestions |
| `prefetch_bytes` | 2,000 | 0–8,000 | recall hits appended to the input; 0 = off |
| `age_by_use` | true | bool | a zoomed line stays unmerged for one generation |
| `describe_images` | `auto` | `auto` \| `off` | one-line description when an image leaves the tail |

An invalid pair (high not above low) is refused by `endless_settings_set` with a message naming both values. Per-agent overrides in `agents/<name>.md`: `tail_mode`, `tail_low_bytes`, `tail_high_bytes`, `tail_low_share`, `tail_high_share`, `profile`. Per-session overrides are stored on the session row beside `endless_view_bytes`: the tail mode and its two marks.

### 12.2 UI (Sky, 2026-11-21: "just make sure there are settings in the UI to control these new options.")

Every key of §12.1 has a control. The quality bar is the existing Context page's.

- **Settings › Agent › Context** (`crates/desktop/ui/src/components/settings/Memory.tsx`), new and changed sections:
  - **Tail** (new, directly under View): *Tail sizing* (select: Fixed size / Scaled to the model). Fixed shows *Kept after a trim* and *Trim at* in KB with the token estimate the View row already shows; Scaled shows the same two as percentages, with the resolved KB for the current default model beside them. *Trim early when idle* (seconds; empty = automatic). The pair is validated in place: the high mark cannot be set at or below the low one.
  - **View**: *Start merging at* (percent of the view size). The *Settle seconds* row is removed (the key stays accepted in config for compatibility).
  - **Compactor**: *Profile* (select: Automatic / Standard / Small), with the resolved profile and its reason named beside it ("Automatic · small, because the model is local"). *Describe images* (switch).
  - **Ledger** (new): *Keep a state ledger* (switch), *Ledger size* (KB), *Show summary-model suggestions* (switch).
  - **Recall** (new): *Prefetch size* (bytes; 0 turns it off), *Keep zoomed lines unmerged* (switch).
- **The chat's Memory panel** (`components/chat/MemoryPanel.tsx`): shows the live numbers (tail bytes against its two marks, the generation, lines still on a draft, the resolved profile) and carries the per-session tail override next to the existing per-session view size.
- **The agent's page and hire card**: the per-agent keys above, wherever `view_bytes` is already offered.
- **Wire**: `EndlessConfig` and the session-memory status type gain the fields; the TypeScript bindings are regenerated (`just ts-desktop`), never edited by hand.
- TUI: `/endless` status prints the same live numbers; no settings screen there.


## 13. Verification

Sky, 2026-11-20: "no replay benchmark for now." So there is no measured comparison with `epochs` in this design; finding F14 stands. What verifies each phase instead:

- **Test gates.** `cargo test -p agent` (the endless suite), `-p rpc`, `-p roster`, UI checks, as in 137 §14. New unit tests per phase: the trim never splits a turn or a tool call from its results; a request within a generation is a byte-for-byte extension of the previous one; every unit has a line at ingest; a guard-failing line is never stored; each binary shape (user image, PDF, tool picture, image-only message, vision note) yields the unit of §8.
- **Numbers the engine logs.** The `view_rebase` line already carries `changed_from_byte`, `blocks_hit`, `cache_read`, `cache_write` and `prompt` once per turn (`crates/agent/src/loop_.rs#L4880-L4899`). It gains `generation`, `tail_bytes` and `trimmed`. A new `endless: trim` line reports the bytes cut, the bytes rewritten and whether the cache was cold. The pump's batch line gains `drafts` (units still on a draft line) and the age of the oldest draft in the tail. A zoom logs the line's level and generation.
- **Sky's own use.** Each phase lands behind the existing per-session switch; `default_chat` stays `false` until Sky flips it.

What to read in those numbers: cache-read share of prompt tokens between trims (expected near the classic mode's), rewritten bytes per new byte (expected about 0.6 at the fixed defaults), zooms per turn (a rising rate means the fold or the tail is too tight), drafts leaving the tail unrefined (the compactor is not keeping up).

**Deferred: the replay benchmark.** Replaying real sessions with probe questions whose answers are in the log remains the only way to show 201 is at least as good as `epochs` on task quality, and the only number an autoresearch study could tune the tail, line size and fold against. Not built now; nothing in this design depends on it.

Toy model of the expected shape (10 KB of messages per turn, 2 KB of lines per turn, `epochs` compacting at 400 KB to 20 KB, fixed tail 24/64 KB), bytes carried into the turn summed over 48 turns: `epochs` 8,550 KB, 137 as built 2,256 KB, 201 3,843 KB. Illustrative only.

## 14. Build order

| Phase | Content | Gate |
|---|---|---|
| PC | Contract: the config keys of §12.1 with clamps, the wire fields, per-session and per-agent overrides, regenerated bindings | tests; bindings compile |
| PU | The UI of §12.2 (runs beside P1–P6 once PC has landed) | UI type-check and unit tests |
| P1 | Draft lines at ingest; remove `settle` and failing-candidate acceptance; batch thinking check; wake on append | no turn waits on the compactor; tests |
| P2 | Tail (both modes), trim, generations, cache marks, new VIEW_DOC, the log lines of §13 | byte-extension test; cache-read share visible in the log |
| P3 | `small` profile and per-kind lines; lazy merges; local session affinity | a 1–4 B local model keeps up with a live session on this machine (drafts in the tail stay near zero) |
| P4 | Artefact units, descriptions, `zoom` re-attach, vision-note kind | the binary-shape tests; a screenshot is recoverable by `zoom` two generations later |
| P5 | In-turn trim (retires D11's cut for endless) | a 200-tool-call turn completes without the 07 cut |
| P6 | Ledger and its tool, suggestions; ageing by use; prefetch; tool-list batching | tests; the ledger survives a trim and a restart |
| P7 | `default_chat: true` | Sky's decision |

Builds follow the shared-machine rules: capped parallelism, one workspace build at a time, no model-loading run while Sky is at the keyboard.

## 15. What 137 said that no longer holds

| 137 | 201 |
|---|---|
| D4 fresh turn: nothing carried | the tail is carried; the view and ledger are frozen per generation |
| D6 compactor context frozen per turn | frozen per generation |
| D7 "after that the best candidate wins" | a guard-failing line is never stored; the draft stays |
| D10 settle, verbatim-if-unbuilt, placeholders | removed; every unit has a line |
| D11 in-turn overflow uses the 07 cut | in-turn trim (from P5) |
| §4 binaries become `[image]` | named artefact units with descriptions |
| §5.2 every node built as soon as it is eligible | drafts at ingest, refinement by deadline, lazy merges |
| §7.2 four view blocks, each marked | one deep band, the view's end, the tail's newest message |
| VIEW_DOC "No message appears in full" | the messages after the view are word for word |

Unchanged from 137: units and their indices, the tree and its addressing, the guards, the slack fold's arithmetic, `zoom` / `recall` / `date`, the compactor model chain, spend caps, desks and meetings (a meeting still enriches its own head).

## 16. Revision 1 — Sky's answers, 2026-11-20 23:53

Asked with the four open questions of v1 (ask_user); his answers verbatim, and what each changed:

- **Tail size** → "scale to the model or fixed size both as options." `tail_mode: fixed | scaled` with both pairs of marks (§4.2, D3). The default is `fixed`; that choice is this document's, not Sky's.
- **Who writes the ledger** → "Both". The session model writes through the `ledger` tool; the summary model's flags are unconfirmed suggestions (§9, D10).
- **Promotion to the knowledge base** → "Not yet". Removed from scope (§9).
- **Benchmark corpus** → "no replay benchmark for now." P0 is dropped, D14 and §13 rewritten, the phase gates are tests and logged numbers, and the default flips on Sky's decision (§12, §14).

Still open: which `tail_mode` should be the default.

**Revision 2 — 2026-11-21 00:02.** Sky: "just make sure there are settings in the UI to control these new options." Added D15, §12.1 (keys, defaults, ranges), §12.2 (controls) and the PC and PU phases. The keys `ledger`, `ledger_suggestions`, `age_by_use`, `describe_images` and `merge_at` are new in this revision so that each behaviour has a switch.

## 17. Not verified

- O1. Whether per-turn input-block items are stored with the row (§4.3 restart).
- O2. Which memory mode children of an endless session run in, and how scheduled runs and external-harness sessions behave.
- O3. Whether a re-summarise path for existing nodes exists (each node records its model).
- O4. Whether interleaved sessions evict the prefix cache on the local servers in use.
- O5. That rows land in `messages` during a turn rather than at its end is inferred from the per-message `Store::append`, not traced through the turn loop (§2, §6).
- O6. All token and cost figures in §4.3, §7.2 and §13's toy model are estimates.
