# 120 as built — W3 UI (HtmlPage, bootstrap, theme bridge, card/viewer, typed-blob PDFs)

Branch `feat/120-ui` off `fd193e7e`. TypeScript only (`crates/desktop/ui`); no Rust, no `tauri.conf.json`, no
`rpc/bindings`, no `packages/**` touched. Design sections built: §3, §4.3, §4.5, §5 (5.1–5.4), §6, §7, §9b, §12.3
(minus the W4 row), §13.2.

## What was built, where

| file | what |
|---|---|
| `src/lib/page-bootstrap.ts` (new) | `sanitizeVariables` (:25, names `^--[a-z0-9-]+$`, values stripped of `;{}<>`), `hostContext`/`hostContextMessage` (:36, :42), `themeFragment` (:45), `result(id)` (:48), `BASE_CSS` (:56), `DEFAULT_STYLE` (:61: dark `:root`, light `prefers-color-scheme` block, base — the two palettes baked in), `BOOTSTRAP_JS` (:72, the in-page script), `injectBootstrap` (:199, §5.1 rules on a blanked scan copy: comments, `script style textarea title xmp iframe noembed noframes noscript plaintext template`; after `<head>`, else wrapped after `<html>`, else after the doctype, else prepended with `<!doctype html>`; charset kept when the first 4 KiB declare one, viewport kept when present), `readSizeChanged` (:227), `readOpenLink` (:234, `^https?://` only), `readConsole` (:244, 500-char cut) |
| `src/lib/page-theme.ts` (new) | `SOURCES` table (:17): the forty T3 names → theuth token + light/dark literal from `styles.css`; `PAGE_VARIABLES` (:61), `pageDefaults(appearance)` (:64), `pageThemeFor(appearance)` (:86, the class-lift trick in both directions), `useHtmlPageTheme()` (:99, `subscribeDark` + `useMemo`). `--accent` = `--fg-accent` (ochre), `--chart-1..6` = ochre, faience, malachite, lapis, carnelian, plum |
| `src/lib/pages.ts` (new) | `pagePut(call_id, html)` → `invoke<string>('page_put', { call_id, html })` (:8); `pageDrop(call_id)` → `invoke('page_drop', { call_id })` (:11). The `isTauri()` switch is `lib/tauri.ts:18`, reused |
| `src/components/chat/HtmlPage.tsx` (new) | `MIN_HEIGHT 80`/`MAX_HEIGHT 2000`/`RESERVE 240` (:24-27); `Cap = number \| 'none' \| undefined` (:30); `capFromArgs(tool.args)` (:35); `frameHeight(report, cap)` (:41, §6); `errorsAfter` (:48, `error` level only, ≤ 20); `handleBridge` (:65: source must be this frame's window; size → height; link → gated open + `result(id)` answer always; console → chip); the component (:79): `page_put`/`srcdoc` by `isTauri()`, `<iframe sandbox="allow-scripts allow-forms" src={url + themeFragment} \| srcDoc title loading="lazy" className="block w-full border-0 bg-transparent" style={{height}}>`, `host-context-changed` on `load` and on every theme flip, link gate `document.activeElement === frame && navigator.userActivation?.isActive !== false` → `window.open(url, '_blank', 'noopener,noreferrer')`, `bg-layer-2` pulse until `load`, `Unable to load <title>` box at the same height with the toolbar kept, error chip `N error(s)` expanding to the list, `page_drop` on unmount / bytes change |
| `src/components/chat/ArtifactCard.tsx` | `captionLine(a)` (:121, `title · HTML · 12 KB · v2`); `Header` (:346) and `PanelButton` (:356) factored out of `Frame`; `kind === "html"` branch (:448): header row, `<HtmlPage cap={capFromArgs(tool.args)}>` with Reveal / Open in panel / Pop out / More in the hover toolbar, one caption line that opens the panel; text kinds including `html` now fetch their bytes (:442) |
| `src/components/artifacts/Viewer.tsx` | `PdfFrame` (:273) → `useDocumentUrl` (typed `blob:`); `Fenced` (:293) shared by the `text` arm and a page's Source; `HtmlBody` (:302) `Page \| Source` `ToggleGroup` + `<HtmlPage cap="none">`; `Body` routes `html` to it (:345); the inert `srcDoc={CSP + text}` default arm stays for file-like fallbacks (:370) |
| `src/components/chat/Attachments.tsx` | `useDocumentUrl(path)` (:42, `resolveDocument` → `null`/url/`''`, revoked on unmount); lightbox `Original` split into `OriginalImage` (:136, `useFileUrl`) and `OriginalDocument` (:144, typed `blob:`) |
| `src/lib/files.ts` | `readBytes` typed `Uint8Array<ArrayBuffer>` (:91); `resolveDocument(api, path)` (:100): always RPC → `new Blob([bytes], { type: 'application/pdf' })` object URL, never `convertFileSrc`; `resolveFile` unchanged for `<img>` |
| `src/components/chat/ArtifactPanel.tsx` | untouched — the toggle lives inside `Viewer.Body`, so the panel, the explorer and a pop-out all get it with no slot |

vitest (§13.2 names, all present): `src/lib/page-bootstrap.test.ts` — `injects_at_start_of_head`, `wraps_when_no_head`,
`ignores_head_in_comment_script_and_template`, `keeps_existing_charset_and_viewport`,
`whitelists_variable_names_and_strips_delimiters`, `size_changed_requires_finite_positive`,
`open_link_rejects_javascript_file_and_custom_schemes`, `console_caps_at_twenty_by_five_hundred` (+ two more that run
the real `BOOTSTRAP_JS` under `new Function` with fake `window`/`document`/`console`: theme apply/fragment/flip/size
dedupe, and the click interception). `src/components/chat/HtmlPage.test.ts` —
`reserves_height_or_240_before_first_post`, `clamps_to_cap_and_2000`, `ignores_messages_from_other_windows`,
`link_opens_only_with_focus_and_activation`, `error_chip_counts_errors_only`, `shows_unable_to_load_on_failure`.
`src/lib/page-theme.test.ts` — `maps_all_forty_names` (T3 list pinned verbatim), `accent_is_the_brand_token`, plus the
class-lift round trip. `src/lib/files.test.ts` — `documents_never_use_asset_urls`.

## Gates

```
cd crates/desktop/ui && bun run check
  33 × "error TS" — all pre-existing (seeded bindings predate design 109: reduce.ts, sessions.ts, ResumeBanner …); 0 in W3 files
  (baseline on fd193e7e before any W3 edit: 33)
  error: script "check" exited with code 1
cd crates/desktop/ui && CI=1 bun run test
   Test Files  1 failed | 150 passed (151)
        Tests  1 failed | 872 passed (873)
     Duration  128.72s
  the failure: src/rpc/api.test.ts › "METHODS equals the server method list" (331 vs 330 methods — the seeded
  bindings/methods.json; fails on HEAD with every W3 edit stashed). src/lib/mobile-zoom.test.ts timed out once
  (5 s budget, load ~60) in a second full run and passes alone.
  All 20 W3 tests pass (page-bootstrap 10, HtmlPage 6, page-theme 3, files 1).
cd crates/desktop/ui && bunx vite build
  ✓ 13683 modules transformed.
  ✓ built in 1m 49s
  EXIT build 0
```

## Smoke (real Chromium, the `src` path)

The desktop binary has no `page_put` tonight, so the bootstrapped document was written to disk by a bun script
(`injectBootstrap` over a sample page), served over HTTP and framed as `<iframe sandbox="allow-scripts allow-forms"
src="/page.html#theuth-theme=<light>">` in a harness that logs every message whose `source` is the frame. Observed:
origin `null`; `<style id="theuth-theme">` = light before first paint, `location.href` without the fragment; the
page's own `:root{--chart-9:x}` kept; first `size-changed` = 615 (`scrollHeight` > 240 px viewport); console
`log` (`hi {"a":1}`), `error` (`bad`) and the uncaught `Error: late` with its stack all forwarded; a
`host-context-changed` to dark rewrote the style (`;}` stripped from a value, a non-`--` name dropped) and the computed
background flipped `rgb(232,220,200)` → `rgb(18,16,14)`; clicking the `https:` link posted `ui/open-link` id
`theuth-link-1` and did not navigate, `#h` navigated same-document, `javascript:` posted nothing and navigated nowhere.
The `srcdoc` fallback could not be smoked in that Chromium: omp's managed browser runs no script in any `srcdoc`
frame, sandboxed or not (a trivial `<script>document.body.textContent='RAN'</script>` probe stayed inert) — an
environment limit, not a code path difference; the vitest `new Function` runs cover the same script.

## Deviations from the design, with reasons

- **One `page_put` key per mount** (`HtmlPage.tsx` publish effect): the key is `` `${callId}${useId()}` ``, not the
  bare `call_id`. The card, the panel and a pop-out can frame the same call at once; a `page_drop(call_id)` from one
  unmount would 404 the others. Within the frozen contract (the argument is a string); W2's state is keyed by token so
  nothing on its side changes. See Contract notes.
- **The theme fragment is fixed at mount**; flips go by `postMessage` only (§7.2 says the same; stated here because
  re-keying `src` on a flip would re-navigate the frame).
- **Error chip visibility**: when errors > 0 the toolbar container is opaque (not hover-only) so the count is seen
  without a hover; the design places the chip in the hover toolbar — with the toolbar hover-only an error would be
  invisible until pointed at.
- **`… N more console messages omitted`** is posted once, 250 ms after the last over-cap message (debounced), so `N`
  is right for a burst; the design says "one final" message without saying when.
- **`Page | Source` toggle sits in `Viewer.Body`**, not `ArtifactPanel` — no slot needed, and the explorer's Viewer and
  pop-outs get it for free.
- **`BASE_CSS` sets `html{background:var(--background)}`** as T3 does, so the frame is "transparent" only in the sense
  that a page without a background shows the theme's page colour (= the transcript's `--bg-page`).

## Contract notes

- `page_put(call_id, html)`: W3 passes a per-mount string (`<call_id><react-id>`), never a bare `call_id`. W2's
  `page_drop` must evict by that exact string. If W2 wants the bare `call_id` for anything (window-label scoping,
  diagnostics), say so and W3 will add a separate field — do not change the mapping silently.
- Bridge payloads are exactly §5: `size-changed { height }`, `open-link { url }` with `id: "theuth-link-<n>"` answered
  `{ jsonrpc, id, result: {} }`, `console { level, text }`, `host-context-changed { theme, styles: { variables } }`.
  W4 imports `injectBootstrap`, `readSizeChanged`, `readConsole`, `hostContextMessage`, `themeFragment` from
  `lib/page-bootstrap.ts` and `pageThemeFor` from `lib/page-theme.ts`; nothing in `HtmlPage.tsx` is needed by W4.

## Open items

- A handler 404 after eviction (W2's LRU) is indistinguishable from a page that never posts: the frame keeps the
  reserved height and renders blank; `Unable to load` fires only for a failed `page_put` or unreadable bytes (§3's
  wording for the detectable cases). A `load`-without-post timeout could mark it later if it shows up in practice.
- Fresh bindings were not copied into this tree during the wave; the 33 tsc errors and the `api.test.ts` failure are
  theirs. Re-run `bun run check && CI=1 bun run test` after the copy.
- `srcdoc` path smoke in a real browser: run the dev UI against a daemon (`bun run dev`) and show an `.html` artifact;
  jsdom is not a dependency and was not added.
