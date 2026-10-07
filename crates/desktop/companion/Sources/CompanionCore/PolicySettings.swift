import Foundation

/// Every Settings-window value (130 §4.6), one `UserDefaults` key each. W1's `undoDelay` and `railSide`
/// keys are kept verbatim so the Store and the rail keep reading them.
public struct CompanionSettings: Equatable, Sendable {
    public var railSide = "right"
    /// `NSScreen.localizedName`; empty = the first screen.
    public var railDisplay = ""
    /// W6 (R2-3): expanded · sliver when quiet (W1's default) · always a sliver.
    public var railPresence = RailPresence.sliverWhenQuiet
    /// OFF by default: turning it on asks for Accessibility (R1-3).
    public var doubleTapOption = false
    public var undoDelay: TimeInterval = 3
    public var notify = NotificationPrefs()
    public var launchAtLogin = false
    /// W6 (R2-4): every key binding's override, one table for the grammar, the legend and Settings › Keys.
    public var keys = KeyTable()

    public init() {}

    public enum Key: String, CaseIterable {
        case railSide, railDisplay, railCollapse, railPresence, doubleTapOption, undoDelay
        case notifyBlocking, notifyDeferrable, notifyError, soundOnErrors, launchAtLogin, keyOverrides
    }

    public static func load(from d: UserDefaults) -> CompanionSettings {
        var s = CompanionSettings()
        s.railSide = d.string(forKey: Key.railSide.rawValue) ?? s.railSide
        s.railDisplay = d.string(forKey: Key.railDisplay.rawValue) ?? ""
        // W2's "collapse when quiet" toggle (`railCollapse`) migrates: off → expanded.
        if let p = d.string(forKey: Key.railPresence.rawValue).flatMap(RailPresence.init(rawValue:)) { s.railPresence = p }
        else if d.object(forKey: Key.railCollapse.rawValue) != nil, !d.bool(forKey: Key.railCollapse.rawValue) { s.railPresence = .expanded }
        s.doubleTapOption = d.bool(forKey: Key.doubleTapOption.rawValue)
        if let v = d.object(forKey: Key.undoDelay.rawValue) as? Double { s.undoDelay = min(5, max(0, v)) }
        s.notify.blocking = mode(d, .notifyBlocking) ?? s.notify.blocking
        s.notify.deferrable = mode(d, .notifyDeferrable) ?? s.notify.deferrable
        s.notify.error = mode(d, .notifyError) ?? s.notify.error
        if d.object(forKey: Key.soundOnErrors.rawValue) != nil { s.notify.soundOnErrors = d.bool(forKey: Key.soundOnErrors.rawValue) }
        s.launchAtLogin = d.bool(forKey: Key.launchAtLogin.rawValue)
        if let j = d.string(forKey: Key.keyOverrides.rawValue) { s.keys = KeyTable(json: j) }
        return s
    }

    public func save(to d: UserDefaults) {
        d.set(railSide, forKey: Key.railSide.rawValue)
        d.set(railDisplay, forKey: Key.railDisplay.rawValue)
        d.set(railPresence.rawValue, forKey: Key.railPresence.rawValue)
        d.set(doubleTapOption, forKey: Key.doubleTapOption.rawValue)
        d.set(min(5, max(0, undoDelay)), forKey: Key.undoDelay.rawValue)
        d.set(notify.blocking.rawValue, forKey: Key.notifyBlocking.rawValue)
        d.set(notify.deferrable.rawValue, forKey: Key.notifyDeferrable.rawValue)
        d.set(notify.error.rawValue, forKey: Key.notifyError.rawValue)
        d.set(notify.soundOnErrors, forKey: Key.soundOnErrors.rawValue)
        d.set(launchAtLogin, forKey: Key.launchAtLogin.rawValue)
        d.set(keys.json, forKey: Key.keyOverrides.rawValue)
    }

    /// `~/.theuth/companion.json`, read by the Tauri shell at start (R1-2).
    public var loginItemJSON: Data {
        Data("{ \"launch_at_login\": \(launchAtLogin) }\n".utf8)
    }

    private static func mode(_ d: UserDefaults, _ k: Key) -> NotifyMode? {
        d.string(forKey: k.rawValue).flatMap(NotifyMode.init(rawValue:))
    }
}
