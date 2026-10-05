//! The board: one message per parent thread listing its live descendants,
//! edited in place while any is busy (design doc 98 §9.3).

use std::time::Duration;

use runtime::SessionId;
use tokio::time::Instant;

use crate::api::{self, MsgRef};
use crate::bubble::{note_open, report, send_rich_or_plain};
use crate::ui;
use crate::worker::{Row, WorkerCtx, brand};

/// Board edits are at least this far apart.
pub const EDIT_GAP: Duration = Duration::from_secs(2);
/// Rows shown before `… n more`.
pub const ROWS_MAX: usize = 8;

/// The parent thread's view of its children.
#[derive(Debug, Default)]
pub struct Board {
    /// `(child, row, idle)` in first-seen order.
    children: Vec<(SessionId, Row, bool)>,
    msg: Option<MsgRef>,
    dirty: bool,
    last_edit: Option<Instant>,
}

impl Board {
    /// `ChildRow`: remember the row; a change marks the board dirty.
    pub fn update(&mut self, child: SessionId, row: Row, idle: bool) {
        match self.children.iter_mut().find(|(c, _, _)| *c == child) {
            Some((_, r, i)) if *r == row && *i == idle => return,
            Some((_, r, i)) => {
                *r = row;
                *i = idle;
            }
            None => self.children.push((child, row, idle)),
        }
        self.dirty = true;
    }

    fn all_idle(&self) -> bool {
        self.children.iter().all(|(_, _, idle)| *idle)
    }

    /// The table, or `**agents** · all idle`.
    #[must_use]
    pub fn render(&self, ctx: &WorkerCtx) -> String {
        if self.all_idle() {
            return brand(ctx, "**agents** · all idle");
        }
        let header = ["", "name", "state", "detail", "age"]
            .map(str::to_owned)
            .to_vec();
        let rows = self
            .children
            .iter()
            .filter(|(_, _, idle)| !idle)
            .take(ROWS_MAX)
            .map(|(_, r, _)| {
                vec![
                    r.glyph.to_owned(),
                    r.name.clone(),
                    r.state.clone(),
                    r.detail.clone(),
                    fmt_age(r.age_secs),
                ]
            });
        let mut out = format!(
            "**agents**\n{}",
            ui::table(std::iter::once(header).chain(rows))
        );
        let busy = self.children.iter().filter(|(_, _, idle)| !idle).count();
        if busy > ROWS_MAX {
            out.push_str(&format!("\n… {} more", busy - ROWS_MAX));
        }
        brand(ctx, &out)
    }

    /// The tick: send once, then edit ≥ 2 s apart; the all-idle edit ends the message.
    pub async fn flush(&mut self, ctx: &WorkerCtx) {
        if !self.dirty {
            return;
        }
        let idle = self.all_idle();
        match self.msg {
            None => {
                if idle {
                    self.dirty = false;
                    return;
                }
                if let Ok(Some(id)) = send_rich_or_plain(ctx, &self.render(ctx)).await {
                    let m = MsgRef {
                        chat: ctx.to.chat,
                        id,
                    };
                    self.msg = Some(m);
                    note_open(ctx, m, true);
                }
            }
            Some(m) => {
                if self.last_edit.is_some_and(|t| t.elapsed() < EDIT_GAP) {
                    return;
                }
                match ctx.api.edit_rich(&m, &self.render(ctx), None).await {
                    Ok(()) => {}
                    Err(e) if api::is_not_modified(&e) => {}
                    Err(e) if api::is_gone_message(&e) => {
                        note_open(ctx, m, false);
                        self.msg = None;
                        self.dirty = true;
                        return;
                    }
                    Err(e) => report(ctx, &e, "board edit"),
                }
                if idle {
                    note_open(ctx, m, false);
                    self.msg = None;
                }
            }
        }
        self.dirty = false;
        self.last_edit = Some(Instant::now());
    }

    /// Final edit on shutdown: whatever the board shows now.
    pub async fn close(&mut self, ctx: &WorkerCtx) {
        if let Some(m) = self.msg.take() {
            if let Err(e) = ctx.api.edit_rich(&m, &self.render(ctx), None).await
                && !api::is_not_modified(&e)
            {
                report(ctx, &e, "board close");
            }
            note_open(ctx, m, false);
        }
    }
}

/// `—` · `48s` · `1m32s` · `2h05m`.
fn fmt_age(secs: u64) -> String {
    match secs {
        0 => "—".to_owned(),
        s if s < 60 => format!("{s}s"),
        s if s < 3600 => format!("{}m{:02}s", s / 60, s % 60),
        s => format!("{}h{:02}m", s / 3600, (s % 3600) / 60),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ages() {
        assert_eq!(fmt_age(0), "—");
        assert_eq!(fmt_age(48), "48s");
        assert_eq!(fmt_age(92), "1m32s");
        assert_eq!(fmt_age(7500), "2h05m");
    }
}
