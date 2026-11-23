# 210 as built — FIX4-CLIENT (#1, #3, #6, #10, #11, #12, #15, #16, #17)

Branch `feat/210-public-share`. Commits `4b9d1e704` (tools), `c2db6d197` (share + one desktop comment), plus this doc.
Owned: `crates/share/**`, `crates/tools/src/approval.rs`, `crates/desktop/ui/src/lib/describe.ts`.

## What was built

| # | fix | test |
|---|---|---|
| 1 BLOCKER | New local table `share_approvals (share, approval_id, binding, approved_ms)` (migration `0004`, **not** in `store::REPLICATED`). Written only by `Engine::publish_admitted` (same transaction step as the share row; removed on a failed store and on `stop`). `binding` is `(engine, host, path, source, access)` as JSON, so a replicated *edit* of an approved row (other port, password→public) does not match either. `Engine::may_mount` = `store.enabled()` ∧ `store.locally_approved(share)`; required in `restore()` (start + every welcome), in `after_cert` (pending → live) and in the welcome's Offline → Live. A row without it is left as stored, unmounted, removed from the route table if it was there, logged, and listed by `Engine::unmounted()`. | `engine::tests::replicated_rows_without_a_local_approval_stay_unmounted` |
| 3 | `stop_all_with`: after the local stop and fan-out, attaches first (`wait_attached`, bounded by `STOP_ALL_ATTACH_WAIT` = 10 s) when an edge client runs (`Engine.client`, set by `start`) or a link exists; then `send_stop_all`. On failure (and only for the originating engine, `fan_out`), hands Stop all to the lease holder: `family::holder()` → `share_stop {id: "all"}` over `theuth/rpc/1`; the holder runs `stop_all_forwarded` (attaches and posts on its own connection); its `edge_pending == None` clears our pending mark. TODO(210-M2) at the former engine.rs:881 removed. | `engine::tests::stop_all_attaches_first_when_detached` (holder hand-off: no test, needs a mesh) |
| 6 | `Consent` carries `asked_ms`: `Consent::ask` records it before raising the row; `Consent::of_call` (registry-raised row) takes it from the row's v7 `ApprovalId` timestamp. `Engine::still_approved` (switch on ∧ `stop_all_ms < asked_ms`) runs at the top of `publish` and again right before the first store write in `publish_admitted`. Stop all and switch-off call `tools::approval::dismiss_tool("share_publish")`, which dismisses every pending `share_publish` card (new `DismissTool` message). | `engine::tests::publish_refused_after_stop_all_or_switch_off`, `approval::tests::dismiss_tool_withdraws_only_that_tools_cards` |
| 10 | `Engine::start` runs `stop_at_start` before `restore`: every non-stopped share of this engine is stopped unless `created_by` is `Settings` **and** its `<name>.D` is a reserved, unreleased `share_names` row. `restore` stops a share whose `expires_ms` passed (`StoppedBy::Expiry`). | `engine::tests::start_stops_shares_except_reserved_settings_ones` |
| 11 | `routes::Table` keeps one `CancellationToken` per routed share (`stop_token`); a remount keeps it, `remove` cancels it. `Proxy::handle` wraps every routed response body in `until_stopped` (ends with an error → hyper resets the h2 stream / closes the h1 connection; size hint and end-of-stream preserved so framing is unchanged) and the upgrade copy `select!`s on the token, dropping both halves. | `routes::tests::stop_token_survives_remount_and_fires_on_remove` |
| 12 | RPC `share_publish`: after `publish_precheck`, `Engine::publish_preview(&plan)` picks the name (allocating a random one at the edge for a first publish) and computes the exact URL; the card args gain `url`, `name`, `label` (the generated label of a port source included). On Deny/timeout/dismiss, `release_preview` `DELETE`s a name it allocated. Allocated names are kept in `Engine.fresh` so the publish accepts them before a welcome lists them. `describe.ts` already rendered `args.url`; its TODO(210-M1) comment is replaced. | `engine::tests::preview_names_the_url_and_releases_a_fresh_name_on_refusal`; `describe.test.ts` (existing, passes) |
| 15 | `approval.rs`: the asker's reply sender lives in `Pending.asker` (`Arc<Mutex<Option<BoxReplySender>>>`); the waiter polls `poll_closed` through it. `resolve` checks `is_closed()` first: closed → the waiter is told `Dismissed`, an RPC row settles `timed_out` without an answer, `Resolve` returns `false`. | `approval::tests::resolve_after_the_asker_left_is_a_withdrawal` |
| 16 | `PortGuard::check` refuses unless the process-tree check answers `Some(false)` (fail closed on `None`). Other listeners: not registered — see deviations. | `sources::tests::unknown_owner_is_refused` |
| 17 | `sources::resolve`: containment and the dot rule run on the canonical path (after the `index.html` step) relative to the root, so `env -> .env` or `git -> .git/` inside the root is refused. | `sources::tests::link_to_a_dot_entry_inside_the_root_is_refused` |

## Public API changes

- `share::store::Store::{put_approval(&Share, u64), locally_approved(&Share), delete_approval(&str)}`; migration `0004_share_approvals.sql`.
- `share::engine::{STOP_ALL_ATTACH_WAIT, PublishPreview}`, `Engine::{publish_preview, release_preview, unmounted}`, `Consent::asked_ms`.
- `Consent` is now `Consent(ApprovedBy, u64)` (private fields, no external constructor change).
- `share::routes::Table::stop_token`; `share::proxy::until_stopped`.
- `share::sources::NoProcessCheck` now refuses every port (doc updated). Production wiring uses `ProcessTree`.
- `tools::approval::{DismissTool, dismiss_tool}`.
- RPC `share_publish`: approval card args gain `url`, `name`, `label`. No schema/type change on the wire (`wire_golden` unchanged).

## Deviations from the brief / design

- #10: §7.4 says "reserved-name shares started from Settings *without expiry*". `config::Expiry` has no "never", so every share has an expiry; the exempt class is "Settings + reserved name", and those keep their expiry (checked in `restore`). Stopped shares use `StoppedBy::User` (no `quit` variant: adding one changes the replicated `stopped_by` vocabulary that other surfaces parse).
- #11: the reset is per request (response body) and per upgrade, not per TLS connection: a connection may carry several shares of one name (`app.sky.D` and `docs.sky.D`), and its next request for a stopped share already gets `421` from the table lookup.
- #12: the tool path (`share_publish` called by the model) still gets its card from the registry before the engine runs (no pre-approval hook in `crates/tools/src/registry.rs`, not mine): that card has no URL. TODO(210-M4) kept in `describe.ts`.
- #16: other in-process listeners (a2a, mcp-server HTTP, oauth callback, sandbox bring-up, desktop) are not registered: those crates do not depend on `share`, and `ProcessTree` already refuses any listener owned by this PID or a descendant. Documented on `register_own_port`; the old TODO(210-M1) there is removed.
- #1 "reported": unmounted rows are logged and exposed as `Engine::unmounted()`; no RPC/UI surface for it yet (TODO(210-M3)).

## Not built

- TODO(210-M3): `Engine::unmounted()` on an RPC / Settings line ("a share on this machine was not approved here").
- TODO(210-M4): URL on the card for the tool path (needs a registry pre-approval hook).
- No test for the lease-holder hand-off of Stop all (needs a running mesh + replica transport).
- No end-to-end test that an open h2 stream / WebSocket is reset on stop; covered at the token level only.

## Doubts

- `stop_all` now waits up to 10 s for an attach when an edge client runs but is detached, then up to `FORWARD_TIMEOUT` (90 s) for the holder. A Stop all RPC can therefore take long while the edge is down; the local stop happened before both waits.
- `still_approved` uses `stop_all_ms >= asked_ms` with millisecond clocks: a Stop all in the same millisecond as the ask voids it (intended: fail closed).
- `asked_ms` for a registry-raised row comes from the v7 UUID's timestamp, i.e. when the card was raised, which is what we want; a clock jump between the card and Stop all could misorder them.
- `release_preview` does not run when the RPC future itself is dropped during the ask (the caller hung up); the random name stays allocated to the mesh until released by hand.
- Existing shares published before this change have no `share_approvals` row and will **not** remount after upgrade (they are stopped at start anyway unless Settings+reserved, and those stay unmounted until republished).

## Verification (each its own lock hold, after the last code edit)

```
cargo test -p share                                    # 55 passed (lib), 0 failed
cargo test -p tools --lib -- --skip git::              # 301 passed, 3 ignored
cargo clippy -p share -p tools --all-targets --no-deps -- -D warnings   # clean
cargo test -p rpc-compose --test wire_golden           # 2 passed
cd crates/desktop/ui && bun run check                  # tsc -b --noEmit: clean
cd crates/desktop/ui && bunx vitest run src/lib/describe   # 6 passed
```

Cargo.lock: not changed by this work.
