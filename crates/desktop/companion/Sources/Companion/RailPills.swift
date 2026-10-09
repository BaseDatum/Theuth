import AppKit
import Combine
import CompanionCore
import SwiftUI
import TheuthKit

/// 149 W1: the one `HoverSync` the rail and the agents panel share (a hovered row grows its dot, §3.1).
enum W1Hover {
    @MainActor static let shared = HoverSync()
}

/// Dot and pill frames in the `"rail"` coordinate space: `mark.id` → dot, `"pill:<kind>"` → capsule.
struct RailAnchorKey: PreferenceKey {
    static let defaultValue: [String: CGRect] = [:]
    static func reduce(value: inout [String: CGRect], nextValue: () -> [String: CGRect]) {
        value.merge(nextValue(), uniquingKeysWith: { $1 })
    }
}

/// §3.1 / D1: the rail as four separate 52 pt capsules — inbox · agents · actions · more — 12 pt apart.
struct RailPillsView: View {
    var pills: RailPills
    var collapsed: Bool
    var side: String
    @ObservedObject var hover: HoverSync
    var connectionOpen: Bool
    var onMark: (Mark) -> Void
    /// View-space frame of the tapped capsule; the controller converts to screen.
    var onPill: (RailPills.Kind, CGRect) -> Void
    var onAnchors: ([String: CGRect]) -> Void

    @State private var anchors: [String: CGRect] = [:]
    @State private var hoveredId: String?
    @Environment(\.accessibilityReduceMotion) private var reduceMotion

    static let size = CGFloat(RailPills.height)
    static let gap = CGFloat(RailPills.gap)
    private static let dot: CGFloat = 10
    private static let dotGap: CGFloat = 6

    var body: some View {
        if collapsed {
            Capsule().fill(Color.sand.opacity(0.35)).frame(width: RailController.sliver, height: 120)
                .accessibilityLabel(pills.sliverLabel)
        } else {
            VStack(spacing: Self.gap) {
                ForEach(pills.pills) { pill in
                    capsule(pill.kind) { content(pill) }
                }
            }
            .frame(width: Self.size)
            .coordinateSpace(name: "rail")
            .onPreferenceChange(RailAnchorKey.self) { anchors = $0; onAnchors($0) }
        }
    }

    @ViewBuilder
    private func content(_ pill: RailPills.Pill) -> some View {
        switch pill.kind {
        case .inbox:
            VStack(spacing: 3) {
                Wordmark()
                Text("\(pill.count)").font(Type.mono).foregroundColor(pill.count > 0 ? .gold : .ink2)
                    .accessibilityLabel("\(pill.count) need you")
                if !connectionOpen {
                    Circle().fill(Color.carnelian).frame(width: 4, height: 4).help("engine unreachable")
                }
            }
        case .agents:
            let cols = pill.dots.count > 6 ? 2 : 1
            VStack(spacing: Self.dotGap) {
                LazyVGrid(columns: Array(repeating: GridItem(.fixed(Self.dot), spacing: Self.dotGap), count: cols), spacing: Self.dotGap) {
                    ForEach(pill.dots) { dot($0) }
                }
                .frame(width: CGFloat(cols) * Self.dot + CGFloat(cols - 1) * Self.dotGap)
                if pill.overflow > 0 {
                    Text("+\(pill.overflow)").font(Type.mono).foregroundColor(.ink2)
                }
            }
        case .actions:
            VStack(spacing: 8) {
                Image(systemName: "text.cursor").font(.system(size: 14)).foregroundColor(.ink2).accessibilityLabel("Ask")
                if pill.levels.isEmpty {
                    Image(systemName: "mic").font(.system(size: 14)).foregroundColor(.ink2).accessibilityLabel("Talk")
                } else {
                    LevelBars(levels: pill.levels, height: 14)
                }
                Image(systemName: "scope").font(.system(size: 14)).foregroundColor(.ink2).accessibilityLabel("Point")
            }
        case .more:
            Text("···").font(Type.body).foregroundColor(.ink2).accessibilityLabel("More")
        }
    }

    private func capsule<C: View>(_ kind: RailPills.Kind, @ViewBuilder _ content: () -> C) -> some View {
        content()
            .padding(.vertical, 10)
            .frame(width: Self.size)
            .frame(minHeight: Self.size)
            .background(Capsule().fill(Color.surface.opacity(0.88)))
            .overlay(Capsule().stroke(Color.rule, lineWidth: 0.5))
            .contentShape(Capsule())
            .onTapGesture { onPill(kind, anchors["pill:\(kind)"] ?? .zero) }
            .background(anchor("pill:\(kind)"))
    }

    /// One agent dot: grows on hover here or when the agents panel hovers its row (`hover.session`).
    private func dot(_ mark: Mark) -> some View {
        let hovered = hoveredId == mark.id || (mark.session != nil && hover.session == mark.session)
        return Group {
            if mark.state == .working, !reduceMotion { WorkingArc() } else { MarkView(state: mark.state) }
        }
        .frame(width: Self.dot, height: Self.dot)
        .scaleEffect(hovered ? 1.4 : 1)
        .animation(.spring(duration: 0.12), value: hovered)
        .contentShape(Rectangle())
        .onHover { inside in
            hoveredId = inside ? mark.id : nil
            hover.session = inside ? mark.session : nil
        }
        .onTapGesture { onMark(mark) }
        .help(mark.detail)
        .accessibilityElement()
        .accessibilityLabel(mark.detail)
        .accessibilityAddTraits(.isButton)
        .background(anchor(mark.id))
    }

    private func anchor(_ key: String) -> some View {
        GeometryReader { g in
            Color.clear.preference(key: RailAnchorKey.self, value: [key: g.frame(in: .named("rail"))])
        }
    }
}

/// `.working` in the pills: a 0.38 s rotating faience arc (the mock's `.spin`), not MarkView's opacity beat.
struct WorkingArc: View {
    @State private var spin = false

    var body: some View {
        Circle().trim(from: 0, to: 0.7)
            .stroke(Color.faience, lineWidth: 2)
            .rotationEffect(.degrees(spin ? 360 : 0))
            .onAppear { withAnimation(.linear(duration: 0.38).repeatForever(autoreverses: false)) { spin = true } }
    }
}

// MARK: --probe-rail N (§10)

/// Seeded fixture (3 need you · 7 agents · six 0.6 level bars), three phases — pills right · sliver · pills left — then quit.
/// Never key, never acts: `AskFlags.automated` is true, `ignoresMouseEvents` stays true.
enum RailProbe {
    @MainActor private static var subs = Set<AnyCancellable>()

    @MainActor
    static func run(seconds: TimeInterval, store: Store, rail: RailController) {
        W1Log.write("probe-rail start seconds=\(seconds) shareable=\(Flags.shareable) pills=\(SettingsModel.shared.value.railPills)")
        seed(store)
        store.onRefetched = { seed(store) }
        store.$connection.removeDuplicates().filter { $0 != .open }.receive(on: RunLoop.main)
            .sink { _ in MainActor.assumeIsolated { seed(store) } }.store(in: &subs)

        func log(_ phase: String) {
            W1Log.write("probe-rail phase=\(phase) frame=\(rail.panel.frame) visible=\(rail.panel.isVisible) key=\(rail.panel.isKeyWindow) ignoresMouse=\(rail.panel.ignoresMouseEvents) active=\(NSApp.isActive) needsYou=\(store.rail.needsYouCount) marks=\(store.rail.marks.count) levels=\(store.ask.dictation.levels.count) hover=\(W1Hover.shared.session ?? "nil")")
        }
        DispatchQueue.main.asyncAfter(deadline: .now() + 1.5) { log("pills") }
        DispatchQueue.main.asyncAfter(deadline: .now() + seconds / 3) {
            rail.probeCollapse(true)
            log("sliver")
        }
        DispatchQueue.main.asyncAfter(deadline: .now() + 2 * seconds / 3) {
            rail.sideOverride = "left"
            rail.probeCollapse(false)
            log("pills-left")
        }
        DispatchQueue.main.asyncAfter(deadline: .now() + seconds) {
            // Ground truth from the window server (top-left origin), as `AppDelegate.runProbe` reads it.
            let wins = (CGWindowListCopyWindowInfo([.optionOnScreenOnly], kCGNullWindowID) as? [[String: Any]] ?? [])
                .filter { ($0[kCGWindowOwnerPID as String] as? Int32) == getpid() }
                .compactMap { $0[kCGWindowBounds as String] as? [String: Any] }
                .map { "x=\($0["X"] ?? 0) y=\($0["Y"] ?? 0) w=\($0["Width"] ?? 0) h=\($0["Height"] ?? 0)" }
            W1Log.write("probe-rail windows (CGWindowList): \(wins)")
            W1Log.write("probe-rail done after \(seconds) s — quitting (rail=\(rail.panel.frame) active=\(NSApp.isActive) key=\(NSApp.keyWindow.map { "\($0.className)" } ?? "none"))")
            NSApp.terminate(nil)
        }
    }

    @MainActor
    private static func seed(_ store: Store) {
        let nowMs = UInt64(Date().timeIntervalSince1970 * 1000)
        let items = [
            AttentionItem(id: "ask:1", kind: .ask, title: "Which spacing?", atMs: nowMs - 120_000, agent: "w1", session: "s1"),
            AttentionItem(id: "approval:2", kind: .approval, title: "Run migrate", atMs: nowMs - 60_000, agent: "w2", session: "s2"),
            AttentionItem(id: "review:3", kind: .review, title: "PR 149", atMs: nowMs - 30_000, agent: "w3", session: "s3"),
        ]
        func info(_ i: Int, _ state: AgentState) -> AgentInfo {
            AgentInfo(id: "s\(i)", name: "agent \(i)", agent: "w\(i)", model: "sonnet", state: state,
                      startedAt: Date().addingTimeInterval(-300),
                      activity: SessionActivity(state: state, turnStartedAt: Date().addingTimeInterval(-40), live: true))
        }
        let activity = [info(1, .running), info(2, .running), info(3, .running), info(4, .awaiting(from: "user")),
                        info(5, .idle), info(6, .error)]
        let agents = [AgentSummary(file: AgentFile(name: "scribe", title: "Scribe", model: "haiku", paused: true), status: .paused)]
        let sessions = activity.map { SessionSummary(id: $0.id, title: $0.name, agent: $0.agent, running: true) }
        store.w1Seed(items: items, agents: agents, activity: activity, sessions: sessions, mesh: nil, status: nil)
        _ = store.ask.dictation.handle(.start)
        _ = store.ask.dictation.handle(.started(id: "probe"))
        _ = store.ask.dictation.handle(.level(id: "probe", levels: [0.6, 0.6, 0.6, 0.6, 0.6, 0.6]))
    }
}
