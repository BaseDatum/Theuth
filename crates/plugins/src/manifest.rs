//! Owner: B1 — `plugin.yaml` parse and §4.3 validation, canonical JSON, manifest and code hashes.
//! Public only for [`check_name`], [`RESERVED_NAMES`] and [`check_offline`] (contract §3.4).

use crate::error::PluginsError;
use crate::types::CheckReport;

/// Built-in slash command names a plugin may not take (without the `/`). B2's TUI test asserts
/// every `crates/tui` COMMANDS name is in this list.
pub const RESERVED_NAMES: &[&str] = &[/* B1 fills from crates/tui/src/model/slash.rs and UI/components/chat/slash.ts */];

/// `^[a-z][a-z0-9-]{0,31}$`, not a reserved name (a built-in slash command of the desktop or
/// the TUI, or `plugins`). Used by the loader and by `scaffold::new_plugin`.
pub fn check_name(name: &str) -> Result<(), PluginsError> {
    let _ = name;
    Err(PluginsError::not_implemented("manifest::check_name"))
}

/// Validate a folder without an engine: parse + every §4.3 rule that needs no vault and no
/// catalog; those two are reported as warnings ("engine not running: secrets and topics not
/// checked"). Also computes both hashes and the card lines.
pub fn check_offline(dir: &std::path::Path) -> CheckReport {
    let _ = dir;
    CheckReport {
        ok: false,
        name: None,
        errors: vec![crate::types::CheckIssue {
            path: String::new(),
            message: PluginsError::not_implemented("manifest::check_offline").to_string(),
        }],
        warnings: Vec::new(),
        manifest_sha256: None,
        code_sha256: None,
        lines: Vec::new(),
    }
}
