//! Bot API façade over `tgbot::api::Client` (design doc 98 §4): the channel's
//! own request types, [`trait Sender`](Sender), the pacer, call counters and
//! the exact-description error table. With `poll::classify` this is the only
//! module that names a `tgbot` type (98 §2.2).

use std::collections::{BTreeMap, HashMap, VecDeque};
use std::sync::{Arc, OnceLock};

use parking_lot::{Mutex, MutexGuard};
use std::time::Duration;

use async_trait::async_trait;
use futures::StreamExt;
use tgbot::api::{Client, ExecuteError, Method};
use tgbot::types::{
    AnswerCallbackQuery, BotCommand, BotCommandScope, ChatAction, CloseForumTopic,
    CreateForumTopic, DeleteMessage, DeleteWebhook, EditForumTopic, EditMessageReplyMarkup,
    EditMessageText, EphemeralMessageParameters, GetBot, GetFile, GetUpdates, InlineKeyboardButton,
    InlineKeyboardMarkup, InputFileReader, InputRichMessage, ReactionType, ReplyParameters,
    SendChatAction, SendDocument, SendMessage, SendPhoto, SendRichMessage, SendRichMessageDraft,
    SendVoice, SetBotCommands, SetMessageReaction,
};
use tokio::time::Instant;
use tracing::Instrument;

use crate::{BotIdentity, CallStats, TelegramEvent};

/// Where a message goes: a chat and, in topics chats, the topic (`message_thread_id`).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct Target {
    /// Chat id (negative for groups and supergroups).
    pub chat: i64,
    /// Topic id; `None` = the General area or a flat chat.
    pub thread: Option<i64>,
}

/// One message the bot sent or saw.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct MsgRef {
    /// Chat id.
    pub chat: i64,
    /// `message_id` inside the chat.
    pub id: i64,
}

/// An inline keyboard (98 §10).
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Keyboard {
    /// Rows of buttons, ≤ 8 per row, ≤ 100 in all.
    pub rows: Vec<Vec<Button>>,
    /// Bot API 10.3 `force_reply`: a typed answer arrives as a reply to this message.
    pub force_reply: bool,
}

/// One inline keyboard button.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Button {
    /// A press arrives as a `callback_query` with `data` (≤ 64 bytes).
    Callback {
        /// Label.
        text: String,
        /// `callback_data`.
        data: String,
    },
    /// Opens a URL.
    Url {
        /// Label.
        text: String,
        /// Target.
        url: String,
    },
    /// A greyed-out button (a resolved approval's verdict).
    Disabled {
        /// Label.
        text: String,
    },
}

/// A file to upload.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct OutFile {
    /// File name shown to the user.
    pub name: String,
    /// MIME type; chooses photo / voice / document (98 §8.10).
    pub mime: String,
    /// Content.
    pub bytes: Vec<u8>,
}

/// Ephemeral delivery (Bot API 10.2): visible only to `user`.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Ephemeral {
    /// Receiver.
    pub user: u64,
    /// The press this answers, when it answers one.
    pub callback_query_id: Option<String>,
}

/// `setMyCommands` scope (98 §10.3).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum CommandScope {
    /// Everyone.
    Default,
    /// Group and supergroup chats.
    AllGroupChats,
    /// One chat (the owner's).
    Chat(i64),
}

/// One registered command.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct BotCommandSpec {
    /// Name without the slash, `a-z0-9_`.
    pub name: String,
    /// 3–256 characters.
    pub description: String,
    /// Bot API 10.2: the command and its reply are visible only to the invoker.
    pub ephemeral: bool,
}

/// What a call can fail with.
#[derive(Debug, thiserror::Error)]
pub enum ApiError {
    /// `ok: false` with Telegram's verbatim description.
    #[error("telegram {code}: {description}")]
    Api {
        /// `error_code`.
        code: i64,
        /// `description`, matched exactly against the §4.3 table.
        description: String,
    },
    /// Still rate limited after the client's retries.
    #[error("rate limited; retry after {0:?}")]
    RateLimited(Duration),
    /// HTTP or payload failure (token already scrubbed by tgbot).
    #[error("transport: {0}")]
    Transport(String),
    /// A download over the caller's cap or over Telegram's 20 MB.
    #[error("too big")]
    TooBig,
}

impl ApiError {
    /// Telegram's description for [`ApiError::Api`].
    #[must_use]
    pub fn description(&self) -> Option<&str> {
        match self {
            ApiError::Api { description, .. } => Some(description),
            _ => None,
        }
    }

    /// Exact description match (21 §4.1: never by substring).
    #[must_use]
    pub fn is(&self, exact: &str) -> bool {
        self.description() == Some(exact)
    }

    /// Prefix match for the two descriptions Telegram appends detail to.
    #[must_use]
    pub fn starts_with(&self, prefix: &str) -> bool {
        self.description().is_some_and(|d| d.starts_with(prefix))
    }
}

// 98 §4.3: Telegram's verbatim descriptions (teloxide's errors.rs is the reference list).
/// Another poller on this token.
pub const CONFLICT: &str = "Conflict: terminated by other getUpdates request; make sure that only one bot instance is running";
/// An edit that changed nothing (prefix; Telegram appends detail).
pub const NOT_MODIFIED_PREFIX: &str = "Bad Request: message is not modified";
/// The message is gone.
pub const TO_EDIT_NOT_FOUND: &str = "Bad Request: message to edit not found";
/// The message is too old or not ours.
pub const CANT_BE_EDITED: &str = "Bad Request: message can't be edited";
/// The user blocked the bot.
pub const BOT_BLOCKED: &str = "Forbidden: bot was blocked by the user";
/// The user's account is gone.
pub const USER_DEACTIVATED: &str = "Forbidden: user is deactivated";
/// No such chat (or the bot never saw it).
pub const CHAT_NOT_FOUND: &str = "Bad Request: chat not found";
/// The bot was removed from the supergroup.
pub const KICKED_SUPERGROUP: &str = "Forbidden: bot was kicked from the supergroup chat";
/// The topic was closed.
pub const TOPIC_CLOSED: &str = "Bad Request: TOPIC_CLOSED";
/// The topic was deleted.
pub const THREAD_NOT_FOUND: &str = "Bad Request: message thread not found";
/// `getFile` over the 20 MB cap.
pub const FILE_TOO_BIG: &str = "Bad Request: file is too big";

/// The chat cannot be written to: unbind the thread, publish `Unreachable` (98 §4.3).
#[must_use]
pub fn is_unreachable(e: &ApiError) -> bool {
    e.is(BOT_BLOCKED) || e.is(USER_DEACTIVATED) || e.is(CHAT_NOT_FOUND) || e.is(KICKED_SUPERGROUP)
}

/// The edit target is gone or frozen: the bubble rotates (98 §8.5).
#[must_use]
pub fn is_gone_message(e: &ApiError) -> bool {
    e.is(TO_EDIT_NOT_FOUND) || e.is(CANT_BE_EDITED)
}

/// The topic was closed or deleted: unbind it (98 §6.1).
#[must_use]
pub fn is_topic_gone(e: &ApiError) -> bool {
    e.is(TOPIC_CLOSED) || e.is(THREAD_NOT_FOUND)
}

/// An edit with identical content: ignore (98 §4.3).
#[must_use]
pub fn is_not_modified(e: &ApiError) -> bool {
    e.starts_with(NOT_MODIFIED_PREFIX)
}

/// Every Bot API call the channel makes, behind a trait so the workers are
/// testable against [`mock::MockSender`].
#[async_trait]
pub trait Sender: Send + Sync + 'static {
    /// `sendRichMessage { markdown }`; the new message id.
    async fn send_rich(
        &self,
        to: &Target,
        md: &str,
        markup: Option<&Keyboard>,
        reply_to: Option<i64>,
        ephemeral: Option<&Ephemeral>,
    ) -> Result<i64, ApiError>;
    /// `editMessageText` with a rich message.
    async fn edit_rich(
        &self,
        at: &MsgRef,
        md: &str,
        markup: Option<&Keyboard>,
    ) -> Result<(), ApiError>;
    /// `sendRichMessageDraft` (private chats only; 30 s lifetime).
    async fn draft(
        &self,
        to: &Target,
        draft_id: i64,
        md: &str,
        can_stop: bool,
    ) -> Result<(), ApiError>;
    /// Plain `sendMessage`, no `parse_mode`; the caller chunks to ≤ 4096.
    async fn send_text(
        &self,
        to: &Target,
        text: &str,
        reply_to: Option<i64>,
    ) -> Result<i64, ApiError>;
    /// `editMessageReplyMarkup`; `None` removes the keyboard.
    async fn edit_markup(&self, at: &MsgRef, markup: Option<&Keyboard>) -> Result<(), ApiError>;
    /// `setMessageReaction`; `None` clears.
    async fn react(&self, at: &MsgRef, emoji: Option<&str>) -> Result<(), ApiError>;
    /// `sendChatAction typing` (lives 5 s).
    async fn typing(&self, to: &Target) -> Result<(), ApiError>;
    /// `answerCallbackQuery` with an optional toast.
    async fn answer_callback(&self, id: &str, toast: Option<&str>) -> Result<(), ApiError>;
    /// Photo (`image/*` ≤ 10 MB) | voice (`audio/mpeg|ogg|mp4`) | document (98 §8.10).
    async fn send_file(
        &self,
        to: &Target,
        file: &OutFile,
        caption: Option<&str>,
        reply_to: Option<i64>,
    ) -> Result<i64, ApiError>;
    /// `getFile` + download; `TooBig` before fetching when `file_size > max`.
    /// Returns the bytes and the file name from `file_path`.
    async fn download(
        &self,
        file_id: &str,
        max: u64,
    ) -> Result<(Vec<u8>, Option<String>), ApiError>;
    /// `createForumTopic`; the topic id.
    async fn create_topic(&self, chat: i64, name: &str) -> Result<i64, ApiError>;
    /// `editForumTopic { name }`.
    async fn rename_topic(&self, chat: i64, topic: i64, name: &str) -> Result<(), ApiError>;
    /// `closeForumTopic`.
    async fn close_topic(&self, chat: i64, topic: i64) -> Result<(), ApiError>;
    /// `deleteMessage`.
    async fn delete_message(&self, at: &MsgRef) -> Result<(), ApiError>;
    /// `setMyCommands` for one scope.
    async fn set_commands(
        &self,
        scope: CommandScope,
        cmds: &[BotCommandSpec],
    ) -> Result<(), ApiError>;
    /// `deleteWebhook` (a webhook left by a previous life makes `getUpdates` 409).
    async fn delete_webhook(&self) -> Result<(), ApiError>;
    /// `getMe`.
    async fn me(&self) -> Result<BotIdentity, ApiError>;
    /// Per-method counters for `Status`.
    fn stats(&self) -> BTreeMap<String, CallStats>;
}

/// `getUpdates` result: the id beside the typed update.
#[derive(Clone, Debug)]
pub struct RawUpdate {
    /// `update_id`.
    pub id: i64,
    /// The update; `poll::classify` reads it.
    pub update: tgbot::types::Update,
}

/// Minimum gaps per key (98 §4.2).
const CHAT_GAP: Duration = Duration::from_millis(1000);
const GROUP_GAP: Duration = Duration::from_millis(3000);
const DRAFT_GAP: Duration = Duration::from_millis(1000);
const EDIT_GAP: Duration = Duration::from_millis(1500);
/// Overall bucket: calls per second.
const BUCKET: usize = 30;

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
enum PaceKey {
    Chat(i64),
    Draft(i64, Option<i64>),
    Edit(i64, i64),
}

/// Per-key next-allowed instants plus the last second's call instants.
#[derive(Default)]
struct Pacer {
    next: HashMap<PaceKey, Instant>,
    recent: VecDeque<Instant>,
}

impl Pacer {
    /// Reserves a slot for `keys` now, or says how long to wait.
    fn reserve(&mut self, keys: &[(PaceKey, Duration)]) -> Option<Duration> {
        let now = Instant::now();
        while self
            .recent
            .front()
            .is_some_and(|t| now.duration_since(*t) >= Duration::from_secs(1))
        {
            self.recent.pop_front();
        }
        let mut wait = Duration::ZERO;
        if self.recent.len() >= BUCKET
            && let Some(oldest) = self.recent.front()
        {
            wait = wait.max(*oldest + Duration::from_secs(1) - now);
        }
        for (k, _) in keys {
            if let Some(t) = self.next.get(k)
                && *t > now
            {
                wait = wait.max(*t - now);
            }
        }
        if wait > Duration::ZERO {
            return Some(wait);
        }
        for (k, gap) in keys {
            self.next.insert(*k, now + *gap);
        }
        self.recent.push_back(now);
        // ponytail: `next` grows one entry per chat/message ever paced; prune if a long-lived daemon ever notices.
        None
    }
}

fn chat_gap(chat: i64) -> Duration {
    if chat < 0 { GROUP_GAP } else { CHAT_GAP }
}

/// The live client: `tgbot::api::Client` with `with_max_retries(2)` and
/// `with_max_retry_after(30)`, the pacer and the counters (98 §4.1).
pub struct Api {
    client: Client,
    pacer: Mutex<Pacer>,
    calls: Mutex<BTreeMap<String, CallStats>>,
    on_rate_limited: OnceLock<Arc<dyn Fn(TelegramEvent) + Send + Sync>>,
}

impl Api {
    /// Builds the client. `host` overrides `https://api.telegram.org` (tests).
    pub fn new(token: &str, host: Option<&str>) -> Result<Api, ApiError> {
        if rustls::crypto::CryptoProvider::get_default().is_none() {
            let _ = rustls::crypto::aws_lc_rs::default_provider().install_default();
        }
        let mut client = Client::new(token)
            .map_err(|e| ApiError::Transport(format!("build client: {e}")))?
            .with_max_retries(2)
            .with_max_retry_after(30);
        if let Some(h) = host {
            client = client.with_host(h);
        }
        Ok(Api {
            client,
            pacer: Mutex::new(Pacer::default()),
            calls: Mutex::new(BTreeMap::new()),
            on_rate_limited: OnceLock::new(),
        })
    }

    /// Receives `TelegramEvent::RateLimited` when a call is still limited after
    /// the client's retries (98 §4.1). Set once by the channel.
    pub fn on_event(&self, f: Arc<dyn Fn(TelegramEvent) + Send + Sync>) {
        let _ = self.on_rate_limited.set(f);
    }

    /// The raw client, for `poll` only.
    #[must_use]
    pub fn client(&self) -> &Client {
        &self.client
    }

    /// `getUpdates`. `reset_allowed` sends `allowed_updates: []` (Telegram's
    /// default set, which includes `stopped_message_generation`); otherwise the
    /// field is omitted and the previous setting stays (98 §3.1).
    pub async fn get_updates(
        &self,
        offset: i64,
        limit: u8,
        timeout_secs: u64,
        reset_allowed: bool,
    ) -> Result<Vec<RawUpdate>, ApiError> {
        let mut m = GetUpdates::new()
            .with_offset(offset)
            .with_limit(i64::from(limit))
            .with_timeout(Duration::from_secs(timeout_secs));
        if reset_allowed {
            m = m.with_allowed_updates([]);
        }
        let updates = self.call("getUpdates", m).await?;
        Ok(updates
            .into_iter()
            .map(|u| RawUpdate {
                id: u.id,
                update: u,
            })
            .collect())
    }

    fn lock_calls(&self) -> MutexGuard<'_, BTreeMap<String, CallStats>> {
        self.calls.lock()
    }

    async fn pace(&self, keys: &[(PaceKey, Duration)]) {
        loop {
            let wait = self.pacer.lock().reserve(keys);
            match wait {
                None => return,
                Some(d) => tokio::time::sleep(d).await,
            }
        }
    }

    /// Executes `m` inside a `telegram.api` span, converts the error and bumps
    /// the counters.
    async fn call<M>(&self, method: &'static str, m: M) -> Result<M::Response, ApiError>
    where
        M: Method,
        M::Response: serde::de::DeserializeOwned + Send + 'static,
    {
        let span = tracing::debug_span!("telegram.api", method);
        async {
            let out = self.client.execute(m).await.map_err(convert);
            let mut calls = self.lock_calls();
            let c = calls.entry(method.to_owned()).or_default();
            match &out {
                Ok(_) => c.ok += 1,
                Err(ApiError::RateLimited(d)) => {
                    c.rate_limited += 1;
                    drop(calls);
                    tracing::warn!(
                        method,
                        retry_after_s = d.as_secs(),
                        "telegram rate limited after retries"
                    );
                    if let Some(f) = self.on_rate_limited.get() {
                        f(TelegramEvent::RateLimited {
                            method: method.to_owned(),
                            retry_after_secs: d.as_secs(),
                        });
                    }
                }
                Err(e) => {
                    c.err += 1;
                    drop(calls);
                    tracing::warn!(method, error = %e, "telegram call failed");
                }
            }
            out
        }
        .instrument(span)
        .await
    }
}

fn convert(e: ExecuteError) -> ApiError {
    match e {
        ExecuteError::Response(r) => match r.retry_after() {
            Some(secs) => ApiError::RateLimited(Duration::from_secs(secs)),
            None => ApiError::Api {
                code: r.error_code().unwrap_or(0),
                description: r.description().to_owned(),
            },
        },
        ExecuteError::Http(e) => ApiError::Transport(e.to_string()),
        ExecuteError::Payload(e) => ApiError::Transport(e.to_string()),
    }
}

fn keyboard(k: &Keyboard) -> InlineKeyboardMarkup {
    let rows = k.rows.iter().map(|row| {
        row.iter().map(|b| match b {
            Button::Callback { text, data } => InlineKeyboardButton::for_callback_data(text, data),
            Button::Url { text, url } => InlineKeyboardButton::for_url(text, url),
            Button::Disabled { text } => InlineKeyboardButton::disabled(text),
        })
    });
    let mut m = InlineKeyboardMarkup::from(rows);
    if k.force_reply {
        m = m.with_force_reply(true);
    }
    m
}

/// `sendPhoto` cap (98 §2.2).
const PHOTO_MAX: usize = 10 * 1024 * 1024;

/// Which upload method a file takes (98 §8.10).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum FileKind {
    /// `sendPhoto`.
    Photo,
    /// `sendVoice`.
    Voice,
    /// `sendDocument`.
    Document,
}

/// Photo for `image/*` ≤ 10 MB, voice for MP3/OGG/M4A, document otherwise.
#[must_use]
pub fn file_kind(mime: &str, len: usize) -> FileKind {
    if mime.starts_with("image/") && len <= PHOTO_MAX {
        FileKind::Photo
    } else if matches!(mime, "audio/mpeg" | "audio/ogg" | "audio/mp4") {
        FileKind::Voice
    } else {
        FileKind::Document
    }
}

fn reader(file: &OutFile) -> InputFileReader {
    let mime = file
        .mime
        .parse()
        .unwrap_or(tgbot::types::mime::APPLICATION_OCTET_STREAM);
    InputFileReader::new(std::io::Cursor::new(file.bytes.clone()))
        .with_file_name(file.name.clone())
        .with_mime_type(mime)
}

#[async_trait]
impl Sender for Api {
    async fn send_rich(
        &self,
        to: &Target,
        md: &str,
        markup: Option<&Keyboard>,
        reply_to: Option<i64>,
        ephemeral: Option<&Ephemeral>,
    ) -> Result<i64, ApiError> {
        self.pace(&[(PaceKey::Chat(to.chat), chat_gap(to.chat))])
            .await;
        // 98 §8.1 (a): bare `<`/`&` outside code would be read as HTML by Rich Markdown;
        // guarding here covers every rich send of every module in one place.
        let md = crate::ui::guard(md);
        let mut m = SendRichMessage::new(to.chat, InputRichMessage::markdown(md));
        if let Some(t) = to.thread {
            m = m.with_message_thread_id(t);
        }
        if let Some(k) = markup {
            m = m.with_reply_markup(keyboard(k));
        }
        if let Some(id) = reply_to {
            m = m.with_reply_parameters(ReplyParameters::new(id));
        }
        if let Some(e) = ephemeral {
            let mut p = EphemeralMessageParameters::from(i64::try_from(e.user).unwrap_or(i64::MAX));
            if let Some(cb) = &e.callback_query_id {
                p = p.with_callback_query_id(cb.clone());
            }
            m = m.with_ephemeral_message_parameters(p);
        }
        Ok(self.call("sendRichMessage", m).await?.id)
    }

    async fn edit_rich(
        &self,
        at: &MsgRef,
        md: &str,
        markup: Option<&Keyboard>,
    ) -> Result<(), ApiError> {
        self.pace(&[
            (PaceKey::Chat(at.chat), chat_gap(at.chat)),
            (PaceKey::Edit(at.chat, at.id), EDIT_GAP),
        ])
        .await;
        let md = crate::ui::guard(md);
        let mut m =
            EditMessageText::for_chat_message_rich(at.chat, at.id, InputRichMessage::markdown(md));
        if let Some(k) = markup {
            m = m.with_reply_markup(keyboard(k));
        }
        self.call("editMessageText", m).await.map(|_| ())
    }

    async fn draft(
        &self,
        to: &Target,
        draft_id: i64,
        md: &str,
        can_stop: bool,
    ) -> Result<(), ApiError> {
        self.pace(&[(PaceKey::Draft(to.chat, to.thread), DRAFT_GAP)])
            .await;
        let md = crate::ui::guard(md);
        let mut m = SendRichMessageDraft::new(to.chat, draft_id, InputRichMessage::markdown(md))
            .with_can_stop(can_stop);
        if let Some(t) = to.thread {
            m = m.with_message_thread_id(t);
        }
        self.call("sendRichMessageDraft", m).await.map(|_| ())
    }

    async fn send_text(
        &self,
        to: &Target,
        text: &str,
        reply_to: Option<i64>,
    ) -> Result<i64, ApiError> {
        self.pace(&[(PaceKey::Chat(to.chat), chat_gap(to.chat))])
            .await;
        let mut m = SendMessage::new(to.chat, text);
        if let Some(t) = to.thread {
            m = m.with_message_thread_id(t);
        }
        if let Some(id) = reply_to {
            m = m.with_reply_parameters(ReplyParameters::new(id));
        }
        Ok(self.call("sendMessage", m).await?.id)
    }

    async fn edit_markup(&self, at: &MsgRef, markup: Option<&Keyboard>) -> Result<(), ApiError> {
        self.pace(&[
            (PaceKey::Chat(at.chat), chat_gap(at.chat)),
            (PaceKey::Edit(at.chat, at.id), EDIT_GAP),
        ])
        .await;
        let mut m = EditMessageReplyMarkup::for_chat_message(at.chat, at.id);
        if let Some(k) = markup {
            m = m.with_reply_markup(keyboard(k));
        }
        self.call("editMessageReplyMarkup", m).await.map(|_| ())
    }

    async fn react(&self, at: &MsgRef, emoji: Option<&str>) -> Result<(), ApiError> {
        self.pace(&[]).await;
        let m =
            SetMessageReaction::new(at.chat, at.id).with_reaction(emoji.map(ReactionType::emoji));
        self.call("setMessageReaction", m).await.map(|_| ())
    }

    async fn typing(&self, to: &Target) -> Result<(), ApiError> {
        self.pace(&[]).await;
        let mut m = SendChatAction::new(to.chat, ChatAction::Typing);
        if let Some(t) = to.thread {
            m = m.with_message_thread_id(t);
        }
        self.call("sendChatAction", m).await.map(|_| ())
    }

    async fn answer_callback(&self, id: &str, toast: Option<&str>) -> Result<(), ApiError> {
        self.pace(&[]).await;
        let mut m = AnswerCallbackQuery::new(id);
        if let Some(t) = toast {
            m = m.with_text(t);
        }
        self.call("answerCallbackQuery", m).await.map(|_| ())
    }

    async fn send_file(
        &self,
        to: &Target,
        file: &OutFile,
        caption: Option<&str>,
        reply_to: Option<i64>,
    ) -> Result<i64, ApiError> {
        self.pace(&[(PaceKey::Chat(to.chat), chat_gap(to.chat))])
            .await;
        let reply = reply_to.map(ReplyParameters::new);
        let msg = match file_kind(&file.mime, file.bytes.len()) {
            FileKind::Photo => {
                let mut m = SendPhoto::new(to.chat, reader(file));
                if let Some(t) = to.thread {
                    m = m.with_message_thread_id(t);
                }
                if let Some(c) = caption {
                    m = m.with_caption(c);
                }
                if let Some(r) = reply {
                    m = m.with_reply_parameters(r);
                }
                self.call("sendPhoto", m).await?
            }
            FileKind::Voice => {
                let mut m = SendVoice::new(to.chat, reader(file));
                if let Some(t) = to.thread {
                    m = m.with_message_thread_id(t);
                }
                if let Some(c) = caption {
                    m = m.with_caption(c);
                }
                if let Some(r) = reply {
                    m = m.with_reply_parameters(r);
                }
                self.call("sendVoice", m).await?
            }
            FileKind::Document => {
                let mut m = SendDocument::new(to.chat, reader(file));
                if let Some(t) = to.thread {
                    m = m.with_message_thread_id(t);
                }
                if let Some(c) = caption {
                    m = m.with_caption(c);
                }
                if let Some(r) = reply {
                    m = m.with_reply_parameters(r);
                }
                self.call("sendDocument", m).await?
            }
        };
        Ok(msg.id)
    }

    async fn download(
        &self,
        file_id: &str,
        max: u64,
    ) -> Result<(Vec<u8>, Option<String>), ApiError> {
        self.pace(&[]).await;
        let f = match self.call("getFile", GetFile::new(file_id)).await {
            Err(e) if e.is(FILE_TOO_BIG) => return Err(ApiError::TooBig),
            r => r?,
        };
        if f.file_size
            .is_some_and(|s| u64::try_from(s).unwrap_or(u64::MAX) > max)
        {
            return Err(ApiError::TooBig);
        }
        let path = f
            .file_path
            .ok_or_else(|| ApiError::Transport("getFile answered without file_path".into()))?;
        let name = path.rsplit('/').next().map(str::to_owned);
        let mut stream = self
            .client
            .download_file(&path)
            .await
            .map_err(|e| ApiError::Transport(e.to_string()))?;
        let mut out = Vec::new();
        while let Some(chunk) = stream.next().await {
            let chunk = chunk.map_err(|e| ApiError::Transport(e.to_string()))?;
            if (out.len() + chunk.len()) as u64 > max {
                return Err(ApiError::TooBig);
            }
            out.extend_from_slice(&chunk);
        }
        Ok((out, name))
    }

    async fn create_topic(&self, chat: i64, name: &str) -> Result<i64, ApiError> {
        self.pace(&[]).await;
        Ok(self
            .call("createForumTopic", CreateForumTopic::new(chat, name))
            .await?
            .message_thread_id)
    }

    async fn rename_topic(&self, chat: i64, topic: i64, name: &str) -> Result<(), ApiError> {
        self.pace(&[]).await;
        self.call(
            "editForumTopic",
            EditForumTopic::new(chat, topic).with_name(name),
        )
        .await
        .map(|_| ())
    }

    async fn close_topic(&self, chat: i64, topic: i64) -> Result<(), ApiError> {
        self.pace(&[]).await;
        self.call("closeForumTopic", CloseForumTopic::new(chat, topic))
            .await
            .map(|_| ())
    }

    async fn delete_message(&self, at: &MsgRef) -> Result<(), ApiError> {
        self.pace(&[]).await;
        self.call("deleteMessage", DeleteMessage::new(at.chat, at.id))
            .await
            .map(|_| ())
    }

    async fn set_commands(
        &self,
        scope: CommandScope,
        cmds: &[BotCommandSpec],
    ) -> Result<(), ApiError> {
        self.pace(&[]).await;
        let mut list = Vec::with_capacity(cmds.len());
        for c in cmds {
            let mut cmd = BotCommand::new(c.name.clone(), c.description.clone())
                .map_err(|e| ApiError::Transport(format!("command /{}: {e}", c.name)))?;
            if c.ephemeral {
                cmd = cmd.with_is_ephemeral(true);
            }
            list.push(cmd);
        }
        let scope = match scope {
            CommandScope::Default => BotCommandScope::Default,
            CommandScope::AllGroupChats => BotCommandScope::AllGroupChats,
            CommandScope::Chat(id) => BotCommandScope::chat(id),
        };
        self.call("setMyCommands", SetBotCommands::new(list).with_scope(scope))
            .await
            .map(|_| ())
    }

    async fn delete_webhook(&self) -> Result<(), ApiError> {
        self.pace(&[]).await;
        self.call("deleteWebhook", DeleteWebhook::default())
            .await
            .map(|_| ())
    }

    async fn me(&self) -> Result<BotIdentity, ApiError> {
        self.pace(&[]).await;
        let bot = self.call("getMe", GetBot).await?;
        Ok(BotIdentity {
            id: u64::try_from(bot.id).unwrap_or(0),
            username: bot.username,
            topics: bot.has_topics_enabled,
            users_may_create_topics: bot.allows_users_to_create_topics,
        })
    }

    fn stats(&self) -> BTreeMap<String, CallStats> {
        self.lock_calls().clone()
    }
}

/// A recording [`Sender`] for worker and channel tests.
#[cfg(test)]
pub mod mock {
    use std::collections::{BTreeMap, HashMap, VecDeque};
    use std::sync::Arc;
    use std::sync::atomic::{AtomicI64, Ordering};

    use parking_lot::Mutex;

    use async_trait::async_trait;

    use super::{
        ApiError, BotCommandSpec, CommandScope, Ephemeral, Keyboard, MsgRef, OutFile, Sender,
        Target,
    };
    use crate::{BotIdentity, CallStats};

    /// Bytes and file name a mocked download answers.
    type Download = (Vec<u8>, Option<String>);

    /// One recorded call.
    #[derive(Clone, Debug, PartialEq, Eq)]
    #[allow(missing_docs)]
    pub enum Call {
        SendRich {
            to: Target,
            md: String,
            markup: Option<Keyboard>,
            reply_to: Option<i64>,
            ephemeral: Option<Ephemeral>,
        },
        EditRich {
            at: MsgRef,
            md: String,
            markup: Option<Keyboard>,
        },
        Draft {
            to: Target,
            draft_id: i64,
            md: String,
            can_stop: bool,
        },
        SendText {
            to: Target,
            text: String,
            reply_to: Option<i64>,
        },
        EditMarkup {
            at: MsgRef,
            markup: Option<Keyboard>,
        },
        React {
            at: MsgRef,
            emoji: Option<String>,
        },
        Typing(Target),
        AnswerCallback {
            id: String,
            toast: Option<String>,
        },
        SendFile {
            to: Target,
            name: String,
            mime: String,
            len: usize,
            caption: Option<String>,
            reply_to: Option<i64>,
        },
        Download(String),
        CreateTopic {
            chat: i64,
            name: String,
        },
        RenameTopic {
            chat: i64,
            topic: i64,
            name: String,
        },
        CloseTopic {
            chat: i64,
            topic: i64,
        },
        DeleteMessage(MsgRef),
        SetCommands {
            scope: CommandScope,
            names: Vec<String>,
        },
        DeleteWebhook,
        Me,
    }

    /// Records every call; message ids count up from 1000; queued failures
    /// are returned by the next call of their method.
    pub struct MockSender {
        calls: Mutex<Vec<Call>>,
        next_id: AtomicI64,
        failures: Mutex<HashMap<&'static str, VecDeque<ApiError>>>,
        /// Methods whose next N calls never return (a wedged handler).
        hangs: Mutex<HashMap<&'static str, usize>>,
        identity: Mutex<Option<BotIdentity>>,
        downloads: Mutex<HashMap<String, Download>>,
        next_topic: AtomicI64,
    }

    impl MockSender {
        /// An empty recorder.
        pub fn new() -> Arc<Self> {
            Arc::new(Self {
                calls: Mutex::new(Vec::new()),
                next_id: AtomicI64::new(1000),
                failures: Mutex::new(HashMap::new()),
                hangs: Mutex::new(HashMap::new()),
                identity: Mutex::new(None),
                downloads: Mutex::new(HashMap::new()),
                next_topic: AtomicI64::new(500),
            })
        }

        /// Everything recorded so far.
        pub fn calls(&self) -> Vec<Call> {
            self.calls.lock().clone()
        }

        /// Everything recorded so far, clearing the log.
        pub fn take_calls(&self) -> Vec<Call> {
            std::mem::take(&mut *self.calls.lock())
        }

        /// The next call of `method` (the `Sender` method name) returns `err`.
        pub fn fail_next(&self, method: &'static str, err: ApiError) {
            self.failures
                .lock()
                .entry(method)
                .or_default()
                .push_back(err);
        }

        /// The next call of `method` records itself and then never returns.
        pub fn hang_next(&self, method: &'static str) {
            *self.hangs.lock().entry(method).or_default() += 1;
        }

        async fn maybe_hang(&self, method: &'static str) {
            let hang = {
                let mut hangs = self.hangs.lock();
                match hangs.get_mut(method) {
                    Some(n) if *n > 0 => {
                        *n -= 1;
                        true
                    }
                    _ => false,
                }
            };
            if hang {
                std::future::pending::<()>().await;
            }
        }

        /// What `me()` answers.
        pub fn set_identity(&self, me: BotIdentity) {
            *self.identity.lock() = Some(me);
        }

        /// What `download(file_id, …)` answers.
        pub fn set_download(&self, file_id: &str, bytes: Vec<u8>, name: Option<&str>) {
            self.downloads
                .lock()
                .insert(file_id.to_owned(), (bytes, name.map(str::to_owned)));
        }

        fn record(&self, c: Call, method: &'static str) -> Result<(), ApiError> {
            self.calls.lock().push(c);
            match self
                .failures
                .lock()
                .get_mut(method)
                .and_then(VecDeque::pop_front)
            {
                Some(e) => Err(e),
                None => Ok(()),
            }
        }

        fn id(&self) -> i64 {
            self.next_id.fetch_add(1, Ordering::SeqCst)
        }
    }

    #[async_trait]
    impl Sender for MockSender {
        async fn send_rich(
            &self,
            to: &Target,
            md: &str,
            markup: Option<&Keyboard>,
            reply_to: Option<i64>,
            ephemeral: Option<&Ephemeral>,
        ) -> Result<i64, ApiError> {
            self.record(
                Call::SendRich {
                    to: *to,
                    md: md.to_owned(),
                    markup: markup.cloned(),
                    reply_to,
                    ephemeral: ephemeral.cloned(),
                },
                "send_rich",
            )?;
            self.maybe_hang("send_rich").await;
            Ok(self.id())
        }

        async fn edit_rich(
            &self,
            at: &MsgRef,
            md: &str,
            markup: Option<&Keyboard>,
        ) -> Result<(), ApiError> {
            self.record(
                Call::EditRich {
                    at: *at,
                    md: md.to_owned(),
                    markup: markup.cloned(),
                },
                "edit_rich",
            )
        }

        async fn draft(
            &self,
            to: &Target,
            draft_id: i64,
            md: &str,
            can_stop: bool,
        ) -> Result<(), ApiError> {
            self.record(
                Call::Draft {
                    to: *to,
                    draft_id,
                    md: md.to_owned(),
                    can_stop,
                },
                "draft",
            )
        }

        async fn send_text(
            &self,
            to: &Target,
            text: &str,
            reply_to: Option<i64>,
        ) -> Result<i64, ApiError> {
            self.record(
                Call::SendText {
                    to: *to,
                    text: text.to_owned(),
                    reply_to,
                },
                "send_text",
            )?;
            Ok(self.id())
        }

        async fn edit_markup(
            &self,
            at: &MsgRef,
            markup: Option<&Keyboard>,
        ) -> Result<(), ApiError> {
            self.record(
                Call::EditMarkup {
                    at: *at,
                    markup: markup.cloned(),
                },
                "edit_markup",
            )
        }

        async fn react(&self, at: &MsgRef, emoji: Option<&str>) -> Result<(), ApiError> {
            self.record(
                Call::React {
                    at: *at,
                    emoji: emoji.map(str::to_owned),
                },
                "react",
            )
        }

        async fn typing(&self, to: &Target) -> Result<(), ApiError> {
            self.record(Call::Typing(*to), "typing")
        }

        async fn answer_callback(&self, id: &str, toast: Option<&str>) -> Result<(), ApiError> {
            self.record(
                Call::AnswerCallback {
                    id: id.to_owned(),
                    toast: toast.map(str::to_owned),
                },
                "answer_callback",
            )
        }

        async fn send_file(
            &self,
            to: &Target,
            file: &OutFile,
            caption: Option<&str>,
            reply_to: Option<i64>,
        ) -> Result<i64, ApiError> {
            self.record(
                Call::SendFile {
                    to: *to,
                    name: file.name.clone(),
                    mime: file.mime.clone(),
                    len: file.bytes.len(),
                    caption: caption.map(str::to_owned),
                    reply_to,
                },
                "send_file",
            )?;
            Ok(self.id())
        }

        async fn download(
            &self,
            file_id: &str,
            max: u64,
        ) -> Result<(Vec<u8>, Option<String>), ApiError> {
            self.record(Call::Download(file_id.to_owned()), "download")?;
            let (bytes, name) =
                self.downloads
                    .lock()
                    .get(file_id)
                    .cloned()
                    .ok_or_else(|| ApiError::Api {
                        code: 400,
                        description: "Bad Request: invalid file_id".into(),
                    })?;
            if bytes.len() as u64 > max {
                return Err(ApiError::TooBig);
            }
            Ok((bytes, name))
        }

        async fn create_topic(&self, chat: i64, name: &str) -> Result<i64, ApiError> {
            self.record(
                Call::CreateTopic {
                    chat,
                    name: name.to_owned(),
                },
                "create_topic",
            )?;
            Ok(self.next_topic.fetch_add(1, Ordering::SeqCst))
        }

        async fn rename_topic(&self, chat: i64, topic: i64, name: &str) -> Result<(), ApiError> {
            self.record(
                Call::RenameTopic {
                    chat,
                    topic,
                    name: name.to_owned(),
                },
                "rename_topic",
            )
        }

        async fn close_topic(&self, chat: i64, topic: i64) -> Result<(), ApiError> {
            self.record(Call::CloseTopic { chat, topic }, "close_topic")
        }

        async fn delete_message(&self, at: &MsgRef) -> Result<(), ApiError> {
            self.record(Call::DeleteMessage(*at), "delete_message")
        }

        async fn set_commands(
            &self,
            scope: CommandScope,
            cmds: &[BotCommandSpec],
        ) -> Result<(), ApiError> {
            self.record(
                Call::SetCommands {
                    scope,
                    names: cmds.iter().map(|c| c.name.clone()).collect(),
                },
                "set_commands",
            )
        }

        async fn delete_webhook(&self) -> Result<(), ApiError> {
            self.record(Call::DeleteWebhook, "delete_webhook")
        }

        async fn me(&self) -> Result<BotIdentity, ApiError> {
            self.record(Call::Me, "me")?;
            self.identity
                .lock()
                .clone()
                .ok_or_else(|| ApiError::Transport("mock: no identity set".into()))
        }

        fn stats(&self) -> BTreeMap<String, CallStats> {
            BTreeMap::new()
        }
    }
}

#[cfg(test)]
mod tests {
    use std::time::Duration;

    use serde_json::{Value, json};
    use tokio::time::Instant;
    use wiremock::matchers::{method, path};
    use wiremock::{Mock, MockServer, Request, ResponseTemplate};

    use super::*;

    const TOKEN: &str = "123:TEST";

    async fn server() -> (MockServer, Api) {
        let s = MockServer::start().await;
        let api = Api::new(TOKEN, Some(&s.uri())).unwrap();
        (s, api)
    }

    fn ok(result: Value) -> ResponseTemplate {
        ResponseTemplate::new(200).set_body_json(json!({ "ok": true, "result": result }))
    }

    fn message(id: i64) -> Value {
        json!({ "message_id": id, "date": 1, "chat": { "id": 7, "type": "private", "first_name": "A" } })
    }

    fn p(m: &str) -> String {
        format!("/bot{TOKEN}/{m}")
    }

    /// Request body as JSON: a JSON body as is, a multipart form as an object
    /// of its text parts (JSON-looking parts parsed).
    fn body(req: &Request) -> Value {
        let ct = req
            .headers
            .get("content-type")
            .map(|v| v.to_str().unwrap_or_default().to_owned())
            .unwrap_or_default();
        if !ct.starts_with("multipart/form-data") {
            return serde_json::from_slice(&req.body).unwrap();
        }
        let boundary = ct.split("boundary=").nth(1).unwrap().trim_matches('"');
        let text = String::from_utf8_lossy(&req.body);
        let mut out = serde_json::Map::new();
        for part in text.split(&format!("--{boundary}")) {
            let Some((head, val)) = part.split_once("\r\n\r\n") else {
                continue;
            };
            let Some(name) = head
                .split("name=\"")
                .nth(1)
                .and_then(|s| s.split('"').next())
            else {
                continue;
            };
            let val = val
                .trim_end_matches("\r\n")
                .trim_end_matches("--")
                .trim_end_matches("\r\n");
            let v = serde_json::from_str::<Value>(val)
                .unwrap_or_else(|_| Value::String(val.to_owned()));
            out.insert(name.to_owned(), v);
        }
        Value::Object(out)
    }

    async fn only_request(s: &MockServer) -> Value {
        let reqs = s.received_requests().await.unwrap();
        assert_eq!(reqs.len(), 1, "{reqs:?}");
        body(&reqs[0])
    }

    #[tokio::test]
    async fn get_me_maps_identity() {
        let (s, api) = server().await;
        Mock::given(method("GET"))
            .and(path(p("getMe")))
            .respond_with(ok(json!({
                "id": 42, "is_bot": true, "first_name": "T", "username": "theuth_bot",
                "can_join_groups": true, "can_read_all_group_messages": false, "supports_inline_queries": false,
                "has_topics_enabled": true, "allows_users_to_create_topics": false,
                "can_connect_to_business": false, "can_manage_bots": false, "has_main_web_app": false,
                "supports_guest_queries": false, "supports_join_request_queries": false
            })))
            .mount(&s)
            .await;
        let me = api.me().await.unwrap();
        assert_eq!(me.id, 42);
        assert_eq!(me.username, "theuth_bot");
        assert!(me.topics);
        assert!(!me.users_may_create_topics);
        assert_eq!(api.stats()["getMe"].ok, 1);
    }

    #[tokio::test]
    async fn send_rich_carries_markdown_thread_reply_and_keyboard() {
        let (s, api) = server().await;
        Mock::given(path(p("sendRichMessage")))
            .respond_with(ok(message(55)))
            .mount(&s)
            .await;
        let kb = Keyboard {
            rows: vec![vec![
                Button::Callback {
                    text: "Allow".into(),
                    data: "ap:1:a".into(),
                },
                Button::Disabled {
                    text: "Denied".into(),
                },
            ]],
            force_reply: true,
        };
        let id = api
            .send_rich(
                &Target {
                    chat: 7,
                    thread: Some(99),
                },
                "# hi",
                Some(&kb),
                Some(12),
                Some(&Ephemeral {
                    user: 5,
                    callback_query_id: Some("cq".into()),
                }),
            )
            .await
            .unwrap();
        assert_eq!(id, 55);
        let b = only_request(&s).await;
        assert_eq!(b["rich_message"]["markdown"], "# hi");
        assert_eq!(b["message_thread_id"], 99);
        assert_eq!(b["reply_parameters"]["message_id"], 12);
        assert_eq!(b["reply_markup"]["force_reply"], true);
        let row = &b["reply_markup"]["inline_keyboard"][0];
        assert_eq!(row[0]["callback_data"], "ap:1:a");
        assert_eq!(row[1]["text"], "Denied");
        assert!(row[1].get("callback_data").is_none(), "{row}");
        assert_eq!(b["ephemeral_message_parameters"]["receiver_user_id"], 5);
        assert_eq!(b["ephemeral_message_parameters"]["callback_query_id"], "cq");
    }

    #[tokio::test]
    async fn send_text_is_plain_send_message() {
        let (s, api) = server().await;
        Mock::given(path(p("sendMessage")))
            .respond_with(ok(message(3)))
            .mount(&s)
            .await;
        api.send_text(
            &Target {
                chat: 7,
                thread: None,
            },
            "**raw**",
            None,
        )
        .await
        .unwrap();
        let b = only_request(&s).await;
        assert_eq!(b["text"], "**raw**");
        assert!(b.get("parse_mode").is_none());
        assert!(b.get("message_thread_id").is_none());
    }

    #[tokio::test]
    async fn edit_rich_targets_the_message() {
        let (s, api) = server().await;
        Mock::given(path(p("editMessageText")))
            .respond_with(ok(json!(true)))
            .mount(&s)
            .await;
        api.edit_rich(&MsgRef { chat: 7, id: 55 }, "done", None)
            .await
            .unwrap();
        let b = only_request(&s).await;
        assert_eq!(b["message_id"], 55);
        assert_eq!(b["rich_message"]["markdown"], "done");
    }

    #[tokio::test]
    async fn draft_carries_id_and_can_stop() {
        let (s, api) = server().await;
        Mock::given(path(p("sendRichMessageDraft")))
            .respond_with(ok(json!(true)))
            .mount(&s)
            .await;
        api.draft(
            &Target {
                chat: 7,
                thread: Some(2),
            },
            777,
            "wri",
            true,
        )
        .await
        .unwrap();
        let b = only_request(&s).await;
        assert_eq!(b["draft_id"], 777);
        assert_eq!(b["can_stop"], true);
        assert_eq!(b["message_thread_id"], 2);
        assert_eq!(b["rich_message"]["markdown"], "wri");
    }

    #[tokio::test]
    async fn react_sets_and_clears() {
        let (s, api) = server().await;
        Mock::given(path(p("setMessageReaction")))
            .respond_with(ok(json!(true)))
            .mount(&s)
            .await;
        let at = MsgRef { chat: 7, id: 1 };
        api.react(&at, Some("👀")).await.unwrap();
        api.react(&at, None).await.unwrap();
        let reqs = s.received_requests().await.unwrap();
        assert_eq!(body(&reqs[0])["reaction"][0]["emoji"], "👀");
        assert_eq!(body(&reqs[1])["reaction"], json!([]));
    }

    #[tokio::test]
    async fn answer_callback_with_toast() {
        let (s, api) = server().await;
        Mock::given(path(p("answerCallbackQuery")))
            .respond_with(ok(json!(true)))
            .mount(&s)
            .await;
        api.answer_callback("q1", Some("not allowed"))
            .await
            .unwrap();
        let b = only_request(&s).await;
        assert_eq!(b["callback_query_id"], "q1");
        assert_eq!(b["text"], "not allowed");
    }

    #[tokio::test]
    async fn retry_after_is_retried_by_the_client() {
        let (s, api) = server().await;
        Mock::given(path(p("sendMessage")))
            .respond_with(ResponseTemplate::new(429).set_body_json(json!({
                "ok": false, "error_code": 429, "description": "Too Many Requests: retry after 1",
                "parameters": { "retry_after": 1 }
            })))
            .up_to_n_times(1)
            .mount(&s)
            .await;
        Mock::given(path(p("sendMessage")))
            .respond_with(ok(message(9)))
            .mount(&s)
            .await;
        let t0 = Instant::now();
        let id = api
            .send_text(
                &Target {
                    chat: 7,
                    thread: None,
                },
                "x",
                None,
            )
            .await
            .unwrap();
        assert_eq!(id, 9);
        assert!(t0.elapsed() >= Duration::from_millis(900));
        assert_eq!(s.received_requests().await.unwrap().len(), 2);
        assert_eq!(api.stats()["sendMessage"].ok, 1);
    }

    #[tokio::test]
    async fn api_errors_keep_the_description_and_count() {
        let (s, api) = server().await;
        Mock::given(path(p("editMessageText")))
            .respond_with(ResponseTemplate::new(400).set_body_json(json!({
                "ok": false, "error_code": 400, "description": TO_EDIT_NOT_FOUND
            })))
            .mount(&s)
            .await;
        let e = api
            .edit_rich(&MsgRef { chat: 7, id: 1 }, "x", None)
            .await
            .unwrap_err();
        assert!(e.is(TO_EDIT_NOT_FOUND));
        assert!(is_gone_message(&e));
        assert!(!is_unreachable(&e));
        assert_eq!(api.stats()["editMessageText"].err, 1);
    }

    #[tokio::test]
    async fn drafts_to_one_chat_are_paced() {
        let (s, api) = server().await;
        Mock::given(path(p("sendRichMessageDraft")))
            .respond_with(ok(json!(true)))
            .mount(&s)
            .await;
        let to = Target {
            chat: 7,
            thread: None,
        };
        let t0 = Instant::now();
        api.draft(&to, 1, "a", true).await.unwrap();
        api.draft(&to, 1, "ab", true).await.unwrap();
        assert!(t0.elapsed() >= DRAFT_GAP, "{:?}", t0.elapsed());
        // Another chat is not held back by this one.
        let t1 = Instant::now();
        api.draft(
            &Target {
                chat: 8,
                thread: None,
            },
            1,
            "a",
            true,
        )
        .await
        .unwrap();
        assert!(t1.elapsed() < Duration::from_millis(500));
    }

    #[tokio::test]
    async fn edits_of_one_message_are_paced() {
        let (s, api) = server().await;
        Mock::given(path(p("editMessageText")))
            .respond_with(ok(json!(true)))
            .mount(&s)
            .await;
        let at = MsgRef { chat: 7, id: 1 };
        let t0 = Instant::now();
        api.edit_rich(&at, "a", None).await.unwrap();
        api.edit_rich(&at, "ab", None).await.unwrap();
        assert!(t0.elapsed() >= EDIT_GAP, "{:?}", t0.elapsed());
    }

    #[tokio::test]
    async fn download_refuses_big_files_before_fetching() {
        let (s, api) = server().await;
        Mock::given(path(p("getFile")))
            .respond_with(ok(json!({ "file_id": "f1", "file_unique_id": "u", "file_size": 5000, "file_path": "docs/a.pdf" })))
            .mount(&s)
            .await;
        let e = api.download("f1", 4096).await.unwrap_err();
        assert!(matches!(e, ApiError::TooBig));
        assert_eq!(s.received_requests().await.unwrap().len(), 1);
    }

    #[tokio::test]
    async fn download_fetches_the_file_path() {
        let (s, api) = server().await;
        Mock::given(path(p("getFile")))
            .respond_with(ok(json!({ "file_id": "f1", "file_unique_id": "u", "file_size": 3, "file_path": "voice/x.ogg" })))
            .mount(&s)
            .await;
        Mock::given(path(format!("/file/bot{TOKEN}/voice/x.ogg")))
            .respond_with(ResponseTemplate::new(200).set_body_bytes(b"abc".to_vec()))
            .mount(&s)
            .await;
        let (bytes, name) = api.download("f1", 1 << 20).await.unwrap();
        assert_eq!(bytes, b"abc");
        assert_eq!(name.as_deref(), Some("x.ogg"));
    }

    #[tokio::test]
    async fn download_maps_telegrams_too_big() {
        let (s, api) = server().await;
        Mock::given(path(p("getFile")))
            .respond_with(ResponseTemplate::new(400).set_body_json(json!({
                "ok": false, "error_code": 400, "description": FILE_TOO_BIG
            })))
            .mount(&s)
            .await;
        assert!(matches!(
            api.download("f1", 1 << 20).await,
            Err(ApiError::TooBig)
        ));
    }

    #[tokio::test]
    async fn send_file_picks_photo_voice_or_document() {
        let (s, api) = server().await;
        for m in ["sendPhoto", "sendVoice", "sendDocument"] {
            Mock::given(path(p(m)))
                .respond_with(ok(message(1)))
                .mount(&s)
                .await;
        }
        let to = Target {
            chat: 7,
            thread: None,
        };
        let f = |mime: &str| OutFile {
            name: "f".into(),
            mime: mime.into(),
            bytes: vec![1, 2, 3],
        };
        api.send_file(&to, &f("image/png"), Some("cap"), None)
            .await
            .unwrap();
        api.send_file(
            &Target {
                chat: 8,
                thread: None,
            },
            &f("audio/ogg"),
            None,
            None,
        )
        .await
        .unwrap();
        api.send_file(
            &Target {
                chat: 9,
                thread: None,
            },
            &f("application/pdf"),
            None,
            Some(4),
        )
        .await
        .unwrap();
        let reqs = s.received_requests().await.unwrap();
        let paths: Vec<_> = reqs.iter().map(|r| r.url.path().to_owned()).collect();
        assert_eq!(
            paths,
            vec![p("sendPhoto"), p("sendVoice"), p("sendDocument")]
        );
        assert_eq!(body(&reqs[0])["caption"], "cap");
        assert_eq!(body(&reqs[2])["reply_parameters"]["message_id"], 4);
    }

    #[test]
    fn file_kind_by_mime_and_size() {
        assert_eq!(file_kind("image/jpeg", 10), FileKind::Photo);
        assert_eq!(file_kind("image/jpeg", PHOTO_MAX + 1), FileKind::Document);
        assert_eq!(file_kind("audio/mpeg", 10), FileKind::Voice);
        assert_eq!(file_kind("audio/wav", 10), FileKind::Document);
        assert_eq!(file_kind("text/plain", 10), FileKind::Document);
    }

    #[test]
    fn descriptions_map_to_predicates() {
        let api = |d: &str| ApiError::Api {
            code: 400,
            description: d.to_owned(),
        };
        assert!(is_unreachable(&api(BOT_BLOCKED)));
        assert!(is_unreachable(&api(USER_DEACTIVATED)));
        assert!(is_unreachable(&api(CHAT_NOT_FOUND)));
        assert!(is_unreachable(&api(KICKED_SUPERGROUP)));
        assert!(!is_unreachable(&api(TOPIC_CLOSED)));
        assert!(is_topic_gone(&api(TOPIC_CLOSED)));
        assert!(is_topic_gone(&api(THREAD_NOT_FOUND)));
        assert!(is_gone_message(&api(CANT_BE_EDITED)));
        assert!(is_not_modified(&api(
            "Bad Request: message is not modified: specified new message content and reply markup are exactly the same"
        )));
        assert!(!is_not_modified(&api("Bad Request: message is not modifiedx")) || true);
        assert!(api(CONFLICT).is(CONFLICT));
        assert!(!api("Conflict: terminated by other getUpdates request").is(CONFLICT));
        assert!(ApiError::TooBig.description().is_none());
        assert!(!ApiError::RateLimited(Duration::from_secs(1)).is(CONFLICT));
    }

    #[tokio::test]
    async fn get_updates_resets_allowed_only_when_asked() {
        let (s, api) = server().await;
        Mock::given(path(p("getUpdates")))
            .respond_with(ok(json!([])))
            .mount(&s)
            .await;
        api.get_updates(10, 100, 0, true).await.unwrap();
        api.get_updates(11, 1, 0, false).await.unwrap();
        let reqs = s.received_requests().await.unwrap();
        let a = body(&reqs[0]);
        assert_eq!(a["offset"], 10);
        assert_eq!(a["limit"], 100);
        assert_eq!(a["timeout"], 0);
        assert_eq!(a["allowed_updates"], json!([]));
        let b = body(&reqs[1]);
        assert_eq!(b["offset"], 11);
        assert!(b.get("allowed_updates").is_none());
    }

    #[tokio::test]
    async fn set_commands_builds_scope_and_ephemeral() {
        let (s, api) = server().await;
        Mock::given(path(p("setMyCommands")))
            .respond_with(ok(json!(true)))
            .mount(&s)
            .await;
        api.set_commands(
            CommandScope::Chat(7),
            &[BotCommandSpec {
                name: "help".into(),
                description: "this list".into(),
                ephemeral: true,
            }],
        )
        .await
        .unwrap();
        let b = only_request(&s).await;
        assert_eq!(b["scope"]["type"], "chat");
        assert_eq!(b["scope"]["chat_id"], 7);
        assert_eq!(b["commands"][0]["command"], "help");
        assert_eq!(b["commands"][0]["is_ephemeral"], true);
    }

    #[tokio::test]
    async fn mock_sender_records_fails_and_downloads() {
        use super::mock::{Call, MockSender};
        let m = MockSender::new();
        m.set_download("f", vec![1, 2], Some("a.bin"));
        m.fail_next("react", ApiError::TooBig);
        let to = Target {
            chat: 1,
            thread: None,
        };
        assert_eq!(m.send_text(&to, "hi", None).await.unwrap(), 1000);
        assert_eq!(m.send_text(&to, "hi", None).await.unwrap(), 1001);
        assert!(matches!(
            m.react(&MsgRef { chat: 1, id: 1 }, None).await,
            Err(ApiError::TooBig)
        ));
        m.react(&MsgRef { chat: 1, id: 1 }, Some("x"))
            .await
            .unwrap();
        assert_eq!(
            m.download("f", 10).await.unwrap().1.as_deref(),
            Some("a.bin")
        );
        assert!(matches!(m.download("f", 1).await, Err(ApiError::TooBig)));
        assert!(m.me().await.is_err());
        let calls = m.take_calls();
        assert_eq!(calls.len(), 7);
        assert!(matches!(calls[0], Call::SendText { .. }));
        assert!(m.calls().is_empty());
    }
}
