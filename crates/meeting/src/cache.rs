//! The brief cache (05): briefs on disk in their own SQLite file, searchable,
//! and the alias scan the actor runs on every closed utterance before any
//! model turn. `briefs` is the truth, `briefs_fts` its mirror; the lowercased
//! alias list is held in memory and rebuilt on every write (writes are rare,
//! the cache is small). `scan` is pure and unit-tested.

use std::path::Path;
use std::sync::{Arc, RwLock};
use std::time::Duration;

use anyhow::Context as _;
use serde::{Deserialize, Serialize};
use sqlx::sqlite::{SqliteConnectOptions, SqliteJournalMode, SqlitePool, SqlitePoolOptions};

use crate::topics::{check_bullets, check_label};
use crate::{BriefId, DIR};

/// Cache file name, inside [`DIR`].
pub const DB: &str = "briefs.sqlite";
/// Most rows `search`/`list` return.
pub const LIST_CAP: u32 = 50;

/// One cached brief — the row. A [`crate::Brief`] shown from it is an instance
/// in that meeting's `briefs.jsonl` with `cached: Some(row.id)`.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, schemars::JsonSchema)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS), ts(export))]
pub struct CachedBrief {
    /// Stable across updates.
    pub id: BriefId,
    /// Title.
    pub title: String,
    /// 2–3 bullets.
    pub bullets: Vec<String>,
    /// How people say it; ≥ 1, each ≤ 4 words; the first is the display label.
    pub aliases: Vec<String>,
    /// Free tags; the prefill conversation uses one per meeting.
    pub tags: Vec<String>,
    /// `meeting:<id>` | `session:<id>` | `user`.
    pub source: String,
    /// RFC 3339.
    pub created_at: String,
    /// RFC 3339; moves on a content change, not on an alias or a use.
    pub updated_at: String,
    /// Times the actor showed it.
    pub uses: u32,
    /// RFC 3339 of the last use.
    pub last_used_at: Option<String>,
}

/// What `brief_put` (tool and RPC) sends: an update when `id` names a row.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize, schemars::JsonSchema)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS), ts(export))]
pub struct CachedBriefPut {
    /// The row to update; omitted → a new brief.
    #[serde(default)]
    pub id: Option<BriefId>,
    /// Title.
    pub title: String,
    /// Two or three short factual bullets.
    pub bullets: Vec<String>,
    /// How people will say it (ticket ids, product names, first names); at least one, each at most four words.
    pub aliases: Vec<String>,
    /// Free tags, e.g. one for the meeting this was prepared for.
    #[serde(default)]
    pub tags: Vec<String>,
}

/// Lowercased alias → row, for [`scan`].
pub type Aliases = Arc<Vec<(String, BriefId)>>;

type Row = (
    String,
    String,
    String,
    String,
    String,
    String,
    String,
    String,
    i64,
    Option<String>,
);

fn row(r: Row) -> anyhow::Result<CachedBrief> {
    let (id, title, bullets, aliases, tags, source, created_at, updated_at, uses, last_used_at) = r;
    Ok(CachedBrief {
        id: id.parse().context("brief id")?,
        title,
        bullets: serde_json::from_str(&bullets).context("bullets")?,
        aliases: serde_json::from_str(&aliases).context("aliases")?,
        tags: serde_json::from_str(&tags).context("tags")?,
        source,
        created_at,
        updated_at,
        uses: uses.max(0) as u32,
        last_used_at,
    })
}

fn json(v: &[String]) -> String {
    serde_json::to_string(v).unwrap_or_else(|_| "[]".into())
}

fn now() -> String {
    chrono::Utc::now().to_rfc3339()
}

/// Whitespace-normalised, non-empty, deduplicated case-insensitively.
fn clean(items: &[String]) -> Vec<String> {
    let mut out: Vec<String> = Vec::with_capacity(items.len());
    for s in items {
        let s = s.split_whitespace().collect::<Vec<_>>().join(" ");
        if !s.is_empty() && !out.iter().any(|o| o.eq_ignore_ascii_case(&s)) {
            out.push(s);
        }
    }
    out
}

/// `<data_dir>/meetings/briefs.sqlite` and the in-memory alias list.
pub struct Cache {
    pool: SqlitePool,
    aliases: RwLock<Aliases>,
}

impl Cache {
    /// Open (creating and migrating) the cache and load the aliases.
    pub async fn open(data_dir: &Path) -> anyhow::Result<Self> {
        let dir = data_dir.join(DIR);
        std::fs::create_dir_all(&dir).with_context(|| format!("create {}", dir.display()))?;
        let db = dir.join(DB);
        let opts = SqliteConnectOptions::new()
            .filename(&db)
            .create_if_missing(true)
            .journal_mode(SqliteJournalMode::Wal)
            .busy_timeout(Duration::from_secs(5));
        let pool = SqlitePoolOptions::new()
            .max_connections(2)
            .connect_with(opts)
            .await
            .with_context(|| format!("open {}", db.display()))?;
        sqlx::migrate!("./migrations/briefs")
            .run(&pool)
            .await
            .context("run brief migrations")?;
        let cache = Self {
            pool,
            aliases: RwLock::new(Arc::new(Vec::new())),
        };
        cache.reload().await?;
        Ok(cache)
    }

    /// The alias list as of the last write.
    pub fn aliases(&self) -> Aliases {
        self.aliases.read().map(|a| a.clone()).unwrap_or_default()
    }

    async fn reload(&self) -> anyhow::Result<()> {
        let rows: Vec<(String, String)> = sqlx::query_as("SELECT id, aliases FROM briefs")
            .fetch_all(&self.pool)
            .await
            .context("load aliases")?;
        let mut list = Vec::new();
        for (id, aliases) in rows {
            let Ok(id) = id.parse::<BriefId>() else { continue };
            let aliases: Vec<String> = serde_json::from_str(&aliases).unwrap_or_default();
            list.extend(aliases.into_iter().map(|a| (a.to_lowercase(), id)));
        }
        if let Ok(mut w) = self.aliases.write() {
            *w = Arc::new(list);
        }
        Ok(())
    }

    /// One row.
    pub async fn get(&self, id: BriefId) -> Result<CachedBrief, String> {
        let r: Option<Row> = sqlx::query_as(
            "SELECT id, title, bullets, aliases, tags, source, created_at, updated_at, uses, last_used_at \
             FROM briefs WHERE id = ?",
        )
            .bind(id.to_string())
            .fetch_optional(&self.pool)
            .await
            .map_err(|e| e.to_string())?;
        let r = r.ok_or_else(|| format!("no cached brief {id}"))?;
        row(r).map_err(|e| format!("{e:#}"))
    }

    /// Newest `updated_at` first, optionally only rows tagged `tag`.
    pub async fn list(&self, tag: Option<&str>, limit: u32) -> anyhow::Result<Vec<CachedBrief>> {
        let rows: Vec<Row> = sqlx::query_as(
            "SELECT id, title, bullets, aliases, tags, source, created_at, updated_at, uses, last_used_at \
             FROM briefs \
             WHERE ? IS NULL OR EXISTS (SELECT 1 FROM json_each(briefs.tags) WHERE value = ?) \
             ORDER BY updated_at DESC LIMIT ?",
        )
        .bind(tag)
        .bind(tag)
        .bind(i64::from(limit.clamp(1, LIST_CAP)))
        .fetch_all(&self.pool)
        .await
        .context("list briefs")?;
        rows.into_iter().map(row).collect()
    }

    /// Every distinct tag, most rows first.
    pub async fn tags(&self) -> anyhow::Result<Vec<String>> {
        let rows: Vec<(String,)> = sqlx::query_as(
            "SELECT value FROM briefs, json_each(briefs.tags) GROUP BY value ORDER BY count(*) DESC, value",
        )
        .fetch_all(&self.pool)
        .await
        .context("list tags")?;
        Ok(rows.into_iter().map(|(t,)| t).collect())
    }

    /// FTS over aliases (10) / title (5) / tags (2) / bullets (1), OR-joined
    /// quoted words, best rank then newest first; a single word equal to an
    /// alias puts that row first regardless of rank (05 §4).
    pub async fn search(&self, query: &str, limit: u32) -> anyhow::Result<Vec<CachedBrief>> {
        let words: Vec<&str> = query.split_whitespace().collect();
        if words.is_empty() {
            return Ok(Vec::new());
        }
        let limit = limit.clamp(1, LIST_CAP);
        let match_ = words
            .iter()
            .map(|w| format!("\"{}\"", w.replace('"', "\"\"")))
            .collect::<Vec<_>>()
            .join(" OR ");
        let rows: Vec<Row> = sqlx::query_as(
            "SELECT b.id, b.title, b.bullets, b.aliases, b.tags, b.source, b.created_at, b.updated_at, \
             b.uses, b.last_used_at FROM briefs_fts f JOIN briefs b ON b.id = f.id \
             WHERE briefs_fts MATCH ? \
             ORDER BY bm25(briefs_fts, 0, 10.0, 5.0, 2.0, 1.0), b.updated_at DESC LIMIT ?",
        )
        .bind(match_)
        .bind(i64::from(limit))
        .fetch_all(&self.pool)
        .await
        .context("search briefs")?;
        let mut hits: Vec<CachedBrief> = rows.into_iter().map(row).collect::<anyhow::Result<_>>()?;
        if let [word] = words.as_slice() {
            let word = word.to_lowercase();
            let exact = self.aliases().iter().find(|(a, _)| *a == word).map(|(_, id)| *id);
            if let Some(id) = exact {
                match hits.iter().position(|h| h.id == id) {
                    Some(0) => {}
                    Some(i) => hits[..=i].rotate_right(1),
                    None => {
                        if let Ok(r) = self.get(id).await {
                            hits.insert(0, r);
                            hits.truncate(limit as usize);
                        }
                    }
                }
            }
        }
        Ok(hits)
    }

    /// Validate, upsert the row and its FTS mirror in one transaction, reload
    /// the aliases. `Ok((row, created))`. An existing row keeps `source`,
    /// `created_at`, `uses`; `updated_at` moves.
    pub async fn put(&self, p: CachedBriefPut, source: &str) -> Result<(CachedBrief, bool), String> {
        let title = p.title.trim();
        if title.is_empty() {
            return Err("title is empty".into());
        }
        check_bullets(&p.bullets)?;
        let bullets: Vec<String> = p.bullets.iter().map(|b| b.trim().to_owned()).collect();
        let aliases = clean(&p.aliases);
        if aliases.is_empty() {
            return Err("at least one alias (how people say it)".into());
        }
        for a in &aliases {
            check_label(a).map_err(|e| format!("alias \"{a}\": {e}"))?;
        }
        let tags = clean(&p.tags);
        let id = p.id.unwrap_or_default();
        let stamp = now();
        let mut tx = self.pool.begin().await.map_err(|e| e.to_string())?;
        let existing: Option<(String,)> = sqlx::query_as("SELECT source FROM briefs WHERE id = ?")
            .bind(id.to_string())
            .fetch_optional(&mut *tx)
            .await
            .map_err(|e| e.to_string())?;
        sqlx::query(
            "INSERT INTO briefs (id, title, bullets, aliases, tags, source, created_at, updated_at) \
             VALUES (?, ?, ?, ?, ?, ?, ?, ?) \
             ON CONFLICT(id) DO UPDATE SET title = excluded.title, bullets = excluded.bullets, \
             aliases = excluded.aliases, tags = excluded.tags, updated_at = excluded.updated_at",
        )
        .bind(id.to_string())
        .bind(title)
        .bind(json(&bullets))
        .bind(json(&aliases))
        .bind(json(&tags))
        .bind(source)
        .bind(&stamp)
        .bind(&stamp)
        .execute(&mut *tx)
        .await
        .map_err(|e| e.to_string())?;
        Self::index(&mut tx, id, &aliases, title, &tags, &bullets).await?;
        tx.commit().await.map_err(|e| e.to_string())?;
        let _ = self.reload().await;
        Ok((self.get(id).await?, existing.is_none()))
    }

    async fn index(
        tx: &mut sqlx::Transaction<'_, sqlx::Sqlite>,
        id: BriefId,
        aliases: &[String],
        title: &str,
        tags: &[String],
        bullets: &[String],
    ) -> Result<(), String> {
        sqlx::query("DELETE FROM briefs_fts WHERE id = ?")
            .bind(id.to_string())
            .execute(&mut **tx)
            .await
            .map_err(|e| e.to_string())?;
        sqlx::query("INSERT INTO briefs_fts (id, aliases, title, tags, bullets) VALUES (?, ?, ?, ?, ?)")
            .bind(id.to_string())
            .bind(aliases.join(" "))
            .bind(title)
            .bind(tags.join(" "))
            .bind(bullets.join(" "))
            .execute(&mut **tx)
            .await
            .map_err(|e| e.to_string())?;
        Ok(())
    }

    /// Remove a row and its mirror. Unknown → `Err`.
    pub async fn delete(&self, id: BriefId) -> Result<(), String> {
        let mut tx = self.pool.begin().await.map_err(|e| e.to_string())?;
        let n = sqlx::query("DELETE FROM briefs WHERE id = ?")
            .bind(id.to_string())
            .execute(&mut *tx)
            .await
            .map_err(|e| e.to_string())?
            .rows_affected();
        if n == 0 {
            return Err(format!("no cached brief {id}"));
        }
        sqlx::query("DELETE FROM briefs_fts WHERE id = ?")
            .bind(id.to_string())
            .execute(&mut *tx)
            .await
            .map_err(|e| e.to_string())?;
        tx.commit().await.map_err(|e| e.to_string())?;
        let _ = self.reload().await;
        Ok(())
    }

    /// The actor showed the brief: `uses += 1`, `last_used_at = now`.
    pub async fn touch_use(&self, id: BriefId) -> anyhow::Result<()> {
        sqlx::query("UPDATE briefs SET uses = uses + 1, last_used_at = ? WHERE id = ?")
            .bind(now())
            .bind(id.to_string())
            .execute(&self.pool)
            .await
            .context("touch brief")?;
        Ok(())
    }

    /// The model registered a topic by reference with a new spoken label:
    /// append it (≤ 4 words; `updated_at` untouched — not a content change).
    /// `Ok(true)` when added, `Ok(false)` when already there.
    pub async fn add_alias(&self, id: BriefId, alias: &str) -> Result<bool, String> {
        let alias = alias.split_whitespace().collect::<Vec<_>>().join(" ");
        check_label(&alias)?;
        let mut r = self.get(id).await?;
        if r.aliases.iter().any(|a| a.eq_ignore_ascii_case(&alias)) {
            return Ok(false);
        }
        r.aliases.push(alias);
        let mut tx = self.pool.begin().await.map_err(|e| e.to_string())?;
        sqlx::query("UPDATE briefs SET aliases = ? WHERE id = ?")
            .bind(json(&r.aliases))
            .bind(id.to_string())
            .execute(&mut *tx)
            .await
            .map_err(|e| e.to_string())?;
        Self::index(&mut tx, id, &r.aliases, &r.title, &r.tags, &r.bullets).await?;
        tx.commit().await.map_err(|e| e.to_string())?;
        let _ = self.reload().await;
        Ok(true)
    }
}

/// Every alias spoken in `text`: case-insensitive, word boundaries on both
/// ends (edge or a non-alphanumeric char), aliases shorter than 3 chars
/// without a digit skipped; one hit per row, in order of first occurrence.
/// The `String` is the alias as it appears in `text`.
pub fn scan(text: &str, aliases: &[(String, BriefId)]) -> Vec<(BriefId, String)> {
    let lower = text.to_lowercase();
    // Byte offsets into `lower` map onto `text` when lowercasing kept the length (ASCII and most Latin).
    let same_len = lower.len() == text.len();
    let mut hits: Vec<(usize, BriefId, String)> = Vec::new();
    for (alias, id) in aliases {
        if alias.is_empty() || (alias.chars().count() < 3 && !alias.chars().any(|c| c.is_ascii_digit())) {
            continue;
        }
        let mut from = 0;
        while let Some(i) = lower[from..].find(alias.as_str()) {
            let start = from + i;
            let end = start + alias.len();
            let before = lower[..start].chars().next_back().is_none_or(|c| !c.is_alphanumeric());
            let after = lower[end..].chars().next().is_none_or(|c| !c.is_alphanumeric());
            if before && after {
                let spoken = if same_len { text[start..end].to_owned() } else { alias.clone() };
                hits.push((start, *id, spoken));
                break;
            }
            from = end;
        }
    }
    hits.sort_by_key(|h| h.0);
    let mut seen: Vec<BriefId> = Vec::new();
    hits.into_iter()
        .filter(|(_, id, _)| {
            if seen.contains(id) {
                false
            } else {
                seen.push(*id);
                true
            }
        })
        .map(|(_, id, spoken)| (id, spoken))
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn aliases(list: &[(&str, BriefId)]) -> Vec<(String, BriefId)> {
        list.iter().map(|(a, id)| (a.to_lowercase(), *id)).collect()
    }

    /// Word boundaries (`DR` is too short and never a substring hit), digits,
    /// case, first-occurrence order, one hit per row.
    #[test]
    fn scan_rules() {
        let (dr, sre, keda) = (BriefId::new(), BriefId::new(), BriefId::new());
        let a = aliases(&[
            ("DR", dr),
            ("the DR cutover", dr),
            ("SRE-2386", sre),
            ("2386", sre),
            ("KEDA", keda),
        ]);
        assert!(scan("the drive is full", &a).is_empty());
        assert!(scan("kedab and dr", &a).is_empty(), "no boundary after, `dr` too short");
        assert_eq!(
            scan("So KEDA, then the dr CUTOVER, then sre-2386.", &a),
            [(keda, "KEDA".to_owned()), (dr, "the dr CUTOVER".to_owned()), (sre, "sre-2386".to_owned())]
        );
        // Two aliases of one row → one hit, at the earliest occurrence.
        assert_eq!(scan("ticket 2386 is SRE-2386", &a), [(sre, "2386".to_owned())]);
        // A boundary failure on the first occurrence does not hide a later one.
        assert_eq!(scan("kedas KEDA", &a), [(keda, "KEDA".to_owned())]);
        assert_eq!(scan("(SRE-2386)", &a), [(sre, "SRE-2386".to_owned())]);
    }

    fn put(title: &str, aliases: &[&str], bullets: &[&str], tags: &[&str]) -> CachedBriefPut {
        CachedBriefPut {
            id: None,
            title: title.into(),
            bullets: bullets.iter().map(|s| (*s).to_owned()).collect(),
            aliases: aliases.iter().map(|s| (*s).to_owned()).collect(),
            tags: tags.iter().map(|s| (*s).to_owned()).collect(),
        }
    }

    /// Put/update/delete round trip on a temp db: validation, kept fields on
    /// update, alias hit ranks above a bullet hit, exact alias first, list by
    /// tag, aliases reload, alias learning leaves `updated_at` alone.
    #[tokio::test]
    async fn round_trip() {
        let dir = tempfile::tempdir().unwrap();
        let c = Cache::open(dir.path()).await.unwrap();
        assert!(c.put(put("x", &[], &["a", "b"], &[]), "user").await.is_err());
        assert!(c.put(put("x", &["a b c d e"], &["a", "b"], &[]), "user").await.is_err());
        assert!(c.put(put("x", &["ok"], &["a"], &[]), "user").await.is_err());
        assert!(c.put(put(" ", &["ok"], &["a", "b"], &[]), "user").await.is_err());

        let (sre, created) = c
            .put(
                put("SRE-2386 image tags", &["SRE-2386", " sre-2386 ", "two three eight six"], &["DR sync blocked on image tags", "owner: Dana"], &["dr-sync"]),
                "meeting:m1",
            )
            .await
            .unwrap();
        assert!(created);
        assert_eq!(sre.aliases, ["SRE-2386", "two three eight six"], "case-insensitive dedupe");
        assert_eq!((sre.uses, sre.source.as_str()), (0, "meeting:m1"));
        let (keda, _) = c
            .put(put("KEDA scaling", &["KEDA"], &["limit 20k", "mentions SRE-2386 tags"], &["dr-sync", "platform"]), "session:s1")
            .await
            .unwrap();
        assert_eq!(c.aliases().len(), 3);

        // Alias hit (SRE row) beats the bullet hit (KEDA row); exact single word too.
        let hits = c.search("SRE-2386", 8).await.unwrap();
        assert_eq!(hits.iter().map(|h| h.id).collect::<Vec<_>>(), [sre.id, keda.id]);
        // Exact alias first even when bm25 would not rank it so: the KEDA row
        // carries "tags" in its bullets, the SRE row only in its title — but
        // "keda" is an alias of the KEDA row and a single word, so it leads.
        let hits = c.search("keda", 8).await.unwrap();
        assert_eq!(hits[0].id, keda.id);
        assert!(c.search("nothing here", 8).await.unwrap().is_empty());

        // Update keeps source/created_at/uses, moves updated_at; the old alias is gone from the list.
        c.touch_use(sre.id).await.unwrap();
        tokio::time::sleep(Duration::from_millis(5)).await;
        let (sre2, created) = c
            .put(
                CachedBriefPut {
                    id: Some(sre.id),
                    title: "SRE-2386".into(),
                    bullets: vec!["fixed".into(), "closed".into()],
                    aliases: vec!["SRE-2386".into()],
                    tags: vec![],
                },
                "user",
            )
            .await
            .unwrap();
        assert!(!created);
        assert_eq!((sre2.uses, sre2.source.as_str(), &sre2.created_at), (1, "meeting:m1", &sre.created_at));
        assert!(sre2.updated_at > sre.updated_at && sre2.last_used_at.is_some());
        assert_eq!(c.aliases().len(), 2);

        // add_alias: appended once, indexed, updated_at untouched.
        assert!(c.add_alias(sre.id, "the failover thing").await.unwrap());
        assert!(!c.add_alias(sre.id, "The Failover Thing").await.unwrap());
        assert!(c.add_alias(sre.id, "a b c d e").await.is_err());
        let r = c.get(sre.id).await.unwrap();
        assert_eq!((r.aliases.len(), &r.updated_at), (2, &sre2.updated_at));
        assert_eq!(c.search("failover", 8).await.unwrap()[0].id, sre.id);

        // list: newest updated first; by tag.
        let all = c.list(None, 50).await.unwrap();
        assert_eq!(all.iter().map(|h| h.id).collect::<Vec<_>>(), [sre.id, keda.id]);
        assert_eq!(c.list(Some("platform"), 50).await.unwrap().len(), 1);
        assert!(c.list(Some("dr-sync"), 50).await.unwrap().iter().all(|h| h.id == keda.id));
        assert_eq!(c.tags().await.unwrap(), ["dr-sync", "platform"]);

        c.delete(keda.id).await.unwrap();
        assert!(c.delete(keda.id).await.is_err());
        assert!(c.get(keda.id).await.is_err());
        assert!(c.search("keda", 8).await.unwrap().is_empty());
        assert_eq!(c.aliases().len(), 2);
    }
}
