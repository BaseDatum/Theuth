// Design 198 phase 1: the controls. A full-screen pass-through view over the webview carrying
//   - the top bar under the safe-area top: round glass `sidebar.left` (drawer) · glass title pill with a chevron
//     (title) · round glass `ellipsis` (menu);
//   - the round glass `arrow.down` (scroll_bottom), centred, its bottom edge at the `y` the web sends (its composer's
//     top edge on screen minus the gap), shown only when the web says so.
// Geometry the web mirrors (`ui/src/lib/native-chrome.ts` `BAR`): the bar's band is TOP + CONTROL + TOP = 52 pt
// under the safe-area top, the web header's height. Controls keep a fixed 44 pt and show the large content viewer
// at accessibility text sizes (the system bars' rule); the title font scales up to 20 pt.

import UIKit

private enum Layout {
  static let top: CGFloat = 4
  static let control: CGFloat = 44
  static let side: CGFloat = 12
  static let gap: CGFloat = 8
  static let symbol: CGFloat = 17
}

/// Claims only touches that land on a control: its own empty area (and that of any pass-through container under
/// it) falls through to the webview.
final class PassthroughView: UIView {
  override func hitTest(_ point: CGPoint, with event: UIEvent?) -> UIView? {
    let hit = super.hitTest(point, with: event)
    return hit is PassthroughView ? nil : hit
  }
}

final class GlassOverlayController: UIViewController {
  /// `drawer` | `title` | `menu` | `scroll_bottom`.
  var onTap: ((String) -> Void)?

  private let bar = PassthroughView()
  private lazy var drawer = roundButton(symbol: "sidebar.left", label: "Open sidebar", id: "drawer")
  private lazy var pill = titleButton()
  private lazy var menu = roundButton(symbol: "ellipsis", label: "Session menu", id: "menu")
  private lazy var scroll = roundButton(symbol: "arrow.down", label: "Scroll to bottom", id: "scroll_bottom")
  private var scrollBottom: NSLayoutConstraint?
  private var hidden = true
  private var scrollVisible = false
  private let haptic = UIImpactFeedbackGenerator(style: .light)

  override func loadView() {
    let root = PassthroughView()
    root.backgroundColor = .clear
    view = root
  }

  override func viewDidLoad() {
    super.viewDidLoad()
    bar.translatesAutoresizingMaskIntoConstraints = false
    view.addSubview(bar)
    for control in [drawer, pill, menu] {
      control.translatesAutoresizingMaskIntoConstraints = false
      bar.addSubview(control)
    }
    scroll.translatesAutoresizingMaskIntoConstraints = false
    view.addSubview(scroll)
    // Long-press a control at accessibility text sizes to read it enlarged.
    bar.addInteraction(UILargeContentViewerInteraction())
    view.addInteraction(UILargeContentViewerInteraction())

    // From the view's top, where the web measured: the webview fills the same host view, so its CSS px are these
    // points. Not `keyboardLayoutGuide`: the native keyboard and the web layout move at different moments, and a
    // keyboard-relative offset measured in one state put the button over the composer in the other.
    let guide = view.safeAreaLayoutGuide
    let bottom = scroll.bottomAnchor.constraint(equalTo: view.topAnchor, constant: 0)
    scrollBottom = bottom
    // Centred on the bar unless a long title would overlap a button; then it keeps the gaps and truncates.
    let centred = pill.centerXAnchor.constraint(equalTo: bar.centerXAnchor)
    centred.priority = .defaultHigh
    pill.setContentCompressionResistancePriority(.defaultLow, for: .horizontal)

    var constraints = [
      bar.topAnchor.constraint(equalTo: guide.topAnchor, constant: Layout.top),
      bar.leadingAnchor.constraint(equalTo: guide.leadingAnchor, constant: Layout.side),
      bar.trailingAnchor.constraint(equalTo: guide.trailingAnchor, constant: -Layout.side),
      bar.heightAnchor.constraint(equalToConstant: Layout.control),
      drawer.leadingAnchor.constraint(equalTo: bar.leadingAnchor),
      drawer.centerYAnchor.constraint(equalTo: bar.centerYAnchor),
      menu.trailingAnchor.constraint(equalTo: bar.trailingAnchor),
      menu.centerYAnchor.constraint(equalTo: bar.centerYAnchor),
      pill.centerYAnchor.constraint(equalTo: bar.centerYAnchor),
      pill.heightAnchor.constraint(equalToConstant: Layout.control),
      pill.leadingAnchor.constraint(greaterThanOrEqualTo: drawer.trailingAnchor, constant: Layout.gap),
      pill.trailingAnchor.constraint(lessThanOrEqualTo: menu.leadingAnchor, constant: -Layout.gap),
      centred,
      scroll.centerXAnchor.constraint(equalTo: guide.centerXAnchor),
      bottom,
    ]
    for round in [drawer, menu, scroll] {
      constraints.append(round.widthAnchor.constraint(equalToConstant: Layout.control))
      constraints.append(round.heightAnchor.constraint(equalToConstant: Layout.control))
    }
    NSLayoutConstraint.activate(constraints)

    for v in [bar, scroll] as [UIView] {
      v.alpha = 0
      v.isHidden = true
    }
    // The title's transformer reads the content size category: re-run it when Dynamic Type changes.
    registerForTraitChanges([UITraitPreferredContentSizeCategory.self]) {
      (controller: GlassOverlayController, _: UITraitCollection) in
      controller.pill.setNeedsUpdateConfiguration()
    }
  }

  func setTitle(_ title: String) {
    loadViewIfNeeded()
    pill.configuration?.title = title
    pill.accessibilityLabel = title
    pill.largeContentTitle = title
  }

  func setHidden(_ hidden: Bool) {
    loadViewIfNeeded()
    self.hidden = hidden
    // Stay above anything added over the webview since the overlay mounted.
    if !hidden { view.superview?.bringSubviewToFront(view) }
    fade(bar, visible: !hidden)
    fade(scroll, visible: !hidden && scrollVisible)
  }

  func setScrollButton(visible: Bool, y: CGFloat) {
    loadViewIfNeeded()
    scrollVisible = visible
    if scrollBottom?.constant != y {
      scrollBottom?.constant = y
      if scroll.isHidden {
        view.layoutIfNeeded()
      } else {
        UIView.animate(withDuration: 0.2) { self.view.layoutIfNeeded() }
      }
    }
    fade(scroll, visible: visible && !hidden)
  }

  private func fade(_ v: UIView, visible: Bool) {
    if visible { v.isHidden = false }
    UIView.animate(withDuration: 0.2, delay: 0, options: [.beginFromCurrentState, .allowUserInteraction]) {
      v.alpha = visible ? 1 : 0
    } completion: { _ in
      // The model value: a fade-in that interrupted this fade-out keeps the view shown.
      if v.alpha == 0 { v.isHidden = true }
    }
  }

  private func tap(_ id: String) {
    haptic.impactOccurred()
    onTap?(id)
  }

  /// Liquid Glass on iOS 26 (SDK and OS). Below that `is_supported` is false and the web keeps its own chrome; the
  /// gray capsule only keeps an older SDK compiling.
  private func glass() -> UIButton.Configuration {
    #if compiler(>=6.2)
      if #available(iOS 26.0, *) {
        return .glass()
      }
    #endif
    var config = UIButton.Configuration.gray()
    config.cornerStyle = .capsule
    return config
  }

  private func roundButton(symbol: String, label: String, id: String) -> UIButton {
    var config = glass()
    config.image = UIImage(
      systemName: symbol,
      withConfiguration: UIImage.SymbolConfiguration(pointSize: Layout.symbol, weight: .semibold))
    config.contentInsets = .zero
    let button = UIButton(configuration: config)
    button.accessibilityLabel = label
    button.showsLargeContentViewer = true
    button.largeContentTitle = label
    button.largeContentImage = config.image
    button.addAction(UIAction { [weak self] _ in self?.tap(id) }, for: .touchUpInside)
    return button
  }

  private func titleButton() -> UIButton {
    var config = glass()
    config.image = UIImage(
      systemName: "chevron.down",
      withConfiguration: UIImage.SymbolConfiguration(pointSize: 11, weight: .semibold))
    config.imagePlacement = .trailing
    config.imagePadding = 5
    config.titleLineBreakMode = .byTruncatingTail
    config.contentInsets = NSDirectionalEdgeInsets(top: 0, leading: 16, bottom: 0, trailing: 14)
    config.titleTextAttributesTransformer = UIConfigurationTextAttributesTransformer { incoming in
      var outgoing = incoming
      outgoing.font = UIFontMetrics(forTextStyle: .subheadline).scaledFont(
        for: .systemFont(ofSize: 15, weight: .semibold), maximumPointSize: 20)
      return outgoing
    }
    let button = UIButton(configuration: config)
    button.showsLargeContentViewer = true
    button.addAction(UIAction { [weak self] _ in self?.tap("title") }, for: .touchUpInside)
    return button
  }
}
