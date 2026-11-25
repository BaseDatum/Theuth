# 217 — Status bar replaces toasts

Owner ruling (2026-11-24): eliminate every toast. Desktop gets a thin bottom status bar ("A, quiet ledger", plus B's click-for-history). The phone gets one centered line under the composer, in the home-indicator band. Rates are formatted "1.5k ev/s". Mock: theuth artifact `statusbar-mock.html`.

Goal: valuable information with minimal visible disruption. Nothing transient ever covers the UI.

All paths are under `crates/desktop/ui/src/` unless stated otherwise.

## 1. Notice API — `lib/notify.ts` (drop-in for sonner's `toast`)

```ts
export type NoticeKind = 'info' | 'done' | 'warn' | 'error' | 'progress'
export type NoticeAction = { label: string; onClick: () => void }
export type NoticeOpts = {
  id?: string | number          // same id replaces in place (progress → done/error)
  description?: string          // second line; shown in history, truncated in the slot
  duration?: number             // ms, capped at 10 s in the slot; Infinity = the 10 s hold, then history
  action?: NoticeAction
  cancel?: NoticeAction         // second button (sonner compat)
}
export function notify(text: string, o?: NoticeOpts): string | number   // kind 'info'
notify.message  // = notify
notify.info     // 'info'
notify.success  // 'done'
notify.warning  // 'warn'
notify.error    // 'error'
notify.loading  // 'progress' (holds the slot until replaced by same id or dismissed; 30 s without an update ends it)
notify.dismiss  // (id?: string | number) => void   — no id = dismiss current
```

The property names must be exactly these, because existing tests mock `{ error, message, success, ... }` by name. `toast.promise` and `toast.custom` are unused and are not provided.

## 2. Store — `store/notices.ts` (global zustand, not per engine)

```ts
type Notice = { id: string | number; kind: NoticeKind; text: string; description?: string;
  action?: NoticeAction; cancel?: NoticeAction; at: number; sticky: boolean; expiresAt: number }
useNotices: { current: Notice | null; queue: number /* newer-than-shown count not needed */;
  history: Notice[] /* newest first, cap 50 */; unreadErrors: number;
  push(n), dismiss(id?), clear(), markRead() }
```

Policy:
- **Slot shows one notice:** the newest. A new notice replaces the current one, and the replaced one stays in history. It never returns to the slot.
- **Nothing lingers (owner ruling 2026-11-25):** the bar shows the current status at all times, so every notice leaves the slot on its lifetime: info 4 s, done 6 s, warn 8 s, error 8 s. A notice with an `action` or `duration: Infinity` holds the slot for 10 s, the cap for any explicit `duration`. After that it stays in history with its action (and `cancel`) still clickable; `unreadErrors` and the chip keep errors in sight.
- **Progress is current state:** a `progress` notice holds the slot while it is in progress, until replaced by the same id or dismissed, and leaves it after 30 s without an update.
- **Held notices** (an action, `duration: Infinity`, progress) carry a × (in the slot and in history) until dismissed or acted on.
- **Same `id`** replaces in place: it updates text, kind and lifetime, and does not create a second history row.
- **Dedupe:** an identical text+kind within 2 s bumps the time and does not add a row.
- **`unreadErrors`** counts errors pushed since the history was last opened. Opening the history calls `markRead()`.
- **No `setTimeout` per notice in the store:** a single interval or timer in the host component expires `current` (testable with fake timers).

## 3. Event rate — `store/event-rate.ts`

- **Counting:** count every bus event in the `subscribe` callback of `attach()` in `App.tsx` (one `tick(n)` per event, or per drained batch length). Do not count in `dispatch`.
- **Buckets:** a ring of 30 one-second buckets for the active engine. `reset()` on engine switch.
- **Readout:** `rate()` is the mean of the last 2 complete buckets. The UI re-renders at most once per second.
- **`formatRate(n)`:** `0`–`999` → integer (`"38"`); `1000`–`9999` → one decimal plus `k` (`"1.5k"`, with `"1.0k"` trimmed to `"1k"`); ≥ 10000 → integer plus `k` (`"15k"`). The label is `"<n> ev/s"`. When disconnected the label is `"– ev/s"`.

## 4. Connection

- **Source:** `useConnection(s => s.state)` for the active engine, mirrored from the `RpcClient` by `mirror()` (`store/connection.ts`). For link engines, use `useEngines(s => s.link[id])`.
- **Live, not last-event (owner ruling 2026-11-25):** the bar shows `liveState()` (`components/status/model.ts`): any frame received on the active client in the last 5 s (`RpcClient.lastRx`) reads as `open`, whatever the last state event said. So "disconnected" and the reconnect text show only when the client says so and nothing has arrived for 5 s. `open` clears `retryAt`/`attempt` in the store unconditionally; a window becoming visible again re-reads `RpcClient.state`/`retry` into the store; a relaunch resets the store to `connecting`.
- **Local retry deadline:** expose `retryAt: number | null` from `RpcClient.#schedule` (`rpc/client.ts`) through the connection store, plus `retryNow()`, which cancels the backoff and connects immediately. An `open` cancels a pending redial. Link engines already have `LinkState.retryAt`.
- **Dot states:**
  - `open` and needs-you = 0 → malachite `--status-success-fg`, pulsing slowly while the rate is > 0, steady at 0.
  - `open` and needs-you > 0 → ochre `--status-warning-fg`.
  - `connecting` → `--fg-tertiary`, pulsing.
  - `reconnecting` / `closed` (live, above) → carnelian `--status-danger-fg`, the label "<engine> disconnected", and in the middle slot "Reconnecting in Ns · attempt k · Retry now". The bar gets a faint danger tint.
- **The dot never reflects a notice's kind:** an error notice does not turn the dot red.
- **The connection toasts are removed:** "Connection lost, reconnecting…", "Reconnected" and the `watchDrops` toast sink go. The dot and countdown are the only connection signal. Remove `watchDrops`'s toast use and its tests, or keep `watchDrops` only if something else needs it. `LinkBanner` stays.
- **"Events dropped, resyncing"** becomes `notify.warning` (throttled as today).

## 5. Needs you

The count is `useApprovals.pending.length + useAsks.pending.length + useSecretRequests.pending.length` for the active engine. The badge "N need you" shows when N > 0 (ochre, monospace digits). Clicking it opens the attention inbox the way the sidebar `AttentionBadge` does.

## 6. Desktop bar — `components/status/StatusBar.tsx`

- **Height and surface:** 22 px, full width below both sidebar and main pane, `bg-sidebar`, top border `border-border-subtle`, 11 px text in `text-fg-tertiary`, `tabular-nums` mono digits.
- **iOS app (iPad, iPhone landscape on the md layout):** the bar is the home-indicator band itself, `max(var(--safe-bottom), 18px)` tall with its content centred, and nothing pads below it. The content row is inset `max(var(--safe-left/right), 28px)` to clear the ~18pt screen corners, and the history popover is inset by the same amount. Type is 10 px, the dot 6 px, and the separators 10 px. `--statusbar-h` equals the band. All of this is keyed on `<html data-container="ios">`, which `markContainer()` (`lib/platform.ts`) sets at boot, through the `ios:` Tailwind variant. Fixture: `?state=ipad` / `ipad-history`.
- **No drag region:** never add `data-tauri-drag-region`.
- **Segments, left to right:**
  1. Dot plus engine name (tooltip: engine, state, rtt if known).
  2. Separator.
  3. Notice slot (flex-1; click opens history).
  4. Needs-you badge.
  5. Separator.
  6. Unread-error chip `● N` in carnelian (only if > 0; click opens history).
  7. Rate: a 46×12 SVG sparkline of the 30 buckets, stroke `--status-success-fg`, followed by `1.5k ev/s`.
- **Notice in the slot:** a small kind tag (done = success tint, needs you = warning tint, error = danger tint, info = neutral; progress shows a tiny spinner), then the text (ellipsis) and the action as an underlined link-button. It slides up 6 px over 350 ms on entry and fades on exit. Use `prefers-reduced-motion` to disable motion.
- **History popover** (`components/status/NoticeHistory.tsx`):
  - Anchored to the bottom-right, growing up, ~340 px wide, max 50 rows.
  - Each row: time (HH:MM, mono), kind tag, text, description (pre-line), and action/cancel buttons.
  - Header "Recent" with a "Clear" button. Esc or an outside click closes it.
  - Mark the popover element `data-notice-popover`.
  - Use the app's existing popover primitive (Radix/shadcn) if there is one.
- **Mounting:**
  - Main window shell in `routes/__root.tsx`: wrap in `flex h-band flex-col`; `SidebarProvider` becomes `min-h-0 flex-1`; then `<StatusBar/>` with `shrink-0`.
  - The fixed sidebar panel in `components/ui/sidebar.tsx` must stop 22 px short on desktop (it is `position: fixed`). Use a CSS var `--statusbar-h` (22px on desktop, 0 when no bar) rather than a hard-coded calc.
  - Not in `Popped()`, island or overlay windows.
  - `GuestApp` gets the bar without the engine, needs-you and rate segments (notices only).
- **Hidden on the phone layout** (`useIsMobile()`).

## 7. Phone line — `components/status/PhoneStatusLine.tsx`

- **Where:** only when `useIsMobile()`. It sits under the composer, inside the bottom safe band.
- **Layout change:** move `pb-[var(--safe-bottom)]` off `SidebarInset` (`routes/__root.tsx`). Render the line as the last flex child of the inset, with `height: max(var(--safe-bottom), 16px)`. Content is vertically aligned to the top 14 px of that band, so it sits just above the home indicator. When the keyboard is open, `--safe-bottom` is 0, so the line is 16 px tall and sits directly above the keyboard (the shell already shrinks via `--visible-band`).
- **Content:** centered, 9.5–10 px, `text-fg-tertiary`. A 5 px dot (same state rules as §4), then `1.5k ev/s`. When offline: dot, then "offline · retry Ns". When a notice is current it takes the line: dot, then the notice text (ellipsis at ~70% width), for its lifetime, then the line returns to the rate.
- **Tap** opens a bottom sheet (`components/ui/sheet.tsx`, `side="bottom"`, ~60vh max):
  - Header: dot, engine, rate, and the needs-you count if any.
  - Then the history rows with actions, plus "Clear".
  - Opening it calls `markRead()`.
- **Never on desktop.**

## 8. Removal

- **Calls:** every `import { toast } from 'sonner'` becomes `import { notify as toast } from '@/lib/notify'` (or a rename to `notify`; either is fine, but be consistent within a file).
- **Mocks:** every `vi.mock('sonner', …)` in tests becomes `vi.mock('@/lib/notify', () => ({ notify: Object.assign(vi.fn(), { error: vi.fn(), message: vi.fn(), success: vi.fn(), warning: vi.fn(), info: vi.fn(), loading: vi.fn(), dismiss: vi.fn() }) }))`, adjusted to what each test asserts.
- **Delete** `components/ui/sonner.tsx`, every `<Toaster/>` mount (`App.tsx`, `GuestApp.tsx`, the `dev/*-fixture.tsx` files), and the `sonner` dependency in `package.json` and `bun.lock`.
- **`components/chat/web-native-frame.ts`:** replace the `[data-sonner-toast]` overlay selector with `[data-notice-popover]`. The bar itself is not an overlay.
- **Hand-work call sites** (they keep their options; the store makes action/Infinity sticky):
  - The speech model download progress (`App.tsx` `toast.loading` with an id) and `lib/speech.ts` (`MODEL_TOAST` loading → dismiss → error by id).
  - `App.tsx` voice-key chord error with action.
  - `OnboardingHost` (Infinity, action and cancel), `TourRunner` (action), `lib/meeting-model-offer.ts` (`notify` injection default).
  - `MessageAssistant` "Saved as a note · Open", `SourcePage` "Promoted · Open", `ExportMenu` "Saved · Reveal", `KernelStrip` "Environment refreshed · Restart".
  - `settings/shared.tsx`: `toastSync` and the default `useAction` `onError`.
- **Done when** `grep -rn "sonner\|toast(" crates/desktop/ui/src` shows no sonner import (the local variable name `toast` aliasing notify is acceptable), and `bun run check && bun run test && bun run build` are green.

## 9. Tests (new)

- `lib/notify.test.ts`: kinds map correctly, id replace, dedupe, sticky rules, `dismiss`.
- `store/event-rate.test.ts`: buckets, `formatRate` table (0, 38, 999, 1000 → "1k", 1499 → "1.5k", 9950 → "9.9k" or "10k" (pick one and document it), 15000 → "15k").
- `components/status/StatusBar.test.ts`: dot state matrix (open/needs-you/reconnecting), notice slot render, needs-you badge. Tests are `.test.ts` with react-dom `act`, like `WebFrame.test.ts`.
