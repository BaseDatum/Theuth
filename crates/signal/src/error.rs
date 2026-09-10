// FROZEN: copied from jarvis 9d74d5a crates/jarvis-signal/src/error.rs, minus the jarvis `Core` variant.
// Protocol code; edit only with a matching note in docs/design/09-signal.md §2.2.
//! Error type for the Signal channel crate.

/// Errors produced by the Signal channel and its transports.
#[derive(Debug, thiserror::Error)]
pub enum SignalError {
    /// The underlying transport failed (network, protocol, store).
    #[error("transport error: {0}")]
    Transport(String),

    /// The Signal account is not linked/registered yet.
    #[error("signal account not registered")]
    NotRegistered,

    /// A referenced entity (conversation, blob, ...) does not exist.
    #[error("not found: {0}")]
    NotFound(String),

    /// The operation is not supported by this transport build.
    #[error("unsupported: {0}")]
    Unsupported(String),
}

/// Convenience result alias over [`SignalError`].
pub type Result<T> = std::result::Result<T, SignalError>;
