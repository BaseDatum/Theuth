//! Thin façade over the slack-morphism session: the methods the plugin
//! uses, exact error codes, per-method call counters and the cross-thread
//! pacer (design doc 21 §4).

use std::collections::{BTreeMap, HashMap, HashSet};
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
    /// (110 §8). Typed through [`blocks`]: a block morphism does not model
    /// lands in `SlackBlock::Unknown` and serialises verbatim.
    Blocks(serde_json::Value),
}

impl TryFrom<Chunk> for SlackStreamChunk {
    type Error = ApiError;

    fn try_from(c: Chunk) -> Result<Self, Self::Error> {
        Ok(match c {
            Chunk::Text(text) => SlackStreamChunk::MarkdownText { text },
            Chunk::Blocks(b) => SlackStreamChunk::Blocks { blocks: blocks(b)? },
            Chunk::Task {
                id,
                title,
                status,
                details,
                output,
                sources,
            } => SlackStreamChunk::TaskUpdate {
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
            },
        })
    }
}

/// `chunks` for a stream request; `None` when there is nothing to send.
fn chunks(v: Vec<Chunk>) -> Result<Option<Vec<SlackStreamChunk>>, ApiError> {
    (!v.is_empty())
        .then(|| v.into_iter().map(TryInto::try_into).collect())
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
    /// `agents.sessions.setStatus`; status `processing` | `active`;
    /// `initiator` (a user id) only counts when the call creates the session.
    /// `Ok` carries Slack's warning codes, e.g.
    /// `missing_agent_session_stopped_event_subscription`.
    async fn set_session_status(
        &self,
        channel: &str,
        thread_ts: &str,
        status: &str,
        initiator: Option<&str>,
    ) -> Result<Vec<String>, ApiError>;
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

/// What the operator changes for an `agents.sessions.setStatus` /
/// `chat.stopStream` warning or error `code`.
pub(crate) fn session_fix(code: &str) -> &'static str {
    match code {
        "missing_agent_session_stopped_event_subscription" => {
            "the installed Slack app is not subscribed to the agent_session_stopped bot event, so Slack shows \"Stop isn't available\" instead of a Stop button: api.slack.com/apps → theuth → Event Subscriptions → Subscribe to bot events → add agent_session_stopped → Save Changes (or paste crates/slack/manifest.yaml under App Manifest), then reinstall if Slack asks"
        }
        "missing_scope" | "not_allowed_token_type" => {
            "the slack_bot secret must be the bot token (xoxb-…) of an app declared as an agent (features.agent_view and chat:write in crates/slack/manifest.yaml); update the app from the manifest and reinstall it"
        }
        "feature_disabled" => {
            "agent sessions are not enabled for this workspace; a workspace admin must allow agent apps"
        }
        "channel_not_found" | "not_in_channel" | "not_authorized" | "no_permission" => {
            "invite the theuth bot to that channel"
        }
        "invalid_arguments"
        | "thread_ts_required"
        | "thread_ts_not_allowed"
        | "invalid_status"
        | "user_not_found" => {
            "Slack rejected theuth's arguments; this is a theuth bug, report it with this line"
        }
        _ => "see https://docs.slack.dev/reference/methods/agents.sessions.setStatus#errors",
    }
}

/// The parts of an `ok: true` answer that flag a misconfiguration; morphism's
/// typed responses drop them.
#[derive(serde::Deserialize)]
struct Warned {
    /// Comma-separated codes.
    warning: Option<String>,
    response_metadata: Option<WarnedMetadata>,
}

#[derive(serde::Deserialize)]
struct WarnedMetadata {
    warnings: Option<Vec<String>>,
}

impl Warned {
    fn codes(self) -> Vec<String> {
        let mut codes: Vec<String> = self
            .warning
            .iter()
            .flat_map(|w| w.split(','))
            .map(|c| c.trim().to_owned())
            .filter(|c| !c.is_empty())
            .collect();
        for c in self
            .response_metadata
            .and_then(|m| m.warnings)
            .into_iter()
            .flatten()
        {
            if !codes.contains(&c) {
                codes.push(c);
            }
        }
        codes
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
    /// `method:code` already logged by [`Api::warn_once`].
    warned: Mutex<HashSet<String>>,
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
            warned: Mutex::new(HashSet::new()),
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

    /// One WARN per `method` and `code` per process, with what to fix.
    fn warn_once(&self, method: &'static str, kind: &str, code: &str) {
        if lock(&self.warned).insert(format!("{method}:{code}")) {
            tracing::warn!(
                method,
                code,
                "slack {method} {kind} {code}: {}",
                session_fix(code)
            );
        }
    }

    /// POSTs `req` to `method` keeping only the warnings of the answer, each
    /// logged once; errors as [`Api::run`].
    async fn post_warned(
        &self,
        method: &'static str,
        req: &(impl serde::Serialize + Send + Sync),
        tier: &'static SlackApiMethodRateControlConfig,
    ) -> Result<Vec<String>, ApiError> {
        let r = self
            .run(method, async {
                self.client
                    .open_session(&self.bot)
                    .http_session_api
                    .http_post::<_, Warned>(method, req, Some(tier))
                    .await
            })
            .await
            .map(Warned::codes);
        for code in r.iter().flatten() {
            self.warn_once(method, "warning", code);
        }
        r
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

    async fn post(&self, req: SlackApiChatPostMessageRequest) -> Result<String, ApiError> {
        self.run("chat.postMessage", async {
            self.client
                .open_session(&self.bot)
                .chat_post_message(&req)
                .await
        })
        .await
        .map(|r| r.ts.0)
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

/// Our serde_json blocks as morphism's typed blocks. Total since the fork
/// pin (design 110 W6): a block morphism does not model — or whose body
/// does not match its model — lands in `SlackBlock::Unknown` and serialises
/// verbatim, so `container` / `plan` / `data_table` / `data_visualization`
/// reach the wire untouched. Only a non-array / non-object input fails.
pub(crate) fn blocks(v: serde_json::Value) -> Result<Vec<SlackBlock>, ApiError> {
    serde_json::from_value(v).map_err(|e| ApiError::Transport(format!("blocks: {e}")))
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
        let req = SlackApiChatStartStreamRequest::new(channel.into())
            .with_thread_ts(thread_ts.into())
            .with_recipient_user_id(user.into())
            .with_recipient_team_id(team.into())
            .with_task_display_mode(SlackStreamTaskDisplayMode::Timeline)
            .opt_chunks(chunks(first)?);
        self.run("chat.startStream", async {
            self.client
                .open_session(&self.bot)
                .chat_start_stream(&req)
                .await
        })
        .await
        .map(|r| r.ts.0)
    }

    async fn append_stream(&self, channel: &str, ts: &str, c: Vec<Chunk>) -> Result<(), ApiError> {
        let req =
            SlackApiChatAppendStreamRequest::new(channel.into(), ts.into()).opt_chunks(chunks(c)?);
        self.pace("chat.appendStream", APPEND_GAP).await;
        self.run("chat.appendStream", async {
            self.client
                .open_session(&self.bot)
                .chat_append_stream(&req)
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
        let content = SlackMessageContent::new().opt_blocks(footer.map(|f| {
            vec![SlackBlock::Context(SlackContextBlock::new(vec![
                SlackContextBlockElement::MarkDown(SlackBlockMarkDownText::new(f)),
            ]))]
        }));
        let req = SlackApiChatStopStreamRequest::new(channel.into(), ts.into(), content)
            .opt_chunks(chunks(c)?)
            .opt_session_status(session_status.map(self::session_status).transpose()?);
        self.post_warned("chat.stopStream", &req, &SLACK_TIER2_METHOD_CONFIG)
            .await
            .map(drop)
    }

    async fn post_markdown(
        &self,
        channel: &str,
        thread_ts: Option<&str>,
        markdown: &str,
    ) -> Result<String, ApiError> {
        let content = SlackMessageContent::new().with_markdown_text(markdown.to_owned());
        self.post(
            SlackApiChatPostMessageRequest::new(channel.into(), content)
                .opt_thread_ts(thread_ts.map(Into::into)),
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
        b: serde_json::Value,
    ) -> Result<String, ApiError> {
        let content = SlackMessageContent::new().with_blocks(blocks(b)?);
        self.post(
            SlackApiChatPostMessageRequest::new(channel.into(), content)
                .opt_thread_ts(thread_ts.map(Into::into))
                .with_unfurl_links(false),
        )
        .await
    }

    async fn update_blocks(
        &self,
        channel: &str,
        ts: &str,
        b: serde_json::Value,
    ) -> Result<(), ApiError> {
        let content = SlackMessageContent::new().with_blocks(blocks(b)?);
        self.update(channel, ts, content).await
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
        initiator: Option<&str>,
    ) -> Result<Vec<String>, ApiError> {
        const METHOD: &str = "agents.sessions.setStatus";
        let req = SlackApiAgentsSessionsSetStatusRequest::new(session_status(status)?)
            .with_channel_id(channel.into())
            .with_thread_ts(thread_ts.into())
            .opt_initiator_user_id(initiator.map(Into::into));
        let r = self
            .post_warned(METHOD, &req, &SLACK_TIER3_METHOD_CONFIG)
            .await;
        if let Err(ApiError::Slack(code)) = &r {
            self.warn_once(METHOD, "error", code);
        }
        r
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
    fn warned_answer_yields_each_code_once() {
        let w: Warned = serde_json::from_str(
            r#"{"ok":true,"status":"processing","agent_status":"processing",
                "warning":"missing_agent_session_stopped_event_subscription,superfluous_charset",
                "response_metadata":{"warnings":["missing_agent_session_stopped_event_subscription"]}}"#,
        )
        .unwrap();
        assert_eq!(
            w.codes(),
            [
                "missing_agent_session_stopped_event_subscription",
                "superfluous_charset"
            ]
        );
        let clean: Warned = serde_json::from_str(r#"{"ok":true,"status":"active"}"#).unwrap();
        assert!(clean.codes().is_empty());
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
        let wire = serde_json::to_value(
            SlackStreamChunk::try_from(task("https://example.com/output")).unwrap(),
        )
        .unwrap();
        assert_eq!(wire["type"], "task_update");
        assert_eq!(wire["status"], "complete");
        assert_eq!(
            wire["sources"][0],
            serde_json::json!({
                "type": "url", "url": "https://example.com/output", "text": "output"
            })
        );
        assert!(SlackStreamChunk::try_from(task("not a URL")).is_err());
    }

    /// 110 §8: a `blocks` chunk carrying a `data_table` (typed by the fork)
    /// and a block morphism does not model (`SlackBlock::Unknown`) reaches
    /// `chunks` value-equal beside a `task_update` and a `markdown_text`;
    /// the typed stream responses parse an echoed message holding them.
    #[test]
    fn blocks_chunk_reaches_the_stream_wire_verbatim() {
        let table = serde_json::json!([
            {
                "type": "data_table",
                "caption": "Table",
                "rows": [
                    [{"type": "raw_text", "text": "n"}],
                    [{"type": "raw_number", "value": 3.0, "text": "3"}],
                ],
            },
            {"type": "some_future_block", "x": [1, 2]},
        ]);
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
        assert!(matches!(c[1], SlackStreamChunk::Blocks { .. }));
        assert!(chunks(Vec::new()).unwrap().is_none());

        let append = serde_json::to_value(
            SlackApiChatAppendStreamRequest::new("C1".into(), "1.1".into()).with_chunks(c.clone()),
        )
        .unwrap();
        assert_eq!(
            append["chunks"][0],
            serde_json::json!({"type": "markdown_text", "text": "before"})
        );
        assert_eq!(append["chunks"][1]["blocks"], table);
        assert_eq!(append["chunks"][2]["type"], "task_update");
        assert_eq!(append["chunks"][2]["id"], "c1");
        assert_eq!(append["chunks"][2]["status"], "in_progress");
        assert!(append["chunks"][2].get("sources").is_none(), "{append:?}");
        let start = serde_json::to_value(
            SlackApiChatStartStreamRequest::new("C1".into())
                .with_thread_ts("1.1".into())
                .with_task_display_mode(SlackStreamTaskDisplayMode::Timeline),
        )
        .unwrap();
        assert_eq!(start["task_display_mode"], "timeline");
        assert!(start.get("chunks").is_none());
        let footer = SlackMessageContent::new().with_blocks(vec![SlackBlock::Context(
            SlackContextBlock::new(vec![SlackContextBlockElement::MarkDown(
                SlackBlockMarkDownText::new("f".into()),
            )]),
        )]);
        let stop = serde_json::to_value(
            SlackApiChatStopStreamRequest::new("C1".into(), "1.1".into(), footer)
                .with_chunks(c)
                .with_session_status(session_status("active").unwrap()),
        )
        .unwrap();
        assert_eq!(stop["session_status"], "active");
        assert_eq!(
            stop["blocks"],
            serde_json::json!([{"type": "context", "elements": [{"type": "mrkdwn", "text": "f"}]}])
        );
        assert_eq!(stop["chunks"][1]["blocks"], table);

        let r: SlackApiChatStartStreamResponse =
            serde_json::from_str(r#"{"ok":true,"channel":"C1","ts":"1.3"}"#).unwrap();
        assert_eq!(r.ts.0, "1.3");
        let echo = serde_json::json!({
            "ok": true, "channel": "C1", "ts": "1.3",
            "message": {"type": "message", "ts": "1.3", "bot_id": "B1", "text": "", "blocks": table},
        });
        let r: SlackApiChatStopStreamResponse = serde_json::from_value(echo).unwrap();
        assert_eq!(r.ts.0, "1.3");
        assert_eq!(
            serde_json::to_value(r.message.unwrap().content.blocks.unwrap()).unwrap(),
            table
        );
    }

    /// 110 §5/§6 after the fork pin: the `container` `ui::collapsed` builds,
    /// the `plan` of `plan::blocks` and an unmodelled block all go through
    /// `blocks()` value-equal, sit in the typed `chat.postMessage` /
    /// `chat.update` requests, and the typed responses parse Slack's echo.
    #[test]
    fn typed_blocks_round_trip_container_plan_and_unknown() {
        let input = serde_json::json!([
            {
                "type": "container",
                "title": {"type": "plain_text", "text": "x"},
                "subtitle": {"type": "plain_text", "text": "first line"},
                "is_collapsible": true,
                "default_collapsed": true,
                "child_blocks": [
                    {"type": "section", "text": {"type": "plain_text", "text": "body"}},
                    {"type": "context", "elements": [{"type": "plain_text", "text": "… 3 more chars"}]},
                ],
            },
            {
                "type": "plan",
                "title": "read the code",
                "tasks": [
                    {"task_id": "0", "title": "read the code", "status": "in_progress"},
                    {
                        "task_id": "1", "title": "wait", "status": "error",
                        "details": {"type": "rich_text", "elements": [{
                            "type": "rich_text_section",
                            "elements": [{"type": "text", "text": "waiting on the user"}],
                        }]},
                    },
                ],
            },
            {"type": "callout", "text": "unmodelled"},
        ]);
        let typed = blocks(input.clone()).unwrap();
        assert!(matches!(typed[0], SlackBlock::Container(_)));
        assert!(matches!(typed[1], SlackBlock::Plan(_)));
        assert!(matches!(typed[2], SlackBlock::Unknown(_)));
        assert_eq!(serde_json::to_value(&typed).unwrap(), input);
        assert!(blocks(serde_json::json!("not blocks")).is_err());

        let content = SlackMessageContent::new().with_blocks(typed.clone());
        let post = serde_json::to_value(
            SlackApiChatPostMessageRequest::new("C1".into(), content.clone())
                .opt_thread_ts(Some("1.1".into()))
                .with_unfurl_links(false),
        )
        .unwrap();
        assert_eq!(post["channel"], "C1");
        assert_eq!(post["thread_ts"], "1.1");
        assert_eq!(post["unfurl_links"], false);
        assert_eq!(post["blocks"], input);
        let top = serde_json::to_value(
            SlackApiChatPostMessageRequest::new("C1".into(), content.clone()).opt_thread_ts(None),
        )
        .unwrap();
        assert!(top.get("thread_ts").is_none());
        let update = serde_json::to_value(SlackApiChatUpdateRequest::new(
            "C1".into(),
            content,
            "1.1".into(),
        ))
        .unwrap();
        assert_eq!(update["ts"], "1.1");
        assert_eq!(update["blocks"], input);

        let echo = serde_json::json!({
            "ok": true, "channel": "C1", "ts": "1.2",
            "message": {
                "type": "message", "subtype": "bot_message", "ts": "1.2", "bot_id": "B1",
                "text": "", "blocks": input,
            },
        });
        let r: SlackApiChatPostMessageResponse = serde_json::from_value(echo.clone()).unwrap();
        assert_eq!(r.ts.0, "1.2");
        assert_eq!(
            serde_json::to_value(r.message.content.blocks.unwrap()).unwrap(),
            input
        );
        let r: SlackApiChatUpdateResponse = serde_json::from_value(echo).unwrap();
        assert_eq!(r.ts.0, "1.2");
    }

    /// Every block `surface::slack_blocks` emits (the `crates/surface`
    /// goldens: container, data_table, data_visualization, alert, header,
    /// image, rich_text, section, context, divider) survives the typed path
    /// value-equal. Known fork gap: `SlackHeaderBlock` has no `level`, so a
    /// `header.level` is dropped; stripped on both sides here.
    // ponytail: drop `strip` once the fork models `header.level`.
    #[test]
    fn surface_goldens_round_trip_through_typed_blocks() {
        fn strip(v: &mut serde_json::Value) {
            if v["type"] == "header" {
                v.as_object_mut().unwrap().remove("level");
            }
            if let Some(kids) = v.get_mut("child_blocks").and_then(|k| k.as_array_mut()) {
                kids.iter_mut().for_each(strip);
            }
        }
        let dir = concat!(env!("CARGO_MANIFEST_DIR"), "/../surface/fixtures/slack");
        let mut seen = 0;
        for e in std::fs::read_dir(dir).unwrap() {
            let p = e.unwrap().path();
            if !p.to_str().unwrap().ends_with(".blocks.json") {
                continue;
            }
            let v: serde_json::Value =
                serde_json::from_str(&std::fs::read_to_string(&p).unwrap()).unwrap();
            if !v.is_array() {
                continue; // an error golden
            }
            let mut back = serde_json::to_value(blocks(v.clone()).unwrap()).unwrap();
            let mut want = v;
            for b in back.as_array_mut().unwrap() {
                strip(b);
            }
            for b in want.as_array_mut().unwrap() {
                strip(b);
            }
            assert_eq!(back, want, "{}", p.display());
            seen += 1;
        }
        assert!(seen >= 10, "{seen} goldens");
    }
}
