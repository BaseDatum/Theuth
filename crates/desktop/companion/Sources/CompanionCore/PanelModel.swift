import Foundation
import TheuthKit

/// Ordering, focus and the header sentence of the Attention panel (130 §4.2).
public struct PanelModel: Equatable, Sendable {
    public private(set) var items: [AttentionItem]
    public private(set) var focus: Int
    public var workingCount: Int

    public init(items: [AttentionItem], workingCount: Int = 0) {
        self.items = Self.order(items)
        self.focus = 0
        self.workingCount = workingCount
    }

    /// Needs-you oldest first, then finished oldest first; receipts (`show`, unknown kinds) last.
    public static func order(_ items: [AttentionItem]) -> [AttentionItem] {
        func group(_ i: AttentionItem) -> Int { i.kind.needsYou ? 0 : (i.kind == .finished ? 1 : 2) }
        return items.sorted { (group($0), $0.waitingSince, $0.id) < (group($1), $1.waitingSince, $1.id) }
    }

    public var focused: AttentionItem? { items.indices.contains(focus) ? items[focus] : nil }
    /// Every other item as a line, in panel order.
    public var others: [AttentionItem] { items.enumerated().filter { $0.offset != focus }.map(\.element) }
    public var needsYouCount: Int { items.filter { $0.kind.needsYou }.count }
    public var doneCount: Int { items.filter { $0.kind == .finished }.count }

    /// J/K · ↓/↑: clamped, never wraps.
    public mutating func move(_ delta: Int) {
        guard !items.isEmpty else { focus = 0; return }
        focus = min(items.count - 1, max(0, focus + delta))
    }

    public mutating func focus(id: String) {
        if let i = items.firstIndex(where: { $0.id == id }) { focus = i }
    }

    /// A fresh list keeps the focused item when it is still there, else clamps.
    public mutating func update(items new: [AttentionItem]) {
        let keep = focused?.id
        items = Self.order(new)
        if let keep, let i = items.firstIndex(where: { $0.id == keep }) { focus = i }
        else { focus = min(max(0, items.count - 1), focus) }
    }

    /// "3 need you · 5 working · 2 done"; "Nothing needs you" when nothing does and nothing works.
    public var headline: String {
        var parts: [String] = []
        let n = needsYouCount
        if n > 0 { parts.append(n == 1 ? "1 needs you" : "\(n) need you") }
        if workingCount > 0 { parts.append("\(workingCount) working") }
        if doneCount > 0 { parts.append("\(doneCount) done") }
        if n == 0 && workingCount == 0 { parts.insert("Nothing needs you", at: 0) }
        return parts.joined(separator: " · ")
    }

    /// "1 of 3"; empty when there is nothing to count.
    public var position: String { items.isEmpty ? "" : "\(focus + 1) of \(items.count)" }
}
