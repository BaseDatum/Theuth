//! The `secret_list`, `secret_request`, `secret_request_form`, `secret_capture`,
//! `secret_restore` and `secret_delete` tools and the plugin's prompt block (design doc 37
//! §3.2, §4; 79 §6.6; 221 D24), registered by `SecretsPlugin::start` once the tool registry
//! is up. None of them returns a value, ever: names, notes, version numbers, dates and byte
//! counts only.

use std::sync::Arc;
use std::time::Duration;

use kameo::actor::ActorRef;
use kameo::error::SendError;
use schemars::JsonSchema;
use serde::Deserialize;
use tools::{
    Invoke, PreviewCx, QuestionKind, QuestionRow, QuestionState, REGISTRY_ACTOR, RegisterPrompt,
    RegisterTool, Risk, SetAlwaysAsk, Tool, ToolCall, ToolCx, ToolOutput, ToolRegistry, WaitKind,
    WaitRow,
};

use crate::vault::{self, MIN_REDACT, SecretInfo, Vault};
use crate::{
    DeleteConfirmed, Outcome, Request, RequestId, SecretRequestField, SecretsActor, expiry,
};

/// The plugin's system-prompt block (37 §4). Static, so it never busts the
/// system cache.
pub const PROMPT: &str = "Named secrets (API keys, tokens, passwords — including the harness's own provider keys) live in an \
encrypted vault you can use but never read. `secret_list()` names them with their versions. Write `{{secret:NAME}}` \
inside any tool argument — a shell command, a header, a URL, file content, an MCP tool's field — and the harness \
swaps in the value when the tool runs; you, the transcript and the user's approval card see only the placeholder, \
and the value is scrubbed from tool output if it echoes back. `{{secret:NAME:-1}}` is the previous version (a token \
you just rotated). In Python keep the placeholder in a plain string, not an f-string (`{{` becomes `{`). Need a \
secret that does not exist? `secret_request` asks the user to type it into a masked field — or several at once \
(username, password, one-time code) on one card with `title` + `fields`; the user may give them an expiry, after \
which they are deleted. `secret_capture()` runs a tool and stores its output (a fresh token from a login command, \
a key from a file) without showing it to you — `replace: true` to update one that exists; `secret_restore()` undoes \
a bad update. `secret_delete` removes a secret and all its versions for good; the user is asked first. Never ask \
the user to paste a secret into the chat, and never try to print one.";

/// Register the six tools and the prompt block on `reg`, and seed `secret_delete`'s
/// always-ask entry once (221 D24). `timeout` is the actor's `request_timeout_secs`, for the
/// `no answer after N s` receipt.
pub async fn register(
    reg: &ActorRef<ToolRegistry>,
    vault: Arc<Vault>,
    actor: ActorRef<SecretsActor>,
    timeout: Duration,
) {
    let _ = reg
        .ask(RegisterTool(Arc::new(SecretListTool(vault.clone()))))
        .await;
    let _ = reg
        .ask(RegisterTool(Arc::new(SecretRequestTool {
            actor: actor.clone(),
            timeout,
        })))
        .await;
    let _ = reg
        .ask(RegisterTool(Arc::new(SecretRequestFormTool {
            actor: actor.clone(),
            timeout,
        })))
        .await;
    let _ = reg
        .ask(RegisterTool(Arc::new(SecretCaptureTool(vault.clone()))))
        .await;
    let _ = reg
        .ask(RegisterTool(Arc::new(SecretRestoreTool(vault.clone()))))
        .await;
    let _ = reg
        .ask(RegisterTool(Arc::new(SecretDeleteTool {
            vault: vault.clone(),
            actor,
        })))
        .await;
    if let Err(e) = seed_delete_ask(reg, &vault).await {
        tracing::warn!("secrets: secret_delete always-ask entry not seeded: {e}");
    }
    let _ = reg
        .ask(RegisterPrompt {
            plugin: "secrets",
            text: PROMPT.to_owned(),
        })
        .await;
}

/// The secret named `name`, if the vault has it.
fn find(vault: &Vault, name: &str) -> Option<SecretInfo> {
    vault.list().into_iter().find(|s| s.name == name)
}

/// `secret_list`: names, notes, versions. Never values.
#[derive(Debug, Deserialize, JsonSchema)]
pub struct ListArgs {}

/// The `secret_list` tool.
pub struct SecretListTool(pub Arc<Vault>);

#[async_trait::async_trait]
impl Tool for SecretListTool {
    const NAME: &'static str = "secret_list";
    const RISK: Risk = Risk::ReadOnly;
    const RETURNS: &'static str = "list[dict]";
    type Args = ListArgs;

    fn description(&self) -> String {
        "List the secrets in the vault: name, current version, how many versions are kept, note and \
         when it was last updated. Never a value. Use a name as `{{secret:NAME}}` in any tool argument."
            .into()
    }

    async fn call(&self, _: ListArgs, _cx: ToolCx) -> anyhow::Result<ToolOutput> {
        let list = self.0.list();
        if list.is_empty() {
            return Ok(ToolOutput::ok(
                "no secrets; the user adds them with /secrets add, Settings › Secrets or `t --secret-set`, \
                 or call secret_request",
            ));
        }
        let now = crate::now_ms();
        let lines: Vec<String> = list.iter().map(|s| list_line(s, now)).collect();
        let mut out = ToolOutput::ok(lines.join("\n"));
        out.structured = serde_json::to_value(&list).ok();
        Ok(out)
    }
}

/// One `secret_list` line: `NAME · vN (K kept) · note · updated · source`, then
/// ` · expires in 40 min` (or the date) when the current version expires.
fn list_line(s: &SecretInfo, now_ms: u64) -> String {
    let note = if s.note.is_empty() {
        String::new()
    } else {
        format!(" · {}", s.note)
    };
    let expires = s
        .expires
        .as_deref()
        .and_then(expiry::from_rfc3339)
        .map(|t| format!(" · {}", expiry::describe(t, now_ms)))
        .unwrap_or_default();
    format!(
        "{} · v{} ({} kept){note} · {} · {}{expires}",
        s.name,
        s.current,
        s.versions.len(),
        s.updated,
        s.source
    )
}

/// One value of a `secret_request` with `fields` (79 §6.6).
#[derive(Debug, Deserialize, JsonSchema)]
pub struct FormField {
    /// Name to store it under (`github.alice.password`; `[A-Za-z0-9_.-]`, no `/`). An existing name gets a new version.
    pub name: String,
    /// Label on the card (`Password`).
    pub label: String,
    /// Masked input; default true. `false` for a username or an e-mail.
    #[serde(default)]
    pub masked: Option<bool>,
    /// What it is for, kept as the secret's note in `secret_list`; default the label.
    #[serde(default)]
    pub note: Option<String>,
}

/// `secret_request`: one value (`name` + `note`) or several on one card (`title` + `fields`).
#[derive(Debug, Default, Deserialize, JsonSchema)]
pub struct RequestArgs {
    /// One value: the name to store it under. An existing name gets a new version; the user is told.
    #[serde(default)]
    pub name: Option<String>,
    /// One value: what it is for, shown to the user (`GitHub personal access token with repo scope`).
    #[serde(default)]
    pub note: Option<String>,
    /// Several values: the card title (`Log in to GitHub as alice`).
    #[serde(default)]
    pub title: Option<String>,
    /// Several values: the fields, in card order, 1 to 16.
    #[serde(default)]
    pub fields: Option<Vec<FormField>>,
    /// Optional suggested expiry the card pre-fills: `30m`, `2h`, `3d`, `2026-12-01` or
    /// `2026-12-01T18:00` (ISO-8601, local time unless it has an offset). The user decides;
    /// once it passes the values are deleted. Omit unless the value is short-lived.
    #[serde(default)]
    pub expires: Option<String>,
    /// 221 §7.3: store the answer(s) in an external manager (a connection from
    /// `secret_external_list`) instead of the local vault; the user may change it on the card.
    #[serde(default)]
    pub destination: Option<crate::connections::Destination>,
}

/// At most this many fields on one card.
pub const MAX_FIELDS: usize = 16;

/// A validated `secret_request`: what [`Request`] carries.
#[derive(Debug, PartialEq, Eq)]
pub struct Ask {
    /// Card title; `None` for one value.
    pub title: Option<String>,
    /// In card order.
    pub fields: Vec<SecretRequestField>,
    /// [`expiry::normalize`]d suggestion.
    pub expires: Option<String>,
}

/// Check `a` against the two shapes — `name` + `note`, or `title` + `fields` (1..=16,
/// valid and unique names) — and the `expires` grammar (a past time is an error).
pub fn parse_request(a: RequestArgs, now_ms: u64) -> Result<Ask, String> {
    let single = a.name.is_some() || a.note.is_some();
    let form = a.title.is_some() || a.fields.is_some();
    let (title, fields) = match (single, form) {
        (true, true) => {
            return Err(
                "pass either name + note (one value) or title + fields (several), not both".into(),
            );
        }
        (false, false) => {
            return Err("pass name + note for one value, or title + fields for several".into());
        }
        (true, false) => {
            let (Some(name), Some(note)) = (a.name, a.note) else {
                return Err("one value needs both name and note".into());
            };
            if !vault::valid_name(&name) {
                return Err("name: [A-Za-z0-9_.-]{1,64}".into());
            }
            let field = SecretRequestField {
                name,
                label: note,
                masked: true,
                current: None,
                note: None,
            };
            (None, vec![field])
        }
        (false, true) => {
            let (Some(title), Some(fields)) = (a.title, a.fields) else {
                return Err("several values need both title and fields".into());
            };
            if fields.is_empty() || fields.len() > MAX_FIELDS {
                return Err(format!("fields: 1 to {MAX_FIELDS}"));
            }
            let mut seen = std::collections::HashSet::new();
            for f in &fields {
                if !vault::valid_name(&f.name) {
                    return Err(format!(
                        "fields[].name {:?}: [A-Za-z0-9_.-]{{1,64}}",
                        f.name
                    ));
                }
                if !seen.insert(f.name.as_str()) {
                    return Err(format!("fields[].name {:?} appears twice", f.name));
                }
            }
            let fields = fields
                .into_iter()
                .map(|f| SecretRequestField {
                    name: f.name,
                    label: f.label,
                    masked: f.masked.unwrap_or(true),
                    current: None,
                    note: f.note.filter(|n| !n.is_empty()),
                })
                .collect();
            (Some(title), fields)
        }
    };
    let expires = a
        .expires
        .filter(|e| !e.trim().is_empty())
        .map(|e| expiry::normalize(&e, now_ms))
        .transpose()?;
    Ok(Ask {
        title,
        fields,
        expires,
    })
}

/// The `secret_request` tool: hands the ask to the actor and reports the receipt.
pub struct SecretRequestTool {
    /// The secrets actor.
    pub actor: ActorRef<SecretsActor>,
    /// How long the actor waits; only for the receipt text.
    pub timeout: Duration,
}

#[async_trait::async_trait]
impl Tool for SecretRequestTool {
    const NAME: &'static str = "secret_request";
    const RISK: Risk = Risk::ReadOnly;
    const DIRECT: bool = true;
    const CONSUMES_SECRETS: bool = false;
    /// D14: with a destination the call writes with the user's credentials.
    const CREDENTIALED: bool = true;
    type Args = RequestArgs;

    fn credentialed(&self, _session: runtime::SessionId, a: &RequestArgs) -> bool {
        a.destination.is_some()
    }

    /// A human wait (145 §0.4).
    fn promotable(&self) -> bool {
        false
    }

    fn description(&self) -> String {
        "Ask the user for secrets on a card in their app and store them in the vault; you never \
         see the values. One value: `name` + `note` (what it is for) — one masked field. Several \
         at once (username, password, one-time code): `title` + `fields: [{name, label, masked?, \
         note?}]` (up to 16; `masked` defaults to true) — one card, one answer. The user may set \
         an expiry on the card, after which the values are deleted; `expires` (`30m`, `2h`, `3d` \
         or an ISO-8601 date / date-time) only pre-fills it. Then use `{{secret:NAME}}` \
         placeholders. Returns `stored a v1, b v2` (and the expiry, if set), `declined` or `no \
         answer`. With `destination` (an external connection) the answer is stored in that \
         manager instead, and the receipt gives the `{{secret:aws/…}}` / `{{secret:bw/…}}` \
         reference. Never ask the user to paste a secret into the chat instead."
            .into()
    }

    async fn call(&self, mut a: RequestArgs, cx: ToolCx) -> anyhow::Result<ToolOutput> {
        let destination = a.destination.take();
        let ask = match parse_request(a, crate::now_ms()) {
            Ok(ask) => ask,
            Err(e) => return Ok(ToolOutput::err(e)),
        };
        let view = match destination {
            None => None,
            Some(d) => {
                match crate::tools_external::destination_view(&self.actor, d, &ask.fields).await {
                    Ok(v) => Some(v),
                    Err(e) => return Ok(ToolOutput::err(e)),
                }
            }
        };
        Ok(run(&self.actor, self.timeout, &cx, ask, view).await)
    }
}

/// Raise the card and turn the [`Outcome`] into the result line.
async fn run(
    actor: &ActorRef<SecretsActor>,
    timeout: Duration,
    cx: &ToolCx,
    ask: Ask,
    destination: Option<crate::connections::DestinationView>,
) -> ToolOutput {
    let single = ask.title.is_none() && ask.fields.len() == 1;
    let names = ask
        .fields
        .iter()
        .map(|f| f.name.as_str())
        .collect::<Vec<_>>()
        .join(", ");
    match request(
        actor,
        timeout,
        cx,
        ask.title,
        ask.fields,
        ask.expires,
        destination,
    )
    .await
    {
        Ok(out) => match out.external.clone() {
            Some(Ok(line)) => ToolOutput::ok(line),
            Some(Err(e)) => ToolOutput::err(e),
            None => receipt(&out, single, timeout, &names),
        },
        Err(e) => ToolOutput::err(e),
    }
}

/// `stored a v1, b v2` (one value also names its previous version's placeholder),
/// `; expires <local time>` when the user set one; else `declined` / `no answer` /
/// 207's "proceed without". `names` = the requested field names joined by ", ".
fn receipt(out: &Outcome, single: bool, timeout: Duration, names: &str) -> ToolOutput {
    if out.stored.is_empty() {
        return not_stored(out, timeout, names);
    }
    let list: Vec<String> = out
        .stored
        .iter()
        .map(|(n, v)| format!("{n} v{v}"))
        .collect();
    let mut line = format!("stored {}", list.join(", "));
    if single
        && let [(name, v)] = &out.stored[..]
        && *v > 1
    {
        line.push_str(&format!(" (v{} is {{{{secret:{name}:-1}}}})", v - 1));
    }
    if let Some(t) = out.expires_ms {
        line.push_str(&format!("; expires {}", expiry::local_time(t)));
    }
    ToolOutput::ok(line)
}

/// The receipt when nothing was stored; `names` = the requested field names joined by ", ".
fn not_stored(out: &Outcome, timeout: Duration, names: &str) -> ToolOutput {
    if out.waived {
        ToolOutput::ok(format!(
            "no secret is coming: the user left this to you — proceed without {names}, and say in your reply what you could not do."
        ))
    } else if out.timed_out {
        ToolOutput::err(format!("no answer after {} s", timeout.as_secs()))
    } else {
        ToolOutput::err("declined")
    }
}

/// Ask the actor and journal the `questions` row around it (109 §6.1): a
/// fresh `secret` row with a tool-minted id, or — for a resumed call whose
/// row [`tools::registry::republish`] left for it — the stored row's id and
/// fields, re-armed on the same `secret_requests` row with its remaining
/// deadline (§6.2). The row ends `answered` (`{outcome}`) or `timed_out`.
async fn request(
    actor: &ActorRef<SecretsActor>,
    timeout: Duration,
    cx: &ToolCx,
    title: Option<String>,
    fields: Vec<SecretRequestField>,
    expires: Option<String>,
    destination: Option<crate::connections::DestinationView>,
) -> Result<Outcome, String> {
    let journal = tools::registry::journal();
    let (row, title, fields, expires) = match tools::registry::take_republish(
        cx.session,
        &cx.call_id,
    ) {
        Some(row) if row.state == QuestionState::Pending => {
            let title = row
                .payload
                .get("title")
                .and_then(|t| serde_json::from_value(t.clone()).ok())
                .unwrap_or(title);
            let fields = row
                .payload
                .get("fields")
                .and_then(|f| serde_json::from_value(f.clone()).ok())
                .unwrap_or(fields);
            let expires = row
                .payload
                .get("expires")
                .and_then(|e| serde_json::from_value(e.clone()).ok())
                .unwrap_or(expires);
            (row, title, fields, expires)
        }
        Some(row) if row.state == QuestionState::Answered => {
            // The answer landed before the kill; the `secret_requests` row
            // already holds it and the actor resolves at once.
            (row, title, fields, expires)
        }
        Some(row) => {
            return Ok(Outcome {
                stored: Vec::new(),
                timed_out: row.state == QuestionState::TimedOut,
                waived: false,
                expires_ms: None,
                external: None,
            });
        }
        None => {
            let id = RequestId(uuid::Uuid::now_v7());
            let asked_at = chrono::Utc::now();
            let row = QuestionRow {
                id: id.0.to_string(),
                session: cx.session,
                tool_call_id: Some(cx.call_id.clone()),
                kind: QuestionKind::Secret,
                payload: serde_json::json!({ "id": id, "title": title, "fields": fields, "expires": expires }),
                state: QuestionState::Pending,
                asked_at,
                deadline: asked_at + timeout,
                answered_at: None,
                answer: None,
                answered_by: None,
            };
            (row, title, fields, expires)
        }
    };
    let id = RequestId(
        uuid::Uuid::parse_str(&row.id)
            .map_err(|e| format!("secret request id {:?}: {e}", row.id))?,
    );
    if row.state == QuestionState::Pending {
        journal.question(row.clone()).await;
        journal
            .wait(
                Some(WaitRow {
                    session: cx.session,
                    tool_call_id: cx.call_id.clone(),
                    kind: WaitKind::Question,
                    target: row.id.clone(),
                    deadline: Some(row.deadline),
                    payload: serde_json::json!({}),
                }),
                cx.session,
                &cx.call_id,
            )
            .await;
    }
    let out = actor
        .ask(crate::RequestTo {
            request: Request {
                session: cx.session,
                title,
                fields,
                id: Some(id),
                expires,
            },
            destination,
        })
        .await;
    if row.state == QuestionState::Pending {
        let mut row = row;
        row.answered_at = Some(chrono::Utc::now());
        match &out {
            Ok(o) if o.timed_out => row.state = QuestionState::TimedOut,
            Ok(o) => {
                row.state = QuestionState::Answered;
                let outcome = if o.waived {
                    crate::PROCEED_WITHOUT
                } else if o.stored.is_empty() {
                    "declined"
                } else {
                    "stored"
                };
                row.answer = Some(serde_json::json!({ "outcome": outcome }));
            }
            // The actor refused or died: the question was never raised.
            Err(_) => row.state = QuestionState::Superseded,
        }
        journal.wait(None, cx.session, &cx.call_id).await;
        journal.question(row).await;
    }
    out.map_err(|e| match e {
        SendError::HandlerError(e) => e,
        e => format!("secrets actor: {e}"),
    })
}

/// `secret_request_form`: the older name for `secret_request` with `title` + `fields`.
#[derive(Debug, Deserialize, JsonSchema)]
pub struct FormArgs {
    /// Card title (`Log in to GitHub as alice`).
    pub title: String,
    /// The fields, in card order, 1 to 16.
    pub fields: Vec<FormField>,
    /// Optional suggested expiry, as `secret_request`'s.
    #[serde(default)]
    pub expires: Option<String>,
}

/// `secret_request_form`, kept for older prompts and transcripts: `secret_request`'s
/// `title` + `fields` shape under another name, the same code path.
pub struct SecretRequestFormTool {
    /// The secrets actor.
    pub actor: ActorRef<SecretsActor>,
    /// How long the actor waits; only for the receipt text.
    pub timeout: Duration,
}

#[async_trait::async_trait]
impl Tool for SecretRequestFormTool {
    const NAME: &'static str = "secret_request_form";
    const RISK: Risk = Risk::ReadOnly;
    const DIRECT: bool = true;
    const CONSUMES_SECRETS: bool = false;
    type Args = FormArgs;

    /// A human wait (145 §0.4).
    fn promotable(&self) -> bool {
        false
    }

    fn description(&self) -> String {
        "Older name for `secret_request` with `title` + `fields` — use `secret_request` instead; \
         same card, same result."
            .into()
    }

    async fn call(&self, a: FormArgs, cx: ToolCx) -> anyhow::Result<ToolOutput> {
        let args = RequestArgs {
            title: Some(a.title),
            fields: Some(a.fields),
            expires: a.expires,
            ..RequestArgs::default()
        };
        let ask = match parse_request(args, crate::now_ms()) {
            Ok(ask) => ask,
            Err(e) => return Ok(ToolOutput::err(e)),
        };
        Ok(run(&self.actor, self.timeout, &cx, ask, None).await)
    }
}

/// `secret_capture`: run one tool and store its output as a secret without seeing it.
#[derive(Debug, Deserialize, JsonSchema)]
pub struct CaptureArgs {
    /// Name to store it under.
    pub name: String,
    /// Tool to run (`shell`, `read_file`, `read_url`, an MCP tool).
    pub tool: String,
    /// Its arguments, exactly as you would call it. Placeholders are allowed.
    pub args: serde_json::Value,
    /// Regex; the first capture group is the value. Default: the whole output, trimmed.
    pub pattern: Option<String>,
    /// Shown in `secret_list`. Kept from the previous version when absent.
    pub note: Option<String>,
    /// Required to add a version to a name that exists (guards against a typo landing on the wrong name).
    #[serde(default)]
    pub replace: bool,
}

/// The `secret_capture` tool.
pub struct SecretCaptureTool(pub Arc<Vault>);

/// Shell output ends in `exit code: N`; the value is the text before it.
fn strip_exit_code(content: &str) -> &str {
    content
        .rsplit_once("exit code: ")
        .filter(|(_, code)| code.trim().parse::<i32>().is_ok())
        .map_or(content, |(body, _)| body)
}

#[async_trait::async_trait]
impl Tool for SecretCaptureTool {
    const NAME: &'static str = "secret_capture";
    const RISK: Risk = Risk::ReadOnly;
    type Args = CaptureArgs;

    fn description(&self) -> String {
        "Run one tool and store its output as a secret without it entering the conversation: a fresh \
         token from a login command, a key from a file or a URL. The whole output, trimmed, is the \
         value, or the first capture group of `pattern`. The inner tool takes its own approval. \
         Returns a receipt with a byte count, never the value. `replace: true` is required to add a \
         version to a name that exists; `secret_restore` undoes it."
            .into()
    }

    async fn call(&self, a: CaptureArgs, cx: ToolCx) -> anyhow::Result<ToolOutput> {
        if !vault::valid_name(&a.name) {
            return Ok(ToolOutput::err("name: [A-Za-z0-9_.-]{1,64}"));
        }
        let vault = &self.0;
        if let Some(cur) = find(vault, &a.name).map(|s| s.current)
            && !a.replace
        {
            return Ok(ToolOutput::err(format!(
                "{} exists (v{cur}); pass replace: true to add v{}, or pick another name",
                a.name,
                cur + 1
            )));
        }
        let pattern = match a.pattern.as_deref().map(regex::Regex::new) {
            Some(Err(e)) => return Ok(ToolOutput::err(format!("pattern: {e}"))),
            Some(Ok(re)) => Some(re),
            None => None,
        };
        let Ok(Some(reg)) = ActorRef::<ToolRegistry>::lookup(REGISTRY_ACTOR) else {
            return Ok(ToolOutput::err("tool registry unavailable"));
        };
        let inner = reg
            .ask(Invoke(ToolCall {
                id: format!("{}.capture", cx.call_id),
                name: a.tool.clone(),
                args: a.args,
                session: cx.session,
                quiet: true,
            }))
            .await?;
        // From here on `inner` is never logged or returned: its text is the value.
        if inner.is_error {
            return Ok(ToolOutput::err(format!(
                "{} returned an error ({} bytes); output withheld — run it without secret_capture to \
                 debug, knowing its output then enters the conversation",
                a.tool,
                inner.content.len()
            )));
        }
        let raw = match inner.structured {
            Some(serde_json::Value::String(s)) => s,
            _ => strip_exit_code(&inner.content).to_owned(),
        };
        let value = match &pattern {
            Some(re) => match re.captures(&raw).and_then(|c| c.get(1)) {
                Some(m) => m.as_str(),
                None => {
                    return Ok(ToolOutput::err(format!(
                        "pattern matched nothing in {} bytes",
                        raw.len()
                    )));
                }
            },
            None => raw.trim(),
        };
        if value.is_empty() {
            return Ok(ToolOutput::err("empty output"));
        }
        let info = match vault.set(&a.name, value, a.note.as_deref(), &a.tool).await {
            Ok(i) => i,
            Err(e) => return Ok(ToolOutput::err(e)),
        };
        let mut receipt = format!(
            "stored {} v{} ({} bytes) from {}",
            a.name,
            info.current,
            value.len(),
            a.tool
        );
        if let Some(prev) = info.versions.get(1) {
            receipt.push_str(&format!(
                "; v{} is {{{{secret:{}:-1}}}}, secret_restore undoes",
                prev.v, a.name
            ));
        }
        if value.chars().count() < MIN_REDACT {
            receipt.push_str(&format!(
                "\nvalue shorter than {MIN_REDACT} characters cannot be scrubbed from tool output"
            ));
        }
        Ok(ToolOutput::ok(receipt))
    }
}

/// `secret_restore`: make an older version current again (as a new version; nothing is lost).
#[derive(Debug, Deserialize, JsonSchema)]
pub struct RestoreArgs {
    /// The secret.
    pub name: String,
    /// How many versions back, 1..=4 — the `:-N` of the placeholder that reaches it.
    pub back: u8,
}

/// The `secret_restore` tool.
pub struct SecretRestoreTool(pub Arc<Vault>);

#[async_trait::async_trait]
impl Tool for SecretRestoreTool {
    const NAME: &'static str = "secret_restore";
    const RISK: Risk = Risk::ReadOnly;
    type Args = RestoreArgs;

    fn description(&self) -> String {
        "Make an older version of a secret current again, as a new version (nothing is lost). `back` \
         is how many versions back, 1..=4, the `:-N` of the placeholder that reaches it."
            .into()
    }

    async fn call(&self, a: RestoreArgs, _cx: ToolCx) -> anyhow::Result<ToolOutput> {
        let vault = &self.0;
        let from =
            find(vault, &a.name).and_then(|s| s.versions.get(usize::from(a.back)).map(|v| v.v));
        let info = match vault.restore(&a.name, a.back).await {
            Ok(i) => i,
            Err(e) => return Ok(ToolOutput::err(e)),
        };
        let mut receipt = format!("{} v{}", a.name, info.current);
        if let Some(from) = from {
            receipt.push_str(&format!(" = v{from}"));
        }
        receipt.push_str(" restored");
        if let Some(prev) = info.versions.get(1) {
            receipt.push_str(&format!("; v{} is {{{{secret:{}:-1}}}}", prev.v, a.name));
        }
        Ok(ToolOutput::ok(receipt))
    }
}

/// `meta` stamp: the `secret_delete` always-ask entry was seeded once (221 D24); a user who
/// removes it keeps it removed.
pub const SEEDED_SECRET_DELETE: &str = "seeded/secret_delete";

/// Add `secret_delete` to `tools.always_ask` unless this engine did so before.
pub async fn seed_delete_ask(reg: &ActorRef<ToolRegistry>, vault: &Vault) -> Result<(), String> {
    let pool = vault.pool();
    let seeded = crate::store::meta_get(pool, SEEDED_SECRET_DELETE)
        .await
        .map_err(|e| format!("{e:#}"))?;
    if seeded.is_some() {
        return Ok(());
    }
    reg.ask(SetAlwaysAsk {
        session: None,
        tool: <SecretDeleteTool as Tool>::NAME.to_owned(),
        on: true,
    })
    .await
    .map_err(|e| e.to_string())?;
    crate::store::meta_put(pool, SEEDED_SECRET_DELETE, b"1")
        .await
        .map_err(|e| format!("{e:#}"))
}

/// `secret_delete` (221 §7.6): delete a local vault secret and every kept version.
#[derive(Debug, Deserialize, JsonSchema)]
pub struct DeleteArgs {
    /// The local NAME (`[A-Za-z0-9_.-]{1,64}`). `/` names and Send link secrets are refused.
    pub name: String,
    /// The current version number from `secret_list` (`v7` → 7): guards against deleting the
    /// wrong name.
    pub confirm: u32,
}

/// The `secret_delete` tool: [`DeleteConfirmed`] on the actor, the user's delete path.
pub struct SecretDeleteTool {
    /// Read for the card's preview.
    pub vault: Arc<Vault>,
    /// Deletes and announces.
    pub actor: ActorRef<SecretsActor>,
}

/// Why `name` may not be deleted by the agent, or its current entry.
fn deletable(vault: &Vault, name: &str) -> Result<SecretInfo, String> {
    if name.contains('/') {
        return Err(format!("{name} is not a user secret"));
    }
    let info = find(vault, name).ok_or_else(|| format!("unknown secret {name}"))?;
    if vault.is_send_link(name) {
        return Err(format!(
            "{name} is a Send link; secret_send_revoke revokes it"
        ));
    }
    Ok(info)
}

#[async_trait::async_trait]
impl Tool for SecretDeleteTool {
    const NAME: &'static str = "secret_delete";
    const RISK: Risk = Risk::Destructive;
    const CONSUMES_SECRETS: bool = false;
    const APPROVAL_PREVIEW: bool = true;
    type Args = DeleteArgs;

    fn description(&self) -> String {
        "Delete a local vault secret and every kept version. Cannot be undone: the vault has no \
         trash and no restore. `confirm` is the current version number from `secret_list` (v7 → 7). \
         The user is asked first."
            .into()
    }

    async fn approval_preview(
        &self,
        a: DeleteArgs,
        _cx: PreviewCx,
    ) -> anyhow::Result<Option<serde_json::Map<String, serde_json::Value>>> {
        let info = deletable(&self.vault, &a.name).map_err(anyhow::Error::msg)?;
        let mut m = serde_json::Map::new();
        m.insert("note".into(), info.note.into());
        m.insert("versions".into(), info.versions.len().into());
        m.insert("updated".into(), info.updated.into());
        m.insert(
            "deletes".into(),
            "every version, for good: the vault has no trash and no restore".into(),
        );
        if let Some(source) = info
            .versions
            .first()
            .map(|v| v.source.as_str())
            .filter(|s| *s != "user" && !s.starts_with("secret_") && !s.starts_with("restore"))
        {
            m.insert(
                "harness".into(),
                format!("theuth itself set this key ({source}); deleting it may break that").into(),
            );
        }
        Ok(Some(m))
    }

    async fn call(&self, a: DeleteArgs, _cx: ToolCx) -> anyhow::Result<ToolOutput> {
        if let Err(e) = deletable(&self.vault, &a.name) {
            return Ok(ToolOutput::err(e));
        }
        let n = match self
            .actor
            .ask(DeleteConfirmed {
                name: a.name.clone(),
                confirm: a.confirm,
            })
            .await
        {
            Ok(n) => n,
            Err(SendError::HandlerError(e)) => return Ok(ToolOutput::err(e)),
            Err(e) => return Ok(ToolOutput::err(format!("secret_delete: {e}"))),
        };
        let older = match n.saturating_sub(1) {
            0 => String::new(),
            1 => " and 1 older version".to_owned(),
            k => format!(" and {k} older versions"),
        };
        Ok(ToolOutput::ok(format!(
            "deleted {name} (v{v}{older}); {{{{secret:{name}}}}} no longer resolves",
            name = a.name,
            v = a.confirm,
        )))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn exit_code_line_is_stripped_only_when_present() {
        assert_eq!(strip_exit_code("tok\nexit code: 0"), "tok\n");
        assert_eq!(strip_exit_code("exit code: 1"), "");
        assert_eq!(strip_exit_code("plain text"), "plain text");
        assert_eq!(strip_exit_code("exit code: soon\n"), "exit code: soon\n");
    }

    /// 207: a waived request is ok, names every requested field and says to proceed without them.
    #[test]
    fn waived_request_says_proceed_without() {
        let out = Outcome {
            stored: Vec::new(),
            timed_out: false,
            waived: true,
            expires_ms: None,
            external: None,
        };
        let o = receipt(&out, false, Duration::from_secs(60), "gh.user, gh.pass");
        assert!(!o.is_error);
        assert_eq!(
            o.content,
            "no secret is coming: the user left this to you — proceed without gh.user, gh.pass, and say in your reply what you could not do."
        );
        let declined = Outcome {
            waived: false,
            ..out
        };
        assert!(not_stored(&declined, Duration::from_secs(60), "x").is_error);
    }

    const NOW: u64 = 1_790_000_000_000;

    fn args(v: serde_json::Value) -> RequestArgs {
        serde_json::from_value(v).unwrap()
    }

    #[test]
    fn one_value_shape_is_unchanged() {
        let ask = parse_request(
            args(serde_json::json!({"name": "TOKEN", "note": "GitHub PAT"})),
            NOW,
        )
        .unwrap();
        assert_eq!(ask.title, None);
        assert_eq!(
            ask.fields,
            [SecretRequestField {
                name: "TOKEN".into(),
                label: "GitHub PAT".into(),
                masked: true,
                current: None,
                note: None,
            }]
        );
        assert_eq!(ask.expires, None);
    }

    #[test]
    fn several_values_shape() {
        let ask = parse_request(
            args(serde_json::json!({
                "title": "Log in to GitHub",
                "fields": [
                    {"name": "gh.user", "label": "Username", "masked": false},
                    {"name": "gh.pass", "label": "Password", "note": "alice's GitHub password"},
                ],
                "expires": "2h",
            })),
            NOW,
        )
        .unwrap();
        assert_eq!(ask.title.as_deref(), Some("Log in to GitHub"));
        assert_eq!(
            ask.fields
                .iter()
                .map(|f| (f.name.as_str(), f.masked, f.note.as_deref()))
                .collect::<Vec<_>>(),
            [
                ("gh.user", false, None),
                ("gh.pass", true, Some("alice's GitHub password"))
            ]
        );
        assert_eq!(ask.expires.as_deref(), Some("2h"));
    }

    #[test]
    fn expires_is_normalized_and_validated() {
        let one = |e: &str| {
            parse_request(
                args(serde_json::json!({"name": "T", "note": "n", "expires": e})),
                NOW,
            )
        };
        assert_eq!(one("30m").unwrap().expires.as_deref(), Some("30m"));
        assert_eq!(one("3d").unwrap().expires.as_deref(), Some("3d"));
        assert_eq!(
            one("2030-01-02T14:30:00Z").unwrap().expires.as_deref(),
            Some("2030-01-02T14:30:00Z")
        );
        assert!(
            one("2030-01-02")
                .unwrap()
                .expires
                .unwrap()
                .starts_with("20")
        );
        assert_eq!(one("").unwrap().expires, None, "empty = no suggestion");
        assert!(one("0h").is_err());
        assert!(one("next week").is_err());
        assert!(one("2020-01-01").unwrap_err().contains("in the past"));
    }

    #[test]
    fn exactly_one_shape_and_valid_unique_bounded_fields() {
        let err = |v: serde_json::Value| parse_request(args(v), NOW).unwrap_err();
        assert!(err(serde_json::json!({})).contains("name + note"));
        assert!(
            err(serde_json::json!({"name": "A", "note": "n", "title": "t", "fields": []}))
                .contains("not both")
        );
        assert!(err(serde_json::json!({"name": "A"})).contains("both name and note"));
        assert!(
            err(serde_json::json!({"fields": [{"name": "a", "label": "a"}]})).contains("title")
        );
        assert!(err(serde_json::json!({"name": "a/b", "note": "n"})).contains("name"));
        assert!(err(serde_json::json!({"title": "t", "fields": []})).contains("1 to 16"));
        let many: Vec<_> = (0..17)
            .map(|i| serde_json::json!({"name": format!("f{i}"), "label": "x"}))
            .collect();
        assert!(err(serde_json::json!({"title": "t", "fields": many})).contains("1 to 16"));
        assert!(
            err(serde_json::json!({"title": "t", "fields": [{"name": "a b", "label": "x"}]}))
                .contains("[A-Za-z0-9_.-]")
        );
        assert!(
            err(serde_json::json!({"title": "t", "fields": [
                {"name": "a", "label": "x"}, {"name": "a", "label": "y"}
            ]}))
            .contains("twice")
        );
    }

    #[test]
    fn receipt_lines() {
        let t = Duration::from_secs(60);
        let out = |stored: &[(&str, u32)], expires_ms| Outcome {
            stored: stored.iter().map(|(n, v)| ((*n).to_owned(), *v)).collect(),
            timed_out: false,
            waived: false,
            expires_ms,
            external: None,
        };
        assert_eq!(
            receipt(&out(&[("a", 1), ("b", 2)], None), false, t, "a, b").content,
            "stored a v1, b v2"
        );
        assert_eq!(
            receipt(&out(&[("a", 2)], None), true, t, "a").content,
            "stored a v2 (v1 is {{secret:a:-1}})"
        );
        let at = NOW + 3_600_000;
        assert_eq!(
            receipt(&out(&[("a", 1)], Some(at)), true, t, "a").content,
            format!("stored a v1; expires {}", expiry::local_time(at))
        );
        assert_eq!(receipt(&out(&[], None), false, t, "a").content, "declined");
        let timed = Outcome {
            stored: vec![],
            timed_out: true,
            waived: false,
            expires_ms: None,
            external: None,
        };
        assert_eq!(
            receipt(&timed, false, t, "a").content,
            "no answer after 60 s"
        );
    }

    #[test]
    fn list_line_describes_the_expiry() {
        let info = |expires: Option<String>| SecretInfo {
            name: "TOKEN".into(),
            note: "GitHub PAT".into(),
            source: "local".into(),
            current: 2,
            versions: vec![],
            updated: "2026-09-21T10:00:00Z".into(),
            expires,
        };
        let base = "TOKEN · v2 (0 kept) · GitHub PAT · 2026-09-21T10:00:00Z · local";
        assert_eq!(list_line(&info(None), NOW), base);
        let soon = NOW + 40 * 60_000;
        assert_eq!(
            list_line(&info(Some(expiry::rfc3339(soon))), NOW),
            format!("{base} · expires in 40 min")
        );
        let far = NOW + 3 * 86_400_000;
        assert_eq!(
            list_line(&info(Some(expiry::rfc3339(far))), NOW),
            format!("{base} · expires {}", expiry::local_time(far))
        );
    }
}

#[cfg(test)]
mod delete_tests {
    use kameo::actor::Spawn;
    use runtime::{Runtime, SessionId};
    use tools::{InspectTool, PolicyAction, SessionResources};

    use super::*;
    use crate::KeyStore;

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

    async fn tool(dir: &std::path::Path) -> (SecretDeleteTool, Arc<Vault>, Runtime) {
        let rt = Runtime::new(
            figment::Figment::from(figment::providers::Serialized::defaults(
                serde_json::json!({"tools": {"approvals": false}}),
            )),
            dir.to_path_buf(),
        );
        let vault = Arc::new(Vault::open(dir, KeyStore::File).await.unwrap());
        let actor = SecretsActor::spawn(SecretsActor::new(
            vault.clone(),
            rt.clone(),
            Duration::from_secs(5),
        ));
        (
            SecretDeleteTool {
                vault: vault.clone(),
                actor,
            },
            vault,
            rt,
        )
    }

    async fn run(t: &SecretDeleteTool, name: &str, confirm: u32) -> ToolOutput {
        t.call(
            DeleteArgs {
                name: name.into(),
                confirm,
            },
            cx(),
        )
        .await
        .unwrap()
    }

    /// 221 §13 B6b: every version goes and the placeholder no longer resolves; a stale
    /// `confirm` deletes nothing; `/` names and Send links are refused.
    #[tokio::test]
    async fn secret_delete_removes_every_version_only_when_confirmed() {
        let dir = tempfile::tempdir().unwrap();
        let (t, vault, _rt) = tool(dir.path()).await;
        for v in ["first-value-long", "second-value-long", "third-value-long"] {
            vault.set("TOK", v, None, "user").await.unwrap();
        }
        let out = run(&t, "TOK", 2).await;
        assert!(out.is_error);
        assert_eq!(
            out.content,
            "TOK is at v3, not v2; secret_list shows the current version"
        );
        assert_eq!(
            find(&vault, "TOK").unwrap().versions.len(),
            3,
            "nothing deleted"
        );

        let out = run(&t, "TOK", 3).await;
        assert_eq!(
            out.content, "deleted TOK (v3 and 2 older versions); {{secret:TOK}} no longer resolves",
            "{out:?}"
        );
        assert!(find(&vault, "TOK").is_none());
        assert!(vault.get("TOK", 2).is_none(), "older versions are gone too");
        let mut args = serde_json::json!({"a": "{{secret:TOK}}"});
        assert!(
            vault
                .expand(&mut args)
                .unwrap_err()
                .starts_with("unknown secret TOK")
        );
        let out = run(&t, "TOK", 3).await;
        assert!(out.content.starts_with("unknown secret TOK"), "{out:?}");

        vault
            .set_if_version("oauth/x", "{}", "oauth", None)
            .await
            .unwrap();
        let out = run(&t, "oauth/x", 1).await;
        assert_eq!(out.content, "oauth/x is not a user secret");
        assert!(vault.get("oauth/x", 0).is_some());
        vault
            .set(
                "send-personal-3fa9c2d1",
                "https://vault/#/send/a/b",
                None,
                "user",
            )
            .await
            .unwrap();
        let out = run(&t, "send-personal-3fa9c2d1", 1).await;
        assert_eq!(
            out.content,
            "send-personal-3fa9c2d1 is a Send link; secret_send_revoke revokes it"
        );
        assert!(find(&vault, "send-personal-3fa9c2d1").is_some());
    }

    /// 221 §13 B6b: the seeded entry makes `secret_delete` ask with approvals off; removing it
    /// (the user's press) stops the asking, and seeding again does not bring it back.
    #[test]
    fn secret_delete_is_seeded_once_as_always_ask() {
        crate::connections::tests::serial(async {
            let dir = tempfile::tempdir().unwrap();
            let (t, vault, rt) = tool(dir.path()).await;
            let reg = ::tools::registry::start(&rt).await.unwrap();
            reg.ask(RegisterTool(Arc::new(t))).await.unwrap();
            let session = SessionId::new();
            let action = || async {
                reg.ask(InspectTool {
                    session,
                    name: "secret_delete".into(),
                    args: None,
                })
                .await
                .unwrap()
                .action
            };
            assert_eq!(
                action().await,
                PolicyAction::Allow,
                "approvals off, no entry"
            );
            seed_delete_ask(&reg, &vault).await.unwrap();
            assert_eq!(
                action().await,
                PolicyAction::Ask,
                "always ask with approvals off"
            );
            reg.ask(SetAlwaysAsk {
                session: None,
                tool: "secret_delete".into(),
                on: false,
            })
            .await
            .unwrap();
            assert_eq!(action().await, PolicyAction::Allow);
            seed_delete_ask(&reg, &vault).await.unwrap();
            assert_eq!(
                action().await,
                PolicyAction::Allow,
                "a removed entry stays removed"
            );
        });
    }
}
