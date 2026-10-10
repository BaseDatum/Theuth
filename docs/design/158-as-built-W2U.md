# 158 as-built — W2U (desktop UI: Settings › Calendar, event switcher)
Branch `feat/158-w2u` from the C158 contract head `ccc48db5`. UI only (`crates/desktop/ui`); no Rust, Swift or C# touched.

## Built
- `src/lib/calendar.ts` — pure helpers: `calendarKeys` (query keys; `lib/rpc.ts` is not W2U's), `eventSpan` / `eventLine`
  (`Title · 10:00–10:30`, local, `formatAt`'s day prefix when not today), `candidateWindow(at)` (§8.1: −15 min … +10 min),
  `eventkitState(on, mac, source)` (§7.2 / §7.6 state line: ICS hint off-Mac, `granted` / `denied` / else "Companion not running"
  from the `eventkit` `CalSource.state`), `choices(match, candidates, current)` (match first, then alternatives, then the
  window's live non-all-day events, each id once; the manifest event stays on top after start), `setEvent(api, target, id)`
  (§8.2 routing: `meeting_detection_set_event` before start, `meeting_set_event(id, …)` after; `null` clears).
- `src/components/settings/Calendar.tsx` — `ready = true`. Sections: **macOS Calendar** (switch → `calendar_settings_set
  { ...config, eventkit }`, disabled off-Mac, state line from the `eventkit` source row); **Calendar subscriptions (ICS / iCal
  secret address)** (ICS rows from `calendar_sources`: name · `synced N min ago` or the amber `last_error`; Refresh →
  `calendar_source_refresh`; Delete → `deleteSource` (confirm, then `calendar_source_delete`); Add form name + URL (password
  field) → `addSource` (trim, `https://` / `webcal://` required, then `calendar_source_add`); the §7.6 caption and the vault
  line); **Pre-meeting briefs** (switch → `prebrief`); **Upcoming** (`calendar_events(now, now+24 h)`, cancelled dropped, start
  order, location · join host). Settings/sources/events are `useQuery`; writes are `useAction` (toast on error, invalidate).
- `src/components/meeting/EventSwitcher.tsx` — props `{ detection, onChanged? }` (before start) or `{ meeting, event,
  startedAt }` (after). A `DropdownMenu` whose trigger reads the current event line or "Which meeting is this?"; the content
  is a radio list of `choices(...)` from the match plus `calendar_events(candidateWindow(at))` (`at` = now / `started_at`,
  fixed per mount), then "None of these" (before) / "No event" (after) → `setEvent`. On success: before start, `onChanged(next
  Detection)` and `meetingStatus` invalidated; after start, `queryKeys.meeting(id)` + `meetingStatus` invalidated.
- Tests: `src/lib/calendar.test.ts` (lines, choices ordering/filters, `setEvent` routing before vs after start, state line),
  `src/components/settings/Calendar.test.ts` (`ready`; static render: names + sync age + amber error, EventKit state line, URL
  field is `type="password"`, the pasted secret never in the markup, vault caption, Upcoming row; `addSource` trims / refuses
  blank name and non-https; `deleteSource` confirm → RPC), `src/components/meeting/EventSwitcher.test.ts` (trigger line in all
  four states).

## Deviations
1. "Masked URL" rows (brief, mock): `CalSource` carries no URL and no RPC returns it (§7.3), so rows show the **name**; the URL is
   typed once into a password field and never rendered. Masking client-side would need a URL the engine does not send.
2. "Upcoming" lists the whole next 24 h (brief) rather than the next 5 (§7.6) — a day rarely exceeds a screen.
3. Delete confirms first (`deleteSource`, like `deleteMeeting`): the vault secret goes with the row. Not in §7.6; one dialog.
4. `addSource` pre-checks `https://` / `webcal://`; the engine still normalises and refuses the rest (§7.3). Local-only loopback
   `http://127.0.0.1` URLs therefore need `https://` from the UI — acceptable for a settings paste box.
5. EventKit "Granted" line appends `pushed <time>` when `last_fetch` is set (the companion's last push) — extra, cheap.

## Hand-offs
- **W1U** (`DetectionBanner.tsx`): render `<EventSwitcher detection={d} onChanged={setDetection} />`; the `[▾]` list and the event
  line are both inside the trigger. If W1U keys its detection query other than `queryKeys.meetingStatus`, pass `onChanged` (the
  switcher does not know that key).
- **156/W2 meeting page** (not W2U's files): the header's Event chip is `<EventSwitcher meeting={id} event={manifest.event}
  startedAt={manifest.started_at} />`.
- **`lib/rpc.ts`** (not W2U's): on `CalendarEvent::SourcesChanged` (once W2 subscribes it in `bridge.rs`) invalidate
  `['calendar-sources']` and `['calendar-events']` so Settings › Calendar follows companion pushes without a Refresh.
- **Rust W2**: the UI reads `CalSource.state` as `granted` / `denied` (the source comment's lowercase words), anything else =
  "Companion not running".

## Libraries
- Dropdown: the existing `radix-ui` `DropdownMenu` wrapper (`components/ui/dropdown-menu.tsx`); no new dependency.
- Relative time: `ago()` from `lib/codemap/map.ts` (already in the tree); day/time: `formatAt` from `lib/time.ts`.
- No date library added: two `Date` arithmetic lines; `date-fns`/`dayjs` rejected as a new dependency for that.

## Open items
- No interactive (click) tests: the UI test setup is `renderToStaticMarkup` without jsdom/testing-library, so the RPC paths are
  proven through the exported seams (`addSource`, `deleteSource`, `setEvent`), as `MeetingList.test.ts` does.

## Gates (after `.ui-ready`, in `crates/desktop/ui`)
- `BUILD_LABEL=158w2u build-lock bunx tsc --noEmit -p .` → exit 0, no diagnostics (`--listFiles` confirms the three new sources
  and three tests are in the program).
- `BUILD_LABEL=158w2u build-lock bunx vitest run src/lib/calendar.test.ts src/components/settings/Calendar.test.ts
  src/components/meeting/EventSwitcher.test.ts` → 3 files, 9 tests passed.
- Not run: vite build, cargo/mbx, Swift, C# (out of scope). No live-engine smoke: the W2 Rust RPCs are still C158 stubs
  (`calendar_sources` → `[]`, `calendar_source_add` → error), so the panel shows the empty states against today's engine.
