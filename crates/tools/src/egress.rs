//! Whether the running tool call may reach this machine's local network
//! (16 §5 as amended 2026-11-11). The registry scopes every tool body with
//! its `lan_allowed(session)`: `false` in a restricted (capped) contact state
//! (183 Z3-R2: the public web only) and for a call a plugin marked
//! ([`crate::SetNoLan`]). Full-share contact turns and full inboxes run with
//! the agent's own tools, so they get `true` like the owner's turns. Outside
//! any scope — a caller that bypassed the registry, a task spawned without
//! re-entering it — the answer is `false`: unknown is treated as restricted.
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
