//! 209 P3, end to end in-process: an agent session attached to a tile drives it through the
//! `browser_*` tools, and a fake client app holding the tile runs every op of 209 §4.7 on its own
//! obscura page (the agent script `tile/agent.js` included) against a loopback fixture site. The
//! client answers through the real `answer_web_view_call` RPC dispatch (artifact's module, an
//! authenticated owner connection), as the app does. obscura has no isolated world (its
//! `browser_evaluate` runs in the page), so the agent script runs in the page world here; the
//! WebKit content world is exercised only by the shell on a device.

use std::io::{Read, Write};
use std::net::TcpListener;
use std::time::Duration;

use artifact::webview::{
    self, WebViewCallAnswer, WebViewCallId, WebViewCallRequested, WebViewOp, WebViewState,
};
use browser::{BrowserConfig, Browsers, Call};
use kameo::actor::{ActorRef, Spawn};
use kameo::message::{Context, Message};
use runtime::SessionId;
use serde_json::{Value, json};
use tools::{DynTool, ToolCx, ToolOutput};

const VIEW: &str = "0123456789abcdef";

const PAGE: &str = r#"<!doctype html><html><head><title>Tile fixture</title></head><body>
<h1>TILE_FIXTURE_ALPHA</h1>
<p>Hello from the tile. <a href="/next">next page</a></p>
<form method="post" action="/submitted">
<label for="q">Query</label><input id="q" name="q">
<input type="password" name="pw" aria-label="Password">
<select name="color" aria-label="Color"><option value="r">Red</option><option value="g">Green</option></select>
<button type="submit">Go</button>
</form>
<textarea aria-label="Note"></textarea>
<div contenteditable="true" aria-label="Draft"></div>
<div hidden><button>Hidden action</button></div>
<button id="counter" onclick="window.n = (window.n || 0) + 1; this.textContent = 'clicked ' + window.n">count</button>
</body></html>"#;

/// `GET /` → [`PAGE`], `GET /next` → a second page, `POST /submitted` → echo of `q` and `color`.
fn serve() -> u16 {
    let listener = TcpListener::bind("127.0.0.1:0").expect("bind");
    let port = listener.local_addr().expect("addr").port();
    std::thread::spawn(move || {
        for stream in listener.incoming() {
            let Ok(mut s) = stream else { continue };
            std::thread::spawn(move || {
                let mut buf = Vec::new();
                let mut chunk = [0u8; 4096];
                let head_end = loop {
                    let n = s.read(&mut chunk).unwrap_or(0);
                    if n == 0 {
                        return;
                    }
                    buf.extend_from_slice(&chunk[..n]);
                    if let Some(i) = buf.windows(4).position(|w| w == b"\r\n\r\n") {
                        break i + 4;
                    }
                };
                let head = String::from_utf8_lossy(&buf[..head_end]).into_owned();
                let mut lines = head.lines();
                let request = lines.next().unwrap_or_default().to_owned();
                let len = lines
                    .filter_map(|l| l.split_once(':'))
                    .find(|(k, _)| k.eq_ignore_ascii_case("content-length"))
                    .and_then(|(_, v)| v.trim().parse::<usize>().ok())
                    .unwrap_or(0);
                while buf.len() < head_end + len {
                    let n = s.read(&mut chunk).unwrap_or(0);
                    if n == 0 {
                        break;
                    }
                    buf.extend_from_slice(&chunk[..n]);
                }
                let body = String::from_utf8_lossy(&buf[head_end..]).into_owned();
                let field = |name: &str| {
                    body.split('&')
                        .filter_map(|kv| kv.split_once('='))
                        .find(|(k, _)| *k == name)
                        .map(|(_, v)| v.replace('+', " "))
                        .unwrap_or_default()
                };
                let (status, html) = if request.starts_with("POST /submitted") {
                    (
                        "200 OK",
                        format!(
                            "<html><head><title>Sent</title></head><body><p id=\"echo\">you sent: {} color {}</p></body></html>",
                            field("q"),
                            field("color")
                        ),
                    )
                } else if request.starts_with("GET /next ") {
                    (
                        "200 OK",
                        "<html><head><title>Next</title></head><body><p>TILE_NEXT_PAGE</p></body></html>"
                            .to_owned(),
                    )
                } else if request.starts_with("GET / ") {
                    ("200 OK", PAGE.to_owned())
                } else {
                    ("404 Not Found", "<html><body>nope</body></html>".to_owned())
                };
                let _ = write!(
                    s,
                    "HTTP/1.1 {status}\r\nContent-Type: text/html; charset=utf-8\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{html}",
                    html.len()
                );
                let _ = s.flush();
            });
        }
    });
    port
}

/// Text blocks of an MCP result and its `isError`.
fn text_of(v: &Value) -> (String, bool) {
    let text = v["content"]
        .as_array()
        .map(|b| {
            b.iter()
                .filter_map(|b| b["text"].as_str())
                .collect::<Vec<_>>()
                .join("\n")
        })
        .unwrap_or_default();
    (text, v["isError"] == true)
}

/// The fake client app: holds tile [`VIEW`] as its own obscura session `page`, claims every
/// call for it and runs the op there, as the shell's `web_view_call` does in the tile, answering
/// over `answer_web_view_call` on one owner connection.
#[derive(kameo::Actor)]
struct Client {
    browsers: ActorRef<Browsers>,
    page: SessionId,
    rpc: jsonrpsee::Methods,
    ops: std::sync::Arc<std::sync::Mutex<Vec<String>>>,
}

/// `answer_web_view_call(id, answer)` through the RPC dispatch.
async fn answer(rpc: &jsonrpsee::Methods, id: WebViewCallId, answer: WebViewCallAnswer) -> bool {
    rpc.call("answer_web_view_call", (id, answer))
        .await
        .expect("answer_web_view_call")
}

impl Message<WebViewCallRequested> for Client {
    type Reply = ();

    async fn handle(&mut self, r: WebViewCallRequested, _: &mut Context<Self, ()>) {
        if r.view != VIEW {
            return;
        }
        assert!(!r.delegated, "the tile's own session acts");
        let cmd = r.args["arguments"]["cmd"].as_str().unwrap_or_default();
        self.ops
            .lock()
            .unwrap()
            .push(format!("{} {cmd}", r.op.as_str()).trim().to_owned());
        let (browsers, page, rpc) = (self.browsers.clone(), self.page, self.rpc.clone());
        tokio::spawn(async move {
            let claim = WebViewCallAnswer::Claimed {
                client: "fake".into(),
            };
            assert!(answer(&rpc, r.id, claim).await, "claim refused");
            let done = run(&browsers, page, r.op, &r.args).await;
            assert!(answer(&rpc, r.id, done).await, "answer refused");
        });
    }
}

async fn obscura(
    b: &ActorRef<Browsers>,
    page: SessionId,
    name: &str,
    args: Value,
) -> Result<Value, String> {
    match b
        .ask(Call {
            session: page,
            name: name.to_owned(),
            args,
        })
        .await
    {
        Ok(v) => match text_of(&v) {
            (t, true) => Err(t),
            _ => Ok(v),
        },
        Err(e) => Err(e.to_string()),
    }
}

/// `source` run as the body of an async function with `arguments` as its parameters, its result
/// returned as JSON (callAsyncJavaScript's contract, 209 handoff).
async fn script(
    b: &ActorRef<Browsers>,
    page: SessionId,
    source: &str,
    arguments: &Value,
    timeout_ms: u64,
) -> Result<Value, String> {
    let map = arguments.as_object().cloned().unwrap_or_default();
    let names: Vec<&str> = map.keys().map(String::as_str).collect();
    let values: Vec<String> = map.values().map(|v| v.to_string()).collect();
    let expression = format!(
        "(async () => {{ const r = await (async ({}) => {{\n{source}\n}})({}); return JSON.stringify(r === undefined ? null : r); }})()",
        names.join(", "),
        values.join(", ")
    );
    let v = obscura(
        b,
        page,
        "browser_evaluate",
        json!({ "expression": expression, "timeoutMs": timeout_ms }),
    )
    .await?;
    serde_json::from_str(&text_of(&v).0).map_err(|e| format!("not JSON: {e}: {}", text_of(&v).0))
}

const WHERE: &str = "return { url: location.href, title: document.title, loading: false };";

async fn run(
    b: &ActorRef<Browsers>,
    page: SessionId,
    op: WebViewOp,
    args: &Value,
) -> WebViewCallAnswer {
    let done = |result| WebViewCallAnswer::Done { result, png: None };
    let failed = |reason| WebViewCallAnswer::Failed { reason };
    let ms = args["timeout_ms"].as_u64().unwrap_or(15_000);
    let out = match op {
        WebViewOp::Script => script(
            b,
            page,
            args["source"].as_str().unwrap_or_default(),
            &args["arguments"],
            ms,
        )
        .await
        .map(done),
        WebViewOp::Navigate | WebViewOp::Back => {
            let step = if op == WebViewOp::Navigate {
                obscura(b, page, "browser_navigate", json!({ "url": args["url"] })).await
            } else {
                obscura(b, page, "browser_back", json!({})).await
            };
            match step {
                Ok(_) => script(b, page, WHERE, &json!({}), ms).await.map(done),
                Err(e) => Err(e),
            }
        }
        WebViewOp::Snapshot => obscura(b, page, "browser_screenshot", json!({}))
            .await
            .and_then(|v| {
                let data = v["content"]
                    .as_array()
                    .and_then(|a| a.iter().find(|b| b["type"] == "image"))
                    .and_then(|b| b["data"].as_str())
                    .ok_or("no image")?
                    .to_owned();
                Ok(WebViewCallAnswer::Done {
                    result: json!({}),
                    png: Some(data),
                })
            }),
        other => Err(format!("op {} not in the fake client", other.as_str())),
    };
    out.unwrap_or_else(failed)
}

struct Kit {
    tools: Vec<DynTool>,
    session: SessionId,
    data_dir: std::path::PathBuf,
}

impl Kit {
    async fn call(&self, name: &str, args: Value) -> ToolOutput {
        let tool = self
            .tools
            .iter()
            .find(|t| t.spec().name == name)
            .unwrap_or_else(|| panic!("no tool {name}"));
        let cx = ToolCx {
            session: self.session,
            call_id: "c1".to_owned(),
            quiet: false,
            resources: tools::SessionResources::durable_arc(&self.data_dir),
            cancel: tools::CancellationToken::new(),
            promote_after: None,
        };
        tools::egress::scope(true, tool.call(args, cx))
            .await
            .expect(name)
    }

    /// The reply of a successful call, its tile prefix checked and removed.
    async fn ok(&self, name: &str, args: Value, prefix: &str) -> String {
        let out = self.call(name, args).await;
        assert!(!out.is_error, "{name} failed: {}", out.content);
        out.content
            .strip_prefix(prefix)
            .and_then(|c| c.strip_prefix('\n'))
            .unwrap_or_else(|| panic!("{name} reply lacks `{prefix}`: {}", out.content))
            .to_owned()
    }
}

/// The ref of the first listing line containing `needle`.
fn ref_of(listing: &str, needle: &str) -> String {
    listing
        .lines()
        .find(|l| l.starts_with("ref=") && l.contains(needle))
        .and_then(|l| l.split_whitespace().next())
        .and_then(|r| r.strip_prefix("ref="))
        .unwrap_or_else(|| panic!("no ref for {needle} in:\n{listing}"))
        .to_owned()
}

#[tokio::test(flavor = "multi_thread")]
async fn drives_a_tile_through_the_agent_script() {
    // SAFETY: first thing in the only test of this binary; nothing has read the environment yet.
    unsafe { std::env::set_var("OBSCURA_ALLOW_PRIVATE_NETWORK", "1") };
    let cfg = BrowserConfig {
        allow_private_network: true,
        stealth: true,
        max_chars: 4000,
        ..Default::default()
    };
    browser::warmup(&cfg);
    let port = serve();
    let base = format!("http://127.0.0.1:{port}/");
    let data_dir = tempfile::tempdir().expect("temp dir");
    let rt = runtime::Runtime::new(figment::Figment::new(), data_dir.path().to_path_buf());
    let browsers = Browsers::spawn(Browsers::new(cfg.clone()));

    // The user's tile shows the fixture page.
    let page = SessionId::new();
    obscura(&browsers, page, "browser_navigate", json!({ "url": base }))
        .await
        .expect("tile page");
    let ops = std::sync::Arc::new(std::sync::Mutex::new(Vec::new()));
    // The app's owner connection: artifact's RPC module with an authenticated peer.
    let mut rpc: jsonrpsee::Methods = artifact::rpc_module(rpc_core::Server::new(rt.clone()))
        .module
        .into();
    let (peer, _conn) = rpc_core::RpcPeer::open();
    rpc.extensions_mut().insert(peer);
    let client = Client::spawn(Client {
        browsers: browsers.clone(),
        page,
        rpc,
        ops: ops.clone(),
    });
    rt.subscribe::<Client, WebViewCallRequested>(&client).await;

    let kit = Kit {
        tools: browser::tools::all(browsers.clone(), cfg, rt.clone()),
        session: SessionId::new(),
        data_dir: data_dir.path().to_path_buf(),
    };
    let prefix = format!("[tile web:{VIEW} \u{b7} 127.0.0.1:{port}]");

    // Another session cannot attach to this session's tile (209 §9).
    let stranger = Kit {
        tools: kit.tools.clone(),
        session: SessionId::new(),
        data_dir: data_dir.path().to_path_buf(),
    };
    let refused = stranger
        .call("browser_attach", json!({ "view": format!("web:{VIEW}") }))
        .await;
    assert!(refused.is_error, "{}", refused.content);
    assert_eq!(
        refused.content,
        format!("tile web:{VIEW} was not opened by this session or its tree")
    );
    // The session opened the tile (its show_web; here the app's report).
    webview::record_state(WebViewState {
        session: kit.session,
        view: format!("web-{VIEW}"),
        url: base.clone(),
        title: "Tile fixture".into(),
        loading: false,
        closed: false,
    });

    // Attach: the probe reads where the tile is.
    let attached = kit
        .call("browser_attach", json!({ "view": format!("web:{VIEW}") }))
        .await;
    assert!(!attached.is_error, "attach: {}", attached.content);
    assert!(
        attached.content.starts_with(&prefix),
        "{}",
        attached.content
    );
    assert!(
        attached.content.contains("\"Tile fixture\""),
        "{}",
        attached.content
    );

    // Read: text and refs, hidden controls left out.
    let read = kit.ok("browser_read", json!({}), &prefix).await;
    assert!(
        read.starts_with(&format!("URL: {base}\nTitle: Tile fixture")),
        "{read}"
    );
    assert!(read.contains("TILE_FIXTURE_ALPHA"), "{read}");
    assert!(
        read.contains("ref=e") && read.contains("\"Query\""),
        "{read}"
    );
    assert!(
        !read.contains("Hidden action"),
        "hidden content listed: {read}"
    );

    let elements = kit.ok("browser_elements", json!({}), &prefix).await;
    let query = ref_of(&elements, "\"Query\"");
    let password = ref_of(&elements, "\"Password\"");
    let counter = ref_of(&elements, "\"count\"");
    assert!(elements.contains("role=textbox"), "{elements}");

    // Fill by ref (native setter + input/change); a password's value is never read back.
    let filled = kit
        .ok(
            "browser_fill",
            json!({ "ref": query, "value": "hello tile" }),
            &prefix,
        )
        .await;
    assert_eq!(filled, "Filled input \"Query\"");
    kit.ok(
        "browser_fill",
        json!({ "ref": password, "value": "Secret-Pass-123" }),
        &prefix,
    )
    .await;
    // What the user typed elsewhere on the page is not read back: no value, no body text.
    kit.ok(
        "browser_eval",
        json!({ "expression": "(document.querySelector('textarea').value = 'USER_DRAFT_SECRET', document.querySelector('[contenteditable]').textContent = 'USER_EDITOR_SECRET', 1);" }),
        &prefix,
    )
    .await;
    let listing = kit.ok("browser_elements", json!({}), &prefix).await;
    assert!(listing.contains("value=\"hello tile\""), "{listing}");
    assert!(
        !listing.contains("Secret-Pass-123"),
        "password value listed: {listing}"
    );
    // The same element keeps its ref across listings.
    assert_eq!(ref_of(&listing, "\"Query\""), query);
    let read = kit.ok("browser_read", json!({}), &prefix).await;
    let markdown = kit
        .ok("browser_read", json!({ "format": "markdown" }), &prefix)
        .await;
    for text in [&listing, &read, &markdown] {
        assert!(
            !text.contains("USER_DRAFT_SECRET") && !text.contains("USER_EDITOR_SECRET"),
            "user input read back: {text}"
        );
    }
    assert!(
        ref_of(&listing, "\"Note\"") != query && listing.contains("value=[set]"),
        "{listing}"
    );

    let selected = kit
        .ok(
            "browser_select",
            json!({ "selector": "select[name=color]", "value": "Green" }),
            &prefix,
        )
        .await;
    assert_eq!(selected, "Selected \"Green\" in select \"Color\"");

    // Click: events plus click(), then the page read back.
    let clicked = kit
        .ok("browser_click", json!({ "ref": counter }), &prefix)
        .await;
    assert!(clicked.starts_with("Clicked button \"count\""), "{clicked}");
    assert!(
        clicked.contains("clicked 1"),
        "click result not read back: {clicked}"
    );

    // browser_eval runs in the page world.
    let n = kit
        .ok("browser_eval", json!({ "expression": "window.n" }), &prefix)
        .await;
    assert_eq!(n, "1");

    // Enter in a form submits it (requestSubmit), then the next page is waited for and read.
    let pressed = kit
        .ok(
            "browser_press",
            json!({ "key": "Enter", "selector": "#q" }),
            &prefix,
        )
        .await;
    assert!(
        pressed.contains("Enter submitted the form on this tile"),
        "{pressed}"
    );
    kit.ok(
        "browser_wait",
        json!({ "text": "you sent", "timeout": 10 }),
        &prefix,
    )
    .await;
    let sent = kit.ok("browser_read", json!({}), &prefix).await;
    assert!(sent.contains("you sent: hello tile color g"), "{sent}");

    // Refs of the old page are gone.
    let stale = kit.call("browser_click", json!({ "ref": query })).await;
    assert!(stale.is_error, "{}", stale.content);
    assert!(
        stale.content.contains("ref ") && stale.content.contains("browser_elements"),
        "{}",
        stale.content
    );

    // Navigation: open, back.
    let opened = kit
        .ok(
            "browser_open",
            json!({ "url": format!("{base}next") }),
            &prefix,
        )
        .await;
    assert!(
        opened.starts_with(&format!("Navigated to {base}next")),
        "{opened}"
    );
    assert!(opened.contains("TILE_NEXT_PAGE"), "{opened}");
    let back = kit.ok("browser_back", json!({}), &prefix).await;
    assert!(back.starts_with("Went back to"), "{back}");

    // Screenshot: the snapshot op's PNG saved and attached.
    let shot = kit.call("browser_screenshot", json!({})).await;
    assert!(!shot.is_error, "screenshot: {}", shot.content);
    assert_eq!(shot.files.len(), 1);
    assert_eq!(
        &std::fs::read(&shot.files[0]).expect("png")[..8],
        b"\x89PNG\r\n\x1a\n"
    );

    // Close detaches; the tile's page stays.
    let closed = kit.ok("browser_close", json!({}), &prefix).await;
    assert!(closed.starts_with("Detached from tile web:"), "{closed}");

    let ops = ops.lock().unwrap().clone();
    for op in [
        "script location",
        "script read",
        "script elements",
        "script fill",
        "script select",
        "script click",
        "script press",
        "script wait",
        "navigate",
        "back",
        "snapshot",
        "script",
    ] {
        assert!(ops.iter().any(|o| o == op), "op {op} never ran: {ops:?}");
    }
    let _ = tokio::time::timeout(
        Duration::from_secs(5),
        browsers.ask(browser::Close { session: page }),
    )
    .await;
}
