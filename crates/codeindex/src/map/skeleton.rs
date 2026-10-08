//! The skeleton — what the index decides, zero tokens (147 §5): units, sizes,
//! edges with evidence, entry points, the structure-only view (§5.6), the
//! drill-down structure (§5.5), facts hashes (§5.4) and the three store reads
//! behind `codemap_overview` / `codemap_element` / `codemap_search`.
//! Runs at the end of every `reconcile`/`touch` that wrote (`index.rs`).
//!
//! owner: W1 — design §5, §8, §10.2 (overview/element/search)

use sha2::Digest;

use super::{Element, MapElement, MapHit, MapIndex, Overview, Search, not_built};
use crate::index::{Codeindex, RootIndex};

/// Sub-folders with at least this many parsed files become a unit (§5.1 item 3).
pub const LARGE_FOLDER_FILES: u32 = 40;
/// A unit of at most this many parsed files is a leaf (§5.1 item 4).
pub const LEAF_FILES: u32 = 2;
/// Edges below this share of a unit's out-weight fold into the bundle (§5.2).
pub const EDGE_MIN_SHARE: f64 = 0.02;
/// Cross-file refs below this confidence are not evidence (§5.2).
pub const REF_MIN_CONFIDENCE: f64 = 0.5;

/// `codemap_overview`: every perspective with its tree; `MapState::Structure`
/// after an inline first pass when the map file is missing (< 1 s). Store
/// reads only — never reconciles, never walks.
pub async fn overview(ix: &Codeindex, m: Overview) -> anyhow::Result<MapIndex> {
    let _ = (ix, m);
    Err(not_built("skeleton::overview", 1))
}

/// `codemap_element`: one element, its diagram (or the structure view when
/// nothing generated), children with thumbnails, edges with evidence,
/// `also_in`, and the live listing for leaves and depth > [`super::MAX_DEPTH`].
pub async fn element(ix: &Codeindex, m: Element) -> anyhow::Result<MapElement> {
    let _ = (ix, m);
    Err(not_built("skeleton::element", 1))
}

/// `codemap_search`: `elements.name LIKE` for a name, `anchors_path` prefix
/// match for a path → the element chain (§11.3, §14 < 20 ms).
pub async fn search(ix: &Codeindex, m: Search) -> anyhow::Result<Vec<MapHit>> {
    let _ = (ix, m);
    Err(not_built("skeleton::search", 1))
}

/// The skeleton pass after a write (§5, §8): recompute units, edges and the
/// structure view; refresh `MapChild` counts, anchors' `exists` and bundle
/// counts; recompute every element's facts hash and mark `stale`; read
/// `<root>/.omm/` through `import::import`; publish `CodeMapChanged` when
/// anything changed. Returns the element ids that changed.
pub async fn recompute(ix: &Codeindex, ri: &RootIndex) -> anyhow::Result<Vec<String>> {
    let _ = (ix, ri);
    Err(not_built("skeleton::recompute", 1))
}

/// §5.4: SHA-256 hex over the sorted `(path, mtime_ns, size)` of every file
/// under an element's anchors followed by the sorted `(from, to, kind, count)`
/// of its sub-skeleton edges. Pure; the one definition W1 and W2 share.
pub fn facts_hash(files: &[(String, i64, i64)], edges: &[(String, String, String, u32)]) -> String {
    let mut files: Vec<&(String, i64, i64)> = files.iter().collect();
    files.sort_unstable();
    let mut edges: Vec<&(String, String, String, u32)> = edges.iter().collect();
    edges.sort_unstable();
    let mut h = sha2::Sha256::new();
    for (p, m, s) in files {
        h.update(p.as_bytes());
        h.update(b"\0");
        h.update(m.to_le_bytes());
        h.update(s.to_le_bytes());
    }
    h.update(b"\n");
    for (f, t, k, n) in edges {
        h.update(f.as_bytes());
        h.update(b"\0");
        h.update(t.as_bytes());
        h.update(b"\0");
        h.update(k.as_bytes());
        h.update(b"\0");
        h.update(n.to_le_bytes());
    }
    format!("{:x}", h.finalize())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn facts_hash_is_order_free_and_sensitive_to_sync_keys() {
        let a = ("a.rs".to_owned(), 1_i64, 10_i64);
        let b = ("b.rs".to_owned(), 2_i64, 20_i64);
        let e = (
            "u1".to_owned(),
            "u2".to_owned(),
            "imports".to_owned(),
            3_u32,
        );
        let h1 = facts_hash(&[a.clone(), b.clone()], &[e.clone()]);
        let h2 = facts_hash(&[b.clone(), a.clone()], &[e.clone()]);
        assert_eq!(h1, h2);
        assert_eq!(h1.len(), 64);
        let touched = ("a.rs".to_owned(), 5_i64, 10_i64);
        assert_ne!(h1, facts_hash(&[touched, b.clone()], &[e.clone()]));
        let recount = (
            "u1".to_owned(),
            "u2".to_owned(),
            "imports".to_owned(),
            4_u32,
        );
        assert_ne!(h1, facts_hash(&[a, b], &[recount]));
    }
}
