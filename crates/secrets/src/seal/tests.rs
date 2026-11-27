//! Gate tests: two engines on temp data dirs talking through an in-process [`PeerCall`] that
//! stamps the caller's id the way `theuth/rpc/1`'s forwarded marker does.

use std::collections::HashMap;
use std::sync::Mutex;

use super::*;
use crate::envelope::tests::MemKeychain;

const PW: &str = "correct horse battery staple";

#[derive(Default)]
struct Net {
    gates: Mutex<HashMap<String, Arc<Gate>>>,
    /// Every `vault_peer_grant` sent: (to, params).
    grants: Mutex<Vec<(String, serde_json::Value)>>,
}

struct Via {
    me: String,
    net: Arc<Net>,
}

impl PeerCall for Via {
    fn call<'a>(
        &'a self,
        peer: &'a str,
        method: &'static str,
        params: serde_json::Value,
    ) -> futures::future::BoxFuture<'a, Result<Option<serde_json::Value>, String>> {
        Box::pin(async move {
            let Some(g) = self.net.gates.lock().unwrap().get(peer).cloned() else {
                return Ok(None);
            };
            let s = |i: usize| params[i].as_str().unwrap_or_default().to_owned();
            match method {
                "vault_peer_ask" => g
                    .on_ask(&self.me, &s(0), &s(1), &s(2))
                    .await
                    .map(|r| Some(serde_json::to_value(r).unwrap()))
                    .map_err(|e| e.to_string()),
                "vault_peer_grant" => {
                    self.net.grants.lock().unwrap().push((peer.to_owned(), params.clone()));
                    g.on_grant(&self.me, &s(0), &s(1), &s(2), &s(3))
                        .await
                        .map(|()| Some(serde_json::Value::Null))
                        .map_err(|e| e.to_string())
                }
                _ => Err("method not found".into()),
            }
        })
    }
}

async fn engine(net: &Arc<Net>, id: &str, dir: &Path, ks: KeyStore) -> Arc<Gate> {
    let g = Gate::boot_with(dir, ks, &MemKeychain { fail: true, ..Default::default() })
        .await
        .unwrap();
    g.set_identity(id, id);
    g.set_peers(Arc::new(Via { me: id.to_owned(), net: net.clone() }));
    net.gates.lock().unwrap().insert(id.to_owned(), g.clone());
    g
}

fn pw(s: &str) -> Zeroizing<String> {
    Zeroizing::new(s.to_owned())
}

async fn until(mut f: impl FnMut() -> bool) {
    for _ in 0..200 {
        if f() {
            return;
        }
        tokio::time::sleep(Duration::from_millis(20)).await;
    }
    panic!("condition not reached");
}

#[tokio::test]
async fn file_key_engine_migrates_warns_then_a_password_replaces_the_file() {
    let net = Arc::new(Net::default());
    let dir = tempfile::tempdir().unwrap();
    let g = engine(&net, "node", dir.path(), KeyStore::File).await;
    let s = g.status();
    assert_eq!(s.state, "unsealed");
    assert_eq!(s.unsealed_by.as_deref(), Some("file"));
    assert!(s.warning.unwrap().contains("unprotected on disk"));
    g.require().unwrap().set("API", "sekrit-value-123", None, "test").await.unwrap();

    assert!(matches!(g.add_password(pw("short")).await, Err(SealError::Refused(_))));
    g.add_password(pw(PW)).await.unwrap();
    let s = g.status();
    assert!(s.warning.is_none(), "{:?}", s.warning);
    assert!(s.password_set);
    assert_eq!(s.methods.len(), 1, "the file method went");
    assert!(!dir.path().join(crate::key::FILE).exists(), "secrets.key shredded");
    assert!(matches!(g.remove_method("password").await, Err(SealError::Refused(_))), "never the last");

    // Restart: sealed, reads refused with the typed error, the password opens it.
    drop(g);
    net.gates.lock().unwrap().clear();
    let g = engine(&net, "node", dir.path(), KeyStore::Auto).await;
    assert!(g.is_sealed());
    match g.require() {
        Err(SealError::Sealed(t)) => assert!(t.starts_with("the vault is sealed on node"), "{t}"),
        other => panic!("{other:?}"),
    }
    assert_eq!(g.status().state, "sealed");
    assert_eq!(g.unseal_password(pw("not the password!!")).await, Err(SealError::WrongPassword));
    g.unseal_password(pw(PW)).await.unwrap();
    assert_eq!(g.require().unwrap().get("API", 0).unwrap().as_str(), "sekrit-value-123");
    assert_eq!(g.status().unsealed_by.as_deref(), Some("password"));
}

#[tokio::test]
async fn wrong_passwords_are_throttled() {
    let net = Arc::new(Net::default());
    let dir = tempfile::tempdir().unwrap();
    envelope::init_password(dir.path(), PW).unwrap();
    let g = engine(&net, "node", dir.path(), KeyStore::Auto).await;
    for _ in 0..FREE_ATTEMPTS {
        assert_eq!(g.unseal_password(pw("wrong password here")).await, Err(SealError::WrongPassword));
    }
    assert!(matches!(g.unseal_password(pw(PW)).await, Err(SealError::Throttled(_))));
}

#[tokio::test]
async fn change_password_needs_the_current_one() {
    let net = Arc::new(Net::default());
    let dir = tempfile::tempdir().unwrap();
    let g = engine(&net, "node", dir.path(), KeyStore::Sealed).await;
    assert!(g.status().warning.unwrap().contains("memory only"));
    g.add_password(pw(PW)).await.unwrap();
    assert!(matches!(g.add_password(pw(PW)).await, Err(SealError::Refused(_))));
    assert_eq!(
        g.change_password(pw("not the current one"), pw("a brand new password")).await,
        Err(SealError::WrongPassword)
    );
    g.change_password(pw(PW), pw("a brand new password")).await.unwrap();
    let f = envelope::load(dir.path()).unwrap().unwrap();
    assert!(envelope::open_password(&f, PW).unwrap().is_none());
    assert!(envelope::open_password(&f, "a brand new password").unwrap().is_some());
}

#[tokio::test]
async fn seal_and_unseal_a_running_vault() {
    let net = Arc::new(Net::default());
    let dir = tempfile::tempdir().unwrap();
    let g = engine(&net, "node", dir.path(), KeyStore::File).await;
    assert!(matches!(g.seal(), Err(SealError::Refused(_))), "nothing could unlock it");
    g.add_password(pw(PW)).await.unwrap();
    let v = g.require().unwrap();
    v.set("TOKEN", "a-long-token-value", None, "test").await.unwrap();
    g.seal().unwrap();
    assert!(g.is_sealed());
    // A reader that kept the Arc gets nothing, and expansion fails with the sealed text.
    assert!(v.get("TOKEN", 0).is_none());
    let mut args = serde_json::json!({"h": "Bearer {{secret:TOKEN}}"});
    let err = v.expand(&mut args).unwrap_err();
    assert!(err.contains("is sealed"), "{err}");
    g.unseal_password(pw(PW)).await.unwrap();
    assert_eq!(v.get("TOKEN", 0).unwrap().as_str(), "a-long-token-value");
}

#[tokio::test]
async fn device_unlock_end_to_end_with_prompt_then_automatic() {
    let net = Arc::new(Net::default());
    let (od, nd) = (tempfile::tempdir().unwrap(), tempfile::tempdir().unwrap());
    let owner = engine(&net, "owner", od.path(), KeyStore::Sealed).await;
    owner.add_password(pw(PW)).await.unwrap();
    let node = engine(&net, "node", nd.path(), KeyStore::File).await;
    node.require().unwrap().set("API", "node-secret-value", None, "test").await.unwrap();

    // Outside the pairing grace an enrolment prompts on the owner.
    assert!(!node.add_device("owner").await.unwrap(), "pending");
    let req = owner.requests();
    assert_eq!(req.len(), 1);
    assert_eq!((req[0].purpose.as_str(), req[0].node.as_str()), ("enroll", "node"));
    assert!(owner.answer(&req[0].id, true, false).await.unwrap());
    assert!(!owner.answer(&req[0].id, true, false).await.unwrap(), "answered once");
    let s = node.status();
    assert!(s.methods.iter().any(|m| m.kind == Kind::Device && m.id == "owner" && m.label == "Unlock from owner"));
    assert!(s.warning.is_none(), "device method replaced the key file");
    assert_eq!(owner.unlock_devices().await.unwrap()[0].node, "node");

    // Restart the node: sealed; it asks, the owner is prompted, approves with "always".
    net.gates.lock().unwrap().remove("node");
    drop(node);
    let node = engine(&net, "node", nd.path(), KeyStore::Auto).await;
    assert!(node.is_sealed());
    let asker = tokio::spawn(node.clone().ask_until_open());
    until(|| !owner.requests().is_empty()).await;
    assert_eq!(node.status().asking, vec!["owner".to_owned()]);
    let req = owner.requests();
    assert_eq!(req[0].purpose, "unlock");
    assert!(owner.answer(&req[0].id, true, true).await.unwrap());
    assert!(!node.is_sealed());
    assert_eq!(node.require().unwrap().get("API", 0).unwrap().as_str(), "node-secret-value");
    assert_eq!(node.status().unsealed_by.as_deref(), Some("device"));
    asker.await.unwrap();

    // Replay: the captured grant does nothing on a later sealed boot.
    let (to, replay) = net.grants.lock().unwrap().last().cloned().unwrap();
    assert_eq!(to, "node");
    net.gates.lock().unwrap().remove("node");
    drop(node);
    let node = engine(&net, "node", nd.path(), KeyStore::Auto).await;
    let s = |i: usize| replay[i].as_str().unwrap().to_owned();
    assert!(matches!(
        node.on_grant("owner", &s(0), &s(1), &s(2), "").await,
        Err(SealError::Refused(_))
    ));
    assert!(node.is_sealed());
    // A grant for a live nonce from the wrong engine is refused, and burns nothing for the owner.
    let nonce = node.mint_ask("owner", "unlock");
    assert!(matches!(node.on_grant("mallory", &nonce, "unlock", &s(2), "").await, Err(SealError::Refused(_))));

    // "Always": the next ask is answered without a prompt.
    assert!(node.ask("owner", "unlock").await.unwrap());
    assert!(!node.is_sealed());
    assert!(owner.requests().is_empty());

    // Forgotten by the owner: refused.
    owner.forget_node("node").await.unwrap();
    let node2 = tempfile::tempdir().unwrap();
    let other = engine(&net, "node", node2.path(), KeyStore::File).await;
    assert!(matches!(other.ask("owner", "unlock").await, Err(SealError::Refused(_))));
}

#[tokio::test]
async fn pairing_pre_approves_enrolment_and_a_sealed_owner_answers_sealed() {
    let net = Arc::new(Net::default());
    let (od, nd) = (tempfile::tempdir().unwrap(), tempfile::tempdir().unwrap());
    let owner = engine(&net, "owner", od.path(), KeyStore::Sealed).await;
    owner.add_password(pw(PW)).await.unwrap();
    let node = engine(&net, "node", nd.path(), KeyStore::Sealed).await;
    lock(&owner.paired).insert("node".into(), now_ms());
    assert!(node.add_device("owner").await.unwrap(), "approved inside the pairing grace");
    assert!(owner.requests().is_empty());
    assert_eq!(node.status().methods.len(), 1);

    owner.seal().unwrap();
    net.gates.lock().unwrap().remove("node");
    drop(node);
    let node = engine(&net, "node", nd.path(), KeyStore::Auto).await;
    match node.ask("owner", "unlock").await {
        Err(SealError::Refused(t)) => assert!(t.contains("sealed"), "{t}"),
        other => panic!("{other:?}"),
    }
}
