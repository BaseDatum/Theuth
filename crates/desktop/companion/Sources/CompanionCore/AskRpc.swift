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

    /// `dictation_start(Some(16000), opts)`: the companion streams its own mono i16 PCM. `None` would open the ENGINE's mic.
    /// 153 §14.2: `opts` is the trailing positional param (`null` = today's behaviour: `Surface::Composer`, `AgentPrompt`).
    func dictationStart(sampleRate: UInt32 = 16_000, opts: DictationOpts? = nil) async throws -> String {
        try await call("dictation_start", [.init(sampleRate), .init(opts)])
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

    // MARK: 153 — dictation, dictionary, polish (`api.rs:1594-1684`, `:462-466`; positional, in trait order)

    /// `dictation_status` → the whole state (`input_device`, `last` timings) for the debug row.
    func dictationStatus() async throws -> DictationState { try await call("dictation_status") }

    /// `dictation_correction(id, sent_text)` (§6.1): what actually went out after dictation `id`.
    func dictationCorrection(id: String, sentText: String) async throws {
        let _: JSONValue = try await call("dictation_correction", [.init(id), .init(sentText)])
    }

    /// `dictation_history(limit)` (§6.7), newest first.
    func dictationHistory(limit: UInt32?) async throws -> [HistoryRow] {
        try await call("dictation_history", [.init(limit)])
    }

    /// `dictation_history_forget(id)`: one row, or everything when `nil`.
    func dictationHistoryForget(id: String?) async throws {
        let _: JSONValue = try await call("dictation_history_forget", [.init(id)])
    }

    /// `dictation_audio_get(id, utterance)` → base64 i16 16 kHz; throws "no audio" when evicted.
    func dictationAudioGet(id: String, utterance: UInt32?) async throws -> String {
        try await call("dictation_audio_get", [.init(id), .init(utterance)])
    }

    /// `dictionary_list(scope)` (§5.6): user + learned entries.
    func dictionaryList(scope: Scope?) async throws -> [Entry] { try await call("dictionary_list", [.init(scope)]) }

    /// `dictionary_put(entry)`: upsert by canonical + scope → the stored entry.
    func dictionaryPut(_ entry: Entry) async throws -> Entry { try await call("dictionary_put", [.init(entry)]) }

    /// `dictionary_delete(id)` → `false` when unknown.
    func dictionaryDelete(id: EntryId) async throws -> Bool { try await call("dictionary_delete", [.init(id)]) }

    /// `dictionary_suggestion_answer(id, answer)` (§6.3 step 4).
    func dictionarySuggestionAnswer(id: String, answer: SuggestionAnswer) async throws {
        let _: JSONValue = try await call("dictionary_suggestion_answer", [.init(id), .init(answer)])
    }

    /// `list_input_devices()` (§8.1).
    func listInputDevices() async throws -> [InputDevice] { try await call("list_input_devices") }

    /// `set_input_devices(names)`: the ordered preference; `[]` = system default only.
    func setInputDevices(_ names: [String]) async throws {
        let _: JSONValue = try await call("set_input_devices", [.init(names)])
    }

    /// `set_shape(surface, preset)` (§4.5).
    func setShape(surface: Surface, preset: ShapePreset) async throws {
        let _: JSONValue = try await call("set_shape", [.init(surface), .init(preset)])
    }

    /// `set_end_phrases(phrases)` (§7.2).
    func setEndPhrases(_ phrases: EndPhrases) async throws {
        let _: JSONValue = try await call("set_end_phrases", [.init(phrases)])
    }

    /// `polish_model()` (§9.2): `nil` = the chain (title model, then session).
    func polishModel() async throws -> ModelInfo? { try await call("polish_model") }

    /// `set_polish_model(model)`: persists `voice.polish_model`; `nil` clears it.
    func setPolishModel(_ model: String?) async throws -> ModelInfo? { try await call("set_polish_model", [.init(model)]) }

    /// `polish(text, destination, session)` (§9): once per finished utterance, never for `.agentPrompt` (refused, not an error).
    func polish(text: String, destination: Destination, session: String?) async throws -> PolishReply {
        try await call("polish", [.init(text), .init(destination), .init(session)])
    }

    /// `rewrite(instruction, selection, app, thread)` (§13.6); `thread` = earlier (instruction, result) pairs as 2-arrays.
    func rewrite(instruction: String, selection: String, app: AppProfile, thread: [[String]]?) async throws -> PolishReply {
        try await call("rewrite", [.init(instruction), .init(selection), .init(app), .init(thread)])
    }

    /// `voice_status` → the model state, voices and 153's `input_devices`.
    func voiceStatus() async throws -> VoiceStatus { try await call("voice_status") }
}
