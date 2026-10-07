//! Design doc 122 D6: the pageable cut — fits, blank-line boundary in the last
//! 15 %, hard cut without one, exact marker, omitted headings (≤ 12), offset
//! past the end, and gap-free chaining over the `docs-page` golden.

use web::fetch::{Cut, MAX_OMITTED_HEADINGS, cut};

const DOCS_PAGE: &str = include_str!("../../firecrawl-native/tests/fixtures/docs-page.expected.md");

#[test]
fn fits_without_a_cut() {
    assert_eq!(
        cut("abc\n\ndef", 0, 100),
        Cut {
            text: "abc\n\ndef".into(),
            ..Cut::default()
        }
    );
    assert_eq!(cut("abcdef", 2, 10).text, "cdef");
    assert_eq!(cut("abcdef", 0, 6).truncated, false);
    assert_eq!(cut("", 0, 10), Cut::default());
}

#[test]
fn cuts_at_the_last_blank_line_in_the_final_15_percent() {
    // 100-char window; a blank line at char 90 (inside the last 15 %) and one at 50 (outside).
    let s = format!(
        "{}\n\n{}\n\n{}",
        "a".repeat(48),
        "b".repeat(38),
        "c".repeat(200)
    );
    let c = cut(&s, 0, 100);
    assert!(c.truncated);
    assert_eq!(c.next_offset, Some(90));
    assert!(
        c.text
            .starts_with(&format!("{}\n\n{}\n\n[… ", "a".repeat(48), "b".repeat(38))),
        "{}",
        c.text
    );
    let total = s.chars().count();
    assert_eq!(
        c.text,
        format!(
            "{}\n\n{}\n\n[… {} chars omitted; continue with offset=90]",
            "a".repeat(48),
            "b".repeat(38),
            total - 90
        )
    );
    assert!(c.omitted_headings.is_empty());
}

#[test]
fn hard_cut_when_no_blank_line_in_the_window_tail() {
    let s = format!("{}\n\n{}", "a".repeat(20), "b".repeat(500));
    let c = cut(&s, 0, 100);
    assert_eq!(c.next_offset, Some(100));
    assert_eq!(
        c.text,
        format!(
            "{}\n\n{}\n\n[… 422 chars omitted; continue with offset=100]",
            "a".repeat(20),
            "b".repeat(78)
        )
    );
    // Multi-byte chars count as one.
    let s = "é".repeat(300);
    let c = cut(&s, 0, 200);
    assert_eq!(c.next_offset, Some(200));
    assert!(c.text.starts_with(&"é".repeat(200)));
    assert!(
        c.text
            .ends_with("[… 100 chars omitted; continue with offset=200]")
    );
}

#[test]
fn omitted_headings_in_order_capped_at_twelve() {
    let mut s = String::from("intro\n\n");
    for i in 1..=15 {
        s.push_str(&format!("## Section {i}\n\n{}\n\n", "text ".repeat(20)));
    }
    s.push_str("### deep\n#notaheading\n");
    // Window 130: the intro and section 1 (123 chars) end at a blank line in the last 15 %,
    // so the tail starts at section 2.
    let c = cut(&s, 0, 130);
    assert!(c.truncated);
    assert_eq!(c.next_offset, Some(123));
    assert_eq!(c.omitted_headings.len(), MAX_OMITTED_HEADINGS);
    assert_eq!(c.omitted_headings[0], "Section 2");
    assert_eq!(c.omitted_headings[11], "Section 13");
    assert!(
        c.text.ends_with("\nOmitted sections: Section 2 · Section 3 · Section 4 · Section 5 · Section 6 · Section 7 · Section 8 · Section 9 · Section 10 · Section 11 · Section 12 · Section 13"),
        "{}",
        c.text
    );
    // A later window: the tail's `###` heading is listed, the `#hashtag` line is not.
    let c = cut(&s, c.next_offset.unwrap(), 1500);
    assert!(c.truncated);
    assert_eq!(c.omitted_headings.last().map(String::as_str), Some("deep"));
    assert!(c.omitted_headings.iter().all(|h| h != "notaheading"));
}

#[test]
fn offset_past_the_end() {
    let c = cut("abc", 5, 10);
    assert_eq!(
        c,
        Cut {
            text: "[offset 5 is past the end (3 chars)]".into(),
            ..Cut::default()
        }
    );
    assert_eq!(
        cut("abc", 3, 10).text,
        "[offset 3 is past the end (3 chars)]"
    );
}

#[test]
fn chaining_covers_the_docs_page_without_gaps_or_overlaps() {
    let total = DOCS_PAGE.chars().count();
    assert!(total > 1000, "golden is {total} chars");
    for window in [300usize, 777, 2000] {
        let mut offset = 0;
        let mut seen = 0;
        let mut hops = 0;
        loop {
            let c = cut(DOCS_PAGE, offset, window);
            hops += 1;
            assert!(hops < 1000, "no progress at offset {offset}");
            match c.next_offset {
                Some(next) => {
                    assert!(c.truncated);
                    assert!(
                        next > offset && next <= offset + window,
                        "{offset} → {next}"
                    );
                    // The kept text is exactly the original between the offsets (trailing blanks aside).
                    let kept: String = DOCS_PAGE.chars().skip(offset).take(next - offset).collect();
                    assert!(
                        c.text.starts_with(kept.trim_end()),
                        "window {window} at {offset}"
                    );
                    assert!(c.text.contains(&format!("continue with offset={next}]")));
                    seen += next - offset;
                    offset = next;
                }
                None => {
                    assert!(!c.truncated);
                    let rest: String = DOCS_PAGE.chars().skip(offset).collect();
                    assert_eq!(c.text, rest);
                    seen += total - offset;
                    break;
                }
            }
        }
        assert_eq!(seen, total, "window {window}");
    }
}
