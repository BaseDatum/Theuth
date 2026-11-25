//! Against a local Vaultwarden (221 §13 B5). Ignored by default; run with
//!
//! ```sh
//! docker run -d --name b221-vaultwarden -e SIGNUPS_ALLOWED=true -e SENDS_ALLOWED=true \
//!   -p 127.0.0.1:18221:80 vaultwarden/server:1.37.5
//! BW_VAULTWARDEN_URL=http://127.0.0.1:18221 cargo test -p secrets-bitwarden -- --ignored
//! ```
//!
//! Plain http works in debug builds only (the SDK's client is `https_only` in release).

use bitwarden_core::Client;
use bitwarden_crypto::Kdf;
use secrets::external::{
    BwField, BwKind, ConnStatus, DeleteMode, ExtError, ExtRef, ExternalBackend, ListQuery,
    Location, Value,
};
use secrets_bitwarden::{ApiKey, BwBackend, BwConfig, Server};
use zeroize::Zeroizing;

const PASSWORD: &str = "correct horse battery staple 221";

fn base() -> String {
    std::env::var("BW_VAULTWARDEN_URL").unwrap_or_else(|_| "http://127.0.0.1:18221".into())
}

/// Register a fresh user (Vaultwarden's two-step registration; without SMTP the verification
/// token is the response body) and fetch its personal API key.
async fn register() -> ApiKey {
    let base = base();
    let email = format!("b5-{}@example.org", &uuid::Uuid::new_v4().to_string()[..8]);
    let http = reqwest::Client::new();
    let kdf = Kdf::default_pbkdf2();
    let Kdf::PBKDF2 { iterations } = &kdf else {
        unreachable!()
    };
    let iterations = iterations.get();
    let keys = Client::new(None)
        .auth()
        .make_register_keys(email.clone(), PASSWORD.into(), kdf)
        .unwrap();
    let hash = keys.master_password_hash.to_string();
    let token = http
        .post(format!(
            "{base}/identity/accounts/register/send-verification-email"
        ))
        .json(&serde_json::json!({ "email": email, "name": "b5", "receiveMarketingEmails": false }))
        .send()
        .await
        .unwrap()
        .text()
        .await
        .unwrap();
    let r = http
        .post(format!("{base}/identity/accounts/register/finish"))
        .json(&serde_json::json!({
            "email": email, "emailVerificationToken": token.trim_matches('"'),
            "masterPasswordHash": hash, "masterPasswordHint": null,
            "userSymmetricKey": keys.encrypted_user_key.to_string(),
            "kdf": 0, "kdfIterations": iterations,
            "userAsymmetricKeys": {
                "publicKey": keys.keys.public.to_string(),
                "encryptedPrivateKey": keys.keys.private.to_string(),
            },
        }))
        .send()
        .await
        .unwrap();
    assert!(
        r.status().is_success(),
        "register: {}",
        r.text().await.unwrap()
    );
    let tok: serde_json::Value = http
        .post(format!("{base}/identity/connect/token"))
        .form(&[
            ("grant_type", "password"),
            ("username", email.as_str()),
            ("password", hash.as_str()),
            ("scope", "api offline_access"),
            ("client_id", "web"),
            ("deviceType", "10"),
            ("deviceIdentifier", &uuid::Uuid::new_v4().to_string()),
            ("deviceName", "b5-suite"),
        ])
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    let access = tok["access_token"].as_str().expect("access token");
    let profile: serde_json::Value = http
        .get(format!("{base}/api/accounts/profile"))
        .bearer_auth(access)
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    let key: serde_json::Value = http
        .post(format!("{base}/api/accounts/api-key"))
        .bearer_auth(access)
        .json(&serde_json::json!({ "masterPasswordHash": hash }))
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    ApiKey {
        client_id: format!("user.{}", profile["id"].as_str().unwrap()),
        client_secret: Zeroizing::new(key["apiKey"].as_str().unwrap().to_owned()),
    }
}

fn config(allow_permanent_delete: bool) -> BwConfig {
    BwConfig {
        server: Server::SelfHosted,
        base_url: Some(base()),
        allow_permanent_delete,
        ..Default::default()
    }
}

fn copy(k: &ApiKey) -> ApiKey {
    ApiKey {
        client_id: k.client_id.clone(),
        client_secret: k.client_secret.clone(),
    }
}

async fn open(key: &ApiKey, allow_permanent_delete: bool) -> std::sync::Arc<BwBackend> {
    let b = BwBackend::new("vw", config(allow_permanent_delete));
    b.set_api_key(copy(key)).await;
    assert_eq!(b.status(), ConnStatus::Locked);
    assert_eq!(
        b.unlock(Zeroizing::new(PASSWORD.into())).await.unwrap(),
        ConnStatus::Ready
    );
    b
}

fn with_key(r: &ExtRef, key: Option<&str>) -> ExtRef {
    ExtRef {
        key: key.map(str::to_owned),
        ..r.clone()
    }
}

#[tokio::test(flavor = "multi_thread")]
#[ignore = "needs a local Vaultwarden (see the module docs)"]
async fn api_key_unlock_crud_conflict_trash_lock() {
    let key = register().await;

    // A wrong master password is refused and the connection stays locked.
    let b = BwBackend::new("vw", config(false));
    b.set_api_key(copy(&key)).await;
    let wrong = b.unlock(Zeroizing::new("not the password".into())).await;
    assert!(matches!(wrong, Err(ExtError::Denied(_))), "{wrong:?}");
    assert_eq!(b.status(), ConnStatus::Locked);

    let b = open(&key, false).await;
    let loc = Location::Bw {
        org: None,
        collections: vec![],
        folder: None,
        kind: BwKind::Login,
        item_name: "B5 login".into(),
        field: BwField::Password,
        username: Some("svc".into()),
        uri: Some("https://svc.example".into()),
    };
    let (r, v1) = b.create(&loc, Value::Plain("first-pw")).await.unwrap();
    assert_eq!(r.key.as_deref(), Some("password"));
    assert_eq!(b.get(&r).await.unwrap().0.as_str(), "first-pw");
    assert_eq!(
        b.get(&with_key(&r, Some("username")))
            .await
            .unwrap()
            .0
            .as_str(),
        "svc"
    );

    let entry = b.describe(&r).await.unwrap();
    assert_eq!(entry.kind, "login");
    assert!(entry.keys.contains(&"password".to_owned()));
    let page = b
        .list(ListQuery {
            query: Some("b5 LOGIN".into()),
            ..Default::default()
        })
        .await
        .unwrap();
    assert_eq!(page.entries.len(), 1);

    // Revision dates have second-ish resolution on the server; make the next one differ.
    tokio::time::sleep(std::time::Duration::from_millis(1100)).await;
    let v2 = b
        .update(&r, Value::Plain("second-pw"), Some(&v1))
        .await
        .unwrap();
    assert_ne!(v2, v1);
    let stale = b.update(&r, Value::Plain("third-pw"), Some(&v1)).await;
    assert!(
        matches!(&stale, Err(ExtError::Conflict { current }) if *current == v2),
        "{stale:?}"
    );
    assert_eq!(b.get(&r).await.unwrap().0.as_str(), "second-pw");

    // Trash and restore; permanent delete refused by default.
    assert!(matches!(
        b.delete(&r, DeleteMode::Permanent, None).await,
        Err(ExtError::Denied(_))
    ));
    let out = b
        .delete(&r, DeleteMode::Recoverable { days: None }, None)
        .await
        .unwrap();
    assert!(out.recoverable_until.is_some());
    assert_eq!(b.get(&r).await.unwrap_err(), ExtError::NotFound);
    b.restore(&r).await.unwrap();
    assert_eq!(b.get(&r).await.unwrap().0.as_str(), "second-pw");

    // Lock drops the keys.
    b.lock().await;
    assert_eq!(b.status(), ConnStatus::Locked);
    assert_eq!(b.get(&r).await.unwrap_err(), ExtError::Locked);

    // With allow_permanent_delete on, the item is gone for good.
    let p = open(&key, true).await;
    assert_eq!(
        p.delete(&r, DeleteMode::Permanent, None)
            .await
            .unwrap()
            .recoverable_until,
        None
    );
    assert_eq!(p.get(&r).await.unwrap_err(), ExtError::NotFound);
}

#[tokio::test(flavor = "multi_thread")]
#[ignore = "needs a local Vaultwarden (see the module docs)"]
async fn idle_lock() {
    let key = register().await;
    let b = BwBackend::new(
        "vw",
        BwConfig {
            lock_after_idle_secs: 1,
            ..config(false)
        },
    );
    b.set_api_key(copy(&key)).await;
    b.unlock(Zeroizing::new(PASSWORD.into())).await.unwrap();
    assert_eq!(b.status(), ConnStatus::Ready);
    tokio::time::sleep(std::time::Duration::from_millis(1600)).await;
    assert_eq!(b.status(), ConnStatus::Locked);
}
