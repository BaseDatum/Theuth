//! Note cells as the document crate serves them (design 171 §9.6, §15.2): the code blocks of a
//! live Y.Doc with their last writer (behind [`crate::CodeBlockText`]), `doc_read`'s run
//! annotations through the [`RunAnnotations`] hook, the reader markdown (cell fence → `output`
//! fence → run line, §4.4) and the `#|`-fence validation the `doc_*` writers run before an edit.
//! Run state never enters the Y.Doc; without an installed hook every rendering is today's.

use std::collections::HashMap;

use kameo::actor::ActorRef;
use kameo::message::{Context, Message};
use serde_json::Value;
use surface::cell::{self, Cell, Severity, ValidateCtx};
use yrs::types::xml::XmlOut;
use yrs::{GetString as _, ReadTxn, Transact as _, Xml as _, XmlFragment as _};

use crate::actor::{DocumentActor, Markdown};
use crate::cells::{CellAnnotation, CodeBlockTextReply, NoteKey, RunAnnotations, run_annotations};
use crate::helper::OutlineResult;
use crate::meta::{DocId, Principal};

/// `ask` → `Result<Vec<CodeBlockTextReply>, String>`: every code block of a document in document
/// order (nested ones included), read from the live Y.Doc, each with its last writer.
#[derive(Clone, Debug)]
pub struct CodeBlocks(pub DocId);

impl Message<CodeBlocks> for DocumentActor {
    type Reply = Result<Vec<CodeBlockTextReply>, String>;

    async fn handle(&mut self, m: CodeBlocks, _: &mut Context<Self, Self::Reply>) -> Self::Reply {
        self.code_blocks(&m.0)
    }
}

/// `ask` → `Result<(), String>`: the §3.6 errors of the cells (`#|` fences) in `markdown` about to
/// land in `doc` (`None` = a new note), as the tool result text; `Ok` = nothing to refuse. Scope =
/// the note's cells after the edit (minus the `replacing` blocks) and its first `cells` block.
#[derive(Clone, Debug)]
pub struct CheckCells {
    /// The document, or `None` for `doc_create`.
    pub doc: Option<DocId>,
    /// The markdown to insert.
    pub markdown: String,
    /// Block ids the edit removes (`doc_replace`).
    pub replacing: Vec<String>,
}

impl Message<CheckCells> for DocumentActor {
    type Reply = Result<(), String>;

    async fn handle(&mut self, m: CheckCells, _: &mut Context<Self, Self::Reply>) -> Self::Reply {
        // A document that does not load fails the edit itself, with its own error.
        let mut existing = match &m.doc {
            Some(d) => self.code_blocks(d).unwrap_or_default(),
            None => Vec::new(),
        };
        existing.retain(|b| !m.replacing.contains(&b.block));
        let secrets = self
            .store
            .vault()
            .map(|v| v.list().into_iter().map(|s| s.name).collect());
        match cell_problems(&existing, &m.markdown, secrets).await {
            Some(text) => Err(text),
            None => Ok(()),
        }
    }
}

impl DocumentActor {
    /// Every code block of `doc` (document order) with its last writer: a member from the member-label
    /// map (`meta.authors`), the agent from `agents.json`, else the owner.
    pub(crate) fn code_blocks(&mut self, doc: &DocId) -> Result<Vec<CodeBlockTextReply>, String> {
        let o = self.load(doc).map_err(|e| format!("document {doc}: {e}"))?;
        Ok(blocks_of(
            &o.doc,
            &o.meta.fragment,
            &o.meta.authors,
            &o.agents,
        ))
    }
}

/// [`DocumentActor::code_blocks`] over any Y.Doc and author maps (184 §6.2: also a throwaway copy
/// of the replica merged with the holder's state).
pub(crate) fn blocks_of(
    doc: &yrs::Doc,
    fragment: &str,
    authors: &std::collections::BTreeMap<String, String>,
    agents: &std::collections::BTreeMap<String, uuid::Uuid>,
) -> Vec<CodeBlockTextReply> {
    read_code_blocks(doc, fragment)
        .into_iter()
        .map(|(block, language, text)| {
            let last_author = match (authors.get(&block), agents.get(&block)) {
                (Some(pub_), _) => Principal::Member {
                    pub_: pub_.clone(),
                    device: None,
                },
                (None, Some(session)) => Principal::Agent { session: *session },
                (None, None) => Principal::Owner { device: None },
            };
            CodeBlockTextReply {
                block,
                language,
                text,
                last_author,
                markdown: String::new(),
            }
        })
        .collect()
}

/// `(block id, language, text)` of every BlockNote `codeBlock` under `fragment`, pre-order.
fn read_code_blocks(doc: &yrs::Doc, fragment: &str) -> Vec<(String, String, String)> {
    let frag = doc.get_or_insert_xml_fragment(fragment);
    let txn = doc.transact();
    let mut out = Vec::new();
    for node in frag.children(&txn) {
        walk(&txn, node, &mut out);
    }
    out
}

fn walk<T: ReadTxn>(txn: &T, node: XmlOut, out: &mut Vec<(String, String, String)>) {
    let XmlOut::Element(el) = node else {
        return;
    };
    if el.try_tag().is_some_and(|t| &**t == "blockContainer")
        && let Some(id) = el.get_attribute(txn, "id").map(|v| v.to_string(txn))
    {
        for c in el.children(txn) {
            if let XmlOut::Element(content) = c
                && content.try_tag().is_some_and(|t| &**t == "codeBlock")
            {
                let language = content
                    .get_attribute(txn, "language")
                    .map(|v| v.to_string(txn))
                    .unwrap_or_default();
                let text: String = content
                    .children(txn)
                    .filter_map(|t| match t {
                        XmlOut::Text(t) => Some(t.get_string(txn)),
                        _ => None,
                    })
                    .collect();
                out.push((id.clone(), language, text));
            }
        }
    }
    for c in el.children(txn) {
        walk(txn, c, out);
    }
}

/// The block `key` names among `blocks` (171 §3.4): by block id, or the one cell whose header says
/// `name: <name>` — two such cells are an error naming both.
pub(crate) fn find(
    blocks: Vec<CodeBlockTextReply>,
    key: &NoteKey,
    doc: &DocId,
) -> Result<CodeBlockTextReply, String> {
    match key {
        NoteKey::Block(id) => blocks
            .into_iter()
            .find(|b| b.block == *id)
            .ok_or_else(|| format!("no code block {id} in document {doc}")),
        NoteKey::Name(name) => {
            let mut hits = blocks.into_iter().filter(|b| {
                cell::parse(&b.language, &b.text)
                    .is_some_and(|c| c.options.name.as_deref() == Some(name.as_str()))
            });
            let first = hits
                .next()
                .ok_or_else(|| format!("no cell named `{name}` in document {doc}"))?;
            let rest: Vec<String> = hits.map(|b| b.block).collect();
            if rest.is_empty() {
                Ok(first)
            } else {
                Err(format!(
                    "two cells are named `{name}` in document {doc}: blocks {}, {}",
                    first.block,
                    rest.join(", ")
                ))
            }
        }
    }
}

// ---- doc_read / export ------------------------------------------------------

/// Lossy markdown of `doc` for a reader (171 §4.4): the [`Markdown`] text, and after every cell the
/// installed hook has a run for, an ```` ```output ```` fence (when the run kept text) and the run
/// line. Without a hook, exactly [`Markdown`]'s text. The derived `note.md` never goes through here.
pub async fn reader_markdown(
    actor: &ActorRef<DocumentActor>,
    doc: DocId,
    blocks: Option<Vec<String>>,
    annotate: bool,
) -> anyhow::Result<String> {
    let md = actor
        .ask(Markdown {
            doc: doc.clone(),
            blocks,
            annotate,
        })
        .await
        .map_err(anyhow::Error::msg)?;
    let Some(hook) = run_annotations() else {
        return Ok(md);
    };
    let cells = actor
        .ask(CodeBlocks(doc.clone()))
        .await
        .map_err(anyhow::Error::msg)?;
    Ok(with_runs(&md, &doc, &cells, hook.as_ref()))
}

/// `doc_read`'s outline with the hook installed: each cell's `[code:<lang>] …` row (and tree text)
/// becomes `[cell:<lang> <name>…] <first code line> … (N lines)` (171 §9.6), labels kept.
pub(crate) fn annotate_outline(
    r: &mut OutlineResult,
    doc: &DocId,
    cells: &[CodeBlockTextReply],
    hook: &dyn RunAnnotations,
) {
    let mut rows: HashMap<&str, (String, String)> = HashMap::new();
    for b in cells {
        let Some(cell) = cell::parse(&b.language, &b.text) else {
            continue;
        };
        let ann = hook.note_cell(doc, &b.block, &b.language, &b.text);
        rows.insert(
            b.block.as_str(),
            (
                helper_row(&b.language, &b.text),
                cell_row(&cell, ann.as_ref()),
            ),
        );
    }
    if rows.is_empty() {
        return;
    }
    r.outline = r
        .outline
        .split('\n')
        .map(|line| {
            let body = line.trim_start_matches(' ');
            let indent = &line[..line.len() - body.len()];
            if let Some((id, rest)) = body.split_once(' ')
                && let Some((was, now)) = rows.get(id)
                && let Some(suffix) = rest.strip_prefix(was.as_str())
            {
                format!("{indent}{id} {now}{suffix}")
            } else {
                line.to_owned()
            }
        })
        .collect::<Vec<_>>()
        .join("\n");
    retext(&mut r.blocks, &rows);
}

fn retext(nodes: &mut [Value], rows: &HashMap<&str, (String, String)>) {
    for n in nodes {
        let id = n.get("id").and_then(Value::as_str).unwrap_or("").to_owned();
        if let Some((was, now)) = rows.get(id.as_str())
            && n.get("text").and_then(Value::as_str) == Some(was.as_str())
        {
            n["text"] = Value::String(now.clone());
        }
        if let Some(c) = n.get_mut("children").and_then(Value::as_array_mut) {
            retext(c, rows);
        }
    }
}

/// The helper's outline text of a code block (`packages/documents/src/convert.ts` `outlineLine`).
fn helper_row(language: &str, text: &str) -> String {
    let lines: Vec<&str> = text.split('\n').collect();
    let tag = if language.is_empty() {
        "[code] ".to_owned()
    } else {
        format!("[code:{language}] ")
    };
    if lines.len() > 1 {
        format!("{tag}{} … ({} lines)", lines[0], lines.len())
    } else {
        format!("{tag}{}", lines[0])
    }
}

/// A cell's outline text: the hook's prefix, else `[cell:<lang> <name>]`; then the first code line.
fn cell_row(cell: &Cell, ann: Option<&CellAnnotation>) -> String {
    let prefix = match ann {
        Some(a) => a.outline.clone(),
        None => {
            let lang = format!("{:?}", cell.lang).to_lowercase();
            match &cell.options.name {
                Some(n) => format!("[cell:{lang} {n}]"),
                None => format!("[cell:{lang}]"),
            }
        }
    };
    let lines: Vec<&str> = cell.code.split('\n').collect();
    let n = lines.len();
    let s = if n == 1 { "" } else { "s" };
    format!("{prefix} {} … ({n} line{s})", lines[0])
}

/// `markdown` with each cell fence the hook has a run for followed by its `output` fence and run
/// line. Fences are matched to `cells` in document order by language and exact text.
///
/// 171F4 T2-3: Open as note writes the chat run's history after each cell as plain blocks (an
/// `output` fence, the `ran …` line). Once the cell has a run of its own, that history gives way
/// to it: the reader sees one output, the newest. An empty history fence is never shown.
pub(crate) fn with_runs(
    markdown: &str,
    doc: &DocId,
    cells: &[CodeBlockTextReply],
    hook: &dyn RunAnnotations,
) -> String {
    let mut out = String::with_capacity(markdown.len());
    let (mut copied, mut next) = (0, 0);
    let all = fences(markdown);
    for (k, f) in all.iter().enumerate() {
        let body = f.body.strip_suffix('\n').unwrap_or(f.body);
        let Some(i) = cells[next..].iter().position(|b| {
            b.text == body && f.info.split_whitespace().next() == Some(b.language.trim())
        }) else {
            continue;
        };
        let b = &cells[next + i];
        next += i + 1;
        if cell::parse(&b.language, &b.text).is_none() {
            continue;
        }
        let old = all
            .get(k + 1)
            .filter(|o| o.info == "output" && o.start == next_block(markdown, f.end));
        let Some(a) = hook.note_cell(doc, &b.block, &b.language, &b.text) else {
            if let Some(o) = old.filter(|o| o.body.trim().is_empty()) {
                out.push_str(&markdown[copied..f.end]);
                copied = o.end;
            }
            continue;
        };
        out.push_str(&markdown[copied..f.end]);
        let after = old.map_or(f.end, |o| o.end);
        let at = next_block(markdown, after);
        let line = markdown[at..].lines().next().unwrap_or("");
        copied = if is_run_line(line) {
            at + line.len()
        } else {
            after
        };
        if let Some(text) = &a.output {
            let fence = "`".repeat(longest_backticks(text).max(2) + 1);
            out.push('\n');
            out.push_str(&fence);
            out.push_str("output\n");
            out.push_str(text.trim_end_matches('\n'));
            out.push('\n');
            out.push_str(&fence);
        }
        out.push('\n');
        out.push_str(&a.run_line);
    }
    out.push_str(&markdown[copied..]);
    out
}

/// Start of the next block after `from`: past blank lines and one `<!-- id -->` line.
fn next_block(markdown: &str, from: usize) -> usize {
    let rest = markdown[from..].trim_start_matches(['\n', '\r']);
    let rest = match rest
        .strip_prefix("<!--")
        .and_then(|r| r.split_once("-->\n"))
    {
        Some((_, r)) => r.trim_start_matches(['\n', '\r']),
        None => rest,
    };
    markdown.len() - rest.len()
}

/// `ran 14:38 · …` or `ran — · …` (§4.4), the run line Open as note and the hook write.
fn is_run_line(line: &str) -> bool {
    line.strip_prefix("ran ")
        .and_then(|r| r.split_once(" · "))
        .is_some_and(|(at, _)| {
            at == "—"
                || (at.len() == 5
                    && at.bytes().enumerate().all(|(i, c)| {
                        if i == 2 {
                            c == b':'
                        } else {
                            c.is_ascii_digit()
                        }
                    }))
        })
}

fn longest_backticks(text: &str) -> usize {
    text.split(|c| c != '`').map(str::len).max().unwrap_or(0)
}

// ---- doc_insert / doc_replace / doc_create ------------------------------------

/// The §3.6 errors of the cells in `markdown`, as `cell <n>: <code> — <message>` lines under a
/// refusal; `None` when every cell is valid (or there is none). Warnings are the editor's.
/// `secrets: None` = no vault, `secret_unknown` is skipped. Monty sufficiency is not checked here.
/// 171F3 R4: a shell cell no program on this engine would run (the note's `shell:` default
/// applies) is `no_interpreter`, as in the cell state — resolved once per language.
pub(crate) async fn cell_problems(
    existing: &[CodeBlockTextReply],
    markdown: &str,
    secrets: Option<Vec<String>>,
) -> Option<String> {
    let fences: Vec<Fence<'_>> = fences(markdown);
    let new: Vec<Cell> = fences
        .iter()
        .filter_map(|f| cell::parse(f.info, f.body))
        .collect();
    if new.is_empty() {
        return None;
    }
    let old: Vec<Cell> = existing
        .iter()
        .filter_map(|b| cell::parse(&b.language, &b.text))
        .collect();
    let is_defaults = |lang: &str| lang.split_whitespace().next() == Some("cells");
    let defaults = existing
        .iter()
        .find(|b| is_defaults(&b.language))
        .map(|b| b.text.as_str())
        .or_else(|| fences.iter().find(|f| is_defaults(f.info)).map(|f| f.body))
        .map(cell::parse_defaults);
    let all: Vec<&Cell> = old.iter().chain(&new).collect();
    let names: Vec<String> = all.iter().filter_map(|c| c.options.name.clone()).collect();
    let mut s = String::new();
    let mut missing: Vec<(cell::Lang, Option<String>)> = Vec::new();
    for (i, c) in new.iter().enumerate() {
        let me = old.len() + i;
        let after_edges = all
            .iter()
            .enumerate()
            .filter(|(j, _)| *j != me)
            .filter_map(|(_, c)| Some((c.options.name.clone()?, c.options.after.clone()?)))
            .collect();
        let ctx = ValidateCtx {
            chat: false,
            names: names.clone(),
            after_edges,
            secrets: secrets.clone(),
            defaults: defaults.clone(),
            ..Default::default()
        };
        for p in cell::validate(c, &ctx)
            .into_iter()
            .filter(|p| p.severity == Severity::Error)
        {
            s.push_str(&format!("cell {}: {} — {}\n", i + 1, p.code, p.message));
        }
        let why = match missing.iter().find(|(l, _)| *l == c.lang) {
            Some((_, why)) => why.clone(),
            None => {
                let shell = defaults.as_ref().and_then(|d| d.shell.as_deref());
                let why = tools::jobs::program_missing(c.lang.word(), shell).await;
                missing.push((c.lang, why.clone()));
                why
            }
        };
        if let Some(why) = why {
            s.push_str(&format!("cell {}: no_interpreter — {why}\n", i + 1));
        }
    }
    (!s.is_empty())
        .then(|| format!("nothing changed: a cell does not validate — fix it and retry\n{s}"))
}

/// One closed fenced block: start of the opening line, trimmed info string, body (its last
/// newline included), end of the closing line (newline excluded).
struct Fence<'a> {
    start: usize,
    info: &'a str,
    body: &'a str,
    end: usize,
}

/// Closed fences of `text` the CommonMark way (≤ 3 spaces of indent, ``` or ~~~ ≥ 3, closed by at
/// least as many of the same character); an unclosed fence runs to the end and is skipped.
fn fences(text: &str) -> Vec<Fence<'_>> {
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
        i += 1;
        let Some((ch, n, info)) = opening(line) else {
            continue;
        };
        let Some(j) = (i..lines.len()).find(|&j| closing(lines[j].1, ch, n)) else {
            break;
        };
        let (cstart, cline) = lines[j];
        out.push(Fence {
            start,
            info,
            body: &text[start + line.len()..cstart],
            end: cstart + cline.trim_end_matches(['\n', '\r']).len(),
        });
        i = j + 1;
    }
    out
}

fn opening(line: &str) -> Option<(char, usize, &str)> {
    let l = line.trim_end_matches(['\n', '\r']);
    let t = l.trim_start_matches(' ');
    if l.len() - t.len() > 3 {
        return None;
    }
    let ch = t.chars().next().filter(|c| *c == '`' || *c == '~')?;
    let n = t.len() - t.trim_start_matches(ch).len();
    let info = t[n..].trim();
    (n >= 3 && !(ch == '`' && info.contains('`'))).then_some((ch, n, info))
}

fn closing(line: &str, ch: char, n: usize) -> bool {
    let l = line.trim_end_matches(['\n', '\r']);
    let t = l.trim_start_matches(' ');
    let rest = t.trim_start_matches(ch);
    l.len() - t.len() <= 3 && t.len() - rest.len() >= n && rest.trim().is_empty()
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;
    use crate::cells::{CellLatest, CellLatestOf, CodeBlockText, PutCellLatest};
    use crate::meta::{AgentStyle, Binding, DocMeta, Member, Role};
    use crate::{ApplyUpdate, DeleteNote};
    use base64::Engine as _;
    use kameo::actor::Spawn as _;
    use yrs::{XmlElementPrelim, XmlElementRef, XmlTextPrelim};

    fn reply(block: &str, language: &str, text: &str) -> CodeBlockTextReply {
        CodeBlockTextReply {
            block: block.into(),
            language: language.into(),
            text: text.into(),
            last_author: Principal::Owner { device: None },
            markdown: String::new(),
        }
    }

    /// Runs for cells named `preview` only.
    struct Hook;
    impl RunAnnotations for Hook {
        fn note_cell(
            &self,
            _: &DocId,
            _: &str,
            language: &str,
            text: &str,
        ) -> Option<CellAnnotation> {
            let c = cell::parse(language, text)?;
            (c.options.name.as_deref() == Some("preview")).then(|| CellAnnotation {
                outline: "[cell:bash preview · ran 14:38 · exit 0 · 33 s · expect ✓]".into(),
                output: Some("3 to update\n```\n".into()),
                run_line: "ran 14:38 · exit 0 · 33 s · expect ✓".into(),
            })
        }
    }

    pub(crate) const PREVIEW: &str = "#| name: preview\n#| cwd: /tmp\npulumi preview -s ir-prod1";
    const APPLY: &str = "#| name: apply\n#| cwd: /tmp\n#| after: preview\npulumi up\nexit";

    #[test]
    fn outline_rows_of_cells_are_rewritten_and_labels_kept() {
        let doc = DocId("d".into());
        let cells = [
            reply("b1", "bash", PREVIEW),
            reply("b2", "bash", APPLY),
            reply("b3", "bash", "ls\npwd"),
        ];
        let mut r = OutlineResult {
            outline: [
                "h1 [h1] Runbook",
                "b1 [code:bash] #| name: preview … (3 lines)",
                "  b2 [code:bash] #| name: apply … (5 lines)  [by Alice · doc-member:abcdefgh]",
                "b3 [code:bash] ls … (2 lines)",
            ]
            .join("\n"),
            blocks: vec![serde_json::json!({
                "id": "b1", "type": "codeBlock", "text": "[code:bash] #| name: preview … (3 lines)",
                "children": [{"id": "b2", "type": "codeBlock", "text": "[code:bash] #| name: apply … (5 lines)", "children": []}]
            })],
        };
        annotate_outline(&mut r, &doc, &cells, &Hook);
        assert_eq!(
            r.outline,
            [
                "h1 [h1] Runbook",
                "b1 [cell:bash preview · ran 14:38 · exit 0 · 33 s · expect ✓] pulumi preview -s ir-prod1 … (1 line)",
                "  b2 [cell:bash apply] pulumi up … (2 lines)  [by Alice · doc-member:abcdefgh]",
                "b3 [code:bash] ls … (2 lines)",
            ]
            .join("\n")
        );
        assert_eq!(
            r.blocks[0]["children"][0]["text"],
            "[cell:bash apply] pulumi up … (2 lines)"
        );
    }

    #[test]
    fn reader_markdown_appends_output_and_run_line_only_where_a_run_exists() {
        let doc = DocId("d".into());
        let cells = [
            reply("b3", "cells", "#| cwd: /tmp"),
            reply("b1", "bash", PREVIEW),
            reply("b2", "bash", APPLY),
        ];
        let md = format!(
            "<!-- b0 -->\n# Runbook\n\n<!-- b3 -->\n```cells\n#| cwd: /tmp\n```\n\n<!-- b1 -->\n```bash\n{PREVIEW}\n```\n\n<!-- b2 -->\n```bash\n{APPLY}\n```\n"
        );
        let out = with_runs(&md, &doc, &cells, &Hook);
        // The output holds a ``` line, so its fence is longer.
        assert_eq!(
            out,
            format!(
                "<!-- b0 -->\n# Runbook\n\n<!-- b3 -->\n```cells\n#| cwd: /tmp\n```\n\n<!-- b1 -->\n```bash\n{PREVIEW}\n```\n````output\n3 to update\n```\n````\nran 14:38 · exit 0 · 33 s · expect ✓\n\n<!-- b2 -->\n```bash\n{APPLY}\n```\n"
            )
        );
        // No run anywhere (the hook knows nothing) → byte-identical.
        struct Never;
        impl RunAnnotations for Never {
            fn note_cell(&self, _: &DocId, _: &str, _: &str, _: &str) -> Option<CellAnnotation> {
                None
            }
        }
        assert_eq!(with_runs(&md, &doc, &cells, &Never), md);
        let mut r = OutlineResult {
            outline: "b3 [code:cells] #| cwd: /tmp".into(),
            blocks: vec![],
        };
        annotate_outline(&mut r, &doc, &cells, &Never);
        assert_eq!(
            r.outline, "b3 [code:cells] #| cwd: /tmp",
            "defaults stay as they are"
        );
    }

    #[test]
    fn open_as_note_history_gives_way_to_the_cells_own_run() {
        // 171F4 T2-3: a 2.18.2 Open as note note, its `preview` cell run in the note with output
        // and its quiet `apply` cell not run here (the hook knows `preview` only).
        let doc = DocId("d".into());
        let cells = [
            reply("b1", "bash", PREVIEW),
            reply("b4", "output", "chat said 3 to update"),
            reply("b2", "bash", APPLY),
            reply("b6", "output", ""),
        ];
        let md = format!(
            "<!-- b1 -->\n```bash\n{PREVIEW}\n```\n\n<!-- b4 -->\n```output\nchat said 3 to update\n```\n\n<!-- b5 -->\nran 14:36 · exit 0 · 33 s · expect ✓\n\n<!-- b2 -->\n```bash\n{APPLY}\n```\n\n<!-- b6 -->\n```output\n```\n\n<!-- b7 -->\nran 14:36 · exit 0 · 0.1 s\n"
        );
        assert_eq!(
            with_runs(&md, &doc, &cells, &Hook),
            format!(
                "<!-- b1 -->\n```bash\n{PREVIEW}\n```\n````output\n3 to update\n```\n````\nran 14:38 · exit 0 · 33 s · expect ✓\n\n<!-- b2 -->\n```bash\n{APPLY}\n```\n\n<!-- b7 -->\nran 14:36 · exit 0 · 0.1 s\n"
            )
        );
        // The same without block comments, and a history line alone (a quiet cell composed by
        // 171F4's Open as note) also gives way.
        let md = format!(
            "```bash\n{PREVIEW}\n```\n\nran 14:36 · exit 0 · 0.1 s\n\n```bash\n{APPLY}\n```\n"
        );
        let cells = [reply("b1", "bash", PREVIEW), reply("b2", "bash", APPLY)];
        assert_eq!(
            with_runs(&md, &doc, &cells, &Hook),
            format!(
                "```bash\n{PREVIEW}\n```\n````output\n3 to update\n```\n````\nran 14:38 · exit 0 · 33 s · expect ✓\n\n```bash\n{APPLY}\n```\n"
            )
        );
        // A paragraph that is not a run line stays.
        let md = format!("```bash\n{PREVIEW}\n```\n\nran out of time\n");
        assert!(
            with_runs(&md, &doc, &cells[..1], &Hook).ends_with("expect ✓\n\nran out of time\n")
        );
    }

    #[tokio::test]
    async fn inserted_cells_are_validated_against_the_note() {
        let existing = [reply("b1", "bash", PREVIEW), reply("b9", "bash", "echo")];
        // Valid: `after` names an existing cell.
        let ok =
            "intro\n\n```bash\n#| name: apply\n#| cwd: /tmp\n#| after: preview\npulumi up\n```\n";
        assert_eq!(cell_problems(&existing, ok, None).await, None);
        // Plain code and `cells` blocks are never validated.
        assert_eq!(cell_problems(&[], "```bash\nls\n```\n", None).await, None);
        let bad = "```bash\n#| name: one\n#| cwd: /tmp\necho\n```\n\n```bash\n#| name: preview\n#| after: nope\n#| env: T={{secret:NOPE}}\necho\n```\n";
        let text = cell_problems(&existing, bad, Some(vec!["KEY".into()]))
            .await
            .expect("refused");
        assert_eq!(
            text,
            "nothing changed: a cell does not validate — fix it and retry\n\
             cell 2: cwd_missing — needs `cwd` or a note default\n\
             cell 2: name_duplicate — two cells are named `preview`\n\
             cell 2: secret_unknown — no secret named `NOPE`\n"
        );
        // A note default supplies the cwd; no vault → no secret check.
        let defaults = [reply("b0", "cells", "#| cwd: /srv")];
        let md = "```bash\n#| name: x\n#| env: T={{secret:NOPE}}\necho\n```\n";
        assert_eq!(cell_problems(&defaults, md, None).await, None);
        let md = format!("```cells\n#| cwd: /srv\n```\n\n{md}");
        assert_eq!(cell_problems(&[], &md, None).await, None);
    }

    /// 171F3 R4: an agent write whose shell cell no program on this engine would run is
    /// refused with the cell state's reason; the note's `shell:` default reaches the resolver.
    #[tokio::test]
    async fn inserted_cells_with_no_program_are_refused() {
        tools::jobs::set_program_check(|lang, shell| async move {
            match (lang.as_str(), shell) {
                ("pwsh", None) => Some("no pwsh on this machine".to_owned()),
                ("bash", Some(s)) => Some(format!("the note's shell `{s}` was not found")),
                _ => None,
            }
        });
        let md = "```pwsh\n#| cwd: /tmp\nGet-Date\n```\n\n```bash\n#| cwd: /tmp\nls\n```\n";
        assert_eq!(
            cell_problems(&[], md, None).await.as_deref(),
            Some(
                "nothing changed: a cell does not validate — fix it and retry\n\
                 cell 1: no_interpreter — no pwsh on this machine\n"
            )
        );
        let defaults = [reply("b0", "cells", "#| shell: /opt/nope/bash")];
        let text = cell_problems(&defaults, "```bash\n#| cwd: /tmp\nls\n```\n", None)
            .await
            .expect("refused");
        assert!(
            text.ends_with(
                "cell 1: no_interpreter — the note's shell `/opt/nope/bash` was not found\n"
            ),
            "{text}"
        );
    }

    #[test]
    fn fences_follow_commonmark_basics() {
        let t = "a\n````md\n```bash\nx\n```\n````\n~~~ py\ny\n~~~~\n```open\nz\n";
        let f = fences(t);
        assert_eq!(f.len(), 2);
        assert_eq!((f[0].info, f[0].body), ("md", "```bash\nx\n```\n"));
        assert_eq!((f[1].info, f[1].body), ("py", "y\n"));
        assert_eq!(
            &t[..f[1].end],
            "a\n````md\n```bash\nx\n```\n````\n~~~ py\ny\n~~~~"
        );
    }

    // ---- through the actor, on a real Y.Doc ----

    const MEMBER: &str = "bWVtYmVyLWtleS1hYmNkZWZnaA";

    async fn actor(
        dir: &std::path::Path,
    ) -> (kameo::actor::ActorRef<DocumentActor>, crate::store::Store) {
        let rt = runtime::Runtime::new(figment::Figment::new(), dir.to_path_buf());
        let store = crate::store::Store::open(dir).await.unwrap();
        let (tx, _rx) = tokio::sync::mpsc::unbounded_channel();
        let helper = crate::helper::Helper::new(tx, rt.shutdown.clone());
        let a = DocumentActor::spawn(DocumentActor::new(
            store.clone(),
            helper,
            crate::DocumentsConfig::default(),
            rt,
        ));
        (a, store)
    }

    async fn note(store: &crate::store::Store) -> DocId {
        note_shared(store, true).await
    }

    /// A note; `shared`: with the member [`MEMBER`].
    async fn note_shared(store: &crate::store::Store, shared: bool) -> DocId {
        let id = DocId::for_binding(&Binding::Note);
        let meta = DocMeta {
            derived: Binding::Note.derived(&id),
            id: id.clone(),
            kind: Binding::Note.kind(),
            title: "Runbook".into(),
            binding: Binding::Note,
            fragment: crate::FRAGMENT.into(),
            members: if shared {
                vec![Member {
                    pub_: MEMBER.into(),
                    name: "Alice".into(),
                    color: "#000".into(),
                    role: Role::Editor,
                    added_ms: 1,
                    via: None,
                    last_seen_ms: 1,
                }]
            } else {
                vec![]
            },
            links: vec![],
            agent: AgentStyle::default(),
            last_export: None,
            authors: Default::default(),
            created_ms: 1,
            updated_ms: 1,
            home: None,
            home_ms: 0,
            serve_epoch: 0,
            served_by: None,
            revoked: Default::default(),
        };
        store.create(&meta).await.unwrap();
        id
    }

    /// A BlockNote `blockContainer` holding one `tag` block (a code block when `language` is set).
    pub(crate) fn push_block(
        txn: &mut yrs::TransactionMut,
        group: &XmlElementRef,
        at: Option<u32>,
        id: &str,
        language: Option<&str>,
        text: &str,
    ) {
        let c = match at {
            Some(i) => group.insert(txn, i, XmlElementPrelim::empty("blockContainer")),
            None => group.push_back(txn, XmlElementPrelim::empty("blockContainer")),
        };
        c.insert_attribute(txn, "id", id.to_owned());
        let el = c.push_back(
            txn,
            XmlElementPrelim::empty(if language.is_some() {
                "codeBlock"
            } else {
                "paragraph"
            }),
        );
        if let Some(l) = language {
            el.insert_attribute(txn, "language", l.to_owned());
        }
        el.push_back(txn, XmlTextPrelim::new(text));
    }

    /// The client's edit `f` as a base64 v1 update.
    pub(crate) fn edit(
        client: &yrs::Doc,
        f: impl FnOnce(&mut yrs::TransactionMut, &XmlElementRef),
    ) -> String {
        let frag = client.get_or_insert_xml_fragment(crate::FRAGMENT);
        let mut txn = client.transact_mut();
        let group = match frag.get(&txn, 0) {
            Some(XmlOut::Element(g)) => g,
            _ => frag.push_back(&mut txn, XmlElementPrelim::empty("blockGroup")),
        };
        f(&mut txn, &group);
        base64::engine::general_purpose::STANDARD.encode(txn.encode_update_v1())
    }

    async fn apply(a: &ActorRef<DocumentActor>, doc: &DocId, update: String, by: Principal) {
        a.ask(ApplyUpdate {
            doc: doc.clone(),
            update,
            by,
        })
        .await
        .unwrap();
    }

    async fn text(
        a: &ActorRef<DocumentActor>,
        doc: &DocId,
        key: NoteKey,
    ) -> Result<CodeBlockTextReply, String> {
        a.ask(CodeBlockText {
            doc: doc.clone(),
            key,
            seen: None,
        })
        .await
        .map_err(|e| match e {
            kameo::error::SendError::HandlerError(e) => e,
            e => e.to_string(),
        })
    }

    /// 171 §15.2 / §3.4: a cell is read from the live Y.Doc by name or block id with its full text and
    /// last writer; the name survives an agent replace that mints a new id; a member's edit makes the
    /// member its author; the agent's authorship outlives a restart.
    #[tokio::test]
    async fn code_block_text_reads_the_live_doc_with_its_last_author() {
        let dir = tempfile::tempdir().unwrap();
        let (a, store) = actor(dir.path()).await;
        let doc = note(&store).await;
        let client = yrs::Doc::new();
        let owner = Principal::Owner { device: Some(1) };
        let up = edit(&client, |txn, g| {
            push_block(txn, g, None, "h1", None, "Runbook");
            push_block(txn, g, None, "b1", Some("bash"), PREVIEW);
            push_block(txn, g, None, "b2", Some("bash"), "ls");
        });
        apply(&a, &doc, up, owner.clone()).await;

        let got = text(&a, &doc, NoteKey::Name("preview".into()))
            .await
            .unwrap();
        assert_eq!(
            got,
            CodeBlockTextReply {
                block: "b1".into(),
                language: "bash".into(),
                text: PREVIEW.into(),
                last_author: Principal::Owner { device: None },
                markdown: String::new(),
            }
        );
        assert_eq!(
            text(&a, &doc, NoteKey::Block("b2".into()))
                .await
                .unwrap()
                .text,
            "ls"
        );
        assert_eq!(
            text(&a, &doc, NoteKey::Name("nope".into()))
                .await
                .unwrap_err(),
            format!("no cell named `nope` in document {doc}")
        );
        assert_eq!(
            text(&a, &doc, NoteKey::Block("h1".into()))
                .await
                .unwrap_err(),
            format!("no code block h1 in document {doc}")
        );
        let missing = DocId::for_binding(&Binding::Note);
        assert!(
            text(&a, &missing, NoteKey::Block("b1".into()))
                .await
                .unwrap_err()
                .starts_with(&format!("document {missing}: ")),
        );

        // The agent's doc_replace: b1 goes, b7 (same name, new code) takes its place.
        let session = uuid::Uuid::new_v4();
        let replaced = PREVIEW.replace("ir-prod1", "ir-prod2");
        let up = edit(&client, |txn, g| {
            g.remove_range(txn, 1, 1);
            push_block(txn, g, Some(1), "b7", Some("bash"), &replaced);
        });
        apply(&a, &doc, up, Principal::Agent { session }).await;
        let got = text(&a, &doc, NoteKey::Name("preview".into()))
            .await
            .unwrap();
        assert_eq!(
            (got.block.as_str(), got.text.as_str()),
            ("b7", replaced.as_str())
        );
        assert_eq!(got.last_author, Principal::Agent { session });

        // Alice types into it.
        let up = edit(&client, |txn, g| {
            let Some(XmlOut::Element(c)) = g.get(txn, 1) else {
                panic!("b7")
            };
            let Some(XmlOut::Element(code)) = c.get(txn, 0) else {
                panic!("code")
            };
            let Some(XmlOut::Text(t)) = code.get(txn, 0) else {
                panic!("text")
            };
            let n = t.get_string(txn).chars().count() as u32;
            yrs::Text::insert(&t, txn, n, " --diff");
        });
        apply(
            &a,
            &doc,
            up,
            Principal::Member {
                pub_: MEMBER.into(),
                device: Some(3),
            },
        )
        .await;
        let got = text(&a, &doc, NoteKey::Block("b7".into())).await.unwrap();
        assert!(got.text.ends_with("ir-prod2 --diff"), "{}", got.text);
        assert_eq!(
            got.last_author,
            Principal::Member {
                pub_: MEMBER.into(),
                device: None
            }
        );

        // Two cells named alike → an error naming both.
        let up = edit(&client, |txn, g| {
            push_block(txn, g, None, "b8", Some("bash"), PREVIEW)
        });
        apply(&a, &doc, up, Principal::Agent { session }).await;
        assert_eq!(
            text(&a, &doc, NoteKey::Name("preview".into()))
                .await
                .unwrap_err(),
            format!("two cells are named `preview` in document {doc}: blocks b7, b8")
        );

        // The agent's authorship of b8 is saved with the tick and read back by a new actor.
        a.ask(crate::actor::Tick).await.unwrap();
        a.stop_gracefully().await.unwrap();
        a.wait_for_shutdown().await;
        let (a, _) = actor(dir.path()).await;
        let got = text(&a, &doc, NoteKey::Block("b8".into())).await.unwrap();
        assert_eq!(got.last_author, Principal::Agent { session });
        assert_eq!(
            text(&a, &doc, NoteKey::Block("b7".into()))
                .await
                .unwrap()
                .last_author,
            Principal::Member {
                pub_: MEMBER.into(),
                device: None
            }
        );
    }

    fn row(doc: &DocId, cell: &str, run: &str) -> CellLatest {
        CellLatest {
            id: format!("{doc}:{cell}"),
            doc: doc.clone(),
            cell: cell.into(),
            run: run.into(),
            state: "finished".into(),
            exit: Some(0),
            secs: Some(33.5),
            expect: Some("✓".into()),
            summary: None,
            code_sha: "abc".into(),
            ended_ms: 7,
            text: "3 to update".into(),
            engine: String::new(),
        }
    }

    /// 171 §13.2: an engine upserts a cell's latest row (one per cell), any engine reads them, and
    /// deleting the note removes its rows. 184 §6.4: off the hub too (ungated), the row read back
    /// is its whole `payload` (`engine` inside), and a pre-184 row (`payload = ''`) reads from the
    /// old columns with `engine = ""`.
    #[tokio::test]
    async fn cell_latest_rows_upsert_anywhere_and_go_with_the_note() {
        let dir = tempfile::tempdir().unwrap();
        let (a, store) = actor(dir.path()).await;
        let doc = note(&store).await;
        a.ask(PutCellLatest(row(&doc, "preview", "r_1")))
            .await
            .unwrap();
        a.ask(PutCellLatest(row(&doc, "preview", "r_2")))
            .await
            .unwrap();
        let mut apply = row(&doc, "apply", "r_3");
        apply.exit = None;
        apply.secs = None;
        apply.expect = None;
        apply.summary = Some("2 rows".into());
        apply.text = String::new();
        a.ask(PutCellLatest(apply.clone())).await.unwrap();
        let rows = a.ask(CellLatestOf(doc.clone())).await.unwrap();
        assert_eq!(
            rows,
            vec![apply.clone(), row(&doc, "preview", "r_2")],
            "one row per cell, by id"
        );
        assert!(
            a.ask(CellLatestOf(DocId("other".into())))
                .await
                .unwrap()
                .is_empty()
        );

        a.tell(crate::hub::SelfId(mesh::EngineId("me".into())))
            .await
            .unwrap();
        a.tell(mesh::MeshEvent::Lease(mesh::LeaseState {
            holder: Some(mesh::EngineId("other".into())),
            since_ms: 0,
            reason: "test".into(),
        }))
        .await
        .unwrap();
        let mut theirs = row(&doc, "x", "r_4");
        theirs.engine = "me".into();
        a.ask(PutCellLatest(theirs.clone())).await.unwrap();
        let rows = a.ask(CellLatestOf(doc.clone())).await.unwrap();
        assert_eq!(rows.len(), 3, "an off-hub engine upserts its own run");
        assert_eq!(rows[2], theirs, "with its engine");

        // The old columns of a replicated row may mix two runs; the payload never does.
        sqlx::query("UPDATE cell_latest SET run = 'mixed', text = 'mixed' WHERE id = ?")
            .bind(&theirs.id)
            .execute(store.pool())
            .await
            .unwrap();
        assert_eq!(a.ask(CellLatestOf(doc.clone())).await.unwrap()[2], theirs);
        // A pre-184 writer leaves `payload` empty: the old columns, no engine.
        sqlx::query("UPDATE cell_latest SET payload = '' WHERE id = ?")
            .bind(&theirs.id)
            .execute(store.pool())
            .await
            .unwrap();
        let old = &a.ask(CellLatestOf(doc.clone())).await.unwrap()[2];
        assert_eq!((old.run.as_str(), old.engine.as_str()), ("mixed", ""));
        a.tell(mesh::MeshEvent::Lease(mesh::LeaseState {
            holder: Some(mesh::EngineId("me".into())),
            since_ms: 0,
            reason: "test".into(),
        }))
        .await
        .unwrap();

        a.ask(DeleteNote { doc: doc.clone() }).await.unwrap();
        assert!(store.cell_latest(&doc).await.unwrap().is_empty());
    }

    // ---- 184 §6.2: the block is read on the document's server ------------------------------

    type Inbound = (replica::PeerId, Box<dyn replica::ByteStream>);

    /// Engine `srv` on a fake mesh: every docs stream answers `Block` with `block` and `Open` with
    /// `open`, and `Closed{not served here}` (an engine that does not serve that tag) for `None` or
    /// any other frame. Records the `seen` of each `Block`.
    struct Srv {
        block: Option<CodeBlockTextReply>,
        open: Option<crate::meta::DocOpen>,
        seen: std::sync::Arc<parking_lot::Mutex<Vec<Option<String>>>>,
        events: tokio::sync::broadcast::Sender<replica::PeerEvent>,
    }

    impl Srv {
        fn new(
            block: Option<CodeBlockTextReply>,
            open: Option<crate::meta::DocOpen>,
        ) -> std::sync::Arc<Self> {
            std::sync::Arc::new(Self {
                block,
                open,
                seen: Default::default(),
                events: tokio::sync::broadcast::channel(8).0,
            })
        }
    }

    #[async_trait::async_trait]
    impl replica::Transport for Srv {
        fn peers(&self) -> Vec<replica::PeerId> {
            vec![replica::PeerId("srv".into())]
        }

        async fn open(
            &self,
            _: &replica::PeerId,
            _: &str,
        ) -> anyhow::Result<Box<dyn replica::ByteStream>> {
            use crate::relay::Msg;
            use futures::{SinkExt as _, StreamExt as _};
            let (mine, theirs) = tokio::io::duplex(1 << 20);
            let (block, open, seen) = (self.block.clone(), self.open.clone(), self.seen.clone());
            tokio::spawn(async move {
                let mut wire = crate::relay::framed(Box::new(theirs));
                while let Some(Ok(frame)) = wire.next().await {
                    let (doc, bytes) = match Msg::decode(&frame) {
                        Ok(Msg::Hello { .. }) => continue,
                        Ok(Msg::Block { doc, seen: s, .. }) => {
                            seen.lock().push(s);
                            let ok = block.as_ref().map(|b| Ok::<_, String>(b.clone()));
                            (doc, ok.map(|r| serde_json::to_vec(&r).unwrap()))
                        }
                        Ok(Msg::Open { doc, .. }) => {
                            (doc, open.as_ref().map(|o| serde_json::to_vec(o).unwrap()))
                        }
                        _ => (String::new(), None),
                    };
                    let msg = match bytes {
                        Some(bytes) => Msg::Event { doc, bytes },
                        None => Msg::Closed {
                            doc,
                            reason: "not served here".into(),
                        },
                    };
                    let _ = wire.send(msg.encode().unwrap()).await;
                }
            });
            Ok(Box::new(mine))
        }

        async fn accept(&self, _: &str) -> anyhow::Result<Inbound> {
            std::future::pending().await
        }

        fn subscribe(&self) -> tokio::sync::broadcast::Receiver<replica::PeerEvent> {
            self.events.subscribe()
        }
    }

    /// A note (`shared`: with a member) holding `b1` from `update`, written while `a` serves it.
    async fn written(
        a: &ActorRef<DocumentActor>,
        store: &crate::store::Store,
        shared: bool,
        update: String,
    ) -> DocId {
        let doc = note_shared(store, shared).await;
        apply(a, &doc, update, Principal::Owner { device: Some(1) }).await;
        doc
    }

    /// `a` loses the lease to `srv` and reaches the mesh through `t`.
    async fn off_server(a: &ActorRef<DocumentActor>, t: std::sync::Arc<Srv>) {
        a.tell(crate::hub::SelfId(mesh::EngineId("me".into())))
            .await
            .unwrap();
        a.tell(mesh::MeshEvent::Lease(mesh::LeaseState {
            holder: Some(mesh::EngineId("srv".into())),
            since_ms: 0,
            reason: "test".into(),
        }))
        .await
        .unwrap();
        a.tell(crate::relay::SetTransport(t)).await.unwrap();
    }

    async fn press(a: &ActorRef<DocumentActor>, doc: &DocId, seen: &str) -> CodeBlockTextReply {
        a.ask(CodeBlockText {
            doc: doc.clone(),
            key: NoteKey::Name("preview".into()),
            seen: Some(seen.to_owned()),
        })
        .await
        .unwrap()
    }

    fn first_block(client: &yrs::Doc) -> String {
        edit(client, |txn, g| {
            push_block(txn, g, None, "b1", Some("bash"), PREVIEW)
        })
    }

    /// `client`'s edit appending `more` to `b1`'s text.
    fn append(client: &yrs::Doc, more: &str) -> String {
        edit(client, |txn, g| {
            let Some(XmlOut::Element(c)) = g.get(txn, 0) else {
                panic!("b1")
            };
            let Some(XmlOut::Element(code)) = c.get(txn, 0) else {
                panic!("code")
            };
            let Some(XmlOut::Text(t)) = code.get(txn, 0) else {
                panic!("text")
            };
            let n = t.get_string(txn).chars().count() as u32;
            yrs::Text::insert(&t, txn, n, more);
        })
    }

    /// 184 §6.2 (W-u, W-i): off the server, a press reads the block and the note's markdown from
    /// the server over a `Block` leg carrying the press's digest — not the stale local copy.
    #[tokio::test]
    async fn off_the_server_a_press_reads_the_servers_block() {
        let dir = tempfile::tempdir().unwrap();
        let (a, store) = actor(dir.path()).await;
        let doc = written(&a, &store, true, first_block(&yrs::Doc::new())).await;
        let fresh = PREVIEW.replace("ir-prod1", "ir-prod2");
        let srv = Srv::new(
            Some(CodeBlockTextReply {
                markdown: "# Runbook, as served".into(),
                ..reply("b1", "bash", &fresh)
            }),
            None,
        );
        off_server(&a, srv.clone()).await;
        let digest = cell::digest(&fresh);
        let got = press(&a, &doc, &digest).await;
        assert_eq!(
            (got.text.as_str(), got.markdown.as_str()),
            (fresh.as_str(), "# Runbook, as served")
        );
        assert_eq!(
            *srv.seen.lock(),
            vec![Some(digest)],
            "the digest went along"
        );
    }

    /// 184 §6.2 (W-i), §13 #43: the server and the holder both unreachable → the replica's block;
    /// a shared note's author is unknown (`?`: the press confirms), an unshared one's is not.
    #[tokio::test]
    async fn nobody_answers_a_shared_note_reads_here_with_its_author_unknown() {
        for (shared, author) in [
            (
                true,
                Principal::Member {
                    pub_: crate::cells::UNKNOWN_AUTHOR.into(),
                    device: None,
                },
            ),
            (false, Principal::Owner { device: None }),
        ] {
            let dir = tempfile::tempdir().unwrap();
            let (a, store) = actor(dir.path()).await;
            let doc = written(&a, &store, shared, first_block(&yrs::Doc::new())).await;
            off_server(&a, Srv::new(None, None)).await;
            let got = press(&a, &doc, &cell::digest(PREVIEW)).await;
            assert_eq!((got.text.as_str(), got.markdown.as_str()), (PREVIEW, ""));
            assert_eq!(got.last_author, author, "shared: {shared}");
        }
    }

    /// 184 §6.2 (W-i): a server that does not serve `Block` (an older engine) → the replica, merged
    /// with the holder's `DocOpen` state, the block's member author from the holder's meta.
    #[tokio::test]
    async fn an_old_server_the_holder_names_the_author() {
        let dir = tempfile::tempdir().unwrap();
        let (a, store) = actor(dir.path()).await;
        let client = yrs::Doc::new();
        let doc = written(&a, &store, true, first_block(&client)).await;
        let mut meta = store.read_meta(&doc).unwrap();
        meta.authors.insert("b1".into(), MEMBER.into());
        let open = crate::meta::DocOpen {
            meta,
            state: vec![append(&client, " --diff")],
            awareness: String::new(),
            server: None,
            dual: None,
            sv: String::new(),
        };
        off_server(&a, Srv::new(None, Some(open))).await;
        let got = press(&a, &doc, "stale").await;
        assert_eq!(got.text, format!("{PREVIEW} --diff"));
        assert_eq!(
            got.last_author,
            Principal::Member {
                pub_: MEMBER.into(),
                device: None
            }
        );
    }

    /// 184 §16, W-u: the engine that serves the note (a lone engine) reads its own live Doc. A read
    /// without a digest is today's reply; a press whose digest the Doc lacks is answered by the
    /// update that brings it, or after `BLOCK_WAIT_MS` with what the Doc has.
    #[tokio::test]
    async fn the_server_reads_its_own_doc_and_waits_for_the_keystroke() {
        let dir = tempfile::tempdir().unwrap();
        let (a, store) = actor(dir.path()).await;
        let client = yrs::Doc::new();
        let doc = written(&a, &store, true, first_block(&client)).await;
        let plain = text(&a, &doc, NoteKey::Name("preview".into()))
            .await
            .unwrap();
        assert_eq!(plain, reply("b1", "bash", PREVIEW));

        let wait = std::time::Duration::from_millis(crate::hub::BLOCK_WAIT_MS);
        let t = std::time::Instant::now();
        assert_eq!(press(&a, &doc, "stale").await.text, PREVIEW);
        assert!(t.elapsed() >= wait, "waited for a matching update");

        let typed = format!("{PREVIEW} x");
        let (a2, doc2, digest) = (a.clone(), doc.clone(), cell::digest(&typed));
        let t = std::time::Instant::now();
        let pressed = tokio::spawn(async move { press(&a2, &doc2, &digest).await });
        tokio::time::sleep(std::time::Duration::from_millis(100)).await;
        apply(
            &a,
            &doc,
            append(&client, " x"),
            Principal::Owner { device: Some(1) },
        )
        .await;
        assert_eq!(pressed.await.unwrap().text, typed);
        assert!(t.elapsed() < wait, "answered by the update");
    }

    /// 184 §10.2, rule 6: a `Block` read relayed here (a leg's `Relayed(CodeBlockText)`) for a
    /// note this engine does not serve is refused — the caller reads its own replica — and never
    /// answered from this engine's replica nor relayed on.
    #[tokio::test]
    async fn a_relayed_read_off_the_server_is_refused() {
        let dir = tempfile::tempdir().unwrap();
        let (a, store) = actor(dir.path()).await;
        let doc = written(&a, &store, false, first_block(&yrs::Doc::new())).await;
        let srv = Srv::new(Some(reply("b1", "bash", "from srv")), None);
        off_server(&a, srv.clone()).await;
        let e = a
            .ask(crate::hub::Relayed(CodeBlockText {
                doc: doc.clone(),
                key: NoteKey::Name("preview".into()),
                seen: None,
            }))
            .await
            .unwrap_err();
        assert_eq!(e.to_string(), "NotHub:srv");
        assert!(srv.seen.lock().is_empty(), "not relayed on");
    }

    /// 184 §9.2 legs are keyed by engine: after the home's `Block` leg went silent, the fallback's
    /// `Open` goes to the holder on a leg of its own (not down the home's leg), and the author
    /// comes from the holder's meta.
    #[tokio::test]
    async fn the_fallback_open_reaches_the_holder_past_a_silent_home() {
        struct Two {
            srv: std::sync::Arc<Srv>,
            opened: parking_lot::Mutex<Vec<String>>,
        }
        #[async_trait::async_trait]
        impl replica::Transport for Two {
            fn peers(&self) -> Vec<replica::PeerId> {
                vec![
                    replica::PeerId("home".into()),
                    replica::PeerId("srv".into()),
                ]
            }
            async fn open(
                &self,
                p: &replica::PeerId,
                alpn: &str,
            ) -> anyhow::Result<Box<dyn replica::ByteStream>> {
                self.opened.lock().push(p.0.clone());
                if p.0 == "srv" {
                    return self.srv.open(p, alpn).await;
                }
                // The home takes the stream and never answers (an engine that is stuck).
                let (mine, theirs) = tokio::io::duplex(1 << 20);
                tokio::spawn(async move {
                    use futures::StreamExt as _;
                    let mut wire = crate::relay::framed(Box::new(theirs));
                    while wire.next().await.is_some() {}
                });
                Ok(Box::new(mine))
            }
            async fn accept(&self, _: &str) -> anyhow::Result<Inbound> {
                std::future::pending().await
            }
            fn subscribe(&self) -> tokio::sync::broadcast::Receiver<replica::PeerEvent> {
                self.srv.events.subscribe()
            }
        }

        let dir = tempfile::tempdir().unwrap();
        let (a, store) = actor(dir.path()).await;
        let doc = note_shared(&store, true).await;
        let mut meta = store.read_meta(&doc).unwrap();
        meta.home = Some(mesh::EngineId("home".into()));
        meta.home_ms = 1;
        store.write_meta(&meta).await.unwrap();
        let client = yrs::Doc::new();
        // Homes off while this engine writes the block (it holds the lease alone).
        apply(
            &a,
            &doc,
            first_block(&client),
            Principal::Owner { device: Some(1) },
        )
        .await;
        meta.authors.insert("b1".into(), MEMBER.into());
        let open = crate::meta::DocOpen {
            meta,
            state: vec![append(&client, " --diff")],
            awareness: String::new(),
            server: None,
            dual: None,
            sv: String::new(),
        };
        let two = std::sync::Arc::new(Two {
            srv: Srv::new(None, Some(open)),
            opened: Default::default(),
        });
        off_server(&a, two.srv.clone()).await;
        a.tell(crate::relay::SetTransport(two.clone()))
            .await
            .unwrap();
        a.tell(crate::hub::HomesOn(true)).await.unwrap();
        let got = press(&a, &doc, "stale").await;
        assert_eq!(got.text, format!("{PREVIEW} --diff"), "the holder's state");
        assert_eq!(
            got.last_author,
            Principal::Member {
                pub_: MEMBER.into(),
                device: None
            }
        );
        assert_eq!(*two.opened.lock(), ["home", "srv"]);
    }
}
