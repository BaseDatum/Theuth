//! What the roster knows that other plugins ask about synchronously (67 §6.2,
//! §6.3, §9): the parsed files, which session is which agent's desk, and which
//! agent a session belongs to. Shared between the [`crate::Roster`] actor (the
//! writer) and [`crate::Desks`], the object registered with the tasks, tools
//! and knowledge plugins (the readers). Plus the desk's texts (operating
//! manual, opening block, closing prompt, scoped wake) and the episode
//! bookkeeping the actor drives (67 §6.2, §7.3).

use std::collections::{BTreeMap, BTreeSet, HashMap};
use std::path::{Path, PathBuf};
use std::sync::RwLock;

use runtime::SessionId;

use crate::types::{AgentFile, AgentName};

/// `<data_dir>/roster/journal/<name>.md` lives under this directory.
pub const JOURNAL_DIR: &str = "roster/journal";

/// 67 §6.3: may an agent see MCP `server`'s `tool`? No `mcp:` key is no limit
/// (`theuth`, files without the key), `mcp: []` hides every server; otherwise
/// an entry names a whole server (`grafana`) or one tool
/// (`argocd_prod.list_applications`).
pub fn mcp_visible(agent: &AgentFile, server: &str, tool: &str) -> bool {
    agent.mcp.as_ref().is_none_or(|l| {
        l.iter().any(|m| {
            m == server
                || m
                    .split_once('.')
                    .is_some_and(|(s, t)| s == server && t == tool)
        })
    })
}

/// The closing turn's prompt (67 §6.2, §9).
pub const CLOSING: &str = "Write your journal: Done / Open / Next / Notes, ≤ 8 KiB. Before you answer, record anything \
durable you learned this episode with `knowledge_put` (it is private to you by default) and call `knowledge_share` \
with `visibility: team` or `shared` for the items your team or everyone should know. Every task you checked out \
must already have its disposition. Answer with the journal text only: it is written to your journal file as is and \
is what your next episode starts from.";

/// The desk's operating manual (67 §6.2, §7.3): the paragraph that follows
/// the charter and the journal in the opening block.
pub fn manual(f: &AgentFile) -> String {
    let name = f.name.as_str();
    let model = f.model.as_deref().unwrap_or("the default");
    let tools = f.tools.as_deref().unwrap_or("destructive");
    format!(
        "You are the desk of agent `{name}` ({title}). You take wakes, read your inbox with `task_inbox`, \
check out one task with `task_checkout`, open a child session for it with `spawn_subagent` exactly where the task \
says (`agent: \"<harness>@<engine>\"` from the task's `harness` and `where`, or the engine name alone for the native \
harness; pass `model` and `tools` from your file: model {model}, tools {tools}), review the result, then set the \
disposition with `task_update`. Never do the work in this session: no files, no shells, no browsing here, only \
through a child. Handle the task the wake names before any other; write progress with `task_note`; call `escalate` \
when you need a decision you may not make yourself, and wait for the answer. Disposition rule: an episode's closing \
turn must leave every task it checked out in `review | blocked | done(by reviewer) | delegated (child task exists) | \
continuing (a schedule, reminder or live child will wake it)`. A task left in `doing` with none of these is set \
`blocked: \"no continuation\"` and the manager is woken. Tasks in `backlog` are not dispatched: they are not in \
your inbox and you do not `task_move` them to `todo` — the user starts them. A role missing from the roster is a \
proposal, not a hire: `agents_list`, then `agent_propose` (it lands paused and files a `hire_agent` approval up \
your line); the user decides. Your `knowledge_put` writes are private to you by default; \
`knowledge_share` opens them to your team or everyone. When asked to write your journal, answer with the journal only.",
        title = f.title,
    )
}

/// The head of an episode's first input: charter, journal, manual (67 §6.2:
/// "the next episode starts from the charter + journal").
pub fn opening(f: &AgentFile, journal: Option<&str>) -> String {
    let journal = journal
        .map(str::trim)
        .filter(|j| !j.is_empty())
        .unwrap_or("(first episode: no journal yet)");
    format!(
        "<desk agent=\"{name}\">\n<charter>\n{charter}\n</charter>\n<journal>\n{journal}\n</journal>\n{manual}\n</desk>",
        name = f.name,
        charter = f.body.trim(),
        manual = manual(f),
    )
}

/// 67 §7.3's scoped wake, as one input: the header line, then the task
/// record the desk needs to act without another lookup.
pub fn wake_text(wake: &tasks::Wake, task: Option<&tasks::Task>) -> String {
    let mut s = String::from("Wake: ");
    match (task, &wake.task) {
        (Some(t), _) => s.push_str(&format!("task {} — {}.", t.id, t.title.trim())),
        (None, Some(id)) => s.push_str(&format!("task {id}.")),
        (None, None) => s.push_str("no task."),
    }
    s.push_str(&format!(
        " Reason: {}.{} Do not switch to another task until this wake is handled.",
        wake.reason.as_str(),
        if wake.count > 1 {
            format!(" (Raised {} times.)", wake.count)
        } else {
            String::new()
        }
    ));
    if let Some(t) = task {
        s.push_str(&format!(
            "\n\n<task id=\"{}\" state=\"{}\" owner=\"{}\"",
            t.id,
            t.state.as_str(),
            t.owner
        ));
        if let Some(r) = &t.reviewer {
            s.push_str(&format!(" reviewer=\"{r}\""));
        }
        if let Some(p) = &t.project {
            s.push_str(&format!(" project=\"{p}\""));
        }
        if let Some(w) = &t.where_ {
            s.push_str(&format!(" where=\"{w}\""));
        }
        if let Some(h) = &t.harness {
            s.push_str(&format!(" harness=\"{h}\""));
        }
        if let Some(th) = &t.thread {
            s.push_str(&format!(" thread=\"{th}\""));
        }
        s.push_str(">\n");
        s.push_str(t.brief.trim());
        if !t.blockers.is_empty() {
            s.push_str(&format!("\n\nBlockers: {}", t.blockers.join("; ")));
        }
        if !t.external.is_empty() {
            s.push_str(&format!("\n\nExternal: {}", t.external.join(", ")));
        }
        s.push_str("\n</task>");
    }
    s
}

/// Read `<data_dir>/roster/journal/<name>.md`; `None` when there is none.
pub fn read_journal(data_dir: &Path, name: &str) -> Option<String> {
    std::fs::read_to_string(journal_path(data_dir, name)).ok()
}

/// Overwrite the agent's journal with the closing turn's reply, cut to
/// `max_bytes` at a char boundary (67 §6.2: ≤ 8 KiB, private).
pub fn write_journal(data_dir: &Path, name: &str, text: &str, max_bytes: usize) -> std::io::Result<PathBuf> {
    let text = text.trim();
    let mut cut = text.len().min(max_bytes);
    while !text.is_char_boundary(cut) {
        cut -= 1;
    }
    let path = journal_path(data_dir, name);
    if let Some(dir) = path.parent() {
        std::fs::create_dir_all(dir)?;
    }
    let tmp = path.with_extension("md.tmp");
    std::fs::write(&tmp, format!("{}\n", &text[..cut]))?;
    std::fs::rename(&tmp, &path)?;
    Ok(path)
}

fn journal_path(data_dir: &Path, name: &str) -> PathBuf {
    data_dir.join(JOURNAL_DIR).join(format!("{name}.md"))
}

/// One live desk as the actor tracks it (67 §6.2).
#[derive(Clone, Debug)]
pub struct Desk {
    /// The pinned session.
    pub session: SessionId,
    /// `episodes.id`.
    pub episode: String,
    /// Last wake delivered, turn started or turn finished (ms since epoch).
    pub last_active_ms: u64,
    /// A turn is running.
    pub working: bool,
    /// The opening block was delivered: the transcript holds context worth a journal.
    pub primed: bool,
    /// Model the last turn ran on (for the context window).
    pub model: Option<String>,
}

impl Desk {
    /// Idle > `idle_secs` since the last activity, with something in the
    /// transcript and no turn running (67 §6.2 "idle 6 h").
    pub fn idle_due(&self, now_ms: u64, idle_secs: u64) -> bool {
        self.primed && !self.working && now_ms.saturating_sub(self.last_active_ms) > idle_secs * 1000
    }
}

/// `context` tokens exceed `pct` percent of a `window` of known size (67 §6.2 "60 % of context").
pub fn context_due(context: u32, window: u32, pct: u8) -> bool {
    window > 0 && u64::from(context) * 100 > u64::from(window) * u64::from(pct)
}

/// One roster file as last read: parsed, or the reason it did not parse.
pub type Parsed = Result<AgentFile, String>;

#[derive(Default)]
struct State {
    /// Every `agents/<name>.md`, by stem. Never holds `theuth`.
    files: BTreeMap<String, Parsed>,
    /// The synthesised default agent (`file::default_agent`).
    theuth: Option<AgentFile>,
    /// Agent → its current desk session.
    desks: HashMap<String, SessionId>,
    /// Session → the agent it works for: desks and every descendant.
    sessions: HashMap<SessionId, String>,
}

/// The roster's shared read model.
#[derive(Default)]
pub struct Shared {
    state: RwLock<State>,
}

impl Shared {
    fn read(&self) -> std::sync::RwLockReadGuard<'_, State> {
        self.state.read().unwrap_or_else(|p| p.into_inner())
    }

    fn write(&self) -> std::sync::RwLockWriteGuard<'_, State> {
        self.state.write().unwrap_or_else(|p| p.into_inner())
    }

    /// Replace the file set (a reconcile).
    pub fn set_files(&self, files: BTreeMap<String, Parsed>, theuth: AgentFile) {
        let mut s = self.write();
        s.files = files;
        s.theuth = Some(theuth);
    }

    /// One file changed or appeared.
    pub fn set_file(&self, name: &str, parsed: Parsed) {
        self.write().files.insert(name.to_owned(), parsed);
    }

    /// One file is gone.
    pub fn remove_file(&self, name: &str) {
        self.write().files.remove(name);
    }

    /// Every roster file (not `theuth`), by name.
    pub fn files(&self) -> BTreeMap<String, Parsed> {
        self.read().files.clone()
    }

    /// The agent's file, `theuth` included; `None` for an unknown or invalid name.
    pub fn file(&self, name: &str) -> Option<AgentFile> {
        let s = self.read();
        if name == AgentName::DEFAULT {
            return s.theuth.clone();
        }
        s.files.get(name).and_then(|r| r.as_ref().ok()).cloned()
    }

    /// The synthesised default agent.
    pub fn theuth(&self) -> Option<AgentFile> {
        self.read().theuth.clone()
    }

    /// A desk opened: the session (and later its children) belongs to `agent`.
    pub fn set_desk(&self, agent: &str, session: SessionId) {
        let mut s = self.write();
        s.desks.insert(agent.to_owned(), session);
        s.sessions.insert(session, agent.to_owned());
    }

    /// A desk closed. Its descendants keep their agent (their work is still
    /// the agent's).
    pub fn clear_desk(&self, agent: &str) -> Option<SessionId> {
        let mut s = self.write();
        let id = s.desks.remove(agent)?;
        s.sessions.remove(&id);
        Some(id)
    }

    /// The agent's current desk session.
    pub fn desk(&self, agent: &str) -> Option<SessionId> {
        self.read().desks.get(agent).copied()
    }

    /// A child of an agent's session is the agent's too (67 §6.3: caps and
    /// allowlists inherit). No-op when the parent is nobody's.
    pub fn inherit(&self, parent: SessionId, child: SessionId) {
        let mut s = self.write();
        if let Some(agent) = s.sessions.get(&parent).cloned() {
            s.sessions.insert(child, agent);
        }
    }

    /// The session's rows are gone.
    pub fn forget(&self, session: SessionId) {
        self.write().sessions.remove(&session);
    }

    /// Which agent `session` works for; `None` for the user's own sessions.
    pub fn agent_of(&self, session: SessionId) -> Option<String> {
        self.read().sessions.get(&session).cloned()
    }

    /// The agent's reporting line, up and down (67 §9): its managers to the
    /// root, and every agent that reports to it, transitively. Includes the
    /// agent itself. Names are bare (`ops`), plus `user` at the top.
    pub fn line(&self, agent: &str) -> BTreeSet<String> {
        let s = self.read();
        let manager = |name: &str| -> Option<String> {
            if name == AgentName::DEFAULT {
                return s.theuth.as_ref().map(|t| t.reports_to.clone());
            }
            s.files
                .get(name)
                .and_then(|r| r.as_ref().ok())
                .map(|f| f.reports_to.clone())
        };
        let mut line = BTreeSet::new();
        line.insert(agent.to_owned());
        // Up.
        let mut cur = agent.to_owned();
        while let Some(m) = manager(&cur) {
            if !line.insert(m.clone()) {
                break; // a cycle in `reports_to`; stop
            }
            cur = m;
        }
        // Down.
        let mut frontier = vec![agent.to_owned()];
        while let Some(boss) = frontier.pop() {
            for (name, f) in &s.files {
                if let Ok(f) = f
                    && f.reports_to == boss
                    && line.insert(name.clone())
                {
                    frontier.push(name.clone());
                }
            }
        }
        line
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::file::parse;

    fn shared() -> Shared {
        let s = Shared::default();
        let mut files = BTreeMap::new();
        files.insert("cto".into(), parse("cto", "Boss."));
        files.insert("ops".into(), parse("ops", "---\nreports_to: cto\n---\nOps."));
        files.insert("sre".into(), parse("sre", "---\nreports_to: ops\n---\nSre."));
        files.insert("mkt".into(), parse("mkt", "Marketing."));
        s.set_files(files, crate::file::default_agent(std::path::Path::new("/nonexistent")));
        s
    }

    #[test]
    fn line_walks_up_and_down() {
        let s = shared();
        let line: Vec<_> = s.line("ops").into_iter().collect();
        assert_eq!(line, ["cto", "ops", "sre", "theuth", "user"]);
        assert!(!s.line("mkt").contains("ops"));
        assert!(s.line("theuth").contains("mkt"), "everyone reports up to theuth");
    }

    #[test]
    fn mcp_allowlist_forms() {
        let ops = parse("ops", "---\nmcp: [grafana, argocd_prod.list_applications]\n---\nOps.").unwrap();
        assert!(mcp_visible(&ops, "grafana", "query"));
        assert!(mcp_visible(&ops, "grafana", "anything"));
        assert!(mcp_visible(&ops, "argocd_prod", "list_applications"));
        assert!(!mcp_visible(&ops, "argocd_prod", "sync_application"));
        assert!(!mcp_visible(&ops, "slack", "post"));
        let open = parse("qa", "QA.").unwrap();
        assert!(mcp_visible(&open, "slack", "post"), "no `mcp:` = no limit");
        let theuth = crate::file::default_agent(std::path::Path::new("/nonexistent"));
        assert!(mcp_visible(&theuth, "argocd_prod", "sync_application"));
        let none = parse("intern", "---\nmcp: []\n---\nIntern.").unwrap();
        assert!(!mcp_visible(&none, "grafana", "query"), "`mcp: []` = nothing visible");
    }

    #[test]
    fn mcp_access_follows_sessions_and_children() {
        use ::tools::McpAccess as _;
        let s = Shared::default();
        let mut files = BTreeMap::new();
        files.insert("ops".into(), parse("ops", "---\nmcp: [grafana]\n---\nOps."));
        s.set_files(files, crate::file::default_agent(std::path::Path::new("/nonexistent")));
        let s = std::sync::Arc::new(s);
        let desks = crate::Desks(s.clone());
        let ops_desk = SessionId::new();
        let ops_child = SessionId::new();
        let theuth_desk = SessionId::new();
        let user = SessionId::new();
        s.set_desk("ops", ops_desk);
        s.inherit(ops_desk, ops_child);
        s.set_desk("theuth", theuth_desk);
        assert!(desks.allows(ops_desk, "grafana", "query"));
        assert!(!desks.allows(ops_desk, "slack", "post"));
        assert!(!desks.allows(ops_child, "slack", "post"), "a child inherits the allowlist");
        assert!(desks.allows(theuth_desk, "slack", "post"), "theuth is unaffected");
        assert!(desks.allows(user, "slack", "post"), "a session of no agent is unaffected");
    }

    #[test]
    fn sessions_follow_the_desk() {
        let s = shared();
        let desk = SessionId(uuid::Uuid::now_v7());
        let child = SessionId(uuid::Uuid::now_v7());
        let stranger = SessionId(uuid::Uuid::now_v7());
        s.set_desk("ops", desk);
        s.inherit(desk, child);
        s.inherit(stranger, SessionId(uuid::Uuid::now_v7()));
        assert_eq!(s.desk("ops"), Some(desk));
        assert_eq!(s.agent_of(child).as_deref(), Some("ops"));
        assert_eq!(s.agent_of(stranger), None);
        assert_eq!(s.clear_desk("ops"), Some(desk));
        assert_eq!(s.agent_of(desk), None);
        assert_eq!(s.agent_of(child).as_deref(), Some("ops"));
    }

    #[test]
    fn wake_text_snapshot() {
        let task = tasks::Task {
            id: tasks::TaskId("12".into()),
            title: "Rotate the relay certificate".into(),
            brief: "The wildcard cert expires Friday.\nRenew and reload.".into(),
            owner: "agent:ops".parse().unwrap(),
            state: tasks::TaskState::Todo,
            reviewer: Some("agent:cto".parse().unwrap()),
            blockers: vec![],
            blocks: vec![],
            parent: None,
            project: Some("relay".into()),
            where_: Some("build-A".into()),
            harness: Some("omp".into()),
            external: vec!["https://jira/SRE-1".into()],
            thread: Some("doc-7".into()),
            checkout: None,
            schedule: None,
            created_by: "user".into(),
            created_ms: 0,
            updated_ms: 0,
            no_progress_count: 0,
            next_wake_after_ms: None,
        };
        let wake = tasks::Wake {
            id: "w1".into(),
            agent: "ops".into(),
            task: Some(task.id.clone()),
            reason: tasks::WakeReason::Comment,
            key: tasks::Wake::key_of("ops", Some(&task.id), tasks::WakeReason::Comment),
            count: 2,
            claimed_by: None,
            created_ms: 0,
        };
        assert_eq!(
            wake_text(&wake, Some(&task)),
            "Wake: task 12 — Rotate the relay certificate. Reason: comment. (Raised 2 times.) Do not switch to \
another task until this wake is handled.\n\n\
<task id=\"12\" state=\"todo\" owner=\"agent:ops\" reviewer=\"agent:cto\" project=\"relay\" where=\"build-A\" \
harness=\"omp\" thread=\"doc-7\">\nThe wildcard cert expires Friday.\nRenew and reload.\n\n\
External: https://jira/SRE-1\n</task>"
        );
        let manual = tasks::Wake {
            task: None,
            reason: tasks::WakeReason::Manual,
            count: 1,
            ..wake
        };
        assert_eq!(
            wake_text(&manual, None),
            "Wake: no task. Reason: manual. Do not switch to another task until this wake is handled."
        );
    }

    #[test]
    fn opening_has_charter_journal_and_disposition_rule() {
        let f = parse("ops", "---\ntitle: Ops\nmodel: m1\ntools: mutating\n---\nYou are Ops.").unwrap();
        let text = opening(&f, Some("## Done\n- x\n"));
        assert!(text.starts_with("<desk agent=\"ops\">\n<charter>\nYou are Ops.\n</charter>\n<journal>\n## Done\n- x\n</journal>\n"));
        assert!(text.contains("model m1, tools mutating"));
        assert!(text.contains("`review | blocked | done(by reviewer) | delegated (child task exists) | continuing (a schedule, reminder or live child will wake it)`"));
        assert!(text.contains("`blocked: \"no continuation\"` and the manager is woken"));
        assert!(opening(&f, None).contains("(first episode: no journal yet)"));
    }

    #[test]
    fn idle_rotation_uses_the_given_clock() {
        let mut d = Desk {
            session: SessionId::new(),
            episode: "e".into(),
            last_active_ms: 1_000_000,
            working: false,
            primed: true,
            model: None,
        };
        let idle = 21_600;
        assert!(!d.idle_due(1_000_000 + idle * 1000, idle), "exactly the limit is not over it");
        assert!(d.idle_due(1_000_000 + idle * 1000 + 1, idle));
        d.working = true;
        assert!(!d.idle_due(u64::MAX / 2, idle), "a running turn is not idle");
        d.working = false;
        d.primed = false;
        assert!(!d.idle_due(u64::MAX / 2, idle), "an empty desk has nothing to journal");
        assert!(context_due(61, 100, 60));
        assert!(!context_due(60, 100, 60));
        assert!(!context_due(1_000_000, 0, 60), "unknown window never rotates");
    }

    #[test]
    fn journal_is_written_and_capped() {
        let dir = std::env::temp_dir().join(format!("roster-journal-{}-{}", std::process::id(), uuid::Uuid::now_v7()));
        assert_eq!(read_journal(&dir, "ops"), None);
        let path = write_journal(&dir, "ops", "  ## Done\n- rotated cert\n", 8192).unwrap();
        assert_eq!(path, dir.join("roster/journal/ops.md"));
        assert_eq!(read_journal(&dir, "ops").unwrap(), "## Done\n- rotated cert\n");
        // Cap at a char boundary: "éé" is 4 bytes; a 3-byte cap keeps one char.
        write_journal(&dir, "ops", "éé", 3).unwrap();
        assert_eq!(read_journal(&dir, "ops").unwrap(), "é\n");
        let _ = std::fs::remove_dir_all(&dir);
    }
}
