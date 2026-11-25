# 216 — As built: B1b host runtime (`crates/plugins`)

Builder B1b of design 216 (contract `docs/design/216-contract.md` v1; B1 split into B1a — wire/tools/secrets/agent —
and B1b — the host inside `crates/plugins`). Internal module contract: `/tmp/theuth-216/b1b-internal.md` (copied in
spirit below).

## Files

| File | What |
|---|---|
| `src/actor.rs` | `PluginsActor` state (folders, grants, board, inbox, router targets); every §5.2 message; internal messages `Boot`, `FolderChanged`, `ProcessUp`, `ProcessEnded`, `Incoming`, `Retry`, `UiChecked`, `FlushPace`, `ShutdownAll`. The gate (`needed_card`): no verified grant → install card; grant does not cover the manifest → widen card; code hash differs → code-changed card. Pipe requests (`action`, `query`, `command`, `render`) run off the mailbox (`DelegatedReply`). |
| `src/host.rs` | `Paths`, `Conn` (control-lane requests with timeout/cancel, response routing, `fail_all` → `Exited`), `PipeError` (§6.5 codes), `supervise` (env, spawn, `.pgids`, hello, writer/reader/stderr tasks, stray flood → crash, grace then kill), `base_env`, `boot`. |
| `src/manifest.rs` | parse + validation with JSON Pointers, reserved keys, hashes, requested grants, card lines, `check_name`, `RESERVED_NAMES`, `check_offline`. |
| `src/grants.rs` | `grants.yaml` (`GrantsFile`, `GrantEntry`), HMAC-SHA256 sign/verify (name + every field but `mac`), `covers`. |
| `src/logs.rs` | per-plugin daily log, redacted, `log_max_bytes` cap, `tail`, `sweep`. |
| `src/proc.rs` | spawn recipe, two-lane `Outbox` + `write_loop`, `LineReader` (discard to newline), `classify`, `StrayGuard`, `Backoff`, `Pgids`, `reap_orphans`, `stop_tree`. |
| `src/route.rs` | `Router` (external_view → remote drop → grant/sub match → meeting strip → redact → string_cap → stub), `Scope`, `feed_loop`. |
| `src/catalog.rs` | public wire entries split per `type`, plugin event entries. |
| `src/ui.rs` | status/ui parsing, OpenUI validation through `app::helper` with one ToolSnapshot per Query/Mutation (§12.4), masked-input refusal, `Board`, `snapshot` override, `RateLimit`. |
| `src/broker.rs` | `call` (headless session `plugin: <name>`, project `plugins`, risk cap = `max_risk`, `tools::InvokeHumanOnly`), `input` (`ChannelVia::Plugin`), `ping`. |
| `src/ptools.rs` | manifest tools → `ErasedTool` (`ToolSource::Plugin`), gate (`plugin.<name>`) then hints. |
| `src/attention.rs` | the attention `Source` (cards and `plugin-notify:` items). |
| `src/watch.rs` | recursive watcher, 300 ms debounce, `FolderChanged`. |
| `tests/examples.rs` | §15: turns + ping through the real host on a temp data dir. |

## Deviations

STATUS: draft; finished at the end of the run.
