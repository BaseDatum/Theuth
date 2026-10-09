import AppKit
import CompanionCore
import TheuthKit

/// 149 W3 — the `meeting` slice: the stream's `MeetingEvent`s through `MeetingReducer`, the connect-time hydrate
/// (`meeting_status` + `meeting_briefs` + `meeting_live`, `meeting.ts:257-265`), and the island's RPC actions.
/// Probes (`AskFlags.automated`) log every action and send nothing.
extension Store {
    /// `Store.handle` passes every stream frame here; non-meeting types answer `nil` from `MeetingEvent.decode`.
    /// `TurnStarted`/`ToolCall*`/`TurnFinished` of the meeting's own session drive the activity spinner.
    func meetingObserve(_ data: Data) {
        if let e = try? MeetingEvent.decode(data) {
            meeting = MeetingReducer.reduce(meeting ?? .empty, e, now: Date())
            return
        }
        guard let s = meeting, let session = s.session,
              let t = try? TheuthJSON.decoder.decode(TurnActivity.self, from: data), t.session == session else { return }
        switch t.type {
        case "TurnStarted", "ToolCallFinished": meeting?.activity = "thinking…"
        case "ToolCallStarted": meeting?.activity = "\(t.name ?? "tool")…"
        case "TurnFinished": meeting?.activity = nil
        default: break
        }
    }

    private struct TurnActivity: Decodable { let type: String; let session: String?; let name: String? }

    /// Connect/reconnect: the status, then briefs + lines when a meeting is live. Errors (meetings disabled on this
    /// engine) leave the slice empty — the island stays hidden.
    func meetingHydrate() async {
        do {
            let status = try await client.meetingStatus()
            var s = MeetingReducer.seed(status, now: Date())
            if let id = status.meeting, status.state.isLive {
                async let b = client.meetingBriefs(id: id)
                async let l = client.meetingLive(id: id)
                s = MeetingReducer.hydrate(s, briefs: try await b, lines: try await l, cursor: status.cursor)
            }
            meeting = s
            IslandLog.write("hydrated meeting=\(status.meeting ?? "none") state=\(status.state) topics=\(s.topics.count) briefs=\(s.briefs.count) lines=\(s.lines.count)")
        } catch {
            IslandLog.write("meeting_status failed: \(error)")
        }
    }

    // MARK: actions (§3.6 "Meeting flows"; every RPC exists, api.rs:1558-1611)

    func meetingTap(_ topic: String) {
        meeting = meeting.map { MeetingReducer.markTapped($0, topic) }
        meetingAct("meeting_tap(\(topic))") { try await $0.meetingTap(topic: topic) }
    }

    func meetingDismiss(_ topic: String) { meetingAct("meeting_dismiss(\(topic))") { try await $0.meetingDismiss(topic: topic) } }
    func meetingDeeper(_ brief: String) { meetingAct("meeting_deeper(\(brief))") { try await $0.meetingDeeper(brief: brief) } }
    func meetingRequest(_ text: String) { meetingAct("meeting_request(\(text.count) chars)") { try await $0.meetingRequest(text: text) } }
    func meetingPtt(_ held: Bool) { meetingAct("meeting_ptt(\(held))") { try await $0.meetingPtt(held: held) } }
    func meetingPause() { meetingAct("meeting_pause") { try await $0.meetingPause() } }
    func meetingResume() { meetingAct("meeting_resume") { try await $0.meetingResume() } }
    func meetingStop() { meetingAct("meeting_stop") { try await $0.meetingStop() } }

    /// Step the brief cursor (the island controller mirrors every cursor change with `meeting_cursor`).
    func meetingMove(_ delta: Int) {
        guard let s = meeting else { return }
        let n = MeetingReducer.move(s, delta)
        if n != s { meeting = n }
    }

    func meetingCursor(_ brief: String?) { meetingAct("meeting_cursor(\(brief ?? "null"))") { try await $0.meetingCursor(brief: brief) } }

    private func meetingAct(_ label: String, _ op: @escaping (RpcClient) async throws -> Void) {
        guard !AskFlags.automated else { meetingActs.append(label); return IslandLog.write("probe: \(label) suppressed") }
        Task { [client] in
            do { try await op(client!) } catch { IslandLog.write("\(label) failed: \(error)") }
        }
    }
}

/// stderr plus `/tmp/149-W3-island.log` (or `--probe-log <path>`) during an island/eye probe.
enum IslandLog {
    static let file = URL(fileURLWithPath: Flags.probeLog ?? "/tmp/149-W3-island.log")

    static func write(_ s: String) {
        Log.write(s)
        guard Flags.probeIsland != nil || Flags.probeEye else { return }
        let line = "[companion \(Date()) pid \(getpid())] \(s)\n"
        if let h = try? FileHandle(forWritingTo: file) {
            h.seekToEndOfFile(); h.write(Data(line.utf8)); try? h.close()
        } else {
            try? Data(line.utf8).write(to: file)
        }
    }
}

/// `--probe-island <closed|open|detached|nonotch>`: a fixture meeting (two topics + a brief), the island shown in that
/// state for N s (`--probe-seconds`, default 6) without key, a saved frame restored from this display's UUID and an
/// off-screen one rejected; quit 0. `--probe-island <clicks|stop|open-no-hover>[-nonotch]` (149i, `IslandProbe.swift`): synthetic
/// input against the probe's own island. `--probe-eye`: the island up with `sharingType = .none` for the shell's capture
/// test (§9/§10). All `--shareable`-aware, all log to `/tmp/149-W3-island.log` or `--probe-log <path>`.
extension Flags {
    static let probeIsland: String? = {
        guard let i = args.firstIndex(of: "--probe-island"), i + 1 < args.count else { return nil }
        return args[i + 1]
    }()
    static let probeEye = args.contains("--probe-eye")
    static let probeSeconds: TimeInterval = {
        guard let i = args.firstIndex(of: "--probe-seconds"), i + 1 < args.count else { return 6 }
        return TimeInterval(args[i + 1]) ?? 6
    }()
    static let probeLog: String? = {
        guard let i = args.firstIndex(of: "--probe-log"), i + 1 < args.count else { return nil }
        return args[i + 1]
    }()
    static var islandProbe: Bool { probeIsland != nil || probeEye }
}
