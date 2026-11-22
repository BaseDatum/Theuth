//! 209 §4.4 over the real RPC methods: `web_profile_push` with a fake client's named params, the
//! caps of one push, `stale-epoch` / `unknown-epoch` with the current epoch as data,
//! `web_profile_sites` with counts only, and `WebProfileChanged` on the bus after a write; and the
//! actor with `browser.profile: off`. The registered actor is process-wide: one test registers.

use std::sync::Arc;

use kameo::actor::Spawn as _;
use parking_lot::Mutex;
use serde_json::{Value, json};
use webprofile::{Profile, WEBPROFILE_ACTOR, WebProfileActor, WebProfileChanged, store};

#[derive(kameo::Actor)]
struct Heard(Arc<Mutex<Vec<WebProfileChanged>>>);

impl kameo::message::Message<WebProfileChanged> for Heard {
    type Reply = ();
    async fn handle(
        &mut self,
        m: WebProfileChanged,
        _: &mut kameo::message::Context<Self, Self::Reply>,
    ) {
        self.0.lock().push(m);
    }
}

async fn call(methods: &jsonrpsee::Methods, method: &str, params: Value) -> Value {
    let req = json!({ "jsonrpc": "2.0", "id": 1, "method": method, "params": params });
    let (resp, _) = methods.raw_json_request(&req.to_string(), 1).await.unwrap();
    serde_json::from_str(resp.get()).unwrap()
}

#[tokio::test]
async fn push_sites_and_stale_epoch_over_rpc() {
    let dir = tempfile::tempdir().unwrap();
    let rt = runtime::Runtime::new(figment::Figment::new(), dir.path().to_path_buf());
    webprofile::declare_wire(&rt).await.unwrap();
    let vault = Arc::new(
        secrets::Vault::open(dir.path(), secrets::KeyStore::File)
            .await
            .unwrap(),
    );
    let profile = Profile::open(&dir.path().join(store::DB), vault)
        .await
        .unwrap();
    let actor = WebProfileActor::spawn(WebProfileActor::new(Some(rt.clone()), Ok(profile)));
    actor.register(WEBPROFILE_ACTOR).unwrap();
    let seen = Arc::new(Mutex::new(Vec::new()));
    let sink = Heard::spawn_with_mailbox(Heard(seen.clone()), runtime::subscriber_mailbox());
    rt.subscribe::<Heard, WebProfileChanged>(&sink).await;

    let mut methods: jsonrpsee::Methods = webprofile::rpc_module(rpc_core::Server::new(rt.clone()))
        .module
        .into();
    let (peer, _guard) = rpc_core::RpcPeer::open();
    methods.extensions_mut().insert(peer);

    let push = call(
        &methods,
        "web_profile_push",
        json!({
            "device": "iPad",
            "epoch": 0,
            "changes": [
                { "op": "set", "cookie": { "name": "__Host-sid", "value": "v1", "domain": "app.example.com",
                  "host_only": true, "path": "/", "expires": null, "secure": true, "http_only": true,
                  "same_site": "Lax", "partitioned": false } },
                { "op": "remove", "name": "old", "domain": ".example.com", "path": "/" }
            ],
            "storage": [ { "origin": "https://app.example.com", "local_storage": [["k", "v"]], "captured_ms": 1 } ]
        }),
    )
    .await;
    assert_eq!(
        push["result"],
        json!({ "applied": 3, "sites": ["example.com"], "epoch": 0, "profile": "shared" }),
        "{push}"
    );

    let sites = call(&methods, "web_profile_sites", json!({})).await;
    let list = sites["result"].as_array().unwrap();
    assert_eq!(list.len(), 1, "{sites}");
    assert_eq!(list[0]["site"], "example.com");
    assert_eq!(list[0]["cookies"], 1);
    assert_eq!(list[0]["origins"], 1);
    assert_eq!(list[0]["writer"], "tile:iPad");
    assert!(list[0]["updated_ms"].as_u64().unwrap() > 0);

    // The caps of one push are checked before any work; the device name is cut to 64 characters.
    let cookie = |i: usize| {
        json!({ "op": "set", "cookie": { "name": format!("c{i}"), "value": "v", "domain": "example.org",
            "path": "/" } })
    };
    let over = call(
        &methods,
        "web_profile_push",
        json!({ "device": "iPad", "epoch": 0, "changes": (0..2001).map(cookie).collect::<Vec<_>>() }),
    )
    .await;
    assert_eq!(
        over["error"]["message"], "invalid push: 2001 changes > 2000",
        "{over}"
    );
    let suffix = call(
        &methods,
        "web_profile_push",
        json!({ "device": "iPad", "epoch": 0, "changes": [
            { "op": "set", "cookie": { "name": "n", "value": "v", "domain": ".co.uk", "path": "/" } } ] }),
    )
    .await;
    assert!(
        suffix["error"]["message"]
            .as_str()
            .unwrap()
            .contains("public suffix"),
        "{suffix}"
    );
    let long = "d".repeat(200);
    let named = call(
        &methods,
        "web_profile_push",
        json!({ "device": long, "epoch": 0, "changes": [cookie(0)] }),
    )
    .await;
    assert_eq!(named["result"]["sites"], json!(["example.org"]), "{named}");
    let sites = call(&methods, "web_profile_sites", json!({})).await;
    let org = sites["result"]
        .as_array()
        .unwrap()
        .iter()
        .find(|s| s["site"] == "example.org")
        .unwrap()
        .clone();
    assert_eq!(org["writer"], format!("tile:{}", "d".repeat(64)));

    // 209 §4.4 `web_profile_pull` on the owner's connection: the site whole, values included,
    // each cookie with its writer.
    let pulled = call(
        &methods,
        "web_profile_pull",
        json!({ "sites": ["app.example.com"] }),
    )
    .await;
    let r = &pulled["result"];
    assert_eq!(r["sites"], json!(["example.com"]), "{pulled}");
    assert_eq!(r["epoch"], 0);
    assert_eq!(r["profile"], "shared");
    assert_eq!(r["cookies"].as_array().unwrap().len(), 1);
    assert_eq!(r["cookies"][0]["name"], "__Host-sid");
    assert_eq!(r["cookies"][0]["value"], "v1");
    assert_eq!(r["cookies"][0]["host_only"], true);
    assert_eq!(r["cookies"][0]["writer"], "tile:iPad");
    assert_eq!(r["cookies"][0]["site"], "example.com");
    assert_eq!(r["storage"][0]["origin"], "https://app.example.com");
    assert_eq!(r["storage"][0]["local_storage"], json!([["k", "v"]]));
    let every = call(&methods, "web_profile_pull", json!({})).await;
    assert_eq!(
        every["result"]["sites"],
        json!(["example.com", "example.org"]),
        "{every}"
    );

    // Pull, forget and clear answer only an owner's connection: nothing authenticated, or a
    // share-link guest, is refused before the actor (and nothing is forgotten or cleared).
    let bare: jsonrpsee::Methods = webprofile::rpc_module(rpc_core::Server::new(rt.clone()))
        .module
        .into();
    let mut as_guest: jsonrpsee::Methods =
        webprofile::rpc_module(rpc_core::Server::new(rt.clone()))
            .module
            .into();
    let (guest, _guest_guard) = rpc_core::RpcPeer::open_guest([7; 32], [9; 32]);
    as_guest.extensions_mut().insert(guest);
    for (who, m) in [("no connection", &bare), ("a guest", &as_guest)] {
        for (method, params) in [
            ("web_profile_pull", json!({})),
            ("web_profile_forget", json!({ "site": "example.org" })),
            ("web_profile_clear", json!({})),
        ] {
            let r = call(m, method, params).await;
            assert_eq!(
                r["error"]["code"],
                rpc_core::UNAUTHORIZED,
                "{who} {method}: {r}"
            );
            assert!(r.get("result").is_none(), "{who} {method}: {r}");
        }
    }

    // Forget one site (any host of it): gone from the list and from a pull, the other stays.
    let forgot = call(
        &methods,
        "web_profile_forget",
        json!({ "site": "www.example.org" }),
    )
    .await;
    assert_eq!(
        forgot["result"],
        json!({ "site": "example.org", "epoch": 0 }),
        "{forgot}"
    );
    let after = call(
        &methods,
        "web_profile_pull",
        json!({ "sites": ["example.org"] }),
    )
    .await;
    assert_eq!(after["result"]["sites"], json!(["example.org"]));
    assert_eq!(after["result"]["cookies"], json!([]), "{after}");
    let sites = call(&methods, "web_profile_sites", json!({})).await;
    assert_eq!(sites["result"].as_array().unwrap().len(), 1, "{sites}");
    let no_site = call(&methods, "web_profile_forget", json!({ "site": " " })).await;
    assert_eq!(
        no_site["error"]["message"], "invalid forget: no site",
        "{no_site}"
    );
    let many = call(
        &methods,
        "web_profile_pull",
        json!({ "sites": vec!["example.com"; 501] }),
    )
    .await;
    assert_eq!(
        many["error"]["message"], "invalid pull: 501 sites > 500",
        "{many}"
    );

    // A client cannot make up an epoch.
    let ahead = call(
        &methods,
        "web_profile_push",
        json!({ "device": "iPad", "epoch": 7, "changes": [] }),
    )
    .await;
    assert_eq!(ahead["error"]["message"], "unknown-epoch", "{ahead}");
    assert_eq!(ahead["error"]["data"], json!({ "epoch": 0 }));

    // Sign out everywhere happened elsewhere: the client's next push is refused with the epoch.
    let p = Profile::open(&dir.path().join(store::DB), {
        Arc::new(
            secrets::Vault::open(dir.path(), secrets::KeyStore::File)
                .await
                .unwrap(),
        )
    })
    .await
    .unwrap();
    p.set_epoch(2).await.unwrap();
    let stale = call(
        &methods,
        "web_profile_push",
        json!({ "device": "iPad", "epoch": 1, "changes": [] }),
    )
    .await;
    assert_eq!(stale["error"]["message"], "stale-epoch", "{stale}");
    assert_eq!(stale["error"]["data"], json!({ "epoch": 2 }));

    // 209 §4.4 `web_profile_clear` over RPC: the epoch goes up; a push under the old one is
    // stale with the new epoch as data.
    let cleared = call(&methods, "web_profile_clear", json!({})).await;
    assert_eq!(cleared["result"], json!({ "epoch": 3 }), "{cleared}");
    let old = call(
        &methods,
        "web_profile_push",
        json!({ "device": "iPad", "epoch": 2, "changes": [] }),
    )
    .await;
    assert_eq!(old["error"]["message"], "stale-epoch", "{old}");
    assert_eq!(old["error"]["data"], json!({ "epoch": 3 }));

    tokio::time::timeout(std::time::Duration::from_secs(5), async {
        while seen.lock().len() < 4 {
            tokio::time::sleep(std::time::Duration::from_millis(20)).await;
        }
    })
    .await
    .expect("WebProfileChanged published");
    // The first push, the long-named device's push, the forget (naming the site) and the clear
    // (the new epoch).
    let heard = seen.lock().clone();
    assert_eq!(
        heard[0],
        WebProfileChanged {
            sites: vec!["example.com".into()],
            epoch: 0
        }
    );
    assert_eq!(
        heard[2],
        WebProfileChanged {
            sites: vec!["example.org".into()],
            epoch: 0
        }
    );
    assert_eq!(heard[3].epoch, 3);
}

/// 209 D6: with `browser.profile: off` a push is a no-op the client can read (`profile: "off"`)
/// and the list is empty — not an error the client would retry forever.
#[tokio::test]
async fn off_answers_empty() {
    let dir = tempfile::tempdir().unwrap();
    let figment = figment::Figment::new().merge(figment::providers::Serialized::defaults(
        json!({ "browser": { "profile": "off" } }),
    ));
    let rt = runtime::Runtime::new(figment, dir.path().to_path_buf());
    assert_eq!(webprofile::mode(&rt), webprofile::ProfileMode::Off);
    let vault = Arc::new(
        secrets::Vault::open(dir.path(), secrets::KeyStore::File)
            .await
            .unwrap(),
    );
    let profile = Profile::open(&dir.path().join(store::DB), vault)
        .await
        .unwrap();
    let actor = WebProfileActor::spawn(WebProfileActor::new(Some(rt), Ok(profile)));
    let wp = webprofile::Handle::from_actor(actor);
    let r = wp
        .push(webprofile::Push {
            writer: "tile:iPad".into(),
            epoch: 0,
            changes: vec![webprofile::CookieChange::Remove {
                name: "n".into(),
                domain: "example.com".into(),
                path: "/".into(),
            }],
            storage: vec![],
            basis_ms: None,
        })
        .await
        .unwrap();
    assert_eq!(
        serde_json::to_value(&r).unwrap(),
        json!({ "applied": 0, "sites": [], "epoch": 0, "profile": "off" })
    );
    assert_eq!(wp.sites().await.unwrap(), vec![]);
    assert!(wp.load(None).await.is_err(), "no load while off");
    let pulled = wp.pull(None).await.unwrap();
    assert_eq!(
        serde_json::to_value(&pulled).unwrap(),
        json!({ "epoch": 0, "sites": [], "cookies": [], "storage": [], "profile": "off" })
    );
    // Off leaves rows until cleared: forget and clear still work.
    assert_eq!(wp.forget("example.com").await.unwrap().site, "example.com");
    assert_eq!(wp.clear().await.unwrap().epoch, 1);
}
