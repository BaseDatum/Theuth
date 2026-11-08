# 183 — build notes

## Phase 1 review round R2 (Z183-R2)

UI re-review approved; the engine re-review had one must-fix. Fixed in 184f7bb5 and a follow-up; evidence `/tmp/rs183/r2-evidence.txt`, screenshots `/tmp/rs183/shots/r2/`.

| Item | Fix |
|---|---|
| R2-M1 half-drawn reply after a follow loss | The slice keeps `following` from `FollowState`. A replicated `ContextChanged` is swallowed (and a settled page refused) over an open reply only while `following` is true. `FollowState{live:false}` (or a peer disconnect) marks the slice stale and requests a settled reload (`theuth:reload-session`), and the stream coming back up retries a settle a still-open turn refused. Reducer test: open tail, follow lost, replicated change → stale → the settled page installs the full reply. Live: B killed mid-reply and restarted (A untouched, no refocus); A showed the full resumed reply, `tick19`. |
| 1 owner change to here | The aborted follower's relayed cards are forgotten and its live flag cleared (with a `FollowState` false). |
| 2 seat at spawn | An actor spawn seats the session at `sessions.cwd` when it exists and no seat is set, so a restart keeps a chosen folder and its prompt. |
| 3 `SESSIONS_WIRE_SCHEMA` | `agent::store::SESSIONS_WIRE_SCHEMA = 53` replaces the newest-migration number in `Hello`/`Release`. **Bump rule** (also on the constant): bump it to a migration's number exactly when that migration adds, drops or renames a column of a replicated session table; a migration touching only local tables leaves it. |
| 4 H1 | Done in R1 (94ce476f). |
| 5a replayed prompts | The owner sends the cards already pending at `Watch` with `seq` 0 (as `Watching`); the viewer adds `replay: true` to their JSON and the UI states no "denies at" for them. Test `a_card_pending_before_the_follow_is_flagged_as_a_replay`; live: after a reload, the replayed card had no deadline. |
| 5b `routed()` | MemoryPanel zoom and recall, Peers answer, Communication session and scope setters (the global switch is this engine's), SessionList bulk delete and archive (per session). |
| 5c tests | Header approvals pill (`lib/header.ts` `approvalsOff`), vanished machine target, Composer NotOwner retry (`retryNotOwner`). |
| Live extra | After B's crash-resume finished, A kept "Running on b": a replica's page with the owner's last report (`resumed: true`, no open turn) now shows no banner. Test in `reduce.test.ts`. |

## Phase 1 review round R1 (Z183-R1)

Both reviews (`/tmp/rs183/review-{engine,ui}-p1.md`) returned CHANGES. Fixed in 2a78d330 and its follow-ups (94ce476f, 470c6e31 for H1–H3); evidence `/tmp/rs183/r1-evidence.txt`, `/tmp/rs183/r1-mixed-87f4.txt` (the mixed run against a `t` built from 87f4acf0 by `git archive` + `mbx build`, binary `/tmp/rs183/t-base-87f4acf0`; an earlier run against 182's `t-base` 8316913d is `/tmp/rs183/r1-mixed.txt`), screenshots `/tmp/rs183/shots/r1/`.

| Item | Fix |
|---|---|
| E-M1 relay blind after reconnect | The owner no longer filters on `from_seq`: nothing is buffered to replay, and its per-session counter restarts with the `served` entry or the process. The viewer starts each stream at 0. Test `a_reconnected_viewer_still_gets_events_and_cards` (drop on the owner, reconnect, new delta and card arrive). Live P3b: A restarted mid-turn, deltas resumed, a new card arrived in 380 ms. |
| E-M2 routine pin outlives its file | Every fire applies the file's `approvals:`: `false` pins off, `true` clears the pin (`schedule::apply_approvals`). Test `approvals_true_fire_clears_the_pin_a_false_fire_left`; live P4b: row `off` after a false fire, `NULL` and a card after a true one. |
| E-M3 mixed versions | The agent schema version (`Store::schema_version()`, the newest migration, 53) rides as a postcard trailer after `Hello` and `Release`. Old decoders ignore trailing bytes (postcard 1.1.3 `from_bytes`); a frame without it decodes as `schema: None`. A peer with no or an older version gets no log rows, relays or snapshots, and its cursor does not move: `sessions: holding log for <peer>: its agent schema unknown < ours 53`. Its `Release` is refused by closing the stream without `Released` (an old taker's `release_from` fails, so an orphan/write take does not happen; an explicit old "Continue here" still takes without the tail, as for an unreachable owner). Skipped tables on apply (log and snapshot) warn `apply SKIPPED table …`. Tests in `types`, `sync`, `owner`. Live: A new, B `t-base`: B got 0 of 8 messages and A logged the hold; B upgraded, restarted, and both sessions with all 8 messages arrived. B's record replica (mesh stats, not sessions) logged `engine_stats.os is not a replicated column here` while old. |
| U-M1 `pin: null` | Done in pass 1 on the engine (`pin` omitted when `None`, 4498d19c); the UI also treats `null` as inherit now, with `pin: null` tests. |
| S1 opt-out row ↔ policy | `mirror` (so `GetCommunication` and every reader) and actor spawn apply `sessions.communication` both ways (`sync_opt_out`). |
| S2 fork opt-out | `ForkSession` writes `communication = 0` on the fork when `record_fork` opts it out; the row sync above would otherwise undo it. |
| S4 cell kernels | The owner's relay drops `CELL_PREFIX` kernel jobs (tracks `Started`), so a mid-run follower never shows one as a terminal. |
| S5 moved row | `append_moved` decides `in_context` inside its own transaction (open turn → context-excluded). |
| S6 `resume_turn` | Dead `from = owner_peer()` removed, doc corrected; `turn_openers` doc names the moved record. |
| S7 `get_approvals` on an old owner | Deviation: no engine fallback. The row's pin alone cannot give the effective value (it needs the owner's `tools.approvals`), so the UI reads `-32601` as "<owner> needs an update" and greys the row. |
| S8 remote running | `mark_remote_running` counts an open remote turn only while its owner is in `mesh::connected_ids()`. |
| UI 2–10 + tests | See the UI agent's list below; tsc 0, full vitest green. |
| Chip "live" | New client event `FollowState{session, live}` from the viewer's relay: `live` once the owner answered `Watch`, `false` when the stream ends; re-announced to a second client. The chip reads `on b · live` only then, `on b` when connected but not followed (old owner), `on b · offline` otherwise. |
| H1 old owner's pending prompt after a move | `release()` drains the session, then tells the approval and ask actors `ForgetSession`: the pending entries go (their waiters publish `*Resolved`, so cards clear and a reload's snapshot no longer lists them) and the `questions` rows stay `pending` for the taker's re-ask. Test `forget_session_drops_the_card_and_keeps_the_row_pending`; live: after a mid-turn move and a reload, one card on each machine. |
| H2 job notes name the machine | `communication::mesh::machine_name` (mesh name, else "another machine") in the interrupted-job note, the lost `wait` reply and the transferred job's `owner changed (ran on …)` body. Unit tests updated (no mesh in tests → "another machine"). |
| H3 banner after a move | A transfer reads `Moved here from <machine> at HH:MM — resumed here`; `Taken from <machine>` stays for a takeover. `Loaded.resume` gains `from` (the latest moved record's `from_id`) so the name survives a reload. Live: `Moved here from a at 08:20 PM — resumed here`. |

UI changes (UI R1): `withdraw` on the approvals/asks stores and `relayGuards` in `App.tsx` (peer disconnect, owner change, unfocus of a remote chat, popouts too); "denies at" only for cards that arrived live (`arrived` map, `firstSeen` gone); `-32013` keeps the draft and retries once after `SessionOwnerChanged`; `-32014` reads "live output on <machine>" for terminals and cells; `routed()` for the model/effort picker, Rename, sidebar row actions, queued withdraw, regenerate/edit, remove_context, Memory panel, Settings › Approvals, and no "Stopped N agents" toast for a parked stop; "<owner> default (on|off)" from the effective value; NewChatPane `pickMachine` drops a foreign model pick, clears a vanished target, aria-labels carry values, Browse debounced with Enter accepting; parked calls clear on peer connected; container-width short placeholder (`useNarrow`); a replica no longer shows a stray "✓" line for a relayed `ToolCallFinished` it never saw start.

Observed, not fixed: after a viewer restart mid-turn, A showed only the deltas since the reconnect until `TurnFinished` settled the full row (P3b). The UI's first page brings the replica's partial, which the next delta replaces as for a local resume.

Gates (R1, final 470c6e31): `check` and `clippy --tests --no-deps -- -D warnings` on agent, rpc, sessionsync, mesh, tools, platform, tui, cells, schedule, replica: 0 / 0. Touched suites twice, both runs green. Full suites (worker env) on agent, rpc, sessionsync, mesh, tools, schedule: agent 795 passed, 1 failed (`capping_binds_out_name`, known); rpc 81, sessionsync 42 + 2, mesh 29, tools 218, schedule 82, 0 failed (an earlier full run also missed `sensor::tests::monty_budgets_and_allowlist`'s 5 s bound under load; it passed alone twice). Bindings 8 passed; tsc 0; vitest 315 files / 2046 tests passed.

## Phase 1 as built + live proof (Z183)

**Merge and fold.** `feat/183-p1-ui` (c44ba763) merged with no conflict; it touched only `crates/desktop/ui/` and `183-ui-notes.md`. `origin/main` folded: three docs-only commits (3d9a4bd5, c5999547, e78ef26b) on `183-remote-sessions-and-a2a.md` and `183-rulings.md`; no code overlap.

### Reconcile: U's assumptions against E's contract and the engine

| U assumption | Finding | Done |
|---|---|---|
| Approvals tri-state reads `ApprovalsState.pin` (`undefined` = machine default) | The wire sent `"pin": null` (serde default, no skip) while TS says `pin?`, so a session without a pin read **Off**. And `pin` came from the registry's memory, filled only when the session's actor spawns: a session idle since a restart or a move read "inherit" though its row said `on`. | `pin` (and `MeshStatus.relay`) skip `None`. `get_approvals` re-applies the row's pin (`RestoreApprovals`) before reading; `SetApprovals` now awaits its journal write, so that re-read never undoes a fresh click; actor spawn restores `None` too, dropping a pin this registry kept from before a move away. Test: `tools` `session_pin_is_journaled_and_restore_is_not` (written before the reply). |
| Relayed card deadline = first seen + 300 s | The owner's deadline is not on `ApprovalRequested`; adding it touches ~20 struct literals in 6 crates. | Kept U's ponytail (an owner with another `approval_timeout_secs` shows the wrong time). |
| "live" = owner connected | The engine exposes no follow-stream state to the UI. The open chat is focused, so it is followed whenever the owner is connected; P3 shows the stream under `on b · live`. | Kept. |
| Header chip `approvals off` | Read the engine-wide `Info.approvals`: a chat pinned On under `tools.approvals: false` (or any chat owned elsewhere) showed this machine's switch. | `Header.tsx` reads `get_approvals(session)` (keyed on this engine's switch too). |
| ⋯ Effort (U skipped) | §12.2 has it. | Built: Effort ▸ default + the model's levels, `set_effort` through the owner, greyed offline or when the model has no levels. |

### Closer items

- **(a)** The placeholder takes its short form under `md` (768 px, the sidebar breakpoint) as well as on touch, and the machine name never breaks: no-break spaces and a word joiner after each hyphen. At 430 px: `Runs on b` (`a-430-remote-chat-placeholder.png`).
- **(b)** With the owner offline every setter greys (Approvals, Agent messaging, Model, Effort) and Continue here stays enabled (`p2-a-offline-menu.png`). Move to shows "<owner> is offline — Continue here first" when it shows at all (two engines: the only target is the owner, so it is absent).
- **(c)** `resume_calls::resume_job_call` and the lost `wait` reply said "the engine was stopped … and restarted" after a transfer whose job row named no other engine; any transfer or takeover now says the session moved (`job_note_after_a_move_says_moved_without_an_engine_on_the_row`). Live, the mid-call move found the shell already promoted to a job (within ~1 s), so the interrupted path was not hit; the unit test covers it.
- **(d)** Built the sidebar Chat row's **▸** menu ("New chat on" each machine, only with paired peers) and the ⌘K empty-query group "New chat on <machine>" (`openNewChatOn` in `NewChatPane.tsx`), plus Effort ›. Nothing skipped.
- **(e)** `▶` → `►` in `crates/rpc/src/api.rs` (`cell_run`) and in the same cells sentence in `crates/cells/src/{types,actor}.rs`; the UI (`NoteCell.tsx`, `rpc/api.ts`) already said `►`.
- **(f)** Not real. `partial_upsert` runs only inside an open turn's stream; `remove_context` refuses unless the session is idle (`BUSY`), and the title job captures its revision at the end of a turn, after which the next turn opens with a user-row append that already bumps the revision. A partial therefore never invalidates a revision that an append would not. No change.
- **(g)** Covered live: a forwarded `create_session` owned by the target from epoch 1 (P6), a forwarded setter changing the owner's row (`sessions.model = fake/crawler-2` and `approvals = on` on B, P1), a pin that survives two real moves (P5: `approvals kept: Ask` both ways; B's `get_approvals` read `pin: true` right after the idle move, before any turn ran there). No rpc test: the forward tests pair transports, not engines with stores and registries.

### Gates

- `check` and `clippy --tests --no-deps -- -D warnings` on `-p agent -p rpc -p sessionsync -p mesh -p tools -p platform -p tui -p cells`: 0 errors, 0 warnings.
- Touched suites twice (same set, filter `session_pin job_note a_chat_created notes_spell_the_cause approvals pin forward`): 14 + 12 + 4 passed, both runs.
- Full `agent` with the worker env: 794 passed, 2 failed: `capping_binds_out_name` (known) and `talk_ending_releases_parked_items` (load-sensitive: passes alone twice). Full `rpc` 81, `sessionsync` 36 + 2, `mesh` 29, `tools` 217 passed, 0 failed.
- `tui` `no_emoji_in_desktop_chrome_strings`: passed. Bindings regenerated (8 passed); `tsc -b --noEmit` 0 errors; vitest 315 files / 2027 tests passed.

### Live proof (two scratch engines, loopback)

Rig `/tmp/rs183/rig/` (from `/tmp/taskhome`): A rpc 18561 / mesh 18661 / fake 18461, B 18562 / 18662 / 18462; `setup.sh` turns relay, discovery and every outside plugin off; B paired with `mesh_pairing_string` on A → `mesh_add`. `fake.py` streams `SLOW` over 20 s, answers `FAKE-SHELL` / `FAKE-SLEEP` with a `shell` call, and logs each request's `Machine:` / `Working directory:` lines and any `<session_moved/>` or `<engine_restart/>` it sees. vite dev on 18587 (A) and 18588 (B), headless Chromium on 127.0.0.1 only. Evidence `/tmp/rs183/evidence.txt`; screenshots `/tmp/rs183/shots/*.png` (all PNG).

| Proof | Result |
|---|---|
| P1 forward without take | PASS: send, approve, set_model and set_approvals from A; owner `b` epoch 1 throughout; B's fake ran each turn |
| P2 offline card | PASS: card 35 ms after Enter, nothing ran; Continue here → epoch 5 on A, the parked send ran on A |
| P3 live stream | PASS after the fix below; sidebar `Thinking` glyph on an unfocused B chat |
| P4 relayed approval | PASS: `b asks: shell` / `destructive · relayed from b · denies at …`; Allow on A ran on B; Deny → `denied by user`; answered on B → A's card gone in 88 ms |
| P5 moved card | PASS after the fixes below: MovedCard on both; B's next request carried `<session_moved to="b" from="a" os="macOS" … cwd=… approvals="ask"/>`; mid-turn move B→A: EngineRestartCard `Resumed on a after 9 s · moved from b · 1 call replayed`, 0 raw tags in either DOM |
| P6 new chat on B | PASS after the fix below: sidebar ▸ / ⌘K / New chat on [b ▾], Browse b… (forwarded completion), owner `b` epoch 1, `Machine: b (macOS, aarch64)`, `Working directory:` the chosen folder; a missing folder is refused `… does not exist on b` |
| P7 phone | Not driven: devices need the link plugin, which the network rule keeps off. Their mode-0 records enter the same Methods (`crates/link/src/conn.rs:796`), so the P1 front door and E's `classify_forwards_to_a_connected_owner_and_is_offline_otherwise` cover it |

### Defects found live and fixed

1. **A followed reply restarted every 5 s** (P3). Each replicated partial raised `ContextChanged{replicated}`; the settle swapped the live text for the greyed partial, and the next delta began a new reply. A settle retry could also drop the first delta. Fix: a replicated `ContextChanged` leaves the slice alone while an assistant text is open, and a settled page is refused while the replica reports the owner's turn open (`resume.here: false`) over an open text (`openTail`, `reduce.ts` / `sessions.ts`). The owner's `TurnFinished` settles. Tests in `reduce.test.ts`.
2. **"Running on b" never cleared** on a replica after the owner's turn ended (until a reload): `resumeFromLoaded` drops a replica's banner when the page has no resume.
3. **A mid-turn move's `moved` row rendered as a "Removed from context" tombstone** (it is context-excluded by design); `Message.tsx` renders it as its MovedCard.
4. **Two approval cards after a mid-turn move**, on both machines: the suspended turn's card (still parked in the old owner's approval actor) beside the new owner's re-ask. `App.tsx` withdraws a session's cards on `SessionOwnerChanged` (they precede the re-ask on both sides). The snapshot half is a HANDOFF.
5. **The moved banner said "Sending a message here brings it back"**, false since D1: now "…it runs there; what you send here runs there too." (wording test deleted).
6. **A new chat's folder was only recorded**: the session ran in the engine's own cwd. `new_root` seats a created root at its `cwd` when it exists there (`a_chat_created_in_a_folder_is_seated_there`).
7. **Header approvals chip** (reconcile table).

### HANDOFF

- *(fixed in R1, H1)* **The old owner keeps a moved turn's approval or ask pending** in its actor until `approval_timeout_secs`, and its `PendingPrompts` snapshot lists it: after a reload B showed its own dead card beside A's relayed re-ask. Shape: at release, drop the session's pending prompts in `Approval` / `Ask` (the waiter then publishes `ApprovalResolved`) without settling the `questions` row, which the taker re-asks from.
- *(fixed in R1, H2)* Interrupted-job notes and the job card name the origin by engine id (`moved here from 91f6…`, `owner changed (ran on 91f680ca…)`); §11 wants mesh names, which `resume_calls` cannot see today.
- *(fixed in R1, H3)* The taker's banner after a transfer reads `Taken from another engine … resumed here`: `TurnResumed.from` is not reaching the replica banner (139 text).
- In headless Chromium a Puppeteer mouse click on a Radix sub-menu item did not select it (the live proof used DOM clicks). [INFERENCE] harness-only; not seen with a real pointer.
- Pre-existing (E): three `tools` registry tests share the global `jobs` actor name.

## Contract changes after C (Phase 1 engine)

No TypeScript shape changed. The UI worker should know two meaning changes:

1. **`ApprovalsState.session` is now the effective value for the session**: its own or nearest ancestor's pin, else `tools.approvals`. Before 183 it was "the session did not opt out", which was `true` even with the global switch off. `pin` is the row's own `sessions.approvals`. A ⋯ switch that toggles `!state.session` therefore pins on with the global switch off (ask 7). The doc comment changed, so the regenerated `ApprovalsState.ts` comment differs.
2. **`set_approvals(session, true)` now pins `'on'`** instead of clearing the opt-out. `clear: true` returns to inherit (the "follow cloud-1 default" item).

Wire (engine-internal, no TS): an owner answers `Msg::Watch` with a first `Msg::Bus{kind: "Watching", seq: 0, bytes: []}`. Then it sends the session's approvals and asks that are already pending, so a focus after the ask still shows the card. A viewer that sent `Watch` and saw no frame at all treats the owner as pre-183 and uses `Events` until that owner reconnects.

## Phase 1 engine notes (E183)

Calls made where the design left room:

- **Front door.** `forward::route` forwards for every row owned by a connected engine (user sessions and desks), answers `-32010` when the owner is not connected, and never takes. A `Forwarded` call for a row owned elsewhere is `-32013 NotOwner` (`forward::admit_forwarded`). `session_move` on a viewer forwards to the owner, which makes its own `session_take` call to the target (a fresh extension set, so it is not refused as a second hop). `cell_attach` on a run forwarded to its owner, and `subscribe_pty` for a job of a session owned elsewhere, answer `-32014 RunsElsewhere`.
- **Relay.** A Bus frame is re-published locally as `sessionsync::events::Relayed`, and only the RPC bridge subscribes to it. The viewer's telegram, slack, signal, double and attention actors therefore never act on another engine's prompt. Relayed `JobEvent`s pass through the bridge's cell-kernel filter. `CellEvent::KernelChanged` names no session and is not relayed. Relayed cards are dropped when the follow stream ends or on unfollow. No synthetic `*Resolved` is published, so the UI withdraws them through its chip state or the next snapshot.
- **Focus follows every focused session.** The relay's follower returns at once for a row owned here, and starts following if the session later moves away.
- **`resolve_approval` / `answer_ask`** fall back to the relayed card's session when the `questions` row has not replicated yet.
- **`create_session{engine}`** forwards with `engine` cleared, then waits up to `sessions.release_wait_secs` for the row to replicate. A forwarded create whose `cwd` is not a directory on the target is refused: "`<cwd>` does not exist on `<machine>`".
- **`list_models(engine)` / `path_complete(None, …, engine)`** forward to that engine. A failed remote completion returns an empty completion, never this machine's paths.
- **Settings.** `ToolRegistry.bypass` became `pin: HashMap<SessionId, bool>`, and the nearest pin up the ancestor chain wins. The registry persists through a new `tools::Journal::approvals` method (default no-op), and the row's pin is applied at actor spawn by the new `tools::RestoreApprovals`, which does not persist. Communication: an opt-out is stored as `0` and an opt-in as NULL. `Store::session` (which returns the wire `SessionSummary`) is unchanged. `Store::session_settings(id)` reads the two columns.
- **Moved notice.** It is written by `Take::take` after `set_owner`, once per real ownership change. It goes in context only when no turn is open. With an open turn the row is UI-only, the model reads the extended `<engine_restart … to= os= cwd=/>`, and `from` is now the mesh name. A take that leaves the turn pending writes `pending: true` and replaces the old local "taken from" note. `runtime = reset` when the session has any `python` tool call. With no pin, `approvals` comes from the taker's `tools.approvals`. `seat::land` seats the taken session at its recorded cwd, or at the home directory when that is missing (W-9), and `sessions.cwd` is not rewritten.
- **Machine.** `platform::process::machine()` takes its name from the mesh (`set_machine_name` at mesh start), else the hostname. The first read fixes it for the process.
- **Partial redraw.** `partial_upsert` bumps `context_revision`. Risk: a title job or a `remove_context` that captured the revision before a 5 s checkpoint of the *next* turn is discarded as stale, as for any other mutation.
- **Phase 2 markers.** The supervisor's `PeerEnvelope.chain` and `PeerInfo.reachable` markers were labelled `183 P1: ENGINE` but belong to §6.5/§6.6. They are relabelled `183 P2:` and left alone.
- **Pre-existing test isolation bug (not C's).** Three `tools` registry tests (`slow_call_is_promoted_to_a_job`, `registration_adopts_a_running_untracked_invocation`, `lifecycle_barrier_waits_for_expected_and_collects_acks`) fail when run concurrently and pass with `--test-threads=1`. Cause: `slow_call` registers a live `Jobs` actor under the process-global name `jobs` (`registry.rs` test), and the other two do a global lookup of that name. C's diff touches none of this. The same failures reproduce at the parent commit 87f4acf0 (built from `git archive`).

## Contract commit (Phase 1)

Skeleton only: every shape of §14.1–§14.3 compiles and decodes old wire; no behaviour changes. Each place behaviour goes carries a one-line `// 183 P1: ENGINE` (or `// 183 P2:` for a Phase 2 field that ships as a shape now).

### Migrations

| Store | File | Content |
|---|---|---|
| agent | `crates/agent/migrations/0053_remote_sessions.sql` | `sessions.approvals TEXT NULL CHECK IN ('on','off')`, `sessions.communication INTEGER NULL CHECK IN (0,1)`, `messages.moved TEXT NULL` |
| mesh | `crates/mesh/migrations/0006_engine_os.sql` | `engine_stats.{os, os_name, arch, host} TEXT NOT NULL DEFAULT ''` |

§9's guessed numbers were free in this tree (agent newest was 0052, mesh 0005). Every column is nullable or defaulted. Nothing writes the session columns yet (NULL); `engine_stats` writes and reads the four fields (`Store::put_stats` / `Store::stats`), empty until the engine fills `stats::probe`. Tests: `agent::store::tests::remote_session_columns_default_null_and_check_values`, `mesh::store::tests::tables_attach_to_replication` (round-trip).

### Shapes (file:line at this commit)

| Area | Shape | Where |
|---|---|---|
| Front door | `NOT_OWNER = -32013`, `RUNS_ELSEWHERE = -32014` | `crates/rpc/src/forward.rs:53`, `:57` |
| | `FrontDoorError::NotOwner{owner}`, `::RunsElsewhere{owner}` | `crates/rpc/src/forward.rs:92`, `:97` |
| | `Route::Offline(PeerId)` (beside `Here`, `Forward`, and the old `Take`) | `crates/rpc/src/forward.rs:114` |
| | `classify(store, session, connected: &[PeerId])` | `crates/rpc/src/forward.rs:205` |
| Relay | `Msg::Watch{session, from_seq}` tag 11, `Msg::Bus{session, seq, kind, bytes}` tag 12 | `crates/sessionsync/src/types.rs:140`, `:148`, tags `:176-177` |
| | owner answers `Watch` as unknown (bails, stream closes: the old-owner fallback) | `crates/sessionsync/src/lib.rs:193` |
| | `events::RelayedPrompts{approvals, asks}`, `events::relayed_prompts()` (empty) | `crates/sessionsync/src/events.rs:102`, `:110` |
| Approvals | `SetApprovals.clear: bool` (serde default) | `crates/tools/src/registry.rs:450` |
| | `ApprovalsState.pin: Option<bool>` (serde default; always `None` yet) | `crates/tools/src/registry.rs:782` |
| Moved notice | `agent::SessionMoved` | `crates/agent/src/lib.rs:1821` |
| | `HistoryEntry::Moved{moved, context}` (role `moved`; never produced) | `crates/agent/src/lib.rs:3081` |
| New chat | `CreateSessionOptions.engine: Option<String>` (ignored) | `crates/agent/src/lifecycle.rs:74` |
| Messaging (P2 fields) | `PeerEnvelope.chain: u32` (serde default 0) | `crates/agent/src/communication.rs:599` |
| | `PeerInfo.reachable: Option<bool>` (skipped when `None`) | `crates/agent/src/communication.rs:406` |
| Mesh | `EngineStats.{os, os_name, arch, host}` | `crates/mesh/src/types.rs:215-224` |
| | `Peer.path: PeerPath`, `Peer.rtt_ms` (P2; `Offline`/`None` yet) | `crates/mesh/src/types.rs:110`, `:113` |
| | `PeerPath{Lan, Relay, Offline}` (lowercase, default `Offline`) | `crates/mesh/src/types.rs:122` |
| | `RelayState`, `NearbyEngine`, `PairCeremony` | `crates/mesh/src/types.rs:135`, `:149`, `:166` |
| | `MeshEvent::Nearby(Vec<NearbyEngine>)`, `MeshEvent::Pair(PairCeremony)` | `crates/mesh/src/types.rs:289`, `:291` |
| | `MeshStatus.stats` (empty yet), `MeshStatus.relay` (`None` yet) | `crates/mesh/src/types.rs:337`, `:341` |
| RPC | `path_complete(session, input, cursor, engine: Option<String>)` | `crates/rpc/src/api.rs:215` (impl `:4634`) |
| | `list_models(engine: Option<String>)`, now `with_extensions` | `crates/rpc/src/api.rs:726` (impl `:5091`) |
| | `set_approvals(session, enabled, clear: Option<bool>)` | `crates/rpc/src/api.rs:849` (impl `:5345`) |
| | `get_approvals(session: Option<SessionId>) → ApprovalsState` (new; local answer) | `crates/rpc/src/api.rs:858` (impl `:5370`) |

Callers updated for the new fields and params: `tui` (client calls, `HistoryEntry` arms), `schedule`, `signal`, `slack`, `telegram` (`SetApprovals`), `roster`, `sandbox`, `double`, `tasks` (test literals of `Peer`, `EngineStats`, `MeshStatus`), `rpc/src/acp.rs` (`MeshEvent` and `HistoryEntry` arms). New deps: `mesh → uuid` (`PairCeremony.id`), `sessionsync → tools` (`RelayedPrompts`).

### Deviations from §14, and calls made

1. **`Route::Take` stays** until the engine worker lands §4.1. Removing it now would change behaviour (D1 forward-not-take). `classify` takes `connected` and ignores it; `route` passes `&[]`; the `Offline` arm already answers `-32010`.
2. **`relayed_prompts()` takes no session and returns `sessionsync::events::RelayedPrompts`**, not `rpc_types::PendingPrompts`. No `rpc_types` crate exists; `PendingPrompts` lives in `rpc`, which depends on `sessionsync`, so returning it would be a cycle. §5.1 already calls it with no argument, and `PendingPrompts::collect` has no session in hand (it filters through `keep`).
3. **`ApprovalResolved` / `AskResolved` are unchanged.** The brief mentions a session id on `ApprovalResolved`. The design does not have one: §10.4 says the shapes are unchanged, and §5.1 keeps `prompts: HashMap<Uuid, SessionId>` in the owner's relay for exactly this reason. Adding the field would also touch 13 constructors across 6 crates.
4. **`MeshStatus` loses `Eq`.** `stats: Vec<EngineStats>` holds an `f32`. No caller needed `Eq`.
5. **TS optionality.** The new `Option` fields are `ts(optional)`: `ApprovalsState.pin?`, `CreateSessionOptions.engine?`, `MeshStatus.relay?`, `PeerInfo.reachable?`. Existing TS literals keep compiling, and the trailing RPC params are optional in `rpc/api.ts`. `PeerEnvelope.chain` is a plain `number` because ts-rs allows `optional` only on `Option`.
6. **`PeerInfo.reachable` is `skip_serializing_if = None`.** `discover_peers` output, which is model-facing, stays byte-identical until the engine fills it.
7. **`get_approvals` offline.** W-5 says the offline reader answers with `global: null`, but frozen §14.2 has `global: bool`. §14.2 wins. Call: when the owner is offline, `get_approvals` returns `-32010` like the setters, and the ⋯ menu renders its disabled offline line (§4.4, §12.2). The pin is read again after Continue here.
8. **`set_approvals` forwards `(session, enabled)` only.** `clear` is applied locally, and is not yet forwarded to the owner. The engine worker forwards all three params once owners take them.
9. **`platform::process::Machine` / `machine()` is not added here.** It is in §14.4, outside this contract's §14.1–§14.3. It is engine-internal (no TS), so the P1 engine worker adds it verbatim from §14.4 with §5.4.
10. **Phase 2 fields ship as shapes now**, because they are in §14.2–§14.3: `Peer.path` / `rtt_ms`, `RelayState`, `NearbyEngine`, `PairCeremony`, the `MeshEvent` variants, `PeerEnvelope.chain` and `PeerInfo.reachable`. They are marked `// 183 P2:` where they are filled.
11. **The TUI and ACP need a `Moved` arm today.** The TUI renders nothing (`crates/tui/src/lib.rs`, `model.rs::from_history`). ACP replays `[session moved to <to>]`. Both are marked ENGINE (`tui/src/model.rs` is the P1 engine worker's per §15).

### Bindings the UI worker imports

Generate: `BUILD_SLOTS=1 build-lock mbx test -p rpc --features ts --lib --quiet export_bindings`, then `cp -R bindings/. crates/desktop/ui/src/rpc/bindings/`. Keep the root `bindings/` too (`api.test.ts` reads `bindings/methods.json`).

New or changed files in `crates/desktop/ui/src/rpc/bindings/`: `SessionMoved.ts`, `HistoryEntry.ts` (`role: "moved"`), `ApprovalsState.ts` (`pin?`), `CreateSessionOptions.ts` (`engine?`), `EngineStats.ts` (+4), `MeshStatus.ts` (`stats`, `relay?`), `Peer.ts` (`path`, `rtt_ms`), `PeerPath.ts`, `RelayState.ts`, `NearbyEngine.ts`, `PairCeremony.ts`, `MeshEvent.ts` (`nearby`, `pair`), `FrontDoorError.ts` (`NotOwner`, `RunsElsewhere`), `PeerEnvelope.ts` (`chain`), `PeerInfo.ts` (`reachable?`), `methods.json` (`get_approvals`).

Hand-kept `crates/desktop/ui/src/rpc/api.ts`: `get_approvals(session)`, `list_models(engine?)`, `path_complete(…, engine?)`, `set_approvals(session, enabled, clear?)`, and `'get_approvals'` in `METHODS`.

UI edits forced by the types (each marked `// 183 P1: UI`): `store/reduce.ts` `historyEntries` gets a `moved` case that renders nothing, and `components/tasks/mock.ts` `MESH` gets `Peer.path` / `rtt_ms` and `stats: []`.
