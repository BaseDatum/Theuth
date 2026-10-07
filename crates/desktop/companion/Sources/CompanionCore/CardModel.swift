import Foundation
import TheuthKit

/// What a keystroke commits (after the hold). The app maps each case to exactly one RPC.
public enum CardAction: Equatable, Sendable {
    case answerAsk(askId: String, answers: [Answer])
    case decide(decision: String, answer: String)
    case approve(approvalId: String, decision: Decision)
    case reviewApprove(task: String)
    /// Needs a note first; the app collects it and holds `.reviewSendBackNote`.
    case reviewSendBack(task: String)
    case reviewSendBackNote(task: String, note: String)
    case sendNextStep(session: String, text: String)
    case dismiss(id: String)
    case snooze(id: String)
    /// W3 R on an approval (R1-5: Decision is Allow|Deny): `resolve_approval(Deny)` then `steer(session, note)`.
    case denyWithNote(approvalId: String, session: String?, note: String)
    /// W3 Ask bar: `send_input(theuth's desk, text)` after the hold; no item, so no stale guard.
    case askTheuth(text: String)
    /// Not sendable yet: the app must collect the note and hold `.reviewSendBackNote` instead.
    public var needsNote: Bool {
        if case .reviewSendBack = self { return true }
        return false
    }
}

public struct CardOption: Equatable, Sendable {
    /// 1 · 2 · 3.
    public var key: Int
    public var label: String
    public var detail: String?
    public var recommended: Bool
    public var action: CardAction

    public init(key: Int, label: String, detail: String?, recommended: Bool, action: CardAction) {
        self.key = key; self.label = label; self.detail = detail; self.recommended = recommended; self.action = action
    }
}

public struct Chip: Equatable, Sendable {
    public var text: String
    /// `nil` = neutral.
    public var ok: Bool?
}

/// The focused card, in the order a person decides (130 §4.2): who · how long → question → why → ≤ 3 options.
public struct Card: Equatable, Sendable {
    public var item: AttentionItem
    public var who: String
    public var model: String?
    public var question: String
    public var why: String?
    public var options: [CardOption]
    public var chips: [Chip]
    /// Options beyond the first three (shown as "+N more in theuth").
    public var moreOptions: Int
    /// The item can be cleared with E.
    public var clearable: Bool

    public var recommended: CardOption? { options.first { $0.recommended } }
    public var sessionURL: URL? { item.session.map(DeepLink.session) }
    public var attentionURL: URL { DeepLink.attention(item.id) }

    /// `packet` for decisions/reviews (fetched with `packet_get`), `approval` for approvals (from the stream).
    public static func make(item: AttentionItem, packet: Packet? = nil, approval: ApprovalRequested? = nil,
                            agentModel: String? = nil) -> Card {
        var card = Card(item: item, who: item.agent ?? shortSession(item.session) ?? "theuth", model: agentModel,
                        question: item.title, why: item.detail, options: [], chips: [], moreOptions: 0, clearable: false)
        switch item.kind {
        case .ask:
            guard let ask = item.ask, ask.questions.count == 1, let q = ask.questions.first else {
                card.why = item.ask.map { "\($0.questions.count) questions — answer them in theuth" } ?? item.detail
                return card
            }
            card.question = q.question
            card.options = q.options.prefix(3).enumerated().map { i, c in
                CardOption(key: i + 1, label: c.label, detail: c.description, recommended: q.recommended == i,
                           action: .answerAsk(askId: ask.id, answers: [.choice(index: i)]))
            }
            card.moreOptions = max(0, q.options.count - 3)
        case .decision:
            guard let p = packet, let decision = item.decision else { return card }
            card.question = p.question ?? item.title
            card.why = p.summary.isEmpty ? item.detail : p.summary
            card.options = p.options.prefix(3).enumerated().map { i, o in
                CardOption(key: i + 1, label: o.label, detail: o.description, recommended: p.preferredOption == i,
                           action: .decide(decision: decision, answer: o.label))
            }
            card.moreOptions = max(0, p.options.count - 3)
        case .approval:
            guard let id = item.approval else { return card }
            // P13: a destructive call has no pre-pick. "Allow for this session" is not on the wire — left out.
            let destructive = approval?.risk == .destructive
            if let a = approval {
                let cmd = a.args["command"]?.string.map { " · \($0)" } ?? ""
                card.why = "\(a.tool) · \(a.risk.rawValue)\(cmd)"
            }
            card.options = [
                CardOption(key: 1, label: "Allow", detail: nil, recommended: !destructive,
                           action: .approve(approvalId: id, decision: .allow)),
                CardOption(key: 2, label: "Deny", detail: nil, recommended: false,
                           action: .approve(approvalId: id, decision: .deny)),
            ]
        case .review:
            guard let task = item.task else { return card }
            if let p = packet {
                card.why = p.summary.isEmpty ? item.detail : p.summary
                card.chips = p.checks.map { Chip(text: $0.text, ok: $0.done) }
            }
            // P13: failing checks remove the pre-pick.
            let failing = packet?.checks.contains { !$0.done } ?? false
            card.options = [
                CardOption(key: 1, label: "Approve", detail: nil, recommended: !failing, action: .reviewApprove(task: task)),
                CardOption(key: 2, label: "Send back…", detail: "with a note", recommended: false, action: .reviewSendBack(task: task)),
            ]
        case .finished:
            card.why = item.summary
            card.clearable = true
            if let s = item.session {
                card.options = item.nextSteps.prefix(2).enumerated().map { i, t in
                    CardOption(key: i + 1, label: t, detail: nil, recommended: i == 0, action: .sendNextStep(session: s, text: t))
                }
            }
        case .show:
            card.clearable = true
        default:
            break
        }
        return card
    }

    static func shortSession(_ s: String?) -> String? { s.map { String($0.prefix(8)) } }
}
