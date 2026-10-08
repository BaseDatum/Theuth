//! Linux: no companion (130 §5.9). The tray stays, nothing is launched or watched.

use tauri::AppHandle;

/// Nothing to launch.
pub fn init(_app: &AppHandle) -> anyhow::Result<()> {
    Ok(())
}

/// Never alive.
pub fn alive() -> bool {
    false
}

/// Nothing to stop.
pub fn terminate() {}
