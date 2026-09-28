//! Live speaker labels on the system track (07 §2–§3): one blocking task per meeting
//! drains the system feed's diariser ring into a [`Streamer`] in the [`LIVE`]
//! geometry and `tell`s the actor [`Labels`] after every step — the speaker spans it
//! knows about the last [`KEEP_MS`], in 16 kHz sample coordinates anchored to the
//! newest ring `Instant`, and how far it has scored (the horizon). The actor maps
//! them onto the recording clock and labels the system utterances it holds back
//! (`lib.rs`, `Live::held`). Pause: nothing arrives; resume discards the stale ring
//! and **keeps** the speaker cache (speakers survive a pause). Stop: the tail is
//! scored with no look-ahead so every utterance can be labelled before the last batch.

use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::{Duration, Instant};

use kameo::actor::ActorRef;
use rtrb::Consumer;
use rubato::Resampler as _;

use crate::capture::{RATE, Sample};
use crate::post::align::Span;
use crate::post::diarize::{
    Geometry, LIVE, LIVE_INTRA_THREADS, MERGE_GAP_MS, MIN_TURN_MS, Models, RATE as MODEL_RATE,
    SPEECH, Streamer,
};
use crate::state::LiveLabels;
use crate::{Degradation, Degrade, Meeting};

/// Sleep when the ring is empty (a chunk lands every 20 ms).
const IDLE: Duration = Duration::from_millis(10);
/// Mel hop and the model's frame period.
const HOP: usize = 160;
const FRAME_MS: u64 = 10;
const SPEAKERS: usize = 8;
/// Mel frames of audio kept before the next chunk for the analysis window's left half.
const LEFT_FRAMES: usize = 2;
/// How far back the emitted spans (and the labeller's memory) reach: an utterance is
/// at most 15 s, the hold-back at most the 8 s buffer plus a step.
pub const KEEP_MS: u64 = 120_000;
/// The VAD pre-roll at the head of every utterance (`voice::vad::Gate::PREROLL`, 320 ms):
/// left out when choosing an utterance's speaker.
const PREROLL_MS: u64 = 320;
/// Least share of an utterance one label must cover to be its speaker (07 §3).
const COVER: f64 = 0.3;

/// `tell` to the actor after every step: the diariser's current view.
#[derive(Clone, Debug)]
pub struct Labels {
    /// Receipt stamp of the newest audio the step saw.
    pub at: Instant,
    /// 16 kHz samples resampled so far — the sample `at` corresponds to.
    pub position: usize,
    /// 16 kHz samples scored so far (the horizon): labels exist up to here.
    pub scored: usize,
    /// Every span of the last [`KEEP_MS`], `(start_sample, end_sample, label 1..=8)`;
    /// an open turn ends at `scored`. Replaces the actor's previous view wholesale.
    pub spans: Vec<(usize, usize, u8)>,
}

impl Labels {
    /// The view on the recording clock: `at` is `at_ms` there, and a sample `s` sits
    /// `(position − s) / 16 kHz` before it.
    pub fn view(&self, at_ms: u64) -> LiveLabels {
        let ms = |s: usize| {
            at_ms.saturating_sub(
                (self.position.saturating_sub(s) as u64 * 1000) / u64::from(MODEL_RATE),
            )
        };
        LiveLabels {
            horizon_ms: ms(self.scored),
            spans: self
                .spans
                .iter()
                .map(|&(a, b, l)| Span {
                    t0_ms: ms(a),
                    t1_ms: ms(b),
                    speaker: format!("s{l}"),
                })
                .collect(),
        }
    }
}

/// Speaker of a system utterance `[t0_ms, t1_ms]` from labelled `spans` (07 §3): the
/// label covering the most of `[t0 + pre-roll, t1]`, if that is at least [`COVER`] of
/// it; else `None` (`them:`). Overlapping speech goes to the dominant voice.
pub fn speaker_of(t0_ms: u64, t1_ms: u64, spans: &[Span]) -> Option<String> {
    let t0 = (t0_ms + PREROLL_MS).min(t1_ms);
    if t1_ms <= t0 {
        return None;
    }
    let mut cover: Vec<(&str, u64)> = Vec::new();
    for s in spans {
        let overlap = s.t1_ms.min(t1_ms).saturating_sub(s.t0_ms.max(t0));
        if overlap == 0 {
            continue;
        }
        match cover.iter_mut().find(|(l, _)| *l == s.speaker) {
            Some((_, c)) => *c += overlap,
            None => cover.push((&s.speaker, overlap)),
        }
    }
    cover
        .into_iter()
        .max_by_key(|&(_, c)| c)
        .filter(|&(_, c)| c as f64 >= COVER * (t1_ms - t0) as f64)
        .map(|(l, _)| l.to_owned())
}

/// What the task needs besides its ring (the capture's flags, as `stt::Flags`).
pub struct Flags {
    /// Capture's stop: flush the tail and exit.
    pub stop: Arc<AtomicBool>,
    /// Capture's pause: on resume discard the ring; the speaker cache is kept.
    pub paused: Arc<AtomicBool>,
}

/// Open the live session: `LIVE` geometry, [`LIVE_INTRA_THREADS`]. Blocking.
pub fn open(models: &Models) -> anyhow::Result<Streamer> {
    Streamer::open(
        &models.diarization,
        &models.data_dir,
        LIVE,
        Some(LIVE_INTRA_THREADS),
    )
}

/// Incremental `turns()` (04 §5) over a growing stream of 10 ms probabilities: per
/// channel the runs where `p ≥ SPEECH`, gaps under [`MERGE_GAP_MS`] bridged, runs under
/// [`MIN_TURN_MS`] ignored (an open run is never dropped — it may still grow). A channel
/// gets its label `1..=8` the first time one of its runs reaches [`MIN_TURN_MS`], so
/// labels are dense and, unlike `turns()`'s per-call renumbering, never change.
#[derive(Debug, Default)]
pub struct Labeller {
    /// Frames scored so far.
    frames: u64,
    channels: [Channel; SPEAKERS],
    next_label: u8,
}

#[derive(Debug, Default)]
struct Channel {
    /// Closed runs `(t0, t1)` in frames, by time; short ones included until pruned.
    runs: Vec<(u64, u64)>,
    /// The run in progress.
    open: Option<(u64, u64)>,
    label: Option<u8>,
}

impl Labeller {
    /// Feed one step's probabilities (`rows × SPEAKERS`, the frames after the last push).
    pub fn push(&mut self, probs: &[f32]) {
        let merge = MERGE_GAP_MS / FRAME_MS;
        let min = MIN_TURN_MS / FRAME_MS;
        for row in probs.chunks_exact(SPEAKERS) {
            let t = self.frames;
            for (s, &p) in row.iter().enumerate() {
                let c = &mut self.channels[s];
                match (c.open, p >= SPEECH) {
                    (Some((t0, _)), true) => c.open = Some((t0, t + 1)),
                    (None, true) => {
                        // A pause shorter than the merge gap reopens the last run.
                        c.open = match c.runs.last() {
                            Some(&(t0, t1)) if t - t1 < merge => {
                                c.runs.pop();
                                Some((t0, t + 1))
                            }
                            _ => Some((t, t + 1)),
                        };
                    }
                    (Some(run), false) => {
                        c.runs.push(run);
                        c.open = None;
                    }
                    (None, false) => {}
                }
                // The label goes to a channel the moment its first turn is long enough,
                // so numbering follows time (ties within a frame: channel order).
                if c.label.is_none() && c.open.is_some_and(|(a, b)| b - a >= min) {
                    self.next_label += 1;
                    c.label = Some(self.next_label);
                }
            }
            self.frames += 1;
        }
        // Forget what is older than the window.
        let keep = KEEP_MS / FRAME_MS;
        let cutoff = self.frames.saturating_sub(keep);
        for c in &mut self.channels {
            c.runs.retain(|&(_, t1)| t1 >= cutoff);
        }
    }

    /// Frames scored so far.
    pub fn frames(&self) -> u64 {
        self.frames
    }

    /// Labelled spans of the window, `(t0_frame, t1_frame, label)`, by start; runs under
    /// [`MIN_TURN_MS`] are left out, open runs end at the last scored frame.
    pub fn spans(&self) -> Vec<(u64, u64, u8)> {
        let min = MIN_TURN_MS / FRAME_MS;
        let mut out: Vec<(u64, u64, u8)> = Vec::new();
        for c in &self.channels {
            let Some(label) = c.label else { continue };
            out.extend(
                c.runs
                    .iter()
                    .chain(c.open.iter())
                    .filter(|&&(a, b)| b - a >= min)
                    .map(|&(a, b)| (a, b, label)),
            );
        }
        out.sort_unstable();
        out
    }
}

/// Run until stop; the final view after the flush. `sink` receives the view after every
/// step and says whether to go on (`false`: the actor is gone). Blocking, call from
/// `spawn_blocking`.
pub fn run(
    mut ring: Consumer<Sample>,
    mut streamer: Streamer,
    flags: Flags,
    mut sink: impl FnMut(Labels) -> bool,
) -> anyhow::Result<Labels> {
    let g: Geometry = streamer.geometry();
    let block = RATE as usize / 50;
    let mut resampler =
        rubato::FftFixedIn::<f32>::new(RATE as usize, MODEL_RATE as usize, block, 2, 1)?;
    let mut inbox: Vec<f32> = Vec::new();
    // 16 kHz samples from absolute sample `base` (a multiple of HOP) on.
    let mut buf: Vec<f32> = Vec::new();
    let mut base = 0usize;
    let mut position = 0usize;
    // Absolute mel frame the next step starts at.
    let mut scored_m = 0usize;
    let mut labeller = Labeller::default();
    let mut last = Instant::now();
    let mut was_paused = false;
    let mut drops = 0u32;
    let mut drop_log = Instant::now();
    let mut report = |labeller: &Labeller, at: Instant, position: usize, scored_m: usize| -> bool {
        sink(view(labeller, at, position, scored_m))
    };
    loop {
        if flags.stop.load(Ordering::SeqCst) {
            break;
        }
        let paused = flags.paused.load(Ordering::SeqCst);
        if paused != was_paused {
            was_paused = paused;
            if !paused {
                // Whatever the producer pushed before its flag flipped is stale.
                while ring.pop().is_ok() {}
                inbox.clear();
            }
        }
        let Ok((at, pcm)) = ring.pop() else {
            std::thread::sleep(IDLE);
            continue;
        };
        if paused {
            continue;
        }
        // A full ring means we fell behind: the producer dropped chunks (time shifts 20 ms each).
        if ring.slots() == 0 {
            drops += 1;
        }
        if drops > 0 && drop_log.elapsed() >= Duration::from_secs(60) {
            tracing::warn!(
                "meeting: live diarization behind real time: {drops} full-ring chunks in the last minute"
            );
            drops = 0;
            drop_log = Instant::now();
        }
        last = at;
        inbox.extend_from_slice(&pcm);
        let mut used = 0;
        while inbox.len() - used >= block {
            let out = resampler.process(&[&inbox[used..used + block]], None)?;
            buf.extend_from_slice(&out[0]);
            position += out[0].len();
            used += block;
        }
        inbox.drain(..used);
        // One step per chunk + look-ahead of audio past the scored position.
        while position >= scored_m * HOP + g.step_samples() {
            let probs = streamer.step(&buf, scored_m - base / HOP)?;
            scored_m += probs.len() / SPEAKERS;
            labeller.push(&probs);
            let new_base = scored_m.saturating_sub(LEFT_FRAMES) * HOP;
            if new_base > base {
                buf.drain(..new_base - base);
                base = new_base;
            }
            if !report(&labeller, at, position, scored_m) {
                // The actor is gone: nothing to tell; return what we have.
                return Ok(view(&labeller, at, position, scored_m));
            }
        }
    }
    // Stop: score the tail with whatever look-ahead is left, then none.
    while position / HOP > scored_m {
        let probs = streamer.step(&buf, scored_m - base / HOP)?;
        if probs.is_empty() {
            break;
        }
        scored_m += probs.len() / SPEAKERS;
        labeller.push(&probs);
    }
    Ok(view(&labeller, last, position, scored_m))
}

/// The labeller's spans in 16 kHz samples, with the horizon, anchored to `at`.
fn view(labeller: &Labeller, at: Instant, position: usize, scored_m: usize) -> Labels {
    Labels {
        at,
        position,
        scored: scored_m * HOP,
        spans: labeller
            .spans()
            .into_iter()
            .map(|(a, b, l)| (a as usize * HOP, b as usize * HOP, l))
            .collect(),
    }
}

/// Spawn the task; `Degradation::DiarizationDown` on a model error or panic. Resolves
/// to the final view after the flush at stop (`None` when it failed).
pub fn spawn(
    ring: Consumer<Sample>,
    streamer: Streamer,
    flags: Flags,
    me: ActorRef<Meeting>,
) -> tokio::task::JoinHandle<Option<Labels>> {
    tokio::spawn(async move {
        let actor = me.clone();
        let r = tokio::task::spawn_blocking(move || {
            run(ring, streamer, flags, |l| {
                me.tell(l).blocking_send().is_ok()
            })
        })
        .await;
        match r {
            Ok(Ok(labels)) => Some(labels),
            Ok(Err(e)) => {
                tracing::warn!("meeting: live diarization failed: {e:#}");
                let _ = actor.tell(Degrade(Degradation::DiarizationDown)).await;
                None
            }
            Err(e) => {
                tracing::warn!("meeting: live diarization task panicked: {e}");
                let _ = actor.tell(Degrade(Degradation::DiarizationDown)).await;
                None
            }
        }
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn frames(active: &[(usize, std::ops::Range<usize>)], n: usize) -> Vec<f32> {
        let mut p = vec![0.0f32; n * SPEAKERS];
        for &(s, ref r) in active {
            for t in r.clone() {
                p[t * SPEAKERS + s] = 0.9;
            }
        }
        p
    }

    fn span(t0_ms: u64, t1_ms: u64, speaker: &str) -> Span {
        Span {
            t0_ms,
            t1_ms,
            speaker: speaker.into(),
        }
    }

    #[test]
    fn utterance_speaker_is_the_dominant_label_past_the_preroll_or_none() {
        // 1.0–4.0 s: s1 covers 1.3–2.5 (1.2 s of the 2.68 s after the pre-roll), s2 2.5–4.0
        // (1.5 s) → s2. The pre-roll head (1.0–1.32) belonging to s3 does not count.
        let spans = [
            span(0, 1_320, "s3"),
            span(1_300, 2_500, "s1"),
            span(2_500, 4_000, "s2"),
        ];
        assert_eq!(speaker_of(1_000, 4_000, &spans).as_deref(), Some("s2"));
        // Two runs of one label add up: s1 1.3–2.0 and 3.4–4.0 (1.3 s) beat s2 2.0–3.0 (1.0 s).
        let spans = [
            span(1_300, 2_000, "s1"),
            span(2_000, 3_000, "s2"),
            span(3_400, 4_000, "s1"),
        ];
        assert_eq!(speaker_of(1_000, 4_000, &spans).as_deref(), Some("s1"));
        // Under 30 % coverage: nobody.
        assert_eq!(speaker_of(1_000, 4_000, &[span(1_500, 2_000, "s1")]), None);
        assert_eq!(speaker_of(1_000, 4_000, &[]), None);
        // A pre-roll-only utterance has no body.
        assert_eq!(speaker_of(1_000, 1_200, &[span(0, 5_000, "s1")]), None);
    }

    #[test]
    fn view_maps_samples_onto_the_recording_clock_behind_at() {
        let l = Labels {
            at: Instant::now(),
            position: 16_000 * 100,
            scored: 16_000 * 92,
            spans: vec![(16_000 * 10, 16_000 * 12, 1), (16_000 * 90, 16_000 * 92, 2)],
        };
        // `at` is 100.5 s on the recording clock: 100 s of audio ↔ 100.5 s → +500 ms offset.
        let v = l.view(100_500);
        assert_eq!(v.horizon_ms, 92_500);
        assert_eq!(
            v.spans,
            vec![span(10_500, 12_500, "s1"), span(90_500, 92_500, "s2")]
        );
    }

    #[test]
    fn labels_are_dense_by_first_turn_and_stable_across_steps() {
        let mut l = Labeller::default();
        // Step 1 (0–3 s): channel 3 speaks 0–1 s; channel 0 speaks 1.2–2 s; channel 5
        // has a 100 ms blip at 2.5 s (never a turn).
        l.push(&frames(&[(3, 0..100), (0, 120..200), (5, 250..260)], 300));
        assert_eq!(
            l.spans(),
            vec![(0, 100, 1), (120, 200, 2)],
            "channel 3 is Speaker 1 (first), channel 0 Speaker 2, the blip nothing"
        );
        // Step 2 (3–6 s): channel 0 resumes at 3.3 s after a 1.3 s gap → a new run;
        // channel 3 is heard again → keeps label 1; channel 5 finally talks → label 3.
        l.push(&frames(&[(0, 30..100), (3, 150..250), (5, 250..300)], 300));
        assert_eq!(
            l.spans(),
            vec![
                (0, 100, 1),
                (120, 200, 2),
                (330, 400, 2),
                (450, 550, 1),
                (550, 600, 3)
            ]
        );
        assert_eq!(l.frames(), 600);
    }

    #[test]
    fn short_pauses_merge_across_a_step_boundary_and_open_runs_grow() {
        let mut l = Labeller::default();
        // Channel 1 speaks to the end of step 1 (open at the boundary)…
        l.push(&frames(&[(1, 50..300)], 300));
        assert_eq!(l.spans(), vec![(50, 300, 1)]);
        // …pauses 300 ms (< 500 merge gap), then resumes: one run.
        l.push(&frames(&[(1, 30..120)], 120));
        assert_eq!(l.spans(), vec![(50, 420, 1)]);
        // A 190 ms fragment open at a boundary is below the minimum until it grows past it.
        l.push(&frames(&[(2, 181..200)], 200));
        assert_eq!(l.spans(), vec![(50, 420, 1)], "19 frames is not a turn yet");
        l.push(&frames(&[(2, 0..5)], 5));
        assert_eq!(l.spans(), vec![(50, 420, 1), (601, 625, 2)]);
    }
}
