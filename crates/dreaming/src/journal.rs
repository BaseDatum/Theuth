//! The `Dreaming` journal (design doc 136 §8, §4 phase 7). The text of
//! record is `<data_dir>/dreaming/journal.md` (dreaming's own file, so a
//! night without the documents plugin still journals); after every write
//! the whole text is pushed to the `Dreaming` document in the documents
//! store — created once (`CreateNote`), its id kept in `state.json`, then
//! replaced (`ImportMarkdown{mode: Replace}`). Shape:
//!
//! ```text
//! # Dreaming
//!
//! ## 2026-10-08 · nightly
//! ### Tidy
//! …
//! ### Undone
//! …
//! ## 2026-10-07 · nightly
//! ```
//!
//! The newest night is at the top; inside a night, sections are appended in
//! the order they arrive; a section named again gets its markdown appended.

use std::path::{Path, PathBuf};

use kameo::actor::ActorRef;

use crate::night::{read_state, write_atomic, write_state};
use crate::scrub::scrub;
use crate::types::NightId;

/// The document's title.
pub const TITLE: &str = "Dreaming";

/// `<data_dir>/dreaming/journal.md`.
pub fn path(data_dir: &Path) -> PathBuf {
    crate::night::dir(data_dir).join("journal.md")
}

/// The whole journal text ("" when absent).
pub fn text(data_dir: &Path) -> String {
    std::fs::read_to_string(path(data_dir)).unwrap_or_default()
}

fn night_heading(night: &str) -> String {
    format!("## {night}")
}

/// Insert `markdown` under `### section` of the `## <night>` block, creating
/// the block at the top (after the title) when absent. `mode` labels a new
/// night block (`nightly` | `pass` | `backlog`).
pub fn insert(journal: &str, night: &str, mode: &str, section: &str, markdown: &str) -> String {
    let markdown = markdown.trim_end();
    let section_heading = format!("### {section}");
    let mut lines: Vec<String> = journal.lines().map(str::to_owned).collect();
    if lines.is_empty() {
        lines.push(format!("# {TITLE}"));
        lines.push(String::new());
    }
    let heading = night_heading(night);
    let night_start = lines
        .iter()
        .position(|l| l == &heading || l.starts_with(&format!("{heading} ·")));
    let (start, end) = match night_start {
        Some(s) => {
            let e = lines[s + 1..]
                .iter()
                .position(|l| l.starts_with("## "))
                .map_or(lines.len(), |i| s + 1 + i);
            (s, e)
        }
        None => {
            // New block after the title line and its blank.
            let at = lines
                .iter()
                .position(|l| l.starts_with("## "))
                .unwrap_or(lines.len());
            let block = vec![format!("{heading} · {mode}"), String::new()];
            let n = block.len();
            lines.splice(at..at, block);
            (at, at + n)
        }
    };
    // Within [start, end): the block is `heading, "", (### X, body…, "")*`.
    let sec = lines[start..end]
        .iter()
        .position(|l| l == &section_heading)
        .map(|i| start + i);
    // A `#`/`##`/`###` heading inside the body would read as a night or
    // section boundary on the next insert: demote it under the section.
    let mut body: Vec<String> = markdown
        .lines()
        .map(|l| {
            let hashes = l.bytes().take_while(|b| *b == b'#').count();
            if (1..=3).contains(&hashes) && l[hashes..].starts_with(' ') {
                format!("####{}", &l[hashes..])
            } else {
                l.to_owned()
            }
        })
        .collect();
    match sec {
        Some(s) => {
            let e = lines[s + 1..end]
                .iter()
                .position(|l| l.starts_with("### "))
                .map_or(end, |i| s + 1 + i);
            // Append before the section's trailing blank.
            let mut at = e;
            while at > s + 1 && lines[at - 1].trim().is_empty() {
                at -= 1;
            }
            lines.splice(at..at, body);
        }
        None => {
            let mut block = vec![section_heading];
            block.append(&mut body);
            block.push(String::new());
            lines.splice(end..end, block);
        }
    }
    let mut out = lines.join("\n");
    out.push('\n');
    out
}

/// Append `markdown` under `section` of tonight's entry, then mirror the
/// journal into the `Dreaming` document when the documents plugin runs.
pub async fn append(
    data_dir: &Path,
    night: &NightId,
    section: &str,
    markdown: &str,
) -> anyhow::Result<()> {
    let text = append_file(data_dir, night, section, markdown)?;
    mirror(data_dir, &text).await;
    Ok(())
}

/// [`append`] from a sync path (the server-side counters, 136 D10): the
/// file only; the next `append` mirrors the whole text, so the document
/// catches up at the latest with the night's `Receipt` line.
pub fn append_sync(data_dir: &Path, night: &NightId, section: &str, markdown: &str) {
    if let Err(e) = append_file(data_dir, night, section, markdown) {
        tracing::warn!("dreaming: journal line not written: {e}");
    }
}

fn append_file(
    data_dir: &Path,
    night: &NightId,
    section: &str,
    markdown: &str,
) -> anyhow::Result<String> {
    let mode = read_state(data_dir)
        .last_night
        .as_ref()
        .filter(|n| &n.night == night)
        .map_or_else(|| "nightly".to_owned(), |n| n.mode.clone());
    let text = insert(&text(data_dir), night, &mode, section, &scrub(markdown));
    write_atomic(&path(data_dir), text.as_bytes())?;
    Ok(text)
}

/// Push the journal text into the documents store (best effort).
async fn mirror(data_dir: &Path, text: &str) {
    let Ok(Some(docs)) = ActorRef::<document::DocumentActor>::lookup(document::DOCUMENT_ACTOR)
    else {
        return;
    };
    let mut state = read_state(data_dir);
    let doc = match state.journal.as_deref().and_then(document::DocId::parse) {
        Some(id) => id,
        None => {
            match docs
                .ask(document::CreateNote {
                    title: TITLE.into(),
                    markdown: text.to_owned(),
                    by: document::Principal::Helper,
                })
                .await
            {
                Ok(summary) => {
                    state.journal = Some(summary.id.as_str().to_owned());
                    if let Err(e) = write_state(data_dir, &state) {
                        tracing::warn!("dreaming: journal id not saved: {e}");
                    }
                }
                Err(e) => tracing::warn!("dreaming: journal document not created: {e}"),
            }
            return;
        }
    };
    match docs
        .ask(document::ImportMarkdown {
            doc,
            markdown: text.to_owned(),
            by: document::Principal::Helper,
            mode: document::ImportMode::Replace,
        })
        .await
    {
        Ok(_) => {}
        Err(e) => tracing::warn!("dreaming: journal document not updated: {e}"),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn nights_prepend_sections_append_and_secrets_are_scrubbed() {
        let j = insert("", "2026-10-07", "nightly", "Tidy", "- 3 merged");
        assert_eq!(
            j,
            "# Dreaming\n\n## 2026-10-07 · nightly\n\n### Tidy\n- 3 merged\n\n"
        );
        let j = insert(&j, "2026-10-07", "nightly", "Mine", "- 2 cases");
        let j = insert(&j, "2026-10-07", "nightly", "Tidy", "- 1 archived");
        assert_eq!(
            j,
            "# Dreaming\n\n## 2026-10-07 · nightly\n\n### Tidy\n- 3 merged\n- 1 archived\n\n### Mine\n- 2 cases\n\n"
        );
        let j = insert(&j, "2026-10-08", "backlog", "Journal", "spent $0.40");
        assert!(j.starts_with("# Dreaming\n\n## 2026-10-08 · backlog\n\n### Journal\nspent $0.40\n\n## 2026-10-07 · nightly\n"), "{j}");
        // Adding to the older night lands in its block, not the newest.
        let j = insert(&j, "2026-10-07", "nightly", "Undone", "merge undone");
        let newer = j.find("## 2026-10-08").unwrap();
        let older = j.find("## 2026-10-07").unwrap();
        let undone = j.find("### Undone").unwrap();
        assert!(newer < older && older < undone, "{j}");
        // Headings inside a body are demoted, so a later insert finds the
        // right block (the live night wrote `## Tidy` … `## Summary` inside
        // its `journal` section and the Undone line landed above them).
        let j = insert(
            "",
            "2026-10-08",
            "pass",
            "journal",
            "## Tidy\n\ntext\n\n### Merged\n- x\n# Summary\ndone\nnot # a heading",
        );
        let j = insert(&j, "2026-10-08", "pass", "Undone", "- whole night");
        assert_eq!(
            j,
            "# Dreaming\n\n## 2026-10-08 · pass\n\n### journal\n#### Tidy\n\ntext\n\n#### Merged\n- x\n#### Summary\ndone\nnot # a heading\n\n### Undone\n- whole night\n\n"
        );
    }

    #[tokio::test]
    async fn append_writes_the_file_and_tolerates_no_document_actor() {
        let dir = tempfile::tempdir().unwrap();
        append(
            dir.path(),
            &"2026-10-08".to_owned(),
            "Tidy",
            "token sk-abcdefghijklmnop via {{secret:OPENAI_KEY}}",
        )
        .await
        .unwrap();
        let t = text(dir.path());
        assert!(t.contains("### Tidy\ntoken [redacted] via [secret]"), "{t}");
        assert!(!t.contains("OPENAI_KEY"));
        assert!(t.contains("## 2026-10-08 · nightly"));
    }
}
