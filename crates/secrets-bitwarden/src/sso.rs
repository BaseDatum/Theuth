//! Bitwarden "Login with SSO" (221 §9.6), for organisations whose members sign in through SSO and
//! keep a master password. The SDK has no SSO login, so this is theuth's own HTTP through the SDK
//! client's `reqwest`, like the password login (`password.rs`):
//!
//! 1. [`BwBackend::sso_begin`]: `GET {identity}/sso/prevalidate?domainHint=<org>` for the
//!    `ssoToken`, a fresh `state` and PKCE verifier (kept here, in memory, for
//!    [`PENDING_TTL`]), and the identity server's `/connect/authorize` URL for the browser. The
//!    engine never opens that URL itself: the browser that signs in must be the one that opened it.
//! 2. [`BwBackend::sso_complete`]: the code (bare, or the whole address the browser landed on) is
//!    exchanged with the `authorization_code` grant. An account without a master password is
//!    refused before anything is stored; otherwise the refresh token is kept and the connection is
//!    `Locked`: the unlock card asks for the master password, as after a password login.
//!
//! The code and the verifier are never logged, audited or put in an error.

use std::ops::RangeInclusive;
use std::sync::PoisonError;
use std::time::{Duration, Instant};

use base64::Engine as _;
use secrets::external::{ExtError, SsoBegin};
use serde_json::Value as Json;
use sha2::Digest as _;
use zeroize::Zeroizing;

use crate::config::LoginKind;
use crate::password::{self, LoginStep, TokenOutcome, TwoFactorKind};
use crate::{BwBackend, Urls, server_message};

/// The loopback ports Bitwarden's `cli` client may redirect to (`http://localhost:<port>`).
pub const PORTS: RangeInclusive<u16> = 8065..=8070;
/// How long a begun SSO login waits for its code.
pub const PENDING_TTL: Duration = Duration::from_secs(600);
/// The refusal for accounts that unlock without a master password.
pub const TDE_OR_KEY_CONNECTOR: &str =
    "this account unlocks with trusted devices / Key Connector, which theuth doesn't support yet";
/// The refusal for accounts that never set a master password.
pub const NO_MASTER_PASSWORD: &str =
    "this account has no master password yet; log in once in the Bitwarden web vault to set one";
/// The connect flows' hint (and the rewording of a password login an SSO policy refused).
pub const SSO_HINT: &str = "organisation requires SSO? use Login with SSO or API key";

/// A begun SSO login of one connection (at most one at a time).
pub(crate) struct Pending {
    state: String,
    verifier: Zeroizing<String>,
    redirect_uri: String,
    created: Instant,
}

/// 64 random characters (two v4 UUIDs, hex): a `state` or a PKCE verifier.
fn random_token() -> String {
    format!(
        "{}{}",
        uuid::Uuid::new_v4().simple(),
        uuid::Uuid::new_v4().simple()
    )
}

/// PKCE S256: base64url(SHA-256(verifier)), unpadded.
pub fn challenge(verifier: &str) -> String {
    base64::engine::general_purpose::URL_SAFE_NO_PAD
        .encode(sha2::Sha256::digest(verifier.as_bytes()))
}

/// `http://localhost:<port>`: the only redirect URIs the `cli` client has.
pub fn redirect_uri(port: u16) -> String {
    format!("http://localhost:{port}")
}

/// The port the browser comes back to: `None` = the first of [`PORTS`].
pub fn check_port(port: Option<u16>) -> Result<u16, ExtError> {
    let port = port.unwrap_or(*PORTS.start());
    if PORTS.contains(&port) {
        Ok(port)
    } else {
        Err(ExtError::Unsupported(format!(
            "redirect port {port}: Bitwarden accepts {}..={} only",
            PORTS.start(),
            PORTS.end()
        )))
    }
}

/// The identity server's authorize URL, as Bitwarden's web vault builds it after prevalidate.
pub fn authorize_url(
    identity: &str,
    redirect_uri: &str,
    state: &str,
    challenge: &str,
    org: &str,
    sso_token: &str,
) -> String {
    let mut u = format!("{}/connect/authorize?", identity.trim_end_matches('/'));
    u.push_str(
        &url::form_urlencoded::Serializer::new(String::new())
            .append_pair("client_id", password::CLIENT_ID)
            .append_pair("redirect_uri", redirect_uri)
            .append_pair("response_type", "code")
            .append_pair("scope", "api offline_access")
            .append_pair("state", state)
            .append_pair("code_challenge", challenge)
            .append_pair("code_challenge_method", "S256")
            .append_pair("response_mode", "query")
            .append_pair("domain_hint", org)
            .append_pair("ssoToken", sso_token)
            .finish(),
    );
    u
}

/// Constant-time string equality (length is not secret).
pub fn ct_eq(a: &str, b: &str) -> bool {
    a.len() == b.len()
        && a.bytes()
            .zip(b.bytes())
            .fold(0u8, |acc, (x, y)| acc | (x ^ y))
            == 0
}

/// The code in `input`: a bare code, or the address the browser landed on
/// (`http://localhost:8065/?code=…&state=…`; a bare `?code=…` query too). An address must carry
/// `state` (Bitwarden's clients append `_identifier=<org>`; only the part before it counts) and it
/// must be `expected`'s; an `error` in it is the IdP's refusal.
pub fn code_from(input: &str, expected: &str) -> Result<Zeroizing<String>, ExtError> {
    let input = input.trim();
    if input.is_empty() {
        return Err(ExtError::Denied(
            "paste the code or the address the browser landed on".into(),
        ));
    }
    let is_address = input.contains("://") || input.contains("code=") || input.contains("error=");
    if !is_address {
        if input.chars().any(char::is_whitespace) {
            return Err(ExtError::Denied("that is not an SSO code".into()));
        }
        return Ok(Zeroizing::new(input.to_owned()));
    }
    let query = input.split_once('?').map_or(input, |(_, q)| q);
    let query = query.split_once('#').map_or(query, |(q, _)| q);
    let (mut code, mut state, mut error, mut desc) = (None, None, None, None);
    for (k, v) in url::form_urlencoded::parse(query.as_bytes()) {
        match &*k {
            "code" => code = Some(Zeroizing::new(v.into_owned())),
            "state" => state = Some(v.into_owned()),
            "error" => error = Some(v.into_owned()),
            "error_description" => desc = Some(v.into_owned()),
            _ => {}
        }
    }
    if let Some(e) = error {
        return Err(ExtError::Denied(format!(
            "the SSO provider refused: {}",
            desc.filter(|d| !d.is_empty()).unwrap_or(e)
        )));
    }
    let state = state.ok_or_else(|| ExtError::Denied("that address has no state".into()))?;
    let state = state
        .split_once("_identifier=")
        .map_or(state.as_str(), |(s, _)| s);
    if !ct_eq(state, expected) {
        return Err(ExtError::Denied(
            "the SSO answer belongs to another login attempt".into(),
        ));
    }
    code.filter(|c| !c.is_empty())
        .ok_or_else(|| ExtError::Denied("that address has no code".into()))
}

/// A field of a Bitwarden JSON object, PascalCase or camelCase.
fn field<'a>(v: &'a Json, pascal: &str) -> Option<&'a Json> {
    v.get(pascal)
        .or_else(|| {
            let mut c = pascal.chars();
            let camel: String = c
                .next()
                .map(|f| f.to_ascii_lowercase().to_string() + c.as_str())
                .unwrap_or_default();
            v.get(camel)
        })
        .filter(|x| !x.is_null())
}

/// Whether the token answer's account can be unlocked with a master password (§9.6). Servers that
/// send no `UserDecryptionOptions` predate keyless accounts: allowed.
pub fn check_decryption(token_answer: &Json) -> Result<(), ExtError> {
    let Some(opts) = field(token_answer, "UserDecryptionOptions") else {
        return Ok(());
    };
    let has_master = field(opts, "HasMasterPassword").and_then(Json::as_bool) == Some(true)
        || field(opts, "MasterPasswordUnlock").is_some();
    if has_master {
        return Ok(());
    }
    if field(opts, "TrustedDeviceOption").is_some() || field(opts, "KeyConnectorOption").is_some() {
        return Err(ExtError::Unsupported(TDE_OR_KEY_CONNECTOR.into()));
    }
    Err(ExtError::Unsupported(NO_MASTER_PASSWORD.into()))
}

/// A password login an organisation's "require SSO" policy refused, reworded to [`SSO_HINT`].
pub fn reword_sso_required(why: String) -> String {
    let l = why.to_lowercase();
    if l.contains("sso_required") || (l.contains("sso") && l.contains("required")) {
        SSO_HINT.to_owned()
    } else {
        why
    }
}

/// A provider name from a UI (`authenticator` | `email` | `yubikey`).
pub fn factor(name: &str) -> Result<TwoFactorKind, ExtError> {
    match name {
        "authenticator" => Ok(TwoFactorKind::Authenticator),
        "email" => Ok(TwoFactorKind::Email),
        "yubikey" => Ok(TwoFactorKind::YubiKey),
        p => Err(ExtError::Unsupported(format!(
            "second factor {p:?}: authenticator, email or yubikey"
        ))),
    }
}

/// [`LoginStep`] in the `secrets` wire type.
pub fn wire_step(step: LoginStep) -> secrets::external::LoginStep {
    use secrets::external::LoginStep as Step;
    match step {
        LoginStep::Done => Step::Done,
        LoginStep::NeedNewDeviceOtp => Step::NeedNewDeviceOtp,
        LoginStep::NeedTwoFactor { providers } => Step::NeedTwoFactor {
            providers: providers
                .into_iter()
                .map(|k| {
                    match k {
                        TwoFactorKind::Authenticator => "authenticator",
                        TwoFactorKind::Email => "email",
                        TwoFactorKind::YubiKey => "yubikey",
                    }
                    .to_owned()
                })
                .collect(),
        },
    }
}

impl BwBackend {
    fn pending(&self) -> std::sync::MutexGuard<'_, Option<Pending>> {
        self.sso.lock().unwrap_or_else(PoisonError::into_inner)
    }

    fn sso_urls(&self) -> Result<Urls, ExtError> {
        if self.config().login != LoginKind::Sso {
            return Err(ExtError::Unsupported(format!(
                "bw/{} does not log in with SSO",
                self.conn
            )));
        }
        self.urls.clone().map_err(ExtError::Unsupported)
    }

    /// Begin an SSO login for organisation `org` with the browser coming back to
    /// `http://localhost:<port>` (`None` = 8065). Replaces a login begun before.
    pub async fn sso_begin(&self, org: &str, port: Option<u16>) -> Result<SsoBegin, ExtError> {
        let urls = self.sso_urls()?;
        let port = check_port(port)?;
        let org = org.trim();
        if org.is_empty() {
            return Err(ExtError::Denied(
                "the organisation's SSO identifier is empty".into(),
            ));
        }
        let client = Self::new_client(&urls);
        let url = format!(
            "{}/sso/prevalidate?{}",
            urls.identity,
            url::form_urlencoded::Serializer::new(String::new())
                .append_pair("domainHint", org)
                .finish()
        );
        let (status, text) = self.request(&client, url, None).await?;
        let token = serde_json::from_str::<Json>(&text)
            .ok()
            .and_then(|v| field(&v, "Token").and_then(Json::as_str).map(str::to_owned))
            .filter(|_| (200..300).contains(&status))
            .ok_or_else(|| {
                ExtError::Denied(format!("{} (SSO identifier {org})", server_message(&text)))
            })?;
        let state = random_token();
        let verifier = Zeroizing::new(random_token());
        let redirect = redirect_uri(port);
        let authorize = authorize_url(
            &urls.identity,
            &redirect,
            &state,
            &challenge(&verifier),
            org,
            &token,
        );
        *self.pending() = Some(Pending {
            state: state.clone(),
            verifier,
            redirect_uri: redirect,
            created: Instant::now(),
        });
        Ok(SsoBegin {
            authorize_url: authorize,
            state,
            redirect_port: port,
        })
    }

    /// Finish the SSO login `state`: exchange the code in `input` (bare, or the address the
    /// browser landed on). `NeedTwoFactor` keeps the login open: call again with the same input and
    /// the factor. `Done` = logged in and `Locked`.
    pub async fn sso_complete(
        &self,
        state: &str,
        input: &str,
        two_factor: Option<(TwoFactorKind, Zeroizing<String>)>,
    ) -> Result<LoginStep, ExtError> {
        let urls = self.sso_urls()?;
        let (verifier, redirect) = {
            let mut p = self.pending();
            match p.as_ref() {
                None => {
                    return Err(ExtError::Denied(format!(
                        "no SSO login is waiting for bw/{}; start again",
                        self.conn
                    )));
                }
                Some(x) if x.created.elapsed() > PENDING_TTL => {
                    *p = None;
                    return Err(ExtError::Denied(
                        "the SSO login timed out; start again".into(),
                    ));
                }
                Some(x) if !ct_eq(&x.state, state) => {
                    return Err(ExtError::Denied(
                        "the SSO answer belongs to another login attempt".into(),
                    ));
                }
                Some(x) => (x.verifier.clone(), x.redirect_uri.clone()),
            }
        };
        let code = code_from(input, state)?;
        let client = Self::new_client(&urls);
        let device = self.device_id().await;
        let retrying = two_factor.is_some();
        let body = password::sso_token_form(
            &code,
            &verifier,
            &redirect,
            &device,
            two_factor.as_ref().map(|(k, c)| (*k, c.as_str())),
        );
        let (status, text) = self
            .post(
                &client,
                format!("{}/connect/token", urls.identity),
                "application/x-www-form-urlencoded; charset=utf-8",
                body,
            )
            .await?;
        let done = |p: &mut Option<Pending>| *p = None;
        match password::parse_token_response(status, &text) {
            TokenOutcome::Tokens(t) => {
                done(&mut self.pending());
                check_decryption(&serde_json::from_str(&text).unwrap_or(Json::Null))?;
                let Some(refresh) = t.refresh_token else {
                    return Err(ExtError::Sdk(
                        "the SSO login returned no refresh token".into(),
                    ));
                };
                // Any session of an earlier login goes; the new login is `Locked` until unlock.
                self.lock_now().await;
                self.remember_refresh(Some(refresh)).await;
                self.refresh_status();
                Ok(LoginStep::Done)
            }
            TokenOutcome::TwoFactor(codes) => match password::supported_factors(&codes) {
                Ok(providers) => Ok(LoginStep::NeedTwoFactor { providers }),
                Err(e) => {
                    done(&mut self.pending());
                    Err(ExtError::Unsupported(e))
                }
            },
            TokenOutcome::NewDevice => {
                done(&mut self.pending());
                Err(ExtError::Sdk(
                    "Bitwarden asked for new-device verification after SSO, which it does not do; \
                     try again"
                        .into(),
                ))
            }
            TokenOutcome::Refused(why) => {
                // A wrong second factor may be typed again; anything else ends the attempt.
                if !retrying {
                    done(&mut self.pending());
                }
                Err(ExtError::Denied(why))
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn pkce_is_s256_base64url_unpadded() {
        // RFC 7636 appendix B.
        assert_eq!(
            challenge("dBjftJeZ4CVP-mB92K27uhbUJU1p1r_wW1gFWFOEjXk"),
            "E9Melhoa2OwvFrEMTJguCHaoeK1t8URWbuGJSstw-cM"
        );
        let t = random_token();
        assert_eq!(t.len(), 64);
        assert!(t.bytes().all(|b| b.is_ascii_alphanumeric()));
        assert_ne!(t, random_token());
    }

    #[test]
    fn ports_are_the_cli_clients() {
        assert_eq!(check_port(None).unwrap(), 8065);
        assert_eq!(check_port(Some(8070)).unwrap(), 8070);
        for bad in [8064, 8071, 80, 0] {
            assert!(
                matches!(check_port(Some(bad)), Err(ExtError::Unsupported(_))),
                "{bad}"
            );
        }
        assert_eq!(redirect_uri(8066), "http://localhost:8066");
    }

    #[test]
    fn authorize_url_carries_every_parameter() {
        let u = authorize_url(
            "https://identity.bitwarden.com/",
            "http://localhost:8065",
            "st",
            "ch",
            "acme corp",
            "tok",
        );
        let (base, q) = u.split_once('?').unwrap();
        assert_eq!(base, "https://identity.bitwarden.com/connect/authorize");
        let q: std::collections::HashMap<_, _> = url::form_urlencoded::parse(q.as_bytes())
            .into_owned()
            .collect();
        for (k, v) in [
            ("client_id", "cli"),
            ("redirect_uri", "http://localhost:8065"),
            ("response_type", "code"),
            ("scope", "api offline_access"),
            ("state", "st"),
            ("code_challenge", "ch"),
            ("code_challenge_method", "S256"),
            ("response_mode", "query"),
            ("domain_hint", "acme corp"),
            ("ssoToken", "tok"),
        ] {
            assert_eq!(q.get(k).map(String::as_str), Some(v), "{k}");
        }
    }

    #[test]
    fn the_code_comes_bare_or_in_the_landing_address() {
        let st = "a".repeat(64);
        assert_eq!(code_from(" abc.DEF-1 ", &st).unwrap().as_str(), "abc.DEF-1");
        let addr = format!("http://localhost:8065/?code=c%2B1&state={st}&scope=api&iss=x");
        assert_eq!(code_from(&addr, &st).unwrap().as_str(), "c+1");
        // Bitwarden's own clients append the identifier to the state.
        let addr = format!("localhost:8065/?code=c2&state={st}_identifier=acme");
        assert_eq!(code_from(&addr, &st).unwrap().as_str(), "c2");
        let other = format!("http://localhost:8065/?code=c&state={}", "b".repeat(64));
        assert!(
            matches!(code_from(&other, &st), Err(ExtError::Denied(m)) if m.contains("another login attempt"))
        );
        assert!(
            code_from("http://localhost:8065/?code=c", &st).is_err(),
            "no state"
        );
        assert!(matches!(
            code_from(&format!("http://localhost:8065/?error=access_denied&error_description=nope&state={st}"), &st),
            Err(ExtError::Denied(m)) if m.ends_with("nope")
        ));
        assert!(code_from("", &st).is_err());
        assert!(code_from("two words", &st).is_err());
    }

    #[test]
    fn decryption_options_decide_who_may_log_in() {
        let ok = |v: Json| check_decryption(&v).is_ok();
        let refused = |v: Json| match check_decryption(&v) {
            Err(ExtError::Unsupported(m)) => m,
            r => panic!("{r:?}"),
        };
        assert!(ok(serde_json::json!({ "access_token": "a" })), "old server");
        assert!(ok(
            serde_json::json!({ "UserDecryptionOptions": { "HasMasterPassword": true } })
        ));
        assert!(ok(serde_json::json!({ "userDecryptionOptions": {
            "hasMasterPassword": false, "masterPasswordUnlock": { "salt": "x" } } })));
        // Vaultwarden 1.37.5's answer for an SSO user without a master password (spike).
        assert_eq!(
            refused(serde_json::json!({ "UserDecryptionOptions": {
                "HasMasterPassword": false, "MasterPasswordUnlock": null, "Object": "userDecryptionOptions" } })),
            NO_MASTER_PASSWORD
        );
        assert_eq!(
            refused(serde_json::json!({ "UserDecryptionOptions": {
                "HasMasterPassword": false, "TrustedDeviceOption": { "HasAdminApproval": true } } })),
            TDE_OR_KEY_CONNECTOR
        );
        assert_eq!(
            refused(serde_json::json!({ "userDecryptionOptions": {
                "hasMasterPassword": false, "keyConnectorOption": { "keyConnectorUrl": "https://k" } } })),
            TDE_OR_KEY_CONNECTOR
        );
        assert_eq!(
            refused(serde_json::json!({ "UserDecryptionOptions": {
                "HasMasterPassword": false, "WebAuthnPrfOption": { "CredentialId": "c" } } })),
            NO_MASTER_PASSWORD
        );
    }

    #[test]
    fn sso_required_is_reworded() {
        assert_eq!(reword_sso_required("sso_required".into()), SSO_HINT);
        assert_eq!(
            reword_sso_required("SSO authentication is required.".into()),
            SSO_HINT
        );
        assert_eq!(
            reword_sso_required("Username or password is incorrect.".into()),
            "Username or password is incorrect."
        );
    }

    fn sso_backend() -> std::sync::Arc<BwBackend> {
        BwBackend::new(
            "work",
            crate::BwConfig {
                login: LoginKind::Sso,
                email: Some("me@example.org".into()),
                ..Default::default()
            },
        )
    }

    fn pend(b: &BwBackend, state: &str, age: Duration) {
        *b.pending() = Some(Pending {
            state: state.into(),
            verifier: Zeroizing::new("v".into()),
            redirect_uri: redirect_uri(8065),
            created: Instant::now().checked_sub(age).unwrap(),
        });
    }

    /// Refusals that happen before any network call: wrong login kind, port, no pending login,
    /// another state, an expired login.
    #[tokio::test]
    async fn complete_checks_state_and_ttl_before_the_network() {
        let api = BwBackend::new("p", crate::BwConfig::default());
        assert!(matches!(
            api.sso_begin("acme", None).await,
            Err(ExtError::Unsupported(_))
        ));
        let b = sso_backend();
        assert!(matches!(
            b.sso_begin("acme", Some(9000)).await,
            Err(ExtError::Unsupported(_))
        ));
        assert!(matches!(
            b.sso_begin(" ", None).await,
            Err(ExtError::Denied(_))
        ));
        let st = "s".repeat(64);
        assert!(
            matches!(b.sso_complete(&st, "code", None).await, Err(ExtError::Denied(m)) if m.contains("no SSO login"))
        );
        pend(&b, &st, Duration::ZERO);
        assert!(
            matches!(b.sso_complete(&"t".repeat(64), "code", None).await, Err(ExtError::Denied(m)) if m.contains("another login attempt"))
        );
        assert!(
            b.pending().is_some(),
            "a stray state does not end the real attempt"
        );
        pend(&b, &st, PENDING_TTL + Duration::from_secs(1));
        assert!(
            matches!(b.sso_complete(&st, "code", None).await, Err(ExtError::Denied(m)) if m.contains("timed out"))
        );
        assert!(b.pending().is_none(), "an expired login is dropped");
    }
}
