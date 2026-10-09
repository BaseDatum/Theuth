import Foundation
import TheuthKit

/// 153 §13 / §9.1: which `AppProfile` a gesture in `bundleId` runs with. The rows live in `ModeKeySettings.appProfiles`
/// (CC, JSON `[AppProfile]`); `TheuthKit.AppProfile` is the wire shape sent as `DictationOpts.app`.
public enum AppProfiles {
    /// The "non-agent apps" group default row (§9.1): matched when no row has the bundle id.
    public static let anyAppId = "any_app"

    /// Terminals and the editors whose integrated terminal / Monaco editor must take the lease: `unknown` + terminal spacing
    /// (§4.2 rule 9) unless a row says otherwise.
    public static let terminalBundleIds: Set<String> = [
        "com.apple.Terminal", "com.googlecode.iterm2", "com.mitchellh.ghostty", "dev.warp.Warp-Stable", "dev.warp.Warp",
        "net.kovidgoyal.kitty", "org.alacritty", "io.alacritty", "com.github.wez.wezterm",
        "com.microsoft.VSCode", "com.microsoft.VSCodeInsiders", "com.todesktop.230313mzl4w4u92" /* Cursor */, "dev.zed.Zed", "dev.zed.Zed-Preview",
    ]

    public static func isTerminal(bundleId: String?) -> Bool {
        guard let bundleId else { return false }
        return terminalBundleIds.contains(bundleId)
    }

    /// Resolution: the row whose `id` is `bundleId` → else the `any_app` row (with `terminal` from the heuristics when the
    /// app is a known terminal) → else a synthesized row: `terminal` from the heuristics, `polish`/`learn` off, no commands.
    /// `name` is the app's localized name for a synthesized row (the card title "Couldn't type into Terminal").
    public static func resolve(bundleId: String?, name: String? = nil, in profiles: [AppProfile]) -> AppProfile {
        let terminal = isTerminal(bundleId: bundleId)
        if let bundleId, let row = profiles.first(where: { $0.id == bundleId }) { return row }
        if var row = profiles.first(where: { $0.id == anyAppId }) {
            row.id = bundleId ?? anyAppId
            if let name { row.name = name }
            row.terminal = row.terminal || terminal
            return row
        }
        return AppProfile(id: bundleId ?? anyAppId, name: name ?? bundleId ?? "any app", terminal: terminal)
    }
}
