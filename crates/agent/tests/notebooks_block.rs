//! 144 §5.3: the `<notebook>` block — shape, budget ordering, truncation notices, the allow-list sentence —
//! and the two things a bound session changes in the system prompt (nothing, outside one).

use std::collections::HashMap;

use agent::notebook_context::{Level, Pins, estimate_of, plan};
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
    let got = render(
        "NB",
        "DR verification",
        "Verify the DR plan.",
        items,
        &Pins::new(),
        b,
    );
    let want = "<notebook id=\"NB\" title=\"DR verification\">\n\
<description>Verify the DR plan.</description>\n\
<item kind=\"source\" id=\"S1\" title=\"Runbook v7\" origin=\"file · 2 pages\" level=\"full\" status=\"available\">\n\
<!-- r1 -->\n## Steps\n\n<!-- r2 -->\nStep one</item>\n\
<item kind=\"note\" id=\"N1\" title=\"Drill &quot;timeline&quot;\" level=\"full\" status=\"available\">\n\
<!-- a -->\n# Drill timeline\n\n<!-- b -->\nT+0 failover</item>\n\
<item kind=\"session\" id=\"01SESSION\" title=\"Earlier chat\" level=\"mention\" status=\"available\">\n\
mention only: read with session_read(\"01SESSION\")</item>\n\
You may cite only these ids: S1, N1, 01SESSION (and the blocks inside them).\n\
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
    let got = render("NB", "t", "", items, &Pins::new(), b);
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
    let got = render(
        "NB",
        "t",
        "0123456789abcdefghij",
        Vec::new(),
        &Pins::new(),
        b,
    );
    assert!(got.contains(
        "<description>0123456789abcdef\n[truncated: 4 of 5 tokens shown; the rest with doc_read(\"notebook:NB\")]</description>"
    ));
    assert!(got.contains("nothing in context — the agent sees only the description"));
}

/// 144 §5.1–5.3 (E1): pinned levels, the default `summary` for a source with insights, `off` items
/// absent from the block and the allow-list, order full → summary → mention, and the estimate's numbers
/// equal to what the block injects (one planner).
#[test]
fn dial_levels_render_in_order_off_is_not_citable_and_the_estimate_matches() {
    let insights = "<!-- s1 -->\n# Runbook\n\n<!-- s2 -->\nStep one, step two.\n\n<!-- s3 -->\n## Insights\n\n<!-- s4 -->\n### Dense summary · m · 2026-10-09\n\n<!-- s5 -->\nTwo steps.";
    let items = vec![
        item("source", "WITH", "With insights", Some(insights), 40),
        item(
            "source",
            "RAW",
            "Raw source",
            Some("<!-- r1 -->\n# Raw\n\n<!-- r2 -->\nBody"),
            30,
        ),
        item(
            "note",
            "NOTE",
            "A note",
            Some("<!-- n1 -->\n# A note\n\n<!-- n2 -->\nText"),
            20,
        ),
        item("note", "OFF", "Hidden", Some("<!-- o1 -->\nsecret"), 50),
        item("session", "SESS", "Chat", None, 10),
    ];
    let mut pins = Pins::new();
    pins.insert("OFF".into(), Level::Off);
    pins.insert("NOTE".into(), Level::Mention);
    pins.insert("SESS".into(), Level::Full); // clamped to mention
    let b = Budget {
        context_budget_tokens: 40_000,
        description_tokens: 2_000,
    };
    let got = render("NB", "t", "d", items.clone(), &pins, b);
    let want = "<notebook id=\"NB\" title=\"t\">\n\
<description>d</description>\n\
<item kind=\"source\" id=\"RAW\" title=\"Raw source\" origin=\"file · 2 pages\" level=\"full\" status=\"available\">\n\
<!-- r1 -->\n# Raw\n\n<!-- r2 -->\nBody</item>\n\
<item kind=\"source\" id=\"WITH\" title=\"With insights\" origin=\"file · 2 pages\" level=\"summary\" status=\"available\">\n\
mention only: 17 words; read with doc_read(\"source:WITH\") if needed\n\
<!-- s3 -->\n## Insights\n\n<!-- s4 -->\n### Dense summary · m · 2026-10-09\n\n<!-- s5 -->\nTwo steps.</item>\n\
<item kind=\"note\" id=\"NOTE\" title=\"A note\" level=\"mention\" status=\"available\">\n\
mention only: 4 words; read with doc_read(\"NOTE\") if needed</item>\n\
<item kind=\"session\" id=\"SESS\" title=\"Chat\" level=\"mention\" status=\"available\">\n\
mention only: read with session_read(\"SESS\")</item>\n\
You may cite only these ids: RAW, WITH, NOTE, SESS (and the blocks inside them).\n\
</notebook>";
    assert_eq!(got, want);

    // The same planner feeds the estimate: tokens per item are the injected text's chars / 4.
    let placed = plan(&items, &pins, 40_000);
    let est = estimate_of(&placed, 40_000);
    let by_id: HashMap<&str, _> = est.items.iter().map(|i| (i.id.as_str(), i)).collect();
    assert_eq!(est.items.len(), 5);
    assert_eq!(
        (est.omitted, est.empty, est.budget_tokens),
        (1, false, 40_000)
    );
    assert_eq!(by_id["OFF"].tokens, 0);
    assert!(by_id["OFF"].pinned && by_id["NOTE"].pinned && by_id["SESS"].pinned);
    assert!(!by_id["RAW"].pinned && !by_id["WITH"].pinned);
    assert_eq!(
        by_id["RAW"].tokens,
        agent::notebooks::tokens("<!-- r1 -->\n# Raw\n\n<!-- r2 -->\nBody")
    );
    assert_eq!(
        est.total_tokens,
        placed
            .iter()
            .map(|p| agent::notebooks::tokens(&p.text))
            .sum::<u32>()
    );
    // Every injected id of the block is in the estimate as not-off, and OFF is nowhere in the block.
    for id in ["RAW", "WITH", "NOTE", "SESS"] {
        assert!(got.contains(&format!("id=\"{id}\"")));
        assert_ne!(by_id[id].level, Level::Off);
    }
    assert!(!got.contains("OFF") && !got.contains("secret"));

    // Every item off: the sentence, no allow-list.
    let all_off: Pins = items.iter().map(|i| (i.id.clone(), Level::Off)).collect();
    let got = render("NB", "t", "d", items.clone(), &all_off, b);
    assert!(got.contains("nothing in context — the agent sees only the description"));
    assert!(!got.contains("You may cite"));
    assert!(estimate_of(&plan(&items, &all_off, 40_000), 40_000).empty);
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
