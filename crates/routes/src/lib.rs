//! Routes (design doc 44): learn a site's JSON routes from the browser
//! session's XHR/fetch traffic and replay them over `web::Fetcher` without a
//! browser. `classify` and `store` are the pure core; [`Routes`] is the shared
//! handle behind the `site_*` tools (44 §3, §4). No actor: nothing outside
//! this crate asks it, so 44 §6's messages are methods.

use std::collections::{BTreeMap, HashMap};
use std::path::Path;
use std::sync::Arc;
use std::time::{Duration, Instant};

use ::tools::{REGISTRY_ACTOR, ToolRegistry};
use runtime::{Runtime, SessionId};
use serde::{Deserialize, Serialize};
use serde_json::{Map, Value};
use url::{Position, Url};
use web::fetch::Fetcher;
use zeroize::Zeroizing;

mod capture;
mod classify;
mod replay;
mod store;
mod tools;

pub use classify::{
    Collection, Exchange, Hole, HoleIn, Learned, Route, classify_all, decode, drift, fill,
    find_collection, registrable_domain,
};
pub use store::{SiteRoutes, Store};

/// `routes:` config section (44 §5). Every field has a default; the section
/// may be absent.
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(default)]
pub struct RoutesConfig {
    /// `false`: no tools, no prompt block, no directory touched.
    pub enabled: bool,
    /// Sleep after `networkidle0` so late fetches land before the capture.
    pub settle_ms: u64,
    /// Timeout for one `site_call`.
    pub call_timeout_ms: u64,
    /// Replayed body cap.
    pub max_body_bytes: usize,
}

impl Default for RoutesConfig {
    fn default() -> Self {
        Self {
            enabled: true,
            settle_ms: 4000,
            call_timeout_ms: 15000,
            max_body_bytes: 2 * 1024 * 1024,
        }
    }
}

/// Credentials untouched this long are dropped (44 §3.6; mirrors
/// `browser.idle_secs`' default).
pub const CREDS_IDLE: Duration = Duration::from_secs(300);

/// How often [`Routes::sweep`] runs.
const SWEEP_EVERY: Duration = Duration::from_secs(30);

/// Session-scoped credential values for one domain (44 §3.6); never serialised.
struct CredEntry {
    touched: Instant,
    /// `(lowercase header name, value)`, `cookie` included when known.
    headers: Vec<(String, Zeroizing<String>)>,
}

/// The routes runtime: store, fetcher and the session credential map.
pub struct Routes {
    cfg: RoutesConfig,
    store: Store,
    fetcher: Fetcher,
    creds: tokio::sync::Mutex<HashMap<(SessionId, String), CredEntry>>,
}

/// `site_learn` result (44 §4).
#[derive(Debug, Serialize)]
pub struct LearnReply {
    /// Registrable domain the rows belong to.
    pub domain: String,
    /// Every route stored for `domain` after the merge.
    pub routes: Vec<Route>,
    /// Exchanges drained from the capture buffer.
    pub captured: usize,
    /// ≤ 3 distinct captured URLs (query dropped) when traffic was seen but
    /// nothing classified; else empty.
    pub top_urls: Vec<String>,
}

/// `site_call` result (44 §3.5).
#[derive(Debug, Serialize)]
pub struct CallReply {
    /// The filled URL that was requested.
    pub url: String,
    /// HTTP status.
    pub status: u16,
    /// `records.len()`.
    pub count: usize,
    /// JSON pointer the records were read from.
    pub pointer: String,
    /// The records.
    pub records: Vec<Value>,
    /// `auth_headers` names the session had no value for.
    pub unarmed: Vec<String>,
}

impl Routes {
    /// Store under `<data_dir>/routes`, an empty credential map.
    #[must_use]
    pub fn new(cfg: RoutesConfig, data_dir: &Path, fetcher: Fetcher) -> Self {
        Self {
            cfg,
            store: Store::new(data_dir),
            fetcher,
            creds: tokio::sync::Mutex::new(HashMap::new()),
        }
    }

    /// 44 §3.2–§3.4, §3.6: with `url`, navigate this session's isolate there
    /// (`networkidle0`) and settle; drain the capture buffer, classify, merge
    /// per registrable domain, arm the session's credentials. Replies for the
    /// domain of `url`, else the domain with most routes.
    ///
    /// # Errors
    /// No browser actor, a failed navigation, an unparseable capture, or a
    /// store write failure.
    pub async fn learn(&self, session: SessionId, url: Option<&Url>) -> anyhow::Result<LearnReply> {
        if let Some(u) = url {
            capture::navigate(session, u).await?;
            tokio::time::sleep(Duration::from_millis(self.cfg.settle_ms)).await;
        }
        let exs = capture::exchanges(session).await?;
        let captured = exs.len();
        let grouped = group(classify_all(&exs));
        let top_urls = if grouped.is_empty() {
            first_urls(&exs)
        } else {
            Vec::new()
        };
        let domain = url.map(registrable_domain).or_else(|| {
            grouped
                .iter()
                .max_by_key(|(_, v)| v.len())
                .map(|(d, _)| d.clone())
                .or_else(|| {
                    exs.first()
                        .and_then(|e| Url::parse(&e.url).ok())
                        .map(|u| registrable_domain(&u))
                })
        });
        for (d, learned) in grouped {
            let cookie = capture::cookie_header(session, &d).await;
            self.absorb(session, &d, learned, cookie).await?;
        }
        let domain = domain.unwrap_or_default();
        let routes = self.store.load(&domain).await?.routes;
        Ok(LearnReply {
            domain,
            routes,
            captured,
            top_urls,
        })
    }

    /// Merge one domain's classified routes into the store and its credential
    /// values into the session map (later values win). A non-empty jar
    /// `cookie` marks every route `cookies: true` and adds `cookie` to its
    /// `auth_headers`.
    async fn absorb(
        &self,
        session: SessionId,
        domain: &str,
        learned: Vec<Learned>,
        cookie: Option<String>,
    ) -> anyhow::Result<SiteRoutes> {
        let mut routes = Vec::with_capacity(learned.len());
        let mut fresh: Vec<(String, Zeroizing<String>)> = Vec::new();
        for l in learned {
            let mut route = l.route;
            if cookie.is_some() {
                route.cookies = true;
                if let Err(i) = route
                    .auth_headers
                    .binary_search_by(|h| h.as_str().cmp("cookie"))
                {
                    route.auth_headers.insert(i, "cookie".to_owned());
                }
            }
            routes.push(route);
            fresh.extend(l.creds.into_iter().map(|(k, v)| (k, Zeroizing::new(v))));
        }
        if let Some(c) = cookie {
            fresh.push(("cookie".to_owned(), Zeroizing::new(c)));
        }
        let site = self.store.merge(domain, routes).await?;
        if fresh.is_empty() {
            return Ok(site);
        }
        let mut creds = self.creds.lock().await;
        let entry = creds
            .entry((session, domain.to_owned()))
            .or_insert_with(|| CredEntry {
                touched: Instant::now(),
                headers: Vec::new(),
            });
        entry.touched = Instant::now();
        for (k, v) in fresh {
            match entry.headers.iter_mut().find(|(n, _)| *n == k) {
                Some(slot) => slot.1 = v,
                None => entry.headers.push((k, v)),
            }
        }
        Ok(site)
    }

    /// The stored routes for `url`'s registrable domain.
    ///
    /// # Errors
    /// Store read failure.
    pub async fn list(&self, url: &Url) -> anyhow::Result<SiteRoutes> {
        self.store.load(&registrable_domain(url)).await
    }

    /// 44 §3.5: fill, arm, request, drift-check (`replay::call`).
    ///
    /// # Errors
    /// Unknown or non-replayable route, missing path params, transport
    /// failure, timeout, or drift (the route is marked `stale`).
    pub async fn call(
        &self,
        session: SessionId,
        route_id: &str,
        params: &Map<String, Value>,
    ) -> anyhow::Result<CallReply> {
        replay::call(self, session, route_id, params).await
    }

    /// Drop credential entries untouched for [`CREDS_IDLE`].
    pub async fn sweep(&self) {
        self.creds
            .lock()
            .await
            .retain(|_, e| e.touched.elapsed() < CREDS_IDLE);
    }
}

/// Group by the template's registrable domain, keeping rank order within a
/// domain.
fn group(learned: Vec<Learned>) -> BTreeMap<String, Vec<Learned>> {
    let mut out: BTreeMap<String, Vec<Learned>> = BTreeMap::new();
    for l in learned {
        let Ok(u) = Url::parse(&l.route.template) else {
            continue;
        };
        out.entry(registrable_domain(&u)).or_default().push(l);
    }
    out
}

/// ≤ 3 distinct captured URLs with query and fragment dropped, capture order.
fn first_urls(exs: &[Exchange]) -> Vec<String> {
    let mut out: Vec<String> = Vec::with_capacity(3);
    for ex in exs {
        let Ok(u) = Url::parse(&ex.url) else {
            continue;
        };
        let s = &u[..Position::AfterPath];
        if !out.iter().any(|o| o == s) {
            out.push(s.to_owned());
            if out.len() == 3 {
                break;
            }
        }
    }
    out
}

/// The plugin: reads `routes`, builds the fetcher, registers `site_learn` /
/// `site_routes` / `site_call` once the tool registry is up, and sweeps idle
/// credentials until shutdown.
pub struct RoutesPlugin;

#[async_trait::async_trait]
impl runtime::Plugin for RoutesPlugin {
    fn name(&self) -> &'static str {
        "routes"
    }

    async fn start(self: Box<Self>, rt: &Runtime) -> anyhow::Result<()> {
        let cfg: RoutesConfig = rt.section("routes").unwrap_or_else(|e| {
            tracing::debug!("no [routes] config section, using defaults: {e}");
            RoutesConfig::default()
        });
        if !cfg.enabled {
            tracing::info!("routes disabled by config");
            return Ok(());
        }
        let fetcher = Fetcher::new_with_timeouts(
            web::engines::USER_AGENT,
            cfg.max_body_bytes,
            web::WebConfig::default().max_redirects,
            Duration::from_millis(cfg.call_timeout_ms.max(1)),
            web::fetch::CONNECT_TIMEOUT,
        )?;
        let routes = Arc::new(Routes::new(cfg, &rt.data_dir, fetcher));
        let sweeper = Arc::clone(&routes);
        let shutdown = rt.shutdown.clone();
        tokio::spawn(async move {
            let mut tick = tokio::time::interval(SWEEP_EVERY);
            loop {
                tokio::select! {
                    () = shutdown.cancelled() => break,
                    _ = tick.tick() => sweeper.sweep().await,
                }
            }
        });
        let rt = rt.clone();
        tokio::spawn(async move {
            if let Some(reg) = rt.lookup_when::<ToolRegistry>(REGISTRY_ACTOR).await {
                tools::register(&reg, routes).await;
            }
        });
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn defaults_match_design() {
        let d = RoutesConfig::default();
        assert!(d.enabled);
        assert_eq!(d.settle_ms, 4000);
        assert_eq!(d.call_timeout_ms, 15_000);
        assert_eq!(d.max_body_bytes, 2 * 1024 * 1024);
    }

    #[test]
    fn empty_section_is_defaults() {
        let d: RoutesConfig = serde_json::from_str("{}").expect("empty section");
        assert!(d.enabled);
        assert_eq!(d.settle_ms, 4000);
        assert_eq!(d.max_body_bytes, 2 * 1024 * 1024);
    }

    fn exchange(url: &str, auth: &[(&str, &str)]) -> Exchange {
        let body = r#"[{"id":1,"n":"a"},{"id":2,"n":"b"}]"#;
        Exchange {
            url: url.to_owned(),
            method: "GET".to_owned(),
            resource_type: "Fetch".to_owned(),
            status: 200,
            request_headers: auth
                .iter()
                .map(|(k, v)| ((*k).to_owned(), (*v).to_owned()))
                .collect(),
            response_headers: HashMap::new(),
            content_type: Some("application/json".to_owned()),
            body: Some(body.to_owned()),
            body_len: body.len(),
            truncated: false,
            ts: 0,
        }
    }

    fn tmp_routes() -> (std::path::PathBuf, Routes) {
        let nanos = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .expect("clock")
            .as_nanos();
        let dir = std::env::temp_dir().join(format!("wth-routes-lib-{nanos}"));
        let fetcher = Fetcher::new("test", 1024, 1).expect("fetcher");
        let routes = Routes::new(RoutesConfig::default(), &dir, fetcher);
        (dir, routes)
    }

    #[tokio::test]
    async fn absorb_groups_by_domain_and_keeps_values_off_disk() {
        let (dir, routes) = tmp_routes();
        let session = SessionId::new();
        let grouped = group(classify_all(&[
            exchange(
                "https://api.example.com/v1/items?page=1",
                &[("authorization", "Bearer SECRET-TOKEN")],
            ),
            exchange("https://other.org/list", &[]),
        ]));
        assert_eq!(
            grouped.keys().collect::<Vec<_>>(),
            ["example.com", "other.org"]
        );

        let mut sites = Vec::new();
        for (d, learned) in grouped {
            let cookie = (d == "example.com").then(|| "sid=SECRET-COOKIE".to_owned());
            sites.push(
                routes
                    .absorb(session, &d, learned, cookie)
                    .await
                    .expect("absorb"),
            );
        }
        let example = &sites[0];
        assert_eq!(example.routes.len(), 1);
        assert!(example.routes[0].cookies);
        assert_eq!(example.routes[0].auth_headers, ["authorization", "cookie"]);
        assert!(!sites[1].routes[0].cookies);

        let file =
            std::fs::read_to_string(dir.join("routes").join("example.com.json")).expect("file");
        assert!(file.contains("example.com:GET:/v1/items"));
        assert!(!file.contains("SECRET"));

        let reply = serde_json::to_string(&LearnReply {
            domain: "example.com".to_owned(),
            routes: example.routes.clone(),
            captured: 2,
            top_urls: Vec::new(),
        })
        .expect("json");
        assert!(!reply.contains("SECRET"));

        let creds = routes.creds.lock().await;
        let entry = &creds[&(session, "example.com".to_owned())];
        assert_eq!(entry.headers.len(), 2);
        assert!(
            entry
                .headers
                .iter()
                .any(|(n, v)| n == "cookie" && v.as_str() == "sid=SECRET-COOKIE")
        );
        assert!(!creds.contains_key(&(session, "other.org".to_owned())));
        drop(creds);

        std::fs::remove_dir_all(&dir).expect("cleanup");
    }

    #[tokio::test]
    async fn sweep_drops_idle_entries_only() {
        let (_, routes) = tmp_routes();
        let session = SessionId::new();
        let stale = Instant::now()
            .checked_sub(CREDS_IDLE + Duration::from_secs(1))
            .expect("instant");
        {
            let mut creds = routes.creds.lock().await;
            creds.insert(
                (session, "old.io".to_owned()),
                CredEntry {
                    touched: stale,
                    headers: vec![("cookie".to_owned(), Zeroizing::new("a=b".to_owned()))],
                },
            );
            creds.insert(
                (session, "fresh.io".to_owned()),
                CredEntry {
                    touched: Instant::now(),
                    headers: Vec::new(),
                },
            );
        }
        routes.sweep().await;
        let creds = routes.creds.lock().await;
        assert!(!creds.contains_key(&(session, "old.io".to_owned())));
        assert!(creds.contains_key(&(session, "fresh.io".to_owned())));
    }
}
