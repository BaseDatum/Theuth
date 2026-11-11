import Foundation
import TheuthKit

/// 192 P1: an answer split into what the card draws in order — markdown runs and picture fences. Pure.
public enum AnswerSegment: Equatable, Sendable {
    /// Markdown for SwiftStreamingMarkdown, every non-picture fence included.
    case markdown(String)
    /// A closed picture fence: drawn by the desktop app (`fence_snapshot`).
    case picture(PictureFence)
    /// A picture fence still streaming: a "drawing…" line, never its source; `title` is a json-render root title when one
    /// is already in the source.
    case drawing(lang: String, title: String?)
}

/// A picture fence's language (the canonical name `fence_snapshot` takes) and its body without the fence lines.
public struct PictureFence: Hashable, Sendable {
    public var lang: String
    public var source: String
    public init(lang: String, source: String) { self.lang = lang; self.source = source }

    /// The same fence as a plain code block (the fallback when no picture comes).
    public var codeBlock: String { "```\(lang)\n\(source)\n```" }

    /// Info-string word → `fence_snapshot` lang; `jsonrender` is an alias of `json-render`. Everything else is a code block.
    public static func lang(_ info: Substring) -> String? {
        switch info.lowercased() {
        case "json-render", "jsonrender": return "json-render"
        case "mermaid": return "mermaid"
        case "openui": return "openui"
        default: return nil
        }
    }
}

public enum AnswerSegments {
    /// Split `text` (CommonMark fences: up to 3 spaces of indent, 3+ backticks or tildes, closed by a run of the same
    /// character at least as long with nothing after it). A picture fence is a `.picture` only once closed; still open, it
    /// is `.drawing` while the reply streams and stays markdown (a code block to the end) once `finished`. Fences inside
    /// another fence are its text. Whitespace-only markdown runs are dropped.
    public static func split(_ text: String, finished: Bool) -> [AnswerSegment] {
        var out: [AnswerSegment] = []
        var run: [Substring] = []
        var open: (char: Character, count: Int, lang: String?, lines: [Substring], fenceLine: Substring)?
        func flush() {
            let s = run.joined(separator: "\n")
            if !s.trimmingCharacters(in: .whitespacesAndNewlines).isEmpty { out.append(.markdown(s)) }
            run = []
        }
        for line in text.split(separator: "\n", omittingEmptySubsequences: false) {
            if let o = open {
                if isClosing(line, char: o.char, count: o.count) {
                    if let lang = o.lang {
                        flush()
                        out.append(.picture(PictureFence(lang: lang, source: o.lines.joined(separator: "\n"))))
                    } else {
                        run.append(line)
                    }
                    open = nil
                } else if o.lang != nil {
                    open?.lines.append(line)
                } else {
                    run.append(line)
                }
                continue
            }
            if let f = opening(line) {
                open = (f.char, f.count, f.lang, [], line)
                if f.lang == nil { run.append(line) }
                continue
            }
            run.append(line)
        }
        if let o = open, let lang = o.lang {
            if finished {
                run.append(o.fenceLine)
                run.append(contentsOf: o.lines)
            } else {
                flush()
                out.append(.drawing(lang: lang, title: lang == "json-render" ? jsonTitle(o.lines.joined(separator: "\n")) : nil))
            }
        }
        flush()
        return out
    }

    /// An opening fence: its character, length and picture lang (nil for any other info string).
    static func opening(_ line: Substring) -> (char: Character, count: Int, lang: String?)? {
        let indent = line.prefix { $0 == " " }.count
        guard indent <= 3 else { return nil }
        let s = line.dropFirst(indent)
        guard let c = s.first, c == "`" || c == "~" else { return nil }
        let count = s.prefix { $0 == c }.count
        guard count >= 3 else { return nil }
        let info = s.dropFirst(count).trimmingCharacters(in: .whitespaces)
        if c == "`", info.contains("`") { return nil } // CommonMark: a backtick fence's info string has no backticks
        let word = info.split(whereSeparator: { $0 == " " || $0 == "{" }).first ?? ""
        return (c, count, PictureFence.lang(word))
    }

    static func isClosing(_ line: Substring, char: Character, count: Int) -> Bool {
        let indent = line.prefix { $0 == " " }.count
        guard indent <= 3 else { return false }
        let s = line.dropFirst(indent)
        let n = s.prefix { $0 == char }.count
        return n >= count && s.dropFirst(n).allSatisfy { $0 == " " || $0 == "\t" }
    }

    /// The first `"title": "…"` in the source (json-render's root title, usually near the top); 80 characters at most.
    public static func jsonTitle(_ source: String) -> String? {
        guard let r = source.range(of: #""title"\s*:\s*"([^"\\\n]{1,80})""#, options: .regularExpression) else { return nil }
        let m = source[r]
        guard let q = m.dropLast().lastIndex(of: "\"") else { return nil }
        let t = m[m.index(after: q)..<m.index(before: m.endIndex)]
        return t.isEmpty ? nil : String(t)
    }
}

/// 192 P2: what one `fence_snapshot` is asked for; the in-process cache key.
public struct FenceKey: Hashable, Sendable, CustomStringConvertible {
    public var lang: String
    public var source: String
    /// Points of the card's content width (CSS px to the renderer).
    public var width: Int
    /// `dark` / `light`.
    public var appearance: String
    /// The window's backing scale.
    public var scale: Double
    public init(lang: String, source: String, width: Int, appearance: String, scale: Double) {
        self.lang = lang; self.source = source; self.width = width; self.appearance = appearance; self.scale = scale
    }
    public init(_ fence: PictureFence, width: Int, dark: Bool, scale: Double) {
        self.init(lang: fence.lang, source: fence.source, width: width, appearance: dark ? "dark" : "light", scale: scale)
    }
    public var description: String { "\(lang), \(width) pt, \(appearance), @\(scale)x" }
}

/// A picture fence's state in the card.
public enum FencePictureState: Equatable, Sendable {
    case drawing
    /// The PNG and its CSS size.
    case image(png: Data, width: Double, height: Double)
    /// No picture: show the fence as a code block with "open theuth to see the diagram"; `reason` is logged.
    case fallback(reason: String)

    /// `fence_snapshot`'s answer → the state. `done` with a decodable PNG and a size draws it; every other outcome,
    /// a refusal (-32601: an engine without the method) or any other error falls back.
    public static func from(_ result: Result<FenceSnapshotReply, Error>) -> FencePictureState {
        switch result {
        case .failure(let e as RpcClient.RpcError) where e.code == -32601:
            return .fallback(reason: "no fence_snapshot on this engine")
        case .failure(let e):
            return .fallback(reason: "\(e)")
        case .success(let r):
            guard r.outcome == "done" else { return .fallback(reason: r.outcome + (r.error.map { ": \($0)" } ?? "")) }
            guard let b64 = r.png, let png = Data(base64Encoded: b64), !png.isEmpty,
                  let w = r.width, let h = r.height, w > 0, h > 0 else { return .fallback(reason: "done without a picture") }
            return .image(png: png, width: w, height: h)
        }
    }
}

/// 192 P2: the in-process picture cache and its request queue — at most `maxInFlight` draws at once, a key asked for
/// again never redraws. Pure; `FenceSnapshots` (the Companion) runs the RPCs it says to start.
public struct FenceCache: Equatable, Sendable {
    public static let maxInFlight = 2
    public enum Request: Equatable, Sendable {
        /// Already drawn (or fallen back): use it.
        case hit(FencePictureState)
        /// Start this draw now.
        case start
        /// Asked for already (in flight or queued): wait.
        case pending
    }

    public private(set) var states: [FenceKey: FencePictureState] = [:]
    public private(set) var inFlight: Set<FenceKey> = []
    public private(set) var queue: [FenceKey] = []

    public init() {}

    public mutating func request(_ key: FenceKey) -> Request {
        if let s = states[key], s != .drawing { return .hit(s) }
        if inFlight.contains(key) || queue.contains(key) { return .pending }
        states[key] = .drawing
        guard inFlight.count < Self.maxInFlight else { queue.append(key); return .pending }
        inFlight.insert(key)
        return .start
    }

    /// A draw came back: record it; answers the queued key to start next, if any.
    public mutating func finished(_ key: FenceKey, _ state: FencePictureState) -> FenceKey? {
        states[key] = state
        inFlight.remove(key)
        guard !queue.isEmpty, inFlight.count < Self.maxInFlight else { return nil }
        let next = queue.removeFirst()
        inFlight.insert(next)
        return next
    }
}
