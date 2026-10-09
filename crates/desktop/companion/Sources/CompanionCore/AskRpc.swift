import Foundation
import TheuthKit

// MARK: the Ask bar's calls (W3), positional, in `crates/rpc/src/api.rs` order

public extension RpcClient {
    /// `steer(session, text, null, [], null)` → the input id (`api.rs:207-215`).
    func steer(session: String, text: String) async throws -> String {
        try await call("steer", [.init(session), .init(text), .init(String?.none), .init([String]()), .init(String?.none)])
    }

    func meshStatus() async throws -> MeshStatus { try await call("mesh_status") }

    /// `desk_open(name)` → the desk's session; refused off the hub (`api.rs:1692-1694`).
    func deskOpen(name: String) async throws -> String { try await call("desk_open", [.init(name)]) }

    /// `activity_list({since_ms, kinds, limit})`, newest first (`api.rs:1737-1742`).
    func activityList(_ filter: ActivityFilter) async throws -> [ActivityEntry] {
        try await call("activity_list", [.init(filter)])
    }

    /// `set_event_focus(sessions)`: delta-class events for these sessions only (129 §3.2; `api.rs:1862-1868`).
    func setEventFocus(_ sessions: [String]) async throws {
        let _: JSONValue = try await call("set_event_focus", [.init(sessions)])
    }

    // MARK: live dictation (W6; design 54 §8, `api.rs:1293-1325`)

    /// Load the streaming model once after connect; never opens a microphone. The reply is the model state.
    func dictationPrewarm() async throws {
        let _: JSONValue = try await call("dictation_prewarm")
    }

    /// `dictation_start(Some(16000))`: the companion streams its own mono i16 PCM. `None` would open the ENGINE's mic.
    func dictationStart(sampleRate: UInt32 = 16_000) async throws -> String {
        try await call("dictation_start", [.init(sampleRate)])
    }

    /// ~100 ms of little-endian mono i16 samples, base64.
    func dictationAudio(id: String, pcm16: [Int16]) async throws {
        let bytes = pcm16.withUnsafeBufferPointer { Data(buffer: $0) } // macOS is little-endian
        let _: JSONValue = try await call("dictation_audio", [.init(id), .init(bytes.base64EncodedString())])
    }

    /// Flushes the open utterance; `DictationFinished` follows as an event. Idempotent.
    func dictationStop(id: String) async throws {
        let _: JSONValue = try await call("dictation_stop", [.init(id)])
    }

    /// `dictation_status` → the active dictation's id, if any (the one-dictation slot; 149f's probe asserts it is free).
    func dictationActive() async throws -> String? {
        struct State: Decodable { let active: String? }
        let s: State = try await call("dictation_status")
        return s.active
    }
}
