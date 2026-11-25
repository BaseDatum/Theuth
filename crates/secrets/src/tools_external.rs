//! 221 §7: the `secret_external_*` tools. Metadata, receipts and references only: no tool here
//! ever returns a value or a length (37 D1, D21). Every one takes references as data
//! (`CONSUMES_SECRETS = false`); the gate reads their `connection` / `ref` by tool name
//! ([`crate::external::Hook`]'s `external_access`) and asks per the connection's always-ask
//! entries (D6).

use std::sync::Arc;
use std::time::Duration;

use kameo::actor::ActorRef;
use schemars::JsonSchema;
use serde::Deserialize;
use tools::{RegisterPrompt, RegisterTool, Risk, Tool, ToolCx, ToolOutput, ToolRegistry};

use crate::connections::{Destination, GetExternalSettings, ListConnections};
use crate::external::{
    BwField, BwKind, ConnStatus, DeleteMode, ExtEntry, ExtError, ExtRef, ExtVersion,
    ExternalBackend, Externals, ListQuery, Location, Value, parse_bare,
};
use crate::settings::WriteScope;
use crate::vault::{self, Vault};
use crate::{SecretsActor, expiry};

/// §7.5: the prompt paragraph, beside 37's block once a connection exists. `{conns}` is filled in.
pub const PROMPT: &str = "External managers are connected: {conns}. Reference one of their secrets like a vault \
secret: {{secret:aws/CONN/NAME}} (#key for one JSON key, :-1 for the previous version) or \
{{secret:bw/CONN/ITEM-ID#password}} (#username, #totp, #notes, #field:NAME). The user may be asked to approve \
calls that read, write, delete or share one (per their settings). secret_external_list shows what is there \
(names and places, never values); secret_external_import copies one into the vault; secret_request with a \
destination asks the user for a value and stores it in the manager. You can never see these values.";

/// What every external tool needs.
#[derive(Clone)]
pub struct ExtCx {
    /// Settings, connection list.
    pub actor: ActorRef<SecretsActor>,
    /// Local values (`from`) and imports.
    pub vault: Arc<Vault>,
    /// The registered connections.
    pub externals: Arc<Externals>,
}

/// `<backend>/<conn>/<id>[#key]`: the placeholder body of `r`.
pub fn ref_text(backend: &str, r: &ExtRef) -> String {
    let mut s = format!("{backend}/{}/{}", r.conn, r.id);
    if let Some(k) = &r.key {
        s.push('#');
        s.push_str(k);
    }
    s
}

impl ExtCx {
    async fn timeout(&self) -> Duration {
        let secs = self
            .actor
            .ask(GetExternalSettings)
            .await
            .map(|s| s.timeout_secs)
            .unwrap_or(20);
        Duration::from_secs(secs)
    }

    /// `conn`, logged in and unlocked (the unlock card when locked).
    pub(crate) async fn conn(
        &self,
        conn: &str,
        why: &str,
    ) -> Result<Arc<dyn ExternalBackend>, String> {
        let b = self.externals.by_conn(conn).ok_or_else(|| {
            format!("unknown connection {conn} on this engine; secret_external_list shows the connections")
        })?;
        match b.status() {
            ConnStatus::Locked => self.externals.request_unlock(&b, why.to_owned()).await?,
            ConnStatus::LoggedOut => {
                return Err(format!(
                    "{}/{conn} is logged out on this engine; the user logs in from the Secrets page",
                    b.backend()
                ));
            }
            _ => {}
        }
        Ok(b)
    }

    /// A bare reference (`aws/prod/x#k`), its connection ready.
    pub(crate) async fn open(
        &self,
        text: &str,
        why: &str,
    ) -> Result<(Arc<dyn ExternalBackend>, ExtRef), String> {
        let (backend, r) = parse_bare(text).ok_or_else(|| {
            format!("{text:?} is not a reference: aws/CONNECTION/NAME or bw/CONNECTION/ITEM-ID")
        })?;
        let b = self.conn(&r.conn, why).await?;
        if b.backend() != backend {
            return Err(format!(
                "{} is a {} connection, not {backend}",
                r.conn,
                b.backend()
            ));
        }
        Ok((b, r))
    }

    /// `fut` under `secrets.external.timeout_secs`; errors name `what`.
    pub(crate) async fn bounded<T>(
        &self,
        what: &str,
        backend: &str,
        fut: impl Future<Output = Result<T, ExtError>>,
    ) -> Result<T, ExtError> {
        let t = self.timeout().await;
        tokio::time::timeout(t, fut).await.unwrap_or_else(|_| {
            Err(ExtError::Network(format!(
                "{what}: {backend} did not answer in {} s",
                t.as_secs()
            )))
        })
    }

    async fn settings(&self, conn: &str) -> Result<crate::settings::ConnSettings, String> {
        self.actor
            .ask(crate::connections::ConnSettingsOf(conn.to_owned()))
            .await
            .map_err(|e| e.to_string())
    }
}

/// Register the six tools and the prompt paragraph (idempotent: a re-register replaces).
pub async fn register(reg: &ActorRef<ToolRegistry>, cx: ExtCx, conns: &[String]) {
    let tools: [Arc<dyn tools::ErasedTool>; 6] = [
        Arc::new(ListTool(cx.clone())),
        Arc::new(DescribeTool(cx.clone())),
        Arc::new(ImportTool(cx.clone())),
        Arc::new(CreateTool(cx.clone())),
        Arc::new(UpdateTool(cx.clone())),
        Arc::new(DeleteTool(cx)),
    ];
    for t in tools {
        let _ = reg.ask(RegisterTool(t)).await;
    }
    let _ = reg
        .ask(RegisterPrompt {
            plugin: "secrets-external",
            text: PROMPT.replace("{conns}", &conns.join(", ")),
        })
        .await;
}

fn version_line(v: &ExtVersion) -> String {
    let mut s = v.id.clone();
    if !v.stages.is_empty() {
        s.push(' ');
        s.push_str(&v.stages.join(","));
    }
    if let Some(c) = &v.changed {
        s.push(' ');
        s.push_str(c);
    }
    s
}

/// One `list` / `describe` line. Names and places, never a value.
pub fn entry_line(e: &ExtEntry) -> String {
    let mut parts = vec![e.r#ref.clone(), format!("{} {:?}", e.kind, e.name)];
    if !e.location.is_empty() {
        parts.push(e.location.clone());
    }
    if !e.keys.is_empty() {
        parts.push(format!("keys: {}", e.keys.join(", ")));
    }
    if let Some(v) = e.versions.first() {
        parts.push(version_line(v));
    }
    if let Some(t) = &e.recoverable_until {
        parts.push(format!("deleted, recoverable until {t}"));
    }
    parts.join(" · ")
}

fn structured(v: impl serde::Serialize) -> Option<serde_json::Value> {
    serde_json::to_value(v).ok()
}

fn err_text(what: &str, e: &ExtError) -> String {
    match e {
        ExtError::Exists => format!("{what} exists; secret_external_update replaces it"),
        ExtError::Conflict { current } => {
            format!(
                "{what} changed since (now {}); describe it again",
                current.id
            )
        }
        _ => format!("{what}: {e}"),
    }
}

/// `secret_external_list`: what an external manager holds. Names and places, never values.
#[derive(Debug, Deserialize, JsonSchema)]
pub struct ExtListArgs {
    /// `prod`, `personal`; absent = the connections, with status, and nothing else.
    #[serde(default)]
    pub connection: Option<String>,
    /// Substring on name, folder, collection.
    #[serde(default)]
    pub query: Option<String>,
    /// From the previous page's `next`.
    #[serde(default)]
    pub cursor: Option<String>,
}

/// The `secret_external_list` tool.
pub struct ListTool(pub ExtCx);

#[async_trait::async_trait]
impl Tool for ListTool {
    const NAME: &'static str = "secret_external_list";
    const RISK: Risk = Risk::ReadOnly;
    const CONSUMES_SECRETS: bool = false;
    type Args = ExtListArgs;

    fn description(&self) -> String {
        "What the connected external secret managers hold (AWS Secrets Manager, Bitwarden): names, \
         places, keys and versions, never values. Without `connection`: the connections and their \
         status (no manager is contacted). With it: that manager's entries, each with the \
         `{{secret:…}}` reference to use; `query` filters, `cursor` pages."
            .into()
    }

    async fn call(&self, a: ExtListArgs, _cx: ToolCx) -> anyhow::Result<ToolOutput> {
        let cx = &self.0;
        let Some(conn) = a.connection else {
            let conns = match cx.actor.ask(ListConnections).await {
                Ok(c) => c,
                Err(e) => return Ok(ToolOutput::err(e.to_string())),
            };
            if conns.is_empty() {
                return Ok(ToolOutput::ok("no external connections"));
            }
            let lines: Vec<String> = conns
                .iter()
                .map(|c| {
                    let status = match &c.status {
                        ConnStatus::Ready => "ready".to_owned(),
                        ConnStatus::Locked => "locked".to_owned(),
                        ConnStatus::LoggedOut => "logged out".to_owned(),
                        ConnStatus::Expired { hint } => format!("expired ({hint})"),
                        ConnStatus::Error(e) => format!("error ({e})"),
                        ConnStatus::Unsupported(e) => format!("unsupported ({e})"),
                    };
                    format!("{} · {} · {} · {status}", c.conn, c.backend, c.summary)
                })
                .collect();
            let mut out = ToolOutput::ok(lines.join("\n"));
            out.structured = structured(
                conns
                    .iter()
                    .map(|c| serde_json::json!({"connection": c.conn, "backend": c.backend, "summary": c.summary, "status": c.status}))
                    .collect::<Vec<_>>(),
            );
            return Ok(out);
        };
        let b = match cx.conn(&conn, "the agent lists its entries").await {
            Ok(b) => b,
            Err(e) => return Ok(ToolOutput::err(e)),
        };
        let q = ListQuery {
            query: a.query,
            cursor: a.cursor,
            deleted: false,
        };
        let page = match cx.bounded(&conn, b.backend(), b.list(q)).await {
            Ok(p) => p,
            Err(e) => {
                return Ok(ToolOutput::err(err_text(
                    &format!("{}/{conn}", b.backend()),
                    &e,
                )));
            }
        };
        let mut lines: Vec<String> = page.entries.iter().map(entry_line).collect();
        if lines.is_empty() {
            lines.push("no entries".into());
        }
        if let Some(n) = &page.next {
            lines.push(format!("more: cursor {n:?}"));
        }
        let mut out = ToolOutput::ok(lines.join("\n"));
        out.structured = structured(&page);
        Ok(out)
    }
}

/// `secret_external_describe`: one entry's versions and fields.
#[derive(Debug, Deserialize, JsonSchema)]
pub struct ExtDescribeArgs {
    /// `aws/prod/app/db/main` or `bw/personal/<item-id>`.
    pub r#ref: String,
}

/// The `secret_external_describe` tool.
pub struct DescribeTool(pub ExtCx);

#[async_trait::async_trait]
impl Tool for DescribeTool {
    const NAME: &'static str = "secret_external_describe";
    const RISK: Risk = Risk::ReadOnly;
    const CONSUMES_SECRETS: bool = false;
    type Args = ExtDescribeArgs;

    fn description(&self) -> String {
        "One external entry's metadata: kind, place, the keys or fields you can reference \
         (`#password`, `#host`), and its versions (the `expected` for update and delete). Never a \
         value."
            .into()
    }

    async fn call(&self, a: ExtDescribeArgs, _cx: ToolCx) -> anyhow::Result<ToolOutput> {
        let (b, r) = match self.0.open(&a.r#ref, "the agent describes an entry").await {
            Ok(x) => x,
            Err(e) => return Ok(ToolOutput::err(e)),
        };
        match self.0.bounded(&a.r#ref, b.backend(), b.describe(&r)).await {
            Ok(e) => {
                let mut lines = vec![entry_line(&e)];
                for v in e.versions.iter().skip(1) {
                    lines.push(format!("  {}", version_line(v)));
                }
                let mut out = ToolOutput::ok(lines.join("\n"));
                out.structured = structured(&e);
                Ok(out)
            }
            Err(e) => Ok(ToolOutput::err(err_text(&a.r#ref, &e))),
        }
    }
}

/// `secret_external_import`: copy an external value into the local vault under NAME.
#[derive(Debug, Deserialize, JsonSchema)]
pub struct ExtImportArgs {
    /// The reference, with `#key` / `#field` (`bw/personal/<id>#password`).
    pub r#ref: String,
    /// The local NAME (`[A-Za-z0-9_.-]{1,64}`).
    pub name: String,
    /// The local note; default `from <ref> <version>`.
    #[serde(default)]
    pub note: Option<String>,
    /// Replace an existing local secret (a new version).
    #[serde(default)]
    pub replace: bool,
    /// `30m`, `2h`, `3d` or an ISO-8601 date: the local copy is deleted then.
    #[serde(default)]
    pub expires: Option<String>,
}

/// The `secret_external_import` tool.
pub struct ImportTool(pub ExtCx);

#[async_trait::async_trait]
impl Tool for ImportTool {
    const NAME: &'static str = "secret_external_import";
    const RISK: Risk = Risk::ReadOnly;
    const CONSUMES_SECRETS: bool = false;
    type Args = ExtImportArgs;

    fn description(&self) -> String {
        "Copy an external secret's current value into the local vault under `name`, so later uses \
         follow the vault's rules and do not touch the manager. A snapshot: it does not follow \
         rotations. You never see the value. `replace: true` to update an existing name."
            .into()
    }

    async fn call(&self, a: ExtImportArgs, _cx: ToolCx) -> anyhow::Result<ToolOutput> {
        if self.0.vault.list().iter().any(|s| s.name == a.name) && !a.replace {
            return Ok(ToolOutput::err(format!(
                "{} exists; replace: true stores a new version",
                a.name
            )));
        }
        Ok(
            match import(
                &self.0,
                &a.r#ref,
                &a.name,
                a.note,
                a.expires.as_deref(),
                "the agent imports an entry",
            )
            .await
            {
                Ok((info, version)) => ToolOutput::ok(format!(
                    "stored {name} v{v} from {ref} ({ver}); {{{{secret:{name}}}}}",
                    name = a.name,
                    v = info.current,
                    r#ref = a.r#ref,
                    ver = version_line(&version),
                )),
                Err(e) => ToolOutput::err(e),
            },
        )
    }
}

/// `secret_external_import` and the owner's Import (`import_external_secret`): fetch `ref` and
/// store it as a new version of local `name` (source: the ref). The value is never returned.
pub(crate) async fn import(
    cx: &ExtCx,
    r#ref: &str,
    name: &str,
    note: Option<String>,
    expires: Option<&str>,
    why: &str,
) -> Result<(crate::SecretInfo, ExtVersion), String> {
    if !vault::valid_name(name) {
        return Err("name: [A-Za-z0-9_.-]{1,64}".into());
    }
    let now = crate::now_ms();
    let expires_ms = match expires.filter(|e| !e.trim().is_empty()) {
        Some(e) => Some(expiry::parse(e, now)?.at(now)),
        None => None,
    };
    let (b, r) = cx.open(r#ref, why).await?;
    if r.key.as_deref() == Some("totp") {
        return Err(
            "a TOTP code changes every 30 s; reference it as {{secret:…#totp}} where it is used"
                .into(),
        );
    }
    let (value, version) = cx
        .bounded(r#ref, b.backend(), b.get(&r))
        .await
        .map_err(|e| err_text(r#ref, &e))?;
    let note = note.unwrap_or_else(|| format!("from {ref} {}", version.id));
    let info = cx
        .vault
        .set_expiring(name, &value, Some(&note), r#ref, expires_ms)
        .await?;
    drop(value);
    Ok((info, version))
}

/// `secret_external_create`: put a local secret's current value into the manager as a new entry.
#[derive(Debug, Deserialize, JsonSchema)]
pub struct ExtCreateArgs {
    /// The connection (`prod`, `personal`).
    pub connection: String,
    /// Local NAME whose current value is written. Never a literal value.
    pub from: String,
    /// Where: name (AWS secret name / Bitwarden item name) and the backend's options.
    pub destination: Destination,
}

/// The `secret_external_create` tool.
pub struct CreateTool(pub ExtCx);

/// The [`Location`] of a new entry at `d` on `b`, for a value stored under local `from`.
pub fn location(
    backend: &str,
    d: &Destination,
    from: &str,
    scope: WriteScope,
) -> Result<Location, String> {
    match backend {
        "aws" => Ok(Location::Aws {
            name: d.name.clone(),
            kms_key_id: d.kms_key_id.clone(),
            description: None,
            json_key: matches!(d.format, Some(crate::connections::AwsFormat::Json))
                .then(|| from.to_owned()),
        }),
        "bw" => {
            if (d.org.is_some() || d.collection.is_some()) && scope == WriteScope::Personal {
                return Err(format!(
                    "writes to organisation items are off for bw/{c} (Settings › Secrets › {c} › Write scope)",
                    c = d.connection
                ));
            }
            let field = match d
                .fields
                .as_ref()
                .and_then(|f| f.get(from))
                .map(String::as_str)
            {
                None | Some("password") => BwField::Password,
                Some("notes") => BwField::Notes,
                Some("totp") => BwField::Totp,
                Some(other) => match other.strip_prefix("field:") {
                    Some(name) => BwField::Custom {
                        name: name.to_owned(),
                        hidden: true,
                    },
                    None => return Err(format!("{other}: password | notes | totp | field:<name>")),
                },
            };
            // Names as the agent gave them; the backend resolves them to ids (§7.3).
            Ok(Location::Bw {
                org: d.org.clone().map(crate::external::OrgId),
                collections: d
                    .collection
                    .clone()
                    .map(crate::external::CollectionId)
                    .into_iter()
                    .collect(),
                folder: d.folder.clone().map(crate::external::FolderId),
                kind: d.item.unwrap_or(BwKind::Login),
                item_name: d.name.clone(),
                field,
                username: None,
                uri: d.uri.clone(),
            })
        }
        other => Err(format!("unknown secret backend {other}")),
    }
}

#[async_trait::async_trait]
impl Tool for CreateTool {
    const NAME: &'static str = "secret_external_create";
    const RISK: Risk = Risk::Mutating;
    const CONSUMES_SECRETS: bool = false;
    type Args = ExtCreateArgs;

    fn description(&self) -> String {
        "Store a local vault secret's current value as a new entry in an external manager. `from` \
         is the local NAME (put a fresh value there first with secret_capture or secret_request); \
         `destination.name` is the AWS secret name or the Bitwarden item name. Fails if the entry \
         exists (use secret_external_update). Returns the reference to use."
            .into()
    }

    async fn call(&self, a: ExtCreateArgs, _cx: ToolCx) -> anyhow::Result<ToolOutput> {
        let cx = &self.0;
        let Some(value) = cx.vault.get(&a.from, 0) else {
            return Ok(ToolOutput::err(format!("unknown secret {}", a.from)));
        };
        let b = match cx.conn(&a.connection, "the agent creates an entry").await {
            Ok(b) => b,
            Err(e) => return Ok(ToolOutput::err(e)),
        };
        let scope = match cx.settings(&a.connection).await {
            Ok(s) => s.write_scope,
            Err(e) => return Ok(ToolOutput::err(e)),
        };
        let mut d = a.destination;
        d.connection = a.connection.clone();
        let loc = match location(b.backend(), &d, &a.from, scope) {
            Ok(l) => l,
            Err(e) => return Ok(ToolOutput::err(e)),
        };
        let what = format!("{}/{}/{}", b.backend(), a.connection, d.name);
        match cx
            .bounded(&what, b.backend(), b.create(&loc, Value::Plain(&value)))
            .await
        {
            Ok((r, v)) => {
                let t = ref_text(b.backend(), &r);
                Ok(ToolOutput::ok(format!(
                    "created {t} (version {}); use {{{{secret:{t}}}}}",
                    version_line(&v)
                )))
            }
            Err(e) => Ok(ToolOutput::err(err_text(&what, &e))),
        }
    }
}

/// `secret_external_update`: replace an external value with a local secret's current value.
#[derive(Debug, Deserialize, JsonSchema)]
pub struct ExtUpdateArgs {
    /// The reference, with `#key` (AWS JSON: that key only) / `#field`.
    pub r#ref: String,
    /// Local NAME whose current value is written.
    pub from: String,
    /// The version from describe / list (AWS VersionId / Bitwarden revision date). No blind writes.
    pub expected: String,
}

/// The `secret_external_update` tool.
pub struct UpdateTool(pub ExtCx);

#[async_trait::async_trait]
impl Tool for UpdateTool {
    const NAME: &'static str = "secret_external_update";
    const RISK: Risk = Risk::Destructive;
    const CONSUMES_SECRETS: bool = false;
    type Args = ExtUpdateArgs;

    fn description(&self) -> String {
        "Replace an external value (one JSON key with `#key`, one Bitwarden field with `#field`) \
         with a local vault secret's current value. `expected` is the version from \
         secret_external_describe; a changed entry is refused, never overwritten. AWS keeps the \
         old value as `:-1`; a Bitwarden field edit keeps only the password's history."
            .into()
    }

    async fn call(&self, a: ExtUpdateArgs, _cx: ToolCx) -> anyhow::Result<ToolOutput> {
        let cx = &self.0;
        let Some(value) = cx.vault.get(&a.from, 0) else {
            return Ok(ToolOutput::err(format!("unknown secret {}", a.from)));
        };
        let (b, r) = match cx.open(&a.r#ref, "the agent updates an entry").await {
            Ok(x) => x,
            Err(e) => return Ok(ToolOutput::err(e)),
        };
        let expected = ExtVersion {
            id: a.expected,
            stages: Vec::new(),
            changed: None,
        };
        match cx
            .bounded(
                &a.r#ref,
                b.backend(),
                b.update(&r, Value::Plain(&value), Some(&expected)),
            )
            .await
        {
            Ok(v) => {
                let mut line = format!("updated {}: version {}", a.r#ref, version_line(&v));
                if b.backend() == "aws" {
                    let base = ref_text(
                        "aws",
                        &ExtRef {
                            key: None,
                            ..r.clone()
                        },
                    );
                    line.push_str(&format!("; the previous one is {{{{secret:{base}:-1}}}}"));
                }
                Ok(ToolOutput::ok(line))
            }
            Err(e) => Ok(ToolOutput::err(err_text(&a.r#ref, &e))),
        }
    }
}

/// `secret_external_delete`: delete an external entry. Recoverable by default.
#[derive(Debug, Deserialize, JsonSchema)]
pub struct ExtDeleteArgs {
    /// `aws/prod/app/old-token` or `bw/personal/<item-id>`; no `#key` (an entry goes whole).
    pub r#ref: String,
    /// The version from describe / list. No blind deletes.
    pub expected: String,
    /// AWS only: days before AWS purges it, 7..=30. Default: the connection's setting (30).
    #[serde(default)]
    pub recovery_days: Option<u8>,
    /// Skip recovery. Refused unless the connection allows permanent delete.
    #[serde(default)]
    pub permanent: bool,
}

/// The `secret_external_delete` tool.
pub struct DeleteTool(pub ExtCx);

#[async_trait::async_trait]
impl Tool for DeleteTool {
    const NAME: &'static str = "secret_external_delete";
    const RISK: Risk = Risk::Destructive;
    const CONSUMES_SECRETS: bool = false;
    type Args = ExtDeleteArgs;

    fn description(&self) -> String {
        "Delete an external entry: AWS schedules deletion after a recovery window (7 to 30 days), \
         Bitwarden moves it to the trash; the user can restore it from the Secrets page. \
         `permanent: true` skips that and works only if the user allowed it for the connection. \
         `expected` is the version from secret_external_describe."
            .into()
    }

    async fn call(&self, a: ExtDeleteArgs, _cx: ToolCx) -> anyhow::Result<ToolOutput> {
        let cx = &self.0;
        let (b, r) = match cx.open(&a.r#ref, "the agent deletes an entry").await {
            Ok(x) => x,
            Err(e) => return Ok(ToolOutput::err(e)),
        };
        if r.key.is_some() {
            return Ok(ToolOutput::err(
                "an entry is deleted whole: pass the reference without #key",
            ));
        }
        let settings = match cx.settings(&r.conn).await {
            Ok(s) => s,
            Err(e) => return Ok(ToolOutput::err(e)),
        };
        let name = format!("{}/{}", b.backend(), r.conn);
        if a.permanent && !settings.allow_permanent_delete {
            return Ok(ToolOutput::err(format!(
                "permanent delete is off for {name} (Settings › Secrets › {} › Allow permanent delete)",
                r.conn
            )));
        }
        if let Some(d) = a.recovery_days
            && !(7..=30).contains(&d)
        {
            return Ok(ToolOutput::err("recovery_days: 7 to 30"));
        }
        let mode = if a.permanent {
            DeleteMode::Permanent
        } else {
            DeleteMode::Recoverable {
                days: Some(a.recovery_days.unwrap_or(settings.delete_recovery_days)),
            }
        };
        let expected = ExtVersion {
            id: a.expected,
            stages: Vec::new(),
            changed: None,
        };
        match cx
            .bounded(&a.r#ref, b.backend(), b.delete(&r, mode, Some(&expected)))
            .await
        {
            Ok(out) => {
                let _ = cx
                    .actor
                    .tell(crate::connections::Audit(format!(
                        "agent deleted {} ({mode:?})",
                        a.r#ref
                    )))
                    .await;
                Ok(ToolOutput::ok(match out.recoverable_until {
                    Some(t) => format!(
                        "deleted {}; recoverable until {t} (restore from Secrets › {})",
                        a.r#ref, r.conn
                    ),
                    None => format!("deleted {} permanently", a.r#ref),
                }))
            }
            Err(e) => Ok(ToolOutput::err(err_text(&a.r#ref, &e))),
        }
    }
}

/// 221 §7.3: `d` resolved on this engine for the request card: the connection exists, its backend,
/// summary and status, the replaced entry's current revision. Checks what can be checked before
/// the user types anything (the connection, the AWS `format`, the write scope).
pub(crate) async fn destination_view(
    actor: &ActorRef<SecretsActor>,
    d: Destination,
    fields: &[crate::SecretRequestField],
) -> Result<crate::connections::DestinationView, String> {
    let info = actor
        .ask(crate::connections::ConnInfoOf(d.connection.clone()))
        .await
        .map_err(|e| e.to_string())?;
    let externals = actor
        .ask(crate::connections::GetExternals)
        .await
        .map_err(|e| e.to_string())?;
    let b = externals.by_conn(&d.connection).ok_or_else(|| {
        format!(
            "connection {} is not available on this engine (no {} backend in this build)",
            d.connection, info.backend
        )
    })?;
    if b.backend() == "aws"
        && fields.len() > 1
        && !matches!(d.format, Some(crate::connections::AwsFormat::Json))
    {
        return Err("several fields into one AWS secret need format: json".into());
    }
    if d.replace.is_none() {
        let first = fields.first().map(|f| f.name.as_str()).unwrap_or_default();
        location(b.backend(), &d, first, info.settings.write_scope)?;
    }
    let current_rev = match &d.replace {
        Some(r) => {
            let (_, er) =
                parse_bare(r).ok_or_else(|| format!("replace: {r:?} is not a reference"))?;
            if er.conn != d.connection {
                return Err(format!(
                    "replace: {r} is not on connection {}",
                    d.connection
                ));
            }
            if matches!(b.status(), ConnStatus::Ready) {
                let t = actor
                    .ask(GetExternalSettings)
                    .await
                    .map(|s| Duration::from_secs(s.timeout_secs))
                    .unwrap_or(Duration::from_secs(20));
                match tokio::time::timeout(t, b.describe(&er)).await {
                    Ok(Ok(e)) => e.versions.first().map(|v| v.id.clone()),
                    Ok(Err(e)) => return Err(err_text(r, &e)),
                    Err(_) => None,
                }
            } else {
                None
            }
        }
        None => None,
    };
    Ok(crate::connections::DestinationView {
        backend: info.backend,
        summary: info.summary,
        status: info.status,
        current_rev,
        destination: d,
    })
}

#[cfg(test)]
mod tests {
    use std::collections::HashMap;
    use std::sync::Mutex;

    use kameo::actor::Spawn;
    use runtime::{Runtime, SessionId};
    use tools::{InspectTool, PolicyAction, SessionResources};
    use zeroize::Zeroizing;

    use super::*;
    use crate::connections::{
        AddConnection, AwsFormat, BackendFactory, ConnConfig, ConnStore, DestinationView,
        GetExternals, RegisterBackendFactory, SeedConnections,
    };
    use crate::external::{DeleteOutcome, ExtEntry, ListPage};
    use crate::{Answer, AnswerTo, KeyStore, Request, RequestId, RequestTo, SecretRequestField};

    /// An in-memory AWS-like manager: `create` stores, `get` reads, versions count up.
    #[derive(Default)]
    struct Mem {
        conn: String,
        rows: Mutex<HashMap<String, (String, u32)>>,
    }

    #[async_trait::async_trait]
    impl ExternalBackend for Mem {
        fn backend(&self) -> &'static str {
            "aws"
        }
        fn conn(&self) -> &str {
            &self.conn
        }
        fn status(&self) -> ConnStatus {
            ConnStatus::Ready
        }
        async fn probe(&self) -> Result<ConnStatus, ExtError> {
            Ok(ConnStatus::Ready)
        }
        async fn list(&self, _: ListQuery) -> Result<ListPage, ExtError> {
            let rows = self.rows.lock().unwrap();
            let mut entries: Vec<ExtEntry> = rows
                .iter()
                .map(|(id, (_, v))| ExtEntry {
                    r#ref: format!("aws/{}/{id}", self.conn),
                    name: id.clone(),
                    kind: "string".into(),
                    keys: Vec::new(),
                    location: "eu-west-1".into(),
                    versions: vec![version(*v)],
                    recoverable_until: None,
                })
                .collect();
            entries.sort_by(|a, b| a.name.cmp(&b.name));
            Ok(ListPage {
                entries,
                next: None,
            })
        }
        async fn describe(&self, r: &ExtRef) -> Result<ExtEntry, ExtError> {
            let rows = self.rows.lock().unwrap();
            let (_, v) = rows.get(&r.id).ok_or(ExtError::NotFound)?;
            Ok(ExtEntry {
                r#ref: format!("aws/{}/{}", self.conn, r.id),
                name: r.id.clone(),
                kind: "string".into(),
                keys: Vec::new(),
                location: "eu-west-1".into(),
                versions: vec![version(*v)],
                recoverable_until: None,
            })
        }
        async fn get(&self, r: &ExtRef) -> Result<(Zeroizing<String>, ExtVersion), ExtError> {
            let rows = self.rows.lock().unwrap();
            let (value, v) = rows.get(&r.id).ok_or(ExtError::NotFound)?;
            Ok((Zeroizing::new(value.clone()), version(*v)))
        }
        async fn create(
            &self,
            loc: &Location,
            value: Value<'_>,
        ) -> Result<(ExtRef, ExtVersion), ExtError> {
            let Location::Aws { name, .. } = loc else {
                return Err(ExtError::Unsupported("bw".into()));
            };
            let text = match value {
                Value::Plain(s) => s.to_owned(),
                Value::Json(kv) => {
                    serde_json::to_string(&kv.into_iter().collect::<HashMap<_, _>>()).unwrap()
                }
            };
            let mut rows = self.rows.lock().unwrap();
            if rows.contains_key(name) {
                return Err(ExtError::Exists);
            }
            rows.insert(name.clone(), (text, 1));
            let r = ExtRef {
                conn: self.conn.clone(),
                id: name.clone(),
                key: None,
                back: 0,
            };
            Ok((r, version(1)))
        }
        async fn update(
            &self,
            r: &ExtRef,
            value: Value<'_>,
            expected: Option<&ExtVersion>,
        ) -> Result<ExtVersion, ExtError> {
            let Value::Plain(s) = value else {
                return Err(ExtError::Unsupported("json".into()));
            };
            let mut rows = self.rows.lock().unwrap();
            let row = rows.get_mut(&r.id).ok_or(ExtError::NotFound)?;
            if let Some(e) = expected
                && e.id != row.1.to_string()
            {
                return Err(ExtError::Conflict {
                    current: version(row.1),
                });
            }
            *row = (s.to_owned(), row.1 + 1);
            Ok(version(row.1))
        }
        async fn delete(
            &self,
            r: &ExtRef,
            mode: DeleteMode,
            _: Option<&ExtVersion>,
        ) -> Result<DeleteOutcome, ExtError> {
            self.rows
                .lock()
                .unwrap()
                .remove(&r.id)
                .ok_or(ExtError::NotFound)?;
            Ok(DeleteOutcome {
                recoverable_until: matches!(mode, DeleteMode::Recoverable { .. })
                    .then(|| "2026-12-25T00:00:00Z".to_owned()),
            })
        }
        async fn restore(&self, _: &ExtRef) -> Result<ExtVersion, ExtError> {
            Err(ExtError::Unsupported("restore".into()))
        }
        async fn lock(&self) {}
    }

    fn version(v: u32) -> ExtVersion {
        ExtVersion {
            id: v.to_string(),
            stages: vec!["AWSCURRENT".into()],
            changed: None,
        }
    }

    struct MemFactory(Arc<Mem>);

    impl BackendFactory for MemFactory {
        fn backend(&self) -> &'static str {
            "aws"
        }
        fn build(
            &self,
            _: &str,
            _: &ConnConfig,
            _: ConnStore,
        ) -> Result<Arc<dyn ExternalBackend>, String> {
            Ok(self.0.clone())
        }
    }

    fn cx() -> ToolCx {
        ToolCx {
            session: SessionId::new(),
            call_id: "1".into(),
            quiet: false,
            resources: SessionResources::durable_arc(std::path::Path::new("/tmp")),
            cancel: Default::default(),
            promote_after: None,
        }
    }

    struct Env {
        actor: ActorRef<SecretsActor>,
        reg: ActorRef<ToolRegistry>,
        ext: ExtCx,
        mem: Arc<Mem>,
    }

    /// Registry (approvals off, every policy allow) + secrets actor with an AWS connection `prod`
    /// served by [`Mem`].
    async fn env(dir: &std::path::Path) -> Env {
        let rt = Runtime::new(
            figment::Figment::from(figment::providers::Serialized::defaults(
                serde_json::json!({
                    "tools": {"approvals": false, "policy": {"read_only": "allow", "mutating": "allow", "destructive": "allow"}}
                }),
            )),
            dir.to_path_buf(),
        );
        let reg = ::tools::registry::start(&rt).await.unwrap();
        let vault = Arc::new(Vault::open(dir, KeyStore::File).await.unwrap());
        let actor = SecretsActor::spawn(
            SecretsActor::new(vault.clone(), rt, Duration::from_secs(5)).with_config(
                &crate::SecretsConfig::default(),
                Some(dir.join("config.yaml")),
            ),
        );
        actor.ask(SeedConnections).await.unwrap();
        let mem = Arc::new(Mem {
            conn: "prod".into(),
            ..Mem::default()
        });
        actor
            .ask(RegisterBackendFactory(Arc::new(MemFactory(mem.clone()))))
            .await
            .unwrap();
        actor
            .ask(AddConnection {
                conn: "prod".into(),
                cfg: ConnConfig::Aws {
                    profile: None,
                    region: "eu-west-1".into(),
                    kms_key_id: None,
                    prefix: None,
                },
            })
            .await
            .unwrap();
        let externals = actor.ask(GetExternals).await.unwrap();
        Env {
            ext: ExtCx {
                actor: actor.clone(),
                vault,
                externals,
            },
            actor,
            reg,
            mem,
        }
    }

    fn destination(name: &str) -> Destination {
        Destination {
            connection: "prod".into(),
            name: name.into(),
            replace: None,
            keep_local: false,
            kms_key_id: None,
            format: None,
            org: None,
            collection: None,
            folder: None,
            item: None,
            fields: None,
            uri: None,
        }
    }

    /// 221 §13 B6: `secret_external_list{}` touches no manager and asks nothing;
    /// `{connection: prod}` is a read of `prod`, which its seeded entry makes always ask (with
    /// approvals off and every policy allow); removing the entry returns it to the policy.
    #[test]
    fn listing_connections_is_free_and_listing_a_manager_asks() {
        crate::connections::tests::serial(async {
            let dir = tempfile::tempdir().unwrap();
            let e = env(dir.path()).await;
            ::tools::secrets::install(Arc::new(crate::external::Hook::new(
                e.ext.vault.clone(),
                e.ext.externals.clone(),
                Duration::from_secs(5),
            )));
            let session = SessionId::new();
            let action = |args| {
                let reg = e.reg.clone();
                async move {
                    reg.ask(InspectTool {
                        session,
                        name: "secret_external_list".into(),
                        args: Some(args),
                    })
                    .await
                    .unwrap()
                    .action
                }
            };
            assert_eq!(action(serde_json::json!({})).await, PolicyAction::Allow);
            assert_eq!(
                action(serde_json::json!({"connection": "prod"})).await,
                PolicyAction::Ask
            );
            e.reg
                .ask(::tools::SetAlwaysAsk {
                    session: None,
                    tool: "secret:aws/prod:read".into(),
                    on: false,
                })
                .await
                .unwrap();
            assert_eq!(
                action(serde_json::json!({"connection": "prod"})).await,
                PolicyAction::Allow
            );
            let out = ListTool(e.ext.clone())
                .call(
                    ExtListArgs {
                        connection: None,
                        query: None,
                        cursor: None,
                    },
                    cx(),
                )
                .await
                .unwrap();
            assert_eq!(out.content, "prod · aws · default · eu-west-1 · ready");
        });
    }

    /// 221 §13 B6: capture → create → a placeholder use expands to the right value, and no
    /// receipt carries it; update with a stale `expected` is refused; delete is recoverable
    /// and permanent delete is off by default.
    #[test]
    fn create_use_update_delete_through_the_tools() {
        crate::connections::tests::serial(async {
            let dir = tempfile::tempdir().unwrap();
            let e = env(dir.path()).await;
            let token = "ci-token-value-0123456789";
            e.ext
                .vault
                .set("CI_TOKEN", token, None, "secret_capture")
                .await
                .unwrap();
            let out = CreateTool(e.ext.clone())
                .call(
                    ExtCreateArgs {
                        connection: "prod".into(),
                        from: "CI_TOKEN".into(),
                        destination: destination("app/ci/token"),
                    },
                    cx(),
                )
                .await
                .unwrap();
            assert_eq!(
                out.content,
                "created aws/prod/app/ci/token (version 1 AWSCURRENT); use {{secret:aws/prod/app/ci/token}}"
            );
            let again = CreateTool(e.ext.clone())
                .call(
                    ExtCreateArgs {
                        connection: "prod".into(),
                        from: "CI_TOKEN".into(),
                        destination: destination("app/ci/token"),
                    },
                    cx(),
                )
                .await
                .unwrap();
            assert_eq!(
                again.content,
                "aws/prod/app/ci/token exists; secret_external_update replaces it"
            );

            use ::tools::secrets::Secrets as _;
            let hook = crate::external::Hook::new(
                e.ext.vault.clone(),
                e.ext.externals.clone(),
                Duration::from_secs(5),
            );
            let mut args = serde_json::json!({"command": "printf %s '{{secret:aws/prod/app/ci/token}}' | wc -c"});
            let f = hook
                .resolve_external(
                    &args,
                    ::tools::secrets::ResolveCx {
                        session: SessionId::new(),
                        call_id: "2".into(),
                    },
                )
                .await
                .unwrap();
            hook.expand_with(&mut args, &f).unwrap();
            assert_eq!(args["command"], format!("printf %s '{token}' | wc -c"));
            assert_eq!(
                hook.redact(&format!("out {token}")),
                "out {{secret:CI_TOKEN}}"
            );

            e.ext
                .vault
                .set("CI_TOKEN", "ci-token-value-rotated-1", None, "user")
                .await
                .unwrap();
            let stale = UpdateTool(e.ext.clone())
                .call(
                    ExtUpdateArgs {
                        r#ref: "aws/prod/app/ci/token".into(),
                        from: "CI_TOKEN".into(),
                        expected: "7".into(),
                    },
                    cx(),
                )
                .await
                .unwrap();
            assert!(
                stale.is_error && stale.content.contains("changed since"),
                "{stale:?}"
            );
            let ok = UpdateTool(e.ext.clone())
                .call(
                    ExtUpdateArgs {
                        r#ref: "aws/prod/app/ci/token".into(),
                        from: "CI_TOKEN".into(),
                        expected: "1".into(),
                    },
                    cx(),
                )
                .await
                .unwrap();
            assert!(!ok.is_error, "{ok:?}");
            assert!(!ok.content.contains("rotated"));

            let perm = DeleteTool(e.ext.clone())
                .call(
                    ExtDeleteArgs {
                        r#ref: "aws/prod/app/ci/token".into(),
                        expected: "2".into(),
                        recovery_days: None,
                        permanent: true,
                    },
                    cx(),
                )
                .await
                .unwrap();
            assert_eq!(
                perm.content,
                "permanent delete is off for aws/prod (Settings › Secrets › prod › Allow permanent delete)"
            );
            let del = DeleteTool(e.ext.clone())
                .call(
                    ExtDeleteArgs {
                        r#ref: "aws/prod/app/ci/token".into(),
                        expected: "2".into(),
                        recovery_days: None,
                        permanent: false,
                    },
                    cx(),
                )
                .await
                .unwrap();
            assert_eq!(
                del.content,
                "deleted aws/prod/app/ci/token; recoverable until 2026-12-25T00:00:00Z (restore from Secrets › prod)"
            );
            assert!(e.mem.rows.lock().unwrap().is_empty());

            e.mem
                .rows
                .lock()
                .unwrap()
                .insert("db".into(), ("db-password-value-1".into(), 3));
            let imp = ImportTool(e.ext.clone())
                .call(
                    ExtImportArgs {
                        r#ref: "aws/prod/db".into(),
                        name: "DB".into(),
                        note: None,
                        replace: false,
                        expires: None,
                    },
                    cx(),
                )
                .await
                .unwrap();
            assert_eq!(
                imp.content,
                "stored DB v1 from aws/prod/db (3 AWSCURRENT); {{secret:DB}}"
            );
            assert_eq!(
                e.ext.vault.get("DB", 0).as_deref().map(String::as_str),
                Some("db-password-value-1")
            );
        });
    }

    /// 221 §7.3: a `secret_request` with a destination stores the typed value in the manager,
    /// not the local vault; the row and the card carry the destination, the receipt the reference.
    #[test]
    fn request_with_destination_stores_in_the_manager() {
        crate::connections::tests::serial(async {
            let dir = tempfile::tempdir().unwrap();
            let e = env(dir.path()).await;
            let fields = vec![SecretRequestField {
                name: "NPM_TOKEN".into(),
                label: "npm token".into(),
                masked: true,
                current: None,
                note: None,
            }];
            let view: DestinationView = destination_view(&e.actor, destination("ci/npm"), &fields)
                .await
                .unwrap();
            assert_eq!(view.summary, "default · eu-west-1");
            let id = RequestId(uuid::Uuid::now_v7());
            let asking = tokio::spawn({
                let actor = e.actor.clone();
                let fields = fields.clone();
                async move {
                    actor
                        .ask(RequestTo {
                            request: Request {
                                session: SessionId::new(),
                                title: None,
                                fields,
                                id: Some(id),
                                expires: None,
                            },
                            destination: Some(view),
                        })
                        .await
                }
            });
            let row = loop {
                if let Some(r) = crate::requests::get(e.ext.vault.pool(), id).await.unwrap() {
                    break r;
                }
                tokio::time::sleep(Duration::from_millis(10)).await;
            };
            assert_eq!(row.destination.as_ref().unwrap().destination.name, "ci/npm");
            let mut format_json = destination("ci/npm");
            format_json.format = Some(AwsFormat::String);
            let fresh = e
                .actor
                .ask(AnswerTo {
                    answer: Answer {
                        id,
                        value: Some("npm_secret_value_123".into()),
                        values: Default::default(),
                        expires_ms: None,
                    },
                    destination: Some(format_json),
                })
                .await
                .unwrap();
            assert!(fresh);
            let out = asking.await.unwrap().unwrap();
            assert_eq!(
                out.external,
                Some(Ok(
                    "stored in aws/prod/ci/npm (version 1); use {{secret:aws/prod/ci/npm}}".into()
                ))
            );
            assert!(
                e.ext.vault.get("NPM_TOKEN", 0).is_none(),
                "the manager is the truth"
            );
            assert_eq!(
                e.mem.rows.lock().unwrap()["ci/npm"].0,
                "npm_secret_value_123"
            );
        });
    }
}
