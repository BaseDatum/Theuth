# 210 as built — DEPLOY1 (share-edge image and rwx-dev manifests)

Branch `feat/210-public-share`. Scope: `deploy/share-edge/Dockerfile`, `deploy/share-edge/k8s/rwx-dev/`. Nothing
was applied to the cluster; kubectl use was read-only (`get`, `logs`, `kustomize`, `apply --dry-run=client`).

## Image

BUILD_RESULT

`deploy/share-edge/Dockerfile` (commit 6f6c70bd2), rewritten from EDGE1's `rust:1-alpine` stage:

- **Cross-compiled, no emulation.** The build stage is `FROM --platform=$BUILDPLATFORM rust:1-bookworm`; it maps
  `$TARGETARCH` to `x86_64-unknown-linux-musl` / `aarch64-unknown-linux-musl` and builds with
  `cargo zigbuild` (zig as C compiler and linker for aws-lc-sys and the bundled SQLite). Zig and
  cargo-zigbuild come from PyPI, pinned (`ziglang==0.13.0`, `cargo-zigbuild==0.24.1`). Output is still a static
  musl binary in `FROM scratch`, uid 65532, same ports/entrypoint as EDGE1.
- **Reproducible against the repo lock.** Only `Cargo.toml`, `Cargo.lock`, `rust-toolchain.toml`,
  `crates/{share-edge,share-proto,platform}` are copied. The workspace is trimmed to those three members and the
  `[patch.crates-io]` block (and everything after it) is cut, as for the relay. The one patched crate the edge
  reaches is `libsqlite3-sys` (through sqlx): without the patch cargo uses stock crates.io `libsqlite3-sys`,
  pinned with `cargo update -p libsqlite3-sys --precise 0.36.0` (the version `vendor/libsqlite3-sys` forks), built
  with its `bundled` SQLite; the vendored copy only adds SQLCipher-over-BoringSSL for the Signal store, which the
  edge does not use. The build then runs `--locked`, so any other drift from `Cargo.lock` fails the build.
- **Clean context.** The push was built from `git archive 6f6c70bd2` of exactly those paths plus the Dockerfile
  (`/tmp/share-edge-ctx-6f6c70bd2`), not the shared working tree, so other builders' uncommitted edits cannot leak
  into the image. `just share-edge-docker-push` still works but builds from the working tree.

## Manifests — `deploy/share-edge/k8s/rwx-dev/`

| file | in kustomization | content |
|---|---|---|
| `kustomization.yaml` | — | `../base` + `rbac.yaml`, namespace `theuth`, image tag (see Image); Deployment: `imagePullSecrets: ghcr-secret`, `nodeSelector kubernetes.io/arch: amd64`, `serviceAccountName: theuth-share-edge`, `automountServiceAccountToken: true` (base had false), emptyDir `run` at `/run/theuth-share-edge`; Service → `ClusterIP` TCP 8443 only; PVC → `storageClassName: nfs-csi` |
| `rbac.yaml` | yes | ServiceAccount, Role (`networking.k8s.io` `ingresses`: get, list, watch, create, update, patch, delete), RoleBinding; namespace `theuth` |
| `configmap.yaml` | yes (patch) | full `config.yaml`: `domain: tun.zip`, `admission: closed`, LE **staging** directory, `acme.contact: mailto:sky@rwx.dev` (sent verbatim as the ACME contact URI, `acme.rs` `register`), terms/abuse `example.com` placeholders, `require_caa: false`, `dns: porkbun / tun.zip / edge.tun.zip`, credential `/run/secrets/theuth-share-edge/dns.json` (existing Secret `theuth-share-edge`, key `dns.json`; not created or committed), `admin.socket` on the emptyDir, `listen_proxy_protocol.trusted: [10.244.0.0/16]`, `k8s:` templates (below) |
| `service-iroh-udp.yaml` | no (commented) | NodePort 30785/UDP → 7850, with the enable steps |

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
   pins it). `share-edge.sqlite` in WAL mode on NFS relies on NFS byte-range locks and a shared-memory `-shm`
   file that SQLite documents as unsafe over network filesystems. One writer (`replicas: 1`, `Recreate`) limits the
   damage, but a local-path/RBD class would be safer. `iroh.key` and `acme/` are fine on NFS.
4. Terms/abuse URLs and contact, then the prod directory.

## Doubts

DOUBTS

## Verification

VERIFICATION
