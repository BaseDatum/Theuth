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
use tools::{
    ActivitySink, Admission, Admit, DynTool, Kind, Notice, Settle, ToolCx, ToolOutput, Tracked,
};

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

impl Message<Notice> for Ledger {
    type Reply = ();

    async fn handle(&mut self, _: Notice, _: &mut Context<Self, Self::Reply>) {}
}

const PAGE: &str = r#"<!doctype html><html><head><title>Fixture</title></head><body>
<h1>THEUTH_FIXTURE_ALPHA</h1>
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
        // The owner's call (16 §5): only a call with the local-network grant gets the browser.
        tools::egress::scope(true, tool.call(args, cx))
            .await
            .expect(name)
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
    let data_dir = std::env::temp_dir().join(format!("theuth-browser-live-{}", uuid_ish()));
    std::fs::create_dir_all(&data_dir).expect("temp dir");

    let rt = runtime::Runtime::new(figment::Figment::new(), data_dir.clone());
    let actor = Browsers::spawn(Browsers::new(cfg.clone()));
    let kit = Kit {
        tools: browser::tools::all(actor.clone(), cfg, rt.clone()),
        session: SessionId::new(),
        data_dir: data_dir.clone(),
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
    assert!(open.contains("THEUTH_FIXTURE_ALPHA"), "open reply: {open}");

    let elements = kit.ok("browser_elements", json!({})).await;
    assert!(elements.contains("ref=e"), "elements: {elements}");
    assert!(elements.contains("input"), "elements: {elements}");

    kit.ok(
        "browser_fill",
        json!({ "selector": "input[name=q]", "value": "hello theuth" }),
    )
    .await;

    let clicked = kit
        .ok(
            "browser_click",
            json!({ "selector": "button[type=submit]" }),
        )
        .await;
    let echoed = if clicked.contains("you sent: hello theuth") {
        clicked
    } else {
        eprintln!("click reply lacked the echo, waiting:\n{clicked}");
        kit.ok("browser_wait", json!({ "text": "you sent", "timeout": 10 }))
            .await;
        kit.ok("browser_read", json!({})).await
    };
    assert!(
        echoed.contains("you sent: hello theuth"),
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
            args: json!({ "text": "THEUTH_NEVER_APPEARS", "timeout": 2 }),
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
        read.contains("you sent: hello theuth"),
        "read after wait: {read}"
    );

    // 57 §4.1: `browser_run` on the fixture with a scripted oracle standing in for the browser
    // head — active mode types the quoted value, submits, sees the echo and says DONE; shadow
    // mode decides one step, hands back, and the model's own next click labels it.
    let run_tool = kit
        .tools
        .iter()
        .find(|t| t.spec().name == "browser_run")
        .expect("browser_run registered (decision.families.browser defaults to shadow)");
    assert_eq!(run_tool.spec().risk, tools::Risk::Mutating);
    assert!(run_tool.spec().description.contains("up to `max_steps`"));
    let engine = scripted_engine(&data_dir);
    kit.ok(
        "browser_open",
        json!({ "url": format!("http://127.0.0.1:{port}/") }),
    )
    .await;
    let run = kit
        .call(
            "browser_run",
            json!({ "goal": "Type 'hello loop' into the field and submit it with Go" }),
        )
        .await;
    assert!(!run.is_error, "browser_run: {}", run.content);
    let lines: Vec<&str> = run.content.lines().collect();
    assert!(
        lines[0].starts_with("browser_run (active): 3 step(s)"),
        "{}",
        run.content
    );
    assert!(
        lines[1]
            .starts_with("step 1: TYPE_TEXT 0.90 [1] \"textbox\" (textbox) 1.00 = 'hello loop'"),
        "{}",
        run.content
    );
    assert!(
        lines[2].starts_with("step 2: CLICK 0.90 [2] \"Go\" (button) 0.90"),
        "{}",
        run.content
    );
    assert!(lines[3].starts_with("step 3: DONE 0.90"), "{}", run.content);
    assert!(run.content.contains("stopped: DONE"), "{}", run.content);
    assert!(run.content.contains("/submitted"), "{}", run.content);
    assert!(
        !run.content.contains("Continue with the browser_* tools"),
        "{}",
        run.content
    );
    let read = kit.ok("browser_read", json!({})).await;
    assert!(
        read.contains("you sent: hello loop"),
        "after browser_run: {read}"
    );

    // Shadow: one decision, nothing executed, the hand-back carries the element listing.
    engine
        .ask(decision::SetMode {
            prefix: "browser".into(),
            mode: decision::Mode::Shadow,
        })
        .await
        .expect("set mode");
    kit.ok(
        "browser_open",
        json!({ "url": format!("http://127.0.0.1:{port}/") }),
    )
    .await;
    let shadow = kit
        .call(
            "browser_run",
            json!({ "goal": "Type 'hello loop' into the field and submit it with Go" }),
        )
        .await;
    assert!(!shadow.is_error, "{}", shadow.content);
    assert!(
        shadow
            .content
            .starts_with("browser_run (shadow): 1 step(s)"),
        "{}",
        shadow.content
    );
    assert!(
        shadow.content.contains("step 1: TYPE_TEXT"),
        "{}",
        shadow.content
    );
    assert!(
        shadow.content.contains("stopped: shadow mode"),
        "{}",
        shadow.content
    );
    assert!(
        shadow.content.contains("Continue with the browser_* tools"),
        "{}",
        shadow.content
    );
    assert!(shadow.content.contains("ref=e1"), "{}", shadow.content);
    let read = kit.ok("browser_read", json!({})).await;
    assert!(!read.contains("you sent"), "shadow must not act: {read}");
    // The model disagrees (clicks Go instead of typing): both shadowed rows get `corrected`.
    kit.ok("browser_elements", json!({})).await; // a read settles nothing
    kit.call("browser_click", json!({ "ref": "e2" })).await;
    tokio::time::sleep(Duration::from_millis(300)).await;
    let stats = engine
        .ask(decision::Stats {
            family: "browser.operation".into(),
        })
        .await
        .expect("stats");
    assert_eq!(stats.asks, 4, "{stats:?}");
    assert_eq!(stats.agreement, Some(0.0), "{stats:?}");
    let stats = engine
        .ask(decision::Stats {
            family: "browser.type_target".into(),
        })
        .await
        .expect("stats");
    assert_eq!(stats.agreement, Some(0.0), "{stats:?}");

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

/// A stand-in for the browser head: step 1 `TYPE_TEXT` into the first editable, step 2
/// `CLICK` the `(button)`, step 3 `DONE`, cyclic. The operation question (the one offering
/// `DONE`) opens a step — counting there, not in `run`, keeps the script independent of
/// `decision.batch_forward` (one forward per batch or per row). `build` records the pick in
/// its own [`Pick`] row (it sees the question; `run` sees rows), P 0.9 on the pick.
struct Scripted {
    step: Arc<std::sync::atomic::AtomicUsize>,
}

/// The scripted row: which of `k` candidates wins.
struct Pick {
    pick: usize,
    k: usize,
    ids: [i64; 1],
}

impl decision::Row for Pick {
    fn truncated(&self) -> bool {
        false
    }
    fn state_cut(&self) -> bool {
        false
    }
    fn markers_lost(&self) -> bool {
        false
    }
    fn ids(&self) -> &[i64] {
        &self.ids
    }
    fn as_any(&self) -> &dyn std::any::Any {
        self
    }
}

impl decision::Oracle for Scripted {
    fn model_sha(&self) -> decision::engine::ModelSha {
        Arc::new(parking_lot::Mutex::new("scripted".into()))
    }
    fn build(&self, q: &decision::Question) -> Result<Box<dyn decision::Row>, String> {
        let decision::Question::Choice { candidates, .. } = q else {
            return Err("choice only".into());
        };
        let ids: Vec<&str> = candidates.iter().map(|c| c.id.as_str()).collect();
        let pick = if ids.contains(&"DONE") {
            let step = self.step.fetch_add(1, std::sync::atomic::Ordering::SeqCst) % 3;
            let want = ["TYPE_TEXT", "CLICK", "DONE"][step];
            ids.iter().position(|i| *i == want).unwrap_or(ids.len() - 1)
        } else if candidates.iter().any(|c| c.text.contains("(button)"))
            && !candidates.iter().any(|c| c.text.starts_with("[1] textbox"))
        {
            candidates
                .iter()
                .position(|c| c.text.contains("(button)"))
                .unwrap()
        } else {
            0
        };
        Ok(Box::new(Pick {
            pick,
            k: candidates.len(),
            ids: [pick as i64],
        }))
    }
    fn run(&mut self, rows: &[Box<dyn decision::Row>]) -> anyhow::Result<Vec<decision::Scores>> {
        rows.iter()
            .map(|r| {
                let p = r
                    .as_any()
                    .downcast_ref::<Pick>()
                    .ok_or_else(|| anyhow::anyhow!("not a scripted row"))?;
                let k = p.k;
                let mut probs = vec![if k > 1 { 0.1 / (k - 1) as f32 } else { 0.0 }; k];
                probs[p.pick] = if k > 1 { 0.9 } else { 1.0 };
                Ok(decision::Scores {
                    logits: vec![0.0; k],
                    probs,
                    temperature: 1.0,
                    act_p: 0.5,
                    embedding: None,
                })
            })
            .collect()
    }
    fn load_time(&self) -> Duration {
        Duration::ZERO
    }
}

/// The decision engines the loop talks to: `decision` (default) and `decision/browser`, the
/// browser families active, both on the [`Scripted`] oracle (the files are touched so the
/// engine's presence check passes).
fn scripted_engine(data_dir: &std::path::Path) -> kameo::actor::ActorRef<decision::Engine> {
    let models = data_dir.join("models");
    for dir in ["decision", "decision-browser"] {
        for f in decision::engine::model_files(decision::Kind::Laya) {
            let p = models.join(dir).join(f);
            std::fs::create_dir_all(p.parent().unwrap()).unwrap();
            std::fs::write(p, b"x").unwrap();
        }
    }
    let mut cfg = decision::DecisionConfig::default();
    for (k, g) in cfg.families.iter_mut() {
        if k.starts_with("browser.") {
            g.mode = decision::Mode::Active;
        }
    }
    let step = Arc::new(std::sync::atomic::AtomicUsize::new(0));
    let loader: decision::Loader = Arc::new(move |_, _, _| {
        Ok(Box::new(Scripted { step: step.clone() }) as Box<dyn decision::Oracle>)
    });
    let log = decision::log::Log::new(data_dir.join("decisions"));
    let default = decision::Engine::spawn(decision::Engine::with_loader(
        cfg.clone(),
        models.join("decision"),
        log.clone(),
        loader.clone(),
    ));
    default.register(decision::DECISION_ACTOR).unwrap();
    let browser = decision::Engine::spawn(
        decision::Engine::with_loader(cfg, models.join("decision"), log, loader)
            .with_checkpoint("browser", models.join("decision-browser")),
    );
    browser.register(decision::actor_name("browser")).unwrap();
    default
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
