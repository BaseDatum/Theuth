# 188 as built: VOICE (Voice, Meetings, Calendar, meeting hotkeys in Shortcuts)

Branch `feat/188-voice`, one commit on C0b `d02f4959`. Tab ids, labels, icons and order are C0's, unchanged.

## What each tab holds

| Category › tab (id) | Component | Rows |
|---|---|---|
| Voice › Microphone (`voice`) | `Voice.tsx` `Microphone` | Microphone: Input device · Talk: Turn chime (moved from Chat › Talk) |
| Voice › Dictation (`dictation`) | `Voice.tsx` `Dictation` | Shaping (Composer, Phone) · End phrases (Send, Scratch, Literal) · Polish: Polish model |
| Voice › Voice key (`voice-key`) | `VoiceKey.tsx` `VoiceKey` | Voice key · Send the screen · Read what the pointer touched · Draw the trail · Panel position · Hints · disclosure |
| Voice › History (`voice-history`) | `Voice.tsx` `VoiceHistory` | History (Forget all, rows) · Debug: Last dictation |
| Meetings › Defaults (`meetings`) | `Meetings.tsx` `MeetingDefaults` | Meeting model · Summary model · Template · Transcript language · Default project |
| Meetings › Detection (`detection`) | `Meetings.tsx` `MeetingDetection` | Detect meetings from the microphone · Ignored apps |
| Meetings › Live (`live`) | `Meetings.tsx` `MeetingLive` | Echo in your microphone (not macOS) · Live tools: Allowed while a meeting runs · Screen capture: Hidden from capture · Live transcript: Live speaker labels |
| Meetings › Speakers (`speakers`) | `Meetings.tsx` `MeetingSpeakers` | Enrolled speakers |
| Meetings › Templates (`templates`) | `MeetingTemplates.tsx` `MeetingTemplates` | Templates list and editor |
| Calendar › Calendars (`calendar`) | `Calendar.tsx` `CalendarSources` | macOS Calendar: Use macOS Calendar · Calendar subscriptions: rows, Add subscription |
| Calendar › Briefs (`briefs`) | `Calendar.tsx` `CalendarBriefs` | Pre-meeting briefs |
| Calendar › Upcoming (`upcoming`) | `Calendar.tsx` `CalendarUpcoming` | Upcoming (next 24 h) |
| General › Shortcuts (`shortcuts`) | `Shortcuts.tsx` `Shortcuts` | existing groups, then **Meetings**: Eye hotkey · Push-to-talk hotkey |

Shared queries live once: `components/settings/meeting-queries.ts` (`SETTINGS_KEY`, `useMeetingBlock`: the `meeting` block query, its whole-block save and `set(patch)`, used by Defaults, Detection, Live) and `components/settings/calendar-queries.ts` (`useCalendarConfig` for Calendars and Briefs; `nextDay()`, the minute-rounded 24 h window that Calendars invalidates and Upcoming reads). The Voice tabs share only query keys and the surface table, which sit once at the top of `Voice.tsx`.

## Removals and moves

- **Voice key › Keys** (approved removal 6): the row, its `Link` and the `dictate` chord read are gone from `VoiceKey.tsx`.
- **Meetings › Local decision oracle** (approved removal 10): `OraclePointer` deleted, with its `decision_mode` query and the `Link` import.
- **Eye hotkey, Push-to-talk hotkey** moved from `Meetings.tsx` to `Shortcuts.tsx` as a "Meetings" group after System-wide. Today's `Hotkey` moved as `MeetingHotkey`: same probe-then-`setHotkey` commit (no probe when the chord is the fallback), same labels, descriptions and fallbacks (`DEFAULT_EYE`, `DEFAULT_PTT`). On touch devices the existing notice is still the whole page.
- **Turn chime** re-created in Voice › Microphone exactly as in `Chat.tsx` › Talk (label, description, `Switch` on `useTalkSettings` `cue` / `setCue`). `lib/talk-settings.ts` header updated.
- **`Calendar.tsx` `ready`** deleted (only its test imported it) with the test "the tab is live". The whole-page `Voice`, `Meetings` and `Calendar` components are gone; nothing outside `categories/*.tsx` imported them.

## Decisions

1. Tab components stay in their existing files (one exported component per tab) rather than one file per tab: fewest files; the shared hooks are the `.ts` modules above.
2. The meeting chords display through `label()` like every other chord on the Shortcuts page (⌥⇧E, not `Alt+Shift+E`). Stored values and the commit are unchanged.
3. Hidden from capture's description adds "The eye hotkey that flips it is in General › Shortcuts."
4. Voice key's accessibility row now says "its state is in Tools › Computer use" (was "Settings › Computer", a tab that no longer exists). File header comments updated to the same names.
5. The meeting block's load error shows on each of Defaults, Detection and Live (it was shown once, on the old page's Defaults section); each tab is its own page now.
6. The one-time "use your remembered meeting model" offer (158 §6.4) runs from Meetings › Defaults, the tab holding the Meeting model row and the one `/settings/meetings` lands on.
7. `MeetingTemplates` props are `Partial<TabProps> & { initial? }` so the component is a `SettingsTab.Component` directly (its own `{ initial? }` alone fails the weak-type check); it takes no `focus`.
8. Voice.test's old "preferred mic is selected" assertion only passed because the whole-page markup contained "Shure MV7" in the debug row; Radix renders no value server-side. The Microphone test now checks the device select is enabled (the list landed) and that Turn chime is there.

## Hand-offs

- **W-AGENT:** delete Turn chime from `Chat.tsx` (until then it shows in both places). `Meetings.tsx` still imports `SAME_AS_NEW` from `@/components/settings/Chat`, as `AgentDetail.tsx` does; if that constant moves, re-point both imports.
- **W-SHELL:** no change needed; ids and the category modules' shape are C0's.

## Deviations from the brief

None.
