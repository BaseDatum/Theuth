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

1. **`watch` is not part of widening.** `Grants` (frozen §5.1) has no `watch` field, so a `watch` edit cannot be compared
   against the grant. It is covered by the code hash for plugins that carry one; a read-only plugin restarts.
2. **Code-changed `changed_paths`** come from the files hashed at the last start/approval in this engine run; after an
   engine restart (no per-file hashes in `grants.yaml`) the card lists every code file.
3. **`max_risk` of `call`** is enforced by creating the headless session with `CreateSessionOptions.cap = max_risk`
   (the registry's ceiling) instead of a lookup of the tool's risk in the host; above-cap calls come back as a tool
   error inside `{content, is_error}`, not `-32003`. `-32004` is never produced: a policy deny is likewise an
   `is_error` result of `tools::InvokeHumanOnly`.
4. **`notify` OS notifications** (warn/error) are not sent: only the attention item is filed (`plugin-notify:<name>:<n>`,
   title suffixed `· plugin <name>`). Hand-off: whoever owns the desktop notification path.
5. **Input loop disable** ("three minutes in a row at the limit → disabled") is not implemented; `input_per_min`
   answers `-32005`.
6. **Validation errors back to the plugin** are a host → plugin `log` notification `{level: error, message, path}`
   plus a plugin-log line (design §9.4 wording read literally).
7. **`hello.params.clients`** is `{}` and `engine` is absent (the host has no client registry).
8. **OpenUI `Action` without `activation`** on an `openui` placement is refused with `NotAPress`.
9. **A manifest that turns invalid while running** keeps the process (state `running`, `error` = the issues).
10. The `host::tests::turns_example_end_to_end` unit test of contract §11.2 is `tests/examples.rs`
    (`turns_and_ping_examples_run_through_the_host`), an integration test with a real vault (file key store) and a
    `HumanPress` built from `RpcPeer::open()` — both examples run, a `TurnFinished` reaches turns, the reset action
    and the next surface are checked, and shutdown empties `.pgids`.
11. Sub-builder decisions (route/catalog/ui/manifest/grants/logs/proc): see their notes at the top of each file;
    notably `meeting.events#Deleted` does not drop a session from the meeting set (whole life), revisions survive a
    restart, the stray/long-line reader keeps the first 4 KiB of a discarded line to find its `id`.

## Hand-offs

- Z: OS notification for `notify` at warn/error (deviation 4).
- Z / roster owner: `cargo check -p plugins --features ts` fails in `crates/roster/src/attention.rs` L374/L388/L392
  (`#[ts(optional)]` on `Vec` fields, from commit 2ad29fa6d, before 216), so the plugins `ts` derives are unverified.
- Route `SEED_WAIT` (10 s): frames are held, not dropped, until the meeting set is seeded; with `meeting.enabled`
  on and no meeting actor (tests), the first frames wait 10 s.

## Gates

All under `/tmp/theuth-build.lock`, crate-scoped:

- `cargo test -p plugins --no-fail-fast` — lib 73/73, `tests/examples.rs` 1/1 (turns + ping, 10 s), `tests/scaffold.rs`
  (B2) 8/8.
- `cargo clippy -p plugins --no-deps -- -D warnings` — green.
- `cargo check -p plugins --features ts` — blocked by roster (hand-off above).
- `cargo test -p tui tui_commands_are_reserved_plugin_names` — green (TUI component slash specs added to
  `RESERVED_NAMES`).
