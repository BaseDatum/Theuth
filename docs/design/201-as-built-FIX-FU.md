# 201 as-built — fix builder FU (desktop UI and TUI)

Scope: R3-2, R3-12 (UI side), R3-13 from docs/design/201-review.md. Only crates/desktop/ui/** and crates/tui/**.

## R3-2 — harness notes shown as the user's messages: fixed

Confirmed: `classify` in `lib/injected.ts` returned `null` for `<ledger>`, `<recall>`, `<tools>` (the engine persists each as
its own user row, `Turn::note` in loop_.rs: `<{kind}>\n{text}\n</{kind}>`), so they rendered as "you" bubbles; the TUI
(`model/history.rs` `user_entries`) only knew `<memory>`.

- `lib/injected.ts`: `HARNESS_NOTES` (tag → label: "Ledger updated", "Recalled from earlier", "Tools changed"), a new
  `Injected` kind `note`; `classify` takes the bare tag only (with attributes it stays the user's text, as `<memory>`);
  `isClassified` covers the three names.
- `store/reduce.ts` maps `note` to an `info` entry with a `label`; `store/sessions.ts` `info` gains `label?`.
- `components/chat/MessageInfo.tsx`: an `info` with a `label` is a muted line, collapsed by default (`Details`, store-backed
  disclosure `note`), the body under it. Plain `info` (and so `<memory>`) unchanged.
- `lib/injected-wrappers.test.ts`: `ledger_snapshot` added to `NOT_A_ROW` (rendered by `Ledger::render` into the endless
  frame via `pump.rs`, never a session row). The three note tags pass by classification, no file excluded.
- TUI `model/history.rs` `user_entries`: `agent::endless_ext::ledger_note` / `input_note` → a collapsed
  `CardKind::Memory` card titled with the same labels.
- Tests: `reduce.test.ts` "201 R3-2 …" (each kind, attribute and prose variants stay user rows);
  `components/chat/MessageInfo.test.ts` (collapsed shows only the label, opened shows the body); TUI
  `model::tests::harness_notes_are_labelled_cards_not_user_rows`.

Deviation: live turns do not show the notes until reload (they are persisted rows with no live event); unchanged behaviour,
not in scope.

## R3-12 (UI side) — fixed

Confirmed: `toWire` wrote `Number(raw) * 1000` (24.0005 KB → 24000.5 bytes); `tailError` checked only a full pair.
- `RuntimeAccessFields.tsx`: `tailWire(raw, fixed)` (exported, used by both mark inputs) rounds KB to whole bytes.
- `wizard.ts`: `tailError(d, tails?)` pairs a lone mark with Settings › Context's other mark (`TailDefaults`, engine
  defaults `ENDLESS_DEFAULTS` while unknown) and, when the order fails because of the inherited mark, appends
  "… is empty, so it is Settings › Context's N KB; set it here too."; `validate` / `firstInvalid` take `tails?`.
- `queries.ts` `useEndlessSettings()` (`endless_settings_get`); `TailFields` and `HireWizard` pass its data.
- Test: `RuntimeAccessFields.test.ts` "201 R3-12 …".
The roster-side validation (file.rs) is the Rust builders' half of R3-12; not touched here.

## R3-13 — fixed

- One estimate: the page's existing ~3 bytes/token (`tokenEstimate`, `scaledMarks`); `tokenEstimate` now reads
  "about N k tokens", the scaled-mark hints "· about N KB". Not changed to the engine's per-model rate (`tokens::bytes_per_token`):
  the brief asked for consistency with the page.
- "Describe images" no longer mentions PDFs.
- Suggestions help: "Phrases in your own messages that read as a decision or a constraint are offered in the ledger as
  unconfirmed suggestions, for the model to confirm or drop." (confirmed: `Ledger::trim` cues only `Kind::User` units).
  Label "Show summary-model suggestions" kept (no renames).
- MemoryPanel: `TailSize` trigger `aria-label="Tail size setting"`, `TailBar` meter `aria-label="Tail fill"`.
- Trim now keeps its label; its help line and the button `title` add `TRIM_INSTEAD`: "In meetings and while history is
  still being summarised this folds the view instead."
- Tests updated: `Memory.test.ts`, `MemoryPanel.test.ts`.
