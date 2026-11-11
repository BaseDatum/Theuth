//! The `web` actor (design doc 16 §3.1), registered as
//! [`crate::provider::WEB_ACTOR`]: owns the provider set in registration
//! order and answers `Search` / `Read` / `Render` off the mailbox, so a slow
//! provider never blocks a `RegisterProvider` from `search-providers`.

use std::path::PathBuf;
use std::sync::{Arc, RwLock};
use std::time::{Duration, Instant};

use kameo::message::{Context, Message};
use kameo::reply::{DelegatedReply, ReplySender};
use tools::{Activity, Tracked};

use crate::aggregate::{self, Drift, SearchResponse, is_external};
use crate::health::{Health, ProviderStatus};
use crate::provider::{
    Category, Provider, ProviderMeta, Query, RegisterProvider, Rendered, UnregisterProvider,
};
use crate::settings::{GetDefaults, Probe, ProviderProbe, SearchDefaults, SetDefaults, Status};
use crate::{SharedConfig, WebConfig};

/// Live provider metadata in registration order, shared with the tools so
/// `Tool::description` (sync) reflects the current set without a mailbox hop.
pub type ProviderMetas = Arc<RwLock<Vec<ProviderMeta>>>;

/// Error text when `Read` / `Render` finds no rendering provider (16 §5).
pub const NO_RENDERER: &str =
    "rendered fetch needs a rendering provider: scrape, browser, or firecrawl";

/// Error text of a rendered page asked for outside the local-network grant (16 §5, amended
/// 2026-11-11): a browser (or a self-hosted renderer) resolves and redirects on its own, so a
/// contact's turn gets none.
pub const CONTACT_NO_RENDER: &str = "rendered pages aren't available in a contact's turn";

/// The provider registry actor, registered as [`crate::provider::WEB_ACTOR`].
#[derive(kameo::Actor)]
pub struct WebActor {
    cfg: SharedConfig,
    /// `config.yaml` for [`SetDefaults`]; `None` in library use (apply only).
    config_path: Option<PathBuf>,
    client: reqwest::Client,
    /// Registration order; re-registering a name replaces in place.
    providers: Vec<Arc<dyn Provider>>,
    metas: ProviderMetas,
    drift: Arc<Drift>,
    health: Arc<Health>,
}

impl WebActor {
    /// Build with the built-in engines; `metas` is filled from `providers`.
    #[must_use]
    pub fn new(
        cfg: SharedConfig,
        config_path: Option<PathBuf>,
        client: reqwest::Client,
        providers: Vec<Arc<dyn Provider>>,
        metas: ProviderMetas,
    ) -> Self {
        let a = Self {
            cfg,
            config_path,
            client,
            providers,
            metas,
            drift: Arc::default(),
            health: Arc::default(),
        };
        a.publish();
        a
    }

    /// Library-use constructor: defaults, no config file.
    #[must_use]
    pub fn with_config(
        cfg: WebConfig,
        client: reqwest::Client,
        providers: Vec<Arc<dyn Provider>>,
        metas: ProviderMetas,
    ) -> Self {
        Self::new(Arc::new(RwLock::new(cfg)), None, client, providers, metas)
    }

    fn config(&self) -> WebConfig {
        self.cfg.read().map(|c| c.clone()).unwrap_or_default()
    }

    fn publish(&self) {
        if let Ok(mut m) = self.metas.write() {
            *m = self.providers.iter().map(|p| p.meta()).collect();
        }
    }

    /// The provider `Read` / `Render` go through: `scrape` when registered,
    /// else the first that renders.
    fn renderer(&self) -> Option<Arc<dyn Provider>> {
        self.providers
            .iter()
            .find(|p| p.meta().renders && p.meta().name == "scrape")
            .or_else(|| self.providers.iter().find(|p| p.meta().renders))
            .cloned()
    }

    /// [`Search`] off the mailbox, under `activity` when the caller carried one.
    fn search(
        &self,
        m: Search,
        activity: Option<Activity>,
        tx: ReplySender<Result<SearchResponse, String>>,
    ) {
        let providers = self.providers.clone();
        let client = self.client.clone();
        let cfg = self.config();
        let drift = Arc::clone(&self.drift);
        let health = Arc::clone(&self.health);
        spawn_under(activity, async move {
            tx.send(
                aggregate::search(
                    &providers,
                    &client,
                    &m.query,
                    m.providers.as_deref(),
                    &cfg,
                    &drift,
                    &health,
                )
                .await,
            );
        });
    }

    /// [`Read`] (`read: true`) or [`Render`] through [`Self::renderer`] off
    /// the mailbox, under `activity` when the caller carried one.
    fn page(
        &self,
        url: url::Url,
        read: bool,
        lan: bool,
        activity: Option<Activity>,
        tx: ReplySender<Result<Rendered, String>>,
    ) {
        let Some(p) = self.renderer() else {
            tx.send(Err(NO_RENDERER.to_owned()));
            return;
        };
        // 16 §5 (2026-11-11): without the grant only `scrape`'s read runs (its fetch tier keeps
        // to the public lane, its browser rung refuses); a render, any other provider's read
        // (a browser or a remote renderer), never.
        if !lan && !(read && p.meta().name == "scrape") {
            tx.send(Err(CONTACT_NO_RENDER.to_owned()));
            return;
        }
        let client = self.client.clone();
        // The grant rides into the detached work explicitly, like the activity.
        spawn_under(
            activity,
            tools::egress::scope(lan, async move {
                let r = if read {
                    p.read(&client, &url).await
                } else {
                    p.render(&client, &url).await
                };
                tx.send(r.map_err(|e| format!("{}: {e}", p.meta().name)));
            }),
        );
    }
}

/// Detach `fut`, scoped to `activity` so the operation stays admitted until
/// the provider work settles even after the asking caller timed out and
/// dropped its `ask` (47 §4.2 rule 5). Provider code reads
/// [`tools::activity::current`] inside and hands it on to the browser.
fn spawn_under<F>(activity: Option<Activity>, fut: F)
where
    F: Future<Output = ()> + Send + 'static,
{
    match activity {
        Some(a) => {
            tokio::spawn(a.scope(fut));
        }
        None => {
            tokio::spawn(fut);
        }
    }
}

/// `ask` → `Vec<ProviderMeta>` in registration order.
#[derive(Clone, Copy, Debug, Default)]
pub struct ListProviders;

/// `ask` → `Result<SearchResponse, String>`; `Err` only for a bad provider
/// selection. Runs off the mailbox. `Search`, [`Render`] and [`Read`] are
/// also accepted wrapped in [`Tracked`]: the detached work then runs under
/// the caller's activity, so a tool invocation stays admitted until the
/// provider (and any browser command it issues) actually settles, even when
/// the caller times out and drops its `ask`. Tool paths send the tracked form;
/// the plain form is the uncertified one.
#[derive(Clone, Debug)]
pub struct Search {
    /// The query.
    pub query: Query,
    /// Subset of provider names; `None` = every provider serving the category.
    pub providers: Option<Vec<String>>,
}

/// `ask` → `Result<Rendered, String>` through the browser of the `scrape`
/// provider, else the first whose `meta().renders`; [`NO_RENDERER`] when
/// none. Always a browser render (`read_url {rendered: true}`). Runs off the
/// mailbox, under `lan`; [`CONTACT_NO_RENDER`] without it.
#[derive(Clone, Debug)]
pub struct Render {
    /// The page.
    pub url: url::Url,
    /// The caller's `tools::egress::lan()`, read in the caller's task.
    pub lan: bool,
}

/// `ask` → `Result<Rendered, String>`: the page's real content the cheapest
/// way the same provider `Render` would pick offers ([`Provider::read`]:
/// scrape fetches first, browser only when the fetch is thin). What
/// `search_web` reads hits with. Runs off the mailbox, under `lan`.
#[derive(Clone, Debug)]
pub struct Read {
    /// The page.
    pub url: url::Url,
    /// The caller's `tools::egress::lan()`, read in the caller's task.
    pub lan: bool,
}

impl Message<RegisterProvider> for WebActor {
    type Reply = ();

    async fn handle(&mut self, m: RegisterProvider, _: &mut Context<Self, Self::Reply>) {
        let name = m.0.meta().name;
        match self.providers.iter().position(|p| p.meta().name == name) {
            Some(i) => self.providers[i] = m.0,
            None => self.providers.push(m.0),
        }
        tracing::info!(provider = %name, "registered search provider");
        self.publish();
    }
}

impl Message<UnregisterProvider> for WebActor {
    type Reply = ();

    async fn handle(&mut self, m: UnregisterProvider, _: &mut Context<Self, Self::Reply>) {
        self.providers.retain(|p| p.meta().name != m.0);
        self.publish();
    }
}

impl Message<ListProviders> for WebActor {
    type Reply = Vec<ProviderMeta>;

    async fn handle(
        &mut self,
        _: ListProviders,
        _: &mut Context<Self, Self::Reply>,
    ) -> Self::Reply {
        self.providers.iter().map(|p| p.meta()).collect()
    }
}

impl Message<Search> for WebActor {
    type Reply = DelegatedReply<Result<SearchResponse, String>>;

    async fn handle(&mut self, m: Search, ctx: &mut Context<Self, Self::Reply>) -> Self::Reply {
        let (reply, tx) = ctx.reply_sender();
        if let Some(tx) = tx {
            self.search(m, None, tx);
        }
        reply
    }
}

impl Message<Tracked<Search>> for WebActor {
    type Reply = DelegatedReply<Result<SearchResponse, String>>;

    async fn handle(
        &mut self,
        m: Tracked<Search>,
        ctx: &mut Context<Self, Self::Reply>,
    ) -> Self::Reply {
        let (reply, tx) = ctx.reply_sender();
        if let Some(tx) = tx {
            self.search(m.message, m.activity, tx);
        }
        reply
    }
}

impl Message<Render> for WebActor {
    type Reply = DelegatedReply<Result<Rendered, String>>;

    async fn handle(
        &mut self,
        Render { url, lan }: Render,
        ctx: &mut Context<Self, Self::Reply>,
    ) -> Self::Reply {
        let (reply, tx) = ctx.reply_sender();
        if let Some(tx) = tx {
            self.page(url, false, lan, None, tx);
        }
        reply
    }
}

impl Message<Tracked<Render>> for WebActor {
    type Reply = DelegatedReply<Result<Rendered, String>>;

    async fn handle(
        &mut self,
        m: Tracked<Render>,
        ctx: &mut Context<Self, Self::Reply>,
    ) -> Self::Reply {
        let (reply, tx) = ctx.reply_sender();
        if let Some(tx) = tx {
            self.page(m.message.url, false, m.message.lan, m.activity, tx);
        }
        reply
    }
}

impl Message<Read> for WebActor {
    type Reply = DelegatedReply<Result<Rendered, String>>;

    async fn handle(
        &mut self,
        Read { url, lan }: Read,
        ctx: &mut Context<Self, Self::Reply>,
    ) -> Self::Reply {
        let (reply, tx) = ctx.reply_sender();
        if let Some(tx) = tx {
            self.page(url, true, lan, None, tx);
        }
        reply
    }
}

impl Message<Tracked<Read>> for WebActor {
    type Reply = DelegatedReply<Result<Rendered, String>>;

    async fn handle(
        &mut self,
        m: Tracked<Read>,
        ctx: &mut Context<Self, Self::Reply>,
    ) -> Self::Reply {
        let (reply, tx) = ctx.reply_sender();
        if let Some(tx) = tx {
            self.page(m.message.url, true, m.message.lan, m.activity, tx);
        }
        reply
    }
}

impl Message<GetDefaults> for WebActor {
    type Reply = SearchDefaults;

    async fn handle(&mut self, _: GetDefaults, _: &mut Context<Self, Self::Reply>) -> Self::Reply {
        SearchDefaults::from(&self.config())
    }
}

impl Message<SetDefaults> for WebActor {
    type Reply = Result<SearchDefaults, String>;

    async fn handle(&mut self, m: SetDefaults, _: &mut Context<Self, Self::Reply>) -> Self::Reply {
        m.0.validate()?;
        if let Some(path) = &self.config_path {
            for (key, value) in m.0.yaml_pairs() {
                runtime::write_config_value(path, "web", key, &value)
                    .map_err(|e| format!("config.yaml: {e}"))?;
            }
        }
        let mut cfg = self
            .cfg
            .write()
            .map_err(|_| "config lock poisoned".to_owned())?;
        m.0.apply(&mut cfg);
        tracing::info!(defaults = ?m.0, "search defaults updated");
        Ok(m.0)
    }
}

impl Message<Status> for WebActor {
    type Reply = Vec<ProviderStatus>;

    async fn handle(&mut self, _: Status, _: &mut Context<Self, Self::Reply>) -> Self::Reply {
        self.providers
            .iter()
            .map(|p| {
                let meta = p.meta();
                let mut s = self.health.get(meta.name).unwrap_or_default();
                s.name = meta.name.to_owned();
                s.external = is_external(meta.name);
                s.searches = !meta.categories.is_empty();
                s
            })
            .collect()
    }
}

impl Message<Probe> for WebActor {
    type Reply = DelegatedReply<Result<ProviderProbe, String>>;

    async fn handle(&mut self, m: Probe, ctx: &mut Context<Self, Self::Reply>) -> Self::Reply {
        let (reply, tx) = ctx.reply_sender();
        let Some(tx) = tx else {
            return reply;
        };
        let Some(p) = self
            .providers
            .iter()
            .find(|p| p.meta().name == m.name)
            .cloned()
        else {
            let known: Vec<&str> = self.providers.iter().map(|p| p.meta().name).collect();
            tx.send(Err(format!(
                "provider {} is not registered (enabled with a key?); registered: {}",
                m.name,
                known.join(", ")
            )));
            return reply;
        };
        let client = self.client.clone();
        let cfg = self.config();
        let health = Arc::clone(&self.health);
        tokio::spawn(async move {
            let name = p.meta().name.to_owned();
            let category = p
                .meta()
                .categories
                .first()
                .copied()
                .unwrap_or(Category::General);
            let q = Query {
                text: m.query,
                category,
                limit: 5,
                site: None,
                recent: None,
            };
            let start = Instant::now();
            let r = tokio::time::timeout(
                p.timeout()
                    .unwrap_or(Duration::from_millis(cfg.timeout_ms.max(1))),
                p.search(&client, &q),
            )
            .await
            .unwrap_or(Err(crate::provider::ProviderError::Timeout));
            let took_ms = u64::try_from(start.elapsed().as_millis()).unwrap_or(u64::MAX);
            let mut outcome = aggregate::FanoutOutcome::default();
            let probe = match r {
                Ok((hits, _)) => {
                    let detail = hits.first().map(|h| h.title.clone()).unwrap_or_default();
                    outcome.ok.push(name.clone());
                    outcome.hits.push((name.clone(), hits.clone()));
                    ProviderProbe {
                        name,
                        ok: true,
                        took_ms,
                        rows: hits.len(),
                        detail,
                    }
                }
                Err(e) => {
                    outcome.failed.push((name.clone(), e.to_string()));
                    ProviderProbe {
                        name,
                        ok: false,
                        took_ms,
                        rows: 0,
                        detail: e.to_string(),
                    }
                }
            };
            health.observe(&outcome);
            tx.send(Ok(probe));
        });
        reply
    }
}

#[cfg(test)]
mod tests {
    use std::sync::OnceLock;
    use std::time::Duration;

    use kameo::actor::Spawn as _;
    use runtime::SessionId;
    use tokio::sync::Notify;
    use tools::activity::{self, Kind, OperationId};

    use super::*;
    use crate::provider::{Feature, ProviderError, RawResult};

    /// Renderer that records the activity it runs under and then blocks
    /// until released, so the asking side can time out first.
    struct Held {
        release: Arc<Notify>,
        finished: Arc<Notify>,
        ran_under: Arc<OnceLock<Option<OperationId>>>,
    }

    #[async_trait::async_trait]
    impl Provider for Held {
        fn meta(&self) -> ProviderMeta {
            ProviderMeta {
                name: "held",
                categories: &[],
                features: &[],
                renders: true,
                billable: false,
            }
        }

        async fn search(
            &self,
            _: &reqwest::Client,
            _: &Query,
        ) -> Result<(Vec<RawResult>, Vec<Feature>), ProviderError> {
            Err(ProviderError::Unsupported)
        }

        async fn render(
            &self,
            _: &reqwest::Client,
            url: &url::Url,
        ) -> Result<Rendered, ProviderError> {
            let _ = self.ran_under.set(activity::current().map(|a| a.id()));
            self.release.notified().await;
            self.finished.notify_one();
            Ok(Rendered {
                url: url.to_string(),
                ..Rendered::default()
            })
        }
    }

    /// 47 §4.2 rule 5: the provider runs under the caller's activity, and a
    /// caller that times out and drops its `ask` leaves the detached render
    /// (and so the guard) running to completion.
    #[tokio::test]
    async fn tracked_read_runs_provider_under_caller_activity_past_timeout() {
        let release = Arc::new(Notify::new());
        let finished = Arc::new(Notify::new());
        let ran_under = Arc::new(OnceLock::new());
        let held: Arc<dyn Provider> = Arc::new(Held {
            release: Arc::clone(&release),
            finished: Arc::clone(&finished),
            ran_under: Arc::clone(&ran_under),
        });
        let actor = WebActor::spawn(WebActor::with_config(
            WebConfig::default(),
            reqwest::Client::new(),
            vec![held],
            ProviderMetas::default(),
        ));
        let owner = Activity::untracked(SessionId::new(), Kind::Command("test"));
        let url: url::Url = "https://example.com/".parse().unwrap();

        let asked = tokio::time::timeout(
            Duration::from_millis(50),
            actor.ask(Tracked {
                message: Read { url, lan: true },
                activity: Some(owner.clone()),
            }),
        )
        .await;
        assert!(
            asked.is_err(),
            "held renderer must outlive the caller's deadline"
        );

        release.notify_one();
        tokio::time::timeout(Duration::from_secs(2), finished.notified())
            .await
            .expect("detached render finishes after the caller dropped its ask");
        assert_eq!(ran_under.get(), Some(&Some(owner.id())));
    }
}
