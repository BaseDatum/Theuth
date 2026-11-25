//! The desktop stub (218): there is no Apple Pencil on macOS, Linux or Windows. `pencil_listen` says so,
//! `pencil_unlisten` is a no-op and `recognize_text` answers an empty recognition, so a call never fails.

use tauri::ipc::Channel;

use crate::models::{InkEvent, ListenResult, Recognized};

/// What `pencil_listen` answers on desktop.
pub(crate) fn listen_result() -> ListenResult {
    ListenResult { ok: false }
}

#[tauri::command]
pub(crate) fn pencil_listen(on_event: Channel<InkEvent>) -> ListenResult {
    let _ = on_event;
    listen_result()
}

#[tauri::command]
pub(crate) fn pencil_unlisten() {}

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
    fn pencil_listen_reports_desktop() {
        let result = listen_result();
        assert!(!result.ok);
        assert_eq!(
            serde_json::to_value(&result).unwrap(),
            serde_json::json!({ "ok": false })
        );
    }
}
