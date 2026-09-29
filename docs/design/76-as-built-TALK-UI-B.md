# 76 as-built — TALK-UI-B (talk UI follow-ups B3, B5)

Branch `fix/talk-ui-b`, worktree `theuth-talk-ui-b`, based on `db4fa2c3` (the brief named `31d0d710`; the orchestrator
said either is fine). UI only — no Rust touched. A first worker (omp, died in a provider outage) left 8 files of
uncommitted work; it was reviewed, kept with one colour fix, and completed (Esc-to-leave, Esc ownership, verification).

## B3 — a way back from talk to the ordinary chat

Rule: the talk view is a mode of the session, not a different place.

- **Back to chat** in the talk view's header. The talk view's header *is* the shared `components/chat/Header.tsx`
  (`<Header popout={{ kind: 'talk' }} />`), so the item lives there: a new `HeaderId` `chat` in `lib/header.ts`,
  emitted by `headerOrder(panel, popout, chat)` only when `chat` is true (the talk view), placed in the `bar` slot
  right after the sidebar toggle and before the title, so it stays inline at every width tier. Rendered as a
  `Link` to `/c/$session` (same session) with lucide `MessageSquare` (the icon the artifact viewer already uses for
  "chat") — labelled "Back to chat" on desktop, icon-only (aria-label kept) on phones and in the compact bar.
  Leaving never stops the talk (the talk store is global; the route's unmount only stops the dev simulator).
- **Active-talk indicator in the chat.** `Composer.tsx`: when the talk runs on this very session and this is the
  chat view (`talkHere`: phase ≠ off, `talk.session === session`, not the talk view's read-only composer), the Talk
  button is replaced by a compact pill — a pulsing `AudioLines` button ("Talk running — open", tooltip "Talking
  here — open") that navigates to `/talk/$session`, and a small labelled **x** ("End talk", tooltip "End talk") that
  calls `talk.stop`. A talk on another session keeps the existing "Talk in ‹title›" behaviour.
- **Esc in the talk view** (`talk.$session.tsx`, the leave/escape handler only): unchanged meaning while the agent
  thinks/speaks/holds — the agent's cancel, view stays. Otherwise (idle, listening, no talk here, talk elsewhere) Esc
  **leaves for `/c/$session`**, never a blank view. It yields to every other Esc owner: typing in an input/textarea/
  contenteditable, an already `defaultPrevented` event, the phone transcript sheet being open, or any open Radix
  dialog/alertdialog/menu (`[data-state="open"]` — the dev-only OpenUI Inspect/Debug asides are `role=dialog` but
  never `data-state=open`, which the first selector tripped on). The decision is taken in a `setTimeout(0)` so the
  composer's recording/dictation Esc handlers (now `preventDefault()`-ing the event they spend) run first.
- **Mobile back.** The same header item is the phone's back affordance (icon-only, top-left beside the sidebar
  toggle); there is no separate back arrow in this shell, so nothing else to wire.
- **Entering talk never creates a session except from the new-chat page.** The composer's Talk button already used
  `talk.open(api, session)` (null only on `/`). The sidebar's Talk entry used `talk.open(api, null)` — always a new
  session; it now passes the session of the current `/c/<id>` or `/talk/<id>` path (`AppSidebar.tsx`, from the
  router `href`), so only elsewhere (home, knowledge, …) does it create one. A talk already running anywhere is still
  opened rather than a second started (`talk.open` unchanged).

## B5 — hold-to-dictate feedback

- **Reducer** (`lib/hold.ts`): `ring(h): 'empty' | 'filling' | 'full'` — `pressed` → filling, `held`/`locked` →
  full, `idle` → empty. No new timing constant: the fill duration is the reducer's `HOLD_MS` (350 ms).
- **Mic button** (`components/chat/MicButton.tsx`): an SVG progress ring (r 18 in a 40 px box, 2 px outside the
  36 px button). `filling` animates `stroke-dashoffset` from full circumference to 0 over `HOLD_MS` (CSS
  transition, accent colour); at the threshold the reducer's `record` verb fires (existing `buzz()` =
  `navigator.vibrate?.(15)`, iOS has none) and the ring snaps `full` in the danger colour while the button turns
  destructive (the existing recording look); the ring pulses (`motion-safe:animate-pulse`) while recording. Release
  before the threshold → `idle` → the ring collapses over 150 ms (the tap still starts dictation as before);
  release after → the existing send/cancel; lock/slide keep their visuals and the ring stays full while locked.
- **Level-driven pulse: not done** — the voice note records through `MediaRecorder` in `Composer.tsx`, and
  `mic-tap.ts` exposes no level for it, so the brief's fallback (a steady pulse) is used.
- Deviation from the first worker's code: its ring was drawn in the danger colour whenever not `filling`, so a
  released press collapsed in red; it now stays accent until `full`.

## Files

`lib/hold.ts`, `lib/hold.test.ts`, `components/chat/MicButton.tsx` (B5); `components/chat/Header.tsx`,
`lib/header.ts`, `lib/header.test.ts`, `components/chat/Composer.tsx`, `components/sidebar/AppSidebar.tsx`,
`routes/talk.$session.tsx` (leave/escape handler + `useNavigate` only) (B3).

Allow-list deviations: `Header.tsx` / `header.ts` / `header.test.ts` — the talk view's header block is the shared
`Header`, so "Back to chat" has to be a header item there; `Composer.tsx` — it owns both the mic (`MicButton`'s
parent) and the Talk button the indicator replaces, and its two Esc handlers now `preventDefault()`. `talk-sim.ts`
was not extended: every new visual is reachable with the existing simulator (`?sim=speaking`, then Back to chat —
the store keeps the held state, so the chat shows the indicator) and the ring needs no talk at all.

## Verification

- `bunx tsc --noEmit` clean; `bun run build` green; `CI=1 bunx vitest run`: 117 files, **662 tests passed**
  (`hold.test.ts` 6 incl. the new ring-transition test; `header.test.ts` 12 incl. "Back to chat inline top-left at
  every tier, absent from the chat view"; `talk.test.ts` 4).
- Live, headless Chrome (CDP, `--use-fake-device-for-media-stream`) against `bunx vite` + a scratch engine
  (installed `t` 2.3.2, `--data-dir /tmp/talkuib-data`, port 7793; nothing under `~/.theuth` touched), at 1280×800
  and 390×844 (mobile emulation, 2×):
  - `/talk/<id>?sim=speaking`: "Back to chat" link `href=/c/<id>`, top-left beside the toggle; text "Back to chat" on
    desktop, empty (icon-only) on the phone. Click → `/c/<id>`; composer shows "Talk running — open" + "End talk".
    Indicator tap → `/talk/<id>`. Esc while speaking → stays (cancel). End talk → indicator gone, Talk button back.
  - `/talk/<id>` idle, Esc → `/c/<id>` (desktop and phone). Phone with the sidebar sheet open, Esc → stays on
    `/talk/<id>` (the sheet takes it).
  - Mic ring (stroke-dashoffset of 113.1 = empty): press +170 ms → 43.1 (filling, accent); +870 ms → 0, danger,
    pulsing, button "Recording"; slide up 80 px + release → "Stop and send", ring 0/danger/pulsing; cancel → 113.1;
    tap (120 ms press) → 107.2 at +50 ms after release, 113.1 at +450 ms (collapsed, no recording).
  - Screenshots in `/tmp/talkuib/shots/` (`{desktop,phone}-{talk-header,chat-indicator,ring-filling,
    ring-recording,ring-locked}.png`), not committed.
- **Not verified:** haptics on a real Android device (no device; `navigator.vibrate` call unchanged from before);
  iOS/Tauri WKWebView rendering of the ring; a real (non-simulated) talk surviving the round trip talk → chat → talk
  (the store is global and the route's cleanup only stops the simulator, so by construction; the sim's fake talk id
  makes "End talk" toast "Invalid params" from the engine in this setup — expected, the store still goes off).

DONE TALK-UI-B
