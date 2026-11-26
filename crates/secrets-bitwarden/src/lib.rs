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
pub mod password;
pub mod refs;
pub mod send;
pub mod sso;
mod sync;

use std::collections::HashMap;
use std::fmt::Display;
use std::sync::{Arc, Mutex, PoisonError, RwLock, Weak};
use std::time::{Duration, Instant};

use bitwarden_auth::token_management::PasswordManagerTokenHandler;
use bitwarden_core::auth::login::{ApiKeyLoginRequest, LoginError};
use bitwarden_core::client::login_method::UserLoginMethod;
use bitwarden_core::client::persisted_state::{
    AUTHENTICATION_TOKENS, AuthenticationTokens, USER_LOGIN_METHOD,
};
use bitwarden_core::key_management::SymmetricKeySlotId;
use bitwarden_core::key_management::account_cryptographic_state::WrappedAccountCryptographicState;
use bitwarden_core::key_management::crypto::{InitUserCryptoMethod, InitUserCryptoRequest};
use bitwarden_core::key_management::{
    MasterPasswordAuthenticationData, MasterPasswordUnlockData, UserDecryptionData,
};
use bitwarden_core::{
    ApiError, Client, ClientBuilder, ClientSettings, FromClient as _, OrganizationId, UserId,
};
use bitwarden_crypto::{EncString, Kdf};
use bitwarden_send::SendSyncHandler;
use bitwarden_sync::{SyncClient, SyncError, SyncRequest};
use bitwarden_vault::{
    CipherEditRequest, CipherId, CipherView, CreateCipherError, DeleteCipherError, EditCipherError,
    FolderSyncHandler, GetCipherError, RestoreCipherError, VaultClientExt as _,
};
use chrono::Utc;
use secrets::external::{
    ConnStatus, DeleteMode, DeleteOutcome, ExtEntry, ExtError, ExtRef, ExtVersion, ExternalBackend,
    ListPage, ListQuery, Location, LoginForm, Value,
};
use tokio::sync::{Mutex as AsyncMutex, MutexGuard};
use zeroize::Zeroizing;

pub use config::{BwConfig, LoginKind, Server, Urls, WriteScope};
use fields::{NewItem, created_key, field_keys, read_field, same_version, version, write_field};
use password::TokenOutcome;
pub use password::{LoginStep, PasswordLogin, TwoFactorKind};
pub use refs::{BwKey, format_ref, parse_ref};
use sync::TheuthSyncHandler;

/// Bitwarden's KDF salt: the account email, trimmed and lowercased.
fn salt(email: &str) -> String {
    email.trim().to_lowercase()
}

/// The master-password authentication hash the identity server checks (KDF on a blocking
/// thread).
async fn auth_hash(
    password: &Zeroizing<String>,
    kdf: &Kdf,
    email: &str,
) -> Result<String, ExtError> {
    let (password, kdf, email) = (password.clone(), kdf.clone(), email.to_owned());
    tokio::task::spawn_blocking(move || {
        MasterPasswordAuthenticationData::derive(&password, &kdf, &email)
            .map(|d| d.master_password_authentication_hash.to_string())
    })
    .await
    .map_err(sdk)?
    .map_err(sdk)
}

/// The sdk-internal commit [`BW_CLIENT_VERSION`] belongs to: the workspace `Cargo.toml`'s `rev` of
/// every `bitwarden-*` crate (a test checks they agree).
pub const BW_SDK_REV: &str = "2b06afa3b3579af116cd60f83d65b9e411fd2a56";

/// The Bitwarden client release whose data model the pinned SDK ([`BW_SDK_REV`]) speaks, sent as
/// `Bitwarden-Client-Version` on every request (the SDK client's default headers, [`client_settings`]).
/// Bump together with sdk-internal.
///
/// The identity server refuses a token request without it ("No client version header found",
/// bitwarden/server `src/Identity/IdentityServer/RequestValidators/ClientVersionValidator.cs`
/// L31, L50-54) and, for accounts with V2 encryption, one below
/// `MinimumClientVersionForV2Encryption` = 2026.10.1 (`src/Core/KeyManagement/Constants.cs` L5;
/// L58-71 of the validator: `invalid_client_version`). Feature gates on the same header live in
/// `src/Core/Constants.cs` (L34-40).
///
/// Derivation: `2b06afa` is npm `@bitwarden/sdk-internal@0.2.0-main.1259` (its `gitHead`), published
/// 2026-11-24. bitwarden/clients' latest release, `cli-v2026.11.1` (2026-11-23), ships
/// `0.2.0-main.1214`; clients `main` on 2026-11-24..26 is still `apps/cli` 2026.11.1 with
/// `0.2.0-main.1258`. So the pin is the 2026.11.1 line; no later client release exists.
pub const BW_CLIENT_VERSION: &str = "2026.11.1";

/// The SDK settings of every client: its default headers go on SDK and raw requests alike, as
/// Bitwarden's clients send them (bitwarden/clients `libs/common/src/services/api.service.ts`
/// `applyPlatformHeaders`): `Device-Type` (the CLI type of this OS), `Bitwarden-Client-Name: cli`
/// (the SDK derives it from the device type), `Bitwarden-Client-Version` and `User-Agent`.
fn client_settings(urls: &Urls) -> ClientSettings {
    ClientSettings {
        identity_url: urls.identity.clone(),
        api_url: urls.api.clone(),
        user_agent: "theuth".into(),
        device_type: password::sdk_device_type(),
        bitwarden_client_version: Some(BW_CLIENT_VERSION.into()),
        ..Default::default()
    }
}

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

/// Where a connection keeps its credentials across restarts: the `secrets` crate's engine-local
/// `meta` rows `ext/bw/<conn>/<key>`, sealed with the engine key (D11). Keys: [`KEY_CLIENT_ID`],
/// [`KEY_CLIENT_SECRET`], [`KEY_REFRESH_TOKEN`], [`KEY_DEVICE_ID`], [`KEY_USER_KEY`].
#[async_trait::async_trait]
pub trait CredentialStore: Send + Sync + 'static {
    /// The stored value, if any.
    async fn load(&self, key: &str) -> Option<Zeroizing<String>>;
    /// Store (`Some`) or delete (`None`).
    async fn save(&self, key: &str, value: Option<&str>);
}

/// API-key login: the client id.
pub const KEY_CLIENT_ID: &str = "client_id";
/// API-key login: the client secret.
pub const KEY_CLIENT_SECRET: &str = "client_secret";
/// Password login: the refresh token (rotated tokens are saved again).
pub const KEY_REFRESH_TOKEN: &str = "refresh_token";
/// Password login: this engine's device id towards Bitwarden (kept across logins).
pub const KEY_DEVICE_ID: &str = "device_id";
/// `remember_unlock` (D12): the user key, so a restart unlocks without the master password.
/// Written only while the setting is on; deleted when it goes off and on every lock or logout.
pub const KEY_USER_KEY: &str = "user_key";

/// How [`BwBackend::unlock_with_tokens`] decrypts the user key.
enum UnlockWith<'a> {
    /// The master password (KDF).
    Master(&'a Zeroizing<String>),
    /// The sealed user key (`remember_unlock`).
    Key(&'a Zeroizing<String>),
}

#[derive(Default)]
struct Creds {
    api_key: Option<ApiKey>,
    refresh_token: Option<Zeroizing<String>>,
    device_id: Option<String>,
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
    cfg: RwLock<BwConfig>,
    urls: Result<Urls, String>,
    status: RwLock<ConnStatus>,
    creds: Mutex<Creds>,
    store: Option<Arc<dyn CredentialStore>>,
    /// Serialises every SDK operation of this connection (one sync at a time, edits after syncs).
    session: AsyncMutex<Option<Session>>,
    last_use: Mutex<Instant>,
    idle: Mutex<Option<tokio::task::JoinHandle<()>>>,
    /// Turns `true` once the connection's settings were applied: a restart's remembered unlock
    /// waits for `remember_unlock` before it uses (or deletes) the sealed key.
    settings_seen: tokio::sync::watch::Sender<bool>,
    /// §9.6: the SSO login begun and not yet completed (state, PKCE verifier), memory only.
    sso: Mutex<Option<sso::Pending>>,
}

impl BwBackend {
    /// A logged-out connection with no credential store. `cfg.urls()` errors surface as
    /// `Unsupported` status.
    pub fn new(conn: impl Into<String>, cfg: BwConfig) -> Arc<Self> {
        Self::with_store(conn, cfg, None)
    }

    /// A logged-out connection whose credentials persist in `store`; call
    /// [`BwBackend::load_credentials`] to pick up what is there.
    pub fn with_store(
        conn: impl Into<String>,
        cfg: BwConfig,
        store: Option<Arc<dyn CredentialStore>>,
    ) -> Arc<Self> {
        let urls = cfg.urls();
        let status = match &urls {
            Ok(_) => ConnStatus::LoggedOut,
            Err(e) => ConnStatus::Unsupported(e.clone()),
        };
        Arc::new_cyclic(|me| Self {
            me: me.clone(),
            conn: conn.into(),
            cfg: RwLock::new(cfg),
            urls,
            status: RwLock::new(status),
            creds: Mutex::new(Creds::default()),
            store,
            session: AsyncMutex::new(None),
            last_use: Mutex::new(Instant::now()),
            idle: Mutex::new(None),
            settings_seen: tokio::sync::watch::channel(false).0,
            sso: Mutex::new(None),
        })
    }

    /// The connection's settings.
    pub fn config(&self) -> BwConfig {
        self.cfg
            .read()
            .unwrap_or_else(PoisonError::into_inner)
            .clone()
    }

    /// Apply the connection's D20 settings (the trait's `apply_settings`). Turning
    /// `remember_unlock` on seals an open session's user key; turning it off deletes it.
    pub fn set_settings(&self, s: &secrets::settings::ConnSettings) {
        let was = {
            let mut c = self.cfg.write().unwrap_or_else(PoisonError::into_inner);
            c.allow_permanent_delete = s.allow_permanent_delete;
            c.lock_after_idle_secs = s.lock_after_idle_secs;
            c.expose_password_history = s.expose_password_history;
            c.expose_totp_seed = s.expose_totp_seed;
            c.write_scope = match s.write_scope {
                secrets::settings::WriteScope::Personal => WriteScope::Personal,
                secrets::settings::WriteScope::PersonalAndOrg => WriteScope::PersonalAndOrg,
            };
            c.send_owner_only = s.send_owner_only;
            std::mem::replace(&mut c.remember_unlock, s.remember_unlock)
        };
        self.settings_seen.send_replace(true);
        if was != s.remember_unlock
            && let (Some(me), Ok(rt)) = (self.me.upgrade(), tokio::runtime::Handle::try_current())
        {
            rt.spawn(async move { me.apply_remember().await });
        }
    }

    /// D12: seal the open session's user key while `remember_unlock` is on, delete it when off.
    /// Serialised with unlocks and locks by the session lock.
    pub async fn apply_remember(&self) {
        let guard = self.session.lock().await;
        if !self.config().remember_unlock {
            self.save(KEY_USER_KEY, None).await;
        } else if let Some(s) = guard.as_ref() {
            self.seal_user_key(&s.client).await;
        }
    }

    /// Store the unlocked user key under the engine key, when `remember_unlock` is on.
    async fn seal_user_key(&self, client: &Client) {
        if self.store.is_none() || !self.config().remember_unlock {
            return;
        }
        match client.crypto().get_user_encryption_key().await {
            Ok(k) => {
                let k = Zeroizing::new(k.to_string());
                self.save(KEY_USER_KEY, Some(&k)).await;
            }
            Err(e) => {
                tracing::warn!(conn = %self.conn, error = %e, "bitwarden: user key not sealed")
            }
        }
    }

    /// The web vault URL Send links are built on (§6.2), when the URLs are valid.
    pub fn web_vault_url(&self) -> Option<&str> {
        self.urls.as_ref().ok().map(|u| u.web_vault.as_str())
    }

    fn creds(&self) -> std::sync::MutexGuard<'_, Creds> {
        self.creds.lock().unwrap_or_else(PoisonError::into_inner)
    }

    async fn save(&self, key: &str, value: Option<&str>) {
        if let Some(s) = &self.store {
            s.save(key, value).await;
        }
    }

    /// Read the stored credentials (after a restart): `Locked` when there are some. With a sealed
    /// user key and `remember_unlock` on, unlock with it (`Ready`, no master password); with the
    /// setting off, the sealed key is deleted.
    pub async fn load_credentials(&self) {
        let Some(store) = &self.store else { return };
        let id = store.load(KEY_CLIENT_ID).await;
        let secret = store.load(KEY_CLIENT_SECRET).await;
        let refresh = store.load(KEY_REFRESH_TOKEN).await;
        let device = store.load(KEY_DEVICE_ID).await;
        {
            let mut c = self.creds();
            c.api_key = id.zip(secret).map(|(id, secret)| ApiKey {
                client_id: id.to_string(),
                client_secret: secret,
            });
            c.refresh_token = refresh;
            c.device_id = device.map(|d| d.to_string());
        }
        self.refresh_status();
        let Some(key) = store.load(KEY_USER_KEY).await else {
            return;
        };
        // The builder applies the settings right after it spawns this load.
        let mut seen = self.settings_seen.subscribe();
        // A bool, not the `watch::Ref`: the guard must not live across the awaits below.
        let applied = matches!(
            tokio::time::timeout(Duration::from_secs(10), seen.wait_for(|s| *s)).await,
            Ok(Ok(_))
        );
        drop(seen);
        if !applied || !self.config().remember_unlock {
            self.save(KEY_USER_KEY, None).await;
            return;
        }
        match self.unlock_remembered(&key).await {
            Ok(_) => {
                tracing::info!(conn = %self.conn, "bitwarden: unlocked with the remembered key")
            }
            Err(e) => tracing::warn!(conn = %self.conn, "bitwarden: remembered unlock failed: {e}"),
        }
    }

    /// Unlock with the sealed user key: a fresh login from the stored API key or refresh token,
    /// then `DecryptedKey`. A key Bitwarden no longer accepts is deleted; on failure the
    /// connection is left locked.
    async fn unlock_remembered(&self, key: &Zeroizing<String>) -> Result<ConnStatus, ExtError> {
        let r = match self.config().login {
            LoginKind::ApiKey => self.open_api_key_remembered(key).await,
            LoginKind::Password | LoginKind::Sso => self.open_refresh(UnlockWith::Key(key)).await,
        };
        match &r {
            Ok(_) => {}
            Err(ExtError::Denied(_) | ExtError::LoggedOut | ExtError::Expired(_)) => {
                self.save(KEY_USER_KEY, None).await;
                self.refresh_status();
            }
            Err(_) => self.refresh_status(),
        }
        r
    }

    /// API-key restart with the sealed key: the SDK's token handler logs in with the client
    /// credentials on the first request (no master password involved), then `DecryptedKey`.
    async fn open_api_key_remembered(
        &self,
        key: &Zeroizing<String>,
    ) -> Result<ConnStatus, ExtError> {
        let urls = self.urls.clone().map_err(ExtError::Unsupported)?;
        let (client_id, client_secret) = {
            let c = self.creds();
            let k = c.api_key.as_ref().ok_or(ExtError::LoggedOut)?;
            (k.client_id.clone(), k.client_secret.to_string())
        };
        let email = self.config().email.as_deref().map(salt).unwrap_or_default();
        let kdf = Kdf::default_pbkdf2();
        let guard = self.fresh_slot().await;
        let client = Self::new_client(&urls);
        let tokens = password::Tokens {
            access_token: String::new(),
            refresh_token: None,
            expires_in: 0,
        };
        let login = UserLoginMethod::ApiKey {
            client_id,
            client_secret,
            email: email.clone(),
            kdf: kdf.clone(),
        };
        self.unlock_with_tokens(
            guard,
            client,
            tokens,
            login,
            &email,
            kdf,
            UnlockWith::Key(key),
        )
        .await
    }

    /// Install (or replace) the API key, saving it to the store: the connection becomes
    /// `Locked`. A replaced key locks a session that was open under the old one.
    pub async fn set_api_key(&self, key: ApiKey) {
        self.lock_now().await;
        self.save(KEY_CLIENT_ID, Some(&key.client_id)).await;
        self.save(KEY_CLIENT_SECRET, Some(&key.client_secret)).await;
        self.creds().api_key = Some(key);
        self.refresh_status();
    }

    /// Forget the login (API key or refresh token, here and in the store) and lock: `LoggedOut`.
    /// The device id stays, so a later password login is the same device to Bitwarden.
    pub async fn logout(&self) {
        {
            let mut c = self.creds();
            c.api_key = None;
            c.refresh_token = None;
        }
        for k in [KEY_CLIENT_ID, KEY_CLIENT_SECRET, KEY_REFRESH_TOKEN] {
            self.save(k, None).await;
        }
        self.lock_now().await;
    }

    /// Unlock with the master password. API-key connections run a fresh API-key login, which
    /// also derives the user key; password connections renew from the stored refresh token (no
    /// 2FA) and unlock with the master password. The KDF runs on a blocking thread. `Ready` on
    /// success; on failure the connection is left locked.
    pub async fn unlock(&self, password: Zeroizing<String>) -> Result<ConnStatus, ExtError> {
        let r = match self.config().login {
            LoginKind::ApiKey => self.open_api_key(password).await,
            LoginKind::Password | LoginKind::Sso => {
                self.open_refresh(UnlockWith::Master(&password)).await
            }
        };
        if r.is_err() {
            self.refresh_status();
        }
        r
    }

    fn new_client(urls: &Urls) -> Client {
        ClientBuilder::new()
            .with_settings(client_settings(urls))
            .with_token_handler(Arc::new(PasswordManagerTokenHandler::default()))
            .build()
    }

    /// Drop the current session (if any) and keep its slot locked for the new one.
    async fn fresh_slot(&self) -> MutexGuard<'_, Option<Session>> {
        let mut guard = self.session.lock().await;
        if let Some(s) = guard.take() {
            s.client.internal.get_key_store().clear();
        }
        guard
    }

    /// Register the sync handlers (G6), sync, and install the session: `Ready`.
    async fn install(
        &self,
        mut guard: MutexGuard<'_, Option<Session>>,
        client: Client,
    ) -> Result<ConnStatus, ExtError> {
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
        self.seal_user_key(&s.client).await;
        *guard = Some(s);
        drop(guard);
        self.set_status(ConnStatus::Ready);
        self.touch();
        Ok(ConnStatus::Ready)
    }

    async fn open_api_key(&self, password: Zeroizing<String>) -> Result<ConnStatus, ExtError> {
        let urls = self.urls.clone().map_err(ExtError::Unsupported)?;
        let (client_id, client_secret) = {
            let c = self.creds();
            let k = c.api_key.as_ref().ok_or(ExtError::LoggedOut)?;
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
        let guard = self.fresh_slot().await;
        let client = Self::new_client(&urls);
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
                LoginError::Api(e) => api_err(&e),
                LoginError::IdentityFail(_) => {
                    ExtError::Denied("Bitwarden refused the API key".into())
                }
                e @ (LoginError::Crypto(_) | LoginError::EncryptionSettings(_)) => {
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
        self.install(guard, client).await
    }

    /// Password login (G7): email + master password, then a second factor or a new-device code
    /// when the server asks (call again with it). `Done` = logged in, unlocked and synced; the
    /// refresh token goes to the store so a restart unlocks with the master password alone.
    pub async fn login_password(&self, form: PasswordLogin) -> Result<LoginStep, ExtError> {
        let r = self.login_password_inner(form).await;
        if r.is_err() {
            self.refresh_status();
        }
        r
    }

    async fn login_password_inner(&self, form: PasswordLogin) -> Result<LoginStep, ExtError> {
        match self.config().login {
            LoginKind::Password => {}
            LoginKind::ApiKey => {
                return Err(ExtError::Unsupported(format!(
                    "bw/{} logs in with an API key",
                    self.conn
                )));
            }
            LoginKind::Sso => {
                return Err(ExtError::Unsupported(format!(
                    "bw/{} logs in with SSO",
                    self.conn
                )));
            }
        }
        let urls = self.urls.clone().map_err(ExtError::Unsupported)?;
        let email = salt(&form.email);
        let client = Self::new_client(&urls);
        let kdf = self.prelogin(&client, &urls, &email).await?;
        let hash = auth_hash(&form.password, &kdf, &email).await?;
        let device = self.device_id().await;
        let body = password::token_form(
            &email,
            &hash,
            &device,
            form.two_factor.as_ref().map(|(k, c)| (*k, c.as_str())),
            form.new_device_otp.as_deref().map(String::as_str),
        );
        let (status, text) = self
            .post(
                &client,
                format!("{}/connect/token", urls.identity),
                "application/x-www-form-urlencoded; charset=utf-8",
                body,
            )
            .await?;
        match password::parse_token_response(status, &text) {
            TokenOutcome::Tokens(t) => {
                let guard = self.fresh_slot().await;
                self.remember_refresh(t.refresh_token.clone()).await;
                let login = UserLoginMethod::Username {
                    client_id: password::CLIENT_ID.into(),
                    email: email.clone(),
                    kdf: kdf.clone(),
                };
                self.unlock_with_tokens(
                    guard,
                    client,
                    t,
                    login,
                    &email,
                    kdf,
                    UnlockWith::Master(&form.password),
                )
                .await?;
                Ok(LoginStep::Done)
            }
            TokenOutcome::TwoFactor(codes) => Ok(LoginStep::NeedTwoFactor {
                providers: password::supported_factors(&codes).map_err(ExtError::Unsupported)?,
            }),
            TokenOutcome::NewDevice => Ok(LoginStep::NeedNewDeviceOtp),
            TokenOutcome::Refused(why) => Err(ExtError::Denied(sso::reword_sso_required(why))),
        }
    }

    /// Ask Bitwarden to email a login code (the Email second factor).
    pub async fn send_two_factor_email(
        &self,
        email: &str,
        password: &Zeroizing<String>,
    ) -> Result<(), ExtError> {
        let urls = self.urls.clone().map_err(ExtError::Unsupported)?;
        let email = salt(email);
        let client = Self::new_client(&urls);
        let kdf = self.prelogin(&client, &urls, &email).await?;
        let hash = auth_hash(password, &kdf, &email).await?;
        let device = self.device_id().await;
        let body = password::email_code_body(&email, &hash, &device).to_string();
        let (status, text) = self
            .post(
                &client,
                format!("{}/two-factor/send-email-login", urls.api),
                "application/json",
                body,
            )
            .await?;
        if (200..300).contains(&status) {
            Ok(())
        } else {
            Err(ExtError::Denied(server_message(&text)))
        }
    }

    /// Restart path of a password connection: renew from the stored refresh token, unlock.
    async fn open_refresh(&self, with: UnlockWith<'_>) -> Result<ConnStatus, ExtError> {
        let urls = self.urls.clone().map_err(ExtError::Unsupported)?;
        let refresh = self
            .creds()
            .refresh_token
            .clone()
            .ok_or(ExtError::LoggedOut)?;
        let email = self.config().email.as_deref().map(salt).ok_or_else(|| {
            ExtError::Unsupported(format!("bw/{} has no email configured", self.conn))
        })?;
        let guard = self.fresh_slot().await;
        let client = Self::new_client(&urls);
        let kdf = self.prelogin(&client, &urls, &email).await?;
        // An already-expired access token: the SDK's middleware renews from the refresh token
        // on the first request.
        let tokens = password::Tokens {
            access_token: String::new(),
            refresh_token: Some(refresh),
            expires_in: 0,
        };
        let login = UserLoginMethod::Username {
            client_id: password::CLIENT_ID.into(),
            email: email.clone(),
            kdf: kdf.clone(),
        };
        match self
            .unlock_with_tokens(guard, client, tokens, login, &email, kdf, with)
            .await
        {
            Err(ExtError::Network(e) | ExtError::Sdk(e)) if e.contains("invalid_grant") => {
                self.remember_refresh(None).await;
                Err(ExtError::Expired(format!(
                    "the Bitwarden session of bw/{} ended; log in again",
                    self.conn
                )))
            }
            r => r,
        }
    }

    /// Hand `tokens` to the SDK (`AUTHENTICATION_TOKENS` + `USER_LOGIN_METHOD` = `login`, so it
    /// renews from the refresh token or the API key from now on), fetch the decryption data,
    /// unlock `with` the master password or the sealed user key, sync.
    #[allow(clippy::too_many_arguments)]
    async fn unlock_with_tokens(
        &self,
        guard: MutexGuard<'_, Option<Session>>,
        client: Client,
        tokens: password::Tokens,
        login: UserLoginMethod,
        email: &str,
        kdf: Kdf,
        with: UnlockWith<'_>,
    ) -> Result<ConnStatus, ExtError> {
        let state = client.platform().state();
        state
            .value::<AUTHENTICATION_TOKENS>()
            .set(AuthenticationTokens {
                access_token: tokens.access_token,
                refresh_token: tokens.refresh_token.map(|t| t.to_string()),
                expires_on: Utc::now().timestamp() + tokens.expires_in as i64,
            })
            .await
            .map_err(sdk)?;
        state
            .value::<USER_LOGIN_METHOD>()
            .set(login)
            .await
            .map_err(sdk)?;
        let api = client.internal.get_api_configurations();
        let raw = self
            .timed(api.api_client.sync_api().get(Some(true)))
            .await?
            .map_err(|e| api_err(&ApiError::from(e)))?;
        let profile = raw
            .profile
            .as_deref()
            .ok_or_else(|| ExtError::Sdk("the sync answer has no profile".into()))?;
        let user_id = UserId::new(
            profile
                .id
                .ok_or_else(|| ExtError::Sdk("the profile has no id".into()))?,
        );
        let private_key: EncString = profile
            .private_key
            .as_deref()
            .ok_or_else(|| ExtError::Sdk("the profile has no private key".into()))?
            .parse()
            .map_err(sdk)?;
        let master_unlock = raw
            .user_decryption
            .as_deref()
            .map(UserDecryptionData::try_from)
            .transpose()
            .map_err(sdk)?
            .and_then(|d| d.master_password_unlock);
        let (kdf_params, salt_, method, refused) = match with {
            UnlockWith::Master(master) => {
                let unlock = match master_unlock {
                    Some(u) => u,
                    // Older servers: the master-key-wrapped user key is the profile's `key`.
                    None => MasterPasswordUnlockData {
                        kdf: kdf.clone(),
                        master_key_wrapped_user_key: profile
                            .key
                            .as_deref()
                            .ok_or_else(|| ExtError::Sdk("the profile has no user key".into()))?
                            .parse()
                            .map_err(sdk)?,
                        salt: email.to_owned(),
                        contained_key_id: None,
                    },
                };
                (
                    unlock.kdf.clone(),
                    unlock.salt.clone(),
                    InitUserCryptoMethod::MasterPasswordUnlock {
                        password: master.to_string(),
                        master_password_unlock: unlock,
                    },
                    "wrong master password",
                )
            }
            UnlockWith::Key(key) => {
                let (k, s) = master_unlock.map_or((kdf, email.to_owned()), |u| (u.kdf, u.salt));
                (
                    k,
                    s,
                    InitUserCryptoMethod::DecryptedKey {
                        decrypted_user_key: key.to_string(),
                    },
                    "the remembered unlock no longer matches this account; unlock with the master \
                     password",
                )
            }
        };
        let req = InitUserCryptoRequest {
            user_id: Some(user_id),
            kdf_params,
            email: salt_,
            account_cryptographic_state: WrappedAccountCryptographicState::V1 { private_key },
            method,
            upgrade_token: None,
        };
        let init = {
            let client = client.clone();
            let rt = tokio::runtime::Handle::current();
            tokio::task::spawn_blocking(move || {
                rt.block_on(async move { client.crypto().initialize_user_crypto(req).await })
            })
        };
        self.timed(init).await?.map_err(sdk)?.map_err(|e| {
            tracing::debug!(error = %e, "bitwarden unlock");
            ExtError::Denied(refused.into())
        })?;
        if !has_user_key(&client) {
            return Err(ExtError::Denied(refused.into()));
        }
        let _ = client.internal.init_user_id(user_id).await;
        self.install(guard, client).await
    }

    /// The KDF of `email`'s account: `/accounts/prelogin/password`, else the legacy
    /// `/accounts/prelogin` (older Vaultwarden).
    async fn prelogin(&self, client: &Client, urls: &Urls, email: &str) -> Result<Kdf, ExtError> {
        let body = serde_json::json!({ "email": email }).to_string();
        let mut last = String::new();
        for path in ["accounts/prelogin/password", "accounts/prelogin"] {
            let (status, text) = self
                .post(
                    client,
                    format!("{}/{path}", urls.identity),
                    "application/json",
                    body.clone(),
                )
                .await?;
            if (200..300).contains(&status) {
                return password::parse_prelogin(&text).map_err(ExtError::Sdk);
            }
            last = format!("prelogin answered {status}: {}", server_message(&text));
        }
        Err(ExtError::Sdk(last))
    }

    /// This engine's device id towards Bitwarden, created once.
    async fn device_id(&self) -> String {
        if let Some(d) = self.creds().device_id.clone() {
            return d;
        }
        let stored = match &self.store {
            Some(s) => s.load(KEY_DEVICE_ID).await.map(|d| d.to_string()),
            None => None,
        };
        let id = match stored {
            Some(d) => d,
            None => {
                let d = uuid::Uuid::new_v4().to_string();
                self.save(KEY_DEVICE_ID, Some(&d)).await;
                d
            }
        };
        self.creds().device_id = Some(id.clone());
        id
    }

    /// Keep (and store) the current refresh token; `None` forgets it.
    async fn remember_refresh(&self, token: Option<Zeroizing<String>>) {
        self.save(KEY_REFRESH_TOKEN, token.as_deref().map(String::as_str))
            .await;
        self.creds().refresh_token = token;
    }

    /// A password connection's refresh token rotates on renewal; store the new one.
    async fn persist_rotated(&self, s: &Session) {
        if self.config().login == LoginKind::ApiKey {
            return;
        }
        let Ok(Some(t)) = s
            .client
            .platform()
            .state()
            .value::<AUTHENTICATION_TOKENS>()
            .try_get()
            .await
        else {
            return;
        };
        let Some(now) = t.refresh_token else { return };
        let changed = self
            .creds()
            .refresh_token
            .as_deref()
            .is_none_or(|old| *old != now);
        if changed {
            self.remember_refresh(Some(Zeroizing::new(now))).await;
        }
    }

    /// An unauthenticated POST: [`Self::request`] with a body.
    async fn post(
        &self,
        client: &Client,
        url: String,
        content_type: &str,
        body: String,
    ) -> Result<(u16, String), ExtError> {
        self.request(client, url, Some((content_type, body))).await
    }

    /// Every unauthenticated request of theuth's own (prelogin, password and SSO grants, SSO
    /// prevalidate, the email code): a GET, or a POST of `body` (content type, text). It goes
    /// through the SDK's identity HTTP client, so its TLS rules and default headers
    /// ([`client_settings`]: the client name, version and device type) apply; build no request
    /// on another client.
    pub(crate) async fn request(
        &self,
        client: &Client,
        url: String,
        body: Option<(&str, String)>,
    ) -> Result<(u16, String), ExtError> {
        let api = client.internal.get_api_configurations();
        let http = &api.identity_config.client;
        let req = match body {
            Some((content_type, body)) => http
                .post(url)
                .header("Content-Type", content_type)
                .body(body),
            None => http.get(url),
        }
        .header("Accept", "application/json");
        self.timed(async {
            let r = req
                .send()
                .await
                .map_err(|e| ExtError::Network(e.to_string()))?;
            let status = r.status().as_u16();
            let text = r
                .text()
                .await
                .map_err(|e| ExtError::Network(e.to_string()))?;
            Ok((status, text))
        })
        .await?
    }

    fn set_status(&self, s: ConnStatus) {
        *self.status.write().unwrap_or_else(PoisonError::into_inner) = s;
    }

    /// `Locked` with a login (API key or refresh token, per `login`), `LoggedOut` without, or
    /// `Unsupported` for bad URLs.
    fn refresh_status(&self) {
        let logged_in = {
            let c = self.creds();
            match self.config().login {
                LoginKind::ApiKey => c.api_key.is_some(),
                LoginKind::Password | LoginKind::Sso => c.refresh_token.is_some(),
            }
        };
        let s = match &self.urls {
            Err(e) => ConnStatus::Unsupported(e.clone()),
            Ok(_) if logged_in => ConnStatus::Locked,
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
        // D12: a lock is a lock across restarts too.
        self.save(KEY_USER_KEY, None).await;
        self.refresh_status();
    }

    /// Record a use and (re)arm the idle lock.
    fn touch(&self) {
        *self.last_use.lock().unwrap_or_else(PoisonError::into_inner) = Instant::now();
        let idle = self.config().lock_after_idle_secs;
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
                    b.save(KEY_USER_KEY, None).await;
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
        tokio::time::timeout(Duration::from_secs(self.config().timeout_secs.max(1)), f)
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
        self.persist_rotated(s).await;
        Ok(())
    }

    /// Sync when the last one is older than `sync_max_age_secs` (reads) or always (writes; the
    /// revision-date check keeps a no-change sync to one small request).
    async fn fresh(&self, s: &mut Session, write: bool) -> Result<(), ExtError> {
        let max = Duration::from_secs(self.config().sync_max_age_secs);
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
        if org.is_some() && self.config().write_scope == WriteScope::Personal {
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
                // Vaultwarden answers a missing Send / cipher with 400 "… not found".
                400 if msg.to_lowercase().contains("not found") => ExtError::NotFound,
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
            .filter(|c| c.deleted_date.is_some() == q.deleted)
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
                    recoverable_until: c.deleted_date.map(trash_until),
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
            recoverable_until: None,
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
        let value = read_field(&v, key.as_ref(), r.back, &self.config(), &self.conn, None)?;
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
        if mode == DeleteMode::Permanent && !self.config().allow_permanent_delete {
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
                Ok(DeleteOutcome {
                    recoverable_until: Some(trash_until(Utc::now())),
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

    async fn login(&self, form: LoginForm) -> Result<secrets::external::LoginStep, ExtError> {
        use secrets::external::LoginStep as Step;
        let kind = self.config().login;
        match form {
            LoginForm::ApiKey {
                client_id,
                client_secret,
            } => {
                if kind != LoginKind::ApiKey {
                    return Err(ExtError::Unsupported(format!(
                        "bw/{} logs in with {}",
                        self.conn,
                        if kind == LoginKind::Sso {
                            "SSO"
                        } else {
                            "email and master password"
                        }
                    )));
                }
                self.set_api_key(ApiKey {
                    client_id,
                    client_secret: Zeroizing::new(client_secret),
                })
                .await;
                Ok(Step::Done)
            }
            LoginForm::Password {
                email,
                password,
                two_factor,
                new_device_otp,
            } => {
                let two_factor = match two_factor {
                    None => None,
                    Some((p, code)) => Some((sso::factor(&p)?, Zeroizing::new(code))),
                };
                let step = self
                    .login_password(PasswordLogin {
                        email,
                        password: Zeroizing::new(password),
                        two_factor,
                        new_device_otp: new_device_otp.map(Zeroizing::new),
                    })
                    .await?;
                Ok(sso::wire_step(step))
            }
        }
    }

    async fn sso_begin(
        &self,
        org_identifier: &str,
        redirect_port: Option<u16>,
    ) -> Result<secrets::external::SsoBegin, ExtError> {
        BwBackend::sso_begin(self, org_identifier, redirect_port).await
    }

    async fn sso_complete(
        &self,
        state: &str,
        code_or_redirect_url: &str,
        two_factor: Option<(String, String)>,
    ) -> Result<secrets::external::LoginStep, ExtError> {
        let two_factor = match two_factor {
            None => None,
            Some((p, code)) => Some((sso::factor(&p)?, Zeroizing::new(code))),
        };
        BwBackend::sso_complete(self, state, code_or_redirect_url, two_factor)
            .await
            .map(sso::wire_step)
    }

    async fn unlock(&self, password: Zeroizing<String>) -> Result<ConnStatus, ExtError> {
        BwBackend::unlock(self, password).await
    }

    async fn logout(&self) {
        BwBackend::logout(self).await;
    }

    fn apply_settings(&self, settings: &secrets::settings::ConnSettings) {
        self.set_settings(settings);
    }

    fn sender(&self) -> Option<Arc<dyn secrets::tools_send::SendBackend>> {
        self.me
            .upgrade()
            .map(|b| b as Arc<dyn secrets::tools_send::SendBackend>)
    }
}

/// Until when a trashed item can be restored (cloud: 30 days; self-hosted: the server's setting).
fn trash_until(deleted: chrono::DateTime<Utc>) -> String {
    (deleted + chrono::Duration::days(TRASH_DAYS))
        .to_rfc3339_opts(chrono::SecondsFormat::Secs, true)
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

    /// `BW_CLIENT_VERSION` belongs to one sdk-internal commit: bumping the rev in the workspace
    /// `Cargo.toml` fails here until `BW_SDK_REV` (and, after checking, the version) follow.
    #[test]
    fn client_version_follows_the_sdk_pin() {
        let toml =
            std::fs::read_to_string(concat!(env!("CARGO_MANIFEST_DIR"), "/../../Cargo.toml"))
                .unwrap();
        let revs: Vec<&str> = toml
            .lines()
            .filter(|l| l.starts_with("bitwarden-") && l.contains("bitwarden/sdk-internal"))
            .filter_map(|l| l.split("rev = \"").nth(1)?.split('"').next())
            .collect();
        assert!(revs.len() >= 7, "{revs:?}");
        for rev in revs {
            assert_eq!(
                rev, BW_SDK_REV,
                "sdk-internal moved: re-derive BW_CLIENT_VERSION, then update BW_SDK_REV"
            );
        }
        // bitwarden/server src/Core/KeyManagement/Constants.cs L5: V2-encryption accounts need 2026.10.1.
        let v: Vec<u32> = BW_CLIENT_VERSION
            .split('.')
            .map(|p| p.parse().unwrap())
            .collect();
        assert_eq!(v.len(), 3);
        assert!(v >= vec![2026, 10, 1], "{BW_CLIENT_VERSION}");
    }

    /// One HTTP/1.1 request off `sock`: the request line and the lower-cased headers.
    async fn read_request(sock: &mut tokio::net::TcpStream) -> (String, HashMap<String, String>) {
        use tokio::io::AsyncReadExt as _;
        let mut buf = Vec::new();
        let mut chunk = [0u8; 4096];
        let head_end = loop {
            let n = sock.read(&mut chunk).await.unwrap();
            assert!(n > 0, "the client closed before the headers ended");
            buf.extend_from_slice(&chunk[..n]);
            if let Some(i) = buf.windows(4).position(|w| w == b"\r\n\r\n") {
                break i + 4;
            }
        };
        let head = String::from_utf8_lossy(&buf[..head_end]).into_owned();
        let mut lines = head.split("\r\n");
        let line = lines.next().unwrap().to_owned();
        let headers: HashMap<String, String> = lines
            .filter_map(|l| l.split_once(':'))
            .map(|(k, v)| (k.trim().to_lowercase(), v.trim().to_owned()))
            .collect();
        let len: usize = headers
            .get("content-length")
            .map_or(0, |l| l.parse().unwrap());
        while buf.len() < head_end + len {
            let n = sock.read(&mut chunk).await.unwrap();
            assert!(n > 0);
            buf.extend_from_slice(&chunk[..n]);
        }
        (line, headers)
    }

    /// The raw requests (prelogin, password grant, SSO prevalidate, SSO code exchange) carry
    /// Bitwarden's client headers, and `invalid_client_version` reads as "update theuth".
    #[tokio::test]
    async fn raw_requests_carry_the_client_headers() {
        use tokio::io::AsyncWriteExt as _;
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let base = format!("http://{}", listener.local_addr().unwrap());
        let seen = Arc::new(Mutex::new(Vec::new()));
        let log = seen.clone();
        tokio::spawn(async move {
            loop {
                let (mut sock, _) = listener.accept().await.unwrap();
                let (line, headers) = read_request(&mut sock).await;
                let (status, body) = if line.contains("/accounts/prelogin") {
                    ("200 OK", r#"{"kdf":0,"kdfIterations":5000}"#)
                } else if line.contains("/sso/prevalidate") {
                    ("200 OK", r#"{"token":"sso-token"}"#)
                } else if line.contains("/connect/token") {
                    // bitwarden/server ClientVersionValidator.cs L67-70 via CustomTokenRequestValidator.
                    (
                        "400 Bad Request",
                        r#"{"error":"invalid_client_version","error_description":"Please update your app to continue using Bitwarden","ErrorModel":{"Message":"Please update your app to continue using Bitwarden"}}"#,
                    )
                } else {
                    ("404 Not Found", "{}")
                };
                log.lock().unwrap().push((line, headers));
                let resp = format!(
                    "HTTP/1.1 {status}\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
                    body.len()
                );
                sock.write_all(resp.as_bytes()).await.unwrap();
                let _ = sock.shutdown().await;
            }
        });
        let config = |login| BwConfig {
            server: Server::SelfHosted,
            base_url: Some(base.clone()),
            login,
            email: Some("me@example.org".into()),
            ..Default::default()
        };
        let outdated = password::outdated_client();
        assert!(outdated.contains(BW_CLIENT_VERSION), "{outdated}");

        let pw = BwBackend::new("pw", config(LoginKind::Password));
        let r = pw
            .login_password(PasswordLogin {
                email: "me@example.org".into(),
                password: Zeroizing::new("pw".into()),
                two_factor: None,
                new_device_otp: None,
            })
            .await;
        assert!(
            matches!(&r, Err(ExtError::Denied(m)) if *m == outdated),
            "{r:?}"
        );

        let sso = BwBackend::new("sso", config(LoginKind::Sso));
        let begin = sso.sso_begin("acme", None).await.unwrap();
        assert!(begin.authorize_url.contains("ssoToken=sso-token"));
        let r = sso.sso_complete(&begin.state, "code", None).await;
        assert!(
            matches!(&r, Err(ExtError::Denied(m)) if *m == outdated),
            "{r:?}"
        );

        let seen = seen.lock().unwrap();
        let paths: Vec<&str> = seen.iter().map(|(l, _)| l.as_str()).collect();
        for want in [
            "POST /identity/accounts/prelogin/password ",
            "GET /identity/sso/prevalidate?domainHint=acme ",
            "POST /identity/connect/token ",
        ] {
            assert!(
                paths.iter().any(|p| p.starts_with(want)),
                "{want}: {paths:?}"
            );
        }
        assert_eq!(seen.len(), 4, "{paths:?}");
        let device_type = password::device_type().to_string();
        for (line, h) in seen.iter() {
            let get = |k: &str| h.get(k).map(String::as_str);
            assert_eq!(get("bitwarden-client-name"), Some("cli"), "{line}");
            assert_eq!(
                get("bitwarden-client-version"),
                Some(BW_CLIENT_VERSION),
                "{line}"
            );
            assert_eq!(get("device-type"), Some(device_type.as_str()), "{line}");
            assert_eq!(get("user-agent"), Some("theuth"), "{line}");
        }
    }

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

    fn settings(remember_unlock: bool) -> secrets::settings::ConnSettings {
        secrets::settings::ConnSettings {
            remember_unlock,
            ..Default::default()
        }
    }

    /// D12: a sealed user key is deleted when `remember_unlock` goes off, when a restart finds it
    /// off, and on logout. (Sealing and the remembered unlock need a server: the Vaultwarden
    /// suite.)
    #[tokio::test]
    async fn the_sealed_user_key_goes_when_remember_unlock_is_off() {
        let store = Arc::new(MemStore::default());
        let sealed = || async {
            store.save(KEY_USER_KEY, Some("k")).await;
        };
        let b = BwBackend::with_store("p", BwConfig::default(), Some(store.clone()));
        sealed().await;
        b.set_settings(&settings(true));
        b.apply_remember().await;
        assert!(
            store.load(KEY_USER_KEY).await.is_some(),
            "on without a session kept it"
        );
        b.set_settings(&settings(false));
        b.apply_remember().await;
        assert!(store.load(KEY_USER_KEY).await.is_none(), "off kept it");

        // A restart with the setting off.
        sealed().await;
        let b = BwBackend::with_store("p", BwConfig::default(), Some(store.clone()));
        b.set_settings(&settings(false));
        b.load_credentials().await;
        assert!(
            store.load(KEY_USER_KEY).await.is_none(),
            "load with off kept it"
        );

        sealed().await;
        b.set_settings(&settings(true));
        b.logout().await;
        assert!(store.load(KEY_USER_KEY).await.is_none(), "logout kept it");
    }
}
