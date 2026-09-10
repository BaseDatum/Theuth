# 04 — System prompt, skill sources, MCP sources

Status: **shipped** (2026-09-09).

Deviations from the plan, as built: `add_mcp_server` does not poll — the
`Mcp` actor awaits connect inline under a 10 s `CONNECT_TIMEOUT` that now
applies to every server (a slower one fails until the next reconcile); a
failed add deletes the file it just wrote so a retry needs no
`remove_mcp_server`. `Command::AddMcp` in the TUI carries a required name
(resolved from the JSON by `model::mcp_entry`). `/prompt` renders the text
verbatim in a code fence. `RestartMcp` has no TUI/RPC surface — `sync_mcp`
restarts affected servers actor-side. Skills tests clone from a local bare
repo; no network in the suite.

Goal: the agent can grow its own capabilities without the user editing
files: write a skill, clone a skill repo, keep repos synced, install an MCP
server (package or git repo), and the system prompt reflects all of it on
the next round. The human gets the same operations as slash commands and
RPC methods, and one file for standing instructions.

Everything below reuses the two mechanisms that already exist:

- **a directory is the registry** (`skills/`, `mcp/`); tools write files,
  nothing else holds state;
- **the agent reads the directory when it needs it** (MCP already
  reconciles from the dir; skills move from load-once to load-per-use).

## 1. System prompt

### 1.1 Assembly (`agent::frame::baseline`, per turn; see `07-context.md` §4)

```
1. built-in header      identity line, working directory, date
2. <data_dir>/prompt.md standing instructions, verbatim (missing → nothing)
3. always-on skills     bodies of skills with `always: true`, in name order
4. skills index         "## Skills" list, one line each (unchanged shape)
5. peers                child identity line (07 §10); static spawn/send + todo guidance
6. runtime              `## Runtime` tool index when the Python runtime is on (08 §3.2)
```

Each part is separated by a blank line; empty parts are skipped. The
prompt is rebuilt from disk once per **turn** (`Turn::rebase`), not per
round: editing `prompt.md` or adding a skill applies on the next turn — no
watcher, no cache invalidation beyond the one deliberate rebase miss
(`07 §4`). Reason: a multi-round turn is where the provider prefix cache
matters most, and a turn is seconds to minutes long; the model can still
use a skill it just wrote via `load_skill` mid-turn. Nothing custom to
explain (rule §1.2 of 00).

`agent.system_prompt` in `config.yaml` is **removed** (clean cutover): the
file is the one place. The first-run template gains a `prompt.md` with a
comment header explaining what it is; `AgentConfig::system_prompt` and its
test fixtures go.

`/prompt` (TUI) and `get_prompt` (RPC) return the assembled text for the
shown session so the user can see exactly what the model sees. Agent-side
message: `ShowPrompt { session } → String`.

### 1.2 Skill format (parse)

Today: any `*.md` under `skills/` is a skill. That breaks the moment a git
repo is cloned in (README.md, CHANGELOG.md become skills). New rules,
compatible with the de-facto agent-skills layout (`<name>/SKILL.md` plus
supporting files, front matter `name`/`description`):

```
walk <data_dir>/skills/ recursively, skipping `.git/`
  a directory containing SKILL.md         → one skill; its subtree is not walked further
  a loose *.md elsewhere (not README/LICENSE/CHANGELOG/CONTRIBUTING, any case)
                                          → one skill (today's behaviour)
```

Front matter (`gray_matter`, as now):

| key           | default                              | new |
|---------------|--------------------------------------|-----|
| `name`        | file stem / directory name           |     |
| `description` | first non-empty body line            |     |
| `always`      | `false`                              | yes — body inlined into the system prompt instead of indexed |

Unknown keys (`license`, `allowed-tools`, `metadata`, …) are ignored.
Duplicate names: last by sorted path wins, warning as now.

`Skills::load(dir)` becomes `Skills::index(dir) -> Vec<SkillMeta {name,
description, path, always, source}>` reading only front matter (the first
4 KiB of each file); `Skills::body(&meta)` reads the file on demand.
`load_skill` returns the body and, for `SKILL.md` skills, the relative
paths of the other files in its directory so the model can `read_file`
them. `source` is `Local` or `Repo(dir)` (the nearest ancestor with `.git`),
used for display and for `sync`.

## 2. Skill sources — the agent grows its own registry

### 2.1 Layout

```
~/.wth/skills/
  commit.md                      loose skill (human or add_skill)
  review/SKILL.md                directory skill
  anthropics-skills/             git clone (add_skill_repo / `/skills add`)
    .git/
    skills/pdf/SKILL.md          found by the walk, source = Repo
    skills/xlsx/SKILL.md
    README.md                    ignored
```

No manifest. A repo is a subdirectory with `.git`; its URL is
`git remote get-url origin`. Reason: the clone *is* the record; a manifest
would be a second source of truth to keep in step with `rm -rf`.

### 2.2 Tools (agent crate, registered like `load_skill`)

| tool | args | risk | does |
|---|---|---|---|
| `add_skill` | `name`, `description`, `body`, `always?` | Mutating | writes `skills/<name>.md` with front matter; refuses to overwrite a `Repo` skill (edit upstream instead) |
| `remove_skill` | `name` | Destructive | deletes a `Local` skill file/dir; refuses `Repo` skills (say `remove_skill_repo`) |
| `add_skill_repo` | `url`, `name?` | Mutating | `git clone --depth 1 <url> skills/<name>` (name defaults to the repo basename); returns the skills found |
| `remove_skill_repo` | `name` | Destructive | `rm -rf skills/<name>` if it has `.git` |
| `sync_skills` | — | Mutating | `git pull --ff-only` in every repo; returns per-repo `updated`/`unchanged`/`error` |

`name` is validated `[a-z0-9][a-z0-9._-]*` and must not escape the dir
(no `/`, no `..`). The index is rebuilt at the next turn start (§1.1) and
`load_skill` reads the directory live, so every tool's output can simply
say "available now".

Git runs as the user's `git` binary via `tokio::process::Command`, in
`tools::git` (`clone`, `pull`, `remote_url`, `repos(dir)`), shared with §3.
Reason (rule §1.2 of 00): `git2`/`gix` add a large dependency and their
own credential/ssh story; the user's git already has keys, config and
proxies set up. Ceiling: no git on PATH → the repo tools report that and
everything else works.

### 2.3 Human surface

- `/skills` — picker: `name  description  [repo-name]`, Enter shows the
  body as a card (peer card shape, `◇ name` header, collapsed, Ctrl+O).
- `/skills add <url>`, `/skills rm <name>`, `/skills sync` — the tools
  above without the model in the loop.
- RPC: `list_skills → Vec<SkillMeta>`, `load_skill`, `add_skill_repo`,
  `remove_skill`, `remove_skill_repo`, `sync_skills`. Same actor messages
  the tools use (`agent::skills::{ListSkills, …}` on the `"agent"` actor).

## 3. MCP sources — packages and repos

Today `mcp/*.json` is already a hot-reloaded registry (01 §1.4). The agent
only needs to write files; the watcher does the rest. Two additions to the
loader, both shapes already exist in the wild:

- entry field `cwd` (VS Code / Cursor accept it): relative paths resolve
  against `mcp/`;
- `Mcp` actor message `Restart(name)`: drop + reconnect one server even if
  its entry is unchanged (needed after a repo pull rebuilt the binary).

### 3.1 Layout

```
~/.wth/mcp/
  config.yaml                    gates (unchanged)
  everything.json                package server (npx …)
  my-server.json                 { "command": "node", "args": ["dist/index.js"], "cwd": "repos/my-server" }
  repos/                         git clones; not scanned for *.json
    my-server/.git
```

`repos/` is skipped by the json scan (the scan is non-recursive already;
this pins it).

### 3.2 Tools (tools crate, next to the `Mcp` actor)

| tool | args | risk | does |
|---|---|---|---|
| `add_mcp_server` | `name`, then either `command`+`args?`+`env?`+`cwd?` or `url`+`headers?` | **Destructive** | writes `mcp/<name>.json`, asks `Mcp` to `Reconcile`, waits ≤10 s for the server, returns `connected` + tool names or the connect error |
| `remove_mcp_server` | `name` | Destructive | the file whose stem is `name` if it holds only that server; otherwise sets `"disabled": true` on that entry in its file |
| `add_mcp_repo` | `url`, `name?` | Mutating | `git clone --depth 1` into `mcp/repos/<name>`; returns the path and the repo's README head (first 60 lines) so the model can plan the build |
| `sync_mcp` | — | Mutating | `git pull --ff-only` every repo; for each updated repo, `Restart` every server whose `cwd` is inside it |

`add_mcp_server` is Destructive because it makes the harness spawn a
model-chosen executable on every start; that is a shell command by another
name. Building a cloned repo (`npm install`, `cargo build`, …) is not a
tool of its own: the model does it through the shell tool under the normal
approval policy, then calls `add_mcp_server` with `cwd`. Reason: build
steps vary per repo; one more layer would just be a worse shell.

Gates are **not** exposed to the model. `mcp/config.yaml` is the human's
trust boundary; only `/mcp` and RPC `set_mcp_gate` write it.

### 3.3 Human surface

- `/mcp` menu gains a `sync` action (key `s`) and shows `⟳ repo` on
  servers with a `cwd` inside `repos/`.
- `/mcp add <url|json>`: a URL ending in `.git` or on a git host → repo
  clone; anything else is parsed as one server entry (the same structural
  detection as files) and written as `<name>.json`.
- RPC: `add_mcp_server`, `remove_mcp_server`, `add_mcp_repo`, `sync_mcp`
  → actor messages on `"mcp"`.

## 4. Approvals and safety

- All write tools carry `Risk::Mutating`/`Destructive` and go through the
  existing policy (`tools.policy`, `/approvals`). Nothing new.
- Approval modals for these tools show the described args like any other
  (`describe` already handles `key: value`); for `add_mcp_server` the
  command line is joined so the human reads `npx -y @scope/server` in one
  line.
- Skills are text the model reads; a malicious skill is a prompt-injection
  vector. Mitigation is the same as for any file the model reads: the user
  chose the repo. `always: true` from a `Repo` skill is honoured but the
  `/skills` picker marks it `always` so it is visible.

## 5. Implementation order

1. **Prompt** (agent): `prompt.md`, drop `agent.system_prompt`, per-round
   rebuild, `always`, `SKILL.md` walk + ignore list, `Skills::index/body`,
   `load_skill` file listing, `/prompt` + `get_prompt`. Tests: walk rules
   (dir skill wins over its subtree; README ignored; loose md still a
   skill), `always` inlined and not indexed.
2. **Skill tools** (agent + tools::git): the five tools, `/skills …`, RPC.
   Test: `add_skill` then the next round's prompt lists it (throwaway
   e2e); `add_skill_repo` against a local bare repo in a temp dir
   (deterministic, no network) → skills indexed with `source = Repo`;
   `sync_skills` after a commit to the bare repo → `updated`.
3. **MCP tools** (tools): `cwd`, `Restart`, the four tools, `/mcp add|sync`,
   RPC. Test: `add_mcp_server` with the `everything` package → connected
   with tool names; `remove_mcp_server` on a multi-server file →
   `disabled: true` written, other servers untouched.
4. **Docs**: `docs/skills.md` (format, repos, `always`), README config
   list (`prompt.md`, `skills/` repos, `mcp/repos/`), 01 §1.4 (`cwd`,
   `Restart`), bindings.

## 6. Rejected

- `git2`/`gix` — see §2.2.
- A sources manifest (`skills/sources.yaml`) — the clones are the record.
- Skill "marketplaces"/search — the model can `add_skill_repo` any URL the
  user names; discovery is a web search away and not the harness's job.
- Watcher + cache for skills — per-round rescan is cheaper than the code;
  revisit past ~1000 skill files.
- Letting the model set MCP gates — §3.2.
- A build-step tool for MCP repos — the shell tool under approvals is
  exactly that.
- Keeping `agent.system_prompt` alongside `prompt.md` — two places for one
  thing.
