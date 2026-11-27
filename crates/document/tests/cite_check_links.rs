//! 144 §6.6: what `doc_insert` / `doc_replace` / `doc_create` do with `theuth://` links — extraction, the
//! refusal of `missing` / `unavailable` targets with the exact message, `changed` accepted, `?s=` added when
//! absent and kept when present.

use std::collections::HashMap;

use document::cite_check::{check_theuth_links, judge, links, sha8};
use document::{Citation, CiteKind, LocateStatus};
use surface::anchor::{Locator, TheuthAnchor, TheuthKind};

fn cite(a: &TheuthAnchor, status: LocateStatus, available: bool, text: Option<&str>) -> Citation {
    Citation {
        href: a.href(),
        kind: match a.kind {
            TheuthKind::Notes => CiteKind::Notes,
            TheuthKind::Sources => CiteKind::Sources,
            TheuthKind::Knowledge => CiteKind::Knowledge,
            TheuthKind::Meetings => CiteKind::Meetings,
            TheuthKind::Sessions => CiteKind::Sessions,
        },
        id: a.id.clone(),
        locator: None,
        sha8: a.sha8.clone(),
        status,
        available,
        title: "t".into(),
        text: text.map(str::to_owned),
    }
}

fn parsed(md: &str) -> Vec<(&str, TheuthAnchor)> {
    links(md)
        .into_iter()
        .map(|(raw, p)| (raw, p.unwrap()))
        .collect()
}

#[test]
fn finds_every_theuth_link_once_and_skips_other_schemes() {
    let md = "See [runbook §4.2](theuth://sources/01S1#b12) and [again](theuth://sources/01S1#b12), \
              a [note](theuth://notes/01N1#x?s=0123abcd), [code](code:src/a.rs#L1) and [web](https://x.y/).";
    let got = check_theuth_links(md);
    assert_eq!(got.len(), 2);
    assert_eq!(got[0].kind, TheuthKind::Sources);
    assert_eq!(got[0].locator, Some(Locator::Block("b12".into())));
    assert_eq!(got[1].sha8.as_deref(), Some("0123abcd"));
    // A malformed href is listed (so `check` can refuse it) but never parsed into the anchors.
    let md = "[bad](theuth://pages/01#x) [ok](theuth://sessions/01S#3)";
    assert_eq!(check_theuth_links(md).len(), 1);
    assert!(links(md)[0].1.is_err());
}

#[test]
fn sha8_is_the_locate_convention_cut_to_eight() {
    // The trailing newline is not part of the hash (`text_sha` over the trimmed text).
    assert_eq!(sha8("pub fn helper() -> u32 {\n    1\n}\n"), "c6529027");
    assert_eq!(sha8("pub fn helper() -> u32 {\n    1\n}"), "c6529027");
}

#[test]
fn missing_and_unavailable_are_refused_with_the_exact_message() {
    let md = "[a](theuth://sources/01S1#7f2a) [b](theuth://notes/01N1#q) [c](theuth://sessions/01X#4) \
              [d](theuth://meetings/01M@41:12) [e](theuth://knowledge/ops/dr)";
    let l = parsed(md);
    let cites = vec![
        cite(&l[0].1, LocateStatus::Missing, true, None),
        cite(&l[1].1, LocateStatus::Same, false, None),
        cite(&l[2].1, LocateStatus::Missing, true, None),
        cite(&l[3].1, LocateStatus::Missing, true, None),
        cite(&l[4].1, LocateStatus::Missing, true, None),
    ];
    let near = HashMap::from([(
        "01S1".to_owned(),
        vec!["7f1e".to_owned(), "7f2b".to_owned()],
    )]);
    let err = judge(&l, &cites, &near).unwrap_err();
    assert_eq!(
        err,
        "- theuth://sources/01S1#7f2a: block not found; blocks near the end are 7f1e, 7f2b\n\
         - theuth://notes/01N1#q: note unavailable (trashed or not readable)\n\
         - theuth://sessions/01X#4: no such turn\n\
         - theuth://meetings/01M@41:12: no segment at that time (past the end)\n\
         - theuth://knowledge/ops/dr: not found\n"
    );
    // No nearby ids known → the short form.
    let err = judge(&l[..1], &cites[..1], &HashMap::new()).unwrap_err();
    assert_eq!(err, "- theuth://sources/01S1#7f2a: block not found\n");
}

#[test]
fn changed_is_accepted_and_sha_is_added_only_where_absent() {
    let md = "[a](theuth://sources/01S1#b1) and [b](theuth://notes/01N1#x?s=0123abcd) and \
              [c](theuth://sessions/01X#2) and [d](theuth://sources/01S1#b1)";
    let l = parsed(md);
    let cites = vec![
        cite(
            &l[0].1,
            LocateStatus::Changed,
            true,
            Some("the block now\n"),
        ),
        cite(&l[1].1, LocateStatus::Changed, true, Some("other text")),
        cite(&l[2].1, LocateStatus::Same, true, None),
    ];
    let rewrites = judge(&l, &cites, &HashMap::new()).unwrap();
    let want = format!(
        "]({}?s={})",
        "theuth://sources/01S1#b1",
        sha8("the block now")
    );
    assert_eq!(
        rewrites,
        vec![("](theuth://sources/01S1#b1)".to_owned(), want.clone())]
    );
    // Applied the way `check` does: every occurrence of that href, the `?s=` one untouched.
    let mut out = md.to_owned();
    for (from, to) in rewrites {
        out = out.replace(&from, &to);
    }
    assert_eq!(out.matches(&want).count(), 2);
    assert!(out.contains("theuth://notes/01N1#x?s=0123abcd)"));
    assert!(out.contains("](theuth://sessions/01X#2)"));
}
