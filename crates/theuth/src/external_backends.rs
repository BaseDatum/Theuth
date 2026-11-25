//! 221 §6: the external secret backends this binary links. At start, one
//! [`BackendFactory`] per backend goes to the secrets actor
//! ([`RegisterBackendFactory`]); the actor then builds every configured connection of that
//! backend (after seeding its always-ask entries), builds the ones added later, and unregisters
//! removed ones. AWS is always linked; Bitwarden with the `bitwarden` feature (D3, D22).
//! Also the engine's artifact lookup for `artifact:<id>` Send sources (§12.3), which `secrets`
//! cannot do itself (`artifact` depends on `agent`, which depends on `secrets`).

use std::sync::Arc;

use runtime::SessionId;
use runtime::kameo::actor::ActorRef;
use secrets::connections::{ConnConfig, SetArtifactSource};
use secrets::external::ExternalBackend;
use secrets::tools_send::{ArtifactRef, ArtifactSource};
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
            let _ = actor.ask(SetArtifactSource(Arc::new(Artifacts))).await;
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

/// `artifact:<id>`: the artifact `id` anywhere in the tree of `session` (from its root down).
struct Artifacts;

#[async_trait::async_trait]
impl ArtifactSource for Artifacts {
    async fn find(&self, session: SessionId, id: &str) -> Option<ArtifactRef> {
        let agents = ActorRef::<agent::Agents>::lookup(agent::AGENT_ACTOR).ok()??;
        let chain = agents.ask(agent::Ancestors { session }).await.ok()?;
        let root = chain.last().map_or(session, |a| a.id);
        let store =
            ActorRef::<artifact::ArtifactActor>::lookup(artifact::ARTIFACT_ACTOR).ok()??;
        let all = store
            .ask(artifact::List {
                session: Some(root),
                descendants: true,
            })
            .await
            .ok()?;
        all.into_iter().find(|a| a.id == id).map(|a| ArtifactRef {
            path: a.path.to_string_lossy().into_owned(),
            name: a.name,
            bytes: a.bytes,
        })
    }
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

    /// [`CredentialStore`] over the connection's `ext/bw/<conn>/` rows. An empty row reads as
    /// absent.
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
            let r = match value {
                Some(v) => self.0.put(key, v.as_bytes()).await,
                None => self.0.delete(key).await,
            };
            if let Err(e) = r {
                tracing::warn!("secrets: bw credential {key} not saved: {e}");
            }
        }
    }
}
