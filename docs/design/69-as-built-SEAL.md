# 69 follow-up SEAL — as built

Branch `fix/seal-share-secret-hash`, worktree `/Users/Sky.Moore/Develop/theuth-seal`.

## Commit `fix(document): share-link secret_hash sealed with the mesh key`

- `crates/document/src/meta.rs` — `ShareLink.secret_hash` → `secret_hash_sealed: String`: base64url(`nonce ‖ XChaCha20-Poly1305(mesh key, aad = link id, sha256(secret))`), still `skip_serializing_if empty` + `ts(skip)`, blanked by `public()`.
- `crates/document/src/share.rs` — `seal_hash(vault, link_id, hash)` / `open_hash(vault, link)` over `secrets::Vault::{seal_with_mesh_key, open_with_mesh_key}` (the 69 pending-PSK pattern, `crates/link/src/devices.rs`); `verify(vault: Option<&Vault>, link, …)` opens the hash at proof time — no vault = no key = refused; `LinkCreate` seals with the vault or refuses (`secrets vault unavailable; cannot seal the link secret`); `join` takes the vault before `load` (borrow). Proof math (`secret_hash`, `proof`) untouched.
- `crates/document/src/store.rs` — `Store.vault: Option<Arc<secrets::Vault>>`, `with_vault(Arc<Vault>)`, `vault()`; `parse_meta(bytes) -> (DocMeta, sealed)` seals a plaintext `links[].secret_hash` (decoded base64url) into `secret_hash_sealed` via `serde_json::Value` before typing; `read_meta` rewrites `meta.json` in place + `mirror_write` when it did (one-time migration, logged); `mirror_apply` `Meta` parses the incoming foreign meta through `parse_meta` so an un-upgraded peer's plaintext link is sealed before `merge_meta`.
- `crates/document/src/actor.rs` — `MirrorChanged` (open document) parses the foreign meta through `store.parse_meta` for the same reason.
- `crates/document/src/mirror.rs` — merge rule reads `secret_hash_sealed` (empty never replaces set).
- `crates/document/src/lib.rs` — plugin start: `secrets::handle(rt).await` → `store.with_vault(v)`; `None` logs a warning and continues without sharing (secrets starts before document in `theuth/src/main.rs`).
- `crates/document/Cargo.toml` — `secrets = { workspace = true }` (no cycle: secrets does not depend on document).
- `crates/document/tests/actor.rs` — `start_in` gives the store a `Vault::open(data_dir, KeyStore::File)`; the `meta.json` assertion now checks `secret_hash_sealed` is present, non-empty and not the plaintext hash, and that no `secret_hash` key remains; wire checks use `secret_hash_sealed`.
- Docs: `docs/design/69-contract-docs.md` §3.2 (merge rule line + the "Security" paragraph rewritten for SEAL, incl. migration and ceilings); `docs/design/69-devices.md` §8 "Found, not fixed" — `secret_hash` removed with a "fixed after merge" note.

## Confirmation asked for in the brief (engine without a mesh key)

`Vault::open` (`crates/secrets/src/vault.rs:197-207`) mints a random mesh key and stores it sealed under the engine key on the very first open when `meta.mesh_key` is absent. So there is never an engine "without a mesh key": an unpaired engine seals with its own freshly minted mesh key through the same `seal_with_mesh_key` path — no separate engine-local fallback was built. Pairing later replaces it via `AdoptMeshKey` (`crates/mesh/src/net.rs:524` → `Vault::adopt_mesh_key`), which re-seals vault *rows* only.

## Deviations

- Encoding is base64url (no pad), not standard base64: the field's previous encoding and every other document-crate encoding is base64url; one alphabet.
- The vault is a `Store` field rather than an actor field: the migration runs where `meta.json` is parsed (`Store::read_meta` / `mirror_apply`), which the actor does not see.
- `verify` is `pub(crate)` (was private) so the store migration test can prove a guest proof verifies after migration.
- `DocumentPlugin::start` now awaits `secrets::handle` (≤ 10 s when no secrets actor ever registers, as `link` does). `crates/rpc/src/search.rs` `registered_tool_obeys_the_registry_and_reports_owner_states` starts `DocumentPlugin` without `SecretsPlugin` → that test gains the 10 s wait; not changed (adding `SecretsPlugin` there would touch the rpc crate and `KeyStore::Auto` may hit the keychain in a test).

## Ceilings (recorded in `69-contract-docs.md` §3.2)

- No vault (secrets failed to open): `LinkCreate` refuses, every proof fails; a plaintext pre-SEAL hash cannot be sealed and is dropped by the typed parse (the link then refuses every proof, as it would without the key). Nothing is rewritten on disk in that case (`parse_meta` returns `sealed = false`), so the plaintext survives until some other `write_meta`.
- A link sealed before this engine adopted another mesh's key does not open afterwards (the vault re-seals its rows, not `meta.json`; the old key is gone). Recreate the link. Upgrade path if it matters: cache open documents' plaintext hashes in the actor and re-seal on `SecretsEvent::Changed`.

## Not built

- No re-seal on `AdoptMeshKey` (above).
- No change to the UI/TS: the field was `ts(skip)` and blank on the wire before and after.

## Tests written (not run, per the brief)

- `share.rs` `sealed_hash_round_trips_under_the_mesh_key_only` — seal/open round trip; another vault's mesh key does not open it; a different link id (aad) does not; the wire copy has nothing.
- `share.rs` `proof_binds_secret_connection_and_key` — as before, now through the sealed link and `Some(&vault)`; plus `verify(None, …)` refuses.
- `store.rs` `plaintext_link_hash_is_sealed_on_load` — a pre-SEAL plaintext `meta.json`: `read_meta` seals, rewrites the file and the mirror copy (no `secret_hash` key left), the sealed value opens to the same hash, the guest proof still verifies (and a wrong one does not), a second read is stable, a foreign plaintext meta is sealed by `parse_meta`, and a store without a vault rewrites nothing.
- `tests/actor.rs` `share_links_and_members` (existing) — `meta.json` holds `secret_hash_sealed`, never plaintext; joins through the sealed link still succeed.

## `cargo check -p document -p secrets -p rpc --tests`

Run once, after all edits (first run hit the 15 min tool deadline on a cold target; the rerun finished):

```
    Checking secrets v1.17.0 (/Users/Sky.Moore/Develop/theuth-seal/crates/secrets)
    Checking document v1.17.0 (/Users/Sky.Moore/Develop/theuth-seal/crates/document)
warning: unused variable: `vlink`   --> crates/document/tests/actor.rs:775:10
warning: unused variable: `elink`   --> crates/document/tests/actor.rs:807:10
    Checking rpc v1.17.0 (/Users/Sky.Moore/Develop/theuth-seal/crates/rpc)
    Finished `dev` profile [unoptimized + debuginfo] target(s) in 10m 49s
```

Both warnings pre-exist on `HEAD` (`vlink`/`elink` at the same lines before this change). No TS touched, so no `bunx tsc`.

## Commits

1. `b31ee05 fix(document): share-link secret_hash sealed with the mesh key`
2. `docs(design): 69 as-built SEAL report` — this file as `docs/design/69-as-built-SEAL.md`.

DONE SEAL
