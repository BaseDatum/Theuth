// 218 §6: the `ink` plugin's commands. Tauri calls them on its IPC queue; every one hops to the main
// thread, where the overlay lives. Native capture needs iPadOS 26 (`pencilKitResponderState`, R5); on
// anything else `attach` answers why not and the UI keeps drawing with pointer events.

import Foundation
import Tauri
import UIKit
import WebKit

struct AttachArgs: Decodable {
  let onStrokes: Channel
  let onEvent: Channel
}

struct EnabledArgs: Decodable {
  let enabled: Bool
}

struct ViewportArgs: Decodable {
  let tx: Double
  let ty: Double
  let zoom: Double
}

struct ToolArgs: Decodable {
  let tool: String
  let colour: String
  let width: Double
}

struct PickerArgs: Decodable {
  let visible: Bool
}

struct AckArgs: Decodable {
  let seqs: [Int]
}

struct RecognizeArgs: Decodable {
  let png: String
}

struct AttachResult: Encodable {
  let native: Bool
  var reason: String? = nil
  var os: String? = nil
  var route: String? = nil
}

class InkPlugin: Plugin {
  /// Tauri's main web view, from `load(webview:)`.
  private weak var webview: WKWebView?
  /// The overlay while attached (iPadOS 26 only).
  private var overlay: NativeInkOverlay?

  override func load(webview: WKWebView) {
    self.webview = webview
  }

  @objc public func attach(_ invoke: Invoke) throws {
    let args = try invoke.parseArgs(AttachArgs.self)
    let os = UIDevice.current.systemVersion
    DispatchQueue.main.async {
      MainActor.assumeIsolated {
        guard UIDevice.current.userInterfaceIdiom == .pad else {
          invoke.resolve(AttachResult(native: false, reason: "not_ipad", os: os))
          return
        }
        guard #available(iOS 26.0, *) else {
          invoke.resolve(AttachResult(native: false, reason: "unsupported_os", os: os))
          return
        }
        guard let webview = self.webview else {
          invoke.resolve(AttachResult(native: false, reason: "route_failed", os: os))
          return
        }
        let overlay = self.overlay ?? InkOverlay(webview: webview)
        guard let route = overlay.attach(strokes: args.onStrokes, events: args.onEvent) else {
          overlay.detach()
          self.overlay = nil
          invoke.resolve(AttachResult(native: false, reason: "route_failed", os: os))
          return
        }
        self.overlay = overlay
        invoke.resolve(AttachResult(native: true, os: os, route: route.rawValue))
      }
    }
  }

  @objc public func detach(_ invoke: Invoke) throws {
    onOverlay(invoke) { overlay in
      overlay.detach()
      self.overlay = nil
    }
  }

  @objc public func setEnabled(_ invoke: Invoke) throws {
    let args = try invoke.parseArgs(EnabledArgs.self)
    onOverlay(invoke) { $0.setEnabled(args.enabled) }
  }

  @objc public func setViewport(_ invoke: Invoke) throws {
    let args = try invoke.parseArgs(ViewportArgs.self)
    onOverlay(invoke) { $0.setViewport(NativeView(tx: args.tx, ty: args.ty, zoom: args.zoom)) }
  }

  @objc public func setTool(_ invoke: Invoke) throws {
    let args = try invoke.parseArgs(ToolArgs.self)
    guard let tool = InkTool(rawValue: args.tool) else {
      invoke.reject("unknown tool `\(args.tool)`")
      return
    }
    onOverlay(invoke) { $0.setTool(ToolState(tool: tool, colour: args.colour, width: args.width)) }
  }

  @objc public func setPickerVisible(_ invoke: Invoke) throws {
    let args = try invoke.parseArgs(PickerArgs.self)
    onOverlay(invoke) { $0.setPickerVisible(args.visible) }
  }

  @objc public func ack(_ invoke: Invoke) throws {
    let args = try invoke.parseArgs(AckArgs.self)
    onOverlay(invoke) { $0.ack(args.seqs) }
  }

  @objc public func recognizeText(_ invoke: Invoke) throws {
    let args = try invoke.parseArgs(RecognizeArgs.self)
    HandwritingRecognizer.recognize(base64: args.png) { result in
      switch result {
      case .success(let text):
        invoke.resolve(text)
      case .failure(let error):
        invoke.reject(error.localizedDescription)
      }
    }
  }

  /// Run `body` with the overlay on the main thread and resolve; without an overlay (not attached, or not
  /// iPadOS 26) the command is a no-op, as on desktop.
  private func onOverlay(_ invoke: Invoke, _ body: @escaping @MainActor (NativeInkOverlay) -> Void) {
    DispatchQueue.main.async {
      MainActor.assumeIsolated {
        if let overlay = self.overlay {
          body(overlay)
        }
        invoke.resolve()
      }
    }
  }
}

@_cdecl("init_plugin_ink")
public func initPlugin() -> Plugin {
  return InkPlugin()
}
