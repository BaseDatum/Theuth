# 210 as built: FIX5-CLIENT

Branch `feat/210-public-share`, commit `1ad86c4a1`. Scope: `crates/share` only. Fixes the FIX4 re-review regressions R2 and R4 and makes short revoke answers fail.

## What was built

### R2: the card's name is claimed only after approval
- `Engine::publish_preview(&Plan)` claims nothing at the edge now. A held name is used as is (requested, else primary, else a name claimed earlier in this run). Otherwise the engine draws a random name locally (`names::random_name`, the same alphabet and length the edge validates) and checks it with the read-only `GET /v1/names/{name}` (`Control::check_name`). It draws again on `available: false`, up to 3 draws. The check is advisory: if it fails, the engine keeps the draw.
- `PublishPreview.allocated` is renamed to `PublishPreview.drawn`: the name is a local draw and has not been claimed.
- New: `Engine::publish_previewed(plan, origin, consent, preview)`. A held preview name is pinned (`plan.name`). A drawn name is passed as the *candidate* and claimed with `POST /v1/names` only now, after `Allow`. On `name_taken` it draws a fresh name and claims that, 3 attempts in total. The answer's `share.url` carries the final URL.
- `Engine::publish(plan, origin, consent)` keeps its signature (model tool path). Both go through the private `publish_with(…, candidate)`.
- Private `Claim` guard in `publish_admitted`: a name claimed by this call is released (`DELETE /v1/names/{name}`, on a spawned task) if the guard drops before the share row is stored. That covers every `?` and bail after the claim (the "already shared" check, the cert step, the hash, the final `still_approved`, store errors) and a caller dropping the future. `Claim::keep()` runs right after the row and approval are stored.
- `release_unused(name)` skips the release if a stored share of this engine already sits under that name. Otherwise it drops the name from `fresh` and releases it.
- `rpc_share::share_publish`: precheck → preview (no claim) → `Consent::ask` → `publish_previewed`. A refusal, a timeout or a dropped RPC before `Allow` claims nothing. `Engine::release_preview` is removed because nothing calls it any more.
- `pick_name` is split into a sync `held_pick` (shared with the preview) and the claim loop.

### R4: the local approval binds the expiry
- `store::binding(s)` now includes `s.expires_ms`. A replicated edit of `expires_ms` therefore fails `locally_approved`.
- New: `Store::rebind_approval(before, after) -> bool`. It is a compare-and-set on `share_approvals` (`share`, `approval_id`, `binding = binding(before)`). `after_cert` calls it when it takes a pending share live; the expiry comes from the local `share_requests` lifetime. The order is: rebind first, then `put` the row. If the rebind fails the share is not taken live. If the `put` fails the binding is moved back.
- The 30 s tick (`Engine::expire`) now also calls `may_mount` for every *routed* share of this engine. A share whose row no longer matches its local approval leaves the route table (stream reset through its stop token) and is listed in `unmounted()`, and routes are pushed again. Without this, an extended expiry only took effect at the next restore.
- New: `routes::Table::routes(&share) -> bool`.

### Revoke answers
- `duties::DialedEdges::revoke` checks the answer with `revoke_covers(asked, &RevokeResult)`. An answer whose `revoked` list lacks any engine that was asked for is an error, so the rekey row's revoke step fails and is retried. The edge now lists unknown ids too, so a full answer is expected.

## Public API changes (crate `share`)
- `PublishPreview { name, url, drawn }` (was `allocated`).
- `Engine::publish_previewed(Plan, CallOrigin, Consent, PublishPreview) -> anyhow::Result<Published>` (new).
- `Engine::release_preview` removed.
- `Store::rebind_approval(&Share, &Share) -> anyhow::Result<bool>` (new).
- `Table::routes(&str) -> bool` (new).
- No wire, RPC-schema or migration change.

## Deviations
- Approval rows written before this commit have no expiry in their binding. Live shares from an older build of this branch are therefore left unmounted after the upgrade and need a fresh publish. This only affects the unreleased branch, so no migration was written.
- After 3 `available: false` checks the preview keeps the last draw instead of failing, because the post-approval claim redraws anyway.

## Not built
- TODO(210-M3) is unchanged: a failed background cert step after a stored row still leaves the share `pending`. The name stays claimed there, which is intended, because the row exists.

## Doubts
- `fresh` race: two concurrent first publishes can share one claimed name, because `held_pick` returns a name in `fresh`. If the first fails and the second has not stored its row yet, `release_unused` (which checks stored shares only) can release a name the second is about to use. The window is narrow (between the claim and the second publish's row store) and needs a concurrent failure. A refcount on `fresh` would close it.
- The release runs on a spawned task through `with_control`, which waits for an attached link. If the engine detaches, the release waits until the next attach; if the process exits first, the name is never released (the edge's own idle-name expiry is then the backstop) [INFERENCE: not checked against share-edge].
- The `Claim` drop needs a tokio runtime handle. Without one (dropped outside a runtime) nothing is released. Publishes always run on the runtime.

## Verification (each its own lock hold, after the last edit)
- `cargo test -p share` → 57 passed, 0 failed. New or changed tests:
  - `engine::tests::preview_claims_nothing_and_a_failed_publish_gives_its_claim_back`: the preview makes no `POST /v1/names` and exactly one read-only check. After approval, `taken1` gets `name_taken`, a fresh draw is claimed, the publish fails at the cert step, and that name is released and dropped from `fresh`. A dropped `Claim` releases its name.
  - `engine::tests::an_extended_expiry_unbinds_the_local_approval`: a replicated expiry extension unbinds the row; the tick unroutes it, and restore does not remount it.
  - `engine::tests::cold_publish_answers_pending_promptly_and_goes_live_later`: now also asserts the approval moved with the go-live expiry.
  - `duties::tests::a_revoke_answer_missing_an_engine_fails`.
- `cargo clippy -p share --all-targets --no-deps -- -D warnings` → clean.
- `cargo test -p rpc-compose --test wire_golden` → 2 passed.
- No Cargo.lock change.
