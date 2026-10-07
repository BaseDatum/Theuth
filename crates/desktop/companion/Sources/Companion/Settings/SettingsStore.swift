import AppKit
import Combine
import CompanionCore
import TheuthKit

/// W2: the persisted `CompanionSettings` (suite `io.theuth.mac.companion`, 130 §4.6) as one observable the
/// Settings window edits and the Store/rail/policy read live. From the bundle `UserDefaults.standard` IS
/// that suite (W1's `undoDelay`/`railSide` readers keep working); the bare executable gets the named suite.
extension Settings {
    static let suite = "io.theuth.mac.companion"
    static let defaults: UserDefaults = Bundle.main.bundleIdentifier == suite
        ? .standard
        : (UserDefaults(suiteName: suite) ?? .standard)

    /// `<data_dir>/companion.json`, read by the Tauri shell (R1-2).
    static var loginItemFile: URL { EngineLocation.dataDir().appendingPathComponent("companion.json") }
}

@MainActor
final class SettingsModel: ObservableObject {
    static let shared = SettingsModel()

    @Published var value: CompanionSettings {
        didSet {
            value.save(to: Settings.defaults)
            if value.launchAtLogin != oldValue.launchAtLogin { writeLoginItem() }
            if value.keys != oldValue.keys { Hotkeys.apply(value.keys) }
        }
    }
    /// W6: "⌃⇧X is held by another app — using ⌃⇧Space"; shown in Settings › Keys and the legend, never a dialog.
    @Published var hotkeyNotice: String?
    /// W6: the recorder's verdict on the last chord typed into a row (collision, reserved, held by another app).
    @Published var rowNotice: [KeyAction: RowNotice] = [:]

    var keys: KeyTable {
        get { value.keys }
        set { value.keys = newValue }
    }

    private init() {
        value = CompanionSettings.load(from: Settings.defaults)
    }

    /// The shell registers/unregisters `SMAppService` from this file at its next start (R1-2).
    func writeLoginItem() {
        do {
            try FileManager.default.createDirectory(at: Settings.loginItemFile.deletingLastPathComponent(), withIntermediateDirectories: true)
            try value.loginItemJSON.write(to: Settings.loginItemFile, options: .atomic)
            Log.write("settings: wrote \(Settings.loginItemFile.path) launch_at_login=\(value.launchAtLogin)")
        } catch {
            Log.write("settings: companion.json write failed: \(error)")
        }
    }
}

/// W2 switches. `--probe-notify N`: dry-run the policy against the live engine for N s (log only, no
/// authorization request), show Settings for 3 s, screenshot, quit. `--request-notifications`: the ONE
/// non-UI place the authorization dialog may be shown — the user runs it.
extension Flags {
    static let requestNotifications = args.contains("--request-notifications")
    static let probeNotify: TimeInterval? = {
        guard let i = args.firstIndex(of: "--probe-notify"), i + 1 < args.count else { return nil }
        return TimeInterval(args[i + 1])
    }()
    /// Any probe: the surfaces must not take key, nothing may be answered.
    static var anyProbe: Bool { probe != nil || probeNotify != nil || w6Probe }
}
