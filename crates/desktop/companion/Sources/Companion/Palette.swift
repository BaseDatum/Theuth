import AppKit
import SwiftUI

/// theuth's palette as dark/light pairs, hex values verbatim from `crates/desktop/ui/src/styles.css`.
/// Gold is used ONLY for needs-you marks, the count and the wordmark (C7).
enum Palette {
    static let gold = pair(dark: 0xc47a2c, light: 0xb4531e)
    static let faience = pair(dark: 0x5b8f8a, light: 0x3d6b66)
    static let malachite = pair(dark: 0x4a8a6e, light: 0x2f5d4a)
    static let carnelian = pair(dark: 0xc45a48, light: 0x9b2d1f)
    /// `--fg-tertiary`: sand / dust — the lapsed ring and dim text.
    static let sand = pair(dark: 0x9a8b72, light: 0x8f7f6a)
    /// `--fg-primary`: bone on soot / soot on bone.
    static let text = pair(dark: 0xe8dcc8, light: 0x1a1612)
    /// `--fg-secondary`.
    static let text2 = pair(dark: 0xc9b89a, light: 0x6b5e4f)
    /// `--bg-layer-1`: the rail's chrome.
    static let surface = pair(dark: 0x1c1915, light: 0xf3eadf)
    /// `--border-strong`.
    static let border = pair(dark: 0x3a3228, light: 0xc9b48a)

    static func pair(dark: UInt32, light: UInt32) -> NSColor {
        NSColor(name: nil) { appearance in
            let isDark = appearance.bestMatch(from: [.darkAqua, .aqua]) == .darkAqua
            return NSColor(hex: isDark ? dark : light)
        }
    }
}

extension NSColor {
    convenience init(hex: UInt32, alpha: CGFloat = 1) {
        self.init(srgbRed: CGFloat((hex >> 16) & 0xff) / 255, green: CGFloat((hex >> 8) & 0xff) / 255,
                  blue: CGFloat(hex & 0xff) / 255, alpha: alpha)
    }
}

extension Color {
    static let gold = Color(nsColor: Palette.gold)
    static let faience = Color(nsColor: Palette.faience)
    static let malachite = Color(nsColor: Palette.malachite)
    static let carnelian = Color(nsColor: Palette.carnelian)
    static let sand = Color(nsColor: Palette.sand)
    static let ink = Color(nsColor: Palette.text)
    static let ink2 = Color(nsColor: Palette.text2)
    static let surface = Color(nsColor: Palette.surface)
    static let rule = Color(nsColor: Palette.border)
}

/// stderr plus `/tmp/130-w1-probe.log` during `--probe` and `/tmp/130-w6-probe.log` during a W6 probe (`open`
/// gives the app no stdout).
enum Log {
    static let file = URL(fileURLWithPath: "/tmp/130-w1-probe.log")
    static let w6File = URL(fileURLWithPath: "/tmp/130-w6-probe.log")
    private static let f: DateFormatter = { let f = DateFormatter(); f.dateFormat = "HH:mm:ss.SSS"; return f }()

    static func write(_ s: String) {
        let line = "[companion \(f.string(from: Date())) pid \(getpid())] \(s)\n"
        FileHandle.standardError.write(Data(line.utf8))
        let file: URL
        if Flags.probe != nil { file = Self.file } else if Flags.w6Probe { file = w6File } else { return }
        if let h = try? FileHandle(forWritingTo: file) {
            h.seekToEndOfFile(); h.write(Data(line.utf8)); try? h.close()
        } else {
            try? Data(line.utf8).write(to: file)
        }
    }
}

/// Command-line switches. `--probe N`: connect, log counts, show the surfaces for N s, quit; never activates,
/// never answers. `--shareable`: `sharingType = .readOnly` so `screencapture` can see the panels (spike SA).
/// W6: `--probe-settings <tab>` shows Settings on that tab (not key) for 3 s and quits 0; `--probe-legend` the
/// keyboard-shortcuts legend likewise. Both log to `/tmp/130-w6-probe.log` (the screenshot is the shell's job).
enum Flags {
    static let args = CommandLine.arguments
    static let shareable = args.contains("--shareable")
    static let probe: TimeInterval? = {
        guard let i = args.firstIndex(of: "--probe"), i + 1 < args.count else { return nil }
        return TimeInterval(args[i + 1])
    }()
    static let probeSettings: String? = {
        guard let i = args.firstIndex(of: "--probe-settings"), i + 1 < args.count else { return nil }
        return args[i + 1]
    }()
    static let probeLegend = args.contains("--probe-legend")
    static var w6Probe: Bool { probeSettings != nil || probeLegend }
}

/// UserDefaults-backed settings for this wave (D12 window is W2).
enum Settings {
    static var undoDelay: TimeInterval {
        let v = UserDefaults.standard.object(forKey: "undoDelay") as? Double ?? 3
        return min(5, max(0, v))
    }
    /// `"right"` (default) or `"left"`.
    static var railSide: String { UserDefaults.standard.string(forKey: "railSide") ?? "right" }
}
