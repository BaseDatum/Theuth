import AppKit

// Revision 1: a second instance (an `open` while the login item runs) exits 0 at once.
if let id = Bundle.main.bundleIdentifier,
   NSRunningApplication.runningApplications(withBundleIdentifier: id).count > 1 {
    FileHandle.standardError.write(Data("[companion] another instance is running — exiting 0\n".utf8))
    exit(0)
}

let app = NSApplication.shared
app.setActivationPolicy(.accessory)
let delegate = MainActor.assumeIsolated { AppDelegate() }
app.delegate = delegate
app.run()
