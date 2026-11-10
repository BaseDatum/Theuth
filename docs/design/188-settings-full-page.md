# 188 Settings: a full page with categories and tabs

Status: building (2026-11-10). Plan approved by Sky with rulings D1–D4 and all 12 removals.

## Sky's words

- 2026-11-10 21:39: "we really need to clean up and enhance the settings UI of theuth. right now contacts are in communication and in network, I want you to plan a categories column, and each selected column view has tabs for each item. organize it much better than it is and remove redundancy. make it EASY to navigate and understand, remove, do not add new settings. … I want it to be a full view not in a modal like it is now, it's too cramped in the modal."
- "all the settings that just say, enable in config, or this is set in config file should be modifyable in the settings, and not defer the user to editing a file."
- ~22:00: "voice and meetings can each be their own item, not merged. and calendar can also be it's own item. the point of this clean up is it should be 99% UI changes only, only the settings that have no path to change without editing a file should require further changes."
- 22:08: approved all 12 removals. 22:15: "ok build it, omp subagents, parallelize as much as possible. high quality clean code."

## Contract (C0, commit on `feat/188-contract`)

Every worker branches from C0. C0 changes no behaviour except that the old modal and the route are driven by the registry:

| File | What C0 puts there | Owner after C0 |
|---|---|---|
| `crates/desktop/ui/src/components/settings/types.ts` | `TabProps`, `SettingsTab` (with optional `visible()`), `SettingsCategory` | frozen (orchestrator) |
| `crates/desktop/ui/src/components/settings/registry.ts` | `CATEGORIES` (column order), `placeOf`, `isTab`, `MOVED_TABS` | W-SHELL (may add helpers; ids and order frozen) |
| `crates/desktop/ui/src/components/settings/categories/<id>.tsx` (10 files) | each category's tabs (id, label, icon, order) rendering today's components | the category's worker; it swaps each `Component` for its split one. Tab ids, labels, icons and order are frozen. |
| `crates/desktop/ui/src/lib/restart-pending.ts` | `useRestartPending` store: `mark(key, label)`, `clear(key)`, `reset()` | frozen |
| `crates/desktop/ui/src/components/settings/ConfigFlagRow.tsx` (C0b) | `useConfigFlag(key)` and `ConfigFlagRow({ flag, label, description })`: the switch for a `config_flag_*` key, its restart note and its banner entry | frozen; used by W-LEARN (decision, computer) and W-NET (a2a) |
| `crates/desktop/ui/src/rpc/api.ts` | `ConfigFlagKey`, `ConfigFlag`, `config_flag_get`, `config_flag_set` (interface + `METHODS`) | frozen; W-ENG implements the engine side |
| `crates/desktop/ui/src/components/settings/SettingsDialog.tsx` | the old modal, now listing `CATEGORIES` tabs and rendering `placeOf(tab).tab.Component` | W-SHELL (deletes it) |
| `crates/desktop/ui/src/routes/settings.$tab.tsx` | redirects through `MOVED_TABS`; unknown → `appearance` | W-SHELL |
| `crates/desktop/ui/src/routes/c.$session.tsx` | search params `messaging?: '1'` and `peer?: string` (validated, unused yet) | W-CHAT (only the search handling) |

Rules that follow from it:
- **No file outside `categories/*.tsx` imports a tab component.** A worker may rename, split or delete the components it owns, as long as its own category modules still compile.
- **Tab ids are URLs** (`/settings/<id>`), unique across categories, and fixed by C0: `appearance app shortcuts · models helpers providers · loop memory approvals advisor autoresearch harnesses · dreaming double decision · voice dictation voice-key voice-history · meetings detection live speakers templates · calendar briefs upcoming · machine machines devices contacts messaging vms · signal slack telegram · search mcp computer`.
- **`SettingsTab.visible`** hides a tab where it cannot work (a phone or web client with no local node: `hasLocal()` from `link/engines`). W-SHELL hides a category whose tabs are all hidden and redirects a hidden tab to the category's first visible tab, else `appearance`.
- **`TabProps.focus`** is the route's `?focus=`. Memory keeps `model | spend`. A tab may define its own focus values (Contacts: `access:<contact name>`).
- **Restart-needed rows** call `useRestartPending.getState().mark(key, label)` after a successful save whose effect waits for a restart. They call `clear(key)` when the saved value matches what the engine runs. Keys are the config keys (`double.enabled`, `autoresearch.enabled`, `decision.enabled`, `a2a.enabled`, `computer.enabled`). W-SHELL renders one banner from the store.
- **Engine flags (W-ENG).** `config_flag_get(key)` and `config_flag_set(key, on)` accept only `decision.enabled`, `a2a.enabled` and `computer.enabled`; any other key is an RPC error. `set` writes that one key into the engine's `config.yaml` through the same config-persistence path the engine already uses for its other settings writes, leaving every other key untouched. Both return `{ key, enabled, running, restart_required }`: `enabled` is the value now in config.yaml, `running` is the value this engine process started with, and `restart_required = enabled != running`.
- **Chat Messaging sheet (W-CHAT).** `/c/$session?messaging=1` opens that chat's Messaging… sheet; `&peer=<message id>` also focuses that pending peer request. W-SHELL redirects old `/settings/communication?session=<id>[&peer=<id>]` links there; without `session` they go to `/settings/messaging`.
- **Restart theuth** (W-SHELL) calls the desktop shell's existing `relaunch()` (`lib/tauri.ts`: "Stop the daemon we own (if any) and connect again"). It is shown only where a restart can actually happen; elsewhere the banner says to restart theuth where it runs.
- **Nothing new.** No new setting or config key, except the three writes above for keys that already exist.

## Work split

| Worker | Delivers | Owns (only it edits these) |
|---|---|---|
| W-SHELL | the full page: categories column + section band tabs + body; phone list → category with icon tabs; restart banner; route, redirects, legacy communication links; `SettingsDialog` deleted; entry points (`slash.ts` `settings()` + its tab-id type, sidebar, tray path) | `components/settings/{SettingsPage,SettingsDialog,registry}.*`, new shell files under `components/settings/page/`, `routes/settings.$tab.tsx`, `components/chat/slash.ts` (settings helper and its id type only), `components/sidebar/AppSidebar.tsx`, `lib/nav.ts`, `lib/tauri.ts` (comment), `components/settings/shared.tsx` |
| W-AGENT | General (Appearance, App), Models (Defaults, Helpers, Providers), Agent (Loop, Memory, Approvals, Advisor, Autoresearch, Harnesses) | `categories/{general,models,agent}.tsx`; `General.tsx`, `Chat.tsx`, `Loop.tsx`, `Connections.tsx`, `providers.ts`, `Memory.tsx`, `Approvals.tsx`, `Advisor.tsx`, `Autoresearch.tsx`, `Mechanism.tsx`, `Agents.tsx`, `SummaryTestCall.tsx` and their tests |
| W-VOICE | Voice (Microphone, Dictation, Voice key, History), Meetings (Defaults, Detection, Live, Speakers, Templates), Calendar (Calendars, Briefs, Upcoming), and the two meeting hotkeys in General › Shortcuts | `categories/{voice,meetings,calendar}.tsx`; `Voice.tsx`, `VoiceKey.tsx`, `Meetings.tsx`, `MeetingTemplates.tsx`, `Calendar.tsx`, `Shortcuts.tsx` and their tests |
| W-LEARN | Learning (Dreaming, Double, Decision models), Channels (Signal, Slack, Telegram), Tools › Web search and Computer use | `categories/{learning,channels,tools}.tsx`; `Dreaming.tsx`, `Double.tsx`, `Labs.tsx`, `Channels.tsx`, `Computer.tsx`, `Search.tsx`, `search-settings.ts` and their tests |
| W-NET | Network (This machine, Machines, Devices, Contacts, Messaging, Sandbox VMs) | `categories/network.tsx`; `Network.tsx`, `Link.tsx`, `Contacts.tsx`, `Communication.tsx`, `PairDialog.tsx`, `UnpairDialog.tsx`, `SandboxVms.tsx`, `SandboxSheets.tsx`, `sandbox-queries.ts` and their tests |
| W-CHAT | the chat menu's Messaging… sheet (root scope, alias, effective reason, peer requests) and its URL; MCP server home + login on Agents › Tools rows; `Mcp.tsx` without Tool servers | `components/chat/{SessionMenu,MessagePeer,Peers}.tsx`, new `components/chat/MessagingSheet.tsx`, `routes/c.$session.tsx` (search params only), `components/library/Tools.tsx`, `components/settings/Mcp.tsx` and their tests |
| W-ENG | `config_flag_get` / `config_flag_set` in the engine, tests, bindings | Rust only: `crates/rpc/src/api.rs` and the config crate or module that persists `config.yaml` |

The 12 approved removals and F1–F7 belong to the worker that owns the file they are in.

# The approved plan

## In numbers

| | Today | Plan |
|---|---|---|
| Where it opens | 880 × 640 modal over the app | Full page in the main window |
| Navigation | one flat list of 24 tabs | 10 categories, 39 tabs (at most 6 per category) |
| Rows that leave Settings without a new home | | 12: 3 switches that also live elsewhere, 9 read-only rows or pointers (the list below) |
| Settings removed entirely | | 0: every value stays changeable |
| Rows that send you to config.yaml | 6 flags + 4 wordings | 0 |
| Settings added | | 0 |
| Engine changes | | 3 writes: `decision.enabled`, `a2a.enabled`, `computer.enabled`. Everything else is UI. |

Coverage was checked against the code. All 95 sections of today's 24 tabs land somewhere: 78 keep or move, 10 go, 5 go to the chat menu, 2 go to Agents › Tools. Each of the 39 new tabs holds only settings that exist today.

## Rules

1. **One home per setting.** A setting appears in exactly one place. No read-only copies and no "see X" pointer rows.
2. **Settings is global.** Anything that belongs to one chat lives in that chat's menu.
3. **A category is what you would look for; a tab is one item.** No category has more than 6 tabs, and every tab has its own URL.
4. **Nothing sends you to a file.** Every flag the UI describes as "in config.yaml" becomes a control.
5. **Nothing new.** Moves, merges and removals only.
6. **UI only.** The engine changes only where no write path exists today: three flags.

## What leaves Settings (all 12 approved 2026-11-10)

| # | Row in Settings today | What it is | Where the same thing stays |
|---|---|---|---|
| 1 | Approvals › **Ask in this chat** | switch (per chat) | The chat's menu › Approvals (default / on / off), the same call. Covered by your D1 answer. |
| 2 | Communication › **This chat may communicate** | switch (per chat) | The chat's menu › Messaging checkbox, the same call. Covered by D1. |
| 3 | Dreaming › **Enabled**, which today is a copy of the dreaming *routine's* switch | switch | Agents › Routines, on the dreaming row. In Settings this row becomes the dreaming *plugin* switch (`dreaming.enabled`), which already turns the routine and sensor on and off with it. |
| 4 | General › Node › **Name** | read-only text | Network › This machine, where the name is editable |
| 5 | General › **Toggle window "⌥Space · Fixed in this version"** | read-only, and wrong | General › Shortcuts › System-wide, which rebinds it |
| 6 | Voice key › **Keys** | read-only chord summary with a link | General › Shortcuts (Voice key, Dictate) |
| 7 | Network › **Agent messaging** (On/Off) | read-only mirror | Network › Messaging › Let sessions message each other |
| 8 | Communication › Network (A2A) › **contact rows** | read-only second contacts list | Network › Contacts |
| 9 | Communication › **"No peers or exports yet · Contacts…"** | pointer button | the Contacts tab sits next to it |
| 10 | Meetings › **"Local decision oracle: state · Change in Labs"** | status line with a pointer | Decision models › Meeting oracle, which shows the same state with its control |
| 11 | Approvals › **Pending "N waiting · Review"** | queue count with a button | Attention, and the chat header's approvals sheet |
| 12 | Contacts' access › **Contacts…** button | pointer | the contacts list, which is on the same tab now |

No setting is removed entirely: every value above stays changeable where the last column says. Three links inside Contacts ("Choose what X may reach…", the add-contact flow's access step, and the chat's share dialog) are retargeted to the Contacts tab, not removed.

## Layout

- A full page at `/settings/<tab>`. The app sidebar stays and its Settings row lights up. Back works like on any page. There is no backdrop and no Esc-to-close.
- **Left:** the categories column: General, Models, Agent, Learning, Voice, Meetings, Calendar, Network, Channels, Tools.
- **Top:** the category's tabs, in the same 52 px band that Agents and Library use. The tabs are the title, so there is no title bar, and they span the width.
- **Body:** one readable column (about 640 px) built from the same Section and Row components as today.
- **Phone:** the category list comes first. Tapping a category opens it with icon-only tabs and a back chevron, as elsewhere in the app.

## New structure

| Category | Tab | What it holds |
|---|---|---|
| **General** | Appearance | Theme · Serif prose on boards · Collapse tool calls by default *(from Chat › Tool calls)* · Show theuth nudges *(from Agent loop › Context)* |
|  | App | Open at login · App version · Node update · Data directory · Documents helper · Connection |
|  | Shortcuts | Keyboard shortcuts · Composer · System-wide · Eye hotkey *(from Meetings › Screen capture)* · Push-to-talk hotkey *(from Meetings › Push-to-talk)* |
| **Models** | Defaults | New sessions *(from Chat › Models)* · Subagents *(from Chat › Models)* · theuth agent *(from Chat › Models)* |
|  | Helpers | Title model *(from Agent loop › Helper models)* · Vision fallback model *(from Agent loop › Helper models)* · Escalation model *(from Agent loop › Helper models)* |
|  | Providers | Built-in providers *(duplicate merged)* · Local servers · Custom providers · AWS Bedrock profile |
| **Agent** | Loop | Stream idle timeout · Reply recoveries per turn · Tool rounds per turn · Subagent timeout · Terminal idle wake · Routine run time limit |
|  | Memory | New chats start endless · Desks start endless · Meetings start endless · Default compact style *(from Agent loop › Context)* · View size · Slack · Settle seconds · Compactor model · Guards · Speed & spend |
|  | Approvals | Ask before risky tools |
|  | Advisor | Default effort · Notes per update · Immune turns · Catch-up · Consults per turn |
|  | Autoresearch | Enabled **(now editable)** · Budget per study · per experiment · Baseline repeats · Timeouts · Models · Review and the machine · Mechanism |
|  | Harnesses | Installed · Registry |
| **Learning** | Dreaming | Status · Enabled **(now editable)** · Window · Budget per night · Models · Auto-promote preferences · Weekly digest · Pause · Recent nights · Proposals |
|  | Double | Enabled **(now editable)** · Pause learning · Active version · Last run · Next window · Approvals off · Improvement · Classes · Training · Training log |
|  | Decision models | Enabled **(now editable)** · Loaded now · Meeting oracle · Browser System-1 · Training data |
| **Voice** | Microphone | Input device · Turn chime *(from Chat › Talk)* |
|  | Dictation | Shaping · End phrases · Polish model |
|  | Voice key | Voice key · Send the screen · Read what the pointer touched · Draw the trail · Panel position · Hints |
|  | History | History · Last dictation |
| **Meetings** | Defaults | Meeting model · Summary model · Template · Transcript language · Default project |
|  | Detection | Detect meetings from the microphone · Ignored apps |
|  | Live | Echo in your microphone · Allowed while a meeting runs · Hidden from capture · Live speaker labels |
|  | Speakers | Enrolled speakers |
|  | Templates | Templates |
| **Calendar** | Calendars | Use macOS Calendar · Subscriptions |
|  | Briefs | Pre-meeting briefs |
|  | Upcoming | Upcoming |
| **Network** | This machine | Name *(duplicate merged)* · Let nearby machines find this one while pairing · Mesh name *(from Network › Advanced)* · Prefer this node as hub *(from Network › Advanced)* · Sessions · Replication log *(from Network › Advanced)* · Rotate vault key *(from Network › Advanced)* |
|  | Machines | Machines in this mesh · Nearby · Pair by string *(from Network › Advanced)* · On another computer over SSH *(from Network › Advanced)* |
|  | Devices | Devices paired with this mesh · Relay · This node · This mesh · Legacy devices *(from Link › Legacy devices + Relay › Node name)* · Meshes this app reaches *(from Network › Other meshes this app can reach)* |
|  | Contacts | Contacts *(duplicate merged)* · Access *(from Communication › Contacts' access)* |
|  | Messaging | Let sessions message each other *(duplicate merged)* · A2A over HTTP **(now editable)** · Listener · Problem · HTTP peers · Exports |
|  | Sandbox VMs | Sandbox VMs |
| **Channels** | Signal | Account · Link this device · Owner |
|  | Slack | Bot · Owner · App manifest |
|  | Telegram | Bot · Bot token · Owner · Pair the owner |
| **Tools** | Web search | Providers · Pages read · Read budget · Render cap · Default recency · Keyless engines as fallback only · Treat degraded as error · Status |
|  | Serve over MCP | theuth as an MCP server · Port · Token · Copy MCP config · Recent calls |
|  | Computer use | Enabled **(now editable)** · Displays · Permissions |

## Today → where each part goes

| Today's tab | Where each part goes |
|---|---|
| General | General: Theme · General: Serif prose on boards → General › Appearance<br>General: Open at login · General: App version / updates · Node: Data directory · Node: Documents · Node: Update · Connection: diagnostics → General › App<br>General: Toggle window ⌥Space (fixed) → **removed** (see list)<br>Node: Name (read-only) → **removed** (see list) |
| Chat | Models: New sessions · Subagents · theuth agent → Models › Defaults<br>Tool calls: Collapse tool calls by default → General › Appearance<br>Talk: Turn chime → Voice › Microphone |
| Agent loop | Model calls: five limits · Routines: Run time limit → Agent › Loop<br>Context: Default compact style → Agent › Memory<br>Context: Show theuth nudges → General › Appearance<br>Helper models: Title · Vision fallback · Escalation → Models › Helpers |
| Advisor | Advisor: five limits → Agent › Advisor |
| Dreaming | Dreaming: status · Window · Budget per night · Models per tier · Preferences and digest · Recent nights · Proposals → Learning › Dreaming<br>Enabled (a copy of the routine's switch) → **removed** (see list) |
| Autoresearch | Autoresearch: Enabled · budgets · repeats · timeouts · Models per role · Review and the machine · Mechanism → Agent › Autoresearch |
| Approvals | Ask before risky tools → Agent › Approvals<br>Ask in this chat → **chat menu**: already there: Approvals default / on / off<br>Pending: N waiting · Review → **removed** (see list) |
| Communication | Let sessions message each other · Network (A2A): listener · problem · reload · Network (A2A): HTTP peers · exports · held first messages → Network › Messaging<br>This chat may communicate → **chat menu**: already there: the Messaging checkbox<br>Effective · Open ancestor → **chat menu**: the menu already shows the effective state; its reason line moves with it<br>Root scope: Who may discover this tree · Alias → **chat menu**: new Messaging… sheet in the chat menu, root chats only<br>Network (A2A): contact rows → **removed** (see list)<br>Network (A2A): No peers or exports yet → Contacts… → **removed** (see list)<br>Peer requests (this chat) → **chat menu**: same Messaging… sheet; the peer-message card's link opens it<br>Contacts' access (rows) → Network › Contacts<br>Contacts' access: Contacts… button → **removed** (see list) |
| Memory | Defaults: three endless switches · View: size · slack · settle · Compactor · Speed & spend → Agent › Memory |
| Computer | Displays · Permissions → Tools › Computer use |
| Meetings | Defaults → Meetings › Defaults<br>Detection → Meetings › Detection<br>Echo · Live tools · Screen capture: Hidden from capture · Live transcript → Meetings › Live<br>Screen capture: Eye hotkey · Push-to-talk hotkey → General › Shortcuts<br>Local decision oracle: status · Change in Labs → **removed** (see list)<br>Enrolled speakers → Meetings › Speakers<br>Templates → Meetings › Templates |
| Calendar | macOS Calendar · Subscriptions → Calendar › Calendars<br>Pre-meeting briefs → Calendar › Briefs<br>Upcoming → Calendar › Upcoming |
| Voice | Microphone: Input device → Voice › Microphone<br>Shaping · End phrases · Polish → Voice › Dictation<br>History · Debug (last dictation) → Voice › History |
| Voice key | Voice key · screen · pointer · trail · panel · hints → Voice › Voice key<br>Keys (chord summary) → **removed** (see list) |
| Search | Providers · Defaults · Status → Tools › Web search |
| Labs | Loaded now · Meeting oracle · Browser System-1 · Training data → Learning › Decision models |
| Double | Status · Improvement · Classes · Training · Training log → Learning › Double |
| Harnesses | Installed · Registry → Agent › Harnesses |
| Network | This machine · Advanced: mesh · hub · sessions · replication · rotate key → Network › This machine<br>Nearby · Machines in this mesh · Advanced: pairing string · paste · over SSH → Network › Machines<br>Contacts → Network › Contacts<br>Agent messaging (read-only) → **removed** (see list)<br>Sandbox VMs → Network › Sandbox VMs<br>Other meshes this app can reach → Network › Devices |
| Link | Devices paired with this mesh · Relay: switch · address · this node · this mesh · Relay: Node name (legacy) · Legacy devices · Pair with a mesh (phone / web) → Network › Devices |
| Connections | Accounts: provider logins · Providers · AWS Bedrock → Models › Providers<br>Accounts: MCP server logins → **Agents › Tools page**: on each server's row, next to its gate |
| Channels | Signal → Channels › Signal<br>Slack → Channels › Slack<br>Telegram → Channels › Telegram |
| Shortcuts | Keyboard shortcuts · Composer · System-wide → General › Shortcuts |
| MCP | Tool servers: where each runs → **Agents › Tools page**: on each server's row, next to its gate<br>MCP server · Recent calls → Tools › Serve over MCP |

## Moves out of Settings (not removals)

| Today | New home |
|---|---|
| Communication › Effective · Open ancestor | Chat menu: it already shows the effective state; the reason line moves with it |
| Communication › Root scope: Who may discover this tree · Alias | Chat menu › **Messaging…** sheet (root chats only) |
| Communication › Peer requests (this chat) | Same Messaging… sheet; the peer-message card's link opens it |
| MCP › Tool servers (which machine runs each server) | Agents › Tools, on each server's row next to its gate (D2) |
| Connections › Accounts (MCP server logins) | Agents › Tools, on each server's row (D2) |

Inside Settings, two more pieces move without losing anything:
- Provider logins from Connections › Accounts join their provider rows in Models › Providers.
- Link › Relay › **Node name** is the name legacy devices see, a separate value from the machine name. It moves to Network › Devices › Legacy devices and shows only when legacy devices exist.

## "Set it in config.yaml" becomes a control

| | Where | Today | Becomes | Work | Takes effect |
|---|---|---|---|---|---|
| F1 | Learning › Double › Enabled | a status dot and "`double.enabled` is off … Turn it on in config.yaml and restart." | Switch | **UI only**: `double_settings_set` already persists `enabled` | after restart |
| F2 | Learning › Dreaming › Enabled | a copy of the routine switch, plus "dreaming.enabled is off in config" | Plugin switch | **UI only**: `dreaming.set_settings` already persists `enabled`; off disables the routine and removes the sensor, on restores both (verified in `dreaming/src/night.rs`) | at once |
| F3 | Agent › Autoresearch › Enabled | read-only "on/off" and "Set in config.yaml (autoresearch.enabled)" | Switch | **UI only**: `autoresearch.settings.set` persists the whole section and works while the plugin is off | after restart (the plugin starts then) |
| F4 | Learning › Decision models (was Labs) | the whole tab is hidden while `decision.enabled` is false | Tab always listed, with an Enabled switch | **engine**: a write for `decision.enabled` (today `set_decision_mode`, `set_decision_training` and `set_decision_share` only write per-family values) | after restart |
| F5 | Network › Messaging › A2A over HTTP | "a2a.enabled is false in config.yaml … (enabling it needs a restart)" | Switch | **engine**: a write for `a2a.enabled` (today only `reload_a2a`) | after restart |
| F6 | Tools › Computer use › Enabled | "Computer control is disabled (`computer.enabled: false`)", with no switch | Switch | **engine**: a write for `computer.enabled` (today only `computer_permissions`) | check in build |
| F7 | wording only | Approvals "saved to the config file"; Messaging "Saved to config.yaml" / "From config.yaml" / "`always` writes config.yaml"; Contacts "pin its risk in mcp/config.yaml"; Double's tier C tooltip names `double.training.tiers.c.enabled` | No file names. The MCP note links to the gate control in Agents › Tools. | **UI only** | |

**Restart handling (UI only).** A row whose change needs a restart says "Applies after theuth restarts". One banner at the top of the page collects every pending change and has a **Restart theuth** button. The button calls the desktop shell's existing `relaunch`, which stops the daemon the app owns and connects again. When the app does not own the daemon (a service install or a remote node), the banner shows without the button.

**Environment override.** When `THEUTH_AGENT__COMMUNICATION` is set, the messaging switch still shows one line saying it is overridden at startup. That describes real state rather than pointing you to a file, so it stays.

## URLs and deep links (UI only)

- The route stays `/settings/$tab`. Tab ids are unique and the category is derived from the tab. `voice`, `voice-key`, `meetings` and `calendar` keep their ids and open the first tab of their category.
- Old ids redirect, the same way `mesh → network` does today:

| Old id | New id |
|---|---|
| general | appearance |
| chat | models |
| connections | providers |
| agents | harnesses |
| labs | decision |
| network, mesh | machine |
| link | devices |
| channels | signal |
| communication | messaging; with `?session` / `?peer`, the chat's Messaging… sheet |

- These keep working: `memory?focus=model|spend` from the chat's Memory panel, the engine switcher's Network item, the harness picker, the tray's Settings item and the `/settings` slash command.

## Build order (when you say go)

1. **Shell (UI).** A `SettingsPage` (categories column + section band) replaces `SettingsDialog`'s Dialog/Sheet, and the redirects go in. The existing components render unchanged inside their new tabs.
2. **Split, move and remove (UI).** Split the files that feed several tabs (Communication, Network, Link, Connections, General, Chat, Loop, Voice, Meetings, Calendar) by tab, and delete the 12 approved rows in the list above.
3. **Per-chat move (UI).** Add the chat menu's Messaging… sheet and point the peer-message card's link at it.
4. **MCP rows (UI).** Move each server's home and login onto Agents › Tools.
5. **Config flags.** F1–F3 and F7 are UI only, and so is the restart banner (it uses `relaunch`). F4–F6 are the only engine changes: three small writes.
6. **Proof.** Every old URL and deep link lands; screenshots of every category at desktop and phone widths; the Network, Contacts, Channels and Memory tests updated.

## Decisions

Answered 2026-11-10:

| | Decision | Your answer |
|---|---|---|
| D1 | Per-chat rows | Move them to the chat's own menu |
| D2 | MCP server homes and logins | Agents › Tools rows |
| D3 | Feature model pickers | Stay with their feature |
| D4 | Node update line (`t --update`) | Leave it |

Removal list: all 12 rows approved, 2026-11-10. Nothing is open; the build waits for your go.