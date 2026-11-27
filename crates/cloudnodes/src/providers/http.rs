//! The one HTTP seam of the provider clients: [`Transport`] sends a [`HttpRequest`]; production
//! uses [`ReqwestTransport`], tests [`testing::FakeTransport`] (scripted responses, recorded
//! requests). The bearer token lives in a [`Zeroizing`] string and never reaches `Debug` output.

use std::sync::Arc;
use std::time::Duration;

use anyhow::Context as _;
use async_trait::async_trait;
use serde_json::Value;
use zeroize::Zeroizing;

use super::{CloudError, Result};

/// One request. `json` and `form` are exclusive (`json` wins).
#[derive(Clone, Default)]
pub struct HttpRequest {
    /// `GET`, `POST`, `DELETE`.
    pub method: String,
    /// Absolute URL (never carries a credential).
    pub url: String,
    /// Sent as `Authorization: Bearer`.
    pub bearer: Option<Zeroizing<String>>,
    /// JSON body.
    pub json: Option<Value>,
    /// Form body (when no `json`).
    pub form: Option<Vec<(String, String)>>,
}

impl std::fmt::Debug for HttpRequest {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("HttpRequest")
            .field("method", &self.method)
            .field("url", &self.url)
            .field("bearer", &self.bearer.as_ref().map(|_| "<redacted>"))
            .field("json", &self.json)
            .field("form", &self.form.as_ref().map(|f| f.len()))
            .finish()
    }
}

#[derive(Clone, Debug, Default, PartialEq)]
/// Status and raw body.
pub struct HttpResponse {
    /// HTTP status.
    pub status: u16,
    /// Raw body.
    pub body: Vec<u8>,
}

impl HttpResponse {
    /// The body as JSON; an empty body is `null`.
    pub fn json(&self) -> anyhow::Result<Value> {
        if self.body.iter().all(u8::is_ascii_whitespace) {
            return Ok(Value::Null);
        }
        serde_json::from_slice(&self.body).context("response is not JSON")
    }
}

#[async_trait]
/// Sends one request (reqwest in production, scripted in tests).
pub trait Transport: Send + Sync {
    /// Any HTTP status is `Ok`; `Err` only for transport failures.
    async fn send(&self, req: HttpRequest) -> anyhow::Result<HttpResponse>;
}

/// reqwest (rustls), 60 s timeout, UA `theuth-cloudnodes`.
pub struct ReqwestTransport {
    client: reqwest::Client,
}

impl ReqwestTransport {
    /// The production client.
    pub fn new() -> anyhow::Result<Self> {
        let client = reqwest::Client::builder()
            .timeout(Duration::from_secs(60))
            .user_agent("theuth-cloudnodes")
            .build()
            .context("http client")?;
        Ok(Self { client })
    }
}

#[async_trait]
impl Transport for ReqwestTransport {
    async fn send(&self, req: HttpRequest) -> anyhow::Result<HttpResponse> {
        let method = reqwest::Method::from_bytes(req.method.as_bytes()).context("http method")?;
        let mut b = self.client.request(method, &req.url);
        if let Some(t) = &req.bearer {
            b = b.bearer_auth(t.as_str());
        }
        if let Some(j) = &req.json {
            b = b.json(j);
        } else if let Some(f) = &req.form {
            b = b.form(f);
        }
        // reqwest's error names the URL, never headers.
        let resp = b.send().await.with_context(|| format!("{} {}", req.method, req.url))?;
        let status = resp.status().as_u16();
        let body = resp.bytes().await.context("response body")?.to_vec();
        Ok(HttpResponse { status, body })
    }
}

/// A provider REST API: base URL, bearer token, error text the UI can show.
pub(crate) struct Api {
    label: &'static str,
    base: String,
    token: Zeroizing<String>,
    transport: Arc<dyn Transport>,
}

pub(crate) const OK: &[u16] = &[200, 201, 202, 204];

impl Api {
    pub fn new(
        label: &'static str,
        base: &str,
        token: Zeroizing<String>,
        transport: Arc<dyn Transport>,
    ) -> Self {
        Self {
            label,
            base: base.trim_end_matches('/').to_owned(),
            token,
            transport,
        }
    }

    /// `method base+path` with `body`; a status outside `ok` is an error.
    pub async fn call_any(
        &self,
        method: &str,
        path: &str,
        body: Option<Value>,
        ok: &[u16],
    ) -> Result<(u16, Value)> {
        let req = HttpRequest {
            method: method.to_owned(),
            url: format!("{}{path}", self.base),
            bearer: Some(self.token.clone()),
            json: body,
            form: None,
        };
        let resp = self.transport.send(req).await.map_err(|e| {
            CloudError(format!("{}: {method} {}: {e:#}", self.label, strip_query(path)))
        })?;
        if !ok.contains(&resp.status) {
            let v = resp.json().unwrap_or(Value::Null);
            let msg = v["message"]
                .as_str()
                .or(v["error"].as_str())
                .map(str::to_owned)
                .unwrap_or_else(|| {
                    String::from_utf8_lossy(&resp.body).chars().take(200).collect()
                });
            return Err(CloudError(format!(
                "{}: {method} {}: HTTP {} {msg}",
                self.label,
                strip_query(path),
                resp.status
            )));
        }
        let v = resp
            .json()
            .map_err(|e| CloudError(format!("{}: {method} {}: {e}", self.label, strip_query(path))))?;
        Ok((resp.status, v))
    }

    pub async fn call(&self, method: &str, path: &str, body: Option<Value>) -> Result<Value> {
        Ok(self.call_any(method, path, body, OK).await?.1)
    }
}

fn strip_query(path: &str) -> &str {
    path.split('?').next().unwrap_or(path)
}

/// Percent-encode one query/path component.
pub(crate) fn enc(s: &str) -> String {
    url::form_urlencoded::byte_serialize(s.as_bytes())
        .collect::<String>()
        .replace('+', "%20")
}

#[cfg(any(test, feature = "testing"))]
pub mod testing {
    //! A scripted [`Transport`]: rules `(method, url substring) → (status, JSON)`, first match
    //! wins; a `once` rule is used up by its first match. Unmatched requests get 404 `{}`.
    use super::*;
    use parking_lot::Mutex;

    struct Rule {
        method: String,
        url_part: String,
        status: u16,
        body: Value,
        once: bool,
    }

    #[derive(Default)]
    /// Scripted responses, recorded requests.
    pub struct FakeTransport {
        rules: Mutex<Vec<Rule>>,
        log: Mutex<Vec<HttpRequest>>,
    }

    impl FakeTransport {
        /// An empty script (every request answers 404).
        pub fn new() -> Arc<Self> {
            Arc::new(Self::default())
        }

        /// Answer every matching request.
        pub fn on(&self, method: &str, url_part: &str, status: u16, body: Value) -> &Self {
            self.push(method, url_part, status, body, false)
        }

        /// Answer the next matching request only (earlier `once` rules shadow later `on` rules).
        pub fn once(&self, method: &str, url_part: &str, status: u16, body: Value) -> &Self {
            self.push(method, url_part, status, body, true)
        }

        fn push(&self, method: &str, url_part: &str, status: u16, body: Value, once: bool) -> &Self {
            self.rules.lock().push(Rule {
                method: method.into(),
                url_part: url_part.into(),
                status,
                body,
                once,
            });
            self
        }

        /// Every request so far, in order.
        pub fn requests(&self) -> Vec<HttpRequest> {
            self.log.lock().clone()
        }

        /// `"METHOD url"` per request.
        pub fn lines(&self) -> Vec<String> {
            self.log
                .lock()
                .iter()
                .map(|r| format!("{} {}", r.method, r.url))
                .collect()
        }
    }

    #[async_trait]
    impl Transport for FakeTransport {
        async fn send(&self, req: HttpRequest) -> anyhow::Result<HttpResponse> {
            self.log.lock().push(req.clone());
            let mut rules = self.rules.lock();
            let Some(i) = rules
                .iter()
                .position(|r| r.method == req.method && req.url.contains(&r.url_part))
            else {
                return Ok(HttpResponse {
                    status: 404,
                    body: b"{}".to_vec(),
                });
            };
            let r = &rules[i];
            let resp = HttpResponse {
                status: r.status,
                body: if r.body.is_null() {
                    Vec::new()
                } else {
                    serde_json::to_vec(&r.body)?
                },
            };
            if r.once {
                rules.remove(i);
            }
            Ok(resp)
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn debug_redacts_the_bearer() {
        let r = HttpRequest {
            method: "GET".into(),
            url: "https://x/y".into(),
            bearer: Some(Zeroizing::new("dop_v1_secret".into())),
            ..Default::default()
        };
        let s = format!("{r:?}");
        assert!(!s.contains("dop_v1_secret") && s.contains("<redacted>"));
    }

    #[tokio::test]
    async fn api_errors_name_the_call_not_the_token() {
        let t = testing::FakeTransport::new();
        t.on("GET", "/x", 401, serde_json::json!({"message": "Unable to authenticate you"}));
        let api = Api::new("DigitalOcean", "https://api/v2", Zeroizing::new("tok".into()), t.clone());
        let e = api.call("GET", "/x?a=b", None).await.unwrap_err().0;
        assert_eq!(e, "DigitalOcean: GET /x: HTTP 401 Unable to authenticate you");
        assert_eq!(t.requests()[0].bearer.as_deref().map(String::as_str), Some("tok"));
        assert_eq!(enc("theuth-node:a b"), "theuth-node%3Aa%20b");
    }
}
