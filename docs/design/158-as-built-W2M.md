# 158 as-built — W2M (macOS companion: EventKit calendar source)

Branch `feat/158-w2m`, cut from the C158 PIN `715e22d3`. Swift only; no Rust, TS or C# touched.

## Built

- `crates/desktop/companion/Sources/Companion/Calendar/EventKitMapping.swift` — `EventFields` (the `EKEvent` fields the push
  reads, as a plain value; `init(EKEvent)`), `EventFields.Person.attendee` (name → else the address's local part;
  `participantStatus` → `ACCEPTED` / `TENTATIVE` / `DECLINED` / `NEEDS-ACTION`; `isCurrentUser` → `is_me`),
  `CalEvent.init(EventFields)` (`id = eventkit:<uid>:<recurrence_id or "">`, RFC 3339 UTC whole seconds, `occurrenceDate`
  as `recurrence_id` when `hasRecurrenceRules || isDetached`, `status == .canceled` → `cancelled`), `JoinLink` (event URL
  first, then the first URL in location / notes whose host matches §8.1's host map; Google `/url?q=` wrappers unwrapped).
- `…/Calendar/EventKitSource.swift` — `EventKitSource` (`@MainActor`, `shared`): `install(store:)` from `AppDelegate`
  (skipped under any probe / `--ask-probe`); on every RPC connect `calendar_settings_get` → `eventkit`; `observe(Data)`
  decodes the `{"type":"EventKit","enabled":…}` bus item; enabling checks `EKEventStore.authorizationStatus(for: .event)`,
  calls `requestFullAccessToEvents()` when not determined, reports `calendar_source_state("eventkit", granted | denied |
  not_determined)` (`.writeOnly` / `.restricted` count as denied), then pushes the −1 … +14 day window
  (`predicateForEvents(withStart:end:calendars: nil)`) with `calendar_push`; re-pushes on `EKEventStoreChanged` (debounced
  2 s), `NSWorkspace.didWakeNotification`, and a 15-min timer; one push in flight, a request during it runs once after.
  Disabling tears observers and timers down. `Store.calendarObserve(_:)` lives here too.
- Hooks: `AppDelegate.applicationDidFinishLaunching` — `EventKitSource.install(store: store)` after `store.start(…)`
  (needs the client); `Store.handle` — `calendarObserve(data)` beside `pointObserve`.
- `scripts/bundle-companion.sh` — `NSCalendarsFullAccessUsageDescription` in the generated `Info.plist` (the companion's
  plist is a heredoc there; there is no checked-in companion `Info.plist`).
- `crates/desktop/companion.entitlements.plist` — `com.apple.security.personal-information.calendars`.
- `crates/desktop/companion/Package.swift` — `CompanionTests` test target depending on the `Companion` executable
  (SwiftPM links executable modules for `@testable import`).
- `crates/desktop/companion/Tests/CompanionTests/EventKitMappingTests.swift` — 8 tests: event URL wins; Teams link in
  notes (`<…>` wrapped, percent-encoded), Meet link in location, Zoom in notes over a non-meeting event URL; Google
  redirect unwrapped; plain-text URLs ignored; organizer + attendees (status words, local-part name, `is_me`); all-day,
  RFC 3339 UTC, `id`, `tz`; recurring instance `recurrence_id` + cancelled; snake_case wire shape.

## Deviations

- Usage string: the brief's "nothing leaves your Mac" contradicts §7.5 (event text reaches the chosen model); the plist
  carries the design's §7.2 sentence instead. Orchestrator may swap it — one line in `scripts/bundle-companion.sh`.
- Two hook lines, not one: the bus item needs a line in `Store.handle` (private), the install a line in `AppDelegate`.
- `calendar_settings_get` has no `RpcClient` method in the contract; the source calls `client.call("calendar_settings_get")`
  with a private `{ eventkit }` decodable rather than widening `RpcClient` (not mine).
- `id` for a one-off event is `eventkit:<uid>:` (empty trailing `recurrence_id`). The engine's W2 store must build the same
  key for EventKit rows, or treat `id` as the companion's opaque key.
- State words are lowercase snake (`granted` / `denied` / `not_determined`), matching `CalSource.state`'s doc
  (`granted` / `denied` / `not_running`), not §7.2's `Granted | Denied | NotDetermined` casing.

## Hand-offs

- Engine W2: forward `CalendarEvent::EventKit` onto `subscribe_events` as `{"type":"EventKit","enabled":bool}` (the
  contract says "forwarded like MeetingEvent"); accept `x-theuth-client: companion` for `calendar_push` /
  `calendar_source_state` (the companion sends the header on every dial, `RpcClient.request(for:)`).
- Settings UI: "Companion not running" is the engine/UI's inference when no state was reported; the companion only
  reports after a connect with `eventkit = true`.

## Permission prompt on this Mac

Not exercised live. `swift build` yields a bare executable with no bundle / `Info.plist`; EventKit refuses
`requestFullAccessToEvents` without `NSCalendarsFullAccessUsageDescription` (the process is killed by TCC). Exercising it
needs `scripts/bundle-companion.sh` (release build + signing) and a launch of the bundle with `meeting.calendar.eventkit =
true`; the TCC prompt then names "Theuth Companion" and the result lands in `~/.theuth/logs/companion.log` as
`calendar: eventkit granted|denied`. Reset for a re-test: `tccutil reset Calendar <bundle id>`.

## Libraries

- Link detection: Foundation `NSDataDetector(.link)` (stdlib; handles `<…>` wrapping, trailing punctuation, bare hosts).
  Rejected: a regex (worse on edge cases, more code) and a SwiftPM URL-extraction package (no maintained one worth a dep).
- Dates: `ISO8601DateFormatter` for RFC 3339 UTC. Calendar access and recurrence expansion: EventKit itself (instances
  come expanded from `predicateForEvents`; no RRULE code on the companion).
- Debounce / timers: `Timer` on the main run loop, the pattern `Store.scheduleRefetch` already uses. No new dependencies.

## Open items

- `store.events(matching:)` runs synchronously on the main actor (ponytail: a 15-day window is small; move to a detached
  task if a huge calendar ever stalls the UI).
- The 15-min re-push and the `EKEventStoreChanged` push are full-window replaces by design (§7.2, idempotent).

## Gates

- `BUILD_LABEL=158w2m ~/.theuth/bin/build-lock swift build` (in `crates/desktop/companion`) → `Build complete!`
  (pre-existing `NSEvent`/`Sendable` warnings in `AskBarPanel.swift` only).
- `BUILD_LABEL=158w2m ~/.theuth/bin/build-lock ./test.sh` → `Test run with 269 tests in 45 suites passed`
  (8 new in `EventKitMappingTests`, all three test targets).
