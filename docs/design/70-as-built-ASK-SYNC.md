# 70 — as built: ASK-SYNC (prompts stay consistent across every client)

Branch `fix/ask-sync-across-clients`, 2026-09-27. Defect (Sky, ~14:40): chat open on the iPad over the relay; the
`ask_user` card was answered on the Mac; the iPad kept the actionable card through backgrounding, foregrounding and
reconnects. Only quitting the app cleared it.

Rule: a prompt (approval, `ask_user`, `secret_request`) answered anywhere is gone everywhere within one event
round-trip, and a client's pending set after any (re)subscribe is exactly what the engine reports — nothing more.

## Root cause (traced, reproduced)

The live path was never the problem. Two clients on one engine both receive `AskResolved` (smoke below: B live
saw it). The iPad was in a **gap** — iOS suspends the socket without telling the page (`rpc/client.ts:41-49`),
`LinkTransport` reconnects, `RpcClient` re-issues `subscribe_events` (`client.ts:66-71`) and "the gap between is
lost by design" — and nothing on either side ever reconciled the pending prompts after the gap:

- The three pending stores (`store/approvals.ts:22-34`, `createPending`) shrank **only** on a live `*Resolved`
  (`store/sessions.ts:598-604`). No `clear`, no `replace`. `reload()` on reconnect/`Lagged` (`App.tsx:100-117`)
  reset sessions and queries but never touched `approvals`/`asks`/`secretRequests`; the engine-switch cleanup
  (`App.tsx:430-436`) did not either. A `*Resolved` missed in the gap left the card until `dropScope` — i.e. app quit.
- The engine replayed **pending asks only** on subscribe (`rpc/src/api.rs:4359-4377`, `tools::PendingAsks`), never
  approvals or secret requests (`Approval.pending` kept only the `oneshot::Sender`, `approval.rs:96`; `SecretsActor`
  kept `(session, sender)`, `secrets/lib.rs:196`), and replay could only **add** (`add` dedups by id) — nothing said
  which previously seen prompts were no longer pending.
- `Lagged` (broadcast ring of 1024, `bridge.rs:43`) drops events silently; a dropped `*Resolved` had the same effect.
- Secondary: answering a stale card returned `false` (`answer_ask`/`resolve_approval`) or `()` (`answer_secret`, which
  never reported "not pending"), the cards only toasted (`AskCard.tsx:151`, `ApprovalCard.tsx:39`) and stayed actionable.
- Key mismatch was ruled out: `AskResolved.id: AskId` (`ask.rs:94-101`) = `AskRequested.id`, `#[serde(transparent)]`
  uuid; same for `ApprovalId` and secrets `RequestId`. `wanted()` (`api.rs:4442-4455`) passes everything for the UI's
  `(null, null)` subscription. The bridge forwards all six events (`bridge.rs:90-120`).

Evidence, pre-fix binary (`1af0df84`, scratch engine, scripted OpenAI gateway returning an `ask_user` call, three raw
WebSocket subscribers; `/tmp/69/smoke.ts`): A, B, C all receive `AskRequested`; C closes its socket; A answers
(`answer_ask → true`, again → `false`); B receives `AskResolved`; C reconnects and re-subscribes and receives **no
prompt-related item at all** — its store would keep the card forever.

## What changed

### Engine

- `crates/rpc/src/api.rs`: new wire type `PendingPrompts { approvals, asks, secrets }` (`#[serde(tag = "type")]`,
  exported to TS). `PendingPrompts::collect(keep)` asks the three actors and filters each item through the same
  `wanted()`/`admitted()` as live events. `subscribe_events` sends it as the **first item of every stream, empty or
  not** (replaces the `PendingAsks` loop), and again after a `Lagged` — the dropped events may have held a `*Resolved`.
- `crates/tools/src/approval.rs`: `pending` keeps the `ApprovalRequested` beside the sender; `PendingApprovals` →
  `Vec<ApprovalRequested>` (re-exported from `tools`).
- `crates/secrets/src/lib.rs`: the waiter keeps the `SecretsEvent::Requested`; `PendingRequests` → `Vec<SecretsEvent>`.
- `answer_secret` → `RpcResult<bool>` (`true` if it was still pending), like `answer_ask`/`resolve_approval`.
- `crates/rpc/src/client.rs` `mirror`: a `PendingPrompts` item is unpacked and each prompt published on the local bus,
  so the TUI (which attaches through `mirror`) still sees pending cards as `*Requested`.

### Desktop / mobile UI (`crates/desktop/ui`)

- `store/approvals.ts`: `replace(requests)` — the authoritative snapshot; `resolved: Set<id>` remembers what `resolve`
  dropped so a `*Requested` that arrives after its `*Resolved` (reordered across a reconnect) is ignored; `replace`
  clears it.
- `store/sessions.ts` `dispatch`: `PendingPrompts` → `replace` on all three stores (`SecretsChanged` filtered out of
  `secrets`). `rpc/events.ts`: `PendingPrompts` in `WireEvent`.
- `AskCard`, `ApprovalCard`, `SecretRequestCard`: a `false` from the answer RPC drops the card locally
  (`resolve(id)`) and toasts "already answered/decided" — the tool row's result is the answered state
  (`MessageTool.tsx:126` for `ask_user`). `rpc/api.ts`: `answer_secret(): Promise<boolean>`.

Nothing in `reload()`/`App.tsx` changed: every (re)subscribe is a fresh stream and the snapshot is its first item, so
the stores are reconciled without the client knowing why it re-subscribed.

## Tests

- Rust `crates/rpc/tests/prompts.rs` `prompts_resolve_everywhere_and_replay_only_while_pending` (own binary: it
  registers the `ask`/`approval` actors, which the process-wide registry allows once): real `bridge::start`,
  `AskUser` + `Approval` actors registered, `Server::into_rpc()` `Methods` with an `RpcPeer`; A subscribes (empty
  snapshot), an ask and an approval are raised, B subscribes and gets both in its snapshot, `answer_ask`/
  `resolve_approval` return `true` then `false`, A and B both receive `AskResolved`/`ApprovalResolved` by id, C
  subscribes and gets an empty snapshot.
- vitest `src/store/approvals.test.ts`: `resolve` drops by id, a late `Requested` for a resolved id is ignored,
  unknown id is a no-op; `dispatch([PendingPrompts])` replaces asks/approvals/secrets (non-prompt `SecretsChanged`
  filtered), and a later snapshot is authoritative over the resolved memory.

## Verification (scratch engine, `THEUTH_DATA_DIR=/tmp/69/asksync-data`, `THEUTH_SECRETS__KEY_STORE=file`)

- Wire (`/tmp/69/smoke.ts`, fixed binary): every subscriber's first item is `PendingPrompts`; D subscribing while the
  ask is pending gets it in the snapshot; A answers → B and D receive `AskResolved`; second `answer_ask` → `false`;
  C re-subscribes → `PendingPrompts asks=[]`. PASS.
- Browser (Vite dev UI, two headless Chromium tabs on the same session, WebSocket instrumented via init script):
  tab B opened after the ask → card rendered from the snapshot; B's engine socket closed and reconnects blocked while
  A clicked "Send answers"; B showed "Reconnecting…"; back online → `RpcClient` re-issued `subscribe_events`, the
  first item was `PendingPrompts []`, the card was gone and the row read "The agent asked … red / noted." No
  `AskResolved` ever reached B. Live case (B connected while A answers): card gone on `AskResolved`.
- Gates: `cargo check --workspace --tests`; `cargo test -p tools -p secrets -p rpc`; `tsc -b --noEmit` 0;
  vitest 96 files / 564 tests green.

## Not exercised

- Real iOS/iPad over the relay (`LinkTransport`); the reconnect path is the shared `RpcClient` (`#subs` re-issued on
  `open`), exercised here with the local transport in Chromium.
- Approvals and `secret_request` in the browser (the snapshot/replace path is one code path for all three; approvals
  are covered by the Rust test, secrets by the store test only — no `SecretsActor` in the rpc unit test).
- The "stale card answered" UI branch (`answer_* → false` → local drop): with the snapshot in place the stale card no
  longer occurs in the tested scenarios; the branch is covered by the `false` return in the Rust test and by reading.
- `descendants: true` subscribers still miss `*Resolved` events (they carry no `session`, `wanted()` tree mode drops
  them); the UI does not use tree mode. Left as is.

DONE ASK-SYNC
