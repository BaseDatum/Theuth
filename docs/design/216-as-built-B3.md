# 216 — As built: B3 UI-A (Settings › Plugins, install card)

Builder B3 of design 216 (contract `docs/design/216-contract.md` v1, §2.3 B3, §12.2, §12.6). `UI` = `crates/desktop/ui/src`.

## Files

| File | What |
|---|---|
| `UI/lib/rpc.ts` | `queryKeys.plugins = ['plugins']`, `queryKeys.plugin(name) = ['plugins', name]`, `queryKeys.pluginLogs(name) = ['plugins', 'logs', name]`; `useInvalidateOn` invalidates `['plugins']` on `plugins.ui@1` `PluginState` and `PluginsChanged` |
| `UI/components/settings/categories/plugins.tsx` *new* | the 13th category `plugins` (lucide `Puzzle`), one tab `plugins`, `required` exactly §12.6 |
| `UI/components/settings/registry.ts` | `plugins` last in `CATEGORIES`, `'plugins'` last in `TAB_IDS` |
| `UI/components/settings/registry.test.ts` | thirteen categories ending in `plugins`; new test `plugins → [['plugins','Plugins']]` + its required methods/topic |
| `UI/components/settings/Plugins.tsx` *new* | `Section` "Plugins" (Refresh, New plugin); one `Collapsible` row per plugin in `plugin_list` order (the `McpServersTab` row look): chevron, placement icon, name, version, an "approval waiting" badge when a card waits on a running plugin, state chip (`PluginDot` + `STATE_LABEL`), enable `Switch`, Uninstall (`AlertDialog` with a Keep its data `Switch`); second line Restart, Logs (`LogsSheet`), Open folder (`openPath(dir)`, only under `isDesktop()`), up/down (`plugin_set_order` with the two names swapped); description and `error` under the name. Expanded (fetches `plugin_get` only while open): the waiting card (`CARD_WORDS[kind]`, unverified notice, `GrantLines(card.lines)`, changed paths, Approve with the card's hashes verbatim), "Granted" (`grantedLines(granted)`, read-only, "To grant less, edit its plugin.yaml"), per-placement show/hide `Switch` (`plugin_set_hidden`), tools with risk, folder path, warnings, stderr tail. A row with `needs_approval` starts open; `?focus=<name>` opens and scrolls to it. Every mutation invalidates `['plugins']`; failures through `useAction` → `notify.error` |
| `UI/components/settings/Plugins.test.ts` *new* | rows/version/state chip; Approve only on the waiting card, `plugin_approve(name, card.manifest_sha256, card.code_sha256)`; Uninstall through the `AlertDialog` (keep-data on → `plugin_uninstall('turns', true)`); placement switch → `plugin_set_hidden`; enable and reorder; New plugin → `plugin_new('demo', 'sh', true)` and Raw off for go; Open folder absent unless `isDesktop()`; `moved` |
| `UI/components/plugins/GrantLines.tsx` *new* | `GrantLines({ lines })`: one `li` per line (`data-grant`, `data-new`), a "new" chip on widen lines, the risk word right (`read-only` grey, `mutating` amber, `destructive` red); `RISK_WORD`; `grantedLines(g: Grants): GrantLine[]` (run, secrets, subscribe, emits, placements, tools, call, input, notify) for the Settings "Granted" list |
| `UI/components/plugins/GrantLines.test.ts` *new* | one row per line, new marked, risk shown; `grantedLines` order and risks |
| `UI/components/plugins/NewPluginPopover.tsx` *new* | name (client-side `^[a-z][a-z0-9-]{0,31}$` hint; the host still checks reserved names), `ToggleGroup` py/js/sh/go/rb/rs, Raw `Switch` enabled for py/sh only → `plugin_new(name, lang, raw)`; on success `notify.success("Created <name>", { description: next })` (the note pointing at the card) |
| `UI/components/plugins/LogsSheet.tsx` *new* | right `Sheet`: `plugin_logs(name, 200)` while open, path, lines in a `pre`, Refresh |
| `UI/components/attention/PluginInstallScreen.tsx` *new* | `plugin:<name>:…`: Source, "plugin · <name>" attribution, title, `CARD_LEAD[kind]`, unverified notice, `GrantLines(card.lines)`, changed files, folder, version; a card whose id is not the item's says so (replaced / nothing waits). `plugin-notify:…`: attribution, title, body. Bar: `KindActions` |
| `UI/components/attention/PluginInstallScreen.test.ts` *new* | card lines + changed paths + folder + unverified; Approve → `plugin_approve('turns', M, C)`; replaced card keeps Approve off; notice has no Approve and fetches nothing; `pluginRef`/`dismissHint` |
| `UI/components/attention/actions.tsx` | `PluginActions`: Approve (only when `plugin_get(name).card.id === item.id`; exactly its hashes; failure on the item via `setItemError`; success invalidates the inbox and `['plugins']`) · Review (→ `/settings/plugins?focus=<name>`); notice → "Open plugin" only. `SnoozeDismiss`'s tooltip goes through `dismissHint` |
| `UI/components/attention/labels.ts` | `PLUGIN_DISMISS_HINT` "Dismiss: the plugin stays unapproved until it changes", `dismissHint(item, leaves)`, `pluginRef(item)` (`plugin:` / `plugin-notify:` → name, notify); `plugin` stays out of `LEFT_KINDS` |
| `UI/components/attention/Screen.tsx` | `SCREEN.plugin = PluginInstallScreen` |
| `UI/components/attention/Card.tsx` | `needsPane()` true for `plugin` (the card shows Open · Snooze · Dismiss) |
| `UI/components/attention/model.ts`, `UI/components/icons/channels.tsx` | unchanged from B0 (`'plugin'` in `KIND_ORDER`, `ICONS.plugin`) — nothing more was needed |

## Deviations from the contract

1. **Notice "Open" goes to Settings › Plugins (`?focus=<name>`), not to the placement.** `AttentionItem` carries no placement id for a `plugin-notify:` item (§10.1 puts the body in `detail`, nothing else), so the screen cannot know which placement `open` named. Hand-off: if B1 wants Open to reach the placement, the item needs the placement id (e.g. in `packet` or a new field) and the button can call B4's `openPluginTile`.
2. Approve on the inbox (pane and card) is disabled until `plugin_get(name).card.id` equals the item id, so a stale item can never approve a newer card's hashes. (The host's `Stale` check would refuse it anyway; this keeps the button honest.)
3. `queryKeys.plugin(name)` is `['plugins', name]` (prefix `plugins`), so a plugin named `logs` shares a prefix with `pluginLogs`: only an extra refetch, harmless.

## Hand-offs

- B1 (or whoever owns `UI/store/reduce.test.ts`): `bun run check` fails only at `src/store/reduce.test.ts:1693`, a `MessageDelivered` literal without the new `contact` field (contract §5.7). Not a B3 file; add `contact: false` there.
- Optional: the placement id on notify items (deviation 1).

## Gates

Bindings: B3's own `just ts-desktop` attempts failed twice on an in-flight `crates/tui` compile error; the bindings used are the ones regenerated by another builder (~04:35, `method.plugins.*`, `event.plugins_2e{ui,events}.*` present).

- `bun run check` (tsc): no error in any B3 file; one error elsewhere (hand-off above).
- `bunx vitest run` `settings/Plugins.test.ts`, `plugins/GrantLines.test.ts`, `attention/PluginInstallScreen.test.ts`, `settings/registry.test.ts`, `settings/route-target.test.ts`, `lib/no-emoji.test.ts`, `components/disclosure.test.ts`, `attention/{Card,model,Bundle}.test.ts`, `sidebar/AppSidebar.test.ts`, `status/StatusBar.test.ts`, `lib/popout.test.ts`, `chat/Composer.test.ts`: all pass (15 files).

## Not verified

- No run against a live engine (B1/B2 behaviour); the tests use fake engines.
- Visual check in the app was not done (no engine with the plugins host running in this worktree).
