// FROZEN: copied from jarvis 9d74d5a crates/jarvis-signal/src/mock.rs (MockTransport half only).
// Protocol test double; edit only with a matching note in docs/design/09-signal.md §2.2.
//! Test double: [`MockTransport`] with scripted link attempts, connections
//! and envelopes, recording every send.

use std::collections::{HashMap, VecDeque};
use std::sync::atomic::{AtomicBool, AtomicU32, Ordering};
use std::sync::{Arc, Mutex, MutexGuard, PoisonError};

use async_trait::async_trait;
use futures::StreamExt;
use futures::stream::BoxStream;
use tokio::sync::mpsc;
use uuid::Uuid;

use crate::error::{Result, SignalError};
use crate::transport::{LinkAttempt, LinkUrl, Recipient, SignalEnvelope, SignalTransport};

fn lock<T>(mutex: &Mutex<T>) -> MutexGuard<'_, T> {
    mutex.lock().unwrap_or_else(PoisonError::into_inner)
}

/// One item recorded by [`MockTransport`] for a successful send.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SentItem {
    /// A text message.
    Text {
        /// Recipient number.
        to: String,
        /// Message body.
        text: String,
        /// Sent timestamp returned to the caller (theuth 09 §2.2 edit 2).
        timestamp: u64,
    },
    /// An attachment.
    Attachment {
        /// Recipient number.
        to: String,
        /// Attachment MIME type.
        mime: String,
        /// Attachment bytes.
        bytes: Vec<u8>,
        /// Optional file name (theuth 09 §2.2 edit 3).
        file_name: Option<String>,
        /// Optional caption.
        caption: Option<String>,
    },
    /// A typing-indicator update.
    Typing {
        /// Recipient number.
        to: String,
        /// Whether typing is active.
        active: bool,
    },
    /// A read receipt.
    Receipt {
        /// Recipient number.
        to: String,
        /// Acknowledged sender timestamp.
        ts: u64,
    },
    /// A reaction to a prior message.
    Reaction {
        /// Recipient number.
        to: String,
        /// Emoji glyph.
        emoji: String,
        /// Author of the reacted-to message (E.164 when known).
        target_author: String,
        /// Sender timestamp of the reacted-to message.
        target_ts: u64,
    },
}

enum Connection {
    Scripted {
        envelopes: Vec<SignalEnvelope>,
        hang: bool,
    },
    Live(mpsc::UnboundedReceiver<SignalEnvelope>),
}

/// One scripted provisioning attempt (see
/// [`MockTransport::script_link_attempt`]).
struct LinkScript {
    url: String,
    done: tokio::sync::oneshot::Receiver<Result<()>>,
}

struct Inner {
    connections: Mutex<VecDeque<Connection>>,
    sent: Mutex<Vec<SentItem>>,
    incoming_calls: AtomicU32,
    text_attempts: AtomicU32,
    fail_text_sends: AtomicU32,
    registered: AtomicBool,
    linked_names: Mutex<Vec<String>>,
    link_scripts: Mutex<VecDeque<LinkScript>>,
    account: Mutex<Option<Recipient>>,
    uuid_directory: Mutex<HashMap<String, Uuid>>,
    /// theuth 09 §2.2 edit 4: the full `Recipient` of every successful send,
    /// parallel to `sent` (which keeps only the number).
    recipients: Mutex<Vec<Recipient>>,
}

/// Scriptable in-memory [`SignalTransport`] for tests.
///
/// Script "connections" with [`MockTransport::script_connection`] (each
/// `incoming()` call consumes one; a non-hanging connection ends its
/// stream, driving the channel's reconnect path) or attach a live sender
/// with [`MockTransport::script_live`] to push envelopes while the channel
/// is running. All sends are recorded and inspectable via
/// [`MockTransport::sent`].
#[derive(Clone)]
pub struct MockTransport {
    inner: Arc<Inner>,
}

impl Default for MockTransport {
    fn default() -> Self {
        Self::new()
    }
}

impl MockTransport {
    /// Creates a registered mock transport with no scripted connections
    /// (streams hang, yielding nothing).
    #[must_use]
    pub fn new() -> Self {
        Self {
            inner: Arc::new(Inner {
                connections: Mutex::new(VecDeque::new()),
                sent: Mutex::new(Vec::new()),
                incoming_calls: AtomicU32::new(0),
                text_attempts: AtomicU32::new(0),
                fail_text_sends: AtomicU32::new(0),
                registered: AtomicBool::new(true),
                linked_names: Mutex::new(Vec::new()),
                link_scripts: Mutex::new(VecDeque::new()),
                account: Mutex::new(None),
                uuid_directory: Mutex::new(HashMap::new()),
                recipients: Mutex::new(Vec::new()),
            }),
        }
    }

    /// Scripts the next `link_device` provisioning attempt: it yields
    /// `url`, and the attempt's `done` future resolves with whatever the
    /// test sends on the returned sender (`Ok(())` = linked, `Err` =
    /// timed out). Dropping the sender unresolved fails the attempt.
    /// Unscripted attempts get a default URL and hang forever.
    #[must_use]
    pub fn script_link_attempt(
        &self,
        url: impl Into<String>,
    ) -> tokio::sync::oneshot::Sender<Result<()>> {
        let (tx, rx) = tokio::sync::oneshot::channel();
        lock(&self.inner.link_scripts).push_back(LinkScript {
            url: url.into(),
            done: rx,
        });
        tx
    }

    /// Sets the account identity reported by `account`.
    pub fn set_account(&self, account: Option<Recipient>) {
        *lock(&self.inner.account) = account;
    }

    /// Scripts a contact-store entry for `resolve_uuid`: the given E.164
    /// number resolves to `uuid` (like a synced presage contact).
    pub fn script_uuid(&self, number: impl Into<String>, uuid: Uuid) {
        lock(&self.inner.uuid_directory).insert(number.into(), uuid);
    }

    /// Scripts the next `incoming()` connection: it yields `envelopes`,
    /// then hangs open (`hang = true`) or ends (`hang = false`,
    /// triggering the channel's reconnect logic).
    pub fn script_connection(&self, envelopes: Vec<SignalEnvelope>, hang: bool) {
        lock(&self.inner.connections).push_back(Connection::Scripted { envelopes, hang });
    }

    /// Scripts a live connection; envelopes sent on the returned sender
    /// are yielded by the stream as they arrive.
    #[must_use]
    pub fn script_live(&self) -> mpsc::UnboundedSender<SignalEnvelope> {
        let (tx, rx) = mpsc::unbounded_channel();
        lock(&self.inner.connections).push_back(Connection::Live(rx));
        tx
    }

    /// Everything successfully sent so far.
    #[must_use]
    pub fn sent(&self) -> Vec<SentItem> {
        lock(&self.inner.sent).clone()
    }

    /// The `Recipient` of every successful send, in `sent()` order
    /// (theuth 09 §2.2 edit 4).
    #[must_use]
    pub fn sent_recipients(&self) -> Vec<Recipient> {
        lock(&self.inner.recipients).clone()
    }

    /// Number of times `incoming()` was called (reconnect counter).
    #[must_use]
    pub fn incoming_calls(&self) -> u32 {
        self.inner.incoming_calls.load(Ordering::SeqCst)
    }

    /// Total `send_text` attempts, including injected failures.
    #[must_use]
    pub fn text_attempts(&self) -> u32 {
        self.inner.text_attempts.load(Ordering::SeqCst)
    }

    /// Makes the next `n` `send_text` calls fail (retry testing).
    pub fn fail_next_text_sends(&self, n: u32) {
        self.inner.fail_text_sends.store(n, Ordering::SeqCst);
    }

    /// Sets the registration state reported by `is_registered`.
    pub fn set_registered(&self, registered: bool) {
        self.inner.registered.store(registered, Ordering::SeqCst);
    }

    /// Device names passed to `link_device`.
    #[must_use]
    pub fn linked_device_names(&self) -> Vec<String> {
        lock(&self.inner.linked_names).clone()
    }

    /// Polls until at least `count` items were sent (10 ms interval);
    /// callers should wrap in `tokio::time::timeout`.
    pub async fn wait_for_sent(&self, count: usize) {
        loop {
            if lock(&self.inner.sent).len() >= count {
                return;
            }
            tokio::time::sleep(std::time::Duration::from_millis(10)).await;
        }
    }
}

#[async_trait]
impl SignalTransport for MockTransport {
    async fn link_device(&self, device_name: &str) -> Result<LinkAttempt> {
        lock(&self.inner.linked_names).push(device_name.to_string());
        match lock(&self.inner.link_scripts).pop_front() {
            Some(script) => Ok(LinkAttempt {
                url: LinkUrl(script.url),
                done: Box::pin(async move {
                    script.done.await.unwrap_or_else(|_| {
                        Err(SignalError::Transport(
                            "provisioning attempt aborted (script sender dropped)".into(),
                        ))
                    })
                }),
            }),
            None => Ok(LinkAttempt {
                url: LinkUrl(format!("sgnl://linkdevice?uuid=mock&device={device_name}")),
                done: Box::pin(futures::future::pending()),
            }),
        }
    }

    async fn is_registered(&self) -> Result<bool> {
        Ok(self.inner.registered.load(Ordering::SeqCst))
    }

    async fn account(&self) -> Result<Option<Recipient>> {
        Ok(lock(&self.inner.account).clone())
    }

    async fn resolve_uuid(&self, number: &str) -> Result<Option<Uuid>> {
        Ok(lock(&self.inner.uuid_directory).get(number).copied())
    }

    fn incoming(&self) -> BoxStream<'static, SignalEnvelope> {
        self.inner.incoming_calls.fetch_add(1, Ordering::SeqCst);
        match lock(&self.inner.connections).pop_front() {
            None => futures::stream::pending().boxed(),
            Some(Connection::Scripted { envelopes, hang }) => {
                let scripted = futures::stream::iter(envelopes);
                if hang {
                    scripted.chain(futures::stream::pending()).boxed()
                } else {
                    scripted.boxed()
                }
            }
            Some(Connection::Live(rx)) => {
                futures::stream::unfold(
                    rx,
                    |mut rx| async move { rx.recv().await.map(|env| (env, rx)) },
                )
                .boxed()
            }
        }
    }

    async fn send_text(&self, to: &Recipient, text: &str) -> Result<u64> {
        self.inner.text_attempts.fetch_add(1, Ordering::SeqCst);
        let failures = self.inner.fail_text_sends.load(Ordering::SeqCst);
        if failures > 0 {
            self.inner
                .fail_text_sends
                .store(failures - 1, Ordering::SeqCst);
            return Err(SignalError::Transport("injected failure".into()));
        }
        // theuth 09 §2.2 edit 2: deterministic, strictly increasing timestamps
        // so tests can target a sent message with a reaction.
        lock(&self.inner.recipients).push(to.clone());
        let mut sent = lock(&self.inner.sent);
        let timestamp = 1_000_000 + sent.len() as u64;
        sent.push(SentItem::Text {
            to: to.number.clone(),
            text: text.to_string(),
            timestamp,
        });
        Ok(timestamp)
    }

    async fn send_attachment(
        &self,
        to: &Recipient,
        bytes: &[u8],
        mime: &str,
        file_name: Option<&str>,
        caption: Option<&str>,
    ) -> Result<()> {
        lock(&self.inner.recipients).push(to.clone());
        lock(&self.inner.sent).push(SentItem::Attachment {
            to: to.number.clone(),
            mime: mime.to_string(),
            bytes: bytes.to_vec(),
            file_name: file_name.map(str::to_string),
            caption: caption.map(str::to_string),
        });
        Ok(())
    }

    async fn send_typing(&self, to: &Recipient, active: bool) -> Result<()> {
        lock(&self.inner.recipients).push(to.clone());
        lock(&self.inner.sent).push(SentItem::Typing {
            to: to.number.clone(),
            active,
        });
        Ok(())
    }

    async fn send_receipt(&self, to: &Recipient, ts: u64) -> Result<()> {
        lock(&self.inner.recipients).push(to.clone());
        lock(&self.inner.sent).push(SentItem::Receipt {
            to: to.number.clone(),
            ts,
        });
        Ok(())
    }

    async fn send_reaction(
        &self,
        to: &Recipient,
        emoji: &str,
        target_author: &Recipient,
        target_ts: u64,
    ) -> Result<()> {
        lock(&self.inner.recipients).push(to.clone());
        lock(&self.inner.sent).push(SentItem::Reaction {
            to: to.number.clone(),
            emoji: emoji.to_string(),
            target_author: target_author.number.clone(),
            target_ts,
        });
        Ok(())
    }
}
