//! Design doc 122 D7: each compaction pass in isolation, the fence exemption,
//! idempotence and the size targets on the two synthetic fixtures.

use web::compact::{CompactOptions, Compacted, compact};

const OPTS: CompactOptions = CompactOptions {
    fold_links: false,
    keep_images: true,
};

fn run(md: &str) -> Compacted {
    compact(md, &OPTS)
}

fn text(md: &str) -> String {
    run(md).text
}

fn fixture(name: &str) -> String {
    std::fs::read_to_string(format!(
        "{}/tests/fixtures/compact/{name}.md",
        env!("CARGO_MANIFEST_DIR")
    ))
    .unwrap()
}

#[test]
fn pass1_strips_invisible_chars_without_joining_words() {
    let out =
        run("soft\u{200B}ware ship\u{AD}ping \u{FEFF}start\u{200C}\u{200D} two\u{200B} words");
    assert_eq!(out.text, "software shipping start two words");
    assert_eq!(out.removed_chars, 6);
}

#[test]
fn pass1_decodes_leaked_entities() {
    assert_eq!(
        text("Tom &amp; Jerry, 1 &lt; 2 &gt; 0, &quot;q&quot; it&#39;s a&nbsp;b"),
        "Tom & Jerry, 1 < 2 > 0, \"q\" it's a b"
    );
}

#[test]
fn pass2_drops_utility_class_lines_only_past_half() {
    let md = "Intro\n\nflex items-center px-4 py-2 bg-white\n\ntext-heavy pages load slowly\n\nflex grid two three";
    assert_eq!(
        text(md),
        "Intro\n\ntext-heavy pages load slowly\n\nflex grid two three"
    );
}

#[test]
fn pass3_folds_image_runs_keeping_first_alt() {
    let md = "![](a.png)\n![Globex](b.png)\n![Initech](c.png)\n![Hooli](d.png)\n\ntext\n\n![solo](e.png)";
    assert_eq!(text(md), "[4 images: Globex]\n\ntext\n\n![solo](e.png)");
    let md = "[![](a.png)](https://x.test/)\n![](b.png)\n- ![](c.png)";
    assert_eq!(text(md), "[3 images]");
}

#[test]
fn pass3_drops_remaining_images_without_keep_images() {
    let opts = CompactOptions {
        fold_links: false,
        keep_images: false,
    };
    let md = "![one](a.png)\n\ntext ![inline](b.png)\n\n![a](1.png)\n![b](2.png)\n![c](3.png)";
    assert_eq!(
        compact(md, &opts).text,
        "text ![inline](b.png)\n\n[3 images: a]"
    );
}

#[test]
fn pass4_removes_a11y_chrome_and_emptied_lines() {
    let md = "[Skip to content](#main)\n\n- Skip to main content\n\nRead the [docs (opens in new tab)](https://d.test) now.\nPrivacy (Opens in a new window)";
    assert_eq!(text(md), "Read the [docs ](https://d.test) now.\nPrivacy");
}

#[test]
fn pass5_dedups_blocks_by_full_and_prefix_fingerprint() {
    let quote = "> Acme cut our widget lead time in half, says Jane.";
    let md = format!(
        "{quote}\n\nOther paragraph that is long enough to count.\n\n{}\n\nAcme cut our widget lead time in half, says Jane, repeatedly.\n\nshort\n\nshort",
        quote.to_uppercase()
    );
    // full fingerprint is case-insensitive; the prefix (10 words) catches the variant; `short` is < 20 chars.
    assert_eq!(
        text(&md),
        format!("{quote}\n\nOther paragraph that is long enough to count.\n\nshort\n\nshort")
    );
}

#[test]
fn pass6_dedups_lines_inside_one_block_only() {
    let md = "- Home\n- Docs\n- Home\n\n- Home";
    assert_eq!(text(md), "- Home\n- Docs\n\n- Home");
}

#[test]
fn pass7_dedups_comma_lists_of_four_or_more() {
    assert_eq!(text("a, b, a, c, B"), "a, b, c");
    assert_eq!(text("x, y, x"), "x, y, x");
}

#[test]
fn pass8_drops_empty_and_trailing_headings() {
    let md = "# T\n\n## A\n\n## B\n\ntext\n\n## C\n\n### D\n\n## E\n\n###";
    assert_eq!(text(md), "# T\n\n## B\n\ntext");
}

#[test]
fn pass9_collapses_blank_runs_and_trailing_whitespace() {
    let out = run("\n\na   \n\n\n\nb\t\n\n\n");
    assert_eq!(out.text, "a\n\nb\n");
    assert_eq!(out.removed_chars, 10);
}

#[test]
fn fenced_code_is_untouched_by_every_pass() {
    let fence = "```\nacme init\nacme init\nacme init\nflex grid hidden block\n![a](1)\n![b](2)\n![c](3)\nskip to content\n&amp; \u{200B}\n## H\n\n\n\n## H\n```";
    let md = format!("# Doc\n\n{fence}\n\ntail\n\n~~~\nx, x, x, x\n~~~");
    assert_eq!(text(&md), md);
}

#[test]
fn fold_links_lists_deduped_capped_links() {
    let mut md = String::from(
        "See [docs](https://d.test/a) and [again](https://d.test/a) or [top](#top) or [js](javascript:void(0)) ![img](i.png) [](https://bare.test)\n\n",
    );
    for i in 0..70 {
        md.push_str(&format!("[link {i}](https://many.test/{i}) "));
    }
    let out = compact(
        &md,
        &CompactOptions {
            fold_links: true,
            keep_images: true,
        },
    );
    let (body, links) = out.text.split_once("\n\nLinks:\n").unwrap();
    assert!(body.starts_with("See docs and again or top or js img\n\nlink 0 link 1 "));
    let links: Vec<&str> = links.lines().collect();
    assert_eq!(links.len(), 60);
    assert_eq!(links[0], "- docs: https://d.test/a");
    assert_eq!(links[1], "- https://bare.test: https://bare.test");
    assert_eq!(links[2], "- link 0: https://many.test/0");
    assert_eq!(links[59], "- link 57: https://many.test/57");
    assert!(!out.text.contains("#top") && !out.text.contains("javascript"));
}

#[test]
fn links_stay_inline_without_fold_links() {
    let md = "See [docs](https://d.test/a).";
    assert_eq!(text(md), md);
}

#[test]
fn chrome_heavy_page_loses_at_least_15_percent() {
    let input = fixture("chrome-heavy");
    let out = run(&input);
    let input_chars = input.chars().count();
    assert_eq!(out.removed_chars, input_chars - out.text.chars().count());
    assert!(
        out.removed_chars * 100 >= input_chars * 15,
        "removed {} of {input_chars}",
        out.removed_chars
    );
    // What each pass had to do on this page.
    assert_eq!(out.text.matches("Acme cut our widget lead time").count(), 1);
    assert!(out.text.contains("[12 images: Globex]"));
    assert!(!out.text.contains("px-4") && !out.text.contains("dark:bg-gray-800"));
    assert!(!out.text.contains("Skip to content") && !out.text.contains("opens in new tab"));
    assert_eq!(out.text.matches("[Home](https://acme.test/)").count(), 1);
    assert!(
        out.text
            .contains("Tags: widgets, pipelines, yaml, automation, shipping")
    );
    assert!(
        out.text.contains("software, shipping, and a BOM: start")
            && out.text.contains("Tom & Jerry")
    );
    assert!(
        out.text
            .contains("```bash\nacme init\nacme init\nacme init\nacme ship --all\n```")
    );
    assert!(!out.text.contains("### Company") && !out.text.contains("## Related"));
    assert!(out.text.contains("## Footer\n\n### Legal"));
    assert!(!out.text.contains("\n\n\n"));
}

#[test]
fn clean_article_is_unchanged() {
    let input = fixture("clean-article");
    let out = run(&input);
    assert_eq!(out.text, input);
    assert_eq!(out.removed_chars, 0);
}

#[test]
fn compaction_is_idempotent() {
    for name in ["chrome-heavy", "clean-article"] {
        let once = run(&fixture(name));
        let twice = run(&once.text);
        assert_eq!(twice.text, once.text, "{name}");
        assert_eq!(twice.removed_chars, 0, "{name}");
    }
    let folded = CompactOptions {
        fold_links: true,
        keep_images: false,
    };
    let once = compact(&fixture("chrome-heavy"), &folded);
    assert_eq!(compact(&once.text, &folded).text, once.text);
}

#[test]
fn headings_only_document_is_kept_rather_than_emptied() {
    // Pass 8 alone would drop both headings (a two-line PDF rendered by anydoc).
    let md = "## Page one\n\n## Page two\n";
    let out = run(md);
    assert_eq!(out.text, md);
    assert_eq!(out.removed_chars, 0);
}
