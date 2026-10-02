//! `search_x` (design doc 80): X (Twitter) posts through xAI's Responses API
//! and its server-side `x_search` tool. One POST per call, Grok's synthesis
//! plus the cited posts back. The bearer is the `xai` provider's (05 §2.2):
//! vault key, then login (`oauth::XAI`), then `XAI_API_KEY`. Not a
//! [`web::Provider`]: the result is an answer with citations, not a hit list.

use std::sync::Arc;
use std::time::{Duration, Instant};

use chrono::{NaiveDate, Utc};
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use tools::{Risk, Tool, ToolCx, ToolOutput};

/// Production API root; the login token is minted for it and nothing else (80 §0.7).
pub const BASE_URL: &str = "https://api.x.ai/v1";
/// Vault key and oauth namespace of the xAI provider.
pub const NS: &str = "xai";
/// Environment variable the xAI adapter reads.
pub const ENV_KEY: &str = "XAI_API_KEY";
/// xAI's per-list handle maximum.
pub const MAX_HANDLES: usize = 20;
/// Body bytes quoted in an HTTP error.
const ERROR_BODY_CHARS: usize = 500;

/// System-prompt block registered with the tool (plugins.md §3.5.1).
pub const PROMPT: &str = "search_x searches X (Twitter) posts through xAI's live index: reach for it \
instead of search_web when the question is what people are saying on X, reactions to something, or a \
handle's recent posts.";

/// `search_providers.x` (80 §5). Present by default, inert without an `xai`
/// credential.
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(default)]
pub struct XConfig {
    /// Set `false` to keep the block but register nothing.
    pub enabled: bool,
    /// xAI model that runs the server-side tool; any model with agent tools.
    pub model: String,
    /// Deadline for one call; one Grok search can take tens of seconds.
    pub timeout_ms: u64,
}

impl Default for XConfig {
    fn default() -> Self {
        Self {
            enabled: true,
            model: "grok-4.7".into(),
            timeout_ms: 90_000,
        }
    }
}

/// Arguments for [`XSearchTool`].
#[derive(Debug, Deserialize, JsonSchema)]
pub struct XSearchArgs {
    /// What to find out on X, in natural language.
    pub query: String,
    /// Only posts from these handles (with or without `@`), at most 20. Not with `exclude_handles`.
    #[serde(default)]
    pub handles: Option<Vec<String>>,
    /// Never posts from these handles (with or without `@`), at most 20.
    #[serde(default)]
    pub exclude_handles: Option<Vec<String>>,
    /// Earliest post date, `YYYY-MM-DD` (UTC); not after today.
    #[serde(default)]
    pub from_date: Option<String>,
    /// Latest post date, `YYYY-MM-DD` (UTC); not before `from_date`.
    #[serde(default)]
    pub to_date: Option<String>,
    /// Also look at the images in posts (slower, costs more).
    #[serde(default)]
    pub images: Option<bool>,
    /// Also look at the videos in posts (slower, costs more).
    #[serde(default)]
    pub videos: Option<bool>,
}

/// Where the bearer came from (05 §2.2 order).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum Credential {
    /// Vault key `xai`.
    Vault,
    /// `/login xai` grant.
    Login,
    /// `XAI_API_KEY` in the process environment.
    Env,
}

/// 05 §2.2 precedence: a vault key wins; else a login's access token (its
/// error is final: the model should see "run /login xai"); else the env key.
pub fn credential(
    vault: Option<String>,
    login: Option<Result<String, String>>,
    env: Option<String>,
) -> Result<Option<(Credential, String)>, String> {
    if let Some(k) = vault {
        return Ok(Some((Credential::Vault, k)));
    }
    if let Some(l) = login {
        return l.map(|t| Some((Credential::Login, t)));
    }
    Ok(env.map(|k| (Credential::Env, k)))
}

/// `XAI_API_KEY`, trimmed; `None` when unset or blank.
#[must_use]
pub fn env_key() -> Option<String> {
    std::env::var(ENV_KEY)
        .ok()
        .map(|k| k.trim().to_owned())
        .filter(|k| !k.is_empty())
}

fn handles(list: Option<&[String]>, field: &str) -> Result<Option<Vec<String>>, String> {
    let Some(list) = list else { return Ok(None) };
    let hs: Vec<String> = list
        .iter()
        .map(|h| h.trim().trim_start_matches('@').to_owned())
        .filter(|h| !h.is_empty())
        .collect();
    if hs.len() > MAX_HANDLES {
        return Err(format!(
            "{field}: at most {MAX_HANDLES} handles, got {}",
            hs.len()
        ));
    }
    Ok((!hs.is_empty()).then_some(hs))
}

fn date(s: Option<&str>, field: &str) -> Result<Option<NaiveDate>, String> {
    let Some(s) = s.map(str::trim).filter(|s| !s.is_empty()) else {
        return Ok(None);
    };
    NaiveDate::parse_from_str(s, "%Y-%m-%d")
        .map(Some)
        .map_err(|_| format!("{field}: expected YYYY-MM-DD, got {s:?}"))
}

/// Validate the filters and build the `x_search` tool definition with only
/// the ones given; also the names of the filters set (for `degraded`).
fn tool_def(a: &XSearchArgs, today: NaiveDate) -> Result<(Value, Vec<&'static str>), String> {
    let mut def = serde_json::Map::new();
    def.insert("type".into(), json!("x_search"));
    let mut set = Vec::new();
    let allow = handles(a.handles.as_deref(), "handles")?;
    let deny = handles(a.exclude_handles.as_deref(), "exclude_handles")?;
    if allow.is_some() && deny.is_some() {
        return Err("handles and exclude_handles cannot be combined".into());
    }
    if let Some(h) = allow {
        def.insert("allowed_x_handles".into(), json!(h));
        set.push("handles");
    }
    if let Some(h) = deny {
        def.insert("excluded_x_handles".into(), json!(h));
        set.push("exclude_handles");
    }
    let from = date(a.from_date.as_deref(), "from_date")?;
    let to = date(a.to_date.as_deref(), "to_date")?;
    if let Some(f) = from
        && f > today
    {
        return Err(format!("from_date {f} is after today ({today})"));
    }
    if let (Some(f), Some(t)) = (from, to)
        && f > t
    {
        return Err(format!("from_date {f} is after to_date {t}"));
    }
    if let Some(f) = from {
        def.insert("from_date".into(), json!(f.to_string()));
        set.push("from_date");
    }
    if let Some(t) = to {
        def.insert("to_date".into(), json!(t.to_string()));
        set.push("to_date");
    }
    if let Some(b) = a.images {
        def.insert("enable_image_understanding".into(), json!(b));
    }
    if let Some(b) = a.videos {
        def.insert("enable_video_understanding".into(), json!(b));
    }
    Ok((Value::Object(def), set))
}

#[derive(Deserialize, Default)]
#[serde(default)]
struct Response {
    output_text: Option<String>,
    output: Vec<Item>,
    citations: Vec<String>,
}

#[derive(Deserialize, Default)]
#[serde(default)]
struct Item {
    #[serde(rename = "type")]
    kind: String,
    content: Vec<Part>,
}

#[derive(Deserialize, Default)]
#[serde(default)]
struct Part {
    #[serde(rename = "type")]
    kind: String,
    text: String,
    annotations: Vec<Annotation>,
}

#[derive(Deserialize, Default)]
#[serde(default)]
struct Annotation {
    #[serde(rename = "type")]
    kind: String,
    url: String,
    title: Option<String>,
}

#[derive(Clone, Debug, PartialEq, Serialize)]
struct Citation {
    url: String,
    title: Option<String>,
}

#[derive(Serialize)]
struct XSearchResult<'a> {
    query: &'a str,
    model: &'a str,
    credential: Credential,
    answer: String,
    citations: Vec<Citation>,
    degraded: bool,
    reasons: Vec<String>,
    took_ms: u128,
}

/// Answer text and citations (top-level URLs first, then inline
/// `url_citation` annotations), de-duplicated by URL in first-seen order.
fn parse(r: Response) -> (String, Vec<Citation>) {
    let parts: Vec<&Part> = r
        .output
        .iter()
        .filter(|i| i.kind == "message")
        .flat_map(|i| i.content.iter())
        .filter(|p| p.kind == "output_text" || p.kind == "text")
        .collect();
    let answer = match r.output_text {
        Some(t) if !t.trim().is_empty() => t,
        _ => parts
            .iter()
            .map(|p| p.text.as_str())
            .collect::<Vec<_>>()
            .join("\n"),
    };
    let mut cites: Vec<Citation> = Vec::new();
    let inline = parts
        .iter()
        .flat_map(|p| p.annotations.iter())
        .filter(|a| a.kind == "url_citation")
        .map(|a| (a.url.as_str(), a.title.as_deref()));
    for (url, title) in r.citations.iter().map(|u| (u.as_str(), None)).chain(inline) {
        if url.is_empty() {
            continue;
        }
        match cites.iter_mut().find(|c| c.url == url) {
            Some(c) => {
                if c.title.is_none() {
                    c.title = title.map(str::to_owned);
                }
            }
            None => cites.push(Citation {
                url: url.to_owned(),
                title: title.map(str::to_owned),
            }),
        }
    }
    (answer, cites)
}

fn structured<T: Serialize>(text: String, data: &T, is_error: bool) -> ToolOutput {
    ToolOutput {
        is_error,
        structured: serde_json::to_value(data).ok(),
        ..ToolOutput::ok(text)
    }
}

/// `search_x`: one Responses API call with the `x_search` tool.
pub struct XSearchTool {
    http: reqwest::Client,
    base: String,
    cfg: XConfig,
    vault: Option<Arc<secrets::Vault>>,
    oauth: oauth::Store,
    env_key: Option<String>,
}

impl XSearchTool {
    /// Tool against the production endpoint.
    #[must_use]
    pub fn new(
        cfg: XConfig,
        vault: Option<Arc<secrets::Vault>>,
        oauth: oauth::Store,
        env_key: Option<String>,
    ) -> Self {
        Self {
            http: reqwest::Client::builder()
                .user_agent(web::WebConfig::default().user_agent)
                .build()
                .unwrap_or_default(),
            base: BASE_URL.to_owned(),
            cfg,
            vault,
            oauth,
            env_key,
        }
    }

    /// Point at a different API root (tests).
    #[cfg(test)]
    fn with_base(mut self, base: &str) -> Self {
        self.base = base.trim_end_matches('/').to_owned();
        self
    }

    async fn bearer(&self) -> Result<Option<(Credential, String)>, String> {
        let vault = self
            .vault
            .as_ref()
            .and_then(|v| v.get(NS, 0))
            .map(|k| k.as_str().to_owned());
        let login = if vault.is_none() && self.oauth.load(NS).is_some() {
            Some(self.oauth.access(&oauth::XAI).await.map(|t| t.access_token))
        } else {
            None
        };
        credential(vault, login, self.env_key.clone())
    }

    async fn run(&self, a: XSearchArgs) -> ToolOutput {
        let query = a.query.trim();
        if query.is_empty() {
            return ToolOutput::err("query must not be empty");
        }
        let (def, filters) = match tool_def(&a, Utc::now().date_naive()) {
            Ok(v) => v,
            Err(e) => return ToolOutput::err(e),
        };
        let (credential, token) = match self.bearer().await {
            Ok(Some(c)) => c,
            Ok(None) => {
                return ToolOutput::err("No xAI credential: /login xai or add the `xai` secret");
            }
            Err(e) => return ToolOutput::err(e),
        };
        let body = json!({
            "model": self.cfg.model,
            "input": [{ "role": "user", "content": query }],
            "tools": [def],
            "store": false,
        });
        let started = Instant::now();
        let resp = self
            .http
            .post(format!("{}/responses", self.base))
            .timeout(Duration::from_millis(self.cfg.timeout_ms.max(1)))
            .bearer_auth(&token)
            .json(&body)
            .send()
            .await;
        let resp = match resp {
            Ok(r) => r,
            Err(e) => return ToolOutput::err(format!("xAI request failed: {e}")),
        };
        let status = resp.status();
        let text = resp.text().await.unwrap_or_default();
        if !status.is_success() {
            let head: String = text.chars().take(ERROR_BODY_CHARS).collect();
            let mut msg = format!("xAI HTTP {}: {head}", status.as_u16());
            if matches!(status.as_u16(), 401 | 403) {
                msg.push_str(" — /login xai again or set the xai secret");
            }
            return ToolOutput::err(msg);
        }
        let parsed: Response = match serde_json::from_str(&text) {
            Ok(p) => p,
            Err(e) => return ToolOutput::err(format!("xAI response: {e}")),
        };
        let (answer, citations) = parse(parsed);
        let degraded = !filters.is_empty() && citations.is_empty();
        let reasons = if degraded {
            vec![format!("no citations with filters {}", filters.join(", "))]
        } else {
            Vec::new()
        };
        let mut out = answer.clone();
        if !citations.is_empty() {
            out.push_str("\n\nSources:\n");
            for c in &citations {
                out.push_str(&c.url);
                out.push('\n');
            }
        }
        structured(
            out,
            &XSearchResult {
                query,
                model: &self.cfg.model,
                credential,
                answer,
                citations,
                degraded,
                reasons,
                took_ms: started.elapsed().as_millis(),
            },
            false,
        )
    }
}

#[async_trait::async_trait]
impl Tool for XSearchTool {
    const NAME: &'static str = "search_x";
    const RISK: Risk = Risk::ReadOnly;
    const RETURNS: &'static str = "dict";
    type Args = XSearchArgs;

    fn description(&self) -> String {
        format!(
            "Search X (Twitter) posts, profiles and threads through xAI's live index ({}): what \
             people are saying on X, reactions, claims, a handle's recent posts. Returns Grok's \
             synthesis with the cited posts; cite them as links. Not for general web pages — \
             that is search_web.",
            self.cfg.model
        )
    }

    async fn call(&self, args: XSearchArgs, _: ToolCx) -> anyhow::Result<ToolOutput> {
        Ok(self.run(args).await)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use wiremock::matchers::{body_partial_json, header, method, path};
    use wiremock::{Mock, MockServer, Request, ResponseTemplate};

    fn args(query: &str) -> XSearchArgs {
        XSearchArgs {
            query: query.into(),
            handles: None,
            exclude_handles: None,
            from_date: None,
            to_date: None,
            images: None,
            videos: None,
        }
    }

    fn today() -> NaiveDate {
        Utc::now().date_naive()
    }

    fn tempdir(tag: &str) -> std::path::PathBuf {
        let d = std::env::temp_dir().join(format!("xai-{tag}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&d);
        std::fs::create_dir_all(&d).unwrap();
        d
    }

    fn tool(server: &MockServer, dir: &std::path::Path, env: Option<&str>) -> XSearchTool {
        XSearchTool::new(
            XConfig::default(),
            None,
            oauth::Store::new(dir),
            env.map(str::to_owned),
        )
        .with_base(&server.uri())
    }

    #[test]
    fn validation_rejects_bad_filters_and_strips_at() {
        let mut a = args("q");
        a.handles = Some(vec!["@a".into()]);
        a.exclude_handles = Some(vec!["b".into()]);
        assert!(
            tool_def(&a, today())
                .unwrap_err()
                .contains("cannot be combined")
        );

        let mut a = args("q");
        a.handles = Some((0..21).map(|i| format!("h{i}")).collect());
        assert!(tool_def(&a, today()).unwrap_err().contains("at most 20"));

        let mut a = args("q");
        a.from_date = Some("2026/01/01".into());
        assert!(tool_def(&a, today()).unwrap_err().contains("YYYY-MM-DD"));

        let mut a = args("q");
        a.from_date = Some((today() + chrono::Days::new(1)).to_string());
        assert!(tool_def(&a, today()).unwrap_err().contains("after today"));

        let mut a = args("q");
        a.from_date = Some("2026-02-02".into());
        a.to_date = Some("2026-02-01".into());
        assert!(tool_def(&a, today()).unwrap_err().contains("after to_date"));

        let mut a = args("q");
        a.handles = Some(vec!["@elonmusk".into(), " ".into(), "x".into()]);
        a.images = Some(true);
        let (def, set) = tool_def(&a, today()).unwrap();
        assert_eq!(
            def,
            json!({ "type": "x_search", "allowed_x_handles": ["elonmusk", "x"], "enable_image_understanding": true })
        );
        assert_eq!(set, ["handles"]);
        assert_eq!(
            tool_def(&args("q"), today()).unwrap().0,
            json!({ "type": "x_search" })
        );
    }

    #[test]
    fn credential_precedence_is_vault_login_env() {
        let v = || Some("vk".to_owned());
        let l = || Some(Ok("lt".to_owned()));
        let e = || Some("ek".to_owned());
        assert_eq!(
            credential(v(), l(), e()).unwrap(),
            Some((Credential::Vault, "vk".into()))
        );
        assert_eq!(
            credential(None, l(), e()).unwrap(),
            Some((Credential::Login, "lt".into()))
        );
        assert_eq!(
            credential(None, None, e()).unwrap(),
            Some((Credential::Env, "ek".into()))
        );
        assert_eq!(credential(None, None, None).unwrap(), None);
        assert_eq!(
            credential(v(), Some(Err("expired".into())), None).unwrap(),
            Some((Credential::Vault, "vk".into()))
        );
        assert_eq!(
            credential(None, Some(Err("expired".into())), e()).unwrap_err(),
            "expired"
        );
    }

    #[tokio::test]
    async fn login_in_the_store_beats_the_env_key() {
        let dir = tempdir("login");
        let server = MockServer::start().await;
        let t = tool(&server, &dir, Some("env-key"));
        assert_eq!(
            t.bearer().await.unwrap(),
            Some((Credential::Env, "env-key".into()))
        );
        t.oauth
            .save(
                NS,
                &oauth::Tokens {
                    access_token: "login-token".into(),
                    refresh_token: None,
                    expires_at: u64::MAX / 2,
                    account_id: None,
                },
            )
            .await
            .unwrap();
        assert_eq!(
            t.bearer().await.unwrap(),
            Some((Credential::Login, "login-token".into()))
        );
        let _ = std::fs::remove_dir_all(dir);
    }

    #[tokio::test]
    async fn answer_and_citations_from_a_message() {
        let dir = tempdir("ok");
        let server = MockServer::start().await;
        Mock::given(method("POST"))
            .and(path("/responses"))
            .and(header("Authorization", "Bearer env-key"))
            .and(body_partial_json(json!({
                "model": "grok-4.7",
                "tools": [{ "type": "x_search" }],
                "store": false,
                "input": [{ "role": "user", "content": "rust 2026" }]
            })))
            .respond_with(ResponseTemplate::new(200).set_body_json(json!({
                "citations": ["https://x.com/a/status/1"],
                "output": [
                    { "type": "reasoning", "content": [] },
                    { "type": "message", "content": [{
                        "type": "output_text",
                        "text": "People like it.",
                        "annotations": [
                            { "type": "url_citation", "url": "https://x.com/a/status/1", "title": "A" },
                            { "type": "url_citation", "url": "https://x.com/b/status/2", "title": "B" },
                            { "type": "url_citation", "url": "https://x.com/b/status/2" }
                        ]
                    }]}
                ]
            })))
            .expect(1)
            .mount(&server)
            .await;
        let out = tool(&server, &dir, Some("env-key"))
            .run(args(" rust 2026 "))
            .await;
        assert!(!out.is_error, "{}", out.content);
        assert_eq!(
            out.content,
            "People like it.\n\nSources:\nhttps://x.com/a/status/1\nhttps://x.com/b/status/2\n"
        );
        let s = out.structured.unwrap();
        assert_eq!(s["credential"], "env");
        assert_eq!(s["degraded"], false);
        assert_eq!(s["citations"][0]["title"], "A");
        assert_eq!(s["citations"][1]["url"], "https://x.com/b/status/2");
        assert_eq!(s["citations"].as_array().unwrap().len(), 2);
        let sent: Value =
            serde_json::from_slice(&server.received_requests().await.unwrap()[0].body).unwrap();
        assert_eq!(sent["tools"][0], json!({ "type": "x_search" }));
        let _ = std::fs::remove_dir_all(dir);
    }

    #[tokio::test]
    async fn filters_with_no_citations_are_degraded() {
        let dir = tempdir("degraded");
        let server = MockServer::start().await;
        Mock::given(method("POST"))
            .and(path("/responses"))
            .respond_with(
                ResponseTemplate::new(200)
                    .set_body_json(json!({ "output_text": "Nothing found." })),
            )
            .mount(&server)
            .await;
        let mut a = args("anything");
        a.handles = Some(vec!["@someone".into()]);
        a.from_date = Some("2026-01-01".into());
        let out = tool(&server, &dir, Some("k")).run(a).await;
        assert!(!out.is_error);
        assert_eq!(out.content, "Nothing found.");
        let s = out.structured.unwrap();
        assert_eq!(s["degraded"], true);
        assert_eq!(
            s["reasons"][0],
            "no citations with filters handles, from_date"
        );
        let req: &Request = &server.received_requests().await.unwrap()[0];
        let sent: Value = serde_json::from_slice(&req.body).unwrap();
        assert_eq!(
            sent["tools"][0],
            json!({ "type": "x_search", "allowed_x_handles": ["someone"], "from_date": "2026-01-01" })
        );
        assert_eq!(sent["store"], false);
        let _ = std::fs::remove_dir_all(dir);
    }

    #[tokio::test]
    async fn unauthorized_names_login() {
        let dir = tempdir("401");
        let server = MockServer::start().await;
        Mock::given(method("POST"))
            .respond_with(ResponseTemplate::new(401).set_body_string("{\"error\":\"bad key\"}"))
            .mount(&server)
            .await;
        let out = tool(&server, &dir, Some("k")).run(args("q")).await;
        assert!(out.is_error);
        assert_eq!(
            out.content,
            "xAI HTTP 401: {\"error\":\"bad key\"} — /login xai again or set the xai secret"
        );
        let none = tool(&server, &dir, None).run(args("q")).await;
        assert!(none.is_error && none.content.contains("/login xai"));
        let bad = tool(&server, &dir, Some("k")).run(args("  ")).await;
        assert!(bad.is_error && bad.content.contains("query"));
        let _ = std::fs::remove_dir_all(dir);
    }
}
