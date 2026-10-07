//! Design doc 122 D11: the eight-row table (positive and negative URLs) and
//! the renderers on saved, hand-written samples. No network.

use url::Url;
use web::sources::{ApiSource, api_source, follow_up, render};

fn src(page: &str) -> Option<ApiSource> {
    api_source(&Url::parse(page).unwrap())
}

fn expect(page: &str, kind: &str, fetch: &str) {
    let got = src(page).unwrap_or_else(|| panic!("{page} should match {kind}"));
    assert_eq!(got.kind, kind, "{page}");
    assert_eq!(got.fetch_url.as_str(), fetch, "{page}");
}

fn none(page: &str) {
    assert_eq!(src(page), None, "{page}");
}

fn fixture(name: &str) -> String {
    std::fs::read_to_string(format!(
        "{}/tests/fixtures/sources/{name}",
        env!("CARGO_MANIFEST_DIR")
    ))
    .unwrap()
}

#[test]
fn hn_item_table_row() {
    expect(
        "https://news.ycombinator.com/item?id=41234567",
        "hn-item",
        "https://hn.algolia.com/api/v1/items/41234567",
    );
    expect(
        "http://www.news.ycombinator.com/item?p=2&id=7",
        "hn-item",
        "https://hn.algolia.com/api/v1/items/7",
    );
    none("https://news.ycombinator.com/news");
    none("https://news.ycombinator.com/item?id=abc");
    none("https://news.ycombinator.com/item");
    none("https://news.ycombinator.com.evil.test/item?id=1");
}

#[test]
fn reddit_thread_table_row() {
    expect(
        "https://www.reddit.com/r/rust/comments/1abc2d/some_title/?utm=1",
        "reddit-old",
        "https://old.reddit.com/r/rust/comments/1abc2d/some_title/",
    );
    expect(
        "https://np.reddit.com/r/rust/comments/1abc2d",
        "reddit-old",
        "https://old.reddit.com/r/rust/comments/1abc2d",
    );
    none("https://www.reddit.com/r/rust");
    none("https://www.reddit.com/r/rust/comments/");
    none("https://www.reddit.com/user/someone/comments/1abc2d");
    none("https://notreddit.com/r/rust/comments/1abc2d");
    none("https://reddit.com.evil.test/r/rust/comments/1abc2d");
}

#[test]
fn npm_table_row() {
    expect(
        "https://www.npmjs.com/package/left-pad",
        "npm",
        "https://registry.npmjs.org/left-pad",
    );
    expect(
        "https://www.npmjs.com/package/@acme/widgets/v/2.3.1",
        "npm",
        "https://registry.npmjs.org/@acme/widgets",
    );
    none("https://www.npmjs.com/search?q=widgets");
    none("https://www.npmjs.com/package/");
    none("https://www.npmjs.com/package/@acme");
    none("https://npmjs.org/package/left-pad");
}

#[test]
fn pypi_table_row() {
    expect(
        "https://pypi.org/project/requests/",
        "pypi",
        "https://pypi.org/pypi/requests/json",
    );
    expect(
        "https://pypi.org/project/requests/2.31.0/",
        "pypi",
        "https://pypi.org/pypi/requests/json",
    );
    none("https://pypi.org/search/?q=requests");
    none("https://pypi.org/user/someone/");
    none("https://test.pypi.org/project/requests/");
}

#[test]
fn crates_io_table_row() {
    expect(
        "https://crates.io/crates/serde",
        "crates-io",
        "https://crates.io/api/v1/crates/serde",
    );
    expect(
        "https://crates.io/crates/serde/1.0.0",
        "crates-io",
        "https://crates.io/api/v1/crates/serde",
    );
    none("https://crates.io/search?q=serde");
    none("https://crates.io/users/dtolnay");
    none("https://docs.rs/serde");
}

#[test]
fn arxiv_table_row() {
    expect(
        "https://arxiv.org/abs/2401.00001v2",
        "arxiv",
        "https://export.arxiv.org/api/query?id_list=2401.00001v2",
    );
    expect(
        "https://arxiv.org/abs/hep-th/9901001",
        "arxiv",
        "https://export.arxiv.org/api/query?id_list=hep-th%2F9901001",
    );
    none("https://arxiv.org/pdf/2401.00001v2");
    none("https://arxiv.org/list/cs.IR/recent");
    none("https://arxiv.org/abs/");
}

#[test]
fn wikipedia_table_row() {
    expect(
        "https://en.wikipedia.org/wiki/Rust_(programming_language)#History",
        "wikipedia",
        "https://en.wikipedia.org/w/api.php?action=query&prop=extracts&explaintext=1&format=json&titles=Rust_%28programming_language%29",
    );
    expect(
        "https://de.wikipedia.org/wiki/Caf%C3%A9",
        "wikipedia",
        "https://de.wikipedia.org/w/api.php?action=query&prop=extracts&explaintext=1&format=json&titles=Caf%C3%A9",
    );
    // A colon inside an article title is not a namespace.
    expect(
        "https://en.wikipedia.org/wiki/Star_Wars:_Episode_IV",
        "wikipedia",
        "https://en.wikipedia.org/w/api.php?action=query&prop=extracts&explaintext=1&format=json&titles=Star_Wars%3A_Episode_IV",
    );
    none("https://en.wikipedia.org/wiki/Special:Search");
    none("https://en.wikipedia.org/wiki/Talk:Rust");
    none("https://en.wikipedia.org/wiki/User_talk:Someone");
    none("https://en.wikipedia.org/wiki/File:Rust_logo.svg");
    none("https://en.wikipedia.org/w/index.php?title=Rust");
    none("https://www.wikipedia.org/");
    none("https://en.m.wikipedia.org/wiki/Rust");
    none("https://en.wikipedia.org/wiki/Rust/extra");
}

#[test]
fn stackoverflow_table_row() {
    let fetch =
        "https://api.stackexchange.com/2.3/questions/77001?site=stackoverflow&filter=withbody";
    expect(
        "https://stackoverflow.com/questions/77001/why-does-this-borrow-not-live-long-enough",
        "stackoverflow",
        fetch,
    );
    expect("https://stackoverflow.com/q/77001", "stackoverflow", fetch);
    let source = src("https://stackoverflow.com/questions/77001").unwrap();
    assert_eq!(
        follow_up(&source).unwrap().as_str(),
        "https://api.stackexchange.com/2.3/questions/77001/answers?site=stackoverflow&filter=withbody&sort=votes"
    );
    assert_eq!(
        follow_up(&src("https://crates.io/crates/serde").unwrap()),
        None
    );
    none("https://stackoverflow.com/questions/tagged/rust");
    none("https://stackoverflow.com/users/42/asker");
    none("https://superuser.com/questions/77001/x");
}

#[test]
fn unrelated_hosts_match_nothing() {
    for page in [
        "https://example.com/item?id=1",
        "https://github.com/acme/widgets",
        "https://docs.rs/url/latest/url/",
        "file:///etc/passwd",
    ] {
        none(page);
    }
}

// ---- renderers --------------------------------------------------------------

#[test]
fn render_hn_item() {
    let md = render("hn-item", &fixture("hn_item.json")).unwrap();
    assert!(md.starts_with("# Show HN: A tiny Rust HTML compactor\n\nby throwaway_dev · 212 points · 2026-10-01T14:02:11Z\n\nhttps://example.test/compactor\n\n"));
    assert!(md.contains("Feedback welcome & appreciated."));
    assert!(md.contains("\n- **alice**: Does it keep code fences?\n  - **throwaway_dev**: Yes — fences pass through verbatim.\n    - **bob**: Even `~~~` fences?\n- **carol**: How does it compare to [the other one](https://example.test/other)?"));
    assert!(!md.contains("Depth four"), "depth 4 is cut");
    assert!(
        !md.contains("[deleted]"),
        "a comment without text is skipped"
    );
}

#[test]
fn render_hn_caps_comments_at_60() {
    let children: Vec<String> = (0..80)
        .map(|i| format!(r#"{{"author":"u{i}","text":"<p>comment {i}</p>","children":[]}}"#))
        .collect();
    let body = format!(
        r#"{{"title":"T","author":"a","points":1,"created_at":"2026-01-01T00:00:00Z","children":[{}]}}"#,
        children.join(",")
    );
    let md = render("hn-item", &body).unwrap();
    assert_eq!(md.matches("- **u").count(), 60);
    assert!(md.contains("comment 59") && !md.contains("comment 60"));
}

#[test]
fn render_npm() {
    let md = render("npm", &fixture("npm_package.json")).unwrap();
    assert_eq!(
        md,
        "# @acme/widgets 2.3.1\n\nBuild, test and ship widgets from one YAML file.\n\nlicense MIT · homepage https://acme.test/widgets · repository https://github.com/acme/widgets\n\n# @acme/widgets\n\nInstall with `npm i @acme/widgets`.\n\n## Usage\n\n```js\nimport { ship } from '@acme/widgets';\nawait ship('widgets.yaml');\n```"
    );
}

#[test]
fn render_npm_cuts_readme_head() {
    let readme = "x".repeat(5000);
    let body = format!(r#"{{"name":"big","dist-tags":{{"latest":"1.0.0"}},"readme":"{readme}"}}"#);
    let md = render("npm", &body).unwrap();
    assert!(md.starts_with("# big 1.0.0\n\nxxx"));
    assert!(md.ends_with("x…"));
    assert_eq!(md.chars().filter(|c| *c == 'x').count(), 3000);
}

#[test]
fn render_pypi() {
    let md = render("pypi", &fixture("pypi_project.json")).unwrap();
    assert!(md.starts_with("# widgets 1.4.2\n\nWidget pipelines for Python.\n\nrequires_python >=3.10 · license MIT · home_page https://acme.test/widgets-py\n\nWidgets\n======="));
    assert!(
        !md.contains("Permission is hereby granted"),
        "license text is not a licence"
    );
}

#[test]
fn render_crates_io() {
    let md = render("crates-io", &fixture("crates_io_crate.json")).unwrap();
    assert_eq!(
        md,
        "# widgets 0.8.3\n\nWidget pipelines for Rust.\n\ndownloads 1234567 · repository https://github.com/acme/widgets-rs · documentation https://docs.rs/widgets\n\nkeywords: widgets, pipeline, yaml"
    );
}

#[test]
fn render_arxiv() {
    let md = render("arxiv", &fixture("arxiv_abs.xml")).unwrap();
    assert_eq!(
        md,
        "# Compacting Web Pages for Language Models: A Synthetic Study\n\nAda Example, Grace Sample\n\npublished 2024-01-01T09:00:00Z\n\nWe study nine line-oriented passes that remove page chrome from markdown before a token cap & measure the savings on a synthetic corpus. Fenced code is left verbatim.\n\nPDF: http://arxiv.org/pdf/2401.00001v2"
    );
    // The search-shaped fixture still renders its first entry.
    let md = render("arxiv", &fixture("../arxiv_atom.xml")).unwrap();
    assert!(md.starts_with(
        "# Dilated Neighborhood Attention Transformer\n\npublished 2022-09-29T17:57:08Z\n\n"
    ));
    assert!(md.ends_with("PDF: https://arxiv.org/pdf/2209.15001v3"));
}

#[test]
fn render_wikipedia() {
    let md = render("wikipedia", &fixture("wikipedia_extract.json")).unwrap();
    assert!(md.starts_with(
        "# Rust (programming language)\n\nRust is a general-purpose programming language"
    ));
    assert!(md.contains("== History ==\nThe language grew out of a personal project"));
    assert_eq!(
        render("wikipedia", &fixture("wikipedia_missing.json")),
        None
    );
    let long = format!(
        r#"{{"query":{{"pages":{{"1":{{"title":"Long","extract":"{}"}}}}}}}}"#,
        "é".repeat(20_000)
    );
    let md = render("wikipedia", &long).unwrap();
    assert_eq!(md.chars().filter(|c| *c == 'é').count(), 12_000);
    assert!(md.ends_with('…'));
}

#[test]
fn render_stackoverflow_top_three_accepted_first() {
    let body = format!(
        "{}\n{}",
        fixture("stackoverflow_question.json"),
        fixture("stackoverflow_answers.json")
    );
    let md = render("stackoverflow", &body).unwrap();
    assert!(md.starts_with("# Why does this borrow \"not live long enough\"?\n\nscore 57 · asked by asker & co · tags: rust, lifetimes\n\nI get `E0597` on this snippet:\n\n```\nlet r;\n{\n    let x = 5;\n    r = &x;\n}\nprintln!(\"{r}\");\n```\n\nWhy?\n\n## Answers\n\n### ✓ accepted · score 80 · by borrowck\n\nThe reference outlives the value. Move `x` out of the block.\n\n### · score 120 · by lifetime_lou\n\n"), "{md}");
    assert!(md.contains("### · score 12 · by third"));
    assert!(!md.contains("Fourth answer") && !md.contains("Fifth answer"));
    // The question alone (no answers fetched yet) still renders.
    let md = render("stackoverflow", &fixture("stackoverflow_question.json")).unwrap();
    assert!(md.starts_with("# Why does this borrow") && !md.contains("## Answers"));
    // Both documents wrapped in one array work too.
    let wrapped = format!(
        "[{},{}]",
        fixture("stackoverflow_question.json"),
        fixture("stackoverflow_answers.json")
    );
    assert!(
        render("stackoverflow", &wrapped)
            .unwrap()
            .contains("### ✓ accepted · score 80")
    );
}

#[test]
fn render_rejects_garbage_and_unknown_kinds() {
    for kind in [
        "hn-item",
        "npm",
        "pypi",
        "crates-io",
        "arxiv",
        "wikipedia",
        "stackoverflow",
    ] {
        assert_eq!(render(kind, "<html>not json</html>"), None, "{kind}");
        assert_eq!(render(kind, "{}"), None, "{kind}");
    }
    assert_eq!(
        render("reddit-old", "<html><body>thread</body></html>"),
        None
    );
    assert_eq!(render("nope", "{}"), None);
}
