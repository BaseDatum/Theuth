//! AWS Secrets Manager as an external secret backend (221 §4, §6.1): one [`AwsBackend`] per
//! connection, registered on the secrets actor with `secrets::RegisterExternal`.
//!
//! - Credentials are the user's AWS chain (`~/.aws`, a named profile); theuth stores none. The
//!   client is built from `aws_config` with the connection's profile and region, never through
//!   `std::env` mutation.
//! - `{{secret:aws/CONN/NAME}}` is `AWSCURRENT`'s `SecretString`; `#KEY` a top-level JSON key;
//!   `:-1` the `AWSPREVIOUS` stage (no other `:-N`).
//! - An expired SSO session maps to [`ExtError::Expired`] with the `aws sso login` hint; theuth
//!   never runs the device flow itself.
//! - No value is cached. `ListSecrets` pages are, for `list_ttl` (the UI and the list tool only).

use std::collections::{BTreeMap, HashMap};
use std::error::Error as StdError;
use std::sync::atomic::{AtomicBool, AtomicU8, Ordering};
use std::sync::{Mutex, PoisonError};
use std::time::{Duration, Instant};

use aws_config::BehaviorVersion;
use aws_config::timeout::TimeoutConfig;
use aws_credential_types::provider::error::CredentialsError;
use aws_sdk_secretsmanager::Client;
use aws_sdk_secretsmanager::config::Region;
use aws_sdk_secretsmanager::error::{DisplayErrorContext, ProvideErrorMetadata, SdkError};
use aws_sdk_secretsmanager::operation::describe_secret::DescribeSecretOutput;
use aws_sdk_secretsmanager::primitives::{DateTime, DateTimeFormat};
use aws_sdk_secretsmanager::types::{Filter, FilterNameStringType};
use secrets::external::{
    ConnStatus, DeleteMode, DeleteOutcome, ExtEntry, ExtError, ExtRef, ExtVersion, ExternalBackend,
    ListPage, ListQuery, Location, Value, valid_conn,
};
use zeroize::Zeroizing;

/// The placeholder namespace and always-ask backend id.
pub const BACKEND: &str = "aws";
const CURRENT: &str = "AWSCURRENT";
const PREVIOUS: &str = "AWSPREVIOUS";
const PENDING: &str = "AWSPENDING";
/// `ListSecrets` page size.
const PAGE: i32 = 100;
/// `ExtEntry.versions` holds at most this many (221 §4).
const MAX_VERSIONS: usize = 5;

/// One AWS connection's settings (`secrets.connections.<conn>`, 221 §6.1, D20). Nothing here is
/// a credential.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct AwsConnConfig {
    /// The connection name, `[a-z0-9-]{1,32}`.
    pub conn: String,
    /// A profile from `aws_profiles::AwsProfiles`; `None` = the default chain.
    pub profile: Option<String>,
    /// Secrets Manager is regional; required.
    pub region: String,
    /// The default KMS key for `create`; `None` = `aws/secretsmanager`.
    pub kms_key_id: Option<String>,
    /// List/browse scope: a `ListSecrets` name filter.
    pub prefix: Option<String>,
    /// `DeleteMode::Permanent` is refused unless this is on (D13, D20; default off).
    pub allow_permanent_delete: bool,
    /// The recovery window when a delete names none (7..=30; default 30).
    pub delete_recovery_days: u8,
    /// Every method's deadline (`secrets.external.timeout_secs`, default 20 s).
    pub timeout: Duration,
    /// How long a `ListSecrets` page is reused (`list_ttl_secs`, default 60 s).
    pub list_ttl: Duration,
}

impl AwsConnConfig {
    /// A connection with the documented defaults.
    pub fn new(conn: impl Into<String>, region: impl Into<String>) -> Self {
        Self {
            conn: conn.into(),
            profile: None,
            region: region.into(),
            kms_key_id: None,
            prefix: None,
            allow_permanent_delete: false,
            delete_recovery_days: 30,
            timeout: Duration::from_secs(20),
            list_ttl: Duration::from_secs(60),
        }
    }

    /// Checks the name, region, recovery window and that the profile is one this harness has.
    pub fn validate(&self, profiles: &aws_profiles::AwsProfiles) -> Result<(), String> {
        if !valid_conn(&self.conn) {
            return Err(format!(
                "invalid connection name {:?}: [a-z0-9-], 1 to 32 characters",
                self.conn
            ));
        }
        if self.region.trim().is_empty() {
            return Err(format!(
                "aws/{}: a region is required (Secrets Manager is regional)",
                self.conn
            ));
        }
        if !(7..=30).contains(&self.delete_recovery_days) {
            return Err(format!(
                "aws/{}: delete_recovery_days must be 7 to 30",
                self.conn
            ));
        }
        if let Some(p) = &self.profile
            && !profiles.contains(p)
        {
            return Err(format!("AWS profile {p:?} was not found on this harness"));
        }
        Ok(())
    }
}

/// `(query, cursor, deleted)` of a cached `ListSecrets` page.
type ListKey = (Option<String>, Option<String>, bool);

/// One AWS Secrets Manager connection.
pub struct AwsBackend {
    /// Fixed at build; the D20 delete settings below change live.
    cfg: AwsConnConfig,
    /// A client given from outside (tests, a custom endpoint): `lock` keeps it.
    fixed: bool,
    client: tokio::sync::RwLock<Option<Client>>,
    status: Mutex<ConnStatus>,
    lists: Mutex<HashMap<ListKey, (Instant, ListPage)>>,
    allow_permanent_delete: AtomicBool,
    delete_recovery_days: AtomicU8,
}

impl AwsBackend {
    /// A connection whose client is built from the AWS config on first use (and again after
    /// `lock`).
    pub fn new(cfg: AwsConnConfig) -> Self {
        Self::build(cfg, None)
    }

    /// A connection over a ready client.
    pub fn with_client(cfg: AwsConnConfig, client: Client) -> Self {
        Self::build(cfg, Some(client))
    }

    fn build(cfg: AwsConnConfig, client: Option<Client>) -> Self {
        Self {
            allow_permanent_delete: AtomicBool::new(cfg.allow_permanent_delete),
            delete_recovery_days: AtomicU8::new(cfg.delete_recovery_days),
            cfg,
            fixed: client.is_some(),
            client: tokio::sync::RwLock::new(client),
            status: Mutex::new(ConnStatus::Ready),
            lists: Mutex::default(),
        }
    }

    /// This connection's settings as built (the delete settings may have changed since).
    pub fn config(&self) -> &AwsConnConfig {
        &self.cfg
    }

    /// D20 `allow_permanent_delete` / `delete_recovery_days` changed. A window outside 7..=30 is
    /// ignored (the settings layer validates it).
    pub fn set_delete_policy(&self, allow_permanent: bool, recovery_days: u8) {
        self.allow_permanent_delete
            .store(allow_permanent, Ordering::Relaxed);
        if (7..=30).contains(&recovery_days) {
            self.delete_recovery_days
                .store(recovery_days, Ordering::Relaxed);
        }
    }

    async fn client(&self) -> Client {
        if let Some(c) = self.client.read().await.as_ref() {
            return c.clone();
        }
        let mut slot = self.client.write().await;
        if let Some(c) = slot.as_ref() {
            return c.clone();
        }
        let mut loader = aws_config::defaults(BehaviorVersion::latest())
            .region(Region::new(self.cfg.region.clone()))
            .timeout_config(
                TimeoutConfig::builder()
                    .connect_timeout(Duration::from_secs(5))
                    .operation_timeout(Duration::from_secs(15))
                    .build(),
            );
        if let Some(p) = &self.cfg.profile {
            loader = loader.profile_name(p);
        }
        let client = Client::new(&loader.load().await);
        *slot = Some(client.clone());
        client
    }

    /// Runs `f` under the connection's deadline and records the state it reveals.
    async fn run<T>(&self, f: impl Future<Output = Result<T, ExtError>>) -> Result<T, ExtError> {
        let out = tokio::time::timeout(self.cfg.timeout, f)
            .await
            .unwrap_or(Err(ExtError::Timeout));
        let seen = match &out {
            Ok(_) => Some(ConnStatus::Ready),
            Err(ExtError::Expired(hint)) => Some(ConnStatus::Expired { hint: hint.clone() }),
            Err(ExtError::LoggedOut) => Some(ConnStatus::LoggedOut),
            Err(_) => None,
        };
        if let Some(s) = seen {
            *self.status.lock().unwrap_or_else(PoisonError::into_inner) = s;
        }
        out
    }

    fn sso_hint(&self) -> String {
        match &self.cfg.profile {
            Some(p) => format!("run aws sso login --profile {p}, then retry"),
            None => "run aws sso login, then retry".into(),
        }
    }

    fn map_err<E>(&self, e: SdkError<E>) -> ExtError
    where
        E: ProvideErrorMetadata + StdError + Send + Sync + 'static,
    {
        if let Some(mapped) = self.credentials_err(&e) {
            return mapped;
        }
        match &e {
            SdkError::TimeoutError(_) => ExtError::Timeout,
            SdkError::DispatchFailure(d) if d.is_timeout() => ExtError::Timeout,
            SdkError::DispatchFailure(_) => ExtError::Network(DisplayErrorContext(&e).to_string()),
            SdkError::ServiceError(_) => {
                let code = e.code().unwrap_or("Unknown");
                let msg = e.message().unwrap_or(code).to_owned();
                match code {
                    "ResourceNotFoundException" => ExtError::NotFound,
                    "ResourceExistsException" => ExtError::Exists,
                    "AccessDeniedException"
                    | "AccessDenied"
                    | "DecryptionFailure"
                    | "UnrecognizedClientException" => ExtError::Denied(msg),
                    "ExpiredTokenException" | "ExpiredToken" => ExtError::Expired(self.sso_hint()),
                    _ => ExtError::Sdk(format!("{code}: {msg}")),
                }
            }
            _ => ExtError::Sdk(DisplayErrorContext(&e).to_string()),
        }
    }

    /// A credentials-chain failure anywhere in the error: no credentials → `LoggedOut`; an
    /// expired or never-started SSO session → `Expired` with the login hint.
    fn credentials_err(&self, e: &(dyn StdError + 'static)) -> Option<ExtError> {
        let mut cur = Some(e);
        while let Some(err) = cur {
            if let Some(ce) = err.downcast_ref::<CredentialsError>() {
                if matches!(ce, CredentialsError::CredentialsNotLoaded(_)) {
                    return Some(ExtError::LoggedOut);
                }
                let text = DisplayErrorContext(ce).to_string();
                return Some(if sso_expired(&text) {
                    ExtError::Expired(self.sso_hint())
                } else {
                    ExtError::Sdk(format!("AWS credentials: {text}"))
                });
            }
            cur = err.source();
        }
        sso_expired(&DisplayErrorContext(e).to_string()).then(|| ExtError::Expired(self.sso_hint()))
    }

    fn reference(&self, name: &str) -> String {
        format!("{BACKEND}/{}/{name}", self.cfg.conn)
    }

    fn entry(
        &self,
        name: &str,
        kms: Option<&str>,
        changed: Option<&DateTime>,
        stages: Option<&HashMap<String, Vec<String>>>,
        deleted: Option<&DateTime>,
    ) -> ExtEntry {
        ExtEntry {
            r#ref: self.reference(name),
            name: name.to_owned(),
            // The type is only known from a value fetch; metadata calls never fetch one.
            kind: "string".into(),
            keys: Vec::new(),
            location: format!(
                "{} · {}",
                self.cfg.region,
                kms.unwrap_or("aws/secretsmanager")
            ),
            versions: versions(stages, changed),
            recoverable_until: deleted.and_then(restorable_until),
        }
    }

    fn forget_lists(&self) {
        self.lists
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .clear();
    }

    async fn describe_raw(&self, name: &str) -> Result<DescribeSecretOutput, ExtError> {
        self.client()
            .await
            .describe_secret()
            .secret_id(name)
            .send()
            .await
            .map_err(|e| self.map_err(e))
    }

    /// `AWSCURRENT` now, and `Conflict` when the caller saw another version.
    async fn check_expected(
        &self,
        name: &str,
        expected: Option<&ExtVersion>,
    ) -> Result<ExtVersion, ExtError> {
        let d = self.describe_raw(name).await?;
        let current = current_version(&d).ok_or(ExtError::NotFound)?;
        match expected {
            Some(e) if e.id != current.id => Err(ExtError::Conflict { current }),
            _ => Ok(current),
        }
    }

    /// `AWSCURRENT`'s string and version id.
    async fn current_string(&self, name: &str) -> Result<(Zeroizing<String>, String), ExtError> {
        let mut out = self
            .client()
            .await
            .get_secret_value()
            .secret_id(name)
            .version_stage(CURRENT)
            .send()
            .await
            .map_err(|e| self.map_err(e))?;
        let Some(s) = out.secret_string.take().map(Zeroizing::new) else {
            return Err(unsupported_binary(&out.secret_binary));
        };
        Ok((s, out.version_id.unwrap_or_default()))
    }
}

#[async_trait::async_trait]
impl ExternalBackend for AwsBackend {
    fn backend(&self) -> &'static str {
        BACKEND
    }

    fn conn(&self) -> &str {
        &self.cfg.conn
    }

    fn status(&self) -> ConnStatus {
        self.status
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .clone()
    }

    async fn probe(&self) -> Result<ConnStatus, ExtError> {
        let r = self
            .run(async {
                let mut req = self.client().await.list_secrets().max_results(1);
                if let Some(p) = &self.cfg.prefix {
                    req = req.filters(name_filter(p));
                }
                req.send().await.map_err(|e| self.map_err(e)).map(|_| ())
            })
            .await;
        let status = match r {
            Ok(()) => ConnStatus::Ready,
            Err(ExtError::Expired(hint)) => ConnStatus::Expired { hint },
            Err(ExtError::LoggedOut) => ConnStatus::LoggedOut,
            Err(e @ (ExtError::Timeout | ExtError::Network(_))) => return Err(e),
            Err(e) => {
                let s = ConnStatus::Error(e.to_string());
                *self.status.lock().unwrap_or_else(PoisonError::into_inner) = s.clone();
                s
            }
        };
        Ok(status)
    }

    async fn list(&self, q: ListQuery) -> Result<ListPage, ExtError> {
        let key = (q.query.clone(), q.cursor.clone(), q.deleted);
        if let Some((at, page)) = self
            .lists
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .get(&key)
            && at.elapsed() < self.cfg.list_ttl
        {
            return Ok(page.clone());
        }
        let page = self
            .run(async {
                let mut req = self
                    .client()
                    .await
                    .list_secrets()
                    .max_results(PAGE)
                    .set_next_token(q.cursor.clone())
                    .include_planned_deletion(q.deleted);
                if let Some(p) = &self.cfg.prefix {
                    req = req.filters(name_filter(p));
                }
                let out = req.send().await.map_err(|e| self.map_err(e))?;
                let needle = q.query.as_deref().map(str::to_lowercase);
                let entries = out
                    .secret_list()
                    .iter()
                    .filter_map(|s| {
                        let name = s.name()?;
                        let in_scope = self
                            .cfg
                            .prefix
                            .as_deref()
                            .is_none_or(|p| name.starts_with(p));
                        let hit = needle.as_deref().is_none_or(|n| {
                            name.to_lowercase().contains(n)
                                || s.description()
                                    .is_some_and(|d| d.to_lowercase().contains(n))
                        });
                        let wanted = s.deleted_date().is_some() == q.deleted;
                        (in_scope && hit && wanted).then(|| {
                            self.entry(
                                name,
                                s.kms_key_id(),
                                s.last_changed_date(),
                                s.secret_versions_to_stages(),
                                s.deleted_date(),
                            )
                        })
                    })
                    .collect();
                Ok(ListPage {
                    entries,
                    next: out.next_token().map(str::to_owned),
                })
            })
            .await?;
        self.lists
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .insert(key, (Instant::now(), page.clone()));
        Ok(page)
    }

    async fn describe(&self, r: &ExtRef) -> Result<ExtEntry, ExtError> {
        self.run(async {
            let d = self.describe_raw(&r.id).await?;
            Ok(self.entry(
                d.name().unwrap_or(&r.id),
                d.kms_key_id(),
                d.last_changed_date(),
                d.version_ids_to_stages(),
                d.deleted_date(),
            ))
        })
        .await
    }

    async fn get(&self, r: &ExtRef) -> Result<(Zeroizing<String>, ExtVersion), ExtError> {
        let stage = match r.back {
            0 => CURRENT,
            1 => PREVIOUS,
            n => {
                return Err(ExtError::Unsupported(format!(
                    "{}:-{n}: AWS keeps one previous version (:-1, AWSPREVIOUS)",
                    self.reference(&r.id)
                )));
            }
        };
        self.run(async {
            let mut out = self
                .client()
                .await
                .get_secret_value()
                .secret_id(&r.id)
                .version_stage(stage)
                .send()
                .await
                .map_err(|e| self.map_err(e))?;
            let version = ExtVersion {
                id: out.version_id.take().unwrap_or_default(),
                stages: out.version_stages.take().unwrap_or_default(),
                changed: out.created_date().and_then(rfc3339),
            };
            let Some(whole) = out.secret_string.take().map(Zeroizing::new) else {
                return Err(unsupported_binary(&out.secret_binary));
            };
            let value = match &r.key {
                None => whole,
                Some(k) => json_key(&whole, k)?,
            };
            Ok((value, version))
        })
        .await
    }

    async fn create(
        &self,
        loc: &Location,
        value: Value<'_>,
    ) -> Result<(ExtRef, ExtVersion), ExtError> {
        let Location::Aws {
            name,
            kms_key_id,
            description,
            json_key,
        } = loc
        else {
            return Err(ExtError::Unsupported(
                "an AWS connection stores AWS secrets, not Bitwarden items".into(),
            ));
        };
        if !valid_name(name) {
            return Err(ExtError::Unsupported(format!(
                "AWS secret name {name:?}: 1 to 512 of A-Z a-z 0-9 / _ + = . @ -"
            )));
        }
        let body = body(value, json_key.as_deref())?;
        let out = self
            .run(async {
                self.client()
                    .await
                    .create_secret()
                    .name(name)
                    .secret_string(body.as_str())
                    .client_request_token(uuid::Uuid::new_v4().to_string())
                    .set_kms_key_id(kms_key_id.clone().or_else(|| self.cfg.kms_key_id.clone()))
                    .set_description(description.clone())
                    .send()
                    .await
                    .map_err(|e| self.map_err(e))
            })
            .await?;
        self.forget_lists();
        Ok((
            ExtRef {
                conn: self.cfg.conn.clone(),
                id: name.clone(),
                key: json_key.clone(),
                back: 0,
            },
            ExtVersion {
                id: out.version_id().unwrap_or_default().to_owned(),
                stages: vec![CURRENT.into()],
                changed: None,
            },
        ))
    }

    async fn update(
        &self,
        r: &ExtRef,
        value: Value<'_>,
        expected: Option<&ExtVersion>,
    ) -> Result<ExtVersion, ExtError> {
        if r.back != 0 {
            return Err(ExtError::Unsupported(
                "a write makes a new current version; drop the :-1".into(),
            ));
        }
        let out = self
            .run(async {
                let current = self.check_expected(&r.id, expected).await?;
                let body = match (&r.key, value) {
                    (None, v) => body(v, None)?,
                    (Some(k), Value::Plain(s)) => {
                        // Replace one key, keep the others: read AWSCURRENT, set, write back.
                        let (whole, read_id) = self.current_string(&r.id).await?;
                        if read_id != current.id {
                            let current = self.check_expected(&r.id, None).await?;
                            return Err(ExtError::Conflict { current });
                        }
                        set_json_key(&whole, k, s)?
                    }
                    (Some(_), Value::Json(_)) => {
                        return Err(ExtError::Unsupported(
                            "a #key reference takes one plain value".into(),
                        ));
                    }
                };
                // Check-then-put, not compare-and-swap: `PutSecretValue` has no version
                // precondition, so a writer landing between `check_expected` (or the `#key`
                // read) and this put is overwritten — the window is one round trip. Its
                // value stays reachable as `AWSPREVIOUS` (`:-1`) until the next write; the
                // design doc's "As built" notes the gap.
                self.client()
                    .await
                    .put_secret_value()
                    .secret_id(&r.id)
                    .secret_string(body.as_str())
                    .client_request_token(uuid::Uuid::new_v4().to_string())
                    .send()
                    .await
                    .map_err(|e| self.map_err(e))
            })
            .await?;
        self.forget_lists();
        Ok(ExtVersion {
            id: out.version_id().unwrap_or_default().to_owned(),
            stages: out.version_stages().to_vec(),
            changed: None,
        })
    }

    async fn delete(
        &self,
        r: &ExtRef,
        mode: DeleteMode,
        expected: Option<&ExtVersion>,
    ) -> Result<DeleteOutcome, ExtError> {
        let days = match mode {
            DeleteMode::Recoverable { days } => {
                let d = days.unwrap_or(self.delete_recovery_days.load(Ordering::Relaxed));
                if !(7..=30).contains(&d) {
                    return Err(ExtError::Unsupported(format!(
                        "a recovery window of {d} days; AWS allows 7 to 30"
                    )));
                }
                Some(d)
            }
            DeleteMode::Permanent if !self.allow_permanent_delete.load(Ordering::Relaxed) => {
                return Err(ExtError::Denied(format!(
                    "permanent delete is off for {BACKEND}/{c}; Settings › Secrets › {c} › allow_permanent_delete",
                    c = self.cfg.conn
                )));
            }
            DeleteMode::Permanent => None,
        };
        let out = self
            .run(async {
                if expected.is_some() {
                    self.check_expected(&r.id, expected).await?;
                }
                let req = self.client().await.delete_secret().secret_id(&r.id);
                let req = match days {
                    Some(d) => req.recovery_window_in_days(i64::from(d)),
                    None => req.force_delete_without_recovery(true),
                };
                req.send().await.map_err(|e| self.map_err(e))
            })
            .await?;
        self.forget_lists();
        Ok(DeleteOutcome {
            recoverable_until: days.and(out.deletion_date()).and_then(rfc3339),
        })
    }

    async fn restore(&self, r: &ExtRef) -> Result<ExtVersion, ExtError> {
        let current = self
            .run(async {
                self.client()
                    .await
                    .restore_secret()
                    .secret_id(&r.id)
                    .send()
                    .await
                    .map_err(|e| self.map_err(e))?;
                self.check_expected(&r.id, None).await
            })
            .await?;
        self.forget_lists();
        Ok(current)
    }

    async fn lock(&self) {
        if !self.fixed {
            *self.client.write().await = None;
        }
        self.forget_lists();
    }

    fn apply_settings(&self, settings: &secrets::settings::ConnSettings) {
        self.set_delete_policy(
            settings.allow_permanent_delete,
            settings.delete_recovery_days,
        );
    }
}

fn name_filter(prefix: &str) -> Filter {
    Filter::builder()
        .key(FilterNameStringType::Name)
        .values(prefix)
        .build()
}

fn rfc3339(t: &DateTime) -> Option<String> {
    t.fmt(DateTimeFormat::DateTime).ok()
}

/// The earliest a pending deletion can be purged. `ListSecrets` gives `DeletedDate` as when the
/// delete happened, not the window, so this is the lower bound: AWS's 7-day minimum window.
fn restorable_until(deleted: &DateTime) -> Option<String> {
    rfc3339(&DateTime::from_secs(
        deleted.secs().checked_add(7 * 24 * 3600)?,
    ))
}

/// An SSO session that is expired or was never started: the user runs `aws sso login`.
fn sso_expired(text: &str) -> bool {
    let t = text.to_ascii_lowercase();
    t.contains("sso")
        && (t.contains("expired")
            || t.contains("sso login")
            || t.contains("unauthorizedexception")
            || t.contains("failed to load the cached sso token"))
}

fn unsupported_binary<T>(binary: &Option<T>) -> ExtError {
    if binary.is_some() {
        ExtError::Unsupported("binary secret".into())
    } else {
        ExtError::NotFound
    }
}

/// AWS's secret name alphabet: `[A-Za-z0-9/_+=.@-]{1,512}`.
fn valid_name(name: &str) -> bool {
    (1..=512).contains(&name.len())
        && name
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || b"/_+=.@-".contains(&b))
}

/// `AWSCURRENT` first, then `AWSPREVIOUS`, `AWSPENDING`, the rest by id; at most five.
fn versions(
    stages: Option<&HashMap<String, Vec<String>>>,
    changed: Option<&DateTime>,
) -> Vec<ExtVersion> {
    let rank = |s: &[String]| {
        [CURRENT, PREVIOUS, PENDING]
            .iter()
            .position(|w| s.iter().any(|x| x == w))
            .unwrap_or(3)
    };
    let mut out: Vec<ExtVersion> = stages
        .into_iter()
        .flatten()
        .map(|(id, s)| ExtVersion {
            id: id.clone(),
            stages: s.clone(),
            changed: s
                .iter()
                .any(|x| x == CURRENT)
                .then(|| changed.and_then(rfc3339))
                .flatten(),
        })
        .collect();
    out.sort_by(|a, b| {
        rank(&a.stages)
            .cmp(&rank(&b.stages))
            .then_with(|| a.id.cmp(&b.id))
    });
    out.truncate(MAX_VERSIONS);
    out
}

fn current_version(d: &DescribeSecretOutput) -> Option<ExtVersion> {
    versions(d.version_ids_to_stages(), d.last_changed_date())
        .into_iter()
        .find(|v| v.stages.iter().any(|s| s == CURRENT))
}

/// The top-level JSON key `key` of `whole` as text (string, number or bool).
fn json_key(whole: &str, key: &str) -> Result<Zeroizing<String>, ExtError> {
    let Ok(serde_json::Value::Object(mut map)) = serde_json::from_str(whole) else {
        return Err(ExtError::Unsupported(format!(
            "#{key}: the secret is not a JSON object"
        )));
    };
    match map.remove(key) {
        None => Err(ExtError::NotFound),
        Some(serde_json::Value::String(s)) => Ok(Zeroizing::new(s)),
        Some(serde_json::Value::Number(n)) => Ok(Zeroizing::new(n.to_string())),
        Some(serde_json::Value::Bool(b)) => Ok(Zeroizing::new(b.to_string())),
        Some(_) => Err(ExtError::Unsupported(format!(
            "#{key} is not a string, number or bool"
        ))),
    }
}

/// `whole` (a JSON object, or empty) with `key` set to `value`.
fn set_json_key(whole: &str, key: &str, value: &str) -> Result<Zeroizing<String>, ExtError> {
    let mut map = if whole.trim().is_empty() {
        serde_json::Map::new()
    } else {
        match serde_json::from_str(whole) {
            Ok(serde_json::Value::Object(m)) => m,
            _ => {
                return Err(ExtError::Unsupported(format!(
                    "#{key}: the secret is not a JSON object"
                )));
            }
        }
    };
    map.insert(key.to_owned(), serde_json::Value::String(value.to_owned()));
    serde_json::to_string(&map)
        .map(Zeroizing::new)
        .map_err(|e| ExtError::Sdk(e.to_string()))
}

/// The `SecretString` a create or whole-value update writes.
fn body(value: Value<'_>, json_key: Option<&str>) -> Result<Zeroizing<String>, ExtError> {
    let pairs: BTreeMap<&str, &str> = match (value, json_key) {
        (Value::Plain(s), None) => return Ok(Zeroizing::new(s.to_owned())),
        (Value::Plain(s), Some(k)) => [(k, s)].into(),
        (Value::Json(p), None) => p.into_iter().collect(),
        (Value::Json(_), Some(_)) => {
            return Err(ExtError::Unsupported(
                "key/value pairs already name their keys; drop json_key".into(),
            ));
        }
    };
    serde_json::to_string(&pairs)
        .map(Zeroizing::new)
        .map_err(|e| ExtError::Sdk(e.to_string()))
}

#[cfg(test)]
mod tests;
