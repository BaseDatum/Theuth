//! In-process integration test (design doc 23 §11): a loopback fixture
//! server, one isolate, the `browser_*` tools driven through `ErasedTool`.
//! No network beyond 127.0.0.1.

use std::io::{Read, Write};
use std::net::TcpListener;
use std::time::Instant;

use browser::{BrowserConfig, Browsers};
use kameo::actor::Spawn;
use runtime::SessionId;
use serde_json::{Value, json};
use tools::{DynTool, ToolCx, ToolOutput};

const PAGE: &str = r#"<!doctype html><html><head><title>Fixture</title></head><body>
<h1>WTH_FIXTURE_ALPHA</h1>
<p>Hello from the fixture.</p>
<form method="post" action="/submitted"><input name="q"><button type="submit">Go</button></form>
</body></html>"#;

/// Minimal HTTP/1.1 server: `GET /` → [`PAGE`], `POST /submitted` → echo of `q`.
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
                let (status, html) = if request.starts_with("POST /submitted") {
                    let q = body
                        .split('&')
                        .filter_map(|kv| kv.split_once('='))
                        .find(|(k, _)| *k == "q")
                        .map(|(_, v)| urldecode(v))
                        .unwrap_or_default();
                    (
                        "200 OK",
                        format!("<html><body><p id=\"echo\">you sent: {q}</p></body></html>"),
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

fn urldecode(s: &str) -> String {
    let bytes = s.as_bytes();
    let mut out = Vec::with_capacity(bytes.len());
    let mut i = 0;
    while i < bytes.len() {
        match bytes[i] {
            b'+' => out.push(b' '),
            b'%' if i + 2 < bytes.len() => {
                let hex = std::str::from_utf8(&bytes[i + 1..i + 3]).unwrap_or("");
                match u8::from_str_radix(hex, 16) {
                    Ok(b) => {
                        out.push(b);
                        i += 2;
                    }
                    Err(_) => out.push(b'%'),
                }
            }
            b => out.push(b),
        }
        i += 1;
    }
    String::from_utf8_lossy(&out).into_owned()
}

struct Kit {
    tools: Vec<DynTool>,
    session: SessionId,
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
        };
        tool.call(args, cx).await.expect(name)
    }

    async fn ok(&self, name: &str, args: Value) -> String {
        let out = self.call(name, args).await;
        assert!(!out.is_error, "{name} failed: {}", out.content);
        out.content
    }
}

#[tokio::test(flavor = "multi_thread")]
async fn drives_fixture_page() {
    // SAFETY: first thing in the only test of this binary; nothing has read
    // the environment yet.
    unsafe { std::env::set_var("OBSCURA_ALLOW_PRIVATE_NETWORK", "1") };
    let cfg = BrowserConfig {
        allow_private_network: true,
        stealth: true,
        max_chars: 4000,
        ..Default::default()
    };
    browser::warmup(&cfg);

    let started = Instant::now();
    let port = serve();
    let data_dir = std::env::temp_dir().join(format!("wth-browser-live-{}", uuid_ish()));
    std::fs::create_dir_all(&data_dir).expect("temp dir");

    let actor = Browsers::spawn(Browsers::new(cfg.clone()));
    let kit = Kit {
        tools: browser::tools::all(actor, cfg, data_dir.clone()),
        session: SessionId::new(),
    };

    let open = kit
        .ok(
            "browser_open",
            json!({ "url": format!("http://127.0.0.1:{port}/") }),
        )
        .await;
    assert!(open.contains("WTH_FIXTURE_ALPHA"), "open reply: {open}");

    let elements = kit.ok("browser_elements", json!({})).await;
    assert!(elements.contains("ref=e"), "elements: {elements}");
    assert!(elements.contains("input"), "elements: {elements}");

    kit.ok(
        "browser_fill",
        json!({ "selector": "input[name=q]", "value": "hello wth" }),
    )
    .await;

    let clicked = kit
        .ok(
            "browser_click",
            json!({ "selector": "button[type=submit]" }),
        )
        .await;
    let echoed = if clicked.contains("you sent: hello wth") {
        clicked
    } else {
        eprintln!("click reply lacked the echo, waiting:\n{clicked}");
        kit.ok("browser_wait", json!({ "text": "you sent", "timeout": 10 }))
            .await;
        kit.ok("browser_read", json!({})).await
    };
    assert!(
        echoed.contains("you sent: hello wth"),
        "after submit: {echoed}"
    );

    let shot = kit.call("browser_screenshot", json!({})).await;
    assert!(!shot.is_error, "screenshot: {}", shot.content);
    assert_eq!(shot.files.len(), 1, "files: {:?}", shot.files);
    let png = std::fs::read(&shot.files[0]).expect("read png");
    assert_eq!(&png[..8], b"\x89PNG\r\n\x1a\n");
    let w = u32::from_be_bytes([png[16], png[17], png[18], png[19]]);
    let h = u32::from_be_bytes([png[20], png[21], png[22], png[23]]);
    assert_eq!(
        shot.content,
        format!(
            "screenshot {w}\u{d7}{h} saved to {}",
            shot.files[0].display()
        ),
        "size line"
    );
    assert!(shot.files[0].starts_with(&data_dir));

    kit.ok("browser_close", json!({})).await;

    let dead = kit
        .call("browser_open", json!({ "url": "http://127.0.0.1:1/" }))
        .await;
    assert!(dead.is_error, "expected error, got: {}", dead.content);
    assert!(!dead.content.is_empty());

    eprintln!("live test body took {:?}", started.elapsed());
    let _ = std::fs::remove_dir_all(&data_dir);
}

fn uuid_ish() -> String {
    format!(
        "{}-{}",
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_nanos())
            .unwrap_or(0)
    )
}
