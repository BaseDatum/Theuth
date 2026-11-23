//! 209 P5 end to end in-process: `browser_show_element` on the agent's own obscura browser against
//! a loopback sign-in page. The card is answered through the real `answer_element_form` RPC
//! dispatch on an owner connection (browser's RPC module), as the app does; the server records
//! what the form posted. The test password must appear in no wire event, no log line (captured at
//! debug level), no tool output and no file under the data dir, which holds the session store
//! with every tool output of the run.

use std::io::{Read, Write};
use std::net::TcpListener;
use std::sync::{Arc, Mutex};
use std::time::Duration;

use browser::show_element::{ElementFormId, ElementFormRequested};
use browser::{BrowserConfig, Browsers};
use kameo::actor::Spawn;
use runtime::SessionId;
use serde_json::{Value, json};
use tools::{DynTool, Risk, ToolCx, ToolOutput};

const PASSWORD: &str = "Corr3ct-Horse-Battery-Staple";
const STALE_VALUE: &str = "STALE_QUERY_VALUE";

fn page(port: u16) -> String {
    let filler = "<p>lorem ipsum dolor sit amet, consectetur adipiscing elit</p>".repeat(6_000);
    format!(
        r#"<!doctype html><html><head><title>Sign in fixture</title></head><body>
<h1>Welcome back</h1>
<form id="login" method="post" action="/login" onsubmit="window.submitted = 1">
<input type="hidden" name="csrf" value="tok-123">
<label for="email">Email</label><input id="email" name="email" type="email" onfocus="window.bad = 1">
<label for="pw">Password</label><input id="pw" name="pw" type="password" value="prefilled-not-copied">
<label><input type="checkbox" name="remember"> Remember me</label>
<script>window.inline = 1;</script>
<a href="/forgot">Forgot your password?</a>
<button type="submit">Sign in</button>
</form>
<div id="framed"><input name="u" aria-label="User"><iframe src="http://127.0.0.1:{port}/frame"></iframe></div>
<div id="big"><input name="b" aria-label="Big">{filler}</div>
<form id="search" action="/search"><input name="q" aria-label="Search"><button>Go</button></form>
</body></html>"#
    )
}

/// `GET /` → the sign-in page; `POST /login` → records the body, answers a welcome page that
/// echoes it (so a later read must scrub it); every request line is recorded.
fn serve(log: Arc<Mutex<Vec<String>>>) -> u16 {
    let listener = TcpListener::bind("127.0.0.1:0").expect("bind");
    let port = listener.local_addr().expect("addr").port();
    std::thread::spawn(move || {
        for stream in listener.incoming() {
            let Ok(mut s) = stream else { continue };
            let log = log.clone();
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
                log.lock().unwrap().push(format!("{request} {body}"));
                let html = if request.starts_with("POST /login") {
                    // The page shows what was posted, written by its script: the HTML parser
                    // (html5ever logs every text token at debug) never sees it in clear.
                    use base64::Engine as _;
                    let pw = body
                        .split('&')
                        .find_map(|kv| kv.strip_prefix("pw="))
                        .unwrap_or_default();
                    let pw = base64::engine::general_purpose::STANDARD.encode(pw);
                    format!(
                        "<html><head><title>Welcome</title></head><body><p id=\"echo\"></p><script>document.getElementById('echo').textContent = 'signed in with ' + atob('{pw}');</script></body></html>"
                    )
                } else if request.starts_with("GET / ") {
                    page(port)
                } else {
                    "<html><head><title>Other</title></head><body>other</body></html>".to_owned()
                };
                let _ = write!(
                    s,
                    "HTTP/1.1 200 OK\r\nContent-Type: text/html; charset=utf-8\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{html}",
                    html.len()
                );
                let _ = s.flush();
            });
        }
    });
    port
}

/// Log lines, as the subscriber writes them.
#[derive(Clone, Default)]
struct Logs(Arc<Mutex<Vec<u8>>>);

impl Write for Logs {
    fn write(&mut self, b: &[u8]) -> std::io::Result<usize> {
        self.0.lock().unwrap().extend_from_slice(b);
        Ok(b.len())
    }
    fn flush(&mut self) -> std::io::Result<()> {
        Ok(())
    }
}

struct Kit {
    tools: Vec<DynTool>,
    session: SessionId,
    data_dir: std::path::PathBuf,
}

impl Kit {
    fn tool(&self, name: &str) -> DynTool {
        self.tools
            .iter()
            .find(|t| t.spec().name == name)
            .cloned()
            .unwrap_or_else(|| panic!("no tool {name}"))
    }

    fn cx(&self, call_id: &str) -> ToolCx {
        ToolCx {
            session: self.session,
            call_id: call_id.to_owned(),
            quiet: false,
            resources: tools::SessionResources::durable_arc(&self.data_dir),
            cancel: tools::CancellationToken::new(),
            promote_after: None,
        }
    }

    async fn call(&self, name: &str, args: Value) -> ToolOutput {
        let tool = self.tool(name);
        tools::egress::scope(true, tool.call(args, self.cx("c0")))
            .await
            .expect(name)
    }

    /// `browser_show_element` started in the background (it waits for the card's answer).
    fn show(&self, args: Value, call_id: &str) -> tokio::task::JoinHandle<ToolOutput> {
        let tool = self.tool("browser_show_element");
        let cx = self.cx(call_id);
        tokio::spawn(async move {
            tools::egress::scope(true, tool.call(args, cx))
                .await
                .expect("browser_show_element")
        })
    }
}

/// Every wire frame of the feed as `(topic, event JSON)`, collected in the background.
type Frames = Arc<Mutex<Vec<(String, Value)>>>;

async fn next_card(frames: &Frames, seen: &mut usize) -> ElementFormRequested {
    for _ in 0..200 {
        {
            let all = frames.lock().unwrap();
            for (topic, ev) in all.iter().skip(*seen) {
                *seen += 1;
                if topic == "browser.element-form-requested" {
                    return serde_json::from_value(ev.clone()).expect("card");
                }
            }
        }
        tokio::time::sleep(Duration::from_millis(100)).await;
    }
    panic!("no ElementFormRequested within 20 s");
}

async fn outcome_of(frames: &Frames, id: ElementFormId) -> String {
    for _ in 0..100 {
        if let Some(o) = frames.lock().unwrap().iter().find_map(|(t, e)| {
            (t == "browser.element-form-resolved" && e["id"] == json!(id))
                .then(|| e["outcome"].as_str().unwrap_or_default().to_owned())
        }) {
            return o;
        }
        tokio::time::sleep(Duration::from_millis(100)).await;
    }
    panic!("no ElementFormResolved for {id:?}");
}

async fn answer(rpc: &jsonrpsee::Methods, id: ElementFormId, answer: Value) -> bool {
    rpc.call(
        "answer_element_form",
        (id, answer, Option::<SessionId>::None),
    )
    .await
    .expect("answer_element_form")
}

fn files_containing(dir: &std::path::Path, needle: &[u8]) -> Vec<std::path::PathBuf> {
    let mut hits = Vec::new();
    for entry in std::fs::read_dir(dir).into_iter().flatten().flatten() {
        let path = entry.path();
        if path.is_dir() {
            hits.extend(files_containing(&path, needle));
        } else if std::fs::read(&path).is_ok_and(|b| b.windows(needle.len()).any(|w| w == needle)) {
            hits.push(path);
        }
    }
    hits
}

#[tokio::test(flavor = "multi_thread")]
async fn a_sign_in_form_goes_to_the_chat_and_back_into_the_page() {
    // SAFETY: first thing in the only test of this binary; nothing has read the environment yet.
    unsafe { std::env::set_var("OBSCURA_ALLOW_PRIVATE_NETWORK", "1") };
    let logs = Logs::default();
    let writer = logs.clone();
    tracing_subscriber::fmt()
        .with_max_level(tracing::Level::DEBUG)
        .with_writer(move || writer.clone())
        .init();
    let cfg = BrowserConfig {
        allow_private_network: true,
        stealth: true,
        max_chars: 4000,
        ..Default::default()
    };
    browser::warmup(&cfg);
    let requests = Arc::new(Mutex::new(Vec::new()));
    let port = serve(requests.clone());
    let base = format!("http://127.0.0.1:{port}/");
    let origin = format!("http://127.0.0.1:{port}");
    let data_dir = tempfile::tempdir().expect("temp dir");
    let rt = runtime::Runtime::new(figment::Figment::new(), data_dir.path().to_path_buf());
    browser::declare_wire(&rt).await.expect("declare");
    let frames: Frames = Arc::default();
    {
        use wire::RuntimeWire as _;
        let mut feed = rt.wire_events();
        let frames = frames.clone();
        tokio::spawn(async move {
            while let Ok(f) = feed.recv().await {
                let topic = f.descriptor.as_ref().map(|d| d.topic.to_owned());
                frames
                    .lock()
                    .unwrap()
                    .push((topic.unwrap_or_default(), f.event.clone()));
            }
        });
    }
    let browsers = Browsers::spawn(Browsers::new(cfg.clone()));
    let mut rpc: jsonrpsee::Methods = browser::rpc_module(rpc_core::Server::new(rt.clone()))
        .module
        .into();
    let (peer, _conn) = rpc_core::RpcPeer::open();
    rpc.extensions_mut().insert(peer);
    let kit = Kit {
        tools: browser::tools::all(browsers.clone(), cfg, rt.clone()),
        session: SessionId::new(),
        data_dir: data_dir.path().to_path_buf(),
    };
    let mut outputs: Vec<String> = Vec::new();

    // The tool is Mutating and approved by its card (a read_only cap hides it: 209 §9).
    let spec = kit.tool("browser_show_element").spec();
    assert_eq!((spec.risk, spec.direct), (Risk::Mutating, true));
    assert!(kit.tool("browser_show_element").approved_by_card());
    assert!(!kit.tool("browser_show_element").promotable());

    let opened = kit.call("browser_open", json!({ "url": base })).await;
    assert!(!opened.is_error, "{}", opened.content);

    // Refusals: exactly one target, a frame, an oversize element.
    let both = kit
        .call(
            "browser_show_element",
            json!({ "ref": "e1", "selector": "#login" }),
        )
        .await;
    assert_eq!(both.content, "pass exactly one of `ref` or `selector`");
    let framed = kit
        .call("browser_show_element", json!({ "selector": "#framed" }))
        .await;
    assert!(framed.is_error);
    assert_eq!(
        framed.content,
        format!(
            "the form contains a frame from {origin} (a CAPTCHA or a sign-in provider); it can only be used in the page itself: show it to the user with show_web or browser_attach to their tile"
        )
    );
    let big = kit
        .call("browser_show_element", json!({ "selector": "#big" }))
        .await;
    assert!(big.is_error);
    assert!(
        big.content.starts_with("element too large to show (")
            && big
                .content
                .ends_with(" KiB > 256 KiB); pick a smaller element"),
        "{}",
        big.content
    );
    assert!(
        frames
            .lock()
            .unwrap()
            .iter()
            .all(|(t, _)| !t.starts_with("browser.element-form")),
        "a refused element published a card"
    );

    // The sign-in form as a card: no script, no handler, no address, no value; password masked.
    let mut seen = 0;
    let task = kit.show(json!({ "selector": "#login" }), "call-login");
    let card = next_card(&frames, &mut seen).await;
    assert_eq!(card.session, kit.session);
    assert_eq!(card.call_id, "call-login");
    assert_eq!(card.origin, origin);
    assert_eq!(card.action_origin, origin, "the form posts to its own page");
    assert_eq!(card.url, base);
    assert_eq!(card.source, "obscura:local");
    assert_eq!(card.title, "Sign in fixture");
    for bad in [
        "script",
        "inline",
        "onfocus",
        "onsubmit",
        "action",
        "href",
        "/login",
        "prefilled-not-copied",
        "tok-123",
        "csrf",
        "value=",
    ] {
        assert!(!card.html.contains(bad), "{bad} in the card: {}", card.html);
    }
    assert!(card.html.contains(r#"type="password""#), "{}", card.html);
    assert!(card.html.contains("Forgot your password?"), "{}", card.html);
    let kinds: Vec<(&str, &str, bool)> = card
        .fields
        .iter()
        .map(|f| (f.kind.as_str(), f.name.as_str(), f.masked))
        .collect();
    assert_eq!(
        kinds,
        [
            ("email", "email", false),
            ("password", "pw", true),
            ("checkbox", "remember", false),
            ("submit", "", false),
        ]
    );
    assert_eq!(card.fields[0].label, "Email");
    for (i, f) in card.fields.iter().enumerate() {
        assert_eq!(f.index as usize, i);
        assert!(
            card.html.contains(&format!("data-theuth-field=\"{i}\"")),
            "{}",
            card.html
        );
    }

    // A second card of the same element leaves the first one valid (each card marks the page
    // with its own attributes); closing it touches nothing.
    let second = kit.show(json!({ "selector": "#login" }), "call-login-2");
    let card2 = next_card(&frames, &mut seen).await;
    assert_ne!(card2.id, card.id);
    assert!(answer(&rpc, card2.id, json!({ "cancel": {} })).await);
    assert!(
        second
            .await
            .expect("tool task")
            .content
            .starts_with("cancelled · ")
    );

    // The app shows the card; a submit naming no field the card fills is refused and the card
    // stays.
    let on_screen: bool = rpc
        .call("element_form_shown", (card.id, Option::<SessionId>::None))
        .await
        .expect("element_form_shown");
    assert!(on_screen);
    assert!(
        !answer(
            &rpc,
            card.id,
            json!({ "submit": { "values": { "3": "x" } } })
        )
        .await,
        "a submit with nothing to fill was taken"
    );

    // A late client sees it; a guest may neither list nor answer it.
    let pending: Vec<ElementFormRequested> = rpc
        .call("element_forms_pending", (Option::<SessionId>::None,))
        .await
        .expect("pending");
    assert_eq!(pending, vec![card.clone()]);
    let mut as_guest: jsonrpsee::Methods = browser::rpc_module(rpc_core::Server::new(rt.clone()))
        .module
        .into();
    let (guest, _guest_conn) = rpc_core::RpcPeer::open_guest([7; 32], [9; 32]);
    as_guest.extensions_mut().insert(guest);
    let refused = as_guest
        .call::<_, bool>(
            "answer_element_form",
            (card.id, json!({ "cancel": {} }), Option::<SessionId>::None),
        )
        .await;
    assert!(refused.is_err(), "a guest answered: {refused:?}");
    let listed = as_guest
        .call::<_, Vec<ElementFormRequested>>("element_forms_pending", (Option::<SessionId>::None,))
        .await;
    assert!(listed.is_err(), "a guest listed the cards");

    // Submit through the RPC: the page's real form gets the values by index and is posted.
    let values = json!({ "submit": { "values": {
        "0": "me@example.com", "1": PASSWORD, "2": "true", "3": "ignored", "9": "out of range",
    } } });
    assert!(answer(&rpc, card.id, values.clone()).await);
    assert!(
        !answer(&rpc, card.id, values).await,
        "a second answer was taken"
    );
    let out = task.await.expect("tool task");
    outputs.push(out.content.clone());
    assert!(!out.is_error, "{}", out.content);
    assert_eq!(
        out.content,
        format!(
            "submitted · {origin} · filled 0 email, 1 pw, 2 remember · now {base}login — Welcome"
        )
    );
    assert_eq!(outcome_of(&frames, card.id).await, "submitted");
    let posted = requests
        .lock()
        .unwrap()
        .iter()
        .find(|r| r.starts_with("POST /login"))
        .cloned()
        .expect("the form was posted");
    let encoded: String = url::form_urlencoded::byte_serialize(PASSWORD.as_bytes()).collect();
    assert!(posted.contains("email=me%40example.com"), "{posted}");
    assert!(posted.contains(&format!("pw={encoded}")), "{posted}");
    assert!(posted.contains("remember=on"), "{posted}");
    assert!(
        posted.contains("csrf=tok-123"),
        "the hidden input stayed: {posted}"
    );

    // The page now echoes the password: later replies of this session are scrubbed.
    let read = kit.call("browser_read", json!({})).await;
    outputs.push(read.content.clone());
    assert!(
        read.content.contains("signed in with [typed value]"),
        "{}",
        read.content
    );

    // Stale: the page is reloaded before the answer; nothing is typed or sent.
    let opened = kit.call("browser_open", json!({ "url": base })).await;
    assert!(!opened.is_error, "{}", opened.content);
    let task = kit.show(json!({ "selector": "#search" }), "call-search");
    let card = next_card(&frames, &mut seen).await;
    assert_eq!(card.fields.len(), 2);
    let reopened = kit.call("browser_open", json!({ "url": base })).await;
    assert!(!reopened.is_error, "{}", reopened.content);
    let values = json!({ "submit": { "values": { "0": STALE_VALUE } } });
    assert!(answer(&rpc, card.id, values).await);
    let out = task.await.expect("tool task");
    outputs.push(out.content.clone());
    assert!(out.is_error);
    assert_eq!(
        out.content,
        "the page changed since the form was shown; nothing was typed"
    );
    assert_eq!(outcome_of(&frames, card.id).await, "stale");
    let typed = kit
        .call(
            "browser_eval",
            json!({ "expression": "document.querySelector('#search input').value" }),
        )
        .await;
    assert_eq!(typed.content.trim_matches('"'), "", "{}", typed.content);

    // Cancel: the card closes, nothing is typed.
    let task = kit.show(
        json!({ "selector": "#login", "title": "Example sign-in" }),
        "call-cancel",
    );
    let card = next_card(&frames, &mut seen).await;
    assert_eq!(card.title, "Example sign-in");
    assert!(answer(&rpc, card.id, json!("cancel")).await);
    let out = task.await.expect("tool task");
    outputs.push(out.content.clone());
    assert_eq!(
        out.content,
        format!("cancelled · {origin} · the user closed the form; nothing was typed")
    );
    assert_eq!(outcome_of(&frames, card.id).await, "cancelled");

    // The session ends while a card shows: the card closes as cancelled and leaves the list.
    let task = kit.show(json!({ "selector": "#search" }), "call-ended");
    let card = next_card(&frames, &mut seen).await;
    browser::show_element::forget(&[kit.session]);
    let out = task.await.expect("tool task");
    outputs.push(out.content.clone());
    assert!(out.content.starts_with("cancelled · "), "{}", out.content);
    assert_eq!(outcome_of(&frames, card.id).await, "cancelled");
    let pending: Vec<ElementFormRequested> = rpc
        .call("element_forms_pending", (Option::<SessionId>::None,))
        .await
        .expect("pending");
    assert!(pending.is_empty());

    // Nothing of what the user typed left the page, the server and the RPC params.
    assert!(
        requests
            .lock()
            .unwrap()
            .iter()
            .all(|r| !r.contains(STALE_VALUE)),
        "a stale card was typed or sent"
    );
    for (topic, ev) in frames.lock().unwrap().iter() {
        let text = ev.to_string();
        assert!(
            !text.contains(PASSWORD) && !text.contains(&encoded),
            "{topic}: {text}"
        );
        assert!(!text.contains("me@example.com"), "{topic}: {text}");
    }
    for out in &outputs {
        assert!(!out.contains(PASSWORD) && !out.contains(&encoded), "{out}");
    }
    let logged = String::from_utf8_lossy(&logs.0.lock().unwrap()).into_owned();
    assert!(!logged.is_empty(), "no log line was captured");
    let leaks: Vec<String> = logged
        .lines()
        .filter(|l| l.contains(PASSWORD) || l.contains(&encoded))
        .map(|l| {
            let l = l
                .replace(PASSWORD, "<PASSWORD>")
                .replace(&encoded, "<PASSWORD>");
            l.chars().take(300).collect()
        })
        .collect();
    assert!(leaks.is_empty(), "the password is in the log: {leaks:#?}");

    // The run's transcript, as the engine keeps it: every tool output in `sessions.sqlite`.
    let store = agent::store::Store::open(&data_dir.path().join("sessions.sqlite"), None)
        .await
        .expect("session store");
    store
        .create_session(kit.session, "m", None, None, None)
        .await
        .expect("session");
    for out in &outputs {
        store
            .append_local_note(kit.session, out)
            .await
            .expect("note");
    }
    drop(store);
    assert!(
        !files_containing(data_dir.path(), b"signed in with [typed value]").is_empty(),
        "the store holds no transcript"
    );
    assert_eq!(
        files_containing(data_dir.path(), PASSWORD.as_bytes()),
        Vec::<std::path::PathBuf>::new()
    );
    let _ = tokio::time::timeout(
        Duration::from_secs(5),
        browsers.ask(browser::Close {
            session: kit.session,
        }),
    )
    .await;
}
