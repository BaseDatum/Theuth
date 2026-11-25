//! A Bitwarden connection's settings (221 §6.2, D20): the `secrets.connections.<conn>` keys this
//! backend reads. Parsing `config.yaml` and seeding always-ask entries are the `secrets` crate's.

use serde::{Deserialize, Serialize};

/// Which Bitwarden server.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Server {
    /// `vault.bitwarden.com`.
    #[default]
    Us,
    /// `vault.bitwarden.eu`.
    Eu,
    /// Official self-host or Vaultwarden at `base_url`.
    SelfHosted,
}

/// How the connection logs in.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum LoginKind {
    /// Personal API key; unlock = a fresh API-key login with the master password.
    #[default]
    ApiKey,
    /// Email + master password + 2FA (B5b).
    Password,
}

/// Where creates and updates may go.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum WriteScope {
    /// The personal vault only.
    #[default]
    Personal,
    /// Also organisation collections.
    PersonalAndOrg,
}

/// One Bitwarden connection's settings. Every field has the doc's default.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct BwConfig {
    /// `us` | `eu` | `self_hosted`.
    pub server: Server,
    /// `self_hosted`: `https://vault.example.org`.
    pub base_url: Option<String>,
    /// Override; default `<base>/api` (cloud: the preset).
    pub api_url: Option<String>,
    /// Override; default `<base>/identity` (cloud: the preset).
    pub identity_url: Option<String>,
    /// Override of the web vault used in Send links.
    pub web_vault_url: Option<String>,
    /// Shown on the unlock card.
    pub email: Option<String>,
    /// `api_key` | `password`.
    pub login: LoginKind,
    /// Lock after this long without a get/create/update/delete; 0 = only on quit.
    pub lock_after_idle_secs: u64,
    /// A read whose last sync is older than this syncs first.
    pub sync_max_age_secs: u64,
    /// `Permanent` deletes are refused unless on (D13).
    pub allow_permanent_delete: bool,
    /// `#password:-N` reads the password history (D20).
    pub expose_password_history: bool,
    /// `#totp-seed` reads the seed, not only the current code (D20).
    pub expose_totp_seed: bool,
    /// `personal` | `personal_and_org` (D20).
    pub write_scope: WriteScope,
    /// Every network operation's deadline (`secrets.external.timeout_secs`).
    pub timeout_secs: u64,
    /// A Send's card is answerable only by the owner in person (D17).
    pub send_owner_only: bool,
    /// Seal the user key in the credential store so a restart unlocks without the master
    /// password (D12).
    pub remember_unlock: bool,
}

impl Default for BwConfig {
    fn default() -> Self {
        Self {
            server: Server::Us,
            base_url: None,
            api_url: None,
            identity_url: None,
            web_vault_url: None,
            email: None,
            login: LoginKind::ApiKey,
            lock_after_idle_secs: 900,
            sync_max_age_secs: 300,
            allow_permanent_delete: false,
            expose_password_history: false,
            expose_totp_seed: false,
            write_scope: WriteScope::Personal,
            timeout_secs: 20,
            send_owner_only: true,
            remember_unlock: false,
        }
    }
}

/// The resolved server URLs.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Urls {
    /// `…/api`.
    pub api: String,
    /// `…/identity`.
    pub identity: String,
    /// The web vault (Send links).
    pub web_vault: String,
}

impl BwConfig {
    /// The server URLs, or why the configuration cannot be served (model- and form-facing).
    pub fn urls(&self) -> Result<Urls, String> {
        let (api, identity, web) = match self.server {
            Server::Us => (
                "https://api.bitwarden.com".to_owned(),
                "https://identity.bitwarden.com".to_owned(),
                "https://vault.bitwarden.com".to_owned(),
            ),
            Server::Eu => (
                "https://api.bitwarden.eu".to_owned(),
                "https://identity.bitwarden.eu".to_owned(),
                "https://vault.bitwarden.eu".to_owned(),
            ),
            Server::SelfHosted => {
                let base = self
                    .base_url
                    .as_deref()
                    .map(|b| b.trim_end_matches('/'))
                    .filter(|b| !b.is_empty())
                    .ok_or("a self-hosted connection needs base_url")?;
                (
                    format!("{base}/api"),
                    format!("{base}/identity"),
                    base.to_owned(),
                )
            }
        };
        let pick = |o: &Option<String>, d: String| {
            o.as_deref()
                .map(|u| u.trim_end_matches('/').to_owned())
                .unwrap_or(d)
        };
        let urls = Urls {
            api: pick(&self.api_url, api),
            identity: pick(&self.identity_url, identity),
            web_vault: pick(&self.web_vault_url, web),
        };
        for u in [&urls.api, &urls.identity] {
            check_scheme(u)?;
        }
        Ok(urls)
    }
}

/// The SDK's HTTP client is `https_only` outside debug builds (221 §5.1).
fn check_scheme(url: &str) -> Result<(), String> {
    if url.starts_with("https://") || (cfg!(debug_assertions) && url.starts_with("http://")) {
        Ok(())
    } else if url.starts_with("http://") {
        Err(format!(
            "{url}: plain http is not supported; the Bitwarden SDK only speaks https in release builds"
        ))
    } else {
        Err(format!("{url}: not an https URL"))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn presets_and_self_hosted_derivation() {
        let us = BwConfig::default().urls().unwrap();
        assert_eq!(us.api, "https://api.bitwarden.com");
        assert_eq!(us.identity, "https://identity.bitwarden.com");
        assert_eq!(us.web_vault, "https://vault.bitwarden.com");

        let eu = BwConfig {
            server: Server::Eu,
            ..Default::default()
        }
        .urls()
        .unwrap();
        assert_eq!(eu.identity, "https://identity.bitwarden.eu");
        assert_eq!(eu.web_vault, "https://vault.bitwarden.eu");

        let sh = BwConfig {
            server: Server::SelfHosted,
            base_url: Some("https://vault.example.org/".into()),
            identity_url: Some("https://id.example.org".into()),
            ..Default::default()
        }
        .urls()
        .unwrap();
        assert_eq!(sh.api, "https://vault.example.org/api");
        assert_eq!(sh.identity, "https://id.example.org");
        assert_eq!(sh.web_vault, "https://vault.example.org");

        let missing = BwConfig {
            server: Server::SelfHosted,
            ..Default::default()
        };
        assert!(missing.urls().is_err());
    }

    #[test]
    fn config_yaml_shape_and_defaults() {
        let c: BwConfig = serde_json::from_value(serde_json::json!({
            "server": "self_hosted", "base_url": "https://v.example", "write_scope": "personal_and_org"
        }))
        .unwrap();
        assert_eq!(c.server, Server::SelfHosted);
        assert_eq!(c.write_scope, WriteScope::PersonalAndOrg);
        assert_eq!(c.lock_after_idle_secs, 900);
        assert_eq!(c.sync_max_age_secs, 300);
        assert!(!c.allow_permanent_delete);
    }
}
