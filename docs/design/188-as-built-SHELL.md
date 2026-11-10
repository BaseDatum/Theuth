# 188 as built: SHELL (page, route, entry points)

Branch `feat/188-shell`, one commit on `d02f4959`. Design: `188-settings-full-page.md`; brief: `188-SHELL.md`.

## What the shell is now

- `/settings/$tab` renders `components/settings/SettingsPage.tsx` in the main pane; the app sidebar stays and its Settings button is lit (`sectionOf` → `'settings'`). No Dialog, Sheet, backdrop, Esc or close button.
- **Desktop and iPad** (`useIsMobile()` false): a 208 px `<nav aria-label="Settings categories">` with a 52 px "Settings" label head (drag region; clears the traffic lights when the rail is closed), one row per shown category with `aria-current="page"` on the current one, each opening its category's first shown tab. Right of it: the section band (`TabBand`) with the category's shown tabs, then a scrolling body (`max-w-[720px]`, `px-4 md:px-6`, keyed by tab so a new tab starts at the top) holding the restart banner and the tab's `Component` with `focus` from `?focus=`.
- **Phone**: `/settings` (`routes/settings.index.tsx`) is the category list (icon, label, chevron); on a wider window it redirects (replace) to `/settings/appearance`. A category page is the band with icon-only tabs (`aria-label` = label), a 44 px `SectionRow` with a back chevron to `/settings` and the category label, then the body. A deep link opens the category page directly.
- **Restart banner** (`components/settings/page/RestartBanner.tsx`): "1 change applies after theuth restarts: Double." / "2 changes apply after theuth restarts: Double, Decision models." The **Restart theuth** button shows only in Tauri, on the local engine, when the shell started the daemon (`connection().owned`); elsewhere the banner adds "Restart theuth where it runs to apply them."
- **Route** (`routes/settings.$tab.tsx`) is a thin wrapper over the pure `components/settings/route-target.ts`: moved pages (`skills`, `tools`, `secrets`, `schedules`) → `communication?session=` → chat `/c/$session?messaging=1[&peer=]` → `MOVED_TABS` → unknown → `appearance` → hidden tab → its category's first shown tab, else `appearance`.
- **Registry helpers** (ids and order untouched): `TAB_IDS` + `SettingsTabId`, `isShown`, `shownTabs`, `shownCategories`, `shownTab`.

## Removals and moves

- `SettingsDialog.tsx` deleted, along with everything only it used (the flat tab list, the Sheet/Dialog shells, the route's `onClose`/history-back logic and its empty drag strip).
- `slash.ts`: the `SettingsTab` union of old ids is gone; `settings(navigate, tab: SettingsTabId, focus?)` lost its `session` parameter.
- `SectionBand.tsx`: the band body moved into `TabBand` (any tabs) and the sidebar toggle + search into `SidebarLead`; `SectionBand({ group })` now feeds `TabBand` from `GROUPS`. Every existing caller is unchanged.

## Decisions

1. **Phone order: band, then the back row** (brief S3 lists row, then band). House rule 97b (binding) puts the band at the top and a view's own row below it, every other detail page does that (`RoutineDetail`, `AskView`), and the band carries the phone's only sidebar toggle, which would otherwise sit below the back row.
2. **Restart = `relaunch()` then `location.reload()`**, not `reset()` + invalidate. `Launch::relaunch` binds the new daemon to a fresh random port (`launch.rs` `start`), so this window's `RpcClient` would keep dialling the old one into an outage. After the reload the app boots on the shell's cached new connection; react-query and the restart store start empty, which is what `reset()` + invalidate were for. Spinner/disabled pattern as General's "Install and restart"; a failure toasts (`useAction`) and keeps the banner.
3. **Ownership is known**: `connection()` returns the cached `Connection` with `owned`; on a reused daemon (service install, terminal `t --serve`) `relaunch` would only reconnect to the same process, so the button stays hidden there.
4. Band tabs show icon + label from `md` up and icons only below, as every other band does; `TabBand` takes `inset: false` beside the column so the column, not the band, clears the macOS traffic lights.
5. `SectionRow`'s `back.to` widened to `Tab | '/settings'` instead of a second row component.
6. Every redirect replaces (C0's unknown-id redirect did not). Tab targets carry only `focus`; `session`/`peer` are read only for the chat redirect. `MOVED_TABS` is read with `Object.hasOwn`, so `/settings/constructor` is an unknown id, not a crash. An old id whose new tab is hidden lands on the next shown tab in one hop.
7. Slash call sites point where the setting now lives: `/title`, `/escalate`, `/vision` → `helpers`; `/subagent` → `models`; `/communication`, `/a2a` → `messaging`; `/acp` → `harnesses`; `/login`, `/provider`, `/aws` → `providers`. `/peers` (per chat) opens the chat's Messaging… sheet (`/c/$session?messaging=1`). The `/approvals` help line no longer names config.yaml ("`always` persists the node-wide default", matching `/communication`).
8. Desktop `/settings` redirects in the component (`<Navigate replace>`): `useIsMobile` is a hook. The sidebar button navigates straight to `/settings/appearance` on desktop and `/settings` on a phone, so it never takes that hop.
9. The banner sits at the top of the scrolling body, inside the 720 px column.
10. `__root.tsx`: the shortcuts effect lost its `session` dependency with the parameter.
11. Tests stub `localStorage` (the registry imports every tab component; `lib/nudges` reads it at import), and the visibility cases set every tab's `visible` themselves: a real one (`hasLocal()`) reads the container.

## Deviations from the brief

- Decision 1 (phone order) and decision 2 (reload in place of reset + invalidate), reasons above.
- `CommunicationNotice.tsx` (W-CHAT's): `settings(navigate, 'messaging')` left its `session` prop unread (TS6133), so the call is `session ? navigate(chat ?messaging=1) : settings(navigate, 'messaging')`, the same place the old `/settings/communication?session=` link now redirects. `SessionMenu.tsx` got the literal `settings(navigate, 'messaging')`. The integrator keeps W-CHAT's version of both lines.

## Hand-offs

- **W-CHAT**: `SessionMenu.tsx` ~560 and `CommunicationNotice.tsx` ~66 as above; `/c/$session?messaging=1[&peer=]` is what the route redirect and `/peers` open.
- **W-VOICE**: `Calendar.tsx` line 19's comment still names `SettingsDialog`.
- **W-NET**: `Contacts.tsx` and `Communication.tsx` still link `communication` / `network`; the redirects cover them until retargeted. `Contacts.test.ts` asserts the old id.
- **Integrator**: after Restart theuth, other webviews (island, overlays, pop-outs) keep clients on the old port, as after the startup screen's Relaunch today; a shell broadcast or an exported App relaunch would cover them. The Rust tray still sends `/settings/general`; the redirect covers it.

## Proof

`bunx vite` on 5188 without an engine shows the pairing screen, not the router, so no screenshots. A throwaway SSR render (stubbed router, sidebar, registry, restart store; deleted after) confirmed: the column lists only categories with a shown tab and marks the current one; hidden tabs leave the band; the body gets `focus`; both banner wordings; the phone page has the back row and no column; the phone list links each category's first tab.
