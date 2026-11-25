# 216 — As built: B0 skeleton

Builder B0 of design 216 (contract `docs/design/216-contract.md` v1). Everything below compiles; nothing behaves yet: every
actor message answers `PluginsError::NotImplemented` (the lists answer empty), every RPC method answers after admission
with `NotImplemented` (`-32000`, "`<method>`: not implemented yet (216)"), `t plugin …` bails "t plugin: not implemented
yet (216 B0)".

## Files

### `crates/plugins/` (new crate)

| File | What | Owner now |
|---|---|---|
| `Cargo.toml` | contract §3.1 verbatim; `tempfile = "3"` (no workspace entry); every crate in the `ts` list has a `ts` feature | frozen (Z) |
| `src/lib.rs` | §3.2 module list, re-exports, consts, `declare_wire`, `PluginsPlugin` (§3.3 behaviour); tests `config_defaults_match_the_contract`, `both_topics_declare`, `wire_json_shapes` | frozen |
| `src/config.rs` | `PluginsConfig` + `Default` (§4) | frozen |
| `src/types.rs` | every §5.1 type, `ts` derive behind the feature | frozen |
| `src/messages.rs` | every §5.2 message | frozen |
| `src/events.rs` | `PluginEvent`, `PluginUi` + `WireEvent` (§5.3) | frozen |
| `src/error.rs` | `PluginsError`, `Display`, `code()` (§9.1); test `codes_match_rpc_core` | frozen |
| `src/press.rs` | `HumanPress` complete (§8) | B2 |
| `src/actor.rs` | `PluginsActor { rt, cfg }` (`#[derive(kameo::Actor)]`), `spawn`, one `impl Message` per message | B1 |
| `src/rpc.rs` | full `PluginsApi` (19 methods), `rpc_err`, stub impl, `rpc_module` | B2 |
| `src/host.rs` | `boot` stub (logs at debug, returns) | B1 |
| `src/manifest.rs` | `RESERVED_NAMES` (empty), `check_name` (answers `NotImplemented`), `check_offline` (a report with one "not implemented" error) | B1 |
| `src/{attention,broker,catalog,grants,logs,proc,ptools,route,ui,watch}.rs` | one `//! Owner: B1 — …` line each | B1 |
| `src/scaffold.rs` | `new_plugin` stub (`NotImplemented`) | B2 |
| `src/atools.rs` | `register` (does nothing) | B2 |

### Outside the crate

| File | Edit |
|---|---|
| `Cargo.toml` | `plugins = { path = "crates/plugins" }` after `webprofile` |
| `crates/theuth/Cargo.toml` | `plugins = { workspace = true }  # 216` |
| `crates/theuth/src/main.rs` | the seven §2.1 spots: `mod plugin_cli;`, `Cmd::Plugin`, the `run()` arm, the `cmd_67` `unreachable!`, `plugins::rpc_module` after `webprofile::rpc_module`, `Box::new(plugins::PluginsPlugin)` after `share::SharePlugin::engine()`, the `# plugins:` block after `# sandbox:` |
| `crates/theuth/src/plugin_cli.rs` | `PluginCmd`, `LangArg` (§7.2), stub `run` |
| `crates/rpc-compose/Cargo.toml` | `plugins = { workspace = true }` |
| `crates/rpc-compose/src/api/tests.rs` | `plugins::rpc_module(server.clone()),` before `sandbox::rpc_module(server)` |
| `crates/rpc-compose/src/client_mirror.rs` | `plugins::PluginUi, plugins::PluginEvent,` last in `republish!` |
| `crates/roster/src/attention.rs` | `AttentionKind::Plugin` last; id doc gains `plugin:<name>:<m8>:<c8>:<seq>` |
| `crates/roster/src/alert.rs` | exhaustive matches: `class_of` → needs-you for `plugin:` ids, nothing for `plugin-notify:`; `why_of` → "a plugin waits for your approval" |
| `crates/roster/src/score.rs` | exhaustive matches: `base_class` → `Yours` for `plugin:`, `Notice` for `plugin-notify:`; `base_text` → "plugin approval" / "plugin notice" |
| `crates/agent/src/communication.rs` | `ChannelVia::Plugin` last; `ChannelOrigin.via` doc |
| `crates/tools/src/lib.rs` | `ToolSource::Plugin { name, version }` last |
| `crates/tools/src/registry.rs` | `ToolSource::Plugin { .. } => None` in the hide-key match (B1 maps it to `plugin.<name>` per §5.6) |
| `crates/desktop/ui/src/components/icons/channels.tsx` | `PluginChannelIcon` (lucide `Puzzle`) as `ICONS.plugin` |
| `crates/desktop/ui/src/components/attention/{labels.ts,Screen.tsx,model.ts}` | `KIND_LABEL.plugin`, `TITLE.plugin`, `SCREEN.plugin = HomeScreen`, `'plugin'` last in `KIND_ORDER` |
| `crates/desktop/ui/src/components/plugins/{icons.tsx,PluginDot.tsx,state.ts}` | §12.3, complete |

## Deviations from the contract

1. `PluginsError::not_implemented(what: &str) -> Self` added to `error.rs` (a constructor the stubs share; additive).
2. `PluginsActor` holds `rt` and `cfg` marked `#[allow(dead_code)]` until B1 reads them; B1 may change its fields freely (`actor.rs` is B1's).
3. `HumanPress::for_test` carries `#[allow(dead_code)]` (nothing in B0 calls it).
4. `PluginCmd` carries `#[allow(dead_code)]` until B2's `run` reads the fields; B2 removes it.
5. `types.rs` does not import `runtime::SessionId` (no type there uses it; it would be an unused import).
6. Roster's exhaustive matches (`alert.rs`, `score.rs`) needed `Plugin` arms; the semantics above are B0's choice (B1 may refine; both files are roster's, not in any builder's row — hand-off to Z if B1 wants other wording).
7. `PluginIcon` draws an `svg:` icon only when the rest is a `data:image/svg+xml` URL; anything else falls back to `Puzzle`.
8. `rpc.rs` carries a module-level `#![allow(clippy::too_many_arguments)]`: `plugin_action`'s seven parameters push the jsonrpsee-generated server/client methods past clippy's limit, and the macro output takes no item attribute.
9. Contract §11.1 item 7 says 19 `plugin_*` methods; §6 declares 18 (list, get, check, install, new, approve, set_enabled, set_hidden, set_order, restart, uninstall, action, query, command, surfaces, surface, catalog, logs). The bindings hold those 18.
10. The first `just wire-schema-accept` was not done by B0 (orchestrator ruling): B1/B2 began editing the shared tree (B1a's `exposure` keys in `rpc-core/src/discovery.rs`) before B0's gates ran, so an accept from the working tree would have baked B1a's in-flight values into the fixture. B1a owns the first accept.

## Gates (contract §11.1)

Run in the shared worktree, each under `/tmp/theuth-build.lock`:

| Gate | Result |
|---|---|
| `cargo check -p plugins -p theuth` | green on the B0 tree (no warnings) before the other builders started; first run found the roster exhaustive matches, fixed |
| `cargo clippy -p plugins --no-deps -- -D warnings` | first run: 2 × `too_many_arguments` from the `rpc` macro; fixed (deviation 8) and committed in `4a8f8e819`; not re-run by B0 (the crate then held B1/B2 work in progress) |
| `cargo test -p plugins` | not observed green: by the time it ran, B2's in-flight `scaffold.rs` (`OUT_DIR`/`SDK_FILES` before `build.rs` was wired) failed the crate |
| `cargo check -p rpc-compose --tests` | not observed green: the `plugins` crate had 23 errors from in-flight B1/B2 files at that moment |
| `just wire-schema` / snapshot / accept | not run by B0 (deviation 10) |
| `bun install` (desktop/ui, packages/apps, packages/documents) | green, no changes |
| `just ts-desktop` | green; `bindings/openrpc.json` holds 18 `plugin_*` methods |
| `bun run check` in `crates/desktop/ui` | green (0 errors) against the tree as of the regen, B3/B4 checkpoints included |
| `t --help` shows `plugin` | not run (no `cargo build`); `Cmd::Plugin` compiles in `cargo check -p theuth` |

## TS surface (verbatim)

Files in `crates/desktop/ui/src/rpc/bindings/`: `method.plugins.input.ts`, `method.plugins.output.ts`, `method.plugins.receiver.ts`, `event.plugins_2eui.output.ts`, `event.plugins_2eui.receiver.ts`, `event.plugins_2eevents.output.ts`, `event.plugins_2eevents.receiver.ts`.

`method.plugins.output`: `Badge BindingKind CallGrant Capabilities CardKind CatalogEntry CheckIssue CheckReport CommandInfo CommandReply CommandUi Contract Dot Fallback GrantKey GrantLine Grants InstallCard InstallReport Lang LogTail OpenMode PlacementInfo PlacementKind PluginActionResult PluginApproveResult PluginCatalogResult PluginCheckResult PluginCommandResult PluginDetail PluginGetResult PluginInfo PluginInstallResult PluginListResult PluginLogsResult PluginNewResult PluginQueryResult PluginRestartResult PluginRunState PluginSetEnabledResult PluginSetHiddenResult PluginSetOrderResult PluginSurfaceResult PluginSurfacesResult PluginUninstallResult Risk ScaffoldReport Scoped Size SurfaceBinding SurfaceContent SurfaceFormat SurfaceLoad SurfaceSnapshot ToolGrant`

`method.plugins.input`: `Contract Lang PluginActionParams PluginApproveParams PluginCatalogParams PluginCheckParams PluginCommandParams PluginGetParams PluginInstallParams PluginListParams PluginLogsParams PluginNewParams PluginQueryParams PluginRestartParams PluginSetEnabledParams PluginSetHiddenParams PluginSetOrderParams PluginSurfaceParams PluginSurfacesParams PluginUninstallParams SessionId`

`event.plugins_2eui.output`: `Badge Dot PluginRunState PluginUi SurfaceFormat Contract` (topic key `plugins.ui@1`).

`event.plugins_2eevents.output`: `SessionId Contract PluginEvent` (topic key `plugins.events@1`).

## Hand-offs and notes for B1–B4

- `rpc.rs` stubs call `rpc_core::owner(ext)?` (or `HumanPress::from_rpc(ext)` for `plugin_approve`) before answering; B2 replaces each body with lookup + one `ask`. `rpc_err` is in `rpc.rs` already (code + message, no `data` yet).
- `manifest::RESERVED_NAMES` is empty: B2's TUI test `tui_commands_are_reserved_plugin_names` fails until B1 fills it.
- `client_mirror.rs`: `PluginEvent` is a tagged struct, and serde does not check a struct's tag; it sits last in `republish!`, so only a frame no earlier type accepted can reach it. Its field set (`plugin`, `event`, `version`, `session`, `data`) is unique today.
- `wire_golden` / `wire_parity_inc5` families and fixtures for the two topics are B1's (not added by B0).

## Not verified

- `cargo check -p plugins --features ts` (the `ts` derives) was not run.
- No runtime check: nothing starts a plugin yet.
