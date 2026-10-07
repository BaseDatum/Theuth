//! Seconds since the user last touched the machine (126 §8.2, the "user idle"
//! precondition).
//!
//! macOS: `CGEventSourceSecondsSinceLastEventType(kCGEventSourceStateCombinedSessionState,
//! kCGAnyInputEventType)` — every HID input (keys, mouse, trackpad, tablet),
//! no accessibility permission needed. Linux and Windows have no probe here
//! yet and answer `None`; callers treat `None` as "not idle" (the conservative
//! reading: training waits rather than fighting the user for the GPU).

/// Seconds since the last user input, or `None` when the platform cannot say.
pub fn idle_secs() -> Option<f64> {
    imp::idle_secs()
}

#[cfg(target_os = "macos")]
mod imp {
    use objc2_core_graphics::{CGEventSource, CGEventSourceStateID, CGEventType};

    /// `kCGAnyInputEventType` is `(CGEventType)(~0)`; the generated bindings
    /// leave it out because it is a macro, not an enum case.
    const ANY_INPUT: CGEventType = CGEventType(u32::MAX);

    pub fn idle_secs() -> Option<f64> {
        let s = CGEventSource::seconds_since_last_event_type(
            CGEventSourceStateID::CombinedSessionState,
            ANY_INPUT,
        );
        s.is_finite().then_some(s.max(0.0))
    }
}

#[cfg(not(target_os = "macos"))]
mod imp {
    pub fn idle_secs() -> Option<f64> {
        None
    }
}

#[cfg(all(test, target_os = "macos"))]
mod tests {
    #[test]
    fn mac_answers_a_finite_non_negative_number() {
        let s = super::idle_secs().expect("CoreGraphics answers on macOS");
        assert!(s >= 0.0);
    }
}
