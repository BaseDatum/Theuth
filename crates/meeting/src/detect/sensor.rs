//! 158 W1 owns this file (stub from C158).
//!
//! The mic sensor (158 §4.1): which processes hold an input stream right now. Per-OS code
//! beside `capture::system::processes()` — macOS reads `kAudioProcessPropertyIsRunningInput`
//! on the same process objects, Windows walks the default capture endpoint's sessions, Linux
//! lists PipeWire `Stream/Input/Audio` nodes. The sensor never opens a device (§4.6).

use crate::ProcessInfo;

/// Processes holding an input stream, longest-known first; `active` means "input running".
/// Skips our own pid and `com.apple.CoreSpeech` like `processes()` does. Stub: nobody.
pub fn mic_holders() -> anyhow::Result<Vec<ProcessInfo>> {
    Ok(Vec::new())
}
