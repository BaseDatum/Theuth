# Where wth already fails its original design

Checked against `fa50c31` (`docs/design/00-architecture.md` as written on
phase 0), not the later docs that were rewritten to match the code. Current
`00-architecture.md` still asserts several of these. The constitution wasn’t
amended; it was outrun.

What still holds, so the rest isn’t noise: `runtime` is 225 lines (ceiling
400). There is no shared `Event` enum. The bus is still typed kameo.
Approvals still go through one actor. Plugins still implement `Plugin`. One
process.

---

## 1.7 — “That is the whole permission system for v0”

Still in 00 line 162. Original system: `Risk × {allow,ask,deny}`. Timeout →
deny. Stop.

Now, in one `Invoke` (`crates/tools/src/registry.rs`):

```rust
if action == PolicyAction::Ask && (!self.approvals || self.bypassed(call.session)) {
    action = PolicyAction::Allow;
}
```

That’s four extra axes on the same gate:

| axis | what |
|---|---|
| `ErasedTool::gate()` | MCP tools skip `tools.policy` entirely. Second policy, per-server/per-tool, in `mcp/config.yaml` |
| `tools.approvals` | global kill switch, persisted |
| `bypass: HashSet<SessionId>` | per-session kill switch |
| `parents: HashMap` + `SetParent` | lineage walk so children inherit the parent’s switch |

00 §1.7’s example of what not to do was “jarvis: tiers, approvals, kill
switch, safe mode grew ad hoc.” This is a kill switch, a session switch,
inheritance, and a parallel MCP policy. Same shape, nicer names.

---

## 1.6 + §13 — crates and the “not planned” list

Original workspace: six crates. Current 00 still says “Six crates for v0”
(line 387) and still says signal/okf are “not planned. They return as
plugins only when needed, and only as plugins” (line 502).

| crate | plugin? | in any original phase? |
|---|---|---|
| oauth, models, markdown | no | arguably earned |
| **signal** (5.4k) | yes | **explicitly not planned** |
| **cursor** (1.5k) | **no** | not in any phase |
| knowledge/ | **no — module inside agent** | **okf was on the same not-planned list** |

Signal at least came back as a plugin, which is the half of §13 they kept.
Knowledge did not. It’s `agent/src/knowledge/` (harvest inside the loop,
tools registered onto the registry from `supervisor::start`). That is the
jarvis pattern §13 existed to forbid: a subsystem growing inside the agent
instead of earning a plugin.

Cursor is worse on the crate rule: not a plugin, not a phase, and it punches
a second LLM client through the hot path.

Crate sizes at time of writing:

| lines | crate |
|---|---|
| 225 | runtime |
| 388 | wth |
| 393 | models |
| 685 | markdown |
| 815 | rpc |
| 942 | oauth |
| 1481 | cursor |
| 3318 | tools |
| 3484 | tui |
| 5453 | signal |
| 9875 | agent |

---

## 1.4 — “main.rs is, in full: build Runtime, start plugins, wait”

`crates/wth/src/main.rs` is 267 lines plus a 121-line `worker.rs`. Special
cases that are not a plugin list:

- `#[global_allocator]` and `wth subprocess` before clap (monty)
- `first_run` writing the data-dir skeleton
- `--link-signal` talking to `signal::link_oneshot` / `signal::qr` /
  `signal::e164` and `write_config_value` **directly**, then `process::exit`
- `process::exit(0)` after a 100ms sleep because a stuck `spawn_blocking`
  drain pins the runtime

The spirit was “don’t thread handles between subsystems.” They mostly don’t.
`link_signal` is the actual god-wiring: the binary knows a plugin’s pairing
protocol.

---

## §4 — “looks it up by name at first use, never at start”

`crates/signal/src/lib.rs:319`:

```rust
// tools starts before us (main.rs order), so one lookup is enough.
```

That’s start-order coupling with a comment admitting it.

Agent does the other half: `supervisor::start` spawns a task that retries
the registry lookup 50 times to register skills/knowledge/spawn/send_message.
First-use would have been `Invoke` time, which is what the approval actor
actually does (`approval` looked up on first `Ask`). Two plugins, two
policies.

---

## §5 / §1.2 — one LLM library, glue only

Original: “Library: genai.” Rejected writing a client.

```rust
// crates/agent/src/loop_.rs
if ns(&model.genai) == "cursor" {
    let t = self.shared.auth.oauth.access(&oauth::CURSOR).await?;
    cursor::chat_stream(&t.access_token, slug, req, &self.shared.id.to_string()).await
```

A handwritten Connect-RPC/protobuf HTTP/2 client, plus a `cursor/` namespace
that “never reaches genai” (`frame.rs`). 00 rejected `tonic` for the RPC
plugin; then they wrote the worse version of that client for a provider. No
written reason in 00. (05 has the how, not the why-not-a-library.)

---

## §8 — “TUI is an ordinary bus subscriber”

`tui/src/model.rs` is 2283 lines. `Command` has 31 variants (`Send` through
`Logout`). `UiEvent` has 27. The TUI imports `AgentConfig`, `models::Catalog`,
oauth login rows, knowledge types, markdown. It is a control plane.

Original surface was `UserInput` / `Steer` / `Resolve`. Tauri-over-RPC is not
the same path; RPC had to grow a matching facade (`rpc/src/api.rs`, 93
`async fn`s, depends on agent + tools + signal + knowledge) so a second UI
could exist at all. That’s the “protocol dumping ground” §1.5 named, just
split across `tui::Command` and `rpc::Api` instead of one `Event`.

---

## §9 — “No hot reload in v0”

MCP directory is `notify::recommended_watcher` (`mcp.rs`). Skills and
knowledge resync on turn start. The constraint is still in the original 00
and was never struck.

---

## §4 / §6 — every invocation through the registry; MCP names `<server>__<tool>`

`python` and `todo` are loop-local (`loop_.rs`: “Registry batch first, then
the loop-local calls”). They never hit `Invoke`, so they never hit the
approval policy. Inner python→shell calls do, which is why it mostly works;
the original rule still says every tool.

MCP prefix: 00 §6 required `<server>__<tool>`. 01 later *rejected* it. That’s
an amendment, not a silent miss — unlike the items above, someone wrote the
decision down.

---

## §1.8 — “Not a coding agent… not for its tool set”

Squishier, because original §6 already specified shell + fs + MCP. They did
not copy oh-my-pi’s grep/edit/notebook. What they added beyond that contract:
a persistent Python REPL, knowledge harvest, skill/MCP git forges, and
**Cursor’s coding-agent backend**. The last one is the non-negotiable failing
on purpose. Turns route through `api2.cursor.sh` `AgentService`. That’s a
coding agent with extra steps.

---

## Agent is the new `types.rs`

00 capped the *shared* crate and kept that cap. Everything else landed in
`agent/` (9875 lines, 17 files): loop, repl, compact, spill, frame, skills,
peers, todo, knowledge, supervisor. `agent/src/lib.rs` is 662 lines of
command structs. Not jarvis’s 129 KB `types.rs`. The same thing as a crate
named `agent`.

---

## Deliberate rewrites, not failures

yaml vs toml, detached vs blocking subagents, MCP without prefix,
Tool/Approval moved *out* of `runtime` into `tools` (that one is more
faithful to “owning plugin” than the original §4 table). Those changed the
requirement.

---

## The pattern

1.7, 1.6, §13, and §8 are still on the page in 00 and already false in the
tree. The docs that grew (01, 05, 08, 09, 10) describe what was built; the
constitution still describes what was promised not to. That’s how jarvis
started too — the postmortem stayed true while the tree moved.

---

## Earlier notes from using it (2026-09-10)

The cache invariant (07) is the actual product and is being kept. Python
persistence, tools-as-coroutines, heap inventory, harvest-on-cached-frame
are real. The risk is velocity plus one more plugin before the loop’s error
paths are boring.

Known gaps the design already listed and did not close: session actors never
evicted, unbounded mailboxes, `setsid` children escape `killpg`, cancel/error
paths in the loop code-reviewed not smoke-tested, RPC token compare is plain
`==`.

Monty tax: no generators, no inheritance, `gather` errors uncatchable inside
the coroutine. Documented, keeps biting models.

TUI: eight-screen tail, no wheel scroll (mouse capture off for native
select). Will hurt on a long debug session.

`python` and `todo` being loop-local plus native-direct plus runtime-function
is three ways to call something.
