# 218 — proof against a real engine (browser, light and dark)

2026-11-25, branch `feat/218-whiteboard`, worktree `theuth-218-whiteboard`. Driven headless; no agent screenshot tool.

## Setup (all torn down afterwards)

- **Engine.** `cargo build -p theuth --bin t` (under `/tmp/theuth-build.lock`, warm target, exit 0), then
  `env -u THEUTH_RPC__LISTEN THEUTH_DATA_DIR=/tmp/theuth-218-dev target/debug/t --headless`. `config.yaml`:
  `rpc.listen 127.0.0.1:17218`, `secrets.key_store: file`, `link.enabled: false`, mesh `relay: none`, `discovery: false`,
  `bind 127.0.0.1:17219`, `hub: never`, `sandbox.enabled: false`, `agent.model` / `title_model: fake/fake-model`.
- **Model.** `providers.yaml` row `fake: { base_url: http://127.0.0.1:17290/v1/, owns_key: false }` — a 40-line Bun
  OpenAI-compatible server (keyless local provider). Scripted: on the owner's message it answers with two tool calls,
  `python` → `await wb_place(kind="sticky", title="From the agent", source={"t":"sticky","text":"Placed by the agent via wb_place"})`
  and `show_file(<abs path>/crates/desktop/ui/package.json, 1–14)`; after the tool results, the text
  "Placed a sticky on the whiteboard." Every request body was logged, which is how the injection was read.
- **UI.** `crates/desktop/ui`: `VITE_THEUTH_URL=ws://127.0.0.1:17218/ VITE_THEUTH_TOKEN=<data dir token> bun run dev`
  (port 17280) — the dev-browser path of `lib/tauri.ts#devConnection`.
- **Browser.** Playwright-core 1.65 driving Chrome for Testing (ms-playwright chromium-1254), headless, persistent
  profile per configuration. Viewports 1366×1024 and 1024×1366. Theme: `localStorage.theme` + `colorScheme`, and one
  live switch on an open whiteboard (`.dark` on `<html>`, what Settings › Appearance does). Pen strokes are trusted
  CDP `Input.dispatchMouseEvent` with `pointerType: "pen"`; the touch run uses `hasTouch`/`isMobile` and
  `touchscreen.tap` (coarse pointer).

## What was exercised (per configuration: landscape/portrait × light/dark, all four passing at the end)

1. Sidebar → Whiteboards → All whiteboards → the list (Library band) → **New whiteboard** → full-screen canvas with
   the glass pill bottom-right ("no session · Ask…").
2. Tap on empty canvas → control cluster + exit button; the **React Flow attribution shows top-left only then** (R3).
   ⋯ menu and the ink-tools popover open beside the cluster.
3. Cluster → Add → **Sticky**: the new sticky opens for typing; text typed, committed on blur.
4. **Pen stroke** (pointerType pen) on empty canvas → one ink path, full strength (black on paper, cream on slate).
5. Pill → **composer sheet** with no session: tray "1 image · 1 mark" with the Canvas thumbnail, "Send to a new
   session" (R1). Message sent.
6. **Agent round trip**: the engine created the session bound to the whiteboard; the model's `wb_place` (through the
   python runtime) asked for approval — the card now shows on the whiteboard (below) — Allow → the agent's sticky
   appeared **live** right of the owner's (placement), the sent mark dropped to 30 %, and the **reply bubble** pinned to
   that card with *Undo this turn · Open session · Dismiss*.
7. Pill → sheet again: **session dot + short name chip** ("Whiteboard proof", green dot, link glyph) and the **model
   picker** (`fake/fake-model ⌄`) (A2, R12).
8. Cluster → Add → **File excerpt** (absolute path the agent had shown) → excerpt card with numbered lines; Add →
   **Web page** (`http://127.0.0.1:17290/mock`) → web card; **Fit**; **Outline view**.
9. Touch run (1366×1024): finger tap raises the cluster; a pen stroke across a sticky becomes a mark **on the card**
   (tray thumbnail "Sticky, 1 mark"; `<whiteboard>` says `m1 on c1 v1 · sent`); send, Allow, bubble, **Undo this turn**
   removed the agent's card (pill: "Undid the agent's turn…"); live switch to dark re-themed canvas, cards, ink, glass.

### The `<whiteboard>` injection (from `load_session` of the bound session, and the model request log)

```
[attached whiteboard agrbcngwtn347ju224unbiojoe "Whiteboard" · 1 card]
Put a sticky on the whiteboard, please.[image: 50ebf30e413fa502.png]
<whiteboard id="agrbcngwtn347ju224unbiojoe" title="Whiteboard" cards="1" links="0" marks="1">
loose
  row 1: c1
c1 sticky "Sticky" · live · 1 lines · #9996
m1 on c1 v1 · sent
</whiteboard>
<whiteboard_changes whiteboard="agrbcngwtn347ju224unbiojoe">
in view: c1
image 1: c1 · marks m1
</whiteboard_changes>
```
The tool result the model got back: `{'card': 'c2', 'id': '01M88JQ9999RVA4VCCK4TM7H0A', 'placeholder': False, 'frame':
None, 'x': 817.0, 'y': 437.0, 'w': 220.0, 'h': 150.0}`. `whiteboard_sessions` lists the new session as bound.

## Screenshots — `/tmp/wb218-proof/` (`<tag>` = `land-light`, `land-dark`, `port-light`, `port-dark`; 4 of each)

| File | State |
|---|---|
| `01-list-<tag>.png` | Library band, Whiteboards tab, list with New whiteboard |
| `02-empty-<tag>.png` | New whiteboard: dotted paper / slate, pill only |
| `03-cluster-<tag>.png` | After a canvas tap: cluster, exit button, attribution top-left |
| `03b-more-<tag>.png` | ⋯ menu: title, background (current one highlighted), toggles, exports, close |
| `03c-ink-tools-<tag>.png` | Ink tools popover: pen (selected, ochre), fine liner, marker, eraser, six inks |
| `04-add-menu-<tag>.png` | Add menu (session-only kinds disabled without a session) |
| `05-sticky-<tag>.png` | The owner's sticky with typed text |
| `06-ink-<tag>.png` | A fresh pen stroke on the canvas |
| `07-sheet-nosession-<tag>.png` | Composer sheet, no session: tray thumbnail, "1 image · 1 mark", Send to a new session |
| `08a-approval-<tag>.png` | The `wb_place` approval card above the pill; pill "waiting for you: wb_place" |
| `08-agent-card-<tag>.png` | Agent's sticky placed live; sent ink at 30 %; reply bubble with Undo this turn |
| `09-sheet-session-<tag>.png` | Sheet with session chip (dot + name + link glyph) and model picker |
| `10-excerpt-<tag>.png` | File excerpt card (package.json 1–14) |
| `11-fit-<tag>.png` | Fit on the selected web card (100 %, centred) |
| `12-outline-<tag>.png` | Outline view (rows, cards, marks) with the bubble |
| `20-touch-cluster-land-light.png` | Touch: finger tap raised the cluster |
| `21-touch-mark-on-card-land-light.png` | Touch: pen mark across a sticky |
| `22-touch-sheet-card-mark-land-light.png` | Sheet tray: the card's thumbnail with its mark |
| `23-touch-bubble-land-light.png` | Reply bubble after Allow |
| `24-touch-after-undo-land-light.png` | After Undo this turn: agent card gone, pill notice |
| `25-touch-live-dark-land.png` | Same board after a live switch to dark |

## Defects found and fixed

| Commit | Defect (seen in the browser) | Fix |
|---|---|---|
| `51f18b7b5` | Pill broken: the session chip overlapped the grabber and "Ask…", its label had 0 px height. B4's glass `.wb-chip` collided with B3's card-body `.wb-chip` (`position: absolute; inset: 0`) | B4's class renamed `wb-well` (Pill, Bubble, ⋯ title field, chooser input, `glass.css`) |
| `b5dcc0ef1` | Add / Ink tools / ⋯ popovers opened at (0, −820), off screen: `ClusterButton` (a `PopoverTrigger asChild`) dropped the ref and Radix props | `ClusterButton` spreads the rest props (ref, aria-expanded, data-state) |
| `5a9be2a6e` | (a) An agent's `wb_*` write waiting on approval was invisible full-window: pill said "working…" forever. (b) Outline view showed the canvas cards through it (React Flow sets `visibility: visible` inline on nodes). (c) A new sticky was not editable until double-click, and the Add popover returned focus to its trigger, so typed keys hit the shortcuts (`O` opened the outline, `N` added stickies). (d) Resting the mouse in the bottom-right quadrant raised the cluster (and attribution) under an open sheet | (a) the target session's pending approval renders as the chat's `ApprovalCard` above the pill, pill status "waiting for you: <tool>"; (b) `opacity-0 pointer-events-none` with `invisible`; (c) Add → Sticky enters the new card; Add popover `onCloseAutoFocus` prevented; (d) quadrant rest is ignored while the composer is open |
| `18bc3836a` | Pill status/notice cut to "workin…" / "Could not…" by an 11 rem session chip; Fit on one selected card zoomed to ~3× | chip shrinks (min 5.5 rem) while a status keeps ≥ 9.5 rem, full text in `title`; Fit-to-selection centres with zoom ≤ 100 % |
| `51a8e28f6` | Reply bubble never appeared: the whiteboard does not stream the session transcript, so the final reply was not in the store when `running` dropped (store had only the tool rows) | at turn end the view re-reads the session (`load_session`, 60 rows) and opens the bubble when the reply row arrives (subscribed only while awaiting). The subject line also claims the flip fix; that is the next commit |
| `d2087d1ef` | The card-pinned bubble ran under the raised cluster | anchored node flips left when bubble + cluster would not fit |
| `4eda66bf8` | Card titles clipped descenders ("Sticky", "agent") — `line-height: 1` with `overflow: hidden` | header line-height 16 px |
| `0f9b77291` | Selected tool (pen), current background, colour ring and Send / Add buttons used `--accent`, which in this app is shadcn's muted layer colour: on-states near-invisible, Send was white on beige | `--fg-accent` / `bg-fg-accent text-primary-foreground` (the chat's SendButton tokens) everywhere in the whiteboard |

Checks after the last fix: `bun run check` exit 0; `bun x vitest run src/components/whiteboard src/lib` → 157 files,
1137 tests passed.

## Defects left (not fixed)

- **File excerpt from an unseated session.** `file_view_read` reads only files the engine showed for the session or
  files inside its seat's cwd/repo (`artifact/src/codeview.rs#allowed_dirs`); a session the whiteboard composer
  creates has no seat, so the Add → File excerpt dialog (placeholder `src/app.ts:10-40`) fails with "not a shown file"
  for any path the agent did not show. Proven here only with a shown absolute path. Needs a product decision (seat the
  session, or read through the bound session's cwd).
- **Web card in a plain browser** shows a globe placeholder, not the page [INFERENCE: live web cards are native-webview
  only]; not checked in Tauri.
- **Approval card** shows the `source` argument as raw JSON (the chat's `describe()`); readable, not pretty.
- **Anchored bubble** may cover a neighbouring card when flipped left (seen over the owner's sticky header).
- **Nav**: on `/whiteboards` both the sidebar's Whiteboards row and Library are highlighted.
- **Not exercised**: iPad / Pencil (native ink plugin), link mode, frames, version scrubber, placeholders shimmer,
  `wb_crop` snapshots (needs Tauri capture), Follow, voice note. Default approval mode makes every agent `wb_*` write
  ask; an owner with auto-approve would not see the card.
