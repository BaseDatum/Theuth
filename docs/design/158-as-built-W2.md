# 158 as-built — W2 (calendar + matching + seed): the wave

Closed on `feat/158-w2e` (closer W2E). Base: C158 PIN `715e22d3` → contract head `ccc48db5` merged; main not merged.
Per-worker detail: `158-as-built-W2E.md` (engine), `158-as-built-W2M.md` (companion), `158-as-built-W2U.md` (UI).

| Worker | Branch / head | Scope |
|---|---|---|
| W2E | `feat/158-w2e` `458c43d2` (P1) | store, calcard ICS glue, fetch, matcher, RPC bodies, seed, poll task, `CalendarEvent` on the rpc bridge |
| W2M | `feat/158-w2m` `a9d02221` | EventKit source in the macOS companion: permission, −1…+14 d push, state report, entitlement + usage string |
| W2U | `feat/158-w2u` `7afc8ba8` (merged twice: `d75ee6e5`, then the flat-props commit) | Settings › Calendar (`ready = true`), `EventSwitcher` (flat props `{ detection?, meeting?, onChanged?, className? }`), `lib/calendar.ts`, tests |
| close | this branch `be80a9c8`+ | Event chip `<EventSwitcher meeting={id} />` in the meeting page header, `CalendarEvent` in the TS wire union, `SourcesChanged` invalidation, bindings, gates |

## What landed

- **Store** (`crates/meeting/src/calendar/store.rs`): `cal_sources` / `cal_events` queries over migration 0003; windowed replace in
  one transaction; UTC `…Z` spelling so string order is time order.
- **ICS** (`calendar/ics.rs`): [calcard 0.3.14](https://docs.rs/calcard) does parsing, RRULE/RDATE/EXDATE expansion, RECURRENCE-ID +
  THISANDFUTURE, VTIMEZONE and Windows zone names; our glue groups by UID (calcard keys overrides by `(SEQUENCE, RID)` without UID),
  filters the window, extracts organiser/attendees/join link (URL → conference X-props → first rule host in LOCATION/DESCRIPTION,
  Google `/url?q=` unwrapped). No hand-written ICS reader (Sky 14:16).
- **Fetch** (`calendar/fetch.rs`): conditional GET (ETag / If-Modified-Since), 30 s, 10 MB, `BEGIN:VCALENDAR` check; 401/403/404/410 →
  "URL rejected — regenerate the secret address in your calendar"; the URL is a vault row `calendar.ics.<id>` and never appears in an
  error, log or RPC (host + masked tail, then `Vault::redact`). Poll task from boot (`ensure_poll`, `poll_secs` ≥ 60).
- **Matcher** (`calendar/matcher.rs`): §8.1 `time + host + rsvp` (`title` = 0: no window title reaches the engine), SHOW 0.8 / MARGIN
  0.25 / LONE 0.5, the `[▾]` pick remembered per app (`choose` / `clear_choice`) so every later tick honours it.
- **RPCs** (`calendar/mod.rs`): `calendar_sources/add/delete/refresh`, `calendar_push` + `calendar_source_state` (companion-only via
  `x-theuth-client`, `source == eventkit`), `calendar_events`, `calendar_settings_get/set` (writes `meeting.calendar.*`, publishes
  `CalendarEvent::EventKit` on a flip), `meeting_detection_set_event`, `meeting_set_event` (manifest + steer line + attendee aliases).
- **Seed** (`lib.rs` StartMeeting handler): `StartMeeting.event` or `Source::Calendar` → title if empty, context block
  (`Event: …`, description ≤ 1 000 chars, `Attendees: A, B (organizer: A)` — names, never emails), `Manifest.event`.
- **Bus → clients**: `CalendarEvent` forwarded in `rpc/src/bridge.rs` + `client.rs`; TS `WireEvent` union gains it; `lib/rpc.ts`
  invalidates `['calendar-sources']` / `['calendar-events']` on `SourcesChanged`.
- **Companion**: `EventKitSource` enables from `calendar_settings_get` on connect and from the `{"type":"EventKit","enabled":…}` bus
  item; pushes `eventkit:<uid>:<recurrence_id or "">` rows; states `granted` / `denied` / `not_determined`.
- **UI**: Settings › Calendar (macOS Calendar switch + state line, ICS rows with sync age / amber error / Refresh / Delete, Add form
  with password URL field, Pre-meeting briefs switch, Upcoming 24 h); `EventSwitcher` before start (`detection`) and after
  (`meeting`, now rendered in the meeting page header, `ControlCenter.tsx`).

## Gates (merged tree, each its own command)

- `build-lock mbx test -p meeting -p rpc --no-run` → 0; `mbx test -p meeting -p rpc` → meeting 75 + 15 + 1 + 1, rpc 56 + 1 + 1 passed,
  0 failed (2 pre-existing ignored).
- `build-lock mbx clippy -p meeting -p rpc` → clean.
- `build-lock bash crates/desktop/companion/test.sh` → 269 tests in 45 suites passed (run alone).
- `build-lock bun install` (ui, packages/apps) → ok; `BUILD_SLOTS=1 build-lock mbx test -p rpc --features ts --lib --quiet
  export_bindings` → 8 passed, 870 files copied.
- `build-lock bunx tsc --noEmit -p .` → 0. `build-lock bunx vitest run` on `lib/calendar`, `settings/Calendar`, `meeting/EventSwitcher`,
  `meeting/MeetingList`, `meeting/ControlCenter`, `rpc/api`, `store/meeting` → 7 files, 23 tests passed.
- `BUILD_SLOTS=1 build-lock mbx check --workspace --tests` → 0 (only the pre-existing `block v0.1.6` future-incompat note).

## Live proof

**Skipped — machine gate.** `vm_stat` free + speculative × 16 KiB was 0.47 GB at 15:47 and 1.89 GB at 16:10 (load 147), both under
the 4 GB rule for starting a scratch engine. What stands in: `tests/calendar.rs::ics_server_to_rows_to_match_to_seed` runs a real
`tokio::net::TcpListener` HTTP server serving `google.ics` → `fetch_url` 200 (ETag kept) → 304 → 404 (`REJECTED`, no URL) → refused
connection (masked host only) → `replace_window` → `calendar_events`-equivalent query → `matcher::best` (Chrome at 10:02 → "Design
review") → `seed` → manifest/`started.` line with attendee names and no emails. The RPC bodies above it are one delegate call each.
The orchestrator's live pass (Sky's secret address, macOS Calendar, Settings › Calendar "Upcoming") is still owed; no real calendar
URL was used anywhere in this wave.

## Deviations (wave level; details per worker)

- `EventMatch` has no `shown` flag (frozen): `best` caps `score` just under `SHOW` when the list should open unselected.
- `is_me` is never known from an ICS feed → rsvp term is EventKit-only.
- Agenda lines reach the model inside the seed's context block, not a separate head section (`head.rs` not in W2).
- UI rows show the source **name** (no RPC returns the URL); "Upcoming" is the next 24 h rather than 5 rows; delete confirms.
- UI `addSource` pre-checks `https://`/`webcal://` — a loopback `http://127.0.0.1` test URL must be added over RPC.
- Companion usage string is §7.2's sentence (not the brief's "nothing leaves your Mac", which contradicts §7.5).

## Open items / seams for later waves

- **W1 seam**: `detect/rules.rs` must set `detection.event = calendar::match_for(&store, now, &app_id).await` on each tick and call
  `matcher::clear_choice()` on `DetectionCleared`; `rpc_detection_set_event` relies on `detect::rpc_detection(rt)` returning the live
  prompt. **W1U**: `DetectionBanner.tsx` renders `<EventSwitcher detection={d} onChanged={…} />` (the banner switcher is W1's).
- "Three consecutive failures → amber": no counter in 0003; the UI colours on `last_error`.
- calcard expands from DTSTART with a 5 000-instance cap per series (`ics.rs` `LIMIT`).
- Companion permission prompt not exercised live (needs the signed bundle; see W2M as-built).
- `Cargo.lock`: + calcard, chrono-tz, mail-parser, mail-builder, hashify, phf.
