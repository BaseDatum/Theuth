import AppKit
import Combine
import CompanionCore
import SwiftUI
import TheuthKit

/// The rail (D4): a never-ordered-out `NSPanel` at the edge of the main display, mouse-transparent except while the
/// pointer is over it, a 4 pt sliver after 8 s of quiet — or always (W6, Settings › Rail presence).
/// 149 W1 (D1): four separate 52 pt pills (`RailPillsView`); Settings › "Separate pills" off keeps 130's 26 pt strip.
@MainActor
final class RailController {
    static let width = CGFloat(RailCollapse.width)
    static let sliver = CGFloat(RailCollapse.sliverWidth)
    /// 149 W1: the pills column — 4 × 52 + 3 × 12.
    static let pillsWidth = CGFloat(RailPills.height)
    static let pillsHeight = CGFloat(4 * RailPills.height + 3 * RailPills.gap)

    let panel: NSPanel
    private let store: Store
    private let hover: HoverSync
    private let hosting: NSHostingView<AnyView>
    private var subs = Set<AnyCancellable>()
    private var quietTimer: Timer?
    private var mouseMonitor: Any?
    private(set) var collapsed = false
    private var hovering = false
    /// 149 W1: dot/pill frames in the hosting view's top-left space (`RailAnchorKey`).
    private var anchors: [String: CGRect] = [:]
    private var pills = RailPills(pills: [])
    /// 149 W1 probe: pinned by `probeCollapse` so refetches and hover cannot flip the phase under the screenshot.
    private var pinned = false
    var onMark: ((Mark) -> Void)?
    /// 149 W1 §3.1: a capsule was clicked (not a dot); the pill's screen frame anchors what opens.
    var onPill: ((RailPills.Kind, NSRect) -> Void)?
    /// 149 W1 probe: used instead of `Settings.railSide` when set.
    var sideOverride: String?

    init(store: Store, hover: HoverSync = W1Hover.shared) {
        self.store = store
        self.hover = hover
        panel = NSPanel(contentRect: NSRect(x: 0, y: 0, width: Self.width, height: 100),
                        styleMask: [.borderless, .nonactivatingPanel], backing: .buffered, defer: false)
        panel.level = .statusBar
        panel.collectionBehavior = [.canJoinAllSpaces, .fullScreenAuxiliary, .stationary]
        panel.sharingType = Flags.shareable ? .readOnly : .none
        panel.ignoresMouseEvents = true
        panel.isOpaque = false
        panel.backgroundColor = .clear
        panel.hasShadow = false
        panel.hidesOnDeactivate = false
        panel.isReleasedWhenClosed = false
        hosting = NSHostingView(rootView: AnyView(EmptyView()))
        panel.contentView = hosting
        panel.setAccessibilityLabel("Theuth rail")
        pills = RailPills.derive(rail: store.rail, levels: store.ask.dictation.levels, hovered: nil)
        render()

        store.$rail.receive(on: RunLoop.main).sink { [weak self] rail in MainActor.assumeIsolated { self?.railChanged(rail) } }.store(in: &subs)
        // 149 W1 (D8): the actions pill's mic shows the Ask bar's live level bars.
        store.$ask.map(\.dictation.levels).removeDuplicates().receive(on: RunLoop.main)
            .sink { [weak self] levels in MainActor.assumeIsolated { self?.levelsChanged(levels) } }.store(in: &subs)
        store.$connection.removeDuplicates().receive(on: RunLoop.main)
            .sink { [weak self] _ in MainActor.assumeIsolated { self?.render() } }.store(in: &subs)
        // `ignoresMouseEvents` is the hit test: a global mouse-moved monitor flips it while the pointer is over
        // the rail (no permission needed for mouse events). Hover over the sliver reveals the rail.
        mouseMonitor = NSEvent.addGlobalMonitorForEvents(matching: [.mouseMoved, .leftMouseDragged]) { [weak self] _ in
            Task { @MainActor in self?.mouseMoved() }
        }
        SettingsModel.shared.$value.map(\.railPresence).removeDuplicates().dropFirst().receive(on: RunLoop.main)
            .sink { [weak self] _ in MainActor.assumeIsolated { self?.presenceChanged() } }.store(in: &subs)
        // 149 W1: pills ↔ strip live.
        SettingsModel.shared.$value.map(\.railPills).removeDuplicates().dropFirst().receive(on: RunLoop.main)
            .sink { [weak self] _ in MainActor.assumeIsolated { self?.render(); self?.layout() } }.store(in: &subs)
        layout()
        scheduleCollapse(afterHover: false) // `alwaysSliver`: a sliver from the first frame
        panel.orderFrontRegardless()
    }

    private var presence: RailPresence { SettingsModel.shared.value.railPresence }
    private var usePills: Bool { SettingsModel.shared.value.railPills }
    private var side: String { sideOverride ?? Settings.railSide }

    /// The root view for the current mode; `AnyView` so one hosting view carries both.
    private func render() {
        if usePills {
            hosting.rootView = AnyView(RailPillsView(
                pills: pills, collapsed: collapsed, side: side, hover: hover, connectionOpen: store.connection == .open,
                onMark: { [weak self] in self?.onMark?($0) },
                onPill: { [weak self] kind, r in
                    guard let self else { return }
                    self.onPill?(kind, self.screenRect(r))
                },
                onAnchors: { [weak self] in self?.anchors = $0 }))
        } else {
            hosting.rootView = AnyView(RailView(store: store, collapsed: collapsed, side: side) { [weak self] m in self?.onMark?(m) })
        }
    }

    private func railChanged(_ rail: Rail) {
        pills = RailPills.derive(rail: rail, levels: store.ask.dictation.levels, hovered: nil)
        render()
        guard !pinned else { return layout() }
        if !rail.quiet, collapsed, RailCollapse.expandsOnActivity(presence) { setCollapsed(false) }
        scheduleCollapse(afterHover: false)
        layout()
    }

    private func levelsChanged(_ levels: [Float]) {
        pills = RailPills.derive(rail: store.rail, levels: levels, hovered: nil)
        render()
    }

    /// W6: `RailCollapse.collapseDelay` decides; a running quiet timer is left alone (W1: one 8 s window per
    /// quiet spell, not restarted by every refetch) unless the pointer just left.
    private func scheduleCollapse(afterHover: Bool) {
        guard !collapsed else { return }
        if quietTimer != nil, !afterHover { return }
        quietTimer?.invalidate(); quietTimer = nil
        guard let delay = RailCollapse.collapseDelay(presence, quiet: store.rail.quiet, hovering: hovering, afterHover: afterHover) else { return }
        if delay == 0 { return setCollapsed(true) }
        quietTimer = Timer.scheduledTimer(withTimeInterval: delay, repeats: false) { [weak self] _ in
            Task { @MainActor in self?.setCollapsed(true) }
        }
    }

    /// Settings › Rail changed: apply live.
    private func presenceChanged() {
        guard !pinned else { return }
        quietTimer?.invalidate(); quietTimer = nil
        if collapsed, presence == .expanded || (presence == .sliverWhenQuiet && !store.rail.quiet) { setCollapsed(false) }
        scheduleCollapse(afterHover: false)
        layout()
    }

    private func setCollapsed(_ c: Bool) {
        quietTimer?.invalidate()
        quietTimer = nil
        collapsed = c
        render()
        layout()
    }

    /// 149 W1 `--probe-rail`: force a phase and keep it there.
    func probeCollapse(_ c: Bool) {
        pinned = true
        setCollapsed(c)
    }

    private func mouseMoved() {
        guard !pinned else { return }
        let inside = panel.frame.insetBy(dx: -2, dy: -2).contains(NSEvent.mouseLocation)
        if inside != hovering {
            hovering = inside
            panel.ignoresMouseEvents = !inside
            if inside {
                quietTimer?.invalidate(); quietTimer = nil
                if collapsed { setCollapsed(false) }
            } else {
                scheduleCollapse(afterHover: true)
            }
        }
    }

    /// Right edge of the main display (the one with the menu bar), vertically centred; Settings move it left.
    func layout() {
        guard let screen = NSScreen.screens.first else { return }
        let size = hosting.fittingSize
        let w = collapsed ? Self.sliver : usePills ? Self.pillsWidth : Self.width
        let h = max(collapsed ? 120 : usePills ? Self.pillsHeight : 60, size.height)
        let f = screen.frame
        let x = side == "left" ? f.minX : f.maxX - w
        panel.setFrame(NSRect(x: x, y: f.midY - h / 2, width: w, height: h), display: true)
        hosting.frame = NSRect(origin: .zero, size: NSSize(width: w, height: h))
    }

    /// A top-left view-space rect (SwiftUI, `"rail"` space) → screen.
    private func screenRect(_ r: CGRect) -> NSRect {
        let f = panel.frame
        return NSRect(x: f.minX + r.minX, y: f.maxY - r.maxY, width: r.width, height: r.height)
    }

    /// Where a mark sits on screen, for anchoring the panel beside it: the dot's preference anchor in pills mode,
    /// the strip's fixed 16 pt rows otherwise.
    func frame(of mark: Mark) -> NSRect {
        if let r = anchors[mark.id] { return screenRect(r) }
        guard let i = store.rail.marks.firstIndex(where: { $0.id == mark.id }) else { return panel.frame }
        let f = panel.frame
        let y = f.maxY - 10 - 20 - CGFloat(i) * 16
        return NSRect(x: f.minX, y: y, width: f.width, height: 16)
    }

    /// 149 W1: a capsule's screen frame; the whole rail when unknown (strip mode, collapsed).
    func frame(ofPill kind: RailPills.Kind) -> NSRect {
        anchors["pill:\(kind)"].map(screenRect) ?? panel.frame
    }
}

// MARK: SwiftUI

struct RailView: View {
    @ObservedObject var store: Store
    var collapsed: Bool
    var side: String
    var onMark: (Mark) -> Void

    var body: some View {
        if collapsed {
            Capsule().fill(Color.tertiary.opacity(0.35)).frame(width: RailController.sliver, height: 120)
                .accessibilityLabel("Theuth rail, quiet")
        } else {
            VStack(spacing: 6) {
                Wordmark()
                if store.rail.needsYouCount > 0 {
                    Text("\(store.rail.needsYouCount)")
                        .font(.caption2.monospaced().weight(.semibold))
                        .foregroundColor(.accentColor)
                        .accessibilityLabel("\(store.rail.needsYouCount) need you")
                }
                ForEach(store.rail.needsYou) { mark in marker(mark) }
                Rectangle().fill(Color.primary.opacity(0.18)).frame(width: 12, height: 1).padding(.vertical, 2)
                ForEach(store.rail.agents) { mark in marker(mark) }
                if store.rail.overflow > 0 {
                    Text("+\(store.rail.overflow)").font(.caption2.monospaced()).foregroundColor(.secondary)
                }
                if store.connection != .open {
                    Circle().fill(Color.failed).frame(width: 4, height: 4).help("engine: \(store.connection)")
                }
            }
            .padding(.vertical, 10)
            .frame(width: RailController.width)
            .background(
                .regularMaterial,
                in: UnevenRoundedRectangle(topLeadingRadius: side == "left" ? 0 : 10, bottomLeadingRadius: side == "left" ? 0 : 10,
                                           bottomTrailingRadius: side == "left" ? 10 : 0, topTrailingRadius: side == "left" ? 10 : 0)
            )
        }
    }

    private func marker(_ mark: Mark) -> some View {
        MarkView(state: mark.state)
            .frame(width: 16, height: 16)
            .contentShape(Rectangle())
            .onTapGesture { onMark(mark) }
            .help(mark.detail)
            .accessibilityElement()
            .accessibilityLabel(mark.detail)
            .accessibilityAddTraits(.isButton)
    }
}

/// The wordmark glyph: the only accent chrome besides the count and the needs-you marks.
struct Wordmark: View {
    var body: some View {
        RoundedRectangle(cornerRadius: 3).fill(Color.accentColor).frame(width: 12, height: 12)
            .overlay(RoundedRectangle(cornerRadius: 1).fill(Color.white).padding(3))
            .accessibilityLabel("theuth")
    }
}

/// One mark (130 §4.1 table): colour and shape both carry the state.
struct MarkView: View {
    var state: MarkState
    @State private var beat = false
    @Environment(\.accessibilityReduceMotion) private var reduceMotion

    var body: some View {
        switch state {
        case .needsYou(let kind):
            // A filled ring with a notch at the kind's clock position: question 12 · approval 3 · review 6 · other 9.
            Circle().trim(from: 0.06, to: 0.94)
                .stroke(Color.accentColor, style: StrokeStyle(lineWidth: 3, lineCap: .butt))
                .rotationEffect(.degrees(clock(kind)))
                .frame(width: 10, height: 10)
        case .working:
            Circle().fill(Color.accentColor).frame(width: 10, height: 10)
                .opacity(beat ? 0.55 : 1)
                .onAppear {
                    guard !reduceMotion else { return }
                    withAnimation(.easeInOut(duration: 0.38).repeatForever(autoreverses: true)) { beat = true }
                }
        case .idle:
            Circle().strokeBorder(Color.accentColor, lineWidth: 1.5).frame(width: 10, height: 10)
        case .finished:
            Circle().fill(Color.done).frame(width: 10, height: 10)
                .overlay(Image(systemName: "checkmark").font(.system(size: 5, weight: .heavy)).foregroundColor(.white)) // native: glyph inside a 10 pt dot
        case .error:
            Circle().fill(Color.failed).frame(width: 10, height: 10)
                .overlay(Rectangle().fill(Color.white).frame(width: 5, height: 1.5))
        case .lapsed:
            Circle().strokeBorder(style: StrokeStyle(lineWidth: 1.5, dash: [2, 1.5])).foregroundColor(.tertiary)
                .frame(width: 10, height: 10)
        }
    }

    private func clock(_ k: AttentionKind) -> Double {
        switch k {
        case .ask, .decision: return -90 // 12
        case .approval: return 0        // 3
        case .review: return 90         // 6
        default: return 180             // 9
        }
    }
}
