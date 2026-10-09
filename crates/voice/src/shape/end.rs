//! Spoken end commands on partials (153 §7.1): a state machine the shaper
//! feeds per piece — `Idle → Armed` on an exact phrase at a partial's tail,
//! sticky across re-decodes, fired at the final.
//!
//! 153-C stub — A1 fills.

use super::{EndCommand, EndPhrases};

/// The §7.1 state machine for one utterance.
#[derive(Debug, Default)]
pub struct EndParser {
    armed: Option<EndCommand>,
}

impl EndParser {
    /// A fresh parser for `phrases`.
    pub fn new(phrases: &EndPhrases) -> Self {
        let _ = phrases;
        Self::default()
    }

    /// Feed the utterance's text so far; `Some` when a command is armed (partial) or fires (final).
    pub fn feed(&mut self, raw: &str, is_final: bool) -> Option<EndCommand> {
        // 153-C stub — A1 fills.
        let _ = (raw, is_final);
        self.armed
    }

    /// Back to `Idle` (a new utterance).
    pub fn reset(&mut self) {
        self.armed = None;
    }
}
