//! The subprocess transport with its exit result kept (141 §4.3, R2A §5.1).
//!
//! SDK 3.1.0's `connect_with` no longer awaits the owned transport driver
//! once the foreground returns, so after theuth's `main` sees
//! `incoming_closed` the child's exit status and stderr tail (`Process
//! exited with exit status: N: …`) are usually still in flight and the
//! connection reports a bare `process exited`. [`ChildProcess`] wraps the
//! SDK's `AcpAgent` and publishes the driver's result into an [`ExitSlot`]
//! the `main` closure waits on for up to [`EXIT_GRACE`] before returning.

use std::sync::{Arc, Mutex};
use std::time::Duration;

use agent_client_protocol::schema::v1::Error;
use agent_client_protocol::{AcpAgent, Agent, Client, ConnectTo};
use tokio::sync::Notify;

/// How long `main` waits for the driver's exit result after the incoming
/// side closed. The SDK itself waits up to 1 s for stderr EOF after a
/// non-zero exit, so this covers that and a little scheduling slack.
pub(crate) const EXIT_GRACE: Duration = Duration::from_millis(1500);

/// Where the transport driver's result lands.
#[derive(Default)]
pub(crate) struct ExitSlot {
    result: Mutex<Option<Result<(), Error>>>,
    ready: Notify,
}

impl ExitSlot {
    fn set(&self, r: Result<(), Error>) {
        *self.result.lock().unwrap_or_else(|p| p.into_inner()) = Some(r);
        // `notify_one` stores a permit, so a `wait` that starts after `set`
        // returns at once; there is exactly one waiter (`main`).
        self.ready.notify_one();
    }

    fn get(&self) -> Option<Result<(), Error>> {
        self.result
            .lock()
            .unwrap_or_else(|p| p.into_inner())
            .clone()
    }

    /// The driver's result, waiting up to [`EXIT_GRACE`] for it; `None`
    /// when the driver is still running after that (a child that closed
    /// stdout but lingers — the SDK will kill it when the connection ends).
    pub(crate) async fn wait(&self) -> Option<Result<(), Error>> {
        if let Some(r) = self.get() {
            return Some(r);
        }
        let _ = tokio::time::timeout(EXIT_GRACE, self.ready.notified()).await;
        self.get()
    }
}

/// `AcpAgent` whose driver result is kept for `main` (141 §4.3).
pub(crate) struct ChildProcess {
    inner: AcpAgent,
    exit: Arc<ExitSlot>,
}

impl ChildProcess {
    pub(crate) fn new(inner: AcpAgent, exit: Arc<ExitSlot>) -> Self {
        Self { inner, exit }
    }
}

impl ConnectTo<Client> for ChildProcess {
    async fn connect_to(self, client: impl ConnectTo<Agent>) -> Result<(), Error> {
        let r = ConnectTo::<Client>::connect_to(self.inner, client).await;
        self.exit.set(r.clone());
        r
    }
}
