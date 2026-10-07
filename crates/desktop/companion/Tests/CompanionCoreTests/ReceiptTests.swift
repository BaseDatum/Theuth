import Testing
import Foundation
import TheuthKit
@testable import CompanionCore

private let t0 = Date(timeIntervalSince1970: 1_791_448_800)
private func entry(_ id: Int64, agoS: Double, actor: String = Receipt.companionActor, summary: String = "answered question q") -> ActivityEntry {
    ActivityEntry(id: id, tsMs: UInt64((t0.timeIntervalSince1970 - agoS) * 1000), kind: "ask_answered", actor: actor,
                  subject: "ask:\(id)", summary: summary)
}

struct ReceiptModelTests {
    @Test func newestFirstAcrossAsksAndAnswers() {
        var m = ReceiptModel()
        m.add(Receipt(id: "ask:a", kind: .ask, at: t0.addingTimeInterval(-30), who: "theuth", text: "handed to w3", session: "desk"), now: t0)
        m.merge(activity: [entry(1, agoS: 300), entry(2, agoS: 5)], now: t0)
        #expect(m.entries.map(\.id) == ["activity:2", "ask:a", "activity:1"])
        #expect(m.entries[0].kind == .answer && m.entries[1].kind == .ask)
    }

    @Test func onlyCompanionAnswersBecomeReceipts() {
        var m = ReceiptModel()
        m.merge(activity: [entry(1, agoS: 1, actor: "user@desktop"), entry(2, agoS: 1, actor: "agent:w2"), entry(3, agoS: 1)], now: t0)
        #expect(m.entries.map(\.id) == ["activity:3"])
    }

    @Test func dedupeByActivityIdKeepsTheLatestText() {
        var m = ReceiptModel()
        m.merge(activity: [entry(7, agoS: 10)], now: t0)
        m.merge(activity: [entry(7, agoS: 10, summary: "denied approval ap-7")], now: t0)
        m.add(Receipt(id: "ask:x", kind: .ask, at: t0, who: "theuth", text: "…"), now: t0)
        m.update(id: "ask:x", text: "handed to w3 — it owns crates/rpc")
        #expect(m.count == 2)
        #expect(m.entries.first { $0.id == "activity:7" }?.text == "denied approval ap-7")
        #expect(m.entries.first { $0.id == "ask:x" }?.text == "handed to w3 — it owns crates/rpc")
    }

    @Test func twentyFourHourWindow() {
        var m = ReceiptModel()
        m.merge(activity: [entry(1, agoS: ReceiptModel.window - 1), entry(2, agoS: ReceiptModel.window + 1)], now: t0)
        #expect(m.entries.map(\.id) == ["activity:1"])
        m.prune(now: t0.addingTimeInterval(2))
        #expect(m.isEmpty, "a receipt expires as time passes, not only on arrival")
    }

    @Test func clearedStaysClearedThroughRefetches() {
        var m = ReceiptModel()
        m.merge(activity: [entry(1, agoS: 1), entry(2, agoS: 2)], now: t0)
        m.clear(id: "activity:1")
        #expect(m.entries.map(\.id) == ["activity:2"])
        m.merge(activity: [entry(1, agoS: 1), entry(2, agoS: 2)], now: t0)
        #expect(m.entries.map(\.id) == ["activity:2"], "the engine still lists it; the Done zone does not")
    }

    @Test func activityEntryDecodesTheWireShape() throws {
        let json = #"{"id":41,"ts_ms":1791448700000,"kind":"approval_resolved","actor":"user@companion","subject":"ap-9","summary":"denied approval ap-9","cost":null}"#
        let e = try TheuthJSON.decoder.decode(ActivityEntry.self, from: Data(json.utf8))
        #expect(e.id == 41 && e.actor == "user@companion" && e.cost == nil)
        let r = try #require(Receipt(activity: e))
        #expect(r.id == "activity:41" && r.who == "ap-9" && r.at == Date(timeIntervalSince1970: 1_791_448_700))
        let f = try TheuthJSON.encoder.encode(ActivityFilter(sinceMs: 5, limit: 100))
        #expect(String(decoding: f, as: UTF8.self) == #"{"limit":100,"since_ms":5}"#)
    }
}
