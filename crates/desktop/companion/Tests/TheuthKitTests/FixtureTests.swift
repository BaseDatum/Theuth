import Testing
import Foundation
@testable import TheuthKit

/// The contract test (130 §5.6): every fixture under `Fixtures/` decodes with the hand-written models.
struct FixtureTests {
    private func fixture(_ name: String) throws -> Data {
        let url = try #require(Bundle.module.url(forResource: name, withExtension: "json", subdirectory: "Fixtures"))
        return try Data(contentsOf: url)
    }

    private func decode<T: Decodable>(_ type: T.Type, _ name: String) throws -> T {
        try TheuthJSON.decoder.decode(T.self, from: try fixture(name))
    }

    @Test func attentionListEveryKind() throws {
        let items = try decode([AttentionItem].self, "attention_list")
        #expect(items.map(\.kind) == [.approval, .decision, .ask, .review, .hire, .show])
        #expect(items[0].approval == "3f6c1a2e-9b1d-4e0a-8c4b-5d2a7f1e9c01")
        #expect(items[1].packet == "decision:dec-2")
        #expect(items[1].task == "task-7")
        let ask = try #require(items[2].ask)
        #expect(ask.questions[0].options.count == 4)
        #expect(ask.questions[0].recommended == 1)
        #expect(ask.questions[0].options[1].description == "safer; ~20 min later")
        #expect(items[2].waitingSince == Date(timeIntervalSince1970: 1_791_448_700))
        #expect(items[0].kind.needsYou)
        #expect(!(items[5].kind.needsYou))
        // Pre-W0 engine: the optional fields default.
        #expect(items[0].waitingSinceMs == nil)
        #expect(items[0].nextSteps == [])
    }

    @Test func finishedItemCarriesW0Fields() throws {
        let item = try decode(AttentionItem.self, "attention_item_finished")
        #expect(item.kind == .finished)
        #expect(item.turn == 12)
        #expect(item.nextSteps.count == 2)
        #expect(item.summary?.prefix(13) == "Signed tokens")
        #expect(!(item.kind.needsYou))
    }

    @Test func unknownKindDecodesToOther() throws {
        let item = try decode(AttentionItem.self, "attention_item_unknown_kind")
        #expect(item.kind == .other("weather"))
        #expect(item.kind.rawValue == "weather")
        #expect(!(item.kind.needsYou))
    }

    /// 155 §7.1: `alert` decodes with its snake_case class; a 155 engine's `null` is "decided, nothing"; an older
    /// engine's missing key is "not decided"; an unknown class word is no alert, not a failed list.
    @Test func alertDecodesAndMarksTheEngineDecision() throws {
        let item = try decode(AttentionItem.self, "attention_item_155_alert")
        #expect(item.engineDecides)
        let a = try #require(item.alert)
        #expect(a.id == "\(item.id)@1791448440000" && a.cls == .finished && a.atMs == 1_791_448_440_000)
        #expect(a.why == "turn ended: Signed tokens replace the session cookie.")
        let older = try decode(AttentionItem.self, "attention_item_finished")
        #expect(!older.engineDecides && older.alert == nil)
        var json = try JSONSerialization.jsonObject(with: fixture("attention_item_155_alert")) as! [String: Any]
        json["alert"] = NSNull()
        let blanked = try TheuthJSON.decoder.decode(AttentionItem.self, from: JSONSerialization.data(withJSONObject: json))
        #expect(blanked.engineDecides && blanked.alert == nil)
        json["alert"] = ["id": "x@1", "class": "shouting", "why": "", "at_ms": 1]
        let unknown = try TheuthJSON.decoder.decode(AttentionItem.self, from: JSONSerialization.data(withJSONObject: json))
        #expect(unknown.engineDecides && unknown.alert == nil)
    }

    @Test func agentsList() throws {
        let agents = try decode([AgentSummary].self, "agents_list")
        #expect(agents.map(\.status) == [.working, .paused, .invalid])
        #expect(agents[0].file.model == "claude-fable")
        #expect(agents[0].desk == "0192c5e0-7a1b-7c2d-9e3f-4a5b6c7d8e02")
        #expect(agents[0].spentMonth == 12.5)
        #expect(agents[1].file.paused)
        #expect(agents[2].error != nil)
    }

    @Test func listActivity() throws {
        let rows = try decode([AgentInfo].self, "list_activity")
        #expect(rows[0].state == .running)
        #expect(rows[0].activity.live)
        #expect(rows[0].activity.children == 2)
        // Fractional and whole-second RFC 3339 both parse.
        #expect(rows[0].activity.turnStartedAt != nil)
        #expect(rows[0].finishedAt == Date(timeIntervalSince1970: 1_791_367_920))
        #expect(rows[1].state == .approval(tool: "shell"))
        #expect(rows[1].activity.waitingSinceMs == 1_791_448_800_000)
        // No `activity` on the wire (pre-62 engine): defaults from `state`.
        #expect(rows[2].activity.state == .error)
        #expect(!(rows[2].activity.live))
        #expect(rows[2].agent == "codex")
    }

    @Test func packets() throws {
        let decision = try decode(Packet.self, "packet_decision")
        #expect(decision.kind == .decision)
        #expect(decision.options.count == 4)
        #expect(decision.recommended == 0)
        #expect(decision.preferredOption == 2, "the forwarded recommendation wins")
        #expect(decision.pointers[0].ref == "src/pricing.tsx#L10-L24")
        let review = try decode(Packet.self, "packet_review")
        #expect(review.kind == .review)
        #expect(review.preferredOption == nil)
        #expect(review.checks.map(\.done) == [true, false])
        #expect(review.to == nil)
    }

    @Test func events() throws {
        guard case .approvalRequested(let a) = try WireEvent.decode(fixture("approval_requested")) else { Issue.record("wrong case"); return }
        #expect(a.risk == .mutating)
        #expect(a.args["command"]?.string == "git push origin v2.10.1")

        guard case .approvalResolved(let r) = try WireEvent.decode(fixture("approval_resolved")) else { Issue.record("wrong case"); return }
        #expect(r.decision == .deny)
        #expect(r.timedOut)

        guard case .askRequested(let q) = try WireEvent.decode(fixture("ask_requested")) else { Issue.record("wrong case"); return }
        #expect(q.questions.count == 2)
        #expect(q.questions[1].options == [], "`#[serde(default)]` options")

        guard case .askResolved(let ar) = try WireEvent.decode(fixture("ask_resolved")) else { Issue.record("wrong case"); return }
        #expect(!(ar.dismissed))

        guard case .turnFinished(let t) = try WireEvent.decode(fixture("turn_finished")) else { Issue.record("wrong case"); return }
        #expect(t.stopReason == "end_turn")
        #expect(t.context == 1500)

        guard case .attentionChanged(let c) = try WireEvent.decode(fixture("attention_changed")) else { Issue.record("wrong case"); return }
        #expect(c.count == 3)
        #expect(c.idsRemoved == ["approval:b2"])

        guard case .lagged(let l) = try WireEvent.decode(fixture("lagged")) else { Issue.record("wrong case"); return }
        #expect(l.count == 17)

        guard case .pendingPrompts(let p) = try WireEvent.decode(fixture("pending_prompts")) else { Issue.record("wrong case"); return }
        #expect(p.approvals.first?.risk == .destructive)
        #expect(p.secrets.count == 1)

        #expect(try WireEvent.decode(Data(#"{"type":"TextDelta","session":"s","text":"x"}"#.utf8)) == .other("TextDelta"))
    }

    @Test func answerAndDecisionEncodeLikeSerde() throws {
        let enc = TheuthJSON.encoder
        #expect(String(decoding: try enc.encode(Answer.choice(index: 1)), as: UTF8.self) == #"{"index":1,"kind":"choice"}"#)
        #expect(String(decoding: try enc.encode(Answer.text("hi")), as: UTF8.self) == #"{"kind":"text","text":"hi"}"#)
        #expect(String(decoding: try enc.encode(Answer.skipped), as: UTF8.self) == #"{"kind":"skipped"}"#)
        #expect(String(decoding: try enc.encode([Decision.allow]), as: UTF8.self) == #"["allow"]"#)
        // Positional params with a trailing `null` (answers: None).
        let params = [AnyEncodable("id-1"), AnyEncodable([Answer]?.none)]
        #expect(String(decoding: try enc.encode(params), as: UTF8.self) == #"["id-1",null]"#)
    }

    @Test func discovery() throws {
        let dir = FileManager.default.temporaryDirectory.appendingPathComponent("theuth-kit-\(UUID().uuidString)")
        try FileManager.default.createDirectory(at: dir, withIntermediateDirectories: true)
        defer { try? FileManager.default.removeItem(at: dir) }
        #expect(throws: (any Error).self) { try EngineLocation.discover(dataDir: dir) }
        try "abc123\n".write(to: dir.appendingPathComponent("token"), atomically: true, encoding: .utf8)
        #expect(try EngineLocation.discover(dataDir: dir) == EngineLocation(url: URL(string: "ws://127.0.0.1:7777/")!, token: "abc123"))
        try #"{"pid":1,"listen":"127.0.0.1:49431"}"#.write(to: dir.appendingPathComponent("engine.json"), atomically: true, encoding: .utf8)
        #expect(try EngineLocation.discover(dataDir: dir).url == URL(string: "ws://127.0.0.1:49431/")!)
        #expect(EngineLocation.dataDir(env: ["THEUTH_DATA_DIR": "/tmp/x"]).path == "/tmp/x")
    }

    // MARK: 149 (design 149 §5.1 row 1; fixtures derived from the ts-rs bindings / the Rust structs)

    @Test func attentionItemContextPriorityRank() throws {
        let with = try decode(AttentionItem.self, "attention_item_149_context")
        #expect(with.context == ItemContext(request: "Rename the pricing helper and keep the tests green",
                                            lastReply: "Two call sites disagree on the rounding; which one is right?"))
        #expect(with.priority == .high)
        #expect(with.rank == 17)
        let without = try decode(AttentionItem.self, "attention_item_149_no_context")
        #expect(without.context == nil)
        #expect(without.priority == nil)
        #expect(without.rank == 5)
        // Pre-149 fixture: absent `rank` takes the engine's no-priority value.
        #expect(try decode([AttentionItem].self, "attention_list")[0].rank == 5)
    }

    @Test func packetIfWrong() throws {
        let p = try decode(Packet.self, "packet_149_if_wrong")
        #expect(p.ifWrong?.hasPrefix("Invoices and the ledger drift") == true)
        #expect(try decode(Packet.self, "packet_decision").ifWrong == nil)
    }

    @Test func sessionsAndStatus() throws {
        let rows = try decode([SessionSummary].self, "list_sessions_149")
        #expect(rows.map(\.running) == [true, false, false])
        #expect(rows.map(\.owner) == ["", "engine-studio", ""])
        #expect(rows[0].cwd == "/Users/sky/Develop/theuth")
        #expect(rows[0].project == "theuth")
        #expect(rows[0].name == "w3")
        #expect(rows[1].title == "")
        #expect(rows[1].parent == "s-7")
        #expect(rows[1].desk == "0192c5e0-7a1b-7c2d-9e3f-4a5b6c7d8e02")
        #expect(rows[2].archived, "pre-149 row shape: the defaults fill in")
        let status = try decode(SessionsStatus.self, "sessions_status_149")
        #expect(status.here == 2 && status.elsewhere == 1 && status.engine == "engine-mbp")
    }

    @Test func meetingStatusInEveryShape() throws {
        let live = try decode(MeetingStatus.self, "meeting_status_149")
        #expect(live.state == .recording && live.state.isLive)
        #expect(live.degraded == [.micNoAec, .sttDown(.system)])
        #expect(live.topics.map(\.label) == ["quarterly numbers", "launch date"])
        #expect(live.topics[0].source == .cache && live.topics[1].source == nil)
        #expect(live.cursor == "0192c5e0-7a1b-7c2d-9e3f-4a5b6c7d8e31")
        #expect(live.supported == .partial("no process tap on this host"))
        let idle = try decode(MeetingStatus.self, "meeting_status_149_idle")
        #expect(idle.state == .idle && !idle.state.isLive && idle.meeting == nil && idle.supported == .full)
        let post = try decode(MeetingStatus.self, "meeting_status_149_post")
        #expect(post.state == .post(.diarizing))
        #expect(post.degraded == [.permissionDenied("AudioCapture")])
        #expect(post.supported == nil)
        // The externally tagged state round-trips.
        let enc = TheuthJSON.encoder
        #expect(String(decoding: try enc.encode(MeetingState.post(.aligning)), as: UTF8.self) == #"{"Post":"Aligning"}"#)
        #expect(String(decoding: try enc.encode(MeetingState.paused), as: UTF8.self) == #""Paused""#)
    }

    @Test func meetingLists() throws {
        let briefs = try decode([Brief].self, "meeting_briefs_149")
        #expect(briefs[0].bullets.count == 2 && briefs[0].source == .cache && briefs[0].cached == briefs[0].id)
        #expect(briefs[1].parent == briefs[0].id && briefs[1].topic == nil && briefs[1].source == .request)
        let lines = try decode([Utterance].self, "meeting_live_149")
        #expect(lines.map(\.track) == [.mic, .system, .mic])
        #expect(lines[1].speaker == "s2" && lines[0].speaker == nil)
        #expect(lines[2].request)
        #expect(try decode([Topic].self, "meeting_topics_149").count == 2)
        let perms = try decode(MeetingPermissions.self, "meeting_permissions_149")
        #expect(perms.microphone == .granted && perms.audioCapture == .notDetermined)
    }

    @Test func meetingEventsEveryVariant() throws {
        let items = try JSONSerialization.jsonObject(with: fixture("meeting_events_149")) as! [Any]
        let events = try items.map { try MeetingEvent.decode(try JSONSerialization.data(withJSONObject: $0)) }
        #expect(events.count == 16 && events.allSatisfy { $0 != nil })
        let m = "0192c5e0-7a1b-7c2d-9e3f-4a5b6c7d8e10"
        guard case .started(m, "s-meeting-1", let process, "2026-10-09T00:45:00Z") = events[0] else { Issue.record("Started"); return }
        #expect(process.name == "zoom.us" && process.bundleId == "us.zoom.xos")
        #expect(events[1] == .state(meeting: m, state: .recording, degraded: [.micNoAec]))
        #expect(events[2] == .level(Level(meeting: m, micRms: 0.12, systemRms: 0.4)))
        guard case .utterance(m, let partial, false) = events[3] else { Issue.record("partial"); return }
        #expect(partial.text == "Revenue is up" && partial.speaker == nil)
        guard case .utterance(m, let closed, true) = events[4] else { Issue.record("final"); return }
        #expect(closed.speaker == "s2" && closed.t0Ms == 2500)
        guard case .topicAdded(m, let topic) = events[5] else { Issue.record("TopicAdded"); return }
        #expect(topic.label == "quarterly numbers" && topic.source == .cache)
        #expect(events[6] == .topicRemoved(meeting: m, topic: topic.id, why: .viewed))
        guard case .brief(m, let brief) = events[7] else { Issue.record("Brief"); return }
        #expect(brief.title == "Launch plan" && brief.source == .request)
        #expect(events[8] == .listening(meeting: m, held: true))
        #expect(events[9] == .approvalWaiting(meeting: m, waiting: true))
        #expect(events[10] == .postProgress(meeting: m, stage: .transcribing, pct: 40))
        #expect(events[11] == .reviewReady(meeting: m))
        #expect(events[12] == .summarized(meeting: m, title: "Q3 review"))
        #expect(events[13] == .deleted(meeting: m))
        #expect(events[14] == .oracleDeciding(meeting: m, batchId: "00:12:31"))
        #expect(events[15] == .oracleDecision(meeting: m, decision: OracleDecision(batchId: "00:12:31", family: "numbers")))
        // Not a meeting item: nil, so `WireEvent.decode` keeps owning it.
        #expect(try MeetingEvent.decode(fixture("turn_finished")) == nil)
        // A meeting item missing a field is an error, not a guess.
        #expect(throws: (any Error).self) { try MeetingEvent.decode(Data(#"{"type":"Listening","meeting":"m"}"#.utf8)) }
    }

    @Test func captureAndFallbackShapesDecode() throws {
        // 173: the object-valued degradation never fails the whole State; unknown shapes read as `.other`.
        let state = try MeetingEvent.decode(Data(#"{"type":"State","meeting":"m","state":"Recording","degraded":[{"SystemAudioFallback":{"reason":"Silent","at_ms":1,"until_ms":null}},{"SystemAudioFallback":{"reason":"Stalled","at_ms":1,"until_ms":9}},{"PermissionDenied":"Microphone"},{"Future":{"x":1}}]}"#.utf8))
        #expect(state == .state(meeting: "m", state: .recording, degraded: [
            .systemAudioFallback(reason: "Silent", ended: false), .systemAudioFallback(reason: "Stalled", ended: true),
            .permissionDenied("Microphone"), .other("Future"),
        ]))
        let capture = try MeetingEvent.decode(Data(#"{"type":"Capture","meeting":"m","capture":{"mic":{"state":"Ok"},"system":{"state":"Lost","reason":"PermissionDenied","since":"2026-11-03T10:00:00Z"}}}"#.utf8))
        #expect(capture == .capture(meeting: "m", capture: CaptureStatus(mic: .ok, system: .lost(reason: .permissionDenied, since: "2026-11-03T10:00:00Z"))))
        // An unknown state or reason degrades, never throws.
        let odd = try TheuthJSON.decoder.decode(CaptureStatus.self, from: Data(#"{"mic":{"state":"Teleporting"},"system":{"state":"Degraded","reason":"Cosmic"}}"#.utf8))
        #expect(odd == CaptureStatus(mic: .ok, system: .degraded(reason: .unknown)))
        // `meeting_status` with and without `capture` (older engines).
        let status = try TheuthJSON.decoder.decode(MeetingStatus.self, from: Data(#"{"state":"Recording","capture":{"mic":{"state":"Recovering","reason":"Stalled","since":"2026-11-03T10:00:00Z"},"system":{"state":"Ok"}}}"#.utf8))
        #expect(status.capture == CaptureStatus(mic: .recovering(reason: .stalled, since: "2026-11-03T10:00:00Z"), system: .ok))
        #expect(try decode(MeetingStatus.self, "meeting_status_149").capture == .ok)
        #expect(try json(Degradation.systemAudioFallback(reason: "Silent", ended: false)) == #"{"SystemAudioFallback":{"reason":"Silent"}}"#)
    }

    @Test func pointAndDecisionShapes() throws {
        let reply = try decode(ScreenContextReply.self, "screen_context_reply_149")
        #expect(reply.lines.count == 2 && reply.files.count == 1)
        let mode = try decode(ModeInfo.self, "decision_mode_149")
        #expect(mode.mode == .shadow && mode.effective == .off && !mode.installed && mode.budgetMs == 1500)
        // The request encodes with the engine's snake_case keys and the pointer's `pen`.
        let req = ScreenContextReq(samples: [PointerSample(tMs: 0, x: 10, y: 20)], color: "#007aff", sinceMs: 1_791_448_800_000, accessibility: true)
        #expect(String(decoding: try TheuthJSON.encoder.encode(req), as: UTF8.self)
                == ##"{"accessibility":true,"color":"#007aff","samples":[{"pen":true,"t_ms":0,"x":10,"y":20}],"since_ms":1791448800000}"##)
        #expect(String(decoding: try TheuthJSON.encoder.encode([AnyEncodable("s"), AnyEncodable(CancelScope.tree)]), as: UTF8.self) == #"["s","tree"]"#)
        #expect(String(decoding: try TheuthJSON.encoder.encode([AnyEncodable(Delivery.nextTurn), AnyEncodable(Delivery?.none)]), as: UTF8.self) == #"["next_turn",null]"#)
    }

    // MARK: 153 (design 153 §14.1/§14.4; fixtures written from the Rust types' serde attributes — the ts-rs bindings are the cross-check)

    private func json<T: Encodable>(_ v: T) throws -> String { String(decoding: try TheuthJSON.encoder.encode(v), as: UTF8.self) }

    @Test func dictationOptsEncodeLikeSerde() throws {
        let full = try decode(DictationOpts.self, "153-dictation_opts")
        #expect(full.surface == .anyApp && full.destination == .anyApp(app: "com.apple.Terminal") && full.pressAgoMs == 42)
        #expect(full.app?.terminal == true && full.app?.commands == ["/clear"] && full.app?.polishPrompt == nil)
        // Round trip keeps the tag/field names (`kind`, `any_app`, `press_ago_ms`).
        #expect(try TheuthJSON.decoder.decode(DictationOpts.self, from: try TheuthJSON.encoder.encode(full)) == full)
        let defaults = try decode(DictationOpts.self, "153-dictation_opts_defaults")
        #expect(defaults == DictationOpts(), "serde(default): nulls decode to the empty opts")
        #expect(try json(DictationOpts()) == #"{"commands":[]}"#, "nil optionals are omitted, the engine defaults them")
        #expect(try json(DictationOpts(surface: .companion, destination: .agentPrompt, pressAgoMs: 7))
                == #"{"commands":[],"destination":{"kind":"agent_prompt"},"press_ago_ms":7,"surface":"companion"}"#)
        // The trailing positional of `dictation_start`: `[16000, null]` is today's call.
        #expect(try json([AnyEncodable(UInt32(16_000)), AnyEncodable(DictationOpts?.none)]) == "[16000,null]")
    }

    @Test func dictationEventsEveryNewShape() throws {
        let items = try JSONSerialization.jsonObject(with: try fixture("153-dictation_events")) as! [Any]
        let wires = try items.map { try TheuthJSON.decoder.decode(DictationWire.self, from: JSONSerialization.data(withJSONObject: $0)) }
        #expect(wires.map(\.type) == ["DictationText", "DictationText", "DictationText", "DictationDevice", "DictationDevice", "DictionarySuggestion", "DictationFinished"])
        #expect(wires[0].shaped == "Hi Dana," && wires[0].command == .send(nearMiss: false) && wires[0].actions == [] && wires[0].isFinal == false)
        #expect(wires[1].command == .scratchThat && wires[1].actions == ["new_line"] && wires[1].actions?.compactMap(Action.init(rawValue:)) == [.newLine])
        #expect(wires[2].shaped == nil && wires[2].command == nil && wires[2].actions == nil, "a pre-153 engine's event still decodes")
        #expect(wires[3].device == "AirPods Pro" && wires[3].state == .changed)
        #expect(wires[4].device == nil && wires[4].state == .lost)
        #expect(wires[6].reason?.reason == "done")
        let device = try TheuthJSON.decoder.decode(DictationDevice.self, from: JSONSerialization.data(withJSONObject: items[4]))
        #expect(device.state == .lost && device.device == nil)
        let s = try TheuthJSON.decoder.decode(DictionarySuggestion.self, from: JSONSerialization.data(withJSONObject: items[5]))
        #expect(s.heard == "parakeet tee dee tee" && s.typed == "Parakeet TDT" && s.occurrences == 2 && s.surface == .composer && s.hasAudio)
        #expect(try json(SuggestionAnswer.notNow) == #""not_now""#)
    }

    @Test func dictionaryEntriesEveryScope() throws {
        let entries = try decode([Entry].self, "153-dictionary_list")
        #expect(entries.map(\.scope) == [.global, .surface(.tui), .app("com.apple.Terminal")])
        #expect(entries.map(\.kind) == [.term, .command, .abbrev] && entries.map(\.source) == [.user, .learned, .user])
        #expect(entries[0].variants == [Variant(heard: "parakeet t d t", metaphone: ["PRKT", "PRKT"], seen: 3, lastMs: 1_791_448_800_000)])
        // `dictionary_put` sends the same shape back: tagged scope, tuple-as-array metaphone.
        #expect(try json(Entry(id: "user:rg", canonical: "rg", kind: .abbrev, spoken: ["ripgrep"], scope: .app("com.apple.Terminal")))
                == #"{"canonical":"rg","id":"user:rg","kind":"abbrev","scope":{"app":"com.apple.Terminal","scope":"app"},"source":"user","spoken":["ripgrep"],"tags":[],"variants":[]}"#)
        #expect(try json(Scope.surface(.anyApp)) == #"{"scope":"surface","surface":"any_app"}"#)
    }

    @Test func historyPolishAndStatus() throws {
        let rows = try decode([HistoryRow].self, "153-dictation_history")
        #expect(rows[0].surface == .anyApp && rows[0].app == "com.apple.Notes" && rows[0].polished != nil && rows[0].hasAudio && rows[0].pressMs == 42)
        #expect(rows[1].finishedMs == nil && rows[1].reason.hasPrefix("error:") && rows[1].audioBytes == 0)
        let ok = try decode(PolishReply.self, "153-polish_reply")
        #expect(ok.text == "Hi Dana, the deploy is done." && ok.ms == 820 && ok.refused == nil)
        let refused = try decode(PolishReply.self, "153-polish_reply_refused")
        #expect(refused.text == nil && refused.refused == "agent_prompt is never polished")
        let timings = try decode(DictationTimings.self, "153-dictation_timings")
        #expect(timings.surface == .companion && timings.pressToFirstPartialMs == 310 && timings.polishMs == nil)
        let status = try decode(DictationState.self, "153-dictation_status")
        #expect(status.model == .ready && status.active == nil && status.inputDevice == "MacBook Pro Microphone" && status.last == timings)
        let voice = try decode(VoiceStatus.self, "153-voice_status")
        #expect(voice.model == .failed(message: "checkpoint missing") && voice.inputDevices == ["AirPods Pro", "MacBook Pro Microphone"])
        let devices = try decode([InputDevice].self, "153-input_devices")
        #expect(devices.map(\.default) == [false, true])
        #expect(try decode(EndPhrases.self, "153-end_phrases") == EndPhrases())
        let model = try decode(ModelInfo.self, "153-model_info")
        #expect(model.id == "anthropic/claude-haiku" && model.provider == "anthropic")
        // `set_shape(surface, preset)` and `polish(text, destination, session)` positional params.
        #expect(try json([AnyEncodable(Surface.tui), AnyEncodable(ShapePreset.plain)]) == #"["tui","plain"]"#)
        #expect(try json([AnyEncodable("t"), AnyEncodable(Destination.anyApp(app: "com.apple.Notes")), AnyEncodable(String?.none)])
                == #"["t",{"app":"com.apple.Notes","kind":"any_app"},null]"#)
    }
}
