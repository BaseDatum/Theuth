//! `<data_dir>/routes/<registrable-domain>.json` (44 §3.4): one pretty JSON
//! file per domain, written whole via `.tmp` + rename. Delete the file to
//! forget a site.

use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

use crate::classify::Route;

/// One domain's learned routes.
#[derive(Clone, Debug, Default, Serialize, Deserialize)]
pub struct SiteRoutes {
    /// Registrable domain.
    pub domain: String,
    /// Last merge, RFC 3339.
    pub learned_at: Option<String>,
    /// The routes.
    pub routes: Vec<Route>,
}

/// The routes directory.
pub struct Store {
    dir: PathBuf,
}

impl Store {
    /// `<data_dir>/routes`.
    #[must_use]
    pub fn new(data_dir: &Path) -> Self {
        Self {
            dir: data_dir.join("routes"),
        }
    }

    fn path(&self, domain: &str) -> PathBuf {
        self.dir.join(format!("{domain}.json"))
    }

    /// Load a domain's file; missing → empty with `domain` set.
    pub async fn load(&self, domain: &str) -> anyhow::Result<SiteRoutes> {
        match tokio::fs::read(self.path(domain)).await {
            Ok(bytes) => Ok(serde_json::from_slice(&bytes)?),
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(SiteRoutes {
                domain: domain.to_owned(),
                ..SiteRoutes::default()
            }),
            Err(e) => Err(e.into()),
        }
    }

    async fn save(&self, site: &SiteRoutes) -> anyhow::Result<()> {
        tokio::fs::create_dir_all(&self.dir).await?;
        let path = self.path(&site.domain);
        let tmp = path.with_extension("json.tmp");
        tokio::fs::write(&tmp, serde_json::to_vec_pretty(site)?).await?;
        tokio::fs::rename(&tmp, &path).await?;
        Ok(())
    }

    /// Merge freshly classified routes by `(method, template)`: known routes
    /// get `seen + 1` and refreshed shape, new ones `seen = 1`; `stale` clears.
    pub async fn merge(&self, domain: &str, routes: Vec<Route>) -> anyhow::Result<SiteRoutes> {
        let mut site = self.load(domain).await?;
        for fresh in routes {
            match site
                .routes
                .iter_mut()
                .find(|r| r.method == fresh.method && r.template == fresh.template)
            {
                Some(r) => {
                    r.seen += 1;
                    r.keys = fresh.keys;
                    r.records_seen = fresh.records_seen;
                    r.example_url = fresh.example_url;
                    r.auth_headers = fresh.auth_headers;
                    r.cookies = fresh.cookies;
                    r.replay = fresh.replay;
                    r.holes = fresh.holes;
                    r.stale = false;
                }
                None => site.routes.push(Route { seen: 1, ..fresh }),
            }
        }
        site.learned_at = Some(chrono::Utc::now().to_rfc3339());
        self.save(&site).await?;
        Ok(site)
    }

    /// Record a replay outcome: `ok` sets `last_ok` and clears `stale`;
    /// `!ok` sets `stale`. Unknown `route_id` is a no-op.
    pub async fn mark(&self, domain: &str, route_id: &str, ok: bool) -> anyhow::Result<()> {
        let mut site = self.load(domain).await?;
        let Some(r) = site.routes.iter_mut().find(|r| r.id == route_id) else {
            return Ok(());
        };
        if ok {
            r.last_ok = Some(chrono::Utc::now().to_rfc3339());
            r.stale = false;
        } else {
            r.stale = true;
        }
        self.save(&site).await
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::classify::{Exchange, classify_all};
    use std::collections::HashMap;

    fn route() -> Route {
        let body = r#"[{"id":1},{"id":2}]"#;
        classify_all(&[Exchange {
            url: "https://x.io/items".to_owned(),
            method: "GET".to_owned(),
            resource_type: "Fetch".to_owned(),
            status: 200,
            request_headers: HashMap::new(),
            response_headers: HashMap::new(),
            content_type: Some("application/json".to_owned()),
            body: Some(body.to_owned()),
            body_len: body.len(),
            truncated: false,
            ts: 0,
        }])
        .remove(0)
        .route
    }

    fn tmp_dir() -> PathBuf {
        let nanos = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .expect("clock")
            .as_nanos();
        std::env::temp_dir().join(format!("wth-routes-{nanos}"))
    }

    #[tokio::test]
    async fn merge_bumps_seen_mark_sets_stale() {
        let dir = tmp_dir();
        let store = Store::new(&dir);
        assert!(store.load("x.io").await.expect("load").routes.is_empty());

        let first = store.merge("x.io", vec![route()]).await.expect("merge");
        assert_eq!(first.routes[0].seen, 1);
        assert!(first.learned_at.is_some());
        assert!(dir.join("routes").join("x.io.json").is_file());

        let id = first.routes[0].id.clone();
        store.mark("x.io", &id, false).await.expect("mark");
        assert!(store.load("x.io").await.expect("load").routes[0].stale);

        let second = store.merge("x.io", vec![route()]).await.expect("merge");
        assert_eq!(second.routes.len(), 1);
        assert_eq!(second.routes[0].seen, 2);
        assert!(!second.routes[0].stale);

        store.mark("x.io", &id, true).await.expect("mark");
        let loaded = store.load("x.io").await.expect("load");
        assert!(loaded.routes[0].last_ok.is_some());

        std::fs::remove_dir_all(&dir).expect("cleanup");
    }
}
