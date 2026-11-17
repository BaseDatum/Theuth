// Design 198 phase 1: the `glass` Tauri plugin. The commands the web sends as `plugin:glass|…` (Tauri forwards them
// here in lowerCamelCase) drive one `GlassOverlayController`, a child of the webview's host view controller whose
// view lies over the WKWebView. Commands arrive on Tauri's serial ipc queue; every UIKit touch hops to the main
// queue in arrival order (the upstream plugins' pattern). Taps go back as the `tap` event, `{ id }`.
// `setBackground` is the one command that works on every iOS version and device: the page's theme for the native
// layers behind the webview.

import SwiftRs
import Tauri
import UIKit
import WebKit

struct ConfigureArgs: Decodable {
  let title: String
  /// The page's own theme (Settings › Appearance can differ from the system); absent = follow the system.
  let dark: Bool?
}

struct TitleArgs: Decodable {
  let title: String
}

struct ScrollButtonArgs: Decodable {
  let visible: Bool
  /// Points from the top of the screen to the button's bottom edge: the web's composer top edge (on screen) minus
  /// the gap. Top-based, so it never depends on which of the web layout and the keyboard moved first.
  let y: Double
}

/// The page's background, sRGB 0–255 (the `<html>` background the web computed).
struct BackgroundArgs: Decodable {
  let red: Double
  let green: Double
  let blue: Double
}

struct HiddenArgs: Decodable {
  let hidden: Bool
}

class GlassPlugin: Plugin {
  private var overlay: GlassOverlayController?
  private weak var webView: WKWebView?

  @objc public override func load(webview: WKWebView) {
    self.webView = webview
  }

  /// Real Liquid Glass only: built with the iOS 26 SDK (Swift 6.2) and running on iOS 26+, on an iPhone (phase 1 is
  /// the phone layout; an iPad keeps the web header in every window size). Main thread: `UIDevice` is UIKit.
  private static func supported() -> Bool {
    #if compiler(>=6.2)
      if #available(iOS 26.0, *) {
        return UIDevice.current.userInterfaceIdiom == .phone
      }
    #endif
    return false
  }

  @objc public func isSupported(_ invoke: Invoke) {
    DispatchQueue.main.async {
      invoke.resolve(["supported": GlassPlugin.supported()])
    }
  }

  /// Mounts the overlay (idempotent), sets the title and appearance, and shows the bar.
  @objc public func configure(_ invoke: Invoke) throws {
    let args = try invoke.parseArgs(ConfigureArgs.self)
    onMain(invoke) { overlay in
      overlay.setTitle(args.title)
      if let dark = args.dark {
        overlay.overrideUserInterfaceStyle = dark ? .dark : .light
      } else {
        overlay.overrideUserInterfaceStyle = .unspecified
      }
      overlay.setHidden(false)
    }
  }

  @objc public func setTitle(_ invoke: Invoke) throws {
    let args = try invoke.parseArgs(TitleArgs.self)
    onMain(invoke) { $0.setTitle(args.title) }
  }

  @objc public func setScrollButton(_ invoke: Invoke) throws {
    let args = try invoke.parseArgs(ScrollButtonArgs.self)
    onMain(invoke) { $0.setScrollButton(visible: args.visible, y: CGFloat(args.y)) }
  }

  /// Every native layer that can show where the page does not paint (the window, the host view, the webview, its
  /// scroll view and under-page area) in the page's background. They are set once at launch from the system
  /// appearance (`desktop` `page_background`); without this a page whose theme differs (the system switched since
  /// launch, or Settings › Appearance forces one) shows the launch colour in the unpainted band under the composer.
  @objc public func setBackground(_ invoke: Invoke) throws {
    let args = try invoke.parseArgs(BackgroundArgs.self)
    DispatchQueue.main.async { [weak self] in
      let color = UIColor(red: args.red / 255, green: args.green / 255, blue: args.blue / 255, alpha: 1)
      if let web = self?.webView {
        web.backgroundColor = color
        web.scrollView.backgroundColor = color
        web.underPageBackgroundColor = color
        web.superview?.backgroundColor = color
        web.window?.backgroundColor = color
      }
      self?.manager.viewController?.view.backgroundColor = color
      invoke.resolve()
    }
  }

  /// Hides (or shows again) the bar and the scroll button together: a web sheet, dialog or the drawer is over the chat.
  @objc public func setHidden(_ invoke: Invoke) throws {
    let args = try invoke.parseArgs(HiddenArgs.self)
    onMain(invoke) { $0.setHidden(args.hidden) }
  }

  /// `body` on the main queue against the overlay, mounting it on first use; resolves after it ran.
  private func onMain(_ invoke: Invoke, _ body: @escaping (GlassOverlayController) -> Void) {
    DispatchQueue.main.async { [weak self] in
      guard let self else { return }
      guard GlassPlugin.supported() else {
        invoke.reject("glass chrome needs iOS 26 on an iPhone")
        return
      }
      guard let overlay = self.overlay ?? self.mount() else {
        invoke.reject("host view controller unavailable")
        return
      }
      body(overlay)
      invoke.resolve()
    }
  }

  /// The overlay as a child of the controller hosting the webview (set by Tauri's `on_webview_created`), its view
  /// pinned to the host view's edges and added after the webview, so it lies above it and glass samples the chat.
  private func mount() -> GlassOverlayController? {
    guard let host = manager.viewController else { return nil }
    let overlay = GlassOverlayController()
    overlay.onTap = { [weak self] id in
      self?.trigger("tap", data: ["id": id])
    }
    host.addChild(overlay)
    overlay.view.translatesAutoresizingMaskIntoConstraints = false
    host.view.addSubview(overlay.view)
    NSLayoutConstraint.activate([
      overlay.view.leadingAnchor.constraint(equalTo: host.view.leadingAnchor),
      overlay.view.trailingAnchor.constraint(equalTo: host.view.trailingAnchor),
      overlay.view.topAnchor.constraint(equalTo: host.view.topAnchor),
      overlay.view.bottomAnchor.constraint(equalTo: host.view.bottomAnchor),
    ])
    overlay.didMove(toParent: host)
    self.overlay = overlay
    return overlay
  }
}

@_cdecl("init_plugin_glass")
func initPlugin() -> Plugin {
  return GlassPlugin()
}
