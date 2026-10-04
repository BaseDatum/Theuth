//! Pointer grammar (design doc 96 §6.2): a pure function from the sampled
//! pointer path to the gestures it drew — `Loop`, `Stroke`, `Dwell`, `End` —
//! ordered by precedence. Points are logical global (origin top-left of the
//! primary display), as the shell samples them.

use serde::{Deserialize, Serialize};

/// One pointer sample.
#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize, schemars::JsonSchema)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS), ts(export))]
pub struct Sample {
    /// Milliseconds since the key press.
    pub t_ms: u32,
    /// Logical global x.
    pub x: f32,
    /// Logical global y.
    pub y: f32,
}

/// A logical point.
#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
pub struct Pt {
    /// x.
    pub x: f32,
    /// y.
    pub y: f32,
}

/// An axis-aligned box in logical points.
#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
pub struct Bbox {
    /// Left.
    pub x: f32,
    /// Top.
    pub y: f32,
    /// Width.
    pub w: f32,
    /// Height.
    pub h: f32,
}

/// What the path drew (96 §0.9); precedence for "this": Loop > Stroke > Dwell > End.
#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
pub enum Gesture {
    /// The path closed on itself: what it encloses.
    Loop {
        /// Bounds of the closed portion.
        bbox: Bbox,
        /// Mean of the closed portion's samples.
        centroid: Pt,
    },
    /// A straight run: what it underlines or points along.
    Stroke {
        /// Start.
        from: Pt,
        /// End.
        to: Pt,
    },
    /// The pointer rested.
    Dwell {
        /// Mean of the resting samples.
        at: Pt,
        /// How long it rested.
        ms: u32,
    },
    /// The last sample; always present.
    End {
        /// Where the key was released.
        at: Pt,
    },
}

/// A dwell stays within this radius of its first sample.
pub const DWELL_RADIUS_PT: f32 = 12.0;
/// A dwell lasts at least this long.
pub const DWELL_MIN_MS: u32 = 500;
/// A loop closes when the path returns within this distance of an earlier sample.
pub const LOOP_CLOSE_PT: f32 = 24.0;
/// That earlier sample must be at least this much older.
pub const LOOP_MIN_AGE_MS: u32 = 300;
/// A loop's bbox is at least this on both sides.
pub const LOOP_MIN_PT: f32 = 20.0;
/// A stroke's chord is at least this long.
pub const STROKE_MIN_PT: f32 = 80.0;
/// Every point of a stroke lies within this distance of its chord.
pub const STROKE_MAX_DEV_PT: f32 = 15.0;

/// The gestures in `samples`, ordered Loop, Stroke, Dwell, End; empty input
/// yields nothing, otherwise `End` is always last.
pub fn gestures(samples: &[Sample]) -> Vec<Gesture> {
    let Some(last) = samples.last() else {
        return Vec::new();
    };
    let mut out = Vec::new();
    let mut covered = vec![false; samples.len()];
    for (i, j, g) in loops(samples) {
        covered[i..=j].iter_mut().for_each(|c| *c = true);
        out.push(g);
    }
    out.extend(strokes(samples, &covered));
    out.extend(dwells(samples));
    out.push(Gesture::End { at: pt(last) });
    out
}

/// Where a gesture is for hit-testing and display assignment: the loop's
/// centroid, the stroke's midpoint, the dwell's and the end's point.
#[must_use]
pub fn anchor(g: &Gesture) -> Pt {
    match *g {
        Gesture::Loop { centroid, .. } => centroid,
        Gesture::Stroke { from, to } => Pt {
            x: (from.x + to.x) / 2.0,
            y: (from.y + to.y) / 2.0,
        },
        Gesture::Dwell { at, .. } | Gesture::End { at } => at,
    }
}

fn pt(s: &Sample) -> Pt {
    Pt { x: s.x, y: s.y }
}

fn dist(a: Pt, b: Pt) -> f32 {
    ((a.x - b.x).powi(2) + (a.y - b.y).powi(2)).sqrt()
}

fn bbox_of(samples: &[Sample]) -> Bbox {
    let (mut x0, mut y0, mut x1, mut y1) = (f32::MAX, f32::MAX, f32::MIN, f32::MIN);
    for s in samples {
        x0 = x0.min(s.x);
        y0 = y0.min(s.y);
        x1 = x1.max(s.x);
        y1 = y1.max(s.y);
    }
    Bbox {
        x: x0,
        y: y0,
        w: x1 - x0,
        h: y1 - y0,
    }
}

fn mean(samples: &[Sample]) -> Pt {
    let n = samples.len() as f32;
    Pt {
        x: samples.iter().map(|s| s.x).sum::<f32>() / n,
        y: samples.iter().map(|s| s.y).sum::<f32>() / n,
    }
}

/// Proper crossing of segments `a1-a2` and `b1-b2`.
fn segments_cross(a1: Pt, a2: Pt, b1: Pt, b2: Pt) -> bool {
    fn orient(p: Pt, q: Pt, r: Pt) -> f32 {
        (q.x - p.x) * (r.y - p.y) - (q.y - p.y) * (r.x - p.x)
    }
    orient(b1, b2, a1) * orient(b1, b2, a2) < 0.0 && orient(a1, a2, b1) * orient(a1, a2, b2) < 0.0
}

fn iou(a: &Bbox, b: &Bbox) -> f32 {
    let w = (a.x + a.w).min(b.x + b.w) - a.x.max(b.x);
    let h = (a.y + a.h).min(b.y + b.h) - a.y.max(b.y);
    if w <= 0.0 || h <= 0.0 {
        return 0.0;
    }
    let inter = w * h;
    inter / (a.w * a.h + b.w * b.h - inter)
}

/// Closed loops as `(first index, last index, gesture)`; the walk resumes after each.
// ponytail: O(n²) over the path (a few thousand samples at 60 Hz); bucket the
// samples spatially if holds ever run minutes.
fn loops(samples: &[Sample]) -> Vec<(usize, usize, Gesture)> {
    let mut out: Vec<(usize, usize, Gesture)> = Vec::new();
    let mut start = 0;
    let mut j = 1;
    while j < samples.len() {
        let pj = pt(&samples[j]);
        let returns = (start..j).find(|&i| {
            samples[j].t_ms.saturating_sub(samples[i].t_ms) >= LOOP_MIN_AGE_MS
                && dist(pt(&samples[i]), pj) <= LOOP_CLOSE_PT
        });
        let crosses = || {
            let pj1 = pt(&samples[j - 1]);
            (start..j.saturating_sub(2))
                .find(|&k| segments_cross(pj1, pj, pt(&samples[k]), pt(&samples[k + 1])))
        };
        if let Some(i) = returns.or_else(crosses) {
            let bbox = bbox_of(&samples[i..=j]);
            if bbox.w >= LOOP_MIN_PT && bbox.h >= LOOP_MIN_PT {
                let dup = out.iter().any(|(_, _, o)| match o {
                    Gesture::Loop { bbox: b, .. } => iou(b, &bbox) > 0.5,
                    _ => false,
                });
                if !dup {
                    let centroid = mean(&samples[i..=j]);
                    out.push((i, j, Gesture::Loop { bbox, centroid }));
                }
                start = j + 1;
                j = start + 1;
                continue;
            }
        }
        j += 1;
    }
    out
}

/// Distance from `p` to the line through `a` and `b` (to `a` when they coincide).
fn deviation(p: Pt, a: Pt, b: Pt) -> f32 {
    let len = dist(a, b);
    if len == 0.0 {
        return dist(p, a);
    }
    ((b.x - a.x) * (a.y - p.y) - (a.x - p.x) * (b.y - a.y)).abs() / len
}

/// Straight runs outside loops: extend from `a` while every point stays near
/// the chord and the chord keeps growing; a run ≥ `STROKE_MIN_PT` is a stroke.
fn strokes(samples: &[Sample], covered: &[bool]) -> Vec<Gesture> {
    let mut out = Vec::new();
    let mut a = 0;
    while a + 1 < samples.len() {
        if covered[a] {
            a += 1;
            continue;
        }
        let pa = pt(&samples[a]);
        let (mut far, mut far_len) = (a, 0.0f32);
        let mut b = a + 1;
        while b < samples.len() && !covered[b] {
            let pb = pt(&samples[b]);
            let len = dist(pa, pb);
            if len + STROKE_MAX_DEV_PT < far_len
                || samples[a..=b]
                    .iter()
                    .any(|s| deviation(pt(s), pa, pb) > STROKE_MAX_DEV_PT)
            {
                break;
            }
            if len > far_len {
                (far, far_len) = (b, len);
            }
            b += 1;
        }
        if far_len >= STROKE_MIN_PT {
            out.push(Gesture::Stroke {
                from: pa,
                to: pt(&samples[far]),
            });
        }
        a = far.max(a + 1);
    }
    out
}

/// Maximal runs within `DWELL_RADIUS_PT` of their first sample spanning
/// ≥ `DWELL_MIN_MS`. A path that never moved is `End` alone.
fn dwells(samples: &[Sample]) -> Vec<Gesture> {
    let mut out: Vec<Gesture> = Vec::new();
    let first = pt(&samples[0]);
    if samples.iter().all(|s| pt(s) == first) {
        return out;
    }
    let mut a = 0;
    while a < samples.len() {
        let pa = pt(&samples[a]);
        let b = (a..samples.len())
            .take_while(|&k| dist(pt(&samples[k]), pa) <= DWELL_RADIUS_PT)
            .last()
            .unwrap_or(a);
        let ms = samples[b].t_ms.saturating_sub(samples[a].t_ms);
        if ms >= DWELL_MIN_MS {
            let at = mean(&samples[a..=b]);
            let dup = out.iter().any(|g| match g {
                Gesture::Dwell { at: o, .. } => dist(*o, at) <= DWELL_RADIUS_PT,
                _ => false,
            });
            if !dup {
                out.push(Gesture::Dwell { at, ms });
            }
            a = b + 1;
        } else {
            a += 1;
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    const STEP_MS: u32 = 1000 / 60;

    fn path(points: impl IntoIterator<Item = (f32, f32)>) -> Vec<Sample> {
        points
            .into_iter()
            .enumerate()
            .map(|(i, (x, y))| Sample {
                t_ms: i as u32 * STEP_MS,
                x,
                y,
            })
            .collect()
    }

    /// `n` samples on a circle of radius `r` around `c`, from angle 0 over `turns` turns.
    fn circle(c: (f32, f32), r: f32, n: usize, turns: f32) -> Vec<(f32, f32)> {
        (0..n)
            .map(|i| {
                let a = std::f32::consts::TAU * turns * i as f32 / n as f32;
                (c.0 + r * a.cos(), c.1 + r * a.sin())
            })
            .collect()
    }

    fn line(from: (f32, f32), to: (f32, f32), n: usize) -> Vec<(f32, f32)> {
        (0..n)
            .map(|i| {
                let t = i as f32 / (n - 1) as f32;
                (from.0 + (to.0 - from.0) * t, from.1 + (to.1 - from.1) * t)
            })
            .collect()
    }

    /// `n` samples around `at` with deterministic ±3 pt jitter.
    fn hold(at: (f32, f32), n: usize) -> Vec<(f32, f32)> {
        (0..n)
            .map(|i| {
                let j = (i % 7) as f32 - 3.0;
                (at.0 + j, at.1 - j * 0.5)
            })
            .collect()
    }

    fn near(p: Pt, x: f32, y: f32) -> bool {
        (p.x - x).abs() <= 10.0 && (p.y - y).abs() <= 10.0
    }

    fn kinds(gs: &[Gesture]) -> Vec<&'static str> {
        gs.iter()
            .map(|g| match g {
                Gesture::Loop { .. } => "loop",
                Gesture::Stroke { .. } => "stroke",
                Gesture::Dwell { .. } => "dwell",
                Gesture::End { .. } => "end",
            })
            .collect()
    }

    #[test]
    fn circle_is_one_loop() {
        let gs = gestures(&path(circle((300.0, 300.0), 60.0, 72, 1.0)));
        assert_eq!(kinds(&gs), ["loop", "end"]);
        let Gesture::Loop { bbox, centroid } = gs[0] else {
            unreachable!()
        };
        assert!(
            (bbox.x - 240.0).abs() <= 5.0 && (bbox.y - 240.0).abs() <= 5.0,
            "{bbox:?}"
        );
        assert!(
            (bbox.w - 120.0).abs() <= 8.0 && (bbox.h - 120.0).abs() <= 8.0,
            "{bbox:?}"
        );
        assert!(near(centroid, 300.0, 300.0), "{centroid:?}");
    }

    #[test]
    fn underline_is_a_stroke() {
        let gs = gestures(&path(line((100.0, 500.0), (400.0, 500.0), 30)));
        assert_eq!(kinds(&gs), ["stroke", "end"]);
        let Gesture::Stroke { from, to } = gs[0] else {
            unreachable!()
        };
        assert!(near(from, 100.0, 500.0) && near(to, 400.0, 500.0), "{gs:?}");
    }

    #[test]
    fn jittery_hold_is_a_dwell() {
        let gs = gestures(&path(hold((200.0, 200.0), 43)));
        assert_eq!(kinds(&gs), ["dwell", "end"]);
        let Gesture::Dwell { at, ms } = gs[0] else {
            unreachable!()
        };
        assert!(ms >= DWELL_MIN_MS && near(at, 200.0, 200.0), "{gs:?}");
    }

    #[test]
    fn short_dash_is_not_a_stroke() {
        let gs = gestures(&path(line((0.0, 0.0), (50.0, 0.0), 10)));
        assert_eq!(kinds(&gs), ["end"]);
    }

    #[test]
    fn open_arc_is_not_a_loop() {
        let gs = gestures(&path(circle((300.0, 300.0), 60.0, 40, 0.6)));
        assert!(!kinds(&gs).contains(&"loop"), "{gs:?}");
    }

    #[test]
    fn no_movement_is_end_only() {
        let gs = gestures(&path(std::iter::repeat_n((50.0, 50.0), 60)));
        assert_eq!(kinds(&gs), ["end"]);
        assert_eq!(
            gs[0],
            Gesture::End {
                at: Pt { x: 50.0, y: 50.0 }
            }
        );
    }

    #[test]
    fn circle_then_hold_orders_loop_dwell_end() {
        let mut pts = circle((300.0, 300.0), 60.0, 72, 1.0);
        pts.extend(hold((360.0, 300.0), 45));
        let gs = gestures(&path(pts));
        assert_eq!(kinds(&gs), ["loop", "dwell", "end"], "{gs:?}");
    }

    #[test]
    fn two_samples_are_end_only() {
        let gs = gestures(&path([(1.0, 1.0), (30.0, 40.0)]));
        assert_eq!(
            gs,
            [Gesture::End {
                at: Pt { x: 30.0, y: 40.0 }
            }]
        );
    }

    #[test]
    fn empty_is_nothing() {
        assert!(gestures(&[]).is_empty());
    }

    #[test]
    fn two_lobes_are_two_loops() {
        // A right lobe, then a left lobe that begins where the first one passed.
        let mut pts = circle((360.0, 300.0), 60.0, 72, 1.0);
        pts.extend(circle((240.0, 300.0), 60.0, 72, 1.0));
        let gs = gestures(&path(pts));
        assert_eq!(
            kinds(&gs).iter().filter(|k| **k == "loop").count(),
            2,
            "{gs:?}"
        );
    }

    #[test]
    fn stroke_ending_in_a_dwell_reports_both() {
        let mut pts = line((100.0, 500.0), (400.0, 500.0), 30);
        pts.extend(hold((400.0, 500.0), 45));
        let gs = gestures(&path(pts));
        assert_eq!(kinds(&gs), ["stroke", "dwell", "end"], "{gs:?}");
    }

    #[test]
    fn scribble_is_end_only() {
        // A zigzag that never closes, never runs straight for 80 pt, never rests.
        let pts: Vec<_> = (0..50u32)
            .map(|i| {
                let y = if i.is_multiple_of(2) { 100.0 } else { 160.0 };
                (100.0 + i as f32 * 9.0, y)
            })
            .collect();
        let gs = gestures(&path(pts));
        assert_eq!(kinds(&gs), ["end"], "{gs:?}");
    }
}
