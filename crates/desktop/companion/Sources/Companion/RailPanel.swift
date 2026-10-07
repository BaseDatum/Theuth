import AppKit
import Combine
import CompanionCore
import SwiftUI
import TheuthKit

/// The rail (D4): a never-ordered-out `NSPanel` at the edge of the main display, 26 pt wide, mouse-transparent
/// except while the pointer is over it, a 4 pt sliver after 8 s of quiet.
@MainActor
final class RailController {
    static let width: CGFloat = 26
    static let sliver: CGFloat = 4
    static let quietDelay: TimeInterval = 8

    let panel: NSPanel
    private let store: Store
    private let hosting: NSHostingView<RailView>
    private var subs = Set<AnyCancellable>()
    private var quietTimer: Timer?
    private var mouseMonitor: Any?
    private(set) var collapsed = false
    private var hovering = false
    var onMark: ((Mark) -> Void)?

    init(store: Store) {
        self.store = store
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
        hosting = NSHostingView(rootView: RailView(store: store, collapsed: false, side: Settings.railSide, onMark: { _ in }))
        hosting.rootView = RailView(store: store, collapsed: false, side: Settings.railSide) { [weak self] m in self?.onMark?(m) }
        panel.contentView = hosting
        panel.setAccessibilityLabel("Theuth rail")

        store.$rail.receive(on: RunLoop.main).sink { [weak self] rail in MainActor.assumeIsolated { self?.railChanged(rail) } }.store(in: &subs)
        // `ignoresMouseEvents` is the hit test: a global mouse-moved monitor flips it while the pointer is over
        // the rail (no permission needed for mouse events). Hover over the sliver reveals the rail.
        mouseMonitor = NSEvent.addGlobalMonitorForEvents(matching: [.mouseMoved, .leftMouseDragged]) { [weak self] _ in
            Task { @MainActor in self?.mouseMoved() }
        }
        layout()
        panel.orderFrontRegardless()
    }

    private func railChanged(_ rail: Rail) {
        if rail.quiet {
            if quietTimer == nil && !collapsed {
                quietTimer = Timer.scheduledTimer(withTimeInterval: Self.quietDelay, repeats: false) { [weak self] _ in
                    Task { @MainActor in self?.setCollapsed(true) }
                }
            }
        } else {
            quietTimer?.invalidate()
            quietTimer = nil
            if collapsed { setCollapsed(false) }
        }
        layout()
    }

    private func setCollapsed(_ c: Bool) {
        quietTimer?.invalidate()
        quietTimer = nil
        collapsed = c
        hosting.rootView = RailView(store: store, collapsed: c, side: Settings.railSide) { [weak self] m in self?.onMark?(m) }
        layout()
    }

    private func mouseMoved() {
        let inside = panel.frame.insetBy(dx: -2, dy: -2).contains(NSEvent.mouseLocation)
        if inside != hovering {
            hovering = inside
            panel.ignoresMouseEvents = !inside
            if inside && collapsed { setCollapsed(false) }
            if !inside && store.rail.quiet && !collapsed { railChanged(store.rail) }
        }
    }

    /// Right edge of the main display (the one with the menu bar), vertically centred; Settings move it left.
    func layout() {
        guard let screen = NSScreen.screens.first else { return }
        let size = hosting.fittingSize
        let w = collapsed ? Self.sliver : Self.width
        let h = max(collapsed ? 120 : 60, size.height)
        let f = screen.frame
        let x = Settings.railSide == "left" ? f.minX : f.maxX - w
        panel.setFrame(NSRect(x: x, y: f.midY - h / 2, width: w, height: h), display: true)
        hosting.frame = NSRect(origin: .zero, size: NSSize(width: w, height: h))
    }

    /// Where a mark sits on screen, for anchoring the panel beside it.
    func frame(of mark: Mark) -> NSRect {
        guard let i = store.rail.marks.firstIndex(where: { $0.id == mark.id }) else { return panel.frame }
        let f = panel.frame
        let y = f.maxY - 10 - 20 - CGFloat(i) * 16
        return NSRect(x: f.minX, y: y, width: f.width, height: 16)
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
            Capsule().fill(Color.sand.opacity(0.35)).frame(width: RailController.sliver, height: 120)
                .accessibilityLabel("Theuth rail, quiet")
        } else {
            VStack(spacing: 6) {
                Wordmark()
                if store.rail.needsYouCount > 0 {
                    Text("\(store.rail.needsYouCount)")
                        .font(.system(size: 9, weight: .semibold, design: .monospaced))
                        .foregroundColor(.gold)
                        .accessibilityLabel("\(store.rail.needsYouCount) need you")
                }
                ForEach(store.rail.needsYou) { mark in marker(mark) }
                Rectangle().fill(Color.ink.opacity(0.18)).frame(width: 12, height: 1).padding(.vertical, 2)
                ForEach(store.rail.agents) { mark in marker(mark) }
                if store.rail.overflow > 0 {
                    Text("+\(store.rail.overflow)").font(.system(size: 9, design: .monospaced)).foregroundColor(.ink2)
                }
                if store.connection != .open {
                    Circle().fill(Color.carnelian).frame(width: 4, height: 4).help("engine: \(store.connection)")
                }
            }
            .padding(.vertical, 10)
            .frame(width: RailController.width)
            .background(
                UnevenRoundedRectangle(topLeadingRadius: side == "left" ? 0 : 10, bottomLeadingRadius: side == "left" ? 0 : 10,
                                       bottomTrailingRadius: side == "left" ? 10 : 0, topTrailingRadius: side == "left" ? 10 : 0)
                    .fill(Color.surface.opacity(0.82))
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

/// The wordmark glyph: the only gold chrome besides the count and the needs-you marks.
struct Wordmark: View {
    var body: some View {
        RoundedRectangle(cornerRadius: 3).fill(Color.gold).frame(width: 12, height: 12)
            .overlay(RoundedRectangle(cornerRadius: 1).fill(Color.surface).padding(3))
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
                .stroke(Color.gold, style: StrokeStyle(lineWidth: 3, lineCap: .butt))
                .rotationEffect(.degrees(clock(kind)))
                .frame(width: 10, height: 10)
                .shadow(color: Color.gold.opacity(0.35), radius: 1.5)
        case .working:
            Circle().fill(Color.faience).frame(width: 10, height: 10)
                .opacity(beat ? 0.55 : 1)
                .onAppear {
                    guard !reduceMotion else { return }
                    withAnimation(.easeInOut(duration: 0.38).repeatForever(autoreverses: true)) { beat = true }
                }
        case .idle:
            Circle().strokeBorder(Color.faience, lineWidth: 1.5).frame(width: 10, height: 10)
        case .finished:
            Circle().fill(Color.malachite).frame(width: 10, height: 10)
                .overlay(Image(systemName: "checkmark").font(.system(size: 5, weight: .heavy)).foregroundColor(.surface))
        case .error:
            Circle().fill(Color.carnelian).frame(width: 10, height: 10)
                .overlay(Rectangle().fill(Color.surface).frame(width: 5, height: 1.5))
        case .lapsed:
            Circle().strokeBorder(style: StrokeStyle(lineWidth: 1.5, dash: [2, 1.5])).foregroundColor(.sand)
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
