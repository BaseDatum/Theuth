import Testing
import Foundation
@testable import TheuthKit

/// The contract test (130 §5.6): every fixture under `Fixtures/` decodes with the hand-written models.
struct FixtureTests {
    private func fixture(_ name: String) throws -> Data {
        let url = try #require(Bundle.module.url(forResource: name, withExtension: "json", subdirectory: "Fixtures"))
        return try Data(contentsOf: url)
    }

    private func decode<T: Decodable>(_ type: T.Type, _ name: String) throws -> T {
        try TheuthJSON.decoder.decode(T.self, from: try fixture(name))
    }

    @Test func attentionListEveryKind() throws {
        let items = try decode([AttentionItem].self, "attention_list")
        #expect(items.map(\.kind) == [.approval, .decision, .ask, .review, .hire, .show])
        #expect(items[0].approval == "3f6c1a2e-9b1d-4e0a-8c4b-5d2a7f1e9c01")
        #expect(items[1].packet == "decision:dec-2")
        #expect(items[1].task == "task-7")
        let ask = try #require(items[2].ask)
        #expect(ask.questions[0].options.count == 4)
        #expect(ask.questions[0].recommended == 1)
        #expect(ask.questions[0].options[1].description == "safer; ~20 min later")
        #expect(items[2].waitingSince == Date(timeIntervalSince1970: 1_791_448_700))
        #expect(items[0].kind.needsYou)
        #expect(!(items[5].kind.needsYou))
        // Pre-W0 engine: the optional fields default.
        #expect(items[0].waitingSinceMs == nil)
        #expect(items[0].nextSteps == [])
    }

    @Test func finishedItemCarriesW0Fields() throws {
        let item = try decode(AttentionItem.self, "attention_item_finished")
        #expect(item.kind == .finished)
        #expect(item.turn == 12)
        #expect(item.nextSteps.count == 2)
        #expect(item.summary?.prefix(13) == "Signed tokens")
        #expect(!(item.kind.needsYou))
    }

    @Test func unknownKindDecodesToOther() throws {
        let item = try decode(AttentionItem.self, "attention_item_unknown_kind")
        #expect(item.kind == .other("weather"))
        #expect(item.kind.rawValue == "weather")
        #expect(!(item.kind.needsYou))
    }

    @Test func agentsList() throws {
        let agents = try decode([AgentSummary].self, "agents_list")
        #expect(agents.map(\.status) == [.working, .paused, .invalid])
        #expect(agents[0].file.model == "claude-fable")
        #expect(agents[0].desk == "0192c5e0-7a1b-7c2d-9e3f-4a5b6c7d8e02")
        #expect(agents[0].spentMonth == 12.5)
        #expect(agents[1].file.paused)
        #expect(agents[2].error != nil)
    }

    @Test func listActivity() throws {
        let rows = try decode([AgentInfo].self, "list_activity")
        #expect(rows[0].state == .running)
        #expect(rows[0].activity.live)
        #expect(rows[0].activity.children == 2)
        // Fractional and whole-second RFC 3339 both parse.
        #expect(rows[0].activity.turnStartedAt != nil)
        #expect(rows[0].finishedAt == Date(timeIntervalSince1970: 1_791_367_920))
        #expect(rows[1].state == .approval(tool: "shell"))
        #expect(rows[1].activity.waitingSinceMs == 1_791_448_800_000)
        // No `activity` on the wire (pre-62 engine): defaults from `state`.
        #expect(rows[2].activity.state == .error)
        #expect(!(rows[2].activity.live))
        #expect(rows[2].agent == "codex")
    }

    @Test func packets() throws {
        let decision = try decode(Packet.self, "packet_decision")
        #expect(decision.kind == .decision)
        #expect(decision.options.count == 4)
        #expect(decision.recommended == 0)
        #expect(decision.preferredOption == 2, "the forwarded recommendation wins")
        #expect(decision.pointers[0].ref == "src/pricing.tsx#L10-L24")
        let review = try decode(Packet.self, "packet_review")
        #expect(review.kind == .review)
        #expect(review.preferredOption == nil)
        #expect(review.checks.map(\.done) == [true, false])
        #expect(review.to == nil)
    }

    @Test func events() throws {
        guard case .approvalRequested(let a) = try WireEvent.decode(fixture("approval_requested")) else { Issue.record("wrong case"); return }
        #expect(a.risk == .mutating)
        #expect(a.args["command"]?.string == "git push origin v2.10.1")

        guard case .approvalResolved(let r) = try WireEvent.decode(fixture("approval_resolved")) else { Issue.record("wrong case"); return }
        #expect(r.decision == .deny)
        #expect(r.timedOut)

        guard case .askRequested(let q) = try WireEvent.decode(fixture("ask_requested")) else { Issue.record("wrong case"); return }
        #expect(q.questions.count == 2)
        #expect(q.questions[1].options == [], "`#[serde(default)]` options")

        guard case .askResolved(let ar) = try WireEvent.decode(fixture("ask_resolved")) else { Issue.record("wrong case"); return }
        #expect(!(ar.dismissed))

        guard case .turnFinished(let t) = try WireEvent.decode(fixture("turn_finished")) else { Issue.record("wrong case"); return }
        #expect(t.stopReason == "end_turn")
        #expect(t.context == 1500)

        guard case .attentionChanged(let c) = try WireEvent.decode(fixture("attention_changed")) else { Issue.record("wrong case"); return }
        #expect(c.count == 3)
        #expect(c.idsRemoved == ["approval:b2"])

        guard case .lagged(let l) = try WireEvent.decode(fixture("lagged")) else { Issue.record("wrong case"); return }
        #expect(l.count == 17)

        guard case .pendingPrompts(let p) = try WireEvent.decode(fixture("pending_prompts")) else { Issue.record("wrong case"); return }
        #expect(p.approvals.first?.risk == .destructive)
        #expect(p.secrets.count == 1)

        #expect(try WireEvent.decode(Data(#"{"type":"TextDelta","session":"s","text":"x"}"#.utf8)) == .other("TextDelta"))
    }

    @Test func answerAndDecisionEncodeLikeSerde() throws {
        let enc = TheuthJSON.encoder
        #expect(String(decoding: try enc.encode(Answer.choice(index: 1)), as: UTF8.self) == #"{"index":1,"kind":"choice"}"#)
        #expect(String(decoding: try enc.encode(Answer.text("hi")), as: UTF8.self) == #"{"kind":"text","text":"hi"}"#)
        #expect(String(decoding: try enc.encode(Answer.skipped), as: UTF8.self) == #"{"kind":"skipped"}"#)
        #expect(String(decoding: try enc.encode([Decision.allow]), as: UTF8.self) == #"["allow"]"#)
        // Positional params with a trailing `null` (answers: None).
        let params = [AnyEncodable("id-1"), AnyEncodable([Answer]?.none)]
        #expect(String(decoding: try enc.encode(params), as: UTF8.self) == #"["id-1",null]"#)
    }

    @Test func discovery() throws {
        let dir = FileManager.default.temporaryDirectory.appendingPathComponent("theuth-kit-\(UUID().uuidString)")
        try FileManager.default.createDirectory(at: dir, withIntermediateDirectories: true)
        defer { try? FileManager.default.removeItem(at: dir) }
        #expect(throws: (any Error).self) { try EngineLocation.discover(dataDir: dir) }
        try "abc123\n".write(to: dir.appendingPathComponent("token"), atomically: true, encoding: .utf8)
        #expect(try EngineLocation.discover(dataDir: dir) == EngineLocation(url: URL(string: "ws://127.0.0.1:7777/")!, token: "abc123"))
        try #"{"pid":1,"listen":"127.0.0.1:49431"}"#.write(to: dir.appendingPathComponent("engine.json"), atomically: true, encoding: .utf8)
        #expect(try EngineLocation.discover(dataDir: dir).url == URL(string: "ws://127.0.0.1:49431/")!)
        #expect(EngineLocation.dataDir(env: ["THEUTH_DATA_DIR": "/tmp/x"]).path == "/tmp/x")
    }
}
