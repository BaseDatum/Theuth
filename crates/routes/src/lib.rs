//! Routes (design doc 44): learn a site's JSON routes from the browser
//! session's XHR/fetch traffic and replay them over `web::Fetcher` without a
//! browser. `classify` and `store` are the pure core; [`Routes`] is the shared
//! handle behind the `site_*` tools (44 §3, §4). No actor: nothing outside
//! this crate asks it, so 44 §6's messages are methods.

use std::collections::{BTreeMap, HashMap};
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::{Duration, Instant};

use ::tools::{CloseAck, CloseTree, REGISTRY_ACTOR, RegisterLifecycle, ToolRegistry};
use kameo::actor::Spawn;
use kameo::message::{Context, Message};
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
    /// Domain of each session's last `learn`, so `site_learn()` with no url
    /// on a quiet page still answers for the site the session is on. Never
    /// swept: one `String` per session ever seen is nothing.
    last_domain: tokio::sync::Mutex<HashMap<SessionId, String>>,
    /// 51 §5: a private tree's learned routes, by its scratch root and
    /// domain; never merged into `store`, dropped on [`CloseTree`].
    private: tokio::sync::Mutex<HashMap<PathBuf, HashMap<String, SiteRoutes>>>,
    /// Private session → its root, recorded on `learn`, so a close by
    /// subtree finds the root's map.
    members: tokio::sync::Mutex<HashMap<SessionId, PathBuf>>,
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
    /// One model-facing sentence: what was captured and what to do next.
    pub hint: String,
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
            last_domain: tokio::sync::Mutex::new(HashMap::new()),
            private: tokio::sync::Mutex::new(HashMap::new()),
            members: tokio::sync::Mutex::new(HashMap::new()),
        }
    }

    /// A domain's routes: the private root's map when `root` is given, else
    /// the disk store.
    pub(crate) async fn load(
        &self,
        root: Option<&Path>,
        domain: &str,
    ) -> anyhow::Result<SiteRoutes> {
        let Some(root) = root else {
            return self.store.load(domain).await;
        };
        Ok(self
            .private
            .lock()
            .await
            .get(root)
            .and_then(|m| m.get(domain))
            .cloned()
            .unwrap_or_else(|| SiteRoutes {
                domain: domain.to_owned(),
                ..SiteRoutes::default()
            }))
    }

    async fn merge(
        &self,
        root: Option<&Path>,
        domain: &str,
        routes: Vec<Route>,
    ) -> anyhow::Result<SiteRoutes> {
        let Some(root) = root else {
            return self.store.merge(domain, routes).await;
        };
        let mut private = self.private.lock().await;
        let site = private
            .entry(root.to_path_buf())
            .or_default()
            .entry(domain.to_owned())
            .or_insert_with(|| SiteRoutes {
                domain: domain.to_owned(),
                ..SiteRoutes::default()
            });
        store::merge_into(site, routes);
        Ok(site.clone())
    }

    pub(crate) async fn mark(
        &self,
        root: Option<&Path>,
        domain: &str,
        route_id: &str,
        ok: bool,
    ) -> anyhow::Result<()> {
        let Some(root) = root else {
            return self.store.mark(domain, route_id, ok).await;
        };
        if let Some(site) = self
            .private
            .lock()
            .await
            .get_mut(root)
            .and_then(|m| m.get_mut(domain))
        {
            store::mark_in(site, route_id, ok);
        }
        Ok(())
    }

    /// [`CloseTree`]: drop the subtree's credentials, last domains and, for
    /// any private member, its root's routes.
    pub async fn close(&self, subtree: &[SessionId]) {
        let roots: Vec<PathBuf> = {
            let mut members = self.members.lock().await;
            subtree.iter().filter_map(|s| members.remove(s)).collect()
        };
        if !roots.is_empty() {
            let mut private = self.private.lock().await;
            for r in &roots {
                private.remove(r);
            }
        }
        self.creds
            .lock()
            .await
            .retain(|(s, _), _| !subtree.contains(s));
        self.last_domain
            .lock()
            .await
            .retain(|s, _| !subtree.contains(s));
    }

    /// 44 §3.2–§3.4, §3.6: with `url`, navigate this session's isolate there
    /// (`networkidle0`) and settle; drain the capture buffer, classify, merge
    /// per registrable domain, arm the session's credentials. Replies for the
    /// domain of `url`, else the domain with most routes. `root` is the
    /// private tree's scratch root (51 §5): its routes stay in memory under
    /// it; `None` merges into `<data_dir>/routes`.
    ///
    /// # Errors
    /// No browser actor, a failed navigation, an unparseable capture, or a
    /// store write failure.
    pub async fn learn(
        &self,
        session: SessionId,
        root: Option<&Path>,
        url: Option<&Url>,
    ) -> anyhow::Result<LearnReply> {
        if let Some(root) = root {
            self.members
                .lock()
                .await
                .insert(session, root.to_path_buf());
        }
        if let Some(u) = url {
            capture::navigate(session, u).await?;
            tokio::time::sleep(Duration::from_millis(self.cfg.settle_ms)).await;
        }
        let exs = capture::exchanges(session).await?;
        let captured = exs.len();
        let grouped = group(classify_all(&exs));
        let classified = !grouped.is_empty();
        let top_urls = if classified {
            Vec::new()
        } else {
            first_urls(&exs)
        };
        // url → what was classified → the session's last domain → whatever
        // the first stray exchange hit (a late beacon must not hide stored rows).
        let domain = {
            let mut last = self.last_domain.lock().await;
            let d = url
                .map(registrable_domain)
                .or_else(|| {
                    grouped
                        .iter()
                        .max_by_key(|(_, v)| v.len())
                        .map(|(d, _)| d.clone())
                })
                .or_else(|| last.get(&session).cloned())
                .or_else(|| {
                    exs.first()
                        .and_then(|e| Url::parse(&e.url).ok())
                        .map(|u| registrable_domain(&u))
                })
                .unwrap_or_default();
            if !d.is_empty() {
                last.insert(session, d.clone());
            }
            d
        };
        for (d, learned) in grouped {
            let cookie = capture::cookie_header(session, &d).await;
            self.absorb(session, root, &d, learned, cookie).await?;
        }
        let routes = self.load(root, &domain).await?.routes;
        let hint = hint(&exs, classified, &top_urls, &domain, routes.len());
        Ok(LearnReply {
            domain,
            routes,
            captured,
            top_urls,
            hint,
        })
    }

    /// Merge one domain's classified routes into the store and its credential
    /// values into the session map (later values win). A non-empty jar
    /// `cookie` marks every route `cookies: true` and adds `cookie` to its
    /// `auth_headers`.
    async fn absorb(
        &self,
        session: SessionId,
        root: Option<&Path>,
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
        let site = self.merge(root, domain, routes).await?;
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

    /// The stored routes for `url`'s registrable domain (`root` as in
    /// [`Self::learn`]).
    ///
    /// # Errors
    /// Store read failure.
    pub async fn list(&self, root: Option<&Path>, url: &Url) -> anyhow::Result<SiteRoutes> {
        self.load(root, &registrable_domain(url)).await
    }

    /// 44 §3.5: fill, arm, request, drift-check (`replay::call`); `root` as
    /// in [`Self::learn`].
    ///
    /// # Errors
    /// Unknown or non-replayable route, missing path params, transport
    /// failure, timeout, or drift (the route is marked `stale`).
    pub async fn call(
        &self,
        session: SessionId,
        root: Option<&Path>,
        route_id: &str,
        params: &Map<String, Value>,
    ) -> anyhow::Result<CallReply> {
        replay::call(self, session, root, route_id, params).await
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

/// The model-facing one-liner for a `learn` reply. Distinguishes the empty
/// cases the model otherwise reads as one "zero": nothing captured, a full
/// navigation (HTML + beacons, no JSON), or JSON with no record collections.
fn hint(exs: &[Exchange], classified: bool, top_urls: &[String], domain: &str, n: usize) -> String {
    let captured = exs.len();
    if classified {
        return format!("{n} routes for {domain}; call one with site_call");
    }
    if n > 0 {
        return format!("nothing new captured; {n} stored routes for {domain}");
    }
    if captured == 0 {
        return "no traffic captured — nothing loaded since the last site_learn; \
                click a control that changes the list, then site_learn() again"
            .to_owned();
    }
    let json = exs.iter().any(|e| {
        (200..300).contains(&e.status)
            && e.content_type
                .as_deref()
                .is_some_and(|c| c.contains("json"))
            && e.body.as_deref().is_some_and(|b| !b.is_empty())
    });
    if json {
        format!(
            "{captured} requests, no record collections; seen: {}",
            top_urls.join(", ")
        )
    } else {
        format!(
            "{captured} requests, HTML page + beacons only (a full navigation or server-rendered list); \
             click a filter/sort/search that updates the list in place, then site_learn() with no url; \
             scrape_url reads the HTML"
        )
    }
}

/// The `"routes"` lifecycle participant (51 §4.4): answers [`CloseTree`]
/// with [`Routes::close`]. Nothing to prepare on open, so no `opened`.
#[derive(kameo::Actor)]
struct Lifecycle(Arc<Routes>);

impl Message<CloseTree> for Lifecycle {
    type Reply = CloseAck;

    async fn handle(&mut self, m: CloseTree, _: &mut Context<Self, Self::Reply>) -> CloseAck {
        self.0.close(&m.subtree).await;
        CloseAck::ok("routes")
    }
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
        let web_cfg: web::WebConfig = rt.section("web").unwrap_or_default();
        let fetcher = Fetcher::new_with_timeouts(
            web::engines::USER_AGENT,
            cfg.max_body_bytes,
            web::WebConfig::default().max_redirects,
            Duration::from_millis(cfg.call_timeout_ms.max(1)),
            web::fetch::CONNECT_TIMEOUT,
        )?
        .with_private_network(web_cfg.allow_private_network)?;
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
                let lifecycle = Lifecycle::spawn(Lifecycle(Arc::clone(&routes)));
                let _ = reg
                    .ask(RegisterLifecycle {
                        owner: "routes",
                        opened: None,
                        close: lifecycle.reply_recipient(),
                    })
                    .await;
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
        static N: std::sync::atomic::AtomicU32 = std::sync::atomic::AtomicU32::new(0);
        let n = N.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
        let nanos = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .expect("clock")
            .as_nanos();
        let dir = std::env::temp_dir().join(format!("theuth-routes-lib-{n}-{nanos}"));
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
                    .absorb(session, None, &d, learned, cookie)
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
            hint: String::new(),
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
    async fn private_root_stays_in_memory_and_closes() {
        let (dir, routes) = tmp_routes();
        let session = SessionId::new();
        let scratch = dir.join("scratch");
        let root = Some(scratch.as_path());
        let grouped = group(classify_all(&[exchange(
            "https://api.example.com/v1/items?page=1",
            &[("authorization", "Bearer SECRET-TOKEN")],
        )]));
        for (d, learned) in grouped {
            routes
                .absorb(session, root, &d, learned, None)
                .await
                .expect("absorb");
        }
        routes.members.lock().await.insert(session, scratch.clone());
        let url = Url::parse("https://api.example.com/").expect("url");
        let site = routes.list(root, &url).await.expect("list");
        assert_eq!(site.routes.len(), 1);
        let id = site.routes[0].id.clone();
        routes
            .mark(root, "example.com", &id, false)
            .await
            .expect("mark");
        assert!(routes.list(root, &url).await.expect("list").routes[0].stale);
        // Never on disk, never visible to a durable session.
        assert!(!dir.join("routes").exists());
        assert!(
            routes
                .list(None, &url)
                .await
                .expect("list")
                .routes
                .is_empty()
        );

        routes.close(&[session]).await;
        assert!(
            routes
                .list(root, &url)
                .await
                .expect("list")
                .routes
                .is_empty()
        );
        assert!(routes.creds.lock().await.is_empty());
        assert!(routes.members.lock().await.is_empty());
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

#[cfg(test)]
mod hint_tests {
    use super::*;

    fn ex(resource_type: &str, content_type: &str, body: Option<&str>) -> Exchange {
        Exchange {
            url: "https://x.io/p".to_owned(),
            method: "GET".to_owned(),
            resource_type: resource_type.to_owned(),
            status: 200,
            request_headers: HashMap::new(),
            response_headers: HashMap::new(),
            content_type: Some(content_type.to_owned()),
            body: body.map(str::to_owned),
            body_len: body.map_or(0, str::len),
            truncated: false,
            ts: 0,
        }
    }

    #[test]
    fn hint_separates_the_empty_cases() {
        assert!(hint(&[], false, &[], "", 0).starts_with("no traffic captured"));
        assert!(hint(&[], false, &[], "x.io", 2).starts_with("nothing new captured; 2 stored"));
        let nav = [
            ex("Document", "text/html", Some("<html>")),
            ex("Fetch", "text/plain", None),
        ];
        assert!(hint(&nav, false, &[], "", 0).contains("HTML page + beacons only"));
        let json = [ex("Xhr", "application/json", Some("{\"ok\":true}"))];
        let seen = ["https://x.io/p".to_owned()];
        assert_eq!(
            hint(&json, false, &seen, "", 0),
            "1 requests, no record collections; seen: https://x.io/p"
        );
        assert_eq!(
            hint(&json, true, &[], "x.io", 1),
            "1 routes for x.io; call one with site_call"
        );
    }
}
