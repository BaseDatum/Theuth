// 218 §6: the wire types (mirrors `src/models.rs`, whose golden tests pin the JSON) and small helpers.

import Foundation
import Tauri
import UIKit

/// World → web-view points: `screen = world × zoom + (tx, ty)`, CSS px from the web view's top-left.
struct NativeView: Codable, Equatable {
  var tx: Double
  var ty: Double
  var zoom: Double

  static let identity = NativeView(tx: 0, ty: 0, zoom: 1)
}

enum InkTool: String, Codable {
  case monoline, pen, marker, eraser
}

/// The current pen: what JS selected (`set_tool`) or the picker changed to.
struct ToolState: Equatable {
  var tool: InkTool
  /// `#rrggbb`.
  var colour: String
  /// Points.
  var width: Double

  static let initial = ToolState(tool: .pen, colour: "#1a1612", width: 2.8)
}

enum InkRoute: String {
  /// `PKCanvasView.drawingGestureRecognizer` moved onto the web view.
  case A
  /// The plugin's own pencil-only recogniser drawing `CAShapeLayer`s.
  case B
}

struct NativeStroke: Encodable {
  let seq: Int
  let tool: String
  let colour: String
  let width: Double
  let t0: Double
  let view: NativeView
  /// x, y (web-view CSS px), force 0…1, t (ms since `t0`) per point.
  let pts: [Double]
  let retry: Int
}

struct StrokeBatch: Encodable {
  let batch: Int
  let strokes: [NativeStroke]
}

struct StateEvent: Encodable {
  let kind = "state"
  let drawing: Bool
}

struct ToolEvent: Encodable {
  let kind = "tool"
  let tool: String
  let colour: String
  let width: Double
}

struct PencilEvent: Encodable {
  let kind = "pencil"
  let gesture: String
  let action: String
}

struct ErrorEvent: Encodable {
  let kind = "error"
  let message: String
}

/// A finished stroke as a capture surface hands it over: points in the web view's own coordinates.
struct CapturedInk {
  /// The surface's own id, for removal once acknowledged.
  let local: Int
  let tool: ToolState
  /// Ms since the Unix epoch at the first point.
  let t0: Double
  /// The viewport when the stroke began.
  let view: NativeView
  /// (point in the web view, force 0…1, ms since `t0`).
  let points: [(CGPoint, Double, Double)]
}

/// What the overlay needs from a surface at pen-down: the pen and viewport the stroke is drawn with.
struct BeginContext {
  let tool: ToolState
  let view: NativeView
}

/// The overlay as the plugin drives it; only `InkOverlay` (iPadOS 26) implements it.
@MainActor
protocol NativeInkOverlay: AnyObject {
  func attach(strokes: Channel, events: Channel) -> InkRoute?
  func detach()
  func setEnabled(_ enabled: Bool)
  func setViewport(_ view: NativeView)
  func setTool(_ tool: ToolState)
  func setPickerVisible(_ visible: Bool)
  func ack(_ seqs: [Int])
}

/// A place Pencil strokes are captured: route A (`CanvasSurface`) or route B (`ShapeSurface`).
@MainActor
protocol CaptureSurface: AnyObject {
  /// Pen down (true) or up (false).
  var onPen: ((Bool) -> Void)? { get set }
  /// A stroke finished; `take()` has it.
  var onStroke: (() -> Void)? { get set }
  /// The pen and viewport at pen-down.
  var context: (() -> BeginContext)? { get set }
  func mount(over webview: UIView) -> Bool
  func unmount()
  func setEnabled(_ enabled: Bool)
  func setTool(_ tool: ToolState)
  func raise()
  /// Strokes finished since the last call, in stroke order.
  func take() -> [CapturedInk]
  /// Remove acknowledged strokes from the screen.
  func remove(_ locals: Set<Int>)
}

/// Pencil force normalised to 0…1 (1.0 is an average press; §6 `min(1, force / 2)`).
func normalisedForce(_ force: CGFloat) -> Double {
  return Double(min(1, max(0, force / 2)))
}

/// Ms since the Unix epoch.
func nowMs() -> Double {
  return Date().timeIntervalSince1970 * 1000
}

extension UIColor {
  /// `#rrggbb` (alpha dropped), resolved in the light appearance the overlay draws in.
  var inkHex: String {
    let resolved = resolvedColor(with: UITraitCollection(userInterfaceStyle: .light))
    var r: CGFloat = 0
    var g: CGFloat = 0
    var b: CGFloat = 0
    var a: CGFloat = 0
    guard resolved.getRed(&r, green: &g, blue: &b, alpha: &a) else { return "#000000" }
    let byte = { (v: CGFloat) in Int((min(1, max(0, v)) * 255).rounded()) }
    return String(format: "#%02x%02x%02x", byte(r), byte(g), byte(b))
  }

  /// A `#rrggbb` colour; black when it does not parse.
  convenience init(inkHex hex: String) {
    var digits = hex.trimmingCharacters(in: .whitespaces)
    if digits.hasPrefix("#") { digits.removeFirst() }
    let value = digits.count == 6 ? UInt32(digits, radix: 16) ?? 0 : 0
    self.init(
      red: CGFloat((value >> 16) & 0xff) / 255,
      green: CGFloat((value >> 8) & 0xff) / 255,
      blue: CGFloat(value & 0xff) / 255,
      alpha: 1)
  }
}
