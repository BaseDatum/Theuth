// 218: Pencil double-tap and squeeze for the web whiteboard. Ink itself is drawn by the web view.

import UIKit

/// Pencil double-tap and squeeze (Pencil 2 / Pro), reported with the user's preferred action
/// (Settings → Apple Pencil). The plugin does not act on them itself: JS owns the tool state, so a
/// gesture is applied exactly once. Squeeze and `preferredSqueezeAction` need iOS 17.5.
@available(iOS 17.5, *)
@MainActor
final class PencilController: NSObject, UIPencilInteractionDelegate {
  private let interaction: UIPencilInteraction
  private weak var view: UIView?
  private let report: (String, String) -> Void

  init(view: UIView, report: @escaping (String, String) -> Void) {
    self.report = report
    interaction = UIPencilInteraction()
    super.init()
    interaction.delegate = self
    view.addInteraction(interaction)
    self.view = view
  }

  func detach() {
    view?.removeInteraction(interaction)
    view = nil
  }

  nonisolated func pencilInteraction(
    _ interaction: UIPencilInteraction, didReceiveTap tap: UIPencilInteraction.Tap
  ) {
    MainActor.assumeIsolated {
      report("tap", Self.action(UIPencilInteraction.preferredTapAction))
    }
  }

  nonisolated func pencilInteraction(
    _ interaction: UIPencilInteraction, didReceiveSqueeze squeeze: UIPencilInteraction.Squeeze
  ) {
    MainActor.assumeIsolated {
      guard squeeze.phase == .ended else { return }
      report("squeeze", Self.action(UIPencilInteraction.preferredSqueezeAction))
    }
  }

  private static func action(_ action: UIPencilPreferredAction) -> String {
    switch action {
    case .switchEraser: return "switchEraser"
    case .switchPrevious: return "switchPrevious"
    case .showColorPalette: return "showColorPalette"
    case .showInkAttributes: return "showInkAttributes"
    case .showContextualPalette: return "showContextualPalette"
    case .runSystemShortcut: return "runSystemShortcut"
    case .ignore: return "ignore"
    @unknown default: return "ignore"
    }
  }
}
