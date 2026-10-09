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

    public static let empty = MeetingSlice(meeting: nil, session: nil, state: .idle, degraded: [], elapsedMs: 0, clockAt: nil,
                                           topics: [], tapped: [], briefs: [], cursor: -1, unseen: 0, listening: false,
                                           approvalWaiting: false, progress: nil, lines: [], deciding: nil, oracle: [], level: nil)
    public static let linesKept = 200
    public static let oracleKept = 8

    public init(meeting: String?, session: String?, state: MeetingState, degraded: [Degradation], elapsedMs: UInt64, clockAt: Date?,
                topics: [Topic], tapped: [String], briefs: [Brief], cursor: Int, unseen: Int, listening: Bool, approvalWaiting: Bool,
                progress: Progress?, lines: [Line], deciding: String?, oracle: [OracleDecision], level: Level?) {
        self.meeting = meeting; self.session = session; self.state = state; self.degraded = degraded; self.elapsedMs = elapsedMs
        self.clockAt = clockAt; self.topics = topics; self.tapped = tapped; self.briefs = briefs; self.cursor = cursor
        self.unseen = unseen; self.listening = listening; self.approvalWaiting = approvalWaiting; self.progress = progress
        self.lines = lines; self.deciding = deciding; self.oracle = oracle; self.level = level
    }

    public var isLive: Bool { state.isLive }
    public var current: Brief? { briefs.indices.contains(cursor) ? briefs[cursor] : nil }
}

public enum MeetingReducer {
    /// Pure transition, one case per `MeetingEvent` variant.
    public static func reduce(_ s: MeetingSlice, _ e: MeetingEvent, now: Date) -> MeetingSlice {
        fatalError("149 W3")
    }

    /// `meeting_status` at connect → the slice (the stream fills the rest).
    public static func seed(_ status: MeetingStatus, now: Date) -> MeetingSlice {
        fatalError("149 W3")
    }

    /// Elapsed ms at `now` (pauses excluded).
    public static func elapsed(_ s: MeetingSlice, now: Date) -> UInt64 {
        fatalError("149 W3")
    }
}
