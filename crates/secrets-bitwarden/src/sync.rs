//! `TheuthSyncHandler` (221 §5.4, gaps G1–G4): what the SDK's own handlers do not keep after a
//! `/sync`. Registered once on the connection's single `SyncClient` (G6).

use std::collections::HashMap;
use std::sync::{Mutex, PoisonError};

use bitwarden_api_api::models::SyncResponseModel;
use bitwarden_collections::collection::Collection;
use bitwarden_core::key_management::crypto::InitOrgCryptoRequest;
use bitwarden_core::{Client, OrganizationId};
use bitwarden_crypto::UnsignedSharedKey;
use bitwarden_sync::{SyncHandler, SyncHandlerError};
use bitwarden_vault::Cipher;

/// Ciphers into the SDK's repository (G1), organisation keys into its key store (G2), and the
/// collections (G3) and organisation names (G4) the SDK has no store for.
pub(crate) struct TheuthSyncHandler {
    client: Client,
    collections: Mutex<Vec<Collection>>,
    orgs: Mutex<HashMap<OrganizationId, String>>,
}

impl TheuthSyncHandler {
    pub(crate) fn new(client: Client) -> Self {
        Self {
            client,
            collections: Mutex::default(),
            orgs: Mutex::default(),
        }
    }

    /// The encrypted collections of the last sync.
    pub(crate) fn collections(&self) -> Vec<Collection> {
        self.collections
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .clone()
    }

    /// Organisation id → name, from the last sync's profile.
    pub(crate) fn orgs(&self) -> HashMap<OrganizationId, String> {
        self.orgs
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .clone()
    }
}

#[async_trait::async_trait]
impl SyncHandler for TheuthSyncHandler {
    async fn on_sync(&self, r: &SyncResponseModel) -> Result<(), SyncHandlerError> {
        // G2 + G4: organisation keys and names from the profile.
        let mut keys = HashMap::new();
        let mut names = HashMap::new();
        for o in r
            .profile
            .iter()
            .flat_map(|p| p.organizations.iter().flatten())
        {
            if let (Some(id), Some(key)) = (o.id, &o.key) {
                let id = OrganizationId::new(id);
                keys.insert(id, key.parse::<UnsignedSharedKey>()?);
                names.insert(id, o.name.clone().unwrap_or_default());
            }
        }
        self.client
            .crypto()
            .initialize_org_crypto(InitOrgCryptoRequest {
                organization_keys: keys,
            })
            .await?;
        *self.orgs.lock().unwrap_or_else(PoisonError::into_inner) = names;

        // G3: collections stay encrypted until listed.
        let mut collections = Vec::new();
        for c in r.collections.iter().flatten() {
            match Collection::try_from(c.clone()) {
                Ok(c) => collections.push(c),
                Err(e) => tracing::warn!(id = ?c.id, error = %e, "bitwarden: skipping collection"),
            }
        }
        *self
            .collections
            .lock()
            .unwrap_or_else(PoisonError::into_inner) = collections;

        // G1: the cipher repository `ciphers().list/get/edit` read.
        let mut items = Vec::new();
        for c in r.ciphers.iter().flatten() {
            match Cipher::try_from(c.clone()) {
                Ok(cipher) => {
                    if let Some(id) = cipher.id {
                        items.push((id, cipher));
                    }
                }
                Err(e) => tracing::warn!(id = ?c.id, error = %e, "bitwarden: skipping cipher"),
            }
        }
        self.client
            .platform()
            .state()
            .get::<Cipher>()?
            .replace_all(items)
            .await?;
        Ok(())
    }
}
