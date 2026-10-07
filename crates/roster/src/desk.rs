//! What the roster knows that other plugins ask about synchronously (67 §6.2,
//! §6.3, §9): the parsed files, which session is which agent's desk, and which
//! agent a session belongs to. Shared between the [`crate::Roster`] actor (the
//! writer) and [`crate::Desks`], the object registered with the tasks, tools
//! and knowledge plugins (the readers). Plus the desk's texts (operating
//! manual, opening block, closing prompt, scoped wake) and the episode
//! bookkeeping the actor drives (67 §6.2, §7.3).

use std::collections::{BTreeMap, BTreeSet, HashMap, HashSet};
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
                || m.split_once('.')
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

/// A harness input to a desk as a `<kind>` notice (the shape `agent::loop_::note`
/// uses): the desktop renders it as the nudge card, hidden unless "Show theuth
/// nudges" is on, never as the user's words. `kind` is `nudge` or `journal`.
pub fn notice(kind: &str, text: &str) -> String {
    format!("<{kind}>\n{text}\n</{kind}>")
}

/// The desk's operating manual (67 §6.2, §7.3; 70 DESK-PROTOCOL): the part
/// of the identity block after the charter and the journal. `max_rounds` is
/// `roster.max_rounds`.
pub fn manual(f: &AgentFile, max_rounds: u32) -> String {
    let name = f.name.as_str();
    let model = f.model.as_deref().unwrap_or("the default");
    let tools = f.tools.as_deref().unwrap_or("destructive");
    format!(
        "You are the desk of agent `{name}` ({title}): an employee finishing a job, not a chat bot relaying \
output. You take wakes, read your inbox with `task_inbox()`, and handle the task the wake names before any other. \
Never do the work in this session: no files, no shells, no browsing here, only through a child session.\n\
\n\
1. Fit check first. On an `assigned` or `reassigned` wake, before any work, read the roster the wake carries \
(active agents with their capabilities). If another agent's capabilities match the task better than your \
charter, hand it over at once with `task_reassign {{task, to, why}}`: the thread records `reassigned to ‹agent›: \
‹why›` and they are woken. `theuth`'s desk is the router by default — most user tasks land on it, and routing IS \
its job. Only when nobody fits do you work the task yourself: `task_checkout()`, then `spawn_subagent` exactly where \
the task says (`agent: \"<harness>@<engine>\"` from the task's `harness` and `where`, or the engine name alone for \
the native harness; pass `model` and `tools` from your file: model {model}, tools {tools}) with a complete brief. \
A role missing from the roster is a proposal, not a hire: `agent_propose()` (it lands paused and files a \
`hire_agent` approval up your line); the user decides.\n\
\n\
2. Follow through. A child's result arrives in this same session as a `<message>`; your episode does not end \
there. (i) Read it. (ii) Decide whether the task's `done_when` (its acceptance criteria) is met — if the findings \
raise follow-up questions, spawn further children (at most {max_rounds} rounds per task in one episode) or file \
child tasks with `task_create {{parent}}`. (iii) Write the deliverable into the thread with `task_note()`: a \
report block — summary, findings, evidence, recommended actions — in your own words, having verified the child's \
claims. (iv) Set the disposition with `task_update()`. Write progress with `task_note progress: true` at natural \
boundaries; call `escalate()` when you need a decision you may not make yourself, and wait for the answer. \
Before escalating, call `task_waiting_on_user()`: a task already waiting on the user does not get a second \
escalation from you — stuck is the bar; a follow-up earns a new one only with a new option, a fact that changes \
the decision, or a deadline; otherwise `task_note` on the waiting one and go on with work the answer does not gate. \
Escalating to the user needs a packet: a `question` whose first sentence is the thing the user has to say back \
(at most 15 words) with `options` that answer it in the same words — write the question last, off the options; \
`if_wrong`, one honest line on the cost of a wrong answer (\"low, one-line revert\" / \"high, real users get this \
email\"); a `summary` of at most 120 words answering what is being decided and why \
now; at most 4 `options` unless it is a bundle, one line each, exactly one `recommended`; and a \
`board` — a decision mini-board (`board` skill, `doc_create(kind=\"board\")`) with `## What you're deciding` · \
`## Why it matters to you` · `## The picture` · `## Options` · `## Look at`, written for the person deciding: what, \
why it matters to them, the whole picture; diagram and link heavy; short — the tool refuses without. A report's \
decision that only the user can make is forwarded, not re-typed: `escalate` the same task with `question: \"\"` \
and your `recommendation {{text, option}}` — their question, summary, board, options and `if_wrong` are inherited. \
N related decisions (records, hosts, PRs) are one `escalate` with `items`, one `###` per item under the board's \
`## Options`.\n\
\n\
3. Delegation and waiting. Long or parallel work is a child task (`task_create()` with `parent` and your task's \
id in `blockers`, or `disposition: delegated {{to}}`). You are woken by `blockers_resolved` when what you wait on \
is done and by `children_completed` when every child is closed — never poll, never busy-wait. Talking sideways \
to another agent is creating a self-contained task for them. `blocked` names what you wait on (a task id, \
`decision:<id>`, `person:<name>`); prose is not a waiting path.\n\
\n\
4. Disposition checklist — every turn woken for a task ends with the task moved: `done` (reviewer only, with \
`done_because`), `review {{reviewer}}`, `blocked {{on}}`, `delegated {{to}}` or `continuing {{by}}` (something \
that will actually wake you: a schedule, a reminder, a live child). A task is done when (1) it meets its \
`done_when`, or (2) it meets the user's evident intention, or (3) you filed the follow-up tasks that naturally \
follow it — judge in that order and say which. When you hand over or finish, retitle the task (`task_update \
title`) to state the outcome and lead the `summary` with it; options that can no longer be chosen come out. \
Handing work to the user's review needs the review packet: a \
one-paragraph `summary`, `checks` against `done_when`, and a `board` — a review mini-board (`board` skill, \
`doc_create(kind=\"board\")`) with `## What was done` · `## Why it matters to you` · `## The picture` · \
`## Against the criteria` · `## Walk through`, written for the person reviewing: what, why it matters to them, \
the whole picture; diagram and link heavy; short — the tool refuses without. Show what it does: `## Walk through` \
opens with a demonstration (an image, a page, a URL, real input and output) when one can exist, diffs second. \
If your run changed anything there is something to decide: the review's `options` are the next moves after \
approval (e.g. \"Merge and continue to X\" (recommended) / \"Revise Y first\" / \"Park it\"; at most 4) — never \
what the run already did; a question answered or a thing read ends with no options. Assigning yourself plus a \
please-review comment is not a review \
path: the reviewer is a different desk or the user; a desk that is its own reviewer marks done only against an \
explicit `done_when`, as `criteria_met`; without one the review goes to your manager. On a `review` wake you \
end the turn with `task_move done` (with `done_because`) or send it back to the owner with notes. On a \
`comment` wake you reply in the thread. A wake that ends with its task unmoved gets ONE nudge; if the task is \
still unmoved after that it is set `blocked: \"no continuation\"` and your manager is woken — the same rule \
that closes an episode: every task you checked out must be in `review | blocked | done(by reviewer) | \
delegated (child task exists) | continuing` when the episode closes.\n\
\n\
Tasks in `backlog` are not dispatched: they are not in your inbox and you do not `task_move()` them to `todo` — \
the user starts them. Your `knowledge_put()` writes are private to you by default; `knowledge_share()` opens them \
to your team or everyone. When asked to write your journal, answer with the journal only.\n\
\n\
End every turn whose text the user will read (not the journal) with a `## Next` section: at most two one-line \
suggested replies the user could send, as a `- ` list, each under 80 characters. Nothing else goes under that heading.",
        title = f.title,
    )
}

/// 70 DESK-PROTOCOL (e): the capabilities roster an `assigned` wake carries so
/// the fit check is one glance — every other active agent (not paused, not a
/// proposal), one line each: name, title, capabilities.
pub fn roster_block<'a>(me: &str, agents: impl Iterator<Item = &'a AgentFile>) -> String {
    let mut lines: Vec<String> = agents
        .filter(|f| f.name.as_str() != me && !f.paused && f.proposed_by.is_none())
        .map(|f| {
            let caps = f
                .capabilities
                .as_deref()
                .map(str::trim)
                .filter(|c| !c.is_empty())
                .unwrap_or("(no capabilities stated)");
            format!(
                "- `{}` ({}): {}",
                f.name,
                f.title,
                caps.lines().collect::<Vec<_>>().join(" ")
            )
        })
        .collect();
    if lines.is_empty() {
        lines.push("- (no other active agent)".into());
    }
    format!(
        "<roster>\nFit check: hand the task over with `task_reassign()` if one of these fits it better than your \
charter; otherwise work it.\n{}\n</roster>",
        lines.join("\n")
    )
}

/// The desk session's identity block (70 DESK-PROTOCOL; 67 §6.2 "the next
/// episode starts from the charter + journal"): leads the system prompt in
/// theuth's place — who the agent is, whom it reports to, its charter,
/// capabilities, journal and the manual. For `theuth` the charter is
/// `prompt.md` (67 decision 1), so it never appears for anyone else.
pub fn identity(f: &AgentFile, journal: Option<&str>, max_rounds: u32) -> String {
    let journal = journal
        .map(str::trim)
        .filter(|j| !j.is_empty())
        .unwrap_or("(first episode: no journal yet)");
    let mut s = format!(
        "You are {title} (`{name}`), an agent on this theuth roster; you report to {boss}. Your charter:\n{charter}",
        title = f.title,
        name = f.name,
        boss = f.reports_to,
        charter = f.body.trim(),
    );
    if let Some(c) = f
        .capabilities
        .as_deref()
        .map(str::trim)
        .filter(|c| !c.is_empty())
    {
        s.push_str(&format!(
            "\n\nCapabilities (when you are the one to hand a task to): {c}"
        ));
    }
    if let Some(vm) = crate::pin::record_name(f.name.as_str(), &f.runs_in) {
        s.push_str(&format!(
            "\n\nRuns in: sandbox VM `{vm}` (your file's `runs_in`). Every work child of yours runs on that VM's \
             engine, never here or on another engine: `task_place` resolves it into the task's `where` (the task \
             waits `blocked: sandbox:{vm}` while the VM is not ready — do nothing until the wake), and \
             `spawn_subagent` then takes `agent: \"<where>\"` or `\"<harness>@<where>\"` from the task."
        ));
    }
    s.push_str(&format!(
        "\n\n<journal>\n{journal}\n</journal>\n\n{}",
        manual(f, max_rounds)
    ));
    s
}

/// The identity block of a session a desk spawned for a task (70
/// DESK-PROTOCOL): it works for the agent, not as theuth; `task` is the
/// desk's checked-out task when one is known.
pub fn child_identity(f: &AgentFile, task: Option<&tasks::Task>) -> String {
    let mut s = format!(
        "You are working for {title} (`{name}`), an agent on this theuth roster",
        title = f.title,
        name = f.name,
    );
    match task {
        Some(t) => s.push_str(&format!(", on task {} — {}.", t.id, t.title.trim())),
        None => s.push('.'),
    }
    s.push_str(
        " Its desk briefed you in your first message and reads your final message as your report: do the work \
         it asked for, state what you verified and what you did not, and stop.",
    );
    s
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
        if let Some(w) = t
            .done_when
            .as_deref()
            .map(str::trim)
            .filter(|w| !w.is_empty())
        {
            s.push_str(&format!("\n\nDone when: {w}"));
        } else {
            s.push_str("\n\nDone when: (no acceptance criteria stated — judge on the user's evident intention; set them with `task_update done_when` once you know them)");
        }
        if !t.blockers.is_empty() {
            s.push_str(&format!("\n\nBlockers: {}", t.blockers.join("; ")));
        }
        if !t.external.is_empty() {
            s.push_str(&format!("\n\nExternal: {}", t.external.join(", ")));
        }
        if !t.attachments.is_empty() {
            s.push_str("\n\nAttached (read with the tool for the kind, see Attachments):");
            for r in &t.attachments {
                s.push('\n');
                s.push_str(&agent::attach::marker(r));
            }
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
pub fn write_journal(
    data_dir: &Path,
    name: &str,
    text: &str,
    max_bytes: usize,
) -> std::io::Result<PathBuf> {
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
    /// The wake the desk is handling (70 DESK-PROTOCOL 3): set at delivery,
    /// judged at every turn end by [`nudges::TABLE`], cleared when handled.
    pub serving: Option<Serving>,
}

/// What a desk was woken for, as it stood at delivery (70 DESK-PROTOCOL 3).
#[derive(Clone, Debug)]
pub struct Serving {
    /// The wake.
    pub wake: tasks::Wake,
    /// The task's state at delivery (`None`: no task or unknown).
    pub state: Option<tasks::TaskState>,
    /// Its owner at delivery.
    pub owner: Option<tasks::Owner>,
    /// `task_events` rows at delivery: anything after them is this wake's work.
    pub events: usize,
    /// The one nudge was sent.
    pub nudged: bool,
    /// Children spawned for it this episode (70 DESK-PROTOCOL 2: bounded by `roster.max_rounds`).
    pub rounds: u32,
    /// The direct children spawned under the desk while this wake was served
    /// (WAKE-LIVE-CHILD D1): only these count as "not started yet" when cold.
    pub children: HashSet<SessionId>,
    /// The "a child is still running" deferral was logged for this wake.
    pub logged_live_child: bool,
}

/// 70 DESK-PROTOCOL 3: the nudge points, in one table so Sky can rule on
/// more. A desk turn woken for a task that ends with the task unmoved gets
/// ONE nudge (fixed options, paperclip's successful-run-handoff shape) and the
/// turn is re-run; a second unmoved end applies the 67 §7.3 rule
/// (`blocked: no continuation`, manager woken). Every nudge is logged in the
/// thread as `nudged: …`.
pub mod nudges {
    use tasks::{Task, TaskEvent, TaskState, WakeReason};

    use super::Serving;

    /// What the desk sees after the nudged turn if the task is still unmoved.
    pub const THEN: &str = "One attempt: if the task is unchanged when this turn ends it is set `blocked: \"no \
continuation\"` and your manager is woken.";

    /// One row.
    pub enum Nudge {
        /// Judged when a desk turn ends.
        TurnEnd {
            /// Row name (the thread's `nudged: <name>` line).
            name: &'static str,
            /// Wake reasons it covers.
            reasons: &'static [WakeReason],
            /// Still unhandled? `(at delivery, task now, events since delivery by
            /// the desk's agent, a child of the desk is still running)`.
            unhandled: fn(&Serving, &Task, &[TaskEvent], bool) -> bool,
            /// The nudge input.
            text: fn(&Task) -> String,
        },
        /// Judged by the clock, in the tasks ledger; listed here for the ruling.
        Time {
            /// Row name.
            name: &'static str,
            /// Where and how it is enforced.
            rule: &'static str,
        },
    }

    /// A disposition event after delivery that names a real continuation.
    fn continued(events: &[TaskEvent]) -> bool {
        events.iter().any(|e| {
            e.field == "disposition"
                && e.after
                    .as_deref()
                    .is_some_and(|a| a.starts_with("continuing") || a.starts_with("delegated"))
        })
    }

    /// The task left the state it was delivered in, or changed hands.
    fn moved(s: &Serving, t: &Task) -> bool {
        Some(t.state) != s.state || s.owner.as_ref().is_some_and(|o| *o != t.owner)
    }

    /// The rows. Turn-end rows are matched by wake reason ([`for_reason`]).
    pub const TABLE: &[Nudge] = &[
        // (a) child result / assigned: a successful turn with no disposition.
        Nudge::TurnEnd {
            name: "disposition",
            reasons: &[
                WakeReason::Assigned,
                WakeReason::Reassigned,
                WakeReason::Manual,
                WakeReason::Schedule,
                WakeReason::Resume,
                WakeReason::BlockersResolved,
                WakeReason::ChildrenCompleted,
            ],
            unhandled: |s, t, events, live_child| match t.state {
                // Not even checked out (or reassigned).
                TaskState::Todo => !moved(s, t),
                // Checked out, no live child, nothing that will wake the desk.
                TaskState::Doing => !live_child && !continued(events),
                _ => false,
            },
            text: |t| {
                format!(
                    "Task {} needs a disposition before this turn can end. Move it — exactly one of: `task_update \
disposition: done` with `done_because` (reviewers only) · `disposition: review {{reviewer}}` (a different desk or \
the user) · `disposition: blocked {{on}}` with what you wait on · `disposition: continuing {{by}}` / `delegated \
{{to}}` (something that will wake you next). If it is still `todo`, either `task_reassign()` it to the agent whose \
capabilities fit or `task_checkout()` and start the child now. Write the deliverable (`task_note()` report: summary, \
findings, evidence, recommended actions) before you dispose. {THEN}",
                    t.id
                )
            },
        },
        // Review wake: the turn must end in a transition.
        Nudge::TurnEnd {
            name: "review",
            reasons: &[WakeReason::Review],
            unhandled: |_, t, _, _| t.state == TaskState::Review,
            text: |t| {
                format!(
                    "Task {} is still in `review`. Move it — one of: `task_move done` with `done_because` \
(criteria_met against its done_when · intent_met · follow_ups_filed) · `task_move todo` / `task_update state: \
doing` back to the owner with your notes in the thread · `task_update disposition: blocked {{on}}` with a reason. \
Your opinion alone is not a review: the transition is. {THEN}",
                    t.id
                )
            },
        },
        // Comment / mention / answer: the thread wants a reply.
        Nudge::TurnEnd {
            name: "reply",
            reasons: &[WakeReason::Comment, WakeReason::Mention, WakeReason::Answer],
            unhandled: |s, t, events, _| events.is_empty() && !moved(s, t),
            text: |t| {
                format!(
                    "The thread of task {} got no reply from you. Answer it with `task_note()` (or move the task if \
the comment changes its disposition). {THEN}",
                    t.id
                )
            },
        },
        // (b) time-based rows, enforced in crates/tasks.
        Nudge::Time {
            name: "no-progress back-off",
            rule: "tasks::ledger::complete_wake — a wake the owner's next write leaves unchanged counts; from the \
second in a row the task's wakes are held 2 min, doubling to 30 min (tasks.backoff_min_secs / backoff_max_secs); a \
user comment clears it.",
        },
        Nudge::Time {
            name: "stale checkout",
            rule: "tasks::ledger::sweep — a `doing` checkout on a lost engine, or with no progress note and no \
session activity (own turn, job, PTY or any descendant) for tasks.stale_secs (15 min), is orphaned; a desk idle \
between its children's results is not. Its epoch is bumped and the owner is woken `resume` once; a task that stays \
orphaned is re-placed by task_place.",
        },
    ];

    /// The turn-end row for a wake reason.
    pub fn for_reason(reason: WakeReason) -> Option<&'static Nudge> {
        TABLE
            .iter()
            .find(|n| matches!(n, Nudge::TurnEnd { reasons, .. } if reasons.contains(&reason)))
    }

    /// A child of the desk whose result is still to come (WAKE-LIVE-CHILD D1):
    /// running now (whichever turn spawned it), or spawned while `s` was
    /// served and not finished. A descendant that is cold and was not spawned
    /// for this wake never counts — after a restart every pre-restart child
    /// is `Idle` with `finished_at: None`, and that must not park the wake.
    pub fn live_child(s: &Serving, kids: &[agent::AgentInfo]) -> bool {
        kids.iter().any(|k| {
            matches!(
                k.state,
                agent::AgentState::Running
                    | agent::AgentState::Awaiting { .. }
                    | agent::AgentState::Approval { .. }
            ) || (s.children.contains(&k.id) && k.finished_at.is_none())
        })
    }

    /// The turn ended by the model's own hand (`agent::AgentEvent::TurnFinished::stop_reason`
    /// is a provider stop such as `end_turn`). A user cancel or the engine
    /// shutting down (`cancelled`), a timeout (`timed_out`) or a provider
    /// error (`error`) is not a yield: nothing is judged, the task keeps its
    /// checkout and the wake is released for re-delivery (WAKE-LIVE-CHILD D2).
    pub fn yielded(stop_reason: &str) -> bool {
        !matches!(stop_reason, "cancelled" | "timed_out" | "error")
    }
}

impl Desk {
    /// Idle > `idle_secs` since the last activity, with something in the
    /// transcript, no turn running and no child or job at work
    /// (`live_work`, WAKE-LIVE-CHILD D5) (67 §6.2 "idle 6 h").
    pub fn idle_due(&self, now_ms: u64, idle_secs: u64, live_work: bool) -> bool {
        self.primed
            && !self.working
            && !live_work
            && now_ms.saturating_sub(self.last_active_ms) > idle_secs * 1000
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

    /// The agent's desk session is retired (superseded for context) or gone.
    /// A mere episode close does not call this: the mapping survives it, so
    /// the row is reused (STALE-WAKE-DESKS). Descendants keep their agent
    /// (their work is still the agent's).
    pub fn clear_desk(&self, agent: &str) -> Option<SessionId> {
        self.write().desks.remove(agent)
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
        files.insert(
            "ops".into(),
            parse("ops", "---\nreports_to: cto\n---\nOps."),
        );
        files.insert(
            "sre".into(),
            parse("sre", "---\nreports_to: ops\n---\nSre."),
        );
        files.insert("mkt".into(), parse("mkt", "Marketing."));
        s.set_files(
            files,
            crate::file::default_agent(std::path::Path::new("/nonexistent")),
        );
        s
    }

    #[test]
    fn line_walks_up_and_down() {
        let s = shared();
        let line: Vec<_> = s.line("ops").into_iter().collect();
        assert_eq!(line, ["cto", "ops", "sre", "theuth", "user"]);
        assert!(!s.line("mkt").contains("ops"));
        assert!(
            s.line("theuth").contains("mkt"),
            "everyone reports up to theuth"
        );
    }

    #[test]
    fn mcp_allowlist_forms() {
        let ops = parse(
            "ops",
            "---\nmcp: [grafana, argocd_prod.list_applications]\n---\nOps.",
        )
        .unwrap();
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
        assert!(
            !mcp_visible(&none, "grafana", "query"),
            "`mcp: []` = nothing visible"
        );
    }

    #[test]
    fn mcp_access_follows_sessions_and_children() {
        use ::tools::McpAccess as _;
        let s = Shared::default();
        let mut files = BTreeMap::new();
        files.insert("ops".into(), parse("ops", "---\nmcp: [grafana]\n---\nOps."));
        s.set_files(
            files,
            crate::file::default_agent(std::path::Path::new("/nonexistent")),
        );
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
        assert!(
            !desks.allows(ops_child, "slack", "post"),
            "a child inherits the allowlist"
        );
        assert!(
            desks.allows(theuth_desk, "slack", "post"),
            "theuth is unaffected"
        );
        assert!(
            desks.allows(user, "slack", "post"),
            "a session of no agent is unaffected"
        );
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
        assert_eq!(
            s.agent_of(desk).as_deref(),
            Some("ops"),
            "the closing turn is still the agent's"
        );
        s.forget(desk);
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
            done_when: Some("curl shows the new expiry".into()),
            attachments: vec![agent::attach::AttachRef {
                kind: agent::attach::Kind::Note,
                id: "agqmlbgmk57ybm2rdgyj7kcery".into(),
                title: "Cert runbook".into(),
                meta: "updated Sep 24".into(),
            }],
            priority: tasks::Priority::Medium,
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
Done when: curl shows the new expiry\n\n\
External: https://jira/SRE-1\n\n\
Attached (read with the tool for the kind, see Attachments):\n\
[attached note agqmlbgmk57ybm2rdgyj7kcery \"Cert runbook\" · updated Sep 24]\n</task>"
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
    fn identity_leads_with_the_agent_and_has_the_disposition_rule() {
        let f = parse(
            "karol",
            "---\ntitle: Infrastructure\nmodel: m1\ntools: mutating\ncapabilities: k8s, networks\n---\nYou run the clusters.",
        )
        .unwrap();
        let text = identity(&f, Some("## Done\n- x\n"), 3);
        assert!(text.starts_with(
            "You are Infrastructure (`karol`), an agent on this theuth roster; you report to theuth. Your charter:\n\
             You run the clusters.\n\nCapabilities (when you are the one to hand a task to): k8s, networks\n\n\
             <journal>\n## Done\n- x\n</journal>\n\nYou are the desk of agent `karol`"
        ), "{text}");
        assert!(text.contains("model m1, tools mutating"));
        assert!(text.contains("at most 3 rounds per task"));
        assert!(text.contains("`task_reassign {task, to, why}`"));
        assert!(
            text.contains("Assigning yourself plus a please-review comment is not a review path")
        );
        assert!(
            text.contains("`blockers_resolved`")
                && text.contains("`children_completed`")
                && text.contains("never poll")
        );
        assert!(text.contains(
            "`review | blocked | done(by reviewer) | delegated (child task exists) | continuing`"
        ));
        assert!(text.contains("`blocked: \"no continuation\"` and your manager is woken"));
        assert!(!text.contains("You are theuth"));
        assert!(identity(&f, None, 3).contains("(first episode: no journal yet)"));
        let child = child_identity(&f, None);
        assert!(child.starts_with(
            "You are working for Infrastructure (`karol`), an agent on this theuth roster. Its desk"
        ));
    }

    /// 79 §4.9: a pinned agent's desk is told its children run on the VM's
    /// engine; an unpinned one has no such line.
    #[test]
    fn identity_names_the_pin() {
        let unpinned = parse("ops", "---\ntitle: Ops\n---\nx").unwrap();
        assert!(!identity(&unpinned, None, 3).contains("Runs in:"));
        let own = parse("ops", "---\ntitle: Ops\nruns_in: vm\n---\nx").unwrap();
        let text = identity(&own, None, 3);
        assert!(
            text.contains("Runs in: sandbox VM `vm-ops`")
                && text.contains("`blocked: sandbox:vm-ops`"),
            "{text}"
        );
        let named = parse("ops", "---\ntitle: Ops\nruns_in: vm:lab\n---\nx").unwrap();
        assert!(identity(&named, None, 3).contains("Runs in: sandbox VM `lab`"));
    }

    /// 70 DESK-PROTOCOL (e): the fit-check roster lists every other active agent with capabilities.
    #[test]
    fn roster_block_lists_active_others_with_capabilities() {
        let karol = parse(
            "karol",
            "---\ntitle: Infrastructure\ncapabilities: k8s clusters,\n  networks\n---\nK.",
        )
        .unwrap();
        let paused = parse("mkt", "---\npaused: true\ncapabilities: ads\n---\nM.").unwrap();
        let plain = parse("ops", "Ops.").unwrap();
        let files = [karol.clone(), paused, plain.clone()];
        let block = roster_block("theuth", files.iter());
        assert!(
            block.contains("- `karol` (Infrastructure): k8s clusters, networks"),
            "{block}"
        );
        assert!(
            block.contains("- `ops` (ops): (no capabilities stated)"),
            "{block}"
        );
        assert!(
            !block.contains("mkt"),
            "paused agents are not offered: {block}"
        );
        assert!(block.contains("`task_reassign()`"));
        assert!(roster_block("karol", [karol].iter()).contains("(no other active agent)"));
    }

    /// A user's stop (or a shutdown, timeout, provider error) is not a model yield: nothing is judged.
    #[test]
    fn interrupted_turns_are_not_yields() {
        use super::nudges::yielded;
        for natural in ["end_turn", "stop", "STOP", "max_tokens", "length", ""] {
            assert!(yielded(natural), "{natural}");
        }
        for interrupted in ["cancelled", "timed_out", "error"] {
            assert!(!yielded(interrupted), "{interrupted}");
        }
    }

    /// 70 DESK-PROTOCOL 3: the table's rows judge a turn end — an assigned wake
    /// left in `todo` or in `doing` with nothing to wait on is unhandled; a
    /// live child or a continuation is not; a review wake needs the transition;
    /// a comment wake needs a reply.
    #[test]
    fn nudge_table_judges_turn_ends() {
        use tasks::{TaskEvent, TaskState as S, WakeReason as R};
        let mut t = tasks::Task {
            id: tasks::TaskId("12".into()),
            title: "Check cluster health".into(),
            brief: String::new(),
            owner: "agent:karol".parse().unwrap(),
            state: S::Todo,
            reviewer: None,
            blockers: vec![],
            blocks: vec![],
            parent: None,
            project: None,
            where_: None,
            harness: None,
            external: vec![],
            thread: None,
            checkout: None,
            schedule: None,
            created_by: "user".into(),
            created_ms: 0,
            updated_ms: 0,
            no_progress_count: 0,
            next_wake_after_ms: None,
            done_when: None,
            attachments: vec![],
            priority: tasks::Priority::Medium,
        };
        let id = t.id.clone();
        let wake = move |reason| tasks::Wake {
            id: "w".into(),
            agent: "karol".into(),
            task: Some(id.clone()),
            reason,
            key: String::new(),
            count: 1,
            claimed_by: None,
            created_ms: 0,
        };
        let serving = |reason, state, owner: &str| Serving {
            wake: wake(reason),
            state: Some(state),
            owner: Some(owner.parse().unwrap()),
            events: 0,
            nudged: false,
            rounds: 0,
            children: HashSet::new(),
            logged_live_child: false,
        };
        let ev = |field: &str, after: &str| TaskEvent {
            at_ms: 0,
            actor: "agent:karol".into(),
            session: None,
            field: field.into(),
            before: None,
            after: Some(after.into()),
        };
        let judge = |s: &Serving, t: &tasks::Task, events: &[TaskEvent], live: bool| {
            match nudges::for_reason(s.wake.reason) {
                Some(nudges::Nudge::TurnEnd {
                    unhandled, text, ..
                }) => (unhandled(s, t, events, live), text(t)),
                _ => panic!("no row for {:?}", s.wake.reason),
            }
        };

        // assigned: still todo → nudge names the four options
        let s = serving(R::Assigned, S::Todo, "agent:karol");
        let (u, text) = judge(&s, &t, &[], false);
        assert!(u);
        for opt in [
            "disposition: done",
            "disposition: review {reviewer}",
            "disposition: blocked {on}",
            "disposition: continuing {by}",
        ] {
            assert!(text.contains(opt), "{text}");
        }
        assert!(text.contains(nudges::THEN));
        // reassigned → handled
        t.owner = "agent:ops".parse().unwrap();
        assert!(!judge(&s, &t, &[], false).0);
        t.owner = "agent:karol".parse().unwrap();
        // checked out, child running → handled; child gone, nothing filed → unhandled
        t.state = S::Doing;
        assert!(!judge(&s, &t, &[], true).0);
        assert!(
            judge(&s, &t, &[], false).0,
            "child returned, no disposition: the follow-through nudge"
        );
        assert!(!judge(&s, &t, &[ev("disposition", "delegated:77")], false).0);
        assert!(!judge(&s, &t, &[ev("disposition", "continuing:nightly")], false).0);
        // a disposition moved it → handled
        t.state = S::Review;
        assert!(!judge(&s, &t, &[], false).0);

        // review wake: only the transition counts
        let s = serving(R::Review, S::Review, "agent:ops");
        assert!(
            judge(&s, &t, &[ev("comment", "looks fine to me")], false).0,
            "an opinion is not a review"
        );
        t.state = S::Done;
        assert!(!judge(&s, &t, &[], false).0);

        // comment wake: any event by the agent is a reply
        t.state = S::Doing;
        let s = serving(R::Comment, S::Doing, "agent:karol");
        assert!(judge(&s, &t, &[], false).0);
        assert!(!judge(&s, &t, &[ev("comment", "answered")], false).0);

        // the time rows exist for the ruling
        assert!(
            nudges::TABLE
                .iter()
                .filter(|n| matches!(n, nudges::Nudge::Time { .. }))
                .count()
                >= 2
        );
    }

    /// WAKE-LIVE-CHILD D1: a cold descendant that was not spawned for this
    /// wake is not a live child — after a restart every pre-restart child
    /// is `Idle` with `finished_at: None`, and that must not park the wake.
    #[test]
    fn cold_children_are_not_live() {
        use agent::AgentState as A;
        let kid = |id, state, finished: bool| agent::AgentInfo {
            id,
            name: "kid".into(),
            parent: SessionId::new(),
            depth: 1,
            agent: None,
            model: String::new(),
            state,
            started_at: None,
            finished_at: finished.then(chrono::Utc::now),
            last: String::new(),
            project: None,
            retention: Default::default(),
            activity: Default::default(),
            cap: None,
            kind: Default::default(),
        };
        let mine = SessionId::new();
        let other = SessionId::new();
        let s = Serving {
            wake: tasks::Wake {
                id: "w".into(),
                agent: "theuth".into(),
                task: None,
                reason: tasks::WakeReason::Mention,
                key: String::new(),
                count: 1,
                claimed_by: None,
                created_ms: 0,
            },
            state: None,
            owner: None,
            events: 0,
            nudged: false,
            rounds: 0,
            children: HashSet::from([mine]),
            logged_live_child: false,
        };
        // (a) the restart case: cold, never seen finishing, not ours
        assert!(!nudges::live_child(&s, &[kid(other, A::Idle, false)]));
        // (b) spawned for this wake, not started yet
        assert!(nudges::live_child(&s, &[kid(mine, A::Idle, false)]));
        // (c) running now, whoever spawned it
        assert!(nudges::live_child(&s, &[kid(other, A::Running, false)]));
        assert!(nudges::live_child(
            &s,
            &[kid(
                other,
                A::Awaiting {
                    from: "parent".into()
                },
                false
            )]
        ));
        // (d) ours and finished
        assert!(!nudges::live_child(&s, &[kid(mine, A::Idle, true)]));
        // (e) failed or cancelled strangers
        assert!(!nudges::live_child(
            &s,
            &[kid(other, A::Error, false), kid(other, A::Cancelled, false)]
        ));
        assert!(!nudges::live_child(&s, &[]));
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
            serving: None,
        };
        let idle = 21_600;
        assert!(
            !d.idle_due(1_000_000 + idle * 1000, idle, false),
            "exactly the limit is not over it"
        );
        assert!(d.idle_due(1_000_000 + idle * 1000 + 1, idle, false));
        d.working = true;
        assert!(
            !d.idle_due(u64::MAX / 2, idle, false),
            "a running turn is not idle"
        );
        d.working = false;
        d.primed = false;
        assert!(
            !d.idle_due(u64::MAX / 2, idle, false),
            "an empty desk has nothing to journal"
        );
        assert!(context_due(61, 100, 60));
        assert!(!context_due(60, 100, 60));
        assert!(
            !context_due(1_000_000, 0, 60),
            "unknown window never rotates"
        );
    }

    /// WAKE-LIVE-CHILD D5: a desk waiting on a running child or job is not idle.
    #[test]
    fn desk_with_running_child_is_not_idle() {
        let d = Desk {
            session: SessionId::new(),
            episode: "e".into(),
            last_active_ms: 0,
            working: false,
            primed: true,
            model: None,
            serving: None,
        };
        assert!(d.idle_due(u64::MAX / 2, 1, false));
        assert!(!d.idle_due(u64::MAX / 2, 1, true));
    }

    #[test]
    fn journal_is_written_and_capped() {
        let dir = std::env::temp_dir().join(format!(
            "roster-journal-{}-{}",
            std::process::id(),
            uuid::Uuid::now_v7()
        ));
        assert_eq!(read_journal(&dir, "ops"), None);
        let path = write_journal(&dir, "ops", "  ## Done\n- rotated cert\n", 8192).unwrap();
        assert_eq!(path, dir.join("roster/journal/ops.md"));
        assert_eq!(
            read_journal(&dir, "ops").unwrap(),
            "## Done\n- rotated cert\n"
        );
        // Cap at a char boundary: "éé" is 4 bytes; a 3-byte cap keeps one char.
        write_journal(&dir, "ops", "éé", 3).unwrap();
        assert_eq!(read_journal(&dir, "ops").unwrap(), "é\n");
        let _ = std::fs::remove_dir_all(&dir);
    }
}
