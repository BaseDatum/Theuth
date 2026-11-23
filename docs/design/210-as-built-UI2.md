# 210 as built — UI2 (generated bindings, password once, edge-pending stop, native tray Stop all, approvals warning)

Design: `docs/design/210-public-share.md` rev 8. Seams: UI1, WIRE1, FIX-A as-built notes.

## Files

| path | what |
|---|---|
| `ui/src/components/public-share/wire.ts` | hand types removed; re-exports the generated `Share`, `ShareChanged`, `ShareStatus`, `EdgeState`, `ShareTraffic`, `Published`, `Stopped`, `ShareSettings`, `NameCheck`, `PublishArgs`, `Expiry`, `Source as ShareSource`; `shareApi()` (typed `Api`), `reserveName()` (raw, M2 method not on the engine), `useShareMethod()` |
| `ui/src/components/public-share/actions.ts` | typed calls; `stoppedText(id, Stopped)`, `applyStopped(id, Stopped)` (sets `edgePending` on Stop all, warns instead of "stopped"); `takePassword(id, known?)` |
| `ui/src/components/public-share/PasswordOnce.tsx` (new) | `useSharePassword(id, known?)`, `<PasswordOnce>` (password, copy, Done) |
| `ui/src/components/public-share/PublicShareHost.tsx` | subscribes `share.changed@1`, `share.traffic@1`, `share.status@1` via `onTopic`; one `share_list` per open connection (listeners first); 10 s poll and `POLL_MS` removed; takes the tray's result from `share-stopped-all` |
| `ui/src/components/public-share/PublicShareStrip.tsx` | with nothing active but `edgePending` set, the strip stays: "Stopped here · the edge stop is pending and will be retried (<why>)", Stop all retries |
| `ui/src/components/public-share/SharePublishCard.tsx` | password share: `share_password` once, shown with copy; stopped share + `edgePending` → pending line |
| `ui/src/components/settings/Sharing.tsx` | typed calls; `ApprovalsOffWarning`; publish form (`PublishForm`, `publishArgs`); edge-pending line in Live shares |
| `ui/src/components/settings/categories/sharing.ts` | tab `required: {methods: ['share_settings_get','share_list']}` (WIRE1 TODO closed) |
| `ui/src/store/public-shares.ts` | `edgePending`, `setEdgePending`; `setStatus` clears it on a transition into `attached`; `reset()` keeps it; `edgePendingLabel(why)` |
| `ui/src/lib/tauri.ts` | `onShareStopAll` → `onShareStoppedAll(cb(Stopped))` (event `share-stopped-all`) |
| `src/share.rs` (new) | native tray Stop all: `stop_all(app)`, `call_stop_all(ws_url)`, `outcome_text`, `STOPPED_EVENT`; 2 tests |
| `src/lib.rs`, `src/commands.rs` | `mod share`; tray `share_stop_all` → `crate::share::stop_all(app)`; `SHARE_STOP_ALL_EVENT` (`share-stop-all`) removed |
| tests | `PublicShareStrip.test.ts` (rewritten on the typed `Api`; + edge pending, tray result, password once), `settings/Sharing.test.ts` (new) |

## Public API

- TS: `shareApi()`, `reserveName(name)`, `applyStopped(id, r)`, `stoppedText(id, r)`, `takePassword(id, known?)`,
  `useSharePassword(id, known?)`, `PasswordOnce`, `ApprovalsOffWarning({sharing, approvals})`,
  `publishArgs(source, expiry, password)`, `edgePendingLabel(why)`, store `edgePending`/`setEdgePending`.
- Tauri: event `share-stopped-all` with `{stopped, edge_pending}` (replaces `share-stop-all`). Command `share_tray` unchanged.
- Rust (crate-private): `share::stop_all(&AppHandle)`, `share::call_stop_all(&str) -> anyhow::Result<Stopped>`.

## Item by item

1. **Bindings.** `just ts-desktop` regenerated (`share_password`, `Stopped.edge_pending` now present). All share
   calls go through the typed `Api`; events through `onTopic` with the generated payload types. No polling.
2. **Password once.** Chat card: when the call's share is `access: 'password'` and not stopped, `share_password {id}`
   once; Settings publish form: `share_password {id}` after `share_publish`, falling back to the RPC
   `Published.password`. "Once" = per share id: the ids (never the password) go in a module set and
   `localStorage['theuth.public-share.revealed']` (last 200). The password lives only in the showing component's
   state; nothing logs it. A non-owner's call fails → nothing shown.
3. **edge_pending.** `applyStopped` stores it (Stop all only); strip, chat card (stopped share) and Settings › Live
   shares say "Stopped here · the edge stop is pending and will be retried (<why>)"; the toast is a warning, not
   success. Cleared by the next Stop all without `edge_pending` or when `share.status` goes into `attached`
   (the engine re-sends the stop at that welcome, FIX-A M7).
4. **Tray Stop all without a webview.** The shell has no WebSocket of its own but already makes plain-HTTP JSON-RPC
   calls to the daemon (`models.rs` `decision_loaded`): jsonrpsee serves HTTP on the same port, `?token=` from the
   connection URL authorises, and a token holder is the owner (`rpc_core::owner` refuses only guest peers). The tray
   item now runs `share::stop_all`: `Launch::peek()` (cached connection or a probe; never spawns), POST
   `{"method":"share_stop","params":["all"]}`, then a native notification ("Public sharing stopped" / "…edge stop
   pending" / "…failed") and `share-stopped-all` to the webview (ignored if none). Independent of the main window
   (closing it only hides it; a wedged or absent webview no longer matters). Tests: `share::tests::*` against a
   one-shot HTTP fake: request line carries the token, body is `share_stop ["all"]`, `edge_pending` parsed, RPC error
   surfaced.
5. **Approvals warning.** `ApprovalsOffWarning` under the switch, from `useConnection().info.approvals.global`
   (what Settings › Approvals reads): shown iff sharing is enabled and approvals is `false` (not while unknown).
6. **Approval card URL.** `crates/share/src/tools.rs` after FIX-A: approval args are still the tool arguments
   (`Plan` is built "before the name and edge are known"); no URL. Card text unchanged.

## Deviations

- **Settings publish form added** (it did not exist): source (port number or folder), expiry, password checkbox,
  Publish. The call blocks until the owner answers the human-only row the engine raises (button: "Waiting for
  approval…"). No label/name/path fields.
- Native path replaces the webview path entirely (not both), so one click is one `share_stop("all")`.
- "Once" across webview reloads relies on `localStorage`; without it only within the page.

## Not built

- `TODO(210-M2)`: Extend, names list, reserve (`share_names_reserve` still raw/discovery-gated), per-name certificate state.
- `TODO(210-M1, crates share)`: computed URL in the `share_publish` approval args (item 6).
- Native Stop all targets the local daemon only (`Launch::peek`); a desktop pointed at a remote engine is not a case
  the shell supports today.

## Doubts

- `edgePending` clears on any transition into `attached`; if the edge refuses again at that welcome the UI no longer
  says so (the engine has no event for "stop-all sent"/"still pending").
- Notification permission: the native notification is best-effort (logged on failure); the stop itself does not depend on it.
- `takePassword` marks the id before the RPC answers; a transient RPC failure loses the one showing.

## Verification

- `just ts-desktop` (under lock): ok, 635 methods, 71 topics.
- `bunx vitest run src/store/public-shares.test.ts src/components/public-share src/components/settings/Sharing.test.ts src/components/settings/registry.test.ts src/lib/describe.test.ts` → 5 files, 26 tests passed.
- `bun run check` → 15 errors, none in UI2 files: all `human_only` missing in `ApprovalRequested` literals of test
  files (`chat/ApprovalCard.test.ts`, `chat/rows.test.ts`, `sidebar/SessionItem.test.ts`, `lib/owner.test.ts`,
  `store/approvals.test.ts`, `store/reduce.test.ts`) — from CONSENT1's wire field, now in the regenerated bindings.
- `bunx vitest run src/components/settings` → 29 files pass; `Contacts.test.ts` fails to load (`localStorage`
  undefined in the test env), not touched here.
- `cargo clippy -p desktop --all-targets --no-deps -- -D warnings` (under lock): clean.
- `cargo test -p desktop` (under lock): 115 passed, 0 failed, incl. `share::tests::stop_all_posts_share_stop_all_with_the_token_and_reads_edge_pending`, `share::tests::stop_all_reports_an_rpc_error_and_a_clean_stop`.
- `Cargo.lock`: no change from UI2 (`reqwest`, `tauri-plugin-notification`, `tokio` were already desktop deps).
