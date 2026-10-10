import EventKit
import Foundation
import Testing
import TheuthKit
@testable import Companion

/// 158 §7.2 proof: `EKEvent`-like fixtures map to the contract's `CalEvent` (join link, attendees, all-day, recurrence).
struct EventKitMappingTests {
    private let t0 = Date(timeIntervalSince1970: 1_791_453_600) // 2026-10-10T09:00:00Z

    private func fixture(_ notes: String? = nil, location: String? = nil, url: URL? = nil) -> EventFields {
        var f = EventFields(uid: "ABC-123", title: "Weekly sync", start: t0, end: t0.addingTimeInterval(1800))
        f.notes = notes; f.location = location; f.url = url; f.tz = "Europe/London"
        return f
    }

    @Test func joinUrlFromEventUrlWins() {
        let e = CalEvent(fixture("https://meet.google.com/abc-defg-hij", url: URL(string: "https://us02web.zoom.us/j/123?pwd=x")))
        #expect(e.joinUrl == "https://us02web.zoom.us/j/123?pwd=x")
        #expect(e.joinHost == "us02web.zoom.us")
    }

    @Test func joinUrlFromNotesAndLocation() {
        let teams = "Microsoft Teams meeting\nJoin on your computer: <https://teams.microsoft.com/l/meetup-join/19%3ameeting_abc%40thread.v2/0?context=%7b%7d>\nMeeting ID: 1"
        #expect(CalEvent(fixture(teams)).joinHost == "teams.microsoft.com")
        let meet = CalEvent(fixture(nil, location: "https://meet.google.com/abc-defg-hij, Room 4"))
        #expect(meet.joinUrl == "https://meet.google.com/abc-defg-hij")
        #expect(meet.joinHost == "meet.google.com")
        // The event's own URL is not a meeting link: the notes' Zoom link wins over it.
        let zoom = CalEvent(fixture("Agenda https://example.com/doc then https://zoom.us/j/999", url: URL(string: "https://example.com/agenda")))
        #expect(zoom.joinUrl == "https://zoom.us/j/999")
        #expect(zoom.joinHost == "zoom.us")
    }

    @Test func googleRedirectUnwrapped() {
        let e = CalEvent(fixture("https://www.google.com/url?q=https://company.webex.com/meet/sky&sa=D"))
        #expect(e.joinUrl == "https://company.webex.com/meet/sky")
        #expect(e.joinHost == "company.webex.com")
    }

    @Test func noJoinLinkForPlainText() {
        let e = CalEvent(fixture("Lunch at https://example.com/menu", location: "Café"))
        #expect(e.joinUrl == nil && e.joinHost == nil)
        #expect(e.location == "Café")
    }

    @Test func attendeesAndOrganizer() {
        var f = fixture()
        f.organizer = .init(name: "Ada", email: "mailto:ada@example.com")
        f.attendees = [
            .init(name: "Sky Moore", email: "mailto:sky@example.com", status: .accepted, isMe: true),
            .init(name: nil, email: "mailto:bob@example.com", status: .declined),
            .init(name: "", email: nil, status: .pending),
        ]
        let e = CalEvent(f)
        #expect(e.organizer == Attendee(name: "Ada", email: "ada@example.com"))
        #expect(e.attendees[0] == Attendee(name: "Sky Moore", email: "sky@example.com", status: "ACCEPTED", isMe: true))
        #expect(e.attendees[1] == Attendee(name: "bob", email: "bob@example.com", status: "DECLINED"))
        #expect(e.attendees[2] == Attendee(name: "", email: nil, status: "NEEDS-ACTION"))
    }

    @Test func allDayAndTimes() {
        var f = fixture()
        f.isAllDay = true
        f.end = t0.addingTimeInterval(86_400)
        let e = CalEvent(f)
        #expect(e.allDay)
        #expect(e.start == "2026-10-10T09:00:00Z")
        #expect(e.end == "2026-10-11T09:00:00Z")
        #expect(e.tz == "Europe/London")
        #expect(e.id == "eventkit:ABC-123:")
        #expect(e.recurrenceId == nil)
        #expect(e.source == "eventkit" && e.uid == "ABC-123")
    }

    @Test func recurringInstanceCarriesOccurrence() {
        var f = fixture()
        f.occurrence = t0
        f.cancelled = true
        let e = CalEvent(f)
        #expect(e.recurrenceId == "2026-10-10T09:00:00Z")
        #expect(e.id == "eventkit:ABC-123:2026-10-10T09:00:00Z")
        #expect(e.cancelled)
    }

    @Test func wireShapeIsSnakeCase() throws {
        let data = try TheuthJSON.encoder.encode(CalEvent(fixture("https://zoom.us/j/1")))
        let s = String(decoding: data, as: UTF8.self)
        #expect(s.contains("\"join_host\":\"zoom.us\"") && s.contains("\"all_day\":false") && s.contains("\"start\":\"2026-10-10T09:00:00Z\""))
    }
}
