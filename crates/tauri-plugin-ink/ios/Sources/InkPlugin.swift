// 218: the `ink` plugin's commands. The whiteboard draws ink in the web view; this plugin only reports
// Pencil double-tap and squeeze (`pencilListen`) and recognises handwriting (`recognizeText`). Tauri
// calls the commands on its IPC queue; the Pencil ones hop to the main thread, where UIKit lives.

import Foundation
import Tauri
import UIKit
import WebKit

struct ListenArgs: Decodable {
  let onEvent: Channel
}

struct RecognizeArgs: Decodable {
  let png: String
}

struct ListenResult: Encodable {
  let ok: Bool
}

class InkPlugin: Plugin {
  /// Tauri's main web view, from `load(webview:)`.
  private weak var webview: WKWebView?
  /// The `PencilController` while listening (iOS 17.5+, iPad only); `AnyObject` for the availability gate.
  private var pencil: AnyObject?

  override func load(webview: WKWebView) {
    self.webview = webview
  }

  @objc public func pencilListen(_ invoke: Invoke) throws {
    let args = try invoke.parseArgs(ListenArgs.self)
    let channel = args.onEvent
    DispatchQueue.main.async {
      MainActor.assumeIsolated {
        self.removePencil()
        guard UIDevice.current.userInterfaceIdiom == .pad, #available(iOS 17.5, *),
          let webview = self.webview
        else {
          invoke.resolve(ListenResult(ok: false))
          return
        }
        self.pencil = PencilController(view: webview) { gesture, action in
          do {
            try channel.send(PencilEvent(gesture: gesture, action: action))
          } catch {
            try? channel.send(ErrorEvent(message: "pencil event: \(error)"))
          }
        }
        invoke.resolve(ListenResult(ok: true))
      }
    }
  }

  @objc public func pencilUnlisten(_ invoke: Invoke) throws {
    DispatchQueue.main.async {
      MainActor.assumeIsolated {
        self.removePencil()
        invoke.resolve()
      }
    }
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

  @MainActor
  private func removePencil() {
    if #available(iOS 17.5, *), let pencil = pencil as? PencilController {
      pencil.detach()
    }
    pencil = nil
  }
}

@_cdecl("init_plugin_ink")
public func initPlugin() -> Plugin {
  return InkPlugin()
}
