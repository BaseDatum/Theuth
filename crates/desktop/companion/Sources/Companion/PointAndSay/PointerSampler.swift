import AppKit
import CompanionCore
import TheuthKit

/// 149 §3.7: the pointer at 60 Hz on a `.common` Timer (it keeps ticking while a menu or drag tracks). `NSEvent.mouseLocation`
/// (AppKit, bottom-left origin) becomes the engine's logical global point (top-left origin of the primary display,
/// `pointer.rs:3-5`); no event tap, no TCC (D20). `pressedMouseButtons & 1` is the pen in `annotate = mouse`.
@MainActor
final class PointerSampler {
    /// 120 s at 60 Hz, as the shell's `MAX_SAMPLES` (`DESK/point/mod.rs:42`).
    static let maxSamples = Int(PointMachine.sampleHz) * 120

    private(set) var samples: [PointerSample] = []
    private var timer: Timer?
    /// Every tick with the sample it produced (`nil` when the pointer is on no display) and the raw global point.
    var onTick: ((PointerSample?, CGPoint) -> Void)?

    /// Global logical, top-left origin — what the engine and `OverlayModel` speak.
    static func flipped(_ p: NSPoint) -> CGPoint {
        CGPoint(x: p.x, y: (NSScreen.screens.first?.frame.maxY ?? 0) - p.y)
    }

    /// `displays` as `OverlayModel` wants them: `NSScreen.frame` with the y flipped against the primary's top edge.
    static func displays() -> [OverlayDisplay] {
        let top = NSScreen.screens.first?.frame.maxY ?? 0
        return NSScreen.screens.map { s in
            OverlayDisplay(id: s.displayID, frame: CGRect(x: s.frame.minX, y: top - s.frame.maxY, width: s.frame.width, height: s.frame.height))
        }
    }

    /// `t0`: the press, on the sampler's clock (`DispatchTime` ms); `penAlways`: `annotate = always`.
    func start(t0: UInt64, penAlways: Bool, displays: [OverlayDisplay]) {
        stop()
        samples = []
        let t = Timer(timeInterval: 1 / Double(PointMachine.sampleHz), repeats: true) { [weak self] _ in
            Task { @MainActor in self?.tick(t0: t0, penAlways: penAlways, displays: displays) }
        }
        RunLoop.main.add(t, forMode: .common)
        timer = t
    }

    /// The samples so far; the sampler stops.
    @discardableResult
    func stop() -> [PointerSample] {
        timer?.invalidate(); timer = nil
        return samples
    }

    private func tick(t0: UInt64, penAlways: Bool, displays: [OverlayDisplay]) {
        let p = Self.flipped(NSEvent.mouseLocation)
        guard displays.contains(where: { $0.frame.contains(p) }) else { return onTick?(nil, p) ?? () }
        let pen = penAlways || NSEvent.pressedMouseButtons & 1 == 1
        let t = UInt32(clamping: PointClock.now() &- t0)
        let s = PointerSample(tMs: t, x: Float(p.x), y: Float(p.y), pen: pen)
        if samples.count < Self.maxSamples { samples.append(s) }
        onTick?(s, p)
    }
}

/// ms since boot — the machine's clock (`t0` is the press on it), monotonic, never adjusted.
enum PointClock {
    static func now() -> UInt64 { DispatchTime.now().uptimeNanoseconds / 1_000_000 }
}

extension NSScreen {
    /// The display's `CGDirectDisplayID` as a stable string key (per-screen overlay windows and `OverlayDisplay.id`).
    var displayID: String {
        let n = deviceDescription[NSDeviceDescriptionKey("NSScreenNumber")] as? NSNumber
        return n.map { "\($0.uint32Value)" } ?? localizedName
    }
}
