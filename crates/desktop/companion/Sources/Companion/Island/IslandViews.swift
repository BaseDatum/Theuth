import CompanionCore
import SwiftUI
import TheuthKit

/// What the controller drives and the views read (the island's own UI state; the meeting data is `store.meeting`).
@MainActor
final class IslandUI: ObservableObject {
    @Published var state: IslandState = .closed
    @Published var anchor: IslandGeometry.Anchor?
    @Published var boxFocused = false
    @Published var menuOpen = false
    @Published var transcriptOpen = false
    @Published var eyeHidden = true
    /// `decision_mode('meeting')` every 30 s (`Island.tsx:337`).
    @Published var oracle: ModeInfo?
    @Published var verdictFaded = false
    /// ⌃⇧M: the stop hold's start; nil = no hold.
    @Published var stopHoldStarted: Date?
    @Published var now = Date()
    @Published var requestText = ""
    @Published var blurTick = 0
    /// Reported by the views, in SwiftUI's window-global (top-left) coordinates — the hosting view's own, it is flipped.
    @Published var drawn: CGRect = .zero
    /// Every control's frame by name (`islandRect`), same coordinates; the controller's key test (`box`) and the probes.
    var rects: [String: CGRect] = [:]
    /// The topic E dismisses (0 = the first; hover moves it).
    @Published var focusedTopic = 0

    /// A grip `mouseDown` (click count 1 drags, 2 snaps back); a click on the live pill or the strip (collapsed ↔ open).
    var onGrip: ((NSEvent) -> Void)?
    var onCollapseToggle: (() -> Void)?
    var isOpen: Bool { if case .closed = state { return false } else { return true } }
    var isDetached: Bool { if case .detached = state { return true } else { return false } }
}

extension View {
    /// Reports this control's SwiftUI-global frame under `name` (`IslandUI.rects`).
    func islandRect(_ name: String, _ ui: IslandUI) -> some View {
        onGeometryChange(for: CGRect.self) { $0.frame(in: .global) } action: { ui.rects[name] = $0 }
    }
}

/// The island's root inside the (oversized when attached) window: the pill/pane sits top-centre and animates in
/// place (D9); detached it fills the window. Reports its drawn frame for `hitTest`.
struct IslandRoot: View {
    @ObservedObject var ui: IslandUI
    @ObservedObject var store: Store
    @ObservedObject var settings = SettingsModel.shared

    var body: some View {
        ZStack(alignment: .top) {
            Color.clear
            Group {
                if ui.isDetached {
                    DetachedIsland(ui: ui, store: store)
                } else if ui.isOpen {
                    OpenIsland(ui: ui, store: store, width: IslandLayout.openWidth, attached: true)
                        .transition(.asymmetric(insertion: .scale(scale: 0.96, anchor: .top).combined(with: .opacity), removal: .opacity))
                } else if let a = ui.anchor {
                    ClosedPill(ui: ui, store: store, size: a.rect.size, notch: a.notch)
                        .transition(.opacity)
                }
            }
            .onGeometryChange(for: CGRect.self) { $0.frame(in: .global) } action: { ui.drawn = $0 }
            // Attached to a screen without a notch the pill (and the pane) hang under the menu bar, not over it.
            .padding(.top, ui.isDetached ? 0 : (ui.anchor.map { $0.screen.frame.maxY - $0.rect.maxY } ?? 0))
        }
        .frame(maxWidth: .infinity, maxHeight: .infinity, alignment: .top)
        .animation(ui.isOpen ? .bouncy(duration: 0.4) : .smooth(duration: 0.4), value: ui.isOpen)
        .foregroundColor(.primary)
        .onChange(of: ui.blurTick) { _, _ in NSApp.keyWindow?.makeFirstResponder(nil) }
    }
}

// MARK: closed

/// The notch-hugging pill: meters · the speaker · `rec mm:ss` · the topic count. Hugs the real notch (its width + wings,
/// its height) or is the 300 × menu-bar fallback. Idle ("Always show", nothing live) → the wordmark, click → `theuth://meetings`.
/// Over the hardware notch the pill is black like the notch itself (and so always dark); the fallback pill is a material.
struct ClosedPill: View {
    @ObservedObject var ui: IslandUI
    @ObservedObject var store: Store
    let size: CGSize
    let notch: Bool
    @Environment(\.colorScheme) private var scheme

    var body: some View {
        let s = store.meeting ?? .empty
        let w = notch ? size.width + 2 * IslandLayout.pillWings : size.width
        let h = size.height
        let font: Font = h >= 34 ? .body : .subheadline
        let small: Font = h >= 34 ? .callout : .caption
        HStack(spacing: 10) {
            if s.isLive || s.state == .starting || s.state == .stopping {
                Meters(level: s.level, height: h * 0.45)
                Text(s.state == .paused ? "paused" : (speaker(s) ?? "Meeting")).font(font.weight(.semibold)).foregroundColor(s.state == .paused ? .paused : .primary).lineLimit(1).layoutPriority(1)
                Spacer(minLength: notch ? size.width - 20 : 4)
                RecClock(slice: s, now: ui.now, font: small)
                if !s.topics.isEmpty {
                    Text("\(s.topics.count) topic\(s.topics.count == 1 ? "" : "s")").font(small).foregroundColor(.tertiary).lineLimit(1).fixedSize()
                }
            } else {
                Image(systemName: "rectangle.topthird.inset.filled").font(font.weight(.bold)).foregroundStyle(Color.accentColor).accessibilityHidden(true)
                Text(s.briefs.isEmpty ? "theuth" : "\(s.briefs.count) brief\(s.briefs.count == 1 ? "" : "s")").font(font).foregroundColor(.secondary)
                Spacer(minLength: 0)
            }
        }
        .padding(.horizontal, notch ? 14 : 12)
        .frame(width: w, height: h)
        .background {
            if notch {
                IslandShape(radius: Type.cardRadius, bottomOnly: true).fill(Color.black) // native: the hardware notch is black
            } else {
                IslandShape(radius: h / 2, bottomOnly: false).fill(.regularMaterial)
            }
        }
        .environment(\.colorScheme, notch ? .dark : scheme)
        .contentShape(Rectangle())
        .islandRect("pill", ui)
        .onTapGesture {
            if s.isLive || s.state == .starting || s.state == .stopping { ui.onCollapseToggle?() }
            else { store.onOpenURL?(URL(string: "theuth://meetings")!) }
        }
        .accessibilityLabel(s.isLive ? "Meeting island, \(s.topics.count) topics; click to open" : "theuth; click opens Meetings")
    }

    private func speaker(_ s: MeetingSlice) -> String? {
        guard let l = s.lines.last(where: { $0.utterance.track == .system }) else { return nil }
        return l.utterance.speaker.flatMap { $0.hasPrefix("s") ? "Speaker \($0.dropFirst())" : $0 }
    }
}

// MARK: open / detached

/// The strip (meters, clock, oracle chip, topics or ⋮ menu, badges, activity, eye, ⋮) + the brief pane + the collapsed
/// transcript + the request box — `Island.tsx:181-398` (D11).
struct OpenIsland: View {
    @ObservedObject var ui: IslandUI
    @ObservedObject var store: Store
    let width: CGFloat
    let attached: Bool

    var body: some View {
        let s = store.meeting ?? .empty
        VStack(alignment: .leading, spacing: 0) {
            if attached, let a = ui.anchor, a.notch {
                // The real notch's silhouette: the pane hangs below it. The strip at notch height stays black (the
                // notch is black; its wings sit in the menu bar beside it) — the pane below is material like every surface.
                Color.black.frame(height: a.rect.height) // native: the hardware notch is black
            }
            if case .detected(let d)? = s.prompt {
                PromptView(ui: ui, store: store, detection: d)
            } else if case .upcoming(let e, let p, let b)? = s.prompt {
                UpcomingView(ui: ui, store: store, event: e, project: p, brief: b)
            } else {
                Strip(ui: ui, store: store, slice: s)
                    .background(DragGrip(ui: ui))
                if let w = MeetingReducer.captureWarning(s.capture, app: s.app) {
                    Divider()
                    CaptureWarningRow(ui: ui, text: w)
                }
                if case .stop(let app, let secs)? = s.prompt {
                    Divider()
                    StopRow(ui: ui, store: store, app: app, releasedForS: secs)
                }
                if let b = s.current {
                    Divider()
                    BriefPane(ui: ui, store: store, slice: s, brief: b)
                }
                Divider()
                TranscriptRow(ui: ui, slice: s)
                Divider()
                RequestBox(ui: ui, store: store)
            }
        }
        .frame(width: width)
        // Detached, `DetachedIsland` draws the one material behind the grip and the pane (two stacked would seam).
        .background { if attached { IslandShape(radius: Type.cardRadius, bottomOnly: true).fill(.regularMaterial) } }
    }
}

struct DetachedIsland: View {
    @ObservedObject var ui: IslandUI
    @ObservedObject var store: Store
    var body: some View {
        VStack(spacing: 0) {
            // The grip NSView is the overlay: a SwiftUI shape in front of it would take the hit (149i).
            Capsule().fill(Color.primary.opacity(0.2)).frame(width: 36, height: 4).padding(.top, 8).padding(.bottom, 2)
                .frame(maxWidth: .infinity)
                .overlay(DragGrip(ui: ui))
                .islandRect("grip-detached", ui)
                .accessibilityLabel("Drag handle; double-click returns the island to the notch")
            OpenIsland(ui: ui, store: store, width: IslandLayout.detachedWidth, attached: false)
        }
        .frame(width: IslandLayout.detachedWidth)
        .background(.regularMaterial, in: RoundedRectangle(cornerRadius: Type.cardRadius, style: .continuous))
    }
}

/// 158 §5.2 (a)/(b) — the detection prompt in the open pane: `<App> is using your microphone`, the event line + project chip
/// when W2/W3 filled them, Start recording · Not now, ⋮ → Never for <App>, and the lawful sentence (R3). No clock timeout.
struct PromptView: View {
    @ObservedObject var ui: IslandUI
    @ObservedObject var store: Store
    let detection: Detection

    static let lawful = "By starting the recording you acknowledge the recording is lawful."

    var body: some View {
        let app = detection.app.name
        let headline = "\(app) is using your microphone"
        VStack(alignment: .leading, spacing: 8) {
            HStack(spacing: 6) {
                DragGrip(ui: ui).frame(width: 18, height: 26).islandRect("grip", ui)
                Circle().fill(Color.accentColor).frame(width: 7, height: 7).overlay(Circle().stroke(Color.accentColor.opacity(0.3), lineWidth: 3)).accessibilityHidden(true)
                Text(headline).font(.callout.weight(.semibold)).lineLimit(1)
                Spacer(minLength: 0)
                RoundButton(icon: EyeGlyph.symbol(ui.eyeHidden), label: EyeGlyph.label(ui.eyeHidden), tint: EyeGlyph.tint(ui.eyeHidden)) {
                    ui.eyeHidden.toggle()
                }.islandRect("eye", ui)
                RoundButton(icon: "ellipsis", label: "More: never for \(app)") { ui.menuOpen.toggle() }.islandRect("menu", ui)
            }
            if detection.event != nil || detection.project != nil {
                HStack(spacing: 8) {
                    if let e = detection.event?.event { Text(MeetingReducer.eventLine(e)).font(.subheadline).foregroundColor(.secondary).lineLimit(1) }
                    if let p = detection.project { ProjectChip(ui: ui, project: p) }
                }.padding(.leading, 24)
            }
            if let b = detection.brief, !b.isEmpty { BriefBullets(bullets: b).padding(.leading, 24) } // §5.2 (c) above (b): both apply
            HStack(spacing: 8) {
                if ui.menuOpen {
                    PromptButton("Never for \(app)") { ui.menuOpen = false; store.meetingNever(detection) }.islandRect("never", ui)
                    StripButton(nil, icon: "xmark") { ui.menuOpen = false }.islandRect("menu-close", ui)
                } else {
                    PromptButton("Start recording", primary: true) { store.meetingStartDetected(detection) }.islandRect("start", ui)
                    PromptButton("Not now") { store.meetingNotNow(detection) }.islandRect("not-now", ui)
                }
            }.padding(.leading, 24)
            Text(Self.lawful).font(.caption).foregroundColor(.secondary).fixedSize(horizontal: false, vertical: true).padding(.leading, 24)
        }
        .padding(.horizontal, 14).padding(.top, 8).padding(.bottom, 10)
        .accessibilityElement(children: .contain).accessibilityLabel(headline)
        .islandRect("prompt", ui)
    }
}

/// 158 §5.2 (c) — the pre-meeting brief: `In N min · <Title>` + project chip, the brief's bullets, `From the last meetings in
/// <project> · attendees …`, Start recording · Dismiss, the lawful sentence (R3). Clears at start + 5 min (`MeetingReducer.tick`).
struct UpcomingView: View {
    @ObservedObject var ui: IslandUI
    @ObservedObject var store: Store
    let event: EventRef
    let project: String?
    let brief: [String]

    var body: some View {
        let headline = MeetingReducer.upcomingLine(event, now: ui.now)
        VStack(alignment: .leading, spacing: 8) {
            HStack(spacing: 6) {
                DragGrip(ui: ui).frame(width: 18, height: 26).islandRect("grip", ui)
                Image(systemName: "clock").font(.caption.weight(.semibold)).foregroundColor(.accentColor).accessibilityHidden(true)
                Text(headline).font(.callout.weight(.semibold)).lineLimit(1)
                Spacer(minLength: 0)
                if let p = project { ProjectChip(ui: ui, project: p) }
                RoundButton(icon: EyeGlyph.symbol(ui.eyeHidden), label: EyeGlyph.label(ui.eyeHidden), tint: EyeGlyph.tint(ui.eyeHidden)) {
                    ui.eyeHidden.toggle()
                }.islandRect("eye", ui)
            }
            if !brief.isEmpty { BriefBullets(bullets: brief).padding(.leading, 24) }
            if let src = MeetingReducer.upcomingSource(project: project, attendees: event.attendees) {
                Text(src).font(.caption).foregroundColor(.secondary).lineLimit(2).padding(.leading, 24)
            }
            HStack(spacing: 8) {
                PromptButton("Start recording", primary: true) { store.meetingStartUpcoming(event, project: project) }.islandRect("start", ui)
                PromptButton("Dismiss") { store.meetingDismissUpcoming() }.islandRect("dismiss", ui)
            }.padding(.leading, 24)
            Text(PromptView.lawful).font(.caption).foregroundColor(.secondary).fixedSize(horizontal: false, vertical: true).padding(.leading, 24)
        }
        .padding(.horizontal, 14).padding(.top, 8).padding(.bottom, 10)
        .accessibilityElement(children: .contain).accessibilityLabel(headline)
        .islandRect("upcoming", ui)
    }
}

/// The prompt's project chip (`⟨ theuth ⟩`, §5.2 b/c).
struct ProjectChip: View {
    @ObservedObject var ui: IslandUI
    let project: String
    var body: some View {
        Text(project).font(.caption).padding(.horizontal, 8).frame(height: 20).background(Capsule().fill(Color.primary.opacity(0.08)))
            .lineLimit(1).accessibilityLabel("Project \(project)").islandRect("project", ui)
    }
}

/// The pre-meeting brief's bullets (≤ 3 by §9.4; drawn as sent).
struct BriefBullets: View {
    let bullets: [String]
    var body: some View {
        VStack(alignment: .leading, spacing: 3) {
            ForEach(Array(bullets.enumerated()), id: \.offset) { _, b in
                Text("• \(b)").font(.subheadline).lineLimit(2).fixedSize(horizontal: false, vertical: true)
            }
        }
    }
}

/// 158 §5.2 (d) — under the live strip after `AppReleased`: `<App> stopped using your microphone N s ago` · Stop recording? ·
/// Stop / Keep recording. Nothing stops by itself (D4).
struct StopRow: View {
    @ObservedObject var ui: IslandUI
    @ObservedObject var store: Store
    let app: MeetingProcess
    let releasedForS: UInt32

    var body: some View {
        let line = "\(app.name) stopped using your microphone \(releasedForS) s ago"
        VStack(alignment: .leading, spacing: 6) {
            Text(line).font(.callout).foregroundColor(.secondary).lineLimit(1)
            HStack(spacing: 8) {
                Text("Stop recording?").font(.callout.weight(.semibold))
                Spacer(minLength: 0)
                PromptButton("Stop") { store.meetingStop() }.islandRect("stop-now", ui)
                PromptButton("Keep recording", primary: true) { store.meetingKeepRecording() }.islandRect("keep", ui)
            }
        }
        .padding(.horizontal, 14).padding(.vertical, 8)
        .accessibilityElement(children: .contain).accessibilityLabel(line)
        .islandRect("stop-row", ui)
    }
}

/// 173 — under the live strip while a capture track is not heard: `captureWarning`'s one line (the web island's tooltip
/// wording), gone the moment both tracks are `Ok`.
struct CaptureWarningRow: View {
    @ObservedObject var ui: IslandUI
    let text: String

    var body: some View {
        HStack(spacing: 6) {
            Image(systemName: "exclamationmark.triangle.fill").font(.caption).foregroundColor(.paused).accessibilityHidden(true)
            Text(text).font(.callout).foregroundColor(.secondary).lineLimit(1).truncationMode(.tail)
        }
        .padding(.horizontal, 14).padding(.vertical, 6)
        .frame(maxWidth: .infinity, alignment: .leading)
        .help(text)
        .accessibilityElement(children: .combine).accessibilityLabel(text)
        .islandRect("capture-warning", ui)
    }
}

/// The prompt's answers: the primary is an accent capsule (a non-key panel draws `.borderedProminent` grey), the rest the
/// strip's capsule. The title is the accessibility label.
struct PromptButton: View {
    let title: String
    var primary = false
    let action: () -> Void
    init(_ title: String, primary: Bool = false, action: @escaping () -> Void) { self.title = title; self.primary = primary; self.action = action }
    var body: some View {
        Button(action: action) {
            Text(title).font(.subheadline.weight(primary ? .semibold : .regular))
                .foregroundStyle(primary ? Color.white : Color.primary)
                .padding(.horizontal, 12).frame(height: 26)
                .background(Capsule().fill(primary ? Color.accentColor : Color.primary.opacity(0.08)))
                .contentShape(Capsule())
        }.buttonStyle(.plain)
    }
}

struct Strip: View {
    @ObservedObject var ui: IslandUI
    @ObservedObject var store: Store
    let slice: MeetingSlice

    var body: some View {
        HStack(spacing: 6) {
            // The left 24 pt is always the grip — the NSView itself, nothing hit-testable in front of it.
            DragGrip(ui: ui).frame(width: 18, height: IslandLayout.stripHeight).islandRect("grip", ui)
            Meters(level: slice.level, height: 16)
            RecClock(slice: slice, now: ui.now, font: .subheadline)
            OracleChip(ui: ui, slice: slice)
            HStack(spacing: 4) {
                if ui.menuOpen {
                    StripButton(slice.state == .paused ? "resume" : "pause", icon: slice.state == .paused ? "play.fill" : "pause.fill") {
                        ui.menuOpen = false
                        slice.state == .paused ? store.meetingResume() : store.meetingPause()
                    }.islandRect("pause", ui)
                    StripButton("stop", icon: "stop.fill") { ui.menuOpen = false; store.meetingStop() }.islandRect("stop", ui)
                    StripButton(nil, icon: "xmark") { ui.menuOpen = false }.islandRect("menu-close", ui)
                } else {
                    ForEach(Array(slice.topics.enumerated()), id: \.element.id) { i, t in
                        TopicButton(ui: ui, topic: t, tapped: slice.tapped.contains(t.id), focused: i == ui.focusedTopic,
                                    tap: { store.meetingTap(t.id) }, dismiss: { store.meetingDismiss(t.id) })
                            .onHover { if $0 { ui.focusedTopic = i } }
                            .islandRect("topic:\(t.id)", ui)
                    }
                    ForEach(badges, id: \.self) { Text($0).font(.caption).foregroundColor(.tertiary).lineLimit(1) }
                    if let a = slice.activity {
                        HStack(spacing: 3) { ProgressView().controlSize(.mini); Text(a).font(.caption) }.foregroundColor(.tertiary)
                    }
                    if let p = slice.progress { Text("\(p.stage.rawValue.lowercased()) \(p.pct) %").font(.caption).foregroundColor(.tertiary) }
                    if let h = ui.stopHoldStarted {
                        let left = max(0, Settings.undoDelay - ui.now.timeIntervalSince(h))
                        Text("stopping in \(String(format: "%.1f", left)) s · ⎋ undo").font(.caption).foregroundColor(.failed)
                    }
                }
            }
            .frame(maxWidth: .infinity, alignment: .leading)
            .clipped()
            if !ui.isDetached, !ui.boxFocused, ui.stopHoldStarted == nil, let m = chord(.islandMute), let st = chord(.islandStop) {
                Hint(m, "mute"); Hint(st, "stop")
            }
            RoundButton(icon: EyeGlyph.symbol(ui.eyeHidden), label: EyeGlyph.label(ui.eyeHidden), tint: EyeGlyph.tint(ui.eyeHidden)) {
                ui.eyeHidden.toggle()
            }.islandRect("eye", ui)
            RoundButton(icon: "ellipsis", label: "Pause or stop") { ui.menuOpen.toggle() }.islandRect("menu", ui)
        }
        .padding(.leading, 4).padding(.trailing, 8)
        .frame(height: IslandLayout.stripHeight)
    }

    private var badges: [String] {
        // The capture badge is the live word; a degradation that says the same (the fallback, a denied permission) is not repeated.
        let alert = MeetingReducer.captureBadge(slice.capture)
        return slice.degraded.compactMap(badge).filter { $0 != alert } + (alert.map { [$0] } ?? [])
            + (slice.approvalWaiting ? ["approval waiting"] : []) + (slice.listening ? ["listening…"] : [])
    }

    /// `Island.tsx` `badge`; a `SystemAudioFallback` that ended is history, not a badge.
    private func badge(_ d: Degradation) -> String? {
        switch d {
        case .micNoAec: return "headphones recommended"
        case .modelDown: return "model down"
        case .diskLow: return "disk low"
        case .nearMaxLength: return "10 min left"
        case .sttDown: return "transcription down"
        case .systemAudioFallback(_, let ended): return ended ? nil : "system audio fallback"
        case .permissionDenied(let p): return p == "Microphone" ? "no mic permission" : "no system audio permission"
        case .other("NoNote"): return "no note"
        case .other("DiarizationDown"): return "speaker labels down"
        case .other: return nil
        }
    }

    private func chord(_ a: KeyAction) -> String? { SettingsModel.shared.keys.chord(for: a)?.symbol }
}

struct TopicButton: View {
    let ui: IslandUI
    let topic: Topic
    let tapped: Bool
    let focused: Bool
    let tap: () -> Void
    let dismiss: () -> Void
    @State private var hovering = false

    var body: some View {
        HStack(spacing: 4) {
            Text(topic.label).font(.callout).lineLimit(1)
            if topic.source == .oracle { Image(systemName: "bolt.fill").font(.caption2).foregroundStyle(Color.accentColor).accessibilityLabel("published by the decision oracle") }
            else if topic.cached != nil { Circle().fill(Color.accentColor).frame(width: 5, height: 5).accessibilityLabel("cached brief") }
            if hovering {
                Button(action: dismiss) { Image(systemName: "xmark").font(.caption2).foregroundColor(.tertiary) }.buttonStyle(.plain).accessibilityLabel("Dismiss")
                    .islandRect("dismiss:\(topic.id)", ui)
            }
        }
        .padding(.horizontal, 10).frame(height: 28).frame(maxWidth: 128)
        .background(Capsule().fill(Color.primary.opacity(hovering ? 0.14 : 0.08)))
        .overlay(Capsule().stroke(Color.primary.opacity(focused ? 0.25 : 0), lineWidth: 1))
        .opacity(tapped ? 0.5 : 1)
        .contentShape(Capsule())
        .onTapGesture(perform: tap)
        .onHover { hovering = $0 }
        .help(topic.label)
        .accessibilityLabel("Topic \(topic.label); tap for the brief, E dismisses")
    }
}

struct BriefPane: View {
    @ObservedObject var ui: IslandUI
    @ObservedObject var store: Store
    let slice: MeetingSlice
    let brief: Brief

    var body: some View {
        HStack(alignment: .top, spacing: 8) {
            VStack(alignment: .leading, spacing: 4) {
                Text(brief.title).font(.callout.weight(.semibold)).fixedSize(horizontal: false, vertical: true)
                ForEach(Array(brief.bullets.prefix(3).enumerated()), id: \.offset) { _, b in
                    HStack(alignment: .top, spacing: 6) {
                        Circle().fill(Color.tertiary).frame(width: 4, height: 4).padding(.top, 5).accessibilityHidden(true)
                        Text(b).fixedSize(horizontal: false, vertical: true)
                    }.font(.callout).foregroundColor(.secondary)
                }
                HStack(spacing: 8) {
                    StripButton("deeper", icon: "plus.magnifyingglass") { store.meetingDeeper(brief.id) }
                        .help("Ask for a longer, more specific brief on this").islandRect("deeper", ui)
                    if !ui.boxFocused, let d = SettingsModel.shared.keys.chord(for: .islandDeeper)?.symbol { Hint(d, "deeper") }
                    Text(brief.source.rawValue.lowercased()).font(.caption).foregroundColor(.tertiary)
                }.padding(.top, 2)
            }
            .frame(maxWidth: .infinity, alignment: .leading)
            VStack(spacing: 2) {
                Button { store.meetingMove(1) } label: { Image(systemName: "chevron.up") }.buttonStyle(.plain)
                    .disabled(slice.cursor >= slice.briefs.count - 1).opacity(slice.cursor >= slice.briefs.count - 1 ? 0.3 : 1)
                    .accessibilityLabel("Newer brief").islandRect("newer", ui)
                if slice.unseen > 0 {
                    HStack(spacing: 2) { Image(systemName: "arrow.up"); Text("\(slice.unseen) new") }
                        .font(.caption.weight(.semibold)).foregroundStyle(.white)
                        .padding(.horizontal, 6).padding(.vertical, 1).background(Capsule().fill(Color.accentColor))
                        .accessibilityLabel("\(slice.unseen) new briefs")
                }
                Button { store.meetingMove(-1) } label: { Image(systemName: "chevron.down") }.buttonStyle(.plain)
                    .disabled(slice.cursor <= 0).opacity(slice.cursor <= 0 ? 0.3 : 1)
                    .accessibilityLabel("Older brief").islandRect("older", ui)
                Text("\(slice.cursor + 1)/\(slice.briefs.count)").font(.caption2.monospaced()).foregroundColor(.tertiary)
            }
            .foregroundColor(.secondary)
        }
        .padding(.horizontal, 14).padding(.vertical, 10)
        .islandRect("brief", ui)
    }
}

/// `▸ Transcript` collapsed; open = the last six lines `me:` / `Speaker N:` / `them:` (`IslandTranscript.tsx`, `meeting.ts:46-50`).
struct TranscriptRow: View {
    @ObservedObject var ui: IslandUI
    let slice: MeetingSlice

    var body: some View {
        VStack(alignment: .leading, spacing: 3) {
            Button { ui.transcriptOpen.toggle() } label: {
                HStack(spacing: 4) {
                    Image(systemName: ui.transcriptOpen ? "chevron.down" : "chevron.right").font(.caption2)
                    Text("Transcript").font(.subheadline)
                    Text("\(slice.lines.count)").font(Type.mono)
                    Spacer()
                }.foregroundColor(.tertiary).contentShape(Rectangle())
            }.buttonStyle(.plain).islandRect("transcript", ui)
            if ui.transcriptOpen {
                ForEach(Array(slice.lines.suffix(6).enumerated()), id: \.offset) { _, l in
                    HStack(alignment: .top, spacing: 6) {
                        Text(who(l)).font(.subheadline.weight(.medium)).foregroundColor(l.utterance.track == .mic ? .done : .secondary)
                        Text(l.utterance.text).font(.subheadline).foregroundColor(l.open ? .tertiary : .primary).lineLimit(2)
                    }
                }
            }
        }
        .padding(.horizontal, 14).padding(.vertical, 6)
    }

    private func who(_ l: MeetingSlice.Line) -> String {
        if l.utterance.track == .mic { return "me:" }
        if let s = l.utterance.speaker, s.hasPrefix("s"), Int(s.dropFirst()) != nil { return "Speaker \(s.dropFirst()):" }
        return "them:"
    }
}

/// A typed request to the meeting session (a bare topic name = brief it). Typing makes the panel key; ⏎ sends,
/// ⎋ or blur hands key back (`island_resign_key` today).
struct RequestBox: View {
    @ObservedObject var ui: IslandUI
    @ObservedObject var store: Store
    @FocusState private var focused: Bool

    var body: some View {
        HStack(spacing: 6) {
            TextField("Ask the meeting agent… a topic name = brief it", text: $ui.requestText)
                .textFieldStyle(.plain).font(.callout)
                .focused($focused)
                .onSubmit(send)
                .onChange(of: focused) { _, f in ui.boxFocused = f }
                .onChange(of: ui.blurTick) { _, _ in focused = false }
                .accessibilityLabel("Ask the meeting agent")
            Button(action: send) { Image(systemName: "arrow.up").font(.subheadline.weight(.semibold)) }
                .buttonStyle(.plain).foregroundColor(.secondary)
                .disabled(ui.requestText.trimmingCharacters(in: .whitespaces).isEmpty)
                .opacity(ui.requestText.trimmingCharacters(in: .whitespaces).isEmpty ? 0.3 : 1)
                .accessibilityLabel("Send request").islandRect("send", ui)
        }
        .padding(.horizontal, 14).padding(.vertical, 7)
        .islandRect("box", ui) // the row: a click anywhere in it makes the panel key (`wantsKeyOnClick`)
    }

    private func send() {
        let t = ui.requestText.trimmingCharacters(in: .whitespacesAndNewlines)
        guard !t.isEmpty else { return }
        store.meetingRequest(t)
        ui.requestText = ""
        focused = false
    }
}

// MARK: pieces

/// Two bars from the `Level` event at 10 Hz (`Island.tsx:439-457`): mic accent, system secondary.
struct Meters: View {
    let level: Level?
    let height: CGFloat
    var body: some View {
        HStack(alignment: .bottom, spacing: 2) {
            RoundedRectangle(cornerRadius: 1.5).fill(Color.accentColor).frame(width: 5, height: bar(level?.micRms))
            RoundedRectangle(cornerRadius: 1.5).fill(Color.secondary).frame(width: 5, height: bar(level?.systemRms))
        }
        .frame(height: height, alignment: .bottom)
        .animation(.linear(duration: 0.1), value: level)
        .accessibilityHidden(true)
    }
    private func bar(_ rms: Float?) -> CGFloat { height * (0.25 + 0.75 * CGFloat(min(1, (rms ?? 0) * 10))) }
}

/// `rec mm:ss`; a pause glyph while paused (`Island.tsx:460-473`).
struct RecClock: View {
    let slice: MeetingSlice
    let now: Date
    let font: Font
    var body: some View {
        HStack(spacing: 4) {
            if slice.state == .paused { Image(systemName: "pause.fill").font(font).imageScale(.small).foregroundColor(.paused) }
            else { Circle().fill(Color.accentColor).frame(width: 7, height: 7).overlay(Circle().stroke(Color.accentColor.opacity(0.3), lineWidth: 3)) }
            Text(MeetingReducer.mmss(MeetingReducer.elapsed(slice, now: now))).font(font.monospaced()).monospacedDigit().lineLimit(1).fixedSize()
        }
        .foregroundColor(.secondary)
        .accessibilityLabel("Recording \(MeetingReducer.mmss(MeetingReducer.elapsed(slice, now: now)))")
    }
}

/// 57 §3.4: the mode badge (`shadow` / `active`; nothing when off) + `deciding` → `skip` / `brief` / `model` (`Island.tsx:338-373`).
struct OracleChip: View {
    @ObservedObject var ui: IslandUI
    let slice: MeetingSlice
    var body: some View {
        let effective = ui.oracle?.effective.rawValue ?? "off"
        let last = slice.oracle.last
        if effective != "off" || slice.deciding != nil || last != nil {
            HStack(spacing: 3) {
                if effective != "off" { Text(effective.uppercased()).tracking(0.5) }
                if slice.deciding != nil { Image(systemName: "bolt.fill").font(.caption2).foregroundStyle(Color.accentColor); Text("deciding").foregroundStyle(Color.accentColor) }
                else if let l = last, !ui.verdictFaded { Text(verdict(l.family)).opacity(l.family == "skip" ? 0.6 : 1) }
            }
            .font(.caption).foregroundColor(.tertiary)
            .padding(.horizontal, 6).frame(height: 18).background(Capsule().fill(Color.primary.opacity(0.08)))
            .help("decision oracle \(effective)")
        }
    }
    private func verdict(_ family: String) -> String { family == "skip" ? "skip" : family == "brief" ? "brief" : "model" }
}

struct StripButton: View {
    let title: String?
    let icon: String
    let action: () -> Void
    init(_ title: String?, icon: String, action: @escaping () -> Void) { self.title = title; self.icon = icon; self.action = action }
    var body: some View {
        Button(action: action) {
            HStack(spacing: 4) {
                Image(systemName: icon).font(.caption2)
                if let t = title { Text(t).font(.subheadline) }
            }
            .padding(.horizontal, 9).frame(height: 24).background(Capsule().fill(Color.primary.opacity(0.08)))
            .contentShape(Capsule())
        }.buttonStyle(.plain)
    }
}

struct RoundButton: View {
    let icon: String
    let label: String
    var tint: Color? = nil
    let action: () -> Void
    var body: some View {
        Button(action: action) {
            Image(systemName: icon).font(.callout).foregroundColor(tint ?? .secondary).frame(width: 26, height: 26).contentShape(Circle())
        }.buttonStyle(.plain).help(label).accessibilityLabel(label)
    }
}

/// 149i addendum E: the eye says at a glance whether screen-share viewers can see the island — an orange filled eye while
/// VISIBLE (`sharingType .readOnly`: caution, people can see it), a green slashed eye while HIDDEN (`.none`: safe). System
/// colours, the glyph alone tinted; the same glyph, tint and label wherever the state is shown (strip, Settings › Island).
enum EyeGlyph {
    static func symbol(_ hidden: Bool) -> String { hidden ? "eye.slash.fill" : "eye.fill" }
    static func tint(_ hidden: Bool) -> Color { Color(nsColor: hidden ? .systemGreen : .systemOrange) }
    static func label(_ hidden: Bool) -> String {
        hidden ? "Hidden from screen sharing (best effort)" : "Visible in screen sharing — viewers can see the island"
    }
}

/// Bottom-rounded while attached to the top edge (the notch's silhouette), all round when detached.
struct IslandShape: Shape {
    let radius: CGFloat
    let bottomOnly: Bool
    func path(in r: CGRect) -> Path {
        bottomOnly
            ? UnevenRoundedRectangle(bottomLeadingRadius: radius, bottomTrailingRadius: radius, style: .continuous).path(in: r)
            : RoundedRectangle(cornerRadius: radius, style: .continuous).path(in: r)
    }
}

/// The grip: `mouseDown` → the controller (`performDrag(with:)`, or snap-back on a double-click; recon A §A5 point 7).
/// Sits behind the strip's buttons, so a press on nothing drags and a press on a button clicks.
struct DragGrip: NSViewRepresentable {
    @ObservedObject var ui: IslandUI
    func makeNSView(context: Context) -> GripView { let v = GripView(); v.onDown = { ui.onGrip?($0) }; return v }
    func updateNSView(_ v: GripView, context: Context) { v.onDown = { ui.onGrip?($0) } }

    final class GripView: NSView {
        var onDown: ((NSEvent) -> Void)?
        override func mouseDown(with event: NSEvent) { onDown?(event) }
        override func acceptsFirstMouse(for event: NSEvent?) -> Bool { true }
    }
}
