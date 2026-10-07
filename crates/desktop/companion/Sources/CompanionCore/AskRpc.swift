import Foundation
import TheuthKit

/// `voice::Transcript` (`crates/voice/src/lib.rs:103-108`).
public struct Transcript: Codable, Equatable, Sendable {
    public var text: String
    public var durationMs: UInt64
    public init(text: String, durationMs: UInt64) { self.text = text; self.durationMs = durationMs }
}

// MARK: the Ask bar's calls (W3), positional, in `crates/rpc/src/api.rs` order

public extension RpcClient {
    /// `steer(session, text, null, [], null)` → the input id (`api.rs:207-215`).
    func steer(session: String, text: String) async throws -> String {
        try await call("steer", [.init(session), .init(text), .init(String?.none), .init([String]()), .init(String?.none)])
    }

    /// `put_attachment(base64, ext)` → `<data_dir>/attachments/<sha>.<ext>` (`api.rs:1186-1191`).
    func putAttachment(_ bytes: Data, ext: String) async throws -> String {
        try await call("put_attachment", [.init(bytes.base64EncodedString()), .init(ext)])
    }

    /// `transcribe(path)` for a file `put_attachment` stored (`api.rs:1240-1244`).
    func transcribe(path: String) async throws -> Transcript {
        try await call("transcribe", [.init(path)])
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
}
