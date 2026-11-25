# 216 — As built: B2 Management

Builder B2 of design 216 (contract `docs/design/216-contract.md` v1): RPC, agent tools, scaffold,
the human press, `t plugin …`, the harness note and the TUI.

## Files

| File | What |
|---|---|
| `crates/plugins/build.rs` | Embeds `sdk/{templates,python,ts,go,rust,ruby,sh}/**` (skips `target/`, `node_modules/`, dot-files) as `$OUT_DIR/sdk_files.rs` → `SDK_FILES: &[(&str, &[u8])]`, paths relative to `sdk/`, sorted. No `sdk/` → empty table. |
| `crates/plugins/src/scaffold.rs` | `new_plugin`: own syntax check of the name (path safety), then `manifest::check_name`; raw only py/sh; plan from `SDK_FILES`; write to `plugins/.<name>.new-<rand>` and rename into place; `run.sh`/`main.*` → 0755. Tests: `every_lang_writes_manifest_and_sdk`, `raw_only_for_py_and_sh`, `placeholder_renamed`, `existing_folder_refused`, `name_rules_enforced`. |
| `crates/plugins/tests/scaffold.rs` | Per language with its toolchain on `PATH`: scaffold, (go/rs: build), `<run> --theuth-manifest` exits 0 within 10 s; SDK templates must print something, raw templates only exit 0. |
| `crates/plugins/src/rpc.rs` | Every method: admission (`owner`, or `HumanPress::from_rpc` for `plugin_approve`), `rpc_core::lookup` (`NOT_RUNNING` when no actor), one `ask`. `rpc_err` adds `data` for `RateLimited`/`Plugin`. `plugin_action` with an activation from the internal peer → `NotAPress`. Defaults: `lang` py, `raw` false, `keep_data` false, `tail` 200 (capped 2000). `plugin_check`/`plugin_install` refuse a relative path (`Invalid`). Tests: `approve_over_internal_peer_is_32002`, `action_with_activation_from_internal_peer_refused`, `errors_carry_data`, `relative_paths_refused`. |
| `crates/plugins/src/press.rs` | Unchanged code; tests `internal_peer_refused`, `guest_refused`, `forwarded_refused`, `local_peer_admitted`, `closed_peer_refused`, `no_peer_refused`. |
| `crates/plugins/src/atools.rs` | The ten tools of §7.1 with the contract's names, risks, args and result texts; one `RegisterPrompt` with the verbatim `PROMPT`. No tool sends `Approve`. Tests: names/risks, arg schemas and defaults, info line. |
| `crates/theuth/src/plugin_cli.rs` | All subcommands of §7.2. `approve` checks stdin+stdout TTY before connecting (exit 2), prints the card, `[y/N]`, approves exactly the card's hashes. `new`/`check` fall back offline. `logs` reads `logs/plugins/<name>.log.<local YYYY-MM-DD>` and `-f` polls 500 ms across midnight. `check --sync` runs `run` + `--theuth-manifest` (5 s, stdin closed) and warns on tools / commands / `on` names plugin.yaml lacks. Tests: arg parsing (no `--yes`), shell words, drift, SDK manifest output parses. |
| `crates/theuth/harness.md` | `## Plugins` after `## MCP`. |
| `crates/tui/src/components/plugins.rs` | `Plugins` component: `/plugins` modal menu (⏎ content, `r`, `d`, `l` 40 lines), status segments in `StatusSlot::AfterCore` coloured by dot (cap 3), plugin commands via `slash_fallback` → `plugin_command`. Data: `plugin_list` + `plugin_surfaces` on mount/engine change/resync and on `PluginState`/`PluginsChanged`; `PluginStatus` patched in place. Tests incl. `tui_commands_are_reserved_plugin_names`. |
| `crates/tui/src/components/{mod,registry}.rs`, `src/model/slash.rs`, `Cargo.toml`, `tests/dependency_limit.rs` | Module + registration; plugin commands join `slash_rows` after built-ins (built-ins win); `plugins` dependency (ratchet list gains `plugins`). |
| `crates/tui/src/model/tests.rs` | `EventDescriptor::of::<KnowledgeChanged>()` instead of a struct literal (B1a request). |

## Deviations from the contract

1. **Rust SDK destination**: the `rs` template says `mod theuth;`, so `sdk/rust/src/lib.rs` is vendored as `src/theuth.rs` (contract table: `theuth-plugin/Cargo.toml`, `theuth-plugin/src/lib.rs`). The SDK file's own header says "Vendored as src/theuth.rs".
2. **Renaming** `presses` applies to template files only; vendored SDK files are copied byte-for-byte (the Rust SDK mentions `presses` in doc examples).
3. **TUI `/plugins` lives in a component** (`components/plugins.rs`), not in `model/slash.rs` `COMMANDS` / `model/menus.rs` `Menu::Plugins` / `model/status.rs`: the TUI moved feature menus, status contributions and slash routing to the components registry; a `COMMANDS` row plus a component would double-route. Plugin commands appear in completion through `Model::slash_rows` and route through `Component::slash_fallback`.
4. `tui_commands_are_reserved_plugin_names` checks `COMMANDS` **and every built-in component's slash spec** (`/mcp`, `/sandbox`, `/plugins`, …), since most TUI commands now live in components.
5. Tool output texts: `plugin_enable` answers `started` for every state but `awaiting_approval`.

## Hand-offs

- **B1**: `manifest::RESERVED_NAMES` must cover every TUI command, including the components' (`crate::components::builtin().slash_specs()`), not only `model/slash.rs` — `cargo test -p tui tui_commands_are_reserved_plugin_names` lists what is missing. `scaffold::new_plugin` calls `manifest::check_name`; until it is implemented every scaffold answers its `NotImplemented`.
- **B1**: the RPC/agent-tool `ask` helpers accept any reply whose `kameo::Reply` is `Ok = R, Error = PluginsError` (so `DelegatedReply<Result<…>>` works).

## Not verified

See the gate list in the final report.
