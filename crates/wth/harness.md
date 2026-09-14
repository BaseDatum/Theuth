---
name: harness
description: How wth (this harness) is laid out and configured - data dir, config.yaml keys, credentials, MCP files and gates, skills, knowledge, sessions, logs, what only the user can do. Load before touching any of them.
---

<!-- Written by wth on every start from crates/wth/harness.md; edits here are overwritten. -->

# wth: operating your own harness

One process, one data dir. The data dir is `~/.wth` unless `--data-dir` /
`WTH_DATA_DIR` moved it; the `agent` block of your system prompt names the
real path. Everything below is relative to it.

## Layout

| path | what | live? |
|---|---|---|
| `config.yaml` | every plugin's section (below) | read at startup |
| `credentials.yaml` | `provider: key`, one per line, mode 0600; beats the environment | read at startup |
| `providers.yaml` | named OpenAI-compatible gateways: `name: {base_url, key?}`; `/provider add` writes it | startup |
| `models.json` | optional; same shape as the bundled models.dev catalog, adds or overrides models | startup |
| `prompt.md` | standing instructions, prepended to every model call | every call |
| `skills/` | markdown skills (`*.md` or `<dir>/SKILL.md`); `add_skill_repo` clones under it | every call (index) |
| `mcp/*.json` | MCP servers, one or many per file, any `mcp.json` shape | ~1 s (watched) |
| `mcp/config.yaml` | risk gates per server / tool (below) | ~1 s (watched) |
| `mcp/repos/` | `add_mcp_repo` clones; `cwd: "repos/<name>"` in an entry runs from one | - |
| `oauth/<ns>.json`, `oauth/mcp/<server>.json` | login tokens (providers, MCP servers); 0600 | - |
| `acp/agents.yaml` | external harnesses runnable as subagents (`spawn_subagent{agent}`) | startup |
| `knowledge/` | the knowledge bundle (markdown, git history); `knowledge.sqlite` is a derived index | live |
| `sessions.sqlite` | every session's history; `/sessions`, `/resume <id>` | live |
| `attachments/`, `spill/` | pasted images; tool output that exceeded the cap (`read_file` the path a result names) | - |
| `logs/wth.log.<YYYY-MM-DD>` | the process log; `RUST_LOG=debug` for more | live |
| `token` | bearer for the JSON-RPC server | startup |
| `signal/` | Signal linked-device state and `inbox/` for files the user texts | - |

## config.yaml

Env overrides any key: `WTH_<SECTION>__<KEY>` (`WTH_RPC__LISTEN=127.0.0.1:7799`).

```yaml
agent:
  model: anthropic/claude-sonnet-4-5   # provider/model, or bare id resolved against configured providers
  effort: null                         # low | high | off | token budget; provider default when unset
  max_tool_rounds: 32
  max_tool_output_chars: 32000
  compact_at: 0.8                      # auto-compact at this fraction of the context window; 0 = off
  compact_style: summary               # summary | cut | snap
  subagents: {model: null, effort: null, max_depth: 1, max_concurrent: 8, max_runtime_secs: 600, reply_timeout_secs: 120}
  knowledge: {hint_limit: 5, hint_rounds: 4, harvest_every: 3, harvest_max: 8}
tools:
  policy: {read_only: allow, mutating: ask, destructive: ask}   # allow | ask | deny per risk
  approvals: true                      # false: never prompt (`/approvals off always` writes this)
rpc: {listen: 127.0.0.1:7777}
web: {enabled: true, engines: [duckduckgo, wikipedia, arxiv, github], fetch_max_chars: 4000}
search_providers: {firecrawl: {base_url: https://api.firecrawl.dev}}   # key in credentials.yaml under the same name
knowledge: {enabled: true}
signal: {enabled: true, device_name: wth, owner: "+1555…", allowed: []}
```

Only `agent.model` is required. Changing this file needs a restart, which
only the user can do; say so instead of pretending it took effect.

## MCP

A file is either one bare entry (named by the file stem) or an object of
named entries, optionally under one wrapper key (`mcpServers`, `servers`, …)
- the shapes README files use. Keys: `command` + `args` + `env` + `cwd` (stdio),
or `url` + `headers` (streamable HTTP; `type` is ignored); `enabled: false`
or `disabled: true` skips it. `$VAR`, `${VAR:-x}`, `$(cmd)` and backticks in
any of those strings expand through the user's shell when the server
connects, so `"GITHUB_TOKEN": "$(gh auth token)"` is fine and nothing is
stored in the file.

Remote servers that answer 401 need the user to log in (`l` on the row in
`/mcp`, or `/login <server>`); you cannot. An optional `oauth` object tunes
that login: `client_id` (+ `client_secret`) for servers without dynamic
registration, `scopes`, and `redirect_uri` (`http://localhost:8765/callback`)
for servers that only accept a pre-registered loopback URL - the port in it
is what gets listened on.

Tools register under their remote name. When two servers offer the same name
(four `grafana-*` entries), or a native tool has it, each registers as
`<server>__<name>` instead; the prefix drops when the clash does.

`mcp/config.yaml` overrides a tool's risk (default: the server's own
annotations; unset = `mutating`):

```yaml
servers:
  kubernetes:
    risk: read_only          # every tool of the server, unless a tool line says otherwise
    tools:
      pods_delete: destructive
      helm_uninstall: off    # off = not registered at all
  teams:
    risk: off                # server not even started
```

Values: `read_only | mutating | destructive | off`. The user's `/mcp` menu
writes this same file; hand edits are picked up within a second. Per-session
hiding (`/mcp` ⏎ → `s`) is in memory only and not something you can set.

## Approvals

Every tool call has a risk. `tools.policy` maps it to `allow` (runs), `ask`
(the user sees a prompt; the call waits) or `deny` (`denied by policy`, no
retry will help). You cannot approve or bypass; if a denied call is
essential, tell the user which tool and why.

## Skills

A skill is a markdown file with optional front matter (`name`,
`description`, `always: true` to inline it every call). The index in your
system prompt lists them; `load_skill` fetches a body; `add_skill` /
`remove_skill` write local ones; `add_skill_repo` / `sync_skills` manage
cloned repos (read-only). This file is one; it is rewritten on every start.

## Sessions, subagents, jobs, knowledge

- History lives in `sessions.sqlite`; compaction folds it per
  `agent.compact_style`; `/compact` does it on demand.
- `spawn_subagent{task, name?, agent?, cwd?, model?, effort?}` returns
  immediately; the result comes back as a `<subagent_result>` message when the
  child finishes. Children see only `task`. `agent: <name>` runs an ACP
  harness from `acp/agents.yaml` in `cwd` (it picks its own model); without
  it a wth subagent runs on `model`/`effort` you choose (`list_models` shows
  the ids; unset = `agent.subagents.model`/`effort`, then `agent.model`).
  `send_message{to, message, await_reply}` talks to `parent` or a child by
  name.
- The shell tool returns within a few seconds or hands back a job id;
  `<job_result>` arrives when it ends. `jobs` lists, `cancel_job` kills.
- `start{command, name?, ready?}` runs on a pseudo-terminal; `send`/`screen`/
  `wait` drive it; use it for REPLs, servers, TUIs and prompts — `shell` for
  anything that runs and exits (22).
- `browser_*` (23) when a page needs JavaScript, a form, clicking through,
  or a visual check; `read_url` for plain pages. `browser_open{url}` returns
  the readable text and element refs (`ref=eN`); refs go stale after
  navigation or interaction, so call `browser_read` / `browser_elements`
  again before reusing one. Submit forms by clicking the submit button
  (`browser_click`); Enter in a field does not submit. `browser_screenshot`
  returns an image you can look at. `localhost` and private addresses are
  blocked unless `browser.allow_private_network` is set in config.
- Durable facts go through `knowledge_put`; `<knowledge>` notes in your
  context are hints from that store. The bundle is plain markdown under
  `knowledge/` with git history (`/knowledge` in the UI).

## Only the user can

Restart the process, log in (providers or MCP), approve a call, switch the
running session's model (`/model`), pair Signal, edit `credentials.yaml`
safely (you may, but say what you wrote). When a task needs one of these,
stop and ask.
