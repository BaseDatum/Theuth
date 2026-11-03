# 176 LIVE-UNDER-LOAD: the desktop stays connected while many subagents run

Branch `integrate/176`, base main 6f5e32cc. Merged heads: feat/176-agents 20a7ac66,
feat/176-acp 8fa56019, feat/176-rpc c0142cf3, feat/176-clients 1cb0672c. All four merged
without textual conflicts; main had not moved, so there was no fold.

## Problem

With many subagents active the desktop UI showed "Connection lost, reconnecting…" and reloaded.
Root cause, measured on the live engine on 2026-11-03:

1. The UI's liveness probe (`probe()`, rpc/client.ts, `PROBE_MS = 5_000`) sent `info` and called
   `reset()` on a healthy socket when no reply came within 5 s. It fired on visibilitychange /
   pageshow and after a 15 s chat send timeout. Main window, island and every pop-out each ran it.
   A reset meant toast, 500 ms redial, full `reload()`, and the LOCAL router unmounting to the
   StartupScreen.
2. `info` awaited 13 asks in sequence: 10 to the Agents supervisor, 2 to ToolRegistry, 1 to
   schedule. kameo 0.22 runs one handler at a time per actor, unbounded mailbox, no ask timeout.
3. The Agents `ListActivity` handler ran inline and called `store.last_turn_text(id)` per row,
   which replayed the whole transcript. Each call held the Agents mailbox for 0.96-6.9 s
   (≈164 rows, 131 KB reply). A trivial ask (`compact_style`) waited behind it; otherwise it
   answered in ≤1 ms. Over 10 min `info` had p50 1 ms, p99 25.6 s, max 29.6 s, 15 of 299 calls
   over 5 s. jsonrpsee's own unknown-method reply stayed ≤10 ms: socket, runtime and jsonrpsee
   were healthy.
4. The steady caller was the macOS companion's `Store.refetch()`, on every turn / spawn /
   approval / ask / attention / Lagged event (250 ms debounce), firing six list RPCs including
   `list_activity` and `list_sessions` (2.39 MB). More subagents → more events → back-to-back
   ListActivity. ListAgents/Roster, LoadSession, ListPeers (Mesh `Status`, `COUNT(*) FROM outbox`
   up to 2 s), SpendReportQuery (HUD, 60 s) and SessionContextBreakdown (30 s) were also inline.
5. WebKit's log showed five UI socket drops that day (17:38:52, 17:43:41, 19:17:41, 19:17:42,
   19:33:04), the last inside a measured 29.6 s Agents stall; each redialled 500 ms later.

Also in scope: ONNX Runtime's INFO logger flooded the engine log through `ort::logging`
(840k lines, 122 MB on 2026-11-02), and idle external ACP children (15 `t mcp --session` bridges,
idle up to 2 h 52 min) were never stopped.

Ruled out: event flood (20 frames/s, max 32 KB, no Lagged), message size, server-side closes,
Spotlight.

## Contracts

- **C1 `ping`**: params `[]`, result `null`. Asks no actor, awaits nothing.
- **C2 `client_log`**: params `[level, source, message]`; `level` "warn" → WARN, else INFO;
  `source` ≤32 chars `[a-z0-9-]` (else logged as `-`); `message` cut to 2,000 chars. Logged with
  target `client`, fields `source`, `conn`. 20/min per connection, then dropped with one WARN per
  minute. Asks no actor.
- **C3 client tag**: optional `client` query parameter on the engine WebSocket URL, same charset,
  in the `rpc: ws open` / `rpc: ws closed` lines. `desktop-main`, `island`, `popout`,
  `companion`, `mcp-bridge`.
- **C4 `acp.idle_stop_secs`**: integer, default 1800, 0 disables.

## Decisions and what each branch changed

### feat/176-agents — the supervisor never holds its mailbox for I/O
- **E1** ListActivity, ActivityOf, ListAgents, Roster, ListSessions, ReadSession, LoadSession,
  SessionContextBreakdown, SessionCostQuery, SpendReportQuery return `DelegatedReply`; the
  handler clones a `View` (in-memory state the reply reads) and a spawned task does store SQL and
  the `JobCounts` ask — the pattern Compact / Harvest / LedgerSince already used. Handlers that
  need state afterwards are split: ListPeers (read off-mailbox, `PeersRead` applies), LoadSession
  (`Mirrored` applies lineage). Reason: replies keep rows/order/fields; the only change is the
  snapshot instant.
- **E2** `last_turn_text` reads the transcript tail in 64-row pages back to the last exchange and
  caches the result in a new local-only table `last_texts(session, rev, text)` keyed by
  `context_revision`. Reason for a table, not the briefed `sessions` columns: `sessions` is
  replicated to mesh peers, and seqs are reused after undo, so a seq key could serve stale text.
- **E3** `GetInfoDefaults` returns every Agents field `info` needs in one ask; `info` JSON shape
  unchanged.
- **E4** (one recursive CTE for `list_sessions` roots) skipped.
- **E5** proof test: with a 1 s test-only store read delay, GetCompactStyle answers <100 ms
  during ListActivity / ListAgents / LoadSession; with the old inline handlers it failed
  ("ListActivity: GetCompactStyle took 953.556083ms").
- Files: `crates/agent/migrations/0050_last_texts.sql`, `crates/agent/src/{lib.rs, store.rs,
  supervisor.rs, supervisor/drain.rs, supervisor/view.rs}`, `crates/rpc/src/api.rs` (body of
  `info`, unused `Get*` imports).

### feat/176-acp — idle external harness children park and resume
- **A1** `acp_child/idle.rs`: pure `parkable()` over `IdleFacts` (turn in flight, pending
  approval/ask, own job/PTY, watch-only subagent running via `observed.rs::any_running()`, ACP
  traffic within `idle_stop_secs`). A 60 s ticker sends `ParkIdle`; the handler only snapshots
  candidates into `Agents.parking` and spawns `park_round` (Jobs ask 2 s timeout, `ParkIfIdle`
  to all candidates concurrently, 2 s each), which reports `ParkRound`; that removes the live
  entry and records it in `parked` only if the entry is still the same actor. Logs
  `acp: parked idle child session=… harness=… idle=…s`.
- Inputs routed to a child while its round is in flight are held in `parking[id]` and replayed
  after `ParkRound` (kameo `ctx.stop()` drops messages queued behind `ParkIfIdle`).
- **A2** resume goes through the existing `session()` → `acp_session` path (`session/load`, as
  `relaunch_acp_child(Idle)`), logging `acp: resumed parked child session=… after=…s`; fallback
  to a fresh session logs WARN. Session delete clears `parked`/`parking`.
- **A3** `t mcp --session` bridge exits on stdin EOF, parent death (`getppid()` polled every
  2 s), engine "no such session", or 60 s of engine down (redial with backoff before that); it
  dials with `client=mcp-bridge`.
- **A4** `acp.idle_stop_secs` read from the top-level `acp:` section at supervisor start;
  documented in `crates/theuth/harness.md`.
- Files: `crates/agent/src/{acp_child.rs, acp_child/idle.rs, acp_child/observed.rs, lib.rs,
  supervisor.rs}`, `crates/theuth/{harness.md, src/mcp.rs}`.

### feat/176-rpc — liveness, connection logs, slow handlers, quiet ORT
- **R1** C1 and C2 at the end of the `Api` trait and `impl ApiServer for Server`;
  `crates/rpc/src/diag.rs` holds the sink, rate limit, truncation (control chars → spaces) and
  the shared tag validator.
- **R2** `crates/rpc/src/wslog.rs`: `rpc: ws open conn=<n> client=<tag|-> peer=<addr>` and
  `rpc: ws closed conn=<n> client=… after=… reason=close(<code>)|error(<kind>)|eof`; a
  process-wide counter, a `Watched` TCP wrapper that records the first I/O error and parses the
  client close code from frames. Token never logged; `ConnId` stored in request extensions.
- **R3** `crates/theuth/src/logs.rs`: a `SlowHandlers` layer timing only
  `actor.handle_message` spans; ≥1,000 ms → WARN target `actor`
  `slow handler actor=… message=… took=…ms`.
- **R4** `engine_filter()` puts `ort=warn` before RUST_LOG directives so an explicit
  `RUST_LOG=ort=…` wins; `models/src/onnx.rs` sets ORT's environment log level to Warning after
  commit (`ORT_LOG` env skips that).
- Files: `crates/models/src/onnx.rs`, `crates/rpc/src/{api.rs, diag.rs, lib.rs, wslog.rs}`,
  `crates/theuth/src/{logs.rs, main.rs}`.

### feat/176-clients — clients stop dropping healthy sockets and stop hammering
- **U1** `probe()` sends `ping`; any inbound frame after the probe starts (reply, error reply,
  notification) cancels the reset.
- **U2** LocalTransport does not probe an open socket on foreground; LinkTransport keeps its
  probe (now ping) and watchdogs. The Composer send-timeout probe stays on both.
- **U3** LOCAL keeps the router mounted for 10 s while reconnecting ("Reconnecting…" in
  LinkBanner); the "Connection lost" toast waits 3 s; "Reconnected" only if it was shown.
- **U4** reconnect gap <30 s skips `loadInfo` and the extra `list_sessions`; foreground reseed
  only after >30 s hidden.
- **U5/U6** drop reason (`probe-timeout`, `send-timeout`, `onclose code=… clean=…`,
  `foreground-reset`) sent once via `client_log` after reconnect; URL carries the C3 tag.
- **K1-K3** companion `RefetchPacer`: 250 ms debounce from the first event, ≥2 s between starts;
  `list_sessions` every 30 s and on PendingPrompts; `client=companion`; backoff reset after 10 s
  up or a successful call; 5 s engine-banner grace. `maximumMessageSize` confirmed raised.
- Files: companion `Sources/Companion/Store.swift`, `Sources/CompanionCore/{Commit,Policy}.swift`,
  `Sources/TheuthKit/RpcClient.swift`, `Tests/CompanionCoreTests/{Parity,Policy}Tests.swift`;
  UI `src/App.tsx`, `components/chat/Composer.tsx`, `components/link/LinkFailure.tsx`,
  `rpc/{api.ts, client.ts, client.test.ts, transport.ts}`, `store/{connection.ts,
  connection.test.ts}`.

## Deviations reported by workers

- Agents: cache table instead of `sessions` columns, keyed by `context_revision`; ListPeers asks
  Jobs counts for every row; cold sessions take last-active from the row; E5 via a test delay hook.
- ACP: stop grace is 1 s, not 5 s (ACP SDK 3.6.0 closes → 1 s → SIGKILL of the process group and
  exposes no pid); `parked` is not persisted — a parked child has no open journal turn, so a
  restart already treats it as idle; dialled node children, connecting/failed children and
  children with a pending respawn or option change are never parked; no Jobs actor → zero jobs;
  a probe over 2 s skips the round; the catalogue bridge (`t mcp` without `--session`) only has
  the EOF and parent-death exits. Listings show a parked child as Idle, `live: false`.
- RPC: own WebSocket frame parser for close codes; `ORT_LOG` escape hatch; invalid tags logged as
  `-`; `update_cli` subscriber untouched.
- Clients: `client_log` only from local clients; "Reconnecting…" lives in LinkBanner; a failed
  first dial is not a drop; `list_live_counts` still runs on a cheap reconnect; the private
  session 30 s window is unchanged (the engine starts it on close).

## Gates (integrate/176, build label 176-z)

- `cargo fmt -p agent -p rpc -p models -p theuth`: no diff.
- Bindings export: 8 passed; `methods.json` lists `client_log` and `ping` from the Rust export.
- `mbx test -p agent -p rpc -p models -p theuth --no-fail-fast` with `THEUTH_MONTY_WORKER` /
  `THEUTH_WORKER` = the fresh `target/debug/t`: agent lib 668 passed, 1 failed, 4 ignored;
  notebooks_binding 2; notebooks_block 7; models lib 26; trim 0; ort_load_failure 1; rpc lib 68;
  alerts 1; prompts 1; t 11; doc-tests 0. The one failure,
  `repl::tests::capping_binds_out_name`, reproduces on main 6f5e32cc (worktree check): Python's
  repr of the `list_dir` help switches to double quotes because the tool doc contains
  "session's". Pre-existing, outside 176.
- `mbx clippy … --all-targets`: stops on the pre-existing `voice/src/history/mod.rs:465`
  `chunks_exact_to_as_chunks`; with `-- -A clippy::chunks_exact_to_as_chunks`, 0 warnings.
- UI: `tsc --noEmit` clean; `vitest run` 278 files, 1,784 tests passed.
- Companion: `swift build` ok; `./test.sh` 291 tests in 46 suites passed.

## Scratch-engine smoke (debug `t`, fresh data dir, own port, no providers)

| Step | Result |
|---|---|
| (a) `ping` ×50 | PASS: `null`, p50 0.19 ms, max 1.6 ms |
| (b) `client_log ["warn","desktop-main","176 smoke"]` | PASS: one `WARN client: 176 smoke source="desktop-main" conn=1` |
| (c) `&client=desktop-main` socket | PASS: `rpc: ws open conn=1 client=desktop-main peer=…`, `rpc: ws closed conn=1 client=desktop-main after=0.0s reason=close(1000)` |
| (d) `info` keys | PASS: identical 18-key set to the installed app binary run on a second scratch dir |
| (e) `compact_style` every 200 ms, `list_activity` every 2 s, 60 s, 250 empty sessions | PASS: 297 calls, p99 1.2 ms, max 2.8 ms (list_activity p50 24 ms, 125 KB). Same run on the app binary: p99 20 ms, max 37.6 ms. Weak evidence: sessions have no transcripts; E5 is the deterministic proof |
| (f) no `ort::logging` INFO | PASS but weak: no ONNX model loaded (no models in the scratch dir); W-RPC's `--bench-models` run showed 3 lines vs 933 with `ORT_LOG=info` |

## Follow-ups

- Still on the Agents mailbox: Ancestors, ContextItems, SessionMemoryQuery, EndlessZoom,
  ShowPrompt, GetCommunication{session}, Broadcast dry-run.
- E4: `list_sessions` correlated `root_col!` subqueries → one recursive CTE.
- A delete racing a spawned read can re-learn lineage for a deleted session.
- Migration number 0050 may clash with other in-flight designs.
- No explicit `parked` state in listings.
- A UI reconnect re-issues all subscriptions, so PendingPrompts → `seedLive` runs twice.
- `RUST_LOG=debug` is still overridden by the forced `info` directive (pre-existing).
- Pre-existing: voice clippy error; `repl::tests::capping_binds_out_name` fails on main.
