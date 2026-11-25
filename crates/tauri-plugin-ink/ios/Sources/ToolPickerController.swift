// 218 §6: the system tool picker — monoline, pen, marker and the eraser, nothing else (no ruler, lasso or
// finger-drawing toggle: fingers always navigate, R2). Attached to the web view through its
// `pencilKitResponderState` (iPadOS 26), so the web view stays first responder and the keyboard keeps
// working. Selection changes go to JS as `tool` events.

import PencilKit
import UIKit

@available(iOS 26.0, *)
@MainActor
final class ToolPickerController: NSObject, PKToolPickerObserver {
  var onChange: ((ToolState) -> Void)?

  private let picker: PKToolPicker
  private weak var host: UIResponder?
  /// While JS is selecting (`set_tool`), the picker's change is JS's own echo: not reported.
  private var selecting = false

  private static func id(_ tool: InkTool) -> String { "ink.\(tool.rawValue)" }

  init(webview: UIResponder, initial: ToolState) {
    let colour = UIColor(inkHex: initial.colour)
    picker = PKToolPicker(toolItems: [
      PKToolPickerInkingItem(
        type: .monoline, color: colour, width: 2.4, identifier: Self.id(.monoline)),
      PKToolPickerInkingItem(type: .pen, color: colour, width: 2.8, identifier: Self.id(.pen)),
      PKToolPickerInkingItem(type: .marker, color: colour, width: 6, identifier: Self.id(.marker)),
      PKToolPickerEraserItem(type: .vector),
    ])
    super.init()
    picker.showsDrawingPolicyControls = false
    // Colours are the page's, already resolved for its theme.
    picker.colorUserInterfaceStyle = .light
    picker.addObserver(self)
    webview.pencilKitResponderState.activeToolPicker = picker
    webview.pencilKitResponderState.toolPickerVisibility = .visible
    host = webview
    select(initial)
  }

  func addObserver(_ observer: PKToolPickerObserver) {
    picker.addObserver(observer)
  }

  func detach() {
    picker.removeObserver(self)
    if let host = host {
      host.pencilKitResponderState.toolPickerVisibility = .inactive
      host.pencilKitResponderState.activeToolPicker = nil
    }
    host = nil
  }

  func setVisible(_ visible: Bool) {
    host?.pencilKitResponderState.toolPickerVisibility = visible ? .visible : .hidden
  }

  /// Select `state`'s item (always one of the picker's own items: a foreign item is not the picker's to
  /// select), then its colour and width through `selectedTool` (deprecated in favour of items, which have
  /// no setter for either; it updates the selected item).
  func select(_ state: ToolState) {
    selecting = true
    defer { selecting = false }
    if state.tool == .eraser {
      if let eraser = picker.toolItems.first(where: { $0 is PKToolPickerEraserItem }) {
        picker.selectedToolItem = eraser
      }
      return
    }
    if let item = picker.toolItems.first(where: { $0.identifier == Self.id(state.tool) }) {
      picker.selectedToolItem = item
    }
    picker.selectedTool = PKInkingTool(
      inkType(state.tool), color: UIColor(inkHex: state.colour), width: CGFloat(state.width))
  }

  /// What the picker has selected now.
  private func current() -> ToolState? {
    let item = picker.selectedToolItem
    if let eraser = item as? PKToolPickerEraserItem {
      return ToolState(
        tool: .eraser, colour: "#000000", width: Double(eraser.eraserTool.width))
    }
    if let inking = item as? PKToolPickerInkingItem {
      let tool = inking.inkingTool
      return ToolState(
        tool: inkTool(tool.inkType), colour: tool.color.inkHex, width: Double(tool.width))
    }
    return nil
  }

  nonisolated func toolPickerSelectedToolItemDidChange(_ toolPicker: PKToolPicker) {
    MainActor.assumeIsolated {
      guard !selecting, let state = current() else { return }
      onChange?(state)
    }
  }
}

/// Pencil double-tap and squeeze (Pencil 2 / Pro), reported with the user's preferred action
/// (Settings → Apple Pencil). The plugin does not act on them itself: JS owns the tool state and answers
/// with `set_tool` / `set_picker_visible`, so a gesture is applied exactly once.
@available(iOS 26.0, *)
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
