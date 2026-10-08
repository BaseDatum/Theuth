// 144-W3 owns this file.
//! Citation validation at `doc_insert` / `doc_replace` / `doc_create` (design 144 §6.6): every
//! `](theuth:` link of inserted markdown is resolved on the hub (`CiteResolve`); a `missing` /
//! `unavailable` target is refused with the nearby block ids, `changed` is accepted (the chip will
//! say so), and a `?s=<sha8>` is added to every block citation that lacks one so promoted answers
//! carry drift information from day one. `knowledge` / `sessions` / `meetings` targets are checked
//! for existence only (no text, no hash).

use std::collections::HashMap;

use codeindex::LocateStatus;
use kameo::actor::ActorRef;
use surface::anchor::{Locator, THEUTH_SCHEME, TheuthAnchor};
use tools::ToolOutput;

use crate::actor::{DocumentActor, Markdown};
use crate::meta::{DocId, Principal};
use crate::notebook::{Citation, CiteResolve};

/// The `theuth://` links of `markdown` the edit must check: every `](theuth://…)` href that parses,
/// deduplicated, in document order. Malformed hrefs are not here; [`check`] refuses them.
pub fn check_theuth_links(markdown: &str) -> Vec<TheuthAnchor> {
    let mut out: Vec<TheuthAnchor> = Vec::new();
    for (_, parsed) in links(markdown) {
        if let Ok(a) = parsed
            && !out.contains(&a)
        {
            out.push(a);
        }
    }
    out
}

/// Every `](theuth://…)` href as written, once each, with its parse.
pub fn links(markdown: &str) -> Vec<(&str, Result<TheuthAnchor, String>)> {
    let mut out: Vec<(&str, Result<TheuthAnchor, String>)> = Vec::new();
    for (i, _) in markdown.match_indices("](") {
        let Some(href) = markdown[i + 2..].split(')').next() else {
            continue;
        };
        if href.starts_with(THEUTH_SCHEME) && !out.iter().any(|(h, _)| *h == href) {
            out.push((href, TheuthAnchor::parse(href)));
        }
    }
    out
}

/// §6.6 at the three writers: resolve every citation of `markdown` through the hub as `by`. `Some(refusal)`
/// when a link is malformed or its target is `missing` / `unavailable`; else `None` with `markdown`
/// rewritten so each block citation carries `?s=<sha8>` of the block's text now. Fails open — no
/// citations, or a resolver that cannot answer (off the hub, not started) — because only a real dead
/// target may stop a write.
pub async fn check(
    actor: &ActorRef<DocumentActor>,
    markdown: &mut String,
    by: Principal,
) -> Option<ToolOutput> {
    let links = links(markdown);
    if links.is_empty() {
        return None;
    }
    let mut bad = String::new();
    let mut good: Vec<(&str, TheuthAnchor)> = Vec::new();
    for (raw, parsed) in links {
        match parsed {
            Ok(a) => good.push((raw, a)),
            Err(e) => bad.push_str(&format!("- `{raw}`: {e}\n")),
        }
    }
    if !bad.is_empty() {
        return Some(refused(bad));
    }
    let hrefs = good.iter().map(|(_, a)| a.href()).collect();
    let cites = match actor.ask(CiteResolve { hrefs, by }).await {
        Ok(c) => c,
        Err(e) => {
            tracing::debug!(target: "document", error = %e, "citation check skipped");
            return None;
        }
    };
    // The refusal names the last block ids of every document a missing block citation points at.
    let mut near: HashMap<String, Vec<String>> = HashMap::new();
    for ((_, a), c) in good.iter().zip(&cites) {
        if c.available
            && c.status == LocateStatus::Missing
            && matches!(a.locator, Some(Locator::Block(_)))
            && !near.contains_key(&a.id)
            && let Some(doc) = DocId::parse(&a.id)
        {
            near.insert(a.id.clone(), nearby_blocks(actor, doc).await);
        }
    }
    match judge(&good, &cites, &near) {
        Err(lines) => Some(refused(lines)),
        Ok(rewrites) => {
            for (from, to) in rewrites {
                *markdown = markdown.replace(&from, &to);
            }
            None
        }
    }
}

/// The pure core of [`check`]: `links` (as written, parsed) against their resolved `cites` (positionally
/// aligned). `Err` is the refusal body — one line per dead target; `Ok` the `(from, to)` rewrites that add
/// `?s=` to block citations that lack one. `near` maps a document id to its last block ids for the message.
pub fn judge(
    links: &[(&str, TheuthAnchor)],
    cites: &[Citation],
    near: &HashMap<String, Vec<String>>,
) -> Result<Vec<(String, String)>, String> {
    let mut lines = String::new();
    let mut rewrites = Vec::new();
    for ((raw, a), c) in links.iter().zip(cites) {
        if let Some(line) = dead(a, c, near) {
            lines.push_str(&format!("- {line}\n"));
            continue;
        }
        if a.sha8.is_none()
            && matches!(a.locator, Some(Locator::Block(_)))
            && let Some(text) = &c.text
        {
            rewrites.push((format!("]({raw})"), format!("]({raw}?s={})", sha8(text))));
        }
    }
    if lines.is_empty() {
        Ok(rewrites)
    } else {
        Err(lines)
    }
}

/// First 8 hex of `sha256(text)` with trailing whitespace trimmed — the `?s=` drift hash (§6.4).
pub fn sha8(text: &str) -> String {
    codeindex::locate::text_sha(text.trim_end())[..8].to_owned()
}

/// The refusal line for a citation whose target is gone, `None` for `same` / `changed` / `moved`.
fn dead(a: &TheuthAnchor, c: &Citation, near: &HashMap<String, Vec<String>>) -> Option<String> {
    let href = a.href();
    if !c.available {
        return Some(format!(
            "{href}: {} unavailable (trashed or not readable)",
            a.kind.as_str().trim_end_matches('s')
        ));
    }
    if c.status != LocateStatus::Missing {
        return None;
    }
    Some(match &a.locator {
        Some(Locator::Block(_)) => match near.get(&a.id).filter(|n| !n.is_empty()) {
            Some(ids) => format!(
                "{href}: block not found; blocks near the end are {}",
                ids.join(", ")
            ),
            None => format!("{href}: block not found"),
        },
        Some(Locator::Time(_)) => format!("{href}: no segment at that time (past the end)"),
        Some(Locator::Turn(_)) => format!("{href}: no such turn"),
        None => format!("{href}: not found"),
    })
}

/// The last three block ids of `doc` (its annotated markdown's `<!-- id -->` marks), for the refusal.
async fn nearby_blocks(actor: &ActorRef<DocumentActor>, doc: DocId) -> Vec<String> {
    let Ok(text) = actor
        .ask(Markdown {
            doc,
            blocks: None,
            annotate: true,
        })
        .await
    else {
        return Vec::new();
    };
    let ids: Vec<String> = text
        .lines()
        .filter_map(|l| l.strip_prefix("<!-- "))
        .filter_map(|l| l.split_whitespace().next())
        .map(str::to_owned)
        .collect();
    ids[ids.len().saturating_sub(3)..].to_vec()
}

fn refused(lines: String) -> ToolOutput {
    ToolOutput::err(format!(
        "nothing changed: a theuth:// citation has no live target — cite a block you were given or read back the document and retry\n{lines}"
    ))
}
