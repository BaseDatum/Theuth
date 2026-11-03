//! `CellsError` (owner: C, FROZEN): every refusal of §5.6, §5.10, §7, §12.1 as one variant, so
//! every worker maps a refusal to the same reason and the RPC layer turns it into one stable
//! JSON-RPC error (`code()` is the machine reason clients branch on; `Display` is the text the
//! cell shows).

/// A cells failure or refusal.
#[derive(Clone, Debug, PartialEq, Eq, thiserror::Error)]
pub enum CellsError {
    /// The code path belongs to a 171 worker that has not landed yet (the worker tag).
    #[error("not built yet (171:{0})")]
    NotBuilt(&'static str),
    /// The cells plugin is disabled or its actor is not running.
    #[error("cells is not running")]
    NotRunning,
    /// Press guard (§12.1 invariant 1): guest, internal peer or unauthenticated.
    #[error("only the owner can run cells")]
    NotOwner,
    /// Invariant 5: the engine's source digest differs from `seen`, or the message is not final.
    #[error("this cell changed — look again")]
    Changed,
    /// §3.5: a newer version of the name exists.
    #[error("replaced by a newer version")]
    Superseded,
    /// Invariant 8 / §7.6: the `after` gate is not satisfied (the reason text).
    #[error("{0}")]
    Disarmed(String),
    /// Invariant 7: a `danger` cell pressed with `mode != cell` (the label).
    #[error("press {0} to run this")]
    Danger(String),
    /// Invariant 6: a member-written block without `member_ack` (the member's pub8).
    #[error("changed by {0} since your last run — review it first")]
    MemberAck(String),
    /// §3.6: the cell has a validation error (the problem code).
    #[error("this cell is invalid: {0}")]
    Invalid(String),
    /// §5.6: the cwd does not exist at press.
    #[error("folder not found — this cell will not start anywhere else")]
    FolderNotFound(String),
    /// §5.10: every kernel slot is busy (the cap).
    #[error("{0} kernels are busy — close one")]
    KernelBusy(u32),
    /// §5.10: a reused run id with a different cell or digest.
    #[error("run id {0} is already used by another cell")]
    RunIdConflict(String),
    /// No such run, cell or kernel.
    #[error("not found: {0}")]
    NotFound(String),
    /// §9.1: a private run's output was asked for.
    #[error("output kept from theuth (private)")]
    Private,
    /// The interpreter cannot be resolved (§6.1 "No Python for this folder", §5.7 no bash).
    #[error("{0}")]
    NoInterpreter(String),
    /// Anything else (I/O, sqlite, a dead kernel).
    #[error("{0}")]
    Failed(String),
}

impl CellsError {
    /// Stable machine reason, carried as the JSON-RPC error's `data.reason`.
    pub fn code(&self) -> &'static str {
        match self {
            CellsError::NotBuilt(_) => "not_built",
            CellsError::NotRunning => "not_running",
            CellsError::NotOwner => "not_owner",
            CellsError::Changed => "changed",
            CellsError::Superseded => "superseded",
            CellsError::Disarmed(_) => "disarmed",
            CellsError::Danger(_) => "danger",
            CellsError::MemberAck(_) => "member_ack",
            CellsError::Invalid(_) => "invalid",
            CellsError::FolderNotFound(_) => "folder_not_found",
            CellsError::KernelBusy(_) => "kernel_busy",
            CellsError::RunIdConflict(_) => "run_id_conflict",
            CellsError::NotFound(_) => "not_found",
            CellsError::Private => "private",
            CellsError::NoInterpreter(_) => "no_interpreter",
            CellsError::Failed(_) => "failed",
        }
    }
}

impl From<anyhow::Error> for CellsError {
    fn from(e: anyhow::Error) -> Self {
        CellsError::Failed(format!("{e:#}"))
    }
}

impl From<std::io::Error> for CellsError {
    fn from(e: std::io::Error) -> Self {
        CellsError::Failed(e.to_string())
    }
}

/// Result alias.
pub type Result<T, E = CellsError> = std::result::Result<T, E>;
