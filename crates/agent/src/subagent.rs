//! `spawn_subagent` tool: runs a task in a child session and returns its final
//! answer. Children are depth 1 and cannot spawn further subagents.

use std::time::Duration;

use kameo::actor::{ActorRef, Spawn};
use kameo::message::{Context, Message};
use runtime::{Runtime, SessionId};
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use tokio::sync::oneshot;
use tools::{Risk, Tool, ToolCx, ToolOutput};

use crate::supervisor::SpawnChild;
use crate::{AGENT_ACTOR, AgentEvent, Agents, SendInput};

/// How long a child may run before the parent gets an error output.
const CHILD_TIMEOUT: Duration = Duration::from_secs(600);

/// Arguments for [`SpawnSubagentTool`].
#[derive(Debug, Serialize, Deserialize, JsonSchema)]
pub struct SpawnSubagentArgs {
    /// Complete, self-contained instructions for the subagent.
    pub task: String,
}

/// The tool. Looks up the `"agent"` supervisor at call time.
pub struct SpawnSubagentTool {
    /// For bus subscription.
    pub rt: Runtime,
}

#[async_trait::async_trait]
impl Tool for SpawnSubagentTool {
    const NAME: &'static str = "spawn_subagent";
    const RISK: Risk = Risk::Mutating;
    type Args = SpawnSubagentArgs;

    fn description(&self) -> String {
        "Delegate a self-contained task to a subagent with its own session; returns its final answer.".into()
    }

    async fn call(&self, args: SpawnSubagentArgs, cx: ToolCx) -> anyhow::Result<ToolOutput> {
        let Some(agents) = ActorRef::<Agents>::lookup(AGENT_ACTOR).ok().flatten() else {
            return Ok(ToolOutput::err("agent supervisor not running"));
        };
        let child = match agents.ask(SpawnChild { parent: cx.session }).await {
            Ok(Some(id)) => id,
            Ok(None) => return Ok(ToolOutput::err("could not create child session")),
            Err(e) => return Ok(ToolOutput::err(format!("agent supervisor: {e}"))),
        };
        // Subscribe before the first input so no event is missed.
        let (tx, rx) = oneshot::channel();
        let collector = Collector::spawn_with_mailbox(
            Collector {
                session: child,
                text: String::new(),
                done: Some(tx),
            },
            runtime::subscriber_mailbox(),
        );
        self.rt.subscribe::<Collector, AgentEvent>(&collector).await;
        if let Err(e) = agents
            .ask(SendInput {
                session: Some(child),
                text: args.task,
            })
            .await
        {
            collector.kill();
            return Ok(ToolOutput::err(format!("agent supervisor: {e}")));
        }
        let out = match tokio::time::timeout(CHILD_TIMEOUT, rx).await {
            Ok(Ok((text, stop_reason))) if stop_reason == "end_turn" || stop_reason == "stop" => {
                ToolOutput::ok(text)
            }
            Ok(Ok((text, stop_reason))) => {
                ToolOutput::err(format!("subagent stopped ({stop_reason}): {text}"))
            }
            Ok(Err(_)) => ToolOutput::err("subagent collector died"),
            Err(_) => {
                let _ = agents.tell(crate::Cancel { session: child }).await;
                ToolOutput::err("subagent timed out after 10 minutes")
            }
        };
        collector.kill();
        Ok(out)
    }
}

/// One-shot bus subscriber: gathers the child's text until its turn ends.
#[derive(kameo::Actor)]
struct Collector {
    session: SessionId,
    text: String,
    done: Option<oneshot::Sender<(String, String)>>,
}

impl Message<AgentEvent> for Collector {
    type Reply = ();

    async fn handle(&mut self, ev: AgentEvent, ctx: &mut Context<Self, Self::Reply>) {
        if ev.session() != self.session {
            return;
        }
        match ev {
            AgentEvent::TextDelta { text, .. } => self.text.push_str(&text),
            AgentEvent::Error { message, .. } => {
                self.text.push_str("\n[error] ");
                self.text.push_str(&message);
            }
            AgentEvent::TurnFinished { stop_reason, .. } => {
                if let Some(tx) = self.done.take() {
                    let _ = tx.send((std::mem::take(&mut self.text), stop_reason));
                }
                ctx.stop();
            }
            _ => {}
        }
    }
}
