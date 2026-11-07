# 180 MESH-COST-OWNERS — working notes

Worker notes for `180-mesh-cost-owners.md`. Branch `fix/mesh-cost-owners` off `de34b3e3`.

## Scratch setup (Phase A and C)

- Engines: `target/debug/t` copied to `/tmp/fk/t-base` (base `de34b3e3`) and later `/tmp/fk/t-fix`; data dirs
  `/tmp/fk/{a,b}` (run archives `run*/`), started with `THEUTH_RPC__LISTEN` and `THEUTH_LAUNCH` unset (this shell
  exports `THEUTH_RPC__LISTEN`, which would override `rpc.listen` and point at Sky's engine).
- Every outside connection off before the first start (`/tmp/fk/setup.sh`): `mesh.relay: none`,
  `mesh.discovery: false`, `mesh.bind: 127.0.0.1:1863x`, `link/nodes/signal/slack/telegram/update/sandbox: enabled:
  false`, browser/computer/codeindex/knowledge/dreaming/decision/talk/meeting/voice/web/autoresearch off,
  `secrets.key_store: file`, `rpc.listen: 127.0.0.1:1853x`. Startup log: `mesh: engine … on relay none (discovery
  false)`. `lsof` on the engine: `127.0.0.1:18631` (mesh), `127.0.0.1:18531` (rpc) and one wildcard IPv6 UDP socket iroh
  binds besides the configured v4 one (receive only; nothing dials out without discovery or relay). The pairing
  string carries `ip:127.0.0.1:18631` only.
- Fake model: `/tmp/fk/fake.py` on `127.0.0.1:18431` (OpenAI-compatible, streams `FAKE-REPLY-<n> to: …`), provider
  `fake` added with `add_provider` and a fake key. RPC driver `/tmp/fk/rpc.ts`; Phase A steps `/tmp/fk/phaseA.sh`.
- Pairing exactly as the app does it: `mesh_pairing_string` on A, `mesh_add(<string>)` on B.

## Phase A — reproduced on the base build

### Runs 1–2: what does *not* break

1. **Live, small history.** A started, B started and paired, S created on A after A's start and chatted once. B got
   S's `cost_owners` row in under a second through the record replica's capture triggers
   (`_replica_a{i,u,d}_cost_owners`): the triggers are installed at every attach and log every later insert/update,
   so "rows created after startup are never sent" is not what happens — those rows keep `hlc` NULL in the table
   (the triggers do not stamp the row, the capture stamps the change), which is what the field read showed.
   Send on B took S (`sessions: … taken by …`) and got `FAKE-REPLY-3`. No FK error.
2. **Pruned-outbox snapshot.** A paired with a third scratch engine M first (A's outbox pruned to its newest batch),
   M stopped, fresh B paired: `replica: … is behind the pruned outbox; sent a snapshot of 108 change(s) as seq 11`.
   The snapshot carried both `cost_owners` rows (with their clocks); B had them at once. So the stage-1 snapshot
   itself does not drop accounting rows.

### Runs 3–4: the field failure

What differed in the field: A has weeks of record history and, while it ran alone (or with any known peer unseen
since its start), it never pruned its outbox (`Engine::prune_outbox`: "A solo engine keeps its outbox"; the outbox on
this Mac's own engine, read-only: one origin, seq 1…164464, never pruned). A newcomer is therefore not snapshotted
but **replayed**: every batch A ever captured, oldest first, one `Engine::apply` each. The session log
(`sessionsync`) is a separate stream and ships S's `sessions`, `messages` and `model_calls` rows in milliseconds.
`cost_owners` rides the slow stream, `model_calls` the fast one.

Run 4 (`/tmp/fk/run4`, `phaseA-base.txt`): A builds a record history of 255 batches (400 000 `projects` rows written
in 250 chunks with the scratch engine running — a stand-in for weeks of record traffic), B starts fresh and pairs,
S is created on A after A's start and chatted once:

```
17:42:13 pair B→A; S=01a1b71a-8f2e… created on A
17:42:17 S on B      a: owner=A@1 co=1 calls=2 projects=400000   b: owner=A@1 co=0 calls=2 projects=17600
17:42:17 send on B   → take (A: "sessions: 01a1b71a… taken by 5a410666…")
17:42:18.30 B WARN agent::loop_: model: accounting: error returned from database: (code: 787) FOREIGN KEY constraint
            failed … session=01a1b71a-8f2e…      (the field message, word for word; the error row replicates to A)
17:42:24 send from A → A takes it back (epoch 3), runs on A: FAKE-REPLY-15 (A has its own row)
17:42:30 Continue here on B → epoch 4 here:true; send on B → FK error again (17:42:33.14)
17:42:38 Continue here on A → epoch 5; send on A → FAKE-REPLY-17
17:43:01 PRAGMA foreign_key_check  A: clean   B: 6 × model_calls → cost_owners (S's calls, landed as orphans)
17:43:33 S's cost_owners row lands on B (projects 400000: the replay reached A's newest batches); both checks clean
```

Run 3 (64-batch history) showed the same at 17:38:42 and S's row landing ~25 s later.

- **In the field "Sending from A failed the same way"**: there A's send was served by xeus (owner at epoch 4); here
  A's send took S back first (A reachable, `TakeReason::Write`). Either way the call fails wherever the owner lacks
  the row, and succeeds on A, which always has it.
- **Continue here** on B is a take to B: it moves S onto the engine without the row and the next call fails; on A it
  works. Nothing here makes a session unusable on the engine that created it.

### Why the hlc'd rows are missing too (step 5): the apply order, not the snapshot

- Code: `cost_owners` replicates only on the record replica (`Store::replicate_records`, `replica::attach(&self.write,
  &["projects", "cost_owners"])`), `model_calls`/`remote_meters`/`sessions` only on the session log
  (`capture::ATTACHED`). The two streams are independent (`replica::sync::run_stream` vs `sessionsync`); nothing orders
  a parent on one before its children on the other. `sqlite3changeset_apply` defers FKs and `owner_wins` answers
  `SQLITE_CHANGESET_FOREIGN_KEY` with `OMIT`, i.e. "commit anyway": the children land as orphans.
- A newcomer reads A's record outbox from seq 1 when A never pruned (`run_stream`: snapshot only when
  `after + 1 < oldest`), so every accounting row — stamped (`hlc` set by an announce) or not — arrives in the
  order A captured it, after everything older. A session's model calls arrive within a second; its owner row after
  the replay reaches it (run 4: 76 s for 255 batches). [inference] A's field outbox is far longer: the only
  production outbox readable here (this Mac's engine, read-only) holds 164 464 never-pruned batches.
- Field log fits: xeus' first FK at 17:00:40.068 is 95 s after pairing. [inference] The `sent a snapshot of 827
  change(s) as seq 48` line names `01a0e309`, a replica id ~41 days old (uuid v7 time), with a seq and size that fit
  a young outbox, not A's — so it reads as xeus snapshotting A, and A→xeus as a replay; the field logs were not
  available to confirm which side logged it.
- So the ~97 hlc'd rows were rows A captured late in its history that xeus' replay had not reached (or never did, if
  the stream kept restarting), not rows a snapshot dropped. The pruned-outbox snapshot carries them (run 2).

## Correction of 17:33 (a code-level recon) — applied

It replaced the brief's D1. A first build moved `cost_owners` into `capture::ATTACHED` (session log) with a fence
exemption for ancestor bumps and an apply-time repair; the correction ruled that out (`bump_in` writes ancestors
owned elsewhere → `NotOwner`). Commit `56fd6edd` reverts it whole (ATTACHED, fence, migration, sessionsync apply,
docs) and builds the corrected design:

- **The three suspected receive-side failures, checked against Phase A.** In run 4 every record batch landed: B ended
  with all 400 000 `projects` rows and S's owner row, no `behind the pruned outbox` line on either side (no snapshot
  A→B), and no `changes skipped` warning. So in the repro the row was **late, not lost** — none of (a) the
  snapshot-batch abort, (b) re-send with tail suppression, (c) the skipped INSERT fired. Stream ends were only
  logged at debug on the base build, so a silent restart loop could not be ruled out from its info log; on the fixed
  build they log at warn and Phase C shows none (below). (a) is fixed anyway (D1b): one failing table no longer
  aborts the batch.
- **What A shows.** A's `errors` table held B's two FK rows for S (run 4, `at_seq` 2 and 5): B's error replicates
  into S's transcript (`errors` is attached), and A renders it as its own. `errors` is `(id, session_id, at_seq,
  text)` — no engine; naming the producing engine on the card needs a schema change. Not done; noted.
- **Reverse direction** (a session created on B, taken by A) is covered by
  `taken_session_reserves_without_replicated_owner`, which runs both ways.

## Decisions taken on ambiguities

1. **Stand-in coverage / marker.** D1a rows: `history_coverage = 'unknown'`, `hlc = STAND_IN_HLC`, revision 0, no
   ancestor bump (they carry no money). The sentinel stamp is the marker: H3 (pre-ledger sessions) excludes them,
   and the real row replaces them with the origin's coverage and stamp. `remote_turns`' and `remote_meters`' writers
   ignore a `None` (the FK refuses as before); only `reserve_call_in` turns it into the plain refusal.
2. **Error text.** `session <id> has no row here`; the loop's `accounting: ` prefix makes it
   `model: accounting: session <id> has no row here`; `refused_reservation_aborts_before_transport` still passes.
3. **Boot pass, not a migration.** The derivation is recursive and must also cover rows that replicate in later, so
   it runs at every `Store::open` (cheap on a healthy store: two anti-joins). Dangling `parent_id`s get a tombstoned
   root like the other orphan ids (add-only; no `UPDATE` of a self-referencing row). One info line, only when it wrote.
4. **D1b scope.** Per-table isolation with a bookkeeping `SAVEPOINT` (a failed table's shadow clocks roll back with
   its rows, so nothing claims a value that was not written); the batch is still recorded, so the failed table's
   changes of that batch are not retried (`ponytail:` comment). Stream ends at warn. `open` failures to unreachable
   peers stay at debug (every backoff tick would warn).
5. **Test homes.** The two-store take tests sit in `sessionsync::owner::tests` (its `engine`/`pull`/`take`
   harness; no record replica, the bug's precondition); the sentinel test in `replica/tests/sync_loopback.rs`; the
   failing-table test in `replica::tests`; the repair and refusal tests in `agent::store::accounting`; the transport
   test beside `refused_reservation_aborts_before_transport` in `loop_`.
6. **Cube counts.** `repair_backfills_orphans` asserts on `Store::cost_native` — the cube's own join (the `Cube` type
   is private to `costs`); a tombstoned root attributes to "other".
7. **Merge of 179 PAUSE-SUSPEND (`dce0424b`, merged as `19b6b5a3`, no conflicts) and boot order.** The 180 repair
   runs inside `Store::open` (after migrations and the owner backfill), so at engine start it comes first, then
   `unresolve_open_calls`, then 179's `pause::boot_accounting` (the boot hold: paused trees held, restart accounting
   of `accepted` exchanges). They are independent: the repair only inserts `cost_owners` rows and reads `sessions`,
   `model_calls`, `remote_meters`, `remote_turns`; the hold reads agent files, `sessions` and fails/spares
   `peer_exchanges`/`a2a_tasks`. Neither reads what the other writes, so the order does not matter.
   179's `durability.rs` additions do not touch `reserve_journaled_call`, which still routes through `reserve_call_in`.
8. **Review round (18:33).** (1) D1b parks a failed table's changes in `replica.sqlite` `parked` and retries them
   after later batches touching the table and at attach, so a recorded batch drops nothing that could apply later;
   child-before-parent on `cost_owners` is handled at the row by agent migration 0052's triggers (a stand-in root
   for the missing parent, sentinel stamp), chosen over a replica-side hook because it covers every writer and needs
   no `cost_owners` knowledge in the generic replica. Not parked: unattached tables (a disabled plugin would park
   forever) and partial rows of unknown pks (no clock set, the full row still lands). (2) `remote_turns` orphans are
   in the repair; no `cost_owners` write is a delete-and-insert. (3) H3 excludes sentinel-stamped owners. (4) Counts
   come from `rows_affected`; nothing runs per applied row except the 0052 trigger's indexed `NOT EXISTS`. (5)
   `Engine::note`: once per `(origin, table)` at warn, then debug. (6) `remote_meters` and `remote_turns` writers
   ensure first (since 56fd6edd); `INSERT OR REPLACE INTO remote_turns` cascades nothing (no FK points at it). (7)
   merged (decision 7).

## Phase C — live proof on the corrected build (`/tmp/fk/t-fix2` @ `56fd6edd`, `/tmp/fk/runC2`, `phaseC-fix2.txt`)

Same steps as run 4: A with a 254-batch record history, fresh B paired, S created on A after A's start:

```
18:41:08 pair B→A; S=01a1b750-7ff0… created on A
18:41:12 S on B      a: owner=A@1 co=1 calls=2   b: owner=A@1 co=0 calls=2 projects=19200   (B's replay running)
18:41:12 send on B   → take → FAKE-REPLY-3        b: co=1 calls=4   (B derived the stand-in)
18:41:18 send from A → A takes it back (epoch 3) → FAKE-REPLY-5     calls=6
18:41:25 Continue here on B → epoch 4; send on B → FAKE-REPLY-7     calls=8
18:41:35 Continue here on A → epoch 5; send on A → FAKE-REPLY-9     calls=10
18:41:52 foreign_key_check  A: 0 rows   B: 0 rows   (B's replay at 169 600 / 400 000)
         cost_owners S   A: root, complete, rev 6, hlc NULL (A's own row, untouched)
                         B: root, unknown,  rev 4, hlc 0000000000000000-00000000-0 (the stand-in)
18:42:37 replay done    B: root, complete, rev 6, hlc 000001a1b750f174-00000001-… (A's row replaced the stand-in)
         A unchanged; both checks still empty; 0 error rows for S
```

Logs (warn level now): no `stream … ended`, no `its changes skipped`, no `changes skipped`, no `behind the pruned
outbox`, no `FOREIGN KEY` on either engine — the replay ran to the end without a restart.

Seeded store (`/tmp/fk/x`: run 4's base-build B, `cost_owners` emptied, an orphan call and an orphan remote turn of
unknown owners, a ledger child whose parent is missing; `foreign_key_check`: 7 × model_calls, 1 × remote_turns,
1 × cost_owners), started on the corrected build:

```
18:43:00.070 INFO agent::store: accounting: repaired 4 missing cost owner(s): 1 derived from sessions, 3 tombstoned root(s)
foreign_key_check: 0 rows; model_calls 7 (unchanged); S's stand-in unknown + sentinel; 3 tombstoned roots
18:43:15 second start: no repair line, 5 cost_owners rows, check still empty
```

All processes started for the proofs (engines, fake) were stopped.

## Live re-proof after the review round (merged tree, `/tmp/fk/t-fk2`, `/tmp/fk/runC4`, `phaseC-fk2.txt`)

Same steps, A with a 153-batch record history:

```
19:41:18 S on B      b: co=0 (replay at 25 600 / 240 000)
19:41:18 send on B → take → reply     b: co=1 calls=4 (stand-in: unknown, rev 4, sentinel)
19:41:24 send from A → A retakes (epoch 3) → reply          calls=6
19:41:30 Continue here on B (epoch 4) → send → reply        calls=8
19:41:39 Continue here on A (epoch 5) → send → reply        calls=10
foreign_key_check A 0, B 0; 0 error rows for S
19:41:57 replay done: B's S row = complete, rev 6, A's stamp (the stand-in replaced); A unchanged
logs: no stream ended / parked / skipped / unattached / FOREIGN KEY line on either engine
```

Seeded store (base-era B of run 4, `cost_owners` emptied, an orphan call, two `remote_turns` rows — one of an unknown
owner, one of the store's own session — and a dangling ledger child):

```
19:42:16 INFO agent::store: accounting: repaired 4 missing cost owner(s): 1 derived from sessions, 3 tombstoned root(s)
foreign_key_check 0; remote_turns 2 (kept); model_calls 7 (unchanged); unknown owners counted by H3: 0
second start: no repair line, check still empty, remote_turns 2
```

All processes started for the re-proof were stopped.
