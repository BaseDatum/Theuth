# 138 UNDO as built — native undo/redo survives every composer edit

Branch `fix/composer-undo` on `07f8e1f3` (v2.12.3). Brief: `~/.theuth/briefs/2026-10-08/138-ui-fixes/omp-138-UNDO.md`.
## The ask
Sky, 2026-10-08: "Cmd+z and such don't work in chat input on desktop apps."

## Reproduction (orchestrator, installed 2.12.3, main-window composer)
Typing `alpha beta gamma` then ⌘Z/⇧⌘Z works (the macOS Edit menu and wry's `performKeyEquivalent` are fine — no Rust
change). ⌘A ⌘C → ⌘V → ⌘Z: nothing changes, the caret jumps to the start; a second ⌘Z is dead. Typing ` zz` after the paste
and ⌘Z undoes only ` zz`. A paste was never undoable and destroyed the history before it; every edit the composer makes
itself (slash/@/path pick, chip ✕, dictation, withdraw) behaved the same — that is the "⌘Z doesn't work" Sky meets.

## Mechanism
The composer is a controlled `<textarea value={text}>` over the `useDrafts` store. Typing: the browser edits the DOM,
`onChange` copies into the store, React sees an equal value and never writes `node.value`. A composer-made edit was a
store write → React sets `node.value` → not an editing command, so WebKit/Blink record no undo step and the earlier steps
no longer apply. The only correct path was New line's `document.execCommand('insertLineBreak')`.

## What changed
- `lib/text-edit.ts` (new): `diffRange(prev, next)` — smallest single replacement (prefix, then a non-overlapping suffix;
  never splits a surrogate pair); `editText(ta, start, end, text, doc)` — focus if needed (`preventScroll`), select,
  `execCommand('insertText')` (`'delete'` for an empty replacement); `true` only when the command took AND the value
  matches, `false` with no call on read-only/disabled; `applyText` = diff + edit. Structural `ta`/`doc` types for node
  tests. The command's `input` runs the normal `onChange`, so nothing calls `setText` on success.
- `lib/pastes.ts`: `Shelf` + `reconcileShelved(text, pastes, shelf)` — a text block whose token left moves to the shelf; a
  shelved block whose token is back returns with its original content; same maps back when nothing moved. Optional
  `floor` on `tokenize`/`attach`/`attachRef`/`hold` (default 0: existing callers/tests unchanged). `reconcile`/`expand` as is.
- `components/chat/Composer.tsx` (edit paths only; `DraftCreate` and the `create_session` block untouched): `edit(next,
  caret?)` = `applyText` on the mounted textarea, else store write + shelf reconcile; `reconcileHeld(next)` = functional
  `setPastes` through the per-composer `shelf` ref (a paste holds its block BEFORE `execCommand` fires the nested
  `onChange`); `floor()` = top shelved key, passed to every `tokenize`/`attach`/`attachRef`. `pendingCaret` → `pending:
  { text?, caret }`: the dictation finish stores `{ value, caret }`, the `[pending, dictating]` effect applies it with
  `applyText` once the textarea is back, then places the caret. `insert()` folded into `onPaste`. Shelf reset on
  `draftKey` change, in `reset()` and after a send is acked.

## Undoable now (one step each; ⇧⌘Z redoes) vs. deliberately not
Undoable: paste (inline and big-token), slash pick, @-mention strip, path pick, held chip ✕ (text block — file/ref chips
have no token and keep the plain `drop`), withdraw last queued, queued-chip edit, dictation finish. Caret rules kept via
`pending`: path pick → `next.caret`, @-strip → `atWord.start`, dictation → the splice caret; the rest take the command's.
Plain store writes: send / `reset()` / `acked` clearing, `prefillDraft` / `appendDraft`, skill-draft words, voice-note
`spoken` fallback, `retext` from the chip editor, image/file attach. The dictation layout unmounts the textarea, so the
history from before a dictation does not survive the remount; the dictated insertion itself is one step.

## Shelf and key rules · touch rule
A block never dies when its token disappears (undo, ✕, overwrite) — it waits on the shelf and returns only with its own
token. New keys = max(held ∪ shelved) + 1, so a redo can never pick up a newer paste's content. On a coarse pointer
(`touch()`, `lib/platform`) `edit` does not focus an unfocused textarea just to be undoable (the keyboard would pop on a
chip ✕): store-write fallback. Paste always has focus; `picked()` focuses first (as before); the dictation effect focuses.

## D4 proof — headless Chrome via CDP (throwaway harness under /tmp, not committed)
Controlled textarea + zustand store like the composer, real `lib/pastes.ts` and `lib/text-edit.ts`; `execCommand` inside
React's `paste` handler (A, B), a click handler (C), outside any React handler (D); old store-write pattern as control (E).
Output, 19/19 PASS: A0 typed text reaches the store · A1 paste inserts via execCommand, store agrees · A2 undo restores
the pre-paste value, store agrees · A3 redo re-applies the paste · A4 a further undo reaches the typing before the paste ·
B1 big paste lands as token with block 1 · B2 undo removes the token; block shelved, not lost · B3 redo brings token and
its original block back · B4 a new big paste while 1 is shelved takes key 2 (floor) · B5 undo/redo of paste 2 restores its
own block · C1 chip ✕ removes the token as an edit; block shelved · C2 undo of the ✕ brings token and block back · C3 one
more undo takes the paste itself · D1 edit outside React replaces the text, store agrees · D2 undo restores what was typed
· D3 redo re-applies it · E1 legacy paste lands via store write · E2 control: legacy undo does NOT restore the pre-paste
value · E3 control: legacy history before the paste is gone too.

## Gates (build-lock, niced)
`bun run check`: 0 errors. Scoped line (`text-edit`, `pastes`, `drafts`, `composer-state`, `dictation` tests): 5 files,
34 tests passed. `vitest run src/components/chat`: 46 files, 487 tests passed. Added: 9 tests in `text-edit.test.ts`,
3 in `pastes.test.ts` (undo/redo round-trip, ✕ via the shelf, key floor).

## Unverified
WebKit/WKWebView (orchestrator re-checks in Safari; a refused `insertText` makes `applyText` return `false` → fallback),
Windows WebView2, iPad hardware keyboard under the touch rule, the live dictation remount (the effect re-runs on
`dictating`, so either order of store flush and remount applies the text).
