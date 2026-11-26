//! Placement (contract §3.1): where a new card goes. Pure. Nothing existing moves and no
//! auto-layout ever runs; only a frame may grow to take a card.

use super::model::{CardKind, CardState, Frame, Snapshot};
use super::outline::rows;

/// Gap between cards and the scan step.
pub const GAP: f64 = 24.0;
/// Frame padding around its cards.
pub const PAD: f64 = 32.0;
/// Frame title band above the top padding.
pub const BAND: f64 = 40.0;
/// A frame row grows to the right until the frame would be this wide, then wraps.
pub const ROW_MAX: f64 = 1600.0;
/// Clearance kept from other cards and ink while scanning.
const CLEAR: f64 = 8.0;
/// Scan steps before giving up on a direction.
const STEPS: usize = 4000;

/// Default size of a card of `kind`; `device` (or the frame's preset) sizes a `page`.
pub fn default_size(kind: CardKind, device: Option<&str>) -> (f64, f64) {
    match kind {
        CardKind::Page => match device {
            Some("phone") => (180.0, 390.0),
            Some("tablet") => (300.0, 432.0),
            Some("desktop") => (420.0, 263.0),
            _ => (360.0, 270.0),
        },
        CardKind::Sticky => (220.0, 150.0),
        CardKind::Diagram => (360.0, 260.0),
        CardKind::Excerpt | CardKind::Diff | CardKind::Terminal => (420.0, 300.0),
        CardKind::Image | CardKind::Web | CardKind::Note | CardKind::Session => (320.0, 240.0),
        CardKind::View => (560.0, 400.0),
    }
}

/// Where to put a card.
#[derive(Clone, Debug, Default)]
pub struct Want<'a> {
    /// Size.
    pub w: f64,
    /// Size.
    pub h: f64,
    /// Next to this card (ULID).
    pub near: Option<&'a str>,
    /// `right` (default) or `below` first.
    pub below_first: bool,
    /// Inside this frame (ULID).
    pub frame: Option<&'a str>,
}

/// The answer.
#[derive(Clone, Debug, PartialEq)]
pub struct Spot {
    /// Top-left x.
    pub x: f64,
    /// Top-left y.
    pub y: f64,
    /// The frame's new box when it had to grow.
    pub grow: Option<[f64; 4]>,
}

fn overlaps(a: [f64; 4], b: [f64; 4]) -> bool {
    a[0] < b[0] + b[2] && b[0] < a[0] + a[2] && a[1] < b[1] + b[3] && b[1] < a[1] + a[3]
}

/// What a new card must not touch: live and dimmed cards (placeholders too) and free strokes.
pub fn obstacles(s: &Snapshot) -> Vec<[f64; 4]> {
    let mut out: Vec<[f64; 4]> = s
        .cards
        .iter()
        .filter(|c| c.state != CardState::Archived)
        .map(|c| c.rect())
        .collect();
    out.extend(s.strokes.iter().filter(|k| k.card.is_none()).map(|k| k.bb));
    out
}

fn free(b: [f64; 4], obs: &[[f64; 4]]) -> bool {
    let grown = [b[0] - CLEAR, b[1] - CLEAR, b[2] + 2.0 * CLEAR, b[3] + 2.0 * CLEAR];
    !obs.iter().any(|o| overlaps(grown, *o))
}

/// Scan from `(x, y)` in 24 px steps along `(dx, dy)` until the box is free.
fn scan(x: f64, y: f64, w: f64, h: f64, d: (f64, f64), obs: &[[f64; 4]]) -> Option<(f64, f64)> {
    (0..STEPS)
        .map(|k| (x + d.0 * GAP * k as f64, y + d.1 * GAP * k as f64))
        .find(|&(x, y)| free([x, y, w, h], obs))
}

/// Right of everything: always free.
fn beyond(y: f64, obs: &[[f64; 4]]) -> (f64, f64) {
    let right = obs.iter().map(|o| o[0] + o[2]).fold(f64::MIN, f64::max);
    (if right == f64::MIN { 0.0 } else { right + GAP + CLEAR }, y)
}

/// Next to `anchor`: to its right (gap 24) then below it, alternating outward in 24 px steps.
fn beside(a: [f64; 4], w: f64, h: f64, below_first: bool, obs: &[[f64; 4]]) -> (f64, f64) {
    for k in 0..STEPS {
        let off = GAP * k as f64;
        let right = (a[0] + a[2] + GAP + off, a[1]);
        let below = (a[0], a[1] + a[3] + GAP + off);
        let order = if below_first { [below, right] } else { [right, below] };
        if let Some(p) = order.into_iter().find(|&(x, y)| free([x, y, w, h], obs)) {
            return p;
        }
    }
    beyond(a[1], obs)
}

/// Where a card of `want` goes on `s`.
pub fn place(s: &Snapshot, want: &Want) -> Spot {
    let obs = obstacles(s);
    let (w, h) = (want.w, want.h);
    if let Some(f) = want.frame.and_then(|f| s.frame(f)) {
        return in_frame(s, f, w, h, &obs);
    }
    if let Some(a) = want.near.and_then(|n| s.card(n)) {
        let (x, y) = beside(a.rect(), w, h, want.below_first, &obs);
        return Spot { x, y, grow: None };
    }
    // Right of the right-most card of the bottom row of loose cards.
    let loose: Vec<(String, [f64; 4])> = s
        .cards
        .iter()
        .filter(|c| c.state != CardState::Archived && c.frame.as_deref().and_then(|f| s.frame(f)).is_none())
        .map(|c| (c.id.clone(), c.rect()))
        .collect();
    let (x, y) = match rows(&loose).last().and_then(|r| r.last()).and_then(|id| s.card(id)) {
        Some(c) => beside(c.rect(), w, h, false, &obs),
        None if obs.is_empty() => (0.0, 0.0),
        None => scan(0.0, 0.0, w, h, (1.0, 0.0), &obs).unwrap_or_else(|| beyond(0.0, &obs)),
    };
    Spot { x, y, grow: None }
}

fn in_frame(s: &Snapshot, f: &Frame, w: f64, h: f64, obs: &[[f64; 4]]) -> Spot {
    let members: Vec<(String, [f64; 4])> = s
        .cards
        .iter()
        .filter(|c| c.state != CardState::Archived && c.frame.as_deref() == Some(f.id.as_str()))
        .map(|c| (c.id.clone(), c.rect()))
        .collect();
    let left = f.x + PAD;
    let limit = f.x + f.w.max(ROW_MAX) - PAD;
    let (mut x, mut y) = (left, f.y + BAND + PAD);
    if let Some(last) = rows(&members).last() {
        let boxes: Vec<[f64; 4]> = last.iter().filter_map(|id| s.card(id)).map(|c| c.rect()).collect();
        let right = boxes.iter().map(|b| b[0] + b[2]).fold(f64::MIN, f64::max);
        let top = boxes.iter().map(|b| b[1]).fold(f64::MAX, f64::min);
        let bottom = members.iter().map(|(_, b)| b[1] + b[3]).fold(f64::MIN, f64::max);
        (x, y) = (right + GAP, top);
        if x + w > limit || scan_row(x, y, w, h, limit, obs).is_none() {
            (x, y) = (left, bottom + GAP);
        }
    }
    // Along the row, then row after row, until a free slot.
    let (x, y) = loop {
        if let Some(px) = scan_row(x, y, w, h, limit, obs) {
            break (px, y);
        }
        (x, y) = (left, y + GAP);
        if y > f.y + 100_000.0 {
            break beyond(y, obs);
        }
    };
    let need = [
        f.x.min(x - PAD),
        f.y.min(y - PAD - BAND),
        0.0,
        0.0,
    ];
    let right = (f.x + f.w).max(x + w + PAD);
    let bottom = (f.y + f.h).max(y + h + PAD);
    let grown = [need[0], need[1], right - need[0], bottom - need[1]];
    let grow = (grown != f.rect()).then_some(grown);
    Spot { x, y, grow }
}

fn scan_row(x: f64, y: f64, w: f64, h: f64, limit: f64, obs: &[[f64; 4]]) -> Option<f64> {
    let mut px = x;
    while px + w <= limit {
        if free([px, y, w, h], obs) {
            return Some(px);
        }
        px += GAP;
    }
    None
}

/// The box of a frame around `cards` (32 px padding, a 40 px title band on top).
pub fn frame_around(cards: &[[f64; 4]]) -> Option<[f64; 4]> {
    let x0 = cards.iter().map(|b| b[0]).fold(f64::MAX, f64::min);
    let y0 = cards.iter().map(|b| b[1]).fold(f64::MAX, f64::min);
    let x1 = cards.iter().map(|b| b[0] + b[2]).fold(f64::MIN, f64::max);
    let y1 = cards.iter().map(|b| b[1] + b[3]).fold(f64::MIN, f64::max);
    (!cards.is_empty()).then_some([x0 - PAD, y0 - PAD - BAND, x1 - x0 + 2.0 * PAD, y1 - y0 + 2.0 * PAD + BAND])
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::wb::model::{Snapshot, StrokeInfo, tests::card};

    struct Rng(u64);
    impl Rng {
        fn next(&mut self) -> u64 {
            self.0 ^= self.0 << 13;
            self.0 ^= self.0 >> 7;
            self.0 ^= self.0 << 17;
            self.0
        }
        fn range(&mut self, lo: f64, hi: f64) -> f64 {
            lo + (self.next() % 1_000_000) as f64 / 1_000_000.0 * (hi - lo)
        }
    }

    const KINDS: [CardKind; 11] = [
        CardKind::Page,
        CardKind::Diagram,
        CardKind::Note,
        CardKind::Excerpt,
        CardKind::Diff,
        CardKind::Terminal,
        CardKind::Web,
        CardKind::Image,
        CardKind::Sticky,
        CardKind::Session,
        CardKind::View,
    ];

    #[test]
    fn never_overlaps_cards_or_ink() {
        let mut rng = Rng(0x2180_2180_dead_beef);
        for round in 0..500 {
            let mut s = Snapshot::default();
            let n = (rng.next() % 30) as usize;
            for i in 0..n {
                let kind = KINDS[(rng.next() % 10) as usize];
                let (w, h) = default_size(kind, Some("phone"));
                let mut c = card(&format!("C{round:03}{i:03}"), kind, [rng.range(-800.0, 2400.0), rng.range(-600.0, 1800.0), w, h], "t", vec![]);
                if rng.next().is_multiple_of(7) {
                    c.state = CardState::Archived;
                }
                s.cards.push(c);
            }
            for i in 0..(rng.next() % 20) {
                let anchored = rng.next().is_multiple_of(3);
                s.strokes.push(StrokeInfo {
                    id: format!("S{i:03}"),
                    n: 2,
                    bytes: 12,
                    bb: [rng.range(-800.0, 2400.0), rng.range(-600.0, 1800.0), rng.range(2.0, 400.0), rng.range(2.0, 300.0)],
                    card: anchored.then(|| "C000000".to_owned()),
                    ver: None,
                    at: 0,
                });
            }
            s.cards.sort_by(|a, b| a.id.cmp(&b.id));
            let kind = KINDS[(rng.next() % 10) as usize];
            let (w, h) = default_size(kind, None);
            let near = (n > 0 && rng.next().is_multiple_of(2)).then(|| s.cards[(rng.next() as usize) % n].id.clone());
            let spot = place(&s, &Want { w, h, near: near.as_deref(), below_first: rng.next().is_multiple_of(2), frame: None });
            let b = [spot.x, spot.y, w, h];
            for o in obstacles(&s) {
                assert!(!overlaps(b, o), "round {round}: {b:?} overlaps {o:?}");
            }
        }
    }

    #[test]
    fn fills_frame_row_then_wraps() {
        let mut s = Snapshot::default();
        s.frames.push(Frame {
            id: "F".into(),
            title: "Variants".into(),
            x: 0.0,
            y: 0.0,
            w: 300.0,
            h: 500.0,
            colour: "neutral".into(),
            preset: Some("phone".into()),
            by: "owner".into(),
        });
        let (w, h) = default_size(CardKind::Page, Some("phone"));
        let mut placed = Vec::new();
        for i in 0..9 {
            let spot = place(&s, &Want { w, h, near: None, below_first: false, frame: Some("F") });
            if let Some(g) = spot.grow {
                let f = &mut s.frames[0];
                (f.x, f.y, f.w, f.h) = (g[0], g[1], g[2], g[3]);
            }
            let mut c = card(&format!("C{i}"), CardKind::Page, [spot.x, spot.y, w, h], "v", vec![]);
            c.frame = Some("F".into());
            s.cards.push(c);
            s.cards.sort_by(|a, b| a.id.cmp(&b.id));
            placed.push((spot.x, spot.y));
        }
        // First slot under the title band; then one row, left to right, 24 px apart.
        assert_eq!(placed[0], (PAD, BAND + PAD));
        assert_eq!(placed[1], (PAD + w + GAP, BAND + PAD));
        let per_row = placed.iter().filter(|p| p.1 == BAND + PAD).count();
        assert!((3..9).contains(&per_row), "{per_row} in the first row");
        // The next card wraps to a new row at the left edge, under the first row.
        assert_eq!(placed[per_row], (PAD, BAND + PAD + h + GAP));
        // Every card sits inside the (grown) frame.
        let f = &s.frames[0];
        for c in &s.cards {
            assert!(c.x >= f.x + PAD && c.x + c.w <= f.x + f.w - PAD + 0.001);
            assert!(c.y >= f.y + BAND && c.y + c.h <= f.y + f.h - PAD + 0.001);
        }
        assert!(f.w <= ROW_MAX + 0.001);
        // Nothing overlaps.
        for (i, a) in s.cards.iter().enumerate() {
            for b in &s.cards[i + 1..] {
                assert!(!overlaps(a.rect(), b.rect()));
            }
        }
    }

    #[test]
    fn frame_around_pads_and_bands() {
        assert_eq!(frame_around(&[[100.0, 100.0, 10.0, 10.0]]), Some([68.0, 28.0, 74.0, 114.0]));
        assert_eq!(frame_around(&[]), None);
    }
}
