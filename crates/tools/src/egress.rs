//! Whether the running tool call may reach this machine's local network
//! (16 §5 as amended 2026-11-11). The registry scopes every tool body with
//! `!contact_scoped(session)` (183 Z3-R2): a contact's turn in an exported
//! chat, a contact's inbox and everything under it get `false`. Outside any
//! scope — a caller that bypassed the registry, a task spawned without
//! re-entering it — the answer is `false`: unknown is treated as a contact.
//! `web.allow_private_network` is the fetcher's half; both must say yes.

tokio::task_local! {
    static LAN: bool;
}

/// Run `fut` with the local network reachable (`lan`) or not. Task-locals do
/// not cross `spawn`: wrap the future you spawn.
pub fn scope<F: Future>(lan: bool, fut: F) -> impl Future<Output = F::Output> {
    LAN.scope(lan, fut)
}

/// The current scope's grant; `false` outside one (fail closed).
#[must_use]
pub fn lan() -> bool {
    LAN.try_with(|l| *l).unwrap_or(false)
}
