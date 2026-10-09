import AppKit
import CompanionCore
import SwiftUI
import TheuthKit

/// 149i — `--probe-island clicks|stop|open-no-hover[-nonotch]`: synthetic input (`CGEvent` at `.cghidEventTap`) against the
/// probe's OWN island only — every target rect is logged before a post, the owner's companion is never touched. Each
/// control's handler is asserted (PASS/FAIL lines), PNGs go to `/tmp/149i-<scenario>-<step>.png`. Posting needs
/// Accessibility; untrusted, every input step is reported NOT PROVABLE. Settings the probe flips (the eye, the frames) are
/// restored before it quits.
extension IslandController {
    func runInputProbe(_ which: String) async {
        let savedEye = IslandSettings.eyeHidden, savedFrames = IslandSettings.frames, pointer = NSEvent.mouseLocation
        let trusted = AXIsProcessTrusted()
        var pass = 0, fail = 0
        func check(_ name: String, _ ok: Bool, _ detail: @autoclosure () -> String) {
            if ok { pass += 1 } else { fail += 1 }
            IslandLog.write("probe \(which) \(ok ? "PASS" : "FAIL") \(name): \(detail())")
        }
        func sleep(_ s: Double) async { try? await Task.sleep(for: .seconds(s)) }
        IslandLog.write("probe \(which): AXIsProcessTrusted=\(trusted) shareable=\(Flags.shareable) screens=\(NSScreen.screens.map { "\($0.localizedName) \($0.frame)" }) pointer=\(pointer) number=\(window.windowNumber)")

        // The pointer is parked far from the island first: the open layout must not depend on hover.
        guard let s = screen else { return IslandLog.write("probe \(which): no screen") }
        let park = NSPoint(x: s.visibleFrame.maxX - 40, y: s.visibleFrame.minY + 40)
        if trusted { post(.mouseMoved, at: park) }
        var slice = IslandFixture.slice()
        slice.briefs.append(Brief(id: "b2", topic: "t2", title: "Flipt DR — prepared, not deployed", bullets: ["flags mirrored to the DR cluster", "cut-over runbook in review"], source: .tap))
        slice.cursor = 1
        store.meeting = slice
        await sleep(2)
        let pane = drawnGlobal.height > IslandLayout.stripHeight + (ui.anchor?.rect.height ?? 0)
        check("open-no-hover", ui.isOpen && !ui.isDetached && pane, "state=\(ui.state) drawn=\(drawnGlobal) pointer=\(NSEvent.mouseLocation) park=\(park) visible=\(window.isVisible)")
        logProbeState(which)
        await shot("\(which)-start")

        if which.hasPrefix("stop") {
            // The wire's Done through the real decoder: `State{Done}` → close → `orderOut` within 1 s.
            let json = #"{"type":"State","meeting":"probe-meeting","state":"Done","degraded":[]}"#
            store.meetingObserve(Data(json.utf8))
            let st = (store.meeting?.state).map { "\($0)" } ?? "nil"
            IslandLog.write("probe \(which): fed \(json) → slice.state=\(st)")
            check("decoded Done", store.meeting?.state == .done, "state=\(st)")
            await sleep(1)
            check("hidden within 1 s", !window.isVisible && !shown, "visible=\(window.isVisible) shown=\(shown) state=\(ui.state)")
            await shot("\(which)-after")
        } else if which.hasPrefix("clicks") {
            if !ui.isOpen { IslandLog.write("probe \(which): the layout is not open — the clicks below have nothing to hit") }
            logRects()
            guard trusted else {
                check("synthetic input", false, "NOT PROVABLE: CGEventPost needs Accessibility, which this probe bundle lacks")
                return await finish(which, pass: pass, fail: fail, eye: savedEye, frames: savedFrames, pointer: pointer)
            }
            // Topic 2: the tap handler + the hover focus.
            await click("topic:t2")
            check("topic click", store.meeting?.tapped.contains("t2") == true && ui.focusedTopic == 1 && store.meetingActs.contains("meeting_tap(t2)"),
                  "tapped=\(store.meeting?.tapped ?? []) focusedTopic=\(ui.focusedTopic) acts=\(store.meetingActs)")
            // The eye: the click must land on the same path the ⌥⇧E hotkey takes (`ui.eyeHidden.toggle()` → `setEye` → its
            // "eye hidden=… → sharingType=…" log line); the owner saw the hotkey work and the click not.
            let eye = ui.eyeHidden
            let mark = (try? Data(contentsOf: IslandLog.file))?.count ?? 0
            await click("eye")
            let want: NSWindow.SharingType = (ui.eyeHidden && !Flags.shareable) ? .none : .readOnly
            let logged = String(decoding: ((try? Data(contentsOf: IslandLog.file)) ?? Data()).dropFirst(mark), as: UTF8.self)
            let line = "eye hidden=\(!eye) → sharingType=\(want.rawValue)"
            check("eye click", ui.eyeHidden != eye && window.sharingType == want && logged.contains(line),
                  "eyeHidden \(eye) → \(ui.eyeHidden) sharingType=\(window.sharingType.rawValue) (want \(want.rawValue); --shareable keeps readOnly) setEye logged \"\(line)\"=\(logged.contains(line))")
            await click("eye") // back to where it was
            // ⋮ → the menu → stop (stubbed: the store logs it, sends nothing).
            await click("menu")
            check("menu click", ui.menuOpen, "menuOpen=\(ui.menuOpen)")
            logRects(["pause", "stop", "menu-close"])
            await shot("\(which)-menu")
            await click("stop")
            check("stop click", store.meetingActs.contains("meeting_stop") && !ui.menuOpen, "acts=\(store.meetingActs) menuOpen=\(ui.menuOpen)")
            // The brief pane: older / newer walk the briefs; deeper asks.
            await click("older")
            check("older click", store.meeting?.cursor == 0, "cursor=\(store.meeting?.cursor ?? -1) brief=\(store.meeting?.current?.title ?? "none")")
            await click("newer")
            check("newer click", store.meeting?.cursor == 1, "cursor=\(store.meeting?.cursor ?? -1) brief=\(store.meeting?.current?.title ?? "none")")
            await click("deeper")
            check("deeper click", store.meetingActs.contains("meeting_deeper(b2)"), "acts=\(store.meetingActs)")
            // Hover topic 1 → its dismiss x appears → click it. A real hover is a sweep of moves, not one jump: the tracking
            // area wants an enter and movement inside before SwiftUI's `onHover` flips; a human wiggles, so up to three sweeps.
            if let r = rect("topic:t1") {
                for attempt in 1...3 where rect("dismiss:t1") == nil {
                    for i in 0...8 { post(.mouseMoved, at: NSPoint(x: r.minX - 10 + CGFloat(i) * (r.width / 2 + 10) / 8, y: r.midY)); await sleep(0.04) }
                    await sleep(0.7)
                    if rect("dismiss:t1") == nil { IslandLog.write("probe hover topic:t1: no dismiss after sweep \(attempt)"); post(.mouseMoved, at: NSPoint(x: r.minX - 40, y: r.midY)); await sleep(0.3) }
                }
            }
            logRects(["dismiss:t1"])
            await click("dismiss:t1")
            check("dismiss click", store.meetingActs.contains("meeting_dismiss(t1)"), "acts=\(store.meetingActs)")
            await click("transcript")
            check("transcript click", ui.transcriptOpen, "transcriptOpen=\(ui.transcriptOpen)")
            // A click on the strip's grip (no drag) collapses to the pill; a click on the live pill opens again (D1).
            await click("grip")
            let collapsed = ui.state == .closed && model.collapsed
            check("strip click collapses", collapsed, "state=\(ui.state) collapsed=\(model.collapsed)")
            await shot("\(which)-collapsed")
            logRects(["pill"])
            await click("pill")
            check("pill click opens", collapsed && ui.state == .open && !model.collapsed, "state=\(ui.state) collapsed=\(model.collapsed)")
            // The request box takes key on click; ⎋ hands it back.
            await click("box")
            check("box takes key", window.isKeyWindow && ui.boxFocused, "key=\(window.isKeyWindow) boxFocused=\(ui.boxFocused) active=\(NSApp.isActive)")
            postKey(53)
            await sleep(0.5)
            check("esc releases key", !window.isKeyWindow && !ui.boxFocused && ui.isOpen, "key=\(window.isKeyWindow) boxFocused=\(ui.boxFocused) state=\(ui.state)")
            // The grip: down, 10 moves of 20 px, up → detached ~200 px lower on this display; double-click → back.
            let uuid = IslandGeometry.uuid(of: s)
            if let g = rect("grip") {
                let before = drawnGlobal, start = NSPoint(x: g.midX, y: g.midY)
                IslandLog.write("probe \(which): drag grip from \(start) by (0, -200) in 10 steps; drawn=\(before) window=\(window.frame)")
                post(.mouseMoved, at: start); await sleep(0.1)
                post(.leftMouseDown, at: start); await sleep(0.1)
                for i in 1...10 { post(.leftMouseDragged, at: NSPoint(x: start.x, y: start.y - 20 * CGFloat(i))); await sleep(0.05) }
                post(.leftMouseUp, at: NSPoint(x: start.x, y: start.y - 200)); await sleep(1)
                let moved = before.maxY - window.frame.maxY
                check("grip drag detaches", ui.isDetached && abs(moved - 200) <= 40 && model.frames[uuid] != nil,
                      "state=\(ui.state) moved=\(moved) window=\(window.frame) frames[\(uuid)]=\(model.frames[uuid].map { "\($0)" } ?? "nil") dragPoll=\(dragPoll != nil)")
                await shot("\(which)-detached")
            } else {
                check("grip drag detaches", false, "no grip rect reported")
            }
            logRects(["grip-detached"])
            if let g = rect("grip-detached") {
                let p = NSPoint(x: g.midX, y: g.midY)
                post(.mouseMoved, at: p); await sleep(0.1)
                post(.leftMouseDown, at: p, clicks: 1); post(.leftMouseUp, at: p, clicks: 1); await sleep(0.08)
                post(.leftMouseDown, at: p, clicks: 2); post(.leftMouseUp, at: p, clicks: 2); await sleep(1)
                check("grip double-click snaps back", !ui.isDetached && model.frames[uuid] == nil && window.frame.maxY == s.frame.maxY,
                      "state=\(ui.state) window=\(window.frame) screenTop=\(s.frame.maxY) frames=\(model.frames)")
                await shot("\(which)-snapped")
            } else {
                check("grip double-click snaps back", false, "no detached grip rect reported")
            }
        }
        await finish(which, pass: pass, fail: fail, eye: savedEye, frames: savedFrames, pointer: pointer)
    }

    private func finish(_ which: String, pass: Int, fail: Int, eye: Bool, frames: [String: CGRect], pointer: NSPoint) async {
        if ui.isDetached { snapBack() }
        IslandSettings.eyeHidden = eye
        IslandSettings.frames = frames
        if AXIsProcessTrusted() { post(.mouseMoved, at: pointer) }
        IslandLog.write("probe \(which): \(pass)/\(pass + fail) PASS\(fail > 0 ? " (\(fail) FAIL)" : "") — settings restored (eyeHidden=\(eye) frames=\(frames.count)), pointer restored; quitting 0")
        await Task.yield()
        NSApp.terminate(nil)
    }

    // MARK: geometry

    /// A control's rect in global (AppKit, bottom-left) screen coordinates.
    private func rect(_ name: String) -> NSRect? {
        guard let r = ui.rects[name], r != .zero else { return nil }
        return flip(r).offsetBy(dx: window.frame.minX, dy: window.frame.minY)
    }

    /// Every (or the named) control's SwiftUI → AppKit → global rect, and what `hitTest` answers at its centre.
    private func logRects(_ names: [String]? = nil) {
        let keys = (names ?? Array(ui.rects.keys)).sorted()
        IslandLog.write("probe rects: window=\(window.frame) drawn(global)=\(drawnGlobal) hosting.drawn=\(hosting.drawn.map { "\($0)" } ?? "nil") hosting.isFlipped=\(hosting.isFlipped) hosting.frame=\(hosting.frame)")
        for k in keys {
            guard let r = rect(k), let sw = ui.rects[k] else { IslandLog.write("probe rect \(k): not reported"); continue }
            let local = flip(CGPoint(x: sw.midX, y: sw.midY))
            let hit = hosting.hitTest(local) // hosting.frame == contentView.bounds: its superview's coordinates are its own
            IslandLog.write("probe rect \(k): swiftui=\(sw) global=\(r) centre=\(NSPoint(x: r.midX, y: r.midY)) hitTest=\(hit.map { String(describing: type(of: $0)) } ?? "nil (click-through)")")
        }
    }

    // MARK: synthetic input (only ever at this probe's own window)

    private func click(_ name: String) async {
        guard let r = rect(name) else { return IslandLog.write("probe click \(name): no rect — nothing posted") }
        let p = NSPoint(x: r.midX, y: r.midY)
        IslandLog.write("probe click \(name) at \(p) (rect \(r))")
        post(.mouseMoved, at: p); try? await Task.sleep(for: .milliseconds(80))
        post(.leftMouseDown, at: p); try? await Task.sleep(for: .milliseconds(50))
        post(.leftMouseUp, at: p); try? await Task.sleep(for: .milliseconds(450))
    }

    private func post(_ type: CGEventType, at p: NSPoint, clicks: Int64 = 1) {
        let cg = CGPoint(x: p.x, y: NSScreen.screens[0].frame.maxY - p.y)
        guard let e = CGEvent(mouseEventSource: nil, mouseType: type, mouseCursorPosition: cg, mouseButton: .left) else { return }
        if type != .mouseMoved { e.setIntegerValueField(.mouseEventClickState, value: clicks) }
        e.post(tap: .cghidEventTap)
    }

    private func postKey(_ code: CGKeyCode) {
        IslandLog.write("probe key: code \(code) down/up at .cghidEventTap (key window=\(NSApp.keyWindow.map { "\($0.className)" } ?? "none"))")
        CGEvent(keyboardEventSource: nil, virtualKey: code, keyDown: true)?.post(tap: .cghidEventTap)
        CGEvent(keyboardEventSource: nil, virtualKey: code, keyDown: false)?.post(tap: .cghidEventTap)
    }

    /// `screencapture -l` of this window (needs `--shareable`); the owner's windows are never captured.
    private func shot(_ name: String) async {
        try? await Task.sleep(for: .milliseconds(300))
        let p = Process()
        p.executableURL = URL(fileURLWithPath: "/usr/sbin/screencapture")
        p.arguments = ["-x", "-o", "-l", "\(window.windowNumber)", "/tmp/149i-\(name).png"]
        do { try p.run() } catch { return IslandLog.write("probe shot \(name): \(error)") }
        IslandLog.write("probe shot: /tmp/149i-\(name).png (window \(window.windowNumber))")
        try? await Task.sleep(for: .milliseconds(400))
    }
}
