// 218 §6: the system tool picker — monoline, pen, marker and the eraser, nothing else (no ruler, lasso or
// finger-drawing toggle: fingers always navigate, R2). Attached to the web view through its
// `pencilKitResponderState` (iPadOS 26), so the web view stays first responder and the keyboard keeps
// working. Selection changes go to JS as `tool` events.
//
// Never the deprecated `selectedTool` setter: on a picker built with `init(toolItems:)` it looks the tool
// up among the items and, when none matches (another colour or width), PencilKit raises NSRangeException
// (NSNotFound index) — an ObjC exception Swift cannot catch, which killed the app on whiteboard open
// (iPadOS 27.2.1). Items are immutable, so a colour or width change rebuilds the picker's items, and
// selection only ever names one of the picker's own items, by identifier.

import PencilKit
import UIKit

@available(iOS 26.0, *)
@MainActor
final class ToolPickerController: NSObject, PKToolPickerObserver {
  var onChange: ((ToolState) -> Void)?

  private var picker: PKToolPicker
  private weak var host: UIResponder?
  /// While JS is selecting (`set_tool`), the picker's change is JS's own echo: not reported.
  private var selecting = false
  private var visible = true
  /// Observers besides `self` (the route A canvas), re-added to a rebuilt picker.
  private let observers = NSHashTable<AnyObject>.weakObjects()
  /// What the current items were built with: one colour for all inking items, a width per tool.
  private var colour: String
  private var widths: [InkTool: Double] = [.monoline: 2.4, .pen: 2.8, .marker: 6]

  static func id(_ tool: InkTool) -> String { "ink.\(tool.rawValue)" }

  static func makePicker(colour hex: String, widths: [InkTool: Double]) -> PKToolPicker {
    let colour = UIColor(inkHex: hex)
    func width(_ tool: InkTool) -> CGFloat { CGFloat(widths[tool] ?? 2.8) }
    return PKToolPicker(toolItems: [
      PKToolPickerInkingItem(
        type: .monoline, color: colour, width: width(.monoline), identifier: id(.monoline)),
      PKToolPickerInkingItem(type: .pen, color: colour, width: width(.pen), identifier: id(.pen)),
      PKToolPickerInkingItem(
        type: .marker, color: colour, width: width(.marker), identifier: id(.marker)),
      PKToolPickerEraserItem(type: .vector),
    ])
  }

  init(webview: UIResponder, initial: ToolState) {
    colour = initial.colour
    if initial.tool != .eraser, initial.width.isFinite, initial.width > 0 {
      widths[initial.tool] = initial.width
    }
    picker = Self.makePicker(colour: colour, widths: widths)
    super.init()
    configure(picker)
    host = webview
    webview.pencilKitResponderState.activeToolPicker = picker
    webview.pencilKitResponderState.toolPickerVisibility = .visible
    select(initial)
  }

  private func configure(_ picker: PKToolPicker) {
    picker.showsDrawingPolicyControls = false
    // Colours are the page's, already resolved for its theme.
    picker.colorUserInterfaceStyle = .light
    picker.addObserver(self)
    for observer in observers.allObjects {
      if let observer = observer as? PKToolPickerObserver { picker.addObserver(observer) }
    }
  }

  func addObserver(_ observer: PKToolPickerObserver) {
    observers.add(observer)
    picker.addObserver(observer)
  }

  func detach() {
    picker.removeObserver(self)
    for observer in observers.allObjects {
      if let observer = observer as? PKToolPickerObserver { picker.removeObserver(observer) }
    }
    if let host = host {
      host.pencilKitResponderState.toolPickerVisibility = .inactive
      host.pencilKitResponderState.activeToolPicker = nil
    }
    host = nil
  }

  func setVisible(_ visible: Bool) {
    self.visible = visible
    host?.pencilKitResponderState.toolPickerVisibility = visible ? .visible : .hidden
  }

  /// Select `state`'s item. A colour or width the items were not built with rebuilds them first (items
  /// have no setters). Selection names only an identifier the picker has; anything else is a no-op.
  func select(_ state: ToolState) {
    selecting = true
    defer { selecting = false }
    if state.tool != .eraser, state.width.isFinite, state.width > 0,
      state.colour.lowercased() != colour.lowercased() || widths[state.tool] != state.width
    {
      colour = state.colour
      widths[state.tool] = state.width
      rebuild()
    }
    let item = picker.toolItems.first { candidate in
      state.tool == .eraser
        ? candidate is PKToolPickerEraserItem : candidate.identifier == Self.id(state.tool)
    }
    guard let item = item, picker.selectedToolItem !== item else { return }
    picker.selectedToolItem = item
  }

  /// A new picker with the current colour and widths, swapped in on the responder with the same
  /// observers and visibility.
  private func rebuild() {
    let old = picker
    old.removeObserver(self)
    for observer in observers.allObjects {
      if let observer = observer as? PKToolPickerObserver { old.removeObserver(observer) }
    }
    picker = Self.makePicker(colour: colour, widths: widths)
    configure(picker)
    guard let host = host else { return }
    host.pencilKitResponderState.activeToolPicker = picker
    host.pencilKitResponderState.toolPickerVisibility = visible ? .visible : .hidden
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
