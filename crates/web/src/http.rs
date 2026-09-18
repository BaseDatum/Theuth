//! `http_request` (design doc 46 §5): one explicit HTTP call to a REST
//! endpoint through [`Fetcher::request`], so it shares the SSRF guard, the
//! guarded resolver and the per-hop redirect checks of `read_url`; with any
//! caller header present a redirect to another origin is refused, so a key
//! under any header name never leaves the origin the caller named. The
//! reply comes back as data: `body` is the parsed JSON value when the
//! service answered JSON, otherwise the text. A reply that declares JSON
//! but does not parse is an error result (46 §12 rule 5: request errors are
//! real errors); its text still rides along so the caller can see why.
//!
//! Bounds: [`MAX_BYTES`] of reply body (streamed transfer included, the
//! fetcher cuts at the cap), [`TIMEOUT`] for the whole call including
//! redirects, no retries of any kind. The cap is on body bytes: the
//! serialized [`HttpResponse`] (headers, JSON escaping) and the
//! `TEXT_CHARS` excerpt in `content` can push the whole result a little
//! past 1 MiB. The "1 MiB serialized response before client forwarding"
//! bound of 46 §11 is the app plugin's to enforce on the serialized tool
//! result; this module only guarantees the body half. `Risk::Mutating` for
//! every method: the verb is caller-chosen and the tool cannot know a GET is
//! side-effect free. Secret header values arrive as placeholders and are
//! expanded by the registry's `Invoke` before `call` runs; this module never
//! touches the vault.

use std::borrow::Cow;
use std::collections::BTreeMap;
use std::collections::btree_map::Entry;
use std::sync::Arc;
use std::time::Duration;

use reqwest::header::{HeaderName, HeaderValue};
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use serde_json::Value;
use tools::{Risk, Tool, ToolCx, ToolOutput};

use crate::fetch::{Fetcher, RawResponse, hard_truncate, is_binary};

/// Reply body cap in bytes (46 §5), request body cap too.
pub const MAX_BYTES: usize = 1 << 20;
/// Deadline for the whole call, redirects included.
pub const TIMEOUT: Duration = Duration::from_secs(30);
/// Body characters echoed in the text half of the result; the structured
/// half carries the full (capped) body.
const TEXT_CHARS: usize = 4000;

/// Methods `http_request` accepts.
const METHODS: [&str; 7] = ["GET", "HEAD", "POST", "PUT", "PATCH", "DELETE", "OPTIONS"];

/// Arguments for [`HttpRequestTool`].
#[derive(Debug, Serialize, Deserialize, JsonSchema)]
pub struct HttpRequestArgs {
    /// Absolute http(s) URL, no embedded credentials.
    pub url: String,
    /// GET, HEAD, POST, PUT, PATCH, DELETE or OPTIONS (any case).
    pub method: String,
    /// Request headers by name. For a secret, put its placeholder as the
    /// value; the harness expands it. Headers go only to the origin of `url`:
    /// a redirect elsewhere fails instead of forwarding them. `Host` and
    /// `Content-Length` are set by the client and rejected here.
    #[serde(default)]
    pub headers: Option<BTreeMap<String, String>>,
    /// Request body, at most 1 MiB. A string is sent verbatim; any other JSON
    /// value is serialized and sent as `application/json` unless `headers`
    /// names a `Content-Type`. Not allowed with GET or HEAD.
    #[serde(default)]
    pub body: Option<Value>,
}

/// The reply of one request (the `structured` half of `http_request`).
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct HttpResponse {
    /// Final URL after redirects.
    pub url: String,
    /// HTTP status code.
    pub status: u16,
    /// Response headers, lowercase names; repeated headers joined with `, `.
    pub headers: BTreeMap<String, String>,
    /// Parsed JSON when the reply is JSON (`application/json`, `text/json`,
    /// `+json`), complete and well formed; the text otherwise; `null` for an
    /// empty body. A body cut at the cap is never parsed; a declared-JSON
    /// body that does not parse stays text and the result is an error.
    pub body: Value,
    /// The body exceeded [`MAX_BYTES`] and was cut there.
    pub truncated: bool,
    /// Wall time in milliseconds.
    pub took_ms: u64,
}

/// `http_request`: explicit method, headers and body; structured reply.
pub struct HttpRequestTool {
    /// Built with [`MAX_BYTES`] and [`TIMEOUT`] (see `WebPlugin`).
    pub fetcher: Arc<Fetcher>,
    /// Whole-call deadline, [`TIMEOUT`] in production.
    pub deadline: Duration,
}

/// Validated request: method, `(lowercase name, value)` headers, body bytes.
#[derive(Debug)]
struct Prepared {
    method: &'static str,
    headers: Vec<(String, String)>,
    body: Option<Vec<u8>>,
}

/// Validate `a` before anything is sent. Errors are for the caller.
fn prepare(a: &HttpRequestArgs) -> Result<Prepared, String> {
    let wanted = a.method.trim().to_ascii_uppercase();
    let method = METHODS
        .iter()
        .copied()
        .find(|m| *m == wanted)
        .ok_or_else(|| {
            format!(
                "method must be one of {}, got `{}`",
                METHODS.join(", "),
                a.method
            )
        })?;
    let mut headers = Vec::new();
    let mut has_content_type = false;
    for (name, value) in a.headers.iter().flatten() {
        if HeaderName::from_bytes(name.as_bytes()).is_err() {
            return Err(format!("invalid header name `{name}`"));
        }
        if name.eq_ignore_ascii_case("host") || name.eq_ignore_ascii_case("content-length") {
            return Err(format!(
                "header `{name}` is set by the client; do not pass it"
            ));
        }
        if HeaderValue::from_str(value).is_err() {
            return Err(format!("invalid value for header `{name}`"));
        }
        let lower = name.to_ascii_lowercase();
        has_content_type |= lower == "content-type";
        headers.push((lower, value.clone()));
    }
    let body = match &a.body {
        None | Some(Value::Null) => None,
        Some(_) if matches!(method, "GET" | "HEAD") => {
            return Err(format!("{method} takes no body"));
        }
        Some(Value::String(s)) => Some(s.clone().into_bytes()),
        Some(v) => {
            if !has_content_type {
                headers.push(("content-type".to_owned(), "application/json".to_owned()));
            }
            Some(serde_json::to_vec(v).map_err(|e| format!("body: {e}"))?)
        }
    };
    if body.as_ref().is_some_and(|b| b.len() > MAX_BYTES) {
        return Err(format!("body larger than {MAX_BYTES} bytes"));
    }
    Ok(Prepared {
        method,
        headers,
        body,
    })
}

/// Lowercased primary media type of `content-type`, without parameters.
fn media_type(headers: &BTreeMap<String, String>) -> Option<String> {
    headers.get("content-type").map(|ct| {
        ct.split(';')
            .next()
            .unwrap_or(ct)
            .trim()
            .to_ascii_lowercase()
    })
}

fn is_json_type(ct: Option<&str>) -> bool {
    ct.is_some_and(|ct| matches!(ct, "application/json" | "text/json") || ct.ends_with("+json"))
}

/// Turn the raw reply into the tool result. Non-2xx, a body cut at the cap,
/// a binary reply and a declared-JSON body that does not parse are errors
/// that still carry status, headers and the (bounded) body, so a consumer
/// can act on them.
fn respond(method: &str, raw: RawResponse) -> ToolOutput {
    let mut headers: BTreeMap<String, String> = BTreeMap::new();
    for (k, v) in raw.headers {
        match headers.entry(k) {
            Entry::Occupied(mut e) => {
                let e = e.get_mut();
                e.push_str(", ");
                e.push_str(&v);
            }
            Entry::Vacant(e) => {
                e.insert(v);
            }
        }
    }
    let ct = media_type(&headers);
    let binary = is_binary(ct.as_deref());
    // Borrows `raw.body` when it is valid UTF-8; owned only on the text path.
    let text = if binary {
        Cow::Borrowed("")
    } else {
        String::from_utf8_lossy(&raw.body)
    };
    // Parse the bytes, not the text: valid JSON never needs the string copy.
    let json = (!raw.body.is_empty() && !binary && !raw.truncated && is_json_type(ct.as_deref()))
        .then(|| serde_json::from_slice::<Value>(&raw.body));
    let bad_json = json
        .as_ref()
        .and_then(|r| r.as_ref().err().map(ToString::to_string));
    let ok_status = (200..300).contains(&raw.status);
    let is_error = !ok_status || raw.truncated || binary || bad_json.is_some();
    let mut content = format!(
        "HTTP {} for {method} {} ({} bytes, {} ms)",
        raw.status,
        raw.url,
        raw.body.len(),
        raw.took_ms
    );
    if raw.truncated {
        content.push_str(&format!(
            "\nbody exceeded {MAX_BYTES} bytes and was cut; not parsed as JSON"
        ));
    }
    if let Some(e) = &bad_json {
        content.push_str(&format!(
            "\nreply declared {} but is not valid JSON ({e}); body returned as text",
            ct.as_deref().unwrap_or("json")
        ));
    }
    if binary {
        content.push_str(&format!(
            "\nbinary reply ({}) is not returned",
            ct.as_deref().unwrap_or("unknown")
        ));
    } else if !text.is_empty() {
        let (excerpt, cut) = hard_truncate(&text, TEXT_CHARS);
        content.push('\n');
        content.push_str(&excerpt);
        if cut {
            content.push_str("\n… (text cut; the structured result has the full body)");
        }
    }
    let body = match json {
        Some(Ok(v)) => v,
        _ if raw.body.is_empty() || binary => Value::Null,
        _ => Value::String(text.into_owned()),
    };
    let resp = HttpResponse {
        url: raw.url,
        status: raw.status,
        headers,
        body,
        truncated: raw.truncated,
        took_ms: raw.took_ms,
    };
    ToolOutput {
        is_error,
        structured: serde_json::to_value(&resp).ok(),
        ..ToolOutput::ok(content)
    }
}

#[async_trait::async_trait]
impl Tool for HttpRequestTool {
    const NAME: &'static str = "http_request";
    const RISK: Risk = Risk::Mutating;
    const RETURNS: &'static str = "dict";
    type Args = HttpRequestArgs;

    fn description(&self) -> String {
        "One HTTP request to an API endpoint with an explicit method, headers and body. \
         Returns {url, status, headers, body, truncated, took_ms}; body is the parsed JSON \
         when the service answers JSON, else the text. Non-2xx, a reply cut at the cap and a \
         JSON reply that does not parse are error results that still carry status, headers \
         and body. For a secret header put its placeholder as the value. \
         No retries; replies are capped at 1 MiB and 30 s. Use read_url for web pages."
            .to_owned()
    }

    async fn call(&self, a: HttpRequestArgs, _cx: ToolCx) -> anyhow::Result<ToolOutput> {
        let p = match prepare(&a) {
            Ok(p) => p,
            Err(e) => return Ok(ToolOutput::err(e)),
        };
        let sent = self.fetcher.request(p.method, &a.url, &p.headers, p.body);
        match tokio::time::timeout(self.deadline, sent).await {
            Ok(Ok(raw)) => Ok(respond(p.method, raw)),
            Ok(Err(e)) => Ok(ToolOutput::err(e.to_string())),
            Err(_) => Ok(ToolOutput::err(format!(
                "{} {} produced no complete reply within {} s",
                p.method,
                a.url,
                self.deadline.as_secs()
            ))),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn args(
        method: &str,
        headers: Option<BTreeMap<String, String>>,
        body: Option<Value>,
    ) -> HttpRequestArgs {
        HttpRequestArgs {
            url: "https://api.example/x".into(),
            method: method.into(),
            headers,
            body,
        }
    }

    fn hdrs(pairs: &[(&str, &str)]) -> Option<BTreeMap<String, String>> {
        Some(
            pairs
                .iter()
                .map(|(k, v)| ((*k).to_owned(), (*v).to_owned()))
                .collect(),
        )
    }

    #[test]
    fn method_is_normalised_and_restricted() {
        assert_eq!(prepare(&args(" post ", None, None)).unwrap().method, "POST");
        assert!(prepare(&args("BREW", None, None)).is_err());
        assert!(prepare(&args("GET", None, Some(json!({"a": 1})))).is_err());
        assert!(prepare(&args("HEAD", None, Some(json!("x")))).is_err());
    }

    #[test]
    fn json_body_gets_content_type_unless_given() {
        let p = prepare(&args("POST", None, Some(json!({"a": 1})))).unwrap();
        assert_eq!(p.body.as_deref(), Some(br#"{"a":1}"#.as_slice()));
        assert_eq!(
            p.headers,
            [("content-type".to_owned(), "application/json".to_owned())]
        );

        let p = prepare(&args(
            "PUT",
            hdrs(&[("Content-Type", "application/vnd.api+json")]),
            Some(json!([1])),
        ))
        .unwrap();
        assert_eq!(
            p.headers,
            [(
                "content-type".to_owned(),
                "application/vnd.api+json".to_owned()
            )]
        );

        let p = prepare(&args("POST", None, Some(json!("raw=1")))).unwrap();
        assert_eq!(p.body.as_deref(), Some(b"raw=1".as_slice()));
        assert!(p.headers.is_empty(), "string body sets no content-type");

        assert!(
            prepare(&args("DELETE", None, Some(Value::Null)))
                .unwrap()
                .body
                .is_none()
        );
    }

    #[test]
    fn client_owned_and_malformed_headers_are_refused() {
        for (name, value) in [
            ("Host", "evil.example"),
            ("Content-Length", "1"),
            ("bad header", "x"),
            ("x-a", "line\nbreak"),
        ] {
            assert!(
                prepare(&args("GET", hdrs(&[(name, value)]), None)).is_err(),
                "{name}: {value:?}"
            );
        }
        assert!(prepare(&args("GET", hdrs(&[("X-Api-Key", "k")]), None)).is_ok());
    }

    #[test]
    fn oversize_request_body_is_refused() {
        assert!(
            prepare(&args(
                "POST",
                None,
                Some(Value::String("x".repeat(MAX_BYTES)))
            ))
            .is_ok()
        );
        let big = "x".repeat(MAX_BYTES + 1);
        assert!(prepare(&args("POST", None, Some(Value::String(big)))).is_err());
    }

    fn raw(status: u16, ct: Option<&str>, body: &[u8], truncated: bool) -> RawResponse {
        RawResponse {
            url: "https://api.example/x".into(),
            status,
            headers: ct
                .map(|ct| vec![("content-type".to_owned(), ct.to_owned())])
                .unwrap_or_default(),
            body: body.to_vec(),
            truncated,
            took_ms: 1,
        }
    }

    fn structured(out: &ToolOutput) -> HttpResponse {
        serde_json::from_value(out.structured.clone().expect("structured")).expect("shape")
    }

    #[test]
    fn json_reply_is_data() {
        let out = respond(
            "GET",
            raw(
                200,
                Some("application/json; charset=utf-8"),
                br#"{"ok":true,"n":[1,2]}"#,
                false,
            ),
        );
        assert!(!out.is_error, "{}", out.content);
        let r = structured(&out);
        assert_eq!(r.status, 200);
        assert_eq!(r.body, json!({"ok": true, "n": [1, 2]}));
        assert!(
            out.content.contains(r#"{"ok":true,"n":[1,2]}"#),
            "{}",
            out.content
        );
    }

    #[test]
    fn non_json_is_text_and_malformed_declared_json_is_error_text() {
        let out = respond("GET", raw(200, Some("text/plain"), b"hello", false));
        assert!(!out.is_error);
        assert_eq!(structured(&out).body, json!("hello"));
        let out = respond("GET", raw(200, Some("application/json"), b"{oops", false));
        assert!(
            out.is_error,
            "declared JSON that does not parse is not a success"
        );
        assert_eq!(structured(&out).body, json!("{oops"));
        let out = respond("HEAD", raw(204, Some("application/json"), b"", false));
        assert!(!out.is_error);
        assert_eq!(structured(&out).body, Value::Null);
    }

    #[test]
    fn non_success_is_error_with_parsed_body() {
        let out = respond(
            "POST",
            raw(
                422,
                Some("application/problem+json"),
                br#"{"detail":"bad"}"#,
                false,
            ),
        );
        assert!(out.is_error);
        let r = structured(&out);
        assert_eq!(r.status, 422);
        assert_eq!(r.body, json!({"detail": "bad"}));
        assert!(
            out.content.contains(r#"{"detail":"bad"}"#),
            "{}",
            out.content
        );
    }

    #[test]
    fn truncated_json_is_error_and_never_parsed() {
        let out = respond(
            "GET",
            raw(200, Some("application/json"), br#"{"a":[1,2,"#, true),
        );
        assert!(out.is_error);
        let r = structured(&out);
        assert!(r.truncated);
        assert_eq!(r.body, json!(r#"{"a":[1,2,"#));
    }

    #[test]
    fn binary_reply_is_error_without_body() {
        let out = respond("GET", raw(200, Some("image/png"), b"\x89PNG", false));
        assert!(out.is_error);
        let r = structured(&out);
        assert_eq!(r.body, Value::Null);
        assert!(!out.content.contains("PNG"), "{}", out.content);
    }

    #[test]
    fn repeated_headers_join_and_long_text_is_cut_in_content_only() {
        let mut r = raw(
            200,
            Some("text/plain"),
            "y".repeat(TEXT_CHARS + 10).as_bytes(),
            false,
        );
        r.headers.push(("vary".into(), "a".into()));
        r.headers.push(("vary".into(), "b".into()));
        let out = respond("GET", r);
        assert!(!out.is_error);
        let s = structured(&out);
        assert_eq!(s.headers["vary"], "a, b");
        let full = s.body.as_str().expect("text body");
        assert_eq!(full.len(), TEXT_CHARS + 10);
        assert!(
            !out.content.contains(full),
            "content carries the whole body"
        );
    }
}
