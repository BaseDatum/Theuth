//! Local `web::Provider` with `renders: true` so `read_url {rendered: true}`
//! and `search_web`'s top-hit reads resolve in-process (38 §5).

use std::sync::Arc;

use runtime::Runtime;
use web::{Provider, ProviderError, ProviderMeta, Query, Rendered, WEB_ACTOR};

use crate::engine::Scrape;

/// `web::Provider { name: "scrape", renders: true }`.
pub struct ScrapeProvider {
    scrape: Arc<Scrape>,
}

impl ScrapeProvider {
    /// Wrap the shared scrape handle.
    #[must_use]
    pub fn new(scrape: Arc<Scrape>) -> Self {
        Self { scrape }
    }
}

#[async_trait::async_trait]
impl Provider for ScrapeProvider {
    fn meta(&self) -> ProviderMeta {
        ProviderMeta {
            name: "scrape",
            categories: &[],
            features: &[],
            renders: true,
            billable: false,
        }
    }

    async fn search(
        &self,
        _client: &reqwest::Client,
        _q: &Query,
    ) -> Result<(Vec<web::RawResult>, Vec<web::Feature>), ProviderError> {
        Err(ProviderError::Unsupported)
    }

    async fn render(
        &self,
        _client: &reqwest::Client,
        url: &url::Url,
    ) -> Result<Rendered, ProviderError> {
        let page = self
            .scrape
            .fetch_page(url, false, true)
            .await
            .map_err(|e| ProviderError::Http(e.to_string()))?;
        Ok(Rendered {
            url: page.url,
            title: page.title,
            markdown: page.markdown,
        })
    }
}

/// Register [`ScrapeProvider`] with the `web` actor once it is up.
pub async fn register(rt: Runtime, scrape: Arc<Scrape>) {
    let Some(web) = rt.lookup_when::<web::WebActor>(WEB_ACTOR).await else {
        tracing::warn!("scrape: web plugin not running; render provider not registered");
        return;
    };
    match web
        .ask(web::RegisterProvider(Arc::new(ScrapeProvider::new(scrape))))
        .await
    {
        Ok(()) => tracing::info!("scrape: registered as rendering provider with web"),
        Err(e) => tracing::warn!("scrape: registering render provider failed: {e}"),
    }
}
