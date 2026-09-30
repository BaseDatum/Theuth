# 79 — As built: H (hand-off and secure login)

Branch `feature/79-H`, base `9f282800` (K), worktree `../theuth-79-H`. Design §0 decision 15, §6.6; contract §2.7, §2.4 (`handoffs_list`, `handoff_resolve`), §2.5 (`desktop_handoff`, `secret_request_form`). Rows replicate; no bus event crosses an engine (`EventRelay` untouched).

## Built, per commit

1. `05ff9a27 feat(secrets)` — `migrations/0002_requests.sql` (`secret_requests`, §2.7 columns verbatim), `src/requests.rs` (`insert`, `get`, `list(pool, unanswered_only)` newest first, `answer(pool, id, outcome, now_ms) -> bool` — only an unanswered row, the first answer sticks), `store::TABLES` += `secret_requests` (attached with `vault`/`vault_current` by the existing `replicate`, `announce_unstamped` included). `SecretsEvent::Requested` gains `title: Option<String>` and `fields: Vec<SecretRequestField>` (both `#[serde(default)]`); `name`/`note`/`current` are the first field. `Request { session, title, fields }`, `Outcome { stored: Vec<(String, u32)>, timed_out }`, `Answer { id, value, values } -> Result<bool, String>`, `SetEngine(String)` (tell), `ListRequests { unanswered_only } -> Result<Vec<SecretRequest>, String>`, `secrets::actor()`. The `Request` waiter (`wait`) re-reads its row every 1 s (`REQUEST_POLL`) and at once on a local `Answer`; it finishes on `answered_ms` (any engine), on every field's vault row advancing past its `current` (a peer stored them, `answered_ms` still in flight — the row is then closed locally as `stored`), or on timeout (row closed as `timeout` unless a peer answered first). `SecretsEvent::Resolved` is published as before (`stored` = first field's version). `tools.rs`: `secret_request` builds a one-field `Request` (label = note); new `secret_request_form { title, fields: [{ name, label, masked? }] }` (`Risk::ReadOnly`, `DIRECT`, `CONSUMES_SECRETS = false`) → `stored a, b, c` | `declined` | `no answer after N s`. 6 actor tests + 1 row test.
2. `51bc36e5 feat(roster)` — `migrations/0004_handoffs.sql` (`handoffs`, §2.7 verbatim), `src/handoff.rs`: `Handoff` (moved from `attention.rs`; `pub use crate::handoff::Handoff` there keeps `roster::attention::Handoff` for `rpc`), `insert`, `get`, `list(pool, unresolved_only)` newest first, `resolve(pool, id, outcome, now_ms) -> bool` (unresolved rows only), `DesktopHandoffTool { pool, shared, timeout }` (`desktop_handoff { reason, url?, timeout_secs? }`, `Risk::ReadOnly`, `DIRECT`, `CONSUMES_SECRETS = false`, `RETURNS = "dict"`): `engine` = `mesh::Status.self_id` (hostname without a mesh), `session` = `cx.session`, `agent` = `Shared::agent_of`; polls its row every 1 s; on timeout writes `outcome: timeout` itself; returns `"<outcome> after N s"` (`ok` for `done`, `err` otherwise) + `structured { outcome, elapsed_secs }`. `handoff::timeout(rt)` reads `sandbox.handoff_timeout_secs` by name (private `SandboxSection`, default 1800). `attention.rs`: `Inputs` += `handoffs`, `secrets: Vec<secrets::SecretRequest>`, `engines: HashMap<id, name>` (peers + self from `mesh::Status`); derive `handoff:<id>` (title `"<agent title | agent name | session[..8]> needs you: <reason>"`, `detail` = engine name else id, `session`, `agent`, `at_ms = created_ms`) and `secret:<id>` (title `"<session[..8]> asks for a secret: <title | field names>"`, `detail` = engine name, `session`, `at_ms = created_ms`); dismiss/snooze fall through the existing `retain`. New messages on the **`Attention` actor**: `ListHandoffs -> Result<Vec<Handoff>, String>` (unresolved, newest first), `ResolveHandoff { id, outcome } -> Result<(), String>` (`done | cancelled` only; `Err` for unknown/already resolved). `lib.rs`: `replica::attach(pool, ["agents", "budget", "handoffs"])`, tool registered on every engine after `tools::register`, `secrets::SetEngine` told right after `SetSelf`. `Cargo.toml`: `secrets = { workspace = true }` (no cycle: `secrets` → runtime, tools, replica, oauth, platform). 2 tests.
3. `fc3b1281 feat(rpc)` — `api.rs`: `handoffs_list` → `ListHandoffs`, `handoff_resolve { id, outcome }` → `ResolveHandoff`, both via `attention_actor()`; `answer_secret(id, value: Option<String>, values: Option<HashMap<String, String>>) -> bool` (trailing optional param; the two-arg call keeps working). `crates/tui`: compile fix only (below).

## `SecretsEvent::Requested` after this change (wire, `type: "SecretRequested"`)

```json
{ "type": "SecretRequested", "id": "<uuid>", "session": "<uuid>",
  "name": "<first field name>", "note": "<title, else first label>", "current": null | n,
  "title": null | "Log in to GitHub", "fields": [ { "name", "label", "masked": bool, "current": null | n } ] }
```

`title`/`fields` are `serde(default)`: an old client ignores them, an old producer omits them (`fields: []`). The pending snapshot (`PendingPrompts.secrets`) carries the same shape. `SecretResolved` is unchanged.

## `answer_secret` params

`answer_secret(id: RequestId, value: string | null, values?: Record<string, string> | null)`. `value` answers the (first) field; `values` answers a form by field name (names not on the card are ignored); both absent/`null` = decline. Works for a row that originated on another engine: the vault rows and `answered_ms`/`outcome` are written here and replicate back; the origin's waiter finishes the tool call. `true` = the row was unanswered until now; `false` = unknown id or already answered (timed out, declined or answered elsewhere).

## TS bindings (`cargo test -p rpc --features ts export_bindings`)

See gate table. Field names the UI worker needs: `Handoff { id, engine, session, agent, reason, url, created_ms, resolved_ms, outcome, hlc }`, `SecretRequest { id, engine, session, title, fields, created_ms, answered_ms, outcome, hlc }`, `SecretRequestField { name, label, masked, current }`, `SecretsEvent` `SecretRequested` gains `title: string | null`, `fields: Array<SecretRequestField>`. `AttentionItem` unchanged (`kind: "handoff" | "secret"`, `detail` = engine display name, `session` set on both, `agent` on hand-offs).

## Deviations (decisions; nobody to ask)

- **Handoff messages live on the `Attention` actor**, not a `HANDOFF_ACTOR` or `Roster`: it already holds the pool, derives the items and `rpc` already has `attention_actor()`. The tool talks to the pool directly (`Store` is `Clone`), no actor round-trip.
- **`handoffs.outcome` also takes `timeout`** (contract lists `done | cancelled` for `handoff_resolve`; the RPC refuses anything else). The tool closes its own row on timeout so the item leaves every inbox.
- **`secrets` learns its mesh id from the roster** (`SetEngine`), since `mesh` depends on `secrets` and `crates/mesh/**` is not mine. Rows written before the roster's `Status` round-trip carry the hostname (same fallback as `budget.engine`). Follow-up for C/I: one `tell(secrets::SetEngine(id))` where `mesh` starts would remove the roster's role.
- **`Outcome.stored` is `Vec<(String, u32)>`** (was `Option<u32>`); `Resolved.stored` (wire) stays the first field's version.
- **`Answer` reply is `Result<bool, String>`** (was `Option<SessionId>`): a replicated row has only a session *string*; `rpc` needed the bool anyway. A vault write failure is the `Err`.
- **Same value re-entered**: `Vault::set` writes nothing for an unchanged value, so the "vault rows advanced" shortcut does not fire; the `answered_ms` path still finishes the wait. `stored` then reports the existing version.
- **The `secret:<id>` title is by session short id** — `secret_requests` has no `agent` column (contract); the UI can resolve it from `session`.
- **Names with `/`** stay invalid (`vault::valid_name`, 37 §11); the design's `github.com/<user>/password` example is not storable — tool docs say so. Vault untouched, as briefed.
- `secret_request` on an engine whose vault failed to open: unchanged (no actor, no tool).

## Not built

UI (E); `sandboxes_list` engine→VM mapping (the item carries the engine id/name); relaying `SecretsEvent` across engines (by design); a hand-off `Requested`-style bus event (rows only; the HUD polls `attention_list` as today).

## Tests written

`secrets::tests::{single_request_round_trip_through_the_row, form_stores_every_field_in_order, decline_stores_nothing, a_peer_answer_arriving_as_rows_finishes_the_wait, vault_rows_alone_finish_the_wait, timeout_closes_the_row}`, `secrets::requests::tests::insert_list_answer`; `roster::handoff::tests::insert_list_resolve`, `roster::attention::tests::handoffs_and_secret_requests_become_items` (+ `derives_every_kind_and_hides_dismissed_or_snoozed` fixture uses `..Default::default()`). Not exercised live: a two-engine replication run (the peer paths are simulated by writing the rows the replica would apply).

## Gate

| command | result |
|---|---|
| `cargo check -p roster -p secrets -p rpc --tests` (+ `-p tui`) | ok (tui needed the compile fix below; 7m20s) |
| `cargo clippy -p roster -p secrets -- -D warnings` | ok |
| `cargo test -p secrets --lib` | 22 passed |
| `cargo test -p roster --lib attention` | 3 passed |
| `cargo test -p roster --lib handoff` | 2 passed |
| `cargo test -p rpc --features ts export_bindings` | ok, 8 export tests passed (11m30s) |

`cargo` is at `/opt/homebrew/opt/rustup/bin`. `cargo fmt` run on the touched crates; unrelated fmt churn in `crates/tui` reverted.

## Edits outside H's files (flag)

- `crates/tui/src/lib.rs` — `api.answer_secret(id, value, None)` (new trailing param). `crates/tui/src/model.rs` — `Requested { .. }` pattern + `title: None, fields: vec![]` in three test literals. Compile-required.
- `crates/rpc/bindings/{SecretsEvent.ts (title, fields), SecretRequestField.ts (new)}` — written by the export test; tracked files, committed with the docs. The desktop copies (`crates/desktop/ui/src/rpc/bindings/`) are gitignored — E regenerates with `just ts-desktop`.
- `crates/roster/Cargo.toml` — `secrets` dep; `crates/secrets/Cargo.toml` — `figment` dev-dep (tests build a `Runtime`). `Cargo.lock` follows.

## Follow-ups for the integrator

- E: `ui/src/rpc/api.ts` `answer_secret(id, value, values?)`; `SecretRequestCard` renders `fields` when `fields.length > 1` (single-field cards keep working from `name`/`note`); HUD `handoff` click → `handoff_resolve`; `secret` click → the masked card for the row (`secret:<id>` id = `RequestId`).
- C: consider `mesh` telling `secrets::SetEngine` at start (see deviations).
- I: merge order K → … → H keeps; `roster` now depends on `secrets` (plugin start order is unaffected — lookups are lazy).
