# 158 as-built — W1U (desktop UI: detection banner, Settings › Meetings defaults, one lawful sentence)

Branch `feat/158-w1u` from the C158 contract head `ccc48db5`. UI only, under `crates/desktop/ui/src`. No Rust, Swift, C#, bindings or
`lib/meeting-settings.ts` changes.

## Built

- `lib/detection.ts` (new) — the prompt's pure half: `LAWFUL` (R3 verbatim), `detectedStart(Detection) → StartMeeting` (§5.4: the
  detected pid, `consent: true`, `source: { kind: 'detected', app_id }`, `title`/`event`/`project` when W2/W3 fill them, everything else
  the engine's default), `reduce(BannerState, MeetingEvent | Keep)` (a prompt per app id; `DetectionCleared` for another app ignored;
  `Started` ends the prompt; `AppReleased` → a release prompt until Keep or that meeting leaves `Recording`), `timeRange`.
- `components/meeting/DetectionBanner.tsx` (stub → real, §5.5) — seeded by `meeting_detection` (`enabled` only for the local desktop
  daemon: `useEngines().active === LOCAL && isDesktop()`), then driven by `on('Detected' | 'DetectionCleared' | 'AppReleased' | 'Started'
  | 'State')` from `store/sessions`; once the bus has spoken (`live`) the RPC answer no longer applies. Renders `<App> is using your
  microphone`, the `Title · hh:mm–hh:mm` line + project chip + `<EventSwitcher detection={…}/>` when an event or project is present,
  **Start recording** (`meeting_start(detectedStart(d))`, then navigates to the meeting), **Not now** (`meeting_detection_dismiss(app_id,
  false)`), ⋮ → **Never for <App>** (`…, true`), and the lawful sentence once. `ReleasePrompt` (exported): `<App> stopped using the
  microphone 30 s ago — Stop recording?` with **Stop** (`meeting_stop`) / **Keep recording** (local dismiss). Same `Column` width as the list.
- `components/meeting/StartForm.tsx` — Amendment A2: the consent checkbox, `CONSENT` export and the 53 §0.14 wording are gone; the R3
  sentence sits where the checkbox was; `startOptions` lost its `consent` parameter and always sends `consent: true`; Start is no longer
  gated. Nothing else in the form changed.
- `components/settings/Meetings.tsx` — §6.3 sections above today's Screen capture, bound whole to `meeting_settings_get/set`
  (`SETTINGS_KEY = ['meeting-settings']`; each control writes `{ ...block, field }`; a failed write toasts and changes nothing):
  **Defaults** — Meeting model / Summary model (`ModelCombobox`, `none` = "Same as new sessions" / "Same as meeting model"), Template
  (`Select` over `meeting_templates`, else the one slug on file labelled "General"), Transcript language (`en` only, caption says so),
  Default project (`Select`: None + distinct session projects ∪ styled projects, the sidebar's `useProjects` + `useProjectStyles`).
  **Detection** — "Detect meetings from the microphone" switch; Ignored apps as removable chips + add field (Enter or Add; trimmed, deduped).
  **Live tools** — the allowlist as the same chips; header action "Reset to default" writes `DEFAULT_LIVE_TOOLS` (idle when already default).
  The existing Live speaker labels switch stays on `set_meeting_live_diarization` and now also invalidates the block (it carries
  `live_diarization` too), and a block write invalidates the switch.
- Tests: `lib/detection.test.ts` (5), `meeting/DetectionBanner.test.ts` (5), `meeting/StartForm.test.ts` (3, rewritten for the sentence),
  `settings/Meetings.test.ts` (3). All static-markup (`renderToStaticMarkup`, the repo's convention); handlers are exercised through
  label-keyed mocks of `Button` / `DropdownMenuItem` / `ModelCombobox` / `Switch`, so Start, Not now, Never, Stop/Keep and every
  settings write are called for real against a mocked `Api`.

## Deviations

1. `EventSwitcher` is rendered as `<Switcher detection={d}/>` through a cast (`EventSwitcher as unknown as (p: { detection }) => ReactNode`):
   the C158 stub takes no props and TS rejects attributes on a zero-parameter component. Hand-off to W2U below.
2. (R1, built) §6.4's one-time migration: `lib/meeting-model-offer.ts` `offerMeetingDefault` — when the block has `model: null` and
   `useMeetingSettings.getState().model` is set, one `sonner` toast "Use <model name> as the meeting default?" with **Use it** →
   `meeting_settings_set({ ...block, model })`; shown once either way, remembered under `localStorage['meeting-settings-model-offered']`
   beside `meeting-settings` (that store's fields untouched). Fired from a `useEffect` in `Meetings.tsx` once block + models load.
   Tests `lib/meeting-model-offer.test.ts` (2: shown once, action writes the model, dismissed never again, nothing when the engine has a
   model or the form never picked one). Manual starts keep sending their localStorage model as `session_model`.
3. §6.3 names the Default project control a "combobox"; it is a `Select` (None + known projects), no inline "New project…" — a default
   project should be an existing one, and a per-keystroke write of a new name would hit the engine on every character.
4. `DEFAULT_LIVE_TOOLS` duplicates `MeetingConfig::default().live_tools` (`crates/meeting/src/lib.rs`): there is no RPC for the default.
   Drift risk is one list; a `meeting_settings_default` RPC would remove it (not requested).
5. Detection chips take any string (bundle id / image name / application name per OS, as the caption says); no per-OS validation.
6. No jsdom in the UI's `node_modules` (only `packages/apps` has one), so there is no DOM-event test; see Tests above for how handlers are hit.
7. Screenshot from `vite dev` not taken: the banner needs an engine answering `meeting_detection` with a prompt (no page fixture exists
   for it) and the W1 engine side is still the C158 stub (`Ok(None)`).

## Hand-offs

- **W2U** `components/meeting/EventSwitcher.tsx`: give the stub `({ detection }: { detection: Detection })` (the banner already passes it;
  drop the cast in `DetectionBanner.tsx` line "const Switcher" when the signature lands). Before start it should call
  `meeting_detection_set_event` and the banner will re-render from the next `Detected`.
- **W1 (Rust)** `rpc_settings_set` is `Err("158 W1: not built yet")` at this head: every Settings › Meetings write toasts that until W1 lands.
- **W1 (Rust)** `rpc_detection_dismiss` likewise; the banner's optimistic clear (`DetectionCleared` dispatched on success) only runs on `Ok`.

## Libraries

- No new dependencies. Reused: `@tanstack/react-query` (block + detection queries, `useAction`), Radix via `components/ui` (`Select`,
  `Switch`, `DropdownMenu`, `Badge`, `Input`), `ModelCombobox`, `ProjectPicker.useProjects`, `useProjectStyles`, `store/sessions.on`.
- Considered and rejected: a tag-input package for the chips (two lists, add + remove — 30 lines of `Badge` + `Input`); jsdom /
  `@testing-library/react` for click tests (not installed in the UI package; adding a test dependency is outside this wave's remit).

## Gates (BUILD_LABEL=158w1u, light pool)

- `build-lock bunx tsc --noEmit -p .` → exit 0 (baseline at ccc48db5 also 0).
- `build-lock bunx vitest run src/lib/detection.test.ts src/components/meeting/DetectionBanner.test.ts src/components/meeting/StartForm.test.ts
  src/components/settings/Meetings.test.ts` → 16 passed.
- `build-lock bunx vitest run src/lib/detection.test.ts src/components/meeting src/components/settings/Meetings.test.ts
  src/components/settings/Chat.test.ts src/dev/meeting-page-fixture.test.ts` → 10 files, 29 passed (the four new/changed suites plus the
  untouched meeting-page suites and Chat, whose `SAME_AS_NEW` Meetings now imports).

## R1 (one-time §6.4 offer)

- `build-lock bunx tsc --noEmit -p .` → 0. `build-lock bunx vitest run src/lib/meeting-model-offer.test.ts src/components/settings/Meetings.test.ts` → 5 passed.
