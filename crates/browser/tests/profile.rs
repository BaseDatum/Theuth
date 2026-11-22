//! 209 §10 P1, the agent's side of the shared browser profile, in process: a real profile store
//! (vault, sealed rows) behind the `webprofile` actor, real obscura isolates, the `browser_*`
//! tools driven through `ErasedTool`. No network. One test, so the actor registers once per binary.
//!
//! - a `__Host-` HttpOnly cookie survives tile push → profile → isolate (needs obscura with the
//!   lossless `host_only` round trip, 209 §4.3) and a save writes it back unchanged;
//! - a private-id isolate (the render provider's and scrape slots' kind) never gets the profile;
//! - an isolate's changes are saved on `Close` and on the idle `Sweep`;
//! - `browser_open` to a signed-in site is credentialed, to another site not (D14).

use std::sync::Arc;
use std::time::{Duration, Instant};

use browser::{BrowserConfig, Browsers, Call, Close, Sweep};
use kameo::actor::{ActorRef, Spawn};
use runtime::SessionId;
use serde_json::{Value, json};
use tools::{DynTool, ToolCx, ToolOutput};
use webprofile::{Cookie, CookieChange, OriginStorage, Push, WebProfileActor};

/// Tool calls of one session.
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

/// Wait until the profile row of `site` holds cookie `name`; the row's cookie of that name.
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

    // The user signed in on the tile: the app pushed the jar (209 §5 steps 1-3).
    profile
        .push(Push {
            writer: "tile:ipad".into(),
            epoch: 0,
            changes: vec![
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
            ],
            storage: vec![OriginStorage {
                origin: "https://app.example.com".into(),
                local_storage: vec![("token-key".into(), "storage-value".into())],
                captured_ms: 1,
            }],
        })
        .await
        .expect("tile push");

    let rt = runtime::Runtime::new(figment::Figment::new(), dir.path().to_path_buf());
    let actor = Browsers::spawn(Browsers::new(cfg.clone()));
    let kit = Kit {
        tools: browser::tools::all(actor.clone(), cfg, rt),
        data_dir: dir.path().to_path_buf(),
    };

    // Session A's first browser_* call loads the profile into its isolate (209 D4).
    let a = SessionId::new();
    kit.call(a, "browser_tabs", json!({ "action": "list" }))
        .await;
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

    // D14: open to the signed-in site is credentialed, to another site not.
    let open = kit.tool("browser_open");
    assert!(open.credentialed(a, &json!({ "url": "https://app.example.com/orders" })));
    assert!(!open.credentialed(a, &json!({ "url": "https://unrelated.test/" })));

    // A private-id isolate (render provider, scrape slots) never gets the profile.
    let private = SessionId::new();
    let bare = export(&actor, private).await;
    assert_eq!(bare["cookies"], json!([]), "private isolate has no cookies");

    // Save on Close: a cookie the site set reaches the profile; the loaded `__Host-` cookie comes
    // back unchanged (still host-only) rather than rewritten as a domain cookie.
    site_sets(&actor, a, "fresh", "example.com").await;
    actor.tell(Close { session: a }).await.unwrap();
    let fresh = saved(&profile, "example.com", "fresh").await;
    assert_eq!(fresh.domain, "example.com");
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

    // Save on the idle Sweep.
    let b = SessionId::new();
    kit.call(b, "browser_tabs", json!({ "action": "list" }))
        .await;
    assert!(
        named(&export(&actor, b).await, "fresh").is_some(),
        "b loaded a's save"
    );
    site_sets(&actor, b, "swept", "example.org").await;
    actor.tell(Sweep).await.unwrap();
    saved(&profile, "example.org", "swept").await;

    // The private isolate was dropped by the sweep too, and saved nothing.
    let sites = profile.sites().await.unwrap();
    let mut names: Vec<&str> = sites.iter().map(|s| s.site.as_str()).collect();
    names.sort_unstable();
    assert_eq!(names, ["example.com", "example.org"]);
}
