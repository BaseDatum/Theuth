//! 144 §5.3: the `<notebook>` block — shape, budget ordering, truncation notices, the allow-list sentence —
//! and the two things a bound session changes in the system prompt (nothing, outside one).

use agent::notebooks::{Budget, Item, PROMPT, fallback_title, prompt_blocks, provenance, render};
use runtime::SessionId;

fn item(kind: &str, id: &str, title: &str, body: Option<&str>, added_ms: i64) -> Item {
    Item {
        kind: kind.into(),
        id: id.into(),
        title: title.into(),
        origin: if kind == "source" {
            "file · 2 pages".into()
        } else {
            String::new()
        },
        body: body.map(str::to_owned),
        words: body.map_or(0, |b| {
            b.lines()
                .filter(|l| !l.starts_with("<!-- "))
                .map(|l| l.split_whitespace().count() as u32)
                .sum()
        }),
        added_ms,
    }
}

#[test]
fn plain_session_prompt_is_byte_identical_and_bound_one_gains_the_contract() {
    let plain = "**Attachments.** …\n\n**Documents.** …".to_owned();
    assert_eq!(prompt_blocks(plain.clone(), false), plain);
    let bound = prompt_blocks(plain.clone(), true);
    assert!(bound.starts_with(&plain));
    assert!(bound.ends_with(PROMPT));
    assert!(PROMPT.contains("Never invent an id"));
    assert!(PROMPT.contains("Call things by title in prose, never by id"));
}

#[test]
fn block_shape_order_and_allow_list() {
    let items = vec![
        item(
            "note",
            "N1",
            "Drill \"timeline\"",
            Some("<!-- a -->\n# Drill timeline\n\n<!-- b -->\nT+0 failover"),
            10,
        ),
        item(
            "source",
            "S1",
            "Runbook v7",
            Some("<!-- r1 -->\n## Steps\n\n<!-- r2 -->\nStep one"),
            30,
        ),
        item("session", "01SESSION", "Earlier chat", None, 20),
    ];
    let b = Budget {
        context_budget_tokens: 40_000,
        description_tokens: 2_000,
    };
    let got = render("NB", "DR verification", "Verify the DR plan.", items, b);
    let want = "<notebook id=\"NB\" title=\"DR verification\">\n\
<description>Verify the DR plan.</description>\n\
<item kind=\"source\" id=\"S1\" title=\"Runbook v7\" origin=\"file · 2 pages\" level=\"full\" status=\"available\">\n\
<!-- r1 -->\n## Steps\n\n<!-- r2 -->\nStep one</item>\n\
<item kind=\"note\" id=\"N1\" title=\"Drill &quot;timeline&quot;\" level=\"full\" status=\"available\">\n\
<!-- a -->\n# Drill timeline\n\n<!-- b -->\nT+0 failover</item>\n\
<item kind=\"session\" id=\"01SESSION\" title=\"Earlier chat\" level=\"mention\" status=\"available\">\n\
mention only: read with session_read(\"01SESSION\")</item>\n\
You may cite only these ids: S1, 01SESSION, N1 (and the blocks inside them).\n\
</notebook>";
    assert_eq!(got, want);
}

#[test]
fn budget_truncates_the_first_overflow_at_a_block_and_mentions_the_rest() {
    // Three 40-char blocks = 10 tokens each; the budget (32) holds the newest note whole and 2 of the 3
    // blocks of the next, so the oldest falls to `mention`/`omitted`.
    let blk = |id: &str| format!("<!-- {id} -->\n{}\n", "x".repeat(27));
    let newest = blk("n1");
    let middle = format!("{}{}{}", blk("m1"), blk("m2"), blk("m3"));
    let oldest = blk("o1");
    let items = vec![
        item("note", "OLD", "Oldest", Some(&oldest), 1),
        item("note", "MID", "Middle", Some(&middle), 2),
        item("note", "NEW", "Newest", Some(&newest), 3),
    ];
    let b = Budget {
        context_budget_tokens: 32,
        description_tokens: 2_000,
    };
    let got = render("NB", "t", "", items, b);
    let order: Vec<usize> = ["id=\"NEW\"", "id=\"MID\"", "id=\"OLD\""]
        .iter()
        .map(|k| got.find(k).unwrap())
        .collect();
    assert!(
        order[0] < order[1] && order[1] < order[2],
        "full before mention, newest first"
    );
    assert!(got.contains("id=\"NEW\" title=\"Newest\" level=\"full\" status=\"available\""));
    assert!(got.contains("id=\"MID\" title=\"Middle\" level=\"full\" status=\"truncated\""));
    assert!(got.contains("<!-- m2 -->"), "two blocks kept");
    assert!(!got.contains("<!-- m3 -->"), "the third is cut");
    assert!(
        got.contains("[truncated: 20 of 30 tokens shown; the rest with doc_read(blocks=[…]); first omitted block m3]"),
        "{got}"
    );
    assert!(got.contains("id=\"OLD\" title=\"Oldest\" level=\"mention\" status=\"omitted\">\nmention only: 1 words; read with doc_read(\"OLD\") if needed</item>"));
    assert!(got.ends_with(
        "You may cite only these ids: NEW, MID, OLD (and the blocks inside them).\n</notebook>"
    ));
}

#[test]
fn description_is_capped_with_a_notice_and_an_empty_notebook_says_so() {
    let b = Budget {
        context_budget_tokens: 40_000,
        description_tokens: 4,
    };
    let got = render("NB", "t", "0123456789abcdefghij", Vec::new(), b);
    assert!(got.contains(
        "<description>0123456789abcdef\n[truncated: 4 of 5 tokens shown; the rest with doc_read(\"notebook:NB\")]</description>"
    ));
    assert!(
        got.contains(
            "Nothing in context — you see only the description; there are no ids to cite."
        )
    );
}

#[test]
fn provenance_line_and_fallback_title() {
    let s: SessionId = "01926f3e-0000-7000-8000-000000000001".parse().unwrap();
    let md = provenance(
        "The answer [§4](theuth://sources/S1#b1).",
        s,
        7,
        "anthropic/claude",
        "2026-10-08",
    );
    assert_eq!(
        md,
        "The answer [§4](theuth://sources/S1#b1).\n\n*Answer from [chat](theuth://sessions/01926f3e-0000-7000-8000-000000000001#7) · anthropic/claude · 2026-10-08*"
    );
    assert_eq!(
        fallback_title(
            "## The failover drill passed with two open items on DNS TTL and the runbook"
        ),
        "The failover drill passed with two open items on DNS TTL and"
    );
    assert_eq!(fallback_title("\n\n"), "Answer");
}
