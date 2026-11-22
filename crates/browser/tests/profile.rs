//! 209 §10 P1, the agent's side of the shared browser profile, in process: a real profile store
//! (vault, sealed rows) behind the `webprofile` actor, real obscura isolates, the `browser_*`
//! tools driven through `ErasedTool`, and a loopback HTTP server that echoes the `Cookie` header
//! it receives. One test, so the actor registers once per binary.
//!
//! - a `__Host-` HttpOnly cookie survives tile push → profile → isolate (needs obscura with the
//!   lossless `host_only` round trip, 209 §4.3) and a save writes it back unchanged;
//! - a profile cookie reaches a real request of a tool session, and the tool reply that echoes it
//!   comes back scrubbed through `Ctx::call` (209 §9);
//! - the render provider's isolate (a private id) sends no profile cookie on its own request;
//! - an isolate's changes are saved on `Close`, the idle `Sweep`, `CloseTree` and `CloseAll`;
//!   a session never adds a site; a stale session's save does not revert another writer's cookie;
//! - `browser_open` to a signed-in site is credentialed, to another site not; `browser_eval` is
//!   credentialed once the session holds profile cookies (D14);
//! - with `browser.profile: off` a session loads nothing.

use std::sync::Arc;
use std::time::{Duration, Instant};

use browser::{BrowserConfig, Browsers, Call, Close, CloseAll, Sweep};
use kameo::actor::{ActorRef, Spawn};
use parking_lot::Mutex;
use runtime::SessionId;
use serde_json::{Value, json};
use tokio::io::{AsyncReadExt as _, AsyncWriteExt as _};
use tools::{DynTool, ToolCx, ToolOutput};
use web::Provider as _;
use webprofile::{Cookie, CookieChange, OriginStorage, Push, WebProfileActor};

const LOOPBACK_SECRET: &str = "loopback-secret-0123456789";

/// Tool calls of one session.
struct Kit {
    tools: Vec<DynTool>,
    data_dir: std::path::PathBuf,
}

impl Kit {
    fn new(actor: &ActorRef<Browsers>, cfg: &BrowserConfig, rt: runtime::Runtime) -> Self {
        Self {
            data_dir: rt.data_dir.clone(),
            tools: browser::tools::all(actor.clone(), cfg.clone(), rt),
        }
    }

    fn tool(&self, name: &str) -> &DynTool {
        self.tools
            .iter()
            .find(|t| t.spec().name == name)
            .unwrap_or_else(|| panic!("no tool {name}"))
    }

    async fn call(&self, session: SessionId, name: &str, args: Value) -> ToolOutput {
        let cx = ToolCx {
            session,
            call_id: "c1".to_owned(),
            quiet: false,
            resources: tools::SessionResources::durable_arc(&self.data_dir),
            cancel: tools::CancellationToken::new(),
            promote_after: None,
        };
        tools::egress::scope(true, self.tool(name).call(args, cx))
            .await
            .expect(name)
    }

    /// The session's first `browser_*` call: loads the profile.
    async fn start(&self, session: SessionId) {
        self.call(session, "browser_tabs", json!({ "action": "list" }))
            .await;
    }
}

fn cookie(name: &str, value: &str, domain: &str, host_only: bool) -> Cookie {
    Cookie {
        name: name.into(),
        value: value.into(),
        domain: domain.into(),
        host_only,
        path: "/".into(),
        expires: Some(4_000_000_000),
        secure: true,
        http_only: true,
        same_site: "Lax".into(),
        partitioned: false,
    }
}

fn tile_push(changes: Vec<CookieChange>) -> Push {
    Push {
        writer: "tile:ipad".into(),
        epoch: 0,
        changes,
        storage: vec![],
        basis_ms: None,
    }
}

/// The isolate's exported cookies of `session` (obscura's `browser_storage_state`), straight
/// through the actor like the render provider's calls.
async fn export(actor: &ActorRef<Browsers>, session: SessionId) -> Value {
    let v = actor
        .ask(Call {
            session,
            name: "browser_storage_state".into(),
            args: json!({}),
        })
        .await
        .expect("export");
    let text = v["content"][0]["text"].as_str().expect("export text");
    serde_json::from_str(text).expect("export json")
}

fn named<'a>(export: &'a Value, name: &str) -> Option<&'a Value> {
    export["cookies"]
        .as_array()?
        .iter()
        .find(|c| c["name"] == name)
}

/// Simulate the site setting a cookie in `session`'s isolate.
async fn site_sets(actor: &ActorRef<Browsers>, session: SessionId, name: &str, domain: &str) {
    let c = json!({
        "name": name, "value": format!("{name}-value-0123456789"), "domain": domain,
        "path": "/", "secure": true, "http_only": true, "same_site": "Lax", "expires": 4_000_000_000i64,
    });
    actor
        .ask(Call {
            session,
            name: "browser_set_storage_state".into(),
            args: json!({ "state": { "cookies": [c] } }),
        })
        .await
        .expect("set cookie");
}

/// Wait until the profile holds cookie `name` of `site`; that cookie.
async fn saved(profile: &webprofile::Handle, site: &str, name: &str) -> Cookie {
    let until = Instant::now() + Duration::from_secs(20);
    loop {
        let load = profile.load(Some(vec![site.into()])).await.expect("load");
        if let Some(c) = load
            .sites
            .iter()
            .flat_map(|s| s.jar.cookies.iter())
            .find(|c| c.name == name)
        {
            return c.clone();
        }
        assert!(Instant::now() < until, "{name} never saved to {site}");
        tokio::time::sleep(Duration::from_millis(100)).await;
    }
}

/// A loopback HTTP server: every request's path and `Cookie` header are recorded, and the page
/// shows the header back.
async fn echo_server() -> (String, Arc<Mutex<Vec<(String, Option<String>)>>>) {
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let base = format!("http://127.0.0.1:{}", listener.local_addr().unwrap().port());
    let seen = Arc::new(Mutex::new(Vec::new()));
    let log = seen.clone();
    tokio::spawn(async move {
        loop {
            let Ok((mut sock, _)) = listener.accept().await else {
                return;
            };
            let log = log.clone();
            tokio::spawn(async move {
                let mut buf = Vec::new();
                let mut chunk = [0u8; 4096];
                while !buf.windows(4).any(|w| w == b"\r\n\r\n") {
                    match sock.read(&mut chunk).await {
                        Ok(0) | Err(_) => return,
                        Ok(n) => buf.extend_from_slice(&chunk[..n]),
                    }
                }
                let head = String::from_utf8_lossy(&buf).into_owned();
                let path = head
                    .split_whitespace()
                    .nth(1)
                    .unwrap_or_default()
                    .to_owned();
                let cookie = head.lines().find_map(|l| {
                    let (k, v) = l.split_once(':')?;
                    k.eq_ignore_ascii_case("cookie")
                        .then(|| v.trim().to_owned())
                });
                let body = format!(
                    "<html><head><title>echo</title></head><body><p>you sent: {}</p></body></html>",
                    cookie.as_deref().unwrap_or("nothing")
                );
                log.lock().push((path, cookie));
                let reply = format!(
                    "HTTP/1.1 200 OK\r\nContent-Type: text/html\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
                    body.len()
                );
                let _ = sock.write_all(reply.as_bytes()).await;
                let _ = sock.shutdown().await;
            });
        }
    });
    (base, seen)
}

/// The `Cookie` header of the last request to `path`.
fn cookie_sent(seen: &Mutex<Vec<(String, Option<String>)>>, path: &str) -> Option<Option<String>> {
    seen.lock()
        .iter()
        .rev()
        .find(|(p, _)| p == path)
        .map(|(_, c)| c.clone())
}

#[tokio::test(flavor = "multi_thread")]
async fn shared_profile_loads_saves_and_stays_out_of_private_isolates() {
    let cfg = BrowserConfig {
        idle_secs: 0,
        ..Default::default()
    };
    browser::warmup(&cfg);
    let dir = tempfile::tempdir().unwrap();

    let vault = Arc::new(
        secrets::Vault::open(dir.path(), secrets::KeyStore::File)
            .await
            .unwrap(),
    );
    let store = webprofile::Profile::open(&dir.path().join(webprofile::store::DB), vault)
        .await
        .unwrap();
    let wp = WebProfileActor::spawn(WebProfileActor::new(None, Ok(store)));
    wp.register(webprofile::WEBPROFILE_ACTOR).unwrap();
    let profile = webprofile::Handle::from_actor(wp);

    // The user signed in on the tile: the app pushed the jar (209 §5 steps 1-3), and a loopback
    // site's session cookie.
    let mut loopback = cookie("sid", LOOPBACK_SECRET, "127.0.0.1", true);
    loopback.secure = false;
    let mut first = tile_push(vec![
        CookieChange::Set {
            cookie: cookie(
                "__Host-sid",
                "host-sid-value-0123456789",
                "app.example.com",
                true,
            ),
        },
        CookieChange::Set {
            cookie: cookie("pref", "pref-value-0123456789", ".example.com", false),
        },
        CookieChange::Set { cookie: loopback },
    ]);
    first.storage = vec![OriginStorage {
        origin: "https://app.example.com".into(),
        local_storage: vec![("token-key".into(), "storage-value".into())],
        captured_ms: 1,
    }];
    profile.push(first).await.expect("tile push");

    let rt = runtime::Runtime::new(figment::Figment::new(), dir.path().to_path_buf());
    let actor = Browsers::spawn(Browsers::new(cfg.clone()));
    let kit = Kit::new(&actor, &cfg, rt);

    // Session A's first browser_* call loads the profile into its isolate (209 D4).
    let a = SessionId::new();
    let fresh = SessionId::new();
    assert!(
        !kit.tool("browser_eval")
            .credentialed(a, &json!({ "expression": "1" })),
        "nothing loaded yet"
    );
    kit.start(a).await;
    let state = export(&actor, a).await;
    let sid = named(&state, "__Host-sid").expect("__Host- cookie loaded");
    assert_eq!(sid["host_only"], true);
    assert_eq!(sid["http_only"], true);
    assert_eq!(sid["domain"], "app.example.com");
    assert!(named(&state, "pref").is_some(), "domain cookie loaded");
    assert!(
        state["origins"]
            .as_array()
            .unwrap()
            .iter()
            .any(|o| o["origin"] == "https://app.example.com"
                && o["localStorage"][0][0] == "token-key"),
        "localStorage restored with the cookies"
    );

    // D14: open to a signed-in site is credentialed, to another site not; eval is credentialed on
    // any page once the session holds profile cookies.
    let open = kit.tool("browser_open");
    assert!(open.credentialed(a, &json!({ "url": "https://app.example.com/orders" })));
    assert!(!open.credentialed(a, &json!({ "url": "https://unrelated.test/" })));
    let eval = kit.tool("browser_eval");
    assert!(eval.credentialed(a, &json!({ "expression": "1" })));
    assert!(!eval.credentialed(fresh, &json!({ "expression": "1" })));

    // The profile's loopback cookie reaches a real request, and the reply that echoes it is
    // scrubbed through `Ctx::call` (209 §9).
    let (base, seen) = echo_server().await;
    let out = kit
        .call(
            a,
            "browser_open",
            json!({ "url": format!("{base}/signed") }),
        )
        .await;
    assert!(!out.is_error, "{}", out.content);
    let sent = cookie_sent(&seen, "/signed").expect("the page was requested");
    assert!(
        sent.as_deref().is_some_and(|c| c.contains(LOOPBACK_SECRET)),
        "the profile cookie was sent"
    );
    assert!(out.content.contains("echo"), "{}", out.content);
    assert!(!out.content.contains(LOOPBACK_SECRET), "{}", out.content);
    assert!(out.content.contains("[cookie sid]"), "{}", out.content);

    // The render provider's isolate (a private id) sends no profile cookie on its own request.
    let render = browser::render::ObscuraRender::new(actor.clone());
    let url = url::Url::parse(&format!("{base}/rendered")).unwrap();
    render
        .render(&reqwest::Client::new(), &url)
        .await
        .expect("render");
    assert_eq!(
        cookie_sent(&seen, "/rendered"),
        Some(None),
        "the render isolate sent a cookie"
    );
    // Nor does a private isolate hold any.
    let private = SessionId::new();
    assert_eq!(export(&actor, private).await["cookies"], json!([]));

    // Save on Close: a cookie the site set on a profile site reaches the profile; the loaded
    // `__Host-` cookie comes back unchanged (still host-only); a site the profile does not hold
    // is never added by a session (209 §4.2).
    site_sets(&actor, a, "fresh", "example.com").await;
    site_sets(&actor, a, "own", "never-signed-in.example").await;
    actor.tell(Close { session: a }).await.unwrap();
    let fresh_cookie = saved(&profile, "example.com", "fresh").await;
    assert_eq!(fresh_cookie.domain, "example.com");
    let load = profile
        .load(Some(vec!["example.com".into()]))
        .await
        .unwrap();
    let row = &load.sites[0];
    assert!(row.writer.starts_with("obscura:"), "writer {}", row.writer);
    let sid = row
        .jar
        .cookies
        .iter()
        .find(|c| c.name == "__Host-sid")
        .unwrap();
    assert!(sid.host_only && sid.http_only && sid.secure);

    // A stale session's save does not revert another writer's cookie: C loads, the tile then
    // refreshes `pref`; C changes only its own cookie, and its save keeps the tile's `pref`.
    let c = SessionId::new();
    kit.start(c).await;
    profile
        .push(tile_push(vec![CookieChange::Set {
            cookie: cookie("pref", "pref-from-tile-later-0123", "example.com", false),
        }]))
        .await
        .unwrap();
    site_sets(&actor, c, "c-own", "example.com").await;
    actor.tell(Close { session: c }).await.unwrap();
    saved(&profile, "example.com", "c-own").await;
    assert_eq!(
        saved(&profile, "example.com", "pref").await.value,
        "pref-from-tile-later-0123"
    );

    // Save on the idle Sweep (which also drops the private isolates; they save nothing).
    let b = SessionId::new();
    kit.start(b).await;
    assert!(
        named(&export(&actor, b).await, "fresh").is_some(),
        "b loaded a's save"
    );
    site_sets(&actor, b, "swept", "example.com").await;
    actor.tell(Sweep).await.unwrap();
    saved(&profile, "example.com", "swept").await;

    // Save on CloseTree, and the ack is not held up by the save.
    let d = SessionId::new();
    kit.start(d).await;
    site_sets(&actor, d, "treed", "example.com").await;
    let ack = actor
        .ask(tools::CloseTree {
            subtree: vec![d],
            generation: 1,
            cancel: tools::CancellationToken::new(),
        })
        .await
        .unwrap();
    assert!(ack.ok, "{:?}", ack.detail);
    saved(&profile, "example.com", "treed").await;

    // Save on CloseAll (shutdown).
    let e = SessionId::new();
    kit.start(e).await;
    site_sets(&actor, e, "at-shutdown", "example.com").await;
    actor.tell(CloseAll).await.unwrap();
    saved(&profile, "example.com", "at-shutdown").await;

    // `browser.profile: off`: a new session loads nothing.
    let off_rt = runtime::Runtime::new(
        figment::Figment::new().merge(figment::providers::Serialized::defaults(
            json!({ "browser": { "profile": "off" } }),
        )),
        dir.path().to_path_buf(),
    );
    let off_kit = Kit::new(&actor, &cfg, off_rt);
    let f = SessionId::new();
    off_kit.start(f).await;
    assert_eq!(export(&actor, f).await["cookies"], json!([]));

    let sites = profile.sites().await.unwrap();
    let mut names: Vec<&str> = sites.iter().map(|s| s.site.as_str()).collect();
    names.sort_unstable();
    assert_eq!(names, ["127.0.0.1", "example.com"]);
}
