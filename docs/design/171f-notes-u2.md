# 171F-U2 notes — user bubble: parked cell results, long pastes, result labels

## B4 — parked cell results
- `lib/injected.ts` `splitLeading(text)` peels only leading `<cell_result>` blocks (parser shared with `classify` via
  `cellResult`); `rest` is left-trimmed when blocks were peeled, else the input untouched (so `ctx.sent` matching of a
  plain send is unchanged). An unterminated or run-less block stops the peel and stays text.
- `store/reduce.ts` TurnStarted: `rest` is classified as before. Own send/steer → `stampUser` stamps and attaches
  `cells` (small signature change to `stampUser`, a helper only TurnStarted calls). Not own and not wrapped → one echoed
  user entry with `cells`. Wrapped `rest` — `''` (review / Ask-theuth wake) or another wrapper such as `<message>`
  after the results — appends one `cell_result` card per result, matching the reload (results followed by a non-user
  row are not folded). The `splitLeading` import on line 4 is outside the listed hunks; it is required by the hunk.
- `MessageDelivered` with `from` starting `"cell "` returns the slice unchanged (no card, no `live` bump: nothing on
  screen changed). A sender like `cellar` still gets its card.
- `foldCells` lives in `store/sessions.ts` (exported, tested from `reduce.test.ts`). Applied to the loaded page and,
  for `prepend`, to the joined list, so a page boundary between the parked rows and the user's row still folds.
  The folded user entry takes `run[0].turn` (`cell_result` history entries now carry `turn`), so `adopt` pairs it with
  the live entry by `u<turn>\n<text>` and Edit/Fork return. The cards' own `context` (removal targets of the result
  rows) is dropped on fold; the user row's context stays.
- UI: `MessageUser` takes `id` (optional; tests render without it) and `cells`; `SentResults` is one collapsed row
  ("N cell results sent with this message", singular for 1) keyed `disclosureKey(session, id, 'cells')`, expanding to
  `ResultCard` lines (attributes only, so a private cell's output never shows).

## B6 — long text
- `lib/long-text.ts`: `longText` counts lines after CRLF/CR normalisation and characters as code points; `showAll`
  labels. `LongText` keeps the full text in the DOM, clamps with `line-clamp-8` + Tailwind v4 `mask-b-from-60%` fade,
  toggle `min-h-11`, `CopyButton`, the toggle row stops propagation (bubble stamp toggle). Applied to the plain words
  and each Slack text segment (`disclosureKey(session, id, 'text', i)`); `EditBubble` untouched.

## B20 — result label
- `label` parsed (unescaped) from the tag; `ResultCard` shows `label ?? name ?? cell N` (run as last resort when the tag
  has neither) and no longer reads the cells store, so its `session` prop is gone (`Message.tsx` call updated).
