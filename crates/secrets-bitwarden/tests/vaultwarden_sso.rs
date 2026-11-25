//! §9.6 Bitwarden SSO login against Vaultwarden + Keycloak. Ignored by default; run with the
//! recipe of design 221 §9.6 (Keycloak publishes Vaultwarden's port; one network namespace):
//!
//! ```sh
//! docker run -d --rm --name sso-kc -p 127.0.0.1:8081:8081 -p 127.0.0.1:18232:18232 \
//!   -e KC_BOOTSTRAP_ADMIN_USERNAME=admin -e KC_BOOTSTRAP_ADMIN_PASSWORD=admin -e KC_HTTP_PORT=8081 \
//!   quay.io/keycloak/keycloak:26.6.2 start-dev
//! docker exec sso-kc bash -c 'export PATH=/opt/keycloak/bin:$PATH
//!   kcadm.sh config credentials --server http://127.0.0.1:8081 --realm master --user admin --password admin --client admin-cli
//!   kcadm.sh create realms -s realm=test -s enabled=true
//!   kcadm.sh create clients -r test -s clientId=warden -s secret=warden -s "redirectUris=[\"http://127.0.0.1:18232/*\"]"'
//! docker run -d --rm --name sso-vw --network container:sso-kc \
//!   -e ROCKET_PORT=18232 -e DOMAIN=http://127.0.0.1:18232 -e I_REALLY_WANT_VOLATILE_STORAGE=true \
//!   -e SIGNUPS_ALLOWED=true -e SSO_ENABLED=true -e SSO_AUTHORITY=http://127.0.0.1:8081/realms/test \
//!   -e SSO_CLIENT_ID=warden -e SSO_CLIENT_SECRET=warden -e SSO_SIGNUPS_MATCH_EMAIL=true \
//!   -e LOGIN_RATELIMIT_MAX_BURST=1000 -e LOGIN_RATELIMIT_SECONDS=1 vaultwarden/server:1.37.5
//! cargo test -p secrets-bitwarden --test vaultwarden_sso -- --ignored
//! docker stop sso-vw sso-kc
//! ```
//!
//! The browser is a cookie-carrying HTTP client that fills in Keycloak's login form. The tests
//! create their Keycloak users through the admin API.

use std::collections::HashMap;
use std::sync::{Arc, Mutex};

use bitwarden_core::Client;
use bitwarden_crypto::Kdf;
use secrets::external::{BwField, BwKind, ConnStatus, ExtError, ExternalBackend, Location, Value};
use secrets_bitwarden::sso::NO_MASTER_PASSWORD;
use secrets_bitwarden::{
    BwBackend, BwConfig, CredentialStore, KEY_REFRESH_TOKEN, LoginKind, LoginStep, Server,
    TwoFactorKind,
};
use zeroize::Zeroizing;

const PASSWORD: &str = "correct horse battery staple 221";
const IDP_PASSWORD: &str = "idp-pass-221";

fn vw() -> String {
    std::env::var("BW_SSO_VAULTWARDEN_URL").unwrap_or_else(|_| "http://127.0.0.1:18232".into())
}

fn kc() -> String {
    std::env::var("BW_SSO_KEYCLOAK_URL").unwrap_or_else(|_| "http://127.0.0.1:8081".into())
}

fn email() -> String {
    format!("sso-{}@example.org", &uuid::Uuid::new_v4().simple().to_string()[..8])
}

#[derive(Default)]
struct MemStore(Mutex<HashMap<String, String>>);

#[async_trait::async_trait]
impl CredentialStore for MemStore {
    async fn load(&self, key: &str) -> Option<Zeroizing<String>> {
        self.0.lock().unwrap().get(key).cloned().map(Zeroizing::new)
    }
    async fn save(&self, key: &str, value: Option<&str>) {
        let mut m = self.0.lock().unwrap();
        match value {
            Some(v) => m.insert(key.to_owned(), v.to_owned()),
            None => m.remove(key),
        };
    }
}

fn backend(email: &str, store: Arc<MemStore>) -> Arc<BwBackend> {
    BwBackend::with_store(
        "work",
        BwConfig {
            server: Server::SelfHosted,
            base_url: Some(vw()),
            email: Some(email.to_owned()),
            login: LoginKind::Sso,
            ..Default::default()
        },
        Some(store),
    )
}

/// A Keycloak user in realm `test` with [`IDP_PASSWORD`].
async fn idp_user(email: &str) {
    let http = reqwest::Client::new();
    let tok: serde_json::Value = http
        .post(format!("{}/realms/master/protocol/openid-connect/token", kc()))
        .form(&[
            ("grant_type", "password"),
            ("client_id", "admin-cli"),
            ("username", "admin"),
            ("password", "admin"),
        ])
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    let r = http
        .post(format!("{}/admin/realms/test/users", kc()))
        .bearer_auth(tok["access_token"].as_str().unwrap())
        .json(&serde_json::json!({
            "username": email, "email": email, "emailVerified": true, "enabled": true,
            "firstName": "s", "lastName": "s",
            "credentials": [{ "type": "password", "value": IDP_PASSWORD, "temporary": false }],
        }))
        .send()
        .await
        .unwrap();
    assert!(r.status().is_success(), "keycloak user: {}", r.status());
}

/// A Vaultwarden account with a master password; returns a web access token and the hash (to
/// turn on 2FA).
async fn vw_account(email: &str) -> (String, String) {
    let http = reqwest::Client::new();
    let kdf = Kdf::default_pbkdf2();
    let Kdf::PBKDF2 { iterations } = &kdf else {
        unreachable!()
    };
    let iterations = iterations.get();
    let keys = Client::new(None)
        .auth()
        .make_register_keys(email.to_owned(), PASSWORD.into(), kdf)
        .unwrap();
    let hash = keys.master_password_hash.to_string();
    let token = http
        .post(format!("{}/identity/accounts/register/send-verification-email", vw()))
        .json(&serde_json::json!({ "email": email, "name": "sso", "receiveMarketingEmails": false }))
        .send()
        .await
        .unwrap()
        .text()
        .await
        .unwrap();
    let r = http
        .post(format!("{}/identity/accounts/register/finish", vw()))
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
    assert!(r.status().is_success(), "register: {}", r.text().await.unwrap());
    let tok: serde_json::Value = http
        .post(format!("{}/identity/connect/token", vw()))
        .form(&[
            ("grant_type", "password"),
            ("username", email),
            ("password", hash.as_str()),
            ("scope", "api offline_access"),
            ("client_id", "web"),
            ("deviceType", "10"),
            ("deviceIdentifier", &uuid::Uuid::new_v4().to_string()),
            ("deviceName", "sso-suite"),
        ])
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    (tok["access_token"].as_str().expect("access token").to_owned(), hash)
}

/// Turn on authenticator 2FA; returns the seed.
async fn enable_totp(access: &str, hash: &str) -> String {
    let http = reqwest::Client::new();
    let get: serde_json::Value = http
        .post(format!("{}/api/two-factor/get-authenticator", vw()))
        .bearer_auth(access)
        .json(&serde_json::json!({ "masterPasswordHash": hash }))
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    let seed = get["authenticator"]["key"].as_str().unwrap().to_owned();
    let code = bitwarden_vault::generate_totp(seed.clone(), None).unwrap().code;
    let r = http
        .post(format!("{}/api/two-factor/authenticator", vw()))
        .bearer_auth(access)
        .json(&serde_json::json!({
            "key": seed, "token": code, "masterPasswordHash": hash,
            "userVerificationToken": get["userVerificationToken"],
        }))
        .send()
        .await
        .unwrap();
    assert!(r.status().is_success(), "enable totp: {}", r.text().await.unwrap());
    seed
}

/// The browser: open `authorize_url`, sign in at Keycloak, stop at the `http://localhost:806x`
/// redirect and return that address (what a user would paste). Cookies are kept by name across
/// both hosts (Keycloak's are `Secure`; this is plain http on loopback).
async fn browse(authorize_url: &str, user: &str) -> String {
    let http = reqwest::Client::builder()
        .redirect(reqwest::redirect::Policy::none())
        .build()
        .unwrap();
    let mut jar: HashMap<String, String> = HashMap::new();
    let mut url = authorize_url.to_owned();
    let mut form: Option<Vec<(String, String)>> = None;
    for _ in 0..20 {
        if url.starts_with("http://localhost:806") {
            return url;
        }
        let cookie = jar
            .iter()
            .map(|(k, v)| format!("{k}={v}"))
            .collect::<Vec<_>>()
            .join("; ");
        let req = match form.take() {
            Some(f) => http.post(&url).form(&f),
            None => http.get(&url),
        };
        let r = req.header("Cookie", cookie).send().await.unwrap();
        for c in r.headers().get_all("set-cookie") {
            let c = c.to_str().unwrap();
            if let Some((k, v)) = c.split(';').next().and_then(|kv| kv.split_once('=')) {
                jar.insert(k.trim().to_owned(), v.to_owned());
            }
        }
        let status = r.status();
        if status.is_redirection() {
            let loc = r.headers()["location"].to_str().unwrap().to_owned();
            url = reqwest::Url::parse(&url).unwrap().join(&loc).unwrap().to_string();
            continue;
        }
        let body = r.text().await.unwrap();
        let action = body
            .split("action=\"")
            .nth(1)
            .and_then(|s| s.split('"').next())
            .unwrap_or_else(|| panic!("no login form at {url} ({status}): {body:.300}"))
            .replace("&amp;", "&");
        url = action;
        form = Some(vec![
            ("username".into(), user.into()),
            ("password".into(), IDP_PASSWORD.into()),
            ("credentialId".into(), String::new()),
        ]);
    }
    panic!("the SSO flow never came back to localhost");
}

/// SSO with a master-password account and authenticator 2FA: begin → browser → complete asks for
/// the factor; a wrong code keeps the attempt; the right one logs in (`Locked`, refresh token
/// stored); the master password unlocks and the vault works.
#[tokio::test(flavor = "multi_thread")]
#[ignore = "needs Vaultwarden + Keycloak (see the module docs)"]
async fn sso_login_with_2fa_then_master_password_unlock() {
    let email = email();
    let (access, hash) = vw_account(&email).await;
    let seed = enable_totp(&access, &hash).await;
    idp_user(&email).await;
    let store = Arc::new(MemStore::default());
    let b = backend(&email, store.clone());
    assert_eq!(b.status(), ConnStatus::LoggedOut);

    let begun = b.sso_begin("acme", Some(8066)).await.unwrap();
    assert_eq!(begun.redirect_port, 8066);
    let landed = browse(&begun.authorize_url, &email).await;
    assert!(landed.starts_with("http://localhost:8066"), "{landed}");

    let step = b.sso_complete(&begun.state, &landed, None).await.unwrap();
    assert_eq!(
        step,
        LoginStep::NeedTwoFactor {
            providers: vec![TwoFactorKind::Authenticator]
        }
    );
    let wrong = b
        .sso_complete(
            &begun.state,
            &landed,
            Some((TwoFactorKind::Authenticator, Zeroizing::new("000000".into()))),
        )
        .await;
    assert!(matches!(wrong, Err(ExtError::Denied(_))), "{wrong:?}");
    // The enabling code is spent; the next time step's code is inside the drift window.
    let next = bitwarden_vault::generate_totp(
        seed,
        Some(chrono::Utc::now() + chrono::Duration::seconds(30)),
    )
    .unwrap()
    .code;
    let step = b
        .sso_complete(
            &begun.state,
            &landed,
            Some((TwoFactorKind::Authenticator, Zeroizing::new(next))),
        )
        .await
        .unwrap();
    assert_eq!(step, LoginStep::Done);
    assert_eq!(b.status(), ConnStatus::Locked);
    assert!(store.load(KEY_REFRESH_TOKEN).await.is_some());
    // The attempt is over: the same answer cannot log in twice.
    assert!(b.sso_complete(&begun.state, &landed, None).await.is_err());

    assert_eq!(
        b.unlock(Zeroizing::new(PASSWORD.into())).await.unwrap(),
        ConnStatus::Ready
    );
    let loc = Location::Bw {
        org: None,
        collections: vec![],
        folder: None,
        kind: BwKind::SecureNote,
        item_name: "SSO note".into(),
        field: BwField::Notes,
        username: None,
        uri: None,
    };
    let (r, _) = b.create(&loc, Value::Plain("via sso")).await.unwrap();
    assert_eq!(b.get(&r).await.unwrap().0.as_str(), "via sso");
}

/// An SSO user Vaultwarden creates on the spot has no master password: refused at login, nothing
/// stored, still logged out.
#[tokio::test(flavor = "multi_thread")]
#[ignore = "needs Vaultwarden + Keycloak (see the module docs)"]
async fn sso_account_without_master_password_is_refused() {
    let email = email();
    idp_user(&email).await;
    let store = Arc::new(MemStore::default());
    let b = backend(&email, store.clone());
    let begun = b.sso_begin("acme", None).await.unwrap();
    let landed = browse(&begun.authorize_url, &email).await;
    let r = b.sso_complete(&begun.state, &landed, None).await;
    assert!(
        matches!(&r, Err(ExtError::Unsupported(m)) if m == NO_MASTER_PASSWORD),
        "{r:?}"
    );
    assert!(store.load(KEY_REFRESH_TOKEN).await.is_none());
    assert_eq!(b.status(), ConnStatus::LoggedOut);
}

/// The pasted address of one attempt does not complete another.
#[tokio::test(flavor = "multi_thread")]
#[ignore = "needs Vaultwarden + Keycloak (see the module docs)"]
async fn an_older_attempts_address_is_refused() {
    let email = email();
    idp_user(&email).await;
    let b = backend(&email, Arc::new(MemStore::default()));
    let first = b.sso_begin("acme", None).await.unwrap();
    let landed = browse(&first.authorize_url, &email).await;
    let second = b.sso_begin("acme", None).await.unwrap();
    let r = b.sso_complete(&second.state, &landed, None).await;
    assert!(
        matches!(&r, Err(ExtError::Denied(m)) if m.contains("another login attempt")),
        "{r:?}"
    );
}
