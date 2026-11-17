# 198 — Liquid Glass: native glass chrome over the web chat

Status: phase 1 (iPhone) first pass on `feat/198-liquid-glass`, built and tested on the release Mac 333
(`198-liquid-glass-333.md`). Phases 2 and 3 are designed here, not built.

## 0. Ask and rulings (Sky, 2026-11-17 01:45–02:04)

> "I want this set up for the iOS and macOS apps basically chat input, certain key buttons, drawer sidebar button
> floats as glass, etc. I want to keep all the same UI for chat and stuff, and I want to put some native UI flare to
> make it a lot more beautiful and native appearing." (Reference: the Grok iPhone app's glass composer over
> scrolling content.)

| question | ruling |
|---|---|
| composer | **native input, web brain** (phase 2) |
| order | **buttons first, then composer** |
| platforms | **iPhone first, then Mac** |
| Mac glass sidebar | **yes** (phase 3; we ship outside the Mac App Store) |
| process | an omp subagent does the first pass in a separate worktree; 333 builds and tests; lands separately from everything else |

Mock: `~/.theuth/briefs/2026-11-17/glass/mock.html` (iPhone: floating glass drawer button, title pill with chevron,
⋯, scroll-to-bottom and the composer; the chat scrolls under the glass).

## 1. Phases

1. **iPhone chrome (this pass).** Native iOS 26 glass top bar (drawer · title pill · ⋯) and scroll-to-bottom
   button floating over the webview; the transcript scrolls under the bar. Everything else stays web.
2. **iPhone composer: native input, web brain.** A `UITextView` in a glass container docked to
   `keyboardLayoutGuide`; the web keeps every decision (send, slash commands, mentions, attachments, drafts,
   approvals) through a bridge: text + selection + marked-text state up, commands and replacement text down.
   Must cover IME / marked text (no commits while `markedTextRange` is set), caret and selection sync, paste
   (text, images → `put_attachment`), dictation, the keyboard inset (`keyboardLayoutGuide` replaces the web's
   `--visible-band` while native composes), the transcript's bottom inset (the composer's height to the web), and
   hiding the native composer whenever a web sheet or dialog takes the screen (the phase-1 `covered` rule).
3. **macOS.** Glass sidebar through a transparent main window (`macos-private-api` is already on) with
   `NSGlassEffectView` behind the sidebar column, the floating composer and toolbar as glass; through
   objc2-app-kit 0.3.2 `NSGlassEffectView` behind feature flags (`#available(macOS 26)` fallback
   `NSVisualEffectView`). The web column paints transparent where the glass shows.

## 2. Phase 1 contract

### 2.1 Plugin

`crates/tauri-plugin-glass`: the first first-party Tauri mobile plugin, in the upstream layout
(v2.tauri.app/develop/plugins/develop-mobile; `tauri-plugin-haptics` as the template):

| file | role |
|---|---|
| `build.rs` | `tauri_plugin::Builder::new(COMMANDS).ios_path("ios")`: permissions per command, Swift package linked on iOS |
| `src/lib.rs` | `init()`: `Builder::new("glass")`; on iOS `register_ios_plugin(init_plugin_glass)` (`tauri::ios_plugin_binding!`). No Rust commands: Tauri forwards unhandled `plugin:glass|x_y` to Swift as `xY` |
| `permissions/default.toml` | every command + `register_listener`/`remove_listener` (`addPluginListener`) |
| `ios/Package.swift` | static library `tauri-plugin-glass` on `../.tauri/tauri-api`, iOS 17 floor |
| `ios/Sources/GlassPlugin.swift` | `Plugin` subclass: the commands, main-queue hops, the overlay as a child of `manager.viewController` |
| `ios/Sources/GlassOverlay.swift` | the pass-through overlay view controller and its controls |

Registered by `crates/desktop` on iOS only (`[target.'cfg(target_os = "ios")'.dependencies]`, `mobile::configure`);
ACL `capabilities/glass.json` (platform iOS, `glass:default`).

### 2.2 Commands and events

| command | args | effect |
|---|---|---|
| `is_supported` | — | `{ supported }`: true only built with the iOS 26 SDK (`#if compiler(>=6.2)`), running iOS 26+ (`#available`), on an iPhone |
| `configure` | `{ title, dark? }` | mounts the overlay (idempotent), sets the title, the appearance (`dark` = the page's theme; absent = system), shows the bar |
| `set_title` | `{ title }` | the pill's title and accessibility label |
| `set_scroll_button` | `{ visible, bottom }` | shows/hides the round button; `bottom` pt from the visible band's bottom (keyboard top, else screen bottom) to its bottom edge |
| `set_hidden` | `{ hidden }` | hides/shows bar and scroll button together |

Event `tap { id }`, `id` ∈ `drawer` · `title` · `menu` · `scroll_bottom`; every tap plays a light impact haptic.
Every command is rejected unless `is_supported` would be true. Commands arrive on Tauri's serial ipc queue; all UIKit
work hops to the main queue in arrival order; the web serialises its sends through one promise chain.

### 2.3 Layout

- Bar: under the safe-area top by 4 pt, 12 pt side insets, 44 pt controls: round glass `sidebar.left` ("Open
  sidebar"), the glass title pill (title + `chevron.down`, centred, truncating between the buttons), round glass
  `ellipsis` ("Session menu"). Its band is 4 + 44 + 4 = **52 pt**, the web header's height (`BAR` in
  `ui/src/lib/native-chrome.ts`).
- Scroll-to-bottom: round glass `arrow.down` ("Scroll to bottom"), centred, `bottom` above
  `keyboardLayoutGuide.top` (`usesBottomSafeArea = false`).
- Glass: `UIButton.Configuration.glass()`; the overlay is added after the webview in the host view, so it samples
  the chat. Light and dark follow the page's theme (`dark`), else the system.
- Dynamic Type: fixed 44 pt controls (the system bars' rule), the title font scales to 20 pt, the large content
  viewer shows every control at accessibility sizes.
- Touches on empty overlay space fall through to the webview (`hitTest` returns nil for the pass-through views).

### 2.4 Web integration (only `container() === 'ios'` and `is_supported`)

- `lib/native-chrome.ts`: probe, store (`useNativeChrome`), `useNativeBar(own)` (supported ∧ phone layout ∧ the
  header owns the window), `mountBar`, `setBarTitle`, `setScrollButton`, `onNativeTap`, `covered`.
- `Header` (`first`, not a popout, not the talk view): native mode renders the `<header>` `invisible` — still
  mounted as the anchor of the chevron panel and the ⋯ dropdown and the host of their dialogs. `nativeLayout` keeps
  only those two menus in the bar and lists every other item in the chevron panel. `drawer` → `toggleSidebar()`,
  `title` → the chevron panel (`HeaderMenu`, now optionally controlled), `menu` → `SessionMenu` (optionally
  controlled). `under` (the live chat): the header is absolute and takes no room; otherwise (the new-chat pane) it
  keeps its 52 px.
- `LiveSession`: `under`; the banner strips (link, private, orphans, resume, search return) float just under the bar
  over the transcript top instead of pushing it down.
- `Transcript under`: the scroller reaches up through the safe-area top (`margin-top: -safe-top`) and pads its
  content by `safe-top + 52px`; the virtualizer's coordinates stay list-relative (its `paddingStart` slack is
  untouched), so the row at `scrollTop` is the row at the bar's bottom edge and the §3.3 anchor's element search
  uses that edge. The web scroll-to-bottom is not rendered; the native one mirrors `unstuck` and sits 12 px above
  the scroller's bottom (`scrollButtonBottom`, re-measured on scroller resize and `visualViewport` resize).
- Hidden while covered: any Radix dialog outside a popper (sheets, the phone sidebar drawer, centred and
  full-screen dialogs) hides bar and button; the chevron popover and the ⋯ dropdown do not. Watched with a
  `MutationObserver` on `<body>`'s children only (portals), never the subtree.
- Fallback: iOS < 26, no plugin, iPad (any width), desktop, browser: no plugin call beyond the probe on iOS, the web
  header and button render exactly as before.
