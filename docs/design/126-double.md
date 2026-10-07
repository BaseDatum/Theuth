# 126 — double: a local model of the owner's own decisions that measurably improves with use

Sibling numbers taken tonight: 124 model picker (docs/design/124-model-picker.md); 125 advisor (claimed by another session, no file yet). This document is 126.

Status: design v0 · 2026-10-07 · main f2225972 (read from `.git/refs/heads/main`)

Extends / Amends:
- 57 decision (the oracle, families, gate, §11 training capture) — double is a new consumer; §7 adds a pooled-embedding output to the vendored model.
- 92 decide tool (§5 temperature calibration, §6 capture → fine-tune loop) — double supplies the fine-tune half 92 left open.
- 104 attention inbox, 119 dismissal versioning — new item kinds and "answered for you" rows.
- 101 / 103 decision surfaces and packets — the chip on packet options.
- 109 durability journal (`questions`, `tool_calls`, `turn_steps`) — the label source.
- 29 rewind — a hindsight-negative signal.
- 41 knowledge checks (nightly sweep precedent), 34 schedule (run caps, sensors) — the scheduler shape.
- 32 nodes / 63 one-engine / 68 sessions replication, 67 mesh (`EngineStats.gpu`, `class`) — the training venue.
- 58 cross-platform (`cuda` feature, `models.accelerator`).
- 110 Slack cards, 25 desktop, 123 chart kernel (LineChart) — surfaces.
- 00 architecture §1 item 1 (functional crate names), item 4 (no god-wiring), item 7 (permissions are one contract type).

## §0 Decisions in one screen

1. **Name and shape.** The feature is `double`: crate `crates/double`, config section `double:`, RPC `double_*` methods (the rpc crate's flat `#[method(name = "…")]` convention, crates/rpc/src/api.rs:160-161, 1398-1409 — not a `double.*` namespace, see §13), Settings page "Double", chip text "double". Rejected: the working name "ka"; a `decision`-crate submodule (double has its own store, trainer and surfaces — a plugin crate per 00 §1 item 6).
2. **Double is a client, not a hook.** It subscribes to `ApprovalRequested` / `AskRequested` on the runtime bus (`Runtime::subscribe`, crates/runtime/src/lib.rs:55-64) exactly as the RPC bridge does (crates/rpc/src/bridge.rs:153-156), predicts, and answers at L2/L3 through the same actor messages the RPC handlers send: `Resolve { id, decision }` to the approval actor (crates/tools/src/approval.rs:70-75, crates/rpc/src/api.rs:3368-3370) and `agent::answer_ask` (crates/agent/src/lib.rs:311-330). The registry's `invoke` (crates/tools/src/registry.rs:1562-1601) and `ask_user` are not changed. Rejected: a pre-approval step inside `invoke` (god-wiring, 00 §1 item 4); answering by pretending to be a UI client over WebSocket (no, double is in-process).
3. **Prequential or nothing.** Every prediction is written to `double.sqlite` BEFORE the owner sees the card, tagged with the model version; the owner's answer later marks it correct/wrong. The baseline (frozen zero-shot laya) is scored on the same rows for ever, and "improvement" is the gap over baseline. Rejected: retrospective accuracy over a held-out split only (a growing, easier corpus masquerades as learning); any metric without the baseline column.
4. **Per-class autonomy ladder, earned and revoked.** L0 observe → L1 suggest → L2 act with grace → L3 silent; promotion thresholds from the last 30 days of prequential rows; one override at L2/L3 demotes one rung at once. Hard caps: destructive ≤ L1 always; mutating approvals ≤ L1 unless the class is opted in; secrets, handoffs and MFA never. Rejected: a global confidence threshold (one number cannot be right for `git push` and for `rm -rf`); autonomy by owner toggle alone (the point is measured trust).
5. **Class key = kind + shape + requester**, deterministic, discovered, no v1 merge/split (§3). Rejected: a learned clustering of cards (unexplainable, unstable keys break the ladder's bookkeeping).
6. **Three tiers of learning, one encoder.** Tier 0 memory (k-NN over pooled embeddings, the early workhorse because zero-shot is near chance on some tasks — knowledge `facts/laya-base-checkpoint-zero-shot-limits`); Tier A (nightly or every 25 labels: temperatures, k-NN index, logistic heads; in-process, < 2 min, deterministic); Tier B (LoRA adapter on the vendored ModernBERT encoder, data-triggered, full-corpus, gate-promoted — §7.3); Tier C full fine-tune only on explicit owner action. Rejected: "incrementally fine-tune the embedding model every night on the day's rows" — see §7.3 for the three measured reasons; a separate small model (the decision crate already loads laya; a second encoder doubles memory and loses calibration work).
7. **Embeddings come from the existing forward.** The vendored model already computes the pooled `[CLS]` state at crates/decision/src/candle/model.rs:627 and throws it away; `ModelOutput` (crates/decision/src/candle/model.rs:507-512) gains `pooled: Tensor` (batch, 1024) f32. One field, no second forward. Rejected: a separate sentence-embedding model (memory, a second download, un-shared training).
8. **Training is a state machine with preconditions, not a cron line.** idle → eligible → waiting → acquiring → running → evaluating → promoted|rejected; preconditions re-checked every 60 s, pause/resume at step boundaries, one trainer process per machine (own lock + a `BUILD_SLOTS=1` slot from `~/.theuth/bin/build-lock`, label `double-train`). Rejected: a `schedule` routine (no idle/power sensor exists under `~/.theuth/sensors/` — fs_exists.py, pr_watch.py, rss.py only; and a routine is a prompt to a model, training is not).
9. **Venue: a GPU node in the owner's mesh when one exists.** `double.training.venue: auto | local | node:<name> | off`; auto = a reachable `EngineStats` with `gpu = true` (crates/mesh/src/types.rs:129-130) whose `class` is not `Laptop` and not `Vm` (crates/mesh/src/types.rs:82-86), else local. The bundle never carries weights; the node downloads the base checkpoint itself. Rejected: training on a sandbox VM (it is on the laptop's own CPU/GPU and shares its thermal budget); replicating `double.sqlite` through 68 (engine-local like `attention_state`, crates/roster/src/attention.rs:10).
10. **Store is its own SQLite file**, `~/.theuth/double/double.sqlite`, scrubbed at write with `tools::secrets::redact_value` (crates/tools/src/secrets.rs:77) exactly as 57 §11 capture does (crates/decision/src/training.rs:295-300). Rejected: new tables in sessions.sqlite (replicated, and the journal is a different lifetime).
11. **Honesty about the label pipeline.** The 109 `questions` table (crates/agent/migrations/0033_durability.sql:48-60) exists in the tree but the running engine has not been rebuilt with it; double's live labels start when that engine ships. Pre-0033 history is bootstrapped from `messages` bodies (§6.4) — with the orchestrator's counts as the only volume estimate because this writer had no shell (§6.5).
12. **Acceptance is a published curve, not a promise.** Provisional targets (top-5 approval classes ≥ +15 pts over zero-shot by week 2; coverage@2 % ≥ 40 % of approvals by week 4; zero false accepts on mutating classes) are set formally after a 14-day L0 shadow run. Rejected: shipping L2 before the shadow run.
13. **One owner per design section in the build** (109's duplicate-section lesson): C contract, W0 spike, W1 labels+predictor+ladder, W2 tier A + scheduler + venue, W3 tier B, W4 desktop, W5 TUI/Slack/tools/docs, I integration (§15).

## 1. The ask

Sky, 2026-10-07 01:36, after a chat in which the feature was proposed under the working name "ka":

> "Ok write up the full plan including UI to make it visible, and to give the user control of training when they want to, including docs. This needs to be as full feature for a model that actually measurably improves the more you use the harness, the feature should be called double.
>
> Why wouldn't we just incrementally fine tune the tiny embedding model every night? It's a tiny model. The training cycle needs to be well designed, not just randomly mashing out the gpu while the user is using the machine? Leverage the mesh as well. If the mesh has a gpu node, that's where the training should happen."

The direct answer to the fine-tuning question is §7.3. The training cycle is §8. The mesh is §8.4.

### 1.1 What double is

A local model of the owner's own decisions. It watches every decision card the harness raises to the owner — tool-call approvals raised by the tools registry, `ask_user` questions, attention-inbox items (104), decision packets (101/103), task/agent/routine draft choices — predicts the owner's answer before the owner answers (so every prediction is an honest out-of-sample test), learns from the answer and from hindsight signals that arrive for free, and climbs a per-class autonomy ladder. Autonomy is earned per decision class from measured, time-ordered performance and revoked by a single override. It never answers secret requests, desktop handoffs or MFA. It is not a new model or runtime: it is a new consumer of the existing `decision` crate (laya / GLiNER2.5-Decide in candle 0.11, crates/decision/Cargo.toml:6, 10, 41-43, 48-51) plus a memory, a trained head, an optional LoRA adapter, a trainer with a disciplined schedule, a venue chooser, a store, RPC and surfaces.

### 1.2 Prior art and the novelty claim

Replay and evaluation of agent traces is table stakes (knowledge `facts/agent-eval-replay-and-runtime-guard-prior-art-2026-09-29`: fixed datasets before release, production trace sampling, "convert confirmed failures into versioned regression tasks", replay + CI gates, runtime guard DSLs such as AgentSpec). None of those has what theuth has: free labels from what the owner did next, on the owner's own machine. Double's novelty is limited to three things and this document claims no more: (a) a calibrated owner-model trained on-device from free labels; (b) autonomy earned and revoked per class from prequential measurement; (c) the training cycle of §8. The classifier, the LoRA method and the k-NN memory are ordinary.

## 2. Concepts

| term | meaning |
|---|---|
| decision card | anything the harness raises to the owner and waits on: `ApprovalRequested` (crates/tools/src/approval.rs:41-52), `AskRequested` (crates/tools/src/ask.rs:84-91), an `AttentionItem` (crates/roster/src/attention.rs:71-101), a decision `Packet` (crates/tasks/src/packet.rs:76-81, 196), a draft card set (crates/app/src/draft.rs:1-9, agent_draft.rs, routine_draft.rs). Not a card: secret requests, desktop handoffs, MFA — double is blind to them by construction. |
| decision class | the deterministic key of §3; the unit of statistics, ladder rung and caps. |
| label | the owner's answer to a card, with a `source` and a `weight` (§6): direct, hindsight (rewind / correction / interrupt), override, timeout. |
| prediction row | one row of the prequential ledger (§5 `predictions`), written before the owner saw the card, naming the model version, the choice, the probabilities and the rung at that time; later marked correct / wrong / unlabelled. |
| rung | the class's autonomy level L0–L3 (§9). |
| grace | the L2 countdown during which the owner can take a card over; for mutating approvals the grace is the undo. |
| version | one trained artefact set (temperatures + k-NN index + heads, optionally an adapter) with its metrics; exactly one is `active`; every prediction names one. |
| venue | where a training run executes: `local` or `node:<name>` (a mesh engine). |
| window | the local hours in which Tier B may run (default 02:00–06:00). |
| baseline | the frozen zero-shot oracle, scored prequentially on every row for ever; improvement is always a gap over it. |
| golden set | the first 10 % of each class's rows (min 5), never trained on, re-scored by every candidate version. |

## 3. Decision classes (the keyer)

The class key is `kind:shape:requester`, computed by a pure function in `crates/double/src/class.rs` from the card payload alone, so the same card always lands in the same class on every engine and after every restart.

| kind | shape rule |
|---|---|
| `approval` | `approval:<tool>:<risk>:<shape>`; `risk` is `tools::Risk::as_str` (`read_only` / `mutating` / `destructive`, the same spelling as `tool_calls.risk`, crates/agent/migrations/0033_durability.sql:34). Shape per tool: `shell` → the first two command tokens after stripping leading `cd … &&`, `VAR=value` assignments and `nohup` / `nice` / `env` prefixes (`git push`, `cargo test`, `rm -rf`); an MCP tool (`<server>__<tool>` or a bare MCP name) → tool name + the sorted top-level argument keys; `edit` / `write_file` → the first path component under the home directory or the repo root (`Develop/theuth`, `.theuth`); every other tool → the tool name only. |
| `ask` | `ask:<h12>` where `h12` is the first 12 hex of sha256 over the normalised option labels (lower-cased, whitespace collapsed, sorted) — the question text is not part of the key, so "add to backlog / start now / discard" is one class however the question is phrased. A question with no options (freeform) → `ask:freeform` and is never auto-answered. |
| `attention` | `attention:<kind>` with `AttentionKind` in snake_case (approval, decision, hire, blocked, review, budget, node, handoff, secret, ask, show, resume — crates/roster/src/attention.rs:39-66). `approval` and `ask` items are the same card as the registry's, so they are keyed by the approval / ask rule, not here; `handoff` and `secret` are never predicted. |
| `packet` | `packet:<kind>` with `PacketKind::as_str` (`decision` / `review`, crates/tasks/src/packet.rs:83-90). |
| `draft` | `draft:task` / `draft:agent` / `draft:routine` (the three draft tools in crates/app/src/draft.rs, agent_draft.rs, routine_draft.rs). |

Requester = `root` (a session the owner opened), `subagent` (a child of a root session), `desk:<agent>` (a desk session, from the roster's session→agent map, crates/roster/src/attention.rs:782-798), `routine:<name>` (a schedule run). The requester is part of the key because the same `git push` means different things from Sky's own session and from a nightly routine; a class learned on `root` never transfers to `routine:*` automatically.

Ten example keys. Without a shell this writer could not query `sessions.sqlite`; the keys below use the tool names and command shapes visible in this evening's orchestration transcripts and the registry (the orchestrator counted 7,365 messages containing "approv" and 90 containing "ask_user" over 125,122 messages / 610 sessions, 2026-09-28 → 2026-10-07):

1. `approval:shell:mutating:git push:root`
2. `approval:shell:mutating:git worktree:root`
3. `approval:shell:mutating:cargo test:subagent`
4. `approval:shell:destructive:rm -rf:root` (capped at L1 for ever)
5. `approval:write_file:mutating:Develop/theuth:subagent`
6. `approval:edit:mutating:.theuth:root`
7. `approval:create_pull_request:mutating:base,body,head,owner,repo,title:root` (MCP, sorted arg keys)
8. `approval:knowledge_put:mutating:root`
9. `ask:3f9a1c2b7d0e:root` (the task_draft trio "Add to backlog / Start now / Discard", hashed)
10. `draft:agent:root`

Classes are discovered, not declared: the first card of a new key creates the `classes` row at L0. v1 has no merge/split (Open questions).

### 3.1 Where the brief was wrong (so far)

- The brief names the risk spelling only loosely; the tree's `tool_calls.risk` comment pins `read_only | mutating | destructive` via `tools::Risk::as_str` (crates/agent/migrations/0033_durability.sql:34). The key uses that spelling.
- The brief asked for ten keys "from real history". With no `shell()` in this writer's runtime (the function was absent, not denied) the sqlite query was impossible; the ten keys above are real tool names and real command shapes from tonight's sessions, but their counts are not measured. §6.5 lists the queries the W1 worker must run.

## 4. Where double sits (architecture)

`crates/double` is a plugin crate (00 §1 item 1: named by function — it is the owner's double) registering one actor, `DOUBLE_ACTOR`, plus the trainer and the store. It owns its message types (00 §1 item 5). Nothing inside the agent loop changes (00 §1 item 4): the registry and `ask_user` do not know double exists.

### 4.1 Seams, with the exact calls

| seam | today | double's use |
|---|---|---|
| raised approvals | `Approval` publishes `ApprovalRequested` on the bus after journaling the `questions` row (crates/tools/src/approval.rs:197-247; `rt.publish(requested)` at :247) | `rt.subscribe::<Double, ApprovalRequested>(&actor)` — the same call the RPC bridge makes (crates/rpc/src/bridge.rs:153) |
| raised asks | `AskUser` publishes `AskRequested { id, session, questions }` (crates/tools/src/ask.rs:226-231) | `rt.subscribe::<Double, AskRequested>` (crates/rpc/src/bridge.rs:155) |
| answered approvals | `ApprovalResolved { id, decision, timed_out }` published at crates/tools/src/approval.rs:257-262; the row is settled by `settle` (crates/tools/src/approval.rs:144-152) | subscribe (crates/rpc/src/bridge.rs:154) → direct label; `timed_out` → weak label |
| answered asks | `AskResolved { id, dismissed, timed_out }` (crates/tools/src/ask.rs:241-246); the chosen `Answer::Choice { index }` (crates/tools/src/ask.rs:65-78) is in the settled `questions.answer` | subscribe (crates/rpc/src/bridge.rs:156) for the event; read the answer from the journal row (`Store::question_*`, crates/agent/src/store/durability.rs:905-944) |
| answering an approval | RPC `resolve_approval` → `ask::<Approval, _>(tools::APPROVAL_ACTOR, Resolve { id, decision })` (crates/rpc/src/api.rs:3368-3370); `Resolve` at crates/tools/src/approval.rs:70-75 | double sends the same `Resolve` at L2 grace expiry / L3 |
| answering an ask | RPC `answer_ask` → `agent::answer_ask(id, answers)` (crates/rpc/src/api.rs:3372-3376, crates/agent/src/lib.rs:311-330) | double calls `agent::answer_ask(id, Some(vec![Answer::Choice { index }]))`; never `None` (dismiss cancels the turn, crates/agent/src/lib.rs:320-327) |
| attention items | `Attention::derive` (crates/roster/src/attention.rs:220-592), `DismissAttention` / `SnoozeAttention` (crates/roster/src/attention.rs:160-169), dismissal versioning 119 (crates/roster/src/attention.rs:10-11) | subscribe to the inbox's change event for act/dismiss labels of non-card items; add `double:<question>` items for "answered for you" (§10) |
| packets and drafts | `packet_get` RPC (crates/rpc/src/api.rs:1738), `task_decide` (crates/rpc/src/api.rs:1741); draft presses arrive as a chat message `[app task-drafts …]` (crates/app/src/draft.rs:4-7) | predict only (L0/L1 chip) in v1; labels from the RPC call and the press message |
| rewind / edit / regenerate | `AgentEvent::ContextChanged { reason: "rewind" | "context_edit" | … }` (crates/agent/src/lib.rs:2028-2035) | hindsight negative for the rewound turn's decisions (§6.2) |
| interrupt | `AgentEvent::TurnFinished { stop_reason: "cancelled" }` (crates/agent/src/lib.rs:1824-1835; "cancelled" per crates/agent/src/lib.rs:293) | hindsight negative (weak) |
| the oracle | `Engine` actor, `Ask` / `AskMany` (crates/decision/src/engine.rs:127-142, 1063-1115), `with_none` appends the `none` control (crates/decision/src/engine.rs:655-676), `FamilyGate` per family (crates/decision/src/gate.rs:59-99) | families `double.approval`, `double.ask`, `double.attention`, `double.packet`, `double.draft`, `double.correction`; plus the new embedding request (§7.1) |
| audit | `questions.answer` (crates/agent/migrations/0033_durability.sql:58) holds the bare `Decision` JSON written by `settle` (crates/tools/src/approval.rs:147) | see 4.3 — the tree does not have `answered_by`; it is added as a column |

### 4.2 The flow

```
registry.invoke                Approval actor              bus              double                     UI (desktop/TUI/Slack)
     |  RequestApproval{call_id}    |                        |                 |                              |
     |----------------------------->| journal.question(row) |                 |                              |
     |                              | publish(ApprovalRequested) ------------>|---- (bridge) --------------->| card renders
     |                              |                        |   subscribe     |                              |
     |                              |                        |---------------->| key = class(card)            |
     |                              |                        |                 | embed + predict (≤ 150 ms)   |
     |                              |                        |                 | INSERT predictions(version)  |
     |                              |                        |<-- publish(DoublePredicted) -----------------> | chip "double · Allow · 91 %"
     |                              |                        |                 |                              |
     |   L0/L1: owner answers       |<--- Resolve{id,decision} (RPC resolve_approval) -------------------------|
     |                              | settle(row) ; publish(ApprovalResolved) -------------------------------->| card closes
     |                              |                        |---------------->| label(direct) ; outcome      |
     |                              |                        |                 |                              |
     |   L2: grace runs             |                        |                 | timer grace_secs             | countdown on card
     |   (owner may take over: RPC double_take_over(question_id) cancels the timer; a human Resolve wins the race)
     |                              |<--- Resolve{id,decision, by: Double} ---|                              |
     |                              | settle(row, answered_by=double) ; publish(ApprovalResolved) ----------->| "answered by double"
     |<--- ApprovalResolved         |                        |---------------->| publish(DoubleAnswered)      | inbox row with undo (grace)
```

The `ask_user` path is identical with `AskRequested` / `AnswerAsk` / `AskResolved` and the agent's `answer_ask` entry. The registry's `invoke` waits on `approval.ask(req)` (crates/tools/src/registry.rs:1585) exactly as today; from its point of view a double answer is a fast human.

### 4.3 Where the brief was wrong

- The brief says double answers "with `answered_by: "double"` in `questions.answer`". In the tree `settle` writes `row.answer = answer.map(|d| serde_json::json!(d))` — the bare `Decision` (crates/tools/src/approval.rs:147) — and 109's replay (`HonourAnswer`, crates/tools/src/approval.rs:94-99, 163-169) consumes that shape. Changing the JSON shape would touch the replay path. Ruling: a new nullable column `questions.answered_by TEXT` (migration `0034_double.sql` in crates/agent/migrations), set from a new optional field `Resolve.by: Option<Answerer>` / `AnswerAsk.by` (default `None` = human) threaded through `settle`. The `answer` JSON is unchanged.
- The brief lists "ApprovalRequested / AskRequested event types" to be found in crates/rpc. They live in crates/tools (crates/tools/src/approval.rs:41, crates/tools/src/ask.rs:84); rpc only forwards them (crates/rpc/src/bridge.rs:102-105).
- The brief expects a `double.*` RPC namespace. The `Api` trait is flat (`#[method(name = "decision_mode")]`, crates/rpc/src/api.rs:1398); double follows it with `double_*` (§13).

### 4.4 What double deliberately does not touch

- `Approval`'s timeout (`self.timeout`, crates/tools/src/approval.rs:205, 244-255): an L2 grace shorter than the approval timeout is the only requirement; the default grace (20 s) is far under any configured approval timeout, and a grace that would expire after the deadline is clamped to `deadline − 2 s`.
- `HonourAnswer` (approval.rs:94-99, 163-169): after a restart, 109 replays an already-answered question without a second card; a double answer recorded with `answered_by = 'double'` is honoured the same way, so double never answers twice.
- The `waits` row the approval creates for its call (approval.rs:219-234): double's answer settles it through `settle` exactly as a human's; resume logic is unchanged.
- `PendingApprovals` (approval.rs:297-308), the snapshot a reconnecting client gets: a card double already answered is not in it; a card in grace is, with its `DoublePredicted` re-published on subscribe so the countdown survives a UI reload.
- The decision crate's own `Mode` / `FamilyGate` semantics (57 §0 item 6: every family ships in `shadow`): the `double.*` families run `active` because their output is a chip, not an action; the action is the ladder's, gated by §9, not by `FamilyGate.mode`.

## 5. Store: `~/.theuth/double/double.sqlite`

Own file, like `knowledge.sqlite` and `tasks.sqlite`; engine-local, not replicated by 68 (the mesh ships training bundles explicitly, §8.4). Written through sqlx with WAL; every string leaf scrubbed with `tools::secrets::redact_value` (crates/tools/src/secrets.rs:77) before it is bound — the same scrubber 57 §11 capture uses (crates/decision/src/training.rs:295-300).

```
classes      (key TEXT PK, kind TEXT, rung INTEGER, cap INTEGER, mutating_opt_in INTEGER, n INTEGER,
              stats TEXT /* JSON: answer histogram, median_answer_ms, last_30d {n, acc, ece, overrides} */,
              updated_at TEXT)
examples     (id TEXT PK /* uuid v7 */, class TEXT, question_id TEXT, session_id TEXT, requester TEXT,
              card_text TEXT /* ≤ 4 KB, scrubbed */, options TEXT /* JSON labels in card order */,
              embedding BLOB /* f16 × 1024 = 2 KB */, asked_at TEXT)
labels       (example_id TEXT, source TEXT /* direct|hindsight_rewind|hindsight_correction|hindsight_interrupt|override|timeout */,
              choice TEXT, weight REAL, at TEXT, PRIMARY KEY (example_id, source))
predictions  (example_id TEXT, version TEXT, choice TEXT, probs TEXT /* JSON */, p_top REAL,
              rung_at_time INTEGER, acted INTEGER, grace_until TEXT,
              outcome TEXT /* correct|wrong|unlabelled */, at TEXT, PRIMARY KEY (example_id, version))
versions     (id TEXT PK, tier TEXT /* a|b|c|baseline */, base_hash TEXT, adapter_path TEXT, head_path TEXT,
              knn_path TEXT, metrics TEXT /* JSON */, data_range TEXT, trained_at TEXT, venue TEXT,
              duration_s INTEGER, active INTEGER)
runs         (id TEXT PK, tier TEXT, venue TEXT, state TEXT, started_at TEXT, ended_at TEXT,
              result TEXT, log_path TEXT)
golden       (example_id TEXT PK)
```

`predictions` is the prequential ledger: `(example_id, version)` is unique, and the `baseline` version gets a row for every example alongside the active version, so the two curves are always over the same rows. `embedding` is f16 little-endian (2,048 bytes); 50,000 examples ≈ 100 MB plus text — well inside a laptop. `card_text` is the 4 KB state of §7.1, stored so a candidate version can be re-scored without the original payload. Retention: `double.retention_days` (default 365) deletes `examples`/`labels`/`predictions` older than that except golden rows and the last 1,000 per class.

## 6. Labels (free supervision)

| source | what | weight |
|---|---|---|
| `direct` | the owner's answer: `ApprovalResolved.decision` with `timed_out = false` (crates/tools/src/approval.rs:257-262); `Answer::Choice { index }` from the settled ask row; a `task_decide` call for a packet; the `[app task-drafts …]` press message for a draft; `DismissAttention` / an act on an inbox item (crates/roster/src/attention.rs:169) | 1.0 |
| `override` | the owner takes over an L2 card (`double_take_over`) or reverses an L2/L3 answer (undo during grace, or a contrary answer within the same turn) | 3.0 — the strongest signal, and the demotion trigger (§9) |
| `hindsight_rewind` | `ContextChanged { reason: "rewind" }` (crates/agent/src/lib.rs:2033) landing on a turn that contained decisions: every decision in that turn answered `Allow` / an option becomes a negative for that choice | 0.5 |
| `hindsight_correction` | the next user message in the session is judged a correction of the assistant's previous action by the `decision` Bool family `double.correction` ("Is this message a correction of the assistant's previous action?") — zero-shot at first, itself trained later from rewinds (a rewind is a confirmed correction) | 0.5 × P(true), only when P(true) ≥ 0.8 |
| `hindsight_interrupt` | `TurnFinished { stop_reason: "cancelled" }` (crates/agent/src/lib.rs:1824-1835) within 30 s of a decision in that turn | 0.25 |
| `timeout` | `ApprovalResolved.timed_out = true` / `AskResolved.timed_out = true`, or a `superseded` row: "the owner did not care" — a weak label for the card's default (Deny / Skipped), and a routine-ness feature, not a positive for any option | 0.25 |

Time-to-answer (`answered_at − asked_at` from the `questions` row, crates/agent/migrations/0033_durability.sql:55-57) is stored in `classes.stats.median_answer_ms`: a class the owner answers in 2 s without reading is routine; one that takes 40 s is deliberated. It feeds the head (§7.1) and the "minutes saved" KPI (§7.4).

### 6.1 Attachment rules for hindsight

A hindsight signal attaches to the decisions of the same turn (by `turn_steps` / `tool_calls.turn_id`, crates/agent/migrations/0033_durability.sql:10-21, 27-44), and to the decisions of the previous turn only when the correction arrives within two user turns of it. It never attaches across sessions, never to a decision the owner already overrode (the override already carries the information), and never to `ask:*` classes whose chosen option was "escalate to me" (the owner asked to be asked). When both a direct label and a hindsight negative exist for one example, both rows stay; the trainer sees the weighted sum (a confirmed-then-regretted `Allow` nets to +0.5 for Allow — the owner did say yes).

### 6.2 Labels double never takes

Secret requests, desktop handoffs and MFA (`questions.kind = handoff | secret`, crates/agent/migrations/0033_durability.sql:52) are not examples at all; cards from a session with `double.capture.<kind>: false`; cards in a session the owner marked "don't learn from this one" (chip toggle, §10) — stored with `labels.source = 'excluded'` weight 0 so the example count stays honest.

### 6.3 Dependency: the 0033 engine

The `questions` table (crates/agent/migrations/0033_durability.sql:48-60) is what makes the direct labels and the timings free. The table exists in the tree (crates/agent/migrations/0033_durability.sql) but, as of 2026-10-07 01:40, the running engine had not been rebuilt with it (`sqlite3 -readonly ~/.theuth/sessions.sqlite ".tables"` showed no `questions`, per the orchestrator). Double's live label pipeline starts the day the 0033 engine runs; until then the bootstrapper below is the only source. The design does not block on the rebuild for anything else: predictions can be made from the bus events alone.

### 6.4 Bootstrapping pre-0033 history from `messages`

Before 0033, approval and ask events exist only as JSON in `messages.body` (the orchestrator's measurement: 125,122 messages, 610 sessions, 2026-09-28 → 2026-10-07; 7,365 bodies contain "approv", 90 contain "ask_user"). The bootstrapper (`t --double-bootstrap`, run once, idempotent on `question_id`) walks `messages` in `(session_id, seq)` order and:

1. finds assistant `tool_use` rows and their `tool_result` rows; a result whose text is `<tool>: denied by user` or `<tool>: approval timed out` (the exact strings at crates/tools/src/registry.rs:1591-1593) is a Deny / timeout; a tool whose `ToolSpec.risk` is mutating or destructive and whose result is not one of those strings, in a session with approvals on, is an Allow;
2. reconstructs the card payload from the `tool_use` args (the same fields `ApprovalRequested` carries: tool, args, risk — crates/tools/src/approval.rs:41-52) and keys it with §3;
3. takes `ask_user` tool rows with their result (the answers JSON) as `ask:*` examples;
4. embeds each card with the active oracle and writes `examples` + `labels(source = 'direct', weight = 0.75)` — discounted because step 1 is inference, not a journaled answer;
5. writes **no** `predictions` rows for them: bootstrapped examples are training data and k-NN memory, never prequential evidence. The published curve starts at the first live prediction.

### 6.5 Volume — what is known and what is owed

The brief asked this writer to run read-only sqlite queries for approvals per day, asks per day and distinct tools approved. This writer's runtime had no `shell()` function at all, so the orchestrator's numbers above are the only estimate in this document. Order of magnitude from them: ~7,000 approval-bearing messages over 9 days is ~800/day during a wave week; 90 `ask_user` mentions is ~10/day. The W1 worker must run and record in the as-built:

```
-- approvals per day (denied / timed out are explicit; allowed = mutating tool_result without those strings)
SELECT date(created_at), count(*) FROM messages WHERE body LIKE '%denied by user%' OR body LIKE '%approval timed out%' GROUP BY 1;
-- ask_user per day
SELECT date(created_at), count(*) FROM messages WHERE body LIKE '%"name":"ask_user"%' GROUP BY 1;
-- distinct tools in tool_use rows (JSON1)
SELECT json_extract(value, '$.name') AS tool, count(*) FROM messages, json_each(json_extract(body, '$.content'))
  WHERE json_extract(value, '$.type') = 'tool_use' GROUP BY 1 ORDER BY 2 DESC LIMIT 40;
```

(Column names to be confirmed against the `messages` schema in crates/agent/migrations; the orchestrator's `messages.body` naming is assumed.)

### 6.6 Dependency: approvals must be on for approval cards to exist

`tools.approvals: false` "never prompts: every `ask` (policy or MCP gate) runs as `allow`" (crates/tools/src/lib.rs:455-457), per session or globally through `set_approvals` (crates/rpc/src/api.rs:660-663, Settings › Approvals). Sky's config had approvals off on 2026-09-29 (knowledge `facts/theuth-config-approvals-off-2026-09-29`) — with it off there are no approval cards, no `questions` rows of kind `approval`, and nothing for double to learn from or answer. Double is the argument for turning them back on: a routine class climbs to L2 and the owner stops seeing it, while the long tail stays visible. The user guide (§11) says this in its first paragraph, and the Settings › Double status header shows "approvals: off for N sessions — double learns only from asks" when that is the case. Double never flips `tools.approvals` itself.

## 7. The model (predictor and training)

### 7.1 Prediction

Prediction is a stack over the existing oracle, cheap enough to run before the card renders (budget 150 ms; the chip arrives after the card, never delays it):

1. **Zero-shot oracle probs.** One `Ask` to the `Engine` (crates/decision/src/engine.rs:1063-1084) under family `double.<kind>` with a `Question::Choice` (crates/decision/src/question.rs:21-30) whose candidates are the card's options — `Allow` / `Deny` for an approval, the option labels for an ask (labels, never ids: knowledge `facts/laya-base-checkpoint-zero-shot-limits`), the packet options, the draft's three presses — plus the mandatory `none` control the engine appends (`with_none`, crates/decision/src/engine.rs:655-676); `none` means "escalate to the owner". The 4 KB state (right-truncated at the model's cap, `state_cut` is tolerated via `cut_ok: true` on the `double.*` gates, crates/decision/src/gate.rs:89-93) is laid out as: card text (shell command / described args / question text), then `class: <key>` and `requester: <r>`, then `previously: Allow ×12 Deny ×1 (median 2 s)`, then the current user goal (the last user message of the session, truncated to 600 chars).
2. **Pooled embedding.** The same forward now returns `pooled` (§7.2); it is stored on the `examples` row.
3. **k-NN vote.** Cosine over the class's stored embeddings, k = 15, weights `exp(−age_days / 30)` × label weight; the vote is a probability vector over the card's options (an option never seen before gets the Laplace floor). Memory is the early workhorse: with 20 examples of `git push` it already answers better than zero-shot.
4. **Class statistics.** n, the recent answer histogram, median time-to-answer, requester, risk — from `classes.stats`.
5. **Head.** A logistic regression over the concatenation [oracle probs ‖ k-NN probs ‖ class statistics ‖ one-hot requester ‖ one-hot risk] → option logits; per class when n ≥ 20, else the global head with the class features; a temperature per class fitted as 92 §5 fits them (NLL on held-out rows). Output probs → the 57 gate (`FamilyGate::decide` with τ_top / τ_margin, crates/decision/src/gate.rs:187-210) → a choice or `Escalate`. `none` winning or `Escalate` = L0 behaviour for that card whatever the rung.
6. **Write the prediction row** (`predictions`, version = the active version id) before publishing `DoublePredicted`. Also write the baseline row (version = `baseline`, the zero-shot probs of step 1 alone).

Why a logistic head and not a second transformer: it is 60–200 parameters per class, fits in milliseconds, is convex (deterministic for a given dataset — the reproducibility §7.4 needs), and explains itself (the chip popover can say "memory 0.62, oracle 0.21, class prior 0.17").

### 7.2 Where the embedding comes from (crates/decision change)

The vendored laya forward (`DecisionModel::forward`, crates/decision/src/candle/model.rs:552-639) already builds the pooled state the act head reads: `let pooled = Tensor::cat(&[xs.i((.., 0))?.to_dtype(DType::F32)?, features], D::Minus1)?;` at crates/decision/src/candle/model.rs:627 — the `[CLS]` position after the decision head plus four confidence features. `ModelOutput` (crates/decision/src/candle/model.rs:505-512) returns only `logits` and `action` (crates/decision/src/candle/model.rs:638). The smallest change: add `pub pooled: Tensor` (`(batch, hidden_size)` f32 — the `xs.i((.., 0))` slice without the four features) to `ModelOutput` and set it at crates/decision/src/candle/model.rs:638; thread it through `Scores` (crates/decision/src/engine.rs:75-84) as `pub embedding: Option<Vec<f32>>`, filled only when the `FamilyGate` says `embed: true` (a new flag, default false, so the meeting and browser families pay nothing). One extra `to_vec1` per row; no second forward.

Weights load as plain `Tensor`s: `VarBuilder::from_backend(Box::new(backend), dtype, device)` over an mmapped safetensors (crates/decision/src/candle/mod.rs:214-228). A `VarBuilder` from a backend yields constants, not `Var`s, so candle's autograd does not see them — right for inference and exactly what a LoRA wants: the base stays frozen and only the adapter's `Var`s (held in a `candle_nn::VarMap`) have gradients.

### 7.3 Tier B — the LoRA adapter, and the direct answer to Sky's question

**Yes, we fine-tune the encoder, and yes it can run nightly.** But the run is **data-triggered, full-corpus and gate-promoted**, not "an incremental pass over the day's rows", for three reasons that are measurable in this very store:

1. **Burstiness.** One wave night produces hundreds of near-identical `git push` / `cargo test` approvals (the orchestrator's 7,365 approval-bearing messages in 9 days were not evenly spread; design 109's build night alone produced several hundred). A gradient pass over "today's rows" is a pass over one class. The encoder is shared by every `double.*` family and — if the adapter is ever shared — by the meeting and browser families of 57 §11; a one-class pass moves it for all of them. That is catastrophic forgetting, and the golden-set check of §7.4 would catch it the next morning — after a night of training for nothing.
2. **A monotone curve is the whole point.** "Measurably improves" means the weekly prequential curve of §7.4 is comparable week to week. A model that changes every night from a different slice is a random walk around its mean; the curve would be noise and the ladder's promotion thresholds would be gambling. A version trained on the full labelled corpus with a fixed replay sample from every class changes only when the corpus has changed enough to matter.
3. **Rollback must be cheap and exact.** Every prediction names its version; `double_rollback(version)` must restore the exact artefact that produced the numbers the owner saw. Incremental nightly updates make "the version of last Tuesday" a chain of deltas; full-corpus versions make it one directory.

So the adapter rule is:

- **What.** A LoRA adapter, rank 8–16, on the encoder's attention `Wqkv` / `Wo` and MLP `Wi` / `Wo` matrices (`EncoderAttention::load` and `EncoderMlp::load`, crates/decision/src/candle/model.rs:216-234 and 262-277, inside `EncoderLayer`, crates/decision/src/candle/model.rs:290-341); one adapter serving every `double.*` family with the class in the prompt; base checkpoint frozen; the meeting / browser families untouched unless `decision.families.<prefix>.adapter: double` opts them in (Open questions).
- **When.** Eligible when ≥ 200 new labels have landed since the active adapter's `data_range` end, or 7 days have passed, whichever first. On a wave week that is every night; on a quiet week it is Sunday.
- **On what.** The full labelled corpus (direct + hindsight + override rows, weights of §6) with rehearsal: a fixed replay sample of up to 200 rows from every class, re-drawn only when a class doubles. Golden rows excluded.
- **Promotion gate.** The candidate is activated only if, re-scored locally (inference only), it (a) beats the active version on the last-7-days prequential proxy (log-loss over the rows the active version predicted live, the candidate scored offline on the same `card_text`), (b) does not regress more than 1 point of accuracy on the golden set, and (c) has ECE ≤ 0.05 after its temperature fit. Otherwise it is written to `versions` with `active = 0` and `metrics`, the `runs` row says `rejected: <which gate>`, and nothing changes for the owner.
- **Tier C** — a full fine-tune of the encoder — is an explicit owner action ("Rebuild" in Settings or `double_train("c", venue)`), preferred on a mesh GPU node, never scheduled, and produces a new base hash that every later adapter is trained against.

**What the tree says about feasibility** (checked in the vendored sources, nothing executed):

- candle 0.11.0 is the pinned version (Cargo.lock:2789-2790). `candle_core::Var` (`pub struct Var(Tensor)`, candle-core-0.11.0/src/variable.rs:10), `Tensor::backward` (candle-core-0.11.0/src/backprop.rs:165), `candle_nn::VarMap` (candle-nn-0.11.0/src/var_map.rs:12) and `candle_nn::AdamW` (candle-nn-0.11.0/src/optim.rs:111) all exist, so an adapter can be trained in-process on CPU and Metal without a Python sidecar.
- The backward pass covers the ops the loss path uses — `Op::Gather` (the marker gather at crates/decision/src/candle/model.rs:589) and `Op::WhereCond` (the mask at crates/decision/src/candle/model.rs:601-603) have gradients (candle-core-0.11.0/src/backprop.rs:90, 423, 59, 228). `sort_last_dim` (crates/decision/src/candle/model.rs:619) has none, but it only feeds the act head's features, which the LoRA loss does not use.
- **One known blocker.** The vendored attention uses `candle_nn::rotary_emb::rope` (crates/decision/src/candle/model.rs:75-76). In candle-nn 0.11 that function is a `CustomOp3` (`impl candle::CustomOp3 for RotaryEmb`, candle-nn-0.11.0/src/rotary_emb.rs:314) with no `bwd` override, and the default `bwd` returns `Error::BackwardNotSupported` (candle-core-0.11.0/src/custom_op.rs:142-151). A backward through the encoder therefore fails at the first attention layer today. The fix is small and local: in training mode call `candle_nn::rotary_emb::rope_slow` (candle-nn-0.11.0/src/rotary_emb.rs:590), which is composed of differentiable primitives, or vendor a `rope` with a `bwd`. This is W0's first task and the reason W0 exists.
- Metal: candle's Metal backend implements the forward kernels used here (the engine runs on it today, knowledge `facts/laya-rs-candle-ports`: 74–111 ms/question f16); backward on Metal is the same primitives in reverse, but any op whose Metal kernel is missing surfaces as `no metal implementation for …` (candle-core-0.11.0/src/custom_op.rs:138) — W0 measures this, and the fallback is CPU with Accelerate for the adapter step, or the mesh node.
- Precedent: `github.com/apiplant/laya-rs` ships an RLCD trainer in Rust on candle with CUDA (knowledge `facts/laya-rs-candle-ports`) — the from-scratch port without a Metal path. Its trainer shape is the reference for W3; its code is not vendored.

**Cost estimates (to be measured by W0, labelled estimates):** laya is 421 M parameters, 0.85 GB in fp16 (knowledge `facts/laya-typed-decision-classifier`); a rank-8 adapter on the matrices above is ~10–30 MB. Forward is ~100 ms/row on Metal today; forward + backward ≈ 3× → ~0.3 s/row → a 5,000-row epoch ≈ 25 min, 2–3 epochs ≈ 1 h locally — inside the 90-minute nightly cap only when the corpus is under ~8,000 rows, which is why the corpus is capped by the rehearsal sampler (every class contributes at most 200 rows beyond its newest 200) and why a CUDA node (minutes for the same epoch) is preferred when one exists. Memory: fp16 weights 0.85 GB + fp32 adapter and optimizer state (≤ 0.2 GB) + activations at batch 8 × 512 tokens (~2–3 GB) fits a 16 GB unified-memory Mac with the desktop app running; batch 4 if memory pressure is reported.

**W0 exit criterion:** one LoRA step (forward, loss, backward, AdamW update) on the vendored model on Metal and on CPU, with measured step time and peak memory at batch 8 × 512 and a loss that falls over 50 steps on 200 bootstrapped rows — or a documented failure naming the op, and the sidecar fallback (a `python -m peft` trainer in a venv, invoked as a job on the venue, producing the same adapter files) adopted for W3.

### 7.4 Measurement — "measurably improves"

All metrics are computed from `predictions` joined to `labels`, per class and overall, for the active version and for `baseline` on the same rows:

| metric | definition |
|---|---|
| prequential accuracy | share of labelled predictions whose `choice` equals the direct/override label, in time order |
| Brier score | mean squared error of `probs` against the one-hot label |
| ECE | expected calibration error over 10 equal-width bins of `p_top` |
| coverage@ε | the fraction of decisions double would have answered at ≤ ε error with the current gate; ε = 2 % — the number that says how much of the owner's clicking is removable |
| asks avoided | count of L2/L3 answers (`acted = 1`) |
| minutes saved | Σ over acted rows of the class's `median_answer_ms`, plus the interruption cost (a constant 30 s per avoided card, labelled as such) |
| overrides / 100 | overrides per 100 acted rows |
| undo rate | undos during grace per 100 acted rows |
| gap over baseline | each of the above minus the same metric for `baseline` — the only number allowed to be called "improvement" |

The golden set (first 10 % of each class's rows, min 5, `golden` table) is never trained on and is re-scored by every candidate version; it is the forgetting detector. The learning curve — weekly prequential accuracy and coverage@2 %, current vs baseline, per class and overall — is the feature's proof and sits at the top of the Settings › Double page (§10). Provisional targets, to be made firm after the first 14-day L0 shadow run: top-5 approval classes ≥ +15 pts over zero-shot by week 2; coverage@2 % ≥ 40 % of approvals by week 4; zero false accepts (an L2/L3 `Allow` later overridden or rewound) on mutating classes. If the curve does not clear the first target the honest outcome is to keep L0 and say so in the as-built.

### 7.5 Reproducibility and determinism

Tier A is deterministic: temperatures by a 1-D NLL minimisation with a fixed grid; the k-NN index is a flat f16 matrix (no approximate index, no randomness — at ≤ 50,000 rows a brute-force cosine over 1,024 dims is ~50 ms on Accelerate); the logistic heads by L-BFGS from zero with L2 = 1.0 and a fixed iteration cap. Running Tier A twice on the same `data_range` yields byte-identical `heads.json` / `knn.bin` / `temperatures.json`, which the W2 tests assert. Tier B seeds candle's RNG from the run id, fixes the rehearsal sample by a hash of the class key, and records the seed in `manifest.json`; two runs on the same corpus and seed agree to fp16 rounding on Metal, which is the honest limit.

### 7.6 Where the brief was wrong (model)

- The brief said "find whether weights load as `Var` or plain `Tensor`". They load as plain tensors through `VarBuilder::from_backend` over an mmapped safetensors (crates/decision/src/candle/mod.rs:214-228); there is no `VarMap` anywhere in crates/decision. For inference this is right; for LoRA it is right too — only the adapter is a `Var`.
- The brief listed "custom kernels without backward?" as a question. The answer is precise: the only custom op in the vendored forward is `candle_nn::rotary_emb::rope` (crates/decision/src/candle/model.rs:75-76), and it has no backward in candle 0.11 (candle-nn-0.11.0/src/rotary_emb.rs:314; default `bwd` at candle-core-0.11.0/src/custom_op.rs:142-151). Everything else in the loss path is a core op with a gradient. The attention itself is a plain `matmul` + softmax (`attention`, model.rs:149-153), not a fused kernel.
- The brief's "forward ~100 ms/row on Metal" matches knowledge `facts/laya-rs-candle-ports` (74–111 ms/question f16; 72 ms per 4-question batch): batching helps, so the training throughput estimate above is conservative at batch 8.

## 8. The training cycle

### 8.1 State machine (one per tier)

```
            trigger met                   preconditions ok           venue + slot
  idle ───────────────▶ eligible ───────────────────────▶ acquiring ──────────────▶ running
    ▲                      │                                  │  (checkpoint each epoch)   │
    │                      │ preconditions fail               │ slot/venue unavailable      │ epoch done /
    │                      ▼                                  ▼                             │ run cap
    │                   waiting ◀────────────────────── (retry every 60 s)                  ▼
    │                      ▲     precondition violated mid-run: pause at the step        evaluating
    │                      └──────────────────────────────── boundary, checkpoint ◀─────────┤
    │                                                                                     │
    │              promoted (gate passed, version activated, DoubleVersionActivated)       │
    ◀────────────────────────────────────────────────────────────────────────────────────┤
    │              rejected (gate failed; version kept inactive with metrics)              │
    ◀────────────────────────────────────────────────────────────────────────────────────┘
```

Transitions are rows in `runs` (`state`), published as `DoubleTrainingState` so Settings shows them live. A paused run resumes across nights: the checkpoint is `~/.theuth/double/runs/<id>/epoch-<n>.safetensors` plus the optimizer state; the run cap (default 90 min of `running` per night) is cumulative per run, and a run that has not finished after 3 nights is `rejected: stale` so the corpus can move on. Exactly one training process per machine: a `flock` on `~/.theuth/double/train.lock` and — because cargo gates share the laptop — one `BUILD_SLOTS=1` slot taken through `~/.theuth/bin/build-lock` with `BUILD_LABEL=double-train`. That script takes slot `i` by `mkdir "$DIR/$i"` (atomic on APFS), writes `pid label epoch argv` into `$slot/owner`, releases on EXIT/INT/TERM/HUP and reclaims a slot whose pid is dead (`~/.theuth/bin/build-lock:13-32`); a trainer that dies leaves nothing behind, and a cargo gate started by another session waits politely behind it, which is what Sky asked for in the shared-MacBook rule (knowledge `procedures/theuth-shared-macbook-build-coordination-2026-10-07`).

### 8.2 Local preconditions (all required; re-checked every 60 s while waiting and while running)

| precondition | default | source today | new |
|---|---|---|---|
| inside the window | 02:00–06:00 local | — | `double.training.window` |
| on AC power | required | nothing in crates/platform (env.rs, files.rs, process.rs, shell.rs, target.rs … — no power API) | macOS: `IOPSCopyPowerSourcesInfo` via IOKit, or `pmset -g batt` parsed; Linux: `/sys/class/power_supply/*/online` |
| user idle | ≥ 15 min | nothing; `objc2-core-graphics` is already a workspace dependency (Cargo.toml:293, used by crates/computer) but `CGEventSourceSecondsSinceLastEventType` is referenced nowhere | macOS: `CGEventSourceSecondsSinceLastEventType(kCGEventSourceStateCombinedSessionState, kCGAnyInputEventType)`; Linux: X11 screensaver idle / `org.freedesktop.ScreenSaver.GetSessionIdleTime` |
| no live meeting / talk / dictation | required | the meeting, talk and voice actors are registered by name; a `kameo` lookup answers whether a capture is live | one `Busy` ask to each |
| 1-min load | < cores / 2 | `platform::process` has `proc_pid_rusage` for the daemon (crates/decision/Cargo.toml:35 comment) | `getloadavg(3)` |
| thermal state | nominal | — | `NSProcessInfo.thermalState` (objc2-foundation) on macOS; `/sys/class/thermal` on Linux |
| free disk | > 10 GB | `statvfs` via libc (Cargo.toml:88 comment) | reuse |
| memory pressure | normal | — | macOS `vm_pressure` / `memory_pressure` notification; Linux PSI `/proc/pressure/memory` |
| build slot | one `BUILD_SLOTS=1` slot | `~/.theuth/bin/build-lock` | invoked as the trainer's wrapper |

GPU courtesy: `nice -n 15` on the trainer process, batch cap from `double.training.tiers.b.batch`, Metal only when the display is asleep or the idle precondition holds (a Metal training loop and a user dragging windows do not share a GPU well); when idle breaks mid-run the run pauses at the next step boundary and the display gets the GPU back within one step (≤ 2 s at batch 8). Tier A ignores the window and the power precondition (it is < 2 min of CPU) but still yields to a live meeting / talk / dictation and to a running cargo gate (it does not take a build slot; it checks `/tmp/theuth-build-slots/*/owner` and waits if a slot is held by a label other than its own).

### 8.3 Why not a `schedule` routine

Routines are prompts a model runs (crates/schedule: a run has a `run_timeout_secs` cap, default 1800, crates/schedule/src/lib.rs:53-60; a watcher `when:` is a sensor script under `<data_dir>/sensors/`, crates/schedule/src/file.rs:25); the installed sensors are `fs_exists.py`, `pr_watch.py`, `rss.py` — there is no idle, power or thermal sensor. Training is a deterministic job with its own state machine, not a model turn; wrapping it in a routine would add a model call, a 30-minute cap that is wrong for both tiers, and no preconditions. The knowledge-checks nightly sweep (design 41) is the precedent for a daily in-crate job; double follows that shape with the preconditions above.

### 8.4 Venue: the mesh

`double.training.venue: auto | local | node:<name> | off`.

- **Discovery.** Every engine advertises `EngineStats { class, harnesses, repos, display, gpu, … }` (crates/mesh/src/types.rs:113-131), re-advertised every `mesh.capabilities_secs` (crates/mesh/src/config.rs:34-35) and stored in `engine_stats` (crates/mesh/src/store.rs:144-158). `mesh_peers()` prints `<name> id=<id> <reach> class=<class> harnesses=… repos=… display=… gpu=…[ hub]` (crates/mesh/src/tools.rs:83, 110). On 2026-10-07 01:38 the mesh was exactly one engine: this laptop, `class=laptop … gpu=true`.
- **`gpu=true` is not enough.** `has_gpu()` returns `true` for every Apple-silicon Mac and for any Linux box with a DRM render node or `/dev/nvidia0` (crates/mesh/src/stats.rs:96-109); an Intel iGPU laptop advertises `gpu=true`. `auto` therefore selects a reachable engine with `gpu = true` AND `class ∉ {Laptop, Vm}` (crates/mesh/src/types.rs:82-86; `Vm` is the sandbox class and is never a venue), preferring one whose advertised accelerator is `cuda`. The tree has no accelerator field; §15 C adds `EngineStats.accelerator: Option<String>` (`metal | cuda | webgpu | cpu`, the `models::accelerator` spelling, crates/models/src/accelerator.rs:18-33) and the `t --bench-models`-style probe fills it. Until that lands, `auto` on a one-engine mesh means `local`.
- **Transport.** The mesh already carries typed streams by ALPN over iroh QUIC: `theuth/acp/1` for remote children (crates/acp/src/mesh.rs:1-22, `Transport::open(peer, ALPN_ACP)`), `theuth/artifacts/1` for blob fetch with `LengthDelimitedCodec` + postcard frames and chunked files (crates/artifact/src/mesh.rs:1-14, 253), `theuth/sessions/1`, `theuth/knowledge/1`, `theuth/rpc/1`, `theuth/docs/1` (crates/replica/src/lib.rs:41-55). Double adds `ALPN_DOUBLE = "theuth/double/1"` in replica (one constant, the place every ALPN lives) and a `double::mesh` module modelled on `artifact::mesh`: request `TrainBundle { run_id, manifest, sha }`, chunked bundle, then `TrainResult { run_id, adapter, head, manifest, metrics }` streamed back the same way.
- **The job on the node.** The node runs the trainer as a `jobs`-style process (109 §5: `jobs` rows, pgid kept for the reaper, crates/agent/migrations/0033_durability.sql:74-83) under its own build-lock and preconditions (a desktop node skips the idle check when `display = false`). The bundle carries scrubbed JSONL (`examples` + `labels`), the embeddings, the trainer config and the base checkpoint hash; the node fetches the weights itself through the models downloader (the manifest + sha256 path the engine already uses, crates/decision/src/candle/mod.rs:202-210) — the bundle never carries weights.
- **Activation stays local.** The returned adapter is re-scored on the golden set and the last-7-days proxy on the laptop (inference only) before §7.3's gate decides; a node cannot activate a version.
- **Privacy.** Data leaves the laptop only to the owner's own paired engines (the mesh is the owner's; a joining engine holds the owner's vault), only when `double.training.share_data` is true (default true for owner nodes), and never to a `Vm`-class engine. A node keeps nothing after `TrainResult` is acknowledged (bundle and checkpoints deleted; a `DoubleTrainingState { venue, state: cleaned }` confirms).
- **Where the brief was wrong.** The brief expected a cuda flag on `mesh_peers`; there is none — only `gpu: bool` (crates/mesh/src/types.rs:130). It also named "crates/mesh or crates/nodes" for the stats line; it is crates/mesh/src/tools.rs:83-110, and crates/nodes is the link/device layer (crates/nodes/src/lib.rs:15), not the engine mesh.

### 8.5 Versioning and rollback

`~/.theuth/double/models/<version>/` holds `adapter.safetensors` (Tier B/C), `heads.json`, `knn.bin`, `temperatures.json`, `manifest.json` (tier, base hash, data range, venue, metrics, trained_at) and `active` is a symlink-free pointer file `~/.theuth/double/models/active` naming the version. `double_rollback(version)` rewrites the pointer and sets `versions.active`; the next prediction uses it; every prediction row already names its version, so the curve shows the change as a vertical line. Baseline is `versions.id = 'baseline'`: no adapter, no heads, zero-shot probs only.

## 9. The ladder (autonomy earned per class)

```
            n≥20, acc≥0.90, ECE≤0.05            n≥50, acc≥0.97, ECE≤0.05,            30 days at L2,
            (last 30 d)                          0 overrides in last 20                0 strikes
   L0 ────────────────────────▶ L1 ────────────────────────────────▶ L2 ─────────────────────▶ L3
 observe                      suggest                             act with grace             silent
   ▲                            │                                   │                          │
   │  3 wrong suggestions       │                                   │ any override: one rung   │ any override:
   │  in the last 20            │                                   │ down immediately         │ one rung down
   └────────────────────────────┘                                   ▼                          ▼
                                                                   L1                         L2
```

| rung | behaviour |
|---|---|
| L0 observe | predict, write the row, show a muted chip ("double · Allow · 71 %" in the muted style); never pre-select. |
| L1 suggest | the predicted answer is pre-selected / highlighted; one tap confirms. For `ask_user`: fills `Question.recommended` (crates/tools/src/ask.rs:55-58) in the rendered card when the model left it `None`; when the model set one and double disagrees, the card shows both ("model recommends B · double expects A, 88 %"). |
| L2 act with grace | the card shows a countdown ("double answers Allow in 18 s — tap to take over"); `double.ladder.grace_secs` default 20; when it expires double sends `Resolve` / `answer_ask` with `by: Double`. For mutating approvals the grace **is** the undo: nothing has run until the grace expires, because the registry is still parked at `approval.ask(req)` (crates/tools/src/registry.rs:1585). Taking over (`double_take_over`) cancels the timer and counts as an override only if the owner then answers differently. |
| L3 silent | answers at once, no countdown; a line in the daily digest. `ask` kinds and `read_only` approvals only. |

Caps (`classes.cap`, and the global defaults in `double.ladder.caps`): destructive ≤ L1 always; mutating approvals ≤ L1 unless `classes.mutating_opt_in` (per-class toggle in Settings, default off); content flagged by 28 ≤ L1 when 28 ships; `none` winning or `Escalate` → L0 behaviour for that card; `double.enabled: false` → nothing; `pause learning` → predictions and rungs continue, labels stop accruing (the curve shows the gap). Routines with `approvals: false` raise no cards and are untouched. A child's approvals go to the owner and therefore to the ladder, but under their own requester (`subagent`, `desk:<agent>`, `routine:<name>`) — a class learned on `root` never transfers.

Promotion is evaluated after every label from the class's last 30 days of prequential rows (the live ones; bootstrapped rows never count). Demotion is immediate on the override event. A strike at L2 is any override or any hindsight negative with weight ≥ 0.5 on an acted row.

### 9.1 A worked example

Day 1: Sky opens a wave; the first `git push` approval creates `approval:shell:mutating:git push:root` at L0; the chip says "double · Allow · 58 %" (zero-shot, muted); Sky clicks Allow; the row is labelled correct. Day 3: 24 rows, 23 Allow; k-NN now dominates the head; accuracy over the last 30 days 0.96, ECE 0.03 → L1; the Allow button is pre-selected. Day 9: 61 rows, 60 correct, no overrides in the last 20 → eligible for L2, but the class is `mutating` and `mutating_opt_in` is off → capped at L1; the classes table shows "L1 (cap: mutating — opt in to allow L2)". Sky opts in. Day 10: cards from Sky's own sessions show "double answers Allow in 20 s"; a `git push --force` from a subagent is a different class (`:subagent`) at L0 and `--force` is far from the cluster in embedding space — chip muted, nobody acts. Day 15: Sky takes over one card and clicks Deny (a push to the wrong remote) → override (weight 3) → L1 at once; the Settings log says "demoted: override on 2026-10-22 14:02". Day 22: 20 more correct → L2 again. Throughout, the baseline curve for the class sits at ~0.6 and the active curve at ~0.97; the gap is the number the page shows.

### 9.2 Where the brief was wrong (ladder)

The brief says L1 "fills `recommended` when the model did not". `Question.recommended` (crates/tools/src/ask.rs:55-58) is part of the published `AskRequested` payload and of the journaled row; double must not rewrite the payload (the journal is the record of what the model asked). The UI fills the pre-selection from `DoublePredicted` instead and renders the model's own `recommended`, when present, beside it. Same effect, no mutation of the question.

## 10. Surfaces (make it visible)

**Desktop (crates/desktop/ui).** The chip is one component, `DoubleChip`, rendered from the `DoublePredicted` event keyed by `question_id`, on every card: `ApprovalCard` (crates/desktop/ui/src/components/chat/ApprovalCard.tsx:29-33, next to the `approval: tool (risk)` title at :52-54), `AskCard` (crates/desktop/ui/src/components/chat/AskCard.tsx:159, per question), the attention row and the decision/review screens (101/103 packet options), and the draft card set. Text "double · Allow · 91 %" plus a rung glyph (○ L0 · ◐ L1 · ● L2 · ●● L3). Tap → popover: class key, rung, what would promote it ("12 more correct in a row"), the top-3 nearest past decisions with date and what the owner chose, the head's contributions, and a "don't learn from this one" toggle. At L2 the countdown replaces the chip text and the card's buttons stay live. Attention inbox (104): a new `AttentionKind::Double` item per acted card ("double answered Allow for `git push` · undo 14 s"), dismissed per 119's versioning; a daily digest item at L3. Settings › Double, a new tab in `TABS` (crates/desktop/ui/src/components/settings/SettingsDialog.tsx:45-62), beside Labs where the oracle switches already live (crates/desktop/ui/src/components/settings/Labs.tsx:1-4): status header (enabled, paused, active version, last run when/where/duration/result, next window), the Improvement chart — design 123's own SVG kernel `LineChart` (docs/design/123-diagram-grammars.md:17; the UI has no chart library today — no recharts in crates/desktop/ui/package.json; `Sparkline` is hand-drawn in components/chat/surface/registry.tsx:515) with weekly prequential accuracy and coverage@2 %, current vs baseline; KPI tiles (asks avoided this week, minutes saved, overrides); the classes table (class, n, rung, acc, ECE, cap, mutating opt-in, last decision; row actions: cap, opt in, forget, examples); training controls (Train now: tier A / tier B local / tier B on node:<name>; Pause learning; Rebuild (tier C); Roll back; window and venue; consent toggles: capture approvals / asks / attention / packets / drafts, share dataset with mesh nodes; Export dataset; Delete all double data); the training log table (`runs`). Bindings: the ts-rs exports regenerate with `just ts-desktop` (justfile:90-97) — the UI wave hand-writes the stubs from the contract and the integrator regenerates.

**TUI (crates/tui).** The chip text in the approval / ask prompt line (`Command::Resolve` / `Command::AnswerAsk`, crates/tui/src/lib.rs:339-345 is the answer path); a `/double` command: `status`, `off`, `pause`, `train [a|b] [node]`, `rollback <version>`.

**Slack (110, crates/slack).** A context line "double would: Allow (91 %)" appended to `approvals::card` (crates/slack/src/approvals.rs:17) and `asks::question_card` (crates/slack/src/asks.rs:22); an L2/L3 answer posts one line in the thread via the existing `resolved_card` / `answered_card` path (crates/slack/src/approvals.rs:41, crates/slack/src/asks.rs:78). Signal and Telegram get the same one line in their text cards (the `approvals.rs` / `asks.rs` modules of each crate).

**Chat runtime functions** (registered by the double plugin like `decide` is by decision, crates/decision/src/tool.rs): `double_status()`, `double_classes()`, `double_class(key)` — `read_only`; `double_train(tier, venue)`, `double_pause(on)`, `double_forget(class | example_ids)`, `double_rollback(version)`, `double_cap(class, rung)` — `mutating`. They are the model's view; they never answer a card.

**Mobile.** Chips and the Settings page follow the desktop; the L2 countdown must be visible on a phone card (the card is already the bottom-stacked prompt, 112), and the take-over target is the whole card.

## 11. Docs and prompt

| file | content |
|---|---|
| `docs/double.md` | user guide: what it is; what it learns from and what it never does; the ladder in plain words; every control; privacy and the mesh; FAQ "why did it answer X" (the popover's explanation), "how do I make it forget" (`forget` per class / example, Delete all), "why did it stop acting" (an override demoted it). |
| `crates/double/double.md` → `<data_dir>/skills/double.md` | the `double` skill, ≤ 40 lines: how the reasoning model treats chips and `none`; never argue with the double; a double answer is the owner's answer; when the model disagrees with an L1 suggestion it says so in one line and lets the owner decide. Installed the way `decide.md` is (knowledge `projects/theuth-decide-tool-build-2026-10-04`). |
| system prompt | a ≤ 8-line `## Double` paragraph registered through `RegisterPrompt` only when `double.enabled` (92's rule for the `## Bulk classification` paragraph). |
| `docs/design/126-as-built-<wave>.md` | one per wave. |
| release note stub | `docs/release-notes/<version>.md`: "double: …". |
| knowledge | `projects/theuth-design-126-double-2026-10-07` updated after landing; a `facts/` entry with the first published curve. |

## 12. Config keys (`double:` section)

config.yaml is read once at startup (the 00 rule); like `decision.families.*` (which `set_decision_mode` patches live and persists, crates/rpc/src/api.rs:1400-1409), every `double.*` key below is **live** through its RPC setter and persisted to config.yaml, except `enabled` (registers tools and the prompt paragraph — restart) and `model.*` (a different checkpoint or adapter rank — restart).

```yaml
double:
  enabled: false                      # restart
  capture: { approvals: true, asks: true, attention: true, packets: true, drafts: true }
  ladder:
    grace_secs: 20
    caps: { read_only: 3, mutating: 1, destructive: 1 }   # max rung by risk; mutating 3 only with per-class opt-in
    mutating_opt_in: false            # default for new classes
  training:
    venue: auto                       # auto | local | node:<name> | off
    window: "02:00-06:00"
    idle_min: 15
    run_cap_min: 90
    triggers: { labels: 200, days: 7 }  # tier B; tier A: every 25 labels or nightly
    share_data: true
    tiers:
      a: { enabled: true }
      b: { enabled: true, rank: 8, batch: 8, epochs: 3, lr: 2.0e-4 }
      c: { enabled: false }           # owner action only
  retention_days: 365
  model:
    checkpoint: default               # a `decision.checkpoints` name; restart
    adapter_rank: 8                   # restart
```

The decision side gains `decision.families.double.*` entries (mode `active`, `none: true`, `cut_ok: true`, `embed: true`, `train: false` — double keeps its own corpus, 57 §11 capture stays off for these families) and the `double.correction` Bool family.

## 13. RPC and events

Following the `Api` trait's flat naming (`#[rpc(server, client)] pub trait Api`, crates/rpc/src/api.rs:160-161; `decision_mode` / `set_decision_mode` at :1398-1409):

| method | returns | risk |
|---|---|---|
| `double_status()` | `DoubleStatus { enabled, paused, active_version, last_run, next_window, venue, kpis }` | read |
| `double_classes()` | `Vec<DoubleClass>` | read |
| `double_class(key)` | `DoubleClassDetail { class, curve, examples (last 50, scrubbed), neighbours }` | read |
| `double_train(tier, venue)` | `RunId` | mutating |
| `double_pause(on)` | `DoubleStatus` | mutating |
| `double_forget(target)` | count | mutating (`target = class | example ids`) |
| `double_rollback(version)` | `DoubleStatus` | mutating |
| `double_set_cap(class, rung, mutating_opt_in)` | `DoubleClass` | mutating |
| `double_export()` | path of a scrubbed JSONL + embeddings archive under `~/.theuth/double/export/` | mutating |
| `double_delete_all()` | () — store, models, runs removed; config untouched | destructive |
| `double_take_over(question_id)` | bool — cancels a grace; the owner answers through the ordinary `resolve_approval` / `answer_ask` | mutating |
| `double_settings_get()` / `double_settings_set(settings)` | the `double:` section, persisted and applied live, as `search_settings_get/set` do (crates/rpc/src/api.rs:1413-1422) | read / mutating |

Events, forwarded by the bridge's `forward!` list (crates/rpc/src/bridge.rs:95-130) and subscribed at start (crates/rpc/src/bridge.rs:147-159): `DoublePredicted { question_id, class, choice, p, probs, rung, grace_until, version, explain: [Neighbour; 3] }`, `DoubleAnswered { question_id, class, choice, version, undo_until }`, `DoubleTrainingState { run_id, tier, venue, state, progress, message }`, `DoubleVersionActivated { version, metrics, previous }`. All four derive `ts_rs::TS` under the `ts` feature like `ApprovalRequested` (crates/tools/src/approval.rs:38-40), so `just ts` exports them.

### 13.1 The wire types (contract for C; `ts-rs` exported)

```rust
pub struct DoublePredicted {
    pub question_id: String,        // ApprovalId / AskId / packet key / draft id / attention item id
    pub kind: CardKind,             // approval | ask | attention | packet | draft
    pub class: String,              // §3 key
    pub choice: String,             // option label as rendered on the card ("Allow", "Start now", …)
    pub index: Option<usize>,       // ask: the option index the choice maps to
    pub p: f32,                     // calibrated top-1
    pub probs: Vec<(String, f32)>,  // every option incl. "none"
    pub rung: u8,                   // 0..=3, the class's rung when predicted
    pub will_act_at: Option<String>,// RFC 3339 grace end at L2; None at L0/L1; Some(now) at L3
    pub version: String,
    pub explain: Explain,           // { neighbours: [{asked_at, card_excerpt ≤ 160 chars, chose}; ≤ 3],
                                    //   parts: {oracle, memory, prior}, promote_hint: String }
}
pub struct DoubleAnswered { pub question_id: String, pub class: String, pub choice: String, pub version: String, pub undo_until: Option<String> }
pub struct DoubleTrainingState { pub run_id: String, pub tier: Tier, pub venue: String, pub state: RunState, pub progress: Option<f32>, pub message: String }
pub struct DoubleVersionActivated { pub version: String, pub previous: Option<String>, pub metrics: serde_json::Value }
pub enum Answerer { Human, Double }   // the new `Resolve.by` / `AnswerAsk.by`; serialises as "human" | "double"
```

The desktop keeps predictions in a small zustand store keyed by `question_id` (like `useApprovals`, crates/desktop/ui/src/components/chat/ApprovalCard.tsx:41) and drops them on `ApprovalResolved` / `AskResolved`; on (re)subscribe the engine re-publishes `DoublePredicted` for every pending card so a reload shows the chips and countdowns again (§4.4).

## 14. Security and privacy

- **Untrusted content (28 not built).** A prompt injection can shape a card to look like a routinely approved class — a `shell` command that starts `git push` and continues `&& curl …`. Defences, in order: the class key includes the risk and the requester, so a `destructive` or a `routine:*` card is never in the owner's routine class; destructive is capped at L1 for ever; mutating approvals need the per-class opt-in; the shape rule keys `shell` on the first two tokens but the k-NN runs on the whole command text, so a `git push && curl` is far from the `git push` cluster and `none` wins; `Escalate` from the gate is L0 behaviour. None of this is a substitute for 28; it bounds the blast radius to what L1 can do, which is highlight a button.
- **Data never leaves the owner's mesh.** Bundles go only to paired engines with `share_data` on; sandbox VMs are excluded; nothing is uploaded to theuth (the 57 §7.1 `share` consent is a different, separate switch and stays off for `double.*`).
- **Scrubbing.** Every `card_text`, option label and exported row passes `tools::secrets::redact_value` (crates/tools/src/secrets.rs:77); the embedding is computed from the scrubbed text, so a secret never enters the k-NN memory either.
- **Export and delete.** `double_export` produces the owner's own data in the open 57 §11 JSONL shape; `double_delete_all` removes store, models and runs; both are Settings buttons.
- **Audit trail.** Every double answer is a `questions` row with `answered_by = 'double'` (§4.3) and a `predictions` row with `acted = 1`, so "what did double do last night" is one query, and 109's resume logic sees an answered question exactly as it would a human's.
- **What double must never do.** Answer secrets, handoffs or MFA; approve destructive; act on a card it did not predict before the owner saw it (an `acted` row without a prior `predictions` row is a bug the W1 tests assert against); answer with `None` to an ask (that cancels the turn); train while the owner is active (§8.2); activate a version a node trained without the local golden re-score.

## 15. Waves (implementation plan)

Each wave is a worktree off main with its own `CARGO_TARGET_DIR`, crate-level gates only, and these lines verbatim in every brief: "Every cargo / tsc / vitest invocation goes through `BUILD_LABEL=<wave> /Users/Sky.Moore/.theuth/bin/build-lock <cmd>`; crate-level gates only (`-p <crate>`); CARGO_BUILD_JOBS=4; one build at a time per worker; workspace-wide runs only by the integrator with BUILD_SLOTS=1."

| wave | owns (design sections) | crates / files | gates | size |
|---|---|---|---|---|
| **W0 spike** (concurrent with C) | §7.3 feasibility | `crates/decision/examples/lora_step.rs` (not shipped in the binary); a `rope_slow` switch behind a `train` cfg in `crates/decision/src/candle/model.rs:75-76` | `cargo run -p decision --example lora_step` on Metal and CPU (+ CUDA on a node if one exists); a report with step time, peak memory, loss over 50 steps — or the failing op and the sidecar decision | 1 omp, half a day |
| **C contract** | §3 keyer, §5 schema, §12 config, §13 types + RPC stubs, §7.2 embedding exposure, §4.3 `answered_by` column + `Resolve.by` / `AnswerAsk.by`, `ALPN_DOUBLE`, `EngineStats.accelerator` | new `crates/double` (lib.rs, class.rs, store.rs + `migrations/0001_double.sql`, config.rs, events.rs, rpc stubs in `crates/rpc/src/api.rs`), `crates/decision/src/candle/model.rs`, `engine.rs`, `gate.rs`, `crates/tools/src/approval.rs`, `ask.rs`, `journal.rs`, `crates/agent/migrations/0034_double.sql`, `crates/replica/src/lib.rs`, `crates/mesh/src/types.rs` | `cargo test -p double -p decision -p tools -p agent -p replica -p mesh`; `just ts` | 1 omp, 1 day |
| **W1** | §4 subscriptions, §6 labeller (+ bootstrapper `t --double-bootstrap`), §7.1 predictor tiers 0/1, prequential ledger, §9 ladder state machine, `double_take_over` | `crates/double/src/{actor,labels,predict,ladder,bootstrap}.rs`, `crates/theuth/src/main.rs` (flag) | `cargo test -p double`; a fake-oracle test that asserts "no acted row without a prior prediction row" | 1 omp, 2 days |
| **W2** | §7.2 Tier A trainer, §8.1–8.3 scheduler + preconditions, §8.4 venue chooser + mesh bundle job, §8.5 versioning/rollback | `crates/double/src/{train_a,sched,precond,venue,mesh,versions}.rs`, `crates/platform/src/power.rs` + `idle.rs` (new) | `cargo test -p double -p platform`; a two-engine smoke like 68's (docs/design/68-two-engine-smoke.md) with a fake GPU node | 1 omp, 2 days |
| **W3** (after W0) | §7.3 Tier B LoRA trainer (candle in-process, or the sidecar if W0 said so), promotion gate, golden re-score | `crates/double/src/train_b.rs`, `crates/decision/src/candle/lora.rs` | `cargo test -p double -p decision`; one real run on the bootstrapped corpus with the measured numbers in the as-built | 1 omp, 2 days |
| **W4** | §10 desktop: `DoubleChip`, countdown, inbox rows + `AttentionKind::Double`, Settings › Double with the 123 `LineChart`, bindings regen | `crates/desktop/ui/src/components/{chat/DoubleChip.tsx, settings/Double.tsx, attention/...}`, `crates/roster/src/attention.rs` (new kind) | `tsc`, `vite build`, `bunx vitest run src/components/settings src/components/chat`; stubs hand-written from C's contract, regenerated by I | 1 omp, 2 days |
| **W5** | §10 TUI + Slack/Signal/Telegram lines + chat runtime functions; §11 docs, skill, prompt paragraph, release note | `crates/tui`, `crates/slack`, `crates/signal`, `crates/telegram`, `crates/double/src/tool.rs`, `crates/double/double.md`, `docs/double.md` | `cargo test -p tui -p slack -p signal -p telegram -p double` | 1 omp, 1 day |
| **I integration** | merge order C → W1 → W2 → W4 → W5 → W3; `just ts-desktop`; the 14-day L0 shadow-run plan (`double.enabled: true`, every class capped at L0, the curve published to Settings and to a knowledge entry on day 14); the firm targets of §7.4 | — | workspace `cargo check`, `cargo test -p` for every touched crate, desktop tsc/vite/vitest, with `BUILD_SLOTS=1` | orchestrator + 1 closer omp |

Dependencies: the 0033 engine rebuild must ship before W1's live labels flow (W1 can land before it; the bootstrapper and predictions do not need it). W3 waits for W0. W4 can start from C's TypeScript contract before W1 finishes. One owner per design section; two waves that touch the same file (C and W1 both touch `crates/double/src/lib.rs`; C and W4 both touch `attention.rs`) get the exact insertion points named in their briefs.

### 15.1 Risks the waves must carry in their briefs

| risk | mitigation owned by |
|---|---|
| the rope backward (§7.3) stays unsolved on Metal | W0 decides CPU/Accelerate at night vs sidecar; W3 implements the decision; the ladder, Tier 0/A and the curve do not depend on Tier B at all |
| the 0033 engine ships late | W1's bootstrapper and bus-event labels (`ApprovalResolved`, `AskResolved`) carry the design until it does; §6.3 |
| a wave night of 500 `cargo test` approvals skews the k-NN | time decay (30-day half-life) and per-class rehearsal caps; W1 tests with a synthetic burst |
| `has_gpu()` lies about training-grade GPUs (§8.4) | C adds `accelerator` to `EngineStats`; `auto` requires it or falls back to local |
| the countdown is missed on a phone | W4: the card is the bottom stack (112), the whole card is the take-over target, and the inbox row carries the undo |
| a destructive tool misclassified as mutating by its `ToolSpec.risk` | out of scope for double (the registry's risk is the contract, 00 §1 item 7) — but double's chip makes a wrong risk visible, and the as-built lists any it finds |
| two sessions' orchestrators both land on 126 | this document and `~/.theuth/briefs/2026-10-07/126/` claim the number; the sibling 125 has no file yet |

## Open questions

Only Sky can decide these; each has the default the design assumes.

1. **Class merge/split in v1.** Default: no. Classes are keys; a wrong split costs a slower climb, a wrong merge costs a wrong answer.
2. **Share the adapter with the meeting oracle.** Default: no, opt-in per family (`decision.families.meeting.*.adapter: double`) after the double curve has been published for a month.
3. **Grace length.** Default 20 s; 10 s on the phone card is tempting and too short to read a shell command.
4. **Should L3 exist in v1.** Default: ship L0–L2; L3 behind `double.ladder.caps.read_only: 3` which defaults to 2 in the first release.
5. **Venue when the only GPU node is a sandbox VM.** Default: never — it is the laptop's own GPU behind a hypervisor.
6. **Retention.** Default 365 days, golden rows and the last 1,000 per class exempt.
7. **Expose double to desks' and agents' own approvals.** Default: the owner's cards only in v1; a desk's children raise their approvals to the owner anyway (they appear under `desk:<agent>` requesters).
8. **Bootstrapped examples in the k-NN memory from day one.** Default: yes (they are labelled data), weight 0.75, never in the curve.
9. **The sidecar fallback.** If W0 fails on Metal, default: train Tier B on CPU/Accelerate at night (slower, inside the cap for small corpora) rather than add a Python dependency; a Python sidecar only on a node.

## Revision 1 — Sky's rulings on the open questions (2026-10-07 08:40)

Asked with explanations and answered by Sky on two ask_user cards (08:33–08:40). Where a ruling differs from the v0 default, the section it changes is named; the body text above is v0 and this section is binding.

| # | Question | Ruling | Changes |
|---|---|---|---|
| 1 | Class merge/split in v1 | **No merge/split** (default) | — |
| 2 | Share the adapter with the meeting / other families | **`double.*` only; per-family opt-in later** (default) | — |
| 3 | Grace length at L2 | **30 s**, not 20 s | §9 and §10: `double.ladder.grace_secs` default 30; the countdown copy and the Slack/TUI lines read 30 s; §12 default value |
| 4 | L3 in v1 | **L0–L2 only; L3 behind the cap defaulting to 2** (default) | — |
| 5 | Sandbox VM as venue | **Never** (default) | — |
| 6 | Retention | **365 days, golden rows and the last 1,000 per class exempt** (default) | — |
| 7 | Whose cards | **The owner's cards only** (default) | — |
| 8 | Pre-0033 history in the k-NN memory | **No — journal rows only** | §6.4 bootstrap is NOT built in v1: double starts cold when the 0033 engine ships; §6.5's volume estimate stays as sizing information only; W1 loses the bootstrap task; the §0 item 11 sentence "Pre-0033 history is bootstrapped from `messages` bodies" is superseded |
| 9 | Sidecar fallback if W0 fails on Metal | **No fallback decided — "we'll need to do more research in that case"** | §7.3 / §15 W0: a W0 failure ends in a research note (what failed, the candle fix options, the sidecar options with their dependency cost) and a new ask to Sky, not in an automatic CPU/Accelerate or Python path; W3 is not briefed until that ruling exists |

Sky's words on 9: "We'll need to do more research in that case."

## Revision log

- v0 · 2026-10-07 · written from the brief at ~/.theuth/briefs/2026-10-07/126/brief.md. Writer's limits: no `shell()` in the runtime (no sqlite volume queries, no `git log`; main sha read from `.git/refs/heads/main`); every `path:line` cite verified by `read_file` / `grep` on the working tree at f2225972; candle facts read from the cargo registry sources of the pinned 0.11.0 crates, nothing executed. Committed on main `e70471e6` (02:30).
- Revision 1 · 2026-10-07 08:40 · Sky's nine rulings (table above); grace 30 s, no bootstrap, W0 failure → research, the other six defaults confirmed.

## Revision 2 — the W0 spike's findings and Sky's Tier B venue ruling (2026-10-07 12:40)

Source: `docs/design/126-as-built-W0.md` (spike `spike/126-w0-lora`, commit 3519322c; cherry-picked onto `feat/126-w3` as c72e428d…74977c67). Sky's rulings on two ask_user cards at 12:40 after reading the spike's open items 1–2. Binding over §7.3, §7.6, §8.2 and §15 W3 where they differ.

### What the spike measured (Mac15,8, M3 Max class, 64 GB, candle 0.11, Metal f16, release)

- **The blocker was mis-described in §7.3/§7.6.** `candle_nn::rotary_emb::rope`, `ops::softmax_last_dim` and the biased `LayerNorm` path are applied with `apply_op*_no_bwd`: `backward()` returns `Ok` and the gradient is **silently dropped** (graph cut; loss flat) — there is no `BackwardNotSupported` error. Under a `train` cargo feature (off by default, default forward byte-identical) the three are swapped for their differentiable siblings (`rope_slow`, `softmax(D::Minus1)`, `layer_norm_slow`); after all three swaps the gradient crosses all 28 encoder layers (q/k through rope and v all receive gradient at layers 0 and 27).
- **Mechanics: SUCCESS.** Rank-8 LoRA on 112 matrices (3,598,336 trainable f32 params, 14.4 MB), base frozen f16: at batch **2 × 256**, lr 1e-4, **1,089 ms/step** steady (fwd ≈ 170 / bwd ≈ 750 / opt ≈ 85 ms), **12.2 GB physical footprint** (RSS 1.9 GB — Metal buffers count in phys_footprint; candle's Metal allocator never frees within a step), loss 1.4660 → 0.0012 (step 25) → 0.0001 (step 50) monotone; adapter saved via `VarMap::save` (14,417,560 bytes).
- **The design's batch does not exist here.** 8 × 512 OOMs in debug (`kIOGPUCommandBufferCallbackErrorOutOfMemory`) and in release did not finish step 1 in 9.5 min (no activation checkpointing in candle; attention scores 8 × 16 × 512 × 512 f16 = 64 MB per layer per tensor × 28 layers kept in the graph). §7.3's "5,000-row epoch ≈ 25 min at 8 × 512" is replaced by **≈ 45 min at 2 × 256** (0.54 s/row).
- **Interactivity.** Even the one-minute 2 × 256 run made the owner's Mac unusable; Sky stopped it at 11:17 ("the lora training that's happening right now is making my machine unusable, stop it"). Metal command buffers cannot be throttled. A decision-crate test run that loads the checkpoint (laya on Metal) had the same effect and was killed at 11:19.
- CPU/Accelerate: **unmeasured** (pre-empted by the stop). Not a fallback decision — Revision 1 ruling 9 stands.

### Rulings (Sky, 12:40)

1. **Tier B venue: node-first, local behind an off-by-default flag.** `double.training.venue: auto` resolves Tier B to a mesh GPU node (`EngineStats { gpu: true, accelerator: Some(Cuda), class ∉ {Laptop, Vm} }`); with no node, Tier B is inactive and the k-NN memory + Tier A carry the feature and the curve. A new key `double.training.tiers.b.local` (default `false`) enables local Tier B; when enabled it runs only inside the window (default 02:00–06:00), on AC, with the owner idle ≥ 15 min at start, holding a `BUILD_SLOTS=1` build slot, **and aborts within one step on any keyboard/mouse activity** (checkpoint the adapter and optimiser state, resume in the next window) — an abort-on-activity detector, not only a start gate. §8.2's preconditions are necessary, not courtesy.
2. **Batch.** Micro-batch 2 × 256 with gradient accumulation to a configurable effective batch (default 16); the rehearsal sampler caps the corpus (default 2,000 rows ≈ 18 min at the measured rate); run cap 90 min per night stands.
3. **W3 is briefed now**, in parallel with W1/W2/W4/W5, on `feat/126-w3` (C's contract + the four spike commits): the trainer, the node job entry, fixture-level tests on a tiny model config, and a documented node run recipe — **no training run and no checkpoint load on the owner's machine during the build**.
4. **Operational rule for the waves (Sky, 11:17–11:19):** no test, example or job that loads a model checkpoint or trains runs on the owner's machine while he uses it; the decision crate's checkpoint tests stay gated (`DECISION_EMBED_TEST=1`, off).

### Revision log (continued)

- Revision 2 · 2026-10-07 12:40 · W0 findings (silent graph cut, 2 × 256 numbers, 8 × 512 unreachable, machine unusable) and the Tier B venue/batch rulings; W3 briefed.


