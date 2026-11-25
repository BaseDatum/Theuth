//! Bitwarden Sends against a local Vaultwarden (221 §13 B5c/B5d). Ignored by default; run against
//! the container of `tests/vaultwarden.rs` (it must have `SENDS_ALLOWED=true` and
//! `USER_SEND_LIMIT=2048`):
//!
//! ```sh
//! BW_VAULTWARDEN_URL=http://127.0.0.1:18221 cargo test -p secrets-bitwarden --test vaultwarden_send -- --ignored
//! ```

use bitwarden_auth::send_access::{
    SendAccessCredentials, SendAccessTokenRequest, SendPasswordCredentials,
};
use bitwarden_core::{Client, ClientSettings};
use bitwarden_crypto::Kdf;
use bitwarden_send::SendReceiveClient;
use chrono::{Duration, Utc};
use secrets::external::{ConnStatus, ExtError};
use secrets::tools_send::{SendContent, SendOptions};
use secrets_bitwarden::{ApiKey, BwBackend, BwConfig, Server};
use zeroize::Zeroizing;

const PASSWORD: &str = "correct horse battery staple 221";

fn base() -> String {
    std::env::var("BW_VAULTWARDEN_URL").unwrap_or_else(|_| "http://127.0.0.1:18221".into())
}

/// A fresh user's API key (same fixture as `tests/vaultwarden.rs`).
async fn register() -> ApiKey {
    let base = base();
    let email = format!("b5d-{}@example.org", &uuid::Uuid::new_v4().to_string()[..8]);
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
        .json(
            &serde_json::json!({ "email": email, "name": "b5d", "receiveMarketingEmails": false }),
        )
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
            ("deviceName", "b5d-suite"),
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

async fn open() -> std::sync::Arc<BwBackend> {
    let b = BwBackend::new(
        "vw",
        BwConfig {
            server: Server::SelfHosted,
            base_url: Some(base()),
            ..Default::default()
        },
    );
    b.set_api_key(register().await).await;
    assert_eq!(
        b.unlock(Zeroizing::new(PASSWORD.into())).await.unwrap(),
        ConnStatus::Ready
    );
    b
}

fn opts(password: Option<&str>) -> SendOptions {
    SendOptions {
        name: "Shared from theuth".into(),
        notes: None,
        expiration: Some(Utc::now() + Duration::hours(1)),
        deletion: Utc::now() + Duration::days(1),
        max_access_count: Some(1),
        password: password.map(|p| Zeroizing::new(p.to_owned())),
        hide_email: true,
    }
}

/// `(access_id, url key)` from a link `…/#/send/<access_id>/<key>`.
fn parts(link: &str) -> (String, String) {
    let tail = link.split("/#/send/").nth(1).expect("send link");
    let (a, k) = tail.split_once('/').unwrap();
    (a.to_owned(), k.to_owned())
}

fn receiver() -> SendReceiveClient {
    SendReceiveClient::new(Some(ClientSettings {
        identity_url: format!("{}/identity", base()),
        api_url: format!("{}/api", base()),
        ..Default::default()
    }))
}

/// Open the Send as a recipient: an access token (password hashed with the link key), then
/// the access call. `Err` = refused.
async fn access(
    link: &str,
    password: &str,
) -> Result<(String, bitwarden_send::SendAccessResponse), String> {
    let (access_id, key) = parts(link);
    let rx = receiver();
    let hash = rx
        .hash_send_password(key.clone(), password.into())
        .map_err(|e| e.to_string())?;
    let token = rx
        .request_send_access_token(SendAccessTokenRequest {
            send_id: access_id,
            send_access_credentials: Some(SendAccessCredentials::Password(
                SendPasswordCredentials {
                    password_hash_b64: hash,
                },
            )),
        })
        .await
        .map_err(|e| e.to_string())?;
    let resp = rx
        .access_send(token.token.clone())
        .await
        .map_err(|e| e.to_string())?;
    Ok((token.token, resp))
}

#[tokio::test(flavor = "multi_thread")]
#[ignore = "needs a local Vaultwarden (see the module docs)"]
async fn text_send_opens_once_then_revoke() {
    let b = open().await;
    let s = b
        .create_send(
            SendContent::Text(Zeroizing::new("the shared text".into())),
            opts(Some("pw-1")),
        )
        .await
        .unwrap();
    assert!(
        s.link
            .starts_with(&format!("{}/#/send/{}/", base(), s.access_id))
    );
    let (_, resp) = access(&s.link, "pw-1").await.unwrap();
    let view = receiver()
        .decrypt_send_access(parts(&s.link).1, resp)
        .unwrap();
    assert_eq!(
        view.text.and_then(|t| t.text).as_deref(),
        Some("the shared text")
    );
    // max 1 open.
    assert!(access(&s.link, "pw-1").await.is_err());

    let listed = b.list_sends().await.unwrap();
    let info = listed.iter().find(|i| i.send_id == s.send_id).unwrap();
    assert_eq!(info.kind, "text");
    assert!(info.has_password);

    let again = b
        .create_send(
            SendContent::Text(Zeroizing::new("second".into())),
            opts(Some("pw-2")),
        )
        .await
        .unwrap();
    b.delete_send(&again.send_id).await.unwrap();
    assert!(access(&again.link, "pw-2").await.is_err());
    assert_eq!(b.delete_send(&again.send_id).await, Err(ExtError::NotFound));
}

#[tokio::test(flavor = "multi_thread")]
#[ignore = "needs a local Vaultwarden (see the module docs)"]
async fn file_send_round_trip_and_quota() {
    let b = open().await;
    let body: Vec<u8> = (0..(1 << 20)).map(|i| (i % 251) as u8).collect();
    let s = b
        .create_send(
            SendContent::File {
                name: "one-mb.bin".into(),
                bytes: Zeroizing::new(body.clone()),
            },
            opts(Some("file-pw")),
        )
        .await
        .unwrap();
    let (token, resp) = access(&s.link, "file-pw").await.unwrap();
    let file_id = resp.file.as_ref().and_then(|f| f.id.clone()).unwrap();
    let rx = receiver();
    let dl = rx.get_file_download_data(token, file_id).await.unwrap();
    let url = dl.url.unwrap();
    let url = if url.starts_with("http") {
        url
    } else {
        format!("{}{url}", base())
    };
    let enc = reqwest::get(url).await.unwrap().bytes().await.unwrap();
    let plain = rx.decrypt_send_access_file(parts(&s.link).1, &enc).unwrap();
    assert_eq!(plain, body);

    // USER_SEND_LIMIT=2048 KB: 3 MB more does not fit; the server refuses and no Send stays.
    let before = b.list_sends().await.unwrap().len();
    let big = b
        .create_send(
            SendContent::File {
                name: "three-mb.bin".into(),
                bytes: Zeroizing::new(vec![7u8; 3 << 20]),
            },
            opts(None),
        )
        .await;
    let Err(e) = big else {
        panic!("a 3 MB Send over a 2 MB quota was accepted")
    };
    eprintln!("quota refusal: {e}");
    assert!(matches!(e, ExtError::Denied(_) | ExtError::Sdk(_)), "{e:?}");
    assert_eq!(b.list_sends().await.unwrap().len(), before);

    b.delete_send(&s.send_id).await.unwrap();
    assert!(access(&s.link, "file-pw").await.is_err());
}

/// Against a container started with `SENDS_ALLOWED=false` (set `BW_SENDS_DISABLED=1`).
#[tokio::test(flavor = "multi_thread")]
#[ignore = "needs a local Vaultwarden with SENDS_ALLOWED=false"]
async fn sends_disabled_is_mapped() {
    if std::env::var("BW_SENDS_DISABLED").is_err() {
        return;
    }
    let b = open().await;
    let e = b
        .create_send(SendContent::Text(Zeroizing::new("x".into())), opts(None))
        .await
        .err()
        .expect("a Send on a server with Sends disabled");
    assert!(
        matches!(&e, ExtError::Denied(m) if m.contains("SENDS_ALLOWED")),
        "{e:?}"
    );
}
