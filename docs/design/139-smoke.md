# 139 — two-engine smoke: the §11 Z matrix against two real engines on one Mac

*Wave Z, 2026-10-08. Branch `feat/139-z` (cut from the integrated `e07fca4e`). Companion:
`139-as-built-Z.md`. Shape: `68-two-engine-smoke.md` §1 and `109-as-built-W6.md` §8. Not CI.*

## 0. Result

Full run (`scripts/durability-mesh-smoke.sh`, every row, one `t` built from this branch at
`cb772c58`): **76/76 checks, 185.7 s wall**, engines A (`8ae7900b…`) and B (`eadd1df7…`). Nine
rows: T1–T8 of §11 Z plus **T2c**, the native-child variant of T2. T9 (D15) is in-process
(`crates/sessionsync/tests/mesh_durability.rs`, no engine needed).

| row | invariant (§4) | result | key numbers |
|---|---|---|---|
| T1 replica boots mid-turn | I1, I2 | ✓ | B restart → 0 `TurnResumed`/`ResumePending`, 0 own log rows, no report; A's turn finished uninterrupted |
| T2 transfer mid-batch | I3, I4, I8 | ✓ | take 5.0 s (= `release_wait_secs − 1`); `suspended{release}` on B at epoch 1; one `TurnResumed{transfer, from: A, suspended}`; orphans 0; mutating `shell` interrupted, hits = 1; transcript shape = control |
| T2c native child | §8 Children, 69 P | ✓ | child on A, parent taken by B; child's `<message from="session:…">` on B 15 s later; B ran a turn on it |
| T3 takeover, partition | I5, I6, I8 | ✓ | B sees A unreachable after 33.8 s; take 6 ms; `TurnResumed{takeover, interrupted}`; `shell` interrupted, `wait` re-run → lost PTY banner; A ran demote via **snapshot**; tail orphaned (`181–195`) *and* dropped StaleEpoch on B; A's PTY group alive |
| T4 Write take (Q10) / system take | I5 | ✓ | `send_input` on B → `TurnResumed{takeover}` at once; `session_take(orphan)` → `ResumePending`, Attention `resume:<s>:<t>` "taken from A", System note, nothing ran; `resume_turn` → `TurnResumed{takeover, from: A}`, item cleared |
| T5 ask re-arms | I7 | ✓ | same `ask:<id>` on B after the take; `answer_ask` on B → true, call settled "blue"; A's row `answered` |
| T6 jobs + PTY | I6 | ✓ | `list_jobs` on B: both `lost`; `pty_screen` on B: stored page, `epoch: null`; `screen()` → `lost: True` + A's rows; A's two groups dead (KillSession) |
| T7 replica display + D18 | I2, D18 | ✓ | B's `Loaded.resume = {owner: A, here: false, resumed: false}`, `running: false`, no `resume:` item; `resume_turn` on B takes (Q15) → `TurnResumed{transfer}`; `answer_ask` with A down → `-32010`, with A back → forwarded, A finished |
| T8 input mid-drain | D16 | ✓ | `send_input` on A during the release drain → `-32012 {SessionDraining{reason: release}}`; the text is in no transcript |

## 1. Recipe

- `scripts/durability-mesh-smoke.sh [T1 …]` → `bun scripts/durability-mesh/smoke.ts`; `T_BIN`
  (default `target/debug/t`), `SMOKE_DIR` (`/tmp/139-z`, wiped), ports 7931/7932 (rpc),
  7941/7942 (`mesh.bind`), 7950 (fake model).
- Model: `scripts/durability-mesh/fake-openai.ts`, an OpenAI-compatible SSE server added to both
  engines as gateway `fake` (`add_provider`), model `fake/batch` via `set_model`. A user message
  `SMOKE {"calls":[{name,args}…]}` is answered with exactly that tool batch; a request whose turn
  already carries a tool result gets `done.`. The 109 `BatchFake` rule, driven by the prompt.
- Held calls: `shell("echo hit >> hits; sleep N")` with `tools.job_grace_secs = 120` keeps the call
  `started` for N s; `wait(id, "NEVER-MATCHES", 100)` on a PTY from `start(.., ready: {log})`
  holds a read-only call. Direct `shell`/`start`/`wait`/`screen` calls are **bridged** into python
  cells (`repl::bridge_cell`), one cell at a time per session — so T3 holds the mutating call in
  one session and the read-only one in another.
- Config per engine: `mesh {relay: none, discovery: false, bind}`, `agent.communication: true`,
  `tools {approvals: false, job_grace_secs: 120, login_path: false}`, `sessions
  {release_wait_secs: 6, takeover_secs: 0}`, `durability {resume: auto, drain_secs: 10,
  pty_snapshot_secs: 1}`.
- Partition = `SIGSTOP` on A until B's `mesh_status` says `reachable: false` (QUIC idle, 31–35 s
  measured). T3 then stops B, resumes A (A finishes its turn alone, writing its tail), resumes B.
- Evidence sources: RPC replies, `subscribe_events` streams of both engines, `sessions.sqlite` of
  both (`turn_steps`, `tool_calls`, `jobs`, `questions`, `session_log`, `resume_reports`), the
  engines' `logs/theuth.log.*`, `kill -0 -pgid` for process groups.

## 2. Every check of the full run

| row | check | result | detail |
|---|---|---|---|
| T1 | B published no TurnResumed and no ResumePending | ✓ |  |
| T1 | B's boot log resumed nothing | ✓ | (no open-turn line) |
| T1 | B wrote nothing: no own log rows, no resume report | ✓ |  |
| T1 | A's transcript uninterrupted | ✓ | user tool:read_file tool:shell tool:list_dir assistant |
| T1 | B still a replica at epoch 1 | ✓ | {"owner":"A","epoch":1} |
| T2 | session_take(user) on B → owner B, epoch 2, here | ✓ | {"owner":"B","epoch":2,"here":true} in 5025 ms |
| T2 | B TurnResumed{cause: transfer, from: A, kind: suspended} | ✓ | {"cause":"transfer","from":"A","kind":"suspended"} |
| T2 | suspended{release} is on B (applied at epoch 1 — a later arrival would be StaleEpoch) | ✓ | release |
| T2 | A published TurnSuspended{release} | ✓ | {"type":"TurnSuspended","session":"01a11a6c-febe-7be3-adca-a7bb7784285c","turn":"01a11a6c-fec4-79e1-980e-c06334828af6","reason":"release"} |
| T2 | A demoted with session_orphans = 0 | ✓ | [] |
| T2 | B published exactly one TurnResumed | ✓ |  |
| T2 | shell (mutating, started) reported interrupted with the moved-here job note | ✓ | ERROR: interrupted: the session moved here from A at 08:31:53 while this command ran (job j1, started 08:31:53); the job stayed on A and its exit code is unknown here … |
| T2 | mutating call ran exactly once across A and B | ✓ | 1 hit(s) |
| T2 | read_file output reused (finished row) | ✓ | note: read_file is a runtime function, not a direct tool; it ran for you as python (await read_file(...)). Next time call it from python yourself — `r = await read_fil… |
| T2 | <engine_restart kind="transfer" from=A> heads the resumed batch | ✓ | <engine_restart at="08:31:58" after="5s" kind="transfer" from="A" replayed="0" interrupted="1"/> |
| T2 | resume_reports row on B: cause transfer, shell interrupted, lost job | ✓ | {"cause":"transfer","replayed":"[]","interrupted":"[[\"smoke_1_shell\",\"shell\"]]","lost_jobs":"[\"j1\"]"} |
| T2 | transcript shape equals the uninterrupted run's (modulo notes) | ✓ | taken: user tool:read_file tool:shell tool:list_dir assistant \| control: user tool:read_file tool:shell tool:list_dir assistant \| finals "done." / "done." |
| T2 | job row lost on B with engine = A | ✓ | [{"id":"j1","kind":"shell","pgid":7878,"state":"lost","exit":"lost","engine":"A"}] |
| T2 | A's shell process group dead after the transfer | ✓ | pgid 7878 |
| T2 | A's copy equals B's (B's rows overwrote A's) | ✓ | user tool:read_file tool:shell tool:list_dir assistant |
| T2c | child spawned on A, owned by A | ✓ | 01a11a6d-19e8-78c1-b653-ef162115fc20 |
| T2c | parent resumed on B as a transfer | ✓ |  |
| T2c | child still owned by A after the parent's take (children are not co-owned) | ✓ |  |
| T2c | the child's result arrived in the parent's transcript on B | ✓ | <message from="session:01a11a6d-19e8-78c1-b653-ef162115fc20" id="01a11a6d-6932-75a0-9d4e-b83ffbe34b87" at="08:32"> |
| T2c | B ran a turn on the delivered result | ✓ | 2 TurnFinished on B |
| T3 | a PTY job runs on A | ✓ | {"id":"j1","kind":"pty","pgid":8821,"state":"running","exit":null,"engine":"A"} |
| T3 | session_take(user) on B with A partitioned → B, epoch 2 | ✓ | {"owner":"B","epoch":2,"here":true} in 6 ms |
| T3 | B TurnResumed{cause: takeover, from: A, kind: interrupted} | ✓ | {"cause":"takeover","from":"A","kind":"interrupted"} |
| T3 | no suspended row shipped | ✓ |  |
| T3 | A's started mutating `shell` reported interrupted (takeover wording) | ✓ | ERROR: interrupted: the session moved here from A at 08:32:20 while this command ran (job j1, started 08:32:20); the job stayed on A and its exit code is unknown here … |
| T3 | started read-only `wait` re-run on B: lost PTY, moved-from-A banner over A's last page | ✓ | [process gone — the session moved from A at 08:32:20; thi |
| T3 | <engine_restart kind="takeover"> note on both | ✓ |  |
| T3 | read_file output reused (finished row) | ✓ |  |
| T3 | A wrote a tail at epoch 1 while partitioned | ✓ | 14 log rows |
| T3 | mutating call ran exactly once (A's run; B did not re-run it) | ✓ | 1 hit(s) |
| T3 | A ran demote when B's take reached it (log row or snapshot) | ✓ | 2026-10-08T07:32:55.788302Z  INFO actor.handle_message{actor.name="SessionSync" actor.id=#28 sessionsync::owner::Demote}: sessionsync: sessions: 01a11a6d-6966-70f1-a44… |
| T3 | A's tail discarded: in session_orphans or dropped as StaleEpoch on B | ✓ | orphans=[{"epoch":1,"from_seq":181,"to_seq":195,"at":"2026-10-08T07:32:54.865+00:00"}] stale-dropped-on-B=true |
| T3 | A's transcript is B's (nothing merged) | ✓ | user tool:read_file tool:shell assistant |
| T3 | A's PTY group still alive after the takeover (B never signalled it) | ✓ | pgid 8821 |
| T3 | B lists A's PTY job lost with engine = A (settle_transferred, no signal) | ✓ | [{"id":"j1","kind":"pty","pgid":8821,"state":"lost","exit":"lost","engine":"A"}] |
| T4 | (Q10) send_input on B took the session and resumed at once: TurnResumed{takeover} | ✓ | {"cause":"takeover","from":"A"} |
| T4 | (Q10) no ResumePending for the client's write | ✓ |  |
| T4 | session_take(orphan) on B → B owns it | ✓ |  |
| T4 | system take → ResumePending{from: A}, no TurnResumed | ✓ |  |
| T4 | B's attention_list has resume:<session>:<turn> with the takeover title | ✓ | resume:01a11a6d-f3da-7b61-b76e-435401f6999e:01a11a6d-f454-7990-909e-0336b329d691 01a11a6d was taken from A |
| T4 | local note row: taken from A … press Resume | ✓ | {"role":"System","content":[{"Text":"taken from A at 08:3 |
| T4 | newest step still open on B (nothing ran) | ✓ |  |
| T4 | resume_turn on B → TurnResumed{cause: takeover, from: A} | ✓ | {"ok":true,"cause":"takeover","from":"A"} |
| T4 | the resume: item cleared | ✓ |  |
| T4 | A demoted both | ✓ |  |
| T5 | question row replicated pending | ✓ | 01a11a6e-8347-78c3-85a1-9afd09a2532b |
| T5 | A lists ask:<id> | ✓ |  |
| T5 | B (replica) lists no ask item and no resume item for it | ✓ |  |
| T5 | B's attention_list has ask:<same id> after the take | ✓ | {"id":"ask:01a11a6e-8347-78c3-85a1-9afd09a2532b","kind":"ask","title":"01a11a6e asks: Which?","detail":"red · blue","at_ms":1791444817826,"task":null,"agent":nu |
| T5 | answer_ask on B → true | ✓ |  |
| T5 | the call settled with the answer on B | ✓ | <engine_restart at="08:33:37" after="5s" kind="transfer" from="A" replayed="0" interrupted="0"/> |
| T5 | A's questions row is answered after replication | ✓ |  |
| T6 | two running jobs on A, engine = A | ✓ | [{"id":"j1","kind":"pty","pgid":9379,"state":"running","exit":null,"engine":"A"},{"id":"j2","kind":"shell","pgid":9385,"state":"running","exit":null,"engine":"A"}] |
| T6 | B's list_jobs shows both jobs lost | ✓ | [{"session":"01a11a6e-9819-77f2-9f9e-77b591d0f52a","id":"j1","command":"printf 'HELLO-T6\\n'; sleep 300","state":"lost","secs":6,"log":"/tmp/139-z/A/jobs/01a11a6e-9819… |
| T6 | pty_screen on B: the replicated page (stored_page, epoch null = lost) | ✓ | updated \| HELLO-T6 |
| T6 | screen() on B: the PTY is lost, A's last page served | ✓ | → {'job': 'j1', 'rows': ['HELLO-T6'], 'cursor': [1, 0], 'lost': True, 'restarted_at': '2026-10-08T07:33:45.125328+00:00'} |
| T6 | resumed shell call carries the owner-changed job note | ✓ | <engine_restart at="08:33:45" after="5s" kind="transfer" from="A" replayed="0" interrupted="1"/> |
| T6 | A's process groups dead after the transfer (KillSession) | ✓ | j1:9379,j2:9385 |
| T6 | rows lost/lost with engine = A on B | ✓ | [{"id":"j1","kind":"pty","pgid":9379,"state":"lost","exit":"lost","engine":"A"},{"id":"j2","kind":"shell","pgid":9385,"state":"lost","exit":"lost","engine":"A"}] |
| T7 | B's load_session: resume.owner = A, here = false, kind interrupted, resumed false | ✓ | {"turn":"01a11a6e-b502-77c3-b7e4-c6b70d4480d4","kind":"interrupted","interrupted_at":"2026-10-08T07:33:45.368590Z","resumed":false,"report":null,"owner":"A","here":fal… |
| T7 | B's Loaded.running is false (process-local flag) | ✓ |  |
| T7 | B's list_sessions row: owner A, here false | ✓ | {"id":"01a11a6e-b4fd-7ee2-b551-f8acdea7b8a0","created_at":"2026-10-08T07:33:45.341352Z","updated_at":"2026-10-08T07:33:45.368310Z","title":"SMOKE {\"calls\":[{\"name\"… |
| T7 | B raises no resume: item for A's turn | ✓ |  |
| T7 | A's load_session: here true, resumed true (its live turn) | ✓ | {"turn":"01a11a6e-b502-77c3-b7e4-c6b70d4480d4","kind":"interrupted","interrupted_at":"2026-10-08T07:33:45.368590Z","resumed":true,"report":null,"owner":"A","here":true… |
| T7 | resume_turn on B (A up) takes the session: TurnResumed on B, from A | ✓ | ok=true cause=transfer |
| T7 | answer_ask on B for A's question with A down → -32010 OwnerUnreachable | ✓ | {"code":-32010,"message":"owner 8ae7900b unreachable: not connected","data":{"OwnerUnreachable":{"owner":"A","last_seen":1791444812430}}} |
| T7 | answer_ask on B with A back is forwarded and settles A's call | ✓ | {"result":true} |
| T7 | A finished the turn with the forwarded answer | ✓ |  |
| T8 | send_input on A mid-drain → -32012 SessionDraining{reason: release} | ✓ | {"code":-32012,"message":"01a11a6f-4a1c-7a31-8843-66a04d35a710 is moving to another engine — retry in a moment","data":{"SessionDraining":{"session":"01a11a6f-4a1c-7a3… |
| T8 | the take completed | ✓ |  |
| T8 | the refused input is in no transcript | ✓ |  |

## 3. What the matrix found (fixed as `139 Z-fix:` commits, each re-run green)

1. **Bridged job ids** (`750e03f5`). A direct `shell`/`start` runs as a python cell; its `jobs.call_id`
   is `<call_id>.<n>`. `resume_job_call` matched by equality → "no row → never spawned" → the
   mutating `started` call was **re-run on B** (T2 first run: `shell` attempt 2, a second `sleep 90`
   on B, 2 hits). The reaper's / `settle_transferred`'s `in_batch` exclusion missed the same way →
   a duplicate `<job_result exit="lost">` user row. `durability::jobs::job_of_call` + 3 call sites.
2. **Child results after a parent transfer** (`9a3beb33`). `hand_off → deliver_result → enqueue_peer`
   spawned a local actor for the parent (now owned by B) whose intake refused the row
   (`PeerIntakeFailed: session … is owned by B`); the result never reached B. `deliver_result` now
   forwards through `forward_peer` when `remote_owner(parent)` is set (69 P). T2c.
3. **Pending takeover resumed as `crash`** (`2a743524`). The Attention item's `resume_turn` on the
   new owner sent `cause: None` → boot rule → `TurnResumed{cause: crash, from: None}`; `on_resume_here`
   (jobs/child-waits settle) never ran. `Agents.pending_takeovers` filled by `Take::run`'s Pending arm
   (`TakeoverPending`), consumed by `resume_turn`. T4.
4. **The taker's note hid the dangling batch** (`2a743524`). `append_local_note` writes a `System` row
   above the assistant `tool_use`; `dangling_calls()` returned empty on it → the resume opened a fresh
   round → `UNIQUE constraint failed: tool_calls` and the whole batch, mutating `shell` included,
   **ran again** (60 s). `dangling_calls` skips `System` rows like `User` ones. T4.
5. **No demote from a snapshot** (`ef6ccafc`). After the partition B re-synced A from a snapshot
   (`receive_snapshot`), which published `SessionOwnerChanged` but never ran `on_applied` → A kept its
   actor (its writes refused `NotOwner` until the `wait` timed out), no orphan accounting, no note.
   `snapshot::apply` now reports the local fence before each applied `sessions` row and
   `receive_snapshot` calls `on_applied`. T3.

Found, not fixed (owning wave / closer): §4 of `139-as-built-Z.md`.
