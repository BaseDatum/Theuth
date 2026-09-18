//! In-process integration test (design doc 23 §11): a loopback fixture
//! server, one isolate, the `browser_*` tools driven through `ErasedTool`.
//! No network beyond 127.0.0.1.

use std::io::{Read, Write};
use std::net::TcpListener;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use base64::Engine as _;
use browser::{BrowserConfig, Browsers, Call};
use kameo::actor::Spawn;
use kameo::message::{Context, Message};
use runtime::SessionId;
use serde_json::{Value, json};
use tools::{ActivitySink, Admission, Admit, DynTool, Kind, Settle, ToolCx, ToolOutput, Tracked};

/// Activity supervisor double (47 §4): admits everything, records settlements.
#[derive(kameo::Actor)]
struct Ledger {
    settled: Arc<Mutex<Vec<Settle>>>,
    changed: Arc<tokio::sync::Notify>,
}

impl Message<Admit> for Ledger {
    type Reply = Admission;

    async fn handle(&mut self, _: Admit, _: &mut Context<Self, Self::Reply>) -> Admission {
        Admission::Admitted
    }
}

impl Message<Settle> for Ledger {
    type Reply = ();

    async fn handle(&mut self, m: Settle, _: &mut Context<Self, Self::Reply>) {
        self.settled.lock().expect("ledger").push(m);
        self.changed.notify_one();
    }
}

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
            quiet: false,
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

    let rt = runtime::Runtime::new(figment::Figment::new(), data_dir.clone());
    let actor = Browsers::spawn(Browsers::new(cfg.clone()));
    let kit = Kit {
        tools: browser::tools::all(actor.clone(), cfg, rt.clone()),
        session: SessionId::new(),
    };

    // A viewer polling an unknown session gets nothing and spawns nothing.
    let none = actor
        .ask(browser::Screenshot {
            session: SessionId::new(),
        })
        .await
        .expect("ask");
    assert!(none.is_none(), "screenshot of unknown session");

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

    let live = actor
        .ask(browser::Screenshot {
            session: kit.session,
        })
        .await
        .expect("ask")
        .expect("live screenshot of an open page");
    let live = base64::engine::general_purpose::STANDARD
        .decode(live)
        .expect("base64");
    assert_eq!(&live[..8], b"\x89PNG\r\n\x1a\n");

    kit.ok("browser_show", json!({})).await;

    // 47 §4.2 rule 5: a caller that times out cannot free the outstanding
    // command. The `Tracked<Call>` guard stays with the isolate's work and
    // settles only once the wait actually ends, error and all.
    let settled = Arc::new(Mutex::new(Vec::new()));
    let changed = Arc::new(tokio::sync::Notify::new());
    let ledger = Ledger::spawn(Ledger {
        settled: Arc::clone(&settled),
        changed: Arc::clone(&changed),
    });
    let activity = ActivitySink::new(ledger)
        .admit(kit.session, Kind::Command("browser"))
        .await
        .expect("admit");
    let op = activity.id();
    let pending = actor.ask(Tracked {
        message: Call {
            session: kit.session,
            name: "browser_wait_for_text".to_owned(),
            args: json!({ "text": "WTH_NEVER_APPEARS", "timeout": 2 }),
        },
        activity: Some(activity),
    });
    assert!(
        tokio::time::timeout(Duration::from_millis(200), pending)
            .await
            .is_err(),
        "caller should have timed out while the wait runs"
    );
    tokio::time::sleep(Duration::from_millis(500)).await;
    assert!(
        settled.lock().expect("ledger").is_empty(),
        "settled while the isolate still waits: {:?}",
        settled.lock().expect("ledger")
    );
    tokio::time::timeout(Duration::from_secs(15), async {
        while settled.lock().expect("ledger").is_empty() {
            changed.notified().await;
        }
    })
    .await
    .expect("settlement once the wait ended");
    assert!(
        matches!(settled.lock().expect("ledger").as_slice(), [Settle::Finished { op: o }] if *o == op),
        "unexpected settlement: {:?}",
        settled.lock().expect("ledger")
    );
    // The isolate is intact and serves the next command.
    let read = kit.ok("browser_read", json!({})).await;
    assert!(
        read.contains("you sent: hello wth"),
        "read after wait: {read}"
    );

    kit.ok("browser_close", json!({})).await;

    let closed = actor
        .ask(browser::Screenshot {
            session: kit.session,
        })
        .await
        .expect("ask");
    assert!(closed.is_none(), "screenshot after close: {closed:?}");

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
