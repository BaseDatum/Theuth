# 171F-C — contract skeleton (worker C notes)

Base `5628e678`. Types and stubs only; no behaviour change.

## K1 — disclosure store (B13) — `crates/desktop/ui/src/lib/tool-runs.ts`
- `useToolRuns` state `disclosed: Record<string, boolean>` (initially `{}`), action `setDisclosed(key: string, open: boolean)`.
- `export function disclosureKey(...parts: Array<string | number | null | undefined>): string` — drops `null`/`undefined`, joins with `|`. `0` and `''` are kept (`disclosureKey(null, 'a', 0, '') === 'a|0|'`).
- `export function useDisclosure(key: string, initial = false): [boolean, (next: boolean | ((open: boolean) => boolean)) => void]` — `useSyncExternalStore(useToolRuns.subscribe, read, read)`, `read = () => disclosed[key] ?? initial`; the setter resolves a function against `read()` at call time.
- No component converted (U1/U2/U3).

## K2 — pass end (B11) — `crates/cells/src/types.rs`
```rust
pub struct PassStop { pub cell: CellRef, pub why: String, pub before: bool }
pub struct PassEnd { pub pass: String, pub total: u32, pub ran: u32, pub stopped: Option<PassStop> }
// StateReply
#[serde(default)] pub pass: Option<PassEnd>,
```
- Both via the `wire!` macro (same derives / `ts(export)` as the neighbours), registered in `crates/rpc/src/export.rs` after `cells::StateReply`.
- `pass: None` set in `Cells::state` (`actor.rs`, marked `// 171:E1`) and the not-running fallback in `rpc::cells::state`. No other `StateReply` literal exists.
- TS: `StateReply.pass: PassEnd | null` is a required key in the binding (ts-rs default for `Option`); UI code only reads replies, so no UI literal needed it.
- Decided: doc comments read `before: true` = stopped before running `cell`, `false` = `cell` ran and failed. E1 may refine the wording.

## K3 — reply→note RPC (B9)
- `rpc::Api` (`api.rs`, after `cell_latest`):
  - `#[method(name = "cell_reply_note_get", with_extensions)] async fn cell_reply_note_get(&self, session: cells::SessionId, entry: String) -> RpcResult<Option<cells::DocId>>;`
  - `#[method(name = "cell_reply_note_set", with_extensions)] async fn cell_reply_note_set(&self, session: cells::SessionId, entry: String, doc: cells::DocId) -> RpcResult<()>;`
- Impl delegates one line each to `crate::cells::reply_note_get` / `reply_note_set` (`crates/rpc/src/cells.rs`).
- Stubs: guard (`cells_guard::read` / `cells_guard::press`), then `route_scope(Scope::Session(session), "<method>", (session, entry[, doc]))` like `cell_runs` — a remote owner's answer is returned; locally get → `Ok(None)`, set → `Ok(())`. E2 replaces the local tail (marked `// 171:E2 stub`).
- Guard tests: `reply_note_set` added to `press_codes` (8 → 9), `reply_note_get` to `read_codes` (7 → 8).
- Not on `link::conn::GUEST_METHODS`, so the guest gate refuses both before the guard.
- UI `crates/desktop/ui/src/rpc/api.ts`: `cell_reply_note_get(session: SessionId, entry: string): Promise<DocId | null>`, `cell_reply_note_set(session: SessionId, entry: string, doc: DocId): Promise<null>`; both in `METHODS` (sorted).

## K4 — bindings
Generated into `bindings/` and `crates/desktop/ui/src/rpc/bindings/` (gitignored, left in the tree).

## Gate deviation
`mbx clippy -p cells -p rpc -- -D warnings` fails on main's untouched `crates/voice/src/history/mod.rs:465`
(`clippy::chunks_exact_to_as_chunks`, new in clippy 1.99). Gated with `--no-deps` instead, which lints exactly
`cells` and `rpc`: 0 warnings. The `voice` lint is pre-existing and out of scope.
