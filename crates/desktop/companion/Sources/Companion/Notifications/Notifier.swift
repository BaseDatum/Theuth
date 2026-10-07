import AppKit
import Combine
import CompanionCore
import TheuthKit
import UserNotifications

/// 130 D8 / §4.4 / R1-4: the companion's banners. Policy decisions come from `NotificationPolicy`; this class
/// registers the categories on every launch, posts/withdraws, and routes action taps into W1's answer path
/// (`Store.answerNow`, no hold) — then a receipt banner. Authorization is requested ONLY by
/// `requestAuthorization()` (Settings button / `--request-notifications`). `dryRun` (`--probe-notify`) logs
/// what would have been posted instead of posting.
@MainActor
final class Notifier: NSObject, ObservableObject, UNUserNotificationCenterDelegate {
    static let probeLog = URL(fileURLWithPath: "/tmp/130-w2-probe.log")

    @Published private(set) var authorization: UNAuthorizationStatus = .notDetermined
    @Published private(set) var pausedUntil: Date?

    private let store: Store
    private let openOnItem: (String?) -> Void
    private let dryRun: Bool
    private let center: UNUserNotificationCenter?
    private var policy: NotificationPolicy
    private let breakpoints = BreakpointSource()
    private var subs = Set<AnyCancellable>()
    private var dynamicCategories: [String: UNNotificationCategory] = [:]
    /// A notification action is in flight: the next Store receipt becomes a receipt banner.
    private var receiptFor: (who: String, label: String)?
    private(set) var probeCounts: [String: Int] = [:]
    var inFlood: Bool { policy.inFlood }
    var heldCount: Int { policy.heldCount }

    init(store: Store, dryRun: Bool, openOnItem: @escaping (String?) -> Void) {
        self.store = store
        self.dryRun = dryRun
        self.openOnItem = openOnItem
        // The bare executable has no bundle proxy: `UNUserNotificationCenter.current()` would crash (SA §5).
        center = Bundle.main.bundleIdentifier != nil ? .current() : nil
        policy = NotificationPolicy(prefs: SettingsModel.shared.value.notify)
        super.init()
        if center == nil { Log.write("notifications: no bundle id — center disabled") }
    }

    func start() {
        center?.delegate = self
        registerCategories()
        refreshAuthorization()

        // The first `apply` seeds the policy (items already waiting are not "new"); `[]` before connect is skipped.
        store.$items.dropFirst().receive(on: RunLoop.main).sink { [weak self] items in
            MainActor.assumeIsolated { self?.run { $0.update(items: items, now: Date()) } }
        }.store(in: &subs)
        store.$connection.dropFirst().receive(on: RunLoop.main).sink { [weak self] s in
            MainActor.assumeIsolated {
                guard s != .connecting else { return }
                self?.run { $0.engine(connected: s == .open, now: Date()) }
            }
        }.store(in: &subs)
        store.$receipt.receive(on: RunLoop.main).sink { [weak self] r in
            MainActor.assumeIsolated { self?.receipt(r) }
        }.store(in: &subs)
        SettingsModel.shared.$value.map(\.notify).removeDuplicates().receive(on: RunLoop.main).sink { [weak self] prefs in
            MainActor.assumeIsolated { self?.policy.prefs = prefs }
        }.store(in: &subs)

        breakpoints.onBreakpoint = { [weak self] in self?.run { $0.breakpoint(now: Date()) } }
        breakpoints.onTick = { [weak self] now in
            guard let self else { return }
            self.run { $0.tick(now: now) }
            if self.pausedUntil != self.policy.pausedUntil { self.pausedUntil = self.policy.pausedUntil }
        }
        breakpoints.start()
    }

    // MARK: pause (menu)

    func pause(until: Date?) {
        policy.pause(until: until)
        pausedUntil = until
        Log.write("notifications: pause until \(until.map { "\($0)" } ?? "—")")
    }

    static var tomorrow: Date {
        let cal = Calendar.current
        return cal.startOfDay(for: cal.date(byAdding: .day, value: 1, to: Date())!)
    }

    // MARK: authorization

    /// The dialog. Only ever from a user action (Settings › Enable notifications, `--request-notifications`).
    func requestAuthorization() {
        guard let center else { return }
        Log.write("notifications: requesting authorization (user-initiated)")
        center.requestAuthorization(options: [.alert, .sound]) { [weak self] granted, err in
            Task { @MainActor in
                Log.write("notifications: authorization granted=\(granted) err=\(err.map { "\($0)" } ?? "none")")
                self?.refreshAuthorization()
            }
        }
    }

    func refreshAuthorization() {
        center?.getNotificationSettings { [weak self] s in
            Task { @MainActor in self?.authorization = s.authorizationStatus }
        }
    }

    // MARK: categories (registered on EVERY launch — they read back empty before authorization, R1-4)

    static let optionIds = ["opt-1", "opt-2", "opt-3"]

    private static func action(_ id: String, _ title: String, destructive: Bool = false) -> UNNotificationAction {
        UNNotificationAction(identifier: id, title: title, options: destructive ? [.destructive] : [])
    }

    private static func text(_ id: String, _ title: String, placeholder: String) -> UNTextInputNotificationAction {
        UNTextInputNotificationAction(identifier: id, title: title, options: [], textInputButtonTitle: "Send", textInputPlaceholder: placeholder)
    }

    /// The static set; `titles` replaces the option-button titles for a per-item variant.
    static func category(_ c: Banner.Category, id: String? = nil, titles: [String] = []) -> UNNotificationCategory {
        func opts(_ defaults: [String]) -> [UNNotificationAction] {
            let t = titles.isEmpty ? defaults : titles
            return zip(optionIds, t).map { action($0, $1, destructive: $1.lowercased().hasPrefix("deny")) }
        }
        let actions: [UNNotificationAction]
        switch c {
        case .question: actions = opts(["Option 1", "Option 2", "Option 3"]) + [text("reply", "Reply…", placeholder: "Your answer")]
        case .approval: actions = opts(["Allow once", "Deny"]) + [text("deny-note", "Deny with note…", placeholder: "Why not")]
        case .review: actions = opts(["Approve"]) + [text("send-back", "Send back…", placeholder: "What to change"), action("open", "Open")]
        case .finished: actions = opts(["Next step 1", "Next step 2"]) + [action("clear", "Clear")]
        case .batch: actions = [action("open", "Open panel"), action("later", "Later")]
        case .error: actions = [action("open", "Open")]
        }
        return UNNotificationCategory(identifier: id ?? c.rawValue, actions: actions, intentIdentifiers: [], options: [])
    }

    func registerCategories() {
        guard let center else { return }
        let all = Set(Banner.Category.allCases.map { Self.category($0) }).union(dynamicCategories.values)
        center.setNotificationCategories(all)
        Log.write("notifications: registered \(all.count) categories")
    }

    // MARK: policy → center

    private func run(_ step: (inout NotificationPolicy) -> PolicyOutput) {
        let out = step(&policy)
        guard !out.isEmpty else { return }
        out.withdraw.forEach(withdraw)
        out.post.forEach(post)
    }

    private func post(_ b: Banner) {
        var category = b.id.hasPrefix("receipt-") ? "" : b.category.rawValue
        // Real option titles from W1's card (≤ 3): a per-banner category variant, merged into the set.
        if b.itemIds.count == 1, let item = store.items.first(where: { $0.id == b.itemIds[0] }) {
            let labels = store.card(for: item).options.filter { !$0.action.needsNote }.map(\.label)
            if !labels.isEmpty, b.category != .batch, b.category != .error {
                category = "\(b.category.rawValue):\(b.id)"
                dynamicCategories[category] = Self.category(b.category, id: category, titles: labels)
                registerCategories()
            }
        }
        let line = "post \(b.category.rawValue) level=\(b.level.rawValue) sound=\(b.sound) thread=\(b.thread) id=\(b.id) category=\(category) title=\"\(b.title)\" body=\"\(b.body)\" items=\(b.itemIds.count)"
        if dryRun {
            probeCounts[b.category.rawValue, default: 0] += 1
            return probeLine("would \(line)")
        }
        guard let center else { return }
        let content = UNMutableNotificationContent()
        content.title = b.title
        content.body = b.body
        content.threadIdentifier = b.thread
        content.categoryIdentifier = category
        content.userInfo = ["items": b.itemIds]
        content.sound = b.sound ? .default : nil
        switch b.level {
        case .active: content.interruptionLevel = .active
        case .passive: content.interruptionLevel = .passive
        case .timeSensitive: content.interruptionLevel = .timeSensitive
        }
        center.add(UNNotificationRequest(identifier: b.id, content: content, trigger: nil)) { err in
            Log.write("notifications: \(line)\(err.map { " ERROR \($0)" } ?? "")")
        }
    }

    /// An item answered elsewhere (or a drained batch): the banner goes away (D8).
    private func withdraw(_ id: String) {
        for key in dynamicCategories.keys where key.hasSuffix(":\(id)") { dynamicCategories[key] = nil }
        if dryRun { return probeLine("would withdraw \(id)") }
        center?.removeDeliveredNotifications(withIdentifiers: [id])
        center?.removePendingNotificationRequests(withIdentifiers: [id])
    }

    func probeLine(_ s: String) {
        Log.write("probe-notify \(s)")
        let line = "\(Date()) \(s)\n"
        if let h = try? FileHandle(forWritingTo: Self.probeLog) {
            h.seekToEndOfFile(); h.write(Data(line.utf8)); try? h.close()
        } else {
            try? Data(line.utf8).write(to: Self.probeLog)
        }
    }

    // MARK: taps → W1's answer path (immediately, no hold) → receipt

    nonisolated func userNotificationCenter(_ center: UNUserNotificationCenter, willPresent notification: UNNotification,
                                            withCompletionHandler handler: @escaping (UNNotificationPresentationOptions) -> Void) {
        handler([.banner, .list, .sound])
    }

    nonisolated func userNotificationCenter(_ center: UNUserNotificationCenter, didReceive response: UNNotificationResponse,
                                            withCompletionHandler handler: @escaping () -> Void) {
        let action = response.actionIdentifier
        let ids = response.notification.request.content.userInfo["items"] as? [String] ?? []
        let text = (response as? UNTextInputNotificationResponse)?.userText.trimmingCharacters(in: .whitespacesAndNewlines)
        Task { @MainActor in
            self.handle(action: action, itemIds: ids, text: text)
            handler()
        }
    }

    private func handle(action: String, itemIds: [String], text: String?) {
        Log.write("notifications: action \(action) items=\(itemIds) text=\(text.map { "\($0.count) chars" } ?? "none")")
        let item = itemIds.count == 1 ? store.items.first { $0.id == itemIds[0] } : nil
        switch action {
        case UNNotificationDefaultActionIdentifier, "open":
            openOnItem(item?.id ?? itemIds.first)
        case UNNotificationDismissActionIdentifier, "later":
            break
        case "clear":
            guard let item else { return }
            answer(item, label: "cleared", action: .dismiss(id: item.id))
        case "reply":
            guard let item, let ask = item.ask, let text, !text.isEmpty else { return }
            answer(item, label: "replied", action: .answerAsk(askId: ask.id, answers: [.text(text)]))
        case "deny-note":
            guard let item, let approval = item.approval, let text, !text.isEmpty else { return }
            answer(item, label: "denied with note", action: .approve(approvalId: approval, decision: .deny), note: text)
        case "send-back":
            guard let item, let task = item.task, let text, !text.isEmpty else { return }
            answer(item, label: "sent back", action: .reviewSendBackNote(task: task, note: text))
        default:
            guard let item, let n = Self.optionIds.firstIndex(of: action) else { return }
            let card = store.card(for: item)
            let options = card.options.filter { !$0.action.needsNote }
            guard n < options.count else { return }
            answer(item, label: options[n].label, action: options[n].action)
        }
    }

    private func answer(_ item: AttentionItem, label: String, action: CardAction, note: String? = nil) {
        let who = store.card(for: item).who
        receiptFor = (who, label)
        store.answerNow(Store.Held(itemId: item.id, who: who, label: label, action: action), note: note, session: item.session)
    }

    /// The Store's outcome for the tapped action → a small `.passive` receipt banner.
    private func receipt(_ r: Store.Receipt?) {
        guard let (who, label) = receiptFor, let r else { return }
        let body: String
        switch r {
        case .holding: return
        case .sent: body = "\(label) — sent"
        case .stale: body = "already answered elsewhere"
        case .failed(let e): body = "failed: \(e)"
        case .undone: return
        case .reply: return // the Ask bar's streamed answer, not a tapped action's outcome
        }
        receiptFor = nil
        post(Banner(id: "receipt-\(UInt64(Date().timeIntervalSince1970 * 1000))", category: .batch, title: who,
                    body: body, thread: who, level: .passive, sound: false, itemIds: []))
    }
}
