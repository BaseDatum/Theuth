# wth

A single-process AI harness that does things for the user. Plugins are Rust
crates talking over a typed message bus; any frontend attaches over JSON-RPC.
Design: [`docs/design/00-architecture.md`](docs/design/00-architecture.md).
Adding a plugin: [`docs/plugins.md`](docs/plugins.md).

Install without building: `brew install skymoore/tap/wth` (the CLI, macOS
arm64 and Linux) or `brew install --cask skymoore/tap/wth` (the macOS app).
Both keep themselves current ([`docs/design/39-update.md`](docs/design/39-update.md));
`wth --update` forces a check. Building from source:

```sh
just setup                 # rustup toolchain; `just toolchain` prints its bin dir
export ANTHROPIC_API_KEY=… # any genai provider works; default model in config   # or: wth --secret-set anthropic
just run                   # terminal UI; data dir is ~/.wth (--data-dir / WTH_DATA_DIR relocate it)
just run -- --headless     # JSON-RPC only, ws://127.0.0.1:7777 with <data_dir>/token
just run -- --resume <id>  # reopen a session
just run -- -p "prompt"    # print mode: stream the reply, exit when the turn ends
echo "prompt" | just run -- -p   # same, prompt from stdin
just run -- --link-signal --owner +15551234567   # pair as a Signal linked device (one-shot QR), then run --headless
just run -- --link             # pair a phone with the running daemon (one-shot QR + key); needs `wth --headless` on this box
just run -- --install-service --relay wss://relay.example.com   # on a server: run wth --headless as a launchd/systemd user service through your login shell, print the key to pair it to your main harness as a node
just run -- --uninstall-service                                  # remove that unit; the data dir and every pairing stay
just relay                     # local wth-relay on 127.0.0.1:8787 for link development; `just web-build` builds the browser UI it serves
just run -- --slack-setup      # create the Slack app from the manifest, verify tokens, write config; then run --headless
just run -- --slack-manifest   # manual path: print the Slack app manifest for api.slack.com/apps → From a manifest
just run -- --acp          # Agent Client Protocol on stdio: use wth from Zed/editors (agent_servers: {"wth": {"command": "wth", "args": ["--acp"]}})
just ts                    # TypeScript bindings for the wire types → bindings/
just desktop               # macOS menubar app in dev mode (Tauri + Vite); starts the daemon itself
just package-macos         # wth.app + dmg; ad-hoc unless WTH_SIGN_IDENTITY / .env.signing
just package-ios           # signed IPA for TestFlight (Apple Distribution)
just testflight            # upload last IPA to App Store Connect
just install-macos         # copy the packaged app to /Applications
```

Build prerequisites: `cmake`, `clang`/`libclang` and `llvm-objcopy`/`llvm-nm` for the embedded browser's
BoringSSL (`brew install cmake llvm` / `apt install cmake clang libclang-dev`),
and network on the first build for the prebuilt V8 archive (set
`RUSTY_V8_ARCHIVE` to a local copy to build offline).

macOS permissions: `computer_*` needs Screen Recording and Accessibility
granted to the app that launched wth — your terminal (Ghostty, iTerm,
Terminal) or wth.app — in System Settings › Privacy & Security. Screen
Recording takes effect only after restarting that app; Accessibility is
immediate. `/computer` shows what is granted.

Crates: `runtime` (plugin trait), `models` (bundled models.dev catalog),
`markdown` (streaming markdown: raw tail, blocks rendered once), `oauth`
(loopback callback, PKCE, provider token store), `tools` (registry, approval
gate, shell via brush, fs (read, write, list, grep, edit), MCP; shell commands
run as background jobs: output within 3 s comes back inline, otherwise the
model gets a job id and is woken with `<job_result>` when it exits —
[`docs/design/14-jobs.md`](docs/design/14-jobs.md); `start` / `send` /
`screen` / `wait` run a command on a pseudo-terminal (REPL, server, TUI) and
drive it: type, read the rendered 120×40 screen, wait for a prompt or a port —
[`docs/design/22-pty.md`](docs/design/22-pty.md)), `secrets` (an encrypted,
versioned vault the agent uses by name but never reads: `{{secret:NAME}}` in
any tool argument is swapped for the value at run time and scrubbed from the
output; holds the provider, web and Slack keys too —
[`docs/design/37-secrets.md`](docs/design/37-secrets.md)), `browser` (sixteen
`browser_*` tools: an embedded headless browser (obscura) with screenshots —
`browser_open`, `_read`, `_elements`, `_click`, `_fill`, `_type`, `_press`,
`_select`, `_scroll`, `_wait`, `_eval`, `_back`, `_screenshot`, `_tabs`,
`_close`, `_show` (plus `browser_list_profiles` + `browser_import_cookies` when
`import_cookies` is on: discover local browser profiles and load the user's
real cookies so the agent acts as the signed-in user; `browser_show` opens a
live view of the session's browser in the desktop app); one
isolate per session, screenshots come back as images in the
tool result — [`docs/design/23-browser.md`](docs/design/23-browser.md)), `scrape` (`scrape_url`, `crawl`, `map_site`: a crawler built on the vendored `firecrawl-native` crate, with `web`'s fetcher and obscura as its two engine tiers; `crawl` stays under the seed's directory, honours `robots.txt` and seeds from the site's sitemaps, `map_site` lists a site's URLs from sitemap + links without page bodies — [`docs/design/38-scrape-vendored-crawler.md`](docs/design/38-scrape-vendored-crawler.md)), `computer`
(eight `computer_*` tools: `computer_screenshot`, `_move`, `_click`, `_drag`,
`_scroll`, `_type`, `_key`, `_open` — one composite screenshot of every
display as an image, then native mouse and keyboard on the user's own desktop —
[`docs/design/27-computer.md`](docs/design/27-computer.md)), `knowledge`
(OKF bundle, FTS index, gix history, `knowledge_*` tools —
[`docs/design/10-knowledge.md`](docs/design/10-knowledge.md)), `agent` (genai
loop, sqlite sessions with compaction epochs, skills, subagents, persistent
Python runtime over monty worker subprocesses —
[`docs/design/08-repl.md`](docs/design/08-repl.md); `message_user` sends text
and files to the user on Slack or Signal out of band —
[`docs/design/24-message-user.md`](docs/design/24-message-user.md); `ask_user`
puts one or more multiple-choice questions to the user — pick, type your own,
skip one, or dismiss the set to end the turn — in the TUI, the desktop app, a
Slack thread (Block Kit) or over Signal one question at a time), `web` (`search_web` and
`read_url`: SSRF-guarded fetch, keyless google/bing/brave/qwant/wikipedia/arxiv/github
merged by RRF with SERP features; `search_web` also reads the top hits and returns
the passages relevant to the query in the same call — [`docs/design/16-web.md`](docs/design/16-web.md)),
`search-providers` (optional Firecrawl, SearXNG, OpenSERP, Brave API as `brave_api`; registered with `web`
over the bus), `acp` (external
harnesses over the Agent Client Protocol: agents.yaml, registry, harness
connection), `signal` (phone frontend
over Signal messenger), `slack` (Slack frontend over Socket Mode), `rpc` (jsonrpsee),
`link` (drive the harness from a phone or browser through a relay: Noise
end-to-end encryption, pairing QR, device registry —
[`docs/design/31-link.md`](docs/design/31-link.md)), `relay` (the `wth-relay`
binary: a stateless relay that copies encrypted frames between one harness
and its devices), `schedule` (scheduled runs and loops: one `~/.wth/schedules/<name>.md`
per schedule with `cron:` / `every:` / `at:` front matter and the prompt as
body; each fire is a turn in that schedule's own session, the reply is
delivered to the user; add `until:` (a shell check) or `max_runs:` and it
loops until done, `every: 0s` for back to back, `approvals: false` for
unattended, `schedule_done` when the model decides —
[`docs/design/34-schedule.md`](docs/design/34-schedule.md),
[`docs/design/33-loops.md`](docs/design/33-loops.md)), `artifact`
(the `show` tool: files the agent makes for you, shown as cards in the chat —
markdown, code, CSV, images, SVG, PDFs compiled in-process from markdown or
Typst; a content-addressed store under `~/.wth/artifacts/` —
[`docs/design/35-artifacts.md`](docs/design/35-artifacts.md)), `tui` (iocraft),
`wth` (binary).

Config (`~/.wth/`, created on first run; details in
[`docs/design/01-config-and-subagents.md`](docs/design/01-config-and-subagents.md) §1):

- `config.yaml` — every plugin reads its own section; `WTH_<SECTION>__<KEY>` env overrides. `agent.model` is a models.dev id (`anthropic/claude-sonnet-4-5`); `/model` rewrites that one line in place. The `agent`, `tools`, `secrets`, `knowledge`, `web`, `scrape`, `search_providers`, `browser`, `computer`, `link`, `nodes`, `schedule`, `update`, `artifact`, `voice` and `slack` sections with their defaults:

  ```yaml
  agent:
    model: anthropic/claude-sonnet-4-5
    # effort: high        # low|medium|high|… per model (/model lists them); unset = provider default
    compact_at: 0.8        # compact when the context is this full; 0 disables
    compact_style: summary  # summary | cut | snap
    knowledge:             # how the loop uses the store
      hint_limit: 5        # lines per <knowledge> hint
      hint_rounds: 4       # mid-turn hint cadence in tool rounds
      harvest_every: 3     # tool-using turns per harvest; 0 disables harvest
      harvest_max: 8       # concepts accepted per harvest
  tools:
    job_grace_secs: 3      # inline `shell` reply if the job exits within this; 0 = always a stub
    job_timeout_secs: 1800 # kill the process group after this; result exit="timeout"
    pty_ready_timeout_secs: 30   # `start` readiness wait when `ready` gives no timeout
    ask_timeout_secs: 1800  # `ask_user` gives up (the model sees "no answer") after this
  secrets:                       # design doc 37; `/secrets`, Settings › Secrets, `wth --secret-set`
    key_store: auto              # auto | keychain | file — where the vault key lives; auto = keychain when one answers, else <data_dir>/secrets.key (0600) with a warning
    tools: true                  # false: no secret_* tools, no prompt block, `{{secret:…}}` stays literal, nothing is scrubbed; the vault, /secrets and the harness's own keys are unaffected
    request_timeout_secs: 1800   # `secret_request` gives up (the model sees "no answer") after this
  knowledge:
    enabled: true          # false: no store, no knowledge_* tools, no hints, no harvest
  web:
    engines: [google, bing, brave, qwant, duckduckgo, wikipedia, arxiv, github]   # keyless; search_web / read_url
  scrape:                         # design doc 38; scrape_url / crawl / map_site
    concurrency: 5
    max_pages: 50                 # default limit for crawl / map_site
    max_depth: 3
    same_domain: true
    page_timeout_ms: 8000
    run_timeout_ms: 60000
    robots: true
    delay_ms: 100                 # per-host gap between requests; a larger robots Crawl-delay wins
  search_providers:               # optional external engines registered with `web`; the built-ins above need none of these
    firecrawl: { base_url: https://api.firecrawl.dev }   # present by default; inert until a `firecrawl` secret is in the vault
    # searxng: { base_url: http://127.0.0.1:8080 }     # a SearXNG instance you run
    # openserp: { base_url: http://127.0.0.1:7000 }
    # brave: {}                    # Brave Search API (provider `brave_api`); secret `brave` in the vault
  browser:                        # design doc 23
    enabled: true
    stealth: true                 # wreq TLS fingerprint + tracker blocklist (§0.4)
    proxy: null                   # http://user:pass@host:port | socks5://host:port
    user_agent: null              # default: obscura's current-Chrome UA for the platform
    timezone: null                # IANA zone; null = the host's zone (not obscura's Europe/Berlin)
    geolocation: null             # "lat,lon"
    allow_private_network: false  # §0.8
    import_cookies: false         # let browser_import_cookies load the user's real cookies (§7)
    idle_secs: 300                # isolate teardown after inactivity
    max_chars: 4000               # default cap of browser_read / browser_open replies
    script_deadline_ms: 30000     # OBSCURA_SCRIPT_DEADLINE_MS for heavy SPAs
  computer:                       # design doc 27
    enabled: true                 # the OS permission prompts are the consent; actions still ask (tools.policy)
    max_edge: 1568                # long edge of a screenshot after logical scaling (17 §1; ~1280 for Anthropic click accuracy)
  link:                           # design doc 31; `/link on`, Settings › Link or hand-edit
    enabled: false                # the uplink dials out; nothing listens
    relay: wss://relay.wth.rwx.dev   # self-host: your `wth-relay` URL
    name: ""                      # display name in the QR and on devices; "" → hostname
    max_devices: 16
  nodes:                          # design doc 32; `/nodes add <key>` pairs, the node ran `wth --install-service`
    enabled: true                 # false: no dials, nodes invisible to the model and the UI; nodes.yaml kept
  schedule:                       # design docs 34 + 33; `~/.wth/schedules/*.md` files, `/schedules` lists them
    enabled: true                 # false: no watcher, no actor, no `schedule` tool, no prompt block
    run_timeout_secs: 1800        # a run still going after this is cancelled and reported as `timeout`
  update:                         # design doc 39; self-update, silent: verify the signature, replace the binary on disk, live at the next start
    enabled: true                 # false: never checks; `wth --update` still works
    url: https://s3.rwx.dev/wth/releases/latest.json   # the release manifest; point at a scratch bucket to test
  artifact:                       # design doc 35; the `show` tool, `/artifacts` lists a session's
    enabled: true                 # false: no `show` tool, no prompt block, no /artifacts; blobs and index kept
    fonts: []                     # extra directories searched for PDF fonts, besides system fonts and the embedded set
  voice:                          # design doc 43; the desktop's Read-aloud button: Pocket TTS in the harness, Opus to the frontend
    enabled: true                 # false: no actor, `speak` answers "not running", the desktop shows no button
    voice: peter_yearsley         # a `voices/<name>.safetensors` in the checkpoint directory
    # model_dir: ~/.wth/models/pocket-tts   # the Pocket TTS checkpoint (weights, tokenizer.model, voices/); this is the default
  slack:
    # owner: U0123456789           # owner-only commands, may click any approval, receives unbound approvals
    allowed: []                    # more user ids; each owns their threads
    channels: []                   # channel ids where @wth is honoured; empty = DMs only
    tools: all                     # all | errors | none — task cards per tool call
    reasoning: true                # the quoted reasoning message
    approvals: all                 # all | threads — where approvals for non-Slack sessions go
  ```

  The Python runtime is always on: the worker is the `wth` binary itself (`wth subprocess`, nothing to install); every tool is an `async` function inside the sandbox and state persists across turns.
- `secrets.vault` — encrypted (XChaCha20-Poly1305) vault of named, versioned secrets; the key lives in the OS keychain (or `secrets.key`, mode 0600, where there is none). Provider keys (genai adapter names: `anthropic`, `openai`, …) beat env vars; a provider with a key, a login, or its env var is "configured": `/model` lists only those. Named OpenAI-compatible gateways (`/provider add`) store their key here under the gateway name; web search keys (`firecrawl`, `brave`, optional `github`) and the Slack tokens (`slack_bot`, `slack_app`) live here too. Add, replace, restore or delete with `/secrets`, Settings › Secrets or `wth --secret-set NAME` (a server over ssh; value read without echo or piped on stdin). The agent uses a secret by name — `{{secret:NAME}}` in any tool argument — and never sees its value; an existing `credentials.yaml` is imported into the vault and removed on first start. See [`docs/design/37-secrets.md`](docs/design/37-secrets.md).
- `providers.yaml` — named OpenAI-compatible gateways (`name: { base_url }`); keys live in the vault. `/provider add` writes both and fetches `GET {base}models` into `models.json`.
- `models.json` — optional, same shape as the bundled catalog; adds or overrides models (local `ollama/...`, private gateways). Refresh the bundled data with `just models`. `/provider add` writes the fetched model list here.
- `oauth/` — tokens from `/login` (`openai.json` = ChatGPT subscription via the Codex client, `anthropic.json` = Claude Pro/Max via the Claude Code client, `cursor.json` = Cursor subscription, `xai.json` = SuperGrok via the Grok CLI client; `mcp/<server>.json` = MCP servers that answered 401), mode 0600; `/logout <name>` deletes one. See [`docs/design/05-oauth.md`](docs/design/05-oauth.md).
- `prompt.md` — standing instructions, read into the system prompt on every model call; `/prompt` shows the assembled prompt.
- `mcp/*.json` — drop in any `mcp.json` (Claude Desktop, Cursor, OpenCode, …); hot-reloaded, tools registered under their own names — a name two servers both offer (four `grafana-*` entries) registers as `<server>__<name>` for each. `${VAR}` and `$(cmd)` in `command`/`args`/`env`/`url`/`headers` expand through your shell when the server starts (`"GITHUB_PERSONAL_ACCESS_TOKEN=$(gh auth token)"`); `{{secret:NAME}}` in the same fields is filled from the vault, so a token never sits in the file. `cwd` (relative → `mcp/`) points a server at a checkout under `mcp/repos/`, where `/mcp add <url>` clones and `/mcp sync` pulls. A remote entry's optional `oauth: {client_id, client_secret, scopes, redirect_uri}` shapes its login; `redirect_uri` (`http://localhost:8765/callback`) is for servers that only accept a registered loopback URL.
- `mcp/config.yaml` — per-server / per-tool risk override (`read_only` | `mutating` | `destructive` | `off`); `/mcp` edits it (`off` on a server = disabled). An MCP tool's risk defaults to its annotations, and one without annotations prompts by default; set `risk: read_only` on a trusted server once.
- `acp/agents.yaml` — one entry per external harness (`name: {command, args, env, description}`); `/acp` lists, probes, adds from the registry (`/acp add <id>`), removes. The model spawns one with `spawn_subagent{agent}`; it runs as a child session like a wth subagent.
- `skills/` — `<name>/SKILL.md` directories, loose `*.md`, or git clones (`/skills add <url>`, `/skills sync`); see [`docs/skills.md`](docs/skills.md). `harness.md` is written here on every start: the model's manual for this data dir and its config, loaded on demand.
- `knowledge/` — the knowledge bundle (OKF markdown, one `<id>.md` per concept, a git repo kept in-process); `knowledge.sqlite` beside it is its search index and can be deleted.
- `artifacts/` — content-addressed blobs of every file the agent has `show`n; `artifacts.sqlite` beside it is the index. Deleting the index orphans the blobs, which are swept at the next start.

TUI: `/` opens the command list; `/model` picks a model (header above the
input shows model, effort and context used); `/effort [level|clear]` sets the
reasoning effort for this session and as the default (`agent.effort`), from
the next turn; `/mcp` sets a server's or tool's risk or turns it off (`l`
logs a `login required` server in), `/mcp add <url|json>` installs one,
`/mcp sync` pulls repos; `/login` picks a provider or MCP server to log in
to; `/provider add` prompts for name, base url, then API key (an OpenAI-compatible
gateway such as `https://api.ai.rwx.dev/v1`); `/provider` lists them, `/provider rm <name>`
forgets one; `/skills` lists skills (⏎ hands one to the model, `s` shows it; `/<skill> [words]` does the same; the status line names the skills in context); `/prompt` shows the system prompt; `/compact` folds into a summary by default; `/compact cut` drops the folded span with no model call; `/compact style` lists/sets the default; `/clear` drops the conversation and keeps the session (id, title, model, settings; the history is still there on `/resume`);
`/undo` drops the last turn (yours and the reply), `/redo` restores it; `/edit [n]` edits a user message and regenerates from it, `/regen [n]` regenerates the reply to a turn, `/fork [n]` opens a new session with the conversation through a turn; URLs in the transcript are OSC 8 hyperlinks (click them in a terminal that supports it, e.g. ghostty/iTerm2/kitty/WezTerm);
`/jobs` lists the session's shell jobs, `/jobs kill <id>` stops one (a running job's card shows its last 10 output lines live);
`/schedules` lists the scheduled runs and loops with their next and last fire and run count, `/schedules run <name>` fires one now, `/schedules pause|resume <name>` flips its `enabled:` line, `/schedules history <name>` shows its runs and reports;
`/artifacts` lists the files the agent has shown in this session and its subagents (⏎ opens one with the OS, `s` shows it inline, `e` exports it); a `▣` artifact card's path is a `file://` link, `o` opens the newest one, Ctrl+O renders markdown, code and CSV inline;
`/secrets` lists the vault's secrets with their versions (`a` adds one into a masked field, `r` replaces the value as a new version, `h` shows history and restores an older version, `d` deletes); `/secrets add [name]`, `/secrets rm <name>`, `/secrets restore <name> [n]` skip the menu; when the agent calls `secret_request` a masked box asks you to type it;
`/acp` manages external ACP harnesses;
`/knowledge` browses the knowledge base (below); `/approvals off` stops
approval prompts for this session, `/approvals off always` for good
(`tools.approvals` in config.yaml). Typing while the model works queues the
message: it is shown under the response until it enters the context (after
the current tool batch or when the turn ends), and `⌥↑` pulls the newest one
back into the editor. Shift+Enter inserts a newline; PgUp/PgDn (or the
wheel, ↑/↓) scroll the whole history — what you are reading stays put while
a reply streams, PgDn to the bottom follows again; select
text with the mouse to copy it (terminal-native). Images: **Ctrl+V** (not
⌘V — the terminal's paste sends text only, so a screenshot on the clipboard
pastes nothing) attaches the clipboard image as `[⎘ image 1: W×H]`; dropping
an image file onto the terminal attaches it by path. See
[`docs/design/02-models-and-menus.md`](docs/design/02-models-and-menus.md).

Knowledge: durable facts about your machines, projects, accounts and
decisions, kept across sessions as plain markdown concepts in
`~/.wth/knowledge/` (Google's Open Knowledge Format — any editor works). The
agent reads it through `knowledge_search`/`knowledge_get`, and matching
concepts are surfaced to it as `<knowledge>` hints at the start of a turn and
every few tool rounds. It is built while you work: after every third turn
that used tools (and right before a compaction) the harness asks the model,
on the frame it already has cached, what is worth keeping, and writes the
answer as concepts; the model can also record with `knowledge_put` at any
time. Every write is a git commit made in-process (`gix`), so no `git`
binary is needed and history and undo work everywhere: `/knowledge` lists
concepts, `/knowledge <query>` searches, `/knowledge history [id]` shows the
commits (and the latest diff), `/knowledge restore <id> [commit]` puts a
concept back, `/knowledge harvest` runs a harvest now. Turn off with
`knowledge.enabled: false`; tune hint/harvest cadence under `agent.knowledge`
in `config.yaml`. See
[`docs/design/10-knowledge.md`](docs/design/10-knowledge.md).

Files the agent makes for you: when the deliverable is a document — a report,
a table, a chart, a PDF — the agent calls `show` instead of pasting it into
the reply. `show{path}` displays a file that exists (something it wrote, a
screenshot, a PDF you dropped); `show{content, name}` writes one under
`~/.wth/artifacts/` and displays it. The extension decides the rendering:
`.md` as markdown, `.csv` as a table, `.png`/`.svg` as a picture, `.pdf`
compiled from the markdown (or Typst) in `content` into a real PDF — the
model gets the first page back as an image and fixes what it sees. Every
card is an immutable snapshot with a version, so the transcript never lies
about what you were shown; the same name again in a session is `v2`. In
Slack or Signal the file lands in the bound thread. `message_user` is still
how the agent reaches you when you are not looking at this chat. Turn off
with `artifact.enabled: false`. See
[`docs/design/35-artifacts.md`](docs/design/35-artifacts.md).

Signal: a phone-side frontend that runs only in the daemon. Pair once with
`wth --link-signal --owner +15551234567` (scan the QR from Signal → Settings
→ Linked devices on the phone that owns the account; the daemon is a linked
device, so that account needs a primary phone — usually a dedicated number
you text from your own), then `wth --headless`. The owner's number must be
saved as a contact on that primary phone, or its messages arrive without a
number and are dropped. Texts become turns in a per-contact session (👀
while working, ✅/🛑/❌ when done; a message mid-turn steers it); the slash
commands mirror the TUI (`/status`, `/new`, `/sessions`, `/resume <id>`,
`/model`, `/effort`, `/compact`, `/clear`, `/undo`, `/redo`, `/signal allow <n>`, …); tool approvals arrive as a
message you answer with 👍/👎 or `/approve <code>`. Inbound files land in
`~/.wth/signal/inbox/`. See
[`docs/design/09-signal.md`](docs/design/09-signal.md).

Slack: a workspace-side frontend that runs only in the daemon. `wth
--slack-setup` walks through it in one terminal session: it asks for an
app configuration token (api.slack.com/apps → Your App Configuration
Tokens), creates the app from the bundled manifest (`--app <A…>` updates an
existing one instead), prints the install URL, verifies the bot token
(`xoxb-`) with `auth.test` and the app-level token (`xapp-`,
`connections:write`) with `apps.connections.open`, asks for your member id
(`U…`), and writes `slack_bot`/`slack_app` into the secrets vault and
`slack.owner`/`slack.enabled` into `config.yaml`; re-running skips every
step whose token still verifies. The manual path is `wth --slack-manifest`:
create the app from the printed manifest at api.slack.com/apps, install it,
store the two tokens with `wth --secret-set slack_bot` / `slack_app` and set `slack.owner` yourself.
Then `wth --headless`. Each top-level message in the bot's DM (or an `@wth` mention in
a `slack.channels` channel) opens a session whose replies stream live into
the thread — one message per turn with task cards for tool calls, a quoted
reasoning message when `slack.reasoning` is on, and a footer with model,
tokens and elapsed time; a reply in the thread steers a running turn or
starts the next one. Tool approvals arrive as a card with Allow/Deny
buttons (in the session's thread, or in the owner's DM for sessions started
elsewhere when `slack.approvals: all`). Every subagent gets its own thread
with a live board of descendants in the parent thread. The `/wth` slash
command carries the sub-commands (replies are ephemeral): `help` — the
list; `status` — daemon and session state; `sessions` — recent sessions;
`resume <id>` — bind a new thread to a session; `cancel [id]` — stop the
turn; `compact [id]` — compact the context; `model [id]` — list or set the
model; `agents` — the subagent tree; `prompt` — the system prompt; `skills
[add <url>|rm <name>|sync]`; `mcp [add <url|json>|sync]`; `login [name]` /
`logout [name]` — MCP OAuth; `approvals on|off [always]` (owner); `slack
[status|allow <U>|rm <U>|channel add <C>|channel rm <C>]` (owner). The
native stop button needs the `agent_session_stopped` event in the app's
Event Subscriptions (the manifest includes it). See
[`docs/design/21-slack.md`](docs/design/21-slack.md).

Desktop: a macOS menubar app (`crates/desktop`, Tauri 2) that looks like
chatgpt.com and does everything the TUI does over the JSON-RPC socket —
sessions in a sidebar with the subagent tree under the open one, streamed
markdown with tool cards, inline approvals, steering chips, attachments,
the same slash commands, a settings dialog for models, approvals, MCP,
skills, ACP agents, logins, gateways, channels and linked devices, and a knowledge explorer
with working links and a raw editor; a globe in the chat header opens a live
view of the agent's browser while it works. A file the agent shows is a card
with a thumbnail in the transcript and opens in a side panel with a viewer per
kind (markdown, highlighted code, CSV table, image, native PDF) and Copy /
Save as / Open / Reveal; a `▣ N` button in the header lists the session's
artifacts. It has no Dock icon; the tray icon or
⌥Space toggles the window. On launch it reuses a running `wth --serve` on
the configured port, otherwise it starts the bundled `wth --headless` on a
pseudo-terminal through your login shell so the daemon has exactly the
environment a terminal would (design doc 18). `just desktop` runs it
against `target/debug/wth`; `just package-macos` produces `wth.app`. See
[`docs/design/25-desktop.md`](docs/design/25-desktop.md).

## Link

Drive the harness from a phone (or any browser) without opening a port.
Pair in three steps:

1. Run the daemon with link on: set `link.enabled: true` (or `/link on` in
   the TUI, or the toggle in the desktop app's Settings › Link) and start
   `wth --headless`.
2. Show a pairing code: `wth --link` on the same machine, `/link pair` (or
   `/link` then `p`) in the TUI, or Settings › Link › Pair a device. It is
   a QR of `https://<relay>/link#1.…` plus the same string as text, valid
   for a few minutes; the secret is in the URL fragment, which the relay
   never receives.
3. Scan it with the phone's camera, open the URL, or paste the string into
   the app's Add harness screen. A phone without the app lands on the page
   the relay serves and pairs there; the desktop app pairs the same way to
   drive another machine. Devices are listed, renamed and revoked from
   `/link` or Settings › Link.

Self-host the relay (the default `link.relay` is the operator's; any
`wth-relay` works):

```sh
just web-build                                                             # browser UI → crates/desktop/ui/dist
cargo run -p relay -- --listen 0.0.0.0:8787 --web crates/desktop/ui/dist   # plain; put TLS in front or add --tls CERT KEY
RELAY_HOST=relay.example.com docker compose -f deploy/relay/compose.yaml up -d   # or: relay + Caddy with automatic TLS
# then in config.yaml: link.relay: wss://relay.example.com
```

The relay keeps no state and no accounts: everything after the WebSocket
upgrade is Noise-encrypted end to end between your device and your harness,
so the relay (ours or anyone's) sees ciphertext, frame sizes, timing, IP
addresses and the harness id — never your messages, keys or which device
is connected. Design: [`docs/design/31-link.md`](docs/design/31-link.md).

## Nodes

Give the model, and your phone, another machine. Four lines on the box:

```sh
ssh box
brew install skymoore/tap/wth         # or `just install` (release `wth` into ~/.cargo/bin), or copy the binary over
wth --secret-set anthropic            # a model key, typed without echo (or piped: `printf %s "$KEY" | wth --secret-set anthropic`)
wth --install-service --relay wss://relay.example.com   # writes a launchd LaunchAgent / systemd user unit running `$SHELL -l -i -c 'exec wth --headless'`, sets link.enabled, starts it, prints a QR + key
```

On your main harness, `/nodes add <key>` in the TUI (or Settings › Nodes ›
Add node in the desktop app, or scan the QR with the phone and choose "as a
node of …"). From then on `spawn_subagent{agent: "<node>"}` runs the child
on that machine over the same encrypted link conn, its approvals land in
your UI, and the desktop and phone switcher list the node nested under its
parent — your devices pair with it automatically, so switching to it shows
its own sessions. `/nodes` lists, renames, removes and invites; `wth
--uninstall-service` removes the unit and nothing else. The node has its own
`~/.wth` (config, secrets vault, skills, MCP servers): nothing is synced.
Design: [`docs/design/32-nodes.md`](docs/design/32-nodes.md).
