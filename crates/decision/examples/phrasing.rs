//! `decision phrasing` (57 §3.5): how the meeting oracle's two tier-1 questions should be
//! worded, decided by numbers on `tests/fixtures/meeting-batches.json` — real batches from
//! recorded meetings (labelled `kind` by hand, `published` by what the model's turn did) plus
//! hand-written small talk, narratives and brief-matching batches.
//!
//! `cargo run --release -p decision --example phrasing -- [model_dir] [--device metal|cpu]
//! [--only 1a|1b] [--verbose] [--fixture PATH]`. Model-gated: prints a reason and exits 0 when
//! the checkpoint is not there. `--fixture` replaces the built-in fixture with an exported one
//! (`t --decision-export meeting --out FILE`, 57 §11.4), so captured data is benched the same way.
//!
//! - **1a `meeting.kind`**: every variant — 3-way `Choice` phrasings × label sets, 2-way
//!   `Choice`s, and `Bool` statements — with the confusion matrix, mean P(correct), and the
//!   cost that matters: how many *skip* verdicts (`small_talk` / `true`) at τ ∈ {0.6, 0.8, 0.9}
//!   fall on non-small-talk batches, and how many of those hid a published brief.
//! - **1b `meeting.brief`**: the unchanged instructions over title-only candidates, K ∈ {4, 8}
//!   and two id styles (the row's UUID vs `b1..bK` — a UUID is ~25 tokens of the 192-token head
//!   budget per option); per variant the pick accuracy, mean P(correct), and publishes at
//!   τ 0.6 / margin 0.3 split into right and wrong.
//!
//! 2026-09-21 result (57 as-built step 4b): every 3-way `Choice` is near-uniform (mean
//! P(correct) 0.27–0.31, everything lands on `general`); the `Bool` head is biased to `false`,
//! which makes it a usable *small-talk detector* and a useless subject detector. Winner:
//! `Bool("The speakers are only greeting, scheduling or saying goodbye")`, skip at P(true) ≥ 0.6
//! (`oracle::KIND_STATEMENT`); 1b: K = 4, `b1..b4` ids, titles only. The winner's constants live
//! in `meeting::oracle`; re-run this when changing them.

use std::path::PathBuf;
use std::time::{Duration, Instant};

use candle_core::DType;
use decision::candle::{Gpu, Loaded};
use decision::laya::RowOut;
use decision::{Candidate, Question};
use serde::Deserialize;

#[derive(Deserialize)]
struct Fixture {
    batches: Vec<Batch>,
}

#[derive(Deserialize)]
struct Batch {
    id: String,
    kind: String,
    state: String,
    published: Option<Vec<Published>>,
    expected_brief: Option<String>,
    candidates: Vec<Cand>,
}

#[derive(Deserialize)]
struct Published {
    title: String,
}

#[derive(Deserialize)]
struct Cand {
    id: String,
    title: String,
}

const KINDS: [&str; 3] = ["small_talk", "named_subject", "general"];

/// 3-way instructions under test. Questions are about the text only, never about the listener.
const INSTRUCTIONS: [&str; 4] = [
    "What is this stretch of conversation mostly about?",
    "Which best describes what the speakers are talking about?",
    "Is the conversation small talk, about a specific named subject, or a general discussion?",
    "Does the conversation name a specific subject?",
];

/// 3-way label sets (`small_talk`, `named_subject`, `general` texts).
const LABEL_SETS: [(&str, [&str; 3]); 3] = [
    (
        "A (§3.5)",
        [
            "small talk, greetings or scheduling",
            "a specific named subject (a person, place, object, ticket, system, event or technical term)",
            "general discussion without one specific subject",
        ],
    ),
    (
        "B (short)",
        [
            "small talk or scheduling, nothing to look up",
            "a specific named thing: a person, product, system, ticket, place or term",
            "a general discussion with no single named subject",
        ],
    ),
    (
        "C (concrete)",
        [
            "greetings, goodbyes, scheduling, acknowledgements",
            "a named person, system, product, ticket, place or event",
            "anything else",
        ],
    ),
];

/// 2-way `Choice` variants: (instructions, small-talk text, subject text).
const TWO_WAY: [(&str, &str, &str); 3] = [
    (
        "Is this small talk or a conversation about something?",
        "small talk: greetings, scheduling, goodbyes, acknowledgements",
        "a conversation about something: a subject, a problem, a person, a system, a plan",
    ),
    (
        "Which describes the transcript?",
        "only greetings, scheduling or goodbyes",
        "the speakers discuss a subject",
    ),
    (
        "What kind of conversation is this?",
        "small talk",
        "a discussion with content",
    ),
];

/// `Bool` variants: (statement, truth is `true` when kind == small_talk).
const BOOLS: [(&str, bool); 6] = [
    (
        "The speakers are only greeting, scheduling or saying goodbye",
        true,
    ),
    ("This is small talk with nothing to look up", true),
    (
        "Nothing of substance is discussed: only greetings, thanks, scheduling or goodbyes",
        true,
    ),
    (
        "The speakers only exchange pleasantries, thanks, scheduling or goodbyes; no subject is discussed",
        true,
    ),
    (
        "The speakers discuss a specific named subject: a person, system, product, ticket, place or event",
        false,
    ),
    (
        "The conversation has a subject someone could look up",
        false,
    ),
];

/// 1b instructions (unchanged from step 4, `oracle::BRIEF_INSTRUCTIONS`).
const BRIEF_INSTRUCTIONS: &str = "which cached brief is the subject being discussed";

fn pct(sorted: &[Duration], p: f64) -> Duration {
    sorted[((sorted.len() - 1) as f64 * p).round() as usize]
}

fn argmax(ps: &[f32]) -> usize {
    ps.iter()
        .enumerate()
        .max_by(|a, b| a.1.total_cmp(b.1))
        .map_or(0, |(i, _)| i)
}

/// One 1a variant: how to ask, the label names in option order, which option means "skip", and
/// the truth index for a batch.
struct KindVariant {
    name: String,
    labels: Vec<&'static str>,
    skip: usize,
    question: Box<dyn Fn(&str) -> Question>,
    truth: Box<dyn Fn(&Batch) -> usize>,
}

fn kind_variants() -> Vec<KindVariant> {
    let mut v = Vec::new();
    let kind_index = |b: &Batch| {
        KINDS
            .iter()
            .position(|k| *k == b.kind)
            .expect("labelled kind")
    };
    for (lname, texts) in LABEL_SETS {
        for instr in INSTRUCTIONS {
            v.push(KindVariant {
                name: format!("3-way `{instr}` · {lname}"),
                labels: KINDS.to_vec(),
                skip: 0,
                question: Box::new(move |state| Question::Choice {
                    instructions: instr.to_owned(),
                    candidates: KINDS
                        .iter()
                        .zip(texts)
                        .map(|(id, text)| Candidate {
                            id: (*id).to_owned(),
                            text: text.to_owned(),
                        })
                        .collect(),
                    state: state.to_owned(),
                }),
                truth: Box::new(kind_index),
            });
        }
    }
    for (instr, small, subject) in TWO_WAY {
        v.push(KindVariant {
            name: format!("2-way `{instr}` · `{small}` / `{subject}`"),
            labels: vec!["small_talk", "subject"],
            skip: 0,
            question: Box::new(move |state| Question::Choice {
                instructions: instr.to_owned(),
                candidates: vec![
                    Candidate {
                        id: "small_talk".into(),
                        text: small.to_owned(),
                    },
                    Candidate {
                        id: "subject".into(),
                        text: subject.to_owned(),
                    },
                ],
                state: state.to_owned(),
            }),
            truth: Box::new(|b| usize::from(b.kind != "small_talk")),
        });
    }
    for (statement, true_is_small) in BOOLS {
        v.push(KindVariant {
            name: format!("Bool `{statement}`"),
            labels: vec!["false", "true"],
            skip: usize::from(true_is_small),
            question: Box::new(move |state| Question::Bool {
                statement: statement.to_owned(),
                state: state.to_owned(),
            }),
            truth: Box::new(move |b| {
                let small = b.kind == "small_talk";
                usize::from(if true_is_small { small } else { !small })
            }),
        });
    }
    v
}

fn main() -> anyhow::Result<()> {
    let data_dir = std::env::var_os("THEUTH_DATA_DIR")
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from(std::env::var_os("HOME").expect("HOME")).join(".theuth"));
    let mut positional = Vec::new();
    let mut metal = decision::Device::Auto.metal();
    let mut verbose = false;
    let mut only: Option<String> = None;
    let mut variant: Option<usize> = None;
    let mut fixture_path: Option<PathBuf> = None;
    let mut args = std::env::args().skip(1);
    while let Some(a) = args.next() {
        match a.as_str() {
            "--device" => metal = args.next().as_deref() == Some("metal"),
            "--verbose" => verbose = true,
            "--only" => only = args.next(),
            "--variant" => variant = args.next().and_then(|v| v.parse().ok()),
            "--fixture" => fixture_path = args.next().map(PathBuf::from),
            _ => positional.push(a),
        }
    }
    let part = |p: &str| only.as_deref().is_none_or(|o| o == p);
    let dir = positional
        .first()
        .map(PathBuf::from)
        .unwrap_or_else(|| data_dir.join("models").join(decision::MODEL_SUBDIR));
    if !decision::engine::MODEL_FILES
        .iter()
        .all(|f| dir.join(f).is_file())
    {
        println!(
            "phrasing: no checkpoint in {} — nothing to bench",
            dir.display()
        );
        return Ok(());
    }
    let fixture: Fixture = match &fixture_path {
        Some(p) => serde_json::from_str(&std::fs::read_to_string(p)?)?,
        None => serde_json::from_str(include_str!("../tests/fixtures/meeting-batches.json"))?,
    };
    let batches = &fixture.batches;
    let published = batches.iter().filter(|b| b.published.is_some()).count();
    println!(
        "{} batches ({} small_talk, {} named_subject, {} general; {published} with a published brief, {} with a cached one among the candidates)",
        batches.len(),
        batches.iter().filter(|b| b.kind == "small_talk").count(),
        batches.iter().filter(|b| b.kind == "named_subject").count(),
        batches.iter().filter(|b| b.kind == "general").count(),
        batches
            .iter()
            .filter(|b| b.expected_brief.is_some())
            .count(),
    );

    let t = Instant::now();
    let m = Loaded::open(
        &dir,
        if metal { Gpu::Metal } else { Gpu::None },
        if metal { DType::F16 } else { DType::F32 },
    )?;
    println!(
        "model: {} loaded in {} ms on {:?}\n",
        dir.display(),
        t.elapsed().as_millis(),
        m.device.location()
    );
    // Warm-up run so the first row's shader compile does not land in a p50.
    let warm = m
        .build(&Question::Bool {
            statement: "warm".into(),
            state: "warm".into(),
        })
        .map_err(anyhow::Error::msg)?;
    m.run(std::slice::from_ref(&warm))?;

    // One row at a time: production asks one question per batch, so that is the latency.
    let run_one = |q: &Question| -> anyhow::Result<(RowOut, Duration, bool)> {
        let seq = m.build(q).map_err(anyhow::Error::msg)?;
        let t = Instant::now();
        let out = m.run(std::slice::from_ref(&seq))?;
        let dt = t.elapsed();
        Ok((out.into_iter().next().expect("one row"), dt, seq.truncated))
    };

    // ---- 1a: kind ------------------------------------------------------------------------
    if part("1a") {
        let variants = kind_variants();
        println!("## 1a `meeting.kind` — {} variants\n", variants.len());
        println!(
            "Confusion rows = truth, cols = argmax, option order as listed. `skips@τ` = skip verdicts (`small_talk` / `true`) with P ≥ τ, as `small talk / other / hidden` — `other` are non-small-talk batches skipped (the false positives), `hidden` those among them whose turn published a brief (the cost that matters).\n"
        );
        println!(
            "| # | variant | labels | acc | mean P(correct) | P(correct) per truth | confusion | skips@0.6 | skips@0.8 | skips@0.9 | p50 / p95 ms |"
        );
        println!("|---|---|---|---|---|---|---|---|---|---|---|");
        let mut best: Option<((usize, usize, i64), String)> = None;
        for (n, v) in variants
            .iter()
            .enumerate()
            .filter(|(n, _)| variant.is_none_or(|w| w == n + 1))
        {
            let k = v.labels.len();
            let mut conf = vec![vec![0usize; k]; k];
            let mut p_sum = vec![0f32; k];
            let mut p_n = vec![0usize; k];
            // Per τ ∈ {0.6, 0.8, 0.9}: skipped small talk, skipped other, hidden briefs.
            let mut skips = [[0usize; 3]; 3];
            let mut times = Vec::with_capacity(batches.len());
            let mut hidden_ids: Vec<String> = Vec::new();
            for b in batches {
                let (out, dt, _) = run_one(&(v.question)(&b.state))?;
                times.push(dt);
                let truth = (v.truth)(b);
                let pred = argmax(&out.probs);
                conf[truth][pred] += 1;
                p_sum[truth] += out.probs[truth];
                p_n[truth] += 1;
                if pred == v.skip {
                    let p = out.probs[v.skip];
                    for (i, tau) in [0.6f32, 0.8, 0.9].iter().enumerate() {
                        if p < *tau {
                            continue;
                        }
                        skips[i][usize::from(b.kind != "small_talk")] += 1;
                        if b.published.is_some() {
                            skips[i][2] += 1;
                            if i == 0 {
                                hidden_ids.push(format!(
                                    "{} ({}, {p:.2})",
                                    b.id,
                                    b.published
                                        .as_ref()
                                        .and_then(|p| p.first())
                                        .map_or("?", |p| p.title.as_str())
                                ));
                            }
                        }
                    }
                }
                if verbose {
                    println!(
                        "    {:<16} truth {:<13} → {:<13} {:?}",
                        b.id,
                        v.labels[truth],
                        v.labels[pred],
                        out.probs
                            .iter()
                            .map(|p| (p * 100.0).round() / 100.0)
                            .collect::<Vec<_>>()
                    );
                }
            }
            times.sort();
            let total = batches.len();
            let correct: usize = (0..k).map(|i| conf[i][i]).sum();
            let mean_p = p_sum.iter().sum::<f32>() / total as f32;
            let per: Vec<String> = (0..k)
                .map(|i| {
                    format!(
                        "{} {:.2}",
                        v.labels[i],
                        if p_n[i] == 0 {
                            0.0
                        } else {
                            p_sum[i] / p_n[i] as f32
                        }
                    )
                })
                .collect();
            let cell = |s: &[usize; 3]| format!("{} / {} / {}", s[0], s[1], s[2]);
            println!(
                "| {} | {} | {} | {correct}/{total} | {mean_p:.3} | {} | `{conf:?}` | {} | {} | {} | {:.0} / {:.0} |",
                n + 1,
                v.name,
                v.labels.join(" / "),
                per.join(", "),
                cell(&skips[0]),
                cell(&skips[1]),
                cell(&skips[2]),
                pct(&times, 0.5).as_secs_f64() * 1e3,
                pct(&times, 0.95).as_secs_f64() * 1e3,
            );
            if !hidden_ids.is_empty() {
                println!("|   | ↳ hidden@0.6: {} |", hidden_ids.join("; "));
            }
            // At τ 0.6: no hidden brief, then the fewest false positives, then the most small talk skipped.
            let key = (skips[0][2], skips[0][1], -(skips[0][0] as i64));
            if best.as_ref().is_none_or(|(bk, _)| key < *bk) {
                best = Some((key, format!("#{}: {}", n + 1, v.name)));
            }
        }
        if let Some((_, w)) = &best {
            println!("\n1a winner by (hidden, other, −small talk) skipped at τ 0.6: {w}\n");
        }
    }

    // ---- 1b: brief -----------------------------------------------------------------------
    if part("1b") {
        println!("## 1b `meeting.brief` — `{BRIEF_INSTRUCTIONS}`, title-only candidates + none\n");
        println!(
            "Truth = the published cached row when it is among the K candidates, else `none`. `publish@0.6/0.3` = accepted picks (P ≥ 0.6 and margin ≥ 0.3 over the runner-up) that name a brief: right / wrong (wrong = a brief when the truth was `none` or another row).\n"
        );
        println!(
            "| K | ids | batches | acc | mean P(correct) | mean P(correct) when a brief is the truth | publish@0.6/0.3 right / wrong | none-accepted right / wrong | truncated | p50 / p95 ms |"
        );
        println!("|---|---|---|---|---|---|---|---|---|---|");
        for k in [4usize, 8] {
            for short in [false, true] {
                let (mut n_b, mut correct, mut p_sum, mut p_sum_brief, mut n_brief) =
                    (0usize, 0usize, 0f32, 0f32, 0usize);
                let (mut pub_right, mut pub_wrong, mut none_right, mut none_wrong, mut trunc) =
                    (0usize, 0usize, 0usize, 0usize, 0usize);
                let mut times = Vec::new();
                for b in batches {
                    let cands: Vec<&Cand> = b.candidates.iter().take(k).collect();
                    if cands.is_empty() {
                        continue;
                    }
                    n_b += 1;
                    let mut candidates: Vec<Candidate> = cands
                        .iter()
                        .enumerate()
                        .map(|(i, c)| Candidate {
                            id: if short {
                                format!("b{}", i + 1)
                            } else {
                                c.id.clone()
                            },
                            text: c.title.clone(),
                        })
                        .collect();
                    candidates.push(Candidate {
                        id: decision::engine::NONE_ID.to_owned(),
                        text: "no listed brief fits".to_owned(),
                    });
                    let q = Question::Choice {
                        instructions: BRIEF_INSTRUCTIONS.to_owned(),
                        candidates,
                        state: b.state.clone(),
                    };
                    let (out, dt, truncated) = run_one(&q)?;
                    times.push(dt);
                    trunc += usize::from(truncated);
                    let truth = b
                        .expected_brief
                        .as_ref()
                        .and_then(|e| cands.iter().position(|c| c.id == *e))
                        .unwrap_or(cands.len());
                    let pred = argmax(&out.probs);
                    correct += usize::from(pred == truth);
                    p_sum += out.probs[truth];
                    if truth < cands.len() {
                        p_sum_brief += out.probs[truth];
                        n_brief += 1;
                    }
                    let mut ranked = out.probs.clone();
                    ranked.sort_by(|a, b| b.total_cmp(a));
                    let accepted = ranked[0] >= 0.6
                        && ranked[0] - ranked.get(1).copied().unwrap_or(0.0) >= 0.3;
                    if accepted {
                        match (pred == cands.len(), pred == truth) {
                            (false, true) => pub_right += 1,
                            (false, false) => pub_wrong += 1,
                            (true, true) => none_right += 1,
                            (true, false) => none_wrong += 1,
                        }
                    }
                    if verbose {
                        println!(
                            "    K{k} {:<5} {:<16} truth {} → {} {:?}",
                            if short { "short" } else { "uuid" },
                            b.id,
                            if truth < cands.len() {
                                cands[truth].title.as_str()
                            } else {
                                "none"
                            },
                            if pred < cands.len() {
                                cands[pred].title.as_str()
                            } else {
                                "none"
                            },
                            out.probs
                                .iter()
                                .map(|p| (p * 100.0).round() / 100.0)
                                .collect::<Vec<_>>()
                        );
                    }
                }
                times.sort();
                println!(
                    "| {k} | {} | {n_b} | {correct}/{n_b} | {:.3} | {:.3} (n={n_brief}) | {pub_right} / {pub_wrong} | {none_right} / {none_wrong} | {trunc} | {:.0} / {:.0} |",
                    if short { "b1..bK" } else { "uuid" },
                    p_sum / n_b as f32,
                    if n_brief == 0 {
                        0.0
                    } else {
                        p_sum_brief / n_brief as f32
                    },
                    pct(&times, 0.5).as_secs_f64() * 1e3,
                    pct(&times, 0.95).as_secs_f64() * 1e3,
                );
            }
        }
    }
    Ok(())
}
