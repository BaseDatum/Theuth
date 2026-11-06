# CELLHL — syntax highlighting for cell code (decisions)

Branch `fix/cell-highlight`, base `b540592e`. Sky 2026-11-06: bash and every cell language highlighted in chat cells and note cells.

## Root causes found (differ from the brief's findings)

- **Notes.** BlockNote 0.54 takes the highlighter through the `syntaxHighlighter` extension (`Editor.tsx` already registered it), not through `codeBlockOptions` (which has no `createHighlighter` in this version). Highlighting died because `prosemirror-highlight` computes every block's decorations in one `try`: BlockNote's parser calls shiki's `loadLanguage` for each block's language, and shiki *throws synchronously* for a language it has no grammar for (`cells`, `json-render`, `pwsh`, a typo). One such block in a note (a `cells` defaults block, as in Sky's note) dropped the colours of every block in that note, on every transaction. Second fault: BlockNote's own CSS paints `.shiki` with the dark colours in both themes.
- **Chat.** CodeMirror's legacy shell mode plus a 6-tag `HighlightStyle` (the brief's finding holds).

## Decisions

- **One highlighter** (`src/lib/shiki.ts`): shiki 3.23.0 `createHighlighter`, github-light + github-dark, the forgiving JS regex engine, which is what `@streamdown/code` (message fences) uses. It is loaded by dynamic `import()`, and grammars load per language. `shiki` is now a direct dependency at exactly the bun.lock version 3.23.0, because it was already the transitive one under `@streamdown/code`.
- **Unknown languages**: the highlighter's `loadLanguage` is wrapped as `async`, so a missing grammar rejects. BlockNote then remembers the language as "no grammar" and leaves only that block plain. shiki's plain-text ids (`ansi`, `plain`, `plaintext`, `txt`) map to `text`, which BlockNote skips; they never show up in `getLoadedLanguages`, so BlockNote would otherwise load them again on every refresh. A failed highlighter load is not memoised: the next caller retries.
- **H2 deviation: shiki directly, not `@streamdown/code`'s `code.highlight`.** Its result cache is keyed on `lang:themes:length:first 100 chars:last 100 chars`. In a cell over 200 characters (the sample is ~1 KB), a same-length edit in the middle (select one character, type one) returns the *stale* tokens. The cache also keeps one full token result per keystroke, forever. Same grammars, themes and engine, so the look is identical.
- **Chat cells** (`cells/shiki-marks.ts`): a `StateField` of mark decorations (`class="shiki"`, `style="--shiki-light:…;--shiki-dark:…"`) and a `ViewPlugin` that tokenizes when the editor is created, then 120 ms after the last edit. Until then the marks are mapped through the edits. A result for an outdated doc is dropped. The text is plain until the grammar loads.
  - Size cap (R1): above 100 000 characters or 5 000 lines a cell stays plain, and a cell that grows past the cap drops its marks.
  - Measured in headless Chrome on a 200-line, 40 KB cell: 1.2 ms median and 3.1 ms max per keystroke. One tokenize of a 200-line, 17 KB cell takes ~18.5 ms, which is why it runs after a pause and never per keystroke.
  - No fallback was needed.
- **Legacy CodeMirror languages dropped.** With `basicSetup={false}` nothing else used them: no indent-on-input and no comment keymap; `indentWithTab` does not use the language. `@uiw/codemirror-extensions-langs` was removed from package.json and bun.lock (`CellEditor.tsx` was its only importer), and the `HighlightStyle` was removed from `cells/theme.ts`.
- **Colours**: one global rule in `styles.css`, `html .shiki { color: var(--shiki-light) }` and `html.dark .shiki { color: var(--shiki-dark) }`, with the same `[data-snapshot-light]` exclusion as the `dark:` variant the message fences use. It covers BlockNote blocks (note cells, guest cells, plain code blocks) and CodeMirror alike. No note-cells.css rule flattened tokens: `color: inherit` sits on the block container, and token spans carry their own colour.
- **H4 alias map** (`shikiLang` in `lib/shiki.ts`): bash/zsh/sh/shellscript/shell → `shellscript`, pwsh/powershell/ps1 → `powershell`, python/py → `python`. `Lang` also has `javascript`/`typescript` (bindings), so js/javascript → `javascript` and ts/typescript → `typescript` too. Anything else passes through unchanged, so non-cell note blocks keep BlockNote's table (whose ids are shiki ids). Notes apply it through the code block's `meta.highlight` hook (`CodeBlock.tsx`); chat applies it in `tokensOf`.
- **H3 read-only code views**: the chat cell's raw `#|` header (`Cell.tsx`, the `#|` pill) and a stale note cell's "show the code that ran" (`NoteCell.tsx` `RanCode`) now render shiki tokens (`cells/CodeTokens.tsx`).
  - Left alone: outputs, terminal text, and the member-edit diff (`MemberReview`), whose +/- row colours are the content.
  - The raw header is always coloured as bash, so its `#|` lines are comments in every cell language (js/ts included).
- **`#|` header lines** are shell, python and powershell comments, so they get the comment colour in notes (unfolded or guest) and in the chat raw header.
- **Plain `$NAME`** stays the foreground colour. github-light/dark do not colour `variable.other.normal`, and the message fences behave the same. `$?`, `$(…)`, flags, strings, commands and comments are all distinct.
- **Types**: BlockNote types `createHighlighter` against shiki 4's types. shiki 3's `Highlighter` differs only in an unused option type, so `codeLanguages.ts` casts at that one boundary.
- **`CODE_BLOCK_OPTIONS` carries `createHighlighter`**, and `Editor.tsx` builds `SyntaxHighlightingExtension(CODE_BLOCK_OPTIONS)` from it instead of BlockNote's prebuilt `syntaxHighlighter`.
- **Bundle**: shiki core (`dist-*.js`, 135 kB) was already in the entry's static import graph at base, through `@streamdown/code` in `MessageAssistant`. This change adds no static shiki import. The grammars and themes are separate lazy chunks, and none is in the entry chunk `index-*.js`. BlockNote's shiki-4 grammar chunks are still emitted (`@blocknote/code-block` is kept for its language table) but are no longer fetched.

## Proof path

- Rendering used the existing dev fixtures under `bunx vite`, with a new `?hl` mode. `src/dev/cells-sample.ts` holds the brief's sample and python/pwsh cells, shown by the chat fixture (`cells-fixture.html?hl`, real `MessageAssistant` → `CellFence` → `CellEditor`) and the note fixture (`cells-notes-fixture.html?hl`, real `Editor`, owner and guest views, a `cells` block, the sample stored as `shellscript`, a plain `bash` block).
- Headless Chrome was driven over CDP, with the light/dark scheme set by `prefers-color-scheme` emulation. (The fixtures' `?theme=` parameter is not read by `theme.js`.)
- No scratch engine was started, no cell was run, and the only click was on `#|` pills and "show the code that ran".
- Screenshots are in `/tmp/cellhl/shots/`: `before-*` (base code plus the fixture) and `after-*`.
