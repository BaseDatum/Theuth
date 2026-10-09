# 149 as-built — NATIVE LOOK, Windows companion (fix/149w-native)

**Ruling (Sky, 2026-10-09 08:33).** 1. "no one asked you to theme the companion app. it's supposed to be fully native." 2. "did you really put an emoji for the microphone…. seriously?"
**Scope.** Only the look of `crates/desktop/companion-windows` changed: colour, material, type, shape, icons, light/dark. Behaviour, RPCs, keys, window anatomy and UIA ids are untouched. The macOS twin is `docs/design/149-as-built-native-look.md` (main 3ae1c54d); this document is the Windows column of its shared table.
**Branch.** `fix/149w-native` off `origin/main d2c59c24`, folded `origin/main 3ae1c54d` at 0532b052 (the Mac as-built + the redrawn shared harness assets). Final sha: see the report (`~/.theuth/briefs/2026-10-09/149-windows/reports/NATIVE.md`).

## 1. Where colour comes from now

- `src/Theuth.Companion.Core/NativeLook.cs` (new, tested) — the portable table: `StatusTone` (NeedsYou · Running · Done · Failed · Paused · Remote · Idle), `Tone(MarkStateKind|AgentRowState)`, `Brush(tone)` → a WinUI theme-resource KEY, `IsHollow`, `IsSpinner`, every resource key the app uses as a constant, the Segoe Fluent Icons glyph table, `KeyCapText()` (symbols → Windows key words) and `HasIconCharacter()` (the detector the test suite and the final grep use).
- `src/Theuth.Companion/Windows/Theme.cs` (new) — `Theme.Brush(key, theme)` resolves a key LIVE from `XamlControlsResources`' `ThemeDictionaries["Default"|"Light"]` for the element's theme (Default → the system's mode read from `UISettings` background luminance). `Theme.Status(tone, theme)`, `Theme.Accent()` (`SystemAccentColor` → raster colour), `Theme.Icon(glyph, theme, key, size)` (a `FontIcon` in **Segoe Fluent Icons**, **Segoe MDL2 Assets** below build 22000), `Theme.Follow(root, rerender)` (sets the root's `RequestedTheme` to the system mode now and on every `SystemEffects.ThemeChanged`, re-renders on `ActualThemeChanged`). Every resolved key is logged once with its colour — on build-B: `SolidBackgroundFillColorBaseBrush [Dark] = #FF202020`, `TextFillColorPrimaryBrush [Dark] = #FFFFFFFF`, `AccentFillColorDefaultBrush [Dark] = #FF2C06AB` (= `UISettings AccentLight2`; build-B's accent is `#1F047A`), `SystemFillColorCriticalBrush [Dark] = #FFFF99A4`, `ControlFillColorDefaultBrush [Light] = #B3FFFFFF` … no key fell back.
- `SystemEffects.ThemeChanged` (new) — `UISettings.ColorValuesChanged` (background thread, marshalled) + `WM_SETTINGCHANGE`; every window re-renders through `Theme.Follow`. Proven live: `w6-gate.ps1 theme light|dark` flips Windows' mode and the open panels, pills, legend, island, Ask bar and Settings re-paint within a second (evidence pairs below were taken on the same running instance).
- `Palette.cs` (the theuth web palette: bone/soot, gold, faience, malachite, carnelian, sand, ink, rule) is **deleted**. Final grep of `src`: zero hex colours (the two literals left are the capture-probe's test magenta `#ff00ff` the engine paints, and Windows' own default accent `0,120,212` as the last-resort fallback in `Theme.Accent()` when every API fails), zero `FromArgb` for UI colour, zero `Palette`.

## 2. Status semantics — the shared table, Windows column

| State | macOS | Windows (`NativeLook`) | Drawn as |
|---|---|---|---|
| needs you / waiting on the user (asks, approvals, selection, the recording mic, the inbox count, the point-and-say trail while held) | `Color.accentColor` | `AccentFillColorDefaultBrush` (text on it: `TextOnAccentFillColorPrimaryBrush`) | filled disc; notched accent ring on the strip rail; accent count; accent level bars; accent raster trail |
| running / in flight | no colour + `ProgressView(.small)` | no colour: `TextFillColorSecondaryBrush` + a native **`ProgressRing`** (12 epx, IsIndeterminate, Foreground = secondary); a static secondary ring under "Animation effects" off | `PillGlyphs.Spinner`, `MarkGlyph` Working |
| done / finished | `.systemGreen` | `SystemFillColorSuccessBrush` | disc + drawn check (on-accent stroke) |
| failed / error / stop | `.systemRed` | `SystemFillColorCriticalBrush` | disc + drawn dash; failed receipts; "Stop agent" text |
| paused / blocked / lapsed (= `AgentStatus.Paused`) | `.systemOrange` | `SystemFillColorCautionBrush` | disc + two drawn pause bars. *Deviation from the Mac mark:* the Mac draws a lapsed MARK hollow tertiary; on Windows `Lapsed` is literally the paused desk, so it takes the paused colour. |
| remote / unknown (E7 fallback) | hollow `.tertiaryLabelColor` | hollow ring stroked `TextFillColorTertiaryBrush` | `Ring` |
| idle | hollow `.secondary` | hollow ring stroked `TextFillColorSecondaryBrush` | `Ring` |
| text | `.primary/.secondary/.tertiary` | `TextFillColorPrimary/Secondary/Tertiary/DisabledBrush` | |
| rules / strokes | `.separator` | `DividerStrokeColorDefaultBrush`; card outlines `CardStrokeColorDefaultBrush`; control strokes `ControlStrokeColorDefaultBrush` | |
| fills | `.primary.opacity(x)` | `ControlFillColorDefault/Secondary/Tertiary`, `SubtleFillColorTransparent/Secondary/Tertiary`, `CardBackgroundFillColorDefault/Secondary` — the system's own translucent fills, never an opacity on a text colour | |
| backgrounds | materials | materials (§4), never a painted fill over them | |

## 3. Mapping — old token → WinUI resource

| Old (`Palette.*` / literal) | New | Where |
|---|---|---|
| `Ink` (`e8dcc8`/`1a1612`) | `TextFillColorPrimaryBrush` | all primary text, the wordmark's old fill |
| `Ink2` | `TextFillColorSecondaryBrush` | meta text, icons at rest, key caps |
| `Sand` | `TextFillColorTertiaryBrush` | hints, "answers wait 3 s", quiet sliver, remote rings |
| `Gold` (`c47a2c`/`b4531e`) | `AccentFillColorDefaultBrush` | needs-you discs/rings/count, headline, position dot, recommended badge/outline, left bar, "N new", oracle verdict, wordmark (accent fill + on-accent inner square, like the Mac) |
| `Faience` | `AccentFillColorDefaultBrush` (the user bubble, mic meter) · `TextFillColorSecondaryBrush` (idle ring, spinner) · `SubtleFillColorSecondary/Tertiary` (focused/hovered row + an accent selection pill at the left, the Windows list idiom) | CardChrome, PillGlyphs, AgentsPanel |
| `Malachite` | `SystemFillColorSuccessBrush` | done marks, Sent receipts, "Done" zone, attached-screenshot outline |
| `Carnelian` | `SystemFillColorCriticalBrush` for failed/error/stop; `AccentFillColorDefaultBrush` for the recording mic + level bars (recording = needs you) | receipts, dots, Ask bar, reply row |
| `Surface` (`1c1915`/`f3eadf`) and the 82 % tint over Mica | `SolidBackgroundFillColorBaseBrush` only where no material can exist; `Transparent` over a material | every window |
| `Rule` | `DividerStrokeColorDefaultBrush` | hairlines, receipt borders, hints' cap outlines |
| `Palette.Tint(Ink, 0.03…0.30)` | `CardBackgroundFillColorDefault` · `ControlFillColorDefault/Secondary/Tertiary` | frames, badges, hover/pressed |
| Island `0x141210` / `0x3a3228` | `SolidBackgroundFillColorBaseBrush` / `CardStrokeColorDefaultBrush` | island shell |
| Ask bar `Solid(0x…)` ×7, `Translucent(malachite, 0.12)` | the brushes above; receipt fill none | AskBarWindow |
| Settings' local `Theme` shim (`FromArgb(gray)` fallback) | the shared `Ui.Theme` | Settings, ShortcutRecorder |
| `PointFlow.TrailRgb = 0x4a8a6e` (const malachite) | `PointFlow.TrailRgb` = the system accent, set from `Theme.Accent()` at start and on every accent/theme change (`OverlayWindows.SetColor`); `TrailHex` goes to `screen_context` so the engine stamps the same colour; Windows' default `0078D4` until the app reads it | PointAndSay |
| tray sprites: gold `●N` badge | monochrome like Windows 11's tray — white glyph + white disc with black digits on a dark taskbar, the inverse on light (`make-tray-sprites.swift`, 22 `.ico` regenerated) | Tray |
| accent on tray / shell | — (a pre-rendered icon cannot follow the accent; recorded) | |

## 4. Backdrop per window

| Window | Material | How | Why |
|---|---|---|---|
| Attention panel | **Mica Base** (Acrylic Base where Mica is unsupported) | `Window.SystemBackdrop = new MicaBackdrop()` (`FrameworkBackdrop.Apply`), `Background = Transparent` | The manual `MicaController` + `SystemBackdropConfiguration.Theme = Dark` painted Mica LIGHT under dark text on build-B (reproduced at 09:14, as 134 W6 had seen); the framework controller reads the content's `ActualTheme` and was dark at once. 134 W6's 82 % painted surface is gone. `--panel-opaque` keeps a solid `SolidBackgroundFillColorBase`. |
| Agents panel | Mica Base | same | same; it had no backdrop before |
| Legend | Mica Base + DWM round corners | same | a transient like the panels |
| Settings | Mica (Acrylic where unsupported) | `Window.SystemBackdrop` on the titled window | titled window; title bar follows the accent as Windows draws it |
| Ask bar | Desktop Acrylic | `Window.SystemBackdrop = new DesktopAcrylicBackdrop()` (as before), `Background = Transparent` | unchanged controller, the painted fallback colour is now `SolidBackgroundFillColorBase` |
| Strip rail (§3.8 escape hatch) | Desktop Acrylic **Thin** via the manual controller (`Backdrop.Attach`, `IsInputActive` held true — A7: a never-activated window must not fall back) with no tint and no painted fill | `Backdrop.cs` | the custom `TintColor/TintOpacity/LuminosityOpacity` toward the palette surface is removed; fallback = `SolidBackgroundFillColorBase` |
| Rail pills | `SolidBackgroundFillColorBaseBrush` + 1 epx `CardStrokeColorDefaultBrush` hairline | painted | one `SetWindowRgn`-shaped window (capsules with transparent gaps) cannot host a system backdrop (platform finding) |
| Island | `SolidBackgroundFillColorBaseBrush` + `CardStrokeColorDefaultBrush` hairline | painted | un-regioned NOACTIVATE tool window whose shape is the Border's own rounded rectangle over a transparent root — a system backdrop would fill the whole HWND |
| Point-and-say overlays | layered raster (no XAML) | — | the trail colour is the accent (§3) |
| Every panel/legend HWND | `DWMWA_USE_IMMERSIVE_DARK_MODE` follows the theme | `FrameworkBackdrop.ImmersiveDark` | |

## 5. Type and shape

- Ramp only: caption 12 · body 14 · body-strong 14 semibold · subtitle 20 semibold (panel/agents/settings page titles, the empty-state headline) — no 10.5/11.5/12.5/13/15/17/19/24, no `Bold`/`Medium` weights, no `Consolas` (key caps and receipt times are caption text now; the log window uses **Cascadia Mono**, Windows' default monospace). `CardChrome.Question/Body/Meta/Caps` = 14/14/12/12.
- Radii: `ControlCornerRadius` (4) on key caps, buttons, badges, chips, the reply box, the island's buttons/badges; `OverlayCornerRadius` (8) on cards, bubbles, option rows, the island shell, the agents row frame; capsules only where the capsule is the layout (rail pills, Ask pill, island strip, footer pills, the long position dot). Read from the resources where the app has them (`CardChrome.ControlRadius/OverlayRadius`).
- Header rows (Sky's Mac review): the card title sits in the star column with `TextWrapping=NoWrap` + `CharacterEllipsis`, its `MaxWidth` following the column on every size change so the position dots stay right after the (truncated) name; key caps have `NoWrap` and live in Auto columns. Proof: `panel-long-title-{light,dark}.png` (a 31-character agent name).

## 6. Icons — old character → glyph

| Old | New (Segoe Fluent Icons) | Where |
|---|---|---|
| 🎙 / `\uE720` literal | `Glyph.Microphone` E720 via `Theme.Icon` | reply row, agents rows, actions pill, Ask bar |
| 📷 / ▣ / `\uE722` literal | `Glyph.Camera` E722 | reply row P, actions pill "point" |
| ✕ · × (close, stop row) | `Glyph.Cancel` E711 (+ the "Esc" cap on the close button) | panel/agents headers, agents rows, island dismiss |
| ··· / ⋯ | `Glyph.More` E712 | more pill |
| ↑ / ⏎ (send) | `Glyph.Up` E74A + cap "Enter" | reply row |
| → (option arrow) | `Glyph.ChevronRight` E76C | option rows |
| ‹ older / newer › | `Glyph.ChevronLeft` E76B / `ChevronRight` E76C + the word | island briefs |
| ⋮⋮ (drag grip) | `Glyph.GripperBarVertical` E784 | island |
| ◉ / ○ (eye) | `Glyph.Hide` ED1A / `Glyph.View` E890 | island |
| ⑂N (children) | `Glyph.Flow` E7C5 + caption N | agents rows |
| 🌐 (mesh group, in the Core title) | title without the character (Core `AgentsModel`, test updated) + `Glyph.Globe` E774 drawn by the panel | agents groups |
| ↑N new | `Glyph.Up` + "N new" in accent | island |
| "• " bullets | a drawn 4 epx `Ellipse` column | island briefs |
| ● ● ▬ position dots, status dots, level bars | drawn `Border`/`Ellipse`/`Rectangle`/`Path` with theme brushes; `ProgressRing` for running | everywhere |
| ⏎ ⎋ ⌃ ⇧ ⌥ ← → ↑ ↓ in caps/hints | "Enter", "Esc", "Ctrl", "Shift", "Alt", "Left", "Right", "Up", "Down" (`NativeLook.KeyCapText`; `KeyNames` arrow words; `CardAnatomy` "Enter") | key caps, legend, hints, placeholders ("Enter sends, Esc leaves") |
| harness icons (shared monochrome PNGs) | kept — rendered as **template images**: tinted `TextFillColorPrimary` at load (`PillGlyphs.TintAsync`) so they read on light too; the redrawn `omp` from main (lowercase wordmark) is what the csproj links (`Assets\harness\*.png`) | agents rows, card status |
| the theuth wordmark | kept (brand): accent square + on-accent inner square | pills, strip, empty state |

Prose typography stays prose: "·", "—", "…", "›" in Settings paths, "1920×1080" in the display picker. Log lines and code comments were not in scope.

## 7. Files changed (companion-windows)
Core: `NativeLook.cs` (new) · `AgentsModel.cs` (mesh title) · `CardAnatomy.cs` (Enter) · `KeyBindings.cs` (arrow words) · `PointSettings.cs` (TrailRgb live, `PackRgb`) · tests `NativeLookTests.cs` (new, 8), `Ww2Tests.cs`, `W6Tests.cs`, `PointSettingsTests.cs`.
App: `Windows/Theme.cs` (new) · `Windows/Palette.cs` (deleted) · `Windows/Backdrop.cs` (+`FrameworkBackdrop`) · `Windows/SystemEffects.cs` (ThemeChanged) · `Windows/CardChrome.cs` · `Windows/PillGlyphs.cs` · `Windows/MarkGlyph.cs` · `Windows/FlatButtons.cs` · `Windows/AttentionPanelWindow.cs` · `Windows/AgentsPanelWindow.cs` · `Windows/RailPillsWindow.cs` · `Windows/RailWindow.cs` · `Windows/LegendWindow.cs` · `Windows/Surfaces.cs` · `Island/IslandWindow.cs` · `AskBar/AskBarWindow.cs` · `AskBar/ReplyField.cs` · `PointAndSay/PointController.cs` · `PointAndSay/OverlayWindow.cs` · `PointAndSay/TrailRaster.cs` · `Settings/SettingsWindow.cs` · `Settings/ShortcutRecorder.cs` · `LogWindow.cs` · `Store.WW2.cs` (comment) · `Assets/tray/make-tray-sprites.swift` + 22 `.ico`.
Tools: `w6-gate.ps1` (`theme show|light|dark`, `hold-panel`) · `tools/Theuth.Companion.Seed/Program.cs` (`hire --name`).

## 8. Gates
- Mac: Core **282/282** (274 + 8 new), Rpc see report; 0 warnings (TreatWarningsAsErrors).
- build-B Debug x64 of the whole solution: **0 errors / 0 warnings** at every pushed sha (last 20261009T083859-318).
- UI rounds on build-B (desk lock `native`, installed companion quit/restarted, Run value intact, theme restored to the original dark): evidence in `~/.theuth/briefs/2026-10-09/149-windows/evidence/native/` (31 PNG) — `pills`, `pills-agents-fixture`, `panel-decision`, `panel-hire`, `panel-empty`, `panel-long-title`, `agents`, `agents-omp`, `legend`, `strip-rail`, `settings-rail`, `settings-keys`, `island`, `ask-bar`, `point-trail` × `{light,dark}`; `panel-mica-light-under-dark-BEFORE-fix.png` documents the controller bug.

## 9. Deviations and open items
1. **Ask pill "recording" screenshot not taken.** The recording state needs Ctrl+Shift+Space HELD; a `RegisterHotKey` chord sent by `SendInput` from the build service does not reach the companion on build-B (recheck item 7 — `hold-panel` was added to `w6-gate.ps1` and fired cleanly, no `WM_HOTKEY`), while the voice key Ctrl+Alt+Space does. The recording look is the same code path as the proven pieces: `PillGlyphs.LevelBars` = accent (seen on the island meter), the mic glyph turns accent, status text accent.
2. **Mica via the manual controller** painted light under a dark theme (reproduced); the panels now use the framework backdrop (§4). The strip rail keeps the manual Thin Acrylic controller (needed for the never-activated window) — it rendered correctly in both themes.
3. **Accent contrast on build-B** is poor by the system's own choice: its accent is `#1F047A`, so `AccentFillColorDefault` in dark (`AccentLight2`) is `#2C06AB`. The companion shows exactly what Windows resolves; nothing was tuned.
4. **Lapsed = Caution** (paused desk) rather than the Mac's hollow tertiary lapsed mark (§2).
5. **Idle vs remote** are both hollow rings (secondary vs tertiary stroke) — as the 130 shape table already had them.
6. `AgentsPanel` focused row = subtle fill + accent selection pill (Windows list idiom) instead of the Mac's accent-tinted row.
7. Trailing `//` comments and probe log lines still contain arrows; the display picker says "1920×1080"; none render as icons.


## R1 (09:49 → 09:57) — accent TEXT uses the text resources; main folded again

- **Accent-coloured text → `AccentTextFillColorPrimaryBrush`** (`NativeLook.AccentText`, plus `AccentTextSecondary/Tertiary` for lesser text, test `AccentTextIsATextResourceNotAFill`). Applied to: the panel sub-line ("N need you · M done"), "(Recommended)" and "recommended · Enter", the inbox count digit (pills and strip), the island's "N new", verdict, "listening…"/"approval waiting", hint text and the text inside accent-outlined badges, the Ask bar's recording mic glyph + status, the reply row's recording mic glyph. **Fills keep `AccentFillColorDefaultBrush`**: dots, notched rings, the long position dot, the left needs-you bar, the recommended badge/outline, the Clear pill, the undo countdown bar, the agents selection pill, level bars, the island meter/rec dot, the trail.
- Mapping rows: *text ON accent* = `TextOnAccentFillColorPrimaryBrush` (digits in the recommended badge, the check/dash/pause marks, the wordmark's inner square, the Clear pill's label) · *accent TEXT* = `AccentTextFillColorPrimaryBrush` (the list above) · *accent FILL* = `AccentFillColorDefaultBrush`.
- Resolved on build-B: `AccentTextFillColorPrimaryBrush [Dark] = #FF3408CD` (= `SystemAccentColorLight3`), `[Light] = #FF120249` (= `SystemAccentColorDark2`); `AccentFillColorDefaultBrush [Dark] = #FF2C06AB` (Light2). With build-B's deep-indigo accent `#1F047A`, WinUI's own Light3 is still dim — a WinUI `HyperlinkButton` on this machine reads the same; the companion now uses exactly the resource the system's controls use, nothing tuned.
- Fold: `origin/main` (one commit, iOS Info.plist) merged.
- Gates: Core 283/283, Rpc 68/68; build-B Debug 0/0 (20261009T085219-331 at 0315a59f).
- Evidence (re-shot on the same running instance, light and dark): `panel-decision-dark`, `panel-long-title-{light,dark}`, `pills-agents-fixture-{light,dark}`, plus `panel-ask-fixture-{light,dark}` (the `--ww2-fixture` ask card: bubbles, "(Recommended)" accent text, Stop agent in Critical). The pre-R1 versions are kept as `*-r0.png`.
