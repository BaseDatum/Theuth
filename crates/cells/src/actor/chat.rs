//! Owner: E1 — the engine's own record of chat cells (design 171 §3.4, §3.5, §12.1 inv. 5):
//! the stored assistant messages of a session, the cells each presents (fences in document
//! order, CommonMark rules), and the files the session's agent wrote or edited (§3.7 lint).
//!
//! `CellRef::Chat.entry` is the stored message's `seq` as text (the desktop's own entry ids
//! are client counters the engine never sees). A message is in the record only once its
//! model round is stored, so a stored message is final.

use std::path::PathBuf;

use kameo::actor::ActorRef;

use crate::error::{CellsError, Result};
use crate::types::{Cell, SessionId};

/// One presented cell: the fence as written and its parse.
#[derive(Clone, Debug)]
pub(crate) struct Presented {
    /// The parse.
    pub cell: Cell,
    /// §3.4 digest of `body`.
    pub digest: String,
}

/// A session's assistant messages (seq, text), oldest first, and the agent's edited files.
#[derive(Clone, Debug, Default)]
pub(crate) struct Transcript {
    pub messages: Vec<(i64, String)>,
    pub edited: Vec<PathBuf>,
}

impl Transcript {
    /// Every message's cells, oldest first, with its seq.
    pub fn all(&self) -> Vec<(i64, Vec<Presented>)> {
        self.messages
            .iter()
            .map(|(s, t)| (*s, cells_of(t)))
            .collect()
    }
}

/// Read the session's record through `agent::ReadSession` (private sessions included).
pub(crate) async fn transcript(session: SessionId) -> Result<Transcript> {
    let agents = ActorRef::<agent::Agents>::lookup(agent::AGENT_ACTOR)
        .ok()
        .flatten()
        .ok_or_else(|| CellsError::Failed("the agent is not running".into()))?;
    let read = agents
        .ask(agent::ReadSession {
            session,
            caller: session,
        })
        .await
        .map_err(|e| CellsError::Failed(format!("read session: {e}")))?;
    let cwd = read.summary.cwd.clone().map(PathBuf::from);
    let mut t = Transcript::default();
    for e in &read.history {
        let seq = match e.context().map(|c| &c.target) {
            Some(agent::ContextTarget::Message { seq }) => Some(*seq),
            _ => None,
        };
        match e {
            agent::HistoryEntry::Assistant { text, .. } => {
                if let Some(seq) = seq {
                    t.messages.push((seq, text.clone()));
                }
            }
            agent::HistoryEntry::Tool { name, args, .. }
                if matches!(name.as_str(), "edit" | "write_file") =>
            {
                if let Some(p) = args.get("path").and_then(|v| v.as_str()) {
                    let p = PathBuf::from(p);
                    t.edited.push(match (&cwd, p.is_relative()) {
                        (Some(c), true) => c.join(p),
                        _ => p,
                    });
                }
            }
            _ => {}
        }
    }
    Ok(t)
}

/// The cells of a markdown text: closed fences whose body parses as a cell, in order. The
/// ordinal of a cell is its 1-based index here (`CellRef::Chat.ordinal`, §3.4).
pub(crate) fn cells_of(text: &str) -> Vec<Presented> {
    fences(text)
        .into_iter()
        .filter_map(|(lang, body)| {
            let cell = surface::cell::parse(lang, body)?;
            Some(Presented {
                digest: surface::cell::digest(body),
                cell,
            })
        })
        .collect()
}

/// Every closed fence `(info string, body)` of `text` in order, CommonMark-style: an opening
/// line of ≥ 3 backticks or tildes (≤ 3 spaces indent), closed by a line of at least as many
/// of the same character. A local copy of `surface`'s crate-private scanner (frozen crate).
fn fences(text: &str) -> Vec<(&str, &str)> {
    let mut lines = Vec::new();
    let mut offset = 0;
    for line in text.split_inclusive('\n') {
        lines.push((offset, line));
        offset += line.len();
    }
    let mut out = Vec::new();
    let mut i = 0;
    while i < lines.len() {
        let (start, line) = lines[i];
        let Some((marker, len, lang)) = opening(line) else {
            i += 1;
            continue;
        };
        let body_start = start + line.len();
        match (i + 1..lines.len()).find(|&j| closing(lines[j].1, marker, len)) {
            Some(j) => {
                out.push((lang, &text[body_start..lines[j].0]));
                i = j + 1;
            }
            None => break, // unclosed: still streaming, never a cell
        }
    }
    out
}

fn unindented(line: &str) -> Option<&str> {
    let n = line.bytes().take_while(|b| *b == b' ').count();
    (n <= 3).then_some(&line[n..])
}

fn opening(line: &str) -> Option<(u8, usize, &str)> {
    let line = unindented(line)?;
    let marker = *line.as_bytes().first()?;
    if marker != b'`' && marker != b'~' {
        return None;
    }
    let len = line.bytes().take_while(|b| *b == marker).count();
    let rest = &line[len..];
    if len < 3 || (marker == b'`' && rest.contains('`')) {
        return None;
    }
    Some((marker, len, rest.trim()))
}

fn closing(line: &str, marker: u8, len: usize) -> bool {
    unindented(line).is_some_and(|l| {
        let n = l.bytes().take_while(|b| *b == marker).count();
        n >= len && l[n..].trim().is_empty()
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ordinals_count_cells_not_fences() {
        let text = "intro\n```bash\necho snippet\n```\n\n```bash\n#| cwd: /tmp\n#| name: a\nls\n```\n\n```json\n{}\n```\n````zsh\n#| cwd: /tmp\npwd\n````\n```bash\n#| cwd: /x\nunclosed\n";
        let cells = cells_of(text);
        assert_eq!(cells.len(), 2);
        assert_eq!(cells[0].cell.options.name.as_deref(), Some("a"));
        assert_eq!(cells[0].cell.code, "ls");
        assert_eq!(
            cells[0].digest,
            surface::cell::digest("#| cwd: /tmp\n#| name: a\nls\n")
        );
        assert_eq!(cells[1].cell.code, "pwd");
    }
}
