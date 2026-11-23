//! The edge's admin socket protocol (§8.5): newline-delimited JSON over a 0600 Unix socket, any number of
//! requests per connection, answered in order. `theuth-share-edge admin <cmd> [args]` maps one to one.
//!
//! ```text
//! → {"id": 1, "cmd": "ban", "args": {"kind": "name", "value": "evil", "reason": "phishing report 123"}}
//! ← {"id": 1, "ok": true, "result": {"closed_tunnels": 1}}
//! ← {"id": 2, "ok": false, "error": {"code": "not_found", "message": "no name evil"}}
//! ```

use iroh_base::PublicKey;
use serde::{Deserialize, Serialize, de::DeserializeOwned};

use crate::api::{Admission, MeshEngine, NameInfo, RekeyEntry};
use crate::error::ErrorCode;
use crate::ticket::ShareEdgeTicket;

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
        /// Operator note stored with the invite (`invites.note`).
        #[serde(default, skip_serializing_if = "Option::is_none")]
        note: Option<String>,
    },
    /// List live invites (never the secrets, only their hashes).
    InviteList,
    /// Revoke an invite by its hash ([`InviteRow::hash`]).
    InviteRevoke {
        /// Lowercase hex of [`crate::Invite::hash`], or a unique prefix of at least 8 characters.
        hash: String,
    },
    /// Resolve and check `D`'s CAA now (§8.2).
    CaaCheck,
    /// Revoke a name's certificate.
    RevokeCert {
        /// The name.
        name: String,
        /// RFC 5280 reason (default `unspecified`).
        #[serde(default, skip_serializing_if = "Option::is_none")]
        reason: Option<RevokeReason>,
    },
    /// Make a name's queued certificate job due now and reset its attempt count (after an infrastructure fix,
    /// instead of waiting out the backoff).
    CertRetry {
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
                | AdminCommand::InviteList
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

    /// A success with a typed result (one of this module's `*Result` types).
    pub fn ok_typed<T: Serialize>(id: u64, result: &T) -> Self {
        Self::ok(
            id,
            serde_json::to_value(result).expect("admin results always serialise"),
        )
    }

    /// Decodes `result` as `T`: `None` when there is none (an error answer).
    pub fn result_as<T: DeserializeOwned>(&self) -> Option<Result<T, serde_json::Error>> {
        self.result
            .as_ref()
            .map(|v| serde_json::from_value(v.clone()))
    }
}

/// RFC 5280 revocation reasons the edge sends (§6.3).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Default, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum RevokeReason {
    /// `unspecified`.
    #[default]
    Unspecified,
    /// `keyCompromise`: a rotation after a compromised removal, or an operator decision.
    KeyCompromise,
    /// `superseded`: an ordinary rotation.
    Superseded,
    /// `cessationOfOperation`: release, expiry, transfer.
    CessationOfOperation,
}

/// `ban`, `pause`: how many tunnels were closed.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct ClosedTunnels {
    /// Tunnels closed.
    pub closed_tunnels: u64,
}

/// `admission`: the mode now in force.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct AdmissionResult {
    /// The mode.
    pub admission: Admission,
}

/// `status`. The M1 edge sends the first eight fields; the rest are `#[serde(default)]`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct StatusResult {
    /// `theuth-share-edge` version.
    pub version: String,
    /// `D`.
    pub domain: String,
    /// The live endpoint id.
    pub edge_id: PublicKey,
    /// Admission mode.
    pub admission: Admission,
    /// `admin pause` in force.
    pub paused: bool,
    /// Live tunnels.
    pub tunnels: u64,
    /// Live routes.
    pub routes: u64,
    /// Open visitor connections.
    pub visitors: u64,
    /// `names_max` reached (`/v1/info.full`).
    #[serde(default)]
    pub full: bool,
    /// Names holding a slot.
    #[serde(default)]
    pub names_slotted: u64,
    /// `names_max`.
    #[serde(default)]
    pub names_max: u64,
    /// Names waiting for a slot.
    #[serde(default)]
    pub queued: u64,
    /// `queue_max`.
    #[serde(default)]
    pub queue_max: u64,
    /// Non-invited tunnels throttled to `throttle_mbit` (§9.6).
    #[serde(default)]
    pub throttled: bool,
    /// Egress so far this UTC month, bytes.
    #[serde(default)]
    pub egress_month_bytes: u64,
    /// `egress_month` × elapsed fraction of the month: the pace above which the throttle engages.
    #[serde(default)]
    pub egress_pace_bytes: u64,
    /// The standby key this edge's ticket pins, when configured (§5.5).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub standby: Option<PublicKey>,
}

/// The bucket a certificate job drains (§6.5).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum QueueClass {
    /// The mesh entered by invite.
    Invited,
    /// Any other mesh.
    Open,
}

/// `cert_jobs.kind`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum JobKind {
    /// A new name: needs a slot.
    New,
    /// ARI renewal.
    Renew,
    /// Key rotation.
    Rotate,
}

/// One queued or running certificate job in `queue`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct QueueRow {
    /// The name.
    pub name: String,
    /// Owner mesh.
    pub mesh: PublicKey,
    /// Bucket.
    pub class: QueueClass,
    /// Job kind.
    pub kind: JobKind,
    /// 1-based position within `class` (0 while ordering).
    pub position: u32,
    /// ETA as answered to the client.
    pub eta_s: u64,
    /// The mesh's first name (ordered before additional names).
    pub first_of_mesh: bool,
    /// Network key (IPv4 /24 or IPv6 /48).
    pub net: String,
    /// Enqueue time.
    pub enqueued_ms: u64,
    /// Order attempts so far.
    pub attempts: u32,
}

/// `queue`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct QueueResult {
    /// Whole tokens in the invited bucket now.
    pub invited_tokens: u32,
    /// Whole tokens in the open bucket now.
    pub open_tokens: u32,
    /// Seconds until the next token of either bucket.
    pub next_token_s: u64,
    /// Jobs in drain order, invited class first.
    pub jobs: Vec<QueueRow>,
}

/// A ban record (`meshes.banned`, `names.banned`).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct BanInfo {
    /// Reason shown to the hosting engine.
    pub reason: String,
    /// When.
    pub ms: u64,
    /// Operator ref.
    pub by: String,
}

/// `mesh <id>`: names, engines, revoke list, rekey history (§8.5).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct MeshResult {
    /// The mesh id asked for.
    pub mesh: PublicKey,
    /// Entered by invite.
    pub invited: bool,
    /// Network key it was first seen from.
    pub net: String,
    /// Origin ASN, when the table knows it.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub asn: Option<u32>,
    /// First seen.
    pub created_ms: u64,
    /// Names held.
    pub names: Vec<NameInfo>,
    /// Engines seen.
    pub engines: Vec<MeshEngine>,
    /// Revoke list.
    pub revoked: Vec<PublicKey>,
    /// Applied rekeys touching this mesh, oldest first.
    pub rekeys: Vec<RekeyEntry>,
    /// Set when this id was rekeyed away.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub rekeyed_to: Option<PublicKey>,
    /// Last Stop all.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub stop_all_ms: Option<u64>,
    /// Ban, if any.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub banned: Option<BanInfo>,
}

/// `release <name>`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ReleaseResult {
    /// The name.
    pub name: String,
    /// The previous owner may reclaim until then.
    pub reclaim_after_ms: u64,
    /// Routes dropped.
    pub closed_routes: u64,
}

/// `evict-unpublished`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct EvictResult {
    /// Names evicted (records deleted, certificates revoked, no cooldown).
    pub evicted: Vec<String>,
}

/// `recover <name|mesh> --to-mesh <id>` (§5.7).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct RecoverResult {
    /// The mesh that held the names.
    pub from_mesh: PublicKey,
    /// The mesh that holds them now.
    pub to_mesh: PublicKey,
    /// Names moved.
    pub names: Vec<String>,
    /// Tunnels of `from_mesh` closed.
    pub closed_tunnels: u64,
}

/// `invite new`: the secret appears here once and never again.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct InviteMinted {
    /// The ticket carrying the invite, to hand out.
    pub ticket: ShareEdgeTicket,
    /// [`crate::Invite::hash_hex`], the id for `invite list` / `invite revoke`.
    pub hash: String,
    /// Uses left.
    pub uses: u32,
    /// Expiry.
    pub expires_ms: u64,
}

/// One `invites` row.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct InviteRow {
    /// Lowercase hex of the invite hash.
    pub hash: String,
    /// Uses left.
    pub uses_left: u32,
    /// Expiry.
    pub expires_ms: u64,
    /// Minted.
    pub created_ms: u64,
    /// Operator note.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub note: Option<String>,
}

/// `invite list`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct InviteListResult {
    /// Unexpired invites with uses left.
    pub invites: Vec<InviteRow>,
}

/// `invite revoke`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct InviteRevoked {
    /// Full hash of the revoked invite.
    pub hash: String,
}

/// `caa-check` (§8.2, daily re-check flips admission to `closed` on failure).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct CaaCheckResult {
    /// Every expected record resolved.
    pub ok: bool,
    /// CAA records found at `D`.
    pub found: Vec<String>,
    /// Expected records not found.
    pub missing: Vec<String>,
    /// Admission after the check.
    pub admission: Admission,
    /// When.
    pub checked_ms: u64,
}

/// `revoke-cert <name>`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct RevokeCertResult {
    /// The name.
    pub name: String,
    /// Reason sent to the CA.
    pub reason: RevokeReason,
}

/// `cert retry <name>`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct CertRetryResult {
    /// The name.
    pub name: String,
    /// The job made due.
    pub job_id: String,
    /// Its kind.
    pub kind: JobKind,
    /// Attempts it had before the reset.
    pub attempts_before: u32,
    /// Its last error, if any.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub last_error: Option<String>,
    /// New `next_at_ms` (now).
    pub next_at_ms: u64,
}

/// `reload`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ReloadResult {
    /// Config keys whose value changed.
    pub changed: Vec<String>,
    /// Changed keys that need a restart and were not applied.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub restart_required: Vec<String>,
}

/// A brake condition of §9.5.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum BrakeKind {
    /// An `edge_full` answer was given.
    EdgeFull,
    /// Queue ETA above 72 h.
    QueueEta,
    /// A Let's Encrypt bucket empty for a week.
    BucketEmpty,
    /// A Search Console security issue for `D`.
    SearchConsole,
    /// A second Safe Browsing hit within 7 days.
    SafeBrowsing,
    /// An abuse spike not cleared within a day.
    AbuseSpike,
    /// A condition this build does not know.
    #[serde(other)]
    Unknown,
}

/// One brake condition and whether it holds.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct BrakeCondition {
    /// Which.
    pub kind: BrakeKind,
    /// Holds now.
    pub holds: bool,
    /// Evidence (a count, an ETA, a date).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub detail: Option<String>,
}

/// `brake-status`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct BrakeStatus {
    /// Admission now.
    pub admission: Admission,
    /// Some condition holds: pull `admission invite`.
    pub pull: bool,
    /// Every condition.
    pub conditions: Vec<BrakeCondition>,
    /// Days since every condition last cleared (back to `open` after 7).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub clear_days: Option<u32>,
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

    #[test]
    fn m4_commands_and_typed_results() {
        let cmds = [
            AdminCommand::InviteNew {
                uses: Some(3),
                expires_s: None,
                note: Some("beta".into()),
            },
            AdminCommand::InviteList,
            AdminCommand::InviteRevoke {
                hash: "abcdef01".into(),
            },
            AdminCommand::RevokeCert {
                name: "sky".into(),
                reason: Some(RevokeReason::KeyCompromise),
            },
            AdminCommand::Queue,
            AdminCommand::CaaCheck,
            AdminCommand::BrakeStatus,
            AdminCommand::Reload,
            AdminCommand::CertRetry { name: "sky".into() },
        ];
        for (i, c) in cmds.into_iter().enumerate() {
            let r = AdminRequest {
                id: i as u64,
                command: c,
                reference: Some("T-9".into()),
            };
            let line = serde_json::to_string(&r).unwrap();
            assert_eq!(serde_json::from_str::<AdminRequest>(&line).unwrap(), r);
        }
        assert!(!AdminCommand::InviteList.is_mutating());
        assert!(AdminCommand::CertRetry { name: "x".into() }.is_mutating());
        let line = r#"{"id":1,"cmd":"cert-retry","args":{"name":"sky"}}"#;
        assert_eq!(
            serde_json::from_str::<AdminRequest>(line).unwrap().command,
            AdminCommand::CertRetry { name: "sky".into() }
        );
        let cr = CertRetryResult {
            name: "sky".into(),
            job_id: "j1".into(),
            kind: JobKind::New,
            attempts_before: 4,
            last_error: Some("dns".into()),
            next_at_ms: 9,
        };
        let back = AdminResponse::ok_typed(1, &cr);
        assert_eq!(back.result_as::<CertRetryResult>().unwrap().unwrap(), cr);
        assert!(AdminCommand::InviteRevoke { hash: "x".into() }.is_mutating());
        let line = r#"{"id":1,"cmd":"invite-revoke","args":{"hash":"ab"}}"#;
        assert!(matches!(
            serde_json::from_str::<AdminRequest>(line).unwrap().command,
            AdminCommand::InviteRevoke { .. }
        ));
        // Old CLI lines without the new optional args.
        let old: AdminRequest =
            serde_json::from_str(r#"{"id":1,"cmd":"revoke-cert","args":{"name":"sky"}}"#).unwrap();
        assert_eq!(
            old.command,
            AdminCommand::RevokeCert {
                name: "sky".into(),
                reason: None
            }
        );

        // The M1 edge's `status` object decodes as StatusResult.
        let edge = iroh_base::SecretKey::from_bytes(&[7; 32]).public();
        let m1 = serde_json::json!({"version": "0.1.0", "domain": "tun.zip", "edge_id": edge.to_string(),
            "admission": "open", "paused": false, "tunnels": 2, "routes": 3, "visitors": 4});
        let resp = AdminResponse::ok(5, m1);
        let s: StatusResult = resp.result_as().unwrap().unwrap();
        assert_eq!((s.edge_id, s.tunnels, s.throttled, s.standby), (edge, 2, false, None));
        let back = AdminResponse::ok_typed(5, &s);
        assert_eq!(back.result_as::<StatusResult>().unwrap().unwrap(), s);
        assert!(AdminResponse::err(1, ErrorCode::Invalid, "x").result_as::<StatusResult>().is_none());

        let b = BrakeStatus {
            admission: Admission::Open,
            pull: true,
            conditions: vec![BrakeCondition {
                kind: BrakeKind::QueueEta,
                holds: true,
                detail: Some("p95 80 h".into()),
            }],
            clear_days: None,
        };
        let j = serde_json::to_string(&b).unwrap();
        assert_eq!(serde_json::from_str::<BrakeStatus>(&j).unwrap(), b);
        let u: BrakeCondition = serde_json::from_str(r#"{"kind":"meteor","holds":false}"#).unwrap();
        assert_eq!(u.kind, BrakeKind::Unknown);
    }
}
