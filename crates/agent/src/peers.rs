//! Peer tools (design doc 01 §2.4): `spawn_subagent` returns immediately;
//! `send_message` routes text between a session and its parent/children.
//! Both are `Risk::ReadOnly`: nothing outside the harness changes.
//!
//! ponytail: the registry is global, so both tools are visible to every
//! session and depth/peer availability is enforced at call time; per-session
//! tool visibility is the upgrade path.

use std::path::PathBuf;
use std::sync::Arc;
use std::time::Duration;

use kameo::actor::ActorRef;
use kameo::error::SendError;
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use tools::{Risk, Tool, ToolCx, ToolOutput};

use crate::supervisor::{Deliver, SpawnChild};
use crate::{AGENT_ACTOR, AgentConfig, Agents};

/// Arguments for [`SpawnSubagentTool`].
#[derive(Debug, Serialize, Deserialize, JsonSchema)]
pub struct SpawnSubagentArgs {
    /// Complete, self-contained instructions; the subagent sees nothing of this conversation.
    pub task: String,
    /// Stable peer name (used to address it with send_message); generated when omitted.
    #[serde(default)]
    pub name: Option<String>,
    /// An external harness from ~/.wth/acp/agents.yaml; default: a wth subagent.
    #[serde(default)]
    pub agent: Option<String>,
    /// Absolute working directory for an external harness; default: wth's cwd.
    #[serde(default)]
    pub cwd: Option<PathBuf>,
}

/// Non-blocking delegation to a child session.
pub struct SpawnSubagentTool {
    /// For `acp/agents.yaml`: the harness names the description offers.
    pub data_dir: PathBuf,
}

/// The configured harnesses, empty (and logged) when the file is unreadable.
fn harnesses(data_dir: &std::path::Path) -> std::collections::BTreeMap<String, acp::Entry> {
    acp::load(data_dir).unwrap_or_else(|e| {
        tracing::warn!("acp/agents.yaml: {e:#}");
        Default::default()
    })
}

fn supervisor() -> Result<ActorRef<Agents>, ToolOutput> {
    ActorRef::<Agents>::lookup(AGENT_ACTOR)
        .ok()
        .flatten()
        .ok_or_else(|| ToolOutput::err("agent supervisor not running"))
}

#[async_trait::async_trait]
impl Tool for SpawnSubagentTool {
    const NAME: &'static str = "spawn_subagent";
    const RISK: Risk = Risk::ReadOnly;
    type Args = SpawnSubagentArgs;

    fn description(&self) -> String {
        let mut s = String::from(
            "Delegate a complete, self-contained task to a subagent running in its own session while you continue. \
             Returns immediately with its name; its final message arrives later as a <subagent_result> input, so do not poll. \
             Several calls in one message run concurrently. Message it with send_message.",
        );
        let line = acp::describe(&harnesses(&self.data_dir));
        if !line.is_empty() {
            s.push(' ');
            s.push_str(&line);
        }
        s
    }

    async fn call(&self, args: SpawnSubagentArgs, cx: ToolCx) -> anyhow::Result<ToolOutput> {
        let agents = match supervisor() {
            Ok(a) => a,
            Err(out) => return Ok(out),
        };
        if let Some(agent) = &args.agent {
            let known = harnesses(&self.data_dir);
            if !known.contains_key(agent) {
                let names: Vec<&str> = known.keys().map(String::as_str).collect();
                return Ok(ToolOutput::err(format!(
                    "unknown agent `{agent}`; configured: {}",
                    names.join(", ")
                )));
            }
        }
        Ok(
            match agents
                .ask(SpawnChild {
                    parent: cx.session,
                    name: args.name,
                    task: args.task,
                    agent: args.agent,
                    cwd: args.cwd,
                })
                .await
            {
                Ok((name, id)) => ToolOutput::ok(format!(
                    "Spawned `{name}` (session {id}). Its result will arrive as a <subagent_result> when it finishes; message it with send_message."
                )),
                Err(SendError::HandlerError(e)) => ToolOutput::err(e),
                Err(e) => ToolOutput::err(format!("agent supervisor: {e}")),
            },
        )
    }
}

/// Arguments for [`SendMessageTool`].
#[derive(Debug, Serialize, Deserialize, JsonSchema)]
pub struct SendMessageArgs {
    /// `parent` (when you are a subagent) or the name of one of your subagents.
    pub to: String,
    /// Body.
    pub message: String,
    /// Block until the addressee's next message to you (bounded by a timeout).
    #[serde(default)]
    pub await_reply: bool,
    /// Id of the `<message>` you are answering.
    #[serde(default)]
    pub reply_to: Option<String>,
}

/// Symmetric peer messaging.
pub struct SendMessageTool {
    /// For `reply_timeout_secs`.
    pub cfg: Arc<AgentConfig>,
}

#[async_trait::async_trait]
impl Tool for SendMessageTool {
    const NAME: &'static str = "send_message";
    const RISK: Risk = Risk::ReadOnly;
    type Args = SendMessageArgs;

    fn description(&self) -> String {
        "Send a message to a peer: `parent` if you are a subagent, or one of your subagents by name. \
         The text is injected into their conversation. With await_reply=true you block until they message you back \
         (or the timeout passes). When answering a <message> that has reply_expected=\"true\", set reply_to to its id."
            .into()
    }

    async fn call(&self, args: SendMessageArgs, cx: ToolCx) -> anyhow::Result<ToolOutput> {
        let agents = match supervisor() {
            Ok(a) => a,
            Err(out) => return Ok(out),
        };
        let to = args.to;
        let rx = match agents
            .ask(Deliver {
                from: cx.session,
                to: to.clone(),
                text: args.message,
                await_reply: args.await_reply,
                reply_to: args.reply_to,
            })
            .await
        {
            Ok(rx) => rx,
            Err(SendError::HandlerError(e)) => return Ok(ToolOutput::err(e)),
            Err(e) => return Ok(ToolOutput::err(format!("agent supervisor: {e}"))),
        };
        let Some(rx) = rx else {
            return Ok(ToolOutput::ok("delivered"));
        };
        let secs = self.cfg.subagents.reply_timeout_secs;
        Ok(
            match tokio::time::timeout(Duration::from_secs(secs), rx).await {
                Ok(Ok(reply)) => ToolOutput::ok(reply),
                _ => ToolOutput::ok(format!("no reply from {to} within {secs}s")),
            },
        )
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn spawn_description_lists_configured_harnesses() {
        let dir = std::env::temp_dir().join(format!("wth-peers-{}", uuid::Uuid::now_v7()));
        std::fs::create_dir_all(&dir).unwrap();
        let plain = SpawnSubagentTool {
            data_dir: dir.clone(),
        }
        .description();
        assert!(!plain.contains("External harnesses"));

        let mut entries = std::collections::BTreeMap::new();
        entries.insert(
            "codex".to_string(),
            acp::Entry {
                command: "codex-acp".into(),
                args: Vec::new(),
                env: Default::default(),
                description: Some("ChatGPT's coding agent".into()),
            },
        );
        acp::save(&dir, &entries).unwrap();
        let with = SpawnSubagentTool {
            data_dir: dir.clone(),
        }
        .description();
        assert!(with.starts_with(&plain));
        assert!(with.contains("External harnesses: codex"));
        assert!(with.contains("ChatGPT's coding agent"));
        let _ = std::fs::remove_dir_all(&dir);
    }
}
