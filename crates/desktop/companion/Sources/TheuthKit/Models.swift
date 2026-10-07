import Foundation

// Hand-written mirrors of the Rust wire shapes the companion uses (design 130 §5.6). Only the fields
// the companion reads are declared; unknown keys are ignored by `Codable`. The fixtures under
// `Tests/TheuthKitTests/Fixtures` are the contract test.

// MARK: roster::attention (crates/roster/src/attention.rs:35-101)

/// `AttentionKind`, snake_case; `finished` is the W0 E3 addition; anything newer decodes to `.other`.
public enum AttentionKind: Codable, Equatable, Hashable, Sendable {
    case approval, decision, hire, blocked, review, budget, node, handoff, secret, ask, show, resume
    case finished
    case other(String)

    static let known: [String: AttentionKind] = [
        "approval": .approval, "decision": .decision, "hire": .hire, "blocked": .blocked,
        "review": .review, "budget": .budget, "node": .node, "handoff": .handoff,
        "secret": .secret, "ask": .ask, "show": .show, "resume": .resume, "finished": .finished,
    ]

    public init(from decoder: Decoder) throws {
        let s = try decoder.singleValueContainer().decode(String.self)
        self = Self.known[s] ?? .other(s)
    }

    public func encode(to encoder: Encoder) throws {
        var c = encoder.singleValueContainer()
        try c.encode(rawValue)
    }

    public var rawValue: String {
        if case .other(let s) = self { return s }
        return Self.known.first { $0.value == self }!.key
    }

    /// Needs a human action (gold). `show` is a receipt; `finished` is the second zone.
    public var needsYou: Bool {
        switch self {
        case .show, .finished, .other: return false
        default: return true
        }
    }
}

public struct AttentionItem: Codable, Equatable, Sendable, Identifiable {
    public var id: String
    public var kind: AttentionKind
    public var title: String
    public var detail: String?
    public var atMs: UInt64
    public var task: String?
    public var agent: String?
    public var session: String?
    public var approval: String?
    public var decision: String?
    public var packet: String?
    public var ask: AskRequested?
    public var turn: UInt32?
    // W0 (optional on today's engine):
    public var waitingSinceMs: UInt64?
    public var summary: String?
    public var nextSteps: [String]

    public init(id: String, kind: AttentionKind, title: String, detail: String? = nil, atMs: UInt64,
                task: String? = nil, agent: String? = nil, session: String? = nil, approval: String? = nil,
                decision: String? = nil, packet: String? = nil, ask: AskRequested? = nil, turn: UInt32? = nil,
                waitingSinceMs: UInt64? = nil, summary: String? = nil, nextSteps: [String] = []) {
        self.id = id; self.kind = kind; self.title = title; self.detail = detail; self.atMs = atMs
        self.task = task; self.agent = agent; self.session = session; self.approval = approval
        self.decision = decision; self.packet = packet; self.ask = ask; self.turn = turn
        self.waitingSinceMs = waitingSinceMs; self.summary = summary; self.nextSteps = nextSteps
    }

    public init(from decoder: Decoder) throws {
        let c = try decoder.container(keyedBy: CodingKeys.self)
        id = try c.decode(String.self, forKey: .id)
        kind = try c.decode(AttentionKind.self, forKey: .kind)
        title = try c.decode(String.self, forKey: .title)
        detail = try c.decodeIfPresent(String.self, forKey: .detail)
        atMs = try c.decode(UInt64.self, forKey: .atMs)
        task = try c.decodeIfPresent(String.self, forKey: .task)
        agent = try c.decodeIfPresent(String.self, forKey: .agent)
        session = try c.decodeIfPresent(String.self, forKey: .session)
        approval = try c.decodeIfPresent(String.self, forKey: .approval)
        decision = try c.decodeIfPresent(String.self, forKey: .decision)
        packet = try c.decodeIfPresent(String.self, forKey: .packet)
        ask = try c.decodeIfPresent(AskRequested.self, forKey: .ask)
        turn = try c.decodeIfPresent(UInt32.self, forKey: .turn)
        waitingSinceMs = try c.decodeIfPresent(UInt64.self, forKey: .waitingSinceMs)
        summary = try c.decodeIfPresent(String.self, forKey: .summary)
        nextSteps = try c.decodeIfPresent([String].self, forKey: .nextSteps) ?? []
    }

    /// When the wait began: E2's `waiting_since_ms`, else `at_ms`.
    public var waitingSince: Date { Date(timeIntervalSince1970: Double(waitingSinceMs ?? atMs) / 1000) }
}

/// W0 E1 — optional on today's engine.
public struct AttentionChanged: Codable, Equatable, Sendable {
    public var count: UInt32
    public var idsAdded: [String]
    public var idsRemoved: [String]
    public var atMs: UInt64
}

// MARK: roster::types (crates/roster/src/types.rs:213-284)

public enum AgentStatus: String, Codable, Sendable {
    case idle, working, blocked, paused, proposed, unreachable, invalid
}

public struct AgentFile: Codable, Equatable, Sendable {
    public var name: String
    public var title: String
    public var model: String?
    public var paused: Bool
    public init(name: String, title: String, model: String? = nil, paused: Bool = false) {
        self.name = name; self.title = title; self.model = model; self.paused = paused
    }
}

public struct AgentSummary: Codable, Equatable, Sendable {
    public var file: AgentFile
    public var status: AgentStatus
    public var desk: String?
    public var openTasks: UInt32
    public var spentMonth: Double
    public var error: String?
    public init(file: AgentFile, status: AgentStatus, desk: String? = nil, openTasks: UInt32 = 0,
                spentMonth: Double = 0, error: String? = nil) {
        self.file = file; self.status = status; self.desk = desk; self.openTasks = openTasks
        self.spentMonth = spentMonth; self.error = error
    }
}

// MARK: agent (crates/agent/src/lib.rs:887-944, 1056-1075)

/// `#[serde(tag = "state", rename_all = "snake_case")]`.
public enum AgentState: Codable, Equatable, Sendable {
    case idle, running
    case awaiting(from: String)
    case approval(tool: String)
    case error, cancelled

    private enum K: String, CodingKey { case state, from, tool }

    public init(from decoder: Decoder) throws {
        let c = try decoder.container(keyedBy: K.self)
        switch try c.decode(String.self, forKey: .state) {
        case "idle": self = .idle
        case "running": self = .running
        case "awaiting": self = .awaiting(from: try c.decode(String.self, forKey: .from))
        case "approval": self = .approval(tool: try c.decode(String.self, forKey: .tool))
        case "error": self = .error
        case "cancelled": self = .cancelled
        case let s: throw DecodingError.dataCorruptedError(forKey: .state, in: c, debugDescription: "unknown state \(s)")
        }
    }

    public func encode(to encoder: Encoder) throws {
        var c = encoder.container(keyedBy: K.self)
        try c.encode(word, forKey: .state)
        if case .awaiting(let f) = self { try c.encode(f, forKey: .from) }
        if case .approval(let t) = self { try c.encode(t, forKey: .tool) }
    }

    public var word: String {
        switch self {
        case .idle: return "idle"
        case .running: return "running"
        case .awaiting: return "awaiting"
        case .approval: return "approval"
        case .error: return "error"
        case .cancelled: return "cancelled"
        }
    }
}

public struct SessionActivity: Codable, Equatable, Sendable {
    public var state: AgentState
    public var lastActive: Date?
    public var turnStartedAt: Date?
    public var jobs: UInt16
    public var ptys: UInt16
    public var children: UInt16
    public var asks: UInt16?
    public var live: Bool
    /// W0 E2 — optional on today's engine.
    public var waitingSinceMs: UInt64?

    public init(state: AgentState = .idle, lastActive: Date? = nil, turnStartedAt: Date? = nil, jobs: UInt16 = 0,
                ptys: UInt16 = 0, children: UInt16 = 0, asks: UInt16? = nil, live: Bool = false,
                waitingSinceMs: UInt64? = nil) {
        self.state = state; self.lastActive = lastActive; self.turnStartedAt = turnStartedAt; self.jobs = jobs
        self.ptys = ptys; self.children = children; self.asks = asks; self.live = live
        self.waitingSinceMs = waitingSinceMs
    }
}

public struct AgentInfo: Codable, Equatable, Sendable {
    public var id: String
    public var name: String
    public var parent: String
    public var depth: UInt8
    public var agent: String?
    public var model: String
    public var state: AgentState
    public var startedAt: Date?
    public var finishedAt: Date?
    public var last: String
    public var project: String?
    public var activity: SessionActivity

    public init(id: String, name: String, parent: String = "", depth: UInt8 = 0, agent: String? = nil,
                model: String = "", state: AgentState = .idle, startedAt: Date? = nil, finishedAt: Date? = nil,
                last: String = "", project: String? = nil, activity: SessionActivity = .init()) {
        self.id = id; self.name = name; self.parent = parent; self.depth = depth; self.agent = agent
        self.model = model; self.state = state; self.startedAt = startedAt; self.finishedAt = finishedAt
        self.last = last; self.project = project; self.activity = activity
    }

    public init(from decoder: Decoder) throws {
        let c = try decoder.container(keyedBy: CodingKeys.self)
        id = try c.decode(String.self, forKey: .id)
        name = try c.decode(String.self, forKey: .name)
        parent = try c.decode(String.self, forKey: .parent)
        depth = try c.decode(UInt8.self, forKey: .depth)
        agent = try c.decodeIfPresent(String.self, forKey: .agent)
        model = try c.decode(String.self, forKey: .model)
        state = try c.decode(AgentState.self, forKey: .state)
        startedAt = try c.decodeIfPresent(Date.self, forKey: .startedAt)
        finishedAt = try c.decodeIfPresent(Date.self, forKey: .finishedAt)
        last = try c.decode(String.self, forKey: .last)
        project = try c.decodeIfPresent(String.self, forKey: .project)
        // `#[serde(default)]`: absent on engines older than 62; `state` duplicates `activity.state`.
        activity = try c.decodeIfPresent(SessionActivity.self, forKey: .activity) ?? SessionActivity(state: state)
    }
}

// MARK: tools::approval (crates/tools/src/approval.rs:30-66)

public enum Risk: String, Codable, Sendable { case readOnly = "read_only", mutating, destructive }

/// `Allow | Deny` only — there is no "allow for this session" on the wire.
public enum Decision: String, Codable, Sendable { case allow, deny }

public struct ApprovalRequested: Codable, Equatable, Sendable {
    public var id: String
    public var session: String
    public var tool: String
    public var args: JSONValue
    public var risk: Risk
    public init(id: String, session: String, tool: String, args: JSONValue, risk: Risk) {
        self.id = id; self.session = session; self.tool = tool; self.args = args; self.risk = risk
    }
}

public struct ApprovalResolved: Codable, Equatable, Sendable {
    public var id: String
    public var decision: Decision
    public var timedOut: Bool
}

// MARK: tools::ask (crates/tools/src/ask.rs:35-105)

public struct Choice: Codable, Equatable, Sendable {
    public var label: String
    public var description: String?
    public init(label: String, description: String? = nil) { self.label = label; self.description = description }
}

public struct Question: Codable, Equatable, Sendable {
    public var id: String
    public var question: String
    public var options: [Choice]
    public var recommended: Int?

    public init(id: String, question: String, options: [Choice] = [], recommended: Int? = nil) {
        self.id = id; self.question = question; self.options = options; self.recommended = recommended
    }

    public init(from decoder: Decoder) throws {
        let c = try decoder.container(keyedBy: CodingKeys.self)
        id = try c.decode(String.self, forKey: .id)
        question = try c.decode(String.self, forKey: .question)
        options = try c.decodeIfPresent([Choice].self, forKey: .options) ?? []
        recommended = try c.decodeIfPresent(Int.self, forKey: .recommended)
    }
}

/// `#[serde(tag = "kind", rename_all = "snake_case")]`.
public enum Answer: Codable, Equatable, Sendable {
    case choice(index: Int)
    case text(String)
    case skipped

    private enum K: String, CodingKey { case kind, index, text }

    public init(from decoder: Decoder) throws {
        let c = try decoder.container(keyedBy: K.self)
        switch try c.decode(String.self, forKey: .kind) {
        case "choice": self = .choice(index: try c.decode(Int.self, forKey: .index))
        case "text": self = .text(try c.decode(String.self, forKey: .text))
        case "skipped": self = .skipped
        case let s: throw DecodingError.dataCorruptedError(forKey: .kind, in: c, debugDescription: "unknown answer \(s)")
        }
    }

    public func encode(to encoder: Encoder) throws {
        var c = encoder.container(keyedBy: K.self)
        switch self {
        case .choice(let i): try c.encode("choice", forKey: .kind); try c.encode(i, forKey: .index)
        case .text(let t): try c.encode("text", forKey: .kind); try c.encode(t, forKey: .text)
        case .skipped: try c.encode("skipped", forKey: .kind)
        }
    }
}

public struct AskRequested: Codable, Equatable, Sendable {
    public var id: String
    public var session: String
    public var questions: [Question]
    public init(id: String, session: String, questions: [Question]) {
        self.id = id; self.session = session; self.questions = questions
    }
}

public struct AskResolved: Codable, Equatable, Sendable {
    public var id: String
    public var dismissed: Bool
    public var timedOut: Bool
}

// MARK: tasks::packet (crates/tasks/src/packet.rs:76-234)

public enum PacketKind: String, Codable, Sendable { case decision, review }

public struct PacketOption: Codable, Equatable, Sendable {
    public var label: String
    public var description: String?
    public init(label: String, description: String? = nil) { self.label = label; self.description = description }
}

public struct Check: Codable, Equatable, Sendable {
    public var text: String
    public var done: Bool
    public var note: String?
    public init(text: String, done: Bool, note: String? = nil) { self.text = text; self.done = done; self.note = note }
}

public struct Pointer: Codable, Equatable, Sendable {
    public var kind: String
    public var ref: String
    public var title: String
    public var note: String?
    public var text: String?
}

public struct Recommendation: Codable, Equatable, Sendable {
    public var text: String
    public var option: UInt32?
    public init(text: String, option: UInt32? = nil) { self.text = text; self.option = option }
}

public struct Packet: Codable, Equatable, Sendable {
    public var key: String
    public var task: String
    public var kind: PacketKind
    public var by: String
    public var atMs: UInt64
    public var question: String?
    public var summary: String
    public var board: String?
    public var options: [PacketOption]
    public var recommended: UInt32?
    public var pointers: [Pointer]
    public var checks: [Check]
    public var recommendation: Recommendation?
    public var to: String?

    public init(key: String, task: String, kind: PacketKind, by: String = "user", atMs: UInt64 = 0,
                question: String? = nil, summary: String = "", board: String? = nil, options: [PacketOption] = [],
                recommended: UInt32? = nil, pointers: [Pointer] = [], checks: [Check] = [],
                recommendation: Recommendation? = nil, to: String? = nil) {
        self.key = key; self.task = task; self.kind = kind; self.by = by; self.atMs = atMs
        self.question = question; self.summary = summary; self.board = board; self.options = options
        self.recommended = recommended; self.pointers = pointers; self.checks = checks
        self.recommendation = recommendation; self.to = to
    }

    public init(from decoder: Decoder) throws {
        let c = try decoder.container(keyedBy: CodingKeys.self)
        key = try c.decode(String.self, forKey: .key)
        task = try c.decode(String.self, forKey: .task)
        kind = try c.decode(PacketKind.self, forKey: .kind)
        by = try c.decode(String.self, forKey: .by)
        atMs = try c.decode(UInt64.self, forKey: .atMs)
        question = try c.decodeIfPresent(String.self, forKey: .question)
        summary = try c.decode(String.self, forKey: .summary)
        board = try c.decodeIfPresent(String.self, forKey: .board)
        options = try c.decodeIfPresent([PacketOption].self, forKey: .options) ?? []
        recommended = try c.decodeIfPresent(UInt32.self, forKey: .recommended)
        pointers = try c.decodeIfPresent([Pointer].self, forKey: .pointers) ?? []
        checks = try c.decodeIfPresent([Check].self, forKey: .checks) ?? []
        recommendation = try c.decodeIfPresent(Recommendation.self, forKey: .recommendation)
        to = try c.decodeIfPresent(String.self, forKey: .to)
    }

    /// The pre-pick: the forwarding manager's `recommendation.option` wins over `recommended` (103 F2).
    public var preferredOption: Int? {
        if let o = recommendation?.option { return Int(o) }
        return recommended.map(Int.init)
    }
}

// MARK: rpc (crates/rpc/src/api.rs:1874-1881) and the stream

public struct PendingPrompts: Codable, Equatable, Sendable {
    public var approvals: [ApprovalRequested]
    public var asks: [AskRequested]
    public var secrets: [JSONValue]
}

public struct TurnFinished: Codable, Equatable, Sendable {
    public var session: String
    public var turn: String
    public var stopReason: String
    public var context: UInt32
}

public struct Lagged: Codable, Equatable, Sendable {
    public var count: UInt64
}

/// One `subscribe_events` item, keyed by `type`. Everything the companion does not read is `.other`.
public enum WireEvent: Equatable, Sendable {
    case pendingPrompts(PendingPrompts)
    case approvalRequested(ApprovalRequested)
    case approvalResolved(ApprovalResolved)
    case askRequested(AskRequested)
    case askResolved(AskResolved)
    case turnFinished(TurnFinished)
    case attentionChanged(AttentionChanged)
    case lagged(Lagged)
    case other(String)

    private struct Tag: Decodable { let type: String }

    public static func decode(_ data: Data) throws -> WireEvent {
        let d = TheuthJSON.decoder
        switch try d.decode(Tag.self, from: data).type {
        case "PendingPrompts": return .pendingPrompts(try d.decode(PendingPrompts.self, from: data))
        case "ApprovalRequested": return .approvalRequested(try d.decode(ApprovalRequested.self, from: data))
        case "ApprovalResolved": return .approvalResolved(try d.decode(ApprovalResolved.self, from: data))
        case "AskRequested": return .askRequested(try d.decode(AskRequested.self, from: data))
        case "AskResolved": return .askResolved(try d.decode(AskResolved.self, from: data))
        case "TurnFinished": return .turnFinished(try d.decode(TurnFinished.self, from: data))
        case "AttentionChanged": return .attentionChanged(try d.decode(AttentionChanged.self, from: data))
        case "Lagged": return .lagged(try d.decode(Lagged.self, from: data))
        case let t: return .other(t)
        }
    }

    /// The 13 kinds `UI/components/hud/queries.ts:24-43` re-fetches `attention_list` on, until E1 lands.
    public static let attentionTriggers: Set<String> = [
        "ActivityAppended", "ApprovalRequested", "ApprovalResolved", "ArtifactShown", "AskRequested", "AskResolved",
        "NodeAdded", "NodeOnline", "NodeRemoved", "NodeRevoked", "SecretRequested", "SecretResolved", "Takeover",
    ]
}
