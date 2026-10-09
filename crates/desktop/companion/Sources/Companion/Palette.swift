import AppKit
import SwiftUI

/// 149n — fully native: system semantic colours only. The web palette (bone/soot, gold, faience, malachite, carnelian,
/// sand) is gone; see docs/design/149-as-built-native-look.md for the mapping. Backgrounds are materials, never fills.
extension Color {
    static let tertiary = Color(nsColor: .tertiaryLabelColor)
    static let separator = Color(nsColor: .separatorColor)
    /// Status: red = failed / stopped, green = done / sent, orange = waiting. Accent = needs you / selection.
    static let failed = Color(nsColor: .systemRed)
    static let done = Color(nsColor: .systemGreen)
    static let waiting = Color(nsColor: .systemOrange)
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

// MARK: 149n — the visual language, native

/// System text styles only (Dynamic Type sizes on macOS: title2 17 · title3 15 · body 13 · callout 12 · caption 10).
enum Type {
    static let headline = Font.title2.weight(.semibold)
    static let question = Font.title3.weight(.semibold)
    static let body = Font.body
    static let meta = Font.callout
    static let caps = Font.caption.weight(.medium)
    /// Key caps, counts, ids.
    static let mono = Font.caption.monospaced()
    /// Standard macOS radii: panels/cards 12, option rows 10, buttons 6, key caps 4. Rail pills and the Ask bar are capsules.
    static let cardRadius: CGFloat = 12
    static let optionRadius: CGFloat = 10
    static let buttonRadius: CGFloat = 6
    static let capRadius: CGFloat = 4
}

/// Accent level bars from `DictationLevel.levels` (D8): one bar per band, newest right, 20 ms per frame as 54 §8's
/// meter. Replaces the mic glyph in the Ask bar and the rail's actions pill while a dictation records.
struct LevelBars: View {
    var levels: [Float]
    var height: CGFloat = 14
    var barWidth: CGFloat = 2
    var spacing: CGFloat = 1.5

    var body: some View {
        HStack(alignment: .center, spacing: spacing) {
            ForEach(levels.indices, id: \.self) { i in
                RoundedRectangle(cornerRadius: 1).fill(Color.accentColor)
                    .frame(width: barWidth, height: max(2, CGFloat(min(1, max(0, levels[i]))) * height))
            }
        }
        .frame(height: height)
        .animation(.linear(duration: 0.02), value: levels)
        .accessibilityLabel("Listening")
    }
}

/// 149 W1 probes (§10): `--probe-rail N` · `--probe-agents N` · `--probe-panel <finished|ask|approval|review|empty>`;
/// every one `--shareable`, seeded fixtures in place of the engine, never acting; log `/tmp/149-W1-<probe>.log`.
extension Flags {
    static let probeRail: TimeInterval? = {
        guard let i = args.firstIndex(of: "--probe-rail"), i + 1 < args.count else { return nil }
        return TimeInterval(args[i + 1])
    }()
    static let probeAgents: TimeInterval? = {
        guard let i = args.firstIndex(of: "--probe-agents"), i + 1 < args.count else { return nil }
        return TimeInterval(args[i + 1])
    }()
    static let probePanel: String? = {
        guard let i = args.firstIndex(of: "--probe-panel"), i + 1 < args.count else { return nil }
        return args[i + 1]
    }()
    static var w1Probe: Bool { probeRail != nil || probeAgents != nil || probePanel != nil }
    static var w1ProbeName: String? { probeRail != nil ? "rail" : probeAgents != nil ? "agents" : probePanel != nil ? "panel" : nil }
}

/// stderr plus `/tmp/149-W1-<probe>.log` during a W1 probe.
enum W1Log {
    static func write(_ s: String) {
        Log.write(s)
        guard let name = Flags.w1ProbeName else { return }
        let file = URL(fileURLWithPath: "/tmp/149-W1-\(name).log")
        let line = "[companion \(Date()) pid \(getpid())] \(s)\n"
        if let h = try? FileHandle(forWritingTo: file) { h.seekToEndOfFile(); h.write(Data(line.utf8)); try? h.close() }
        else { try? Data(line.utf8).write(to: file) }
    }
}
