# 221 as built — B0 spike (Bitwarden SDK against Vaultwarden)

Date 2026-11-25. Spike: `/tmp/b221-spike` (standalone cargo project, outside the workspace, own
target dir), `bitwarden/sdk-internal` at `2b06afa3b3579af116cd60f83d65b9e411fd2a56` as git
dependencies with `no-memory-hardening`. Server: `vaultwarden/server:latest` = **1.37.5**
(`GET /api/version`), `SIGNUPS_ALLOWED=true`, `SENDS_ALLOWED=true`, plain HTTP on
`127.0.0.1:18221`. Debug build, so the SDK's `https_only` is off (§5.1); nothing else was needed for
plain HTTP.

**Not done:** the `vault.bitwarden.eu` run (no test account), and so the official server's
Premium-refusal text for file Sends (§12.3, §12.6) stays unrecorded. No organisation or collection
was created (a fresh Vaultwarden user has none), so G2/G3 ran on empty input only.

## Result

The whole §5 sequence ran on the **light set** (`bitwarden-core` + `-auth` + `-vault` + `-send` +
`-sync` + `-collections` + `-crypto` + `-api-api` + `-state`), with no `bitwarden-pm`:

```
register: 200 OK
api key client_id=user.c49c3129-3d6a-4ec6-a673-092fb415b8a0
login_api_key: authenticated=true in 2.081561958s; user_id after login=None
sync #1 full=true in 3.991583ms; orgs=[] collections=0
sync #2 (no change) full=false in 1.579458ms
list: 0 ok, 0 failed
created 0ad8cf48-eba2-412f-a924-0eee34b34e0b rev=2026-11-25 18:05:51.607747 UTC
edited rev=2026-11-25 18:05:52.715268 UTC
STALE EDIT ACCEPTED rev=2026-11-25 18:05:52.719512 UTC
decrypted password=Some("third-password") field=Some("tok-123") totp code len=6 history=2
sync #3 full=true; list=1
soft_delete ok; deleted_date=Some(2026-11-25T18:05:52.731077Z)
restore ok; deleted_date=None
text send link: http://127.0.0.1:18221/#/send/A68iPF2ER6yrdoxFOMwJBQ/7dwGxzkQhtdfpmz24rgmHQ
send opened via SDK: Some("the secret text")
file send created: upload type=Direct url=/sends/4ab86bf6-…/file/93b6cc16…
file send uploaded; link http://127.0.0.1:18221/#/send/Srhr9jq0RJie2WbfxjINGw/-TRYfFFmLcYhNK59XdgDog
after delete: get → Item does not exist
SPIKE OK
```

(The 2.1 s login is PBKDF2 600 000 iterations in an unoptimised debug build.)

## Calls that differ from §5 / §12 (amendments)

1. **No `bitwarden-pm`.** `PasswordManagerClient` is not needed: build with
   `bitwarden_core::ClientBuilder::new().with_settings(..).with_token_handler(Arc::new(bitwarden_auth::token_management::PasswordManagerTokenHandler::default())).build()`
   (what `PasswordManagerClientBuilder::build` does, minus the tracing middleware). Sub-clients come
   from extension traits: `VaultClientExt::vault`, `SendClientExt::sends`, `client.crypto()`,
   `client.auth()` (core's, which has `login_api_key`), `client.platform()`. `is_unlocked()` is
   `client.internal.get_key_store().context().has_symmetric_key(SymmetricKeySlotId::User)`. Lock
   is `client.internal.get_key_store().clear()` plus dropping the client (`unlock().lock()` lives in
   `bitwarden-unlock`, a `pm` dependency). §5.10's dependency list becomes the light set above.
2. **`bitwarden-core` needs feature `internal`** (for `auth().login_api_key`, `make_register_keys`,
   `init_user_id`), besides `no-memory-hardening`.
3. **User id (G5): from the API key, not the JWT.** `login_api_key` leaves `get_user_id() == None`
   (confirmed). A personal API key's `client_id` is `user.<uuid>`; theuth parses that and calls
   `client.internal.init_user_id(..)`. No token parsing. A non-`user.` client id is refused.
4. **Sync handlers (G6) confirmed:** `bitwarden_sync::SyncClient::new(client.clone())`, register
   `FolderSyncHandler::from_client(&client)`, `SendSyncHandler::from_client(&client)` (both via
   `FromClient`) and theuth's handler once. `last_sync` is a persisted value in the client's state, so
   the revision-date skip works across `SyncClient` instances too; the handlers do not. A no-change
   sync is one `GET /accounts/revision-date` (1.6 ms locally).
5. **G1 confirmed:** `Cipher::try_from(CipherDetailsResponseModel)` +
   `client.platform().state().get::<Cipher>()?.replace_all(..)` makes `ciphers().list/get/edit` work.
   `initialize_org_crypto(InitOrgCryptoRequest { organization_keys: HashMap<OrganizationId, UnsignedSharedKey> })`
   with an empty map is fine (no org on the test user).
6. **The server-side compare-and-swap is coarse — §4 "a real compare-and-swap" and §5.6 are
   amended.** The SDK sends `lastKnownRevisionDate` = the request's `revision_date`, truncated to
   whole seconds (`to_rfc3339_opts(SecondsFormat::Secs, ..)`, `cipher.rs#L459-L461`). An edit built
   on a revision 1.1 s stale was accepted by Vaultwarden 1.37.5 (`STALE EDIT ACCEPTED`); how much
   staleness the server tolerates was not measured further. So `secrets-bitwarden` checks
   `expected` itself against the freshly synced revision (exact instant compare) before the PUT, and
   maps a server "out of date" refusal to `Conflict` as a backstop. The window left is the time
   between that sync and the PUT plus the server's tolerance; documented, like AWS's
   describe-then-put race.
7. **Edit keeps password history:** after two password edits `password_history` has 2 entries
   (`update_password_history` in the SDK), newest first, which is what `#password:-N` reads.
8. **Send access works against Vaultwarden 1.37.5 with the SDK's own receive path**
   (`SendReceiveClient::request_send_access_token` with `SendPasswordCredentials` from
   `hash_send_password`, then `access_send` + `decrypt_send_access`): the text came back. B5c can
   use it in its suite. The link format `<web>/#/send/<access_id>/<url-b64 key>` (G8) is as §12.2.
9. **File Send, Direct:** `create_file_send` returned `file_upload_type = Direct` and a relative
   `url` (`/sends/<id>/file/<file_id>`); `upload_send_file(.., Direct, ..)` succeeded. No Premium on
   Vaultwarden, as §12.6 says.
10. **Registration (test fixture only):** Vaultwarden 1.37.5 has no `/identity/accounts/register`
    (404). It is two steps: `POST /identity/accounts/register/send-verification-email` (without SMTP
    the body *is* the verification token) then `POST /identity/accounts/register/finish` with
    `userSymmetricKey` / `userAsymmetricKeys` from `auth().make_register_keys`. The API key comes
    from a password-grant token + `POST /api/accounts/api-key { masterPasswordHash }`. The B5 suite
    does the same.
11. **`ClientSettings` has `..Default::default()`**, so custom URLs are two fields; `user_agent` is
    set to `theuth`.

## Dependency weight

Two standalone crates at the pin, each `cargo build` (debug, `CARGO_BUILD_JOBS=4`) into an empty
target dir: the spike on the light set, and `pm-weight` (`bitwarden-pm` with `no-memory-hardening`
and a one-line `main`).

| | light set (spike) | `bitwarden-pm` |
|---|---|---|
| packages in `Cargo.lock` | 406 | 477 |
| `bitwarden-*` crates | 30 | 48 |
| crates compiled | 317 | 394 |
| wall time, empty target dir | 53.5 s | 55.9 s |

The wall times are **not** cold builds: this machine runs cargo through `mbx`, whose object cache
outlives the target dir. The first, genuinely cold `cargo check` of the light set took 397 s on a
loaded machine. Read the crate counts, not the seconds: `pm` adds 18 SDK crates (exporters,
importers, fido, generators, policies, unlock, …) and 71 packages for nothing theuth calls.

Workspace effect (B5): adding the light set to theuth's lock adds the same 30 `bitwarden-*` crates;
`rusqlite` stays one package (0.38.0) on one `libsqlite3-sys` (0.36.0, the vendored SQLCipher
build), as §5.8 hoped.

## Consequence for B5

`secrets-bitwarden` uses the light set (amendment 1). The SDK pins `argon2 = "=0.6.0-rc.8"`
(`bitwarden-crypto`), which shares a semver slot with `crates/share`'s `argon2 = "0.6.0"`; cargo
cannot hold both. `crates/share` is held at `=0.6.0-rc.8` (its only use is `Argon2::default()`
PHC hash/verify in `gate.rs`; `cargo test -p share`: 72 passed). Drop the hold when the SDK pin
moves to argon2 0.6.0.
