//! Helpers the chat adapters (`signal`, `slack`, `telegram`) share. Every item
//! here was a byte-identical copy in two or three of them; adapter-specific
//! variants stay in their crates.

pub mod commands;
pub mod media;
pub mod port;
pub mod recent;

use std::time::Duration;

pub use commands::{
    Selector, agent, communication_text, err, git_url, is_provider, jobs, mcp, mcp_entry, on_off,
    parse_broadcast, pending_id, receipt_text, short, synced, table, word,
};
pub use media::{extension_for, sniff};
pub use port::agents;
pub use recent::{RECENT_KEYS, RecentKeys};

/// Reconnect backoff: `min(2^min(attempt, 6), 60)` seconds (09 §3.1).
#[must_use]
pub fn backoff_delay(attempt: u32) -> Duration {
    Duration::from_secs((1u64 << attempt.min(6)).min(60))
}

/// The 37 §7 line: where to enter a requested secret. Never answerable in chat.
#[must_use]
pub fn secret_notice(name: &str, note: &str) -> String {
    format!(
        "theuth asks for secret `{name}` — {note}. Enter it in the desktop app, the TUI (/secrets add {name}) or `t --secret-set {name}`; it will not be read from this chat."
    )
}

/// The 221 §9.4 line for a `secret_request` that stores into an external manager (`to` =
/// `bw/personal`). Answerable only on the app's masked card, never in chat.
#[must_use]
pub fn external_secret_notice(name: &str, note: &str, to: &str) -> String {
    format!(
        "theuth asks for secret `{name}` — {note}, to store in `{to}`. Answer it in the desktop app or the TUI (/secrets); it will not be read from this chat."
    )
}

/// The 221 §9.4 line for an unlock card (`target` = `bw/personal`). Never answerable in chat.
#[must_use]
pub fn unlock_notice(target: &str, account: &str) -> String {
    format!(
        "theuth needs `{target}` ({account}) unlocked. Unlock it in the desktop app or the TUI; the master password will not be read from this chat."
    )
}

/// `s` cut to its first `max` chars with `…` appended when it was longer.
#[must_use]
pub fn truncate(s: &str, max: usize) -> String {
    platform::text::clip_then_ellipsis(s, max).into_owned()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn backoff_caps_at_sixty() {
        assert_eq!(backoff_delay(0), Duration::from_secs(1));
        assert_eq!(backoff_delay(3), Duration::from_secs(8));
        assert_eq!(backoff_delay(6), Duration::from_secs(60));
        assert_eq!(backoff_delay(40), Duration::from_secs(60));
    }
}
