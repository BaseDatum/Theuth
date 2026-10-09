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

    public init(state: MeetingState = .idle, meeting: String? = nil, session: String? = nil, elapsedMs: UInt64 = 0,
                degraded: [Degradation] = [], cursor: String? = nil, topics: [Topic] = [], approvalWaiting: Bool = false,
                supported: MeetingSupport? = nil, accelerator: String = "") {
        self.state = state; self.meeting = meeting; self.session = session; self.elapsedMs = elapsedMs
        self.degraded = degraded; self.cursor = cursor; self.topics = topics; self.approvalWaiting = approvalWaiting
        self.supported = supported; self.accelerator = accelerator
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

    public static let types: Set<String> = [
        "Started", "State", "Level", "Utterance", "TopicAdded", "TopicRemoved", "Brief", "Listening", "ApprovalWaiting",
        "PostProgress", "ReviewReady", "Summarized", "Deleted", "OracleDeciding", "OracleDecision",
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
    }
    private struct TopicAddedRaw: Decodable { let topic: Topic }
    private struct TopicRemovedRaw: Decodable { let topic: String; let why: TopicGone }
    private struct Missing: Error, CustomStringConvertible { let description: String }

    public static func decode(_ data: Data) throws -> MeetingEvent? {
        let d = TheuthJSON.decoder
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
        default:
            return .oracleDecision(meeting: meeting, decision: try need(r.decision, "decision"))
        }
    }
}
