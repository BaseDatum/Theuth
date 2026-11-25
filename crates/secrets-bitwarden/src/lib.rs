//! Bitwarden Password Manager as a theuth external secret backend (design 221 §5, §6.2), on the
//! official `bitwarden/sdk-internal` crates (pinned in the workspace manifest), in process.
//!
//! One [`BwBackend`] is one connection. Login is the personal API key: the `secrets` crate keeps
//! `client_id` / `client_secret` sealed in `meta` and hands them over with
//! [`BwBackend::set_api_key`]; "unlock" is then a fresh API-key login with the typed master
//! password ([`BwBackend::unlock`]), which also initialises the user's crypto (§5.2). The derived
//! keys and the synced (still encrypted) ciphers live only in the SDK client, in memory; lock drops
//! the whole client.

pub mod config;
pub mod fields;
pub mod refs;
mod sync;

use std::collections::HashMap;
use std::fmt::Display;
use std::sync::{Arc, Mutex, PoisonError, RwLock, Weak};
use std::time::{Duration, Instant};

use bitwarden_auth::token_management::PasswordManagerTokenHandler;
use bitwarden_core::auth::login::ApiKeyLoginRequest;
use bitwarden_core::key_management::SymmetricKeySlotId;
use bitwarden_core::{
    ApiError, Client, ClientBuilder, ClientSettings, FromClient as _, OrganizationId, UserId,
};
use bitwarden_send::SendSyncHandler;
use bitwarden_sync::{SyncClient, SyncError, SyncRequest};
use bitwarden_vault::{
    CipherEditRequest, CipherId, CipherView, CreateCipherError, DeleteCipherError, EditCipherError,
    FolderSyncHandler, GetCipherError, RestoreCipherError, VaultClientExt as _,
};
use chrono::Utc;
use secrets::external::{
    ConnStatus, DeleteMode, DeleteOutcome, ExtEntry, ExtError, ExtRef, ExtVersion, ExternalBackend,
    ListPage, ListQuery, Location, Value,
};
use tokio::sync::{Mutex as AsyncMutex, MutexGuard};
use zeroize::Zeroizing;

pub use config::{BwConfig, LoginKind, Server, Urls, WriteScope};
use fields::{NewItem, created_key, field_keys, read_field, same_version, version, write_field};
pub use refs::{BwKey, format_ref, parse_ref};
use sync::TheuthSyncHandler;

/// The backend id: `{{secret:bw/…}}`.
pub const BACKEND: &str = "bw";
/// Entries per [`ListPage`].
const PAGE: usize = 200;
/// How long the cloud keeps an item in the trash (self-hosted: the server's own setting).
const TRASH_DAYS: i64 = 30;

/// A personal API key (Account settings › Security › Keys).
pub struct ApiKey {
    /// `user.<uuid>`.
    pub client_id: String,
    /// The secret half.
    pub client_secret: Zeroizing<String>,
}

/// One unlocked connection: the SDK client and its single sync client (G6).
struct Session {
    client: Client,
    sync: SyncClient,
    handler: Arc<TheuthSyncHandler>,
    last_sync: Option<Instant>,
}

/// A Bitwarden connection.
pub struct BwBackend {
    me: Weak<BwBackend>,
    conn: String,
    cfg: BwConfig,
    urls: Result<Urls, String>,
    status: RwLock<ConnStatus>,
    creds: Mutex<Option<ApiKey>>,
    /// Serialises every SDK operation of this connection (one sync at a time, edits after syncs).
    session: AsyncMutex<Option<Session>>,
    last_use: Mutex<Instant>,
    idle: Mutex<Option<tokio::task::JoinHandle<()>>>,
}

impl BwBackend {
    /// A logged-out connection. `cfg.urls()` errors surface as `Unsupported` status.
    pub fn new(conn: impl Into<String>, cfg: BwConfig) -> Arc<Self> {
        let urls = cfg.urls();
        let status = match &urls {
            Ok(_) => ConnStatus::LoggedOut,
            Err(e) => ConnStatus::Unsupported(e.clone()),
        };
        Arc::new_cyclic(|me| Self {
            me: me.clone(),
            conn: conn.into(),
            cfg,
            urls,
            status: RwLock::new(status),
            creds: Mutex::new(None),
            session: AsyncMutex::new(None),
            last_use: Mutex::new(Instant::now()),
            idle: Mutex::new(None),
        })
    }

    /// The connection's settings.
    pub fn config(&self) -> &BwConfig {
        &self.cfg
    }

    /// The web vault URL Send links are built on (§6.2), when the URLs are valid.
    pub fn web_vault_url(&self) -> Option<&str> {
        self.urls.as_ref().ok().map(|u| u.web_vault.as_str())
    }

    /// Install (or replace) the API key: the connection becomes `Locked`. A replaced key locks a
    /// session that was open under the old one.
    pub async fn set_api_key(&self, key: ApiKey) {
        self.lock_now().await;
        *self.creds.lock().unwrap_or_else(PoisonError::into_inner) = Some(key);
        self.refresh_status();
    }

    /// Forget the API key and lock: `LoggedOut`.
    pub async fn logout(&self) {
        self.creds
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .take();
        self.lock_now().await;
    }

    /// API-key login with the master password: authenticates, derives the user key (the KDF runs
    /// on a blocking thread), syncs. `Ready` on success; on failure the connection is left locked.
    pub async fn unlock(&self, password: Zeroizing<String>) -> Result<ConnStatus, ExtError> {
        let r = self.open(password).await;
        if r.is_err() {
            self.refresh_status();
        }
        r
    }

    async fn open(&self, password: Zeroizing<String>) -> Result<ConnStatus, ExtError> {
        let urls = self.urls.clone().map_err(ExtError::Unsupported)?;
        let (client_id, client_secret) = {
            let creds = self.creds.lock().unwrap_or_else(PoisonError::into_inner);
            let k = creds.as_ref().ok_or(ExtError::LoggedOut)?;
            (k.client_id.clone(), k.client_secret.to_string())
        };
        let user_id: UserId = client_id
            .strip_prefix("user.")
            .and_then(|u| u.parse().ok())
            .ok_or_else(|| {
                ExtError::Denied(
                    "the client_id is not a personal API key (user.<id>); organisation and \
                     machine keys cannot open a personal vault"
                        .into(),
                )
            })?;
        let mut guard = self.session.lock().await;
        if let Some(s) = guard.take() {
            s.client.internal.get_key_store().clear();
        }
        let client = ClientBuilder::new()
            .with_settings(ClientSettings {
                identity_url: urls.identity,
                api_url: urls.api,
                user_agent: "theuth".into(),
                ..Default::default()
            })
            .with_token_handler(Arc::new(PasswordManagerTokenHandler::default()))
            .build();
        let login = {
            let client = client.clone();
            let req = ApiKeyLoginRequest {
                client_id,
                client_secret,
                password: password.to_string(),
            };
            let rt = tokio::runtime::Handle::current();
            tokio::task::spawn_blocking(move || {
                rt.block_on(async move { client.auth().login_api_key(&req).await })
            })
        };
        let resp = self
            .timed(async { login.await.map_err(|e| ExtError::Sdk(e.to_string())) })
            .await??
            .map_err(|e| match e {
                bitwarden_core::auth::login::LoginError::Api(e) => api_err(&e),
                bitwarden_core::auth::login::LoginError::IdentityFail(_) => {
                    ExtError::Denied("Bitwarden refused the API key".into())
                }
                e @ (bitwarden_core::auth::login::LoginError::Crypto(_)
                | bitwarden_core::auth::login::LoginError::EncryptionSettings(_)) => {
                    tracing::debug!(error = %e, "bitwarden unlock");
                    ExtError::Denied("wrong master password".into())
                }
                e => ExtError::Sdk(e.to_string()),
            })?;
        if !resp.authenticated {
            return Err(ExtError::Denied("Bitwarden refused the API key".into()));
        }
        if !has_user_key(&client) {
            return Err(ExtError::Unsupported(
                "the server's token response carries no master-password unlock data; this server \
                 version is too old for API-key unlock"
                    .into(),
            ));
        }
        client
            .internal
            .init_user_id(user_id)
            .await
            .map_err(|e| ExtError::Sdk(e.to_string()))?;
        let handler = Arc::new(TheuthSyncHandler::new(client.clone()));
        let sync = SyncClient::new(client.clone());
        sync.register_sync_handler(Arc::new(FolderSyncHandler::from_client(&client)));
        sync.register_sync_handler(Arc::new(SendSyncHandler::from_client(&client)));
        sync.register_sync_handler(handler.clone());
        let mut s = Session {
            client,
            sync,
            handler,
            last_sync: None,
        };
        self.sync(&mut s, true).await?;
        *guard = Some(s);
        drop(guard);
        self.set_status(ConnStatus::Ready);
        self.touch();
        Ok(ConnStatus::Ready)
    }

    fn set_status(&self, s: ConnStatus) {
        *self.status.write().unwrap_or_else(PoisonError::into_inner) = s;
    }

    /// `Locked` with credentials, `LoggedOut` without (or `Unsupported` for bad URLs).
    fn refresh_status(&self) {
        let s = match &self.urls {
            Err(e) => ConnStatus::Unsupported(e.clone()),
            Ok(_)
                if self
                    .creds
                    .lock()
                    .unwrap_or_else(PoisonError::into_inner)
                    .is_some() =>
            {
                ConnStatus::Locked
            }
            Ok(_) => ConnStatus::LoggedOut,
        };
        self.set_status(s);
    }

    async fn lock_now(&self) {
        if let Some(h) = self
            .idle
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .take()
        {
            h.abort();
        }
        if let Some(s) = self.session.lock().await.take() {
            // `KeyStore::clear` drops the user and org keys; dropping the client drops the
            // in-memory state (ciphers, still encrypted) with it.
            s.client.internal.get_key_store().clear();
        }
        self.refresh_status();
    }

    /// Record a use and (re)arm the idle lock.
    fn touch(&self) {
        *self.last_use.lock().unwrap_or_else(PoisonError::into_inner) = Instant::now();
        let idle = self.cfg.lock_after_idle_secs;
        if idle == 0 {
            return;
        }
        let mut slot = self.idle.lock().unwrap_or_else(PoisonError::into_inner);
        if slot.as_ref().is_some_and(|h| !h.is_finished()) {
            return;
        }
        let me = self.me.clone();
        let idle = Duration::from_secs(idle);
        *slot = Some(tokio::spawn(async move {
            loop {
                let Some(b) = me.upgrade() else { return };
                let last = *b.last_use.lock().unwrap_or_else(PoisonError::into_inner);
                let due = last + idle;
                if Instant::now() >= due {
                    // Not `lock_now`: it would abort this very task before the lock finishes.
                    if let Some(s) = b.session.lock().await.take() {
                        s.client.internal.get_key_store().clear();
                    }
                    b.refresh_status();
                    tracing::info!(conn = %b.conn, "bitwarden: locked after idle");
                    return;
                }
                drop(b);
                tokio::time::sleep_until(due.into()).await;
            }
        }));
    }

    async fn timed<T>(&self, f: impl Future<Output = T>) -> Result<T, ExtError> {
        tokio::time::timeout(Duration::from_secs(self.cfg.timeout_secs.max(1)), f)
            .await
            .map_err(|_| ExtError::Timeout)
    }

    /// The open session, or why there is none.
    async fn ready(&self) -> Result<MutexGuard<'_, Option<Session>>, ExtError> {
        let guard = self.session.lock().await;
        if guard.is_some() {
            return Ok(guard);
        }
        Err(match self.status() {
            ConnStatus::Unsupported(e) => ExtError::Unsupported(e),
            ConnStatus::LoggedOut => ExtError::LoggedOut,
            _ => ExtError::Locked,
        })
    }

    /// `/sync` (revision-date check first unless `force`).
    async fn sync(&self, s: &mut Session, force: bool) -> Result<(), ExtError> {
        self.timed(s.sync.sync(SyncRequest {
            force,
            exclude_subdomains: Some(true),
        }))
        .await?
        .map_err(|e| match e {
            SyncError::Api(e) => api_err(&e),
            SyncError::AccountDeleted => ExtError::Denied("the account was deleted".into()),
            e => ExtError::Sdk(e.to_string()),
        })?;
        s.last_sync = Some(Instant::now());
        Ok(())
    }

    /// Sync when the last one is older than `sync_max_age_secs` (reads) or always (writes; the
    /// revision-date check keeps a no-change sync to one small request).
    async fn fresh(&self, s: &mut Session, write: bool) -> Result<(), ExtError> {
        let max = Duration::from_secs(self.cfg.sync_max_age_secs);
        if write || s.last_sync.is_none_or(|t| t.elapsed() > max) {
            self.sync(s, false).await?;
        }
        Ok(())
    }

    async fn view(&self, s: &Session, id: CipherId) -> Result<CipherView, ExtError> {
        s.client
            .vault()
            .ciphers()
            .get(id)
            .await
            .map_err(|e| match e {
                GetCipherError::ItemNotFound(_) => ExtError::NotFound,
                e => ExtError::Sdk(e.to_string()),
            })
    }

    /// A live (not trashed) item.
    async fn live(&self, s: &Session, id: CipherId) -> Result<CipherView, ExtError> {
        let v = self.view(s, id).await?;
        if v.deleted_date.is_some() {
            return Err(ExtError::NotFound);
        }
        Ok(v)
    }

    fn check_expected(v: &CipherView, expected: Option<&ExtVersion>) -> Result<(), ExtError> {
        match expected {
            Some(e) if !same_version(e, v.revision_date) => Err(ExtError::Conflict {
                current: version(v.revision_date),
            }),
            _ => Ok(()),
        }
    }

    fn check_scope(&self, org: Option<OrganizationId>) -> Result<(), ExtError> {
        if org.is_some() && self.cfg.write_scope == WriteScope::Personal {
            return Err(ExtError::Denied(format!(
                "bw/{c} writes to the personal vault only; Settings › Secrets › {c} › write scope",
                c = self.conn
            )));
        }
        Ok(())
    }

    /// `org / collection / folder` names for an item.
    fn location(
        orgs: &HashMap<OrganizationId, String>,
        collections: &HashMap<String, String>,
        folders: &HashMap<String, String>,
        org: Option<OrganizationId>,
        cols: &[bitwarden_collections::collection::CollectionId],
        folder: Option<bitwarden_vault::FolderId>,
    ) -> String {
        let mut parts = vec![match org {
            Some(o) => orgs.get(&o).cloned().unwrap_or_else(|| o.to_string()),
            None => "personal".to_owned(),
        }];
        let names: Vec<_> = cols
            .iter()
            .filter_map(|c| collections.get(&c.to_string()).cloned())
            .collect();
        if !names.is_empty() {
            parts.push(names.join(", "));
        }
        if let Some(f) = folder.and_then(|f| folders.get(&f.to_string())) {
            parts.push(f.clone());
        }
        parts.join(" / ")
    }

    /// Org, collection and folder names, decrypted (metadata, §11 item 2).
    async fn names(
        &self,
        s: &Session,
    ) -> (
        HashMap<OrganizationId, String>,
        HashMap<String, String>,
        HashMap<String, String>,
    ) {
        let folders = s
            .client
            .vault()
            .folders()
            .list()
            .await
            .unwrap_or_default()
            .into_iter()
            .filter_map(|f| Some((f.id?.to_string(), f.name)))
            .collect();
        let collections = s
            .client
            .vault()
            .collections()
            .decrypt_list(s.handler.collections())
            .unwrap_or_default()
            .into_iter()
            .filter_map(|c| Some((c.id?.to_string(), c.name)))
            .collect();
        (s.handler.orgs(), collections, folders)
    }

    fn entry_ref(&self, id: CipherId) -> String {
        format!("bw/{}/{}", self.conn, id)
    }
}

fn has_user_key(c: &Client) -> bool {
    c.internal
        .get_key_store()
        .context()
        .has_symmetric_key(SymmetricKeySlotId::User)
}

fn cipher_id(r: &ExtRef) -> Result<CipherId, ExtError> {
    if !refs::valid_item_id(&r.id) {
        return Err(ExtError::NotFound);
    }
    uuid::Uuid::parse_str(&r.id)
        .map(CipherId::new)
        .map_err(|_| ExtError::NotFound)
}

fn parse_uuid(what: &str, id: &str) -> Result<uuid::Uuid, ExtError> {
    uuid::Uuid::parse_str(id).map_err(|_| ExtError::Sdk(format!("invalid {what} id {id:?}")))
}

/// An HTTP-level SDK error as an [`ExtError`].
fn api_err(e: &ApiError) -> ExtError {
    match e {
        ApiError::Response(r) => {
            let msg = server_message(&r.message);
            match r.status.as_u16() {
                401 => ExtError::Expired(
                    "Bitwarden rejected the session; unlock the connection again".into(),
                ),
                403 => ExtError::Denied(msg),
                404 => ExtError::NotFound,
                code => ExtError::Sdk(format!("Bitwarden answered {code}: {msg}")),
            }
        }
        ApiError::Reqwest(_) | ApiError::ReqwestMiddleware(_) | ApiError::Io(_) => {
            ExtError::Network(e.to_string())
        }
        ApiError::Serde(_) => ExtError::Sdk(e.to_string()),
    }
}

/// The `message` of a Bitwarden / Vaultwarden error body, else the body.
fn server_message(body: &str) -> String {
    serde_json::from_str::<serde_json::Value>(body)
        .ok()
        .and_then(|v| {
            v.get("message")
                .or_else(|| v.get("Message"))
                .and_then(|m| m.as_str())
                .map(str::to_owned)
        })
        .unwrap_or_else(|| body.chars().take(200).collect())
}

/// Bitwarden refuses an edit built on a stale revision with this text (Vaultwarden and the
/// official server).
fn is_stale(e: &ApiError) -> bool {
    matches!(e, ApiError::Response(r)
        if r.status.as_u16() == 400 && r.message.contains("out of date"))
}

fn sdk<E: Display>(e: E) -> ExtError {
    ExtError::Sdk(e.to_string())
}

#[async_trait::async_trait]
impl ExternalBackend for BwBackend {
    fn backend(&self) -> &'static str {
        BACKEND
    }

    fn conn(&self) -> &str {
        &self.conn
    }

    fn status(&self) -> ConnStatus {
        self.status
            .read()
            .unwrap_or_else(PoisonError::into_inner)
            .clone()
    }

    async fn probe(&self) -> Result<ConnStatus, ExtError> {
        let mut guard = self.session.lock().await;
        let Some(s) = guard.as_mut() else {
            return Ok(self.status());
        };
        match self.sync(s, false).await {
            Ok(()) => Ok(ConnStatus::Ready),
            Err(e @ ExtError::Expired(_)) => {
                let st = ConnStatus::Expired {
                    hint: e.to_string(),
                };
                self.set_status(st.clone());
                Ok(st)
            }
            Err(e) => Ok(ConnStatus::Error(e.to_string())),
        }
    }

    async fn list(&self, q: ListQuery) -> Result<ListPage, ExtError> {
        let mut guard = self.ready().await?;
        let s = guard.as_mut().expect("ready() checked");
        self.fresh(s, false).await?;
        let list = s.client.vault().ciphers().list().await.map_err(sdk)?;
        let (orgs, collections, folders) = self.names(s).await;
        let needle = q.query.as_deref().map(str::to_lowercase);
        let mut entries: Vec<ExtEntry> = list
            .successes
            .into_iter()
            .filter(|c| c.deleted_date.is_none())
            .filter_map(|c| {
                let id = c.id?;
                let location = Self::location(
                    &orgs,
                    &collections,
                    &folders,
                    c.organization_id,
                    &c.collection_ids,
                    c.folder_id,
                );
                if let Some(n) = &needle
                    && !c.name.to_lowercase().contains(n)
                    && !location.to_lowercase().contains(n)
                {
                    return None;
                }
                Some(ExtEntry {
                    r#ref: self.entry_ref(id),
                    name: c.name,
                    kind: kind_of_list(&c.r#type).into(),
                    keys: Vec::new(),
                    location,
                    versions: vec![version(c.revision_date)],
                })
            })
            .collect();
        entries.sort_by(|a, b| a.name.cmp(&b.name).then_with(|| a.r#ref.cmp(&b.r#ref)));
        let start: usize = q
            .cursor
            .as_deref()
            .and_then(|c| c.parse().ok())
            .unwrap_or(0);
        let next = (start + PAGE < entries.len()).then(|| (start + PAGE).to_string());
        let entries = entries.into_iter().skip(start).take(PAGE).collect();
        Ok(ListPage { entries, next })
    }

    async fn describe(&self, r: &ExtRef) -> Result<ExtEntry, ExtError> {
        let id = cipher_id(r)?;
        let mut guard = self.ready().await?;
        let s = guard.as_mut().expect("ready() checked");
        self.fresh(s, false).await?;
        let v = self.live(s, id).await?;
        let (orgs, collections, folders) = self.names(s).await;
        Ok(ExtEntry {
            r#ref: self.entry_ref(id),
            kind: fields::kind_name(v.r#type).into(),
            keys: field_keys(&v),
            location: Self::location(
                &orgs,
                &collections,
                &folders,
                v.organization_id,
                &v.collection_ids,
                v.folder_id,
            ),
            versions: vec![version(v.revision_date)],
            name: v.name,
        })
    }

    async fn get(&self, r: &ExtRef) -> Result<(Zeroizing<String>, ExtVersion), ExtError> {
        let id = cipher_id(r)?;
        let key = BwKey::of(r).map_err(ExtError::Unsupported)?;
        let mut guard = self.ready().await?;
        self.touch();
        let s = guard.as_mut().expect("ready() checked");
        self.fresh(s, false).await?;
        let v = self.live(s, id).await?;
        let value = read_field(&v, key.as_ref(), r.back, &self.cfg, &self.conn, None)?;
        Ok((value, version(v.revision_date)))
    }

    async fn create(
        &self,
        loc: &Location,
        value: Value<'_>,
    ) -> Result<(ExtRef, ExtVersion), ExtError> {
        let Location::Bw {
            org,
            collections,
            folder,
            kind,
            item_name,
            field,
            username,
            uri,
        } = loc
        else {
            return Err(ExtError::Unsupported("not a Bitwarden destination".into()));
        };
        let Value::Plain(value) = value else {
            return Err(ExtError::Unsupported(
                "Bitwarden stores one value per field, not JSON key/value pairs".into(),
            ));
        };
        let org = org
            .as_ref()
            .map(|o| parse_uuid("organisation", &o.0).map(OrganizationId::new))
            .transpose()?;
        self.check_scope(org)?;
        if org.is_none() && !collections.is_empty() {
            return Err(ExtError::Unsupported(
                "collections hold organisation items; choose the organisation too".into(),
            ));
        }
        let mut req = fields::create_request(
            &NewItem {
                kind: *kind,
                item_name,
                field,
                username: username.as_deref(),
                uri: uri.as_deref(),
            },
            value,
        )?;
        req.organization_id = org;
        req.collection_ids = collections
            .iter()
            .map(|c| {
                parse_uuid("collection", &c.0)
                    .map(bitwarden_collections::collection::CollectionId::new)
            })
            .collect::<Result<_, _>>()?;
        req.folder_id = folder
            .as_ref()
            .map(|f| parse_uuid("folder", &f.0).map(bitwarden_vault::FolderId::new))
            .transpose()?;
        let mut guard = self.ready().await?;
        self.touch();
        let s = guard.as_mut().expect("ready() checked");
        self.fresh(s, true).await?;
        let v = self
            .timed(s.client.vault().ciphers().create(req))
            .await?
            .map_err(|e| match e {
                CreateCipherError::Api(e) => api_err(&e),
                e => sdk(e),
            })?;
        let id =
            v.id.ok_or_else(|| ExtError::Sdk("created item has no id".into()))?;
        Ok((
            ExtRef {
                conn: self.conn.clone(),
                id: id.to_string(),
                key: Some(created_key(field).as_key()),
                back: 0,
            },
            version(v.revision_date),
        ))
    }

    async fn update(
        &self,
        r: &ExtRef,
        value: Value<'_>,
        expected: Option<&ExtVersion>,
    ) -> Result<ExtVersion, ExtError> {
        let id = cipher_id(r)?;
        let key = BwKey::of(r).map_err(ExtError::Unsupported)?;
        let Value::Plain(value) = value else {
            return Err(ExtError::Unsupported(
                "Bitwarden stores one value per field, not JSON key/value pairs".into(),
            ));
        };
        if r.back > 0 {
            return Err(ExtError::Unsupported(
                "password history is read-only".into(),
            ));
        }
        let mut guard = self.ready().await?;
        self.touch();
        let s = guard.as_mut().expect("ready() checked");
        self.fresh(s, true).await?;
        let v = self.live(s, id).await?;
        Self::check_expected(&v, expected)?;
        self.check_scope(v.organization_id)?;
        if !v.edit {
            return Err(ExtError::Denied("this item is read-only for you".into()));
        }
        let mut req = CipherEditRequest::try_from(v).map_err(sdk)?;
        write_field(&mut req, key.as_ref(), value)?;
        match self.timed(s.client.vault().ciphers().edit(req)).await? {
            Ok(v) => Ok(version(v.revision_date)),
            Err(EditCipherError::Api(e)) if is_stale(&e) => {
                self.sync(s, true).await?;
                let v = self.view(s, id).await?;
                Err(ExtError::Conflict {
                    current: version(v.revision_date),
                })
            }
            Err(EditCipherError::Api(e)) => Err(api_err(&e)),
            Err(EditCipherError::ItemNotFound(_)) => Err(ExtError::NotFound),
            Err(e) => Err(sdk(e)),
        }
    }

    async fn delete(
        &self,
        r: &ExtRef,
        mode: DeleteMode,
        expected: Option<&ExtVersion>,
    ) -> Result<DeleteOutcome, ExtError> {
        let id = cipher_id(r)?;
        if mode == DeleteMode::Permanent && !self.cfg.allow_permanent_delete {
            return Err(ExtError::Denied(format!(
                "permanent delete is off for bw/{c}; items go to the trash. Settings › Secrets › \
                 {c} › allow permanent delete",
                c = self.conn
            )));
        }
        let mut guard = self.ready().await?;
        self.touch();
        let s = guard.as_mut().expect("ready() checked");
        self.fresh(s, true).await?;
        let v = self.view(s, id).await?;
        Self::check_expected(&v, expected)?;
        self.check_scope(v.organization_id)?;
        let ciphers = s.client.vault().ciphers();
        let map = |e: DeleteCipherError| match e {
            DeleteCipherError::Api(e) => api_err(&e),
            e => sdk(e),
        };
        match mode {
            DeleteMode::Permanent => {
                self.timed(ciphers.delete(id)).await?.map_err(map)?;
                Ok(DeleteOutcome {
                    recoverable_until: None,
                })
            }
            DeleteMode::Recoverable { .. } => {
                if v.deleted_date.is_some() {
                    return Err(ExtError::NotFound);
                }
                self.timed(ciphers.soft_delete(id)).await?.map_err(map)?;
                let until = Utc::now() + chrono::Duration::days(TRASH_DAYS);
                Ok(DeleteOutcome {
                    recoverable_until: Some(
                        until.to_rfc3339_opts(chrono::SecondsFormat::Secs, true),
                    ),
                })
            }
        }
    }

    async fn restore(&self, r: &ExtRef) -> Result<ExtVersion, ExtError> {
        let id = cipher_id(r)?;
        let mut guard = self.ready().await?;
        self.touch();
        let s = guard.as_mut().expect("ready() checked");
        self.fresh(s, true).await?;
        let v = self
            .timed(s.client.vault().ciphers().restore(id))
            .await?
            .map_err(|e| match e {
                RestoreCipherError::Api(e) => api_err(&e),
                e => sdk(e),
            })?;
        Ok(version(v.revision_date))
    }

    async fn lock(&self) {
        self.lock_now().await;
    }
}

fn kind_of_list(t: &bitwarden_vault::CipherListViewType) -> &'static str {
    use bitwarden_vault::CipherListViewType as T;
    match t {
        T::Login(_) => "login",
        T::SecureNote => "secure_note",
        T::Card(_) => "card",
        T::Identity => "identity",
        T::SshKey => "ssh_key",
        T::BankAccount(_) => "bank_account",
        T::Passport => "passport",
        T::DriversLicense => "drivers_license",
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn status_follows_credentials_and_bad_urls() {
        let b = BwBackend::new("personal", BwConfig::default());
        assert_eq!(b.status(), ConnStatus::LoggedOut);
        assert!(matches!(
            b.get(&parse_ref(&format!("bw/personal/{}", uuid::Uuid::nil())).unwrap())
                .await,
            Err(ExtError::LoggedOut)
        ));
        b.set_api_key(ApiKey {
            client_id: "user.x".into(),
            client_secret: Zeroizing::new("s".into()),
        })
        .await;
        assert_eq!(b.status(), ConnStatus::Locked);
        // A malformed client_id fails before any network call.
        assert!(matches!(
            b.unlock(Zeroizing::new("pw".into())).await,
            Err(ExtError::Denied(_))
        ));
        b.logout().await;
        assert_eq!(b.status(), ConnStatus::LoggedOut);

        let bad = BwBackend::new(
            "sh",
            BwConfig {
                server: Server::SelfHosted,
                ..Default::default()
            },
        );
        assert!(matches!(bad.status(), ConnStatus::Unsupported(_)));
    }

    #[tokio::test]
    async fn permanent_delete_needs_the_setting() {
        let b = BwBackend::new("personal", BwConfig::default());
        let r = parse_ref(&format!("bw/personal/{}", uuid::Uuid::nil())).unwrap();
        assert!(matches!(
            b.delete(&r, DeleteMode::Permanent, None).await,
            Err(ExtError::Denied(_))
        ));
        // Recoverable is not refused by the setting; it needs a session.
        assert!(matches!(
            b.delete(&r, DeleteMode::Recoverable { days: None }, None)
                .await,
            Err(ExtError::LoggedOut)
        ));
    }

    #[test]
    fn server_errors_map_without_values() {
        assert_eq!(
            server_message(
                r#"{"message":"The client copy of this cipher is out of date.","object":"error"}"#
            ),
            "The client copy of this cipher is out of date."
        );
        assert_eq!(server_message("plain"), "plain");
    }
}
