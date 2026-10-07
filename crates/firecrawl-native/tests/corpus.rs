//! Fixture corpus (design doc 122 D13): every `tests/fixtures/<name>.html` is
//! run through `transform_html` → html-to-markdown → `post_process_markdown`
//! (the same calls `scrape`'s engine makes) and checked against
//! `<name>.expected.md` (golden; `UPDATE_GOLDEN=1` rewrites it) and the
//! `must_contain` / `must_not_contain` / `min_words` / `method` keys of
//! `<name>.expect.json`; `tier_hint: "island"` asserts that `extract_islands`
//! yields ≥ 800 chars (and every other hint that it does not). `issue` is
//! asserted by `web`'s `issue.rs` reading the same HTML files.

use std::fs;
use std::path::{Path, PathBuf};

use firecrawl_native::{
    Method, TransformHtmlOptions, extract_islands, post_process_markdown,
    transform_html_with_method,
};
use html_to_markdown_rs::{ConversionOptions, PreprocessingOptions, TierStrategy, convert};

fn fixtures_dir() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures")
}

/// `scrape/src/engine.rs::html_to_markdown` with `strip_chrome` on.
fn html_to_markdown(html: &str) -> String {
    let options = ConversionOptions {
        extract_metadata: false,
        skip_images: false,
        exclude_selectors: [
            "script", "style", "noscript", "template", "svg", "iframe", "canvas", "object", "embed",
        ]
        .iter()
        .map(|s| (*s).to_owned())
        .collect(),
        preprocessing: PreprocessingOptions {
            remove_navigation: true,
            remove_forms: true,
            ..PreprocessingOptions::default()
        },
        tier_strategy: TierStrategy::Tier2,
        ..ConversionOptions::default()
    };
    convert(html, options)
        .expect("conversion")
        .content
        .unwrap_or_default()
}

/// The engine's `to_markdown` with `only_main_content: true`, plus the selection method.
fn pipeline(html: &str, name: &str) -> (String, Method) {
    let (transformed, method) = transform_html_with_method(TransformHtmlOptions {
        html: html.to_owned(),
        url: format!("https://{name}.example.test/"),
        include_tags: Vec::new(),
        exclude_tags: Vec::new(),
        only_main_content: true,
        omce_signatures: None,
    })
    .expect("transform_html");
    (
        post_process_markdown(html_to_markdown(&transformed)),
        method.expect("only_main_content yields a method"),
    )
}

/// The island rung's threshold (122 D9: `prose_chars(text) ≥ 800`), approximated as chars.
const ISLAND_MIN_CHARS: usize = 800;

#[derive(Default, serde::Deserialize)]
#[serde(default)]
struct Expect {
    tier_hint: Option<String>,
    method: Option<String>,
    min_words: usize,
    must_contain: Vec<String>,
    must_not_contain: Vec<String>,
}

#[test]
fn corpus_goldens_and_expectations() {
    let dir = fixtures_dir();
    let mut html_files: Vec<PathBuf> = fs::read_dir(&dir)
        .expect("fixtures dir")
        .map(|e| e.expect("entry").path())
        .filter(|p| p.extension().is_some_and(|x| x == "html"))
        .collect();
    html_files.sort();
    assert!(
        !html_files.is_empty(),
        "no fixtures under {}",
        dir.display()
    );
    let update = std::env::var_os("UPDATE_GOLDEN").is_some();
    let mut failures = Vec::new();

    for html_path in html_files {
        let name = html_path
            .file_stem()
            .unwrap()
            .to_string_lossy()
            .into_owned();
        let html = fs::read_to_string(&html_path).expect("read html");
        let (md, method) = pipeline(&html, &name);

        let golden = dir.join(format!("{name}.expected.md"));
        if update {
            fs::write(&golden, &md).expect("write golden");
        } else if golden.exists() {
            let want = fs::read_to_string(&golden).expect("read golden");
            if want != md {
                failures.push(format!(
                    "{name}: markdown differs from {} (rerun with UPDATE_GOLDEN=1 to accept)\n--- got ---\n{md}\n--- want ---\n{want}",
                    golden.display()
                ));
            }
        }

        let expect_path = dir.join(format!("{name}.expect.json"));
        if expect_path.exists() {
            let expect: Expect =
                serde_json::from_str(&fs::read_to_string(&expect_path).expect("read expect"))
                    .unwrap_or_else(|e| panic!("{}: {e}", expect_path.display()));
            let words = md.split_whitespace().count();
            if words < expect.min_words {
                failures.push(format!(
                    "{name}: {words} words < min_words {}",
                    expect.min_words
                ));
            }
            for s in &expect.must_contain {
                if !md.contains(s.as_str()) {
                    failures.push(format!("{name}: must_contain {s:?} missing"));
                }
            }
            for s in &expect.must_not_contain {
                if md.contains(s.as_str()) {
                    failures.push(format!("{name}: must_not_contain {s:?} present"));
                }
            }
            if let Some(want) = &expect.method {
                let got = serde_json::to_value(method).expect("method json");
                if got.as_str() != Some(want.as_str()) {
                    failures.push(format!("{name}: method {got} != expected {want:?}"));
                }
            }
            let island_chars = extract_islands(&html).map_or(0, |t| t.chars().count());
            let island = island_chars >= ISLAND_MIN_CHARS;
            let want_island = expect.tier_hint.as_deref() == Some("island");
            if island != want_island {
                failures.push(format!(
                    "{name}: extract_islands yields {island_chars} chars; tier_hint {:?}",
                    expect.tier_hint
                ));
            }
        }
    }

    assert!(failures.is_empty(), "{}", failures.join("\n\n"));
}
