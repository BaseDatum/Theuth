//! `site_learn`, `site_routes`, `site_call` (design doc 44 §4). Risk
//! `ReadOnly`; http(s) only. Rows are [`Route`]s: header names, never values.

use std::path::Path;
use std::sync::Arc;

use kameo::actor::ActorRef;
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use serde_json::{Map, Value};
use tools::registry::ToolRegistry;
use tools::{RegisterPrompt, RegisterTool, Risk, Tool, ToolCx, ToolOutput};
use url::Url;

use crate::{CallReply, Route, Routes};

/// Plugin prompt block (44 §4): when to learn routes instead of scraping.
pub const PROMPT: &str = "site_learn, site_routes, site_call are runtime functions. \
Listing pages (search, tables, feeds, dashboards) often render the first page as HTML: \
site_learn(url=…) opens the page in your browser session and returns whatever JSON routes it called, \
as URL templates with {holes}; if routes is empty, click one control that changes the list in place \
(a filter, sort, search or next page) with browser_click, then site_learn() with no url — \
the capture buffer is drained on every read, so learn right after the click. \
Then site_call(route=…, params={…}) returns records directly, no page, no rendering, for any hole value; \
optional holes are sent only when you pass them. \
site_routes(url) lists what is already learned for a site. \
Prefer this over scrape_url {rendered: true} when you need data, not prose; \
a route that says drifted needs site_learn again.";

/// Register the three tools and the prompt block.
pub async fn register(reg: &ActorRef<ToolRegistry>, routes: Arc<Routes>) {
    let _ = reg
        .ask(RegisterPrompt {
            plugin: "routes",
            text: PROMPT.to_owned(),
        })
        .await;
    let _ = reg
        .ask(RegisterTool(Arc::new(SiteLearnTool(Arc::clone(&routes)))))
        .await;
    let _ = reg
        .ask(RegisterTool(Arc::new(SiteRoutesTool(Arc::clone(&routes)))))
        .await;
    let _ = reg.ask(RegisterTool(Arc::new(SiteCallTool(routes)))).await;
}

fn structured<T: Serialize>(text: String, data: T) -> ToolOutput {
    ToolOutput {
        structured: serde_json::to_value(data).ok(),
        ..ToolOutput::ok(text)
    }
}

fn parse_url(s: &str) -> anyhow::Result<Url> {
    let u = Url::parse(s).map_err(|e| anyhow::anyhow!("invalid url: {e}"))?;
    if u.scheme() != "http" && u.scheme() != "https" {
        anyhow::bail!("only http(s) URLs");
    }
    Ok(u)
}

/// 51 §5: the scratch root a private session's routes live under; `None`
/// for a durable session (the disk store).
fn private_root(cx: &ToolCx) -> Option<&Path> {
    cx.resources
        .is_private()
        .then(|| cx.resources.root.as_path())
}

fn auth(names: &[String]) -> String {
    if names.is_empty() {
        "none".to_owned()
    } else {
        names.join(", ")
    }
}

/// One text row: `GET https://x/{id} → 214 records at /versions (auth: none) [stale]`.
fn row(r: &Route) -> String {
    let at = if r.collection.is_empty() {
        "/"
    } else {
        r.collection.as_str()
    };
    let mut s = format!(
        "{} {} → {} records at {} (auth: {})",
        r.method,
        r.template,
        r.records_seen,
        at,
        auth(&r.auth_headers)
    );
    if r.stale {
        s.push_str(" [stale]");
    }
    if !r.replay {
        s.push_str(" [no replay]");
    }
    s
}

fn rows(domain: &str, routes: &[Route]) -> String {
    let mut s = format!("{domain}: {} routes", routes.len());
    for r in routes {
        s.push('\n');
        s.push_str(&row(r));
    }
    s
}

/// Arguments for [`SiteLearnTool`].
#[derive(Debug, Serialize, Deserialize, JsonSchema)]
pub struct SiteLearnArgs {
    /// Page to open in this session's browser first; omit to read what the
    /// session's current page has fetched so far.
    pub url: Option<String>,
}

/// `site_learn`: capture the session's XHR/fetch traffic, classify, merge.
pub struct SiteLearnTool(pub Arc<Routes>);

#[async_trait::async_trait]
impl Tool for SiteLearnTool {
    const NAME: &'static str = "site_learn";
    const RETURNS: &'static str = "dict";
    const RISK: Risk = Risk::ReadOnly;
    const DIRECT: bool = false;
    type Args = SiteLearnArgs;

    fn description(&self) -> String {
        "Learn the JSON routes a page's own JavaScript calls. First load is often HTML: if routes is empty, click a control that changes the list, then call again with no url (the capture buffer is drained on every read). With url: opens it in this session's browser and waits for the network to settle. Returns {domain, routes, captured, top_urls, hint}: routes are URL templates with {holes}, record keys and a collection pointer; hint says what was captured and what to do next. Then use site_call.".into()
    }

    async fn call(&self, a: SiteLearnArgs, cx: ToolCx) -> anyhow::Result<ToolOutput> {
        let url = a.url.as_deref().map(parse_url).transpose()?;
        let reply = match self.0.learn(cx.session, private_root(&cx), url.as_ref()).await {
            Ok(r) => r,
            Err(e) => return Ok(ToolOutput::err(e.to_string())),
        };
        let mut text = reply.hint.clone();
        if !reply.routes.is_empty() {
            text.push('\n');
            text.push_str(&rows(&reply.domain, &reply.routes));
        }
        Ok(structured(text, &reply))
    }
}

/// Arguments for [`SiteRoutesTool`].
#[derive(Debug, Serialize, Deserialize, JsonSchema)]
pub struct SiteRoutesArgs {
    /// Any URL on the site; routes are stored per registrable domain.
    pub url: String,
}

/// `site_routes`: stored rows for the URL's registrable domain.
pub struct SiteRoutesTool(pub Arc<Routes>);

#[async_trait::async_trait]
impl Tool for SiteRoutesTool {
    const NAME: &'static str = "site_routes";
    const RETURNS: &'static str = "list[dict]";
    const RISK: Risk = Risk::ReadOnly;
    const DIRECT: bool = false;
    type Args = SiteRoutesArgs;

    fn description(&self) -> String {
        "Routes already learned for the URL's site, ranked; call one with site_call. Empty means site_learn first.".into()
    }

    async fn call(&self, a: SiteRoutesArgs, cx: ToolCx) -> anyhow::Result<ToolOutput> {
        let url = parse_url(&a.url)?;
        let site = match self.0.list(private_root(&cx), &url).await {
            Ok(s) => s,
            Err(e) => return Ok(ToolOutput::err(e.to_string())),
        };
        let text = if site.routes.is_empty() {
            format!("no routes learned for {}; site_learn first", site.domain)
        } else {
            rows(&site.domain, &site.routes)
        };
        Ok(structured(text, site.routes))
    }
}

/// Arguments for [`SiteCallTool`].
#[derive(Debug, Serialize, Deserialize, JsonSchema)]
pub struct SiteCallArgs {
    /// Route id from site_learn/site_routes (`domain:METHOD:/path`).
    pub route: String,
    /// Hole values by name; path holes required, query holes default to the learned example, holes marked optional are omitted unless passed.
    pub params: Option<Map<String, Value>>,
}

/// `site_call`: replay a learned route over plain HTTP.
pub struct SiteCallTool(pub Arc<Routes>);

#[async_trait::async_trait]
impl Tool for SiteCallTool {
    const NAME: &'static str = "site_call";
    const RETURNS: &'static str = "list[dict]";
    const RISK: Risk = Risk::ReadOnly;
    const DIRECT: bool = false;
    type Args = SiteCallArgs;

    fn description(&self) -> String {
        "Call a learned route over plain HTTP, no browser: route is the id from site_learn/site_routes, params fills its {holes} (path holes required, query holes default to the learned example, holes marked optional are omitted unless passed). Returns the records at the route's collection pointer; a 'drifted' error means site_learn again.".into()
    }

    async fn call(&self, a: SiteCallArgs, cx: ToolCx) -> anyhow::Result<ToolOutput> {
        let params = a.params.unwrap_or_default();
        let CallReply {
            url,
            status,
            count,
            pointer,
            records,
            unarmed,
        } = match self
            .0
            .call(cx.session, private_root(&cx), &a.route, &params)
            .await
        {
            Ok(r) => r,
            Err(e) => return Ok(ToolOutput::err(e.to_string())),
        };
        let method = a.route.split(':').nth(1).unwrap_or("GET");
        let at = if pointer.is_empty() {
            "/"
        } else {
            pointer.as_str()
        };
        let armed: Vec<String> = self
            .0
            .list(private_root(&cx), &parse_url(&url)?)
            .await
            .ok()
            .and_then(|s| s.routes.into_iter().find(|r| r.id == a.route))
            .map(|r| r.auth_headers)
            .unwrap_or_default();
        let mut text = format!(
            "{method} {url} → {status}, {count} records at {at} (auth: {})",
            auth(&armed)
        );
        if !unarmed.is_empty() {
            text.push_str(&format!(
                "; sent without {}: open the site in this session and site_learn again to arm them",
                unarmed.join(", ")
            ));
        }
        Ok(structured(text, records))
    }
}

const _: () = {
    assert!(!<SiteLearnTool as Tool>::DIRECT);
    assert!(!<SiteRoutesTool as Tool>::DIRECT);
    assert!(!<SiteCallTool as Tool>::DIRECT);
};

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn row_marks_auth_stale_and_replay() {
        let r: Route = serde_json::from_value(serde_json::json!({
            "id": "crates.io:GET:/api/v1/crates/{id}",
            "name": "crates", "method": "GET",
            "template": "https://crates.io/api/v1/crates/{id}",
            "holes": [], "collection": "/versions", "keys": {},
            "records_seen": 214, "auth_headers": ["authorization"],
            "cookies": true, "replay": false, "seen": 1, "last_ok": null,
            "stale": true, "example_url": "https://crates.io/api/v1/crates/tokio"
        }))
        .expect("route");
        assert_eq!(
            row(&r),
            "GET https://crates.io/api/v1/crates/{id} → 214 records at /versions (auth: authorization) [stale] [no replay]"
        );
        assert!(rows("crates.io", &[r]).starts_with("crates.io: 1 routes\nGET "));
    }
}
