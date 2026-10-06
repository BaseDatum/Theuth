# 98 — As built: SITE (Telegram on the public site and docs)

Status: as-built · 2026-10-06 · worktree `theuth-site-telegram`, branch `docs/site-telegram`,
base `89beba6f` (main). Brief: `omp` SITE-TELEGRAM. Sources read in the brief's order:
`98-telegram.md`, the `98-as-built-*` notes, then the code; every `path:line` below was read
on `89beba6f`. Where the design and the code disagree the code is what the docs say (see
"Code vs design" at the end).

## Files changed (all under `site/`)

| file | change |
|---|---|
| `content/docs/automation/channels.mdx` | title/description/intro for three channels; Telegram prerequisites bullet; Telegram column in the comparison table; routing paragraph says Telegram does **not** route by first word; "Several engines" gains the hub-lease poll; new walkthrough *Set up Telegram and pair the owner* (setup transcript, pairing alternatives, groups, "How replies look"); `telegram:` config table; Telegram commands section; CLI/tools/desktop row; safety bullets; Telegram limits list; "Not available"; troubleshooting rows; Related |
| `content/docs/automation/agents.mdx` | routing paragraph (~L106) states Telegram sends `name:` as plain text; tool-ceiling bullet; Related link line |
| `content/docs/automation/{index,delegation,mesh,sandboxes,schedules,tasks,watchers}.mdx` | "Slack buttons, Signal 👍/👎" → "Slack or Telegram buttons, Signal 👍/👎"; channel lists name Telegram; `config.yaml` section list and data-dir table gain `telegram`; delegation gets a Telegram row (topics per subagent, board, `/agents` buttons) and the `--acp` row names Telegram |
| `content/docs/reference/cli.mdx` | description; `t --headless` row; new `t --telegram-setup` row; "`/status` exists on Signal and Telegram"; new **Telegram commands** section; closing line |
| `content/docs/reference/configuration.mdx` | data-dir row gains `telegram/`; written-keys table gains `--telegram-setup` and the pairing/allowlist writers; keys table section `signal, slack, telegram, sessions` with three `telegram.*` rows |
| `content/docs/reference/tools-permissions.mdx` | `message_user` note; tool groups gain `telegram_status` / `telegram_link`; `/approvals` paragraph; Answering table gains a Telegram row; unbound-approvals paragraph |
| `content/docs/reference/{index,development,mcp,secrets,skills,troubleshooting}.mdx` | one-line parity mentions |
| `content/docs/getting-started/models.mdx` | `/model` in a Telegram topic changes only that session |
| `content/docs/workspace/{apps,desktop,documents,sessions,voice}.mdx` | parity mentions; desktop Settings › Channels describes the Telegram card |
| `src/App.tsx` | `FLAGS` gains `t --telegram-setup`; `SURFACES` "Away from it" gains a Telegram entry; `LEDGER` gains a `telegram` row after `signal`; mesh bodies name the Telegram poller; `message_user`, `ask_user`, `app`, `visualize`, `broadcast` bodies name Telegram; `NOT` is now `Discord or WhatsApp`; the two prose paragraphs (frontends list, approvals) name Telegram |

No file outside `site/` was touched except this note. `meta.json` files are unchanged (sidebar labels come from frontmatter titles).

## Claims added, with the code that backs them

Lines are `crates/…` unless stated. "verbatim" = the string appears in the docs as the code prints it (Markdown bold stripped inside fenced examples).

| claim in the docs | evidence |
|---|---|
| **Setup, secret, config** | |
| token is the `telegram_bot` secret, never in `config.yaml` | `telegram/src/lib.rs:48-49` `SECRET_TOKEN = "telegram_bot"`; `theuth/src/main.rs:1128-1136` default-config comment |
| `t --telegram-setup` flag; refuses while an engine runs: `daemon running: use /telegram link or the telegram_link tool` | `theuth/src/main.rs:70-73`, `:1305-1306`; `telegram/src/setup.rs:49-50`, `:66-69` |
| setup transcript lines (verbatim): `theuth telegram setup`, `1. bot token: @BotFather → /newbot …`, `bot token: `, `   @<bot> · topics on|off`, `1. bot token ok (existing)`, `2. token saved to the vault as telegram_bot`, `3. commands registered`, `4. open this from the owner's Telegram and press Start (valid 10 min):`, `   paired with <name> (<id>)`, `5. telegram.owner = <id>, telegram.enabled = true`, `groups: add the bot as an admin with Manage topics (…)`, `telegram ready: run t --headless and message the bot` | `setup.rs:65`, `:197-199`, `:201`, `:96-100`, `:80`, `:119`, `:128`, `:143-147`, `:151`, `:166-171`, `:172` |
| token checked with `getMe`, three attempts | `setup.rs:185-206` |
| Topics-off hint: @BotFather → `/mybots` → the bot → Bot Settings → Topics | `setup.rs:101-105` |
| re-run: `re-pair? [y/N]`, owner kept unless `y` | `setup.rs:131-139` |
| writes `telegram.owner`, `telegram.enabled: true`; registers owner command scope | `setup.rs:157-165` |
| pairing: deep link `https://t.me/<bot>?start=<code>`, 16-char code, 10 min, single-use, first `/start <code>` becomes owner, late/wrong code silent | `lib.rs:51-55` (`PAIR_TTL = 600 s`, `CODE_LEN = 16`), `setup.rs:176-183`; `channel.rs:712-724` (`take_pairing`), `:1023-1031`, `:1099-1108` |
| welcome `👋 paired: **<name>** is the owner now.` + help | `channel.rs:1521-1543` |
| owner applies live and is removed from `allowed` | `channel.rs:584-587`; `lib.rs:415-423` |
| Settings › Channels › Telegram: Bot state, **Bot token**, **Owner** + **Pair**/typed id, **Allowed users**, **Groups**; saving the token starts the channel; countdown on the link | `desktop/ui/src/components/settings/Channels.tsx:355-470`, `:323-329`; hot start `lib.rs:436-451` |
| `/telegram link` for re-pairing; `telegram_link` tool, `telegram_link{owner}` sets the id directly | `commands.rs:76-80`, `:646-675`; `lib.rs:661-699`; `channel.rs:1712-1725` |
| `telegram_link` in terminal mode: `pairing needs the daemon: run t --headless, or pass the owner id` (reason the docs say "with the daemon already running") | `lib.rs:496` |
| `telegram:` keys and defaults: `enabled true`, `owner` unset, `allowed []`, `groups []`, `tools all|errors|none`, `reasoning true`, `approvals all|threads` | `lib.rs:59-89`, `:93-113` |
| env override `THEUTH_TELEGRAM__<KEY>` | `theuth/src/main.rs:1236-1237` (`Env::prefixed("THEUTH_").split("__")`) |
| `owner`/`allowed`/`groups` written back live from chat, desktop, RPC; `tools`/`reasoning`/`approvals`/`enabled` need a restart | `lib.rs:400-404`, `:506`, `:541`, `:555`, `:569`, `:423`; cfg captured at start `lib.rs:581-588`, `channel.rs:909-912` |
| replacing the token under a running channel applies at next restart | `lib.rs:438-443` |
| `telegram_status` strings: `unconfigured (secret telegram_bot missing; run t --telegram-setup)`, `daemon only (runs in t --headless)`, `disconnected: hub lease held elsewhere`, `conflict: another poller uses this token`, `stream fallback for <session>: <reason>`, `chat <id> unreachable: <reason>`, `topics on|off` | `lib.rs:741-758`; `channel.rs:657-659` |
| RPC `telegram_status`, `telegram_link(owner?)`, `telegram_allow`, `telegram_remove_allowed`, `telegram_allow_group`, `telegram_remove_group` | `rpc/src/api.rs:1097-1115` |
| `t --secret-set telegram_bot` manual path | `theuth/src/main.rs:1129`, `:1289-1294` |
| **Trust and routing** | |
| who may talk: owner + `telegram.allowed`; stranger DM dropped, log `dropping telegram message from a user not in the allowlist` | `channel.rs:589-591`, `:1047-1053` |
| unlisted group ignored; non-allowed member addressing the bot in a listed group → ephemeral `not allowed` | `channel.rs:1035-1046`, `:1485-1502` |
| bots and broadcast channels ignored | `channel.rs:1019-1021` |
| nothing auto-added; only pairing changes the owner | `channel.rs:352-357`, `:1751-1754`, `:1508-1509` |
| thread key `<chat>` / `<chat>:<topic>` | `threads.rs:13-23` |
| topics shape from `getMe` (private) / `is_forum` (groups); identity read once at start (hence "restart" in troubleshooting) | `channel.rs:934-940`, `:953-954`, `:476-501` |
| General message → session first, then a topic named from the first line, bound | `channel.rs:1215-1219`, `:1296-1313`, `:2422-2430` |
| follow-up steers a running turn, else next turn | `channel.rs:1264-1281` |
| `/new`: `new thread: the next message starts a session`; `/resume` from General creates a topic named after the title | `commands.rs:329-342`; `channel.rs:1605-1650` |
| owner roots `owner` scope, others `isolated` | `channel.rs:1253-1257`; `port.rs:72-93` |
| **Telegram does not route by first word** | `roster/src/routing.rs:17-31` has two callers: `signal/src/channel.rs:1294`, `slack/src/channel.rs:1247`; no `routing`/`RoutableAgents` in `crates/telegram`; `telegram/src/port.rs:72-93` sends text verbatim |
| plain group: `@mention`, `/command` or reply-to-bot; mention stripped | `channel.rs:1196-1198`, `:1222-1226`, `:2432` |
| forum group: a bound topic takes every message | `channel.rs:1223-1226` |
| group command replies ephemeral, fallback to a normal send | `channel.rs:1785-1812` |
| session commands without a session: `no session yet: send a message first` + **Sessions** button; in General they act on the latest topic | `commands.rs:122`; `ui.rs:331-335`; `channel.rs:1558-1563` |
| bot added to a group → log `telegram bot added to chat` (with `chat=<id>`); removed → bindings dropped, group stays in config | `channel.rs:986-998` |
| **Mesh** | |
| polling is a hub-lease duty; non-hub reports `hub lease held elsewhere` | `channel.rs:279-280`, `:619-663` |
| offset confirmed on stop; handover answered once (`tasks::MarkAnswered`, `update_id` dedup) | `poll.rs:81-82`, `:140-142`; `channel.rs:1082-1096`; `poll.rs:105-109` |
| 409 → `Conflict`; `deleteWebhook` at start | `poll.rs:120-127`; `channel.rs:500-502` |
| **Rendering** | |
| live draft in private chats, edit mode in groups | `bubble.rs:127-131` |
| native **Stop** (`can_stop`) → cancel | `bubble.rs:230`; `poll.rs:386`; `worker.rs:355-365` |
| `thinking · N s` indicator in the draft | `bubble.rs:91-92` |
| persisted Markdown at block boundaries: first tool call, end of turn, size cap | `worker.rs:566-589`; `bubble.rs:303-330` |
| keepalive 25 s; rotation 30 000 bytes at a block boundary, fence reopened; plain fallback 4 096 chunks | `bubble.rs:17-21`, `:355-370`, `:378-386` |
| one activity card per batch, `<details>` per call, `✓ shell · … · 1.2 s`, 2 500 chars, **full output** button → `<call id>-output.txt`, 1 h TTL, `output expired`; summary 256 | `cards.rs:13-19`, `:245-270`; `channel.rs:2011-2040`; `worker.rs:163` |
| reasoning as expandable quote; `reasoning: false` keeps the indicator | `bubble.rs:78-82`, `:160-166`; `worker.rs:421` |
| footer: model · tokens · time · tools | `bubble.rs:454-475` |
| reactions 👀 → 👌 / 🤷 / 😱 | `worker.rs:170-175`, `:529-548`; `channel.rs:59-61`, `:1238`, `:1332-1333` |
| typing indicator | `worker.rs:161`, `:393`, `:658-663` |
| topics renamed to the session title (≤ 128), `↳ <name> · <title>` for children, unless user-named; closing a topic unbinds | `worker.rs:604-640`, `:167`; `channel.rs:1057-1066` |
| **Approvals and asks** | |
| card `⚠️ **shell** wants to run` + fence; **Allow** / **Deny**; `✓ **shell** allowed by <name>`, `✗ … denied`, `⌛ … timed out`; one disabled verdict button; args 600 | `approvals.rs:12`, `:44-58`, `:60-80`, `:85-110` |
| who may press: owner or the thread's user; `not allowed`, `already resolved` toasts | `channel.rs:1838-1865` |
| unbound sessions → owner chat with `session <last 8> · <title>` when `approvals: all`; subagents → own topic or parent's chat (no ancestor walk) | `channel.rs:2172-2207`, `:838-860` |
| `ask_user`: `❓ **Question i/n** — …`, ≤ 8 buttons per row, **Skip**, **Dismiss** (cancels the turn), typed reply via `force_reply` | `asks.rs:16`, `:160-192`, `:244-264`; `channel.rs:1946-1960`, `:1120-1129` |
| ask timeout `tools.ask_timeout_secs` 1 800 s (already on the page) | `tools/src/lib.rs:455-456`, `:484` |
| secrets notice to the owner; value never read from chat | `channel.rs:73-77`, `:2152-2163` |
| **Commands** | |
| native `setMyCommands`; three scopes; group set `help status new sessions resume cancel compact model agents`, ephemeral | `commands.rs:115-117`, `:141-162`; `channel.rs:685-692` |
| the command table (names and args) | `commands.rs:25-102` |
| owner-only set and `This command is restricted to the owner.` | `commands.rs:104-112`, `:120` |
| `/telegram [status|link|allow <id>|rm <id>|group add <id>|group rm <id>]` | `commands.rs:76-80`, `:646-675` |
| panels with buttons: Sessions **Resume**, Agents **Cancel** / **Close topic**, Models, Skills/MCP **Sync**/**Add…**, Login, Approvals three states, Status **Refresh** | `ui.rs:36-77`, `:320-335`, `:497-529` |
| `/a2a reload` answers that it needs the terminal (`a2a reload is not available from Telegram yet; use the terminal`) | `commands.rs:1255-1258` |
| no `/approve` / `/deny` | absent from `commands.rs:25-102` |
| **Files and voice** | |
| inbound ≤ 20 MB → `telegram/inbox/`; over → `[attachment: … — too large for the Bot API (20 MB); not downloaded]`; `[attachment: <path> (<mime>, <size>)]`; images and PDFs as inputs | `channel.rs:45`, `:1389`, `:1409-1417`, `:1472-1478` |
| voice/video notes/audio → `[voice note, N s] …`; `(transcription unavailable)` | `channel.rs:1440-1468`; `voice/src/lib.rs:132-135` |
| stickers/locations/contacts as text lines; albums within 1 s merged; `(replying to: "…")`, `(forwarded from …)` | `channel.rs:1375-1402`, `:48-49`, `:1152-1189`, `:1346-1356` |
| typed message ends voice mode | `worker.rs:348-352` |
| outbound: photo ≤ 10 MB, MP3 → voice bubble, else document; artifacts > 50 MB → note | `api.rs:520`, `:535-543`; `worker.rs:272-276`; `channel.rs:47`, `:2339-2346` |
| `message_user` `via: "telegram"`; 100 MiB cap (shared) | `telegram/src/user.rs:15-18`; `agent/src/user.rs:24-25`, `:144-147` |
| `show` / `message_user` land in the bound thread (walk-up for subagents), else owner chat | `telegram/src/user.rs:27-43`; `channel.rs:2326-2338` |
| **Pacing and polling** | |
| 1 s per private chat, 3 s per group, drafts 1 s, edits 1.5 s, 30 calls/s; 429 retried twice then counted | `api.rs:304-309`, `:385-387`, `:437-451` |
| 100 updates per 30 s long poll; backoff 1 s → 60 s | `poll.rs:26-28`, `:73-75` |
| **Homepage copy** | |
| "topic per session", "live draft with a Stop button", "Allow and Deny buttons", "eyes while working, an OK when done", "owner-only /broadcast", "files land in an inbox", "the Telegram poller" is a hub duty | rows above (`channel.rs:1296-1313`, `bubble.rs:230`, `approvals.rs:46-53`, `worker.rs:170-175`, `commands.rs:97-112`, `channel.rs:1389`, `channel.rs:619-663`) |
| `t --acp` loads Telegram without its channel (status only) | `98-as-built-WIRE.md` (`TelegramPlugin { channel: false }` in the `--acp` vector); `lib.rs:468-472` `DaemonOnly` |

## Not verified / left out

- **Telegram's own limits** (32 768-character rich messages, 73-emoji reaction set, the 30 s
  draft lifetime as a platform number, 64-byte `/start` payload): design-only. The docs quote
  only the crate's constants (30 000-byte rotation, 25 s keepalive, 4 096 plain chunks).
- **Why 👌/🤷 instead of ✅/🛑** (the fixed reaction set): a platform fact, not in code — the
  docs state the emoji without the rationale.
- **Privacy mode** hides plain group messages from a non-admin bot: a Telegram platform
  behaviour. Kept one sentence because the setup hint `setup.rs:170` prescribes the fix
  (`@BotFather /setprivacy → Disable, then re-add it`); the mechanism itself is unverified.
- **Finding a group's chat id**: the setup hint says `/status shows the id once the bot is in
  the group`, but `/status` renders `TelegramStatus.groups` (the configured list,
  `lib.rs:782-786`, `ui.rs:342`), and an unlisted group's `/status` is ignored
  (`channel.rs:1035-1041`). The docs point at the `telegram bot added to chat` log line
  (`channel.rs:994-997`, field `chat`) instead and do not repeat the hint's claim.
- **Live rendering** of `<tg-thinking>`, `<footer>`, `<tg-button>`, `<blockquote expandable>`,
  drafts inside private topics, reactions in groups without `can_react_to_messages`, ephemeral
  group replies: never exercised against a real bot (`98-as-built-INTEGRATE.md` "Not
  exercised"). The docs describe what the code sends, not what Telegram was seen to draw.
- **A `--owner <id>` CLI flag**: does not exist (`main.rs:70-73`); the brief's "or by numeric
  owner id" is documented through the desktop **Owner** field and `telegram_link{owner}`.
- **Hand-editing `telegram.allowed` / `groups` while the daemon runs**: not picked up until
  restart (no watcher); the page's existing sentence "everything else needs a daemon restart"
  covers it, nothing Telegram-specific added.
- **Plan mode** (`TodoChanged`): no channel renders it; nothing said.
- Docs do not claim a Telegram-specific `message_user` file cap; the shared 100 MiB cap is the
  agent's (`agent/src/user.rs:24-25`), and a Bot API refusal above 50 MB would surface as an
  error — untested, so only the `show` artifact cap (50 MB, in code) is stated.

## Code vs design (where the docs follow the code)

- Rotation is 30 000 **bytes** (`bubble.rs:17`), not 32 768 characters.
- Typing is sent at `TurnStarted` and then only after 4 s of silence (`worker.rs:658-663`),
  not "every 4 s"; the docs say "a typing indicator".
- The General area of a topics chat takes plain messages (session first, then topic), not
  "commands and new sessions only".
- Approval timeout text is `⌛ **tool** timed out` (no "(denied)").
- Subagent approvals have no ancestor walk (own thread or flat slot, else owner chat / nowhere);
  only `show` and `message_user` walk up.
- No first-word agent routing on Telegram (design §4.7 parity claim does not hold for desks;
  CONTRACT note: "Telegram has no desks").

## Gates

`cd site && bun install && bun run build` (twice; second run's tail):

```
$ bun install
337 packages installed [1056.00ms]
$ bun run build
$ tsc -b && bun scripts/check-docs.ts
docs ok: 34 pages
…
Prerender (data): /api/search -> build/client/api/search.data
Prerender (resource): /api/search -> build/client/api/search
Prerender (html): /docs/automation/channels -> build/client/docs/automation/channels/index.html
…
Prerender (html): SPA Fallback -> build/client/__spa-fallback.html
dist ok: 34 docs pages, sitemap.xml, api/search
```

```
$ grep -rn -i "telegram" site/content site/src | wc -l
138
$ ls site/dist/docs/automation/channels
index.html
$ grep -o "Channels — Slack, Signal and Telegram" site/dist/docs/automation/channels/index.html | head -1
Channels — Slack, Signal and Telegram
$ grep -c -i telegram site/dist/docs/automation/channels/index.html
48
$ grep -l "Discord or WhatsApp" site/dist/assets/*.js        # the homepage is a hydrated shell; copy lives in the bundle
site/dist/assets/home-_BhJeNV2.js
$ grep -c "Telegram, Discord" site/dist/assets/*.js | grep -v ":0"
(none)
```

`site/dist/` and `site/build/` are gitignored (`git status --short` → clean after the build). No
Tauri, no cargo, no network beyond `bun install`.

## Commits (`docs/site-telegram`)

- `604d353a` docs(site): channels page — Telegram as the third channel
- `00c43dd2` docs(site): name Telegram beside Slack and Signal across automation, reference and workspace pages
- `94c99fab` docs(site): homepage — Telegram as a surface, ledger entry, out of the not-yet list
- (this note) docs(site): as-built note for SITE-TELEGRAM
