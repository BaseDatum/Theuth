//! Design doc 109 §7.3 (W4): the delta handoff an ACP child that could not be
//! restored with `session/load` is prompted with, and the short note a child
//! that *was* restored gets instead. [`render`] is the pure half (budget,
//! tail-first truncation, empty sections omitted); [`compose_handoff`]
//! gathers the sections from theuth's records and two `git` reads in the
//! child's `cwd`.

use std::path::Path;
use std::time::Duration;

use chrono::{DateTime, Local, Utc};
use runtime::SessionId;

use crate::communication::PeerMessageState;
use crate::store::Store;
use crate::todo::Status;

/// Each `git` read in the child's cwd gives up after this long (§7.3).
const GIT_TIMEOUT: Duration = Duration::from_secs(5);
/// Queued-message lookback: the child's newest exchanges (`peer_records` page).
const QUEUED_LOOKBACK: u16 = 32;
/// Room kept for the "omitted for budget" line when sections do not fit.
const OMITTED_RESERVE: usize = 120;
/// Estimated chars per token (§7.3: `handoff_tokens` × 4).
const CHARS_PER_TOKEN: usize = 4;
/// What the boot pass failed a steer with (`supervisor::start`'s
/// `fail_interrupted` reason): such a message never reached the child.
const RESTART_REASON: &str = "interrupted_by_restart";

/// The sections of one handoff, before rendering. Empty strings and empty
/// lists are omitted by [`render`].
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Handoff {
    /// The child session (the `session_read` pointer).
    pub session: String,
    /// When the restart happened; rendered local `HH:MM:SS`.
    pub at: DateTime<Utc>,
    /// The child's working directory, when the row has one.
    pub cwd: Option<String>,
    /// Assistant text of the child's last turn (what it reported last).
    pub last_report: String,
    /// `git status --porcelain` in `cwd`.
    pub status: String,
    /// `git diff --stat HEAD` in `cwd`.
    pub diff: String,
    /// Messages sent to the child it never consumed, oldest first.
    pub queued: Vec<String>,
    /// Todo items not done/dropped, as `[status] text`.
    pub todos: Vec<String>,
    /// 146 D11: the child's original brief (its first user message), excerpted.
    pub brief: String,
    /// 146 D11: the prompt it was interrupted in (its last user message), excerpted.
    pub interrupted: String,
    /// 146 D11: the harness session id that could not be loaded, when known.
    pub previous: Option<String>,
}

/// Local `HH:MM:SS`, as §4.3's examples read.
fn hhmmss(t: DateTime<Utc>) -> String {
    t.with_timezone(&Local).format("%H:%M:%S").to_string()
}

/// Chars of the brief / interrupted-prompt excerpts in a handoff (146 D10/D11).
pub const EXCERPT_CHARS: usize = 400;

/// The first input of a child whose `session/load` succeeded: no delta (the
/// harness replayed its own context), only the restart, what it was working
/// on (146 D10: the interrupted prompt's excerpt) and the ask to go on
/// exactly where it stopped — never 75 §10's cancelled note.
pub async fn restored_note(store: &Store, session: SessionId, at: DateTime<Utc>) -> String {
    let (_, interrupted) = store.prompt_excerpts(session, EXCERPT_CHARS).await;
    render_restored(at, &interrupted)
}

/// The pure half of [`restored_note`].
pub fn render_restored(at: DateTime<Utc>, interrupted: &str) -> String {
    let working_on = if interrupted.is_empty() {
        String::new()
    } else {
        format!(" while you were working on: \"{interrupted}\"")
    };
    format!(
        "<resume_handoff>\n\
         The theuth engine restarted at {}{working_on}. Your session with this harness has been \
         restored with its context; the files and the git state are as you left them. Continue \
         exactly where you left off — do not start over and do not redo finished steps; if you \
         have uncommitted work, checkpoint-commit it first. Report the result when you are done.\n\
         </resume_handoff>",
        hhmmss(at)
    )
}

/// Render `h` within `max_chars`: the header and footer are fixed; the
/// sections follow in order and are truncated tail-first — a section that
/// does not fit keeps its head and says how much was cut, every section
/// after it is named as omitted. Empty sections are left out.
pub fn render(h: &Handoff, max_chars: usize) -> String {
    let previous = h.previous.as_deref().map_or_else(
        || "your previous harness session".to_owned(),
        |p| format!("your previous harness session {p}"),
    );
    let header = format!(
        "<resume_handoff>\n\
         The theuth engine restarted at {} and {previous} could not be loaded, so its context \
         could not be restored. You are a fresh process in the same working directory; the \
         worktree is as you left it — read git log and git status first. Keep the uncommitted \
         diff: do not revert, stash or discard it.\n",
        hhmmss(h.at)
    );
    let footer = format!(
        "The full earlier transcript is available with session_read(\"{}\"); read it if you need \
         the exact brief. Continue from the uncommitted state — do not start over and do not redo \
         finished steps; checkpoint-commit first if you have uncommitted work.\n\
         </resume_handoff>",
        h.session
    );
    let lines = |v: &[String]| v.iter().map(|s| format!("- {s}\n")).collect::<String>();
    let where_ = h
        .cwd
        .as_deref()
        .map_or(String::new(), |c| format!(" in {c}"));
    let sections: [(&str, String); 7] = [
        (
            "original brief",
            if h.brief.is_empty() {
                String::new()
            } else {
                format!("Your original brief: \"{}\"\n", h.brief)
            },
        ),
        (
            "interrupted prompt",
            if h.interrupted.is_empty() || h.interrupted == h.brief {
                String::new()
            } else {
                format!(
                    "The prompt you were interrupted in: \"{}\"\n",
                    h.interrupted
                )
            },
        ),
        (
            "last report",
            if h.last_report.is_empty() {
                String::new()
            } else {
                format!("What you had reported last: \"{}\"\n", h.last_report.trim())
            },
        ),
        (
            "git status",
            if h.status.is_empty() {
                String::new()
            } else {
                format!(
                    "Uncommitted work{where_} (git status --porcelain, {} lines):\n{}\n",
                    h.status.lines().count(),
                    h.status.trim_end()
                )
            },
        ),
        (
            "git diff",
            if h.diff.is_empty() {
                String::new()
            } else {
                format!(
                    "Diff since the last commit (git diff --stat, {} lines):\n{}\n",
                    h.diff.lines().count(),
                    h.diff.trim_end()
                )
            },
        ),
        (
            "queued messages",
            if h.queued.is_empty() {
                String::new()
            } else {
                format!(
                    "Messages queued for you while you were away ({}):\n{}",
                    h.queued.len(),
                    lines(&h.queued)
                )
            },
        ),
        (
            "open todos",
            if h.todos.is_empty() {
                String::new()
            } else {
                format!("Open todos ({}):\n{}", h.todos.len(), lines(&h.todos))
            },
        ),
    ];
    let mut budget = max_chars
        .saturating_sub(header.len() + footer.len())
        .saturating_sub(OMITTED_RESERVE);
    let mut out = header;
    let mut omitted = Vec::new();
    for (name, body) in sections {
        if body.is_empty() {
            continue;
        }
        if body.len() <= budget {
            budget -= body.len();
            out.push_str(&body);
            continue;
        }
        let marker = |cut: usize| format!("… [{cut} chars cut]\n");
        let room = budget.saturating_sub(marker(body.len()).len());
        if room < 40 {
            omitted.push(name);
            continue;
        }
        let mut end = room;
        while !body.is_char_boundary(end) {
            end -= 1;
        }
        out.push_str(body[..end].trim_end());
        out.push('\n');
        out.push_str(&marker(body.len() - end));
        budget = 0;
    }
    if !omitted.is_empty() {
        out.push_str(&format!(
            "(omitted for the handoff budget: {})\n",
            omitted.join(", ")
        ));
    }
    out.push_str(&footer);
    out
}

/// `git <args>` in `cwd`, stdout as text; `""` on any failure (no repo, no
/// commit, no git, timeout) — the section is then simply absent.
pub async fn git(cwd: &Path, args: &[&str]) -> String {
    let run = tokio::process::Command::new("git")
        .args(args)
        .current_dir(cwd)
        .stdin(std::process::Stdio::null())
        .kill_on_drop(true)
        .output();
    match tokio::time::timeout(GIT_TIMEOUT, run).await {
        Ok(Ok(o)) if o.status.success() => String::from_utf8_lossy(&o.stdout).into_owned(),
        Ok(Ok(o)) => {
            tracing::debug!(?args, "git exited {}: section omitted", o.status);
            String::new()
        }
        Ok(Err(e)) => {
            tracing::debug!(?args, "git not run: {e}; section omitted");
            String::new()
        }
        Err(_) => {
            tracing::warn!(?args, cwd = %cwd.display(), "git timed out after {GIT_TIMEOUT:?}; section omitted");
            String::new()
        }
    }
}

/// The §7.3 delta handoff for `session` within `max_tokens` (× 4 chars) —
/// 146 D11's RESUME brief for a fresh harness session on the same row: its
/// original brief and the interrupted prompt (excerpts), its last report,
/// `git status --porcelain` and `git diff --stat HEAD` in its stored `cwd`,
/// the messages it never consumed, its open todos. Store failures leave the
/// section empty; nothing here fails the respawn.
pub async fn compose_handoff(store: &Store, session: SessionId, max_tokens: usize) -> String {
    let row = store.session(session).await.ok().flatten();
    let cwd = row.and_then(|r| r.cwd);
    let (status, diff) = match &cwd {
        Some(c) => {
            let c = Path::new(c);
            (
                git(c, &["status", "--porcelain"]).await,
                git(c, &["diff", "--stat", "HEAD"]).await,
            )
        }
        None => (String::new(), String::new()),
    };
    let to_me = format!("session:{session}");
    let queued = match store.peer_records(session, None, QUEUED_LOOKBACK).await {
        Ok(records) => {
            let mut q: Vec<String> = records
                .into_iter()
                .rev()
                .filter(|r| r.receipt.to == to_me)
                .filter(|r| match r.receipt.state {
                    PeerMessageState::Accepted => true,
                    PeerMessageState::Failed => r.receipt.error.as_deref() == Some(RESTART_REASON),
                    _ => false,
                })
                .map(|r| format!("from {}: {}", r.from, r.text))
                .collect();
            q.dedup();
            q
        }
        Err(e) => {
            tracing::warn!(session = %session, "handoff: peer_records: {e:#}");
            Vec::new()
        }
    };
    let todos = match store.todos(session).await {
        Ok(t) => t
            .items
            .into_iter()
            .filter(|i| !matches!(i.status, Status::Done | Status::Dropped))
            .map(|i| {
                let status = match i.status {
                    Status::InProgress => "in_progress",
                    Status::Blocked => "blocked",
                    _ => "pending",
                };
                match i.reason {
                    Some(why) => format!("[{status}] {} ({why})", i.text),
                    None => format!("[{status}] {}", i.text),
                }
            })
            .collect(),
        Err(e) => {
            tracing::warn!(session = %session, "handoff: todos: {e:#}");
            Vec::new()
        }
    };
    let last_report = store.last_turn_text(session).await.unwrap_or_default();
    let (brief, interrupted) = store.prompt_excerpts(session, EXCERPT_CHARS).await;
    // The id the fresh session replaced (set_acp_session moved it aside);
    // a child that never connected again still names the one it had.
    let previous = match store.acp_previous(session).await.ok().flatten() {
        Some(p) => Some(p),
        None => store.acp_session(session).await.ok().flatten(),
    };
    render(
        &Handoff {
            session: session.to_string(),
            at: Utc::now(),
            cwd,
            last_report,
            status,
            diff,
            queued,
            todos,
            brief,
            interrupted,
            previous,
        },
        max_tokens.saturating_mul(CHARS_PER_TOKEN),
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    fn full() -> Handoff {
        Handoff {
            session: "11111111-1111-1111-1111-111111111111".into(),
            at: Utc::now(),
            cwd: Some("/tmp/x".into()),
            last_report: "Edited foo.rs; tests next.".into(),
            status: " M foo.rs\n?? bar.rs\n".into(),
            diff: " foo.rs | 3 ++-\n 1 file changed\n".into(),
            queued: vec!["from session:p: also fix bar".into()],
            todos: vec!["[in_progress] tests".into(), "[pending] docs".into()],
            brief: "Fix foo.rs and run the tests.".into(),
            interrupted: "also fix bar".into(),
            previous: Some("s-old".into()),
        }
    }

    #[test]
    fn empty_sections_are_omitted_and_full_ones_rendered() {
        let bare = render(&Handoff::default(), 16_000);
        assert!(bare.starts_with("<resume_handoff>\n"), "{bare}");
        assert!(bare.ends_with("</resume_handoff>"), "{bare}");
        for banned in [
            "What you had reported",
            "Uncommitted work",
            "Diff since",
            "Messages queued",
            "Open todos",
            "omitted",
        ] {
            assert!(!bare.contains(banned), "{banned}: {bare}");
        }
        assert!(bare.contains("could not be loaded"), "{bare}");
        assert!(bare.contains("Keep the uncommitted diff"), "{bare}");

        let all = render(&full(), 16_000);
        let order: Vec<usize> = [
            "What you had reported last: \"Edited foo.rs; tests next.\"",
            "Uncommitted work in /tmp/x (git status --porcelain, 2 lines):\n M foo.rs\n?? bar.rs\n",
            "Diff since the last commit (git diff --stat, 2 lines):\n foo.rs",
            "Messages queued for you while you were away (1):\n- from session:p: also fix bar\n",
            "Open todos (2):\n- [in_progress] tests\n- [pending] docs\n",
            "session_read(\"11111111-1111-1111-1111-111111111111\")",
        ]
        .iter()
        .map(|s| all.find(s).unwrap_or_else(|| panic!("{s:?} in {all}")))
        .collect();
        assert!(
            order.windows(2).all(|w| w[0] < w[1]),
            "section order: {all}"
        );
        assert!(!all.contains("omitted"), "{all}");
    }

    #[test]
    fn budget_truncates_tail_first_with_markers() {
        let mut h = full();
        h.last_report = "r".repeat(300);
        h.status = (0..100).map(|i| format!(" M f{i}.rs\n")).collect();
        h.diff = (0..100).map(|i| format!(" f{i}.rs | 1 +\n")).collect();
        let unbounded = render(&h, 1 << 20);
        let fixed = unbounded.len() - 300 - h.status.len() - h.diff.len();
        // Room for the report and part of the status: the diff and the
        // later sections are omitted by name, the status keeps its head.
        let max = fixed + 300 + 400;
        let out = render(&h, max);
        assert!(out.len() <= max, "{} > {max}", out.len());
        assert!(
            out.contains(&"r".repeat(300)),
            "the earlier section is whole"
        );
        assert!(
            out.contains(" M f0.rs\n"),
            "the truncated section keeps its head: {out}"
        );
        assert!(!out.contains(" M f99.rs"), "and loses its tail: {out}");
        assert!(out.contains("chars cut]\n"), "{out}");
        assert!(
            out.contains("(omitted for the handoff budget: git diff, queued messages, open todos)"),
            "{out}"
        );
        assert!(
            out.ends_with("</resume_handoff>"),
            "the footer survives: {out}"
        );
        // Everything fits: nothing cut, nothing omitted.
        let whole = render(&h, unbounded.len() + OMITTED_RESERVE);
        assert_eq!(whole, unbounded);
    }

    /// 146 D10: the restored note names the interrupted prompt and asks to
    /// continue exactly there (never 75 §10's cancelled note); 146 D11: the
    /// fresh-session brief carries the original brief, the interrupted
    /// prompt and the unloadable harness session id.
    #[test]
    fn restored_note_has_no_delta() {
        let n = render_restored(Utc::now(), "run the tests");
        assert!(n.starts_with("<resume_handoff>\n"), "{n}");
        assert!(
            n.contains("while you were working on: \"run the tests\""),
            "{n}"
        );
        assert!(n.contains("Continue exactly where you left off"), "{n}");
        assert!(n.contains("checkpoint-commit"), "{n}");
        assert!(!n.contains("session_read"), "{n}");
        assert!(!n.contains("cancelled"), "{n}");
        assert!(!render_restored(Utc::now(), "").contains("working on"));
        let fresh = render(&full(), 16_000);
        assert!(
            fresh.contains("your previous harness session s-old could not be loaded"),
            "{fresh}"
        );
        assert!(
            fresh.contains("Your original brief: \"Fix foo.rs and run the tests.\""),
            "{fresh}"
        );
        assert!(
            fresh.contains("The prompt you were interrupted in: \"also fix bar\""),
            "{fresh}"
        );
        assert!(
            fresh.contains("read git log and git status first"),
            "{fresh}"
        );
        assert_eq!(
            crate::store::durability::excerpt("<time now=\"x\"/>\n  a  b\n c ", 3),
            "a…"
        );
    }

    /// `git status --porcelain` / `git diff --stat HEAD` from a temp repo,
    /// composed through the store with a stored `cwd`, open todos and a
    /// queued steer; a session without `cwd` renders no git sections.
    #[tokio::test]
    async fn compose_reads_git_store_and_omits_without_cwd() {
        use crate::store::communication::AcceptExchange;
        use crate::todo::{Todo, TodoSource};
        use genai::chat::ChatMessage;

        let scratch = crate::TestDir::new("handoff");
        let dir = scratch.0.clone();
        let sh = |args: &[&str]| {
            let o = std::process::Command::new("git")
                .args(args)
                .current_dir(&dir)
                .env("GIT_AUTHOR_NAME", "t")
                .env("GIT_AUTHOR_EMAIL", "t@t")
                .env("GIT_COMMITTER_NAME", "t")
                .env("GIT_COMMITTER_EMAIL", "t@t")
                .output()
                .unwrap();
            assert!(
                o.status.success(),
                "git {args:?}: {}",
                String::from_utf8_lossy(&o.stderr)
            );
        };
        sh(&["init", "-q"]);
        std::fs::write(dir.join("a.txt"), "one\n").unwrap();
        sh(&["add", "a.txt"]);
        sh(&["-c", "commit.gpgsign=false", "commit", "-q", "-m", "base"]);
        std::fs::write(dir.join("a.txt"), "one\ntwo\n").unwrap();
        std::fs::write(dir.join("new.txt"), "n\n").unwrap();

        let store = Store::open_memory().await.unwrap();
        let parent = SessionId::new();
        let child = SessionId::new();
        store
            .create_session(parent, "m", None, None, None)
            .await
            .unwrap();
        store
            .create_session(child, "omp", Some(parent), Some("w"), None)
            .await
            .unwrap();
        store.set_acp(child, "omp", Some(&dir), &[]).await.unwrap();
        store.append(child, &ChatMessage::user("go")).await.unwrap();
        store
            .append(child, &ChatMessage::assistant("Edited a.txt so far"))
            .await
            .unwrap();
        store
            .save_todos(
                child,
                TodoSource::Acp,
                &[
                    Todo {
                        text: "edit a".into(),
                        status: Status::Done,
                        reason: None,
                    },
                    Todo {
                        text: "run tests".into(),
                        status: Status::InProgress,
                        reason: None,
                    },
                ],
            )
            .await
            .unwrap();
        let (from, to) = (format!("session:{parent}"), format!("session:{child}"));
        store
            .accept_exchange(&AcceptExchange {
                sender: &from,
                recipient: &to,
                sender_session: Some(parent),
                recipient_session: Some(child),
                request_id: None,
                reply_to: None,
                request_scope: None,
                generation: 1,
                body: "also bump the version",
            })
            .await
            .unwrap();
        // A steer the boot pass failed is still "queued for you".
        store
            .accept_exchange(&AcceptExchange {
                sender: &from,
                recipient: &to,
                sender_session: Some(parent),
                recipient_session: Some(child),
                request_id: Some("r2"),
                reply_to: None,
                request_scope: None,
                generation: 1,
                body: "and the changelog",
            })
            .await
            .unwrap();
        store.fail_interrupted(RESTART_REASON).await.unwrap();

        let out = compose_handoff(&store, child, 4000).await;
        assert!(out.len() <= 16_000, "{}", out.len());
        assert!(
            out.contains("What you had reported last: \"Edited a.txt so far\""),
            "{out}"
        );
        assert!(out.contains(" M a.txt\n?? new.txt"), "{out}");
        assert!(out.contains("a.txt | 1 +"), "{out}");
        assert!(out.contains("(2):\n- from session:"), "{out}");
        assert!(
            out.contains("also bump the version\n- from session:"),
            "{out}"
        );
        assert!(out.contains("and the changelog\n"), "{out}");
        assert!(
            out.contains("Open todos (1):\n- [in_progress] run tests\n"),
            "{out}"
        );
        assert!(!out.contains("edit a"), "done items are not open: {out}");
        assert!(out.contains(&format!("session_read(\"{child}\")")), "{out}");

        // No cwd → no git sections; the rest unchanged.
        let bare = SessionId::new();
        store
            .create_session(bare, "omp", None, None, None)
            .await
            .unwrap();
        let out = compose_handoff(&store, bare, 4000).await;
        assert!(!out.contains("Uncommitted work"), "{out}");
        assert!(!out.contains("Diff since"), "{out}");
    }
}
