// 144-W3 owns this file.
//! Citation validation at `doc_insert` / `doc_replace` / `doc_create` (design 144 §6.6): every
//! `](theuth:` link of inserted markdown is resolved on the hub (`CiteResolve`); a `missing` /
//! `unavailable` target is refused with the nearby block ids, `changed` is accepted (the chip will
//! say so), and a `?s=<sha8>` is added to every block citation that lacks one so promoted answers
//! carry drift information from day one. `knowledge` / `sessions` / `meetings` targets are checked
//! for existence only (no text, no hash).

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

/// Every `](theuth://…)` href as written, with its parse.
fn links(markdown: &str) -> Vec<(&str, Result<TheuthAnchor, String>)> {
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
    let mut s = String::new();
    let mut good: Vec<(&str, TheuthAnchor)> = Vec::new();
    for (raw, parsed) in links {
        match parsed {
            Ok(a) => good.push((raw, a)),
            Err(e) => s.push_str(&format!("- `{raw}`: {e}\n")),
        }
    }
    if !s.is_empty() {
        return Some(refused(s));
    }
    let hrefs = good.iter().map(|(_, a)| a.href()).collect();
    let cites = match actor.ask(CiteResolve { hrefs, by }).await {
        Ok(c) => c,
        Err(e) => {
            tracing::debug!(target: "document", error = %e, "citation check skipped");
            return None;
        }
    };
    let mut rewrites: Vec<(String, String)> = Vec::new();
    for ((raw, anchor), cite) in good.iter().zip(&cites) {
        if let Some(line) = dead(actor, anchor, cite).await {
            s.push_str(&format!("- {line}\n"));
            continue;
        }
        if anchor.sha8.is_none()
            && matches!(anchor.locator, Some(Locator::Block(_)))
            && let Some(text) = &cite.text
        {
            rewrites.push((format!("]({raw})"), format!("]({raw}?s={})", sha8(text))));
        }
    }
    if !s.is_empty() {
        return Some(refused(s));
    }
    for (from, to) in rewrites {
        *markdown = markdown.replace(&from, &to);
    }
    None
}

/// First 8 hex of `sha256(text)` with trailing whitespace trimmed — the `?s=` drift hash (§6.4).
pub fn sha8(text: &str) -> String {
    codeindex::locate::text_sha(text.trim_end())[..8].to_owned()
}

/// The refusal line for a citation whose target is gone, `None` for `same` / `changed` / `moved`.
async fn dead(actor: &ActorRef<DocumentActor>, a: &TheuthAnchor, c: &Citation) -> Option<String> {
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
        Some(Locator::Block(_)) => {
            let near = match DocId::parse(&a.id) {
                Some(doc) => nearby_blocks(actor, doc).await,
                None => Vec::new(),
            };
            if near.is_empty() {
                format!("{href}: block not found")
            } else {
                format!(
                    "{href}: block not found; blocks near the end are {}",
                    near.join(", ")
                )
            }
        }
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

#[cfg(test)]
mod tests {
    use super::*;
    use surface::anchor::TheuthKind;

    #[test]
    fn finds_every_theuth_link_once_and_skips_other_schemes() {
        let md = "See [runbook §4.2](theuth://sources/01S1#b12) and [again](theuth://sources/01S1#b12), \
                  a [note](theuth://notes/01N1#x?s=0123abcd), [code](code:src/a.rs#L1) and [web](https://x.y/).";
        let got = check_theuth_links(md);
        assert_eq!(got.len(), 2);
        assert_eq!(got[0].kind, TheuthKind::Sources);
        assert_eq!(got[0].locator, Some(Locator::Block("b12".into())));
        assert_eq!(got[1].sha8.as_deref(), Some("0123abcd"));
    }

    #[test]
    fn malformed_links_are_listed_not_parsed() {
        let md = "[bad](theuth://pages/01#x) [ok](theuth://sessions/01S#3)";
        assert_eq!(check_theuth_links(md).len(), 1);
        let l = links(md);
        assert_eq!(l.len(), 2);
        assert!(l[0].1.is_err());
    }

    #[test]
    fn sha8_is_the_locate_convention_cut_to_eight() {
        // The trailing newline is not part of the hash (`text_sha` over the trimmed text).
        assert_eq!(sha8("pub fn helper() -> u32 {\n    1\n}\n"), "c6529027");
        assert_eq!(sha8("pub fn helper() -> u32 {\n    1\n}"), "c6529027");
    }
}
