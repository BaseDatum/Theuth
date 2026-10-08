//! Feed 1 — live progress rows (141 §6.4): every `tool_call` /
//! `tool_call_update` of omp's `task` call carries the whole roster in
//! `rawOutput.details.progress[]`, also *after* the call reported `completed`
//! (F18: the `completed` frame is a spawn acknowledgement with stale `pending`
//! rows; 27 live snapshots follow on the same `toolCallId`). Frames arrive in
//! wire order and each is a full snapshot, so applying them in order makes the
//! latest win per id; the call's own `status` is never read as a child state.

use agent_client_protocol::schema::v1::SessionUpdate;
use serde::{Deserialize, Serialize};
use serde_json::{Map, Value};

use crate::harness::{ChildAnnounce, ChildSource, ChildState, CostRule};

/// One `task` frame's worth of subagent rows (141 §6.4 feed 1): the
/// `tool_call_update` (or `tool_call`) whose `rawOutput.details.progress[]`
/// is an array of rows. Each frame is a full snapshot; the latest wins per id.
#[derive(Clone, Debug, PartialEq)]
pub struct TaskFrame {
    /// `toolCallId` of the frame — the spawning tool call (`observed_tool_call`).
    pub tool_call_id: String,
    /// `details.async` present: background spawn, excluded from the parent's
    /// `usage_update` → `CostRule::Counted` (F23). Absent: sync → `InsideParent`.
    pub background: bool,
    /// `details.async.state` (`running` / `completed`), the roster footer.
    pub batch_state: Option<String>,
    /// The rows, in `index` order as sent.
    pub rows: Vec<Progress>,
}

/// A recent tool of a subagent (`progress[].recentTools[]`, F19; 18.8.4 adds
/// `intent` and `isError`, Q9).
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct RecentTool {
    /// Tool name.
    pub tool: String,
    /// Rendered arguments.
    #[serde(default)]
    pub args: String,
    /// Unix ms when it ended; `None` while running.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub end_ms: Option<u64>,
    /// The intent the model attached to the call (18.8.4).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub intent: Option<String>,
    /// The call failed (18.8.4).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub is_error: Option<bool>,
}

/// Retry bookkeeping of a subagent (`retryState` / `retryFailure`, F19).
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct RetryState {
    /// Attempt number.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub attempt: Option<u32>,
    /// Attempts allowed.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub max_attempts: Option<u32>,
    /// Backoff before the next attempt.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub delay_ms: Option<u64>,
    /// The error that caused the retry / the final failure.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub error_message: Option<String>,
}

/// One omp `AgentProgress` row (F19; capture `omp-18.3.1-task.jsonl` line 27).
/// `id` is the child's external id (= job id = transcript stem; nested
/// `a.b`); `status` is `pending | running | completed | failed | aborted`.
/// Every other field is optional: omp fills them in as the child runs.
#[derive(Clone, Debug, Default, PartialEq, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Progress {
    /// Position in the batch.
    #[serde(default)]
    pub index: Option<u32>,
    /// External id / job id / transcript stem.
    pub id: String,
    /// `pending | running | completed | failed | aborted`.
    pub status: String,
    /// Agent type (`scout`, `task`, …).
    #[serde(default)]
    pub agent: Option<String>,
    /// `bundled | project | user`.
    #[serde(default)]
    pub agent_source: Option<String>,
    /// Model role the agent was configured with.
    #[serde(default)]
    pub model_role: Option<String>,
    /// The task text (prompt).
    #[serde(default)]
    pub task: Option<String>,
    /// The rendered assignment (preferred for `observed_description`).
    #[serde(default)]
    pub assignment: Option<String>,
    /// Recent tools, newest first.
    #[serde(default)]
    pub recent_tools: Vec<RecentTool>,
    /// Recent output lines.
    #[serde(default)]
    pub recent_output: Vec<String>,
    /// Tools called so far.
    #[serde(default)]
    pub tool_count: Option<u32>,
    /// Model requests so far.
    #[serde(default)]
    pub requests: Option<u32>,
    /// input + output + cacheWrite tokens (billing volume).
    #[serde(default)]
    pub tokens: Option<u64>,
    /// USD so far.
    #[serde(default)]
    pub cost: Option<f64>,
    /// Wall time so far.
    #[serde(default)]
    pub duration_ms: Option<u64>,
    /// `provider/model:thinking` the child resolved to → `harness_model`.
    #[serde(default)]
    pub resolved_model: Option<String>,
    /// `provider/model` without the thinking suffix.
    #[serde(default)]
    pub resolved_model_identity: Option<String>,
    /// Thinking level in force.
    #[serde(default)]
    pub resolved_thinking_level: Option<String>,
    /// Latest context size.
    #[serde(default)]
    pub context_tokens: Option<u64>,
    /// Context window.
    #[serde(default)]
    pub context_window: Option<u64>,
    /// What the child says it is doing.
    #[serde(default)]
    pub last_intent: Option<String>,
    /// The tool running now.
    #[serde(default)]
    pub current_tool: Option<String>,
    /// Its rendered arguments.
    #[serde(default)]
    pub current_tool_args: Option<String>,
    /// The intent attached to the running call (18.8.4, Q9).
    #[serde(default)]
    pub current_tool_intent: Option<String>,
    /// Completion estimate 0–100 while running (18.8.4, Q9).
    #[serde(default)]
    pub completion_percent: Option<f64>,
    /// `extractedToolData.yield[0].data` and friends, kept raw → `observed_detail.result`.
    #[serde(default)]
    pub extracted_tool_data: Option<Value>,
    /// Retry in progress.
    #[serde(default)]
    pub retry_state: Option<RetryState>,
    /// Retries exhausted.
    #[serde(default)]
    pub retry_failure: Option<RetryState>,
}

/// Roster rows kept in the detail (141 §6.4: `recentTools[≤5]`, `recentOutput[≤8]`).
const RECENT_TOOLS: usize = 5;
const RECENT_OUTPUT: usize = 8;
/// `observed_description` head.
const DESCRIPTION_CHARS: usize = 200;

impl Progress {
    /// The announce for this row (141 §6.4 feed 1 table): identity from `id`
    /// (nested `a.b` → parent `a`), the frame's tool call and cost rule, the
    /// state from `status`, `resolvedModel` as the model, `assignment` (else
    /// `task`) head as the description; live numbers go to `detail`, only
    /// the keys omp sent (the store merges shallowly, so absent = unchanged).
    #[must_use]
    pub fn announce(&self, frame: &TaskFrame) -> ChildAnnounce {
        let mut a = ChildAnnounce::new(ChildSource::OmpTask, self.id.clone());
        a.parent = self.id.rsplit_once('.').map(|(p, _)| p.to_owned());
        a.title = Some(self.id.clone());
        a.tool_call = Some(frame.tool_call_id.clone());
        a.model = self.resolved_model.clone();
        a.description = self
            .assignment
            .as_deref()
            .or(self.task.as_deref())
            .map(|s| head(s, DESCRIPTION_CHARS).to_owned());
        a.cost_rule = Some(if frame.background {
            CostRule::Counted
        } else {
            CostRule::InsideParent
        });
        let stop = self
            .retry_failure
            .as_ref()
            .and_then(|r| r.error_message.clone());
        a.state = Some(match self.status.as_str() {
            "pending" => ChildState::Pending,
            "running" => ChildState::Running,
            "completed" => ChildState::Completed,
            "failed" => ChildState::Failed { error: stop },
            "aborted" => ChildState::Cancelled,
            _ => ChildState::Unknown,
        });
        a.detail = self.detail(frame);
        a
    }

    /// The §6.4 detail object: every live field omp sent, by its wire name.
    /// A `pending` row's counters are omitted (141 R3 T10): omp sends zeros
    /// before the child ran, and a zero is not a measurement — the UI shows
    /// `—`, and the tail fills real numbers in when feed 1 falls silent.
    fn detail(&self, frame: &TaskFrame) -> Map<String, Value> {
        let mut d = Map::new();
        let mut put = |k: &str, v: Option<Value>| {
            if let Some(v) = v {
                d.insert(k.to_owned(), v);
            }
        };
        put("index", self.index.map(Value::from));
        put("agent", self.agent.clone().map(Value::from));
        put("agentSource", self.agent_source.clone().map(Value::from));
        put("modelRole", self.model_role.clone().map(Value::from));
        if self.status != "pending" {
            put("tokens", self.tokens.map(Value::from));
            put("cost", self.cost.map(Value::from));
            put("contextTokens", self.context_tokens.map(Value::from));
            put("contextWindow", self.context_window.map(Value::from));
            put("requests", self.requests.map(Value::from));
            put("toolCount", self.tool_count.map(Value::from));
            put("durationMs", self.duration_ms.map(Value::from));
        }
        put("lastIntent", self.last_intent.clone().map(Value::from));
        put("currentTool", self.current_tool.clone().map(Value::from));
        put(
            "currentToolArgs",
            self.current_tool_args.clone().map(Value::from),
        );
        put(
            "currentToolIntent",
            self.current_tool_intent.clone().map(Value::from),
        );
        put(
            "completionPercent",
            self.completion_percent.map(Value::from),
        );
        put(
            "recentTools",
            Some(
                serde_json::to_value(
                    &self.recent_tools[..self.recent_tools.len().min(RECENT_TOOLS)],
                )
                .unwrap_or_default(),
            ),
        );
        put(
            "recentOutput",
            Some(Value::from(
                &self.recent_output[..self.recent_output.len().min(RECENT_OUTPUT)],
            )),
        );
        put(
            "retry",
            self.retry_failure
                .as_ref()
                .or(self.retry_state.as_ref())
                .and_then(|r| serde_json::to_value(r).ok()),
        );
        put(
            "result",
            self.extracted_tool_data
                .as_ref()
                .and_then(|x| x.get("yield")?.get(0)?.get("data").cloned()),
        );
        put("batch", frame.batch_state.clone().map(Value::from));
        d
    }
}

/// The first `n` chars of `s`, on a char boundary.
fn head(s: &str, n: usize) -> &str {
    s.char_indices().nth(n).map_or(s, |(i, _)| &s[..i])
}

/// The task frame in `update`, if it is one: any `tool_call` /
/// `tool_call_update` whose `rawOutput.details.progress` is an array of
/// objects with string `id` and `status` — regardless of the call's own
/// `status` (omp keeps updating a `completed` call, F18). `None` otherwise.
#[must_use]
pub fn progress_rows(update: &SessionUpdate) -> Option<TaskFrame> {
    let (id, raw) = match update {
        SessionUpdate::ToolCall(tc) => (&tc.tool_call_id, tc.raw_output.as_ref()?),
        SessionUpdate::ToolCallUpdate(u) => (&u.tool_call_id, u.fields.raw_output.as_ref()?),
        _ => return None,
    };
    let details = raw.get("details")?;
    let rows: Vec<Progress> = serde_json::from_value(details.get("progress")?.clone()).ok()?;
    let batch = details.get("async").filter(|a| a.is_object());
    Some(TaskFrame {
        tool_call_id: id.0.to_string(),
        background: batch.is_some(),
        batch_state: batch
            .and_then(|a| a.get("state")?.as_str())
            .map(str::to_owned),
        rows,
    })
}

#[cfg(test)]
mod tests {
    use std::collections::HashMap;

    use serde_json::json;

    use super::*;

    /// A recorded frame (`crates/acp/fixtures/omp-18.3.1`, capture
    /// `omp-18.3.1-task.jsonl` 2026-10-08) as the SDK parses it.
    fn frame(name: &str) -> SessionUpdate {
        let path = format!("{}/fixtures/omp-18.3.1/{name}", env!("CARGO_MANIFEST_DIR"));
        let v: Value = serde_json::from_str(&std::fs::read_to_string(&path).expect(&path)).unwrap();
        serde_json::from_value(v["params"]["update"].clone()).expect(name)
    }

    fn rows(name: &str) -> TaskFrame {
        progress_rows(&frame(name)).expect(name)
    }

    /// 141 §6.4 / F18: the recogniser is structural — the spawn `tool_call`
    /// (no `rawOutput`) is not a task frame; the `completed` acknowledgement
    /// is one with stale `pending` rows (its call status is not a child
    /// state); the `in_progress` frames after it are the live ones.
    #[test]
    fn task_frames_are_recognised_regardless_of_call_status() {
        assert!(progress_rows(&frame("task.tool_call.json")).is_none());

        let stale = rows("task.completed_stale.json");
        assert_eq!(stale.tool_call_id, "toolu_013x6rgBK7MEUsQq79DoL4YK");
        assert!(stale.background);
        assert_eq!(stale.batch_state.as_deref(), Some("running"));
        let ids: Vec<&str> = stale.rows.iter().map(|r| r.id.as_str()).collect();
        assert_eq!(ids, ["ReadA", "ReadB"]);
        for r in &stale.rows {
            let a = r.announce(&stale);
            assert_eq!(a.state, Some(ChildState::Pending));
            assert_eq!(a.cost_rule, Some(CostRule::Counted));
            assert_eq!(a.tool_call.as_deref(), Some(stale.tool_call_id.as_str()));
            assert_eq!(a.model, None);
            // 141 R3 T10: nothing has been measured yet — not zeros.
            assert!(a.detail.get("tokens").is_none(), "{:?}", a.detail);
            assert!(a.detail.get("toolCount").is_none());
            assert_eq!(a.detail["index"], r.index.unwrap());
        }

        let mid = rows("task.mid.json");
        let a = mid.rows[0].announce(&mid);
        assert_eq!(a.state, Some(ChildState::Running));
        assert_eq!(a.model.as_deref(), Some("anthropic/claude-fable-5-1:high"));
        assert_eq!(a.title.as_deref(), Some("ReadA"));
        assert!(a.description.as_deref().unwrap().starts_with("# Target"));
        assert_eq!(a.detail["lastIntent"], "Reading first line");
        assert_eq!(a.detail["recentTools"][0]["tool"], "read");
        assert_eq!(a.detail["agent"], "scout");
        assert_eq!(a.detail["batch"], "running");
        assert!(a.detail.get("result").is_none());
        assert!(a.detail.get("completionPercent").is_none());
    }

    /// 141 §6.4: frames are full snapshots; applied in wire order the latest
    /// wins per id, so the final frame's `completed` rows (cost
    /// 0.12128025 / 0.12170525, F23) replace the stale and mid ones.
    #[test]
    fn latest_snapshot_wins_per_id() {
        let mut seen: HashMap<String, ChildAnnounce> = HashMap::new();
        for name in [
            "task.completed_stale.json",
            "task.mid.json",
            "task.final.json",
        ] {
            let f = rows(name);
            for r in &f.rows {
                seen.insert(r.id.clone(), r.announce(&f));
            }
        }
        assert_eq!(seen.len(), 2);
        let a = &seen["ReadA"];
        assert_eq!(a.state, Some(ChildState::Completed));
        assert_eq!(a.detail["cost"], 0.12128025);
        assert_eq!(a.detail["tokens"], 9118);
        assert_eq!(a.detail["toolCount"], 2);
        assert_eq!(a.detail["durationMs"], 4819);
        assert_eq!(a.detail["recentTools"].as_array().unwrap().len(), 2);
        assert_eq!(a.detail["recentTools"][0]["tool"], "yield");
        assert_eq!(a.detail["result"], json!({"first_line": "alpha line one"}));
        assert_eq!(a.detail["batch"], "completed");
        let b = &seen["ReadB"];
        assert_eq!(b.state, Some(ChildState::Completed));
        assert_eq!(b.detail["cost"], 0.12170525);
        assert_eq!(b.detail["result"], json!({"first_line": "bravo line one"}));
    }

    /// 141 §6.4: nested ids `a.b` name their parent `a`; top-level rows have
    /// none (= the driven session).
    #[test]
    fn nested_ids_name_their_parent() {
        let f = rows("task.mid.json");
        assert_eq!(f.rows[0].announce(&f).parent, None);
        let nested = Progress {
            id: "ReadA.sub".into(),
            status: "running".into(),
            ..Progress::default()
        };
        let a = nested.announce(&f);
        assert_eq!(a.parent.as_deref(), Some("ReadA"));
        assert_eq!(a.child, "ReadA.sub");
        assert_eq!(a.title.as_deref(), Some("ReadA.sub"));
    }

    /// 141 §6.4 / F23: `details.async` present = background spawn, counted;
    /// absent (sync, `details.usage` folded into the parent) = inside parent.
    /// Unknown statuses and a `retryFailure` map to `Unknown` / `Failed{error}`.
    #[test]
    fn sync_spawns_are_inside_parent_and_states_map() {
        let mut v: Value = serde_json::to_value(frame("task.mid.json")).unwrap();
        let details = &mut v["rawOutput"]["details"];
        details.as_object_mut().unwrap().remove("async");
        details["usage"] = json!({"input": 1, "output": 2});
        details["progress"][0]["status"] = json!("failed");
        details["progress"][0]["retryFailure"] =
            json!({"attempt": 3, "maxAttempts": 3, "errorMessage": "rate limited"});
        details["progress"][1]["status"] = json!("aborted");
        let f = progress_rows(&serde_json::from_value(v).unwrap()).unwrap();
        assert!(!f.background);
        assert_eq!(f.batch_state, None);
        let a = f.rows[0].announce(&f);
        assert_eq!(a.cost_rule, Some(CostRule::InsideParent));
        assert_eq!(
            a.state,
            Some(ChildState::Failed {
                error: Some("rate limited".into())
            })
        );
        assert_eq!(a.detail["retry"]["errorMessage"], "rate limited");
        assert!(a.detail.get("batch").is_none());
        assert_eq!(f.rows[1].announce(&f).state, Some(ChildState::Cancelled));
    }

    /// Revision 1 Q9: the 18.8.4 fields (`completionPercent`,
    /// `currentToolIntent`, `recentTools[].isError`) surface in the detail
    /// when present; the roster caps stay (≤5 tools, ≤8 output lines).
    #[test]
    fn newer_fields_surface_when_present_and_rosters_are_capped() {
        let mut v: Value = serde_json::to_value(frame("task.mid.json")).unwrap();
        let row = &mut v["rawOutput"]["details"]["progress"][0];
        row["completionPercent"] = json!(42);
        row["currentTool"] = json!("bash");
        row["currentToolIntent"] = json!("Listing files");
        row["recentTools"] = json!(
            (0..7)
                .map(|i| json!({"tool": format!("t{i}"), "args": "", "endMs": i}))
                .collect::<Vec<_>>()
        );
        row["recentTools"][0]["isError"] = json!(true);
        row["recentOutput"] = json!((0..10).map(|i| i.to_string()).collect::<Vec<_>>());
        let f = progress_rows(&serde_json::from_value(v).unwrap()).unwrap();
        let d = f.rows[0].announce(&f).detail;
        assert_eq!(d["completionPercent"], 42.0);
        assert_eq!(d["currentTool"], "bash");
        assert_eq!(d["currentToolIntent"], "Listing files");
        assert_eq!(d["recentTools"].as_array().unwrap().len(), RECENT_TOOLS);
        assert_eq!(d["recentTools"][0]["isError"], true);
        assert!(d["recentTools"][1].get("isError").is_none());
        assert_eq!(d["recentOutput"].as_array().unwrap().len(), RECENT_OUTPUT);
    }

    /// Frames without a well-formed `details.progress[]` are not task frames.
    #[test]
    fn other_frames_are_not_task_frames() {
        let mut v: Value = serde_json::to_value(frame("task.mid.json")).unwrap();
        v["rawOutput"]["details"]["progress"] = json!([{"id": "x"}]);
        assert!(progress_rows(&serde_json::from_value(v.clone()).unwrap()).is_none());
        v["rawOutput"]["details"]["progress"] = json!("nope");
        assert!(progress_rows(&serde_json::from_value(v.clone()).unwrap()).is_none());
        v["rawOutput"] = json!({"output": "plain"});
        assert!(progress_rows(&serde_json::from_value(v).unwrap()).is_none());
        let usage: SessionUpdate =
            serde_json::from_value(json!({"sessionUpdate": "usage_update", "size": 1, "used": 1}))
                .unwrap();
        assert!(progress_rows(&usage).is_none());
    }
}
