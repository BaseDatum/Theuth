//! The lease-gated `getUpdates` loop (design doc 98 §3.1), its backoff, the
//! `update_id` dedup set and [`classify`], which turns a `tgbot` update into
//! the channel's own [`Incoming`]. With `api.rs` the only module that names
//! `tgbot` types.

use std::collections::{HashSet, VecDeque};
use std::sync::Arc;

use parking_lot::Mutex;
use std::time::Duration;

use runtime::Runtime;
use tgbot::types::{
    Chat, ChatMember, MaybeInaccessibleMessage, Message, MessageData, MessageOrigin, ReplyTo,
    TextEntity, Update, UpdateType,
};
use tokio::sync::mpsc;
use tokio_util::sync::CancellationToken;

use crate::api::{Api, CONFLICT};
use crate::{BotIdentity, TelegramEvent};

/// Dedup set size (21 §3.1).
pub const RECENT_KEYS: usize = 1024;
/// Updates per `getUpdates`.
const BATCH: u8 = 100;
/// Long-poll timeout.
const LONG_POLL_SECS: u64 = 30;

/// Bounded FIFO set of recently seen `update_id`s.
#[derive(Debug, Default)]
pub struct RecentKeys {
    order: VecDeque<i64>,
    set: HashSet<i64>,
}

impl RecentKeys {
    /// Records `id`; `true` when it is new.
    pub fn insert(&mut self, id: i64) -> bool {
        if !self.set.insert(id) {
            return false;
        }
        if self.order.len() >= RECENT_KEYS
            && let Some(old) = self.order.pop_front()
        {
            self.set.remove(&old);
        }
        self.order.push_back(id);
        true
    }
}

/// What [`run`] needs.
pub struct PollCtx {
    /// The live client.
    pub api: Arc<Api>,
    /// `getMe`, for command and mention matching.
    pub me: BotIdentity,
    /// Classified updates, in order, to the channel's inbound task.
    pub tx: mpsc::UnboundedSender<Incoming>,
    /// The runtime (reserved for the channel's use; the loop publishes through `on_state`).
    pub rt: Runtime,
    /// Lease lost or shutdown: confirm the offset and return.
    pub stop: CancellationToken,
    /// `Connected` / `Disconnected` / `Conflict` as they happen.
    pub on_state: Arc<dyn Fn(TelegramEvent) + Send + Sync>,
    /// The process-wide dedup set (98 §11), shared with the channel.
    pub recent: Arc<Mutex<RecentKeys>>,
}

/// `min(2^min(attempt, 6), 60)` seconds (09 §3.1).
#[must_use]
pub fn backoff_delay(attempt: u32) -> Duration {
    Duration::from_secs((1u64 << attempt.min(6)).min(60))
}

/// 98 §3.1: `getUpdates { offset: last + 1, limit: 100, timeout: 30 }` until
/// `stop`; the first call resets `allowed_updates`; `Ok` publishes `Connected`
/// (once per (re)start and again after an error cleared); the §4.3 conflict
/// description publishes `Conflict`, any other error `Disconnected`, both
/// then back off. On stop one `getUpdates { offset: last + 1, limit: 1,
/// timeout: 0 }` confirms what was consumed, then the loop returns.
pub async fn run(ctx: PollCtx) {
    let mut last: i64 = -1;
    let mut attempt: u32 = 0;
    let mut first = true;
    let mut connected = false;
    loop {
        let fetch = ctx.api.get_updates(last + 1, BATCH, LONG_POLL_SECS, first);
        let result = tokio::select! {
            () = ctx.stop.cancelled() => break,
            r = fetch => r,
        };
        match result {
            Ok(batch) => {
                first = false;
                attempt = 0;
                if !connected {
                    connected = true;
                    (ctx.on_state)(TelegramEvent::Connected {
                        bot: ctx.me.username.clone(),
                    });
                }
                for u in batch {
                    last = last.max(u.id);
                    let fresh = ctx.recent.lock().insert(u.id);
                    if !fresh {
                        tracing::debug!(update_id = u.id, "duplicate telegram update dropped");
                        continue;
                    }
                    if ctx.tx.send(classify(&u.update, &ctx.me)).is_err() {
                        tracing::debug!("telegram inbound closed; poller stopping");
                        return;
                    }
                }
            }
            Err(e) => {
                connected = false;
                let delay = backoff_delay(attempt);
                attempt = attempt.saturating_add(1);
                if e.is(CONFLICT) {
                    tracing::warn!(
                        delay_s = delay.as_secs(),
                        "telegram getUpdates conflict: another poller on this token"
                    );
                    (ctx.on_state)(TelegramEvent::Conflict);
                } else {
                    tracing::warn!(error = %e, delay_s = delay.as_secs(), "telegram getUpdates failed; retrying");
                    (ctx.on_state)(TelegramEvent::Disconnected {
                        reason: e.to_string(),
                    });
                }
                tokio::select! {
                    () = ctx.stop.cancelled() => break,
                    () = tokio::time::sleep(delay) => {}
                }
            }
        }
    }
    if let Err(e) = ctx.api.get_updates(last + 1, 1, 0, false).await {
        tracing::debug!(error = %e, "telegram offset confirmation failed");
    }
}

/// Where a message came from.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ChatKind {
    /// A private chat with the bot.
    Private,
    /// A plain group.
    Group,
    /// A supergroup; `forum` = topics enabled.
    Supergroup {
        /// Topics are on.
        forum: bool,
    },
    /// A broadcast channel (ignored, 98 §6.3).
    Channel,
}

/// The chat of an update.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ChatRef {
    /// Chat id.
    pub id: i64,
    /// Kind.
    pub kind: ChatKind,
    /// Group / channel title; `None` for private chats.
    pub title: Option<String>,
}

/// The user who sent something.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SenderRef {
    /// User id.
    pub id: u64,
    /// Bots are dropped before any other check (98 §5).
    pub is_bot: bool,
    /// Full name.
    pub name: String,
    /// `@username`, without the `@`.
    pub username: Option<String>,
}

/// What kind of attachment (98 §7.1).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[allow(missing_docs)]
pub enum MediaKind {
    Photo,
    Document,
    Voice,
    VideoNote,
    Video,
    Audio,
    Animation,
    Sticker,
}

/// One attachment.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Media {
    /// Kind.
    pub kind: MediaKind,
    /// For `getFile`.
    pub file_id: String,
    /// Telegram's file name, when it has one.
    pub file_name: Option<String>,
    /// Telegram's MIME, when it has one.
    pub mime: Option<String>,
    /// Size in bytes, when known.
    pub size: Option<u64>,
    /// Voice / video / audio length.
    pub duration_ms: Option<u64>,
    /// A sticker's emoji.
    pub emoji: Option<String>,
}

/// A `/command` at offset 0 (98 §7.2).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Command {
    /// Lowercase, without the slash or the `@bot` suffix.
    pub name: String,
    /// `/cmd@otherbot`: addressed to a different bot.
    pub for_other_bot: bool,
    /// Everything after the command word, trimmed.
    pub arg: String,
}

/// `reply_to_message`.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ReplyRef {
    /// The quoted message.
    pub message_id: i64,
    /// The quoted message is ours.
    pub from_bot: bool,
    /// Its text or caption.
    pub text: Option<String>,
}

/// Forum service messages (98 §6.1).
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Service {
    /// A topic was created.
    TopicCreated {
        /// Topic name.
        name: String,
        /// The client derived the name from the text (`is_name_implicit`).
        implicit: bool,
    },
    /// A topic was renamed (`name`) or its icon changed (`None`).
    TopicEdited {
        /// The new name.
        name: Option<String>,
    },
    /// Closed.
    TopicClosed,
    /// Reopened.
    TopicReopened,
    /// Any other service message (members, pins, titles, …).
    Other,
}

/// A message update, flattened.
#[derive(Clone, Debug, PartialEq)]
pub struct IncomingMessage {
    /// `update_id`.
    pub update_id: i64,
    /// The chat.
    pub chat: ChatRef,
    /// Topic id when the message is in one.
    pub thread: Option<i64>,
    /// `message_id`.
    pub message_id: i64,
    /// Unix seconds.
    pub date: i64,
    /// Sender; `None` for anonymous / channel senders.
    pub from: Option<SenderRef>,
    /// Text or caption.
    pub text: Option<String>,
    /// A command at offset 0.
    pub command: Option<Command>,
    /// `@bot` mention or `text_mention` of the bot.
    pub mentions_bot: bool,
    /// `reply_to_message`.
    pub reply_to: Option<ReplyRef>,
    /// Forwarded from this name.
    pub forward_from: Option<String>,
    /// Attachments (one per message; albums arrive as several messages).
    pub media: Vec<Media>,
    /// Album id.
    pub media_group_id: Option<String>,
    /// `(latitude, longitude)`.
    pub location: Option<(f64, f64)>,
    /// A shared contact as one line.
    pub contact: Option<String>,
    /// Service message, when it is one.
    pub service: Option<Service>,
}

/// An inline-keyboard press.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct IncomingCallback {
    /// `update_id`.
    pub update_id: i64,
    /// `callback_query.id`, for `answerCallbackQuery`.
    pub id: String,
    /// Who pressed.
    pub from: SenderRef,
    /// Chat of the card.
    pub chat: i64,
    /// The card.
    pub message_id: i64,
    /// Topic of the card.
    pub thread: Option<i64>,
    /// `callback_data`.
    pub data: String,
}

/// A classified update (98 §7).
#[derive(Clone, Debug, PartialEq)]
pub enum Incoming {
    /// A new message (boxed: ~400 bytes against the other variants).
    Message(Box<IncomingMessage>),
    /// A button press.
    Callback(IncomingCallback),
    /// The user pressed Stop on a draft.
    Stopped {
        /// `update_id`.
        update_id: i64,
        /// Chat.
        chat: i64,
        /// Topic.
        thread: Option<i64>,
        /// The draft.
        draft_id: i64,
    },
    /// The bot's membership in a chat changed.
    MyChatMember {
        /// `update_id`.
        update_id: i64,
        /// Chat.
        chat: i64,
        /// Removed or kicked.
        left: bool,
    },
    /// Counted in `Status.dropped_updates`.
    Ignored {
        /// `update_id`.
        update_id: i64,
        /// Update kind (snake_case), `"unknown"` for kinds tgbot does not know.
        kind: &'static str,
    },
}

impl Incoming {
    /// The update kind for logs.
    #[must_use]
    pub fn kind(&self) -> &'static str {
        match self {
            Self::Message(m) if m.command.as_ref().is_some_and(|c| !c.for_other_bot) => "command",
            Self::Message(_) => "message",
            Self::Callback(_) => "callback",
            Self::Stopped { .. } => "stopped",
            Self::MyChatMember { .. } => "my_chat_member",
            Self::Ignored { kind, .. } => kind,
        }
    }

    /// The `update_id`.
    #[must_use]
    pub fn update_id(&self) -> i64 {
        match self {
            Self::Message(m) => m.update_id,
            Self::Callback(cb) => cb.update_id,
            Self::Stopped { update_id, .. }
            | Self::MyChatMember { update_id, .. }
            | Self::Ignored { update_id, .. } => *update_id,
        }
    }
}

/// Classifies one update (98 §7).
#[must_use]
pub fn classify(u: &Update, me: &BotIdentity) -> Incoming {
    let update_id = u.id;
    match &u.update_type {
        UpdateType::Message(m) => Incoming::Message(Box::new(message(update_id, m, me))),
        UpdateType::CallbackQuery(q) => {
            let (chat, message_id, thread) = match &q.message {
                Some(MaybeInaccessibleMessage::Message(m)) => {
                    (chat_id(&m.chat), m.id, m.message_thread_id)
                }
                Some(MaybeInaccessibleMessage::InaccessibleMessage(m)) => {
                    (chat_id(&m.chat), m.message_id, None)
                }
                None => {
                    return Incoming::Ignored {
                        update_id,
                        kind: "callback_query",
                    };
                }
            };
            Incoming::Callback(IncomingCallback {
                update_id,
                id: q.id.clone(),
                from: sender(&q.from),
                chat,
                message_id,
                thread,
                data: q.data.clone().unwrap_or_default(),
            })
        }
        UpdateType::StoppedMessageGeneration(s) => Incoming::Stopped {
            update_id,
            chat: chat_id(&s.chat),
            thread: s.message_thread_id,
            draft_id: s.draft_id,
        },
        UpdateType::BotStatus(cm) => Incoming::MyChatMember {
            update_id,
            chat: chat_id(&cm.chat),
            left: matches!(
                cm.new_chat_member,
                ChatMember::Left(_) | ChatMember::Kicked(_)
            ),
        },
        other => Incoming::Ignored {
            update_id,
            kind: kind_name(other),
        },
    }
}

fn kind_name(t: &UpdateType) -> &'static str {
    match t {
        UpdateType::EditedMessage(_) => "edited_message",
        UpdateType::ChannelPost(_) => "channel_post",
        UpdateType::EditedChannelPost(_) => "edited_channel_post",
        UpdateType::BusinessConnection(_) => "business_connection",
        UpdateType::BusinessMessage(_) => "business_message",
        UpdateType::EditedBusinessMessage(_) => "edited_business_message",
        UpdateType::DeletedBusinessMessages(_) => "deleted_business_messages",
        UpdateType::MessageReaction(_) => "message_reaction",
        UpdateType::MessageReactionCount(_) => "message_reaction_count",
        UpdateType::InlineQuery(_) => "inline_query",
        UpdateType::ChosenInlineResult(_) => "chosen_inline_result",
        UpdateType::ShippingQuery(_) => "shipping_query",
        UpdateType::PreCheckoutQuery(_) => "pre_checkout_query",
        UpdateType::PurchasedPaidMedia(_) => "purchased_paid_media",
        UpdateType::Poll(_) => "poll",
        UpdateType::PollAnswer(_) => "poll_answer",
        UpdateType::UserStatus(_) => "chat_member",
        UpdateType::ChatJoinRequest(_) => "chat_join_request",
        UpdateType::ChatBoostUpdated(_) => "chat_boost",
        UpdateType::ChatBoostRemoved(_) => "removed_chat_boost",
        UpdateType::GuestMessage(_) => "guest_message",
        UpdateType::ManagedBot(_) => "managed_bot",
        UpdateType::Subscription(_) => "subscription",
        UpdateType::Unknown(_) => "unknown",
        UpdateType::Message(_)
        | UpdateType::CallbackQuery(_)
        | UpdateType::StoppedMessageGeneration(_)
        | UpdateType::BotStatus(_) => "handled",
    }
}

fn chat_id(c: &Chat) -> i64 {
    i64::from(c.get_id())
}

fn chat_ref(c: &Chat) -> ChatRef {
    let id = chat_id(c);
    match c {
        Chat::Private(_) => ChatRef {
            id,
            kind: ChatKind::Private,
            title: None,
        },
        Chat::Group(g) => ChatRef {
            id,
            kind: ChatKind::Group,
            title: Some(g.title.clone()),
        },
        Chat::Supergroup(s) => ChatRef {
            id,
            kind: ChatKind::Supergroup {
                forum: s.is_forum.unwrap_or(false),
            },
            title: Some(s.title.clone()),
        },
        Chat::Channel(ch) => ChatRef {
            id,
            kind: ChatKind::Channel,
            title: Some(ch.title.clone()),
        },
    }
}

fn sender(u: &tgbot::types::User) -> SenderRef {
    SenderRef {
        id: u64::try_from(i64::from(u.id)).unwrap_or(0),
        is_bot: u.is_bot,
        name: u.get_full_name(),
        username: u.username.clone().map(String::from),
    }
}

/// Telegram entity offsets count UTF-16 code units.
fn slice_utf16(text: &str, offset: u32, length: u32) -> String {
    let units: Vec<u16> = text.encode_utf16().collect();
    let start = (offset as usize).min(units.len());
    let end = (start + length as usize).min(units.len());
    String::from_utf16_lossy(&units[start..end])
}

fn to_u64(i: i64) -> u64 {
    u64::try_from(i).unwrap_or(0)
}

fn message(update_id: i64, m: &Message, me: &BotIdentity) -> IncomingMessage {
    let chat = chat_ref(&m.chat);
    // `message_thread_id` is a topic only in forum / private-topics chats; a
    // plain supergroup sets it for reply chains too.
    let thread = if m.is_topic_message == Some(true) || chat.kind == ChatKind::Private {
        m.message_thread_id
    } else {
        None
    };
    let from = m.sender.get_user().map(sender);
    let text_obj = m.get_text();
    let text = text_obj.map(|t| t.data.clone());
    let mut command = None;
    let mut mentions_bot = false;
    if let Some(t) = text_obj
        && let Some(entities) = &t.entities
    {
        let at_me = format!("@{}", me.username).to_lowercase();
        for e in entities {
            match e {
                TextEntity::BotCommand(pos) if pos.offset == 0 && command.is_none() => {
                    let word = slice_utf16(&t.data, 0, pos.length);
                    let rest = t.data.get(word.len()..).unwrap_or("").trim().to_owned();
                    let (name, suffix) = word.trim_start_matches('/').split_once('@').map_or_else(
                        || (word.trim_start_matches('/').to_owned(), None),
                        |(n, s)| (n.to_owned(), Some(s.to_owned())),
                    );
                    command = Some(Command {
                        name: name.to_lowercase(),
                        for_other_bot: suffix
                            .is_some_and(|s| !s.eq_ignore_ascii_case(&me.username)),
                        arg: rest,
                    });
                }
                TextEntity::Mention(pos) => {
                    if slice_utf16(&t.data, pos.offset, pos.length).to_lowercase() == at_me {
                        mentions_bot = true;
                    }
                }
                TextEntity::TextMention { user, .. } if to_u64(i64::from(user.id)) == me.id => {
                    mentions_bot = true;
                }
                _ => {}
            }
        }
    }
    let reply_to = match &m.reply_to {
        Some(ReplyTo::Message(r)) => Some(ReplyRef {
            message_id: r.id,
            from_bot: r
                .sender
                .get_user_id()
                .is_some_and(|id| to_u64(i64::from(id)) == me.id),
            text: r.get_text().map(|t| t.data.clone()),
        }),
        _ => None,
    };
    let forward_from = m.forward_origin.as_ref().map(|o| match o {
        MessageOrigin::User(u) => u.sender_user.get_full_name(),
        MessageOrigin::HiddenUser(h) => h.sender_user_name.clone(),
        MessageOrigin::Chat(c) => chat_ref(&c.sender_chat).title.unwrap_or_default(),
        MessageOrigin::Channel(c) => chat_ref(&c.chat).title.unwrap_or_default(),
    });
    let mut media = Vec::new();
    let mut location = None;
    let mut contact = None;
    let mut service = None;
    let secs = |d: i64| Some(to_u64(d).saturating_mul(1000));
    let size = |s: Option<i64>| s.map(to_u64);
    let media_item = |kind,
                      file_id: &str,
                      file_name: Option<&str>,
                      mime: Option<&str>,
                      sz,
                      dur,
                      emoji: Option<&str>| Media {
        kind,
        file_id: file_id.to_owned(),
        file_name: file_name.map(str::to_owned),
        mime: mime.map(str::to_owned),
        size: sz,
        duration_ms: dur,
        emoji: emoji.map(str::to_owned),
    };
    match &m.data {
        MessageData::Photo(p) => {
            if let Some(best) = p.data.iter().max_by_key(|s| s.width * s.height) {
                media.push(media_item(
                    MediaKind::Photo,
                    &best.file_id,
                    None,
                    Some("image/jpeg"),
                    size(best.file_size),
                    None,
                    None,
                ));
            }
        }
        MessageData::Document(d) => media.push(media_item(
            MediaKind::Document,
            &d.data.file_id,
            d.data.file_name.as_deref(),
            d.data.mime_type.as_deref(),
            size(d.data.file_size),
            None,
            None,
        )),
        MessageData::Voice(v) => media.push(media_item(
            MediaKind::Voice,
            &v.data.file_id,
            None,
            v.data.mime_type.as_deref(),
            size(v.data.file_size),
            secs(v.data.duration),
            None,
        )),
        MessageData::VideoNote(v) => media.push(media_item(
            MediaKind::VideoNote,
            &v.file_id,
            None,
            None,
            size(v.file_size),
            secs(v.duration),
            None,
        )),
        MessageData::Video(v) => media.push(media_item(
            MediaKind::Video,
            &v.data.file_id,
            v.data.file_name.as_deref(),
            v.data.mime_type.as_deref(),
            size(v.data.file_size),
            secs(v.data.duration),
            None,
        )),
        MessageData::Audio(a) => media.push(media_item(
            MediaKind::Audio,
            &a.data.file_id,
            a.data.file_name.as_deref(),
            a.data.mime_type.as_deref(),
            size(a.data.file_size),
            secs(a.data.duration),
            None,
        )),
        MessageData::Animation(a) => media.push(media_item(
            MediaKind::Animation,
            &a.file_id,
            a.file_name.as_deref(),
            a.mime_type.as_deref(),
            size(a.file_size),
            secs(a.duration),
            None,
        )),
        MessageData::Sticker(s) => media.push(media_item(
            MediaKind::Sticker,
            &s.file_id,
            None,
            None,
            size(s.file_size),
            None,
            s.emoji.as_deref(),
        )),
        MessageData::Location(l) => {
            location = Some((f64::from(l.latitude), f64::from(l.longitude)))
        }
        MessageData::Venue(v) => {
            location = Some((
                f64::from(v.location.latitude),
                f64::from(v.location.longitude),
            ))
        }
        MessageData::Contact(c) => {
            let mut line = c.first_name.clone();
            if let Some(l) = &c.last_name {
                line.push(' ');
                line.push_str(l);
            }
            line.push(' ');
            line.push_str(&c.phone_number);
            contact = Some(line);
        }
        MessageData::ForumTopicCreated(t) => {
            service = Some(Service::TopicCreated {
                name: t.name.clone(),
                implicit: t.is_name_implicit.unwrap_or(false),
            });
        }
        MessageData::ForumTopicEdited(t) => {
            service = Some(Service::TopicEdited {
                name: t.name.clone(),
            })
        }
        MessageData::ForumTopicClosed => service = Some(Service::TopicClosed),
        MessageData::ForumTopicReopened => service = Some(Service::TopicReopened),
        MessageData::NewChatMembers(_)
        | MessageData::LeftChatMember(_)
        | MessageData::NewChatTitle(_)
        | MessageData::NewChatPhoto(_)
        | MessageData::DeleteChatPhoto
        | MessageData::GroupChatCreated
        | MessageData::SupergroupChatCreated
        | MessageData::ChannelChatCreated
        | MessageData::MigrateToChatId(_)
        | MessageData::MigrateFromChatId(_)
        | MessageData::PinnedMessage(_)
        | MessageData::GeneralForumTopicHidden
        | MessageData::GeneralForumTopicUnhidden
        | MessageData::VideoChatScheduled(_)
        | MessageData::VideoChatStarted
        | MessageData::VideoChatEnded(_)
        | MessageData::VideoChatParticipantsInvited(_)
        | MessageData::BoostAdded(_)
        | MessageData::AutoDeleteTimerChanged(_)
        | MessageData::WriteAccessAllowed(_)
        | MessageData::ChatBackgroundSet(_)
        | MessageData::ChatOwnerChanged(_)
        | MessageData::ChatOwnerLeft(_)
        | MessageData::ProximityAlertTriggered(_)
        | MessageData::ConnectedWebsite(_)
        | MessageData::ChatShared(_)
        | MessageData::UsersShared(_) => service = Some(Service::Other),
        _ => {}
    }
    IncomingMessage {
        update_id,
        chat,
        thread,
        message_id: m.id,
        date: m.date,
        from,
        text,
        command,
        mentions_bot,
        reply_to,
        forward_from,
        media,
        media_group_id: m.media_group_id.clone(),
        location,
        contact,
        service,
    }
}

#[cfg(test)]
mod tests {
    use std::sync::Arc;
    use std::time::Duration;

    use serde_json::{Value, json};
    use wiremock::matchers::path;
    use wiremock::{Mock, MockServer, ResponseTemplate};

    use super::*;

    fn me() -> BotIdentity {
        BotIdentity {
            id: 42,
            username: "theuth_bot".into(),
            topics: true,
            users_may_create_topics: false,
        }
    }

    fn update(v: Value) -> Update {
        serde_json::from_value(v).unwrap()
    }

    fn user(id: i64) -> Value {
        json!({ "id": id, "is_bot": false, "first_name": "Ann", "last_name": "Lee", "username": "ann" })
    }

    fn private() -> Value {
        json!({ "id": 7, "type": "private", "first_name": "Ann" })
    }

    #[test]
    fn private_text_with_command_and_arg() {
        let u = update(json!({
            "update_id": 1,
            "message": {
                "message_id": 10, "date": 1700000000, "chat": private(), "from": user(5),
                "text": "/Model@theuth_bot gpt-5",
                "entities": [{ "type": "bot_command", "offset": 0, "length": 17 }]
            }
        }));
        let Incoming::Message(m) = classify(&u, &me()) else {
            panic!()
        };
        assert_eq!(m.chat.kind, ChatKind::Private);
        assert_eq!(m.chat.id, 7);
        assert_eq!(m.from.as_ref().unwrap().name, "Ann Lee");
        assert_eq!(m.from.as_ref().unwrap().username.as_deref(), Some("ann"));
        let c = m.command.unwrap();
        assert_eq!(c.name, "model");
        assert!(!c.for_other_bot);
        assert_eq!(c.arg, "gpt-5");
        assert!(!m.mentions_bot);
        assert!(m.media.is_empty());
        assert!(m.service.is_none());
    }

    #[test]
    fn command_for_another_bot_is_flagged_and_not_first_entity_ignored() {
        let u = update(json!({
            "update_id": 2,
            "message": {
                "message_id": 10, "date": 1, "chat": private(), "from": user(5),
                "text": "/help@otherbot",
                "entities": [{ "type": "bot_command", "offset": 0, "length": 14 }]
            }
        }));
        let Incoming::Message(m) = classify(&u, &me()) else {
            panic!()
        };
        assert!(m.command.unwrap().for_other_bot);
        let u = update(json!({
            "update_id": 3,
            "message": {
                "message_id": 10, "date": 1, "chat": private(), "from": user(5),
                "text": "see /help",
                "entities": [{ "type": "bot_command", "offset": 4, "length": 5 }]
            }
        }));
        let Incoming::Message(m) = classify(&u, &me()) else {
            panic!()
        };
        assert!(m.command.is_none());
    }

    #[test]
    fn group_message_mentioning_the_bot_with_reply() {
        let u = update(json!({
            "update_id": 4,
            "message": {
                "message_id": 20, "date": 1, "from": user(5),
                "chat": { "id": -100123, "type": "supergroup", "title": "Ops", "is_forum": false },
                "message_thread_id": 20,
                "text": "héllo @theuth_bot look",
                "entities": [{ "type": "mention", "offset": 6, "length": 11 }],
                "reply_to_message": {
                    "message_id": 19, "date": 1, "chat": { "id": -100123, "type": "supergroup", "title": "Ops" },
                    "from": { "id": 42, "is_bot": true, "first_name": "T" },
                    "text": "earlier"
                }
            }
        }));
        let Incoming::Message(m) = classify(&u, &me()) else {
            panic!()
        };
        assert_eq!(m.chat.kind, ChatKind::Supergroup { forum: false });
        assert_eq!(m.chat.title.as_deref(), Some("Ops"));
        assert!(m.mentions_bot);
        // Not a topic message: the reply-chain thread id is not a topic.
        assert_eq!(m.thread, None);
        let r = m.reply_to.unwrap();
        assert_eq!(r.message_id, 19);
        assert!(r.from_bot);
        assert_eq!(r.text.as_deref(), Some("earlier"));
    }

    #[test]
    fn text_mention_of_the_bot_counts() {
        let u = update(json!({
            "update_id": 5,
            "message": {
                "message_id": 20, "date": 1, "from": user(5),
                "chat": { "id": -5, "type": "group", "title": "G" },
                "text": "hey bot",
                "entities": [{ "type": "text_mention", "offset": 4, "length": 3, "user": { "id": 42, "is_bot": true, "first_name": "T" } }]
            }
        }));
        let Incoming::Message(m) = classify(&u, &me()) else {
            panic!()
        };
        assert!(m.mentions_bot);
        assert_eq!(m.chat.kind, ChatKind::Group);
    }

    #[test]
    fn photo_album_message_keeps_the_largest_size() {
        let u = update(json!({
            "update_id": 6,
            "message": {
                "message_id": 30, "date": 1, "chat": private(), "from": user(5),
                "media_group_id": "album1", "caption": "pics",
                "photo": [
                    { "file_id": "small", "file_unique_id": "a", "width": 90, "height": 90, "file_size": 100 },
                    { "file_id": "big", "file_unique_id": "b", "width": 800, "height": 600, "file_size": 50000 },
                    { "file_id": "mid", "file_unique_id": "c", "width": 320, "height": 240 }
                ]
            }
        }));
        let Incoming::Message(m) = classify(&u, &me()) else {
            panic!()
        };
        assert_eq!(m.text.as_deref(), Some("pics"));
        assert_eq!(m.media_group_id.as_deref(), Some("album1"));
        assert_eq!(m.media.len(), 1);
        assert_eq!(m.media[0].kind, MediaKind::Photo);
        assert_eq!(m.media[0].file_id, "big");
        assert_eq!(m.media[0].size, Some(50000));
        assert_eq!(m.media[0].mime.as_deref(), Some("image/jpeg"));
    }

    #[test]
    fn voice_note_has_duration_and_mime() {
        let u = update(json!({
            "update_id": 7,
            "message": {
                "message_id": 31, "date": 1, "chat": private(), "from": user(5),
                "voice": { "file_id": "v1", "file_unique_id": "u", "duration": 4, "mime_type": "audio/ogg", "file_size": 9000 }
            }
        }));
        let Incoming::Message(m) = classify(&u, &me()) else {
            panic!()
        };
        assert!(m.text.is_none());
        let v = &m.media[0];
        assert_eq!(v.kind, MediaKind::Voice);
        assert_eq!(v.duration_ms, Some(4000));
        assert_eq!(v.mime.as_deref(), Some("audio/ogg"));
        assert_eq!(v.size, Some(9000));
    }

    #[test]
    fn document_sticker_location_contact() {
        let base = |data: Value| {
            let mut m = json!({ "message_id": 1, "date": 1, "chat": private(), "from": user(5) });
            for (k, v) in data.as_object().unwrap() {
                m[k] = v.clone();
            }
            update(json!({ "update_id": 8, "message": m }))
        };
        let Incoming::Message(m) = classify(
            &base(
                json!({ "document": { "file_id": "d", "file_unique_id": "u", "file_name": "a.pdf", "mime_type": "application/pdf" } }),
            ),
            &me(),
        ) else {
            panic!()
        };
        assert_eq!(m.media[0].kind, MediaKind::Document);
        assert_eq!(m.media[0].file_name.as_deref(), Some("a.pdf"));
        let Incoming::Message(m) = classify(
            &base(
                json!({ "sticker": { "file_id": "s", "file_unique_id": "u", "width": 1, "height": 1, "is_animated": false, "is_video": false, "type": "regular", "emoji": "🦀" } }),
            ),
            &me(),
        ) else {
            panic!()
        };
        assert_eq!(m.media[0].kind, MediaKind::Sticker);
        assert_eq!(m.media[0].emoji.as_deref(), Some("🦀"));
        let Incoming::Message(m) = classify(
            &base(json!({ "location": { "latitude": 1.5, "longitude": -2.5 } })),
            &me(),
        ) else {
            panic!()
        };
        assert_eq!(m.location, Some((1.5, -2.5)));
        let Incoming::Message(m) = classify(
            &base(
                json!({ "contact": { "phone_number": "+1555", "first_name": "Bob", "last_name": "K" } }),
            ),
            &me(),
        ) else {
            panic!()
        };
        assert_eq!(m.contact.as_deref(), Some("Bob K +1555"));
    }

    #[test]
    fn forum_topic_message_and_service_messages() {
        let chat = json!({ "id": -100123, "type": "supergroup", "title": "Ops", "is_forum": true });
        let u = update(json!({
            "update_id": 9,
            "message": {
                "message_id": 40, "date": 1, "from": user(5), "chat": chat,
                "message_thread_id": 33, "is_topic_message": true, "text": "in topic"
            }
        }));
        let Incoming::Message(m) = classify(&u, &me()) else {
            panic!()
        };
        assert_eq!(m.chat.kind, ChatKind::Supergroup { forum: true });
        assert_eq!(m.thread, Some(33));
        assert_eq!(m.text.as_deref(), Some("in topic"));
        let u = update(json!({
            "update_id": 10,
            "message": {
                "message_id": 41, "date": 1, "from": user(5), "chat": chat,
                "message_thread_id": 34, "is_topic_message": true,
                "forum_topic_created": { "name": "Fix the build", "icon_color": 7322096, "is_name_implicit": true }
            }
        }));
        let Incoming::Message(m) = classify(&u, &me()) else {
            panic!()
        };
        assert_eq!(
            m.service,
            Some(Service::TopicCreated {
                name: "Fix the build".into(),
                implicit: true
            })
        );
        let u = update(json!({
            "update_id": 11,
            "message": {
                "message_id": 42, "date": 1, "from": user(5), "chat": chat,
                "message_thread_id": 34, "is_topic_message": true, "forum_topic_closed": {}
            }
        }));
        let Incoming::Message(m) = classify(&u, &me()) else {
            panic!()
        };
        assert_eq!(m.service, Some(Service::TopicClosed));
        let u = update(json!({
            "update_id": 12,
            "message": {
                "message_id": 43, "date": 1, "from": user(5), "chat": chat,
                "new_chat_members": [user(6)]
            }
        }));
        let Incoming::Message(m) = classify(&u, &me()) else {
            panic!()
        };
        assert_eq!(m.service, Some(Service::Other));
    }

    #[test]
    fn forwarded_message_names_the_origin() {
        let u = update(json!({
            "update_id": 13,
            "message": {
                "message_id": 1, "date": 1, "chat": private(), "from": user(5), "text": "fwd",
                "forward_origin": { "type": "hidden_user", "date": 1, "sender_user_name": "Someone" }
            }
        }));
        let Incoming::Message(m) = classify(&u, &me()) else {
            panic!()
        };
        assert_eq!(m.forward_from.as_deref(), Some("Someone"));
    }

    #[test]
    fn callback_query_is_a_press() {
        let u = update(json!({
            "update_id": 14,
            "callback_query": {
                "id": "cq1", "from": user(5), "chat_instance": "x", "data": "ap:abc:a",
                "message": { "message_id": 50, "date": 1, "chat": private(), "message_thread_id": 3, "text": "card" }
            }
        }));
        let Incoming::Callback(c) = classify(&u, &me()) else {
            panic!()
        };
        assert_eq!(c.id, "cq1");
        assert_eq!(c.from.id, 5);
        assert_eq!(c.chat, 7);
        assert_eq!(c.message_id, 50);
        assert_eq!(c.thread, Some(3));
        assert_eq!(c.data, "ap:abc:a");
    }

    #[test]
    fn stopped_message_generation_and_my_chat_member() {
        let u = update(json!({
            "update_id": 15,
            "stopped_message_generation": { "chat": private(), "draft_id": 777, "message_thread_id": 9 }
        }));
        assert_eq!(
            classify(&u, &me()),
            Incoming::Stopped {
                update_id: 15,
                chat: 7,
                thread: Some(9),
                draft_id: 777
            }
        );
        let u = update(json!({
            "update_id": 16,
            "my_chat_member": {
                "chat": { "id": -100123, "type": "supergroup", "title": "Ops" }, "date": 1, "from": user(5),
                "old_chat_member": { "status": "member", "user": { "id": 42, "is_bot": true, "first_name": "T" } },
                "new_chat_member": { "status": "left", "user": { "id": 42, "is_bot": true, "first_name": "T" } }
            }
        }));
        assert_eq!(
            classify(&u, &me()),
            Incoming::MyChatMember {
                update_id: 16,
                chat: -100123,
                left: true
            }
        );
    }

    #[test]
    fn unknown_and_other_kinds_are_ignored_with_a_name() {
        let u = update(json!({ "update_id": 17, "brand_new_kind": { "x": 1 } }));
        assert_eq!(
            classify(&u, &me()),
            Incoming::Ignored {
                update_id: 17,
                kind: "unknown"
            }
        );
        let u = update(json!({
            "update_id": 18,
            "edited_message": { "message_id": 1, "date": 1, "chat": private(), "text": "edit" }
        }));
        assert_eq!(
            classify(&u, &me()),
            Incoming::Ignored {
                update_id: 18,
                kind: "edited_message"
            }
        );
    }

    #[test]
    fn recent_keys_dedup_and_evict() {
        let mut r = RecentKeys::default();
        assert!(r.insert(1));
        assert!(!r.insert(1));
        for i in 2..=(RECENT_KEYS as i64 + 1) {
            assert!(r.insert(i));
        }
        // 1 was evicted: it is new again.
        assert!(r.insert(1));
    }

    #[test]
    fn backoff_caps_at_sixty() {
        assert_eq!(backoff_delay(0), Duration::from_secs(1));
        assert_eq!(backoff_delay(3), Duration::from_secs(8));
        assert_eq!(backoff_delay(6), Duration::from_secs(60));
        assert_eq!(backoff_delay(40), Duration::from_secs(60));
    }

    const TOKEN: &str = "123:TEST";

    fn body(req: &wiremock::Request) -> Value {
        serde_json::from_slice(&req.body).unwrap()
    }

    fn ok(result: Value) -> ResponseTemplate {
        ResponseTemplate::new(200).set_body_json(json!({ "ok": true, "result": result }))
    }

    struct Fixture {
        server: MockServer,
        rx: mpsc::UnboundedReceiver<Incoming>,
        events: Arc<Mutex<Vec<TelegramEvent>>>,
        stop: CancellationToken,
        task: tokio::task::JoinHandle<()>,
    }

    async fn start(server: MockServer, recent: Arc<Mutex<RecentKeys>>) -> Fixture {
        let api = Arc::new(Api::new(TOKEN, Some(&server.uri())).unwrap());
        let (tx, rx) = mpsc::unbounded_channel();
        let events = Arc::new(Mutex::new(Vec::new()));
        let ev = Arc::clone(&events);
        let stop = CancellationToken::new();
        let dir = tempfile::tempdir().unwrap();
        let rt = Runtime::new(figment::Figment::new(), dir.path().to_path_buf());
        let task = tokio::spawn(run(PollCtx {
            api,
            me: me(),
            tx,
            rt,
            stop: stop.clone(),
            on_state: Arc::new(move |e| ev.lock().push(e)),
            recent,
        }));
        Fixture {
            server,
            rx,
            events,
            stop,
            task,
        }
    }

    fn text_update(id: i64, text: &str) -> Value {
        json!({
            "update_id": id,
            "message": { "message_id": id, "date": 1, "chat": private(), "from": user(5), "text": text }
        })
    }

    #[tokio::test]
    async fn batch_delivered_in_order_deduped_and_offset_confirmed_on_stop() {
        let server = MockServer::start().await;
        Mock::given(path(format!("/bot{TOKEN}/getUpdates")))
            .respond_with(ok(json!([
                text_update(1, "a"),
                text_update(2, "b"),
                text_update(3, "c")
            ])))
            .up_to_n_times(1)
            .mount(&server)
            .await;
        Mock::given(path(format!("/bot{TOKEN}/getUpdates")))
            .respond_with(ok(json!([])).set_delay(Duration::from_millis(200)))
            .mount(&server)
            .await;
        let recent = Arc::new(Mutex::new(RecentKeys::default()));
        recent.lock().insert(2);
        let mut f = start(server, recent).await;
        let first = f.rx.recv().await.unwrap();
        let second = f.rx.recv().await.unwrap();
        let Incoming::Message(a) = first else {
            panic!()
        };
        let Incoming::Message(c) = second else {
            panic!()
        };
        assert_eq!((a.update_id, a.text.as_deref()), (1, Some("a")));
        assert_eq!((c.update_id, c.text.as_deref()), (3, Some("c")));
        assert_eq!(
            f.events.lock().as_slice(),
            &[TelegramEvent::Connected {
                bot: "theuth_bot".into()
            }]
        );
        f.stop.cancel();
        f.task.await.unwrap();
        let reqs = f.server.received_requests().await.unwrap();
        let first = body(&reqs[0]);
        assert_eq!(first["offset"], 0);
        assert_eq!(first["limit"], 100);
        assert_eq!(first["timeout"], 30);
        assert_eq!(first["allowed_updates"], json!([]));
        assert!(body(&reqs[1]).get("allowed_updates").is_none());
        let confirm = body(reqs.last().unwrap());
        assert_eq!(confirm["offset"], 4);
        assert_eq!(confirm["limit"], 1);
        assert_eq!(confirm["timeout"], 0);
        assert!(f.rx.try_recv().is_err());
    }

    #[tokio::test]
    async fn conflict_is_published_and_the_loop_continues() {
        let server = MockServer::start().await;
        Mock::given(path(format!("/bot{TOKEN}/getUpdates")))
            .respond_with(ResponseTemplate::new(409).set_body_json(json!({
                "ok": false, "error_code": 409, "description": CONFLICT
            })))
            .up_to_n_times(1)
            .mount(&server)
            .await;
        Mock::given(path(format!("/bot{TOKEN}/getUpdates")))
            .respond_with(ok(json!([text_update(9, "after")])))
            .up_to_n_times(1)
            .mount(&server)
            .await;
        Mock::given(path(format!("/bot{TOKEN}/getUpdates")))
            .respond_with(ok(json!([])).set_delay(Duration::from_millis(200)))
            .mount(&server)
            .await;
        let mut f = start(server, Arc::new(Mutex::new(RecentKeys::default()))).await;
        let got = tokio::time::timeout(Duration::from_secs(5), f.rx.recv())
            .await
            .unwrap()
            .unwrap();
        let Incoming::Message(m) = got else { panic!() };
        assert_eq!(m.update_id, 9);
        assert_eq!(
            f.events.lock().as_slice(),
            &[
                TelegramEvent::Conflict,
                TelegramEvent::Connected {
                    bot: "theuth_bot".into()
                }
            ]
        );
        f.stop.cancel();
        f.task.await.unwrap();
        // The call after the conflict still asked for the allowed-updates reset.
        let reqs = f.server.received_requests().await.unwrap();
        assert_eq!(body(&reqs[1])["allowed_updates"], json!([]));
    }

    #[tokio::test]
    async fn transport_error_publishes_disconnected() {
        let server = MockServer::start().await;
        Mock::given(path(format!("/bot{TOKEN}/getUpdates")))
            .respond_with(ResponseTemplate::new(500).set_body_string("boom"))
            .up_to_n_times(1)
            .mount(&server)
            .await;
        Mock::given(path(format!("/bot{TOKEN}/getUpdates")))
            .respond_with(ok(json!([])).set_delay(Duration::from_millis(200)))
            .mount(&server)
            .await;
        let f = start(server, Arc::new(Mutex::new(RecentKeys::default()))).await;
        tokio::time::sleep(Duration::from_millis(1300)).await;
        f.stop.cancel();
        f.task.await.unwrap();
        let events = f.events.lock().clone();
        assert!(
            matches!(events[0], TelegramEvent::Disconnected { .. }),
            "{events:?}"
        );
        assert!(
            matches!(events.get(1), Some(TelegramEvent::Connected { .. })),
            "{events:?}"
        );
    }
}
