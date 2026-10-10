//! 158 §7.7 / §8.4: the ICS fixtures through calcard glue, the matcher's examples, a local
//! HTTP server → rows → match → seed. No real calendar URL anywhere.

use chrono::{DateTime, TimeZone, Utc};
use meeting::calendar::fetch::{self, Fetched};
use meeting::calendar::store::FetchMeta;
use meeting::calendar::{Attendee, CalEvent, EventId, ics, matcher, seed, store};
use meeting::{Manifest, MeetingId, ProcessInfo, Store};
use tokio::io::{AsyncReadExt, AsyncWriteExt};

fn fixture(name: &str) -> String {
    std::fs::read_to_string(format!(
        "{}/tests/fixtures/calendar/{name}",
        env!("CARGO_MANIFEST_DIR")
    ))
    .unwrap()
}

fn t(s: &str) -> DateTime<Utc> {
    store::parse(s).unwrap()
}

const FROM: &str = "2026-10-01T00:00:00Z";
const TO: &str = "2026-11-15T00:00:00Z";

fn expand(name: &str) -> Vec<CalEvent> {
    ics::expand(&fixture(name), "src", t(FROM), t(TO)).unwrap()
}

// ---- §7.7 fixtures

#[test]
fn google_recurrence_exdate_and_override() {
    let rows = expand("google.ics");
    let weekly: Vec<&CalEvent> = rows
        .iter()
        .filter(|e| e.uid == "weekly-sync@example.test")
        .collect();
    let mut starts: Vec<&str> = weekly.iter().map(|e| e.start.as_str()).collect();
    starts.sort_unstable();
    // Tuesdays in the window: 6 Oct (EXDATE), 13 (moved to 11:00 PDT), 20, 27, then 3 and
    // 10 Nov after the clocks change (PST).
    assert_eq!(
        starts,
        [
            "2026-10-13T18:00:00Z",
            "2026-10-20T17:00:00Z",
            "2026-10-27T17:00:00Z",
            "2026-11-03T18:00:00Z",
            "2026-11-10T18:00:00Z",
        ]
    );
    let mut weekly = weekly;
    weekly.sort_by(|a, b| a.start.cmp(&b.start));
    let moved = weekly[0];
    assert_eq!(moved.title, "Weekly sync (moved)");
    assert_eq!(moved.sequence, 1);
    assert_eq!(moved.recurrence_id.as_deref(), Some("2026-10-13T17:00:00Z"));
    assert_eq!(
        moved.id.0,
        "src:weekly-sync@example.test:2026-10-13T17:00:00Z"
    );
    assert_eq!(moved.join_host.as_deref(), Some("zoom.us"));
    assert_eq!(moved.tz.as_deref(), Some("America/Los_Angeles"));
    // The generated instances carry their own occurrence as the recurrence id.
    assert_eq!(
        weekly[1].recurrence_id.as_deref(),
        Some("2026-10-20T17:00:00Z")
    );
    assert_eq!(weekly[1].title, "Weekly sync");
    assert_eq!(weekly[1].sequence, 0);
    // Folded DESCRIPTION, escapes, a 3-line agenda.
    let d = weekly[1].description.as_deref().unwrap();
    assert!(
        d.contains("Agenda:\n- Roadmap\n- Incidents\n- Hiring"),
        "{d}"
    );
    assert!(d.contains("Meeting ID: 987 654 3210"));
    // Attendees: the room is dropped, CN folded across a line, no CN → local part.
    let names: Vec<&str> = weekly[1]
        .attendees
        .iter()
        .map(|a| a.name.as_str())
        .collect();
    assert_eq!(names, ["Ada Lovelace", "Grace Hopper", "linus"]);
    assert_eq!(weekly[1].attendees[1].status.as_deref(), Some("ACCEPTED"));
    assert_eq!(weekly[1].attendees[2].status.as_deref(), Some("TENTATIVE"));
    assert_eq!(
        weekly[1].attendees[2].email.as_deref(),
        Some("linus@example.test")
    );
    let org = weekly[1].organizer.as_ref().unwrap();
    assert_eq!(
        (org.name.as_str(), org.email.as_deref()),
        ("Ada Lovelace", Some("ada@example.test"))
    );
}

#[test]
fn google_single_allday_cancelled_and_meet_link() {
    let rows = expand("google.ics");
    let by = |uid: &str| rows.iter().find(|e| e.uid == uid).unwrap();
    let review = by("design-review@example.test");
    assert_eq!(review.start, "2026-10-13T17:00:00Z");
    assert_eq!(review.end, "2026-10-13T17:30:00Z");
    assert_eq!(review.recurrence_id, None);
    assert_eq!(review.id.0, "src:design-review@example.test:");
    assert_eq!(review.join_host.as_deref(), Some("meet.google.com"));
    assert_eq!(
        review.join_url.as_deref(),
        Some("https://meet.google.com/abc-defg-hij")
    );
    let holiday = by("holiday@example.test");
    assert!(holiday.all_day);
    let cancelled = by("cancelled-one@example.test");
    assert!(cancelled.cancelled);
    assert_eq!(cancelled.sequence, 2);
    assert_eq!(cancelled.join_host.as_deref(), Some("zoom.us"));
}

#[test]
fn google_redirect_wrapper_is_unwrapped() {
    let (url, host) = ics::join_link(
        None,
        std::iter::empty(),
        None,
        Some("Join: https://www.google.com/url?q=https%3A%2F%2Fzoom.us%2Fj%2F42&sa=D ok"),
    )
    .unwrap();
    assert_eq!(
        (url.as_str(), host.as_str()),
        ("https://zoom.us/j/42", "zoom.us")
    );
}

#[test]
fn outlook_windows_zone_dst_and_thisandfuture() {
    let rows = expand("outlook.ics");
    let by = |uid: &str| rows.iter().find(|e| e.uid == uid).unwrap();
    let q = by("040000008200E00074C5B7101A82E00800000000A1B2C3D4E5F6@example.test");
    // 09:00 "W. Europe Standard Time" on 14 Oct (CEST) = 07:00Z; on 28 Oct (CET) = 08:00Z.
    assert_eq!(q.start, "2026-10-14T07:00:00Z");
    assert_eq!(q.tz.as_deref(), Some("W. Europe Standard Time"));
    assert_eq!(q.join_host.as_deref(), Some("teams.microsoft.com"));
    assert!(
        q.join_url
            .as_deref()
            .unwrap()
            .starts_with("https://teams.microsoft.com/l/meetup-join/")
    );
    assert_eq!(q.organizer.as_ref().unwrap().name, "Linus T");
    assert_eq!(by("after-dst@example.test").start, "2026-10-28T08:00:00Z");

    let standup: Vec<(&str, &str)> = rows
        .iter()
        .filter(|e| e.uid == "standup@example.test")
        .map(|e| (e.start.as_str(), e.title.as_str()))
        .collect();
    assert_eq!(
        standup,
        [
            ("2026-10-05T07:00:00Z", "Standup"),
            ("2026-10-12T07:00:00Z", "Standup"),
            ("2026-10-19T08:00:00Z", "Standup (later from now on)"),
            ("2026-10-26T09:00:00Z", "Standup (later from now on)"),
            ("2026-11-02T09:00:00Z", "Standup (later from now on)"),
            ("2026-11-09T09:00:00Z", "Standup (later from now on)"),
        ]
    );
    assert!(
        rows.iter()
            .filter(|e| e.uid == "standup@example.test")
            .all(|e| e.join_host.as_deref() == Some("webex.com"))
    );
    // Ids stay unique across the shifted tail.
    let mut ids: Vec<&str> = rows.iter().map(|e| e.id.0.as_str()).collect();
    ids.sort_unstable();
    ids.dedup();
    assert_eq!(ids.len(), rows.len());
}

#[test]
fn icloud_url_property_and_utc() {
    let rows = expand("icloud.ics");
    assert_eq!(rows.len(), 1);
    assert_eq!(rows[0].start, "2026-10-15T16:00:00Z");
    assert_eq!(rows[0].join_host.as_deref(), Some("facetime.apple.com"));
    assert_eq!(rows[0].tz, None);
}

#[test]
fn fastmail_floating_time_is_local_and_unknown_hosts_are_not_links() {
    let rows = expand("fastmail.ics");
    let floating = rows
        .iter()
        .find(|e| e.uid == "floating-1@example.test")
        .unwrap();
    let start = t(&floating.start).with_timezone(&chrono::Local);
    assert_eq!(
        start.format("%Y-%m-%d %H:%M").to_string(),
        "2026-10-16 09:00"
    );
    assert_eq!(
        t(&floating.end) - t(&floating.start),
        chrono::Duration::minutes(45)
    );
    assert_eq!(floating.join_host.as_deref(), Some("webex.com"));
    assert_eq!(
        floating.description.as_deref(),
        Some("No TZID and no Z: a floating time, read as local.")
    );
    let lunch = rows
        .iter()
        .find(|e| e.uid == "no-link@example.test")
        .unwrap();
    assert_eq!(lunch.join_url, None);
    assert_eq!(lunch.join_host, None);
}

#[test]
fn twenty_kb_description_and_window_filter() {
    let line = "Lorem ipsum dolor sit amet, consectetur adipiscing elit. ";
    let body = line.repeat(400); // ~23 kB
    let folded: String = body
        .as_bytes()
        .chunks(70)
        .map(|c| std::str::from_utf8(c).unwrap())
        .collect::<Vec<_>>()
        .join("\r\n ");
    let text = format!(
        "BEGIN:VCALENDAR\r\nVERSION:2.0\r\nBEGIN:VEVENT\r\nUID:big@example.test\r\n\
         DTSTART:20261020T100000Z\r\nDTEND:20261020T110000Z\r\nSUMMARY:Big\r\n\
         DESCRIPTION:{folded}\r\nEND:VEVENT\r\n\
         BEGIN:VEVENT\r\nUID:old@example.test\r\nDTSTART:20200101T100000Z\r\n\
         DTEND:20200101T110000Z\r\nSUMMARY:Old\r\nEND:VEVENT\r\nEND:VCALENDAR\r\n"
    );
    let rows = ics::expand(&text, "s", t(FROM), t(TO)).unwrap();
    assert_eq!(rows.len(), 1, "the 2020 event is outside the window");
    assert!(rows[0].description.as_deref().unwrap().len() >= 20_000);
    assert!(rows[0].to_ref().description.unwrap().chars().count() <= 1000);
    assert!(ics::expand("hello", "s", t(FROM), t(TO)).is_err());
}

// ---- §8.4 matcher

fn ev(id: &str, start: &str, end: &str, host: Option<&str>, me: Option<&str>) -> CalEvent {
    CalEvent {
        id: EventId(format!("s:{id}:")),
        source: "s".into(),
        uid: id.into(),
        title: id.into(),
        start: start.into(),
        end: end.into(),
        join_url: host.map(|h| format!("https://{h}/x")),
        join_host: host.map(str::to_owned),
        organizer: None,
        attendees: me
            .map(|s| {
                vec![Attendee {
                    name: "Me".into(),
                    email: None,
                    status: Some(s.into()),
                    is_me: true,
                }]
            })
            .unwrap_or_default(),
        description: None,
        all_day: false,
        tz: None,
        cancelled: false,
        sequence: 0,
        location: None,
        recurrence_id: None,
    }
}

const NOW: &str = "2026-10-13T17:02:00Z"; // 10:02 PDT

fn weekly() -> CalEvent {
    ev(
        "weekly",
        "2026-10-13T17:00:00Z",
        "2026-10-13T17:30:00Z",
        Some("zoom.us"),
        Some("ACCEPTED"),
    )
}
fn review() -> CalEvent {
    ev(
        "review",
        "2026-10-13T17:00:00Z",
        "2026-10-13T17:30:00Z",
        Some("meet.google.com"),
        Some("ACCEPTED"),
    )
}

fn near(a: f32, b: f32) -> bool {
    (a - b).abs() < 1e-4
}

/// `matcher::choose` is process-global (one prompt at a time in the engine): the tests that
/// score `us.zoom.xos` while `example_3` picks and clears its event take this lock.
static ZOOM: std::sync::Mutex<()> = std::sync::Mutex::new(());

fn zoom_lock() -> std::sync::MutexGuard<'static, ()> {
    ZOOM.lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner)
}

#[test]
fn example_1_zoom_holds_the_mic() {
    let _g = zoom_lock();
    let m = matcher::best(t(NOW), "us.zoom.xos", &[weekly()]).unwrap();
    assert!(near(m.score, 1.8), "{}", m.score);
    assert!(matcher::shown(&m));
    assert!(m.alternatives.is_empty());
}

#[test]
fn example_2_chrome_prefers_meet_over_zoom() {
    let m = matcher::best(t(NOW), "com.google.Chrome", &[weekly(), review()]).unwrap();
    assert_eq!(m.event.uid, "review");
    assert!(near(m.score, 1.8));
    assert!(matcher::shown(&m));
    assert_eq!(m.alternatives.len(), 1);
    assert_eq!(m.alternatives[0].uid, "weekly");
    // The runner-up really scored 0.9 (1.0 − 0.3 + 0.2).
    assert!(near(
        matcher::score(t(NOW), "com.google.Chrome", &weekly())
            .unwrap()
            .0,
        0.9
    ));
}

#[test]
fn example_3_two_zoom_events_within_the_margin_open_the_list() {
    let _g = zoom_lock();
    let now = t("2026-10-13T17:28:00Z");
    let standup = ev(
        "standup",
        "2026-10-13T17:00:00Z",
        "2026-10-13T17:30:00Z",
        Some("zoom.us"),
        None,
    );
    let one = ev(
        "one-on-one",
        "2026-10-13T17:30:00Z",
        "2026-10-13T18:00:00Z",
        Some("zoom.us"),
        None,
    );
    assert!(near(
        matcher::score(now, "us.zoom.xos", &standup).unwrap().0,
        1.6
    ));
    assert!(near(
        matcher::score(now, "us.zoom.xos", &one).unwrap().0,
        1.4
    ));
    let m = matcher::best(now, "us.zoom.xos", &[one.clone(), standup.clone()]).unwrap();
    assert_eq!(m.event.uid, "standup");
    assert!(!matcher::shown(&m), "margin 0.2 < 0.25");
    assert_eq!(m.alternatives[0].uid, "one-on-one");

    // §8.2: the user picks the 1:1 from the list; every later tick honours it.
    matcher::choose("us.zoom.xos", Some(one.id.clone()));
    let m = matcher::best(now, "us.zoom.xos", &[standup.clone(), one.clone()]).unwrap();
    assert_eq!(m.event.uid, "one-on-one");
    assert!(matcher::shown(&m));
    assert_eq!(m.alternatives[0].uid, "standup");
    // "None of these" clears the match.
    matcher::choose("us.zoom.xos", None);
    assert!(matcher::best(now, "us.zoom.xos", &[standup, one]).is_none());
    matcher::clear_choice();
}

#[test]
fn example_4_teams_with_a_lone_meet_event() {
    let lone = ev(
        "review",
        "2026-10-13T17:00:00Z",
        "2026-10-13T17:30:00Z",
        Some("meet.google.com"),
        None,
    );
    let m = matcher::best(t(NOW), "com.microsoft.teams2", &[lone]).unwrap();
    assert!(near(m.score, 0.7), "{}", m.score); // 1.0 − 0.3: a different known app
    assert!(!matcher::shown(&m));
    assert!(m.alternatives.is_empty());
}

#[test]
fn example_5_nothing_cancelled_allday_declined_and_edges() {
    let _g = zoom_lock();
    assert!(matcher::best(t(NOW), "us.zoom.xos", &[]).is_none());
    let mut cancelled = weekly();
    cancelled.cancelled = true;
    assert!(matcher::best(t(NOW), "us.zoom.xos", &[cancelled]).is_none());
    let mut allday = weekly();
    allday.all_day = true;
    assert!(matcher::best(t(NOW), "us.zoom.xos", &[allday]).is_none());
    // Declined drops below an accepted twin.
    let declined = ev(
        "declined",
        "2026-10-13T17:00:00Z",
        "2026-10-13T17:30:00Z",
        Some("zoom.us"),
        Some("DECLINED"),
    );
    let m = matcher::best(t(NOW), "us.zoom.xos", &[declined, weekly()]).unwrap();
    assert_eq!(m.event.uid, "weekly");
    assert!(matcher::shown(&m));
    // Window edges: time = 0 exactly at −10 / +15, None one second beyond.
    let start = t("2026-10-13T17:00:00Z");
    let end = t("2026-10-13T17:30:00Z");
    assert!(near(
        matcher::time_term(start - chrono::Duration::minutes(10), start, end).unwrap(),
        0.0
    ));
    assert!(near(
        matcher::time_term(end + chrono::Duration::minutes(15), start, end).unwrap(),
        0.0
    ));
    assert!(matcher::time_term(start - chrono::Duration::seconds(601), start, end).is_none());
    assert!(matcher::time_term(end + chrono::Duration::seconds(901), start, end).is_none());
    assert!(near(
        matcher::time_term(start - chrono::Duration::minutes(2), start, end).unwrap(),
        0.8
    ));
    // A lone candidate at time 0 is not offered at all.
    assert!(
        matcher::best(
            start - chrono::Duration::minutes(10),
            "us.zoom.xos",
            &[weekly()]
        )
        .is_none()
    );
}

// ---- §7.7 fetch: a local server → rows → match → seed

/// One-shot HTTP/1.1 responder: `(status line, extra headers, body)` chosen per request from
/// whether it carried `If-None-Match`.
async fn serve(body: String, status: &'static str, etag: &'static str) -> String {
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    tokio::spawn(async move {
        loop {
            let Ok((mut sock, _)) = listener.accept().await else {
                return;
            };
            let mut buf = Vec::new();
            let mut tmp = [0u8; 1024];
            while !buf.windows(4).any(|w| w == b"\r\n\r\n") {
                match sock.read(&mut tmp).await {
                    Ok(0) | Err(_) => break,
                    Ok(n) => buf.extend_from_slice(&tmp[..n]),
                }
            }
            let req = String::from_utf8_lossy(&buf).to_ascii_lowercase();
            let resp = if req.contains(&format!("if-none-match: {etag}")) {
                "HTTP/1.1 304 Not Modified\r\nConnection: close\r\n\r\n".to_owned()
            } else {
                format!(
                    "HTTP/1.1 {status}\r\nContent-Type: text/calendar\r\nETag: {etag}\r\n\
                     Content-Length: {}\r\nConnection: close\r\n\r\n{body}",
                    body.len()
                )
            };
            let _ = sock.write_all(resp.as_bytes()).await;
            let _ = sock.shutdown().await;
        }
    });
    format!("http://{addr}/cal/secret-token.ics")
}

#[tokio::test]
async fn ics_server_to_rows_to_match_to_seed() {
    let url = serve(fixture("google.ics"), "200 OK", "\"v1\"").await;
    let window = (t(FROM), t(TO));

    // 200 → rows + validators.
    let Fetched::Rows(rows, meta) = fetch::fetch_url(&url, &FetchMeta::default(), "sub", window)
        .await
        .unwrap()
    else {
        panic!("expected rows");
    };
    assert_eq!(meta.etag.as_deref(), Some("\"v1\""));
    assert!(rows.iter().any(|e| e.uid == "design-review@example.test"));

    // 304 → untouched.
    assert!(matches!(
        fetch::fetch_url(&url, &meta, "sub", window).await.unwrap(),
        Fetched::NotModified
    ));

    // 404 → the rejected message, without the URL.
    let gone = serve(String::new(), "404 Not Found", "\"x\"").await;
    let err = fetch::fetch_url(&gone, &FetchMeta::default(), "sub", window)
        .await
        .unwrap_err()
        .to_string();
    assert_eq!(err, fetch::REJECTED);
    assert!(!err.contains("secret-token"));

    // Connection refused → an error naming host + masked tail only.
    let dead = "http://127.0.0.1:9/cal/secret-token.ics";
    let err = fetch::fetch_url(dead, &FetchMeta::default(), "sub", window)
        .await
        .unwrap_err()
        .to_string();
    assert!(!err.contains("secret-token"), "{err}");
    assert!(err.contains("127.0.0.1/…"), "{err}");

    // Rows land in the store; a second replace drops what the feed no longer has.
    let dir = tempfile::tempdir().unwrap();
    let store = Store::open(dir.path()).await.unwrap();
    let src = meeting::CalSource {
        id: "sub".into(),
        kind: "ics".into(),
        name: "Synthetic".into(),
        enabled: true,
        last_fetch: None,
        last_error: None,
        state: None,
    };
    store::insert_source(&store, &src, Some("calendar.ics.sub"))
        .await
        .unwrap();
    let n = store::replace_window(&store, "sub", (FROM, TO), &rows)
        .await
        .unwrap();
    assert_eq!(n as usize, rows.len());
    store::mark_fetched(&store, "sub", &meta).await.unwrap();
    assert_eq!(store::fetch_meta(&store, "sub").await.unwrap(), meta);
    let listed = store::sources(&store).await.unwrap();
    assert_eq!(listed.len(), 1);
    assert!(listed[0].last_fetch.is_some());
    let day = store::events(&store, "2026-10-13T00:00:00Z", "2026-10-13T23:59:59Z")
        .await
        .unwrap();
    let uids: Vec<&str> = day.iter().map(|e| e.uid.as_str()).collect();
    assert_eq!(
        uids,
        [
            "holiday@example.test",
            "design-review@example.test",
            "weekly-sync@example.test",
            "cancelled-one@example.test"
        ]
    );
    let kept: Vec<CalEvent> = rows
        .iter()
        .filter(|e| e.uid != "design-review@example.test")
        .cloned()
        .collect();
    store::replace_window(&store, "sub", (FROM, TO), &kept)
        .await
        .unwrap();
    assert!(
        store::event(&store, &EventId("src:design-review@example.test:".into()))
            .await
            .unwrap()
            .is_none()
    );
    store::replace_window(&store, "sub", (FROM, TO), &rows)
        .await
        .unwrap();

    // A fake detection: Chrome holds the mic at 10:02 PDT on the 13th → Design review.
    let now = t(NOW);
    let candidates = store::events(
        &store,
        &store::stamp(now - chrono::Duration::minutes(15)),
        &store::stamp(now + chrono::Duration::minutes(10)),
    )
    .await
    .unwrap();
    let m = matcher::best(now, "com.google.Chrome", &candidates).unwrap();
    assert_eq!(m.event.title, "Design review");
    assert!(matcher::shown(&m));
    assert!(m.alternatives.is_empty(), "{:?}", m.alternatives);

    // A start with that event seeds the manifest.
    let row = store::event(&store, &m.event.id).await.unwrap().unwrap();
    let mut manifest = Manifest::new(
        MeetingId::new(),
        ProcessInfo {
            pid: 1,
            name: "Google Chrome".into(),
            bundle_id: Some("com.google.Chrome".into()),
            active: true,
        },
        Utc.with_ymd_and_hms(2026, 10, 13, 17, 2, 0)
            .unwrap()
            .to_rfc3339(),
    );
    manifest.context = Some("typed by hand".into());
    seed(&mut manifest, &row);
    assert_eq!(manifest.title.as_deref(), Some("Design review"));
    let ctx = manifest.context.as_deref().unwrap();
    assert!(
        ctx.starts_with("typed by hand\n\nEvent: Design review"),
        "{ctx}"
    );
    assert!(ctx.contains("Review the island mock."));
    assert!(
        ctx.contains("Attendees: Grace Hopper, Ada Lovelace (organizer: Grace Hopper)"),
        "{ctx}"
    );
    assert!(
        !ctx.contains("@example.test"),
        "emails never reach the model: {ctx}"
    );
    let started = meeting::session::started_line(manifest.context.as_deref());
    assert!(started.contains("Grace Hopper"));
    let e = manifest.event.as_ref().unwrap();
    assert_eq!(e.join_host.as_deref(), Some("meet.google.com"));
    assert_eq!(e.attendees.len(), 2);
    // The manifest round-trips with the new field.
    let json = serde_json::to_string(&manifest).unwrap();
    let back: Manifest = serde_json::from_str(&json).unwrap();
    assert_eq!(back.event, manifest.event);

    // Delete takes the events with it.
    assert!(store::delete_source(&store, "sub").await.unwrap());
    assert!(store::events(&store, FROM, TO).await.unwrap().is_empty());
}

#[test]
fn url_rules() {
    assert_eq!(
        fetch::normalise_url("webcal://cal.example.test/p/abc.ics").unwrap(),
        "https://cal.example.test/p/abc.ics"
    );
    assert!(fetch::normalise_url("http://cal.example.test/p/abc.ics").is_err());
    assert!(fetch::normalise_url("http://127.0.0.1:8080/x.ics").is_ok());
    assert!(fetch::normalise_url("ftp://cal.example.test/x.ics").is_err());
    assert!(fetch::normalise_url("not a url").is_err());
    assert_eq!(
        fetch::masked("https://cal.example.test/private/abcdef0123456789/basic.ics"),
        "cal.example.test/….ics"
    );
    assert_eq!(fetch::secret_name("ab12"), "calendar.ics.ab12");
}

#[test]
fn calendar_event_wire_shape_matches_the_companion() {
    // W2M decodes `{"type":"EventKit","enabled":bool}` from `subscribe_events`.
    let v = serde_json::to_value(meeting::CalendarEvent::EventKit { enabled: true }).unwrap();
    assert_eq!(v, serde_json::json!({"type": "EventKit", "enabled": true}));
    let v = serde_json::to_value(meeting::CalendarEvent::SourcesChanged).unwrap();
    assert_eq!(v, serde_json::json!({"type": "SourcesChanged"}));
}
