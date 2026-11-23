# 210 as built — CLIENT-M4 (indicators and one-press stop on TUI, Slack, Telegram, Signal; desktop mesh id, typed names, island LIVE badge)

Design: `docs/design/210-public-share.md` rev 8 (§7.4 items 2, 6, 8, 9; D12; G4). Seams: CLIENT1, WIRE1, UI1, UI2,
UI-M2, CLIENT-M2 as-built notes. `crates/share` and `share-proto` were not changed.

## TUI (`crates/tui`)

| path | what |
|---|---|
| `src/components/share.rs` (new) | component `share`: status segment, `/share`, 4 tests |
| `src/components/mod.rs`, `registry.rs` | module + registration (t1 block, after `files`) |

- **Segment**: `Surface::Status(StatusSlot::AfterCore)`, red (`ui::CARNELIAN`), text `  ● LIVE N · tun.zip` while any
  share is active (`live | offline | pending`, the desktop's convention). Domain = the last `share.status` domain,
  else the first active share's `domain`. Hidden at 0.
- **Feed**: interests `share.changed@1` (`ShareChanged`) and `share.status@1` (`ShareStatus`); seeded by `share_list`
  on `Mount`/`Resync`, cleared and re-seeded on `EngineChanged`. A quiet refresh that fails (engine without sharing,
  `NOT_RUNNING`, older engine) shows nothing. Stopped records leave the map.
- **`/share`** (`[list|stop [id|all]]`): `/share` and `/share list` print each active share (`url (state) · expires
  in 3h05m · id`); `/share stop` and `/share stop all` → `share_stop("all")`; `/share stop <id>` → `share_stop(id)`.
  Reply `public sharing: stopped N shares[; the edge stop is pending and will be retried (<why>)]`, then a quiet
  `share_list`. **Never asks**: no confirmation, no focus claim; the engine's `share_stop` is ungated.
- Public: `components::share::{Share, Live, Request, Msg, parse, list_text, stopped_text}`.

Deviation: the TUI does **not** depend on the `share` crate. `tests/dependency_limit.rs` only lets the TUI's
workspace dependencies shrink, so the wire records are read through a local serde mirror (`Live {id, url, domain,
state, expires_ms}`) and the RPC answers are mapped field by field inside the effect (`ShareApiClient` is already
re-exported by `rpc-compose`). The design's text `● public · 2 live` became the brief's `● LIVE N · tun.zip`.

## Desktop (`crates/desktop`)

| path | what |
|---|---|
| `ui/src/components/public-share/wire.ts` | `namesApi.{list, check, reserve, release, primary, rotate, transfer}` on the generated typed `Api`; generated `HeldName`, `NameCheck`, `NameReleasedInfo` re-exported; hand types, `NAME_METHODS`, `checkName` and the `TODO(210-M2, CLIENT-M2)` removed; `NotOnEngine` kept (older/remote engines) |
| `ui/src/components/settings/Sharing.tsx` | `MeshIdRow({meshId})` at the end of "Your name": "Your mesh id — give this to the other mesh to receive a transfer", monospace id, copy button (toast "Mesh id copied"); hidden while `mesh_id` is null |
| `ui/src/components/public-share/LiveBadge.tsx` (new) | `liveBadge(active, mainVisible, meeting)` (pure decision), `useLiveBadge(meeting)`, `<LiveBadge count>` (`● LIVE N · Stop`) |
| `ui/src/routes/island.tsx` | precedence: voice panel, then the LIVE badge, then the meeting `Island` |
| `ui/src/lib/tauri.ts` | `shareStopAll()` (command `share_stop_all`), `mainWindowVisible()` |
| `src/commands.rs`, `src/lib.rs` | Tauri command `share_stop_all` → existing native `crate::share::stop_all(&app)` (same path as the tray) |
| tests | `settings/Sharing.test.ts` (+3), `public-share/LiveBadge.test.ts` (new, 2) |

- **Island badge (§7.4 item 6, built in ~107 lines)**: runs in the island webview (never hidden, only transparent;
  own connection), keeps its own share list (`share_list` per open connection, then `share.changed@1`), and only while
  something is active polls the main window's visibility every 2 s (`MAIN_POLL_MS`). Shows when active > 0, the main
  window is not visible and no meeting is on the island; sized 300×44 like the meeting island. Stop = the native
  Stop all (notification + `share-stopped-all` to the main webview).
- `just ts-desktop` was not needed: the bindings already carried `share_names_*`, `HeldName`, `NameReleasedInfo`,
  `NameCheck.reclaim_after_ms`, `ShareSettings.mesh_id`.

Gaps: during a meeting the meeting island shows no LIVE segment (`TODO(210-M4)`: a segment in `Island.tsx`); while
the macOS companion owns the island nothing is drawn; a minimised main window may still report `isVisible` true
(unverified) so the badge may not appear then; `share_stop_all` command has no Rust test (a one-line call into the
tested `stop_all`; Tauri's test runtime is not a dev-dependency).

## Slack (`crates/slack`, commit `50f72077d`)

| path | what |
|---|---|
| `Cargo.toml` | `share = { workspace = true }` (workspace crate; no new package) |
| `src/public_share.rs` (new) | `share_status_text(shares, now_ms) -> Option<String>`, `share_list_text`, `status_blocks`, `ended_blocks`, `is_active`, `now_ms`; `LiveMessage {observe, active, next(now_ms, holding) -> Option<LiveOp::{Post, Update, End}>, posted}`; `parse(args) -> ShareCmd::{List, StopAll, Stop, Usage}`, `pick`, `USAGE`, `STOPPED_TEXT`, `MOVED_TEXT` |
| `src/channel.rs` | `ShareChanged` bus subscription; seed + 60 s tick (`engine().list()` once the engine runs; `NOT_RUNNING` ignored); `sync_live_share` (the only Slack writer of the message); share-click replies posted beside the message |
| `src/commands.rs`, `src/ui.rs`, `src/lib.rs` | `Cmd::Share`, help entry, owner gate, buttons, `share_cmd` |

- **Status message** in the owner's DM: `● Sharing publicly · 2 live`, one line per share `https://app.sky.tun.zip/ —
  expires 14:02 (in 2h)` (`pending`/`offline` marked; other days `Wed 25 Nov 12:00 (in 2d)`), hint
  `/theuth share stop stops all`, a Stop button per share (≤ 4) and a red Stop all. Posted on the first active
  share, `chat.update` on every `share.changed`; at zero edited to `Public sharing stopped`, buttons removed, forgotten.
- **Only the lease holder posts it** (every machine sees every replicated share; otherwise one copy per machine). On a
  lease move the old message reads `Public sharing status moved to the hub machine` and the new holder posts afresh.
- **Commands** (owner-only, existing refusal for others; never asks): `/theuth share [list]`, `/theuth share stop
  [all]` → `engine.stop_all()` (`stopped N`, `edge stop pending: …`), `/theuth share stop <id>` →
  `engine.stop(id, StoppedBy::User)` (id, unique id prefix/suffix, host or URL).

Deviations: the command is `/theuth share …`, not `/share`: the Slack app registers only `/theuth`
(`manifest.yaml`); a real `/share` needs a manifest change and an app reinstall. Not pinned:
`TODO(210-M4)` in `public_share.rs` — `api.rs` has no `pins.add` and the manifest lacks `pins:write`; edit in place
only. `share.status`/`share.traffic` unused; no per-machine counts. Test fix outside the feature:
`threads::tests::agent_binding_persists` (flaky on same-millisecond timestamps) gained one `th.save()`.

## Telegram and Signal (`crates/telegram`, `crates/signal`, commit `63e288b9a`)

| path | what |
|---|---|
| `telegram/src/sharing.rs` (new) | `share_status_text`, `stop_all_text`, `keyboard`; `LiveMessage`, `Step`; `load_message`/`save_message` (`<data_dir>/telegram/share_message.json`); `command` |
| `telegram/src/channel.rs` | `ShareChanged` subscription, seed from `engine.list()`, `share_sync`; Stop all press replies with a separate message |
| `telegram/src/api.rs` | `Sender::pin` (pin/unpin, `disable_notification`) |
| `telegram/src/{commands,ui,lib}.rs`, `Cargo.toml` | `share` in `COMMANDS` + `OWNER_ONLY`, help panel, `share = { workspace = true }` |
| `signal/src/sharing.rs` (new) | `share_status_text`, `Notices`, `command` |
| `signal/src/{channel,commands,lib}.rs`, `Cargo.toml` | subscription, seed, `share_notice`; `share` owner-only command |

- **Telegram**: one message in the owner's private chat on the first active share, **pinned** silently; lines
  `https://x.tun.zip/ — expires 14:00 (in 3h)` (pending/offline marked), `/share stop` hint, **Stop all** button.
  `editMessageText` on every `share.changed`; at zero `Public sharing stopped`, unpinned, forgotten. Its location is
  persisted so a restart edits/ends the same message instead of leaving a stale pin.
- **Signal**: no pin, no reliable edit → a message to the owner on transitions: the list when the first share goes
  active, `Public sharing stopped` when none remain; a failed send retries on the next change.
- **Commands** (both; owner-only, standard refusal; never asks): `/share`, `/share list`, `/share stop [all]` →
  `engine.stop_all()` (count + edge-pending note), `/share stop <id>` (id, unique prefix, URL or host) →
  `engine.stop(id, StoppedBy::User)`. Engine not started → `sharing is not running on this engine`.
- **Only the lease holder posts** (same reason as Slack). On a lease move the new holder posts its own message; on
  Telegram the old one stays (unlike Slack, which edits it to "moved").

Gaps: `TODO(210-M4)` in `signal/src/sharing.rs` — text and `/share` logic duplicated from Telegram's; they belong in
`chat-common` (with Slack's `public_share.rs`). No timed refresh on Telegram/Signal: the "(in 3h)" countdown moves
only when a share changes (the absolute time stays right); Slack ticks every 60 s.

## Not built

- `TODO(210-M4)`: Slack pin (`pins.add` + `pins:write` scope, manifest change), a real Slack `/share` slash command;
  shared channel text in `chat-common`; LIVE segment inside the meeting island during a meeting.
- §7.4 items 5 (attention items, crate `roster`) and 7 (notification Stop actions) — not in this phase's crates.
- Per-machine breakdown and visitor counts in TUI/channels (`share.traffic` unused there).

## Gaps in `crates/share` (none blocking)

No RPC or event was missing. The channels call the in-process `share::rpc_support::engine()`; the TUI uses the
`share_list`/`share_stop` RPC and `share.changed@1`/`share.status@1`.

## Doubts

- Channel status messages are posted only by the engine holding the channel's hub lease (the existing `holding`
  flag the channels already use for hub duties); a family whose holder has no Slack/Telegram configured gets no
  status message even if another machine has the channel. The commands still work everywhere.
- TUI segment counts `pending`/`offline` as LIVE (desktop convention); the brief says "while any share is live".
- Island badge on a minimised main window: `isVisible` behaviour unverified.

## Cargo.lock

Modified, not committed: `share` added to the dependency lists of `slack`, `telegram`, `signal`; no new registry
package. The TUI and desktop add no dependency.

## Outside-the-feature fix

`crates/tui/src/components/mcp.rs`: `Request::SetHidden.draft` is now `Option<Box<agent::CreateSessionOptions>>`
(clippy `large_enum_variant`, pre-existing: `CreateSessionOptions` grew in `agent`); one test guard in
`model/tests.rs` adapted. No behaviour change.

## Verification (after the last edit; each cargo command under its own `/tmp/theuth-build.lock` hold, `CARGO_BUILD_JOBS=6`)

| command | result |
|---|---|
| `cargo test -p tui -p slack -p telegram -p signal` | ok: tui 254 (incl. 4 `components::share::tests`), slack 175, telegram 144, signal 67, tui `dependency_limit` 1, `no_emoji` 1; 0 failed |
| `cargo clippy -p tui -p slack -p telegram -p signal -p desktop --all-targets --no-deps -- -D warnings` | clean (first run failed on the pre-existing `mcp.rs` lint and one `share.rs` test lint; both fixed, rerun clean) |
| `cargo test -p desktop` | 115 passed, 0 failed (ran before the last TUI-only edits; desktop untouched since) |
| `crates/desktop/ui`: `bun run check` | 0 errors |
| `crates/desktop/ui`: `bunx vitest run src/components/settings/Sharing.test.ts src/components/public-share src/store/public-shares.test.ts` | 4 files, 26 tests passed |

Not run: an end-to-end publish (no edge), a live Slack/Telegram/Signal account, the island badge on a real macOS
window (only its decision function and render are tested).
