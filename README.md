# theuth

A single-process AI harness that does things for the user. Plugins are Rust
crates talking over a typed message bus; any frontend attaches over JSON-RPC.
Design: [`docs/design/00-architecture.md`](docs/design/00-architecture.md).
Adding a plugin: [`docs/plugins.md`](docs/plugins.md).

Install without building: `brew install basedatum/tap/theuth` (the CLI, macOS
arm64 and Linux) or `brew install --cask basedatum/tap/theuth` (the macOS app).
Both keep themselves current ([`docs/design/39-update.md`](docs/design/39-update.md));
`t --update` forces a check. Building from source:

```sh
just setup                 # rustup toolchain; `just toolchain` prints its bin dir
export ANTHROPIC_API_KEY=… # any genai provider works; default model in config   # or: t --secret-set anthropic
just run                   # terminal UI; data dir is ~/.theuth (--data-dir / THEUTH_DATA_DIR relocate it)
just run -- --headless     # JSON-RPC only, ws://127.0.0.1:7777 with <data_dir>/token
just run -- --resume <id>  # reopen a session
just run -- -p "prompt"    # print mode: stream the reply, exit when the turn ends
echo "prompt" | just run -- -p   # same, prompt from stdin
just run -- --link-signal --owner +15551234567   # pair as a Signal linked device (one-shot QR), then run --headless
just run -- --link             # pair a phone with the running daemon (one-shot QR + key); needs `t --headless` on this box
just run -- --install-service --relay wss://relay.example.com   # on a server: run t --headless as a launchd/systemd user service through your login shell, print the key to pair it to your main harness as a node
just run -- --uninstall-service                                  # remove that unit; the data dir and every pairing stay
just relay                     # local theuth-relay on 127.0.0.1:8787 for link development; `just web-build` builds the browser UI it serves
just run -- --slack-setup      # create the Slack app from the manifest, verify tokens, write config; then run --headless
just run -- --slack-manifest   # manual path: print the Slack app manifest for api.slack.com/apps → From a manifest
just run -- --acp          # Agent Client Protocol on stdio: use theuth from Zed/editors (agent_servers: {"theuth": {"command": "t", "args": ["--acp"]}})
just ts                    # TypeScript bindings for the wire types → bindings/
just helper                # app parser helper (bun build --compile) → packages/apps/dist/theuth-app-parser; apps need it beside `t`
THEUTH_APP_PARSER=packages/apps/dist/theuth-app-parser just run   # dev run with that helper (release layouts find theuth-app-parser-<hash> next to `t` on their own)
just install               # release `t` + `theuth-app-parser-<hash>` into ~/.cargo/bin; older helpers stay for any still-running t
just desktop               # macOS menubar app in dev mode (Tauri + Vite); starts the daemon itself
just package-macos         # theuth.app + dmg; ad-hoc unless THEUTH_SIGN_IDENTITY / .env.signing
just package-ios           # signed IPA for TestFlight (Apple Distribution)
just testflight            # upload last IPA to App Store Connect
just install-macos         # copy the packaged app to /Applications
```

Build prerequisites: `cmake`, `clang`/`libclang` and `llvm-objcopy`/`llvm-nm` for the embedded browser's
BoringSSL (`brew install cmake llvm` / `apt install cmake clang libclang-dev`),
network on the first build for the prebuilt V8 archive (set
`RUSTY_V8_ARCHIVE` to a local copy to build offline), and `bun` to *build* the
app parser helper (`brew install oven-sh/bun/bun`) — the result is a standalone
executable, so an installed `t` needs no Node or Bun at run time. The Homebrew
formula and the self-updater install the helper with `t`.

macOS permissions: `computer_*` needs Screen Recording and Accessibility
granted to the app that launched theuth — your terminal (Ghostty, iTerm,
Terminal) or theuth.app — in System Settings › Privacy & Security. Screen
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
drive it: type, read the rendered 120×40 screen, wait for a prompt or a port;
you can open the same terminal from `/terminals` or the desktop and type into
it yourself (`i` / Ctrl-] in the TUI) —
[`docs/design/22-pty.md`](docs/design/22-pty.md), [`55`](docs/design/55-interactive-terminals.md)), `secrets` (an encrypted,
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
[`docs/design/27-computer.md`](docs/design/27-computer.md)), `document`
(the Yjs authority behind every BlockNote editor — `yrs` documents under
`documents/`, derived markdown export/import, the `packages/documents` helper
— [`docs/design/59-documents.md`](docs/design/59-documents.md)), `knowledge`
(OKF bundle, FTS index, gix history, `knowledge_*` tools —
[`docs/design/10-knowledge.md`](docs/design/10-knowledge.md)), `agent` (genai
loop, sqlite sessions with compaction epochs, skills, subagents, persistent
Python runtime over monty worker subprocesses —
[`docs/design/08-repl.md`](docs/design/08-repl.md); with `agent.vision_model`
set, a text-only main model still sees images: it writes a question, that
image-capable fallback model answers it, and the answer is appended as an observation —
[`docs/design/52-vision-fallback.md`](docs/design/52-vision-fallback.md); `message_user` sends text
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
over Signal messenger), `slack` (Slack frontend over Socket Mode), `rpc` (jsonrpsee;
also registers the `search` tool — one fuzzy search over this harness's saved
sessions, shown artifacts, current knowledge and knowledge history, for when
you ask the agent to find something you did earlier —
[`docs/design/49-search.md`](docs/design/49-search.md)),
`link` (drive the harness from a phone or browser through a relay: Noise
end-to-end encryption, pairing QR, device registry —
[`docs/design/31-link.md`](docs/design/31-link.md)), `relay` (the `theuth-relay`
binary: a stateless relay that copies encrypted frames between one harness
and its devices), `schedule` (scheduled runs and loops: one `~/.theuth/schedules/<name>.md`
per schedule with `cron:` / `every:` / `at:` front matter and the prompt as
body; each fire is a turn in that schedule's own session, the reply is
delivered to the user; add `until:` (a shell check) or `max_runs:` and it
loops until done, `every: 0s` for back to back, `approvals: false` for
unattended, `schedule_done` when the model decides —
[`docs/design/34-schedule.md`](docs/design/34-schedule.md),
[`docs/design/33-loops.md`](docs/design/33-loops.md)), `artifact`
(the `show` tool: files the agent makes for you, shown as cards in the chat —
markdown, code, CSV, images, SVG, PDFs compiled in-process from markdown or
Typst; a content-addressed store under `~/.theuth/artifacts/` —
[`docs/design/35-artifacts.md`](docs/design/35-artifacts.md)), `a2a` (remote
agents over A2A 1.0 on the official Rust SDK: configured peers and exports,
off by default — Communication below,
[`docs/design/50-agent-communication.md`](docs/design/50-agent-communication.md)),
[`docs/design/35-artifacts.md`](docs/design/35-artifacts.md)), `app` (the
`app` tool: interactive OpenUI Lang screens the agent publishes as `.openui`
artifacts and you use on desktop/web; a headless `theuth-app-parser-<hash>`
helper validates source, `app_*` RPC runs an app's Query/Mutation bindings
through the tool registry — [`docs/design/46-apps.md`](docs/design/46-apps.md)),
`tui` (iocraft), `theuth` (binary).

Bedrock: select an `amazon-bedrock/<model-id>` through `/model` (cross-region
inference profile ids such as `us.anthropic.…` work too). Configure AWS normally;
the Rust SDK discovers profiles from both AWS files, including SSO/role profiles
and custom file paths. `/aws` opens the TUI picker when multiple profiles exist;
desktop Settings → Connections → AWS Bedrock provides the same selection.
`/aws work` persists `agent.aws_profile: work` for every session's next Bedrock
request; `/aws clear` restores automatic authentication. An explicit profile
overrides ambient access keys and Bedrock bearer keys. Otherwise a `bedrock_api`
vault secret, `BEDROCK_API_KEY` or `AWS_BEARER_TOKEN_BEDROCK` wins over the default
AWS chain. The SDK owns region, credentials and SSO; sign in with
`aws sso login --profile work` when required. No AWS credentials enter theuth's UI.

Chat failures (including denied model/region access and expired AWS sessions) stay
in the TUI and desktop/mobile transcript after the turn finishes, on reload, and
when reopening the session. They are not sent to the model as conversation text.
AWS credential failures include SSO login guidance; fix the profile/login or
model access, then send the request again.

Config (`~/.theuth/`, created on first run; details in
[`docs/design/01-config-and-subagents.md`](docs/design/01-config-and-subagents.md) §1):

- `config.yaml` — every plugin reads its own section; `THEUTH_<SECTION>__<KEY>` env overrides. `agent.model` is a models.dev id (`anthropic/claude-sonnet-4-5`); `/model` rewrites that one line in place. The `agent`, `tools`, `secrets`, `documents`, `knowledge`, `web`, `scrape`, `search_providers`, `browser`, `computer`, `link`, `nodes`, `schedule`, `a2a`, `update`, `artifact`, `app`, `voice` and `slack` sections with their defaults:

  ```yaml
  agent:
    model: anthropic/claude-sonnet-4-5
    communication: false   # model-to-model communication (design doc 50, below): off = no spawn_subagent, discover_peers or send_message; `/communication on always` writes true
    # effort: high        # low|medium|high|… per model (/model lists them); unset = provider default
    # vision_model: anthropic/claude-haiku-4-5  # answers the main model's questions about images it cannot see (design doc 52);
                             # unset: no fallback. Must be an exact catalog `provider/model` that accepts images;
                             # images then go to that provider, with extra calls. /vision or Settings → Agent loop
    # stream_idle_secs: 120  # seconds without a stream event before a model call is abandoned (design doc 36 §2.3)
    # max_salvages: 2        # interrupted-reply recoveries per turn (stalled stream or output cap) before the turn errors; 0 = off
    # aws_profile: work   # Bedrock inference; /aws or Settings → Connections selects it
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
  secrets:                       # design doc 37; `/secrets`, Settings › Secrets, `t --secret-set`
    key_store: auto              # auto | keychain | file — where the vault key lives; auto = keychain when one answers, else <data_dir>/secrets.key (0600) with a warning
    tools: true                  # false: no secret_* tools, no prompt block, `{{secret:…}}` stays literal, nothing is scrubbed; the vault, /secrets and the harness's own keys are unaffected
    request_timeout_secs: 1800   # `secret_request` gives up (the model sees "no answer") after this
  knowledge:
    enabled: true          # false: no store, no knowledge_* tools, no hints, no harvest
  documents:               # design doc 59; the BlockNote editor's Yjs documents
    enabled: true          # false: no store, no helper, no doc_* methods; concepts open as raw text
    export_debounce_ms: 400
    max_doc_bytes: 8388608
    snapshots_keep: 30
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
    relay: wss://relay.theuth.io   # self-host: your `theuth-relay` URL
    name: ""                      # display name in the QR and on devices; "" → hostname
    max_devices: 16
  nodes:                          # design doc 32; `/nodes add <key>` pairs, the node ran `t --install-service`
    enabled: true                 # false: no dials, nodes invisible to the model and the UI; nodes.yaml kept
  schedule:                       # design docs 34 + 33; `~/.theuth/schedules/*.md` files, `/schedules` lists them
    enabled: true                 # false: no watcher, no actor, no `schedule` tool, no prompt block
    run_timeout_secs: 1800        # a run still going after this is cancelled and reported as `timeout`
  a2a:                            # design doc 50 §6; remote agents over A2A 1.0. `/a2a` shows status, `/a2a reload` re-reads ~/.theuth/a2a/peers.yaml
    enabled: false                # false: no actor, no listener, no card fetch, no credential load. Also needs agent.communication: true
    listen: 127.0.0.1:7780        # loopback only; restart to change
    public_url: null              # the HTTPS base your reverse proxy publishes; without it the listener is plain HTTP for loopback development only
  update:                         # design doc 39; self-update, silent: verify the signature, replace the binary on disk, live at the next start
    enabled: true                 # false: never checks; `t --update` still works
    url: https://s3.rwx.dev/theuth/releases/latest.json   # the release manifest; point at a scratch bucket to test
  artifact:                       # design doc 35; the `show` tool, `/artifacts` lists a session's
    enabled: true                 # false: no `show` tool, no prompt block, no /artifacts; blobs and index kept
    fonts: []                     # extra directories searched for PDF fonts, besides system fonts and the embedded set
  app:                            # design doc 46; the `app` tool, `/apps` lists a session's
    enabled: true                 # false: no `app` tool, prompt block, helper, `get_app`/`app_*` RPC or /apps; existing apps stay readable as source (/artifacts, Ctrl+O)
  voice:                          # design docs 43 + 45 + 54; Read-aloud, voice notes and dictation: Pocket TTS, Parakeet and Nemotron streaming STT in the harness
    enabled: true                 # false: no actors, `speak`/`transcribe` answer "not running", the desktop shows no speaker or mic
    voice: peter_yearsley         # a `voices/<name>.safetensors` in the checkpoint directory
    # model_dir: ~/.theuth/models/pocket-tts   # the Pocket TTS checkpoint (weights, tokenizer.model, voices/); this is the default
    # stt_model_dir: ~/.theuth/models/parakeet-tdt  # Parakeet TDT ONNX (encoder-model.onnx, decoder_joint-model.onnx, vocab.txt) plus libonnxruntime.{dylib,so}; this is the default
    stt_idle_secs: 600            # unload the speech-to-text model (1.7 GB resident) after this long unused; 0 keeps it resident
    # dictation_model_dir: ~/.theuth/models/nemotron-streaming-en  # Nemotron streaming STT ONNX (encoder.onnx, decoder_joint.onnx, tokenizer.model) for live dictation; this is the default
    dictation_max_secs: 300       # a running dictation stops itself after this long (design doc 54)
  slack:
    # owner: U0123456789           # owner-only commands, may click any approval, receives unbound approvals
    allowed: []                    # more user ids; each owns their threads
    channels: []                   # channel ids where @theuth is honoured; empty = DMs only
    tools: all                     # all | errors | none — task cards per tool call
    reasoning: true                # the quoted reasoning message
    approvals: all                 # all | threads — where approvals for non-Slack sessions go
  ```

  The Python runtime is always on: the worker is the `theuth` binary itself (`t subprocess`, nothing to install); every tool is an `async` function inside the sandbox and state persists across turns.
- `secrets.vault` — encrypted (XChaCha20-Poly1305) vault of named, versioned secrets; the key lives in the OS keychain (or `secrets.key`, mode 0600, where there is none). Provider keys (genai adapter names: `anthropic`, `openai`, …) beat env vars; a provider with a key, a login, or its env var is "configured": `/model` lists only those. Named OpenAI-compatible gateways (`/provider add`) store their key here under the gateway name; web search keys (`firecrawl`, `brave`, optional `github`) and the Slack tokens (`slack_bot`, `slack_app`) live here too. Add, replace, restore or delete with `/secrets`, Settings › Secrets or `t --secret-set NAME` (a server over ssh; value read without echo or piped on stdin). The agent uses a secret by name — `{{secret:NAME}}` in any tool argument — and never sees its value; an existing `credentials.yaml` is imported into the vault and removed on first start. See [`docs/design/37-secrets.md`](docs/design/37-secrets.md).
- `providers.yaml` — named OpenAI-compatible gateways (`name: { base_url }`); keys live in the vault. `/provider add` writes both and fetches `GET {base}models` into `models.json`.
- `models.json` — optional, same shape as the bundled catalog; adds or overrides models (local `ollama/...`, private gateways). Refresh the bundled data with `just models`. `/provider add` writes the fetched model list here.
- `oauth/` — tokens from `/login` (`openai.json` = ChatGPT subscription via the Codex client, `anthropic.json` = Claude Pro/Max via the Claude Code client, `cursor.json` = Cursor subscription, `xai.json` = SuperGrok via the Grok CLI client; `mcp/<server>.json` = MCP servers that answered 401), mode 0600; `/logout <name>` deletes one. See [`docs/design/05-oauth.md`](docs/design/05-oauth.md).
- `prompt.md` — standing instructions, read into the system prompt on every model call; `/prompt` shows the assembled prompt.
- `mcp/*.json` — drop in any `mcp.json` (Claude Desktop, Cursor, OpenCode, …); hot-reloaded, tools registered under their own names — a name two servers both offer (four `grafana-*` entries) registers as `<server>__<name>` for each. `${VAR}` and `$(cmd)` in `command`/`args`/`env`/`url`/`headers` expand through your shell when the server starts (`"GITHUB_PERSONAL_ACCESS_TOKEN=$(gh auth token)"`); `{{secret:NAME}}` in the same fields is filled from the vault, so a token never sits in the file. `cwd` (relative → `mcp/`) points a server at a checkout under `mcp/repos/`, where `/mcp add <url>` clones and `/mcp sync` pulls. A remote entry's optional `oauth: {client_id, client_secret, scopes, redirect_uri}` shapes its login; `redirect_uri` (`http://localhost:8765/callback`) is for servers that only accept a registered loopback URL.
- `mcp/config.yaml` — per-server / per-tool risk override (`read_only` | `mutating` | `destructive` | `off`); `/mcp` edits it (`off` on a server = disabled). An MCP tool's risk defaults to its annotations, and one without annotations prompts by default; set `risk: read_only` on a trusted server once.
- `acp/agents.yaml` — one entry per external harness (`name: {command, args, env, description}`); `/acp` lists, probes, adds from the registry (`/acp add <id>`), removes. The model spawns one with `spawn_subagent{agent}`; it runs as a child session like a theuth subagent.
- `a2a/peers.yaml` — remote A2A agents and the local sessions published to them (see Communication below); owner-edited, `/a2a reload` validates and applies it atomically. No model tool writes it.
- `skills/` — `<name>/SKILL.md` directories, loose `*.md`, or git clones (`/skills add <url>`, `/skills sync`); see [`docs/skills.md`](docs/skills.md). `harness.md` is written here on every start: the model's manual for this data dir and its config, loaded on demand.
- `knowledge/` — the knowledge bundle (OKF markdown, one `<id>.md` per concept, a git repo kept in-process); `knowledge.sqlite` beside it is its search index and can be deleted.
- `documents/` — one directory per Yjs document (`meta.json`, `snapshot.yjs`, `updates.log`, `snapshots/`) behind the BlockNote editor; `index.sqlite` is its index. A knowledge concept's document is the truth while it is open in the editor and `knowledge/<id>.md` is its export; deleting a document directory loses only the editing history, the file is recreated as a document on next open.
- `artifacts/` — content-addressed blobs of every file the agent has `show`n or published with `app`; `artifacts.sqlite` beside it is the index, and also holds one row per app invocation (which view pressed which binding, status, result). Deleting the index orphans the blobs, which are swept at the next start. `artifacts.owners/` holds one OS-locked file per running harness on this data dir: invocation rows belong to the owner that dispatched them, so a second harness (or a restart) recovers only rows whose owner is provably dead and reports them as `unknown`, never retried.

TUI: `/` opens the command list; `/model` picks a model (header above the
input shows model, effort and context used); `/effort [level|clear]` sets the
reasoning effort for this session and as the default (`agent.effort`), from
the next turn; `/mcp` sets a server's or tool's risk or turns it off (`l`
logs a `login required` server in), `/mcp add <url|json>` installs one,
`/mcp sync` pulls repos; `/login` picks a provider or MCP server to log in
to; `/provider add` prompts for name, base url, then API key (an OpenAI-compatible
gateway such as `https://api.ai.rwx.dev/v1`); `/provider` lists them, `/provider rm <name>`
forgets one; `/skills` lists skills (⏎ hands one to the model, `s` shows it; `/<skill> [words]` does the same; the status line names the skills in context); `/prompt` shows the system prompt; `/compact` folds into a summary by default; `/compact cut` drops the folded span with no model call; `/compact style` lists/sets the default; `/clear` drops the conversation and keeps the session (id, title, model, settings; the history is still there on `/resume`);
`/undo` drops the last turn (yours and the reply), `/redo` restores it; `/shake [images|documents|messages|tools|summary]` removes chosen content from the model's context without dropping what came after it — bare `/shake` opens a picker, a category (e.g. `/shake images`) pre-selects every matching occurrence in the current context, Space toggles a row, Enter reviews the exact targets (a removed tool exchange or assistant message takes its protocol companions with it) and confirms; no model call, later replies stay, the original rows stay in the transcript as collapsed *removed* cards and `/undo` does not bring the content back (attach or send it again); works on idle native sessions only ([`docs/design/48-shake.md`](docs/design/48-shake.md)); `/edit [n]` edits a user message and regenerates from it, `/regen [n]` regenerates the reply to a turn, `/fork [n]` opens a new session with the conversation through a turn; URLs in the transcript are OSC 8 hyperlinks (click them in a terminal that supports it, e.g. ghostty/iTerm2/kitty/WezTerm);
`/jobs` lists the session's shell jobs, `/jobs kill <id>` stops one (a running job's card shows its last 10 output lines live);
`/schedules` lists the scheduled runs and loops with their next and last fire and run count, `/schedules run <name>` fires one now, `/schedules pause|resume <name>` flips its `enabled:` line, `/schedules history <name>` shows its runs and reports;
`/artifacts` lists the files the agent has shown in this session and its subagents (⏎ opens one with the OS, `s` shows it inline, `e` exports it); a `▣` artifact card's path is a `file://` link, `o` opens the newest one, Ctrl+O renders markdown, code and CSV inline; `/apps` is the same picker for the apps the agent built — the TUI cannot run them (`interactive on desktop/web`), so `s`/Ctrl+O show the `.openui` source read-only and `e` exports it;
`/secrets` lists the vault's secrets with their versions (`a` adds one into a masked field, `r` replaces the value as a new version, `h` shows history and restores an older version, `d` deletes); `/secrets add [name]`, `/secrets rm <name>`, `/secrets restore <name> [n]` skip the menu; when the agent calls `secret_request` a masked box asks you to type it;
`/acp` manages external ACP harnesses;
`/communication` shows whether this session may talk to other agents and why not (global switch, an ancestor's opt-out, network state), `/communication on|off` opts this session and its descendants out or back in until the daemon restarts, `/communication on|off always` flips the harness-wide switch and writes `agent.communication`; `/peers` lists the peers this session may message and the requests waiting for a human answer (⏎ opens, `i` inspects, `a` answers by exact id), `/peers scope owner|isolated [alias]` sets the shown root's reach; `/a2a` shows network status and configuration errors, `/a2a reload` re-reads `a2a/peers.yaml`;
`/search [query]` searches everything saved on this harness live as you type
(below);
`/knowledge` browses the knowledge base (below); `/approvals off` stops
approval prompts for this session, `/approvals off always` for good
(`tools.approvals` in config.yaml). Typing while the model works queues the
message: it is shown under the response until it enters the context (after
the current tool batch or when the turn ends), `⌃↵`/`⌘↵` interjects instead
(the turn is cancelled and the message starts the next one now), and `⌥↑`
pulls the newest queued one back into the editor. Shift+Enter inserts a newline; PgUp/PgDn (or the
wheel, ↑/↓) scroll the whole history — what you are reading stays put while
a reply streams, PgDn to the bottom follows again; select
text with the mouse to copy it (terminal-native). Images: **Ctrl+V** (not
⌘V — the terminal's paste sends text only, so a screenshot on the clipboard
pastes nothing) attaches the clipboard image as `[⎘ image 1: W×H]`; dropping
an image file onto the terminal attaches it by path. See
[`docs/design/02-models-and-menus.md`](docs/design/02-models-and-menus.md).

Search: `/search [query]` in the TUI, and ⌘K / Ctrl+K, `/search`, or the
magnifier beside the sidebar toggle in the desktop/mobile app, open one
search over four categories — **Sessions** (titles and your/the agent's
messages), **Artifacts** (every version the agent has shown, by its saved
text or source — HTML, SVG and compiled PDFs match on their saved source,
PDFs without one and Office documents are converted once, images and other
binaries by name only, no OCR), **Knowledge** (current concepts) and
**Knowledge history** (the complete before/after text of every recorded
change, deleted concepts included). Chips toggle categories independently
(All selects all four) and are remembered per harness; results come grouped,
ten per category with Show more, and each group says whether it is ready,
still indexing, unavailable or partially covered. fzf syntax: fuzzy words,
`'exact`, `^prefix`, `suffix$`, `!not`, each against one title or body.
Opening a hit only reads: a session opens at that message, an artifact at
that exact version with a **Matched text** panel, a concept as it is now, a
history hit as a read-only **Historical version** — restoring stays a
separate action. The agent has the same search as the `search` tool
(read-only, also `await search(query, categories=…)` from the Python
runtime) for requests like "find the deployment plan I showed you last
week"; it returns the same grouped hits with exact ids, and it is not a
web search (`search_web`) and does not replace `knowledge_search`, which
the agent keeps using for ordinary fact lookup and the automatic
`<knowledge>` hints. Everything runs on the harness; a paired phone or
browser receives results, never the corpus. An app or browser build older
than the harness gets *Method not found* for `search` and needs updating
together with it. See [`docs/design/49-search.md`](docs/design/49-search.md).

Knowledge: durable facts about your machines, projects, accounts and
decisions, kept across sessions as plain markdown concepts in
`~/.theuth/knowledge/` (Google's Open Knowledge Format — any editor works). The
agent reads it through `knowledge_search`/`knowledge_get`, and matching
concepts are surfaced to it as `<knowledge>` hints at the start of a turn and
every few tool rounds. It is built while you work: after every third turn
that used tools (and right before a compaction) the harness asks the model,
on the frame it already has cached, what is worth keeping, and writes the
answer as concepts; the model can also record with `knowledge_put` at any
time. Every write is a git commit made in-process (`gix`), so no `git`
binary is needed and history and undo work everywhere. In the desktop/mobile
Knowledge view, **History** browses the whole bundle; a document's History
action narrows it to that concept. Select a commit to inspect its changes,
including deleted documents, and use **Older** to keep exploring. In the TUI,
`/knowledge` opens a picker (`h` for that concept's history, `H` for all),
`/knowledge <query>` searches, and `/knowledge history [id]` opens history
directly. Enter selects a commit's diff; the older row loads more commits.
`/knowledge restore <id> [commit]` puts a concept back;
`/knowledge harvest` runs a harvest now. Turn off with
`knowledge.enabled: false`; tune hint/harvest cadence under `agent.knowledge`
in `config.yaml`. See
[`docs/design/10-knowledge.md`](docs/design/10-knowledge.md).

Files the agent makes for you: when the deliverable is a document — a report,
a table, a chart, a PDF — the agent calls `show` instead of pasting it into
the reply. `show{path}` displays a file that exists (something it wrote, a
screenshot, a PDF you dropped); `show{content, name}` writes one under
`~/.theuth/artifacts/` and displays it. The extension decides the rendering:
`.md` as markdown, `.csv` as a table, `.png`/`.svg` as a picture, `.pdf`
compiled from the markdown (or Typst) in `content` into a real PDF — the
model gets the first page back as an image and fixes what it sees. Every
card is an immutable snapshot with a version, so the transcript never lies
about what you were shown; the same name again in a session is `v2`. In
Slack or Signal the file lands in the bound thread. `message_user` is still
how the agent reaches you when you are not looking at this chat. Turn off
with `artifact.enabled: false`. See
[`docs/design/35-artifacts.md`](docs/design/35-artifacts.md).

Apps: when the deliverable is a screen you work *in* — a dashboard, a control
panel, a form, a clickable prototype — the agent calls `app{name, source}`
with an OpenUI Lang program instead of a static `json-render`/Mermaid picture
or a `show` file. The desktop and the relay's browser UI open a panel while
the source is still streaming and promote it when the harness commits the
version; you press buttons, fill inputs, and its `Query` bindings read
installed tools (native, MCP, `http_request`) live, without another model
turn. **The press is the approval**: one click runs the chosen action — a
`Mutation`, including destructive or `{{secret:NAME}}`-bearing calls — and
there is no second confirmation card; a tool the policy would ask about
becomes a Load/Refresh button whose press is the consent, and a denied tool
is shown unavailable, not confirmed. Timer/load reads never run mutations.
Links, external HTTP(S) images and external services are ordinary app
functionality; a `{{secret:…}}` placeholder in the source is committed
literally and resolved only when the bound tool runs. Every version is an
immutable `.openui` artifact (`edit: true` merges a patch onto the receipt's
`record_call_id` and commits only if that version is still the latest); an
app's tool bindings are pinned by identity and schema, so a changed MCP
server shows as unavailable rather than silently rebinding. Where an app
cannot run — the TUI, Slack, Signal — its card is a receipt plus read-only
source (`/apps`). Validation runs in `theuth-app-parser-<hash>` beside the `theuth`
binary (one immutable file per library hash; `t --app-parser-hash` prints
the one this build wants; `THEUTH_APP_PARSER=<path>` points at another
executable for development); a missing helper fails the `app` call with
`apps are unavailable on this harness: parser helper missing at …`, a
wrong-version one with both hashes, and nothing else stops working. Turn off with `app.enabled: false`: no `app`
tool, prompt block, helper or `/apps`; the bundled `apps` skill is still
written, and existing apps stay readable as source. See
[`docs/design/46-apps.md`](docs/design/46-apps.md).

Signal: a phone-side frontend that runs only in the daemon. Pair once with
`t --link-signal --owner +15551234567` (scan the QR from Signal → Settings
→ Linked devices on the phone that owns the account; the daemon is a linked
device, so that account needs a primary phone — usually a dedicated number
you text from your own), then `t --headless`. The owner's number must be
saved as a contact on that primary phone, or its messages arrive without a
number and are dropped. Texts become turns in a per-contact session (👀
while working, ✅/🛑/❌ when done; a message mid-turn steers it); the slash
commands mirror the TUI (`/status`, `/new`, `/sessions`, `/resume <id>`,
`/model`, `/effort`, `/compact`, `/clear`, `/undo`, `/redo`, `/signal allow <n>`, …); tool approvals arrive as a
message you answer with 👍/👎 or `/approve <code>`. Inbound files land in
`~/.theuth/signal/inbox/`. See
[`docs/design/09-signal.md`](docs/design/09-signal.md).

Slack: a workspace-side frontend that runs only in the daemon. `theuth
--slack-setup` walks through it in one terminal session: it asks for an
app configuration token (api.slack.com/apps → Your App Configuration
Tokens), creates the app from the bundled manifest (`--app <A…>` updates an
existing one instead), prints the install URL, verifies the bot token
(`xoxb-`) with `auth.test` and the app-level token (`xapp-`,
`connections:write`) with `apps.connections.open`, asks for your member id
(`U…`), and writes `slack_bot`/`slack_app` into the secrets vault and
`slack.owner`/`slack.enabled` into `config.yaml`; re-running skips every
step whose token still verifies. The manual path is `t --slack-manifest`:
create the app from the printed manifest at api.slack.com/apps, install it,
store the two tokens with `t --secret-set slack_bot` / `slack_app` and set `slack.owner` yourself.
Then `t --headless`. Each top-level message in the bot's DM (or an `@theuth` mention in
a `slack.channels` channel) opens a session whose replies stream live into
the thread — one message per turn with task cards for tool calls, a quoted
reasoning message when `slack.reasoning` is on, and a footer with model,
tokens and elapsed time; a reply in the thread steers a running turn or
starts the next one. Tool approvals arrive as a card with Allow/Deny
buttons (in the session's thread, or in the owner's DM for sessions started
elsewhere when `slack.approvals: all`). Every subagent gets its own thread
with a live board of descendants in the parent thread. The `/theuth` slash
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
Save as / Open / Reveal; an app card opens the running app in that panel
(provisional while its source streams, live once committed); a `▣ N` button
in the header lists the session's artifacts. It has no Dock icon; the tray icon or
⌥Space toggles the window. On launch it reuses a running `t --serve` on
the configured port, otherwise it starts the bundled `t --headless` on a
pseudo-terminal through your login shell so the daemon has exactly the
environment a terminal would (design doc 18). `just desktop` runs it
against `target/debug/theuth`; `just package-macos` produces `theuth.app`. See
[`docs/design/25-desktop.md`](docs/design/25-desktop.md).

## Link

Drive the harness from a phone (or any browser) without opening a port.
Pair in three steps:

1. Run the daemon with link on: set `link.enabled: true` (or `/link on` in
   the TUI, or the toggle in the desktop app's Settings › Link) and start
   `t --headless`.
2. Show a pairing code: `t --link` on the same machine, `/link pair` (or
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
`theuth-relay` works):

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

## Communication

**Status: implemented.** Final communication verification after every review
correction: `cargo check --workspace --all-targets` green, 273 runtime tests,
208 channel/TUI/RPC tests and 134 frontend tests passed, `tsc` clean. Loopback
smokes passed 57/57 checks over the real supervisor and store and 39/39 checks
with the official Python SDK against a production export (plus 3/3 with the
export closed); the desktop surface was visually approved by the user.
Merged-main apps/search integration is being checked separately; these results
do not claim that merged integration has passed. The official A2A
conformance kit and Go CLI have only been run against the SDK probe, not the
production adapter. The network side rides on the young official Rust SDK
(`a2a-lf` 0.3.1 with two narrow vendored patches; accepted by the user with a
maturity waiver, nothing here calls it mature). The as-built record and the
open items are
[`docs/design/50-agent-communication.md`](docs/design/50-agent-communication.md)
§12.4–12.5; treat those, not this section, as the authority on what is proven.

**Breaking default.** Model-to-model communication is now off unless you
turn it on, and off means all of it: `spawn_subagent`, `discover_peers`
and `send_message` disappear from the model's tools, no child result wakes a
parent, no peer message is delivered. Existing configs without
`agent.communication` behave as `false`; nothing is grandfathered from earlier
spawns. To get subagents back:

```text
/communication on always      # writes agent.communication: true; same command in the TUI and desktop, `/theuth communication …` on Slack, `/communication …` on Signal
/communication off always     # turn it off again for good
/communication off            # this session and its descendants only; in memory, gone at daemon restart
/communication on             # undo that; cannot override a global off or an ancestor's off
/communication                # show global, session, inherited blocker, effective and network state
/peers                        # who this session may message, and requests waiting for your answer; `/requests` on Slack/Signal
```

Or set it in `config.yaml` (`agent.communication: true`) or the environment
(`THEUTH_AGENT__COMMUNICATION=true`; an env value wins over the file on every
start, and the status says where the value came from). The global switch is a
ceiling, like `/approvals`: a session can opt out under it, never opt in above
it. Session opt-outs are memory-only and end with the process. Approvals stay
separate: `/approvals off` does not turn communication on, and `send_message`
is a mutating tool that asks under the default policy.

With communication on, sessions of one tree (parent, children, siblings) can
discover and message each other by canonical address (`session:<uuid>`); a
child's final message arrives at its parent as a
`<message from="session:…" id="…">` input. Roots created from the owner
console are `owner` scope and may also reach other owner roots; roots created
by a scheduled run, an ACP client, or a non-owner Slack/Signal contact are
`isolated` to their own tree. Promote a root from its communication settings
(owner only; no model tool does this). `/peers` is the human view of the same
list; the model never sees another root's title, cwd or transcript.

**Network (A2A).** Remote agents are a second, separate switch. Nothing
listens, dials, fetches a card or loads a credential until `a2a.enabled: true`
is in `config.yaml` **and** communication is on; the default first-run config
has the section commented out. Peers and exports are the owner's file
`~/.theuth/a2a/peers.yaml`:

```yaml
peers:
  lab:
    card_url: https://lab.example/agents/research/.well-known/agent-card.json
    outbound_secret: a2a_lab_outbound   # vault names (`t --secret-set a2a_lab_outbound`), never the value
    inbound_secret: a2a_lab_inbound
    sessions: ["<local-root-uuid>"]     # local roots whose enabled trees may send to `lab`
    exports: [research]                 # exports `lab` may call
exports:
  research:
    session: "<local-session-uuid>"     # that exact session, not its tree
    name: Research
    description: Answer explicitly submitted research requests.
```

`/a2a reload` validates the whole file (unknown sessions, missing secrets,
duplicate names, non-HTTPS card URLs) and installs all of it or none;
`/a2a` shows the result. The listener is loopback only (`a2a.listen`,
restart to change). For anything beyond same-machine development put an
HTTPS reverse proxy in front of it and set `a2a.public_url` to the base it
publishes: without `public_url` the listener speaks plain HTTP and the
harness treats the deployment as development. Never forward that port to a
network directly. Cards are served at
`<public_url>/agents/<export>/.well-known/agent-card.json` to holders of a
matching inbound credential only; there is no public list of exports. Turning
communication off (globally, or for an exported session) closes its routes at
once; a global off also stops the listener. Setting `a2a.enabled: false` (or
removing the section) and restarting removes the actor entirely.

An exported session is still yours: what you type there never joins a peer's
task, it waits behind one and runs as a private turn (the composer says “Send
privately…” and the notice above it spells this out). The peer's messages can
run the session's tools under its normal approvals, and the model may
summarize anything already in that session into its reply to the peer, so
export a session dedicated to that peer rather than a working conversation.
The only way you take part in a peer exchange is answering a specific pending
request from `/peers` (or `/requests answer <id> <text>` on Slack/Signal).

## Nodes

Give the model, and your phone, another machine. Four lines on the box:

```sh
ssh box
brew install basedatum/tap/theuth         # or `just install` (release `theuth` + its `theuth-app-parser-<hash>` into ~/.cargo/bin), or copy both binaries over
t --secret-set anthropic            # a model key, typed without echo (or piped: `printf %s "$KEY" | t --secret-set anthropic`)
t --install-service --relay wss://relay.example.com   # writes a launchd LaunchAgent / systemd user unit running `$SHELL -l -i -c 'exec t --headless'`, sets link.enabled, starts it, prints a QR + key
```

On your main harness, `/nodes add <key>` in the TUI (or Settings › Nodes ›
Add node in the desktop app, or scan the QR with the phone and choose "as a
node of …"). From then on `spawn_subagent{agent: "<node>"}` runs the child
on that machine over the same encrypted link conn, its approvals land in
your UI, and the desktop and phone switcher list the node nested under its
parent — your devices pair with it automatically, so switching to it shows
its own sessions. `/nodes` lists, renames, removes and invites; `theuth
--uninstall-service` removes the unit and nothing else. The node has its own
`~/.theuth` (config, secrets vault, skills, MCP servers): nothing is synced.
Design: [`docs/design/32-nodes.md`](docs/design/32-nodes.md).
