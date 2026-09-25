# 64 — Bots: always-on agents on a computer of their own

Status: design v0.3 · 2026-09-23 (v0.2 the same day: computer = local | node |
container, bundle over node RPC; v0.3: episodes + journal, node-side
`UserSink` outbox, approval wait, push from a node, secret routes) · extends
`32-nodes.md` (a node is the bot's
computer; amends §0.7 "nothing is synced" and §14/§15), `06-acp.md` (a bot on a
node is a *top-level, persistent* ACP session on the parent), `34-schedule.md`
(one front-matter key, `into:`), `37-secrets.md` (§7 "nothing crosses the link"
is replaced by the bundle), `05-oauth.md` (a login may be run *for* another
harness), `21-slack.md` (channel → session binding), `01-config-and-subagents.md`
§1.4 (drop-in directories), `00-architecture.md` §4, §9, §10. `15-server-config.md`
stays shelved: this doc provisions *another machine*; it does not sandbox a
worker on this one. Where this doc disagrees with 32 or 37, this doc wins; §16
lists the sentences.

Inspiration: Grok Bot 0.18 (recon: `scratch-always-on-recon-grok.md`) — named
Bots with a job and compounding memory, one persistent computer per account
shared by all Bots, routines, take-over for sensitive steps, update/reset with
durable state kept apart from compute. What theuth already has: `scratch-always-on-recon-theuth.md`.

**Scope.** Two plugins. `crates/sandbox` gives a bot a *computer*: this
engine, any paired node (32), or a Docker container on this machine for the
user who wants isolation without a second box — and brings the user's
providers, MCP servers and skills to every one of them so nothing is set up
by hand there. It also turns a bare SSH-reachable machine into a node. `crates/bot` runs a named, always-on agent — the
*bot* — on a computer or on the local harness, reachable from every frontend
and chat channel the parent has, with its own memory and the means to hand
knowledge and artifacts up to the parent. A first-class **Bots** area in the
desktop and mobile app manages both.

**Not in v1 (and the seams left for them).** Windows *computers* (the installer
is OS-detecting and `platform::service` already has the Windows branch; the
trait does not care); a display in the Docker computer for `computer_*` tools
(`Capabilities.screen`; the image gains Xvfb + a VNC route when someone needs
it); bot-to-bot messages across computers (bots on one computer are peers
today through 50; cross-computer routing is a parent relay, one method);
per-bot isolation on a shared computer (a computer is one trust domain, as in
Grok Bot; "one bot, one computer" is the isolation knob); knowledge sync
*down* to a computer beyond the optional seed; snapshots on SSH computers
(`Capabilities.snapshot = false`); cloud provisioning of the VM itself
(Hetzner/EC2 backends are further `Sandbox` impls that end in the SSH
installer); Signal channel binding (same shape as Slack, second); bots
learning skills from demonstration (the skills dir is already pushed, so a
skill written on the parent reaches the bot on the next `update`).

## 0. Decisions in one screen

1. **A bot is a file; it lives where its file is.** `<data_dir>/bots/<name>.md`
   — YAML front matter (`computer`, `model`, `effort`, `approvals`,
   `channels`) and a body that is the bot's standing instructions. The same
   `bot` plugin runs on every harness: a file with `computer: local` is one
   durable, pinned, named session on this engine, woken by messages,
   routines and job results; a file with `computer: <name>` is pushed to
   that computer with `computer: local` rewritten, and this harness holds a
   *mirror* of it (decision 3). Hot-reloaded like `schedules/` (34 §2).
   Rejected: rows in `sessions.sqlite` with an `always_on` flag — no file to
   edit, nothing to push, and the schedule precedent shows the file shape
   works; a bot defined only in the parent's UI with the node driven by
   remote calls — the node then knows nothing when the parent is away, which
   is the one moment a bot exists for.
2. **A computer is where a bot executes: this engine, a node, or a
   container on this machine.** Three kinds, one word, one list
   (`computers_list`): `local` — the parent's own engine (63: a session in
   it, not a process); a **node** — any 32 node, hand-paired or set up by
   §4's SSH installer, with as many bots on it as the user likes, so they
   share files, knowledge and workspace and hand work to each other (§6.6);
   a **container** — a Docker container on the *parent's* machine running
   `t --headless`, for the user who wants bots isolated from their own
   account without owning another machine. A container is registered as a
   node internally (paired over the link like any harness) so `nodes`,
   ACP and every UI that shows nodes are unchanged, but it is shown as a
   container. **Docker is never installed on a node and bots are never
   nested in containers on nodes**; the container is an alternative to a
   node, not a layer on one. `crates/sandbox` owns: the `Sandbox` trait
   (lifecycle of compute it created: `docker` via bollard, `ssh` via a
   built-in `russh` client with a private key from the vault or the system
   `ssh`/`scp`); the **bring-up** that gets `t` running and paired on new
   compute (the trait's two primitives, `exec` and `put`: detect OS/arch →
   get `t` → `first_run` → config → start the daemon → read the pairing
   string → `nodes::Add`); and the **bundle** (decision 4), which travels
   over the node's control conn RPC and therefore reaches *every* node,
   including one the user paired by hand yesterday. Rejected: a new
   execution plane where the parent runs tools remotely (15's worker; 32
   §0.3 already rejected proxying, and it would teach every tool about
   remote paths); containers on nodes (two layers of the same thing and a
   Docker dependency on every box); Docker via the `docker` CLI (parsing
   text; the daemon is the API and bollard is the typed client); SSH only
   via the system binary (a Windows or minimal parent may have none — the
   user's ruling); `russh` only (the user's `~/.ssh/config`, agents and jump
   hosts are a real want).
3. **A remote bot is a top-level, persistent ACP session on the parent.**
   06's `AcpChild` already projects an external harness's updates onto
   ordinary `AgentEvent`s, persists input, queues steers, resumes with
   `session/load` and carries permission requests; 32 §0.2 already serves
   the node's ACP half over the link. The bot plugin creates one such
   session per remote bot with `parent: None`, `agent: Some("bot:<name>")`,
   `pinned: true`, `name: "<bot name>"`, and keeps it forever: on
   disconnect it reconnects with backoff and `session/load`; the bot on the
   node keeps working meanwhile because the session is the node's own. Every
   parent feature that takes a session id then works for a bot with no new
   code: the sidebar, the chat pane, approvals, `message_user`, Slack
   threads (`threads.json`), `send_message peer:<name>` (50),
   `list_artifacts`. Rejected: mirroring `AgentEvent`s with rewritten ids
   (32 §0.2's rejection stands: a second child kind and a foreign id space);
   a bot reachable only by connecting the device to the node directly (32
   §0.3 — still allowed, but Slack, `send_message` and the parent's UI
   would not see it).
4. **Providers travel in the bundle, over the node's own RPC; OAuth is
   logged in *for* the computer, from the parent.** Once a node is online
   the parent pushes, through the control conn (`nodes::client`, the
   methods the node already serves: `set_secret`, `add_mcp_server`,
   `add_skill`, plus a new `oauth_import`): every vault secret (or the
   `bundle.secrets` allowlist); every MCP server file and `mcp/config.yaml`;
   `skills/`. This works for a container, an SSH-installed node and a node
   the user paired by hand — the bundle does not care how the node came to
   exist. OAuth token files are **not copied**: the parent runs the
   provider's own flow (05 §2.1 — the browser and the loopback port are on
   the parent) a second time with the result staged for the target
   computer, delivered with `oauth_import { ns, json }` (written 0600 on
   the node), and the staging copy deleted. Each computer then holds its
   own grant and refreshes independently — no two harnesses rotating one
   refresh token. The same applies to remote MCP logins
   (`oauth/mcp/<server>.json`). Values never enter a model context: the
   push is plugin code reading the vault through `secrets::Handle`, the
   same in-process read genai's auth resolver makes (37 §3.6), and the
   conn is Noise-encrypted end to end (31). Rejected: pushing through
   `exec t --secret-set` on the setup channel (needs an SSH/Docker handle,
   so a hand-paired node could never receive providers); copying
   `oauth/*.json` (refresh-token rotation logs one side out — OpenAI's
   Codex grant rotates); the parent as a 37 §11 `Backend` for the node over
   the link (the bot stops when the laptop closes, which defeats the
   point); making the user log in on the node (the user's ruling: it should
   just work).
5. **Memory is the computer's; sharing up is a tool and an outbox.** A bot's
   durable memory is its session (compaction, 36) plus the knowledge bundle
   of the harness it runs on — shared with the other bots on that computer,
   as Grok Bot shares the computer. It is not synced with the parent. A bot
   calls `bot_share { kind: knowledge|artifact, id, note }` when *it* judges
   something belongs to the user; the item goes into an outbox on its
   harness; the parent pulls it over the node's control conn (`knowledge_get`
   / `get_attachment`), files the concept into its own bundle with
   `source: bot:<name>` and a `bot:<name>` tag, or records the artifact under
   the mirror session and publishes `ArtifactShown` there — so it lands in
   the bot's transcript, the Bots screen and the bound Slack thread. Pull,
   not push: 32 §0.1 rejected requests flowing node→parent on the link conn
   and nothing here needs them. Rejected: two-way knowledge sync (a merge
   problem nobody asked for); a bot writing straight into the parent bundle
   (no provenance, no place to review).
6. **Routines are schedules with `into:`.** `34` gains one front-matter key,
   `into: <peer name>`: the wrapped prompt is delivered as input to the
   session that owns that name (a bot's session is named `bot:<name>`) instead
   of a fresh session. `schedule` keeps depending only on `agent`; `bot`
   depends on nothing in `schedule`. A bot's routines live in the
   `schedules/` directory of *its* harness, so they fire while the parent is
   away; the Bots UI edits them through the node's RPC. Rejected: a
   `routines:` list inside the bot file (a second cron parser, a second run
   store, a second status view); `schedule` resolving `bot:` by asking the
   bot actor (a dependency cycle).
7. **Crates `sandbox` and `bot`; the user-facing words are Computer and Bot.**
   `crates/computer` already exists (27, screenshot and input), so the
   adapter crate is named for what it does — it sandboxes execution on
   another machine. Both sit below `rpc`: `… ← link ← nodes ← sandbox ←
   bot ← rpc` (`bot` also depends on `agent`, `acp`, `knowledge`, `artifact`,
   `slack`). Rejected: `crates/computer` (taken); `crates/agents` (one letter
   from the loop crate).
8. **Compute and durable state are separate, and the trait says which is
   which.** `Capabilities.durable_roots` names the paths that survive
   `reset { preserve_data: true }` (Docker: the `data` and `workspace`
   volumes; SSH: `<data_dir>` and `~/workspace`); `snapshot`/`restore` exist
   on the trait and are `Unsupported` on SSH in v1. `update` re-runs the
   installer idempotently after asking the computer's bots to pause at a turn
   boundary. Rejected: `docker commit` as a snapshot (volumes are not in
   it); equating "delete container", "reprovision machine" and "roll back"
   (Grok Bot's docs keep them apart for a reason).
9. **Readiness, not process existence.** A computer is `ready` only after:
   the daemon answers `info` over the control conn with a version the parent
   accepts, the node is `online`, a probe turn on the node (`spawn_subagent`
   with a one-line task) returns, and — when a bot is placed on it — the
   bot's ACP session opens. Health afterwards is the 32 presence bit plus a
   periodic `exec` probe (`df`, uptime, `t` version) recorded on the
   computer row. Rejected: trusting `create` (the container is up long before
   the relay registration completes).
10. **A bot is a continuous conversation over episodic sessions, with
    explicit memory.** One long session dies slowly (summaries of
    summaries, a fat baseline, "what did we decide in March" gone). A bot
    instead works in **episodes** — one session per unit of work: a Slack
    thread, a routine run, a conversation the user starts, a hand-off from a
    teammate — one active at a time, inputs queued like a person's. An
    episode closes when its thread ends, it idles past
    `bot.episode_idle_close_secs`, or it reaches `bot.episode_max_tokens`.
    Closing writes the bot's **journal** (a bot-owned markdown document in
    36's compaction shape — *Done / Open / Next / Notes* — kept under
    `bot.journal_max_bytes`) and runs the detached harvest (10 §5) so durable
    facts land in the computer's knowledge bundle. Every episode starts with
    three baseline parts beside the harness prompt: the instructions (the
    user's), the journal (the bot's), knowledge hints (the computer's,
    shared with teammates); old episodes are searchable (49). The user sees
    one continuous conversation with a quiet divider where an episode
    closed. Rejected: one session forever with compaction (the failure mode
    above); a fresh session per input with no journal (a bot that forgets
    what it was doing at lunch); a vector memory store (10 §1 already
    rejected it; the knowledge bundle plus hints is the memory).
11. **On a computer, the bot plugin is the user.** A node has no Slack, no
    Signal, no desktop of its own, so the `bot` plugin registers itself as
    the node's `UserSink` (24 §2's `RegisterUserSink`): every `message_user`
    text, every artifact a bot `show`s, every routine result, every
    `secret_request` and every parked approval becomes one durable **outbox**
    item, and the parent — which *is* where the user is — pulls and delivers
    each through its own channels, thread and cards. Artifacts therefore
    flow up automatically (a bot's `show` is for the user by definition);
    `bot_share` remains for knowledge, where judgement is the point.
    Rejected: per-kind plumbing (a share tool for artifacts, a message
    relay, a secrets relay — four mechanisms for "give this to the user");
    running the parent's Slack/Signal on the node (second app, split DM).
12. **A bot survives the parent being away, and can still reach the phone.**
    Everything a bot needs is on its computer; everything for the user waits
    in the outbox. Two things change for bot sessions: an approval waits
    `bot.approval_wait_secs` (hours, not the gate's default) and is re-sent
    to the parent on reconnect, with the bot told to park the task rather
    than fail it; and urgent items (`Approval needed`, `Response ready`) may
    wake the user's phone directly through 42's push — the phone is a device
    of every node (32 §0.3), so the node's `link` sends the `PUSH` frame
    itself, parent or no parent. Rejected: the gate's timeout → deny for
    bots (an unattended bot would fail every risky step overnight); a
    node-side chat channel (decision 11).

## 1. Why

The user wants teammates, not tasks. A *session* is a conversation; a *bot*
is a colleague who has many conversations — a name and a job, a place it
works, standing instructions you can edit, memory that compounds, routines,
teammates it hands work to, and the habit of coming back when it needs a
decision. It keeps working while the laptop is shut and answers in Slack.
That sentence is the product: it belongs in §1, in the empty state of the
Bots screen (§8) and in the store copy, and the UI never says "session"
about a bot. Grok Bot
sells exactly this, on a cloud computer the user cannot see into. theuth is
one step short of it on the user's own machines:

- 32 gives the model another machine — but only one somebody already set up
  by hand: install `t`, `--install-service`, `--secret-set` per provider
  over ssh, paste a key into the parent. There is no Docker path for
  testing, no SSH path that does the setup, and "nothing is synced" means a
  new box starts with no providers, no MCP servers and no skills.
- Sessions are durable and schedules wake them, but nothing is *always on*:
  `pinned` is retention (51 §3), a schedule run is a fresh session, a
  subagent finishes. There is no record that says "this is Ops, it lives on
  build-A, its instructions are these, its routines are those".
- A node's sessions are reachable from a device that pairs with the node, not
  from the parent's Slack, `send_message`, sidebar or approvals.

The pieces are all present — nodes, ACP over the link, the vault, OAuth
flows, schedules, knowledge, artifacts — and this doc is the glue and the two
new records: the computer and the bot.

## 2. Facts relied on (recon 2026-09-23; `path:line` in this repo)

Nodes and link.
- `nodes::Node {id,name,relay,platform,version,paired_at_ms,last_seen_ms,online,revoked,children}`; id = 43-char base64url harness public key — `crates/nodes/src/types.rs:26-50`. Commands `Status | Add{pairing,name} | Remove | Rename | Invite`; `Add` consumes the string `t --install-service`/`t --link` prints — `:81-118`. Events `NodeAdded|NodeOnline|NodeRemoved|NodeRevoked` — `:52-79`.
- One control conn task per node; `Live { cancel, online, children, rpc: Option<Arc<Rpc>> }` — `crates/nodes/src/lib.rs:85-92`; presence 10 s, revoke 3 s, keepalive 300 s, backoff 1–30 s — `:49-58`.
- The control conn is a jsonrpsee client over a mode-0 channel; every method it may call is listed in `client::METHODS = ["info","link_pair","link_revoke_device","link_status"]` and a test checks each against `bindings/methods.json` — `crates/nodes/src/client.rs:15-17`; 30 s per request — `:20`.
- On connect `nodes` sends `agent::RegisterNode { name, describe, dialer }`; `spawn_subagent{agent:<node>}` resolves through it; name collisions with `agents.yaml` are rejected — `crates/nodes/src/lib.rs:129-170,388-417`; `crates/agent/src/supervisor.rs:5452-5460`.
- `link.relay` default `wss://relay.theuth.io`; `--install-service --relay` overrides — `crates/link/src/types.rs:19-33`; `crates/theuth/src/main.rs:82-84`. Pairing string shape `https://<relay-host>/link#1.<b64url(pub‖secret)>.<b64url(name)>` — `crates/link/src/types.rs:63`.
- 32 §0.7: "Nothing is synced … the user runs `t --secret-set anthropic` over the same ssh session" — `docs/design/32-nodes.md` §0.7; §15 defers credential/skill sync to "a separate doc when someone has three nodes". This is that doc.

ACP.
- `acp::harness::Transport` abstracts subprocess stdio vs an injected `Dialer`; `Harness::spawn/open/connect` initialise, create or **load** a session, stream updates — `crates/acp/src/harness.rs:36-46,198-282,303-404`.
- `agent::AcpChild` persists input, queues/withdraws steers, projects tool/plan updates to ordinary events, cancels with 5 s grace, stores the remote session id and passes it to `session/load` on resume — `crates/agent/src/acp_child.rs:50-54,147-189,839-891`.
- `agents.yaml` schema is `command, args, env, description` only; `cwd` travels per spawn in `session/new` — `crates/acp/src/agents.rs:17-33`; `docs/design/06-acp.md:135-173`.

Sessions.
- `SessionSummary` row: ids, times, title, tags, model, `parent`, `name`, effort, `agent: Option<String>`, `cwd`, `pinned`, `archived`, `project`, `retention` — `crates/agent/src/lib.rs:1204`; `AgentInfo` (live state, `last`) — `:691`; `AgentState = Idle|Running|Awaiting|Approval|Error|Cancelled` — `:860`; no always-on field exists.
- `sessions.sqlite`, WAL, four connections, embedded migrations — `crates/agent/src/store.rs:183-200`; cold durable sessions resume lazily on `Agents::input → route → tell_input` — `crates/agent/src/supervisor.rs:700-839`; schedule wrappers, job results, child completion and peer messages all enter there — `:1540-1650`.
- Peer names resolve in `crates/agent/src/peers.rs:61-90,141-228`.

Schedules.
- Front matter: exactly one of `cron|every|at`, plus `enabled, model, cwd, quiet, approvals, until, max_runs`; body = prompt — `crates/schedule/src/file.rs:14-77,96-184`. A fire creates or reuses a session and injects `<schedule name= at=>…</schedule>` — `crates/schedule/src/lib.rs:500-677`.

Secrets and OAuth.
- Vault = XChaCha20-Poly1305 blob, key in the OS keychain or `<data_dir>/secrets.key` 0600 when none is reachable (`secrets.key_store`) — `docs/design/37-secrets.md` §0.6. `t --secret-set NAME` is the headless setter — 37 §0.8. Placeholders expand in `crates/tools/src/secrets.rs:1-178`, immediately before invocation; echoed values are scrubbed.
- 37 §7: "Nodes: … Nothing crosses the link." Replaced by §7 of this doc.
- Provider flows: `openai` PKCE port 1455, `anthropic` PKCE port 54545, `cursor` deep-link + poll, `xai` device grant; tokens at `<data_dir>/oauth/<ns>.json` 0600; precedence vault key → oauth file → env — `docs/design/05-oauth.md` §1, §2.1, §2.2. MCP tokens at `oauth/mcp/<server>.json` — 05 §3.

Tools and locality.
- `Tool` and `ToolCx { session, turn, cwd, … }` — `crates/tools/src/lib.rs:280-326`; every native tool calls host APIs directly (`fs.rs`, `shell.rs`, `pty.rs`, `crates/browser`, `crates/computer`, `crates/agent/src/repl.rs`); there is no fs/exec indirection. This doc adds none.
- `RegisterPrompt { plugin, text }` gives a plugin one static prompt paragraph — `docs/design/00-architecture.md:232-250`.

CLI, service, first run, distribution.
- Flags: `--headless`, `--serve`, `--acp`, link and node one-shots, `--install-service`/`--uninstall-service`, `--secret-set`, `--update`, hidden `subprocess` — `crates/theuth/src/main.rs:36-135`. `first_run` creates `config.yaml`, `acp/agents.yaml`, `mcp/`, `skills/`, `logs/` — `:385-437`.
- `platform::service`: macOS LaunchAgent, Linux `~/.config/systemd/user/theuth.service`, Windows per-user scheduled task (unverified); unit runs `$SHELL -l -i -c "exec … t --headless --data-dir …"`, `THEUTH_LAUNCH=1`, restart after 2 s — `crates/platform/src/service.rs:1-14,43-109,172-232`.
- Linux assets `theuth_<version>_linux-{x86_64,aarch64}.tar.gz`, manifest keys `cli-linux-*`; glibc, no musl; the Linux floor is glibc 2.39 (Ubuntu 24.04) — `docs/design/39-update.md:189-209`; manifest keys tested in `crates/update/src/lib.rs:670-690`; `crates/platform/src/target.rs:72-90`.
- Docker in-repo is packaging only: `deploy/relay/Dockerfile`, `deploy/update/Dockerfile`, Linux release builds in Docker — `justfile:103-112`. No Dockerfile makes an agent host; no `russh`, `bollard`, `openssh` in `Cargo.lock`.

Knowledge, artifacts, Slack, UI.
- Knowledge is an OKF bundle, one concept per `<id>.md` with frontmatter (`type,title,description,tags,scope,source,timestamp`), every write a `gix` commit — `docs/design/10-knowledge.md` §0.1, §2.
- An artifact is `<data_dir>/artifacts/<sha256[..16]>.<ext>` + a row `{call_id, session, name, title, kind, bytes, version}` in `artifacts.sqlite`; the live signal is `ArtifactShown { call_id, artifact }`; phones fetch through `get_attachment` — `docs/design/35-artifacts.md` §0.2–0.3.
- Slack binds a thread to a session in `threads.json` and every reply goes in that thread; approvals for a bound session go to that thread — `docs/design/21-slack.md` §6, §11, §12.
- Desktop left rail: New chat, Knowledge, Notes, Artifacts, Meetings (conditional), then `SessionList`, then Settings — `crates/desktop/ui/src/components/sidebar/AppSidebar.tsx:95-160`. Node settings: `components/settings/Nodes.tsx`; pairing/event projection `src/link/nodes.ts`.
- Runtime is 357 of 400 lines — `crates/runtime/src/lib.rs`. Plugin vector order ends `… routes, link, nodes, schedule, update, signal, slack, rpc, tui` — `crates/theuth/src/main.rs:726-795`.

Grok Bot (recon sheet, `scratch-always-on-recon-grok.md` §C, §G, §H).
- Connector seam: `connect() → GatewayConnection`, optional `recreate({preserveData, force})`, `forceRecreate()`, credential minting; readiness and ports are private to each adapter.
- Local Docker adapter: two named volumes (`/workspace`, data), loopback-only ports, content-addressed runtime mounted read-only, labels identify ownership/schema/hash and a mismatch causes `rm --force` + clean create; auth directories mounted read-only into the container.
- Bots share files, cookies, credentials on one computer; each has a screen lease; `reset` restores a snapshot and can lose recent work; `update` keeps files; admin recreate waits for bots to pause.

## 3. Shape

Three kinds of computer, one bot model:

```
                 ┌─ local ─────────── this engine; a bot = one pinned session here
 computer  ──────┼─ node ──────────── another machine running t --headless, paired (32);
                 │                    hand-paired, or brought up by the ssh installer;
                 │                    N bots share it and hand work to each other
                 └─ container ─────── docker on THIS machine running t --headless,
                                      paired like a node; isolation without a second box
```

```
parent (~/.theuth)                                   computer "ops-1" (node or container)
┌──────────────────────────────────────┐   relay    ┌──────────────────────────────┐
│ bots/ops.md   (computer: ops-1)      │◄──────────►│ t --headless                 │
│ bots/triage.md(computer: ops-1)      │   link     │  bots/ops.md, bots/triage.md │
│ bots/scout.md (computer: local) ─┐   │            │   └─ sessions bot:ops,bot:triage│
│                                  ▼   │   mode 0   │      (peers: hand-off via 50) │
│ bot plugin                           │── control ─┼─► bot plugin (same crate)     │
│  ├─ local bot = pinned session       │   RPC      │  schedules/*.md  into: bot:…  │
│  ├─ mirror sessions (AcpChild) ──────┼── mode 1 ──┼─► rpc::acp  session/load      │
│  └─ outbox puller ───────────────────┼── pull ────┼─► outbox (bot_share)          │
│ sandbox plugin                       │            │  knowledge/  (own bundle)     │
│  ├─ bundle → set_secret, add_mcp_…, ─┼── RPC ─────┼─► secrets.vault, mcp/, skills/│
│  │   add_skill, oauth_import          │            │  oauth/<ns>.json (proxy login)│
│  ├─ bring-up: Sandbox docker | ssh ──┼─ exec/put ─┼─► t installed, paired (new    │
│  └─ computers/*.yaml, snapshots/     │  (once)    │      compute only)            │
│ slack: threads.json → mirror session │            └──────────────────────────────┘
│ desktop/mobile: Bots  ·  TUI: /bots  │
└──────────────────────────────────────┘
```

Three records, three owners:

| Record | File | Owner | Edited by |
|---|---|---|---|
| bot | `<data_dir>/bots/<name>.md` | `bot` on the harness it names as `local`; the parent keeps the authoring copy | user (UI form or editor); parent pushes on save |
| computer | `<data_dir>/computers/<name>.yaml` — only for compute `sandbox` created (containers, ssh-installed nodes); hand-paired nodes and `local` are rows synthesised from `nodes` | `sandbox` on the parent | the plugin; the UI drives it |
| routine | `<data_dir>/schedules/<file>.md` with `into: bot:<name>` | `schedule` on the bot's harness | user, through the Bots UI (parent) or directly (node) |

### 3.1 Where a bot's state is

| State | Local bot | Remote bot |
|---|---|---|
| episodes (sessions, `bot = stem`) | parent `sessions.sqlite` | computer's `sessions.sqlite`; the mirror row on the parent holds the projection `AcpChild` already persists, across episodes |
| journal | parent `bot/journal/<stem>.md` | computer's `bot/journal/<stem>.md`; readable in the Bots screen over the control conn |
| knowledge | parent bundle | computer's bundle (shared by its bots); shared items copied up with provenance |
| artifacts | parent `artifacts/` | computer's `artifacts/`; shared ones copied up under the mirror session |
| routines | parent `schedules/` | computer's `schedules/` |
| secrets, MCP, skills | parent's | pushed copies; refreshed on `update` |
| files | parent cwd (`bots.workspace`, default `~/bots/<name>`) | `/workspace/<name>` (Docker) or `~/workspace/<name>` (SSH) |

## 4. `crates/sandbox`

### 4.1 Trait

```rust
// crates/sandbox/src/lib.rs
#[async_trait::async_trait]
pub trait Sandbox: Send + Sync {
    fn kind(&self) -> &'static str;                       // "docker" | "ssh"
    /// Create the compute (container) or verify reachability (host). Idempotent:
    /// an existing compute with our labels/record is adopted, a mismatched one replaced.
    async fn create(&self, spec: &ComputerSpec, out: Progress) -> Result<Handle, Error>;
    async fn status(&self, h: &Handle) -> Result<ComputeStatus, Error>; // Running | Stopped | Gone
    async fn start(&self, h: &Handle) -> Result<(), Error>;
    async fn stop(&self, h: &Handle, grace: Duration) -> Result<(), Error>;
    /// The two primitives the installer is written against.
    async fn exec(&self, h: &Handle, cmd: Cmd) -> Result<ExecOutput, Error>; // argv, env, stdin bytes, timeout, cwd
    async fn put(&self, h: &Handle, path: &str, bytes: Bytes, mode: u32) -> Result<(), Error>;
    /// What this compute can do; the installer and the UI plan against it.
    async fn capabilities(&self, h: &Handle) -> Result<Capabilities, Error>;
    /// Compute replacement. `preserve_data` keeps `Capabilities.durable_roots`.
    async fn recreate(&self, h: &Handle, preserve_data: bool, out: Progress) -> Result<Handle, Error>;
    async fn destroy(&self, h: &Handle, preserve_data: bool) -> Result<(), Error>;
    /// Unsupported on ssh in v1 (Capabilities.snapshot = false).
    async fn snapshot(&self, h: &Handle, label: &str, out: Progress) -> Result<SnapshotId, Error>;
    async fn restore(&self, h: &Handle, id: &SnapshotId, out: Progress) -> Result<(), Error>;
}

pub struct Capabilities {
    pub os: String, pub arch: String,        // "linux"/"x86_64" — from `uname` or the image
    pub glibc: Option<String>,               // installer refuses < 2.39
    pub durable_roots: Vec<String>,          // survive recreate{preserve_data:true}
    pub snapshot: bool,
    pub screen: bool,                        // a display for computer_* (false in v1 images)
    pub tools: BTreeMap<String, bool>,       // curl git python3 node npx uv rg docker — for MCP stdio servers
    pub daemon: DaemonKind,                  // Entrypoint (docker) | Service (systemd/launchd) | Manual
}
```

`Progress` is an `mpsc::Sender<ProgressLine { step, line, pct }>`; the actor
publishes each line as `ComputerEvent::Progress` so the UI streams the setup
log. `Handle` is backend-opaque (`container_id` / `host,port,user,host_key`),
serialised into the computer record.

Why a trait with `exec`/`put` and not "a remote shell": bring-up is six
steps of run-and-check on compute that has no theuth yet; two primitives keep both backends thin (bollard
`create_exec`/`start_exec` + `upload_to_container` tar; russh `channel_open_session`/`exec`
+ `russh-sftp` write, or `ssh`/`scp` subprocesses). Rejected: an
`ssh`-shaped API on the Docker backend (nothing needs a PTY here; 55's
interactive terminals to a computer are a later doc).

### 4.2 Backends

**docker.** Crate `bollard` (Docker Engine API, hyper, maintained, used by
testcontainers-rs). Socket discovery: `DOCKER_HOST`, then
`~/.docker/run/docker.sock` (Docker Desktop on macOS), then
`/var/run/docker.sock`; Podman's compatible socket works unchanged. Per
computer: image (§4.3), container `theuth-computer-<name>`, labels
`io.theuth.computer=<name>`, `io.theuth.owner=<parent harness id>`,
`io.theuth.schema=1`, `io.theuth.image=<digest>`; named volumes
`theuth-<name>-data:/data` and `theuth-<name>-workspace:/workspace`;
`restart: unless-stopped`; **no published ports** — the daemon reaches the
relay outbound, the parent reaches it over the relay like any node; env
`THEUTH_DATA_DIR=/data`, `THEUTH_LAUNCH=1`; CPU/memory limits from the spec.
A container with our labels but a different `owner`/`schema`/`image` is
removed with force and recreated (data volumes kept), following the Grok
Bot adapter. `DaemonKind::Entrypoint`: the container command *is*
`t --headless --data-dir /data`, so "start on boot" is the restart policy and
`install-service` is skipped. `snapshot` = stop → `tar` both volumes through a
helper container → `<data_dir>/sandbox/snapshots/<name>/<ts>.tar.zst` → start;
`restore` reverses it. Keeps `sandbox.snapshots_keep` (3).

Loopback note: parent and container are on one machine, and every byte still
goes through `wss://relay.theuth.io`. Acceptable for v1 (this is the testing
backend); the seam for a direct dial is `link`'s conn, not this crate. The
compose file in `deploy/computer/` also runs `theuth-relay` locally for an
offline test rig (`link.relay: ws://host.docker.internal:8787`).

**ssh.** Two transports behind one `SshTransport` enum, chosen per computer:

- `builtin` (default): `russh` + `russh-sftp` (pure Rust, tokio, maintained;
  in use by warp/zellij-family tools). Auth: a private key held in the vault
  under the name the record gives (`auth: { key: computer-ops-1-key }`,
  entered through the masked `secret_request` field or the UI's key drop
  zone — never the chat), optional passphrase as a second secret; password
  auth is accepted but the UI warns. Host key: trust-on-first-use, pinned in
  the record (`host_key`), mismatch is a hard error with a "forget host
  key" action. Jump hosts: `proxy_jump: host` (one hop, v1).
- `system`: `ssh`/`scp` subprocesses with `-o BatchMode=yes` and the user's
  `~/.ssh/config`, agent and `ProxyJump` as-is; `Handle` carries only the
  alias. Present so the user who already has a working `ssh box` keeps it.

Requirements on the target: sshd, a POSIX shell, `curl` or `wget`, `tar`,
and a login shell for the user (the service unit uses it, 18). `DaemonKind`
is `Service` when `systemctl --user` (needs `loginctl enable-linger`, which
the installer runs and, failing that, reports) or `launchctl` exists,
otherwise `Manual` and the installer prints the ExecStart line into the
record and the UI (58's non-systemd behaviour: an ExecStart instruction).

### 4.3 The image (`deploy/computer/`)

`Dockerfile` — `ubuntu:24.04` (glibc 2.39 floor), `curl git ca-certificates
python3 ripgrep tar zstd` + Node LTS and `uv` (for stdio MCP servers), a
non-root user `bot` with `/data` and `/workspace` owned; build arg
`THEUTH_VERSION` and `TARGETARCH` select `theuth_<version>_linux-<arch>.tar.gz`
from the GitHub release, verified against the signed 39 manifest with the
public key baked into the image; `ENTRYPOINT ["t","--headless","--data-dir","/data"]`.
Published as `ghcr.io/basedatum/theuth-computer:<version>` by the release
recipe; `just computer-image` builds it locally. **Development path:**
`image: build` with `binary: /path/to/t` (a Linux `t` from build-A or from
`just release-linux`) copies that file into the image instead of downloading;
`image: <ref>` uses any pre-built ref. The parent refuses an image whose `t`
version is older than its own minus one minor (this doc's window; 39 defines none).
No display, no browser binary: `browser_*` is pure Rust (23) and works;
`computer_*` reports `screen: false` and the bot is told so in its prompt block.

### 4.4 Bring-up and bundle

Two phases, deliberately separate. **Bring-up** exists only for compute the
plugin creates (a container, an SSH host) and is the only place `exec`/`put`
are used; it ends when the harness is a paired, online node. **Bundle** runs
over the node's control conn RPC and is the same for every node — one the
bring-up just made, one the user paired by hand, one that has been around
since 32. Both are state machines whose steps are idempotent and recorded on
the computer row (`setup: { step, at, error }`) so `update` resumes and the
UI shows where it is.

**Bring-up** (`sandbox::bringup::run(sb, handle, spec, out)`):

1. `probe` — `exec uname -sm`, `ldd --version`, `command -v` for each tool →
   `Capabilities`; refuse unsupported OS/arch/glibc with the reason.
2. `binary` — `exec t --version`; missing or older → download the release
   asset for the triple on the target (`curl`), verify against the manifest
   the parent fetched (39), untar to `~/.local/bin` (ssh) or skip (docker:
   already in the image); `put` a local binary when `spec.binary` is set.
3. `first_run` — `exec t --data-dir <dd> --first-run` (a new hidden one-shot
   that runs `first_run` and exits; today it happens on first start).
4. `config` — `put` a `config.local.yaml` (layered over the node's own
   `config.yaml`, §14): `link.enabled: true`, `link.relay`, `link.name:
   <computer>`, `secrets.key_store: file`, `signal.enabled: false`,
   `slack.enabled: false`, `rpc.listen` absent (loopback ephemeral, 63),
   `tools.policy` = the parent's.
5. `daemon` — `Entrypoint`: nothing; `Service`: `exec t --install-service
   --relay <relay>` and capture stdout; `Manual`: `exec nohup t --headless …`
   plus the recorded instruction.
6. `pair` — parse the pairing string from step 5's stdout (or `exec t --link`
   when the daemon was already running), `nodes::Add { pairing, name }`, wait
   `NodeOnline` (10 s presence, 3 tries).

**Bundle** (`sandbox::bundle::run(node, bundle, out)`, RPC over the control
conn; `nodes::client::METHODS` grows accordingly):

7. `secrets` — `set_secret { name, value, note }` for each name in the
   bundle (value read through `secrets::Handle`, zeroized after; 37 keeps
   five versions on the node too, so a re-push is a rollback point).
8. `oauth` — `oauth_import { ns, json }` (new node method: writes
   `oauth/<ns>.json` or `oauth/mcp/<server>.json` 0600 and reloads the
   store) for each staged login (§5); the staging file is deleted after.
9. `mcp`, `skills` — `add_mcp_server` for each server file (minus
   `repos/`, which the node clones itself with `add_mcp_repo`),
   `set_mcp_gate`/`set_mcp_hidden` for `mcp/config.yaml` entries, `add_skill`
   for each skill (or `add_skill_repo` for a cloned repo). MCP servers whose
   `command` is not in `Capabilities.tools` are recorded as `unavailable`
   and shown in the UI.
10. `knowledge` (optional, `seed: true`) — `knowledge_put` for each concept
    matching `seed_tags` (history is not carried; the node's bundle starts a
    new `gix` history with `source: seed`).
11. `bots` — `bot_put { name, front_matter, body }` (new node method, §6.7)
    for each bot placed on this computer, `computer` rewritten to `local`;
    `schedule_put { file, text }` (new) for their routines.
12. `verify` — `info` over the control conn (version, platform, provider
    states), one probe `spawn_subagent{agent:<computer>}` turn ("reply with
    the output of `uname -a`"), and for each placed bot the ACP
    `session/load` succeeds → `ready`.

Every step has a timeout (`sandbox.step_timeout_secs`, 600) and a one-line
failure reason. `update` re-runs bring-up steps 2 and 4 (only for compute
the plugin owns) and the whole bundle; the rest verify. Before
`update`/`recreate` the actor asks `bot` to pause the computer's bots
(`bot::Pause { computer }` → each bot finishes its current turn, further
input queues), and resumes them after `verify`. A hand-paired node gets
`update` = bundle only, and its `t` is the node's own updater's business
(39).

### 4.5 Actor, commands, events, RPC

`SANDBOX_ACTOR` — `Create { spec } → Result<Computer>`, `Setup { name } →
()` (runs the installer; progress by event), `Update { name }`, `Recreate {
name, preserve_data }`, `Snapshot { name, label }`, `Restore { name, id }`,
`Destroy { name, preserve_data }`, `Status → Vec<Computer>`, `Login { name,
provider } → ()` (§5), `Providers { name } → Vec<ProviderState>`.

```rust
pub struct Computer {
    pub name: String, pub backend: String,           // "docker" | "ssh" | "local"
    pub state: ComputerState,                        // Creating | SettingUp{step} | Ready | Degraded{why} | Offline | Failed{step, error} | Destroyed
    pub node_id: Option<String>,                     // nodes::Node.id once paired
    pub version: Option<String>, pub platform: Option<String>,
    pub capabilities: Option<Capabilities>,
    pub health: Option<Health>,                      // disk_free_bytes, uptime_secs, load, checked_at
    pub bots: Vec<String>, pub providers: Vec<ProviderState>,
    pub snapshots: Vec<SnapshotInfo>, pub created_at_ms: u64, pub updated_at_ms: u64,
}
pub struct ProviderState { pub ns: String, pub kind: ProviderKind /* Key | OAuth | Mcp */, pub state: Pushed | NeedsLogin | Missing | Failed(String), pub at_ms: u64 }
#[serde(tag = "type")]
pub enum ComputerEvent { Changed { computer: Computer }, Progress { name: String, step: String, line: String, pct: u8 }, Removed { name: String } }
```

`local` is a pseudo-computer row the actor synthesises for the parent itself
(never created or destroyed) so the UI and the bot file can name it uniformly.

RPC (`rpc::Api`, generated TS bindings as usual): `computers_list`,
`computer_create(spec)`, `computer_setup(name)`, `computer_update(name)`,
`computer_recreate(name, preserve_data)`, `computer_snapshot(name, label)`,
`computer_restore(name, id)`, `computer_destroy(name, preserve_data)`,
`computer_login(name, provider)`, `computer_providers(name)`,
`computer_capabilities(name)`; event `ComputerEvent` through `rpc::bridge`.
The control-conn method list (`nodes::client::METHODS`) grows by what the
bundle (§4.4), §6.4–6.5 and the Bots UI need on a node: existing `set_secret`,
`add_mcp_server`, `add_mcp_repo`, `set_mcp_gate`, `set_mcp_hidden`,
`add_skill`, `add_skill_repo`, `knowledge_put`, `knowledge_get`,
`knowledge_list`, `knowledge_search`, `list_sessions`, `send_input`,
`list_schedules`, `schedule_history`, `get_attachment`, `list_artifacts`;
new `oauth_import`, `bot_put`, `schedule_put`, `schedule_delete`,
`bot_outbox`, `bot_outbox_ack` — each added to the checked constant.

### 4.6 Tools

The model may inspect and, for testing, drive computers; creation of an SSH
computer needs a key and stays a UI act.

| Tool | Risk | Args | Returns |
|---|---|---|---|
| `computers` | ReadOnly | — | `Vec<Computer>` (health, providers, bots) |
| `computer_create` | Mutating | `{ name, backend: "docker", image?, cpus?, memory_mb?, bundle? }` | the `Computer`; runs `Setup` and streams progress as events; `backend: "ssh"` → error "create SSH computers from the app" |
| `computer_update` | Mutating | `{ name }` | setup result |
| `computer_recreate` | Destructive | `{ name, preserve_data }` | — |
| `computer_destroy` | Destructive | `{ name, preserve_data }` | — |

Prompt block (`RegisterPrompt`): one paragraph — what a computer is, that
bots live on them, that `computers` shows health and that SSH setup is done
in the app.

### 4.7 Config and disk

```yaml
sandbox:
  enabled: true                 # false: no actor, no tools, no RPC; computers/ and snapshots/ kept on disk
  docker:
    socket: auto                # auto | unix:///path | tcp://host:2375
    image: ghcr.io/basedatum/theuth-computer   # tag defaults to this binary's version
    cpus: 2
    memory_mb: 4096
  ssh:
    transport: builtin          # builtin | system
    connect_timeout_secs: 20
  bundle:
    secrets: all                # all | [names]; `exclude: [names]` also honoured
    mcp: true
    skills: true
    knowledge_seed: false
    knowledge_seed_tags: []
  step_timeout_secs: 600
  health_interval_secs: 300
  snapshots_keep: 3
  max_computers: 8
```

Disk: `<data_dir>/computers/<name>.yaml` (the `Computer` row plus `Handle`
and `spec`; may be deleted — the compute then has to be adopted or removed by
hand), `<data_dir>/sandbox/staging/<name>/oauth/…` (transient, 0600, deleted
after push), `<data_dir>/sandbox/snapshots/<name>/*.tar.zst`,
`<data_dir>/sandbox/manifest.json` (cached 39 manifest for verification).

## 5. Providers on the computer

The Computer screen has a **Providers** panel: one row per provider the
parent has configured (vault key → `Key`; `oauth/<ns>.json` → `OAuth`;
remote MCP with `oauth/mcp/<s>.json` → `Mcp`), with the state on this computer.

- `Key`: pushed in bundle step 7; `Pushed ✓ <when>`. Re-pushed on `update`
  and whenever the parent's secret gets a new version (`SecretsEvent::Changed`
  → a targeted step-7 rerun; 37 keeps five versions so rollback on the node
  is `--secret-set` again).
- `OAuth`: `NeedsLogin` until the user presses **Log in for this computer**.
  `sandbox::Login { name, provider }` runs `oauth::Provider::flow` exactly as
  `/login` does (05 §2.1: browser on the parent, loopback port on the
  parent, poll or device code), with `Store` pointed at
  `sandbox/staging/<name>/oauth/` instead of `<data_dir>/oauth/`. The
  desktop shows the same login UI 05 §4 describes. On completion step 8 imports
  the file and deletes the staging copy → `Pushed ✓`. From then on the node
  refreshes its own grant; a grant that dies (Anthropic's ~30-day family,
  05 §2.1) shows as `NeedsLogin` again via the node's `info` provider status
  (one boolean per ns added to `rpc::Info`, §14) and the Bots screen badges
  the bot "provider login needed". The `oauth` crate change is one
  parameter — the store directory — and one new `LoginFor` command on the
  agent's login actor with a destination path.
- `Mcp`: same as OAuth with the tools crate's MCP login (05 §3) and the
  destination `oauth/mcp/<server>.json`.

What the user does for a new computer, in full: name it, pick Docker (or
paste a key and a host), tick the OAuth providers to log in for and click
through each browser consent. Nothing is typed on the computer.

### 5.1 Secrets for bots: three routes, one store

A bot uses secrets the way any session does — placeholders (37 §3) that the
*computer's* vault expands. Three ways a value gets there:

1. **The bundle** (§4.4 step 7): every vault secret, or the allowlist, at
   setup and on `update`; a new version on the parent (`SecretsEvent::Changed`)
   re-pushes that one name.
2. **The Secrets panel of a computer** (§8): the parent's secret names with a
   checkbox each — the allowlist made visible — plus "share with…" from the
   parent's own Secrets settings, which lists computers. Pushing is
   `set_secret` over the control conn; removing is `delete_secret` there
   (the parent's copy is untouched).
3. **The bot asks.** `secret_request` in a bot session becomes an outbox
   `secret_request` item (§6.5); the parent raises 37's masked field with the
   bot's name and note ("Ops asks for `datadog-api-key` — for the on-call
   dashboard"); the value goes to the computer with `set_secret` over the
   conn and, if the user ticks "also keep on this device", into the parent's
   vault too. The value is typed once, on the parent, never on the node,
   never in chat.

Secrets on a computer are shared by every bot on it — one trust domain, as
Grok Bot documents for its computer. A secret meant for one bot alone wants
its own computer.

**Trust.** A computer holds copies of every pushed secret and its own OAuth
grants, protected by a `secrets.key` file on that machine (no keychain in a
container or on a headless box). The operator of an SSH host can read them;
a Docker computer is as private as the parent's user account. This is the
32 §0.6 statement applied to credentials — the computer is inside the
parent's trust boundary — and the UI says so once, on the create screen. The
bundle allowlist exists for the user who wants less on a given box.

## 6. `crates/bot`

### 6.1 The file

```markdown
---
computer: ops-1            # local | <node name> | <container name>; required — any row of `computers_list`
model: anthropic/claude-opus-4-1   # optional; default agent.model of the home harness
effort: high               # optional
approvals: ask             # ask (default) | allow — `allow` = tools.approvals off for this session (33 §7 wording applies)
channels:                  # optional; where the parent binds this bot
  - slack: "#ops"          # every top-level message in #ops → the bot; replies in thread
  - slack: dm              # the owner's DM tab: `@ops …` prefix routes to this bot
workspace: /workspace/ops  # optional; default per §3.1
paused: false              # optional; a paused bot queues input and skips routines
---
You are Ops, the on-call engineer for … (standing instructions; the bot's prompt.md)
```

Name = file stem, `[a-z0-9-]{1,32}`; the peer name is `bot:<stem>`; the
session title is the stem capitalised; `effort`, `model`, `approvals` are
validated with `schedule::file`'s helpers (same shapes). Front matter errors
make the bot `Invalid { reason }` in the list, never a crash (34 §2's rule).

### 6.2 On the bot's home harness (`computer: local`): episodes

The `bot` plugin watches `bots/` (notify, debounce 500 ms, like 34 §2). A
valid file is a **bot**; the plugin keeps for it a `Bot` row in
`<data_dir>/bot/bots.sqlite` (`stem, computer, active_episode, episodes,
journal_updated_ms, paused`) and at most one **active episode**: a session
`SessionSummary { name: "bot:<stem>", pinned: true, retention: Durable,
parent: None, agent: None, cwd: workspace, title: "<Stem> · <episode
title>" }`, created through `agent::lifecycle::CreateSession`
(`crates/agent/src/lifecycle.rs:87`) with a new option `bot: Some(stem)` (a
column; every episode of a bot carries it, so "all of Ops's sessions" is a
query and the sidebar can hide them). The peer name `bot:<stem>` moves to
the active episode, so `send_message peer:bot:ops` and `into: bot:ops`
always reach the bot, never a closed episode.

**Opening.** An input for the bot with no active episode opens one; the
plugin sends `agent::SetSessionPrompt { session, text }` (new, §14) with
three parts the agent appends as one baseline part after the plugin blocks
at every turn start: (1) the plugin's own paragraph — name, computer,
teammates on this computer with their job lines, whether a screen exists,
how the user is reached (§6.5); (2) the **instructions** = the file body;
(3) the **journal** (§6.3). All three are re-read each turn, so an edit to
the file or the journal is live on the next turn without touching history —
the "read every call" contract `prompt.md` has. Knowledge hints (10 §4)
arrive as they do for any session; the bundle they come from is the
computer's.

**Closing.** An episode closes when: its Slack thread is resolved or the
user says so (`/close` on the bot, or the "Close episode" action); it has
been idle `bot.episode_idle_close_secs` (default 6 h); its context reaches
`bot.episode_max_tokens` (default 60 % of the model's window — measured as
36 §7 measures it) at a turn boundary; or the file changes `computer:`. To
close, the plugin runs one **closing turn** on the episode with a fixed
prompt (36's compaction style, *Done / Open / Next / Notes*, addressed to
"your future self starting the next episode") whose output replaces the
journal; then the detached harvest (10 §5) runs as after any turn; then the
row is archived (51) and `Bot.active_episode` cleared. A closed episode
stays searchable (49) and readable; it is never deleted by the plugin.
`paused: true` → `agent::SetPaused` on the active episode (input queues;
`AgentState::Paused` is new); routines skip a paused bot and say so in
their run row. File deleted → the active episode closes and every episode
is archived, not deleted; a re-created file with the same stem re-adopts
the journal and the history.

**One active episode.** A second thread arriving mid-episode queues behind
it (the 62 `NextTurn` lane) unless the current episode is idle, in which
case the plugin closes it if the new input names another thread
(`channel`+`thread_ts` differ) and opens a new one. This is the person
model: one thing at a time, a queue you can see. Parallel episodes per
thread are §17.

Wake sources are all existing: `Agents::input` from a device or the mirror,
`<schedule into: bot:<stem>>` routines (34 + §14), `job_result`/`job_idle`
(14, 61), peer messages (50), child completions (01 §2). A bot's turn budget
is a normal turn's; a bot that wants to keep going uses a loop schedule
(33) with `into:` — no new "run forever" primitive (33 §0's argument).

### 6.3 Memory: instructions, journal, knowledge, history

| Layer | Owner | Where | Size | Read |
|---|---|---|---|---|
| instructions | the user | `bots/<stem>.md` body | ≤ 32 KiB | every turn |
| journal | the bot | `<data_dir>/bot/journal/<stem>.md` (a 59 document, so the user can read and edit it in the app) | ≤ `bot.journal_max_bytes` (8 KiB); the closing prompt says so | every turn |
| knowledge | the computer (shared by its bots) | the harness's 10 bundle; a bot's writes carry `source: bot:<stem>` | 10's limits | hints per turn; `knowledge_search` |
| episodes | the bot | `sessions.sqlite`, `bot = stem` | 51 retention | `search` (49) on demand |
| files | the computer | `/workspace/<stem>` and shared dirs | disk | as the bot reads them |

The journal is the thing a plain session lacks: a short, current, first-person
statement of *what I am doing and what I know that is not yet a fact* — open
tasks, people, decisions, a note to self. It is rewritten, not appended, so
it cannot grow; a bot that needs more room writes knowledge. The user can
open it from the Bots screen's Memory tab, and a routine's `<schedule>`
prompt can say "update your journal first". Rejected: appending compaction
summaries (grows without bound and repeats itself); putting the journal in
the knowledge bundle (it is state, not fact; it changes hourly and would
churn the `gix` history).

Tools registered by `bot` on every harness:

| Tool | Risk | Args | Returns |
|---|---|---|---|
| `bots` | ReadOnly | — | every bot this harness knows: `{ name, computer, state, episode, last_active, last, channels, routines: n, shared: n }` |
| `bot_share` | Mutating | `{ id, note? }` | queues knowledge concept `id` for the user's harness: `{ queued: true }`; on the parent itself (a local bot) it is a no-op that says so — the knowledge is already the user's |
| `bot_journal` | Mutating | `{ text }` | replaces the calling bot's journal now (not only at close); refuses over `journal_max_bytes` with the size; refused for non-bot sessions |
| `bot_close_episode` | Mutating | `{ why? }` | the calling bot ends its own episode after this turn (runs the closing turn) |
| `bot_put` | Mutating | `{ name, front_matter?, body? }` | writes/updates `bots/<name>.md` on this harness (the model may draft a bot; the UI is the usual path) |

Prompt block: bots exist, `bots` lists them, a bot is a peer
(`send_message peer:bot:<name>`), `bot_share` hands the user a fact for
keeps. The bot's own per-session paragraph (§6.2 part 1) adds: `show`,
`message_user`, `secret_request` and approvals reach the user through the
user's own harness even when it is away; `bot_journal` is for notes to your
future self; hand-offs are messages to teammates.

### 6.4 On the parent (`computer: <name>`)

For a bot placed on a computer the plugin keeps three things:

1. **The push.** On file save: rewrite `computer: local`, `bot_put { name,
   front_matter, body }` over the control conn (§4.4 step 11), which the
   node's bot plugin writes to `bots/<stem>.md` and its watcher picks up.
   Routines edited in the Bots UI go through `schedule_put` on the node.
   Works for any node — a hand-paired one needs no sandbox record. Changing
   `computer:` closes the episode on the old computer (its journal is
   fetched and pushed to the new one with the file; the old computer's
   knowledge stays where it is — the UI says so), then pushes to the new.
2. **The mirror.** One `AcpChild` session per bot: `SessionSummary { name:
   "bot:<stem>", agent: Some("bot:<stem>"), parent: None, pinned: true, bot:
   Some(stem) }`. Transport = `nodes::NodeDialer` for the computer (mode-1
   ACP conn, 32 §0.2). It follows the bot's **active episode**: open with
   `session/load` of the episode id (`bot_status` over the control conn
   reports it); when the node closes an episode and opens the next, the
   mirror re-targets (`BotEvent::EpisodeChanged` on the node's event stream,
   or noticed at the next pull) — `AcpChild` already persists everything it
   projected, so the mirror's transcript is the bot's whole life in one
   continuous conversation, with a divider entry (`HistoryEntry::Marker
   { kind: "episode", title }`, new) where an episode closed. Disconnect:
   `AgentState::Offline` (new) on the mirror; reconnect on `NodeOnline` with
   `session/load`, which replays what the node did meanwhile (06 §3.2).
   Input typed into the mirror while offline is queued on the parent
   (AcpChild already persists input) and sent on reconnect. Steers, cancel
   and permission requests are 06's. A mirror is never "finished":
   `AcpChild`'s completion path is suppressed for `bot` rows (one `if` in
   `acp_child.rs`). Local bots need no mirror: the Bots screen stitches
   their episodes by the `bot` column the same way.
3. **The puller.** On `NodeOnline` and every `bot.pull_interval_secs` (60):
   `bot_outbox` over the control conn → one delivery per item kind (§6.5):
   - `message { text, files? }` → `UserSink` delivery on the parent
     (24): the bot's bound thread if any, else the owner's channels, prefixed
     `<Stem>:`; files fetched by `get_attachment`.
   - `artifact { id }` → `get_attachment` bytes → the artifact store's
     insert path (`crates/artifact/src/store.rs`) under the mirror session,
     provenance `bot:<stem>` → `ArtifactShown` in the mirror session, so it
     appears in the conversation, the bound Slack thread and the Artifacts
     page.
   - `knowledge { id }` → `knowledge_get(id)` → `knowledge_put` locally with
     the original id (under `bots/<stem>/…` only if the id collides), with
     frontmatter `source: bot:<stem>`, tag `bot:<stem>`, commit author `bot
     <stem>`.
   - `secret_request { name, note }` → `SecretsEvent::Requested` on the
     parent with `for: computer` — the user's masked field (37 §5–6), the
     value goes straight to the computer with `set_secret` over the control
     conn, plus "also keep on this device" (§5.1).
   - `approval { id, tool, args }` → `ApprovalRequested` mirrored on the
     parent; the answer goes back as `resolve_approval { id, decision }`
     over the control conn (§7).
   Then `bot_outbox_ack(id)`. 100 MiB per item (24's `MAX_FILE`,
   `crates/agent/src/user.rs:25`). Items also appear in the Bots screen's
   **Shared** tab (knowledge and artifacts, with "revert"/"open") and
   Activity tab (all kinds).

The mirror is what Slack binds: `channels: [{slack: "#ops"}]` → the plugin
sends `slack::BindChannel { channel, session: mirror }` (new, §14): top-level
messages in the channel become `SendInput` to the mirror, replies go in the
message's thread, approvals for the mirror land there (21 §12, already
"session bound to a thread → that thread"). `slack: dm` → the owner DM
accepts `@<stem> …` and strips the prefix. A local bot binds the same way
with its active episode (the plugin re-binds on rotation). Signal: same
message on `signal` later.

### 6.5 On the node: the bot plugin is the user (outbox)

A node has no Slack, no Signal and no desktop of its own; whatever a bot
does "for the user" would otherwise go nowhere. The `bot` plugin therefore
registers itself with `agent::RegisterUserSink` (24 §2) on every harness
where a bot session exists and the harness is a node (i.e. `link` is
enabled and at least one device is a theuth parent — `devices.yaml`
`platform: "theuth"`, 32 §0.1). Its sink writes rows to
`<data_dir>/bot/outbox.sqlite` `{ id, bot, kind, payload, at_ms, acked }`:

| Source on the node | Outbox kind |
|---|---|
| `message_user { text, files }` from a bot session | `message` |
| `show` in a bot session (`ArtifactShown`) | `artifact` |
| a routine's final reply (34 §4 delivery, unless `quiet`) | `message` |
| `bot_share { id }` | `knowledge` |
| `secret_request` (`SecretsEvent::Requested`) in a bot session | `secret_request` |
| an `ApprovalRequested` in a bot session (§7) | `approval` |

`bot_outbox` (RPC, ReadOnly) lists unacked rows; `bot_outbox_ack(id)` marks
them; `bot_status` reports active episodes and pending approvals. Rows acked
more than 30 days ago are pruned. The node needs no knowledge of the parent:
whoever holds the device key that paired it may pull, and a phone paired to
the node (32 §0.3) sees the same items in the node's own UI. When a node has
*no* theuth parent among its devices the sink is not registered and the
node behaves as any harness (its own channels, if configured).

### 6.6 Several bots on one computer: hand-off

Bots on one computer are sessions on one harness, so 50 already makes them
peers: `discover_peers` lists `bot:ops`, `bot:triage`, … with 62's activity;
`send_message { to: "peer:bot:triage", await_reply }` carries a request and
gets an answer; they share the workspace, the knowledge bundle (a concept one
writes, the other's hints surface), the artifacts store and the vault. That
is the whole hand-off mechanism; the bot plugin adds the conventions, not a
protocol:

- The per-session prompt paragraph (§6.2 part 1) names the other bots on the same
  computer and their job lines, and says: hand a task over by message with
  what "done" looks like; leave durable results in `/workspace/<shared>` or
  the knowledge bundle, not in chat; when you finish a hand-off, tell the
  user through `message_user` only if the user asked to be told.
- A message from a bot arrives as a `<message from="session:…">` input like
  any peer message (50 §3); untrusted as any peer body.
- The Bots screen's Activity tab shows hand-offs as edges (who → whom,
  when, the first line) from the `send_message` tool calls in each bot's
  history; the overview groups bots by computer so a shared computer reads
  as a team.
- Two bots may not be the same bot: a file name is unique per harness; a
  bot's mirror on the parent is one session, so a parent-side bot and a
  node-side bot with the same stem is a validation error (`Invalid`).

Across computers, the parent is the only common point; v1 does not route
bot-to-bot messages between computers (not in v1, above). A bot that needs
another computer's bot asks the user, or the user runs both on one computer.

### 6.7 Actor, events, RPC

`BOT_ACTOR` — `List → Vec<BotInfo>`, `Put { name, front_matter, body }`,
`Delete { name }`, `Pause { computer: Option<String>, name: Option<String> }`,
`Resume {…}`, `CloseEpisode { name }`, `Journal { name } → String`,
`SetJournal { name, text }`, `Episodes { name } → Vec<SessionSummary>`,
`Shared { name, limit } → Vec<SharedItem>`, `Outbox → Vec<OutboxItem>` (node
side).

```rust
pub struct BotInfo {
    pub name: String, pub computer: String, pub session: Option<SessionId>,   // mirror, or the local active episode
    pub episode: Option<EpisodeInfo>,                            // id, title, opened_ms, turns, tokens_pct — None between episodes
    pub episodes: u32, pub journal_updated_ms: Option<u64>,
    pub state: BotState,        // Idle | Working | Awaiting{what} | Approval{tool} | Paused | Offline | Invalid{reason} | Provisioning | Closing
    pub model: Option<String>, pub approvals: Approvals, pub channels: Vec<Channel>,
    pub last_active_ms: Option<u64>, pub last: String,          // first line of the last assistant text (AgentInfo.last)
    pub routines: Vec<RoutineInfo>,                              // name, next fire, last result — from the home harness
    pub shared: u32, pub cost_today: Option<Cost>,               // 51's accounting for the mirror/local session
    pub workspace: String, pub file_mtime_ms: u64,
}
#[serde(tag = "type")]
pub enum BotEvent { Changed { bot: BotInfo }, Removed { name: String }, Shared { name: String, item: SharedItem },
                    EpisodeChanged { name: String, from: Option<SessionId>, to: Option<SessionId> },
                    Delivered { name: String, item: OutboxItem } }   // the parent re-publishes pulled items
```

RPC: `bots_list`, `bot_get(name) → { info, front_matter, body }`,
`bot_put(name, front_matter, body)`, `bot_delete(name, keep_session)`,
`bot_pause(name)`, `bot_resume(name)`, `bot_close_episode(name)`,
`bot_journal(name)`, `bot_set_journal(name, text)`, `bot_episodes(name)`,
`bot_shared(name, limit)`, `bot_status` (node side: active episodes, pending
approvals), `resolve_approval(id, decision)` (node side, from the parent),
`bot_routines(name)`, `bot_routine_put(name, file)`, `bot_routine_delete(name,
file)` (the last three proxy to the home harness's schedule methods);
events `BotEvent` through the bridge. `discover_peers` lists bots with
`bot:` prefix and their `BotState` (62's activity-bearing directory).

### 6.8 Config

```yaml
bot:
  enabled: true              # false: no watcher, mirrors are not opened, files stay
  workspace: ~/bots          # local bots: <workspace>/<name>
  pull_interval_secs: 60
  reconnect_backoff_secs: [1, 30]
  max_bots: 16
  episode_idle_close_secs: 21600   # 6 h idle → close the episode (journal, harvest, archive)
  episode_max_tokens: 0.6          # fraction of the model window (or an absolute count) that closes at the next turn boundary
  journal_max_bytes: 8192
  approval_wait_secs: 14400        # 4 h: how long a bot's approval waits for the user before the bot is told to park the task
  push: true                       # node side: wake the user's paired phone through 42 for approvals and finished work
```

Disk: `<data_dir>/bots/*.md` (the user's), `<data_dir>/bot/journal/<stem>.md`
(the bot's; a 59 document — back it up with the data dir), `<data_dir>/bot/bots.sqlite`
(bot rows and episode index; rebuildable from `sessions.sqlite` by the `bot`
column), `<data_dir>/bot/outbox.sqlite` (rebuildable: unacked items are
lost), `<data_dir>/bot/mirrors.json` (`stem → { computer, episode_id }`,
rebuildable from `bot_status`).

## 7. Approvals, security (28), and what a bot may do unattended

- A bot's tools run on its home harness with that harness's `tools.policy`;
  the pushed `config.yaml` copies the parent's policy. `approvals: ask`
  (default) means a Mutating/Destructive call raises `ApprovalRequested` on
  the node; the ACP permission request reaches the parent's mirror and every
  parent frontend and bound thread shows the card (06 §3.2, 21 §12). For
  bot sessions the gate's timeout is `bot.approval_wait_secs` (4 h) instead
  of the default: the ask is also written to the outbox (§6.5) so it
  survives the ACP conn dropping and is re-shown when the parent reconnects
  (`bot_status` lists pending approvals; `resolve_approval` answers them
  over the control conn), and the node's `link` sends a 42 `PUSH` (*Approval
  needed*) to every paired phone. When the wait expires the call is denied
  and the bot is told "the user was unreachable; park this task and note it
  in your journal" — a bot never fails a night's work on one prompt.
  `approvals: allow` is 33 §7's unattended mode and the UI explains it in
  the same words.
- **Parent unreachable.** The bot keeps working: sessions, routines, jobs,
  knowledge, providers and secrets are on its computer. Everything for the
  user waits in the outbox and is delivered on reconnect; nothing is lost.
  Approvals wait as above. A phone paired to the node (32 §0.3) can open the
  bot's episode directly through the relay and answer there. Only Slack and
  Signal need the parent (§10).
- Everything a bot reads is untrusted (28); the node applies 28 as any
  harness. Shared knowledge is *from the bot*: provenance in frontmatter, a
  distinct commit author, a revert button; hints (10 §4) render `source:
  bot:<name>` so the parent model knows where a fact came from.
- The bundle makes the computer a member of the trust domain (§5). A
  revoked computer (`destroy`) also runs `nodes::Remove` (which revokes the
  parent device on the node, 32 §8.4) and deletes the staged files; secrets
  already on a destroyed SSH host are the operator's to wipe —
  `destroy { preserve_data: false }` runs `t --uninstall-service` and `rm -rf
  <dd>` first and reports what failed.
- The parent's control conn gains write methods on the node (`send_input`,
  `schedule_put`, `put` via `exec`). The parent is already the node's trust
  root (32 §0.6); no new principal.

## 8. UI

The user-facing vocabulary is **Bots** and **Computers**. The desktop and
mobile app get one nav item; the TUI gets two slash commands.

### 8.1 Desktop and mobile (`crates/desktop/ui`)

Sidebar (`AppSidebar.tsx`): **Bots** (`<Bot />` icon) between Artifacts and
Meetings, route `/bots`. Bots do not appear in `SessionList` (their mirror
rows are filtered by `bot != null`), they appear here.

`/bots` — **overview.** A responsive card grid (one column on phones). Each
card: initial-avatar, name, the first line of the instructions as the job
line, a state pill (`Idle · 2h`, `Working`, `Needs approval`, `Paused`,
`Offline`, `Provisioning`), the computer chip with a dot (green/grey), the
channel chips, `cost today`, and "last said" in one dimmed line. Card
actions: **Message** (opens the session in the focused pane, the same rule New
chat follows), **Pause/Resume**, **⋯** (edit, routines, move to computer,
delete). Header: **New bot**, **Computers** (segmented control switches the
grid to computers). Empty state, verbatim: **"A bot is a teammate with a
name, a job and a computer. It keeps working when you're not here."** —
then "Create your first bot" and two templates (Ops on-call, Research
scout). The word *session* appears nowhere on these screens.

**New bot** — a three-step sheet: (1) name + instructions (textarea with the
two templates and a "describe the job in a sentence and let the model draft
the instructions" button → `bot_put` via `@ToAssistant`-style draft in a
private session); (2) computer — radio list of ready computers, **This
device (local)**, **New computer…** (inlines the computer wizard); (3) model
(the 02 model menu), approvals toggle with the 33 §7 sentence, channel
picker (Slack channels the app knows, DM). **Create** writes the file;
provisioning progress, if any, streams on the card.

`/bots/<name>` — **detail.** Two-pane on desktop: the bot's **conversation**
in the chat pane (the unchanged component over the mirror, or over the
local episodes stitched by the `bot` column) — one continuous thread, with
a quiet divider where an episode closed ("Episode closed · journal
updated", expandable to the journal diff) and a header line for the
current episode ("Working on: <episode title> · 3 turns · 41 %") with a
**Close episode** action; a right rail with tabs:
- **Activity** — timeline of turns, episodes opened/closed, routine fires
  (`<schedule>` inputs), deliveries (messages, artifacts, knowledge),
  hand-offs, approvals (with wait time), offline gaps; from `AgentEvent`
  history + `BotEvent`.
- **Routines** — the home harness's `into: bot:<name>` schedules: name,
  cadence (human text), next fire, last result; add/edit sheet with a
  cadence builder (every N / daily at / weekdays at / cron), prompt body,
  `quiet`, `until`/`max_runs` for loops (33 §6's columns).
- **Shared** — knowledge concepts and artifacts the bot handed up, newest
  first, each with open and revert.
- **Memory** — the **journal** first (rendered markdown, editable in place
  with 59's editor; "last updated 2 h ago by Ops"), then the computer's
  knowledge: counts by type, the last 20 concepts (via `knowledge_list` over
  the control conn, the bot's own marked), a search box (`knowledge_search`
  there), and **Episodes** — the list of closed episodes with title, span
  and turn count, each opening read-only.
- **Settings** — the front matter as a form (computer, model, effort,
  approvals, channels, workspace) and the body in the markdown editor
  (59's editor component); save = `bot_put`.
Phones: the rail becomes a segmented control above a single column.

`/bots/computers` — **computers.** Cards: backend glyph (Docker whale /
terminal for SSH / this device), name, state pill, `theuth <version>` with an
"update available" dot when behind the parent, uptime, disk bar, the bots on
it as avatars, CPU/mem for Docker. Actions: **Update**, **Snapshot**,
**Reset (keep data)**, **Rebuild (wipe)**, **Remove**, **Providers**. Reset,
Rebuild and Remove use the 62-style confirmation with the consequence
spelled out ("Rebuild replaces the machine and its files; snapshots stay").

**New computer** — wizard sheet: (1) backend — Docker (shows the detected
daemon and version, or "Docker Desktop not running" with a retry), SSH
(host, port, user, then **Private key**: a drop zone/paste field that goes
straight to the vault through the 37 masked flow, never the chat; or
"Use my system ssh" with an alias field), the trust sentence from §5; (2)
bundle — secrets (all / choose), MCP servers, skills, seed knowledge (tag
picker); (3) providers — the OAuth rows with **Log in for this computer**
buttons (each opens the 05 §4 login card); (4) **Create**. The card then
shows the installer as a step list with the live log line under the current
step and a "show log" disclosure; failure shows the step, the reason and
**Retry** (resumes at that step).

**Take-over.** When a bot's turn is blocked on an approval or asks for a
secret, the card and the bot's session show it exactly as today; a
notification (42 push) reads "Ops needs you: <tool>". No remote screen in
v1 (`screen: false`).

### 8.2 TUI

`/bots` — a list (name, computer, state, last active, last line) with Enter
opening the bot's session (mirror or local) in the normal transcript view;
`/bot new <name>` opens `bots/<name>.md` in `$EDITOR` from a template;
`/computers` — the computer table with `u`pdate, `r`eset and the setup log
when one is running. Creation of computers stays in the app or the CLI.

### 8.3 CLI

`t --computer-create <name> --docker` and `t --computer-create <name> --ssh
user@host --key <secret-name>` for the terminal user and for CI of this
feature; they attach to the running engine (63) and stream the progress
lines. `t --bot-new <name>` writes the template.

## 9. Grok Bot, and what we did not copy

| Grok Bot | Here | Why |
|---|---|---|
| Cloud computer the vendor owns; client ↔ gateway over HTTP + SSE | The user's own container or machine; parent ↔ node over the 31 relay, ACP over the same link | already built; the user keeps the box |
| Broker mints gateway/inference credentials per session | The bundle: real secrets and per-computer OAuth grants pushed once | no broker exists; a node is a full harness |
| One computer per account | Any number: this engine, any node, any container; several bots share one and hand off through 50 | the user asked for both |
| Per-bot screen leases on a shared desktop | none in v1 (`screen: false`) | no display in the image; seam kept |
| Reset = restore snapshot | `recreate{preserve_data}` + explicit `snapshot`/`restore` (Docker) | keep the three operations apart |
| Cursorless SSE stream | ACP `session/load` replays; the node's store is the truth | 06 already solved catch-up |
| Marketplace connectors | the parent's MCP servers, pushed | one mechanism |
| Skills learned from demonstration | `skills/` pushed; authoring stays the parent's | not in v1 |

Copied on purpose: the narrow connector seam (create/recreate/destroy, the
rest private); labels that prevent adopting a stranger's container;
durable volumes apart from compute; readiness by authenticated probe;
"bots share the computer — do not put on it what all of them may not use".

## 10. What we knowingly give up

- **Two harnesses, two knowledge bases.** A bot does not see what the parent
  learned yesterday unless the user seeds or the bot is told. The seed and
  `bot_share` are the v1 answer; sync is a later doc.
- **The parent must be up to reach a remote bot from Slack.** Slack and
  Signal run on the parent. The bot keeps working; the reply waits in the
  outbox until the parent returns; the phone can still be woken (42) and can
  talk to the node directly. A node-side Slack would need a second Slack app
  and split the owner's DM — rejected.
- **One active episode per bot.** A bot answers threads one at a time, like
  a person; a second thread queues. Parallel episodes are §17.
- **Traffic through the relay even for a local Docker computer.**
- **A computer is one trust domain.** Two bots on one computer share files,
  cookies and secrets, as in Grok Bot.
- **Docker computers have no display.** `computer_*` reports the absence.
- **OAuth logins are per computer.** A second computer means clicking
  through consent again; a full `recreate{preserve_data:false}` too. The
  UI says so before the click.

## 11. Alternatives considered (beyond §0)

- **Run the bot's model calls through the parent** (the node executes
  tools, the parent talks to providers; 15's control/execution split). Keeps
  credentials on one machine, but the bot dies with the laptop lid, and it
  is a new protocol between two harnesses. Rejected for the always-on case;
  it remains 15's shape if a multi-user server ever ships.
- **A cloud-provider backend first** (Hetzner/Fly). Provisioning a VM is a
  third-party API plus the SSH installer; v1 builds the installer and the
  seam is `impl Sandbox`.
- **SSH without a built-in client.** Rejected by the user; a parent without
  `ssh` (Windows, minimal macOS) must still work.
- **Copy `oauth/*.json`** to the computer. Rejected: refresh rotation.
- **A `routines:` list in the bot file** instead of `into:`. Rejected: a
  second scheduler.
- **Bots as a Slack-first product** (each bot its own Slack app, no
  parent). Rejected: N apps, no shared vault, no desktop.
- **`bollard` vs `testcontainers-rs`**: the latter is a test harness over
  bollard; we need the API, not a fixture.
- **`russh` vs `ssh2` (libssh2 FFI) vs `openssh` crate (wraps the binary)**:
  `ssh2` is a C dependency with blocking I/O; `openssh` is the system
  transport with a nicer API and may replace hand-rolled `ssh` argv in the
  `system` transport if it proves cleaner — a build-time choice inside one
  file.

## 12. Licensing

`bollard` (Apache-2.0), `russh` and `russh-sftp` (Apache-2.0), `ubuntu:24.04`
image (Ubuntu licence; no redistribution of theuth beyond the release
asset), Node LTS (MIT) and `uv` (MIT/Apache-2.0) inside the image. No GPL.
Pin versions at build time and record them in Cargo.lock as usual.

## 13. Numbers

| Limit | Value | Where |
|---|---|---|
| computers per parent | 8 | `sandbox.max_computers` |
| bots per harness | 16 | `bot.max_bots` |
| installer step timeout | 600 s | `sandbox.step_timeout_secs` |
| control-conn request | 30 s (existing) | `nodes/client.rs:20` |
| bundle tar (mcp + skills) | 64 MiB | refuse with the offending dir |
| secret value on stdin | 64 KiB | this doc's bound for `--secret-set` (37 sets none) |
| outbox item | 100 MiB | 24's file cap |
| outbox pull interval | 60 s + on `NodeOnline` | `bot.pull_interval_secs` |
| health probe | 300 s | `sandbox.health_interval_secs` |
| snapshots kept | 3 | `sandbox.snapshots_keep` |
| mirror reconnect backoff | 1–30 s, like nodes | `bot.reconnect_backoff_secs` |
| bot file | ≤ 32 KiB body | matches `prompt.md` guidance |
| journal | ≤ 8 KiB | `bot.journal_max_bytes` |
| episode idle close | 6 h | `bot.episode_idle_close_secs` |
| episode token budget | 60 % of the model window | `bot.episode_max_tokens` |
| approval wait for bots | 4 h | `bot.approval_wait_secs` |
| episodes kept | all (51 retention) | archived, never deleted by the plugin |
| version window | node ≥ parent minor − 1 | 39 compatibility |

## 14. Amendments to earlier docs (exact sentences; applied with the code)

- **00 §4 dependency line** becomes `runtime ← tools ← secrets ← knowledge ←
  agent ← link ← {signal, slack, nodes, schedule, artifact, update, voice}
  ← sandbox ← bot ← rpc ← tui ← theuth` (`sandbox` depends on `nodes`,
  `secrets`, `oauth`, `link`, `update`; `bot` on `agent`, `acp`, `nodes`,
  `sandbox`, `knowledge`, `artifact`, `schedule::file`, `slack`).
- **00 §10 crate table**: add `sandbox` (plugin, 64) and `bot` (plugin, 64).
- **00 §2 process model**: after the apps helper paragraph add: "A
  *computer* (64) is where a bot runs: this engine, a node, or a Docker
  container on this machine running `t --headless` and paired as a node;
  the `sandbox` plugin creates, pairs and provisions it, never runs tools
  on it, and never puts containers on nodes."
- **32 §0.7** "Nothing is synced." → "Nothing is synced *by `nodes`*. The
  `sandbox` plugin (64) pushes secrets, OAuth grants it logged in for,
  MCP configs, skills and bot files to any node the user places a bot on
  — provisioned or hand-paired — over the node's own RPC." §15's "credential/skill sync"
  item is closed by 64.
- **32 §0.2** add: "A 64 bot on a node is the same ACP conn with a
  top-level, never-finishing `AcpChild` on the parent."
- **37 §7 Nodes bullet** → "Nodes: a subagent on another machine runs in
  *that* theuth, whose placeholders resolve against *that* vault. For a
  computer (64) the vault is filled by the `sandbox` installer through `t
  --secret-set` on stdin over the setup channel; for a hand-paired node by
  `--secret-set` on that box. Values never cross as model-visible text."
- **05 §2**: "`LoginFor { ns, dest_dir }` runs the same flow with the token
  store pointed at `dest_dir`; 64 uses it to log a *computer* in from the
  parent."
- **34 §2 front matter**: add `into: <peer name>` — "deliver the wrapped
  prompt as input to the session that owns that peer name instead of a new
  session; the session's own `approvals`/`model` win; `quiet` still applies."
- **36 §6**: the compaction style *Done / Open / Next / Notes* is also the
  bot journal's closing prompt (64 §6.2); one shared template.
- **24 §2**: `RegisterUserSink` may be called by `bot` on a node; a
  `UserSink` delivery carries `for_bot: Option<String>`.
- **tools gate**: per-session approval timeout (`SetApprovalWait { session,
  secs }`); `resolve_approval` RPC answerable by a paired parent.
- **42 §4/§6**: a node's `link` sends `PUSH` for bot approvals and finished
  work to devices that presented a token; the parent is not in the path.
- **agent history**: `HistoryEntry::Marker { kind, title }` for episode
  dividers in a mirror/local bot conversation.
- **06 §3.2**: "A child row with `bot: Some(_)` (64) is never completed; its
  transport reconnects and reloads."
- **07 §4 baseline parts**: part 7b "session prompt" — `agent::SetSessionPrompt
  { session, text }` text, after the plugin blocks; owned by whoever set it.
- **01 §1.4**: `config.local.yaml` layered over `config.yaml` (figment) so a
  pushed configuration does not overwrite a user's edits on a node.
- **21 §6**: `slack::BindChannel { channel, session }` and the `@<name>`
  DM prefix.
- **agent**: `SessionSummary.bot: Option<String>`, `AgentState::{Paused,
  Offline}`, `SetPaused`, `SetSessionPrompt`, `CreateSession.bot`.
- **rpc::Info**: `providers: Vec<{ ns, state }>` (`ok | needs_login`).
- **rpc::Api (served by every harness, called by a parent over the control
  conn)**: `oauth_import { ns, json }`, `bot_put { name, front_matter, body }`,
  `schedule_put { file, text }`, `schedule_delete { file }`, `bot_outbox`,
  `bot_outbox_ack { id }`; `nodes::client::METHODS` lists every one the
  parent calls.
- **`t` one-shots**: `--first-run`, `--knowledge-import <bundle>`,
  `--computer-create …`, `--bot-new <name>`.
- **15 §0**: "64 provisions another machine as a node; it is not this doc's
  worker sandbox. 15 stays shelved."

## 15. Build steps, each with its check

| # | Step | Done when |
|---|---|---|
| 1 | `crates/sandbox`: trait, `Computer` record (incl. synthesised rows for `local` and hand-paired nodes), actor, RPC, `docker` backend (bollard), `deploy/computer/Dockerfile` + compose with local relay, bring-up steps 1–6 + verify | `t --computer-create test --docker` on the Mac ends `ready`; `/nodes` shows `test · linux · theuth <v>`; `spawn_subagent{agent:"test"}` returns `uname -a` from the container; killing the container → `Offline`, `docker start` → `Ready` without a re-pair |
| 2 | Bundle over RPC: steps 7–11, node methods `oauth_import`, `bot_put`, `schedule_put`, `METHODS` extended; `LoginFor` in `oauth`/agent login actor; staging dir; Providers panel data | on a fresh container **and** on an existing hand-paired node (build-A), with only the UI: a vault key provider makes a model call on the node; `Log in for this computer` with `openai` yields `oauth/openai.json` on the node whose refresh token differs from the parent's; pushed `mcp/github.json` server lists tools on the node; `secret_list` there shows the parent's names; the staging dir is empty afterwards |
| 3 | `crates/bot` local: watcher, episodes (open/close/queue), journal document + closing turn, `SetSessionPrompt`, `Paused`, `into:` in schedule, `bots`/`bot_journal`/`bot_close_episode`/`bot_put` tools, TUI `/bots` | `bots/scout.md` (`computer: local`) → an episode named `bot:scout` whose first turn shows instructions + (empty) journal in the prompt; editing the body is live next turn; a schedule with `into: bot:scout` fires into the active episode (one row, not two); `bot_close_episode` writes a journal ≤ 8 KiB, archives the row, and the next input opens a new episode whose prompt carries that journal; a forced 60 % budget closes at a turn boundary; `paused: true` queues an input and releases it on `false` |
| 4 | Remote bots: push, mirror `AcpChild`, reconnect with `session/load`, `Offline` state, peer directory | `bots/ops.md` (`computer: test`) → the file exists on the node, a mirror row on the parent; a message typed in the mirror is answered by the node; `docker stop` → mirror `Offline`; input typed while stopped is delivered after `docker start`; history matches the node's `list_sessions` transcript; an approval raised by the bot appears on the parent and Slack |
| 5 | The node as user: `bot` `UserSink` → outbox (message, artifact, knowledge, secret_request, approval), puller with per-kind delivery, provenance, Shared/Activity data, mirror re-target on `EpisodeChanged` with a divider entry | on the node the bot runs `show`, `message_user`, `bot_share` and `secret_request` → within 60 s the parent has the artifact card in the mirror, the message in the owner's channel prefixed `Ops:`, the concept with `source: bot:ops` and commit author `bot ops`, and a masked field naming Ops; answering it yields `secret_list` on the node showing the name; `knowledge_restore` reverts the concept; all four made while the parent was stopped arrive after it restarts; an episode closed on the node shows as a divider in the mirror and the next turn lands under it |
| 6 | Slack channel binding, `@<name>` DM prefix | a message in the bound channel reaches the bot and the reply threads under it; `@ops` in the owner DM routes; an unbound channel is untouched |
| 7 | `ssh` backend: `builtin` (russh + sftp, vault key, TOFU host key, one `proxy_jump`), `system` transport, `DaemonKind::Service`/`Manual`, `linger` | a fresh Ubuntu 24.04 VM with only sshd + curl becomes `ready` from the wizard with a pasted key on a parent that has no `ssh` binary on PATH; the same with `transport: system` and an alias; a changed host key fails the setup with the named reason; the node survives a VM reboot (systemd user unit + linger) |
| 8 | Lifecycle: `update` (pause bots, re-run, resume), `recreate{preserve_data}`, `snapshot`/`restore` (Docker), health probe, version window, `destroy` with uninstall; bot approvals: `approval_wait_secs`, outbox `approval`, `resolve_approval`, 42 push from the node | a bot's Destructive call with the parent stopped stays pending 10 min (test value), the phone receives *Approval needed*, the parent restarts and shows the card, approving runs the tool; past the wait the bot is told to park and its journal says so; `update` to a newer `t` keeps the bot's session id and transcript; `recreate{true}` keeps `/data` and `/workspace`; `recreate{false}` ends with `NeedsLogin` on OAuth rows; `snapshot` → write a file on the node → `restore` → the file is gone; disk/uptime appear on the card within 5 min |
| 9 | Desktop + mobile UI: sidebar item, overview, new-bot sheet, detail rail (five tabs), computers, new-computer wizard, progress stream, take-over notification; TS bindings; tests | every RPC of §4.5/§6.7 is exercised from the UI; the wizard creates a Docker computer end-to-end without the terminal; the phone shows the overview and can Message/Pause; `bun test` for the bots components pass |
| 10 | Docs: 00/32/37/05/34/06/07/01/21 amendments, `plugins.md` example pointer, this doc's Status | `just check-docs` (if present) and a reviewer reading 00 §4 sees both crates |

Order: 1 → 2 → 3 → 4 → 5 → 6 in one series (Docker only), 7 and 8 next, 9
alongside 4–8 once the RPC surface is stable, 10 last. Step 1 alone is a
useful thing: a Docker node in one command.

## 16. Where this doc wins

32 §0.7 and §15 (sync), 37 §7 (nodes bullet), 34 §2 (front-matter key set),
06 §3.2 (a child that never finishes). Everything else here extends without
contradiction; if a builder finds otherwise, this section grows a line.

## 17. Open

1. **Cost visibility for remote bots.** 51's accounting is per session on
   the harness that paid; the mirror sees ACP updates, not tokens. Options:
   the node's `list_sessions` row carries cost (one field, pulled with health)
   or ACP `usage` metadata if the SDK exposes it. Decide in step 4.
2. **Session prompt vs first message.** `SetSessionPrompt` re-reads the body
   every turn (live edits) but changes the cached prefix (07 §4) on every
   edit; acceptable for a bot, worth one measurement in step 3.
3. **`update` while a bot is mid-turn for hours.** Pause waits for a turn
   boundary; a bot in a long PTY job may not reach one. Step 8 sets a cap
   (10 min) after which the UI offers "update anyway".
4. **Which `t` binary a Docker computer runs during development on a Mac.**
   The release asset is the default; `image: build` + `binary:` needs a
   Linux `t` from build-A. A `just computer-image-dev` recipe that cross-builds
   in Docker (justfile:103's path) may be enough.
5. **Windows parent**: `builtin` ssh covers it; the Docker socket on Windows
   is a named pipe (`npipe:////./pipe/docker_engine`) — bollard supports it;
   verify in 58's next round.
6. **Should a local bot's approvals bypass the desktop when the user is in
   Slack?** Today the card goes everywhere; fine for v1.
7. **Signal binding** — same shape; when someone asks.
8. **Parallel episodes.** One active episode per bot is the person model and
   keeps "what is Ops doing" answerable; two Slack threads at once would
   want two episodes with one journal. Decide after watching real bots
   queue; the seam is `Bot.active_episode` → a set keyed by thread.
9. **Push from a node.** 42's token rides the device's handshake, so a
   phone that has connected to the node once can be woken by it; confirm
   against 42 §4 in step 8 and decide whether the parent should suppress
   its own duplicate push for the same item.
10. **Journal quality.** Whether the closing turn's *Done / Open / Next /
    Notes* is the right shape for months of work, or whether the journal
    wants sections the user names in the instructions. Measure on two bots
    for two weeks before changing the template.

## 18. Status

Design only (2026-09-23, v0.3). Nothing built; no crate exists. Recon sheets:
`scratch-always-on-recon-theuth.md` (405 lines) and
`scratch-always-on-recon-grok.md` (209 lines), both to be deleted when step 1
lands.
