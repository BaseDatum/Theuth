# PTY-COLOR as built — terminals lost every colour in the 2.1.0 app

Branch `fix/pty-colours`, worktree `theuth-ptycolor`, base `1af0df84`.

Defect (Sky, 2026-09-27): every terminal view — desktop panel, pop-out window, iPhone and iPad over the relay — shows PTY output in black and white. Glyphs and box drawing fine. Good in v2.0.4 (`3a816bca`), bad in the app built from `90c8cc32`.

## Root cause

`9a3d8264` fix(ui): give <html> --bg-page before the stylesheet loads — `crates/desktop/ui/index.html` lines 11–14:

```html
<style>
  html { background: #e8dcc8 }
  html:where(.dark) { background: #12100e }
</style>
```

The style itself is harmless. What it does to the CSP is not:

1. At codegen Tauri stamps a nonce token on every `<style>` element of `index.html` (`tauri-utils 2.9.3 html2::inject_nonce_token`: `inject_nonce(document, "style", STYLE_NONCE_TOKEN)`).
2. When it serves the page, `tauri 2.11.5 manager/mod.rs set_csp → replace_csp_nonce` swaps the token for a random nonce and appends `'nonce-N'` to `style-src`, so the configured `style-src 'self' 'unsafe-inline'` ships as `style-src 'self' 'unsafe-inline' 'nonce-N'`.
3. Per CSP Level 2+, `'unsafe-inline'` is ignored when the same directive carries a nonce or hash. Every stylesheet the app creates at runtime without a nonce is now blocked: xterm's DOM renderer injects three `<style>` elements into `.xterm-screen` — the `.xterm-fg-N` / `.xterm-bg-N` palette, the `color:#rrggbb` truecolor styles (`setAttribute('style')`), the font family / cell metrics. Text still renders (inherited foreground), so the terminal reads as monochrome with a proportional font.

The same bundle and `index.html` ship in the desktop, the pop-out and the iOS app, hence every surface. Chromium against `bunx vite` shows colours because there is no Tauri CSP there.

## Why it escaped the gates

- `tsc`, `vitest` (jsdom) and a browser smoke against the Vite dev server never see Tauri's CSP rewrite; the DARK-FLASH report checked the *configured* CSP (`'unsafe-inline'` present in both) and not what Tauri emits once a `<style>` exists.
- No test opened a terminal in the built app; the DARK-FLASH smoke measured white frames, not the terminal.
- `securitypolicyviolation` fires nothing visible: the page has no console error for a blocked stylesheet.

## Reproduction (no human eye)

Scratch engine from this build (`THEUTH_DATA_DIR=/tmp/69/ptycolor-data THEUTH_SECRETS__KEY_STORE=file THEUTH_TOOLS__APPROVALS=false THEUTH_RPC__LISTEN=127.0.0.1:7911 target/debug/t --headless`), one `send_input` asking the model to `start` (PTY) `sh -c 'printf "\033[31mred \033[38;5;208morange \033[38;2;0;200;0mtrue\033[0m plain\n"; sleep 3600'`, then:

1. **Engine byte path** (`/tmp/69/ptycolor-rpc.ts replay`): `subscribe_pty` `reset` replay carries `\x1b[31m`, `\x1b[38;5;208m`, `\x1b[38;2;0;200;0m` byte for byte. The engine is not involved (pty.rs and `subscribe_pty` unchanged since 09-22 anyway).
2. **UI** (`/tmp/69/ptycolor-serve.ts`): `vite build` of a commit, served with SPA fallback and the `Content-Security-Policy` Tauri would emit — `tauri.conf.json`'s csp, plus `'nonce-N'` on `style-src` iff `index.html` has a `<style>` element (comments excluded), the nonce written onto those elements. Headless Chromium opens `/pty/<session>/<job>?window=popout-pty-x` and reads `getComputedStyle(span).color` for every `.xterm-rows span` and how many of the three `.xterm style` sheets have rules applied.

| build | spans | sheets applied |
|---|---|---|
| `3a816bca` v2.0.4 (good endpoint) | red `rgb(204,0,0)`, orange `rgb(255,135,0)`, true `rgb(0,200,0)`, plain `rgb(232,220,200)` | 3/3 |
| `90c8cc32` = HEAD (bad endpoint) | all four `rgb(232,220,200)` | 0/3 |
| `git bisect` `3a816bca..90c8cc32`: `3ca45d10` good, `f02f6961` good, **`9a3d8264` bad** → first bad commit | | |
| HEAD + this fix | red / orange / green / bone as v2.0.4 | 3/3; `<html>` computes `rgb(18,16,14)` from `theme.css` |

Sky confirmed the same on his machine with a build of this branch.

## Fix — `fix/pty-colours`

- `crates/desktop/ui/index.html`: the inline `<style>` becomes `<link rel="stylesheet" href="/theme.css" />`. A `<link>` is neither nonced nor hashed by Tauri, so `style-src` ships exactly as configured and `'unsafe-inline'` stays in force. Still in `<head>` after `theme.js`, still render-blocking, so DARK-FLASH's pre-paint colour is unchanged (same two rules, same `:where` specificity so `html[data-island] { background: transparent }` keeps winning).
- `crates/desktop/ui/public/theme.css` (new, beside `theme.js`): the two rules with the reason they are external.
- `crates/desktop/ui/src/lib/theme.test.ts`: `index.html has no inline <style> (a Tauri nonce there voids style-src unsafe-inline)` — strips HTML comments (not elements to Tauri's selector either) and asserts no `<style` element. Fails on `main`, passes here. It is a source lint like `no-emoji.test.ts` because this is the only layer that sees the input Tauri rewrites.
- `f02f6961` (native webview background) and 71's fonts are untouched; bisect cleared both.

## Gates

`cargo check --workspace --tests` clean (no Rust change) · `bunx tsc --noEmit` 0 · `CI=1 bunx vitest run` 95 files / 563 tests (no-emoji, surface parity, the new test) · `bunx vite build` ✓, dist `index.html` has the `<link>` and no `<style>`.

## What remains

- **Any future inline `<style>` in `index.html` re-breaks this** — the test is the guard; the alternative (`dangerousDisableAssetCspModification: ["style-src"]` in `tauri.conf.json`) would keep `'unsafe-inline'` honest even with inline styles but disables the modification Tauri does on purpose; not taken.
- Anything else that injects styles at runtime (BlockNote, Mermaid SVG `<style>`, streamdown) was also blocked in 2.1.0 and comes back with this fix; not separately measured.
- The relay's `web.rs` CSP is untouched and never nonced; iOS was affected only because it ships the Tauri bundle.
- The reproduction models Tauri's CSP rewrite from its source rather than running the .app (no Xcode here, and a Tauri build per bisect step is ~15 min); Sky's build of this branch is the in-app confirmation.

DONE PTY-COLOR
