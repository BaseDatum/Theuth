//! Thin façade over the slack-morphism session: the methods the plugin
//! uses, exact error codes, per-method call counters and the cross-thread
//! pacer (design doc 21 §4).

use std::collections::{BTreeMap, HashMap};
use std::sync::{Mutex, MutexGuard, PoisonError};
use std::time::Duration;

use slack_morphism::errors::SlackClientError;
use slack_morphism::prelude::*;
use tokio::time::Instant;
use tracing::Instrument;

use crate::CallStats;

/// `markdown_text` limit per call (`msg_too_long` above it).
pub const MARKDOWN_TEXT_MAX: usize = 12_000;

/// Minimum gap between two `chat.appendStream` calls across all threads (21 §4.2).
pub const APPEND_GAP: Duration = Duration::from_millis(650);
/// Minimum gap between two `chat.update` calls across all threads.
pub const UPDATE_GAP: Duration = Duration::from_millis(1200);

/// A failed Slack call.
#[derive(Debug, thiserror::Error)]
pub enum ApiError {
    /// `ok: false` with this exact `error` code, e.g. `message_not_in_streaming_state`.
    #[error("slack: {0}")]
    Slack(String),
    /// 429 after morphism's own retries; wait this long.
    #[error("rate limited; retry after {0:?}")]
    RateLimited(Duration),
    /// Anything that never produced a Slack answer.
    #[error("transport: {0}")]
    Transport(String),
}

/// `task_update.status` (21 §8.3).
#[derive(Clone, Debug, PartialEq, Eq, serde::Serialize)]
#[serde(rename_all = "snake_case")]
pub enum TaskStatus {
    /// Announced, not yet running.
    Pending,
    /// Running.
    InProgress,
    /// Finished fine.
    Complete,
    /// Finished with an error.
    Error,
}

impl TaskStatus {
    fn wire(&self) -> SlackTaskCardStatus {
        match self {
            Self::Pending => SlackTaskCardStatus::Pending,
            Self::InProgress => SlackTaskCardStatus::InProgress,
            Self::Complete => SlackTaskCardStatus::Complete,
            Self::Error => SlackTaskCardStatus::Error,
        }
    }
}

/// `agents.sessions.setStatus` / `chat.stopStream` `session_status` from its wire name.
fn session_status(s: &str) -> Result<SlackAgentSessionStatus, ApiError> {
    match s {
        "active" => Ok(SlackAgentSessionStatus::Active),
        "processing" => Ok(SlackAgentSessionStatus::Processing),
        "suspended" => Ok(SlackAgentSessionStatus::Suspended),
        "closed" => Ok(SlackAgentSessionStatus::Closed),
        other => Err(ApiError::Transport(format!(
            "unknown session status {other:?}"
        ))),
    }
}

/// A `task_update` source: a link shown on the card.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Source {
    /// `http(s)` URL.
    pub url: String,
    /// Link text.
    pub text: String,
}

/// One stream chunk (21 §8.1).
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Chunk {
    /// Raw markdown appended to the body.
    Text(String),
    /// A timeline card, created or updated by `id`.
    Task {
        /// Card id (the tool `call_id`).
        id: String,
        /// Card title (≤ 256).
        title: String,
        /// Card state.
        status: TaskStatus,
        /// One line under the title.
        details: Option<String>,
        /// Card body (reasoning text, 21 §8.5); no server-side cap seen up to 40 000.
        output: Option<String>,
        /// Links.
        sources: Vec<Source>,
    },
    /// Block Kit blocks (a serde_json array) in the body where they appear
    /// (110 §8): sent verbatim, so `container` / `data_table` /
    /// `data_visualization` / `plan` pass although morphism's `SlackBlock`
    /// cannot type them.
    Blocks(serde_json::Value),
}

impl Chunk {
    /// The chunk object inside `chunks` on the wire. `Task` goes through
    /// morphism's `SlackStreamChunk::TaskUpdate` so its source URLs are
    /// validated; the other two are raw.
    fn wire(self) -> Result<serde_json::Value, ApiError> {
        Ok(match self {
            Chunk::Text(text) => serde_json::json!({ "type": "markdown_text", "text": text }),
            Chunk::Blocks(blocks) => serde_json::json!({ "type": "blocks", "blocks": blocks }),
            Chunk::Task {
                id,
                title,
                status,
                details,
                output,
                sources,
            } => {
                let task = SlackStreamChunk::TaskUpdate {
                    id: id.into(),
                    title,
                    hide_title: None,
                    icon: None,
                    status: status.wire(),
                    details,
                    output,
                    sources: (!sources.is_empty())
                        .then(|| {
                            sources
                                .into_iter()
                                .map(|s| {
                                    let url = s.url.parse().map_err(|e| {
                                        ApiError::Transport(format!("task source URL: {e}"))
                                    })?;
                                    Ok(SlackTaskCardSource::Url(SlackUrlSourceElement::new(
                                        url, s.text,
                                    )))
                                })
                                .collect::<Result<Vec<_>, ApiError>>()
                        })
                        .transpose()?,
                };
                serde_json::to_value(task)
                    .map_err(|e| ApiError::Transport(format!("task_update: {e}")))?
            }
        })
    }
}

/// `chunks` for a stream request; `None` when there is nothing to send.
fn chunks(v: Vec<Chunk>) -> Result<Option<Vec<serde_json::Value>>, ApiError> {
    (!v.is_empty())
        .then(|| v.into_iter().map(Chunk::wire).collect())
        .transpose()
}

/// One file for [`SlackApi::upload_file`] (24 §5).
#[derive(Debug, Clone)]
pub struct UploadFile {
    /// File name shown in Slack (also the `title`).
    pub name: String,
    /// Content type sent with the upload PUT.
    pub mime: String,
    /// The file's contents.
    pub bytes: Vec<u8>,
}

/// What `bubble.rs` and `channel.rs` need from Slack. [`Api`] implements it
/// over slack-morphism; tests use a fake.
#[async_trait::async_trait]
pub trait SlackApi: Send + Sync {
    /// `chat.startStream` (timeline mode) → stream message ts.
    async fn start_stream(
        &self,
        channel: &str,
        thread_ts: &str,
        user: &str,
        team: &str,
        chunks: Vec<Chunk>,
    ) -> Result<String, ApiError>;
    /// `chat.appendStream`.
    async fn append_stream(
        &self,
        channel: &str,
        ts: &str,
        chunks: Vec<Chunk>,
    ) -> Result<(), ApiError>;
    /// `chat.stopStream`; `footer` becomes one `context` block (mrkdwn);
    /// `session_status` e.g. `Some("active")`.
    async fn stop_stream(
        &self,
        channel: &str,
        ts: &str,
        chunks: Vec<Chunk>,
        footer: Option<String>,
        session_status: Option<&str>,
    ) -> Result<(), ApiError>;
    /// `chat.postMessage` with `markdown_text` → ts.
    async fn post_markdown(
        &self,
        channel: &str,
        thread_ts: Option<&str>,
        markdown: &str,
    ) -> Result<String, ApiError>;
    /// `chat.update` with `markdown_text`.
    async fn update_markdown(
        &self,
        channel: &str,
        ts: &str,
        markdown: &str,
    ) -> Result<(), ApiError>;
    /// `chat.postMessage` with `blocks` (a serde_json array) → ts.
    async fn post_blocks(
        &self,
        channel: &str,
        thread_ts: Option<&str>,
        blocks: serde_json::Value,
    ) -> Result<String, ApiError>;
    /// `chat.update` with `blocks`.
    async fn update_blocks(
        &self,
        channel: &str,
        ts: &str,
        blocks: serde_json::Value,
    ) -> Result<(), ApiError>;
    /// `chat.postEphemeral`.
    async fn ephemeral(&self, channel: &str, user: &str, text: &str) -> Result<(), ApiError>;
    /// `reactions.add`; `name` without colons (`eyes`).
    async fn react(&self, channel: &str, ts: &str, name: &str) -> Result<(), ApiError>;
    /// `agents.sessions.setStatus`; status `processing` | `active`.
    async fn set_session_status(
        &self,
        channel: &str,
        thread_ts: &str,
        status: &str,
    ) -> Result<(), ApiError>;
    /// `agents.sessions.rename`.
    async fn rename_session(
        &self,
        channel: &str,
        thread_ts: &str,
        title: &str,
    ) -> Result<(), ApiError>;
    /// files v2 upload (24 §5): one `files.getUploadURLExternal` + PUT per
    /// file, then one `files.completeUploadExternal` sharing all of them
    /// into `channel` (top level when `thread_ts` is `None`) with
    /// `comment` as the mrkdwn `initial_comment`; the first file's permalink.
    async fn upload_file(
        &self,
        channel: &str,
        thread_ts: Option<&str>,
        files: &[UploadFile],
        comment: Option<&str>,
    ) -> Result<String, ApiError>;
    /// A text snippet as one `<title>.txt` file in a thread (21 §8.3); the permalink.
    async fn upload_text(
        &self,
        channel: &str,
        thread_ts: &str,
        title: &str,
        text: &str,
    ) -> Result<String, ApiError> {
        let file = UploadFile {
            name: format!("{title}.txt"),
            mime: "text/plain; charset=utf-8".into(),
            bytes: text.as_bytes().to_vec(),
        };
        self.upload_file(channel, Some(thread_ts), std::slice::from_ref(&file), None)
            .await
    }
    /// `chat.getPermalink`.
    async fn permalink(&self, channel: &str, ts: &str) -> Result<String, ApiError>;
    /// GET `url_private_download` with the bot token.
    async fn download(&self, url: &str) -> Result<Vec<u8>, ApiError>;
    /// POST `{"response_type":"ephemeral","text"}` to a slash command's
    /// `response_url`. Fakes without a transport keep the erroring default.
    async fn respond(&self, url: &str, text: &str) -> Result<(), ApiError> {
        let _ = (url, text);
        Err(ApiError::Transport(
            "response_url posting is not supported".into(),
        ))
    }
    /// POST `{"response_type":"ephemeral","text","blocks","replace_original"}`
    /// to a slash command's or click's `response_url` (21 §10.2).
    async fn respond_blocks(
        &self,
        url: &str,
        text: &str,
        blocks: serde_json::Value,
        replace_original: bool,
    ) -> Result<(), ApiError> {
        let _ = (url, text, blocks, replace_original);
        Err(ApiError::Transport("unsupported".into()))
    }
    /// `views.open` with a click's `trigger_id`; `view` is a serde_json modal.
    async fn views_open(&self, trigger_id: &str, view: serde_json::Value) -> Result<(), ApiError> {
        let _ = (trigger_id, view);
        Err(ApiError::Transport("unsupported".into()))
    }
}

/// Exact `error` codes that mean "this call was rate limited".
const RATELIMITED: &str = "ratelimited";

fn map_err(e: SlackClientError) -> ApiError {
    match e {
        SlackClientError::RateLimitError(r) => {
            ApiError::RateLimited(r.retry_after.unwrap_or(Duration::from_secs(1)))
        }
        SlackClientError::ApiError(a) if a.code == RATELIMITED => {
            ApiError::RateLimited(Duration::from_secs(1))
        }
        SlackClientError::ApiError(a) => ApiError::Slack(a.code),
        other => ApiError::Transport(other.to_string()),
    }
}

/// The live façade (21 §4.1).
pub struct Api {
    client: SlackClient<SlackClientHyperHttpsConnector>,
    bot: SlackApiToken,
    http: reqwest::Client,
    // ponytail: one std mutex each; never held across an await.
    pacer: Mutex<HashMap<&'static str, Instant>>,
    calls: Mutex<BTreeMap<&'static str, CallStats>>,
}

impl Api {
    /// Builds the hyper client with morphism's rate controller on.
    ///
    /// Both `ring` (reqwest) and `aws-lc-rs` (morphism's hyper-rustls) are
    /// linked, so rustls cannot pick a provider itself (21 §2.1); the first
    /// `Api` installs one, a no-op when `main` already did.
    pub fn new(bot: &str) -> std::io::Result<Self> {
        if rustls::crypto::CryptoProvider::get_default().is_none() {
            let _ = rustls::crypto::aws_lc_rs::default_provider().install_default();
        }
        let connector =
            SlackClientHyperConnector::new()?.with_rate_control(SlackApiRateControlConfig::new());
        Ok(Self {
            client: SlackClient::new(connector),
            bot: SlackApiToken::new(bot.into()),
            http: reqwest::Client::new(),
            pacer: Mutex::new(HashMap::new()),
            calls: Mutex::new(BTreeMap::new()),
        })
    }

    /// Per-method counters for `Status`.
    pub fn stats(&self) -> BTreeMap<String, CallStats> {
        lock(&self.calls)
            .iter()
            .map(|(k, v)| ((*k).to_owned(), v.clone()))
            .collect()
    }

    /// `auth.test` → `(bot_user_id, team_id, team_url)`.
    pub async fn auth_test(&self) -> Result<(String, String, String), ApiError> {
        let r = self
            .run("auth.test", async {
                self.client.open_session(&self.bot).auth_test().await
            })
            .await?;
        Ok((r.user_id.0, r.team_id.0, r.url.0.to_string()))
    }

    /// `conversations.open` with one user → the DM channel id.
    pub async fn open_dm(&self, user: &str) -> Result<String, ApiError> {
        let req = SlackApiConversationsOpenRequest::new().with_users(vec![user.into()]);
        let r = self
            .run("conversations.open", async {
                self.client
                    .open_session(&self.bot)
                    .conversations_open(&req)
                    .await
            })
            .await?;
        Ok(r.channel.id.0)
    }

    /// Reserves the next slot for `method` and sleeps until it (21 §4.2).
    async fn pace(&self, method: &'static str, gap: Duration) {
        let at = {
            let mut p = lock(&self.pacer);
            let now = Instant::now();
            let at = p.get(method).map_or(now, |t| (*t).max(now));
            p.insert(method, at + gap);
            at
        };
        tokio::time::sleep_until(at).await;
    }

    /// Runs one call under a `slack.api` span, maps the error, bumps the counters.
    async fn run<T>(
        &self,
        method: &'static str,
        call: impl Future<Output = ClientResult<T>>,
    ) -> Result<T, ApiError> {
        let r = call
            .instrument(tracing::debug_span!("slack.api", method))
            .await
            .map_err(map_err);
        let mut calls = lock(&self.calls);
        let c = calls.entry(method).or_default();
        match &r {
            Ok(_) => c.ok += 1,
            Err(ApiError::RateLimited(after)) => {
                c.rate_limited += 1;
                tracing::warn!(
                    method,
                    retry_after_s = after.as_secs(),
                    "slack rate limited"
                );
            }
            Err(e) => {
                c.err += 1;
                tracing::debug!(method, error = %e, "slack call failed");
            }
        }
        r
    }

    fn post(
        &self,
        channel: &str,
        thread_ts: Option<&str>,
        content: SlackMessageContent,
    ) -> impl Future<Output = Result<String, ApiError>> + '_ {
        let req = SlackApiChatPostMessageRequest::new(channel.into(), content)
            .opt_thread_ts(thread_ts.map(Into::into));
        async move {
            self.run("chat.postMessage", async {
                self.client
                    .open_session(&self.bot)
                    .chat_post_message(&req)
                    .await
            })
            .await
            .map(|r| r.ts.0)
        }
    }

    fn update(
        &self,
        channel: &str,
        ts: &str,
        content: SlackMessageContent,
    ) -> impl Future<Output = Result<(), ApiError>> + '_ {
        let req = SlackApiChatUpdateRequest::new(channel.into(), content, ts.into());
        async move {
            self.pace("chat.update", UPDATE_GAP).await;
            self.run("chat.update", async {
                self.client.open_session(&self.bot).chat_update(&req).await
            })
            .await
            .map(|_| ())
        }
    }

    /// POSTs `body` to a `response_url`; counted under `response_url`.
    async fn response_url(&self, url: &str, body: serde_json::Value) -> Result<(), ApiError> {
        let r = self
            .http
            .post(url)
            .json(&body)
            .send()
            .await
            .and_then(reqwest::Response::error_for_status)
            .map(|_| ())
            .map_err(|e| ApiError::Transport(e.to_string()));
        let mut calls = lock(&self.calls);
        let c = calls.entry("response_url").or_default();
        if r.is_ok() {
            c.ok += 1;
        } else {
            c.err += 1;
        }
        r
    }
}

/// Our serde_json blocks as morphism's typed blocks (the slash-command ack,
/// `channel::ack`, answers with a typed `SlackCommandEventResponse`).
pub(crate) fn blocks(v: serde_json::Value) -> Result<Vec<SlackBlock>, ApiError> {
    serde_json::from_value(v).map_err(|e| ApiError::Transport(format!("blocks: {e}")))
}

/// `chat.postMessage` body for [`SlackApi::post_blocks`]: the caller's
/// blocks verbatim. Morphism `e2a4b01`'s `SlackBlock` has no `container`,
/// `plan`, `data_table` or `data_visualization` variant and no catch-all, so
/// the 110 §5 plan and §6 peer container cannot go through
/// `SlackMessageContent` (design 110 as-built INTEGRATE).
#[derive(serde::Serialize)]
struct PostBlocks<'a> {
    channel: &'a str,
    #[serde(skip_serializing_if = "Option::is_none")]
    thread_ts: Option<&'a str>,
    blocks: &'a serde_json::Value,
    unfurl_links: bool,
}

/// `chat.update` body for [`SlackApi::update_blocks`]; see [`PostBlocks`].
#[derive(serde::Serialize)]
struct UpdateBlocks<'a> {
    channel: &'a str,
    ts: &'a str,
    blocks: &'a serde_json::Value,
}

/// `chat.startStream` body (timeline mode); `chunks` are [`Chunk::wire`]
/// objects, so a `blocks` chunk carries any block (110 §8, the same reason
/// as [`PostBlocks`]).
#[derive(serde::Serialize)]
struct StartStream<'a> {
    channel: &'a str,
    thread_ts: &'a str,
    recipient_user_id: &'a str,
    recipient_team_id: &'a str,
    task_display_mode: &'static str,
    #[serde(skip_serializing_if = "Option::is_none")]
    chunks: Option<Vec<serde_json::Value>>,
}

/// `chat.appendStream` body; see [`StartStream`].
#[derive(serde::Serialize)]
struct AppendStream<'a> {
    channel: &'a str,
    ts: &'a str,
    #[serde(skip_serializing_if = "Option::is_none")]
    chunks: Option<Vec<serde_json::Value>>,
}

/// `chat.stopStream` body; `blocks` is the footer `context` block.
#[derive(serde::Serialize)]
struct StopStream<'a> {
    channel: &'a str,
    ts: &'a str,
    #[serde(skip_serializing_if = "Option::is_none")]
    blocks: Option<serde_json::Value>,
    #[serde(skip_serializing_if = "Option::is_none")]
    chunks: Option<Vec<serde_json::Value>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    session_status: Option<SlackAgentSessionStatus>,
}

/// What we read back from `chat.postMessage` / `chat.update` and the three
/// stream methods. Slack echoes the message (with its blocks) too; ignoring
/// it keeps a `container` from failing typed deserialization on the way back.
#[derive(serde::Deserialize)]
struct Posted {
    ts: String,
}

fn lock<T>(m: &Mutex<T>) -> MutexGuard<'_, T> {
    m.lock().unwrap_or_else(PoisonError::into_inner)
}

#[async_trait::async_trait]
impl SlackApi for Api {
    async fn start_stream(
        &self,
        channel: &str,
        thread_ts: &str,
        user: &str,
        team: &str,
        first: Vec<Chunk>,
    ) -> Result<String, ApiError> {
        let req = StartStream {
            channel,
            thread_ts,
            recipient_user_id: user,
            recipient_team_id: team,
            task_display_mode: "timeline",
            chunks: chunks(first)?,
        };
        self.run("chat.startStream", async {
            self.client
                .open_session(&self.bot)
                .http_session_api
                .http_post::<_, Posted>("chat.startStream", &req, Some(&SLACK_TIER2_METHOD_CONFIG))
                .await
        })
        .await
        .map(|r| r.ts)
    }

    async fn append_stream(&self, channel: &str, ts: &str, c: Vec<Chunk>) -> Result<(), ApiError> {
        let req = AppendStream {
            channel,
            ts,
            chunks: chunks(c)?,
        };
        self.pace("chat.appendStream", APPEND_GAP).await;
        self.run("chat.appendStream", async {
            self.client
                .open_session(&self.bot)
                .http_session_api
                .http_post::<_, Posted>(
                    "chat.appendStream",
                    &req,
                    Some(&SLACK_TIER4_METHOD_CONFIG),
                )
                .await
        })
        .await
        .map(|_| ())
    }

    async fn stop_stream(
        &self,
        channel: &str,
        ts: &str,
        c: Vec<Chunk>,
        footer: Option<String>,
        session_status: Option<&str>,
    ) -> Result<(), ApiError> {
        let req = StopStream {
            channel,
            ts,
            blocks: footer.map(|f| {
                serde_json::json!([{
                    "type": "context",
                    "elements": [{ "type": "mrkdwn", "text": f }],
                }])
            }),
            chunks: chunks(c)?,
            session_status: session_status.map(self::session_status).transpose()?,
        };
        self.run("chat.stopStream", async {
            self.client
                .open_session(&self.bot)
                .http_session_api
                .http_post::<_, Posted>("chat.stopStream", &req, Some(&SLACK_TIER2_METHOD_CONFIG))
                .await
        })
        .await
        .map(|_| ())
    }

    async fn post_markdown(
        &self,
        channel: &str,
        thread_ts: Option<&str>,
        markdown: &str,
    ) -> Result<String, ApiError> {
        self.post(
            channel,
            thread_ts,
            SlackMessageContent::new().with_markdown_text(markdown.to_owned()),
        )
        .await
    }

    async fn update_markdown(
        &self,
        channel: &str,
        ts: &str,
        markdown: &str,
    ) -> Result<(), ApiError> {
        self.update(
            channel,
            ts,
            SlackMessageContent::new().with_markdown_text(markdown.to_owned()),
        )
        .await
    }

    async fn post_blocks(
        &self,
        channel: &str,
        thread_ts: Option<&str>,
        blocks: serde_json::Value,
    ) -> Result<String, ApiError> {
        let req = PostBlocks {
            channel,
            thread_ts,
            blocks: &blocks,
            unfurl_links: false,
        };
        self.run("chat.postMessage", async {
            self.client
                .open_session(&self.bot)
                .http_session_api
                .http_post::<_, Posted>(
                    "chat.postMessage",
                    &req,
                    Some(&CHAT_POST_MESSAGE_SPECIAL_LIMIT_RATE_CTL),
                )
                .await
        })
        .await
        .map(|r| r.ts)
    }

    async fn update_blocks(
        &self,
        channel: &str,
        ts: &str,
        blocks: serde_json::Value,
    ) -> Result<(), ApiError> {
        let req = UpdateBlocks {
            channel,
            ts,
            blocks: &blocks,
        };
        self.pace("chat.update", UPDATE_GAP).await;
        self.run("chat.update", async {
            self.client
                .open_session(&self.bot)
                .http_session_api
                .http_post::<_, Posted>("chat.update", &req, Some(&SLACK_TIER3_METHOD_CONFIG))
                .await
        })
        .await
        .map(|_| ())
    }

    async fn ephemeral(&self, channel: &str, user: &str, text: &str) -> Result<(), ApiError> {
        let req = SlackApiChatPostEphemeralRequest::new(
            channel.into(),
            user.into(),
            SlackMessageContent::new().with_text(text.to_owned()),
        );
        self.run("chat.postEphemeral", async {
            self.client
                .open_session(&self.bot)
                .chat_post_ephemeral(&req)
                .await
        })
        .await
        .map(|_| ())
    }

    async fn react(&self, channel: &str, ts: &str, name: &str) -> Result<(), ApiError> {
        let req = SlackApiReactionsAddRequest::new(channel.into(), name.into(), ts.into());
        self.run("reactions.add", async {
            self.client
                .open_session(&self.bot)
                .reactions_add(&req)
                .await
        })
        .await
        .map(|_| ())
    }

    async fn set_session_status(
        &self,
        channel: &str,
        thread_ts: &str,
        status: &str,
    ) -> Result<(), ApiError> {
        let req = SlackApiAgentsSessionsSetStatusRequest::new(session_status(status)?)
            .with_channel_id(channel.into())
            .with_thread_ts(thread_ts.into());
        self.run("agents.sessions.setStatus", async {
            self.client
                .open_session(&self.bot)
                .agents_sessions_set_status(&req)
                .await
        })
        .await
        .map(|_| ())
    }

    async fn rename_session(
        &self,
        channel: &str,
        thread_ts: &str,
        title: &str,
    ) -> Result<(), ApiError> {
        let req = SlackApiAgentsSessionsRenameRequest::new(title.to_owned())
            .with_channel_id(channel.into())
            .with_thread_ts(thread_ts.into());
        self.run("agents.sessions.rename", async {
            self.client
                .open_session(&self.bot)
                .agents_sessions_rename(&req)
                .await
        })
        .await
        .map(|_| ())
    }

    async fn upload_file(
        &self,
        channel: &str,
        thread_ts: Option<&str>,
        files: &[UploadFile],
        comment: Option<&str>,
    ) -> Result<String, ApiError> {
        let mut done = Vec::with_capacity(files.len());
        for f in files {
            let get = SlackApiFilesGetUploadUrlExternalRequest::new(f.name.clone(), f.bytes.len());
            let slot = self
                .run("files.getUploadURLExternal", async {
                    self.client
                        .open_session(&self.bot)
                        .get_upload_url_external(&get)
                        .await
                })
                .await?;
            let put = SlackApiFilesUploadViaUrlRequest::new(
                slot.upload_url,
                f.bytes.clone(),
                f.mime.clone(),
            );
            self.run("files.upload", async {
                self.client
                    .open_session(&self.bot)
                    .files_upload_via_url(&put)
                    .await
            })
            .await?;
            done.push(SlackApiFilesComplete::new(slot.file_id).with_title(f.name.clone()));
        }
        let mut req =
            SlackApiFilesCompleteUploadExternalRequest::new(done).with_channel_id(channel.into());
        if let Some(ts) = thread_ts {
            req = req.with_thread_ts(ts.into());
        }
        if let Some(c) = comment {
            req = req.with_initial_comment(c.to_owned());
        }
        let r = self
            .run("files.completeUploadExternal", async {
                self.client
                    .open_session(&self.bot)
                    .files_complete_upload_external(&req)
                    .await
            })
            .await?;
        r.files
            .into_iter()
            .next()
            .and_then(|f| f.permalink)
            .map(|u| u.to_string())
            .ok_or_else(|| {
                ApiError::Transport("completeUploadExternal returned no permalink".into())
            })
    }

    async fn permalink(&self, channel: &str, ts: &str) -> Result<String, ApiError> {
        let req = SlackApiChatGetPermalinkRequest::new(channel.into(), ts.into());
        self.run("chat.getPermalink", async {
            self.client
                .open_session(&self.bot)
                .chat_get_permalink(&req)
                .await
        })
        .await
        .map(|r| r.permalink.to_string())
    }

    async fn download(&self, url: &str) -> Result<Vec<u8>, ApiError> {
        let span = tracing::debug_span!("slack.api", method = "files.download");
        let r = async {
            let resp = self
                .http
                .get(url)
                .bearer_auth(&self.bot.token_value.0)
                .send()
                .await
                .and_then(reqwest::Response::error_for_status)
                .map_err(|e| ApiError::Transport(e.to_string()))?;
            resp.bytes()
                .await
                .map(|b| b.to_vec())
                .map_err(|e| ApiError::Transport(e.to_string()))
        }
        .instrument(span)
        .await;
        let mut calls = lock(&self.calls);
        let c = calls.entry("files.download").or_default();
        if r.is_ok() {
            c.ok += 1;
        } else {
            c.err += 1;
        }
        r
    }

    async fn respond(&self, url: &str, text: &str) -> Result<(), ApiError> {
        self.response_url(
            url,
            serde_json::json!({ "response_type": "ephemeral", "text": text }),
        )
        .await
    }

    async fn respond_blocks(
        &self,
        url: &str,
        text: &str,
        blocks: serde_json::Value,
        replace_original: bool,
    ) -> Result<(), ApiError> {
        self.response_url(
            url,
            serde_json::json!({
                "response_type": "ephemeral",
                "text": text,
                "blocks": blocks,
                "replace_original": replace_original,
            }),
        )
        .await
    }

    async fn views_open(&self, trigger_id: &str, view: serde_json::Value) -> Result<(), ApiError> {
        let view: SlackView =
            serde_json::from_value(view).map_err(|e| ApiError::Transport(format!("view: {e}")))?;
        let req = SlackApiViewsOpenRequest::new(trigger_id.into(), view);
        self.run("views.open", async {
            self.client.open_session(&self.bot).views_open(&req).await
        })
        .await
        .map(|_| ())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test(start_paused = true)]
    async fn pacer_spaces_reservations_by_the_gap() {
        let api = Api::new("xoxb-test").unwrap();
        let t0 = Instant::now();
        api.pace("chat.appendStream", APPEND_GAP).await;
        api.pace("chat.appendStream", APPEND_GAP).await;
        api.pace("chat.appendStream", APPEND_GAP).await;
        // First slot is immediate; the next two wait one gap each.
        assert_eq!(t0.elapsed(), APPEND_GAP * 2);
        // Other methods are not paced.
        api.pace("chat.postMessage", Duration::ZERO).await;
        assert_eq!(t0.elapsed(), APPEND_GAP * 2);
    }

    #[test]
    fn task_source_urls_are_validated_before_sending() {
        let task = |url: &str| Chunk::Task {
            id: "c1".into(),
            title: "shell".into(),
            status: TaskStatus::Complete,
            details: None,
            output: None,
            sources: vec![Source {
                url: url.into(),
                text: "output".into(),
            }],
        };
        let wire = task("https://example.com/output").wire().unwrap();
        assert_eq!(wire["type"], "task_update");
        assert_eq!(wire["status"], "complete");
        assert_eq!(
            wire["sources"][0],
            serde_json::json!({
                "type": "url", "url": "https://example.com/output", "text": "output"
            })
        );
        assert!(task("not a URL").wire().is_err());
    }

    /// 110 §8: a `blocks` chunk carrying a `data_table` (unknown to
    /// morphism) sits verbatim in `chunks` beside a `task_update` and a
    /// `markdown_text`; the stream responses parse to their `ts`.
    #[test]
    fn blocks_chunk_reaches_the_stream_wire_verbatim() {
        let table = serde_json::json!([{
            "type": "data_table",
            "caption": "Table",
            "column_settings": [{"is_wrapped": true}],
            "rows": [[{"type": "raw_text", "text": "a"}]],
        }]);
        let c = chunks(vec![
            Chunk::Text("before".into()),
            Chunk::Blocks(table.clone()),
            Chunk::Task {
                id: "c1".into(),
                title: "shell".into(),
                status: TaskStatus::InProgress,
                details: None,
                output: None,
                sources: Vec::new(),
            },
        ])
        .unwrap()
        .unwrap();
        assert_eq!(c[0], serde_json::json!({"type": "markdown_text", "text": "before"}));
        assert_eq!(c[1], serde_json::json!({"type": "blocks", "blocks": table}));
        assert_eq!(c[2]["type"], "task_update");
        assert_eq!(c[2]["id"], "c1");
        assert_eq!(c[2]["status"], "in_progress");
        assert!(c[2].get("sources").is_none(), "{:?}", c[2]);
        assert!(chunks(Vec::new()).unwrap().is_none());

        let append = serde_json::to_value(AppendStream {
            channel: "C1",
            ts: "1.1",
            chunks: Some(c.clone()),
        })
        .unwrap();
        assert_eq!(append["chunks"][1]["blocks"], table);
        let start = serde_json::to_value(StartStream {
            channel: "C1",
            thread_ts: "1.1",
            recipient_user_id: "U1",
            recipient_team_id: "T1",
            task_display_mode: "timeline",
            chunks: None,
        })
        .unwrap();
        assert_eq!(start["task_display_mode"], "timeline");
        assert!(start.get("chunks").is_none());
        let stop = serde_json::to_value(StopStream {
            channel: "C1",
            ts: "1.1",
            blocks: Some(serde_json::json!([{"type": "context"}])),
            chunks: Some(c),
            session_status: Some(session_status("active").unwrap()),
        })
        .unwrap();
        assert_eq!(stop["session_status"], "active");
        assert_eq!(stop["blocks"][0]["type"], "context");
        assert_eq!(stop["chunks"][1]["type"], "blocks");
        let r: Posted = serde_json::from_str(
            r#"{"ok":true,"channel":"C1","ts":"1.3","message":{"blocks":[{"type":"data_table"}]}}"#,
        )
        .unwrap();
        assert_eq!(r.ts, "1.3");
    }

    /// 110 §6: a `container` (unknown to morphism's `SlackBlock`) reaches
    /// the wire untouched; the echoed message is not deserialized.
    #[test]
    fn raw_blocks_go_over_the_wire_verbatim_and_only_ts_is_read_back() {
        let blocks = serde_json::json!([{
            "type": "container",
            "title": {"type": "plain_text", "text": "x"},
            "is_collapsible": true,
            "default_collapsed": true,
            "blocks": [{"type": "markdown", "text": "body"}],
        }]);
        assert!(
            super::blocks(blocks.clone()).is_err(),
            "typed path must still refuse"
        );
        let wire = serde_json::to_string(&PostBlocks {
            channel: "C1",
            thread_ts: Some("1.1"),
            blocks: &blocks,
            unfurl_links: false,
        })
        .unwrap();
        assert_eq!(
            wire,
            format!(
                "{{\"channel\":\"C1\",\"thread_ts\":\"1.1\",\"blocks\":{},\"unfurl_links\":false}}",
                serde_json::to_string(&blocks).unwrap()
            )
        );
        let top = serde_json::to_value(PostBlocks {
            channel: "C1",
            thread_ts: None,
            blocks: &blocks,
            unfurl_links: false,
        })
        .unwrap();
        assert!(top.get("thread_ts").is_none());
        let update = serde_json::to_value(UpdateBlocks {
            channel: "C1",
            ts: "1.1",
            blocks: &blocks,
        })
        .unwrap();
        assert_eq!(update["ts"], "1.1");
        assert_eq!(update["blocks"], blocks);
        let r: Posted = serde_json::from_str(
            r#"{"ok":true,"channel":"C1","ts":"1.2","message":{"blocks":[{"type":"container","title":{"type":"plain_text","text":"x"}}]}}"#,
        )
        .unwrap();
        assert_eq!(r.ts, "1.2");
    }
}
