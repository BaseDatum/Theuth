# cc-version: the Claude Code client version, live

## Cause

An Anthropic subscription (OAuth login) request claims to be Claude Code:
`User-Agent: claude-cli/<version> (external, cli)`, `x-app: cli`, the
`anthropic-beta` list `claude-code-20250219,oauth-2025-04-20,interleaved-thinking-2025-05-14`.
The version was a compiled constant, `2.1.257`. On 2026-11-18 Anthropic began
refusing it for new models:

```
400 {"type":"error","error":{"type":"invalid_request_error","message":"Claude Code 2.1.257 does not support this model; version 2.1.280 or newer is required. …","details":{"error_code":"claude_code_version_too_old"}},"request_id":"req_…"}
```

The compiled default is now `2.1.327` (npm `latest` that day), and the
version no longer needs a rebuild to change.

## Body shape (`crates/agent/src/claude_code/wire.rs`)

Headers alone are not enough. On 2026-11-18 every theuth subscription call
got `429 {"type":"rate_limit_error","message":"Error"}` with the plan at 2 %
of its week, while Hermes Agent, sending the same headers from the same
account, was served. The difference was the body: theuth joined the identity
sentence and its own prompt into one system string and sent bare tool names
(`python`, `read_file`); Hermes sends the identity as its own first system
block and names every tool `mcp__<name>`. Anthropic bills a body that does not
look like Claude Code's as a third-party app (extra usage; none left → 429).

`Wire::outgoing` runs on every subscription call in `Backend::stream` (the
one place model calls leave, so titles, compaction, the advisor and contact
sessions get it too) and does:

1. The identity as a separate first system message (`system`). A request
   built with `ChatRequest::system` has it moved after the identity. When no
   system message carries a cache mark, the last one gets `Ephemeral`, since
   genai joins unmarked system messages into one string.
2. `system_replace` pairs, applied to the system text, in order (none by default).
3. `tool_prefix` (`mcp__`) on every tool name lacking it, in the tool list
   and in replayed tool calls/results. `Names::event` restores names on the
   stream: the offered tool's own name, else the prefix stripped (a runtime
   function called directly is still bridged).

Each rule is a `claude-code.json` field, live like `betas`:

```json
{ "system": "You are Claude Code, Anthropic's official CLI for Claude.",
  "tool_prefix": "mcp__",
  "system_replace": [["You are theuth, a meta-harness", "You are a harness"]] }
```

`""` turns `system` or `tool_prefix` off. A heal or npm update rewrites the
version and keeps every other field.

### If this stops being enough: what oh-my-pi sends

oh-my-pi (`packages/ai/src/providers/anthropic.ts`, `anthropic-headers.ts`,
`anthropic-identity.ts`, `claude-code-fingerprint.ts`) mirrors Claude Code
far more closely and is maintained against it; it is the reference for the
next requirement. Beyond our rules it sends:

- system[0] = `x-anthropic-billing-header: cc_version=<ver>.<sha256 suffix>;
  cc_entrypoint=cli; cch=<xxhash64 of the serialized body>;`, before the
  identity block (system[1]). The `cch` hash covers the final body bytes, so
  it needs a genai hook after serialization.
- tool prefix `_` (not `mcp__`), except Anthropic's built-ins (`web_search`,
  `code_execution`, `text_editor`, `computer`). Either prefix passes today;
  set `tool_prefix` to `_` to follow omp.
- `metadata.user_id`: a JSON string carrying `device_id`, `account_uuid` and
  `session_id`.
- the `X-Stainless-*` headers, `X-Claude-Code-Session-Id`, and a longer beta
  list (`context-management-2025-06-27`, `prompt-caching-scope-2026-01-05`,
  `effort-2025-11-24`, `fallback-credit-2026-06-01`, …). Betas are already a
  `claude-code.json` field.
- never `context-1m-2025-08-07`: a subscription without long-context credit
  gets a hard 429 ("Usage credits are required for long context requests").

## Sources and order (`crates/agent/src/claude_code.rs`)

1. Compiled default `DEFAULT_VERSION` = 2.1.327, betas `DEFAULT_BETAS`.
2. `~/.theuth/claude-code.json` (data dir), re-read when its mtime changes;
   the stat happens at most every 3 s, on the request path, under a mutex
   (no file read per request).
3. A version learned in this process (a heal or npm), also written to the file.

The newest of these wins. A file with `"pin": true` and a valid `version`
wins outright, even below the default; heals and npm then leave the file alone
(a heal against a pin logs a warning naming the required version). `betas`, when
present and non-empty, replaces the compiled beta list.

The resolved identity is read in the genai service-target resolver
(`supervisor::client`), i.e. per request.

## Heal path

`backend::provider_error` is the one place every genai error becomes text.
When the text carries `claude_code_version_too_old`, `Store::heal` parses
`version X.Y.Z or newer`; if X.Y.Z is newer than the current version (and
nothing is pinned) it adopts max(X.Y.Z, npm `latest` if fetched within the last
minute), writes the file with `source: "auto: <request_id>"`, logs at info, and
the error text gains `theuth updated its Claude Code client version to X; resend.`

The turn loop (`Turn::call`, beside the strip-reasoning and overflow resends)
resends the call once, at once, when nothing streamed and the refused version
(`Claude Code A.B.C does not support`) is older than the current one. At most
one such resend per request: a second refusal is the turn's error. This also
covers a request that raced another one's heal (refused 2.1.257 while the file
already says 2.1.400). Detached side calls (`loop_/side.rs`: titles, vision …)
do not resend; their error says to resend, and their next request uses the new
version.

## npm refresh

`claude_code::start` (from `supervisor::start`) binds the file and spawns a
task: at startup and then every 24 h, while `auth.login("anthropic")`, GET
`https://registry.npmjs.org/@anthropic-ai/claude-code/latest` (5 s timeout,
failures ignored). A newer, unpinned version is written with `source: "npm"`;
never a downgrade. No theuth setting gates outbound fetches of this kind (the
usage-limit pollers fetch the same way), so none is consulted. The 24 h sleep
is tokio's monotonic clock: a sleeping Mac delays it; a restart always fetches.

## File format

```json
{
  "version": "2.1.327",
  "betas": "claude-code-20250219,oauth-2025-04-20,interleaved-thinking-2025-05-14",
  "source": "npm",
  "checked_at": "2026-11-18T14:02:11.123+00:00",
  "pin": false
}
```

All fields optional. An unreadable file logs a warning and counts as absent.

## Fixing it by hand

Edit `~/.theuth/claude-code.json`, e.g. `{"version": "2.1.400"}`; the next
request (within 3 s) uses it, no restart. To force an exact version or beta
list, add `"pin": true`; remove the pin to let heals and npm raise it again.

## Other Anthropic OAuth callers

- `limits/poll.rs` (usage poll) sends only the bearer and `anthropic-beta: oauth-2025-04-20`: no Claude Code UA.
- `oauth` token refresh sends `User-Agent: anthropic-sdk-typescript/0.112.1 userOAuthProvider`: an SDK string, not a Claude Code version; left as is.

## Codex / ChatGPT

The ChatGPT override sends `originator: codex_cli_rs` with no version, so it
has no hard-coded version to go stale today. If OpenAI ever gates on a Codex
version header, the same pattern (file + heal) would apply; not done.

## Not done

The Settings › Providers row does not show the client version: it would need
an RPC type change.
