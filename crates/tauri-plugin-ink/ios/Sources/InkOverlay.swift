// 218 §6: the capture overlay and its hand-over protocol.
//
// A capture surface (route A: PencilKit's own canvas; route B: the plugin's recogniser and shape layers)
// shows each stroke the instant the Pencil draws it. At pen-idle (no Pencil touch for 120 ms) the strokes
// not yet sent go to JS in one `StrokeBatch`, in stroke order, each with a `seq` (monotonic per attach).
// JS commits them to the whiteboard and acknowledges; an acknowledged stroke leaves the overlay (the web
// layer paints it by then). Without an ack within 300 ms the same strokes go again, same `seq`,
// `retry + 1`, every 300 ms for as long as the overlay is attached: a stroke is never dropped.

import PencilKit
import Tauri
import UIKit
import WebKit

@available(iOS 26.0, *)
@MainActor
final class InkOverlay: NativeInkOverlay {
  private static let idleDelay: TimeInterval = 0.12
  private static let retryDelay: TimeInterval = 0.3

  private struct Pending {
    var seq: Int
    let ink: CapturedInk
    var retry: Int
    /// When it last went to JS; nil = not yet sent.
    var sentAt: CFTimeInterval?
  }

  private weak var webview: WKWebView?
  private var surface: CaptureSurface?
  private var route: InkRoute?
  private var picker: ToolPickerController?
  private var pencil: PencilController?
  private var strokes: Channel?
  private var events: Channel?

  private var view = NativeView.identity
  private var tool = ToolState.initial
  private var enabled = true
  private var penDown = false
  private var nextSeq = 1
  private var nextBatch = 1
  private var queue: [Pending] = []
  private var idleTimer: Timer?
  private var retryTimer: Timer?

  init(webview: WKWebView) {
    self.webview = webview
  }

  func attach(strokes: Channel, events: Channel) -> InkRoute? {
    self.strokes = strokes
    self.events = events
    if let route = route {
      // A second attach replaces the channels and starts a new `seq` space: strokes still on the
      // overlay are renumbered and handed over again.
      surface?.raise()
      nextSeq = 1
      nextBatch = 1
      for index in queue.indices {
        queue[index].seq = nextSeq
        queue[index].retry = 0
        queue[index].sentAt = nil
        nextSeq += 1
      }
      flush()
      return route
    }
    guard let webview = webview else { return nil }
    guard let (surface, route) = mountSurface(over: webview) else { return nil }
    self.surface = surface
    self.route = route
    surface.context = { [unowned self] in BeginContext(tool: self.tool, view: self.view) }
    surface.onPen = { [unowned self] down in self.pen(down) }
    surface.onStroke = { [unowned self] in self.strokeEnded() }

    let picker = ToolPickerController(webview: webview, initial: tool)
    picker.onChange = { [unowned self] state in self.pickerChanged(state) }
    if let canvas = (surface as? CanvasSurface)?.canvas {
      picker.addObserver(canvas)
    }
    self.picker = picker
    pencil = PencilController(view: webview) { [unowned self] gesture, action in
      self.emit(PencilEvent(gesture: gesture, action: action))
    }
    applyTool()
    return route
  }

  /// Route A unless `InkRoute` (user defaults, e.g. the launch argument `-InkRoute B`) asks for B or A
  /// cannot be installed.
  private func mountSurface(over webview: WKWebView) -> (CaptureSurface, InkRoute)? {
    let forced = UserDefaults.standard.string(forKey: "InkRoute")
    if forced != "B" {
      let canvas = CanvasSurface()
      if canvas.mount(over: webview) { return (canvas, .A) }
      emit(ErrorEvent(message: "route A could not be installed; using route B"))
    }
    let shapes = ShapeSurface()
    if shapes.mount(over: webview) { return (shapes, .B) }
    return nil
  }

  func detach() {
    idleTimer?.invalidate()
    idleTimer = nil
    retryTimer?.invalidate()
    retryTimer = nil
    pencil?.detach()
    pencil = nil
    picker?.detach()
    picker = nil
    surface?.unmount()
    surface = nil
    route = nil
    queue.removeAll()
    strokes = nil
    events = nil
    penDown = false
  }

  func setEnabled(_ enabled: Bool) {
    self.enabled = enabled
    applyEnabled()
  }

  func setViewport(_ view: NativeView) {
    guard view.zoom.isFinite, view.zoom > 0, view.tx.isFinite, view.ty.isFinite else { return }
    self.view = view
    // A tile web view added since attach sits directly above the main web view; stay above it.
    surface?.raise()
  }

  func setTool(_ tool: ToolState) {
    self.tool = tool
    picker?.select(tool)
    applyTool()
  }

  func setPickerVisible(_ visible: Bool) {
    picker?.setVisible(visible)
  }

  func ack(_ seqs: [Int]) {
    let acked = Set(seqs)
    var locals = Set<Int>()
    queue.removeAll { pending in
      guard acked.contains(pending.seq) else { return false }
      locals.insert(pending.ink.local)
      return true
    }
    if !locals.isEmpty { surface?.remove(locals) }
    if !queue.contains(where: { $0.sentAt != nil }) {
      retryTimer?.invalidate()
      retryTimer = nil
    }
  }

  // MARK: - Surface and picker callbacks

  private func pen(_ down: Bool) {
    guard down != penDown else { return }
    penDown = down
    emit(StateEvent(drawing: down))
    if down {
      idleTimer?.invalidate()
      idleTimer = nil
    } else {
      scheduleIdle()
    }
  }

  private func strokeEnded() {
    guard let surface = surface else { return }
    for ink in surface.take() {
      queue.append(Pending(seq: nextSeq, ink: ink, retry: 0, sentAt: nil))
      nextSeq += 1
    }
    if !penDown { scheduleIdle() }
  }

  private func pickerChanged(_ state: ToolState) {
    guard state != tool else { return }
    tool = state
    applyTool()
    emit(ToolEvent(tool: state.tool.rawValue, colour: state.colour, width: state.width))
  }

  private func applyTool() {
    surface?.setTool(tool)
    applyEnabled()
  }

  /// The eraser erases whole strokes web-side: the overlay lets pen events through to the page.
  private func applyEnabled() {
    surface?.setEnabled(enabled && tool.tool != .eraser)
  }

  // MARK: - Hand-over

  private func scheduleIdle() {
    idleTimer?.invalidate()
    idleTimer = Timer.scheduledTimer(withTimeInterval: Self.idleDelay, repeats: false) {
      [weak self] _ in
      MainActor.assumeIsolated { self?.flush() }
    }
  }

  /// Send every stroke not yet sent, as one batch.
  private func flush() {
    idleTimer = nil
    send(where: { $0.sentAt == nil }, bump: false)
  }

  /// Resend what has waited `retryDelay` for its ack.
  private func retry() {
    let now = CACurrentMediaTime()
    send(where: { pending in
      guard let sentAt = pending.sentAt else { return false }
      return now - sentAt >= Self.retryDelay - 0.01
    }, bump: true)
  }

  private func send(where pick: (Pending) -> Bool, bump: Bool) {
    guard let channel = strokes, let webview = webview else { return }
    let now = CACurrentMediaTime()
    var out: [NativeStroke] = []
    for index in queue.indices where pick(queue[index]) {
      if bump { queue[index].retry += 1 }
      queue[index].sentAt = now
      out.append(wire(queue[index], in: webview))
    }
    guard !out.isEmpty else { return }
    let batch = StrokeBatch(batch: nextBatch, strokes: out)
    nextBatch += 1
    do {
      try channel.send(batch)
    } catch {
      emit(ErrorEvent(message: "stroke hand-over failed: \(error)"))
    }
    if retryTimer == nil {
      retryTimer = Timer.scheduledTimer(withTimeInterval: Self.retryDelay, repeats: true) {
        [weak self] _ in
        MainActor.assumeIsolated { self?.retry() }
      }
    }
  }

  /// A queued stroke as JS reads it: points in CSS px of the layout viewport (through the scroll view,
  /// so a content inset or page zoom cannot shift the ink), the rest as captured.
  private func wire(_ pending: Pending, in webview: WKWebView) -> NativeStroke {
    let scroll = webview.scrollView
    let zoom = max(scroll.zoomScale, 0.0001)
    var pts: [Double] = []
    pts.reserveCapacity(pending.ink.points.count * 4)
    for (point, force, t) in pending.ink.points {
      let content = scroll.convert(point, from: webview)
      pts.append(Double(content.x / zoom))
      pts.append(Double(content.y / zoom))
      pts.append(force)
      pts.append(t)
    }
    let tool = pending.ink.tool
    return NativeStroke(
      seq: pending.seq,
      tool: tool.tool == .eraser ? InkTool.pen.rawValue : tool.tool.rawValue,
      colour: tool.colour,
      width: tool.width,
      t0: pending.ink.t0,
      view: pending.ink.view,
      pts: pts,
      retry: pending.retry)
  }

  private func emit<T: Encodable>(_ event: T) {
    try? events?.send(event)
  }
}
