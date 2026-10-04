//! 92 step 2 bench: `decide_many`'s batch size. 500 synthetic ~150-token rows, 5 labels, sent
//! as `AskMany` batches of 8 / 16 / 32 through the engine actor exactly as the tool sends
//! them, on the gliner and the laya checkpoint, with `batch_forward` on (one padded forward
//! per batch) and off (one forward per row, the batch only shapes the mailbox). Prints one
//! markdown row per cell: p50 / p95 ms per row (per batch wall ÷ rows), rows/s, footprint.
//!
//! `cargo run --release -p decision --example decide_bench -- [--rows 500] [--checkpoint gliner|default] [--batch 8,16,32] [--forward both|on|off]`
//!
//! Checkpoints under `$THEUTH_MODELS_DIR` (`~/.theuth/models`): `decision-gliner/`, `decision/`.

use std::path::PathBuf;
use std::time::{Duration, Instant};

use decision::engine::AskMany;
use decision::log::Log;
use decision::{
    Ask, Candidate, Checkpoint, DecisionConfig, Disposition, Engine, FamilyGate, Kind, Question,
    Release, TOOL_FAMILY,
};
use kameo::actor::Spawn;
use tokenizers::Tokenizer;

const LABELS: [&str; 5] = [
    "invoices and receipts",
    "recruiting and candidates",
    "newsletters and announcements",
    "personal",
    "urgent operations",
];

/// A state of about `tokens` subwords, varied per row so the tokenizer and the model see
/// different text.
fn state_of(tok: &Tokenizer, tokens: usize, i: usize) -> String {
    let units = [
        "Hi team, the invoice for order #{i} is attached, net 30, please confirm receipt. ",
        "Reminder: the candidate interview for req {i} moved to Thursday 10:00, room B. ",
        "Weekly digest #{i}: three releases, two incidents, one new dashboard. ",
        "Deploy {i} failed on the canary; rolling back now, follow the thread for updates. ",
    ];
    let mut s = String::new();
    let mut k = i;
    while tok.encode(s.as_str(), false).map(|e| e.len()).unwrap_or(0) < tokens {
        s.push_str(&units[k % units.len()].replace("{i}", &(1000 + i).to_string()));
        k += 1;
    }
    s
}

fn question(state: String) -> Question {
    Question::Choice {
        instructions: "Which folder does this email belong in?".into(),
        candidates: LABELS
            .iter()
            .enumerate()
            .map(|(i, text)| Candidate {
                id: i.to_string(),
                text: (*text).to_owned(),
            })
            .collect(),
        state,
    }
}

fn percentile(sorted: &[f64], p: f64) -> f64 {
    if sorted.is_empty() {
        return 0.0;
    }
    let idx = ((sorted.len() - 1) as f64 * p).round() as usize;
    sorted[idx]
}

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    tracing_subscriber::fmt()
        .with_env_filter(
            tracing_subscriber::EnvFilter::try_from_default_env()
                .unwrap_or_else(|_| tracing_subscriber::EnvFilter::new("warn")),
        )
        .with_writer(std::io::stderr)
        .init();
    let models = std::env::var_os("THEUTH_MODELS_DIR")
        .map(PathBuf::from)
        .unwrap_or_else(|| {
            PathBuf::from(std::env::var_os("HOME").expect("HOME")).join(".theuth/models")
        });
    let (mut rows, mut checkpoints, mut batches, mut forward) = (
        500usize,
        vec!["gliner".to_owned(), "default".to_owned()],
        vec![8usize, 16, 32],
        "both".to_owned(),
    );
    let mut args = std::env::args().skip(1);
    while let Some(a) = args.next() {
        match a.as_str() {
            "--rows" => rows = args.next().unwrap().parse()?,
            "--checkpoint" => checkpoints = vec![args.next().unwrap()],
            "--batch" => {
                batches = args
                    .next()
                    .unwrap()
                    .split(',')
                    .map(|b| b.parse())
                    .collect::<Result<_, _>>()?
            }
            "--forward" => forward = args.next().unwrap(),
            other => anyhow::bail!("unknown flag {other}"),
        }
    }
    let forwards: Vec<bool> = match forward.as_str() {
        "on" => vec![true],
        "off" => vec![false],
        _ => vec![true, false],
    };
    let tmp = tempfile::tempdir()?;
    println!(
        "| checkpoint | forward | batch | rows | tokens/row | p50 ms/row | p95 ms/row | rows/s | total s | footprint MiB |"
    );
    println!("|---|---|---|---|---|---|---|---|---|---|");
    for name in &checkpoints {
        let (dir, tok_path, kind) = if name == "gliner" {
            let d = models.join("decision-gliner");
            (d.clone(), d.join("tokenizer.json"), Kind::Gliner)
        } else {
            let d = models.join("decision");
            (d.clone(), d.join("tokenizer/tokenizer.json"), Kind::Laya)
        };
        if !tok_path.is_file() {
            eprintln!("SKIP {name}: {} missing", tok_path.display());
            continue;
        }
        let tok = Tokenizer::from_file(&tok_path).map_err(|e| anyhow::anyhow!("{e}"))?;
        let asks: Vec<Ask> = (0..rows)
            .map(|i| {
                let mut a = Ask::new(TOOL_FAMILY, question(state_of(&tok, 150, i)));
                a.budget = Duration::from_secs(600);
                a
            })
            .collect();
        let tokens: usize = asks
            .iter()
            .map(|a| match &a.question {
                Question::Choice { state, .. } => tok.encode(state.as_str(), false).unwrap().len(),
                _ => 0,
            })
            .sum::<usize>()
            / rows.max(1);
        for &batch_forward in &forwards {
            for &batch in &batches {
                let mut cfg = DecisionConfig {
                    batch_forward,
                    ..DecisionConfig::default()
                };
                let mut entry = Checkpoint::from(dir.clone());
                entry.kind = kind;
                cfg.checkpoints.insert(name.clone(), entry);
                let mut gate = FamilyGate::tool();
                gate.checkpoint = Some(name.clone());
                cfg.families.insert(TOOL_FAMILY.to_owned(), gate);
                let engine = Engine::spawn(
                    Engine::new(
                        cfg,
                        models.join("decision"),
                        Log::new(
                            tmp.path()
                                .join(format!("log-{name}-{batch_forward}-{batch}")),
                        ),
                    )
                    .with_checkpoint(name, dir.clone()),
                );
                // Warm: load + first forward, outside the measurement.
                let warm = engine
                    .ask(AskMany {
                        asks: asks[..batch.min(rows)].to_vec(),
                        checkpoint: Some(name.clone()),
                    })
                    .await?;
                if let Some(d) = warm
                    .iter()
                    .find(|d| matches!(d.disposition, Disposition::Escalate(_)))
                {
                    eprintln!("SKIP {name}: {:?}", d.disposition);
                    break;
                }
                let t_all = Instant::now();
                let mut per_row = Vec::new();
                let mut escalated = 0usize;
                for chunk in asks.chunks(batch) {
                    let t = Instant::now();
                    let ds = engine
                        .ask(AskMany {
                            asks: chunk.to_vec(),
                            checkpoint: Some(name.clone()),
                        })
                        .await?;
                    let ms = t.elapsed().as_secs_f64() * 1e3 / chunk.len() as f64;
                    per_row.extend(std::iter::repeat_n(ms, chunk.len()));
                    escalated += ds
                        .iter()
                        .filter(|d| matches!(d.disposition, Disposition::Escalate(_)))
                        .count();
                }
                let total = t_all.elapsed().as_secs_f64();
                per_row.sort_by(f64::total_cmp);
                let fp = decision::process_memory().footprint as f64 / (1024.0 * 1024.0);
                println!(
                    "| {name} | {} | {batch} | {rows} | {tokens} | {:.1} | {:.1} | {:.1} | {total:.1} | {fp:.0} |{}",
                    if batch_forward { "padded" } else { "per-row" },
                    percentile(&per_row, 0.5),
                    percentile(&per_row, 0.95),
                    rows as f64 / total,
                    if escalated > 0 {
                        format!(" ⚠ {escalated} escalated")
                    } else {
                        String::new()
                    }
                );
                engine
                    .tell(Release {
                        checkpoint: None,
                        reason: "bench cell done",
                    })
                    .await?;
                tokio::time::sleep(Duration::from_millis(500)).await;
            }
        }
    }
    Ok(())
}
