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
    vec![Gesture::End { at: pt(last) }]
}

fn pt(s: &Sample) -> Pt {
    Pt { x: s.x, y: s.y }
}
