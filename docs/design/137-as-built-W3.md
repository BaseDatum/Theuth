# 137 — As built, W3 (UI)

Branch `feat/137-w3` off a0046c22. Scope: `crates/desktop/ui/**` (plus this file and `docs/design/137-ui/*.png`). Spec: `137-endless.md` §0 D13, §9, §11.

## What was built (file level)

| File | Change |
|---|---|
| `src/rpc/endless-stubs.ts` | **New, the one stub file.** `MemoryMode`, `ViewLine`, `GuardTrips`, `EndlessStats`, `Backfill`, `SessionMemory`, `EndlessProgress`, `EndlessConfig` (ts-rs casing); `CreateSessionOptionsEndless` (= `CreateSessionOptions & { memory }`); `memoryOf(row)` (reads `SessionSummary.memory`, `'epochs'` when absent); `EndlessApi` + `ENDLESS_METHODS` + `bindEndless()` for the six request methods. |
| `src/rpc/api.ts` | `interface Api extends EndlessApi`; `makeApi` calls `bindEndless` after `METHODS`. `METHODS` itself is untouched so `api.test.ts` (which diffs it against `bindings/methods.json`) stays green until the closer regenerates. |
| `src/rpc/events.ts` | `WireEvent` gains `EndlessProgress`. |
| `src/lib/rpc.ts` | `queryKeys.sessionMemory(id)`, `queryKeys.endlessSettings`; `useInvalidateOn` refetches `sessionMemory` on `EndlessProgress` (the existing `on(...)` bus — no new bus). `useApi()` wraps the api in the dev fixture under `import.meta.env.DEV` (folds away in production). |
| `src/components/chat/Composer.tsx` | `DraftCreate.endless: boolean`; new exported `createOptions(draft)` builds the `create_session` options (`memory: 'endless' \| null`); the create path takes `create_session` when private, project **or endless** is set. Draft panes and whole-window `/new` share this path. |
| `src/components/chat/NewChatPane.tsx` | `CreateOptions` (now exported, for tests): third item right after Private — `Switch` (aria-label "Endless memory") + `∞` one size up (`text-base leading-5`, `-top-px`, `font-feature-settings: normal`) inside `Tooltip` "Endless memory". Neutral notice (`border-border-subtle bg-muted/40`, same geometry as the private card) with the three §11.3 sentences and `Compactor: <model \| same as the session>`. Initial state = `endless_settings_get().default_chat`, remembered per draft (`useState<boolean \| null>` overlaid on the setting). |
| `src/lib/header.ts` | `HeaderId 'memory'`, placed right after `title`, `slot: 'menu'` (inline in the full bar; a chevron-menu row on narrow panes). Existing tier tests unchanged and green. |
| `src/components/chat/Header.tsx` | `memory` render entry → `MemoryBadge`; `useState` for the panel like the other sheets; `<MemoryPanel>` mounted with the session sheets. |
| `src/components/chat/MemoryPanel.tsx` | **New.** `MemoryBadge` (∞ pill, cost-pill height, Tooltip `Endless memory · view N KB · n lines · up to date / settling k lines / backfill d/t`, `motion-safe:animate-pulse` while settling or backfilling, click toggles; `MenuRow` in row mode). `MemoryPanel` right sheet (`sm:w-[560px]`): stats row (mode badge · compactor model · calls · guard trips with a Tooltip breakdown per guard · ≈ $usd with token detail on `title`), recall input (Enter → `endless_recall(session, q, 20)`; hits listed above the view with `<mark>` highlights; `/` focuses it from anywhere in the sheet but a text field), the view grouped by band with `×n` labels at every `n` change, each line `id+n` muted + text (unbuilt lines italic/tertiary), click/Enter/→ zooms via `endless_zoom` into two children (indented, grid-rows 0fr→1fr animated under `motion-safe`, recursive) or, for `n = 1`, the whole message in a monospace block; ↑/↓ move, ← collapses or goes to the parent, Esc closes (Radix). Footer `Progress` (determinate: `built/lines` while settling, `done/total` + ETA while backfilling). Pending → skeletons; error → "Memory details unavailable: …"; `mode: epochs` → one explanatory line. Tree state is an exported `reduce()` (toggle/loaded/failed/hits) + `flatten()` for keyboard order; `highlight()`, `badgeTip()`, `viewBytes()` exported too. |
| `src/components/sidebar/SessionItem.tsx`, `SessionList.tsx` | `endless` prop → `∞` after the title (`text-sm leading-4 text-fg-tertiary`, `role="img"` "Endless memory"); `SessionList` passes `memoryOf(row.session) === 'endless'`. |
| `src/components/settings/Memory.tsx` | **New** Settings › Memory: Switch "New chats start endless", Switch "Desks start endless"; view size native range 16–256 KB step 8 with live `≈ N k tokens` (bytes ÷ 3) in the description; slack range 0–50 % step 2.5; settle seconds `Count` 5–180; compactor `ModelCombobox` with `none: 'Same as the session'`; read-only guards line. Reads `endless_settings_get()`, writes the whole `EndlessConfig` through `endless_settings_set()` (the `search_settings_*` / `double_settings_*` section pattern) via `useAction`. Sliders write once on release. |
| `src/components/settings/SettingsDialog.tsx` | `['memory', 'Memory', Infinity]` tab after Communication; `Pane` renders `<Memory />`. |
| `src/dev/endless-fixture.ts` | **New, dev-only** (`VITE_ENDLESS_FIXTURE=1` or `localStorage.endless-fixture = "1"`, like `routines-fixture.ts`): answers the six methods from memory and tags the two newest root chats `memory: 'endless'` in `list_sessions`, so the badge, panel, sidebar glyph and Settings page render against an engine without the 137 RPCs. Memoised per api object. |
| Tests | `chat/NewChatPane.test.ts` (switch placement, notice on/off, compactor line, `createOptions` → `memory`), `chat/MemoryPanel.test.ts` (expand/collapse/recursive flatten, failed zoom, hit prefix, highlight, badge tooltip, badge render + pulse with a mocked `session_memory`), `settings/Memory.test.ts` (token estimate; stored values on switches/sliders/count, "Same as the session", guards line, with a mocked `endless_settings_get`). |

## Gates (exact commands, result lines)

- `cd crates/desktop/ui && BUILD_LABEL=137-W3 $HOME/.theuth/bin/build-lock bunx tsc --noEmit -p .` → no output, `TSC-EXIT=0`.
- `cd crates/desktop/ui && BUILD_LABEL=137-W3 $HOME/.theuth/bin/build-lock bunx vitest run src/components/chat/NewChatPane.test.ts src/components/chat/MemoryPanel.test.ts src/components/settings/Memory.test.ts src/lib/header.test.ts src/rpc/api.test.ts src/components/sidebar/SessionList.test.ts src/components/settings/Channels.test.ts src/components/chat/CostPill.test.ts` → `Test Files  8 passed (8)` · `Tests  43 passed (43)`.
- `cd crates/desktop/ui && BUILD_LABEL=137-W3 $HOME/.theuth/bin/build-lock bun run build` → `✓ built in 8.36s`, `BUILD-EXIT=0` (only the pre-existing chunk-size warning).
- No JS formatter step exists in the UI gates; none added.

Bindings note: the worktree's `src/rpc/bindings` (installed by the prep) predated ~35 types (`AdvisoryNote`, `LiveCountsRow`, `ReminderEvent`, …; 261 tsc errors at baseline). I regenerated them from this worktree's Rust with the crate-level `cargo test -p rpc --features ts --lib --quiet export_bindings` (8 passed) behind the lock and copied `bindings/.` into the UI tree (the `just ts-desktop` copy step, by hand). Both directories are git-ignored; nothing of that is in the commits.

## Visual check (`docs/design/137-ui/`)

vite dev (`VITE_THEUTH_URL=ws://127.0.0.1:61623`, token from `~/.theuth/token`, `VITE_ENDLESS_FIXTURE=1`) against Sky's running engine (v2.12.3), headless Chromium via the harness browser. Live rows are real; the endless data is the fixture's.

- `new-chat-endless-desktop-light.png`, `new-chat-endless-desktop-dark.png` (1280×800), `new-chat-endless-mobile-light.png`, `new-chat-endless-mobile-dark.png` (390×844): New chat with ∞ on (switch clicked in the page; `aria-checked="true"` read back), the neutral notice, `Compactor: same as the session`.
- `sidebar-glyph-dark.png`: two sessions carrying the `∞` glyph after the title (the fixture's tagged rows).
- `header-badge-dark.png`: the ∞ pill after the title; tooltip read back as `Endless memory · view 1.5 KB · 15 lines · settling 1 line`; pulse class present while `settling`.
- `memory-panel-expanded-dark.png`: bands `×64 ×32 ×16 ×8 ×4 ×2 ×1`; `0+64` expanded by click into `0+32`, `32+32`; `→ →` on the keyboard moved to `0+32` and expanded it into `0+16`, `16+16`; `190+1` opened into the monospace message block; footer `Settling 1 line 93%`.
- `memory-panel-recall-dark.png`, `memory-panel-recall-light.png`: `/` focused the recall box, `badge pulse` + Enter → `1 hit for “badge pulse”`, 5 `<mark>`s.
- `memory-panel-mobile-light.png`, `memory-panel-mobile-dark.png` (390×844): the badge sits behind the header chevron as a row ("Endless memory"), opened from there; `64+32` expanded.
- `settings-memory-light.png`, `settings-memory-dark.png`: Settings › Memory; clicking "Desks start endless" wrote through `endless_settings_set` (fixture) and the refetch shows it on.

Esc: closes the sheet from the tree (verified). The recall box is `type="text"` so Esc there is not swallowed by the native search-field clear (the first run with `type="search"` needed a second Esc).

## Deviations and decisions

1. **Settings RPC.** There is no generic settings RPC in the UI; each section has its own pair. I used the existing section pattern: `endless_settings_get(): EndlessConfig` and `endless_settings_set(settings: EndlessConfig): EndlessConfig` (whole-struct write, like `search_settings_*` / `double_settings_*`). The engine side (W2/closer) must provide these two methods with §13's `EndlessConfig` shape; the desktop writes all fields back, so unknown/omitted fields would be lost — keep the struct whole on the wire.
2. **`memory` on the wire.** `create_session` sends `memory: null` when the switch is off (ts-rs `Option<MemoryMode>` → `MemoryMode | null`), `'endless'` when on; the engine applies `agent.endless.default_chat` to `null`.
3. **Message date.** §11.3 says the zoomed message renders "with its date", but §11.2's `ViewLine` carries no timestamp and no RPC exposes `date(id)`. The message block renders without a date. Hand-off: add `at: string | null` to `ViewLine` (set for `n = 1`) and the panel shows it in the block header — one line in `LineNode`.
4. **Badge placement.** Inline right after the title in the full bar; behind the chevron (as a labelled row) on compact panes, so the wave-97 tier tests and bar lists stay as they are.
5. **Panel state** lives in `Header` (`useState`, like cost/terminals), not in the sessions store — nothing else needs it.
6. **Guard trips** show the six counters the stats carry (`copy · grounding · status · format · empty · size`); the Settings guards line names the four the design lists as "always on".
7. **View bytes** in the badge are computed client-side from the rendered form (`id+n|text\n` per line, UTF-8) with `fmtBytes` (1024-based, matching the rest of the UI); the Settings slider speaks decimal KB (`64 KB` = `view_bytes 64000`, the config's range).
8. **Animated height** uses the CSS grid `0fr → 1fr` transition (no new dependency); children stay mounted (inert, aria-hidden) when collapsed so the collapse animates too and re-opening is instant.
9. **Tests** are vitest without a DOM (the tree's convention): component tests use `renderToStaticMarkup`; interaction logic is exercised through the exported `reduce`/`flatten`/`highlight` and in the browser run above.
10. **Dev fixture at `useApi`.** The fixture wraps at `useApi()` (DEV only) rather than per call site so every `list_sessions` observer sees the tagged rows; a per-site wrap flickered because React Query takes the queryFn of whichever observer refetches.

## Hand-offs

- **Closer (bindings swap):** `just ts-desktop`, then delete `src/rpc/endless-stubs.ts` and: `api.ts` — drop the `EndlessApi`/`bindEndless` import, `extends EndlessApi`, the `bindEndless(api, client)` line; add the six signatures to `interface Api` and the names to `METHODS` (sorted). `events.ts` — import `EndlessProgress` from bindings. `Composer.tsx` — `createOptions` returns `CreateSessionOptions`. `MemoryPanel.tsx`, `SessionList.tsx` — `memoryOf(x)` → `x.memory`. `NewChatPane.tsx`, `Memory.tsx`, tests, `dev/endless-fixture.ts` — type imports → `@/rpc/bindings/*`. `grep -rn endless-stubs crates/desktop/ui/src` must come back empty.
- **W2 / closer (engine):** methods `session_memory`, `endless_zoom`, `endless_recall(session, query, limit: Option<u8>)` (the UI passes `20`), `set_session_memory` (unused by the UI tonight; the `/endless` command drives it), **`endless_settings_get` / `endless_settings_set(EndlessConfig)`** (deviation 1); `SessionSummary.memory`; `CreateSessionOptions.memory`; `EndlessProgress` on the session stream. `SessionMemory.backfill.eta_secs` is read as `number | null`.
- **Follow-up (cheap):** `ViewLine.at` for the message date (deviation 3); `set_session_memory` wiring to a panel action if a UI toggle is wanted beyond `/endless`.
