# 210 as built — FIX3-CLIENT (E2E1 findings 4, 6, 7, 8, 9)

Branch `feat/210-public-share`. Commits `9eabc07c6` (share), `ba322a755` (tools), plus this doc.
Owned: `crates/share/**`, `crates/tools/src/approval.rs`.

## What was built

| # | finding | fix |
|---|---|---|
| 1 | release right after Stop all fails with `edge transport: closed by peer: stop_all` | Two causes, both fixed. (a) `edge::Client::run` drops the session future when nothing needs the edge (`need` false after Stop all); `session` used to call `conn.close()` + `Attachment::lost()` only on a normal return, so the engine kept a `Link` on the dead connection and `attached == true`. A drop guard (`edge::Lost`) now closes and reports on every way out. (b) `EdgeConn::is_closed` (iroh: `close_reason().is_some()`); `Engine::wait_attached` drops a closed link first (`live_link`) and waits for the redial; `Engine::with_control` retries a call once after `EdgeError::Transport`, dropping and closing the stale link (`drop_link`) so the state machine redials. Used by `names::{reserve, release, transfer}` and `Engine::check_name`. |
| 2 | `X-Forwarded-For: ::ffff:a.b.c.d` | `visitor::read_preamble` canonicalises an IPv6 source (`to_canonical`), and `proxy::request_headers` canonicalises again before writing `X-Forwarded-For`. The engine sends no `Forwarded` header (stripped) and no PROXY preamble upstream (it is an HTTP proxy), so nothing else carries the address. |
| 3 | an approval whose RPC caller gave up stays pending; a late Allow does nothing | `Approval::request` keeps the raw reply sender (`ReplySender::boxed`) and its waiter `select!`s on `closed()`: when the asker's future is dropped, the internal `Gone(id)` removes the entry and `ApprovalResolved {decision: deny, timed_out: false, dismissed: true}` is published, so every surface drops the card. A later `Resolve` returns `false`. Journal: a row without `tool_call_id` (RPC) settles `timed_out` with no answer; a tool call's row is left `pending` for its 109 resume (same rule as `ForgetSession`). Applies to every `ask` (human-only or not); `tell` senders have no reply sender and are unchanged. |
| 4 | terms checked after the owner approved | `Engine::publish_precheck(want)` runs in `share_publish` (RPC) before `Consent::ask`: switch off, no edge, edge unreachable (attaches, up to `ATTACH_WAIT`), and — when the publish would allocate a fresh name (no `name` asked, no held name in the welcome) — terms accepted at the edge's current `info.terms_rev`. The refusal is the same text `pick_name` used (`accept the terms of <domain> first (<terms_url>, revision <rev>): …`), which `public-shares.ts` already parses. |
| 5 | cold `share_publish` blocks 60 s and answers `pending` | The 60 s was the settle rule of the first name-key write (`REKEY_SETTLE_SECS = 60` in `holder::ensure_name_key` via `FamilyNameKeys::create`), not the edge (its CSR post answers 202 at once). `publish_admitted` now runs `ensure_cert` on its own task and waits `PUBLISH_CERT_WAIT` (1.5 s); still running → the share is stored `pending` and the answer returns; the task goes on (`finish_cert_later`): an installed chain calls `after_cert`, which takes the name's pending shares live and emits `share.changed`; a queued CSR waits for the edge's `cert` event as before. |

## Public API changes

- `share::engine::Published` gains `pending: Option<PendingCert>` (`#[serde(default, skip_serializing_if = "Option::is_none")]`): `None` when live; `Some({queue_pos, eta_s})` from the edge's 202; `Some({null, null})` when the certificate step was still running at the deadline.
- New `share::engine::PendingCert { queue_pos: Option<u32>, eta_s: Option<u64> }` (JsonSchema, ts).
- New `share::engine::PUBLISH_CERT_WAIT`, `Engine::publish_precheck(Option<&str>)`.
- `share::edge::EdgeConn::is_closed()` (default `false`), `Control::{is_closed, close}`.
- `share_publish` tool: structured output adds `state` and `pending`; the text names queue position / ETA when known.
- `tools::approval`: no type change. Private `Gone` message.

## Deviations from the brief

- Finding 3 asked for "ApprovalResolved with a 'gone' reason". `ApprovalResolved` is a struct literal in tui, slack, telegram, signal, double and sessionsync tests; adding a field breaks crates I do not own. The withdrawal is reported as `dismissed: true, timed_out: false` (what Attention-dismiss already sends; every surface removes the card on any `ApprovalResolved`). TODO(210-M4): a `reason` field once the owners of those crates can take the literal change.
- Finding 4 is fixed for the `share_publish` RPC only. For the `share_publish` *tool* the registry raises the human-only row before the tool body runs and `Tool` has no pre-approval hook; `crates/tools/src/registry.rs`/`lib.rs` are not mine. TODO(210-M4): a `Tool::precheck` (or equivalent) called by the registry before `RequestHumanOnly`, implemented by `SharePublishTool` with `publish_precheck`.

## Not built

- TODO(210-M3): a background certificate step that fails (vault, edge refusal) only logs; the share stays `pending` until a `cert` event or the next attach retries. Before this change the error failed the publish call.
- TODO(210-M4): desktop bindings (`crates/desktop/ui/src/rpc/bindings/method.share.output.ts`) are generated from the schema and do not show `Published.pending` yet; CLIENT-M4 owns `crates/desktop`. The field is optional on the wire, so the current UI is unaffected.

## Doubts

- `with_control` retries non-GET calls (`DELETE` name, `PUT` claim, `POST` transfer) once after a transport error. If the first request reached the edge and only the answer was lost, the retry gets the edge's answer for a repeated call (e.g. `not_owner` for a release that already happened). Not retried: `allocate` (POST of a random name), `csr`, `stop_all`, routes.
- `publish_precheck` attaches (up to 30 s) before the card appears when the edge is not attached yet. The alternative (card first) is what E2E1 complained about.
- Withdrawing on a dropped `ask` is now true for ordinary tool approvals too: a cancelled turn's card disappears instead of lingering until its timeout. That is the intent, but it changes behaviour beyond share.

## Verification (each under its own lock hold, after the last code edit)

```
cargo test -p share                                    # 47 passed (lib), 0 failed
cargo test -p share --lib -- redial precheck cold_publish mapped header_policy attaches_sends   # 6 passed
cargo test -p tools --lib -- --skip git::              # 299 passed, 3 ignored, incl. approval::tests::abandoned_ask_withdraws_the_card
cargo clippy -p share -p tools --all-targets --no-deps -- -D warnings   # clean
cargo test -p rpc-compose --test wire_golden           # 2 passed
```

New tests: `edge::tests::attaches_sends_routes_and_redials_after_drain` (extended: `lost()` after going idle),
`engine::tests::control_calls_redial_a_closed_connection`, `engine::tests::publish_precheck_refuses_unaccepted_terms_before_the_ask`,
`engine::tests::cold_publish_answers_pending_promptly_and_goes_live_later`, `visitor::tests::preamble_v4_mapped_visitor_is_ipv4`,
`proxy::tests::header_policy` (mapped and plain v6 cases), `approval::tests::abandoned_ask_withdraws_the_card`.

Cargo.lock: not changed by this work.
