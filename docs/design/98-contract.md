# 98 — Contract: crate skeleton and seam ownership for the Telegram channel

Status: contract v0 · 2026-10-05 · for `98-telegram.md` v0.1 (tgbot 0.48.1). Base: local `main`
`7e514107` (v2.7.0). Integration branch `feature/98-telegram` in `~/Develop/theuth-98`.

This file is the agreement between the five omp workers that build design 98 in parallel. Stage
1 (CONTRACT) lands the crate skeleton below, compiling and tested, on `feature/98-telegram`.
Stage 2 cuts four worktrees from that sha; each worker owns the modules in §1 and may change
the signatures in §2 only inside its own modules. A cross-seam need is a `// NEEDS(<OWNER>): …`
comment plus a line in the worker's as-built note, never an edit in another worker's file.
Where the compiler forces a different shape than §2, CONTRACT records it under "Deviations" at
the end of this file; stage-2 workers read the landed code, not this prose, for the exact types.

Hard rules for every worker: **no network calls to Telegram** — there is no bot, no token, and
`api.telegram.org` must never be contacted; every test runs against `wiremock` on 127.0.0.1
(`Api::new(token, Some(host))` / `tgbot::api::Client::with_host`) or against `api::mock::
MockSender`. Never run `t --headless` against `~/.theuth`; a scratch data dir only, and only the
`Unconfigured` path (no token). Per-crate cargo gates only (§4).

## 1. Seams

| module (`crates/telegram/src/…`) | stage 1 (CONTRACT) | stage 2 owner | callers |
|---|---|---|---|
| `Cargo.toml`, workspace `[workspace.dependencies]` (`tgbot = "=0.48.1"`, `telegram = { path }`) | real | — (integrator) | — |
| `lib.rs` — `TelegramConfig`, `TelegramEvent`, `TelegramStatus`, `BotIdentity`, `CallStats`, `Pairing`, actor `Telegram` + messages, `Plugin::start`, tools `telegram_link` / `telegram_status`, `render_status` | real | CONTRACT; small fixes by the integrator | rpc (WIRE), main.rs (WIRE) |
| `api.rs` — channel-facing types, `trait Sender`, `Api` over `tgbot::api::Client`, pacer, counters, error table; `#[cfg(test)] pub mod mock` (`MockSender`) | real | CONTRACT | everyone |
| `port.rs` — `trait Port` (agent/approval/ask asks behind a trait) + live impl | real (copied from `crates/slack/src/channel.rs` `trait Port` + impl, plus `ancestors`) | CONTRACT | RENDER, ROUTE, COMMANDS |
| `threads.rs` — `ThreadKey`, `Thread`, `Topic`, `Threads` (+ `open` list), `threads.json` | real | CONTRACT | ROUTE, COMMANDS |
| `poll.rs` — lease-gated getUpdates loop, backoff, `RecentKeys`, `classify(Update) -> Incoming` | real | CONTRACT | ROUTE (spawns it) |
| `worker.rs` — per-thread turn renderer (`WorkerCtx`, `WorkerMsg`, `spawn`) | **stub**: signatures, a loop that logs and drops | **RENDER** | ROUTE |
| `bubble.rs`, `cards.rs`, `board.rs` | absent | **RENDER** | worker.rs |
| `crates/markdown/src/lib.rs` — `pub fn plain(md: &str) -> String`, `pub fn chunks(text: &str, max: usize) -> Vec<String>` (copies of `signal::text::{markdown_to_plain, chunk_text}` with their unit tests; `crates/signal` is never edited) | real (`crates/markdown/src/plain.rs`, re-exported from `lib.rs`; gate `cargo test -p markdown`) | — | RENDER (plain fallback); `Api::send_text` still takes already-plain text |
| `channel.rs` — `Channel`, `Handle`, `spawn`, `Bus`, inbound routing, router, owner chat, lease gate, approvals/asks/artifact/secret/`message_user` dispatch | **skeleton**: `spawn` returns a `Handle` whose methods work (status/allow/…); `Bus` subscribes to the eight event types and forwards into one ordered `mpsc`; `handle_inbound`/`handle_event` log and drop | **ROUTE** | lib.rs |
| `approvals.rs`, `asks.rs`, `user.rs` | `user.rs` **stub** (`impl UserSink for Handle` → `Ok(None)` / `Err("telegram: not implemented")`); the others absent | **ROUTE** | channel.rs |
| `commands.rs`, `ui.rs`, `setup.rs` | **stubs** with the §2 signatures (`handle` → help text; `action` → `None`; `guard` → identity; `setup::run` → `Err("not implemented")`) | **COMMANDS** | channel.rs (ROUTE), poll.rs boot (`scopes`), main.rs (WIRE) |
| `crates/theuth/src/main.rs`, `crates/rpc/src/{api,bridge,export}.rs`, `crates/rpc/Cargo.toml`, `crates/theuth/Cargo.toml`, README, `docs/design/{00,09,24,45,62,67}` amendments, `docs/plugins.md` | absent | **WIRE** | — |
| `docs/design/98-as-built-<TAG>.md` | — | each worker its own | — |

Nothing outside this table is touched by anyone without a NEEDS note.

## 2. Public surface

Field lists are the minimum; a worker may add fields to types it owns. Types below live in the
module named in the heading unless a path says otherwise. All `async fn` in traits use
`async_trait`.

### 2.1 `lib.rs`

```rust
pub const TELEGRAM_ACTOR: &str = "telegram";
pub const SECRET_TOKEN: &str = "telegram_bot";            // vault key (37 §3.6)

#[derive(Clone, Debug, Serialize, Deserialize)] #[serde(default)]
pub struct TelegramConfig {
    pub enabled: bool,              // true
    pub owner: Option<u64>,
    pub allowed: Vec<u64>,
    pub groups: Vec<i64>,
    pub tools: ToolsMode,           // all | errors | none  (serde rename_all = "lowercase")
    pub reasoning: bool,            // true
    pub approvals: ApprovalsMode,   // all | threads
}
pub enum ToolsMode { All, Errors, None }
pub enum ApprovalsMode { All, Threads }

#[derive(Clone, Debug, Serialize, Deserialize, JsonSchema)] #[cfg_attr(feature = "ts", derive(ts_rs::TS), ts(export))]
#[serde(tag = "type")]
pub enum TelegramEvent {
    Identity      { bot: String, topics: bool },
    Connected     { bot: String },
    Disconnected  { reason: String },
    Conflict,
    RateLimited   { method: String, retry_after_secs: u64 },
    StreamFallback{ session: SessionId, reason: String },
    Unreachable   { chat: i64, reason: String },
    PairingLink   { url: String, expires_at_ms: u64 },
    Paired        { user: u64, name: String },
    Unconfigured,
    DaemonOnly,
}
pub struct BotIdentity { pub id: u64, pub username: String, pub topics: bool, pub users_may_create_topics: bool }
pub struct CallStats { pub ok: u64, pub err: u64, pub rate_limited: u64 }
pub struct TelegramStatus {              // Serialize/Deserialize/JsonSchema/TS, kameo::Reply
    pub state: Option<TelegramEvent>, pub bot: Option<BotIdentity>, pub polling: bool,
    pub owner: Option<u64>, pub allowed: Vec<u64>, pub groups: Vec<i64>,
    pub threads: Vec<(String, SessionId)>, pub open: u32,
    pub calls: BTreeMap<String, CallStats>, pub dropped_updates: u64,
}
pub struct PairingLink { pub url: String, pub expires_at_ms: u64 }   // Serialize/Deserialize/JsonSchema/TS
pub struct Pairing { pub code: String, pub expires: Instant }
pub const PAIR_TTL: Duration = Duration::from_secs(600);
pub fn mint_code() -> String;                                       // 16 chars of the 09 §10 alphabet

pub struct TelegramPlugin { pub channel: bool }
// actor `Telegram` registered as TELEGRAM_ACTOR; messages (ask → reply):
pub struct Status;                              // → TelegramStatus
pub struct Link { pub owner: Option<u64> }      // → Result<Option<PairingLink>, String>  (None = owner set directly; §4)
pub struct Allow(pub u64); pub struct RemoveAllowed(pub u64);   // → Result<(), String>  (live list + write_config_value)
pub struct AllowGroup(pub i64); pub struct RemoveGroup(pub i64); // → Result<(), String>
pub struct SetOwner(pub u64);                   // → Result<(), String>  (live + config; used by pairing)
pub fn render_status(s: &TelegramStatus) -> String;
// tools: TelegramLinkTool (NAME "telegram_link", Risk::Mutating, args { owner: Option<u64> }),
//        TelegramStatusTool (NAME "telegram_status", Risk::ReadOnly, args {})
```

`Plugin::start` is `crates/slack/src/lib.rs:343-425` with the names changed: section `telegram`,
one secret `telegram_bot`, `channel::spawn(cfg, token, rt)`, actor, two tools, `RegisterUserSink`
with the `Handle`.

### 2.2 `api.rs`

```rust
pub struct Target { pub chat: i64, pub thread: Option<i64> }
pub struct MsgRef { pub chat: i64, pub id: i64 }
pub struct Keyboard { pub rows: Vec<Vec<Button>>, pub force_reply: bool }
pub enum Button { Callback { text: String, data: String }, Url { text: String, url: String }, Disabled { text: String } }
pub struct OutFile { pub name: String, pub mime: String, pub bytes: Vec<u8> }
pub struct Ephemeral { pub user: u64, pub callback_query_id: Option<String> }
pub enum CommandScope { Default, AllGroupChats, Chat(i64) }
pub struct BotCommandSpec { pub name: String, pub description: String, pub ephemeral: bool }

#[derive(Debug, thiserror::Error)]
pub enum ApiError {
    #[error("telegram {code}: {description}")] Api { code: i64, description: String },
    #[error("rate limited; retry after {0:?}")] RateLimited(Duration),
    #[error("transport: {0}")] Transport(String),
    #[error("too big")] TooBig,
}
impl ApiError { pub fn description(&self) -> Option<&str>; pub fn is(&self, exact: &str) -> bool; pub fn starts_with(&self, prefix: &str) -> bool; }

// §4.3 table: exact Telegram description strings as consts (CONFLICT, NOT_MODIFIED_PREFIX, TO_EDIT_NOT_FOUND,
// CANT_BE_EDITED, BOT_BLOCKED, USER_DEACTIVATED, CHAT_NOT_FOUND, KICKED_SUPERGROUP, TOPIC_CLOSED, THREAD_NOT_FOUND,
// FILE_TOO_BIG, and fn is_unreachable(&ApiError) -> bool, is_gone_message(&ApiError) -> bool, is_topic_gone(..)).

#[async_trait]
pub trait Sender: Send + Sync + 'static {
    async fn send_rich(&self, to: &Target, md: &str, markup: Option<&Keyboard>, reply_to: Option<i64>, ephemeral: Option<&Ephemeral>) -> Result<i64, ApiError>;
    async fn edit_rich(&self, at: &MsgRef, md: &str, markup: Option<&Keyboard>) -> Result<(), ApiError>;
    async fn draft(&self, to: &Target, draft_id: i64, md: &str, can_stop: bool) -> Result<(), ApiError>;
    async fn send_text(&self, to: &Target, text: &str, reply_to: Option<i64>) -> Result<i64, ApiError>;   // plain, no parse_mode, caller chunks ≤ 4096
    async fn edit_markup(&self, at: &MsgRef, markup: Option<&Keyboard>) -> Result<(), ApiError>;
    async fn react(&self, at: &MsgRef, emoji: Option<&str>) -> Result<(), ApiError>;                      // None clears
    async fn typing(&self, to: &Target) -> Result<(), ApiError>;
    async fn answer_callback(&self, id: &str, toast: Option<&str>) -> Result<(), ApiError>;
    async fn send_file(&self, to: &Target, file: &OutFile, caption: Option<&str>, reply_to: Option<i64>) -> Result<i64, ApiError>; // photo | voice | document by mime+size (§8.10)
    async fn download(&self, file_id: &str, max: u64) -> Result<(Vec<u8>, Option<String>), ApiError>;     // TooBig before fetching when file_size > max
    async fn create_topic(&self, chat: i64, name: &str) -> Result<i64, ApiError>;
    async fn rename_topic(&self, chat: i64, topic: i64, name: &str) -> Result<(), ApiError>;
    async fn close_topic(&self, chat: i64, topic: i64) -> Result<(), ApiError>;
    async fn delete_message(&self, at: &MsgRef) -> Result<(), ApiError>;
    async fn set_commands(&self, scope: CommandScope, cmds: &[BotCommandSpec]) -> Result<(), ApiError>;
    async fn delete_webhook(&self) -> Result<(), ApiError>;
    async fn me(&self) -> Result<BotIdentity, ApiError>;
    fn stats(&self) -> BTreeMap<String, CallStats>;
}

pub struct Api { /* tgbot::api::Client with_max_retries(2).with_max_retry_after(30), pacer, counters */ }
impl Api {
    pub fn new(token: &str, host: Option<&str>) -> Result<Api, ApiError>;   // host = wiremock base URL in tests
    pub async fn get_updates(&self, offset: i64, limit: u8, timeout_secs: u64, reset_allowed: bool) -> Result<Vec<RawUpdate>, ApiError>;
    pub fn client(&self) -> &tgbot::api::Client;      // poll::classify needs the raw Update; nobody else
}
impl Sender for Api { … }
pub struct RawUpdate { pub id: i64, pub update: tgbot::types::Update }

// Pacer (§4.2): keys Chat(chat) 1000 ms (groups 3000 ms), Draft(chat, thread) 1000 ms, Edit(message) 1500 ms,
// overall 30/s; implemented inside Api, invisible to callers. Every method bumps counters and is a tracing span.

#[cfg(test)]
pub mod mock {
    pub enum Call { SendRich { to: Target, md: String, markup: Option<Keyboard>, reply_to: Option<i64>, ephemeral: Option<Ephemeral> }, EditRich { at: MsgRef, md: String, markup: Option<Keyboard> }, Draft { to: Target, draft_id: i64, md: String, can_stop: bool }, SendText { .. }, EditMarkup { .. }, React { at: MsgRef, emoji: Option<String> }, Typing(Target), AnswerCallback { id: String, toast: Option<String> }, SendFile { to: Target, name: String, mime: String, len: usize, caption: Option<String>, reply_to: Option<i64> }, Download(String), CreateTopic { chat: i64, name: String }, RenameTopic { .. }, CloseTopic { .. }, DeleteMessage(MsgRef), SetCommands { scope: CommandScope, names: Vec<String> }, DeleteWebhook, Me }
    pub struct MockSender { /* Mutex<Vec<Call>>, next message id from 1000, failure queue */ }
    impl MockSender {
        pub fn new() -> Arc<Self>;
        pub fn calls(&self) -> Vec<Call>;
        pub fn take_calls(&self) -> Vec<Call>;
        pub fn fail_next(&self, method: &'static str, err: ApiError);   // the next call of that method returns err
        pub fn set_identity(&self, me: BotIdentity);
        pub fn set_download(&self, file_id: &str, bytes: Vec<u8>, name: Option<&str>);
    }
    impl Sender for MockSender { … }
}
```

### 2.3 `port.rs`

```rust
#[async_trait]
pub trait Port: Send + Sync {                        // = crates/slack/src/channel.rs:94-123 + ancestors
    async fn send_input(&self, session: Option<SessionId>, text: String, files: Vec<PathBuf>, scope: PeerScope) -> Result<SessionId, String>;
    async fn new_root(&self, scope: PeerScope) -> Result<SessionId, String>;
    async fn steer(&self, session: SessionId, text: String, files: Vec<PathBuf>) -> Result<(), String>;
    async fn cancel(&self, session: SessionId) -> Result<(), String>;
    async fn list_sessions(&self) -> Result<Vec<SessionSummary>, String>;
    async fn load_session(&self, session: SessionId) -> Result<LoadedSession /* whatever slack's uses */, String>;
    async fn resolve(&self, id: ApprovalId, decision: Decision) -> Result<bool, String>;
    async fn answer_ask(&self, id: AskId, answers: Option<Vec<Answer>>) -> Result<bool, String>;
    async fn list_agents(&self, session: SessionId) -> Result<Vec<AgentInfo>, String>;
    async fn ancestors(&self, session: SessionId) -> Result<Vec<SessionId>, String>;   // self first, root last (24 §3)
    async fn session_title(&self, session: SessionId) -> Result<Option<String>, String>;
}
pub struct LivePort;   // the slack impl, verbatim semantics
```

### 2.4 `threads.rs`

```rust
pub type ThreadKey = String;                                   // "<chat>" | "<chat>:<topic>"
pub fn key(chat: i64, thread: Option<i64>) -> ThreadKey;
pub fn target(key: &ThreadKey) -> Option<Target>;
pub struct Topic { pub id: i64, pub named_by_user: bool }
pub struct Thread { pub session: SessionId, pub user: u64, pub title: Option<String>, pub topic: Option<Topic> }
pub struct OpenMsg { pub chat: i64, pub thread: Option<i64>, pub message_id: i64 }
pub struct Threads { /* BTreeMap<ThreadKey, Thread>, open: Vec<OpenMsg>, path */ }
impl Threads {
    pub fn load(path: &Path) -> Threads;                      // missing or corrupt → empty
    pub fn save(&self);
    pub fn get(&self, key: &ThreadKey) -> Option<&Thread>;
    pub fn bind(&mut self, key: ThreadKey, thread: Thread);
    pub fn unbind(&mut self, key: &ThreadKey) -> Option<Thread>;
    pub fn by_session(&self, session: SessionId) -> Option<(&ThreadKey, &Thread)>;
    pub fn target_for(&self, session: SessionId) -> Option<Target>;
    pub fn latest_in_chat(&self, chat: i64, user: u64) -> Option<(&ThreadKey, &Thread)>;   // most recently bound/active
    pub fn touch(&mut self, key: &ThreadKey);                  // activity for latest_in_chat
    pub fn set_title(&mut self, key: &ThreadKey, title: String);
    pub fn set_named_by_user(&mut self, key: &ThreadKey, v: bool);
    pub fn open_add(&mut self, m: OpenMsg); pub fn open_remove(&mut self, m: &OpenMsg); pub fn open_take(&mut self) -> Vec<OpenMsg>;
    pub fn list(&self) -> Vec<(String, SessionId)>;
    pub fn unbind_chat(&mut self, chat: i64) -> Vec<Thread>;
}
```

### 2.5 `poll.rs`

```rust
pub struct RecentKeys { /* 1024-entry FIFO set */ } impl RecentKeys { pub fn insert(&mut self, id: i64) -> bool /* true = new */ }
pub struct PollCtx { pub api: Arc<Api>, pub me: BotIdentity, pub tx: mpsc::UnboundedSender<Incoming>, pub rt: Runtime, pub stop: CancellationToken, pub on_state: Arc<dyn Fn(TelegramEvent) + Send + Sync> }
pub async fn run(ctx: PollCtx);                                 // §3.1 loop; returns after the confirming GetUpdates
pub fn backoff_delay(attempt: u32) -> Duration;                 // min(2^min(attempt,6), 60) s
pub fn classify(u: &tgbot::types::Update, me: &BotIdentity) -> Incoming;

pub enum ChatKind { Private, Group, Supergroup { forum: bool }, Channel }
pub struct ChatRef { pub id: i64, pub kind: ChatKind, pub title: Option<String> }
pub struct SenderRef { pub id: u64, pub is_bot: bool, pub name: String, pub username: Option<String> }
pub enum MediaKind { Photo, Document, Voice, VideoNote, Video, Audio, Animation, Sticker }
pub struct Media { pub kind: MediaKind, pub file_id: String, pub file_name: Option<String>, pub mime: Option<String>, pub size: Option<u64>, pub duration_ms: Option<u64>, pub emoji: Option<String> }
pub struct Command { pub name: String /* without slash, lowercase */, pub for_other_bot: bool, pub arg: String }
pub struct ReplyRef { pub message_id: i64, pub from_bot: bool, pub text: Option<String> }
pub enum Service { TopicCreated { name: String, implicit: bool }, TopicEdited { name: Option<String> }, TopicClosed, TopicReopened, Other }
pub struct IncomingMessage {
    pub update_id: i64, pub chat: ChatRef, pub thread: Option<i64>, pub message_id: i64, pub date: i64,
    pub from: Option<SenderRef>, pub text: Option<String> /* text or caption */, pub command: Option<Command>,
    pub mentions_bot: bool, pub reply_to: Option<ReplyRef>, pub forward_from: Option<String>,
    pub media: Vec<Media>, pub media_group_id: Option<String>, pub location: Option<(f64, f64)>, pub contact: Option<String>,
    pub service: Option<Service>,
}
pub struct IncomingCallback { pub update_id: i64, pub id: String, pub from: SenderRef, pub chat: i64, pub message_id: i64, pub thread: Option<i64>, pub data: String }
pub enum Incoming {
    Message(IncomingMessage),
    Callback(IncomingCallback),
    Stopped { update_id: i64, chat: i64, thread: Option<i64>, draft_id: i64 },
    MyChatMember { update_id: i64, chat: i64, left: bool },
    Ignored { update_id: i64, kind: &'static str },
}
```

### 2.6 `worker.rs` (stub in stage 1; RENDER replaces the body, keeps the signatures)

```rust
pub struct RenderConfig { pub tools: ToolsMode, pub reasoning: bool }
pub struct VoiceHandles { pub voice: Option<ActorRef<voice::Voice>>, pub stt: Option<ActorRef<voice::Transcriber>> }
pub struct Row { pub glyph: &'static str, pub name: String, pub state: String, pub detail: String, pub age_secs: u64 }
pub struct WorkerCtx {
    pub key: ThreadKey, pub to: Target, pub user: u64, pub session: SessionId,
    pub api: Arc<dyn Sender>, pub port: Arc<dyn Port>, pub rt: Runtime, pub cfg: RenderConfig,
    pub voice: VoiceHandles, pub topic: Option<Topic>, pub notes: mpsc::UnboundedSender<WorkerNote>,
    pub child_name: Option<String>,    // the subagent's name when this worker renders a child session (§9)
}
pub enum WorkerMsg {
    Agent(AgentEvent),                 // only this session's events
    Mark { message_id: i64, voice: bool },   // the user's message that started/steered the turn (👀 now, terminal mark later)
    Stopped { draft_id: i64 },         // a Stop press for a draft this worker owns
    ChildRow { child: SessionId, row: Row, idle: bool },
    TopicNamedByUser(bool),
    Output { call_id: String, reply: tokio::sync::oneshot::Sender<Option<String>> },   // `out:<call_id>` press: the full tool output, `None` when expired (§8.4); the stub answers `None`
    Shutdown,
}
pub enum WorkerNote {                  // worker → channel
    OpenMsg { msg: OpenMsg, open: bool },           // edit-mode messages in flight (threads.json.open)
    Renamed { key: ThreadKey, title: String },
    Fallback { session: SessionId, reason: String },
    Unreachable { chat: i64, reason: String },
}
pub fn spawn(ctx: WorkerCtx) -> mpsc::UnboundedSender<WorkerMsg>;
```

### 2.7 `commands.rs`, `ui.rs`, `setup.rs` (stubs in stage 1; COMMANDS owns)

```rust
// commands.rs
pub const COMMANDS: &[(&str, &str, &str)];          // name without slash, arg hint, description — the Signal table + start, telegram; minus approve/deny/signal
pub fn owner_only(name: &str) -> bool;
pub fn scopes(owner: Option<i64>) -> Vec<(CommandScope, Vec<BotCommandSpec>)>;   // §10.3 three scopes
pub struct CommandCx<'a> {
    pub user: u64, pub owner: Option<u64>, pub chat: i64, pub thread: Option<i64>, pub in_group: bool,
    pub session: Option<SessionId>, pub status: TelegramStatus, pub port: &'a dyn Port, pub page: usize,
}
pub struct Reply { pub markdown: String, pub keyboard: Option<Keyboard> }
pub enum Outcome {
    Reply(Reply),
    NewThread(Reply),                                 // unbind this thread
    Resume { session: SessionId, title: String, reply: Reply },
    Cancelled { session: SessionId, reply: Reply },
    Allow(u64), RemoveAllowed(u64), AllowGroup(i64), RemoveGroup(i64),   // owner-only; channel applies + replies
    Link,                                              // channel mints a code and replies with link + QR
    Prompt { reply: Reply, expect: Expect },           // force_reply prompt; the next reply to that message is `expect`
}
pub enum Expect { SkillsAdd, McpAdd }
pub async fn handle(cx: CommandCx<'_>, name: &str, arg: &str) -> Outcome;
pub async fn continue_prompt(cx: CommandCx<'_>, expect: Expect, text: &str) -> Outcome;
pub fn help() -> Reply;

// ui.rs
pub async fn action(cx: CommandCx<'_>, data: &str) -> Option<Outcome>;   // `cmd:` presses (data without the prefix)
pub fn guard(md: &str) -> String;                                         // escape bare < and & outside code spans/fences
pub fn table(rows: impl IntoIterator<Item = Vec<String>>) -> String;      // GFM pipe table

// setup.rs
pub trait Terminal { fn prompt(&mut self, q: &str) -> anyhow::Result<String>; fn secret(&mut self, q: &str) -> anyhow::Result<String>; fn say(&mut self, s: &str); }
pub struct Tty; impl Terminal for Tty { … }
pub async fn run(data_dir: &Path, term: &mut dyn Terminal) -> anyhow::Result<()>;   // t --telegram-setup (§10.4)
pub fn deep_link(username: &str, code: &str) -> String;                              // https://t.me/<u>?start=<code>
```

### 2.8 `channel.rs` (skeleton in stage 1; ROUTE owns)

```rust
pub struct Handle { chan: Arc<Channel> }
impl Handle {
    pub fn status(&self) -> TelegramStatus;
    pub async fn link(&self, owner: Option<u64>) -> Result<Option<PairingLink>, String>;   // §4
    pub fn allow(&self, user: u64); pub fn remove_allowed(&self, user: u64);
    pub fn allow_group(&self, chat: i64); pub fn remove_group(&self, chat: i64);
    pub fn set_owner(&self, user: u64);
}
pub fn spawn(cfg: TelegramConfig, token: String, rt: Runtime) -> anyhow::Result<Handle>;
pub(crate) struct Channel { /* api: Arc<Api>, rt, identity: OnceLock<BotIdentity>, owner: Mutex<Option<u64>>, state: Mutex<State { allowed, groups, threads: Threads, workers: HashMap<ThreadKey, UnboundedSender<WorkerMsg>>, recent: RecentKeys, last_event, dropped, pairing: Option<Pairing>, lease: roster::lease::Follower, polling: bool }>, port: Arc<dyn Port>, cfg */ }
// bus: AgentEvent, ApprovalRequested, ApprovalResolved, AskRequested, AskResolved, ArtifactShown, secrets::SecretsEvent, mesh::MeshEvent
//      (crates/slack/src/channel.rs:509-516) → one ordered mpsc<Event> → handle_event
// poll: channel owns the poller task (PollCtx) and starts/stops it on the lease (slack sync_socket shape, channel.rs:671-699)
```

## 3. Rules

- Worktree only; never touch `~/Develop/theuth` (Sky's checkout). Never run `t` against `~/.theuth`.
- No network: no `api.telegram.org`, no token, no bot. Tests use `wiremock` (workspace dep) via `Api::new(token, Some(&mock.uri()))` or `MockSender`.
- Cargo: `PATH=/Users/Sky.Moore/.rustup/toolchains/1.98.1-aarch64-apple-darwin/bin:$PATH` and `CARGO_TARGET_DIR=/Users/Sky.Moore/Develop/theuth-98-target` on every cargo command (one shared target for the wave; waits on the build lock are expected).
- Gates per crate only: `cargo check -p telegram --all-targets`, `cargo test -p telegram`, `cargo clippy -p telegram --all-targets -- -D warnings`, `cargo fmt -p telegram -- --check` (CHECK only; never `cargo fmt --all`). WIRE additionally `cargo check -p rpc --features ts` and `cargo check -p theuth`. Nobody runs `--workspace`.
- `[lints] workspace = true` → `missing_docs = warn` is a gate failure under `-D warnings`: document every pub item.
- Small conventional commits (`feat(telegram): …`), never amend/squash/rebase; if context runs out, commit what compiles and write a `Remaining` section in the as-built note.
- Cross-seam needs: `// NEEDS(<OWNER>): <what>` in code + the as-built note; never edit another seam's file.
- As-built note `docs/design/98-as-built-<TAG>.md`: what was built (file list), judgement calls, gate lines verbatim, NEEDS, Remaining. Final chat line `DONE 98-<TAG>`.

## 4. Deviations (CONTRACT fills in)

Shapes that differ from §2 as landed on `feature/98-telegram`; the code is the reference.

- **`Link` reply is `Result<Option<PairingLink>, String>`** (`lib.rs` actor message and `channel::Handle::link`): `owner: Some(id)` sets the owner live + `write_config_value(config.yaml, "telegram", "owner", id)` and answers `Ok(None)` (no link needed); `None` mints a code, publishes `TelegramEvent::PairingLink` and answers `Ok(Some(link))`; `Err("bot identity unknown yet")` before `getMe` answered. In terminal mode (no channel) `Link { owner: None }` is `Err(...)` — minting needs the bot username, which only the channel learns.
- **`Incoming::Message(Box<IncomingMessage>)`** (clippy `large_enum_variant`, ~420 bytes vs 150). `IncomingMessage` and `Incoming` derive `PartialEq` only, not `Eq` (`location: Option<(f64, f64)>`).
- **`PollCtx` gains `pub recent: Arc<parking_lot::Mutex<RecentKeys>>`**: the process-wide dedup set (§11) is shared between the channel (`Channel.recent`) and the poller instead of living only in `State`.
- **`Api::on_event(&self, Arc<dyn Fn(TelegramEvent) + Send + Sync>)`** added: how `RateLimited` reaches the bus (98 §4.1); `channel::run` sets it once. Also added: `pub fn file_kind(mime, len) -> FileKind` (`Photo | Voice | Document`, the §8.10 choice) and `pub fn is_not_modified(&ApiError)` (the `NOT_MODIFIED_PREFIX` prefix match).
- **Mutexes are `parking_lot::{Mutex, MutexGuard}`** everywhere in the crate (project rule; slack/signal predate it).
- **`threads.rs`**: `target(key: &str)` and every `key: &ThreadKey` parameter take `&str`; `Thread` gains `pub updated_at: i64` (orders `latest_in_chat`, like slack) and `Thread::new(session, user, topic)`; `Topic` is `Copy`; `Threads::open_count() -> u32` added (fills `TelegramStatus.open`). `bind` enforces one session ↔ one thread (an older key bound to the same session is dropped), so `by_session` is a plain `find`.
- **`Pairing.expires` is `tokio::time::Instant`**; `CODE_LEN = 16` is a pub const beside `PAIR_TTL`.
- **`BotIdentity` derives `JsonSchema` + `TS`** (it is a field of `TelegramStatus`).
- **`Handle::allow / remove_allowed / allow_group / remove_group / set_owner` return `()`** as written; the actor in `lib.rs` does the validation (`Allow(owner)` → `Err("… is the owner")`, removing an unlisted id → `Err`), and `set_owner` drops the new owner from the live allowlist.
- **`Port::send_input(session: None, …, scope)`** runs `NewRootSession { scope }` first, then `SendInput { session: Some(root) }` (`agent::SendInput` has no scope field); `Port::steer` mints the `InputId` itself. No signature change.
- **`poll::run`** publishes `Connected` again after an error cleared (not only once per (re)start) so `Status.state` recovers; `Unauthorized` (401) is treated like any other error (`Disconnected` + backoff ≤ 60 s) — "stop polling until restart" is left to ROUTE/WIRE if wanted. `message_thread_id` is taken as a topic only when `is_topic_message == true` or the chat is private (a plain supergroup sets it for reply chains).
- **Boot repair** of `threads.json.open` is `edit_markup(at, None)` per entry (best effort, `not modified` ignored): `OpenMsg` carries no text, so `· interrupted` cannot be appended — `// NEEDS(RENDER)` in `channel::repair_open`.
- **Counters** are per method name only (`calls[method].{ok,err,rate_limited}`); unmatched descriptions are logged at `warn` with code + text, not keyed by `error_code`.
- **Pacer**: drafts pace only `Draft(chat, thread)`; sends/edits pace `Chat(chat)` (3000 ms for `chat < 0`) and edits additionally `Edit(chat, message)`; every call except `getUpdates` goes through the 30/s bucket.
- **`commands::COMMANDS`** starts with `("start", "[code]", "pair with a code, or this list")`; `scopes(owner)` emits `Default`, `AllGroupChats` (9 commands, `is_ephemeral`), and `Chat(owner)` only when `owner` is `Some`.
- **`setup::Terminal::secret`** echoes (slack's rule: tokens are pasted, not typed).
- **Crate deps dropped** as unused in stage 1: `markdown`, `tasks`, `mime_guess` (RENDER/ROUTE add them back); added `parking_lot`, `rustls` (provider install, 21 §2.1), dev `figment`, `serde_yaml`.
