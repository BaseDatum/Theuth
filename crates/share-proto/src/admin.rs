//! The edge's admin socket protocol (§8.5): newline-delimited JSON over a 0600 Unix socket, any number of
//! requests per connection, answered in order. `theuth-share-edge admin <cmd> [args]` maps one to one.
//!
//! ```text
//! → {"id": 1, "cmd": "ban", "args": {"kind": "name", "value": "evil", "reason": "phishing report 123"}}
//! ← {"id": 1, "ok": true, "result": {"closed_tunnels": 1}}
//! ← {"id": 2, "ok": false, "error": {"code": "not_found", "message": "no name evil"}}
//! ```

use iroh_base::PublicKey;
use serde::{Deserialize, Serialize};

use crate::api::Admission;
use crate::error::ErrorCode;

/// What a ban or unban targets (§9.3).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum BanKind {
    /// A name under `D`.
    Name,
    /// A mesh id.
    Mesh,
    /// An engine key.
    Key,
    /// A client network (address or prefix).
    Net,
}

/// `list` selector.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ListWhat {
    /// Names.
    Names,
    /// Meshes.
    Meshes,
    /// Live tunnels.
    Tunnels,
}

/// One admin command, tagged by `cmd` with its `args`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "cmd", content = "args", rename_all = "kebab-case")]
pub enum AdminCommand {
    /// Edge health, admission, counts.
    Status,
    /// List names, meshes or tunnels (default names).
    List {
        /// Which table.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        what: Option<ListWhat>,
    },
    /// The certificate queue.
    Queue,
    /// One mesh: names, engines, revoke list, rekey history.
    Mesh {
        /// Mesh id.
        id: PublicKey,
    },
    /// Ban and close affected tunnels.
    Ban {
        /// Target kind.
        kind: BanKind,
        /// Target value.
        value: String,
        /// Reason shown to the hosting engine.
        reason: String,
    },
    /// Lift a ban.
    Unban {
        /// Target kind.
        kind: BanKind,
        /// Target value.
        value: String,
    },
    /// Release a name (records deleted, certificate revoked).
    Release {
        /// The name.
        name: String,
    },
    /// Evict names never live 7 days after allocation (§9.4).
    EvictUnpublished,
    /// Operator judgement after a lost rekey race (§5.7).
    Recover {
        /// A name or a mesh id.
        target: String,
        /// The mesh to move it to.
        to_mesh: PublicKey,
    },
    /// Switch admission live.
    Admission {
        /// New mode.
        mode: Admission,
    },
    /// Mint an invite; the result carries the ticket.
    InviteNew {
        /// Uses (default 1).
        #[serde(default, skip_serializing_if = "Option::is_none")]
        uses: Option<u32>,
        /// Lifetime in seconds (default 7 days).
        #[serde(default, skip_serializing_if = "Option::is_none")]
        expires_s: Option<u64>,
    },
    /// Resolve and check `D`'s CAA now (§8.2).
    CaaCheck,
    /// Revoke a name's certificate.
    RevokeCert {
        /// The name.
        name: String,
    },
    /// Re-read the reloadable config (§8.1).
    Reload,
    /// Close every tunnel and refuse `hello`.
    Pause,
    /// Undo `pause`.
    Resume,
    /// Which brake conditions of §9.5 hold.
    BrakeStatus,
}

impl AdminCommand {
    /// Whether the command changes state (and so is logged with its `ref`).
    pub fn is_mutating(&self) -> bool {
        !matches!(
            self,
            AdminCommand::Status
                | AdminCommand::List { .. }
                | AdminCommand::Queue
                | AdminCommand::Mesh { .. }
                | AdminCommand::CaaCheck
                | AdminCommand::BrakeStatus
        )
    }
}

/// One request line.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct AdminRequest {
    /// Echoed in the answer.
    pub id: u64,
    /// The command and its args.
    #[serde(flatten)]
    pub command: AdminCommand,
    /// The operator's `--ref` (ticket, report), recorded in `audit` for mutating commands.
    #[serde(default, rename = "ref", skip_serializing_if = "Option::is_none")]
    pub reference: Option<String>,
}

/// An admin failure.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct AdminError {
    /// Code (`not_found`, `invalid`, …).
    pub code: ErrorCode,
    /// Detail for the operator.
    pub message: String,
}

/// One answer line. `result` is command-specific JSON, printed by the CLI as YAML.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct AdminResponse {
    /// The request's id.
    pub id: u64,
    /// Success.
    pub ok: bool,
    /// The result when `ok`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub result: Option<serde_json::Value>,
    /// The error when not `ok`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub error: Option<AdminError>,
}

impl AdminResponse {
    /// A success.
    pub fn ok(id: u64, result: serde_json::Value) -> Self {
        AdminResponse {
            id,
            ok: true,
            result: Some(result),
            error: None,
        }
    }

    /// A failure.
    pub fn err(id: u64, code: ErrorCode, message: impl Into<String>) -> Self {
        AdminResponse {
            id,
            ok: false,
            result: None,
            error: Some(AdminError {
                code,
                message: message.into(),
            }),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn design_example_lines() {
        let line = r#"{"id": 1, "cmd": "ban", "args": {"kind": "name", "value": "evil", "reason": "phishing report 123"}}"#;
        let r: AdminRequest = serde_json::from_str(line).unwrap();
        assert_eq!(
            r.command,
            AdminCommand::Ban {
                kind: BanKind::Name,
                value: "evil".into(),
                reason: "phishing report 123".into()
            }
        );
        assert!(r.command.is_mutating());
        let back: AdminRequest = serde_json::from_str(&serde_json::to_string(&r).unwrap()).unwrap();
        assert_eq!(back, r);

        let s: AdminRequest = serde_json::from_str(r#"{"id":2,"cmd":"status"}"#).unwrap();
        assert_eq!(s.command, AdminCommand::Status);
        let s: AdminRequest =
            serde_json::from_str(r#"{"id":3,"cmd":"evict-unpublished","ref":"T-1"}"#).unwrap();
        assert_eq!(s.command, AdminCommand::EvictUnpublished);
        assert_eq!(s.reference.as_deref(), Some("T-1"));
        let s = serde_json::to_string(&AdminRequest {
            id: 4,
            command: AdminCommand::Pause,
            reference: None,
        })
        .unwrap();
        assert_eq!(
            serde_json::from_str::<AdminRequest>(&s).unwrap().command,
            AdminCommand::Pause
        );

        let e = AdminResponse::err(2, ErrorCode::NotFound, "no name evil");
        assert_eq!(
            serde_json::to_string(&e).unwrap(),
            r#"{"id":2,"ok":false,"error":{"code":"not_found","message":"no name evil"}}"#
        );
    }
}
