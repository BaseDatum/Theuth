# 96 POINT-AND-SAY — as built, FIX CARD (the `[screen …]` card in the chat)

## Branch + base sha

`feat/96b-card` in `/Users/Sky.Moore/Develop/theuth-wt-96b-card`. Base `ead30467` on `main` (holds design 96 and the first UI build; the brief named `52a70f15`, which is the merge one commit below). Bindings generated here with `just ts-desktop` (gitignored).

## Commits (oldest first)

- `7fe6f27a` feat(desktop): parse the voice key's [screen …] line and [image: …] tokens (96 §6.6, CARD)
- `b65ff2ed` feat(desktop): ScreenContextCard — the voice key's screen line as a card under the user's words (96, CARD)
- (this note) docs(design): 96 as-built FIX CARD

## What changed (all under `crates/desktop/ui/src/`)

- `lib/screen-context.ts` (new). `parseScreenLines(text) → { body, screens, images }`: strips every `[image: <name>]` / `[document: <name>]` token (glued or not; `images` keeps both kinds, in order — overviews then crops for a voice-key message), then takes each whole line the §6.6 grammar admits as a `Screen`; anything else stays `body` (trimmed). `parseScreenLine`, `parseGesture`, `parseElements` exported for tests. Types `Screen`, `Gesture`, `Element` as the brief specifies (`Element.label` optional: `computer::context::element_text` writes a bare role when the element has neither label nor value). The header is one anchored regex — `"<app>"` glued to `screen i/n`, then optional ` · "<title>"`, ` · <url>`, ` · pointer: …`, mandatory ` · accessibility: on|off` — so a title holding ` · ` parses. Gestures split on `; ` outside quotes; elements are read with a sticky regex whose closing quote must be followed by ` → `, `, ` or the end, so labels with commas, `; `, `…`, `·`, raw or `\"`-escaped quotes parse. `ended on (x,y)` (the daemon's spelling when AX gave no element) → `{ kind: 'ended', at }`.
- `components/chat/ScreenContextCard.tsx` (new). Header: `ScanEye` icon, "Pointed at the screen", the app as a pill, the title truncated with `title=` tooltip, the URL as `<a target=_blank rel=noopener noreferrer>` showing it without the scheme, an outline `i/n` badge when `count > 1`, an `Accessibility on|off` outline badge at the right. One `<li>` per gesture: a lucide icon (`Circle`, `Underline`, `Timer`, `MousePointer2`), the verb ("Circled", "Underlined", "Rested 1.8 s on", "Ended on"), then the elements as `role · label` pills (a link element is an `<a>` to its URL); the row's `title=` holds the coordinates (`(x1,y1)–(x2,y2)`, `(x1,y1)→(x2,y2)`, `(x,y) · 1849 ms`, `(x,y)`). Below, `<Attachments>` (the existing thumbnail row + lightbox, `Attachments.tsx`) with the card's `files` or stored `occurrences` (removal controls included). Shell classes copied from `HarnessCards.tsx`'s `SHELL` (`rounded-xl border border-border-subtle bg-layer-1 px-4 py-3 text-sm`), full width like `SkillCard`. Exports `verb`, `coords` for tests.
- `components/chat/MessageUser.tsx:95-101` `imagesFor(xs, i, n)`: one card gets every image; with `n` cards, card `i` gets image `i` (its overview) and the last card also gets images `n..` (the crops; see Deviations). `:137-146` `parseNote` first (unchanged), then `parseScreenLines(note?.body ?? spoken?.text ?? body)`; `dataDir = useConnection(s => s.info?.data_dir)`; `paths = files ?? (occurrences || !dataDir ? undefined : images → \`${dataDir}/attachments/${name}\`)` — the token path is used only when the message has neither our optimistic `files` nor the stored row's `context.attachments`, and never for a token naming a MIME type (`[image: image/png]`, no file behind it). `:151` the top `<Attachments>` row renders only when there is no screen card (else the images belong to the cards). `:173-177` the bubble shows `words` (the body without the line and tokens); `:178-188` one `<ScreenContextCard>` per screen after the bubble, not while editing. Editing is unchanged: `beginEdit` still gets the full `body`, so a re-send keeps the screen context.

## Tests written

- `lib/screen-context.test.ts` (8): Sky's exact line (body, 4 gestures each one `window` element, 4 glued image tokens); the §6.6 doc example (app, title, URL, link + text elements, ended on a link); header with only the app and the daemon test's `[screen 2/2 · pointer: …]` with no elements anywhere; a gesture with no element list and a line with no `pointer:` field; elements with commas / `; ` / escaped and raw quotes / bare role / URL without label; a title holding ` · ` and a URL with a query string; image and document tokens without a screen line; non-screen bracket text and a malformed line stay body, two displays in order.
- `components/chat/ScreenContextCard.test.ts` (5, `renderToStaticMarkup`): Sky's exact message through `MessageUser` (voice-note caption, words in the bubble, card header, the four verbs, coordinates only in `title=`, four `Open image: <name>` thumbnails resolved under `/dd/attachments/`, no raw `[screen`/`[image:`/`accessibility: on` text, no `1/1` badge); the §6.6 card alone (title tooltip, external URL link, link chip href, `2/2` badge, `Accessibility off`, bare `ended on (x,y)` → "Ended"); `verb`/`coords` per kind; `imagesFor`; a token-only history message (thumbnail from the token, bubble without it, no card).

## Gates

1. `just ts-desktop` (nohup, `/tmp/mc-2026-10-04/CARD-ts.log`): `test result: ok. 8 passed; 0 failed; 0 ignored; 0 measured; 36 filtered out; finished in 3.19s`, then `cp -R bindings/. crates/desktop/ui/src/rpc/bindings/` (467 files present).
2. `cd crates/desktop/ui && bun install && bun run check` → `619 packages installed`; `$ tsc -b --noEmit` exit 0, no diagnostics (TypeScript 7.0.2). As in the UI as-built, `packages/apps` needed its own `bun install` first (`484 packages installed`) — the `@openuidev/*` resolution for `MessageAssistant`'s import tree; not a source change.
3. `cd crates/desktop/ui && CI=1 bun run test < /dev/null` → `Test Files  136 passed (136)` / `Tests  766 passed (766)` (first run; `pairing.test.ts` did not flake this time).
4. Alone: `CI=1 bunx vitest run src/lib/screen-context.test.ts` → `Tests  8 passed (8)`; `CI=1 bunx vitest run src/components/chat/ScreenContextCard.test.ts` → `Tests  5 passed (5)`.

## Deviations

- Base sha: the worktree's `main` was `ead30467` (one `fix(site)` commit above the brief's `52a70f15`); built on it as instructed by the task message.
- Crop placement with more than one display: §6.4 orders crops "loops first" across all displays and the line carries no crop-to-display mapping, so a crop's display is not recoverable. Decision: overview `i` on card `i`, every crop on the last card (the release display). With one display every image is on the one card, as the brief asks.
- Verb wording when a gesture has no elements: "Rested 1.8 s on" / "Ended on" lose the trailing " on" ("Rested 1.8 s", "Ended") — the brief's "the verb alone" read literally would leave a dangling preposition. "Circled"/"Underlined" are unchanged.
- `images` from `parseScreenLines` holds `[document: …]` names too (the brief names only images); `Attachments` already renders a non-image as a name chip, so a reloaded message with a PDF shows it rather than hiding it.
- `Element.label` is optional (not in the brief's shape): the daemon writes a bare `<role>` when AX returned neither label nor value, and the brief requires accepting what the daemon emits.
- No `lib/files.ts` change: `<data_dir>/attachments/<name>` through the existing `resolveFile` (via `Attachments`'s `useFileUrl`) covers the token path; `data_dir` comes from the connection store's `info` (`store/connection.ts:12`, the engine's `info()` reply), not from `lib/tauri.ts`'s `Connection` (PANEL's file; and the `info` value is per active engine, which is what a relay needs).
- Editing a message keeps the raw line and tokens in the textarea (the stored text is re-sent verbatim); hiding them there would drop the screen context from the edited turn.

## Needs from siblings

- None blocking. DAEMON2: if the `[screen …]` writer changes beyond §6.6 plus the two shapes named in the brief (bare `circled (x1,y1)–(x2,y2)`; link/text elements), the parser's tests in `lib/screen-context.test.ts` are the contract to update. A line the parser rejects falls back to plain text in the bubble, never a crash.

## Not built / Not verified

- How the card looks in the live app (dark mode, badge wrap on narrow panes, the thumbnail row's size against the bubble) — not launched here per the working rules; the orchestrator checks with Sky.
- The live path from the voice panel's send (`TurnStarted` from another client → `[image: …]` tokens → `convertFileSrc` on the local daemon): the test exercises the non-local branch (`get_attachment` pending → placeholder); the local `asset://` branch is the same `useFileUrl` the existing `files` thumbnails use.
- A message whose `[image: …]` tokens name a MIME type (a part stored without a name) shows no thumbnail and no chip for it (the token is still stripped from the words).
