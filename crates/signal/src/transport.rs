// FROZEN: copied from jarvis 9d74d5a crates/jarvis-signal/src/transport.rs.
// Protocol code; edit only with a matching note in docs/design/09-signal.md §2.2.
//! Transport abstraction isolating the Signal protocol implementation
//! (spec 06).
//!
//! The channel service only ever talks to [`SignalTransport`]; presage (or
//! the sanctioned signal-cli fallback) can be swapped without touching the
//! rest of the system.

use async_trait::async_trait;
use futures::stream::BoxStream;
use uuid::Uuid;

use crate::error::Result;

/// A Signal contact messages are sent to / received from.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct Recipient {
    /// Phone number in E.164 form (stable identity used for config
    /// matching and conversation mapping).
    pub number: String,
    /// Signal account UUID (ACI), when known.
    pub uuid: Option<Uuid>,
}

impl Recipient {
    /// Creates a recipient from a phone number.
    #[must_use]
    pub fn new(number: impl Into<String>) -> Self {
        Self {
            number: number.into(),
            uuid: None,
        }
    }
}

/// Device-linking provisioning URL, rendered as a QR code by clients
/// (terminal on first boot, UI onboarding via `signal.status` +
/// `SubsystemStatus` detail).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LinkUrl(pub String);

impl std::fmt::Display for LinkUrl {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.0)
    }
}

/// One linked-device provisioning attempt (spec 06, T39).
///
/// `url` is available as soon as the Signal server issues it; `done`
/// resolves when the attempt ends — `Ok(())` when the primary device
/// scanned the QR and linking completed, `Err` when the attempt timed
/// out or failed. Provisioning URLs are single-use and short-lived
/// (the provisioning websocket holds ~90 s), so on `Err` the channel
/// starts a fresh attempt and publishes the new URL.
pub struct LinkAttempt {
    /// Provisioning URL to render as a QR code.
    pub url: LinkUrl,
    /// Resolves when this attempt completes (`Ok`) or fails/times out
    /// (`Err`).
    pub done: futures::future::BoxFuture<'static, Result<()>>,
}

impl std::fmt::Debug for LinkAttempt {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("LinkAttempt")
            .field("url", &self.url)
            .finish_non_exhaustive()
    }
}

/// An attachment carried by an incoming envelope, already downloaded by
/// the transport.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct IncomingAttachment {
    /// Raw attachment bytes.
    pub bytes: Vec<u8>,
    /// MIME type reported by the sender.
    pub mime: String,
    /// Original file name, when provided.
    pub file_name: Option<String>,
    /// Whether the sender flagged this attachment as a voice note.
    pub voice_note: bool,
    /// Audio duration in milliseconds, when known.
    pub duration_ms: Option<u64>,
}

/// A reaction the sender put on one of our messages (theuth 09 §2.2 edit 1).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct IncomingReaction {
    /// The emoji.
    pub emoji: String,
    /// `sent_timestamp` of the message reacted to (what `send_text` returned).
    pub target_timestamp: u64,
    /// The reaction was removed, not added.
    pub remove: bool,
}

/// One incoming Signal message, normalized by the transport.
#[derive(Debug, Clone, PartialEq)]
pub struct SignalEnvelope {
    /// Message sender.
    pub sender: Recipient,
    /// Sender timestamp (milliseconds since epoch); echoed back in read
    /// receipts.
    pub timestamp: u64,
    /// Text body, when present.
    pub text: Option<String>,
    /// Text of the message this is a swipe-reply to, when present.
    ///
    /// Signal clients set `DataMessage.quote` on reply; only the quoted
    /// body text is kept (attachments/thumbnails are ignored).
    pub quote_text: Option<String>,
    /// Downloaded attachments.
    pub attachments: Vec<IncomingAttachment>,
    /// A reaction instead of a message (theuth 09 §2.2 edit 1); `text` and
    /// `attachments` are empty when set.
    pub reaction: Option<IncomingReaction>,
}

/// Signal protocol transport (spec 06).
///
/// `PresageTransport` is the v1 implementation; `SignalCliTransport`
/// (JSON-RPC to a signal-cli daemon) is the sanctioned fallback — same
/// trait, no other changes. Tests use `MockTransport`.
#[async_trait]
pub trait SignalTransport: Send + Sync {
    /// Starts one linked-device provisioning attempt: the returned
    /// [`LinkAttempt`] carries the URL to render as a QR code plus a
    /// future resolving when the attempt completes or times out (the
    /// channel then chains a fresh attempt, spec 06 linking state
    /// machine).
    async fn link_device(&self, device_name: &str) -> Result<LinkAttempt>;

    /// Whether the account is linked and ready to send/receive.
    async fn is_registered(&self) -> Result<bool>;

    /// The linked account's identity (E.164 number and/or ACI UUID),
    /// when registered and known. Default: unknown.
    async fn account(&self) -> Result<Option<Recipient>> {
        Ok(None)
    }

    /// Resolves an E.164 phone number to the contact's ACI UUID, when the
    /// transport knows the contact (e.g. the presage contact store synced
    /// from the primary device). Sealed-sender envelopes identify the
    /// sender only by ACI, so the channel uses this to recognize the
    /// configured owner even when no phone number is available on the
    /// envelope. Default: unknown.
    async fn resolve_uuid(&self, _number: &str) -> Result<Option<Uuid>> {
        Ok(None)
    }

    /// Stream of incoming envelopes. The stream ending signals a
    /// disconnect; the caller (channel supervisor) reconnects with
    /// backoff by calling this again.
    fn incoming(&self) -> BoxStream<'static, SignalEnvelope>;

    /// Sends a text message; returns its sent timestamp (ms), the target of
    /// any later reaction to it (theuth 09 §2.2 edit 2).
    async fn send_text(&self, to: &Recipient, text: &str) -> Result<u64>;

    /// Sends an attachment with an optional file name (shown for non-media
    /// types; theuth 09 §2.2 edit 3) and an optional caption.
    async fn send_attachment(
        &self,
        to: &Recipient,
        bytes: &[u8],
        mime: &str,
        file_name: Option<&str>,
        caption: Option<&str>,
    ) -> Result<()>;

    /// Sets or clears the typing indicator.
    async fn send_typing(&self, to: &Recipient, active: bool) -> Result<()>;

    /// Sends a read receipt for a sender timestamp.
    async fn send_receipt(&self, to: &Recipient, ts: u64) -> Result<()>;

    /// Reacts to a prior message with `emoji` (e.g. "👀").
    ///
    /// `target_ts` is the sender timestamp of the message being reacted to;
    /// `target_author` is that message's author (usually the peer).
    async fn send_reaction(
        &self,
        to: &Recipient,
        emoji: &str,
        target_author: &Recipient,
        target_ts: u64,
    ) -> Result<()>;
}
