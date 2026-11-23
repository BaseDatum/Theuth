# 210 as built — UI1 (desktop/web indicators, one-press stop; §7.3, §7.4, §7.6, D12, G4)

Design: `docs/design/210-public-share.md` rev 8. Shapes from `210-as-built-CLIENT1.md` (`crates/share/src/model.rs`,
`rpc_share.rs`). Owner-only card per `210-as-built-CONSENT1.md`.

## Files

| path | what |
|---|---|
| `ui/src/components/public-share/wire.ts` | hand mirror of `Share`, `ShareChanged`, `ShareTraffic`, `ShareStatus`/`EdgeState`, `PublishArgs`, `ShareSettings`, `Published`, `NameCheck`; `shareCall(method, …params)` over the raw `RpcClient.call`; `useShareMethod` (discovery); `onShareTopic` (only when `TOPICS` routes the topic) |
| `ui/src/store/public-shares.ts` | zustand store `usePublicShares` {shares, traffic, status, loaded, terms}; pure `summarize`, `stripLabel`, `activeShares`, `timeLeft`, `stateLabel`, `sourceLabel`, `parseTermsError`, `newlyLive`, `firstVisitors`, `trayBadge` |
| `ui/src/components/public-share/actions.ts` | `loadShares()`, `stopShare(id)`, `stopAllShares()` (never gated; reloads `share_list` after) |
| `ui/src/components/public-share/PublicShareHost.tsx` | mounted once in the shell: seeds by `share_list` on each open connection, follows `share.*@1` or polls (`POLL_MS` 10 s) while the bindings lack them; tells the tray the active count; tray Stop → `share_stop("all")`; `notifyIfHidden` "Live on the public internet" / "First visitor on <host>" |
| `ui/src/components/public-share/PublicShareStrip.tsx` | red strip (`bg-danger`, VmDesktop's `ring-danger` colour) at the top of the routed column: "Sharing publicly · N live · M pending · V visitors · Manage · Stop all" |
| `ui/src/components/public-share/SharePublishCard.tsx` | chat card for `share_publish`: URL (open, copy), source, access, state, time left (30 s tick), visitors + last visit, Stop; a terms refusal renders the accept step |
| `ui/src/components/public-share/SharePublishApproval.tsx` | approval card layout for `share_publish` (§7.3) |
| `ui/src/lib/describe.ts` | `describeSharePublish(args, now)` |
| `ui/src/components/chat/{ApprovalCard,MessageTool,ToolRunCard}.tsx` | `share_publish` routed to the two cards; deliverable (never folded) |
| `ui/src/components/settings/Sharing.tsx`, `categories/sharing.ts`, `registry.ts` | Settings › Sharing as its own category after Network (tab id `sharing`) |
| `ui/src/routes/__root.tsx` | mounts `PublicShareHost` and the strip in `Root` |
| `ui/src/App.tsx` | tray title/tooltip via `trayBadge` (one writer): `◉ LIVE N` before the attention `●n` |
| `ui/src/lib/tauri.ts` | `shareTray(live)`, `onShareStopAll(cb)` |
| `crates/desktop/src/lib.rs`, `commands.rs` | `Flags.sharing`, `set_sharing`, "Stop public sharing (N)" first in `tray_menu` while N > 0, menu id `share_stop_all` → event `share-stop-all`; command `share_tray(live)` |
| tests | `ui/src/store/public-shares.test.ts`, `ui/src/components/public-share/PublicShareStrip.test.ts`; `registry.test.ts` updated (twelve categories) |

## Public API (UI)

- `usePublicShares` store; `summarize(shares, traffic) → {live, pending, visitors|null}`; `stripLabel(sum) → string|null`.
- `stopShare(id)`, `stopAllShares()`, `loadShares()`.
- Tauri: command `share_tray { live: u32 }`; event `share-stop-all` (no payload).

## Deviations

- **Bindings not generated yet** (WIRE1): RPC goes through `useConnection.getState().client.call`; events only reach
  listeners once `share.*@1` is in `TOPICS` (`rpc/topics.ts` drops unknown topics), so until then the host polls
  `share_list` every 10 s. `TODO(210-M1, WIRE1)` in `wire.ts`, `PublicShareHost.tsx`, `categories/sharing.ts`
  (add `required`). Traffic and status stay empty until then.
- **"Live" for the strip/tray** = `live | offline | pending` (pending goes live by itself; offline comes back). The
  label separates "N live" and "M pending"; with only pending it reads "Going public".
- **Settings › Sharing is a category**, not a Network tab: Network already has the 6-tab maximum the registry test
  holds. The registry contract test now lists twelve categories.
- **Tray**: the title changes (`◉ LIVE N`), not the icon (no red-dot asset; an icon swap also needs
  `core:tray:allow-set-icon` in `capabilities/`, which I do not own). The title writer stays the webview's badge;
  Rust only owns the menu item. Windows: tooltip "sharing publicly (N)".
- **Notifications** use the existing `notifyIfHidden` (only when the window is hidden/unfocused), without a Stop
  action (`tauri-plugin-notification` actions not wired). The meeting-island "LIVE · Stop" (§7.4 item 6) is not built.
- **Approval card URL**: the approval's `args` are the tool's arguments; they carry no computed URL. The card shows
  `args.url` when present, else "The edge assigns the address on publish (label/name/path)".
  `TODO(210-M1, crates share + tools)`: put the computed URL into the approval args (§7.3 "exact URL").
- **Default expiry shown as "8 hours"** (CLIENT1 config default); no RPC to read/set it (`TODO(210-M2)`).

## Not built

- `TODO(210-M2)`: Extend button (no `share_extend`), Reserve is wired to `share_names_reserve` but disabled until
  discovery lists it; names list with Change/Release/Transfer/Rotate; per-name certificate state and queue position;
  "Can't renew sharing: <hub> unreachable"; rekey states; the §9.1 PSL sentence (needs PSL state from the edge).
- Strip in popout windows (no host there), iOS-specific placement, meeting island LIVE, attention items (roster).
- Machine names in the strip ("1 on this Mac, 1 on Studio"): `Share.engine` is an endpoint id; mapping to a label
  not done.
- Terms: Settings shows the accept step only after a refused publish was seen by this webview (no RPC reads the edge's
  terms URL/revision).

## Doubts

- An old transcript card with a terms refusal re-raises the terms ask in Settings on mount even after acceptance.
- Polling + `reset()` on every reconnect: the first `share_list` after a reconnect is treated as the baseline, so a share
  that went live during the gap gives no notification.
- `share_settings_set(true)` from the switch: CLIENT1 does not yet route it through the owner-only approval row
  (`TODO(210-M1, crate tools)` there); the UI calls it directly.

## Verification

```
cd crates/desktop/ui && bun install --frozen-lockfile   # and packages/apps (jsdom for DOM tests)
bunx vitest run src/store/public-shares.test.ts src/components/public-share src/components/settings/registry.test.ts src/lib/describe.test.ts
  → 4 files, 21 tests passed
bun run check
  → fails with 21 errors, all pre-existing and outside UI1's files (element-form bindings missing:
    ElementFormCard.tsx, element-form*.ts, web-view-agent.ts, store/sessions.ts:979-980); none in files touched here
cargo clippy -p desktop --all-targets   (under /tmp/theuth-build.lock)
  → see report
```

No `Cargo.lock` change from UI1 (no new crates or npm packages).
