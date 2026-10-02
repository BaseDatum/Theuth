# 81 — As built: MEETING-HEAD (the meeting owns its compaction head)

Branch `feat/meeting-owned-head`, base `95f6496a`, worktree `../theuth-wt-meeting-head`. Brief: `/tmp/mc-2026-10-02/omp-meeting-head.md`. Four commits; a meeting session now compacts over a head the plugin renders deterministically — no summariser, no snap frames, no harvest — and keeps its last minutes verbatim.

## Built, per commit

1. `fe0c8d49 feat(agent)` — the seam.
   - `crates/agent/src/lib.rs:342-366`: `SetCompaction.head: Option<String>` (`#[serde(default)]`), doc: a ready-made head body used by every compaction of the session that brings no head of its own; Summary forced over `agent.compact_style`; `None` clears; in memory only.
   - `crates/agent/src/loop_.rs:341-351`: `CompactionOverride.head`.
   - `crates/agent/src/loop_.rs:4365-4376` (first lines of `compact()`, above `let budgets = …`): `given = given.or_else(|| self.shared.compaction.read().head.clone())`; with a head and no `Clear` override, `r#override = Some(CompactStyle::Summary)` so `compact::resolve` yields Summary and the existing `CompactStyle::Summary if given.is_some() => compact::neutralise(given)` arm builds the body. `prepare_images` and the pre-compaction harvest already gate on `given.is_none()`, so neither runs. `/clear` keeps its own body and harvest (the session head is not adopted when the override is `Clear`).
   - `crates/agent/src/supervisor.rs:2722-2726`: the `SetCompaction` handler copies `head`.
   - Test `loop_/compact_queue_tests.rs::session_head_forces_a_text_summary_without_a_model_call` (added to the one-registry `scenarios()` run): cfg style `Snap` on the vision model `openai/eyes`, `SetCompactionOverride{at_tokens: Some(1), head: Some(..)}`, an auto compaction mid-turn → `Compacted{style: Summary}` exactly once, no `Error`, no request to the provider carries `VISION_SYSTEM` or `COMPACT_INSTRUCTION` (the fake would answer the summariser with 400), the stored frame head has no binaries and contains the body, `spill/<session>/epoch_1.summary.md` contains the body. Harness support in `loop_/vision_fallback_tests.rs`: `Harness::actor_styled(main, CompactStyle)`, `dir`/`requests`/`system`/`texts` made `pub(super)`.
2. `fadfbf4d feat(meeting)` — the head and its push.
   - `crates/meeting/src/head.rs` (new): `HeadInput{meeting, title, started_at, elapsed_ms, speakers, live: Vec<LiveTopic{label, at_ms}>, dismissed, briefs: Vec<BriefLine{at_ms, label, title}>, outstanding}`; `initial(&Manifest, session_title)` for `start_task`; `of(&Recording, now)` for the actor (live labels via `Topics::batch_labels(now)` zipped with `Topics::live()`, briefs via `Topics::briefs()`, outstanding via `Live::outstanding`); `render(&HeadInput) -> String` (markdown, stable order, the elapsed stamp is the only "now"; briefs capped at the last 20 with `… and N earlier`; ends with the `meeting_read(meeting=<id>, to_ms=<elapsed>)` pointer line and the standing rule); `changed(last, next)`.
   - `crates/meeting/src/state.rs:200-204`: `Live.speakers: Vec<String>` (order of first utterance) and `Live.last_head: Option<String>`; `Live::log_utterance` (:222) records `Utterance::who()` on first hearing — the funnel every closed utterance passes.
   - `crates/meeting/src/lib.rs:42` `pub mod head;`; `:534-539` `context_tokens` doc (trigger is a per-session override; the agent sizes tail and head as fractions of `trigger − fixed`, COMPACT-FLOOR; 32k trigger + ~7k system prompt ≈ last 8–9k tokens verbatim, compaction every ~15k tokens of growth at no model cost); `:569` default `32_000`.
   - `lib.rs:1306-1331` `Meeting::push_head`: render, compare with `last_head`, on change one `agents.ask(SetCompaction{session, at_tokens: Some(cfg.context_tokens), hint: Some(COMPACTION_HINT), head: Some(head)})`. Call sites: `deliver_batch` (:1387, immediately before `deliver`), `topic_gone` (:1474 — viewed, dismissed, expired all funnel here), new `topic_added` (:1478; replaces the three inline `TopicAdded` publishes at `cached_topics` :1132, `oracle::Verdict` :2564, `AddTopic` :2926), `show_brief` (:1505). Never a timer.
   - `lib.rs:1814-1826` `start_task`: the initial head (`head::initial`) rides on the existing `SetCompaction`; `:2285-2297` stop clears all three (`head: None`).
   - `crates/meeting/src/tools.rs:55-56`: `COMPACTION_HINT` doc — the summariser's fallback while no head is set.
   - `meeting_read` was already in the default `live_tools` (`lib.rs:579`); nothing to add.
   - Tests `head::tests::{golden, empty_state_prints_none_and_the_fixed_tail, briefs_cap_at_the_last_twenty, only_a_changed_state_pushes}` — the last is the "a topic change pushes a new head, an unchanged state does not" case on `render` + `changed`.
3. `6c9bfa74 docs(meetings)` — `docs/design/meetings/02-live.md` §0.2 (a) third field and its semantics; §4 start/stop with `head`, the push cadence, the 32k default, `COMPACTION_HINT` as fallback; new §4.1 "Meeting memory shape" (head / tail / record). `docs/design/meetings/README.md` config table `context_tokens: 32000`. `crates/theuth/harness.md` does not list `meeting.context_tokens` (grep); the config template comment in `crates/theuth/src/main.rs:1013` did and was updated (Deviations).
4. this note.

## Behaviour

- A meeting session with `agent.compact_style: snap` (the 2026-09-28 case) compacts as Summary over the plugin head: no archive image, no summariser call, no vision call, no harvest. The head lands in the frame head (`<context_summary … style="summary">`) and in `epoch_<n>.summary.md`.
- The head changes on every batch (elapsed stamp, cached-age tags) and on every topic/brief event; `last_head` dedupes exact repeats, so a topic event that leaves the rendered text identical (none in practice: each adds or removes a line) pushes nothing.
- Stop clears trigger, hint and head; a `/compact` afterwards uses the plain instruction.

## Gate (verbatim)

| command | result |
|---|---|
| `cargo test -p agent -p meeting` | first run: agent lib `349 passed; 1 failed` under concurrent sibling builds (the failing name was not captured by the output filter); rerun ×2: agent lib `350 passed; 0 failed` (14.86s), meeting lib `60 passed; 0 failed`, `tests/note.rs` 1 passed, `tests/oracle.rs` 1 passed, 2 empty doc-test bins |
| `cargo clippy -p agent -p meeting --all-targets -- -D warnings` | `Finished dev profile` clean (6m02s) |
| `cargo fmt --all -- --check` | exit 1 — every diff pre-exists at base `95f6496a` (51 files: sandbox, mesh, schedule, tui, …; the three agent files touched here differ only at base lines, e.g. `loop_.rs:3821`, `supervisor.rs:94/850/1767/2543/6314`, `vision_fallback_tests.rs:172`); same result with the repo's `stable` rustfmt 1.9.0 (1.96.1) and 1.98.1. My files: `head.rs` formatted with `rustfmt --edition 2024`; `rustfmt --check` on `lib.rs`/`state.rs`/`tools.rs` (meeting) clean |

Toolchain note: `rustup`'s `stable` proxy on this machine cannot execute `rustc` (`rustc -vV … No such file`), so every gate ran with `~/.rustup/toolchains/1.98.1-aarch64-apple-darwin/bin` first on `PATH` (the toolchain installed 2026-09-28).

## Deviations

1. **`/clear` does not adopt the session head.** The brief: "when `given.is_some()` and the style is not `Clear`, set override = Summary". Implemented so that with a `Clear` override the session head is not even taken as `given` (`loop_.rs:4369-4376`); otherwise `/clear` in a meeting would have skipped its pre-compaction harvest (the harvest gates on `given.is_none()`). Body and head of `/clear` are unchanged either way.
2. **`Compact{head}` with a configured non-Summary style now forces Summary too** — a consequence of the single `given`-based rule; before, a ready-made head under cfg `snap` was ignored in favour of the archive. Matches the `Compact::head` doc ("stands in for the summariser").
3. **`to_ms` pointer = the elapsed stamp at render time**, not "the ms of the first `[meeting hh:mm:ss]` line still visible": the plugin cannot know which line the agent's fold will keep. The pointer is a superset (reading up to it overlaps the visible tail, never misses it).
4. **`Live.speakers` added** (brief named only `last_head`): the head needs the speaker set and nothing tracked it; recorded in `Live::log_utterance`, the funnel every closed utterance passes.
5. **Brief lines whose topic is no longer live print the brief's source** (`request`, `deeper`, `tap`, …) as the label: `Topics` keeps no label for viewed/expired topics and the dismissed list is labels only.
6. **The stop-time last batch (`lib.rs:2265-2284`) does not push a head**: the override is cleared right after it.
7. **No `head_changed` actor test**: the only actor harness (`bench()`, `lib.rs` tests) runs without an `Agents` actor, so a `SetCompaction` ask cannot be observed there; the brief's alternative — the pure `changed` fn plus `render` determinism — is tested in `head::tests::only_a_changed_state_pushes`. `head::of` (the `Recording` → `HeadInput` projection) is exercised only by compilation; building a `Recording` needs a `Capture`.
8. **Outside my seams**: `crates/agent/src/loop_/vision_fallback_tests.rs:282,430-458,221,231,536` (test harness visibility + `actor_styled`), `crates/theuth/src/main.rs:1013` (config template comment `context_tokens: 32000`).
9. **Two asks on one event when an add expires an older topic** (`topics_gone(expired)` then `topic_added`): each is a change; bounded by `max_topics`.

## Open follow-ups

- Head size: a typical meeting renders well under 1,500 chars (golden ≈ 700); 20 briefs with long titles can reach ~2,500. Trim `BRIEF_CAP` or title length if the head starts to crowd the tail.
- `Topics` could keep every topic's label (viewed/expired included) so brief lines never fall back to the source word.
- When COMPACT-FLOOR lands, re-check the `context_tokens` doc numbers (8–9k tail / ~15k growth) against the actual fractions.
- The agent's `Compacted` event carries no body; the test reads the store frame and the spill file instead.
