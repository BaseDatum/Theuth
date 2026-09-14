//! `read_url rendered: true` without Firecrawl (23 §0.9, §8): a
//! [`web::Provider`] that fetches a page through the embedded browser and
//! registers itself with the `web` actor over the bus (16 §3.1). It owns no
//! isolate; every step is a [`Call`] on one fixed private session.

use std::sync::{Arc, LazyLock};

use kameo::actor::ActorRef;
use runtime::{Runtime, SessionId};
use serde_json::{Value, json};
use web::{Provider, ProviderError, ProviderMeta, Query, Rendered};

use crate::{Browsers, Call};

/// The render provider's own isolate (23 §8): fixed so it is never confused
/// with a conversation and survives across `read_url` calls until idle.
static RENDER_SESSION: LazyLock<SessionId> = LazyLock::new(|| {
    SessionId(uuid::Uuid::from_u128(
        0x7768_5f62_726f_7773_6572_5f72_656e_6465,
    ))
});

/// `web::Provider { renders: true }` backed by the `Browsers` actor.
pub struct ObscuraRender {
    actor: ActorRef<Browsers>,
    // ponytail: one isolate, one page at a time; a session pool if concurrent
    // rendered fetches ever matter.
    lock: tokio::sync::Mutex<()>,
}

impl ObscuraRender {
    /// Wrap the browser actor.
    pub fn new(actor: ActorRef<Browsers>) -> Self {
        Self {
            actor,
            lock: tokio::sync::Mutex::new(()),
        }
    }

    /// One obscura tool call on the render session, reduced to its text
    /// block. `isError` and an unavailable browser both surface as
    /// [`ProviderError::Http`]: the aggregator reports the text verbatim.
    async fn call(&self, name: &str, args: Value) -> Result<String, ProviderError> {
        let result = self
            .actor
            .ask(Call {
                session: *RENDER_SESSION,
                name: name.to_owned(),
                args,
            })
            .await
            .map_err(|e| ProviderError::Http(e.to_string()))?;
        let text = result["content"]
            .as_array()
            .into_iter()
            .flatten()
            .filter_map(|c| c["text"].as_str())
            .collect::<Vec<_>>()
            .join("\n");
        if result["isError"].as_bool() == Some(true) {
            return Err(ProviderError::Http(
                text.strip_prefix("Error: ").unwrap_or(&text).to_owned(),
            ));
        }
        Ok(text)
    }
}

/// Split obscura's `browser_navigate` reply, `Navigated to <url> — "<title>"`,
/// into `(url, title)`; `None` when it has another shape.
fn parse_navigated(reply: &str) -> Option<(&str, Option<&str>)> {
    let rest = reply.trim().strip_prefix("Navigated to ")?;
    match rest.rsplit_once(" — \"") {
        Some((url, title)) => {
            let title = title.strip_suffix('"').unwrap_or(title);
            Some((url, (!title.is_empty()).then_some(title)))
        }
        None => Some((rest, None)),
    }
}

#[async_trait::async_trait]
impl Provider for ObscuraRender {
    fn meta(&self) -> ProviderMeta {
        ProviderMeta {
            name: "browser",
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

    /// `browser_navigate` → `browser_markdown` → `browser_close` (23 §8).
    async fn render(
        &self,
        _client: &reqwest::Client,
        url: &url::Url,
    ) -> Result<Rendered, ProviderError> {
        let _page = self.lock.lock().await;
        let navigated = self
            .call("browser_navigate", json!({ "url": url.as_str() }))
            .await?;
        let markdown = self
            .call("browser_markdown", json!({ "max_chars": 1_000_000 }))
            .await;
        // Close even when markdown failed; its own failure is not the page's.
        let _ = self.call("browser_close", json!({})).await;
        let markdown = markdown?;
        let (final_url, title) = parse_navigated(&navigated).unwrap_or((url.as_str(), None));
        Ok(Rendered {
            url: final_url.to_owned(),
            title: title.map(str::to_owned),
            markdown,
        })
    }
}

/// Register [`ObscuraRender`] with the `web` actor once it is up (16 §3.1).
/// Without `web` there is nothing to render for; log and return.
pub async fn register(rt: Runtime, actor: ActorRef<Browsers>) {
    let Some(web) = rt.lookup_when::<web::WebActor>(web::WEB_ACTOR).await else {
        tracing::warn!("browser: web plugin not running; render provider not registered");
        return;
    };
    match web
        .ask(web::RegisterProvider(Arc::new(ObscuraRender::new(actor))))
        .await
    {
        Ok(()) => tracing::info!("browser: registered as rendering provider with web"),
        Err(e) => tracing::warn!("browser: registering render provider failed: {e}"),
    }
}

#[cfg(test)]
mod tests {
    use super::parse_navigated;

    #[test]
    fn navigate_reply_yields_url_and_title() {
        assert_eq!(
            parse_navigated("Navigated to https://example.com/ — \"Example Domain\""),
            Some(("https://example.com/", Some("Example Domain")))
        );
        assert_eq!(
            parse_navigated("Navigated to https://example.com/ — \"\""),
            Some(("https://example.com/", None))
        );
        assert_eq!(
            parse_navigated("Navigated to https://example.com/x"),
            Some(("https://example.com/x", None))
        );
        assert_eq!(parse_navigated("Error: no page"), None);
    }
}
