//! The mic sensor (158 §4.1): which processes hold an input stream right now. Thin per-OS
//! glue over `capture::system`'s process walks — macOS reads `kAudioProcessPropertyIsRunningInput`
//! on the same process objects, Windows walks the default capture endpoints' sessions, Linux
//! lists PipeWire `Stream/Input/Audio` nodes. The sensor never opens a device (§4.6).

use crate::ProcessInfo;
use crate::capture::system;

/// Processes holding an input stream, by name; `active` is always `true` here ("input
/// running"). Skips our own pid and, on macOS, `com.apple.CoreSpeech` (Siri's helper holds the
/// mic on every "Hey Siri" Mac).
pub fn mic_holders() -> anyhow::Result<Vec<ProcessInfo>> {
    let mut out = holders()?;
    out.retain(|p| p.active && p.bundle_id.as_deref() != Some("com.apple.CoreSpeech"));
    out.sort_by_key(|p| p.name.to_lowercase());
    Ok(out)
}

#[cfg(target_os = "macos")]
fn holders() -> anyhow::Result<Vec<ProcessInfo>> {
    system::process_objects(objc2_core_audio::kAudioProcessPropertyIsRunningInput)
}

/// Default capture endpoint for `eConsole` **and** `eCommunications` (they can differ).
#[cfg(target_os = "windows")]
fn holders() -> anyhow::Result<Vec<ProcessInfo>> {
    use windows::Win32::Media::Audio::{eCapture, eCommunications, eConsole};
    system::sessions_on(eCapture, &[eConsole, eCommunications])
}

/// A node's existence is "holding the mic" (no cheap running bit).
#[cfg(target_os = "linux")]
fn holders() -> anyhow::Result<Vec<ProcessInfo>> {
    system::nodes("Stream/Input/Audio")
}

#[cfg(not(any(target_os = "macos", target_os = "windows", target_os = "linux")))]
fn holders() -> anyhow::Result<Vec<ProcessInfo>> {
    anyhow::bail!("no mic sensor on this platform")
}

#[cfg(test)]
mod tests {
    /// §4.7: the probe returns on this host (empty is fine); its failure mode is `Err`, not a
    /// panic or a hang.
    #[test]
    fn mic_holders_returns() {
        match super::mic_holders() {
            Ok(list) => assert!(list.iter().all(|p| p.active && p.pid != std::process::id())),
            Err(e) => eprintln!("mic_holders: {e:#} (no audio HAL on this host)"),
        }
    }
}
