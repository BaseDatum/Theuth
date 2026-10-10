import Foundation
import TheuthKit

// 149 W3 fills the bodies (design 149 §3.6; the port of `UI/store/meeting.ts:156-220`). C fixes the shapes.

/// The island's meeting state — the `MeetingSlice` of `UI/store/meeting.ts`.
public struct MeetingSlice: Equatable, Sendable {
    public struct Line: Equatable, Sendable {
        public var utterance: Utterance
        /// A rolling partial (`final == false`).
        public var open: Bool
        public init(utterance: Utterance, open: Bool) { self.utterance = utterance; self.open = open }
    }

    public struct Progress: Equatable, Sendable {
        public var stage: PostStage
        public var pct: UInt8
        public init(stage: PostStage, pct: UInt8) { self.stage = stage; self.pct = pct }
    }

    public var meeting: String?
    public var session: String?
    public var state: MeetingState
    public var degraded: [Degradation]
    /// Recording clock at `clockAt`.
    public var elapsedMs: UInt64
    public var clockAt: Date?
    public var topics: [Topic]
    public var tapped: [String]
    public var briefs: [Brief]
    /// Index into `briefs`; the brief-stack rule (`meeting.ts:181-190`).
    public var cursor: Int
    public var unseen: Int
    public var listening: Bool
    public var approvalWaiting: Bool
    public var progress: Progress?
    public var lines: [Line]
    public var deciding: String?
    public var oracle: [OracleDecision]
    public var level: Level?
    /// What the meeting's session is doing right now (`TurnStarted`/`ToolCall*`/`TurnFinished`): `thinking…`, a tool name, or nothing.
    public var activity: String?

    public static let empty = MeetingSlice(meeting: nil, session: nil, state: .idle, degraded: [], elapsedMs: 0, clockAt: nil,
                                           topics: [], tapped: [], briefs: [], cursor: -1, unseen: 0, listening: false,
                                           approvalWaiting: false, progress: nil, lines: [], deciding: nil, oracle: [], level: nil)
    public static let linesKept = 200
    public static let oracleKept = 8

    public init(meeting: String?, session: String?, state: MeetingState, degraded: [Degradation], elapsedMs: UInt64, clockAt: Date?,
                topics: [Topic], tapped: [String], briefs: [Brief], cursor: Int, unseen: Int, listening: Bool, approvalWaiting: Bool,
                progress: Progress?, lines: [Line], deciding: String?, oracle: [OracleDecision], level: Level?, activity: String? = nil) {
        self.meeting = meeting; self.session = session; self.state = state; self.degraded = degraded; self.elapsedMs = elapsedMs
        self.clockAt = clockAt; self.topics = topics; self.tapped = tapped; self.briefs = briefs; self.cursor = cursor
        self.unseen = unseen; self.listening = listening; self.approvalWaiting = approvalWaiting; self.progress = progress
        self.lines = lines; self.deciding = deciding; self.oracle = oracle; self.level = level; self.activity = activity
    }

    public var isLive: Bool { state.isLive }
    public var current: Brief? { briefs.indices.contains(cursor) ? briefs[cursor] : nil }
}

public enum MeetingReducer {
    /// Pure transition, one case per `MeetingEvent` variant (`meeting.ts:156-220`).
    public static func reduce(_ s: MeetingSlice, _ e: MeetingEvent, now: Date) -> MeetingSlice {
        var n = s
        switch e {
        case .started(let meeting, let session, _, _):
            n = .empty
            n.meeting = meeting; n.session = session; n.state = .recording; n.clockAt = now
        case .state(let meeting, let state, let degraded):
            if s.meeting != nil, s.meeting != meeting { return s }
            n.meeting = meeting; n.state = state; n.degraded = degraded
            n.elapsedMs = elapsed(s, now: now); n.clockAt = now
            // The cues end with the recording.
            n.listening = state.isLive && s.listening
            n.approvalWaiting = state.isLive && s.approvalWaiting
            if !state.isLive { n.activity = nil }
            if case .post = state {} else { n.progress = nil }
        case .level(let l):
            n.level = l
        case .utterance(let meeting, let u, let final):
            if s.meeting != meeting { return s }
            let line = MeetingSlice.Line(utterance: u, open: !final)
            // A labelled re-publish of a committed row updates it in place (07 §3).
            if final, u.speaker != nil, let i = s.lines.firstIndex(where: { !$0.open && $0.utterance.track == u.track && $0.utterance.t0Ms == u.t0Ms }) {
                n.lines[i] = line
            } else {
                n.lines = s.lines.filter { !($0.open && $0.utterance.track == u.track) } + [line]
                n.lines = Array(n.lines.suffix(MeetingSlice.linesKept))
            }
        case .topicAdded(_, let topic):
            n.topics = s.topics.filter { $0.id != topic.id } + [topic]
        case .topicRemoved(_, let topic, _):
            n.topics = s.topics.filter { $0.id != topic }
            n.tapped = s.tapped.filter { $0 != topic }
        case .brief(_, let brief):
            // 53 §4.6: Auto at the top → show; Auto stepped back → queue and badge; anything the user asked for jumps.
            // 05 §7: a Cache brief (the actor's alias scan) is an Auto for the stack.
            let auto = brief.source == .auto || brief.source == .cache
            n.briefs = s.briefs + [brief]
            if auto, !s.briefs.isEmpty, s.cursor < s.briefs.count - 1 {
                n.unseen = s.unseen + 1
            } else {
                n.cursor = n.briefs.count - 1; n.unseen = 0
                if let t = brief.topic { n.tapped = s.tapped.filter { $0 != t } }
            }
        case .listening(_, let held):
            n.listening = held
        case .approvalWaiting(_, let waiting):
            n.approvalWaiting = waiting
        case .postProgress(let meeting, let stage, let pct):
            if s.meeting != nil, s.meeting != meeting { return s }
            n.meeting = meeting; n.state = .post(stage); n.progress = .init(stage: stage, pct: pct)
        case .reviewReady(let meeting):
            if s.meeting != meeting { return s }
            n.state = .review; n.progress = nil
        case .summarized:
            return s
        case .deleted(let meeting):
            return s.meeting == meeting ? .empty : s
        case .oracleDeciding(let meeting, let batchId):
            if s.meeting != meeting { return s }
            n.deciding = batchId
        case .oracleDecision(let meeting, let decision):
            if s.meeting != meeting { return s }
            if let i = s.oracle.firstIndex(where: { $0.batchId == decision.batchId }) { n.oracle[i] = decision }
            else { n.oracle = Array((s.oracle + [decision]).suffix(MeetingSlice.oracleKept)) }
            if s.deciding == decision.batchId { n.deciding = nil }
        case .detected, .detectionCleared, .appReleased, .upcoming:
            // 158 C158: inert until W1/W3 add `MeetingSlice.prompt` (§15.4); the slice does not change.
            return s
        }
        return n
    }

    /// `meeting_status` at connect → the slice (`hydrate` adds the briefs and lines).
    public static func seed(_ status: MeetingStatus, now: Date) -> MeetingSlice {
        var n = MeetingSlice.empty
        n.meeting = status.meeting; n.session = status.session; n.state = status.state; n.degraded = status.degraded
        n.elapsedMs = status.elapsedMs; n.clockAt = now; n.topics = status.topics; n.approvalWaiting = status.approvalWaiting
        return n
    }

    /// `meeting_briefs` + `meeting_live` after `seed`; the cursor lands on `status.cursor` when it is one of them, else the top.
    public static func hydrate(_ s: MeetingSlice, briefs: [Brief], lines: [Utterance], cursor: String?) -> MeetingSlice {
        var n = s
        n.briefs = briefs
        n.cursor = cursor.flatMap { id in briefs.firstIndex { $0.id == id } } ?? briefs.count - 1
        n.lines = lines.suffix(MeetingSlice.linesKept).map { .init(utterance: $0, open: false) }
        n.unseen = 0
        return n
    }

    /// Step the cursor (`+1` newer, `-1` older); stepping up eats the badge one by one, down leaves it. `wrap` (the J/K/↑/↓
    /// keys, 149i §N) goes round at either end; the pane's chevrons clamp.
    public static func move(_ s: MeetingSlice, _ delta: Int, wrap: Bool = false) -> MeetingSlice {
        guard !s.briefs.isEmpty else { return s }
        let n = s.briefs.count
        let cursor = wrap ? ((s.cursor + delta) % n + n) % n : min(n - 1, max(0, s.cursor + delta))
        guard cursor != s.cursor else { return s }
        var r = s
        r.cursor = cursor; r.unseen = min(s.unseen, n - 1 - cursor)
        return r
    }

    /// Tapped, brief not arrived yet: greyed.
    public static func markTapped(_ s: MeetingSlice, _ topic: String) -> MeetingSlice {
        guard !s.tapped.contains(topic) else { return s }
        var n = s; n.tapped.append(topic); return n
    }

    /// Elapsed ms at `now` (pauses excluded): the clock runs only while recording.
    public static func elapsed(_ s: MeetingSlice, now: Date) -> UInt64 {
        guard s.state == .recording, let at = s.clockAt else { return s.elapsedMs }
        return s.elapsedMs + UInt64(max(0, now.timeIntervalSince(at)) * 1000)
    }

    /// `mm:ss` on the recording clock; minutes run past 59.
    public static func mmss(_ ms: UInt64) -> String {
        let s = ms / 1000
        return String(format: "%02d:%02d", Int(s / 60), Int(s % 60))
    }
}
