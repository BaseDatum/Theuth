import Foundation

/// JSON-RPC 2.0 over one `URLSessionWebSocketTask` (jsonrpsee framing, mirrors `UI/rpc/client.ts`):
/// positional params, numeric ids, `subscribe_events` items as notifications keyed by `params.subscription`.
/// Lost sockets are redialled with capped exponential backoff (1 s → 30 s) and the event subscription is
/// re-issued on every open, so the first item (`PendingPrompts`) arrives again — callers resync on it.
/// The engine is located afresh on every dial (`locate`), so a companion launched before the engine has
/// written `engine.json`, or an engine restarted on a new port, is found on the next attempt.
public actor RpcClient {
    public enum State: Equatable, Sendable { case connecting, open, reconnecting, closed }

    public struct RpcError: Error, CustomStringConvertible {
        public let code: Int
        public let message: String
        public static let notConnected = RpcError(code: -32300, message: "not connected")
        public var description: String { "rpc \(code): \(message)" }
    }

    private struct Request: Encodable {
        let jsonrpc = "2.0"
        let id: Int
        let method: String
        let params: [AnyEncodable]
    }

    private let locate: @Sendable () -> EngineLocation?
    private let session = URLSession(configuration: .ephemeral)
    private let onState: @Sendable (State) -> Void
    private let onEvent: @Sendable (Data) -> Void
    private var task: URLSessionWebSocketTask?
    private var open = false
    private var closed = false
    private var attempt = 0
    private var nextId = 1
    private var pending: [Int: CheckedContinuation<Data, Error>] = [:]
    private var wantsEvents = false
    private var subscriptionId: String?

    /// A fixed location (tests, probes).
    public init(location: EngineLocation, onState: @escaping @Sendable (State) -> Void,
                onEvent: @escaping @Sendable (Data) -> Void) {
        self.init(locate: { location }, onState: onState, onEvent: onEvent)
    }

    /// `locate` runs before every dial; nil (no token yet) waits and retries like a lost socket.
    public init(locate: @escaping @Sendable () -> EngineLocation?, onState: @escaping @Sendable (State) -> Void,
                onEvent: @escaping @Sendable (Data) -> Void) {
        self.locate = locate
        self.onState = onState
        self.onEvent = onEvent
    }

    private static func request(for location: EngineLocation) -> URLRequest {
        var r = URLRequest(url: location.url)
        r.setValue("Bearer \(location.token)", forHTTPHeaderField: "Authorization")
        r.setValue("companion", forHTTPHeaderField: "X-Theuth-Client")
        return r
    }

    // MARK: lifecycle

    /// Dial; keeps redialling until `close()`.
    public func start() {
        closed = false
        connect()
    }

    public func close() {
        closed = true
        let t = task
        task = nil
        open = false
        t?.cancel(with: .goingAway, reason: nil)
        failPending(RpcError(code: -32300, message: "closed"))
        onState(.closed)
    }

    /// Ask for the event stream (`subscribe_events(null, null)`); issued now if open, and after every reconnect.
    public func subscribeEvents() {
        wantsEvents = true
        if open { Task { await self.issueSubscription() } }
    }

    private func connect() {
        guard !closed else { return }
        onState(attempt == 0 ? .connecting : .reconnecting)
        guard let location = locate() else { return retryLater() }
        let t = session.webSocketTask(with: Self.request(for: location))
        task = t
        // Default cap is 1 MiB; `list_sessions` alone passed 2 MB on a long-lived data dir, and a too-long
        // frame fails `receive` → `lost` → a reconnect loop.
        t.maximumMessageSize = 64 << 20
        t.resume()
        // No delegate: the first successful ping proves the upgrade.
        t.sendPing { [weak self] error in
            guard let self else { return }
            Task { await self.pinged(t, error: error) }
        }
    }

    private func pinged(_ t: URLSessionWebSocketTask, error: Error?) {
        guard task === t, !closed else { return }
        if error != nil { return lost(t) }
        open = true
        attempt = 0
        onState(.open)
        receive(t)
        if wantsEvents { Task { await self.issueSubscription() } }
    }

    private func receive(_ t: URLSessionWebSocketTask) {
        t.receive { [weak self] result in
            guard let self else { return }
            Task { await self.received(result, from: t) }
        }
    }

    private func received(_ result: Result<URLSessionWebSocketTask.Message, Error>, from t: URLSessionWebSocketTask) {
        guard task === t else { return }
        switch result {
        case .failure: lost(t)
        case .success(let m):
            switch m {
            case .data(let d): dispatch(d)
            case .string(let s): dispatch(Data(s.utf8))
            @unknown default: break
            }
            receive(t)
        }
    }

    private func lost(_ t: URLSessionWebSocketTask) {
        guard task === t else { return }
        task = nil
        open = false
        subscriptionId = nil
        t.cancel(with: .abnormalClosure, reason: nil)
        failPending(RpcError(code: -32300, message: "connection closed"))
        guard !closed else { return }
        retryLater()
    }

    /// Capped exponential backoff (1 s → 30 s), then dial again (re-locating the engine).
    private func retryLater() {
        onState(.reconnecting)
        let delay = min(30.0, pow(2.0, Double(attempt)))
        attempt += 1
        Task {
            try? await Task.sleep(for: .seconds(delay))
            await self.connect()
        }
    }

    private func failPending(_ e: Error) {
        let p = pending
        pending = [:]
        for c in p.values { c.resume(throwing: e) }
    }

    // MARK: requests

    /// One request; decodes the `result` with the wire codec.
    public func call<T: Decodable>(_ method: String, _ params: [AnyEncodable] = []) async throws -> T {
        try TheuthJSON.decoder.decode(T.self, from: try await raw(method, params))
    }

    private func raw(_ method: String, _ params: [AnyEncodable]) async throws -> Data {
        guard open, let t = task else { throw RpcError.notConnected }
        let id = nextId
        nextId += 1
        let body = try TheuthJSON.encoder.encode(Request(id: id, method: method, params: params))
        return try await withCheckedThrowingContinuation { cont in
            pending[id] = cont
            t.send(.string(String(decoding: body, as: UTF8.self))) { [weak self] error in
                guard let error, let self else { return }
                Task { await self.fail(id, error) }
            }
        }
    }

    private func fail(_ id: Int, _ e: Error) {
        pending.removeValue(forKey: id)?.resume(throwing: e)
    }

    private func issueSubscription() async {
        guard open, subscriptionId == nil else { return }
        do {
            let d = try await raw("subscribe_events", [AnyEncodable(String?.none), AnyEncodable(Bool?.none)])
            subscriptionId = Self.describe(try JSONSerialization.jsonObject(with: d, options: .fragmentsAllowed))
        } catch {
            // Lost mid-handshake: the reconnect path re-issues.
        }
    }

    private func dispatch(_ data: Data) {
        guard let obj = try? JSONSerialization.jsonObject(with: data) as? [String: Any] else { return }
        if let params = obj["params"] as? [String: Any], let sub = params["subscription"] {
            guard Self.describe(sub) == subscriptionId, let result = params["result"],
                  let d = try? JSONSerialization.data(withJSONObject: result, options: .fragmentsAllowed) else { return }
            onEvent(d)
            return
        }
        guard let id = obj["id"] as? Int, let cont = pending.removeValue(forKey: id) else { return }
        if let err = obj["error"] as? [String: Any] {
            cont.resume(throwing: RpcError(code: err["code"] as? Int ?? -1, message: err["message"] as? String ?? "error"))
        } else if let d = try? JSONSerialization.data(withJSONObject: obj["result"] ?? NSNull(), options: .fragmentsAllowed) {
            cont.resume(returning: d)
        } else {
            cont.resume(throwing: RpcError(code: -32700, message: "unreadable result"))
        }
    }

    private static func describe(_ v: Any) -> String { "\(v)" }
}

// MARK: the fourteen calls the companion makes (params positional, in `crates/rpc/src/api.rs` trait order)

public extension RpcClient {
    func attentionList() async throws -> [AttentionItem] { try await call("attention_list") }

    func attentionSnooze(id: String, untilMs: UInt64) async throws {
        let _: JSONValue = try await call("attention_snooze", [.init(id), .init(untilMs)])
    }

    func attentionDismiss(id: String) async throws {
        let _: JSONValue = try await call("attention_dismiss", [.init(id)])
    }

    /// `set_event_focus(sessions)` (129 §3.4; 155 §13.5): the sessions this connection has in view — the engine
    /// blanks their alerts in `attention_list` and marks a focused finish seen; `[]` = none. Per connection.
    func setEventFocus(sessions: [String]) async throws {
        let _: JSONValue = try await call("set_event_focus", [.init(sessions)])
    }

    func agentsList() async throws -> [AgentSummary] { try await call("agents_list") }

    func listActivity() async throws -> [AgentInfo] { try await call("list_activity") }

    func packetGet(key: String) async throws -> Packet? { try await call("packet_get", [.init(key)]) }

    /// `true` if the ask was still pending (E5: the engine's own stale check).
    func answerAsk(id: String, answers: [Answer]?) async throws -> Bool {
        try await call("answer_ask", [.init(id), .init(answers)])
    }

    func resolveApproval(id: String, decision: Decision) async throws -> Bool {
        try await call("resolve_approval", [.init(id), .init(decision)])
    }

    func taskDecide(decision: String, answer: String) async throws {
        let _: JSONValue = try await call("task_decide", [.init(decision), .init(answer)])
    }

    /// `state` is a `TaskState` word: `done` | `todo` | …
    func taskMove(id: String, state: String, note: String?) async throws {
        let _: JSONValue = try await call("task_move", [.init(id), .init(state), .init(note)])
    }

    /// `agent_approve(name)`: hire a proposed agent; the engine stamps the actor (E4).
    func agentApprove(name: String) async throws {
        let _: JSONValue = try await call("agent_approve", [.init(name)])
    }

    /// `agent_remove(name, note)`: decline a proposal (the note goes on its hire task) or delete an agent.
    func agentRemove(name: String, note: String?) async throws {
        let _: JSONValue = try await call("agent_remove", [.init(name), .init(note)])
    }

    /// `send_input(session, text, files, delivery, id)` (api.rs:194-203) → the session id. `files` are image paths
    /// the engine attaches (`screen_context`'s PNGs); `session == nil` lets the engine pick.
    func sendInput(session: String?, text: String, files: [String], delivery: Delivery?, id: String?) async throws -> String {
        try await call("send_input", [.init(session), .init(text), .init(files), .init(delivery), .init(id)])
    }

    /// The files-less form every pre-149 caller uses.
    func sendInput(session: String, text: String) async throws -> String {
        try await sendInput(session: session, text: text, files: [], delivery: nil, id: nil)
    }
}

// MARK: 149 — the calls the agents panel, the island and point-and-say make (api.rs order; positional)

public extension RpcClient {
    // agents panel (§3.2)
    func listSessions() async throws -> [SessionSummary] { try await call("list_sessions") }

    func sessionsStatus() async throws -> SessionsStatus { try await call("sessions_status") }

    /// `cancel(session, scope)` (api.rs:255-256): `.tree` stops the children too; `nil` = the engine's default (root).
    func cancel(session: String, scope: CancelScope?) async throws {
        let _: JSONValue = try await call("cancel", [.init(session), .init(scope)])
    }

    /// `agent_pause(name, paused)` (api.rs:2088-2089).
    func agentPause(name: String, paused: Bool) async throws {
        let _: JSONValue = try await call("agent_pause", [.init(name), .init(paused)])
    }

    /// `steer(session, text, id, files, delivery)` (api.rs:218-226) → the input id.
    func steer(session: String, text: String, id: String?, files: [String], delivery: Delivery?) async throws -> String {
        try await call("steer", [.init(session), .init(text), .init(id), .init(files), .init(delivery)])
    }

    // point-and-say (§3.7)
    /// `screen_context(req)` (api.rs:335-339): AX lines + the PNGs under the trail.
    func screenContext(_ req: ScreenContextReq) async throws -> ScreenContextReply {
        try await call("screen_context", [.init(req)])
    }

    /// `speak(session, text)` (api.rs:1467-1468) → the speech id; the sink is the engine host (Revision 1 row 10).
    func speak(session: String, text: String) async throws -> String {
        try await call("speak", [.init(session), .init(text)])
    }

    /// `stop_speaking(id)` (api.rs:1470-1471); `nil` stops whatever is speaking.
    func stopSpeaking(id: String?) async throws {
        let _: JSONValue = try await call("stop_speaking", [.init(id)])
    }

    /// `decision_mode(prefix)` (api.rs:1655-1656).
    func decisionMode(prefix: String) async throws -> ModeInfo { try await call("decision_mode", [.init(prefix)]) }

    // meeting (§3.6; api.rs:1558-1611)
    func meetingPause() async throws { let _: JSONValue = try await call("meeting_pause") }

    func meetingResume() async throws { let _: JSONValue = try await call("meeting_resume") }

    func meetingStop() async throws { let _: JSONValue = try await call("meeting_stop") }

    func meetingStatus() async throws -> MeetingStatus { try await call("meeting_status") }

    func meetingPermissions() async throws -> MeetingPermissions { try await call("meeting_permissions") }

    func meetingTap(topic: String) async throws { let _: JSONValue = try await call("meeting_tap", [.init(topic)]) }

    func meetingDismiss(topic: String) async throws { let _: JSONValue = try await call("meeting_dismiss", [.init(topic)]) }

    func meetingDeeper(brief: String) async throws { let _: JSONValue = try await call("meeting_deeper", [.init(brief)]) }

    func meetingRequest(text: String) async throws { let _: JSONValue = try await call("meeting_request", [.init(text)]) }

    func meetingPtt(held: Bool) async throws { let _: JSONValue = try await call("meeting_ptt", [.init(held)]) }

    /// `meeting_cursor(brief)`: where the island says the user is reading; `nil` = none.
    func meetingCursor(brief: String?) async throws { let _: JSONValue = try await call("meeting_cursor", [.init(brief)]) }

    func meetingBriefs(id: String) async throws -> [Brief] { try await call("meeting_briefs", [.init(id)]) }

    func meetingTopics() async throws -> [Topic] { try await call("meeting_topics") }

    func meetingLive(id: String) async throws -> [Utterance] { try await call("meeting_live", [.init(id)]) }

    // 158 (C158 contract; W1/W2 call these). The engine answers inertly until the waves land.
    /// `meeting_start(StartMeeting)` → the meeting and session ids; the island's "Start recording" (158 §5.4).
    func meetingStart(_ start: StartMeeting) async throws -> MeetingStarted { try await call("meeting_start", [.init(start)]) }

    /// `meeting_detection()` (158 §4.5): the current prompt, `nil` when none.
    func meetingDetection() async throws -> Detection? { try await call("meeting_detection") }

    /// `meeting_detection_dismiss(app_id, never)` (158 §4.4): "Not now" / "Never for <App>".
    func meetingDetectionDismiss(appId: String, never: Bool) async throws {
        let _: JSONValue = try await call("meeting_detection_dismiss", [.init(appId), .init(never)])
    }

    /// `meeting_detection_set_event(event)` (158 §8.2): pick or clear (`nil`) the prompt's event.
    func meetingDetectionSetEvent(_ event: String?) async throws -> Detection? {
        try await call("meeting_detection_set_event", [.init(event)])
    }

    /// `calendar_push(source, window, events)` (158 §7.2, companion only) → rows written.
    func calendarPush(source: String, window: (String, String), events: [CalEvent]) async throws -> UInt32 {
        try await call("calendar_push", [.init(source), .init([window.0, window.1]), .init(events)])
    }

    /// `calendar_source_state(source, state)` (158 §7.2, companion only): `granted` / `denied` / ….
    func calendarSourceState(source: String, state: String) async throws {
        let _: JSONValue = try await call("calendar_source_state", [.init(source), .init(state)])
    }
}

/// `Started` (crates/meeting/src/lib.rs): `meeting_start`'s reply.
public struct MeetingStarted: Codable, Equatable, Sendable {
    public var meeting: String
    public var session: String
}
