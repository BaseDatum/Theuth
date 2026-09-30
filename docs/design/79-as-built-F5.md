# 79 — As built, worker F5: the mesh vault key never moves from an established engine to a fresh one (smoke bug 10)

Branch `feature/79`, worktree `theuth-79`, 2026-09-30 11:15–12:30 BST. Start `fc349b93`; end: the docs commit after `2a25cde7` (see the final message). Brief `/tmp/79/F5.md`. Nothing pushed; no amend/squash/rebase.

## The bug and the decision

`bringup.rs` `pair`: the guest prints its string (its own fresh `vault_key`), the host runs `Net::add_peer`, and `add_peer`'s last step made the *dialer* adopt the *string's* key — so the host re-keyed its vault under every new VM's key and every other engine's copy stopped opening (Z2: `lab` lost `anthropic` the second `lab2` paired). Root cause: adoption followed the dial direction, not who owns the established mesh.

Fix (brief option b, generalised): **key ownership by rank, decided on both sides of the handshake.** `Rank = (paired_peers, first_seen_ms, id)`; more peers owns, tie → older identity (the `identity.rs` file age F3 already uses for the mesh name), tie → lower id. Both sides compute `pairing::key_owner(dialer, acceptor)` from the pre-pairing counts (the dialer's rank travels in `PairHello`, the acceptor's in `PairAck`), so they always agree. Owner = acceptor → the dialer adopts the string's key exactly as before. Owner = dialer → the acceptor adopts the key carried in `PairHello` *before* replying `PairAck { adopted: true }`, and the dialer keeps its key. The key only ever rides inside the `ALPN_MESH` QUIC stream (TLS to both identities; the dialer checked `remote_id`, the acceptor checked the token first) — doc comment on `Msg`.

## Commits

| sha | what |
|---|---|
| `65ae8620` | `fix(mesh)`: `pairing::{Rank, Side, key_owner}` (+ table test); `Msg::PairHello { vault_key, rank }`, `Msg::PairAck { adopted, rank }` (all `serde(default)`, so a pre-F5 peer parses and is parsed); `Net::rank()`, `Net::adopt_key()`; `add_peer` decides with the ack's rank, warns `runs a pre-79-F5 t (no key rank in its PairAck): taking its vault key as before` when the rank is missing; `accept_control` adopts when `key_owner == Dialer`; `BindOpts { first_seen_ms, keys: Option<Arc<dyn VaultKeys>> }`; `trait VaultKeys { get, adopt }` with `SecretsKeys` (the secrets actor) and a test `MemKeys`; three-node loopback test `vault_key_stays_with_the_established_engine`. `lib.rs` passes `identity.first_seen_ms`; `rpc/src/acp.rs` test literal gains the two fields |
| `c8813bb2` | `fix(sandbox)`: the `daemon` step runs `systemctl restart theuth` once after `first_run` (smoke bug 11, first half — see deviations 7–8) |
| `89a37fdd` | `fix(sandbox)`: `config::mode()` checks `guest` before `enabled` (smoke bug 12: no VM ever registered the guest prompt block / `sandbox_takeover`); `mode_follows_the_switches` updated. Also `mesh: adopted the vault key from …` info line on the acceptor |
| `c2f123c7` | `fix(secrets)`: `store::open` runs the migrator inside `BEGIN IMMEDIATE` (`run_direct` on the transaction's connection); test `concurrent_first_opens_all_succeed` (8 openers of one fresh file; fails 2/3 runs without the lock) — smoke bug 11's root |
| `2a25cde7` | rustfmt of the `config.rs` test |
| docs commit | `79-smoke.md` (bug 10 → fixed; "F5 proof" section with 5 checks, bugs 11–12, timings), `79-sandbox-vm.md` (§13 amendment for 67 §5.4/§4.1; §16 rows for bugs 10/11/12; open list), this file |

## Live proof (`79-smoke.md` "F5 proof" has the table)

Final round (check 5), image `a1b28990893f`, guest `t` sha `4bb6926f…` = `deploy/vm/t` built from `c2f123c7`: host fingerprint (`sha256` of the sealed `mesh_key` blob and of `anthropic#1`'s ciphertext) `38c60fb199b5c85c` / `810ed93197c36d46` unchanged before `lab`, after `lab`, after `lab2`, after both destroys; both guests log `mesh: adopted the vault key from smoke-host`, list the host's three names (`anthropic`, `smoke.db.pw`, `smoke.pw`), and a child on `lab` answered a one-line prompt (`4`) and quoted F2's masked-field sentence verbatim from its system prompt. `lab` 15.0 s, `lab2` 15.0 s to `ready`; child turn 10 s; destroy 3.4/3.6 s; `msb ls` and `msb snapshot ls` empty; engine stopped. Compat path (check 1, pre-F5 guest image): the warning named the peer and the host adopted as before — expected, since a pre-F5 acceptor cannot adopt.

## Gate

| command | result |
|---|---|
| `cargo check -p mesh -p secrets -p sandbox -p rpc -p theuth --tests` | exit 0 |
| `cargo clippy -p mesh -p secrets -- -D warnings` | exit 0, no warnings |
| `cargo test -p mesh -p secrets -p sandbox --lib` | mesh **28** (26 + `key_owner_is_peers_then_age_then_id`, `vault_key_stays_with_the_established_engine`) · secrets **23** (22 + `concurrent_first_opens_all_succeed`) · sandbox **41** — all passed |
| `cargo build -p theuth` (host `t` for the rig) | ok, three times |
| guest `t` (`/tmp/79/deb-build-f5.sh`, rust:1-bookworm, release) | 3 builds, 8 min each warm; `deploy/vm/Dockerfile.dev` bake + `msb load` 45–60 s |

## Deviations and decisions (nobody to ask)

1. **`VaultKeys` trait instead of the direct actor lookup.** The secrets actor is one per process (`ActorRef::lookup(SECRETS_ACTOR)`), so a two-store pairing test cannot give each `Net` its own vault; `BindOpts.keys: None` keeps production on the actor. One trait, two impls (prod + test) — the smallest seam that makes the test real.
2. **Both `PairHello` and `PairAck` carry the rank**, not only `PairHello` as the brief lists: the dialer must know the acceptor's rank to compute the same answer (and to tell "old peer" from "acceptor owns"). `adopted` is still the acceptor's word; the dialer only warns if it disagrees while it sent a key.
3. **"Naming the peer's version"**: neither `PeerEntry` nor the string carries a version, and a pre-F5 peer cannot send one, so the warning names the peer (`name (id)`) and the class (`pre-79-F5 t`). No version field added (the old side could never fill it).
4. **`paired_peers` counts `Net::paired_ids()`** (the live, non-tombstoned set at handshake time, before `learn`). The host's replicated `peers` table can contain its own row (seen in `/tmp/79smoke/mesh.sqlite`), which inflates the count symmetrically on every engine — harmless for the ordering. A host with 0 peers and a fresh VM tie on peers and fall to identity age; the host's identity always predates a guest's (created at the guest's first boot) — a same-millisecond tie would fall to the id and could go either way `[INFERENCE: not observed]`.
5. **Bring-up direction unchanged** (host → guest); the `pair` step itself needed nothing. `t mesh add <string>` from a fresh laptop into an existing mesh behaves exactly as before (test `c` dials `a`).
6. **Step 4 on the old image = the compat path, not the proof.** A pre-F5 guest cannot adopt, so the unrebuilt `theuth-vm:dev` can only show the warning and the old behaviour (one VM was enough to show it). The two-VM proof ran on the rebaked image (three times, as bugs 11 and 12 surfaced).
7. **Bug 11 (new): fresh-vault migration race.** The image enables `theuth.service` at boot, so the guest engine and the host's `first_run` CLI both migrate the fresh `secrets.sqlite` in the same second; sqlx's "list applied, then create" is not atomic, and either side can lose (`table vault|secret_requests already exists`). First fix: the `daemon` step restarts `theuth` after `first_run` (protects the daemon; also makes it start on the re-owned `/data` and the final `t` deterministically — the timing-dependent tail of smoke bug 1). Second, at the root: `secrets::store::open` migrates inside `BEGIN IMMEDIATE` (`Migrator::run_direct` on the transaction's connection — `run(&mut tx)` trips "implementation of `Send` is not general enough" in the plugin's `start`; `run_direct` is `#[doc(hidden)]` but public and exists for exactly this). Kept both: the restart costs nothing measurable (create → ready 13.9–15.1 s vs Z2's 15.4 s). The other `sqlx::migrate!` callers (mesh, roster, tasks, …) are opened by one process only and were left alone.
8. **Bug 12 (new): `config::mode()`** returned `Disabled` for the exact config the host writes (`enabled: false, guest: {…}`, §5.3), so Z2's "guest prompt block not observable" was this, not only the old guest binary. `guest` now wins; the test that pinned the wrong order was rewritten.
9. Build script: `/tmp/79/deb-build.sh` died silently after `ls` (`set -e` + a `grep` with no match as the last command of an `&&` list), which is why `deploy/vm/t.bookworm` was an empty directory. `/tmp/79/deb-build-f5.sh` tolerates it and writes `deploy/vm/t.f5` (gitignored, like `deploy/vm/t`).
10. Rig: the environment inherited `THEUTH_RPC__LISTEN=127.0.0.1:51908` from the parent app, which overrides `rpc.listen`; the smoke engine runs with it unset.
11. `crates/mesh/src/lease.rs` fails `cargo fmt --check` on `main` already (not touched); only my files were formatted.

## Tests written

- `pairing::tests::key_owner_is_peers_then_age_then_id` — six-row table, each also asserted with the roles swapped.
- `net::tests::vault_key_stays_with_the_established_engine` — three loopback nodes with per-node `MemKeys`: fresh dials established (adopts), established dials fresh (fresh adopts, established bytes unchanged).
- `store::tests::concurrent_first_opens_all_succeed` (secrets) — eight concurrent first opens of one file.
- `config::tests::mode_follows_the_switches` (sandbox) — updated for `guest` over `enabled`.

## Edits outside owned files (flagged)

- `crates/rpc/src/acp.rs:1148-1150` — the test's `BindOpts` literal gains `first_seen_ms`, `keys: None` (required by the struct change).
- `crates/sandbox/src/bringup.rs` `daemon` step — the one restart (bug 11; the brief allowed bringup only for the pair step, this was needed for the proof to run at all).
- `crates/sandbox/src/config.rs` — `mode()` order + test (bug 12; needed for step 5's masked-field check).
- `crates/secrets/src/store.rs` — `open()` migration lock + test (bug 11 root; "only what key adoption needs" — a guest without a vault cannot adopt).
- `docs/design/79-sandbox-vm.md` §13 (one amendment line), §16 (three rows, open list); `79-smoke.md` (bug 10 → fixed; "F5 proof" section).

## Not built / follow-ups

1. `67-agents.md` §5.4 / §4.1 text itself still says "the joiner adopts the inviter's key"; the amendment sentence is in 79 §13 as the brief asked — the 67 edit is the integrator's.
2. A version field in `PairHello`/`PairAck` (`env!("CARGO_PKG_VERSION")`) would let the compat warning name a real version from the next incompatible change on; not added (pre-F5 peers cannot send one).
3. `Net::rank()`'s `paired_peers` counts the replicated self-row when present (deviation 4); pruning self from `peers` is a separate mesh cleanup.
4. Rig left: `/tmp/79/{deb-build-f5.sh,f5-fp.sh,f5-build[1-4].log,f5-events[1-3].jsonl,f5-prompt*.out,f5-engine*.err}`, `/tmp/79smoke` (data dir; host vault now under the key adopted from Z2's `lab2` — fine, it is the smoke's). `deploy/vm/t` = the `c2f123c7` guest build (gitignored; the script's `t.f5` copy removed). `msb ls` / `msb snapshot ls` empty; no smoke engine running; `theuth-vm:dev` = `a1b28990893f` in Docker and msb.
