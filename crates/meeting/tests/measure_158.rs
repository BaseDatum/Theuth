//! 158 §13/§14 (W6): measure the later-quality gates over real meetings, read-only. Prints
//! counts only — never a transcript line, a title or a name. Run:
//! `THEUTH_MEETINGS_DIR=~/.theuth/meetings mbx test -p meeting --test measure_158 -- --ignored --nocapture`
//! (`THEUTH_SPEAKERS_DIR` optional: enrolled profiles for the suggestion gate).

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::time::Instant;

use meeting::post::echo;
use meeting::post::suggest::suggest;
use meeting::transcript::default_name;
use meeting::{Manifest, MeetingState, Profiles, Track, Transcript};

struct Row {
    id: String,
    manifest: Manifest,
    transcript: Option<Transcript>,
    dir: PathBuf,
}

/// Every meeting dir with a manifest, newest first.
fn load(dir: &Path) -> Vec<Row> {
    let mut rows: Vec<Row> = std::fs::read_dir(dir)
        .expect("meetings dir")
        .flatten()
        .filter_map(|e| {
            let dir = e.path();
            let manifest: Manifest =
                serde_json::from_slice(&std::fs::read(dir.join("manifest.json")).ok()?).ok()?;
            let transcript = std::fs::read(dir.join("transcript.json"))
                .ok()
                .and_then(|b| serde_json::from_slice(&b).ok());
            Some(Row {
                id: dir.file_name()?.to_string_lossy().chars().take(8).collect(),
                manifest,
                transcript,
                dir,
            })
        })
        .collect();
    rows.sort_by(|a, b| b.manifest.started_at.cmp(&a.manifest.started_at));
    rows
}

/// §14 "post-STT guard": runs of ≥ 3 consecutive same-track segments at ≥ 0.8 similarity.
fn repetition_runs(t: &Transcript) -> (usize, usize) {
    let mut runs = 0;
    let mut in_runs = 0;
    for track in [Track::Mic, Track::System] {
        let segs: Vec<&str> = t
            .segments
            .iter()
            .filter(|s| s.track == track)
            .map(|s| s.text.as_str())
            .collect();
        let mut len = 1;
        for w in segs.windows(2) {
            if echo::similar(w[0], w[1]) >= echo::THRESHOLD {
                len += 1;
            } else {
                if len >= 3 {
                    runs += 1;
                    in_runs += len;
                }
                len = 1;
            }
        }
        if len >= 3 {
            runs += 1;
            in_runs += len;
        }
    }
    (runs, in_runs)
}

#[test]
#[ignore = "reads real meetings from THEUTH_MEETINGS_DIR; prints counts only"]
fn measure_gates() {
    let dir = PathBuf::from(std::env::var_os("THEUTH_MEETINGS_DIR").expect("THEUTH_MEETINGS_DIR"));
    let rows = load(&dir);
    println!(
        "meetings: {} (with transcript: {})",
        rows.len(),
        rows.iter().filter(|r| r.transcript.is_some()).count()
    );

    // (1) echo ratio per meeting.
    println!(
        "\n(1) echo: id · mic_segments · echo_dupes · ratio · speakers(answer) · on-disk stats"
    );
    let mut ratios = Vec::new();
    for r in &rows {
        let Some(t) = &r.transcript else { continue };
        let s = echo::count(&t.segments);
        let ratio = if s.mic_segments == 0 {
            0.0
        } else {
            f64::from(s.echo_dupes) / f64::from(s.mic_segments)
        };
        ratios.push(ratio);
        println!(
            "  {} · {} · {} · {:.1}% · {:?} · {}",
            r.id,
            s.mic_segments,
            s.echo_dupes,
            100.0 * ratio,
            r.manifest.stats.and_then(|s| s.speakers),
            r.manifest.stats.is_some()
        );
    }
    ratios.sort_by(|a, b| a.total_cmp(b));
    if !ratios.is_empty() {
        println!(
            "  median ratio {:.1}% over {} meetings (gate: ≥ 5% over ≥ 5 speaker meetings)",
            100.0 * ratios[ratios.len() / 2],
            ratios.len()
        );
    }

    // (2) repetition runs over the last 20.
    println!(
        "\n(2) repetition runs (≥ 3 consecutive same-track segments at ≥ 0.8), last 20: id · segments · runs · segments_in_runs"
    );
    let (mut runs_all, mut in_all, mut segs_all, mut n) = (0, 0, 0, 0);
    for r in rows.iter().filter(|r| r.transcript.is_some()).take(20) {
        let t = r.transcript.as_ref().unwrap();
        let (runs, in_runs) = repetition_runs(t);
        println!("  {} · {} · {} · {}", r.id, t.segments.len(), runs, in_runs);
        runs_all += runs;
        in_all += in_runs;
        segs_all += t.segments.len();
        n += 1;
    }
    println!(
        "  total: {n} meetings · {segs_all} segments · {runs_all} runs · {in_all} segments in runs ({:.2}%)",
        if segs_all == 0 {
            0.0
        } else {
            100.0 * in_all as f64 / segs_all as f64
        }
    );

    // (3) window-read-names gate: suggestions over the last 10.
    println!(
        "\n(3) suggestions, last 10: id · labels(non-me) · already_named · suggested · unnamed_after · event"
    );
    let profiles = std::env::var_os("THEUTH_SPEAKERS_DIR")
        .map(|p| Profiles::load(Path::new(&p)).expect("speakers dir"))
        .unwrap_or_else(|| {
            Profiles::load(&std::env::temp_dir().join("theuth-158w6-empty-speakers")).unwrap()
        });
    let (mut labels_all, mut unnamed_all, mut named_all) = (0, 0, 0);
    for r in rows.iter().filter(|r| r.transcript.is_some()).take(10) {
        let t = r.transcript.as_ref().unwrap();
        let embeddings: HashMap<String, Vec<f32>> = std::fs::read(r.dir.join("embeddings.json"))
            .ok()
            .and_then(|b| serde_json::from_slice(&b).ok())
            .unwrap_or_default();
        let labels: Vec<_> = t.speakers.iter().filter(|s| s.id != "me").collect();
        let named = labels
            .iter()
            .filter(|s| s.name != default_name(&s.id))
            .count();
        let out = suggest(t, &embeddings, &profiles, r.manifest.event.as_ref());
        let unnamed = labels
            .iter()
            .filter(|s| s.name == default_name(&s.id) && !out.iter().any(|g| g.label == s.id))
            .count();
        println!(
            "  {} · {} · {} · {} · {} · {}",
            r.id,
            labels.len(),
            named,
            out.len(),
            unnamed,
            r.manifest.event.is_some()
        );
        labels_all += labels.len();
        unnamed_all += unnamed;
        named_all += named;
    }
    println!(
        "  total: {labels_all} labels · {named_all} named by hand · {unnamed_all} unnamed after suggestions ({:.0}%; gate: ≥ 20% unnamed over 10 meetings)",
        if labels_all == 0 {
            0.0
        } else {
            100.0 * unnamed_all as f64 / labels_all as f64
        }
    );

    // (4) decisions gate: Done meetings per project.
    let mut per_project: HashMap<Option<&str>, usize> = HashMap::new();
    for r in rows
        .iter()
        .filter(|r| r.manifest.state == MeetingState::Done)
    {
        *per_project
            .entry(r.manifest.project.as_deref())
            .or_default() += 1;
    }
    let max_named = per_project
        .iter()
        .filter(|(p, _)| p.is_some())
        .map(|(_, n)| *n)
        .max()
        .unwrap_or(0);
    println!(
        "\n(4) decisions: Done meetings {} · projects with Done meetings {} · max Done in one project {} · Done without project {} (gate: ≥ 10 in one project)",
        rows.iter()
            .filter(|r| r.manifest.state == MeetingState::Done)
            .count(),
        per_project.keys().filter(|p| p.is_some()).count(),
        max_named,
        per_project.get(&None).copied().unwrap_or(0)
    );

    // (5) multilingual: the manifest carries no language; `meeting.language` is one global setting.
    println!(
        "\n(5) multilingual: manifests with a language field: 0 of {} (no such field; `meeting.language` is global, default `en`)",
        rows.len()
    );

    // (6) detection poll cost: 100 ticks of the mic-holder sensor.
    let t0 = Instant::now();
    let mut holders = 0;
    let mut errs = 0;
    for _ in 0..100 {
        match meeting::detect::sensor::mic_holders() {
            Ok(h) => holders = h.len(),
            Err(_) => errs += 1,
        }
    }
    println!(
        "\n(6) sensor: 100 ticks in {:.1} ms → {:.2} ms/tick · holders now {holders} · errors {errs} (gate: the tick shows in `top`)",
        t0.elapsed().as_secs_f64() * 1e3,
        t0.elapsed().as_secs_f64() * 10.0
    );
}
