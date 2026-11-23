# 210 as built — DEPLOY1 (share-edge image and rwx-dev manifests)

Branch `feat/210-public-share`. Scope: `deploy/share-edge/Dockerfile`, `deploy/share-edge/k8s/rwx-dev/`. Nothing
was applied to the cluster; kubectl use was read-only (`get`, `logs`, `kustomize`, `apply --dry-run=client`).

## Image

**`ghcr.io/skymoore/theuth/share-edge:share-edge-387c4ec28`** (also `:latest`), index digest
`sha256:d24411f11a23246a9b5679065ce479126e3c9aebb7dfb0f0c02a348a063b27de`: `linux/amd64`
(`sha256:a3fecbe0…`) and `linux/arm64` (`sha256:119183ca…`), built with `--build-arg FEATURES=k8s` from
`git archive 387c4ec28` (EDGE-K8S code + its committed Cargo.lock). Build time 538 s with both platforms
compiled in parallel on the arm64 Mac (amd64 cross-compiled, no QEMU); not counting the build-lock wait.
The first attempt (6f6c70bd2, no features) failed in 30 s at the libsqlite3-sys pin (below); fixed in f431ef768.

`deploy/share-edge/Dockerfile` (6f6c70bd2, f431ef768, `FEATURES` arg 94a4f2057), rewritten from EDGE1's
`rust:1-alpine` stage:

- **Cross-compiled, no emulation.** The build stage is `FROM --platform=$BUILDPLATFORM rust:1-bookworm`; it maps
  `$TARGETARCH` to `x86_64-unknown-linux-musl` / `aarch64-unknown-linux-musl` and builds with
  `cargo zigbuild` (zig as C compiler and linker for aws-lc-sys and the bundled SQLite). Zig and
  cargo-zigbuild come from PyPI, pinned (`ziglang==0.13.0`, `cargo-zigbuild==0.24.1`). Output is still a static
  musl binary in `FROM scratch`, uid 65532, same ports/entrypoint as EDGE1.
- **Reproducible from the committed lock.** Only `Cargo.toml`, `Cargo.lock`, `rust-toolchain.toml`,
  `crates/{share-edge,share-proto,platform}` are copied. The workspace is trimmed to those three members and the
  `[patch.crates-io]` block (and everything after it) is cut, as for the relay. The one patched crate the edge
  reaches is `libsqlite3-sys` (through sqlx). With the patch gone the lock's path entry is orphaned, so
  `cargo update -p libsqlite3-sys` alone fails ("did not match any packages"); the Dockerfile first runs
  `cargo metadata` (cargo keeps every existing lock entry and adds crates.io `libsqlite3-sys` 0.37.0), then
  `cargo update -p libsqlite3-sys --precise 0.36.0` (the version `vendor/libsqlite3-sys` forks; build log:
  `Adding libsqlite3-sys v0.37.0`, `Downgrading libsqlite3-sys v0.37.0 -> v0.36.0`). Checked on the host with
  the same steps: no name/version pair in the trimmed lock that was not in the repo lock (only pruning and
  libsqlite3-sys path → registry). Built with its `bundled` SQLite; the vendored copy only adds
  SQLCipher-over-BoringSSL for the Signal store, which the edge does not use. The build then runs `--locked`.
- **Clean context.** The push was built from `git archive 387c4ec28` of exactly those paths plus the Dockerfile,
  not the shared working tree, so other builders' uncommitted edits cannot leak into the image.
  `just share-edge-docker-push` still works but builds from the working tree and without `FEATURES`; use
  `docker buildx build --build-arg FEATURES=k8s …` (Verification) for the rwx-dev image.

## Manifests — `deploy/share-edge/k8s/rwx-dev/`

| file | in kustomization | content |
|---|---|---|
| `kustomization.yaml` | — | `../base` + `rbac.yaml`, namespace `theuth`, image tag (see Image); Deployment: `imagePullSecrets: ghcr-secret`, `nodeSelector kubernetes.io/arch: amd64`, `serviceAccountName: theuth-share-edge`, `automountServiceAccountToken: true` (base had false), emptyDir `run` at `/run/theuth-share-edge`; Service → `ClusterIP` TCP 8443 only; PVC → `storageClassName: nfs-csi` |
| `rbac.yaml` | yes | ServiceAccount, Role (`networking.k8s.io` `ingresses`: get, list, watch, create, update, patch, delete), RoleBinding; namespace `theuth` |
| `configmap.yaml` | yes (patch) | full `config.yaml`: `domain: tun.zip`, `admission: closed`, LE **staging** directory, `acme.contact: mailto:sky@rwx.dev` (sent verbatim as the ACME contact URI, `acme.rs` `register`), terms/abuse `example.com` placeholders, `require_caa: false`, `dns: porkbun / tun.zip / edge.tun.zip`, credential `/run/secrets/theuth-share-edge/dns.json` (existing Secret `theuth-share-edge`, key `dns.json`; not created or committed), `admin.socket` on the emptyDir, `listen_proxy_protocol.trusted: [10.244.0.0/16]`, `k8s:` templates (below) |
| `service-iroh-udp.yaml` | no (commented) | NodePort 30785/UDP → 7850, with the enable steps |
| `acme-register-job.yaml` | no (one-off) | Job `acme register --ca letsencrypt` on the data PVC; see “First start” |

Ingresses are not committed: per the owner's 13:53 decision the edge (cargo feature `k8s`, EDGE-K8S) creates them
from `k8s.static` (one `share-edge-static`, host `*.tun.zip`) and `k8s.per_name` (`share-edge-name-${slug}`, host
`*.${name}.tun.zip`). Both: `ingressClassName: haproxy`, `haproxy.org/ssl-passthrough: "true"`,
`haproxy.org/send-proxy-protocol: proxy-v2`, `haproxy.org/timeout-server`/`timeout-tunnel: 3600s`, backend
`theuth-share-edge:8443`. PROXY sources: kubeadm `podSubnet` 10.244.0.0/16 (`kubeadm-config`); the HAProxy
DaemonSet is not `hostNetwork`, its pods are 10.244.x.

`kubectl kustomize deploy/share-edge/k8s/rwx-dev` renders; `kubectl apply --dry-run=client` accepts the render and
`service-iroh-udp.yaml`; both templates, with `${slug}`/`${name}` = `sky`, pass `kubectl apply --dry-run=client`.

## HAProxy SNI passthrough — what the running controller can do

Running: `haproxytech/kubernetes-ingress` **v3.2.14** (HAProxy 3.2.23), DaemonSet
`haproxy-controller/haproxy-kubernetes-ingress` (there is no Deployment, so `get deploy` shows nothing); version
from the image labels of digest `sha256:d90f628d…f5ae`. Installed CRDs: only `backends.ingress.v1.haproxy.org`.

- Passthrough is a TCP frontend `ssl` on :443 with an `sni` map. An Ingress host `*.tun.zip` becomes map key
  `.tun.zip`; lookup is the exact SNI, then the SNI with its first label cut
  ([pkg/handler/https.go#L345-L376](https://github.com/haproxytech/kubernetes-ingress/blob/v3.2.14/pkg/handler/https.go#L345-L376),
  [pkg/route/route.go#L75-L94](https://github.com/haproxytech/kubernetes-ingress/blob/v3.2.14/pkg/route/route.go#L75-L94)).
  So `*.tun.zip` serves `<name>.tun.zip` but not `<label>.<name>.tun.zip` (D5); that is why each name gets its own
  `*.<name>.tun.zip` Ingress (cut label → `.<name>.tun.zip` matches). Without it those SNIs fall to the
  TLS-terminating https frontend and get the controller's certificate. This settles the appendix §8.4 open
  question (“whether haproxytech's passthrough SNI map matches wildcards deeper than the spec is not checked”).
  Depth is one label below each Ingress host; `a.b.<name>.tun.zip` would not route (D5 does not allow it).
- No hook adds `req_ssl_sni -m end .tun.zip` to the `ssl` frontend: `frontend-config-snippet` goes into http/https
  only ([annotations.md#frontend-config-snippet](https://github.com/haproxytech/kubernetes-ingress/blob/v3.2.14/documentation/annotations.md#frontend-config-snippet));
  the Frontend CR covers http/https/stats only
  ([custom-resources.md#frontend](https://github.com/haproxytech/kubernetes-ingress/blob/v3.2.14/documentation/custom-resources.md#frontend));
  `haproxy-aux.cfg` cannot extend a generated frontend. The TCP CR
  ([custom-resource-tcp.md](https://github.com/haproxytech/kubernetes-ingress/blob/v3.2.14/documentation/custom-resource-tcp.md))
  takes acl/use_backend rules but is a new frontend on its own port, and its CRD `tcps.ingress.v3.haproxy.org` is
  not installed. Dropped per the owner's decision (commit 94a4f2057 deletes the draft `haproxy-passthrough.yaml`).
- Per-name Ingress churn reloads HAProxy config on all 13 controller pods (the controller updates map files at
  runtime where it can; a new backend needs a reload) [INFERENCE from the controller design, not measured].

## Deviations from the design

- §8.4 / appendix §8.4 recommend a `LoadBalancer` Service with its own IP. rwx-dev has no LB controller, so the
  overlay is ClusterIP behind HAProxy passthrough with edge-managed Ingresses (owner decision 13:53).
- `admin.socket` moved off the data volume to an emptyDir (Unix socket on NFS).
- `require_caa: false` (code allows it with `closed`).

## Not built

- TODO(210-M1): UDP 7850 exposure (`service-iroh-udp.yaml` not enabled; `iroh.public_addrs` empty, clients use
  the relay).
- Production config: prod ACME directory, terms/abuse pages, `require_caa: true` with CAA records,
  admission `invite`/`open`.

## Owner decisions

1. Router: public 443 → 192.168.8.99 already serves HAProxy; `edge.tun.zip` must resolve to that public address.
2. Router UDP forward for 7850 → NodePort 30785.
3. **SQLite on NFS.** The only StorageClass is `nfs-csi` (nfs.rwx.dev, NFSv4.1; not marked default, so the PVC
   pins it). *Updated (FIX4-EDGE #22):* the overlay now sets `THEUTH_SHARE_EDGE_DB_JOURNAL=delete` (owner
   2026-11-23): rollback journal instead of WAL, so no shared-memory `-shm` file on NFS; `Store::open` with
   `DbJournal::Delete` takes `locking_mode=EXCLUSIVE` and a one-connection pool, so the single writer
   (`replicas: 1`, `Recreate`) holds the file lock for its lifetime. It still relies on NFSv4 byte-range locks; a
   local-path/RBD class would remain safer. `iroh.key` and `acme/` are fine on NFS.
4. Terms/abuse URLs and contact, then the prod directory.

## First start (owner, in order)

`run` refuses to start without `<data_dir>/acme/letsencrypt.json` (seen in the smoke run below), and
`acme register` writes that file only if it is absent, so it cannot run as an initContainer (the scratch image has
no shell to guard a re-run). Steps are in the header of `acme-register-job.yaml`: apply the overlay, scale to 0, run
the Job, scale to 1. **Blocked by the rustls bug below:** the Job panics today.

## Blocker found: rustls CryptoProvider panic (crates/share-edge, not DEPLOY1's)

`acme register` in the built image (`--network none`) panics before any I/O:
`Could not automatically determine the process-level CryptoProvider from Rustls crate features … make sure exactly
one of the 'aws-lc-rs' and 'ring' features is enabled` (rustls 0.23.46, `crypto/mod.rs:249`). In the trimmed lock
of 387c4ec28 `rustls` depends on both `aws-lc-rs` and `ring`, so some dependency of the edge's own graph turns on
`ring` [INFERENCE: which one is unchecked; `cargo tree -e features -i rustls` in the trimmed workspace will say].
The host tests do not see it because the full workspace's feature set differs. `run` will hit the same panic at
its first rustls client (ACME, kube) [INFERENCE]. Fix (share-edge owner): at the top of `main`,
`rustls::crypto::aws_lc_rs::default_provider().install_default()`, then rebuild the image. Whether the non-k8s
build has it too is unchecked.

## Doubts

- The rustls blocker above. Until it is fixed the image starts, loads the config and stops at the ACME account
  check; nothing past that has been exercised.
- SQLite on NFS (owner decision 3): delete journal + exclusive lock since FIX4-EDGE; still NFS locking.
- Per-name Ingress churn reloads HAProxy on 13 controller pods [INFERENCE].
- The base Deployment's probes use `/healthz` on 9090. While the pod has no ACME account it exits, so it
  CrashLoops until the Job has run; expected.
- `latest` now points at a `FEATURES=k8s` image; a plain-host user pulling `latest` gets the k8s client too
  (harmless while `k8s.enabled: false`).

## Verification

```
# image (under /tmp/theuth-build.lock)
git archive 387c4ec28 Cargo.toml Cargo.lock rust-toolchain.toml crates/share-edge crates/share-proto \
  crates/platform deploy/share-edge/Dockerfile | tar -x -C /tmp/share-edge-ctx-387c4ec28
docker buildx build --builder desktop-linux --platform linux/amd64,linux/arm64 --build-arg FEATURES=k8s \
  -f /tmp/share-edge-ctx-387c4ec28/deploy/share-edge/Dockerfile \
  -t ghcr.io/skymoore/theuth/share-edge:share-edge-387c4ec28 -t ghcr.io/skymoore/theuth/share-edge:latest \
  --push /tmp/share-edge-ctx-387c4ec28                                       # rc=0, 538 s
docker buildx imagetools inspect ghcr.io/skymoore/theuth/share-edge:share-edge-387c4ec28
                                                     # linux/amd64 + linux/arm64 (+ attestation manifests)
# smoke, no network (no Let's Encrypt / Porkbun), rendered overlay config + fake dns.json
docker run --rm --network none --platform linux/arm64 … share-edge-387c4ec28
                                                     # config + k8s templates accepted; stops: "no ACME account for letsencrypt"
docker run --rm --network none --platform linux/arm64 … share-edge-387c4ec28 acme register --ca letsencrypt
                                                     # rustls CryptoProvider panic (above)
docker run --rm --network none --platform linux/amd64 --entrypoint /theuth-share-edge … --help   # runs
# manifests (read-only cluster access)
kubectl kustomize deploy/share-edge/k8s/rwx-dev      # renders; image share-edge-387c4ec28
kubectl apply --dry-run=client -n theuth -f <render> -f service-iroh-udp.yaml -f acme-register-job.yaml   # accepted
# k8s templates with ${slug}/${name}=sky through kubectl apply --dry-run=client: share-edge-static, share-edge-name-sky
```

Cluster facts used (read-only): HAProxy DaemonSet image labels (v3.2.14), `kubectl get nodes -L kubernetes.io/arch`,
`kubeadm-config` podSubnet 10.244.0.0/16, `get sc` (nfs-csi only), `get secret theuth-share-edge` (key `dns.json`),
`get svc` in `theuth` (iroh-relay-quic LoadBalancer `<pending>`), no NodePort 30785 in use, no existing `tun.zip`
Ingress.
