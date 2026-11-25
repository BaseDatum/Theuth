// 218 §6.2: the two ways the overlay captures the Pencil.
//
// Route A — a capture-only `PKCanvasView` above the web view: never transformed, `.pencilOnly`, its
// `hitTest` answers nil so fingers reach the page, and its `drawingGestureRecognizer` moves onto the web
// view (pencil touches only), so PencilKit draws with its own latency while the page keeps every finger.
// Route B — the fallback: a pencil-only recogniser on the web view and one `CAShapeLayer` per stroke on a
// pass-through view. Both keep the web view's frame, so their coordinates are the web view's.

import PencilKit
import UIKit
import UIKit.UIGestureRecognizerSubclass

/// A view that never takes a touch.
final class PassThroughView: UIView {
  override func hitTest(_ point: CGPoint, with event: UIEvent?) -> UIView? { nil }
}

/// A `PKCanvasView` that never takes a touch itself (its recogniser lives on the web view).
@available(iOS 26.0, *)
final class CaptureCanvas: PKCanvasView {
  override func hitTest(_ point: CGPoint, with event: UIEvent?) -> UIView? { nil }
}

private let pencilOnly = [NSNumber(value: UITouch.TouchType.pencil.rawValue)]

/// Keep `overlay` on `host`'s frame and above every sibling (the web view tiles insert themselves directly
/// above the main web view).
@MainActor
private func cover(_ overlay: UIView, host: UIView) {
  overlay.frame = host.frame
  overlay.superview?.bringSubviewToFront(overlay)
}

// MARK: - Route A

@available(iOS 26.0, *)
@MainActor
final class CanvasSurface: NSObject, CaptureSurface, PKCanvasViewDelegate {
  var onPen: ((Bool) -> Void)?
  var onStroke: (() -> Void)?
  var context: (() -> BeginContext)?

  let canvas = CaptureCanvas()
  private weak var host: UIView?
  private var recognizer: UIGestureRecognizer?
  /// The strokes on the canvas, in its order, with their local ids.
  private var entries: [(local: Int, stroke: PKStroke)] = []
  /// Acknowledged while the pen was down: removed at pen-up (replacing the drawing mid-stroke would
  /// cancel the stroke in progress).
  private var removed = Set<Int>()
  private var ready: [CapturedInk] = []
  private var begin: BeginContext?
  private var nextLocal = 1
  private var replacing = false
  private var down = false

  func mount(over webview: UIView) -> Bool {
    guard let superview = webview.superview else { return false }
    canvas.backgroundColor = .clear
    canvas.isOpaque = false
    canvas.drawingPolicy = .pencilOnly
    canvas.isScrollEnabled = false
    canvas.contentInsetAdjustmentBehavior = .never
    canvas.minimumZoomScale = 1
    canvas.maximumZoomScale = 1
    // JS sends colours already resolved for the theme: draw them as given, never dark-adapted.
    canvas.overrideUserInterfaceStyle = .light
    canvas.autoresizingMask = [.flexibleWidth, .flexibleHeight]
    canvas.isAccessibilityElement = false
    canvas.delegate = self
    superview.addSubview(canvas)
    cover(canvas, host: webview)

    let recognizer = canvas.drawingGestureRecognizer
    canvas.removeGestureRecognizer(recognizer)
    webview.addGestureRecognizer(recognizer)
    recognizer.allowedTouchTypes = pencilOnly
    guard recognizer.view === webview else {
      unmount()
      return false
    }
    self.recognizer = recognizer
    host = webview
    return true
  }

  func unmount() {
    if let recognizer = recognizer {
      recognizer.view?.removeGestureRecognizer(recognizer)
      canvas.addGestureRecognizer(recognizer)
    }
    recognizer = nil
    canvas.delegate = nil
    canvas.removeFromSuperview()
    entries.removeAll()
    removed.removeAll()
    ready.removeAll()
  }

  /// Only the recogniser is switched: the canvas itself stays drawing-enabled (an off recogniser already leaves every
  /// Pencil touch to the page), so capture going off and on never cycles PencilKit's own state. Should PencilKit have
  /// taken its recogniser back onto the canvas (which never takes a touch), it goes back onto the web view.
  func setEnabled(_ enabled: Bool) {
    if let recognizer = recognizer, let host = host, recognizer.view !== host {
      recognizer.view?.removeGestureRecognizer(recognizer)
      host.addGestureRecognizer(recognizer)
      recognizer.allowedTouchTypes = pencilOnly
    }
    recognizer?.isEnabled = enabled
  }

  func setTool(_ tool: ToolState) {
    guard tool.tool != .eraser else { return }
    canvas.tool = PKInkingTool(
      inkType(tool.tool), color: UIColor(inkHex: tool.colour), width: CGFloat(tool.width))
  }

  func raise() {
    if let host = host { cover(canvas, host: host) }
  }

  func take() -> [CapturedInk] {
    defer { ready.removeAll() }
    return ready
  }

  func remove(_ locals: Set<Int>) {
    removed.formUnion(locals)
    if !down { compact() }
  }

  private func compact() {
    guard !removed.isEmpty else { return }
    entries.removeAll { removed.contains($0.local) }
    removed.removeAll()
    replacing = true
    canvas.drawing = PKDrawing(strokes: entries.map(\.stroke))
    replacing = false
  }

  // MARK: PKCanvasViewDelegate

  func canvasViewDidBeginUsingTool(_ canvasView: PKCanvasView) {
    down = true
    begin = context?()
    onPen?(true)
  }

  func canvasViewDidEndUsingTool(_ canvasView: PKCanvasView) {
    down = false
    compact()
    onPen?(false)
  }

  func canvasViewDrawingDidChange(_ canvasView: PKCanvasView) {
    guard !replacing else { return }
    let strokes = canvasView.drawing.strokes
    guard strokes.count > entries.count else { return }
    for stroke in strokes[entries.count...] {
      let local = nextLocal
      nextLocal += 1
      entries.append((local, stroke))
      ready.append(capture(stroke, local: local))
    }
    onStroke?()
  }

  /// The stroke's centre line every 1.5 pt, through its transform, with normalised force and ms times.
  private func capture(_ stroke: PKStroke, local: Int) -> CapturedInk {
    let at = begin ?? context?() ?? BeginContext(tool: .initial, view: .identity)
    var points: [(CGPoint, Double, Double)] = []
    for point in stroke.path.interpolatedPoints(by: .distance(1.5)) {
      points.append(
        (point.location.applying(stroke.transform), normalisedForce(point.force),
         point.timeOffset * 1000))
    }
    let tool = ToolState(
      tool: inkTool(stroke.ink.inkType), colour: stroke.ink.color.inkHex, width: at.tool.width)
    return CapturedInk(
      local: local, tool: tool, t0: stroke.path.creationDate.timeIntervalSince1970 * 1000,
      view: at.view, points: points)
  }
}

@available(iOS 26.0, *)
func inkType(_ tool: InkTool) -> PKInkingTool.InkType {
  switch tool {
  case .monoline: return .monoline
  case .marker: return .marker
  case .pen, .eraser: return .pen
  }
}

@available(iOS 26.0, *)
func inkTool(_ type: PKInkingTool.InkType) -> InkTool {
  switch type {
  case .monoline: return .monoline
  case .marker: return .marker
  default: return .pen
  }
}

// MARK: - Route B

/// Tracks one Pencil touch at a time on the web view; never fingers.
final class PencilRecognizer: UIGestureRecognizer {
  /// (phase, the touch's samples since the last call, the touch).
  var onSamples: ((UIGestureRecognizer.State, [UITouch], UITouch) -> Void)?
  private weak var tracked: UITouch?

  override func touchesBegan(_ touches: Set<UITouch>, with event: UIEvent) {
    guard tracked == nil, let touch = touches.first(where: { $0.type == .pencil }) else { return }
    tracked = touch
    state = .began
    onSamples?(.began, [touch], touch)
  }

  override func touchesMoved(_ touches: Set<UITouch>, with event: UIEvent) {
    guard let touch = tracked, touches.contains(touch) else { return }
    state = .changed
    onSamples?(.changed, event.coalescedTouches(for: touch) ?? [touch], touch)
  }

  override func touchesEnded(_ touches: Set<UITouch>, with event: UIEvent) {
    guard let touch = tracked, touches.contains(touch) else { return }
    onSamples?(.ended, event.coalescedTouches(for: touch) ?? [touch], touch)
    state = .ended
  }

  override func touchesCancelled(_ touches: Set<UITouch>, with event: UIEvent) {
    guard let touch = tracked, touches.contains(touch) else { return }
    onSamples?(.cancelled, [], touch)
    state = .cancelled
  }

  override func reset() {
    super.reset()
    tracked = nil
  }
}

@MainActor
final class ShapeSurface: CaptureSurface {
  var onPen: ((Bool) -> Void)?
  var onStroke: (() -> Void)?
  var context: (() -> BeginContext)?

  private struct Live {
    let local: Int
    let layer: CAShapeLayer
    let path = UIBezierPath()
    let at: BeginContext
    let t0: Double
    let start: TimeInterval
    var points: [(CGPoint, Double, Double)] = []
  }

  private let layerView = PassThroughView()
  private let recognizer = PencilRecognizer()
  private weak var host: UIView?
  private var live: Live?
  private var layers: [Int: CAShapeLayer] = [:]
  private var ready: [CapturedInk] = []
  private var nextLocal = 1

  func mount(over webview: UIView) -> Bool {
    guard let superview = webview.superview else { return false }
    layerView.backgroundColor = .clear
    layerView.isOpaque = false
    layerView.isUserInteractionEnabled = false
    layerView.autoresizingMask = [.flexibleWidth, .flexibleHeight]
    superview.addSubview(layerView)
    cover(layerView, host: webview)
    recognizer.allowedTouchTypes = pencilOnly
    recognizer.delaysTouchesBegan = false
    recognizer.onSamples = { [weak self] phase, samples, touch in
      MainActor.assumeIsolated { self?.samples(phase, samples, touch) }
    }
    webview.addGestureRecognizer(recognizer)
    host = webview
    return true
  }

  func unmount() {
    recognizer.view?.removeGestureRecognizer(recognizer)
    recognizer.onSamples = nil
    layerView.removeFromSuperview()
    layerView.layer.sublayers?.forEach { $0.removeFromSuperlayer() }
    layers.removeAll()
    ready.removeAll()
    live = nil
  }

  func setEnabled(_ enabled: Bool) {
    recognizer.isEnabled = enabled
  }

  func setTool(_ tool: ToolState) {}

  func raise() {
    if let host = host { cover(layerView, host: host) }
  }

  func take() -> [CapturedInk] {
    defer { ready.removeAll() }
    return ready
  }

  func remove(_ locals: Set<Int>) {
    for local in locals {
      layers.removeValue(forKey: local)?.removeFromSuperlayer()
    }
  }

  private func samples(_ phase: UIGestureRecognizer.State, _ samples: [UITouch], _ touch: UITouch) {
    guard let host = host else { return }
    switch phase {
    case .began:
      let at = context?() ?? BeginContext(tool: .initial, view: .identity)
      let layer = CAShapeLayer()
      layer.fillColor = nil
      layer.strokeColor = UIColor(inkHex: at.tool.colour).cgColor
      layer.lineCap = .round
      layer.lineJoin = .round
      layer.lineWidth = CGFloat(at.tool.tool == .marker ? at.tool.width * 3 : at.tool.width)
      layer.opacity = at.tool.tool == .marker ? 0.45 : 1
      layerView.layer.addSublayer(layer)
      let lag = ProcessInfo.processInfo.systemUptime - touch.timestamp
      live = Live(
        local: nextLocal, layer: layer, at: at, t0: nowMs() - lag * 1000, start: touch.timestamp)
      nextLocal += 1
      append(samples, in: host)
      onPen?(true)
    case .changed:
      append(samples, in: host)
    case .ended:
      append(samples, in: host)
      if let done = live {
        layers[done.local] = done.layer
        ready.append(
          CapturedInk(
            local: done.local, tool: done.at.tool, t0: done.t0, view: done.at.view,
            points: done.points))
        live = nil
        onStroke?()
      }
      onPen?(false)
    default:
      live?.layer.removeFromSuperlayer()
      live = nil
      onPen?(false)
    }
  }

  private func append(_ samples: [UITouch], in host: UIView) {
    guard var stroke = live else { return }
    for sample in samples {
      let point = sample.preciseLocation(in: host)
      if let last = stroke.points.last?.0, hypot(point.x - last.x, point.y - last.y) < 0.5 {
        continue
      }
      if stroke.points.isEmpty {
        stroke.path.move(to: point)
        stroke.path.addLine(to: point)
      } else {
        stroke.path.addLine(to: point)
      }
      stroke.points.append(
        (point, normalisedForce(sample.force), (sample.timestamp - stroke.start) * 1000))
    }
    stroke.layer.path = stroke.path.cgPath
    live = stroke
  }
}
