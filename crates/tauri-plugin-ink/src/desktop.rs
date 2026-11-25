//! The desktop stub (218 §6): there is no Pencil layer on macOS, Linux or Windows. `attach` says so and
//! the UI draws with pointer events; every other command is a no-op so a stray call never fails.

use tauri::ipc::Channel;

use crate::models::{
    AttachResult, InkEvent, NativeView, Reason, Recognized, StrokeBatch, Tool, ToolArgs,
};

/// What `attach` answers on desktop.
pub(crate) fn attach_result() -> AttachResult {
    AttachResult::off(Reason::Desktop)
}

#[tauri::command]
pub(crate) fn attach(
    on_strokes: Channel<StrokeBatch>,
    on_event: Channel<InkEvent>,
) -> AttachResult {
    let _ = (on_strokes, on_event);
    attach_result()
}

#[tauri::command]
pub(crate) fn detach() {}

#[tauri::command]
pub(crate) fn set_enabled(enabled: bool) {
    let _ = enabled;
}

#[tauri::command]
pub(crate) fn set_viewport(tx: f64, ty: f64, zoom: f64) {
    let _ = NativeView { tx, ty, zoom };
}

#[tauri::command]
pub(crate) fn set_tool(tool: Tool, colour: String, width: f64) {
    let _ = ToolArgs {
        tool,
        colour,
        width,
    };
}

#[tauri::command]
pub(crate) fn set_picker_visible(visible: bool) {
    let _ = visible;
}

#[tauri::command]
pub(crate) fn ack(seqs: Vec<u64>) {
    let _ = seqs;
}

/// Vision is wired on iPadOS only; desktop answers an empty recognition rather than an error.
#[tauri::command]
pub(crate) fn recognize_text(png: String) -> Recognized {
    let _ = png;
    Recognized {
        text: String::new(),
        confidence: 0.0,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn attach_reports_desktop() {
        let result = attach_result();
        assert!(!result.native);
        assert_eq!(result.reason, Some(Reason::Desktop));
        assert_eq!(
            serde_json::to_value(&result).unwrap(),
            serde_json::json!({ "native": false, "reason": "desktop" })
        );
    }
}
