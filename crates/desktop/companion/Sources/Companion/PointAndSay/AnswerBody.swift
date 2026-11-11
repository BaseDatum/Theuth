import AppKit
import CompanionCore
import SwiftUI
import TheuthKit

/// 192 P1/P2: the expanded answer as segments — markdown runs through `AnswerMarkdown`, closed picture fences as the
/// desktop app's pictures (`FencePictureView`), a picture fence still streaming as one "drawing…" line.
struct AnswerBody: View {
    let text: String
    /// The reply finished: an unclosed picture fence is a code block, not a placeholder.
    let finished: Bool
    var onOpen: (() -> Void)?

    var body: some View {
        VStack(alignment: .leading, spacing: 8) {
            ForEach(Array(AnswerSegments.split(text, finished: finished).enumerated()), id: \.offset) { _, segment in
                switch segment {
                case .markdown(let t): AnswerMarkdown(text: t)
                case .picture(let fence): FencePictureView(fence: fence, onOpen: onOpen)
                case .drawing(let lang, let title): DrawingLine(lang: lang, title: title)
                }
            }
        }
    }
}

/// "drawing…" in the secondary colour, with a json-render root title when known.
private struct DrawingLine: View {
    let lang: String
    let title: String?
    var body: some View {
        Text(title.map { "drawing \($0)…" } ?? "drawing \(lang == "mermaid" ? "diagram" : "picture")…")
            .font(.callout).foregroundColor(.secondary).lineLimit(1)
    }
}

/// One closed picture fence: requested once per (lang, source, width, appearance, scale) from `FenceSnapshots`, drawn at
/// the card's content width with the PNG's aspect ratio; a click opens the reply in theuth (the card's Open action).
/// No picture (no renderer, a failure, an engine without `fence_snapshot`): the fence as a code block and a caption.
struct FencePictureView: View {
    let fence: PictureFence
    var onOpen: (() -> Void)?
    @ObservedObject private var snapshots = FenceSnapshots.shared
    @Environment(\.colorScheme) private var scheme
    @Environment(\.displayScale) private var scale

    private var key: FenceKey {
        FenceKey(fence, width: Int(AnswerCard.contentWidth), dark: scheme == .dark, scale: Double(scale))
    }

    var body: some View {
        let key = self.key
        Group {
            switch snapshots.state(key) {
            case .image(_, let w, let h):
                if let image = snapshots.image(key) {
                    Image(nsImage: image).resizable().interpolation(.high)
                        .aspectRatio(CGSize(width: w, height: h), contentMode: .fit)
                        .frame(width: AnswerCard.contentWidth)
                        .contentShape(Rectangle())
                        .onTapGesture { onOpen?() }
                        .accessibilityLabel("\(fence.lang) picture — opens in theuth")
                } else {
                    fallback
                }
            case .fallback:
                fallback
            case .drawing, nil:
                DrawingLine(lang: fence.lang, title: fence.lang == "json-render" ? AnswerSegments.jsonTitle(fence.source) : nil)
            }
        }
        .task(id: key) { snapshots.request(key) }
    }

    private var fallback: some View {
        VStack(alignment: .leading, spacing: 4) {
            AnswerMarkdown(text: fence.codeBlock)
            Text("open theuth to see the diagram").font(.caption).foregroundColor(.secondary)
        }
    }
}

/// 192 P2: the in-process picture cache and its `fence_snapshot` calls (`FenceCache` decides; at most two in flight).
/// `draw` is the engine's RPC (`Store.start`) or a probe's stub.
@MainActor
final class FenceSnapshots: ObservableObject {
    static let shared = FenceSnapshots()

    var draw: ((FenceKey) async throws -> FenceSnapshotReply)?
    @Published private(set) var cache = FenceCache()
    private var images: [FenceKey: NSImage] = [:]

    func state(_ key: FenceKey) -> FencePictureState? { cache.states[key] }
    func image(_ key: FenceKey) -> NSImage? { images[key] }

    func request(_ key: FenceKey) {
        switch cache.request(key) {
        case .hit: Log.write("fence_snapshot cache hit (\(key))")
        case .start: start(key)
        case .pending: break
        }
    }

    private func start(_ key: FenceKey) {
        let t0 = Date()
        Log.write("fence_snapshot(\(key), \(key.source.count) chars) requested")
        Task {
            var state: FencePictureState
            if let draw {
                do { state = .from(.success(try await draw(key))) } catch { state = .from(.failure(error)) }
            } else {
                state = .fallback(reason: "not connected")
            }
            if case .image(let png, _, _) = state {
                if let image = NSImage(data: png) { images[key] = image } else { state = .fallback(reason: "the PNG did not decode") }
            }
            let ms = Int(Date().timeIntervalSince(t0) * 1000)
            switch state {
            case .image(let png, _, _): Log.write("fence_snapshot(\(key.lang)): done in \(ms) ms, \(png.count) bytes")
            case .fallback(let why): Log.write("fence_snapshot(\(key.lang)): no picture in \(ms) ms — \(why)")
            case .drawing: break
            }
            if let next = cache.finished(key, state) { start(next) }
        }
    }
}

/// 192 P3: the turn's reasoning. Live: "Thinking" and the text's tail (≈ 6 lines, following it as it grows) in the
/// secondary colour and a smaller font, where the spinner was. Ended: one "Thought for N s" disclosure with the whole text.
struct ThinkingView: View {
    let thinking: Thinking
    /// The view changed height on its own (the disclosure): the card re-measures.
    var onResize: (() -> Void)?
    @State private var open = false

    static let font = Font.callout
    /// Six lines of `.callout`.
    static let tailHeight: CGFloat = {
        let f = NSFont.preferredFont(forTextStyle: .callout)
        return (NSLayoutManager().defaultLineHeight(for: f) * 6).rounded(.up)
    }()

    var body: some View {
        Group {
            if thinking.live {
                VStack(alignment: .leading, spacing: 2) {
                    Text("Thinking").font(.caption.weight(.medium)).foregroundColor(.secondary)
                    ScrollView(.vertical) {
                        Text(thinking.text).font(Self.font).foregroundColor(.secondary)
                            .frame(maxWidth: .infinity, alignment: .leading)
                    }
                    .scrollIndicators(.never)
                    .defaultScrollAnchor(.bottom, for: .sizeChanges)
                    .defaultScrollAnchor(.bottom)
                    .frame(maxHeight: Self.tailHeight)
                }
            } else if let summary = thinking.summary {
                DisclosureGroup(isExpanded: $open) {
                    Text(thinking.text).font(Self.font).foregroundColor(.secondary).textSelection(.enabled)
                        .frame(maxWidth: .infinity, alignment: .leading)
                } label: {
                    Text(summary).font(Self.font).foregroundColor(.secondary)
                }
            }
        }
        .onGeometryChange(for: CGFloat.self, of: { $0.size.height }, action: { _ in onResize?() })
    }
}
