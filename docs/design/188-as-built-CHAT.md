# 188 as built: W-CHAT (chat Messaging… sheet, MCP rows on Agents › Tools, Serve over MCP)

Base `d02f4959`. Branch `feat/188-chat`.

## What each surface now holds

- **Chat ⋯ menu › Messaging…** (was "Communication & peers…", which opened Settings) opens the new side sheet
  `components/chat/MessagingSheet.tsx`. It holds:
  - **Status**: Effective on/off with the reason line in Settings' exact wording. **Open ancestor** appears when an ancestor
    opted out. When the node-wide switch is off, a link button **Open Network › Messaging** follows (`/settings/messaging`).
  - **Root scope**: on a loaded root, "Who may discover this tree" (select, `aria-label="Root scope"`) and Alias
    (`aria-label="Root alias"`, 64 chars, saved on blur or Enter when changed), with the same `set_peer_scope` call through
    `routed` and the same revision-guarded install of the reply. On any other chat it shows the inherited scope badge, the
    alias and **Open parent**.
  - **Peer requests**: `<Peers session focus={peer} />`, unchanged.
  - Not in the sheet: the chat's own on/off (the menu checkbox stays its only home) and the node-wide switch.
- **Agents › Tools**: each MCP server row now has its **home chip** (`on 333` / `home not claimed yet`) beside the name and
  its **home actions** (Run everywhere, Only on <home>, Move home here, Turn off / on here; `set_mcp_home`) in the controls
  line. Its state text now uses the mesh's machine names. It also shows the server's `reason` line and its login state
  ("logged in" plus **Log in** / **Log out**). The `homes.yaml` problem line sits above the list.
- **Settings › Tools › Serve over MCP** (`Mcp.tsx`): only "MCP server" (switch, port, token, Copy MCP config) and "Recent
  calls". The Tool servers section is gone.

## Moves and removals

- Settings › Communication's Effective/reason, Open ancestor, Root scope (select, alias, inherited view) and Peer requests
  are re-created in `MessagingSheet.tsx`. `messaging-sheet.ts` holds the pure parts: `effectiveLine`, `scopeView`,
  `messagingSearch`, `withoutMessaging` and the `useMessagingSheet` store. Nothing imports `Communication.tsx`. W-NET deletes
  the originals.
- `Mcp.tsx` › Tool servers moved to `Tools.tsx` rows. `lib/mcp-home.ts` moved (git mv) to `components/library/mcp-home.ts`,
  since its only user is now Tools. Its two pure tests moved from `Mcp.test.ts` to `library/mcp-home.test.ts`. The two
  pane-level DOM tests (no chip before the mesh answers; a button sets its own row's home) moved to `library/Tools.test.ts`,
  rewritten against the Tools rows.
- `SessionMenu` and `CommunicationNotice` no longer import `settings` from `slash.ts`.

## Opening the sheet

- Store: `useMessagingSheet` = `{ session, peer, show(session, peer?), hide() }`. It is mounted in `Header.tsx` beside the
  other per-session sheets, so it outlives the menu.
- `/c/$session?messaging=1[&peer=<id>]`: `SessionRoute` calls `show(session, peer)` and then
  `navigate({ search: withoutMessaging, replace: true })`, so a reload does not reopen the sheet. This is the target of W-SHELL's
  legacy `/settings/communication?session=…&peer=…` redirect.
- `CommunicationNotice`'s button opens the sheet when there is a chat, labelled **Messaging…**. With no chat (`session === null`)
  it still reads **Settings** and goes to `/settings/messaging`.
- `MessagePeer`'s "Open in Communication" is now **Open in Messaging**, a `<Link to="/c/$session" search={{ messaging: '1', peer: origin.id }}>`.

## Decisions

1. **The store is keyed by chat** (`session`, not a bare `open`). With split panes every pane's Header mounts a sheet, and a
   bare flag would open all of them.
2. **The sheet closes when the page path changes or its Header unmounts** (one effect cleanup keyed on `pathname`).
   Otherwise Open parent / Open ancestor / a peer's Open / the Settings link would leave the store pointing at the chat, and
   the sheet would reopen when you came back. This replaces explicit `hide()` calls at each navigation.
3. **`peer` counts only alongside `messaging=1`.** A lone `?peer=` is dropped by `validateSearch`. C0 kept it separately.
4. **The MessagePeer link uses the URL, not the store.** Peer cards also render in transcripts with no chat Header (notebook
   Ask view, chat column, note chat), and only the URL works from all of them. Cost: on the chat page itself, one extra
   history entry (`/c/x` pushed, then its params replaced away).
5. **Alias is a controlled input** (`AliasInput`, keyed on the saved alias) instead of Settings' `defaultValue`. The
   `disclosure.test.ts` ratchet bans `defaultValue` under `components/chat/` outside its CHROME allow-list, which I don't own.
   Behaviour is unchanged: trimmed, saved on blur or Enter when it differs, reset by a new snapshot.
6. **Section titles in the sheet**: "Status", "Root scope", then Peers' own "Peers" and "Peer requests". The sheet title is
   "Messaging".
7. **MCP login follows Connections' Accounts flow.** Locally `window.open(url, '_blank')`. Over link (`useEngines.active !== LOCAL`)
   the row shows "Sign in from the node (<name>): <url>" until the server is logged in. Before, Tools called the Tauri
   opener, which fails on web and phone. The buttons now read **Log in** / **Log out** (were Login / Logout) and are disabled
   while either call is pending, as in Connections. "logged in" shows beside Log out; "login required" stays in the status.
   The whole MCP login flow now lives in `components/library/Tools.tsx`, since Settings has no MCP login UI left after
   W-AGENT's change. `useMcpActions().login` and `.logout` hold the calls, and `ServerRow` holds the over-link URL as state.
   **There is no `LoginFinished` listener on the Tools page, on purpose.** The engine never publishes `LoginFinished` for an
   MCP login: only the provider `Login` handler does (`agent/src/supervisor.rs`). `McpLogin` (`tools/src/mcp.rs`) ends in
   `Restart` on success or `LoginFailed` on failure, and both publish `McpProgress`. `lib/rpc.ts` turns that into a
   refetch of the `['mcp']` queries. The row reacts to the new `auth`: the shown URL disappears once the server stops
   being `required`, and a failed login shows up in the server's state. A listener here would only toast provider logins
   that happen to finish while Tools is open. For the same reason, logout invalidates the MCP list keys but not
   `queryKeys.models`, because `list_models` does not depend on MCP auth.
8. **The `set_mcp_home` action invalidates both `mcp(session)` and `mcp(null)`**, the keys Tools' other mutations already use.
9. **C4 wording**: the Remove dialog's "Its entry is deleted from the config file, or disabled when the file holds other
   servers" now reads "Its entry is deleted, or disabled when it is defined alongside other servers". The two
   `mcp/config.yaml` mentions left in `Tools.tsx` are code comments.
10. `Peers.tsx` needed no change: Radix unmounts the sheet's content when it closes, so each open gets a fresh `focus` state.

## Deviations from the brief

- The brief says to move the home helpers "out of `Mcp.tsx`". They already lived in `lib/mcp-home.ts`, so that file moved to
  the suggested `components/library/mcp-home.ts`.
- The brief describes the store as `{ open, peer, show(peer?), hide() }`. It is keyed by chat instead (decision 1).

## Hand-offs

- **W-SHELL**: keep my versions of `SessionMenu.tsx`'s Messaging… item and `CommunicationNotice.tsx`'s button. Neither calls
  `settings()` any more. Redirect legacy `/settings/communication?session=<id>[&peer=<id>]` to `/c/<id>?messaging=1[&peer=<id>]`.
- **W-NET**: Communication.tsx's per-chat parts can go. Nothing of mine imports them. `Peers` is now used only by the sheet.
- **W-AGENT**: Connections › Accounts' MCP rows can go. Login and logout for MCP servers now live on the Tools rows.
- **W-LEARN**: `categories/tools.tsx` still imports `Mcp` from `components/settings/Mcp`. The export name is unchanged.
