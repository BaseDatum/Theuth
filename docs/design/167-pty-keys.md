# 167 + 169 as-built — terminal keys and replies; Settings stacked rows

Branch `fix/167-pty-keys`, base `b3d33c8e`. Brief: `/tmp/167/brief.md`. UI only (`crates/desktop/ui`); no Rust touched.

## 167: keys and replies in the terminal viewer

### Ask (Sky, 2026-11-03 16:02)

> whenever it creates a PTY a terminal if I open that terminal and I use the arrow keys or control characters of any kind or like option left, option right None of that stuff works in the raw characters are actually put on the end of whatever I typed after I press enter … if I press the up arrows or if I press like Ctrl C or something, I don't know, something is weird with the control characters and option and arrow keys.

Later: "it actually seems to be fine now… odd, I think it was happening when the terminal was running omp".

### Evidence (orchestrator, before this branch)

- Keyboard path intact: `TerminalViewer.tsx` `Terminal` (xterm 6.0.0) sends `term.onData` to `api.pty_input(session, job, d, null)`; `crates/tools/src/pty.rs` `encode` writes `text` bytes verbatim.
- Live capture in Sky's app (raw-mode key logger in a PTY): every key arrived at once, one read per key. ↑↓←→ `ESC[A`…`ESC[D`; Ctrl-C `0x03`; Ctrl-A/E `0x01`/`0x05`; ⌥→ `ESC[1;3C` (⌥← `ESC[1;3D`) — xterm 6.0 has no macOS "ESC b / ESC f" hack. In `bash -l`, ↑ recalled history and Ctrl-C cancelled the line.
- bash and zsh do not bind `ESC[1;3C`/`ESC[1;3D`: they insert `;3C` into the line, seen only when Enter runs it. Terminal.app sends `ESC b`/`ESC f`.
- Duplicate replies: the engine answers DA1 `CSI c`/`CSI 0 c`, DA2 `CSI > c`/`CSI > 0 c`, DSR `CSI 5 n`, CPR `CSI 6 n`, `OSC 10;?`/`OSC 11;?` (BEL or ST) itself (`pty.rs` `QUERIES` + `CPR`, 22 §4.4). Every attached xterm.js viewer answered them again through `onData`, so with a viewer open the program got each answer twice; a late CPR such as `ESC[12;1R` reads as a key to many programs.

### Causes

1. ⌥←/⌥→ junk: xterm 6 encodes them as xterm modifier sequences (`ESC[1;3D/C`) that the shells' default keymaps do not bind; ⌘←/⌘→ send nothing and ⌘⌫ sends a plain DEL.
2. Doubled replies: two responders (engine + each viewer's xterm) for the same queries. The "only with omp" observation fits this: omp queries the terminal (DA1/CPR/OSC colours) at start, the viewer's extra replies land in its input. [INFERENCE: not reproduced against Sky's omp; the brief's quick test showed omp 18.8.5 tolerating injected replies.]

### As built

**D1 — macOS editing keys.** `src/lib/terminal-keys.ts` `macKeyBytes(e, mac)` (pure; reads `type`, `code`, modifiers) returns bytes or `null`:

| key | bytes |
|---|---|
| ⌥← / ⌥→ | `ESC b` / `ESC f` |
| ⌘← / ⌘→ | `0x01` (^A) / `0x05` (^E) |
| ⌘⌫ | `0x15` (^U) |

Only a `keydown` with exactly ⌥ or exactly ⌘ (no ⇧/⌃, not both) maps; everything else returns `null` and stays with xterm. ⌥⌫: verified in both browsers that xterm already sends `ESC DEL`, so it is not in the table. `Terminal` attaches it with `term.attachCustomKeyEventHandler`: a mapped keydown calls `preventDefault()`, goes through the same `send` → `lane` as `onData` (ordering holds), and returns `false` so xterm does not also encode it. Windows and Linux: `mac` is false, xterm's defaults.

**D2 — one responder.** `src/lib/terminal-replies.ts` `swallowEngineQueries(term.parser)` registers handlers that return `true` (swallow) only for:
- `CSI c` with no param or a single 0 (xterm parses `CSI c` as `[0]`); `CSI > c` likewise;
- `CSI 5 n` / `CSI 6 n` (no prefix, single param);
- `OSC 10` / `OSC 11` whose data is exactly `?`.

Everything else returns `false` and falls through to xterm's own handler: DECRQM, `CSI ?6n`, `CSI 1 c`, colour SETs (`OSC 11;rgb:…` still repaints), multi-query `OSC 10;?;?`.

**D3.** Focus (`CSI ?1004h` → `ESC[I`/`ESC[O`) and mouse reports are input: untouched.

Wiring: `src/components/chat/TerminalViewer.tsx` `Terminal` — `send` extracted from the `onData` callback, `attachCustomKeyEventHandler`, `swallowEngineQueries(term.parser)`. Both are disposed with `term.dispose()`.

### Decisions and deviations

- **Platform helper: `isApple()` from `src/lib/chord.ts`, not `container()`/`isMacDesktop()`.** `container()` is `'web'` for every browser, so the relay web UI on a Mac would keep the junk sequences. The key convention follows the keyboard (Mac/iPad hardware keyboards use ⌥/⌘ the same way), which is what `isApple()` already decides for chord labels. Windows and Linux are false.
- `macKeyBytes` matches `code` (physical key), the convention `chord.ts` uses; for arrows and Backspace `code` and `key` agree.
- The mapped keydown also calls `preventDefault()` so the hidden textarea does not run its own ⌥⌫/⌘← editing.
- `src/components/chat/TerminalViewer.test.ts` unchanged and green.

### Proof

1. Vitest (node): `src/lib/terminal-keys.test.ts` (mapping + every non-mapped case: non-mac, keyup, extra/mixed modifiers, bare keys, ⌥⌫), `src/lib/terminal-replies.test.ts` (hand stub of `term.parser` that dispatches by exact identifier like xterm: swallowed forms and pass-through forms).
2. Real xterm 6.0.0 from this worktree's `node_modules`, in headless Playwright Chromium (headless shell 1226) and WebKit (2370), served by this worktree's vite on port 1467 with a throwaway config. Harness `/tmp/167/proof/harness.ts` builds the `Terminal` exactly as `TerminalViewer` wires it (imports `macKeyBytes`, `swallowEngineQueries`, `isApple`); `?mode=before` is plain xterm as on main. `navigator.platform` read `MacIntel`, `isApple()` true. Driver `/tmp/167/proof/run.mjs`; results `/tmp/167/proof/xterm-results.json`, table `/tmp/167/proof/xterm-table.md`, screenshots `/tmp/167/proof/xterm-{chromium,webkit}-{before,after}.png` (after an `OSC 11` colour SET: background repainted dark red in both modes). Nothing connected to the live engine.

Replies the viewer writes to `onData` (`-` = nothing):

| query | Chromium before | Chromium after | WebKit before | WebKit after |
|---|---|---|---|---|
| DA1 CSI c | ESC [?1;2c | - | ESC [?1;2c | - |
| DA1 CSI 0 c | ESC [?1;2c | - | ESC [?1;2c | - |
| DA2 CSI > c | ESC [>0;276;0c | - | ESC [>0;276;0c | - |
| DA2 CSI > 0 c | ESC [>0;276;0c | - | ESC [>0;276;0c | - |
| DSR CSI 5 n | ESC [0n | - | ESC [0n | - |
| CPR CSI 6 n | ESC [1;1R | - | ESC [1;1R | - |
| OSC 10 ? BEL | ESC ]10;rgb:e8e8/dcdc/c8c8 ESC \ | - | ESC ]10;rgb:e8e8/dcdc/c8c8 ESC \ | - |
| OSC 10 ? ST | ESC ]10;rgb:e8e8/dcdc/c8c8 ESC \ | - | ESC ]10;rgb:e8e8/dcdc/c8c8 ESC \ | - |
| OSC 11 ? BEL | ESC ]11;rgb:1212/1010/0e0e ESC \ | - | ESC ]11;rgb:1212/1010/0e0e ESC \ | - |
| OSC 11 ? ST | ESC ]11;rgb:1212/1010/0e0e ESC \ | - | ESC ]11;rgb:1212/1010/0e0e ESC \ | - |
| DECRQM CSI ?2026$p | ESC [?2026;2$y | ESC [?2026;2$y | ESC [?2026;2$y | ESC [?2026;2$y |
| DECXCPR CSI ?6n | ESC [?1;1R | ESC [?1;1R | ESC [?1;1R | ESC [?1;1R |
| DA1 CSI 1 c | - | - | - | - |
| CSI ?u | - | - | - | - |
| CSI >0q | - | - | - | - |
| focus CSI ?1004h | ESC [I | ESC [I | ESC [I | ESC [I |

Bytes sent for real `page.keyboard` presses (macOS platform):

| key | Chromium before | Chromium after | WebKit before | WebKit after |
|---|---|---|---|---|
| ⌥← Alt+ArrowLeft | ESC [1;3D | ESC b | ESC [1;3D | ESC b |
| ⌥→ Alt+ArrowRight | ESC [1;3C | ESC f | ESC [1;3C | ESC f |
| ⌘← Meta+ArrowLeft | - | 0x01 | - | 0x01 |
| ⌘→ Meta+ArrowRight | - | 0x05 | - | 0x05 |
| ⌘⌫ Meta+Backspace | DEL | 0x15 | DEL | 0x15 |
| ⌥⌫ Alt+Backspace | ESC DEL | ESC DEL | ESC DEL | ESC DEL |
| ↑ ArrowUp | ESC [A | ESC [A | ESC [A | ESC [A |
| Control+C | 0x03 | 0x03 | 0x03 | 0x03 |
| Control+A | 0x01 | 0x01 | 0x01 | 0x01 |
| Control+E | 0x05 | 0x05 | 0x05 | 0x05 |

Each mapped key produced exactly one send (no double encode by xterm).

### Open items

- **Multi-viewer caveat (follow-up):** queries the engine does not answer (DECRQM, `CSI ?6n`, DA3, XTVERSION in newer xterm, …) are still answered by every attached viewer, so with two viewers open (panel + popped window, or desktop + phone) the program gets them twice. Fix candidates: an engine-side answer for these too, or one "primary" viewer that answers.
- With no viewer open the engine answers only its table; unchanged.
- The engine's DA1/DA2 strings (`ESC[?62;22c`, `ESC[>41;0;0c`) differ from xterm's; programs now see only the engine's, consistently.

## 169: Settings rows with `stack` break on desktop

### Ask (Sky, 2026-11-03 16:33, screenshot of Settings › Meetings › Detection › "Ignored apps")

> what's going on with this meetings settings UI. that needs to be fixed too.

Label/description squeezed to ~30 px, one word per line; the chip list and the "Ignored app" input on top of the description.

### Cause

`src/components/settings/shared.tsx` `Row`: `stack` switched to a column only below the `md` viewport breakpoint (`max-md:flex-col …`). The desktop window is wider than `md` but the settings content column is 594 px (dialog `max-w-[880px]` − nav 220 − padding 64), so the row stayed a flex row; the label side is `min-w-0` and collapsed, the control side `shrink-0`, so `Chips` (`max-w-xl`, 576 px) took the row. Measured before (Chromium and WebKit, 1280×800 and 900×660 identical, the dialog is 880 wide in both): Ignored apps label **0 px** wide × 666 px tall, Live tools label 0 px, Echo label 90 px.

### As built (R1, after the orchestrator's review)

- **`stack` is exactly as on main**: right of the label on desktop, under it below `md`; desktop classes byte-identical. The 148 R3 test `src/components/settings/shared.test.ts` is back unchanged.
- **New `Row` prop `wide`** for a control that grows (chips, lists, switch or button groups, long text): always a column at any width — `flex flex-col gap-3 …` with label and description on top and the control wrapper `flex w-full items-center gap-2` (no `shrink-0`) below, so `Chips` wraps inside it. Plain rows unchanged. Both props doc-commented. New vitest beside 148 R3 pins `wide`'s classes and checks that `stack` and plain rows keep main's classes.

**Why `wide` instead of changing `stack`.** R0 made every `stack` row a column. That also moved the model pickers (Chat › New sessions / Subagents / theuth agent, Memory › Compactor, Meetings › Meeting/Summary model, Loop, Voice, …) under their labels on desktop, although 148 R3 had right-aligned them on purpose and Sky only asked about Meetings. The two cases differ: a picker is narrow on desktop and needs stacking only on a phone; a chip list or switch group is wide at every width. One prop per case keeps 148 R3's layout and fixes the growing controls.

Audit (`rg -n '<Chips|flex-wrap|<Row stack' src/components/settings`, plus every settings `Row` whose control can grow):

| row | before (594 px column) | change |
|---|---|---|
| Meetings › Ignored apps (`Chips`) | label 0 px — the reported defect | `stack` → `wide` |
| Meetings › Allowed while a meeting runs (Live tools, `Chips`) | label 0 px | `stack` → `wide` |
| Meetings › Echo in your microphone (`echoSummary` sentence in a `shrink-0` span) | label 90 px, sentence on one line | `wide` |
| Double › Train now (Tier A, Tier B local, `w-36` node select, Tier B on node, Rebuild ≈ 580 px in a `shrink-0` wrapper) | label 0 px | `wide`; `justify-end` dropped so the buttons start under the label |
| Double › Capture (five labelled switches, ≈ 450 px) | label 124 px, description 7 lines | `wide`; `justify-end` dropped |
| Model pickers with `stack` (Chat ×3, Loop ×3, Memory, Voice, Meetings ×2), Dreaming (Window, tiers, Weekly digest), Autoresearch (roles, Bench lock), Calendar › Add subscription (its form lives in the description) | fine on desktop (e.g. Chat label 360 px) | unchanged, keep `stack` |
| Double › Roll back, Window and venue (≈ 330 px), Link › Relay, Communication › Who may discover, VoiceKey › Draw the trail (`w-72`), General › Data directory (`max-w-96 truncate`) | label ≥ 200 px | unchanged |

Other `flex-wrap` hits (Dreaming night/proposal lists, Search, SandboxVms/Sheets, Mesh, Link, Communication label spans) are not `Row` controls.

### Proof

Worktree UI (vite on 1467) against a scratch engine: installed `/Applications/theuth.app/Contents/MacOS/t --data-dir /tmp/167/eng --headless` (block-style config, `rpc.listen 127.0.0.1:17467`, every plugin off except `meeting` and `double`; `schedule` off), detached with `setsid`. The Double page loaded with no credentials (empty classes, KPIs zero). "Before" ran with main's `shared.tsx`/`Meetings.tsx`/`Double.tsx` checked out in the working tree, "after" with `89fd8cc2`. Headless Chromium and WebKit, 1280×800 and 900×660. Driver `/tmp/167/proof/settings.mjs`, geometry `/tmp/167/proof/settings-results.json`, PNGs `/tmp/167/proof/settings-{before,after}-{chromium,webkit}-{1280x800,900x660}-{meetings-detection,meetings-livetools,meetings-echo,chat-top,double-train,double-capture}.png` (48). R0's shots (every `stack` a column) are kept in `/tmp/167/proof/r0/`.

| row (all four browser × size runs identical) | before label | after label | after control |
|---|---|---|---|
| Meetings › Ignored apps | 0 × 666 | 594 × 70 | 594 × 258 |
| Meetings › Allowed while a meeting runs | 0 × 582 | 594 × 70 | 594 × 118 |
| Meetings › Echo in your microphone | 90 × 474 | 594 × 94 | 594 × 24 |
| Double › Train now | 0 × 666 | 594 × 70 | 594 × 36 |
| Double › Capture | 124 × 166 | 594 × 70 | 594 × 18 |
| Chat › New sessions | 360 × 70 | 360 × 70 | 210 × 36 |

Chat top is pixel-identical before/after: `/tmp/167/proof/pixdiff.mjs` decodes both PNGs on a canvas in headless Chromium and compares every RGBA pixel — 0 differing pixels in all four browser × size pairs; the PNG SHA-1s are equal too.

Engine and vite stopped (`/bin/kill` on the engine pid matched by `/tmp/167/eng`), `/tmp/167/eng` removed.

### Open items

- None for 169. The Loop/Memory/Voice pickers were not reshot; they share `stack`'s byte-identical classes with Chat.

## Sky's live checks

1. Open a zsh PTY in the terminal viewer: ⌥←/⌥→ move by word, ⌘←/⌘→ to line start/end, ⌘⌫ clears to line start; ↑ and Ctrl-C as before.
2. Run omp in a PTY with the viewer open: no stray `;1R`/`?1;2c` text in its input.
3. Settings › Meetings: Ignored apps and Live tools show the description on top, chips wrapping below; Echo reads as a sentence under its label.
