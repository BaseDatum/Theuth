//! `decision bench` (57 §6 step 2 / §12.7 step 2d): cold load, RSS breakdown, warm p50/p95
//! per K × S, the CoreML partition report and the published README example next to ours.
//!
//! `cargo run --release -p decision --example bench -- [model_dir] [threads] [flags]`; the ONNX
//! Runtime is found under `$THEUTH_DATA_DIR` (default `~/.theuth`) like the daemon does.
//! Flags: `--coreml=all|ane` (CoreML EP, padded to S 512 / K 32), `--pad` (pad on CPU too),
//! `--dump FILE` (write the README example's probabilities), `--ref FILE` (compare against a
//! dump: argmax parity and max probability delta).

use std::path::PathBuf;
use std::time::{Duration, Instant};

use decision::laya::{self, Loaded, Provider, Sequence, Units};
use decision::{Candidate, Question};

fn rss_mib() -> f64 {
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

fn pct(sorted: &[Duration], p: f64) -> Duration {
    sorted[((sorted.len() - 1) as f64 * p).round() as usize]
}

/// A state of at least `tokens` tokens (the tokenizer is the judge; prose repeated).
fn state_of(m: &Loaded, tokens: usize) -> String {
    let unit = "s1: the deploy is blocked on the image tags, the pipeline retagged the release twice. \
                s2: right, ping the on-call and check the SRE ticket before the standup. ";
    let mut s = String::new();
    loop {
        s.push_str(unit);
        let n = m
            .tokenizer
            .encode(s.as_str(), false)
            .map(|e| e.len())
            .unwrap_or(0);
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
    let (mut provider, mut pad, mut dump, mut reference) = (Provider::Cpu, false, None, None);
    let mut args = std::env::args().skip(1);
    while let Some(a) = args.next() {
        match a.as_str() {
            "--coreml=all" => provider = Provider::CoreMl(Units::All),
            "--coreml=ane" => provider = Provider::CoreMl(Units::CpuAndNeuralEngine),
            "--pad" => pad = true,
            "--dump" => dump = args.next().map(PathBuf::from),
            "--ref" => reference = args.next().map(PathBuf::from),
            _ => positional.push(a),
        }
    }
    let dir = positional
        .first()
        .map(PathBuf::from)
        .unwrap_or_else(|| data_dir.join("models/decision"));
    let threads: usize = positional.get(1).and_then(|t| t.parse().ok()).unwrap_or(4);
    println!(
        "model dir: {}  threads: {threads}  provider: {provider:?}  rss before: {:.0} MiB",
        dir.display(),
        rss_mib()
    );

    let t = Instant::now();
    let mut m = Loaded::open(&dir, &data_dir, threads, provider)?;
    if pad {
        m.bucket = laya::BUCKET;
    }
    println!(
        "cold load (incl. compile): {} ms (sha256 {}…)  rss after load: {:.0} MiB  provider: {:?}  bucket: {:?}",
        t.elapsed().as_millis(),
        &m.model_sha[..12],
        rss_mib(),
        m.provider,
        m.bucket
    );
    match m.partitions {
        Some((n, nodes, took)) => println!(
            "coreml partitions: {n}  nodes in graph: {nodes}  nodes on CoreML: {took}  ({})",
            if n <= 3 { "≤ 3, PASS" } else { "> 3, FAIL" }
        ),
        None if matches!(m.provider, Provider::CoreMl(_)) => {
            println!("coreml partitions: (no GetCapability line seen)")
        }
        None => {}
    }
    println!(
        "config: max_len {} head_max_len {} temperature {:?}",
        m.laya.max_len, m.laya.head_max_len, m.laya.temperature
    );
    // First inference on its own: the RSS step from load to first run is what Core ML
    // allocates lazily.
    let warm = m
        .build(&choice(2, state_of(&m, 256)))
        .map_err(anyhow::Error::msg)?;
    let t = Instant::now();
    m.run(std::slice::from_ref(&warm))?;
    println!(
        "first inference: {} ms  rss after first inference: {:.0} MiB ({})",
        t.elapsed().as_millis(),
        rss_mib(),
        if rss_mib() <= 2048.0 {
            "≤ 2 GiB"
        } else {
            "OVER 2 GiB"
        }
    );

    // (b) warm latency per K × S, 20 runs each after one warm-up.
    println!("\n| K | S (tokens) | padded to | p50 ms | p95 ms | RSS after MiB |");
    println!("|---|---|---|---|---|---|");
    let mut peak = rss_mib();
    for &k in &[2usize, 5, 10] {
        for &target in &[256usize, 512] {
            let q = choice(k, state_of(&m, target));
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
            println!(
                "| {k} | {s}{} | S {} K {} | {:.1} | {:.1} | {rss:.0} |",
                if seq.truncated {
                    " (state truncated)"
                } else {
                    ""
                },
                s.max(m.bucket.0),
                k.max(m.bucket.1),
                pct(&times, 0.5).as_secs_f64() * 1e3,
                pct(&times, 0.95).as_secs_f64() * 1e3
            );
        }
    }
    println!(
        "rss peak: {peak:.0} MiB  ({})",
        if peak <= 2048.0 {
            "under 2 GiB"
        } else {
            "OVER 2 GiB"
        }
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
    println!("\nrss final: {:.0} MiB", rss_mib());
    Ok(())
}
