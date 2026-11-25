# 216 — As built: B4 UI-B (placements)

Builder B4 of design 216 (contract `docs/design/216-contract.md` v1, §2.3 "B4", §12.2, §12.4, §12.5, §11.4).
`UI` = `crates/desktop/ui/src`.

## Files

### New

| File | What |
|---|---|
| `UI/lib/plugins.ts` | One zustand store per engine (`pluginStore(st)`, a `WeakMap<Stores, …>`; `usePlugins(select)` reads the enclosing engine's). Pure `reduce(state, PluginUi)` for the five `plugins.ui` variants and `seed(state, epoch, plugins, surfaces)`; the five `on("plugins.ui@1", …)` listeners are registered at module load and write the active engine's store (the feed reaches only the active engine's listeners). `usePluginsFeed` (internal, mounted by every placement hook) loads `plugin_list` + `plugin_surfaces` once per epoch: on first use, after `PluginsChanged` (or a `PluginState` for an unknown plugin), after a dropped connection reopens; one load at a time per engine. Exports `usePluginPlacements(kind)`, `usePluginCommands()`, `usePluginSurface(plugin, placement, session)` (contract §12.2) plus `usePluginPlacement`, `usePluginInvalidated`, `setCommandCard`, the pure `placementsOf`/`commandsOf`, and the entry helpers `entryDot`, `overflowDot`, `entryTip`, `badgeText` shared by the sidebar and the status strip. |
| `UI/lib/plugins.test.ts` | the store applies all five variants; seed; ordering, hidden and unapproved/disabled filtering; hidden commands still listed |
| `UI/components/plugins/provider.ts` | `pluginProvider` exactly as contract §12.4 (types: `AppRendererProps['provider']`, since `@theuth/apps/runtime-types` has no tsconfig path) |
| `UI/components/plugins/provider.test.ts` | mutation + `action_id` → `plugin_action`; query on load and on press → `plugin_query`; mutation without `action_id` and unknown statement rejected, nothing called |
| `UI/components/plugins/Attribution.tsx` | the host strip: `PluginIcon` + `plugin · <name>` |
| `UI/components/plugins/PluginSurface.tsx` | `PluginSurface({ plugin, placement, session, content, icon?, className?, actions? })`: strip above and outside `[data-plugin-content]`; `openui` → `AppRenderer` + `pluginProvider` (generation bumps only when the source text changes; `PluginInvalidate` → `runtime.refresh()`; `onAction`: `open_url` http(s) only, `CopyToClipboard`, `continue_conversation` → `openDraftSession` + the draft text, navigated, never sent); `json-render` → `JsonRenderFence`; `markdown` → `Streamdown` with `skipHtml`, the chat's `rehypePlugins` minus `raw` and `markdownComponents` and no fence renderers (a plugin's markdown cannot smuggle an unvalidated OpenUI fence); `loading`/no content → host spinner frame; `error` → the message; no source → `text`. Also `PluginCommandCard({ session })`: the ephemeral command reply card. |
| `UI/components/plugins/PluginSurface.test.ts` | attribution outside the content node; markdown `<input>`/`<form>`/`<b>` not rendered; json-render renders; loading/error/text frames |
| `UI/components/plugins/PlacementHost.tsx` | Popover (declared size, else 360 × 280; `max-h/max-w` from Radix's available size, no viewport units) or, under `useIsMobile()`, a bottom `Sheet` (`max-h-[85%]`). `anchor` mode (PopoverAnchor) for the overflow menu's popover. Content mounts only while open, so opening fetches it. |
| `UI/components/plugins/SidebarSlot.tsx` | `SidebarSlot({ variant: 'rail' \| 'row' })`, gated on `plugin_list`/`plugin_surfaces`/`plugin_surface`; cap 5 (`SIDEBAR_CAP`) + lucide `Ellipsis` `DropdownMenu` whose button carries the worst `warn`/`error` dot of what it holds; dot + Inbox-style badge per icon; tooltip `label · status label · tooltip`; click toggles the popover, or for `open: tile` opens `openPluginTile(focused chat leaf, its tile placement)` when `tileable(focused)`, else the popover shows that tile placement's content; context menu → Hide (`plugin_set_hidden(name, id, true)`). |
| `UI/components/plugins/SidebarSlot.test.ts` | 6 plugins → 5 icons + overflow, hidden absent, overflow lit `warn`; ≤ 5 → no overflow; fine overflow unlit |
| `UI/components/plugins/StatusStrip.tsx` | `StatusStrip({ variant?: 'bar' \| 'sheet' })`: `bar` = icon + dot, tooltip label + tooltip, cap 3 (`STATUS_CAP`) + overflow; nothing on a phone. `sheet` = the phone's sidebar footer, every item with its label; nothing off a phone. |
| `UI/components/plugins/StatusStrip.test.ts` | cap 3 + lit overflow; nothing on mobile; sheet variant on mobile only |
| `UI/components/plugins/test-fixtures.ts` | test-only `pluginInfo`/`placement`/`snap` factories (a file outside §2.3's list; test support only) |
| `UI/components/tiles/kinds/plugin.tsx` | `pluginTile`: `TileFrame` (title/tab = placement label, meta `plugin · <name>`, Reload, pop-out) around `PluginSurface` |

### Edited

| File | Edit |
|---|---|
| `UI/components/tiles/registry.tsx` | `TILE_KINDS.plugin`, `TILE_ICONS.plugin` (`Puzzle`) |
| `UI/components/tiles/open.ts` | `PluginTileProps`, `openPluginTile(host, p)` keyed `<plugin>/<placement>` |
| `UI/components/sidebar/AppSidebar.tsx` | rail: `<SidebarSlot variant="rail"/>` after `<RailSections/>` (it draws its own separator); expanded: `<SidebarSlot variant="row"/>` between `</SidebarMenu>` (Desks) and `<SessionList/>`; footer: `<StatusStrip variant="sheet"/>` after `<Checklist/>` |
| `UI/components/status/StatusBar.tsx` | `<StatusStrip/>` in the `right` slot only; `dotOf` untouched |
| `UI/components/chat/slash.ts` | `slashMenu(word, harness, plugins = [])` appends plugin commands after built-ins and harness commands, `source: 'plugin · <name>'`, a name already listed stays the built-in's/harness's; `isCommand(line, skills, plugins = [])`; `run()`'s `default:` first looks the word up among the active engine's plugin commands → `api.plugin_command(plugin, command.name, rest, session)`; a `ui` reply → `setCommandCard`, a text-only reply → `notify`. `COMMANDS`/`dictationCommands` untouched. |
| `UI/components/chat/Composer.tsx` | `usePluginCommands()` passed to both `slashMenu` calls and to `isCommand`; `<PluginCommandCard session={session}/>` above the pill row |
| `UI/components/chat/slash.test.ts` | new tests: plugin rows after built-ins with `plugin · turns`, harness wins a clash, `COMMANDS` unchanged; `isCommand('/turns:reset …')`; `run` → `plugin_command`, card / note |
| `UI/lib/popout.test.ts` | one case: `{tile:'plugin', props:{plugin:'turns', placement:'turns'}}` round-trips |

`UI/components/sidebar/Rail.tsx` was not edited: `RAIL_ITEM` was already exported; the plugin popovers are click-only, so `RailPopover`/`useRailPopover` are not needed.

## Deviations and decisions

1. **Which plugins draw.** `usePluginPlacements` and `usePluginCommands` include only plugins with `enabled` and a state other than `disabled`, `invalid`, `awaiting_approval` (a crashed or exited plugin still shows its icon with the host-overridden dot). Contract §12.2 only says "hidden removed"; drawing an unapproved plugin's icons would put unapproved UI on screen.
2. **Status strip shows `status` placements only**; a `sidebar` placement with `status: true` shows its dot on its sidebar icon (the TUI, which has no sidebar, folds both into its status line).
3. **Command card.** The contract says "an attributed ephemeral `PluginSurface` card" without a home: it is `CommandCard` in the plugins store (`{ plugin, slash, session, ui }`) shown by `PluginCommandCard` above the composer of the session that ran the command; Close clears it; a new reply replaces it. The card's content is the `CommandUi` mapped to a `SurfaceContent` (no bindings: OpenUI is refused in v1 replies).
4. **`PluginSurface` props** gained optional `icon`, `className`, `actions` beside the contract's four.
5. **Surfaces are fetched with `session: null`** everywhere (every v1 placement is global, design §9.1); `plugin_command` passes the composer's session.
6. **Feed reload triggers:** `PluginState` for a plugin the list does not hold bumps the epoch (a reload) instead of being dropped.
7. `test-fixtures.ts` is an extra file under `components/plugins/` (test support).

## Hand-offs

- **B1 / Z:** `src/store/reduce.test.ts(1693)` fails `bun run check` (TS2322: a `MessageDelivered` literal without the new `contact` field, from B1's §5.7 `#[serde(default)] contact: bool`). Not a B4 file; the fixture needs `contact: false`.
- B3's `useInvalidateOn` lines (`['plugins']` on `PluginState`/`PluginsChanged`) do not overlap: B4's store does not use React Query.
- Found while testing: the chat's `rehypePlugins` include streamdown's `raw` (rehype-raw), so `skipHtml` alone does not drop raw HTML (an `<input>` came out as a sanitised checkbox). `PluginSurface` filters `defaultRehypePlugins.raw` out of the list; the chat renderer is unchanged.

## Gates

Run in `crates/desktop/ui`, each under `/tmp/theuth-build.lock`:

- `just ts-desktop` (B4 ran it once at 04:15, bindings were absent): rc 0, `method.plugins.*`, `event.plugins_2eui.*`, `event.plugins_2eevents.*` generated.
- `bun run check`: the only error is `src/store/reduce.test.ts(1693)` (not B4's, see Hand-offs); no error in any B4 file.
- `bunx vitest run src/lib/plugins.test.ts src/components/plugins/ src/components/chat/slash.test.ts src/lib/popout.test.ts src/lib/no-emoji.test.ts src/components/disclosure.test.ts src/components/settings/registry.test.ts src/components/sidebar/AppSidebar.test.ts src/components/status/StatusBar.test.ts src/components/chat/Composer.test.ts`: first run 4 failures in B4's new tests (missing `TooltipProvider` in the SidebarSlot test; the raw-HTML finding above); after the fixes `bunx vitest run src/components/plugins/` 5 files / 14 tests green; the other 9 files were green in the first run (138 passed overall).

## Not verified

- No live engine: the store, popovers, sheet, tile open and command path are covered by unit/DOM tests with a seeded store and fake API only; real `plugins.ui` traffic and `plugin_surface` loading → `PluginSurface` refetch were not exercised end to end.
- OpenUI rendering of a plugin surface (provider wiring through `AppRenderer`, `PluginInvalidate` → `refresh`, identical-source no-remount) is not covered by a test; the provider routing itself is.
- Visual check in the running app was not done (no engine with a plugin in this worktree).
