import AppKit
import CompanionCore
import TheuthKit

/// 153 §16 row 20: `--probe-command <heard|working|approval|done>` and `--probe-card <collapsed|expanded|failed>` — a recorded
/// desk sequence (with `purpose` strings) fed straight into the card's state, rendered not key, screenshotted with
/// `--shareable` to `/tmp/153-c1/153-<command|card>-<kind>.png`, quit. Nothing is sent, no hot key, no microphone
/// (`AskFlags.automated`), no TCC prompt.
@MainActor
enum CommandProbe {
    static let dir = URL(fileURLWithPath: "/tmp/153-c1", isDirectory: true)

    static func run(kind: String, card isCard: Bool, store: Store, controller c: PointController) {
        try? FileManager.default.createDirectory(at: dir, withIntermediateDirectories: true)
        try? FileManager.default.removeItem(at: PointLog.file153)
        let name = "153-\(isCard ? "card" : "command")-\(kind)"
        guard let state = isCard ? cardState(kind) : commandState(kind) else {
            PointLog.write("probe: unknown kind \(kind)")
            return NSApp.terminate(nil)
        }
        PointLog.write("probe \(name): shareable=\(Flags.shareable) automated=\(AskFlags.automated)")
        Task {
            try? await Task.sleep(for: .milliseconds(400))
            c.card.state = state
            c.card.show(near: NSEvent.mouseLocation, key: false)
            try? await Task.sleep(for: .milliseconds(700))
            let png = dir.appendingPathComponent("\(name).png")
            let shot = Process()
            shot.executableURL = URL(fileURLWithPath: "/usr/sbin/screencapture")
            shot.arguments = ["-x", "-o", "-l", "\(c.card.panel.windowNumber)", png.path]
            try? shot.run()
            shot.waitUntilExit()
            let size = (try? FileManager.default.attributesOfItem(atPath: png.path)[.size] as? Int) ?? 0
            PointLog.write("probe \(name): frame=\(c.card.panel.frame) visible=\(c.card.isVisible) expanded=\(state.expanded) png=\(png.path) bytes=\(size)")
            try? await Task.sleep(for: .milliseconds(300))
            NSApp.terminate(nil)
        }
    }

    /// The §10.3 sequence: the request, four `ToolCallStarted`s with `purpose`, two finished, one approval, the ✓ line.
    static func commandState(_ kind: String) -> AnswerCardState? {
        var s = AnswerCardState()
        s.begin(question: "move today's screenshots into Desktop/Shots and make a reminder to review them at six", kind: .command)
        s.session = "probe-desk"
        s.status = "heard → working"
        if kind == "heard" { return s }
        s.command?.observe(.toolCallStarted(id: "1", tool: "shell", purpose: "checking: ls ~/Desktop/*.png · 14 files"))
        s.command?.observe(.toolCallFinished(id: "1", ok: true))
        s.command?.observe(.toolCallStarted(id: "2", tool: "shell", purpose: "executing: mkdir -p ~/Desktop/Shots && mv ~/Desktop/*.png ~/Desktop/Shots/"))
        s.command?.observe(.toolCallFinished(id: "2", ok: true))
        s.command?.observe(.toolCallStarted(id: "3", tool: "shell", purpose: "verifying: ls ~/Desktop/Shots | wc -l"))
        s.command?.observe(.toolCallStarted(id: "4", tool: "applescript", purpose: "executing: Reminders “Review shots” 18:00"))
        s.tools = ["shell", "shell", "shell", "applescript"]
        s.status = "working · applescript"
        switch kind {
        case "working": return s
        case "approval":
            s.approval = .init(id: "ap-1", tool: "shell", target: "rm -rf ~/Develop/theuth/build", risk: "destructive")
            s.approval?.listening = true
            s.status = "needs your ok"
            return s
        case "done":
            s.command?.observe(.toolCallFinished(id: "3", ok: true))
            s.command?.observe(.toolCallFinished(id: "4", ok: true))
            s.command?.observe(.textDelta("✓ 14 screenshots moved to ~/Desktop/Shots; reminder “Review shots” set for 18:00."))
            s.command?.observe(.turnFinished(stopReason: "end_turn"))
            s.phase = .answer; s.turnFinished = true; s.status = "done · 6 s"
            return s
        default: return nil
        }
    }

    /// The §12.2 anatomies for the ask card.
    static func cardState(_ kind: String) -> AnswerCardState? {
        var s = AnswerCardState()
        s.begin(question: "why does the onset test fail", kind: .voiceNote)
        s.session = "probe-desk"
        s.tools = ["read_file", "code_symbol", "cargo test -p voice"]
        s.text = """
        The failing test is `vad::gate_onset`. The onset run is now 8 frames (160 ms) but the fixture feeds 7.

        - fix the fixture → vad.rs:112
        - or keep 7 and lower ONSET_MS to 140

        Recommend the first; it matches design 54 §0.3.

        ```
        cargo test -p voice vad::gate_onset
        ```
        """
        switch kind {
        case "collapsed":
            s.phase = .answer; s.turnFinished = true; s.status = "replied just now"
            return s
        case "expanded":
            s.phase = .answer; s.turnFinished = true; s.status = "replied"
            s.followUp(question: "and the Windows twin?")
            s.text = "The C# port feeds the same fixture; `PointMachine.cs` mirrors the 8-frame onset once `vad.rs:112` lands."
            s.tools = ["read_file"]
            s.phase = .answer; s.turnFinished = true; s.status = "replied"
            s.expanded = true
            s.current = 1
            return s
        case "failed":
            s.text = ""
            s.tools = ["read_file"]
            s.phase = .failed; s.turnFinished = true; s.status = "couldn't reply — the turn failed"
            return s
        default: return nil
        }
    }
}
