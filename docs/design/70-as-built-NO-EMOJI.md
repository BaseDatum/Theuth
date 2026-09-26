# NO-EMOJI — as built (branch `fix/no-emoji-ui`)

Rule (user): "There should be no emojis in the UI. Everywhere that there is an emoji, it should be replaced with an icon."

Two commits: the desktop/apps sweep with its vitest lint, then the Rust glyph sweep with its cargo lint.

## What counted as an emoji

`\p{Extended_Pictographic} | \p{Emoji_Presentation} | \p{Regional_Indicator} | keycap | U+FE0F`. Extended_Pictographic is the
set that matters: it covers the text-default symbols (⚠ ⚙ ✖ ⏸ ☑ ▶ ℹ) that macOS/Chromium still draw with the emoji font.
Allowed as typography: the arrows block U+2190–21FF (↔ ↩ ↕), © ® ™. Not emoji, kept: ● ○ ■ ✓ ✗ ◉ ↳ ⟵ ⌘ ☐ and box drawing.

## Desktop / apps (`cf17f019`)

| Site | Was | Now |
|---|---|---|
| `components/chat/MessageTool.tsx` tool header (×2: `MessageTool`, `ReminderCard`) | `⚙` / `⏲` | `<Wrench className="size-3.5 shrink-0 text-fg-secondary" />` / `<Timer …/>` |
| `ToolRunCard.tsx`, `ScrapeCard.tsx`, `ArtifactCard.tsx::Frame` headers | `⚙` | `Wrench` (child tools keep `↳`) |
| `store/reduce.ts` reminder peer label + `MessageTool` delivered check | `⏲ Reminder r3 · after 10 min` | `Reminder r3 · …`; `MessagePeer` draws `Timer` when `from` starts with `Reminder ` |
| `sidebar/AgentTree.tsx::GLYPH` (also `BroadcastDialog` rows) | `awaiting: '⏳'`, `approval: '⚠'` | `Record<state, ReactNode>`: `<Hourglass className="inline size-3"/>`, `<TriangleAlert className="inline size-3 text-warning"/>` |
| `lib/meeting-line.ts::oracleRow`, `store/meeting.ts::oracleSummary` | `⚡ …` string prefix | plain text; `MeetingCard.OracleRow` and `ControlCenter` header render `<Zap className="size-3"/>` beside it; the island chip's tooltip is text only |
| `meeting/Island.tsx::OracleChip` | `'⚡ deciding'` | `<Zap className="size-2.5 text-fg-accent"/>` + `deciding` |
| `meeting/Island.tsx::Clock` | `'⏸'` | `<Pause className="size-3"/>` |
| `chat/surface/Graph.tsx::GLYPH.danger`, `packages/apps/src/library.ts` adjacency text | `✖` | `✗` (text fallback for a timed-out layout; matches the UI's error glyph) |
| Comments in `TerminalViewer`, `MeetingCard`, `meeting-line`, `store/meeting`, `Island` | `⌨` `⚡` | reworded |
| `rpc/bindings/OracleDecision.ts`, `Topic.ts` (generated) | `⚡` in doc comments | regenerated after the Rust doc comments changed |

Tests updated: `meeting-line.test.ts`, `store/meeting.test.ts` (no prefix), `surface/Graph.test.ts` (`✗`).

### Lint — `crates/desktop/ui/src/lib/no-emoji.test.ts`

Runs with the rest of `vitest run` (the gate `CI=1 bunx vitest run` already executes). Walks `src/**` and
`packages/apps/{src,generated,test}` (`.ts .tsx .js .jsx .css .html .json .md`, skipping `node_modules`), reports every
offender as `path:line  glyphs`, fails on any. Allowlist: a line ending in `// no-emoji: allow`. Against the previous
commit (`d346375d`, bindings regenerated) it reported **38 lines**; now 0. Timeout is 60 s: the walk is ~300 ms alone but
took 12 s inside a cold parallel run on a loaded machine.

## Rust (`75e9fda9`)

Chrome text the desktop shows comes from enums (`AgentState`, `OracleDecision.disposition`, notice kinds); no Rust string
the desktop renders carried an emoji apart from the two doc comments that leak into the TS bindings. The terminal side
cannot draw icons, so its emoji-presentation glyphs became text symbols:

| Site | Was | Now |
|---|---|---|
| `tui/src/broadcast.rs::glyph` | `⏳` `⚠` | `◔` `!` |
| `tui/src/ui.rs`, `tui/src/lib.rs` tool headers, `tui/src/model.rs::CardKind::Job` | `⚙` | `▸` |
| `tui/src/lib.rs` `-p` print mode thinking header (THINKING-TRACES) | `💭` | `┆` |
| `tui/src/model.rs` search hit line | `▶ matched message` | `▸ matched message` |
| `surface/src/render.rs` Metric flat trend / Alert info, warning / Callout tip / Image | `▶` `ℹ` `⚠` `💡` `🖼` | `▸` `i` `!` `◇` `▣` (golden files regenerated with `UPDATE_GOLDEN=1`) |
| `surface/src/graph.rs::GraphState::Danger` | `✖` | `✗` |
| `agent/src/todo.rs` `<todo>` frame (shown literally in the desktop's engine-notice body) | `▶` `☑` | `▸` `✓` (`frame.rs` fixtures updated) |
| `meeting/src/lib.rs`, `oracle.rs`, `decision/src/subject.rs` doc comments | `⚡` | words |

### Lint — `crates/tui/tests/no_emoji.rs`

`cargo test -p tui` scans `crates/{tui,surface,agent,rpc}/src/**/*.rs` with a hard-coded Unicode 16
Extended_Pictographic ∪ Emoji_Presentation range table (minus arrows/™), fails with `crate/path:line  glyphs`; same
`// no-emoji: allow` marker (used once: the `👨‍👩` width fixture in `agent/src/search.rs`). It lives in `tui` rather than
`agent`/`rpc` because those crates own no glyph strings — the desktop picks icons from kinds — while `tui` is the one
place that still draws text glyphs.

## Out of scope, deliberately

- `crates/slack`, `crates/signal`: ✅ ❌ ⚠️ 👀 ⌛ … are Slack/Signal message text and reactions, rendered by those apps,
  not by the desktop. Their reaction parsing (`decision_for_emoji`) is a feature.
- User/model content: a knowledge entry title on `/knowledge` contains ⚡ (`theuth meeting oracle as built …`); the rule
  covers chrome, not records.
- `crates/web` test fixtures (Brave HTML), `browser`/`search` unit fixtures.
- `Labs.tsx` copy "Off ↔ on" and `⌘↩` shortcut comments: arrows are typography.

## Verification

`just ts-desktop` 0 · `bunx tsc --noEmit` 0 · `bunx vite build` 0 · `CI=1 bunx vitest run` 84 files / 504 tests green ·
`cargo check --workspace --tests` 0 · `cargo test -p tui -p surface -p agent -p meeting -p decision` green (agent 319,
meeting 56+2, decision 62+3, surface 35, tui 106+1). Browser pass against the live engine (`vite --port 5191`,
`VITE_THEUTH_URL/TOKEN`): chat with tool cards (wrench headers, timer on the reminder row), agents, tasks, notes,
meetings, settings — `document.body.innerText` scanned with the same regex on every view: no chrome emoji.
Screenshots `/tmp/69/noemoji-{chat,agents,tasks,notes,meetings,settings}.png`.

DONE NO-EMOJI
