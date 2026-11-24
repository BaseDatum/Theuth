# 215 — First-run onboarding (desktop)

Status: contract, 2026-11-24. Branch `feat/onboarding`, worktree `/Users/sky/Develop/skymoore/theuth-onboarding`.

## Goal

A new desktop user goes from "installed" to "my agent did something useful" in under three minutes, and leaves knowing where everything lives: models, devices, inbox, chat, costs/subscription, library. Hand-holding, value first, skippable at every step, restartable.

## Shape (four layers)

1. **Welcome + setup wizard** — a full-window dialog on first run: Welcome → Connect a model → Pick default model → First chat (starter prompts that send immediately).
2. **Guided tour** — driver.js spotlight steps anchored on `[data-tour="…"]` attributes in the real UI, run while the first answer streams: Inbox, Library, Costs, Agents, Settings › Devices, the chat composer.
3. **Getting-started checklist** — a compact card in the sidebar footer, items auto-completing from live engine state, each with a "Show me" that runs a focused mini-tour or deep-links. Dismissable; reopenable.
4. **Value surfaces** — a "Subscription value" card on Costs (what your subscription usage would have cost at API rates), a Library overview strip (what is in each tab, with counts), and an Inbox empty-state explainer.

Library: **driver.js ^1.14** (MIT, 0 deps, ~15 kB gz, `waitForElement`, `advanceOn`). Shepherd/Intro.js are AGPL — never add them. Welcome/wizard/checklist are hand-built with the existing shadcn kit (`src/components/ui/*`), tokens from `src/styles.css` (`--bg-*`, `--fg-*`, `--fg-accent`, `--border-*`). No emoji anywhere (ratchet `lib/no-emoji.test.ts`); use lucide icons.

## Trigger and persistence

- Engine flag: new `ConfigFlagKey` variant **`onboarding.completed`** (config.yaml `onboarding.completed`, default `false`), read/written with the existing `config_flag_get` / `config_flag_set`. Set `true` when the user finishes OR skips the wizard.
- Client state (step, tour progress, checklist dismissed, visited sections): `localStorage` key **`theuth-onboarding-v1`** (JSON), zustand store. When the engine does not support the flag (capability check fails), localStorage `completed` is the fallback.
- Decide on boot (desktop shell with local engine only — `hasLocal()`; never in pop-outs, island, overlay, phones, web clients):
  - flag `true` → nothing modal; checklist shows unless dismissed or all done.
  - flag `false` AND (`list_models()` empty OR default model's provider unconfigured) → open the wizard at Welcome.
  - flag `false` AND models configured (upgrading user) → no modal; a single sonner toast "New: a 2-minute tour of theuth" with action "Take the tour" / "Dismiss" (dismiss sets the flag).
- Restart: Settings › General gets a row "Onboarding — Restart the welcome tour" (button) which resets local state and opens the wizard at Welcome; also a `/onboarding` slash command if the slash registry makes that trivial.

## Wizard steps (exact content is the builder's craft; these are the requirements)

1. **Welcome.** Headline + three one-line value props (agents that act on your machine; everything they make lands in your Library; you approve what matters from your Inbox). Primary "Get set up" (time hint "about 2 minutes"), secondary "Skip — I know theuth". Step dots.
2. **Connect a model.** Option cards, recommended first:
   - "Claude Pro / Max" → `login("anthropic")`; "ChatGPT Plus / Pro" → `login("openai")`; also xai/cursor under "More subscriptions". Card shows spinner "Finish in your browser…" until `agent.login@1` `LoginFinished` arrives (error → inline message + retry). Explain subscriptions in one line: "use your plan, no per-token bill".
   - "API key" → provider select (Anthropic, OpenAI, OpenRouter, OpenCode, Google) + masked input → `set_secret(<adapter lower name>, key, note)`; then refetch `list_models`.
   - "Local model" → `local_servers_probe()` on open; each found server is a one-click `add_provider(name, base_url, null, runtime)`. None found → link text to ollama.com.
   - Already configured (list_models non-empty) → show a success state listing connected providers, "Continue".
   - Continue disabled until `list_models()` is non-empty; "Skip for now" always available.
3. **Pick your default model.** 3–5 recommended models from `list_models()` (prefer the provider just connected; show name, provider, context size) → `set_model(null, id, true)`. Link "See all models" opens the existing picker.
4. **Your first chat.** Explain chat in one line, then 4 starter cards that show the harness's reach (each a real prompt, sent with `send_input(null, text, [], null, null)` and navigate to `/c/$session`):
   - "Tour this Mac" — what is on this machine, disk, big folders, as a picture.
   - "Make me a page" — a one-page HTML dashboard of today's weather/calendar-free content (no accounts needed).
   - "Research something" — web research with sources on a topic the user types (card has an inline input).
   - "Explain a project" — pick a folder (Tauri dialog if available, else path input) and get a board/overview.
   - plus "Write my own" → focuses the normal composer at `/new`.
   After sending: close the wizard, mark wizard done (flag true), start the **main tour** once the chat view mounts (`waitForElement`).
5. (Optional, offered after tour, also a checklist item) **Link your phone.** Dialog with `device_pair_start(null)` QR (`QRCodeSVG`) + copyable string + countdown + note that the iOS app currently needs the string pasted; success on `link.events@1` `LinkPaired`. Secondary: "Another computer" → deep link `settings(navigate,'machines')`; "Slack / Telegram / Signal" → deep link channels.

## Main tour (driver.js), ids are `TourId = 'main' | 'inbox' | 'library' | 'costs' | 'devices' | 'chat'`

`main` steps, each anchored on a `data-tour` attribute:
`chat-composer` (you can keep typing while it works; steer mid-turn) → `nav-inbox` (approvals, questions, finished work land here; badge counts) → `nav-library` (artifacts, knowledge, skills, notes, notebooks) → `nav-costs` (spend + subscription limits + API-rate value) → `nav-agents` (standing agents, tasks, routines) → `nav-settings` (providers, devices, channels) → `onboarding-checklist` (keep going here). Last step "Done".
Focused tours: `inbox`, `library`, `costs` navigate to the route then highlight in-page anchors (`inbox-list`, `library-tabs`, `costs-limits`, `costs-subscription-value`); `devices` opens Settings › Devices and highlights `devices-pair`; `chat` highlights composer, model menu (`chat-model`), context meter.
Tour rules: skip a step whose element is absent after `waitForElement: 4000` (capability-hidden rows); Esc/close = skip; keyboard on; popover class `theuth-tour` styled from tokens, light + dark; progress text "2 of 7"; never start in pop-outs.

## Checklist items (ids fixed), auto-complete source

| id | label | done when |
|---|---|---|
| `model` | Connect a model | `list_models()` non-empty |
| `chat` | Start your first chat | any session exists (sessions store non-empty) |
| `inbox` | Check your Inbox | user visited `/attention` |
| `costs` | See your costs and subscription | visited `/costs` |
| `library` | Look inside your Library | visited any library route |
| `phone` | Link your phone | `devices_list()` has a non-revoked device whose platform is not `theuth` |
| `channel` | Reach theuth from Slack, Telegram or Signal (optional) | any of slack/telegram/signal status connected/registered |

Card: title "Get started", progress "3 of 7", collapsible, "Hide" (sets dismissed). All done → a one-time completion state, then auto-hide.

## Value surfaces

- **Costs › Subscription value card** (`data-tour="costs-subscription-value"`): for each provider with a logged-in OAuth login (`list_logins()` `logged_in`) and no own API key, sum `cost_cube` cells for that provider with basis `estimated` in the current month (and last 30 days) → "Your Claude subscription covered $X of usage at API rates this month" + the plan/limit windows from `usage_limits()` for that provider. If no subscription is connected, a one-line explainer. Pure client-side derivation; computation lives in a tested pure function.
- **Library overview strip** at the top of the Library pages (only while the checklist is not dismissed OR a library tab is empty): one tile per tab with icon, one-line purpose, count (`list_artifacts`, `knowledge_list`, `list_skills`, notes, notebooks — whatever is cheaply available; capability-gated).
- **Inbox empty state**: replace/augment `ATTENTION_EMPTY` with a short explainer of what arrives here and why.

## File ownership (builders must not touch each other's files)

- **B-engine** (omp): Rust only. `ConfigFlagKey::OnboardingCompleted` in `crates/rpc-compose` (wire name `"onboarding.completed"`, config section `onboarding`, key `completed`, default false), its tests, the wire snapshot fixture (`just wire-schema` then `just wire-schema-accept`), regenerated UI bindings copied into `crates/desktop/ui/src/rpc/bindings` (`just ts-desktop`). Docs: add a line to the config docs if a config reference exists.
- **B-wizard** (omp): `crates/desktop/ui/src/lib/onboarding/state.ts` (+ test), `lib/onboarding/detect.ts` (+ test), `src/components/onboarding/{OnboardingHost,Wizard,steps/*,LinkPhoneDialog}.tsx`, mount `<OnboardingHost/>` in `src/routes/__root.tsx` `Root()`, Settings › General restart row, `package.json` (driver.js dependency added by B-wizard only).
- **B-tour** (omp): `lib/onboarding/tours.ts` (+ test for step definitions), `lib/onboarding/checklist.ts` (+ test), `lib/onboarding/subscription-value.ts` (+ test), `src/components/onboarding/{Checklist,tour.css}`, `data-tour` attributes in `AppSidebar.tsx`, `Composer.tsx`, `attention/Page.tsx`, `costs/*`, `library/*`, `settings/Link.tsx`; the Subscription value card, Library overview strip, Inbox empty state.

## Shared interface (B-wizard implements in `lib/onboarding/state.ts`; B-tour imports only these)

```ts
export type ChecklistId = 'model'|'chat'|'inbox'|'costs'|'library'|'phone'|'channel'
export type TourId = 'main'|'inbox'|'library'|'costs'|'devices'|'chat'
export type WizardStep = 'welcome'|'model'|'default-model'|'first-chat'
export interface OnboardingState {
  wizardOpen: boolean; step: WizardStep
  completed: boolean            // mirror of engine flag (or local fallback)
  checklistDismissed: boolean
  visited: Partial<Record<'inbox'|'costs'|'library', true>>
  pendingTour: TourId | null    // tour to start once its first anchor mounts
  openWizard(step?: WizardStep): void
  closeWizard(): void
  setStep(s: WizardStep): void
  markVisited(k: 'inbox'|'costs'|'library'): void
  dismissChecklist(): void
  showChecklist(): void
  requestTour(id: TourId): void // sets pendingTour
  clearPendingTour(): void
  reset(): void
}
export const useOnboarding: import('zustand').UseBoundStore<import('zustand').StoreApi<OnboardingState>>
export function useOnboardingCompleted(): { completed: boolean; setCompleted(v: boolean): Promise<void> }
```
B-tour implements in `lib/onboarding/tours.ts`: `export function startTour(id: TourId, navigate: NavigateFn): void` and a `<TourRunner/>` component (in `components/onboarding/TourRunner.tsx`, owned by B-tour) that watches `pendingTour` and runs it; B-wizard mounts `<TourRunner/>` and `<Checklist/>` inside `OnboardingHost` (sidebar footer slot for the checklist: B-tour places `<Checklist/>` in `AppSidebar` footer directly instead — B-wizard does NOT mount Checklist). `markVisited` is called by B-tour from the route components it owns.

## Rules for every builder

- Work only in the worktree; path-scoped commits (`git add <your paths>`); never `git stash`, `reset`, `checkout`, `rebase`. `commit.gpgsign` is false in this worktree.
- Heavy commands (cargo, bun run check/test/build, vitest, vite) only inside `until mkdir /tmp/theuth-build.lock 2>/dev/null; do sleep 10; done; trap 'rmdir /tmp/theuth-build.lock' EXIT; …` in one bash invocation; `CARGO_BUILD_JOBS=4`, `CARGO_INCREMENTAL=0`, crate-scoped only (`-p`), never workspace-wide. Machine load is high (16 cores shared).
- Tests are `src/**/*.test.ts` (no jsdom): put logic in pure functions and test those; components may be tested with `renderToStaticMarkup`. Keep ratchets green: `lib/no-emoji.test.ts`, `components/disclosure.test.ts` (register any new local open/closed state per its rules), `settings/registry.test.ts`.
- Capability-gate every RPC use (`useQuerySupport`, `Capability`); degrade silently when missing.
- Accessible: dialogs via the Radix `Dialog`, focus trapped, Esc closes (= skip with confirm-less skip), all controls keyboard reachable, visible focus.
