# 225 — Sealed vault: envelope key, unlock methods, device unlock over the mesh

Status: built on `feat/sealed-vault` (2026-11-27). Supersedes 37 §3.5's key paragraph; 183's
pairing keyring hand-over is unchanged.

## 0. Decisions in one screen

1. **Envelope.** The engine key of 37 §3.5 / 67 §5.4 (it seals the mesh keyring and the
   engine-local `meta` rows, which seal every vault row) becomes the **DEK**. It never sits on
   disk unwrapped except in the legacy `file` method. `<data_dir>/vault-keys.json` (0600, atomic
   tmp + fsync + rename) holds one wrapped copy per unlock method:
   `{kind, id, label?, kdf?, wrapped = base64(nonce[24] ‖ XChaCha20-Poly1305(KEK, DEK, aad)), created_ms}`,
   `aad = "theuth-vault-dek-1\0<kind>\0<id>"` (a blob cannot be moved to another method).
   Adding/removing a method rewraps the DEK only; the vault is never re-encrypted.
2. **Methods.**
   - `keychain` — KEK = SHA-256(domain ‖ keychain item). On a migrated desktop the item *is* the
     pre-225 key (no keychain write, no prompt, an older build still opens the vault); a fresh
     install mints a separate item and DEK.
   - `password` — KEK = Argon2id(password, 16-byte salt), `m = 64 MiB, t = 3, p = 1` (a
     1-vCPU / 2 GB box), params stored with the blob. ≥ 12 characters. Unwrap/derive runs on a
     blocking thread. 5 free attempts, then 30 s doubling to 15 min between attempts.
   - `device:<owner endpoint id>` — KEK = `K_dev = SHA-256("theuth-unseal-kdev-1\0" ‖ S ‖ node id)`,
     `S` = 32 random bytes in the **owner's** vault (`meta` row `unseal/device-secret`, sealed by
     the owner's DEK, never replicated). The node keeps only `wrap(DEK, K_dev)`; the owner keeps
     only `S`. Neither ever stores the other's DEK.
   - `file` — legacy plaintext `secrets.key`; exists only on migrated Linux engines. Status warns
     *"vault key stored unprotected on disk — add a master password"*; adding a `password` or
     `device` method removes it and shreds the file (zeros, fsync, unlink; best effort).
3. **Sealed.** Boot opens the DEK with `file` or `keychain` if present; otherwise the engine starts
   **sealed**: mesh, sync, sessions, every non-secret feature run; the secrets plugin finishes its
   start (actor, tools, replication of the vault tables) when the vault opens. While sealed:
   placeholder expansion fails with *"the vault is sealed on sky-cloud — unlock it from your phone or another
   of your devices, or with the master password"*; `secrets::handle` answers `None` at once and
   `secrets::sealed()` says why; RPC errors carry code **-32052** (`VAULT_SEALED`, the same code the per-machine settings forward uses), message prefix "the vault is sealed", `data: {engine: <name>, methods: [<kinds>]}`; due schedules
   are **deferred** (their `next` stays due; the first tick after the unseal fires them, `missed:`
   deciding as after a pause).
4. **Memory.** The DEK lives in `LockedKey` (boxed, `mlock`ed best effort, zeroed + `munlock`ed on
   drop). While a DEK is in memory: `RLIMIT_CORE = 0` and (Linux) `prctl(PR_SET_DUMPABLE, 0)`.
   The mesh keyring and decrypted values are ordinary `Zeroizing` heap memory (not mlocked).
5. **Seal a running engine** (`t vault seal`): reads fail with the sealed text and the plaintext
   copy is dropped; the DEK and mesh keys stay in the process (other plugins hold the vault)
   until a restart. Needs a `password` or `device` method.
6. **Device unlock rides `theuth/rpc/1`** (`agent::rpc_forward::to_peer`), no new channel. The
   receiver reads the caller's authenticated endpoint id from the forwarded marker. See §2.
7. **Pairing.** 183's keyring hand-over is untouched (that is the *mesh* key, not the DEK). After
   a pairing completes, both sides note it; an engine without a `keychain` method (a headless node)
   asks the new peer to become an unlock device. An `enroll` within 15 min of a pairing is approved
   without a prompt — the pairing ceremony was the human's approval.
8. **No recovery.** Lose every method and the secrets on that machine are gone; the UI says so.

## 1. Boot and migration (`envelope::boot`)

| on disk at first start of this version | result |
|---|---|
| `vault-keys.json` | try `file`, then `keychain`; else sealed |
| `secrets.key` (Linux, pre-225) | the file is the DEK → `file` method; loud warning; nobody is locked out |
| pre-225 keychain item | the item is the DEK → `keychain` method wrapping it; no prompt |
| nothing, `key_store: auto/keychain` | new keychain item + new DEK; `auto` without a keychain falls back to `file` (as before) |
| nothing, `key_store: file` | `file` (tests, explicit choice) |
| nothing, `key_store: sealed` | DEK in memory only, zero methods; status warns until a password/device method is added |

`key_store: sealed` (new) never writes a key file or a keychain item. `t vault init
--password-stdin` provisions a fresh data dir with only a `password` method (the engine then boots
sealed and opens with the password).

## 2. Device unlock protocol (`seal.rs`)

Two one-way calls over the authenticated, end-to-end-encrypted `theuth/rpc/1`:

1. node → owner `vault_peer_ask(nonce, purpose, name)`; `purpose` = `unlock` | `enroll`. The node
   mints a 16-byte nonce per (owner, purpose), valid 10 min. The owner checks the caller (the
   forwarded marker, not a parameter): `unlock` only for a node it enrolled; it answers
   `{key}` at once when that node is set to *Unlock automatically* (or an `enroll` inside the
   pairing grace), else `{pending}` and shows a prompt.
2. owner → node `vault_peer_grant(nonce, purpose, key, name)` after the human pressed Unlock.

The node accepts a key only for an outstanding nonce it minted **for that owner and purpose**,
within the TTL, and consumes it on the first grant; a replay, a nonce sent to another owner, or a
stale one is refused. A grant from the wrong engine does not burn the nonce. While sealed the node
asks every reachable device owner, re-asking with back-off 5 s → 60 s (15 s while a prompt is
showing) until any method opens the vault; the master password works throughout.

## 3. RPC (`crates/secrets/src/rpc.rs`, owner audience unless noted)

`engine: string | null` — `null` = this engine; an id forwards over the mesh (D15's forward).

| method | params | result |
|---|---|---|
| `vault_status` | `[engine]` | `VaultStatus` |
| `vault_unseal` | `[engine, password]` | `VaultStatus`; -32042 wrong password / throttled |
| `vault_seal` | `[engine]` | `VaultStatus` |
| `vault_add_password` | `[engine, password]` | `VaultStatus` |
| `vault_change_password` | `[engine, old, new]` | `VaultStatus` |
| `vault_remove_method` | `[engine, id]` | `VaultStatus` (never the last) |
| `vault_add_device` | `[engine, owner]` | `VaultStatus` (method appears once the owner approves) |
| `vault_unseal_requests` | `[]` | `UnsealRequestView[]` (poll) |
| `vault_answer_unseal` | `[id, approve, always]` | `bool`; human press only |
| `vault_unlock_devices` | `[]` | `UnlockDevice[]` |
| `vault_set_auto_unlock` | `[node, auto]` | `null`; human press only |
| `vault_forget_node` | `[node]` | `null`; human press only |
| `vault_peer_ask` (mesh) | `[nonce, purpose, name]` | `{key?, pending, name?}` |
| `vault_peer_grant` (mesh) | `[nonce, purpose, key, name]` | `null` |

```ts
VaultStatus { engine: string|null, name: string|null, state: 'unsealed'|'sealed',
  unsealed_by: 'keychain'|'file'|'password'|'device'|'memory'|null, methods: VaultMethod[],
  password_set: boolean, warning: string|null, asking: string[], error: string|null }
VaultMethod { kind: 'keychain'|'password'|'device'|'file', id: string, label: string, created_ms: number }
UnsealRequestView { id, node, node_name, purpose: 'unlock'|'enroll', created_ms }
UnlockDevice { node, name, auto: boolean, enrolled_ms }
```

No events: the prompt host polls `vault_unseal_requests` every 3 s (a wire-event addition would
need a schema snapshot regeneration; polling is enough for a prompt that waits minutes).

## 4a. CLI (`crates/theuth/src/vault_cli.rs`)

`t vault status [--json] | seal | unseal | methods | add-password | change-password |
remove-method <id> | init`, each with `--engine <id>` (a mesh peer, forwarded) and
`--password-stdin` where a password is read (else a no-echo prompt; new passwords typed twice).
All but `init` call the running engine's RPC (the `cmd_67` transport); `methods` falls back to
`vault-keys.json` offline; `init` refuses a running engine or an existing vault. Exit 2 on
-32052 (sealed).

## 4. Cloud-node integration (built on `feat/wizard-vault`; 224 §16.11 has the detail)

- **Config.** `theuth-install.sh` appends `secrets: {key_store: sealed}` to the node's first
  `config.yaml` (VM cloud-init and the engine image alike), only when the installed `t` has `t vault`
  and no vault exists yet. The node starts with its DEK in memory and no method (§1), never writes
  `secrets.key`, pairs, and its pairing `enroll` adds `device:<creating engine>` (auto-approved in
  the 15-minute grace). `t vault init --password-stdin` is not used: it would need the password on
  the node before first start, i.e. in user data.
- **Password.** The wizard sends it in `cloud_create(spec, vault_password)` to the engine it is
  connected to; the create job holds it in a `Zeroizing<String>` in memory only. After the join it
  waits for `vault_status(node).methods` to list `device:<this engine>` (asking
  `vault_add_device(node, <this engine>)` once if missing), calls `vault_add_password(node, pw)`
  (forwarded over `theuth/rpc/1`, KEK derived on the node), drops the password, and reads
  `vault_status(node)` back. Never in the node record, user data, provider metadata, env, the node's
  disk (only the Argon2id-wrapped DEK), logs, the replicated vault or any event.
- **Gaps.** The node's `t` predates 225 (it installs the released `t`): `vault_status` answers
  method-not-found, the stage is skipped with *"Vault protection arrives with the next theuth
  release on this node"*, the create succeeds. The call fails, or the creating engine restarted (the
  password is gone): the node is ready with *"Vault: device only — set a master password"*, whose
  action is this design's `PasswordForm` aimed at the node (`vault_add_password(node, pw)`).
- **Another owner later:** `vault_add_device(nodeId, ownerId)`.
- **Phone-only nodes** (224 §16.12): the creating engine is an iPhone's own, not a mesh member, so
  there is no device unlock and no password through `cloud_create` (refused there). The app pairs
  with the node over the relay and calls `vault_add_password(null, pw)` on it as its connected
  engine; after a node restart the app's `SealedEngineCard` (in `UnsealHost`) offers
  `vault_unseal(null, pw)`. When the iOS engine can join meshes, the phone joins the node's mesh
  and device unlock applies as for any owner.

## 5. UI (`crates/desktop/ui/src/components/vault/`)

`UnsealHost` (mounted in `routes/__root.tsx`, top-right, beside the secrets `UnlockHost`) shows
`UnsealPrompt` cards — approval-style, not toasts: *"sky-cloud restarted — unlock its vault?"*,
Unlock / Not now, "Always unlock automatically from this device". `beforeApprove` is the seam
for a later biometric gate. `PasswordUnlockSheet`; Settings › Security › Vault (`VaultSettings`):
per-machine state, methods, add/change password with confirmation and strength meter, remove,
seal, unlock, machines this device unlocks with the auto toggle. The plain no-recovery sentence.
The page calls the methods by name through the raw client (`api.ts`) until the bindings are
regenerated.

## 6. Threat model

Protects: a copied disk, volume snapshot or backup of a node (no key beside `secrets.sqlite`);
a stolen sealed node; a captured unlock grant (single use, bound to nonce + owner); a non-owner
engine asking for a key (owner checks the authenticated caller and its enrolment); core dumps.
Does not protect: a running unsealed engine's memory (root, a debugger with CAP_SYS_PTRACE, the
hypervisor); a node impersonator holding the stolen mesh identity — the owner's prompt is the
control, and *Unlock automatically* removes it; a paired engine (it already holds the mesh key);
a weak master password against offline Argon2id guessing; residual blocks of a shredded key file.

## 7. Not built

- Recovery kit export (password-wrapped blob + instructions).
- Withdrawing an owner prompt when the node unsealed by another method (it expires in 10 min;
  answering it then is a harmless no-op on the node).
- Rotating `S`/`K_dev`; re-enrolment is remove + add.
- The real `theuth/rpc/1` path between two full engines is not exercised by a test; the gate
  tests run two gates with an in-process peer call that stamps the caller as the forwarded marker
  does.
- Face ID / biometric gate (iOS shell owns it).
