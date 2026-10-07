import Foundation

/// JSON-RPC 2.0 over one `URLSessionWebSocketTask` (jsonrpsee framing, mirrors `UI/rpc/client.ts`):
/// positional params, numeric ids, `subscribe_events` items as notifications keyed by `params.subscription`.
/// Lost sockets are redialled with capped exponential backoff (1 s → 30 s) and the event subscription is
/// re-issued on every open, so the first item (`PendingPrompts`) arrives again — callers resync on it.
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

    private let request: URLRequest
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

    public init(location: EngineLocation, onState: @escaping @Sendable (State) -> Void,
                onEvent: @escaping @Sendable (Data) -> Void) {
        var r = URLRequest(url: location.url)
        r.setValue("Bearer \(location.token)", forHTTPHeaderField: "Authorization")
        r.setValue("companion", forHTTPHeaderField: "X-Theuth-Client")
        request = r
        self.onState = onState
        self.onEvent = onEvent
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
        let t = session.webSocketTask(with: request)
        task = t
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

// MARK: the twelve calls the companion makes (params positional, in `crates/rpc/src/api.rs` trait order)

public extension RpcClient {
    func attentionList() async throws -> [AttentionItem] { try await call("attention_list") }

    func attentionSnooze(id: String, untilMs: UInt64) async throws {
        let _: JSONValue = try await call("attention_snooze", [.init(id), .init(untilMs)])
    }

    func attentionDismiss(id: String) async throws {
        let _: JSONValue = try await call("attention_dismiss", [.init(id)])
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

    /// `send_input(session, text, [], null, null)` → the session id.
    func sendInput(session: String, text: String) async throws -> String {
        try await call("send_input", [.init(session), .init(text), .init([String]()), .init(String?.none), .init(String?.none)])
    }
}
