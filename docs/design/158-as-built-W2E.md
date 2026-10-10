# 158 as-built — W2E (engine: calendar sources, event store, matching, seed)

Branch `feat/158-w2e`, cut from the C158 PIN `715e22d3`; `feat/158-contract` (`ccc48db5`) merged after the gate — clean,
none of my files touched; §18 A3 is not in the merged design text (ends at A2). Design §7, §8, §15.1–§15.3.
Ownership added by the orchestrator at 15:04: `crates/rpc/src/bridge.rs` (`CalendarEvent` forwarding).

## Built

- `crates/meeting/src/calendar/store.rs` — queries over migration 0003: `sources` / `source` / `insert_source` /
  `delete_source` (events deleted explicitly; sqlx does not switch `foreign_keys` on), `fetch_meta` / `mark_fetched` /
  `mark_failed` / `set_state`, `replace_window` (one transaction: delete the source's rows whose `start` is inside the
  window, upsert the new ones), `events` (overlap, `start` ascending), `event`. Every stored instant is respelled as
  `YYYY-MM-DDTHH:MM:SSZ` (`stamp` / `normalise` / `parse`) so string order is time order.
- `calendar/ics.rs` — glue over **calcard** (below): group VEVENTs by UID, expand each UID with `SEQUENCE` stripped
  (calcard keys overrides by `(SEQUENCE, RECURRENCE-ID)` without the UID — wrong for a multi-UID feed where Google bumps
  the override's SEQUENCE), window filter, rows with `id = source:uid:recurrence_id` (recurrence id = the original
  occurrence, UTC; the override's `RECURRENCE-ID`; a `THISANDFUTURE` tail keeps its own start), all-day from
  `VALUE=DATE`, `tz` = the `TZID` param, `STATUS:CANCELLED`, `SEQUENCE`, organiser + attendees (CN, `mailto:`, PARTSTAT;
  CUTYPE RESOURCE/ROOM dropped; no CN → local part), `join_link` (URL → `X-GOOGLE-CONFERENCE` /
  `X-MICROSOFT-SKYPETEAMSMEETINGURL` → first rule-matching URL in LOCATION/DESCRIPTION; Google `/url?q=` unwrapped;
  canonical hosts `zoom.us`, `meet.google.com`, `teams.microsoft.com`, `teams.live.com`, `webex.com`,
  `facetime.apple.com`). Floating times are local (fixed offset of now).
- `calendar/fetch.rs` — `normalise_url` (`webcal://`→`https://`; HTTPS only except localhost/127.0.0.1), `masked`
  (host + last 4 path chars), `fetch_url` (reqwest, 30 s, 10 MB, `If-None-Match` / `If-Modified-Since`, 304 →
  `NotModified`, 401/403/404/410 → `REJECTED`, body must start with `BEGIN:VCALENDAR`; every error has the URL replaced
  by `masked`), `url_of` (vault `calendar.ics.<id>` via `secrets::handle`), `refresh` (fetch → `replace_window` →
  `mark_fetched`, or `mark_failed` with the message passed through `Vault::redact`; rows and validators stay on failure).
- `calendar/matcher.rs` — §8.1 terms (`time_term`, `host_term`, `rsvp_term`, `score`), `best` (sort, SHOW/MARGIN, lone
  rule), `shown`, and the §8.2 pick: `choose(app_id, Option<EventId>)` / `clear_choice` — a process-wide 30-min choice
  that `best` honours on every later tick (chosen row preselected with `score ≥ SHOW`; `None` clears the match).
- `calendar/mod.rs` — `window()` (−1 d … +14 d), `CalEvent::to_ref` (description capped at 1 000 chars),
  `EventRef::{attendee_line, context_block}`, `seed(&mut Manifest, &CalEvent)` (title if empty, context block appended,
  `manifest.event`), `match_for(store, now, app_id)` for W1; actor messages implemented here on `Meeting` (child module,
  private fields): `Handles` (store + calendar config), `SetCalendarConfig`, `SetEvent` (manifest update, title if empty,
  attendee aliases scanned into cached topics, steer line `[meeting hh:mm:ss] event: … · Attendees: …` while live);
  `ensure_poll` (one task per process, `poll_secs` ≥ 60, `SourcesChanged` after a pass that changed a row); all nine RPC
  delegate bodies. `calendar_push` / `calendar_source_state` accept only `client == companion` and `source == eventkit`;
  push rebuilds ids as `eventkit:uid:recurrence_id` and refuses while `meeting.calendar.eventkit` is off.
  `calendar_settings_set` writes `meeting.calendar.{eventkit,prebrief,poll_secs}` with `write_nested_config_value`,
  tells the actor, publishes `CalendarEvent::EventKit` when that switch moved.
- `lib.rs` seed lines (StartMeeting handler, after `manifest.context`): `StartMeeting.event` or
  `Source::Calendar { event }` → `calendar::store::event` → `calendar::seed`. The existing alias scan over
  `title + context` then surfaces cached briefs for attendee names; `Manifest.event.attendees` is W4's speaker list.
- Tests `crates/meeting/tests/calendar.rs` (14) over synthetic fixtures `tests/fixtures/calendar/{google,outlook,icloud,
  fastmail}.ics`: weekly RRULE + EXDATE + moved instance (SEQUENCE 1), all-day, cancelled, Meet via X-prop and a Google
  redirect wrapper, folded lines, attendee PARTSTAT, a Windows zone name across the DST boundary, `THISANDFUTURE`, UTC
  `URL`, floating time, no-link event, a 23 kB folded description, window filter, non-ICS input; the five §8.1 examples,
  cancelled/all-day/declined, −10/+15 edges, the pick; a `tokio::net::TcpListener` server: 200 → rows, 304, 404 →
  `REJECTED` without the URL, refused connection → masked error; rows → store → stale row dropped → Chrome at 10:02 →
  Design review → `seed` → title/context/attendees/no emails/`started_line`/manifest round-trip → delete cascades.

## Libraries

- **calcard 0.3.14** (Stalwart, Apache-2.0/MIT, `default-features = false`) — chosen: parser + RRULE/RDATE/EXDATE
  expansion + RECURRENCE-ID/THISANDFUTURE + VTIMEZONE/X-LIC-LOCATION/X-MICROSOFT-CDO + the full Windows/Outlook zone
  name table in one crate; pulls `chrono-tz`, `mail-parser`, `mail-builder`, `hashify`, `phf` (new in the lock).
- `icalendar 0.17` + `rrule 0.14` + `chrono-tz 0.10` — rejected: `rrule` has no RECURRENCE-ID, so overrides,
  THISANDFUTURE and the Windows-name table would be ours (~the design's W-d reader), i.e. more hand-written logic.
- `reqwest` (workspace 0.13) for the GET; `url` (workspace) for host/redirect parsing; `secrets` (workspace) for the
  vault. No hand-written ICS reader; `urls_in` (find `http(s)://` tokens in free text) and the Google `/url?q=` unwrap
  are the only text handling.

## Gates

- `build-lock mbx test -p meeting --no-run` → ok; `mbx test -p meeting` → 75 + 14 + 1 + 1 passed, 0 failed, 2 ignored
  (pre-existing).
- `build-lock mbx clippy -p meeting` (and `--tests`) → clean after one `manual_pattern_char_comparison` fix.
- After the contract merge + bridge change: `build-lock mbx test -p meeting -p rpc --no-run` → ok; `mbx test -p meeting -p rpc`
  → meeting 75 + 15 + 1 + 1, rpc 56 + 1 + 1 passed, 0 failed; `build-lock mbx clippy -p meeting -p rpc` → clean.
- CLOSE W2 phase 1 (after `feat/158-contract` + `feat/158-w2m` merged, `ensure_poll` at boot): `build-lock mbx test -p meeting
  -p rpc --no-run` → ok; `mbx test -p meeting -p rpc` → meeting 75 + 15 + 1 + 1, rpc 56 + 1 + 1 passed, 0 failed;
  `build-lock mbx clippy -p meeting -p rpc` → clean; `build-lock bash crates/desktop/companion/test.sh` → 269 tests in
  45 suites passed.

## Deviations

- `EventMatch` has no `shown` flag (frozen). `best` reports the true §8.1 sum when the row is preselected and caps it just
  under `SHOW` when the margin/lone rule says "list, no preselection", so the stub's documented rule (`score < SHOW` ⇒
  unselected) holds for all three clients. `matcher::shown(&m)` encodes it.
- `is_me` is never set from an ICS feed (a secret address does not say whose it is); the rsvp term is 0 for ICS rows and
  works for EventKit (`isCurrentUser`).
- The agenda is not a separate `## Agenda` head section (head.rs is not mine); agenda lines reach the model inside the
  description block of the `started.` context, which is where §8.3's refinement wants them read from anyway.
- "Three consecutive failures → amber": no counter column in 0003; the row carries `last_error` and the UI may colour on
  it (`last_fetch` stays from the last success).

## Hand-offs

1. ~~`calendar::ensure_poll(rt)` at plugin start~~ — done here (`lib.rs` `MeetingPlugin::start`, after `register`): ICS
   polling starts at boot (CLOSE W2 phase 1 made these lines mine).
2. ~~forward `CalendarEvent`~~ — done here: `rpc/src/bridge.rs` (`forward!` + `rt.subscribe`) and `rpc/src/client.rs`
   (in-process republish). Wire shape pinned by `calendar_event_wire_shape_matches_the_companion`:
   `{"type":"EventKit","enabled":bool}` / `{"type":"SourcesChanged"}`. `export.rs` already lists `CalendarEvent` (C158).
3. W1 (`detect/rules.rs`): on each tick `detection.event = calendar::match_for(&store, now, &app_id).await` (or
   `matcher::best` over `calendar::store::events`); call `matcher::clear_choice()` on `DetectionCleared`.
   `rpc_detection_set_event` relies on `detect::rpc_detection(rt)` returning the live prompt.
4. UI (W2 UI worker): `CalSource.last_error` → amber row; the add form sends the raw pasted URL (engine normalises).

W2M (companion, `a9d02221`) wire contract, checked against this side: push ids `eventkit:<uid>:<recurrence_id or "">`
(rebuilt by `rpc_push`); states `granted` | `denied` | `not_determined` (stored lowercase as sent); `calendar_settings_get`
→ `CalendarConfig.eventkit`; header `x-theuth-client: companion` is what `companion_only` requires. No engine change needed.

## Open items

- calcard expands from DTSTART with a 5 000-instance cap per series: a daily series older than ~13 years yields nothing
  in the window (`ics.rs` `LIMIT`, ponytail note).
- Live proof (Sky's real secret address + macOS Calendar) is the orchestrator's; nothing here touched a real URL.
