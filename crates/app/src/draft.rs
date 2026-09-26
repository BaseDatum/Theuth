//! The `task_draft` tool (69 TASKS, Sky's rulings): when the user describes
//! work in chat, the agent proposes tasks as an OpenUI card set the user
//! approves in place. Each card carries the proposed fields and three
//! `@ToAssistant` buttons — *Add to backlog*, *Start now*, *Discard* — whose
//! press arrives in chat as the user's message naming the draft id and the
//! choice; the agent then calls `task_create` with `state: backlog | todo`, or
//! nothing. Drafts are never persisted as tasks: a draft id lives in the card
//! only. The card set goes through the same commit path as the `app` tool, so
//! it renders inline under the tool call and opens in the artifact panel.

use std::sync::Arc;

use agent::attach::AttachRef;
use serde::{Deserialize, Serialize};
use tools::{Risk, Tool, ToolCx, ToolOutput};

use crate::actor::Core;
use crate::tool::{AppArgs, Origin, commit_source};

/// App name every draft set is published under (a new version per call).
const APP_NAME: &str = "task-drafts";

/// One proposed task.
#[derive(Clone, Debug, Default, Serialize, Deserialize, schemars::JsonSchema)]
pub struct Draft {
    /// One line, imperative: what done looks like.
    pub title: String,
    /// Markdown brief: context, constraints, acceptance, links. The whole of what the owner gets.
    pub brief: String,
    /// `agent:<name>` or `user`; default: you (the calling agent).
    #[serde(default)]
    pub owner: Option<String>,
    /// Project label; default: the session's project.
    #[serde(default)]
    pub project: Option<String>,
    /// Node the work must run on (a `mesh_peers` name); unset = theuth decides.
    #[serde(default, rename = "where")]
    pub where_: Option<String>,
    /// Task ids that wait on this one (set on them with `task_update` after approval).
    #[serde(default)]
    pub blocks: Vec<String>,
    /// What this one waits on: task ids, `decision:<id>`, `person:<name>`.
    #[serde(default)]
    pub blocked_by: Vec<String>,
    /// Acceptance criteria: one to three checkable lines; drafted from the brief when the user did not
    /// state them. The user sees and can correct them before approval.
    #[serde(default)]
    pub done_when: Option<String>,
    /// References the task is about (`{kind: note|knowledge|artifact|session|meeting, id, title}`). The
    /// `[attached …]` lines of the user's message are added for you; list here anything else you propose.
    #[serde(default)]
    pub attachments: Vec<AttachRef>,
}

/// Arguments of `task_draft`.
#[derive(Clone, Debug, Serialize, Deserialize, schemars::JsonSchema)]
pub struct TaskDraftArgs {
    /// One or more proposed tasks, in the order they should be read.
    pub tasks: Vec<Draft>,
}

/// `task_draft`: proposed tasks as approval cards in chat.
pub struct TaskDraftTool {
    /// Shared with the actor.
    pub core: Arc<Core>,
}

#[async_trait::async_trait]
impl Tool for TaskDraftTool {
    const NAME: &'static str = "task_draft";
    const RISK: Risk = Risk::ReadOnly;
    const RETURNS: &'static str = "dict";
    const CONSUMES_SECRETS: bool = false;
    type Args = TaskDraftArgs;

    fn description(&self) -> String {
        "Propose tasks for the user's approval before anything is created. When the user describes work \
         (\"we need to…\", \"can someone…\", a plan with several pieces), split it into tasks and call this \
         with 1..n drafts {title, brief, done_when, owner?, project?, where?, blocks?, blocked_by?, attachments?} — \
         `done_when` is the acceptance criteria, drafted from the brief when the user did not state them; \
         `attachments` are references the owner will read (the user's `[attached …]` lines are added by default). \
         Each renders as a card with three buttons: Add to backlog, Start now, Discard. Then STOP and wait: the \
         press arrives as the user's next message (it begins `[app task-drafts …]`) naming the draft id and the \
         choice — `add to backlog` → `task_create` with `state: backlog`; `start now` → `task_create` with \
         `state: todo`; `discard` → nothing. Create only what was approved, with the draft's fields (attachments \
         included), and set blockers with `task_update` afterwards. Drafts are not tasks: nothing exists until approval."
            .into()
    }

    async fn call(&self, mut args: TaskDraftArgs, cx: ToolCx) -> anyhow::Result<ToolOutput> {
        if args.tasks.is_empty() {
            return Ok(ToolOutput::err("task_draft: at least one task"));
        }
        if let Some(i) = args.tasks.iter().position(|d| d.title.trim().is_empty()) {
            return Ok(ToolOutput::err(format!("task_draft: task {} has no title", i + 1)));
        }
        let context = turn_attachments(cx.session).await;
        for d in &mut args.tasks {
            offer(&mut d.attachments, &context);
        }
        let alnum: Vec<char> = cx.call_id.chars().filter(char::is_ascii_alphanumeric).collect();
        let suffix: String = alnum[alnum.len().saturating_sub(4)..].iter().collect();
        let (source, ids) = source(&args.tasks, &suffix);
        let app = AppArgs {
            name: APP_NAME.into(),
            source,
            title: Some(if ids.len() == 1 { "Task draft".into() } else { format!("{} task drafts", ids.len()) }),
            edit: false,
            base_call_id: None,
        };
        let (loaded, _) = match commit_source(&self.core, app, cx.session, cx.call_id.clone(), Origin::Tool).await {
            Ok(v) => v,
            Err(e) => return Ok(ToolOutput::err(format!("task_draft: {e}"))),
        };
        let lines: Vec<String> = ids
            .iter()
            .zip(&args.tasks)
            .map(|(id, d)| {
                let mut l = format!("{id} · {}", d.title.trim());
                if !d.attachments.is_empty() {
                    l.push_str(&format!(" · attachments: {}", d.attachments.iter().map(label).collect::<Vec<_>>().join(", ")));
                }
                l
            })
            .collect();
        let content = format!(
            "drafted {} task card{} (record_call_id {}):\n{}\nWait for the user. Their press arrives as a message \
             beginning `[app {APP_NAME} …]` with `draft <id>: add to backlog | start now | discard`. Then call \
             `task_create` with `state: backlog` or `state: todo` using that draft's fields — `attachments` \
             included, as `{{kind, id, title}}` — (nothing for discard), and `task_update` for its blockers.",
            ids.len(),
            if ids.len() == 1 { "" } else { "s" },
            loaded.artifact.call_id,
            lines.join("\n"),
        );
        Ok(ToolOutput {
            structured: Some(serde_json::json!({
                "record_call_id": loaded.artifact.call_id,
                "drafts": ids.iter().zip(&args.tasks).map(|(id, d)| serde_json::json!({ "id": id, "title": d.title, "attachments": d.attachments })).collect::<Vec<_>>(),
            })),
            ..ToolOutput::ok(content)
        })
    }
}

/// A double-quoted OpenUI string literal: the tokenizer reads it as JSON.
pub(crate) fn lit(s: &str) -> String {
    serde_json::to_string(s).unwrap_or_else(|_| "\"\"".into())
}

/// `<kind> "<title>"`.
fn label(r: &AttachRef) -> String {
    format!("{} \"{}\"", r.kind, if r.title.is_empty() { &r.id } else { &r.title })
}

/// Add `context` refs (the user's message attachments) that `refs` lacks, by kind + id.
pub fn offer(refs: &mut Vec<AttachRef>, context: &[AttachRef]) {
    for r in context {
        if !refs.iter().any(|x| x.kind == r.kind && x.id == r.id) {
            refs.push(r.clone());
        }
    }
}

/// The `[attached …]` references of the calling session's latest user turn — the
/// message the drafts are being made from. Empty when the transcript is unreadable.
async fn turn_attachments(session: runtime::SessionId) -> Vec<AttachRef> {
    let Ok(Some(agents)) = kameo::actor::ActorRef::<agent::Agents>::lookup(agent::AGENT_ACTOR) else { return vec![] };
    let Ok(read) = agents.ask(agent::ReadSession { session, caller: session }).await else { return vec![] };
    read.history
        .iter()
        .rev()
        .find_map(|e| match e {
            agent::HistoryEntry::User { text, .. } => Some(agent::attach::parse(text.as_str()).refs),
            _ => None,
        })
        .unwrap_or_default()
}

/// The card set's OpenUI source and the draft ids, one per task (`d1-<suffix>`, …).
/// Every button's `@ToAssistant` message is `draft <id>: <choice>` so the approval
/// names the card without any state on the engine.
pub fn source(tasks: &[Draft], suffix: &str) -> (String, Vec<String>) {
    let ids: Vec<String> = (1..=tasks.len()).map(|i| if suffix.is_empty() { format!("d{i}") } else { format!("d{i}-{suffix}") }).collect();
    let mut out = String::new();
    let cards: Vec<String> = (1..=tasks.len()).map(|i| format!("c{i}")).collect();
    out.push_str(&format!("root = Stack([intro, {}])\n", cards.join(", ")));
    out.push_str(&format!(
        "intro = TextContent({}, \"small\")\n",
        lit(&format!(
            "**{} task draft{}** — for each: *Add to backlog* (on the board, not started), *Start now* (queued for the owner's desk), or *Discard*.",
            tasks.len(),
            if tasks.len() == 1 { "" } else { "s" }
        ))
    ));
    for (i, (d, id)) in tasks.iter().zip(&ids).enumerate() {
        let c = &cards[i];
        let mut meta = vec![
            format!("owner `{}`", d.owner.as_deref().unwrap_or("you")),
            format!("project `{}`", d.project.as_deref().unwrap_or("session's")),
            format!("where `{}`", d.where_.as_deref().unwrap_or("theuth decides")),
        ];
        if !d.blocked_by.is_empty() {
            meta.push(format!("blocked by {}", d.blocked_by.join(", ")));
        }
        if !d.blocks.is_empty() {
            meta.push(format!("blocks {}", d.blocks.join(", ")));
        }
        let brief = if d.brief.trim().is_empty() { "_No brief._" } else { d.brief.trim() };
        let brief = match d.done_when.as_deref().map(str::trim).filter(|w| !w.is_empty()) {
            Some(w) => format!("{brief}\n\n**Done when:** {w}"),
            None => brief.to_owned(),
        };
        let brief = if d.attachments.is_empty() {
            brief
        } else {
            format!("{brief}\n\n**Attached:** {}", d.attachments.iter().map(label).collect::<Vec<_>>().join(" · "))
        };
        out.push_str(&format!("{c} = Card([{c}_head, {c}_brief, {c}_meta, {c}_buttons])\n"));
        out.push_str(&format!("{c}_head = CardHeader({}, {})\n", lit(d.title.trim()), lit(&format!("draft {id}"))));
        out.push_str(&format!("{c}_brief = TextContent({})\n", lit(&brief)));
        out.push_str(&format!("{c}_meta = TextContent({}, \"small\")\n", lit(&meta.join(" · "))));
        out.push_str(&format!("{c}_buttons = Buttons([{c}_backlog, {c}_start, {c}_discard])\n"));
        out.push_str(&format!(
            "{c}_backlog = Button(\"Add to backlog\", Action([@ToAssistant({})]))\n",
            lit(&format!("draft {id}: add to backlog"))
        ));
        out.push_str(&format!(
            "{c}_start = Button(\"Start now\", Action([@ToAssistant({})]), \"secondary\")\n",
            lit(&format!("draft {id}: start now"))
        ));
        out.push_str(&format!(
            "{c}_discard = Button(\"Discard\", Action([@ToAssistant({})]), \"tertiary\", \"destructive\")\n",
            lit(&format!("draft {id}: discard"))
        ));
    }
    (out, ids)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn card_buttons_carry_the_draft_id_and_choice() {
        let drafts = vec![
            Draft { title: "Fix the \"flaky\" deploy".into(), brief: "line one\nline two".into(), owner: Some("agent:ops".into()), project: Some("sre".into()), ..Default::default() },
            Draft { title: "Write release notes".into(), brief: String::new(), blocked_by: vec!["0192-abc".into()], ..Default::default() },
        ];
        let (src, ids) = source(&drafts, "ab12");
        assert_eq!(ids, ["d1-ab12", "d2-ab12"]);
        assert!(src.starts_with("root = Stack([intro, c1, c2])\n"));
        for (id, c) in ids.iter().zip(["c1", "c2"]) {
            assert!(src.contains(&format!("{c}_backlog = Button(\"Add to backlog\", Action([@ToAssistant(\"draft {id}: add to backlog\")]))")), "{src}");
            assert!(src.contains(&format!("{c}_start = Button(\"Start now\", Action([@ToAssistant(\"draft {id}: start now\")]), \"secondary\")")), "{src}");
            assert!(src.contains(&format!("{c}_discard = Button(\"Discard\", Action([@ToAssistant(\"draft {id}: discard\")]), \"tertiary\", \"destructive\")")), "{src}");
            assert!(src.contains(&format!("\"draft {id}\"")), "the header names the draft");
        }
        // strings are JSON literals: quotes and newlines survive the tokenizer
        assert!(src.contains(r#"c1_head = CardHeader("Fix the \"flaky\" deploy", "draft d1-ab12")"#));
        assert!(src.contains(r#"c1_brief = TextContent("line one\nline two")"#));
        assert!(src.contains("owner `agent:ops` · project `sre` · where `theuth decides`"));
        assert!(src.contains("owner `you` · project `session's` · where `theuth decides` · blocked by 0192-abc"));
        assert!(src.contains(r#"c2_brief = TextContent("_No brief._")"#));
        // one statement per line, every card reachable from root
        assert!(src.lines().all(|l| l.contains(" = ")));
        assert_eq!(source(&drafts[..1], "").1, ["d1"]);
    }

    /// 70 TASK-ATTACHMENTS: the card names what is attached; the user's message
    /// refs are offered on top of the model's, once each.
    #[test]
    fn drafts_carry_attachments() {
        use agent::attach::Kind;
        let note = AttachRef { kind: Kind::Note, id: "n1".into(), title: "Groceries".into(), meta: String::new() };
        let art = AttachRef { kind: Kind::Artifact, id: "toolu_1".into(), title: "Q3 \"report\"".into(), meta: "pdf".into() };
        let mut refs = vec![note.clone()];
        offer(&mut refs, &[note.clone(), art.clone()]);
        assert_eq!(refs, vec![note, art]);
        let d = Draft { title: "Fold it".into(), brief: "b".into(), attachments: refs, ..Default::default() };
        let (src, _) = source(&[d], "");
        assert!(src.contains(r#"c1_brief = TextContent("b\n\n**Attached:** note \"Groceries\" · artifact \"Q3 \"report\"\"")"#), "{src}");
    }
}
