//! `http_request` (design doc 46 §5) end to end against wiremock on loopback:
//! the request that reaches the server, the structured reply, non-2xx and a
//! declared-JSON reply that does not parse as error results with data, the
//! reply cap on a streamed body, and the deadline.

use std::collections::BTreeMap;
use std::sync::Arc;
use std::time::Duration;

use runtime::SessionId;
use serde_json::{Value, json};
use tools::{Tool, ToolCx, ToolOutput};
use web::fetch::{CONNECT_TIMEOUT, Fetcher};
use web::http::{HttpRequestArgs, HttpRequestTool, HttpResponse, MAX_BYTES, TIMEOUT};
use wiremock::matchers::{body_json, header, method, path};
use wiremock::{Mock, MockServer, ResponseTemplate};

fn tool(allow: Vec<std::net::SocketAddr>, deadline: Duration) -> HttpRequestTool {
    HttpRequestTool {
        fetcher: Arc::new(
            Fetcher::new_with_timeouts("theuth-test", MAX_BYTES, 5, TIMEOUT, CONNECT_TIMEOUT)
                .expect("client")
                .allowing(allow),
        ),
        deadline,
    }
}

fn cx() -> ToolCx {
    ToolCx {
        session: SessionId::new(),
        call_id: "c".into(),
        quiet: false,
        resources: tools::SessionResources::durable_arc(&std::env::temp_dir()),
    }
}

fn args(
    method: &str,
    url: String,
    headers: &[(&str, &str)],
    body: Option<Value>,
) -> HttpRequestArgs {
    HttpRequestArgs {
        url,
        method: method.into(),
        headers: (!headers.is_empty()).then(|| {
            headers
                .iter()
                .map(|(k, v)| ((*k).to_owned(), (*v).to_owned()))
                .collect::<BTreeMap<_, _>>()
        }),
        body,
    }
}

fn structured(out: &ToolOutput) -> HttpResponse {
    serde_json::from_value(out.structured.clone().expect("structured")).expect("shape")
}

#[tokio::test]
async fn json_post_round_trips_as_data() {
    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .and(path("/v1/items"))
        .and(header("content-type", "application/json"))
        .and(header("authorization", "Bearer tok"))
        .and(body_json(json!({"name": "x"})))
        .respond_with(
            ResponseTemplate::new(201)
                .set_body_raw(r#"{"id":7,"name":"x"}"#, "application/json; charset=utf-8"),
        )
        .expect(1)
        .mount(&server)
        .await;
    let out = tool(vec![*server.address()], TIMEOUT)
        .call(
            args(
                "post",
                format!("{}/v1/items", server.uri()),
                &[("Authorization", "Bearer tok")],
                Some(json!({"name": "x"})),
            ),
            cx(),
        )
        .await
        .expect("call");
    assert!(!out.is_error, "{}", out.content);
    let r = structured(&out);
    assert_eq!(r.status, 201);
    assert_eq!(r.body, json!({"id": 7, "name": "x"}));
    assert_eq!(r.url, format!("{}/v1/items", server.uri()));
    assert!(!r.truncated);
    assert_eq!(
        r.headers.get("content-type").map(String::as_str),
        Some("application/json; charset=utf-8")
    );
}

#[tokio::test]
async fn non_success_is_error_result_with_status_and_body() {
    let server = MockServer::start().await;
    Mock::given(method("DELETE"))
        .and(path("/v1/items/7"))
        .respond_with(
            ResponseTemplate::new(409).set_body_raw(r#"{"error":"in use"}"#, "application/json"),
        )
        .mount(&server)
        .await;
    let out = tool(vec![*server.address()], TIMEOUT)
        .call(
            args("DELETE", format!("{}/v1/items/7", server.uri()), &[], None),
            cx(),
        )
        .await
        .expect("call");
    assert!(out.is_error);
    let r = structured(&out);
    assert_eq!(r.status, 409);
    assert_eq!(r.body, json!({"error": "in use"}));
}

#[tokio::test]
async fn malformed_declared_json_is_error_result_carrying_the_text() {
    let server = MockServer::start().await;
    Mock::given(method("GET"))
        .and(path("/v1/broken"))
        .respond_with(ResponseTemplate::new(200).set_body_raw(r#"{"id":7,"#, "application/json"))
        .mount(&server)
        .await;
    let out = tool(vec![*server.address()], TIMEOUT)
        .call(
            args("GET", format!("{}/v1/broken", server.uri()), &[], None),
            cx(),
        )
        .await
        .expect("call");
    assert!(
        out.is_error,
        "2xx with unparseable declared JSON must not read as success"
    );
    let r = structured(&out);
    assert_eq!(r.status, 200);
    assert!(!r.truncated);
    assert_eq!(
        r.body,
        json!(r#"{"id":7,"#),
        "body stays the raw text, never a guessed value"
    );
}

#[tokio::test]
async fn argument_errors_never_reach_the_network() {
    let server = MockServer::start().await;
    Mock::given(method("GET"))
        .respond_with(ResponseTemplate::new(200))
        .expect(0)
        .mount(&server)
        .await;
    let t = tool(vec![*server.address()], TIMEOUT);
    let bad = [
        args("FETCH", server.uri(), &[], None),
        args("GET", server.uri(), &[], Some(json!({"a": 1}))),
        args("GET", server.uri(), &[("Host", "evil.example")], None),
        args(
            "GET",
            format!("http://user:pw@{}/", server.address()),
            &[],
            None,
        ),
        args("GET", "ftp://example.org/".into(), &[], None),
    ];
    for a in bad {
        let out = t.call(a, cx()).await.expect("call");
        assert!(out.is_error, "{}", out.content);
        assert!(out.structured.is_none(), "{}", out.content);
    }
}

/// wiremock always sends `Content-Length`, which the fetcher refuses up front;
/// the streaming cap needs a chunked body, so serve one from a std socket.
fn chunked_json_server(total: usize) -> std::net::SocketAddr {
    use std::io::{Read, Write};
    let listener = std::net::TcpListener::bind("127.0.0.1:0").expect("bind");
    let addr = listener.local_addr().expect("addr");
    std::thread::spawn(move || {
        let (mut sock, _) = listener.accept().expect("accept");
        let mut head = [0u8; 4096];
        let _ = sock.read(&mut head);
        let mut out = b"HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nTransfer-Encoding: chunked\r\nConnection: close\r\n\r\n".to_vec();
        out.extend_from_slice(format!("{total:x}\r\n").as_bytes());
        out.extend_from_slice(b"[");
        out.extend(std::iter::repeat_n(b'1', total - 2));
        out.extend_from_slice(b"]\r\n0\r\n\r\n");
        sock.write_all(&out).expect("write");
    });
    addr
}

#[tokio::test]
async fn streamed_reply_over_the_cap_is_cut_flagged_and_not_parsed() {
    let addr = chunked_json_server(MAX_BYTES + 4096);
    let out = tool(vec![addr], TIMEOUT)
        .call(args("GET", format!("http://{addr}/big"), &[], None), cx())
        .await
        .expect("call");
    assert!(out.is_error);
    let r = structured(&out);
    assert!(r.truncated);
    assert_eq!(r.status, 200);
    assert!(r.body.is_string(), "cut JSON must stay text: {}", r.body);
    assert!(r.body.as_str().map(str::len).unwrap_or(0) <= MAX_BYTES);
}

#[tokio::test]
async fn deadline_covers_the_whole_call() {
    let server = MockServer::start().await;
    Mock::given(method("GET"))
        .and(path("/slow"))
        .respond_with(ResponseTemplate::new(200).set_delay(Duration::from_secs(5)))
        .mount(&server)
        .await;
    let out = tool(vec![*server.address()], Duration::from_millis(200))
        .call(
            args("GET", format!("{}/slow", server.uri()), &[], None),
            cx(),
        )
        .await
        .expect("call");
    assert!(out.is_error);
    assert!(out.structured.is_none());
}

#[tokio::test]
async fn cross_origin_redirect_never_carries_a_custom_header() {
    let api = MockServer::start().await;
    let other = MockServer::start().await;
    Mock::given(method("GET"))
        .and(path("/v1/me"))
        .respond_with(
            ResponseTemplate::new(302).insert_header("Location", format!("{}/steal", other.uri())),
        )
        .mount(&api)
        .await;
    Mock::given(method("GET"))
        .and(path("/steal"))
        .respond_with(ResponseTemplate::new(200).set_body_string("got it"))
        .expect(0)
        .mount(&other)
        .await;
    let out = tool(vec![*api.address(), *other.address()], TIMEOUT)
        .call(
            args(
                "GET",
                format!("{}/v1/me", api.uri()),
                &[("X-Api-Key", "expanded-secret")],
                None,
            ),
            cx(),
        )
        .await
        .expect("call");
    assert!(out.is_error, "{}", out.content);
    assert!(out.structured.is_none(), "{}", out.content);
}
