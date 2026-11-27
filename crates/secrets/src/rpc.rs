use crate as secrets;
use std::collections::HashMap;
use std::sync::Arc;

use jsonrpsee::Extensions;
use jsonrpsee::core::{RpcResult, async_trait};
use jsonrpsee::proc_macros::rpc;
use jsonrpsee::types::ErrorObjectOwned;
use kameo::actor::ActorRef;
use rpc_core::{ASK_FAILED, Server, send};
use secrets::connections::{
    AddConnection, Audit, BuildProbe, ConnConfig, ConnInfo, ConnInfoOf, ConnSettingsOf,
    Destination, GetExternalSettings, GetExternals, ListConnections, LockOnSleep, RemoveConnection,
    SetSetting,
};
use secrets::external::{
    ConnStatus, DeleteMode, DeleteOutcome, ExtRef, ExtVersion, ExternalBackend, ListPage,
    ListQuery, LoginForm, LoginStep, SsoBegin, parse_bare,
};
use secrets::rpc_support::secrets;
use secrets::settings::ExternalSettings;
use zeroize::Zeroizing;

#[rpc_core_macros::rpc_descriptors(owner = "secrets", descriptors = rpc_descriptors, audience = "owner")]
#[rpc(server, client)]
pub trait CredentialsApi {
    /// Every secret's name, note and version history (37 §3.1). Never values.
    #[method(name = "list_secrets")]
    async fn list_secrets(&self) -> RpcResult<secrets::SecretsList>;
    /// Store a new version of `name` as `source: user`; `note: None` keeps
    /// the old note. Unchanged value → the current info, nothing written.
    #[method(name = "set_secret")]
    async fn set_secret(
        &self,
        name: String,
        value: String,
        note: Option<String>,
    ) -> RpcResult<secrets::SecretInfo>;
    /// Copy version `back` (1 = previous … 4) of `name` to the top as a new version.
    #[method(name = "restore_secret")]
    async fn restore_secret(&self, name: String, back: u8) -> RpcResult<secrets::SecretInfo>;
    /// Remove `name` and every kept version. `Err` when unknown.
    #[method(name = "delete_secret")]
    async fn delete_secret(&self, name: String) -> RpcResult<()>;
    /// Answer a `SecretsEvent::Requested` on this or any engine (79 §6.6): `value` for a
    /// single field, `values` (`{ name: value }`) for several, both `None` declines. Stored as
    /// `source: user`. `expires_ms` (absolute Unix ms, in the future) makes every stored value
    /// expire and be deleted then; `None` = never. 221 §7.3: `destination` is the card's final
    /// destination for a request that stores into an external manager (`None` keeps the one the
    /// request named). `true` if it was still unanswered. D15: a request with a destination is
    /// answered on the engine that owns the asking session; another engine forwards it there.
    #[method(name = "answer_secret", with_extensions)]
    async fn answer_secret(
        &self,
        id: secrets::RequestId,
        value: Option<String>,
        values: Option<HashMap<String, String>>,
        expires_ms: Option<u64>,
        destination: Option<Destination>,
    ) -> RpcResult<bool>;
    /// The unanswered replicated `secret_requests` rows, newest first (79 §6.6): what the
    /// attention inbox's `secret:<id>` items render (`title`, `fields`) and answer with
    /// `answer_secret` from any engine.
    #[method(name = "secret_requests_list")]
    async fn secret_requests_list(&self) -> RpcResult<Vec<secrets::SecretRequest>>;

    /// 221 §6.3: this engine's external connections, with status, ask switches and settings.
    #[method(name = "list_secret_connections")]
    async fn list_secret_connections(&self) -> RpcResult<Vec<ConnInfo>>;
    /// §9.1 "Test": a throwaway backend for `cfg` probes the manager (AWS `ListSecrets` max 1;
    /// Bitwarden prelogin). Nothing is stored.
    #[method(name = "probe_secret_connection")]
    async fn probe_secret_connection(&self, conn: String, cfg: ConnConfig)
    -> RpcResult<ConnStatus>;
    /// Add a connection: its four always-ask entries first (D6), then `config.yaml`, then the
    /// backend. Settings start at their D20 defaults.
    #[method(name = "add_secret_connection")]
    async fn add_secret_connection(&self, conn: String, cfg: ConnConfig) -> RpcResult<ConnInfo>;
    /// Remove a connection: its credentials, config and always-ask entries. The owner's own
    /// press; any other caller gets the human-only card `secrets.connection.remove`.
    #[method(name = "remove_secret_connection", with_extensions)]
    async fn remove_secret_connection(&self, conn: String) -> RpcResult<()>;
    /// Log a connection in (credentials sealed in this engine's `meta`, never replicated). The
    /// owner's own press runs directly; any other caller gets the human-only card
    /// `secrets.connection_login`.
    #[method(name = "connection_login", with_extensions)]
    async fn connection_login(&self, conn: String, form: LoginForm) -> RpcResult<LoginStep>;
    /// §9.6: begin a Bitwarden SSO login. `org_identifier: Some` stores it on the connection,
    /// `None` uses the stored one. `redirect_port` is where the browser comes back
    /// (`http://localhost:<port>`, 8065..=8070; `None` = 8065). Open `authorize_url` in the browser,
    /// then call `connection_sso_complete`. Gated like `connection_login`.
    #[method(name = "connection_sso_begin", with_extensions)]
    async fn connection_sso_begin(
        &self,
        conn: String,
        org_identifier: Option<String>,
        redirect_port: Option<u16>,
    ) -> RpcResult<SsoBegin>;
    /// §9.6: finish the SSO login `state` with the code, or the whole address the browser landed
    /// on. `NeedTwoFactor`: call again with the same `state` and input plus `two_factor`
    /// (`[provider, code]`). `Done` = logged in, `Locked` (the unlock card follows). Gated like
    /// `connection_login`.
    #[method(name = "connection_sso_complete", with_extensions)]
    async fn connection_sso_complete(
        &self,
        conn: String,
        state: String,
        code_or_redirect_url: String,
        two_factor: Option<(String, String)>,
    ) -> RpcResult<LoginStep>;
    /// Unlock with the master password (never stored).
    #[method(name = "connection_unlock")]
    async fn connection_unlock(&self, conn: String, password: String) -> RpcResult<ConnInfo>;
    /// Lock now (drop keys / cached credentials).
    #[method(name = "connection_lock")]
    async fn connection_lock(&self, conn: String) -> RpcResult<ConnInfo>;
    /// Metadata of a connection's entries (`deleted`: the pending-deletion / trash view).
    #[method(name = "list_external_secrets")]
    async fn list_external_secrets(&self, conn: String, q: ListQuery) -> RpcResult<ListPage>;
    /// Answer an `ExternalUnlockRequested` card: `Some(password)` unlocks, `None` declines.
    /// `true` when the card was still waiting.
    #[method(name = "answer_unlock")]
    async fn answer_unlock(
        &self,
        id: secrets::RequestId,
        password: Option<String>,
    ) -> RpcResult<bool>;
    /// Set one D20 key of `conn`. Loosening from anything but the owner's own press raises the
    /// human-only card `secrets.setting`; tightening never asks.
    #[method(name = "set_connection_setting", with_extensions)]
    async fn set_connection_setting(
        &self,
        conn: String,
        key: String,
        value: serde_json::Value,
    ) -> RpcResult<()>;
    /// The harness-wide D20 settings (`secrets.external`).
    #[method(name = "get_external_settings")]
    async fn get_external_settings(&self) -> RpcResult<ExternalSettings>;
    /// Set one harness-wide D20 key; loosening asks like `set_connection_setting`.
    #[method(name = "set_external_setting", with_extensions)]
    async fn set_external_setting(&self, key: String, value: serde_json::Value) -> RpcResult<()>;
    /// The Secrets page's Delete. `id` is the entry's ref (`aws/prod/app/x`) or its id inside the
    /// connection. `Permanent` needs `allow_permanent_delete`. The owner's press is the consent;
    /// any other caller with the connection's `…:delete` always ask in force gets the human-only
    /// card `secrets.external.delete`.
    #[method(name = "delete_external_secret", with_extensions)]
    async fn delete_external_secret(
        &self,
        conn: String,
        id: String,
        mode: DeleteMode,
    ) -> RpcResult<DeleteOutcome>;
    /// The Secrets page's Restore of a recoverable delete. A non-press caller with the
    /// connection's `…:write` always ask in force gets the human-only card
    /// `secrets.external.restore`.
    #[method(name = "restore_external_secret", with_extensions)]
    async fn restore_external_secret(&self, conn: String, id: String) -> RpcResult<ExtVersion>;
    /// D25: the value at `ref` for the owner's eyes. The owner's own press only (never the
    /// internal connection, where the agent's halves live); refused with `allow_reveal` off.
    /// Every reveal is audited.
    #[method(name = "reveal_external_secret", with_extensions)]
    async fn reveal_external_secret(&self, conn: String, r#ref: String) -> RpcResult<String>;
    /// The Secrets page's Import: copy `ref`'s value into the local vault as a new version of
    /// `name` (source: the ref), as `secret_external_import` does. The owner's press is the
    /// consent (D25); any other caller with the connection's `…:read` always ask in force gets
    /// the human-only card `secrets.import`. The value is never returned.
    #[method(name = "import_external_secret", with_extensions)]
    async fn import_external_secret(
        &self,
        conn: String,
        r#ref: String,
        name: String,
        note: Option<String>,
    ) -> RpcResult<secrets::SecretInfo>;
    /// D20 `lock_on_sleep`: the desktop reports `sleep` or `screen_lock`; every connection with
    /// the setting on locks.
    #[method(name = "secrets_power_event")]
    async fn secrets_power_event(&self, kind: String) -> RpcResult<()>;
    /// §12.5: create a Bitwarden Send from the owner's dialog (`args` = `SendCreateArgs` without
    /// `connection`). The owner's press is the consent. Any other caller (the TUI, a guest) gets
    /// the human-only card `secrets.create_send` while the connection's `send_owner_only` is on
    /// or its `…:send` always ask is in force, and creates only on the owner's approval.
    #[method(name = "create_send", with_extensions)]
    async fn create_send(
        &self,
        conn: String,
        args: serde_json::Value,
    ) -> RpcResult<secrets::tools_send::SendRecord>;
    /// §12.4: the Sends of a connection (never the link).
    #[method(name = "list_sends")]
    async fn list_sends(&self, conn: String) -> RpcResult<Vec<secrets::tools_send::SendRecord>>;
    /// §12.4: revoke a Send. A non-press caller with the Send's connection `…:send` always ask
    /// in force (or a Send this engine has no row for) gets the human-only card
    /// `secrets.revoke_send`.
    #[method(name = "revoke_send", with_extensions)]
    async fn revoke_send(&self, id: String) -> RpcResult<()>;
    /// §12.5: a Send's link for the clipboard. A press gets it directly. The internal peer (where
    /// the agent's halves live) or a share guest gets it only after the owner approves the
    /// human-only card `secrets.copy_send_link`; any other caller gets that card while
    /// `copy_link_needs_press` is on.
    #[method(name = "copy_send_link", with_extensions)]
    async fn copy_send_link(&self, id: String) -> RpcResult<String>;
    /// §9.4 (U1): the unlock cards waiting on this engine, oldest first — what an app reseeds its
    /// cards from on (re)connect. Fields as `ExternalUnlockRequested`.
    #[method(name = "list_unlock_requests")]
    async fn list_unlock_requests(&self) -> RpcResult<Vec<secrets::external::UnlockRequestView>>;

    /// 225: the sealed vault of this engine (`engine: null`) or of a paired engine, forwarded
    /// over the mesh. Never key material.
    #[method(name = "vault_status", with_extensions)]
    async fn vault_status(&self, engine: Option<String>) -> RpcResult<secrets::seal::VaultStatus>;
    /// 225: unseal with the master password. The password travels only inside the mesh's
    /// end-to-end connection when `engine` is another machine; the KEK is derived there.
    /// `-32042` wrong password (throttled after 5 in a row).
    #[method(name = "vault_unseal", with_extensions)]
    async fn vault_unseal(
        &self,
        engine: Option<String>,
        password: String,
    ) -> RpcResult<secrets::seal::VaultStatus>;
    /// 225: lock reads now (needs a password or device method).
    #[method(name = "vault_seal", with_extensions)]
    async fn vault_seal(&self, engine: Option<String>) -> RpcResult<secrets::seal::VaultStatus>;
    /// 225: add the master password (≥ 12 characters); drops a legacy `file` method.
    #[method(name = "vault_add_password", with_extensions)]
    async fn vault_add_password(
        &self,
        engine: Option<String>,
        password: String,
    ) -> RpcResult<secrets::seal::VaultStatus>;
    /// 225: change the master password (the current one must open the vault).
    #[method(name = "vault_change_password", with_extensions)]
    async fn vault_change_password(
        &self,
        engine: Option<String>,
        old: String,
        new: String,
    ) -> RpcResult<secrets::seal::VaultStatus>;
    /// 225: remove an unlock method by id; never the last.
    #[method(name = "vault_remove_method", with_extensions)]
    async fn vault_remove_method(
        &self,
        engine: Option<String>,
        id: String,
    ) -> RpcResult<secrets::seal::VaultStatus>;
    /// 225: ask paired engine `owner` to become an unlock device of `engine`'s vault (the owner
    /// may show a prompt; the method appears once it approves).
    #[method(name = "vault_add_device", with_extensions)]
    async fn vault_add_device(
        &self,
        engine: Option<String>,
        owner: String,
    ) -> RpcResult<secrets::seal::VaultStatus>;
    /// 225: unlock / enrol prompts waiting on this engine (poll).
    #[method(name = "vault_unseal_requests")]
    async fn vault_unseal_requests(&self) -> RpcResult<Vec<secrets::seal::UnsealRequestView>>;
    /// 225: answer a prompt; the owner's own press only. `false` = already answered or expired.
    #[method(name = "vault_answer_unseal", with_extensions)]
    async fn vault_answer_unseal(&self, id: String, approve: bool, always: bool) -> RpcResult<bool>;
    /// 225: the machines this engine unlocks.
    #[method(name = "vault_unlock_devices")]
    async fn vault_unlock_devices(&self) -> RpcResult<Vec<secrets::seal::UnlockDevice>>;
    /// 225: "Unlock automatically" for one machine; the owner's own press only.
    #[method(name = "vault_set_auto_unlock", with_extensions)]
    async fn vault_set_auto_unlock(&self, node: String, auto: bool) -> RpcResult<()>;
    /// 225: stop unlocking a machine.
    #[method(name = "vault_forget_node", with_extensions)]
    async fn vault_forget_node(&self, node: String) -> RpcResult<()>;
    /// 225 engine-to-engine (mesh only): a sealed node asks this owner (`purpose` = `unlock` |
    /// `enroll`). The caller is the forwarded request's authenticated endpoint id.
    #[method(name = "vault_peer_ask", with_extensions)]
    #[rpc_audience("mesh")]
    async fn vault_peer_ask(
        &self,
        nonce: String,
        purpose: String,
        name: String,
    ) -> RpcResult<secrets::seal::AskReply>;
    /// 225 engine-to-engine (mesh only): the owner's approved key for the node's outstanding
    /// `nonce`; single use, bound to the owner the nonce was sent to.
    #[method(name = "vault_peer_grant", with_extensions)]
    #[rpc_audience("mesh")]
    async fn vault_peer_grant(
        &self,
        nonce: String,
        purpose: String,
        key: String,
        name: String,
    ) -> RpcResult<()>;
}

/// 225: the seal gate; absent when the plugin is not loaded.
fn gate() -> RpcResult<Arc<secrets::seal::Gate>> {
    secrets::seal::installed().ok_or_else(|| {
        ErrorObjectOwned::owned(rpc_core::NOT_RUNNING, "secrets vault unavailable", None::<()>)
    })
}

/// 225: a [`secrets::seal::SealError`] with its code (`-32052` sealed, `-32042` password). A
/// sealed error carries `data: {engine, methods}` (the engine's name and its method kinds).
fn seal_err(e: secrets::seal::SealError) -> ErrorObjectOwned {
    let data = match (&e, secrets::seal::installed()) {
        (secrets::seal::SealError::Sealed(_), Some(g)) => {
            let s = g.status();
            let mut methods: Vec<&str> = s.methods.iter().map(|m| m.kind.as_str()).collect();
            methods.sort_unstable();
            methods.dedup();
            Some(serde_json::json!({"engine": s.name, "methods": methods}))
        }
        _ => None,
    };
    ErrorObjectOwned::owned(e.code(), e.to_string(), data)
}

/// 225: run a `vault_*` call on `engine` over the mesh (D15's forward, `theuth/rpc/1`).
/// `Ok(None)`: here (no engine, this engine, or already forwarded).
async fn forward_vault<T: serde::de::DeserializeOwned>(
    ext: &Extensions,
    engine: &Option<String>,
    method: &'static str,
    params: serde_json::Value,
) -> RpcResult<Option<T>> {
    let (Some(engine), Some(fwd)) = (engine.as_deref().filter(|e| !e.is_empty()), FORWARD.get())
    else {
        return Ok(None);
    };
    match fwd(ext, engine, method, params).await? {
        Some(v) => serde_json::from_value(v).map(Some).map_err(failed),
        None => Ok(None),
    }
}

/// 225: answering a prompt or loosening auto-unlock is the owner's own press, never the
/// internal peer (the agent's halves, a forwarded call) or a share guest.
fn press_only(ext: &Extensions) -> RpcResult<()> {
    if human_press(ext) {
        Ok(())
    } else {
        Err(ErrorObjectOwned::owned(
            rpc_core::UNAUTHORIZED,
            "only the owner's own press answers a vault unlock",
            None::<()>,
        ))
    }
}

/// 225: the authenticated endpoint id of a call forwarded over `theuth/rpc/1`; anything else
/// is refused.
fn mesh_caller(ext: &Extensions) -> RpcResult<String> {
    secrets::seal::caller(ext).ok_or_else(|| {
        ErrorObjectOwned::owned(
            rpc_core::UNAUTHORIZED,
            "vault_peer_* is engine-to-engine over the mesh only",
            None::<()>,
        )
    })
}

fn failed(e: impl std::fmt::Display) -> ErrorObjectOwned {
    ErrorObjectOwned::owned(ASK_FAILED, e.to_string(), None::<()>)
}

/// D15: send `method` with positional `params` to engine `engine` over the mesh (the agent's
/// `rpc_forward::to_engine`). `Ok(None)`: run it here (this engine, or already forwarded).
pub type EngineForward =
    for<'a> fn(
        &'a Extensions,
        &'a str,
        &'static str,
        serde_json::Value,
    ) -> futures::future::BoxFuture<'a, RpcResult<Option<serde_json::Value>>>;

static FORWARD: std::sync::OnceLock<EngineForward> = std::sync::OnceLock::new();

/// Install the D15 forward once, before dispatch (the RPC composition does). Without it an
/// answer for another engine's destination request is refused with "answer it there".
pub fn install_engine_forward(f: EngineForward) {
    let _ = FORWARD.set(f);
}

/// 219 §2.4: the owner's own press (desktop/app socket, a paired device): authenticated, not a
/// share guest, not the internal peer (MCP console, ACP agent half, TUI, forwarded calls), still
/// open. A call forwarded from another engine is never a press.
pub fn human_press(ext: &Extensions) -> bool {
    ext.get::<rpc_core::RpcPeer>()
        .is_some_and(|p| p.guest().is_none() && !p.is_internal() && !p.is_closed())
}

/// D20: a loosening from a caller that is not the owner's press needs the human-only card
/// `tool` (with `args` on it); tightening, or a press, needs nothing. `approval` is the approval
/// actor (`None`: none running, so a loosening is refused).
pub async fn confirm_loosening(
    approval: Option<&ActorRef<::tools::approval::Approval>>,
    press: bool,
    loosens: bool,
    tool: &str,
    args: serde_json::Value,
) -> Result<(), String> {
    if press || !loosens {
        return Ok(());
    }
    let Some(approval) = approval else {
        return Err(format!(
            "{tool}: needs the owner's approval and no approval service is running"
        ));
    };
    let req = ::tools::approval::RequestApproval {
        session: runtime::SessionId(uuid::Uuid::nil()),
        tool: tool.to_owned(),
        args,
        risk: ::tools::Risk::Destructive,
        call_id: None,
        always_ask: None,
    };
    ::tools::approval::ask_human_only_on(approval, req)
        .await
        .map(|_| ())
        .map_err(|e| format!("{tool}: not approved ({e:#})"))
}

fn approval_actor() -> Option<ActorRef<::tools::approval::Approval>> {
    ActorRef::<::tools::approval::Approval>::lookup(::tools::APPROVAL_ACTOR)
        .ok()
        .flatten()
}

/// Whether the always ask `secret:<backend>/<conn>:<dir>` is in force. Fail-closed: no tool
/// registry, or a registry error, counts as asking.
async fn always_asks(backend: &str, conn: &str, dir: ::tools::secrets::Direction) -> bool {
    let key = ::tools::secrets::always_ask_key(backend, conn, dir);
    match ActorRef::<::tools::ToolRegistry>::lookup(::tools::REGISTRY_ACTOR) {
        Ok(Some(reg)) => reg
            .ask(::tools::GetAlwaysAsk { session: None })
            .await
            .map(|s| s.global.contains(&key))
            .unwrap_or(true),
        _ => true,
    }
}

/// The registered backend of `conn`.
async fn backend(conn: &str) -> RpcResult<Arc<dyn ExternalBackend>> {
    let ext = send(&secrets()?, GetExternals).await?;
    ext.by_conn(conn).ok_or_else(|| {
        failed(format!(
            "connection {conn} is not available on this engine (unknown, or no backend for it in this build)"
        ))
    })
}

/// A login of `conn` (`connection_login`, the SSO pair): the owner's press runs directly; any
/// other caller needs the human-only card `secrets.connection_login`. The card names the
/// connection and the method, never a credential.
async fn login_gate(ext: &Extensions, conn: &str, how: &str) -> RpcResult<()> {
    confirm_loosening(
        approval_actor().as_ref(),
        human_press(ext),
        true,
        "secrets.connection_login",
        serde_json::json!({ "connection": conn, "method": how }),
    )
    .await
    .map_err(failed)
}

/// `id` (a full ref `aws/prod/x#k` or a bare id) as an [`ExtRef`] of `b`.
fn ext_ref(b: &dyn ExternalBackend, id: &str) -> ExtRef {
    match parse_bare(id) {
        Some((backend, r)) if backend == b.backend() && r.conn == b.conn() => r,
        _ => ExtRef {
            conn: b.conn().to_owned(),
            id: id.to_owned(),
            key: None,
            back: 0,
        },
    }
}

/// Run a backend call under `secrets.external.timeout_secs`.
async fn bounded<T>(
    fut: impl Future<Output = Result<T, secrets::external::ExtError>>,
) -> RpcResult<T> {
    let secs = send(&secrets()?, GetExternalSettings).await?.timeout_secs;
    match tokio::time::timeout(std::time::Duration::from_secs(secs), fut).await {
        Ok(Ok(v)) => Ok(v),
        Ok(Err(e)) => Err(failed(e)),
        Err(_) => Err(failed(format!("the manager did not answer in {secs} s"))),
    }
}

/// The Send context, once a Bitwarden connection exists.
async fn send_cx() -> RpcResult<secrets::tools_send::SendCx> {
    send(&secrets()?, secrets::connections::GetSendCx)
        .await?
        .ok_or_else(|| failed("no Bitwarden connection on this engine"))
}

#[async_trait]
impl CredentialsApiServer for Server {
    async fn list_secrets(&self) -> RpcResult<secrets::SecretsList> {
        send(&secrets()?, secrets::List).await
    }

    async fn set_secret(
        &self,
        name: String,
        value: String,
        note: Option<String>,
    ) -> RpcResult<secrets::SecretInfo> {
        send(
            &secrets()?,
            secrets::Set {
                name,
                value,
                note,
                source: "user".into(),
            },
        )
        .await
    }

    async fn restore_secret(&self, name: String, back: u8) -> RpcResult<secrets::SecretInfo> {
        send(&secrets()?, secrets::Restore { name, back }).await
    }

    async fn delete_secret(&self, name: String) -> RpcResult<()> {
        send(&secrets()?, secrets::Delete { name }).await
    }

    async fn answer_secret(
        &self,
        ext: &Extensions,
        id: secrets::RequestId,
        value: Option<String>,
        values: Option<HashMap<String, String>>,
        expires_ms: Option<u64>,
        destination: Option<Destination>,
    ) -> RpcResult<bool> {
        let a = secrets()?;
        // D15: a destination request is written by the engine that owns the asking session; the
        // value travels once over the mesh RPC, never as a replicated row.
        if let Some(fwd) = FORWARD.get()
            && let Some(engine) = send(
                &a,
                secrets::ForeignOwner {
                    id,
                    destination: destination.is_some(),
                },
            )
            .await?
            && let Some(v) = fwd(
                ext,
                &engine,
                "answer_secret",
                serde_json::json!([id, value, values, expires_ms, destination]),
            )
            .await?
        {
            return serde_json::from_value(v).map_err(failed);
        }
        send(
            &a,
            secrets::AnswerTo {
                answer: secrets::Answer {
                    id,
                    value,
                    values: values.unwrap_or_default(),
                    expires_ms,
                },
                destination,
            },
        )
        .await
    }

    async fn secret_requests_list(&self) -> RpcResult<Vec<secrets::SecretRequest>> {
        send(
            &secrets()?,
            secrets::ListRequests {
                unanswered_only: true,
            },
        )
        .await
    }

    async fn list_secret_connections(&self) -> RpcResult<Vec<ConnInfo>> {
        send(&secrets()?, ListConnections).await
    }

    async fn probe_secret_connection(
        &self,
        conn: String,
        cfg: ConnConfig,
    ) -> RpcResult<ConnStatus> {
        let b = send(&secrets()?, BuildProbe { conn, cfg }).await?;
        bounded(b.probe()).await
    }

    async fn add_secret_connection(&self, conn: String, cfg: ConnConfig) -> RpcResult<ConnInfo> {
        let a = secrets()?;
        let info = send(
            &a,
            AddConnection {
                conn: conn.clone(),
                cfg,
            },
        )
        .await?;
        let _ = a
            .tell(Audit(format!("connection {}/{conn} added", info.backend)))
            .await;
        Ok(info)
    }

    async fn remove_secret_connection(&self, ext: &Extensions, conn: String) -> RpcResult<()> {
        let a = secrets()?;
        confirm_loosening(
            approval_actor().as_ref(),
            human_press(ext),
            true,
            "secrets.connection.remove",
            serde_json::json!({ "connection": conn }),
        )
        .await
        .map_err(failed)?;
        send(&a, RemoveConnection(conn.clone())).await?;
        let _ = a.tell(Audit(format!("connection {conn} removed"))).await;
        Ok(())
    }

    async fn connection_login(
        &self,
        ext: &Extensions,
        conn: String,
        form: LoginForm,
    ) -> RpcResult<LoginStep> {
        let b = backend(&conn).await?;
        login_gate(ext, &conn, "login").await?;
        bounded(b.login(form)).await
    }

    async fn connection_sso_begin(
        &self,
        ext: &Extensions,
        conn: String,
        org_identifier: Option<String>,
        redirect_port: Option<u16>,
    ) -> RpcResult<SsoBegin> {
        let b = backend(&conn).await?;
        login_gate(ext, &conn, "SSO login").await?;
        let org = send(
            &secrets()?,
            secrets::connections::SsoIdentifier {
                conn,
                set: org_identifier,
            },
        )
        .await?;
        bounded(b.sso_begin(&org, redirect_port)).await
    }

    async fn connection_sso_complete(
        &self,
        ext: &Extensions,
        conn: String,
        state: String,
        code_or_redirect_url: String,
        two_factor: Option<(String, String)>,
    ) -> RpcResult<LoginStep> {
        let b = backend(&conn).await?;
        login_gate(ext, &conn, "SSO login").await?;
        bounded(b.sso_complete(&state, &code_or_redirect_url, two_factor)).await
    }

    async fn connection_unlock(&self, conn: String, password: String) -> RpcResult<ConnInfo> {
        let b = backend(&conn).await?;
        // Argon2id runs about a second (§6.2); the bound is the external timeout all the same.
        bounded(b.unlock(Zeroizing::new(password))).await?;
        send(&secrets()?, ConnInfoOf(conn)).await
    }

    async fn connection_lock(&self, conn: String) -> RpcResult<ConnInfo> {
        backend(&conn).await?.lock().await;
        send(&secrets()?, ConnInfoOf(conn)).await
    }

    async fn list_external_secrets(&self, conn: String, q: ListQuery) -> RpcResult<ListPage> {
        let b = backend(&conn).await?;
        bounded(b.list(q)).await
    }

    async fn answer_unlock(
        &self,
        id: secrets::RequestId,
        password: Option<String>,
    ) -> RpcResult<bool> {
        let ext = send(&secrets()?, GetExternals).await?;
        Ok(ext.answer_unlock(id, password.map(Zeroizing::new)))
    }

    async fn list_unlock_requests(&self) -> RpcResult<Vec<secrets::external::UnlockRequestView>> {
        Ok(send(&secrets()?, GetExternals).await?.unlock_requests())
    }

    async fn vault_status(
        &self,
        ext: &Extensions,
        engine: Option<String>,
    ) -> RpcResult<secrets::seal::VaultStatus> {
        if let Some(v) = forward_vault(ext, &engine, "vault_status", serde_json::json!([null])).await? {
            return Ok(v);
        }
        Ok(gate()?.status())
    }

    async fn vault_unseal(
        &self,
        ext: &Extensions,
        engine: Option<String>,
        password: String,
    ) -> RpcResult<secrets::seal::VaultStatus> {
        let password = Zeroizing::new(password);
        if let Some(v) =
            forward_vault(ext, &engine, "vault_unseal", serde_json::json!([null, *password])).await?
        {
            return Ok(v);
        }
        let g = gate()?;
        g.unseal_password(password).await.map_err(seal_err)?;
        Ok(g.status())
    }

    async fn vault_seal(
        &self,
        ext: &Extensions,
        engine: Option<String>,
    ) -> RpcResult<secrets::seal::VaultStatus> {
        if let Some(v) = forward_vault(ext, &engine, "vault_seal", serde_json::json!([null])).await? {
            return Ok(v);
        }
        let g = gate()?;
        g.seal().map_err(seal_err)?;
        Ok(g.status())
    }

    async fn vault_add_password(
        &self,
        ext: &Extensions,
        engine: Option<String>,
        password: String,
    ) -> RpcResult<secrets::seal::VaultStatus> {
        let password = Zeroizing::new(password);
        if let Some(v) =
            forward_vault(ext, &engine, "vault_add_password", serde_json::json!([null, *password]))
                .await?
        {
            return Ok(v);
        }
        let g = gate()?;
        g.add_password(password).await.map_err(seal_err)?;
        Ok(g.status())
    }

    async fn vault_change_password(
        &self,
        ext: &Extensions,
        engine: Option<String>,
        old: String,
        new: String,
    ) -> RpcResult<secrets::seal::VaultStatus> {
        let (old, new) = (Zeroizing::new(old), Zeroizing::new(new));
        if let Some(v) = forward_vault(
            ext,
            &engine,
            "vault_change_password",
            serde_json::json!([null, *old, *new]),
        )
        .await?
        {
            return Ok(v);
        }
        let g = gate()?;
        g.change_password(old, new).await.map_err(seal_err)?;
        Ok(g.status())
    }

    async fn vault_remove_method(
        &self,
        ext: &Extensions,
        engine: Option<String>,
        id: String,
    ) -> RpcResult<secrets::seal::VaultStatus> {
        if let Some(v) =
            forward_vault(ext, &engine, "vault_remove_method", serde_json::json!([null, id])).await?
        {
            return Ok(v);
        }
        let g = gate()?;
        g.remove_method(&id).await.map_err(seal_err)?;
        Ok(g.status())
    }

    async fn vault_add_device(
        &self,
        ext: &Extensions,
        engine: Option<String>,
        owner: String,
    ) -> RpcResult<secrets::seal::VaultStatus> {
        if let Some(v) =
            forward_vault(ext, &engine, "vault_add_device", serde_json::json!([null, owner])).await?
        {
            return Ok(v);
        }
        let g = gate()?;
        g.add_device(&owner).await.map_err(seal_err)?;
        Ok(g.status())
    }

    async fn vault_unseal_requests(&self) -> RpcResult<Vec<secrets::seal::UnsealRequestView>> {
        Ok(gate()?.requests())
    }

    async fn vault_answer_unseal(
        &self,
        ext: &Extensions,
        id: String,
        approve: bool,
        always: bool,
    ) -> RpcResult<bool> {
        press_only(ext)?;
        gate()?.answer(&id, approve, always).await.map_err(seal_err)
    }

    async fn vault_unlock_devices(&self) -> RpcResult<Vec<secrets::seal::UnlockDevice>> {
        gate()?.unlock_devices().await.map_err(seal_err)
    }

    async fn vault_set_auto_unlock(&self, ext: &Extensions, node: String, auto: bool) -> RpcResult<()> {
        press_only(ext)?;
        gate()?.set_auto(&node, auto).await.map_err(seal_err)
    }

    async fn vault_forget_node(&self, ext: &Extensions, node: String) -> RpcResult<()> {
        press_only(ext)?;
        gate()?.forget_node(&node).await.map_err(seal_err)
    }

    async fn vault_peer_ask(
        &self,
        ext: &Extensions,
        nonce: String,
        purpose: String,
        name: String,
    ) -> RpcResult<secrets::seal::AskReply> {
        let caller = mesh_caller(ext)?;
        gate()?
            .on_ask(&caller, &nonce, &purpose, &name)
            .await
            .map_err(seal_err)
    }

    async fn vault_peer_grant(
        &self,
        ext: &Extensions,
        nonce: String,
        purpose: String,
        key: String,
        name: String,
    ) -> RpcResult<()> {
        let caller = mesh_caller(ext)?;
        let key = Zeroizing::new(key);
        gate()?
            .on_grant(&caller, &nonce, &purpose, &key, &name)
            .await
            .map_err(seal_err)
    }

    async fn set_connection_setting(
        &self,
        ext: &Extensions,
        conn: String,
        key: String,
        value: serde_json::Value,
    ) -> RpcResult<()> {
        set_setting(&secrets()?, human_press(ext), Some(conn), key, value)
            .await
            .map_err(failed)
    }

    async fn get_external_settings(&self) -> RpcResult<ExternalSettings> {
        send(&secrets()?, GetExternalSettings).await
    }

    async fn set_external_setting(
        &self,
        ext: &Extensions,
        key: String,
        value: serde_json::Value,
    ) -> RpcResult<()> {
        set_setting(&secrets()?, human_press(ext), None, key, value)
            .await
            .map_err(failed)
    }

    async fn delete_external_secret(
        &self,
        ext: &Extensions,
        conn: String,
        id: String,
        mode: DeleteMode,
    ) -> RpcResult<DeleteOutcome> {
        let b = backend(&conn).await?;
        let r = ext_ref(b.as_ref(), &id);
        let press = human_press(ext);
        let asks =
            !press && always_asks(b.backend(), &conn, ::tools::secrets::Direction::Delete).await;
        confirm_loosening(
            approval_actor().as_ref(),
            press,
            asks,
            "secrets.external.delete",
            serde_json::json!({ "connection": conn, "id": r.id, "mode": mode }),
        )
        .await
        .map_err(failed)?;
        let out = bounded(b.delete(&r, mode, None)).await?;
        let _ = secrets()?
            .tell(Audit(format!(
                "deleted {}/{conn}/{} ({mode:?}) from the Secrets page",
                b.backend(),
                r.id
            )))
            .await;
        Ok(out)
    }

    async fn restore_external_secret(
        &self,
        ext: &Extensions,
        conn: String,
        id: String,
    ) -> RpcResult<ExtVersion> {
        let b = backend(&conn).await?;
        let r = ext_ref(b.as_ref(), &id);
        let press = human_press(ext);
        let asks =
            !press && always_asks(b.backend(), &conn, ::tools::secrets::Direction::Write).await;
        confirm_loosening(
            approval_actor().as_ref(),
            press,
            asks,
            "secrets.external.restore",
            serde_json::json!({ "connection": conn, "id": r.id }),
        )
        .await
        .map_err(failed)?;
        bounded(b.restore(&r)).await
    }

    async fn reveal_external_secret(
        &self,
        ext: &Extensions,
        conn: String,
        r#ref: String,
    ) -> RpcResult<String> {
        reveal(&secrets()?, human_press(ext), &conn, &r#ref)
            .await
            .map_err(failed)
    }

    async fn import_external_secret(
        &self,
        ext: &Extensions,
        conn: String,
        r#ref: String,
        name: String,
        note: Option<String>,
    ) -> RpcResult<secrets::SecretInfo> {
        let a = secrets()?;
        let (backend, r) =
            parse_bare(&r#ref).ok_or_else(|| failed(format!("{ref} is not a reference")))?;
        if r.conn != conn {
            return Err(failed(format!("{ref} is not on connection {conn}")));
        }
        let press = human_press(ext);
        let asks = !press && always_asks(backend, &conn, ::tools::secrets::Direction::Read).await;
        confirm_loosening(
            approval_actor().as_ref(),
            press,
            asks,
            "secrets.import",
            serde_json::json!({ "ref": r#ref, "name": name }),
        )
        .await
        .map_err(failed)?;
        let cx = secrets::tools_external::ExtCx {
            actor: a.clone(),
            vault: send(&a, secrets::Handle).await?,
            externals: send(&a, GetExternals).await?,
        };
        let (info, _) = secrets::tools_external::import(
            &cx,
            &r#ref,
            &name,
            note,
            None,
            "the owner imports an entry",
        )
        .await
        .map_err(failed)?;
        let _ = a
            .tell(Audit(format!(
                "imported {ref} into {name} v{}",
                info.current
            )))
            .await;
        Ok(info)
    }

    async fn secrets_power_event(&self, kind: String) -> RpcResult<()> {
        if !matches!(kind.as_str(), "sleep" | "screen_lock") {
            return Err(failed(format!("unknown power event {kind:?}")));
        }
        for b in send(&secrets()?, LockOnSleep).await? {
            b.lock().await;
        }
        Ok(())
    }

    async fn create_send(
        &self,
        ext: &Extensions,
        conn: String,
        args: serde_json::Value,
    ) -> RpcResult<secrets::tools_send::SendRecord> {
        let press = human_press(ext);
        if !press {
            create_send_gate(&secrets()?, &conn, &args)
                .await
                .map_err(failed)?;
        }
        let cx = send_cx().await?;
        let mut args = args;
        if let Some(o) = args.as_object_mut() {
            o.insert("connection".into(), conn.into());
        }
        let args: secrets::tools_send::SendCreateArgs =
            serde_json::from_value(args).map_err(|e| failed(format!("create_send: {e}")))?;
        let call_id = format!("rpc-{}", uuid::Uuid::now_v7());
        let (record, _receipt) = secrets::tools_send::create_send(
            &cx,
            args,
            runtime::SessionId(uuid::Uuid::nil()),
            &call_id,
        )
        .await
        .map_err(failed)?;
        Ok(record)
    }

    async fn list_sends(&self, conn: String) -> RpcResult<Vec<secrets::tools_send::SendRecord>> {
        let cx = send_cx().await?;
        secrets::tools_send::list_records(&cx, Some(&conn))
            .await
            .map_err(failed)
    }

    async fn revoke_send(&self, ext: &Extensions, id: String) -> RpcResult<()> {
        let cx = send_cx().await?;
        let press = human_press(ext);
        if !press {
            let conn = secrets::tools_send::list_records(&cx, None)
                .await
                .map_err(failed)?
                .into_iter()
                .find(|r| r.id == id || r.link_secret.as_deref() == Some(id.as_str()))
                .map(|r| r.conn);
            let asks = match &conn {
                Some(c) => always_asks("bw", c, ::tools::secrets::Direction::Send).await,
                None => true,
            };
            confirm_loosening(
                approval_actor().as_ref(),
                press,
                asks,
                "secrets.revoke_send",
                serde_json::json!({ "send": id, "connection": conn }),
            )
            .await
            .map_err(failed)?;
        }
        secrets::tools_send::revoke(&cx, &id, None)
            .await
            .map(|_| ())
            .map_err(failed)
    }

    async fn copy_send_link(&self, ext: &Extensions, id: String) -> RpcResult<String> {
        let a = secrets()?;
        copy_link_gate(&a, ext, &id).await.map_err(failed)?;
        let cx = send_cx().await?;
        let link = secrets::tools_send::send_link(&cx, &id)
            .await
            .map_err(failed)?
            .ok_or_else(|| failed(format!("no Send {id} (unknown, revoked or deleted)")))?;
        let _ = a.tell(Audit(format!("copied the link of Send {id}"))).await;
        Ok(link.as_str().to_owned())
    }
}

/// R21: `create_send` from a caller that is not the owner's press (the TUI over the internal
/// connection, a share guest): the human-only card `secrets.create_send` while the connection's
/// `send_owner_only` is on or its `…:send` always ask is in force; `Ok` only on the owner's
/// approval.
pub async fn create_send_gate(
    actor: &ActorRef<secrets::SecretsActor>,
    conn: &str,
    args: &serde_json::Value,
) -> Result<(), String> {
    let owner_only = actor
        .ask(ConnSettingsOf(conn.to_owned()))
        .await
        .map_err(|e| e.to_string())?
        .send_owner_only;
    let asks = owner_only || always_asks("bw", conn, ::tools::secrets::Direction::Send).await;
    confirm_loosening(
        approval_actor().as_ref(),
        false,
        asks,
        "secrets.create_send",
        serde_json::json!({ "connection": conn, "args": args }),
    )
    .await
}

/// R3/R21: whether `copy_send_link` may return Send `id`'s link to this caller. The owner's
/// press: yes. The internal connection (where the agent's halves live) or a share guest: only
/// after the owner approves the human-only card `secrets.copy_send_link`, whatever
/// `copy_link_needs_press` says. Any other caller: that card while `copy_link_needs_press` is on.
pub async fn copy_link_gate(
    actor: &ActorRef<secrets::SecretsActor>,
    ext: &Extensions,
    id: &str,
) -> Result<(), String> {
    if human_press(ext) {
        return Ok(());
    }
    let agent_side = ext
        .get::<rpc_core::RpcPeer>()
        .is_none_or(|p| p.is_internal() || p.guest().is_some());
    let asks = agent_side
        || actor
            .ask(GetExternalSettings)
            .await
            .map_err(|e| e.to_string())?
            .copy_link_needs_press;
    confirm_loosening(
        approval_actor().as_ref(),
        false,
        asks,
        "secrets.copy_send_link",
        serde_json::json!({ "send": id }),
    )
    .await
}

/// `set_connection_setting` / `set_external_setting`: classify, ask the human for a loosening
/// that is not a press, then commit.
pub async fn set_setting(
    actor: &ActorRef<secrets::SecretsActor>,
    press: bool,
    conn: Option<String>,
    key: String,
    value: serde_json::Value,
) -> Result<(), String> {
    let ask = |dry_run| SetSetting {
        conn: conn.clone(),
        key: key.clone(),
        value: value.clone(),
        dry_run,
    };
    let loosens = actor.ask(ask(true)).await.map_err(|e| e.to_string())?;
    let args = serde_json::json!({ "connection": conn, "key": key, "value": value });
    confirm_loosening(
        approval_actor().as_ref(),
        press,
        loosens,
        "secrets.setting",
        args,
    )
    .await?;
    actor.ask(ask(false)).await.map_err(|e| e.to_string())?;
    let scope = conn.as_deref().unwrap_or("harness");
    let _ = actor
        .tell(Audit(format!("setting {scope}.{key} = {value}")))
        .await;
    Ok(())
}

/// D25 `reveal_external_secret`: the owner's press only; `allow_reveal` on; audited.
pub async fn reveal(
    actor: &ActorRef<secrets::SecretsActor>,
    press: bool,
    conn: &str,
    r#ref: &str,
) -> Result<String, String> {
    if !press {
        return Err("reveal is an owner action in the desktop app".to_owned());
    }
    let settings = actor
        .ask(ConnSettingsOf(conn.to_owned()))
        .await
        .map_err(|e| e.to_string())?;
    if !settings.allow_reveal {
        return Err(format!(
            "reveal is off for {conn} (Settings › Secrets › {conn} › Allow reveal)"
        ));
    }
    let ext = actor.ask(GetExternals).await.map_err(|e| e.to_string())?;
    let (backend, r) = parse_bare(r#ref).ok_or_else(|| format!("{ref} is not a reference"))?;
    if r.conn != conn {
        return Err(format!("{ref} is not on connection {conn}"));
    }
    let b = ext
        .get(backend, conn)
        .ok_or_else(|| format!("unknown connection {backend}/{conn} on this engine"))?;
    let timeout = actor
        .ask(GetExternalSettings)
        .await
        .map_err(|e| e.to_string())?
        .timeout_secs;
    let (value, _) = tokio::time::timeout(std::time::Duration::from_secs(timeout), b.get(&r))
        .await
        .map_err(|_| format!("{ref}: {backend} did not answer in {timeout} s"))?
        .map_err(|e| format!("{ref}: {e}"))?;
    actor
        .ask(Audit(format!("revealed {ref} to the owner")))
        .await
        .map_err(|e| e.to_string())?;
    Ok(value.as_str().to_owned())
}

/// Build this domain's RPC module.
pub fn rpc_module(server: rpc_core::Server) -> rpc_core::DescribedModule {
    rpc_core::DescribedModule::new(CredentialsApiServer::into_rpc(server), rpc_descriptors())
        .expect("secrets RPC descriptors")
}
