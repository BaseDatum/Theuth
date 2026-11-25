//! 221 §6: the external secret backends this binary links. At start, one
//! [`BackendFactory`] per backend goes to the secrets actor
//! ([`RegisterBackendFactory`]); the actor then builds every configured connection of that
//! backend (after seeding its always-ask entries), builds the ones added later, and unregisters
//! removed ones. AWS is always linked; Bitwarden with the `bitwarden` feature (D3, D22).

use std::sync::Arc;

use secrets::connections::ConnConfig;
use secrets::external::ExternalBackend;
use secrets::{BackendFactory, ConnStore, RegisterBackendFactory, SECRETS_ACTOR, SecretsActor};

/// Registers the factories once the secrets actor is up. Placed after `SecretsPlugin`.
pub struct ExternalBackendsPlugin;

#[async_trait::async_trait]
impl runtime::Plugin for ExternalBackendsPlugin {
    fn name(&self) -> &'static str {
        "external-backends"
    }

    async fn start(self: Box<Self>, rt: &runtime::Runtime) -> anyhow::Result<()> {
        let rt = rt.clone();
        tokio::spawn(async move {
            // `None` never comes back: the vault did not open, and nothing resolves externals.
            let Some(actor) = rt.lookup_when::<SecretsActor>(SECRETS_ACTOR).await else {
                return;
            };
            for f in factories() {
                let backend = f.backend();
                if let Err(e) = actor.ask(RegisterBackendFactory(f)).await {
                    tracing::warn!("secrets: {backend} backend not registered: {e}");
                }
            }
        });
        Ok(())
    }
}

fn factories() -> Vec<Arc<dyn BackendFactory>> {
    vec![
        Arc::new(AwsFactory),
        #[cfg(feature = "bitwarden")]
        Arc::new(bitwarden::BwFactory),
    ]
}

/// §6.1: AWS connections. No credentials in theuth: the user's `~/.aws` chain.
struct AwsFactory;

impl BackendFactory for AwsFactory {
    fn backend(&self) -> &'static str {
        secrets_aws::BACKEND
    }

    fn build(
        &self,
        conn: &str,
        cfg: &ConnConfig,
        _store: ConnStore,
    ) -> Result<Arc<dyn ExternalBackend>, String> {
        let ConnConfig::Aws {
            profile,
            region,
            kms_key_id,
            prefix,
        } = cfg
        else {
            return Err(format!("{conn} is not an AWS connection"));
        };
        if region.trim().is_empty() {
            return Err(format!(
                "aws/{conn}: a region is required (Secrets Manager is regional)"
            ));
        }
        let mut c = secrets_aws::AwsConnConfig::new(conn, region.clone());
        c.profile = profile.clone();
        c.kms_key_id = kms_key_id.clone();
        c.prefix = prefix.clone();
        Ok(Arc::new(secrets_aws::AwsBackend::new(c)))
    }
}

#[cfg(feature = "bitwarden")]
mod bitwarden {
    use std::sync::Arc;

    use secrets::connections::{BwLogin, BwServer, ConnConfig};
    use secrets::external::ExternalBackend;
    use secrets::{BackendFactory, ConnStore};
    use secrets_bitwarden::{BwBackend, BwConfig, CredentialStore, LoginKind, Server};
    use zeroize::Zeroizing;

    /// §6.2: Bitwarden connections; credentials in the connection's sealed `meta` rows.
    pub(super) struct BwFactory;

    impl BackendFactory for BwFactory {
        fn backend(&self) -> &'static str {
            secrets_bitwarden::BACKEND
        }

        fn build(
            &self,
            conn: &str,
            cfg: &ConnConfig,
            store: ConnStore,
        ) -> Result<Arc<dyn ExternalBackend>, String> {
            let ConnConfig::Bw {
                server,
                base_url,
                api_url,
                identity_url,
                web_vault_url,
                email,
                login,
            } = cfg
            else {
                return Err(format!("{conn} is not a Bitwarden connection"));
            };
            let bw = BwConfig {
                server: match server {
                    BwServer::Us => Server::Us,
                    BwServer::Eu => Server::Eu,
                    BwServer::SelfHosted => Server::SelfHosted,
                },
                base_url: base_url.clone(),
                api_url: api_url.clone(),
                identity_url: identity_url.clone(),
                web_vault_url: web_vault_url.clone(),
                email: Some(email.clone()).filter(|e| !e.is_empty()),
                login: match login {
                    BwLogin::ApiKey => LoginKind::ApiKey,
                    BwLogin::Password => LoginKind::Password,
                },
                ..BwConfig::default()
            };
            let backend = BwBackend::with_store(conn, bw, Some(Arc::new(MetaStore(store))));
            // Stored credentials make it `Locked` (the unlock card), none leave it `LoggedOut`.
            let loading = backend.clone();
            tokio::spawn(async move { loading.load_credentials().await });
            Ok(backend)
        }
    }

    /// [`CredentialStore`] over the connection's `ext/bw/<conn>/` rows. `ConnStore` has no
    /// single-row delete, so a removal is an empty row, which `load` reads as absent.
    struct MetaStore(ConnStore);

    #[async_trait::async_trait]
    impl CredentialStore for MetaStore {
        async fn load(&self, key: &str) -> Option<Zeroizing<String>> {
            match self.0.get(key).await {
                Ok(Some(mut bytes)) if !bytes.is_empty() => {
                    String::from_utf8(std::mem::take(&mut *bytes))
                        .ok()
                        .map(Zeroizing::new)
                }
                Ok(_) => None,
                Err(e) => {
                    tracing::warn!("secrets: bw credential {key} unreadable: {e}");
                    None
                }
            }
        }

        async fn save(&self, key: &str, value: Option<&str>) {
            if let Err(e) = self.0.put(key, value.unwrap_or("").as_bytes()).await {
                tracing::warn!("secrets: bw credential {key} not saved: {e}");
            }
        }
    }
}
