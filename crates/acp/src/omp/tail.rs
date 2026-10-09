//! Feed 2 — the on-disk tail (141 §6.4, F21/F22): omp drops every wire frame
//! after the parent's turn ends (F20), so a child's transcript and completion
//! are read from `~/.omp/agent/sessions/<encoded-cwd>/<ts>_<acpSessionId>/`
//! — `<id>.jsonl` (transcript, nested `<a>/<a>.<b>.jsonl`), `<id>.md`
//! (final output = done) — and from the parent transcript's `async-result`
//! entries. Reads are whole-line and cursor-based; a cursor past the end of a
//! file (truncation / rotation) resets to 0 (Revision 1 Q6).

use std::fs::{self, File};
use std::io::{self, Read, Seek, SeekFrom};
use std::path::{Path, PathBuf};

use agent_client_protocol::schema::v1::{
    ContentChunk, SessionUpdate, ToolCall, ToolCallContent, ToolCallId, ToolCallStatus,
    ToolCallUpdate, ToolCallUpdateFields, ToolKind,
};
use serde_json::Value;

/// Where omp keeps one ACP session's transcripts on disk (141 §6.4 feed 2,
/// F21): the parent file `~/.omp/agent/sessions/<encoded-cwd>/<ts>_<sid>.jsonl`
/// and, beside it, the artifacts dir `<same path minus .jsonl>/` holding
/// `<id>.jsonl` / `<id>.md` / `<id>.json` per child and `<a>/<a>.<b>.jsonl`
/// for nested ones.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Tailer {
    /// The engine's home (`~`); omp writes under `<home>/.omp`.
    pub home: PathBuf,
    /// The parent session's `.jsonl`.
    pub parent_file: PathBuf,
    /// The artifacts dir (parent file minus `.jsonl`).
    pub artifacts: PathBuf,
}

impl Tailer {
    /// Find the parent transcript for `acp_session` (the ACP session id ==
    /// omp session id) under `home`: glob `.omp/agent/sessions/*/*_<sid>.jsonl`.
    /// `None` until omp has written it (retried on the first task frame).
    #[must_use]
    pub fn locate(home: &Path, acp_session: &str) -> Option<Tailer> {
        let suffix = format!("_{acp_session}.jsonl");
        let sessions = home.join(".omp").join("agent").join("sessions");
        let parent_file = fs::read_dir(&sessions)
            .ok()?
            .flatten()
            .filter_map(|cwd| fs::read_dir(cwd.path()).ok())
            .flatten()
            .flatten()
            .map(|e| e.path())
            .find(|p| {
                p.file_name()
                    .and_then(|n| n.to_str())
                    .is_some_and(|n| n.ends_with(&suffix))
            })?;
        let stem = parent_file.file_name()?.to_str()?.strip_suffix(".jsonl")?;
        let artifacts = parent_file.with_file_name(stem);
        Some(Tailer {
            home: home.to_owned(),
            parent_file,
            artifacts,
        })
    }

    /// The child transcript for external id `id`: `<artifacts>/<id>.jsonl`;
    /// nested `a.b` → `<artifacts>/a/a.b.jsonl`.
    #[must_use]
    pub fn child_path(&self, id: &str) -> PathBuf {
        match id.rsplit_once('.') {
            Some((parent, _)) => self.artifacts.join(parent).join(format!("{id}.jsonl")),
            None => self.artifacts.join(format!("{id}.jsonl")),
        }
    }

    /// Whole lines of `id`'s transcript from byte `cursor`, parsed into
    /// [`Entry`]s, and the new cursor (a partial trailing line is left
    /// unread; a missing file reads as nothing).
    pub fn read(&self, id: &str, cursor: u64) -> io::Result<(Vec<Entry>, u64)> {
        let (lines, cursor) = lines_from(&self.child_path(id), cursor)?;
        Ok((lines.iter().map(Entry::parse).collect(), cursor))
    }

    /// The entries before byte `end` of `id`'s transcript — the part a
    /// previous engine life already replayed. 141 R4: a re-attached tail
    /// rebuilds its per-child sums (cost, tools, tokens) from it, then
    /// continues from `end` as usual. A missing file, or `end` past EOF
    /// (truncated / rotated: [`Self::read`] then starts over at 0), reads as nothing.
    pub fn read_until(&self, id: &str, end: u64) -> io::Result<Vec<Entry>> {
        let file = match File::open(self.child_path(id)) {
            Ok(f) => f,
            Err(e) if e.kind() == io::ErrorKind::NotFound => return Ok(Vec::new()),
            Err(e) => return Err(e),
        };
        if end > file.metadata()?.len() {
            return Ok(Vec::new());
        }
        let mut buf = Vec::new();
        file.take(end).read_to_end(&mut buf)?;
        Ok(parse_lines(&buf).0.iter().map(Entry::parse).collect())
    }

    /// `<id>.md` exists with content: the child finished and omp wrote its
    /// final output. A zero-byte `.md` is what omp leaves behind when it
    /// aborts the job on exit (141 R1 T5) — not a completion.
    #[must_use]
    pub fn sidecar_done(&self, id: &str) -> bool {
        fs::metadata(self.child_path(id).with_extension("md"))
            .is_ok_and(|m| m.is_file() && m.len() > 0)
    }

    /// The `<id>.md` final output, once written.
    #[must_use]
    pub fn sidecar_text(&self, id: &str) -> Option<String> {
        fs::read_to_string(self.child_path(id).with_extension("md")).ok()
    }

    /// The `<id>.json` structured output, once written (`observed_detail.result`).
    #[must_use]
    pub fn result_json(&self, id: &str) -> Option<Value> {
        serde_json::from_str(&fs::read_to_string(self.child_path(id).with_extension("json")).ok()?)
            .ok()
    }

    /// Job ids the parent transcript settled since `parent_cursor`
    /// (`custom_message{customType:"async-result"}.details.jobs[].jobId`, F22)
    /// and the new cursor.
    pub fn settled_jobs(&self, parent_cursor: u64) -> io::Result<(Vec<String>, u64)> {
        let (lines, cursor) = lines_from(&self.parent_file, parent_cursor)?;
        let jobs = lines
            .iter()
            .filter(|v| v["type"] == "custom_message" && v["customType"] == "async-result")
            .filter_map(|v| v["details"]["jobs"].as_array())
            .flatten()
            .filter_map(|j| j["jobId"].as_str().map(str::to_owned))
            .collect();
        Ok((jobs, cursor))
    }
}

/// Complete lines of `path` from `cursor` as JSON, and the cursor after the
/// last newline consumed. Missing file → nothing; `cursor` past EOF
/// (truncated / rotated) → start over at 0 (Q6). Unparseable lines are
/// skipped but still consumed.
fn lines_from(path: &Path, cursor: u64) -> io::Result<(Vec<Value>, u64)> {
    let mut file = match File::open(path) {
        Ok(f) => f,
        Err(e) if e.kind() == io::ErrorKind::NotFound => return Ok((Vec::new(), cursor)),
        Err(e) => return Err(e),
    };
    let len = file.metadata()?.len();
    let cursor = if cursor > len { 0 } else { cursor };
    file.seek(SeekFrom::Start(cursor))?;
    let mut buf = Vec::new();
    file.read_to_end(&mut buf)?;
    let (lines, end) = parse_lines(&buf);
    Ok((lines, cursor + end as u64))
}

/// The whole lines of `buf` as JSON (unparseable ones skipped) and the byte
/// length consumed (through the last newline).
fn parse_lines(buf: &[u8]) -> (Vec<Value>, usize) {
    let end = buf.iter().rposition(|&b| b == b'\n').map_or(0, |i| i + 1);
    let lines = buf[..end]
        .split(|&b| b == b'\n')
        .filter(|l| !l.is_empty())
        .filter_map(|l| serde_json::from_slice(l).ok())
        .collect();
    (lines, end)
}

/// A tool call inside a child transcript entry.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Call {
    /// Tool call id (shared by the `toolCall` part, `tool_execution_start` and `toolResult`).
    pub id: String,
    /// Tool name.
    pub name: String,
    /// Arguments as sent (JSON text).
    pub arguments: String,
    /// The intent the model gave, if any.
    pub intent: Option<String>,
}

/// Usage of one assistant entry in a child transcript (`message.usage`, F21).
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct Usage {
    /// Input tokens.
    pub input: u64,
    /// Output tokens.
    pub output: u64,
    /// Cache read tokens.
    pub cache_read: u64,
    /// Cache write tokens.
    pub cache_write: u64,
    /// Total tokens as omp reports them.
    pub total_tokens: u64,
    /// `cost.total` in USD.
    pub cost: f64,
}

/// One parsed child-transcript jsonl entry (141 §6.4 feed 2 table, F21).
#[derive(Clone, Debug, PartialEq)]
pub enum Entry {
    /// `session {id, cwd, parentSession}`.
    Session {
        /// omp's own session id for the child.
        id: String,
        /// Working directory.
        cwd: Option<String>,
        /// The parent transcript path omp recorded.
        parent_session: Option<String>,
    },
    /// `model_change {model}`.
    Model(String),
    /// `message role:user` text.
    User(String),
    /// `message role:assistant`: text parts, tool-call parts, usage, model.
    Assistant {
        /// Concatenated text parts.
        text: String,
        /// Tool-call parts in order.
        calls: Vec<Call>,
        /// Usage of this response.
        usage: Option<Usage>,
        /// `provider/model` of this response.
        model: Option<String>,
        /// `stopReason` (`toolUse`, `stop`, `aborted`, …).
        stop_reason: Option<String>,
    },
    /// `custom {customType:"tool_execution_start"}`.
    ToolStart(Call),
    /// `message role:toolResult`.
    ToolResult {
        /// Tool call id.
        call_id: String,
        /// Tool name.
        name: String,
        /// Result text.
        text: String,
        /// The tool failed.
        is_error: bool,
    },
    /// `title`, `session_init`, `thinking_level_change`, anything else.
    Other,
}

impl Entry {
    /// One jsonl line of a child transcript (F21 entry types).
    fn parse(v: &Value) -> Entry {
        let s = |x: &Value| x.as_str().map(str::to_owned);
        match v["type"].as_str() {
            Some("session") => Entry::Session {
                id: s(&v["id"]).unwrap_or_default(),
                cwd: s(&v["cwd"]),
                parent_session: s(&v["parentSession"]),
            },
            Some("model_change") => s(&v["model"]).map_or(Entry::Other, Entry::Model),
            Some("custom") if v["customType"] == "tool_execution_start" => {
                let d = &v["data"];
                Entry::ToolStart(Call {
                    id: s(&d["toolCallId"]).unwrap_or_default(),
                    name: s(&d["toolName"]).unwrap_or_default(),
                    arguments: args_text(&d["args"]),
                    intent: s(&d["intent"]),
                })
            }
            Some("message") => {
                let m = &v["message"];
                let content = m["content"]
                    .as_array()
                    .map(Vec::as_slice)
                    .unwrap_or_default();
                match m["role"].as_str() {
                    Some("user") => Entry::User(text_of(content)),
                    Some("assistant") => Entry::Assistant {
                        text: text_of(content),
                        calls: content
                            .iter()
                            .filter(|p| p["type"] == "toolCall")
                            .map(|p| Call {
                                id: s(&p["id"]).unwrap_or_default(),
                                name: s(&p["name"]).unwrap_or_default(),
                                arguments: args_text(&p["arguments"]),
                                intent: s(&p["intent"]),
                            })
                            .collect(),
                        usage: m.get("usage").filter(|u| u.is_object()).map(|u| Usage {
                            input: u["input"].as_u64().unwrap_or_default(),
                            output: u["output"].as_u64().unwrap_or_default(),
                            cache_read: u["cacheRead"].as_u64().unwrap_or_default(),
                            cache_write: u["cacheWrite"].as_u64().unwrap_or_default(),
                            total_tokens: u["totalTokens"].as_u64().unwrap_or_default(),
                            cost: u["cost"]["total"].as_f64().unwrap_or_default(),
                        }),
                        model: match (m["provider"].as_str(), m["model"].as_str()) {
                            (Some(p), Some(model)) => Some(format!("{p}/{model}")),
                            (None, Some(model)) => Some(model.to_owned()),
                            _ => None,
                        },
                        stop_reason: s(&m["stopReason"]),
                    },
                    Some("toolResult") => Entry::ToolResult {
                        call_id: s(&m["toolCallId"]).unwrap_or_default(),
                        name: s(&m["toolName"]).unwrap_or_default(),
                        text: text_of(content),
                        is_error: m["isError"].as_bool().unwrap_or(false),
                    },
                    _ => Entry::Other,
                }
            }
            _ => Entry::Other,
        }
    }
}

/// The `text` parts of a content array, joined.
fn text_of(content: &[Value]) -> String {
    content
        .iter()
        .filter(|p| p["type"] == "text")
        .filter_map(|p| p["text"].as_str())
        .collect()
}

/// Tool arguments as JSON text (`""` when absent).
fn args_text(v: &Value) -> String {
    if v.is_null() {
        String::new()
    } else {
        v.to_string()
    }
}

/// The SDK `SessionUpdate`s that replay `entries` on the child so
/// `acp_child`'s `translate()` is reused verbatim (141 D3): assistant text →
/// `agent_message_chunk`, a call → `tool_call` (`in_progress`, `title` = tool
/// name, `rawInput` = arguments), a result → `tool_call_update
/// completed|failed` with the text as content. User, session, model and
/// `tool_execution_start` entries produce nothing: the actor opens the turn
/// itself, and omp persists the assistant `toolCall` part before the start
/// marker (`ReadA.jsonl` 7→8, 10→11), so the marker would only duplicate
/// the card.
#[must_use]
pub fn to_updates(entries: &[Entry]) -> Vec<SessionUpdate> {
    let mut out = Vec::new();
    for e in entries {
        match e {
            Entry::Assistant { text, calls, .. } => {
                if !text.is_empty() {
                    out.push(SessionUpdate::AgentMessageChunk(ContentChunk::new(
                        text.as_str().into(),
                    )));
                }
                for c in calls {
                    let args = serde_json::from_str(&c.arguments).unwrap_or(Value::Null);
                    out.push(SessionUpdate::ToolCall(
                        ToolCall::new(ToolCallId::new(c.id.as_str()), c.name.as_str())
                            .kind(kind_of(&c.name))
                            .status(ToolCallStatus::InProgress)
                            .raw_input(args),
                    ));
                }
            }
            Entry::ToolResult {
                call_id,
                text,
                is_error,
                ..
            } => out.push(SessionUpdate::ToolCallUpdate(ToolCallUpdate::new(
                ToolCallId::new(call_id.as_str()),
                ToolCallUpdateFields::new()
                    .status(if *is_error {
                        ToolCallStatus::Failed
                    } else {
                        ToolCallStatus::Completed
                    })
                    .content(vec![ToolCallContent::from(text.as_str())]),
            ))),
            Entry::Session { .. }
            | Entry::Model(_)
            | Entry::User(_)
            | Entry::ToolStart(_)
            | Entry::Other => {}
        }
    }
    out
}

/// omp tool name → ACP kind (drives the card icon and `acp::risk`).
fn kind_of(tool: &str) -> ToolKind {
    match tool {
        "read" => ToolKind::Read,
        "grep" | "glob" | "web_search" => ToolKind::Search,
        "edit" | "write" | "ast_edit" => ToolKind::Edit,
        "bash" | "eval" => ToolKind::Execute,
        "web_fetch" => ToolKind::Fetch,
        _ => ToolKind::Other,
    }
}

#[cfg(test)]
mod tests {
    use std::fs;
    use std::io::Write;

    use super::*;

    const SID: &str = "01a11b25-d8f7-71b3-a309-a306e24281dc";
    const STEM: &str = "2026-10-08T10-53-47-639Z_01a11b25-d8f7-71b3-a309-a306e24281dc";

    fn fixture(name: &str) -> PathBuf {
        PathBuf::from(format!(
            "{}/fixtures/omp-18.3.1/{name}",
            env!("CARGO_MANIFEST_DIR")
        ))
    }

    /// A throwaway `~` with omp's layout (F21): the parent transcript
    /// `.omp/agent/sessions/<encoded-cwd>/<ts>_<sid>.jsonl` and the artifacts
    /// dir beside it holding `ReadA.jsonl` (sidecars added by the tests).
    struct Home(PathBuf);

    impl Home {
        fn new() -> Self {
            let root = std::env::temp_dir().join(format!("acp-omp-{}", uuid::Uuid::new_v4()));
            let cwd = root
                .join(".omp/agent/sessions")
                .join("--private-tmp-recon-141-omp-scratch--");
            fs::create_dir_all(cwd.join(STEM)).unwrap();
            fs::copy(fixture("parent.jsonl"), cwd.join(format!("{STEM}.jsonl"))).unwrap();
            fs::copy(fixture("ReadA.jsonl"), cwd.join(STEM).join("ReadA.jsonl")).unwrap();
            Home(root)
        }

        fn tailer(&self) -> Tailer {
            Tailer::locate(&self.0, SID).expect("located")
        }
    }

    impl Drop for Home {
        fn drop(&mut self) {
            let _ = fs::remove_dir_all(&self.0);
        }
    }

    /// 141 §6.4 feed 2: `locate` globs `*/*_<sid>.jsonl`; the artifacts dir
    /// is the file minus `.jsonl`; nested `a.b` lives in `a/`; an unknown
    /// session or a home without omp finds nothing.
    #[test]
    fn locate_finds_the_parent_file_and_names_child_paths() {
        let home = Home::new();
        let t = home.tailer();
        assert!(t.parent_file.ends_with(format!("{STEM}.jsonl")));
        assert_eq!(t.artifacts, t.parent_file.with_extension(""));
        assert_eq!(t.child_path("ReadA"), t.artifacts.join("ReadA.jsonl"));
        assert_eq!(
            t.child_path("ReadA.sub"),
            t.artifacts.join("ReadA").join("ReadA.sub.jsonl")
        );
        assert!(Tailer::locate(&home.0, "nobody").is_none());
        assert!(Tailer::locate(&home.0.join("elsewhere"), SID).is_none());
    }

    /// 141 §6.4 feed 2 table on `ReadA.jsonl`: the 12 entries parse to their
    /// kinds, the assistant entries carry usage/model/calls, and
    /// `to_updates` replays read → result → yield → result as SDK updates
    /// (user/start markers produce nothing); the cursor lands at EOF and a
    /// second read is empty.
    #[test]
    fn read_a_transcript_replays_as_session_updates() {
        let home = Home::new();
        let t = home.tailer();
        let (entries, cursor) = t.read("ReadA", 0).unwrap();
        assert_eq!(cursor, fs::metadata(t.child_path("ReadA")).unwrap().len());
        assert_eq!(entries.len(), 12);
        assert!(matches!(
            &entries[1],
            Entry::Session { id, cwd: Some(cwd), parent_session: Some(p) }
                if id == "01a11b25-eb85-7543-b4dd-a28e46375917"
                    && cwd == "/tmp/recon-141/omp-scratch"
                    && p.ends_with(&format!("{STEM}.jsonl"))
        ));
        assert_eq!(
            entries[2],
            Entry::Model("anthropic/claude-fable-5-1".into())
        );
        assert_eq!(entries[0], Entry::Other);
        assert_eq!(entries[3], Entry::Other);
        assert!(matches!(&entries[5], Entry::User(t) if t.starts_with("Complete assignment")));
        let Entry::Assistant {
            text,
            calls,
            usage,
            model,
            stop_reason,
        } = &entries[6]
        else {
            panic!("{:?}", entries[6]);
        };
        assert_eq!(stop_reason.as_deref(), Some("toolUse"));
        assert_eq!(text, "");
        assert_eq!(calls.len(), 1);
        assert_eq!(calls[0].id, "toolu_01N97dmhtms4GH34fEciDfVT");
        assert_eq!(calls[0].name, "read");
        assert_eq!(calls[0].intent.as_deref(), Some("Reading first line"));
        assert_eq!(
            serde_json::from_str::<Value>(&calls[0].arguments).unwrap()["path"],
            "a.txt:1-1"
        );
        let u = usage.unwrap();
        assert_eq!(
            (u.input, u.output, u.cache_write, u.total_tokens),
            (4, 77, 8881, 8962)
        );
        assert_eq!(u.cost, 0.1149025);
        assert_eq!(model.as_deref(), Some("anthropic/claude-fable-5-1"));
        assert!(matches!(&entries[7], Entry::ToolStart(c) if c.name == "read"));
        assert!(matches!(
            &entries[8],
            Entry::ToolResult { call_id, name, text, is_error: false }
                if call_id == "toolu_01N97dmhtms4GH34fEciDfVT" && name == "read"
                    && text == "alpha line one\nalpha line two"
        ));
        assert!(matches!(&entries[9], Entry::Assistant { calls, .. } if calls[0].name == "yield"));
        assert!(
            matches!(&entries[11], Entry::ToolResult { text, .. } if text == "Result submitted.")
        );

        let updates = to_updates(&entries);
        assert_eq!(updates.len(), 4);
        let SessionUpdate::ToolCall(tc) = &updates[0] else {
            panic!("{:?}", updates[0]);
        };
        assert_eq!(&*tc.tool_call_id.0, "toolu_01N97dmhtms4GH34fEciDfVT");
        assert_eq!(
            (tc.title.as_str(), tc.kind, tc.status),
            ("read", ToolKind::Read, ToolCallStatus::InProgress)
        );
        assert_eq!(tc.raw_input.as_ref().unwrap()["path"], "a.txt:1-1");
        let SessionUpdate::ToolCallUpdate(u) = &updates[1] else {
            panic!("{:?}", updates[1]);
        };
        assert_eq!(&*u.tool_call_id.0, "toolu_01N97dmhtms4GH34fEciDfVT");
        assert_eq!(u.fields.status, Some(ToolCallStatus::Completed));
        assert_eq!(u.fields.content.as_ref().unwrap().len(), 1);
        assert!(
            matches!(&updates[2], SessionUpdate::ToolCall(tc) if tc.title == "yield" && tc.kind == ToolKind::Other)
        );
        assert!(
            matches!(&updates[3], SessionUpdate::ToolCallUpdate(u) if u.fields.status == Some(ToolCallStatus::Completed))
        );

        let (again, cursor2) = t.read("ReadA", cursor).unwrap();
        assert!(again.is_empty());
        assert_eq!(cursor2, cursor);
    }

    /// 141 §6.4 / Q6: a partial trailing line is left unread (the cursor
    /// stays before it) until its newline lands; a failed tool result maps to
    /// `failed`; assistant text becomes a message chunk; a truncated file
    /// (cursor past EOF) restarts at 0; a child without a file reads nothing.
    #[test]
    fn cursor_stops_at_a_partial_line_and_resets_on_truncation() {
        let home = Home::new();
        let t = home.tailer();
        let path = t.child_path("ReadA");
        let (_, eof) = t.read("ReadA", 0).unwrap();

        let line = r#"{"type":"message","id":"x","parentId":"y","message":{"role":"toolResult","toolCallId":"c9","toolName":"bash","content":[{"type":"text","text":"boom"}],"isError":true}}"#;
        let (half, rest) = line.split_at(40);
        let mut f = fs::OpenOptions::new().append(true).open(&path).unwrap();
        f.write_all(half.as_bytes()).unwrap();
        let (entries, cursor) = t.read("ReadA", eof).unwrap();
        assert!(entries.is_empty());
        assert_eq!(cursor, eof);

        f.write_all(rest.as_bytes()).unwrap();
        f.write_all(b"\n").unwrap();
        f.write_all(br#"{"type":"message","message":{"role":"assistant","content":[{"type":"text","text":"done"}]}}"#).unwrap();
        f.write_all(b"\n").unwrap();
        let (entries, cursor) = t.read("ReadA", eof).unwrap();
        assert_eq!(entries.len(), 2);
        assert_eq!(cursor, fs::metadata(&path).unwrap().len());
        let updates = to_updates(&entries);
        assert!(matches!(&updates[0], SessionUpdate::ToolCallUpdate(u)
            if &*u.tool_call_id.0 == "c9" && u.fields.status == Some(ToolCallStatus::Failed)));
        assert!(matches!(&updates[1], SessionUpdate::AgentMessageChunk(c)
            if serde_json::to_value(&c.content).unwrap()["text"] == "done"));

        fs::write(&path, "{\"type\":\"title\"}\n").unwrap();
        let (entries, cursor) = t.read("ReadA", eof).unwrap();
        assert_eq!(entries, vec![Entry::Other]);
        assert_eq!(cursor, 17);

        assert_eq!(t.read("ReadB", 5).unwrap(), (Vec::new(), 5));
    }

    /// 141 §6.4: completion comes from disk — `<id>.md` appearing (with its
    /// text and the `.json` result), and the parent's `async-result` entry
    /// naming settled job ids; the parent cursor advances so a job is
    /// reported once.
    #[test]
    fn sidecar_and_async_result_settle_children() {
        let home = Home::new();
        let t = home.tailer();
        assert!(!t.sidecar_done("ReadA"));
        assert_eq!(t.sidecar_text("ReadA"), None);
        assert_eq!(t.result_json("ReadA"), None);
        // 141 R1 T5: an aborted job leaves a 0-byte `.md` — not done.
        fs::write(t.artifacts.join("ReadA.md"), b"").unwrap();
        assert!(!t.sidecar_done("ReadA"));
        fs::copy(fixture("ReadA.md"), t.artifacts.join("ReadA.md")).unwrap();
        fs::copy(fixture("ReadA.json"), t.artifacts.join("ReadA.json")).unwrap();
        assert!(t.sidecar_done("ReadA"));
        assert!(t.sidecar_text("ReadA").unwrap().contains("alpha line one"));
        assert_eq!(
            t.result_json("ReadA").unwrap()["first_line"],
            "alpha line one"
        );
        assert!(!t.sidecar_done("ReadB"));

        let (jobs, cursor) = t.settled_jobs(0).unwrap();
        assert_eq!(jobs, ["ReadA"]);
        assert_eq!(cursor, fs::metadata(&t.parent_file).unwrap().len());
        assert_eq!(t.settled_jobs(cursor).unwrap(), (Vec::new(), cursor));
    }

    /// 141 R1 T5: a child omp aborted on exit (`SlowA.aborted.jsonl`, recorded
    /// 2026-10-08 after SIGTERM of the engine): the open call's result is
    /// `Command aborted` (`isError`), the closing assistant entry has
    /// `stopReason: aborted` and empty content; `to_updates` yields the
    /// call and its failed result and nothing for the empty entry.
    #[test]
    fn aborted_child_entries_carry_the_stop_reason() {
        let home = Home::new();
        let t = home.tailer();
        fs::copy(
            fixture("SlowA.aborted.jsonl"),
            t.artifacts.join("SlowA.jsonl"),
        )
        .unwrap();
        let (entries, _) = t.read("SlowA", 0).unwrap();
        assert_eq!(entries.len(), 8);
        assert!(matches!(
            &entries[3],
            Entry::Assistant { calls, stop_reason: Some(r), .. }
                if calls[0].name == "bash" && r == "toolUse"
        ));
        assert!(matches!(
            &entries[6],
            Entry::ToolResult { text, is_error: true, .. } if text == "Command aborted"
        ));
        assert!(matches!(
            &entries[7],
            Entry::Assistant { text, calls, stop_reason: Some(r), .. }
                if text.is_empty() && calls.is_empty() && r == "aborted"
        ));
        let updates = to_updates(&entries);
        assert_eq!(updates.len(), 2);
        assert!(matches!(&updates[1], SessionUpdate::ToolCallUpdate(u)
            if u.fields.status == Some(ToolCallStatus::Failed)));
    }
}
