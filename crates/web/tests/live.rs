//! Design doc 122 §6.3 live smoke for `read_url` (network). Ignored by
//! default; runs when `THEUTH_LIVE=1`:
//!
//! ```sh
//! THEUTH_LIVE=1 THEUTH_LIVE_IMPERSONATE=always cargo test -p web --test live -- --ignored --nocapture
//! THEUTH_LIVE=1 THEUTH_LIVE_IMPERSONATE=never  cargo test -p web --test live -- --ignored --nocapture
//! ```
//!
//! `THEUTH_LIVE_IMPERSONATE` (`always` | `on_challenge` | `never`, default
//! `always`) is the test-local switch for `web.impersonate`. Every row asserts
//! shape only (no panic, tier/charset/attempts present) and prints one line:
//! `url · client · tier · issue · chars · compacted · took_ms`. Sites that are
//! down or block both ways are recorded, not fixed.

use std::sync::Arc;
use std::time::Instant;

use kameo::actor::Spawn as _;
use tools::{Tool as _, ToolCx};
use web::WebActor;
use web::actor::ProviderMetas;
use web::fetch::Fetcher;
use web::fetch::client::Impersonate;
use web::tools::{FetchArgs, FetchTool, Source};

const URLS: &[&str] = &[
    "https://example.com",
    "https://docs.rs/reqwest/latest/reqwest/",
    "https://news.ycombinator.com/item?id=1",
    "https://en.wikipedia.org/wiki/Rust_(programming_language)",
    "https://httpbin.org/status/403",
    "https://httpbin.org/status/429",
    "https://www.aozora.gr.jp/",
    "https://nowsecure.nl",
    "https://www.g2.com/",
    "https://www.reddit.com/r/rust/comments/1g0x9zl/announcing_rust_1820/",
    "https://stackoverflow.com/questions/26960537/how-do-i-print-a-rust-string",
    "https://www.npmjs.com/package/left-pad",
    "https://pypi.org/project/requests/",
    "https://crates.io/crates/reqwest",
    "https://arxiv.org/abs/hep-th/9901001",
    "https://arxiv.org/abs/2106.09685",
];

fn live() -> bool {
    std::env::var("THEUTH_LIVE").is_ok_and(|v| v == "1")
}

fn impersonate() -> Impersonate {
    match std::env::var("THEUTH_LIVE_IMPERSONATE").as_deref() {
        Ok("never") => Impersonate::Never,
        Ok("on_challenge") => Impersonate::OnChallenge,
        _ => Impersonate::Always,
    }
}

fn tool() -> FetchTool {
    let cfg = web::WebConfig {
        impersonate: impersonate(),
        ..web::WebConfig::default()
    };
    let fetcher = Fetcher::new(&cfg.user_agent, cfg.max_fetch_bytes, cfg.max_redirects)
        .expect("client")
        .with_policy(
            cfg.impersonate,
            cfg.header_ladder,
            Some(&cfg.accept_language),
        );
    let actor = WebActor::spawn(WebActor::with_config(
        cfg.clone(),
        reqwest::Client::new(),
        vec![],
        ProviderMetas::default(),
    ));
    FetchTool {
        actor,
        fetcher: Arc::new(fetcher),
        providers: ProviderMetas::default(),
        cfg: Arc::new(std::sync::RwLock::new(cfg)),
    }
}

fn cx() -> ToolCx {
    ToolCx {
        session: runtime::SessionId::new(),
        call_id: "live".into(),
        quiet: true,
        resources: tools::SessionResources::durable_arc(&std::env::temp_dir()),
    }
}

fn args(url: &str, offset: Option<u32>) -> FetchArgs {
    FetchArgs {
        url: url.to_owned(),
        max_chars: Some(4000),
        format: None,
        rendered: None,
        offset,
        compact: None,
        source: Some(Source::Auto),
    }
}

/// `url · client · tier · issue · chars · compacted · took_ms` (plus
/// attempts/ladder/charset/next_offset for the as-built).
fn row(url: &str, out: &tools::ToolOutput, took: u128) -> String {
    let Some(d) = &out.structured else {
        return format!(
            "{url} · ERROR · - · {} · 0 · 0 · {took}",
            out.content.trim()
        );
    };
    let s = |k: &str| d[k].as_str().unwrap_or("-").to_owned();
    let issue = d["issue"].as_object().map_or_else(
        || "-".to_owned(),
        |i| {
            format!(
                "{} ({})",
                i["kind"].as_str().unwrap_or("?"),
                i["detail"].as_str().unwrap_or("")
            )
        },
    );
    format!(
        "{url} · {} · {} · {issue} · {} · {} · {took} · attempts={} ladder={} charset={} next_offset={} source_kind={}",
        s("client"),
        s("tier"),
        d["content"].as_str().map_or(0, |c| c.chars().count()),
        d["compacted_chars"].as_u64().unwrap_or(0),
        d["attempts"].as_u64().unwrap_or(0),
        d["ladder"].as_bool().unwrap_or(false),
        s("charset"),
        d["next_offset"],
        s("source_kind"),
    )
}

#[tokio::test]
#[ignore = "network; THEUTH_LIVE=1"]
async fn read_url_matrix() {
    if !live() {
        eprintln!("THEUTH_LIVE unset; skipping");
        return;
    }
    let tool = tool();
    println!("impersonate={:?}", impersonate());
    for url in URLS {
        let t = Instant::now();
        let out = tool.call(args(url, None), cx()).await.expect("tool call");
        println!("{}", row(url, &out, t.elapsed().as_millis()));
        if let Some(d) = &out.structured {
            assert!(
                matches!(d["tier"].as_str(), Some("fetch" | "render" | "api")),
                "{url}: {d}"
            );
            assert!(
                d["charset"].as_str().is_some_and(|c| !c.is_empty()),
                "{url}"
            );
            assert!(d["attempts"].as_u64().is_some_and(|a| a >= 1), "{url}");
            assert!(d["client"].as_str().is_some(), "{url}");
        }
    }
}

#[tokio::test]
#[ignore = "network; THEUTH_LIVE=1"]
async fn docs_page_chains_with_offset() {
    if !live() {
        return;
    }
    let tool = tool();
    let url = "https://docs.rs/reqwest/latest/reqwest/";
    let t = Instant::now();
    let first = tool.call(args(url, None), cx()).await.expect("tool call");
    println!("{}", row(url, &first, t.elapsed().as_millis()));
    let Some(d) = &first.structured else { return };
    let Some(next) = d["next_offset"].as_u64() else {
        println!("{url}: not cut at 4000 chars; nothing to chain");
        return;
    };
    assert!(
        d["content"]
            .as_str()
            .unwrap_or("")
            .contains(&format!("continue with offset={next}]")),
        "marker names next_offset"
    );
    println!("omitted_headings={}", d["omitted_headings"]);
    let t = Instant::now();
    let second = tool
        .call(args(url, Some(u32::try_from(next).unwrap())), cx())
        .await
        .expect("tool call");
    println!(
        "{}",
        row(
            &format!("{url} offset={next}"),
            &second,
            t.elapsed().as_millis()
        )
    );
    let s = second.structured.expect("structured");
    assert!(!s["content"].as_str().unwrap_or("").is_empty());
}
