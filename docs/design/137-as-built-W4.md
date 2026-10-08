# 137 — As built, W4 MEETING (the hybrid: tree over the meeting inside head D)

Status: as built · 2026-10-08 · branch `feat/137-w4` from `bba57894` (W1 skeleton + design R1). Spec: 137-endless.md §12, §17 R1-D6; API: 137-contract.md §4. Frame shape unchanged from wave 81 (head D + verbatim tail, cut compaction at no model cost).

## 1. What was built (file level)

| file | change |
|---|---|
| `crates/meeting/src/endless.rs` (new) | The meeting side of the mode. `Config { default_meeting, meeting_view_bytes }` read from `agent.endless` (`Config::load(rt)`, defaults `false` / 32 000; `resolve(start option)`). `trait Memory { enable_meeting(session), render(session, bytes) -> Vec<agent::ViewLine> }` — the port to `agent::endless::Endless` (`set_mode(.., Endless, UnitFilter::Meeting)` and `render_with_budget`), with `install(Arc<dyn Memory>)` / `port()` (a `OnceLock`, runtime input). `TOOLS = [zoom, recall, date]`, `toolset(base)` (adds each once). `VIEW_DOC` (meeting variant, §3). `earlier(port, session, budget) -> Option<head::Earlier>` within `RENDER_WAIT` (2 s): the view, or `None` on error/stall so the caller keeps the last one — the head never waits for the compactor. |
| `crates/meeting/src/endless_agent.rs` (new, **not in the module tree yet**) | `AgentsMemory`: the production `Memory` over W2's `agent::SetSessionMemory` / `agent::RenderMemoryView` kameo messages, plus `install()`. Enabled by the closer (§4 item 1). |
| `crates/meeting/src/head.rs` | `Earlier { lines: Vec<ViewLine>, budget }`; `HeadInput.earlier: Option<Earlier>`; `initial(.., earlier)`, `of(rec, now, earlier)`. `render` appends, after the standing rule, `## Earlier in this meeting` + `VIEW_DOC` + `<chat>` … `</chat>` with one `id+n|text` line per view line (newlines → spaces, oldest first). `push_view` bounds the lines to `budget` bytes: the oldest fold into one `first+count|(earlier in this meeting, not shown; recall searches it)` line until the rest fit (the marker counts against the budget); no lines → `nothing yet`. Pure function of its input. |
| `crates/meeting/src/lib.rs` | `pub mod endless`. `StartMeeting.memory: Option<agent::MemoryMode>` (`serde(default)`, `ts(optional)`) — `meeting_start` over RPC passes the struct through, so crates/rpc needed no edit. `Meeting.endless: endless::Config` loaded at plugin start. `StartMeeting` handler: `manifest.memory = endless.resolve(m.memory)`; endless with no port installed → `warn!` once, epochs. `Setup.view_bytes`. `start_task`: `CreateSessionOptions.memory = Some(Epochs)` explicitly (never the chat default, so the agent never starts an unfiltered tree on a meeting session), then with the mode on `port.enable_meeting(session)` — a refusal degrades to epochs with a `warn!` — then toolset = `endless::toolset(base)` and the initial head with an empty `Earlier`. `Begun`: `rec.live.earlier = Some(empty)` with the mode on. `push_head`: with the mode on, `endless::earlier(..)` first (same cadence: every batch and topic change, one more section), result cached in `Live::earlier`; then `head::of(rec, now, earlier)` → `SetCompaction{head}` as before. Stop path unchanged: trigger/hint/head/toolset cleared, the tree stays. |
| `crates/meeting/src/state.rs` | `Live.earlier: Option<head::Earlier>` (the last view rendered into the head). |
| `crates/meeting/src/store.rs` | `Manifest.memory: agent::MemoryMode` (`serde(default)` → `epochs` for old manifests). |
| `crates/meeting/Cargo.toml` | `ts` feature += `agent/ts` (`MemoryMode` on the wire in `StartMeeting`/`Manifest`). |

The head with the mode on (after wave 81's last line `Do not re-offer a live or dismissed topic; reply with no text unless asked.`):

```
## Earlier in this meeting
<VIEW_DOC, one paragraph>
<chat>
0+4|user: meeting started, no context; roadmap and ACME-1234 raised by Speaker 1
4+1|tool: meeting_topic {"label":"Q4 roadmap"}
5+1|user: [meeting 00:06:40] me: what did we promise sales
</chat>
```

Tests (`crates/meeting/src/{head,endless}.rs`): section present with a fake tree and byte-identical head D before it; absent with the mode off (the wave-81 golden is untouched); empty view → `nothing yet`; bounded by the budget, oldest folded first, no marker when it fits; deterministic for the same input; `earlier()` against a double (view / error / stall under paused time, gives up at exactly `RENDER_WAIT`); `Config` from the `agent.endless` YAML section + defaults + `resolve`; `StartMeeting.memory` deserialises and defaults to `None`, `Manifest.memory` defaults for pre-137 manifests; the toolset gains exactly `zoom`, `recall`, `date` once and is otherwise unchanged (idempotent).

## 2. Gates (exact result lines)

```
BUILD_LABEL=137-W4 build-lock cargo check -p meeting -p rpc
    Finished `dev` profile [unoptimized + debuginfo] target(s) in 59.14s      (0 diagnostics in crates/meeting or crates/rpc; 94 warnings are W1's agent skeleton)
BUILD_LABEL=137-W4 build-lock cargo test -p meeting --no-fail-fast
test result: ok. 69 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in 0.10s   (lib)
test result: ok. 1 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in 0.00s    (tests/note.rs)
test result: ok. 1 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in 2.35s    (tests/oracle.rs)
test result: ok. 0 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in 0.00s    (doc)
BUILD_LABEL=137-W4 build-lock cargo clippy -p meeting --no-deps -- -D warnings
    Finished `dev` profile [unoptimized + debuginfo] target(s) in 5.01s       (exit 0)
cargo fmt -p meeting                                                          (clean; formatted files committed)
```

No live smoke: the agent bridge (§4 item 1) is not wired on this branch, so a started meeting runs in epochs with one `warn!`; the head/tool/start paths are exercised by the unit tests above.

Re-run after the orchestrator merged W1's full implementation (`b4a7fd1f`, W1 at `8f9cc634`): `cargo check -p meeting -p rpc` → `Finished … in 35.47s`, exit 0; `cargo test -p meeting --no-fail-fast` → `ok. 69 passed` / `ok. 1 passed` / `ok. 1 passed`; `cargo clippy -p meeting --no-deps -- -D warnings` → `Finished … in 39.93s`, exit 0. The merged `agent::endless` surface matches what the port abstracts (`ViewLine.text` is the verbatim unit or placeholder when unbuilt; `SetMode`, `MemoryMode` unchanged); `render_with_budget`, `UnitFilter`, `set_view_bytes`, `backfill_estimate` are still W1's follow-up and are reached only through `endless_agent.rs`.

Re-run after W1's R1 interfaces merged (`b8f0b019`, W1 at `087f527c`; `endless::Config` now from `agent::EndlessConfig`, `lines_of` added for the `ViewBlock` return of `render_with_budget`): `cargo check -p meeting -p rpc` → `Finished … in 38.09s`, exit 0; `cargo test -p meeting --no-fail-fast` → `ok. 70 passed` / `ok. 1 passed` / `ok. 1 passed`; `cargo clippy -p meeting --no-deps -- -D warnings` → `Finished … in 17.22s`, exit 0; `cargo fmt -p meeting` clean.

Re-run after W1 `18a9cff2` merged (`2bf11613`; `render_with_budget -> Vec<ViewLine>`, so `lines_of` was removed and the bridge maps `ViewLine` directly): `cargo check -p meeting -p rpc` → `Finished … in 42.87s`, exit 0; `cargo test -p meeting --no-fail-fast` → `ok. 69 passed` / `ok. 1 passed` / `ok. 1 passed`; `cargo clippy -p meeting --no-deps -- -D warnings` → `Finished … in 45.29s`, exit 0; `cargo fmt -p meeting` clean.

## 3. Deviations and decisions (design silent or adjusted)

1. **VIEW_DOC rides in the head, not the system prompt.** The agent has no per-session system-prompt block: `Toolset.prompts` only selects *plugin* blocks by name, and a block registered with `RegisterPrompt` is seen by every session without a toolset (`loop_.rs` `toolset.is_none_or(..)`), i.e. every ordinary chat. The meeting's standing rules already live in the head (wave 81), so the doc sits at the top of the `## Earlier in this meeting` section — per session, mode-gated, no agent edit, constant bytes.
2. **VIEW_DOC wording.** Besides the last sentence the brief asks for ("Zoom or recall when a topic was raised earlier in the meeting and the summary only hints at it."), three fresh-turn sentences were wrong for a head+tail frame and were adjusted: scope ("this meeting so far"), kinds (only `user` and `tool` survive `UnitFilter::Meeting`), and "No message appears in full, not even the last ones" → "The most recent messages also follow this head in full"; the `(not summarized yet: zoom it)` placeholder sentence is gone because the meeting renders unbuilt parts verbatim. Constant in `endless::VIEW_DOC`; not derived from `agent::VIEW_DOC` at runtime (a silent drift would be worse than a visible copy).
3. **Section placement.** After the standing rule, as the brief says ("your section goes after them"), so the mode-off head is byte-identical to wave 81 (golden test untouched). The head therefore ends with `</chat>` rather than the standing rule; if that measurably weakens the "reply with no text" rule, move the section before the `Earlier transcript …` paragraph — one `write!` swap.
4. **Budget.** `meeting_view_bytes` bounds the `<chat>` lines; the heading and the ~1 KB doc paragraph are constant extras. W1's `render_with_budget` folds to the budget itself; `push_view` is the hard ceiling (and the only bound in the tests' fake-tree case). Oldest lines fold into one marker line in view syntax so the model sees the gap and knows `recall` still reaches it.
5. **Recent duplication.** The view covers every unit, so its level-0 lines for the last few batches duplicate text that is also in the verbatim tail (R1-D6 c says "the rendered view"; ~1–2 KB). Follow-up if it matters: render only parts whose units are before the compaction cut — needs the cut's unit index from the loop.
6. **`CreateSessionOptions.memory = Some(Epochs)`** on the meeting session, then `enable_meeting` — never the chat default, so a user with `default_chat: true` does not get an unfiltered tree + fresh-turn frame on a meeting session.
7. **Degrade, don't fail.** No port installed or `set_mode` refused → the meeting starts in epochs, `Manifest.memory = Epochs`, one `warn!`. A meeting is never refused for its memory mode.
8. **No `MeetingConfig` keys.** Budget and default live under `agent.endless` as R1 says; the meeting crate reads just those two keys from the agent's section with its own small struct (serde ignores the rest). W1's `EndlessConfig` does not have them yet → hand-off 2.
9. **Post-meeting.** Nothing is done at stop; the tree stays as the meeting's zoomable record and `sessions.memory` stays `endless`.

## 4. Hand-offs

1. **Bridge to `agent::Endless` (final pin 04:16: kameo messages, W2; the closer enables; blocks the live path).** `Endless` is constructed by the supervisor (`new` is `pub(crate)`); nothing outside crates/agent can reach it. W2 adds two `Agents` messages — `agent::SetSessionMemory { session, mode: MemoryMode, filter: UnitFilter } -> anyhow::Result<SetMode>` (the agent supplies `epoch` to `Endless::set_mode(session, mode, filter, epoch)`; a meeting session has no epoch head to write) and `agent::RenderMemoryView { session, bytes: u32 } -> anyhow::Result<Vec<agent::ViewLine>>` (exactly what `Endless::render_with_budget` returns since W1 `18a9cff2`; `built` tells verbatim from summarized). The production port is written against them in **`crates/meeting/src/endless_agent.rs`** (`AgentsMemory`, `install()`), kept out of the build until W2's commit is on the branch. The closer enables it with two one-line edits in `lib.rs`, both marked `137 R1-D6 hand-off`: uncomment `mod endless_agent;` (module list) and add `endless_agent::install();` in `MeetingPlugin::start` before the `Meeting { .. }` literal. Paths used: `agent::UnitFilter::Meeting`, `agent::SetMode` — adjust if W2 names the messages differently.
2. ~~W1 `EndlessConfig` += the two meeting keys~~ — landed in W1's R1 interfaces (`087f527c`); `endless::Config` is now `From<&agent::EndlessConfig>` and `Config::load` reads `agent::EndlessConfig` from `agent.endless`.
3. **W2 loop**: for a session whose filter is `UnitFilter::Meeting`, do **not** switch to the fresh-turn frame and do **not** append the generic VIEW_DOC (R1-D6 d; the head carries the meeting variant). Suggested: `Endless::active()` false for Meeting-filter sessions, or expose the filter. `zoom`/`recall`/`date` must still be offered: the meeting's allowlist names them (`SetToolset`), so the tool specs need to be gated on `mode == Endless`, not on `active()`. Mechanics: `loop_::direct_specs` is an explicit allowlist over every spec — registry tools pass through `listed(name)` automatically; a loop-local spec (the `todo` pattern) must be pushed only `if listed(name)`, else the three never reach a meeting session.
4. **W3**: `StartMeeting.memory?: MemoryMode` is on the wire (bindings regenerate with `just ts-desktop`); the ∞ switch sends `"endless"`; initial state from `agent.endless.default_meeting`. `Manifest.memory` is exported too, for an ∞ glyph on the meeting row.
5. **Summariser outline (follow-up, not built).** `summary::form_for` / `SUMMARY_FORM` tell the summary turn to `meeting_read` the whole transcript in pieces. For an endless meeting, prefer the view's top-level lines as the outline: render the view once at `Reviewed`, pass it in the form ("the lines below are the meeting's own summary tree, oldest first; use them as the outline and `zoom` where a line is thin; `meeting_read` for quotes"), and keep `meeting_read` for quotes. The `zoom` tool must be available post-meeting (the toolset is `None` after stop, so this is W2's mode gate).
6. **Series continuity (follow-up).** One endless log per recurring meeting series needs a series identity the meeting store lacks (`Manifest` has no `series`/`recurrence`). Sketch: `StartMeeting.series: Option<String>` (title-derived default from the calendar/title), a `meeting_series(series) -> Vec<MeetingId>` store index, and either one agent session per series reused across meetings (the tree then spans them for free) or an `/endless import` of the previous meeting's top lines as `note` units (R1-D5) at start.
7. **Closer smoke** once item 1 lands: `meeting_start { memory: "endless", consent: true }` on the scratch engine, three `[meeting …]` batches, then `session_memory(session)` shows level-0 lines and the session's compaction head (`LoadSession`) ends with `## Earlier in this meeting` … `</chat>` listing them; `meeting_stop` leaves `session_memory` populated.
