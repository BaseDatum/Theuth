# 188 as built: W-LEARN (Learning, Channels, Tools › Web search and Computer use)

Branch `feat/188-learn`, one commit on `d02f4959`.

## What each tab holds now

| Tab | Component | Holds |
|---|---|---|
| Learning › Dreaming | `Dreaming.tsx` | Status card · Dream now · **Enabled** (the dreaming plugin switch, F2) · Window · Budget per night · Models · Auto-promote preferences · Weekly digest · Pause · Recent nights · Proposals |
| Learning › Double | `Double.tsx` | **Enabled** (switch, F1) · Pause learning · Active version · Last run · Next window · Approvals off · Improvement · Classes · Training · Training log |
| Learning › Decision models | `Labs.tsx` | **Enabled** (`ConfigFlagRow decision.enabled`, F4); while the engine runs with it: Loaded now · Meeting oracle · Browser System-1 · Training data |
| Channels › Signal | `Signal.tsx` | Account · Link this device · Owner · Allowed numbers |
| Channels › Slack | `Slack.tsx` | App manifest · Bot · Owner · Allowed users · Channels |
| Channels › Telegram | `Telegram.tsx` | Bot · Bot token · Owner · Pair the owner · Allowed users · Groups · Setup notes |
| Tools › Web search | `Search.tsx` | unchanged |
| Tools › Serve over MCP | `Mcp.tsx` | unchanged mapping (file is W-CHAT's) |
| Tools › Computer use | `Computer.tsx` | **Enabled** (`ConfigFlagRow computer.enabled`, F6); while it runs: Accessibility · Screen Recording · Displays |

## Removals and moves

- **Removal 3:** Dreaming's routine-switch copy (`api.enable_schedule('dreaming', …)`) and its local `enable` action are gone. The schedules query stays: `dreamNowBlocker` and the Tidy tier's default still read the routine. `enable_schedule` itself stays in `api.ts`, because Routines uses it.
- `useLabsVisible` and its header comment were deleted. Nothing imported it.
- `Channels.tsx` was split. `signalState` + `Signal` went to `Signal.tsx`; `slackState`, `Slack` and `SlackManifest` went to `Slack.tsx`; `telegramState`, `telegramPhrase`, `telegramId`, `secondsLeft`, `mmss`, `useCountdown`, `TELEGRAM_LIVE`, `withId` and `Telegram` went to `Telegram.tsx`. `Channels.tsx` keeps only the shared parts: `CHANNEL_EVENTS`, `useChannelEvents(key)` and `AllowList`.
- `Channels.test.ts` was split the same way, case for case: the Telegram cases went to `Telegram.test.ts` and the Slack cases to `Slack.test.ts`.
- Dead ends removed: "dreaming.enabled is off in config", "`double.enabled` is off … Turn it on in config.yaml and restart." and "Computer control is disabled (`computer.enabled: false`)". The tier C tooltip no longer names `double.training.tiers.c.enabled`. In `lib/dreaming.ts`, "dreaming is off in config (dreaming.enabled: false)" now reads "dreaming is off".

## Decisions

1. **Dreaming Enabled** saves through `reduceSettings` with a new `{ kind: 'enabled', on }` change, the same path as Window and Budget. A no-op returns `null`, and that is tested. The switch and its description follow the saved `settings.enabled`. Because the engine applies the change at once, the status refetch (`KEY`) updates the blocker. `save` now invalidates `[KEY, SCHEDULES]` for every dreaming settings write, not only for `enabled`. One action is simpler than a second one just for `enabled`, and a window change refreshing the routine is harmless.
2. **Double Enabled** has its own `double_settings_set` action inside `Status`, and only `doubleSettings` is invalidated. The running value (`double_status().enabled`) cannot change before a restart. The switch's `aria-label` is "Double enabled", matching "Dreaming enabled"; the old dot had none. The off description is "Off: nothing is predicted or learned." The pure `enabledRestartPending(saved, running)` returns `null` while either value is unknown, and the effect then neither marks nor clears, so the banner does not flicker while the tab loads. It is tested in `Double.test.ts`, along with a render check that shows "Applies after theuth restarts.".
3. **Decision models and Computer use need a known running state.** The new `flag-runs.ts` holds `flagRuns` and `useFlagRuns`, tested in `flag-runs.test.ts`, because two tabs share them. They return `null` while `config_flag_get` is loading, and in that state nothing below the switch renders or queries, so neither `decision_stats` nor `computer_permissions` fires before the flag is known. A `config_flag_get` error counts as running, so against an engine without W-ENG the tabs still show what they did before. `ConfigFlagRow` shows the error on its row.
4. The Decision models switch sits in a `Section` titled "Decision models", and its `Empty` line reads: "While they run, this tab also holds the meeting oracle and browser System-1 modes, training-data capture and the models loaded now." It is worded to read correctly both before and after the switch is flipped, until the restart.
5. **Computer use:** the switch is the first row of the existing "Computer" section. Refresh is hidden unless the feature runs, because `refetch()` would bypass `enabled: false`. A permissions error now shows its own message instead of the old dead end. While off: "The displays and permissions show once computer use runs."
6. **Channel events:** each tab calls `useChannelEvents(queryKeys.<channel>)`, which refetches its own status on any channel bus event. Before, one effect refetched all three statuses. Only the mounted tab's query is live, and the others refetch when they mount (they are stale), so what you see is unchanged.
7. The Slack test "App manifest is the first Slack row and renders before slack_status resolves" now checks that the manifest row renders and the Bot row does not, instead of checking its position relative to Telegram, which no longer shares the render.
8. The category modules' C0 placeholder comments were replaced with one-line `188:` comments. Ids, labels, icons and order are unchanged. `Labs.tsx` keeps its file and export names; its header comment now names the tab Decision models.

## Deviations

None from the brief. `Search.tsx` line 4 still has a source comment saying settings are "persisted to config.yaml". It is not user-facing and the file is otherwise untouched, so I left it.

## Hand-offs

- **W-ENG:** `config_flag_get` / `config_flag_set` for `decision.enabled` and `computer.enabled`. Until they land, both rows show the RPC error and the tabs behave as before (decision 3).
- **W-SHELL:** `double.enabled` is marked and cleared through `useRestartPending`, with the label "Double".
