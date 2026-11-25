//! Password login (221 §5.2, G7, milestone B5b): theuth's own `POST {identity}/connect/token`,
//! because the SDK's hard-codes one device id for every user and cannot answer new-device
//! verification. Request bodies and response parsing live here, pure; the HTTP is in `lib.rs`.

use std::num::NonZeroU32;

use bitwarden_crypto::Kdf;
use serde::{Deserialize, Serialize};
use serde_json::Value as Json;
use zeroize::Zeroizing;

/// The OAuth client id theuth's password login and refresh use (the official CLI's).
pub const CLIENT_ID: &str = "cli";

/// A second factor theuth can answer (Duo and WebAuthn need a browser).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum TwoFactorKind {
    /// Authenticator app (TOTP), provider 0.
    Authenticator,
    /// Emailed code, provider 1.
    Email,
    /// YubiKey OTP (a typed token), provider 3.
    YubiKey,
}

impl TwoFactorKind {
    /// Bitwarden's `TwoFactorProviderType` number.
    pub fn code(self) -> u8 {
        match self {
            Self::Authenticator => 0,
            Self::Email => 1,
            Self::YubiKey => 3,
        }
    }

    fn from_code(code: u8) -> Option<Self> {
        match code {
            0 => Some(Self::Authenticator),
            1 => Some(Self::Email),
            3 => Some(Self::YubiKey),
            _ => None,
        }
    }
}

/// The login card's answers.
pub struct PasswordLogin {
    /// The account email.
    pub email: String,
    /// The master password.
    pub password: Zeroizing<String>,
    /// The second factor the user answered, when the server asked for one.
    pub two_factor: Option<(TwoFactorKind, Zeroizing<String>)>,
    /// The new-device verification code from the user's email.
    pub new_device_otp: Option<Zeroizing<String>>,
}

/// What [`crate::BwBackend::login_password`] needs next.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case", tag = "step")]
pub enum LoginStep {
    /// Logged in and unlocked.
    Done,
    /// Ask for one of these second factors and log in again with it.
    NeedTwoFactor {
        /// The supported providers the account has, in the server's order.
        providers: Vec<TwoFactorKind>,
    },
    /// Ask for the code Bitwarden emailed for this new device and log in again with it.
    NeedNewDeviceOtp,
}

/// The `deviceType` theuth reports: the CLI type of this OS.
pub fn device_type() -> u8 {
    if cfg!(target_os = "macos") {
        24
    } else if cfg!(target_os = "windows") {
        23
    } else {
        25
    }
}

/// `application/x-www-form-urlencoded` body of the password grant.
pub fn token_form(
    email: &str,
    password_hash: &str,
    device_id: &str,
    two_factor: Option<(TwoFactorKind, &str)>,
    new_device_otp: Option<&str>,
) -> String {
    let device_type = device_type().to_string();
    let mut f = url::form_urlencoded::Serializer::new(String::new());
    f.append_pair("grant_type", "password")
        .append_pair("username", email)
        .append_pair("password", password_hash)
        .append_pair("scope", "api offline_access")
        .append_pair("client_id", CLIENT_ID)
        .append_pair("deviceType", &device_type)
        .append_pair("deviceIdentifier", device_id)
        .append_pair("deviceName", "theuth");
    if let Some((kind, token)) = two_factor {
        f.append_pair("twoFactorToken", token)
            .append_pair("twoFactorProvider", &kind.code().to_string())
            .append_pair("twoFactorRemember", "0");
    }
    if let Some(otp) = new_device_otp {
        f.append_pair("newDeviceOtp", otp);
    }
    f.finish()
}

/// `POST {api}/two-factor/send-email-login`'s JSON body.
pub fn email_code_body(email: &str, password_hash: &str, device_id: &str) -> Json {
    serde_json::json!({
        "email": email,
        "masterPasswordHash": password_hash,
        "deviceIdentifier": device_id,
    })
}

/// A KDF from a prelogin answer (`/accounts/prelogin/password` or the legacy `/accounts/prelogin`;
/// camelCase or PascalCase).
pub fn parse_prelogin(body: &str) -> Result<Kdf, String> {
    let v: Json = serde_json::from_str(body).map_err(|e| format!("prelogin: {e}"))?;
    let num = |names: &[&str]| {
        names
            .iter()
            .find_map(|n| v.get(*n).and_then(Json::as_u64))
            .and_then(|n| u32::try_from(n).ok())
            .and_then(NonZeroU32::new)
    };
    let kind = num(&["kdf", "Kdf"]).map_or(0, NonZeroU32::get);
    let iterations = num(&["kdfIterations", "KdfIterations"]).ok_or("prelogin: no iterations")?;
    match kind {
        0 => Ok(Kdf::PBKDF2 { iterations }),
        1 => Ok(Kdf::Argon2id {
            iterations,
            memory: num(&["kdfMemory", "KdfMemory"]).ok_or("prelogin: no Argon2 memory")?,
            parallelism: num(&["kdfParallelism", "KdfParallelism"])
                .ok_or("prelogin: no Argon2 parallelism")?,
        }),
        k => Err(format!("prelogin: unknown KDF type {k}")),
    }
}

/// The tokens of a successful grant.
pub struct Tokens {
    /// Bearer token.
    pub access_token: String,
    /// The long-lived token theuth keeps sealed in `meta`.
    pub refresh_token: Option<Zeroizing<String>>,
    /// Seconds.
    pub expires_in: u64,
}

/// How the identity server answered the password grant.
pub enum TokenOutcome {
    /// Logged in.
    Tokens(Tokens),
    /// A second factor is needed: the provider numbers the account has.
    TwoFactor(Vec<u8>),
    /// New-device verification: a code went to the account's email.
    NewDevice,
    /// Refused (wrong password, unknown account, bad 2FA code, …): the server's text.
    Refused(String),
}

/// Parse `/connect/token`'s answer.
pub fn parse_token_response(status: u16, body: &str) -> TokenOutcome {
    let v: Json = serde_json::from_str(body).unwrap_or(Json::Null);
    if (200..300).contains(&status)
        && let Some(access) = v.get("access_token").and_then(Json::as_str)
    {
        return TokenOutcome::Tokens(Tokens {
            access_token: access.to_owned(),
            refresh_token: v
                .get("refresh_token")
                .and_then(Json::as_str)
                .map(|s| Zeroizing::new(s.to_owned())),
            expires_in: v.get("expires_in").and_then(Json::as_u64).unwrap_or(3600),
        });
    }
    if let Some(p) = v
        .get("TwoFactorProviders2")
        .or_else(|| v.get("twoFactorProviders2"))
        .and_then(Json::as_object)
    {
        let mut codes: Vec<u8> = p.keys().filter_map(|k| k.parse().ok()).collect();
        codes.sort_unstable();
        return TokenOutcome::TwoFactor(codes);
    }
    let text = |k: &str| {
        v.get(k)
            .and_then(Json::as_str)
            .map(str::to_owned)
            .unwrap_or_default()
    };
    let desc = text("error_description");
    let message = v
        .get("ErrorModel")
        .or_else(|| v.get("errorModel"))
        .and_then(|m| m.get("Message").or_else(|| m.get("message")))
        .and_then(Json::as_str)
        .unwrap_or_default()
        .to_owned();
    if desc
        .to_lowercase()
        .contains("new device verification required")
        || message
            .to_lowercase()
            .contains("new device verification required")
    {
        return TokenOutcome::NewDevice;
    }
    let why = [message, desc, text("error")]
        .into_iter()
        .find(|s| !s.is_empty())
        .unwrap_or_else(|| format!("HTTP {status}"));
    TokenOutcome::Refused(why)
}

/// The supported subset of `codes`, or why none can be answered.
pub fn supported_factors(codes: &[u8]) -> Result<Vec<TwoFactorKind>, String> {
    let kinds: Vec<_> = codes
        .iter()
        .filter_map(|c| TwoFactorKind::from_code(*c))
        .collect();
    if kinds.is_empty() {
        return Err(
            "this account's second factor (Duo or WebAuthn) needs a browser; use API-key login \
             (Account settings › Security › Keys)"
                .into(),
        );
    }
    Ok(kinds)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn pairs(form: &str) -> Vec<(String, String)> {
        url::form_urlencoded::parse(form.as_bytes())
            .into_owned()
            .collect()
    }

    fn get<'a>(p: &'a [(String, String)], k: &str) -> Option<&'a str> {
        p.iter().find(|(n, _)| n == k).map(|(_, v)| v.as_str())
    }

    #[test]
    fn password_grant_body() {
        let p = pairs(&token_form(
            "me+x@example.org",
            "h/a=sh",
            "dev-1",
            None,
            None,
        ));
        assert_eq!(get(&p, "grant_type"), Some("password"));
        assert_eq!(get(&p, "username"), Some("me+x@example.org"));
        assert_eq!(get(&p, "password"), Some("h/a=sh"));
        assert_eq!(get(&p, "scope"), Some("api offline_access"));
        assert_eq!(get(&p, "client_id"), Some(CLIENT_ID));
        assert_eq!(get(&p, "deviceIdentifier"), Some("dev-1"));
        assert_eq!(
            get(&p, "deviceType"),
            Some(device_type().to_string().as_str())
        );
        assert_eq!(get(&p, "twoFactorToken"), None);
        assert_eq!(get(&p, "newDeviceOtp"), None);

        let p = pairs(&token_form(
            "a@b",
            "h",
            "d",
            Some((TwoFactorKind::YubiKey, "cccc")),
            Some("123456"),
        ));
        assert_eq!(get(&p, "twoFactorToken"), Some("cccc"));
        assert_eq!(get(&p, "twoFactorProvider"), Some("3"));
        assert_eq!(get(&p, "twoFactorRemember"), Some("0"));
        assert_eq!(get(&p, "newDeviceOtp"), Some("123456"));
    }

    #[test]
    fn email_code_request() {
        let b = email_code_body("a@b", "h", "d");
        assert_eq!(b["email"], "a@b");
        assert_eq!(b["masterPasswordHash"], "h");
        assert_eq!(b["deviceIdentifier"], "d");
    }

    #[test]
    fn prelogin_both_shapes() {
        let pb = parse_prelogin(r#"{"kdf":0,"kdfIterations":600000,"kdfMemory":null}"#).unwrap();
        assert_eq!(
            pb,
            Kdf::PBKDF2 {
                iterations: NonZeroU32::new(600_000).unwrap()
            }
        );
        let a = parse_prelogin(r#"{"Kdf":1,"KdfIterations":3,"KdfMemory":64,"KdfParallelism":4}"#)
            .unwrap();
        assert!(matches!(a, Kdf::Argon2id { .. }));
        assert!(parse_prelogin(r#"{"kdf":1,"kdfIterations":3}"#).is_err());
    }

    #[test]
    fn token_answers() {
        let ok = parse_token_response(
            200,
            r#"{"access_token":"a","refresh_token":"r","expires_in":7200,"token_type":"Bearer"}"#,
        );
        let TokenOutcome::Tokens(t) = ok else {
            panic!("tokens")
        };
        assert_eq!(t.access_token, "a");
        assert_eq!(t.refresh_token.as_deref().map(String::as_str), Some("r"));
        assert_eq!(t.expires_in, 7200);

        let tf = parse_token_response(
            400,
            r#"{"error":"invalid_grant","error_description":"Two factor required.",
                "TwoFactorProviders":["1","0"],"TwoFactorProviders2":{"1":{"Email":"m***@x"},"0":null}}"#,
        );
        assert!(matches!(&tf, TokenOutcome::TwoFactor(c) if c == &[0, 1]));

        let nd = parse_token_response(
            400,
            r#"{"error":"invalid_grant","error_description":"New device verification required"}"#,
        );
        assert!(matches!(nd, TokenOutcome::NewDevice));

        let bad = parse_token_response(
            400,
            r#"{"error":"invalid_grant","error_description":"invalid_username_or_password",
                "ErrorModel":{"Message":"Username or password is incorrect. Try again"}}"#,
        );
        assert!(matches!(&bad, TokenOutcome::Refused(m) if m.starts_with("Username or password")));
    }

    #[test]
    fn browser_only_factors_are_refused() {
        assert_eq!(
            supported_factors(&[2, 0, 7, 3]).unwrap(),
            [TwoFactorKind::Authenticator, TwoFactorKind::YubiKey]
        );
        assert!(supported_factors(&[2, 7]).is_err());
    }
}
