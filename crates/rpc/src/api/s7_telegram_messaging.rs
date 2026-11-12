use jsonrpsee::core::{RpcResult, async_trait};
use jsonrpsee::proc_macros::rpc;
use telegram::{PairingLink, TelegramStatus};
use rpc_core::{send, Server};
use telegram::rpc_support::telegram;

#[rpc(server, client)]
pub trait TelegramMessagingApi {
    /// Telegram bot identity, poller state, trust lists, groups and bound threads (98 §13).
    #[method(name = "telegram_status")]
    async fn telegram_status(&self) -> RpcResult<TelegramStatus>;
    /// Pair the owner (98 §10.4): with `owner`, set `telegram.owner` live and in the
    /// config file and answer `None`; without, mint a single-use code and answer the
    /// `t.me/<bot>?start=<code>` deep link (needs the daemon).
    #[method(name = "telegram_link")]
    async fn telegram_link(&self, owner: Option<u64>) -> RpcResult<Option<PairingLink>>;
    /// Add a user id to `telegram.allowed` (live and in the config file).
    #[method(name = "telegram_allow")]
    async fn telegram_allow(&self, user: u64) -> RpcResult<()>;
    /// Remove a user id from `telegram.allowed`.
    #[method(name = "telegram_remove_allowed")]
    async fn telegram_remove_allowed(&self, user: u64) -> RpcResult<()>;
    /// Add a chat id (`-100…`) to `telegram.groups` (live and in the config file).
    #[method(name = "telegram_allow_group")]
    async fn telegram_allow_group(&self, chat: i64) -> RpcResult<()>;
    /// Remove a chat id from `telegram.groups`.
    #[method(name = "telegram_remove_group")]
    async fn telegram_remove_group(&self, chat: i64) -> RpcResult<()>;
}

#[async_trait]
impl TelegramMessagingApiServer for Server {

    async fn telegram_status(&self) -> RpcResult<TelegramStatus> {
        send(&telegram()?, telegram::Status).await
    }

    async fn telegram_link(&self, owner: Option<u64>) -> RpcResult<Option<PairingLink>> {
        send(&telegram()?, telegram::Link { owner }).await
    }

    async fn telegram_allow(&self, user: u64) -> RpcResult<()> {
        send(&telegram()?, telegram::Allow(user)).await
    }

    async fn telegram_remove_allowed(&self, user: u64) -> RpcResult<()> {
        send(&telegram()?, telegram::RemoveAllowed(user)).await
    }

    async fn telegram_allow_group(&self, chat: i64) -> RpcResult<()> {
        send(&telegram()?, telegram::AllowGroup(chat)).await
    }

    async fn telegram_remove_group(&self, chat: i64) -> RpcResult<()> {
        send(&telegram()?, telegram::RemoveGroup(chat)).await
    }
}

pub(super) fn rpc_module(server: rpc_core::Server) -> jsonrpsee::RpcModule<rpc_core::Server> {
    TelegramMessagingApiServer::into_rpc(server)
}
