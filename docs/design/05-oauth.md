# OAuth login: LLM providers and MCP servers

Status: draft v0 · 2026-09-09 · extends `01-config-and-subagents.md` §1.3–1.4, `02-models-and-menus.md` §2, §5.

Two logins that look the same to the user and share one small crate:

1. **Provider login** — use a ChatGPT subscription instead of an API key.
   `wth` is the OAuth client; the token turns into the auth of every model call
   through genai's resolver hooks. No new LLM code path.
2. **MCP login** — a remote (`url`) server answers 401. `rmcp` already
   implements the whole MCP authorization spec (discovery, dynamic client
   registration, PKCE, refresh); we give it a file-backed credential store and
   a loopback redirect.

jarvis is inspiration only: it confirmed the protocol facts (§2, §3) and the
pitfalls (§6). Its shape — a store per provider, a shared axum callback crate,
a `McpClientManager` auth ladder — is not copied; here both logins are
messages on actors that already exist.

## 1. Layout

```
~/.wth/oauth/
  openai.json          provider tokens, mode 0600 (file stem = genai namespace)
  mcp/<server>.json    rmcp StoredCredentials, mode 0600 (file stem = server name)
```

Crate `oauth` (purpose-named, no UI, no genai, no rmcp), ~250 lines:

```rust
/// Bind 127.0.0.1:port (0 = ephemeral), serve exactly one GET, reply with a
/// "you can close this tab" page, resolve with the full redirect URL received.
pub async fn callback(port: u16) -> io::Result<(u16, impl Future<Output = Result<String, String>>)>;
/// `open`/`xdg-open`, best effort.
pub fn open_browser(url: &str);
/// PKCE S256 pair + random state (32 bytes each, base64url no pad).
pub fn pkce() -> Pkce { verifier, challenge, state }

/// One authorization-code + PKCE provider. Data, not code: adding a provider
/// is a row (its request shaping is one `match` arm in `agent`).
pub struct Provider { pub ns: &'static str, auth_url, token_url, client_id, scopes, port: u16, redirect_path, extra_query: &[(&str, &str)] }
pub const PROVIDERS: &[Provider] = &[OPENAI];
pub fn provider(ns: &str) -> Option<&'static Provider>;

#[derive(Serialize, Deserialize)]
pub struct Tokens { access_token, refresh_token: Option<String>, expires_at: u64 /* unix secs */, account_id: Option<String> }
pub struct Store { dir: PathBuf }   // <data_dir>/oauth
impl Store {
    pub fn load(&self, ns) -> Option<Tokens>; pub fn save(&self, ns, &Tokens) -> io::Result<()>; pub fn remove(&self, ns) -> io::Result<()>;
    /// Start a login: the URL to open, and a future that waits for the
    /// callback (5 min), exchanges the code, saves, and yields the tokens.
    pub async fn login(&self, p: &Provider) -> Result<(String, impl Future<Output = Result<Tokens, String>>), String>;
    /// Tokens for `ns`, refreshed and re-saved when within 60 s of expiry.
    /// One refresh at a time per store (`tokio::sync::Mutex`).
    pub async fn access(&self, p: &Provider) -> Result<Tokens, String>;
}
```

The callback server is ~40 lines over `tokio::net::TcpListener`: read until
`\r\n\r\n`, take the request target from the first line, write a fixed
`200 text/html` response. No axum/hyper for one loopback request.

Deps (all already in `Cargo.lock` transitively): `reqwest` 0.13 (same major
as rmcp, `rustls`), `sha2` 0.10, `base64` 0.22; randomness from `uuid` v4
(already a workspace dep). JWT claims (`exp`, account id) are the base64url
payload segment parsed with `serde_json` — no JWT crate, no verification (the
provider verifies; we only read our own token's expiry).

Rejected: `oauth2` crate directly for providers (rmcp already pulls it in,
but the provider flows are a 20-line form POST each and the crate's type
state adds more than it removes); OS keychain (doc 01 §1.3 argument stands);
one merged `oauth.json` (per-file lets `/logout` be `unlink`).

## 2. Provider login (agent crate)

### 2.1 What is supported

| Namespace | Flow | Notes |
|---|---|---|
| `openai` | ChatGPT (Codex) — PKCE, browser, fixed port **1455**, redirect `http://localhost:1455/auth/callback` | The Codex CLI public client id; the port is what that client registered, so it is not configurable. Models are served by `https://chatgpt.com/backend-api/codex/responses` (Responses API only). |
| `anthropic` | Claude Pro/Max (Claude Code) — PKCE against `claude.ai/oauth/authorize`, port **54545**, redirect `http://localhost:54545/callback`, extra `code=true`; exchange and refresh are **JSON** at `api.anthropic.com/v1/oauth/token` (`state` in the exchange body; refresh carries `anthropic-beta: oauth-2025-04-20` + the SDK user agent) | Token `sk-ant-oat…` is a bearer for the plain Messages API with the Claude Code fingerprint (§2.3). The grant family expires ~30 days after the interactive login whatever the refreshes: re-login. `platform.claude.com` mints console tokens without `user:inference`; only the `claude.ai` endpoint works. |
| `cursor` | Cursor subscription — not OAuth: deep link `cursor.com/loginDeepControl?challenge=&uuid=&mode=login&redirectTarget=cli`, then poll `api2.cursor.sh/auth/poll?uuid=&verifier=` (404 = pending) for `{accessToken, refreshToken}`; refresh `POST /auth/exchange_user_api_key` with the refresh token as bearer | No client id, no loopback port. Inference is not genai: Connect-RPC over HTTP/2 + protobuf to `api2.cursor.sh/agent.v1.AgentService/Run`, implemented in crate `cursor` (§2.5). Model list comes from `GetUsableModels` at `/model` time. |
| `xai` | SuperGrok / X Premium+ — RFC 8628 device grant: `POST auth.x.ai/oauth2/device/code` (Grok CLI public client id), open `verification_uri_complete`, poll `oauth2/token` with `grant_type=…device_code` (`authorization_pending`/`slow_down` = not yet); refresh is the standard form grant | No loopback port. Access token is a plain bearer on `api.x.ai/v1`: genai's `xai` adapter unchanged, resolver sets `AuthData::Key`. |

Anthropic's terms restrict subscription tokens to Claude Code and accounts
of third-party harness users have been suspended; the row exists because
the user asked for it knowing that. Not included: **Google** (Gemini CLI's
Cloud Code Assist endpoints) — add when someone needs it. `Provider::flow`
distinguishes `Code { token_body, refresh_headers }`, `Poll { refresh_url }`
and `Device`; everything else is the same row shape.

### 2.2 Precedence

For a genai namespace `ns`, in this order:

1. `credentials.yaml` has `ns:` → API key (unchanged).
2. `<data_dir>/oauth/<ns>.json` exists → OAuth.
3. the adapter's default env var is set → API key (unchanged).

The explicit key stays first (doc 01 §1.3: "the file the user edited on
purpose"); a login is also on purpose but a user who set a key *and* logged
in most likely wants the key. `configured(ns)` (doc 02 §1) gains clause 2, so
`/model` lists OpenAI models right after login.

### 2.3 Wiring

genai resolves a request in three steps: auth resolver → default endpoint →
`ServiceTargetResolver` (final say over `{ model, endpoint, auth }`). Today we
set only the auth resolver. Add an async service-target resolver; in it:

```rust
// adapter OpenAI | OpenAIResp, no key in creds, tokens on disk:
let t = oauth.access(provider("openai")).await?;         // refreshes if needed
target.model.adapter_kind = AdapterKind::OpenAIResp;      // Responses API
target.endpoint = Endpoint::from_static("https://chatgpt.com/backend-api/codex/");
target.auth = AuthData::RequestOverride {
    url: "https://chatgpt.com/backend-api/codex/responses".into(),
    headers: Headers::from(vec![
        ("Authorization", format!("Bearer {}", t.access_token)),
        ("chatgpt-account-id", t.account_id.unwrap_or_default()),
        ("OpenAI-Beta", "responses=v1".into()),
        ("originator", "codex_cli_rs".into()),
    ]),
};
```

`RequestOverride` replaces the adapter's URL and headers wholesale; the JSON
body and `content-type` come from `reqwest::RequestBuilder::json`, so the four
headers above are the complete set. genai's `OpenAIResp` adapter already
does what the Codex backend requires: `store: false`, system prompt as
top-level `instructions`, `stream: true`. Model ids are unchanged
(`openai/gpt-5-codex` → `openai::gpt-5-codex`); only the target flips.

Refresh happens inside `Store::access`, i.e. inside the resolver, per model
call, single-flight. Refresh rejected (400/401/403) → the tokens file is
kept (the refresh token may still be good later), `access` returns
`Err("openai: login expired; run /login openai")`, and the turn fails
visibly with that text — the same place an invalid API key fails today.

Anthropic keeps the adapter and swaps only the auth: URL
`https://api.anthropic.com/v1/messages?beta=true`, headers `Authorization:
Bearer`, `anthropic-version: 2023-06-01`, `anthropic-beta:
claude-code-20250219,oauth-2025-04-20,interleaved-thinking-2025-05-14`,
`User-Agent: claude-cli/2.1.257 (external, cli)`, `x-app: cli`,
`anthropic-dangerous-direct-browser-access: true`. The loop prepends `You
are Claude Code, Anthropic's official CLI for Claude.` to the system prompt
when the login is in force (`Auth::login("anthropic")`), which is what the
backend checks. Deliberately **not** replicated from oh-my-pi's fuller
fingerprint: the billing-header system block with its xxhash `cch`
attestation (needs post-serialisation body patching genai cannot do), the
`_` tool-name prefix, `metadata.user_id`, the Stainless headers. If Anthropic
starts rejecting the minimal set, those need a genai patch
(`[patch.crates-io]`, rule 3), not more resolver code.

### 2.5 Cursor transport (crate `cursor`)

genai has no Cursor adapter and Cursor speaks no OpenAI/Anthropic dialect,
so a purpose-named crate implements the client: `agent.proto` (from
oh-my-pi, compiled with `prost` + `protox` at build time, no system
`protoc`), Connect framing (1 flag byte + u32 length; EndStream flag carries
a JSON error), a bidirectional `Run` over HTTP/2 with a 5 s client
heartbeat, blob-addressed conversation state (`root_prompt_messages_json` =
the whole history as JSON blobs, served back on `KvServerMessage.getBlob`),
the request-context handshake (system prompt as a rule, every tool as an
MCP tool definition — no native exec tools), and `InteractionUpdate` →
`genai::chat::ChatStreamEvent`. The loop sees the same events as from genai:

```rust
pub async fn models(token: &str) -> Result<Vec<Model>, String>;                 // GetUsableModels
pub async fn chat_stream(token: &str, model: &str, req: ChatRequest, conversation: &str)
    -> Result<Pin<Box<dyn Stream<Item = Result<ChatStreamEvent, String>> + Send>>, String>;
```

`loop_::round` picks the transport by namespace (`cursor::…` → this crate,
else genai) behind one `EventStream` type; nothing downstream changes. A tool
call ends the `Run` (`End` with the calls captured); the next round sends the
results in the history and resumes. Model ids are `cursor/<slug>` (rule 2
pass-through in the catalog, no context limit known); `ListModels` appends
the account's `GetUsableModels` slugs when a Cursor login is in force. Their
context limit comes from `Catalog::closest`: the bundled model sharing the
most name tokens with the slug, effort/lane words (`low`, `xhigh`,
`thinking`, `max`, …) ignored (`cursor/claude-4.6-sonnet-medium-thinking` →
`claude-sonnet-4-6`); same rule for any unknown `provider/model`. Limits
only — the id sent to the provider is never rewritten.

### 2.4 Commands and event

```rust
pub struct ListLogins;                              // ask → Vec<LoginInfo>
pub struct LoginInfo { pub name: String, pub logged_in: bool, pub account: Option<String>, pub expires_at: Option<u64> }
pub struct Login  { pub provider: String }          // ask → Result<String /* url */, String>
pub struct Logout { pub provider: String }          // ask → Result<(), String>
/// Event: the login started by `Login` ended.
pub struct LoginFinished { pub provider: String, pub error: Option<String> }
```

`Login` binds the callback port first (fails fast on "1455 in use"),
returns the URL, opens the browser best-effort, and spawns the wait. The
result is published as `LoginFinished` — a separate event struct like
`McpProgress`, not an `AgentEvent` variant, because it has no session.
`Logout` unlinks the file. RPC: `list_logins`, `login`, `logout`;
`LoginFinished` joins `subscribe_events`.

## 3. MCP login (tools crate)

### 3.1 Connect

`Transport::Http` connects through rmcp's `AuthClient<reqwest::Client>`
unless the entry already carries an `Authorization` header (the user is
doing auth by hand; leave them alone):

```rust
let http = reqwest::Client::new();
let mut m = AuthorizationManager::new(url).await?;
m.with_client(http.clone())?;
m.set_credential_store(FileStore::new(dir.join("oauth/mcp").join(format!("{name}.json"))));
m.initialize_from_store().await?;            // discovery only when tokens exist
let transport = StreamableHttpClientTransport::with_client(AuthClient::new(http, m), cfg);
```

With tokens, rmcp adds `Authorization: Bearer`, refreshes before expiry and
retries once on a rejected token. Without tokens the request goes out plain;
a 401 surfaces as `StreamableHttpError::AuthRequired(challenge)` inside
`ClientInitializeError`. `settle` recognises it with
`ClientInitializeError::auth_challenge()` (rmcp does not mark the transport
error as `#[source]`, so anyhow's chain never reaches it), records
`state.login[name] = challenge` instead of a generic connect error, and
`McpServer` gains

```rust
pub auth: McpAuth   // None | Required | LoggedIn
```

so the `/mcp` menu shows `○ login required` rather than `○ not connected`.
Everything else — reconcile, gates, registration — is unchanged.

### 3.2 Login

```rust
pub struct McpLogin  { pub server: String }   // ask → Result<String /* url */, String>
pub struct McpLogout { pub server: String }   // ask → Result<(), String>
```

`McpLogin`: `oauth::callback(0)` → redirect
`http://127.0.0.1:<port>/callback`; `AuthorizationManager::new(url)` with the
same `FileStore`; `resolve_metadata_from_challenge(saved challenge)` then
`set_metadata`; `AuthorizationSession::new(manager,
AuthorizationRequest::new(redirect).with_client_name("wth")[.with_preregistered_client(id).with_client_secret(s).with_scopes(..)])`
— rmcp picks pre-registered → CIMD → dynamic registration in that order. The
handler returns `session.get_authorization_url()`, opens the browser, and
spawns: await the callback (5 min) → `session.handle_callback_url(url)` (rmcp
exchanges the code and **saves through our store**) → `tell Restart(name)`.
Failure → `state.errors[name] = "login failed: …"` + progress publish.

Optional entry fields, read by `entry()` and ignored by everything else;
each is independent:

```json
{ "url": "https://mcp.example.com/mcp",
  "oauth": { "client_id": "…", "client_secret": "…", "scopes": ["…"],
             "redirect_uri": "http://localhost:8765/callback" } }
```

`client_id` (+ `client_secret`) for servers that offer neither DCR nor CIMD
(Slack-style pre-registration); `redirect_uri` for servers that only accept a
registered loopback URL: `McpLogin` listens on its port (`oauth::callback(port)`,
bound on 127.0.0.1, so the host may be `localhost` or `127.0.0.1`) and sends the
URI verbatim as `redirect_uri`. Without it the listener is ephemeral and the
URI `http://127.0.0.1:<port>/callback`. A busy port fails the login with the
bind error rather than falling back, since the fallback URI would be rejected
anyway.

`McpLogout`: unlink the file, `Restart(name)` (the next connect is plain and
gets the 401 again → `login required`).

RPC: `mcp_login`, `mcp_logout`. No new event: the server flipping
failed → connected is already an `McpProgress`.

### 3.3 `FileStore`

`impl rmcp::transport::auth::CredentialStore` over one JSON file:
`load` = read + `serde_json` (missing → `None`), `save` = write-then-rename +
`chmod 600`, `clear` = unlink. `acquire_refresh_guard` keeps the default (one
process owns the file; a second `wth` on the same dir is already unsupported
by the sqlite store).

## 4. TUI

- `/login [name]` — no argument: picker over provider rows (`ListLogins`)
  and MCP rows (`ListServers` filtered to `auth != None`), each with its
  status; Enter logs the row in. With a name: providers first, then servers.
  Both print `login: open <url>` as an `Info` line (the URL is a link in any
  modern terminal; the browser was also opened) and later
  `login: openai ok` / `login: linear failed: …`.
- `/logout <name>` — same resolution.
- `/mcp` menu: `l` on a server row logs in (or out when logged in); the row
  reads `○ login required` / `● connected` as today.
- `/model` needs nothing: `ListModels` already filters by `configured`.

The line printer (non-tty) prints the same `Info` lines, which is what the
headless verification below reads.

## 5. Implementation order

1. `oauth` crate: callback, pkce, `Store`, `PROVIDERS` (`openai`). Check:
   unit test drives `callback` with a local HTTP GET; `Store::login` against
   a throwaway token endpoint on loopback exchanges + saves + refreshes.
2. Agent: resolver, `configured`, `ListLogins`/`Login`/`Logout`,
   `LoginFinished`. Check: with no `OPENAI_API_KEY` and a hand-placed
   `oauth/openai.json`, `list_models` shows openai models and a turn's request
   goes to `chatgpt.com` with the four headers (log at debug).
3. Tools: `FileStore`, `AuthClient` connect, `McpAuth`, `McpLogin`/`McpLogout`,
   `oauth` entry field. Check: `npx @modelcontextprotocol/server-everything`
   has no HTTP+OAuth mode; use rmcp's example `servers/auth_server` (or any
   public OAuth MCP server) — `login required` shown, `/login <name>` →
   browser → connected → tools listed; restart `wth` → connects from the file
   without a prompt.
4. TUI + RPC: `/login`, `/logout`, menu key, methods, event forwarding.
5. Docs: 00 §10 crate table, 01 §1.1 layout + §1.3 precedence, README crates
   line.

1 first; 2 and 3 in parallel against §2.4/§3.2; 4 after both compile.

## 6. Pitfalls carried from jarvis (kept as rules, not code)

- `configure_client*` before `resolve_metadata`/`set_metadata` returns
  `NoAuthorizationSupport` — always resolve first (Home Assistant broke on it).
- Keep the old refresh token when a refresh response omits one.
- Never log OAuth response bodies (they echo tokens).
- Fixed port 1455: bind before returning the URL so "in use" fails at
  `/login`, not five minutes later.
- A refresh that succeeds must be used even if the new expiry is short, or
  the resolver refreshes forever.
- Match rmcp's `reqwest` major or `AuthClient<reqwest::Client>` will not
  satisfy `StreamableHttpClient`.

## 7. Status (2026-09-09)

Shipped. Verified headless over RPC with `OPENAI_API_KEY` unset and
`PATH=/bin` (no `open`, so nothing popped): `list_logins` → `openai` not
logged in and `list_models` shows 0 openai models; a token file appears →
`logged in (acct)` and 48 openai models with no restart; `logout` unlinks;
`login openai` → `auth.openai.com/oauth/authorize` URL with `S256`,
`redirect_uri=http://localhost:1455/auth/callback`, `originator=codex_cli_rs`,
and a second `login` while the port is held fails at once (`1455: Address
already in use`); a callback carrying `error=access_denied` → 400 page and
`LoginFinished { error: "openai: access_denied user said no" }`; with a fake
token a turn on `openai/gpt-5` reaches `chatgpt.com` (adapter `OpenAIResp`,
401 "Could not parse your authentication token" — the Codex backend's text,
not api.openai.com's). A loopback MCP server answering 401 with a Bearer
challenge lists as `auth: required` after one `POST /mcp`; `mcp_login` on it
fails cleanly (`Dynamic registration failed … 401`). TUI on a pty: `/login`
picker (`provider openai … logged in (acct)`, `mcp fake … login required`),
Enter prints `login: open https://…`, `/mcp` shows `○ login required` and
the `l` hint.

Then a real login: the URL from the transcript, ChatGPT sign-in in the
browser, `login: openai ok`, `oauth/openai.json` with `account_id` and a
ten-day `expires_at`; a headless turn on `openai/gpt-5.5` answered `codex
login works` (`completed`, 1848↑ 8↓).

Found while verifying: the Codex backend serves a ChatGPT account only the
slugs its `GET /models?client_version=1.0.0` lists (today `gpt-5.5`,
`gpt-5.6-sol|terra|luna`); `gpt-5` and `gpt-5-codex` answer 400 "not
supported when using Codex with a ChatGPT account". `/model` still lists the
whole openai catalog under a login; the error is the backend's own and names
the model. Ceiling: filter the picker through `/models` when it grates.

Not exercised: a live token refresh (unit-tested against a loopback token
endpoint), a positive MCP OAuth round trip (needs a server with an
authorization server; the code path is rmcp's, our part is the store and the
callback, both exercised).
