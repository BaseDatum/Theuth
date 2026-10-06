//! The plan message of a thread (design doc 110 §5): one `plan` block posted
//! on the first non-empty `TodoChanged`, `chat.update`d in place on later
//! snapshots ≥ [`board::BOARD_MIN_GAP`] apart, forced on `TurnFinished` and
//! left as history. Pure like [`board`]: no I/O, the worker supplies the
//! clock and sends what [`Plan::take_due`] hands back.

use agent::todo::{Status, TodoSnapshot};
use serde_json::{Value, json};
use slack_morphism::prelude::*;
use tokio::time::Instant;

use crate::board;
use crate::ui;

/// `plan.tasks` cap; over it, 49 items and a last `… n more` task.
pub const TASKS_MAX: usize = 50;
/// `plan.title` cap (plain text).
const TITLE_MAX: usize = 150;
/// A task card title cap, as `task_update.title`.
const TASK_TITLE_MAX: usize = 256;

/// The thread's plan message. `ts` is `None` until posted and again after
/// the turn's final edit, so the next turn posts a fresh message (§10).
#[derive(Debug, Default)]
pub struct Plan {
    ts: Option<String>,
    version: u64,
    snapshot: TodoSnapshot,
    dirty: bool,
    last_edit: Option<Instant>,
}

/// What the worker sends: `update_blocks` on `Some(ts)`, else `post_blocks`.
#[derive(Debug, PartialEq, Eq)]
pub struct Edit {
    /// The message to edit; `None` posts a new one.
    pub ts: Option<String>,
    /// One `plan` block.
    pub blocks: Value,
}

impl Plan {
    /// `TodoChanged`: applied when `version` is higher and `items` is not
    /// empty (an emptied list leaves the last plan standing as history).
    pub fn update(&mut self, snapshot: TodoSnapshot) -> bool {
        if snapshot.version <= self.version || snapshot.items.is_empty() {
            return false;
        }
        self.version = snapshot.version;
        self.snapshot = snapshot;
        self.dirty = true;
        true
    }

    /// The edit due at `now`: the first post at once, later edits
    /// [`board::BOARD_MIN_GAP`] apart unless `force` (`TurnFinished`).
    /// Marks the plan clean; call [`Plan::posted`] with the new message's ts.
    pub fn take_due(&mut self, now: Instant, force: bool) -> Option<Edit> {
        if !self.dirty {
            return None;
        }
        let gap_ok = self
            .last_edit
            .is_none_or(|at| now.saturating_duration_since(at) >= board::BOARD_MIN_GAP);
        if !(force || self.ts.is_none() || gap_ok) {
            return None;
        }
        self.dirty = false;
        self.last_edit = Some(now);
        Some(Edit {
            ts: self.ts.clone(),
            blocks: blocks(&self.snapshot),
        })
    }

    /// The `post_blocks` result for an [`Edit`] with `ts: None`.
    pub fn posted(&mut self, ts: String) {
        self.ts = Some(ts);
    }

    /// `TurnFinished`, after the forced edit went out: the message stays as
    /// history and the next snapshot posts a new one.
    pub fn finish(&mut self) {
        self.ts = None;
        self.dirty = false;
        self.last_edit = None;
    }
}

/// One `plan` block (§5): title = the `in_progress` item, else
/// `Plan · n of m done`; one task per non-dropped item, `task_id` = its
/// index in the snapshot; `blocked` → `error` with the reason as details.
/// Built typed (`SlackPlanBlock`), handed to `post_blocks` as JSON.
#[must_use]
pub fn blocks(snapshot: &TodoSnapshot) -> Value {
    let items: Vec<(usize, &agent::todo::Todo)> = snapshot
        .items
        .iter()
        .enumerate()
        .filter(|(_, t)| t.status != Status::Dropped)
        .collect();
    let done = items
        .iter()
        .filter(|(_, t)| t.status == Status::Done)
        .count();
    let title = items
        .iter()
        .find(|(_, t)| t.status == Status::InProgress)
        .map_or_else(
            || format!("Plan · {done} of {} done", items.len()),
            |(_, t)| t.text.clone(),
        );
    let shown = if items.len() > TASKS_MAX {
        TASKS_MAX - 1
    } else {
        items.len()
    };
    let mut tasks: Vec<SlackTaskCardBlock> = items
        .iter()
        .take(shown)
        .map(|(i, t)| {
            let status = match t.status {
                Status::Done => SlackTaskCardStatus::Complete,
                Status::InProgress => SlackTaskCardStatus::InProgress,
                Status::Blocked => SlackTaskCardStatus::Error,
                Status::Pending | Status::Dropped => SlackTaskCardStatus::Pending,
            };
            SlackTaskCardBlock::new(i.to_string().into(), ui::clip(&t.text, TASK_TITLE_MAX))
                .with_status(status)
                .opt_details(
                    t.reason
                        .as_deref()
                        .filter(|r| !r.trim().is_empty())
                        .map(rich_text),
                )
        })
        .collect();
    if items.len() > shown {
        tasks.push(
            SlackTaskCardBlock::new("more".into(), format!("… {} more", items.len() - shown))
                .with_status(SlackTaskCardStatus::Pending),
        );
    }
    json!([SlackBlock::from(SlackPlanBlock::new(
        ui::clip(&title, TITLE_MAX),
        tasks
    ))])
}

fn rich_text(text: &str) -> SlackRichTextInlineContent {
    SlackRichTextBlock::new(vec![SlackRichTextSection::from(text).into()]).into()
}

#[cfg(test)]
mod tests {
    use std::time::Duration;

    use agent::todo::{Todo, TodoSource};

    use super::*;

    fn todo(text: &str, status: Status) -> Todo {
        Todo {
            text: text.into(),
            status,
            reason: (status == Status::Blocked).then(|| "waiting on the user".to_owned()),
        }
    }

    fn snap(version: u64, items: Vec<Todo>) -> TodoSnapshot {
        TodoSnapshot {
            version,
            source: TodoSource::Native,
            items,
        }
    }

    fn plan_block(v: &Value) -> &Value {
        let arr = v.as_array().unwrap();
        assert_eq!(arr.len(), 1);
        assert_eq!(arr[0]["type"], "plan");
        &arr[0]
    }

    #[test]
    fn first_nonempty_snapshot_posts_a_plan_block() {
        let mut p = Plan::default();
        let now = Instant::now();
        assert!(!p.update(snap(1, vec![])), "empty list never posts");
        assert!(p.take_due(now, false).is_none());
        assert!(p.update(snap(2, vec![todo("a", Status::Pending)])));
        let e = p.take_due(now, false).expect("first post is immediate");
        assert_eq!(e.ts, None);
        assert_eq!(plan_block(&e.blocks)["tasks"][0]["title"], "a");
        assert!(p.take_due(now, false).is_none(), "clean after the send");
    }

    #[test]
    fn title_is_the_in_progress_item_else_progress() {
        let b = blocks(&snap(
            1,
            vec![
                todo("done one", Status::Done),
                todo("doing two", Status::InProgress),
                todo("later", Status::Pending),
            ],
        ));
        assert_eq!(plan_block(&b)["title"], "doing two");
        let b = blocks(&snap(
            1,
            vec![
                todo("done one", Status::Done),
                todo("later", Status::Pending),
                todo("gone", Status::Dropped),
            ],
        ));
        assert_eq!(plan_block(&b)["title"], "Plan · 1 of 2 done");
    }

    #[test]
    fn blocked_maps_to_error_with_reason_dropped_omitted() {
        let b = blocks(&snap(
            1,
            vec![
                todo("a", Status::Done),
                todo("gone", Status::Dropped),
                todo("b", Status::Blocked),
                todo("c", Status::Pending),
            ],
        ));
        let tasks = plan_block(&b)["tasks"].as_array().unwrap();
        let ids: Vec<&str> = tasks
            .iter()
            .map(|t| t["task_id"].as_str().unwrap())
            .collect();
        assert_eq!(
            ids,
            ["0", "2", "3"],
            "index in the snapshot, dropped omitted"
        );
        let status: Vec<&str> = tasks
            .iter()
            .map(|t| t["status"].as_str().unwrap())
            .collect();
        assert_eq!(status, ["complete", "error", "pending"]);
        assert_eq!(
            tasks[1]["details"]["elements"][0]["elements"][0]["text"],
            "waiting on the user"
        );
        assert!(tasks[0].get("details").is_none());
    }

    #[test]
    fn lower_version_is_ignored() {
        let mut p = Plan::default();
        assert!(p.update(snap(5, vec![todo("new", Status::Pending)])));
        assert!(!p.update(snap(4, vec![todo("stale", Status::Pending)])));
        assert!(!p.update(snap(5, vec![todo("same", Status::Pending)])));
        let e = p.take_due(Instant::now(), false).unwrap();
        assert_eq!(plan_block(&e.blocks)["tasks"][0]["title"], "new");
    }

    #[test]
    fn edits_coalesce_at_two_seconds_and_turn_finished_forces_the_last() {
        let mut p = Plan::default();
        let t0 = Instant::now();
        p.update(snap(1, vec![todo("a", Status::InProgress)]));
        assert_eq!(p.take_due(t0, false).unwrap().ts, None);
        p.posted("5.5".into());

        p.update(snap(2, vec![todo("a", Status::Done)]));
        assert!(p.take_due(t0 + Duration::from_secs(1), false).is_none());
        let e = p.take_due(t0 + board::BOARD_MIN_GAP, false).unwrap();
        assert_eq!(e.ts.as_deref(), Some("5.5"));
        assert_eq!(plan_block(&e.blocks)["title"], "Plan · 1 of 1 done");

        p.update(snap(
            3,
            vec![todo("a", Status::Done), todo("b", Status::InProgress)],
        ));
        let soon = t0 + board::BOARD_MIN_GAP + Duration::from_millis(100);
        assert!(p.take_due(soon, false).is_none());
        let last = p.take_due(soon, true).expect("TurnFinished forces it");
        assert_eq!(last.ts.as_deref(), Some("5.5"));
        assert_eq!(plan_block(&last.blocks)["title"], "b");
        p.finish();

        // The next turn's snapshot posts a fresh message; a stale one is still refused.
        assert!(!p.update(snap(2, vec![todo("old", Status::Pending)])));
        p.update(snap(4, vec![todo("c", Status::Pending)]));
        assert_eq!(p.take_due(soon, false).unwrap().ts, None);
    }

    #[test]
    fn over_fifty_items_truncates_with_a_count() {
        let items: Vec<Todo> = (0..60)
            .map(|i| todo(&format!("item {i}"), Status::Pending))
            .collect();
        let b = blocks(&snap(1, items));
        let tasks = plan_block(&b)["tasks"].as_array().unwrap();
        assert_eq!(tasks.len(), TASKS_MAX);
        assert_eq!(tasks[48]["task_id"], "48");
        assert_eq!(tasks[49]["title"], "… 11 more");
        assert_eq!(tasks[49]["task_id"], "more");
        let b = blocks(&snap(
            1,
            (0..50)
                .map(|i| todo(&format!("item {i}"), Status::Pending))
                .collect(),
        ));
        assert_eq!(plan_block(&b)["tasks"].as_array().unwrap().len(), 50);
    }
}
