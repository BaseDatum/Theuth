# 201 as built — PC (the contract)

Config keys, wire fields and override plumbing of 201 §12.1/§12.2 "Wire", §4.2, §7.1. No behaviour change: nothing reads the new values for the frame or the compactor yet.

## Landed

- **`crates/agent/src/endless/mod.rs`**
  - `EndlessConfig` gains every §12.1 key with its default: `tail_mode`, `tail_low_bytes`, `tail_high_bytes`, `tail_low_share`, `tail_high_share`, `cold_after_secs`, `profile`, `merge_at`, `ledger`, `ledger_bytes`, `ledger_suggestions`, `prefetch_bytes`, `age_by_use`, `describe_images`. The struct is `#[serde(default)]`, so old sections load. Each new key is optional in TypeScript (`ts(as = "Option<…>", optional)`, as the 137F/G keys are). `settle_secs` and `raw_bytes` are unchanged and still accepted; `default_chat` is unchanged.
  - New enums `TailMode {Fixed, Scaled}`, `Profile {Auto, Standard, Small}` and `DescribeImages {Auto, Off}`. They use `MemoryMode`'s derives and `snake_case`, which is lowercase on the wire, and have `as_str`. `TailMode` and `Profile` also have `FromStr`/`Display`.
  - `EndlessConfig::clamped()` applies the 201 ranges at use.
  - `EndlessConfig::validate_201()` refuses a value out of range and a pair whose high mark is not above its low one. The message names both values ("tail_high_bytes 64000 must be above tail_low_bytes 64000").
  - `EndlessConfig::with_tail(Option<&TailOverride>)` applies an override.
  - `EndlessConfig::tail_marks(allowance_bytes) -> (low, high)` is pure. It clamps both marks to 8 000..=512 000 with high > low, then scales them down together when high > allowance/2. Nothing calls it yet; P2 will.
  - `TailOverride {mode, low, high}` holds the marks in the mode's unit: bytes for fixed, shares for scaled. `validate()` checks it.
  - `Endless::set_tail`.
  - `SessionMemory` gains `tail_mode`, `tail_low`, `tail_high` (bytes in force after the override), `tail_override`, `tail_bytes`, `generation` (P2), `drafts` (P1), `profile` (P3; `standard` until then; the serde default is `standard`) and `profile_reason` (P3). All are serde-defaulted and optional in TypeScript.
- **`agent/migrations/0069_endless_tail.sql`** adds `sessions.endless_tail_mode TEXT`, `endless_tail_low REAL` and `endless_tail_high REAL`, all NULL = inherit. `SESSIONS_WIRE_SCHEMA` goes 67 → 69 under the constant's bump rule (`sessions` is replicated). The schema snapshot test is updated. The 0068 test's `assert_eq!(…, 67)` becomes `assert_ne!(…, 68)`.
- **`store/endless.rs`**: `EndlessMeta.tail` (the struct drops `Eq` because the tail holds f64) and `Store::set_endless_tail(id, Option<TailOverride>)`.
- **`endless/pump.rs`**: `Live.tail` is loaded at attach and resume. Added `set_tail`; `tail_status()` fills the new `SessionMemory` fields on both paths of `memory()`.
- **Wire**: RPC `set_session_tail(session, tail: TailOverride | null) -> SessionMemory` (rpc_turns.rs, forwarded like `set_session_view_bytes`), message `agent::SetSessionTail` and its supervisor handler. `endless_ext::set_tail` validates. `SetEndlessSettings` calls `validate_201()` first and writes the 14 new keys to config.yaml.
- **Roster**: `AgentFile` gains `tail_mode`, `tail_low_bytes`, `tail_high_bytes`, `tail_low_share`, `tail_high_share` and `profile`, all optional. `file::parse` validates them: an unknown value is refused, the §12.1 ranges apply, and a pair given in full must have high > low. `file::render` writes them. `agent_put` and `agent_propose` take no new arguments, because they do not offer `view_bytes` either.
- **Exports**: `agent::{TailMode, Profile, DescribeImages, TailOverride, SetSessionTail}`.

## Deviations

- `Profile` is exported as **`CompactorProfile`** in TypeScript and JSON schema (`serde(rename)` + `ts(rename)`) because `meeting::speakers::Profile` is already ts-exported as `Profile`. The Rust name and the wire values are as specified.
- `SessionMemory` also carries `tail_override`, beside `model_override`, so the Memory panel can show and edit the session's own pick. The brief did not name this field.
- `generation` is `u32`, so it is a TypeScript `number` without a type override.
- `tail_low`/`tail_high` are bytes. In `scaled` mode they are 0 until P2 supplies the allowance (see seams).
- `EndlessProgress` is unchanged. The panel refetches `session_memory` on that event rather than reading values from it.

## Seams for later phases

- `pump::turn_allowance_bytes()` returns `None`. P2 returns the session model's turn allowance from it; scaled marks in the status then become real.
- `tail_bytes` and `generation` are filled by P2, `drafts` by P1, and `profile` (resolved) and `profile_reason` by P3. Each field's doc comment names its phase.
- Per-agent `tail_*` and `profile` are parsed and carried on `AgentFile` but not applied to the desk session. A comment at the `view_bytes` application in `roster/src/lib.rs` marks this; P2 (tail) and P3 (profile) apply them.
- No code reads `Live.tail`, `cold_after_secs`, `merge_at`, `ledger*`, `prefetch_bytes`, `age_by_use` or `describe_images` yet.

## Unfinished / not verified

See the report for what compiled and which tests ran. The TypeScript bindings are untracked (`git ls-files bindings crates/desktop/ui/src/rpc/bindings` is empty), so a regeneration leaves nothing to commit. The wire fixture `crates/theuth/tests/fixtures/wire/openrpc-v1.json` is expected to differ (new method `set_session_tail`, new fields, new schemas). It was not accepted.
