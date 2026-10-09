import Combine
import Foundation
import TheuthKit

/// 149s: spoken replies are two opt-in toggles (S1) — "Voice key — speak the answer aloud" (`voiceKeySpeak`, was on)
/// and "Ask theuth — speak the reply aloud" (`askSpeak`, new). Both read `false` when the key was never set:
/// `bool(forKey:)` is `false` for a missing key, and a user who set the old toggle explicitly keeps that value.
public enum SpokenReplies {
    public static let voiceKeyKey = "voiceKeySpeak"
    public static let askKey = "askSpeak"

    public static func voiceKey(_ d: UserDefaults) -> Bool { d.bool(forKey: voiceKeyKey) }
    public static func ask(_ d: UserDefaults) -> Bool { d.bool(forKey: askKey) }

    /// S3: speak once the reply is final — never a partial, never an empty or failed turn — and only when the
    /// surface's toggle is on. Both surfaces collect their reply in an `AskReply`.
    public static func shouldSpeak(enabled: Bool, reply: AskReply) -> Bool {
        enabled && reply.finished && reply.failure == nil && !reply.text.isEmpty
    }
}

/// The engine's `speak` / `stop_speaking` pair as the Speaker needs it (`RpcClient` conforms; tests record calls).
public protocol SpeechClient: Sendable {
    func speak(session: String, text: String) async throws -> String
    func stopSpeaking(id: String?) async throws
}

extension RpcClient: SpeechClient {}

/// The player: `expect` claims the next `SpeechStarted` on a session; `stop` ends playback at once and reports it ended.
@MainActor
public protocol SpeechSink: AnyObject {
    func expect(session: String)
    func stop()
}

/// S3: ONE "currently speaking" for both surfaces. `speak` ends whatever is playing (whoever started it), then asks
/// the engine for the utterance; `stop` ends it and tells the engine; an id that lands after a `stop` is stopped
/// at once. `surface` is who is being spoken for (requested or playing) — the surface's glyph binds to it.
@MainActor
public final class Speaker: ObservableObject {
    public enum Surface: Equatable, Sendable { case voiceKey, askBar }

    public let sink: SpeechSink
    /// Set when the connection opens.
    public var client: SpeechClient?
    @Published public private(set) var surface: Surface?
    /// The engine's speech id, once its reply lands.
    public private(set) var id: String?
    /// Bumped by every `speak`/`stop`: a `speak` reply from an earlier generation belongs to an utterance already stopped.
    private var generation = 0
    public var log: ((String) -> Void)?

    public init(sink: SpeechSink) { self.sink = sink }

    @discardableResult
    public func speak(_ text: String, on session: String, from surface: Surface) -> Task<Void, Never> {
        stop()
        generation += 1
        let gen = generation
        self.surface = surface
        sink.expect(session: session)
        log?("speak(\(surface)) \(text.count) chars on \(session.prefix(8))…")
        return Task { [weak self] in
            guard let self, let client = self.client else { self?.quiet(); return }
            do {
                let id = try await client.speak(session: session, text: text)
                if gen != self.generation { try? await client.stopSpeaking(id: id); return } // stopped before the id came back
                self.id = id
                log?("speak → \(id)")
            } catch {
                log?("speak failed: \(error)")
                if gen == self.generation { quiet() }
            }
        }
    }

    /// Esc, a new ask, a new gesture, or the other surface starting: silence now, and the engine's utterance with it.
    public func stop() {
        generation += 1
        let stopped = id
        quiet()
        guard let stopped, let client else { return }
        log?("stop \(stopped)")
        Task { try? await client.stopSpeaking(id: stopped) }
    }

    /// The sink's playback ended on its own (`SpeechFinished` and the tail played, or the sink was stopped).
    public func ended(_ id: String) {
        guard surface != nil, self.id == nil || self.id == id else { return }
        generation += 1 // a late `speak` reply is for an utterance that already ended
        quiet()
    }

    private func quiet() {
        id = nil
        if surface != nil { surface = nil }
        sink.stop()
    }
}
