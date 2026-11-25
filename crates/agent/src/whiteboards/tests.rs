use super::*;

const FIXTURE: &str = include_str!("../../tests/fixtures/whiteboard_outline.txt");
const ID: &str = "01a2f3c4-5b6d-7e8f-9a0b-1c2d3e4f5a6b";

/// The §3.2 example, edited: c1 archived, l1 removed, m4 answered, c2 undimmed, c3 a new version,
/// c4 edited, c5 moved into f1, c6 placed (a placeholder), l2 and m5 new.
const EDITED: &str = "<whiteboard id=\"01a2f3c4-5b6d-7e8f-9a0b-1c2d3e4f5a6b\" title=\"Attention inbox\" cards=\"5\" links=\"1\" marks=\"1\">
frame f1 \"Variants\" · phone 390×844
  row 1: c2 c3 c6
  row 2: c5
loose
  row 1: c4
c2 page \"Mock A — grouped\" v1/1 · live
c3 page \"Mock B — flat list\" v3/3 · live
c4 sticky \"Requirements\" · live · 4 lines · #a1b2
c5 diagram \"Item lifecycle\" · live · #dea4
c6 page \"Mock C — timeline\" · placeholder
l2 c3 → c6 \"timeline direction\"
m5 on c3 v3 · sent · \"page says: tab bar, item 3\"
</whiteboard>
";

const EDITED_CHANGES: &str = "<whiteboard_changes whiteboard=\"01a2f3c4-5b6d-7e8f-9a0b-1c2d3e4f5a6b\">
-c1 archived
-l1 removed
-m4 answered
~c2 live
~c3 v2 → v3
~c4 edited · 4 lines · #a1b2
~c5 moved: loose row 1 → f1 row 2
+c6 page \"Mock C — timeline\" · placeholder · in f1 row 1
+l2 c3 → c6 \"timeline direction\"
+m5 on c3 v3 · sent · \"page says: tab bar, item 3\"
</whiteboard_changes>";

/// A `<whiteboard>` block from facts (rows from each card's `at`), the shape B1 renders.
fn render(f: &Facts) -> String {
    let mut s = format!(
        "<whiteboard id=\"{}\" title=\"{}\" cards=\"{}\" links=\"{}\" marks=\"{}\">\n",
        f.id,
        attr_escape(&f.title),
        f.cards.len(),
        f.links.len(),
        f.marks.len()
    );
    let rows_of = |place: Place| {
        let mut rows: BTreeMap<u32, Vec<u32>> = BTreeMap::new();
        for (n, c) in &f.cards {
            if let Some((p, r)) = c.at
                && p == place
            {
                rows.entry(r).or_default().push(*n);
            }
        }
        rows
    };
    let write_rows = |s: &mut String, rows: BTreeMap<u32, Vec<u32>>| {
        for (r, cards) in rows {
            let names: Vec<String> = cards.iter().map(|n| format!("c{n}")).collect();
            let _ = writeln!(s, "  row {r}: {}", names.join(" "));
        }
    };
    for (n, fr) in &f.frames {
        let _ = writeln!(s, "{}", fr.line(*n));
        write_rows(&mut s, rows_of(Place::Frame(*n)));
    }
    let loose = rows_of(Place::Loose);
    if !loose.is_empty() {
        s.push_str("loose\n");
        write_rows(&mut s, loose);
    }
    for (n, c) in &f.cards {
        let _ = writeln!(s, "{}", c.line(*n));
    }
    for (n, l) in &f.links {
        let _ = writeln!(s, "{}", l.line(*n));
    }
    for (n, m) in &f.marks {
        let _ = writeln!(s, "{}", m.line(*n));
    }
    s.push_str("</whiteboard>\n");
    s
}

fn board(text: &str) -> Bound {
    let f = parse_outline(text).expect("outline parses");
    Bound {
        doc: f.id.clone(),
        title: f.title.clone(),
        cards: f.cards.len() as u32,
        updated_ms: 0,
        text: text.to_owned(),
        aliases: BTreeMap::new(),
        context: None,
    }
}

fn sticky(n: u32, row: u32) -> Card {
    Card {
        kind: "sticky".into(),
        title: format!("Note {n}"),
        versions: None,
        state: CardState::Live,
        tag: None,
        extras: " · 1 lines · #0000".into(),
        at: Some((Place::Loose, row)),
    }
}

#[test]
fn parses_the_contract_outline() {
    assert!(FIXTURE.ends_with("</whiteboard>\n") && !FIXTURE.contains('\r'));
    let f = parse_outline(FIXTURE).expect("the fixture parses");
    assert_eq!(f.id, ID);
    assert_eq!(f.title, "Attention inbox");
    assert_eq!(f.frames.len(), 1);
    assert_eq!(
        f.frames[&1],
        Frame {
            title: "Variants".into(),
            preset: Some("phone 390×844".into())
        }
    );
    assert_eq!(f.cards.len(), 5);
    let c2 = &f.cards[&2];
    assert_eq!(
        (c2.kind.as_str(), c2.title.as_str(), c2.versions, c2.state, c2.tag.as_deref()),
        ("page", "Mock A — grouped", Some(1), CardState::Dimmed, Some("header kept; grouping rejected"))
    );
    assert_eq!(f.cards[&3].versions, Some(2));
    assert_eq!(f.cards[&4].extras, " · 3 lines · #7419");
    assert_eq!(f.cards[&4].versions, None);
    assert_eq!(f.cards[&5].extras, " · #dea4");
    for n in 1..=3 {
        assert_eq!(f.cards[&n].at, Some((Place::Frame(1), 1)), "c{n}");
    }
    for n in 4..=5 {
        assert_eq!(f.cards[&n].at, Some((Place::Loose, 1)), "c{n}");
    }
    assert_eq!(
        f.links[&1],
        Link {
            from: 4,
            arrow: "→".into(),
            to: 5,
            label: Some("drives".into())
        }
    );
    let m4 = &f.marks[&4];
    assert_eq!((m4.on.as_str(), m4.state), ("c3 v2", MarkState::Sent));
    assert_eq!(m4.rest, " · \"page says: tab bar, item 3\" · handwriting: \"no third tab\"");
    // Every line re-renders byte for byte: what the parser keeps is what B1 wrote.
    assert_eq!(render(&f), FIXTURE);
}

#[test]
fn baseline_resets_on_a_block() {
    let changes = turn_messages(&[board(EDITED)], &[FIXTURE]);
    let second = FIXTURE.replace("Attention inbox", "Second look");
    let history = [FIXTURE.to_owned(), changes[0].clone(), second.clone()];
    assert_eq!(Baseline::of(&history, ID), parse_outline(&second));
}

#[test]
fn baseline_applies_changes() {
    let history = [FIXTURE, EDITED_CHANGES];
    assert_eq!(Baseline::of(&history, ID), parse_outline(EDITED));
    // Another whiteboard's messages are not this one's.
    let other = FIXTURE.replace(ID, "019a0000-0000-7000-8000-00000000000f");
    assert_eq!(Baseline::of(&[other.as_str()], ID), None);
}

#[test]
fn changes_without_a_block_are_ignored() {
    assert_eq!(Baseline::of(&[EDITED_CHANGES], ID), None);
    assert_eq!(
        Baseline::of(&[EDITED_CHANGES, FIXTURE], ID),
        parse_outline(FIXTURE),
        "changes before the block are not applied"
    );
    // No baseline → the full block.
    assert_eq!(turn_messages(&[board(EDITED)], &[EDITED_CHANGES]), vec![EDITED.to_owned()]);
}

#[test]
fn brief_line_is_not_a_baseline() {
    let brief = brief_line(ID, "Attention \"inbox\" & co", 5);
    assert_eq!(
        brief,
        "<whiteboard id=\"01a2f3c4-5b6d-7e8f-9a0b-1c2d3e4f5a6b\" title=\"Attention &quot;inbox&quot; &amp; co\" cards=\"5\" brief=\"true\"/>"
    );
    assert_eq!(parse_outline(&brief), None);
    assert_eq!(Baseline::of(&[brief.as_str()], ID), None);
    assert_eq!(turn_messages(&[board(FIXTURE)], &[brief.as_str()]), vec![FIXTURE.to_owned()]);
}

#[test]
fn diff_golden() {
    let out = turn_messages(&[board(EDITED)], &[FIXTURE]);
    assert_eq!(out, vec![EDITED_CHANGES.to_owned()]);
    // The placeholder fills: `filled`, and nothing else for that card.
    let filled = EDITED.replace(
        "c6 page \"Mock C — timeline\" · placeholder",
        "c6 page \"Mock C — timeline\" v1/1 · live",
    );
    let out = turn_messages(&[board(&filled)], &[FIXTURE, EDITED_CHANGES]);
    assert_eq!(
        out,
        vec![format!("<whiteboard_changes whiteboard=\"{ID}\">\n~c6 filled v1\n</whiteboard_changes>")]
    );
    // Nothing moved: no message.
    assert!(turn_messages(&[board(FIXTURE)], &[FIXTURE]).is_empty());
}

#[test]
fn changes_round_trip() {
    let base = parse_outline(FIXTURE).unwrap();
    let mut next = base.clone();
    next.title = "Inbox \"v2\" \\ draft".into();
    next.frames.get_mut(&1).unwrap().title = "Variants, round 2".into();
    next.frames.get_mut(&1).unwrap().preset = Some("tablet 820×1180".into());
    next.frames.insert(
        2,
        Frame {
            title: "Parking".into(),
            preset: None,
        },
    );
    let c1 = next.cards.get_mut(&1).unwrap();
    c1.state = CardState::Dimmed;
    c1.tag = Some("too \"busy\"".into());
    c1.title = "Today, again".into();
    c1.at = Some((Place::Frame(2), 1));
    next.cards.get_mut(&2).unwrap().tag = Some("header kept".into());
    let l1 = next.links.get_mut(&1).unwrap();
    l1.arrow = "↔".into();
    l1.label = None;
    next.links.insert(
        2,
        Link {
            from: 3,
            arrow: "—".into(),
            to: 2,
            label: None,
        },
    );
    next.marks.get_mut(&4).unwrap().state = MarkState::Pinned;
    next.marks.insert(
        6,
        Mark {
            on: "canvas".into(),
            state: MarkState::Sent,
            rest: " · handwriting: \"why?\"".into(),
        },
    );
    let mut c7 = sticky(7, 2);
    c7.state = CardState::Placeholder;
    c7.extras.clear();
    next.cards.insert(7, c7);
    let text = render(&next);
    let first = turn_messages(&[board(&text)], &[FIXTURE]);
    assert_eq!(first.len(), 1);
    let history = [FIXTURE.to_owned(), first[0].clone()];
    assert_eq!(Baseline::of(&history, ID).as_ref(), Some(&next));
    assert!(turn_messages(&[board(&text)], &history).is_empty(), "applied: no further diff");
    // And back: the preset goes, the frame goes, the mark unpins.
    let mut back = next.clone();
    back.frames.get_mut(&1).unwrap().preset = None;
    back.frames.remove(&2);
    back.cards.get_mut(&1).unwrap().at = Some((Place::Loose, 2));
    back.marks.get_mut(&4).unwrap().state = MarkState::Sent;
    back.cards.remove(&7);
    let text2 = render(&back);
    let second = turn_messages(&[board(&text2)], &history);
    let history2 = [history[0].clone(), history[1].clone(), second[0].clone()];
    assert_eq!(Baseline::of(&history2, ID).as_ref(), Some(&back));
}

#[test]
fn cap_at_40_lines_heals_next_turn() {
    let empty = format!("<whiteboard id=\"{ID}\" title=\"t\" cards=\"0\" links=\"0\" marks=\"0\">\n</whiteboard>\n");
    let mut f = parse_outline(&empty).unwrap();
    for n in 1..=50 {
        f.cards.insert(n, sticky(n, n));
    }
    let full = render(&f);
    let first = turn_messages(&[board(&full)], &[empty.as_str()]);
    let lines: Vec<&str> = first[0].lines().collect();
    assert_eq!(lines.iter().filter(|l| l.starts_with('+')).count(), MAX_CHANGES);
    assert!(lines.contains(&"… 10 more changes — wb_read for the full outline"));
    assert!(lines.contains(&"+c40 sticky \"Note 40\" · live · 1 lines · #0000 · in loose row 40"));
    let history = vec![empty.clone(), first[0].clone()];
    let second = turn_messages(&[board(&full)], &history);
    assert_eq!(second[0].lines().filter(|l| l.starts_with('+')).count(), 10);
    assert!(!second[0].contains("more changes"));
    let history = vec![empty, first[0].clone(), second[0].clone()];
    assert!(turn_messages(&[board(&full)], &history).is_empty(), "healed");
}

#[test]
fn full_block_again_after_compaction() {
    // Before the compaction the transcript holds the block: a change note only.
    assert_eq!(turn_messages(&[board(EDITED)], &[FIXTURE]), vec![EDITED_CHANGES.to_owned()]);
    // The compacted context starts from a summary: no baseline, so the full block again.
    let compacted = ["<summary>we compared three mocks</summary>", EDITED_CHANGES];
    assert_eq!(turn_messages(&[board(EDITED)], &compacted), vec![EDITED.to_owned()]);
}

#[test]
fn a_mark_reaches_each_bound_session_once() {
    // Session X already heard about m5; session Y has only seen the original block.
    let x = [FIXTURE, EDITED_CHANGES];
    let y = [FIXTURE];
    assert!(turn_messages(&[board(EDITED)], &x).is_empty());
    let to_y = turn_messages(&[board(EDITED)], &y);
    assert_eq!(to_y[0].matches("+m5 ").count(), 1);
    let y2 = [FIXTURE, to_y[0].as_str()];
    assert!(turn_messages(&[board(EDITED)], &y2).is_empty(), "once");
}

#[test]
fn context_and_image_lines() {
    let ulid = |n: u32| format!("01KAR3ZC4M00000000000000{n:02}");
    let aliases: BTreeMap<String, String> = [
        (ulid(9), "f1"),
        (ulid(4), "c4"),
        (ulid(3), "c3"),
        (ulid(8), "m4"),
        (ulid(10), "m6"),
    ]
    .into_iter()
    .map(|(k, v)| (k, v.to_owned()))
    .collect();
    let ctx = WbSendContext {
        in_view: vec![ulid(9), ulid(4), "01KAR3ZC4M0000000000000099".into()],
        selected: vec![ulid(3)],
        images: vec![
            crate::rpc_whiteboards::WbSentImage {
                card: Some(ulid(3)),
                marks: vec![ulid(8)],
            },
            crate::rpc_whiteboards::WbSentImage {
                card: None,
                marks: vec![ulid(10)],
            },
        ],
        omitted: 1,
        omitted_why: Some("budget".into()),
    };
    let lines = [
        "in view: f1 \"Variants\", c4",
        "selected: c3",
        "image 1: c3 · marks m4",
        "image 2: canvas · marks m6",
        "images omitted: 1 (budget)",
    ];
    let mut b = board(FIXTURE);
    b.aliases = aliases;
    b.context = Some(ctx);
    // First send: the full block, then a note with only the context.
    let out = turn_messages(std::slice::from_ref(&b), &[] as &[&str]);
    assert_eq!(out.len(), 2);
    assert_eq!(out[0], FIXTURE);
    assert_eq!(
        out[1],
        format!("<whiteboard_changes whiteboard=\"{ID}\">\n{}\n</whiteboard_changes>", lines.join("\n"))
    );
    // Later: context first, then the changes, in one note; context is not a change.
    let mut e = b.clone();
    e.text = EDITED.to_owned();
    let out = turn_messages(&[e], &[FIXTURE]);
    assert_eq!(out.len(), 1);
    assert!(out[0].starts_with(&format!("<whiteboard_changes whiteboard=\"{ID}\">\n{}\n-c1 archived\n", lines.join("\n"))));
    assert_eq!(Baseline::of(&[FIXTURE, out[0].as_str()], ID), parse_outline(EDITED));
    // `no vision` is said as such.
    let ctx = WbSendContext {
        omitted: 2,
        omitted_why: Some("no vision".into()),
        ..WbSendContext::default()
    };
    assert_eq!(context_lines(&ctx, &BTreeMap::new(), None), vec!["images omitted: 2 (no vision)"]);
}

#[test]
fn more_than_three_whiteboards() {
    let boards: Vec<Bound> = (1..=5)
        .map(|i| {
            let id = format!("019a0000-0000-7000-8000-00000000000{i}");
            let mut b = board(&FIXTURE.replace(ID, &id));
            b.title = format!("Board {i}");
            b.updated_ms = i64::from(i) * 1000;
            b
        })
        .collect();
    let out = turn_messages(&boards, &[] as &[&str]);
    assert_eq!(out.len(), 4, "three blocks and one message of brief lines");
    for (msg, i) in out.iter().zip([5, 4, 3]) {
        assert!(msg.starts_with(&format!("<whiteboard id=\"019a0000-0000-7000-8000-00000000000{i}\"")));
        assert!(!msg.contains("brief="));
    }
    assert_eq!(
        out[3],
        "<whiteboard id=\"019a0000-0000-7000-8000-000000000002\" title=\"Board 2\" cards=\"5\" brief=\"true\"/>\n\
         <whiteboard id=\"019a0000-0000-7000-8000-000000000001\" title=\"Board 1\" cards=\"5\" brief=\"true\"/>"
    );
    // Next turn: nothing changed, the brief lines are not repeated.
    assert!(turn_messages(&boards, &out).is_empty());
    // A send from board 1 promotes it to a full block.
    let mut sent = boards.clone();
    sent[0].context = Some(WbSendContext::default());
    let again = turn_messages(&sent, &out);
    assert_eq!(again.len(), 1);
    assert!(again[0].starts_with("<whiteboard id=\"019a0000-0000-7000-8000-000000000001\""));
}

#[test]
fn quote_cuts_and_escapes() {
    assert_eq!(quote("a \"b\" \\ c\nd", 80), "\"a \\\"b\\\" \\\\ c d\"");
    let long = "x".repeat(100);
    let q = quote(&long, 80);
    assert_eq!(q.chars().count(), 82);
    assert!(q.ends_with("…\""));
    assert_eq!(unquote(&q).unwrap().0.chars().count(), 80);
    assert_eq!(unquote("\"a\\\"b\" rest"), Some(("a\"b".to_owned(), " rest")));
}

#[test]
fn send_context_is_taken_once_and_falls_back_to_any_session() {
    let wb = "019a0000-0000-7000-8000-0000000000aa";
    let (a, b) = (SessionId::new(), SessionId::new());
    let ctx = WbSendContext {
        selected: vec!["x".into()],
        ..WbSendContext::default()
    };
    put_context(Some(a), wb, ctx.clone());
    assert_eq!(take_context(b, wb), None, "another session's");
    assert_eq!(take_context(a, wb), Some(ctx.clone()));
    assert_eq!(take_context(a, wb), None, "consumed");
    put_context(None, wb, ctx.clone());
    assert_eq!(take_context(b, wb), Some(ctx), "a first send is anyone's");
}
