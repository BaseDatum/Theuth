//! 158 W1 owns this file (stub from C158).
//!
//! The rules engine (158 §4.4): Idle → Candidate → Prompting → Recording → Ending as a state
//! machine over [`super::sensor::mic_holders`] ticks, emitting `MeetingEvent::Detected`,
//! `DetectionCleared` and `AppReleased`. The thresholds are the contract; the machine is W1's.

use crate::ProcessInfo;

/// Continuous hold before a prompt (§4.4 rule 2).
pub const HOLD_SECS: u64 = 15;
/// "Not now" cooldown per app id (§4.4 rule 5).
pub const COOLDOWN_SECS: u64 = 10 * 60;
/// Release before `AppReleased` / `DetectionCleared{Released}` (§4.4 rules 7–8).
pub const RELEASE_SECS: u64 = 30;

/// §4.3 app id: the bundle id on macOS, else the process name (W1 lowercases Windows image
/// names and maps Linux `application.name` here).
pub fn app_id(p: &ProcessInfo) -> String {
    p.bundle_id.clone().unwrap_or_else(|| p.name.clone())
}
