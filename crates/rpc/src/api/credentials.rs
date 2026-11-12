use super::*;

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
    /// single field, `values` (`{ name: value }`) for a form, both `None` declines. Stored as
    /// `source: user`. `true` if it was still unanswered.
    #[method(name = "answer_secret")]
    async fn answer_secret(
        &self,
        id: secrets::RequestId,
        value: Option<String>,
        values: Option<HashMap<String, String>>,
    ) -> RpcResult<bool>;
    /// The unanswered replicated `secret_requests` rows, newest first (79 §6.6): what the
    /// attention inbox's `secret:<id>` items render (`title`, `fields`) and answer with
    /// `answer_secret` from any engine.
    #[method(name = "secret_requests_list")]
    async fn secret_requests_list(&self) -> RpcResult<Vec<secrets::SecretRequest>>;
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
        id: secrets::RequestId,
        value: Option<String>,
        values: Option<HashMap<String, String>>,
    ) -> RpcResult<bool> {
        send(
            &secrets()?,
            secrets::Answer {
                id,
                value,
                values: values.unwrap_or_default(),
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
}

/// Build this domain's RPC module.
pub fn rpc_module(server: rpc_core::Server) -> jsonrpsee::RpcModule<rpc_core::Server> {
    CredentialsApiServer::into_rpc(server)
}
