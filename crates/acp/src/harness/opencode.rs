//! opencode's subagent feeds (141 §6.2, §6.3, D5, D6).
//!
//! 2.x forwards each child session's traffic on the extension notification
//! [`CHILD_UPDATE_METHOD`] (opt-in via `_meta["opencode/child-session-updates"]`):
//! status frames become announces, update frames become the child's own
//! traffic — no flattening into the parent, no chunk→thought rewrite. The
//! parent's `subagent` tool call is joined to the child it spawned through
//! its completed `rawOutput.metadata.sessionID` ([`Children::spawn_join`]).
//!
//! 1.x sends nothing child-originated; only the `task` tool lifecycle on the
//! parent reaches ACP ([`task_v1`]). Those frames are the parent's own
//! traffic too, so the transport does not split them: `acp_child` calls
//! [`task_v1::announce`] on an opencode < 2 harness's frames.

use std::collections::{HashMap, HashSet};

use agent_client_protocol::UntypedMessage;
use agent_client_protocol::schema::v1::{SessionUpdate, ToolCallStatus};

use super::{ChildAnnounce, ChildSource, ChildState, CostRule, Update};

/// opencode ≥ 2's extension notification carrying a sub-agent's updates
/// (06 §8.4), sent only to a client that advertised
/// `_meta["opencode/child-session-updates"]`.
pub const CHILD_UPDATE_METHOD: &str = "opencode/session/child_update";

/// `observed_description` head length (141 §6.3).
const DESCRIPTION_HEAD: usize = 200;

/// Per-connection memory for the 2.x feed: which children were announced
/// (so a `type:"update"` for a never-announced child announces it first,
/// and a spawn join never creates a phantom row) and the `subagent` tool
/// calls whose `rawInput` named an agent/description before the join.
#[derive(Debug, Default)]
pub struct Children {
    announced: HashSet<String>,
    spawns: HashMap<String, Spawn>,
}

#[derive(Debug, Default)]
struct Spawn {
    agent: Option<String>,
    description: Option<String>,
}

impl Children {
    /// The [`Update`]s for an extension notification: nothing for another
    /// method, an unknown envelope or an unknown `status`; one
    /// [`Update::Subagent`] for a status frame; for an update frame the
    /// child's traffic as [`Update::Session`] keyed by `childSessionId`,
    /// preceded by an implicit `running` announce when the child was never
    /// announced on this connection (opencode sends `created` first, but a
    /// `session/load` replay may start mid-stream).
    #[must_use]
    pub fn child_update(&mut self, n: &UntypedMessage) -> Vec<Update> {
        if n.method != CHILD_UPDATE_METHOD {
            return Vec::new();
        }
        let p = &n.params;
        let s = |k: &str| p.get(k).and_then(serde_json::Value::as_str);
        let (Some(root), Some(child), Some(kind)) =
            (s("rootSessionId"), s("childSessionId"), s("type"))
        else {
            return Vec::new();
        };
        let parent = s("parentSessionId")
            .filter(|&x| x != root)
            .map(str::to_owned);
        let title = s("title").map(str::to_owned);
        let first = self.announced.insert(child.to_owned());
        let announce = |state: ChildState| {
            let mut a = ChildAnnounce::new(ChildSource::OpencodeExt, child);
            a.parent = parent.clone();
            a.title = title.clone();
            a.state = Some(state);
            if first {
                a.cost_rule = Some(CostRule::Unreported);
            }
            a
        };
        match kind {
            "status" => {
                let a = match s("status") {
                    Some("created") => announce(ChildState::Pending),
                    Some("running") => announce(ChildState::Running),
                    Some("completed") => announce(ChildState::Completed),
                    Some("interrupted") => announce(ChildState::Cancelled),
                    Some("failed") => {
                        let error = p.get("error");
                        let message = error
                            .and_then(|e| e.get("message"))
                            .and_then(serde_json::Value::as_str)
                            .map(str::to_owned);
                        let mut a = announce(ChildState::Failed {
                            error: message.clone(),
                        });
                        if let Some(e) = error {
                            a.detail.insert("error".into(), e.clone());
                        }
                        a
                    }
                    other => {
                        tracing::debug!(child, ?other, "opencode child_update: unknown status");
                        return Vec::new();
                    }
                };
                vec![Update::Subagent(a)]
            }
            "update" => {
                let mut out = Vec::with_capacity(2);
                if first {
                    out.push(Update::Subagent(announce(ChildState::Running)));
                }
                match p
                    .get("update")
                    .cloned()
                    .map(serde_json::from_value::<SessionUpdate>)
                {
                    Some(Ok(update)) => out.push(Update::Session {
                        session: child.to_owned(),
                        update,
                    }),
                    Some(Err(e)) => {
                        tracing::debug!(child, "opencode child_update: unparsed update: {e}");
                    }
                    None => {}
                }
                out
            }
            _ => Vec::new(),
        }
    }

    /// 2.x spawn join (141 §6.2, F14) for the **parent's** own
    /// `tool_call_update`s: the `in_progress` frame of a `subagent` call
    /// carries `rawInput.{agent, description}` (remembered by call id); the
    /// `completed` frame carries `rawOutput.metadata.sessionID` — for an
    /// announced child that yields an announce binding `tool_call`,
    /// `description` and `detail.agent` to it. `None` for any other frame.
    #[must_use]
    pub fn spawn_join(&mut self, update: &SessionUpdate) -> Option<ChildAnnounce> {
        let SessionUpdate::ToolCallUpdate(u) = update else {
            return None;
        };
        let id = u.tool_call_id.0.to_string();
        if let Some(input) = u
            .fields
            .raw_input
            .as_ref()
            .filter(|_| u.fields.title.as_deref() == Some("subagent"))
            && let Some(description) = input.get("description").and_then(serde_json::Value::as_str)
        {
            self.spawns.insert(
                id,
                Spawn {
                    agent: input
                        .get("agent")
                        .and_then(serde_json::Value::as_str)
                        .map(str::to_owned),
                    description: Some(description.to_owned()),
                },
            );
            return None;
        }
        let child = u
            .fields
            .raw_output
            .as_ref()?
            .get("metadata")?
            .get("sessionID")?
            .as_str()?;
        if !self.announced.contains(child) {
            return None;
        }
        let spawn = self.spawns.remove(&id).unwrap_or_default();
        let mut a = ChildAnnounce::new(ChildSource::OpencodeExt, child);
        a.tool_call = Some(id);
        a.description = spawn.description;
        if let Some(agent) = spawn.agent {
            a.detail.insert("agent".into(), agent.into());
        }
        Some(a)
    }
}

/// opencode 1.x: the `task` tool-call lifecycle on the parent (141 §6.3,
/// F16). The child's external id is `task:<toolCallId>` until the completed
/// frame's `rawOutput.metadata.sessionId` supplies the real one: that
/// announce carries `child = <real id>` with the same `tool_call`, and the
/// ingest side re-keys the provisional row (141 §6.3; `acp_child/observed.rs`).
pub mod task_v1 {
    use super::*;

    /// A frame of the 1.x `task` tool: the `tool_call` titled `task`, an
    /// update whose `rawInput.subagent_type` is set, or one whose
    /// `rawOutput.metadata` names `sessionId` and `parentSessionId`.
    #[must_use]
    pub fn is_task_call(update: &SessionUpdate) -> bool {
        match update {
            SessionUpdate::ToolCall(c) => c.title == "task",
            SessionUpdate::ToolCallUpdate(u) => {
                u.fields
                    .raw_input
                    .as_ref()
                    .is_some_and(|i| i.get("subagent_type").is_some())
                    || metadata(update).is_some_and(|m| {
                        m.get("sessionId").is_some() && m.get("parentSessionId").is_some()
                    })
            }
            _ => false,
        }
    }

    fn metadata(update: &SessionUpdate) -> Option<&serde_json::Value> {
        match update {
            SessionUpdate::ToolCallUpdate(u) => u.fields.raw_output.as_ref()?.get("metadata"),
            _ => None,
        }
    }

    /// The announce for a task frame ([`is_task_call`]), else `None`:
    /// `tool_call` → `pending`; `in_progress` → `running` with the
    /// description as title, `rawInput.prompt` head as description and
    /// `subagent_type` as `detail.agent`; the terminal frame → `completed`
    /// or `failed` (tool `status: failed`, or `<task … state="error">` with
    /// the `<task_error>` text), `model = "<providerID>/<modelID>"`,
    /// `child` rewritten to `metadata.sessionId`, `detail.{parentSessionId,
    /// result}` where `result` is the `<task_result>` text (the child's only
    /// transcript content).
    #[must_use]
    pub fn announce(update: &SessionUpdate) -> Option<ChildAnnounce> {
        if !is_task_call(update) {
            return None;
        }
        let (id, status, title, input, output) = match update {
            SessionUpdate::ToolCall(c) => (
                c.tool_call_id.0.to_string(),
                Some(c.status),
                Some(c.title.as_str()),
                c.raw_input.as_ref(),
                c.raw_output.as_ref(),
            ),
            SessionUpdate::ToolCallUpdate(u) => (
                u.tool_call_id.0.to_string(),
                u.fields.status,
                u.fields.title.as_deref(),
                u.fields.raw_input.as_ref(),
                u.fields.raw_output.as_ref(),
            ),
            _ => return None,
        };
        let mut a = ChildAnnounce::new(ChildSource::OpencodeTask, format!("task:{id}"));
        a.tool_call = Some(id);
        a.cost_rule = Some(CostRule::Unreported);
        let str_of = |v: Option<&serde_json::Value>, k: &str| {
            v.and_then(|v| v.get(k))
                .and_then(serde_json::Value::as_str)
                .map(str::to_owned)
        };
        if let Some(description) = str_of(input, "description") {
            a.title = Some(description);
        } else if title == Some("task") {
            a.title = Some("task".to_owned());
        }
        if let Some(prompt) = str_of(input, "prompt") {
            a.description = Some(head(&prompt));
        }
        if let Some(agent) = str_of(input, "subagent_type") {
            a.detail.insert("agent".into(), agent.into());
        }
        let meta = output.and_then(|o| o.get("metadata"));
        if let Some(sid) = str_of(meta, "sessionId") {
            a.child = sid;
        }
        if let Some(pid) = str_of(meta, "parentSessionId") {
            a.detail.insert("parentSessionId".into(), pid.into());
        }
        if let Some(model) = meta.and_then(|m| m.get("model"))
            && let (Some(provider), Some(model)) = (
                model.get("providerID").and_then(serde_json::Value::as_str),
                model.get("modelID").and_then(serde_json::Value::as_str),
            )
        {
            a.model = Some(format!("{provider}/{model}"));
        }
        let text = str_of(output, "output");
        if let Some(result) = text
            .as_deref()
            .and_then(|t| between(t, "<task_result>", "</task_result>"))
        {
            a.detail.insert("result".into(), result.into());
        }
        a.state = Some(match status {
            Some(ToolCallStatus::Pending) => ChildState::Pending,
            Some(ToolCallStatus::InProgress) => ChildState::Running,
            Some(ToolCallStatus::Failed) => ChildState::Failed {
                error: text
                    .as_deref()
                    .and_then(|t| between(t, "<task_error>", "</task_error>"))
                    .map(str::to_owned),
            },
            Some(ToolCallStatus::Completed)
                if text
                    .as_deref()
                    .is_some_and(|t| t.contains("state=\"error\"")) =>
            {
                ChildState::Failed {
                    error: text
                        .as_deref()
                        .and_then(|t| between(t, "<task_error>", "</task_error>"))
                        .map(str::to_owned),
                }
            }
            Some(ToolCallStatus::Completed) => ChildState::Completed,
            _ => return Some(a),
        });
        Some(a)
    }

    fn between<'a>(text: &'a str, open: &str, close: &str) -> Option<&'a str> {
        let start = text.find(open)? + open.len();
        let end = text[start..].find(close)? + start;
        Some(text[start..end].trim())
    }

    fn head(s: &str) -> String {
        match s.char_indices().nth(DESCRIPTION_HEAD) {
            Some((i, _)) => format!("{}…", &s[..i]),
            None => s.to_owned(),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A recorded frame (crates/acp/fixtures/opencode-<version>).
    fn fixture(version: &str, name: &str) -> serde_json::Value {
        let path = format!(
            "{}/fixtures/opencode-{version}/{name}",
            env!("CARGO_MANIFEST_DIR")
        );
        serde_json::from_str(&std::fs::read_to_string(&path).expect(&path)).expect(&path)
    }

    fn untyped(version: &str, name: &str) -> UntypedMessage {
        serde_json::from_value(fixture(version, name)).unwrap()
    }

    fn session_update(version: &str, name: &str) -> SessionUpdate {
        serde_json::from_value(fixture(version, name)["params"]["update"].clone()).unwrap()
    }
    const CHILD: &str = "ses_ee4d9a752ffe2XT5Oz2Fldml2j";

    fn announce_of(u: &Update) -> &ChildAnnounce {
        match u {
            Update::Subagent(a) => a,
            other => panic!("announce expected, got {other:?}"),
        }
    }

    /// 141 §6.2 (capture `opencode-2.0.22-task.jsonl` lines 13, 15, 20, 22,
    /// 26, 27): status frames announce `pending → running → completed` on
    /// the child id, flat under the root; update frames are the child's own
    /// traffic — the tool call keeps its `<child>:<id>` id and `_meta`, the
    /// prose stays `agent_message_chunk` (no thought rewrite, D5); the cost
    /// rule is set once, on the first announce.
    #[test]
    fn child_update_fixtures_become_announces_and_child_traffic() {
        let mut ch = Children::default();
        let out = ch.child_update(&untyped("2.0.22", "child_update.status.created.json"));
        assert_eq!(out.len(), 1);
        let a = announce_of(&out[0]);
        assert_eq!(
            (a.source, a.child.as_str()),
            (ChildSource::OpencodeExt, CHILD)
        );
        assert_eq!(a.parent, None, "parentSessionId == rootSessionId → depth 1");
        assert_eq!(a.title.as_deref(), Some("Read a.txt first line"));
        assert_eq!(a.state, Some(ChildState::Pending));
        assert_eq!(a.cost_rule, Some(CostRule::Unreported));

        let out = ch.child_update(&untyped("2.0.22", "child_update.status.running.json"));
        let a = announce_of(&out[0]);
        assert_eq!(a.state, Some(ChildState::Running));
        assert_eq!(a.cost_rule, None, "unchanged after the first announce");

        let out = ch.child_update(&untyped("2.0.22", "child_update.tool_call.json"));
        assert_eq!(out.len(), 1, "an announced child: traffic only");
        let Update::Session {
            session,
            update: SessionUpdate::ToolCall(tc),
        } = &out[0]
        else {
            panic!("child tool_call expected: {out:?}");
        };
        assert_eq!(session, CHILD);
        assert_eq!(
            &*tc.tool_call_id.0,
            "ses_ee4d9a752ffe2XT5Oz2Fldml2j:toolu_014DGrCCVSsRX5h8vujs7SZh"
        );
        assert_eq!(tc.title, "Read a.txt first line: read");
        assert!(
            tc.meta
                .as_ref()
                .is_some_and(|m| m.contains_key("opencode/child-session"))
        );

        let out = ch.child_update(&untyped(
            "2.0.22",
            "child_update.tool_call_update.completed.json",
        ));
        let Update::Session {
            session,
            update: SessionUpdate::ToolCallUpdate(u),
        } = &out[0]
        else {
            panic!("child tool_call_update expected: {out:?}");
        };
        assert_eq!(session, CHILD);
        assert_eq!(u.fields.status, Some(ToolCallStatus::Completed));

        let out = ch.child_update(&untyped("2.0.22", "child_update.agent_message_chunk.json"));
        let Update::Session {
            session,
            update: SessionUpdate::AgentMessageChunk(c),
        } = &out[0]
        else {
            panic!("child agent_message_chunk expected: {out:?}");
        };
        assert_eq!(session, CHILD);
        assert_eq!(
            crate::render::content_block_text(&c.content),
            "alpha line one"
        );

        let out = ch.child_update(&untyped("2.0.22", "child_update.status.completed.json"));
        assert_eq!(announce_of(&out[0]).state, Some(ChildState::Completed));
    }

    /// A `type:"update"` for a never-announced child announces it `running`
    /// first (replay may start mid-stream); `failed` carries the error,
    /// `interrupted` is cancelled; a nested `parentSessionId` is the parent;
    /// unknown statuses and other methods are nothing.
    #[test]
    fn child_update_implicit_announce_failed_interrupted_and_nesting() {
        let mut ch = Children::default();
        let out = ch.child_update(&untyped("2.0.22", "child_update.agent_message_chunk.json"));
        assert_eq!(out.len(), 2);
        let a = announce_of(&out[0]);
        assert_eq!(
            (a.child.as_str(), a.state.clone()),
            (CHILD, Some(ChildState::Running))
        );
        assert_eq!(a.cost_rule, Some(CostRule::Unreported));
        assert!(matches!(&out[1], Update::Session { session, .. } if session == CHILD));

        let out = ch.child_update(&untyped("2.0.22", "child_update.status.failed.json"));
        let a = announce_of(&out[0]);
        assert_eq!(
            a.state,
            Some(ChildState::Failed {
                error: Some("rate limited".to_owned())
            })
        );
        assert_eq!(a.detail["error"]["type"], "ProviderError");
        let out = ch.child_update(&untyped("2.0.22", "child_update.status.interrupted.json"));
        assert_eq!(announce_of(&out[0]).state, Some(ChildState::Cancelled));

        let nested: UntypedMessage = serde_json::from_value(serde_json::json!({
            "jsonrpc": "2.0", "method": CHILD_UPDATE_METHOD,
            "params": {"rootSessionId": "r", "childSessionId": "gc", "parentSessionId": "c", "depth": 2,
                       "type": "status", "status": "running"}
        }))
        .unwrap();
        let out = ch.child_update(&nested);
        assert_eq!(announce_of(&out[0]).parent.as_deref(), Some("c"));

        let bogus: UntypedMessage = serde_json::from_value(serde_json::json!({
            "jsonrpc": "2.0", "method": CHILD_UPDATE_METHOD,
            "params": {"rootSessionId": "r", "childSessionId": "c", "parentSessionId": "r", "depth": 1,
                       "type": "status", "status": "paused"}
        }))
        .unwrap();
        assert!(ch.child_update(&bogus).is_empty());
        let other = UntypedMessage::new("session/update", serde_json::json!({})).unwrap();
        assert!(ch.child_update(&other).is_empty());
    }

    /// F14: the parent's `subagent` call names agent/description only on
    /// its `in_progress` frame and the child only on its `completed`
    /// `rawOutput.metadata.sessionID`; the join binds the call to an
    /// announced child and never invents one.
    #[test]
    fn spawn_join_binds_the_subagent_call_to_the_announced_child() {
        let mut ch = Children::default();
        let in_progress = session_update("2.0.22", "subagent.tool_call_update.in_progress.json");
        let completed = session_update("2.0.22", "subagent.tool_call_update.completed.json");
        assert!(ch.spawn_join(&in_progress).is_none());
        assert!(
            ch.spawn_join(&completed).is_none(),
            "child not announced: no phantom"
        );

        let _ = ch.child_update(&untyped("2.0.22", "child_update.status.created.json"));
        assert!(ch.spawn_join(&in_progress).is_none());
        let a = ch.spawn_join(&completed).unwrap();
        assert_eq!(
            (a.source, a.child.as_str()),
            (ChildSource::OpencodeExt, CHILD)
        );
        assert_eq!(
            a.tool_call.as_deref(),
            Some("toolu_018wLewXWWb6vBSf6pqKcnLn")
        );
        assert_eq!(a.description.as_deref(), Some("Read a.txt first line"));
        assert_eq!(a.detail["agent"], "general");
        assert_eq!(a.state, None);
        // A second completed frame (replay) still joins, without the forgotten rawInput.
        let again = ch.spawn_join(&completed).unwrap();
        assert_eq!(again.description, None);
        assert_eq!(
            again.tool_call.as_deref(),
            Some("toolu_018wLewXWWb6vBSf6pqKcnLn")
        );
    }

    /// 141 §6.3 (capture `opencode-1.18.30-task.jsonl` lines 15, 16, 21):
    /// one `opencode_task` child keyed `task:<toolCallId>` goes pending →
    /// running (named by the description, agent from `subagent_type`) →
    /// completed with the model, the real session id and the result text.
    #[test]
    fn task_v1_fixtures_announce_one_child_through_its_lifecycle() {
        let call = session_update("1.18.30", "task.tool_call.json");
        assert!(task_v1::is_task_call(&call));
        let a = task_v1::announce(&call).unwrap();
        assert_eq!(
            (a.source, a.child.as_str()),
            (
                ChildSource::OpencodeTask,
                "task:call_a062fdd9472f454ea6e45c9b"
            )
        );
        assert_eq!(a.state, Some(ChildState::Pending));
        assert_eq!(a.title.as_deref(), Some("task"));
        assert_eq!(
            a.tool_call.as_deref(),
            Some("call_a062fdd9472f454ea6e45c9b")
        );
        assert_eq!(a.cost_rule, Some(CostRule::Unreported));

        let running = session_update("1.18.30", "task.tool_call_update.in_progress.json");
        let a = task_v1::announce(&running).unwrap();
        assert_eq!(a.child, "task:call_a062fdd9472f454ea6e45c9b");
        assert_eq!(a.state, Some(ChildState::Running));
        assert_eq!(a.title.as_deref(), Some("Read first line of a.txt"));
        assert!(
            a.description
                .as_deref()
                .unwrap()
                .starts_with("Read the file /private/tmp/recon-141/oc-scratch/a.txt")
        );
        assert_eq!(a.detail["agent"], "explore");
        assert_eq!(a.model, None);

        let done = session_update("1.18.30", "task.tool_call_update.completed.json");
        let a = task_v1::announce(&done).unwrap();
        assert_eq!(a.state, Some(ChildState::Completed));
        assert_eq!(a.model.as_deref(), Some("opencode/big-pickle"));
        assert_eq!(a.child, "ses_ee4d89f0cffe7zXAvHsSIT0UsD");
        assert_eq!(
            a.tool_call.as_deref(),
            Some("call_a062fdd9472f454ea6e45c9b")
        );
        assert_eq!(
            a.detail["parentSessionId"],
            "ses_ee4d8bb5dffeS1akqW7B497imb"
        );
        assert_eq!(a.detail["result"], "alpha line one");

        // Not a task frame: an ordinary tool call, and omp's own `task` tool
        // (title = intent, rawInput {context, tasks}).
        let read: SessionUpdate = serde_json::from_value(serde_json::json!({
            "sessionUpdate": "tool_call", "toolCallId": "c1", "title": "Read a.txt", "kind": "read", "status": "pending"
        }))
        .unwrap();
        assert!(task_v1::announce(&read).is_none());
        let omp: SessionUpdate = serde_json::from_value(serde_json::json!({
            "sessionUpdate": "tool_call_update", "toolCallId": "t1", "status": "in_progress",
            "rawInput": {"context": "x", "tasks": [{"name": "A"}]}
        }))
        .unwrap();
        assert!(task_v1::announce(&omp).is_none());
    }

    /// A 1.x task that errors: tool `status: failed` or a completed call
    /// whose output says `state="error"` → failed with the `<task_error>` text.
    #[test]
    fn task_v1_error_states() {
        let failed: SessionUpdate = serde_json::from_value(serde_json::json!({
            "sessionUpdate": "tool_call_update", "toolCallId": "c9", "status": "completed",
            "rawOutput": {"output": "<task id=\"ses_x\" state=\"error\">\n<task_error>\nmodel refused\n</task_error>\n</task>",
                          "metadata": {"parentSessionId": "p", "sessionId": "ses_x"}}
        }))
        .unwrap();
        let a = task_v1::announce(&failed).unwrap();
        assert_eq!(
            a.state,
            Some(ChildState::Failed {
                error: Some("model refused".to_owned())
            })
        );
        let hard: SessionUpdate = serde_json::from_value(serde_json::json!({
            "sessionUpdate": "tool_call_update", "toolCallId": "c9", "status": "failed",
            "rawInput": {"description": "d", "prompt": "p", "subagent_type": "general"}
        }))
        .unwrap();
        let a = task_v1::announce(&hard).unwrap();
        assert_eq!(a.state, Some(ChildState::Failed { error: None }));
    }
}
