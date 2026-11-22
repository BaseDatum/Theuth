//! 209 §10 P2, the agent's side of forget and sign out everywhere, in process: a real profile
//! store behind the `webprofile` actor, real obscura isolates driven through the `browser_*`
//! tools, and a loopback HTTP server that records the `Cookie` header it receives. One test, so
//! the actor registers once per binary.
//!
//! The events travel as in the engine: the profile actor publishes `WebProfileChanged` on a
//! `Runtime`'s bus and the `Browsers` actor is subscribed to it as the plugin subscribes it
//! (`browser::subscribe`). The forget happens "on another engine": its rows are written straight
//! into the file (as replication would) and the profile actor is told which rows changed
//! (`Replicated`, as the replica feed tells it), so it reads the forgotten site from the marker
//! row and publishes it; sign out everywhere goes through the profile actor itself.
//!
//! - a stale session (loaded before the forget) saving a refreshed cookie of the forgotten site
//!   does not write it back;
//! - after the forget, a live isolate's next call — not an open of that site — already runs
//!   without the site's cookies, its request to the site carries none, and acting there is no
//!   longer credentialed; another site stays;
//! - sign out everywhere drops the isolate's profile cookies before its next call, and a client
//!   push under the old epoch is `stale-epoch`.

use std::sync::Arc;
use std::time::{Duration, Instant};

use browser::{BrowserConfig, Browsers, Call, Close};
use kameo::actor::{ActorRef, Spawn};
use parking_lot::Mutex;
use runtime::SessionId;
use serde_json::{Value, json};
use tokio::io::{AsyncReadExt as _, AsyncWriteExt as _};
use tools::{DynTool, ToolCx, ToolOutput};
use webprofile::{Cookie, CookieChange, Push, WebProfileActor};

/// Wait until `f` holds (an event handled on the bus), at most 10 s.
async fn until(what: &str, f: impl Fn() -> bool) {
    let deadline = Instant::now() + Duration::from_secs(10);
    while !f() {
        assert!(Instant::now() < deadline, "timed out: {what}");
        tokio::time::sleep(Duration::from_millis(20)).await;
    }
}

const LOOPBACK_SECRET: &str = "loopback-secret-0123456789";

struct Kit {
    tools: Vec<DynTool>,
    data_dir: std::path::PathBuf,
}

impl Kit {
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

    /// A call that is not an open: loads the profile the first time, runs queued deletes later.
    async fn touch(&self, session: SessionId) {
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
        secure: false,
        http_only: true,
        same_site: "Lax".into(),
        partitioned: false,
    }
}

fn tile_push(epoch: u64, changes: Vec<CookieChange>) -> Push {
    Push {
        writer: "tile:ipad".into(),
        epoch,
        changes,
        storage: vec![],
        basis_ms: None,
    }
}

async fn export(actor: &ActorRef<Browsers>, session: SessionId) -> Value {
    let v = actor
        .ask(Call {
            session,
            name: "browser_storage_state".into(),
            args: json!({}),
        })
        .await
        .expect("export");
    serde_json::from_str(v["content"][0]["text"].as_str().expect("export text"))
        .expect("export json")
}

fn has(export: &Value, name: &str) -> bool {
    export["cookies"]
        .as_array()
        .is_some_and(|cs| cs.iter().any(|c| c["name"] == name))
}

/// The site sets (or refreshes) a cookie in `session`'s isolate.
async fn site_sets(actor: &ActorRef<Browsers>, session: SessionId, name: &str, domain: &str) {
    let c = json!({
        "name": name, "value": format!("{name}-refreshed-0123456789"), "domain": domain,
        "path": "/", "secure": false, "http_only": true, "same_site": "Lax", "expires": 4_000_000_000i64,
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

async fn names(profile: &webprofile::Handle, site: &str) -> Vec<String> {
    let mut v: Vec<String> = profile
        .pull(Some(vec![site.into()]))
        .await
        .expect("pull")
        .cookies
        .into_iter()
        .map(|c| c.cookie.name)
        .collect();
    v.sort();
    v
}

async fn until_saved(profile: &webprofile::Handle, site: &str, name: &str) {
    let until = Instant::now() + Duration::from_secs(20);
    while !names(profile, site).await.iter().any(|n| n == name) {
        assert!(Instant::now() < until, "{name} never saved to {site}");
        tokio::time::sleep(Duration::from_millis(100)).await;
    }
}

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
                log.lock().push((path, cookie));
                let body = "<html><head><title>echo</title></head><body>ok</body></html>";
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

fn cookie_sent(seen: &Mutex<Vec<(String, Option<String>)>>, path: &str) -> Option<Option<String>> {
    seen.lock()
        .iter()
        .rev()
        .find(|(p, _)| p == path)
        .map(|(_, c)| c.clone())
}

#[tokio::test(flavor = "multi_thread")]
async fn forget_and_sign_out_reach_live_isolates_before_their_next_call() {
    let cfg = BrowserConfig {
        idle_secs: 3600,
        ..Default::default()
    };
    browser::warmup(&cfg);
    let dir = tempfile::tempdir().unwrap();
    let vault = Arc::new(
        secrets::Vault::open(dir.path(), secrets::KeyStore::File)
            .await
            .unwrap(),
    );
    let db = dir.path().join(webprofile::store::DB);
    let store = webprofile::Profile::open(&db, vault.clone()).await.unwrap();
    // This engine's bus: the profile actor publishes on it; `Browsers` listens as in the plugin.
    let rt = runtime::Runtime::new(figment::Figment::new(), dir.path().to_path_buf());
    webprofile::declare_wire(&rt).await.unwrap();
    browser::declare_wire(&rt).await.unwrap();
    let wp = WebProfileActor::spawn(WebProfileActor::new(Some(rt.clone()), Ok(store)));
    wp.register(webprofile::WEBPROFILE_ACTOR).unwrap();
    let profile = webprofile::Handle::from_actor(wp.clone());

    // The user signed in on the tile: a loopback site's session and an example.com cookie.
    profile
        .push(tile_push(
            0,
            vec![
                CookieChange::Set {
                    cookie: cookie("sid", LOOPBACK_SECRET, "127.0.0.1", true),
                },
                CookieChange::Set {
                    cookie: cookie("pref", "pref-value-0123456789", "example.com", false),
                },
            ],
        ))
        .await
        .expect("tile push");

    let actor =
        Browsers::spawn_with_mailbox(Browsers::new(cfg.clone()), runtime::subscriber_mailbox());
    browser::subscribe(&rt, &actor).await;
    let kit = Kit {
        data_dir: rt.data_dir.clone(),
        tools: browser::tools::all(actor.clone(), cfg.clone(), rt.clone()),
    };
    let (base, seen) = echo_server().await;
    let open = kit.tool("browser_open");
    let loopback = json!({ "url": format!("{base}/x") });
    // Session A (stays live) and session S (will be stale) load the profile.
    let (a, s) = (SessionId::new(), SessionId::new());
    kit.touch(a).await;
    kit.touch(s).await;
    assert!(has(&export(&actor, a).await, "sid"));
    let out = kit
        .call(
            a,
            "browser_open",
            json!({ "url": format!("{base}/before") }),
        )
        .await;
    assert!(!out.is_error, "{}", out.content);
    assert!(
        cookie_sent(&seen, "/before")
            .flatten()
            .is_some_and(|c| c.contains(LOOPBACK_SECRET)),
        "signed in before the forget"
    );
    assert!(open.credentialed(a, &loopback));

    // Another engine forgets the loopback site: its rows land in this engine's file, and the
    // replica feed has not told this engine's profile actor yet.
    let elsewhere = webprofile::Profile::open(&db, vault.clone()).await.unwrap();
    elsewhere.forget("127.0.0.1", "settings").await.unwrap();
    // S's site refreshes the forgotten cookie and sets one of a held site; S closes and saves.
    // The held site's cookie is written, the forgotten one is not: S read the profile before
    // the forget.
    site_sets(&actor, s, "sid", "127.0.0.1").await;
    site_sets(&actor, s, "s-own", "example.com").await;
    actor.tell(Close { session: s }).await.unwrap();
    until_saved(&profile, "example.com", "s-own").await;
    assert_eq!(
        names(&profile, "127.0.0.1").await,
        Vec::<String>::new(),
        "a stale session's save does not bring the forgotten site back"
    );

    // The replica feed tells the profile actor which rows changed (the forget marker): it reads
    // the site from it and publishes `WebProfileChanged` on the bus, which `Browsers` handles.
    let marker = webprofile::store::Ids::of(&vault.mesh_key()).forget("127.0.0.1");
    wp.tell(webprofile::Replicated(Some(vec![marker])))
        .await
        .unwrap();
    until(
        "acting on the forgotten site is no longer credentialed",
        || !open.credentialed(a, &loopback),
    )
    .await;
    // A's next call is not an open of that site; the cookie is gone after it, the other stays.
    kit.touch(a).await;
    let state = export(&actor, a).await;
    assert!(!has(&state, "sid"), "{state}");
    assert!(has(&state, "pref"), "{state}");
    let out = kit
        .call(a, "browser_open", json!({ "url": format!("{base}/after") }))
        .await;
    assert!(!out.is_error, "{}", out.content);
    assert_eq!(
        cookie_sent(&seen, "/after"),
        Some(None),
        "the forgotten cookie was sent"
    );
    assert!(open.credentialed(a, &json!({ "url": "https://example.com/" })));

    // Sign out everywhere through this engine's profile actor: the new epoch reaches `Browsers`
    // on the bus; A drops its profile cookies before its next call and loads again (nothing); a
    // client push under the old epoch is stale.
    let epoch = profile.clear().await.unwrap().epoch;
    assert_eq!(epoch, 1);
    until("sign out everywhere reached the browser", || {
        !open.credentialed(a, &json!({ "url": "https://example.com/" }))
    })
    .await;
    kit.touch(a).await;
    let state = export(&actor, a).await;
    assert!(!has(&state, "pref"), "{state}");
    assert_eq!(
        profile
            .push(tile_push(
                0,
                vec![CookieChange::Set {
                    cookie: cookie("pref", "old-epoch", "example.com", false),
                }],
            ))
            .await,
        Err(webprofile::ProfileError::StaleEpoch { current: 1 })
    );
}
