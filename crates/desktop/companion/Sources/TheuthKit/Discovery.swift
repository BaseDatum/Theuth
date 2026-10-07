import Foundation

/// Where the local engine is, the way `DESK/launch.rs` probes it: `<data_dir>/engine.json` → `listen`,
/// else `127.0.0.1:7777`; the token at `<data_dir>/token`; `THEUTH_DATA_DIR` overrides `~/.theuth`.
public struct EngineLocation: Equatable, Sendable {
    public var url: URL
    public var token: String

    public static func dataDir(env: [String: String] = ProcessInfo.processInfo.environment) -> URL {
        if let d = env["THEUTH_DATA_DIR"], !d.isEmpty { return URL(fileURLWithPath: d) }
        return FileManager.default.homeDirectoryForCurrentUser.appendingPathComponent(".theuth")
    }

    public enum Error: Swift.Error { case noToken(URL) }

    private struct Record: Decodable { let listen: String? }

    public static func discover(dataDir: URL = dataDir()) throws -> EngineLocation {
        let token = try? String(contentsOf: dataDir.appendingPathComponent("token"), encoding: .utf8)
        guard let token = token?.trimmingCharacters(in: .whitespacesAndNewlines), !token.isEmpty else {
            throw Error.noToken(dataDir)
        }
        let record = (try? Data(contentsOf: dataDir.appendingPathComponent("engine.json")))
            .flatMap { try? JSONDecoder().decode(Record.self, from: $0) }
        let listen = record?.listen ?? "127.0.0.1:7777"
        return EngineLocation(url: URL(string: "ws://\(listen)/")!, token: token)
    }
}
