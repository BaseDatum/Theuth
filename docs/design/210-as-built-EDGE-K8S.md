# 210 as built — EDGE-K8S phase (per-name Kubernetes objects, PROXY protocol inbound)

Branch `feat/210-public-share`. Design `docs/design/210-public-share.md` rev 8 plus the owner's decision for the
HAProxy passthrough frontend: the edge itself creates and deletes one object set per name from config templates,
with no controller-specific code. Builds on EDGE1/EDGE2/FIX-EDGE. Also carries the three FIX-EDGE gate fixes the
orchestrator asked for (below).

## Files

| path | what |
|---|---|
| `crates/share-edge/Cargo.toml` | feature `k8s = ["dep:kube", "dep:k8s-openapi"]`; optional `kube` 5.1 (`client`, `rustls-tls`, `aws-lc-rs`, no defaults), `k8s-openapi` 0.29 (`v1_33`). Both Apache-2.0. No root `Cargo.toml` change |
| `src/k8s.rs` | always compiled: `K8sConfig`, substitution, slug, desired set, diff, `K8s` manager over the `KubeApi` trait, `granted`/`gone` hooks, the `run` loop; unit tests with a fake API |
| `src/k8s/client.rs` | feature `k8s` only: `KubeClient` (`Config::infer`, `discovery::pinned_kind` cached, `Api<DynamicObject>`, SSA) |
| `src/config.rs` | `Config::{k8s, listen_proxy_protocol}`, `ProxyProtocolIn`, `Cidr`; `validate` checks the k8s templates; `dur` now `pub(crate)` |
| `src/sni.rs` | `read_proxy_header`, `Refuse::Proxy`, `PROXY_HEADER_MAX`, `PROXY_HEADER_TIMEOUT`; `visitor` parses PROXY from trusted peers |
| `src/state.rs` | `Edge::k8s: OnceLock<Arc<K8s>>` |
| `src/store.rs` | `Store::serving_names(domain)`; `ClaimRow` alias (FIX-EDGE clippy) |
| `src/control.rs` | claim (`Granted`/`AlreadyHeld`) → `k8s::granted`; release → `k8s::gone` |
| `src/admin.rs` | `ban name` → `k8s::gone`; short admin-socket staging dir (FIX-EDGE test fix) |
| `src/lib.rs` | `start_with` builds the manager before binding (when enabled), spawns `k8s::run`; `/metrics` adds `share_edge_k8s_errors_total` |
| `src/certs.rs` | unused `Context` import removed (FIX-EDGE clippy) |
| `tests/common/mod.rs` | `fetch_update` → `try_update` (deprecated on rustc 1.99; `-D warnings` failed the test targets) |

## Config keys (for DEPLOY1)

Build the image with `--features k8s` (DEPLOY1's Dockerfile already takes `--build-arg FEATURES=k8s`). Without the
feature, `k8s.enabled: true` makes `start` fail with "needs a build with the cargo feature `k8s`".

```yaml
listen_proxy_protocol:
  trusted: ["10.244.0.0/16"]     # CIDRs or bare addresses; default [] (never parse PROXY)
k8s:
  enabled: false                 # default; false = no client constructed, zero API calls
  namespace: ""                  # default "" = Config::infer's namespace (in a pod: the service-account file)
  field_manager: theuth-share-edge   # SSA field manager and the managed-by label value (a valid label value)
  resync: 5m                     # humantime or seconds; >= 10s
  static:                        # applied until it succeeds once after start; variables ${domain} ${namespace}
    - {apiVersion: networking.k8s.io/v1, kind: Ingress, metadata: {name: share-edge-static}, spec: {...}}
  per_name:                      # per serving name; ${name} ${domain} ${host} ${wildcard} ${slug} ${namespace}
    - apiVersion: networking.k8s.io/v1
      kind: Ingress
      metadata: {name: "share-edge-name-${slug}"}
      spec:
        ingressClassName: haproxy
        rules: [{host: "${wildcard}", http: {paths: [{path: /, pathType: Prefix,
                 backend: {service: {name: theuth-share-edge, port: {number: 8443}}}}]}}]
```

Env overrides follow the existing figment rule (`THEUTH_SHARE_EDGE_K8S__ENABLED=true`). DEPLOY1's
`deploy/share-edge/k8s/rwx-dev/configmap.yaml` already matches these keys; its `rbac.yaml` (Role on `ingresses`:
get, list, create, patch, delete, …) is what the code needs: discovery endpoints are readable by any authenticated
SA by default.

## Behaviour

- **Templates**: plain YAML objects, any `apiVersion`/`kind`. `${var}` is substituted in every string value, never
  in keys; `$` not followed by `{` is literal; nothing is evaluated. At config load (only when `enabled`): every
  template must be an object with string `apiVersion`, `kind`, `metadata.name`; an unknown or unterminated
  variable is an error naming it; static templates know only `${domain}`/`${namespace}`; a per-name template's
  `metadata.name` must change with the name and its `metadata.namespace` must not.
- **`${slug}`**: lowercase; every character outside `[a-z0-9-]` → `-`; leading/trailing `-` trimmed; at most 40
  characters (longer: first 31 + `-` + 8 hex FNV-1a of the whole name); never empty (`x`). Share names are
  already DNS labels, so in practice it is the name.
- **Stamping**: labels `app.kubernetes.io/managed-by=<field_manager>` on every object, plus
  `theuth.dev/share-name=<name>` on per-name objects; merged into the template's labels, the edge's win. A
  namespaced kind gets `metadata.namespace` = the template's or `k8s.namespace`; a cluster-scoped kind has it
  removed (scope from discovery).
- **Apply**: server-side apply with `field_manager`, `force = false`; a conflict is an error (logged, counted,
  retried), never overridden.
- **Ownership**: the edge lists only with selector `managed-by=<field_manager>` (and `share-name=<name>` for a
  release) and deletes only what such a list returns. Static objects (no share-name label) are never deleted.
- **Lifecycle**: a claim that is granted or already held spawns `apply_name` (checks under the manager's lock that
  the name is still held, then applies its objects), retried with backon (1 s → 30 s, 4 retries, jitter); a final
  failure wakes the reconcile loop. Release and `admin ban name` spawn `delete_name` (list by both labels, delete
  each) with the same retry. Expiry/eviction are not built in the edge yet (M3/M4); the periodic reconcile
  deletes objects of any name that left the serving set however it left.
- **Reconcile** (`k8s::run`): static objects until they succeed once; then a pass at start and every `resync`
  (sooner after a failing pass: 30 s doubling up to `resync`; at once on a nudge). A pass: desired =
  per-name templates × `Store::serving_names` (states `allocated|queued|slotted|valid`, `banned IS NULL`); apply
  every desired object (SSA is a no-op when nothing drifted and restores what did); list managed objects of each
  per-name kind/namespace; delete those with a share-name label not in desired. One async mutex serializes passes,
  claims and releases so a release cannot be undone by a pass that read the names before it.
- **Errors**: every failed call is `tracing::warn!` and `share_edge_k8s_errors_total` += 1 (on `/metrics` when
  enabled). Claims never wait on k8s.
- **Disabled**: `Edge::k8s` stays unset, `granted`/`gone` return at once, no client is built.
- **PROXY inbound**: on the visitor listener, if the peer address is in `listen_proxy_protocol.trusted`
  (v4-mapped v6 matched as v4), the edge requires a PROXY v1 or v2 header (`ppp::HeaderResult::parse`) within
  5 s and 4 KiB, before the ClientHello; bytes read past the header are fed to the ClientHello peek first. The
  header's source address replaces the peer for the per-IP rate and count, `conn_log` and the PROXY v2 preamble
  to the engine. v2 `LOCAL`, v1 `UNKNOWN` and non-IP families keep the LB's own address. No/malformed/oversized/
  slow header → TCP reset. Untrusted peers are never parsed. The global visitor semaphore is taken before the
  header read.

## Public API added

```rust
share_edge::k8s::{K8sConfig, Vars, TypeKey, ObjKey, Obj, Listed, Pass, K8s, KubeApi,
                  MANAGED_BY, SHARE_NAME, SLUG_MAX, slug, render, diff, granted, gone, run}
share_edge::k8s::client::KubeClient            // feature `k8s`: KubeClient::infer(field_manager) -> (client, ns)
trait KubeApi: Send + Sync {
    fn namespaced(&self, &TypeKey) -> BoxFuture<Result<bool>>;
    fn apply(&self, &Obj) -> BoxFuture<Result<()>>;
    fn delete(&self, &ObjKey) -> BoxFuture<Result<()>>;           // absent = Ok
    fn list(&self, &TypeKey, namespace: Option<&str>, selector: &str) -> BoxFuture<Result<Vec<Listed>>>;
}
K8s::{new, desired, apply_static, reconcile, apply_name, delete_name}; K8s::errors: AtomicU64
config::{ProxyProtocolIn { trusted }, Cidr}; ProxyProtocolIn::trusts(IpAddr); Cidr::contains; Cidr: FromStr
sni::{read_proxy_header, PROXY_HEADER_MAX, PROXY_HEADER_TIMEOUT, Refuse::Proxy}
Store::serving_names(domain) -> Vec<String>
Edge::k8s: OnceLock<Arc<K8s>>
```

## FIX-EDGE gate fixes (orchestrator request)

1. `src/certs.rs`: unused `anyhow::Context` import removed.
2. `src/store.rs`: `type ClaimRow` for `Store::claim`'s row tuple (clippy `type_complexity`).
3. `src/admin.rs::bind`: staging dir `.ab<8 hex>` next to the socket and staging socket name `s` (was
   `.admin-bind-<pid>-<16 hex>/admin.sock`), so the staging path is only 2 bytes longer than
   `<dir>/admin.sock` and fits `SUN_LEN` (104 on macOS) under a long `TMPDIR`. The guarantee is unchanged: the
   socket is bound inside a fresh 0700 directory (`create`, not `create_all`, so a collision fails), chmod 0600,
   then renamed into place.

## Deviations from the brief, and why

- **`kube` without the `runtime` feature**: nothing here uses watchers/controllers (a periodic list + SSA is
  the whole reconcile); `runtime` would add crates for no use.
- **Config key `listen_proxy_protocol`** (brief) instead of the appendix's `proxy_protocol_in: bool`; the appendix
  key is ignored (unknown keys are ignored) and `deploy/share-edge/config.example.yaml` still shows it
  (DEPLOY1's file).
- **Static objects are applied until they succeed once**, not re-applied every resync (brief: "applied once at
  start"); drift of a static object is not repaired until restart.
- **Drift**: every pass re-applies every desired object instead of comparing; SSA makes an unchanged apply a
  no-op server-side. Cost: names × templates PATCH calls per `resync`.
- **`k8s-openapi` `v1_33`** (earliest the 0.29 line supports): only kube's own types use it; DynamicObject is
  version-independent.

## Not built

- `TODO(210-M3)`/`TODO(210-M4)`: the edge has no inactivity expiry or eviction yet; when they land they must call
  `k8s::gone` (the periodic reconcile already removes their objects within `resync`).
- Objects of a kind removed from `per_name` are no longer listed, so they stay until deleted by hand
  (`kubectl delete ingress -l app.kubernetes.io/managed-by=theuth-share-edge`).
- No test against a real API server (no cluster in the brief); `KubeClient` is compiled and clippy-clean only.

## Doubts

- `Config::infer` falls back to kubeconfig outside a pod, so an `enabled` edge on a dev laptop talks to the
  current context. Intended for `kubectl`-style testing, but worth knowing.
- `granted` runs on `AlreadyHeld` too (idempotent re-claims re-apply); cheap, and repairs a missed apply.
- A trusted LB that sends nothing (TCP health check) is reset after 5 s or at EOF; fine for HAProxy
  (`send-proxy-v2` on every connection) but a plain TCP health check logs nothing and costs a visitor slot for
  up to 5 s.
- The preamble's destination stays the edge's own socket address, not the header's destination.

## For the orchestrator

- `Cargo.lock` changes (not committed): new packages `kube`, `kube-client`, `kube-core`, `k8s-openapi`,
  `jsonpath-rust`, `secrecy`, `serde-saphyr`, `serde-value`, `ordered-float`, `nohash-hasher`, `granit-parser`,
  `annotate-snippets` (feature `k8s` only). DEPLOY1's Dockerfile builds with `--locked`, so the lock must be
  committed before an image with `FEATURES=k8s` can build.

## Verification

Each command ran under its own hold of `/tmp/theuth-build.lock`, after the last code edit, rustc 1.99.0:

- `CARGO_BUILD_JOBS=6 cargo clippy -p share-edge --all-targets --all-features -- -D warnings`: clean.
- `CARGO_BUILD_JOBS=6 cargo test -p share-edge --all-features`: lib 25 passed (new: `k8s::tests::{substitutes_
  every_variable_in_values_only, unknown_or_unterminated_variables_are_refused, config_validation, slug_rules,
  desired_set_is_templates_times_names_with_stamped_labels, diff_deletes_only_undesired_per_name_objects,
  reconcile_applies_and_deletes_and_lifecycle_ops}`, `sni::tests::{proxy_v1_source_is_the_visitor,
  proxy_v2_source_is_the_visitor, proxy_missing_or_malformed_is_refused, proxy_header_times_out}`,
  `config::tests::{proxy_protocol_cidrs, k8s_section_loads_and_is_checked}`); `tests/certs.rs` 12 passed;
  `tests/visitor.rs` 2 passed (`visitor_hello_reaches_the_engine_behind_proxy_v2`, `control_gates`, which
  failed on macOS before the admin-socket fix).
- `cargo tree -p share-edge -e normal` (default features): path crates `share-edge`, `share-proto`, `platform`,
  `vendor/libsqlite3-sys` only; no `kube`/`k8s-openapi` lines.
- Not run: anything against a real API server; the end-to-end visitor test with a trusted PROXY peer (the parse
  and the address substitution are unit-tested; the `visitor` wiring is three lines and covered by review only).
