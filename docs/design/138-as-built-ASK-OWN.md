# 138 — as-built ASK-OWN: a typed answer survives changing the pick, and is sent only when it is the pick

Branch `fix/ask-own-answer` off `07f8e1f3` (v2.12.3). Brief `~/.theuth/briefs/2026-10-08/138-ui-fixes/omp-138-ASK-OWN.md`.
**The ask** (Sky, 2026-10-08 03:01): "If i change answers on a question the typed response disappears, it should persist,
but only be included if it was the selected one when I submit."
**Mechanism.** `AskCard`/`AskScreen` kept one `Draft = Answer | null` per question in component state; the `Type your own…`
box showed `draft.text` only while the draft *was* text, so an option or Skip replaced the words and the box went blank.
A second loss: an unmount (virtualised transcript, chat ↔ Attention) reset every pick and word.

## As built

- **D1 — two pieces per question.** `ask-drafts.ts` holds `{ picks: Draft[]; own: string[] }` per ask. `QuestionBlock` takes
  `own`/`onOwn` beside `draft`/`onChange`; the box always shows `own`. Both props are optional: without them the block
  behaves as before (the box is the text pick), so the exported component stays backwards compatible.
- **D2 — rules** (pure, in `ask-drafts.ts`): `typed(d,i,text)` sets own and the pick `{text}` (emptying leaves `{text:''}`,
  so `allAnswered` is false); `picked(d,i,pick)` changes the pick only (option, skipped, `null` for un-skip);
  `ownPick(pick, own)` is the focus / ○ rule — non-blank words that are not the pick become the pick, else the pick as is.
  `QuestionBlock` calls it on the box's `onFocus` and on the row's marker button (`aria-label="Use your answer"`,
  `aria-pressed`); ○ on a blank box focuses the box instead. Send maps `picks.map(toAnswer)` exactly as before: the own
  text of a question whose pick is not text is never sent.
- **D3 — visual.** Words that are the pick: `border-fg-accent` (as before). Words kept but not the pick: regular border,
  `text-fg-secondary`. The marker is a `Button size="icon-sm" variant="ghost"` (h-8, same as the box) with `ml-0.5`,
  which puts its glyph within 1 px of the option rows' markers at 1280 and 390. Locked (sent) view unchanged.
- **D4 — durability.** `askDrafts` (zustand `createStore`, in memory, no localStorage: a relaunch starts the ask fresh)
  keyed by ask id. `update(id, questions, f)` seeds from `draftsFor(questions)` + `''` on first write and writes nothing when
  `f` returns the same object. `useAskDraft(ask, prediction, live)` is the one hook both `AskCard` and `AskScreen` use;
  before the first edit it returns the seed without writing. The double's L1 pre-pick runs through the store in the
  hook's effect (`withDoublePick`, untouched pick only, identity-preserving so no entry is created when nothing applies)
  and only while `live` — `AskCard` passes `sent === null`. `AnsweredAskCard` still renders from the tool output.
- **Drop.** The entry is dropped on a successful `answer_ask` from either surface (answer or dismiss). `AskCard` also drops
  it when the engine reports the ask already closed (the card resolves it locally anyway). Decided here: an `AskResolved`
  from another UI does not drop the entry (a few strings per ask; add an `on('AskResolved')` hook in `ask-drafts.ts` if it
  ever matters).
- **Sent lock.** `AskCard`'s `sent` is now the `Answer[]` that went on the wire (was a boolean), so the locked view renders
  what was sent after the store entry is gone.
- Moved, signatures and behaviour unchanged: `Draft`, `draftsFor`, `allAnswered`, `withDoublePick`, `recommendedLabel`,
  `toAnswer` from `AskCard.tsx` to `ask-drafts.ts` (a `.ts` file the node tests can import; avoids a cycle with the hook).
  `AskScreen.tsx` and `DoubleChip.test.ts` import from the new module.

## Files · tests · gates

`crates/desktop/ui/src/components/chat/ask-drafts.ts` (new), `ask-drafts.test.ts` (new), `AskCard.tsx`,
`components/attention/AskScreen.tsx`, `DoubleChip.test.ts` (import only), this note.
`ask-drafts.test.ts`: 9 tests — seed; type → option keeps words; focus/○ rule incl. blank and already-text; skip/un-skip
and typing after a choice; emptying the box; two questions independent; Send mapping; store seed/key/share/drop;
pre-pick through the store (untouched moves, text pick stays, words untouched). Scoped run
(`ask-drafts.test.ts DoubleChip.test.ts src/components/attention`): 6 files, 35 tests passed. `bun run check`: 0 errors.
## Visual check

Throwaway vite harness under `/tmp/138-ask-harness` (not committed; heavy modules stubbed) rendering the real
`QuestionBlock` via `useAskDraft` in four states — typed answer chosen · typed answer kept while an option is chosen ·
skipped with words kept · empty — at 1280 and 390 px, light and dark. PNGs: `/tmp/138-ask-shots/ask-own-{1280,390}-{light,dark}.png`.
Measured via CDP: marker glyph centre 307 vs option 308 (1280), 51 vs 52 (390); document width never exceeds the viewport;
the box truncates/scrolls at 390 (no wrap). Driven live in the same page: focus → text pick; option → words stay dimmed;
○ → text pick again; ○ on a blank box → focuses it, pick unchanged.

## Not verified

iPad/phone touch (pointer-coarse widens the marker to 44 px; alignment there not measured), the Attention Ask screen
live against an engine, dictation finishing into the box (lands through `onChange` → `typed`, same as typing).
