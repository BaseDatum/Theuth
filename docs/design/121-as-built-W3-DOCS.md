# 121 as-built — W3 DOCS (design 27 revision, spike outcome, upstream issue drafts)

Branch `feat/121-w3-docs` off `d06b08c3` (main `5bc474b9` + the six spike-121
commits), worktree `/Users/Sky.Moore/Develop/theuth-121-w3`. Brief
`~/.theuth/briefs/2026-10-07/121/W3-DOCS.md`. Docs only: no code, no Cargo
files, no cargo/just/bun/tsc/vitest run. Nothing posted upstream.

## What was written, where

| File | Section | Content |
|---|---|---|
| `docs/design/27-computer.md` | §0.4 item 4 (after the "beats a deprecated API" line), §2.1 xcap bullet (after the `CGPreflightScreenCaptureAccess` line), §3.2 capture paragraph (after `features = ["png"]`) | One sentence each, identical: "Superseded 2026-10-07 — see the Revision at the end: SCK is the default, the shell-out the fallback." Original text untouched. |
| `docs/design/27-computer.md` | `## Revision 2026-10-07 — ScreenCaptureKit is the default macOS capture (wave 121)` appended at the end (`:578-719`) | **What changed** (SCK `capture_sample_buffer` → `CVPixelBuffer` → RGBA from the existing `spawn_blocking` call site; `screencapture` fallback on any SCK error / macOS < 14; `THEUTH_CAPTURE_BACKEND=screencapture\|sck`; API unchanged). **Why** (table: 49.4 vs 162.7 ms median quiet; 103.0 vs 407.6 under load 25–50 — run 3, `CGImage` route; composite 96.6 vs 211.3; mean \|ΔRGB\| 0.004–0.005; 49 MB RSS flat; the two user quotes). **What it costs** (+63 s check / +76 s build cold, `swift` + SDK ≥ 13 per Mac host, CLT suffice, Linux/Windows untouched, the two carried fixes with the dyld/ld error texts, `t` +1 066 064 B, pin `=11.0.x`). **Invariants** (never `capture_image` + `rgba_data`; rpath `build.rs` in every crate linking `computer`; keep `computer/build.rs`; cursor out / menu bar in / physical pixels / BGRA asserted; preflight unchanged). **What remains** (Retina → `121-as-built-W1-CAPTURE.md`; display/filter caching 11–24 ms; deadline parity → W1 as-built; upstream drafts; Intel; CI notes; the §7.8 in-place edits deliberately not made). |
| `docs/design/121-sck-capture-spike.md` | `## 10. Outcome (2026-10-07)` appended (`:409-441`) | Ruling quoted; worker table (W1 CAPTURE, W2 LINK, W3 DOCS) with ownership; §7 items 1–8 each marked met / open / "see W1 as-built"; last line `Integration: TBD-by-integrator` for the orchestrator. Nothing above line 407 changed. |
| `docs/design/121-upstream-issues.md` (new) | Header + three drafts | Header: drafts awaiting the user's approval, common environment block. Issue 1 apple-metal (repo `doom-fish/apple-metal-rs`): CLT link failure, `build.rs:71-79` cause, `.macOS(.v11)` / unversioned triple, our `xcrun --find swift` shim quoted, two fixes. Issue 2 screencapturekit (repo `doom-fish/screencapturekit-rs`): Part A no `LC_RPATH` in dependents (`build.rs:305`, README has no "rpath"), one-line workaround, doc + `.tbd` proposal; Part B the `rgba_data` purgeable CG cache with the 1/5/20/60 RSS table and `vmmap` evidence, doc note + provider-read proposal. Issue 3 screencapturekit: `apple-metal` optional (`Cargo.toml:371-373`, used only by `src/metal.rs` + `lib.rs:756` re-export; 26.5 s / 2 078 792 B), proposed feature TOML. Each ≤ 60 lines of body; every unchecked claim marked (unverified). |
| `docs/design/121-as-built-W3-DOCS.md` (new) | this file | |

## Sources used

- `docs/design/121-sck-capture-spike.md` §0, §1, §2, §3, §4.1–4.5, §5, §6 items 1–9, §7, §8, §9 — every number in the revision, the outcome and the drafts is from there (run 5 for the quiet grab and composite, run 3 for the load row, `rss-scaling-final` table for memory, §4.4 for build cost and binary size, §6.2/§6.3 for the verbatim ld/dyld errors).
- `docs/design/27-computer.md` §0.4 item 4 (L37-42), §2.1 xcap bullet (L86-97), §3.2 capture paragraph (L218-232) — line numbers verified by reading before editing.
- `crates/computer/build.rs` (41 lines, unchanged) — quoted in Issue 1 and Issue 2.
- Crate sources, read-only, `~/.cargo/registry/src/index.crates.io-1949cf8c6b5b557f/`: `apple-metal-0.10.0/{Cargo.toml (repository L54), build.rs (triple L29-33, link lines L67-79), swift-bridge/Package.swift}`; `screencapturekit-11.0.0/{Cargo.toml (repository L56, features L73-82, apple-metal dep L371-373), build.rs (rpath L305/L313), README.md (Requirements & Permissions, no "rpath" match), src/lib.rs (L744, L756), src/screenshot_manager.rs (CGImageExt L213-373, render_pixel_data L430)}`; `apple-cf-0.11.0/{Cargo.toml (repository L51; no apple-metal dependency), swift-bridge/Sources/CoreGraphicsBridge/CoreGraphics.swift (cgimage_render_rgba_into L102-156, CGContext into caller buffer)}`.
- Style: `docs/design/120-as-built-W1-ENGINE.md`, `docs/design/78-aec-spike.md`, existing `## Revision …` sections in `101-decision-review-surface.md`, `122-web-reading.md`.
- User quotes as given in the brief (02:08 and 08:34).

## Deviations from the brief

- Brief T1 says the display/filter caching follow-up is "worth 14–24 ms"; the spike (§4.2, §7.4) says 11–24 ms. I wrote 11–24 ms with the §4.2 cite, since the brief names the spike as the source of every number.
- Brief T1's "103 vs 408 ms under load avg 25–50": the spike's loaded run with that load figure is run 3, which predates R1, so its SCK number is the `CGImage` route (103.0 median) — stated as such in the table rather than attributed to the shipped route.
- W2's as-built filename is referenced as "W2 LINK's as-built" (design 27) — the brief names only `121-as-built-W1-CAPTURE.md`; W2's name is not known to me.

## Could not verify

- What W1 and W2 actually landed (the branches are parallel). The revision describes the design the brief specifies; status of Retina/scale-2 verification, BGRA on Retina/HDR, and the 5 s deadline parity are deferred to `121-as-built-W1-CAPTURE.md` rather than asserted.
- Whether W2 touched CI notes — written as "not owned by any wave 121 worker" per the brief's worker list.
- Everything marked **(unverified)** in `121-upstream-issues.md`: that pinning apple-metal's bridge to 13.0 removes every force-load; that `.tbd`-stub linking removes the `@rpath` reference; that the SCK `CGImage` exposes a directly readable data provider; that nothing else reaches `apple_metal` through `apple-cf` re-exports; the CG-decode-cache attribution itself (from isolation runs + `vmmap`, not Apple docs).
- The common environment block (macOS 26.6.2 build 25G83, Swift 6.3.2 swiftlang-6.3.2.1.108, SDK 26.5, rustc 1.98.1) is copied from the brief; not re-run here (the brief forbids build tooling and the spike report records 26.6.2 / CLT 26.5 / Swift 6.3.2).

## Gates

None required or run (docs only). `git diff --stat` before the final commit lists exactly the four files above.
