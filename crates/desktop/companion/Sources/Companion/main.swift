import AppKit

// Revision 1: a second instance (an `open` while the login item runs) exits 0 at once. 149 W1: a probe (`--probe-*`) is a
// deliberate second instance for N seconds — it quits itself, takes no key, acts on nothing, and skips `Hotkeys.start`.
if !Flags.anyProbe, let id = Bundle.main.bundleIdentifier,
   NSRunningApplication.runningApplications(withBundleIdentifier: id).count > 1 {
    FileHandle.standardError.write(Data("[companion] another instance is running — exiting 0\n".utf8))
    exit(0)
}

let app = NSApplication.shared
app.setActivationPolicy(.accessory)
let delegate = MainActor.assumeIsolated { AppDelegate() }
app.delegate = delegate
app.run()
