# 158 as-built — W3E (engine: projects as the meeting series + the pre-meeting brief)

Branch `feat/158-w3e` from main 23ea03c5. Design `docs/design/158-meetings-around.md` §9, §5.2 (c), §15 W3 rows, A2 (1).
Owned files touched: `crates/meeting/src/store.rs`, `calendar/prebrief.rs` (the whole W3 module), `calendar/mod.rs` (W3 lines),
`lib.rs` (project lines, boot, one test), `detect/mod.rs` (project lines), `rpc/src/api.rs` (one doc line), fixture
`crates/meeting/tests/fixtures/prebrief/summary.md`. Outside the list, minimal: `detect/rules.rs` `Engine::prompt_mut()` (3 lines),
`oracle.rs` `CALENDAR` → `pub(crate)` (visibility only). No new dependencies.

## What was built

- **Store** (`store.rs`): `meetings.project` read and written — `Row` + `row_to_summary`, `upsert` (insert + `ON CONFLICT` update),
  `list(limit)` kept for `tools.rs`/`search.rs`/`api.rs` callers and now delegates to `list_in(limit, project: Option<&str>)`;
  `done_in_project(project, limit)` = §9.4's `WHERE project = ? AND state = 'Done' ORDER BY started_at DESC LIMIT ?`.
  `ListMeetings` handler (`lib.rs`) applies `m.project`; `meeting_list(limit, project)` is live end to end.
- **Project at start** (`lib.rs` StartMeeting handler, after the W2 seed): `manifest.project` = the start's pick, else memory
  (`uid:` → `title:`), else `meeting.project`. An explicit pick writes the memory rows. `start_task` creates the session with
  `project: manifest.project.or("meetings")` (W-c; A2 (1): a meeting with a project lists its chat under that project).
- **`meeting_set_project`** (`prebrief.rs` `SetProject` actor message, `rpc_set_project` body): live or on-disk manifest, memory
  rows (`remember` / `forget` on `None`), `Store::save` (manifest + index row + `State`), then `agent::SetSessionProject` so the
  sidebar follows (`project.or("meetings")`).
- **Project memory** (`prebrief.rs`): `norm_title` (lowercase, letters only, months/weekdays dropped — `oracle::CALENDAR`),
  keys `uid:<uid>` / `title:<norm>`, `remembered` / `lookup` (`uid` → `title` → default) / `remember` / `forget`;
  `remembered_project(rt, …)` body. `Detection.project` and `.brief` are filled by `prebrief::decorate` in `publish_detect`
  and in `rpc_detection_set_event` (so the `[▾]` pick re-resolves both); the engine's held prompt carries them too
  (`SetPromptEvent` gained `project`/`brief`; `Engine::prompt_mut`).
- **Pre-meeting brief** (`prebrief.rs`): inputs = last 3 Done meetings of the project, each `summary.md` split by heading into
  Decisions / Action items / Open questions (`excerpt`, headings as `## X`, `**X**`, `X:`), capped at 2 500 chars each; the event
  (`EventRef::context_block`: title, time, description, attendee names). One writer turn (`Writer` trait; production `PrivateTurn`):
  `agent::CreateSession { retention: Private, view: "prebrief", project, model: meeting.model (None → agent.model), memory: Epochs }`
  with a synthetic owner peer (`u64::MAX − 158` + a fresh `tools::CancellationToken`), `SetToolset {knowledge_search, knowledge_get,
  meeting_read, meeting_search, meeting_list, brief_search; prompts meeting+knowledge; runtime false; hint_limit Some(0)}`,
  `SendInput`, poll `LoadSession` until idle with an assistant reply, 45 s timeout, `LeaveSession` (Private → nothing persists).
  Bullets parsed (`-`, `*`, `•`, `1.`; ≤ 3). The **engine** files the `briefs.sqlite` row (`Cache::put` + `BriefEvent::Changed`):
  title = event title, aliases = event short label (≤ 4 words) + attendee first names (not `is_me`), tags `["pre-meeting", <project>]`,
  source `event:<id>`; a regeneration updates the same row (`id` kept in `cal_prebrief.brief`).
- **Cache** `cal_prebrief(event_id, generated_at, bullets, brief, sources)`: `sources` = `{meetings: [last-3 Done ids], fingerprint:
  "<sequence>:<hash(description,title,start)>"}`; `ensure` regenerates when either differs — so a new Done meeting of the project
  or an event edit invalidates **lazily on the next tick**, with no hook in `summary.rs` (W4's file).
- **Upcoming tick** (`Scheduler::tick`, `ensure_upcoming(rt)` started at boot beside `ensure_poll`): every 60 s, when
  `meeting.calendar.prebrief`; events with `start ∈ [now − 5, now + 15] min`, not cancelled / all-day; project from memory
  (`uid` → `title` → `meeting.project`; none → skipped, §9.4); generation inside `start − 15 min` (synchronous in the tick's own
  task, one attempt per instance per process); `MeetingEvent::Upcoming { event, project, brief }` **once** per instance from
  `start − 5 min` (also when the engine first sees the event inside the window, up to `start + 5`). `brief_for(rt, event, project)`
  = `ensure` on the stored row (cached bullets only when `prebrief` is off or the row is gone).

## Deviations

1. The brief row is written by the engine after the turn, not by the model calling `brief_put` (§9.4 prompt): deterministic
   aliases/tags, testable with a canned model, no dependency on the model obeying a tool instruction; `brief_put` left out of the
   turn's toolset (read tools only).
2. `Upcoming` is emitted once per event instance (not every minute); a companion connecting inside the window misses it (the
   `Detected` prompt still carries `brief`). Islands clear at `start + 5` on their own (§9.4).
3. Only an explicit `StartMeeting.project` writes memory at start; a project that came from `meeting.project` is not made sticky.
4. Invalidation is lazy (compared on the next tick / `brief_for`) rather than event-driven at `Done`.
5. The cache takes 2–3 bullets: a one-bullet reply is cached and shown on the island but files no brief row.
6. `SetPromptEvent` (W1's struct in `detect/mod.rs`) gained two fields; `rules.rs` gained `prompt_mut`; `oracle::CALENDAR` is `pub(crate)`.
7. `Store::list(limit)` kept (callers in `tools.rs` are W4's); the filter lives in `list_in`.

## Hand-offs

- W3M/W3W: island state (c) consumes `Upcoming` as frozen; `Detection.project`/`brief` are now filled.
- W3U: `meeting_list(limit, project)` filters; `meeting_set_project` live; start form `project` is honoured and remembered.
- Orchestrator live pass: a real recurring event on Sky's calendar five minutes before (needs a running engine + agent).

## Libraries

- Heading split: hand-rolled 20-line splitter — `pulldown-cmark` (workspace) sees `## X` but not the `**X**` / `X:` forms the
  summary form actually writes; rejected. Dates/times: `chrono` (existing). JSON columns: `serde_json` (existing). No new deps.

## Open items

- `PrivateTurn` needs the private-mode participants registry ready (`ParticipantsReady`); if private mode is unavailable the
  tick logs and marks the instance failed (no brief) — retried after a restart.
- `brief_for` is a frozen entry point with no engine caller yet.
- Not exercised in tests: the agent hop (`CreateSession` Private / `SetSessionProject`) — no in-crate mock model; covered by the
  `Writer` seam and the agent's own tests.

## Gates (each its own command, BUILD_LABEL=158w3e)

- `build-lock mbx check -p meeting --tests` → 0, no warnings.
- `build-lock mbx test -p meeting -p rpc --no-run` → 0; `mbx test -p meeting -p rpc` → meeting 99 passed (1 ignored, pre-existing)
  + calendar 15 + 1 + 1, rpc 55 passed; `rpc::engine::tests::second_acquire_in_one_process_is_running` failed once
  ("engine.lock is held but engine.json is missing" — a shared-filesystem lock race with another worker's run; it passed in the
  run before and in the re-run `mbx test -p meeting -p rpc engine::tests` → 4 passed).
- `build-lock mbx clippy -p meeting -p rpc` → 0, no warnings. `cargo fmt -p meeting -p rpc` bare.

## Proof left (§16 W3)

`prebrief::tests`: `memory_lookup_order_and_forget` (uid → title → default → none; forget), `excerpt_splits_the_fixture_by_heading`
(`tests/fixtures/prebrief/summary.md`, three heading styles), `prebrief_caches_and_invalidates` (1 turn for 2 `ensure`s; sequence
bump → 2; another project's Done meeting → still 2; a new Done meeting of the project → 3; one brief row updated in place),
`three_done_meetings_one_turn_brief_row_upcoming_at_t_minus_5` (canned `Writer`: prompt carries the three excerpts newest first,
not the 4th/oldest meeting, not the Numbers section, the attendee names; brief row aliases `["Weekly sync", "Dana", "Lee"]`, tags
`["pre-meeting", "theuth"]`; `Upcoming` once at T−5 with the 3 bullets, nothing at T−4 / T+6; `decorate` fills
`Detection.project`/`brief`; `prebrief = false` inert). `lib.rs::tests::set_project_updates_manifest_index_and_memory`
(manifest + index row + `meeting_list(project)` + `done_in_project` + title memory; clearing forgets). Session project: the one
expression in `start_task` (`manifest.project.or("meetings")`) and `SetSessionProject` in `SetProject` — not exercised (no agent in
the bench).
