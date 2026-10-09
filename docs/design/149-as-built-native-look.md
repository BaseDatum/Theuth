# 149n as built — the companion looks fully native

Branch `fix/149-native` (worktree `~/Develop/theuth-149n`) off `origin/main` 584660bf. Swift only:
`crates/desktop/companion/**` and this file. Commits `faff0621` (the migration) and `6e3b471d` (fixes after the PNG review).

## The ruling
> "no one asked you to theme the companion app. it's supposed to be fully native. second, did you really put an emoji for the
> microphone…. seriously?"

130 ported theuth's web palette (`crates/desktop/ui/src/styles.css`) into `Palette.swift`; 149 W1 grew it into a type scale,
24–28 pt radii and a forced `.darkAqua` on every panel, and used emoji and text characters as icons. All of that is gone.
Behaviour, layout, key bindings, RPCs and store logic are unchanged; this is a presentation change.

## Inventory (what was replaced, by file)
Palette/`Type`/colour-token/fixed-size/appearance sites (`grep -nE 'Palette\.|\.gold\b|…|Type\.|darkAqua|size: [0-9]'`, before):

| File | Sites | Glyph icons removed |
|---|---|---|
| `Palette.swift` | the source: `enum Palette` (9 hex pairs), `NSColor(hex:)`, `extension Color` (9 tokens), `Type` (6 fixed fonts, radii 26/28/24, `dark`, `malachiteHex`), carnelian `LevelBars` | — |
| `PanelView.swift` | 74 | `🎙` (reply row V button), `✕` (close), `‹ ›` (J/K), `▣` (Point), `→` (option rows), `↑` (Send) |
| `Island/IslandViews.swift` | 37 | `⌸` (idle pill wordmark), `•` (brief bullets), `↑ N new`, `→ model` (verdict text) |
| `AgentsPanel.swift` | 30 | `✕` (close), `⑂N` (children count), hand-drawn `ArcSpinner` |
| `RailPanel.swift` | 18 | — (status dots were already shapes; gold shadow, surface fills) |
| `RailPills.swift` | 11 | `···` (more pill), hand-drawn `WorkingArc` |
| `AskBar/AskPanelViews.swift` | 11 | — |
| `Island/VoiceModeView.swift` | 10 | — (hex fill `0x12100e`, forced dark) |
| `AskBar/AskBarView.swift` | 10 | `→` in the hint sentence |
| `PointAndSay/AnswerCard.swift` | 9 | — (forced dark) |
| `Settings/Legend.swift` | 6 | — (`.rounded` design) |
| `AttentionPanel.swift` | 4 | — (forced dark, radius 24, palette border) |
| `AskBar/AskBarPanel.swift` | 4 | — (`GlassPanel.make`: forced dark, radius 28, palette border) |
| `PointAndSay/PointController.swift` | 2 | — (`malachiteHex`) |
| `Island/IslandWindow.swift`, `Island/IslandPanel.swift` | 2 + 2 | — (forced dark; IslandPanel only logged) |
| `AppDelegate.swift` | 2 | `●N` menu-bar title |
| `Settings/ShortcutRecorder.swift`, `PointAndSay/PointProbe.swift`, `PointAndSay/OverlayWindows.swift`, `PanelProbe.swift` | 1 each | — |
| `Settings/SettingsWindow.swift` | prose "gold ●N" | — |

Tests touched: `FixtureTests.swift:255-257` (`color: "malachite"` → `"#007aff"`), `OverlayModelTests.swift` (+1 test).

## Mapping

### Status semantics (fixed by the owner 08:39; the Windows companion applies the same table)
| State | Colour |
|---|---|
| needs you / waiting on the user (asks, approvals, selection, the recording mic) | `Color.accentColor` |
| running / in flight | NO colour: secondary text + `ProgressView().controlSize(.small)` |
| done / finished | `Color(nsColor: .systemGreen)` (`Color.done`) |
| failed / error / stop | `.systemRed` (`Color.failed`) |
| paused / blocked / waiting on something that is not the user | `.systemOrange` (`Color.paused`), not yellow |
| remote / unknown (E7 fallback) | hollow `Circle` stroked in `.tertiaryLabelColor` (`Color.tertiary`) |

Applied: needs-you rings/bar/count, recommended option, focused row/field, Ask-bar and reply-row mic + status while recording, the voice
pill's listening dot and the island's rec dot = accent; `MarkView(.working)` (the Reduce Motion stand-in) and `.idle` = `.secondary`
filled / hollow, live spinners = `ProgressView`; finished/sent = green; error/stop/undo-stop = red; the island's paused clock glyph and
"paused" pill text = orange; remote agent rows and lapsed marks = hollow tertiary.

### Colours (old token → system colour)
| Old | New | Where |
|---|---|---|
| `.ink` / `Palette.text` | `.primary` (SwiftUI default; root `.foregroundColor(.ink)` deleted) | all text |
| `.ink2` / `Palette.text2` | `.secondary` | meta text, icons at rest |
| `.sand` | `Color.tertiary` = `Color(nsColor: .tertiaryLabelColor)` | dim text, placeholders, hollow remote dots, lapsed ring, hints |
| `.gold` | `Color.accentColor` (`NSColor.controlAccentColor`) | needs-you rings/bar/count, recommended option, wordmark, oracle bolt, "N new" badge, mic meter |
| `.faience` | `Color.accentColor` for focus/selection (focused row/field); `.secondary` for working (filled) and idle (hollow) dots | AgentsPanel rows, reply field, MarkView |
| `.malachite` | `Color.done` = `Color(nsColor: .systemGreen)` | sent/finished/ok chips and receipts, "Done" zone, mic transcript track |
| `.carnelian` | `Color.failed` = `Color(nsColor: .systemRed)` for failed, stop/pause-the-desk tints, lost chord; `Color.accentColor` for the recording mic + status, the voice pill's dot and the island's rec dot (waiting on the user) | PanelView, AskBar, AgentsPanel, VoiceModeView, RecClock |
| `.surface` fills | materials: SwiftUI `.regularMaterial` (rail, pills, island open/detached/no-notch), `NSVisualEffectView(.hudWindow)` / `NSGlassEffectView` (panels, Ask bar, legend, answer card, voice pill) | every background |
| `.rule` / `Palette.border` | `Color.separator` / `NSColor.separatorColor` | strokes, panel borders; `Divider()` plain |
| `Color.ink.opacity(x)` | `Color.primary.opacity(x)` | hover/row/field tints (same opacity) |
| `Color.black.opacity(0.94)` | material (island open/detached); `Color.black` kept ONLY for the notch pill and the notch-height strip of the open pane | island |
| `Color.white.opacity(0.1)` (answer-card caps) | `Color.primary.opacity(0.1)` | AnswerCard `Cap` |
| `NSColor(hex: 0x12100e, 0.91)` | `NSVisualEffectView` `.hudWindow` | voice pill |
| `Palette.gold` (menu bar title) | `NSColor.controlAccentColor` | status item count |
| paused / orange | `Color.paused` = `.systemOrange`: the island's paused clock glyph and "paused" pill text; Settings' notices already used `.orange` | IslandViews, Settings |
| text ON an accent fill | `.white` (as macOS draws selected text) | recommended key cap, "N new" badge, wordmark inner square, finished check, error bar |

`Color.systemBlue` is used nowhere.

### Icons (old glyph → SF Symbol)
| Old | New | Where |
|---|---|---|
| `Text("🎙")` | `Image(systemName: "mic.fill")` (LevelBars in accent while recording) | PanelView reply row V |
| `✕` | `Image(systemName: "xmark")` in a `Button` `.borderless`, `.help("Close")`, accessibility "Close" | PanelView header, AgentsPanel header |
| `‹` / `›` | `chevron.left` / `chevron.right` | PanelView J/K |
| `▣` | `scope` (the rail's Point glyph) | PanelView P |
| `↑` (Send) | `arrow.up` | PanelView ⏎ button |
| `→` (option rows) | `arrow.right` | PanelView options |
| `⌸` | `rectangle.topthird.inset.filled` (the Settings › Island tab already uses it for the island) tinted accent | ClosedPill idle |
| `•` | `Circle().fill(Color.tertiary).frame(4×4)` | BriefPane bullets |
| `↑ N new` | `arrow.up` + text, white on accent capsule | BriefPane |
| `⑂N` | `arrow.triangle.branch` + count | AgentRowView |
| `···` | `ellipsis` | RailPills more |
| `●N` | the count alone (`" N"`), accent | menu-bar status item |
| `→` in prose ("release → sends", "→ model") | words ("release sends in N s", "model") | AskBarView hint, OracleChip verdict |
| `ArcSpinner` / `WorkingArc` (hand-drawn arcs) | `ProgressView().controlSize(.mini)` in the 10 pt dot frame | AgentsPanel, RailPills |

Key caps keep macOS's key symbols (⌘ ⌃ ⌥ ⇧ ⏎ ⎋ ↑ ↓ ← →); sentence punctuation (· — … “ ”) stays.

### Typography (old → text style)
| Old | New |
|---|---|
| `Type.headline` 24 semibold | `.title2.weight(.semibold)` (Ask bar line: `.title2`) |
| `Type.question` 17 semibold | `.title3.weight(.semibold)` |
| `Type.body` 15 · fixed 13/14/15 | `.body` |
| `Type.meta` 12.5 · fixed 12/12.5 | `.callout` |
| fixed 11/11.5 | `.subheadline` |
| `Type.caps` 10.5 medium · fixed 10/10.5 | `.caption` (weight kept) |
| `Type.mono` 10.5 mono · fixed 9/10 mono | `.caption.monospaced()` / `.caption2.monospaced()` — key caps, counts, clocks, ids only |
| fixed 16 (icons) | `.title3` |
| `design: .rounded` (legend chords) | dropped |
| Fixed sizes kept, each with a `// native:` comment | `HarnessIcon` fallback letter `size * 0.55` (scales with the icon box); MarkView's 5 pt checkmark inside a 10 pt dot; `HarnessIcon(size: 16)` is view geometry |

### Shapes (old radius → new)
| Old | New |
|---|---|
| cards 24 (`Type.cardRadius`) | 12 (Attention panel, Agents panel, answer card, island open/detached) |
| Ask bar 28 (`Type.askBarRadius`) | 12 — the Ask bar's height varies (72 pt + receipt rows), so a layer capsule would be wrong; `Type.askBarRadius` deleted |
| rail pills 26 (`Type.pillRadius`) | `Capsule()` (pills by design); `Type.pillRadius` deleted |
| island open 26/22, detached 22, notch pill 18, voice pill 20 | 12 (`Type.cardRadius`) |
| option rows 10 | 10 |
| key caps 4, hint caps 3 | 4 (`Type.capRadius`) |
| buttons | 6 (`Type.buttonRadius`; ShortcutRecorder already 6) |

### Appearance
Every `panel.appearance = Type.dark` / `NSAppearance(named: .darkAqua)` is deleted (rail, Attention panel, `GlassPanel.make`, answer card,
voice pill, `IslandWindow`). The only black left is `ClosedPill` when `notch == true` (`.environment(\.colorScheme, .dark)` keeps its text
light) and the notch-height strip at the top of the attached open pane — the hardware notch is black. The no-notch pill, the open pane
and the detached island are `.regularMaterial`.

### Point-and-say
- `OverlayView.show` strokes trail and ring with `NSColor.controlAccentColor.cgColor`, resolved at draw time.
- `screen_context.color` is `PointController.accentHex`: `controlAccentColor` resolved under `NSApp.effectiveAppearance` →
  `usingColorSpace(.sRGB)` → `OverlayModel.hex(red:green:blue:)` → `"#rrggbb"`. `OverlayModel.hex` is the one pure helper (CompanionCore,
  AppKit-free), tested in `OverlayModelTests.trailColourIsSevenCharHex`.
- `PointProbe` keeps a fixed CONTROL colour for the engine's stamp: the accent's complement (`1 − r, 1 − g, 1 − b`), never shipped on screen
  outside the probe. Its detector classifies a spiral pixel as "ours" when it is within 0.4 (RGB Euclidean) of the accent expressed in the
  capture's colour space (raw `colorAt`, no second conversion); measured distance of the drawn trail over the dark desk is 0.29 — the P3 →
  sRGB round trip desaturates the blue beyond the 0.15 × √3 the 0.85 alpha alone would allow.
- `LevelBars` is `Color.accentColor`.

## Gates
- `env BUILD_LABEL=149n build-lock bash crates/desktop/companion/test.sh` — **178 tests in 28 suites passed** (177 + the hex test).
- `swift build -c release --package-path crates/desktop/companion` — Build complete.
- `bash scripts/bundle-companion.sh` + `codesign --verify --deep --strict "target/release/companion/Theuth Companion.app"` — valid on disk ·
  satisfies its Designated Requirement.
- `perl -CSD -ne 'print … if /Text\("[^"]*[\x{1F300}-\x{1FAFF}][^"]*"\)/' $(find Sources -name '*.swift')` — empty.
- `grep -rnE 'ochre|malachite|carnelian|faience|bone|soot|0x[0-9a-f]{6}|#[0-9a-fA-F]{6}' Sources` — two history comments only
  (`Palette.swift:4`, `PointProbe.swift:15`).

## Probes (in-bundle executable run directly, `--shareable`; windows captured with `screencapture -l` by owner pid; Sky's login-item
companion pid 89373 untouched; all exit 0)
| Flag | PNG |
|---|---|
| `--probe-rail 8` | `/tmp/149n-rail.png` |
| `--probe-agents 6` | `/tmp/149n-agents.png` |
| `--probe-panel finished·ask·approval·review·empty` | `/tmp/149n-panel-finished.png`, `-ask.png`, `-approval.png`, `-review.png`, `-empty.png` |
| `--probe-island closed·open·detached·nonotch --probe-seconds 6` | `/tmp/149n-island-closed.png`, `-open.png`, `-detached.png`, `-nonotch.png` (this Mac has no notch: a 200 × 37 notch is pretended, `IslandPanel.swift:403`) |
| `--ask-probe 6` | `/tmp/149n-ask.png` |
| `--probe-legend` | `/tmp/149n-legend.png` |
| `--probe-settings rail·keys·notifications·engine·island·voiceKey` | `/tmp/149n-settings-rail.png`, `-keys.png`, `-notifications.png`, `-engine.png`, `-island.png`, `-voiceKey.png` |
| `--probe-notify 8` | `/tmp/149n-notify.png` |
| `--probe 6` (live rail + panel against Sky's engine) | `/tmp/149n-probe.png` |
| `--probe-point 14` | `/tmp/149n-point-pill.png` (voice pill), `/tmp/149n-point-overlay.png` (full-screen spiral in accent), `/tmp/149n-point-card.png` (answer card); `/tmp/149-W2-probe.log`: control `120/120` → stop `0/120`, **PASS** |
| `--probe-eye` | not captured (it toggles `sharingType` and logs; nothing to look at) |

Fixed after looking: the open island's notch-height strip is black again (first cut was material beside the notch); the point probe's
first detector (0.3, nominal light-mode accent) read 0/120 on the control capture — see Point-and-say above.

R1 (owner's review of the probes, 09:09), every PNG above re-captured afterwards:
1. Header key caps wrapped one letter per line ("e / s / c") beside long card titles. The rule is now in the shared views: `KeyCap`,
   `Hint`, `HeaderButton` and the answer card's `Cap` are `.lineLimit(1).fixedSize().layoutPriority(1)`; the title beside them gives way —
   `.lineLimit(1).truncationMode(.tail).layoutPriority(0)` on the card header name, the Inbox title, the Agents title and the answer card's
   status. The island request box pairs a field with an arrow button, no caps. Confirmed in `149n-panel-{finished,ask,approval,review,empty}.png`
   ("w3-149-island", "w2-149-point", "w1-149-parity", "w2-149-overlay"), `149n-agents.png`, `149n-ask.png`, `149n-point-card.png`.
2. The omp harness glyph read as a subscript "o" + "MP". `crates/desktop/companion-assets/harness/omp.svg` is the owner's lowercase wordmark
   (o, m, p on one x-height, p with a descender; `/tmp/omp-wordmark.svg` verbatim); `swift scripts/render-harness-icons.swift` regenerated
   `omp-{16,20,24,32}.png` (the other glyphs' PNGs re-rendered byte-identical). Confirmed "omp" at 16 px in the panel status line
   (`149n-panel-finished.png`, `-approval.png`) and 20 px in the agents rows (`149n-agents.png`).

## Deviations and what is not native
- `Color.tertiary/.separator/.failed/.done/.paused` are five aliases to system colours in `Palette.swift` (the file keeps its name: it also
  holds `Log`, `Flags`, `Settings`, `LevelBars`). `Type` stays as six text-style aliases plus four radii. No hex, no palette.
- The notch pill (`ClosedPill`, `notch == true`) and the notch-height strip of the attached open pane are `Color.black` — the hardware
  notch is black (the brief's one exception).
- The status table asks for `ProgressView().controlSize(.small)` for running; the rail-pill and agents-row dots are 10 pt frames and `.small`
  is 16 pt, so those two sites use `.controlSize(.mini)` (the island strip's activity spinner is already `.mini` in a 10 pt text line). Under
  Reduce Motion the running dot is a static `.secondary` circle (`MarkView(.working)`), since a `ProgressView` would still spin.
- Two fixed point sizes remain, commented: MarkView's 5 pt heavy checkmark inside the 10 pt finished dot and `HarnessIcon`'s fallback letter
  (`size * 0.55`). `ShortcutRecorder` keeps `NSFont.systemFont(ofSize: 12, weight: .medium)` for its AppKit attributed string (pre-existing,
  not a palette use).
- `PointProbe`'s control colour is a fixed non-system colour (the accent's complement) by design — it never ships on screen outside the probe.
- Pre-existing layout, untouched (layout is out of scope): the legend's row count now exceeds its 760 pt cap on this display, so the header
  is clipped in `/tmp/149n-legend.png` (same on main); Settings › Rail's long caption pushes the form wider than 500 pt (`"ail presence"`
  in `/tmp/149n-settings-rail.png`, same on main).
- The agents-panel stop button and the reply-row buttons keep `.buttonStyle(.plain)`: their 24–28 pt filled hit shapes are the existing
  layout. Header close buttons are `.borderless` with `.help("Close")`.
