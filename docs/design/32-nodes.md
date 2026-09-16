# 32 — Nodes: pair a harness to a parent harness

Status: design v0 · 2026-09-15 · implemented after 31 is built and its §12
checks pass · extends `31-link.md` (§4.3 handshake payload, §6.2 conn
dispatch, §6.6 `wth --link`, §7.3 harness store), `06-acp.md` (§3.2 spawn,
§4.2 "one process per client", §8.5 `acp` as a library), `18-app-launch.md`
(§5: a background service launches through the login shell),
`00-architecture.md` §4, §10, §12. Where 31 and this doc disagree on the
dependency order of `link`, §3.1 here wins. Recon (§2) is from the tree as of
2026-09-15.

Vocabulary. **Parent**: the harness the user chose as their main one; it has
devices (31) and nodes (this doc). **Node**: a harness on another machine,
paired to a parent so the parent — and every device of the parent — can run
sessions on it. **Fleet**: one parent, its devices, its nodes. Everything
else is 31's: harness, device, relay, link.

## 0. Decisions in one screen

1. **A node is an ordinary 31 harness; the parent is one of its devices.**
   The node runs `wth --headless` with `link.enabled: true`, registers at
   `/h/<node_id>`, and hands out a 31 §4.4 pairing string. The parent
   pastes that string, runs `Noise_IKpsk2` as the *initiator*, and is
   recorded in the node's `devices.yaml` as `{ platform: "wth" }`. Nothing on
   the node knows it is a node. No new relay op, no new frame, no new key:
   the parent's `link/identity` is its static in both roles (a WireGuard
   peer does the same). Rejected: the node dialling the parent as a device
   with requests flowing the other way (needs bidirectional dispatch on 31's
   conn); a second protocol beside link.
2. **Subagents on a node are ACP children, over the link conn.** A paired
   device may open a conn in *ACP mode*: the `Noise_IK` message-1 payload,
   empty in 31, carries `mode:u8` (`absent|0` = `Api`, `1` = ACP). On such a
   conn the node serves the same ACP agent half that `wth --acp` serves on
   stdio (`rpc::acp`, 06 §4). On the parent, `spawn_subagent{agent:
   "build-box"}` resolves the name to a node and `AcpChild` (06 §3.2) runs
   unchanged with a dial transport instead of a subprocess. 06 §4.2's "the
   fix is the same shim connecting to a running daemon" is this, without a
   shim. Rejected: mirroring the node's `AgentEvent`s with rewritten session
   ids (a second child kind, and the node's subtree leaks into the parent's
   id space; ACP children are leaves by construction).
3. **A user session on a node is the node's session, driven directly.** The
   device (desktop, phone, browser) connects to the node through the relay
   as 31 §7.3 already allows for any harness, and the switcher shows nodes
   nested under their parent. The parent's job is introduction, not proxying:
   `invite_node(id)` calls `link_pair` *on the node* over the parent's conn
   and returns the pairing string; the device pairs with the node itself, on
   first sight, without a click. Rejected: `send_input{node}` proxied through
   the parent — every `TextDelta` serialised twice, ~100 methods proxied, and
   the sessions would live in the wrong store.
4. **`wth --install-service` is how a node comes to exist.** One flag on the
   remote box: write a launchd LaunchAgent or a systemd user unit that runs
   `$SHELL -l -i -c 'exec wth --headless'` (18's environment argument, the
   pty dropped), set `link.enabled: true`, start it, wait for the port, then
   run 31 §6.6's `wth --link` step and print the key with the sentence to
   paste it into the parent. `wth --uninstall-service` reverses the unit and
   nothing else. Rejected: a `wth install service` subcommand (the binary's
   one-shots are flags: `--link-signal`, `--slack-setup`, `--acp`);
   `systemd-run`/Docker (15 is shelved); a Windows service (no build).
5. **The `nodes` plugin sits below `rpc`, beside `link`.** `rpc::Api` names
   `Node`, `NodeEvent`, and the `Nodes` actor, so `rpc` depends on `nodes`,
   not the reverse; what `nodes` needs from above arrives as messages
   (`RegisterNode` into `agent`, exactly the 24 `RegisterUserSink` shape).
   This also fixes 31's latent cycle (§3.1).
6. **The parent is the trust root of the fleet.** A parent can do anything on
   a node a phone paired to it could do; a device paired to the parent is,
   after auto-pairing, a device of every node. Revoking a device on the
   parent cascades to its nodes (one static per device → the same 8-char id
   on every harness, 31 §4.1). Compromise of the parent is compromise of the
   fleet; the doc says so once (§9) and does not build a second policy.
7. **Nothing is synced.** A node has its own `~/.wth`: config, credentials,
   skills, MCP servers, knowledge, sessions. The service installer creates
   the skeleton (`first_run`) and the user puts a key in `credentials.yaml`
   over the same ssh session that ran the installer. Rejected for now:
   credential/skill sync over link — a real want, a separate doc when
   someone has three nodes (§15).

## 1. Why

31 gives the console in a pocket against any harness the user pairs with.
It does not give the *model* another machine, and it makes every extra
machine a separate thing to pair every device with. The user wants one
machine to be the harness — the one with the credentials, the knowledge,
the phone paired — and to add compute (a Linux box with 32 cores, a Mac
mini under the desk, a cloud VM) that both they and the model can reach
through the same relay, with one pairing per machine, not one per
(machine, device).

**What already works with zero code, and why it is not enough.** An
`agents.yaml` entry `remote: { command: ssh, args: [box, wth, --acp] }`
makes `spawn_subagent{agent: "remote"}` run a child on `box` today (06
§3.1; `wth --acp` is a full harness on stdio). It needs ssh keys and an
inbound port on the box, gives no presence, no UI sessions, no pairing UX,
and the child's `wth --acp` is a second process with its own MCP servers
and no view in the box's `/sessions` (06 §4.2). Nodes are the relay path for
that entry plus the three things it cannot do.

## 2. Facts relied on (recon 2026-09-15)

### 2.1 Binary and plugins

- `crates/wth/src/main.rs:19-65`: one-shots are clap flags with
  `conflicts_with_all` (`--link-signal`, `--slack-manifest`, `--slack-setup`,
  `--acp`); `--headless` implies `--serve`. `:284-341`: three plugin vectors
  (link-signal, acp, default); `rpc::RpcPlugin` is pushed when
  `serve || headless`. `first_run(data_dir, config_path)` (`:176-218`) creates
  the skeleton and never overwrites. `wth subprocess` is the Python worker
  (08 §9) — the only argv shape that is not a flag.
- `runtime::write_config_value(path, section, key, value)` persists one key
  (31 §2.1). `WTH_LAUNCH=1` is the rc-file guard marker 18 §4 established;
  nothing in `wth` reads it.
- `agent/src/user.rs:31-83`: `UserSink` trait, `UserChannels` actor
  (`"user"`), `RegisterUserSink(Arc<dyn UserSink>)` — a plugin above `agent`
  hands `agent` a trait object at start (24 §3). `signal/src/lib.rs:368`,
  `slack/src/lib.rs:407` register at start. This is the pattern §6.5 copies.
- `rpc/Cargo.toml:11-17`: `rpc` depends on `agent`, `knowledge`, `signal`,
  `slack`, `acp`, `browser`, `computer`, `surface` — every plugin whose
  types `Api` names sits *below* `rpc`.

### 2.2 ACP (`crates/acp`, `crates/agent`, `crates/rpc`)

- `acp/src/harness.rs:82-94`: `Connect { config: AcpAgentConfig, cwd,
  resume: Option<String>, stderr_log }`. `:151-218` `Link::spawn` builds
  `AcpAgent::new(config)` — the SDK's subprocess `ConnectTo<Agent>` — and
  `Client.builder()…connect_with(agent, |cx| …)`. The mandatory catch-all
  `method_not_found` (06 §8.3) is at `:192-198`.
- `agent/src/acp_child.rs:40-58`: `AcpChild { id, name /* agents.yaml key */,
  store, rt, connect: Option<Connect>, harness, dead, queue, turn, titled }`;
  answers `UserInput`, `CancelTurn`, `IsRunning` like a native `Session`.
  06 §8.7: connection failure is delivered as `Error` + `TurnFinished
  {stop_reason: "error"}` and `<subagent_result stop="error">`.
- `agent/src/peers.rs:25` `SpawnSubagentArgs { task, name, agent, cwd }`;
  `supervisor.rs:180` `SpawnChild { parent, name, agent, cwd, … }`. Store
  columns `agent TEXT`, `acp_session TEXT`, `cwd TEXT` (06 §8.10).
- `rpc/src/acp.rs:45-70`: `AcpPlugin::start` spawns one `Relay` (bus
  subscriber for `AgentEvent`, `ApprovalRequested`; prompt waiters), then
  `connect(relay, data_dir)` on stdio and **`shutdown.cancel()` when it
  returns**. `:74-190` `connect` is `Agent.builder()…connect_with(Stdio::new(),
  …)`: handlers for `initialize`, `session/new`, `session/load`,
  `session/prompt`, `session/cancel`, `session/list`; `initialize` advertises
  `loadSession: true`, `embeddedContext`, `image`. The wth `SessionId`
  string *is* the ACP session id (06 §8.12).
- 06 §8.5: `acp` is a library with no dependency on `agent` or `tools`; the
  `AcpChild` actor lives in `agent` so it can reach the store and
  `AgentEvent`.

### 2.3 RPC and link (31, design)

- `rpc::Api` (`api.rs:76-81`): `info() -> Info { version, data_dir,
  default_model, approvals, compact_style }` (+ `name` from 31 §6.4);
  `new_session()`, `send_input(session?, …)`, `list_sessions()`,
  `list_agents(session)`, `list_acp/add_acp/remove_acp` (`:353-361`) — the
  naming convention for a managed list.
- 31 §4.3: `Noise_IK` message 1 `payload: empty`; message 2 `SessionHello {
  name, version }`. Both patterns share the message-1 layout; the harness
  parses once. 31 §4.5: 64 conns per harness on the relay, 16
  `link.max_devices`, one pending pairing per harness, 5 min, single use.
- 31 §6.2 conn task: handshake → records → `methods.raw_json_request` per
  request. 31 §6.6: `wth --link` = RPC client of the local daemon
  (`rpc.listen` + `<data_dir>/token`, jsonrpsee `ws-client`) that calls
  `link_pair`, prints QR + key, waits for `LinkPaired`, exits.
- 31 §7.2–7.4: device identity is one X25519 static per install; harness
  list in IDB `harnesses { id, name, relayHost, pairedAt, lastSeen, order }`;
  one `LinkTransport` + `RpcClient` per paired harness; only the active
  harness subscribes; stores keyed by harness id.
- 31 §3 says `… ← rpc ← link ← tui`, while 31 §6.4 has `rpc::Api` returning
  `LinkStatus`, `rpc::export` emitting `LinkStatus`/`Device`/`Pairing`/
  `LinkEvent`, and `rpc::bridge` forwarding `LinkEvent`. Those need
  `rpc → link`. §3.1 resolves it.
- jsonrpsee 0.26 `jsonrpsee-core` client: `ClientBuilder::build_with_tokio(
  sender, receiver)` over any `TransportSenderT`/`TransportReceiverT`;
  `ClientT::request::<R, _>(method, params)` — typed results by string
  method name, no `Api` trait needed `[INFERENCE: API surface from 0.26
  docs; the §11 step 3 check is the proof]`.

## 3. Shape

```
 parent (wth)                                                      node (wth --headless, service)
 ┌────────────────────────────────┐   wss /h/<node_id>   relay   ┌──────────────────────────────┐
 │ nodes plugin                   │ ───── control conn ───────── │ link plugin (31, unchanged   │
 │  nodes.yaml                    │      mode 0: Api             │  except mode byte)           │
 │  dial(node) → Noise IK init.   │ ───── acp conn per child ─── │  mode 0 → raw_json_request   │
 │  jsonrpsee client (info,       │      mode 1: ACP             │  mode 1 → rpc::acp::serve    │
 │   link_pair, link_revoke_…)    │                              │  devices.yaml: {parent: wth, │
 │  RegisterNode ──► agent        │                              │                phone, laptop}│
 │ agent: AcpChild(Dial) ◄────────┼──── session/update ───────── │  sessions.sqlite (the        │
 │ rpc: list/add/remove/rename/   │                              │   children live here)        │
 │      invite_node, NodeEvent    │                              └──────────────────────────────┘
 └────────────────────────────────┘                                          ▲
        ▲ devices (31): desktop, phone, browser ── switcher: Studio Mac ▸ build-box ──┘
          on first sight of a node: invite_node → IKpsk2 with the node → direct sessions
```

Deliverables:

| part | where | kind | depends on |
|---|---|---|---|
| service installer | `crates/wth/src/service.rs` (+ two flags) | bin code | `link` (the `--link` step), `runtime::write_config_value` |
| ACP over a link conn | `crates/link` (mode byte, conn dispatch), `crates/rpc/src/acp.rs` (`serve(transport)`) | amendments | 31 |
| nodes plugin | `crates/nodes/` | plugin (00 §4; `docs/plugins.md`) | `runtime`, `link` (identity, Noise, frames, dial), `acp` (`Dialer`), `agent` (`RegisterNode`), `jsonrpsee` (core client), `tokio-tungstenite`, `snow` |
| `Api` additions + bindings | `crates/rpc` | amendments | `nodes` |
| TUI `/nodes` | `crates/tui` | menu | `Api` |
| device UI | `crates/desktop/ui/src/link/`, `Settings › Nodes`, switcher | TS | 31 §7 |

### 3.1 Dependency order (amends 31 §3)

```
runtime ← tools ← knowledge ← agent ← { signal, slack, link, nodes } ← rpc ← tui ← wth
                                                    ▲        │
                                                    └────────┘  nodes → link (identity, dial, frames)
```

`link` cannot depend on `rpc` because `rpc::Api` names `link`'s types (31
§6.4). What `link` needs from `rpc` — the methods object for
`raw_json_request` and, now, the ACP server — arrives as a message from
`rpc::RpcPlugin::start` (after `serve`, on a spawned task:
`rt.lookup_when::<link::LinkActor>(LINK_ACTOR)` then `tell`). The
`MethodsFactory` that 31's build first threaded through `wth/main.rs` is
deleted: that was the binary handing one plugin another's handle (00 §1.4).

```rust
// link::types — tell on the "link" actor; one field per consumer
pub type AcpServe = Arc<dyn Fn(Runtime, RecordChannel) -> BoxFuture<'static, ()> + Send + Sync>;
pub struct SetDispatch {
    pub methods: jsonrpsee::Methods,   // mode 0: raw_json_request
    pub acp: AcpServe,                 // mode 1 (§4.3): rpc::acp::serve_records
}
/// One reassembled JSON-RPC message per item (no newline); dropped by the conn on close.
pub struct RecordChannel { pub inbound: mpsc::Receiver<Vec<u8>>, pub outbound: mpsc::Sender<Vec<u8>> }
```

The actor stores it in the shared `Ctx` (`OnceCell` + `Notify`); a conn
that handshakes before it arrives waits — no error path, the 10 s
handshake deadline (31 §4.5) bounds it. `link` depends on `jsonrpsee` for
the `Methods` type only. `--acp` runs without the link plugin, so
`AcpPlugin` sends nothing. `signal::qr` still moves to `link::qr`; `tui`
reaches it through the chain as before.

## 4. Protocol

Everything is 31 §4 with one byte added.

### 4.1 The parent as a device

The parent's device static is `<data_dir>/link/identity` — the same 32
bytes 31 §4.1 uses as its harness static. Its device id on every node is
therefore `base64url(pub)[..8]`, the same string 31 shows the parent's own
devices for *their* keys. Pairing (`IKpsk2`, `PairHello { name: link.name
or hostname, platform: "wth" }`) and sessions (`IK`) are byte-identical to
a phone's; `snow` plays the initiator (31 already links it for the
responder). Prologue `"wth-link-1" ‖ node_id`.

### 4.2 Conn mode

```
Noise_IK_25519_AESGCM_SHA256       prologue = "wth-link-1" ‖ harness_id
→ e, es, s, ss          payload: mode:u8 | empty          (31: empty)
← e, ee, se             payload: SessionHello { name, version }
```

| mode | dispatch on the harness | who opens it |
|---|---|---|
| absent, `0` | 31 §6.2: records → `raw_json_request` | every 31 device; the parent's control conn |
| `1` | records → ACP agent half (§4.3) | the parent, one per remote child |
| other | `CLOSE 3` | |

A mode is chosen per conn and never changes. A device that is not in
`devices.yaml` gets `CLOSE 3` before mode is read, as in 31. Mode `1` is
open to *any* paired device: it grants nothing `send_input` over mode `0`
does not already grant.

### 4.3 ACP over a conn

Inside a mode-1 conn the records (31 §4.3: `flags:u8 · bytes`, FIN
reassembly) carry ACP's JSON-RPC exactly as stdio would: one JSON-RPC
message per record, no line framing needed because the record *is* the
frame. On the harness the conn task reassembles records into a `Duplex`
`(impl AsyncRead, impl AsyncWrite)` and calls `SetDispatch.acp(duplex)`;
that future is `rpc::acp::serve(transport, relay, data_dir)` (§5.3). On the
parent, `acp::Harness` connects the SDK's `Client` over the same kind of
duplex (§6.5). The SDK's `Stdio` is a `ConnectTo` built from a reader and a
writer; the same constructor takes ours `[INFERENCE: agent-client-protocol
2.1 transport constructor name; if it only accepts process stdio, a
`tokio::io::duplex` pair with two copy tasks costs one extra copy per
message and nothing else — settled in §11 step 2]`.

A conn closing (relay `CLOSE`, node offline, revoke) is EOF to the SDK on
both ends: the parent's `AcpChild` sees the 06 §3.3 "transport EOF"
row (`Error`, `TurnFinished error`), the node's ACP connection future
returns and its sessions stay in the node's store for `session/load`.

### 4.4 Invites

`invite_node(id)` on the parent is a plain `link_pair()` request over the
control conn to the node, returned verbatim (`Pairing { key, expires_at_ms
}`). The device then pairs with the node exactly as it pairs with any
harness (31 §7.5 device side, paste path, no UI). Consequences to state:
one pending pairing per harness (31 §4.5), so two devices seeing a new node
in the same minute race; the loser's `IKpsk2` fails with `CLOSE 3` and it
retries once after a random 1–5 s. The node's TUI, if one is open, shows
the 31 §6.5 QR card for five minutes — harmless and true.

## 5. Node side

### 5.1 `wth --install-service`

```
wth --install-service [--relay <wss url>] [--name <display name>] [--no-pair]
wth --uninstall-service
```

Both conflict with every other mode flag (`headless`, `serve`, `acp`,
`prompt`, `link_signal`, `slack_*`). Steps, each idempotent and re-runnable:

1. `first_run(data_dir, config)` (already runs for every invocation).
2. `write_config_value("link", "enabled", true)`; `--relay` →
   `link.relay`; `--name` → `link.name` (31 §6.1 keys). Without `--relay`
   the configured or default relay stands.
3. Write the unit for this OS, from `current_exe()` canonicalised, the
   user's shell from `passwd` (18 §2; fallback `/bin/zsh` on macOS,
   `/bin/sh -l -c` elsewhere), and `data_dir`:

   **macOS** — `~/Library/LaunchAgents/wth.plist`:

   ```xml
   Label            wth
   ProgramArguments [<shell>, -l, -i, -c, exec '<wth>' --headless --data-dir '<data_dir>']
   EnvironmentVariables { WTH_LAUNCH: 1, TERM: xterm-256color }
   RunAtLoad true · KeepAlive { SuccessfulExit: false } · ThrottleInterval 5
   StandardOutPath / StandardErrorPath  <data_dir>/logs/service.log
   ```

   `launchctl bootout gui/$UID/wth` (ignore "not found") then `launchctl
   bootstrap gui/$UID <plist>`. A LaunchAgent runs while the user is
   logged in; the installer prints one line saying so and pointing at
   System Settings › Users › Login Options › automatic login for a Mac
   nobody sits at.

   **Linux** — `~/.config/systemd/user/wth.service`:

   ```ini
   [Unit]
   Description=wth harness
   After=network-online.target
   [Service]
   ExecStart=<shell> -l -i -c "exec '<wth>' --headless --data-dir '<data_dir>'"
   Environment=WTH_LAUNCH=1 TERM=xterm-256color
   StandardInput=null
   Restart=on-failure
   RestartSec=2
   [Install]
   WantedBy=default.target
   ```

   `loginctl enable-linger $USER` (so the user manager runs without a
   login session; prints the `sudo` command if it fails), `systemctl --user
   daemon-reload`, `systemctl --user enable --now wth`.

   Why `-l -i` without a pty: 18's argument (the daemon must have the
   environment a terminal would: `PATH` for `npx`/`uvx` MCP servers,
   `NODE_EXTRA_CA_CERTS`, tokens) holds on a server; the pty was for the
   GUI case where nothing else could show the transcript. Under a service
   the transcript is the journal / `service.log`. `zsh -i` without a tty
   prints a job-control warning once; `bash -i` says "no job control" —
   cosmetic, noted so nobody "fixes" it. An rc file that waits for input
   fails fast on `StandardInput=null` instead of hanging (18 §4).
   `ponytail:` if a real rc file needs a real tty, the upgrade is the unit
   running `wth --headless --via-shell` which re-execs itself through
   `portable-pty` (22) exactly as 18 §2 — kept out until a log shows it.

   Anything else (FreeBSD rc.d, OpenRC, Windows): print the exact
   `ExecStart` line and `link.enabled` was set; exit 2.

4. Wait for `rpc.listen` to accept with `<data_dir>/token` (≤ 15 s, 25 §1
   step 3's probe). Timeout → print the last 20 lines of `service.log` and
   exit 1 (the unit stays installed).
5. Unless `--no-pair` or stdout is not a TTY: run 31 §6.6's `--link` step
   in-process (same function), with this banner before the QR:

   ```
   service installed: launchd gui/501/wth · relay relay.wth.rwx.dev · name build-box
   pair this harness to your main one — on it, run  /nodes add <key>
   or paste the key in Settings › Nodes › Add node, or scan with the phone app:
   <QR>
   <key>
   waiting for the pairing… (Ctrl+C to skip; later: wth --link)
   ```

   `LinkPaired { device.platform == "wth" }` → `paired to Studio Mac as a
   node`; a phone or laptop pairing instead → `paired: Sky's iPhone` (a
   device of this harness, 31) and keep waiting for a parent until the
   pairing expires. Exit 0.

`--uninstall-service`: `launchctl bootout` + remove the plist, or
`systemctl --user disable --now wth` + remove the unit + `daemon-reload`.
`data_dir`, `link/identity`, `devices.yaml` untouched — the harness can be
run by hand or re-installed and every pairing survives.

### 5.2 What the node keeps

Nothing new on disk. `devices.yaml` has a `{ name: "Studio Mac", platform:
"wth" }` row; `/link` and Settings › Link show it with the others; revoking
it there unpairs the parent (its control conn and every ACP conn get
`CLOSE 3`, the parent marks the node `revoked`). Remote children are rows in
the node's `sessions.sqlite` with `cwd` set (06 §8.10), so the user who
switches to the node sees them in `/sessions` and can read their
transcripts — 06 §4.2's blind spot closed.

### 5.3 `rpc::acp::serve`

`connect(relay, data_dir)` (`acp.rs:74-190`) becomes
`serve(transport: impl ConnectTo<Client>, relay: ActorRef<Relay>, data_dir)`
returning when the transport ends, *without* touching `shutdown`.
`AcpPlugin::start` calls it on `Stdio::new()` and cancels shutdown after, as
today. `RpcPlugin::start` spawns one `Relay` per process (registered
`"acp.relay"`, mailbox `subscriber_mailbox()`; the one `AcpPlugin` spawns
is the same actor under the same name — whichever plugin starts first) and
sends `SetDispatch` to `"link"` with `acp = |duplex| serve(duplex, relay,
data_dir)`. `Relay` today holds one `cx: Option<ConnectionTo<Client>>`; with
N conns it holds `HashMap<SessionId, ConnectionTo<Client>>` filled by
`session/new`/`session/load` on each connection, so `session/update` and
`request_permission` go to the connection that owns the session, and the
prompt waiters stay keyed by session as they are. A session whose
connection is gone drops its updates (the node's own bus still has them).

## 6. Parent side: `crates/nodes`

### 6.1 Config

```yaml
nodes:
  enabled: true        # false: no dials, nodes invisible to the model and the UI; nodes.yaml kept
```

Relay per node comes from its pairing string (31 §4.4 derives the
WebSocket URL from the host in the string), not from config: nodes on two
relays are fine.

### 6.2 Actor, commands, events

Actor `"nodes"`, one per process.

```rust
// commands (ask)
Status  -> Vec<Node>
Add     { key: String, name: Option<String> } -> Result<Node, String>   // 31 §4.4 string; runs IKpsk2 as initiator
Remove  { id: String } -> ()             // closes conns, deletes the row, best-effort link_revoke_device(self) on the node
Rename  { id: String, name: String } -> ()                              // local alias
Invite  { id: String } -> Result<Pairing, String>                       // link_pair on the node (§4.4)

pub struct Node { id: String /* b64url(pub), 43 chars */, name: String, relay: String,
                  platform: String, version: String, paired_at_ms: u64, last_seen_ms: u64,
                  online: bool, children: u32 /* live ACP conns */ }

// events (bus, forwarded by rpc::bridge)
#[serde(tag = "type")]
pub enum NodeEvent {
    NodeAdded   { node: Node },
    NodeOnline  { id: String, online: bool },
    NodeRemoved { id: String },
    NodeRevoked { id: String },   // the node deleted us from its devices.yaml
}
```

`Add`: parse the string; dial `wss://<host>/h/<Hs>`; `IKpsk2` with
`PairHello { name: link.name or hostname, platform: "wth" }`; on
`PairAccept` write the row (name from the accept unless `name` given), open
the control conn (§6.3), publish `NodeAdded`. Errors are 31 §7.5's three
(expired/used key, harness offline, relay unreachable) plus "this key is
our own harness" (`Hs == identity.pub`) and "already paired". A node name
that collides with an `agents.yaml` entry is refused (`§7`, one
namespace).

### 6.3 Control conn and presence

One WebSocket per node to `/h/<id>` kept open for the process lifetime,
exactly the device model of 31 §7.1 in Rust: parked while the node is
offline (`PRESENCE 0` → `online: false`), `IK` mode 0 on `PRESENCE 1`, then
`info()` → `platform`, `version`, `name` refresh; `NodeOnline` on every
transition; `last_seen_ms` written on close. Backoff 1 s → 30 s ×2 ±20 %
(31 §6.2). `CLOSE 3` on a conn whose `IK` used to succeed → `NodeRevoked`,
row kept with `online: false` and a `revoked` flag so the UI can say why.

Requests on the control conn use jsonrpsee's core client over a
`TransportSenderT`/`TransportReceiverT` pair wrapping the record codec (one
JSON-RPC message per record, as 31 §4.3), so `client.request::<Info,
_>("info", rpc_params![])` is typed without `rpc::Api` (§3.1). The method
names `nodes` uses are a `const METHODS: [&str; 4] = ["info", "link_pair",
"link_revoke_device", "link_status"]`; a unit test checks each is in
`bindings/methods.json`, so an `Api` rename fails a test instead of a
node.

### 6.4 Data on disk

```
<data_dir>/nodes.yaml   - { id, pub: b64url, name, relay, platform, version,
                            paired_at_ms, last_seen_ms, revoke_pending: [device ids] }
```

May be deleted: every node then lists a device (`Studio Mac`) that never
returns, revoked there by hand. `revoke_pending` (§8.4) is the only
non-derivable state.

### 6.5 The model's path: `RegisterNode`, `Dialer`, `AcpChild`

`acp` (library, 06 §8.5) gains the transport seam:

```rust
// crates/acp
pub type Duplex = (Box<dyn AsyncRead + Send + Unpin>, Box<dyn AsyncWrite + Send + Unpin>);
#[async_trait] pub trait Dialer: Send + Sync { async fn dial(&self) -> Result<Duplex, String>; }
pub enum Transport { Process(AcpAgentConfig), Dial(Arc<dyn Dialer>) }
pub struct Connect { pub transport: Transport, pub cwd, pub resume, pub stderr_log }  // was: config
```

`Link::spawn` matches: `Process` → `AcpAgent::new(config)` as today;
`Dial` → `dialer.dial().await?` then the SDK client over the duplex.
`probe` takes a `Transport` too (`/nodes` shows `ok`/`offline` the way
`/acp` shows probe results).

`agent` gains, on the `Agents` supervisor (mirror of `RegisterUserSink`,
24 §3):

```rust
pub struct RegisterNode { pub name: String, pub describe: String /* "linux · wth 0.4.1" */, pub dialer: Arc<dyn acp::Dialer> }
pub struct UnregisterNode { pub name: String }
```

`nodes` registers each node on `NodeAdded`/`NodeOnline{true}` and
unregisters on `Remove`/`NodeRevoked`; an offline node stays registered
with `describe` ending in `· offline` so the model is told rather than
surprised, and its `dial` fails with `"<name> is offline"`.

`SpawnChild { agent: Some(name) }` resolves `agents.yaml` first, then the
registered nodes (`Transport::Dial`), else the tool error lists both. The
`Dialer` for a node opens a *new* WebSocket to `/h/<id>`, runs `IK` with
`mode = 1`, and returns the record duplex — one conn per remote child
(`ponytail:` 64 relay conns per harness minus devices is the ceiling;
multiplex children over one ACP conn via `session/*` ids if a node ever
runs sixty). Everything after the dial is 06: `initialize` → `session/new
{cwd}` (or `session/load` on respawn — the node advertises `loadSession`),
`session/prompt`, `session/update` → `AgentEvent` on the parent's session
id, `request_permission` → `ExternalApproval` → the parent's approval
modal on whichever UI answers first, `send_message` → second prompt,
`Cancel` → `session/cancel` (the 5 s kill becomes closing the conn),
`<subagent_result>` delivery. `TurnStarted.model` shows `build-box`; the
`/agents` board shows `(build-box)` (06 §8.10). The `stderr_log` path is
created and stays empty for dial transports.

### 6.6 RPC additions (`rpc::Api`; `docs/plugins.md` §4 step 2)

| method | → | for |
|---|---|---|
| `list_nodes()` | `Vec<Node>` | Settings › Nodes, `/nodes`, the switcher |
| `add_node(key, name?)` | `Node` | paste path; the phone's "as a node of…" (§8.3) |
| `remove_node(id)` | `()` | |
| `rename_node(id, name)` | `()` | |
| `invite_node(id)` | `Pairing` | device auto-pair (§8.2), `/nodes` QR |
| `NodeEvent` | bridge `forward!` + `export.rs` | live rows |
| `Info.platform: String` | | `std::env::consts::OS`; the node's `describe` line and the UI's icon |

`export.rs` gains `Node`, `NodeEvent`; `bindings/methods.json` the five
names.

### 6.7 TUI: `/nodes`

`COMMANDS` row `("/nodes", "[add <key>|rm <n>|name <n> <name>|invite <n>]",
"machines this harness can run sessions on")`; `Menu::Nodes` (modal):

```
 nodes · 2 paired
 ● build-box   linux   wth 0.4.1   2 running   paired 3d ago
 ○ mac-mini    macos   wth 0.4.1   seen 2h ago
 ─────────────────────────────────────────────
 a add (paste key)   i invite (QR for a phone)   r rename   x remove   Esc
```

`a` → the existing `Prompt` input → `add_node`; `i` → `invite_node` → the
31 §6.5 QR card with the node's name in the header (a phone that scans it
pairs with the node directly). Driving a node's session *from this TUI* is
not built: the TUI's `Api` handle is in-process and its events are a bus
subscription (00 §8); `ssh box wth` is the terminal answer and the desktop
and phone are the switcher answer (§8). Revisit when a transcript shows a
user wanting it.

## 7. What the model sees

`spawn_subagent`'s description (06 §3.2) appends one more line after the
external harnesses:

```
Nodes (wth on other machines, same tools, their own cwd and credentials): build-box — linux · wth 0.4.1; mac-mini — macos · offline
```

Names share one namespace with `agents.yaml` so the model has one field to
fill; a clash is refused at `add_node` (§6.2). `cwd` is a path on the node.
Everything else the model knows about subagents (01 §2, 19) holds: the
child is a leaf, results arrive as `<subagent_result>`, `send_message` by
name, `max_concurrent`/`max_runtime_secs`/`reply_timeout_secs` apply, a
result's text is untrusted content like any other child's (28). No remote
`shell`: the subagent is the unit of remote work, by construction.

## 8. Device UI (`crates/desktop/ui`)

### 8.1 Switcher

31 §7.5's switcher lists paired harnesses flat. A harness entry in IDB
gains `parent?: string`; the list nests children under their parent with
one indent and the same presence dot:

```
 ● Studio Mac ▾
   ● build-box
   ○ mac-mini        offline
 ● laptop
 + Add harness…   Manage…
```

Selecting a node is 31's switch (store swap, remount, subscribe). The
sidebar then shows the *node's* sessions; the header says `build-box ·
via Studio Mac`. New session, composer, approvals, jobs — all the node's,
over the device's own conn. Nodes of a node appear when that node is
active; the tree is not flattened.

### 8.2 Auto-pair

On connect to a harness, after `info`: `list_nodes()` (a harness without
the `nodes` plugin answers method-not-found → skip). For each node id not
in IDB `harnesses`: `invite_node(id)` → the 31 §7.5 paste path with the
returned key, silently → row `{ id, name, relayHost: from the key, parent:
<this harness> }`. Failures: node offline → row with `parent` and
`presence: 'offline'`, retried on `NodeOnline{true}` from the parent's
event stream; pairing race (§4.4) → one retry after 1–5 s; else the row
shows `pairing failed · retry` under the node. A node paired directly by
scan earlier has the same id → `parent` is set on the existing row, no
second pairing.

### 8.3 Settings › Nodes (parent side) and the phone

A tab beside Settings › Link, available when the active harness answers
`list_nodes`:

```
 Nodes
 Machines this harness and your devices can run sessions on.
 ● build-box   linux   wth 0.4.1   2 running   paired 3 d ago   [QR] [rename] [remove]
 ○ mac-mini    macos   seen 2 h ago                              [QR] [rename] [remove]
 Add node   [ paste the key printed by `wth --install-service` ]  [Add]
```

Phone: the `/link` route (31 §7.5) already parses a scanned or opened
pairing string. When the phone has an active harness that answers
`list_nodes`, the route asks first:

```
 Pair build-box
 ○ as a harness on this phone
 ● as a node of Studio Mac        (this phone connects to it too)
```

The second choice calls `add_node(key)` on the parent, then waits for
`NodeAdded` and auto-pairs (§8.2). One dialog, no other UI.

### 8.4 Revoke cascade

`link_revoke_device(id)` on the parent publishes `LinkRevoked{id}` (31
§6.2); `nodes` subscribes and calls `link_revoke_device(id)` on every
online node, appending `id` to `revoke_pending` for offline ones and
flushing on their next connect. The parent's Settings › Link revoke copy
becomes "…and on its N nodes". A device's own "Forget" (31 §7.5) stays
local, as before. Removing a node (`remove_node`) does not revoke the
user's devices from it — they paired with it themselves; the copy says so.

## 9. Threat model (adds to 31 §10)

| threat | answer |
|---|---|
| Parent compromised | every node is reachable as a device: same blast radius as the parent's own `data_dir` plus each node's. Stated in Settings › Nodes. Node-side mitigation is 31's: revoke the parent in the node's `/link` |
| Node compromised | it sees the tasks the parent sends it and can put text in `<subagent_result>` — untrusted content (28), as every child's result is. It cannot reach the parent's `Api`: the parent is the initiator, the node has no static of the parent's devices |
| Stolen pairing key from a terminal / scrollback | 5-min TTL, single use (31 §4.5); the node's TUI/`/link` shows who paired (`platform: wth`, name); revoke there |
| A device coerces `invite_node` to reach a node | only a device already paired to the parent can call it, and the parent could run anything on the node anyway; the invite grants the same |
| Relay learns the fleet shape | it sees that `/h/<node>` has a device whose IP is the parent's and sockets from the same phones — 31 §5.6's disclosure, now "your devices, this harness and its nodes" |
| A node's own `link.confirm` (31 §10, optional) | applies to the parent's pairing too: the node's operator clicks Allow on the node; the installer prints "confirm on the node" |
| `max_depth` bypass by chaining nodes | a node's child is `depth+1` in the *parent's* store; on the node it is a top-level session that may spawn to its own depth; the fleet's effective depth is `parent max_depth + node max_depth`. Documented, not prevented: the node's operator owns the node's limits |

## 10. Limits and budgets

| what | value | why |
|---|---|---|
| conns per node from one parent | 1 control + 1 per live remote child ≤ `agent.subagents.max_concurrent` (8) | 31 §4.5's 64 leaves room for 16 devices |
| remote child event latency | ≤ 31 §11's phone number + one ACP translation (`Relay` → `AcpChild`), measured once on LAN | the only doubly serialised path |
| control conn traffic | zero bytes at rest; `info` on connect, `link_pair` per invite, `link_revoke_device` per revoke | no polling, no event subscription |
| `--install-service` to `paired:` | ≤ 30 s on a box that resolves the relay | 15 s port wait + one handshake |

## 11. Phases and checks

After 31 §12 is green. Each ends with a runnable check.

| step | deliverable | check |
|---|---|---|
| 1 | 31 amendments: `link` below `rpc`, `SetDispatch`, mode byte, `Info.platform` | 31 step-2 bun script unchanged (empty payload) passes; a conn with mode `7` gets `CLOSE 3`; `link_status` over the wire still types |
| 2 | `rpc::acp::serve(transport)`, `Relay` per process with per-session connections, `link` mode 1 → `serve` | the bun script opens mode 1: `initialize` → `session/new` → `session/prompt` streams `agent_message_chunk`, a `shell` call raises `request_permission`, `allow_once` runs it, `end_turn`; a second mode-1 conn from the same device runs a second session concurrently and each conn only sees its own updates; the node's `/sessions` lists both |
| 3 | `crates/nodes`: identity reuse, `Add` (`IKpsk2` initiator), control conn + presence, jsonrpsee client over records, `nodes.yaml`, `RegisterNode`/`Dialer`, `acp::Transport`, `Api` additions, bindings | two headless `wth` on one machine (two data dirs, two ports) through a local relay: parent `add_node(key)` → `NodeAdded`; `spawn_subagent{agent:"node-b", task:"print the hostname and list ~"}` → child rows `(node-b)` in the parent's `/agents`, `ApprovalRequested` on the parent, allow, `<subagent_result>` with node-b's home listing; `send_message` follow-up answered from context; `kill` node-b mid-turn → `TurnFinished error` on the parent, `NodeOnline false`; restart → `NodeOnline true`, `send_message` respawns via `session/load`; revoke the parent in node-b's `devices.yaml` → `NodeRevoked`; `METHODS` ⊂ `methods.json` test |
| 4 | `wth --install-service` / `--uninstall-service` (launchd, systemd), `first_run`, config writes, port wait, `--link` step | over ssh to a Linux box: `wth --install-service --relay wss://…` prints the banner and QR; `/nodes add <key>` on the parent → `paired to Studio Mac as a node` on the box; `sudo reboot` → node back online with no login; a child spawned there sees the rc-file `PATH` (an `npx` MCP server in the node's `mcp/` connects); `--uninstall-service` → unit gone, `devices.yaml` intact, `wth --headless` by hand re-pairs nothing. Same on a Mac mini with automatic login |
| 5 | TUI `/nodes` | add, rename, remove, invite (QR scannable by the phone → the phone lists the node as a harness) |
| 6 | UI: `parent` in IDB, nested switcher, auto-pair, Settings › Nodes, phone `/link` choice, revoke cascade | desktop connected to the parent shows `build-box` under it within 2 s of first connect with no click; switch → node's sessions; new session streams; switch back with no event bleed (31 step 4's fixture); phone scans a node's QR → "as a node of Studio Mac" → row appears on the desktop too; revoke the phone on the parent → the phone loses the parent *and* the node (`revoked by Studio Mac`) |
| 7 | docs: 00 §10/§12, 31 §3/§4.3/§6.2 amendments, 06 §4.2 note, README `## Nodes` (install on a server in three lines) | docs and tree agree |

## 12. Status (as-built v0, 2026-09-16)

Shipped in three commits after 31: foundations (`link` `SetDispatch` +
mode byte + `initiator`, `rpc::acp::serve_records` + per-connection `Relay`,
`acp::Transport::{Process, Dial}` + `Dialer`, `agent::RegisterNode`,
`wth --install-service`/`--uninstall-service`), then `crates/nodes` + the
five `Api` methods + TUI `/nodes` + device UI, then docs. Deviations from
the design text are listed under §13.

Gates: `cargo clippy --workspace --all-targets -D warnings`, `cargo test
--workspace`, `just ts-desktop` (bindings gained `Node`, `NodeEvent`,
`Info.platform`, five method names), UI `tsc -b`, vitest 16 files / 95
tests, `vite build` — all green.

Verified live (one Mac, local `wth-relay --listen 127.0.0.1:8787 --web`,
two headless `wth` on separate data dirs, `claude-haiku-4-5`):

- **Step 1/2**: the 31 device script (`link-smoke.ts`) still pairs and
  gets `hello:` + `list_sessions:` after `SetDispatch` replaced the
  `main.rs` factory. Mode 1 is exercised end to end by step 3 (the parent's
  `Dialer` opens it).
- **Step 3**: `add_node(key)` from node-b's `link_pair` → `NodeAdded`,
  `online: true` within 3 s; node-b's `link_status` lists `parent-a / wth /
  online`. `invite_node` returned a fresh node pairing. `send_input` on the
  parent → the model called `spawn_subagent{agent: "node-b"}` →
  `SubagentSpawned`, child `TurnStarted { model: "node-b" }`; node-b's own
  bus showed the child's `TurnStarted`; node-b's `sessions.sqlite` holds the
  child's row with a `shell` call (`echo NODE-OK-$(hostname) && pwd` →
  `NODE-OK-NAMGQ1Y4930L0`, cwd recorded) and the assistant text; the parent
  received `<subagent_result>` as a new turn; `list_agents` shows
  `brisk-finch (node-b)`; `list_nodes().children == 1` while the child
  lived. `link_revoke_device(parent)` on node-b → parent saw `NodeOnline
  false` then `NodeRevoked`, row `online: false, revoked: true`;
  `remove_node` emptied the list. `METHODS ⊂ methods.json` unit test.
- **Step 4** (macOS only): `wth --install-service --no-pair --relay … --name
  svc-test` on a scratch data dir wrote `link.{enabled,relay,name}`, wrote
  `~/Library/LaunchAgents/wth.plist` (`/bin/zsh -l -i -c exec … --headless`,
  `WTH_LAUNCH=1`, `service.log`), bootstrapped `gui/501/wth` (launchctl
  `state = running`), the relay's `uplinks` went 2 → 3 within 2 s; exit 0.
  `--uninstall-service` booted it out, removed the plist, uplinks 3 → 2,
  `link/identity` intact. Not exercised: the pairing step of the installer
  (`--no-pair`), Linux/systemd, reboot survival, the rc-file `PATH` check
  under launchd (18 §6 covered the same exec line).
- **Step 5**: TUI on a pty: `/nodes add <key>` → `paired node node-b`,
  header badge `nodes ●1`, `/nodes` menu row `● node-b macOS wth 0.1.0
  paired 4s ago`, `i` → `⌁ node invite · node-b` QR card counting down
  from 4:59. Rename/remove verbs unit-tested only.
- **Step 6**: browser container (relay-served build, fresh IndexedDB):
  pasted the parent's key → landed in chat; within seconds the device had
  paired with node-b **with no click** — node-b's `devices.yaml` gained
  `Chrome / web`, IDB `harnesses` had `node-b { parent: <parent id> }`;
  switching to node-b showed the header `node-b · via parent-a` and the
  node's session list (the remote child's session); Settings › Nodes on the
  parent rendered `● node-b · macos · wth 0.1.0 · paired 9m ago` with QR /
  rename / Remove and the disclosure sentence; on a harness without nodes
  the tab shows the empty-state copy. Not exercised: the phone "as a node
  of" choice (no second pairing string scanned), `pairing failed · retry`,
  revoke cascade in the UI (`revoke_pending` unit-tested), iOS.
- **Step 7**: docs and tree agree (§13).

Numbers (§10 row 2) not measured: the remote child's events reached the
parent's subscriber well under a second on loopback; no instrumented run.

Open: Linux service install on a real box; the §10 latency number; UI
revoke-cascade copy on a live revoke.

## 13. Amendments (applied 2026-09-15; same commit series)

- **31 §3**: dependency order becomes §3.1 here; `link` depends on
  `jsonrpsee` (type) and not on `rpc`; `SetDispatch` replaces "methods
  object: `rpc::api::Server { … }.into_rpc()`" inside `link`. **31 §4.3**:
  `IK` message-1 payload `mode:u8 | empty`. **31 §6.2**: conn task
  dispatches by mode. **31 §4.1**: the identity is also the device static
  when this harness pairs *with* another. **31 §7.3**: `Harness.parent?`;
  **31 §7.5**: nested switcher, `/link` route choice, Settings › Nodes
  beside Link; revoke copy.
- **06 §3.2/§5**: `Connect.transport: Transport` (was `config`); `Dialer`;
  `probe(Transport)`. **06 §4.2**: the in-daemon ACP server over a link
  conn is the "shim" — `wth --acp` on stdio stays for editors. **06 §3.8**:
  MCP back-channel note unchanged (a remote child still cannot ask its
  parent).
- **00 §4**: `RegisterNode` joins `RegisterUserSink` as the second
  "higher plugin hands `agent` a trait object" message; the sentence about
  it names both. **00 §10**: `nodes/  plugin: paired remote harnesses,
  control conns, dial transport for ACP children (32)`; `wth/src/service.rs`.
  **00 §12**: row `nodes (32)`, §11 here as its check column. **00 §2**: a
  second sentence after "How a GUI starts the daemon": how a server does
  (`--install-service`, 32 §5.1).
- **18 §5**: the LaunchAgent rejection is scoped to the desktop app's daemon;
  32 §5.1 is the background-service case that paragraph reserved, launched
  through the login shell as it required.
- **`docs/plugins.md` §1 table**: "a peer this harness *dials* and
  proxies into `agent` as a harness source" → plugin (`nodes`); note that
  a plugin needing a handle only a higher plugin can build receives it as
  a message (`SetDispatch`, `RegisterNode`), never by depending upward.
- **README**: `## Nodes` — `ssh box; curl … | sh; wth --install-service;
  paste the key into /nodes add`.

**Deviations** (the tree over the text above; §12 records the smoke):

- **§3.1 / 31 §6.2**: `SetDispatch { methods, acp }` also carries the ACP
  server and replaces the `MethodsFactory` `wth/main.rs` threaded through
  31's first build (`link::types`, `rpc/src/lib.rs`).
- **§4.3, §5.3**: the duplex is not `AsyncRead`/`AsyncWrite`. `AcpServe`
  is `Fn(Runtime, RecordChannel)`; `RecordChannel` is a pair of `mpsc`
  channels of `Vec<u8>`, one JSON-RPC message per item; the public entry is
  `rpc::acp::serve_records(rt, inbound, outbound)`, which wraps both ends in
  the SDK's `Lines::new(sink, stream)` and calls the private
  `serve(transport, relay, data_dir)`. The `[INFERENCE]` in §4.3 is settled:
  no `tokio::io::duplex`, no copy tasks. On the parent `acp::Duplex` is
  `acp::LineDuplex { outgoing: Sink<String>, incoming: Stream<String> }`
  for the same reason. `rpc::acp::relay(rt)` is the idempotent getter of
  the process-wide `Relay` (`"acp.relay"`), whichever plugin asks first.
- **§5.3**: `Relay` keys connections by a per-`serve` counter
  (`connections: HashMap<u64, ConnectionTo<Client>>`, `owners:
  HashMap<SessionId, u64>`) rather than `HashMap<SessionId,
  ConnectionTo<Client>>`, so a connection's sessions are unbound in one
  pass when it ends.
- **§5.1 step 3**: the shell is `$SHELL` when it names an executable, not a
  `passwd` lookup; the fallback (`/bin/zsh` on macOS, `/bin/sh` elsewhere)
  runs `-l -c` without `-i` on both. `--relay` must start with `ws://` or
  `wss://`. Linux runs `systemctl --user enable` then `restart`, not
  `enable --now`: a re-run on a live service must pick up the unit and
  config it just rewrote. `--uninstall-service` (`service::uninstall()`)
  takes no `data_dir` — it only needs the unit path.
- **§5.1 step 5**: a pairing is single use (31 §4.5), so after a phone or
  laptop pairs instead of a parent the installer issues a fresh key and
  keeps waiting on that one (`link_cli::pair(client, wait_for_parent =
  true)`); "until the pairing expires" applies to the reissued key.
- **§6.2**: `Node` has a `revoked: bool` beside `online` (the UI's "why").
  It is not persisted (`Row.revoked` is `#[serde(skip)]`): a restart retries
  the `IK` once, and a still-revoked node flips it back. `Add` refuses a
  name that collides with another node as well as with `agents.yaml`. The
  pairing conn is not reused as the control conn: `info` is sent on it as
  the enrolling first record (31 §4.3), it is dropped, and the control task
  opens its own `IK` mode-0 session. `Rename` publishes no event; UIs
  refetch `list_nodes` after the call.
- **§6.3**: the control conn is not silent at rest. The relay closes a
  device socket idle for 600 s (`relay.rs` `DEVICE_IDLE`, `CLOSE 2`), so
  the control task sends `link_status` every 300 s (`KEEPALIVE`); a failed
  keepalive ends the session and the backoff loop reconnects.
- **§6.5**: a node is registered with `agent` from `Add` on — the control
  task's first act — with `describe` ending `· offline` until its conn is
  up, then re-registered on every presence flip and on `Rename`;
  unregistered on `Remove` and `NodeRevoked`. The `Dialer` refuses before
  dialling when the presence bit is off (`"<name> is offline"`), and waits
  up to 10 s for `PRESENCE 1` on the fresh socket otherwise.

## 14. Rejected

- **Node dials the parent; parent issues requests back over that conn.**
  31's channel is request/response from the device; reversing it needs a
  dispatcher on the device side and a second registration. The parent as
  a device reuses 31 to the byte. §0.1.
- **Parent proxies node sessions into its own `Api`** (`send_input{node}`,
  merged `list_sessions`): every delta serialised twice for *user*
  sessions, ~100 methods to proxy and keep in step, and the session rows
  would live away from the machine that ran them. Direct connection is
  what 31 §7.3 already does for N harnesses. §0.3.
- **`Api`-level children instead of ACP** (`new_session` + `send_input` +
  `subscribe_events(descendants)` with session-id rewriting): higher
  fidelity (`SubagentSpawned`, `ModelChanged` survive), but a second child
  actor beside `AcpChild`, id maps for the node's subtree, and approvals
  routed by hand. ACP has all of it built and smoked (06). §0.2.
- **One multiplexed ACP conn per node**: `AcpChild` is one connection per
  child today; conns are cheap and per-child close is per-child
  lifecycle. Noted as the upgrade in §6.5.
- **A `wth install service` subcommand**: the binary's one-shots are flags;
  `wth subprocess` is the worker, not a convention. §0.4.
- **Running the service through `portable-pty`** (18 §2 verbatim): the pty
  existed so a GUI could show the shell's transcript; a service has a
  journal. Kept as the `ponytail:` upgrade in §5.1.
- **Syncing credentials, skills, knowledge, MCP entries to nodes**: a real
  want; a node is a harness with a `data_dir` of its own, and the same
  ssh session that installs it can `scp credentials.yaml`. Separate doc
  when it hurts. §0.7, §15.
- **Fleet-wide session list / search on the parent**: the switcher is per
  harness (31 §7.4's "never two harnesses in one store"); a cross-node
  search is a UI query over N `search_sessions`, buildable later without
  a protocol change.
- **A node-side `link.parent` / "node mode"**: nothing on the node behaves
  differently for the parent than for a phone. A flag would be costume.
- **Mode negotiated by inspecting the first JSON-RPC method** (`initialize`
  ⇒ ACP): a substring protocol, the thing 31 §1.1 dropped from jarvis. One
  byte in a payload that was empty. §4.2.
- **`ssh` entries in `agents.yaml` as the product**: they work today and
  stay documented as the zero-setup path for one box; they cannot give
  presence, UI sessions or pairing. §1.

## 15. Open

| # | question | default |
|---|---|---|
| 1 | SDK transport constructor over a duplex (§4.3) | confirm in step 2; fallback `tokio::io::duplex` + copy tasks |
| 2 | `Info.platform` values | `std::env::consts::OS` (`macos`, `linux`) |
| 3 | credential sync (`put_credentials` over the control conn) | not built; separate doc |
| 4 | children multiplexed on one ACP conn | when a node approaches the relay conn cap |
| 5 | TUI driving a node's session | not built; `ssh box wth` |
| 6 | Windows service / FreeBSD rc.d | print the `ExecStart` line, exit 2 |
| 7 | auto-pair opt-out per device ("show nodes but ask before pairing") | off; a device paired to the parent already trusts it |
