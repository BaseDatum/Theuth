//! [`PluginsError`] (contract §9.1). B0, frozen.

use serde::{Deserialize, Serialize};

/// Every failure of the plugins host, as actor replies, RPC errors and tool errors carry it.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum PluginsError {
    /// "no plugin named {name}"
    NotFound {
        /// Plugin name.
        name: String,
    },
    /// "{message}" (manifest error with its JSON Pointer, bad name, nothing to approve)
    Invalid {
        /// What is wrong.
        message: String,
    },
    /// "plugins/{name} already exists"
    Exists {
        /// Plugin name.
        name: String,
    },
    /// "{name} is awaiting approval: press Approve in theuth or run `t plugin approve {name}`"
    AwaitingApproval {
        /// Plugin name.
        name: String,
    },
    /// "plugin_approve needs a human press: the Approve button in theuth or `t plugin approve` on a terminal"
    NotAPress,
    /// "the card for {name} changed; review it again"
    Stale {
        /// Plugin name.
        name: String,
    },
    /// "plugin {name} is not running"
    NotRunning {
        /// Plugin name.
        name: String,
    },
    /// "not granted: {what}"
    NotGranted {
        /// The capability, topic, placement or tool.
        what: String,
    },
    /// "rate limited; retry in {retry_after_ms} ms"
    RateLimited {
        /// When to retry, ms.
        retry_after_ms: u64,
    },
    /// "plugin {name} exited"
    Exited {
        /// Plugin name.
        name: String,
    },
    /// "plugin {name} did not answer {method} in time"
    Timeout {
        /// Plugin name.
        name: String,
        /// The pipe method that timed out.
        method: String,
    },
    /// "{message}" — the plugin's own JSON-RPC error, passed through
    Plugin {
        /// The plugin's error code.
        code: i64,
        /// The plugin's error message.
        message: String,
    },
    /// "{message}"
    Io {
        /// What failed.
        message: String,
    },
    /// "{what}: not implemented yet (216)"
    NotImplemented {
        /// The message or method.
        what: String,
    },
}

impl std::fmt::Display for PluginsError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::NotFound { name } => write!(f, "no plugin named {name}"),
            Self::Invalid { message } => write!(f, "{message}"),
            Self::Exists { name } => write!(f, "plugins/{name} already exists"),
            Self::AwaitingApproval { name } => write!(
                f,
                "{name} is awaiting approval: press Approve in theuth or run `t plugin approve {name}`"
            ),
            Self::NotAPress => write!(
                f,
                "plugin_approve needs a human press: the Approve button in theuth or `t plugin approve` on a terminal"
            ),
            Self::Stale { name } => write!(f, "the card for {name} changed; review it again"),
            Self::NotRunning { name } => write!(f, "plugin {name} is not running"),
            Self::NotGranted { what } => write!(f, "not granted: {what}"),
            Self::RateLimited { retry_after_ms } => {
                write!(f, "rate limited; retry in {retry_after_ms} ms")
            }
            Self::Exited { name } => write!(f, "plugin {name} exited"),
            Self::Timeout { name, method } => {
                write!(f, "plugin {name} did not answer {method} in time")
            }
            Self::Plugin { message, .. } => write!(f, "{message}"),
            Self::Io { message } => write!(f, "{message}"),
            Self::NotImplemented { what } => write!(f, "{what}: not implemented yet (216)"),
        }
    }
}

impl std::error::Error for PluginsError {}

impl PluginsError {
    /// JSON-RPC code for RPC callers (216 §6.5 plus rpc-core's).
    pub fn code(&self) -> i32 {
        match self {
            Self::NotAPress => -32002, // rpc_core::UNAUTHORIZED
            Self::NotGranted { .. } => -32003,
            Self::RateLimited { .. } => -32005,
            Self::Exited { .. } | Self::NotRunning { .. } => -32006,
            Self::Invalid { .. } => -32602,
            _ => -32000, // rpc_core::ASK_FAILED
        }
    }

    /// `NotImplemented { what }` — the B0 stub answer.
    pub fn not_implemented(what: &str) -> Self {
        Self::NotImplemented { what: what.to_owned() }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn codes_match_rpc_core() {
        assert_eq!(PluginsError::NotAPress.code(), rpc_core::UNAUTHORIZED);
        assert_eq!(PluginsError::not_implemented("x").code(), rpc_core::ASK_FAILED);
        assert_eq!(PluginsError::not_implemented("plugin_list").to_string(), "plugin_list: not implemented yet (216)");
    }
}
