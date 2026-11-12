use kameo::actor::Recipient;
use runtime::SessionId;

/// `tell`: one local client of `session` is gone.
#[derive(Clone, Copy, Debug)]
pub struct Unfollow(pub SessionId);

/// A lease on the original relay actor, released when the last holder drops it.
pub struct Following {
    session: SessionId,
    actor: Recipient<Unfollow>,
}

impl Following {
    /// Hold the original actor recipient until the lease is dropped.
    pub fn new(session: SessionId, actor: Recipient<Unfollow>) -> Self {
        Self { session, actor }
    }
}

impl Drop for Following {
    fn drop(&mut self) {
        let _ = self.actor.tell(Unfollow(self.session)).try_send();
    }
}
