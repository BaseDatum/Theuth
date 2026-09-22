//! `decision bench` (57 §6 step 2 / §12.8 step 2e / follow-ups): cold load, RSS three ways,
//! warm p50/p95 per K × S, the published README example next to ours, parity against a dump.
//!
//! `cargo run --release -p decision --example bench -- [model_dir] [flags]`; the model dir
//! defaults to `$THEUTH_DATA_DIR/models/decision` (`~/.theuth`). Flags: `--device metal|cpu`
//! and `--dtype f16|f32` (defaults Metal f16 on an Apple-silicon Mac, CPU f32 elsewhere),
//! `--dump FILE` (write the README example's probabilities), `--ref FILE` (compare against a
//! dump: argmax parity and max probability delta — `tests/fixtures/probs-cpu-fp32.json` is the
//! step-2d ORT fp32 reference, kept as the independent-runtime baseline after ORT left the crate).

use std::path::PathBuf;
use std::time::{Duration, Instant};

use candle_core::DType;
use decision::candle::Loaded;
use decision::laya::{self, Sequence};
use decision::{Candidate, Question};
use tokenizers::Tokenizer;

/// Peak resident set (`ru_maxrss`); `NaN` where `getrusage` does not exist.
fn rss_mib() -> f64 {
    #[cfg(unix)]
    {
        let mut ru: libc::rusage = unsafe { std::mem::zeroed() };
        unsafe { libc::getrusage(libc::RUSAGE_SELF, &mut ru) };
        // macOS reports bytes, Linux kilobytes.
        let bytes = if cfg!(target_os = "macos") {
            ru.ru_maxrss as f64
        } else {
            ru.ru_maxrss as f64 * 1024.0
        };
        bytes / (1024.0 * 1024.0)
    }
    #[cfg(not(unix))]
    {
        f64::NAN
    }
}

/// Resident set *now* (`ps -o rss=`, KiB), as opposed to `ru_maxrss`'s peak: a load-time copy
/// that is freed before the first inference shows in the peak, not here.
fn rss_now_mib() -> f64 {
    std::process::Command::new("ps")
        .args(["-o", "rss=", "-p", &std::process::id().to_string()])
        .output()
        .ok()
        .and_then(|o| String::from_utf8(o.stdout).ok())
        .and_then(|s| s.trim().parse::<f64>().ok())
        .map_or(f64::NAN, |kib| kib / 1024.0)
}

fn pct(sorted: &[Duration], p: f64) -> Duration {
    sorted[((sorted.len() - 1) as f64 * p).round() as usize]
}

/// A state of at least `tokens` tokens (the tokenizer is the judge; prose repeated).
fn state_of(tok: &Tokenizer, tokens: usize) -> String {
    let unit = "s1: the deploy is blocked on the image tags, the pipeline retagged the release twice. \
                s2: right, ping the on-call and check the SRE ticket before the standup. ";
    let mut s = String::new();
    loop {
        s.push_str(unit);
        let n = tok.encode(s.as_str(), false).map(|e| e.len()).unwrap_or(0);
        if n >= tokens {
            return s;
        }
    }
}

fn choice(k: usize, state: String) -> Question {
    Question::Choice {
        instructions: "which cached brief is the subject being discussed".into(),
        candidates: (0..k)
            .map(|i| Candidate {
                id: format!("BRIEF-{i}"),
                text: format!("brief {i}: an incident in service {i} with alias {}", i * 7),
            })
            .collect(),
        state,
    }
}

fn main() -> anyhow::Result<()> {
    tracing_subscriber::fmt()
        .with_env_filter(
            tracing_subscriber::EnvFilter::try_from_default_env()
                .unwrap_or_else(|_| tracing_subscriber::EnvFilter::new("info")),
        )
        .with_writer(std::io::stderr)
        .init();
    let data_dir = std::env::var_os("THEUTH_DATA_DIR")
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from(std::env::var_os("HOME").expect("HOME")).join(".theuth"));
    let mut positional = Vec::new();
    let (mut dump, mut reference) = (None, None);
    let mut metal = decision::Device::Auto.metal();
    let mut f16 = None;
    let mut args = std::env::args().skip(1);
    while let Some(a) = args.next() {
        match a.as_str() {
            "--device" => metal = args.next().as_deref() == Some("metal"),
            "--dtype" => f16 = Some(args.next().as_deref() == Some("f16")),
            "--dump" => dump = args.next().map(PathBuf::from),
            "--ref" => reference = args.next().map(PathBuf::from),
            _ => positional.push(a),
        }
    }
    let dtype = if f16.unwrap_or(metal) {
        DType::F16
    } else {
        DType::F32
    };
    let dir = positional
        .first()
        .map(PathBuf::from)
        .unwrap_or_else(|| data_dir.join("models").join(decision::MODEL_SUBDIR));
    // 57 §12.8: RSS ≤ 1.5 GiB, p95 ≤ 50 ms; follow-up (1): cold load ≤ 0.5 s.
    let over = |mib: f64| -> &'static str {
        if mib <= 1536.0 { "≤ 1536 MiB" } else { "OVER 1536 MiB" }
    };
    println!(
        "model dir: {}  device: {}  dtype: {dtype:?}  rss before: {:.0} MiB",
        dir.display(),
        if metal { "metal" } else { "cpu" },
        rss_mib()
    );

    let t = Instant::now();
    let m = Loaded::open(&dir, metal, dtype)?;
    println!(
        "cold load: {} ms ({})  model id at load: {}  rss after load: {:.0} MiB  device: {:?} {:?}",
        t.elapsed().as_millis(),
        if t.elapsed() <= Duration::from_millis(500) {
            "≤ 0.5 s"
        } else {
            "OVER 0.5 s"
        },
        m.model_sha.lock(),
        rss_mib(),
        m.device.location(),
        m.dtype
    );
    println!(
        "config: max_len {} head_max_len {} temperature {:?}",
        m.laya.max_len, m.laya.head_max_len, m.laya.temperature
    );
    // First inference on its own: the RSS step from load to first run is what Metal allocates
    // lazily (and, once per machine, the shader compile).
    let warm = m
        .build(&choice(2, state_of(&m.tokenizer, 256)))
        .map_err(anyhow::Error::msg)?;
    let t = Instant::now();
    m.run(std::slice::from_ref(&warm))?;
    println!(
        "first inference: {} ms  rss after first inference: {:.0} MiB peak ({}), {:.0} MiB now ({})",
        t.elapsed().as_millis(),
        rss_mib(),
        over(rss_mib()),
        rss_now_mib(),
        over(rss_now_mib())
    );
    // (b) warm latency per K × S, 20 runs each after one warm-up.
    println!("\n| K | S (tokens) | padded to | p50 ms | p95 ms | ≤ 50 ms | RSS after MiB |");
    println!("|---|---|---|---|---|---|---|");
    let mut peak = rss_mib();
    for &k in &[2usize, 5, 10] {
        for &target in &[256usize, 512] {
            let q = choice(k, state_of(&m.tokenizer, target));
            let seq: Sequence = m.build(&q).map_err(anyhow::Error::msg)?;
            let s = seq.input_ids.len();
            m.run(std::slice::from_ref(&seq))?;
            let mut times = Vec::with_capacity(20);
            for _ in 0..20 {
                let t = Instant::now();
                m.run(std::slice::from_ref(&seq))?;
                times.push(t.elapsed());
            }
            times.sort();
            let rss = rss_mib();
            peak = peak.max(rss);
            let p95 = pct(&times, 0.95);
            println!(
                "| {k} | {s}{} | S {} K {} | {:.1} | {:.1} | {} | {rss:.0} |",
                if seq.truncated {
                    " (state truncated)"
                } else {
                    ""
                },
                s,
                k.max(2),
                pct(&times, 0.5).as_secs_f64() * 1e3,
                p95.as_secs_f64() * 1e3,
                if p95 <= Duration::from_millis(50) {
                    "yes"
                } else {
                    "NO"
                }
            );
        }
    }
    println!("rss peak: {peak:.0} MiB  ({})", over(peak));

    // (b2) 57 §4.1 item 1: one browser step = three jev-shaped questions (6 operations, ~45
    // click targets, ~10 typeable ones) over a ~1.5k-char state in one `AskMany` forward. On the
    // browser checkpoint (`head_max_len` 768, `max_len` 1024) the state is right-cut by design.
    let page = state_of(&m.tokenizer, 400);
    let page: String = page.chars().take(1500).collect();
    let element = |i: usize, role: &str| Candidate {
        id: i.to_string(),
        text: format!("[{i}] element number {i} on the page ({role})"),
    };
    let step = [
        Question::Choice {
            instructions: r#"{"goal": "open the article about the Python language", "rules": "Advance the user's entire goal from the CURRENT page using one operation."}"#.into(),
            candidates: ["CLICK", "TYPE_TEXT", "SCROLL_DOWN", "WAIT", "DONE", "BLOCKED"]
                .into_iter()
                .map(|id| Candidate { id: id.into(), text: format!("{id} …") })
                .collect(),
            state: page.clone(),
        },
        Question::Choice {
            instructions: r#"{"goal": "open the article about the Python language", "operation": "CLICK", "rules": ["…", "…"]}"#.into(),
            candidates: (1..=45).map(|i| element(i, "link")).collect(),
            state: page.clone(),
        },
        Question::Choice {
            instructions: r#"{"goal": "open the article about the Python language", "operation": "TYPE_TEXT", "rules": ["…", "…"]}"#.into(),
            candidates: (1..=10).map(|i| element(i, "textbox")).collect(),
            state: page.clone(),
        },
    ];
    let rows: Vec<Sequence> = step
        .iter()
        .map(|q| m.build(q).map_err(anyhow::Error::msg))
        .collect::<anyhow::Result<_>>()?;
    m.run(&rows)?;
    let mut times = Vec::with_capacity(20);
    for _ in 0..20 {
        let t = Instant::now();
        m.run(&rows)?;
        times.push(t.elapsed());
    }
    times.sort();
    println!(
        "browser step (3 questions × 6/45/10 candidates, one forward, S {} / K {}, state cut {}): p50 {:.1} ms  p95 {:.1} ms  ({} the §4.1 ≤ 300 ms target)",
        rows.iter().map(|r| r.input_ids.len()).max().unwrap_or(0),
        rows.iter().map(|r| r.marker_pos.len()).max().unwrap_or(0),
        rows.iter().any(|r| r.state_cut),
        pct(&times, 0.5).as_secs_f64() * 1e3,
        pct(&times, 0.95).as_secs_f64() * 1e3,
        if pct(&times, 0.95) <= Duration::from_millis(300) { "within" } else { "OVER" }
    );
    // (b3) The same step as three single-row forwards, each padded only to its own length:
    // the batch pads every row to the longest (S 1024) and our candle/Metal forward is close to
    // linear in padded tokens, so the batch may lose to the sum of the parts.
    let mut times = Vec::with_capacity(20);
    for _ in 0..20 {
        let t = Instant::now();
        for r in &rows {
            m.run(std::slice::from_ref(r))?;
        }
        times.push(t.elapsed());
    }
    times.sort();
    println!(
        "browser step (same 3 questions, three sequential single-row forwards, S {}): p50 {:.1} ms  p95 {:.1} ms  ({} the §4.1 ≤ 300 ms target)",
        rows.iter().map(|r| r.input_ids.len().to_string()).collect::<Vec<_>>().join("/"),
        pct(&times, 0.5).as_secs_f64() * 1e3,
        pct(&times, 0.95).as_secs_f64() * 1e3,
        if pct(&times, 0.95) <= Duration::from_millis(300) { "within" } else { "OVER" }
    );

    // (c) The README / model-card worked example (NandhaKishorM/laya README "Quickstart",
    // convaiinnovations/laya card "Single-Model Mode"): one state, four questions, one pass.
    // Published: department → billing (confidence 0.94); urgency score → 1.84 / 2.0;
    // churn_risk → 0.892; refund_requested: no number given.
    let state = r#"{"from": "user@acme.com", "subject": "Duplicate charge on invoice #4411", "body": "Hi, we were billed twice for March. Please refund the duplicate today or we will cancel our plan."}"#.to_owned();
    let qs = [
        (
            "department",
            Question::Choice {
                instructions: "Which department should handle this request?".into(),
                candidates: [
                    ("billing", "invoices, payments, refunds"),
                    ("technical", "bugs, outages, system errors"),
                    ("sales", "pricing, new contracts"),
                    ("other", "everything else"),
                ]
                .into_iter()
                .map(|(id, text)| Candidate {
                    id: id.into(),
                    text: text.into(),
                })
                .collect(),
                state: state.clone(),
            },
        ),
        (
            "urgency",
            Question::Score {
                instructions: "How urgent is this request?".into(),
                levels: vec![
                    "not urgent".into(),
                    "soon".into(),
                    "critical deadline or blocking issue".into(),
                ],
                state: state.clone(),
            },
        ),
        (
            "churn_risk",
            Question::Bool {
                statement: "Does the user threaten to cancel or leave?".into(),
                state: state.clone(),
            },
        ),
        (
            "refund_requested",
            Question::Bool {
                statement: "Does the user explicitly request a refund?".into(),
                state: state.clone(),
            },
        ),
    ];
    let rows: Vec<Sequence> = qs
        .iter()
        .map(|(_, q)| m.build(q).map_err(anyhow::Error::msg))
        .collect::<anyhow::Result<_>>()?;
    let t = Instant::now();
    let outs = m.run(&rows)?;
    println!(
        "\npublished example, {} questions in one pass: {} ms",
        rows.len(),
        t.elapsed().as_millis()
    );
    println!("| question | ours (fitted T) | ours at T = 1 | published | within 0.01 |");
    println!("|---|---|---|---|---|");
    for ((name, q), out) in qs.iter().zip(&outs) {
        let labels = laya::labels(q);
        let k = labels.len();
        let raw = laya::softmax_with_temperature(&out.logits, k, 1.0);
        let fmt = |ps: &[f32]| -> String {
            labels
                .iter()
                .zip(ps)
                .map(|(l, p)| format!("{l} {p:.3}"))
                .collect::<Vec<_>>()
                .join(", ")
        };
        let score = |ps: &[f32]| -> f32 { ps.iter().enumerate().map(|(i, p)| i as f32 * p).sum() };
        let top = out.probs.iter().copied().fold(0.0, f32::max);
        let (ours, raw_s, published, ok) = match *name {
            "department" => (
                format!(
                    "{}; confidence {:.2} (T {:.2})",
                    fmt(&out.probs),
                    out.confidence,
                    out.temperature
                ),
                format!(
                    "{}; confidence {:.2}",
                    fmt(&raw),
                    laya::entropy_confidence(&raw, k)
                ),
                "billing (confidence 0.94)".to_owned(),
                Some((out.confidence - 0.94).abs() <= 0.01 && out.probs[0] == top),
            ),
            "urgency" => (
                format!(
                    "{}; score {:.2} / 2.0 (T {:.2})",
                    fmt(&out.probs),
                    score(&out.probs),
                    out.temperature
                ),
                format!("{}; score {:.2}", fmt(&raw), score(&raw)),
                "score 1.84 / 2.0".to_owned(),
                Some((score(&out.probs) - 1.84).abs() <= 0.01),
            ),
            "churn_risk" => (
                format!("noul {:.3} (T {:.2})", out.probs[1], out.temperature),
                format!("noul {:.3}", raw[1]),
                "0.892".to_owned(),
                Some((out.probs[1] - 0.892).abs() <= 0.01),
            ),
            _ => (
                format!("noul {:.3} (T {:.2})", out.probs[1], out.temperature),
                format!("noul {:.3}", raw[1]),
                "(no number published)".to_owned(),
                None,
            ),
        };
        println!(
            "| {name} | {ours} | {raw_s} | {published} | {} |",
            match ok {
                Some(true) => "yes",
                Some(false) => "NO",
                None => "—",
            }
        );
    }
    // (d) Parity against another run's dump (57 §12.6: argmax parity, max probability delta).
    let probs: Vec<Vec<f32>> = outs.iter().map(|o| o.probs.clone()).collect();
    if let Some(path) = dump {
        std::fs::write(&path, serde_json::to_vec(&probs)?)?;
        println!("\ndumped probabilities to {}", path.display());
    }
    if let Some(path) = reference {
        let other: Vec<Vec<f32>> = serde_json::from_slice(&std::fs::read(&path)?)?;
        let argmax = |p: &[f32]| {
            p.iter()
                .enumerate()
                .max_by(|a, b| a.1.total_cmp(b.1))
                .map(|(i, _)| i)
        };
        let agree = probs
            .iter()
            .zip(&other)
            .filter(|(a, b)| argmax(a) == argmax(b))
            .count();
        let delta = probs
            .iter()
            .zip(&other)
            .flat_map(|(a, b)| a.iter().zip(b).map(|(x, y)| (x - y).abs()))
            .fold(0.0f32, f32::max);
        println!(
            "\nparity vs {}: argmax {agree}/{} agree ({}), max |Δp| {delta:.4}",
            path.display(),
            probs.len(),
            if agree == probs.len() { "PASS" } else { "FAIL" }
        );
    }
    println!(
        "\nrss final: {:.0} MiB  model id now: {}",
        rss_mib(),
        m.model_sha.lock()
    );
    Ok(())
}
