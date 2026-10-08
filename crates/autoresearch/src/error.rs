//! The crate's error type. Every stub of the wave returns
//! [`Error::NotBuilt`] naming its owner until that worker lands.

/// Everything the plugin's modules can fail with.
#[derive(Debug, thiserror::Error)]
pub enum Error {
    /// The module is a 140 §21.1 stub; the payload names the worker (`"W1"`, …).
    #[error("not built yet (owner {0})")]
    NotBuilt(&'static str),
    /// The call is not allowed here (not an autoresearch session, a slug that
    /// does not exist, a state that forbids the op); shown to the caller verbatim.
    #[error("{0}")]
    Refused(String),
    /// `autoresearch.sqlite`.
    #[error("store: {0}")]
    Sqlx(#[from] sqlx::Error),
    /// Files under `<data_dir>/autoresearch/`.
    #[error("io: {0}")]
    Io(#[from] std::io::Error),
    /// A JSON column or a tool argument.
    #[error("json: {0}")]
    Json(#[from] serde_json::Error),
    /// Anything else.
    #[error(transparent)]
    Other(#[from] anyhow::Error),
}

/// `Result` with the crate's [`Error`] by default.
pub type Result<T, E = Error> = std::result::Result<T, E>;

impl Error {
    /// A [`Error::Refused`] from any message.
    pub fn refused(msg: impl Into<String>) -> Self {
        Self::Refused(msg.into())
    }
}
