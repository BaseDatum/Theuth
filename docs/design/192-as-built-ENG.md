# 192 COMPANION-PICTURES — as built, engine

Branch `feat/192-engine`, cut from main `52475368`. Contract: [192-contract.md](192-contract.md).

## Method and type

| Item | Where |
|---|---|
| `FenceSnapshotStatus` (`done`/`no_renderer`/`failed`/`timed_out`/`cancelled`, snake_case, `ts(export)`) | crates/artifact/src/snapshot.rs:178 |
| `FenceSnapshotReply { outcome, png, width, height, error }` (same derives and `cfg_attr(feature = "ts", …, ts(export))` as the 164 types) | crates/artifact/src/snapshot.rs:195 |
| `From<SnapshotOutcome> for FenceSnapshotReply`: `Done` → base64 (standard) png + header px; `Failed(why)` → `error`; the rest bare. `Done.client` is dropped (not in the contract). | crates/artifact/src/snapshot.rs:208 |
| RPC trait `#[method(name = "fence_snapshot", with_extensions)]` | crates/rpc/src/api.rs:739 |
| Impl: `owner(ext)?` (no peer → `UNAUTHORIZED` "no authenticated connection"; guest → `UNAUTHORIZED` "owner only"), clamp `width` 240..=1200 and `scale` 1..=3, `artifact::snapshot::render(&self.rt, …).await.into()` | crates/rpc/src/api.rs:5830 |
| Guests are also outside `link::conn::GUEST_METHODS` (crates/link/src/conn.rs:538), so the link layer refuses them first; the method guard is the second gate. | — |

## Tests (`rpc` `api::tests::fence_snapshot`)

| Test | Line | Checks |
|---|---|---|
| `the_rpc_returns_the_drawn_png` | crates/rpc/src/api.rs:12449 | a fake client answers `started` then `done` with a real 1×1 PNG (`DOT`, :12404) → exact `{"outcome":"done","png":DOT,"width":1,"height":1,"error":null}` |
| `no_subscriber_is_no_renderer` | :12476 | bare runtime, nobody on the bus → exact `{"outcome":"no_renderer","png":null,"width":null,"height":null,"error":null}` |
| `a_failed_client_is_failed_with_its_reason` | :12491 | `failed("bad json")` → `{"outcome":"failed",…,"error":"bad json"}` |
| `width_and_scale_are_clamped` | :12508 | the published `FenceSnapshotRequested` carries (10, 9) → (240, 3) and (5000, 0) → (1200, 1) (`Requests` subscriber, :12408) |
| `guests_are_refused` | :12530 | an `open_guest` peer and a peerless call both get `UNAUTHORIZED`; no request is published |

Runs: `mbx test -p artifact -p rpc fence_snapshot` → 7 passed (5 new + 2 from 164). `mbx test -p artifact -p rpc --no-fail-fast` → artifact lib 95, document 2, read 7, rpc lib 96, alerts 1, new_chat_elsewhere 1, private_artifacts 1, private_get_attachment 1, prompts 1; 0 failed. `mbx clippy -p artifact -p rpc --no-deps` clean. Without `--no-deps` clippy stops on a `crates/voice/src/history/mod.rs:465` `chunks_exact` lint: the workspace wrapper lints workspace deps, and that file is unchanged from main.

## Bindings

`bindings/` (`.gitignore:4`) and `crates/desktop/ui/src/rpc/bindings/` (`crates/desktop/.gitignore:5`) are generated and not committed, so no generated files are committed. What the convention needs:
- `FenceSnapshotReply` added to the `export!` list, crates/rpc/src/export.rs:675. `export_all` brings `FenceSnapshotStatus` along.
- `crates/desktop/ui/src/rpc/api.ts` mirrors every method. `api.test.ts:13` checks `METHODS` against `bindings/methods.json`. Added: imports :166-168, signature :1160, `METHODS` entry :1638.
- The export was run once (`mbx test -p rpc --features ts export_bindings`) to check the generated `FenceSnapshotReply.ts` and that `methods.json` lists `fence_snapshot`.

## Investigation: does the macOS renderer answer with the main window out of sight?

Verdict: **not reliably, in any of the four states.** The webview is never destroyed. Under WebKit's default inactive-scheduling policy, though, its WebContent process can be suspended. Then nothing answers, not even `started`, and the engine gives `no_renderer` after 3 s and backs off for 60 s (snapshot.rs:28, :33).

Shared facts:
- Only the main window renders: `installFenceSnapshot` is installed only outside the aux windows (crates/desktop/ui/src/App.tsx:72, :689).
- Answers go over the webview's own JS WebSocket (ui/src/rpc/transport.ts:50; fence-snapshot.ts:137). There is no Rust-side answerer.
- `started` is sent synchronously from the event handler (fence-snapshot.ts:164-165). The renderer uses no rAF and no ResizeObserver, only timers, a MutationObserver, `fonts.ready` capped at 1 s, `img.onload` and `toBlob` (FenceSnapshot.tsx:5, :25, :240, :266; lib/diagram-export.ts:33-47). A hidden page that is still running therefore renders. The one known gap is the OpenUI recharts `ResponsiveContainer`, which relies on ResizeObserver (164-as-built-UI.md:189-190).
- Neither `tauri.conf.json` (windows :16-26) nor `tauri.macos.conf.json` (windows :6-18) sets `backgroundThrottling`. wry sets `inactiveSchedulingPolicy` only when that option is `Some` (wry-0.57.1 src/wkwebview/mod.rs:515-538). So WebKit's default `.suspend` applies. [INFERENCE, cited from WebKit source by the investigation, not run here] WebKit treats a view as not visible when its window is ordered out, miniaturised, hidden or occluded, and may then suspend the process after a delay that varies by OS (around minutes).

| State | What the shell does | Webview | Answers? |
|---|---|---|---|
| Hidden (tray toggle, crates/desktop/src/lib.rs:553; the initial `visible:false`, including a whole `--autostart` session, dock.rs:96-108) | `hide()` → orderOut | alive, not visible | until WebKit suspends it, then no |
| Minimised (lib.rs:473, `performMiniaturize:`) | no handler (on_window_event handles only CloseRequested/ThemeChanged, lib.rs:319-335) | alive, not visible [INFERENCE] | same |
| Closed (red button / ⌘W, lib.rs:476) | `CloseRequested` → `prevent_close()` + `hide()` for main and island (lib.rs:321-326); design 166 keeps the app in the Dock | alive, not reloaded | same |
| ⌘H (lib.rs:434) | NSApp hide; no handler | alive, not visible [INFERENCE] | same; App Nap may add to it [INFERENCE] |

The fix, applied in a follow-up commit, is in the next section.

## Hidden-window renderer

**Verdict:** before this change the main webview could be suspended whenever it was hidden, minimised, closed or ⌘H'd. A suspended webview answers nothing, so fence requests got `no_renderer` plus a 60 s back-off. Details are in the investigation above.

**Change:** `"backgroundThrottling": "disabled"` on the `main` window entry, right after `"dragDropEnabled": false`:
- crates/desktop/tauri.macos.conf.json:18. This is the line that takes effect on macOS, because this file redeclares the whole `windows` array and Tauri's merge-patch replaces arrays wholesale (tauri-utils-2.10.2 src/config/parse.rs:185).
- crates/desktop/tauri.conf.json:26. Kept in step with the macOS file; does nothing on Windows and Linux.

wry maps the setting to WKWebView `inactiveSchedulingPolicy = .none` on macOS 14+ (wry-0.57.1 src/wkwebview/mod.rs:515-538). The bundle's `minimumSystemVersion` is 15.0.

**Schema check:** the repo ships no config schema. Both files point `$schema` at `https://schema.tauri.app/config/2`, which could not be fetched for validation here (no PyPI access for a validator). So the check was made against the locked tauri-utils 2.10.2 source. `WindowConfig` is `rename_all = "camelCase", deny_unknown_fields` (config.rs:1951) and has `background_throttling: Option<BackgroundThrottlingPolicy>` (:2254). The enum is camelCase with `disabled | suspend | throttle` (:1855-1866). A script checked that every key of both `main` entries is a `WindowConfig` field and that the value is a valid variant. No desktop build was run: mbx can't run the desktop crate's tauri build script.

**Battery trade-off:** the hidden main webview keeps running instead of being suspended. Its JS, the WebSocket and the 164 renderer stay live while the window is out of sight, which costs idle CPU and energy that WebKit would otherwise reclaim. DOM timers in a hidden page are still aligned and throttled; the renderer's short timers tolerate that. `"throttle"` would save more battery but does not guarantee the `started` answer within 3 s.

**Live checks still owed** (on a desktop build with this config). For each state (main window hidden, minimised, closed, ⌘H'd), leave it there for over 5 minutes, then:
- send a mermaid fence through the native companion: it should show a picture, and `fence_snapshot` should return `done`;
- send a mermaid fence in a Telegram reply: it should arrive as a PNG, not the D7 text, with no `NoRenderer` in the engine log.
