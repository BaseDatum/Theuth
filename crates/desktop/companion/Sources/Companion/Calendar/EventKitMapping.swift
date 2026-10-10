import EventKit
import Foundation
import TheuthKit

/// The fields of one `EKEvent` the push reads (158 §7.2), as a plain value so the mapping is testable without
/// an `EKEventStore` (`EKEvent` cannot be given attendees or an organizer by hand).
struct EventFields: Equatable {
    struct Person: Equatable {
        var name: String?
        /// `mailto:` URL as EventKit gives it, or a bare address.
        var email: String?
        var status: EKParticipantStatus = .unknown
        var isMe = false
    }

    var uid: String
    var title: String?
    var start: Date
    var end: Date
    var isAllDay = false
    var tz: String?
    var location: String?
    var notes: String?
    var url: URL?
    var organizer: Person?
    var attendees: [Person] = []
    /// `occurrenceDate` when the event is an instance of a recurring series (or a detached override).
    var occurrence: Date?
    var cancelled = false

    init(uid: String, title: String?, start: Date, end: Date) {
        self.uid = uid; self.title = title; self.start = start; self.end = end
    }

    init(_ e: EKEvent) {
        self.init(uid: e.calendarItemExternalIdentifier ?? e.calendarItemIdentifier, title: e.title, start: e.startDate, end: e.endDate)
        isAllDay = e.isAllDay
        tz = e.timeZone?.identifier
        location = e.location
        notes = e.notes
        url = e.url
        organizer = e.organizer.map(Person.init)
        attendees = (e.attendees ?? []).map(Person.init)
        if e.hasRecurrenceRules || e.isDetached { occurrence = e.occurrenceDate }
        cancelled = e.status == .canceled
    }
}

extension EventFields.Person {
    init(_ p: EKParticipant) {
        self.init(name: p.name, email: p.url.absoluteString, status: p.participantStatus, isMe: p.isCurrentUser)
    }

    /// `Attendee` (158 §8.3): name, else the address's local part; status as the ICS words the engine stores.
    var attendee: Attendee {
        let mail = email.map { $0.hasPrefix("mailto:") ? String($0.dropFirst(7)) : $0 }.flatMap { $0.isEmpty ? nil : $0 }
        let name = (name?.isEmpty == false ? name : nil) ?? mail.map { String($0.prefix { $0 != "@" }) } ?? ""
        let status: String? = switch self.status {
        case .accepted: "ACCEPTED"
        case .tentative: "TENTATIVE"
        case .declined: "DECLINED"
        case .pending: "NEEDS-ACTION"
        default: nil
        }
        return Attendee(name: name, email: mail, status: status, isMe: isMe)
    }
}

extension CalEvent {
    static let eventKitSource = "eventkit"

    /// `CalEvent` (158 §7.4) from one event; `id` is `source:uid:recurrence_id` (empty `recurrence_id` for a one-off).
    init(_ f: EventFields) {
        let recurrence = f.occurrence.map(CalEvent.rfc3339.string)
        let join = JoinLink.find(url: f.url, location: f.location, notes: f.notes)
        self.init(id: "\(Self.eventKitSource):\(f.uid):\(recurrence ?? "")", source: Self.eventKitSource, uid: f.uid,
                  title: f.title ?? "", start: Self.rfc3339.string(from: f.start), end: Self.rfc3339.string(from: f.end),
                  joinUrl: join?.absoluteString, joinHost: join?.host, organizer: f.organizer?.attendee,
                  attendees: f.attendees.map(\.attendee), description: f.notes.flatMap { $0.isEmpty ? nil : $0 },
                  allDay: f.isAllDay, tz: f.tz, cancelled: f.cancelled, location: f.location.flatMap { $0.isEmpty ? nil : $0 },
                  recurrenceId: recurrence)
    }

    /// RFC 3339 UTC, whole seconds (`2026-10-10T09:00:00Z`), the form `cal_events.start` stores.
    static let rfc3339: ISO8601DateFormatter = {
        let f = ISO8601DateFormatter()
        f.formatOptions = [.withInternetDateTime]
        f.timeZone = TimeZone(identifier: "UTC")
        return f
    }()
}

/// The join link (158 §7.3 "Join link", shared rules with the engine's ICS reader): the event's URL first, then the first
/// URL in `location` / `notes` whose host matches a meeting service. Google redirect wrappers are unwrapped.
enum JoinLink {
    /// OpenWhispr's host rules (158 §8.1 host map); a leading `.` means "any subdomain".
    static let hosts = ["zoom.us", ".zoom.us", "meet.google.com", "teams.microsoft.com", "teams.live.com", ".webex.com", "facetime.apple.com"]

    static func find(url: URL?, location: String?, notes: String?) -> URL? {
        if let u = url.flatMap(unwrap), isMeeting(u) { return u }
        for text in [location, notes].compactMap({ $0 }) {
            if let u = urls(in: text).lazy.map(unwrap).first(where: isMeeting) { return u }
        }
        return nil
    }

    static func isMeeting(_ u: URL) -> Bool {
        guard let host = u.host?.lowercased() else { return false }
        return hosts.contains { $0.hasPrefix(".") ? host.hasSuffix($0) : host == $0 }
    }

    /// `https://www.google.com/url?q=<target>&…` → `<target>`.
    static func unwrap(_ u: URL) -> URL {
        guard let host = u.host?.lowercased(), host == "www.google.com" || host == "google.com", u.path == "/url",
              let q = URLComponents(url: u, resolvingAgainstBaseURL: false)?.queryItems?.first(where: { $0.name == "q" })?.value,
              let target = URL(string: q) else { return u }
        return target
    }

    /// Foundation's link detector over free text (handles `<…>`, trailing punctuation, bare `zoom.us/j/…`).
    static func urls(in text: String) -> [URL] {
        guard let d = try? NSDataDetector(types: NSTextCheckingResult.CheckingType.link.rawValue) else { return [] }
        return d.matches(in: text, range: NSRange(text.startIndex..., in: text)).compactMap(\.url)
    }
}
