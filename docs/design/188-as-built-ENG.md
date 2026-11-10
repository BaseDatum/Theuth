# 188 as built: ENG (`config_flag_get` / `config_flag_set`)

The only engine change in 188: a write path for the three `enabled` flags Settings could only describe. Everything lives in the `rpc` crate.

## Where each piece lives

| Piece | Place |
|---|---|
| Method declarations (`config_flag_get(key)`, `config_flag_set(key, on)`) | `crates/rpc/src/api.rs:2455-2464`, in the `Api` trait after `search_settings_set` |
| `ConfigFlagKey` (the three keys, serde names `decision.enabled` / `a2a.enabled` / `computer.enabled`), `section()`, `read(&Figment)` | `crates/rpc/src/api.rs:3110-3155` |
| `ConfigFlag { key, enabled, running, restart_required }` | `crates/rpc/src/api.rs:3157-3169` |
| `Server::config_flag(key)`: file value, boot value, difference | `crates/rpc/src/api.rs:3270-3281` |
| Method bodies | `crates/rpc/src/api.rs:9262-9276`, after `search_settings_set` |
| Tests | `crates/rpc/src/api.rs:11002-11111`: `config_flag_get_reports_file_boot_and_restart`, `config_flag_set_writes_only_the_key`, `config_flag_rejects_an_unknown_key` |
| ts-rs export (`ConfigFlag`, which pulls in `ConfigFlagKey`) | `crates/rpc/src/export.rs:19`; the per-type `crates/rpc/bindings/{ConfigFlag,ConfigFlagKey}.ts` are committed like the 64 tracked files next to them |
| `figment` moved from rpc's dev-dependencies to its dependencies | `crates/rpc/Cargo.toml:12` |

## Persistence path reused

`runtime::write_config_value(path, section, key, value)` (`crates/runtime/src/lib.rs:109`), on `<data_dir>/config.yaml`. It is the engine's single-scalar patcher: it rewrites only the direct-child `key:` line of the section (keeping a trailing `# comment`), inserts the key under an existing header, appends a missing section, creates a missing file, and refuses a write that would break YAML that parsed before. Link, mesh, voice (`Server::write_voice_config`) and the decision engine's per-family mode writes use it already. The other candidates rewrite a whole section from a struct: `autoresearch::config::persist`, `double::sched::persist`, `dreaming::night::persist` (dreaming's doc points single scalars at `write_config_value`), and `search_settings_set` persists through the web and search-providers actors' own section writers. A whole-section rewrite would drop sibling comments and keys the struct doesn't know, so it was not used.

`set` writes `<section>.enabled: true|false` and then answers like `get`. It changes nothing live: no actor is told, no plugin is started or stopped.

## Where `running` and `enabled` come from

One function, `ConfigFlagKey::read(&Figment)`, extracts the section into its own config type and takes `.enabled`, with `unwrap_or_default()` for a missing or unreadable section. That is how each plugin reads its section at start, so the defaults come from the types, not from this code:

| Key | Type | Default | Plugin start |
|---|---|---|---|
| `decision.enabled` | `decision::DecisionConfig` | `true` | `crates/decision/src/lib.rs:691` (`rt.section("decision")`, default on error) |
| `a2a.enabled` | `a2a::config::A2aConfig` | `false` | `crates/a2a/src/lib.rs:978` (default when absent or malformed) |
| `computer.enabled` | `computer::ComputerConfig` | `true` | `crates/computer/src/lib.rs:279` (`rt.section("computer")`, default on error) |

- **`running`** = `read(&self.rt.config)`: the `Runtime`'s figment, merged once at boot (`crates/theuth/src/main.rs:1591`: config.yaml then `THEUTH_*` env) and never reloaded. It is the same figment every plugin started from, so nothing had to be captured at startup and the file is never re-read for it.
- **`enabled`** = `read(&Figment::from(Yaml::file_exact(<data_dir>/config.yaml)))`, read fresh on every call.
- **`restart_required`** = `enabled != running`.

## Decisions

1. **Key validation by serde.** `ConfigFlagKey` is a serde enum, so any other string fails param parsing: jsonrpsee answers `-32602 Invalid params` with serde's message, `unknown variant \`x\`, expected one of \`decision.enabled\`, \`a2a.enabled\`, \`computer.enabled\``. No hand-written check. The test asserts the code and all three names.
2. **Authorization like `search_settings_set`:** no per-call check and no `with_extensions`. These are node settings of the engine the client talks to, admitted by the connection, and never forwarded.
3. **Sync methods** (`fn`, like `client_log`): there is no actor to ask, only a small file read or write, which is what `write_voice_config` does inline too.
4. **Same reader for both sides.** `enabled` uses the plugins' own extraction (whole section, default on error), not a bare `enabled` lookup. So `enabled` is exactly what the plugin will run with after a restart, and `restart_required` goes false as soon as a restart picks the value up.
5. **Environment overrides.** `running` includes `THEUTH_DECISION__ENABLED`-style overrides, which `enabled` (file only) does not. With such an override set, `restart_required` stays true while the file disagrees. That reports real state (a restart will not change it either), and it is a case the UI never offered before. Left as is.
6. **`figment` became a normal dependency of `rpc`.** `runtime` already depends on it with the same features, so the build graph gains no crate. A new `runtime::read_config_value` helper for one caller was not worth it.
7. **ts-rs:** `ConfigFlag` and `ConfigFlagKey` derive `TS` under the `ts` feature like `Info`, and `export.rs` lists `ConfigFlag`. `bindings/methods.json` lists both methods. The UI keeps its hand-written types in `api.ts`; the field names and key strings are the same.
8. **Non-atomic write.** `write_config_value` writes in place, as it does for every other caller. Making that atomic would be a runtime-wide change outside 188.
9. **Comment spacing on the rewritten line.** `write_config_value` keeps a trailing comment from its ` #`, so `enabled: false  # note` becomes `enabled: true # note` (one space). Every other line stays byte-for-byte. The test fixture uses one space so it can assert the whole file. This writer behaviour predates 188 and is shared by every caller, so it was left alone.

## Gates (final tree)

1. `build-lock mbx check -p rpc --tests`: pass, 0 warnings.
2. `build-lock mbx clippy -p rpc --no-deps --tests -- -D warnings -A clippy::chunks_exact_to_as_chunks -A deprecated`: pass, 0 warnings.
3. `build-lock mbx test -p rpc --no-run`, then `mbx test -p rpc config_flag`: 3 passed. `mbx test -p rpc --lib`: 91 passed, 0 failed (no pre-existing failures in rpc's lib).
4. `build-lock mbx test -p rpc --features ts --lib --quiet export_bindings`: 10 passed; `bindings/methods.json` lists `config_flag_get` and `config_flag_set` (not committed, gitignored).

## Hand-off

- The UI side is C0b's `ConfigFlagRow` / `useConfigFlag` (`crates/desktop/ui/src/components/settings/ConfigFlagRow.tsx`), which calls these methods positionally (`[key]`, `[key, on]`) and reads the four snake_case fields. No UI file was touched here.
- W-LEARN (decision, computer) and W-NET (a2a) mount `ConfigFlagRow`. An engine older than this change answers `Method not found`, and `useConfigFlag` runs with `retry: false`.
