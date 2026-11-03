# 162 PROVIDERS-UNIFY — custom providers instead of "gateways", and Refresh models

Status: building (2026-11-03). Owner: theuth orchestrator for Sky. Recon: `/tmp/162-recon.md` (omp 162-recon, read-only, main 6c456756).

## Ask (Sky, 2026-11-02 23:59, verbatim)

> ok it's with gateways, it seems they are not treated the same as providers, in the UI in settings connections we need a way to refresh models for gateways/providers, honestly I don't understand why their treated differently. gateways should just be "custom" providers. if there's no reason for the distinction, let's just combine them and simplify, and add the refresh button/ui for models on a provider.

## Finding

The split is mostly how the UI is laid out, plus two behaviour gaps; there is no essential reason for it.
- The engine already returns ONE list (`list_providers` → `Vec<ProviderInfo>`, `builtin` marks the kind; `crates/agent/src/lib.rs#L2147-L2163`).
- What is incidental: the two Settings sections ("Built-in providers" and "Gateways", `crates/desktop/ui/src/components/settings/Connections.tsx#L131-L293`), the word "gateway", the add form existing only for gateways, and no refresh anywhere.
- Behaviour gap 1: a gateway's key is copied into `Auth.gateways` at engine start (`crates/agent/src/supervisor.rs#L9113-L9116`, `#L9468`), so a key rotated through Secrets only applies after a restart. Built-in keys are read from the vault on every call (`Auth::key`, `supervisor.rs#L9087-L9090`).
- Behaviour gap 2: `/models` is called only by AddProvider (`supervisor.rs#L8580-L8641`). AddProvider then *replaces* the provider's `models.json` entry (`providers.rs#L182-L190`), which wipes hand edits, and stores an empty list when the listing fails (`supervisor.rs#L8605-L8609`).
- What really differs, and stays different inside the engine:
  - OAuth accounts (Accounts section);
  - Cursor's own backend;
  - Bedrock's AWS credential chain;
  - the models.dev metadata the built-ins carry: Zen's per-model protocol and prices, plus OpenRouter's request options and usage poller.
- The stated reason (85 §0.1: "a user-added gateway cannot carry per-model protocols") is out of date. A custom provider honours a `protocol` field in `models.json` (`backend.rs#L93-L98`).

## Decisions

- **D1 Words.** Everything a user reads says "custom provider", never "gateway":
  - desktop UI, `/provider` help and usage (desktop slash, TUI, Signal, Telegram);
  - error strings returned to users, such as `validate_name` messages;
  - the costs usage-limits panel's display of the `gateway` auth label, which shows `custom`.

  Internal identifiers stay as they are, so existing installs load with no manual step:
  - `providers::Gateway`, `Auth.gateways`;
  - `<data_dir>/providers.yaml` (`name: {base_url}`), the vault entry named exactly `<name>`, `models.json["<name>"]`;
  - the reserved-name rules;
  - the limits row id `<name>:gateway` and the `auth: "gateway"` label persisted in `limits/snapshot.json`;
  - the RPC names `list_providers` / `add_provider` / `remove_provider` and every existing `ProviderInfo` field.
- **D2 One list.** Settings › Connections gets one **Providers** section: built-ins first in engine order, then custom providers (engine order, alphabetical). Accounts (OAuth and MCP logins) and AWS Bedrock remain their own sections, because their login and credential flows are different.
- **D3 Refresh.** New RPC `refresh_provider_models(name)` asks the provider's model-list endpoint again and reloads the catalog (and costs' price table) exactly as AddProvider does (`supervisor.rs#L8632-L8633`).
  - custom → `all_model_names(AdapterKind::OpenAI, (Endpoint <base_url>, key))`, the key read live from the vault;
  - `opencode` (Zen) → the Zen endpoint with `auth.opencode_key()`;
  - `openrouter` → `AdapterKind::OpenRouter` with `auth.openrouter_key()`;
  - any other name → error `<name>'s models come from the built-in catalog` (unknown name → `no provider <name>`);
  - no key → error `<name> has no key; set it first`.
- **D4 Merge semantics (one writer for add and refresh).**
  - **Custom provider:** the listing is the truth for which ids exist.
    - An id still listed keeps its existing `models.json` entry unchanged, hand edits (`protocol`, limits, names) included.
    - A new id is added as a context-0 entry, so it inherits from its models.dev sibling.
    - An id no longer listed is removed.
  - **Built-in:** only ids absent from both the bundled catalog and the user file are added, as context-0 entries under that provider. Bundled ids, provider metadata and existing user entries are never touched, and nothing is removed.
  - **Listing fails or returns zero ids:** return an error and change nothing.
  - **AddProvider** uses the same writer. A re-add therefore no longer wipes hand edits. A listing failure on add still saves the provider (url + key), keeps any existing model list, and logs the error, so Refresh can retry.
  - The writer must not let a user block for a built-in blank the bundled provider metadata on merge (`crates/models/src/lib.rs` `merge`). Verify this with a test.
- **D5 Live keys.** A custom provider's key is read from the vault at call time (`Auth::key(name)`), like the built-ins. `Auth.gateways` keeps the base URL; the cached key is only a fallback when the vault read fails. A key replaced in Secrets then applies to the next call and to Refresh with no restart. The 160 seams must keep working: `Backend::stream` limits attribution (`auth: "gateway"`), and `limits::presence()` listing one row per custom provider (`limits::tests`).
- **D6 `refreshable`.** `ProviderInfo` gains `#[serde(default)] pub refreshable: bool`. It is true for every custom provider, and for `opencode` and `openrouter` when `configured`. The UI shows **Refresh models** only when it is true.
- **D7 Reply.** `RefreshedModels { provider: ProviderInfo, added: Vec<String>, removed: Vec<String> }` (ids sorted). The UI toast reads `rwx: 14 models (+2, −1)`, or `rwx: 14 models, no change`.
- **D8 Surfaces.**
  - desktop slash `/provider refresh <name>`;
  - TUI `/provider refresh <name>` (help + usage);
  - Signal and Telegram `provider refresh <name>` (same three lines; their dead "no custom providers" branch goes).
  - Usage string everywhere: `/provider [add [name [url [key]]] | refresh <name> | rm <name>]`.
- **D9 Per-row UI.**
  - Each row shows: status dot (`configured`) · label (`BUILTIN_LABEL[name] ?? name`) · a `built-in` or `custom` tag · `` `name/…` `` · N models · either the key state (built-in) or the base URL (custom).
  - Actions:
    - built-in: Set key… / Replace key…, as today;
    - **Refresh models** when `refreshable`; it spins while running and its error toasts verbatim;
    - custom: Remove, with the existing confirm behaviour.
  - Below the list goes an **Add custom provider** form (Name, Base URL, API key; same validation and RPC as today). It replaces the "Gateway name" form.
  - Empty custom part: one muted line, "No custom providers yet."
- **D10 Out of scope.**
  - Refresh for OAuth accounts and Bedrock: no working live listing; add it when genai can take an explicit OAuth target.
  - Pruning stale bundled ids.
  - A periodic or automatic refresh.
  - Renaming `Gateway` in code: about 12 test fixtures and a persisted id, for no change in behaviour.

## Contract (verbatim; both workers build against it)

Rust (`crates/agent/src/lib.rs`, beside `ProviderInfo`/`AddProvider`):

```rust
/// One provider row of the Connections view: a built-in or a custom provider.
pub struct ProviderInfo {            // existing fields unchanged, plus:
    /// Refresh models is available (custom providers; opencode/openrouter with a key) (162 D6).
    #[serde(default)]
    pub refreshable: bool,
}

/// `ask` → `Result<RefreshedModels, String>`: list the provider's models again (162 D3/D4).
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct RefreshProviderModels {
    pub name: String,
}

/// Reply of [`RefreshProviderModels`]: the row after the refresh and the ids it added or removed.
#[derive(Clone, Debug, Serialize, Deserialize, schemars::JsonSchema)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS), ts(export))]
pub struct RefreshedModels {
    pub provider: ProviderInfo,
    pub added: Vec<String>,
    pub removed: Vec<String>,
}
```

RPC (`crates/rpc/src/api.rs`, after `remove_provider`):

```rust
/// List a provider's models again (custom: `GET {base}models`; opencode/openrouter: their list) and reload the catalog (162).
#[method(name = "refresh_provider_models")]
async fn refresh_provider_models(&self, name: String) -> RpcResult<RefreshedModels>;
```

TypeScript, as ts-rs emits it (the UI hand-writes these stubs in the gitignored `src/rpc/bindings/` until integration regenerates them):

```ts
export type ProviderInfo = { name: string, base_url: string, models: number, builtin: boolean, configured: boolean, refreshable: boolean, };
export type RefreshedModels = { provider: ProviderInfo, added: Array<string>, removed: Array<string>, };
```

`crates/desktop/ui/src/rpc/api.ts`: `refresh_provider_models(name: string): Promise<RefreshedModels>` and `'refresh_provider_models'` in `METHODS` (alphabetical, after `'remove_provider'` if that is the order).

## Ownership

- **162-ENG** (`~/Develop/theuth-162-eng`, `feat/162-eng`) owns every `.rs` file and the bindings run:
  - crates/agent: providers.rs, lib.rs, supervisor.rs provider handlers and `Auth`, backend.rs only if D5 needs it, limits only if a test needs it;
  - crates/models (merge test);
  - crates/rpc;
  - crates/tui;
  - crates/signal and crates/telegram `provider_cmd`.
- **162-UI** (`~/Develop/theuth-162-ui`, `feat/162-ui`) owns `crates/desktop/ui`:
  - settings/Connections.tsx, settings/providers.ts and their tests;
  - chat/slash.ts;
  - rpc/api.ts;
  - the costs panel label mapping (D1).
- Integration: the orchestrator merges both, regenerates bindings in the ENG worktree, then runs tsc and vitest against the real bindings.

## As-built

Each worker appends `docs/design/162-as-built-<ENG|UI>.md` as its last commit.
