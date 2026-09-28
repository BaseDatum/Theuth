//! PATH-AC in the TUI: Tab on a path token asks the engine (`path_complete`,
//! so a remote session lists its own machine), a compact list sits under the
//! editor, Tab accepts (a directory keeps completing), Esc closes. Replies
//! carry the generation that asked; a superseded one is inert. Typing while
//! the list is up re-asks after [`DEBOUNCE_MS`].

use iocraft::KeyCode;
use rpc::pathac::{PathCandidate, PathCompletion, trigger};
use runtime::SessionId;
use tokio::sync::mpsc::UnboundedSender;

use crate::model::{Command, send};

/// Where the caret sits after `old` became `new` by one edit: the end of
/// the inserted run, or the deletion point (typing, Backspace, Delete, paste).
pub fn caret_after_edit(old: &str, new: &str) -> usize {
    let common = old
        .bytes()
        .zip(new.bytes())
        .take_while(|(a, b)| a == b)
        .count();
    let mut common = common.min(new.len());
    while !new.is_char_boundary(common) {
        common -= 1;
    }
    if new.len() > old.len() {
        (common + new.len() - old.len()).min(new.len())
    } else {
        common
    }
}

/// Quiet typing before a re-ask.
pub const DEBOUNCE_MS: u64 = 120;
/// Rows the list shows at most.
pub const ROWS: usize = 8;

/// Per-editor completion state; lives on the model.
#[derive(Debug, Default)]
pub struct PathState {
    /// The list, once Tab asked; `items` empty while the first reply is out.
    pub open: Option<Popup>,
    /// Tags the newest request; older replies are dropped.
    generation: u64,
    /// A debounced re-ask: `(due unix ms, text, cursor)`.
    due: Option<(u64, String, usize)>,
    /// The listing engine is Windows (its last reply), so `C:\` triggers too.
    windows: bool,
}

/// The open list.
#[derive(Debug, Default)]
pub struct Popup {
    /// The engine's rows, directories first.
    pub items: Vec<PathCandidate>,
    /// The engine cut the listing at its limit.
    pub truncated: bool,
    /// Highlighted row.
    pub cursor: usize,
    /// A request is out; the rows shown are the previous reply's.
    pub pending: bool,
}

/// What the key handler does after the list saw a key.
#[derive(Debug, PartialEq, Eq)]
pub enum Key {
    /// Not ours: the editor (or whatever follows) takes it.
    Pass,
    /// Nothing more.
    Consumed,
    /// Replace the editor's text and put the caret there.
    Input(String, usize),
}

impl PathState {
    /// The path token at the cursor, if any.
    pub fn token(&self, input: &str, cursor: usize) -> Option<(usize, usize)> {
        trigger(input, cursor, self.windows)
    }

    /// A key while the editor has focus. `slash_busy`: the slash popup has
    /// matches, so Tab is its completion, not ours.
    pub fn key(
        &mut self,
        code: KeyCode,
        input: &str,
        cursor: usize,
        slash_busy: bool,
        session: Option<SessionId>,
        tx: &UnboundedSender<Command>,
    ) -> Key {
        let Some(popup) = &mut self.open else {
            if code == KeyCode::Tab && !slash_busy && self.token(input, cursor).is_some() {
                self.request(input, cursor, session, tx);
                return Key::Consumed;
            }
            return Key::Pass;
        };
        match code {
            KeyCode::Up => popup.cursor = popup.cursor.saturating_sub(1),
            KeyCode::Down => {
                popup.cursor = (popup.cursor + 1).min(popup.items.len().saturating_sub(1))
            }
            KeyCode::Esc => self.close(),
            KeyCode::Tab => {
                let Some(pick) = popup.items.get(popup.cursor).cloned() else {
                    return Key::Consumed;
                };
                let Some((start, end)) = self.token(input, cursor) else {
                    self.close();
                    return Key::Pass;
                };
                let text = format!("{}{}{}", &input[..start], pick.insert, &input[end..]);
                let caret = start + pick.insert.len();
                self.close();
                if pick.is_dir {
                    self.request(&text, caret, session, tx);
                }
                return Key::Input(text, caret);
            }
            KeyCode::Enter => {
                self.close();
                return Key::Pass;
            }
            _ => return Key::Pass,
        }
        Key::Consumed
    }

    /// The editor's text changed while the list is up: re-ask after the
    /// debounce, or close when the cursor left the token.
    pub fn edit(&mut self, input: &str, cursor: usize, now: u64) {
        if self.open.is_none() {
            return;
        }
        if self.token(input, cursor).is_some() {
            self.due = Some((now + DEBOUNCE_MS, input.to_owned(), cursor));
        } else {
            self.close();
        }
    }

    /// When the frame loop must next wake for a debounced re-ask.
    pub fn wake(&self) -> Option<u64> {
        self.due.as_ref().map(|d| d.0)
    }

    /// Send the re-ask once due.
    pub fn tick(&mut self, now: u64, session: Option<SessionId>, tx: &UnboundedSender<Command>) {
        if self.due.as_ref().is_some_and(|d| d.0 <= now) {
            let (_, input, cursor) = self.due.take().unwrap();
            self.request(&input, cursor, session, tx);
        }
    }

    /// A reply; stale generations, failures and empty listings all leave
    /// nothing on screen.
    pub fn reply(&mut self, generation: u64, reply: Option<PathCompletion>) {
        if generation != self.generation || self.open.is_none() {
            return;
        }
        match reply {
            Some(c) if !c.candidates.is_empty() => {
                self.windows = c.platform == "windows";
                self.open = Some(Popup {
                    items: c.candidates,
                    truncated: c.truncated,
                    cursor: 0,
                    pending: false,
                });
            }
            _ => self.close(),
        }
    }

    fn request(
        &mut self,
        input: &str,
        cursor: usize,
        session: Option<SessionId>,
        tx: &UnboundedSender<Command>,
    ) {
        self.generation += 1;
        self.due = None;
        let popup = self.open.get_or_insert_with(Popup::default);
        popup.pending = true;
        send(
            tx,
            Command::PathComplete {
                session,
                input: input.to_owned(),
                cursor,
                generation: self.generation,
            },
        );
    }

    fn close(&mut self) {
        self.open = None;
        self.due = None;
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use tokio::sync::mpsc::unbounded_channel;

    fn cand(name: &str, head: &str, is_dir: bool) -> PathCandidate {
        let tail = if is_dir { "/" } else { "" };
        PathCandidate {
            insert: format!("{head}{name}{tail}"),
            display: format!("{name}{tail}"),
            is_dir,
        }
    }

    fn reply(head: &str, names: &[(&str, bool)], truncated: bool) -> PathCompletion {
        PathCompletion {
            token_start: 0,
            token_end: 0,
            candidates: names.iter().map(|(n, d)| cand(n, head, *d)).collect(),
            truncated,
            platform: "macos".into(),
        }
    }

    fn generation(rx: &mut tokio::sync::mpsc::UnboundedReceiver<Command>) -> u64 {
        match rx.try_recv().expect("a request") {
            Command::PathComplete { generation, .. } => generation,
            other => panic!("{other:?}"),
        }
    }

    #[test]
    fn tab_opens_navigates_accepts_and_keeps_completing_dirs() {
        let (tx, mut rx) = unbounded_channel();
        let mut s = PathState::default();
        // No token: Tab is not ours.
        assert_eq!(s.key(KeyCode::Tab, "hello", 5, false, None, &tx), Key::Pass);
        assert!(rx.try_recv().is_err());
        // The slash popup has matches: its Tab.
        assert_eq!(s.key(KeyCode::Tab, "/s", 2, true, None, &tx), Key::Pass);
        // A path token: Tab asks, list opens pending.
        assert_eq!(
            s.key(KeyCode::Tab, "see ~/De", 8, false, None, &tx),
            Key::Consumed
        );
        let g = generation(&mut rx);
        assert!(
            s.open
                .as_ref()
                .is_some_and(|p| p.pending && p.items.is_empty())
        );
        // A stale reply is inert; the right one lands.
        s.reply(g - 1, Some(reply("~/", &[("Old", true)], false)));
        assert!(s.open.as_ref().unwrap().items.is_empty());
        s.reply(
            g,
            Some(reply(
                "~/",
                &[("Develop", true), ("Desktop", true), ("Dfile", false)],
                true,
            )),
        );
        let p = s.open.as_ref().unwrap();
        assert_eq!(p.items.len(), 3);
        assert!(p.truncated && !p.pending);
        // Navigate; clamped at both ends.
        s.key(KeyCode::Up, "see ~/De", 8, false, None, &tx);
        assert_eq!(s.open.as_ref().unwrap().cursor, 0);
        s.key(KeyCode::Down, "see ~/De", 8, false, None, &tx);
        s.key(KeyCode::Down, "see ~/De", 8, false, None, &tx);
        s.key(KeyCode::Down, "see ~/De", 8, false, None, &tx);
        assert_eq!(s.open.as_ref().unwrap().cursor, 2);
        s.key(KeyCode::Up, "see ~/De", 8, false, None, &tx);
        // Tab accepts a directory: text keeps `~/`, caret after it, a new ask goes out.
        assert_eq!(
            s.key(KeyCode::Tab, "see ~/De", 8, false, None, &tx),
            Key::Input("see ~/Desktop/".into(), 14)
        );
        let g2 = generation(&mut rx);
        assert!(g2 > g);
        assert!(s.open.as_ref().is_some_and(|p| p.pending));
        // A file closes the list; nothing more is asked.
        s.reply(g2, Some(reply("~/Desktop/", &[("notes.md", false)], false)));
        assert_eq!(
            s.key(KeyCode::Tab, "see ~/Desktop/", 14, false, None, &tx),
            Key::Input("see ~/Desktop/notes.md".into(), 22)
        );
        assert!(s.open.is_none());
        assert!(rx.try_recv().is_err());
    }

    #[test]
    fn caret_follows_the_edit() {
        assert_eq!(caret_after_edit("~/De", "~/Dev"), 5, "typed at the end");
        assert_eq!(
            caret_after_edit("see  x", "see ~/ x"),
            6,
            "inserted mid-line"
        );
        assert_eq!(caret_after_edit("~/Dev", "~/De"), 4, "backspace");
        assert_eq!(caret_after_edit("~/Dev x", "~/Devx"), 5, "delete mid-line");
        assert_eq!(caret_after_edit("", "~/Develop/"), 10, "paste into empty");
        assert_eq!(caret_after_edit("é", "éa"), 3, "multibyte");
    }

    #[test]
    fn typing_debounces_esc_closes_and_empty_replies_close() {
        let (tx, mut rx) = unbounded_channel();
        let mut s = PathState::default();
        s.key(KeyCode::Tab, "./s", 3, false, None, &tx);
        let g = generation(&mut rx);
        s.reply(g, Some(reply("./", &[("src", true)], false)));
        // Typing inside the token: re-ask after the debounce, not before.
        s.edit("./sr", 4, 1000);
        assert_eq!(s.wake(), Some(1000 + DEBOUNCE_MS));
        s.tick(1000 + DEBOUNCE_MS - 1, None, &tx);
        assert!(rx.try_recv().is_err());
        s.tick(1000 + DEBOUNCE_MS, None, &tx);
        let g2 = generation(&mut rx);
        assert!(g2 > g && s.wake().is_none());
        // An empty listing (missing dir, RPC failure) shows nothing.
        s.reply(g2, None);
        assert!(s.open.is_none());
        // Reopen, then Esc closes; Enter passes through and closes.
        s.key(KeyCode::Tab, "./sr", 4, false, None, &tx);
        assert_eq!(
            s.key(KeyCode::Esc, "./sr", 4, false, None, &tx),
            Key::Consumed
        );
        assert!(s.open.is_none());
        s.key(KeyCode::Tab, "./sr", 4, false, None, &tx);
        assert_eq!(
            s.key(KeyCode::Enter, "./sr", 4, false, None, &tx),
            Key::Pass
        );
        assert!(s.open.is_none());
        // Leaving the token closes without a request.
        s.key(KeyCode::Tab, "./sr", 4, false, None, &tx);
        while rx.try_recv().is_ok() {}
        s.edit("./sr done", 9, 5000);
        assert!(s.open.is_none() && s.wake().is_none());
    }
}
