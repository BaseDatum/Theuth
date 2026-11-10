# 188 as built: W-AGENT (General, Models, Agent)

Branch `feat/188-agent`, base `d02f4959`. UI only; no setting added.

## Tabs

| Tab | Component | Holds |
|---|---|---|
| General › Appearance | `Appearance.tsx` (new) | Interface: Theme, Serif prose on boards · Chat: Collapse tool calls by default (from Chat › Tool calls), Show theuth nudges (from Agent loop › Context) |
| General › App | `App.tsx` (was `General.tsx`) | Desktop app (desktop only): Open at login, App version with Check for updates / Install and restart · Node: Data directory, Documents, Update (text unchanged, `t --update` kept) · Connection with Copy diagnostics |
| General › Shortcuts | `Shortcuts.tsx` | unchanged (W-VOICE) |
| Models › Defaults | `ModelDefaults.tsx` (new, from `Chat.tsx`) | New sessions, Subagents, theuth agent |
| Models › Helpers | `Helpers.tsx` (new, from `Loop.tsx`) | Title model, Vision fallback model, Escalation model |
| Models › Providers | `ModelProviders.tsx` (was `Connections.tsx`) | built-in rows, login-only rows, Local servers, Custom providers, AWS Bedrock › Profile |
| Agent › Loop | `Loop.tsx` | Model calls (five rows), Routines › Run time limit |
| Agent › Memory | `Memory.tsx` | Defaults (three endless switches + Default compact style), View, Compactor; `?focus=model\|spend` unchanged |
| Agent › Approvals | `Approvals.tsx` | Ask before risky tools only; no props |
| Agent › Advisor / Harnesses | `Advisor.tsx` / `Agents.tsx` | unchanged |
| Agent › Autoresearch | `Autoresearch.tsx` | Enabled is a Switch (F3); the rest unchanged |

## Removals and moves
- Removed: General › Node › **Name** row; General › **Toggle window ⌥Space** row; Approvals › **Ask in this chat**; Approvals › **Pending** section (its `theuth:approvals` event had no listener); Connections › **Accounts** section (provider logins now on provider rows; MCP logins go to Agents › Tools, W-CHAT).
- Chat › Talk › **Turn chime** deleted here; W-VOICE re-creates it in Voice › Microphone.
- `Chat.test.ts` → `ModelDefaults.test.ts` (the "Models before Tool calls" order check went with the Tool calls section).

## Decisions
- **`Chat.ts` stays**, holding only `SAME_AS_NEW`: `Meetings.tsx` (W-VOICE) and `agents/AgentDetail.tsx` import it from `@/components/settings/Chat`, which I may not edit. A re-export from a new home would be a shim.
- **File names.** `Providers.tsx` collides with `providers.ts` on a case-insensitive disk (TS1149), so the tab is `ModelProviders.tsx`; `ModelDefaults.tsx` matches it. `General.tsx` → `App.tsx` (export `App`).
- **Node version kept.** The removed Name row's description was the only place the node's `theuth <version>` showed; it now sits muted in the Node section's header (no row added).
- **Login merge** (`providers.ts`): `pairLogins(providers, logins)` puts each login on the provider row of the same name and returns the rest as `alone`; `loginText` is the old Accounts status text. Login-only rows render after the built-ins in the provider row style with the `built-in` badge. A paired row appends ` · <login state>` to its description, gets Log in / Log out beside its key button, and its dot is on when logged in. Logins are paired only once `list_providers` answered, so rows don't jump. Buttons read **Log in / Log out** as the brief words it (Accounts said Login / Logout). Logout invalidates logins + models (MCP list no longer, no MCP rows here).
- **Secret note** of a built-in key now says `(Settings > Models > Providers)`; it said `Connections`, a tab that no longer exists.
- **Shared model list:** Defaults and Helpers use the existing `useModels()` from `components/agents/queries` (same key and call) rather than a new module.
- **Autoresearch "running now" signal:** `autoresearch.mechanism {op: list}` via the existing `useMechanism()` (the Mechanism section on the same tab already runs it, so no extra request). It answers while the actor runs and fails with `NOT_RUNNING` (-32001) while the plugin is off; `autoresearch.list` answers `[]` either way, so it cannot tell. `awaitsRestart(enabled, mechanism)` in `Autoresearch.tsx` returns `true` (mark), `false` (clear) or `null` (do nothing: still loading or another error); tested in `Autoresearch.test.ts`.
- **Section titles:** Appearance is split into Interface and Chat; App into Desktop app, Node and Connection.

## Deviations
- None from what the brief decides. `Chat.tsx` was not deleted outright; see the first decision.

## Hand-off
- **W-VOICE:** re-create Turn chime (`useTalkSettings` › `cue` / `setCue`, "A soft two-tone chime when you stop talking and the model takes the turn.") in Voice › Microphone.
- **W-CHAT:** MCP logins (`mcp_login` / `mcp_logout`, `list_mcp` `auth`) are no longer anywhere in Settings. The `LoginFinished` toast now runs only while Models › Providers is mounted, so the Tools page needs its own listener for MCP sign-ins.
- **Owner of `agents/AgentDetail.tsx`:** two strings still say "Settings › Chat" (lines ~369, ~389); the picker now lives in Settings › Models › Defaults.
- **W-SHELL:** `slash.test.ts` still expects `tab: 'connections'` (that is `slash.ts`, yours; `MOVED_TABS` redirects it to `providers`).
