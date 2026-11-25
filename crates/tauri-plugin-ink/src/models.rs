//! The plugin's wire types (218), as JS and Swift see them. The Rust side only produces [`ListenResult`]
//! and [`Recognized`] (the desktop stub); the rest documents and tests the JSON the Swift side sends, so a
//! change to either side shows up as a failing golden test here.

use serde::{Deserialize, Serialize};

/// `pencil_listen`'s answer.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct ListenResult {
    /// True when Pencil gestures come through `onEvent`; false below iPadOS 17.5, off iPad and on desktop.
    pub ok: bool,
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
    fn ink_event_serde_shape() {
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
                message: "pencil event failed".into(),
            },
            json!({ "kind": "error", "message": "pencil event failed" }),
        );
        round_trip(
            &Recognized {
                text: "no third tab".into(),
                confidence: 0.5,
            },
            json!({ "text": "no third tab", "confidence": 0.5 }),
        );
        round_trip(&ListenResult { ok: true }, json!({ "ok": true }));
    }
}
