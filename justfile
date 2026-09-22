# Rust toolchain lives under rustup but homebrew's rustup does not install
# cargo/rustc proxies on PATH. Every recipe prepends the active toolchain's bin.
toolchain_bin := `dirname "$(rustup which cargo)"`
export PATH := toolchain_bin + ":" + env("PATH")

# Print the toolchain bin dir; `export PATH="$(just toolchain):$PATH"` in any shell.
toolchain:
    @echo {{toolchain_bin}}

# Ensure the stable toolchain from rust-toolchain.toml is installed.
setup:
    rustup show active-toolchain || rustup toolchain install stable
    @echo "cargo: $(rustup which cargo)"

build *ARGS:
    cargo build {{ARGS}}

check *ARGS:
    cargo check --workspace --all-targets {{ARGS}}

test *ARGS:
    cargo test --workspace {{ARGS}}

clippy:
    cargo clippy --workspace --all-targets -- -D warnings

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

# Standalone app parser helper (bun build --compile) → packages/apps/dist/theuth-app-parser,
# checked against packages/apps/generated/library-hash.txt. THEUTH_SKIP_BUN_INSTALL=1 skips bun install.
helper:
    #!/usr/bin/env bash
    set -euo pipefail
    source scripts/app-parser-env.sh
    build_helper
    echo "built $HELPER_SRC ($HELPER)"

# Standalone documents helper (bun build --compile) → packages/documents/dist/theuth-documents (59 §3.2);
# `--version` must print the PROTOCOL_VERSION of packages/documents/src/protocol.ts. Debug builds of `t`
# find it there; `install` and the package recipes copy it beside `t` as theuth-documents-<version>.
documents-helper:
    #!/usr/bin/env bash
    set -euo pipefail
    source scripts/app-parser-env.sh
    build_doc_helper
    echo "built $DOC_HELPER_SRC ($DOC_HELPER)"

# Export TypeScript bindings for the RPC wire types.
ts:
    cargo test -p rpc --features ts --quiet export_bindings

# Bindings for the desktop UI: `just ts` plus a copy into the Vite tree (25 §5).
ts-desktop: ts
    rm -rf crates/desktop/ui/src/rpc/bindings
    mkdir -p crates/desktop/ui/src/rpc/bindings
    cp -R bindings/. crates/desktop/ui/src/rpc/bindings/

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

# Gates, sign, upload <ver>/, verify, latest.json, tap bump (39 §5.2-5.4). Needs THEUTH_S3_*/THEUTH_UPDATE_KEY in .env.signing.
release-s3:
    ./scripts/release-s3.sh

# Speech model files (pocket-tts, parakeet-tdt, nemotron-streaming-en, pyannote) from
# ${THEUTH_DATA_DIR:-~/.theuth}/models → s3://theuth/releases/models/ + models.json; the
# desktop app downloads what a Mac lacks at start. Needs AWS_* in .env.signing.
release-models:
    ./scripts/release-models.sh

# Annotated tag v<ver> pushed to origin. THEUTH_RELEASE_NO_PUSH=1 keeps it local.
tag-release:
    ./scripts/tag-release.sh

# Bump the product version (major | minor | patch | x.y.z): Cargo.toml, tauri.conf.json, Cargo.lock.
bump-version part:
    ./scripts/bump-version.sh {{part}}

# Full macOS release (39 §5.1): package, notarize, update payloads, publish, tag.
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

# Desktop UI gates: types, unit tests.
desktop-check:
    cd crates/desktop/ui && bun run check && bun run test

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
