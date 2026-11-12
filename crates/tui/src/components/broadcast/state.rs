//! `/broadcast` (62 §5.1): pick sessions, compose one message, review the
//! dry run, send. Modelled on the `/shake` picker: nothing is sent before
//! the review's ⏎.

use std::collections::HashSet;

use agent::{
    AgentInfo, AgentState, BroadcastOutcome, BroadcastReceipt, BroadcastRequest, BroadcastTargets,
    Delivery, SessionActivity,
};
use runtime::SessionId;

/// Which `/broadcast` screen is up.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum BroadcastStage {
    /// Ticking session rows.
    Pick,
    /// Typing the message.
    Compose,
    /// The dry-run rows, awaiting ⏎.
    Review,
}

/// `/broadcast` state.
#[derive(Debug)]
pub struct Broadcast {
    /// `list_activity()` grouped root → children, in display order.
    pub rows: Vec<AgentInfo>,
    /// Ticked sessions.
    pub selected: HashSet<SessionId>,
    /// Highlighted row of the current stage.
    pub cursor: usize,
    /// Which screen is up.
    pub stage: BroadcastStage,
    /// How the message lands on a busy recipient (`d` toggles).
    pub delivery: Delivery,
    /// The message.
    pub text: String,
    /// The dry run; `None` while it is in flight.
    pub preview: Option<BroadcastReceipt>,
    /// The last failed request, shown on the last row.
    pub error: Option<String>,
    /// The selector the dry run used; `Sessions` when it came through Pick.
    pub targets: BroadcastTargets,
}

/// A root that counts as "running" (62 §0.2).
pub fn running(a: &SessionActivity) -> bool {
    matches!(
        a.state,
        AgentState::Running | AgentState::Awaiting { .. } | AgentState::Approval { .. }
    ) || a.jobs + a.ptys > 0
}

/// ` j2 p1`: the non-zero job and PTY counts, empty when both are zero.
pub fn counts(a: &SessionActivity) -> String {
    let mut s = String::new();
    if a.jobs > 0 {
        s.push_str(&format!(" j{}", a.jobs));
    }
    if a.ptys > 0 {
        s.push_str(&format!(" p{}", a.ptys));
    }
    s
}

/// Time since `t`, coarse: `48s`, `3m`, `2h`, `3d`; empty when unknown.
pub fn age(t: Option<chrono::DateTime<chrono::Utc>>) -> String {
    t.map_or_else(String::new, |t| since(chrono::Utc::now() - t))
}

/// `48s`, `3m`, `2h`, `3d`.
pub fn since(d: chrono::Duration) -> String {
    let s = d.num_seconds().max(0);
    match s {
        0..60 => format!("{s}s"),
        60..3600 => format!("{}m", s / 60),
        3600..86_400 => format!("{}h", s / 3600),
        _ => format!("{}d", s / 86_400),
    }
}

/// Row glyph for a state; `spinner` is the running glyph.
pub fn glyph<'a>(state: &AgentState, spinner: &'a str) -> &'a str {
    match state {
        AgentState::Running => spinner,
        AgentState::Awaiting { .. } => "◔",
        AgentState::Approval { .. } => "!",
        AgentState::Error => "✗",
        AgentState::Cancelled => "∅",
        AgentState::Idle => "·",
    }
}

/// Roots in list order, each followed by its descendants (depth-first);
/// anything whose parent is not listed trails.
pub fn group(list: Vec<AgentInfo>) -> Vec<AgentInfo> {
    // ponytail: O(n²) over the active set; index by parent if it ever grows past dozens.
    fn children(
        list: &[AgentInfo],
        parent: SessionId,
        seen: &mut HashSet<SessionId>,
        out: &mut Vec<AgentInfo>,
    ) {
        for a in list.iter().filter(|a| a.depth > 0 && a.parent == parent) {
            if seen.insert(a.id) {
                out.push(a.clone());
                children(list, a.id, seen, out);
            }
        }
    }
    let mut seen = HashSet::new();
    let mut out = Vec::with_capacity(list.len());
    for r in list.iter().filter(|a| a.depth == 0) {
        if seen.insert(r.id) {
            out.push(r.clone());
            children(&list, r.id, &mut seen, &mut out);
        }
    }
    out.extend(list.into_iter().filter(|a| !seen.contains(&a.id)));
    out
}

/// `steer` / `next turn`.
fn delivery_word(d: Delivery) -> &'static str {
    match d {
        Delivery::Steer => "steer",
        Delivery::NextTurn => "next turn",
    }
}

impl Broadcast {
    /// The picker over `list`: running roots ticked, delivery `Steer`.
    pub fn pick(list: Vec<AgentInfo>) -> Self {
        let rows = group(list);
        let selected = rows
            .iter()
            .filter(|a| a.depth == 0 && running(&a.activity))
            .map(|a| a.id)
            .collect();
        Self {
            rows,
            selected,
            cursor: 0,
            stage: BroadcastStage::Pick,
            delivery: Delivery::Steer,
            text: String::new(),
            preview: None,
            error: None,
            targets: BroadcastTargets::Sessions { ids: Vec::new() },
        }
    }

    /// Straight to review: `/broadcast running|roots <text>`; the dry run is in flight.
    pub fn review(targets: BroadcastTargets, text: String) -> Self {
        Self {
            rows: Vec::new(),
            selected: HashSet::new(),
            cursor: 0,
            stage: BroadcastStage::Review,
            delivery: Delivery::Steer,
            text,
            preview: None,
            error: None,
            targets,
        }
    }

    /// The request for the current selector, text and delivery.
    pub fn request(&self, dry_run: bool) -> BroadcastRequest {
        BroadcastRequest {
            targets: self.targets.clone(),
            text: self.text.clone(),
            files: Vec::new(),
            delivery: self.delivery,
            dry_run,
        }
    }

    /// Tick or untick the highlighted row.
    pub fn toggle(&mut self) {
        if let Some(a) = self.rows.get(self.cursor)
            && !self.selected.remove(&a.id)
        {
            self.selected.insert(a.id);
        }
    }

    /// Tick exactly the roots `keep` accepts.
    pub fn tick_roots(&mut self, keep: impl Fn(&AgentInfo) -> bool) {
        self.selected = self
            .rows
            .iter()
            .filter(|a| a.depth == 0 && keep(a))
            .map(|a| a.id)
            .collect();
    }

    /// Flip `Steer` ↔ `NextTurn`.
    pub fn toggle_delivery(&mut self) {
        self.delivery = match self.delivery {
            Delivery::Steer => Delivery::NextTurn,
            Delivery::NextTurn => Delivery::Steer,
        };
    }

    /// Leave Compose: the ticked sessions in row order become the selector.
    pub fn select_ticked(&mut self) {
        self.targets = BroadcastTargets::Sessions {
            ids: self
                .rows
                .iter()
                .filter(|a| self.selected.contains(&a.id))
                .map(|a| a.id)
                .collect(),
        };
    }

    /// Review came from the picker (esc goes back to Compose, not out).
    pub fn picked(&self) -> bool {
        matches!(self.targets, BroadcastTargets::Sessions { .. })
    }

    /// Rows the current stage lists; `spinner` is the running glyph.
    pub fn rows(&self, spinner: &str) -> Vec<String> {
        let mut rows = match self.stage {
            BroadcastStage::Pick => self
                .rows
                .iter()
                .map(|a| {
                    let name = match a.depth {
                        0 => a.name.clone(),
                        d => format!("{}↳ {}", "  ".repeat(usize::from(d - 1)), a.name),
                    };
                    format!(
                        "[{}] {} {name:<20} {:<9}{}  {}",
                        if self.selected.contains(&a.id) {
                            "x"
                        } else {
                            " "
                        },
                        glyph(&a.activity.state, spinner),
                        a.activity.state.word(),
                        counts(&a.activity),
                        age(a.activity.last_active),
                    )
                })
                .collect(),
            BroadcastStage::Compose => vec![
                format!("> {}▏", self.text),
                format!(
                    "delivery: {} · {} session(s)",
                    delivery_word(self.delivery),
                    self.selected.len()
                ),
            ],
            BroadcastStage::Review => match &self.preview {
                None => vec!["resolving…".to_owned()],
                Some(r) => {
                    let mut v = vec![
                        format!("[broadcast … · {} sessions]", r.rows.len()),
                        format!("  {}", self.text.lines().next().unwrap_or_default()),
                    ];
                    v.extend(r.rows.iter().map(|row| {
                        let will = match &row.outcome {
                            BroadcastOutcome::Skipped { reason } if reason != "dry run" => {
                                format!("skipped: {reason}")
                            }
                            BroadcastOutcome::Failed { error } => format!("failed: {error}"),
                            _ if running(&row.before) => {
                                format!("will queue ({})", delivery_word(self.delivery))
                            }
                            _ => "will start now".to_owned(),
                        };
                        format!("{} · {} · {will}", row.title, row.before.state.word())
                    }));
                    v
                }
            },
        };
        if let Some(e) = &self.error {
            rows.push(format!("error: {e}"));
        }
        rows
    }

    /// Menu title: keys and the count.
    pub fn title(&self) -> String {
        match self.stage {
            BroadcastStage::Pick => format!(
                "broadcast  ↑↓ move  space tick  r running  a all roots  n none  d delivery: {}  ⏎ compose  esc close   {} ticked",
                delivery_word(self.delivery),
                self.selected.len()
            ),
            BroadcastStage::Compose => {
                "broadcast  type the message  ⏎ preview  esc back".to_owned()
            }
            BroadcastStage::Review => format!(
                "broadcast  ⏎ sends to {} session(s)  esc {}",
                self.preview.as_ref().map_or(0, |r| r.rows.len()),
                if self.picked() { "back" } else { "close" }
            ),
        }
    }
}

/// The receipt as transcript lines: one per row, then the summary
/// `sent to N (S started, Q queued, K skipped)`.
pub fn result_lines(r: &BroadcastReceipt) -> Vec<String> {
    let (mut started, mut queued, mut skipped, mut failed) = (0, 0, 0, 0);
    let mut lines: Vec<String> = r
        .rows
        .iter()
        .map(|row| {
            let outcome = match &row.outcome {
                BroadcastOutcome::Started => {
                    started += 1;
                    "started".to_owned()
                }
                BroadcastOutcome::Queued { delivery, .. } => {
                    queued += 1;
                    format!("queued ({})", delivery_word(*delivery))
                }
                BroadcastOutcome::Skipped { reason } => {
                    skipped += 1;
                    format!("skipped: {reason}")
                }
                BroadcastOutcome::Failed { error } => {
                    failed += 1;
                    format!("failed: {error}")
                }
            };
            format!("{} — {outcome}", row.title)
        })
        .collect();
    let mut summary = format!(
        "sent to {} ({started} started, {queued} queued, {skipped} skipped",
        r.rows.len()
    );
    if failed > 0 {
        summary.push_str(&format!(", {failed} failed"));
    }
    summary.push(')');
    lines.push(summary);
    lines
}
