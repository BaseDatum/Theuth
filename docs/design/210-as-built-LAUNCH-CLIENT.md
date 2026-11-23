# 210 as built: LAUNCH-CLIENT (visitor head deadline, URL on the tool-path card)

Branch `feat/210-public-share`. Owned: `crates/share/**`, the approval-raising path in `crates/tools`
(`lib.rs` trait hook, `registry.rs` free `invoke`), `crates/desktop/ui/src/lib/describe.ts`.

## 1. Visitor head deadline and stream cap (`crates/share/src/visitor.rs`)

- New `visitor::serve_http(io, handler)`; `visitor::serve` now uses it after the PROXY preamble and TLS.
  It counts requests in flight on the connection: a request counts from the moment its head reached the
  service until its response body is dropped (finished, failed, reset). The guard lives in the response body
  (`map_frame` closure), so no extra allocation.
- Whenever the count is 0 for `HEADER_READ_TIMEOUT` (10 s, unchanged constant, now for both protocols) the
  connection gets `graceful_shutdown` (h2 `GOAWAY`, h1 no keep-alive). If it is still idle `CLOSE_GRACE` (2 s)
  later it is dropped, so a peer that ignores `GOAWAY`/`PING` cannot hold it. Covered: an h2 connection
  that opens no stream, a dribbled `HEADERS` + `CONTINUATION` head, an idle keep-alive. A long response or
  upload is in flight and never cut.
- `http2().max_concurrent_streams(MAX_STREAMS)` with `MAX_STREAMS = 100` (hyper's own default was not
  pinned before). h1 keeps hyper's `header_read_timeout`.
- The `TODO(210-M2)` at the former visitor.rs:164 is removed.

Public API: `visitor::{serve_http, MAX_STREAMS, CLOSE_GRACE}`.

## 2. URL on the `share_publish` tool-path approval card

### Generic hook (`crates/tools`)
- `tools::PreviewCx { session, call_id }`.
- `Tool::APPROVAL_PREVIEW: bool = false` and
  `async fn Tool::approval_preview(&self, args: Self::Args, cx: PreviewCx) -> anyhow::Result<Option<serde_json::Map<String, Value>>>`
  (default `Ok(None)`). Erased form `ErasedTool::approval_preview(&self, &Value, PreviewCx) -> BoxFuture<…>`;
  the blanket impl deserialises the args only when `APPROVAL_PREVIEW` is set (args that do not parse: none).
- Registry (`registry.rs`, free `invoke`, runs in the dispatch task, not in the actor): before every approval
  ask (`approval` is `Some`), awaits the preview once. `Ok(Some(map))` is merged key by key into the card's
  `RequestApproval.args`; `call` still gets the model's own args. `Err(e)` returns `"<tool>: <e>"` and raises
  no card.

### `share_publish` (`crates/share/src/tools.rs`)
- `SharePublishTool::new(engine)` (the `engine` field is now private; `lib.rs` registration updated).
- `approval_preview`: `plan` (validation, random label), `publish_precheck`, `publish_preview` (held name or
  local draw + read-only `GET /v1/names/{name}`, no claim). Card gets `url`, `name`, `label` (same keys as the
  RPC path). The `(Plan, PublishPreview)` is kept in `shown`, keyed by `(session, call_id)`.
- `call`: `Consent::of_call()` first (unchanged), then takes the `shown` entry for its `(session, call_id)`
  and runs `Engine::publish_previewed(plan, CallOrigin::Call{…}, consent, preview)`: the drawn name is claimed
  exactly, `NAME_TAKEN_MEANWHILE` if it was taken meanwhile, a held name is pinned. No entry, or one older
  than `PREVIEW_TTL` (30 min): refused (`…the approval card for this call has expired; publish again`), nothing
  claimed. The call's args are ignored in favour of the previewed plan (same call, same args).
- Deny / timeout / withdrawal: `call` never runs; the preview claimed nothing; the entry is purged on the next
  preview once older than `PREVIEW_TTL`.
- Validation and precheck errors (switch off, no edge, terms) now refuse a tool call before the card instead
  of after `Allow`.

Public API: `share::tools::{PREVIEW_TTL, SharePublishTool::new}`.

### Surfaces
- Desktop: `SharePublishApproval` already renders `args.url` through `describeSharePublish`; the
  `TODO(210-M4)` comment in `describe.ts` is replaced.
- TUI (`model/transcript.rs::describe`): `key: value` per arg, so the card shows `url: https://…`,
  `name: …`, `label: …`. No change needed.
- Slack, Telegram, Signal (`approvals.rs` `body`/`message`): pretty JSON of the args (≤ 600 chars), which now
  contains `"url": "https://…"`. No change needed; those crates were not touched.

## Deviations
- The hook runs before **every** registry ask, not only human-only ones (only `share_publish` sets
  `APPROVAL_PREVIEW`, so no other tool pays anything).
- Keyed by `(session, call_id)` rather than the approval id: the approval id is minted inside the `Approval`
  actor after the preview, while `(session, call_id)` is known to both the preview and `ToolCx`.
- The hook returns `anyhow::Result<Option<…>>` (not `Option`), so a preview can refuse before the card.

## Not built
- `ExternalApproval` (`registry.rs`, human-only asks raised for a non-registry caller, `call_id: None`) has no
  preview hook; no tool on that path needs one today. TODO(210-M4) if one does.
- Chat surfaces do not render a dedicated "URL:" line; the URL is inside their JSON block.
- No end-to-end test over TLS for the head deadline; tests drive `serve_http` over an in-memory duplex.

## Doubts
- An h2 connection with one request in flight while a second head is dribbled is not cut by the head
  deadline (the connection is busy). `CONTINUATION` blocks all other frames on that connection, so the cost
  is one connection, bounded by `MAX_HEADER_BYTES` and h2's continuation-frame cap.
- Idle visitor connections now close after 10 s + `GOAWAY`; browsers reconnect transparently, but a race
  with a request sent just as `GOAWAY` leaves is answered by the client's retry, not by us.
- `PREVIEW_TTL` (30 min) is longer than the default approval timeout (300 s); an operator setting
  `tools.approval_timeout_secs` above 30 min gets a refusal after a late `Allow` (fail closed).

## Verification
Commits: `09d24f5f1` (visitor), `27365ea89` (tools hook), `77aa857b5` (share_publish + describe.ts).
Each cargo command under its own `/tmp/theuth-build.lock` hold, `CARGO_BUILD_JOBS=6`, after the last code edit:

| command | result |
|---|---|
| `cargo test -p share` | 63 passed, 0 failed |
| `cargo test -p tools --lib -- --skip git::` | 302 passed, 3 ignored, 0 failed |
| `cargo clippy -p share -p tools --all-targets --no-deps -- -D warnings` | clean |
| `cargo test -p rpc-compose --test wire_golden --test wire_topics` | 2 + 4 passed |
| `crates/desktop/ui`: `bun run check` | clean |
| `crates/desktop/ui`: `bunx vitest run src/lib/describe` | 6 passed |

New tests:
- `share::visitor::tests::h2_connection_without_a_stream_is_dropped`: preface + `SETTINGS`, then silence;
  closed between 10 s and 13 s (paused clock).
- `share::visitor::tests::h2_dribbled_head_is_dropped`: `HEADERS` without `END_HEADERS`, then one
  `CONTINUATION` every 3 s; dropped in the same window, although bytes keep arriving.
- `share::visitor::tests::h2_in_flight_requests_are_kept_and_streams_are_capped`: a 30 s response completes;
  a 150-request burst peaks at exactly `MAX_STREAMS` handlers in flight, and all succeed; the connection closes
  ≥ 10 s after the last response.
- `tools::registry::tests::approval_preview_fills_the_card_and_binds_the_call`: for Allow and Deny, the
  preview runs once, the card args are `{source, url}`, Allow publishes the previewed value and `call` never
  sees `url`, Deny runs nothing; a failing preview raises no card.
- `share::engine::tests::tool_preview_binds_the_call_to_the_shown_name`: the preview makes one read-only check
  and no claim; a denied card claims nothing; the approved call claims exactly the shown name; `taken1` →
  `NAME_TAKEN_MEANWHILE` without a redraw; an expired or missing preview is refused with no claim, and no rows
  are stored.

Not run: tui, slack, telegram, signal (not touched). Cargo.lock: not changed (no new dependencies).
