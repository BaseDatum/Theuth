//! `wth --acp`: the agent half of the Agent Client Protocol on stdio (design
//! doc 06 §4, §8.12-13). The same "registry lookup + `ask`" glue as
//! [`crate::api`], with the ACP client (an editor) as the only frontend.
//!
//! One kameo actor, [`Relay`], is subscribed to the bus: it turns
//! [`AgentEvent`]s into `session/update` notifications, answers
//! `session/prompt` when the session's `TurnFinished` arrives, and forwards
//! [`ApprovalRequested`] as `session/request_permission`. The wth
//! [`SessionId`] string *is* the ACP session id.

use std::collections::HashMap;
use std::fmt::Write as _;
use std::path::{Path, PathBuf};

use agent::{
    AgentEvent, Agents, Cancel, HistoryEntry, ListSessions, LoadSession, NewSession, SendInput,
};
use agent_client_protocol::schema::v1::{
    AgentCapabilities, CancelNotification, ContentBlock, ContentChunk, EmbeddedResourceResource,
    Implementation, InitializeRequest, InitializeResponse, ListSessionsRequest,
    ListSessionsResponse, LoadSessionRequest, LoadSessionResponse, NewSessionRequest,
    NewSessionResponse, PermissionOption, PermissionOptionKind, PromptCapabilities, PromptRequest,
    PromptResponse, RequestPermissionOutcome, RequestPermissionRequest, SessionId as AcpSessionId,
    SessionInfo, SessionNotification, SessionUpdate, StopReason, ToolCall, ToolCallContent,
    ToolCallStatus, ToolCallUpdate, ToolCallUpdateFields, UsageUpdate,
};
use agent_client_protocol::{Agent, Client, ConnectionTo, Error, Stdio};
use base64::Engine as _;
use jsonrpsee::types::ErrorObjectOwned;
use kameo::Actor;
use kameo::actor::{ActorRef, Spawn};
use kameo::message::{Context, Message};
use runtime::{Runtime, SessionId};
use sha2::{Digest, Sha256};
use tokio::sync::oneshot;
use tools::{Approval, ApprovalRequested, Decision, Resolve, Risk};

use crate::api;

/// The plugin: ACP on stdin/stdout until the client closes stdin, then
/// `rt.shutdown`. Nothing else may write to stdout in this mode.
pub struct AcpPlugin;

#[async_trait::async_trait]
impl runtime::Plugin for AcpPlugin {
    fn name(&self) -> &'static str {
        "acp"
    }

    async fn start(self: Box<Self>, rt: &Runtime) -> anyhow::Result<()> {
        let relay = Relay::spawn_with_mailbox(
            Relay {
                cx: None,
                waiters: HashMap::new(),
            },
            runtime::subscriber_mailbox(),
        );
        rt.subscribe::<Relay, AgentEvent>(&relay).await;
        rt.subscribe::<Relay, ApprovalRequested>(&relay).await;
        let shutdown = rt.shutdown.clone();
        let data_dir = rt.data_dir.clone();
        tokio::spawn(async move {
            if let Err(e) = connect(relay, data_dir).await {
                tracing::error!(error = %e, "acp connection failed");
            }
            shutdown.cancel();
        });
        Ok(())
    }
}

/// Serve ACP on stdio; returns when stdin hits EOF or the transport fails.
/// `data_dir` hosts `attachments/` for prompt images (17 §5).
async fn connect(relay: ActorRef<Relay>, data_dir: PathBuf) -> Result<(), Error> {
    let for_prompt = relay.clone();
    Agent
        .builder()
        .name("wth")
        .on_receive_request(
            async move |req: InitializeRequest, responder, _cx| {
                responder.respond(
                    InitializeResponse::new(req.protocol_version)
                        .agent_capabilities(
                            AgentCapabilities::new()
                                .load_session(true)
                                .prompt_capabilities(
                                    PromptCapabilities::new().embedded_context(true).image(true),
                                ),
                        )
                        .agent_info(Implementation::new("wth", env!("CARGO_PKG_VERSION"))),
                )
            },
            agent_client_protocol::on_receive_request!(),
        )
        .on_receive_request(
            async move |req: NewSessionRequest, responder, _cx| {
                let id =
                    api::ask::<Agents, _>(agent::AGENT_ACTOR, NewSession { cwd: Some(req.cwd) })
                        .await
                        .map_err(rpc_error);
                responder.respond_with_result(id.map(|id| NewSessionResponse::new(id.to_string())))
            },
            agent_client_protocol::on_receive_request!(),
        )
        .on_receive_request(
            async move |req: LoadSessionRequest, responder, cx| {
                let session = parse_session(&req.session_id)?;
                let loaded = api::ask::<Agents, _>(agent::AGENT_ACTOR, LoadSession { session })
                    .await
                    .map_err(rpc_error)?;
                for (i, entry) in loaded.history.into_iter().enumerate() {
                    cx.send_notification(SessionNotification::new(
                        req.session_id.clone(),
                        replay(i, entry),
                    ))?;
                }
                responder.respond(LoadSessionResponse::new())
            },
            agent_client_protocol::on_receive_request!(),
        )
        .on_receive_request(
            async move |req: ListSessionsRequest, responder, _cx| {
                let sessions = api::ask::<Agents, _>(agent::AGENT_ACTOR, ListSessions)
                    .await
                    .map_err(rpc_error)?;
                let here = std::env::current_dir().unwrap_or_default();
                let infos = sessions
                    .into_iter()
                    .map(|s| {
                        let cwd = s.cwd.map_or_else(|| here.clone(), PathBuf::from);
                        SessionInfo::new(s.id.to_string(), cwd)
                            .title(s.title)
                            .updated_at(s.updated_at.to_rfc3339())
                    })
                    .filter(|s| req.cwd.as_ref().is_none_or(|want| *want == s.cwd))
                    .collect();
                responder.respond(ListSessionsResponse::new(infos))
            },
            agent_client_protocol::on_receive_request!(),
        )
        .on_receive_request(
            async move |req: PromptRequest, responder, cx| {
                let session = parse_session(&req.session_id)?;
                let (text, files) = prompt_parts(&req.prompt, &data_dir);
                // Register before `SendInput` so the bus cannot deliver
                // `TurnFinished` ahead of the waiter (one ordered mailbox).
                let (tx, rx) = oneshot::channel();
                let _ = for_prompt.tell(Wait { session, tx }).await;
                api::ask::<Agents, _>(
                    agent::AGENT_ACTOR,
                    SendInput {
                        session: Some(session),
                        text,
                        files,
                    },
                )
                .await
                .map_err(rpc_error)?;
                cx.spawn(async move {
                    // A dropped sender means the connection is going away.
                    let stop = rx.await.unwrap_or_default();
                    let _ = responder.respond(PromptResponse::new(stop_reason(stop)));
                    Ok(())
                })
            },
            agent_client_protocol::on_receive_request!(),
        )
        .on_receive_notification(
            async move |n: CancelNotification, _cx| {
                let session = parse_session(&n.session_id)?;
                api::ask::<Agents, _>(agent::AGENT_ACTOR, Cancel { session })
                    .await
                    .map_err(rpc_error)
            },
            agent_client_protocol::on_receive_notification!(),
        )
        .connect_with(Stdio::new(), async move |cx: ConnectionTo<Client>| {
            let _ = relay.tell(SetConnection(cx.clone())).await;
            cx.incoming_closed().await;
            Ok(())
        })
        .await
}

/// A JSON-RPC error from [`crate::api`] as an ACP error (same code and text).
fn rpc_error(e: ErrorObjectOwned) -> Error {
    Error::new(e.code(), e.message())
}

/// The wth session behind an ACP session id.
fn parse_session(id: &AcpSessionId) -> Result<SessionId, Error> {
    id.0.parse()
        .map_err(|_| Error::invalid_params().data(format!("unknown session {id}")))
}

/// `session/prompt` content as one user message: text verbatim, embedded text
/// resources fenced with their uri, links as their uri; image blocks and PDF
/// blob resources decoded to files under `<data_dir>/attachments/` and
/// returned as paths (17 §5, 20 §2.5).
fn prompt_parts(blocks: &[ContentBlock], data_dir: &Path) -> (String, Vec<PathBuf>) {
    let mut out = String::new();
    let mut files = Vec::new();
    for block in blocks {
        if !out.is_empty() {
            out.push('\n');
        }
        match block {
            ContentBlock::Text(t) => out.push_str(&t.text),
            ContentBlock::ResourceLink(l) => out.push_str(&l.uri),
            ContentBlock::Resource(r) => match &r.resource {
                EmbeddedResourceResource::TextResourceContents(t) => {
                    let _ = write!(out, "```{}\n{}\n```", t.uri, t.text);
                }
                EmbeddedResourceResource::BlobResourceContents(b) => {
                    match save_blob(&b.blob, b.mime_type.as_deref().unwrap_or(""), data_dir) {
                        Ok(Some(path)) => {
                            out.pop();
                            files.push(path);
                        }
                        Ok(None) => {
                            let _ = write!(out, "[binary resource {}]", b.uri);
                        }
                        Err(e) => {
                            let _ = write!(out, "[binary resource {}: {e}]", b.uri);
                        }
                    }
                }
                _ => out.push_str("[resource]"),
            },
            ContentBlock::Image(img) => match save_blob(&img.data, &img.mime_type, data_dir) {
                Ok(Some(path)) => {
                    // No text for this block: drop the separator pushed above.
                    out.pop();
                    files.push(path);
                }
                Ok(None) => {
                    let _ = write!(out, "[image: unsupported {}]", img.mime_type);
                }
                Err(e) => {
                    let _ = write!(out, "[image: {e}]");
                }
            },
            ContentBlock::Audio(_) => out.push_str("[audio]"),
            _ => out.push_str("[unsupported content]"),
        }
    }
    (out, files)
}

/// Decodes a base64 image block or PDF blob into
/// `<data_dir>/attachments/<sha256[..16]>.<ext>`; `None` for a mime type
/// outside png/jpeg/gif/webp/pdf. An existing file is not rewritten.
fn save_blob(data: &str, mime: &str, data_dir: &Path) -> anyhow::Result<Option<PathBuf>> {
    let ext = match mime {
        "image/png" => "png",
        "image/jpeg" => "jpg",
        "image/gif" => "gif",
        "image/webp" => "webp",
        "application/pdf" => "pdf",
        _ => return Ok(None),
    };
    let bytes = base64::engine::general_purpose::STANDARD.decode(data)?;
    let hex = format!("{:x}", Sha256::digest(&bytes));
    let dir = data_dir.join("attachments");
    let path = dir.join(format!("{}.{ext}", &hex[..16]));
    if !path.exists() {
        std::fs::create_dir_all(&dir)?;
        std::fs::write(&path, bytes)?;
    }
    Ok(Some(path))
}

/// wth's stop reason string as ACP's: matching serde names pass through,
/// anything else (`error`, provider-specific) is `end_turn`.
fn stop_reason(s: String) -> StopReason {
    serde_json::from_value(serde_json::Value::String(s)).unwrap_or(StopReason::EndTurn)
}

/// One stored history entry as a `session/load` replay update.
fn replay(i: usize, entry: HistoryEntry) -> SessionUpdate {
    match entry {
        HistoryEntry::User { text } => {
            SessionUpdate::UserMessageChunk(ContentChunk::new(text.into()))
        }
        HistoryEntry::Assistant { text } => {
            SessionUpdate::AgentMessageChunk(ContentChunk::new(text.into()))
        }
        HistoryEntry::Tool {
            name, args, result, ..
        } => {
            let (status, output) = match result {
                Some((out, true)) => (ToolCallStatus::Failed, Some(out)),
                Some((out, false)) => (ToolCallStatus::Completed, Some(out)),
                None => (ToolCallStatus::InProgress, None),
            };
            let call = ToolCall::new(format!("h{i}"), name.clone())
                .kind(acp::kind(&name, Risk::Mutating))
                .status(status)
                .raw_input(args);
            SessionUpdate::ToolCall(match output {
                Some(out) => call.raw_output(serde_json::Value::String(out)),
                None => call,
            })
        }
        HistoryEntry::Compacted { summary, .. } => SessionUpdate::AgentMessageChunk(
            ContentChunk::new(format!("[compacted] {summary}").into()),
        ),
    }
}

/// Bus subscriber holding the connection and the prompt waiters.
#[derive(Actor)]
struct Relay {
    /// Set by the connection's main task; events before that are dropped.
    cx: Option<ConnectionTo<Client>>,
    /// `session/prompt` calls waiting for their session's `TurnFinished`.
    waiters: HashMap<SessionId, oneshot::Sender<String>>,
}

impl Relay {
    fn notify(&self, session: SessionId, update: SessionUpdate) {
        let Some(cx) = &self.cx else { return };
        let n = SessionNotification::new(AcpSessionId::new(session.to_string()), update);
        if let Err(e) = cx.send_notification(n) {
            tracing::warn!(error = %e, "acp session/update failed");
        }
    }
}

/// The connection handle, once the transport is up.
struct SetConnection(ConnectionTo<Client>);

impl Message<SetConnection> for Relay {
    type Reply = ();
    async fn handle(&mut self, m: SetConnection, _: &mut Context<Self, ()>) {
        self.cx = Some(m.0);
    }
}

/// Resolve `tx` with the stop reason of `session`'s next `TurnFinished`.
struct Wait {
    session: SessionId,
    tx: oneshot::Sender<String>,
}

impl Message<Wait> for Relay {
    type Reply = ();
    async fn handle(&mut self, m: Wait, _: &mut Context<Self, ()>) {
        self.waiters.insert(m.session, m.tx);
    }
}

impl Message<AgentEvent> for Relay {
    type Reply = ();
    async fn handle(&mut self, ev: AgentEvent, _: &mut Context<Self, ()>) {
        match ev {
            AgentEvent::TextDelta { session, text, .. } => self.notify(
                session,
                SessionUpdate::AgentMessageChunk(ContentChunk::new(ContentBlock::from(&*text))),
            ),
            AgentEvent::ReasoningDelta { session, text, .. } => self.notify(
                session,
                SessionUpdate::AgentThoughtChunk(ContentChunk::new(ContentBlock::from(&*text))),
            ),
            AgentEvent::ToolCallStarted {
                session,
                call_id,
                name,
                args,
                ..
            } => self.notify(
                session,
                SessionUpdate::ToolCall(
                    ToolCall::new(call_id, name.clone())
                        // The registry's risk is not on the event; every
                        // non-read name renders as its 06 §4.1 kind anyway.
                        .kind(acp::kind(&name, Risk::Mutating))
                        .status(ToolCallStatus::InProgress)
                        .raw_input(args),
                ),
            ),
            AgentEvent::ToolCallFinished {
                session,
                call_id,
                output,
                is_error,
                ..
            } => self.notify(
                session,
                SessionUpdate::ToolCallUpdate(ToolCallUpdate::new(
                    call_id,
                    ToolCallUpdateFields::new()
                        .status(if is_error {
                            ToolCallStatus::Failed
                        } else {
                            ToolCallStatus::Completed
                        })
                        .content(vec![ToolCallContent::from(output)]),
                )),
            ),
            AgentEvent::TurnFinished {
                session,
                stop_reason,
                context,
                ..
            } => {
                // ponytail: window size unknown here (the catalog lives in
                // `models`); 0 until a client needs the ratio.
                self.notify(
                    session,
                    SessionUpdate::UsageUpdate(UsageUpdate::new(u64::from(context), 0)),
                );
                if let Some(tx) = self.waiters.remove(&session) {
                    let _ = tx.send(stop_reason);
                }
            }
            AgentEvent::MessageDelivered { session, text, .. } => self.notify(
                session,
                SessionUpdate::UserMessageChunk(ContentChunk::new(text.into())),
            ),
            AgentEvent::Error {
                session, message, ..
            } => self.notify(
                session,
                SessionUpdate::AgentMessageChunk(ContentChunk::new(
                    format!("error: {message}").into(),
                )),
            ),
            _ => {}
        }
    }
}

impl Message<ApprovalRequested> for Relay {
    type Reply = ();
    async fn handle(&mut self, a: ApprovalRequested, _: &mut Context<Self, ()>) {
        let Some(cx) = self.cx.clone() else { return };
        let Ok(approval) = api::lookup::<Approval>(tools::APPROVAL_ACTOR) else {
            return;
        };
        let tool_call = ToolCallUpdate::new(
            a.id.0.to_string(),
            ToolCallUpdateFields::new()
                .title(a.tool.clone())
                .kind(acp::kind(&a.tool, a.risk))
                .raw_input(a.args),
        );
        let req = RequestPermissionRequest::new(
            AcpSessionId::new(a.session.to_string()),
            tool_call,
            vec![
                PermissionOption::new("allow", "Allow", PermissionOptionKind::AllowOnce),
                PermissionOption::new("deny", "Deny", PermissionOptionKind::RejectOnce),
            ],
        );
        let id = a.id;
        // A plain tokio task: a slow human must not hold the dispatch loop,
        // and `Resolve` on an already timed-out id is a harmless `false`.
        tokio::spawn(async move {
            let decision = match cx.send_request(req).block_task().await {
                Ok(r) => match r.outcome {
                    RequestPermissionOutcome::Selected(s) if &*s.option_id.0 == "allow" => {
                        Decision::Allow
                    }
                    _ => Decision::Deny,
                },
                Err(e) => {
                    tracing::warn!(error = %e, "acp request_permission failed");
                    Decision::Deny
                }
            };
            let _ = approval.ask(Resolve { id, decision }).await;
        });
    }
}

#[cfg(test)]
mod tests {
    use agent_client_protocol::schema::v1::{
        BlobResourceContents, EmbeddedResource, ImageContent, TextContent,
    };

    use super::*;

    #[test]
    fn image_block_becomes_sha_named_file() {
        let dir = std::env::temp_dir().join(format!("wth-acp-{}", uuid::Uuid::new_v4()));
        let bytes = b"\x89PNG\r\n\x1a\nnot really a png";
        let b64 = base64::engine::general_purpose::STANDARD.encode(bytes);
        let pdf = b"%PDF-1.7 not really";
        let pdf_b64 = base64::engine::general_purpose::STANDARD.encode(pdf);
        let blob = |b64: &str, mime: &str, uri: &str| {
            let mut r = BlobResourceContents::new(b64, uri);
            r.mime_type = Some(mime.to_owned());
            ContentBlock::Resource(EmbeddedResource::new(
                EmbeddedResourceResource::BlobResourceContents(r),
            ))
        };
        let blocks = [
            ContentBlock::Text(TextContent::new("look")),
            ContentBlock::Image(ImageContent::new(b64, "image/png")),
            ContentBlock::Image(ImageContent::new("AAAA", "image/tiff")),
            ContentBlock::Image(ImageContent::new("not base64!", "image/png")),
            blob(&pdf_b64, "application/pdf", "file:///r.pdf"),
            blob("AAAA", "application/zip", "file:///r.zip"),
        ];
        let (text, files) = prompt_parts(&blocks, &dir);
        let hex = format!("{:x}", Sha256::digest(bytes));
        let pdf_hex = format!("{:x}", Sha256::digest(pdf));
        assert_eq!(
            files,
            [
                dir.join("attachments").join(format!("{}.png", &hex[..16])),
                dir.join("attachments").join(format!("{}.pdf", &pdf_hex[..16])),
            ]
        );
        assert_eq!(std::fs::read(&files[0]).unwrap(), bytes);
        assert_eq!(std::fs::read(&files[1]).unwrap(), pdf);
        assert!(
            text.starts_with("look\n[image: unsupported image/tiff]\n[image: "),
            "{text}"
        );
        assert!(text.ends_with("\n[binary resource file:///r.zip]"), "{text}");
        std::fs::remove_dir_all(&dir).unwrap();
    }
}
