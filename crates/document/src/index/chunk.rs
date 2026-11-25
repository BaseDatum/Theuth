//! Header-aware chunking (144 §7.3): walk the top-level blocks in order; a heading starts a chunk
//! and is prepended to every chunk under it; blocks are appended until `max_tokens`, the next chunk
//! starts with the previous one's tail blocks (`overlap` of `max_tokens`), chunks under `min_tokens`
//! are dropped. Every chunk maps to `[first block, last block]`. Pure: token counts come in.

/// One top-level block as the outline and the annotated markdown describe it.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Block {
    /// The outline id.
    pub id: String,
    /// `Some(level)` when the block is a heading.
    pub heading: Option<u8>,
    /// The block's markdown (headings without their `#`s).
    pub text: String,
}

/// The §14 numbers.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Params {
    /// `notebooks.chunk_tokens` (400).
    pub max_tokens: usize,
    /// `notebooks.chunk_overlap` (0.15 of `max_tokens`, in tail blocks).
    pub overlap: f32,
    /// `notebooks.chunk_min_tokens` (5).
    pub min_tokens: usize,
}

/// One chunk: block indices into the input, the text that is embedded, its token count.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Chunk {
    /// Index of the first block.
    pub first: usize,
    /// Index of the last block (inclusive).
    pub last: usize,
    /// Heading (when under one) and the blocks' text, blank-line separated.
    pub text: String,
    /// Tokens as the counter measured them (sum of parts).
    pub tokens: usize,
}

/// Chunk `blocks` given `tokens[i]` per block.
pub fn chunk(blocks: &[Block], tokens: &[usize], p: &Params) -> Vec<Chunk> {
    debug_assert_eq!(blocks.len(), tokens.len());
    let overlap_budget = (p.max_tokens as f32 * p.overlap).round() as usize;
    let mut out = Vec::new();
    // The nearest heading above the cursor: (block index, tokens).
    let mut heading: Option<usize> = None;
    // Blocks of the open chunk, in order.
    let mut cur: Vec<usize> = Vec::new();

    let flush = |cur: &mut Vec<usize>, heading: Option<usize>, out: &mut Vec<Chunk>| {
        if cur.is_empty() {
            return;
        }
        let with_heading = heading.filter(|h| cur[0] != *h);
        let mut n: usize = cur.iter().map(|&i| tokens[i]).sum();
        let mut parts: Vec<&str> = Vec::with_capacity(cur.len() + 1);
        if let Some(h) = with_heading {
            n += tokens[h];
            parts.push(&blocks[h].text);
        }
        parts.extend(cur.iter().map(|&i| blocks[i].text.as_str()));
        if n >= p.min_tokens {
            out.push(Chunk {
                first: cur[0],
                last: *cur.last().unwrap_or(&cur[0]),
                text: parts.join("\n\n"),
                tokens: n,
            });
        }
        cur.clear();
    };

    for (i, b) in blocks.iter().enumerate() {
        if b.heading.is_some() {
            flush(&mut cur, heading, &mut out);
            heading = Some(i);
            cur.push(i);
            continue;
        }
        let used: usize = cur.iter().map(|&i| tokens[i]).sum::<usize>()
            + heading
                .filter(|h| cur.first() != Some(h))
                .map_or(0, |h| tokens[h]);
        if !cur.is_empty() && used + tokens[i] > p.max_tokens {
            // Overlap: the tail blocks (never the heading block) within the budget.
            let mut tail: Vec<usize> = Vec::new();
            let mut t = 0;
            for &j in cur.iter().rev() {
                if Some(j) == heading || t + tokens[j] > overlap_budget {
                    break;
                }
                t += tokens[j];
                tail.push(j);
            }
            tail.reverse();
            flush(&mut cur, heading, &mut out);
            cur = tail;
        }
        cur.push(i);
    }
    flush(&mut cur, heading, &mut out);
    out
}

/// Block kinds for a document of `doc_kind` (§7.1 `kind`): a source's `## Insights` subtree is
/// `insight`, everything else the document's kind.
pub fn kinds(blocks: &[Block], doc_kind: &str) -> Vec<&'static str> {
    let doc_kind: &'static str = match doc_kind {
        "note" => "note",
        "board" => "board",
        "meeting_note" => "meeting_note",
        "source" => "source",
        "whiteboard" => "whiteboard",
        _ => "notebook",
    };
    let mut out = Vec::with_capacity(blocks.len());
    let mut in_insights = false;
    for b in blocks {
        if let Some(level) = b.heading {
            if doc_kind == "source" && level == 2 && b.text.trim().eq_ignore_ascii_case("insights")
            {
                in_insights = true;
            } else if level <= 2 {
                in_insights = false;
            }
        }
        out.push(if in_insights { "insight" } else { doc_kind });
    }
    out
}

/// `chars / 4`: the estimate when no model tokenizer is installed (§7.3).
pub fn estimate_tokens(text: &str) -> usize {
    text.chars().count().div_ceil(4)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn b(id: &str, heading: Option<u8>, text: &str) -> Block {
        Block {
            id: id.into(),
            heading,
            text: text.into(),
        }
    }

    const P: Params = Params {
        max_tokens: 10,
        overlap: 0.3,
        min_tokens: 2,
    };

    /// A heading starts a chunk and is prepended to every chunk under it; the budget splits;
    /// the overlap is the previous chunk's tail; a tiny chunk is dropped; every chunk maps to
    /// `[first, last]`.
    #[test]
    fn headings_budget_overlap_and_minimum() {
        let blocks = vec![
            b("h1", Some(1), "Title"),
            b("p1", None, "one two three four"),
            b("p2", None, "five six seven"),
            b("p3", None, "eight nine ten eleven"),
            b("h2", Some(2), "Section"),
            b("p4", None, "a b"),
            b("h3", Some(2), "Tail"),
            b("p5", None, "x"),
        ];
        let tokens = vec![1, 4, 3, 4, 1, 2, 1, 1];
        let chunks = chunk(&blocks, &tokens, &P);
        let spans: Vec<(usize, usize)> = chunks.iter().map(|c| (c.first, c.last)).collect();
        // h1+p1+p2 = 8 ≤ 10; p3 would make 12 → split; overlap budget 3 keeps p2 (3 tokens).
        assert_eq!(spans, [(0, 2), (2, 3), (4, 5), (6, 7)], "{chunks:#?}");
        assert_eq!(
            chunks[0].text,
            "Title\n\none two three four\n\nfive six seven"
        );
        assert_eq!(
            chunks[1].text,
            "Title\n\nfive six seven\n\neight nine ten eleven"
        );
        assert_eq!(chunks[1].tokens, 1 + 3 + 4);
        assert_eq!(chunks[2].text, "Section\n\na b");
        // "Tail" + "x" = 2 tokens = the minimum: kept; alone "x" would be dropped.
        assert_eq!(chunks[3].text, "Tail\n\nx");
        let tiny = chunk(&[b("p", None, "x")], &[1], &P);
        assert!(tiny.is_empty());
    }

    #[test]
    fn source_insights_subtree_is_insight_kind() {
        let blocks = vec![
            b("a", Some(1), "Doc"),
            b("b", None, "body"),
            b("c", Some(2), "Insights"),
            b("d", Some(3), "Dense"),
            b("e", None, "summary"),
            b("f", Some(2), "Appendix"),
            b("g", None, "more"),
        ];
        assert_eq!(
            kinds(&blocks, "source"),
            [
                "source", "source", "insight", "insight", "insight", "source", "source"
            ]
        );
        assert!(kinds(&blocks, "note").iter().all(|k| *k == "note"));
        assert_eq!(estimate_tokens("abcdefghi"), 3);
    }
}
