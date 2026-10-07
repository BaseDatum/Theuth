import AppKit

/// The outer theuth.app this companion is nested in (Contents/Library/LoginItems/Theuth Companion.app), by bundle id.
enum MainApp {
    static let bundleID = "io.theuth.mac"

    /// The running main app, if any (several copies → the first).
    static func running() -> NSRunningApplication? {
        NSRunningApplication.runningApplications(withBundleIdentifier: bundleID).first { !$0.isTerminated }
    }

    /// The outer bundle: four levels up from this bundle when nested; else Launch Services' answer for the id.
    static func bundleURL() -> URL? {
        let nested = Bundle.main.bundleURL
            .deletingLastPathComponent() // LoginItems
            .deletingLastPathComponent() // Library
            .deletingLastPathComponent() // Contents
            .deletingLastPathComponent() // theuth.app
        if nested.pathExtension == "app", Bundle(url: nested)?.bundleIdentifier == bundleID { return nested }
        return NSWorkspace.shared.urlForApplication(withBundleIdentifier: bundleID)
    }
}
