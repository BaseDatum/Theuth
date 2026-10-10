# 158 as-built — W3W: Windows companion island state (c) "Upcoming" + the project chip (C#)

Branch `feat/158-w3w` from main 23ea03c5. Scope: `crates/desktop/companion-windows/**` only. Design §5.2 (b)/(c), §9.4, §15.1;
mock `docs/design/assets/158-meetings-around.html` cells B and C; the C# twin of W3M on the W1W shapes.

## Built

**Core (`src/Theuth.Companion.Core`)**
- `MeetingReducer.cs`: `MeetingEventRef(Id, Title, Start, End, Attendees, Description)` (the fields the island shows and sends;
  `Attendees` = the others' names, `is_me` dropped; `StartAt`, `Line` = `<Title> · <hh:mm>–<hh:mm>` local). `MeetingDetection` gains
  optional `Event`, `Project`, `Brief` (W1 callers unchanged). `MeetingUpcoming(Event, Project, Brief)` with `Headline(now)` =
  `In N min · <Title>` (`Now · <Title>` after the start), `SourceLine` (null without a project), `Expired(now)` = `now > start + 5 min`.
  `MeetingChange.Upcoming`. `MeetingSnapshot.Upcoming` is its own slot beside `Prompt`; `UpcomingDismissed` remembers the dismissed
  event id; `Prompting = Prompt != null || Upcoming != null`; `UpcomingMerged` (the detection and the card name the same event) and
  `DetectedShown` (the prompt with the card's bullets / project merged in — its own non-empty brief wins, an empty one never replaces).
  Reducer (W3M parity rules 1–3): `Upcoming` is ignored while a meeting is live and cleared when one goes live; a dismissed event's
  re-send is ignored; a re-send with an empty brief keeps the bullets it had; a newer event replaces; `Started` empties the slice;
  `DismissUpcoming(s)`; `Expire(s, now)`.
- `MeetingWire.cs`: `"Upcoming"` in `EventTypes`; `Upcoming { event: EventRef, project, brief }` decoded (no `id`/`title` → null);
  `Detection.event` (an `EventMatch` — the best `event.event` is kept), `project`, `brief` decoded; `Strings()` for
  `Option<Vec<String>>`.

**Rpc (`src/Theuth.Companion.Rpc`)** — `Models.149.cs`: `Detection.Event: EventMatch?`, `Project`, `Brief: ValueList<string>?`;
`Attendee`, `EventRef` (all §15.1 fields; `id` is the transparent `EventId` string), `EventMatch`.

**App (`src/Theuth.Companion`)**
- `Island/IslandWindow.cs`: `IslandAction.StartUpcoming(MeetingUpcoming)` / `DismissUpcoming`. `RenderOpen`: (c) `RenderUpcoming`
  alone or above (a)/(b); when the detection names the same event, ONE pane — `RenderDetected(DetectedShown)` with the bullets and
  chip merged in (W3M rule 3). The head row (grip + eye) goes to whichever group is first. (c): accent dot · headline · project chip ·
  ≤ 3 bullets (the pane's drawn-dot `Bullet()`) · source line · Start recording (accent) · Dismiss · the R3 line. (b):
  `RenderDetected` adds the event row `<Title> · <hh:mm>–<hh:mm>` + the chip when `Detection.event` / `.project` are set, and the
  bullets when it carries (or merged) a brief. Shared `PromptHead`, `ProjectChip` (the existing accent `Badge`), `Lawful`. Keys:
  ⏎ = Start on (c) when no detection prompt holds ⏎; ⎋ = Dismiss on (c) when no detection prompt holds ⎋. `RenderKey` includes the
  card and its minute headline. UIA ids `island-upcoming`, `-title`, `-project`, `-source`, `-start`, `-dismiss`, `-lawful`;
  `island-prompt-event`, `island-prompt-project`.
- `Island/IslandController.cs`: Start on (c) → `meeting_start { title, context: description[..1000], consent: true, source:
  calendar{event}, event, project }` (no `process`: nothing holds the mic yet; W3M rule 4); Start on (b) now also sends `event` /
  `project` when the detection carried them; Dismiss → `MeetingReducer.DismissUpcoming` (local; no RPC exists). A 20 s
  `upcomingTimer` while a card is up: `Expire` at `start + 5 min` and the "In N min" re-render. Hydration maps
  `MeetingStatus.Detection.{Event,Project,Brief}`. Fixture `--island-upcoming` (event 4 min out, project `theuth`, the mock's three
  bullets); `--island-upcoming-auto`: (c) → same-event detection merges into one pane → Not now un-merges → another event's detection
  shows (c) above (b) → Dismiss → engine re-send ignored → Not now → gone; `upcoming158:` log lines + `%TEMP%\158-upcoming-*.png`.
  The step runner is shared with `--island-prompt-auto` (`FixtureSteps`); its log/PNG names are unchanged.

## Tests
- `IslandTests.cs` (+4): `UpcomingShowsAloneOrAboveTheDetectionAndClearsAtStartPlusFiveOrOnStart` (incl. live gating, empty re-send keeps
  bullets), `DismissedUpcomingIgnoresTheEnginesResends`, `ADetectionOfTheSameEventMergesTheUpcomingIntoOnePane`,
  `UpcomingCopyAndTheEventLine` (headline ceil/Now, source line, expiry boundary, `Line` local time + unparsable fallback).
- `MeetingWireTests.DecodesThePromptEvents`: `Upcoming` decodes (attendees without `is_me`, description, `brief: null`, malformed →
  null); `Detected` with an `EventMatch` + project + brief. The W1W "`Upcoming` → null" pin is replaced.
- `Models149Tests.DetectionDecodesAndStartMeetingEncodesThePromptStart`: `detection.event` match decodes; the (c) start's exact params JSON
  (title, context, consent, calendar source, event, project).

## Deviations
- Headline follows the mock / W3M brief (`In N min · <Title>`), not §5.2's ASCII sketch (`<Title> in N min`); `Now · <Title>` from the
  start until `+5 min` is mine (the design names no copy for that window).
- Source line reads `From the last meetings in <project>` (W3M rule 5: the wire carries no count) with the mock's ` · attendees …`
  suffix (≤ 4 names) when the event has other attendees. W3M's message quotes the line without the suffix — orchestrator may align.
- The chip is not clickable (§9 "click → project list"): no project-list RPC on the island and no `MenuFlyout` in a tool window (W1W).
- Dismiss is local; the reducer ignores re-sends of that event id. A fresh companion connecting mid-window shows the card at the
  engine's next one-minute tick (`Status` carries no `upcoming`; shape frozen).
- W3M parity (per its 16:58 message): rules 1–5 applied. Rule 2 ("no Upcoming beside a live meeting") also clears the card when a
  `State` goes live, not only on `Started`.
- The `[▾]` "Which meeting is this?" list (W2's surface) is not on Windows; `EventMatch.Alternatives` is decoded for it.

## Hand-offs
- None outside `companion-windows`.

## Libraries
- JSON `System.Text.Json`, dates `DateTimeOffset.TryParse` (RFC 3339, roundtrip), WinUI 3 controls already in use. No new dependencies.

## Gates
- `BUILD_LABEL=158w3w build-lock bash test.sh` (portable slnf, local .NET 10): Core 500/500, Rpc 114/114.
- build-B `build` `Theuth.Companion.slnx` Debug: `20261010T155632-328` @ 2a07b567 (1 error: CS0136, fixed), `20261010T155738-329`
  @ 600bb587, `20261010T160455-338` @ 4a148e3c → 0 warnings, 0 errors, WinUI app included.
- build-B `test`: `20261010T155820-330` @ 600bb587 (499/114); `20261010T160534-339` @ 4a148e3c → Core 500/500, Rpc 114/114.
- Visible runs under the desk lock (owner W3W; installed companion quit via `quit-installed`, restarted with `start-installed`, final
  pid 5576): `w6-gate.ps1 launch --island-upcoming --island-upcoming-auto --shareable` — run 1 @ 600bb587 jobs -332…-337, run 2 @
  4a148e3c jobs -340 quit, -341 launch, -342 grep, -343/-344 restore, -345 procs. Run 2 log: `upcoming — shown=True mode=Open prompt=-
  upcoming=fixture:weekly: rect=(685,8,550x228) text="… In 4 min · Weekly product sync · theuth · … Last week: shipped … · From the
  last meetings in theuth · attendees Priya Shah, Tom Okafor · Start recording · Dismiss · By starting … lawful."`; `merged —
  prompt=Detected upcoming=fixture:weekly: rect=(…550x230) text="Zoom is using your microphone · Weekly product sync · 17:10–17:40 ·
  theuth · Last week: shipped … · … Start recording · Not now · Never for Zoom · … lawful."` (one pane); `unmerged — prompt=-
  upcoming=fixture:weekly: rect=(…550x228)` (the card alone again); `both — prompt=Detected upcoming=fixture:weekly: rect=(…550x350)
  tail="… Daily standup · 17:10–17:40 · Start recording · Not now · Never for Zoom · … lawful."` (two panes); `dismissed —
  upcoming=- rect=(…550x146)`; `resent-ignored — upcoming=-`; `not-now — mode=Closed`; `gone — shown=False` (no linger). PNGs:
  `%TEMP%\158-upcoming-{upcoming,merged,unmerged,both,dismissed,resent-ignored,not-now}.png` on build-B.
