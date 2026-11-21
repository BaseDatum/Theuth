# Rust toolchain lives under rustup but homebrew's rustup does not install
# cargo/rustc proxies on PATH. Every recipe prepends the active toolchain's bin.
toolchain_bin := `dirname "$(rustup which cargo)"`
export PATH := toolchain_bin + ":" + env("PATH")

# Gates (check/test/clippy/durability) build without incremental caches: they add ~1 GB per crate
# per session to target/ and only pay off for the second build of an edited crate. Editing and
# running tests in a loop? `CARGO_INCREMENTAL=1 just test -p agent`.
incremental := env("CARGO_INCREMENTAL", "0")

# Print the toolchain bin dir; `export PATH="$(just toolchain):$PATH"` in any shell.
toolchain:
    @echo {{toolchain_bin}}

# Ensure the stable toolchain from rust-toolchain.toml is installed.
setup:
    rustup show active-toolchain || rustup toolchain install stable
    @echo "cargo: $(rustup which cargo)"

build *ARGS:
    cargo build {{ARGS}}

check *ARGS: apps-bundle-check
    CARGO_INCREMENTAL={{incremental}} cargo check --workspace --all-targets {{ARGS}}

test *ARGS:
    CARGO_INCREMENTAL={{incremental}} cargo test --workspace {{ARGS}}

# Design doc 109 §11: the crash-injection suite. The §11.1 matrix (every crash point, spawned
# children that abort, resume, transcript check), the drain/suspend path, the journal store tests,
# the reaper, waits/questions re-arm, the ACP respawn trigger, and the jobs/PTY tests in `tools`.
# Crate-level by name; ~1 minute on an M-series laptop. `--test-threads=1` is the sqlx teardown
# workaround (00 §12) for the `tools` half only; the `agent` half serialises itself.
durability:
    CARGO_INCREMENTAL={{incremental}} cargo test -p agent --lib -- loop_::durability_tests durability:: store::durability supervisor::waits supervisor::drain supervisor::resume supervisor::acp_resume
    CARGO_INCREMENTAL={{incremental}} cargo test -p tools --lib -- --test-threads=1 jobs:: pty::

# Design doc 139 §11 Z: the two-engine matrix. In-process (~15 s of tests): the two-store rows that
# need no running turn (`sessionsync/tests/mesh_durability.rs`: T9 prune through the log, replica
# display/act scope) plus the two-engine unit tests of each wave (owner.rs takes, release drain,
# reaper engine filter, child waits, routed answers; the tools half — KillSession, lost PTY page —
# is `just durability`'s; R2's harness outbox over the stream and reminders-follow-the-owner). The
# rows that need a live turn (T1–T8, R2's T10/T10b) are the scripted smoke against two real engines:
# `just durability-mesh smoke` (not CI; ~7 min; needs bun and free ports 7931/7932/7941/7942/7950;
# evidence in docs/design/139-smoke.md and 139-as-built-R2.md).
durability-mesh *ARGS:
    #!/usr/bin/env bash
    set -euo pipefail
    set -- {{ARGS}}
    export CARGO_INCREMENTAL={{incremental}}
    cargo test -p sessionsync --test mesh_durability
    cargo test -p sessionsync --lib -- owner:: harness_ types::
    cargo test -p agent --lib -- supervisor::transfer supervisor::resume supervisor::drain supervisor::waits supervisor::child_waits durability::jobs loop_::durability_tests::drain_suspends loop_::resume_calls store::durability store::harness
    cargo test -p tools --lib -- --test-threads=1 cancel_reminders_stops
    cargo test -p rpc-compose --lib -- forward routed_methods answers_follow durability_wire_shapes
    if [ "${1:-}" = "smoke" ]; then shift; scripts/durability-mesh-smoke.sh "$@"; fi

# 146 RESTART-CLARITY proof (P1–P6): one real engine on a scratch data dir, the real `omp`
# harness as the child, `kill -9` + reboot. Not CI; ~8 min; needs bun, omp and free ports
# 7960–7966; `BACKUP_DB=<.backup copy> just restart-clarity P6` times the boot query.
restart-clarity *ARGS:
    scripts/restart-clarity-smoke.sh {{ARGS}}

clippy:
    CARGO_INCREMENTAL={{incremental}} cargo clippy --workspace --all-targets -- -D warnings

# Remove target/ artifacts no build has used for three days (cargo keeps every stale hash forever;
# a checkout reached 167 GB). `cargo install cargo-sweep` once. `just sweep 0` clears the lot.
sweep days="3":
    cargo sweep --time {{days}} 2>/dev/null || (echo "cargo-sweep missing: cargo install cargo-sweep" && exit 1)
    rm -rf target/debug/incremental

fmt:
    cargo fmt --all

run *ARGS:
    cargo run -p theuth -- {{ARGS}}

# Release build of `t` + its helpers copied into ~/.cargo/bin (or $THEUTH_INSTALL_BIN). Not `cargo install`:
# that re-resolves (index + git fetch every time) and rebuilds in a scratch dir.
# The helpers land as theuth-app-parser-<hash> (46 §2.1) and theuth-documents-<version>
# (59 §3.2) beside `t`; older names are left in place for any still-running `t`.
install: helper documents-helper
    #!/usr/bin/env bash
    set -euo pipefail
    source scripts/app-parser-env.sh
    cargo build --release -p theuth
    bin="${THEUTH_INSTALL_BIN:-${CARGO_HOME:-$HOME/.cargo}/bin}"
    install -m 755 "$HELPER_SRC" "$bin/$HELPER.new" && mv -f "$bin/$HELPER.new" "$bin/$HELPER"
    install -m 755 "$DOC_HELPER_SRC" "$bin/$DOC_HELPER.new" && mv -f "$bin/$DOC_HELPER.new" "$bin/$DOC_HELPER"
    install -m 755 target/release/t "$bin/t.new" && mv -f "$bin/t.new" "$bin/t"
    echo "installed $(command -v t) + $bin/$HELPER + $bin/$DOC_HELPER"

# The theuth-app-parser-<hash> compatibility stub (crates/app-parser-stub, cargo; `t` validates apps
# in-process, docs/design/apps-inproc-as-built-P1.md) → target/release/theuth-app-parser-stub, checked
# against packages/apps/generated/library-hash.txt. Shipped only for installed updaters (46 §2.1).
helper:
    #!/usr/bin/env bash
    set -euo pipefail
    source scripts/app-parser-env.sh
    build_helper
    echo "built $HELPER_SRC ($HELPER)"

# The in-process OpenUI check/merge bundle: packages/apps/src/inproc.ts → crates/app-js/js/theuth-apps-inproc.js
# (scripts/build-inproc.ts), its golden transcript (scripts/record-inproc-golden.ts) and the inputs' hash in
# crates/app-js/js/BUNDLE.sha256. Needs bun; a cargo build never does (the bundle is committed).
apps-bundle:
    #!/usr/bin/env bash
    set -euo pipefail
    (cd packages/apps && bun install --frozen-lockfile && bun run scripts/build-inproc.ts && bun run scripts/record-inproc-golden.ts)
    cp packages/apps/dist/theuth-apps-inproc.js crates/app-js/js/theuth-apps-inproc.js
    {{just_executable()}} _apps-bundle-hash > crates/app-js/js/BUNDLE.sha256
    cat crates/app-js/js/BUNDLE.sha256

# Fails when packages/apps changed since the committed bundle was built (run `just apps-bundle`),
# or when the committed bundle is not the one recorded.
apps-bundle-check:
    #!/usr/bin/env bash
    set -euo pipefail
    if ! diff <({{just_executable()}} _apps-bundle-hash) crates/app-js/js/BUNDLE.sha256; then
        echo "crates/app-js/js is stale: run \`just apps-bundle\` and commit the result" >&2
        exit 1
    fi

# `inputs <sha256>`: over packages/apps/src/**, generated/*, package.json, bun.lock, tsconfig.json, patches/*,
# scripts/build-inproc.ts (sorted per-file sha256 lines, hashed); `bundle <sha256>`: the committed bundle.
# Shared by `apps-bundle` and `apps-bundle-check`.
_apps-bundle-hash:
    #!/usr/bin/env bash
    set -euo pipefail
    cd packages/apps
    inputs=$({ find src generated patches -type f | LC_ALL=C sort; printf '%s\n' package.json bun.lock tsconfig.json scripts/build-inproc.ts; } | while read -r f; do shasum -a 256 "$f"; done | shasum -a 256 | cut -d' ' -f1)
    bundle=$(shasum -a 256 ../../crates/app-js/js/theuth-apps-inproc.js | cut -d' ' -f1)
    printf 'inputs %s\nbundle %s\n' "$inputs" "$bundle"

# Standalone documents helper (bun build --compile) → packages/documents/dist/theuth-documents (59 §3.2);
# `--version` must print the PROTOCOL_VERSION of packages/documents/src/protocol.ts. Debug builds of `t`
# find it there; `install` and the package recipes copy it beside `t` as theuth-documents-<version>.
documents-helper:
    #!/usr/bin/env bash
    set -euo pipefail
    source scripts/app-parser-env.sh
    build_doc_helper
    echo "built $DOC_HELPER_SRC ($DOC_HELPER)"

# Export the real production contract and compile the shared desktop/iOS client.
ts: wire-schema
    cd crates/desktop/ui && bun install && bun run wire:generate

# Bindings for the desktop UI: `just ts` plus a copy into the Vite tree (25 §5).
ts-desktop: ts
    rm -rf crates/desktop/ui/src/rpc/bindings
    mkdir -p crates/desktop/ui/src/rpc/bindings
    cp -R bindings/typescript/. crates/desktop/ui/src/rpc/bindings/
    cp bindings/surface-catalog* bindings/methods.json crates/desktop/ui/src/rpc/bindings/

# Generate the production contract without starting plugins, ports or stores.
wire-schema:
    cargo test -p theuth --bin t --quiet export_bindings

# Deliberate acceptance after reviewing the classifier; no implicit CI update.
wire-schema-accept: wire-schema
    cp bindings/openrpc.json crates/theuth/tests/fixtures/wire/openrpc-v1.json

# Desktop app in dev mode against `target/debug/t` (or `$THEUTH_BIN`); `THEUTH_DATA_DIR` overrides ~/.theuth.
# `TAURI_FRONTEND_PATH`: the UI lives inside the crate (not beside it as Tauri assumes), so the
# before*Command cwd must be pinned.
desktop: ts-desktop documents-helper
    cargo build -p theuth
    cd crates/desktop/ui && bun install
    cd crates/desktop && TAURI_FRONTEND_PATH=ui THEUTH_BIN="${THEUTH_BIN:-$PWD/../../target/debug/t}" cargo tauri dev

# `theuth.app` + DMG: release sidecar, codesign, hdiutil (scripts/package-macos.sh).
# `externalBin` lives in tauri.bundle.conf.json so `cargo check` does not need the binary.
# Signing: $THEUTH_SIGN_IDENTITY or .env.signing, else ad-hoc.
package-macos:
    ./scripts/package-macos.sh

# Install/overwrite /Applications/theuth.app from the last package (or $THEUTH_APP).
install-macos:
    ./scripts/install-macos.sh

# Submit the packaged DMG to notarytool (needs THEUTH_NOTARY_* or .env.signing).
notarize-macos:
    ./scripts/notarize-macos.sh

# Update payloads from the notarized theuth.app: .app.tar.gz, CLI tar.gz, DMG → target/release/dist (39 §5.1).
package-update-macos:
    ./scripts/package-update-macos.sh

# Linux CLI tarball, built natively in Docker and staged at s3://theuth/staging/<ver>/ (39 §5.1).
# Run on an arm64 Mac for linux-aarch64 and on an x86_64 Linux box for linux-x86_64; never QEMU.
stage-linux:
    ./scripts/stage-linux.sh

# Linux desktop bundles (.deb, .rpm, .AppImage) with the `t` sidecar, staged at s3://theuth/staging/<ver>/
# beside the CLI tarball (scripts/package-linux.sh). Native only: run on a Linux box of the target arch.
# THEUTH_SKIP_UPLOAD=1 stops after target/release/dist.
package-linux:
    ./scripts/package-linux.sh

# Windows desktop installer (unsigned NSIS, per-user, onnxruntime.dll bundled) + CLI zip → target/release/dist
# (scripts/package-windows.ps1). Native only: run on the Windows box (pwsh 7, rustup MSVC, cargo-tauri, bun).
package-windows:
    pwsh -NoProfile -File scripts/package-windows.ps1

# Build the Windows or Linux release on a remote ssh host and pull theuth_<ver>_* into target/release/dist
# (scripts/build-remote.sh). The build runs detached on the host and is polled, so a dropped ssh does not
# kill it: `--no-wait` starts and returns, `--resume` re-attaches (dir = theuth-build/<short sha>);
# THEUTH_REMOTE_DRY_RUN=1 prints the ssh/scp commands. Release order:
#   just build-remote <win-host> windows      → theuth_<ver>_windows-x86_64-setup.exe + theuth_<ver>_windows-x86_64.zip
#   just build-remote <linux-host> linux      → theuth_<ver>_linux-x86_64.{AppImage,deb,rpm,tar.gz}
#   just package-macos → just notarize-macos → just package-update-macos → just release
#   a platform that finishes after the release went out: just release-update
# (linux-aarch64 CLI tarball: `just stage-linux` on this Mac, Docker; it lands in dist/ too.)
build-remote host os *ARGS:
    ./scripts/build-remote.sh {{host}} {{os}} {{ARGS}}

# Gates, sign, upload <ver>/, verify, latest.json, tap bump (39 §5.2-5.4). Needs THEUTH_S3_*/THEUTH_UPDATE_KEY in .env.signing.
# Windows/Linux artifacts are read from dist/ (just build-remote); staging/<ver>/ in the bucket is the fallback.
# THEUTH_SKIP_LINUX_SMOKE=1 when Docker is absent (gate 7 fails otherwise).
release-s3:
    ./scripts/release-s3.sh

# Gates for those files only, append SHA256SUMS + latest.json, tap re-rendered from the merged sums and
# committed only when it changed. Refuses objects already under <ver>/ unless byte-identical. Same env as release-s3.
# Add named platforms to the live version: linux-x86_64 | linux-aarch64 | windows-x86_64, or a comma list.
release-add platforms:
    THEUTH_RELEASE_ADD={{platforms}} ./scripts/release-s3.sh

# Every platform (linux-x86_64, linux-aarch64, windows-x86_64) in dist/ or staging/<ver>/ but missing from
# releases/<ver>/ or latest.json goes out in one pass (release-add with the list worked out for you); nothing
# missing → "up to date", exit 0. THEUTH_S3_DRY_RUN=1 just release-update shows what it would do. Same env as release-s3.
# Add every built platform the live version is missing (Windows, Linux x86_64 / aarch64).
release-update:
    THEUTH_RELEASE_ADD=auto ./scripts/release-s3.sh

# Every model file the daemon loads (pocket-tts, parakeet-tdt, nemotron-streaming-en, nemotron-diarization,
# pyannote/wespeaker, laya decision checkpoints) under ${THEUTH_DATA_DIR:-~/.theuth}/models: missing → fetched from
# its documented origin (HF_TOKEN for gated repos), present with a Hugging Face origin → compared with `main`
# (x-repo-commit / etag) and replaced when it changed. `--check` reports only (exit 2 when stale),
# `--force <path|all>` re-downloads. No AWS creds. MODELS_DRY_RUN=1 prints the plan.
fetch-models *ARGS:
    ./scripts/fetch-models.sh {{ARGS}}

# fetch-models (refreshing stale files), then every new-or-updated file + ONNX Runtime and its WebGPU plugin
# EP for every platform (osx-arm64, linux-x64, linux-aarch64, win-x64) → s3://theuth/releases/models/ (objects
# replaced in place) + models.json (size/sha256/revision), from one machine; the desktop app downloads what
# it lacks at start. Needs AWS_* in .env.signing. Same flags as fetch-models; MODELS_DRY_RUN=1 prints every
# fetch and every aws s3 cp plus the summary table without uploading.
release-models *ARGS:
    ./scripts/release-models.sh {{ARGS}}

# Annotated tag v<ver> pushed to origin. THEUTH_RELEASE_NO_PUSH=1 keeps it local.
tag-release:
    ./scripts/tag-release.sh

# Bump the product version (major | minor | patch | x.y.z): Cargo.toml, tauri.conf.json, Cargo.lock.
bump-version part:
    ./scripts/bump-version.sh {{part}}

# Full macOS release (39 §5.1): package, notarize, update payloads, publish, tag.
# Run `just build-remote <host> windows|linux` first so their artifacts are in dist/ (see build-remote).
# Models are not part of it: when REQUIRED / an origin in scripts/fetch-models.sh or a pin in
# scripts/ort-version.env changed since the last publish, run `just release-models` first
# (docs/update-setup.md §6), so the files the new app loads are in the bucket before it goes live.
release:
    just package-macos
    just notarize-macos
    just package-update-macos
    just release-s3
    just tag-release

# Signed IPA for iOS/iPadOS TestFlight (scripts/package-ios.sh).
# Signing: Apple Distribution + automatic provisioning. See .env.signing.
package-ios:
    ./scripts/package-ios.sh

# Upload the last IPA to App Store Connect (TestFlight). Needs APPLE_API_*.
testflight:
    ./scripts/upload-testflight.sh

# Build a signed IPA and upload it to TestFlight.
release-ios:
    just package-ios
    just testflight

# Alias of package-macos.
desktop-bundle: package-macos

# Desktop UI gates: types, unit tests, and the release build `package-macos` runs.
desktop-check:
    cd crates/desktop/ui && bun run check && bun run test && bun run build

# Regenerates crates/desktop/gen/apple — overwrites the committed Xcode project.
# Caps live in gen/apple/desktop_iOS/*. Do not run this for a normal build.
ios-init:
    cd crates/desktop && TAURI_FRONTEND_PATH=ui cargo tauri ios init --ci --config tauri.ios.conf.json

# Every raster icon from icons/icon.svg (the same mark everywhere): desktop PNGs +
# icns, the iOS AppIcon set (gen/apple is committed; `ios-init` resets it), and the
# menu-bar template from icons/tray.svg. App Store Connect rejects icons with an
# alpha channel; `tauri icon` keeps one, so the iOS set is flattened afterwards.
icons:
    cd crates/desktop && cargo tauri icon icons/icon.svg -o /tmp/theuth-icons --ios-color '#E8DCC8' \
      && cp /tmp/theuth-icons/{32x32,64x64,128x128,128x128@2x,icon}.png /tmp/theuth-icons/icon.icns icons/ \
      && cp /tmp/theuth-icons/ios/*.png gen/apple/Assets.xcassets/AppIcon.appiconset/ \
      && swift ../../scripts/flatten-png.swift gen/apple/Assets.xcassets/AppIcon.appiconset/*.png \
      && swift ../../scripts/svg2png.swift icons/tray.svg 22 icons/tray.png \
      && swift ../../scripts/svg2png.swift icons/tray.svg 44 icons/tray@2x.png

# iOS app in dev mode on a simulator/device. tauri.ios.conf.json overlays the bundle
# id, iOS 17 floor and the universal-link host: `plugins.deep-link.mobile[].host`
# must be the relay the operator serves /link from (relay.theuth.io by default).
ios:
    cd crates/desktop/ui && bun install
    cd crates/desktop && TAURI_FRONTEND_PATH=ui cargo tauri ios dev --config tauri.ios.conf.json

# App Store Connect archive. Alias of package-ios.
ios-bundle: package-ios

# Local relay for link development (31 §5); pair with `t link pair` against ws://127.0.0.1:8787.
relay *ARGS:
    cargo run -p relay -- --listen 127.0.0.1:8787 {{ARGS}}

# Static UI build for the browser container (31 §7.6), served by the relay.
web-build:
    cd crates/desktop/ui && bun install && bun run build
    @echo "serve it: theuth-relay --listen 0.0.0.0:8787 --web crates/desktop/ui/dist"

# Static theuth.io page, served by the relay's `--site` listener (port 8788).
site-build:
    cd site && bun install && bun run build
    @echo "serve it: theuth-relay --listen 0.0.0.0:8787 --site site/dist"

# Multi-arch (amd64+arm64) build+push. Needs buildx + ghcr login.
# SPA compiles once on the host arch (Dockerfile FROM --platform=$BUILDPLATFORM).
# Usage: just relay-docker-push
#        just relay-docker-push v0.1.0
relay-docker-push tag="":
    #!/usr/bin/env bash
    set -euo pipefail
    TAG="{{tag}}"
    if [ -z "$TAG" ]; then
      TAG="${THEUTH_RELAY_TAG:-$(git rev-parse --short HEAD 2>/dev/null || echo dev)}"
    fi
    IMAGE="${THEUTH_RELAY_IMAGE:-ghcr.io/skymoore/theuth/relay}"
    PLATFORM="${THEUTH_RELAY_PLATFORM:-linux/amd64,linux/arm64}"
    echo "==> buildx build+push ${IMAGE}:${TAG} (${PLATFORM})"
    docker buildx build \
      --platform "${PLATFORM}" \
      -f deploy/relay/Dockerfile \
      -t "${IMAGE}:${TAG}" \
      -t "${IMAGE}:latest" \
      --push \
      .
    echo "pushed ${IMAGE}:${TAG} and ${IMAGE}:latest"

# Multi-arch build+push of the upstream iroh-relay (design doc 67 §4.2) for the engine mesh.
# The image is the stock `iroh-relay` binary pinned to the workspace `iroh` line; the tag
# defaults to that version so the deployed relay always matches the crate engines link.
# Needs buildx + ghcr login. k8s base pulls ghcr.io/skymoore/theuth/iroh-relay:latest.
# Usage: just iroh-relay-docker-push
#        just iroh-relay-docker-push 1.2.0
iroh-relay-docker-push tag="":
    #!/usr/bin/env bash
    set -euo pipefail
    # Pinned iroh line is "1.2" (two components); only a full x.y.z pin overrides the default.
    PINNED="$( { grep -E '^iroh\s*=' Cargo.toml || true; } | { grep -oE '[0-9]+\.[0-9]+\.[0-9]+' || true; } | head -1)"
    VERSION="${THEUTH_IROH_RELAY_VERSION:-${PINNED:-1.2.0}}"
    TAG="{{tag}}"
    if [ -z "$TAG" ]; then TAG="$VERSION"; fi
    IMAGE="${THEUTH_IROH_RELAY_IMAGE:-ghcr.io/skymoore/theuth/iroh-relay}"
    PLATFORM="${THEUTH_RELAY_PLATFORM:-linux/amd64,linux/arm64}"
    # Fetch the official release tarballs on the host (containers behind a corporate proxy
    # cannot reach crates.io, and this needs no Rust toolchain). Cached per version.
    DL="deploy/relay/.iroh-relay"; mkdir -p "$DL"
    for triple in x86_64-unknown-linux-musl aarch64-unknown-linux-musl; do
      f="iroh-relay-v${VERSION}-${triple}.tar.gz"
      if [ ! -s "$DL/$f" ]; then
        echo "==> fetching $f"
        curl -fsSL -o "$DL/$f" "https://github.com/n0-computer/iroh/releases/download/v${VERSION}/${f}"
      fi
    done
    echo "==> buildx build+push ${IMAGE}:${TAG} (iroh-relay ${VERSION}, ${PLATFORM})"
    docker buildx build \
      --platform "${PLATFORM}" \
      -f deploy/relay/Dockerfile.iroh-relay \
      --build-arg IROH_RELAY_VERSION="${VERSION}" \
      -t "${IMAGE}:${TAG}" \
      -t "${IMAGE}:latest" \
      --push \
      .
    echo "pushed ${IMAGE}:${TAG} and ${IMAGE}:latest"

# Refresh the bundled model catalog: models.dev api.json trimmed to `models::ROUTABLE`
# providers and re-emitted through the crate's own types, so the field list lives in
# `crates/models/src/lib.rs` (`Model`), not here (design doc 02 §1).
models:
    curl -sL https://models.dev/api.json | cargo run -q -p models --bin trim > crates/models/data/models.json

# Fail when the committed catalog differs from upstream: drift; run `just models`.
models-check:
    #!/usr/bin/env bash
    set -euo pipefail
    tmp=$(mktemp)
    trap 'rm -f "$tmp"' EXIT
    curl -sL https://models.dev/api.json | cargo run -q -p models --bin trim > "$tmp"
    diff -q "$tmp" crates/models/data/models.json || { echo "catalog drift: run \`just models\`" >&2; exit 1; }

# Sandbox VM image (design doc 79 §6.1): buildx amd64+arm64 → ghcr.io/basedatum/theuth-vm:<tag>.
# Local by default; `just vm-image tag=2.3.3 --push` publishes.
vm-image tag push="":
    docker buildx build --platform linux/amd64,linux/arm64 \
      --build-arg THEUTH_PUBKEY="$(cat crates/update/pubkey.txt)" \
      -t ghcr.io/basedatum/theuth-vm:{{tag}} {{push}} deploy/vm

# The `base` stage only (debian:trixie-slim + systemd, getty masked): the bootstrap route's
# base image, ghcr.io/basedatum/theuth-vm-base:<tag>. `--push` publishes.
vm-image-base tag="trixie" push="":
    docker buildx build --platform linux/amd64,linux/arm64 --target base \
      -t ghcr.io/basedatum/theuth-vm-base:{{tag}} {{push}} deploy/vm

# Dev VM image for the host's arch with a local Linux `t` at deploy/vm/t, loaded into msb.
vm-image-dev:
    docker build --platform linux/$(arch) --target desktop -t theuth-vm:desktop deploy/vm
    docker build --platform linux/$(arch) -f deploy/vm/Dockerfile.dev -t theuth-vm:dev deploy/vm
    docker save theuth-vm:dev | msb load

# 130 W1 / 189: the macOS companion — `xcodebuild` (Release) + a hand-assembled, signed `Theuth Companion.app`
# under target/release/companion/ (scripts/bundle-companion.sh). Needs Xcode since 189: SwiftStreamingMarkdown has
# #Preview blocks, and only Xcode's resource accessor finds package bundles in Contents/Resources.
companion:
    scripts/bundle-companion.sh

# 134 W1: the Windows companion's portable half — Theuth.Companion.Rpc + Theuth.Companion.Core and their xUnit suites
# (`dotnet test` through crates/desktop/companion-windows/test.sh; .NET 10 SDK from ~/.dotnet or PATH). The WinUI app
# itself builds on Windows only (crates/desktop/companion-windows/build.ps1 on build-B).
companion-windows-test *args:
    crates/desktop/companion-windows/test.sh {{args}}
