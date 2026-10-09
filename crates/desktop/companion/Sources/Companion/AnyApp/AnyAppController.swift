import AppKit
import CompanionCore
import TheuthKit

/// 153 §3.2 / §13: dictate ANYWHERE. `PointController` owns the gesture (⌃⌥D is its third mode key); this controller owns
/// the target: the AX capture at the press, D1's verdict → `AnyAppSession`, the delivery of every closed utterance (direct
/// `kAXSelectedText` or the clipboard lease + ⌘V posted to the app's pid), the verification reads, "send it"'s Return,
/// "scratch that", polish per app profile, the §13.5 failure card, the §13.6 rewrite panel and the §8.6 media toggle.
@MainActor
final class AnyAppController {
    /// §9.5: polish waits this long, then the shaped text goes.
    static let polishBudget: Duration = .seconds(4)
    /// §13.2: the direct write's read-back.
    static let directReadBackMs = 150
    /// A paste must reach the app before the next lease write replaces the pasteboard (ponytail: fixed gap; an AX
    /// `kAXValueChanged` wait would be exact).
    static let pasteGapMs = 120
    /// §13.6 Replace / §13.5 Try again: focus restored, then this long before the paste.
    static let refocusMs = 200
    static let dir = URL(fileURLWithPath: "/tmp/153-d2", isDirectory: true)

    private let store: Store
    let card = AnyAppCard()
    private let pasteboard = SystemPasteboard()
    private var lease: ClipboardLease
    private(set) var session: AnyAppSession?
    private var target: AXTarget?
    private var gestureId = ""
    private var texts: [Int: String] = [:]
    private var chain: Task<Void, Never>?
    private var finished = false
    private var mediaToggled = false
    /// Deliveries the verification called `notLanded`, in order — the failure card's transcript and what Try again re-pastes.
    private var failed: [String] = []
    private var failureNote = ""
    /// The pill's label changes ("polishing…") and hints after the gesture.
    var onLabel: ((String) -> Void)?
    var onHint: ((String) -> Void)?
    /// Probes: no AX, no keystrokes, no RPC.
    var dryRun = false

    init(store: Store) {
        self.store = store
        lease = ClipboardLease(pasteboard: pasteboard, leaveTextOnClipboard: ModeKeySettings.leaveTextOnClipboard(Settings.defaults))
        card.onEscape = { [weak self] in self?.dismissCard() }
        card.model.onAction = { [weak self] a in self?.cardAction(a) }
    }

    /// The table's ⌃⌥D as the pill prints it.
    static var chordSymbol: String { SettingsModel.shared.value.keys.chord(for: .dictateAnywhere)?.symbol ?? "⌃⌥D" }

    /// Registered when on. Default (key never written): on once Accessibility is granted, off until then (the brief's
    /// rule); an explicit switch wins either way (§13.7: pasting blind works without the grant).
    static var hotkeyEnabled: Bool {
        Settings.defaults.object(forKey: ModeKeySettings.dictateAnywhereEnabledKey) == nil
            ? SystemState.accessibilityTrusted : ModeKeySettings.dictateAnywhereEnabled(Settings.defaults)
    }

    /// `voice.duck_media: pause` (§8.6), companion-local, default off.
    static let pauseMediaKey = "pauseMediaWhileDictating"
    static var pauseMedia: Bool { VoiceKeySettings.flag(pauseMediaKey, false) }

    var isRewrite: Bool { if case .rewrite = session?.entry { return true } else { return false } }

    // MARK: press

    /// At the press (or a mode switch into `.dictate`): capture the focused element, assess, resolve the profile. Answers the
    /// refusal hint (no dictation), else `nil` with the pill's label in `label`. A hold while the rewrite panel is up refines
    /// the same selection (the focused element is our panel then — never re-captured).
    func press() -> (refused: String?, label: String) {
        if card.isVisible, isRewrite, let s = session {
            card.hide()
            finished = false
            texts = [:]
            return (nil, "Refine · \(s.selection.count) chars selected · say what to change")
        }
        if card.isVisible { card.hide() } // a new gesture retires the failure card (the words are in history, §6.5)
        gestureId = UUID().uuidString
        texts = [:]; finished = false; failed = []; failureNote = ""
        let (t, snapshot, selection): (AXTarget?, AXSnapshot, String?) = dryRun ? (nil, AXSnapshot(), nil) : AXTarget.capture()
        target = t
        let own = t.map { $0.pid == getpid() || $0.app.bundleIdentifier == MainApp.bundleID } ?? false
        let profile = AppProfiles.resolve(bundleId: snapshot.appBundleId, name: t?.app.localizedName, in: ModeKeySettings.appProfiles(Settings.defaults))
        let s = AnyAppSession(snapshot: snapshot, profile: profile, selection: selection, trusted: dryRun || SystemState.accessibilityTrusted,
                              ownWindow: own, screenLocked: !dryRun && SystemState.screenLocked)
        session = s
        lease.leaveTextOnClipboard = ModeKeySettings.leaveTextOnClipboard(Settings.defaults)
        PointLog.write("anyapp press: \(snapshot.appBundleId ?? "?") role=\(snapshot.role ?? "nil") sub=\(snapshot.subrole ?? "nil") editable=\(String(describing: snapshot.isEditable)) → \(s.verdict) entry=\(s.entry) profile=\(profile.id) terminal=\(profile.terminal) polish=\(profile.polish)")
        switch s.entry {
        case .refused(let hint):
            session = nil
            return (hint, "")
        case .rewrite(let chars):
            return (nil, "Rewrite · \(chars) chars selected · say what to change")
        case .dictate(let label):
            if Self.pauseMedia, !dryRun { Keystrokes.mediaPlayPause(); mediaToggled = true }
            return (nil, label)
        }
    }

    /// `dictation_start`'s opts (§14.2): `surface: any_app`, `destination: any_app(bundle)`, the profile row.
    func opts(pressAgoMs: UInt32) -> DictationOpts {
        let p = session?.profile ?? AppProfiles.resolve(bundleId: nil, in: [])
        return DictationOpts(surface: .anyApp, destination: .anyApp(app: p.id), app: p, commands: [], pressAgoMs: pressAgoMs)
    }

    // MARK: utterances (§3.2: delivered as they close)

    /// A `DictationText` for utterance `i` (`shaped ?? text`). Finals are delivered in order through one chain; an earlier
    /// utterance re-sent empty is "scratch that" (§4.6) for a delivered one, or just nothing to deliver.
    func utterance(_ i: Int, text: String, final: Bool) {
        guard session != nil, !isRewrite, final else { return }
        texts[i] = text
        let g = gestureId
        enqueue { [weak self] in
            guard let self, self.gestureId == g, var s = self.session else { return }
            let t = self.texts[i] ?? ""
            if t.isEmpty {
                guard s.isDelivered(i) else { return }
                let what = s.scratch(i)
                self.session = s
                await self.scratch(what)
                return
            }
            guard !s.isDelivered(i) else { return }
            if s.exceedsCap(adding: t) {
                self.failed.append(t); self.failureNote = "over \(AnyAppSession.maxCharsPerGesture) chars in one gesture — the words are here"
                return
            }
            let polished = s.profile.polish ? await self.polished(t, profile: s.profile) : t
            guard self.gestureId == g, let s2 = self.session else { return }
            s = s2
            let payload = s.payload(polished)
            let (method, verified) = await self.deliver(payload)
            guard self.gestureId == g, var s3 = self.session else { return }
            s3.record(utterance: i, text: payload, method: method, verified: verified)
            if method == .lease, s.method == .direct { s3.directFailedOnce() }
            self.session = s3
        }
    }

    private func enqueue(_ op: @escaping @MainActor () async -> Void) {
        let prev = chain
        chain = Task { await prev?.value; await op() }
    }

    /// §13.2 then §13.3: direct write + 150 ms read-back on `editable`; the lease otherwise or when the read-back fails.
    private func deliver(_ text: String) async -> (AnyAppSession.Method, Bool) {
        guard let s = session, !dryRun else { return (.lease, false) }
        if s.method == .direct, let t = target {
            let before = t.read()
            if t.insert(text) {
                try? await Task.sleep(for: .milliseconds(Self.directReadBackMs))
                let outcome = PasteVerifier.verify(text: text, before: before ?? FieldRead(), reads: [t.read()])
                PointLog.write("anyapp direct \(text.count) chars → \(outcome)")
                if outcome == .landed { return (.direct, true) }
                if case .unknown = outcome { return (.direct, false) }
            } else {
                PointLog.write("anyapp direct write refused by the app")
            }
            // Fell through: the lease for this and every later utterance (§13.2).
        }
        return (.lease, await paste(text))
    }

    /// §13.3's sequence for one text: snapshot → write → ⌘V to the pid → settle at 500 ms → verify at 150/350/1000 ms.
    private func paste(_ text: String) async -> Bool {
        guard let t = target, !t.app.isTerminated else { fail(text, "the app is gone"); return false }
        let before = t.read()
        let generation: Int
        do {
            generation = try lease.begin()
            try lease.write(text, session: gestureId)
        } catch {
            fail(text, Self.failureKind(error)); return false
        }
        guard Keystrokes.paste(pid: t.pid) else { fail(text, "paste command failed"); return false }
        lease.pasteIssued()
        Task { @MainActor [weak self] in
            try? await Task.sleep(for: .milliseconds(ClipboardLease.settleMs))
            guard let self else { return }
            do { PointLog.write("anyapp settle(\(generation)) → \(try self.lease.settle(generation: generation))") }
            catch { PointLog.write("anyapp settle failed: \(error)") }
        }
        let check = ModeKeySettings.checkPasteLanded(Settings.defaults)
        guard check, let before else {
            try? await Task.sleep(for: .milliseconds(Self.pasteGapMs))
            return !check // blind paste counts as ours when the user turned the check off; unverifiable otherwise
        }
        // The triple stops at the first decision (landed at 150 ms is the usual case), so the chain rarely waits long.
        let outcome = await PasteVerifier.run(text: text, before: before, sleep: { try? await Task.sleep(for: .milliseconds($0)) }, read: { t.read() })
        PointLog.write("anyapp verify \(text.count) chars → \(outcome)")
        if outcome == .notLanded {
            fail(text, "checked at \(PasteVerifier.readDelaysMs.map(String.init).joined(separator: " · ")) ms — field unchanged · clipboard \(lease.leaveTextOnClipboard ? "kept" : "restored")")
        }
        return outcome == .landed
    }

    private static func failureKind(_ e: Error) -> String {
        switch e as? ClipboardLease.Failure {
        case .clipboardSnapshotFailed: return "clipboard snapshot failed"
        case .clipboardWriteFailed: return "clipboard write failed"
        case .restoreFailed: return "clipboard restore failed"
        case nil: return "\(e)"
        }
    }

    private func fail(_ text: String, _ note: String) {
        failed.append(text); failureNote = note
        PointLog.write("anyapp failed: \(note)")
        if finished { showFailure() }
    }

    private func scratch(_ what: AnyAppSession.Scratch) async {
        guard let t = target, !dryRun else { return }
        switch what {
        case .selectBack(let n): PointLog.write("anyapp scratch select-back \(n) → \(t.selectBack(n))")
        case .backspace(let n): PointLog.write("anyapp scratch ⌫×\(n) → \(Keystrokes.backspace(n, pid: t.pid))")
        case .ignore(let hint): onHint?(hint)
        }
    }

    /// §9: `polish` within the budget; refused / timeout / error → the shaped text.
    private func polished(_ text: String, profile: AppProfile) async -> String {
        guard !dryRun else { return text }
        onLabel?("polishing…")
        defer { onLabel?(session?.label ?? "") }
        let client: RpcClient = store.client
        let reply: PolishReply? = await withTaskGroup(of: PolishReply?.self) { group in
            group.addTask { try? await client.polish(text: text, destination: .anyApp(app: profile.id), session: nil) }
            group.addTask { try? await Task.sleep(for: Self.polishBudget); return nil }
            let first = await group.next() ?? nil
            group.cancelAll()
            return first
        }
        guard let r = reply, r.refused == nil, let p = r.text, !p.isEmpty else {
            PointLog.write("anyapp polish → shaped (\(reply?.refused ?? "timeout/error"))"); return text
        }
        return p
    }

    // MARK: finish

    /// The gesture ended: every queued delivery lands, "send it" presses Return when §7.4 allows, media toggles back.
    /// Answers the pill's done copy. Rewrite mode: the instruction goes to `rewrite` and the panel opens instead.
    func finish(text: String, sendIt: Bool, mouse: NSPoint) async -> String {
        guard var s = session else { return "" }
        if isRewrite { return await rewrite(instruction: text, mouse: mouse) }
        await chain?.value
        finished = true
        if mediaToggled { Keystrokes.mediaPlayPause(); mediaToggled = false }
        guard let s2 = session else { return "" }
        s = s2
        if sendIt {
            if s.returnAllowed, let t = target, !dryRun {
                _ = Keystrokes.returnKey(pid: t.pid)
                PointLog.write("anyapp send it → Return")
            } else if !s.returnAllowed {
                PointLog.write("anyapp send it refused: secure or no target")
                if !failed.isEmpty { showFailure(mouse) }
                return s.delivered.isEmpty ? "not sent — secure field" : "\(s.doneCopy) · not sent — secure field"
            }
        }
        if !failed.isEmpty { showFailure(mouse); return "" }
        return s.delivered.isEmpty ? "Nothing recognized" : s.doneCopy
    }

    /// Esc / cancel / a switch away from `.dictate`: what landed stays; media toggles back; nothing else.
    func abandon() {
        if mediaToggled { Keystrokes.mediaPlayPause(); mediaToggled = false }
        if !isRewrite { session = nil; texts = [:] }
    }

    // MARK: §13.5 failure card

    private var failureAnchor = NSPoint.zero

    private func showFailure(_ mouse: NSPoint? = nil) {
        guard let s = session else { return }
        if let mouse { failureAnchor = mouse }
        card.model.kind = .failure
        card.model.title = "Couldn't type into \(s.profile.name) · paste not landed"
        card.model.text = failed.joined(separator: " ")
        card.model.note = failureNote
        card.model.busy = false
        card.show(near: failureAnchor, key: !AskFlags.automated)
    }

    private func dismissCard() {
        card.hide()
        if !isRewrite { failed = [] }
        session = nil
    }

    private func cardAction(_ a: AnyAppCardModel.Action) {
        switch a {
        case .copy:
            pasteboard.copyPlain(failed.joined(separator: " "))
            card.model.note = "copied — paste it where it goes · clipboard is yours again at the next copy"
        case .tryAgain:
            let text = failed.joined(separator: " ")
            guard let t = target, !t.app.isTerminated else { card.model.note = "\(session?.profile.name ?? "the app") is gone"; return }
            card.model.busy = true
            Task { @MainActor in
                self.card.hide()
                t.focus()
                try? await Task.sleep(for: .milliseconds(Self.refocusMs))
                self.failed = []
                _ = await self.paste(text)
                self.card.model.busy = false
                if self.failed.isEmpty { self.onHint?("✓ pasted into \(self.session?.profile.name ?? "the app")"); self.session = nil }
                else { self.card.model.note = "still not landed — " + self.failureNote; self.card.show(near: self.failureAnchor, key: !AskFlags.automated) }
            }
        case .openInTheuth:
            let text = failed.joined(separator: " ")
            card.model.busy = true
            Task { @MainActor in
                do {
                    let desk = try await self.store.resolveDesk()
                    _ = try await self.store.client.sendInput(session: desk, text: PointController.noteLine(text, durationMs: 0))
                    self.dismissCard()
                    self.onHint?("sent to theuth's desk")
                } catch {
                    self.card.model.busy = false
                    self.card.model.note = "couldn't send — \(error)"
                }
            }
        case .replace:
            replace()
        }
    }

    // MARK: §13.6 rewrite

    private var rewriteAnchor = NSPoint.zero

    private func rewrite(instruction: String, mouse: NSPoint) async -> String {
        guard var s = session else { return "" }
        rewriteAnchor = mouse
        let words = instruction.trimmingCharacters(in: .whitespacesAndNewlines)
        guard !words.isEmpty else { return "Nothing recognized" }
        card.model.kind = .rewrite
        card.model.title = "Rewrite · \(s.profile.name) · \(s.selection.count) chars selected"
        card.model.instruction = words
        card.model.excerpt = s.selection
        card.model.text = ""
        card.model.note = s.thread.isEmpty ? "" : "refinement \(s.thread.count + 1)"
        card.model.busy = true
        card.show(near: mouse, key: !AskFlags.automated)
        guard !dryRun else { return "" }
        do {
            let r = try await store.client.rewrite(instruction: words, selection: s.selection, app: s.profile, thread: s.thread.isEmpty ? nil : s.thread)
            guard let text = r.text, r.refused == nil, !text.isEmpty else {
                card.model.busy = false
                card.model.note = "no proposal — \(r.refused ?? "empty reply")"
                return ""
            }
            s.refined(instruction: words, result: text)
            session = s
            card.model.text = text
            card.model.busy = false
            card.model.note = r.model.map { "by \($0) · \(r.ms) ms" } ?? ""
        } catch {
            card.model.busy = false
            card.model.note = "couldn't rewrite — \(error)"
        }
        return ""
    }

    /// Replace: focus back to the captured element first (A:L165), then the proposal through the same delivery path — the
    /// selection is what the write / the paste replaces.
    private func replace() {
        guard let s = session, let text = s.thread.last?[1], let t = target, !t.app.isTerminated else { card.model.note = "the app is gone"; return }
        card.model.busy = true
        Task { @MainActor in
            self.card.hide()
            t.focus()
            try? await Task.sleep(for: .milliseconds(Self.refocusMs))
            let (method, verified) = await self.deliver(text)
            PointLog.write("anyapp replace via \(method) verified=\(verified)")
            self.session = nil
            self.onHint?("✓ replaced in \(s.profile.name)")
        }
    }
}
