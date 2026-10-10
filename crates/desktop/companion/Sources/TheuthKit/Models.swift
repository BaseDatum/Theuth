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
    // 149 (optional on today's engine): E6's conversation context (Revision 1 row 5), 132 D4's order.
    public var context: ItemContext?
    public var priority: Priority?
    /// 132 D4: higher first; the engine's value for an item without a priority (`Priority::Medium.weight()`) when absent.
    public var rank: UInt32
    /// 155 §7.1: the engine's alert decision; `nil` when none, blanked for the focused session or a seen finish.
    public var alert: Alert?
    /// 155 §7.6: the `alert` key was on the wire — a 155 engine decides *whether*; `false` on an older engine,
    /// where the policy's own `classify` table still rules.
    public var engineDecides: Bool

    public init(id: String, kind: AttentionKind, title: String, detail: String? = nil, atMs: UInt64,
                task: String? = nil, agent: String? = nil, session: String? = nil, approval: String? = nil,
                decision: String? = nil, packet: String? = nil, ask: AskRequested? = nil, turn: UInt32? = nil,
                waitingSinceMs: UInt64? = nil, summary: String? = nil, nextSteps: [String] = [],
                context: ItemContext? = nil, priority: Priority? = nil, rank: UInt32 = 5,
                alert: Alert? = nil, engineDecides: Bool = false) {
        self.id = id; self.kind = kind; self.title = title; self.detail = detail; self.atMs = atMs
        self.task = task; self.agent = agent; self.session = session; self.approval = approval
        self.decision = decision; self.packet = packet; self.ask = ask; self.turn = turn
        self.waitingSinceMs = waitingSinceMs; self.summary = summary; self.nextSteps = nextSteps
        self.context = context; self.priority = priority; self.rank = rank
        self.alert = alert; self.engineDecides = engineDecides
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
        context = try c.decodeIfPresent(ItemContext.self, forKey: .context)
        priority = try c.decodeIfPresent(Priority.self, forKey: .priority)
        rank = try c.decodeIfPresent(UInt32.self, forKey: .rank) ?? 5
        summary = try c.decodeIfPresent(String.self, forKey: .summary)
        nextSteps = try c.decodeIfPresent([String].self, forKey: .nextSteps) ?? []
        engineDecides = c.contains(.alert)
        // An unknown future `class` word is no alert rather than a failed list.
        alert = (try? c.decodeIfPresent(Alert.self, forKey: .alert)) ?? nil
    }

    enum CodingKeys: String, CodingKey {
        case id, kind, title, detail, atMs, task, agent, session, approval, decision, packet, ask, turn
        case waitingSinceMs, summary, nextSteps, context, priority, rank, alert
    }

    /// When the wait began: E2's `waiting_since_ms`, else `at_ms`.
    public var waitingSince: Date { Date(timeIntervalSince1970: Double(waitingSinceMs ?? atMs) / 1000) }
}

/// 149 E6 (Revision 1 row 5): the conversation a session-born item sits in; both ≤ 160 chars, `nil` when unknown.
public struct ItemContext: Codable, Equatable, Sendable {
    public var request: String?
    public var lastReply: String?
    public init(request: String? = nil, lastReply: String? = nil) { self.request = request; self.lastReply = lastReply }
}

/// 155 §7.2: why a client should notify — `roster::alert::AlertClass`, snake_case.
public enum AlertClass: String, Codable, Equatable, Sendable {
    case needsYou = "needs_you"
    case finished
}

/// 155 §7.1: the alert on one item. `id` (`<item id>@<episode start ms>`) is stable for one episode; one banner per id.
public struct Alert: Codable, Equatable, Sendable {
    public var id: String
    public var cls: AlertClass
    public var why: String
    public var atMs: UInt64

    public init(id: String, cls: AlertClass, why: String, atMs: UInt64) {
        self.id = id; self.cls = cls; self.why = why; self.atMs = atMs
    }

    enum CodingKeys: String, CodingKey { case id, cls = "class", why, atMs }
}

/// `tasks::Priority`, snake_case (132 D4).
public enum Priority: String, Codable, Sendable { case urgent, high, medium, low }

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
    /// 132 D2: one honest line on the cost of a wrong answer (149 reads it).
    public var ifWrong: String?

    public init(key: String, task: String, kind: PacketKind, by: String = "user", atMs: UInt64 = 0,
                question: String? = nil, summary: String = "", board: String? = nil, options: [PacketOption] = [],
                recommended: UInt32? = nil, pointers: [Pointer] = [], checks: [Check] = [],
                recommendation: Recommendation? = nil, to: String? = nil, ifWrong: String? = nil) {
        self.key = key; self.task = task; self.kind = kind; self.by = by; self.atMs = atMs
        self.question = question; self.summary = summary; self.board = board; self.options = options
        self.recommended = recommended; self.pointers = pointers; self.checks = checks
        self.recommendation = recommendation; self.to = to; self.ifWrong = ifWrong
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
        ifWrong = try c.decodeIfPresent(String.self, forKey: .ifWrong)
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

// MARK: 149 — sessions (crates/agent/src/lib.rs:2232, crates/sessionsync/src/types.rs:251); `MeshStatus` lives in CompanionCore/AskRouter.swift

/// `list_sessions` row; only what the agents panel reads. `owner` is `""` for this engine's own sessions.
public struct SessionSummary: Codable, Equatable, Sendable, Identifiable {
    public var id: String
    public var title: String
    public var name: String?
    public var agent: String?
    public var parent: String?
    public var cwd: String?
    public var project: String?
    public var desk: String?
    public var updatedAt: Date?
    public var running: Bool
    public var owner: String
    public var archived: Bool

    public init(id: String, title: String = "", name: String? = nil, agent: String? = nil, parent: String? = nil,
                cwd: String? = nil, project: String? = nil, desk: String? = nil, updatedAt: Date? = nil,
                running: Bool = false, owner: String = "", archived: Bool = false) {
        self.id = id; self.title = title; self.name = name; self.agent = agent; self.parent = parent; self.cwd = cwd
        self.project = project; self.desk = desk; self.updatedAt = updatedAt; self.running = running
        self.owner = owner; self.archived = archived
    }

    public init(from decoder: Decoder) throws {
        let c = try decoder.container(keyedBy: CodingKeys.self)
        id = try c.decode(String.self, forKey: .id)
        title = try c.decodeIfPresent(String.self, forKey: .title) ?? ""
        name = try c.decodeIfPresent(String.self, forKey: .name)
        agent = try c.decodeIfPresent(String.self, forKey: .agent)
        parent = try c.decodeIfPresent(String.self, forKey: .parent)
        cwd = try c.decodeIfPresent(String.self, forKey: .cwd)
        project = try c.decodeIfPresent(String.self, forKey: .project)
        desk = try c.decodeIfPresent(String.self, forKey: .desk)
        updatedAt = try c.decodeIfPresent(Date.self, forKey: .updatedAt)
        running = try c.decodeIfPresent(Bool.self, forKey: .running) ?? false
        owner = try c.decodeIfPresent(String.self, forKey: .owner) ?? ""
        archived = try c.decodeIfPresent(Bool.self, forKey: .archived) ?? false
    }
}

/// `sessions_status`: the counts that label the host groups.
public struct SessionsStatus: Codable, Equatable, Sendable {
    public var engine: String
    public var here: UInt32
    public var elsewhere: UInt32
    public var logSeq: UInt64
    public var lastSnapshot: String?
}

// MARK: 149 — agent control (crates/agent/src/lib.rs:156, 340)

/// `cancel(session, scope)`: `tree` stops the children too.
public enum CancelScope: String, Codable, Sendable { case root, tree }

/// `send_input`/`steer` delivery.
public enum Delivery: String, Codable, Sendable { case steer, nextTurn = "next_turn" }

// MARK: 149 — point-and-say (crates/computer/src/lib.rs:142-163, pointer.rs:12-22)

public struct PointerSample: Codable, Equatable, Sendable {
    /// ms since the key press.
    public var tMs: UInt32
    /// Logical global points.
    public var x: Float
    public var y: Float
    public var pen: Bool
    public init(tMs: UInt32, x: Float, y: Float, pen: Bool = true) { self.tMs = tMs; self.x = x; self.y = y; self.pen = pen }
}

public struct ScreenContextReq: Codable, Equatable, Sendable {
    public var samples: [PointerSample]
    public var color: String
    public var sinceMs: UInt64
    public var accessibility: Bool
    public init(samples: [PointerSample], color: String, sinceMs: UInt64, accessibility: Bool) {
        self.samples = samples; self.color = color; self.sinceMs = sinceMs; self.accessibility = accessibility
    }
}

/// What the engine read under the trail: AX lines and the PNGs to attach as `files`.
public struct ScreenContextReply: Codable, Equatable, Sendable {
    public var lines: [String]
    public var files: [String]
    public init(lines: [String] = [], files: [String] = []) { self.lines = lines; self.files = files }
}

// MARK: 149 — decision (crates/decision/src/lib.rs:584, gate.rs:18)

public enum DecisionModeKind: String, Codable, Sendable { case shadow, active, off }

public struct ModeInfo: Codable, Equatable, Sendable {
    public var mode: DecisionModeKind
    public var effective: DecisionModeKind
    public var reason: String?
    public var enabled: Bool
    public var installed: Bool
    public var budgetMs: UInt64
    public var escalateBudgetMs: UInt64
}

// MARK: 149 — meeting (crates/meeting/src/lib.rs:139-243, 268-520, 686-765; ids are uuid strings)

public enum Track: String, Codable, Sendable { case mic = "Mic", system = "System" }

public enum PostStage: String, Codable, Sendable { case transcribing = "Transcribing", diarizing = "Diarizing", aligning = "Aligning" }

/// `MeetingState`: externally tagged — a bare word, or `{"Post": "<stage>"}`; anything newer is `.other`.
public enum MeetingState: Codable, Equatable, Sendable {
    case idle, starting, recording, paused, stopping, review, summarizing, done, failed
    case post(PostStage)
    case other(String)

    static let words: [String: MeetingState] = [
        "Idle": .idle, "Starting": .starting, "Recording": .recording, "Paused": .paused, "Stopping": .stopping,
        "Review": .review, "Summarizing": .summarizing, "Done": .done, "Failed": .failed,
    ]

    public init(from decoder: Decoder) throws {
        let c = try decoder.singleValueContainer()
        if let s = try? c.decode(String.self) { self = Self.words[s] ?? .other(s); return }
        let o = try c.decode([String: PostStage].self)
        guard let stage = o["Post"] else { throw DecodingError.dataCorruptedError(in: c, debugDescription: "unknown meeting state \(o.keys)") }
        self = .post(stage)
    }

    public func encode(to encoder: Encoder) throws {
        var c = encoder.singleValueContainer()
        switch self {
        case .post(let s): try c.encode(["Post": s])
        case .other(let s): try c.encode(s)
        default: try c.encode(Self.words.first { $0.value == self }!.key)
        }
    }

    /// `UI/store/meeting.ts`'s `live`: recording or paused.
    public var isLive: Bool { self == .recording || self == .paused }
}

/// `Degradation`: a bare word, or `{"SttDown": "<track>"}` / `{"PermissionDenied": "<permission>"}`.
public enum Degradation: Codable, Equatable, Sendable {
    case micNoAec, modelDown, diskLow, nearMaxLength
    case sttDown(Track)
    case permissionDenied(String)
    case other(String)

    static let words: [String: Degradation] = [
        "MicNoAec": .micNoAec, "ModelDown": .modelDown, "DiskLow": .diskLow, "NearMaxLength": .nearMaxLength,
    ]

    public init(from decoder: Decoder) throws {
        let c = try decoder.singleValueContainer()
        if let s = try? c.decode(String.self) { self = Self.words[s] ?? .other(s); return }
        let o = try c.decode([String: String].self)
        if let t = o["SttDown"], let track = Track(rawValue: t) { self = .sttDown(track) }
        else if let p = o["PermissionDenied"] { self = .permissionDenied(p) }
        else { self = .other(o.keys.first ?? "") }
    }

    public func encode(to encoder: Encoder) throws {
        var c = encoder.singleValueContainer()
        switch self {
        case .sttDown(let t): try c.encode(["SttDown": t.rawValue])
        case .permissionDenied(let p): try c.encode(["PermissionDenied": p])
        case .other(let s): try c.encode(s)
        default: try c.encode(Self.words.first { $0.value == self }!.key)
        }
    }
}

/// `Support` (`crates/meeting/src/lib.rs:211`, `#[serde(tag = "kind", content = "reason")]`): `{"kind":"Full"}`,
/// `{"kind":"Partial","reason":…}`, `{"kind":"None","reason":…}`. 149 W3: the live engine answers this shape — the contract's
/// externally-tagged reading (`"Full"` / `{"Partial": why}`) threw on `meeting_status` and is kept only as a fallback.
public enum MeetingSupport: Codable, Equatable, Sendable {
    case full
    case partial(String)
    case unsupported(String)

    private struct Tagged: Decodable { let kind: String; let reason: String? }

    public init(from decoder: Decoder) throws {
        let c = try decoder.singleValueContainer()
        if let s = try? c.decode(String.self) {
            guard s == "Full" else { throw DecodingError.dataCorruptedError(in: c, debugDescription: "unknown support \(s)") }
            self = .full; return
        }
        if let t = try? c.decode(Tagged.self) {
            switch t.kind {
            case "Full": self = .full
            case "Partial": self = .partial(t.reason ?? "")
            case "None": self = .unsupported(t.reason ?? "")
            default: throw DecodingError.dataCorruptedError(in: c, debugDescription: "unknown support kind \(t.kind)")
            }
            return
        }
        let o = try c.decode([String: String].self)
        if let why = o["Partial"] { self = .partial(why) }
        else if let why = o["None"] { self = .unsupported(why) }
        else { throw DecodingError.dataCorruptedError(in: c, debugDescription: "unknown support \(o.keys)") }
    }

    public func encode(to encoder: Encoder) throws {
        var c = encoder.container(keyedBy: Keys.self)
        switch self {
        case .full: try c.encode("Full", forKey: .kind)
        case .partial(let w): try c.encode("Partial", forKey: .kind); try c.encode(w, forKey: .reason)
        case .unsupported(let w): try c.encode("None", forKey: .kind); try c.encode(w, forKey: .reason)
        }
    }

    private enum Keys: String, CodingKey { case kind, reason }
}

public enum BriefSource: String, Codable, Sendable {
    case auto = "Auto", tap = "Tap", request = "Request", deeper = "Deeper", typed = "Typed", cache = "Cache", oracle = "Oracle"
}

public enum TopicGone: String, Codable, Sendable { case viewed = "Viewed", dismissed = "Dismissed", expired = "Expired" }

public struct Topic: Codable, Equatable, Sendable, Identifiable {
    public var id: String
    public var label: String
    public var atMs: UInt64
    public var brief: String?
    public var cached: String?
    public var source: BriefSource?
    public init(id: String, label: String, atMs: UInt64, brief: String? = nil, cached: String? = nil, source: BriefSource? = nil) {
        self.id = id; self.label = label; self.atMs = atMs; self.brief = brief; self.cached = cached; self.source = source
    }
}

public struct Brief: Codable, Equatable, Sendable, Identifiable {
    public var id: String
    public var topic: String?
    public var title: String
    public var bullets: [String]
    public var source: BriefSource
    public var atMs: UInt64
    public var parent: String?
    public var cached: String?
    public init(id: String, topic: String? = nil, title: String, bullets: [String] = [], source: BriefSource = .auto,
                atMs: UInt64 = 0, parent: String? = nil, cached: String? = nil) {
        self.id = id; self.topic = topic; self.title = title; self.bullets = bullets; self.source = source
        self.atMs = atMs; self.parent = parent; self.cached = cached
    }
}

/// One transcript line (`meeting_live` rows and the `Utterance` event's body).
public struct Utterance: Codable, Equatable, Sendable {
    public var track: Track
    public var t0Ms: UInt64
    public var t1Ms: UInt64
    public var text: String
    public var request: Bool
    public var speaker: String?
    public init(track: Track, t0Ms: UInt64, t1Ms: UInt64, text: String, request: Bool = false, speaker: String? = nil) {
        self.track = track; self.t0Ms = t0Ms; self.t1Ms = t1Ms; self.text = text; self.request = request; self.speaker = speaker
    }
}

/// The 10 Hz meters (`Level` event), linear RMS of the last 100 ms per track.
public struct Level: Codable, Equatable, Sendable {
    public var meeting: String
    public var micRms: Float
    public var systemRms: Float
    public init(meeting: String, micRms: Float, systemRms: Float) { self.meeting = meeting; self.micRms = micRms; self.systemRms = systemRms }
}

/// `meeting::ProcessInfo` (the tapped app); named apart from Foundation's `ProcessInfo`.
public struct MeetingProcess: Codable, Equatable, Sendable {
    public var pid: UInt32
    public var name: String
    public var bundleId: String?
    public var active: Bool
}

// MARK: 158 — detection and calendar (crates/meeting/src/detect/mod.rs, calendar/mod.rs; C158 contract, W1/W2 fill)

/// `Cleared` (158 §4.4): why a detection prompt went away.
public enum DetectionCleared: String, Codable, Sendable { case notNow = "NotNow", never = "Never", released = "Released", started = "Started" }

/// `Attendee` (158 §8.3).
public struct Attendee: Codable, Equatable, Sendable {
    public var name: String
    public var email: String?
    public var status: String?
    public var isMe: Bool
    public init(name: String, email: String? = nil, status: String? = nil, isMe: Bool = false) {
        self.name = name; self.email = email; self.status = status; self.isMe = isMe
    }
    public init(from decoder: Decoder) throws {
        let c = try decoder.container(keyedBy: CodingKeys.self)
        name = try c.decode(String.self, forKey: .name)
        email = try c.decodeIfPresent(String.self, forKey: .email)
        status = try c.decodeIfPresent(String.self, forKey: .status)
        isMe = try c.decodeIfPresent(Bool.self, forKey: .isMe) ?? false
    }
}

/// `EventRef` (158 §8.3): the event a prompt or a manifest is tied to; `id` is `source:uid:recurrence_id`.
public struct EventRef: Codable, Equatable, Sendable, Identifiable {
    public var id: String
    public var uid: String
    public var title: String
    public var start: String
    public var end: String
    public var joinUrl: String?
    public var joinHost: String?
    public var organizer: Attendee?
    public var attendees: [Attendee]
    public var description: String?
    public init(id: String, uid: String, title: String, start: String, end: String, joinUrl: String? = nil, joinHost: String? = nil,
                organizer: Attendee? = nil, attendees: [Attendee] = [], description: String? = nil) {
        self.id = id; self.uid = uid; self.title = title; self.start = start; self.end = end; self.joinUrl = joinUrl
        self.joinHost = joinHost; self.organizer = organizer; self.attendees = attendees; self.description = description
    }
    public init(from decoder: Decoder) throws {
        let c = try decoder.container(keyedBy: CodingKeys.self)
        id = try c.decode(String.self, forKey: .id)
        uid = try c.decode(String.self, forKey: .uid)
        title = try c.decode(String.self, forKey: .title)
        start = try c.decode(String.self, forKey: .start)
        end = try c.decode(String.self, forKey: .end)
        joinUrl = try c.decodeIfPresent(String.self, forKey: .joinUrl)
        joinHost = try c.decodeIfPresent(String.self, forKey: .joinHost)
        organizer = try c.decodeIfPresent(Attendee.self, forKey: .organizer)
        attendees = try c.decodeIfPresent([Attendee].self, forKey: .attendees) ?? []
        description = try c.decodeIfPresent(String.self, forKey: .description)
    }
}

/// `EventMatch` (158 §8.1): the shown event, its score, the other candidates for the `[▾]` list.
public struct EventMatch: Codable, Equatable, Sendable {
    public var event: EventRef
    public var score: Float
    public var alternatives: [EventRef]
    public init(event: EventRef, score: Float, alternatives: [EventRef] = []) {
        self.event = event; self.score = score; self.alternatives = alternatives
    }
    public init(from decoder: Decoder) throws {
        let c = try decoder.container(keyedBy: CodingKeys.self)
        event = try c.decode(EventRef.self, forKey: .event)
        score = try c.decode(Float.self, forKey: .score)
        alternatives = try c.decodeIfPresent([EventRef].self, forKey: .alternatives) ?? []
    }
}

/// `Detection` (158 §4.5): the prompt — `meeting_detection`, `MeetingStatus.detection`, `MeetingEvent.detected`.
public struct Detection: Codable, Equatable, Sendable {
    public var app: MeetingProcess
    public var appId: String
    public var since: String
    public var also: [MeetingProcess]
    public var event: EventMatch?
    public var project: String?
    public var brief: [String]?
    public init(app: MeetingProcess, appId: String, since: String, also: [MeetingProcess] = [], event: EventMatch? = nil,
                project: String? = nil, brief: [String]? = nil) {
        self.app = app; self.appId = appId; self.since = since; self.also = also; self.event = event; self.project = project; self.brief = brief
    }
    public init(from decoder: Decoder) throws {
        let c = try decoder.container(keyedBy: CodingKeys.self)
        app = try c.decode(MeetingProcess.self, forKey: .app)
        appId = try c.decode(String.self, forKey: .appId)
        since = try c.decode(String.self, forKey: .since)
        also = try c.decodeIfPresent([MeetingProcess].self, forKey: .also) ?? []
        event = try c.decodeIfPresent(EventMatch.self, forKey: .event)
        project = try c.decodeIfPresent(String.self, forKey: .project)
        brief = try c.decodeIfPresent([String].self, forKey: .brief)
    }
}

/// `CalEvent` (158 §7.4): one event-store row; what `calendar_push` sends (W2 fills from EventKit).
public struct CalEvent: Codable, Equatable, Sendable, Identifiable {
    public var id: String
    public var source: String
    public var uid: String
    public var title: String
    public var start: String
    public var end: String
    public var joinUrl: String?
    public var joinHost: String?
    public var organizer: Attendee?
    public var attendees: [Attendee]
    public var description: String?
    public var allDay: Bool
    public var tz: String?
    public var cancelled: Bool
    public var sequence: UInt32
    public var location: String?
    public var recurrenceId: String?
    public init(id: String, source: String, uid: String, title: String, start: String, end: String, joinUrl: String? = nil,
                joinHost: String? = nil, organizer: Attendee? = nil, attendees: [Attendee] = [], description: String? = nil,
                allDay: Bool = false, tz: String? = nil, cancelled: Bool = false, sequence: UInt32 = 0, location: String? = nil,
                recurrenceId: String? = nil) {
        self.id = id; self.source = source; self.uid = uid; self.title = title; self.start = start; self.end = end
        self.joinUrl = joinUrl; self.joinHost = joinHost; self.organizer = organizer; self.attendees = attendees
        self.description = description; self.allDay = allDay; self.tz = tz; self.cancelled = cancelled; self.sequence = sequence
        self.location = location; self.recurrenceId = recurrenceId
    }
}

/// `StartMeeting` (crates/meeting/src/lib.rs): the frozen six fields plus 158's optional four; `nil`s are omitted.
public struct StartMeeting: Codable, Equatable, Sendable {
    public var process: UInt32?
    public var title: String?
    public var context: String?
    public var consent: Bool
    public var sessionModel: String?
    public var memory: String?
    /// 158: `{"kind":"detected","app_id":…}` etc.; `nil` = manual.
    public var source: StartSource?
    public var event: String?
    public var project: String?
    public var template: String?
    public init(process: UInt32? = nil, title: String? = nil, context: String? = nil, consent: Bool, sessionModel: String? = nil,
                memory: String? = nil, source: StartSource? = nil, event: String? = nil, project: String? = nil, template: String? = nil) {
        self.process = process; self.title = title; self.context = context; self.consent = consent; self.sessionModel = sessionModel
        self.memory = memory; self.source = source; self.event = event; self.project = project; self.template = template
    }
}

/// `detect::Source` (158 §15.1, TS `StartSource`): `#[serde(tag = "kind", rename_all = "snake_case")]`.
public enum StartSource: Codable, Equatable, Sendable {
    case manual
    case detected(appId: String)
    case calendar(event: String)

    private enum Keys: String, CodingKey { case kind, appId, event }

    public init(from decoder: Decoder) throws {
        let c = try decoder.container(keyedBy: Keys.self)
        switch try c.decode(String.self, forKey: .kind) {
        case "manual": self = .manual
        case "detected": self = .detected(appId: try c.decode(String.self, forKey: .appId))
        case "calendar": self = .calendar(event: try c.decode(String.self, forKey: .event))
        case let s: throw DecodingError.dataCorruptedError(forKey: .kind, in: c, debugDescription: "unknown start source \(s)")
        }
    }

    public func encode(to encoder: Encoder) throws {
        var c = encoder.container(keyedBy: Keys.self)
        switch self {
        case .manual: try c.encode("manual", forKey: .kind)
        case .detected(let appId): try c.encode("detected", forKey: .kind); try c.encode(appId, forKey: .appId)
        case .calendar(let event): try c.encode("calendar", forKey: .kind); try c.encode(event, forKey: .event)
        }
    }
}

/// The oracle's verdict (crates/meeting/src/oracle.rs:89); the island flashes `batchId`, the rest is not read.
public struct OracleDecision: Codable, Equatable, Sendable {
    public var batchId: String
    public var family: String
}

public enum PermissionState: String, Codable, Sendable { case granted = "Granted", denied = "Denied", notDetermined = "NotDetermined" }

public struct MeetingPermissions: Codable, Equatable, Sendable {
    public var microphone: PermissionState
    public var audioCapture: PermissionState
}

/// `meeting_status` (`Status`, exported as `MeetingStatus`). `supported` is `nil` when no loopback backend opens.
public struct MeetingStatus: Codable, Equatable, Sendable {
    public var state: MeetingState
    public var meeting: String?
    public var session: String?
    public var elapsedMs: UInt64
    public var degraded: [Degradation]
    public var cursor: String?
    public var topics: [Topic]
    public var approvalWaiting: Bool
    public var supported: MeetingSupport?
    public var accelerator: String
    /// 158 §4.5: the current detection prompt, for a companion connecting mid-prompt.
    public var detection: Detection?

    public init(state: MeetingState = .idle, meeting: String? = nil, session: String? = nil, elapsedMs: UInt64 = 0,
                degraded: [Degradation] = [], cursor: String? = nil, topics: [Topic] = [], approvalWaiting: Bool = false,
                supported: MeetingSupport? = nil, accelerator: String = "", detection: Detection? = nil) {
        self.state = state; self.meeting = meeting; self.session = session; self.elapsedMs = elapsedMs
        self.degraded = degraded; self.cursor = cursor; self.topics = topics; self.approvalWaiting = approvalWaiting
        self.supported = supported; self.accelerator = accelerator; self.detection = detection
    }

    public init(from decoder: Decoder) throws {
        let c = try decoder.container(keyedBy: CodingKeys.self)
        state = try c.decode(MeetingState.self, forKey: .state)
        meeting = try c.decodeIfPresent(String.self, forKey: .meeting)
        session = try c.decodeIfPresent(String.self, forKey: .session)
        elapsedMs = try c.decodeIfPresent(UInt64.self, forKey: .elapsedMs) ?? 0
        degraded = try c.decodeIfPresent([Degradation].self, forKey: .degraded) ?? []
        cursor = try c.decodeIfPresent(String.self, forKey: .cursor)
        topics = try c.decodeIfPresent([Topic].self, forKey: .topics) ?? []
        approvalWaiting = try c.decodeIfPresent(Bool.self, forKey: .approvalWaiting) ?? false
        supported = try c.decodeIfPresent(MeetingSupport.self, forKey: .supported)
        accelerator = try c.decodeIfPresent(String.self, forKey: .accelerator) ?? ""
        detection = try c.decodeIfPresent(Detection.self, forKey: .detection)
    }
}

/// The meeting items of the `subscribe_events` stream (`#[serde(tag = "type")]`, crates/meeting/src/lib.rs:268-376):
/// `MeetingEvent.decode` answers `nil` for every other `type`, so it sits beside `WireEvent.decode` on the same bytes.
public enum MeetingEvent: Equatable, Sendable {
    case started(meeting: String, session: String, process: MeetingProcess, at: String)
    case state(meeting: String, state: MeetingState, degraded: [Degradation])
    case level(Level)
    /// `final == false` is the rolling partial of an open utterance.
    case utterance(meeting: String, utterance: Utterance, final: Bool)
    case topicAdded(meeting: String, topic: Topic)
    case topicRemoved(meeting: String, topic: String, why: TopicGone)
    case brief(meeting: String, brief: Brief)
    case listening(meeting: String, held: Bool)
    case approvalWaiting(meeting: String, waiting: Bool)
    case postProgress(meeting: String, stage: PostStage, pct: UInt8)
    case reviewReady(meeting: String)
    case summarized(meeting: String, title: String)
    case deleted(meeting: String)
    case oracleDeciding(meeting: String, batchId: String)
    case oracleDecision(meeting: String, decision: OracleDecision)
    /// 158 §4.5: an eligible app has held the mic ≥ 15 s — the prompt. No `meeting` yet.
    case detected(Detection)
    /// 158 §4.5: the prompt went away. No `meeting`.
    case detectionCleared(appId: String, reason: DetectionCleared)
    /// 158 §4.4 rule 7: the detected app released the mic for `releasedForS` — "Stop recording?".
    case appReleased(meeting: String, app: MeetingProcess, releasedForS: UInt32)
    /// 158 §9.4: a matched-or-remembered event within 5 minutes — island state (c). No `meeting`.
    case upcoming(event: EventRef, project: String?, brief: [String]?)

    public static let types: Set<String> = [
        "Started", "State", "Level", "Utterance", "TopicAdded", "TopicRemoved", "Brief", "Listening", "ApprovalWaiting",
        "PostProgress", "ReviewReady", "Summarized", "Deleted", "OracleDeciding", "OracleDecision",
        "Detected", "DetectionCleared", "AppReleased", "Upcoming",
    ]

    /// Every field of every variant but the two topic ones, optional (`topic` is an object in `TopicAdded` and an
    /// id in `TopicRemoved`, so those decode on their own).
    private struct Raw: Decodable {
        let type: String
        let meeting: String?
        let session: String?, process: MeetingProcess?, at: String?
        let state: MeetingState?, degraded: [Degradation]?
        let micRms: Float?, systemRms: Float?
        let track: Track?, t0Ms: UInt64?, t1Ms: UInt64?, text: String?, request: Bool?, `final`: Bool?, speaker: String?
        let brief: Brief?
        let held: Bool?, waiting: Bool?
        let stage: PostStage?, pct: UInt8?
        let title: String?, batchId: String?, decision: OracleDecision?
        let app: MeetingProcess?, releasedForS: UInt32?
    }
    private struct TopicAddedRaw: Decodable { let topic: Topic }
    private struct TopicRemovedRaw: Decodable { let topic: String; let why: TopicGone }
    private struct DetectionClearedRaw: Decodable { let appId: String; let reason: DetectionCleared }
    private struct UpcomingRaw: Decodable { let event: EventRef; let project: String?; let brief: [String]? }
    private struct Missing: Error, CustomStringConvertible { let description: String }
    private struct TypeOnly: Decodable { let type: String }

    public static func decode(_ data: Data) throws -> MeetingEvent? {
        let d = TheuthJSON.decoder
        // 158: the pre-meeting events carry no `meeting` and `brief` is a string list there, so they
        // decode from their own shapes before `Raw` (whose `brief` is a `Brief`).
        switch try d.decode(TypeOnly.self, from: data).type {
        case "Detected":
            return .detected(try d.decode(Detection.self, from: data))
        case "DetectionCleared":
            let c = try d.decode(DetectionClearedRaw.self, from: data)
            return .detectionCleared(appId: c.appId, reason: c.reason)
        case "Upcoming":
            let u = try d.decode(UpcomingRaw.self, from: data)
            return .upcoming(event: u.event, project: u.project, brief: u.brief)
        default: break
        }
        let r = try d.decode(Raw.self, from: data)
        guard Self.types.contains(r.type) else { return nil }
        func need<T>(_ v: T?, _ key: String) throws -> T {
            guard let v else { throw Missing(description: "\(r.type) without `\(key)`") }
            return v
        }
        let meeting = try need(r.meeting, "meeting")
        switch r.type {
        case "Started":
            return .started(meeting: meeting, session: try need(r.session, "session"), process: try need(r.process, "process"), at: try need(r.at, "at"))
        case "State":
            return .state(meeting: meeting, state: try need(r.state, "state"), degraded: r.degraded ?? [])
        case "Level":
            return .level(Level(meeting: meeting, micRms: try need(r.micRms, "mic_rms"), systemRms: try need(r.systemRms, "system_rms")))
        case "Utterance":
            let u = Utterance(track: try need(r.track, "track"), t0Ms: try need(r.t0Ms, "t0_ms"), t1Ms: try need(r.t1Ms, "t1_ms"),
                              text: try need(r.text, "text"), request: r.request ?? false, speaker: r.speaker)
            return .utterance(meeting: meeting, utterance: u, final: r.final ?? true)
        case "TopicAdded":
            return .topicAdded(meeting: meeting, topic: try d.decode(TopicAddedRaw.self, from: data).topic)
        case "TopicRemoved":
            let t = try d.decode(TopicRemovedRaw.self, from: data)
            return .topicRemoved(meeting: meeting, topic: t.topic, why: t.why)
        case "Brief":
            return .brief(meeting: meeting, brief: try need(r.brief, "brief"))
        case "Listening":
            return .listening(meeting: meeting, held: try need(r.held, "held"))
        case "ApprovalWaiting":
            return .approvalWaiting(meeting: meeting, waiting: try need(r.waiting, "waiting"))
        case "PostProgress":
            return .postProgress(meeting: meeting, stage: try need(r.stage, "stage"), pct: try need(r.pct, "pct"))
        case "ReviewReady":
            return .reviewReady(meeting: meeting)
        case "Summarized":
            return .summarized(meeting: meeting, title: try need(r.title, "title"))
        case "Deleted":
            return .deleted(meeting: meeting)
        case "OracleDeciding":
            return .oracleDeciding(meeting: meeting, batchId: try need(r.batchId, "batch_id"))
        case "AppReleased":
            return .appReleased(meeting: meeting, app: try need(r.app, "app"), releasedForS: try need(r.releasedForS, "released_for_s"))
        default:
            return .oracleDecision(meeting: meeting, decision: try need(r.decision, "decision"))
        }
    }
}

// MARK: 153 — voice (crates/voice/src/lib.rs:291-428, shape/mod.rs:57-124, dictionary/mod.rs:27-120, history/mod.rs:58-109;
// crates/agent/src/lib.rs:743-752, 1981-2000). Field names are the Rust ones through `convertFromSnakeCase`; the ts-rs files
// under `crates/desktop/ui/src/rpc/bindings/` are the cross-check. Fixtures: `Tests/TheuthKitTests/Fixtures/153-*.json`.

/// `Surface`, snake_case; `nil` in `DictationOpts` = `composer`.
public enum Surface: String, Codable, Sendable { case composer, tui, companion, phone, anyApp = "any_app" }

/// `Destination`, `#[serde(tag = "kind", rename_all = "snake_case")]`; `anyApp` carries the bundle id / exe.
public enum Destination: Codable, Equatable, Sendable {
    case agentPrompt, document, messageReply
    case anyApp(app: String)

    private enum K: String, CodingKey { case kind, app }

    public init(from decoder: Decoder) throws {
        let c = try decoder.container(keyedBy: K.self)
        switch try c.decode(String.self, forKey: .kind) {
        case "agent_prompt": self = .agentPrompt
        case "document": self = .document
        case "message_reply": self = .messageReply
        case "any_app": self = .anyApp(app: try c.decode(String.self, forKey: .app))
        case let s: throw DecodingError.dataCorruptedError(forKey: .kind, in: c, debugDescription: "unknown destination \(s)")
        }
    }

    public func encode(to encoder: Encoder) throws {
        var c = encoder.container(keyedBy: K.self)
        switch self {
        case .agentPrompt: try c.encode("agent_prompt", forKey: .kind)
        case .document: try c.encode("document", forKey: .kind)
        case .messageReply: try c.encode("message_reply", forKey: .kind)
        case .anyApp(let app): try c.encode("any_app", forKey: .kind); try c.encode(app, forKey: .app)
        }
    }
}

/// `AppProfile` (§13): the companion's per-app row, sent with `DictationOpts`; the engine never persists it.
public struct AppProfile: Codable, Equatable, Sendable, Identifiable {
    public var id: String
    public var name: String
    public var terminal: Bool
    public var polish: Bool
    public var learn: Bool
    public var polishPrompt: String?
    public var rewritePrompt: String?
    public var commands: [String]

    public init(id: String, name: String, terminal: Bool = false, polish: Bool = false, learn: Bool = false,
                polishPrompt: String? = nil, rewritePrompt: String? = nil, commands: [String] = []) {
        self.id = id; self.name = name; self.terminal = terminal; self.polish = polish; self.learn = learn
        self.polishPrompt = polishPrompt; self.rewritePrompt = rewritePrompt; self.commands = commands
    }
}

/// `DictationOpts`, the trailing optional of `dictation_start` (§14.2); every field serde-defaulted, so encode only what is set.
public struct DictationOpts: Codable, Equatable, Sendable {
    public var surface: Surface?
    public var destination: Destination?
    public var app: AppProfile?
    public var commands: [String]
    /// §8.7: `now − press` on the caller's clock at the moment of the call.
    public var pressAgoMs: UInt32?

    public init(surface: Surface? = nil, destination: Destination? = nil, app: AppProfile? = nil, commands: [String] = [],
                pressAgoMs: UInt32? = nil) {
        self.surface = surface; self.destination = destination; self.app = app; self.commands = commands; self.pressAgoMs = pressAgoMs
    }

    private enum K: String, CodingKey { case surface, destination, app, commands, pressAgoMs }

    public init(from decoder: Decoder) throws {
        let c = try decoder.container(keyedBy: K.self)
        surface = try c.decodeIfPresent(Surface.self, forKey: .surface)
        destination = try c.decodeIfPresent(Destination.self, forKey: .destination)
        app = try c.decodeIfPresent(AppProfile.self, forKey: .app)
        commands = try c.decodeIfPresent([String].self, forKey: .commands) ?? []
        pressAgoMs = try c.decodeIfPresent(UInt32.self, forKey: .pressAgoMs)
    }
}

/// `DictationTimings` (§8.7), `dictation_status.last`.
public struct DictationTimings: Codable, Equatable, Sendable {
    public var surface: Surface
    public var pressToFirstPcmMs: UInt32?
    public var pressToFirstPartialMs: UInt32?
    public var pressToFirstFinalMs: UInt32?
    public var stopToFinalMs: UInt32?
    public var polishMs: UInt32?
    public var device: String?
}

/// `EndCommand` (§7), `#[serde(tag = "command", rename_all = "snake_case")]`.
public enum EndCommand: Codable, Equatable, Sendable {
    /// "send it"; `nearMiss` when accepted by tolerance at the final.
    case send(nearMiss: Bool)
    /// "scratch that": drop the previous closed utterance.
    case scratchThat

    private enum K: String, CodingKey { case command, nearMiss }

    public init(from decoder: Decoder) throws {
        let c = try decoder.container(keyedBy: K.self)
        switch try c.decode(String.self, forKey: .command) {
        case "send": self = .send(nearMiss: try c.decodeIfPresent(Bool.self, forKey: .nearMiss) ?? false)
        case "scratch_that": self = .scratchThat
        case let s: throw DecodingError.dataCorruptedError(forKey: .command, in: c, debugDescription: "unknown end command \(s)")
        }
    }

    public func encode(to encoder: Encoder) throws {
        var c = encoder.container(keyedBy: K.self)
        switch self {
        case .send(let n): try c.encode("send", forKey: .command); try c.encode(n, forKey: .nearMiss)
        case .scratchThat: try c.encode("scratch_that", forKey: .command)
        }
    }
}

/// `Action` (W-j): what the surface performs after `shaped`.
public enum Action: String, Codable, Sendable { case newLine = "new_line", newParagraph = "new_paragraph", tab }

/// `Preset` for `set_shape` / `voice.shape.<surface>`.
public enum ShapePreset: String, Codable, Sendable { case full, plain, off }

/// `EndPhrases` (§7.2) for `set_end_phrases`.
public struct EndPhrases: Codable, Equatable, Sendable {
    public var send: [String]
    public var scratch: [String]
    public var literal: String

    public init(send: [String] = ["send it"], scratch: [String] = ["scratch that"], literal: String = "literal") {
        self.send = send; self.scratch = scratch; self.literal = literal
    }
}

/// `EntryId`: a transparent `String` newtype on the wire.
public typealias EntryId = String

/// `dictionary::Kind`, snake_case.
public enum EntryKind: String, Codable, Sendable { case term, symbol, formatter, abbrev, mention, command, path }

/// `dictionary::Source`, snake_case.
public enum EntrySource: String, Codable, Sendable { case packaged, user, learned }

/// `Scope`, `#[serde(tag = "scope", rename_all = "snake_case")]` with named fields (as-built C).
public enum Scope: Codable, Equatable, Sendable {
    case global
    case surface(Surface)
    case app(String)

    private enum K: String, CodingKey { case scope, surface, app }

    public init(from decoder: Decoder) throws {
        let c = try decoder.container(keyedBy: K.self)
        switch try c.decode(String.self, forKey: .scope) {
        case "global": self = .global
        case "surface": self = .surface(try c.decode(Surface.self, forKey: .surface))
        case "app": self = .app(try c.decode(String.self, forKey: .app))
        case let s: throw DecodingError.dataCorruptedError(forKey: .scope, in: c, debugDescription: "unknown scope \(s)")
        }
    }

    public func encode(to encoder: Encoder) throws {
        var c = encoder.container(keyedBy: K.self)
        switch self {
        case .global: try c.encode("global", forKey: .scope)
        case .surface(let s): try c.encode("surface", forKey: .scope); try c.encode(s, forKey: .surface)
        case .app(let a): try c.encode("app", forKey: .scope); try c.encode(a, forKey: .app)
        }
    }
}

/// `Variant`: one heard form; `metaphone` is Rust's `(String, String)` — a two-element JSON array.
public struct Variant: Codable, Equatable, Sendable {
    public var heard: String
    public var metaphone: [String]
    public var seen: UInt32
    public var lastMs: UInt64

    public init(heard: String, metaphone: [String] = ["", ""], seen: UInt32 = 0, lastMs: UInt64 = 0) {
        self.heard = heard; self.metaphone = metaphone; self.seen = seen; self.lastMs = lastMs
    }
}

/// `Entry` (§5.3): `dictionary_list` row and `dictionary_put` argument.
public struct Entry: Codable, Equatable, Sendable, Identifiable {
    public var id: EntryId
    public var canonical: String
    public var kind: EntryKind
    public var spoken: [String]
    public var variants: [Variant]
    public var scope: Scope
    public var source: EntrySource
    public var tags: [String]

    public init(id: EntryId, canonical: String, kind: EntryKind = .term, spoken: [String] = [], variants: [Variant] = [],
                scope: Scope = .global, source: EntrySource = .user, tags: [String] = []) {
        self.id = id; self.canonical = canonical; self.kind = kind; self.spoken = spoken; self.variants = variants
        self.scope = scope; self.source = source; self.tags = tags
    }
}

/// `HistoryRow` (§6.5), `dictation_history` row. `destination` is the flattened word (`any_app:<bundle>`).
public struct HistoryRow: Codable, Equatable, Sendable, Identifiable {
    public var id: String
    public var startedMs: UInt64
    public var finishedMs: UInt64?
    public var surface: Surface
    public var destination: String
    public var app: String?
    public var raw: String
    public var shaped: String
    public var polished: String?
    public var delivered: String?
    public var pressMs: UInt32?
    public var firstPartialMs: UInt32?
    public var firstFinalMs: UInt32?
    public var finalMs: UInt32?
    /// `done | stopped | error:<message>`.
    public var reason: String
    public var audioBytes: UInt64
    public var hasAudio: Bool
}

/// `PolishReply` (§9): `text == nil` → deliver the shaped text; `refused` says why.
public struct PolishReply: Codable, Equatable, Sendable {
    public var text: String?
    public var model: String?
    public var ms: UInt32
    public var refused: String?
}

/// `SuggestionAnswer` (§6.4), snake_case.
public enum SuggestionAnswer: String, Codable, Sendable { case add, notNow = "not_now", never }

/// `DeviceState` (§8.2), snake_case.
public enum DeviceState: String, Codable, Sendable { case changed, lost }

/// `InputDevice`, the `list_input_devices` row.
public struct InputDevice: Codable, Equatable, Sendable {
    public var name: String
    public var `default`: Bool
}

/// `ModelInfo` (`polish_model` reply); only what a picker shows.
public struct ModelInfo: Codable, Equatable, Sendable, Identifiable {
    public var id: String
    public var name: String
    public var provider: String
}

/// `ModelState`, `#[serde(tag = "state", rename_all = "snake_case")]`; `failed` carries the message.
public enum ModelState: Codable, Equatable, Sendable {
    case missing, loading, ready
    case failed(message: String)

    private enum K: String, CodingKey { case state, message }

    public init(from decoder: Decoder) throws {
        let c = try decoder.container(keyedBy: K.self)
        switch try c.decode(String.self, forKey: .state) {
        case "missing": self = .missing
        case "loading": self = .loading
        case "ready": self = .ready
        case "failed": self = .failed(message: try c.decodeIfPresent(String.self, forKey: .message) ?? "")
        case let s: throw DecodingError.dataCorruptedError(forKey: .state, in: c, debugDescription: "unknown model state \(s)")
        }
    }

    public func encode(to encoder: Encoder) throws {
        var c = encoder.container(keyedBy: K.self)
        switch self {
        case .missing: try c.encode("missing", forKey: .state)
        case .loading: try c.encode("loading", forKey: .state)
        case .ready: try c.encode("ready", forKey: .state)
        case .failed(let m): try c.encode("failed", forKey: .state); try c.encode(m, forKey: .message)
        }
    }
}

/// `dictation_status` reply (`DictationState` + 153's `input_device`, `last`).
public struct DictationState: Codable, Equatable, Sendable {
    public var model: ModelState
    /// The running dictation's id, if any.
    public var active: String?
    public var inputDevice: String?
    public var last: DictationTimings?
}

/// `voice_status` reply; `inputDevices` is 153 §8.1's ordered mic preference.
public struct VoiceStatus: Codable, Equatable, Sendable {
    public var model: ModelState
    public var accelerator: String?
    public var voice: String
    public var voices: [String]
    public var speaking: String?
    public var queued: [String]?
    public var inputDevices: [String]?
}

/// Every `Dictation*` event in one decoder (the Ask bar's `dictationObserve` reads it): `type` picks the fields.
/// 153: `shaped`, `command`, `actions` on `DictationText`; `device`/`state` on `DictationDevice`.
public struct DictationWire: Decodable, Equatable, Sendable {
    public struct Reason: Decodable, Equatable, Sendable { public let reason: String; public let message: String? }
    public let type: String
    public let id: String?
    public let utterance: Int?
    public let text: String?
    public let isFinal: Bool?
    public let levels: [Float]?
    public let reason: Reason?
    /// `text` after shaping (§4); absent on a pre-153 engine.
    public let shaped: String?
    public let command: EndCommand?
    /// `Action` raw values (`new_line` …); strings so an unknown action never fails the whole event.
    public let actions: [String]?
    /// `DictationDevice`: the device now open (`nil` when lost) and what happened.
    public let device: String?
    public let state: DeviceState?

    enum CodingKeys: String, CodingKey {
        case type, id, utterance, text, isFinal = "final", levels, reason, shaped, command, actions, device, state
    }
}

/// The `DictationDevice` event (§8.2).
public struct DictationDevice: Codable, Equatable, Sendable {
    public var id: String
    public var device: String?
    public var state: DeviceState
}

/// The `DictionarySuggestion` event payload (§6.3, `history::Suggestion`); answered with `dictionary_suggestion_answer`.
public struct DictionarySuggestion: Codable, Equatable, Sendable, Identifiable {
    public var id: String
    public var heard: String
    public var typed: String
    public var occurrences: UInt32
    public var surface: Surface
    public var hasAudio: Bool
}
