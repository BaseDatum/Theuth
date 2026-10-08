//! The three tables of 144 §7.1 in `documents/index.sqlite`, on their own pool: `vec0` is a
//! process-global auto-extension that only reaches connections opened after [`register_vec`], and
//! the store's pool is already open by then. Not replicated — derived, rebuilt locally.

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::{LazyLock, Once};
use std::time::Duration;

use parking_lot::Mutex;

use sqlx::sqlite::{SqliteConnectOptions, SqliteJournalMode, SqlitePool, SqlitePoolOptions};

/// The store's file (`store.rs` `INDEX`).
const FILE: &str = "index.sqlite";

/// `nb_blocks`: one row per top-level block (plus one `block = ''` row carrying the title).
const CREATE_BLOCKS: &str = "CREATE VIRTUAL TABLE IF NOT EXISTS nb_blocks USING fts5(\
    doc UNINDEXED, block UNINDEXED, kind UNINDEXED, ord UNINDEXED, heading, text, \
    text_sha UNINDEXED, tokens UNINDEXED, \
    tokenize = 'porter unicode61 remove_diacritics 2')";
/// `nb_index_state`: per document, what was indexed and with which model.
const CREATE_STATE: &str = "CREATE TABLE IF NOT EXISTS nb_index_state (\
    doc TEXT PRIMARY KEY, content_sha TEXT, model TEXT, dims INTEGER, indexed_ms INTEGER)";

/// Column indexes of `heading` / `text` in `nb_blocks` (for `snippet()`).
const HEADING_COL: u32 = 4;
const TEXT_COL: u32 = 5;
/// Snippet width in tokens (10 §3.3 uses 12 for concepts; blocks are prose).
const SNIPPET_TOKENS: u32 = 24;
/// Markers `snippet()` wraps matches in; stripped into `MatchRange`s by the caller.
pub const MARK_START: char = '\u{1}';
pub const MARK_END: char = '\u{2}';

/// One `nb_blocks` row.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct BlockRow {
    pub block: String,
    pub kind: String,
    pub ord: i64,
    pub heading: Option<String>,
    pub text: String,
    pub text_sha: String,
    pub tokens: i64,
}

/// One `nb_vec` row to write: the chunk's identity and its vector (or `None` when the model is
/// absent — the row is skipped).
#[derive(Clone, Debug)]
pub struct VecRow {
    pub block: String,
    pub chunk: i64,
    pub sha: String,
    pub embedding: Vec<u8>,
}

/// `nb_index_state` row.
#[derive(Clone, Debug, PartialEq, Eq, sqlx::FromRow)]
pub struct State {
    pub doc: String,
    pub content_sha: Option<String>,
    pub model: Option<String>,
    pub dims: Option<i64>,
    pub indexed_ms: Option<i64>,
}

/// A lexical hit: `bm25` ascending is best; `snippet` (of `text`) and `hsnip` (of `heading`)
/// carry [`MARK_START`]/[`MARK_END`] around matches — none when that column did not match.
#[derive(Clone, Debug, sqlx::FromRow)]
pub struct Lexical {
    pub doc: String,
    pub block: String,
    pub kind: String,
    pub heading: Option<String>,
    pub text: String,
    pub text_sha: String,
    pub snippet: String,
    pub hsnip: String,
}

/// A vector hit: cosine distance ascending is best.
#[derive(Clone, Debug, sqlx::FromRow)]
pub struct Nearest {
    pub doc: String,
    pub block: String,
    pub distance: f64,
}

/// Register `vec0` for every connection this process opens from now on (once).
pub fn register_vec() {
    static ONCE: Once = Once::new();
    ONCE.call_once(|| {
        // SAFETY: `sqlite3_vec_init` is sqlite-vec's entry point with the standard
        // `(db, pzErrMsg, pApi)` signature (the crate declares it without arguments, as its
        // own rusqlite test transmutes it); `sqlite3_auto_extension` is called before any
        // connection of the index pool exists, which is what it is for.
        unsafe {
            libsqlite3_sys::sqlite3_auto_extension(Some(std::mem::transmute::<
                *const (),
                unsafe extern "C" fn(
                    *mut libsqlite3_sys::sqlite3,
                    *mut *mut std::os::raw::c_char,
                    *const libsqlite3_sys::sqlite3_api_routines,
                ) -> std::os::raw::c_int,
            >(
                sqlite_vec::sqlite3_vec_init as *const (),
            )));
        }
    });
}

/// The index over one `documents/` root.
#[derive(Clone)]
pub struct Index {
    db: SqlitePool,
    /// `nb_vec`'s vector length.
    pub dims: usize,
}

/// One pool per root per process (tests open several roots).
static POOLS: LazyLock<Mutex<HashMap<PathBuf, Index>>> = LazyLock::new(Default::default);

impl Index {
    /// Open (or reuse) the index at `root/index.sqlite`; `nb_vec` is created at `dims`, and
    /// recreated when an existing table has other dims (a model change: every vector is stale,
    /// §7.5; FTS5 rows stay).
    pub async fn open(root: &Path, dims: usize) -> anyhow::Result<Index> {
        if let Some(ix) = POOLS.lock().get(root)
            && ix.dims == dims
        {
            return Ok(ix.clone());
        }
        register_vec();
        let opts = SqliteConnectOptions::new()
            .filename(root.join(FILE))
            .create_if_missing(true)
            .journal_mode(SqliteJournalMode::Wal)
            .busy_timeout(Duration::from_secs(5));
        let db = SqlitePoolOptions::new()
            .max_connections(2)
            .connect_with(opts)
            .await?;
        sqlx::query(CREATE_BLOCKS).execute(&db).await?;
        sqlx::query(CREATE_STATE).execute(&db).await?;
        let existing: Option<(String,)> =
            sqlx::query_as("SELECT sql FROM sqlite_master WHERE name = 'nb_vec'")
                .fetch_optional(&db)
                .await?;
        let want = format!("float[{dims}]");
        if let Some((sql,)) = &existing
            && !sql.contains(&want)
        {
            tracing::info!("index: nb_vec dims changed to {dims}; vectors rebuild");
            sqlx::query("DROP TABLE nb_vec").execute(&db).await?;
            sqlx::query("UPDATE nb_index_state SET model = NULL, dims = NULL")
                .execute(&db)
                .await?;
        }
        sqlx::query(&format!(
            "CREATE VIRTUAL TABLE IF NOT EXISTS nb_vec USING vec0(\
             rowid INTEGER PRIMARY KEY, embedding {want}, doc TEXT, block TEXT, chunk INTEGER, sha TEXT)"
        ))
        .execute(&db)
        .await?;
        let ix = Index { db, dims };
        POOLS.lock().insert(root.to_path_buf(), ix.clone());
        Ok(ix)
    }

    /// `nb_index_state` of `doc`.
    pub async fn state(&self, doc: &str) -> anyhow::Result<Option<State>> {
        Ok(sqlx::query_as("SELECT * FROM nb_index_state WHERE doc = ?")
            .bind(doc)
            .fetch_optional(&self.db)
            .await?)
    }

    /// Every `nb_index_state` row, as `doc → model`.
    pub async fn models(&self) -> anyhow::Result<HashMap<String, Option<String>>> {
        let rows: Vec<(String, Option<String>)> =
            sqlx::query_as("SELECT doc, model FROM nb_index_state")
                .fetch_all(&self.db)
                .await?;
        Ok(rows.into_iter().collect())
    }

    /// `(block, text_sha)` of `doc`'s rows.
    pub async fn block_shas(&self, doc: &str) -> anyhow::Result<Vec<(String, String)>> {
        Ok(
            sqlx::query_as("SELECT block, text_sha FROM nb_blocks WHERE doc = ? ORDER BY ord")
                .bind(doc)
                .fetch_all(&self.db)
                .await?,
        )
    }

    /// `doc`'s vectors by chunk sha (reused across a re-index when the chunk text is unchanged).
    pub async fn vectors(&self, doc: &str) -> anyhow::Result<HashMap<String, Vec<u8>>> {
        let rows: Vec<(String, Vec<u8>)> =
            sqlx::query_as("SELECT sha, embedding FROM nb_vec WHERE doc = ?")
                .bind(doc)
                .fetch_all(&self.db)
                .await?;
        Ok(rows.into_iter().collect())
    }

    /// Replace every row of `doc` in one transaction: blocks, vectors, state.
    pub async fn replace(
        &self,
        doc: &str,
        blocks: &[BlockRow],
        vectors: &[VecRow],
        state: &State,
    ) -> anyhow::Result<()> {
        let mut tx = self.db.begin().await?;
        sqlx::query("DELETE FROM nb_blocks WHERE doc = ?")
            .bind(doc)
            .execute(&mut *tx)
            .await?;
        sqlx::query("DELETE FROM nb_vec WHERE doc = ?")
            .bind(doc)
            .execute(&mut *tx)
            .await?;
        for b in blocks {
            sqlx::query(
                "INSERT INTO nb_blocks (doc, block, kind, ord, heading, text, text_sha, tokens) \
                 VALUES (?, ?, ?, ?, ?, ?, ?, ?)",
            )
            .bind(doc)
            .bind(&b.block)
            .bind(&b.kind)
            .bind(b.ord)
            .bind(&b.heading)
            .bind(&b.text)
            .bind(&b.text_sha)
            .bind(b.tokens)
            .execute(&mut *tx)
            .await?;
        }
        for v in vectors {
            sqlx::query(
                "INSERT INTO nb_vec (embedding, doc, block, chunk, sha) VALUES (?, ?, ?, ?, ?)",
            )
            .bind(&v.embedding)
            .bind(doc)
            .bind(&v.block)
            .bind(v.chunk)
            .bind(&v.sha)
            .execute(&mut *tx)
            .await?;
        }
        sqlx::query(
            "INSERT INTO nb_index_state (doc, content_sha, model, dims, indexed_ms) \
             VALUES (?, ?, ?, ?, ?) ON CONFLICT(doc) DO UPDATE SET content_sha = excluded.content_sha, \
             model = excluded.model, dims = excluded.dims, indexed_ms = excluded.indexed_ms",
        )
        .bind(doc)
        .bind(&state.content_sha)
        .bind(&state.model)
        .bind(state.dims)
        .bind(state.indexed_ms)
        .execute(&mut *tx)
        .await?;
        tx.commit().await?;
        Ok(())
    }

    /// Drop every row of `doc`.
    pub async fn delete(&self, doc: &str) -> anyhow::Result<()> {
        let mut tx = self.db.begin().await?;
        for sql in [
            "DELETE FROM nb_blocks WHERE doc = ?",
            "DELETE FROM nb_vec WHERE doc = ?",
            "DELETE FROM nb_index_state WHERE doc = ?",
        ] {
            sqlx::query(sql).bind(doc).execute(&mut *tx).await?;
        }
        tx.commit().await?;
        Ok(())
    }

    /// Documents in `nb_index_state`.
    pub async fn indexed_docs(&self) -> anyhow::Result<Vec<String>> {
        let rows: Vec<(String,)> = sqlx::query_as("SELECT doc FROM nb_index_state")
            .fetch_all(&self.db)
            .await?;
        Ok(rows.into_iter().map(|(d,)| d).collect())
    }

    /// FTS5 top-`k` for `fts` (an FTS5 MATCH expression, [`fts_query`]) within `docs` / `kinds`
    /// when given; headings weigh 3× (10 §3.3's column weights).
    pub async fn lexical(
        &self,
        fts: &str,
        docs: Option<&[String]>,
        kinds: Option<&[String]>,
        k: u32,
    ) -> anyhow::Result<Vec<Lexical>> {
        let mut sql = format!(
            "SELECT doc, block, kind, heading, text, text_sha, \
             snippet(nb_blocks, {TEXT_COL}, '{MARK_START}', '{MARK_END}', '…', {SNIPPET_TOKENS}) AS snippet, \
             snippet(nb_blocks, {HEADING_COL}, '{MARK_START}', '{MARK_END}', '', {SNIPPET_TOKENS}) AS hsnip \
             FROM nb_blocks WHERE nb_blocks MATCH ?"
        );
        push_in(&mut sql, "doc", docs);
        push_in(&mut sql, "kind", kinds);
        sql.push_str(" ORDER BY bm25(nb_blocks, 0, 0, 0, 0, 3.0, 1.0) LIMIT ?");
        let mut q = sqlx::query_as::<_, Lexical>(&sql).bind(fts);
        for v in docs
            .into_iter()
            .flatten()
            .chain(kinds.into_iter().flatten())
        {
            q = q.bind(v);
        }
        Ok(q.bind(k).fetch_all(&self.db).await?)
    }

    /// `vec0` top-`k` nearest to `embedding` (f32 little-endian bytes) within `docs` when given.
    pub async fn nearest(
        &self,
        embedding: &[u8],
        docs: Option<&[String]>,
        k: u32,
    ) -> anyhow::Result<Vec<Nearest>> {
        let mut sql = String::from(
            "SELECT doc, block, distance FROM nb_vec WHERE embedding MATCH ? AND k = ?",
        );
        push_in(&mut sql, "doc", docs);
        sql.push_str(" ORDER BY distance");
        let mut q = sqlx::query_as::<_, Nearest>(&sql).bind(embedding).bind(k);
        for v in docs.into_iter().flatten() {
            q = q.bind(v);
        }
        Ok(q.fetch_all(&self.db).await?)
    }

    /// The rows of `(doc, block)` pairs — the vector hits' text.
    pub async fn rows(&self, keys: &[(String, String)]) -> anyhow::Result<Vec<Lexical>> {
        let mut out = Vec::with_capacity(keys.len());
        for (doc, block) in keys {
            let row: Option<Lexical> = sqlx::query_as(
                "SELECT doc, block, kind, heading, text, text_sha, text AS snippet, '' AS hsnip \
                 FROM nb_blocks WHERE doc = ? AND block = ?",
            )
            .bind(doc)
            .bind(block)
            .fetch_optional(&self.db)
            .await?;
            out.extend(row);
        }
        Ok(out)
    }
}

fn push_in(sql: &mut String, col: &str, values: Option<&[String]>) {
    if let Some(v) = values
        && !v.is_empty()
    {
        sql.push_str(&format!(
            " AND {col} IN ({})",
            std::iter::repeat_n("?", v.len())
                .collect::<Vec<_>>()
                .join(", ")
        ));
    }
}

/// A user query as an FTS5 expression: every word quoted (operators are text), OR-ed — recall
/// over precision, bm25 still ranks rows holding more of the words first and the vector side
/// carries the meaning (§7.4). Empty when no word survives.
pub fn fts_query(q: &str) -> String {
    q.split_whitespace()
        .filter(|w| w.chars().any(char::is_alphanumeric))
        .map(|w| format!("\"{}\"", w.replace('"', "\"\"")))
        .collect::<Vec<_>>()
        .join(" OR ")
}

/// f32s as the little-endian bytes `vec0` stores.
pub fn to_blob(v: &[f32]) -> Vec<u8> {
    v.iter().flat_map(|f| f.to_le_bytes()).collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn fts_query_quotes_words_and_drops_operators() {
        assert_eq!(
            fts_query("fail-over AND \"x\""),
            "\"fail-over\" OR \"\"\"x\"\"\""
        );
        assert_eq!(fts_query("  -  "), "");
    }

    #[tokio::test]
    async fn vec0_loads_and_answers_knn() {
        let dir = tempfile::tempdir().unwrap();
        let ix = Index::open(dir.path(), 2).await.unwrap();
        let s = |sha: &str| State {
            doc: "d".into(),
            content_sha: Some(sha.into()),
            model: Some("tiny".into()),
            dims: Some(2),
            indexed_ms: Some(1),
        };
        let v = |b: &str, x: f32, y: f32| VecRow {
            block: b.into(),
            chunk: 0,
            sha: b.into(),
            embedding: to_blob(&[x, y]),
        };
        ix.replace("d", &[], &[v("a", 1.0, 0.0), v("b", 0.0, 1.0)], &s("1"))
            .await
            .unwrap();
        let near = ix.nearest(&to_blob(&[0.9, 0.1]), None, 2).await.unwrap();
        assert_eq!(near[0].block, "a");
        assert_eq!(near[1].block, "b");
        assert_eq!(ix.vectors("d").await.unwrap().len(), 2);
        // Dims change → nb_vec rebuilt empty, state's model cleared, FTS5 rows untouched.
        let ix = Index::open(dir.path(), 3).await.unwrap();
        assert!(ix.vectors("d").await.unwrap().is_empty());
        assert_eq!(ix.state("d").await.unwrap().unwrap().model, None);
    }
}
