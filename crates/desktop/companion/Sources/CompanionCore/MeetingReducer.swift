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
    /// 158 §5: the island's prompt state — `.detected` (A/B) or `.upcoming` (C) before a meeting, `.stop` (D) during one.
    public var prompt: Prompt?
    /// "Keep recording" answered at this `releasedForS`: the engine re-sends `AppReleased` every 30 s with a growing count,
    /// and those are muted; a smaller count is a fresh release episode and asks again.
    public var keptReleasedForS: UInt32?
    /// "Dismiss" answered for this event id: the engine's one-minute `Upcoming` re-sends of it stay muted.
    public var dismissedUpcoming: String?
    /// 173: both capture tracks' health (`Capture` events / `meeting_status.capture`); all `.ok` outside a recording.
    public var capture: CaptureStatus = .ok
    /// The tapped app's name (`Started.process`); `nil` after a reconnect — `meeting_status` has none.
    public var app: String?

    public enum Prompt: Equatable, Sendable {
        case detected(Detection)
        /// 158 §9.4 / §5.2 (c): the event starts within 5 min and its pre-meeting brief (bullets, may be empty).
        case upcoming(event: EventRef, project: String?, brief: [String])
        case stop(app: MeetingProcess, releasedForS: UInt32)
    }

    public static let empty = MeetingSlice(meeting: nil, session: nil, state: .idle, degraded: [], elapsedMs: 0, clockAt: nil,
                                           topics: [], tapped: [], briefs: [], cursor: -1, unseen: 0, listening: false,
                                           approvalWaiting: false, progress: nil, lines: [], deciding: nil, oracle: [], level: nil)
    public static let linesKept = 200
    public static let oracleKept = 8

    public init(meeting: String?, session: String?, state: MeetingState, degraded: [Degradation], elapsedMs: UInt64, clockAt: Date?,
                topics: [Topic], tapped: [String], briefs: [Brief], cursor: Int, unseen: Int, listening: Bool, approvalWaiting: Bool,
                progress: Progress?, lines: [Line], deciding: String?, oracle: [OracleDecision], level: Level?, activity: String? = nil,
                prompt: Prompt? = nil, keptReleasedForS: UInt32? = nil, dismissedUpcoming: String? = nil) {
        self.meeting = meeting; self.session = session; self.state = state; self.degraded = degraded; self.elapsedMs = elapsedMs
        self.clockAt = clockAt; self.topics = topics; self.tapped = tapped; self.briefs = briefs; self.cursor = cursor
        self.unseen = unseen; self.listening = listening; self.approvalWaiting = approvalWaiting; self.progress = progress
        self.lines = lines; self.deciding = deciding; self.oracle = oracle; self.level = level; self.activity = activity
        self.prompt = prompt; self.keptReleasedForS = keptReleasedForS; self.dismissedUpcoming = dismissedUpcoming
    }

    public var isLive: Bool { state.isLive }
    public var current: Brief? { briefs.indices.contains(cursor) ? briefs[cursor] : nil }
}

public enum MeetingReducer {
    /// Pure transition, one case per `MeetingEvent` variant (`meeting.ts:156-220`).
    public static func reduce(_ s: MeetingSlice, _ e: MeetingEvent, now: Date) -> MeetingSlice {
        var n = s
        switch e {
        case .started(let meeting, let session, let process, _):
            n = .empty
            n.meeting = meeting; n.session = session; n.app = process.name; n.state = .recording; n.clockAt = now
        case .state(let meeting, let state, let degraded):
            if s.meeting != nil, s.meeting != meeting { return s }
            n.meeting = meeting; n.state = state; n.degraded = degraded
            n.elapsedMs = elapsed(s, now: now); n.clockAt = now
            // The cues end with the recording.
            n.listening = state.isLive && s.listening
            n.approvalWaiting = state.isLive && s.approvalWaiting
            if !state.isLive { n.activity = nil; n.capture = .ok; if case .stop? = s.prompt { n.prompt = nil } } // the meeting ended: a "Stop recording?" has nothing to stop
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
        case .capture(let meeting, let capture):
            if s.meeting != meeting { return s }
            n.capture = capture
        case .oracleDeciding(let meeting, let batchId):
            if s.meeting != meeting { return s }
            n.deciding = batchId
        case .oracleDecision(let meeting, let decision):
            if s.meeting != meeting { return s }
            if let i = s.oracle.firstIndex(where: { $0.batchId == decision.batchId }) { n.oracle[i] = decision }
            else { n.oracle = Array((s.oracle + [decision]).suffix(MeetingSlice.oracleKept)) }
            if s.deciding == decision.batchId { n.deciding = nil }
        case .detected(var d):
            // §5.2 (c) "above (b) when both apply": a shown Upcoming for the same event lends its brief and project.
            if case .upcoming(let e, let p, let b)? = s.prompt, d.event?.event.id == e.id { lend(&d, brief: b, project: p) }
            n.prompt = .detected(d)
        case .detectionCleared(let appId, _):
            if case .detected(let d)? = s.prompt, d.appId == appId { n.prompt = nil }
        case .appReleased(let meeting, let app, let releasedForS):
            guard s.meeting == meeting, s.isLive else { return s }
            if let kept = s.keptReleasedForS, releasedForS > kept { return s } // a re-send of the episode the user kept
            n.prompt = .stop(app: app, releasedForS: releasedForS); n.keptReleasedForS = nil
        case .upcoming(let event, let project, let brief):
            // §9.4: never during a meeting, not once dismissed, not past start + 5 min.
            guard !IslandModel.live(s), s.dismissedUpcoming != event.id, !upcomingExpired(event, now: now) else { return s }
            if case .detected(var d)? = s.prompt {
                guard d.event?.event.id == event.id else { return s } // an unrelated prompt keeps the pane
                lend(&d, brief: brief, project: project)
                n.prompt = .detected(d)
            } else {
                n.prompt = .upcoming(event: event, project: project, brief: brief ?? [])
            }
        }
        return n
    }

    /// `meeting_status` at connect → the slice (`hydrate` adds the briefs and lines).
    public static func seed(_ status: MeetingStatus, now: Date) -> MeetingSlice {
        var n = MeetingSlice.empty
        n.meeting = status.meeting; n.session = status.session; n.state = status.state; n.degraded = status.degraded
        n.elapsedMs = status.elapsedMs; n.clockAt = now; n.topics = status.topics; n.approvalWaiting = status.approvalWaiting
        n.capture = status.capture
        n.prompt = status.detection.map { .detected($0) }
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

    /// "Keep recording": the prompt goes, the engine's 30 s re-sends of this release stay muted (`keptReleasedForS`).
    public static func keepRecording(_ s: MeetingSlice) -> MeetingSlice {
        guard case .stop(_, let releasedForS)? = s.prompt else { return s }
        var n = s; n.prompt = nil; n.keptReleasedForS = releasedForS; return n
    }

    /// "Dismiss" on the Upcoming brief: local (no RPC), the engine's minute re-sends of this event stay muted.
    public static func dismissUpcoming(_ s: MeetingSlice) -> MeetingSlice {
        guard case .upcoming(let e, _, _)? = s.prompt else { return s }
        var n = s; n.prompt = nil; n.dismissedUpcoming = e.id; return n
    }

    /// The clock tick: island (c) clears at `start + 5 min` (§9.4). Unchanged slice when nothing expired.
    public static func tick(_ s: MeetingSlice, now: Date) -> MeetingSlice {
        guard case .upcoming(let e, _, _)? = s.prompt, upcomingExpired(e, now: now) else { return s }
        var n = s; n.prompt = nil; return n
    }

    public static let upcomingLinger: TimeInterval = 300

    static func upcomingExpired(_ e: EventRef, now: Date) -> Bool {
        guard let start = date(e.start) else { return false }
        return now >= start + upcomingLinger
    }

    /// The engine's own `Detection.brief`/`project` win; an absent or empty brief takes the Upcoming's.
    private static func lend(_ d: inout Detection, brief: [String]?, project: String?) {
        if d.brief?.isEmpty != false, let b = brief, !b.isEmpty { d.brief = b }
        d.project = d.project ?? project
    }

    /// The prompt's event line (158 §5.2 b): `<Title> · <hh:mm>–<hh:mm>`; a start or end that is not RFC 3339 drops the times.
    public static func eventLine(_ e: EventRef) -> String {
        guard let a = date(e.start), let b = date(e.end) else { return e.title }
        let f = Date.FormatStyle(date: .omitted, time: .shortened)
        return "\(e.title) · \(a.formatted(f))–\(b.formatted(f))"
    }

    /// Island (c)'s headline (the mock): `In N min · <Title>`, minutes rounded up; `Now · <Title>` from the start on.
    public static func upcomingLine(_ e: EventRef, now: Date) -> String {
        guard let start = date(e.start) else { return e.title }
        let mins = Int((start.timeIntervalSince(now) / 60).rounded(.up))
        return mins >= 1 ? "In \(mins) min · \(e.title)" : "Now · \(e.title)"
    }

    /// Island (c)'s source line: `From the last meetings in <project> · attendees A, B` (others only, `isMe` dropped);
    /// `nil` with neither a project nor attendees.
    public static func upcomingSource(project: String?, attendees: [Attendee]) -> String? {
        var parts: [String] = []
        if let p = project { parts.append("From the last meetings in \(p)") }
        let names = attendees.filter { !$0.isMe }.map(\.name)
        if !names.isEmpty { parts.append("attendees \(names.joined(separator: ", "))") }
        return parts.isEmpty ? nil : parts.joined(separator: " · ")
    }

    /// chrono's RFC 3339, with or without fractional seconds (as `TheuthJSON.decoder` accepts).
    public static func date(_ rfc3339: String) -> Date? {
        let fractional = ISO8601DateFormatter(), plain = ISO8601DateFormatter()
        fractional.formatOptions = [.withInternetDateTime, .withFractionalSeconds]
        return fractional.date(from: rfc3339) ?? plain.date(from: rfc3339)
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

    /// Local `HH:MM` of an RFC 3339 stamp (`meeting.ts` `hhmm`); the stamp itself when it does not parse.
    public static func hhmm(_ rfc3339: String) -> String {
        guard let d = date(rfc3339) else { return rfc3339 }
        let f = DateFormatter()
        f.locale = Locale(identifier: "en_US_POSIX"); f.dateFormat = "HH:mm"
        return f.string(from: d)
    }

    /// One track's line of `captureWarning`; `nil` while it is heard.
    private static func trackWarning(_ h: CaptureHealth, source: String, device: String, permission: String) -> String? {
        switch h {
        case .ok: return nil
        case .recovering(_, let since): return "Not hearing \(source) since \(hhmm(since)) — reconnecting…"
        case .lost(.permissionDenied, let since):
            return "Not hearing \(source) since \(hhmm(since)) — \(permission) permission is off (System Settings → Privacy & Security). Retrying…"
        case .lost(_, let since):
            return "Not hearing \(source) since \(hhmm(since)). Still silent — check the \(device) device or \(permission) permission. Retrying…"
        case .degraded: return "\(source)'s audio tap failed; recording all system audio instead"
        }
    }

    /// 173 (`meeting.ts` `captureWarning`): what the user should read about the capture — the system track first, then the
    /// mic; `nil` when both are heard. `app`: the tapped app's name, `nil` when unknown.
    public static func captureWarning(_ capture: CaptureStatus, app: String?) -> String? {
        let parts = [
            trackWarning(capture.system, source: app ?? "the meeting app", device: "output", permission: "System Audio Recording"),
            trackWarning(capture.mic, source: "your microphone", device: "input", permission: "Microphone"),
        ].compactMap { $0 }
        return parts.isEmpty ? nil : parts.joined(separator: " · ")
    }

    /// 173 (`Island.tsx` `captureBadge`): the strip's word for the capture health, the system track first; `nil` when both are heard.
    public static func captureBadge(_ c: CaptureStatus) -> String? {
        switch c.system {
        case .lost(.permissionDenied, _): return "no system audio permission"
        case .lost: return "system audio lost"
        case .recovering: return "reconnecting audio"
        case .degraded: return "system audio fallback"
        case .ok: break
        }
        switch c.mic {
        case .lost(.permissionDenied, _): return "no mic permission"
        case .lost: return "mic lost"
        case .recovering: return "reconnecting mic"
        case .degraded, .ok: return nil
        }
    }

    /// 173 — the collapsed pill's word while a track is not heard, the system track first; `nil` when both are heard.
    public static func capturePill(_ c: CaptureStatus, app: String?) -> String? {
        switch c.system {
        case .recovering, .lost: return "Not hearing \(app ?? "the meeting app")"
        case .degraded: return "Recording all system audio"
        case .ok: break
        }
        switch c.mic {
        case .recovering, .lost, .degraded: return "Not hearing your microphone"
        case .ok: return nil
        }
    }

    /// `Island.tsx` `badge`: a degradation's strip word. A denied permission is `nil` — the capture badge carries the live
    /// permission state and clears itself; the degradation list keeps it until the next `State`.
    public static func badge(_ d: Degradation) -> String? {
        switch d {
        case .micNoAec: return "headphones recommended"
        case .modelDown: return "model down"
        case .diskLow: return "disk low"
        case .nearMaxLength: return "10 min left"
        case .sttDown: return "transcription down"
        case .permissionDenied: return nil
        case .other("NoNote"): return "no note"
        case .other("DiarizationDown"): return "speaker labels down"
        case .other: return nil
        }
    }

    /// The live strip's badges: the degradations, the capture word, then the cues.
    public static func stripBadges(_ s: MeetingSlice) -> [String] {
        s.degraded.compactMap(badge) + (captureBadge(s.capture).map { [$0] } ?? [])
            + (s.approvalWaiting ? ["approval waiting"] : []) + (s.listening ? ["listening…"] : [])
    }
}
