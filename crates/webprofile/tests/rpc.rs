//! 209 §4.4 over the real RPC methods: `web_profile_push` with a fake client's named params,
//! `stale-epoch` with the current epoch as data, `web_profile_sites` with counts only, and
//! `WebProfileChanged` on the bus after a write. One test: the actor registry is process-wide.

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
        json!({ "applied": 3, "sites": ["example.com"], "epoch": 0 }),
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

    tokio::time::timeout(std::time::Duration::from_secs(5), async {
        while seen.lock().is_empty() {
            tokio::time::sleep(std::time::Duration::from_millis(20)).await;
        }
    })
    .await
    .expect("WebProfileChanged published");
    assert_eq!(
        seen.lock()[0],
        WebProfileChanged {
            sites: vec!["example.com".into()],
            epoch: 0
        }
    );
}
