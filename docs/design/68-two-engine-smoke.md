# 68 — two-engine smoke: two engines on one Mac, one lone engine, one migrated store

*Worker Z, 2026-09-25. Proves 68 §11 (single engine) and the two-engine story (§4–§7) on the integrated `feature/68-sessions` branch (all workers merged at `bf18784`), and records what had to change. Companion: `68-as-built-Z.md`. Extends `67-single-engine-smoke.md`; build-A was out of scope (Sky's ruling): both engines ran on this Mac.*

## 0. Result

| # | Invariant | Result | Evidence (§2) |
|---|---|---|---|
| 1 | Lone engine: log written, pruned to ≤ one snapshot, no shipping | ✓ | 12 rows after one turn → 1 (head) after the 60 s prune tick; zero sessionsync lines |
| 2 | Session RPCs byte-identical to `main` (654febe) | ✓ (+`owner/epoch/here` on `list_sessions` rows, by design) | `diff` of 15 calls: only those three fields |
| 3 | Append latency unchanged within noise | **✗ measured** | release in-process +78 µs/append (85 → 163 µs); debug RPC-level +≈300 µs/write |
| 4 | 126 MB store migrates < 2 s, `owner = self, epoch = 1` everywhere | ✓ | migration 3.8 ms, store open 0.44 s, 173/173 `self/1/true` |
| 5 | Chat on A appears on B within 1 s of each turn | ✓ | user row on B 127 ms after `send_input`; assistant row on B ≤ A (1801 vs 1839 ms, poll jitter) |
| 6 | "Continue here" on B → epoch 2 on both, B runs the next turn, A shows the chip | ✓ after fix | take 53 ms; first turn on B failed (`FOREIGN KEY … cost_owners`) → fixed, then `pung` on both at 781 ms |
| 7 | SIGKILL A mid-turn: take `write` refused < 300 s, `user` succeeds; A demotes, orphans listed, nothing merged | ✓ | `owner … was seen 98s ago; takeover after 300s`; epoch 2; A `here:false`, orphans `[{epoch 1, seq 33}]`, transcripts identical |
| 8 | Desk steer from the non-hub forwarded; events on the non-hub within 1 s | ✓ after fix | roster desks were classified as user sessions (taken) → fixed; steer 53 ms, owner unchanged, events ≈15 ms behind the hub |
| 9 | Fresh C bootstraps the 126 MB store from a snapshot, then follows live | ✓ after fix | an owned store with an empty log never offered itself → fixed; 122 MB / 24,944 messages in ≈8 s on loopback; live row at 777 ms |
| 10 | Artifact from A rendered on B by hash; spill fetched | ✓ (spill via a hand-placed file) after fix | record 41 ms, blob 51 ms, identical; spill 109 KB in 70 ms — `get_attachment` now routes spill paths to `fetch_spill` |

Six fixes, each its own commit (`fix(<crate>): two-engine — …`), listed in §3.

## 1. Recipe

Binary: `cargo build -p theuth --bin t` in the worktree; `main`'s `t` built read-only in `~/Develop/theuth` (654febe). Client: bun, HTTP JSON-RPC on `rpc.listen` with `<data_dir>/token`; events over WebSocket on the same port. Scripts in `/tmp/68-z/` (recreate from the snippets below).

```ts
// rpc.ts — bun rpc.ts <data_dir> <port> <method> [params-json]
const [dir, port, method, params = "[]"] = Bun.argv.slice(2);
const token = (await Bun.file(`${dir}/token`).text()).trim();
const res = await fetch(`http://127.0.0.1:${port}/`, { method: "POST",
  headers: { "content-type": "application/json", authorization: `Bearer ${token}` },
  body: JSON.stringify({ jsonrpc: "2.0", id: 1, method, params: JSON.parse(params) }) });
const j = await res.json(); console.log(JSON.stringify(j.result !== undefined ? j.result : { error: j.error }));
```
```ts
// events.ts — bun events.ts <dir> <port> <session|null> <seconds> <out>: every event with +ms since subscribe
const ws = new WebSocket(`ws://127.0.0.1:${port}/`, { headers: { authorization: `Bearer ${token}` } } as any);
ws.onopen = () => ws.send(JSON.stringify({ jsonrpc: "2.0", id: 1, method: "subscribe_events", params: [sid === "null" ? null : sid, false] }));
// … collect `j.params.result` lines, `Bun.write(out, …)` at the end (stdout of a backgrounded bun is not flushed)
```
`watch.ts` polls `load_session` every 50 ms until the history has N rows and prints the elapsed ms; `boot.ts` polls `list_sessions` until N rows; `lat.ts` times 1 000 `rename_session` calls; `cmp.ts` runs the §2.2 call list and normalises ids/timestamps/model text/costs; `pair.ts` fetches `mesh_pairing_string` from one engine and `mesh_add`s it on another (optionally rewriting the addresses to `127.0.0.1`).

Config per engine (`/tmp/68-z/<X>/config.yaml`; `name` and `bind` differ):
```yaml
secrets: { key_store: file }
mesh:
  relay: none            # never dial a relay
  discovery: true        # mDNS on
  name: A
  bind: 127.0.0.1:7941   # NEW (fix 3): fixed loopback socket — several engines on one box
agent: { model: anthropic/claude-haiku-4-5, title_model: anthropic/claude-haiku-4-5 }
slack: { enabled: false }
signal: { enabled: false }
```
Start (`THEUTH_LAUNCH` unset — the desktop exports it; a distinct `rpc.listen` per engine):
```sh
THEUTH_RPC__LISTEN=127.0.0.1:7931 target/debug/t --headless --data-dir /tmp/68-z/A
THEUTH_RPC__LISTEN=127.0.0.1:7932 target/debug/t --headless --data-dir /tmp/68-z/B
bun pair.ts A 7931 B 7932        # mesh_pairing_string on A → mesh_add on B
```
Engines: `one` (lone, branch, :7921), `main` (lone, 654febe, :7923), `mig` = `M` (a `.backup` copy of `~/.theuth/sessions.sqlite`, :7922, bind :7944), `A` (:7931, bind :7941), `B` (:7932, :7942), `C` (fresh, :7933, :7943), `D` (fresh, :7934, :7945). Model calls are real (haiku; `ANTHROPIC_API_KEY` from the environment).

**macOS firewall.** The application firewall was on with `target/debug/t` set to *Block incoming connections*; `t mesh add` with the LAN pairing string timed out (`dial timed out: deadline has elapsed`) and peers never reconnected after a restart (random port, LAN addresses). Without `sudo` the fix was `mesh.bind: 127.0.0.1:<port>` (fix 3): the pairing string then carries the loopback address and the port survives restarts. Two engines each held their own hub lease (6 h probation, 67 §4.5); `mesh_set_hub ["never"]` on B made A the hub for invariant 8.

## 2. Evidence per invariant

### 2.1 Lone engine: log written, pruned, nothing shipped
Engine `one`, `relay: none, discovery: false`, one haiku turn (`send_input` "pong"). t+8 s: `session_log` = 12 rows (seq 1..12, epoch 1, 137–360 B). t+63 s (one `PRUNE_SECS` = 60 tick): 1 row (seq 12, the head — L D2), `sessions_status.log_seq = 12`, `session_cursors` empty. `grep -icE 'sessionsync|snapshot|ship|prune' logs` = 0.

### 2.2 Byte-identical to `main`
`cmp.ts` on `main` (:7923) and `one` (:7921), same config: `new_session`, `rename_session`, `pin_session`, `send_input` (one haiku turn, awaited), `load_session` ×2, `get_todos`, `list_queued`, `session_cost`, `list_activity`, `list_sessions` ×3, `session_descendants`, `rename/pin/archive`, `set_model`, `delete_session`. After normalising ids, timestamps, model text and token/cost numbers, `diff` shows exactly one change: every `list_sessions` row gains `"owner": "<self id>", "epoch": 1, "here": true` (5 rows). `load_session`, `get_todos`, `list_queued`, `session_cost`, `list_activity`, `session_descendants`, `set_model`, `delete_session` are byte-identical.

### 2.3 Append latency
- In-process, release (`cargo test --release`, throwaway `#[ignore]` probe, removed): 1 000 `Store::append` of a 200-char message on one warm durable file: **163 µs/append** with capture + guard + log row, **85 µs** for the same rows through `begin_apply` (capture off, no guard, no log). Debug: 361 vs 149 µs.
- RPC-level, debug builds, same client/box, 1 000 sequential `rename_session` after 50 warm-ups, two runs each: `main` p50 407/422 µs (p99 651/616); branch p50 **710/708 µs** (p99 1004/893).
- Verdict: **not within noise** — +78 µs per write in release (two extra sqlx round trips: the fence `SELECT` and the `session_log` `INSERT`, plus `lock_handle` for the changeset and the per-commit session delete/recreate). Invisible against a model turn, real per write. Follow-up (not done): one `INSERT INTO session_log … SELECT … WHERE owner IN ('', ?)` for guard + log, and a reusable capture session.

### 2.4 Migration of the 126 MB store
`sqlite3 ~/.theuth/sessions.sqlite ".backup /tmp/68-z/mig/sessions.sqlite"` (127,238,144 B; 173 sessions, 24,944 messages, 6,949 model_calls, 20 `errors` with INTEGER ids). Start → `started plugin="agent"` (migrations + owner backfill inside) **0.44 s**; `_sqlx_migrations` 23 "sessions everywhere" `execution_time` **3.76 ms**; `started plugin="rpc"` at +1.07 s. After: `select owner, epoch, count(*) from sessions group by 1,2` → one row `<self>|1|173`; `list_sessions` 173 rows all `self/1/true`; `errors.id` all `text` (`0000000000000001-bc545afd9fb7b801`, order kept).

### 2.5 Chat on A appears on B
`send_input` on A; `watch.ts` on B: user row **127 ms** after the RPC returned; assistant row at 646 ms (model included). Second turn with watchers on both: assistant row seen on A at 1839 ms, on B at **1801 ms** (50 ms poll jitter; B ≤ A). B's `list_sessions` row: `owner = A, epoch 1, here = false`; B cursor `(A,'') = 17`, A's ack `(A, B) = 17`.

### 2.6 Continue here on B
`session_take(id, "user")` on B: **53 ms** → `{owner: B, epoch: 2, here: true}`; A's `session_owner` → `epoch 2, here false`; A's `list_sessions` row `here=false` (chip); `session_epochs = (id, 2, B)` on both; A's `session_orphans = []` (the release moved the tail). The next turn on B **failed**: `role: error — model: accounting: … FOREIGN KEY constraint failed` — `model_calls.owner_id → cost_owners(id)` and `cost_owners` was empty on B: decision 8 (projects/cost_owners join the record replica) was never wired. After fixes 1+2: A logs `accounting: projects/cost_owners replicated (1 row(s) announced)`, B holds the row, `send_input` on B → `pung` on A and B at 781/782 ms; `session_cost` on B answers.

### 2.7 SIGKILL A mid-turn
New A-owned session (`alpha`, on B in 371 ms). `kill -STOP B`; "Write 400 words about the sea." on A; 2.5 s later `kill -KILL A` (exit 137; A's log rows 28–32 for the session are the user row + model_calls, no assistant row; A's ack cursor for B = 27). `kill -CONT B`: B's cursor for A = 32 (datagrams queued in B's socket while stopped). B sees A `reachable:false` ~20 s later.
- `session_take(id,"write")` → `{"code":-32000,"message":"owner f9159c37… was seen 98s ago; takeover after 300s"}` (refused; `last_seen` is the mesh presence stamp).
- `session_take(id,"orphan")` → `epoch 2, here true` (O2: the ledger's reason skips the wait); `session_take(id,"user")` right after → the same row (no-op; the `user` path with an unreachable owner shares the branch). "hello?" on B runs: B answers both pending user rows (`# The Sea…`).
- Restart A: A applies B's row → `sessions: 01a0d6a0… taken by aa89e00c` (Demote); `session_owner` on A = `{owner: B, epoch 2, here: false}`; B logs `dropped stale rows of … epoch 1 from f9159c37 (fence is higher)` for A's post-restart write (seq 33 = `unresolve_open_calls`). `session_orphans` on A = `[{epoch: 1, from_seq: 33, to_seq: 33, at: "2026-09-25T03:34:21.771Z"}]`, on B `[]`. `load_session` on A and B identical (5 rows). Nothing merged.
- Note: rows 28–32 were shipped but unacked at the kill; they were not orphaned only because B had applied them and acked on reconnect before the demote ran — a taker without them would have made them the §4.3 tail.

### 2.8 Desk steer from the non-hub
`agents/ops.md` (haiku, `tools: read_only`) in A's data dir → `roster: desk opened agent=ops session=01a0d6af…` within 6 s. On B the row replicates as `{title: "agent:ops", pinned: true, owner: A, here: false, agent: null}` — `sessions.agent` is **null** for a roster desk (F deviation 6), so F's `classify` took the desk to the steering engine (observed on the pre-fix build: `01a0d6a2… taken by aa89e00c`). After fix 4 (a pinned root titled `agent:<name>` is a desk): `steer(desk, "Ops, what is 6+6?")` on B returned in **53 ms**; `session_owner` before and after on B `{owner: A, epoch 1, here: false}`, on A `here: true` (forwarded over `theuth/rpc/1`, not taken). Events (`subscribe_events(desk)` on both; steer at ≈+1000 ms): B `InputInjected`/`TurnStarted` +1065 (A +1050), `TextDelta` +1829 (A +1814), `TurnFinished` +1861 (A +1845), `TitleChanged` +2480 (A +2464) — **≈15 ms behind the hub**, plus L's `ContextChanged{reason:"replicated"}` as the rows land.

### 2.9 Fresh C bootstraps from the 126 MB store, then follows live
Source `M` = the migrated copy (173 sessions, `session_log` empty, `log_seq 0`). First attempt: **nothing** after 120 s (C: 0 sessions, only `cost_owners` via the record replica) — `held_here` offered an engine's own origin only at `log_seq > 0`, and a snapshot "current to seq 0" would have been skipped (fix 5). Second attempt, fresh C: paired at +3.1 s; `list_sessions` on C = 173 rows **3.1 s after pairing**; mesh `connected` 03:50:22.45 → last write to `C/sessions.sqlite` 03:50:29 (WAL 03:50:30) → the whole snapshot (**122 MB on disk**; 24,944 messages, 6,949 model_calls, 18 epochs, 20 errors, 1,108 inner_calls; multi-part, 18,000 messages visible mid-way) in **≈8 s on loopback**. Cursor `(M,'') = 0`. Row sets identical to M: `sessions (id, owner, epoch, title)` ×173, `messages (session, seq, len(body), excluded, len(vision))` ×24,944, `model_calls (call_id, owner, status, tokens)` ×6,949; every row `owner = M, epoch 1, here = false`. Live: a new haiku turn on M → both rows on C **777 ms** after `send_input` returned; C cursor `(M,'') = 10`, M's ack `(M, C) = 10`.

Join after a take (L's known gap): fresh D paired with A only received the three sessions B had taken as `owner B, epoch 2` with full transcripts (A's relayed snapshot of B's origin, L D1) and the six A-owned at epoch 1, 8 s after pairing. Owner-filtered snapshots carry the current row; the gap remains only for a replica that receives the take row before it ever held the session (a partitioned peer) — not reproduced.

### 2.10 Artifact and spill
A: haiku called `show{name: smoke.md, content: "# Smoke\n\nhello from engine A"}` → `ArtifactShown{call_id: toolu_01GUizkBR9hx4ss3UPoSCSgX, id: 21d6c222a95eaf11}`, blob `A/artifacts/21d6c222a95eaf11.md`. B (`B/artifacts/` empty): `get_artifact(call_id)` → the record with `path` rewritten to `/tmp/68-z/B/artifacts/21d6c222a95eaf11.md` in **41 ms**; `get_attachment(path)` → the bytes in **51 ms**, cached, `cmp` identical, `sha256[..16]` = the name.
Spill: no reachable tool produces > 32,000 chars on this box (the python REPL caps prints at `RUN_OUTPUT_CHARS` = 8,000; `read_file` self-limits; `spawn_subagent` needs `agent.communication`), so the spill file was placed by hand exactly where `spill::cap` writes it: `A/spill/<session>/toolu_01ZSMOKESPILL.txt` (`seq 1 20000`, 108,894 B). F's `GetSpill`/`fetch_spill` had no caller (as-built F item 8) — fix 6 routes `get_attachment("…/spill/<session>/<call_id>.txt")` to `fetch_spill`: on B (no such dir) → **70 ms**, cached at the same relative path, `cmp` identical. Unknown file → `unavailable: owner offline`.

## 3. What changed (commits on `feature/68-sessions`, after `bf18784`)

1. `ea785a5 fix(replica): two-engine — announce_unstamped ships rows that predate an attach` — `ReplicaHandle::announce_unstamped()`: rows with `hlc IS NULL` announced as whole rows and stamped (the update trigger logs only changed data columns, so a stamp-only `UPDATE` announced nothing); `row_as_changes` made `pub(crate)`; test `pre_existing_rows_are_announced_once` (`replica/tests/sync_loopback.rs`).
2. `ab94e7c fix(agent): two-engine — projects/cost_owners join the record replica` — `Store::replicate_records()` (attach on the write pool + announce), called from `supervisor::start` once the replica actor is up. Root cause of 2.6's `FOREIGN KEY constraint failed`.
3. `4075efa fix(mesh): two-engine — mesh.bind knob for the iroh endpoint` — `MeshConfig.bind: Option<SocketAddr>` → `BindOpts.bind_addr`; documented in the default config.
4. `a658477 fix(rpc): two-engine — roster desks forward, not take` — `forward::classify` treats a pinned root titled `agent:<name>` as a desk (`ponytail:` title match; a durable desk mark on the row is the upgrade); `classify_user_vs_desk` extended; generated `crates/rpc/bindings/FrontDoorError.ts` committed.
5. `bafbbd4 fix(sessionsync): two-engine — an owned store with an empty log offers a snapshot` — `held_here` offers `(me, head)` whenever `Store::owns_sessions()`, advertises every cursor row, `Node::cursor → Option`; test `a_store_with_sessions_but_no_log_bootstraps_a_peer`.
6. `b0b8a8f fix(rpc): two-engine — get_attachment fetches a remote spill file` — spill paths route to `artifact::mesh::fetch_spill`; `spill/` joins the served directories.

Found, not fixed: append latency (+78 µs/write, §2.3); `put_session_attachment` / forwarded `send_input{files}` (F item 9); two fresh engines both keep their own hub lease for the 6 h probation (67 §4.5 behaviour); `agents` record/file projection did not reach B within the smoke window; `last_seen` in the take refusal is the mesh presence stamp (reads older than the kill); the demoted owner's orphan count is "unacked", not "undelivered" (2.7 note); the artifact fetch error text says `owner offline` for "no peer has it".
