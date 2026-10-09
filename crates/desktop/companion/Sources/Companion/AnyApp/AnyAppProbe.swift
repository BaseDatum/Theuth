import AppKit
import CompanionCore
import TheuthKit

/// 153 §16 row 24 (render half): `--probe-anyapp <refused|landed|notlanded|rewrite>` — scripted `AXSnapshot`s through the
/// real `AnyAppSession`, the pill / the failure card / the rewrite panel rendered not key, screenshotted with `--shareable`
/// to `/tmp/153-d2/153-anyapp-<kind>.png`, quit. No AX read of a real app, no keystroke, no pasteboard write, no RPC, no
/// hot key, no TCC prompt (`AskFlags.automated`, `dryRun`).
@MainActor
enum AnyAppProbe {
    static let dir = AnyAppController.dir
    static let log = dir.appendingPathComponent("probe.log")

    static func run(kind: String, store: Store, controller c: PointController) {
        try? FileManager.default.createDirectory(at: dir, withIntermediateDirectories: true)
        try? FileManager.default.removeItem(at: log)
        c.anyApp.dryRun = true
        let mail = AppProfile(id: "com.apple.mail", name: "Mail")
        let terminal = AppProfile(id: "com.apple.Terminal", name: "Terminal", terminal: true)
        let mouse = NSEvent.mouseLocation
        let panel: NSWindow
        switch kind {
        case "refused":
            let s = AnyAppSession(snapshot: AXSnapshot(role: "AXButton", appBundleId: mail.id), profile: mail)
            guard case .refused(let hint) = s.entry else { return bad(kind) }
            c.pill.showHint(hint, near: mouse)
            panel = c.pill.panel
            write("refused: \(hint) verdict=\(s.verdict)")
        case "landed":
            var s = AnyAppSession(snapshot: AXSnapshot(role: "AXTextArea", isEditable: true, hasValue: true, appBundleId: mail.id), profile: mail)
            s.record(utterance: 0, text: "Hi Dana, the deploy is done.", method: .direct, verified: true)
            c.pill.state.reset(chord: "⌃⌥D", label: s.label)
            c.pill.state.text = "Hi Dana, the deploy is done."
            c.pill.state.partial = "the rollback plan is in the"
            c.pill.state.level = 0.6
            c.pill.show(near: mouse)
            panel = c.pill.panel
            write("landed: method=\(s.method) returnAllowed=\(s.returnAllowed) done=“\(s.doneCopy)”")
        case "notlanded":
            let s = AnyAppSession(snapshot: AXSnapshot(role: "AXGroup", appBundleId: terminal.id), profile: terminal)
            let m = c.anyApp.card.model
            m.kind = .failure
            m.title = "Couldn't type into \(s.profile.name) · paste not landed"
            m.text = "git rebase -i HEAD~3 then force push with lease"
            m.note = "checked at 150 · 350 · 1000 ms — field unchanged · clipboard restored"
            c.anyApp.card.show(near: mouse, key: false)
            panel = c.anyApp.card.panel
            write("notlanded: verdict=\(s.verdict) method=\(s.method) outcome=\(PasteVerifier.verify(text: m.text, before: FieldRead(value: "$ "), reads: [FieldRead(value: "$ "), FieldRead(value: "$ "), FieldRead(value: "$ ")]))")
        case "rewrite":
            var s = AnyAppSession(snapshot: AXSnapshot(role: "AXTextArea", isEditable: true, appBundleId: mail.id), profile: mail,
                                  selection: "Hey team — quick heads up that the deploy went out this afternoon and so far everything looks fine on our side, let me know if you see anything weird.")
            s.refined(instruction: "make it shorter and more formal", result: "The deployment completed this afternoon and is stable so far. Please report any anomalies.")
            let m = c.anyApp.card.model
            m.kind = .rewrite
            m.title = "Rewrite · \(s.profile.name) · \(s.selection.count) chars selected"
            m.instruction = s.thread[0][0]
            m.excerpt = s.selection
            m.text = s.thread[0][1]
            m.note = "by gpt-5-mini · 912 ms"
            c.anyApp.card.show(near: mouse, key: false)
            panel = c.anyApp.card.panel
            write("rewrite: entry=\(s.entry) thread=\(s.thread.count)")
        default:
            return bad(kind)
        }
        Task {
            try? await Task.sleep(for: .milliseconds(700))
            let png = dir.appendingPathComponent("153-anyapp-\(kind).png")
            let shot = Process()
            shot.executableURL = URL(fileURLWithPath: "/usr/sbin/screencapture")
            shot.arguments = ["-x", "-o", "-l", "\(panel.windowNumber)", png.path]
            try? shot.run()
            shot.waitUntilExit()
            let size = (try? FileManager.default.attributesOfItem(atPath: png.path)[.size] as? Int) ?? 0
            write("probe \(kind): frame=\(panel.frame) visible=\(panel.isVisible) png=\(png.path) bytes=\(size)")
            try? await Task.sleep(for: .milliseconds(300))
            NSApp.terminate(nil)
        }
    }

    private static func bad(_ kind: String) {
        write("probe: unknown kind \(kind)")
        NSApp.terminate(nil)
    }

    static func write(_ s: String) {
        Log.write(s)
        let line = "[companion \(Date()) pid \(getpid())] \(s)\n"
        if let h = try? FileHandle(forWritingTo: log) { h.seekToEndOfFile(); h.write(Data(line.utf8)); try? h.close() }
        else { try? Data(line.utf8).write(to: log) }
    }
}
