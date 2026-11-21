//! 158 §16 W5 proof: catalogue snapshot, Bearer/Origin/Host rejections, paging, the Private
//! filter, the audit line, redaction, an HTTP round trip with rmcp's own client, the stdio
//! bridge over a duplex forwarding to `mcp_server_call`.

use std::collections::HashMap;
use std::sync::Arc;

use parking_lot::Mutex;

use mcp_server::{Audit, Bridge, Catalog, McpCallResult, Rpc, RpcError, catalog};
use rmcp::model::{CallToolRequestParams, CallToolResult, ClientConfig, Implementation, Tool};
use rmcp::transport::StreamableHttpClientTransport;
use rmcp::transport::streamable_http_client::StreamableHttpClientTransportConfig;
use rmcp::{ClientHandler, ServiceExt};
use serde_json::{Value, json};
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio_util::sync::CancellationToken;

/// A fake engine: canned replies per method, every call recorded. With `catalog` set it also
/// answers `mcp_server_call` the way the engine does — by running that catalogue.
#[derive(Default)]
struct Fake {
    calls: Mutex<Vec<(String, Vec<Value>)>>,
    catalog: Mutex<Option<Catalog>>,
}

#[async_trait::async_trait]
impl Rpc for Fake {
    async fn call(&self, method: &str, params: Vec<Value>) -> Result<Value, RpcError> {
        self.calls.lock().push((method.to_owned(), params.clone()));
        if method == "mcp_server_call" {
            let catalog = self.catalog.lock().clone().expect("engine-side catalogue");
            let args = match &params[1] {
                Value::Object(m) => m.clone(),
                _ => Default::default(),
            };
            let r = catalog
                .call(
                    params[0].as_str().unwrap(),
                    args,
                    params[2].as_str().unwrap(),
                )
                .await
                .map_err(|e| RpcError::Call(e.message.to_string()))?;
            return Ok(serde_json::to_value(r).unwrap());
        }
        Ok(match method {
            "meeting_list" => {
                let limit = params[0].as_u64().unwrap_or(100).min(120) as usize;
                Value::Array(
                    (0..limit)
                        .map(|i| json!({"id": format!("m{i}"), "started_at": format!("2026-10-{:02}T10:00:00Z", 1 + i % 28)}))
                        .collect(),
                )
            }
            "list_sessions" => json!([
                {"id": "s-pub", "project": "p1", "retention": "durable"},
                {"id": "s-priv", "project": "p1", "retention": "private"},
                {"id": "s-other", "project": "p2", "retention": "durable"},
            ]),
            "load_session" => {
                let private = params[0] == json!("s-priv");
                json!({
                    "retention": if private { "private" } else { "durable" },
                    "project": "p1",
                    "kind": "chat",
                    "history": (0..5).map(|i| json!({"role": "user", "text": format!("t{i}")})).collect::<Vec<_>>(),
                })
            }
            "list_artifacts" => json!([
                {"id": "a1", "session": "s-pub"},
                {"id": "a2", "session": "s-priv"},
            ]),
            "meeting_summary" => json!("# Summary\nthe api key is hunter2-secret-value"),
            "task_comment" => Value::Null,
            "meeting_transcript" => json!({
                "meeting": "m1", "speakers": [], "diarized": true,
                "segments": (0..300).map(|i| json!({"id": i, "text": format!("s{i}")})).collect::<Vec<_>>(),
            }),
            "search" => {
                json!({"groups": [{"category": "meetings", "page": {"hits": [{"meeting": {"id": "m1"}}]}}]})
            }
            other => return Err(RpcError::Call(format!("no such method {other}"))),
        })
    }
}

/// A real vault (file key, in `dir`) holding one secret, `hunter2-secret-value`.
async fn vault(dir: &std::path::Path) -> Arc<secrets::Vault> {
    let v = secrets::Vault::open(dir, secrets::KeyStore::File)
        .await
        .unwrap();
    v.set("api.key", "hunter2-secret-value", None, "test")
        .await
        .unwrap();
    Arc::new(v)
}

async fn catalog(fake: &Arc<Fake>, dir: &std::path::Path, transport: &'static str) -> Catalog {
    Catalog::new(fake.clone(), transport, Audit::new(dir), vault(dir).await)
}

fn req(name: &'static str, a: Value) -> CallToolRequestParams {
    let mut p = CallToolRequestParams::new(name);
    if let Value::Object(o) = a {
        p = p.with_arguments(o);
    }
    p
}

fn text(r: &CallToolResult) -> &str {
    &r.content[0].as_text().unwrap().text
}

fn json(r: &CallToolResult) -> Value {
    assert_ne!(r.is_error, Some(true), "{r:?}");
    serde_json::from_str(text(r)).unwrap()
}

/// 158 §15.5 + A2: the frozen list, in order; every schema is an object schema. Set
/// `MCP_SNAPSHOT=1` to rewrite `tests/catalog.snapshot.json` after a deliberate change.
#[test]
fn catalogue_snapshot() {
    let tools: Vec<Tool> = catalog::tools();
    let snapshot: Vec<Value> = tools
        .iter()
        .map(|t| json!({"name": t.name, "schema": t.input_schema, "read_only": t.annotations.as_ref().and_then(|a| a.read_only_hint)}))
        .collect();
    let path = concat!(env!("CARGO_MANIFEST_DIR"), "/tests/catalog.snapshot.json");
    let pretty = serde_json::to_string_pretty(&snapshot).unwrap();
    if std::env::var_os("MCP_SNAPSHOT").is_some() {
        std::fs::write(path, &pretty).unwrap();
    }
    let want = std::fs::read_to_string(path)
        .expect("tests/catalog.snapshot.json (MCP_SNAPSHOT=1 writes it)");
    assert_eq!(
        pretty.trim(),
        want.trim(),
        "catalogue changed: review, then MCP_SNAPSHOT=1"
    );
    assert_eq!(tools.len(), 44);
    assert_eq!(
        tools
            .iter()
            .filter(|t| t.annotations.as_ref().and_then(|a| a.read_only_hint) == Some(true))
            .count(),
        25
    );
    for t in &tools {
        assert_eq!(
            t.input_schema.get("type"),
            Some(&json!("object")),
            "{}",
            t.name
        );
    }
}

/// Paging: `limit` default 50 and capped at 200, cursors chain, the last page has none;
/// transcript pages by segment index with `next`.
#[tokio::test]
async fn paging() {
    let fake = Arc::new(Fake::default());
    let dir = tempfile::tempdir().unwrap();
    let (a, b) = tokio::io::duplex(1 << 16);
    let (server, client) = tokio::join!(
        catalog(&fake, dir.path(), "stdio")
            .await
            .serve(tokio::io::split(a)),
        ().serve(tokio::io::split(b)),
    );
    let (server, client) = (server.unwrap(), client.unwrap());

    let p1 = json(
        &client
            .call_tool(req("meeting_list", json!({})))
            .await
            .unwrap(),
    );
    assert_eq!(p1["items"].as_array().unwrap().len(), 50);
    assert_eq!(p1["next_cursor"], "50");
    let p2 = json(
        &client
            .call_tool(req("meeting_list", json!({"cursor": "50", "limit": 1000})))
            .await
            .unwrap(),
    );
    // Asked for 1000, clamped to 200; the fake has 120 rows → 70 left, no next page.
    assert_eq!(p2["items"].as_array().unwrap().len(), 70);
    assert_eq!(p2["next_cursor"], Value::Null);
    let asked: Vec<u64> = fake
        .calls
        .lock()
        .iter()
        .filter(|c| c.0 == "meeting_list")
        .map(|c| c.1[0].as_u64().unwrap())
        .collect();
    assert_eq!(
        asked,
        [51, 251],
        "the RPC is asked for offset + limit + 1 rows"
    );

    let t = json(
        &client
            .call_tool(req("meeting_transcript", json!({"id": "m1"})))
            .await
            .unwrap(),
    );
    assert_eq!(t["segments"].as_array().unwrap().len(), 200);
    assert_eq!(t["next"], 200);
    let t2 = json(
        &client
            .call_tool(req(
                "meeting_transcript",
                json!({"id": "m1", "from_seg": 200}),
            ))
            .await
            .unwrap(),
    );
    assert_eq!(t2["segments"].as_array().unwrap().len(), 100);
    assert_eq!(t2["next"], Value::Null);

    // meeting_search unwraps the meetings group.
    let s = json(
        &client
            .call_tool(req("meeting_search", json!({"query": "x"})))
            .await
            .unwrap(),
    );
    assert_eq!(s["hits"][0]["meeting"]["id"], "m1");
    let search_call = fake
        .calls
        .lock()
        .iter()
        .find(|c| c.0 == "search")
        .cloned()
        .unwrap();
    assert_eq!(search_call.1[1], json!(["meetings"]));

    client.cancel().await.unwrap();
    server.waiting().await.unwrap();
}

/// 158 §12.5: Private sessions never list, never read, and their artifacts vanish.
#[tokio::test]
async fn private_sessions_are_excluded() {
    let fake = Arc::new(Fake::default());
    let dir = tempfile::tempdir().unwrap();
    let (a, b) = tokio::io::duplex(1 << 16);
    let (server, client) = tokio::join!(
        catalog(&fake, dir.path(), "stdio")
            .await
            .serve(tokio::io::split(a)),
        ().serve(tokio::io::split(b)),
    );
    let (server, client) = (server.unwrap(), client.unwrap());

    let l = json(
        &client
            .call_tool(req("session_list", json!({})))
            .await
            .unwrap(),
    );
    let ids: Vec<&str> = l["items"]
        .as_array()
        .unwrap()
        .iter()
        .map(|s| s["id"].as_str().unwrap())
        .collect();
    assert_eq!(ids, ["s-pub", "s-other"]);
    let l = json(
        &client
            .call_tool(req("session_list", json!({"project": "p1"})))
            .await
            .unwrap(),
    );
    assert_eq!(l["items"].as_array().unwrap().len(), 1);

    let r = client
        .call_tool(req("session_read", json!({"id": "s-priv"})))
        .await
        .unwrap();
    assert_eq!(r.is_error, Some(true));
    assert!(text(&r).contains("private"));
    let r = json(
        &client
            .call_tool(req("session_read", json!({"id": "s-pub", "limit": 2})))
            .await
            .unwrap(),
    );
    assert_eq!(r["items"].as_array().unwrap().len(), 2);
    assert_eq!(r["next_cursor"], "2");
    assert_eq!(r["project"], "p1");

    let a = json(
        &client
            .call_tool(req("artifact_list", json!({})))
            .await
            .unwrap(),
    );
    assert_eq!(a["items"].as_array().unwrap().len(), 1);
    assert_eq!(a["items"][0]["session"], "s-pub");

    // An engine error is the tool's error text; bad args are a protocol error.
    let r = client
        .call_tool(req("task_get", json!({"id": "t1"})))
        .await
        .unwrap();
    assert_eq!(r.is_error, Some(true));
    assert_eq!(text(&r), "no such method task_get");
    assert!(client.call_tool(req("task_get", json!({}))).await.is_err());
    assert!(client.call_tool(req("nope", json!({}))).await.is_err());

    client.cancel().await.unwrap();
    server.waiting().await.unwrap();
}

/// Raw HTTP/1.1 to the listener: the status line back.
async fn raw(port: u16, headers: &str) -> String {
    let mut s = tokio::net::TcpStream::connect(("127.0.0.1", port))
        .await
        .unwrap();
    let body = r#"{"jsonrpc":"2.0","id":1,"method":"initialize","params":{"protocolVersion":"2025-03-26","capabilities":{},"clientInfo":{"name":"raw","version":"0"}}}"#;
    let req = format!(
        "POST /mcp HTTP/1.1\r\n{headers}Content-Type: application/json\r\nAccept: application/json, text/event-stream\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
        body.len()
    );
    s.write_all(req.as_bytes()).await.unwrap();
    let mut out = Vec::new();
    let _ = s.read_to_end(&mut out).await;
    String::from_utf8_lossy(&out)
        .lines()
        .next()
        .unwrap_or_default()
        .to_owned()
}

/// A client that names itself, so the audit line carries `clientInfo.name`.
#[derive(Clone)]
struct Named;

impl ClientHandler for Named {
    fn get_info(&self) -> ClientConfig {
        ClientConfig::new(Default::default(), Implementation::new("test-client", "0"))
    }
}

/// 158 §12.2: Bearer (401), Host (403), Origin (403) rules on the real listener; then the round
/// trip with rmcp's own `StreamableHttpClientTransport`: `list_tools`, `meeting_list`, one write
/// tool; the audit file has one line per call with the client's name; cancel ends the listener.
#[tokio::test]
async fn http_rejections_and_round_trip() {
    let fake = Arc::new(Fake::default());
    let dir = tempfile::tempdir().unwrap();
    let cancel = CancellationToken::new();
    let port = mcp_server::http::serve(
        0,
        "tok".into(),
        catalog(&fake, dir.path(), "http").await,
        cancel.clone(),
    )
    .await
    .unwrap();
    let host = format!("Host: 127.0.0.1:{port}\r\n");

    assert!(raw(port, &host).await.contains("401"), "no bearer");
    assert!(
        raw(port, &format!("{host}Authorization: Bearer wrong\r\n"))
            .await
            .contains("401")
    );
    assert!(
        raw(port, &format!("{host}Authorization: Bearer tok\r\n"))
            .await
            .contains("200"),
        "good bearer"
    );
    assert!(
        raw(
            port,
            &format!("Host: evil.example:{port}\r\nAuthorization: Bearer tok\r\n")
        )
        .await
        .contains("403"),
        "foreign host"
    );
    assert!(
        raw(port, "Host: 127.0.0.1:1\r\nAuthorization: Bearer tok\r\n")
            .await
            .contains("403"),
        "wrong port"
    );
    assert!(
        raw(
            port,
            &format!("{host}Origin: http://evil.example\r\nAuthorization: Bearer tok\r\n")
        )
        .await
        .contains("403"),
        "foreign origin"
    );
    assert!(
        raw(
            port,
            &format!("{host}Origin: http://localhost:5173\r\nAuthorization: Bearer tok\r\n")
        )
        .await
        .contains("200"),
        "loopback origin, any port"
    );

    let mut headers = HashMap::new();
    headers.insert(
        "authorization".parse().unwrap(),
        "Bearer tok".parse().unwrap(),
    );
    let cfg = StreamableHttpClientTransportConfig::with_uri(format!("http://127.0.0.1:{port}/mcp"))
        .custom_headers(headers);
    let client = Named
        .serve(StreamableHttpClientTransport::from_config(cfg))
        .await
        .unwrap();
    assert!(
        client
            .peer_info()
            .unwrap()
            .instructions
            .as_deref()
            .unwrap()
            .contains("meeting_search")
    );

    let tools = client.list_all_tools().await.unwrap();
    assert_eq!(tools.len(), 44);
    assert_eq!(tools[0].name, "meeting_list");

    let l = json(
        &client
            .call_tool(req("meeting_list", json!({"limit": 3})))
            .await
            .unwrap(),
    );
    assert_eq!(l["items"].as_array().unwrap().len(), 3);
    assert_eq!(l["next_cursor"], "3");

    let r = client
        .call_tool(req("task_comment", json!({"id": "t1", "text": "hi"})))
        .await
        .unwrap();
    assert_ne!(r.is_error, Some(true));
    let calls = fake.calls.lock().clone();
    assert!(calls.contains(&("task_comment".into(), vec![json!("t1"), json!("hi")])));

    // §12.5: the vault's secret never crosses.
    let s = client
        .call_tool(req("meeting_summary", json!({"id": "m1"})))
        .await
        .unwrap();
    assert!(!text(&s).contains("hunter2-secret-value"), "{}", text(&s));
    assert!(text(&s).starts_with("# Summary\nthe api key is "));

    let audit = Audit::new(dir.path()).tail(10).unwrap();
    assert_eq!(audit.len(), 3);
    assert_eq!(audit[0].tool, "meeting_summary");
    assert_eq!(audit[2].tool, "meeting_list");
    assert!(
        audit
            .iter()
            .all(|l| l.transport == "http" && l.client == "test-client" && l.ok)
    );
    assert_eq!(audit[0].args_sha256.len(), 64);

    client.cancel().await.unwrap();
    cancel.cancel();
    tokio::time::sleep(std::time::Duration::from_millis(50)).await;
    assert!(
        tokio::net::TcpStream::connect(("127.0.0.1", port))
            .await
            .is_err(),
        "listener gone"
    );
}

/// 158 §12.3/§12.5: `t mcp` on stdio is a transport. Every `call_tool` is one `mcp_server_call`
/// on the engine, whose catalogue redacts with the vault and writes the audit line with
/// `transport: stdio` and the stdio client's name; the bytes match the HTTP path. Unknown tool
/// and bad arguments stay protocol errors; an engine refusal stays a tool error.
#[tokio::test]
async fn stdio_bridge_forwards_to_the_engine() {
    let fake = Arc::new(Fake::default());
    let dir = tempfile::tempdir().unwrap();
    *fake.catalog.lock() = Some(catalog(&fake, dir.path(), "stdio").await);
    let (a, b) = tokio::io::duplex(1 << 16);
    let (server, client) = tokio::join!(
        Bridge::new(fake.clone()).serve(tokio::io::split(a)),
        Named.serve(tokio::io::split(b)),
    );
    let (server, client) = (server.unwrap(), client.unwrap());
    assert_eq!(
        client
            .peer_info()
            .unwrap()
            .server_info
            .as_ref()
            .unwrap()
            .name,
        "theuth"
    );
    assert_eq!(client.list_all_tools().await.unwrap().len(), 44);

    let s = client
        .call_tool(req("meeting_summary", json!({"id": "m1"})))
        .await
        .unwrap();
    assert!(!text(&s).contains("hunter2-secret-value"), "{}", text(&s));
    assert!(text(&s).starts_with("# Summary\nthe api key is "));
    // The same call through the engine-side catalogue directly: identical bytes.
    let catalog = fake.catalog.lock().clone().unwrap();
    let direct: McpCallResult = catalog
        .call(
            "meeting_summary",
            json!({"id": "m1"}).as_object().unwrap().clone(),
            "x",
        )
        .await
        .unwrap();
    assert_eq!(direct.text, text(&s));

    let l = json(
        &client
            .call_tool(req("meeting_list", json!({"limit": 2})))
            .await
            .unwrap(),
    );
    assert_eq!(l["items"].as_array().unwrap().len(), 2);

    // Engine refusal → tool error; unknown tool / bad args → protocol error.
    let r = client
        .call_tool(req("task_get", json!({"id": "t1"})))
        .await
        .unwrap();
    assert_eq!(r.is_error, Some(true));
    assert_eq!(text(&r), "no such method task_get");
    assert!(client.call_tool(req("task_get", json!({}))).await.is_err());
    assert!(client.call_tool(req("nope", json!({}))).await.is_err());

    // Only `mcp_server_call` left the bridge; the real calls were made by the engine's catalogue.
    let calls = fake.calls.lock().clone();
    assert_eq!(calls.iter().filter(|c| c.0 == "mcp_server_call").count(), 5);
    assert_eq!(calls[0].1[2], json!("test-client"));
    // 5 through the bridge + the direct comparison call (client "x").
    let audit = Audit::new(dir.path()).tail(10).unwrap();
    assert_eq!(audit.len(), 6, "every call is audited: {audit:?}");
    assert!(audit.iter().all(|l| l.transport == "stdio"));
    assert_eq!(
        audit.iter().filter(|l| l.client == "test-client").count(),
        5
    );
    assert_eq!(audit[0].tool, "nope");
    assert!(!audit[0].ok);
    assert!(audit[5].ok);

    client.cancel().await.unwrap();
    server.waiting().await.unwrap();
}
