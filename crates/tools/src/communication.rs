//! Availability seam for the communication tools (50 §2.2, §8): the registry
//! consults one agent-owned source to decide whether `discover_peers`,
//! `send_message` and `spawn_subagent` exist for a session at all. This is
//! capability visibility, not a second allow/ask/deny policy: it can only
//! remove a route, never grant permission, and `Risk → PolicyAction` still
//! runs unchanged for every call that passes (50 §0.5, 12 §5).
//!
//! `peer_message` (receipt lookup) is deliberately *not* gated here: reading
//! the caller's own sent/received record stays available for audit after a
//! disable, so the model can learn that its accepted message was suppressed
//! (50 §2.2, §5.2). `agent` scopes what that record may show.
//!
//! Effective enablement (`global AND no self/ancestor opt-out`), scope and
//! parent computation live in `agent`; the registry never derives them.
//! Before [`crate::InstallCommunicationAccess`] runs there is no source, so
//! the gated names are disabled fail-closed.

use std::fmt;
use std::sync::Arc;

use runtime::SessionId;
use tokio_util::sync::CancellationToken;

use crate::ToolOutput;

/// Tool names the seam governs. Everything else is untouched.
pub const GATED: [&str; 3] = ["discover_peers", "send_message", "spawn_subagent"];

/// Stable prefix of every refusal this module produces, so callers and tests
/// can tell "communication is off" from "unknown tool" or "denied by policy".
pub const DISABLED: &str = "communication_disabled";

/// `name` is one of [`GATED`].
pub fn gated(name: &str) -> bool {
    GATED.contains(&name)
}

/// Agent-owned enablement source, installed once via
/// [`crate::InstallCommunicationAccess`]. Both methods are called inside the
/// registry mailbox and from invocation tasks: they must be synchronous,
/// cheap and never block (a lock held only for the read is fine, an actor
/// `ask` is not). Exposing a tool through this trait is usability only; the
/// same source is consulted again after any approval resolves (50 §8 items
/// 1–2), and `agent` still validates the full route at send admission.
pub trait CommunicationAccess: Send + Sync {
    /// `local_enabled(session)` as of now: the global switch and no opt-out on
    /// the session or any stored ancestor (50 §2.1).
    fn enabled(&self, session: SessionId) -> bool;

    /// The token of the generation under which `session` is currently
    /// admitted; `None` when the session is unknown or disabled (never a
    /// fabricated live token). `agent` cancels it when an opt-out, global-off,
    /// root scope change or deletion revokes that generation (before the
    /// setter reports success), and mints a *new* token on re-enable: a call
    /// admitted under the old one stays rejected however the switch reads
    /// later (50 §8). May return an already cancelled token; the registry
    /// treats that as disabled.
    fn generation(&self, session: SessionId) -> Option<CancellationToken>;
}

/// Why a gated call was refused.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Disabled {
    /// No [`CommunicationAccess`] installed yet: nothing has said communication is on.
    Uninstalled,
    /// The source reports the session (or an ancestor, or the harness) is off.
    Off,
    /// The call was admitted, then its generation was revoked before dispatch.
    Revoked,
}

impl fmt::Display for Disabled {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(match self {
            Self::Uninstalled => "communication is not configured for this harness",
            Self::Off => "communication is off for this session; /communication shows why",
            Self::Revoked => "communication was disabled while this call waited; nothing was sent",
        })
    }
}

/// The identifiable error output for a refused gated call:
/// `"<tool>: communication_disabled: <why>"`.
pub fn refused(tool: &str, why: Disabled) -> ToolOutput {
    ToolOutput::err(format!("{tool}: {DISABLED}: {why}"))
}

/// One gated call's admission: the source it was checked against and the
/// generation token it was admitted under. Re-checked after approval and
/// before secret expansion / dispatch, so an approval that resolved after a
/// disable cannot carry a stale authorization into the tool body (50 §8).
/// The tool body then runs under [`scope`], so the peer tool reads *this*
/// admission via [`current`] at real dispatch instead of asking the source
/// for a fresh generation: an off→on between the post-approval check and
/// `Deliver` mints a new token but never revives the one captured here.
#[derive(Clone)]
pub struct Admitted {
    access: Arc<dyn CommunicationAccess>,
    session: SessionId,
    token: CancellationToken,
}

impl Admitted {
    /// `Err` when the source is missing or says off; `Ok` captures the current generation.
    pub fn check(
        access: Option<&Arc<dyn CommunicationAccess>>,
        session: SessionId,
    ) -> Result<Self, Disabled> {
        let access = access.ok_or(Disabled::Uninstalled)?;
        let token = access.generation(session).ok_or(Disabled::Off)?;
        if token.is_cancelled() || !access.enabled(session) {
            return Err(Disabled::Off);
        }
        Ok(Self {
            access: Arc::clone(access),
            session,
            token,
        })
    }

    /// Still admitted: the generation was not revoked and the source still
    /// says on. The second read is fail-closed defence, not the contract: a
    /// disable must cancel the token.
    pub fn live(&self) -> bool {
        !self.token.is_cancelled() && self.access.enabled(self.session)
    }

    /// The generation token this call was admitted under. Hand it to `agent`
    /// at send admission; a cancelled token means the route was revoked
    /// after admission, whatever the switch reads now.
    pub fn token(&self) -> &CancellationToken {
        &self.token
    }
}

tokio::task_local! {
    static ADMITTED: Admitted;
}

/// Run `fut` with `admitted` readable via [`current`]; a `None` (ungated
/// tool) leaves `fut` untouched. Task-locals do not cross `tokio::spawn`:
/// wrap the future you spawn, not the one that awaits the handle.
pub fn scope<F: Future>(admitted: Option<Admitted>, fut: F) -> impl Future<Output = F::Output> {
    use futures::future::Either;
    match admitted {
        Some(a) => Either::Left(ADMITTED.scope(a, fut)),
        None => Either::Right(fut),
    }
}

/// The admission the current gated tool body was dispatched under; `None`
/// outside a [`scope`] (ungated tool, or a caller that bypassed the registry).
pub fn current() -> Option<Admitted> {
    ADMITTED.try_with(Clone::clone).ok()
}

impl fmt::Debug for Admitted {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("Admitted")
            .field("session", &self.session)
            .field("cancelled", &self.token.is_cancelled())
            .finish_non_exhaustive()
    }
}

#[cfg(test)]
mod tests {
    use std::sync::Mutex;
    use std::sync::atomic::{AtomicBool, Ordering};

    use super::*;
    use crate::jobs::lock;

    /// A source whose generation is replaced on re-enable, as `agent` does.
    struct Switch {
        on: AtomicBool,
        token: Mutex<CancellationToken>,
    }

    impl Switch {
        fn new() -> (Arc<Self>, Arc<dyn CommunicationAccess>) {
            let sw = Arc::new(Self {
                on: AtomicBool::new(true),
                token: Mutex::new(CancellationToken::new()),
            });
            (sw.clone(), sw)
        }
        fn set(&self, on: bool) {
            self.on.store(on, Ordering::SeqCst);
            if on {
                *lock(&self.token) = CancellationToken::new();
            } else {
                lock(&self.token).cancel();
            }
        }
    }

    impl CommunicationAccess for Switch {
        fn enabled(&self, _: SessionId) -> bool {
            self.on.load(Ordering::SeqCst)
        }
        fn generation(&self, _: SessionId) -> Option<CancellationToken> {
            Some(lock(&self.token).clone())
        }
    }

    #[tokio::test]
    async fn scope_carries_admission_across_spawn() {
        let (_, access) = Switch::new();
        let admitted = Admitted::check(Some(&access), SessionId::new()).unwrap();
        let original = admitted.token().clone();
        // Mirrors the registry: the *spawned* future is wrapped.
        let seen = tokio::spawn(scope(Some(admitted), async { current() }))
            .await
            .unwrap()
            .expect("body sees the admission");
        assert!(seen.live());
        // Same generation, not a copy: cancelling one cancels the other.
        original.cancel();
        assert!(seen.token().is_cancelled());
        // Outside any scope there is nothing to read; ungated bodies see none.
        assert!(current().is_none());
        assert!(scope(None, async { current() }).await.is_none());
    }

    #[tokio::test]
    async fn revoked_admission_stays_revoked_after_re_enable() {
        let (sw, access) = Switch::new();
        let session = SessionId::new();
        let admitted = Admitted::check(Some(&access), session).unwrap();
        sw.set(false);
        sw.set(true);
        // The switch reads on and a fresh generation is live, but the body
        // dispatched under the old admission must not regain it.
        assert!(access.enabled(session));
        assert!(!access.generation(session).unwrap().is_cancelled());
        assert!(!admitted.live());
        let seen = scope(Some(admitted), async { current().unwrap() }).await;
        assert!(seen.token().is_cancelled());
        assert!(!seen.live());
        // A new call admits under the new generation as usual.
        assert!(Admitted::check(Some(&access), session).unwrap().live());
    }
}
