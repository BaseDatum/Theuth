# 15 — Server configuration: many users, one harness, sandboxed execution

Status: preliminary · **shelved** 2026-09-12 · extends `00-architecture.md`
§1, §7, §12; `01-config-and-subagents.md` §1; `06-acp.md` §3. Nothing here
is decided; §9 lists what must be true before any of it starts. Written so
the reasoning survives until then.

## 0. The question in one screen

Today: one process, one data dir, one bearer token, one uid. Several people
can connect at once and see everything. "Multi-user" is three separate
decisions that look like one:

| Layer | Solves | Does not solve |
|---|---|---|
| Owner on sessions + per-user auth | who sees which transcript, events, approvals | isolation of execution |
| Per-user execution worker in a sandbox | the actual isolation | nothing else; the expensive part |
| Postgres instead of sqlite | multi-writer, ownership as rows, ops, more than one box | isolation |
| Redis | login sessions, fan-out across >1 control-plane process | anything with one process |

Storage is the easiest layer and the least important. The tool half is
where the analogy to LibreChat breaks: LibreChat multiplexes model calls and
never executes anything; theuth's value is that it does. The sandbox decides
whether this is a product or a shared shell.

**Rule carried from 00 §1:** the agent loop, the store and the tools never
learn about users. Sessions have owners; frontends authenticate; the worker
boundary isolates. Anything that threads `user_id` through `loop_.rs`,
`compact.rs`, `knowledge/` or a tool is the jarvis shape and is rejected.

## 1. Shape

```
web frontend (accounts, JWT)
     │  ws RPC (existing methods + owner filter)
     ▼
t --serve           control plane: model loop, store, approvals, knowledge, skills
     │  Invoke / ToolOutput over the worker's stdio (same shape as tools::Invoke)
     ├──► theuth worker  (alice)  sandboxed: shell, fs tools, jobs, MCP stdio, ACP harnesses
     └──► theuth worker  (bob)
```

Control plane multi-tenant; execution plane one process tree per user. Same
convergence as every hosted harness (Codex cloud, Claude Code web). Precedent
in-repo: `t subprocess` monty workers (`crates/theuth/src/worker.rs`) — the
binary is already its own worker executable.

Invariant: **the worker never holds provider credentials.** The control plane
makes model calls; the worker only runs tools. `credentials.yaml`, `oauth/`
and the RPC token are outside every sandbox's read set.

## 2. Control plane changes (tentative)

- `sessions.owner TEXT` (migration 0007). `Store::sessions(owner)` filter;
  `Agents` checks ownership on `input/steer/cancel/load/undo/redo/withdraw`.
- Auth: the RPC plugin resolves a principal per connection (jsonrpsee
  connection extensions) from either `users.yaml` (name → token hash, role)
  or a JWT issued by the frontend. Single-user mode keeps `<data_dir>/token`
  and a fixed principal.
- Bridge: `AgentEvent` / `ApprovalRequested` forwarded to a connection only
  when the event's session belongs to its principal. `Approval` resolves from
  the owning connection only.
- Per-principal state that stops being global: `prompt.md`, default model,
  knowledge bundle (doc 10 is single-user by construction — "the user's
  machines, projects"), skills, `mcp/`, `acp/agents.yaml`. Layout:
  `<data_dir>/users/<name>/{prompt.md,knowledge/,skills/,mcp/,acp/}` with
  the top-level files as the single-user case. Provider credentials stay
  the operator's unless a per-user OAuth login (05) is wanted — then
  `users/<name>/oauth/`.
- Usage per owner falls out of the column.

Size guess: ~600 lines across rpc/agent. Touches the two seams the gaps doc
(12) already named: the RPC facade list and the approval gate.

## 3. Execution plane: `theuth worker`

One long-lived process per user, spawned on the first tool call, reaped
when idle (`worker_idle_secs`, default 30 min). Hosts every tool that
touches the host:

| In the worker | Stays in the control plane |
|---|---|
| `shell`, `jobs`, `cancel_job` | `python` (monty: no I/O, already sandboxed) |
| `read_file`, `write_file`, `edit`, `list_dir`, `grep` | `spawn_subagent`, `send_message` |
| MCP stdio servers (`mcp/*.json`) | knowledge tools, skills, todo |
| ACP harnesses (`acp/agents.yaml`) — full agents with host access | remote MCP (HTTP) — keyed by the user's tokens, no host access |

Protocol: length-prefixed JSON on the worker's stdio, one `Invoke` in,
streamed `ToolOutput` / `JobEvent` / `ApprovalRequested` out. The approval
gate stays in `ToolRegistry::Invoke` **before** dispatch; the worker never
decides policy. Spill files (`07 §6`) land in the worker's own dir and are
read back through the worker's `read_file`, so the control plane never opens
a user's filesystem. A dying worker fails every in-flight call with one
error and drops its jobs/MCP/ACP children with its process group (14 §2).

Why a long-lived worker and not a sandbox per command (what Claude Code and
Codex do): per-user *state* — cwd, jobs, MCP server processes, ACP sessions —
needs one process tree to live in, and everything that tree spawns inherits
the sandbox for free. Per-command sandboxing is the single-user design.

## 4. Sandbox: one worker, several launchers

The control plane never learns how the worker was launched. Variance is the
argv prefix plus per-user paths, a config table rather than plugins:

```yaml
server:
  executor: host            # host | docker | k8s
  worker_idle_secs: 1800
  workers_root: /srv/theuth/users   # host: <root>/<name> is the only writable tree
```

| `executor` | Launch | Isolation | Adds |
|---|---|---|---|
| `host` | `theuth worker` under Landlock (Linux) or Seatbelt (macOS), §5 | filesystem, network on/off | nothing to install |
| `docker` | `docker run -i --rm -v <root>/<name>:/work <image> theuth worker` | namespaces + cgroups | CPU/memory caps, per-user toolchain image |
| `k8s` | pod per user running the image, control plane dials its service | same as docker | scheduling across nodes; not a sandbox, a scheduler for the docker option |

Docker and Kubernetes become a plugin only if lifecycle (pool, warm start,
idle reap across nodes) grows past a launcher function — and then it is one
plugin with a launcher enum, not two.

## 5. Host sandbox: Landlock and Seatbelt, both, in Rust

The interesting one, and the reason this doc exists. Both are unprivileged
(no root, no setuid helper, no daemon), both apply to the whole process
tree, both are what the shipping coding agents use. Applied by the worker to
**itself** before it does anything (`landlock::restrict_self` /
`sandbox_init`), so the launcher is just `theuth worker --user <name>` on
either OS and the control plane has one code path.

### 5.1 Linux: Landlock (+ seccomp)

- Crate `landlock` 0.4.x (maintained; 0.4.7 2026-07). ABI is negotiated at
  runtime: v1 (5.13) filesystem, v4 (6.7) TCP bind/connect ports, v5 (6.10)
  ioctl on devices, v6 (6.12) scoped signals and abstract unix sockets.
  `BestEffort` compatibility degrades on older kernels; the worker logs the
  ABI it got and refuses to start below a configured floor (`landlock_min_abi`).
- Ruleset for a worker: read+execute on `/usr`, `/lib*`, `/etc` (selected),
  the toolchains the operator lists; read+write on `<root>/<name>` and
  `/tmp/theuth-<name>`; nothing else readable — in particular not
  `<data_dir>` (credentials, token, other users). TCP connect allowed only
  to ports the operator lists (443, 22 typically) or none.
- Landlock cannot filter by domain and cannot restrict UDP. If per-domain
  allowlists matter, the answer is the proxy pattern (§5.3), not more
  Landlock.
- seccomp is the belt to Landlock's braces: deny `ptrace`, `mount`, module
  and keyctl families. Crate `extrasafe` or a hand-built filter via
  `seccompiler`; small, static.

### 5.2 macOS: Seatbelt (`sandbox_init` / `sandbox-exec`)

- Marked deprecated since 10.x, still present on every macOS and still what
  Claude Code and Codex ship. Profile language (SBPL) is s-expressions:
  `(version 1) (deny default) (allow process-exec ...) (allow file-read*
  (subpath "/usr")) (allow file-write* (subpath "<root>/<name>")) (deny
  network*)`. Generated per user from the same allow lists as §5.1.
- Apply from inside the worker via `sandbox_init(3)` (libc FFI, ~20 lines)
  rather than wrapping with `/usr/bin/sandbox-exec`, so both OSes share the
  "worker restricts itself" model. Fallback to `sandbox-exec -p <profile>`
  if the FFI proves brittle across releases.
- Network is allow-to-port or deny; per-domain again needs §5.3.
- No resource limits. Fine for "a few people on my Mac"; not a tenant story.
  Real tenants run on Linux.

### 5.3 Network filtering by domain (later)

Both primitives are all-or-nothing on egress. Domain allowlists mean a
host-side HTTP/SOCKS proxy that the sandbox is *only* allowed to reach
(Linux: proxy on a Unix socket bound into the sandbox, netns dropped; macOS:
allow-to-localhost-port). This is exactly what Anthropic's
`sandbox-runtime` (`srt`, Apache-2.0, Seatbelt + bubblewrap + proxies)
packages. Options when the need is real: (a) launch the worker under `srt`
instead of self-restricting — one argv prefix, a Node dependency like the
MCP servers already are; (b) port the proxy idea. Not v0.

### 5.4 Alternatives considered

- **bubblewrap** — mature, what Flatpak and `srt` use; needs user
  namespaces enabled and a binary on the box; process-external (wrapper)
  rather than self-applied. Second choice on Linux if a kernel lacks
  Landlock v4.
- **nsjail / firejail / minijail** — same primitives, more surface, less
  reason.
- **`birdcage`** (Rust, Landlock+seccomp+Seatbelt in one crate) — exactly
  the shape we want; last release 2024-04, vendor acquired. Read it, do not
  depend on it.
- **gVisor / Firecracker** — VM-class isolation; if that is needed, that is
  the `docker` executor with a different runtime, not a host option.
- **uid per user + `systemd-run` properties** — the boring server answer;
  composes with Landlock (uid for ownership, Landlock for reach). Likely both
  on a Linux server.

## 6. Storage

sqlite WAL is fine for a handful of users on one box. Postgres when the
control plane needs backups, more than one machine, or ops tooling — not for
isolation. Cost: `Store` is 24 methods on a concrete `SqlitePool`, plus the
knowledge index (FTS5 → `tsvector`), plus two migration dialects; sqlx makes
it tractable, not free. Redis: only when there are two control-plane
processes and events must fan out between them; the in-process bus stays.

## 7. Frontend

A web app over the existing ws RPC and TS bindings, with its own accounts,
holding a per-user principal. LibreChat itself is not reusable: it speaks
OpenAI chat-completions to backends and has no tool-call/approval surface.
The same web build is the Tauri phase's UI with a login page in front — one
frontend, two shells.

## 8. Open questions

1. Principal source of truth: `users.yaml` in theuth, or JWT from the frontend
   with theuth verifying? (Leaning JWT: theuth stays out of the account business.)
2. Per-user knowledge vs a shared team bundle — or both, with the shared one
   read-only. Doc 10's harvest instruction assumes one user.
3. Subagents across users: never (parent/child stay within one owner) — is
   there any case for a shared subagent?
4. What does `/sessions` show an admin role? Everything, or nothing plus an
   audit query?
5. Worker death mid-turn: fail the tool call (model recovers) vs fail the
   turn. Leaning tool call, mirroring 14's job semantics.
6. macOS `sandbox_init` FFI stability across releases; measure before
   committing to self-restriction over `sandbox-exec`.

## 9. Preconditions (why shelved)

- A named second user who would use it weekly. Without one this is the 00
  §13 list (meetings, slack, voice): infrastructure nobody exercises daily.
- Owner + auth (§2) done and used first, on sqlite, in one process — it is
  the step every later one needs and the only one that is cheap.
- A Linux box for the worker; macOS host mode is a development convenience.

Order when unshelved: §2 → §3 with the `host` executor (Landlock, Seatbelt)
→ `docker` when someone needs resource caps → §6 when ops demand → §5.3
when a domain allowlist is asked for by name. Each step leaves single-user
`theuth` unchanged: the server configuration is a mode, not a fork.
