import AppKit
import Combine
import EventKit
import TheuthKit

/// 158 §7.2: the macOS calendar source. Starts when the engine says `meeting.calendar.eventkit` is on (the `EventKit`
/// bus item, and `calendar_settings_get` on every connect), asks for Calendars full access (the companion is the
/// TCC-responsible app), reports `calendar_source_state`, and pushes the −1 … +14 day window with `calendar_push`
/// on start, on every `EKEventStoreChanged` (debounced 2 s), on wake, and every 15 min. Push, not poll (§7.2).
@MainActor
final class EventKitSource {
    static var shared: EventKitSource?

    /// What `calendar_settings_get` answers; only the one key read here.
    private struct Settings: Decodable { let eventkit: Bool }

    private let client: RpcClient
    private let store = EKEventStore()
    private var enabled = false
    private var observers: [NSObjectProtocol] = []
    private var timer: Timer?
    private var debounce: Timer?
    private var subs = Set<AnyCancellable>()
    private var pushing = false
    private var pushAgain = false

    /// The one hook (AppDelegate): never under a probe — a second instance must neither prompt nor push.
    static func install(store s: Store) {
        guard !AskFlags.automated, let client = s.client else { return }
        let src = EventKitSource(client: client)
        shared = src
        s.$connection.receive(on: RunLoop.main).sink { state in
            MainActor.assumeIsolated { if state == .open { src.connected() } }
        }.store(in: &src.subs)
    }

    private init(client: RpcClient) { self.client = client }

    // MARK: engine → companion

    /// `{"type":"EventKit","enabled":…}` (158 §15.1 `CalendarEvent`); every other item is ignored.
    func observe(_ data: Data) {
        struct Raw: Decodable { let type: String; let enabled: Bool? }
        guard let r = try? TheuthJSON.decoder.decode(Raw.self, from: data), r.type == "EventKit", let on = r.enabled else { return }
        set(enabled: on)
    }

    /// A (re)connect: the engine's current setting wins, and a push repeats (the engine may have restarted).
    private func connected() {
        Task {
            do {
                let s: Settings = try await client.call("calendar_settings_get")
                set(enabled: s.eventkit, force: true)
            } catch {
                Log.write("calendar: settings read failed: \(error)")
            }
        }
    }

    private func set(enabled on: Bool, force: Bool = false) {
        guard on != enabled || force else { return }
        enabled = on
        guard on else { return stop() }
        Task { await start() }
    }

    // MARK: lifecycle

    private func start() async {
        // macOS 14+: `.fullAccess` reads events; `.writeOnly` cannot, so it counts as denied.
        var status = EKEventStore.authorizationStatus(for: .event)
        if status == .notDetermined {
            do { _ = try await store.requestFullAccessToEvents() } catch { Log.write("calendar: access request failed: \(error)") }
            status = EKEventStore.authorizationStatus(for: .event)
        }
        guard enabled else { return } // flipped off while the prompt was up
        let state = switch status {
        case .fullAccess: "granted"
        case .notDetermined: "not_determined"
        default: "denied"
        }
        Log.write("calendar: eventkit \(state)")
        do { try await client.calendarSourceState(source: CalEvent.eventKitSource, state: state) } catch { Log.write("calendar: state report failed: \(error)") }
        guard status == .fullAccess else { return }
        if observers.isEmpty {
            observers = [
                NotificationCenter.default.addObserver(forName: .EKEventStoreChanged, object: store, queue: .main) { [weak self] _ in
                    MainActor.assumeIsolated { self?.schedulePush() }
                },
                NSWorkspace.shared.notificationCenter.addObserver(forName: NSWorkspace.didWakeNotification, object: nil, queue: .main) { [weak self] _ in
                    MainActor.assumeIsolated { self?.schedulePush() }
                },
            ]
        }
        timer?.invalidate()
        timer = Timer.scheduledTimer(withTimeInterval: 15 * 60, repeats: true) { [weak self] _ in
            Task { @MainActor in self?.schedulePush() }
        }
        schedulePush(now: true)
    }

    private func stop() {
        for o in observers { NotificationCenter.default.removeObserver(o); NSWorkspace.shared.notificationCenter.removeObserver(o) }
        observers = []
        timer?.invalidate(); timer = nil
        debounce?.invalidate(); debounce = nil
    }

    // MARK: push

    /// Changes arrive in bursts (one notification per touched calendar); 2 s folds them into one push.
    private func schedulePush(now immediate: Bool = false) {
        debounce?.invalidate()
        if immediate { return push() }
        debounce = Timer.scheduledTimer(withTimeInterval: 2, repeats: false) { [weak self] _ in
            Task { @MainActor in self?.push() }
        }
    }

    /// One in flight; a request during it runs once after it.
    private func push() {
        guard enabled else { return }
        guard !pushing else { pushAgain = true; return }
        pushing = true
        Task {
            let now = Date()
            let window = (now.addingTimeInterval(-86_400), now.addingTimeInterval(14 * 86_400))
            let events = store.events(matching: store.predicateForEvents(withStart: window.0, end: window.1, calendars: nil))
                .map { CalEvent(EventFields($0)) }
            do {
                let n = try await client.calendarPush(source: CalEvent.eventKitSource,
                                                      window: (CalEvent.rfc3339.string(from: window.0), CalEvent.rfc3339.string(from: window.1)),
                                                      events: events)
                Log.write("calendar: pushed \(events.count) events (\(n) rows)")
            } catch {
                Log.write("calendar: push failed: \(error)")
            }
            pushing = false
            if pushAgain { pushAgain = false; schedulePush() }
        }
    }
}

extension Store {
    /// The one line in `Store.handle` (158 §7.2).
    func calendarObserve(_ data: Data) { EventKitSource.shared?.observe(data) }
}
