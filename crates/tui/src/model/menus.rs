use super::*;

/// Visible slice of a list `max` rows tall that keeps `cursor` in view.
pub fn window(len: usize, cursor: usize, max: usize) -> Range<usize> {
    let start = (cursor + 1)
        .saturating_sub(max)
        .min(len.saturating_sub(max));
    start..len.min(start + max)
}

/// An open menu; modal except `Slash`, which rides on the input box.
#[derive(Debug)]
pub enum Menu {
    /// Command list filtered by the input's first word.
    Slash {
        /// Highlighted match.
        cursor: usize,
    },
    /// `/sessions` picker.
    Sessions {
        /// Project headings, then roots with their children under them (51 §3).
        items: Vec<Grouped<(SessionSummary, usize)>>,
        /// Highlighted row.
        cursor: usize,
    },
    /// `/edit`, `/regen`, `/fork` picker over the transcript's turns (29 §6.2).
    Turns {
        /// `(entry index, turn seq, first line of the opener)`, oldest first.
        items: Vec<(usize, i64, String)>,
        /// Highlighted row; starts on the newest.
        cursor: usize,
        /// What ⏎ does with the highlighted turn.
        action: TurnAction,
    },
}

/// A picker row that may be a heading: the cursor skips headings.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Grouped<T> {
    /// Non-selectable label.
    Heading(String),
    /// A selectable row.
    Item(T),
}

impl<T> Grouped<T> {
    /// The item, if this is one.
    pub fn item(&self) -> Option<&T> {
        match self {
            Grouped::Item(t) => Some(t),
            Grouped::Heading(_) => None,
        }
    }
}

/// Cursor after moving `delta` selectable rows from `cursor` (headings skipped, ends clamp).
pub fn grouped_step<T>(rows: &[Grouped<T>], cursor: usize, delta: isize) -> usize {
    let mut at = cursor.min(rows.len().saturating_sub(1));
    for _ in 0..delta.unsigned_abs() {
        let next = if delta < 0 {
            (0..at).rev().find(|&i| rows[i].item().is_some())
        } else {
            (at + 1..rows.len()).find(|&i| rows[i].item().is_some())
        };
        match next {
            Some(i) => at = i,
            None => break,
        }
    }
    at
}

/// First selectable row (`0` when there is none).
pub fn grouped_first<T>(rows: &[Grouped<T>]) -> usize {
    rows.iter().position(|r| r.item().is_some()).unwrap_or(0)
}

/// What the key handler should do after a menu saw a key.
#[derive(Debug, PartialEq, Eq)]
pub enum MenuKey {
    /// Nothing more.
    Consumed,
    /// Replace the input text.
    Input(String),
}

impl Model {

    /// Route a key to the open menu (no-op without one). Returns what the
    /// input box should do.
    pub fn menu_key(
        &mut self,
        code: KeyCode,
        input: &str,
        tx: &UnboundedSender<Command>,
    ) -> MenuKey {
        match &self.menu {
            None => MenuKey::Consumed,
            Some(Menu::Slash { .. }) => self.slash_key(code, input, tx),
            Some(Menu::Sessions { .. }) => self.sessions_key(code, tx),
            Some(Menu::Turns { .. }) => self.turns_key(code, tx),

        }
    }
}
