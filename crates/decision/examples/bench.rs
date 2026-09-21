//! `decision bench` (57 §6 step 2): cold load, RSS, warm p50/p95 per K × S, and the
//! published README example next to ours.
//!
//! `cargo run --release -p decision --example bench -- [model_dir] [threads]`

use std::path::PathBuf;
use std::time::{Duration, Instant};

use decision::laya::{self, Loaded, Sequence};
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
    let dir = std::env::args()
        .nth(1)
        .map(PathBuf::from)
        .unwrap_or_else(|| {
            PathBuf::from(std::env::var_os("HOME").expect("HOME")).join(".theuth/models/decision")
        });
    let threads: usize = std::env::args()
        .nth(2)
        .and_then(|t| t.parse().ok())
        .unwrap_or(4);
    println!(
        "model dir: {}  threads: {threads}  rss before: {:.0} MiB",
        dir.display(),
        rss_mib()
    );

    let t = Instant::now();
    let mut m = Loaded::open(&dir, threads, false)?;
    println!(
        "cold load: {} ms (sha256 {}…)  rss after load: {:.0} MiB",
        t.elapsed().as_millis(),
        &m.model_sha[..12],
        rss_mib()
    );
    println!(
        "config: max_len {} head_max_len {} temperature {:?}",
        m.laya.max_len, m.laya.head_max_len, m.laya.temperature
    );

    // (b) warm latency per K × S, 20 runs each after one warm-up.
    println!("\n| K | S (tokens) | p50 ms | p95 ms | RSS after MiB |");
    println!("|---|---|---|---|---|");
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
                "| {k} | {s}{} | {:.1} | {:.1} | {rss:.0} |",
                if seq.truncated {
                    " (state truncated)"
                } else {
                    ""
                },
                pct(&times, 0.5).as_secs_f64() * 1e3,
                pct(&times, 0.95).as_secs_f64() * 1e3
            );
        }
    }
    println!(
        "rss peak: {peak:.0} MiB  ({})",
        if peak < 1024.0 {
            "under 1 GiB"
        } else {
            "OVER 1 GiB"
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
    println!("\nrss final: {:.0} MiB", rss_mib());
    Ok(())
}
