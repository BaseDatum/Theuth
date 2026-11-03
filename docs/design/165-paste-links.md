# 165 as-built — rich paste keeps hyperlinks in the composer

Branch `feat/165-paste-links`, base `f4ae771c` (163 on main). Brief: `~/.theuth/briefs/2026-11-03/165/BRIEF-165.md`.

## The ask (Sky, 2026-11-03)

> If I copy a microsoft teams message that has hyperlinks, and then paste it into the UI in theuth, It seems to just paste the text and doesn't actually capture the links properly. is that something we can fix in the app? I'm on macos right now when I saw that.

On the choices offered: start now. On Safe Links:

> yes unwrap to real address, but this should work copying an embedded hyperlink from more than just microsoft products. there has to be a library for typescript or javascript that just handles this automatically.

## Cause

`Composer.tsx` `onPaste` turned an image item into an attachment. For anything else it read only `clipboardData.getData('text/plain')`, ran it through `tokenize` and edited the textarea. Rich sources (Teams, Outlook, browsers, Google Docs, Slack, Confluence, Notion, Word) also put a `text/html` flavour on the clipboard. The links exist only in that HTML, so they were dropped. `Composer` backs every chat box (chat route, overview, NoteChat, talk, Hud, notebooks ChatColumn/AskView, NewChatPane), so one fix covers all of them.

## As built

### Files
- `crates/desktop/ui/src/lib/paste-markdown.ts` (new, pure). It exports:
  - `pasteText({ html, plain })`: the whole decision;
  - `htmlToMarkdown(html)`;
  - `unwrapUrl(href)`.

  The Turndown instance is created lazily on the first rich paste and then reused.
- `crates/desktop/ui/src/lib/paste-markdown.test.ts`, plus 12 fixtures with goldens under `src/lib/__fixtures__/paste/`.
- `crates/desktop/ui/src/components/chat/Composer.tsx` `onPaste`: the image branch stays first and unchanged. After it, `pasteText({ html: getData('text/html'), plain: getData('text/plain') })` feeds `tokenize` where `plain` used to. The early return when there is nothing to insert stays.
- `crates/desktop/src/lib.rs`: the macOS Edit › Paste as Plain Text item (D5).
- `crates/desktop/ui/package.json` + `bun.lock`: `turndown ^7.2` (resolved to 7.2.4) and dev `@types/turndown ^5.0.6`. Turndown ships no types.

### D1 — library: Turndown, own GFM table rule
- `turndown` 7.2.4 is MIT (`node_modules/turndown/LICENSE`, Dom Christie). `@types/turndown` is MIT too.
- Size: the browser ES build Vite bundles is 25.1 kB raw. Minified with esbuild it is 10.9 kB, and 4.1 kB gzip. The browser build parses with `DOMParser`, which yields an inert document (no scripts run, no images load). The node build used by vitest bundles `@mixmark-io/domino`.
- **Tables: own rule, not a plugin.**
  - `turndown-plugin-gfm` (last release 2018) emits a table without a header row as raw HTML (`keep`). Excel, Sheets and most mail tables have no header row, so their paste would come out worse than today's plain text.
  - `@joplin/turndown-plugin-gfm` is written against Joplin's Turndown fork.
  - The own rule is about 30 lines with no extra dependency:
    - the first row is the header, and the separator row is padded to the widest row;
    - newlines inside a cell become spaces and `|` becomes `\|`;
    - `thead`/`tbody`/`tfoot` pass their rows through;
    - a single-column table is page layout (Outlook mail bodies, signatures), so its cells read as paragraphs instead of a one-column table.

### D2 — gate
- A cheap pre-check comes first: `/<a\s[^>]*href/i` on the HTML. Code copied from VS Code is large HTML with no anchors and never reaches Turndown.
- The exact gate is decided inside the conversion. The link rule counts a link only when its href, after `unwrapUrl`, is `http:`, `https:` or `mailto:` and its visible text is not empty. With a count of 0 (or empty markdown), `pasteText` returns `plain`, so the paste behaves exactly as before, apart from D4's unwrapping of bare wrapper URLs.
- This avoids a second, regex-based HTML parse that could disagree with Turndown about what a link is.
- The image branch is untouched and still first.

### D3 — conversion
- Options: `headingStyle: 'atx'`, `bulletListMarker: '-'`, `codeBlockStyle: 'fenced'`, `emDelimiter: '_'`, `strongDelimiter: '**'`, `br: ''`. A `<br>` is a plain `\n`, never a two-space hard break.
- Post-processing:
  - nbsp → space;
  - trailing spaces/tabs per line stripped;
  - 3+ newlines → 2;
  - ends trimmed;
  - wrapper URLs in the text unwrapped (D4).
- `escape` is the identity, so `file_name_v2`, `1. step`, `*note*`, `# x` and `a\b` paste exactly as written.
- Links:
  - `[text](url)`. Text spanning lines is joined with spaces, so a link around a block never yields a broken `[\n\ntext\n\n](…)`.
  - When the text is the URL (ignoring scheme, `www.`, a trailing `/` and case), the bare href is emitted.
  - A `mailto:` whose text is the address emits the bare address.
  - In the destination, space and `(` / `)` are percent-encoded (`%20`, `%28`, `%29`). That is unambiguous in every markdown renderer, where Turndown's `\(` is noise.
  - Any other scheme (`javascript:`, relative, `#anchor`, `file:`) keeps only the text, and a link with empty text emits nothing.
- `<img>` → its alt when the alt is ≤ 16 code points (emoji: Teams puts the character there, Slack `:tada:`). Otherwise it is dropped.
  - **Added:** an image whose `width` or `height` attribute is > 64 is dropped even with a short alt. A Teams screenshot is `alt="image" width="640"`, which would otherwise paste the word "image".
- Dropped: `style`, `script`, `meta`, `title`, `head`, `noscript`. Comments, Office conditional comments (`<!--[if …]>…<![endif]-->`) and downlevel-revealed markers (`<![if …]>`) parse as comment nodes, which Turndown skips.
- Word/Outlook artefacts:
  - `<o:p>` filler is dropped;
  - the `mso-list:Ignore` bullet/number glyph is dropped;
  - a `MsoListParagraph*` paragraph becomes a `- ` item, or `1. ` when the dropped glyph was a number.

  Without this the list would paste as `·        Timeline`.
- **Added:** list items are `- item` / `1. item` with continuation lines indented to the text. Turndown's own rule emits `-   item` (three spaces), which is not what you'd type.
- Google Docs:
  - `<b>`/`<strong>` styled `font-weight:normal|100–400` (the `docs-internal-guid` selection wrapper) passes its content through;
  - `<span style="font-weight:600–900|bold">` → `**…**` (not inside a heading, so you get `## Q3 plan` rather than `## **Q3 plan**`);
  - `font-style:italic` → `_…_`.
- **Added:** a bare `<pre>` (Slack's `c-mrkdwn__pre`, many pages) is fenced too. Turndown fences only `<pre><code>`.

### D4 — unwrap redirect wrappers
- A table-driven `unwrapUrl`: one `{ host, path, params }` row per wrapper.
  - Safe Links: any `*.safelinks.protection.outlook.com`, `url`.
  - Google: `(www.)google.<tld>[.<cc>]/url`, `q` then `url`.
  - Slack: `slack-redir.net/link`, `url`.
- `URLSearchParams` decodes the parameter once. Only an absolute `http(s)://` result that parses as a URL is accepted. Otherwise the original is kept, so double-encoded values, `javascript:`, relative values and garbage all stay as they are.
- **Added:** up to 3 nested unwraps (Safe Links around a Google redirect happens in forwarded mail).
- Applied to every href on the HTML path, and to bare wrapper URLs in the inserted text on either path. The text scan matches `https?://` runs without whitespace, `<>"'()[]`, and puts trailing `.,;:!?` back after unwrapping. A non-wrapper URL comes back byte-identical.

### D5 — Paste as Plain Text (⇧⌘V)
- Not already bound: no menu accelerator, Tauri global shortcut, `chord.ts` binding or keydown handler in `crates/desktop` or `packages` uses ⇧⌘V. The brief's ⌥⇧⌘V fallback was not needed.
- `lib.rs` `app_menu` → Edit gets `MenuItem::with_id(PASTE_PLAIN = "paste_plain", "Paste as Plain Text", Some(PASTE_PLAIN_ACCEL = "Shift+CmdOrCtrl+V"))` right after Paste. `on_menu` routes the id to `paste_plain` (all `cfg(target_os = "macos")`).
- `paste_plain` finds the focused webview window (main or a popout), then calls `with_webview` (main thread). It checks `responds_to(pasteAsPlainText:)` on the WKWebView's class and sends `-[WKWebView pasteAsPlainText: nil]` through `objc2::msg_send!`.
  - WebKit's `Editor::pasteAsPlainText` dispatches a `paste` whose `DataTransfer` is a static pasteboard holding only `text/plain` (`ClipboardEventKind::PasteAsPlainText`). The D2 gate therefore takes the plain path.
  - The responds check turns a WebKit without the selector into a no-op instead of an Objective-C exception.
- Tauri drops an accelerator it cannot parse without any error (`accelerator.and_then(|s| s.parse().ok())`). The unit test `paste_plain_accelerator_parses` parses the constant with the same grammar (`global-hotkey`'s `Shortcut`, which `muda` shares).
- Windows (WebView2) and Chromium web clients already paste plain on Ctrl/⌘+Shift+V (no `text/html` in the event), so nothing was built there. The Safari web client has ⌥⇧⌘V natively. Linux (WebKitGTK) is not handled: the app has no native menu bar there (`lib.rs`: GTK would paint it inside the window).
- **The live ⇧⌘V check in the real app is Sky's.** Workers may not launch the desktop app. Proven here: it compiles, clippy is clean, the accelerator test passes, and the plain-only `DataTransfer` that WebKit delivers pastes exactly as before in real Chromium and WebKit (proof below).

### D6 — tokenize unchanged
The markdown goes through `tokenize(…)` exactly like plain text. A long result (> 5 lines or > 400 chars) is held as a `⟦paste n⟧` chip: the Outlook and Chrome fixtures did this in both browsers, and the chip dialog showed the markdown. Undo/redo is untouched.

### D7 — scope
Only `Composer.tsx`. See follow-ups.

## Deviations from the brief
1. **nbsp → space in code too.** The brief said "outside code". Turndown moves an element's edge whitespace (nbsp counts, since its regex uses `\s`) outside the element and emits it unescaped, so the `escape` hook cannot see it. Safari's `Apple-converted-space` hit exactly this. The conversion therefore happens once on the output. An nbsp inside pasted code is a copy artefact that breaks code (Python indentation, shell), so a space is the better result there too.
2. **3+ newline collapse applies inside fenced code as well** (marked with a `ponytail:` comment). Splitting on fences can be added if someone pastes code with deliberate runs of blank lines.
3. Additions beyond the brief, each recorded above: the size cap on images, `- ` list markers, Word list paragraphs, bare `<pre>` fencing, layout tables as paragraphs, nested unwrapping, and joining multi-line link text.
4. Fixture goldens write emoji as `{U+XXXX}` and the test expands them, because `no-emoji.test.ts` scans `.md` under `src`. The Teams HTML fixture uses `&#x1F44D;`.
5. tsc baseline: prep captured `/tmp/165/tsc-baseline.txt` after `paste-markdown.ts` existed but before `@types/turndown` was installed. All 21 of its lines are this file's implicit-`any` errors. The real baseline on main is 0, and the branch is 0.

## Proof
- **Unit** (vitest, node env): `src/lib/paste-markdown.test.ts`, 21 tests.
  - Every fixture's golden: Teams (Safe Links, mention span, emoji img, screenshot img, `<p>`/`<br>`), Outlook (Word HTML, Mso classes, conditional comments, Safe Links, mso list), Chrome (`<meta charset>`, styled spans, inline code, ul/ol, `<pre><code>`), Safari (`Apple-converted-space`, `Apple-interchange-newline`), Google Docs (guid wrapper, `google.com/url?q=` links, encoded and unencoded, font-weight/italic spans, list), Slack (slack-redir, emoji alt, mention, `<br>`, inline code, bare `<pre>`), Confluence (table with links, `|` in a cell, multi-paragraph cell), Notion (nested list, strong/em/code), VS Code (no links → exactly `plain`), link text = URL variants, mailto, and `javascript:`/relative/`#`/`file:`/empty links.
  - Gate cases: no HTML, HTML without links, image only, only bad hrefs, empty-text links, `<a name>`, empty everything.
  - Unwrap table: each wrapper, nested, double-encoded kept, non-http kept, a look-alike host kept, the wrong path kept, garbage and empty kept.
  - Plain-path unwrapping, no escaping, nbsp, newline handling, layout vs GFM tables, and idempotence (`pasteText` of its own output as plain is unchanged).
- **Real browsers, real composer.** This worktree's UI ran on `bunx vite --port 1465 --strictPort` against a scratch headless engine (installed `/Applications/theuth.app/Contents/MacOS/t`, `--data-dir /tmp/165/eng`, block-style config with every plugin off). Playwright Chromium (headless shell 1226) and WebKit 2370, headless, each case in a fresh context. Each case focuses the overview composer (`Ask anything`) and dispatches a synthetic `ClipboardEvent('paste')` with a `DataTransfer`.
  - Cases: Teams, Outlook, Google Docs and Chrome (`text/html` + `text/plain`); plain-only (the ⇧⌘V shape); plain-only with a Safe Link; VS Code (HTML without links); a 12-line plain paste.
  - Result: 16/16 PASS (8 cases × 2 browsers). The inserted text equals the golden, or `plain` for the plain cases. For chip cases the dialog textarea was read. Every event was `defaultPrevented`.
  - Artifacts: `/tmp/165/proof/{chromium,webkit}-<case>-{before,after}.png`, `/tmp/165/proof/results.json`, and the driver `/tmp/165/proof/proof.mjs`. No message was sent, and the system clipboard was never touched.
- **Gates:**
  - `cargo fmt -p desktop`: clean after one fmt commit.
  - `mbx clippy -p desktop --all-targets --no-deps -- -D warnings`: exit 0.
  - `mbx test -p desktop --lib`: 55 passed, including `paste_plain_accelerator_parses`.
  - tsc baseline diff: empty (0 errors).
  - vitest: `paste-markdown`, `no-emoji`, `pastes`, `Composer`, `NewChatPane`, `ChatColumn`, `AskView`, `Hud`, `Voice` and `voice-key`, 10 files / 70 tests green.

## Open items
- Live ⇧⌘V in the desktop app (Sky): in a chat box, ⇧⌘V on a Teams message should insert the plain text, and ⌘V should insert markdown links.
- Fixtures are synthetic, built from each app's known clipboard shape. If a real Teams/Outlook paste comes out wrong, save its `text/html` flavour as a new fixture: `e.clipboardData.getData('text/html')` in devtools, which only reads the clipboard.
- Slack and Confluence mentions are anchors to profile pages, so they paste as `[@sky](https://….slack.com/team/U…)`. This is harmless but noisy. Drop them to text (`data-stringify-type="mention"`, `confluence-userlink`) if it bothers anyone.
- An ordered list split across separate `<ul>`/`<ol>` elements (Notion emits one per item) comes out as a loose list with blank lines between items. This is faithful to the HTML.
- Linux WebKitGTK has no ⇧⌘V / plain-paste escape hatch.

## Follow-ups (D7: other textareas still paste plain)
`BriefEditor` (briefs), `NewTaskSheet` and `TaskDetail` (task brief), `RoutineDetail` and `RoutineChat` (routines), `HireWizard`, `HiringInbox`, `ReviewScreen`, `AskCard`, `AskDialog`, `BroadcastDialog`, `MessageUser` (edit), `Peers`, `Skills`, `Tools`, `StartForm` (meeting), `AddSourceMenu`, `SidePanel` (autoresearch), `PairScreen`, and the chip editor in `HeldStrip`. Wiring one is a single `onPaste` that calls `pasteText`, then inserts at the selection.
