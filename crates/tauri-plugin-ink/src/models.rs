//! The plugin's wire types (218 contract §6), as JS and Swift see them. The Rust side only produces
//! [`AttachResult`] (the desktop stub); the rest documents and tests the JSON the Swift side sends, so a
//! change to either side shows up as a failing golden test here.

use serde::{Deserialize, Serialize};

/// Why native capture is off (`AttachResult.reason`).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Reason {
    /// A desktop build: there is no Pencil layer.
    Desktop,
    /// iPadOS older than 26 (`pencilKitResponderState` is 26.0, R5).
    UnsupportedOs,
    /// An iPhone (or another idiom): the whiteboard canvas is read-only there.
    NotIpad,
    /// Neither capture route could be installed on the web view.
    RouteFailed,
}

/// How the overlay receives Pencil touches (design §6.2).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum Route {
    /// `PKCanvasView.drawingGestureRecognizer` moved onto the web view.
    A,
    /// The plugin's own pencil-only recogniser drawing a `CAShapeLayer`.
    B,
}

/// `attach`'s answer.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct AttachResult {
    /// True when the overlay is mounted and Pencil strokes come through `onStrokes`.
    pub native: bool,
    /// Why `native` is false.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub reason: Option<Reason>,
    /// The OS version, e.g. `"26.0"`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub os: Option<String>,
    /// The capture route in use while `native`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub route: Option<Route>,
}

impl AttachResult {
    /// Native capture is off for `reason`.
    pub fn off(reason: Reason) -> Self {
        Self {
            native: false,
            reason: Some(reason),
            os: None,
            route: None,
        }
    }
}

/// The world → web-view-point transform: `screen = world × zoom + (tx, ty)`, CSS px from the web view's
/// top-left.
#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
pub struct NativeView {
    /// Horizontal offset, CSS px.
    pub tx: f64,
    /// Vertical offset, CSS px.
    pub ty: f64,
    /// World px per CSS px.
    pub zoom: f64,
}

/// A drawing tool of the picker. `Eraser` never produces strokes.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Tool {
    /// Constant width.
    Monoline,
    /// Pressure-sensitive pen.
    Pen,
    /// Wide, translucent marker.
    Marker,
    /// Whole-stroke eraser (web side).
    Eraser,
}

/// One captured stroke, handed over at pen-idle.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct NativeStroke {
    /// Monotonic per attach, from 1: identity for ack and de-duplication.
    pub seq: u64,
    /// `monoline`, `pen` or `marker`.
    pub tool: Tool,
    /// `#rrggbb` as picked.
    pub colour: String,
    /// Base width in points.
    pub width: f64,
    /// Ms since the Unix epoch at the first point.
    pub t0: f64,
    /// The viewport at stroke begin.
    pub view: NativeView,
    /// Flat, four numbers a point: x, y (web-view points), force 0..1, t (ms since `t0`).
    pub pts: Vec<f64>,
    /// 0 on first delivery, +1 per resend.
    pub retry: u32,
}

/// One hand-over: the strokes not yet acknowledged, in stroke order.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct StrokeBatch {
    /// Monotonic per attach, from 1.
    pub batch: u64,
    /// The strokes, ordered by `seq`.
    pub strokes: Vec<NativeStroke>,
}

/// What a Pencil gesture asks for: the user's preferred action (Settings → Apple Pencil).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum PencilAction {
    /// Toggle between the current tool and the eraser.
    SwitchEraser,
    /// Toggle between the current and the previous tool.
    SwitchPrevious,
    /// Show the colour palette.
    ShowColorPalette,
    /// Show the ink attributes (width, opacity).
    ShowInkAttributes,
    /// Show the contextual palette at the Pencil's position.
    ShowContextualPalette,
    /// Run the system shortcut.
    RunSystemShortcut,
    /// Do nothing.
    Ignore,
}

/// Which Pencil gesture happened.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum PencilGesture {
    /// Double-tap (Pencil 2, Pro).
    Tap,
    /// Squeeze (Pencil Pro, iPadOS 17.5).
    Squeeze,
}

/// Events on the `onEvent` channel.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum InkEvent {
    /// Pen down (`true`) or up.
    State {
        /// The Pencil is on the glass.
        drawing: bool,
    },
    /// The picker's selection changed.
    Tool {
        /// The selected tool.
        tool: Tool,
        /// `#rrggbb`.
        colour: String,
        /// Base width in points.
        width: f64,
    },
    /// A Pencil tap or squeeze.
    Pencil {
        /// Tap or squeeze.
        gesture: PencilGesture,
        /// The user's preferred action for it.
        action: PencilAction,
    },
    /// Something native failed; the message is for the log.
    Error {
        /// What failed.
        message: String,
    },
}

/// `set_tool`'s arguments.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct ToolArgs {
    /// The tool to select.
    pub tool: Tool,
    /// `#rrggbb`, resolved for the theme.
    pub colour: String,
    /// Base width in points.
    pub width: f64,
}

/// `recognize_text`'s answer.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Recognized {
    /// The recognised text, ≤ 200 chars.
    pub text: String,
    /// Vision's confidence, 0..1.
    pub confidence: f64,
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::{Value, json};

    fn round_trip<T>(value: &T, golden: Value)
    where
        T: Serialize + for<'de> Deserialize<'de> + PartialEq + std::fmt::Debug,
    {
        assert_eq!(serde_json::to_value(value).unwrap(), golden);
        let back: T = serde_json::from_value(golden).unwrap();
        assert_eq!(&back, value);
    }

    #[test]
    fn stroke_batch_serde_shape() {
        let batch = StrokeBatch {
            batch: 3,
            strokes: vec![NativeStroke {
                seq: 7,
                tool: Tool::Pen,
                colour: "#d9480f".into(),
                width: 2.8,
                t0: 1_795_570_200_000.0,
                view: NativeView {
                    tx: -120.5,
                    ty: 40.0,
                    zoom: 1.25,
                },
                pts: vec![10.0, 20.0, 0.5, 0.0, 11.5, 20.25, 1.0, 8.0],
                retry: 0,
            }],
        };
        round_trip(
            &batch,
            json!({
                "batch": 3,
                "strokes": [{
                    "seq": 7, "tool": "pen", "colour": "#d9480f", "width": 2.8, "t0": 1_795_570_200_000.0,
                    "view": { "tx": -120.5, "ty": 40.0, "zoom": 1.25 },
                    "pts": [10.0, 20.0, 0.5, 0.0, 11.5, 20.25, 1.0, 8.0],
                    "retry": 0
                }]
            }),
        );
        round_trip(
            &AttachResult {
                native: true,
                reason: None,
                os: Some("26.0".into()),
                route: Some(Route::A),
            },
            json!({ "native": true, "os": "26.0", "route": "A" }),
        );
        round_trip(
            &AttachResult::off(Reason::UnsupportedOs),
            json!({ "native": false, "reason": "unsupported_os" }),
        );
        round_trip(
            &ToolArgs {
                tool: Tool::Eraser,
                colour: "#1a1612".into(),
                width: 6.0,
            },
            json!({ "tool": "eraser", "colour": "#1a1612", "width": 6.0 }),
        );
    }

    #[test]
    fn ink_event_serde_shape() {
        round_trip(
            &InkEvent::State { drawing: true },
            json!({ "kind": "state", "drawing": true }),
        );
        round_trip(
            &InkEvent::Tool {
                tool: Tool::Marker,
                colour: "#f2c14e".into(),
                width: 6.0,
            },
            json!({ "kind": "tool", "tool": "marker", "colour": "#f2c14e", "width": 6.0 }),
        );
        round_trip(
            &InkEvent::Pencil {
                gesture: PencilGesture::Squeeze,
                action: PencilAction::ShowContextualPalette,
            },
            json!({ "kind": "pencil", "gesture": "squeeze", "action": "showContextualPalette" }),
        );
        round_trip(
            &InkEvent::Pencil {
                gesture: PencilGesture::Tap,
                action: PencilAction::SwitchEraser,
            },
            json!({ "kind": "pencil", "gesture": "tap", "action": "switchEraser" }),
        );
        round_trip(
            &InkEvent::Error {
                message: "route A failed".into(),
            },
            json!({ "kind": "error", "message": "route A failed" }),
        );
        round_trip(
            &Recognized {
                text: "no third tab".into(),
                confidence: 0.5,
            },
            json!({ "text": "no third tab", "confidence": 0.5 }),
        );
    }
}
