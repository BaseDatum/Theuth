//! The `web` actor (design doc 16 §3.1), registered as
//! [`crate::provider::WEB_ACTOR`]: owns the provider set in registration
//! order and answers `Search` / `Render` off the mailbox, so a slow provider
//! never blocks a `RegisterProvider` from `search-providers`.

use std::sync::{Arc, RwLock};

use kameo::message::{Context, Message};
use kameo::reply::DelegatedReply;

use crate::WebConfig;
use crate::aggregate::{self, Drift, SearchResponse};
use crate::provider::{
    Provider, ProviderMeta, Query, RegisterProvider, Rendered, UnregisterProvider,
};

/// Live provider metadata in registration order, shared with the tools so
/// `Tool::description` (sync) reflects the current set without a mailbox hop.
pub type ProviderMetas = Arc<RwLock<Vec<ProviderMeta>>>;

/// Error text when `Render` finds no rendering provider (16 §5).
pub const NO_RENDERER: &str =
    "rendered fetch needs a rendering provider: scrape, browser, or firecrawl";

/// The provider registry actor, registered as [`crate::provider::WEB_ACTOR`].
#[derive(kameo::Actor)]
pub struct WebActor {
    cfg: WebConfig,
    client: reqwest::Client,
    /// Registration order; re-registering a name replaces in place.
    providers: Vec<Arc<dyn Provider>>,
    metas: ProviderMetas,
    drift: Arc<Drift>,
}

impl WebActor {
    /// Build with the built-in engines; `metas` is filled from `providers`.
    #[must_use]
    pub fn new(
        cfg: WebConfig,
        client: reqwest::Client,
        providers: Vec<Arc<dyn Provider>>,
        metas: ProviderMetas,
    ) -> Self {
        let a = Self {
            cfg,
            client,
            providers,
            metas,
            drift: Arc::default(),
        };
        a.publish();
        a
    }

    fn publish(&self) {
        if let Ok(mut m) = self.metas.write() {
            *m = self.providers.iter().map(|p| p.meta()).collect();
        }
    }
}

/// `ask` → `Vec<ProviderMeta>` in registration order.
#[derive(Clone, Copy, Debug, Default)]
pub struct ListProviders;

/// `ask` → `Result<SearchResponse, String>`; `Err` only for a bad provider
/// selection. Runs off the mailbox.
#[derive(Clone, Debug)]
pub struct Search {
    /// The query.
    pub query: Query,
    /// Subset of provider names; `None` = every provider serving the category.
    pub providers: Option<Vec<String>>,
}

/// `ask` → `Result<Rendered, String>` through the first provider whose
/// `meta().renders`; [`NO_RENDERER`] when none. Runs off the mailbox.
#[derive(Clone, Debug)]
pub struct Render(pub url::Url);

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
        let Some(tx) = tx else { return reply };
        let providers = self.providers.clone();
        let client = self.client.clone();
        let cfg = self.cfg.clone();
        let drift = Arc::clone(&self.drift);
        tokio::spawn(async move {
            tx.send(
                aggregate::search(
                    &providers,
                    &client,
                    &m.query,
                    m.providers.as_deref(),
                    &cfg,
                    &drift,
                )
                .await,
            );
        });
        reply
    }
}

impl Message<Render> for WebActor {
    type Reply = DelegatedReply<Result<Rendered, String>>;

    async fn handle(
        &mut self,
        Render(url): Render,
        ctx: &mut Context<Self, Self::Reply>,
    ) -> Self::Reply {
        let (reply, tx) = ctx.reply_sender();
        let Some(tx) = tx else { return reply };
        let Some(p) = self
            .providers
            .iter()
            .find(|p| p.meta().renders && p.meta().name == "scrape")
            .or_else(|| self.providers.iter().find(|p| p.meta().renders))
            .cloned()
        else {
            tx.send(Err(NO_RENDERER.to_owned()));
            return reply;
        };
        let client = self.client.clone();
        tokio::spawn(async move {
            tx.send(
                p.render(&client, &url)
                    .await
                    .map_err(|e| format!("{}: {e}", p.meta().name)),
            );
        });
        reply
    }
}
