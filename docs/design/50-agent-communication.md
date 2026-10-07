# 50 — Agent communication: peers inside the harness, A2A at the boundary

Status: implemented · 2026-09-18 · final communication verification recorded
in §12.3–12.5. Merged-main apps/search integration is being checked separately.
Extends [00](00-architecture.md), [01](01-config-and-subagents.md),
[06](06-acp.md), [07](07-context.md), [19](19-subagent-board.md),
[25](25-desktop.md), [31](31-link.md), [32](32-nodes.md),
[36](36-survival.md) and [37](37-secrets.md). The controls, schemas and
commands below are the contract the code in `crates/agent`, `crates/tools`,
`crates/a2a`, `crates/rpc` and the four frontends implements; where the
as-built behavior differs from the first draft, the text says so at that
point. It supersedes 01's parent/direct-child-only addressing, pair-keyed
reply waiters and unconditional result injection; it does not replace ACP,
link, the session loop, or the tools permission gate.

**Library decision:** the official [A2A Rust SDK][sdk] supplies A2A types,
client, JSON-RPC and SSE, as production dependencies of `crates/a2a`
(`agent` depends on the core types only). The SDK is young and pre-1.0; §3
records the evidence and the two narrow vendored patches it needed. On
2026-09-18 the user **explicitly accepted the young official SDK** (§3.4),
waiving the mature-library prerequisite for the dependency choice only. The
security, lifecycle, conformance and interoperability gates in §12 are not
waived: §12.4–12.5 record, per gate, what was measured against the production
adapter, what was measured only against the throwaway probe and what was not
measured at all. The SDK's published conformance failures are recorded there,
not concealed behind “official.” Nothing here calls the SDK mature.

## 0. Decisions

1. **A peer is an addressable session, not a second agent runtime.** Native
   roots, children and siblings use the existing `Agents` supervisor and
   SQLite identity. No broker service, replicated directory, message bus
   replacement, or LLM router.
2. **Direct means no parent-model relay.** A child can discover a sibling
   and send it a message; two consenting root sessions can exchange messages.
   The supervisor authenticates and routes; it does not ask the parent to
   paraphrase the message. Memory, tools and transcripts remain private.
3. **Local calls remain typed actor messages. A2A is the network boundary.**
   Do not serialize local messages through loopback HTTP to demonstrate
   protocol compliance. Remote agents use standard A2A 1.0 over HTTPS,
   backed by the official SDK, in both client and server roles.
4. **Off by default, off means no model-to-model communication.** A persisted
   global switch and an inherited per-session opt-out cover discovery,
   explicit sends, delegation and automatic child-result injection. Human
   session inspection and steering remain available. There is no hidden
   parent/child bypass. Network A2A has an additional default-off switch.
5. **Availability is not approval.** Communication controls determine whether
   a route exists. `Risk → PolicyAction` still determines whether a tool
   call is permitted. Neither an A2A credential nor a peer's “approved” text
   resolves a theuth approval. Explicit `Deny` always wins.
6. **Authenticated identity and reply correlation are structured state.**
   Session IDs come from `ToolCx`, remote identity from verified credentials;
   neither comes from message text. Replace `mN`/pair-keyed waiting with
   stable message IDs and exact `reply_to` matching.
7. **Acceptance, consumption and completion are different facts.** Persist
   acceptance before acknowledging it. Bound pending work. Do not call a
   mailbox `tell`, HTTP timeout, or a disconnected stream successful delivery
   or failed execution. Do not claim exactly-once external side effects.
8. **One control plane, every frontend.** `rpc::Api` owns the user operations;
   TUI calls it in process, desktop/mobile over existing transports. Settings
   expose authoritative effective state and reasons, not frontend guesses.

## 1. Why this fits theuth

[00 §1–4](00-architecture.md#1-non-negotiables) supplies the design philosophy:
functional names; library-first glue; one process; plugin-owned contracts;
no god-wiring; no new crate without an independent responsibility. Local peer
routing already belongs in `agent`. The network subsystem earns one plugin,
`crates/a2a`, because it owns an optional listener, remote clients, credential
bindings and configuration. `runtime` gains nothing.

[12](12-course-correction.md) rejects parallel permission systems and frontend
business logic. The new availability predicate is equivalent to disabling a
capability, not a second allow/ask/deny policy. It cannot grant permission.
All model sends, including Python host calls, still pass through `Invoke`.

[07](07-context.md) makes a stable request prefix an invariant. Discovery is
on demand, returned at the history tail. No live roster, card, heartbeat,
status clock or changing remote skill list goes in the system prompt. The
identity line is stable; normal arrivals append at safe input boundaries.

[19](19-subagent-board.md) says a subagent is a session, and the board is a
projection of existing state. Reuse the picker, transcript, parent link and
live state; do not create a separate “agent chat” application or global
current-peer setting.

[31](31-link.md) and [32](32-nodes.md) separate frontend transport from remote
execution. Keep link's encrypted console protocol and ACP node execution.
A2A addresses interoperability with independently operated agents; it is not
a reason to rewrite either working path.

[36](36-survival.md) requires bounded, visible recovery. [37](37-secrets.md)
keeps secrets outside model-visible payloads. Both apply to communication.
[28](28-untrusted-content.md) is a **draft, not an implemented defense**;
this design does not claim its taint/sink controls exist.

### 1.1 Starting point: facts traced before implementation

These observations were traced in the source as it stood before this work,
not inferred from old “Today” sections in design drafts. Each row is a seam
the implementation changed; §§2–10 state the as-built behavior. Symbols are
more durable than line numbers in this actively changing tree.

| Fact | Source / consequence |
|---|---|
| `send_message` resolves only `parent` or a direct child name; no sibling or root-to-root route. | `agent/src/peers.rs::SendMessageArgs`; `supervisor.rs::Agents::resolve`. Extend this boundary, not `SendInput`. |
| Human `ListAgents` lists descendants; `ListSessions` is broader. There is no model-facing peer directory. | `supervisor.rs::Agents::agents`, `Roster`; `store.rs::descendants`, `list_sessions`. Human visibility is not model permission. |
| Names are unique per parent; persisted sessions can be lazily resumed. | `SpawnChild`, `Store::child_by_name`, `Agents::session`. Bare names cannot become global identities. |
| Waiters are keyed by `(waiting_session, expected_sender)`; `reply_to` is rendered but not matched. | `supervisor.rs::Deliver`, `human_input`, `child_finished`. Concurrent requests overwrite waiters and human answers pick an arbitrary matching entry. |
| Harness inputs are untyped user-role text; busy native sessions consume them after tool batches. | `loop_.rs::UserInput`, `Session`, `Turn::take_input`; `Agents::inject`. A wrapper is not proof of provenance. |
| `inject` announces delivery after an input path that can report errors without returning them to the sender. Queues/waiters are not durable. | `Agents::input`, `inject`; `LiveActor::tell_input`. Current “delivered” cannot be an A2A receipt. |
| Child results are capped, ordinary peer messages are not. | `child_finished`, `spill::cap`, `Deliver`. Apply one bounded intake contract. |
| Approval prompting is `global && !self_or_ancestor_bypass`; session-on cannot defeat global-off or ancestor-off. | `tools/src/registry.rs::{SetApprovals, GetApprovals, SetParent, bypassed, gate}`. Match this precedence explicitly. |
| Approval setters update memory before attempting the global config write; there is no shared settings-change event. | Same file. Copy the UX pattern, not the write-failure or stale-client behavior. |
| ACP/node children receive prompts and return results, but cannot invoke theuth peer tools through ACP. | `agent/src/acp_child.rs`; `acp/src/harness.rs`; `nodes/src/dialer.rs`. A translated child is not automatically a bidirectional A2A agent. |
| RPC bearer auth grants broad console access; no per-session peer ACL. | `rpc/src/lib.rs`, `rpc/src/acp.rs`. Never give a remote agent the console token. |
| Canceling a turn need not stop a registry invocation; pending work can outlive its caller. | `tools/src/registry.rs::Invoke`; `loop_.rs::CancelTurn`. Revocation must invalidate admission, not merely drop an awaiting future. |

Paths in this table are relative to `crates/`.

## 2. User contract and scope

### 2.1 Controls

Configuration (`AgentConfig.communication`, `a2a::A2aConfig`; first-run
`config.yaml` carries the `a2a` section commented out):

```yaml
agent:
  communication: false       # harness-wide permission to communicate

a2a:
  enabled: false             # permit network client/server machinery
  listen: 127.0.0.1:7780     # SDK HTTP listener, behind HTTPS for production
  public_url: null           # required HTTPS base URL for published exports
```

`agent.communication` is owned by `Agents`. `a2a.*` is owned by the A2A
plugin. Missing keys default to false. Fail-closed as built: an unparsable
`agent` section falls back to `AgentConfig::default()` (communication off)
with a debug-level log, not a user-visible error; an unparsable `a2a` section
starts a status-only actor that reports the error through `/a2a` and never
listens. Environment overlay uses the existing `THEUTH_AGENT__COMMUNICATION`,
`THEUTH_A2A__ENABLED`, `THEUTH_A2A__LISTEN`, `THEUTH_A2A__PUBLIC_URL` convention.
Configuration is loaded at startup; user commands update the live global
communication switch and the YAML line. Environment-derived startup values
are reported as such (`source: env`); the command's persisted YAML value does
not override environment on restart. `a2a.enabled`, `listen` and `public_url`
are startup state: `/a2a reload` re-reads `a2a/peers.yaml` only.

| User operation | Meaning |
|---|---|
| `/communication` | Show global, this session, inherited blocker, effective state and network state. No mutation. |
| `/communication on\|off` | Remove/add this session's in-memory opt-out. Descendants inherit it. |
| `/communication on\|off always` | Persist and apply the global switch; affects all sessions now, not only newly created ones. |
| `/peers` | Human picker of accessible peers with state, scope and disabled reason, plus requests awaiting a human answer. Does not run a model. TUI keys: ⏎ opens a peer's session, `i` inspects a request, `a` answers it by exact id, `r` refreshes. |
| `/peers scope owner\|isolated [alias]` | Owner-only: set the shown root's communication scope (§2.3) and optional alias. TUI, Slack, Signal; the desktop does it from Settings → Communication. |
| `/requests [<id> \| answer <id> <text>]` | Slack/Signal only: list pending peer requests, inspect one, answer one by unique id prefix. The TUI and desktop answer from the `/peers` picker / peers panel. |
| `/a2a` | Network status, configured peers/exports and adoption/configuration errors. |
| `/a2a reload` | Validate and atomically replace peer/export configuration; revoke removed bindings. Owner only. Listener settings require restart. |

Slack spells these `/theuth communication …`, `/theuth peers …`, `/theuth requests …`,
`/theuth a2a …`; all four are owner-only there and on Signal. The desktop's bare
`/communication`, `/peers` and `/a2a` open Settings → Communication. TUI and
desktop call `rpc::Api` (§10); Slack and Signal send the same typed owner
commands to `Agents` in process.

For session `s`:

```text
local_enabled(s) = global_communication
                   AND no opt-out on s or any stored ancestor
local_route(a,b) = local_enabled(a) AND local_enabled(b)
                   AND permitted_scope(a,b) AND a != b
remote_route(s,p) = local_enabled(s) AND a2a.enabled
                    AND configured_grant(s,p) AND authenticated(p)
```

The approval analogy is intentional: **global-off is a ceiling, not an
individually overridable default**. `/communication on` while global-off
reports “still disabled globally; use on always to enable the harness.”
`on` in a child cannot defeat an ancestor opt-out. To allow communication in
selected sessions, enable globally and opt the others out; the separate
network switch and explicit export grants remain closed.

| Global | Self/ancestor opt-out | Effective |
|---|---|---|
| off | absent or present | off |
| on | present | off |
| on | absent | on, subject to scope/grant |

Session overrides are deliberately memory-only, like approvals. They survive
turns, compaction, `/clear` and switching frontends, but not daemon restart.
Status text says this. Global-off survives restart. A new fork has a new root
identity and starts opted out if its source is effectively off; it must not
turn a private conversation public merely by dropping ancestry. Ordinary
new sessions use the global default. Ancestry checks use persisted topology,
not only loaded actors, so lazy resume cannot bypass inheritance.

### 2.2 What off does

Off blocks both sender and receiver, including cached/guessed addresses:

- Hide `discover_peers`, `send_message` and `spawn_subagent` from the next
  tool snapshot and Python function index; reject stale calls at admission.
- No new model-to-model sends, peer-triggered turns, or automatic
  `<subagent_result>` injection. Disabling messaging while leaving spawn
  usable would promise results the parent cannot receive; therefore spawn
  is disabled too. This is an intentional default change from 01.
- Do not cancel unrelated human work, erase histories, kill a child merely
  for existing, or hide its transcript from the user. A child already working
  can finish; its result remains inspectable but cannot wake the parent.
- Already accepted, unconsumed peer input becomes `suppressed`; it does not
  spring back to life on re-enable. The sender sees the terminal suppression
  reason. New explicit input may ask the child to continue later.
- Expire affected waiters with `communication_disabled`, not a fake reply or
  a normal timeout. Stop A2A subscriptions and refuse further protected reads.
- In-flight network sends are aborted best effort. Bytes already sent cannot
  be unsent; mark an unresolved remote outcome `unknown`, not “undelivered.”
- When global communication is off, the A2A listener is stopped even if
  `a2a.enabled` is configured true. Its small control actor can remain to
  react to re-enable. With `a2a.enabled: false`, no network actor, listener,
  discovery fetch, credentials load, heartbeat or background reconnect starts.

For an A2A-owned request, disabling its receiving session cancels pending
input and requests cancellation of its owned active work. Unrelated work in
that session is not retrospectively canceled. Already consumed content
cannot be removed from a model's context by this switch; already-started
external tool side effects are not rolled back. §8 defines the race boundary.

**Migration:** do not grandfather existing parent/child messaging or infer an
opt-in from historical spawns. Missing `agent.communication` means off.
Release notes and all delegation guidance must call out this behavior; the
user enables it once with `/communication on always`. This makes the requested
opt-out complete rather than silently meaning “network only.”

### 2.3 Who can discover whom

No multi-user platform is invented here. theuth remains the single-owner process
of [15](15-server-config.md); same-UID shell/filesystem access is not sandboxed.
Nevertheless, chat contacts and external agents must not automatically gain a
catalogue of unrelated owner conversations.

Add a small, persisted **root communication scope** at trusted session
creation: `owner` or `isolated`. Owner-console roots are `owner`; non-owner
Slack/Signal roots, inbound external-protocol roots and unattended automation
roots are `isolated` unless their existing trusted creator explicitly declares
owner scope. Existing roots with unknown provenance migrate to `isolated`.
The owner can opt a legacy root into owner scope in its communication settings;
this is an owner-only operation, not a model tool. Children inherit the root's
scope dynamically. Scope is not a tool permission or an execution sandbox.

- Within one enabled root tree: parent, children, siblings and descendants
  can discover and message one another directly.
- Across roots: both roots must be in `owner` scope and both endpoints enabled.
- Isolated trees cannot list owner roots, even with a raw UUID or title query.
- Network principals see only explicitly granted exports and their own
  task/context exchanges. They cannot invoke the owner's `ListSessions`.
- Exporting an isolated session is allowed only by explicit owner config;
  it does not promote that tree into owner scope.

This adds origin metadata only at creation/routing boundaries. It does not
thread account IDs into the model loop or claim the shelved multi-user design
has shipped.

## 3. Rust library selection and protocol version

### 3.1 Choose the official SDK; do not overstate maturity

The [official SDK directory][sdk] lists `a2aproject/a2a-rs`. It supplies the
protocol rather than a model framework, fits Tokio/Axum, and supports both
client and server. Published package names differ from their Rust imports:

| Package / reviewed version | Import | Evidence |
|---|---|---|
| [`a2a-lf` 0.3.1][core] | `a2a` | Published 2026-09-14; core protocol types/errors. |
| [`a2a-client-lf` 0.2.5][client] | `a2a_client` | Published 2026-09-15; card resolution, client factory, JSON-RPC/HTTP+JSON, SSE and task operations. |
| [`a2a-server-lf` 0.4.4][server] | `a2a_server` | Published 2026-09-14; Axum routers, `RequestHandler`, executor/store hooks. |

Apache-2.0; declared Rust 1.85 / edition 2024. Verify the resolved dependency
graph on theuth's actual stable toolchain and macOS/Linux targets; a manifest
MSRV is not proof of transitive compatibility. Axum 0.8, reqwest 0.13, Tokio
and tower 0.5 align with existing workspace dependencies. HTTP-only use still
pulls some protobuf support; do not promise a protobuf-free graph.

Pins in the workspace manifest, used as production dependencies by
`crates/a2a` (all three) and `crates/agent` (`a2a-types` only); the probe in
§3.4 shared them. They are the evaluated release set; §12.4 records which
§12.1 gates they have and have not passed:

```toml
# Workspace dependencies; the local plugin crate is `a2a`, so the core SDK is aliased (`a2a_types::…`).
a2a-types = { package = "a2a-lf", version = "=0.3.1" }
a2a-client = { package = "a2a-client-lf", version = "=0.2.5" }
a2a-server = { package = "a2a-server-lf", version = "=0.4.4" }
```

Use default rustls features (`rustls-tls` = aws-lc-rs, the provider `theuth::main`
already installs because ring is also linked). There are no features named
`jsonrpc`, `client` or `server` on these client/server packages. Do not enable
separate gRPC or SLIMRPC crates, push delivery, signing infrastructure or a
framework. Both HTTP crates depend on `a2a-pb` (prost + pbjson): the JSON-RPC
wire is ProtoJSON, and the graph also adds tower-http 0.7 beside the
workspace's 0.6. The lockfile pins this exact set; never a floating Git
branch. Two crates in the graph are vendored path patches (§3.4).

**Evidence against an unqualified maturity claim:**

- Public releases and cross-platform CI exist, but are still pre-1.0. The
  [ACTS workflow][acts-workflow] explicitly uses `continue-on-error: true`.
- The [published rolling ACTS report][acts-report], entry dated
  `2026-09-18T02:10:21Z`, commit
  `b25d010cd3e65ea9c7abe050d8c563a3b0522e45`, reports
  **235 passed, 46 failed, 52 skipped; conformant: false** across 333
  transport/test cases. JSON-RPC MUST cases: 50 passed, 9 failed, 6 skipped.
  `sdk_version` is `unknown`: this is evidence about that nightly SUT, not
  a measurement of the exact published releases above.
- The [released server handler][released-handler] creates a task for an
  unknown client-supplied task ID and does not reject a mismatching context
  as required. Its default authorization hook is optional; task listing is
  not automatically caller-scoped. Current-main fixes cannot be credited to
  a released crate.
- No independent audit or independently verified production-hardening record
  was identified in the inspected primary sources. That is a research limit,
  not a claim that no deployment exists.

### 3.2 Alternatives considered

| Candidate | Strength | Why not selected |
|---|---|---|
| [`a2a-protocol-sdk` 0.12.1][alternative-sdk], `tomtom215/a2a-rust` | A2A 1.0; client/server/SSE, optional SQL/auth/Axum; detailed conformance reporting; Apache-2.0, declared Rust 1.88. | Independent SDK; its [stability policy][alternative-stability] was adopted 2026-09-09 and records recent breaking changes and unmet 1.0 gates. Its [TCK findings][alternative-tck] also contain failures/unmeasured requirements. Not a demonstrably mature substitute. Different suites mean scores cannot be ranked directly. |
| [`ra2a` 0.10.1][ra2a], `qntx/ra2a` | Client/server, Axum/SSE, optional SQL and gRPC; MIT or Apache-2.0; A2A 1.0. | Latest observed publication 2026-04-08; README explicitly says not independently audited; no external conformance report found in inspected sources. No declared MSRV found. |
| Handwritten JSON-RPC/SSE or a generic agent framework | Could reproduce a narrow demo. | Violates 00 §1.2 and the user's library requirement; commits theuth to protocol drift. |
| Python/Go SDK sidecar | Official alternatives exist. | Does not satisfy a mature **Rust** dependency requirement and adds a process/runtime boundary unnecessarily. Not silently substituted. |

The user has accepted the young SDK (§3.4); the alternatives above remain
rejected. Local typed routing is independently useful, but is not presented as
completion of the A2A portion. A bounded in-repo upstream patch is allowed by
00 §1.3; a private SDK fork or custom protocol implementation is not the
fallback.

### 3.3 Version contract

Use A2A **wire version `1.0`**, pinned against the [v1.0.1 release][spec-release]
and [normative proto][proto]. The documentation landing page currently labels
1.0.0 latest while GitHub publishes v1.0.1; record this discrepancy rather
than assuming “latest” is immutable. Patch spec revisions do not change the
wire negotiation string.

The 0.x Rust crate versions are not A2A protocol 0.3. Use SDK serialization,
canonical PascalCase methods (`SendMessage`, `GetTask`, etc.) and
`A2A-Version: 1.0`. Reject unsupported versions; do not interpret an absent
header's legacy 0.3 semantics as permission to silently serve 1.0. No dual
0.3/1.0 compatibility layer in theuth.

### 3.4 SDK acceptance, integration probe and known wire gaps

**Acceptance record.** On 2026-09-18 the user chose “accept the official
SDK” with an explicit maturity waiver. What that waives: the requirement that
the dependency itself be demonstrably mature before it may be depended on.
What it does not waive: every §12 gate, the application-owned handler
(§4), principal scoping (§6.1), bounded intake (§8) and the official
conformance/interop runs. A gate failure caused by the SDK is still a
blocker; the remedy is a narrow, removable upstream patch (00 §1.3), never a
custom protocol layer in theuth.

**Probe.** `a2a_sdk_probe.rs` was a throwaway integration probe against the
exact pins: an `agent` example with its own `a2a-client`/`a2a-server`/`axum`/
`reqwest` dev-dependencies. It never touched the session store and is **not in
the tree**: it was removed before merge together with those dev-dependencies,
and its source is preserved outside the repository as a proof artifact only.
Nothing in this section is a runnable path. It ran the SDK `jsonrpc_router`
and `agent_card_router` nested under `/agents/probe`, behind an Axum bearer
layer, with a theuth-style application `RequestHandler` (principal-scoped
in-memory tasks/contexts), and drove them with `AgentCardResolver`,
`A2AClientFactory` + `JsonRpcTransportFactory`, and interceptor-set
credentials. Three modes: an in-process self-test (one `ok`/`FAIL` line per
check, exit 1 on failure); `serve <addr>`, the System Under Test for the
conformance kit and the Go CLI, with optional bearer principals; and `client
<url>`, the reverse direction against a running peer (card resolve,
factory-selected JSON-RPC client, blocking send, `GetTask`, SSE stream).

Checks the self-test asserts through the SDK client (SDK error codes, not
prose): authenticated card (401 without bearer; no CORS header; `Cache-Control:
private, no-store`), one JSONRPC/1.0 interface; direct `Message` response with
server-minted `contextId` and no task; blocking `Task` completion with history;
`returnImmediately`; `SubscribeToTask` snapshot-first and close on terminal;
`SendStreamingMessage` order `Task, statusUpdate, artifactUpdate, terminal`;
message-only stream of exactly one `Message`; `INPUT_REQUIRED` resume on the
same task with inferred context; `historyLength` unset/1/0; cursor pagination
with empty final `nextPageToken`; cancel/complete terminal race; and the exact
errors `TASK_NOT_FOUND` (unknown or foreign task on send/get/cancel/subscribe),
`UNSUPPORTED_OPERATION` (send/subscribe to a terminal task, extended card),
`TASK_NOT_CANCELABLE`, `INVALID_PARAMS` (unknown or mismatching `contextId`,
foreign context), `CONTENT_TYPE_NOT_SUPPORTED` (non-text part),
`PUSH_NOTIFICATION_NOT_SUPPORTED`, `VERSION_NOT_SUPPORTED` (`0.3`, absent
header), patch-version tolerance (`1.0.7`), and `METHOD_NOT_FOUND` for the
legacy `message/send` name. First execution on 2026-09-18 (macOS arm64,
stable toolchain): the pinned graph resolved and built; **38 of 39 checks
passed**. The one failure was a
probe-side acceptance bug, not an SDK gap: the reply to an `INPUT_REQUIRED`
task appended history but left the snapshot at `INPUT_REQUIRED`, so a blocking
caller observed an already-settled state and returned before the resume ran.
The fix moves the `WORKING` transition into the acceptance step, under the
same lock as the history append and before the receipt is returned; the
production intake (§7) must keep that ordering. Rerun after the fix: **39 of
39**. An independent client, the official Go CLI `a2a` v2.5.0, then sent a
message to the probe and received a `COMPLETED` Task.

First conformance-kit run against the probe (kit `263b9cf`, `--transport
jsonrpc`): 54 failed / 16 passed / 159 skipped / 6 errors, almost all
`JSONDecodeError`, i.e. a non-JSON body. Precise cause, probe-side routing,
not the SDK or the kit: the kit builds `httpx.Client(base_url=<sut-host>)`
and calls `.post("/")`, which resolves to `<sut-host>/` (trailing slash);
the SDK `jsonrpc_router` serves exactly `/`, and axum 0.8 `Router::nest`
flattens a nested `/` onto the bare prefix, so `/agents/probe/` matched no
route and axum answered an empty 404. The card fetch has no such ambiguity,
which is why the 16 card/discovery cases passed. The probe now mounts the
export with `Router::nest_service`, which keeps a `{*rest}` route and strips
the prefix, so `<prefix>`, `<prefix>/` and `<prefix>/.well-known/…` all reach
the SDK routers unchanged; the self-test asserts the trailing-slash form. The
production adapter (`crates/a2a/src/server.rs`) mounts exports the same way:
`/agents/<export>/rpc` and `/rpc/` both reach the SDK router. After this fix,
the required-field validation row below and the `pbjson-types` patch, the
probe self-test stands at 45/45. The kit rerun against the probe (same
commit, `--transport jsonrpc`) **fails 2 cases on the raw kit**, both kit
defects rather than SUT behavior: the `CORE-SEND-003` requirement omits
`CONTENT_TYPE_NOT_SUPPORTED` from its declared expected error, and
`CORE-MULTI-002`/`002a` treat any error for a client-supplied `contextId` as
failure although spec §3.4.1 says an agent that cannot accept one MUST reject
with an error and MUST NOT mint a new `contextId`. With two local, spec-cited
corrections to the kit fixtures (declare that error; accept a protocol error
or an exactly echoed `contextId` for those two requirements, plus 10 kit unit
tests for the rule; local working tree on `263b9cf`, not upstreamed) the same
run reports **80 passed, 155 skipped, 30 deselected, 0 failed at pytest
level**. The kit's own compatibility summary still counts unmeasured
requirements as failures (22 MUST), so this is **not a conformance claim**:
skipped/unmeasured MUST cases are not passes (§12.1 gate 2), the corrections
are fixture-only, and the run was against the probe. The production adapter
has not been run through the kit; what the production adapter was measured
with instead is the official Python SDK smoke of §12.4, and the kit's
unmeasured requirements are mapped to it there.

**Known wire gaps in the pinned releases** (verified in the [server
router][server-jsonrpc], [card route][server-card], [released
handler][released-handler] and [client transport][client-jsonrpc]; each is
either an application duty or a candidate upstream patch, never a reason for
a theuth codec):

| Gap | Consequence for theuth |
|---|---|
| `jsonrpc_router` never reads `A2A-Version`; the handler receives lowercased header names (`a2a-version`, not `a2a::SVC_PARAM_VERSION`). | Version negotiation (§3.3), including “empty means 0.3 → `VERSION_NOT_SUPPORTED`”, is the handler's duty on every method. |
| Only `ServiceParams` (the raw header map) reaches `RequestHandler`; `CallContext`/`User`/`InterceptedHandler` in `a2a_server::middleware` are not wired into the JSON-RPC route. | The trusted principal seam (§6.1) is an HTTP layer that strips any inbound copy of an internal header and sets the verified value; the handler must never read `Authorization` itself. |
| JSON-RPC errors are always HTTP 200; the router has no auth hook. | 401/403 come from the theuth Axum layer before the SDK router; the kit tests that when `securitySchemes` is declared. |
| `agent_card_router` unconditionally emits `Access-Control-Allow-Origin` (`*`, or the request origin with `allow-credentials: true`) and serializes the card with serde, not ProtoJSON. | Wrap with `map_response` to strip CORS headers and add `Cache-Control: private, no-store`; card and RPC JSON are both camelCase, so serde/ProtoJSON agree for the card. |
| `MAX_REQUEST_BODY_BYTES` (10 MiB) is baked into `jsonrpc_router` via `DefaultBodyLimit`. | theuth's 64 KiB bound (§8) is an outer `RequestBodyLimitLayer`; the inner limit never applies. |
| `DefaultRequestHandler::prepare_task_for_execution` creates a task for an unknown client `taskId` and adopts the stored context over a mismatching request context; list/get are not caller-scoped. | theuth does not use `DefaultRequestHandler` or `InMemoryTaskStore` at all (§4). |
| `task_store::apply_history_length` is `pub(crate)`; `pagination::resolve_page_size` is public. | Reimplement the six-line history rule; reuse the page-size rule. |
| SSE keep-alive is fixed at 15 s; event ids restart per response; a mid-stream `A2AError` is emitted as a JSON-RPC error frame and the stream continues until the handler stream ends. | Stream termination and per-subscriber bounds (§8) are the handler's responsibility. |
| `AgentCardResolver` has no interceptor hook; `AgentCardResolver::new(None)` and `JsonRpcTransportFactory::new(None)` `expect(...)` on client construction. | Card credentials are default headers on a dedicated `reqwest::Client`; always pass an explicit client. |
| Client transport applies every interceptor value as an additional header; `A2AClient::new` pre-sets `A2A-Version: 1.0`. | Set `authorization` exactly once (interceptor **or** default header, not both). |
| Client `send_streaming_message` treats a response without `Content-Type` as SSE and a `text/event-stream` body that is a plain JSON-RPC envelope as an error frame. | Acceptable; noted so a proxy that strips content types is diagnosed correctly. |
| `a2a-pb` 0.2.1 [`pbconv::from_proto_send_message_request`][pbconv] replaces an absent `message` with `Message::new(Role::User, vec![])` (fresh random `messageId`, `ROLE_USER`, no parts), and present-but-empty fields decode to proto3 defaults (`""`, `ROLE_UNSPECIFIED`, `[]`). `a2a.proto` marks `Message.message_id`/`role`/`parts` and `SendMessageRequest.message` REQUIRED; the router enforces none of them, so `params: {}` reaches the handler as a plausible request whose only trace is `parts.is_empty()`. | The handler validates required fields itself before anything else: empty `messageId`, unspecified role or no parts → `INVALID_PARAMS` (-32602), no task or context minted. The probe does this; the production adapter must too. |
| `pbjson-types` 0.9.0 (under `a2a-pb`) serializes `google.protobuf.Timestamp` with chrono's `to_rfc3339()`, emitting `+00:00`; the [protobuf JSON mapping][proto-json] requires Z-normalized output with 0, 3, 6 or 9 fractional digits, and a2a-tck `DM-SERIAL003` checks the `Z` form. | Vendored patch below; every SDK timestamp (task status, JSON-RPC and SSE payloads) inherits the fix. Not a theuth codec. |
| `a2a-client-lf` 0.2.5 [`jsonrpc::parse_sse_bytes`][client-jsonrpc] (shared by the JSON-RPC and REST SSE paths) appends every body chunk to a `Vec<u8>` and only then looks for an event terminator; a peer that never sends the blank line, or sends one enormous frame, grows the buffer until the connection drops. theuth's §8 caps run after decoding, so they never see it. | Vendored patch below: the raw event is bounded at 64 KiB before the append; an oversized frame yields one SDK `A2AError` and ends the stream. No theuth codec, no plugin change. |

**Dependency patch record (00 §1.3, §12.1).** `vendor/pbjson-types` is
crates.io `pbjson-types` 0.9.0 (MIT; [influxdata/pbjson][pbjson] commit
`48ee5989564e0ace82ad71b87736d3d1b2e0e1cd`, `path_in_vcs = pbjson-types`),
wired through `[patch.crates-io]` in the root manifest. The only semantic
change is `src/timestamp.rs`: `Serialize for Timestamp` uses
`to_rfc3339_opts(SecondsFormat::AutoSi, true)` instead of `to_rfc3339()`;
parsing still accepts any RFC 3339 offset. Its unit test asserts the `Z`
form and 0/3/6/9-digit fractions, including a pre-epoch instant. The vendored
crate is a path patch, not a workspace member, so its dev-dependencies do not
resolve from the root; run `cargo test` on a standalone copy of the directory
(12 tests passed 2026-09-18). The crate's own published `Cargo.lock` and the
registry `.cargo-ok` marker are not carried; the upstream `LICENSE` is added.
Upstream `main` still emits `+00:00` as of 2026-09-18. Removal condition: a
`pbjson-types` release that Z-normalizes, picked up by `a2a-pb`.

`vendor/a2a-client` is crates.io `a2a-client-lf` 0.2.5 (Apache-2.0;
[a2aproject/a2a-rs][sdk] commit `365d056784ca67466f9767728b5ac3be31ea19a8`,
`path_in_vcs = a2a-client`), wired through `[patch.crates-io]` as
`a2a-client-lf = { path = "vendor/a2a-client" }`. Two semantic changes, both
in `src/jsonrpc.rs` and re-exported from `lib.rs`. First, a new
`pub const MAX_RESPONSE_BYTES: usize = 2 * 1024 * 1024` bounds the raw body
of every unary (non-SSE) response — JSON-RPC calls, the non-stream branch of
a streaming call, the REST binding and `AgentCardResolver` — while it is read
and before `serde_json` sees it (`read_bounded_body`/`read_bounded_json`; a
declared `Content-Length` over the cap is refused before the first byte).
Second, a new `pub const MAX_SSE_EVENT_BYTES: usize = 64 * 1024` in
`parse_sse_bytes` bounds the raw bytes of one SSE event before its terminator.
Each body chunk is appended in slices of at most cap + 4 (the longest
terminator, `\r\n\r\n`) and the events it completes are drained between
slices, so a large chunk of many small events is measured per event, not per
chunk. An event whose content exceeds the cap, whether its terminator arrives
or never does, yields a single `A2AError::internal("SSE event exceeds 65536
bytes; stream closed")` after any events already parsed, and the stream then
ends without draining the rest of the body. An event of exactly the cap is
delivered, including with a fragmented terminator. Everything else
(`find_event_boundary`, `parse_stream_tail`, both `parse_event` callbacks,
the transports' framing) is unchanged. Four regression tests cover the boundary
(exactly the cap, delivered; one byte over, rejected), a delimiter-less body
cut at the cap with the earlier event still delivered, and 2000 small events
in one 200 KiB chunk; as with `pbjson-types`, run them on a standalone copy.
The published `Cargo.lock` and `.cargo-ok` are not carried; the repository
`LICENSE.md` is added. Removal condition: an `a2a-client-lf` release that
bounds both the SSE frame and the unary response body itself.

**Official conformance kit.** Repository
[`a2aproject/a2a-tck`][tck], reviewed at commit
`263b9cfaf16a554bdfb166a7ba5b67716e946349` (2026-09-01), specification
snapshot `v1.0.0` commit `173695755607e884aa9acf8ce4feed90e32727a1`
(`specification/version.json`). Prerequisites: Python ≥ 3.11 and `uv`; no
Docker. The runner is `run_tck.py --sut-host URL [--transport jsonrpc]
[--level must|should|may] [-v]`; reports land in `reports/`
(`compatibility.json`, `compatibility.html`, `junitreport.xml`). The kit
fetches `{sut-host}/.well-known/agent-card.json` with plain `httpx.get` and
builds clients that send only `A2A-Version`; **the `A2A_AUTH_*` variables in
`docs/AUTHENTICATION_SETUP.md` are not implemented at this commit** (no
reference in `tck/` or `tests/`), and that document's `--sut-url --category`
flags are stale. Run it against an anonymous export, or through the Go CLI
proxy below when the export is authenticated. SUT behavior is keyed by the
`messageId` prefixes in `scenarios/*.feature` (`tck-complete-task`,
`tck-artifact-{text,file,file-url,data}`, `tck-message-response`,
`tck-input-required`, `tck-reject-task`, `tck-stream-*`,
`test-resubscribe-message-id`, which must stay non-terminal for
2 × `TCK_STREAMING_TIMEOUT`, default 2 s). The probe implemented them; the
production adapter runs a real turn in the exported session and keys nothing
on those prefixes, so a future kit run against it needs a scripted model or
an inert provider behind the export, not only a URL.

The kit was driven, once, against the probe's `serve` mode on
`127.0.0.1:7781` with an anonymous export: `./run_tck.py --sut-host
http://127.0.0.1:7781/agents/probe --transport jsonrpc --level must` from a
`uv` venv with the kit installed editable. The probe is gone from the tree,
so this is a description of the recorded run, not a recipe.

**Official interop peers.** Go ([CLI reference][go-cli]): `go install
github.com/a2aproject/a2a-go/v2/cmd/a2a@latest` (Go ≥ 1.25) gives a client
(`a2a discover <url>`, `a2a send <url> "text" [--stream|--immediate|--task
<id>|--context <id>] --transport jsonrpc --auth "Bearer <tok>"`, `a2a get task`,
`a2a list tasks`, `a2a cancel`, `a2a subscribe`) and a server (`a2a serve
--echo --transport jsonrpc --port 8080`; `a2a serve --proxy <upstream> --auth
"<tok>"` acts as an authenticated gateway, which lets the credential-less kit
reach an authenticated export). Python: the kit generates
`sut/a2a-python/sut_agent.py` (`make codegen-a2a-python-sut`; `cd
sut/a2a-python && uv sync && uv run python sut_agent.py`, HTTP on :9999); its
`pyproject.toml` resolves `a2a-sdk` from a sibling clone at
`<a2a-tck>/../a2a-python` (`A2A_PYTHON_SDK_PATH`/`A2A_PYTHON_SDK_VERSION`
override) and needs `grpcio` wheels, no Docker. Client direction against the
probe: `a2a send http://127.0.0.1:7781/agents/probe "hello" --transport jsonrpc
--auth "Bearer <tok>"` (performed once with v2.5.0: `COMPLETED` Task).
Server direction: `a2a serve --echo --transport jsonrpc --port 7782`, then
the probe's `client` mode against it (SDK card resolution, factory-selected
JSON-RPC client, blocking send, `GetTask`, `TASK_NOT_FOUND`, SSE framing):
performed once against the Go echo peer on loopback, 5/5 checks. Neither Go
direction has been run against the production adapter (§12.5). The Python
SUT has not been run; the production adapter was exercised with the Python
SDK as a client instead (§12.4).

## 4. Ownership and integration

```text
model -> Invoke -> peer tools -> Agents -> native Session / ACP child
                                  |
                                  +-> registered remote sender -> A2A SDK client

HTTPS -> SDK Axum router -> authenticated RequestHandler -> Agents
                                                           |
                                                           +-> session store / request intake

TUI / desktop / phone -> rpc::Api -> Agents and A2A control actor
```

Dependency direction:

```text
runtime <- tools <- secrets <- agent <- a2a plugin <- rpc <- tui <- theuth
                               ^
                          a2a-types SDK
```

This shows relevant edges, not a replacement for the complete workspace graph.
`agent` may use the SDK's core `Message`, `Task`, IDs/status types for durable
communication records; it does not depend on SDK HTTP clients/servers or the
local `a2a` plugin. No copies of A2A wire structs in `runtime` or `rpc`.

| Owner | Responsibility |
|---|---|
| `agent` | Stable peer identity, scope, enablement, topology, discovery, durable intake/receipts, exact waiters, native request lifecycle, transcript integration. Only it writes its session DB. |
| `tools` | Existing invocation/approval gate, authoritative capability visibility, cancellation admission context and secrets hook. No remote task store. |
| `a2a` plugin | SDK client/server adapters, TLS/auth boundary, configured peer/export mappings, connection lifetime, translation into agent commands. No model loop or second durable inbox. |
| `rpc` | Owner control methods and event bridge. No protocol-task policy in frontend adapters. |
| `theuth` | Plugin vector and first-run config only. No SDK construction or handle threading in `main`. |

The plugin registers remote endpoints through an agent-owned
`RegisterPeerSender` message carrying its sender implementation, following
`RegisterNode`/`RegisterUserSink`. The registration includes a generation and
opaque peer IDs, not an allow decision. `Agents` validates route policy at
each use. Unregister on reload/disable. Local routing remains a direct branch,
not an implementation of an unnecessary universal transport framework.

Use SDK `jsonrpc::jsonrpc_router` and `agent_card::agent_card_router`.
Implement the SDK's **`RequestHandler` application seam**, not its framing or
serializers: map typed requests to agent-owned intake, storage and lifecycle.
This is necessary because theuth already owns sessions and because
`DefaultRequestHandler` plus a shared `InMemoryTaskStore` neither isolates
principals nor supplies the required durability. The default handler's
synchronous optional `RequestAuthorizer` cannot filter a caller's paginated
list or establish complete request identity. Card routes require their own
HTTP authentication; handler authorization does not cover them.

Client side: `AgentCardResolver`, `A2AClientFactory` and `A2AClient` handle
cards, typed calls and streams. Supply the existing rustls HTTP policy, bounded
body/stream handling and credentials via SDK interceptors. Choose the first
supported card interface in advertised order, restricted to JSONRPC 1.0;
validate the SDK factory's selected interface rather than allowing its default
binding preference to silently change the contract.

No network await, reply wait, model execution or stream pumping holds the
`Agents` mailbox. Use the existing delegated-reply/spawn pattern and return
completion messages to the owner. Do not change kameo bus delivery strategy.

## 5. Discovery, identity and model tools

### 5.1 Addresses and minimum disclosure

Local canonical address: `session:<UUID>`. Keep `parent` and direct child names
as conveniences resolved relative to the authenticated caller. New sibling,
descendant and root routes use canonical addresses returned by discovery;
there is no fuzzy global name lookup. A renamed title cannot redirect a send.
Unknown UUIDs **fail**; never route them through `Agents::session`'s
create-if-missing behavior.

Remote canonical address: `peer:<configured-name>`. One entry denotes one
configured Agent Card/endpoint, not a claim that every session at a remote
host is visible. The name is an owner alias; credential binding and validated
endpoint establish identity. Remote context/task IDs remain scoped to that
peer and authenticated principal, never parsed as local SessionIds.
Store a server-minted binding UUID separately from the editable alias. Credential
rotation for the same accepted peer preserves that identity; removal/recreation
or changing its endpoint identity creates a new binding UUID. Reusing an alias
must not grant access to the previous peer's tasks or queued replies.

Add `discover_peers { query?: String, cursor?: String, limit?: u16 }`:

- `Risk::ReadOnly`, runtime function (`DIRECT = false`), structured output.
- Default 25, maximum 100 rows, cursor pagination after permission filtering.
- Return canonical address, short name, kind (`native`, `acp`, `node`,
  `a2a`), relationship, coarse state and capabilities (`send`, `reply`,
  `stream`). Remote descriptions are bounded untrusted data.
- Do not return another root's title, task brief, last message, cwd, model,
  tools, secrets, or transcript. For same-tree peers the existing brief name
  is sufficient. Cross-root aliases are explicit owner labels, defaulting to
  a short session ID, not model-generated conversation titles.
- Listing does not instantiate dormant sessions or probe every network peer.
  Distinguish stored/idle from offline/unavailable; an Agent Card is not a
  heartbeat or execution-liveness guarantee.

A2A standardizes Agent Cards, **not a live session registry or broadcast
presence**. The local directory is theuth state; no custom A2A registry extension,
mDNS scan, fleet broadcast or auto-import of a peer's contacts is required.

### 5.2 Keep one send tool

Extend existing `send_message`, retaining `to`, `message`, `await_reply`,
`reply_to`. Add optional `request_id` for explicitly querying/retrying the
same local acceptance; an absent ID is minted by the harness. Locally minted
IDs are opaque UUIDs, not process-local counters. External A2A identifiers are
bounded opaque strings, as the protocol permits; never require UUID syntax or
use them as paths. Persist their mapping to local message IDs. `reply_to` always
names a local message ID, never a title, peer name, TurnId or task ID.

Return structured `PeerReceipt` alongside concise text:

```text
id, to, state, reply_to?, reply?, task_id?, error?
state = accepted | consumed | replied | suppressed | failed | unknown
```

`accepted` means committed to the recipient's intake, not “the agent read it.”
A2A `Task` acceptance includes the remote task handle when known. A remote
message-only response is a reply without inventing a task. `await_reply`
uses the existing `reply_timeout_secs`; timeout returns the request handle
and current state, explicitly saying the accepted message is not recalled.
A dropped waiter is not interchangeable with a timeout.

Add `peer_message { id }` as a read-only runtime function to inspect the
caller's own sent/received record and current outcome. No arbitrary peer
history API. Bounded result previews use existing `spill::cap`; no automatic
fetch of a remote attachment or spill path.

Keep `send_message` direct. Its cross-root/network egress now warrants
`Risk::Mutating`, consistently for all destinations: default policy asks.
This deliberately supersedes 01's assumption that messaging changes nothing
outside the harness. Do not add a destination-specific parallel approval
policy. Users can disable approval prompts independently; doing so does not
turn communication on. Discovery/receipt lookup remain ReadOnly.

### 5.3 Replies and human intervention

- Persist `(message_id, sender, recipient)` before installing its waiter and
  scheduling intake. Match replies by exact `reply_to` **and reversed
  authenticated endpoints**. Wrong peer, unknown ID or changed destination
  fails before injection.
- Multiple requests to the same peer coexist; IDs prevent overwriting.
  Waiters are bounded and removed on reply, timeout, cancel, disable or
  session deletion. Status derives from live entries, not abandoned oneshots.
- An explicit reply may complete an awaiting local tool call without creating
  another model turn. A late reply after timeout is delivered once as a
  correlated input when enabled, never to a newer waiter.
- Human `Steer` still steers. If exactly one peer wait is pending, the UI may
  offer “Answer this request”; with multiple, require selection by ID. Add a
  typed `answer_peer_request` owner operation; stop selecting a HashMap entry
  arbitrarily. A peer message never uses the human-answer path.
- Implicit native turn results may answer a request only when the turn is
  explicitly bound to that request. Another turn's final text cannot satisfy
  it merely because it came from the same child. Unsolicited messages have no
  implicit reply; recipients answer with `reply_to`.
- Finite waits bound cyclic conversations; there is no advisor model or
  automatic “thanks” turn. A send does not require a response unless requested.

## 6. A2A network contract

### 6.1 Explicit configuration and authentication

`<data_dir>/a2a/peers.yaml` is owner-managed configuration, atomically reloaded
through `/a2a reload`. No model tool adds peers or grants exports. Example:

```yaml
peers:
  lab:
    card_url: https://lab.example/agents/research/.well-known/agent-card.json
    outbound_secret: a2a_lab_outbound
    inbound_secret: a2a_lab_inbound
    sessions: ["<local-session-uuid>"]    # permitted local outbound roots
    exports: [research]                    # inbound export allowlist
exports:
  research:
    session: "<local-session-uuid>"
    name: Research
    description: Answer explicitly submitted research requests.
```

Values in `sessions` are exact roots; their enabled descendants may use the
binding. Export session is exact: granting it does not grant its whole tree.
Outbound-only peers may omit `inbound_secret`/exports; inbound-only peers may
omit card URL/outbound secret/sessions. Unknown session IDs, duplicate names,
missing referenced secrets, invalid URLs or incompatible cards fail validation
without partially installing the new configuration.

Secrets are vault **names**, resolved through `secrets::Handle`. Use separate
random inbound and outbound credentials; never the console token, provider
key or link pairing secret. Obtain/set them through the existing secret UI/CLI.
Credentials are transported only in `Authorization: Bearer`, never a query,
message, model tool argument, card or log. This profile supports configured
bearer auth; peers requiring other schemes fail clearly, not anonymous fallback.
Do not build OAuth/JWT issuance, accounts, key exchange or an agent PKI here.

For an export named `research`, serve its SDK card at
`/agents/research/.well-known/agent-card.json` and JSON-RPC at
`/agents/research/rpc`. Direct card configuration is standard A2A discovery;
no public list of exports is served. Cards and RPC both require a credential
whose binding grants that export, and effective session communication.
Card `supportedInterfaces` advertises the exact HTTPS URL and version 1.0.
No public title, task history, transcript or enabled tool inventory.

A single optional loopback listener avoids merging two independent protocols
into jsonrpsee's console API. Production exposure requires an existing HTTPS
reverse proxy and explicit `public_url`; plain HTTP is development/loopback
only. Configure `Cache-Control: private, no-store` on protected cards and
reject browser origins rather than enabling CORS. Do not add an A2A relay mode
to link or proxy A2A requests through console `Api` credentials.

The session remains private even if a token holder guesses its UUID. Every
get/list/send/cancel/subscribe is authorized against credential → peer binding
→ export → session → owned task/context, **before** lookup or pagination.
Unknown and unauthorized task IDs produce the same protocol not-found result.
Raw headers, `tenant`, metadata and `contextId` cannot select a different
principal. Authentication context is constructed by HTTP middleware and
carried to the request handler through a trusted, non-model-controlled seam;
strip/reject any incoming headers used internally to represent that context.

### 6.2 Supported protocol profile

| Operation/capability | Contract |
|---|---|
| Agent Card | SDK schema; authenticated export card; text input/output, explicit capabilities. |
| `SendMessage` | Accept a text request; return Task or direct Message as allowed by the SDK/spec. Inbound work normally creates a Task. |
| `SendStreamingMessage` | SSE task snapshot first, then ordered status/artifact updates. A message-only stream has exactly one Message. |
| `GetTask`, `ListTasks` | Durable, caller/export-scoped state; cursor pagination; empty final `nextPageToken` present; `historyLength` honored. |
| `CancelTask` | Cancel only the specified request and owned work; terminal races resolved atomically. Not `Cancel(session)` against unrelated work. |
| `SubscribeToTask` | Current task snapshot first, subsequent updates; reconnect is not a replay guarantee. On a terminal task return `UNSUPPORTED_OPERATION`, the pinned kit's reading of the spec. |
| Push notifications | Explicitly false; all push-config operations return the standard unsupported error. No callback URL/SSRF surface. |
| Extended card | Explicitly false; no second private schema. Ordinary export cards are already authenticated. |
| gRPC, REST, SLIMRPC | Not advertised by theuth's server. SDK support does not obligate theuth to expose them. |
| Binary/data parts, remote file URIs | Not accepted in this text communication profile. Return standard unsupported-content errors; never fetch/execute silently. |
| Required extensions | Reject unsupported required extensions. No private extension is needed for basic messages/tasks. |

Explicitly set capability booleans; the reviewed default handler treats
missing flags too permissively. Unsupported version/content/operation errors
must be SDK protocol errors, not successful empty objects or generic 500s.

For outbound asynchronous work, explicitly set `returnImmediately: true`.
Honor the inbound request's blocking/nonblocking choice. The spec's general
“returns immediately” prose conflicts with its specific configuration rule;
use the spec's §3.2.2 explicit `returnImmediately: false`/unset behavior: wait for a
terminal or interrupted state, without holding an actor mailbox. SSE is a
separate operation, not inferred from a timeout.

### 6.3 Sessions, contexts and tasks are not the same identity

An export selects a theuth session. An A2A context is an opaque, server-minted
conversation handle **within `(principal, export)`**. A task is one request's
execution, possibly spanning several native turns and child/job completions.
Neither is the SessionId or an authorization credential.

- Neither context nor task supplied: create a context and return it. With only
  a task ID, infer its existing authorized context. A supplied context must
  already belong to the caller/export; reject unknown/mismatched context,
  rather than silently joining an owner conversation by UUID.
- No task ID: create a new server task ID. Supplied task ID must exist, be
  authorized, nonterminal, and agree with any supplied context.
- A completed/failed/canceled/rejected task never reopens. A follow-up uses
  the same context and a new task. Replies to `input-required` resume the
  existing task, not a new one.
- One active A2A request scope per exported session. Further new requests are
  durably queued within the bound; they do not share an active turn. Preserve
  per-recipient accepted order. Native unsolicited peer notices may steer
  between batches; independent remote tasks wait for their own boundary.
- Bind the initiating input, native TurnIds, delegated child work, tool/job
  activity and queued result continuations to the active request scope.
  `TurnFinished` alone is not Task completion while that scope owns pending
  work. Complete only after a final response and no owned pending work or
  continuation. This requires explicit native lifecycle accounting, not
  parsing “done” from prose or adopting a UI running flag.
- Human input during an exported task is **never shared into the task**. As
  built, `UserInput.scope`/`Input.scope` is `None` for every human or
  unrelated harness input and `TurnScope::admits` only lets an input join a
  turn whose scope matches, so what the owner types while a peer task runs
  waits behind it and starts a private turn of its own; it cannot leak
  through task history. The desktop composer says so (“Send privately…”; the
  notice above it: “Replies in this session can be shared with that peer …
  What you type here stays private: it never joins a peer's task and waits
  while one runs”). There is no “steer the peer's task” input; the only way a
  human contributes to a peer exchange is the explicit `answer_peer_request`
  (§5.3). The first draft's “response shared with <peer>” steer was not
  built. The remaining exposure is the one stated in the notice: the model
  may summarize anything already in the session's context into its reply to
  the peer (§9).
- A2A history contains only exchanged messages for the task, not the theuth
  transcript, system prompt, reasoning, tool arguments/results or another
  caller's context. Final output is a text Artifact. Task status messages are
  progress/questions, not a copy of every local event.

| Native condition | A2A state |
|---|---|
| Accepted, waiting for execution | `SUBMITTED` |
| Request scope executing or awaiting owned background work | `WORKING` |
| Explicit question to the initiating peer | `INPUT_REQUIRED` |
| Local human approval needed | `AUTH_REQUIRED`, generic explanation; approval stays in the owner's UI |
| Final response committed and owned work settled | `COMPLETED` |
| Execution error / interrupted by restart | `FAILED` |
| Cooperative cancellation settled | `CANCELED` |
| Request declined after a task was accepted | `REJECTED` |

Native questions use `send_message` to the initiating peer with its exact
`reply_to`; `await_reply: true` maps to `INPUT_REQUIRED`. The peer answers via
`SendMessage` on the same task/context. One pending external question per
active task; further questions wait or return a clear error. The mapping is
stored state, not a private required A2A metadata field. A nonawaited progress
message does not finish the task; finalization does.

For an inbound-only peer, correlated responses and questions use the already
authorized task's response/stream channel; they do not require a callback URL
or permission to initiate arbitrary outbound tasks. New uncorrelated sends
still require an outbound binding. On the client side, import a remote
`INPUT_REQUIRED` status as a durable question addressed to the initiating
local session. Its local message ID maps to the remote task/context; answering
with `reply_to` sends on that task. A remote `AUTH_REQUIRED` becomes a visible
owner intervention request, never an automatic login or approval. Persist
final remote results before notifying the initiating session, including when
the original nonblocking tool call has already returned or its wait expired.

An ACP proxy is not an exportable execution backend in this profile: native
request ownership and cancellation cannot be imposed on an opaque ACP loop.
Validate exports as native sessions. Distinct network principals must not
share one exported native session; configuration rejects that arrangement.
The owner can create separate dedicated sessions instead. Context IDs group
that one principal's exchanges, not isolated model memories within a session.

An incoming `AUTH_REQUIRED` message never approves anything. Human approvals
are local, expire normally, and resume the native task when resolved. A remote
agent's own tool policy is outside theuth's control; its Agent Card must not be
read as a promise that theuth approvals mediate every remote action.

### 6.4 ACP and nodes

Keep `spawn_subagent { agent: ... }` for installed ACP harnesses and paired
nodes. They remain addressable local child proxies and can receive enabled
peer messages, queued behind an active ACP prompt. Report capabilities
truthfully: `send: true`, native reverse peer tool access: false.

Do not claim A2A makes an arbitrary ACP child able to discover/call siblings.
An external harness with its own A2A implementation can be configured as an
A2A peer; bidirectionality then uses its A2A endpoint and grants. A remote theuth
can likewise export sessions independently. ACP remote session IDs and A2A
contexts stay separate. No automatic fleet-wide export, token sync or mapping
of a node's whole subtree into the parent's namespace.

## 7. Durable intake and recovery

Extend the existing agent SQLite store; no second message database or Redis.
Use migrations and typed store operations, not plugin access to SQLite behind
`Agents`' back. Records as built (`crates/agent/migrations/0011_communication.sql`,
`0012_answered_by.sql`; typed access in `agent/src/store/communication.rs`):

| Record | Durable fields |
|---|---|
| Root communication scope | `sessions.scope` (`owner`/`isolated`, default `isolated`), `sessions.alias`. Children inherit through stored lineage. |
| Peer exchange (`peer_exchanges`) | stable message id, authenticated sender/recipient keys and sessions, owning session, `(sender, request_id)` unique dedup key, content fingerprint, `reply_to`, `request_scope`, admitted generation, bounded redacted body, `accepted_seq`, state, reply, remote `task_id`, error, `history_seq` link set once at consumption, `answered_by` (`owner:<session>` for a human answer). |
| Remote peer binding (`peer_bindings`) | server-minted id, alias, endpoint, `revoked_at`; one live row per alias. |
| A2A context (`a2a_contexts`) | opaque context id, principal, export, session. |
| A2A task (`a2a_tasks`) | the **full SDK `Task` snapshot as JSON** (history, status message, artifacts, metadata), principal/export/session, unique `request_scope`, mirrored `status`, `revision`, `cancel_requested`, error, `dedup_exchange`. |
| A2A task work (`a2a_task_work`) | `(task_id, key, settled)` for every turn, child, ticket or operation the scope owns; a task completes only when all rows are settled. |

Use SDK types for protocol snapshots, not hand-maintained parallel wire
schemas. Inbound task state is retained in full and without a TTL until the
owning session is deleted; the store's `status`/`error` columns are
authoritative after a restart and are projected over a stale snapshot. The
theuth transcript is never appended to a Task's history by these paths. Outbound
(client-side) remote task snapshots and follower state are memory only; the
durable exchange keeps the remote task id, body, reply, state and error, not
the remote history or artifacts. In-process wait channels, network sockets,
config generations and task subscribers are derived memory, not serialized
handles.

Acceptance transaction: validate current route and quota → reserve request ID
with content fingerprint → insert bounded input/task binding → commit → return
receipt → notify recipient. Transaction failure returns failure and starts no
turn. Duplicate `(authenticated_sender, request_id)` with identical target,
body and reply association returns the existing outcome; changed content or
target is an error. JSON-RPC request IDs are not dedup keys.

Consumption transaction links the intake row to an append-only history row
and marks it consumed, before model execution. A repeated wakeup observes the
link and cannot append it again. Waiter replies use the same durable exchange
and append their tool-result linkage once. No “delivered” event before commit.
Normal non-task peer input still uses the existing safe batch boundary.

Automatic child results acquire a stable dedup key from `(child, TurnId,
parent)` and use the same intake path. If an explicitly bound request already
received that result through its waiter, mark the parent notification consumed;
do not inject it again. Extract results by the associated turn/request, not
“assistant text since the latest user row.” A sibling request does not steal
or redirect the child's separate parent result.

Restart policy follows 01: **do not auto-run interrupted agent work**.
Unconsumed accepted work becomes `failed: interrupted_by_restart`; running
A2A task scopes become `FAILED`, retaining partial artifacts and the interruption
reason. Previously committed final outcomes remain queryable. Already committed
but unlinked result notification is reconciled by its dedup key without running
a model. A user/peer must submit a fresh request to continue. This is durable
accounting, not an at-least-once execution promise.

Outbound A2A send idempotency is optional in the standard. Persist its message
ID and any known remote task ID. If transport fails after bytes may have left:
query the known task; without a task handle, report `unknown` and require an
explicit decision before resending. Do not blindly replay SendMessage or infer
failure from an HTTP deadline. Read/status reconnects may retry with the existing
bounded backoff style; retries never create a new task silently.

Retain exchange/task accounting with its owning session until explicit session
deletion. Page histories and do not preload them all. Deleting a session revokes
exports first, resolves waiters and task bindings, then uses existing deletion
semantics. Peers subsequently receive not-found. Fork/undo/redo/clear/compaction
never copy or replay protocol tasks, reconnect subscriptions, revive suppressed
input or resend old messages. Delivery IDs are not message-row sequence numbers,
which rewind can reuse. A future [48](48-shake.md) context mask likewise does not
recall an already sent message.

## 8. Disable races, cancellation and resource bounds

`Agents` serializes route admission with settings changes. A live communication
generation accompanies each admitted exchange/network operation. Recheck at:

1. tool exposure (usability only);
2. send admission, after any approval has resolved;
3. recipient intake consumption;
4. remote network dispatch and each protected stream emission;
5. reply/result routing.

An opt-out invalidates old generations for affected sender/recipient subtrees;
a global-off invalidates all. An already approved but not dispatched send fails.
A process restart has no valid old generation. Registry-owned calls that outlive
their caller cannot reuse a revoked admission. This is the enforcement; hiding
a tool or invalidating a cached card alone is insufficient.
Root scope changes and peer/export reload or credential revocation install the
same barrier for affected bindings. Recheck the complete route, not just its
boolean switch; disabling one export must not leave an already open task stream
as a retained authorization grant.

As built (`agent/src/communication.rs::Generation`, `Policy`): a generation
is a revision plus two `CancellationToken`s, the global epoch and the
session's token nested under its persisted ancestry; it is live only while
neither is cancelled. Every global setter (on or off) cancels the epoch and
drops cached tokens; `set_session(false)` cancels the subtree; re-enabling
mints fresh tokens, so an old generation never revives. `tools/src/registry.rs::Invoke`
captures the admission before the approval prompt and rechecks it after
approval and after an execution-permit resume, before secret expansion and
dispatch; `Deliver` rechecks the token it was handed. Ownership: an inbound
task's `request_scope` is carried side-band on `UserInput.scope`,
`PeerInput.request_scope` and `acp_child::Queued.scope`, never in the
`<message>` wrapper text, and children spawned from a scoped turn are
recorded as owned by it.

The setter must complete suppression and install the revocation barrier before
returning success. Config persistence occurs before publishing the new global
state; if the write fails, retain the old state and return an error (as built:
`SetCommunication` writes the YAML atomically first, then mutates). Off
suppresses accepted-unconsumed exchange rows durably (`communication_disabled`),
sends `SuppressPeerInputs` to native actors, expires exact waiters with
`communication_disabled` and stops the A2A listener through the
`CommunicationChanged` event (asynchronously; the output gate closes first).
ACP children recheck the revoked token at consumption. No silent “saved”
response on a logged write failure.

Cancellation addresses request scope, not a whole conversation. As built
(`network.rs::cancel_task`, `supervisor.rs::cancel_peer_scope`,
`loop_.rs::CancelPeerScope`): a still-queued `SUBMITTED` task becomes
`CANCELED` without starting; an active task records `cancel_requested`, asks
only its scope's turn, queued scoped input and owned children (native or ACP)
to stop, expires its exact waits, drops late owned-child continuations, and
waits up to `CANCEL_WAIT` (10 s) for owned work to settle; on timeout the
response is the honest current `Task`, never an invented `CANCELED`. A
terminal task answers `TASK_NOT_CANCELABLE`. Unrelated turns, queues,
children and waits in the same session are untouched; tool side effects
already started are not rolled back. Dropping SSE or closing a frontend is
**not** cancellation. No task can cancel unrelated owner work or a
neighboring peer's task.

Limits are constants except the existing user-configured child/reply limits
(`subagents.max_concurrent` 8, `max_runtime_secs` 600, `reply_timeout_secs`
120); tune from measurements, not a new configuration matrix. Values as built,
with the constant that holds each:

| Limit | Value / consequence |
|---|---|
| Peer text | 32 KiB UTF-8 before wrapping (`communication::PEER_TEXT_MAX`, `network::inbound_text`); reject oversized requests, never silently truncate the task brief. External ids ≤ 256 bytes (`store::EXTERNAL_ID_MAX`). |
| JSON HTTP body / fetched Agent Card | 64 KiB / 64 KiB (`server::BODY_LIMIT` as an outer `RequestBodyLimitLayer`, `client::CARD_LIMIT`); reject before parsing/retaining more. Unary SDK responses 2 MiB and one raw SSE event 64 KiB in the vendored client (§3.4). |
| Task text artifacts / status | 1 MiB cumulative artifacts per task (`ARTIFACT_BYTES_MAX`, both directions), 32 KiB per status message (`client::STATUS_TEXT_MAX`); imported remote task ≤ 4096 items (`TASK_ITEMS_MAX`). Persist bounded chunks and reject excess visibly; never treat a truncated artifact as complete. |
| Pending peer inputs | 64 per recipient, 1024 per harness (`PENDING_PER_RECIPIENT/HARNESS`); 2 MiB / 32 MiB payload budgets (`store::PENDING_BYTES_*`). Return busy, never silently drop. |
| Pending waits | 32 per session, 1024 per harness (`WAITS_PER_SESSION/HARNESS`); reply timeout `reply_timeout_secs`. |
| Active peer-triggered native scopes | Share the existing child-work semaphore (`max_concurrent`, default 8); include follow-ups, not only initial spawns. Release the execution permit while a scope waits solely for peers, children or humans; reacquire before model/tool continuation. A Python cell awaiting its host calls is suspended, not executing: only those calls count, so a blocking send inside a cell releases exactly as a direct one, and the interpreter resumes only under a permit. Request ownership remains active while the permit is released. |
| Request execution deadline | `subagents.max_runtime_secs` (`supervisor::request_deadline`); wall deadline includes waits, stops unbounded collaboration chains. |
| Automatic communication chain | 256 peer-triggered continuations per initiating request (`CHAIN_MAX`); inherited across local replies/child results, reset only by fresh external/user input. Exhaustion is visible and does not auto-retry. |
| SSE subscribers / buffers | 4 per task, 128 globally (`network::SUBSCRIBERS_PER_TASK/MAX`, `server::SUBSCRIBERS_MAX`); per-subscriber buffer is **64 events** (`SUBSCRIBER_BUFFER`, a broadcast channel), each event itself bounded above, not the 64 KiB byte budget the first draft named. On overflow disconnect that subscriber; stream idle 120 s (`STREAM_IDLE`); retained snapshot enables recovery. |
| Outbound HTTP | 10 s connect, 30 s unary request deadline, stream idle 120 s (`client::CONNECT_TIMEOUT`, `REQUEST_TIMEOUT`, `STREAM_IDLE`); polling a remote task 2–30 s backoff for at most 6 h (`POLL_MIN/MAX`, `FOLLOW_MAX`). No proxy, no redirects, pinned resolved address. Network timeout is not task failure. |

A fixed 10 admitted requests/second (token bucket, burst 10) per authenticated
peer and at most 8 concurrent HTTP request handlers per peer apply before
actor admission (`server::RATE_PER_SEC`, `CONCURRENT_PER_PEER`; 429 and 503
with `Retry-After: 1`), using
existing tower facilities where available. Bound waiting handler state, while
SSE subscriptions use their separate limits; a blocking SendMessage must not
hold the only slot needed to answer an input-required task. A peer can start fresh
requests, so chain counts are not a security boundary; request rate, active work
and byte caps are. Constants' acceptance checks include load and disable latency.
Large final outputs use capped transcript previews and durable task artifacts;
a local absolute spill path is never advertised to a remote agent as a file URL.

## 9. Trust, secret handling and disclosure

Every peer payload is lower-trust than owner input, including a same-process
child that read a malicious website. Persist a structured `InputOrigin` on the
intake/history association: owner, peer (authenticated key/message ID), or
harness result. The provider may still need a user-role message, but that wire
role never changes the stored origin or the routing/approval rules.

Render provenance outside the body, escape wrapper attributes and delimiter
text, and cap before wrapping. Peer aliases/descriptions are escaped data too.
Do not parse `<message from="parent">`, `<system-reminder>`, JSON metadata or
“approved by owner” inside the body into trusted events. A fence improves
presentation, **not prompt-injection security**.

Important current limit: 28's taint/sink enforcement is not built. The default
approval policy remains the consequential-action control; approvals-off remains
an explicit acceptance of that risk. Opt-in communication and a valid token do
not make untrusted text safe. The settings warning must state that an exported
agent can act with its session's existing tool permissions and approvals. This
feature does not quietly add `/yolo`, a scanner, or a second trust-policy engine.

Avoid a specific secrets trap in current `Invoke`: arguments are expanded before
`tool.call`. `send_message` must **redact the expanded body before any peer
persistence, event or transport send**, using the existing secrets hook. Known
secret values become placeholders; unknown literal credentials cannot be
reliably detected. Receiving peer data never expands placeholders as a transport
operation. They can only be used later in a local tool invocation, through the
normal approval gate. Peer tokens themselves bypass model args and live only in
SDK auth configuration. Redact peer errors, task artifacts and status text before
logging, persisting or rendering them.

Configured HTTPS origins are an egress allowlist, not arbitrary model-supplied
URLs. Reject credentials in URLs, disallow redirects to a new origin, validate
card-advertised endpoints against the configured origin, and revalidate changed
cards on reload. Do not follow card icon/docs URLs or file parts. Loopback/private
network destinations require explicit owner configuration; no link-local/cloud
metadata destinations, DNS-rebinding to an unapproved address, or redirect-based
credential forwarding. Pin the connection's validated destination for each
request; TLS hostname validation stays on. Use HTTP client's native policy hooks,
not an HTML/URL scanner.

Network principals receive only their exchange outputs. Agent Cards advertise
owner-written functional capabilities, not local tool schemas. Raw reasoning,
provider credentials, approval argument bodies and sibling transcripts never
leave via the A2A adapter. The model can still deliberately summarize private
information into an exported response: exporting an existing session grants
that response channel access to its context. Recommend a dedicated session for
each remote trust relationship; this is not a sandbox or an information-flow
proof. Shared owner memory/files remain outside the communication boundary.

## 10. Commands, events and frontend parity

Owner API as built (`rpc/src/api.rs::Api`; TS bindings exported from
`rpc/src/export.rs`), all thin asks to the appropriate owner:

```text
get_communication(session?) -> CommunicationState
set_communication(session?, enabled) -> CommunicationState      # session None = global
set_peer_scope(root, scope: owner|isolated, alias?) -> CommunicationState   # alias None keeps, "" clears; root only
list_peers(session, query?, cursor?, limit?) -> PeerPage
peer_message(session, id) -> PeerRecord
list_peer_messages(session, cursor?, limit?, pending_only) -> PeerMessagePage
answer_peer_request(session, reply_to, text) -> PeerReceipt
a2a_status() -> A2aStatus
reload_a2a() -> A2aStatus
```

`CommunicationState` includes global value/source, direct session opt-out,
inherited blocker SessionId, effective value, root scope, network configured /
listening state and revision. `Loaded` and `Info` expose relevant snapshots.
`list_peers` is the same permitted directory as the model sees; the existing
human `/agents` remains the unrestricted owner subtree view, including disabled
children. The human roster's visibility does not permit a model send.

Model `send_message` routes through `Deliver`, not a new owner console send API.
Internal inbound A2A commands carry validated principal/export identity, typed
SDK request data and generation; they are not exposed as unauthenticated RPC
methods or model arguments. Names such as `RegisterPeerSender` are internal
messages, not frontend methods.

Events belong to their owners:

- `agent::CommunicationChanged { session: Option<SessionId>, revision }`:
  invalidate affected root/subtree settings and directory snapshots. A global
  event reaches all owner clients, including scoped subscriptions.
- `agent::PeerMessageChanged { session, id, state }`: per-participant receipt
  change; reload durable record when needed. No body broadcast to unrelated
  model sessions or remote principals.
- `agent::PeerDelivered { session, origin: PeerOrigin { id, from, reply_to,
  scope }, text, seq, turn }`: a peer exchange became part of a session's
  transcript, emitted at consumption for transcript rendering. Built as its
  own typed event rather than the first draft's augmentation of
  `AgentEvent::MessageDelivered`. Not emitted for a suppressed or merely
  accepted input.
- `a2a::A2aChanged`: network configuration/status invalidation, no secrets.

The RPC broadcast bridge is not delivery storage. On `Lagged`, reconnect or
foreground, reload settings, peer requests and session history. Filter both
sender and recipient views deliberately, without forwarding global owner
subscriptions to an A2A client. SDK task streams subscribe to their authorized
task, never to `subscribe_events(null)`.

| Surface | Required integration |
|---|---|
| TUI | Slash entries, existing picker, `communication off`/inherited reason in status; pending requests by ID; explicit answer/inspect controls. |
| Desktop/mobile | Settings beside approvals: global and session switches with exact precedence; root scope and network status; shared peer picker and transcript cards. No Tauri-only logic. |
| Signal/Slack | Owner-only settings commands, compact status and request IDs; non-owner contacts cannot grant scope, enable global communication or reload credentials. |
| RPC/exported TS | Trait, adapters, method inventory, binding exports, frontend interface and event reducers updated together. |

Settings are authoritative on first open. Do not copy the current desktop
approvals component's guessed session value or “overrides the default” wording.
Changing state on one frontend invalidates all others. Disabled rows explain the
blocker, switches have accessible labels and pending/error state, and mobile
controls use existing touch targets. Remote failures do not freeze local chat.

## 11. Implementation boundaries and documentation amendments

These are complete integration boundaries, not an excuse to call a local-only
subset a finished A2A system. Status per step as of 2026-09-18 (evidence in
§12.3–12.5):

| Order | Files / responsibility | Runnable acceptance | Status |
|---|---|---|---|
| 1 | SDK dependency spike: workspace pins, throwaway `a2a_sdk_probe.rs` (§3.4; removed with its dev-dependencies before merge, source kept outside the repository as a proof artifact); reviewed SDK routers/handler/client. | Probe self-test passes; exact release set against pinned spec and official test kit plus an official Python/Go peer; classify every applicable failure/skip. | Done for the probe: 45/45, kit 80/155/30 with two fixture corrections, Go CLI both directions (§3.4). Not a production-adapter measurement. |
| 2 | `agent/src/{lib,peers,supervisor,store,loop_,communication}.rs`, `agent/src/communication/network.rs`, `agent/src/store/communication.rs`, migrations 0011/0012; `tools/src/{registry,lib,communication}.rs`. | Global/session/scope authorization, UUID correlation, transactional intake, generation revocation, bounded waits and native request lifecycle. Exercise Python and direct tools identically. | Done: native smoke 57/57 and runtime tests 273 green after every review correction (§12.4). |
| 3 | `crates/a2a` plugin (`lib`, `config`, `server`, `client`), workspace dependencies, plugin vector/default config, `vendor/{pbjson-types,a2a-client}`. | Authenticated cards and bidirectional SDK messages/tasks over HTTPS; no listener/fetch when off; failure cannot break normal harness startup. | Built and exercised on loopback HTTP: official Python SDK smoke 39/39 + closed-export 3/3 against the production export, final rerun after every review correction (§12.4). Kit and Go CLI never run against the production adapter; HTTPS never exercised. |
| 4 | `rpc/src/{api,bridge,export}.rs`; TUI; shared desktop/mobile RPC/store/components; Signal/Slack commands. | Real frontend controls, blocked reasons, message/task rendering, reconnect and cross-client synchronization. | Done: frontend 134 tests + `tsc` green, channel/TUI/RPC 208 tests green; enabled desktop surface visually approved by the user (§12.4). Two-client convergence not exercised. |
| 5 | Existing design documents and README. | As-built record names exercised and unexercised paths, actual dependency versions and conformance reports. | This revision; 00/01/06/07/19/25/31/32/36/37 amended. |

Amended alongside the implementation (all in the working tree):

- **00:** plugin/dependency/config tables, permission-versus-availability
  distinction, peer topology, durable intake, network profile.
- **01 / 19:** default-off delegation, canonical peer addressing, explicit
  discovery, exact waiters/human answers and scoped result delivery.
- **06 / 32:** accurate ACP reverse-channel limitations; A2A is separate,
  not a migration of node transport.
- **07 / 36:** structured origin, durable intake links, request-bound results,
  tail-only discovery, suppression/restart behavior. No cache invariant change.
- **25 / 31:** parity tables, settings/events and reload behavior over the same
  existing console transports; no new link wire mode.
- **37:** scrub-before-peer-egress boundary and credential references.
- **README / first-run config:** opt-in instructions, session-memory lifetime,
  breaking default behavior, HTTPS setup and honest maturity/release status.

46 apps (a proposal that is not in this tree), [47 todos](47-todos.md),
[48 shake](48-shake.md) and
28 are proposals, not assumed prerequisites. Request-scoped work accounting is
required here because A2A completion/cancel cannot be inferred from TurnFinished.
If 47's activity ledger lands first, extend it rather than building a second
ledger; otherwise implement the required ownership/settlement slice in `agent`.
Do not reset app/todo/context state as a side effect of a communication toggle.

## 12. Acceptance and adoption gates

### 12.1 Library release gate

Before network A2A is marked shippable:

1. Pin compatible published SDK releases and protocol/test-kit revisions;
   record license, toolchain/target builds, dependency/security review and
   any narrow upstream patch with its removal condition.
2. Run the actual theuth adapter through the official conformance kit for the
   advertised JSONRPC/text/streaming profile. Every applicable MUST passes;
   a skipped required case is not a pass. For a kit/spec disagreement, retain
   a pinned reproducer and authoritative adjudication, not blanket exclusions.
3. Exercise both directions with an official Python or Go SDK: authenticated
   card, version negotiation, direct Message response, Task, blocking and
   nonblocking send, input-required, local approval/auth-required, follow-up,
   history limits, terminal resend, cursor pagination and terminal errors.
4. Prove no unknown client task ID creation, mismatched context acceptance,
   caller-unscoped list, permissive missing-capability behavior, or cancel race
   in the chosen application+SDK path. Current published default-handler
   behavior does not meet this gate unchanged.
5. Record remaining operational maturity evidence honestly. Passing theuth's
   profile does not establish years of production hardening or an independent
   audit. The user explicitly accepted the younger dependency on 2026-09-18
   (§3.4); that acceptance lowers only the mature-library requirement for the
   dependency choice, not gates 1–4.

### 12.2 Behavior and failure checks

| Scenario | Required observable result |
|---|---|
| Fresh config / upgrade with missing key | No peer tools usable, no spawn, no A2A socket/fetch; ordinary owner chat/tools work. |
| Full switch truth table | Global off defeats session on; ancestor off defeats child on; session off blocks both directions; approval switches cannot enable a route. |
| Late approval / queued-send race | Disable while send waits for approval or a tool batch; neither late Allow nor a stale tool schema dispatches it. |
| Re-enable and restart | Suppressed input does not wake; session opt-outs reset only on daemon restart; global-off remains; interrupted requests have durable failed outcomes. |
| Two roots and two siblings | Enabled owner roots and same-tree siblings discover and exchange without parent-model mediation; isolated roots and disabled targets do not leak existence/title/history. |
| Lazy ancestry / fork | Unloaded ancestors still block; a disabled source does not fork into enabled public scope; unknown UUID never creates a session. |
| Concurrent correlation | Two requests to the same peer, replies reversed: each reaches the correct waiter; wrong sender/id rejected; timeout cannot consume a newer request. |
| Human answer | One/many pending requests use explicit IDs; peer text cannot enter the human-answer path. |
| Busy recipient / background work | Input appends at valid boundaries; separate A2A requests do not share turns; child/job completion delays the owning task's finalization and does not finish another task. |
| Persistence fault injection | Crash before acceptance commit starts nothing; after commit yields one record; duplicate wakeup links one history occurrence; repeated identical request ID returns the same record; changed payload fails. |
| Remote ambiguity | Disconnect before task ID is known yields unknown outcome, no blind resend; known task reconciles via GetTask; SSE disconnect leaves execution running. |
| Authorization | Principal A cannot get/list/cancel/subscribe to B's tasks; forged context/tenant/export cannot widen access; protected card and empty global list do not leak metadata. |
| Cancellation | Queued task never starts; running task only cancels owned work; completion/cancel is one terminal result; uncancelable side effects are reported honestly. |
| External input | Closing tags, fake parent names and “approved” text cannot forge provenance, resolve approvals or change scope. |
| Secrets / SSRF | Known vault values absent from peer body/store/logs; tokens only in auth headers; cross-origin redirects, card endpoint substitution and unauthorized IP resolution fail. |
| Load / bounds | Oversized body/card, full inbox and slow SSE subscriber fail explicitly without unbounded memory or blocking local owner input/disable. |
| Actual frontends | TUI and shared desktop/mobile show effective settings, inheritance, peer/request states and errors; a second client converges after toggles, Lagged and reconnect. |
| Existing delegation | Native child, ACP child and node follow-up retain their execution semantics when enabled; off has no automatic-result bypass; external reverse messaging is never advertised unless actually implemented by that peer. |

Keep focused regression tests for authorization, correlation, atomic acceptance,
revocation and terminal-state races. Verify wire behavior through an actual SDK
peer and UI behavior in actual frontends; source-text assertions and mock echoes
do not prove these contracts. Measure local typed-route overhead separately
from network latency; no unmeasured “zero cost” or throughput claim.

### 12.3 Design verification record

Research covered the architecture/plugin guide, config/subagents, ACP, context,
course correction, board, desktop, trust proposal, link/nodes, survival, secrets
and related newer proposals. Current peer/intake/store/approval/RPC/frontend
seams were traced against source. The official specification, SDK listing,
release APIs/docs, conformance workflow/report and two Rust alternatives were
inspected. crates.io API requests returned 403; docs.rs and upstream release
sources supplied package evidence instead.

Checks performed for this document: 24 local links resolved, 13 reference-source
labels resolved, eight code-fence pairs balanced. A throwaway Python/SQLite
model passed all 16 global/ancestor/self enablement combinations; symmetric
sender/receiver and scope checks; duplicate acceptance and changed-input
rejection; single history consumption; suppression across re-enable;
transaction rollback; and out-of-order, forged and late reply correlation.
That model exercised proposed policy/storage semantics, not theuth's production
actors or SDK.

Revision 2026-09-18 (SDK acceptance): SDK sources at tags `a2a-lf-v0.3.1`,
`a2a-client-lf-v0.2.5`, `a2a-server-lf-v0.4.4` (lib, types, errors, event,
agent card, JSON-RPC router/transport, SSE, middleware, handler, pagination,
task store) and the conformance kit at commit `263b9cf` (runner, fixtures,
requirement specs, scenarios, SUT generator, auth docs) were read; findings
are in §3.4. Workspace pins and the probe example were added. Probe-level
evidence: self-test 45/45 after three probe-side fixes and one dependency
patch; Go CLI `a2a` v2.5.0 → probe send succeeded; probe SDK client → Go echo
peer 5/5; conformance kit `263b9cf` raw 2 failed (kit defects), 80 passed /
155 skipped / 30 deselected / 0 failed with the two fixture corrections, the
kit's summary still reporting 22 unmeasured MUST requirements (§3.4). None of
this is a conformance claim and none of it exercised the production adapter.
Two narrow dependency patches (`pbjson-types` timestamp Z-normalization;
`a2a-client-lf` 2 MiB unary / 64 KiB SSE-event bounds, §3.4) are in the tree
with their removal conditions recorded.

### 12.4 Runtime verification record (2026-09-18)

What was run against the production runtime. Each line is an observed run,
not a gate closure; §12.5 lists what was consciously not run. The two drivers that
produced the smoke rows were throwaway verification code: they have been
removed from the tree before merge (together with the `agent` dev-dependencies
only they used) and their sources are preserved outside the repository as
proof artifacts. Nothing in this section is a runnable path in this tree,
a test suite, or a conformance claim.

| Check | Result | Caveat |
|---|---|---|
| `cargo check --workspace --all-targets` | Green on the cleaned workspace (drivers and their dev-dependencies removed), after every review correction below | |
| Runtime (Rust) tests: `agent`, `a2a`, `tools` | 273 passed (189 + 24 + 60), final rerun after every review correction | Counts are this run's; unrelated work moves them. |
| Channel/TUI/RPC tests: `rpc`, `signal`, `slack`, `tui` | 208 passed (3 + 57 + 90 + 58) | Command parsing, status rendering, request answering; no live Slack/Signal. |
| Frontend (`crates/desktop/ui`): `vitest` + `tsc` | 134 passed, `tsc` clean | Reducers, RPC surface, settings/peers components. |
| `theuth` binary build (`cargo build`) | Green | |
| `cargo clippy` | Not clean: two **preexisting** warnings in code this work did not touch, `clippy::collapsible_if` at `crates/tools/src/jobs.rs:697` and `clippy::type_complexity` at `crates/tools/src/pty.rs:706` | Left as found; unrelated to communication and out of scope for this change. `clippy::all` is `warn`, not `deny` (00 §10). |
| Native smoke driver (`communication_smoke`, 57 checks) | 57/57 `ok`, exit 0, rerun after every review correction | Inert provider (every completion HTTP 400): proves routing, gating, intake, waiters, suppression, inheritance and accounting; not model execution, peer-visible text or approval prompting. |
| Network smoke driver (`a2a-smoke.py smoke`) with the official Python SDK `a2a-sdk` 1.1.4 against a running production export | 39/39, final rerun against the production binary after every review correction | Loopback, plain HTTP, bearer auth; every send starts and cancels a real turn; the model's output is not asserted. |
| Network smoke driver (`a2a-smoke.py disabled`) | 3/3, same final rerun | Closed export: 401 without, 404 with a valid bearer. |
| Enabled desktop surface (Settings → Communication, peers panel, composer notice, transcript peer cards) | Visually approved by the user | Manual, one client; the two-client convergence row of §12.2 was not exercised. |

Not run, and not claimed: the conformance kit or the Go CLI against the
production adapter (only against the probe, §3.4, so §12.1 gates 2–3 are
**not** closed for the production adapter; gate 4's properties are covered by
the Python smoke and the unit tests below, not by the kit); the kit's
generated Python SUT; an HTTPS deployment behind a reverse proxy (every
network run was loopback HTTP); a model-driven peer conversation whose text
is asserted; two frontends converging after a toggle.

#### Review corrections

Two source reviews before merge, one of the request lifecycle and one of the
network boundary, produced seven lifecycle and five network findings. Each
was corrected in source and each correction left a regression test in the
run counted above (`agent` unless noted):

| Finding | Correction | Regression test |
|---|---|---|
| A scoped job or child result landing after its task ended was injected as a private turn. | `NetworkState::retire` keeps op→scope rows; `network_input_queued` refuses cancelled, interrupted and missing scopes and returns `false`, so `inject`/`enqueue_peer`/`claim_child_turn` release the ticket and route nothing. | `communication::network::tests::late_results_of_a_cancelled_or_ended_task_start_no_turn` |
| A queued task dequeued under the binding's *current* route token, so a grant revoked while it waited could still start it. | Queued tasks remember the admitting principal's grant epoch and a watcher fails them on revocation; `start_next` starts only under the remembered admission (`peer route revoked` / `admission lost before start`). | `communication::network::tests::queued_tasks_keep_their_admitted_epoch_and_never_start_under_a_newer_one` |
| Private owner turns could fill or settle an exported task; owned child turns were counted as results. | Request-scoped result candidates come only from the exported session's own bound turn; owned children are work, not results. | `communication::network::tests::private_turns_never_fill_or_settle_an_exported_task`, `owned_child_turns_are_work_not_results` |
| An owned child's scoped first turn acquired a second permit beside `StartChildTurn`'s. | `loop_::bounded(depth, &Input)`: scoped harness input is bounded only at depth 0. | `loop_::tests::scoped_child_first_turn_holds_no_second_permit` |
| Two suspended waits of one turn resuming concurrently deadlocked on the turn's own permit. | One per-turn `reacquire` mutex; every reacquisition (`Wait::resume`, `Turn::hold_permit`, cell start) goes through `reacquire_permit`, which never installs a permit for a dead turn. | `loop_::tests::concurrent_resumes_share_one_reacquired_permit` |
| A Python cell counted as executing while its host calls blocked, holding the permit through a blocking `send_message`. | The cell's count is dropped at the `ResolveFutures` boundary; its inner `Invoke`s alone decide the permit and the interpreter resumes only after `reacquire_permit`. | `loop_::tests::python_cell_releases_permit_while_its_calls_wait` |
| A scope cancel did not stop owned children or drop their results. | Owned children are cancelled with the scope; their late results take no intake path. | `supervisor::tests::scope_cancel_stops_owned_children_and_drops_their_results` |
| Inbound grants had no epoch: a reloaded or rotated peer kept its in-flight requests, streams and queued tasks. | `a2a`: per-alias grant epoch (`epochs`), `same_grant` decides survival; retired epochs are cancelled after the swap, unary requests race the epoch to a 401, streams end with one revoked error. | `a2a::tests::epochs_carry_only_unchanged_grants`, `a2a::server::tests::bounded_stream_ends_with_revocation_when_grant_retired`, `config::tests::same_grant_tracks_credentials_exports_and_roots` |
| Credential distinctness was checked by name within one peer only. | Every resolved inbound and outbound value across all peers must differ; the console token is refused. | `a2a::config::tests::credential_values_are_distinct_across_peers_and_roles`, `refusals_are_complete_and_atomic` |
| Remote responses were unbounded before `serde` and retained snapshots grew without limit. | Vendored `a2a-client-lf`: 2 MiB unary body cap (§3.4); `a2a::client::Retained` bounds artifact text (1 MiB), serialized snapshot (2 MiB) and item count (4096) before mutation. | `a2a::client::tests::apply_enforces_artifact_bound_and_replacement`, `apply_bounds_empty_text_growth`, `apply_refuses_binary_and_oversized_status` |
| Remote card names and error strings reached logs and the model unredacted; env proxies could bypass egress pinning. | `remote_text` (vault scrub, own-bearer replacement, 4 KiB cut) on every remote string; `reqwest` client `.no_proxy()`; errors never echo remote identifiers. | `a2a::client::tests::interface_must_be_jsonrpc_1_0_on_the_configured_origin` |
| A local error after the request was sent was classified as a refusal, inviting a blind resend. | Only pre-send failures (serialization, refused interface) are refusals; everything after is `unknown`. | `a2a::client::tests::only_pre_send_failures_are_refusals` |

The native smoke driver was rerun after all of them (57/57); the network
smoke rerun is the open line of §12.5.

#### Drivers (removed; described for the record)

Two throwaway drivers exercised the gates against the production runtime
instead of the probe; results above, contents here. Neither is in the tree.

- `communication_smoke.rs` (local boundary, §§2, 5, 7,
  8): boots the real `tools` and `agent` plugins over a fresh temp data dir
  and drives the production supervisor, store and registry through their
  public commands. `agent.model` is `omlx/theuth-smoke-inert` with the genai
  `omlx` endpoint pointed at a loopback listener owned by the example that
  answers every completion HTTP 400, so every turn ends `stop_reason =
  "error"` after exactly one local request and no provider is contacted. It
  checks: default-off with source `default`; hidden gated tools and
  `communication_disabled` refusals through the registry (`discover_peers`,
  `send_message`, `spawn_subagent`); session `on` under global-off; `on
  always` persisting `config.yaml` before the live switch; owner/isolated
  directory rows (short id, never a title), isolated→owner refused as
  `unknown peer`, self send, unknown UUID creating no row, oversize body;
  scope promotion/demotion by alias; `request_id` dedup and conflict;
  consumption on the recipient; exact waiter resolved by the owner's
  `AnswerPeerRequest` with `answered_by = owner:<session>` and refused for a
  third session; `DropWait`; disable while a second message is queued behind
  a held model call (durable `suppressed`, waiter expiry with
  `communication_disabled`, tools hidden, no revival after re-enable);
  child spawned through the gated tool whose result arrives at the parent as
  a `PeerDelivered` from `session:<child>`; relative `parent`/child-name
  addresses; ancestor opt-out inheritance and ceiling; final accounting
  (turn count == inert request count, all `error`). It exited 1 on any
  `FAIL`. What it could not prove: model
  execution, the text a peer receives, or approval prompting
  (`tools.approvals` was off in its config so no human gate blocked the run).
- `a2a-smoke.py` (network boundary, §§3.3, 6, 8): the official Python
  SDK (`a2a-sdk` 1.1.4, run with `<a2a-tck>/.venv/bin/python`) and `httpx`
  against a running export at `A2A_SMOKE_URL` with bearer `A2A_SMOKE_TOKEN`
  (Main provisions `a2a/peers.yaml` and the vault; nothing is hard-coded).
  `smoke` mode checks 401 before existence (card and RPC, wrong bearer,
  `WWW-Authenticate: Bearer`), 404 for an unknown export, private no-CORS
  card, trailing-slash RPC path, legacy `message/send` → -32601, >64 KiB
  body → 413, one JSONRPC/1.0 interface with bearer scheme, version
  negotiation (`0.3`/empty → `VERSION_NOT_SUPPORTED`, `1.0.7` tolerated),
  `TASK_NOT_FOUND`/`INVALID_PARAMS`/`CONTENT_TYPE_NOT_SUPPORTED`/
  `PUSH_NOTIFICATION_NOT_SUPPORTED`/`EXTENSION_SUPPORT_REQUIRED` validation
  before any state exists, `returnImmediately` Task with our message in
  history, `GetTask` (`historyLength: 0`), caller-scoped `ListTasks`, send to
  a running task → `UNSUPPORTED_OPERATION`, cancel → terminal (honest state
  printed), `TASK_NOT_CANCELABLE`, terminal resend/subscribe refused,
  `SendStreamingMessage` snapshot-first and terminal-last with a mid-stream
  cancel, cross-principal `TASK_NOT_FOUND`/foreign-context `INVALID_PARAMS`/
  unlisted tasks or 404 for an ungranted peer (`A2A_SMOKE_TOKEN_B`,
  `A2A_SMOKE_B_GRANTED`), and per-peer 429/503 caps (normal sections paced
  to 5 req/s per credential; only the burst check runs unpaced). `disabled` mode asserts
  a closed export answers 401 without and 404 with a valid bearer. Every send
  starts a real turn in the exported session and is cancelled; the model's
  output is never asserted. `A2A_SMOKE_AWAIT=1` adds one blocking turn whose
  text is printed for a human to judge (paid; not a script assertion).

Unmeasured MUST requirements of the kit run in §3.4 (`reports/compatibility.json`,
22 `NOT TESTED` + 25 `SKIPPED`), mapped for the production adapter rather
than counted as passes:

| Requirements | Disposition |
|---|---|
| AUTH-SERVER-002, AUTH-SCOPE-001/002/003 | Applicable; covered by `a2a-smoke.py` (401 before existence, per-principal `TASK_NOT_FOUND`, scoped list, 404 non-disclosure). Kit cannot send credentials at `263b9cf`. |
| VER-SERVER-001, VER-CLIENT-002 | Applicable; covered by `a2a-smoke.py` version checks. VER-CLIENT-001 is a client duty: the SDK sets `A2A-Version` itself; theuth's client pre-sets it (§3.4). |
| CORE-CAP-004 | Applicable; covered (`EXTENSION_SUPPORT_REQUIRED`). |
| CARD-EXT-001/002 | Not advertised (`extendedAgentCard: false`); the wire answer is still checked (-32004/-32007, never a card). |
| PUSH-CREATE/GET/LIST/DEL/DELIVER-* (10) | Not advertised (`pushNotifications: false`); refusal path checked on send and create. |
| CORE-CAP-002 | Not applicable: streaming is advertised and exercised. |
| HTTP_JSON-*, GRPC-* (12), BIND-EQUIV-001..004 | Not advertised: one JSONRPC binding only (§3.3, §6.2); no equivalence to test. |
| CARD-SIGN-001..004 | Not advertised: no card signatures. |
| AUTH-INTASK-001..004 | Not applicable to this profile: the adapter does enter `AUTH_REQUIRED` while a local approval blocks an owned turn (`network::network_approval`, §6.3), but the status message only says the owner must approve; the peer is never offered a way to supply credentials in-task and cannot resolve it, so the in-task authentication flow these requirements describe is not advertised. Revisit if it ever is. |
| AUTH-TLS-001 | Manual: HTTPS is the reverse proxy's, `a2a.public_url` must be `https` (§6.1); verify on the deployment, not on loopback. |

### 12.5 Not verified

Nothing in §12.4 is left pending: every check listed there was rerun after
the last review correction. These were consciously not done, and no claim
here depends on them:

- The official conformance kit `263b9cf` against the production adapter. It
  ran against the removed probe only (§3.4: 80 passed with two spec-backed
  local fixture corrections, 155 skipped, 22 MUST requirements unmeasured by
  the kit's own summary). §12.1 gate 2 therefore stands unclosed for the
  production adapter; the evidence that exists is the Python SDK smoke and
  the kit-requirement mapping of §12.4. A future run needs a scripted model
  behind the export (§3.4).
- The Go CLI in either direction against the production adapter (probe only).
  Gate 3 is covered for the production adapter by the official Python SDK as
  a client, not by Go, and not for theuth as a client of an official server.
- An HTTPS deployment behind a reverse proxy: every network run was loopback
  HTTP (AUTH-TLS-001 is manual on the deployment).
- A real, paid model answering a peer with its text asserted; every smoke
  turn used an inert provider or was cancelled before the model's output
  mattered.
- Two frontends converging after a toggle; one desktop client was inspected.
- `cargo clippy` clean: two preexisting warnings in untouched `tools` code
  (§12.4), unchanged.

## 13. Rejected additions

- A custom A2A codec, task protocol, message broker or distributed actor mesh.
- Local loopback HTTP, a second session runtime, or an LLM routing/auto-reply agent.
- Automatic all-session export, global bare-name resolution, public discovery,
  unsolicited transcript sharing or credential inheritance between peers.
- Replacing ACP/link, pretending an ACP harness gained reverse tools, or requiring
  every remote agent to implement a private theuth extension.
- Claiming the SDK's in-memory store, a oneshot or the RPC broadcast bridge is
  durable delivery; claiming exactly-once side effects across crashes.
- Webhooks, arbitrary URL attachments, gRPC/SLIMRPC, an OAuth issuer or a generic
  multi-tenant policy engine for text peer communication.
- Calling a young official SDK “mature” to make a library-selection paragraph
  look finished. The explicit dependency gate is part of the design.

## 14. As built: session tags in the directory (2026-09-22)

The title model's call (07 §15, `crates/agent/src/title.rs`) returns a second
line `tags: a, b, c`; the tags are normalised (lowercase, `[a-z0-9-]`, ≤ 4,
≤ 24 chars, deduplicated), stored comma-joined in `sessions.tags`
(migration `0017_session_tags.sql`), written with the title by
`Store::set_title_at_revision` under the same 48 §4.3 revision guard, and
announced on `AgentEvent::TitleChanged.tags`. A reply without a usable
`tags:` line keeps the stored set; a rename keeps it too.

`PeerInfo.tags` carries them on every **local** row of `discover_peers`
(same tree and other owner roots alike), rendered after the title as
`[rust, merge]`; remote A2A rows have none. This amends §5.1's "no content
of another root": a handful of keywords is disclosed across owner roots on
purpose — it is the whole point of the column, letting a model facing forty
peers pick the relevant ones — while `title`, `cwd`, `model` and the
transcript stay same-tree or hidden as before.

## Sources

Local document links above identify the design contracts; source symbols in §1
and the implementation map identify the current code boundaries. External links
below were inspected during this design; rolling reports and `main` URLs are
explicitly distinguished from pinned releases.

[sdk]: https://a2a-protocol.org/latest/sdk/
[spec-release]: https://github.com/a2aproject/A2A/releases/tag/v1.0.1
[proto]: https://raw.githubusercontent.com/a2aproject/A2A/v1.0.1/specification/a2a.proto
[core]: https://docs.rs/crate/a2a-lf/0.3.1
[client]: https://docs.rs/crate/a2a-client-lf/0.2.5
[server]: https://docs.rs/crate/a2a-server-lf/0.4.4
[released-handler]: https://raw.githubusercontent.com/a2aproject/a2a-rs/a2a-server-lf-v0.4.4/a2a-server/src/handler.rs
[acts-workflow]: https://raw.githubusercontent.com/a2aproject/a2a-rs/main/.github/workflows/acts.yaml
[acts-report]: https://github.com/a2aproject/a2a-rs/releases/download/nightly-metrics/acts_rust.json
[alternative-sdk]: https://docs.rs/crate/a2a-protocol-sdk/0.12.1
[alternative-stability]: https://raw.githubusercontent.com/tomtom215/a2a-rust/main/STABILITY.md
[alternative-tck]: https://raw.githubusercontent.com/tomtom215/a2a-rust/main/docs/official-tck-findings.md
[ra2a]: https://docs.rs/crate/ra2a/0.10.1
[server-jsonrpc]: https://raw.githubusercontent.com/a2aproject/a2a-rs/a2a-server-lf-v0.4.4/a2a-server/src/jsonrpc.rs
[server-card]: https://raw.githubusercontent.com/a2aproject/a2a-rs/a2a-server-lf-v0.4.4/a2a-server/src/agent_card.rs
[client-jsonrpc]: https://raw.githubusercontent.com/a2aproject/a2a-rs/a2a-client-lf-v0.2.5/a2a-client/src/jsonrpc.rs
[pbconv]: https://raw.githubusercontent.com/a2aproject/a2a-rs/a2a-server-lf-v0.4.4/a2a-pb/src/pbconv.rs
[pbjson]: https://github.com/influxdata/pbjson/tree/48ee5989564e0ace82ad71b87736d3d1b2e0e1cd/pbjson-types
[proto-json]: https://protobuf.dev/programming-guides/json/
[tck]: https://github.com/a2aproject/a2a-tck/tree/263b9cfaf16a554bdfb166a7ba5b67716e946349
[go-cli]: https://raw.githubusercontent.com/a2aproject/a2a-go/main/cmd/README.md
