//! Words + diarization spans → segments (04 §0.2). Pure.
//!
//! A system word joins the span containing its midpoint, else the nearest span
//! within [`NEAR_MS`], else the previous word's span (the first span when there
//! is none). Mic words are all `me`. Consecutive words of one speaker form a
//! segment; a silence longer than [`GAP_MS`] also closes one, on both tracks,
//! so a monologue is not one row; and a mic row closes where a system row
//! starts, so what the user said before and after the other side spoke stays
//! two rows in reading order even when residual far-end audio on the mic keeps
//! its words gap-free (04 §10 deviation 13).

use crate::Track;
use crate::transcript::{ME, Segment, Word};

/// Nearest-span tolerance for a word outside every span.
pub const NEAR_MS: u64 = 500;
/// Silence that closes a segment.
pub const GAP_MS: u64 = 1_000;

/// One labelled diarization span on the system track.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Span {
    /// Start.
    pub t0_ms: u64,
    /// End.
    pub t1_ms: u64,
    /// Label id (`s1`, `s2`, …).
    pub speaker: String,
}

/// Speaker of `w` per the rules above; `prev` is the previous word's speaker.
fn speaker_of<'a>(w: &Word, spans: &'a [Span], prev: Option<&'a str>) -> &'a str {
    let mid = w.t0_ms.midpoint(w.t1_ms);
    if let Some(s) = spans.iter().find(|s| s.t0_ms <= mid && mid < s.t1_ms) {
        return &s.speaker;
    }
    let nearest = spans
        .iter()
        .map(|s| {
            let d = if mid < s.t0_ms {
                s.t0_ms - mid
            } else {
                mid.saturating_sub(s.t1_ms)
            };
            (d, s)
        })
        .filter(|(d, _)| *d <= NEAR_MS)
        .min_by_key(|(d, _)| *d);
    match (nearest, prev) {
        (Some((_, s)), _) => &s.speaker,
        (None, Some(p)) => p,
        (None, None) => spans.first().map_or("s1", |s| s.speaker.as_str()),
    }
}

/// Group `words` (one track) into segments by `speaker(word, prev)` and gaps;
/// a segment also closes when a `cuts` time falls after its last word's start
/// and at or before the next word's.
fn group<'a>(
    track: Track,
    words: &[Word],
    cuts: &[u64],
    mut speaker: impl FnMut(&Word, Option<&'a str>) -> &'a str,
) -> Vec<Segment> {
    let mut out: Vec<Segment> = Vec::new();
    let mut prev: Option<&'a str> = None;
    let mut prev_t0 = 0;
    for w in words {
        let sp = speaker(w, prev);
        prev = Some(sp);
        let cut = cuts.iter().any(|&c| prev_t0 < c && c <= w.t0_ms);
        prev_t0 = w.t0_ms;
        let open = out.last_mut().filter(|s| {
            s.speaker == sp && !cut && w.t0_ms.saturating_sub(s.t1_ms) <= GAP_MS
        });
        match open {
            Some(s) => {
                s.t1_ms = s.t1_ms.max(w.t1_ms);
                s.text.push(' ');
                s.text.push_str(&w.text);
                s.words.push(w.clone());
            }
            None => out.push(Segment {
                id: 0,
                track,
                speaker: sp.to_owned(),
                t0_ms: w.t0_ms,
                t1_ms: w.t1_ms,
                text: w.text.clone(),
                words: vec![w.clone()],
            }),
        }
    }
    out
}

/// Segments of both tracks, sorted by `t0_ms`, ids `0..n`.
pub fn segments(mic: &[Word], system: &[Word], spans: &[Span]) -> Vec<Segment> {
    let mut all = group(Track::System, system, &[], |w, prev| {
        speaker_of(w, spans, prev)
    });
    let cuts: Vec<u64> = all.iter().map(|s| s.t0_ms).collect();
    all.extend(group(Track::Mic, mic, &cuts, |_, _| ME));
    all.sort_by_key(|s| (s.t0_ms, s.track == Track::System));
    for (i, s) in all.iter_mut().enumerate() {
        s.id = i as u32;
    }
    all
}

#[cfg(test)]
mod tests {
    use super::*;

    fn w(text: &str, t0: u64, t1: u64) -> Word {
        Word {
            text: text.into(),
            t0_ms: t0,
            t1_ms: t1,
        }
    }

    fn span(t0: u64, t1: u64, sp: &str) -> Span {
        Span {
            t0_ms: t0,
            t1_ms: t1,
            speaker: sp.into(),
        }
    }

    fn texts(segs: &[Segment]) -> Vec<(String, String)> {
        segs.iter().map(|s| (s.speaker.clone(), s.text.clone())).collect()
    }

    #[test]
    fn midpoint_inside_a_span_wins() {
        let spans = [span(0, 2_000, "s1"), span(2_000, 4_000, "s2")];
        // "there" straddles the boundary (1 800–2 400): midpoint 2 100 → s2.
        let sys = [w("Hello", 0, 500), w("there", 1_800, 2_400), w("Bob", 2_500, 3_000)];
        let segs = segments(&[], &sys, &spans);
        assert_eq!(
            texts(&segs),
            [("s1".into(), "Hello".into()), ("s2".into(), "there Bob".into())]
        );
        assert_eq!((segs[1].t0_ms, segs[1].t1_ms), (1_800, 3_000));
    }

    #[test]
    fn outside_every_span_takes_the_nearest_within_500ms() {
        let spans = [span(0, 1_000, "s1"), span(5_000, 6_000, "s2")];
        // midpoint 1 300: 300 ms after s1, 3 700 before s2 → s1.
        // midpoint 4 600: 400 ms before s2 → s2.
        let sys = [w("a", 1_200, 1_400), w("b", 4_500, 4_700)];
        let segs = segments(&[], &sys, &spans);
        assert_eq!(texts(&segs), [("s1".into(), "a".into()), ("s2".into(), "b".into())]);
    }

    #[test]
    fn beyond_500ms_falls_back_to_the_previous_word() {
        let spans = [span(0, 1_000, "s1"), span(9_000, 10_000, "s2")];
        // 4 000–4 400: midpoint 4 200, 3 200 ms from either span → the previous word's s1.
        let sys = [w("a", 500, 900), w("b", 4_000, 4_400)];
        let segs = segments(&[], &sys, &spans);
        assert_eq!(segs.len(), 2, "same speaker, but a > 1 s gap closes the segment");
        assert!(segs.iter().all(|s| s.speaker == "s1"));
        // No previous word and no near span: the first span's speaker; no spans at all: s1.
        assert_eq!(segments(&[], &[w("x", 4_000, 4_400)], &spans)[0].speaker, "s1");
        assert_eq!(segments(&[], &[w("x", 4_000, 4_400)], &[])[0].speaker, "s1");
    }

    #[test]
    fn mic_splits_on_sentence_gaps_over_a_second() {
        let mic = [
            w("Hi", 0, 300),
            w("there.", 350, 800),
            w("Second", 1_900, 2_300), // 1 100 ms after → new segment
            w("one.", 2_350, 2_700),
            w("Third", 3_700, 4_000), // exactly 1 000 → same segment
        ];
        let segs = segments(&mic, &[], &[]);
        assert_eq!(
            texts(&segs),
            [
                (ME.into(), "Hi there.".into()),
                (ME.into(), "Second one. Third".into())
            ]
        );
        assert!(segs.iter().all(|s| s.track == Track::Mic));
    }

    #[test]
    fn both_tracks_interleave_by_start_time_with_sequential_ids() {
        let spans = [span(0, 10_000, "s1")];
        let mic = [w("me", 2_000, 2_500)];
        let sys = [w("before", 0, 500), w("after", 4_000, 4_500)];
        let segs = segments(&mic, &sys, &spans);
        assert_eq!(
            texts(&segs),
            [
                ("s1".into(), "before".into()),
                (ME.into(), "me".into()),
                ("s1".into(), "after".into())
            ]
        );
        assert_eq!(segs.iter().map(|s| s.id).collect::<Vec<_>>(), [0, 1, 2]);
    }

    #[test]
    fn mic_row_closes_where_a_system_row_starts() {
        let spans = [span(0, 10_000, "s1")];
        // Gap-free mic words (far-end residual) across a system row at 2 000.
        let mic = [w("first", 0, 900), w("thing", 1_000, 1_900), w("second", 2_100, 2_900), w("thing", 3_000, 3_900)];
        let sys = [w("theirs", 2_000, 2_800)];
        let segs = segments(&mic, &sys, &spans);
        assert_eq!(
            texts(&segs),
            [
                (ME.into(), "first thing".into()),
                ("s1".into(), "theirs".into()),
                (ME.into(), "second thing".into())
            ]
        );
    }
}
