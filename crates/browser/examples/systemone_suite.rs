//! 57 §4.1 item 8: cklxx/laya-browser's 16-task real-browser suite (`code/apps/browser_suite.py`
//! on the hub — name, start URL, goal and outcome check copied from there, read 2026-09-21)
//! driven by `browser::systemone` in **active** mode: pass/fail per run, steps, per-step
//! latency, escalations (steps the gate refused), plus a step-latency histogram. Their v10
//! scored 50 % (8/16 tasks 3/3 or better); the target here is ≥ that and ≤ 300 ms per step.
//!
//! `cargo run --release -p browser --example systemone_suite -- [--runs N] [--filter SUBSTR]
//! [--browser-model DIR]`. Needs the network and the v10 checkpoint
//! (`scripts/fetch-laya-browser.sh`); tasks the DOM reader cannot do by design are still run.

use std::path::PathBuf;
use std::sync::Arc;
use std::time::{Duration, Instant};

use browser::systemone::{Op, Run, Stop};
use browser::tools::Ctx;
use browser::{BrowserConfig, Browsers};
use decision::log::Log;
use decision::{DecisionConfig, Engine, Mode};
use kameo::actor::Spawn;
use runtime::SessionId;
use serde_json::json;

/// `(name, url, goal)`; the check is [`passed`].
const TASKS: [(&str, &str, &str); 16] = [
    (
        "wiki-einstein",
        "https://en.wikipedia.org/wiki/Main_Page",
        "Open the Wikipedia article about Albert Einstein.",
    ),
    (
        "wiki-search",
        "https://en.wikipedia.org/wiki/Main_Page",
        "Search Wikipedia for 'Python programming language' and open the article about the Python language.",
    ),
    (
        "hn-new",
        "https://news.ycombinator.com/",
        "Open the 'new' page that lists the newest submissions.",
    ),
    (
        "hn-login-page",
        "https://news.ycombinator.com/",
        "Go to the login page.",
    ),
    (
        "gh-issues",
        "https://github.com/tile-ai/tilelang",
        "Open the Issues tab of this repository.",
    ),
    (
        "py-downloads",
        "https://www.python.org/",
        "Go to the Downloads page.",
    ),
    (
        "books-travel",
        "https://books.toscrape.com/",
        "Open the 'Travel' category.",
    ),
    (
        "books-open-book",
        "https://books.toscrape.com/",
        "Open the product page of the book 'A Light in the Attic'.",
    ),
    (
        "internet-dropdown",
        "https://the-internet.herokuapp.com/dropdown",
        "Select 'Option 2' in the dropdown.",
    ),
    (
        "internet-checkbox",
        "https://the-internet.herokuapp.com/checkboxes",
        "Tick the first checkbox.",
    ),
    (
        "books-page2",
        "https://books.toscrape.com/",
        "Go to page 2 of the catalogue.",
    ),
    (
        "quotes-tag-love",
        "https://quotes.toscrape.com/",
        "Show the quotes tagged 'love'.",
    ),
    (
        "hn-past",
        "https://news.ycombinator.com/",
        "Open the 'past' page (front pages from previous days).",
    ),
    (
        "ddg-search",
        "https://duckduckgo.com/",
        "Search for 'tilelang github' and show the results.",
    ),
    (
        "arxiv-search",
        "https://arxiv.org/",
        "Search arXiv for 'flash attention' papers and show the results list.",
    ),
    (
        "flights",
        "https://www.google.com/travel/flights?hl=en",
        "Find one-way flights from Zurich to London on September 28, 2026, for one adult in economy. Stop when matching flight options are visible.",
    ),
];

/// Their `check(url, title, text)` per task; the two state checks read the page like theirs.
fn passed(name: &str, url: &str, title: &str, text: &str, state: &str) -> bool {
    let u = url;
    match name {
        "wiki-einstein" => u.contains("Albert_Einstein"),
        "wiki-search" => title.contains("Python"),
        "hn-new" => u.trim_end_matches('/').ends_with("/newest"),
        "hn-login-page" => u.contains("login"),
        "gh-issues" => u.contains("/issues"),
        "py-downloads" => u.contains("/downloads"),
        "books-travel" => u.contains("travel"),
        "books-open-book" => u.contains("a-light-in-the-attic"),
        "internet-dropdown" => state == "2" || state == "Option 2",
        "internet-checkbox" => state == "true",
        "books-page2" => u.contains("page-2"),
        "quotes-tag-love" => u.contains("/tag/love"),
        "hn-past" => u.contains("/front"),
        "ddg-search" => u.contains("q=") && u.to_lowercase().contains("tilelang"),
        "arxiv-search" => u.contains("search") && u.to_lowercase().contains("flash"),
        "flights" => {
            let lower = text.to_lowercase();
            (text.contains("ZRH") || text.contains("Zurich") || text.contains("Zürich"))
                && text.contains("London")
                && lower.contains("one way")
                && text
                    .split(|c: char| !c.is_ascii_digit() && c != ':')
                    .any(|w| {
                        w.split_once(':')
                            .is_some_and(|(h, m)| (1..=2).contains(&h.len()) && m.len() == 2)
                    })
        }
        _ => false,
    }
}

/// The page-state probe for the two form tasks (jev read `current_value` / `checked` from its
/// actions; we read the DOM).
const STATE_JS: &str = r#"(() => { const s=document.querySelector('select'); if (s) return String(s.value); const c=document.querySelector('input[type=checkbox]'); if (c) return String(!!c.checked); return ''; })()"#;

fn pct(sorted: &[u64], p: f64) -> u64 {
    if sorted.is_empty() {
        return 0;
    }
    sorted[((sorted.len() - 1) as f64 * p).round() as usize]
}

struct Row {
    name: &'static str,
    passes: Vec<bool>,
    steps: Vec<usize>,
    stops: Vec<Stop>,
    decide_ms: Vec<u64>,
    escalations: usize,
}

#[tokio::main(flavor = "multi_thread")]
async fn main() -> anyhow::Result<()> {
    let mut args = std::env::args().skip(1);
    let (mut runs, mut filter, mut browser_dir) = (3usize, String::new(), None::<PathBuf>);
    while let Some(a) = args.next() {
        match a.as_str() {
            "--runs" => runs = args.next().and_then(|v| v.parse().ok()).unwrap_or(3),
            "--filter" => filter = args.next().unwrap_or_default(),
            "--browser-model" => browser_dir = args.next().map(PathBuf::from),
            other => anyhow::bail!("unknown flag {other}"),
        }
    }
    let data_dir = std::env::var_os("THEUTH_DATA_DIR")
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from(std::env::var_os("HOME").expect("HOME")).join(".theuth"));
    let browser_dir = browser_dir.unwrap_or_else(|| data_dir.join("models/decision-browser"));
    if !decision::engine::model_files(decision::Kind::Laya)
        .iter()
        .all(|f| browser_dir.join(f).is_file())
    {
        anyhow::bail!(
            "no laya-browser checkpoint in {} (scripts/fetch-laya-browser.sh)",
            browser_dir.display()
        );
    }
    let _ = tracing_subscriber::fmt()
        .with_env_filter(
            tracing_subscriber::EnvFilter::try_from_default_env().unwrap_or_else(|_| "warn".into()),
        )
        .try_init();

    // The decision engines: default (unused here) + browser, browser families active.
    let mut dcfg = DecisionConfig::default();
    dcfg.checkpoints
        .insert("browser".into(), browser_dir.clone().into());
    for g in dcfg.families.values_mut() {
        g.mode = Mode::Active;
    }
    let scratch =
        std::env::temp_dir().join(format!("theuth-systemone-suite-{}", std::process::id()));
    std::fs::create_dir_all(&scratch)?;
    let log = Log::new(scratch.join("decisions"));
    let default_dir = dcfg.model_dir_in(&data_dir);
    Engine::spawn(Engine::new(dcfg.clone(), default_dir.clone(), log.clone()))
        .register(decision::DECISION_ACTOR)?;
    Engine::spawn(Engine::new(dcfg, default_dir, log).with_checkpoint("browser", browser_dir))
        .register(decision::actor_name("browser"))?;

    // The browser.
    let bcfg = BrowserConfig::default();
    browser::warmup(&bcfg);
    let rt = runtime::Runtime::new(figment::Figment::new(), scratch.clone());
    let actor = Browsers::spawn(Browsers::new(bcfg.clone()));
    let ctx = Arc::new(Ctx {
        actor,
        cfg: bcfg,
        rt,
        pending: Default::default(),
    });

    let mut rows = Vec::new();
    let mut all_steps: Vec<u64> = Vec::new();
    let suite_t = Instant::now();
    for (name, url, goal) in TASKS {
        if !filter.is_empty() && !name.contains(&filter) {
            continue;
        }
        let mut row = Row {
            name,
            passes: Vec::new(),
            steps: Vec::new(),
            stops: Vec::new(),
            decide_ms: Vec::new(),
            escalations: 0,
        };
        for r in 0..runs {
            let session = SessionId::new();
            let open = ctx
                .call(session, "browser_navigate", json!({ "url": url }))
                .await?;
            if open.is_error {
                println!(
                    "{name} run {}: open failed: {}",
                    r + 1,
                    open.content.lines().next().unwrap_or("")
                );
                row.passes.push(false);
                row.steps.push(0);
                row.stops.push(Stop::Error("open".into()));
                let _ = ctx.actor.tell(browser::Close { session }).await;
                continue;
            }
            let run: Run = ctx.run(session, goal, 20).await;
            // Their harness lets a slow navigation land before judging.
            tokio::time::sleep(Duration::from_millis(1500)).await;
            let (page, _) = ctx
                .observe(session, Default::default(), &[])
                .await
                .unwrap_or_default();
            let state = ctx
                .call(
                    session,
                    "browser_evaluate",
                    json!({ "expression": STATE_JS }),
                )
                .await
                .map(|o| o.content.trim().trim_matches('"').to_owned())
                .unwrap_or_default();
            let ok = passed(name, &page.url, &page.title, &page.text, &state);
            let executed = run
                .steps
                .iter()
                .filter(|s| !matches!(s.op, Op::Done | Op::Blocked))
                .count();
            if matches!(run.stop, Stop::BelowGate(_)) {
                row.escalations += 1;
            }
            for s in &run.steps {
                row.decide_ms.push(s.decide_ms);
                all_steps.push(s.decide_ms);
            }
            println!(
                "{name} run {}: {} · {} step(s) · {} · {} — {}",
                r + 1,
                if ok { "PASS" } else { "FAIL" },
                executed,
                run.stop,
                page.url,
                run.steps
                    .iter()
                    .map(|s| s.to_string())
                    .collect::<Vec<_>>()
                    .join(" | ")
            );
            row.passes.push(ok);
            row.steps.push(executed);
            row.stops.push(run.stop);
            let _ = ctx.actor.tell(browser::Close { session }).await;
        }
        rows.push(row);
    }

    println!("\n| task | runs (pass/fail) | steps | p50 step ms | escalations | stops |");
    println!("|---|---|---|---|---|---|");
    let mut tasks_pass = 0;
    let mut runs_pass = 0;
    let mut runs_total = 0;
    for r in &rows {
        let mut d = r.decide_ms.clone();
        d.sort_unstable();
        let marks: String = r
            .passes
            .iter()
            .map(|p| if *p { '✓' } else { '✗' })
            .collect();
        let n_pass = r.passes.iter().filter(|p| **p).count();
        runs_pass += n_pass;
        runs_total += r.passes.len();
        if n_pass == r.passes.len() && !r.passes.is_empty() {
            tasks_pass += 1;
        }
        println!(
            "| {} | {marks} ({n_pass}/{}) | {} | {} | {} | {} |",
            r.name,
            r.passes.len(),
            r.steps
                .iter()
                .map(|s| s.to_string())
                .collect::<Vec<_>>()
                .join(" / "),
            pct(&d, 0.5),
            r.escalations,
            r.stops
                .iter()
                .map(|s| format!("{s:?}")
                    .split('(')
                    .next()
                    .unwrap_or("")
                    .to_lowercase())
                .collect::<Vec<_>>()
                .join(" / ")
        );
    }
    all_steps.sort_unstable();
    println!(
        "\ntasks passing every run: {tasks_pass}/{} ({:.0} %); runs passing: {runs_pass}/{runs_total} ({:.0} %); their v10: 50 % of tasks (10 pass 3/3 of 16 incl. checks they later corrected)",
        rows.len(),
        100.0 * tasks_pass as f64 / rows.len().max(1) as f64,
        100.0 * runs_pass as f64 / runs_total.max(1) as f64
    );
    println!(
        "step latency (oracle, {} steps): p50 {} ms  p95 {} ms  max {} ms  ({} the §4.1 ≤ 300 ms target); suite wall {:.0} s",
        all_steps.len(),
        pct(&all_steps, 0.5),
        pct(&all_steps, 0.95),
        all_steps.last().copied().unwrap_or(0),
        if pct(&all_steps, 0.95) <= 300 {
            "within"
        } else {
            "OVER"
        },
        suite_t.elapsed().as_secs_f64()
    );
    println!("\nstep latency histogram (ms):");
    for (lo, hi) in [
        (0, 100),
        (100, 200),
        (200, 300),
        (300, 500),
        (500, 800),
        (800, 1200),
        (1200, 2000),
        (2000, u64::MAX),
    ] {
        let n = all_steps.iter().filter(|&&d| d >= lo && d < hi).count();
        if hi == u64::MAX {
            println!("  {lo:>5}+      {:<40} {n}", "#".repeat(n.min(40)));
        } else {
            println!("  {lo:>5}–{hi:<5} {:<40} {n}", "#".repeat(n.min(40)));
        }
    }
    let _ = std::fs::remove_dir_all(&scratch);
    Ok(())
}
